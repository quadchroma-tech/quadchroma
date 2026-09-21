#import <AppKit/AppKit.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#include "zeiger.h"

// Wartekugel und Textcursor sind Animationen; 20 Abfragen je Sekunde reichen,
// damit sie fluessig aussehen, und kosten fast nichts (ein 32x32-Bild).
#define QC_ZEIGER_POLL_MS 50ull
#define QC_ZEIGER_LEEWAY_MS 10ull
// Groesser schickt der Host keinen Zeiger; was groesser ist (Bedienungshilfen
// "Zeigergroesse", Fadenkreuze von Zeichenprogrammen), wird eingepasst. Die
// Grenze ist dieselbe, die der Client annimmt.
#define QC_ZEIGER_MAX 256

static dispatch_source_t g_timer;
static dispatch_queue_t g_sendq;      // Versand abseits des Hauptfadens
static qc_zeiger_cb g_cb;
static int (*g_aktiv)(void);
static qc_zeiger_log g_log;
static uint64_t g_hash;               // Kennung der zuletzt gesendeten Form
static _Atomic int g_erzwingen;
static _Atomic int g_offen;           // Formen, die auf den Versand warten
static int g_nil_gemeldet;            // "System liefert keinen Zeiger" nur einmal
static int g_gross_gemeldet;          // Verkleinern nur einmal melden

static void melden(const char *text) {
    if (g_log) g_log(text);
}

static uint64_t fnv(const uint8_t *p, size_t n, uint64_t h) {
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 1099511628211ull; }
    return h;
}

