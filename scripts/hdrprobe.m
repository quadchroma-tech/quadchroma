// hdrprobe - Messwerkzeug fuer den Mac-Host (HDR-Plan, Schritt G0).
//
// Was liefert ScreenCaptureKit wirklich, wenn der Host HDR aufnimmt? Davon
// haengt ab, wie QuadChroma HDR10 sendet (host/main.m, aufnahme_farbe_setzen
// und hdr_sdr_weiss_nit). Das Werkzeug laeuft ausserhalb der App, nimmt den
// Bildschirm ein paar Sekunden auf und schreibt einen Bericht nach stdout:
//
//   - EDR-Kopfraum je Bildschirm (NSScreen), Farbraum, SCDisplay-Kennungen
//   - fuer jede Aufnahmeart (SDR; HDR kanonisch und lokal, je mit Display P3
//     PQ + Matrix BT.709 und mit BT.2100 PQ + Matrix BT.2020; Apples
//     Voreinstellung HDRStreamCanonicalDisplay): ob der Strom startet,
//     Pixelformat, Anhaenge (Primaerfarben, Transfer, Matrix), der Y-Code
//     eines weissen SDR-Fensters (das das Werkzeug selbst oeffnet) und daraus
//     das SDR-Weiss in nit, Hoechst- und 99,9-%-Wert des ganzen Bildes
//   - ob updateConfiguration im laufenden Strom von SDR nach HDR und zurueck
//     umstellt (so wechselt der Host die Farbe)
//   - der Weg durch VideoToolbox wie im Host: ein HDR-Bild als HEVC 4:4:4
//     10 Bit mit BT.2020/PQ codieren, decodieren, das weisse Fenster messen
//
// Aufrufe (Liste ohne Aufnahme; alles andere braucht die Freigabe
// "Bildschirmaufnahme" fuer das Terminal):
//   hdrprobe                        Bildschirme und EDR-Werte, keine Aufnahme
//   hdrprobe messen [N]             alle Messungen auf Bildschirm N (Platz in der
//                                   Liste, Vorgabe: Hauptbildschirm)
//   hdrprobe spitze [N] [sekunden]  hoechster Wert ueber die Zeit (Vorgabe 20 s) -
//                                   dabei ein HDR-Video abspielen
//   hdrprobe beobachten [sekunden]  meldet Bildschirm-Ereignisse und EDR-Werte
//                                   (Vorgabe 60 s) - dabei HDR an- und ausschalten
//
// Bauen und starten: scripts/hdrprobe.sh (legt build/hdrprobe an und schreibt
// den Bericht zusaetzlich in eine Datei), oder von Hand:
//   clang -fobjc-arc -O2 -Wall -mmacosx-version-min=15.0 -framework Foundation \
//         -framework AppKit -framework ScreenCaptureKit -framework VideoToolbox \
//         -framework CoreMedia -framework CoreVideo -framework CoreGraphics \
//         scripts/hdrprobe.m -o build/hdrprobe
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#include <dlfcn.h>
#include <math.h>
#include <sys/sysctl.h>

// ------------------------------------------------------------------ Hilfen

static void zeile(NSString *fmt, ...) NS_FORMAT_FUNCTION(1, 2);
static void zeile(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    printf("%s\n", s.UTF8String);
    fflush(stdout);
}

static NSString *fourcc(OSType t) {
    char b[5] = { (char)(t >> 24), (char)(t >> 16), (char)(t >> 8), (char)t, 0 };
    return [NSString stringWithUTF8String:b] ?: [NSString stringWithFormat:@"0x%08x", t];
}

static NSString *cf_text(CFTypeRef v) {
    if (!v) return @"-";
    if (CFGetTypeID(v) == CFStringGetTypeID()) return (__bridge NSString *)v;
    return [(__bridge id)v description];
}

// SMPTE ST 2084 (wie client/src/hdr.rs).
static double pq_nit(double e) {
    const double m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875;
    if (!(e > 0)) return 0;
    if (e > 1) e = 1;
    double p = pow(e, 1 / m2), z = fmax(p - c1, 0);
    return pow(z / (c2 - c3 * p), 1 / m1) * 10000.0;
}

static int ist_pq(CVPixelBufferRef pb) {
    CFTypeRef t = CVBufferCopyAttachment(pb, kCVImageBufferTransferFunctionKey, NULL);
    int pq = t && CFEqual(t, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ);
    if (t) CFRelease(t);
    return pq;
}

