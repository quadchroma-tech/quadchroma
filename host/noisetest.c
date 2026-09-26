// Pruefprogramm fuer die Kryptoschicht.
//
//   noisetest              Selbsttest gegen sich selbst
//   noisetest <port>       wartet auf die Gegenstelle und fuehrt den Handschlag
//
// Beim Netztest gibt jede Seite Pruefsumme, Vergleichscode und Geraete-IDs
// aus, dazu den Namen, den der Client in Nachricht 3 schickt ("QCN1",
// zugang.h). Stimmen die ueberein, sprechen C und Rust nachweislich dieselbe
// Sprache.
//
//   clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/noisetest.c host/qc_noise.c host/zugang.c \
//         host/qc_secure.c host/vendor/monocypher/monocypher.c -o /tmp/noisetest

#include "qc_noise.h"
#include "zugang.h"
#include "monocypher.h"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <time.h>

static int read_full(int fd, void *buf, size_t n) {
    uint8_t *p = buf;
    while (n) {
        ssize_t r = recv(fd, p, n, 0);
        if (r <= 0) return -1;
        p += r; n -= (size_t)r;
    }
    return 0;
}

static int read_msg(int fd, uint8_t *buf, size_t *len) {
    uint8_t l[2];
    if (read_full(fd, l, 2)) return -1;
    size_t n = (size_t)l[0] | ((size_t)l[1] << 8);
    if (n > 65535) return -1;
    if (read_full(fd, buf, n)) return -1;
    *len = n;
    return 0;
}

static int write_msg(int fd, const uint8_t *buf, size_t len) {
    uint8_t l[2] = { (uint8_t)(len & 255), (uint8_t)(len >> 8) };
    if (send(fd, l, 2, 0) != 2) return -1;
    return send(fd, buf, len, 0) == (ssize_t)len ? 0 : -1;
}

// Ein gescheiterter Handschlag laesst kein DH-Ergebnis liegen: Nachricht 3
// mit verfaelschtem Nutzlast-Tag, danach wird in den 16 KB unterhalb des
// Aufrufers - dort lagen eben noch die Rahmen von qc_handshake_read und
// seinen Helfern - nach dem se-Wert gesucht. Den kennt hier nur der Test.
static uint8_t g_se[32];

__attribute__((noinline)) static int stapel_treffer(void) {
    volatile uint8_t marke[1];
    const uint8_t *p = (const uint8_t *)marke - 16384;
    int treffer = 0;
    for (size_t i = 0; i + 32 <= 16384; i++) if (!memcmp(p + i, g_se, 32)) treffer++;
    return treffer;
}

