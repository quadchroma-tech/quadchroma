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
// edr_potentiell und edr_aktuell: der EDR-Kopfraum des Bildschirms (NSScreen
// maximumPotential... bzw. maximumExtendedDynamicRangeColorComponentValue;
// 1.0 = SDR, 0 = unbekannt). Ueber 1.0 potentiell gilt der Bildschirm als
// HDR-faehig - dann darf der Host ihn als HDR aufnehmen (HDR-Plan 5.1).
// punkte_w, punkte_h: derselbe Modus in Punkten (bei HiDPI die Haelfte der
// Pixel: "wie 1920x1080" auf einem 4K-Panel sind 1920x1080 Punkte, 3840x2160
// Pixel); nativ_w, nativ_h: die Aufloesung des Panels - die Obergrenze fuer
// einen skalierten HiDPI-Modus (qc_bildschirm_modus_lesen). 0 = unbekannt
// (Attrappen, virtuelle Bildschirme ohne solche Modi).
@interface QCBildschirm : NSObject
@property (nonatomic) CGDirectDisplayID displayID;
@property (nonatomic, copy) NSString *kennung;
@property (nonatomic, copy) NSString *name;
@property (nonatomic) size_t w, h;
@property (nonatomic) size_t punkte_w, punkte_h;
@property (nonatomic) size_t nativ_w, nativ_h;
@property (nonatomic) double hz;
@property (nonatomic) BOOL haupt;
@property (nonatomic, strong) SCDisplay *sc;
@property (nonatomic) double edr_potentiell, edr_aktuell;
+ (instancetype)kennung:(NSString *)kennung name:(NSString *)name displayID:(CGDirectDisplayID)displayID
                      w:(size_t)w h:(size_t)h hz:(double)hz haupt:(BOOL)haupt;
// Gleich, wenn Kennung, Name, Groesse, Hz, Hauptbildschirm und displayID
// gleich sind - so erkennt der Host, ob sich die Liste geaendert hat. Der
// EDR-Kopfraum zaehlt nicht mit: er steht nicht in Nachricht 12, und sein
// Wechsel (HDR am Host an/aus) ist Sache der HDR-Entscheidung (main.m).
// Punkte und native Aufloesung ebenso wenig: Stromgroesse und Zeigermassstab
// prueft main.m bei jeder Bewertung selbst.
- (BOOL)isEqual:(id)other;
// Kann dieser Bildschirm HDR zeigen? Potentieller EDR-Kopfraum ueber 1.0.
- (BOOL)hdr;
@end

// ------------------------------------------------------------------ Liste

// Lieferant der Liste. Die Produktion fragt ScreenCaptureKit (blockierend,
// bis zu 10 s - nur auf der Lebenslauf-Warteschlange oder vor den
// Warteschlangen rufen); der Pruefstand setzt eine Attrappe. nil = die
// Bildschirme liessen sich nicht abrufen (der Aufrufer behaelt seinen Stand).
typedef NSArray<QCBildschirm *> *(*qc_bildschirm_lieferant)(void);
void qc_bildschirm_liste_setzen(qc_bildschirm_lieferant lieferant);
NSArray<QCBildschirm *> *qc_bildschirme_holen(void);

// Namen der Bildschirme (NSScreen.localizedName) und ihren EDR-Kopfraum in
// einen Vorrat lesen. Nur auf dem Hauptfaden - AppKit gehoert dorthin -,
// beim Start und nach jeder Aenderung der Bildschirmkonfiguration; der
// Lieferant liest aus dem Vorrat.
void qc_bildschirm_namen_auffrischen(void);

// Der EDR-Kopfraum aus dem Vorrat (potentiell, aktuell; 0 = unbekannt).
// Rueckgabe 1, wenn der Bildschirm im Vorrat steht.
int qc_bildschirm_edr(CGDirectDisplayID d, double *potentiell, double *aktuell);

// Stabile Kennung aus den drei Nummern; die Unit haengt der Lieferant nur
// bei Gleichheit an.
NSString *qc_bildschirm_kennung(uint32_t vendor, uint32_t model, uint32_t serial);

