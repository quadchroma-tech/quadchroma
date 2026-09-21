// Noise_XX_25519_ChaChaPoly_SHA256, Umsetzung nach Spezifikation Revision 34.
//
// Bausteine: X25519, ChaCha20 und Poly1305 aus Monocypher, SHA-256 und HMAC
// aus CommonCrypto. Die AEAD-Verknuepfung folgt RFC 8439, Abschnitt 2.8 -
// Monocyphers eigene crypto_aead_* wird bewusst NICHT benutzt, weil sie
// XChaCha20 mit 24-Byte-Nonce verwendet und damit nicht zur Gegenstelle passt.

#include "qc_noise.h"
#include "vendor/monocypher/monocypher.h"
#include <CommonCrypto/CommonDigest.h>
#include <CommonCrypto/CommonHMAC.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>

static const char PROTOCOL[] = "Noise_XX_25519_ChaChaPoly_SHA256";

static void sys_random(uint8_t *out, size_t len) { arc4random_buf(out, len); }
void (*qc_random)(uint8_t *out, size_t len) = sys_random;

// ------------------------------------------------------------ Grundrechnung

static void sha256(const uint8_t *a, size_t alen, const uint8_t *b, size_t blen, uint8_t out[32]) {
    CC_SHA256_CTX c;
    CC_SHA256_Init(&c);
    if (alen) CC_SHA256_Update(&c, a, (CC_LONG)alen);
    if (blen) CC_SHA256_Update(&c, b, (CC_LONG)blen);
    CC_SHA256_Final(out, &c);
}

static void hmac(const uint8_t key[32], const uint8_t *data, size_t len, uint8_t out[32]) {
    CCHmac(kCCHmacAlgSHA256, key, 32, data, len, out);
}

/// HKDF wie in der Noise-Spezifikation, zwei oder drei Ausgaben.
static void hkdf(const uint8_t ck[32], const uint8_t *ikm, size_t ikm_len,
                 int outputs, uint8_t o1[32], uint8_t o2[32], uint8_t o3[32]) {
    uint8_t temp[32], buf[33];
    hmac(ck, ikm, ikm_len, temp);
    buf[0] = 1;
    hmac(temp, buf, 1, o1);
    memcpy(buf, o1, 32);
    buf[32] = 2;
    hmac(temp, buf, 33, o2);
    if (outputs > 2 && o3) {
        memcpy(buf, o2, 32);
        buf[32] = 3;
        hmac(temp, buf, 33, o3);
    }
    crypto_wipe(temp, sizeof temp);
    crypto_wipe(buf, sizeof buf);
}

// ------------------------------------------------- AEAD nach RFC 8439 §2.8

static void pad16(crypto_poly1305_ctx *ctx, size_t len) {
    static const uint8_t zero[16] = {0};
    size_t rest = len % 16;
    if (rest) crypto_poly1305_update(ctx, zero, 16 - rest);
}

static void le64(uint8_t out[8], uint64_t v) {
    for (int i = 0; i < 8; i++) out[i] = (uint8_t)(v >> (8 * i));
}

static void nonce_bytes(uint64_t n, uint8_t out[12]) {
    memset(out, 0, 4);                 // vier Nullbytes, dann der Zaehler
    for (int i = 0; i < 8; i++) out[4 + i] = (uint8_t)(n >> (8 * i));
}

static void aead_tag(const uint8_t key[32], const uint8_t nonce[12],
                     const uint8_t *ad, size_t ad_len,
                     const uint8_t *ct, size_t ct_len, uint8_t tag[16]) {
    uint8_t poly_key[64] = {0};
    crypto_chacha20_ietf(poly_key, poly_key, 64, key, nonce, 0);  // Block 0 ist der Poly1305-Schluessel

    crypto_poly1305_ctx ctx;
    crypto_poly1305_init(&ctx, poly_key);
    if (ad_len) { crypto_poly1305_update(&ctx, ad, ad_len); pad16(&ctx, ad_len); }
    if (ct_len) { crypto_poly1305_update(&ctx, ct, ct_len); pad16(&ctx, ct_len); }
    uint8_t lens[16];
    le64(lens, ad_len);
    le64(lens + 8, ct_len);
    crypto_poly1305_update(&ctx, lens, 16);
    crypto_poly1305_final(&ctx, tag);
    crypto_wipe(poly_key, sizeof poly_key);
}

static void aead_encrypt(const uint8_t key[32], uint64_t n,
                         const uint8_t *ad, size_t ad_len,
                         const uint8_t *pt, size_t pt_len, uint8_t *out) {
    uint8_t nonce[12];
    nonce_bytes(n, nonce);
    if (pt_len) crypto_chacha20_ietf(out, pt, pt_len, key, nonce, 1);  // Zaehler 1, Block 0 gehoert Poly1305
    aead_tag(key, nonce, ad, ad_len, out, pt_len, out + pt_len);
}

