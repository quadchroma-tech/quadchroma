// Zugang: Geraete-ID, Namen, Nachrichten 20-23, Passwort-Beweis, Drossel,
// Grenzen, Anfragen, Geraeteliste und Passwortdatei. Siehe zugang.h.
//
// Kryptografie nur aus CommonCrypto (CCHmac, CCKeyDerivationPBKDF,
// CC_SHA256), Zufall aus arc4random_buf - nichts selbst erfunden.

#include "zugang.h"
#include "qc_secure.h"

#include <CommonCrypto/CommonDigest.h>
#include <CommonCrypto/CommonHMAC.h>
#include <CommonCrypto/CommonCryptoError.h>
#include <CommonCrypto/CommonKeyDerivation.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

// ------------------------------------------------------------ Protokoll

static void protokoll_stdout(const char *zeile) { printf("%s\n", zeile); }
static void (*g_protokoll)(const char *) = protokoll_stdout;

void qc_zugang_protokoll_setzen(void (*fn)(const char *zeile)) {
    g_protokoll = fn ? fn : protokoll_stdout;
}

__attribute__((format(printf, 1, 2)))
static void zeile(const char *fmt, ...) {
    char z[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(z, sizeof z, fmt, ap);
    va_end(ap);
    g_protokoll(z);
}

// -------------------------------------------- Schwache Standardfassungen (13)
//
// Ohne Oberflaeche (Pruefstaende, Host ohne menue.m) gibt es kein "Zulassen":
// keine Anfrage, kein Bit 1. Die Oberflaeche ersetzt diese Fassungen einfach
// durch eigene gleichen Namens.

__attribute__((weak)) void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code) {
    (void)anfrage; (void)name; (void)id; (void)code;
}
__attribute__((weak)) void qc_ui_anfrage_zurueck(uint64_t anfrage) { (void)anfrage; }
__attribute__((weak)) void qc_ui_zustand_geaendert(void) {}
__attribute__((weak)) int qc_ui_vorhanden(void) { return 0; }

// ----------------------------------------------------------- Kleinkram

static void sha256_2(const void *a, size_t alen, const void *b, size_t blen, uint8_t out[32]) {
    CC_SHA256_CTX c;
    CC_SHA256_Init(&c);
    if (alen) CC_SHA256_Update(&c, a, (CC_LONG)alen);
    if (blen) CC_SHA256_Update(&c, b, (CC_LONG)blen);
    CC_SHA256_Final(out, &c);
}

static void le32_schreiben(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); p[2] = (uint8_t)(v >> 16); p[3] = (uint8_t)(v >> 24);
}

