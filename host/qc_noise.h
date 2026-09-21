// Noise_XX_25519_ChaChaPoly_SHA256 fuer den QuadChroma-Host.
//
// Ein Muster fuer alles: der Client faengt an, beide Seiten weisen sich mit
// einem langlebigen Schluesselpaar aus, und am Ende fallen zwei Sitzungs-
// schluessel heraus, je einer je Richtung. Die Gegenstelle nutzt snow, die
// bekannte Rust-Umsetzung derselben Spezifikation.
//
// Absichtlich nichts selbst erfunden: alle Bausteine stammen aus der
// Noise-Spezifikation (Revision 34) und RFC 8439.
#ifndef QC_NOISE_H
#define QC_NOISE_H

#include <stddef.h>
#include <stdint.h>

#define QC_KEYLEN   32
#define QC_TAGLEN   16
#define QC_HASHLEN  32

// Ein Richtungsschluessel nach dem Handschlag.
typedef struct {
    uint8_t k[QC_KEYLEN];
    uint64_t n;
    int has_key;
} qc_cipher;

typedef struct {
    uint8_t ck[QC_HASHLEN];
    uint8_t h[QC_HASHLEN];
    qc_cipher cs;
} qc_symmetric;

typedef struct {
    qc_symmetric sym;
    uint8_t s_priv[32], s_pub[32];   // eigener langlebiger Schluessel
    uint8_t e_priv[32], e_pub[32];   // Schluessel nur fuer diese Verbindung
    uint8_t re[32];                  // fluechtiger Schluessel der Gegenseite
    uint8_t rs[32];                  // langlebiger Schluessel der Gegenseite
    int have_re, have_rs;
    int initiator;
    int step;                        // 0,1,2 = welche Nachricht als Naechstes
    int done;
} qc_handshake;

// Zufall. Standardmaessig arc4random; fuer Pruefzwecke ersetzbar.
extern void (*qc_random)(uint8_t *out, size_t len);

/// Langlebiges Schluesselpaar erzeugen.
void qc_keypair(uint8_t priv[32], uint8_t pub[32]);

/// Oeffentlichen Schluessel aus einem gespeicherten privaten ableiten.
void qc_pubkey(const uint8_t priv[32], uint8_t pub[32]);

/// Handschlag vorbereiten. prologue bindet den Kanal an seinen Zweck.
void qc_handshake_init(qc_handshake *hs, int initiator,
                       const uint8_t s_priv[32],
                       const uint8_t *prologue, size_t prologue_len);

/// Naechste Handschlagnachricht schreiben. Gibt 0 zurueck bei Erfolg.
int qc_handshake_write(qc_handshake *hs, const uint8_t *payload, size_t payload_len,
                       uint8_t *out, size_t *out_len);

/// Naechste Handschlagnachricht lesen. Gibt 0 zurueck bei Erfolg.
int qc_handshake_read(qc_handshake *hs, const uint8_t *msg, size_t msg_len,
                      uint8_t *payload, size_t *payload_len);

/// Nach der letzten Nachricht: Sitzungsschluessel abholen.
void qc_handshake_split(qc_handshake *hs, qc_cipher *send, qc_cipher *recv);

/// Handschlagpruefsumme, bindet den zweiten Kanal an den ersten.
const uint8_t *qc_handshake_hash(const qc_handshake *hs);

/// Sechsstelliger Vergleichscode aus der Pruefsumme, Format "123 456".
void qc_sas(const uint8_t h[QC_HASHLEN], char out[8]);

/// Fingerabdruck eines oeffentlichen Schluessels zum Anzeigen.
void qc_fingerprint(const uint8_t pub[32], char out[24]);

/// Nutzdaten verschluesseln. out braucht len + 16 Bytes Platz.
int qc_encrypt(qc_cipher *c, const uint8_t *in, size_t len, uint8_t *out, size_t *out_len);

/// Nutzdaten entschluesseln. Gibt ungleich 0 zurueck, wenn die Pruefsumme nicht passt.
int qc_decrypt(qc_cipher *c, const uint8_t *in, size_t len, uint8_t *out, size_t *out_len);

/// Selbsttest: Handschlag gegen sich selbst, feste Schluessel, bekannte Werte.
int qc_noise_selftest(void);

#endif
