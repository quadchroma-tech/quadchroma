// Oberflaeche des Mac-Hosts (Spezifikation Pairing 7.1-7.6): Symbol in der
// Menueleiste mit Menue, Zulassen-Fenster, Passwort-Fenster, Rueckfrage
// "Alle Geraete entfernen", Beenden samt Abschied an den Zuschauer.
//
// Faeden: alles, was AppKit anfasst, laeuft auf dem Hauptfaden. Kernfunktionen
// (zugang.h) ruft die Oberflaeche nur auf ihrer eigenen seriellen
// Warteschlange - nie auf der Main Queue, denn sie lesen und schreiben
// Dateien und nehmen Sperren des Kerns. Kein runModal, kein dispatch_sync auf
// die Main Queue: ein Main-Queue-Block, der steht, hielte Zeigerform,
// Zwischenablage und Einstellungen des Zuschauers mit an.
//
// Der Kern meldet sich ueber die qc_ui_*-Rueckrufe aus zugang.h (hier
// umgesetzt); sie kommen aus Netzfaeden und kehren sofort zurueck.
#import <AppKit/AppKit.h>
#include "zugang.h"

typedef struct {
    // Vor dem Beenden: der Zuschauer bekommt seine letzte Nachricht. Laeuft
    // nie auf der Main Queue (nimmt g_send_mtx); darf kurz blockieren.
    void (*abschied)(void);
    // Eine Protokollzeile (deutsch, ASCII-Umschrift wie das uebrige Protokoll).
    void (*protokoll)(NSString *zeile);
} qc_oberflaeche_cfg;

// Aus main() auf dem Hauptfaden, vor [NSApp run]: Delegate, Signale
// (SIGTERM/SIGINT beenden sauber), Warteschlange. Symbol und Menue entstehen
// in applicationDidFinishLaunching.
void qc_oberflaeche_starten(const qc_oberflaeche_cfg *cfg);

// Der Bildport ist von einem anderen Programm belegt (port) oder wieder frei
// (0). Aus jedem Faden.
void qc_oberflaeche_port_belegt(int port);

// ------------------------------------------------------------------ Modell
// Das Menue entsteht in zwei Schritten: ein Zustand (vom Kern gelesen), daraus
// ein Modell (Titel, Aktion, Haken, Untermenues) und erst daraus das NSMenu.
// Zustand -> Modell ist eine reine Funktion; der Pruefstand menuetest prueft
// sie ohne Anzeige.

typedef NS_ENUM(NSInteger, QCAnmelden) {
    QCAnmeldenNichtInProgramme,   // App liegt nicht in /Applications: Eintrag gesperrt, Hinweis
    QCAnmeldenAus,
    QCAnmeldenAn,
    QCAnmeldenFreigabeNoetig,     // registriert, aber in den Systemeinstellungen nicht erlaubt
};

@interface QCMenueZustand : NSObject
@property (nonatomic) uint32_t eigeneId;
@property (nonatomic, copy) NSString *passwort;        // nil = Passwortdatei unlesbar
@property (nonatomic) int geraeteAnzahl;               // -1 = Liste beschaedigt
@property (nonatomic, copy) NSData *geraete;           // geraeteAnzahl mal qc_geraet
@property (nonatomic, copy) NSString *zuschauer;       // nil = niemand verbunden
@property (nonatomic) BOOL bildschirm, bedienung;      // Freigaben erteilt
@property (nonatomic) QCAnmelden anmelden;
@property (nonatomic) int portBelegt;                  // 0 = frei
@end

typedef NS_ENUM(NSInteger, QCAktion) {
    QCAktionKeine,
    QCAktionIdKopieren,
    QCAktionPasswortKopieren,
    QCAktionPasswortAendern,
    QCAktionPasswortZufall,
    QCAktionGeraetEntfernen,      // daten = oeffentlicher Schluessel (32 Byte)
    QCAktionAlleEntfernen,
    QCAktionListeZuruecksetzen,
    QCAktionAnmelden,
    QCAktionBildschirmFreigabe,
    QCAktionBedienungshilfen,
    QCAktionBeenden,
};

@interface QCMenuePunkt : NSObject
@property (nonatomic, copy) NSString *titel;           // nil = Trennlinie
@property (nonatomic) QCAktion aktion;
@property (nonatomic) BOOL aktiv, haken, gemischt, kopf;
@property (nonatomic, copy) NSString *taste;           // Tastenkuerzel (mit Cmd) oder nil
@property (nonatomic, copy) NSData *daten;
@property (nonatomic, copy) NSArray<QCMenuePunkt *> *unter;
@end

// Fragt den Kern (und SMAppService). Nicht auf der Main Queue rufen.
QCMenueZustand *qc_menue_zustand_lesen(void);
// Das Menue fuer einen Zustand, in der aktuellen Sprache (texte.h).
NSArray<QCMenuePunkt *> *qc_menue_modell(QCMenueZustand *z);
// Flache Titelliste fuer Pruefungen: "---" Trennlinie, "# " Kopf, "(...)"
// gesperrt, "[x] " Haken, "[-] " halber Haken, je Untermenue-Ebene zwei
// Leerzeichen Einrueckung.
NSArray<NSString *> *qc_menue_titel(NSArray<QCMenuePunkt *> *modell);
// NSMenu aus dem Modell; die Punkte schicken menueAktion: an ziel.
NSMenu *qc_menue_bauen(NSArray<QCMenuePunkt *> *modell, id ziel);

// "ddd ddd ddd" mit fuehrenden Nullen (5 -> "000 000 005").
NSString *qc_id_text(uint32_t nummer);
// Vergleichscode "ddd ddd" (628306 -> "628 306").
NSString *qc_code_text(uint32_t code);
// Datum aus der Geraeteliste ("2026-09-26") in der Schreibweise der Sprache;
// unlesbar -> unveraendert.
NSString *qc_datum_text(NSString *iso);
// Pruefung im Passwort-Fenster: 0 gut, 1 ungleich, 2 zu kurz (norm(pw)
// unter 8 Byte, norm wie Abschnitt 3.4: ohne Leerzeichen, Tab, CR, LF, '-').
int qc_passwort_pruefen(NSString *pw, NSString *wiederholt);
// Vorlagenbild fuer die Menueleiste, 18 pt: die vier Felder des Logos, das
// vierte hohl (unterscheidet den Host vom Mac-Client), mit Punkt, solange
// jemand zuschaut.
NSImage *qc_menue_symbol(BOOL verbunden);

// ------------------------------------------------------- Zulassen-Anfragen
// Hoechstens eine Anfrage steht im Fenster; weitere warten in Ankunftsfolge.
// Die erste ist die gezeigte. Nur auf dem Hauptfaden.
@interface QCAnfrage : NSObject
@property (nonatomic) uint64_t nr;
@property (nonatomic, copy) NSString *name;
@property (nonatomic) uint32_t geraeteId, code;
@end

@interface QCAnfragen : NSObject
// YES = die neue ist jetzt die erste (anzuzeigen). Eine doppelte Nummer zaehlt nicht.
- (BOOL)neu:(QCAnfrage *)a;
// YES = es war die erste (ihr Fenster schliesst, die naechste kommt).
- (BOOL)weg:(uint64_t)nr;
- (QCAnfrage *)erste;
- (NSUInteger)anzahl;
@end
