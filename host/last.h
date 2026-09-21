// Auslastung des Hosts.
//
// Alles aus dem eigenen Prozess heraus, ohne Root, ohne Hilfsprogramm. Die
// Wege sind auf diesem Mac nachgemessen und kosten zusammen rund eine
// Millisekunde Rechenzeit je Sekunde.
//
// Eine Groesse fehlt bewusst: die Auslastung der Video-Einheit. Der
// Hardware-Encoder taucht in keinem lesbaren Zaehler auf - waehrend 156
// Bildern je Sekunde in Hardware meldet die GPU-Auslastung 0 %. Statt eine
// Zahl zu erfinden, schickt der Host die Encoderzeit je Bild mit; die sagt im
// Vergleich zum Bildabstand mehr, als ein Prozentwert je koennte.
#ifndef QC_LAST_H
#define QC_LAST_H

#include <stdint.h>

typedef struct {
    uint16_t cpu_promille;       // Auslastung aller Kerne, 0..1000
    uint16_t cpu_eigen_promille; // was dieser Prozess davon braucht, gleiche
                                 // Bezugsgroesse (alle Kerne), 0..1000
    uint16_t gpu_promille;       // Grafikkerne, 0..1000; 0xffff = nicht lesbar
    uint16_t druck;              // 1 normal, 2 erhoeht, 4 kritisch
    uint32_t ram_benutzt_mb;
    uint32_t ram_gesamt_mb;
    uint32_t eigen_mb;           // Speicher dieses Prozesses
} qc_last;

/// Eine Probe nehmen. Der erste Aufruf legt nur den Bezugspunkt an und liefert
/// Nullen fuer die Kernauslastung - die entsteht erst aus der Differenz.
void qc_last_probe(qc_last *out);

#endif
