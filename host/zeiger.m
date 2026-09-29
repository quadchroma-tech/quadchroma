#import <AppKit/AppKit.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

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
// Bildpunkte des Stroms je Punkt, in Tausendsteln (siehe zeiger.h).
static _Atomic int g_massstab_promille = 1000;

void qc_zeiger_massstab_setzen(double massstab) {
    if (!(massstab > 0)) massstab = 1.0;
    if (massstab < 0.1) massstab = 0.1;
    if (massstab > 8.0) massstab = 8.0;
    int neu = (int)lround(massstab * 1000.0);
    if (atomic_exchange(&g_massstab_promille, neu) != neu) atomic_store(&g_erzwingen, 1);
}

double qc_zeiger_massstab(void) { return atomic_load(&g_massstab_promille) / 1000.0; }

// Maus einfangen: was die Eingabe eingespeist hat (Eingabefaden) und der
// Waechter samt letztem Urteil (Hauptwarteschlange, zeiger_pruefen).
static _Atomic uint64_t g_bewegungen;
static _Atomic uint64_t g_letzte_taste_us;
static pthread_mutex_t g_ziel_mtx = PTHREAD_MUTEX_INITIALIZER;
static double g_ziel_x, g_ziel_y;     // letzte absolute Bewegung (globale Punkte)
static uint64_t g_ziel_us;            // wann (0 = keine, oder zuletzt relativ)
static qc_fang_waechter g_waechter;
static qc_fang_urteil g_urteil = { 1, 0, 0 };
static int g_lage_zeilen;
#define QC_ZEIGER_LAGE_ZEILEN 40
#define QC_ZEIGER_FOLGT_TOLERANZ 3.0

static void melden(const char *text);

const qc_fang_art QC_FANG_MAC = { 0, 0, 1 };
const qc_fang_art QC_FANG_WINDOWS = { 1, 1, 0 };

static uint64_t jetzt_us(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000ull;
}

qc_fang_urteil qc_fang_schritt(qc_fang_waechter *w, uint64_t jetzt, qc_zeiger_lage lage,
                               qc_eingabe_zaehler ein, qc_fang_art art) {
    if (!lage.versteckt) {
        w->versteckt = 0;
        w->grund_offen = 0;
        if (w->gefangen) {
            if (!w->sichtbar_offen) { w->sichtbar_offen = 1; w->sichtbar_seit = jetzt; }
            if (jetzt - w->sichtbar_seit >= QC_FANG_AUS_US) {
                w->gefangen = 0;
                w->grund = 0;
                w->sichtbar_offen = 0;
            }
        }
        return (qc_fang_urteil){ !w->gefangen, w->gefangen, w->grund };
    }
    w->sichtbar_offen = 0;
    if (!w->versteckt) {
        w->versteckt = 1;
        w->versteckt_seit = jetzt;
        w->bewegungen_basis = ein.bewegungen;
        w->taste_vor_verstecken = ein.letzte_taste_us != 0 && jetzt - ein.letzte_taste_us < QC_FANG_TIPPEN_US;
    }
    uint64_t bewegt = ein.bewegungen >= w->bewegungen_basis ? ein.bewegungen - w->bewegungen_basis : 0;
    int tippen = w->taste_vor_verstecken && !(art.tippen_endet_mit_bewegung && bewegt >= QC_FANG_BEWEGUNGEN_MIN);
    uint8_t grund = 0;
    if (lage.eingesperrt) grund |= QC_FANG_GRUND_EINGESPERRT;
    if (lage.folgt_nicht) grund |= QC_FANG_GRUND_FOLGT_NICHT;
    if (art.bewegung_regel && lage.vollbild && !tippen && bewegt >= QC_FANG_BEWEGUNGEN_MIN &&
        jetzt - w->versteckt_seit >= QC_FANG_VERSTECKT_MIN_US)
        grund |= QC_FANG_GRUND_BEWEGUNG;
    if (!w->gefangen) {
        if (grund) {
            if (!w->grund_offen) { w->grund_offen = 1; w->grund_seit = jetzt; }
            if (jetzt - w->grund_seit >= QC_FANG_AN_US) {
                w->gefangen = 1;
                w->grund = grund;
            }
        } else {
            w->grund_offen = 0;
        }
    }
    int sichtbar = w->gefangen ? 0 : art.versteckt_nur_mit_fang ? 1 : tippen;
    return (qc_fang_urteil){ sichtbar, w->gefangen, w->grund };
}

int qc_zeiger_folgt_nicht(uint64_t jetzt, double px, double py, double zx, double zy, uint64_t ziel_us,
                          double toleranz) {
    if (!ziel_us || jetzt < ziel_us) return 0;
    uint64_t alter = jetzt - ziel_us;
    if (alter < 30000ull || alter > 2000000ull) return 0;
    return fabs(px - zx) > toleranz || fabs(py - zy) > toleranz;
}

void qc_zeiger_eingabe_bewegung(int absolut, double x, double y) {
    atomic_fetch_add(&g_bewegungen, 1);
    pthread_mutex_lock(&g_ziel_mtx);
    if (absolut) { g_ziel_x = x; g_ziel_y = y; g_ziel_us = jetzt_us(); }
    else g_ziel_us = 0;
    pthread_mutex_unlock(&g_ziel_mtx);
}

void qc_zeiger_eingabe_taste(void) {
    uint64_t t = jetzt_us();
    atomic_store(&g_letzte_taste_us, t ? t : 1);
}

