// HDR10 (BT.2020, PQ): der C-Spiegel von client/src/hdr.rs - Beschreibung
// und Nachrichtenformate in hdr.h.
#include "hdr.h"

#include <math.h>
#include <string.h>

static void u16_schreiben(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)(v & 0xff);
    p[1] = (uint8_t)(v >> 8);
}

static uint16_t u16_lesen(const uint8_t *p) {
    return (uint16_t)(p[0] | (p[1] << 8));
}

// ------------------------------------------------------------- Strominfo

void qc_hdr_info_sdr(qc_hdr_info *i, uint8_t grund) {
    memset(i, 0, sizeof *i);
    i->transfer = QC_HDR_TRANSFER_SDR;
    i->primaer = QC_HDR_PRIMAER_709;
    i->matrix = QC_HDR_MATRIX_709;
    i->voll = 1;
    i->grund = grund;
}

void qc_hdr_info_kodieren(const qc_hdr_info *i, uint8_t out[QC_HDR_INFO_LAENGE - QC_HDR_INFO_ALT]) {
    out[0] = QC_HDR_INFO_FASSUNG;
    out[1] = i->transfer;
    out[2] = i->primaer;
    out[3] = i->matrix;
    out[4] = i->voll ? 1 : 0;
    out[5] = i->grund;
    u16_schreiben(out + 6, i->sdr_weiss_nit);
    u16_schreiben(out + 8, i->master_max_nit);
    u16_schreiben(out + 10, i->master_min_zehntausendstel);
    u16_schreiben(out + 12, i->max_cll);
    u16_schreiben(out + 14, i->max_fall);
}

// ------------------------------------------------------------- IN_ANZEIGE

int qc_hdr_anzeige_lesen(const uint8_t *p, size_t n, qc_hdr_anzeige *a) {
    if (!p || !a || n < QC_HDR_ANZEIGE_LAENGE || p[0] != QC_HDR_ANZEIGE_FASSUNG) return 0;
    a->flags = p[1];
    a->wunsch = p[2];
    a->sdr_weiss_nit = u16_lesen(p + 4);
    a->spitze_nit = u16_lesen(p + 6);
    a->vollbild_spitze_nit = u16_lesen(p + 8);
    a->kopfraum_potentiell = u16_lesen(p + 10);
    a->kopfraum_aktuell = u16_lesen(p + 12);
    return 1;
}

// ----------------------------------------------------------- Entscheidung

int qc_hdr_codec_kann(int idx) {
    return idx == 0 || idx == 2;
}

int qc_hdr_entscheiden(int quelle_hdr, int host_kann, int idx, const qc_hdr_anzeige *a) {
    if (!a) return QC_HDR_GRUND_KEIN_IN_ANZEIGE;
    // Ein unbekannter Wunsch gilt wie Aus: Unsicherheit ergibt SDR.
    if (a->wunsch != QC_HDR_WUNSCH_AUTOMATISCH && a->wunsch != QC_HDR_WUNSCH_IMMER) return QC_HDR_GRUND_CLIENT_SDR;
    if (!qc_hdr_codec_kann(idx)) return QC_HDR_GRUND_CODEC;
    if (!host_kann) return QC_HDR_GRUND_HOST_KANN_NICHT;
    if (!quelle_hdr) return QC_HDR_GRUND_HOST_SCHIRM_SDR;
    if (!(a->flags & QC_HDR_ANZEIGE_DARSTELLUNG)) return QC_HDR_GRUND_CLIENT_OHNE_DARSTELLUNG;
    if (a->wunsch == QC_HDR_WUNSCH_AUTOMATISCH && !(a->flags & QC_HDR_ANZEIGE_SCHIRM_HDR)) return QC_HDR_GRUND_CLIENT_SDR;
    return QC_HDR_GRUND_AKTIV;
}

const char *qc_hdr_grund_text(int grund) {
    switch (grund) {
        case QC_HDR_GRUND_AKTIV: return "HDR aktiv";
        case QC_HDR_GRUND_CLIENT_SDR: return "Client-Bildschirm SDR oder HDR aus";
        case QC_HDR_GRUND_CODEC: return "Codec nicht HEVC 10 Bit";
        case QC_HDR_GRUND_HOST_SCHIRM_SDR: return "Bildschirm des Hosts SDR";
        case QC_HDR_GRUND_HOST_KANN_NICHT: return "Host kann kein HDR (System oder Encoder)";
        case QC_HDR_GRUND_CLIENT_OHNE_DARSTELLUNG: return "Client kann HDR nicht darstellen";
        case QC_HDR_GRUND_WECHSEL_GESCHEITERT: return "Wechsel nach HDR gescheitert";
        case QC_HDR_GRUND_KEIN_IN_ANZEIGE: return "kein IN_ANZEIGE vom Client";
        default: return "unbekannter Grund";
    }
}