static NSString *anhaenge(CVPixelBufferRef pb) {
    CFStringRef keys[3] = { kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrixKey };
    NSMutableArray *w = [NSMutableArray array];
    for (int i = 0; i < 3; i++) {
        CFTypeRef v = CVBufferCopyAttachment(pb, keys[i], NULL);
        [w addObject:cf_text(v)];
        if (v) CFRelease(v);
    }
    NSString *cs = @"-";
    CFTypeRef c = CVBufferCopyAttachment(pb, kCVImageBufferCGColorSpaceKey, NULL);
    if (c && CFGetTypeID(c) == CGColorSpaceGetTypeID()) {
        CFStringRef n = CGColorSpaceCopyName((CGColorSpaceRef)c);
        cs = n ? [(__bridge NSString *)n copy] : @"(ohne Namen)";
        if (n) CFRelease(n);
    }
    if (c) CFRelease(c);
    return [NSString stringWithFormat:@"%@ %zux%zu, Primaerfarben %@, Transfer %@, Matrix %@, Farbraum %@",
            fourcc(CVPixelBufferGetPixelFormatType(pb)), CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb), w[0], w[1], w[2], cs];
}

// ----------------------------------------------------------- Bildschirme

typedef struct {
    CGDirectDisplayID id_;
    CGRect punkte;           // NSScreen.frame (Punkte, Ursprung unten links)
    size_t px_w, px_h;       // Pixel des Anzeigemodus
    double edr_pot, edr_akt, edr_ref;
    char name[128];
} schirm;

static schirm g_schirme[16];
static int g_n_schirme = 0;

// Auf dem Hauptfaden.
static void schirme_lesen(void) {
    g_n_schirme = 0;
    for (NSScreen *s in [NSScreen screens]) {
        if (g_n_schirme >= 16) break;
        schirm *x = &g_schirme[g_n_schirme++];
        memset(x, 0, sizeof *x);
        x->id_ = [s.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
        x->punkte = s.frame;
        CGDisplayModeRef m = CGDisplayCopyDisplayMode(x->id_);
        x->px_w = m ? CGDisplayModeGetPixelWidth(m) : 0;
        x->px_h = m ? CGDisplayModeGetPixelHeight(m) : 0;
        if (m) CGDisplayModeRelease(m);
        x->edr_pot = s.maximumPotentialExtendedDynamicRangeColorComponentValue;
        x->edr_akt = s.maximumExtendedDynamicRangeColorComponentValue;
        x->edr_ref = s.maximumReferenceExtendedDynamicRangeColorComponentValue;
        strlcpy(x->name, s.localizedName.UTF8String ?: "?", sizeof x->name);
    }
}

static void schirme_zeigen(void) {
    CGDirectDisplayID haupt = CGMainDisplayID();
    for (int i = 0; i < g_n_schirme; i++) {
        schirm *x = &g_schirme[i];
        CGColorSpaceRef cs = CGDisplayCopyColorSpace(x->id_);
        CFStringRef n = cs ? CGColorSpaceCopyName(cs) : NULL;
        zeile(@"  [%d] \"%s\" id=%u%@, %zux%zu Pixel (%.0fx%.0f Punkte), EDR potentiell %.3f, aktuell %.3f, Referenz %.3f, "
               "Farbraum %@%@", i, x->name, x->id_, x->id_ == haupt ? @" (Hauptbildschirm)" : @"", x->px_w, x->px_h,
              x->punkte.size.width, x->punkte.size.height, x->edr_pot, x->edr_akt, x->edr_ref, cf_text(n),
              x->edr_pot > 1.0 ? @"  -> fuer QuadChroma HDR-faehig" : @"  -> fuer QuadChroma SDR");
        if (n) CFRelease(n);
        if (cs) CGColorSpaceRelease(cs);
    }
}

// ---------------------------------------------------------------- Aufnahme

@interface Fang : NSObject <SCStreamOutput, SCStreamDelegate>
@property (atomic) int gewuenscht;               // so viele vollstaendige Bilder noch aufheben
@property (atomic) CVPixelBufferRef letztes;     // +1, gehoert Fang
@property (atomic, copy) void (^je_bild)(CVPixelBufferRef);
@property (atomic, strong) dispatch_semaphore_t da;
@property (atomic, copy) NSString *fehler;
@end

@implementation Fang
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    CFArrayRef arr = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (arr && CFArrayGetCount(arr) > 0) {
        CFNumberRef st = CFDictionaryGetValue(CFArrayGetValueAtIndex(arr, 0), (__bridge CFStringRef)SCStreamFrameInfoStatus);
        int v = -1;
        if (st) CFNumberGetValue(st, kCFNumberIntType, &v);
        if (v != SCFrameStatusComplete) return;
    }
    CVPixelBufferRef pb = CMSampleBufferGetImageBuffer(sb);
    if (!pb) return;
    if (self.je_bild) self.je_bild(pb);
    if (self.gewuenscht <= 0) return;
    CVPixelBufferRef alt = self.letztes;
    self.letztes = CVPixelBufferRetain(pb);
    if (alt) CVPixelBufferRelease(alt);
    if (--self.gewuenscht == 0 && self.da) dispatch_semaphore_signal(self.da);
}
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    self.fehler = error.localizedDescription;
    if (self.da) dispatch_semaphore_signal(self.da);
}
@end

