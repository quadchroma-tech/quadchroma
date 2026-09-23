// Annahme fuer Bild- und Eingabeport.
//
// Wer anklopft, ist bis zum Ende des Handschlags unbekannt, und der Handschlag
// kann dauern (hoechstens QC_HANDSCHLAG_MS). Deshalb nimmt ein Faden nur an und
// gibt jede Verbindung sofort an einen eigenen Faden weiter: eine Gegenstelle,
// die schweigt oder tropfenweise sendet, haelt so nur sich selbst auf und nie
// den naechsten Zuschauer.
//
// Damit daraus kein Faden-Stapel ohne Ende wird, gibt es Plaetze: hoechstens
// max_gesamt Verbindungen gleichzeitig im Handschlag, davon hoechstens
// max_je_ip von derselben Adresse. Ist kein Platz frei, wird nicht der Neue
// abgewiesen, sondern der aelteste laufende Handschlag verdraengt - derselben
// Adresse, wenn diese schon genug hat, sonst der aelteste ueberhaupt. Wer die
// Plaetze mit stummen Verbindungen belegen will, muss sie damit schneller
// nachschieben, als ein echter Client fuer seinen Handschlag braucht
// (Millisekunden), statt alle fuenf Sekunden eine - und von einer Adresse aus
// kommt er ueber max_je_ip Plaetze ohnehin nicht hinaus.
//
// Verdraengt heisst: shutdown auf den Socket, der Faden kehrt aus seinem
// Warten zurueck. Erst wenn zu viele verdraengte Faeden noch nicht beendet
// sind (2 * max_gesamt Faeden dieser Annahme, laufende Sitzungen mitgezaehlt),
// wird der Neue sofort geschlossen.
#ifndef QC_ANNAHME_H
#define QC_ANNAHME_H

#include <netinet/in.h>

typedef struct qc_platz qc_platz;

// Hoechstens so oft (Sekunden) eine Andrang-Meldung. main.m drosselt seine
// Zeilen ueber ungepruefte Gegenstellen im selben Takt.
#define QC_MELDEN_S 10

// Laeuft im eigenen Faden der Verbindung und gehoert ihr ganz: fn schliesst fd
// selbst - aber erst nach qc_platz_frei. Bis dahin kann die Annahme den Socket
// per shutdown abbrechen, und eine Nummer, die nach close schon einer anderen
// Verbindung gehoert, darf sie nie treffen. Sobald der Handschlag vorbei ist,
// gibt fn den Platz zurueck; spaetestens nach dem Ende von fn geschieht das
// von selbst.
typedef void (*qc_verbindung_fn)(qc_platz *platz, int fd, const struct sockaddr_in *peer, void *ctx);

// Andrang, nur zur Meldung. verdraengt: abgebrochene Handschlaege, zuletzt der
// von verdraengt_von. abgewiesen: sofort geschlossene Verbindungen (zu viele
// Faeden), zuletzt von abgewiesen_von. Kommt hoechstens alle zehn Sekunden,
// zaehlt alles seit der letzten Meldung und bleibt auch dann nicht liegen,
// wenn danach niemand mehr anklopft.
typedef void (*qc_andrang_fn)(long verdraengt, const struct sockaddr_in *verdraengt_von,
                              long abgewiesen, const struct sockaddr_in *abgewiesen_von, void *ctx);

typedef struct {
    int max_gesamt;                 // gleichzeitige Handschlaege an diesem Port
    int max_je_ip;                  // davon von derselben Adresse
    qc_verbindung_fn verbindung;
    qc_andrang_fn andrang;          // darf NULL sein
    void *ctx;
} qc_annahme_cfg;

/// Startet den Annahmefaden fuer einen lauschenden Socket. 0 = Erfolg.
int qc_annahme_starten(int listen_fd, const qc_annahme_cfg *cfg);

/// Handschlag vorbei: Platz zurueckgeben. 1 = die Verbindung wurde inzwischen
/// verdraengt (der Socket ist abgebrochen, nicht weitermachen; gemeldet wird
/// das schon ueber andrang), sonst 0. Mehrfacher Aufruf ist harmlos.
int qc_platz_frei(qc_platz *p);

#endif
