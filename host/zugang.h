// Zugang: wie ein neues Geraet hereinkommt (Spezifikation "Zugang" v1).
//
// Jedes Geraet hat eine feste Geraete-ID aus seinem Schluessel. Bekannte
// Geraete stehen in host-devices.txt und bekommen nach dem Handschlag wie
// bisher "QCH1". Ein unbekanntes Geraet bekommt "QCA1" und die Zugangsphase
// (Nachrichten 20-23 auf dem Bildkanal): es beweist das Zugangspasswort des
// Hosts (host-password.txt), oder jemand am Host klickt "Zulassen". Danach
// ist es eingetragen.
//
// Hier steht alles, was ohne AppKit und ohne Netz geht und sich deshalb im
// Pruefstand zugangtest rechnen laesst: ID, Namen, Nachrichten, Kryptografie
// (CommonCrypto), Drossel, Grenzen der Zugangsphasen, Anfragen an die
// Oberflaeche, die beiden Dateien samt Migration aus authorized.txt, die
// Einzelinstanz. Den Ablauf auf der Leitung faehrt main.m.
//
// Alle Funktionen sind fadensicher. Jede Datei- und Listenfunktion nimmt ihre
// eigene kleine Sperre nur fuer den Augenblick des Lesens oder Schreibens;
// eine Zugangsphase wartet nie unter einer Sperre.
#ifndef QC_ZUGANG_H
#define QC_ZUGANG_H

#include <stddef.h>
#include <stdint.h>

// ---------------------------------------------------------------- Protokoll

#define QC_ZUGANG_KENNUNG      "QCA1"   // statt "QCH1": die Zugangsphase beginnt
#define QC_ZUGANG_NOETIG       20       // Host -> Client, Laenge 8 + n
#define QC_ZUGANG_BEWEIS       21       // Client -> Host, Laenge 32
#define QC_ZUGANG_ERGEBNIS     22       // Host -> Client, Laenge 8 oder 40
#define QC_ZUGANG_ABBRUCH      23       // Client -> Host, Laenge 0
#define QC_ZUGANG_FASSUNG      1
#define QC_ZUGANG_WEG_PASSWORT 0x01     // Bit 0: immer gesetzt
#define QC_ZUGANG_WEG_ZULASSEN 0x02     // Bit 1: am Host kann jemand "Zulassen" klicken

enum {
    QC_ERGEBNIS_PASSWORT   = 0,         // angenommen per Passwort, 32 Byte host_proof folgen
    QC_ERGEBNIS_ZUGELASSEN = 1,         // angenommen per Klick "Zulassen"
    QC_ERGEBNIS_FALSCH     = 2,         // Passwort falsch, erneut nach warten_ms
    QC_ERGEBNIS_ABGELEHNT  = 3,         // Klick "Ablehnen", die Leitung wird geschlossen
    QC_ERGEBNIS_SCHLUSS    = 4,         // zu viele Versuche oder Frist um, Leitung zu
};

#define QC_ZUGANG_KOPF         8        // u8 Typ, u8 Flags, u16 0, u32 Laenge (LE)
#define QC_ZUGANG_NAME_MAX     40       // Geraetenamen in Byte (UTF-8)
#define QC_ZUGANG_NOETIG_MAX   (QC_ZUGANG_KOPF + 8 + QC_ZUGANG_NAME_MAX)
#define QC_ZUGANG_ERGEBNIS_MAX (QC_ZUGANG_KOPF + 8 + 32)

// Ablauf und Grenzen (3.2).
#define QC_ZUGANG_FRIST_MS            120000  // Gesamtfrist einer Zugangsphase
#define QC_ZUGANG_NACHRICHT_MS        10000   // eine Nachricht des Clients, Kopf und Nutzlast zusammen
#define QC_ZUGANG_VERSUCHE            5       // Fehlversuche je Verbindung, dann Schluss
#define QC_ZUGANG_PHASEN              4       // gleichzeitig
#define QC_ZUGANG_PHASEN_JE_IP        2
#define QC_ZUGANG_PHASEN_JE_SCHLUESSEL 1
#define QC_ZUGANG_VOLL_WARTEN_MS      5000    // warten_ms fuer die, die keinen Platz bekommen

// Drossel (3.3).
#define QC_DROSSEL_ERSTE_MS      5000         // ab dem dritten Fehlversuch, dann verdoppelt
#define QC_DROSSEL_HOECHSTENS_MS 300000
#define QC_DROSSEL_VERFALL_MS    (15 * 60 * 1000)
#define QC_DROSSEL_GLOBAL_ANZAHL 30           // mehr als so viele Fehlversuche ...
#define QC_DROSSEL_GLOBAL_FENSTER_MS 60000    // ... in dieser Zeit ...
#define QC_DROSSEL_GLOBAL_WARTEN_MS  60000    // ... und jede weitere Phase wartet so lange

