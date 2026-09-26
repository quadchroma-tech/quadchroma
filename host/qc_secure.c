#define __STDC_WANT_LIB_EXT1__ 1      // fuer memset_s
#include "qc_secure.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

// ---------------------------------------------------------------- Loeschen

// memset_s darf der Uebersetzer nicht weglassen, auch wenn der Speicher gleich
// danach frei wird. Gleiche Zusage wie crypto_wipe, aber in memset-Tempo:
// 64 KB in 0,5 statt 37 Mikrosekunden - im Sendeweg zaehlt das.
void qc_wipe(void *p, size_t n) {
    if (p && n) memset_s(p, n, 0, n);
}

// ------------------------------------------------------------- Rohes Lesen
//
// frist ist ein Zeitpunkt auf der monotonen Uhr in Millisekunden, 0 heisst
// ohne Frist. Der Handschlag hat eine und die Zugangsphase danach
// (qc_chan_read_frist), der Transport nicht: ein stiller Bildkanal ist kein
// Fehler.

static int64_t jetzt_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

// Wartet, bis fd lesbar bzw. schreibbar ist - hoechstens bis zur Frist.
static int warten(int fd, short was, int64_t frist) {
    for (;;) {
        int64_t rest = frist - jetzt_ms();
        if (rest <= 0) return -1;
        struct pollfd p = { .fd = fd, .events = was, .revents = 0 };
        int r = poll(&p, 1, rest > 1000000 ? 1000000 : (int)rest);
        if (r > 0) return 0;          // bereit, oder Fehler - das sagt dann recv/send
        if (r == 0) return -1;        // Frist verstrichen
        if (errno != EINTR) return -1;
    }
}

static int raw_read(int fd, void *buf, size_t n, int64_t frist) {
    uint8_t *p = buf;
    while (n) {
        if (frist && warten(fd, POLLIN, frist)) return -1;
        ssize_t r = recv(fd, p, n, frist ? MSG_DONTWAIT : 0);
        if (r == 0) return -1;
        if (r < 0) {
            if (errno == EINTR || (frist && (errno == EAGAIN || errno == EWOULDBLOCK))) continue;
            return -1;
        }
        p += r; n -= (size_t)r;
    }
    return 0;
}

static int raw_write(int fd, const void *buf, size_t n, int64_t frist) {
    const uint8_t *p = buf;
    while (n) {
        if (frist && warten(fd, POLLOUT, frist)) return -1;
        ssize_t w = send(fd, p, n, frist ? MSG_DONTWAIT : 0);
        if (w <= 0) {
            if (w < 0 && (errno == EINTR || (frist && (errno == EAGAIN || errno == EWOULDBLOCK)))) continue;
            return -1;
        }
        p += w; n -= (size_t)w;
    }
    return 0;
}

static int read_frame(int fd, uint8_t *buf, size_t cap, size_t *len, int64_t frist) {
    uint8_t l[2];
    if (raw_read(fd, l, 2, frist)) return -1;
    size_t n = (size_t)l[0] | ((size_t)l[1] << 8);
    if (n > cap) return -1;
    if (raw_read(fd, buf, n, frist)) return -1;
    *len = n;
    return 0;
}

static int write_frame(int fd, const uint8_t *buf, size_t len, int64_t frist) {
    uint8_t l[2] = { (uint8_t)(len & 255), (uint8_t)(len >> 8) };
    if (raw_write(fd, l, 2, frist)) return -1;
    return raw_write(fd, buf, len, frist);
}

// -------------------------------------------------------------- Handschlag