static SCDisplay *g_sc = nil;
static schirm *g_ziel = NULL;
static CGRect g_fenster_px;          // das weisse Fenster in Pixeln der Aufnahme (Ursprung oben links)
static NSWindow *g_fenster = nil;

typedef struct {
    const char *name;
    int preset;                      // 1 = Apples HDRStreamCanonicalDisplay
    SCCaptureDynamicRange dr;
    CFStringRef farbraum, matrix;    // NULL = Vorgabe
} art;

static SCStreamConfiguration *konfiguration(const art *a) {
    SCStreamConfiguration *c = a->preset ? [SCStreamConfiguration streamConfigurationWithPreset:SCStreamConfigurationPresetCaptureHDRStreamCanonicalDisplay]
                                         : [[SCStreamConfiguration alloc] init];
    c.width = g_ziel->px_w;
    c.height = g_ziel->px_h;
    c.showsCursor = NO;
    c.queueDepth = 6;
    c.minimumFrameInterval = CMTimeMake(1, 30);
    c.captureResolution = SCCaptureResolutionBest;
    if (!a->preset) {
        c.pixelFormat = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
        c.captureDynamicRange = a->dr;
        if (a->farbraum) c.colorSpaceName = a->farbraum;
        if (a->matrix) c.colorMatrix = a->matrix;
    }
    return c;
}

// Luma-Code (auf 10 Bit) an (x, y), Formate xf44/xf20/x444/x420 (10 Bit
// oben buendig) und 420f/420v (8 Bit). -1 = Format unbekannt.
static int luma(CVPixelBufferRef pb, const uint8_t *basis, size_t bpr, int x, int y) {
    OSType f = CVPixelBufferGetPixelFormatType(pb);
    if (f == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange || f == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
        return basis[(size_t)y * bpr + x] << 2;
    if (f == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange || f == kCVPixelFormatType_420YpCbCr10BiPlanarFullRange ||
        f == kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange || f == kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange)
        return ((const uint16_t *)(basis + (size_t)y * bpr))[x] >> 6;
    return -1;
}

typedef struct { int fenster_y, max_y, p999_y, ok; } messung;

// Das weisse Fenster (Median in der inneren Haelfte) und das ganze Bild.
static messung bild_messen(CVPixelBufferRef pb) {
    messung m = { -1, -1, -1, 0 };
    if (!pb || CVPixelBufferGetPlaneCount(pb) < 1) return m;
    CVPixelBufferLockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    const uint8_t *b = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
    size_t bpr = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    int w = (int)CVPixelBufferGetWidthOfPlane(pb, 0), h = (int)CVPixelBufferGetHeightOfPlane(pb, 0);
    if (luma(pb, b, bpr, 0, 0) >= 0) {
        uint32_t hist[1024];
        memset(hist, 0, sizeof hist);
        for (int y = 0; y < h; y++)
            for (int x = 0; x < w; x++) hist[luma(pb, b, bpr, x, y)]++;
        uint64_t n = (uint64_t)w * h, summe = 0;
        for (int v = 1023; v >= 0; v--) if (hist[v]) { m.max_y = v; break; }
        for (int v = 1023; v >= 0; v--) { summe += hist[v]; if (summe * 1000 >= n) { m.p999_y = v; break; } }
        // Fenster: innere Haelfte, auf das Bild skaliert (die Aufnahme kann kleiner sein).
        double sx = (double)w / (double)g_ziel->px_w, sy = (double)h / (double)g_ziel->px_h;
        CGRect f = g_fenster_px;
        int x0 = (int)((f.origin.x + f.size.width / 4) * sx), x1 = (int)((f.origin.x + 3 * f.size.width / 4) * sx);
        int y0 = (int)((f.origin.y + f.size.height / 4) * sy), y1 = (int)((f.origin.y + 3 * f.size.height / 4) * sy);
        if (x0 >= 0 && y0 >= 0 && x1 <= w && y1 <= h && x1 > x0 && y1 > y0) {
            memset(hist, 0, sizeof hist);
            uint64_t k = 0;
            for (int y = y0; y < y1; y++)
                for (int x = x0; x < x1; x++) { hist[luma(pb, b, bpr, x, y)]++; k++; }
            summe = 0;
            for (int v = 0; v < 1024; v++) { summe += hist[v]; if (summe * 2 >= k) { m.fenster_y = v; break; } }
        }
        m.ok = 1;
    }
    CVPixelBufferUnlockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    return m;
}

static NSString *code_text(int y, int pq) {
    if (y < 0) return @"-";
    return pq ? [NSString stringWithFormat:@"%d (%.1f nit)", y, pq_nit(y / 1023.0)]
              : [NSString stringWithFormat:@"%d (%.1f %% SDR)", y, y / 10.23];
}

// Strom mit Konfiguration c starten, n vollstaendige Bilder abwarten.
// Rueckgabe: der Strom (laeuft weiter) oder nil mit Zeile.
static SCStream *strom_starten(SCStreamConfiguration *c, Fang *fang, dispatch_queue_t q, int n) {
    SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:g_sc excludingWindows:@[]];
    SCStream *st = [[SCStream alloc] initWithFilter:filter configuration:c delegate:fang];
    NSError *e = nil;
    if (![st addStreamOutput:fang type:SCStreamOutputTypeScreen sampleHandlerQueue:q error:&e]) {
        zeile(@"    Ausgabe nicht anmeldbar: %@", e.localizedDescription);
        return nil;
    }
    fang.da = dispatch_semaphore_create(0);
    fang.gewuenscht = n;
    fang.fehler = nil;
    __block NSError *start_fehler = nil;
    dispatch_semaphore_t s = dispatch_semaphore_create(0);
    [st startCaptureWithCompletionHandler:^(NSError *x) { start_fehler = x; dispatch_semaphore_signal(s); }];
    if (dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC)) != 0) {
        zeile(@"    Start meldet sich nicht (10 s)");
        return nil;
    }
    if (start_fehler) {
        zeile(@"    Start misslungen: %@ (Code %ld)", start_fehler.localizedDescription, (long)start_fehler.code);
        return nil;
    }
    // Ein stiller Bildschirm liefert nur bei Aenderungen: das Fenster kurz
    // umfaerben und zurueck, damit sicher Bilder kommen.
    dispatch_async(dispatch_get_main_queue(), ^{
        g_fenster.backgroundColor = [NSColor colorWithWhite:0.999 alpha:1];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 150 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            g_fenster.backgroundColor = NSColor.whiteColor;
        });
    });
    if (dispatch_semaphore_wait(fang.da, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0)
        zeile(@"    nach 5 s noch keine %d Bilder%@", n, fang.fehler ? [NSString stringWithFormat:@" (%@)", fang.fehler] : @"");
    return st;
}

