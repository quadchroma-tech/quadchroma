// Gesicherter Kanal ueber eine bestehende TCP-Verbindung.
//
// Nach dem Handschlag wandert der komplette bisherige Datenstrom unveraendert
// durch diese Schicht: sie zerlegt ihn in Stuecke, verschluesselt jedes davon
// und stellt auf der Gegenseite wieder einen fortlaufenden Strom her. Der Rest
// des Programms merkt davon nichts ausser dem Aufruf, der statt write/read
// jetzt qc_chan_send/qc_chan_read heisst.
#ifndef QC_SECURE_H
#define QC_SECURE_H

#include "qc_noise.h"
#include <stddef.h>
#include <stdint.h>
#include <sys/uio.h>

#define QC_CHUNK_MAX 65519          // groesstes Klartextstueck je Datensatz

typedef struct {
    int fd;
    qc_cipher tx, rx;
    uint8_t hh[QC_HASHLEN];         // Handschlagpruefsumme, bindet den zweiten Kanal
    uint8_t peer[32];               // langlebiger Schluessel der Gegenseite
    uint8_t ct[QC_CHUNK_MAX + QC_TAGLEN];   // eingehender Datensatz
    uint8_t in[QC_CHUNK_MAX + QC_TAGLEN];   // entschluesselt
    size_t in_len, in_pos;
    int ok;
} qc_chan;

/// Handschlag als Antwortender (der Host wartet, der Client faengt an).
/// Gibt 0 zurueck bei Erfolg.
int qc_chan_accept(qc_chan *c, int fd, const uint8_t s_priv[32],
                   const uint8_t *prologue, size_t prologue_len);

/// Verschluesselt die Teile und schreibt sie am Stueck. 0 = Erfolg.
int qc_chan_send(qc_chan *c, const struct iovec *iov, int cnt);

/// Liest genau n Klartextbytes. 0 = Erfolg.
int qc_chan_read(qc_chan *c, void *buf, size_t n);

/// Dauerhafter eigener Schluessel. Legt ihn beim ersten Start an.
/// Pfad: ~/Library/Application Support/QuadChroma/host.key
int qc_identity_load(uint8_t priv[32], uint8_t pub[32]);

/// Ist dieser oeffentliche Schluessel schon freigegeben?
/// 1 = ja, 0 = nein (auch: es gibt noch keine Liste), -1 = Liste nicht lesbar.
int qc_is_authorized(const uint8_t pub[32]);

/// Schluessel dauerhaft freigeben. 0 erst, wenn er wirklich in der Liste steht.
int qc_authorize(const uint8_t pub[32], const char *name);

/// Wie viele Gegenstellen sind bisher freigegeben? -1 = Liste nicht lesbar.
int qc_authorized_count(void);

#endif