int qc_chan_accept(qc_chan *c, int fd, const uint8_t s_priv[32],
                   const uint8_t *prologue, size_t prologue_len) {
    memset(c, 0, sizeof *c);
    c->fd = fd;

    // Eine feste Frist fuer den ganzen Handschlag, nicht je Lesen: sonst
    // haelt eine Gegenstelle, die alle paar Sekunden ein Byte schickt, die
    // Verbindung beliebig lange fest - ohne je einen Schluessel zu zeigen.
    int64_t frist = jetzt_ms() + QC_HANDSCHLAG_MS;

    qc_handshake hs;
    qc_handshake_init(&hs, 0, s_priv, prologue, prologue_len);

    uint8_t msg[8192], payload[8192];
    size_t mlen = 0, plen = 0;
    int r = 0;

    if (read_frame(fd, msg, sizeof msg, &mlen, frist)) { r = -1; goto ende; }
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) { r = -2; goto ende; }

    if (qc_handshake_write(&hs, NULL, 0, msg, &mlen)) { r = -3; goto ende; }
    if (write_frame(fd, msg, mlen, frist)) { r = -4; goto ende; }

    if (read_frame(fd, msg, sizeof msg, &mlen, frist)) { r = -5; goto ende; }
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) { r = -6; goto ende; }

    // Nutzlast der Nachricht 3: der Name des Clients (zugang.h). Erst jetzt
    // ist sie beglaubigt verschluesselt angekommen.
    c->nutzlast3_len = plen < sizeof c->nutzlast3 ? plen : sizeof c->nutzlast3;
    memcpy(c->nutzlast3, payload, c->nutzlast3_len);
    memcpy(c->hh, qc_handshake_hash(&hs), QC_HASHLEN);
    memcpy(c->peer, hs.rs, 32);
    qc_handshake_split(&hs, &c->tx, &c->rx);
    c->ok = 1;
ende:
    // Der Handschlagzustand enthaelt ck, aus dem beide Sitzungsschluessel
    // folgen, dazu den fluechtigen Schluessel: nichts davon bleibt liegen.
    qc_wipe(&hs, sizeof hs);
    qc_wipe(payload, sizeof payload);
    return r;
}

void qc_chan_free(qc_chan *c) {
    if (!c) return;
    qc_wipe(c, sizeof *c);
    free(c);
}

// ------------------------------------------------------- Senden und Lesen

int qc_chan_send(qc_chan *c, const struct iovec *iov, int cnt) {
    if (!c->ok) return -1;

    // Gesamtlaenge bestimmen und stueckweise verschluesseln. Die Stuecke
    // gehen in einem einzigen Schreibvorgang raus, damit nichts zerfasert.
    size_t total = 0;
    for (int i = 0; i < cnt; i++) total += iov[i].iov_len;
    if (total == 0) return 0;

    // Alles auf den Haufen: diese Funktion laeuft auch im Ton-Rueckruf, und
    // dort ist der Stapelspeicher knapp.
    size_t chunks = (total + QC_CHUNK_MAX - 1) / QC_CHUNK_MAX;
    size_t cap = total + chunks * (QC_TAGLEN + 2);
    uint8_t *out = malloc(cap + QC_CHUNK_MAX);
    if (!out) return -1;
    uint8_t *plain = out + cap;
    // So viel vom Klartextpuffer wird benutzt - und am Ende geloescht, denn
    // durch ihn geht auch die Zwischenablage.
    size_t benutzt = total < QC_CHUNK_MAX ? total : QC_CHUNK_MAX;
    size_t outp = 0;
    int idx = 0;
    size_t off = 0;

    while (total) {
        size_t want = total < QC_CHUNK_MAX ? total : QC_CHUNK_MAX;
        size_t have = 0;
        while (have < want) {
            size_t avail = iov[idx].iov_len - off;
            size_t take = avail < (want - have) ? avail : (want - have);
            memcpy(plain + have, (const uint8_t *)iov[idx].iov_base + off, take);
            have += take; off += take;
            if (off == iov[idx].iov_len) { idx++; off = 0; }
        }
        size_t ct = 0;
        if (qc_encrypt(&c->tx, plain, want, out + outp + 2, &ct)) { qc_wipe(plain, benutzt); free(out); return -1; }
        out[outp] = (uint8_t)(ct & 255);
        out[outp + 1] = (uint8_t)(ct >> 8);
        outp += 2 + ct;
        total -= want;
    }

    int r = raw_write(c->fd, out, outp, 0);
    if (r == 0) c->gesendet += outp;
    qc_wipe(plain, benutzt);
    free(out);
    return r;
}

