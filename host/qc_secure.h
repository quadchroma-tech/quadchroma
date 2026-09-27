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
#define QC_NUTZLAST3_MAX 64         // so viel von der Nutzlast der Nachricht 3 wird aufgehoben

typedef struct {
    int fd;
    qc_cipher tx, rx;
    uint8_t hh[QC_HASHLEN];         // Handschlagpruefsumme, bindet den zweiten Kanal
    uint8_t peer[32];               // langlebiger Schluessel der Gegenseite
    // Nutzlast der Handschlag-Nachricht 3 (Client -> Host, verschluesselt):
    // heute "QCN1" | u8 n | Name des Clients | u8 Flags (zugang.h), bei alten
    // Clients ohne Flags oder "client". Der Name ist nur Anzeige und
    // unbeglaubigt; Bit 0 der Flags verlangt den Ausweis des Hosts in der
    // Zugangsphase. Was ueber QC_NUTZLAST3_MAX liegt, faellt weg.
    uint8_t nutzlast3[QC_NUTZLAST3_MAX];
    size_t nutzlast3_len;
    uint8_t ct[QC_CHUNK_MAX + QC_TAGLEN];   // eingehender Datensatz
    uint8_t in[QC_CHUNK_MAX + QC_TAGLEN];   // entschluesselt
    size_t in_len, in_pos;
    int ok;
    // Nach einem gescheiterten qc_chan_read_frist ist der Empfang hin (ein
    // halber Datensatz ist verloren), das Senden nicht: tx haengt nicht an rx.
    // So kann der Host noch eine letzte Nachricht schicken (Zugang: 22/4).
    int lesen_aus;
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

/// Wie qc_chan_read, aber mit Frist: nach frist_ms (ab jetzt, fuer alle n
/// Bytes zusammen) ist Schluss, auch wenn die Gegenstelle tropfenweise sendet.
/// 0 = Erfolg, -1 = Frist, Ende der Verbindung oder Fehler - danach liest der
/// Kanal nichts mehr; senden laesst sich noch (eine letzte Nachricht), dann
/// wird er geschlossen.
int qc_chan_read_frist(qc_chan *c, void *buf, size_t n, int frist_ms);

/// Schon entschluesselte, noch nicht abgeholte Bytes. Solange es welche gibt,
/// meldet poll() auf dem Socket nichts - wer vor dem Lesen wartet, fragt erst hier.
size_t qc_chan_gepuffert(const qc_chan *c);

/// Dauerhafter eigener Schluessel. Legt ihn beim ersten Start an.
/// Pfad: ~/Library/Application Support/QuadChroma/host.key
/// 0 = geladen oder neu angelegt, -1 = liess sich nicht anlegen,
/// -2 = vorhanden, aber nicht lesbar oder beschaedigt (bleibt unangetastet).
int qc_identity_load(uint8_t priv[32], uint8_t pub[32]);

/// Wie qc_identity_load, fuer eine beliebige Datei (Tests). Legt ein anderer
/// die Datei an, waehrend dieser Lader erzeugt (EEXIST beim Anlegen), gilt
/// dessen Schluessel: sie wird neu gelesen, nie ueberschrieben.
int qc_identity_load_pfad(const char *path, uint8_t priv[32], uint8_t pub[32]);

// Die Liste der erlaubten Geraete (frueher authorized.txt, hier) steht jetzt
// in zugang.h (host-devices.txt, samt Migration).

/// Pfad einer Datei im Ablageordner ~/Library/Application Support/QuadChroma
/// (der Ordner wird angelegt). HOME aus der Umgebung, sonst aus getpwuid.
/// 0 = out gefuellt, -1 = kein HOME.
int qc_config_path(const char *file, char *out, size_t cap);

#endif
