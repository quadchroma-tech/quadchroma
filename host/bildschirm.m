// QuadChroma Host: Bildschirmwahl, Mac-Seite. Siehe bildschirm.h.
#import "bildschirm.h"
#import <AppKit/AppKit.h>
#include "qc_secure.h"
#include <pthread.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>

@implementation QCBildschirm
+ (instancetype)kennung:(NSString *)kennung name:(NSString *)name displayID:(CGDirectDisplayID)displayID
                      w:(size_t)w h:(size_t)h hz:(double)hz haupt:(BOOL)haupt {
    QCBildschirm *b = [[QCBildschirm alloc] init];
    b.kennung = kennung;
    b.name = name;
    b.displayID = displayID;
    b.w = w;
    b.h = h;
    b.hz = hz;
    b.haupt = haupt;
    return b;
}
- (BOOL)isEqual:(id)other {
    if (![other isKindOfClass:[QCBildschirm class]]) return NO;
    QCBildschirm *o = other;
    return self.displayID == o.displayID && self.w == o.w && self.h == o.h && self.hz == o.hz &&
           self.haupt == o.haupt && [self.kennung isEqualToString:o.kennung] &&
           [(self.name ?: @"") isEqualToString:(o.name ?: @"")];
}
- (NSUInteger)hash { return self.kennung.hash ^ (NSUInteger)self.displayID; }
- (NSString *)description {
    return [NSString stringWithFormat:@"Kennung %u (%@) \"%@\"", self.displayID, self.kennung, self.name ?: @""];
}
@end

// ------------------------------------------------------------ Namensvorrat

// displayID -> localizedName. Gefuellt auf dem Hauptfaden, gelesen auf der
// Lebenslauf-Warteschlange; die Sperre ist ein Blatt, darunter nichts.
static pthread_mutex_t g_namen_mtx = PTHREAD_MUTEX_INITIALIZER;
static NSDictionary<NSNumber *, NSString *> *g_namen = nil;

void qc_bildschirm_namen_auffrischen(void) {
    NSMutableDictionary<NSNumber *, NSString *> *neu = [NSMutableDictionary dictionary];
    for (NSScreen *s in [NSScreen screens]) {
        NSNumber *nr = s.deviceDescription[@"NSScreenNumber"];
        if (![nr isKindOfClass:[NSNumber class]]) continue;
        NSString *name = s.localizedName;
        if (name.length) neu[@(nr.unsignedIntValue)] = name;
    }
    pthread_mutex_lock(&g_namen_mtx);
    g_namen = neu;
    pthread_mutex_unlock(&g_namen_mtx);
}

static NSString *name_fuer(CGDirectDisplayID d) {
    pthread_mutex_lock(&g_namen_mtx);
    NSString *n = g_namen[@(d)];
    pthread_mutex_unlock(&g_namen_mtx);
    return n ?: [NSString stringWithFormat:@"Bildschirm %u", d];
}

// ------------------------------------------------------------------ Liste

NSString *qc_bildschirm_kennung(uint32_t vendor, uint32_t model, uint32_t serial) {
    return [NSString stringWithFormat:@"v%u-m%u-s%u", vendor, model, serial];
}

// Die Produktion: ScreenCaptureKit fragen, Groesse und Hz aus dem
// Anzeigemodus, Kennung aus den CGDisplay-Nummern. Zwei gleiche Kennungen
// (zwei baugleiche Monitore ohne Seriennummer) bekommen die Unit angehaengt.
static NSArray<QCBildschirm *> *bildschirme_sck(void) {
    __block NSArray<SCDisplay *> *displays = nil;
    __block BOOL fehler = NO;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
        if (e) fehler = YES;
        else displays = c.displays;
        dispatch_semaphore_signal(sem);
    }];
    if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC)) != 0) return nil;
    if (fehler || !displays) return nil;
    CGDirectDisplayID haupt = CGMainDisplayID();
    NSMutableArray<QCBildschirm *> *liste = [NSMutableArray arrayWithCapacity:displays.count];
    NSCountedSet *kennungen = [NSCountedSet set];
    for (SCDisplay *d in displays) {
        CGDirectDisplayID id_ = d.displayID;
        CGDisplayModeRef m = CGDisplayCopyDisplayMode(id_);
        size_t pw = m ? CGDisplayModeGetPixelWidth(m) : 0, ph = m ? CGDisplayModeGetPixelHeight(m) : 0;
        double hz = m ? CGDisplayModeGetRefreshRate(m) : 0;
        if (m) CGDisplayModeRelease(m);
        NSString *k = qc_bildschirm_kennung(CGDisplayVendorNumber(id_), CGDisplayModelNumber(id_), CGDisplaySerialNumber(id_));
        [kennungen addObject:k];
        QCBildschirm *b = [QCBildschirm kennung:k name:name_fuer(id_) displayID:id_ w:pw h:ph hz:hz haupt:id_ == haupt];
        b.sc = d;
        [liste addObject:b];
    }
    for (QCBildschirm *b in liste)
        if ([kennungen countForObject:b.kennung] > 1)
            b.kennung = [b.kennung stringByAppendingFormat:@"-u%u", CGDisplayUnitNumber(b.displayID)];
    return liste;
}