// Den systemweiten Zeiger holen, in RGBA zeichnen, bei Aenderung melden.
// [NSCursor currentSystemCursor] ist der Zeiger, den der Benutzer sieht -
// auch der einer fremden Anwendung; das ist genau der, der auf der anderen
// Seite erscheinen soll. Laeuft auf dem Hauptfaden (AppKit); der Versand
// nicht - der wartet unter Umstaenden hinter einem Vollbild, und so lange
// darf der Hauptfaden nicht stehen (Einstellungen, Zwischenablage, Erholung
// nach Bildschirmverlust laufen dort).
static void zeiger_pruefen(void) {
    if (g_aktiv && !g_aktiv()) return;
    NSCursor *c = [NSCursor currentSystemCursor];
    NSImage *img = c ? c.image : nil;
    if (!img) {
        // Apples Kopfdatei sagt, diese Eigenschaft werde in einer kuenftigen
        // macOS-Fassung immer nil liefern. Dann bleibt die Form aus - und das
        // soll im Protokoll stehen, nicht erraten werden muessen. Die Kennung
        // wird geloescht, damit die naechste gueltige Form sicher rausgeht.
        if (!g_nil_gemeldet) {
            g_nil_gemeldet = 1;
            g_hash = 0;
            melden("Zeigerform: das System liefert keinen Zeiger (currentSystemCursor nil) - Form bleibt aus");
        }
        return;
    }
    if (g_nil_gemeldet) {
        g_nil_gemeldet = 0;
        melden("Zeigerform: das System liefert wieder einen Zeiger");
    }
    NSSize sz = img.size;
    int qw = (int)ceil(sz.width), qh = (int)ceil(sz.height);
    if (qw <= 0 || qh <= 0) return;
    // Einpassen statt verwerfen: der Zeiger wird ohnehin gezeichnet, also
    // notfalls kleiner - Hotspot im selben Mass.
    double f = 1.0;
    if (qw > QC_ZEIGER_MAX || qh > QC_ZEIGER_MAX) {
        f = fmin((double)QC_ZEIGER_MAX / qw, (double)QC_ZEIGER_MAX / qh);
        if (!g_gross_gemeldet) {
            g_gross_gemeldet = 1;
            char t[160];
            snprintf(t, sizeof t, "Zeigerform: Zeiger mit %dx%d Punkten wird auf %dx%d verkleinert",
                     qw, qh, (int)ceil(qw * f), (int)ceil(qh * f));
            melden(t);
        }
    }
    int w = (int)ceil(qw * f), h = (int)ceil(qh * f);
    if (w <= 0 || h <= 0) return;
    // Der Hotspot liegt bei NSCursor von links oben aus - wie bei Windows.
    NSPoint hot = c.hotSpot;
    int hxi = (int)(hot.x * f), hyi = (int)(hot.y * f);
    if (hxi < 0) hxi = 0; if (hxi >= w) hxi = w - 1;
    if (hyi < 0) hyi = 0; if (hyi >= h) hyi = h - 1;
    // CGCursorIsVisible gilt als veraltet, liefert hier aber den Wert - ob es
    // je aufhoert, steht im Protokoll: dann bliebe der Zeiger ewig sichtbar,
    // was der harmlosere Fehler ist.
    int sichtbar = CGCursorIsVisible() ? 1 : 0;

    // In Bildpunkten zeichnen (ein Punkt = ein Bildpunkt): auf einem Retina-Mac
    // waehlt AppKit die 2x-Darstellung und rechnet sie herunter. Was ankommt,
    // hat die Groesse, die Windows fuer einen Zeiger erwartet; der Client
    // zieht sie auf den Massstab seines Bildes hoch.
    size_t n = (size_t)w * (size_t)h * 4;
    uint8_t *buf = calloc(n, 1);
    if (!buf) return;
    CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
    CGContextRef ctx = CGBitmapContextCreate(buf, (size_t)w, (size_t)h, 8, (size_t)w * 4, cs,
                                             kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    CGColorSpaceRelease(cs);
    if (!ctx) { free(buf); return; }
    NSGraphicsContext *gc = [NSGraphicsContext graphicsContextWithCGContext:ctx flipped:NO];
    [NSGraphicsContext saveGraphicsState];
    [NSGraphicsContext setCurrentContext:gc];
    [img drawInRect:NSMakeRect(0, 0, w, h) fromRect:NSZeroRect
          operation:NSCompositingOperationCopy fraction:1.0 respectFlipped:YES hints:nil];
    [NSGraphicsContext restoreGraphicsState];
    CGContextRelease(ctx);

    // CoreGraphics liefert vormultiplizierte Farben; der Client will gerade.
    for (size_t i = 0; i < n; i += 4) {
        uint32_t a = buf[i + 3];
        if (a == 0 || a == 255) continue;
        for (int k = 0; k < 3; k++) {
            uint32_t v = (uint32_t)buf[i + k] * 255u / a;
            buf[i + k] = (uint8_t)(v > 255 ? 255 : v);
        }
    }

    uint64_t hh = fnv(buf, n, 1469598103934665603ull);
    uint8_t rand[6] = { (uint8_t)w, (uint8_t)h, (uint8_t)hxi, (uint8_t)hyi, (uint8_t)sichtbar, 0 };
    hh = fnv(rand, sizeof rand, hh);
    // Das Zeichen "auf jeden Fall senden" wird immer verbraucht, sonst ginge
    // dieselbe Form beim naechsten Tick noch einmal raus.
    int erzwingen = atomic_exchange(&g_erzwingen, 0);
    if (hh == g_hash && !erzwingen) { free(buf); return; }
    g_hash = hh;

    // Versand auf der eigenen Reihe. Liegen dort schon Formen, ist diese die
    // neueste - die aelteren ueberspringen sich beim Abarbeiten selbst.
    uint16_t w16 = (uint16_t)w, h16 = (uint16_t)h, hx16 = (uint16_t)hxi, hy16 = (uint16_t)hyi;
    atomic_fetch_add(&g_offen, 1);
    dispatch_async(g_sendq, ^{
        int spaetere = atomic_fetch_sub(&g_offen, 1) - 1;
        if (spaetere == 0 && g_cb) g_cb(w16, h16, hx16, hy16, sichtbar, buf);
        free(buf);
    });
}

void qc_zeiger_start(qc_zeiger_cb cb, int (*aktiv)(void), qc_zeiger_log log) {
    if (g_timer) return;
    g_cb = cb;
    g_aktiv = aktiv;
    g_log = log;
    g_sendq = dispatch_queue_create("tech.quadchroma.zeiger", DISPATCH_QUEUE_SERIAL);
    g_timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    if (!g_sendq || !g_timer) { melden("Zeigerform: Warteschlange oder Zeitgeber nicht anlegbar"); return; }
    dispatch_source_set_timer(g_timer,
                              dispatch_time(DISPATCH_TIME_NOW, (int64_t)(QC_ZEIGER_POLL_MS * NSEC_PER_MSEC)),
                              QC_ZEIGER_POLL_MS * NSEC_PER_MSEC,
                              QC_ZEIGER_LEEWAY_MS * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(g_timer, ^{
        @autoreleasepool { zeiger_pruefen(); }
    });
    dispatch_resume(g_timer);
}

void qc_zeiger_neu_senden(void) {
    atomic_store(&g_erzwingen, 1);
}