// Die Lage des Systems fuer den Waechter: versteckt (CGCursorIsVisible) und
// "folgt nicht" (die Lage des Zeigers gegen das Ziel der letzten absoluten
// Bewegung). Einsperren kennt der Mac nicht.
static void fang_pruefen(void) {
    uint64_t jetzt = jetzt_us();
    qc_zeiger_lage lage = { CGCursorIsVisible() ? 0 : 1, 0, 0, 0 };
    CGEventRef e = CGEventCreate(NULL);
    if (e) {
        CGPoint p = CGEventGetLocation(e);
        CFRelease(e);
        pthread_mutex_lock(&g_ziel_mtx);
        double zx = g_ziel_x, zy = g_ziel_y;
        uint64_t zt = g_ziel_us;
        pthread_mutex_unlock(&g_ziel_mtx);
        lage.folgt_nicht = qc_zeiger_folgt_nicht(jetzt, p.x, p.y, zx, zy, zt, QC_ZEIGER_FOLGT_TOLERANZ);
    }
    qc_eingabe_zaehler ein = { atomic_load(&g_bewegungen), atomic_load(&g_letzte_taste_us) };
    qc_fang_urteil u = qc_fang_schritt(&g_waechter, jetzt, lage, ein, QC_FANG_MAC);
    if (u.sichtbar != g_urteil.sichtbar || u.gefangen != g_urteil.gefangen) {
        if (++g_lage_zeilen <= QC_ZEIGER_LAGE_ZEILEN) {
            char t[200];
            snprintf(t, sizeof t, "Zeiger: %s%s, an den Client: %s", lage.versteckt ? "versteckt" : "sichtbar",
                     u.gefangen ? (u.grund & QC_FANG_GRUND_FOLGT_NICHT ? " - Maus eingefangen (folgt der Maus nicht)"
                                                                      : " - Maus eingefangen")
                                : "",
                     u.sichtbar ? "zeigen" : "nicht zeigen");
            melden(t);
        } else if (g_lage_zeilen == QC_ZEIGER_LAGE_ZEILEN + 1) {
            melden("Zeiger: weitere Wechsel ohne Protokollzeile");
        }
    }
    g_urteil = u;
}

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
    if (g_aktiv && !g_aktiv()) {
        // Kein Zuschauer: der naechste faengt mit einem frischen Waechter an.
        memset(&g_waechter, 0, sizeof g_waechter);
        g_urteil = (qc_fang_urteil){ 1, 0, 0 };
        g_lage_zeilen = 0;
        return;
    }
    fang_pruefen();
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
    if (!(sz.width > 0) || !(sz.height > 0)) return;
    // Groesse im Strom: Punkte mal Massstab (Bildpunkte des Stroms je Punkt).
    double m = qc_zeiger_massstab();
    int qw = (int)ceil(sz.width * m), qh = (int)ceil(sz.height * m);
    if (qw <= 0 || qh <= 0) return;
    // Einpassen statt verwerfen: der Zeiger wird ohnehin gezeichnet, also
    // notfalls kleiner - Hotspot im selben Mass.
    double f = m;
    if (qw > QC_ZEIGER_MAX || qh > QC_ZEIGER_MAX) {
        f = m * fmin((double)QC_ZEIGER_MAX / qw, (double)QC_ZEIGER_MAX / qh);
        if (!g_gross_gemeldet) {
            g_gross_gemeldet = 1;
            char t[160];
            snprintf(t, sizeof t, "Zeigerform: Zeiger mit %dx%d Bildpunkten wird auf %dx%d verkleinert",
                     qw, qh, (int)ceil(sz.width * f), (int)ceil(sz.height * f));
            melden(t);
        }
    }
    int w = (int)ceil(sz.width * f), h = (int)ceil(sz.height * f);
    if (w > QC_ZEIGER_MAX) w = QC_ZEIGER_MAX;
    if (h > QC_ZEIGER_MAX) h = QC_ZEIGER_MAX;
    if (w <= 0 || h <= 0) return;
    // Der Hotspot liegt bei NSCursor von links oben aus - wie bei Windows;
    // in Punkten, also mit f (Massstab samt Einpassen) wie die Groesse.
    NSPoint hot = c.hotSpot;
    int hxi = (int)(hot.x * f), hyi = (int)(hot.y * f);
    if (hxi < 0) hxi = 0; if (hxi >= w) hxi = w - 1;
    if (hyi < 0) hyi = 0; if (hyi >= h) hyi = h - 1;
    // Sichtbar und eingefangen sagt der Waechter (fang_pruefen): versteckt
    // meldet der Mac nur im Fang - CGCursorIsVisible allein meldet auch das
    // Verstecken beim Tippen, das nie endet. CGCursorIsVisible gilt als
    // veraltet, liefert hier aber den Wert; hoerte es auf, bliebe der Zeiger
    // sichtbar und nie eingefangen - der harmlosere Fehler.
    int sichtbar = g_urteil.sichtbar ? 1 : 0;
    int gefangen = g_urteil.gefangen ? 1 : 0;

    // In Bildpunkten des Stroms zeichnen: bei Massstab 2 (HiDPI, nativ
    // gestreamt) waehlt AppKit fuer das doppelt so grosse Ziel die
    // 2x-Darstellung - der Zeiger ist so scharf wie auf dem Mac selbst. Was
    // ankommt, ist so gross wie der Zeiger im Bild; der Client bringt es auf
    // den Massstab, in dem er das Bild zeigt.
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
    uint8_t rand[6] = { (uint8_t)w, (uint8_t)h, (uint8_t)hxi, (uint8_t)hyi, (uint8_t)sichtbar, (uint8_t)gefangen };
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
        if (spaetere == 0 && g_cb) g_cb(w16, h16, hx16, hy16, sichtbar, gefangen, buf);
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
