#import <Foundation/Foundation.h>
#include <math.h>
#include <string.h>

#include "testbild.h"
#include "hdr.h"

// Zwoelf Bilder je Schleife: bei 1080p in 4:4:4 10 Bit sind das 150 MB -
// das ist der Preis dafuer, dass waehrend der Messung nichts gemalt wird.
// Bewegungen sind so angelegt, dass die Schleife nahtlos ist.
#define QC_TESTBILD_N 12

static CVPixelBufferRef g_bilder[QC_TESTBILD_N];
static int g_n = 0, g_i = 0;
static int g_w = 0, g_h = 0;
static OSType g_fmt = 0;
static int g_pq = 0;

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

// ------------------------------------------------------------ HDR (PQ)
//
// Ein SDR-Bildpunkt (8 Bit Y'CbCr BT.709, voller Bereich - so liest ihn der
// Client) als HDR10: R'G'B' BT.709, stueckweise sRGB-EOTF (wie hdr.rs fuer
// SDR), mal SDR-Weiss in nit mal verstaerkung, BT.709 -> BT.2020, PQ, Y'CbCr
// BT.2020-NCL, 10 Bit voll. Ganz ohne Zustand, also fadensicher.

static double sdr_linear(double v) {
    if (v <= 0.0) return 0.0;
    if (v >= 1.0) return 1.0;
    return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4);
}

static int code10(double v, double versatz) {
    long c = lround(v * 1023.0 + versatz);
    return (int)(c < 0 ? 0 : c > 1023 ? 1023 : c);
}

// R'G'B' BT.2020 in PQ (0..1) -> 10-Bit-Codes Y'CbCr BT.2020-NCL, voll.
static void pq_rgb_nach_codes(const double rgb[3], int *yy, int *cb, int *cr) {
    double y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
    *yy = code10(y, 0.0);
    *cb = code10((rgb[2] - y) / 1.8814, 512.0);
    *cr = code10((rgb[0] - y) / 1.4746, 512.0);
}

static void sdr_nach_pq(uint8_t y8, uint8_t cb8, uint8_t cr8, double verstaerkung, int *yy, int *cb, int *cr) {
    double y = y8 / 255.0, u = (cb8 - 128) / 255.0, v = (cr8 - 128) / 255.0;
    double rgb709[3] = { sdr_linear(y + 1.5748 * v), sdr_linear(y - 0.187324 * u - 0.468124 * v), sdr_linear(y + 1.8556 * u) };
    // BT.709 -> BT.2020, linear (BT.2087; dieselbe Matrix wie hdr.rs M_709_NACH_2020).
    static const double m[3][3] = {
        { 0.6274039, 0.32928304, 0.043313066 },
        { 0.06909729, 0.9195404, 0.011362316 },
        { 0.016391439, 0.08801331, 0.89559525 },
    };
    double nit = QC_TESTBILD_PQ_WEISS_NIT * verstaerkung, rgb[3];
    for (int i = 0; i < 3; i++)
        rgb[i] = qc_hdr_pq_aus_nit(nit * (m[i][0] * rgb709[0] + m[i][1] * rgb709[1] + m[i][2] * rgb709[2]));
    pq_rgb_nach_codes(rgb, yy, cb, cr);
}

// Das Quadrat im HDR-Bild: vierfache SDR-Helligkeit, eine Spitze ueber dem
// SDR-Weiss, die nur ein HDR-Schirm zeigt.
#define QC_TESTBILD_PQ_QUADRAT 4.0

void qc_testbild_punkt_pq(int x, int y, int w, int h, int k, int *yy, int *cb, int *cr) {
    uint8_t y8, cb8, cr8;
    punkt(x, y, w, h, k, &y8, &cb8, &cr8);
    if (y >= 2 * h / 3) {
        // Unteres Drittel: statt des SDR-Verlaufs eine PQ-Rampe 0..1000 nit
        // in Grau; das Quadrat darauf in vierfacher SDR-Helligkeit.
        if (y8 == 200 && cb8 == 60 && cr8 == 220) {
            sdr_nach_pq(y8, cb8, cr8, QC_TESTBILD_PQ_QUADRAT, yy, cb, cr);
            return;
        }
        *yy = qc_hdr_pq_code10(QC_TESTBILD_PQ_RAMPE_NIT * x / (w > 1 ? w - 1 : 1));
        *cb = 512;
        *cr = 512;
        return;
    }
    sdr_nach_pq(y8, cb8, cr8, 1.0, yy, cb, cr);
}

