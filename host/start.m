// Der fruehere eigene Mac-Host: der Einstieg. Nur noch fuer Pruefstaende
// und Versuche (make host-allein, build/quadchroma-host) - QuadChroma.app
// ist die eine App, der Rust-Client mit eingebauter Engine
// (client/build.rs baut sie ohne diese Datei ein).
//
// Werkzeuge (--list, --capture, --formattest) laufen zu Ende; sonst startet
// der Dienst (main.m, dienst.h), und die Run-Loop gehoert AppKit.
#import <AppKit/AppKit.h>
#include <stdio.h>
#include "dienst.h"

int main(int argc, const char *argv[]) { @autoreleasepool {
    setvbuf(stdout, NULL, _IONBF, 0);
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];

    int werkzeug = qc_werkzeug(argc, argv);
    if (werkzeug != QC_KEIN_WERKZEUG) return werkzeug;

    qc_dienst_cfg cfg = { .eingebettet = 0, .argc = argc, .argv = argv };
    int r = qc_dienst_starten(&cfg);
    // Ein zweiter Start (Doppelklick, Anmeldeobjekt, "open -n") endet still.
    if (r == QC_DIENST_LAEUFT_SCHON) return 0;
    if (r != QC_DIENST_OK) return r;

    // Kehrt nicht zurueck: Beenden laeuft ueber [NSApp terminate:] (menue.m),
    // erst nach dem Abschied an den Zuschauer.
    [NSApp run];
    return 0;
}}
