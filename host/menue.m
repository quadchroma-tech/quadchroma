// Oberflaeche des Mac-Hosts, siehe menue.h.
//
// Aufbau: Zustand (vom Kern, auf der Oberflaechen-Warteschlange gelesen) ->
// Modell (reine Funktion) -> NSMenu. Das Menue wird bei jeder Aenderung ganz
// neu gebaut - es hat eine Handvoll Punkte, und so gibt es keinen halb
// nachgefuehrten Stand. Der Kern meldet Aenderungen ueber
// qc_ui_zustand_geaendert; zusaetzlich liest das Oeffnen des Menues neu
// (SMAppService und die Freigaben kann der Nutzer in den Systemeinstellungen
// aendern, ohne dass der Kern davon erfaehrt).
#import "menue.h"
#import "texte.h"
#import <ServiceManagement/ServiceManagement.h>
#import <objc/runtime.h>
#include <stdatomic.h>
#include <string.h>
#include <signal.h>
#include <pwd.h>
#include <time.h>
#include <unistd.h>
#include <utmpx.h>

// Wie in clipboard.m: so markierte Inhalte gehen nicht zum Client und
// landen (nach der Verabredung unter nspasteboard.org) in keinem Verlauf.
static NSString *const kVerdeckt = @"org.nspasteboard.ConcealedType";

// Hoechstens so viele Geraete liest das Menue (mehr passt ohnehin nicht auf
// einen Bildschirm).
#define QC_GERAETE_MAX 256

// ------------------------------------------------------------ Modellklassen

@implementation QCMenueZustand
@end

@implementation QCMenuePunkt
@end

@implementation QCAnfrage
@end

@implementation QCAnfragen {
    NSMutableArray<QCAnfrage *> *_liste;
}
- (instancetype)init {
    if ((self = [super init])) _liste = [NSMutableArray array];
    return self;
}
- (BOOL)neu:(QCAnfrage *)a {
    for (QCAnfrage *b in _liste) if (b.nr == a.nr) return NO;
    [_liste addObject:a];
    return _liste.count == 1;
}
- (BOOL)weg:(uint64_t)nr {
    for (NSUInteger i = 0; i < _liste.count; i++)
        if (_liste[i].nr == nr) {
            [_liste removeObjectAtIndex:i];
            return i == 0;
        }
    return NO;
}
- (QCAnfrage *)erste { return _liste.firstObject; }
- (NSUInteger)anzahl { return _liste.count; }
@end

// ------------------------------------------------------------ Hilfen

NSString *qc_id_text(uint32_t nummer) {
    nummer %= 1000000000u;
    return [NSString stringWithFormat:@"%03u %03u %03u", nummer / 1000000u, (nummer / 1000u) % 1000u, nummer % 1000u];
}

NSString *qc_code_text(uint32_t code) {
    code %= 1000000u;
    return [NSString stringWithFormat:@"%03u %03u", code / 1000u, code % 1000u];
}

NSString *qc_datum_text(NSString *iso) {
    if (!iso.length) return @"";
    NSTimeZone *utc = [NSTimeZone timeZoneForSecondsFromGMT:0];
    NSDateFormatter *ein = [[NSDateFormatter alloc] init];
    ein.locale = [NSLocale localeWithLocaleIdentifier:@"en_US_POSIX"];
    ein.timeZone = utc;
    ein.dateFormat = @"yyyy-MM-dd";
    NSDate *d = [ein dateFromString:iso];
    if (!d) return iso;
    NSDateFormatter *aus = [[NSDateFormatter alloc] init];
    const char *code = qc_texte_code(qc_texte_aktuell());
    aus.locale = [NSLocale localeWithLocaleIdentifier:code ? @(code) : @"en"];
    aus.timeZone = utc;   // dieselbe Zone wie beim Lesen, sonst kippt der Tag
    aus.dateStyle = NSDateFormatterMediumStyle;
    aus.timeStyle = NSDateFormatterNoStyle;
    return [aus stringFromDate:d] ?: iso;
}

int qc_passwort_pruefen(NSString *pw, NSString *wiederholt) {
    // norm aus Abschnitt 3.4, nur gezaehlt: Leerzeichen, Tab, LF, CR und '-'
    // fallen weg, alle anderen Bytes zaehlen (UTF-8, keine Normalisierung).
    // Dazu die Grenzen des Kerns: hoechstens QC_ZUGANG_PW_MAX Byte wie
    // eingegeben, eine Zeile (eingefuegter Text kann Umbrueche tragen). Der
    // Kern prueft selbst noch einmal (qc_zugang_passwort_setzen -1).
    size_t n = 0, roh = 0;
    int umbruch = 0;
    for (const unsigned char *p = (const unsigned char *)(pw.UTF8String ?: ""); *p; p++, roh++) {
        if (*p == 0x0A || *p == 0x0D) umbruch = 1;
        if (*p != 0x20 && *p != 0x09 && *p != 0x0A && *p != 0x0D && *p != '-') n++;
    }
    if (n < 8) return 2;
    if (roh > QC_ZUGANG_PW_MAX || umbruch) return 3;
    if (![pw isEqualToString:wiederholt ?: @""]) return 1;
    return 0;
}

// Ein Textfeld fester Laenge aus der Geraeteliste (nicht zwingend mit 0
// abgeschlossen, vom Client gewaehlt): hoechstens bis zur Laenge, kaputtes
// UTF-8 wird zu "?".
static NSString *feld_text(const char *p, size_t max) {
    NSString *s = [[NSString alloc] initWithBytes:p length:strnlen(p, max) encoding:NSUTF8StringEncoding];
    return s.length ? s : @"?";
}

// ------------------------------------------------------------ Modell

static QCMenuePunkt *punkt(NSString *titel, QCAktion aktion, BOOL aktiv) {
    QCMenuePunkt *p = [[QCMenuePunkt alloc] init];
    p.titel = titel;
    p.aktion = aktion;
    p.aktiv = aktiv;
    return p;
}

static QCMenuePunkt *trennlinie(void) { return [[QCMenuePunkt alloc] init]; }

static NSArray<QCMenuePunkt *> *geraete_untermenue(QCMenueZustand *z) {
    NSMutableArray<QCMenuePunkt *> *u = [NSMutableArray array];
    const qc_geraet *g = z.geraete.bytes;
    int n = z.geraeteAnzahl;
    if (n > (int)(z.geraete.length / sizeof *g)) n = (int)(z.geraete.length / sizeof *g);
    if (n <= 0) {
        [u addObject:punkt(qc_text(QCTextHostNoDevices), QCAktionKeine, NO)];
        return u;
    }
    for (int i = 0; i < n; i++) {
        NSString *zeile = qc_text_mit(QCTextHostDeviceLine, @{
            @"n": feld_text(g[i].name, sizeof g[i].name),
            @"i": qc_id_text(g[i].id),
            @"d": qc_datum_text(feld_text(g[i].datum, sizeof g[i].datum)),
        });
        QCMenuePunkt *weg = punkt(qc_text(QCTextHostRemove), QCAktionGeraetEntfernen, YES);
        weg.daten = [NSData dataWithBytes:g[i].pub length:sizeof g[i].pub];
        QCMenuePunkt *p = punkt(zeile, QCAktionKeine, YES);
        p.unter = @[weg];
        [u addObject:p];
    }
    [u addObject:trennlinie()];
    [u addObject:punkt(qc_text(QCTextHostRemoveAll), QCAktionAlleEntfernen, YES)];
    return u;
}

// So viele gefundene Hosts stehen hoechstens im Menue (wie unter Windows,
// tray::HOSTS_MAX).
#define QC_HOSTS_MAX 4