static uint32_t le32_lesen(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static int ist_hex(int c) {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

static int hex_wert(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return c - 'A' + 10;
}

// ------------------------------------------------------------- ID und Code

uint32_t qc_zugang_id(const uint8_t pub[32]) {
    static const char label[] = "QuadChroma/1 ID";      // mit Nullbyte: 16 Byte
    uint8_t d[32];
    sha256_2(label, sizeof label, pub, 32, d);
    uint32_t v = ((uint32_t)d[0] << 24) | ((uint32_t)d[1] << 16) | ((uint32_t)d[2] << 8) | d[3];
    return v % 1000000000u;
}

void qc_zugang_id_text(uint32_t id, char out[12]) {
    id %= 1000000000u;
    snprintf(out, 12, "%03u %03u %03u", id / 1000000u, (id / 1000u) % 1000u, id % 1000u);
}

// Dieselbe Rechnung wie qc_sas (qc_noise.c), nur als Zahl.
uint32_t qc_zugang_code(const uint8_t hh[32]) {
    static const char label[] = "QuadChroma/1 SAS";     // mit Nullbyte: 17 Byte
    uint8_t d[32];
    sha256_2(label, sizeof label, hh, 32, d);
    uint32_t v = ((uint32_t)d[0] << 24) | ((uint32_t)d[1] << 16) | ((uint32_t)d[2] << 8) | d[3];
    return v % 1000000u;
}

void qc_zugang_code_text(uint32_t code, char out[8]) {
    code %= 1000000u;
    snprintf(out, 8, "%03u %03u", code / 1000u, code % 1000u);
}

// ------------------------------------------------------------------ Namen

// Laenge der gueltigen UTF-8-Folge ab p[i], 0 = ungueltig (auch: ueberlang,
// Ersatzzeichen-Bereich, ueber U+10FFFF, abgeschnitten).
static size_t utf8_folge(const uint8_t *p, size_t n, size_t i) {
    uint8_t c = p[i];
    if (c < 0x80) return 1;
    size_t len;
    uint32_t cp, min;
    if (c >= 0xc2 && c <= 0xdf) { len = 2; cp = c & 0x1f; min = 0x80; }
    else if ((c & 0xf0) == 0xe0) { len = 3; cp = c & 0x0f; min = 0x800; }
    else if (c >= 0xf0 && c <= 0xf4) { len = 4; cp = c & 0x07; min = 0x10000; }
    else return 0;
    if (i + len > n) return 0;
    for (size_t k = 1; k < len; k++) {
        if ((p[i + k] & 0xc0) != 0x80) return 0;
        cp = (cp << 6) | (p[i + k] & 0x3f);
    }
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return 0;
    return len;
}

size_t qc_zugang_name_saeubern(const void *in, size_t n, char *out, size_t max) {
    static const uint8_t ersatz[3] = { 0xef, 0xbf, 0xbd };   // U+FFFD
    const uint8_t *p = in;
    size_t o = 0;
    for (size_t i = 0; p && i < n;) {
        size_t len = utf8_folge(p, n, i);
        const uint8_t *von = p + i;
        size_t schritt = len, kopie = len;
        if (!len) { von = ersatz; schritt = 1; kopie = 3; }
        else if (len == 1 && (p[i] < 0x20 || p[i] == 0x7f)) kopie = 0;               // C0 und DEL
        else if (len == 2 && p[i] == 0xc2 && p[i + 1] <= 0x9f) kopie = 0;             // C1
        else if (len == 3 && p[i] == 0xe2 && ((p[i + 1] == 0x80 && ((p[i + 2] >= 0x8e && p[i + 2] <= 0x8f) ||
                                                                    (p[i + 2] >= 0xaa && p[i + 2] <= 0xae))) ||
                                              (p[i + 1] == 0x81 && p[i + 2] >= 0xa6 && p[i + 2] <= 0xa9)))
            kopie = 0;      // Richtungszeichen (U+200E/F, U+202A-202E, U+2066-2069): im Zulassen-Fenster
                            // koennte ein Name sonst rueckwaerts etwas anderes vorgeben
        else if (len == 1 && p[i] == ' ' && o == 0) kopie = 0;                        // Leerzeichen vorn
        i += schritt;
        if (!kopie) continue;
        if (o + kopie > max) break;
        memcpy(out + o, von, kopie);
        o += kopie;
    }
    while (o && out[o - 1] == ' ') o--;                                               // und hinten
    out[o] = 0;
    return o;
}

int qc_zugang_name_lesen(const uint8_t *nutzlast, size_t n, char name[QC_ZUGANG_NAME_MAX + 1]) {
    name[0] = 0;
    if (!nutzlast || n < 5 || memcmp(nutzlast, "QCN1", 4) != 0) return 0;
    size_t l = nutzlast[4];
    // Was hinter dem Namen steht, bleibt fuer spaetere Fassungen frei.
    if (l > QC_ZUGANG_NAME_MAX || n < 5 + l) return 0;
    return qc_zugang_name_saeubern(nutzlast + 5, l, name, QC_ZUGANG_NAME_MAX) > 0;
}

size_t qc_zugang_name_kodieren(const char *name, uint8_t *out, size_t cap) {
    char s[QC_ZUGANG_NAME_MAX + 1];
    size_t l = qc_zugang_name_saeubern(name, name ? strlen(name) : 0, s, QC_ZUGANG_NAME_MAX);
    if (cap < 5 + l) return 0;
    memcpy(out, "QCN1", 4);
    out[4] = (uint8_t)l;
    memcpy(out + 5, s, l);
    return 5 + l;
}

// ------------------------------------------------------------- Bekanntgabe

size_t qc_zugang_bekanntgabe(uint8_t *pkt, size_t cap, uint16_t port, const char *name,
                             uint32_t id, uint8_t flags) {
    char s[QC_ZUGANG_NAME_MAX + 1];
    size_t l = qc_zugang_name_saeubern(name, name ? strlen(name) : 0, s, QC_ZUGANG_NAME_MAX);
    size_t n = 8 + l + 6;
    if (cap < n || n > QC_BEKANNTGABE_MAX) return 0;
    memcpy(pkt, "QCHB", 4);
    pkt[4] = 1;
    pkt[5] = (uint8_t)(port & 0xff);
    pkt[6] = (uint8_t)(port >> 8);
    pkt[7] = (uint8_t)l;
    memcpy(pkt + 8, s, l);
    pkt[8 + l] = QC_BEKANNTGABE_EXT;
    le32_schreiben(pkt + 9 + l, id);
    pkt[13 + l] = flags;
    return n;
}

int qc_zugang_bekanntgabe_lesen(const uint8_t *p, size_t n, uint16_t *port,
                                char name[QC_ZUGANG_NAME_MAX + 1], uint32_t *id, uint8_t *flags) {
    if (n < 8 || memcmp(p, "QCHB", 4) != 0 || p[4] != 1) return -1;
    size_t l = p[7];
    if (n < 8 + l) return -1;
    if (port) *port = (uint16_t)(p[5] | (p[6] << 8));
    if (name) qc_zugang_name_saeubern(p + 8, l, name, QC_ZUGANG_NAME_MAX);
    if (n < 8 + l + 6 || p[8 + l] != QC_BEKANNTGABE_EXT) return 0;
    if (id) *id = le32_lesen(p + 9 + l);
    if (flags) *flags = p[13 + l];
    return 1;
}

// ------------------------------------------------------------- Nachrichten

void qc_zugang_kopf(uint8_t out[QC_ZUGANG_KOPF], uint8_t typ, uint32_t laenge) {
    out[0] = typ;
    out[1] = 0;
    out[2] = out[3] = 0;
    le32_schreiben(out + 4, laenge);
}

size_t qc_zugang_noetig_kodieren(uint8_t *out, size_t cap, uint8_t wege, uint32_t warten_ms,
                                 const char *hostname) {
    char s[QC_ZUGANG_NAME_MAX + 1];
    size_t l = qc_zugang_name_saeubern(hostname, hostname ? strlen(hostname) : 0, s, QC_ZUGANG_NAME_MAX);
    size_t n = QC_ZUGANG_KOPF + 8 + l;
    if (cap < n) return 0;
    qc_zugang_kopf(out, QC_ZUGANG_NOETIG, (uint32_t)(8 + l));
    uint8_t *q = out + QC_ZUGANG_KOPF;
    q[0] = QC_ZUGANG_FASSUNG;
    q[1] = wege;
    q[2] = q[3] = 0;
    le32_schreiben(q + 4, warten_ms);
    memcpy(q + 8, s, l);
    return n;
}

int qc_zugang_noetig_lesen(const uint8_t *nutz, size_t n, uint8_t *wege, uint32_t *warten_ms,
                           char name[QC_ZUGANG_NAME_MAX + 1]) {
    if (n < 8 || n > 8 + QC_ZUGANG_NAME_MAX || nutz[0] != QC_ZUGANG_FASSUNG) return -1;
    if (wege) *wege = nutz[1];
    if (warten_ms) *warten_ms = le32_lesen(nutz + 4);
    if (name) qc_zugang_name_saeubern(nutz + 8, n - 8, name, QC_ZUGANG_NAME_MAX);
    return 0;
}

size_t qc_zugang_beweis_kodieren(uint8_t out[QC_ZUGANG_KOPF + 32], const uint8_t client_proof[32]) {
    qc_zugang_kopf(out, QC_ZUGANG_BEWEIS, 32);
    memcpy(out + QC_ZUGANG_KOPF, client_proof, 32);
    return QC_ZUGANG_KOPF + 32;
}

size_t qc_zugang_ergebnis_kodieren(uint8_t *out, size_t cap, uint8_t ergebnis, uint32_t warten_ms,
                                   const uint8_t *host_proof) {
    int mit = ergebnis == QC_ERGEBNIS_PASSWORT;
    size_t n = QC_ZUGANG_KOPF + 8 + (mit ? 32 : 0);
    if (ergebnis > QC_ERGEBNIS_SCHLUSS || (mit && !host_proof) || cap < n) return 0;
    qc_zugang_kopf(out, QC_ZUGANG_ERGEBNIS, (uint32_t)(n - QC_ZUGANG_KOPF));
    uint8_t *q = out + QC_ZUGANG_KOPF;
    q[0] = ergebnis;
    q[1] = 0;
    q[2] = q[3] = 0;
    // warten_ms gilt nur bei 2 und 4 (3.2), sonst 0.
    le32_schreiben(q + 4, ergebnis == QC_ERGEBNIS_FALSCH || ergebnis == QC_ERGEBNIS_SCHLUSS ? warten_ms : 0);
    if (mit) memcpy(q + 8, host_proof, 32);
    return n;
}

int qc_zugang_ergebnis_lesen(const uint8_t *nutz, size_t n, uint8_t *ergebnis, uint32_t *warten_ms,
                             uint8_t proof[32]) {
    if (n < 8 || nutz[0] > QC_ERGEBNIS_SCHLUSS) return -1;
    if (n != (nutz[0] == QC_ERGEBNIS_PASSWORT ? 40u : 8u)) return -1;
    if (ergebnis) *ergebnis = nutz[0];
    if (warten_ms) *warten_ms = le32_lesen(nutz + 4);
    if (proof && nutz[0] == QC_ERGEBNIS_PASSWORT) memcpy(proof, nutz + 8, 32);
    return 0;
}

size_t qc_zugang_abbruch_kodieren(uint8_t out[QC_ZUGANG_KOPF]) {
    qc_zugang_kopf(out, QC_ZUGANG_ABBRUCH, 0);
    return QC_ZUGANG_KOPF;
}

// ------------------------------------------------------------ Kryptografie

size_t qc_zugang_norm(const char *pw, uint8_t *out, size_t cap) {
    size_t n = 0;
    for (const uint8_t *p = (const uint8_t *)pw; p && *p; p++) {
        uint8_t c = *p;
        if (c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d || c == '-') continue;
        if (c >= 'A' && c <= 'Z') c = (uint8_t)(c + ('a' - 'A'));
        if (out && n < cap) out[n] = c;
        n++;
    }
    return n;
}

void qc_zugang_hmac(const void *schluessel, size_t klen, const void *daten, size_t n, uint8_t out[32]) {
    CCHmac(kCCHmacAlgSHA256, schluessel, klen, daten, n, out);
}

int qc_zugang_schluessel(const char *pw, const uint8_t host_pub[32], uint8_t k[32]) {
    static const char label[] = "QuadChroma/1 pw";      // mit Nullbyte: 16 Byte
    uint8_t norm[QC_ZUGANG_PW_MAX], salz[sizeof label + 32];
    size_t n = qc_zugang_norm(pw, norm, sizeof norm);
    if (n > sizeof norm) { qc_wipe(norm, sizeof norm); return -1; }
    memcpy(salz, label, sizeof label);
    memcpy(salz + sizeof label, host_pub, 32);
    int r = CCKeyDerivationPBKDF(kCCPBKDF2, (const char *)norm, n, salz, sizeof salz,
                                 kCCPRFHmacAlgSHA256, QC_ZUGANG_RUNDEN, k, 32);
    qc_wipe(norm, sizeof norm);
    return r == kCCSuccess ? 0 : -1;
}

void qc_zugang_beweis(const uint8_t k[32], const uint8_t hh[32], int host, uint8_t out[32]) {
    static const char c_label[] = "QuadChroma/1 auth client";   // 25 Byte mit Nullbyte
    static const char h_label[] = "QuadChroma/1 auth host";     // 23 Byte mit Nullbyte
    uint8_t d[sizeof c_label + 32];
    size_t l = host ? sizeof h_label : sizeof c_label;
    memcpy(d, host ? h_label : c_label, l);
    memcpy(d + l, hh, 32);
    qc_zugang_hmac(k, 32, d, l + 32, out);
}

int qc_zugang_gleich(const void *a, const void *b, size_t n) {
    const volatile uint8_t *x = a, *y = b;
    uint8_t d = 0;
    for (size_t i = 0; i < n; i++) d |= (uint8_t)(x[i] ^ y[i]);
    return d == 0;
}

// ----------------------------------------------------------------- Drossel

#define QC_DROSSEL_PLAETZE 128

typedef struct {
    int belegt;
    uint8_t schluessel[32];             // Adresse: die ersten 4 Byte, Rest 0
    int fehl;                           // Fehlversuche seit dem letzten Verfall
    int64_t zuletzt_ms;                 // letzter Fehlversuch
} drossel_eintrag;

static pthread_mutex_t g_drossel_mtx = PTHREAD_MUTEX_INITIALIZER;
static drossel_eintrag g_dr_ip[QC_DROSSEL_PLAETZE], g_dr_pub[QC_DROSSEL_PLAETZE];
// Die letzten 31 Fehlversuche aller Gegenstellen: liegt der aelteste davon
// weniger als 60 s zurueck, waren es mehr als 30 in 60 s.
static int64_t g_dr_global[QC_DROSSEL_GLOBAL_ANZAHL + 1];
static int g_dr_global_pos = 0, g_dr_global_n = 0;

static int64_t wartezeit(int fehl) {
    if (fehl < 3) return 0;
    int64_t w = QC_DROSSEL_ERSTE_MS;
    for (int k = 3; k < fehl && w < QC_DROSSEL_HOECHSTENS_MS; k++) w *= 2;
    return w > QC_DROSSEL_HOECHSTENS_MS ? QC_DROSSEL_HOECHSTENS_MS : w;
}

static int verfallen(const drossel_eintrag *e, int64_t jetzt) {
    return jetzt - e->zuletzt_ms >= QC_DROSSEL_VERFALL_MS;
}

static drossel_eintrag *drossel_suchen(drossel_eintrag *t, const uint8_t k[32]) {
    for (int i = 0; i < QC_DROSSEL_PLAETZE; i++)
        if (t[i].belegt && memcmp(t[i].schluessel, k, 32) == 0) return &t[i];
    return NULL;
}

// Eintrag holen oder anlegen. Ist die Tabelle voll, weicht ein verfallener
// oder sonst der mit dem aeltesten Fehlversuch.
static drossel_eintrag *drossel_holen(drossel_eintrag *t, const uint8_t k[32], int64_t jetzt) {
    drossel_eintrag *e = drossel_suchen(t, k);
    if (e) {
        if (verfallen(e, jetzt)) e->fehl = 0;
        return e;
    }
    drossel_eintrag *frei = NULL;
    for (int i = 0; i < QC_DROSSEL_PLAETZE; i++) {
        drossel_eintrag *q = &t[i];
        if (!q->belegt || verfallen(q, jetzt)) { frei = q; break; }
        if (!frei || q->zuletzt_ms < frei->zuletzt_ms) frei = q;
    }
    memset(frei, 0, sizeof *frei);
    frei->belegt = 1;
    memcpy(frei->schluessel, k, 32);
    return frei;
}

static int64_t eintrag_rest(const drossel_eintrag *e, int64_t jetzt) {
    if (!e || verfallen(e, jetzt)) return 0;
    int64_t rest = e->zuletzt_ms + wartezeit(e->fehl) - jetzt;
    return rest > 0 ? rest : 0;
}

static void ip_schluessel(uint32_t ip, uint8_t k[32]) {
    memset(k, 0, 32);
    memcpy(k, &ip, 4);
}

// Unter g_drossel_mtx.
static uint32_t warten_gesperrt(uint32_t ip, const uint8_t pub[32], int64_t jetzt) {
    uint8_t k[32];
    ip_schluessel(ip, k);
    int64_t w = eintrag_rest(drossel_suchen(g_dr_ip, k), jetzt);
    int64_t w2 = eintrag_rest(drossel_suchen(g_dr_pub, pub), jetzt);
    if (w2 > w) w = w2;
    if (g_dr_global_n > QC_DROSSEL_GLOBAL_ANZAHL &&
        jetzt - g_dr_global[g_dr_global_pos] < QC_DROSSEL_GLOBAL_FENSTER_MS && w < QC_DROSSEL_GLOBAL_WARTEN_MS)
        w = QC_DROSSEL_GLOBAL_WARTEN_MS;
    return (uint32_t)w;
}

uint32_t qc_zugang_drossel_warten(uint32_t ip, const uint8_t pub[32], int64_t jetzt_ms) {
    pthread_mutex_lock(&g_drossel_mtx);
    uint32_t w = warten_gesperrt(ip, pub, jetzt_ms);
    pthread_mutex_unlock(&g_drossel_mtx);
    return w;
}

uint32_t qc_zugang_drossel_fehler(uint32_t ip, const uint8_t pub[32], int64_t jetzt_ms) {
    uint8_t k[32];
    ip_schluessel(ip, k);
    pthread_mutex_lock(&g_drossel_mtx);
    drossel_eintrag *a = drossel_holen(g_dr_ip, k, jetzt_ms);
    a->fehl++;
    a->zuletzt_ms = jetzt_ms;
    drossel_eintrag *b = drossel_holen(g_dr_pub, pub, jetzt_ms);
    b->fehl++;
    b->zuletzt_ms = jetzt_ms;
    g_dr_global[g_dr_global_pos] = jetzt_ms;
    g_dr_global_pos = (g_dr_global_pos + 1) % (QC_DROSSEL_GLOBAL_ANZAHL + 1);
    if (g_dr_global_n <= QC_DROSSEL_GLOBAL_ANZAHL) g_dr_global_n++;
    uint32_t w = warten_gesperrt(ip, pub, jetzt_ms);
    pthread_mutex_unlock(&g_drossel_mtx);
    return w;
}

void qc_zugang_drossel_erfolg(uint32_t ip, const uint8_t pub[32]) {
    uint8_t k[32];
    ip_schluessel(ip, k);
    pthread_mutex_lock(&g_drossel_mtx);
    drossel_eintrag *a = drossel_suchen(g_dr_ip, k), *b = drossel_suchen(g_dr_pub, pub);
    if (a) memset(a, 0, sizeof *a);
    if (b) memset(b, 0, sizeof *b);
    pthread_mutex_unlock(&g_drossel_mtx);
}

void qc_zugang_drossel_leeren(void) {
    pthread_mutex_lock(&g_drossel_mtx);
    memset(g_dr_ip, 0, sizeof g_dr_ip);
    memset(g_dr_pub, 0, sizeof g_dr_pub);
    memset(g_dr_global, 0, sizeof g_dr_global);
    g_dr_global_pos = g_dr_global_n = 0;
    pthread_mutex_unlock(&g_drossel_mtx);
}

// ------------------------------------------------------------ Zugangsphasen

typedef struct { int belegt; uint32_t ip; uint8_t pub[32]; } phase;

static pthread_mutex_t g_phasen_mtx = PTHREAD_MUTEX_INITIALIZER;
static phase g_phasen[QC_ZUGANG_PHASEN];

int qc_zugang_phase_beginnen(uint32_t ip, const uint8_t pub[32]) {
    int frei = -1, je_ip = 0, je_pub = 0;
    pthread_mutex_lock(&g_phasen_mtx);
    for (int i = 0; i < QC_ZUGANG_PHASEN; i++) {
        if (!g_phasen[i].belegt) { if (frei < 0) frei = i; continue; }
        if (g_phasen[i].ip == ip) je_ip++;
        if (memcmp(g_phasen[i].pub, pub, 32) == 0) je_pub++;
    }
    if (frei >= 0 && je_ip < QC_ZUGANG_PHASEN_JE_IP && je_pub < QC_ZUGANG_PHASEN_JE_SCHLUESSEL) {
        g_phasen[frei].belegt = 1;
        g_phasen[frei].ip = ip;
        memcpy(g_phasen[frei].pub, pub, 32);
    } else {
        frei = -1;
    }
    pthread_mutex_unlock(&g_phasen_mtx);
    return frei;
}

void qc_zugang_phase_ende(int platz) {
    if (platz < 0 || platz >= QC_ZUGANG_PHASEN) return;
    pthread_mutex_lock(&g_phasen_mtx);
    memset(&g_phasen[platz], 0, sizeof g_phasen[platz]);
    pthread_mutex_unlock(&g_phasen_mtx);
}

int qc_zugang_phasen_offen(void) {
    int n = 0;
    pthread_mutex_lock(&g_phasen_mtx);
    for (int i = 0; i < QC_ZUGANG_PHASEN; i++) n += g_phasen[i].belegt;
    pthread_mutex_unlock(&g_phasen_mtx);
    return n;
}

// ---------------------------------------------------------------- Anfragen
//
// Je Zugangsphase hoechstens eine Anfrage, also nie mehr als QC_ZUGANG_PHASEN.
// Gezeigt wird die aelteste offene; die Oberflaeche erfaehrt jeden Wechsel
// unter g_anfrage_mtx, damit "zurueck" und "naechste" in dieser Reihenfolge
// bei ihr ankommen.

#define QC_ANFRAGEN (QC_ZUGANG_PHASEN * 2)

typedef struct {
    uint64_t nr;                        // 0 = frei
    char name[QC_ZUGANG_NAME_MAX + 1];
    uint32_t id, code;
    int weck_fd;
    int stand;                          // -1 offen, 0 abgelehnt, 1 zugelassen
    int gezeigt;                        // die Oberflaeche kennt sie
} anfrage;

static pthread_mutex_t g_anfrage_mtx = PTHREAD_MUTEX_INITIALIZER;
static anfrage g_anfragen[QC_ANFRAGEN];
static uint64_t g_anfrage_nr = 0;

static anfrage *anfrage_suchen(uint64_t nr) {
    if (!nr) return NULL;
    for (int i = 0; i < QC_ANFRAGEN; i++) if (g_anfragen[i].nr == nr) return &g_anfragen[i];
    return NULL;
}

// Unter g_anfrage_mtx: ist eine offene Anfrage gezeigt? Sonst die aelteste
// offene zeigen.
static void anfrage_zeigen(void) {
    anfrage *aelteste = NULL;
    for (int i = 0; i < QC_ANFRAGEN; i++) {
        anfrage *a = &g_anfragen[i];
        if (!a->nr || a->stand != -1) continue;
        if (a->gezeigt) return;
        if (!aelteste || a->nr < aelteste->nr) aelteste = a;
    }
    if (!aelteste) return;
    aelteste->gezeigt = 1;
    qc_ui_anfrage(aelteste->nr, aelteste->name, aelteste->id, aelteste->code);
}

uint64_t qc_zugang_anfrage_stellen(const char *name, uint32_t id, uint32_t code, int weck_fd) {
    uint64_t nr = 0;
    pthread_mutex_lock(&g_anfrage_mtx);
    for (int i = 0; i < QC_ANFRAGEN; i++) {
        anfrage *a = &g_anfragen[i];
        if (a->nr) continue;
        memset(a, 0, sizeof *a);
        a->nr = nr = ++g_anfrage_nr;
        qc_zugang_name_saeubern(name, name ? strlen(name) : 0, a->name, QC_ZUGANG_NAME_MAX);
        a->id = id;
        a->code = code;
        a->weck_fd = weck_fd;
        a->stand = -1;
        anfrage_zeigen();
        break;
    }
    pthread_mutex_unlock(&g_anfrage_mtx);
    return nr;
}

int qc_zugang_anfrage_stand(uint64_t nr) {
    pthread_mutex_lock(&g_anfrage_mtx);
    anfrage *a = anfrage_suchen(nr);
    int s = a ? a->stand : -2;
    pthread_mutex_unlock(&g_anfrage_mtx);
    return s;
}

void qc_zugang_anfrage_zurueckziehen(uint64_t nr) {
    pthread_mutex_lock(&g_anfrage_mtx);
    anfrage *a = anfrage_suchen(nr);
    if (a) {
        // Eine entschiedene hat die Oberflaeche schon abgemeldet bekommen.
        int melden = a->gezeigt && a->stand == -1;
        memset(a, 0, sizeof *a);
        if (melden) qc_ui_anfrage_zurueck(nr);
        anfrage_zeigen();
    }
    pthread_mutex_unlock(&g_anfrage_mtx);
}

void qc_zugang_entscheiden(uint64_t nr, int zulassen) {
    pthread_mutex_lock(&g_anfrage_mtx);
    anfrage *a = anfrage_suchen(nr);
    if (a && a->stand == -1) {
        a->stand = zulassen ? 1 : 0;
        if (a->weck_fd >= 0) {
            uint8_t b = 1;
            (void)write(a->weck_fd, &b, 1);        // nicht blockierend; ein Byte genuegt
        }
        if (a->gezeigt) qc_ui_anfrage_zurueck(nr);
        anfrage_zeigen();
    }
    pthread_mutex_unlock(&g_anfrage_mtx);
}

// -------------------------------------------------------------- Dateien

#define QC_DATEI_GERAETE   "host-devices.txt"
#define QC_DATEI_PASSWORT  "host-password.txt"
#define QC_DATEI_ALT       "authorized.txt"
#define QC_DATEI_INSTANZ   "host-instanz.lock"
#define QC_LISTE_MAX       (1 << 20)    // groesser ist es keine Geraeteliste mehr

// Gibt es die Datei wirklich nicht? Ein Verweis ins Leere meldet beim Oeffnen
// ebenfalls ENOENT - dann ist sie aber nicht weg, sondern verbogen.
static int fehlt(const char *pfad, int oeffnen_errno) {
    struct stat st;
    return oeffnen_errno == ENOENT && lstat(pfad, &st) != 0 && errno == ENOENT;
}

// Ganze Datei lesen. 0 = gelesen (*inhalt mit Nullbyte dahinter, freigeben),
// 1 = gibt es nicht, -1 = nicht lesbar oder groesser als max.
static int datei_lesen(const char *pfad, size_t max, char **inhalt, size_t *n) {
    *inhalt = NULL;
    *n = 0;
    int fd = open(pfad, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return fehlt(pfad, errno) ? 1 : -1;
    struct stat st;
    if (fstat(fd, &st) != 0 || !S_ISREG(st.st_mode) || st.st_size < 0 || (size_t)st.st_size > max) {
        close(fd);
        return -1;
    }
    char *b = malloc((size_t)st.st_size + 1);
    size_t gelesen = 0;
    int ok = b != NULL;
    while (ok && gelesen < (size_t)st.st_size) {
        ssize_t r = read(fd, b + gelesen, (size_t)st.st_size - gelesen);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) { ok = 0; break; }
        gelesen += (size_t)r;
    }
    // Waechst sie waehrenddessen, stimmt die Groesse nicht mehr: dann auch kaputt.
    char mehr;
    if (ok && read(fd, &mehr, 1) != 0) ok = 0;
    close(fd);
    if (!ok) { free(b); return -1; }
    b[gelesen] = 0;
    *inhalt = b;
    *n = gelesen;
    return 0;
}

// Atomar ersetzen: <pfad>.neu schreiben, fsync, 0600, rename. 0 = Erfolg.
static int datei_ersetzen(const char *pfad, const char *inhalt, size_t n) {
    char neu[1300];
    snprintf(neu, sizeof neu, "%s.neu", pfad);
    unlink(neu);
    int fd = open(neu, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    int ok = 1;
    for (size_t o = 0; o < n && ok;) {
        ssize_t w = write(fd, inhalt + o, n - o);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) ok = 0; else o += (size_t)w;
    }
    if (ok && fchmod(fd, 0600) != 0) ok = 0;
    if (ok && fsync(fd) != 0) ok = 0;
    if (close(fd) != 0) ok = 0;
    if (ok && rename(neu, pfad) != 0) ok = 0;
    if (!ok) unlink(neu);
    return ok ? 0 : -1;
}

static void heute(char out[11]) {
    time_t t = time(NULL);
    struct tm tm;
    localtime_r(&t, &tm);
    snprintf(out, 11, "%04d-%02d-%02d", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
}

// ----------------------------------------------------------- Geraeteliste

typedef struct {
    qc_geraet *e;
    int n, cap;
} liste;

static void liste_frei(liste *l) {
    free(l->e);
    memset(l, 0, sizeof *l);
}

static int liste_suchen(const liste *l, const uint8_t pub[32]) {
    for (int i = 0; i < l->n; i++) if (memcmp(l->e[i].pub, pub, 32) == 0) return i;
    return -1;
}

static int liste_anhaengen(liste *l, const uint8_t pub[32], const char *datum, const char *name, size_t nlen) {
    if (liste_suchen(l, pub) >= 0) return 0;            // doppelt: der erste gilt
    if (l->n == l->cap) {
        int cap = l->cap ? l->cap * 2 : 16;
        qc_geraet *e = realloc(l->e, (size_t)cap * sizeof *e);
        if (!e) return -1;
        l->e = e;
        l->cap = cap;
    }
    qc_geraet *g = &l->e[l->n++];
    memset(g, 0, sizeof *g);
    memcpy(g->pub, pub, 32);
    g->id = qc_zugang_id(pub);
    snprintf(g->datum, sizeof g->datum, "%s", datum);
    qc_zugang_name_saeubern(name, nlen, g->name, QC_ZUGANG_NAME_MAX);
    return 0;
}

static int ist_ziffer(char c) { return c >= '0' && c <= '9'; }

static int datum_gueltig(const char *d) {
    for (int i = 0; i < 10; i++) {
        if (i == 4 || i == 7) { if (d[i] != '-') return 0; }
        else if (!ist_ziffer(d[i])) return 0;
    }
    int m = (d[5] - '0') * 10 + (d[6] - '0'), t = (d[8] - '0') * 10 + (d[9] - '0');
    return m >= 1 && m <= 12 && t >= 1 && t <= 31;
}

// Eine Zeile ohne Zeilenwechsel: 1 = Geraet, 0 = leer oder Kommentar, -1 =
// etwas anderes - dann ist die Liste beschaedigt. Der Name ist nur Anzeige:
// Steuerzeichen und Ueberlaenge werden still gesaeubert statt die ganze Liste
// zu verwerfen.
static int geraete_zeile(const char *z, size_t n, liste *l) {
    if (n && z[n - 1] == '\r') n--;                      // von Hand unter Windows bearbeitet
    if (memchr(z, 0, n)) return -1;                     // Nullbytes: Muell nach einem Absturz
    size_t i = 0;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    if (i == n || z[i] == '#') return 0;
    if (i != 0 || n < 64 + 1 + 10) return -1;
    uint8_t pub[32];
    for (int k = 0; k < 64; k++) if (!ist_hex((unsigned char)z[k])) return -1;
    for (int k = 0; k < 32; k++) pub[k] = (uint8_t)(hex_wert(z[2 * k]) << 4 | hex_wert(z[2 * k + 1]));
    i = 64;
    if (z[i] != ' ' && z[i] != '\t') return -1;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    if (n - i < 10 || !datum_gueltig(z + i)) return -1;
    char datum[11];
    memcpy(datum, z + i, 10);
    datum[10] = 0;
    i += 10;
    if (i < n && z[i] != ' ' && z[i] != '\t') return -1;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    return liste_anhaengen(l, pub, datum, z + i, n - i) == 0 ? 1 : -1;
}

static pthread_mutex_t g_liste_mtx = PTHREAD_MUTEX_INITIALIZER;
static int g_liste_defekt_gemeldet = 0;     // unter g_liste_mtx

// Unter g_liste_mtx. 0 = gelesen (auch: gibt es nicht), -1 = unlesbar oder
// beschaedigt.
static int liste_lesen(liste *l) {
    memset(l, 0, sizeof *l);
    char pfad[1200];
    if (qc_config_path(QC_DATEI_GERAETE, pfad, sizeof pfad)) return -1;
    char *b;
    size_t n;
    int r = datei_lesen(pfad, QC_LISTE_MAX, &b, &n);
    if (r == 1) return 0;
    if (r < 0) return -1;
    int kaputt = 0;
    for (size_t a = 0; a < n && !kaputt;) {
        char *e = memchr(b + a, '\n', n - a);
        size_t len = e ? (size_t)(e - (b + a)) : n - a;
        if (geraete_zeile(b + a, len, l) < 0) kaputt = 1;
        a += len + 1;
    }
    free(b);
    if (kaputt) { liste_frei(l); return -1; }
    return 0;
}

// Unter g_liste_mtx. Schreibt die ganze Liste neu.
static int liste_schreiben(const liste *l) {
    char pfad[1200];
    if (qc_config_path(QC_DATEI_GERAETE, pfad, sizeof pfad)) return -1;
    size_t cap = (size_t)l->n * (64 + 2 + 10 + 2 + QC_ZUGANG_NAME_MAX + 1) + 1;
    char *b = malloc(cap), *p = b;
    if (!b) return -1;
    for (int i = 0; i < l->n; i++) {
        const qc_geraet *g = &l->e[i];
        for (int k = 0; k < 32; k++) p += snprintf(p, 3, "%02x", g->pub[k]);
        p += snprintf(p, cap - (size_t)(p - b), "  %s  %s\n", g->datum, g->name[0] ? g->name : "?");
    }
    int r = datei_ersetzen(pfad, b, (size_t)(p - b));
    free(b);
    return r;
}

// Unter g_liste_mtx: eine beschaedigte Liste einmal melden, bis sie wieder
// heil ist; die Oberflaeche erfaehrt jeden Wechsel ("Geraeteliste beschaedigt").
static void defekt_melden(int defekt) {
    if (defekt && !g_liste_defekt_gemeldet)
        zeile("Geraeteliste host-devices.txt nicht lesbar oder beschaedigt - niemand gilt als bekannt, "
              "jedes Geraet braucht Passwort oder Zulassen; die Datei bleibt unangetastet");
    int wechsel = defekt != g_liste_defekt_gemeldet;
    g_liste_defekt_gemeldet = defekt;
    if (wechsel) qc_ui_zustand_geaendert();
}

int qc_zugang_bekannt(const uint8_t pub[32], char name[QC_ZUGANG_NAME_MAX + 1]) {
    if (name) name[0] = 0;
    liste l;
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    defekt_melden(r < 0);
    pthread_mutex_unlock(&g_liste_mtx);
    if (r < 0) return -1;
    int i = liste_suchen(&l, pub);
    if (i >= 0 && name) memcpy(name, l.e[i].name, sizeof l.e[i].name);
    liste_frei(&l);
    return i >= 0;
}

int qc_zugang_eintragen(const uint8_t pub[32], const char *name) {
    liste l;
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    defekt_melden(r < 0);
    int neu = 0;
    if (r == 0 && liste_suchen(&l, pub) < 0) {
        char datum[11];
        heute(datum);
        r = liste_anhaengen(&l, pub, datum, name ? name : "", name ? strlen(name) : 0);
        if (r == 0) r = liste_schreiben(&l);
        neu = r == 0;
    }
    pthread_mutex_unlock(&g_liste_mtx);
    liste_frei(&l);
    if (neu) qc_ui_zustand_geaendert();
    return r;
}

int qc_zugang_geraete(qc_geraet *ziel, int max) {
    liste l;
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    defekt_melden(r < 0);
    pthread_mutex_unlock(&g_liste_mtx);
    if (r < 0) return -1;
    for (int i = 0; ziel && i < l.n && i < max; i++) ziel[i] = l.e[i];
    int n = l.n;
    liste_frei(&l);
    return n;
}

static void (*g_entfernt)(const uint8_t *pub) = NULL;

void qc_zugang_entfernt_setzen(void (*fn)(const uint8_t *pub)) { g_entfernt = fn; }

int qc_zugang_geraet_entfernen(const uint8_t pub[32]) {
    liste l;
    char name[QC_ZUGANG_NAME_MAX + 1] = {0};
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    defekt_melden(r < 0);
    int i = r == 0 ? liste_suchen(&l, pub) : -1;
    if (i >= 0) {
        memcpy(name, l.e[i].name, sizeof name);
        memmove(&l.e[i], &l.e[i + 1], (size_t)(l.n - i - 1) * sizeof l.e[0]);
        l.n--;
        r = liste_schreiben(&l);
    }
    pthread_mutex_unlock(&g_liste_mtx);
    liste_frei(&l);
    if (r != 0) {
        zeile("Geraet konnte nicht entfernt werden: Geraeteliste %s",
              i >= 0 ? "liess sich nicht schreiben" : "beschaedigt");
        return -1;
    }
    if (i >= 0) {
        char id[12];
        qc_zugang_id_text(qc_zugang_id(pub), id);
        zeile("Geraet entfernt: %s (ID %s)", name, id);
        qc_ui_zustand_geaendert();
    }
    // Auch wenn es nicht (mehr) in der Liste stand: eine laufende Sitzung
    // dieses Schluessels endet - wer entfernt, will ihn draussen haben.
    if (g_entfernt) g_entfernt(pub);
    return 0;
}

int qc_zugang_alle_entfernen(void) {
    liste l;
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    defekt_melden(r < 0);
    int n = l.n;
    if (r == 0) {
        liste leer = {0};
        r = liste_schreiben(&leer);
    }
    pthread_mutex_unlock(&g_liste_mtx);
    liste_frei(&l);
    if (r != 0) {
        zeile("Geraete konnten nicht entfernt werden: Geraeteliste beschaedigt oder nicht schreibbar");
        return -1;
    }
    zeile("Alle Geraete entfernt (%d)", n);
    qc_ui_zustand_geaendert();
    if (g_entfernt) g_entfernt(NULL);
    return 0;
}

int qc_zugang_liste_zuruecksetzen(void) {
    char pfad[1200], alt[1300];
    if (qc_config_path(QC_DATEI_GERAETE, pfad, sizeof pfad)) return -1;
    liste l;
    pthread_mutex_lock(&g_liste_mtx);
    int r = liste_lesen(&l);
    liste_frei(&l);
    if (r == 0) {                       // heil: nichts zu tun
        pthread_mutex_unlock(&g_liste_mtx);
        return 0;
    }
    time_t t = time(NULL);
    struct tm tm;
    localtime_r(&t, &tm);
    snprintf(alt, sizeof alt, "%s.defekt-%04d%02d%02d-%02d%02d%02d", pfad, tm.tm_year + 1900, tm.tm_mon + 1,
             tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
    r = rename(pfad, alt);
    if (r == 0) {
        liste leer = {0};
        r = liste_schreiben(&leer);
    }
    if (r == 0) g_liste_defekt_gemeldet = 0;
    pthread_mutex_unlock(&g_liste_mtx);
    if (r != 0) {
        zeile("Geraeteliste liess sich nicht zuruecksetzen (%s)", strerror(errno));
        return -1;
    }
    const char *kurz = strrchr(alt, '/');
    zeile("Geraeteliste zurueckgesetzt - die beschaedigte liegt als %s daneben", kurz ? kurz + 1 : alt);
    qc_ui_zustand_geaendert();
    return 0;
}

// ---------------------------------------------------------------- Migration

// Eine Zeile aus authorized.txt: "<64 hex>  <Fingerabdruck>  <Name>". Nur der
// Schluessel muss stimmen; Name ist, was hinter dem Fingerabdruck steht
// (frueher meist die IP-Adresse), sonst der Fingerabdruck.
static void alte_zeile(const char *z, size_t n, liste *l, const char *datum) {
    if (n && z[n - 1] == '\r') n--;
    if (n < 64 || memchr(z, 0, n)) return;
    for (int k = 0; k < 64; k++) if (!ist_hex((unsigned char)z[k])) return;
    if (n > 64 && z[64] != ' ' && z[64] != '\t') return;
    uint8_t pub[32];
    for (int k = 0; k < 32; k++) pub[k] = (uint8_t)(hex_wert(z[2 * k]) << 4 | hex_wert(z[2 * k + 1]));
    size_t i = 64;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    // Fingerabdruck "XXXX-XXXX-XXXX-XXXX" ueberspringen, wenn er da ist.
    size_t a = i;
    while (i < n && z[i] != ' ' && z[i] != '\t') i++;
    int fp = i - a == 19;
    for (size_t k = 0; fp && k < 19; k++) fp = (k % 5 == 4) ? z[a + k] == '-' : ist_hex((unsigned char)z[a + k]);
    if (!fp) i = a;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    char name[QC_ZUGANG_NAME_MAX + 1];
    if (!qc_zugang_name_saeubern(z + i, n - i, name, QC_ZUGANG_NAME_MAX)) {
        uint8_t d[32];
        sha256_2(pub, 32, NULL, 0, d);
        snprintf(name, sizeof name, "%02X%02X-%02X%02X-%02X%02X-%02X%02X", d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]);
    }
    liste_anhaengen(l, pub, datum, name, strlen(name));
}

int qc_zugang_migrieren(void) {
    char neu[1200], alt[1200], weg[1300];
    if (qc_config_path(QC_DATEI_GERAETE, neu, sizeof neu) || qc_config_path(QC_DATEI_ALT, alt, sizeof alt)) return -1;
    struct stat st;
    pthread_mutex_lock(&g_liste_mtx);
    int ergebnis = 0;
    if (lstat(neu, &st) == 0 || errno != ENOENT) goto ende;     // die neue Liste gibt es (oder etwas an ihrer Stelle)
    char *b;
    size_t n;
    int r = datei_lesen(alt, QC_LISTE_MAX, &b, &n);
    if (r == 1) goto ende;                                      // keine alte Liste: nichts zu tun
    if (r < 0) {
        zeile("Migration: authorized.txt ist nicht lesbar - nichts uebernommen, naechster Start versucht es erneut");
        ergebnis = -1;
        goto ende;
    }
    char datum[11];
    heute(datum);
    liste l = {0};
    for (size_t a = 0; a < n;) {
        char *e = memchr(b + a, '\n', n - a);
        size_t len = e ? (size_t)(e - (b + a)) : n - a;
        alte_zeile(b + a, len, &l, datum);
        a += len + 1;
    }
    free(b);
    snprintf(weg, sizeof weg, "%s.migriert", alt);
    if (liste_schreiben(&l) != 0) {
        zeile("Migration: host-devices.txt liess sich nicht schreiben - authorized.txt bleibt");
        ergebnis = -1;
    } else if (rename(alt, weg) != 0) {
        zeile("Migration: %d Geraet(e) uebernommen, authorized.txt liess sich aber nicht umbenennen (%s)",
              l.n, strerror(errno));
        ergebnis = 1;
    } else {
        zeile("Migration: %d Geraet(e) aus authorized.txt in host-devices.txt uebernommen, "
              "die alte Liste heisst jetzt authorized.txt.migriert", l.n);
        ergebnis = 1;
    }
    liste_frei(&l);
ende:
    pthread_mutex_unlock(&g_liste_mtx);
    return ergebnis;
}

// ---------------------------------------------------------------- Passwort

static pthread_mutex_t g_pw_mtx = PTHREAD_MUTEX_INITIALIZER;
static uint8_t g_host_pub[32];
static _Atomic int g_host_pub_da = 0;
// K fuer das Passwort in g_k_pw; unter g_pw_mtx. Aendert sich das Passwort
// (auch von aussen in der Datei), wird neu gerechnet.
static char g_k_pw[QC_ZUGANG_PW_MAX + 1];
static uint8_t g_k[32];
static int g_k_gueltig = 0;

// Gueltig als Passwort: nicht zu lang, eine Zeile, norm mindestens 8 Byte.
static int pw_gueltig(const char *pw) {
    size_t n = strlen(pw);
    if (n > QC_ZUGANG_PW_MAX || strchr(pw, '\n') || strchr(pw, '\r')) return 0;
    return qc_zugang_norm(pw, NULL, 0) >= QC_ZUGANG_PW_MIN;
}

// Unter g_pw_mtx. 0 = gelesen, 1 = gibt es nicht, -1 = unlesbar oder ungueltig.
static int pw_lesen(char out[QC_ZUGANG_PW_MAX + 1]) {
    out[0] = 0;
    char pfad[1200];
    if (qc_config_path(QC_DATEI_PASSWORT, pfad, sizeof pfad)) return -1;
    char *b;
    size_t n;
    int r = datei_lesen(pfad, QC_ZUGANG_PW_MAX + 2, &b, &n);
    if (r != 0) return r;
    if (n && b[n - 1] == '\n') n--;
    if (n && b[n - 1] == '\r') n--;
    b[n] = 0;
    int ok = !memchr(b, 0, n) && pw_gueltig(b);
    if (ok) memcpy(out, b, n + 1);
    qc_wipe(b, n);
    free(b);
    return ok ? 0 : -1;
}

// Unter g_pw_mtx.
static int pw_schreiben(const char *pw) {
    char pfad[1200], z[QC_ZUGANG_PW_MAX + 2];
    if (qc_config_path(QC_DATEI_PASSWORT, pfad, sizeof pfad)) return -1;
    int n = snprintf(z, sizeof z, "%s\n", pw);
    int r = datei_ersetzen(pfad, z, (size_t)n);
    qc_wipe(z, sizeof z);
    g_k_gueltig = 0;
    qc_wipe(g_k, sizeof g_k);
    return r;
}

// 9 Zeichen aus 31 (ohne i, l, o, 0, 1), gleichverteilt: Bytes ab 248 = 8 * 31
// werden verworfen. Angezeigt und gespeichert als "xxx-xxx-xxx".
static void zufallspasswort(char out[12]) {
    static const char abc[] = "abcdefghjkmnpqrstuvwxyz23456789";
    int k = 0;
    while (k < 9) {
        uint8_t b[16];
        arc4random_buf(b, sizeof b);
        for (size_t i = 0; i < sizeof b && k < 9; i++) {
            if (b[i] >= 248) continue;
            out[k + k / 3] = abc[b[i] % 31];
            k++;
        }
        qc_wipe(b, sizeof b);
    }
    out[3] = out[7] = '-';
    out[11] = 0;
}

// Unter g_pw_mtx: fehlt die Datei, ein Zufallspasswort anlegen.
static int pw_holen(char out[QC_ZUGANG_PW_MAX + 1]) {
    int r = pw_lesen(out);
    if (r != 1) return r;
    char z[12];
    zufallspasswort(z);
    r = pw_schreiben(z);
    if (r == 0) {
        memcpy(out, z, sizeof z);
        zeile("Zugangspasswort neu erzeugt (host-password.txt; es steht im Menue)");
    } else {
        zeile("Zugangspasswort liess sich nicht anlegen - neue Geraete nur ueber Zulassen");
    }
    qc_wipe(z, sizeof z);
    return r == 0 ? 0 : -1;
}

int qc_zugang_passwort(char *puffer, size_t groesse) {
    char pw[QC_ZUGANG_PW_MAX + 1];
    pthread_mutex_lock(&g_pw_mtx);
    int r = pw_holen(pw);
    pthread_mutex_unlock(&g_pw_mtx);
    if (r == 0 && strlen(pw) + 1 > groesse) r = -1;
    if (r == 0) memcpy(puffer, pw, strlen(pw) + 1);
    else if (puffer && groesse) puffer[0] = 0;
    qc_wipe(pw, sizeof pw);
    return r == 0 ? 0 : -1;
}

int qc_zugang_passwort_setzen(const char *pw) {
    if (!pw || !pw_gueltig(pw)) return -1;
    pthread_mutex_lock(&g_pw_mtx);
    int r = pw_schreiben(pw);
    pthread_mutex_unlock(&g_pw_mtx);
    if (r != 0) { zeile("Zugangspasswort liess sich nicht speichern"); return -2; }
    zeile("Zugangspasswort geaendert");
    qc_ui_zustand_geaendert();
    return 0;
}

int qc_zugang_passwort_zufall(void) {
    char z[12];
    zufallspasswort(z);
    pthread_mutex_lock(&g_pw_mtx);
    int r = pw_schreiben(z);
    pthread_mutex_unlock(&g_pw_mtx);
    qc_wipe(z, sizeof z);
    if (r != 0) { zeile("Neues Zufallspasswort liess sich nicht speichern"); return -2; }
    zeile("Zugangspasswort geaendert (neues Zufallspasswort)");
    qc_ui_zustand_geaendert();
    return 0;
}

int qc_zugang_pruefen(const uint8_t hh[32], const uint8_t client_proof[32], uint8_t host_proof[32]) {
    if (!atomic_load(&g_host_pub_da)) return -1;
    char pw[QC_ZUGANG_PW_MAX + 1];
    uint8_t k[32];
    pthread_mutex_lock(&g_pw_mtx);
    int r = pw_holen(pw);
    int gemerkt = r == 0 && g_k_gueltig && strcmp(pw, g_k_pw) == 0;
    if (gemerkt) memcpy(k, g_k, 32);
    pthread_mutex_unlock(&g_pw_mtx);
    if (r != 0) { qc_wipe(pw, sizeof pw); return -1; }
    // PBKDF2 (rund 50 ms) ausserhalb der Sperre: die Oberflaeche liest das
    // Passwort derweil ungehindert.
    if (!gemerkt) {
        if (qc_zugang_schluessel(pw, g_host_pub, k) != 0) { qc_wipe(pw, sizeof pw); return -1; }
        pthread_mutex_lock(&g_pw_mtx);
        memcpy(g_k_pw, pw, sizeof g_k_pw);
        memcpy(g_k, k, 32);
        g_k_gueltig = 1;
        pthread_mutex_unlock(&g_pw_mtx);
    }
    uint8_t soll[32];
    qc_zugang_beweis(k, hh, 0, soll);
    int ok = qc_zugang_gleich(soll, client_proof, 32);
    if (ok) qc_zugang_beweis(k, hh, 1, host_proof);
    qc_wipe(soll, sizeof soll);
    qc_wipe(k, sizeof k);
    qc_wipe(pw, sizeof pw);
    return ok;
}

// ------------------------------------------------------------ Einzelinstanz

int qc_zugang_einzelinstanz(void) {
    static int fd = -1;                 // bleibt offen, solange der Prozess lebt
    static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
    pthread_mutex_lock(&m);
    int r = 1;
    if (fd < 0) {
        char pfad[1200];
        int f = qc_config_path(QC_DATEI_INSTANZ, pfad, sizeof pfad) ? -1
              : open(pfad, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
        if (f < 0) r = -1;
        else if (flock(f, LOCK_EX | LOCK_NB) != 0) {
            r = errno == EWOULDBLOCK ? 0 : -1;
            close(f);
        } else {
            fd = f;
        }
    }
    pthread_mutex_unlock(&m);
    return r;
}

// ------------------------------------------------------------------- Start

uint32_t qc_zugang_eigene_id(void) {
    return atomic_load(&g_host_pub_da) ? qc_zugang_id(g_host_pub) : 0;
}

void qc_zugang_start(const uint8_t host_pub[32]) {
    pthread_mutex_lock(&g_pw_mtx);
    memcpy(g_host_pub, host_pub, 32);
    g_k_gueltig = 0;
    pthread_mutex_unlock(&g_pw_mtx);
    atomic_store(&g_host_pub_da, 1);
    qc_zugang_migrieren();
    char pw[QC_ZUGANG_PW_MAX + 1];
    if (qc_zugang_passwort(pw, sizeof pw) != 0)
        zeile("Zugangspasswort host-password.txt nicht lesbar - neue Geraete nur ueber Zulassen");
    qc_wipe(pw, sizeof pw);
}
