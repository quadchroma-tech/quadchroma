// Zeigerform des Macs fuer den Client.
//
// Der Client zeigt seinen eigenen Zeiger - das ist die Regel des Projekts, im
// Video ist keiner. Damit der trotzdem aussieht wie der des Macs (Pfeil, Hand
// ueber Knoepfen, Ziehpfeile am Fensterrand, Textcursor, Wartekugel), schickt
// der Host die FORM: das Bild des systemweiten Zeigers samt Hotspot, und ob er
// ueberhaupt sichtbar ist. Nur bei Aenderung, nur solange jemand zuschaut.
#ifndef QC_ZEIGER_H
#define QC_ZEIGER_H

#include <stddef.h>
#include <stdint.h>

/// Rueckruf: Groesse in Bildpunkten, Hotspot (von links oben), sichtbar,
/// RGBA mit gerader (nicht vormultiplizierter) Deckkraft, w*h*4 Byte.
typedef void (*qc_zeiger_cb)(uint16_t w, uint16_t h, uint16_t hx, uint16_t hy,
                             int sichtbar, const uint8_t *rgba);

/// Abfrage starten: alle 50 ms auf der Hauptwarteschlange. `aktiv` sagt, ob
/// gerade jemand zuschaut - sonst wird nichts abgefragt und nichts gerechnet.
void qc_zeiger_start(qc_zeiger_cb cb, int (*aktiv)(void));

/// Beim naechsten Durchlauf die Form auf jeden Fall schicken - ein neuer
/// Zuschauer kennt sie noch nicht.
void qc_zeiger_neu_senden(void);

#endif