// Passwort (3.4, 4.3).
#define QC_ZUGANG_RUNDEN   100000       // PBKDF2-HMAC-SHA256
#define QC_ZUGANG_PW_MIN   8            // norm(pw) mindestens so viele Byte
#define QC_ZUGANG_PW_MAX   256          // Byte, wie eingegeben; Puffer also 257

// Bekanntgabe (2): an das Paket von heute wird angehaengt
//   u8 ext = 1 | u32 id LE | u8 flags
#define QC_BEKANNTGABE_MAX       128
#define QC_BEKANNTGABE_EXT       1
#define QC_BEKANNTGABE_ZULASSEN  0x01   // flags Bit 0: am Host kann jemand "Zulassen" klicken

// ------------------------------------------------------------ Geraete-ID (1.2)

/// d = SHA-256("QuadChroma/1 ID\0" || pub), id = u32 big endian aus d[0..4] mod 10^9.
uint32_t qc_zugang_id(const uint8_t pub[32]);

/// "ddd ddd ddd" mit fuehrenden Nullen (id 5 -> "000 000 005"). out: 12 Byte.
void qc_zugang_id_text(uint32_t id, char out[12]);

/// Vergleichscode als Zahl 0..999999 - derselbe Wert wie qc_sas (Anzeige "628 306").
uint32_t qc_zugang_code(const uint8_t hh[32]);

/// "ddd ddd". out: 8 Byte.
void qc_zugang_code_text(uint32_t code, char out[8]);

// ---------------------------------------------------------------- Namen (1.3)

/// Macht aus beliebigen Bytes einen anzeigbaren Namen: ungueltiges UTF-8 wird
/// zu U+FFFD, Steuerzeichen (U+0000-001F, U+007F-009F) und Richtungszeichen
/// (U+200E/F, U+202A-202E, U+2066-2069) fallen weg, Leerzeichen am Rand ebenso, danach an einer Zeichengrenze auf hoechstens max Byte
/// gekuerzt (nie mitten in einer UTF-8-Folge). out braucht max + 1 Byte.
/// Rueckgabe: Laenge ohne Nullbyte.
size_t qc_zugang_name_saeubern(const void *in, size_t n, char *out, size_t max);

/// Nutzlast von Handschlag-Nachricht 3: "QCN1" | u8 n | n Byte UTF-8 (n <= 40).
/// 1 = Name gelesen (gesaeubert, nicht leer), 0 = fehlt oder kaputt - dann ist
/// name leer und der Aufrufer nimmt die Adresse der Gegenstelle.
int qc_zugang_name_lesen(const uint8_t *nutzlast, size_t n, char name[QC_ZUGANG_NAME_MAX + 1]);

/// Gegenstueck fuer Clients und Pruefstaende. cap >= 45. Rueckgabe: Laenge.
size_t qc_zugang_name_kodieren(const char *name, uint8_t *out, size_t cap);

// ----------------------------------------------------------- Bekanntgabe (2)

/// "QCHB" | u8 1 | u16 port LE | u8 n | Name | u8 ext=1 | u32 id LE | u8 flags.
/// Der Name wird gesaeubert und auf 40 Byte gekuerzt. cap >= 54. Rueckgabe: Laenge.
size_t qc_zugang_bekanntgabe(uint8_t *pkt, size_t cap, uint16_t port, const char *name,
                             uint32_t id, uint8_t flags);

/// Liest ein Paket wie ein Client. 1 = mit Erweiterung (id, flags gesetzt),
/// 0 = altes Paket ohne Erweiterung, -1 = kein gueltiges Paket.
int qc_zugang_bekanntgabe_lesen(const uint8_t *p, size_t n, uint16_t *port,
                                char name[QC_ZUGANG_NAME_MAX + 1], uint32_t *id, uint8_t *flags);

// ----------------------------------------------------------- Nachrichten (3.2)
// Die ..._kodieren-Funktionen liefern die ganze Nachricht samt 8-Byte-Kopf,
// fertig zum Senden. Die ..._lesen-Funktionen nehmen nur die Nutzlast hinter
// dem Kopf (Typ und Laenge hat der Aufrufer schon gelesen).

/// 8-Byte-Kopf: u8 typ, u8 0, u16 0, u32 laenge LE.
void qc_zugang_kopf(uint8_t out[QC_ZUGANG_KOPF], uint8_t typ, uint32_t laenge);