static int aead_decrypt(const uint8_t key[32], uint64_t n,
                        const uint8_t *ad, size_t ad_len,
                        const uint8_t *ct, size_t ct_len, uint8_t *out) {
    if (ct_len < QC_TAGLEN) return -1;
    size_t body = ct_len - QC_TAGLEN;
    uint8_t nonce[12], tag[16];
    nonce_bytes(n, nonce);
    aead_tag(key, nonce, ad, ad_len, ct, body, tag);
    if (crypto_verify16(tag, ct + body) != 0) return -1;
    if (body) crypto_chacha20_ietf(out, ct, body, key, nonce, 1);
    return 0;
}

// ------------------------------------------------------------- CipherState

static void cipher_init(qc_cipher *c, const uint8_t k[32]) {
    memcpy(c->k, k, 32);
    c->n = 0;
    c->has_key = 1;
}

int qc_encrypt(qc_cipher *c, const uint8_t *in, size_t len, uint8_t *out, size_t *out_len) {
    if (!c->has_key) return -1;
    aead_encrypt(c->k, c->n, NULL, 0, in, len, out);
    c->n++;
    *out_len = len + QC_TAGLEN;
    return 0;
}

int qc_decrypt(qc_cipher *c, const uint8_t *in, size_t len, uint8_t *out, size_t *out_len) {
    if (!c->has_key) return -1;
    if (aead_decrypt(c->k, c->n, NULL, 0, in, len, out) != 0) return -1;
    c->n++;
    *out_len = len - QC_TAGLEN;
    return 0;
}

// ---------------------------------------------------------- SymmetricState

static void sym_init(qc_symmetric *s) {
    size_t len = strlen(PROTOCOL);
    memset(s->h, 0, 32);
    if (len <= 32) memcpy(s->h, PROTOCOL, len);
    else sha256((const uint8_t *)PROTOCOL, len, NULL, 0, s->h);
    memcpy(s->ck, s->h, 32);
    memset(&s->cs, 0, sizeof s->cs);
}

static void sym_mix_hash(qc_symmetric *s, const uint8_t *data, size_t len) {
    sha256(s->h, 32, data, len, s->h);
}

static void sym_mix_key(qc_symmetric *s, const uint8_t ikm[32]) {
    uint8_t ck[32], k[32];
    hkdf(s->ck, ikm, 32, 2, ck, k, NULL);
    memcpy(s->ck, ck, 32);
    cipher_init(&s->cs, k);
    crypto_wipe(ck, 32);
    crypto_wipe(k, 32);
}

static void sym_encrypt_and_hash(qc_symmetric *s, const uint8_t *pt, size_t len, uint8_t *out, size_t *out_len) {
    if (s->cs.has_key) {
        aead_encrypt(s->cs.k, s->cs.n, s->h, 32, pt, len, out);
        s->cs.n++;
        *out_len = len + QC_TAGLEN;
    } else {
        if (len) memcpy(out, pt, len);
        *out_len = len;
    }
    sym_mix_hash(s, out, *out_len);
}

static int sym_decrypt_and_hash(qc_symmetric *s, const uint8_t *ct, size_t len, uint8_t *out, size_t *out_len) {
    uint8_t keep[32];
    memcpy(keep, s->h, 32);
    if (s->cs.has_key) {
        if (aead_decrypt(s->cs.k, s->cs.n, keep, 32, ct, len, out) != 0) return -1;
        s->cs.n++;
        *out_len = len - QC_TAGLEN;
    } else {
        if (len) memcpy(out, ct, len);
        *out_len = len;
    }
    sym_mix_hash(s, ct, len);
    return 0;
}

// ---------------------------------------------------------- HandshakeState

void qc_keypair(uint8_t priv[32], uint8_t pub[32]) {
    qc_random(priv, 32);
    crypto_x25519_public_key(pub, priv);
}

void qc_pubkey(const uint8_t priv[32], uint8_t pub[32]) {
    crypto_x25519_public_key(pub, priv);
}

void qc_handshake_init(qc_handshake *hs, int initiator, const uint8_t s_priv[32],
                       const uint8_t *prologue, size_t prologue_len) {
    memset(hs, 0, sizeof *hs);
    hs->initiator = initiator;
    memcpy(hs->s_priv, s_priv, 32);
    crypto_x25519_public_key(hs->s_pub, hs->s_priv);
    sym_init(&hs->sym);
    sym_mix_hash(&hs->sym, prologue, prologue_len);
}

static void dh(uint8_t out[32], const uint8_t priv[32], const uint8_t pub[32]) {
    crypto_x25519(out, priv, pub);
}