static qc_bildschirm_lieferant g_lieferant = bildschirme_sck;

void qc_bildschirm_liste_setzen(qc_bildschirm_lieferant lieferant) {
    g_lieferant = lieferant ?: bildschirme_sck;
}

NSArray<QCBildschirm *> *qc_bildschirme_holen(void) {
    return g_lieferant();
}

// ------------------------------------------------------------------- Wahl

QCBildschirm *qc_bildschirm_wahl(NSArray<QCBildschirm *> *liste, NSString *wunsch, int *grund) {
    int g = QC_WAHL_KEINER;
    QCBildschirm *ziel = nil;
    if (wunsch.length)
        for (QCBildschirm *b in liste) if ([b.kennung isEqualToString:wunsch]) { ziel = b; g = QC_WAHL_WUNSCH; break; }
    if (!ziel)
        for (QCBildschirm *b in liste) if (b.haupt) { ziel = b; g = QC_WAHL_HAUPT; break; }
    if (!ziel && liste.count) { ziel = liste[0]; g = QC_WAHL_ERSTER; }
    if (grund) *grund = g;
    return ziel;
}

// ------------------------------------------------------ Kodieren und Lesen

// Steuerzeichen wie char::is_control im Rust-Kern (Kategorie Cc): U+0000 bis
// U+001F und U+007F bis U+009F. In UTF-8 ist U+0080..U+009F die Folge C2 80..C2 9F.
static int steuerzeichen(const uint8_t *p, size_t n, size_t i, size_t *len) {
    uint8_t c = p[i];
    if (c < 0x20 || c == 0x7f) { *len = 1; return 1; }
    if (c == 0xc2 && i + 1 < n && p[i + 1] >= 0x80 && p[i + 1] <= 0x9f) { *len = 2; return 1; }
    return 0;
}

// Laenge der UTF-8-Folge, die bei p[i] beginnt (die Eingabe ist gueltiges UTF-8).
static size_t folge_laenge(uint8_t c) {
    if (c < 0x80) return 1;
    if ((c & 0xe0) == 0xc0) return 2;
    if ((c & 0xf0) == 0xe0) return 3;
    return 4;
}

// Steuerzeichen weg, danach an einer Zeichengrenze auf hoechstens max Byte.
static NSString *bereinigen(NSString *s, size_t max) {
    const char *u = s.UTF8String ?: "";
    size_t n = strlen(u);
    const uint8_t *p = (const uint8_t *)u;
    NSMutableData *d = [NSMutableData dataWithCapacity:n < max ? n : max];
    for (size_t i = 0; i < n;) {
        size_t len = 0;
        if (steuerzeichen(p, n, i, &len)) { i += len; continue; }
        len = folge_laenge(p[i]);
        if (i + len > n) break;
        if (d.length + len > max) break;
        [d appendBytes:p + i length:len];
        i += len;
    }
    return [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] ?: @"";
}

NSString *qc_bildschirm_kennung_bereinigen(NSString *kennung) {
    NSString *k = bereinigen(kennung ?: @"", QC_BILDSCHIRM_KENNUNG_MAX);
    return k.length ? k : @"?";
}

NSString *qc_bildschirm_name_bereinigen(NSString *name) {
    return bereinigen(name ?: @"", QC_BILDSCHIRM_NAME_MAX);
}

static void le16(NSMutableData *d, uint16_t v) {
    uint8_t b[2] = { (uint8_t)(v & 0xff), (uint8_t)(v >> 8) };
    [d appendBytes:b length:2];
}