// -------------------------------------------------------------------- PQ

// SMPTE ST 2084, dieselben Konstanten wie hdr.rs (PQ_M1 ... PQ_C3).
#define PQ_M1 0.1593017578125      // 2610 / 16384
#define PQ_M2 78.84375             // 2523 / 4096 * 128
#define PQ_C1 0.8359375            // 3424 / 4096
#define PQ_C2 18.8515625           // 2413 / 4096 * 32
#define PQ_C3 18.6875              // 2392 / 4096 * 32
#define PQ_SPITZE_NIT 10000.0

double qc_hdr_pq_aus_nit(double nit) {
    double y = nit / PQ_SPITZE_NIT;
    if (!(y > 0.0)) y = 0.0;       // auch NaN
    if (y > 1.0) y = 1.0;
    double ym = pow(y, PQ_M1);
    return pow((PQ_C1 + PQ_C2 * ym) / (1.0 + PQ_C3 * ym), PQ_M2);
}

double qc_hdr_nit_aus_pq(double e) {
    if (!(e > 0.0)) e = 0.0;
    if (e > 1.0) e = 1.0;
    double p = pow(e, 1.0 / PQ_M2);
    double z = p - PQ_C1;
    if (z < 0.0) z = 0.0;
    return pow(z / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1) * PQ_SPITZE_NIT;
}

int qc_hdr_pq_code10(double nit) {
    return (int)lround(qc_hdr_pq_aus_nit(nit) * 1023.0);
}

// ------------------------------------------------------------------- SEI

void qc_hdr_sei_primaer(qc_hdr_sei_werte *w, int bt2020) {
    // Gruen, Blau, Rot in 0,00002; Weisspunkt D65 (0,3127 / 0,3290).
    static const uint16_t p3[6] = { 13250, 34500, 7500, 3000, 34000, 16000 };
    static const uint16_t r2020[6] = { 8500, 39850, 6550, 2300, 35400, 14600 };
    const uint16_t *q = bt2020 ? r2020 : p3;
    for (int c = 0; c < 3; c++) {
        w->x[c] = q[2 * c];
        w->y[c] = q[2 * c + 1];
    }
    w->weiss_x = 15635;
    w->weiss_y = 16450;
}

static size_t be16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)(v & 0xff);
    return 2;
}

static size_t be32(uint8_t *p, uint32_t v) {
    for (int k = 0; k < 4; k++) p[k] = (uint8_t)(v >> (24 - 8 * k));
    return 4;
}

size_t qc_hdr_sei_bauen(const qc_hdr_sei_werte *w, uint8_t *out, size_t platz) {
    // RBSP: zwei SEI-Nachrichten (Typ und Groesse je ein Byte, beide < 255),
    // dann rbsp_trailing_bits.
    uint8_t roh[40];
    size_t r = 0;
    roh[r++] = 137;
    roh[r++] = 24;
    for (int c = 0; c < 3; c++) {
        r += be16(roh + r, w->x[c]);
        r += be16(roh + r, w->y[c]);
    }
    r += be16(roh + r, w->weiss_x);
    r += be16(roh + r, w->weiss_y);
    r += be32(roh + r, w->max_lum);
    r += be32(roh + r, w->min_lum);
    roh[r++] = 144;
    roh[r++] = 4;
    r += be16(roh + r, w->max_cll);
    r += be16(roh + r, w->max_fall);
    roh[r++] = 0x80;

    // Kopf: forbidden_zero_bit 0, nal_unit_type 39 (PREFIX_SEI), nuh_layer_id 0,
    // nuh_temporal_id_plus1 1. Dahinter die Nutzlast mit Emulationsschutz: vor
    // jedem Byte 00..03 hinter zwei Nullen ein 03 (die RBSP endet auf 0x80).
    if (platz < 2 + r + r / 2) return 0;
    size_t n = 0;
    out[n++] = 39 << 1;
    out[n++] = 1;
    int nullen = 0;
    for (size_t i = 0; i < r; i++) {
        if (nullen >= 2 && roh[i] <= 3) {
            out[n++] = 3;
            nullen = 0;
        }
        out[n++] = roh[i];
        nullen = roh[i] == 0 ? nullen + 1 : 0;
    }
    return n;
}
