// Pruefprogramm fuer host/clipboard.m: Kennzeichnung des empfangenen Textes,
// Widerhall, Uebertragung dessen, was der Nutzer am Mac kopiert, Lesen nur
// mit Zuschauer (qc_clip_bedingung) und Dateiverweise (public.file-url) lesen
// und schreiben. Die Dateien dafuer liegen unter $TMPDIR.
//
//   clang -fobjc-arc -O2 -Wall -Ihost -mmacosx-version-min=14.0 \
//         -framework Foundation -framework AppKit host/ablagetest.m -o /tmp/ablagetest
//   /tmp/ablagetest
//
// clipboard.m wird hier eingebunden und arbeitet auf einer eigenen, benannten
// Ablage statt der allgemeinen: die Zwischenablage des Nutzers und ein
// laufender Host bleiben unberuehrt, und in /tmp/quadchroma-m1.log landet
// nichts (die Zeilen gehen nur nach stdout). Rueckgabe: Zahl der Fehler.

#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#include <stdio.h>
#include <string.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

static FILE *pruef_fopen(const char *pfad, const char *modus) {
    if (strcmp(pfad, "/tmp/quadchroma-m1.log") == 0) pfad = "/dev/null";
    return fopen(pfad, modus);
}

#define ABLAGE @"tech.quadchroma.ablagetest"
#define fopen pruef_fopen
#define generalPasteboard pasteboardWithName:ABLAGE
#include "clipboard.m"
#undef generalPasteboard
#undef fopen

static int g_fehler = 0;
static NSMutableArray<NSString *> *g_empfangen;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

// Was der Host zum Client schicken wuerde.
static void empfangen(const char *utf8, size_t len) {
    NSString *s = [[NSString alloc] initWithBytes:utf8 length:len encoding:NSUTF8StringEncoding];
    @synchronized (g_empfangen) { [g_empfangen addObject:s ?: @"?"]; }
}

static NSUInteger anzahl(void) {
    @synchronized (g_empfangen) { return g_empfangen.count; }
}

static NSString *zuletzt(void) {
    @synchronized (g_empfangen) { return g_empfangen.lastObject; }
}

// Schaut gerade jemand zu? Im Host ist das g_client_fd >= 0.
static _Atomic int g_zuschauer = 1;
static int zuschauer_da(void) { return atomic_load(&g_zuschauer); }

// Was der Host als Dateiliste zum Client schicken wuerde.
static NSMutableArray<NSArray<NSString *> *> *g_dateilisten;

static void dateien_empfangen(NSArray<NSString *> *pfade) {
    @synchronized (g_dateilisten) { [g_dateilisten addObject:pfade]; }
}

static NSUInteger dateilisten(void) {
    @synchronized (g_dateilisten) { return g_dateilisten.count; }
}

static NSArray<NSString *> *letzte_dateiliste(void) {
    @synchronized (g_dateilisten) { return g_dateilisten.lastObject; }
}

// Pfade vergleichen, wie das Dateisystem sie sieht (/var und /private/var).
static NSString *echt(NSString *p) {
    char r[PATH_MAX];
    return realpath(p.fileSystemRepresentation, r) ? @(r) : p;
}

static BOOL gleiche_dateien(NSArray<NSString *> *a, NSArray<NSString *> *b) {
    if (a.count != b.count) return NO;
    for (NSUInteger i = 0; i < a.count; i++)
        if (![echt(a[i]) isEqualToString:echt(b[i])]) return NO;
    return YES;
}