// Der laufende Modus eines Bildschirms: Pixel (bei HiDPI die Pixel hinter
// den Punkten), Punkte, Hz und die Aufloesung des Panels - die groessten
// Pixelmasse unter den Modi mit 1x-Darstellung (Pixel = Punkte) und den Modi
// mit kDisplayModeNativeFlag, 0 ohne solche Modi. Beides, weil keins allein
// reicht: ein Bildschirm, dessen EDID 1920x1080 bevorzugt, markiert nur das
// als nativ, laeuft aber auch mit 3840x2160 in 1x; ein eingebautes
// Retina-Panel bietet seine volle Aufloesung womoeglich nur als HiDPI-Modus
// an, und der ist nativ markiert. Rueckgabe 0 ohne Modus (alles 0).
typedef struct {
    size_t pw, ph;               // Pixel
    size_t punkte_w, punkte_h;   // Punkte
    size_t nativ_w, nativ_h;     // Aufloesung des Panels
    double hz;
} qc_bildschirm_modus;
int qc_bildschirm_modus_lesen(CGDirectDisplayID d, qc_bildschirm_modus *m);

// Die Obergrenze aus einer Modusliste (Pixel, Punkte, native Markierung je
// Modus): der groesste 1x-Modus oder der groesste native, was mehr Pixel
// hat. Rein, damit der Pruefstand sie mit erfundenen Listen fahren kann.
typedef struct {
    size_t pw, ph, punkte_w, punkte_h;
    int nativ;
} qc_modus_eintrag;
void qc_panelgroesse(const qc_modus_eintrag *modi, size_t n, size_t *w, size_t *h);

// Stromgroesse ohne --out (Produktentscheidung 2026-09-29: nativ als Basis):
// genau die Pixel des Modus, immer gerade. Nur ein skalierter HiDPI-Modus
// (Pixel ungleich Punkte) mit mehr Pixeln als das Panel wird auf das Panel
// begrenzt (nativ 0 = unbekannt: keine Grenze), bei gleichem
// Seitenverhaeltnis: "wie 2560x1440" auf einem 4K-Panel rechnet intern mit
// 5120x2880 und gibt 3840x2160 aufs Panel - mehr sieht auch der Mac selbst
// nicht, gestreamt werden die 3840x2160. Ein Modus in 1x wird nie begrenzt:
// was eingestellt ist, wird gestreamt.
void qc_stromgroesse_nativ(size_t pw, size_t ph, size_t punkte_w, size_t punkte_h, size_t nativ_w, size_t nativ_h,
                           int *w, int *h);

// Die Grenze der H.264-Encoder: hoechstens 4096 Bildpunkte je Seite und
// hoechstens 36864 Makrobloecke (4096x2304, Level 5.2) - dieselbe wie
// encoder::H264_MAX_* beim Windows-Host. Gemessen: VideoToolbox H.264 auf
// dem M1 (Hardware) oeffnet bis 4096 je Seite (auch 4096x4096) und lehnt
// 4112x2304, 5120x1440, 5120x2880 und 6016x3384 mit -12903 ab. NVENC H.264
// nimmt hoechstens 4096x4096 (NVIDIA), und die Hardware-Decoder der Clients
// sind oft auf Level 5.2 begrenzt - darum gilt auch die Flaeche. Die Groesse
// ist eine Vorgabe: codecs_pruefen probiert sie mit dem Encoder aus und
// nimmt 1920x1080, wenn er sie nicht oeffnet.
#define QC_H264_MAX_SEITE 4096
#define QC_H264_MAX_MB    36864
typedef struct {
    int max_seite;   // Bildpunkte je Seite
    int max_mb;      // Makrobloecke (16x16) je Bild
} qc_h264_grenze;

// Passt w x h in die Grenze?
int qc_h264_passt(int w, int h, qc_h264_grenze g);
// w x h in die Grenze einpassen, bei gleichem Seitenverhaeltnis und gerade;
// passt es schon, bleibt es. Rueckgabe 1, wenn eingepasst wurde.
int qc_h264_einpassen(int w, int h, qc_h264_grenze g, int *ow, int *oh);

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
