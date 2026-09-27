// Pruefprogramm fuer die eingebettete Oberflaeche (menue.m), so wie sie im
// Rust-Client laeuft: dort besitzt winit NSApp.delegate. Ein Nachbau von
// winits Delegate (nur applicationDidFinishLaunching: und
// applicationWillTerminate:, wie WinitApplicationDelegate in winit 0.30.13)
// ist Delegate von NSApp; aus seinem applicationDidFinishLaunching: - dort
// loest winit resumed aus - startet die Oberflaeche eingebettet.
//
// Geprueft: NSApp.delegate bleibt der Nachbau; seine Klasse antwortet danach
// auf applicationShouldHandleReopen:hasVisibleWindows: und
// applicationShouldTerminate:; ein zweiter Start der Oberflaeche aendert
// nichts; ein Reopen (Apple Event an sich selbst, wie ein Doppelklick auf die
// laufende App) ruft oeffnen; [NSApp terminate:] geht ueber den Abschied -
// genau einmal, ausserhalb des Hauptfadens - und erst danach in
// applicationWillTerminate: des Nachbaus. qc_oberflaeche_fertig bleibt aus:
// es legte ein Symbol in die Menueleiste.
//
// Braucht eine Anmeldesitzung mit Fensterserver (wie ablagetest), aber keine
// TCC-Freigabe. Die App ist "Prohibited" (kein Dock-Symbol, kein Menue), es
// erscheint nichts. Den Kern (zugang.h) stellt der Pruefstand als Attrappe
// wie menuetest.
//
//   clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
//         -framework Foundation -framework AppKit -framework ServiceManagement -framework CoreServices \
//         host/einbettungstest.m host/menue.m host/texte.m -o build/einbettungstest
//   build/einbettungstest
//
// Rueckgabe: Zahl der Fehler. Nach applicationWillTerminate: beendet AppKit
// den Prozess selbst; das Ergebnis setzt deshalb ein atexit-Haken. Haengt
// etwas, endet der Lauf nach 20 s mit 99.
#import <AppKit/AppKit.h>
#import <CoreServices/CoreServices.h>
#import "menue.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <unistd.h>

static _Atomic int g_fehler = 0;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) atomic_fetch_add(&g_fehler, 1);
}

// ------------------------------------------------------------ Kern-Attrappe
// Nur damit menue.m bindet; der Zustand wird hier nie gelesen (ohne
// qc_oberflaeche_fertig kein Menue).

uint32_t qc_zugang_eigene_id(void) { return 581729911; }
int qc_zugang_passwort(char *puffer, size_t groesse) { strlcpy(puffer, "k7m-4wq-9tz", groesse); return 0; }
int qc_zugang_passwort_setzen(const char *pw) { return 0; }
int qc_zugang_passwort_zufall(void) { return 0; }
int qc_zugang_geraete(qc_geraet *liste, int max) { return 0; }
int qc_zugang_geraet_entfernen(const uint8_t pub[32]) { return 0; }
int qc_zugang_alle_entfernen(void) { return 0; }
int qc_zugang_liste_zuruecksetzen(void) { return 0; }
void qc_zugang_entscheiden(uint64_t anfrage, int zulassen) {}
int qc_zustand_zuschauer(char *name, size_t groesse) { return 0; }
int qc_zustand_bildschirmfreigabe(void) { return 1; }
int qc_zustand_bedienungshilfen(void) { return 1; }
int qc_zustand_port_belegt(void) { return 0; }

// ------------------------------------------------------------ Rueckrufe

static _Atomic int g_abschied = 0;             // Aufrufe des Abschieds
static _Atomic int g_abschied_haupt = 0;       // davon auf dem Hauptfaden (soll 0 bleiben)
static int g_oeffnen = 0, g_oeffnen_zweiter = 0;
static int g_ende_gesehen = 0;                 // applicationWillTerminate: des Nachbaus

static void abschied(void) {
    if ([NSThread isMainThread]) atomic_fetch_add(&g_abschied_haupt, 1);
    usleep(100 * 1000);                        // wie ein Abschied, der kurz auf das Senden wartet
    atomic_fetch_add(&g_abschied, 1);
}
static void protokoll(NSString *zeile) { printf("        | %s\n", zeile.UTF8String); }
static void oeffnen(void) { g_oeffnen++; }
static void oeffnen_zweiter(void) { g_oeffnen_zweiter++; }

