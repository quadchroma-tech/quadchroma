// Pruefprogramm fuer die Annahme: Frist im Handschlag, Plaetze und Verdraengen,
// Freigabeliste, eigener Schluessel.
//
//   clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/annahmetest.c host/qc_annahme.c \
//         host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/annahmetest
//   /tmp/annahmetest
//
// Alles laeuft ueber 127.0.0.1 auf freien Ports ab 19000 und mit einem eigenen
// HOME in einem frischen Ordner unter $TMPDIR - ein laufender Host, seine Ports
// und seine Freigaben bleiben unberuehrt. Dauer rund 25 s (die Andrang-Meldung
// kommt hoechstens alle zehn Sekunden). Rueckgabe: Zahl der Fehler.

#include "qc_annahme.h"
#include "qc_secure.h"

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static const char *PRO = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
static uint8_t g_s_priv[32], g_s_pub[32];
static int g_fehler = 0;

// Zaehler je Port: was die Hostseite erlebt und was die Annahme meldet.
typedef struct {
    _Atomic int erfolg, gescheitert, verdraengt;        // in den Verbindungsfaeden
    _Atomic int meld_verdraengt, meld_abgewiesen, meldungen;
} zaehler;

static int64_t jetzt_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

// ------------------------------------------------------------ Hostseite

// Wie im Host: Handschlag, Platz zurueck, dann arbeiten. Zum Beweis, dass
// der Kanal steht, geht die Kennung verschluesselt zurueck.
static void verbindung(qc_platz *platz, int fd, const struct sockaddr_in *peer, void *ctx) {
    (void)peer;
    zaehler *z = ctx;
    qc_chan *c = malloc(sizeof *c);
    if (!c) { qc_platz_frei(platz); close(fd); return; }
    int r = qc_chan_accept(c, fd, g_s_priv, (const uint8_t *)PRO, strlen(PRO));
    if (qc_platz_frei(platz)) {
        atomic_fetch_add(&z->verdraengt, 1);
    } else if (r == 0) {
        struct iovec iov = { .iov_base = (void *)"QCH1", .iov_len = 4 };
        if (qc_chan_send(c, &iov, 1) == 0) atomic_fetch_add(&z->erfolg, 1);
    } else {
        atomic_fetch_add(&z->gescheitert, 1);
    }
    qc_chan_free(c);
    close(fd);
}

// Ein Faden, der nach dem Verdraengen noch eine Weile lebt - so haeufen sich
// Faeden an, und die Obergrenze muss greifen.
static void zaeh(qc_platz *platz, int fd, const struct sockaddr_in *peer, void *ctx) {
    (void)peer;
    zaehler *z = ctx;
    struct pollfd p = { .fd = fd, .events = POLLIN, .revents = 0 };
    poll(&p, 1, 3000);
    if (qc_platz_frei(platz)) {
        atomic_fetch_add(&z->verdraengt, 1);
        usleep(1500 * 1000);
    }
    close(fd);
}

static void andrang(long verdraengt, const struct sockaddr_in *vv, long abgewiesen,
                    const struct sockaddr_in *av, void *ctx) {
    (void)vv; (void)av;
    zaehler *z = ctx;
    atomic_fetch_add(&z->meld_verdraengt, (int)verdraengt);
    atomic_fetch_add(&z->meld_abgewiesen, (int)abgewiesen);
    atomic_fetch_add(&z->meldungen, 1);
}

// Lauscht auf dem ersten freien Port ab start. Gibt den Port zurueck.
static int lauschen(int start, int max_gesamt, int max_je_ip, qc_verbindung_fn fn, zaehler *z) {
    for (int port = start; port < start + 200; port++) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) return -1;
        struct sockaddr_in a = {0};
        a.sin_family = AF_INET;
        a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        a.sin_port = htons((uint16_t)port);
        if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0 || listen(fd, 32) != 0) { close(fd); continue; }
        qc_annahme_cfg cfg = { max_gesamt, max_je_ip, fn, andrang, z };
        if (qc_annahme_starten(fd, &cfg) != 0) { close(fd); return -1; }
        return port;
    }
    return -1;
}