// Der Nutzer kopiert im Finder: je Datei ein Eintrag mit public.file-url und
// zusaetzlich dem Namen als Text. referenz: als Datei-Referenz-URL;
// verdeckt_an: dieser Eintrag traegt die Kennung "verdeckt".
static void dateien_kopieren(NSPasteboard *pb, NSArray<NSString *> *pfade, BOOL referenz, NSUInteger verdeckt_an) {
    NSMutableArray<NSPasteboardItem *> *items = [NSMutableArray array];
    for (NSUInteger i = 0; i < pfade.count; i++) {
        NSURL *u = [NSURL fileURLWithPath:pfade[i]];
        if (referenz) u = u.fileReferenceURL;
        NSPasteboardItem *it = [[NSPasteboardItem alloc] init];
        [it setString:u.absoluteString forType:NSPasteboardTypeFileURL];
        [it setString:pfade[i].lastPathComponent forType:NSPasteboardTypeString];
        if (i == verdeckt_an) [it setData:[NSData data] forType:@"org.nspasteboard.ConcealedType"];
        [items addObject:it];
    }
    [pb clearContents];
    [pb writeObjects:items];
}

static void laufen(double s) {
    [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:s]];
}

// Der Nutzer kopiert am Mac: ein Eintrag, wie ihn ein Programm ablegt.
static void kopieren(NSPasteboard *pb, NSString *text, NSString *kennung) {
    NSPasteboardItem *it = [[NSPasteboardItem alloc] init];
    [it setString:text forType:NSPasteboardTypeString];
    if (kennung) [it setData:[NSData data] forType:kennung];
    [pb clearContents];
    [pb writeObjects:@[it]];
}

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IONBF, 0);
        g_empfangen = [NSMutableArray array];
        g_dateilisten = [NSMutableArray array];
        NSPasteboard *pb = [NSPasteboard pasteboardWithName:ABLAGE];
        [pb clearContents];
        qc_clip_dateien(dateien_empfangen);
        qc_clip_start(empfangen);
        laufen(0.4);

        printf("\n-- Nutzer kopiert am Mac\n");
        kopieren(pb, @"vom Nutzer", nil);
        laufen(0.5);
        pruefe(anzahl() == 1 && [zuletzt() isEqualToString:@"vom Nutzer"], "wird uebertragen");

        printf("\n-- Text vom Client\n");
        qc_clip_set("vom Client", 10);
        laufen(0.5);
        NSPasteboardItem *it = pb.pasteboardItems.firstObject;
        NSArray *typen = it.types;
        printf("         (Eintraege %lu, Typen: %s)\n", (unsigned long)pb.pasteboardItems.count,
               [typen componentsJoinedByString:@", "].UTF8String);
        pruefe(pb.pasteboardItems.count == 1 && [[it stringForType:NSPasteboardTypeString] isEqualToString:@"vom Client"],
               "steht als einziger Eintrag in der Ablage");
        pruefe([typen containsObject:@"org.nspasteboard.TransientType"] &&
               [typen containsObject:@"org.nspasteboard.AutoGeneratedType"],
               "traegt TransientType und AutoGeneratedType (nicht in den Verlauf)");
        NSData *tr = [it dataForType:@"org.nspasteboard.TransientType"];
        NSData *ag = [it dataForType:@"org.nspasteboard.AutoGeneratedType"];
        pruefe(tr && ag && tr.length == 0 && ag.length == 0,
               "beide Kennungen mit leerem Inhalt, wie die Verabredung es vorsieht");
        pruefe(anzahl() == 1, "kein Widerhall zum Client");
        // Faellt der Zaehlerabgleich einmal aus (eine fremde Aenderung kam
        // dazwischen), liest die Abfrage den eigenen, gekennzeichneten Eintrag
        // wieder - der Lesefilter verwirft die Kennungen nicht, g_last faengt
        // den Widerhall ab. Die Abfrage laeuft auf der Hauptschleife, wie hier.
        g_self_change = -1;
        g_seen -= 1;
        laufen(0.5);
        pruefe(anzahl() == 1, "auch ohne Zaehlerabgleich kein Widerhall (g_last)");

        printf("\n-- Nutzer kopiert danach wieder am Mac\n");
        kopieren(pb, @"zweiter Text", nil);
        laufen(0.5);
        pruefe(anzahl() == 2 && [zuletzt() isEqualToString:@"zweiter Text"], "wird weiter uebertragen");
        kopieren(pb, @"vom Client", nil);
        laufen(0.5);
        pruefe(anzahl() == 3 && [zuletzt() isEqualToString:@"vom Client"],
               "auch derselbe Text wie vorher vom Client, wenn der Nutzer ihn neu kopiert");

        printf("\n-- Kennungen fremder Programme\n");
        kopieren(pb, @"geheim", @"org.nspasteboard.ConcealedType");
        laufen(0.5);
        pruefe(anzahl() == 3, "verdeckter Eintrag (Passwortverwalter) bleibt auf dem Mac");
        kopieren(pb, @"voruebergehend", @"org.nspasteboard.TransientType");
        laufen(0.5);
        printf("         (voruebergehender Eintrag eines anderen Programms: %s)\n",
               anzahl() == 4 ? "uebertragen" : "nicht uebertragen");

        // Wie im Dienstbetrieb (main.m): gelesen wird nur mit Zuschauer.
        printf("\n-- Nur mit Zuschauer (qc_clip_bedingung)\n");
        qc_clip_bedingung(zuschauer_da);
        atomic_store(&g_zuschauer, 1);
        NSUInteger n0 = anzahl();
        kopieren(pb, @"Text A", nil);
        laufen(0.5);
        pruefe(anzahl() == n0 + 1 && [zuletzt() isEqualToString:@"Text A"], "mit Zuschauer: uebertragen");
        atomic_store(&g_zuschauer, 0);
        kopieren(pb, @"Text B", nil);
        laufen(0.5);
        pruefe(anzahl() == n0 + 1 && ![g_last isEqualToString:@"Text B"],
               "ohne Zuschauer: weder gelesen noch gesendet");
        atomic_store(&g_zuschauer, 1);
        laufen(0.5);
        pruefe(anzahl() == n0 + 1, "die Aenderung ohne Zuschauer wird spaeter nicht nachgereicht");
        // Der Nutzer kopiert wieder den Text von vorhin. In der Ablage stand
        // inzwischen "Text B" - das ist eine echte Aenderung und muss hinaus,
        // auch zu einem Zuschauer, der "Text A" nie bekommen hat.
        kopieren(pb, @"Text A", nil);
        laufen(0.5);
        pruefe(anzahl() == n0 + 2 && [zuletzt() isEqualToString:@"Text A"],
               "danach mit Zuschauer: derselbe Text wie vor der Pause wird wieder uebertragen");
        NSUInteger n1 = anzahl();
        qc_clip_set("vom Client 2", 12);
        laufen(0.5);
        pruefe(anzahl() == n1, "auch danach kein Widerhall zum Client");
        qc_clip_bedingung(NULL);

        // Dateiverweise, nur mit Dateien unter $TMPDIR.
        printf("\n-- Dateiverweise (public.file-url)\n");
        const char *tmp = getenv("TMPDIR");
        NSString *vorlage = [[NSString stringWithUTF8String:tmp && *tmp ? tmp : "/tmp"]
                             stringByAppendingPathComponent:@"qc-ablagetest-XXXXXX"];
        char wurzel_c[PATH_MAX];
        strlcpy(wurzel_c, vorlage.fileSystemRepresentation, sizeof wurzel_c);
        NSString *wurzel = mkdtemp(wurzel_c) ? @(wurzel_c) : nil;
        NSString *eins = [wurzel stringByAppendingPathComponent:@"eins.txt"];
        NSString *zwei = [wurzel stringByAppendingPathComponent:@"zwei äö.txt"];
        NSString *ordner = [wurzel stringByAppendingPathComponent:@"Ordner"];
        BOOL angelegt = wurzel && [@"1" writeToFile:eins atomically:NO encoding:NSUTF8StringEncoding error:nil] &&
                        [@"2" writeToFile:zwei atomically:NO encoding:NSUTF8StringEncoding error:nil] &&
                        mkdir(ordner.fileSystemRepresentation, 0700) == 0;
        pruefe(angelegt, "Testdateien unter $TMPDIR angelegt");
        NSUInteger t0 = anzahl(), d0 = dateilisten();
        dateien_kopieren(pb, @[eins, zwei, ordner], NO, NSNotFound);
        laufen(0.5);
        printf("         (%lu Dateiliste(n), zuletzt %lu Pfad(e))\n", (unsigned long)(dateilisten() - d0),
               (unsigned long)letzte_dateiliste().count);
        pruefe(dateilisten() == d0 + 1 && gleiche_dateien(letzte_dateiliste(), @[eins, zwei, ordner]),
               "Finder-Kopie mit drei Eintraegen: alle drei Pfade kommen als Dateiliste, ueber alle Eintraege");
        pruefe(anzahl() == t0, "dabei geht kein Text hinaus, auch nicht der Dateiname");
        pruefe(g_last == nil, "g_last ist geleert");

        dateien_kopieren(pb, @[eins], YES, NSNotFound);
        laufen(0.5);
        pruefe(dateilisten() == d0 + 2 && gleiche_dateien(letzte_dateiliste(), @[eins]),
               "Datei-Referenz-URL (file:///.file/id=...) wird zum Pfad aufgeloest");

        dateien_kopieren(pb, @[eins, zwei], NO, 1);
        laufen(0.5);
        pruefe(dateilisten() == d0 + 2 && anzahl() == t0, "ein verdeckter Eintrag unter den Dateien: nichts wird gelesen");

        printf("\n-- Dateiverweise vom Client\n");
        qc_clip_set_dateien(@[eins, ordner]);
        laufen(0.5);
        NSArray<NSPasteboardItem *> *eintraege = pb.pasteboardItems;
        BOOL form = eintraege.count == 2;
        NSArray *soll = @[ [NSURL fileURLWithPath:eins].absoluteString, [NSURL fileURLWithPath:ordner].absoluteString ];
        for (NSUInteger i = 0; form && i < 2; i++) {
            NSPasteboardItem *e = eintraege[i];
            NSData *tr = [e dataForType:@"org.nspasteboard.TransientType"];
            NSData *ag = [e dataForType:@"org.nspasteboard.AutoGeneratedType"];
            form = [[e stringForType:NSPasteboardTypeFileURL] isEqualToString:soll[i]] &&
                   tr && ag && tr.length == 0 && ag.length == 0;
        }
        printf("         (Eintraege %lu, Typen des ersten: %s)\n", (unsigned long)eintraege.count,
               [eintraege.firstObject.types componentsJoinedByString:@", "].UTF8String);
        pruefe(form, "je Pfad ein Eintrag mit public.file-url, TransientType und AutoGeneratedType (leer)");
        NSArray<NSURL *> *gelesen = [pb readObjectsForClasses:@[[NSURL class]] options:nil];
        pruefe(gelesen.count == 2 && gleiche_dateien(@[gelesen[0].path, gelesen[1].path], @[eins, ordner]),
               "ein Programm liest sie als zwei Datei-URLs zurueck (wie Finder beim Einfuegen)");
        pruefe(dateilisten() == d0 + 2 && anzahl() == t0 && g_last == nil,
               "kein Widerhall zum Client, weder als Dateiliste noch als Text");

        // Nach Dateien gilt der zuletzt gesehene Text nicht mehr: derselbe
        // Text wie vor den Dateien geht wieder hinaus.
        kopieren(pb, @"vom Client 2", nil);
        laufen(0.5);
        pruefe(anzahl() == t0 + 1 && [zuletzt() isEqualToString:@"vom Client 2"],
               "nach Dateien geht derselbe Text wie zuvor wieder hinaus (g_last geleert)");

        qc_clip_bedingung(zuschauer_da);
        atomic_store(&g_zuschauer, 0);
        dateien_kopieren(pb, @[zwei], NO, NSNotFound);
        laufen(0.5);
        pruefe(dateilisten() == d0 + 2 && g_last == nil, "ohne Zuschauer: Dateiverweise weder gelesen noch gemeldet");
        atomic_store(&g_zuschauer, 1);
        qc_clip_bedingung(NULL);
        if (wurzel) [[NSFileManager defaultManager] removeItemAtPath:wurzel error:nil];

        [pb releaseGlobally];
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
