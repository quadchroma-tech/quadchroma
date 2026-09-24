// QuadChroma Host: Dateien ueber die Zwischenablage, Mac-Seite
// (Spezifikation "Dateien ueber die Zwischenablage", Abschnitt 2 und 3.6).
//
// Kopiert der Nutzer am Client Dateien, kommen sie ueber den Eingabekanal
// (Port 9002) hierher und liegen danach als Dateiliste in der Ablage dieses
// Macs. Kopiert er sie hier im Finder, gehen sie ueber den Bildkanal (Port
// 9001) zum Client. Uebertragen wird sofort beim Kopieren, nicht erst beim
// Einfuegen - mit Obergrenze und mit Nachrang hinter Bild, Ton und Eingaben.
//
// Nachrichten (Kopf wie ueberall: u8 Typ, u8 Flags, u16 frei, u32 Laenge;
// alle Zahlen little endian):
//   11 MSG_FAEHIGKEITEN  Host -> Client       u32 Bits, Bit 0 = Dateien Fassung 1
//   69 IN_FAEHIGKEITEN   Client -> Host       dasselbe
//   50 DATEI_ANGEBOT     Sender -> Empfaenger u32 Kennung, u32 Anzahl, u64 Gesamt, Eintraege
//   51 DATEI_STUECK      Sender -> Empfaenger u32 Kennung, u32 Eintrag, u64 Versatz, Daten
//   52 DATEI_ENDE        Sender -> Empfaenger u32 Kennung, u8 Grund
//   53 DATEI_QUITTUNG    Empfaenger -> Sender u32 Kennung, u8 Zustand, u8 0, u16 0, u64 empfangen
//
// Das Modul kennt weder Sockets noch die Sperren von main.m: gesendet wird
// ueber Rueckrufe (qc_dateien_wege), die main.m an die Sitzung bindet. So
// laeuft es auch im Pruefstand dateitest ohne den Rest des Hosts.
//
// Faeden: Der Empfaenger arbeitet auf der seriellen Warteschlange
// "dateien-empfang", der Sender auf "dateien-senden" - nie im Lesefaden eines
// Kanals, nie auf der Hauptwarteschlange und nie unter g_send_mtx. Beide
// laufen mit QOS_CLASS_UTILITY ("Latenz vor Bandbreite"): Aufnahme, Encoder
// und Ton gehen vor.
#ifndef QUADCHROMA_DATEIEN_H
#define QUADCHROMA_DATEIEN_H

#import <Foundation/Foundation.h>
#include <stddef.h>
#include <stdint.h>

// ------------------------------------------------------------ Nachrichten

#define QC_MSG_FAEHIGKEITEN  11
#define QC_IN_FAEHIGKEITEN   69
#define QC_DATEI_ANGEBOT     50
#define QC_DATEI_STUECK      51
#define QC_DATEI_ENDE        52
#define QC_DATEI_QUITTUNG    53

#define QC_FAEHIG_DATEIEN    1u          // Bit 0: Dateien Fassung 1

// ---------------------------------------------------------------- Grenzen

#define QC_DATEI_EINTRAEGE_MAX    10000u
#define QC_DATEI_GESAMT_MAX       4294967296ull          // 4 GiB
#define QC_DATEI_PFAD_MAX         1024u
#define QC_DATEI_BESTANDTEIL_MAX  255u
#define QC_DATEI_TIEFE_MAX        32u
#define QC_DATEI_ANGEBOT_MAX      1048576u               // ganze Nachricht
#define QC_DATEI_STUECK_MAX       49152u                 // Datenbytes je Stueck
#define QC_DATEI_STUECK_KOPF      16u
#define QC_DATEI_FENSTER          262144u                // unquittiert unterwegs
#define QC_DATEI_FENSTER_SPIEL    65536u                 // dasselbe im Spielmodus
#define QC_DATEI_QUITTUNG_ALLE    65536u                 // Empfaenger quittiert spaetestens je 64 KiB
#define QC_DATEI_STILLSTAND_MS    30000u
#define QC_DATEI_PLATZRESERVE     (1ull << 30)           // so viel bleibt nach der Uebertragung frei
#define QC_DATEI_RUECKSTAND_MAX   (128u * 1024u)         // Sendepuffer, ab dem der Sender wartet
#define QC_DATEI_WARTEN_US        2000u                  // so lange wartet er dann je Blick