/// 20 ZUGANG_NOETIG. cap >= QC_ZUGANG_NOETIG_MAX. Rueckgabe: Laenge.
size_t qc_zugang_noetig_kodieren(uint8_t *out, size_t cap, uint8_t wege, uint32_t warten_ms,
                                 const char *hostname);

/// 0 = gelesen, -1 = zu kurz, falsche Fassung oder Name zu lang.
int qc_zugang_noetig_lesen(const uint8_t *nutz, size_t n, uint8_t *wege, uint32_t *warten_ms,
                           char name[QC_ZUGANG_NAME_MAX + 1]);

/// 21 ZUGANG_BEWEIS (40 Byte).
size_t qc_zugang_beweis_kodieren(uint8_t out[QC_ZUGANG_KOPF + 32], const uint8_t client_proof[32]);

/// 22 ZUGANG_ERGEBNIS. host_proof nur bei QC_ERGEBNIS_PASSWORT (dann Pflicht).
/// cap >= QC_ZUGANG_ERGEBNIS_MAX. Rueckgabe: Laenge, 0 = ungueltig.
size_t qc_zugang_ergebnis_kodieren(uint8_t *out, size_t cap, uint8_t ergebnis, uint32_t warten_ms,
                                   const uint8_t *host_proof);

/// 0 = gelesen (proof nur bei ergebnis 0 gefuellt), -1 = falsche Laenge oder
/// unbekanntes Ergebnis.
int qc_zugang_ergebnis_lesen(const uint8_t *nutz, size_t n, uint8_t *ergebnis, uint32_t *warten_ms,
                             uint8_t proof[32]);

/// 23 ZUGANG_ABBRUCH (8 Byte).
size_t qc_zugang_abbruch_kodieren(uint8_t out[QC_ZUGANG_KOPF]);

// ---------------------------------------------------------- Kryptografie (3.4)

/// norm(pw): 0x20, 0x09, 0x0A, 0x0D und '-' fallen weg, A-Z wird a-z, sonst
/// unveraendert. out darf NULL sein (nur zaehlen). Rueckgabe: Laenge von
/// norm(pw) - auch wenn sie ueber cap liegt (dann ist out abgeschnitten).
size_t qc_zugang_norm(const char *pw, uint8_t *out, size_t cap);

/// HMAC-SHA256 (RFC 2104), beliebige Schluessellaenge.
void qc_zugang_hmac(const void *schluessel, size_t klen, const void *daten, size_t n, uint8_t out[32]);

/// K = PBKDF2-HMAC-SHA256(norm(pw), "QuadChroma/1 pw\0" || host_pub, 100000, 32).
/// 0 = gerechnet, -1 = Passwort zu lang oder CommonCrypto scheitert.
int qc_zugang_schluessel(const char *pw, const uint8_t host_pub[32], uint8_t k[32]);

/// client_proof (host = 0) = HMAC(K, "QuadChroma/1 auth client\0" || hh),
/// host_proof (host = 1)   = HMAC(K, "QuadChroma/1 auth host\0" || hh).
void qc_zugang_beweis(const uint8_t k[32], const uint8_t hh[32], int host, uint8_t out[32]);

/// Vergleich in konstanter Zeit. 1 = gleich.
int qc_zugang_gleich(const void *a, const void *b, size_t n);

/// Prueft einen client_proof gegen das aktuelle Zugangspasswort dieses Hosts
/// (qc_zugang_start). K wird fuer das aktuelle Passwort gemerkt; aendert es
/// sich, wird neu gerechnet. 1 = richtig (host_proof gefuellt), 0 = falsch,
/// -1 = kein lesbares Passwort (dann gibt es nur "Zulassen").
int qc_zugang_pruefen(const uint8_t hh[32], const uint8_t client_proof[32], uint8_t host_proof[32]);

// ------------------------------------------------------------- Drossel (3.3)
// Im Speicher, je Adresse und je Client-Schluessel getrennt; der groessere
// Wert gilt. Fehlversuch 1-2: 0 s, ab dem dritten 5 s, dann verdoppelt, bis
// 300 s. Ein richtiger Beweis setzt beide zurueck; ein Eintrag verfaellt
// 15 min nach seinem letzten Fehlversuch. Mehr als 30 Fehlversuche in 60 s
// (alle zusammen): jede weitere Abfrage bekommt mindestens 60 s.
// ip: IPv4-Adresse wie in sin_addr.s_addr. jetzt_ms: monotone Uhr.

/// Wie lange diese Gegenstelle noch warten muss, bevor ein Beweis zaehlt -
/// fuer den Beginn einer Phase (Nachricht 20), samt der globalen Grenze.
uint32_t qc_zugang_drossel_warten(uint32_t ip, const uint8_t pub[32], int64_t jetzt_ms);

