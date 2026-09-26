// QuadChroma Host: Bildschirmwahl und intelligentes Umschalten, Mac-Seite
// (Spezifikation "Bildschirmwahl", Abschnitt 1.3, 1.5, 2 und 3.2).
//
// Der Host streamt genau einen Bildschirm. In der Automatik (Vorgabe) ist das
// der Hauptbildschirm, und der Host folgt ihm, wenn er wechselt; mit einem
// Wunsch des Zuschauers (Nachricht 70) ist es der gewuenschte Bildschirm,
// erkannt an seiner stabilen Kennung - faellt er weg, weicht der Host auf den
// Hauptbildschirm aus und kehrt von selbst zurueck, sobald er wieder da ist.
// Die Liste der Bildschirme geht als Nachricht 12 an den Zuschauer.
//
// Nachrichten (Kopf wie ueberall: u8 Typ, u8 Flags, u16 frei, u32 Laenge;
// alle Zahlen little endian):
//   12 MSG_BILDSCHIRME  Host -> Client
//        u8 fassung = 1, u8 anzahl (0..=16), u8 wunsch_laenge (0 = Automatik),
//        u8 frei, u8[wunsch_laenge] Kennung des Wunsches, dann je Eintrag:
//        u8 kennung_laenge (1..=64), u8 name_laenge (0..=48), u16 breite,
//        u16 hoehe, u16 hz (0 = unbekannt), u8 flags (Bit 0 Hauptbildschirm,
//        Bit 1 wird gestreamt), u8 frei, Kennung, Name (beide UTF-8)
//   70 IN_BILDSCHIRM    Client -> Host
//        u8 kennung_laenge (0 = Automatik), u8[kennung_laenge] Kennung
//
// Dieselben Bytes bildet client/src/bildschirm.rs fuer den Client und die
// Windows-Host-Rolle; massgeblich sind die Pruefvektoren (Spezifikation 2.4),
// die hosttest.m und die Rust-Tests beide erfuellen.
//
// Stabile Kennung (1.3): v<Vendor>-m<Model>-s<Serial> aus den CGDisplay-
// Nummern, bei zwei gleichen in der Liste mit -u<Unit> ergaenzt. Der Name ist
// NSScreen.localizedName des Bildschirms mit derselben displayID, sonst
// "Bildschirm <displayID>". Die Listenplaetze von ScreenCaptureKit sind nicht
// stabil, die displayID nur je Sitzung - beides taugt nicht als Schluessel.
//
// Das Modul kennt weder Warteschlangen noch Sperren von main.m: es liefert
// die Liste (ersetzbar, damit der Pruefstand ohne ScreenCaptureKit laeuft),
// trifft die Wahl als reine Funktion, kodiert und liest die Nachrichten und
// hebt den Wunsch in bildschirm.txt auf. Wer wann neu bewertet und wie der
// Strom wechselt, steht in main.m.
#ifndef QUADCHROMA_BILDSCHIRM_H
#define QUADCHROMA_BILDSCHIRM_H

#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stddef.h>
#include <stdint.h>

// ------------------------------------------------------------ Nachrichten

#define QC_MSG_BILDSCHIRME   12
#define QC_IN_BILDSCHIRM     70
#define QC_FAEHIG_BILDSCHIRM 2u          // Bit 1 in MSG_FAEHIGKEITEN: Bildschirmwahl

#define QC_BILDSCHIRM_FASSUNG        1
#define QC_BILDSCHIRM_KENNUNG_MAX    64u
#define QC_BILDSCHIRM_NAME_MAX       48u
#define QC_BILDSCHIRM_EINTRAEGE_MAX  16u
#define QC_BILDSCHIRM_FLAG_HAUPT     1u
#define QC_BILDSCHIRM_FLAG_GESTREAMT 2u
// Hoechstlaenge einer Liste: 4 + 64 + 16 * (10 + 64 + 48).
#define QC_BILDSCHIRM_LISTE_MAX      (4u + QC_BILDSCHIRM_KENNUNG_MAX + QC_BILDSCHIRM_EINTRAEGE_MAX * (10u + QC_BILDSCHIRM_KENNUNG_MAX + QC_BILDSCHIRM_NAME_MAX))

// Ein Bildschirm des Hosts. displayID ist die Kennung dieser Sitzung (fuer
// ScreenCaptureKit, CGDisplayBounds und die Protokollzeilen), kennung die
// stabile fuer Client und bildschirm.txt. w, h in Pixeln. sc ist der
// SCDisplay, aus dem der Strom gebaut wird - im Pruefstand nil.
@interface QCBildschirm : NSObject
@property (nonatomic) CGDirectDisplayID displayID;
@property (nonatomic, copy) NSString *kennung;
@property (nonatomic, copy) NSString *name;
@property (nonatomic) size_t w, h;
@property (nonatomic) double hz;
@property (nonatomic) BOOL haupt;
@property (nonatomic, strong) SCDisplay *sc;
+ (instancetype)kennung:(NSString *)kennung name:(NSString *)name displayID:(CGDirectDisplayID)displayID
                      w:(size_t)w h:(size_t)h hz:(double)hz haupt:(BOOL)haupt;
