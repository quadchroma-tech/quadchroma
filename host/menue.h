// Oberflaeche des Mac-Hosts (Spezifikation Pairing 7.1-7.6): Symbol in der
// Menueleiste mit Menue, Zulassen-Fenster, Passwort-Fenster, Rueckfrage
// "Alle Geraete entfernen", Beenden (Verbindung zum Zuschauer vorher zu).
// In der einen App (eingebettet mit Rueckruf app, Plan M4) ist es das EINE
// Symbol von Client und Host: dazu "QuadChroma oeffnen", "Verbinden: <Host>",
// "Geraetename aendern ..." (eigenes Fenster), "Diesen Mac freigeben" und
// "Ruhezustand verhindern" - was der Client entscheidet, geht ueber app an
// ihn; das Programmmenue (Beenden) und das verborgene Bearbeiten-Menue baut
// diese Datei, weil winit ohne sein Standardmenue startet.
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
#include "dienst.h"
#import "texte.h"

typedef struct {
    // Vor dem Beenden: Bild- und Eingabekanal des Zuschauers schliessen.
    // Laeuft nie auf der Main Queue (nimmt g_send_mtx); darf kurz blockieren.
    void (*abschied)(void);
    // Eine Protokollzeile (deutsch, ASCII-Umschrift wie das uebrige Protokoll).
    void (*protokoll)(NSString *zeile);
    // 0: eigene App, die Oberflaeche wird NSApp.delegate. 1: im Rust-Client -
    // NSApp.delegate gehoert winit und bleibt, wie es ist (siehe menue.m).
    int eingebettet;
    // Nur eingebettet: Doppelklick auf die laufende App. NULL = Menue zeigen.
    void (*oeffnen)(void);
    // Die eine App: Punkte des Clients (QC_APP_* in dienst.h); NULL = nur Host.
    void (*app)(int was, const char *wert);
    // Die eine App: Pruefung des Geraetenamens (dienst.h); NULL = alles gut.
    int (*name_pruefen)(const char *eingabe);
} qc_oberflaeche_cfg;

// Aus qc_dienst_starten auf dem Hauptfaden, bevor die Run-Loop die
// Oberflaeche braucht: Delegate (eigene App) bzw. Doppelklick und Beenden an
// winits Delegate (eingebettet), Signale (SIGTERM/SIGINT beenden sauber),
// Warteschlange. Symbol und Menue entstehen erst mit qc_oberflaeche_fertig
// (dienst.h) - die eigene App ruft es aus applicationDidFinishLaunching:,
// eingebettet der Client aus resumed. Ein zweiter Aufruf bleibt ohne Wirkung.
void qc_oberflaeche_starten(const qc_oberflaeche_cfg *cfg);

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
// Nur die eine App (app = YES): das Menue traegt auch die Punkte des Clients.
@property (nonatomic) BOOL app;
@property (nonatomic, copy) NSString *geraetename;     // Kopfzeile "QuadChroma – <Name>"
@property (nonatomic) BOOL freigabe;                   // Haken "Diesen Mac freigeben" (Wunsch des Clients)
@property (nonatomic) BOOL ruhe;                       // Haken "Ruhezustand verhindern" (was gilt)
@property (nonatomic, copy) NSString *ruheGrund;       // gewuenscht, aber abgelehnt: Grund; nil = keiner
@property (nonatomic, copy) NSArray<NSArray<NSString *> *> *hosts;   // je @[Name, Adresse], hoechstens 4
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
    // Nur die eine App:
    QCAktionOeffnen,              // Fenster des Clients
    QCAktionVerbinden,            // daten = Adresse (UTF-8)
    QCAktionNameAendern,          // Fenster "Geraetename"
    QCAktionFreigabe,             // Haken "Diesen Mac freigeben"
    QCAktionRuhe,                 // Haken "Ruhezustand verhindern"
    QCAktionFensterSchliessen,    // Programmmenue (Cmd+Q), solange das Symbol steht
};

