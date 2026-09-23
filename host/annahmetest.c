// Pruefprogramm fuer die Annahme: Frist im Handschlag, Plaetze, Freigabeliste.
//
//   clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/annahmetest.c host/qc_annahme.c \
//         host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/annahmetest
//   /tmp/annahmetest
//
// Alles laeuft ueber 127.0.0.1 auf freien Ports ab 19000 und mit einem eigenen
// HOME in einem frischen Ordner unter $TMPDIR - ein laufender Host, seine Ports
// und seine Freigaben bleiben unberuehrt. Rueckgabe: Zahl der Fehler.

#include "qc_annahme.h"
#include "qc_secure.h"

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
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
static _Atomic int g_erfolg = 0, g_gescheitert = 0, g_abgewiesen = 0;
static int g_fehler = 0;

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
    (void)peer; (void)ctx;
    qc_chan *c = malloc(sizeof *c);
    if (!c) { close(fd); return; }
    int r = qc_chan_accept(c, fd, g_s_priv, (const uint8_t *)PRO, strlen(PRO));
    qc_platz_frei(platz);
    if (r == 0) {
        struct iovec iov = { .iov_base = (void *)"QCH1", .iov_len = 4 };
        if (qc_chan_send(c, &iov, 1) == 0) atomic_fetch_add(&g_erfolg, 1);
    } else {
        atomic_fetch_add(&g_gescheitert, 1);
    }
    qc_chan_free(c);
    close(fd);
}

static void abgewiesen(long anzahl, const struct sockaddr_in *peer, void *ctx) {
    (void)peer; (void)ctx;
    atomic_fetch_add(&g_abgewiesen, (int)anzahl);
}

// Lauscht auf dem ersten freien Port ab start. Gibt den Port zurueck.
static int lauschen(int start, int max_gesamt, int max_je_ip) {
    for (int port = start; port < start + 200; port++) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) return -1;
        struct sockaddr_in a = {0};
        a.sin_family = AF_INET;
        a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        a.sin_port = htons((uint16_t)port);
        if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0 || listen(fd, 4) != 0) { close(fd); continue; }
        qc_annahme_cfg cfg = { max_gesamt, max_je_ip, verbindung, abgewiesen, NULL };
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

// ---------------------------------------------------- Freigabeliste (B8)

static char g_liste[1200];

static void liste_schreiben(const char *inhalt) {
    FILE *f = fopen(g_liste, "w");
    if (f) { fputs(inhalt, f); fclose(f); }
    chmod(g_liste, 0600);
}

static int liste_gleich(const char *inhalt) {
    char buf[4096] = {0};
    FILE *f = fopen(g_liste, "r");
    if (!f) return 0;
    size_t n = fread(buf, 1, sizeof buf - 1, f);
    fclose(f);
    return n == strlen(inhalt) && memcmp(buf, inhalt, n) == 0;
}

static void freigaben_pruefen(void) {
    printf("\n-- Freigabeliste\n");
    uint8_t a[32], b[32];
    memset(a, 0x11, 32);
    memset(b, 0x22, 32);
    char hex_a[65];
    for (int i = 0; i < 32; i++) snprintf(hex_a + i * 2, 3, "%02x", a[i]);
    char zeile_a[200];
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
}

// ----------------------------------------------------------------- Ablauf

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    // Eigenes HOME, bevor irgendetwas die Ablage anfasst.
    const char *tmp = getenv("TMPDIR");
    char home[1024];
    snprintf(home, sizeof home, "%s/qc-annahmetest-XXXXXX", tmp && *tmp ? tmp : "/tmp");
    if (!mkdtemp(home)) { perror("mkdtemp"); return 100; }
    setenv("HOME", home, 1);
    char pfad[1100];
    snprintf(pfad, sizeof pfad, "%s/Library", home); mkdir(pfad, 0700);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support", home); mkdir(pfad, 0700);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support/QuadChroma", home); mkdir(pfad, 0700);
    snprintf(g_liste, sizeof g_liste, "%s/authorized.txt", pfad);
    printf("HOME fuer diesen Test: %s\n", home);

    qc_keypair(g_s_priv, g_s_pub);
    int port = lauschen(19000, 4, 2);
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

    // 3. Mehr als zwei von derselben Adresse: die dritte fliegt sofort.
    printf("\n-- Plaetze je Adresse\n");
    int abg0 = atomic_load(&g_abgewiesen);
    int s1 = stummer_client(port), s2 = stummer_client(port);
    usleep(200 * 1000);
    int s3 = verbinden(port);
    int64_t z3 = warte_auf_ende(s3, 1000);
    pruefe(z3 >= 0 && z3 < 500, "dritte Verbindung derselben Adresse wird sofort geschlossen");
    pruefe(warte_zaehler(&g_abgewiesen, abg0 + 1, 500), "Abweisung wird gemeldet");
    close(s3);
    int gesch0 = atomic_load(&g_gescheitert);
    close(s1); close(s2);
    pruefe(warte_zaehler(&g_gescheitert, gesch0 + 2, 1000), "aufgegebene Handschlaege geben ihren Platz frei");
    pruefe(echter_client(port) == 0, "danach kommt ein echter Client wieder durch");

    // 4. Gesamtgrenze an einem zweiten Port: 2 Plaetze, 8 je Adresse.
    printf("\n-- Plaetze gesamt\n");
    int port2 = lauschen(port + 1, 2, 8);
    int g1 = stummer_client(port2), g2 = stummer_client(port2);
    usleep(200 * 1000);
    int g3 = verbinden(port2);
    int64_t zg = warte_auf_ende(g3, 1000);
    pruefe(zg >= 0 && zg < 500, "Verbindung ueber der Gesamtgrenze wird sofort geschlossen");
    close(g1); close(g2); close(g3);
    usleep(200 * 1000);
    pruefe(echter_client(port2) == 0, "nach dem Freiwerden kommt ein echter Client durch");

    pruefe(atomic_load(&g_erfolg) == 3, "genau drei erfolgreiche Handschlaege auf Hostseite");

    freigaben_pruefen();

    // Eigenes HOME wieder wegraeumen.
    unlink(g_liste);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support/QuadChroma", home); rmdir(pfad);
    snprintf(pfad, sizeof pfad, "%s/Library/Application Support", home); rmdir(pfad);
    snprintf(pfad, sizeof pfad, "%s/Library", home); rmdir(pfad);
    rmdir(home);

    printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
    return g_fehler;
}