NSArray<QCMenuePunkt *> *qc_menue_modell(QCMenueZustand *z) {
    NSMutableArray<QCMenuePunkt *> *m = [NSMutableArray array];

    // Kopf: Name und Zustand. Belegt ein anderes Programm den Bildport, kann
    // niemand verbinden - dann steht das statt "Bereit" da. Die eine App
    // nennt im Kopf den Geraetenamen (wie unter Windows) und sagt, wenn die
    // Freigabe aus ist.
    NSString *kopftitel = z.app && z.geraetename.length ? [@"QuadChroma – " stringByAppendingString:z.geraetename]
                                                        : @"QuadChroma";
    QCMenuePunkt *kopf = punkt(kopftitel, QCAktionKeine, NO);
    kopf.kopf = YES;
    [m addObject:kopf];
    NSString *zustand;
    if (z.app && !z.freigabe)
        zustand = qc_text(QCTextHostSharingIsOff);
    else if (z.zuschauer)
        zustand = qc_text_mit(QCTextHostConnected, @{ @"n": z.zuschauer });
    else if (z.portBelegt > 0)
        zustand = qc_text_mit(QCTextHostPortBusy, @{ @"p": [NSString stringWithFormat:@"%d", z.portBelegt] });
    else
        zustand = qc_text(QCTextHostReady);
    [m addObject:punkt(zustand, QCAktionKeine, NO)];
    [m addObject:trennlinie()];

    // Die eine App: ihr Fenster und die gefundenen Hosts (Name, ohne Namen
    // die Adresse - der Client hat die Namen schon entschaerft).
    if (z.app) {
        [m addObject:punkt(qc_text(QCTextTrayOpenApp), QCAktionOeffnen, YES)];
        NSUInteger n = 0;
        for (NSArray<NSString *> *h in z.hosts) {
            if (n >= QC_HOSTS_MAX) break;
            if (h.count < 2 || !h[1].length) continue;
            n++;
            QCMenuePunkt *p = punkt(qc_text_mit(QCTextTrayConnect, @{ @"n": h[0].length ? h[0] : h[1] }), QCAktionVerbinden, YES);
            p.daten = [h[1] dataUsingEncoding:NSUTF8StringEncoding];
            [m addObject:p];
        }
        [m addObject:trennlinie()];
    }

    // Wer dieser Mac ist und wie man hineinkommt.
    [m addObject:punkt(qc_text_mit(QCTextHostDeviceId, @{ @"i": qc_id_text(z.eigeneId) }), QCAktionIdKopieren, YES)];
    if (z.passwort)
        [m addObject:punkt(qc_text_mit(QCTextHostPassword, @{ @"p": z.passwort }), QCAktionPasswortKopieren, YES)];
    else
        [m addObject:punkt(qc_text(QCTextHostPasswordUnreadable), QCAktionKeine, NO)];
    [m addObject:punkt(qc_text(QCTextHostChangePassword), QCAktionPasswortAendern, YES)];
    [m addObject:punkt(qc_text(QCTextHostRandomPassword), QCAktionPasswortZufall, YES)];
    [m addObject:trennlinie()];

    // Wer schon herein darf. Eine beschaedigte Liste steht sichtbar im
    // Hauptmenue, nicht versteckt im Untermenue.
    if (z.geraeteAnzahl < 0) {
        [m addObject:punkt(qc_text(QCTextHostListDamaged), QCAktionKeine, NO)];
        [m addObject:punkt(qc_text(QCTextHostListReset), QCAktionListeZuruecksetzen, YES)];
    } else {
        QCMenuePunkt *g = punkt(qc_text(QCTextHostDevices), QCAktionKeine, YES);
        g.unter = geraete_untermenue(z);
        [m addObject:g];
    }
    [m addObject:trennlinie()];

    // Die eine App: Geraetename und der Schalter der Freigabe.
    if (z.app) {
        [m addObject:punkt(qc_text(QCTextDeviceNameChange), QCAktionNameAendern, YES)];
        QCMenuePunkt *fr = punkt(qc_text(QCTextStartShareMac), QCAktionFreigabe, YES);
        fr.haken = z.freigabe;
        [m addObject:fr];
    }

    // Start bei der Anmeldung und fehlende Freigaben (nur wenn sie fehlen -
    // und in der einen App nur, solange sie freigibt: ohne Freigabe braucht
    // niemand Bildschirmaufnahme und Bedienungshilfen).
    QCMenuePunkt *anm = punkt(qc_text(QCTextHostStartLogin), QCAktionAnmelden,
                              z.anmelden != QCAnmeldenNichtInProgramme);
    anm.haken = z.anmelden == QCAnmeldenAn;
    anm.gemischt = z.anmelden == QCAnmeldenFreigabeNoetig;
    [m addObject:anm];
    if (z.anmelden == QCAnmeldenNichtInProgramme)
        [m addObject:punkt(qc_text(QCTextHostMoveToApps), QCAktionKeine, NO)];
    if (z.app) {
        QCMenuePunkt *ruhe = punkt(qc_text(QCTextPreventSleep), QCAktionRuhe, YES);
        ruhe.haken = z.ruhe;
        [m addObject:ruhe];
        // Gewuenscht, aber vom System abgelehnt: der Grund darunter (vom
        // Client schon uebersetzt), der Haken zeigt, was gilt - aus.
        if (z.ruheGrund.length) [m addObject:punkt(z.ruheGrund, QCAktionKeine, NO)];
    }
    if (!z.app || z.freigabe) {
        if (!z.bildschirm)
            [m addObject:punkt(qc_text(QCTextHostScreenMissing), QCAktionBildschirmFreigabe, YES)];
        if (!z.bedienung)
            [m addObject:punkt(qc_text(QCTextHostAccessMissing), QCAktionBedienungshilfen, YES)];
    }
    [m addObject:trennlinie()];

    QCMenuePunkt *ende = punkt(qc_text(QCTextHostQuit), QCAktionBeenden, YES);
    ende.taste = @"q";
    [m addObject:ende];
    return m;
}

static void titel_sammeln(NSArray<QCMenuePunkt *> *modell, NSString *einzug, NSMutableArray<NSString *> *aus) {
    for (QCMenuePunkt *p in modell) {
        if (!p.titel) { [aus addObject:[einzug stringByAppendingString:@"---"]]; continue; }
        NSString *t = p.kopf ? [@"# " stringByAppendingString:p.titel]
                    : p.aktiv ? p.titel : [NSString stringWithFormat:@"(%@)", p.titel];
        if (p.haken) t = [@"[x] " stringByAppendingString:t];
        else if (p.gemischt) t = [@"[-] " stringByAppendingString:t];
        [aus addObject:[einzug stringByAppendingString:t]];
        if (p.unter) titel_sammeln(p.unter, [einzug stringByAppendingString:@"  "], aus);
    }
}

NSArray<NSString *> *qc_menue_titel(NSArray<QCMenuePunkt *> *modell) {
    NSMutableArray<NSString *> *aus = [NSMutableArray array];
    titel_sammeln(modell, @"", aus);
    return aus;
}

// Ziel der Menuepunkte (QCOberflaeche unten).
@protocol QCMenueZiel <NSObject>
- (void)menueAktion:(NSMenuItem *)punkt;
@end

static NSMenuItem *menue_eintrag(QCMenuePunkt *p, id ziel) {
    if (!p.titel) return [NSMenuItem separatorItem];
    if (p.kopf) return [NSMenuItem sectionHeaderWithTitle:p.titel];
    SEL aktion = p.aktion != QCAktionKeine ? @selector(menueAktion:) : NULL;
    NSMenuItem *it = [[NSMenuItem alloc] initWithTitle:p.titel action:aktion keyEquivalent:p.taste ?: @""];
    it.target = aktion ? ziel : nil;
    it.tag = p.aktion;
    it.enabled = p.aktiv;
    it.state = p.haken ? NSControlStateValueOn : p.gemischt ? NSControlStateValueMixed : NSControlStateValueOff;
    it.representedObject = p.daten;
    if (p.unter) {
        NSMenu *u = [[NSMenu alloc] initWithTitle:p.titel];
        u.autoenablesItems = NO;
        for (QCMenuePunkt *q in p.unter) [u addItem:menue_eintrag(q, ziel)];
        it.submenu = u;
    }
    return it;
}

NSMenu *qc_menue_bauen(NSArray<QCMenuePunkt *> *modell, id ziel) {
    NSMenu *m = [[NSMenu alloc] initWithTitle:@"QuadChroma"];
    m.autoenablesItems = NO;
    for (QCMenuePunkt *p in modell) [m addItem:menue_eintrag(p, ziel)];
    return m;
}

// ------------------------------------------------------------ Symbol

NSImage *qc_menue_symbol(BOOL verbunden) {
    NSImage *bild = [NSImage imageWithSize:NSMakeSize(18, 18) flipped:YES drawingHandler:^BOOL(NSRect r) {
        (void)r;
        // Geometrie wie die Vorlage des Clients (client/src/logo.rs, vorlage):
        // Rand n/12, Luecke n/9, Radius 0,06 n - hier in Punkten statt
        // ganzen Bildpunkten, AppKit rastert in jeder Aufloesung selbst.
        const CGFloat n = 18, rand = n / 12, luecke = n / 9, radius = 0.06 * n;
        const CGFloat feld = (n - 2 * rand - luecke) / 2;
        const CGFloat links[2] = { rand, rand + feld + luecke };
        [[NSColor blackColor] set];
        for (int zeile = 0; zeile < 2; zeile++)
            for (int spalte = 0; spalte < 2; spalte++) {
                NSRect f = NSMakeRect(links[spalte], links[zeile], feld, feld);
                if (zeile == 1 && spalte == 1) {
                    // Das vierte Feld (unten rechts) nur als Rahmen: so sieht
                    // das Host-Symbol anders aus als das des Mac-Clients, auch
                    // wenn beide auf demselben Mac laufen.
                    const CGFloat strich = 1.3;
                    NSBezierPath *p = [NSBezierPath bezierPathWithRoundedRect:NSInsetRect(f, strich / 2, strich / 2)
                                                                      xRadius:radius yRadius:radius];
                    p.lineWidth = strich;
                    [p stroke];
                    if (verbunden) {
                        const CGFloat d = feld * 0.36;
                        [[NSBezierPath bezierPathWithOvalInRect:NSMakeRect(NSMidX(f) - d / 2, NSMidY(f) - d / 2, d, d)] fill];
                    }
                } else {
                    [[NSBezierPath bezierPathWithRoundedRect:f xRadius:radius yRadius:radius] fill];
                }
            }
        return YES;
    }];
    bild.template = YES;
    bild.accessibilityDescription = @"QuadChroma";
    return bild;
}

// ------------------------------------------------------------ Zustand lesen

static qc_oberflaeche_cfg g_cfg;
static _Atomic int g_ui_da = 0;            // qc_oberflaeche_starten ist gelaufen
static _Atomic int g_auffrischen_steht_an = 0;

// Der Stand des Clients in der einen App (qc_app_stand_setzen, Hauptfaden),
// gelesen auf der Oberflaechen-Warteschlange: unter @synchronized auf
// g_app_sperre, nur unveraenderliche Objekte.
static _Atomic int g_app = 0;              // die eine App: cfg.app ist gesetzt
static NSObject *g_app_sperre;
static BOOL g_app_freigabe, g_app_ruhe, g_app_sitzung;
static int g_programm_sprache = -1;        // Sprache des Programmmenues (nur Hauptfaden)
static NSArray<NSArray<NSString *> *> *g_app_hosts;
static NSString *g_app_tooltip;
static NSString *g_app_ruhe_grund;               // leer = kein Grund
static NSString *g_app_schliessen_titel;         // Programmmenue: leer = "QuadChroma beenden" (nur Hauptfaden)

static NSObject *app_sperre(void) {
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{ g_app_sperre = [[NSObject alloc] init]; });
    return g_app_sperre;
}

_Static_assert(QCAnmeldenNichtInProgramme == QC_ANMELDUNG_NICHT_IN_PROGRAMME && QCAnmeldenAus == QC_ANMELDUNG_AUS &&
               QCAnmeldenAn == QC_ANMELDUNG_AN && QCAnmeldenFreigabeNoetig == QC_ANMELDUNG_FREIGABE_NOETIG,
               "QCAnmelden und QC_ANMELDUNG_* (dienst.h) muessen gleich zaehlen");