static void strom_halten(SCStream *st) {
    if (!st) return;
    dispatch_semaphore_t s = dispatch_semaphore_create(0);
    [st stopCaptureWithCompletionHandler:^(NSError *x) { (void)x; dispatch_semaphore_signal(s); }];
    dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
}

// Das letzte Bild melden und messen; *weiss_nit bekommt das SDR-Weiss in
// nit (nur bei PQ), sonst -1.
static void bild_melden(Fang *fang, double *weiss_nit, double *max_nit) {
    CVPixelBufferRef pb = fang.letztes;
    if (weiss_nit) *weiss_nit = -1;
    if (max_nit) *max_nit = -1;
    if (!pb) { zeile(@"    kein Bild"); return; }
    int pq = ist_pq(pb);
    messung m = bild_messen(pb);
    zeile(@"    Bild: %@", anhaenge(pb));
    if (!m.ok) { zeile(@"    (Pixelformat %@ wird nicht vermessen)", fourcc(CVPixelBufferGetPixelFormatType(pb))); return; }
    zeile(@"    weisses SDR-Fenster Y %@ | ganzes Bild: hoechster Y %@, 99,9 %% bis %@", code_text(m.fenster_y, pq),
          code_text(m.max_y, pq), code_text(m.p999_y, pq));
    if (pq && weiss_nit && m.fenster_y >= 0) *weiss_nit = pq_nit(m.fenster_y / 1023.0);
    if (pq && max_nit && m.max_y >= 0) *max_nit = pq_nit(m.max_y / 1023.0);
}

// ------------------------------------------------- VideoToolbox wie im Host

static CMSampleBufferRef g_vt_probe = NULL;
static void vt_enc(void *r, void *s, OSStatus st, VTEncodeInfoFlags f, CMSampleBufferRef sb) {
    (void)r; (void)s; (void)f;
    if (st == noErr && sb && !g_vt_probe) g_vt_probe = (CMSampleBufferRef)CFRetain(sb);
}
static CVPixelBufferRef g_vt_bild = NULL;
static void vt_dec(void *r, void *s, OSStatus st, VTDecodeInfoFlags f, CVImageBufferRef pb, CMTime a, CMTime b) {
    (void)r; (void)s; (void)f; (void)a; (void)b;
    if (st == noErr && pb && !g_vt_bild) g_vt_bild = CVPixelBufferRetain(pb);
}

