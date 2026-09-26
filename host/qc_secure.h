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
#define QC_HANDSCHLAG_MS 5000       // Frist fuer den ganzen Handschlag

typedef struct {
    int fd;
    qc_cipher tx, rx;
    uint8_t hh[QC_HASHLEN];         // Handschlagpruefsumme, bindet den zweiten Kanal
    uint8_t peer[32];               // langlebiger Schluessel der Gegenseite
    uint8_t ct[QC_CHUNK_MAX + QC_TAGLEN];   // eingehender Datensatz
    uint8_t in[QC_CHUNK_MAX + QC_TAGLEN];   // entschluesselt
    size_t in_len, in_pos;
    int ok;
    // Bytes, die qc_chan_send bisher dem Socket uebergeben hat, samt Laengen
    // und Tags - dieselbe Zaehlung wie SO_NWRITE. Aus beiden zusammen folgt,
    // wie viel die Gegenstelle abgenommen hat (Stauregel in main.m).
    uint64_t gesendet;
} qc_chan;

/// Handschlag als Antwortender (der Host wartet, der Client faengt an).
/// Gibt 0 zurueck bei Erfolg. Nach QC_HANDSCHLAG_MS ist Schluss, auch wenn
/// die Gegenstelle noch tropfenweise sendet.
int qc_chan_accept(qc_chan *c, int fd, const uint8_t s_priv[32],
                   const uint8_t *prologue, size_t prologue_len);

/// Loescht Schluessel und Puffer und gibt den Kanal frei. NULL ist erlaubt.
void qc_chan_free(qc_chan *c);

/// Speicher loeschen, ohne dass der Uebersetzer das wegoptimiert.
void qc_wipe(void *p, size_t n);

/// Verschluesselt die Teile und schreibt sie am Stueck. 0 = Erfolg.
int qc_chan_send(qc_chan *c, const struct iovec *iov, int cnt);

/// Liest genau n Klartextbytes. 0 = Erfolg.
int qc_chan_read(qc_chan *c, void *buf, size_t n);

/// Dauerhafter eigener Schluessel. Legt ihn beim ersten Start an.
/// Pfad: ~/Library/Application Support/QuadChroma/host.key
/// 0 = geladen oder neu angelegt, -1 = liess sich nicht anlegen,
/// -2 = vorhanden, aber nicht lesbar oder beschaedigt (bleibt unangetastet).
int qc_identity_load(uint8_t priv[32], uint8_t pub[32]);

/// Ist dieser oeffentliche Schluessel schon freigegeben?
/// 1 = ja, 0 = nein (auch: es gibt noch keine Liste oder sie hat 0 Bytes),
/// -1 = Liste nicht lesbar oder beschaedigt - dann auch fuer Bekannte.
int qc_is_authorized(const uint8_t pub[32]);

/// Schluessel dauerhaft freigeben. 0 erst, wenn er wirklich in der Liste
/// steht und sich von dort wieder lesen laesst.
int qc_authorize(const uint8_t pub[32], const char *name);

/// Wie viele Gegenstellen sind bisher freigegeben? 0 nur, wenn es keine
/// Liste gibt oder sie 0 Bytes hat. -1 = nicht lesbar oder beschaedigt.
int qc_authorized_count(void);

/// Pfad einer Datei im Ablageordner ~/Library/Application Support/QuadChroma
/// (der Ordner wird angelegt). HOME aus der Umgebung, sonst aus getpwuid.
/// 0 = out gefuellt, -1 = kein HOME.
int qc_config_path(const char *file, char *out, size_t cap);

#endif