// Zustand einer Quittung. Alles ausser LAEUFT beendet die Uebertragung.
enum {
    QC_QUITT_LAEUFT       = 0,
    QC_QUITT_FERTIG       = 1,   // fertig und in die Ablage gelegt
    QC_QUITT_ZU_GROSS     = 2,   // zu gross oder zu viele Eintraege
    QC_QUITT_KEIN_PLATZ   = 3,
    QC_QUITT_UNGUELTIG    = 4,   // Pfad, Format, Reihenfolge
    QC_QUITT_SCHREIBFEHLER = 5,
    QC_QUITT_ABGEBROCHEN  = 6,   // beim Empfaenger, etwa Zeitueberschreitung
};

// Grund im DATEI_ENDE.
enum {
    QC_ENDE_VOLLSTAENDIG = 0,
    QC_ENDE_ABGEBROCHEN  = 1,    // neuer Inhalt, Sitzungsende, Nutzer
    QC_ENDE_LESEFEHLER   = 2,
    QC_ENDE_ZEIT         = 3,    // keine Quittung mehr
};

// Obergrenze je Nachricht auf dem Eingabekanal des Hosts (2.6): 50 = 1 MiB,
// 51 = 16 + 49152, 52/53/69 = 256. 0 heisst: kein Typ dieses Moduls.
size_t qc_datei_grenze(uint8_t typ);

// ------------------------------------------------------ Kodieren und Lesen

// Ein Eintrag des Angebots.
@interface QCDateiEintrag : NSObject
@property (nonatomic) uint8_t art;                    // 0 = Datei, 1 = Ordner
@property (nonatomic) uint64_t groesse;               // bei Ordnern 0
@property (nonatomic, copy) NSData *pfad;             // wie auf der Leitung: UTF-8, relativ, Trenner '/'
@property (nonatomic, copy) NSArray<NSString *> *teile;   // Empfaenger: Bestandteile nach der Bereinigung, in NFC
@property (nonatomic, copy) NSString *quelle;         // Sender: absoluter Pfad auf diesem Mac
// Sender: st_dev und st_ino, wie sie beim Auflisten gesehen wurden. Die
// geoeffnete Datei muss genau diese sein (fstat), sonst ENDE mit Grund 2.
@property (nonatomic) uint64_t geraet;
@property (nonatomic) uint64_t knoten;
+ (instancetype)art:(uint8_t)art groesse:(uint64_t)groesse pfad:(NSString *)pfad;
@end

NSData *qc_datei_faehigkeiten_kodieren(uint32_t bits);
// 0 = gelesen (mindestens 4 Byte, weitere werden uebergangen), -1 = zu kurz.
int qc_datei_faehigkeiten_lesen(const uint8_t *p, size_t n, uint32_t *bits);

// Kodiert, was man ihm gibt; Gesamt ist die Summe der Dateigroessen. Geprueft
// wird hier nichts (der Pruefstand baut damit auch feindliche Angebote).
// nil nur, wenn sich ein Pfad nicht in u16 fassen laesst.
NSData *qc_datei_angebot_kodieren(uint32_t kennung, NSArray<QCDateiEintrag *> *eintraege);

