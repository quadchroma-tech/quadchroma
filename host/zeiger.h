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

// UNGEPRUEFT und wichtig: die Abfrage steht auf [NSCursor currentSystemCursor],
// und Apples Kopfdatei (SDK macOS 27) sagt dazu: "This property will always be
// `nil` in a future version of macOS." Einen oeffentlichen Ersatz gibt es nach
// heutigem Wissen nicht (ScreenCaptureKit kennt nur showsCursor, NSCursor.
// currentCursor sieht nur die eigene Anwendung). Faellt die Abfrage aus, steht
// es im Protokoll, und der Client zeigt seinen eigenen Pfeil - nichts bricht.

/// Rueckruf: Groesse in Bildpunkten DES STROMS, Hotspot (von links oben),
/// sichtbar, RGBA mit gerader (nicht vormultiplizierter) Deckkraft, w*h*4
/// Byte. Wird NICHT auf dem Hauptfaden gerufen.
typedef void (*qc_zeiger_cb)(uint16_t w, uint16_t h, uint16_t hx, uint16_t hy,
                             int sichtbar, const uint8_t *rgba);
/// Eine Zeile fuer das Protokoll des Hosts.
typedef void (*qc_zeiger_log)(const char *text);

/// Abfrage starten: alle 50 ms auf der Hauptwarteschlange. `aktiv` sagt, ob
/// gerade jemand zuschaut - sonst wird nichts abgefragt und nichts gerechnet.
void qc_zeiger_start(qc_zeiger_cb cb, int (*aktiv)(void), qc_zeiger_log log);

/// Beim naechsten Durchlauf die Form auf jeden Fall schicken - ein neuer
/// Zuschauer kennt sie noch nicht.
void qc_zeiger_neu_senden(void);

/// Massstab: Bildpunkte des Stroms je Punkt des gestreamten Bildschirms.
/// Der Zeiger des Macs hat seine Groesse in Punkten; der Host streamt nativ
/// (bei HiDPI "wie 1920x1080" auf 4K: 3840x2160 Bildpunkte fuer 1920 Punkte,
/// Massstab 2). Damit er im Bild so gross erscheint wie auf dem Mac, wird er
/// in diesem Massstab gezeichnet - bei HiDPI aus der 2x-Darstellung, also
/// scharf. Vorgabe 1; gesetzt mit jedem Strom (main.m), aus jedem Faden.
/// Eine Aenderung schickt die Form beim naechsten Durchlauf neu.
void qc_zeiger_massstab_setzen(double massstab);
double qc_zeiger_massstab(void);

#endif