// Ein HDR-Bild der Aufnahme wie der Host codieren (HEVC 4:4:4 10 Bit,
// BT.2020/PQ/BT.2020, Mastering P3 1000 nit) und wie der Client decodieren.
static void vt_rundreise(CVPixelBufferRef pb) {
    NSDictionary *spec = @{ (id)kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: @YES };
    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, (int32_t)CVPixelBufferGetWidth(pb), (int32_t)CVPixelBufferGetHeight(pb),
                                             kCMVideoCodecType_HEVC, (__bridge CFDictionaryRef)spec, NULL, NULL, vt_enc, NULL, &s);
    if (st != noErr || !s) { zeile(@"    VideoToolbox: keine Encoder-Sitzung (%d)", (int)st); return; }
    CFStringRef *profil = (CFStringRef *)dlsym(RTLD_DEFAULT, "kVTProfileLevel_HEVC_Main44410_AutoLevel");
    OSStatus p = profil ? VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, *profil) : -1;
    VTSessionSetProperty(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    OSStatus c1 = VTSessionSetProperty(s, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_2020);
    OSStatus c2 = VTSessionSetProperty(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ);
    OSStatus c3 = VTSessionSetProperty(s, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_2020);
    uint8_t mdcv[24] = { 0x33, 0xc2, 0x86, 0xc4, 0x1d, 0x4c, 0x0b, 0xb8, 0x84, 0xd0, 0x3e, 0x80, 0x3d, 0x13, 0x40, 0x42,
                         0x00, 0x98, 0x96, 0x80, 0x00, 0x00, 0x00, 0x32 };
    CFDataRef d = CFDataCreate(NULL, mdcv, sizeof mdcv);
    OSStatus c4 = VTSessionSetProperty(s, kVTCompressionPropertyKey_MasteringDisplayColorVolume, d);
    CFRelease(d);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_AverageBitRate, (__bridge CFNumberRef)@(150000000));
    VTCompressionSessionPrepareToEncodeFrames(s);
    zeile(@"    Encoder: Profil %d, BT.2020 %d, PQ %d, Matrix 2020 %d, Mastering %d (0 = angenommen)", (int)p, (int)c1, (int)c2, (int)c3, (int)c4);
    if (g_vt_probe) { CFRelease(g_vt_probe); g_vt_probe = NULL; }
    st = VTCompressionSessionEncodeFrame(s, pb, CMTimeMake(0, 60), kCMTimeInvalid, NULL, NULL, NULL);
    VTCompressionSessionCompleteFrames(s, kCMTimeInvalid);
    VTCompressionSessionInvalidate(s);
    CFRelease(s);
    if (st != noErr || !g_vt_probe) { zeile(@"    VideoToolbox: Codieren misslungen (%d)", (int)st); return; }
    CMFormatDescriptionRef fd = CMSampleBufferGetFormatDescription(g_vt_probe);
    NSDictionary *aus = @{ (id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_444YpCbCr10BiPlanarFullRange) };
    VTDecompressionOutputCallbackRecord cb = { vt_dec, NULL };
    VTDecompressionSessionRef ds = NULL;
    if (g_vt_bild) { CVPixelBufferRelease(g_vt_bild); g_vt_bild = NULL; }
    if (VTDecompressionSessionCreate(NULL, fd, NULL, (__bridge CFDictionaryRef)aus, &cb, &ds) == noErr && ds) {
        VTDecompressionSessionDecodeFrame(ds, g_vt_probe, 0, NULL, NULL);
        VTDecompressionSessionWaitForAsynchronousFrames(ds);
        VTDecompressionSessionInvalidate(ds);
        CFRelease(ds);
    }
    if (!g_vt_bild) { zeile(@"    VideoToolbox: Decodieren misslungen"); return; }
    messung m = bild_messen(g_vt_bild);
    zeile(@"    nach Codieren und Decodieren: %@", anhaenge(g_vt_bild));
    zeile(@"    weisses SDR-Fenster Y %@ | hoechster Y %@", code_text(m.fenster_y, 1), code_text(m.max_y, 1));
}

// ------------------------------------------------------------ Messungen

static int platz_waehlen(int wunsch) {
    if (wunsch >= 0 && wunsch < g_n_schirme) return wunsch;
    for (int i = 0; i < g_n_schirme; i++) if (g_schirme[i].id_ == CGMainDisplayID()) return i;
    return 0;
}

// SCDisplay zum Ziel (auf einem Nebenfaden: blockiert bis zu 10 s).
static int sc_suchen(void) {
    __block NSArray<SCDisplay *> *liste = nil;
    __block NSError *fehler = nil;
    dispatch_semaphore_t s = dispatch_semaphore_create(0);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
        liste = c.displays;
        fehler = e;
        dispatch_semaphore_signal(s);
    }];
    dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));
    if (fehler) zeile(@"ScreenCaptureKit: %@", fehler.localizedDescription);
    for (SCDisplay *d in liste) if (d.displayID == g_ziel->id_) g_sc = d;
    zeile(@"ScreenCaptureKit: %lu Bildschirm(e): %@", (unsigned long)liste.count, [[liste valueForKey:@"displayID"] componentsJoinedByString:@", "]);
    return g_sc != nil;
}