// Das HDR-Bild k in einen xf44-Puffer. Die SDR-Punkte sind nur eine Handvoll
// Farben: je Farbe einmal rechnen (Grau nach Y, die wenigen bunten in einer
// kleinen Liste), die Rampe einmal je Spalte.
static void bild_fuellen_pq(CVPixelBufferRef pb, int k) {
    int w = (int)CVPixelBufferGetWidth(pb), h = (int)CVPixelBufferGetHeight(pb);
    uint8_t *p0 = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
    uint8_t *p1 = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
    size_t s0 = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    size_t s1 = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
    int grau[256][3];
    uint8_t grau_da[256] = {0};
    struct { uint8_t y, cb, cr; int c[3]; } bunt[32];
    int n_bunt = 0;
    int *rampe = malloc(sizeof(int) * (size_t)(w > 0 ? w : 1));
    if (!rampe) return;
    for (int x = 0; x < w; x++) rampe[x] = qc_hdr_pq_code10(QC_TESTBILD_PQ_RAMPE_NIT * x / (w > 1 ? w - 1 : 1));
    int quadrat[3];
    sdr_nach_pq(200, 60, 220, QC_TESTBILD_PQ_QUADRAT, &quadrat[0], &quadrat[1], &quadrat[2]);
    for (int y = 0; y < h; y++) {
        uint16_t *zy = (uint16_t *)(p0 + (size_t)y * s0);
        uint16_t *zc = (uint16_t *)(p1 + (size_t)y * s1);
        for (int x = 0; x < w; x++) {
            uint8_t y8, cb8, cr8;
            punkt(x, y, w, h, k, &y8, &cb8, &cr8);
            int c[3], *q = c;
            if (y >= 2 * h / 3) {
                if (y8 == 200 && cb8 == 60 && cr8 == 220) q = quadrat;
                else { c[0] = rampe[x]; c[1] = 512; c[2] = 512; }
            } else if (cb8 == 128 && cr8 == 128) {
                if (!grau_da[y8]) { sdr_nach_pq(y8, 128, 128, 1.0, &grau[y8][0], &grau[y8][1], &grau[y8][2]); grau_da[y8] = 1; }
                q = grau[y8];
            } else {
                int i = 0;
                while (i < n_bunt && !(bunt[i].y == y8 && bunt[i].cb == cb8 && bunt[i].cr == cr8)) i++;
                if (i == n_bunt) {
                    if (n_bunt < 32) {
                        bunt[i].y = y8; bunt[i].cb = cb8; bunt[i].cr = cr8;
                        sdr_nach_pq(y8, cb8, cr8, 1.0, &bunt[i].c[0], &bunt[i].c[1], &bunt[i].c[2]);
                        n_bunt++;
                        q = bunt[i].c;
                    } else {
                        sdr_nach_pq(y8, cb8, cr8, 1.0, &c[0], &c[1], &c[2]);
                    }
                } else {
                    q = bunt[i].c;
                }
            }
            zy[x] = (uint16_t)(q[0] << 6);
            zc[2 * x] = (uint16_t)(q[1] << 6);
            zc[2 * x + 1] = (uint16_t)(q[2] << 6);
        }
    }
    free(rampe);
}

static void bild_fuellen(CVPixelBufferRef pb, int k, int pq) {
    CVPixelBufferLockBaseAddress(pb, 0);
    OSType fmt = CVPixelBufferGetPixelFormatType(pb);
    int w = (int)CVPixelBufferGetWidth(pb), h = (int)CVPixelBufferGetHeight(pb);
    uint8_t *p0 = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
    uint8_t *p1 = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
    size_t s0 = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    size_t s1 = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
    if (pq && fmt == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange) {
        bild_fuellen_pq(pb, k);
    } else if (fmt == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange) {
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

void qc_testbild_start(int w, int h, OSType fmt, int pq) {
    // HDR gibt es nur in 4:4:4 10 Bit (xf44) - dem Aufnahmeformat der
    // HDR-Kandidaten 0 und 2.
    pq = pq && fmt == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    if (g_n && g_w == w && g_h == h && g_fmt == fmt && g_pq == pq) return;
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
        bild_fuellen(pb, k, pq);
        if (pq) {
            // Die Farbe steht dran: der Encoder (BT.2020/PQ/BT.2020) sieht
            // dieselbe und rechnet nichts um.
            CVBufferSetAttachment(pb, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_2020, kCVAttachmentMode_ShouldPropagate);
            CVBufferSetAttachment(pb, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVAttachmentMode_ShouldPropagate);
            CVBufferSetAttachment(pb, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVAttachmentMode_ShouldPropagate);
        }
        g_bilder[k] = pb;
        g_n = k + 1;
    }
    g_w = w; g_h = h; g_fmt = fmt; g_pq = pq; g_i = 0;
}

void qc_testbild_stop(void) {
    for (int k = 0; k < g_n; k++) {
        if (g_bilder[k]) CVPixelBufferRelease(g_bilder[k]);
        g_bilder[k] = NULL;
    }
    g_n = 0; g_i = 0; g_w = 0; g_h = 0; g_fmt = 0; g_pq = 0;
}

CVPixelBufferRef qc_testbild_naechstes(void) {
    if (g_n == 0) return NULL;
    CVPixelBufferRef pb = g_bilder[g_i];
    g_i = (g_i + 1) % g_n;
    return pb;
}