static QCAnmelden anmelden_lesen(void) {
    // Ein Anmeldeobjekt zeigt auf den Ort der App. Von der DMG, aus
    // "Downloads" oder verlagert (App Translocation) zeigte es ins Leere.
    if (![[[NSBundle mainBundle] bundlePath] hasPrefix:@"/Applications/"]) return QCAnmeldenNichtInProgramme;
    switch ([SMAppService mainAppService].status) {
        case SMAppServiceStatusEnabled: return QCAnmeldenAn;
        case SMAppServiceStatusRequiresApproval: return QCAnmeldenFreigabeNoetig;
        default: return QCAnmeldenAus;
    }
}

QCMenueZustand *qc_menue_zustand_lesen(void) {
    QCMenueZustand *z = [[QCMenueZustand alloc] init];
    z.eigeneId = qc_zugang_eigene_id();

    char pw[QC_ZUGANG_PW_MAX + 1];
    if (qc_zugang_passwort(pw, sizeof pw) == 0) {
        pw[sizeof pw - 1] = 0;
        z.passwort = [NSString stringWithUTF8String:pw];   // kaputtes UTF-8 -> nil, gilt als unlesbar
    }
    memset_s(pw, sizeof pw, 0, sizeof pw);

    qc_geraet *liste = calloc(QC_GERAETE_MAX, sizeof *liste);
    int n = liste ? qc_zugang_geraete(liste, QC_GERAETE_MAX) : 0;
    if (n > QC_GERAETE_MAX) n = QC_GERAETE_MAX;
    z.geraeteAnzahl = n;
    if (n > 0) z.geraete = [NSData dataWithBytes:liste length:(size_t)n * sizeof *liste];
    free(liste);

    char name[128] = {0};
    if (qc_zustand_zuschauer(name, sizeof name) == 1) z.zuschauer = feld_text(name, sizeof name);

    z.bildschirm = qc_zustand_bildschirmfreigabe() != 0;
    z.bedienung = qc_zustand_bedienungshilfen() != 0;
    z.anmelden = anmelden_lesen();
    z.portBelegt = qc_zustand_port_belegt();

    if (atomic_load(&g_app)) {
        z.app = YES;
        char n[128] = {0};
        qc_zustand_geraetename(n, sizeof n);
        z.geraetename = feld_text(n, sizeof n);
        @synchronized (app_sperre()) {
            z.freigabe = g_app_freigabe;
            z.ruhe = g_app_ruhe;
            z.ruheGrund = g_app_ruhe_grund.length ? g_app_ruhe_grund : nil;
            z.hosts = g_app_hosts ?: @[];
        }
    }
    return z;
}

// ======================================================= Laufende Oberflaeche
// Ab hier: nur im Host (qc_dienst_starten in main.m ruft
// qc_oberflaeche_starten). Der Pruefstand menuetest bindet die Datei ein,
// startet sie aber nie; hosttest bindet sie gar nicht (main.m bringt
// schwache Standardfassungen ihrer Einstiege mit).

static void ui_log(NSString *fmt, ...) NS_FORMAT_FUNCTION(1, 2);
static void ui_log(NSString *fmt, ...) {
    if (!g_cfg.protokoll) return;
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    g_cfg.protokoll(s);
}

// Eigene serielle Warteschlange fuer alle Aufrufe in den Kern.
static dispatch_queue_t ui_q(void) {
    static dispatch_queue_t q;
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{ q = dispatch_queue_create("tech.quadchroma.oberflaeche", DISPATCH_QUEUE_SERIAL); });
    return q;
}

// Nur auf dem Hauptfaden:
@class QCOberflaeche, QCFrage, QCPasswortFenster, QCNamenFenster;
static QCOberflaeche *g_ui;                 // Delegate und Ziel der Menuepunkte
static NSStatusItem *g_item;
static NSMenu *g_menue;
static QCMenueZustand *g_zustand;
static QCAnfragen *g_anfragen;
static uint64_t g_anfrage_gezeigt;          // Nummer im Zulassen-Fenster, 0 = keine
static QCFrage *g_anfrage_fenster;
static QCFrage *g_frage_fenster;            // Rueckfrage "Alle Geraete entfernen"
static QCPasswortFenster *g_passwort_fenster;
static QCNamenFenster *g_namen_fenster;
static NSPopover *g_hinweis;                // die einmalige Hinweisblase (qc_menueleiste_hinweis)
static id<NSObject> g_aktivitaet;
static dispatch_source_t g_signale[2];
static int g_kopiert_nr;
static int g_beenden;                       // 0 laeuft, 1 Abschied unterwegs, 2 fertig
static int g_signal_beenden;                // ein Signal hat terminate: schon angestellt
static int g_eingebettet;                   // im Rust-Client: NSApp.delegate gehoert winit
static id g_ende_beobachter;                // eingebettet: NSApplicationWillTerminateNotification
// Was das Menue zuletzt zeigte: bleibt es gleich, wird nicht neu gebaut -
// sonst flackerte ein offenes Menue (das Oeffnen liest selbst neu).
static NSArray<NSString *> *g_titel_zuletzt;
static NSData *g_geraete_zuletzt;

static void menue_anwenden(void);

// Den Zustand neu lesen (auf ui_q) und das Menue danach neu bauen. Aus jedem
// Faden. Mehrere Anstoesse, waehrend einer wartet, fallen zusammen; einer,
// der waehrend des Lesens kommt, liest danach noch einmal.
static void zustand_auffrischen(void) {
    if (!atomic_load(&g_ui_da)) return;
    if (atomic_exchange(&g_auffrischen_steht_an, 1)) return;
    dispatch_async(ui_q(), ^{ @autoreleasepool {
        atomic_store(&g_auffrischen_steht_an, 0);
        QCMenueZustand *z = qc_menue_zustand_lesen();
        dispatch_async(dispatch_get_main_queue(), ^{
            g_zustand = z;
            menue_anwenden();
        });
    }});
}

// Eine Kernfunktion aus einer Menueaktion: auf ui_q, danach neu lesen.
static void im_kern(dispatch_block_t b) {
    dispatch_async(ui_q(), ^{
        @autoreleasepool { b(); }
        zustand_auffrischen();
    });
}

static void hinweis_zeigen(QCText t, double sekunden);

// Wie im_kern fuer Kernfunktionen mit Rueckgabe (0 = gelungen). Ein Fehler
// kommt ins Protokoll (was: deutsch, ohne Geheimnisse); mit einem Text
// (fehlertext != QCTextAnzahl) steht er zusaetzlich kurz neben dem Symbol.
// Das neu gelesene Menue zeigt ohnehin den wahren Stand (etwa das alte
// Passwort oder das nicht entfernte Geraet).
static void im_kern_pruefen(NSString *was, QCText fehlertext, int (^b)(void)) {
    dispatch_async(ui_q(), ^{
        int r;
        @autoreleasepool { r = b(); }
        if (r != 0) {
            ui_log(@"Oberflaeche: %@ gescheitert (%d)", was, r);
            if (fehlertext != QCTextAnzahl)
                dispatch_async(dispatch_get_main_queue(), ^{ hinweis_zeigen(fehlertext, 4.0); });
        }
        zustand_auffrischen();
    });
}

static void ablage_setzen(NSString *text, BOOL verdeckt) {
    NSPasteboardItem *it = [[NSPasteboardItem alloc] init];
    [it setString:text forType:NSPasteboardTypeString];
    if (verdeckt) [it setString:@"" forType:kVerdeckt];
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb writeObjects:@[it]];
}

// Kurz ein Hinweis neben dem Symbol ("Kopiert", oder dass etwas nicht
// gelang) - das Menue ist nach dem Klick schon zu. Nur auf dem Hauptfaden.
static void hinweis_zeigen(QCText t, double sekunden) {
    if (!g_item) return;
    g_item.button.title = qc_text(t);
    int nr = ++g_kopiert_nr;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(sekunden * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        if (nr == g_kopiert_nr) g_item.button.title = @"";
    });
}

static void kopiert_zeigen(void) { hinweis_zeigen(QCTextHostCopied, 1.5); }

static void einstellungen_oeffnen(NSString *anker) {
    NSString *s = [@"x-apple.systempreferences:com.apple.preference.security?" stringByAppendingString:anker];
    NSURL *u = [NSURL URLWithString:s];
    if (!u || ![[NSWorkspace sharedWorkspace] openURL:u])
        ui_log(@"Oberflaeche: Systemeinstellungen (%@) liessen sich nicht oeffnen", anker);
}

// Eine Accessory-App bekommt ein Fenster erst nach vorn, wenn sie aktiv ist.
// Nur fuer Fenster, die der Nutzer selbst ueber das Menue oeffnet.
static void aktivieren(void) {
    [NSApp activate];
}

// ------------------------------------------------------------ Frage-Fenster
// Ein kleines, nicht-modales Fenster mit Text und zwei Knoepfen - fuer die
// Zulassen-Anfrage und die Rueckfrage vor "Alle Geraete entfernen". Kein
// NSAlert: der braeuchte runModal, und das hielte die Main Queue an.

@interface QCFrage : NSObject <NSWindowDelegate>
@property (nonatomic, strong) NSPanel *panel;
@property (nonatomic, copy) void (^antwort)(BOOL ja);
@property (nonatomic) BOOL zulassen;   // Zulassen-Anfrage: schwebend, nicht aktivierend, mit Ton
@property (nonatomic, strong) NSButton *jaKnopf;
@property (nonatomic) BOOL jaFrei;     // "Ja" nimmt Klick und Return an (Zulassen erst nach ZULASSEN_SPERRE_MS)
@property (nonatomic) NSUInteger runde; // zaehlt jedes gezeigte Fenster
@end