// frist: Zeitpunkt auf der monotonen Uhr (ms), 0 = ohne.
static int chan_lesen(qc_chan *c, void *buf, size_t n, int64_t frist) {
    if (!c->ok) return -1;
    uint8_t *p = buf;
    while (n) {
        if (c->in_pos == c->in_len) {
            size_t ctlen = 0;
            if (read_frame(c->fd, c->ct, sizeof c->ct, &ctlen, frist)) return -1;
            size_t pt = 0;
            if (qc_decrypt(&c->rx, c->ct, ctlen, c->in, &pt)) return -1;
            c->in_len = pt;
            c->in_pos = 0;
            if (pt == 0) continue;
        }
        size_t take = c->in_len - c->in_pos;
        if (take > n) take = n;
        memcpy(p, c->in + c->in_pos, take);
        c->in_pos += take;
        p += take; n -= take;
    }
    return 0;
}

int qc_chan_read(qc_chan *c, void *buf, size_t n) {
    return chan_lesen(c, buf, n, 0);
}

int qc_chan_read_frist(qc_chan *c, void *buf, size_t n, int frist_ms) {
    // Die Frist gilt fuer das Ganze: ein Datensatz, der tropfenweise kommt,
    // verlaengert sie nicht. Ein halb gelesener Datensatz ist danach verloren -
    // der Kanal wird dann ohnehin geschlossen.
    int r = chan_lesen(c, buf, n, jetzt_ms() + (frist_ms > 0 ? frist_ms : 1));
    if (r != 0) c->ok = 0;
    return r;
}

size_t qc_chan_gepuffert(const qc_chan *c) {
    return c->ok ? c->in_len - c->in_pos : 0;
}

// ------------------------------------------------------------ Schluesselablage

static int config_path(const char *file, char *out, size_t cap) {
    const char *home = getenv("HOME");
    if (!home || !*home) {
        struct passwd *pw = getpwuid(getuid());
        home = pw ? pw->pw_dir : NULL;
    }
    if (!home) return -1;
    char dir[1024];
    snprintf(dir, sizeof dir, "%s/Library/Application Support/QuadChroma", home);
    mkdir(dir, 0700);
    snprintf(out, cap, "%s/%s", dir, file);
    return 0;
}

int qc_config_path(const char *file, char *out, size_t cap) {
    return config_path(file, out, cap);
}

// Gibt es die Datei wirklich nicht? Ein Verweis ins Leere meldet beim Oeffnen
// ebenfalls ENOENT - dann ist sie aber nicht weg, sondern verbogen.
static int fehlt(const char *path, int oeffnen_errno) {
    struct stat st;
    return oeffnen_errno == ENOENT && lstat(path, &st) != 0 && errno == ENOENT;
}

// Nur wenn host.key fehlt, entsteht ein neuer Schluessel. Ist die Datei da,
// aber nicht lesbar oder nicht genau 32 Byte lang, bleibt sie unangetastet:
// ein neuer Schluessel waere eine neue Identitaet, und jeder gekoppelte
// Client wiese den Host danach als fremd ab.
int qc_identity_load(uint8_t priv[32], uint8_t pub[32]) {
    char path[1200];
    if (config_path("host.key", path, sizeof path)) return -1;

    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd >= 0) {
        uint8_t buf[33];                // ein Byte mehr: zu lang ist auch beschaedigt
        ssize_t r = read(fd, buf, sizeof buf);
        close(fd);
        int ok = r == 32;
        if (ok) memcpy(priv, buf, 32);
        qc_wipe(buf, sizeof buf);
        if (!ok) return -2;
        qc_pubkey(priv, pub);
        return 0;
    }
    if (!fehlt(path, errno)) return -2;

    qc_keypair(priv, pub);
    // O_EXCL: nie eine Datei ueberschreiben, die inzwischen doch da ist.
    fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    int ok = write(fd, priv, 32) == 32 && fsync(fd) == 0;
    if (close(fd) != 0) ok = 0;
    if (!ok) { unlink(path); return -1; }
    return 0;
}