// Liest und prueft ein Angebot nach 2.3, 2.5 und 2.6, samt Bereinigung fuer
// macOS; die Bestandteile (teile) sind danach in NFC, so werden sie auch
// angelegt. Rueckgabe QC_QUITT_LAEUFT = in Ordnung, sonst der Zustand der
// Ablehnung (QC_QUITT_ZU_GROSS oder QC_QUITT_UNGUELTIG). *kennung ist gesetzt,
// sobald 4 Byte da sind; *grund beschreibt eine Ablehnung.
int qc_datei_angebot_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint64_t *gesamt,
                           NSArray<QCDateiEintrag *> **eintraege, NSString **grund);

NSData *qc_datei_stueck_kodieren(uint32_t kennung, uint32_t eintrag, uint64_t versatz,
                                 const void *daten, size_t n);
// 0 = gelesen; *daten zeigt in p. -1 = kuerzer als Kopf plus ein Datenbyte.
int qc_datei_stueck_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint32_t *eintrag,
                          uint64_t *versatz, const uint8_t **daten, size_t *dn);

NSData *qc_datei_ende_kodieren(uint32_t kennung, uint8_t grund);
// 0 = gelesen (mindestens 5 Byte), -1 = zu kurz.
int qc_datei_ende_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint8_t *grund);

NSData *qc_datei_quittung_kodieren(uint32_t kennung, uint8_t zustand, uint64_t empfangen);
// 0 = gelesen (genau 16 Byte), -1 = sonst.
int qc_datei_quittung_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint8_t *zustand,
                            uint64_t *empfangen);

// Bereinigung eines Bestandteils fuer macOS (2.5): Steuerzeichen U+0000 bis
// U+001F werden zu '_'. ':' bleibt.
NSString *qc_datei_bereinigen(NSString *bestandteil);

// Bereinigung eines Bestandteils fuer Windows (2.5), wie
// bestandteil_bereinigen(.., Regeln::Windows) im Rust-Kern: < > : " | ? * und
// U+0000 bis U+001F werden zu '_', Punkte und Leerzeichen am Ende je zu '_',
// Geraetenamen (CON, PRN, AUX, NUL, CONIN$, CONOUT$, COM0-9, LPT0-9, COM/LPT
// mit hochgestellter 1-3; auch mit Endung, in jeder Schreibweise) bekommen
// ein '_' davor. Der Sender nimmt sie fuer seinen Schluessel gegen Doppelte:
// Er kennt das System der Gegenseite nicht und muss so streng sein wie der
// strengste Empfaenger. Schliesst die macOS-Bereinigung ein.
NSString *qc_datei_bereinigen_windows(NSString *bestandteil);

// ---------------------------------------------------------------- Betrieb

// Was das Modul von main.m braucht. Alle Rueckrufe duerfen blockieren; keiner
// wird unter einer Sperre dieses Moduls gerufen.
typedef struct {
    // Nachricht an den Zuschauer der Sitzung `sitzung` (Bildkanal). 1 =
    // gesendet, 0 = diese Sitzung ist vorbei, es ging nichts hinaus.
    int (*senden)(uint64_t sitzung, uint8_t typ, const void *daten, size_t laenge);
    // Ungesendete Bytes im Sendepuffer dieser Sitzung, -1 = Sitzung vorbei.
    int (*rueckstand)(uint64_t sitzung);
    // Spielmodus an? Dann gilt das kleinere Fenster.
    int (*spielmodus)(void);
    // Eine Protokollzeile ohne Zeilenende. NULL = nur nach stdout.
    void (*protokoll)(const char *zeile);
} qc_dateien_wege;

// Einmal beim Start: legt die beiden Warteschlangen an und merkt sich die Wege.
void qc_dateien_einrichten(const qc_dateien_wege *wege);

// "Fertig, in die Ablage legen": bekommt die obersten Eintraege einer
// vollstaendigen Uebertragung als absolute Pfade. Die Produktion setzt
// qc_clip_set_dateien, der Pruefstand einen Rekorder. NULL = nur Protokoll.
void qc_dateien_fertig_setzen(void (*fertig)(NSArray<NSString *> *pfade));