// Gleich, wenn Kennung, Name, Groesse, Hz, Hauptbildschirm und displayID
// gleich sind - so erkennt der Host, ob sich die Liste geaendert hat.
- (BOOL)isEqual:(id)other;
@end

// ------------------------------------------------------------------ Liste

// Lieferant der Liste. Die Produktion fragt ScreenCaptureKit (blockierend,
// bis zu 10 s - nur auf der Lebenslauf-Warteschlange oder vor den
// Warteschlangen rufen); der Pruefstand setzt eine Attrappe. nil = die
// Bildschirme liessen sich nicht abrufen (der Aufrufer behaelt seinen Stand).
typedef NSArray<QCBildschirm *> *(*qc_bildschirm_lieferant)(void);
void qc_bildschirm_liste_setzen(qc_bildschirm_lieferant lieferant);
NSArray<QCBildschirm *> *qc_bildschirme_holen(void);

// Namen der Bildschirme (NSScreen.localizedName) in einen Vorrat lesen. Nur
// auf dem Hauptfaden - AppKit gehoert dorthin -, beim Start und nach jeder
// Aenderung der Bildschirmkonfiguration; der Lieferant liest aus dem Vorrat.
void qc_bildschirm_namen_auffrischen(void);

// Stabile Kennung aus den drei Nummern; die Unit haengt der Lieferant nur
// bei Gleichheit an.
NSString *qc_bildschirm_kennung(uint32_t vendor, uint32_t model, uint32_t serial);

// ------------------------------------------------------------------- Wahl

enum {
    QC_WAHL_KEINER = 0,   // leere Liste
    QC_WAHL_WUNSCH = 1,   // der gewuenschte Bildschirm ist da
    QC_WAHL_HAUPT  = 2,   // Hauptbildschirm (Automatik oder Ausweichplatz)
    QC_WAHL_ERSTER = 3,   // kein Hauptbildschirm in der Liste: der erste
};

// Reine Wahl: Wunsch (an der Kennung), sonst Hauptbildschirm, sonst der
// erste, sonst nil. *grund sagt, was gezogen hat.
QCBildschirm *qc_bildschirm_wahl(NSArray<QCBildschirm *> *liste, NSString *wunsch, int *grund);

// ------------------------------------------------------ Kodieren und Lesen

// Kennung und Name, wie sie auf die Leitung duerfen: ohne Steuerzeichen,
// hoechstens 64 bzw. 48 Byte, gekuerzt an Zeichengrenzen. Eine leere Kennung
// wird zu "?".
NSString *qc_bildschirm_kennung_bereinigen(NSString *kennung);
NSString *qc_bildschirm_name_bereinigen(NSString *name);

// Die Liste als Nutzlast von Typ 12. wunsch nil = Automatik; gestreamt ist
// die displayID des gestreamten (oder gewaehlten) Bildschirms, 0 = keiner.
// Mehr als 16 Eintraege fallen weg.
NSData *qc_bildschirme_kodieren(NSArray<QCBildschirm *> *liste, NSString *wunsch, CGDirectDisplayID gestreamt);

// Liest die Nutzlast von Typ 70. 0 = gelesen (*kennung nil = Automatik),
// -1 = ungueltig (Laenge passt nicht, kein UTF-8, Steuerzeichen) - dann
// uebergeht der Host die Nachricht.
int qc_bildschirm_wunsch_lesen(const uint8_t *p, size_t n, NSString **kennung);

// Der Wunsch als Nutzlast von Typ 70 (fuer Pruefstaende).
NSData *qc_bildschirm_wunsch_kodieren(NSString *kennung);

// ------------------------------------------------------------- Persistenz

// bildschirm.txt im Ablageordner (~/Library/Application Support/QuadChroma):
// genau eine Zeile, "auto" oder die Kennung.
// Laden: 0 = gelesen (*wunsch nil = Automatik), 1 = Datei fehlt (Automatik),
// -1 = unlesbar oder kaputt (Automatik; die Datei bleibt, wie sie ist).
int qc_bildschirm_wunsch_laden(NSString **wunsch);
// Speichern (nil = "auto"): temporaere Datei, fsync, Umbenennen. 0 = geschrieben.
int qc_bildschirm_wunsch_speichern(NSString *wunsch);

#endif // QUADCHROMA_BILDSCHIRM_H