// ---------------------------------------------------------- Clientseite

static int verbinden(int port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) { close(fd); return -1; }
    return fd;
}

// Wartet, bis die Gegenseite schliesst. Gibt die Wartezeit in ms zurueck,
// -1 wenn bis max_ms nichts geschah.
static int64_t warte_auf_ende(int fd, int max_ms) {
    int64_t t0 = jetzt_ms();
    for (;;) {
        int64_t rest = max_ms - (jetzt_ms() - t0);
        if (rest <= 0) return -1;
        struct pollfd p = { .fd = fd, .events = POLLIN, .revents = 0 };
        int r = poll(&p, 1, (int)rest);
        if (r <= 0) continue;
        uint8_t b[64];
        ssize_t n = recv(fd, b, sizeof b, 0);
        if (n <= 0) return jetzt_ms() - t0;
    }
}

static int lies(int fd, uint8_t *buf, size_t n) {
    while (n) {
        ssize_t r = recv(fd, buf, n, 0);
        if (r <= 0) return -1;
        buf += r; n -= (size_t)r;
    }
    return 0;
}

static int lies_rahmen(int fd, uint8_t *buf, size_t cap, size_t *len) {
    uint8_t l[2];
    if (lies(fd, l, 2)) return -1;
    size_t n = (size_t)l[0] | ((size_t)l[1] << 8);
    if (n > cap || lies(fd, buf, n)) return -1;
    *len = n;
    return 0;
}

static int schreib_rahmen(int fd, const uint8_t *buf, size_t len) {
    uint8_t out[8192 + 2];
    out[0] = (uint8_t)(len & 255); out[1] = (uint8_t)(len >> 8);
    memcpy(out + 2, buf, len);
    return send(fd, out, len + 2, 0) == (ssize_t)(len + 2) ? 0 : -1;
}

// Ein echter Client: Handschlag als Anrufer, dann die Kennung vom Host
// entschluesseln. 0 = alles gut.
static int echter_client(int port) {
    int fd = verbinden(port);
    if (fd < 0) return -1;
    struct timeval tv = { .tv_sec = 3, .tv_usec = 0 };
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    uint8_t priv[32], pub[32];
    qc_keypair(priv, pub);
    qc_handshake hs;
    qc_handshake_init(&hs, 1, priv, (const uint8_t *)PRO, strlen(PRO));
    uint8_t m[8192], p[8192];
    size_t ml = 0, pl = 0;
    int r = -1;
    if (qc_handshake_write(&hs, NULL, 0, m, &ml) || schreib_rahmen(fd, m, ml)) goto ende;
    if (lies_rahmen(fd, m, sizeof m, &ml) || qc_handshake_read(&hs, m, ml, p, &pl)) goto ende;
    if (qc_handshake_write(&hs, NULL, 0, m, &ml) || schreib_rahmen(fd, m, ml)) goto ende;
    if (memcmp(hs.rs, g_s_pub, 32)) goto ende;
    qc_cipher tx, rx;
    qc_handshake_split(&hs, &tx, &rx);
    if (lies_rahmen(fd, m, sizeof m, &ml) || qc_decrypt(&rx, m, ml, p, &pl)) goto ende;
    if (pl == 4 && memcmp(p, "QCH1", 4) == 0) r = 0;
ende:
    close(fd);
    return r;
}

// Stumm: kuendigt eine Nachricht an und schweigt dann.
static int stummer_client(int port) {
    int fd = verbinden(port);
    if (fd < 0) return -1;
    uint8_t l[2] = { 32, 0 };
    send(fd, l, 2, 0);
    return fd;
}