// So lange ist "Zulassen" nach dem Erscheinen des Fensters gesperrt: jede
// unbekannte Gegenstelle kann das schwebende Fenster samt Ton beliebig oft
// neu ausloesen, und ein Klick heisst Vollzugriff - wer gerade dorthin
// klickt, wo es aufgeht, soll nicht versehentlich zulassen. "Ablehnen"
// wirkt sofort. Wie die Windows-Host-Rolle (fenster.rs, 1000 ms).
static const int64_t ZULASSEN_SPERRE_MS = 1000;

@implementation QCFrage

- (void)zeigenText:(NSString *)text zusatz:(NSString *)zusatz ja:(NSString *)ja nein:(NSString *)nein
       jaStandard:(BOOL)jaStandard antwort:(void (^)(BOOL ja))antwort {
    [self schliessen];
    NSWindowStyleMask stil = NSWindowStyleMaskTitled;
    if (self.zulassen) stil |= NSWindowStyleMaskNonactivatingPanel;
    else stil |= NSWindowStyleMaskClosable;
    NSPanel *p = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 380, 120) styleMask:stil
                                              backing:NSBackingStoreBuffered defer:YES];
    p.title = @"QuadChroma";
    p.releasedWhenClosed = NO;
    p.hidesOnDeactivate = NO;
    p.delegate = self;

    NSTextField *t = [NSTextField wrappingLabelWithString:text];
    t.preferredMaxLayoutWidth = 340;
    [t.widthAnchor constraintEqualToConstant:340].active = YES;
    NSMutableArray<NSView *> *teile = [NSMutableArray arrayWithObject:t];
    if (zusatz.length) {
        NSTextField *z = [NSTextField labelWithString:zusatz];
        z.font = [NSFont monospacedDigitSystemFontOfSize:[NSFont systemFontSize] + 2 weight:NSFontWeightSemibold];
        [teile addObject:z];
    }
    NSButton *bj = [NSButton buttonWithTitle:ja target:self action:@selector(jaGeklickt:)];
    NSButton *bn = [NSButton buttonWithTitle:nein target:self action:@selector(neinGeklickt:)];
    self.jaKnopf = bj;
    self.jaFrei = !self.zulassen;
    bj.enabled = self.jaFrei;
    NSUInteger runde = ++self.runde;
    if (jaStandard) {
        bj.keyEquivalent = @"\r";
        bn.keyEquivalent = @"\033";
    } else {
        // Zerstoerendes wird nie mit Return ausgeloest: Abbrechen ist Standard.
        bn.keyEquivalent = @"\r";
        bj.hasDestructiveAction = YES;
    }
    NSStackView *knoepfe = [[NSStackView alloc] init];
    knoepfe.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    [knoepfe addView:bn inGravity:NSStackViewGravityTrailing];
    [knoepfe addView:bj inGravity:NSStackViewGravityTrailing];
    [teile addObject:knoepfe];

    NSStackView *st = [NSStackView stackViewWithViews:teile];
    st.orientation = NSUserInterfaceLayoutOrientationVertical;
    st.alignment = NSLayoutAttributeLeading;
    st.spacing = 12;
    st.edgeInsets = NSEdgeInsetsMake(20, 20, 16, 20);
    [knoepfe.widthAnchor constraintEqualToAnchor:t.widthAnchor].active = YES;
    p.contentView = st;
    [p setContentSize:st.fittingSize];

    self.panel = p;
    self.antwort = antwort;
    [p center];
    if (self.zulassen) {
        // Schwebend und auf jedem Space, auch neben Vollbild-Apps. Bewusst
        // nicht zum Schluesselfenster gemacht und die App nicht aktiviert:
        // wer am Mac gerade tippt, soll nicht mit Return versehentlich
        // zulassen - erst ein Klick ins Fenster gibt ihm die Tastatur (ein
        // nicht aktivierendes Fenster nimmt den Klick, ohne die App nach
        // vorn zu holen).
        p.level = NSFloatingWindowLevel;
        p.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        [p orderFrontRegardless];
        // "Zulassen" erst nach der Sperre - und nur, wenn noch dieses
        // Fenster steht (nicht schon die naechste Anfrage).
        __weak QCFrage *ich = self;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, ZULASSEN_SPERRE_MS * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            QCFrage *f = ich;
            if (!f || !f.panel || f.runde != runde) return;
            f.jaFrei = YES;
            f.jaKnopf.enabled = YES;
        });
        NSSound *ton = [NSSound soundNamed:@"Glass"];
        if (ton) [ton play];
        else NSBeep();
    } else {
        aktivieren();
        [p makeKeyAndOrderFront:nil];
    }
}

- (BOOL)sichtbar { return self.panel.visible; }

- (void)schliessen {
    NSPanel *p = self.panel;
    self.panel = nil;
    self.antwort = nil;
    self.jaKnopf = nil;
    p.delegate = nil;
    [p orderOut:nil];
    [p close];
}

- (void)fertig:(BOOL)ja {
    void (^a)(BOOL) = self.antwort;
    [self schliessen];
    if (a) a(ja);
}

- (void)jaGeklickt:(id)sender {
    (void)sender;
    if (!self.jaFrei) return;          // Zulassen noch gesperrt (auch Return)
    [self fertig:YES];
}
- (void)neinGeklickt:(id)sender { (void)sender; [self fertig:NO]; }

// Schliessknopf der Rueckfrage: wie "Abbrechen". Das Fenster schliesst AppKit
// selbst; hier nur abmelden und antworten.
- (BOOL)windowShouldClose:(NSWindow *)w {
    (void)w;
    void (^a)(BOOL) = self.antwort;
    self.antwort = nil;
    self.panel.delegate = nil;
    self.panel = nil;
    self.jaKnopf = nil;
    if (a) a(NO);
    return YES;
}

@end

// ------------------------------------------------------- Zulassen-Anfragen

static void anfrage_abgleichen(void);

static void anfrage_beantwortet(uint64_t nr, BOOL zulassen) {
    [g_anfragen weg:nr];
    g_anfrage_gezeigt = 0;
    dispatch_async(ui_q(), ^{ qc_zugang_entscheiden(nr, zulassen ? 1 : 0); });
    anfrage_abgleichen();
}

// Das Fenster zeigt immer die erste wartende Anfrage - oder schliesst.
static void anfrage_abgleichen(void) {
    QCAnfrage *a = [g_anfragen erste];
    if (!a) {
        if (g_anfrage_gezeigt) [g_anfrage_fenster schliessen];
        g_anfrage_gezeigt = 0;
        return;
    }
    if (a.nr == g_anfrage_gezeigt && [g_anfrage_fenster sichtbar]) return;
    if (!g_anfrage_fenster) {
        g_anfrage_fenster = [[QCFrage alloc] init];
        g_anfrage_fenster.zulassen = YES;
    }
    g_anfrage_gezeigt = a.nr;
    uint64_t nr = a.nr;
    NSString *text = qc_text_mit(QCTextHostRequest, @{ @"n": a.name ?: @"?", @"i": qc_id_text(a.geraeteId) });
    NSString *code = qc_text_mit(QCTextAccessCode, @{ @"c": qc_code_text(a.code) });
    [g_anfrage_fenster zeigenText:text zusatz:code ja:qc_text(QCTextHostAllow) nein:qc_text(QCTextHostDeny)
                       jaStandard:YES antwort:^(BOOL ja) { anfrage_beantwortet(nr, ja); }];
}

// ------------------------------------------------------------ Passwort-Fenster

@interface QCPasswortFenster : NSObject <NSWindowDelegate>
@property (nonatomic, strong) NSPanel *panel;
@property (nonatomic, strong) NSSecureTextField *eins, *zwei;
@property (nonatomic, strong) NSTextField *hinweis;
@property (nonatomic, strong) NSButton *ok, *abbrechen;
@end

@implementation QCPasswortFenster

- (void)bauen {
    NSPanel *p = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 360, 200)
                                            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                              backing:NSBackingStoreBuffered defer:YES];
    p.title = @"QuadChroma";
    p.releasedWhenClosed = NO;
    p.hidesOnDeactivate = NO;
    p.delegate = self;

    NSTextField *l1 = [NSTextField labelWithString:qc_text(QCTextHostNewPassword)];
    NSTextField *l2 = [NSTextField labelWithString:qc_text(QCTextHostRepeatPassword)];
    self.eins = [[NSSecureTextField alloc] init];
    self.zwei = [[NSSecureTextField alloc] init];
    self.hinweis = [NSTextField wrappingLabelWithString:@""];
    self.hinweis.preferredMaxLayoutWidth = 320;
    self.ok = [NSButton buttonWithTitle:qc_text(QCTextHostOk) target:self action:@selector(okGeklickt:)];
    self.ok.keyEquivalent = @"\r";
    self.abbrechen = [NSButton buttonWithTitle:qc_text(QCTextAccessCancel) target:self action:@selector(abbrechenGeklickt:)];
    self.abbrechen.keyEquivalent = @"\033";
    for (NSView *v in @[ self.eins, self.zwei, self.hinweis ])
        [v.widthAnchor constraintEqualToConstant:320].active = YES;

    NSStackView *knoepfe = [[NSStackView alloc] init];
    knoepfe.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    [knoepfe addView:self.abbrechen inGravity:NSStackViewGravityTrailing];
    [knoepfe addView:self.ok inGravity:NSStackViewGravityTrailing];
    [knoepfe.widthAnchor constraintEqualToConstant:320].active = YES;

    NSStackView *st = [NSStackView stackViewWithViews:@[ l1, self.eins, l2, self.zwei, self.hinweis, knoepfe ]];
    st.orientation = NSUserInterfaceLayoutOrientationVertical;
    st.alignment = NSLayoutAttributeLeading;
    st.spacing = 10;
    [st setCustomSpacing:4 afterView:l1];
    [st setCustomSpacing:4 afterView:l2];
    st.edgeInsets = NSEdgeInsetsMake(20, 20, 16, 20);
    p.contentView = st;
    [p setContentSize:st.fittingSize];
    self.eins.nextKeyView = self.zwei;
    self.zwei.nextKeyView = self.eins;
    self.panel = p;
}