int qc_handshake_write(qc_handshake *hs, const uint8_t *payload, size_t payload_len,
                       uint8_t *out, size_t *out_len) {
    size_t o = 0, n = 0;
    uint8_t shared[32];

    if (hs->step == 0 && hs->initiator) {
        // -> e
        qc_keypair(hs->e_priv, hs->e_pub);
        memcpy(out, hs->e_pub, 32); o = 32;
        sym_mix_hash(&hs->sym, hs->e_pub, 32);
        sym_encrypt_and_hash(&hs->sym, payload, payload_len, out + o, &n); o += n;
        hs->step = 1;
    } else if (hs->step == 1 && !hs->initiator) {
        // <- e, ee, s, es
        qc_keypair(hs->e_priv, hs->e_pub);
        memcpy(out, hs->e_pub, 32); o = 32;
        sym_mix_hash(&hs->sym, hs->e_pub, 32);
        dh(shared, hs->e_priv, hs->re); sym_mix_key(&hs->sym, shared);        // ee
        sym_encrypt_and_hash(&hs->sym, hs->s_pub, 32, out + o, &n); o += n;   // s
        dh(shared, hs->s_priv, hs->re); sym_mix_key(&hs->sym, shared);        // es
        sym_encrypt_and_hash(&hs->sym, payload, payload_len, out + o, &n); o += n;
        hs->step = 2;
    } else if (hs->step == 2 && hs->initiator) {
        // -> s, se
        sym_encrypt_and_hash(&hs->sym, hs->s_pub, 32, out, &n); o = n;        // s
        dh(shared, hs->s_priv, hs->re); sym_mix_key(&hs->sym, shared);        // se
        sym_encrypt_and_hash(&hs->sym, payload, payload_len, out + o, &n); o += n;
        hs->step = 3;
        hs->done = 1;
    } else {
        return -1;
    }
    crypto_wipe(shared, 32);
    *out_len = o;
    return 0;
}

int qc_handshake_read(qc_handshake *hs, const uint8_t *msg, size_t msg_len,
                      uint8_t *payload, size_t *payload_len) {
    size_t o = 0, n = 0;
    uint8_t shared[32], tmp[64];

    if (hs->step == 0 && !hs->initiator) {
        if (msg_len < 32) return -1;
        memcpy(hs->re, msg, 32); hs->have_re = 1; o = 32;
        sym_mix_hash(&hs->sym, hs->re, 32);
        if (sym_decrypt_and_hash(&hs->sym, msg + o, msg_len - o, payload, &n) != 0) return -1;
        *payload_len = n;
        hs->step = 1;
    } else if (hs->step == 1 && hs->initiator) {
        if (msg_len < 32 + 32 + QC_TAGLEN) return -1;
        memcpy(hs->re, msg, 32); hs->have_re = 1; o = 32;
        sym_mix_hash(&hs->sym, hs->re, 32);
        dh(shared, hs->e_priv, hs->re); sym_mix_key(&hs->sym, shared);        // ee
        if (sym_decrypt_and_hash(&hs->sym, msg + o, 32 + QC_TAGLEN, tmp, &n) != 0) return -1;
        if (n != 32) return -1;
        memcpy(hs->rs, tmp, 32); hs->have_rs = 1; o += 32 + QC_TAGLEN;        // s
        dh(shared, hs->e_priv, hs->rs); sym_mix_key(&hs->sym, shared);        // es
        if (sym_decrypt_and_hash(&hs->sym, msg + o, msg_len - o, payload, &n) != 0) return -1;
        *payload_len = n;
        hs->step = 2;
    } else if (hs->step == 2 && !hs->initiator) {
        if (msg_len < 32 + QC_TAGLEN) return -1;
        if (sym_decrypt_and_hash(&hs->sym, msg, 32 + QC_TAGLEN, tmp, &n) != 0) return -1;
        if (n != 32) return -1;
        memcpy(hs->rs, tmp, 32); hs->have_rs = 1; o = 32 + QC_TAGLEN;         // s
        dh(shared, hs->e_priv, hs->rs); sym_mix_key(&hs->sym, shared);        // se
        if (sym_decrypt_and_hash(&hs->sym, msg + o, msg_len - o, payload, &n) != 0) return -1;
        *payload_len = n;
        hs->step = 3;
        hs->done = 1;
    } else {
        return -1;
    }
    crypto_wipe(shared, 32);
    crypto_wipe(tmp, sizeof tmp);
    return 0;
}

void qc_handshake_split(qc_handshake *hs, qc_cipher *send, qc_cipher *recv) {
    uint8_t k1[32], k2[32];
    hkdf(hs->sym.ck, NULL, 0, 2, k1, k2, NULL);
    // Der Anrufer sendet mit dem ersten Schluessel, der Angerufene mit dem zweiten.
    cipher_init(send, hs->initiator ? k1 : k2);
    cipher_init(recv, hs->initiator ? k2 : k1);
    crypto_wipe(k1, 32);
    crypto_wipe(k2, 32);
}

