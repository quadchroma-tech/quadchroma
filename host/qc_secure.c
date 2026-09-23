#define __STDC_WANT_LIB_EXT1__ 1      // fuer memset_s
#include "qc_secure.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
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
// ohne Frist. Der Handschlag hat eine, der Transport nicht: ein stiller
// Bildkanal ist kein Fehler.

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

int qc_chan_read(qc_chan *c, void *buf, size_t n) {
    if (!c->ok) return -1;
    uint8_t *p = buf;
    while (n) {
        if (c->in_pos == c->in_len) {
            size_t ctlen = 0;
            if (read_frame(c->fd, c->ct, sizeof c->ct, &ctlen, 0)) return -1;
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

// ---------------------------------------------------------- Freigabeliste
//
// Eine Zeile je Gegenstelle: 64 Hexziffern ganz vorn, dann zwei Leerzeichen,
// Fingerabdruck, zwei Leerzeichen, Name. Leerzeilen und Kommentare (#) sind
// erlaubt, sonst nichts.

#define QC_FREIGABEN_MAX (1 << 20)     // groesser ist es keine Freigabeliste mehr

static int ist_hex(int c) {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

// Eine Zeile ohne Zeilenwechsel: 1 = Freigabe, 0 = leer oder Kommentar,
// -1 = etwas anderes - dann ist die Liste beschaedigt.
static int freigabe_zeile(const char *z, size_t n) {
    if (n && z[n - 1] == '\r') n--;     // von Hand unter Windows bearbeitet
    if (memchr(z, 0, n)) return -1;     // Nullbytes: Muell nach einem Absturz
    size_t i = 0;
    while (i < n && (z[i] == ' ' || z[i] == '\t')) i++;
    if (i == n || z[i] == '#') return 0;
    if (n < 64) return -1;
    for (i = 0; i < 64; i++) if (!ist_hex((unsigned char)z[i])) return -1;
    return n == 64 || z[64] == ' ' || z[64] == '\t' ? 1 : -1;
}

// Liest die ganze Liste und prueft jede Zeile. Ergebnis: Zahl der Freigaben,
// mit pub steht in *gefunden, ob er darunter ist.
//
// Nur "gibt es nicht" ist ein leerer Anfang (erster Start, nach --forget),
// ebenso eine Datei mit 0 Bytes: dann 0. Alles andere, was nicht glatt geht -
// Rechte, Besitzer nach einer Wiederherstellung, Ein- und Ausgabe, ein Verweis
// ins Leere, Nullbytes, eine Zeile, die keine Freigabe ist, eine Datei ohne
// eine einzige Freigabe - heisst: unbekannt, was gemeint war. Das ist -1, und
// dann gilt weder jemand als freigegeben noch als Erstkontakt.
static int freigaben_lesen(const uint8_t *pub, int *gefunden) {
    char path[1200];
    int dummy;
    if (!gefunden) gefunden = &dummy;
    *gefunden = 0;
    if (config_path("authorized.txt", path, sizeof path)) return -1;
    FILE *f = fopen(path, "r");
    if (!f) return fehlt(path, errno) ? 0 : -1;
    struct stat st;
    if (fstat(fileno(f), &st) != 0 || st.st_size > QC_FREIGABEN_MAX) { fclose(f); return -1; }

    char hex[65];
    if (pub) for (int i = 0; i < 32; i++) snprintf(hex + i * 2, 3, "%02x", pub[i]);
    char *z = NULL;
    size_t cap = 0, gelesen = 0;
    ssize_t len;
    int n = 0, kaputt = 0;
    while ((len = getline(&z, &cap, f)) > 0) {
        gelesen += (size_t)len;
        size_t l = (size_t)len;
        if (z[l - 1] == '\n') l--;
        int art = freigabe_zeile(z, l);
        if (art < 0) { kaputt = 1; break; }
        if (art == 1) {
            n++;
            if (pub && strncasecmp(z, hex, 64) == 0) *gefunden = 1;
        }
    }
    if (ferror(f)) kaputt = 1;
    free(z);
    fclose(f);
    if (kaputt || (gelesen && !n)) { *gefunden = 0; return -1; }
    return n;
}

int qc_is_authorized(const uint8_t pub[32]) {
    int gefunden;
    if (freigaben_lesen(pub, &gefunden) < 0) return -1;
    return gefunden;
}

int qc_authorized_count(void) {
    return freigaben_lesen(NULL, NULL);
}

int qc_authorize(const uint8_t pub[32], const char *name) {
    int bek = qc_is_authorized(pub);
    if (bek < 0) return -1;             // Liste nicht lesbar oder beschaedigt: nichts anhaengen
    if (bek) return 0;
    char path[1200];
    if (config_path("authorized.txt", path, sizeof path)) return -1;

    // Die Zeile entsteht vorher ganz und geht mit einem einzigen write ans
    // Ende. zeile[0] haelt Platz fuer einen Zeilenwechsel davor frei.
    char zeile[400], fp[24];
    size_t n = 1;
    for (int i = 0; i < 32; i++) n += (size_t)snprintf(zeile + n, sizeof zeile - n, "%02x", pub[i]);
    qc_fingerprint(pub, fp);
    n += (size_t)snprintf(zeile + n, sizeof zeile - n, "  %s  ", fp);
    const char *nm = name && *name ? name : "-";
    for (size_t i = 0; nm[i] && i < 200; i++) {
        unsigned char c = (unsigned char)nm[i];
        zeile[n++] = c < 0x20 || c == 0x7f ? '?' : (char)c;     // ein Name bleibt eine Zeile
    }
    zeile[n++] = '\n';

    int fd = open(path, O_RDWR | O_APPEND | O_CREAT | O_CLOEXEC, 0600);
    if (fd < 0) return -1;
    struct stat st;
    int fehler = fstat(fd, &st) != 0;
    // Endet die letzte Zeile ohne Zeilenwechsel (von Hand bearbeitet), landete
    // die neue Freigabe sonst mit in dieser Zeile - und wuerde nie gefunden.
    const char *von = zeile + 1;
    if (!fehler && st.st_size > 0) {
        char letztes = 0;
        if (pread(fd, &letztes, 1, st.st_size - 1) != 1) fehler = 1;
        else if (letztes != '\n') { zeile[0] = '\n'; von = zeile; }
    }
    size_t soll = (size_t)(zeile + n - von);
    if (!fehler && (write(fd, von, soll) != (ssize_t)soll || fsync(fd) != 0)) {
        // Halb oder unsicher geschrieben (Platte voll, Ein- und Ausgabe):
        // zurueck auf den alten Stand, sonst waere die ganze Liste beschaedigt.
        (void)ftruncate(fd, st.st_size);
        fehler = 1;
    }
    fchmod(fd, 0600);
    if (close(fd) != 0) fehler = 1;
    if (fehler) return -1;
    // 0 erst, wenn sich die Freigabe aus der Liste wieder lesen laesst.
    return qc_is_authorized(pub) == 1 ? 0 : -1;
}
