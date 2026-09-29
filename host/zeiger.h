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
/// sichtbar (soll der Client einen Zeiger zeigen?), eingefangen (Merkmal
/// QC_ZEIGER_GEFANGEN), RGBA mit gerader (nicht vormultiplizierter)
/// Deckkraft, w*h*4 Byte. Wird NICHT auf dem Hauptfaden gerufen.
typedef void (*qc_zeiger_cb)(uint16_t w, uint16_t h, uint16_t hx, uint16_t hy,
                             int sichtbar, int gefangen, const uint8_t *rgba);
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

// ------------------------------------------------ Maus einfangen (Spiele)
//
// Dieselben Regeln wie maus.rs im Client (FangWaechter::schritt), hier fuer
// den Mac (FANG_MAC): eingefangen, wenn der Zeiger versteckt ist und der
// letzten absoluten Bewegung nicht folgt (die Anwendung hat ihn abgekoppelt -
// CGAssociateMouseAndMouseCursorPosition - oder setzt ihn zurueck).
// "Versteckt" allein zaehlt nicht: macOS versteckt den Zeiger beim Tippen
// und zeigt ihn erst wieder, wenn sich die echte Maus des Macs bewegt - nie
// durch eingespeiste Bewegung. Deshalb meldet der Mac "nicht zeigen" nur im
// Fang. Das Merkmal geht mit der Form hinaus (Byte 10 von Nachricht 49).

#define QC_FAEHIG_MAUS        8u   // Bit 3 in MSG_FAEHIGKEITEN: Sichtbarkeit verlaesslich, Merkmal, IN_MOVE_REL
#define QC_ZEIGER_GEFANGEN    1    // Byte 10 von Nachricht 49, Bit 0

#define QC_FANG_GRUND_EINGESPERRT 1
#define QC_FANG_GRUND_FOLGT_NICHT 2
#define QC_FANG_GRUND_BEWEGUNG    4
#define QC_FANG_TIPPEN_US       1000000ull
#define QC_FANG_BEWEGUNGEN_MIN  3ull
#define QC_FANG_VERSTECKT_MIN_US 150000ull
#define QC_FANG_AN_US           50000ull
#define QC_FANG_AUS_US          100000ull

// vollbild: das Fenster im Vordergrund deckt seinen Bildschirm (nur die
// Bewegungsregel fragt danach - der Mac hat sie nicht).
typedef struct { int versteckt, eingesperrt, folgt_nicht, vollbild; } qc_zeiger_lage;
typedef struct { uint64_t bewegungen, letzte_taste_us; } qc_eingabe_zaehler;
typedef struct { int tippen_endet_mit_bewegung, bewegung_regel, versteckt_nur_mit_fang; } qc_fang_art;
typedef struct { int sichtbar, gefangen; uint8_t grund; } qc_fang_urteil;
typedef struct {
    int versteckt; uint64_t versteckt_seit;
    uint64_t bewegungen_basis;
    int taste_vor_verstecken;
    int grund_offen; uint64_t grund_seit;
    int sichtbar_offen; uint64_t sichtbar_seit;
    int gefangen; uint8_t grund;
} qc_fang_waechter;

extern const qc_fang_art QC_FANG_MAC;
extern const qc_fang_art QC_FANG_WINDOWS;

/// Ein Durchgang des Waechters (rein, fuer den Pruefstand offen).
qc_fang_urteil qc_fang_schritt(qc_fang_waechter *w, uint64_t jetzt_us, qc_zeiger_lage lage,
                               qc_eingabe_zaehler ein, qc_fang_art art);
/// Folgt der Zeiger (pos) der letzten absoluten Bewegung nicht? Nur 30 ms
/// bis 2 s nach ihr (ziel_us, 0 = keine).
int qc_zeiger_folgt_nicht(uint64_t jetzt_us, double px, double py, double zx, double zy, uint64_t ziel_us,
                          double toleranz);

/// Von der Eingabe (main.m, unter g_inject_mtx): eine Bewegung wurde
/// eingespeist - absolut mit ihrem Ziel (globale Punkte), sonst relativ.
void qc_zeiger_eingabe_bewegung(int absolut, double x, double y);
/// Von der Eingabe: eine Taste wurde gedrueckt.
void qc_zeiger_eingabe_taste(void);

#endif