- (void)hinweis:(QCText)t fehler:(BOOL)fehler {
    self.hinweis.stringValue = qc_text(t);
    self.hinweis.textColor = fehler ? [NSColor systemRedColor] : [NSColor secondaryLabelColor];
}

- (void)zeigen {
    if (!self.panel) [self bauen];
    self.panel.title = qc_text(QCTextHostPasswordTitle);
    self.eins.stringValue = @"";
    self.zwei.stringValue = @"";
    self.hinweis.stringValue = @"";
    self.eins.enabled = self.zwei.enabled = self.ok.enabled = YES;
    aktivieren();
    [self.panel center];
    [self.panel makeKeyAndOrderFront:nil];
    [self.panel makeFirstResponder:self.eins];
}

- (void)okGeklickt:(id)sender {
    (void)sender;
    NSString *pw = self.eins.stringValue;
    int r = qc_passwort_pruefen(pw, self.zwei.stringValue);
    if (r == 2) { [self hinweis:QCTextHostPasswordShort fehler:YES]; return; }
    if (r == 3) { [self hinweis:QCTextHostPasswordInvalid fehler:YES]; return; }
    if (r == 1) { [self hinweis:QCTextHostPasswordsDiffer fehler:YES]; return; }
    self.eins.enabled = self.zwei.enabled = self.ok.enabled = NO;
    self.hinweis.stringValue = @"";
    char *kopie = strdup(pw.UTF8String ?: "");
    __weak QCPasswortFenster *schwach = self;
    dispatch_async(ui_q(), ^{
        int e = kopie ? qc_zugang_passwort_setzen(kopie) : -2;
        if (kopie) { memset_s(kopie, strlen(kopie), 0, strlen(kopie)); free(kopie); }
        if (e != 0) ui_log(@"Oberflaeche: Passwort aendern gescheitert (%d)", e);
        zustand_auffrischen();
        dispatch_async(dispatch_get_main_queue(), ^{
            QCPasswortFenster *f = schwach;
            if (!f.panel.visible) return;
            if (e == 0) {
                [f hinweis:QCTextHostPasswordSaved fehler:NO];
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.8 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                    [schwach.panel close];
                });
                return;
            }
            // Die Laenge hat das Fenster schon geprueft: ein -1 des Kerns
            // heisst hier "so nicht annehmbar" (unzulaessig), nicht "zu
            // kurz"; -2 ist ein Schreibfehler - das alte Passwort gilt weiter.
            [f hinweis:e == -1 ? QCTextHostPasswordInvalid : QCTextHostPasswordNotSaved fehler:YES];
            f.eins.enabled = f.zwei.enabled = f.ok.enabled = YES;
        });
    });
}

- (void)abbrechenGeklickt:(id)sender { (void)sender; [self.panel close]; }

- (void)windowWillClose:(NSNotification *)n {
    (void)n;
    // Nichts vom Passwort bleibt in den Feldern stehen.
    self.eins.stringValue = @"";
    self.zwei.stringValue = @"";
}

@end

// ------------------------------------------------------------ Geraetename
// Das Fenster "Geraetename" der einen App (wie host/fenster.rs unter
// Windows): ein Feld mit dem Namen, der jetzt gilt, der Hinweis mit dem
// Rechnernamen, Abbrechen und OK. Geprueft wird wie im Client (Rueckruf
// name_pruefen); ein guter Name geht ueber app (QC_APP_NAME) an den Client,
// der ihn merkt und dem Dienst gibt (qc_dienst_name_setzen).

QCText qc_geraetename_fehler(NSString *eingabe, int (*pruefen)(const char *)) {
    int r = pruefen ? pruefen((eingabe ?: @"").UTF8String ?: "") : 0;
    return r == 1 ? QCTextDeviceNameTooLong : r == 2 ? QCTextDeviceNameInvalid : QCTextAnzahl;
}

@interface QCNamenFenster : NSObject <NSWindowDelegate>
@property (nonatomic, strong) NSPanel *panel;
@property (nonatomic, strong) NSTextField *feld, *hinweis, *fehler, *beschriftung;
@property (nonatomic, strong) NSButton *ok, *abbrechen;
@end

@implementation QCNamenFenster

- (void)bauen {
    NSPanel *p = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 380, 180)
                                            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                              backing:NSBackingStoreBuffered defer:YES];
    p.releasedWhenClosed = NO;
    p.hidesOnDeactivate = NO;
    p.delegate = self;
    self.beschriftung = [NSTextField wrappingLabelWithString:@""];
    self.feld = [[NSTextField alloc] init];
    self.hinweis = [NSTextField wrappingLabelWithString:@""];
    self.hinweis.textColor = [NSColor secondaryLabelColor];
    self.fehler = [NSTextField wrappingLabelWithString:@""];
    self.fehler.textColor = [NSColor systemRedColor];
    self.ok = [NSButton buttonWithTitle:@"" target:self action:@selector(okGeklickt:)];
    self.ok.keyEquivalent = @"\r";
    self.abbrechen = [NSButton buttonWithTitle:@"" target:self action:@selector(abbrechenGeklickt:)];
    self.abbrechen.keyEquivalent = @"\033";
    for (NSTextField *v in @[ self.beschriftung, self.hinweis, self.fehler ]) v.preferredMaxLayoutWidth = 340;
    for (NSView *v in @[ self.beschriftung, self.feld, self.hinweis, self.fehler ])
        [v.widthAnchor constraintEqualToConstant:340].active = YES;
    NSStackView *knoepfe = [[NSStackView alloc] init];
    knoepfe.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    [knoepfe addView:self.abbrechen inGravity:NSStackViewGravityTrailing];
    [knoepfe addView:self.ok inGravity:NSStackViewGravityTrailing];
    [knoepfe.widthAnchor constraintEqualToConstant:340].active = YES;
    NSStackView *st = [NSStackView stackViewWithViews:@[ self.beschriftung, self.feld, self.hinweis, self.fehler, knoepfe ]];
    st.orientation = NSUserInterfaceLayoutOrientationVertical;
    st.alignment = NSLayoutAttributeLeading;
    st.spacing = 8;
    st.edgeInsets = NSEdgeInsetsMake(20, 20, 16, 20);
    p.contentView = st;
    self.panel = p;
}

- (void)zeigen {
    if (!self.panel) [self bauen];
    char name[128] = {0}, rechner[128] = {0};
    qc_zustand_geraetename(name, sizeof name);
    qc_zustand_rechnername(rechner, sizeof rechner);
    // Texte bei jedem Oeffnen: die Sprache kann sich geaendert haben.
    self.panel.title = qc_text(QCTextDeviceNameTitle);
    self.beschriftung.stringValue = qc_text(QCTextDeviceNameLabel);
    self.hinweis.stringValue = qc_text_mit(QCTextDeviceNameHint, @{ @"n": feld_text(rechner, sizeof rechner) });
    self.ok.title = qc_text(QCTextHostOk);
    self.abbrechen.title = qc_text(QCTextAccessCancel);
    self.feld.stringValue = feld_text(name, sizeof name);
    self.fehler.stringValue = @"";
    self.fehler.hidden = YES;
    [self.panel setContentSize:self.panel.contentView.fittingSize];
    aktivieren();
    [self.panel center];
    [self.panel makeKeyAndOrderFront:nil];
    [self.panel makeFirstResponder:self.feld];
}

- (void)okGeklickt:(id)sender {
    (void)sender;
    NSString *eingabe = self.feld.stringValue ?: @"";
    QCText f = qc_geraetename_fehler(eingabe, g_cfg.name_pruefen);
    if (f != QCTextAnzahl) {
        self.fehler.stringValue = qc_text(f);
        self.fehler.hidden = NO;
        [self.panel setContentSize:self.panel.contentView.fittingSize];
        return;
    }
    [self.panel close];
    ui_log(@"Oberflaeche: Geraetename eingegeben");
    if (g_cfg.app) g_cfg.app(QC_APP_NAME, eingabe.UTF8String ?: "");
}

- (void)abbrechenGeklickt:(id)sender { (void)sender; [self.panel close]; }

@end

void qc_geraetename_fenster(void) {
    if (!atomic_load(&g_ui_da)) return;
    if (!g_namen_fenster) g_namen_fenster = [[QCNamenFenster alloc] init];
    [g_namen_fenster zeigen];
}

// ------------------------------------------------------------ Aktionen

static void alle_entfernen_fragen(void) {
    if (!g_frage_fenster) g_frage_fenster = [[QCFrage alloc] init];
    [g_frage_fenster zeigenText:qc_text(QCTextHostRemoveAllAsk) zusatz:nil ja:qc_text(QCTextHostRemove)
                           nein:qc_text(QCTextAccessCancel) jaStandard:NO antwort:^(BOOL ja) {
        if (ja) im_kern_pruefen(@"Alle Geraete entfernen", QCTextAnzahl, ^{ return qc_zugang_alle_entfernen(); });
    }];
}

static void anmelden_umschalten(QCAnmelden jetzt) {
    if (jetzt == QCAnmeldenNichtInProgramme) return;
    if (jetzt == QCAnmeldenFreigabeNoetig) {
        // Registriert, aber abgeschaltet: nur der Nutzer kann es dort erlauben.
        [SMAppService openSystemSettingsLoginItems];
        return;
    }
    BOOL an = jetzt != QCAnmeldenAn;
    im_kern(^{
        NSError *e = nil;
        SMAppService *s = [SMAppService mainAppService];
        BOOL ok = an ? [s registerAndReturnError:&e] : [s unregisterAndReturnError:&e];
        if (ok) ui_log(@"Oberflaeche: Beim Anmelden starten %@", an ? @"eingeschaltet" : @"ausgeschaltet");
        else ui_log(@"Oberflaeche: Beim Anmelden starten liess sich nicht %@ (%@ %ld)",
                    an ? @"einschalten" : @"ausschalten", e.domain, (long)e.code);
        if (an && s.status == SMAppServiceStatusRequiresApproval)
            dispatch_async(dispatch_get_main_queue(), ^{ [SMAppService openSystemSettingsLoginItems]; });
    });
}

