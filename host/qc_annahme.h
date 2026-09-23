// Annahme fuer Bild- und Eingabeport.
//
// Wer anklopft, ist bis zum Ende des Handschlags unbekannt, und der Handschlag
// kann dauern (hoechstens QC_HANDSCHLAG_MS). Deshalb nimmt ein Faden nur an und
// gibt jede Verbindung sofort an einen eigenen Faden weiter: eine Gegenstelle,
// die schweigt oder tropfenweise sendet, haelt so nur sich selbst auf und nie
// den naechsten Zuschauer. Damit daraus kein Faden-Stapel ohne Ende wird, gibt
// es Plaetze - hoechstens max_gesamt Verbindungen gleichzeitig im Handschlag,
// davon hoechstens max_je_ip von derselben Adresse. Wer keinen Platz bekommt,
// wird sofort geschlossen.
#ifndef QC_ANNAHME_H
#define QC_ANNAHME_H

#include <netinet/in.h>

typedef struct qc_platz qc_platz;

// Laeuft im eigenen Faden der Verbindung und gehoert ihr ganz: fn schliesst fd
// selbst. Sobald der Handschlag vorbei ist, gibt fn den Platz mit qc_platz_frei
// zurueck; spaetestens nach dem Ende von fn geschieht das von selbst.
typedef void (*qc_verbindung_fn)(qc_platz *platz, int fd, const struct sockaddr_in *peer, void *ctx);

// Kein Platz frei: nur zur Meldung, die Verbindung ist schon zu. Kommt
// hoechstens alle zehn Sekunden; anzahl zaehlt alle seit der letzten Meldung.
typedef void (*qc_abgewiesen_fn)(long anzahl, const struct sockaddr_in *peer, void *ctx);

typedef struct {
    int max_gesamt;                 // gleichzeitige Handschlaege an diesem Port
    int max_je_ip;                  // davon von derselben Adresse
    qc_verbindung_fn verbindung;
    qc_abgewiesen_fn abgewiesen;    // darf NULL sein
    void *ctx;
} qc_annahme_cfg;

/// Startet den Annahmefaden fuer einen lauschenden Socket. 0 = Erfolg.
int qc_annahme_starten(int listen_fd, const qc_annahme_cfg *cfg);

/// Handschlag vorbei: Platz zurueckgeben. Mehrfacher Aufruf ist harmlos.
void qc_platz_frei(qc_platz *p);

#endif
