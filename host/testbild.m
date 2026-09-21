#import <Foundation/Foundation.h>
#include <math.h>
#include <string.h>

#include "testbild.h"

// Zwoelf Bilder je Schleife: bei 1080p in 4:4:4 10 Bit sind das 150 MB -
// das ist der Preis dafuer, dass waehrend der Messung nichts gemalt wird.
// Bewegungen sind so angelegt, dass die Schleife nahtlos ist.
#define QC_TESTBILD_N 12

static CVPixelBufferRef g_bilder[QC_TESTBILD_N];
static int g_n = 0, g_i = 0;
static int g_w = 0, g_h = 0;
static OSType g_fmt = 0;

// Farbbalken im vollen Wertebereich, 8 Bit (Cb, Cr): Weiss, Gelb, Cyan,
// Gruen, Magenta, Rot, Blau, Schwarz - die klassische Reihe, an der ein
// 4:2:0-Codec an den Kanten sichtbar ausfranst.
static const uint8_t BALKEN_CB[8] = { 128,  16, 166,  54, 202,  90, 240, 128 };
static const uint8_t BALKEN_CR[8] = { 128, 146,  16,  34, 222, 240, 110, 128 };
static const uint8_t BALKEN_Y[8]  = { 235, 210, 170, 145, 106,  81,  41,  16 };

// Ein Bildpunkt des k-ten Bildes, 8 Bit Y/Cb/Cr im vollen Wertebereich.
// Ganzzahlig, damit das Bild auf jeder Maschine gleich ist.
static inline void punkt(int x, int y, int w, int h, int k, uint8_t *yy, uint8_t *cb, uint8_t *cr) {
    // Oberes Drittel: Farbbalken.
    if (y < h / 3) {
        int b = (x * 8) / w;
        *yy = BALKEN_Y[b]; *cb = BALKEN_CB[b]; *cr = BALKEN_CR[b];
        return;
    }
    // Mittleres Drittel: feine Streifen, die je Bild um zwei Punkte
    // wandern (Periode 4, zwoelf Bilder = sechs Perioden: nahtlos), links
    // grau, rechts in Rot und Blau - da zeigt sich, was 4:4:4 wert ist.
    if (y < 2 * h / 3) {
        int hell = (((x + 2 * k) / 2) & 1) ? 220 : 40;
        *yy = (uint8_t)hell;
        if (x < w / 2) { *cb = 128; *cr = 128; }
        else if (((y / 8) & 1) == 0) { *cb = 90; *cr = 240; }
        else { *cb = 240; *cr = 110; }
        return;
    }
    // Unteres Drittel: waagerechter Verlauf, darauf ein Quadrat, das im
    // Kreis laeuft - eine ganze Runde je Schleife.
    int v = 16 + (x * 219) / (w > 1 ? w - 1 : 1);
    *yy = (uint8_t)v; *cb = 128; *cr = 128;
    int r = h / 10;
    double wink = 2.0 * M_PI * k / QC_TESTBILD_N;
    int cx = w / 2 + (int)(w / 4 * cos(wink));
    int cy = 2 * h / 3 + h / 6 + (int)(h / 12 * sin(wink));
    if (x >= cx - r / 2 && x < cx + r / 2 && y >= cy - r / 2 && y < cy + r / 2) {
        *yy = 200; *cb = 60; *cr = 220;
    }
}

static void bild_fuellen(CVPixelBufferRef pb, int k) {
    CVPixelBufferLockBaseAddress(pb, 0);
    OSType fmt = CVPixelBufferGetPixelFormatType(pb);
    int w = (int)CVPixelBufferGetWidth(pb), h = (int)CVPixelBufferGetHeight(pb);
    uint8_t *p0 = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
    uint8_t *p1 = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
    size_t s0 = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    size_t s1 = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
    if (fmt == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange) {
        // 10 Bit oben buendig in 16 Bit, wie Apple es ablegt; Cb/Cr in
        // voller Aufloesung verschraenkt.
        for (int y = 0; y < h; y++) {
            uint16_t *zy = (uint16_t *)(p0 + (size_t)y * s0);
            uint16_t *zc = (uint16_t *)(p1 + (size_t)y * s1);
            for (int x = 0; x < w; x++) {
                uint8_t yy, cb, cr;
                punkt(x, y, w, h, k, &yy, &cb, &cr);
                zy[x] = (uint16_t)((yy * 4) << 6);
                zc[2 * x] = (uint16_t)((cb * 4) << 6);
                zc[2 * x + 1] = (uint16_t)((cr * 4) << 6);
            }
        }
    } else {
        // 4:2:0 8 Bit: Cb/Cr in halber Aufloesung, je vier Punkte einer.
        for (int y = 0; y < h; y++) {
            uint8_t *zy = p0 + (size_t)y * s0;
            for (int x = 0; x < w; x++) {
                uint8_t yy, cb, cr;
                punkt(x, y, w, h, k, &yy, &cb, &cr);
                zy[x] = yy;
            }
        }
        for (int y = 0; y < h / 2; y++) {
            uint8_t *zc = p1 + (size_t)y * s1;
            for (int x = 0; x < w / 2; x++) {
                uint8_t yy, cb, cr;
                punkt(2 * x, 2 * y, w, h, k, &yy, &cb, &cr);
                zc[2 * x] = cb;
                zc[2 * x + 1] = cr;
            }
        }
    }
    CVPixelBufferUnlockBaseAddress(pb, 0);
}

void qc_testbild_start(int w, int h, OSType fmt) {
    if (g_n && g_w == w && g_h == h && g_fmt == fmt) return;
    qc_testbild_stop();
    if (w <= 0 || h <= 0) return;
    // IOSurface-gestuetzt, damit VideoToolbox die Bilder ohne Kopie nimmt -
    // genau wie die der Aufnahme.
    NSDictionary *attrs = @{ (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey: @{} };
    for (int k = 0; k < QC_TESTBILD_N; k++) {
        CVPixelBufferRef pb = NULL;
        if (CVPixelBufferCreate(kCFAllocatorDefault, (size_t)w, (size_t)h, fmt,
                                (__bridge CFDictionaryRef)attrs, &pb) != kCVReturnSuccess || !pb) {
            qc_testbild_stop();
            return;
        }
        bild_fuellen(pb, k);
        g_bilder[k] = pb;
        g_n = k + 1;
    }
    g_w = w; g_h = h; g_fmt = fmt; g_i = 0;
}

void qc_testbild_stop(void) {
    for (int k = 0; k < g_n; k++) {
        if (g_bilder[k]) CVPixelBufferRelease(g_bilder[k]);
        g_bilder[k] = NULL;
    }
    g_n = 0; g_i = 0; g_w = 0; g_h = 0; g_fmt = 0;
}

CVPixelBufferRef qc_testbild_naechstes(void) {
    if (g_n == 0) return NULL;
    CVPixelBufferRef pb = g_bilder[g_i];
    g_i = (g_i + 1) % g_n;
    return pb;
}