// Ablagebasis: NSTemporaryDirectory()/QuadChroma-Host-Ablage, Rechte 0700.
// ABWEICHUNG von 2.9 (dort .../QuadChroma-Ablage): der Mac-Client (Rust)
// nimmt $TMPDIR/QuadChroma-Ablage, und das ist unter macOS derselbe Ordner.
// Laufen Client und Host unter demselben Nutzer, raeumten sie einander sonst
// Uebertragungen weg. Mit eigener Basis raeumt keiner dem anderen etwas auf.
// Je Uebertragung <basis>/<unix-ms>-<kennung>; solange sie laeuft, liegt
// daneben die leere Marke <unix-ms>-<kennung>.laeuft (wie im Rust-Kern). Sie
// verschwindet mit dem Ende der Uebertragung, bei einem vollstaendigen
// Empfang ebenso wie bei einem Abbruch.
// Der Pruefstand setzt einen eigenen Ordner unter $TMPDIR (nil = wieder die Vorgabe).
#define QC_DATEI_BASIS_NAME "QuadChroma-Host-Ablage"
#define QC_DATEI_MARKE_ENDUNG ".laeuft"
void qc_dateien_basis_setzen(NSString *basis);
NSString *qc_dateien_basis(void);

// Nur Pruefstaende: Stillstand (Vorgabe 30 s) und Platzreserve (Vorgabe 1 GiB).
void qc_dateien_stillstand_setzen(uint32_t ms);
void qc_dateien_platzreserve_setzen(uint64_t bytes);

// Aufraeumen (2.9) in der Basis: die `behalten` neuesten Uebertragungen bleiben
// (0 = keine Obergrenze), aelter als hoechstalter_ms (0 = egal) wird geloescht.
// Nur Unterverzeichnisse "<unix-ms>-<kennung>" (und ihre Marken), nie einer
// Verknuepfung folgend: Die Basis selbst wird mit O_NOFOLLOW geoeffnet und
// alles relativ zu ihrem Deskriptor geloescht - ist sie eine Verknuepfung,
// bleibt alles unberuehrt. Laufende (mit Marke) werden weder gezaehlt noch
// geloescht, ausser nach hoechstalter_ms. Nach einem vollstaendigen Empfang
// bleibt das eben abgelegte Verzeichnis auf jeden Fall (es zaehlt als eines
// der drei), auch wenn die Uhr zurueckgesprungen ist. Laeuft im Faden des
// Aufrufers.
void qc_dateien_aufraeumen(NSUInteger behalten, uint64_t hoechstalter_ms);
// Beim Programmstart, auf "dateien-empfang": aelter als 24 h loeschen und
// dazu jede unfertige Uebertragung (mit Marke) gleich welchen Alters samt
// Marke - beim Start des Dienstes laeuft noch keine (ein Host je Nutzer, mit
// eigener Basis), eine Marke stammt also aus einem Lauf, der mitten im
// Empfang endete. Nur die Uebertragung, die gerade hier laeuft, bleibt.
void qc_dateien_aufraeumen_beim_start(void);

// Sitzung vorbei (Zuschauerwechsel, Zuschauer weg): laufende Uebertragungen
// dieser und aelterer Sitzungen werden verworfen, ihr Verzeichnis geloescht.
// neue_sitzung ist der Stand danach. Blockiert nie - darf unter g_send_mtx
// (nimmt nur kurz die Sperre des Eingangs, unter der nichts anderes laeuft).
// Verarbeitet wird hinter allem, was vorher eingereiht wurde.
void qc_dateien_sitzung_vorbei(uint64_t neue_sitzung);

