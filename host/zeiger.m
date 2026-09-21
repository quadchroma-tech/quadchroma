#import <AppKit/AppKit.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#include "zeiger.h"

// Wartekugel und Textcursor sind Animationen; 20 Abfragen je Sekunde reichen,
// damit sie fluessig aussehen, und kosten fast nichts (ein 32x32-Bild).
#define QC_ZEIGER_POLL_MS 50ull
#define QC_ZEIGER_LEEWAY_MS 10ull
// Groesser ist kein Zeiger. Die Grenze schuetzt vor einem Bild, das nie eins war.
#define QC_ZEIGER_MAX 128

static dispatch_source_t g_timer;
static qc_zeiger_cb g_cb;
static int (*g_aktiv)(void);
static uint64_t g_hash;               // Kennung der zuletzt gesendeten Form
static _Atomic int g_erzwingen;

static uint64_t fnv(const uint8_t *p, size_t n, uint64_t h) {
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 1099511628211ull; }
    return h;
}

// Den systemweiten Zeiger holen, in RGBA zeichnen, bei Aenderung melden.
// [NSCursor currentSystemCursor] ist der Zeiger, den der Benutzer sieht -
// auch der einer fremden Anwendung; das ist genau der, der auf der anderen
// Seite erscheinen soll.
static void zeiger_pruefen(void) {
    if (g_aktiv && !g_aktiv()) return;
    NSCursor *c = [NSCursor currentSystemCursor];
    if (!c) return;
    NSImage *img = c.image;
    if (!img) return;
    NSSize sz = img.size;
    int w = (int)ceil(sz.width), h = (int)ceil(sz.height);
    if (w <= 0 || h <= 0 || w > QC_ZEIGER_MAX || h > QC_ZEIGER_MAX) return;
    // Der Hotspot liegt bei NSCursor von links oben aus - wie bei Windows.
    NSPoint hot = c.hotSpot;
    int hxi = (int)hot.x, hyi = (int)hot.y;
    if (hxi < 0) hxi = 0; if (hxi >= w) hxi = w - 1;
    if (hyi < 0) hyi = 0; if (hyi >= h) hyi = h - 1;
    int sichtbar = CGCursorIsVisible() ? 1 : 0;

    // In Bildpunkten zeichnen (ein Punkt = ein Bildpunkt): auf einem Retina-Mac
    // waehlt AppKit die 2x-Darstellung und rechnet sie herunter. Was ankommt,
    // hat die Groesse, die Windows fuer einen Zeiger erwartet.
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
    if (hh != g_hash || atomic_exchange(&g_erzwingen, 0)) {
        g_hash = hh;
        if (g_cb) g_cb((uint16_t)w, (uint16_t)h, (uint16_t)hxi, (uint16_t)hyi, sichtbar, buf);
    }
    free(buf);
}

void qc_zeiger_start(qc_zeiger_cb cb, int (*aktiv)(void)) {
    if (g_timer) return;
    g_cb = cb;
    g_aktiv = aktiv;
    g_timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    if (!g_timer) return;
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