const uint8_t *qc_handshake_hash(const qc_handshake *hs) { return hs->sym.h; }

void qc_sas(const uint8_t h[QC_HASHLEN], char out[8]) {
    static const char label[] = "QuadChroma/1 SAS";
    uint8_t d[32];
    uint8_t buf[17];
    memcpy(buf, label, 16);
    buf[16] = 0;
    sha256(buf, 17, h, 32, d);
    uint32_t v = ((uint32_t)d[0] << 24) | ((uint32_t)d[1] << 16) | ((uint32_t)d[2] << 8) | d[3];
    snprintf(out, 8, "%03u %03u", (v % 1000000u) / 1000u, (v % 1000000u) % 1000u);
}

void qc_fingerprint(const uint8_t pub[32], char out[24]) {
    uint8_t d[32];
    sha256(pub, 32, NULL, 0, d);
    snprintf(out, 24, "%02X%02X-%02X%02X-%02X%02X-%02X%02X",
             d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]);
}

// ---------------------------------------------------------- Selbsttest

static const uint8_t *g_fixed = NULL;
static int g_fixed_pos = 0;
static void fixed_random(uint8_t *out, size_t len) {
    for (size_t i = 0; i < len; i++) out[i] = g_fixed[(g_fixed_pos++) % 64];
}

int qc_noise_selftest(void) {
    uint8_t i_priv[32], i_pub[32], r_priv[32], r_pub[32];
    static const uint8_t seed[64] = {
        1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,
        33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64
    };
    g_fixed = seed; g_fixed_pos = 0;
    void (*save)(uint8_t *, size_t) = qc_random;
    qc_random = fixed_random;
    qc_keypair(i_priv, i_pub);
    qc_keypair(r_priv, r_pub);

    const char *prologue = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
    qc_handshake ini, res;
    qc_handshake_init(&ini, 1, i_priv, (const uint8_t *)prologue, strlen(prologue));
    qc_handshake_init(&res, 0, r_priv, (const uint8_t *)prologue, strlen(prologue));

    uint8_t m[512], p[512];
    size_t mlen, plen;
    if (qc_handshake_write(&ini, NULL, 0, m, &mlen)) return 1;
    if (qc_handshake_read(&res, m, mlen, p, &plen)) return 2;
    if (qc_handshake_write(&res, NULL, 0, m, &mlen)) return 3;
    if (qc_handshake_read(&ini, m, mlen, p, &plen)) return 4;
    if (qc_handshake_write(&ini, (const uint8_t *)"hallo", 5, m, &mlen)) return 5;
    if (qc_handshake_read(&res, m, mlen, p, &plen)) return 6;
    if (plen != 5 || memcmp(p, "hallo", 5)) return 7;

    // Beide Seiten muessen dieselbe Pruefsumme und denselben Code sehen.
    if (memcmp(qc_handshake_hash(&ini), qc_handshake_hash(&res), 32)) return 8;
    if (memcmp(ini.rs, r_pub, 32)) return 9;
    if (memcmp(res.rs, i_pub, 32)) return 10;

    qc_cipher is, ir, rs_, rr;
    qc_handshake_split(&ini, &is, &ir);
    qc_handshake_split(&res, &rs_, &rr);

    uint8_t ct[128], pt[128];
    size_t ctlen, ptlen;
    if (qc_encrypt(&is, (const uint8_t *)"geheim", 6, ct, &ctlen)) return 11;
    if (qc_decrypt(&rr, ct, ctlen, pt, &ptlen)) return 12;
    if (ptlen != 6 || memcmp(pt, "geheim", 6)) return 13;
    if (qc_encrypt(&rs_, (const uint8_t *)"zurueck", 7, ct, &ctlen)) return 14;
    if (qc_decrypt(&ir, ct, ctlen, pt, &ptlen)) return 15;
    if (ptlen != 7 || memcmp(pt, "zurueck", 7)) return 16;

    // Verfaelschte Nachricht muss auffliegen.
    qc_encrypt(&is, (const uint8_t *)"x", 1, ct, &ctlen);
    ct[0] ^= 0x40;
    if (qc_decrypt(&rr, ct, ctlen, pt, &ptlen) == 0) return 17;

    char sas_a[8], sas_b[8], fp[24];
    qc_sas(qc_handshake_hash(&ini), sas_a);
    qc_sas(qc_handshake_hash(&res), sas_b);
    qc_fingerprint(r_pub, fp);
    if (strcmp(sas_a, sas_b)) return 18;
    printf("Noise-Selbsttest: bestanden. Vergleichscode %s, Fingerabdruck des Hosts %s\n", sas_a, fp);

    qc_random = save;
    return 0;
}