// Wartet bis zu max_ms, bis der Zaehler mindestens soll erreicht.
static int warte_zaehler(_Atomic int *z, int soll, int max_ms) {
    int64_t t0 = jetzt_ms();
    while (atomic_load(z) < soll) {
        if (jetzt_ms() - t0 > max_ms) return 0;
        usleep(10 * 1000);
    }
    return 1;
}

// Angreifer im Takt: alle 50 ms eine neue stumme Verbindung, bis stop.
typedef struct {
    int port;
    _Atomic int stop;
    int geoeffnet;
} angreifer_t;

static void *angreifer(void *arg) {
    angreifer_t *a = arg;
    int fds[64], n = 0;
    while (!atomic_load(&a->stop)) {
        int fd = stummer_client(a->port);
        if (fd >= 0) {
            if (n == 64) { close(fds[0]); memmove(fds, fds + 1, 63 * sizeof fds[0]); n--; }
            fds[n++] = fd;
            __atomic_add_fetch(&a->geoeffnet, 1, __ATOMIC_RELAXED);
        }
        usleep(50 * 1000);
    }
    for (int i = 0; i < n; i++) close(fds[i]);
    return NULL;
}

// ---------------------------------------------------- Freigabeliste (B8)

static char g_ablage[1100], g_liste[1200];

static void datei_schreiben(const char *pfad, const void *inhalt, size_t n) {
    unlink(pfad);
    FILE *f = fopen(pfad, "w");
    if (f) { fwrite(inhalt, 1, n, f); fclose(f); }
    chmod(pfad, 0600);
}

static void liste_schreiben(const char *inhalt) {
    datei_schreiben(g_liste, inhalt, strlen(inhalt));
}

static int datei_gleich(const char *pfad, const void *inhalt, size_t n) {
    static char buf[8192];
    FILE *f = fopen(pfad, "r");
    if (!f) return 0;
    size_t r = fread(buf, 1, sizeof buf, f);
    fclose(f);
    return r == n && memcmp(buf, inhalt, n) == 0;
}

static int liste_gleich(const char *inhalt) {
    return datei_gleich(g_liste, inhalt, strlen(inhalt));
}

static int gibt_es(const char *pfad) {
    struct stat st;
    return lstat(pfad, &st) == 0;
}