// Auf dem Hauptfaden: das weisse SDR-Fenster mitten auf den Zielbildschirm.
static void fenster_oeffnen(void) {
    NSScreen *ns = nil;
    for (NSScreen *s in [NSScreen screens])
        if ([s.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == g_ziel->id_) ns = s;
    NSRect sf = ns ? ns.frame : NSMakeRect(0, 0, 1280, 800);
    NSRect f = NSMakeRect(NSMidX(sf) - 300, NSMidY(sf) - 200, 600, 400);
    g_fenster = [[NSWindow alloc] initWithContentRect:f styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    g_fenster.backgroundColor = NSColor.whiteColor;
    g_fenster.level = NSFloatingWindowLevel;
    g_fenster.hasShadow = NO;
    g_fenster.releasedWhenClosed = NO;
    [g_fenster orderFrontRegardless];
    // In Pixeln der Aufnahme, Ursprung oben links.
    double sx = g_ziel->px_w / sf.size.width, sy = g_ziel->px_h / sf.size.height;
    g_fenster_px = CGRectMake((f.origin.x - sf.origin.x) * sx, (NSMaxY(sf) - NSMaxY(f)) * sy, f.size.width * sx, f.size.height * sy);
}

static void messen(void) {
    zeile(@"\n== Messung auf [%ld] \"%s\" (id %u, %zux%zu Pixel, EDR potentiell %.3f, aktuell %.3f)", (long)(g_ziel - g_schirme),
          g_ziel->name, g_ziel->id_, g_ziel->px_w, g_ziel->px_h, g_ziel->edr_pot, g_ziel->edr_akt);
    zeile(@"   Weisses SDR-Fenster: %.0fx%.0f Pixel ab (%.0f, %.0f)", g_fenster_px.size.width, g_fenster_px.size.height,
          g_fenster_px.origin.x, g_fenster_px.origin.y);
    art arten[] = {
        { "SDR (sRGB, xf44) - heute", 0, SCCaptureDynamicRangeSDR, kCGColorSpaceSRGB, NULL },
        { "HDR kanonisch, Display P3 PQ, Matrix BT.709 (Vorgabe von QuadChroma)", 0, SCCaptureDynamicRangeHDRCanonicalDisplay, kCGColorSpaceDisplayP3_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2 },
        { "HDR lokal, Display P3 PQ, Matrix BT.709", 0, SCCaptureDynamicRangeHDRLocalDisplay, kCGColorSpaceDisplayP3_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2 },
        { "HDR kanonisch, BT.2100 PQ, Matrix BT.2020", 0, SCCaptureDynamicRangeHDRCanonicalDisplay, kCGColorSpaceITUR_2100_PQ, kCVImageBufferYCbCrMatrix_ITU_R_2020 },
        { "HDR lokal, BT.2100 PQ, Matrix BT.2020", 0, SCCaptureDynamicRangeHDRLocalDisplay, kCGColorSpaceITUR_2100_PQ, kCVImageBufferYCbCrMatrix_ITU_R_2020 },
        { "Apples Voreinstellung HDRStreamCanonicalDisplay", 1, 0, NULL, NULL },
    };
    const int n_arten = (int)(sizeof arten / sizeof arten[0]);
    double weiss[8], spitze[8];
    int laeuft[8];
    dispatch_queue_t q = dispatch_queue_create("hdrprobe.aufnahme", DISPATCH_QUEUE_SERIAL);
    CVPixelBufferRef hdr_bild = NULL;
    for (int i = 0; i < n_arten; i++) {
        zeile(@"\n-- %s", arten[i].name);
        Fang *fang = [[Fang alloc] init];
        SCStreamConfiguration *c = konfiguration(&arten[i]);
        zeile(@"    Konfiguration: Dynamikumfang %ld, Pixelformat %@, Farbraum %@, Matrix %@", (long)c.captureDynamicRange,
              fourcc(c.pixelFormat), cf_text(c.colorSpaceName), cf_text(c.colorMatrix));
        SCStream *st = strom_starten(c, fang, q, 3);
        laeuft[i] = st && fang.letztes;
        bild_melden(fang, &weiss[i], &spitze[i]);
        if (i == 1 && fang.letztes && ist_pq(fang.letztes)) hdr_bild = CVPixelBufferRetain(fang.letztes);
        strom_halten(st);
        if (fang.letztes) CVPixelBufferRelease(fang.letztes);
    }

    zeile(@"\n-- Umstellen im laufenden Strom (updateConfiguration): SDR -> HDR kanonisch P3 -> SDR");
    Fang *fang = [[Fang alloc] init];
    SCStreamConfiguration *c = konfiguration(&arten[0]);
    SCStream *st = strom_starten(c, fang, q, 2);
    int um_ok = 0;
    if (st) {
        bild_melden(fang, NULL, NULL);
        for (int schritt = 1; schritt >= 0; schritt--) {
            const art *a = &arten[schritt];
            c.captureDynamicRange = a->dr;
            c.colorSpaceName = a->farbraum;
            c.colorMatrix = a->matrix ?: kCVImageBufferYCbCrMatrix_ITU_R_709_2;
            __block NSError *ue = nil;
            dispatch_semaphore_t s = dispatch_semaphore_create(0);
            fang.gewuenscht = 0;
            [st updateConfiguration:c completionHandler:^(NSError *x) { ue = x; dispatch_semaphore_signal(s); }];
            dispatch_semaphore_wait(s, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
            zeile(@"    nach %s: updateConfiguration %@", schritt ? "HDR" : "SDR", ue ? ue.localizedDescription : @"ohne Fehler");
            // Die ersten Bilder danach koennen noch aus der alten Einstellung sein.
            [NSThread sleepForTimeInterval:0.3];
            fang.da = dispatch_semaphore_create(0);
            fang.gewuenscht = 3;
            dispatch_async(dispatch_get_main_queue(), ^{
                g_fenster.backgroundColor = [NSColor colorWithWhite:0.999 alpha:1];
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 150 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                    g_fenster.backgroundColor = NSColor.whiteColor;
                });
            });
            dispatch_semaphore_wait(fang.da, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
            bild_melden(fang, NULL, NULL);
            if (!ue && fang.letztes && ist_pq(fang.letztes) == schritt) um_ok++;
        }
        strom_halten(st);
        if (fang.letztes) CVPixelBufferRelease(fang.letztes);
    }

    zeile(@"\n-- VideoToolbox wie im Host (HDR kanonisch P3 -> HEVC 4:4:4 10 Bit BT.2020/PQ -> zurueck)");
    if (hdr_bild) {
        vt_rundreise(hdr_bild);
        CVPixelBufferRelease(hdr_bild);
    } else {
        zeile(@"    kein HDR-Bild aus der kanonischen P3-Aufnahme - entfaellt");
    }

    zeile(@"\n== Zusammenfassung fuer QuadChroma (HDR-Plan G0)");
    zeile(@"   Bildschirm: EDR potentiell %.3f -> %@", g_ziel->edr_pot,
          g_ziel->edr_pot > 1.0 ? @"QuadChroma nimmt ihn als HDR-faehig (HDR10 moeglich)" : @"QuadChroma nimmt ihn als SDR (kein HDR10)");
    for (int i = 1; i < n_arten; i++)
        zeile(@"   %-70s %@%@", arten[i].name, laeuft[i] ? @"laeuft" : @"LAEUFT NICHT",
              weiss[i] >= 0 ? [NSString stringWithFormat:@", SDR-Weiss %.1f nit, hoechster Wert %.1f nit", weiss[i], spitze[i]] : @"");
    zeile(@"   Umstellen SDR <-> HDR im laufenden Strom: %@", um_ok == 2 ? @"geht" : @"GEHT NICHT (oder unklar, Zeilen oben)");
    if (laeuft[1] && weiss[1] > 0) {
        long w = lround(weiss[1]);
        zeile(@"   SDR-Weiss der Vorgabe (kanonisch, P3): %ld nit - QuadChroma meldet 203 nit%@", w,
              labs(w - 203) <= 5 ? @" (passt)" : [NSString stringWithFormat:@" -> zum Pruefen QC_HDR_SDR_WEISS=%ld setzen", w]);
    }
    if (!laeuft[1]) {
        for (int i = 2; i < 5; i++)
            if (laeuft[i]) {
                zeile(@"   Die Vorgabe laeuft nicht, \"%s\" schon: QC_HDR_AUFNAHME=%s", arten[i].name,
                      i == 2 ? "lokal-p3" : i == 3 ? "kanonisch-2100" : "lokal-2100");
                break;
            }
    }
    zeile(@"   Fuer Spitzen ueber dem SDR-Weiss: hdrprobe spitze (dabei ein HDR-Video abspielen).");
}

static void spitze(double sekunden) {
    zeile(@"\n== Spitze auf [%ld] \"%s\" ueber %.0f s (HDR kanonisch, Display P3 PQ) - jetzt ein HDR-Video abspielen",
          (long)(g_ziel - g_schirme), g_ziel->name, sekunden);
    art a = { "HDR kanonisch P3", 0, SCCaptureDynamicRangeHDRCanonicalDisplay, kCGColorSpaceDisplayP3_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2 };
    SCStreamConfiguration *c = konfiguration(&a);
    c.minimumFrameInterval = CMTimeMake(1, 10);
    Fang *fang = [[Fang alloc] init];
    __block int hoechst = -1, bilder = 0, pq = 0;
    fang.je_bild = ^(CVPixelBufferRef pb) {
        messung m = bild_messen(pb);
        bilder++;
        pq = ist_pq(pb);
        if (m.max_y > hoechst) hoechst = m.max_y;
    };
    dispatch_queue_t q = dispatch_queue_create("hdrprobe.spitze", DISPATCH_QUEUE_SERIAL);
    SCStream *st = strom_starten(c, fang, q, 1);
    if (!st) return;
    for (int s = 1; s <= (int)sekunden; s++) {
        [NSThread sleepForTimeInterval:1.0];
        __block int h = 0, n = 0;
        dispatch_sync(q, ^{ h = hoechst; n = bilder; });
        zeile(@"   %2d s: %d Bilder, hoechster Y bisher %@", s, n, code_text(h, pq));
    }
    strom_halten(st);
    zeile(@"   Ergebnis: hoechster Wert %@ - %@", code_text(hoechst, pq),
          pq && hoechst >= 0 && pq_nit(hoechst / 1023.0) > 250 ? @"die Aufnahme liefert Werte ueber dem SDR-Weiss (echtes HDR)"
                                                                : @"nichts ueber dem SDR-Weiss (lief ein HDR-Video sichtbar auf diesem Bildschirm?)");
    if (fang.letztes) CVPixelBufferRelease(fang.letztes);
}

static void schirm_ereignis(CGDirectDisplayID d, CGDisplayChangeSummaryFlags flags, void *ctx) {
    (void)ctx;
    zeile(@"   Ereignis CoreGraphics: Bildschirm %u, Flags 0x%x%@", d, flags,
          (flags & kCGDisplayBeginConfigurationFlag) ? @" (vorher)" : @"");
}

static void beobachten(double sekunden) {
    zeile(@"\n== Beobachten %.0f s: jetzt HDR an einem Bildschirm an- und ausschalten", sekunden);
    CGDisplayRegisterReconfigurationCallback(schirm_ereignis, NULL);
    [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationDidChangeScreenParametersNotification object:nil
                                                       queue:[NSOperationQueue mainQueue] usingBlock:^(NSNotification *n) {
        (void)n;
        zeile(@"   Ereignis AppKit: NSApplicationDidChangeScreenParametersNotification");
    }];
    __block NSString *vorher = nil;
    for (int s = 0; s < (int)sekunden; s++) {
        dispatch_sync(dispatch_get_main_queue(), ^{
            schirme_lesen();
            NSMutableString *t = [NSMutableString string];
            for (int i = 0; i < g_n_schirme; i++)
                [t appendFormat:@"%s[%d] %u EDR %.2f/%.2f", i ? ", " : "", i, g_schirme[i].id_, g_schirme[i].edr_pot, g_schirme[i].edr_akt];
            if (![t isEqualToString:vorher ?: @""]) zeile(@"   %2d s: %@", s, t);
            vorher = t;
        });
        [NSThread sleepForTimeInterval:1.0];
    }
}

// ------------------------------------------------------------------- main

int main(int argc, const char *argv[]) { @autoreleasepool {
    NSString *modus = argc > 1 ? @(argv[1]) : @"liste";
    int platz = argc > 2 ? atoi(argv[2]) : -1;
    double dauer = argc > 3 ? atof(argv[3]) : 20;
    if ([modus isEqualToString:@"beobachten"]) dauer = argc > 2 ? atof(argv[2]) : 60;
    if (![@[ @"liste", @"messen", @"spitze", @"beobachten" ] containsObject:modus]) {
        printf("Aufruf: hdrprobe [liste | messen [N] | spitze [N] [sekunden] | beobachten [sekunden]]\n");
        return 2;
    }
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    NSOperatingSystemVersion v = NSProcessInfo.processInfo.operatingSystemVersion;
    int arm = 0;
    size_t n = sizeof arm;
    sysctlbyname("hw.optional.arm64", &arm, &n, NULL, 0);
    zeile(@"hdrprobe - macOS %ld.%ld.%ld, %@, %@", (long)v.majorVersion, (long)v.minorVersion, (long)v.patchVersion,
          arm ? @"Apple Silicon" : @"Intel", [NSDate date]);
    schirme_lesen();
    zeile(@"Bildschirme (NSScreen):");
    schirme_zeigen();
    if ([modus isEqualToString:@"liste"]) return 0;
    if (@available(macOS 15.0, *)) {} else { zeile(@"HDR-Aufnahme braucht macOS 15 - Ende"); return 1; }
    if ([modus isEqualToString:@"beobachten"]) {
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            beobachten(dauer);
            exit(0);
        });
        [NSApp run];
        return 0;
    }
    if (!CGPreflightScreenCaptureAccess()) {
        zeile(@"Keine Freigabe \"Bildschirmaufnahme\" fuer dieses Terminal - Systemeinstellungen > Datenschutz & Sicherheit > "
               "Bildschirm- & Systemaudioaufnahme, das Terminal erlauben, Terminal neu starten, noch einmal aufrufen.");
        CGRequestScreenCaptureAccess();
        return 3;
    }
    g_ziel = &g_schirme[platz_waehlen(platz)];
    fenster_oeffnen();
    BOOL ist_spitze = [modus isEqualToString:@"spitze"];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        if (!sc_suchen()) { zeile(@"Kein SCDisplay zum Bildschirm %u - Ende", g_ziel->id_); exit(4); }
        [NSThread sleepForTimeInterval:0.5];     // das Fenster steht
        if (ist_spitze) spitze(dauer > 0 ? dauer : 20);
        else messen();
        exit(0);
    });
    [NSApp run];
    return 0;
}}