/// Wie qc_zugang_drossel_warten, aber nur je Adresse und je Schluessel, ohne
/// die globale Grenze (die gilt fuer jede weitere Phase, nicht fuer jeden
/// Beweis darin). Fuer den Blick beim Eintreffen eines Beweises: eine zweite
/// Verbindung derselben Adresse darf nicht raten, waehrend die erste wartet.
uint32_t qc_zugang_drossel_rest(uint32_t ip, const uint8_t pub[32], int64_t jetzt_ms);

/// Ein Fehlversuch (auch: ein Beweis, der zu frueh kam). Rueckgabe: die
/// Wartezeit, die jetzt gilt.
uint32_t qc_zugang_drossel_fehler(uint32_t ip, const uint8_t pub[32], int64_t jetzt_ms);

/// Richtiger Beweis: beide Zaehler zurueck.
void qc_zugang_drossel_erfolg(uint32_t ip, const uint8_t pub[32]);

/// Alles vergessen (nur fuer Pruefstaende).
void qc_zugang_drossel_leeren(void);

// ------------------------------------------------- Zugangsphasen (Grenzen, 3.2)

/// Platz fuer eine Zugangsphase: hoechstens 4 gleichzeitig, 2 je Adresse, 1 je
/// Schluessel. Rueckgabe: Platznummer >= 0, oder -1 = kein Platz (dann
/// bekommt der Client sofort Ergebnis 4 mit QC_ZUGANG_VOLL_WARTEN_MS).
int qc_zugang_phase_beginnen(uint32_t ip, const uint8_t pub[32]);
void qc_zugang_phase_ende(int platz);
int qc_zugang_phasen_offen(void);

// ------------------------------------------------ Anfragen an die Oberflaeche
// Eine Zugangsphase mit "Zulassen" stellt eine Anfrage. Die Oberflaeche sieht
// hoechstens eine zugleich (die aelteste offene, qc_ui_anfrage); ist sie
// entschieden oder zurueckgezogen, bekommt die Oberflaeche qc_ui_anfrage_zurueck
// und danach die naechste. weck_fd: das Schreibende einer Pipe der Phase -
// jede Entscheidung schreibt ein Byte hinein, damit die Phase sofort aufwacht.

/// Rueckgabe: Nummer der Anfrage (> 0), 0 = keine Anfrage moeglich.
uint64_t qc_zugang_anfrage_stellen(const char *name, uint32_t id, uint32_t code, int weck_fd);

/// -1 = offen, 0 = abgelehnt, 1 = zugelassen, -2 = unbekannt.
int qc_zugang_anfrage_stand(uint64_t anfrage);

/// Die Phase ist zu Ende (egal wie): Anfrage austragen. Mehrfach harmlos.
void qc_zugang_anfrage_zurueckziehen(uint64_t anfrage);

// ----------------------------------------------------- Geraeteliste (4.1, 4.2)
// ~/Library/Application Support/QuadChroma/host-devices.txt, eine Zeile je
// Geraet: "<64 hex pub>  <YYYY-MM-DD>  <name>". Atomar geschrieben (neu +
// rename, 0600). Unlesbar oder beschaedigt: niemand ist bekannt, die Datei
// wird nicht ueberschrieben, bis qc_zugang_liste_zuruecksetzen sie beiseite
// legt (host-devices.txt.defekt-<zeit>).

typedef struct {
    uint8_t pub[32];
    uint32_t id;
    char name[QC_ZUGANG_NAME_MAX + 1];
    char datum[11];                     // "YYYY-MM-DD"
} qc_geraet;

/// 1 = eingetragen (name bekommt den gespeicherten Namen, darf NULL sein),
/// 0 = unbekannt, -1 = Liste unlesbar oder beschaedigt.
int qc_zugang_bekannt(const uint8_t pub[32], char name[QC_ZUGANG_NAME_MAX + 1]);

/// Eintragen mit dem heutigen Datum. 0 = steht drin (auch schon vorher),
/// -1 = Liste beschaedigt oder Schreibfehler.
int qc_zugang_eintragen(const uint8_t pub[32], const char *name);

/// Beim Start: authorized.txt -> host-devices.txt, wenn es die neue Liste noch
/// nicht gibt (danach authorized.txt.migriert). 1 = uebernommen, 0 = nichts zu
/// tun, -1 = gescheitert (alte Datei bleibt, naechster Start versucht es erneut).
int qc_zugang_migrieren(void);