static void freigaben_pruefen(void) {
    printf("\n-- Freigabeliste\n");
    uint8_t a[32], b[32], c[32];
    memset(a, 0x11, 32);
    memset(b, 0xab, 32);
    memset(c, 0x33, 32);
    char hex_a[65], hex_b[65], hex_b_gross[65];
    for (int i = 0; i < 32; i++) {
        snprintf(hex_a + i * 2, 3, "%02x", a[i]);
        snprintf(hex_b + i * 2, 3, "%02x", b[i]);
        snprintf(hex_b_gross + i * 2, 3, "%02X", b[i]);
    }
    char zeile_a[200], text[4096];
    snprintf(zeile_a, sizeof zeile_a, "%s  AAAA-AAAA-AAAA-AAAA  alt\n", hex_a);

    unlink(g_liste);
    pruefe(qc_authorized_count() == 0, "keine Liste: 0 Freigaben (Erstkontakt)");
    pruefe(qc_is_authorized(a) == 0, "keine Liste: unbekannt, kein Fehler");
    pruefe(qc_authorize(a, "neu") == 0 && qc_is_authorized(a) == 1 && qc_authorized_count() == 1,
           "erste Freigabe wird gespeichert und gefunden");

    liste_schreiben(zeile_a);
    chmod(g_liste, 0000);
    pruefe(qc_authorized_count() == -1, "Liste ohne Leserecht: count -1, kein Erstkontakt");
    pruefe(qc_is_authorized(a) == -1, "Liste ohne Leserecht: auch Bekannte -1");
    pruefe(qc_is_authorized(b) == -1, "Liste ohne Leserecht: Fremde -1");
    pruefe(qc_authorize(b, "fremd") == -1, "Liste ohne Leserecht: nichts wird angehaengt");
    chmod(g_liste, 0600);
    pruefe(liste_gleich(zeile_a), "Liste danach byte-gleich");

    liste_schreiben(zeile_a);
    chmod(g_liste, 0400);
    pruefe(qc_is_authorized(a) == 1, "nur lesbare Liste: Bekannte werden erkannt");
    pruefe(qc_authorize(b, "fremd") == -1, "nur lesbare Liste: Speichern scheitert mit -1");
    chmod(g_liste, 0600);
    pruefe(liste_gleich(zeile_a), "Liste danach byte-gleich");

    unlink(g_liste);
    mkdir(g_liste, 0700);   // ein Ordner an ihrer Stelle: oeffnen geht, lesen nicht
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(a) == -1, "Ordner statt Liste: Lesefehler -1");
    rmdir(g_liste);

    liste_schreiben(zeile_a);
    pruefe(qc_authorize(b, "zweiter") == 0 && qc_authorized_count() == 2 &&
           qc_is_authorized(a) == 1 && qc_is_authorized(b) == 1,
           "zweite Freigabe wird angehaengt, die erste bleibt");

    // Vorhanden, aber beschaedigt: nie Erstkontakt, nichts wird angehaengt.
    static uint8_t nullen[4096];
    datei_schreiben(g_liste, nullen, sizeof nullen);
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(a) == -1 && qc_is_authorized(c) == -1,
           "4096 Nullbytes: -1, kein Erstkontakt");
    pruefe(qc_authorize(c, "fremd") == -1 && qc_authorize(b, "fremd") == -1 &&
           datei_gleich(g_liste, nullen, sizeof nullen),
           "4096 Nullbytes: niemand wird gekoppelt, Datei unveraendert");

    liste_schreiben("hallo welt\n");
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(c) == -1 && qc_authorize(c, "x") == -1 &&
           liste_gleich("hallo welt\n"), "Muell ohne Freigabe: -1, nichts angehaengt");

    liste_schreiben("# nur ein Kommentar\n\n");
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(c) == -1,
           "nur Kommentar und Leerzeile: -1 (vorhanden, aber ohne Freigabe)");

    snprintf(text, sizeof text, "%skaputt\n", zeile_a);
    liste_schreiben(text);
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(a) == -1,
           "gueltige Zeile plus Muellzeile: -1, auch fuer den Bekannten");

    snprintf(text, sizeof text, "%s%s0  x\n", zeile_a, hex_b);
    liste_schreiben(text);
    pruefe(qc_authorized_count() == -1, "Zeile mit 65 Hexziffern: -1");

    snprintf(text, sizeof text, " %s", zeile_a);
    liste_schreiben(text);
    pruefe(qc_authorized_count() == -1, "Schluessel mit Leerraum davor: -1");

    snprintf(text, sizeof text, "%s%c%s", zeile_a, 0, zeile_a);
    datei_schreiben(g_liste, text, strlen(zeile_a) * 2 + 1);
    pruefe(qc_authorized_count() == -1, "Nullbyte zwischen gueltigen Zeilen: -1");

    datei_schreiben(g_liste, "", 0);
    pruefe(qc_authorized_count() == 0 && qc_is_authorized(a) == 0, "Datei mit 0 Bytes: 0 (Erstkontakt)");
    pruefe(qc_authorize(a, "neu") == 0 && qc_authorized_count() == 1, "Datei mit 0 Bytes: erste Freigabe landet darin");

    // Von Hand gepflegt: Kommentar, Leerzeile, Windows-Zeilenende, Grossbuchstaben.
    snprintf(text, sizeof text, "# Freigaben\n\n%s  AAAA-AAAA-AAAA-AAAA  alt\r\n   \n%s\n", hex_a, hex_b_gross);
    liste_schreiben(text);
    pruefe(qc_authorized_count() == 2 && qc_is_authorized(a) == 1 && qc_is_authorized(b) == 1 &&
           qc_is_authorized(c) == 0, "Kommentar, Leerzeilen, CRLF, Grossbuchstaben: 2 Freigaben");

    // Eine Zeile laenger als jeder feste Puffer.
    int n = snprintf(text, sizeof text, "%s  AAAA-AAAA-AAAA-AAAA  ", hex_a);
    memset(text + n, 'n', 1500);
    text[n + 1500] = '\n';
    text[n + 1501] = 0;
    liste_schreiben(text);
    pruefe(qc_authorized_count() == 1 && qc_is_authorized(a) == 1, "Name mit 1500 Zeichen: 1 Freigabe");

    // Groesser als 1 MiB: keine Freigabeliste mehr.
    unlink(g_liste);
    FILE *f = fopen(g_liste, "w");
    if (f) {
        fputs(zeile_a, f);
        for (int i = 0; i < 20000; i++) fputs("# ............................................................\n", f);
        fclose(f);
    }
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(a) == -1, "Liste ueber 1 MiB: -1");

    // Verweis ins Leere: meldet beim Oeffnen ENOENT, ist aber nicht "keine Liste".
    char ziel[1300];
    snprintf(ziel, sizeof ziel, "%s/ziel-gibt-es-nicht.txt", g_ablage);
    unlink(g_liste);
    symlink(ziel, g_liste);
    pruefe(qc_authorized_count() == -1 && qc_is_authorized(c) == -1, "haengender Verweis: -1, kein Erstkontakt");
    pruefe(qc_authorize(c, "fremd") == -1 && !gibt_es(ziel), "haengender Verweis: nichts gekoppelt, kein Ziel angelegt");
    unlink(g_liste);

    // Letzte Zeile ohne Zeilenende (von Hand bearbeitet): die neue Freigabe
    // muss in eine eigene Zeile, sonst waere sie nie wieder zu finden.
    size_t la = strlen(zeile_a);
    datei_schreiben(g_liste, zeile_a, la - 1);
    pruefe(qc_authorized_count() == 1 && qc_is_authorized(a) == 1, "ohne Zeilenende: die Freigabe gilt");
    pruefe(qc_authorize(b, "neu") == 0 && qc_is_authorized(b) == 1 && qc_is_authorized(a) == 1 &&
           qc_authorized_count() == 2, "ohne Zeilenende: neue Freigabe wird gefunden, die alte bleibt");
    snprintf(text, sizeof text, "%.*s\n%s  ", (int)(la - 1), zeile_a, hex_b);
    char buf[512] = {0};
    f = fopen(g_liste, "r");
    size_t r = f ? fread(buf, 1, sizeof buf - 1, f) : 0;
    if (f) fclose(f);
    pruefe(r > strlen(text) && memcmp(buf, text, strlen(text)) == 0, "ohne Zeilenende: Zeilenwechsel davor ergaenzt");

    // Ein Name mit Zeilenwechsel bleibt eine Zeile.
    unlink(g_liste);
    pruefe(qc_authorize(a, "boese\n0000000000000000000000000000000000000000000000000000000000000000") == 0 &&
           qc_authorized_count() == 1, "Name mit Zeilenwechsel: eine Zeile, eine Freigabe");
    unlink(g_liste);
}