@interface QCMenuePunkt : NSObject
@property (nonatomic, copy) NSString *titel;           // nil = Trennlinie
@property (nonatomic) QCAktion aktion;
@property (nonatomic) BOOL aktiv, haken, gemischt, kopf;
@property (nonatomic, copy) NSString *taste;           // Tastenkuerzel (mit Cmd) oder nil
@property (nonatomic, copy) NSData *daten;
@property (nonatomic, copy) NSArray<QCMenuePunkt *> *unter;
@end

// Fragt den Kern (und SMAppService); in der einen App dazu den Stand des
// Clients (qc_app_stand_setzen). Nicht auf der Main Queue rufen.
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
// unter 8 Byte, norm wie Abschnitt 3.4: ohne Leerzeichen, Tab, CR, LF, '-'),
// 3 unzulaessig (ueber QC_ZUGANG_PW_MAX Byte UTF-8 oder mit Zeilenumbruch -
// so nimmt der Kern es nicht an).
int qc_passwort_pruefen(NSString *pw, NSString *wiederholt);
// Pruefung im Fenster "Geraetename" ueber pruefen (NULL: alles gut): der
// Text, der unter dem Feld steht - QCTextDeviceNameTooLong bzw.
// QCTextDeviceNameInvalid -, oder QCTextAnzahl, wenn der Name gut ist.
QCText qc_geraetename_fehler(NSString *eingabe, int (*pruefen)(const char *));
// Das Hauptmenue der einen App (winit startet ohne sein Standardmenue): im
// Programmmenue "QuadChroma beenden" (Cmd+Q, Aktion menueAktion: an ziel mit
// QCAktionBeenden) und verborgen "Ausblenden" (Cmd+H; verborgen heisst: nicht
// zu sehen, die Taste gilt trotzdem - allowsKeyEquivalentWhenHidden). Dazu
// die Punkte des Bearbeiten-Menues (Cmd+Z, Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V,
// Cmd+A an den Ersthelfer) in bearbeiten, nicht im Baum des Hauptmenues: sie
// gelten nur, wenn ein Textfeld den Fokus hat (qc_bearbeiten_taste), damit
// Einfuegen und Kopieren in den Feldern der Fenster dieser Datei wirken. Im
// Fenster des Clients kommen die Tasten so bei winit an - Cmd+V im
// Adressfeld, in einer Sitzung alle an den Rechner drueben. Ein Punkt im
// Baum schluckte seine Taste auch dort: performKeyEquivalent: liefert YES,
// auch wenn der Punkt ohne Ziel gesperrt ist. Kein Cmd+W: in der Sitzung
// gehoert es dem Mac drueben. sitzung: Cmd+Q und Cmd+H ebenso.
// schliessen (nicht leer, schon uebersetzt): statt "QuadChroma beenden"
// dieser Punkt mit Cmd+Q und QCAktionFensterSchliessen - solange das Symbol
// steht, schliesst Cmd+Q nur das Fenster; beendet wird im Menue der
// Menueleiste (qc_app_stand.schliessen_titel).
@interface QCHauptmenue : NSMenu
@property(nonatomic, strong, readonly) NSMenu *bearbeiten;
@end
QCHauptmenue *qc_programmmenue_bauen(id ziel, BOOL sitzung, NSString *schliessen);
// Der Teil von performKeyEquivalent: des Hauptmenues fuer die Bearbeiten-
// Tasten: YES, wenn ersthelfer ein Textfeld ist (Feldeditor, NSText) und
// bearbeiten die Taste nimmt; sonst NO - dann gilt nur das Programmmenue.
// Das Hauptmenue fragt mit dem Ersthelfer des Schluesselfensters.
BOOL qc_bearbeiten_taste(NSMenu *haupt, NSEvent *e, NSResponder *ersthelfer);

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