// Unsichtbares Hauptmenue: nur damit Cmd+V/C/X/A/Z in den Passwortfeldern
// wirken. Eine Accessory-App zeigt es nie an, deshalb ohne Titel.
static void bearbeiten_menue_setzen(void) {
    struct { SEL s; NSString *k; NSEventModifierFlags f; } t[] = {
        { @selector(undo:), @"z", NSEventModifierFlagCommand },
        { @selector(redo:), @"Z", NSEventModifierFlagCommand },
        { @selector(cut:), @"x", NSEventModifierFlagCommand },
        { @selector(copy:), @"c", NSEventModifierFlagCommand },
        { @selector(paste:), @"v", NSEventModifierFlagCommand },
        { @selector(selectAll:), @"a", NSEventModifierFlagCommand },
        { @selector(performClose:), @"w", NSEventModifierFlagCommand },
    };
    NSMenu *bearbeiten = [[NSMenu alloc] initWithTitle:@""];
    for (size_t i = 0; i < sizeof t / sizeof t[0]; i++) {
        NSMenuItem *it = [[NSMenuItem alloc] initWithTitle:@"" action:t[i].s keyEquivalent:t[i].k];
        it.keyEquivalentModifierMask = t[i].f;
        [bearbeiten addItem:it];
    }
    NSMenuItem *oben = [[NSMenuItem alloc] initWithTitle:@"" action:NULL keyEquivalent:@""];
    oben.submenu = bearbeiten;
    NSMenu *haupt = [[NSMenu alloc] initWithTitle:@""];
    [haupt addItem:oben];
    NSApp.mainMenu = haupt;
}

// Hauptmenue der einen App (siehe menue.h). Gemessen 27.09.2026 auf macOS
// 27: ein Punkt im Baum schluckt seine Taste auch gesperrt und ohne Ziel,
// und NSWindow antwortet selbst auf undo: - ob es ein Ziel fuer die Aktion
// gibt, sagt also nichts; es zaehlt, ob ein Textfeld den Fokus hat. Nimmt
// das Hauptmenue die Taste nicht, gibt NSApp keyDown an das Fenster (winit).
@interface QCHauptmenue ()
@property(nonatomic, strong, readwrite) NSMenu *bearbeiten;
@end

@implementation QCHauptmenue
- (BOOL)performKeyEquivalent:(NSEvent *)e {
    if (qc_bearbeiten_taste(self, e, NSApp.keyWindow.firstResponder)) return YES;
    return [super performKeyEquivalent:e];
}
@end

BOOL qc_bearbeiten_taste(NSMenu *haupt, NSEvent *e, NSResponder *ersthelfer) {
    if (![haupt isKindOfClass:QCHauptmenue.class] || ![ersthelfer isKindOfClass:NSText.class]) return NO;
    return [((QCHauptmenue *)haupt).bearbeiten performKeyEquivalent:e];
}

QCHauptmenue *qc_programmmenue_bauen(id ziel, BOOL sitzung, NSString *schliessen) {
    NSMenu *programm = [[NSMenu alloc] initWithTitle:@"QuadChroma"];
    // Solange das Symbol steht, beendet nur dessen Menue: Cmd+Q schliesst
    // dann das Fenster wie das rote Knoepfchen, und der Punkt heisst so.
    BOOL nur_fenster = schliessen.length > 0;
    NSMenuItem *ende = [[NSMenuItem alloc] initWithTitle:nur_fenster ? schliessen : qc_text(QCTextHostQuit)
                                                  action:@selector(menueAktion:)
                                           keyEquivalent:sitzung ? @"" : @"q"];
    ende.target = ziel;
    ende.tag = nur_fenster ? QCAktionFensterSchliessen : QCAktionBeenden;
    NSMenuItem *weg = [[NSMenuItem alloc] initWithTitle:@"" action:@selector(hide:) keyEquivalent:sitzung ? @"" : @"h"];
    weg.hidden = YES;
    weg.allowsKeyEquivalentWhenHidden = YES;
    [programm addItem:weg];
    [programm addItem:ende];
    // Bearbeiten: nur die Tasten, ohne Ziel an den Ersthelfer (das Textfeld).
    // Shift+Cmd+Z als "Z" (wie Apples Vorlage): NSMenu vergleicht mit
    // charactersIgnoringModifiers, und das ist mit Shift gross - gegen "z"
    // mit Shift-Maske fand performKeyEquivalent: nichts (Probe 27.09.2026).
    NSMenu *bearbeiten = [[NSMenu alloc] initWithTitle:@""];
    struct { SEL s; NSString *k; NSEventModifierFlags f; } b[] = {
        { @selector(undo:), @"z", NSEventModifierFlagCommand },
        { @selector(redo:), @"Z", NSEventModifierFlagCommand },
        { @selector(cut:), @"x", NSEventModifierFlagCommand },
        { @selector(copy:), @"c", NSEventModifierFlagCommand },
        { @selector(paste:), @"v", NSEventModifierFlagCommand },
        { @selector(selectAll:), @"a", NSEventModifierFlagCommand },
    };
    for (size_t i = 0; i < sizeof b / sizeof b[0]; i++) {
        NSMenuItem *it = [[NSMenuItem alloc] initWithTitle:@"" action:b[i].s keyEquivalent:b[i].k];
        it.keyEquivalentModifierMask = b[i].f;
        [bearbeiten addItem:it];
    }
    NSMenuItem *oben = [[NSMenuItem alloc] initWithTitle:@"QuadChroma" action:NULL keyEquivalent:@""];
    oben.submenu = programm;
    QCHauptmenue *haupt = [[QCHauptmenue alloc] initWithTitle:@""];
    [haupt addItem:oben];
    haupt.bearbeiten = bearbeiten;
    return haupt;
}

// ------------------------------------------------------------ Delegate

@interface QCOberflaeche : NSObject <NSApplicationDelegate, NSMenuDelegate, QCMenueZiel>
@end

static void beenden_antworten(void) {
    if (g_beenden == 2) return;
    g_beenden = 2;
    [NSApp replyToApplicationShouldTerminate:YES];
}

// Doppelklick auf die laufende App. Eingebettet mit Rueckruf: der Client
// oeffnet sein Fenster. Sonst das Menue zeigen - auch wenn das Symbol
// verdraengt ist (volle Menueleiste), dann an der Mausposition.
static BOOL wieder_geoeffnet(void) {
    if (g_eingebettet && g_cfg.oeffnen) {
        g_cfg.oeffnen();
        return NO;
    }
    if (!g_item || !g_menue) return NO;
    if (g_item.button.window.screen) [g_item.button performClick:nil];
    else [g_menue popUpMenuPositioningItem:nil atLocation:[NSEvent mouseLocation] inView:nil];
    return NO;
}

// Jeder Weg zum Beenden (Menue, Cmd+Q, SIGTERM/SIGINT, Abmelden) kommt hier
// vorbei: erst schliesst der Kern die Verbindung zum Zuschauer (Abschied),
// dann endet der Prozess. Der Abschied laeuft nicht auf der Main Queue;
// haengt er, beendet die Frist trotzdem.
static NSApplicationTerminateReply beenden_erfragen(void) {
    if (g_beenden == 2) return NSTerminateNow;
    if (g_beenden == 0) {
        g_beenden = 1;
        ui_log(@"Oberflaeche: Beenden - Verbindung zum Zuschauer wird geschlossen");
        [g_anfrage_fenster schliessen];
        [g_frage_fenster schliessen];
        [g_passwort_fenster.panel close];
        void (*abschied)(void) = g_cfg.abschied;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            if (abschied) abschied();
            dispatch_async(dispatch_get_main_queue(), ^{ beenden_antworten(); });
        });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            beenden_antworten();
        });
    }
    return NSTerminateLater;
}

@implementation QCOberflaeche

- (void)applicationDidFinishLaunching:(NSNotification *)n {
    (void)n;
    qc_oberflaeche_fertig();
}

- (BOOL)applicationShouldHandleReopen:(NSApplication *)app hasVisibleWindows:(BOOL)sichtbar {
    (void)app; (void)sichtbar;
    return wieder_geoeffnet();
}

- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)app {
    (void)app;
    return beenden_erfragen();
}

- (void)menuWillOpen:(NSMenu *)menu {
    (void)menu;
    zustand_auffrischen();
}