// 0 = wie erwartet abgelehnt, sonst Rueckgabe von qc_handshake_read.
__attribute__((noinline)) static int nachricht3_verfaelscht(int verfaelschen) {
    const char *pro = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
    uint8_t ip[32], ipub[32], rp[32], rpub[32];
    qc_keypair(ip, ipub);
    qc_keypair(rp, rpub);
    qc_handshake ini, res;
    qc_handshake_init(&ini, 1, ip, (const uint8_t *)pro, strlen(pro));
    qc_handshake_init(&res, 0, rp, (const uint8_t *)pro, strlen(pro));
    uint8_t m[512], p[512];
    size_t ml = 0, pl = 0;
    qc_handshake_write(&ini, NULL, 0, m, &ml);
    qc_handshake_read(&res, m, ml, p, &pl);
    qc_handshake_write(&res, NULL, 0, m, &ml);
    qc_handshake_read(&ini, m, ml, p, &pl);
    qc_handshake_write(&ini, (const uint8_t *)"hallo", 5, m, &ml);
    crypto_x25519(g_se, ini.s_priv, res.e_pub);      // se, von der Anruferseite aus
    if (verfaelschen) m[ml - 1] ^= 1;
    int r = qc_handshake_read(&res, m, ml, p, &pl);
    crypto_wipe(&ini, sizeof ini);
    crypto_wipe(&res, sizeof res);
    crypto_wipe(ip, sizeof ip);
    crypto_wipe(rp, sizeof rp);
    return verfaelschen ? (r == -1 ? 0 : 1) : r;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 2) {
        void (*vorher)(uint8_t *, size_t) = qc_random;
        int r = qc_noise_selftest();
        // Der Selbsttest leiht sich den Zufall nur aus (Stelle 99: nicht zurueckgegeben).
        if (r == 0 && qc_random != vorher) r = 99;
        // Stelle 98: nach einer verfaelschten Nachricht 3 lag se noch auf dem
        // Stapel. Die Gegenprobe mit gueltiger Nachricht muss ebenso sauber sein.
        if (r == 0) {
            int ab = nachricht3_verfaelscht(1), t_ab = stapel_treffer();
            int gut = nachricht3_verfaelscht(0), t_gut = stapel_treffer();
            crypto_wipe(g_se, sizeof g_se);
            if (ab != 0 || gut != 0 || t_ab != 0 || t_gut != 0) {
                printf("Fehlerweg: abgelehnt %s, se-Treffer %d (gueltig: %d)\n", ab ? "NEIN" : "ja", t_ab, t_gut);
                r = 98;
            } else {
                printf("Fehlerweg: verfaelschte Nachricht 3 abgelehnt, kein DH-Ergebnis auf dem Stapel\n");
            }
        }
        printf(r == 0 ? "OK\n" : "FEHLER an Stelle %d\n", r);
        return r;
    }

    // "bench": wie viel Durchsatz schafft die Verschluesselung auf diesem Mac?
    // Gemessen wird genau das, was im Betrieb passiert: Stuecke von 64 KB.
    if (strcmp(argv[1], "bench") == 0) {
        qc_cipher c = {0};
        memset(c.k, 0x42, sizeof c.k);
        c.has_key = 1;
        size_t chunk = 65519;
        uint8_t *in = malloc(chunk), *out = malloc(chunk + 32);
        if (!in || !out) return 1;
        memset(in, 0x5a, chunk);
        size_t runden = 4000;
        struct timespec t0, t1;
        clock_gettime(CLOCK_MONOTONIC, &t0);
        for (size_t i = 0; i < runden; i++) {
            size_t n;
            qc_encrypt(&c, in, chunk, out, &n);
        }
        clock_gettime(CLOCK_MONOTONIC, &t1);
        double sek = (t1.tv_sec - t0.tv_sec) + (t1.tv_nsec - t0.tv_nsec) / 1e9;
        double mb = (double)(runden * chunk) / (1024.0 * 1024.0);
        printf("Verschluesselung: %.0f MB in %.2f s = %.0f MB/s = %.1f Gbit/s\n",
               mb, sek, mb / sek, mb / sek * 8.0 / 1024.0);
        return 0;
    }

    int port = atoi(argv[1]);
    uint8_t s_priv[32], s_pub[32];
    qc_keypair(s_priv, s_pub);
    char fp[24];
    qc_fingerprint(s_pub, fp);
    printf("Host-Fingerabdruck: %s\n", fp);
    char id[12];
    qc_zugang_id_text(qc_zugang_id(s_pub), id);
    printf("Geraete-ID des Hosts: %s\n", id);
    printf("oeffentlicher Schluessel (hex): ");
    for (int i = 0; i < 32; i++) printf("%02x", s_pub[i]);
    printf("\n");

    int ls = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(ls, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    a.sin_port = htons((uint16_t)port);
    if (bind(ls, (struct sockaddr *)&a, sizeof a) || listen(ls, 1)) {
        perror("bind");
        return 1;
    }
    printf("warte auf Port %d\n", port);
    int fd = accept(ls, NULL, NULL);
    if (fd < 0) return 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);

    const char *prologue = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
    qc_handshake hs;
    qc_handshake_init(&hs, 0, s_priv, (const uint8_t *)prologue, strlen(prologue));

    uint8_t msg[8192], payload[8192];
    size_t mlen = 0, plen = 0;

    if (read_msg(fd, msg, &mlen)) { printf("Nachricht 1 nicht lesbar\n"); return 2; }
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) { printf("Nachricht 1 abgelehnt\n"); return 3; }
    printf("Nachricht 1 gelesen (%zu Bytes)\n", mlen);

    if (qc_handshake_write(&hs, (const uint8_t *)"host", 4, msg, &mlen)) return 4;
    if (write_msg(fd, msg, mlen)) return 5;
    printf("Nachricht 2 geschrieben (%zu Bytes)\n", mlen);

    if (read_msg(fd, msg, &mlen)) { printf("Nachricht 3 nicht lesbar\n"); return 6; }
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) { printf("Nachricht 3 abgelehnt\n"); return 7; }
    char name[QC_ZUGANG_NAME_MAX + 1];
    if (qc_zugang_name_lesen(payload, plen, name))
        printf("Nachricht 3 gelesen, Name des Clients: %s\n", name);
    else
        printf("Nachricht 3 gelesen, ohne Namen (%zu Byte - aelterer Client)\n", plen);

    char sas[8], cfp[24], cid[12];
    qc_sas(qc_handshake_hash(&hs), sas);
    qc_fingerprint(hs.rs, cfp);
    qc_zugang_id_text(qc_zugang_id(hs.rs), cid);
    printf("Vergleichscode: %s\n", sas);
    printf("Client-Fingerabdruck: %s, Geraete-ID %s\n", cfp, cid);

    qc_cipher send, recv;
    qc_handshake_split(&hs, &send, &recv);

    if (read_msg(fd, msg, &mlen)) return 8;
    uint8_t pt[8192];
    size_t ptlen;
    if (qc_decrypt(&recv, msg, mlen, pt, &ptlen)) { printf("Datensatz nicht entschluesselbar\n"); return 9; }
    pt[ptlen] = 0;
    printf("entschluesselt empfangen: %s\n", pt);

    const char *antwort = "Gruss vom Mac";
    uint8_t ct[8192];
    size_t ctlen;
    qc_encrypt(&send, (const uint8_t *)antwort, strlen(antwort), ct, &ctlen);
    write_msg(fd, ct, ctlen);
    printf("verschluesselt geantwortet\n");

    close(fd);
    close(ls);
    return 0;
}