// ------------------------------------------------ eigener Schluessel

static void schluessel_pruefen(void) {
    printf("\n-- eigener Schluessel (host.key)\n");
    char key[1200];
    snprintf(key, sizeof key, "%s/host.key", g_ablage);
    uint8_t p1[32], q1[32], p2[32], q2[32];
    struct stat st;

    unlink(key);
    pruefe(qc_identity_load(p1, q1) == 0 && stat(key, &st) == 0 && st.st_size == 32 &&
           (st.st_mode & 0777) == 0600, "fehlt: neuer Schluessel, 32 Byte, 0600");
    pruefe(qc_identity_load(p2, q2) == 0 && memcmp(q1, q2, 32) == 0 && memcmp(p1, p2, 32) == 0,
           "zweites Laden: derselbe Schluessel");

    datei_schreiben(key, "kurz", 4);
    pruefe(qc_identity_load(p2, q2) == -2 && datei_gleich(key, "kurz", 4), "4 Byte: -2, Datei bleibt");

    uint8_t lang[33];
    memset(lang, 7, sizeof lang);
    datei_schreiben(key, lang, sizeof lang);
    pruefe(qc_identity_load(p2, q2) == -2 && datei_gleich(key, lang, sizeof lang), "33 Byte: -2, Datei bleibt");

    datei_schreiben(key, "", 0);
    pruefe(qc_identity_load(p2, q2) == -2 && stat(key, &st) == 0 && st.st_size == 0, "0 Byte: -2, Datei bleibt");

    datei_schreiben(key, p1, 32);
    chmod(key, 0000);
    int rc = qc_identity_load(p2, q2);
    chmod(key, 0600);
    pruefe(rc == -2 && datei_gleich(key, p1, 32), "ohne Leserecht: -2, Datei bleibt");

    char ziel[1300];
    snprintf(ziel, sizeof ziel, "%s/key-ziel-gibt-es-nicht", g_ablage);
    unlink(key);
    symlink(ziel, key);
    pruefe(qc_identity_load(p2, q2) == -2 && !gibt_es(ziel), "haengender Verweis: -2, kein Ziel angelegt");
    unlink(key);
}