- (void)menueAktion:(NSMenuItem *)m {
    QCMenueZustand *z = g_zustand;
    switch ((QCAktion)m.tag) {
        case QCAktionIdKopieren:
            // Die 9 Ziffern ohne Leerzeichen: so nimmt sie jedes Eingabefeld.
            ablage_setzen([NSString stringWithFormat:@"%09u", z.eigeneId % 1000000000u], NO);
            kopiert_zeigen();
            break;
        case QCAktionPasswortKopieren:
            if (z.passwort) {
                ablage_setzen(z.passwort, YES);
                kopiert_zeigen();
            }
            break;
        case QCAktionPasswortAendern:
            if (!g_passwort_fenster) g_passwort_fenster = [[QCPasswortFenster alloc] init];
            [g_passwort_fenster zeigen];
            break;
        case QCAktionPasswortZufall:
            // Scheitert das Schreiben, gilt das alte Passwort weiter (das
            // Menue zeigt es) - der Hinweis sagt, warum sich nichts aendert.
            im_kern_pruefen(@"Neues Zufallspasswort", QCTextHostPasswordNotSaved, ^{ return qc_zugang_passwort_zufall(); });
            break;
        case QCAktionGeraetEntfernen: {
            NSData *pub = m.representedObject;
            if ([pub isKindOfClass:[NSData class]] && pub.length == 32)
                im_kern_pruefen(@"Geraet entfernen", QCTextAnzahl, ^{ return qc_zugang_geraet_entfernen(pub.bytes); });
            break;
        }
        case QCAktionAlleEntfernen:
            alle_entfernen_fragen();
            break;
        case QCAktionListeZuruecksetzen:
            im_kern_pruefen(@"Geraeteliste zuruecksetzen", QCTextAnzahl, ^{ return qc_zugang_liste_zuruecksetzen(); });
            break;
        case QCAktionAnmelden:
            anmelden_umschalten(z.anmelden);
            break;
        case QCAktionBildschirmFreigabe:
            einstellungen_oeffnen(@"Privacy_ScreenCapture");
            break;
        case QCAktionBedienungshilfen:
            einstellungen_oeffnen(@"Privacy_Accessibility");
            break;
        case QCAktionBeenden:
            // Die eine App endet ueber den Client: er verlaesst die
            // Ereignisschleife und verabschiedet dann den Zuschauer
            // (qc_dienst_beenden). Die eigene App ueber terminate:.
            if (g_cfg.app) g_cfg.app(QC_APP_BEENDEN, NULL);
            else [NSApp terminate:nil];
            break;
        case QCAktionOeffnen:
            if (g_cfg.app) g_cfg.app(QC_APP_OEFFNEN, NULL);
            break;
        case QCAktionVerbinden: {
            NSData *d = m.representedObject;
            NSString *adresse = [d isKindOfClass:[NSData class]] ? [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] : nil;
            if (g_cfg.app && adresse.length) g_cfg.app(QC_APP_VERBINDEN, adresse.UTF8String);
            break;
        }
        case QCAktionNameAendern:
            qc_geraetename_fenster();
            break;
        case QCAktionFreigabe:
            if (g_cfg.app) g_cfg.app(QC_APP_FREIGABE, NULL);
            break;
        case QCAktionRuhe:
            if (g_cfg.app) g_cfg.app(QC_APP_RUHE, NULL);
            break;
        case QCAktionFensterSchliessen:
            if (g_cfg.app) g_cfg.app(QC_APP_SCHLIESSEN, NULL);
            break;
        case QCAktionKeine:
            break;
    }
}

@end

static void menue_anwenden(void) {
    if (!g_zustand) return;
    if (!g_item) {
        g_item = [[NSStatusBar systemStatusBar] statusItemWithLength:NSVariableStatusItemLength];
        g_item.autosaveName = @"QuadChromaHost";
        g_item.button.toolTip = @"QuadChroma";
        @synchronized (app_sperre()) {
            if (g_app_tooltip.length) g_item.button.toolTip = g_app_tooltip;
        }
        g_item.button.imagePosition = NSImageLeft;
        g_menue = [[NSMenu alloc] initWithTitle:@"QuadChroma"];
        g_menue.autoenablesItems = NO;
        g_menue.delegate = g_ui;
        g_item.menu = g_menue;
        // Verdraengt das System das Symbol (volle Menueleiste,
        // Kameraaussparung, in den Einstellungen ausgeblendet), steht das
        // wenigstens im Protokoll; ein Doppelklick auf die App zeigt das Menue.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            if (g_item && !g_item.button.window.screen)
                ui_log(@"Oberflaeche: Symbol in der Menueleiste nicht sichtbar - Doppelklick auf die App zeigt %@",
                       g_cfg.oeffnen ? @"das Fenster" : @"das Menue");
        });
    }
    NSArray<QCMenuePunkt *> *modell = qc_menue_modell(g_zustand);
    NSArray<NSString *> *titel = qc_menue_titel(modell);
    NSData *geraete = g_zustand.geraete ?: [NSData data];
    if ([titel isEqualToArray:g_titel_zuletzt] && [geraete isEqualToData:g_geraete_zuletzt]) return;
    g_titel_zuletzt = titel;
    g_geraete_zuletzt = geraete;
    [g_menue removeAllItems];
    for (QCMenuePunkt *p in modell) [g_menue addItem:menue_eintrag(p, g_ui)];
    g_item.button.image = qc_menue_symbol(g_zustand.zuschauer != nil);
}

// ------------------------------------------------------------ Start

// SIGTERM/SIGINT beenden wie der Menuepunkt, mit Abschied und Frist. Die
// Quelle meldet sich in einem Block der Main Queue - dort darf terminate:
// NICHT laufen: es dreht bei NSTerminateLater eine eigene Run-Loop, und die
// bedient die Main Queue nicht, solange sie selbst in einem Main-Queue-Block
// steckt. Weder die Antwort nach dem Abschied noch die Frist kaemen an, der
// Host hinge fuer immer (die Signale stehen auf SIG_IGN). Deshalb stellt der
// Block terminate: nur als Block der Run-Loop selbst an - in allen ueblichen
// Modi, also auch bei offenem Menue. Ein zweites Signal, waehrend der
// Abschied laeuft, wartet nicht mehr auf ihn.
static void signale_einrichten(void) {
    int nummern[2] = { SIGTERM, SIGINT };
    for (int i = 0; i < 2; i++) {
        signal(nummern[i], SIG_IGN);   // sonst endete der Prozess, bevor die Quelle es sieht
        dispatch_source_t q = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, (uintptr_t)nummern[i], 0,
                                                     dispatch_get_main_queue());
        if (!q) continue;
        int nr = nummern[i];
        dispatch_source_set_event_handler(q, ^{
            const char *name = nr == SIGTERM ? "SIGTERM" : "SIGINT";
            if (g_beenden == 1) {
                ui_log(@"Oberflaeche: Signal %s waehrend des Abschieds - beende sofort", name);
                beenden_antworten();
                return;
            }
            if (g_beenden || g_signal_beenden) return;
            g_signal_beenden = 1;
            ui_log(@"Oberflaeche: Signal %s - beende", name);
            CFRunLoopRef haupt = CFRunLoopGetMain();
            CFRunLoopPerformBlock(haupt, kCFRunLoopCommonModes, ^{ [NSApp terminate:nil]; });
            CFRunLoopWakeUp(haupt);
        });
        dispatch_resume(q);
        g_signale[i] = q;
    }
}

// Eingebettet (Rust-Client): NSApp.delegate gehoert winit 0.30.13
// (WinitApplicationDelegate, gesetzt in EventLoop::new). winit loest resumed
// nur aus dessen applicationDidFinishLaunching: aus und bricht ab, wenn ein
// anderes Objekt Delegate ist (ApplicationDelegate::get) - also wird
// NSApp.delegate hier nie gesetzt. Die beiden Methoden, die die Oberflaeche
// braucht, kommen per class_addMethod an seine Klasse; winit selbst hat dort
// nur applicationDidFinishLaunching: und applicationWillTerminate:. AppKit
// fragt respondsToSelector: erst beim Aufruf, so greifen auch nach
// setDelegate: angehaengte Methoden (am 27.09.2026 unter macOS 27 mit einem
// nachgebauten Delegate geprueft, auch aus applicationDidFinishLaunching:
// heraus). Hat die Klasse eine Methode schon (andere winit-Fassung), bleibt
// deren und eine Zeile sagt es. Die Typangaben kommen von QCOberflaeche,
// die dieselben Methoden hat.
static void an_winits_delegate_haengen(void) {
    id d = NSApp.delegate;
    if (!d) {
        ui_log(@"Oberflaeche: eingebettet, aber ohne Delegate von NSApp - Doppelklick und Beenden ohne Abschied");
        return;
    }
    Class k = [d class];
    SEL sel_reopen = @selector(applicationShouldHandleReopen:hasVisibleWindows:);
    SEL sel_ende = @selector(applicationShouldTerminate:);
    IMP reopen = imp_implementationWithBlock(^BOOL(id selbst, NSApplication *app, BOOL sichtbar) {
        (void)selbst; (void)app; (void)sichtbar;
        return wieder_geoeffnet();
    });
    IMP ende = imp_implementationWithBlock(^NSApplicationTerminateReply(id selbst, NSApplication *app) {
        (void)selbst; (void)app;
        return beenden_erfragen();
    });
    SEL sels[2] = { sel_reopen, sel_ende };
    IMP imps[2] = { reopen, ende };
    for (int i = 0; i < 2; i++) {
        const char *typen = method_getTypeEncoding(class_getInstanceMethod([QCOberflaeche class], sels[i]));
        if (!class_addMethod(k, sels[i], imps[i], typen)) {
            ui_log(@"Oberflaeche: %@ hat %@ schon - bleibt, wie es ist",
                   NSStringFromClass(k), NSStringFromSelector(sels[i]));
            imp_removeBlock(imps[i]);
        }
    }
}

// Eingebettet das Netz unter dem Beenden: kommt NSApplicationWillTerminate-
// Notification, ohne dass applicationShouldTerminate: den Abschied
// angestossen hat (etwa weil winits Klasse die Methode schon hatte), laeuft
// er hier - auf dem Hauptfaden wartend, mit derselben Frist.
static void ende_beobachten(void) {
    g_ende_beobachter = [[NSNotificationCenter defaultCenter]
        addObserverForName:NSApplicationWillTerminateNotification object:NSApp queue:nil
                usingBlock:^(NSNotification *n) {
        (void)n;
        if (g_beenden != 0) return;
        g_beenden = 1;
        ui_log(@"Oberflaeche: Beenden ohne Rueckfrage - Verbindung zum Zuschauer wird geschlossen");
        void (*abschied)(void) = g_cfg.abschied;
        dispatch_semaphore_t fertig = dispatch_semaphore_create(0);
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            if (abschied) abschied();
            dispatch_semaphore_signal(fertig);
        });
        dispatch_semaphore_wait(fertig, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
        g_beenden = 2;
    }];
}