static void ergebnis(void) {
    pruefe(atomic_load(&g_abschied) == 1, "Abschied genau einmal (kein zweiter aus dem Beobachter des Beendens)");
    pruefe(g_ende_gesehen, "applicationWillTerminate: des Nachbaus ist gelaufen");
    int f = atomic_load(&g_fehler);
    printf("\n%s: %d Fehler\n", f ? "NICHT BESTANDEN" : "bestanden", f);
    _exit(f);
}

// Wie ein Doppelklick im Finder auf die laufende App: kAEReopenApplication an
// den eigenen Prozess.
static OSErr reopen_senden(void) {
    ProcessSerialNumber psn = { 0, kCurrentProcess };
    NSAppleEventDescriptor *ziel = [NSAppleEventDescriptor descriptorWithDescriptorType:typeProcessSerialNumber
                                                                                  bytes:&psn length:sizeof psn];
    NSAppleEventDescriptor *ev = [NSAppleEventDescriptor appleEventWithEventClass:kCoreEventClass
                                                                         eventID:kAEReopenApplication
                                                                targetDescriptor:ziel
                                                                        returnID:kAutoGenerateReturnID
                                                                   transactionID:kAnyTransactionID];
    return AESendMessage(ev.aeDesc, NULL, kAENoReply, kAEDefaultTimeout);
}

// ------------------------------------------------------------ Nachbau von winits Delegate

@interface QCWinitNachbau : NSObject <NSApplicationDelegate>
@end

@implementation QCWinitNachbau

- (void)applicationDidFinishLaunching:(NSNotification *)n {
    printf("\n-- Start der Oberflaeche aus applicationDidFinishLaunching: (dort ruft winit resumed)\n");
    qc_oberflaeche_cfg cfg = { .abschied = abschied, .protokoll = protokoll, .eingebettet = 1, .oeffnen = oeffnen };
    qc_oberflaeche_starten(&cfg);
    pruefe(NSApp.delegate == self, "NSApp.delegate bleibt winits Delegate");
    pruefe([self respondsToSelector:@selector(applicationShouldHandleReopen:hasVisibleWindows:)],
           "winits Delegate antwortet auf applicationShouldHandleReopen:hasVisibleWindows:");
    pruefe([self respondsToSelector:@selector(applicationShouldTerminate:)],
           "winits Delegate antwortet auf applicationShouldTerminate:");
    pruefe(qc_ui_vorhanden() == 1, "Zulassen moeglich (qc_ui_vorhanden)");
    qc_oberflaeche_cfg zweiter = { .abschied = abschied, .protokoll = protokoll, .eingebettet = 0, .oeffnen = oeffnen_zweiter };
    qc_oberflaeche_starten(&zweiter);
    pruefe(NSApp.delegate == self, "ein zweiter Start (auch nicht eingebettet) setzt kein Delegate");

    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        printf("\n-- Doppelklick auf die laufende App\n");
        OSErr e = reopen_senden();
        pruefe(e == noErr, "Reopen-Ereignis an sich selbst gesendet");
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 300 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            pruefe(g_oeffnen == 1, "Reopen ruft oeffnen des Clients genau einmal");
            pruefe(g_oeffnen_zweiter == 0, "der Rueckruf des zweiten Starts bleibt unbenutzt");
            printf("\n-- Beenden\n");
            // Wie der Menuepunkt: terminate: als Block der Run-Loop, nicht in
            // einem Block der Main Queue (siehe signale_einrichten in menue.m).
            CFRunLoopRef haupt = CFRunLoopGetMain();
            CFRunLoopPerformBlock(haupt, kCFRunLoopCommonModes, ^{ [NSApp terminate:nil]; });
            CFRunLoopWakeUp(haupt);
        });
    });
}

- (void)applicationWillTerminate:(NSNotification *)n {
    g_ende_gesehen = 1;
    pruefe(atomic_load(&g_abschied) == 1, "Abschied vor applicationWillTerminate: fertig");
    pruefe(atomic_load(&g_abschied_haupt) == 0, "Abschied nicht auf dem Hauptfaden");
}

@end

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IONBF, 0);
        atexit(ergebnis);
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 20 * NSEC_PER_SEC), dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
            printf("FEHLER  nach 20 s nicht beendet\n");
            _exit(99);
        });
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        QCWinitNachbau *winit = [[QCWinitNachbau alloc] init];
        NSApp.delegate = winit;
        printf("-- Vor dem Start der Oberflaeche\n");
        pruefe(![winit respondsToSelector:@selector(applicationShouldTerminate:)],
               "der Nachbau hat anfangs kein applicationShouldTerminate: (wie winit)");
        [NSApp run];
        printf("FEHLER  [NSApp run] kehrte zurueck\n");
        return 1;
    }
}