// ----------------------------------------------------------------- Ablauf

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    signal(SIGPIPE, SIG_IGN);

    // Eigenes HOME, bevor irgendetwas die Ablage anfasst.
    const char *tmp = getenv("TMPDIR");
    char home[1024];
    snprintf(home, sizeof home, "%s/qc-annahmetest-XXXXXX", tmp && *tmp ? tmp : "/tmp");
    if (!mkdtemp(home)) { perror("mkdtemp"); return 100; }
    setenv("HOME", home, 1);
    char pfad[1100];
    snprintf(pfad, sizeof pfad, "%s/Library", home); mkdir(pfad, 0700);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support", home); mkdir(pfad, 0700);
    snprintf(g_ablage, sizeof g_ablage, "%s/Library/Application Support/QuadChroma", home); mkdir(g_ablage, 0700);
    snprintf(g_liste, sizeof g_liste, "%s/authorized.txt", g_ablage);
    printf("HOME fuer diesen Test: %s\n", home);

    qc_keypair(g_s_priv, g_s_pub);
    static zaehler za, zb, zc;
    int port = lauschen(19000, 4, 2, verbindung, &za);
    if (port < 0) { printf("kein freier Port ab 19000\n"); return 101; }
    printf("Annahme auf 127.0.0.1:%d (4 Plaetze, 2 je Adresse), Frist %d ms\n", port, QC_HANDSCHLAG_MS);

    // 1. Stumme Gegenstelle haelt niemanden auf und fliegt nach der Frist.
    printf("\n-- stumme Gegenstelle\n");
    int stumm = stummer_client(port);
    int64_t t_stumm = jetzt_ms();
    usleep(200 * 1000);
    int64_t t0 = jetzt_ms();
    int r = echter_client(port);
    int64_t dauer = jetzt_ms() - t0;
    pruefe(r == 0 && dauer < 1000, "echter Client kommt trotz haengender Verbindung durch (< 1 s)");
    printf("         (Handschlag %lld ms)\n", (long long)dauer);
    int64_t zu = warte_auf_ende(stumm, 8000) >= 0 ? jetzt_ms() - t_stumm : -1;
    printf("         (stumme Verbindung nach %lld ms geschlossen)\n", (long long)zu);
    pruefe(zu >= QC_HANDSCHLAG_MS - 300 && zu <= QC_HANDSCHLAG_MS + 500,
           "stumme Verbindung wird nach der Frist geschlossen");
    close(stumm);

    // 2. Troepfeln verlaengert nichts: die Frist gilt fuer den ganzen Handschlag.
    printf("\n-- troepfelnde Gegenstelle\n");
    int tr = verbinden(port);
    uint8_t l[2] = { 200, 0 };
    send(tr, l, 2, 0);
    t0 = jetzt_ms();
    int64_t ende = -1;
    for (int i = 0; i < 20 && ende < 0; i++) {
        uint8_t x = 0x55;
        send(tr, &x, 1, 0);
        struct pollfd pf = { .fd = tr, .events = POLLIN, .revents = 0 };
        if (poll(&pf, 1, 700) > 0) {
            uint8_t b[16];
            if (recv(tr, b, sizeof b, 0) <= 0) ende = jetzt_ms() - t0;
        }
    }
    printf("         (troepfelnde Verbindung nach %lld ms geschlossen)\n", (long long)ende);
    pruefe(ende >= QC_HANDSCHLAG_MS - 300 && ende <= QC_HANDSCHLAG_MS + 500,
           "troepfelnde Verbindung wird nach der Frist geschlossen");
    close(tr);
    usleep(200 * 1000);
    pruefe(atomic_load(&za.verdraengt) == 0 && atomic_load(&za.gescheitert) == 2,
           "bis hier nichts verdraengt, zwei Handschlaege nach der Frist gescheitert");

    // 3. Mehr als zwei von derselben Adresse: die aelteste weicht sofort.
    printf("\n-- Plaetze je Adresse\n");
    int s1 = stummer_client(port);
    usleep(50 * 1000);
    int s2 = stummer_client(port);
    usleep(200 * 1000);
    int s3 = verbinden(port);
    int64_t z1 = warte_auf_ende(s1, 1000);
    pruefe(z1 >= 0 && z1 < 500, "dritte Verbindung derselben Adresse verdraengt die aelteste sofort");
    pruefe(warte_auf_ende(s2, 300) < 0, "die zweite bleibt im Handschlag");
    pruefe(warte_zaehler(&za.meld_verdraengt, 1, 500), "erste Verdraengung wird sofort gemeldet");
    t0 = jetzt_ms();
    r = echter_client(port);
    dauer = jetzt_ms() - t0;
    pruefe(r == 0 && dauer < 1000, "echter Client derselben Adresse kommt trotzdem durch");
    int64_t z2 = warte_auf_ende(s2, 1000);
    pruefe(z2 >= 0 && z2 < 500, "... und hat dafuer die aelteste stumme verdraengt");
    pruefe(warte_zaehler(&za.verdraengt, 2, 1000), "verdraengte Faeden erfahren es aus qc_platz_frei");
    close(s1); close(s2); close(s3);

    // 4. Alle Plaetze belegt (4 Plaetze, 8 je Adresse - das steht fuer einen
    // Angreifer mit mehreren Adressen): der Neue verdraengt den Aeltesten.
    printf("\n-- Plaetze gesamt\n");
    int port2 = lauschen(port + 1, 4, 8, verbindung, &zb);
    int g[4];
    for (int i = 0; i < 4; i++) { g[i] = stummer_client(port2); usleep(30 * 1000); }
    usleep(100 * 1000);
    pruefe(echter_client(port2) == 0, "bei vollen Plaetzen kommt ein echter Client durch");
    int64_t zg = warte_auf_ende(g[0], 1000);
    pruefe(zg >= 0 && zg < 500, "... die aelteste stumme Verbindung wurde dafuer abgebrochen");
    pruefe(warte_auf_ende(g[1], 200) < 0, "... die uebrigen bleiben");

    // Frueher genuegten vier stumme Verbindungen alle fuenf Sekunden. Jetzt
    // schiebt ein Angreifer 20 je Sekunde nach, und trotzdem kommt jeder durch.
    angreifer_t ang = { .port = port2 };
    atomic_store(&ang.stop, 0);
    pthread_t at;
    pthread_create(&at, NULL, angreifer, &ang);
    usleep(300 * 1000);
    // Mindestens 5 echte Clients, und weiter, bis der Angreifer 20 stumme
    // Verbindungen geoeffnet hat - auf langsamen CI-Runnern schafft er in
    // den ersten gut 1 s nur 14 (hoechstens 10 s).
    int durch = 0, versuche = 0;
    int64_t t_ang = jetzt_ms();
    while ((versuche < 5 || __atomic_load_n(&ang.geoeffnet, __ATOMIC_RELAXED) < 20) && jetzt_ms() - t_ang < 10000) {
        if (echter_client(port2) == 0) durch++;
        versuche++;
        usleep(200 * 1000);
    }
    atomic_store(&ang.stop, 1);
    pthread_join(at, NULL);
    printf("         (Angreifer oeffnete %d stumme Verbindungen, %d von %d echten Clients durch)\n",
           ang.geoeffnet, durch, versuche);
    pruefe(durch == versuche && versuche >= 5 && ang.geoeffnet >= 20,
           "Angreifer mit 20 stummen Verbindungen (je 50 ms eine): jeder echte Client kommt durch");
    for (int i = 0; i < 4; i++) close(g[i]);

    // 5. Obergrenze der Faeden: verdraengte Faeden, die noch nicht fertig
    // sind, zaehlen mit (2 Plaetze -> hoechstens 4 Faeden).
    printf("\n-- Obergrenze der Faeden\n");
    int port3 = lauschen(port2 + 1, 2, 2, zaeh, &zc);
    int c[6];
    for (int i = 0; i < 4; i++) { c[i] = verbinden(port3); usleep(60 * 1000); }
    pruefe(warte_zaehler(&zc.verdraengt, 2, 1000), "zwei Verbindungen verdraengt, ihre Faeden leben noch");
    c[4] = verbinden(port3);
    int64_t z5 = warte_auf_ende(c[4], 1000);
    pruefe(z5 >= 0 && z5 < 500, "vier Faeden unterwegs: der Neue wird sofort geschlossen");
    usleep(1800 * 1000);
    c[5] = verbinden(port3);
    pruefe(warte_auf_ende(c[5], 300) < 0, "sind die verdraengten Faeden fertig, wird wieder angenommen");
    for (int i = 0; i < 6; i++) close(c[i]);

    // 6. Meldungen: hoechstens alle zehn Sekunden, der Rest kommt auch
    // dann, wenn niemand mehr anklopft.
    printf("\n-- Andrang-Meldungen\n");
    pruefe(warte_zaehler(&za.meld_verdraengt, 2, 11000) && atomic_load(&za.meldungen) == 2,
           "Bildport: zweite Verdraengung kommt nach der Sperre in einer zweiten Meldung");
    pruefe(warte_zaehler(&zc.meld_abgewiesen, 1, 11000), "sofort geschlossene Verbindung wird gemeldet");
    // Zweiter Port: der Client bei vollen Plaetzen plus alle echten Clients
    // der Angreifer-Probe (mindestens 5, auf langsamen Runnern mehr).
    printf("         (Hostseite: %d + %d erfolgreich, erwartet 2 + %d)\n",
           atomic_load(&za.erfolg), atomic_load(&zb.erfolg), 1 + versuche);
    pruefe(atomic_load(&za.erfolg) == 2 && atomic_load(&zb.erfolg) == 1 + versuche,
           "Hostseite: alle echten Handschlaege erfolgreich");

    freigaben_pruefen();
    schluessel_pruefen();

    // Eigenes HOME wieder wegraeumen.
    unlink(g_liste);
    rmdir(g_ablage);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support", home); rmdir(pfad);
    snprintf(pfad, sizeof pfad, "%s/Library", home); rmdir(pfad);
    rmdir(home);

    printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
    return g_fehler;
}