/// Rueckruf fuer entfernte Geraete (pub, NULL = alle): main.m trennt eine
/// laufende Sitzung dieses Schluessels. Wird ausserhalb aller Sperren gerufen.
void qc_zugang_entfernt_setzen(void (*fn)(const uint8_t *pub));

// ------------------------------------------------------------ Einzelinstanz (7.5)

/// flock auf host-instanz.lock im Ablageordner, fuer die Laufzeit des
/// Prozesses. 1 = dieser Prozess ist der Host, 0 = es laeuft schon einer,
/// -1 = keine Sperre moeglich (dann laeuft der Host trotzdem).
int qc_zugang_einzelinstanz(void);

// ------------------------------------------------------------------ Start

/// Eigenen oeffentlichen Schluessel setzen, Migration, Passwort anlegen, wenn
/// es fehlt. Einmal beim Start, vor der Annahme.
void qc_zugang_start(const uint8_t host_pub[32]);

/// Protokollzeilen dieses Teils (Migration, Entfernen, Passwort ...). Ohne
/// Aufruf gehen sie nach stdout.
void qc_zugang_protokoll_setzen(void (*fn)(const char *zeile));

// ======================================================================
// Schnittstelle Kern <-> Oberflaeche (Spezifikation 13)
//
// Vom Kern bereitgestellt (zugang.c und main.m), von der Oberflaeche
// gerufen, fadensicher. Die Oberflaeche ruft sie nie auf der Main Queue mit
// Wartezeit (Dateien, Sperren) - besser auf einer eigenen Warteschlange.

uint32_t qc_zugang_eigene_id(void);
int qc_zugang_passwort(char *puffer, size_t groesse);   // 0 ok, -1 unlesbar (oder Puffer zu klein)
int qc_zugang_passwort_setzen(const char *pw);          // 0 ok, -1 zu kurz/ungueltig, -2 Schreibfehler
int qc_zugang_passwort_zufall(void);                    // 0 ok, -2 Schreibfehler
int qc_zugang_geraete(qc_geraet *liste, int max);       // Anzahl (alle, gefuellt bis max), -1 beschaedigt
int qc_zugang_geraet_entfernen(const uint8_t pub[32]);  // 0 ok (auch: war nicht drin), -1 Fehler
int qc_zugang_alle_entfernen(void);                     // 0 ok, -1 Fehler (auch: Liste beschaedigt)
int qc_zugang_liste_zuruecksetzen(void);                // bei beschaedigter Liste; 0 ok, -1 Fehler
void qc_zugang_entscheiden(uint64_t anfrage, int zulassen);

// In main.m (Zustand des laufenden Hosts):
int qc_zustand_zuschauer(char *name, size_t groesse);   // 1 verbunden (name gefuellt), 0 niemand
int qc_zustand_bildschirmfreigabe(void);                // 1 erteilt
int qc_zustand_bedienungshilfen(void);                  // 1 erteilt
int qc_zustand_port_belegt(void);                       // 0, oder der Bildport, den ein anderes Programm belegt

// Von der Oberflaeche bereitgestellt (P3), vom Kern gerufen. zugang.c
// liefert schwache Standardfassungen, damit Pruefstaende ohne Oberflaeche
// bauen. Sie werden aus Netzfaeden gerufen, die drei Meldungen auch unter
// einer Sperre des Kerns: qc_ui_anfrage und qc_ui_anfrage_zurueck unter der
// Sperre der Anfragen (damit ihre Reihenfolge stimmt), qc_ui_zustand_geaendert
// auch unter der Versandsperre von main.m (Zuschauer weg beim Senden). Also
// arbeiten die Meldungen nur per dispatch_async, rufen im Aufruf selbst nie
// zurueck in den Kern und warten nie synchron auf die Main Queue
// (dispatch_sync). Was die Oberflaeche danach vom Kern wissen will, holt sie
// spaeter aus dem eingereihten Block (wie oben: auf einer eigenen
// Warteschlange). qc_ui_vorhanden antwortet sofort, ohne zu warten.
// qc_ui_anfrage: name zeigt in die Tabelle der Anfragen und gilt nur waehrend
// des Aufrufs - wer ihn spaeter braucht, kopiert ihn vorher (etwa als
// NSString) in den Block.
void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code);
void qc_ui_anfrage_zurueck(uint64_t anfrage);           // entschieden oder zurueckgezogen: Fenster zu
void qc_ui_zustand_geaendert(void);                     // Zuschauer/Freigaben/Liste/Passwort -> Menue neu
int  qc_ui_vorhanden(void);                             // 1 = Zulassen moeglich (Bit 1 in 20 und Bekanntgabe)

#endif
