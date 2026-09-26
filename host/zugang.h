// Schnittstelle Mac-Host: Kern (Zugang, Paket P2) <-> Oberflaeche (Menueleiste,
// Paket P3), wie Spezifikation Pairing Abschnitt 13 sie festlegt.
//
// Diese Fassung legt P3 an, damit die Oberflaeche baut, bevor P2 fertig ist;
// bei der Zusammenfuehrung gilt die von P2 (inhaltlich gleich).
#ifndef QC_ZUGANG_H
#define QC_ZUGANG_H

#include <stddef.h>
#include <stdint.h>

typedef struct { uint8_t pub[32]; uint32_t id; char name[41]; char datum[11]; } qc_geraet;

// vom Kern bereitgestellt (P2), von der UI aufgerufen (P3), fadensicher:
uint32_t    qc_zugang_eigene_id(void);
int         qc_zugang_passwort(char *puffer, size_t groesse);   // 0 ok, -1 unlesbar
int         qc_zugang_passwort_setzen(const char *pw);          // 0 ok, -1 zu kurz, -2 Schreibfehler
int         qc_zugang_passwort_zufall(void);                     // 0 ok
int         qc_zugang_geraete(qc_geraet *liste, int max);        // Anzahl, -1 beschaedigt
int         qc_zugang_geraet_entfernen(const uint8_t pub[32]);   // 0 ok
int         qc_zugang_alle_entfernen(void);
int         qc_zugang_liste_zuruecksetzen(void);                 // bei beschaedigter Liste
void        qc_zugang_entscheiden(uint64_t anfrage, int zulassen);
int         qc_zustand_zuschauer(char *name, size_t groesse);    // 1 verbunden
int         qc_zustand_bildschirmfreigabe(void), qc_zustand_bedienungshilfen(void);

// von der UI bereitgestellt (P3), vom Kern aufgerufen (P2) - P2 liefert schwache
// Standardfassungen (__attribute__((weak))), damit Pruefstaende ohne UI bauen:
void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code);
void qc_ui_anfrage_zurueck(uint64_t anfrage);
void qc_ui_zustand_geaendert(void);   // Zuschauer/Freigaben/Liste geaendert -> Menue neu
int  qc_ui_vorhanden(void);           // 1 = Zulassen moeglich (Bit 1 in 20 und Bekanntgabe)

// Die UI ruft Kernfunktionen nie auf der Main Queue mit Wartezeit; qc_ui_* werden aus
// Netzfaeden gerufen und muessen selbst per dispatch_async(main) arbeiten.

#endif