// --- Empfaenger (Client -> Host)
// Eine Nachricht 50, 51 oder 52 vom Eingabekanal `kanal` der Sitzung
// `sitzung`. Reiht nur ein und kehrt sofort zurueck. Die Warteschlange ist
// wie im Rust-Kern begrenzt: Datenbytes der Stuecke (je Stueck mindestens 1)
// bis FENSTER + STUECK_MAX, hoechstens 8 Enden, ein neues Angebot leert sie
// (nur, was aus seiner oder einer aelteren Sitzung wartet - ein verspaetetes
// Angebot einer vergangenen Sitzung verwirft nichts aus der aktuellen);
// darueber Quittung 4. Beim Annehmen eines Angebots steht im Protokoll
// "Dateien: empfange N Eintraege, X MB". Namen werden in NFC angelegt, so
// wie sie nach der Bereinigung sind (nicht zerlegt wie fileSystemRepresentation). Quittiert wird nach der Annahme (0/0), sobald mehr als
// 16 KiB offen sind, sobald nichts mehr wartet und am Ende. Quittungen gehen
// ueber wege.senden, das blockiert, bis sie hinaus sind - bei Stau geht also
// keine verloren; nur mit dem Ende der Sitzung faellt sie weg.
void qc_empfang_nachricht(uint64_t sitzung, uint64_t kanal, uint8_t typ, NSData *nutzlast);
// Der Eingabekanal ist weg: eine Uebertragung, die ueber ihn kam, endet -
// aber erst hinter allem, was vorher von ihm kam (ein Ende davor zaehlt).
void qc_empfang_kanal_weg(uint64_t sitzung, uint64_t kanal);

// --- Sender (Host -> Client)
// Beginnt eine Sendung an den Zuschauer der Sitzung `sitzung` (an: Adresse
// fuers Protokoll) und bricht eine laufende ab. Kehrt sofort zurueck.
// Doppelte Namen (nach der Windows-Bereinigung, ohne Ruecksicht auf Gross-
// und Kleinschreibung und Normalform) gehen nur einmal hinaus, mit
// Protokollzeile. Ordner werden ueber einen Deskriptor gelesen, der
// nachweislich (st_dev/st_ino) der eben per lstat gesehene Ordner ist, und
// ihre Eintraege relativ dazu (fstatat) - ein waehrenddessen getauschter
// Ordner geht leer hinaus. Gelesen wird jede Datei mit O_NOFOLLOW|O_NONBLOCK
// und erst nach fstat blockierend: Ist sie keine gewoehnliche Datei mehr
// (FIFO, Verknuepfung) oder nicht mehr die beim Auflisten gesehene (st_dev/
// st_ino - etwa weil ein Ordner darueber gegen eine Verknuepfung getauscht
// wurde, die O_NOFOLLOW nicht abfaengt), gibt es ENDE mit Grund 2.
void qc_senden_starten(uint64_t sitzung, NSArray<NSString *> *pfade, NSString *an);
// Eine Quittung (53) vom Eingabekanal. Blockiert nicht. Nur Fortschritt
// (mehr quittiert als bisher, oder die erste Quittung nach dem Angebot)
// erneuert die Stillstandsfrist - wie im Rust-Sender; eine Wiederholung
// desselben Standes haelt den Sender nicht am Leben.
void qc_senden_quittung(uint64_t sitzung, const uint8_t *p, size_t n);
// Neuer Inhalt in der Ablage: eine laufende Sendung endet (ENDE mit Grund 1).
void qc_senden_abbrechen(void);

// --- Pruefstaende
void qc_dateien_abwarten(void);             // beide Warteschlangen und der Eingang leer
NSString *qc_empfang_ordner(void);          // Verzeichnis der laufenden Uebertragung, sonst nil
int qc_senden_laeuft(void);                 // 1 = eine Sendung ist unterwegs
// 1 = "dateien-empfang" anhalten (erst nach qc_dateien_abwarten), 0 = weiter.
// Was derweil kommt, wartet im Eingang - so laesst sich pruefen, wann der
// Empfaenger quittiert. Vor qc_dateien_abwarten wieder freigeben.
void qc_empfang_anhalten(int an);

#endif // QUADCHROMA_DATEIEN_H