void qc_oberflaeche_starten(const qc_oberflaeche_cfg *cfg) {
    if (atomic_load(&g_ui_da)) return;
    g_cfg = *cfg;
    g_eingebettet = cfg->eingebettet != 0;
    atomic_store(&g_app, cfg->app != NULL);
    g_anfragen = [[QCAnfragen alloc] init];
    g_ui = [[QCOberflaeche alloc] init];
    if (g_eingebettet) {
        an_winits_delegate_haengen();
        ende_beobachten();
    } else {
        NSApp.delegate = g_ui;
    }
    signale_einrichten();
    // Ab hier kann jemand am Mac "Zulassen" klicken: Anfragen, bevor die
    // Run-Loop laeuft, warten auf der Main Queue und erscheinen danach.
    atomic_store(&g_ui_da, 1);
}

void qc_oberflaeche_fertig(void) {
    static BOOL gelaufen = NO;          // nur auf dem Hauptfaden
    if (!atomic_load(&g_ui_da) || gelaufen) return;
    gelaufen = YES;
    qc_texte_systemsprache();
    // Die eine App: winit startet ohne sein Standardmenue - das
    // Programmmenue (Beenden) und die Tasten des Bearbeiten-Menues kommen
    // von hier. Sonst bringt ein eingebetteter Client sein eigenes
    // Hauptmenue mit (winit) - das bleibt.
    if (g_cfg.app) {
        BOOL sitzung;
        @synchronized (app_sperre()) { sitzung = g_app_sitzung; }
        NSApp.mainMenu = qc_programmmenue_bauen(g_ui, sitzung, g_app_schliessen_titel);
        g_programm_sprache = qc_texte_aktuell();
    } else if (!g_eingebettet || !NSApp.mainMenu) {
        bearbeiten_menue_setzen();
    }
    // Vorsorge gegen App Nap: eine Menueleisten-App ohne sichtbares Fenster
    // koennte das System drosseln (Timer gebuendelt, Faeden niedriger
    // eingestuft) - fuer einen Host, der jederzeit einen Strom liefern soll,
    // nicht gewollt. UNGEPRUEFT, ob es ohne dies messbar drosselte; der
    // Ruhezustand des Macs bleibt erlaubt (dafuer gibt es die eigene
    // Zusicherung waehrend eines Stroms).
    g_aktivitaet = [[NSProcessInfo processInfo] beginActivityWithOptions:NSActivityUserInitiatedAllowingIdleSystemSleep
                                                                  reason:@"QuadChroma-Host nimmt Verbindungen an"];
    ui_log(@"Oberflaeche: Menueleiste, Sprache %s%s", qc_texte_code(qc_texte_aktuell()),
           g_cfg.app ? " (die eine App)" : g_eingebettet ? " (eingebettet)" : "");
    zustand_auffrischen();   // das Symbol entsteht mit dem ersten gelesenen Zustand
}

// ------------------------------------------------ Rueckrufe aus dem Kern

// name gilt nur waehrend des Aufrufs (zugang.h): er wird hier, noch im
// Faden des Kerns, in einen NSString kopiert; alles Weitere laeuft per
// dispatch_async auf der Main Queue (der Kern haelt dabei seine Sperre).
void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t geraete_id, uint32_t code) { @autoreleasepool {
    QCAnfrage *a = [[QCAnfrage alloc] init];
    a.nr = anfrage;
    a.name = (name ? [NSString stringWithUTF8String:name] : nil) ?: @"?";
    a.geraeteId = geraete_id;
    a.code = code;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!g_anfragen) return;
        [g_anfragen neu:a];
        anfrage_abgleichen();
    });
}}

void qc_ui_anfrage_zurueck(uint64_t anfrage) {
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!g_anfragen) return;
        [g_anfragen weg:anfrage];
        anfrage_abgleichen();
    });
}

void qc_ui_zustand_geaendert(void) {
    zustand_auffrischen();
}

int qc_ui_vorhanden(void) {
    return atomic_load(&g_ui_da);
}

// ------------------------------------------------ Die eine App (dienst.h)

void qc_app_stand_setzen(const qc_app_stand *s) {
    if (!s) return;
    NSMutableArray<NSArray<NSString *> *> *hosts = [NSMutableArray array];
    for (int i = 0; i < s->hosts && i < QC_HOSTS_MAX; i++) {
        const char *n = s->host_namen ? s->host_namen[i] : NULL, *a = s->host_adressen ? s->host_adressen[i] : NULL;
        NSString *adresse = a ? [NSString stringWithUTF8String:a] : nil;
        if (!adresse.length) continue;
        [hosts addObject:@[ (n ? [NSString stringWithUTF8String:n] : nil) ?: @"", adresse ]];
    }
    NSString *tip = (s->tooltip ? [NSString stringWithUTF8String:s->tooltip] : nil) ?: @"QuadChroma";
    NSString *ruhe_grund = (s->ruhe_grund ? [NSString stringWithUTF8String:s->ruhe_grund] : nil) ?: @"";
    NSString *schliessen = (s->schliessen_titel ? [NSString stringWithUTF8String:s->schliessen_titel] : nil) ?: @"";
    BOOL sitzung_neu;
    @synchronized (app_sperre()) {
        sitzung_neu = g_app_sitzung != (s->sitzung != 0);
        g_app_freigabe = s->freigabe != 0;
        g_app_ruhe = s->ruhe != 0;
        g_app_ruhe_grund = ruhe_grund;
        g_app_sitzung = s->sitzung != 0;
        g_app_hosts = [hosts copy];
        g_app_tooltip = tip;
    }
    if (![NSThread isMainThread]) return;
    if (g_item && ![g_item.button.toolTip isEqualToString:tip]) g_item.button.toolTip = tip;
    // In einer Sitzung gehoeren Cmd+Q und Cmd+H dem Mac drueben; eine neue
    // Sprache (qc_texte_setzen_code) braucht den neuen Titel von "Beenden";
    // steht das Symbol (nicht mehr), wird aus "Beenden" "Fenster schliessen"
    // und umgekehrt.
    BOOL schliessen_neu = ![schliessen isEqualToString:g_app_schliessen_titel ?: @""];
    g_app_schliessen_titel = schliessen;
    if ((sitzung_neu || schliessen_neu || g_programm_sprache != qc_texte_aktuell()) && g_cfg.app && NSApp.mainMenu) {
        NSApp.mainMenu = qc_programmmenue_bauen(g_ui, s->sitzung != 0, schliessen);
        g_programm_sprache = qc_texte_aktuell();
    }
    zustand_auffrischen();
}

// ------------------------------------------------ Beim Anmelden starten (dienst.h)

int qc_anmeldung_stand(void) {
    return (int)anmelden_lesen();
}

void qc_anmeldung_umschalten(void) {
    anmelden_umschalten(anmelden_lesen());
}

// Wann sich dieser Nutzer zuletzt an der Konsole angemeldet hat (utmpx,
// USER_PROCESS auf "console" - so zeigt es auch who); 0 = unbekannt.
static int64_t konsole_anmeldung(void) {
    struct passwd *pw = getpwuid(getuid());
    if (!pw || !pw->pw_name) return 0;
    int64_t zuletzt = 0;
    setutxent();
    struct utmpx *u;
    while ((u = getutxent())) {
        if (u->ut_type != USER_PROCESS) continue;
        if (strncmp(u->ut_line, "console", sizeof u->ut_line) != 0) continue;
        if (strncmp(u->ut_user, pw->pw_name, sizeof u->ut_user) != 0) continue;
        if (u->ut_tv.tv_sec > zuletzt) zuletzt = u->ut_tv.tv_sec;
    }
    endutxent();
    return zuletzt;
}

void qc_anmeldestart_lesen(qc_anmeldestart_info *info) {
    if (!info) return;
    memset(info, 0, sizeof *info);
    NSAppleEventDescriptor *e = [[NSAppleEventManager sharedAppleEventManager] currentAppleEvent];
    if (e) {
        info->ereignis_klasse = e.eventClass;
        info->ereignis = e.eventID;
        info->eigenschaft = [e paramDescriptorForKeyword:keyAEPropData].enumCodeValue;
    }
    info->anmeldung = (int)anmelden_lesen();
    int64_t seit = konsole_anmeldung();
    info->seit_anmeldung = seit > 0 ? (int64_t)time(NULL) - seit : -1;
}

int qc_anmeldestart_bewerten(const qc_anmeldestart_info *info) {
    if (!info) return QC_ANMELDESTART_NEIN;
    if (info->ereignis_klasse == kCoreEventClass && info->ereignis == kAEOpenApplication &&
        info->eigenschaft == keyAELaunchedAsLogInItem)
        return QC_ANMELDESTART_EREIGNIS;
    if (info->anmeldung == QC_ANMELDUNG_AN && info->seit_anmeldung >= 0 && info->seit_anmeldung <= QC_ANMELDESTART_FRIST)
        return QC_ANMELDESTART_ZEITNAH;
    return QC_ANMELDESTART_NEIN;
}

void *qc_menueleiste_menue(void) {
    return (__bridge void *)g_menue;
}

int qc_menueleiste_steht(void) {
    return g_item && g_item.button.window.screen ? 1 : 0;
}

int qc_menueleiste_hinweis(const char *text) {
    if (!text || !*text || !qc_menueleiste_steht()) return 0;
    [g_hinweis close];
    NSTextField *l = [NSTextField wrappingLabelWithString:[NSString stringWithUTF8String:text] ?: @""];
    l.preferredMaxLayoutWidth = 260;
    NSStackView *st = [NSStackView stackViewWithViews:@[ l ]];
    st.edgeInsets = NSEdgeInsetsMake(12, 14, 12, 14);
    NSViewController *vc = [[NSViewController alloc] init];
    vc.view = st;
    NSPopover *p = [[NSPopover alloc] init];
    p.behavior = NSPopoverBehaviorTransient;
    p.contentViewController = vc;
    p.contentSize = st.fittingSize;
    [p showRelativeToRect:g_item.button.bounds ofView:g_item.button preferredEdge:NSRectEdgeMinY];
    g_hinweis = p;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 6 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (g_hinweis == p) { [p close]; g_hinweis = nil; }
    });
    return 1;
}