NSData *qc_bildschirme_kodieren(NSArray<QCBildschirm *> *liste, NSString *wunsch, CGDirectDisplayID gestreamt) {
    NSUInteger anzahl = liste.count < QC_BILDSCHIRM_EINTRAEGE_MAX ? liste.count : QC_BILDSCHIRM_EINTRAEGE_MAX;
    // Ein leerer Wunsch hiesse Automatik - eine Kennung, die nur aus
    // Steuerzeichen besteht, wird deshalb wie bei den Eintraegen zu "?".
    NSData *w = wunsch.length ? [qc_bildschirm_kennung_bereinigen(wunsch) dataUsingEncoding:NSUTF8StringEncoding] : [NSData data];
    NSMutableData *d = [NSMutableData dataWithCapacity:QC_BILDSCHIRM_LISTE_MAX];
    uint8_t kopf[4] = { QC_BILDSCHIRM_FASSUNG, (uint8_t)anzahl, (uint8_t)w.length, 0 };
    [d appendBytes:kopf length:4];
    [d appendData:w];
    for (NSUInteger i = 0; i < anzahl; i++) {
        QCBildschirm *b = liste[i];
        NSData *k = [qc_bildschirm_kennung_bereinigen(b.kennung) dataUsingEncoding:NSUTF8StringEncoding];
        NSData *n = [qc_bildschirm_name_bereinigen(b.name) dataUsingEncoding:NSUTF8StringEncoding];
        uint8_t e[2] = { (uint8_t)k.length, (uint8_t)n.length };
        [d appendBytes:e length:2];
        le16(d, (uint16_t)(b.w > 65535 ? 65535 : b.w));
        le16(d, (uint16_t)(b.h > 65535 ? 65535 : b.h));
        double hz = b.hz > 0 ? b.hz + 0.5 : 0;
        le16(d, (uint16_t)(hz > 65535 ? 65535 : hz));
        uint8_t flags = 0;
        if (b.haupt) flags |= QC_BILDSCHIRM_FLAG_HAUPT;
        if (gestreamt && b.displayID == gestreamt) flags |= QC_BILDSCHIRM_FLAG_GESTREAMT;
        uint8_t f[2] = { flags, 0 };
        [d appendBytes:f length:2];
        [d appendData:k];
        [d appendData:n];
    }
    return d;
}

int qc_bildschirm_wunsch_lesen(const uint8_t *p, size_t n, NSString **kennung) {
    if (!p || n < 1) return -1;
    size_t l = p[0];
    if (l > QC_BILDSCHIRM_KENNUNG_MAX || 1 + l > n) return -1;
    if (l == 0) { if (kennung) *kennung = nil; return 0; }
    NSString *s = [[NSString alloc] initWithBytes:p + 1 length:l encoding:NSUTF8StringEncoding];
    if (!s) return -1;
    for (size_t i = 0; i < l;) {
        size_t len = 0;
        if (steuerzeichen(p + 1, l, i, &len)) return -1;
        i += folge_laenge(p[1 + i]);
    }
    if (kennung) *kennung = s;
    return 0;
}

NSData *qc_bildschirm_wunsch_kodieren(NSString *kennung) {
    NSData *k = kennung.length ? [qc_bildschirm_kennung_bereinigen(kennung) dataUsingEncoding:NSUTF8StringEncoding] : [NSData data];
    NSMutableData *d = [NSMutableData dataWithCapacity:1 + k.length];
    uint8_t l = (uint8_t)k.length;
    [d appendBytes:&l length:1];
    [d appendData:k];
    return d;
}

// ------------------------------------------------------------- Persistenz

#define QC_BILDSCHIRM_DATEI "bildschirm.txt"

int qc_bildschirm_wunsch_laden(NSString **wunsch) {
    if (wunsch) *wunsch = nil;
    char pfad[1200];
    if (qc_config_path(QC_BILDSCHIRM_DATEI, pfad, sizeof pfad) != 0) return -1;
    int fd = open(pfad, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return errno == ENOENT ? 1 : -1;
    // Eine Kennung plus Zeilenende; was laenger ist, ist kaputt.
    uint8_t b[QC_BILDSCHIRM_KENNUNG_MAX + 3];
    ssize_t n = read(fd, b, sizeof b);
    close(fd);
    if (n <= 0 || (size_t)n == sizeof b) return -1;
    size_t len = (size_t)n;
    if (b[len - 1] == '\n') { len--; if (len && b[len - 1] == '\r') len--; }
    if (len == 0 || len > QC_BILDSCHIRM_KENNUNG_MAX) return -1;
    uint8_t nutz[QC_BILDSCHIRM_KENNUNG_MAX + 1];
    nutz[0] = (uint8_t)len;
    memcpy(nutz + 1, b, len);
    NSString *k = nil;
    if (qc_bildschirm_wunsch_lesen(nutz, 1 + len, &k) != 0) return -1;
    if (wunsch) *wunsch = [k isEqualToString:@"auto"] ? nil : k;
    return 0;
}

int qc_bildschirm_wunsch_speichern(NSString *wunsch) {
    char pfad[1200], neu[1220];
    if (qc_config_path(QC_BILDSCHIRM_DATEI, pfad, sizeof pfad) != 0) return -1;
    snprintf(neu, sizeof neu, "%s.neu", pfad);
    NSString *zeile = [(wunsch.length ? qc_bildschirm_kennung_bereinigen(wunsch) : @"auto") stringByAppendingString:@"\n"];
    const char *u = zeile.UTF8String;
    size_t n = strlen(u);
    int fd = open(neu, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    int ok = 1;
    for (size_t o = 0; o < n && ok;) {
        ssize_t w = write(fd, u + o, n - o);
        if (w <= 0) ok = 0; else o += (size_t)w;
    }
    if (ok && fsync(fd) != 0) ok = 0;
    if (close(fd) != 0) ok = 0;
    if (ok && rename(neu, pfad) != 0) ok = 0;
    if (!ok) unlink(neu);
    return ok ? 0 : -1;
}
