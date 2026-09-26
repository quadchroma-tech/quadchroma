// Pruefprogramm fuer zugang.c: Geraete-ID, Namen, Bekanntgabe, Nachrichten
// 20-23, norm, HMAC (RFC 4231), PBKDF2 und Beweise mit den Pruefvektoren der
// Spezifikation (3.4, dieselben wie im Rust-Kern), Vergleich in konstanter
// Zeit, Drossel, Grenzen der Zugangsphasen, Anfragen an die Oberflaeche,
// host-devices.txt (Format, beschaedigte Dateien, Entfernen, Zuruecksetzen),
// Migration aus authorized.txt, host-password.txt, Einzelinstanz.
//
//   clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/zugangtest.c host/zugang.c \
//         host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/zugangtest
//   /tmp/zugangtest
//
// Eigenes HOME in einem frischen Ordner unter $TMPDIR - die Dateien des
// Nutzers und ein laufender Host bleiben unberuehrt. Kein Netz, keine
// Oberflaeche: die Rueckrufe qc_ui_* ersetzt dieses Programm durch eigene,
// die mitschreiben. Dauer rund 2 s (PBKDF2). Rueckgabe: Zahl der Fehler.

#include "zugang.h"
#include "qc_noise.h"
#include "qc_secure.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <dirent.h>
#include <ftw.h>

static int g_fehler = 0;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

static void hex_aus(const char *h, uint8_t *out, size_t n) {
    for (size_t i = 0; i < n; i++) {
        unsigned v = 0;
        sscanf(h + 2 * i, "%2x", &v);
        out[i] = (uint8_t)v;
    }
}

static int hex_gleich(const uint8_t *p, const char *h, size_t n) {
    uint8_t soll[64];
    hex_aus(h, soll, n);
    return memcmp(p, soll, n) == 0;
}

// ------------------------------------------ Rueckrufe an die "Oberflaeche"

static char g_ui[2048];                 // Mitschrift: "a<nr> " fuer Anfrage, "z<nr> " fuer zurueck
static int g_ui_zustand = 0;
static char g_ui_name[64];
static uint32_t g_ui_id, g_ui_code;

void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code) {
    size_t n = strlen(g_ui);
    snprintf(g_ui + n, sizeof g_ui - n, "a%llu ", (unsigned long long)anfrage);
    snprintf(g_ui_name, sizeof g_ui_name, "%s", name);
    g_ui_id = id;
    g_ui_code = code;
}
void qc_ui_anfrage_zurueck(uint64_t anfrage) {
    size_t n = strlen(g_ui);
    snprintf(g_ui + n, sizeof g_ui - n, "z%llu ", (unsigned long long)anfrage);
}
void qc_ui_zustand_geaendert(void) { g_ui_zustand++; }
int qc_ui_vorhanden(void) { return 1; }

// Protokollzeilen dieses Teils: die letzte merken, nichts ausgeben.
static char g_zeilen[8192];
static void protokoll(const char *z) {
    size_t n = strlen(g_zeilen);
    snprintf(g_zeilen + n, sizeof g_zeilen - n, "%s\n", z);
}

// ---------------------------------------------------------------- Dateien

static char g_ablage[1100];

static void pfad(const char *datei, char *out, size_t cap) { snprintf(out, cap, "%s/%s", g_ablage, datei); }

static void datei_schreiben(const char *datei, const void *inhalt, size_t n) {
    char p[1200];
    pfad(datei, p, sizeof p);
    unlink(p);
    FILE *f = fopen(p, "w");
    if (f) { fwrite(inhalt, 1, n, f); fclose(f); }
    chmod(p, 0600);
}

static void text_schreiben(const char *datei, const char *t) { datei_schreiben(datei, t, strlen(t)); }

// Inhalt als Text (hoechstens 64 KiB), "" wenn es die Datei nicht gibt.
static const char *inhalt(const char *datei) {
    static char buf[65536];
    char p[1200];
    pfad(datei, p, sizeof p);
    buf[0] = 0;
    FILE *f = fopen(p, "r");
    if (!f) return buf;
    size_t r = fread(buf, 1, sizeof buf - 1, f);
    buf[r] = 0;
    fclose(f);
    return buf;
}

static int gibt_es(const char *datei) {
    char p[1200];
    struct stat st;
    pfad(datei, p, sizeof p);
    return lstat(p, &st) == 0;
}

static int rechte(const char *datei) {
    char p[1200];
    struct stat st;
    pfad(datei, p, sizeof p);
    return stat(p, &st) == 0 ? (int)(st.st_mode & 0777) : -1;
}

static void weg(const char *datei) {
    char p[1200];
    pfad(datei, p, sizeof p);
    unlink(p);
}

// Wie viele Dateien im Ablageordner mit diesem Anfang?
static int dateien_mit(const char *anfang) {
    DIR *d = opendir(g_ablage);
    if (!d) return -1;
    int n = 0;
    struct dirent *e;
    while ((e = readdir(d))) if (strncmp(e->d_name, anfang, strlen(anfang)) == 0) n++;
    closedir(d);
    return n;
}

