#include "qc_secure.h"

#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

// ------------------------------------------------------------- Rohes Lesen

static int raw_read(int fd, void *buf, size_t n) {
    uint8_t *p = buf;
    while (n) {
        ssize_t r = recv(fd, p, n, 0);
        if (r == 0) return -1;
        if (r < 0) { if (errno == EINTR) continue; return -1; }
        p += r; n -= (size_t)r;
    }
    return 0;
}

static int raw_write(int fd, const void *buf, size_t n) {
    const uint8_t *p = buf;
    while (n) {
        ssize_t w = send(fd, p, n, 0);
        if (w <= 0) { if (w < 0 && errno == EINTR) continue; return -1; }
        p += w; n -= (size_t)w;
    }
    return 0;
}

static int read_frame(int fd, uint8_t *buf, size_t cap, size_t *len) {
    uint8_t l[2];
    if (raw_read(fd, l, 2)) return -1;
    size_t n = (size_t)l[0] | ((size_t)l[1] << 8);
    if (n > cap) return -1;
    if (raw_read(fd, buf, n)) return -1;
    *len = n;
    return 0;
}

static int write_frame(int fd, const uint8_t *buf, size_t len) {
    uint8_t l[2] = { (uint8_t)(len & 255), (uint8_t)(len >> 8) };
    if (raw_write(fd, l, 2)) return -1;
    return raw_write(fd, buf, len);
}

// -------------------------------------------------------------- Handschlag

int qc_chan_accept(qc_chan *c, int fd, const uint8_t s_priv[32],
                   const uint8_t *prologue, size_t prologue_len) {
    memset(c, 0, sizeof *c);
    c->fd = fd;

    qc_handshake hs;
    qc_handshake_init(&hs, 0, s_priv, prologue, prologue_len);

    uint8_t msg[8192], payload[8192];
    size_t mlen = 0, plen = 0;

    if (read_frame(fd, msg, sizeof msg, &mlen)) return -1;
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) return -2;

    if (qc_handshake_write(&hs, NULL, 0, msg, &mlen)) return -3;
    if (write_frame(fd, msg, mlen)) return -4;

    if (read_frame(fd, msg, sizeof msg, &mlen)) return -5;
    if (qc_handshake_read(&hs, msg, mlen, payload, &plen)) return -6;

    memcpy(c->hh, qc_handshake_hash(&hs), QC_HASHLEN);
    memcpy(c->peer, hs.rs, 32);
    qc_handshake_split(&hs, &c->tx, &c->rx);
    c->ok = 1;
    return 0;
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
        if (qc_encrypt(&c->tx, plain, want, out + outp + 2, &ct)) { free(out); return -1; }
        out[outp] = (uint8_t)(ct & 255);
        out[outp + 1] = (uint8_t)(ct >> 8);
        outp += 2 + ct;
        total -= want;
    }

    int r = raw_write(c->fd, out, outp);
    free(out);
    return r;
}

int qc_chan_read(qc_chan *c, void *buf, size_t n) {
    if (!c->ok) return -1;
    uint8_t *p = buf;
    while (n) {
        if (c->in_pos == c->in_len) {
            size_t ctlen = 0;
            if (read_frame(c->fd, c->ct, sizeof c->ct, &ctlen)) return -1;
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

int qc_identity_load(uint8_t priv[32], uint8_t pub[32]) {
    char path[1200];
    if (config_path("host.key", path, sizeof path)) return -1;

    int fd = open(path, O_RDONLY);
    if (fd >= 0) {
        ssize_t r = read(fd, priv, 32);
        close(fd);
        if (r == 32) {
            qc_pubkey(priv, pub);
            return 0;
        }
    }

    qc_keypair(priv, pub);
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return -1;
    ssize_t w = write(fd, priv, 32);
    close(fd);
    return w == 32 ? 0 : -1;
}

static int hex_eq(const char *line, const uint8_t pub[32]) {
    char hex[65];
    for (int i = 0; i < 32; i++) snprintf(hex + i * 2, 3, "%02x", pub[i]);
    return strncmp(line, hex, 64) == 0;
}

int qc_is_authorized(const uint8_t pub[32]) {
    char path[1200];
    if (config_path("authorized.txt", path, sizeof path)) return 0;
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char line[512];
    int found = 0;
    while (fgets(line, sizeof line, f)) {
        if (hex_eq(line, pub)) { found = 1; break; }
    }
    fclose(f);
    return found;
}

int qc_authorize(const uint8_t pub[32], const char *name) {
    if (qc_is_authorized(pub)) return 0;
    char path[1200];
    if (config_path("authorized.txt", path, sizeof path)) return -1;
    FILE *f = fopen(path, "a");
    if (!f) return -1;
    for (int i = 0; i < 32; i++) fprintf(f, "%02x", pub[i]);
    char fp[24];
    qc_fingerprint(pub, fp);
    fprintf(f, "  %s  %s\n", fp, name ? name : "-");
    fclose(f);
    chmod(path, 0600);
    return 0;
}

int qc_authorized_count(void) {
    char path[1200];
    if (config_path("authorized.txt", path, sizeof path)) return 0;
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char line[512];
    int n = 0;
    while (fgets(line, sizeof line, f)) if (strlen(line) > 64) n++;
    fclose(f);
    return n;
}