static void heute(char out[11]) {
    time_t t = time(NULL);
    struct tm tm;
    localtime_r(&t, &tm);
    snprintf(out, 11, "%04d-%02d-%02d", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
}

static void zu_hex(const uint8_t *p, char out[65]) {
    for (int i = 0; i < 32; i++) snprintf(out + 2 * i, 3, "%02x", p[i]);
}

// ------------------------------------------------------------- Pruefungen

static const char *PUB_HEX = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

static void id_pruefen(void) {
    printf("\n-- Geraete-ID, Vergleichscode\n");
    uint8_t pub[32];
    hex_aus(PUB_HEX, pub, 32);
    char t[12];
    uint32_t id = qc_zugang_id(pub);
    qc_zugang_id_text(id, t);
    pruefe(id == 581729911 && strcmp(t, "581 729 911") == 0, "ID(0x01..0x20) = 581729911, angezeigt \"581 729 911\"");
    qc_zugang_id_text(5, t);
    pruefe(strcmp(t, "000 000 005") == 0, "fuehrende Nullen: 5 -> \"000 000 005\"");
    qc_zugang_id_text(999999999, t);
    pruefe(strcmp(t, "999 999 999") == 0, "groesste ID");

    // Der Code ist derselbe wie qc_sas, nur als Zahl.
    int gleich = 1;
    for (int i = 0; i < 50; i++) {
        uint8_t h[32];
        for (int k = 0; k < 32; k++) h[k] = (uint8_t)(i * 37 + k * 11);
        char sas[8], c[8];
        qc_sas(h, sas);
        qc_zugang_code_text(qc_zugang_code(h), c);
        if (strcmp(sas, c) != 0) gleich = 0;
    }
    pruefe(gleich, "Vergleichscode als Zahl: gleich qc_sas (50 Pruefsummen)");
}

static void namen_pruefen(void) {
    printf("\n-- Namen (UTF-8-sicher kuerzen, saeubern), QCN1, Bekanntgabe\n");
    char n[64];
    // 39 x 'a' + U+00FC (ue, 2 Byte) = 41 Byte: das ue passt nicht mehr ganz hinein.
    char lang[64];
    memset(lang, 'a', 39);
    strcpy(lang + 39, "\xc3\xbc");
    size_t l = qc_zugang_name_saeubern(lang, strlen(lang), n, 40);
    pruefe(l == 39 && strspn(n, "a") == 39, "Kuerzen auf 40 Byte nie mitten in einer UTF-8-Folge (39 + ue -> 39)");
    l = qc_zugang_name_saeubern("Roberts Mac mini", 16, n, 40);
    pruefe(l == 16 && strcmp(n, "Roberts Mac mini") == 0, "gewoehnlicher Name bleibt");
    l = qc_zugang_name_saeubern("a\tb\nc\x7f" "d\xc2\x85" "e", 10, n, 40);
    pruefe(strcmp(n, "abcde") == 0, "Steuerzeichen (C0, DEL, C1) fallen weg");
    l = qc_zugang_name_saeubern("x\xff" "y\xc3", 4, n, 40);
    pruefe(strcmp(n, "x\xef\xbf\xbdy\xef\xbf\xbd") == 0, "ungueltiges UTF-8 wird zu U+FFFD");
    l = qc_zugang_name_saeubern("\xed\xa0\x80\xc0\xaf", 5, n, 40);
    pruefe(strstr(n, "\xed\xa0\x80") == NULL && strstr(n, "\xc0\xaf") == NULL, "Ersatzzeichen-Bereich und ueberlange Folge sind ungueltig");
    l = qc_zugang_name_saeubern("PC\xe2\x80\xae" "gpj.exe\xe2\x81\xa6" "x\xe2\x80\x8f", 19, n, 40);
    pruefe(strcmp(n, "PCgpj.exex") == 0, "Richtungszeichen (RLO, LRI, RLM) fallen weg");
    l = qc_zugang_name_saeubern("  Mac  ", 7, n, 40);
    pruefe(strcmp(n, "Mac") == 0, "Leerzeichen am Rand fallen weg");
    l = qc_zugang_name_saeubern("Gr\xc3\xbc\xc3\x9f" "e", 7, n, 40);
    pruefe(l == 7 && strcmp(n, "Gr\xc3\xbc\xc3\x9f" "e") == 0, "Umlaute bleiben");

    uint8_t q[64];
    size_t ql = qc_zugang_name_kodieren("PC-Buero", q, sizeof q);
    pruefe(ql == 13 && memcmp(q, "QCN1\x08PC-Buero", 13) == 0, "QCN1 kodieren");
    pruefe(qc_zugang_name_lesen(q, ql, n) == 1 && strcmp(n, "PC-Buero") == 0, "QCN1 lesen");
    pruefe(qc_zugang_name_lesen((const uint8_t *)"client", 6, n) == 0 && n[0] == 0, "alter Client (\"client\"): kein Name");
    pruefe(qc_zugang_name_lesen(q, ql - 1, n) == 0, "QCN1 abgeschnitten: kein Name");
    uint8_t z[64];
    memcpy(z, "QCN1", 4);
    z[4] = 41;
    memset(z + 5, 'x', 41);
    pruefe(qc_zugang_name_lesen(z, 46, n) == 0, "QCN1 mit 41 Byte: kein Name");
    z[4] = 0;
    pruefe(qc_zugang_name_lesen(z, 5, n) == 0, "QCN1 mit leerem Namen: kein Name");
    z[4] = 2; z[5] = '\n'; z[6] = 1;
    pruefe(qc_zugang_name_lesen(z, 7, n) == 0, "QCN1 nur aus Steuerzeichen: kein Name");
    memcpy(q + ql, "zukunft", 7);
    pruefe(qc_zugang_name_lesen(q, ql + 7, n) == 1 && strcmp(n, "PC-Buero") == 0, "QCN1: Bytes hinter dem Namen bleiben frei");

    uint8_t p[QC_BEKANNTGABE_MAX];
    size_t pl = qc_zugang_bekanntgabe(p, sizeof p, 9001, "Roberts Mac mini", 581729911, QC_BEKANNTGABE_ZULASSEN);
    static const uint8_t soll[] = { 'Q','C','H','B', 1, 0x29, 0x23, 16,
        'R','o','b','e','r','t','s',' ','M','a','c',' ','m','i','n','i',
        1, 0x77, 0x7e, 0xac, 0x22, 1 };
    pruefe(pl == sizeof soll && memcmp(p, soll, sizeof soll) == 0,
           "Bekanntgabe: \"QCHB\" 1, Port LE, Name, ext 1, ID LE, Flags (Pruefvektor)");
    uint16_t port = 0; uint32_t id = 0; uint8_t fl = 0;
    pruefe(qc_zugang_bekanntgabe_lesen(p, pl, &port, n, &id, &fl) == 1 && port == 9001 && id == 581729911 &&
           fl == 1 && strcmp(n, "Roberts Mac mini") == 0, "Bekanntgabe lesen mit Erweiterung");
    pruefe(qc_zugang_bekanntgabe_lesen(p, 8 + 16, &port, n, &id, &fl) == 0, "altes Paket ohne Erweiterung: 0");
    pruefe(qc_zugang_bekanntgabe_lesen(p, 7, &port, n, &id, &fl) == -1, "zu kurzes Paket: -1");
    char lang2[80];
    memset(lang2, 'x', 70);
    lang2[70] = 0;
    pl = qc_zugang_bekanntgabe(p, sizeof p, 9001, lang2, 1, 0);
    pruefe(pl == 8 + 40 + 6 && pl <= 128, "langer Name: auf 40 Byte gekuerzt, Paket <= 128 Byte");
}

static void nachrichten_pruefen(void) {
    printf("\n-- Nachrichten 20-23\n");
    uint8_t m[QC_ZUGANG_NOETIG_MAX];
    size_t n = qc_zugang_noetig_kodieren(m, sizeof m, 3, 5000, "Mac");
    static const uint8_t soll20[] = { 20, 0, 0, 0, 11, 0, 0, 0, 1, 3, 0, 0, 0x88, 0x13, 0, 0, 'M', 'a', 'c' };
    pruefe(n == sizeof soll20 && memcmp(m, soll20, n) == 0, "20 ZUGANG_NOETIG: Kopf, Fassung 1, Wege, warten_ms LE, Name (Pruefvektor)");
    uint8_t w = 0; uint32_t wm = 0; char name[41];
    pruefe(qc_zugang_noetig_lesen(m + 8, n - 8, &w, &wm, name) == 0 && w == 3 && wm == 5000 && strcmp(name, "Mac") == 0,
           "20 lesen");
    pruefe(qc_zugang_noetig_lesen(m + 8, 7, &w, &wm, name) == -1, "20 abgeschnitten: -1");
    m[8] = 2;
    pruefe(qc_zugang_noetig_lesen(m + 8, n - 8, &w, &wm, name) == -1, "20 mit Fassung 2: -1");

    uint8_t proof[32];
    memset(proof, 0x5a, 32);
    uint8_t b[40];
    pruefe(qc_zugang_beweis_kodieren(b, proof) == 40 && b[0] == 21 && b[4] == 32 && b[5] == 0 &&
           memcmp(b + 8, proof, 32) == 0, "21 ZUGANG_BEWEIS: Laenge 32");

    uint8_t e[QC_ZUGANG_ERGEBNIS_MAX], hp[32];
    n = qc_zugang_ergebnis_kodieren(e, sizeof e, QC_ERGEBNIS_PASSWORT, 1234, proof);
    uint8_t erg = 9;
    pruefe(n == 48 && e[0] == 22 && e[4] == 40 && e[8] == 0 && e[12] == 0 && memcmp(e + 16, proof, 32) == 0,
           "22/0: 40 Byte Nutzlast mit host_proof, warten_ms 0");
    pruefe(qc_zugang_ergebnis_lesen(e + 8, 40, &erg, &wm, hp) == 0 && erg == 0 && memcmp(hp, proof, 32) == 0, "22/0 lesen");
    n = qc_zugang_ergebnis_kodieren(e, sizeof e, QC_ERGEBNIS_FALSCH, 10000, NULL);
    pruefe(n == 16 && e[4] == 8 && e[8] == 2 && e[12] == 0x10 && e[13] == 0x27, "22/2: 8 Byte, warten_ms 10000");
    pruefe(qc_zugang_ergebnis_lesen(e + 8, 8, &erg, &wm, hp) == 0 && erg == 2 && wm == 10000, "22/2 lesen");
    n = qc_zugang_ergebnis_kodieren(e, sizeof e, QC_ERGEBNIS_ZUGELASSEN, 999, NULL);
    pruefe(n == 16 && e[8] == 1 && e[12] == 0, "22/1: warten_ms immer 0");
    pruefe(qc_zugang_ergebnis_kodieren(e, sizeof e, QC_ERGEBNIS_PASSWORT, 0, NULL) == 0, "22/0 ohne host_proof: nicht kodierbar");
    pruefe(qc_zugang_ergebnis_kodieren(e, sizeof e, 5, 0, NULL) == 0, "Ergebnis 5: nicht kodierbar");
    uint8_t x[40] = {0};
    pruefe(qc_zugang_ergebnis_lesen(x, 8, &erg, &wm, hp) == -1, "22/0 ohne host_proof: -1");
    x[0] = 3;
    pruefe(qc_zugang_ergebnis_lesen(x, 40, &erg, &wm, hp) == -1, "22/3 mit 40 Byte: -1");
    x[0] = 7;
    pruefe(qc_zugang_ergebnis_lesen(x, 8, &erg, &wm, hp) == -1, "unbekanntes Ergebnis: -1");
    pruefe(qc_zugang_abbruch_kodieren(x) == 8 && x[0] == 23 && x[4] == 0, "23 ZUGANG_ABBRUCH: Laenge 0");
}

static void krypto_pruefen(void) {
    printf("\n-- norm, HMAC, PBKDF2, Beweise (Pruefvektoren 3.4)\n");
    uint8_t out[64];
    size_t n = qc_zugang_norm("K7M 4WQ-9TZ", out, sizeof out);
    pruefe(n == 9 && memcmp(out, "k7m4wq9tz", 9) == 0, "norm(\"K7M 4WQ-9TZ\") = \"k7m4wq9tz\"");
    n = qc_zugang_norm("Gr\xc3\xbc\xc3\x9f" "e, Mac!", out, sizeof out);
    pruefe(n == 12 && memcmp(out, "gr\xc3\xbc\xc3\x9f" "e,mac!", 12) == 0, "norm laesst Nicht-ASCII unveraendert");
    n = qc_zugang_norm(" \t\r\n-", out, sizeof out);
    pruefe(n == 0, "norm entfernt Leerraum und Bindestriche");

    // RFC 4231, Testfall 1 und 2.
    uint8_t k1[20], mac[32];
    memset(k1, 0x0b, 20);
    qc_zugang_hmac(k1, 20, "Hi There", 8, mac);
    pruefe(hex_gleich(mac, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7", 32), "HMAC-SHA256 RFC 4231 Testfall 1");
    qc_zugang_hmac("Jefe", 4, "what do ya want for nothing?", 28, mac);
    pruefe(hex_gleich(mac, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843", 32), "HMAC-SHA256 RFC 4231 Testfall 2");

    uint8_t pub[32], k[32], hh[32];
    hex_aus(PUB_HEX, pub, 32);
    memset(hh, 0xaa, 32);
    const char *k_soll = "0d49cfceed95c9e251f41acead7e619c3983ab763249052d022a2efe03149842";
    pruefe(qc_zugang_schluessel("k7m-4wq-9tz", pub, k) == 0 && hex_gleich(k, k_soll, 32), "K(\"k7m-4wq-9tz\")");
    pruefe(qc_zugang_schluessel("K7M 4WQ-9TZ", pub, k) == 0 && hex_gleich(k, k_soll, 32), "K(\"K7M 4WQ-9TZ\") gleich");
    pruefe(qc_zugang_schluessel("Gr\xc3\xbc\xc3\x9f" "e, Mac!", pub, out) == 0 &&
           hex_gleich(out, "bb389a45a9dc1f7217d3e8da30cc95e2912ce4a1a8465ad7dd5d6c0013e74dc0", 32), "K(\"Gruesse, Mac!\") mit Umlauten");
    uint8_t cp[32], hp[32];
    qc_zugang_beweis(k, hh, 0, cp);
    qc_zugang_beweis(k, hh, 1, hp);
    pruefe(hex_gleich(cp, "647398d505263b3af01ba1a04d74aeb467a774588902489bca97c8d3544e3fc0", 32), "client_proof");
    pruefe(hex_gleich(hp, "2f9d511fa97017f1d5b60806aa5ff4d781766e386b92f79d77391e4090cef55d", 32), "host_proof");

    uint8_t a[32], b[32];
    memset(a, 7, 32);
    memcpy(b, a, 32);
    int gut = qc_zugang_gleich(a, b, 32);
    b[31] ^= 1;
    int schlecht1 = qc_zugang_gleich(a, b, 32);
    b[31] ^= 1; b[0] ^= 0x80;
    int schlecht2 = qc_zugang_gleich(a, b, 32);
    pruefe(gut && !schlecht1 && !schlecht2 && qc_zugang_gleich(a, b, 0), "Vergleich in konstanter Zeit: gleich/ungleich vorn und hinten");
}

static void drossel_pruefen(void) {
    printf("\n-- Drossel\n");
    qc_zugang_drossel_leeren();
    uint8_t s1[32], s2[32];
    memset(s1, 1, 32);
    memset(s2, 2, 32);
    uint32_t ip1 = 0x0100007f, ip2 = 0x0200a8c0;
    int64_t t = 1000000;
    uint32_t w1 = qc_zugang_drossel_fehler(ip1, s1, t);
    uint32_t w2 = qc_zugang_drossel_fehler(ip1, s1, t);
    uint32_t w3 = qc_zugang_drossel_fehler(ip1, s1, t);
    uint32_t w4 = qc_zugang_drossel_fehler(ip1, s1, t);
    uint32_t w5 = qc_zugang_drossel_fehler(ip1, s1, t);
    printf("         (Fehlversuch 1-5: %u %u %u %u %u ms)\n", w1, w2, w3, w4, w5);
    pruefe(w1 == 0 && w2 == 0 && w3 == 5000 && w4 == 10000 && w5 == 20000,
           "Fehlversuch 1-2: 0 s, der dritte 5 s, dann verdoppelt");
    pruefe(qc_zugang_drossel_warten(ip1, s1, t + 15000) == 5000, "die Wartezeit laeuft ab dem letzten Fehlversuch");
    pruefe(qc_zugang_drossel_warten(ip1, s1, t + 20000) == 0, "nach Ablauf: 0");
    pruefe(qc_zugang_drossel_warten(ip1, s2, t) == 20000, "je Adresse: ein anderer Schluessel von derselben Adresse wartet mit");
    pruefe(qc_zugang_drossel_warten(ip2, s1, t) == 20000, "je Schluessel: derselbe Schluessel von einer anderen Adresse wartet mit");
    pruefe(qc_zugang_drossel_warten(ip2, s2, t) == 0, "andere Adresse, anderer Schluessel: frei");
    pruefe(qc_zugang_drossel_rest(ip1, s2, t) == 20000 && qc_zugang_drossel_rest(ip2, s1, t + 5000) == 15000 &&
           qc_zugang_drossel_rest(ip2, s2, t) == 0,
           "Rest beim Beweis: je Adresse und je Schluessel wie beim Beginn der Phase");
    uint32_t w = 0;
    for (int i = 0; i < 20; i++) w = qc_zugang_drossel_fehler(ip1, s1, t);
    pruefe(w == 300000, "hoechstens 300 s");
    qc_zugang_drossel_fehler(ip2, s2, t);
    qc_zugang_drossel_fehler(ip2, s2, t);
    qc_zugang_drossel_fehler(ip2, s2, t);
    pruefe(qc_zugang_drossel_warten(ip2, s2, t) == 5000, "zweite Gegenstelle zaehlt fuer sich");
    qc_zugang_drossel_erfolg(ip1, s1);
    pruefe(qc_zugang_drossel_warten(ip1, s1, t) == 0 && qc_zugang_drossel_fehler(ip1, s1, t) == 0,
           "richtiger Beweis setzt Adresse und Schluessel zurueck");
    int64_t spaeter = t + QC_DROSSEL_VERFALL_MS;
    pruefe(qc_zugang_drossel_warten(ip2, s2, spaeter) == 0 && qc_zugang_drossel_fehler(ip2, s2, spaeter) == 0,
           "15 min nach dem letzten Fehlversuch verfallen die Eintraege");

    // Global: mehr als 30 Fehlversuche in 60 s, von lauter verschiedenen Gegenstellen.
    qc_zugang_drossel_leeren();
    uint8_t s[32];
    memset(s, 0, 32);
    int64_t g = 5000000;
    uint32_t vor = 0;
    for (int i = 0; i < 30; i++) {
        s[0] = (uint8_t)i;
        vor = qc_zugang_drossel_fehler(0x0a000000u + (uint32_t)i, s, g + i * 1000);
    }
    s[0] = 200;
    uint32_t frisch30 = qc_zugang_drossel_warten(0x0b000000u, s, g + 30000);
    s[0] = 30;
    uint32_t nach = qc_zugang_drossel_fehler(0x0a00001eu, s, g + 30000);
    s[0] = 201;
    uint32_t frisch31 = qc_zugang_drossel_warten(0x0b000001u, s, g + 30000);
    uint32_t rest31 = qc_zugang_drossel_rest(0x0b000001u, s, g + 30000);
    uint32_t spaet = qc_zugang_drossel_warten(0x0b000001u, s, g + 60001);
    printf("         (30 Fehlversuche: %u / neue Gegenstelle %u ms; 31.: %u / neue %u ms; eine Minute spaeter %u ms)\n",
           vor, frisch30, nach, frisch31, spaet);
    pruefe(vor == 0 && frisch30 == 0 && nach == 60000 && frisch31 == 60000,
           "mehr als 30 Fehlversuche in 60 s: jede weitere Phase bekommt 60 s");
    pruefe(spaet == 0, "... bis die Minute vorbei ist");
    pruefe(rest31 == 0, "die globale Grenze gilt fuer den Beginn einer Phase, nicht fuer den Rest beim Beweis");
    qc_zugang_drossel_leeren();

    // Viele Gegenstellen: die Tabelle ist begrenzt, der aelteste Eintrag weicht.
    // X wartet nach 12 Fehlversuchen 300 s; dann 130 andere, je 2 s
    // auseinander (so greift die globale Grenze nicht).
    uint8_t x[32];
    memset(x, 0x58, 32);
    for (int i = 0; i < 12; i++) qc_zugang_drossel_fehler(0x58585858u, x, g);
    uint32_t x_vorher = qc_zugang_drossel_warten(0x58585858u, x, g + 1000);
    for (int i = 0; i < 130; i++) {
        s[0] = (uint8_t)i; s[1] = 0x77;
        qc_zugang_drossel_fehler(0x0c000000u + (uint32_t)i, s, g + 2000 + i * 2000);
    }
    int64_t ende = g + 2000 + 130 * 2000;
    uint32_t x_nachher = qc_zugang_drossel_warten(0x58585858u, x, ende);
    s[0] = 129;
    uint32_t neu1 = qc_zugang_drossel_fehler(0x0c000000u + 129, s, ende);
    uint32_t neu2 = qc_zugang_drossel_fehler(0x0c000000u + 129, s, ende + 2000);
    printf("         (X vorher %u ms, nach 130 anderen %u ms; der juengste zaehlt weiter: %u, %u ms)\n",
           x_vorher, x_nachher, neu1, neu2);
    pruefe(x_vorher == 299000 && x_nachher == 0 && neu1 == 0 && neu2 == 5000,
           "Tabelle begrenzt (128 je Art): der aelteste Eintrag weicht, die juengsten zaehlen weiter");
    qc_zugang_drossel_leeren();
}

static void phasen_pruefen(void) {
    printf("\n-- Grenzen der Zugangsphasen\n");
    uint8_t a[32], b[32], c[32], d[32], e[32];
    memset(a, 0xa, 32); memset(b, 0xb, 32); memset(c, 0xc, 32); memset(d, 0xd, 32); memset(e, 0xe, 32);
    int p1 = qc_zugang_phase_beginnen(1, a);
    int p2 = qc_zugang_phase_beginnen(1, b);
    int p3 = qc_zugang_phase_beginnen(1, c);
    pruefe(p1 >= 0 && p2 >= 0 && p3 == -1, "hoechstens 2 je Adresse");
    int p4 = qc_zugang_phase_beginnen(2, a);
    pruefe(p4 == -1, "hoechstens 1 je Schluessel (auch von einer anderen Adresse)");
    int p5 = qc_zugang_phase_beginnen(2, c);
    int p6 = qc_zugang_phase_beginnen(3, d);
    int p7 = qc_zugang_phase_beginnen(4, e);
    pruefe(p5 >= 0 && p6 >= 0 && p7 == -1 && qc_zugang_phasen_offen() == 4, "hoechstens 4 gleichzeitig");
    qc_zugang_phase_ende(p1);
    p7 = qc_zugang_phase_beginnen(4, e);
    pruefe(p7 >= 0, "nach dem Ende einer Phase ist wieder Platz");
    qc_zugang_phase_ende(p2); qc_zugang_phase_ende(p5); qc_zugang_phase_ende(p6); qc_zugang_phase_ende(p7);
    qc_zugang_phase_ende(-1); qc_zugang_phase_ende(99);
    pruefe(qc_zugang_phasen_offen() == 0, "alle wieder frei, falsche Platznummern harmlos");
}

static void anfragen_pruefen(void) {
    printf("\n-- Anfragen an die Oberflaeche\n");
    int pf1[2], pf2[2];
    pipe(pf1);
    pipe(pf2);
    fcntl(pf1[1], F_SETFL, O_NONBLOCK);
    fcntl(pf2[1], F_SETFL, O_NONBLOCK);
    g_ui[0] = 0;
    uint64_t a1 = qc_zugang_anfrage_stellen("Laptop", 111222333, 628306, pf1[1]);
    uint64_t a2 = qc_zugang_anfrage_stellen("PC", 444555666, 1, pf2[1]);
    char soll[64];
    snprintf(soll, sizeof soll, "a%llu ", (unsigned long long)a1);
    pruefe(a1 && a2 && strcmp(g_ui, soll) == 0 && strcmp(g_ui_name, "Laptop") == 0 && g_ui_id == 111222333 &&
           g_ui_code == 628306, "hoechstens eine Anfrage zugleich: nur die erste wird gezeigt, mit Name, ID und Code");
    pruefe(qc_zugang_anfrage_stand(a1) == -1 && qc_zugang_anfrage_stand(a2) == -1, "beide offen");
    qc_zugang_anfrage_zurueckziehen(a1);
    snprintf(soll, sizeof soll, "a%llu z%llu a%llu ", (unsigned long long)a1, (unsigned long long)a1, (unsigned long long)a2);
    pruefe(strcmp(g_ui, soll) == 0 && qc_zugang_anfrage_stand(a1) == -2,
           "zieht sich die gezeigte zurueck: Fenster zu, dann die naechste");
    uint64_t a3 = qc_zugang_anfrage_stellen("Tablet", 7, 8, -1);
    qc_zugang_entscheiden(a2, 1);
    uint8_t byte = 0;
    int geweckt = read(pf2[0], &byte, 1) == 1;
    snprintf(soll, sizeof soll, "a%llu z%llu a%llu z%llu a%llu ", (unsigned long long)a1, (unsigned long long)a1,
             (unsigned long long)a2, (unsigned long long)a2, (unsigned long long)a3);
    pruefe(qc_zugang_anfrage_stand(a2) == 1 && geweckt && strcmp(g_ui, soll) == 0,
           "Zulassen: Stand 1, die Phase wird geweckt, Fenster zu, die naechste kommt");
    size_t vorher = strlen(g_ui);
    qc_zugang_anfrage_zurueckziehen(a2);
    pruefe(strlen(g_ui) == vorher, "eine entschiedene zurueckziehen meldet nichts mehr");
    qc_zugang_entscheiden(a3, 0);
    pruefe(qc_zugang_anfrage_stand(a3) == 0, "Ablehnen: Stand 0");
    qc_zugang_entscheiden(a3, 1);
    pruefe(qc_zugang_anfrage_stand(a3) == 0, "eine zweite Entscheidung aendert nichts");
    qc_zugang_anfrage_zurueckziehen(a3);
    qc_zugang_entscheiden(a3, 1);
    qc_zugang_entscheiden(12345, 1);
    pruefe(qc_zugang_anfrage_stand(12345) == -2, "unbekannte Anfragen: harmlos");
    close(pf1[0]); close(pf1[1]); close(pf2[0]); close(pf2[1]);
}

static void liste_pruefen(void) {
    printf("\n-- Geraeteliste host-devices.txt\n");
    uint8_t a[32], b[32], c[32];
    memset(a, 0x11, 32);
    memset(b, 0xab, 32);
    memset(c, 0x33, 32);
    char ha[65], hb[65], datum[11], z[4096];
    zu_hex(a, ha);
    zu_hex(b, hb);
    heute(datum);
    char name[41];

    weg("host-devices.txt");
    pruefe(qc_zugang_bekannt(a, name) == 0 && qc_zugang_geraete(NULL, 0) == 0, "keine Liste: niemand bekannt, 0 Geraete, kein Fehler");
    int zust = g_ui_zustand;
    pruefe(qc_zugang_eintragen(a, "Laptop") == 0 && qc_zugang_bekannt(a, name) == 1 && strcmp(name, "Laptop") == 0,
           "Eintragen, danach bekannt mit Namen");
    pruefe(g_ui_zustand == zust + 1, "Eintragen meldet der Oberflaeche einen neuen Zustand");
    snprintf(z, sizeof z, "%s  %s  Laptop\n", ha, datum);
    pruefe(strcmp(inhalt("host-devices.txt"), z) == 0 && rechte("host-devices.txt") == 0600,
           "Zeile \"<64 hex>  <YYYY-MM-DD>  <Name>\", Datei 0600");
    pruefe(qc_zugang_eintragen(a, "anders") == 0 && strcmp(inhalt("host-devices.txt"), z) == 0 && g_ui_zustand == zust + 1,
           "zweites Eintragen desselben Schluessels: nichts aendert sich");
    pruefe(qc_zugang_eintragen(b, "Tisch\nPC\t\x01 mit einem sehr langen Namen, der nicht passt") == 0, "zweites Geraet");
    qc_geraet g[4];
    int n = qc_zugang_geraete(g, 4);
    pruefe(n == 2 && memcmp(g[1].pub, b, 32) == 0 && g[1].id == qc_zugang_id(b) && strcmp(g[1].datum, datum) == 0 &&
           strlen(g[1].name) <= 40 && !strchr(g[1].name, '\n') && strncmp(g[1].name, "TischPC mit", 11) == 0,
           "Name ohne Steuerzeichen und auf 40 Byte gekuerzt, ID und Datum in der Liste");
    pruefe(qc_zugang_geraete(g, 1) == 2, "Anzahl kommt auch bei kleinem Puffer ganz");
    pruefe(!gibt_es("host-devices.txt.neu"), "keine Reste des atomaren Schreibens");

    // Von Hand gepflegt: Kommentar, Leerzeile, CRLF, Grossbuchstaben, Tab, doppelter Eintrag, ohne Namen.
    char gross[65];
    for (int i = 0; i < 64; i++) gross[i] = (char)(hb[i] >= 'a' ? hb[i] - 32 : hb[i]);
    gross[64] = 0;
    snprintf(z, sizeof z, "# erlaubt\n\n%s  2026-01-02  alt\r\n   \n%s\t2026-03-04\n%s  2026-05-06  doppelt\n", ha, gross, ha);
    text_schreiben("host-devices.txt", z);
    n = qc_zugang_geraete(g, 4);
    pruefe(n == 2 && strcmp(g[0].name, "alt") == 0 && strcmp(g[0].datum, "2026-01-02") == 0 &&
           memcmp(g[1].pub, b, 32) == 0 && g[1].name[0] == 0 && qc_zugang_bekannt(c, NULL) == 0,
           "Kommentar, Leerzeilen, CRLF, Grossbuchstaben, Tab, ohne Namen, doppelt: 2 Geraete");

    // Beschaedigt: niemand ist bekannt, nichts wird geschrieben.
    struct { const char *was; const char *text; } kaputt[] = {
        { "Muellzeile", "hallo welt\n" },
        { "Schluessel mit 63 Hexziffern", NULL },
        { "Datum fehlt", NULL },
        { "Datum ungueltig", NULL },
        { "Leerraum vor dem Schluessel", NULL },
        { "Schluessel ohne Trenner", NULL },
    };
    char t63[200], tdat[200], tbad[200], tvor[200], tohne[200];
    snprintf(t63, sizeof t63, "%.63s  2026-01-01  x\n", ha);
    snprintf(tdat, sizeof tdat, "%s\n", ha);
    snprintf(tbad, sizeof tbad, "%s  2026-13-01  x\n", ha);
    snprintf(tvor, sizeof tvor, " %s  2026-01-01  x\n", ha);
    snprintf(tohne, sizeof tohne, "%sx 2026-01-01  x\n", ha);
    kaputt[1].text = t63; kaputt[2].text = tdat; kaputt[3].text = tbad; kaputt[4].text = tvor; kaputt[5].text = tohne;
    for (size_t i = 0; i < sizeof kaputt / sizeof kaputt[0]; i++) {
        text_schreiben("host-devices.txt", kaputt[i].text);
        char was[200];
        snprintf(was, sizeof was, "beschaedigt (%s): -1, Eintragen scheitert, Datei unveraendert", kaputt[i].was);
        pruefe(qc_zugang_bekannt(a, NULL) == -1 && qc_zugang_geraete(g, 4) == -1 && qc_zugang_eintragen(c, "x") == -1 &&
               strcmp(inhalt("host-devices.txt"), kaputt[i].text) == 0, was);
    }
    static uint8_t nullen[4096];
    datei_schreiben("host-devices.txt", nullen, sizeof nullen);
    pruefe(qc_zugang_bekannt(a, NULL) == -1 && qc_zugang_eintragen(c, "x") == -1, "4096 Nullbytes: beschaedigt");
    snprintf(z, sizeof z, "%s  2026-01-01  a%cb\n", ha, 0);
    datei_schreiben("host-devices.txt", z, strlen(ha) + 18);
    pruefe(qc_zugang_bekannt(a, NULL) == -1, "Nullbyte in einer Zeile: beschaedigt");
    char p[1200];
    pfad("host-devices.txt", p, sizeof p);
    snprintf(z, sizeof z, "%s  2026-01-01  x\n", ha);
    text_schreiben("host-devices.txt", z);
    chmod(p, 0000);
    int r1 = qc_zugang_bekannt(a, NULL), r2 = qc_zugang_eintragen(c, "x");
    chmod(p, 0600);
    pruefe(r1 == -1 && r2 == -1 && strcmp(inhalt("host-devices.txt"), z) == 0, "ohne Leserecht: beschaedigt, Datei bleibt");
    FILE *f = fopen(p, "w");
    fputs(z, f);
    for (int i = 0; i < 20000; i++) fputs("# ............................................................\n", f);
    fclose(f);
    pruefe(qc_zugang_bekannt(a, NULL) == -1, "ueber 1 MiB: beschaedigt");
    unlink(p);
    mkdir(p, 0700);
    pruefe(qc_zugang_bekannt(a, NULL) == -1 && qc_zugang_eintragen(c, "x") == -1, "Ordner an ihrer Stelle: beschaedigt");
    rmdir(p);
    char ziel[1300];
    pfad("ziel-gibt-es-nicht", ziel, sizeof ziel);
    symlink(ziel, p);
    pruefe(qc_zugang_bekannt(a, NULL) == -1 && qc_zugang_eintragen(c, "x") == -1 && !gibt_es("ziel-gibt-es-nicht"),
           "haengender Verweis: beschaedigt, kein Ziel angelegt");
    unlink(p);

    // Zuruecksetzen: die kaputte bleibt als .defekt-<zeit> liegen.
    text_schreiben("host-devices.txt", "kaputt\n");
    g_zeilen[0] = 0;
    pruefe(qc_zugang_liste_zuruecksetzen() == 0 && qc_zugang_geraete(NULL, 0) == 0 &&
           dateien_mit("host-devices.txt.defekt-") == 1 && strstr(g_zeilen, "zurueckgesetzt"),
           "Zuruecksetzen: neue leere Liste, die alte liegt als .defekt-<zeit> daneben, Zeile");
    pruefe(qc_zugang_eintragen(a, "neu") == 0 && qc_zugang_bekannt(a, NULL) == 1, "danach wird wieder eingetragen");
    pruefe(qc_zugang_liste_zuruecksetzen() == 0 && qc_zugang_bekannt(a, NULL) == 1 && dateien_mit("host-devices.txt.defekt-") == 1,
           "heile Liste: Zuruecksetzen tut nichts");

    // Entfernen.
    qc_zugang_eintragen(b, "Zwei");
    qc_zugang_eintragen(c, "Drei");
    g_zeilen[0] = 0;
    pruefe(qc_zugang_geraet_entfernen(b) == 0 && qc_zugang_bekannt(b, NULL) == 0 && qc_zugang_bekannt(a, NULL) == 1 &&
           qc_zugang_bekannt(c, NULL) == 1 && strstr(g_zeilen, "Geraet entfernt: Zwei (ID "),
           "ein Geraet entfernen: die anderen bleiben, Zeile mit Name und ID");
    pruefe(qc_zugang_geraet_entfernen(b) == 0, "nicht (mehr) eingetragen: 0");
    text_schreiben("host-devices.txt", "kaputt\n");
    pruefe(qc_zugang_geraet_entfernen(a) == -1 && qc_zugang_alle_entfernen() == -1 &&
           strcmp(inhalt("host-devices.txt"), "kaputt\n") == 0, "beschaedigte Liste: Entfernen scheitert, Datei bleibt");
    weg("host-devices.txt");
    qc_zugang_eintragen(a, "A");
    qc_zugang_eintragen(b, "B");
    pruefe(qc_zugang_alle_entfernen() == 0 && qc_zugang_geraete(NULL, 0) == 0 && gibt_es("host-devices.txt") &&
           strcmp(inhalt("host-devices.txt"), "") == 0, "alle entfernen: leere Liste");
}

// Rueckruf fuer entfernte Geraete: mitzaehlen.
static int g_entfernt_n = 0, g_entfernt_null = 0;
static void zugangtest_entfernt(const uint8_t *pub) {
    g_entfernt_n++;
    if (!pub) g_entfernt_null++;
}

static void entfernt_rueckruf_pruefen(void) {
    printf("\n-- Rueckruf beim Entfernen\n");
    uint8_t a[32];
    memset(a, 0x42, 32);
    weg("host-devices.txt");
    qc_zugang_eintragen(a, "A");
    g_entfernt_n = g_entfernt_null = 0;
    qc_zugang_entfernt_setzen(zugangtest_entfernt);
    qc_zugang_geraet_entfernen(a);
    qc_zugang_alle_entfernen();
    qc_zugang_entfernt_setzen(NULL);
    pruefe(g_entfernt_n == 2 && g_entfernt_null == 1, "main.m erfaehrt jedes Entfernen (einzeln: Schluessel, alle: NULL)");
}

static void migration_pruefen(void) {
    printf("\n-- Migration aus authorized.txt\n");
    uint8_t a[32], b[32], c[32];
    memset(a, 0x11, 32);
    memset(b, 0xab, 32);
    memset(c, 0x33, 32);
    char ha[65], hb[65], hc[65], datum[11], z[4096], soll[4096];
    zu_hex(a, ha); zu_hex(b, hb); zu_hex(c, hc);
    heute(datum);

    weg("host-devices.txt");
    weg("authorized.txt.migriert");
    snprintf(z, sizeof z,
             "# Freigaben\n%s  AAAA-AAAA-AAAA-AAAA  192.168.178.20\n%s  BBBB-BBBB-BBBB-BBBB  \n"
             "Muell\n%s\n%s  CCCC-CCCC-CCCC-CCCC  doppelt\n", ha, hb, hc, ha);
    text_schreiben("authorized.txt", z);
    g_zeilen[0] = 0;
    int r = qc_zugang_migrieren();
    char fp_b[24], fp_c[24];
    qc_fingerprint(b, fp_b);
    qc_fingerprint(c, fp_c);
    snprintf(soll, sizeof soll, "%s  %s  192.168.178.20\n%s  %s  %s\n%s  %s  %s\n", ha, datum, hb, datum, fp_b, hc, datum, fp_c);
    printf("         (host-devices.txt:\n%s)\n", inhalt("host-devices.txt"));
    pruefe(r == 1 && strcmp(inhalt("host-devices.txt"), soll) == 0,
           "gueltige Eintraege uebernommen: Datum heute, Name = alter Name (IP), sonst Fingerabdruck; Muell und Doppeltes fallen weg");
    pruefe(!gibt_es("authorized.txt") && gibt_es("authorized.txt.migriert") && strcmp(inhalt("authorized.txt.migriert"), z) == 0,
           "authorized.txt heisst danach authorized.txt.migriert, unveraendert");
    pruefe(strstr(g_zeilen, "Migration: 3 Geraet(e)") != NULL, "Zeile mit der Zahl");
    pruefe(qc_zugang_bekannt(a, NULL) == 1 && qc_zugang_bekannt(c, NULL) == 1 && rechte("host-devices.txt") == 0600,
           "die uebernommenen Geraete sind bekannt, Datei 0600");

    text_schreiben("authorized.txt", z);
    pruefe(qc_zugang_migrieren() == 0 && gibt_es("authorized.txt"), "host-devices.txt gibt es schon: nichts zu tun, authorized.txt bleibt");
    weg("authorized.txt");
    weg("host-devices.txt");
    pruefe(qc_zugang_migrieren() == 0 && !gibt_es("host-devices.txt"), "keine alte Liste: nichts, keine neue angelegt");
    text_schreiben("authorized.txt", "");
    pruefe(qc_zugang_migrieren() == 1 && gibt_es("host-devices.txt") && qc_zugang_geraete(NULL, 0) == 0 &&
           gibt_es("authorized.txt.migriert"), "leere alte Liste: leere neue, kein Erstkontakt");
    weg("host-devices.txt");
    text_schreiben("authorized.txt", z);
    char p[1200];
    pfad("authorized.txt", p, sizeof p);
    chmod(p, 0000);
    int r2 = qc_zugang_migrieren();
    chmod(p, 0600);
    pruefe(r2 == -1 && !gibt_es("host-devices.txt") && gibt_es("authorized.txt"), "alte Liste unlesbar: -1, nichts angelegt, sie bleibt");
    weg("authorized.txt");
    weg("authorized.txt.migriert");
}

static void passwort_pruefen(void) {
    printf("\n-- Passwort host-password.txt\n");
    char pw[QC_ZUGANG_PW_MAX + 1], pw2[QC_ZUGANG_PW_MAX + 1];
    weg("host-password.txt");
    g_zeilen[0] = 0;
    int r = qc_zugang_passwort(pw, sizeof pw);
    int form = strlen(pw) == 11 && pw[3] == '-' && pw[7] == '-';
    for (int i = 0; i < 11 && form; i++)
        if (i != 3 && i != 7 && !strchr("abcdefghjkmnpqrstuvwxyz23456789", pw[i])) form = 0;
    char z[64];
    snprintf(z, sizeof z, "%s\n", pw);
    pruefe(r == 0 && form, "fehlt: Zufallspasswort \"xxx-xxx-xxx\" aus 31 Zeichen (ohne i, l, o, 0, 1)");
    pruefe(strcmp(inhalt("host-password.txt"), z) == 0 && rechte("host-password.txt") == 0600, "gespeichert als eine Zeile, 0600");
    pruefe(strstr(g_zeilen, "neu erzeugt") && !strstr(g_zeilen, pw), "Zeile ohne das Passwort");
    pruefe(qc_zugang_passwort(pw2, sizeof pw2) == 0 && strcmp(pw, pw2) == 0, "zweites Lesen: dasselbe");
    char klein[5];
    pruefe(qc_zugang_passwort(klein, sizeof klein) == -1, "zu kleiner Puffer: -1");

    // Verteilung grob: 200 Passwoerter, jedes Zeichen des Alphabets kommt vor, keine Wiederholung.
    int gesehen[256] = {0}, gleich = 0;
    char vorher[12] = {0};
    for (int i = 0; i < 200; i++) {
        qc_zugang_passwort_zufall();
        qc_zugang_passwort(pw2, sizeof pw2);
        if (strcmp(pw2, vorher) == 0) gleich++;
        memcpy(vorher, pw2, 12);
        for (int k = 0; pw2[k]; k++) gesehen[(uint8_t)pw2[k]]++;
    }
    int alle = 1;
    for (const char *q = "abcdefghjkmnpqrstuvwxyz23456789"; *q; q++) if (!gesehen[(uint8_t)*q]) alle = 0;
    pruefe(alle && !gleich && !gesehen['i'] && !gesehen['l'] && !gesehen['o'] && !gesehen['0'] && !gesehen['1'],
           "Zufallspasswort: alle 31 Zeichen kommen vor, keine verwechselbaren, keine Wiederholung");

    int zust = g_ui_zustand;
    g_zeilen[0] = 0;
    pruefe(qc_zugang_passwort_setzen("abc-def-g") == -1 && qc_zugang_passwort_setzen("ab cd ef g") == -1,
           "norm(pw) unter 8 Byte: -1 (Leerzeichen und Bindestriche zaehlen nicht)");
    pruefe(qc_zugang_passwort_setzen("zwei\nzeilen!") == -1 && qc_zugang_passwort_setzen("") == -1, "Zeilenwechsel oder leer: -1");
    char riesig[400];
    memset(riesig, 'x', 300);
    riesig[300] = 0;
    pruefe(qc_zugang_passwort_setzen(riesig) == -1, "ueber 256 Byte: -1");
    pruefe(qc_zugang_passwort_setzen("Gr\xc3\xbc\xc3\x9f" "e, Mac!") == 0 && qc_zugang_passwort(pw2, sizeof pw2) == 0 &&
           strcmp(pw2, "Gr\xc3\xbc\xc3\x9f" "e, Mac!") == 0, "eigenes Passwort gesetzt und gelesen, wie eingegeben");
    pruefe(strstr(g_zeilen, "Zugangspasswort geaendert") && !strstr(g_zeilen, "Mac!") && g_ui_zustand == zust + 1,
           "Zeile \"geaendert\" ohne das Passwort, Oberflaeche erfaehrt es");

    // Pruefen gegen das Passwort mit den Vektoren: host_pub 0x01..0x20, hh 32 x 0xAA.
    uint8_t pub[32], hh[32], cp[32], hp[32], hp_soll[32];
    hex_aus(PUB_HEX, pub, 32);
    memset(hh, 0xaa, 32);
    qc_zugang_start(pub);
    pruefe(qc_zugang_eigene_id() == 581729911, "eigene ID aus dem Schluessel");
    qc_zugang_passwort_setzen("K7M 4WQ-9TZ");
    hex_aus("647398d505263b3af01ba1a04d74aeb467a774588902489bca97c8d3544e3fc0", cp, 32);
    hex_aus("2f9d511fa97017f1d5b60806aa5ff4d781766e386b92f79d77391e4090cef55d", hp_soll, 32);
    memset(hp, 0, 32);
    pruefe(qc_zugang_pruefen(hh, cp, hp) == 1 && memcmp(hp, hp_soll, 32) == 0, "richtiger Beweis (Vektor): 1, host_proof = Vektor");
    cp[5] ^= 4;
    pruefe(qc_zugang_pruefen(hh, cp, hp) == 0, "verfaelschter Beweis: 0");
    cp[5] ^= 4;
    hh[0] ^= 1;
    pruefe(qc_zugang_pruefen(hh, cp, hp) == 0, "anderer Handschlag (Weiterleitung): 0");
    hh[0] ^= 1;
    qc_zugang_passwort_setzen("etwas-ganz-anderes");
    pruefe(qc_zugang_pruefen(hh, cp, hp) == 0, "nach dem Aendern gilt das alte Passwort nicht mehr (gemerktes K verworfen)");
    // Von aussen geaendert: das gemerkte K darf nicht weiter gelten.
    text_schreiben("host-password.txt", "k7m-4wq-9tz\n");
    pruefe(qc_zugang_pruefen(hh, cp, hp) == 1, "von Hand in die Datei geschrieben: gilt sofort");
    text_schreiben("host-password.txt", "kurz\n");
    pruefe(qc_zugang_passwort(pw2, sizeof pw2) == -1 && qc_zugang_pruefen(hh, cp, hp) == -1 && strcmp(inhalt("host-password.txt"), "kurz\n") == 0,
           "zu kurzes Passwort in der Datei: unlesbar, Pruefen -1, Datei bleibt");
    char p[1200];
    pfad("host-password.txt", p, sizeof p);
    text_schreiben("host-password.txt", "k7m-4wq-9tz\n");
    chmod(p, 0000);
    int r3 = qc_zugang_passwort(pw2, sizeof pw2);
    chmod(p, 0600);
    pruefe(r3 == -1 && strcmp(inhalt("host-password.txt"), "k7m-4wq-9tz\n") == 0, "ohne Leserecht: -1, nichts ueberschrieben");
    pruefe(qc_zugang_passwort_zufall() == 0 && qc_zugang_passwort(pw2, sizeof pw2) == 0 && strlen(pw2) == 11,
           "Neues Zufallspasswort ersetzt eine unlesbare Datei");
    text_schreiben("host-password.txt", "k7m-4wq-9tz\r\n");
    pruefe(qc_zugang_passwort(pw2, sizeof pw2) == 0 && strcmp(pw2, "k7m-4wq-9tz") == 0, "Windows-Zeilenende wird toleriert");
}

static void einzelinstanz_pruefen(void) {
    printf("\n-- Einzelinstanz\n");
    char p[1200];
    pfad("host-instanz.lock", p, sizeof p);
    // Ein anderer "Host" haelt die Sperre (eigene Oeffnung = eigene Sperre).
    int fremd = open(p, O_RDWR | O_CREAT, 0600);
    int gesperrt = fremd >= 0 && flock(fremd, LOCK_EX | LOCK_NB) == 0;
    pruefe(gesperrt && qc_zugang_einzelinstanz() == 0, "laeuft schon ein Host: 0");
    close(fremd);
    pruefe(qc_zugang_einzelinstanz() == 1 && qc_zugang_einzelinstanz() == 1, "frei: 1, und bleibt es fuer diesen Prozess");
    int noch = open(p, O_RDWR);
    pruefe(noch >= 0 && flock(noch, LOCK_EX | LOCK_NB) != 0 && errno == EWOULDBLOCK, "ein zweiter Start bekommt die Sperre nicht");
    close(noch);
}

static int loeschen(const char *p, const struct stat *st, int art, struct FTW *f) {
    (void)st; (void)f;
    return art == FTW_DP ? rmdir(p) : unlink(p);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *tmp = getenv("TMPDIR");
    char home[1024];
    snprintf(home, sizeof home, "%s/qc-zugangtest-XXXXXX", tmp && *tmp ? tmp : "/tmp");
    if (!mkdtemp(home)) { perror("mkdtemp"); return 100; }
    setenv("HOME", home, 1);
    char pf[1100];
    snprintf(pf, sizeof pf, "%s/Library", home); mkdir(pf, 0700);
    snprintf(pf, sizeof pf, "%s/Library/Application Support", home); mkdir(pf, 0700);
    snprintf(g_ablage, sizeof g_ablage, "%s/Library/Application Support/QuadChroma", home); mkdir(g_ablage, 0700);
    printf("HOME fuer diesen Test: %s\n", home);
    qc_zugang_protokoll_setzen(protokoll);

    id_pruefen();
    namen_pruefen();
    nachrichten_pruefen();
    krypto_pruefen();
    drossel_pruefen();
    phasen_pruefen();
    anfragen_pruefen();
    liste_pruefen();
    entfernt_rueckruf_pruefen();
    migration_pruefen();
    passwort_pruefen();
    einzelinstanz_pruefen();

    // Das eigene HOME bleibt nur liegen, wenn etwas fehlschlug (zum Nachsehen).
    if (!g_fehler && nftw(home, loeschen, 16, FTW_DEPTH | FTW_PHYS) != 0) printf("(HOME %s nicht ganz entfernt)\n", home);
    printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
    return g_fehler;
}
