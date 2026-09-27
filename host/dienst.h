// Der Mac-Host als startbarer Dienst: die C-Schnittstelle der Host-Engine.
//
// Zwei Einstiege nutzen sie. Die eigene App (QuadChroma.app, Makefile) hat
// ihr main in start.m: Werkzeuge, sonst qc_dienst_starten und [NSApp run].
// Der Rust-Client baut dieselbe Engine ohne start.m als libqchost.a ein
// (client/build.rs, Deklarationen in client/src/host_mac.rs); dort dreht
// winit die Run-Loop, und der Dienst startet eingebettet aus resumed.
//
// Faeden: qc_werkzeug, qc_dienst_starten und qc_oberflaeche_fertig nur auf
// dem Hauptfaden; qc_dienst_beenden aus jedem Faden.
//
// Nur C-Typen, damit Rust (und C) den Kopf ohne Objective-C lesen koennen.
#ifndef QC_DIENST_H
#define QC_DIENST_H

#ifdef __cplusplus
extern "C" {
#endif

// Rueckgaben von qc_dienst_starten. Nach LAEUFT_SCHON, FADEN und DATEI ist
// nichts gestartet, ein spaeterer Versuch ist erlaubt; nach OK liefert jeder
// weitere Aufruf DOPPELT.
#define QC_DIENST_OK            0
#define QC_DIENST_LAEUFT_SCHON  1   // ein anderer Host dieses Nutzers laeuft (host-instanz.lock)
#define QC_DIENST_DOPPELT       2   // in diesem Prozess schon gestartet
#define QC_DIENST_FADEN         3   // nicht auf dem Hauptfaden gerufen
#define QC_DIENST_DATEI         5   // host.key unlesbar oder nicht anlegbar, --capture-Datei nicht schreibbar

typedef struct {
    // 0: eigene App - die Oberflaeche (menue.m) wird NSApp.delegate.
    // 1: im Rust-Client - NSApp.delegate gehoert winit und bleibt unberuehrt;
    //    Doppelklick und Beenden haengt die Oberflaeche an dessen Klasse.
    int eingebettet;
    // Schalter wie auf der Kommandozeile, argv[0] ist der Programmname:
    // --serve [port], --fps N, --mbit N, --out BxH, --display N, --fest,
    // --capture <s> <datei> (der Strom geht zusaetzlich in die Datei).
    // argv NULL: keine Schalter, Standard-Port 9001.
    int argc;
    const char *const *argv;
    // Nur eingebettet: Doppelklick auf die laufende App (das Fenster des
    // Clients oeffnen). NULL: das Menue des Symbols zeigen, wie die eigene App.
    void (*oeffnen)(void);
} qc_dienst_cfg;

// Startet den Dienst ohne Run-Loop und kehrt zurueck: Einzelinstanz,
// Schluessel, Zugang, Oberflaeche (Symbol erst mit qc_oberflaeche_fertig),
// Annahme auf Bild-, Eingabe- und Bekanntgabeport, Takte. Fragt wie bisher
// nach den Freigaben (Bildschirmaufnahme, Bedienungshilfen). Die Aufnahme
// entsteht erst mit dem ersten Zuschauer.
int qc_dienst_starten(const qc_dienst_cfg *cfg);

// Vor dem Prozessende: der Zuschauer bekommt den Abschied (Typ 13, Grund 0),
// dann sind Bild- und Eingabekanal zu. Einmal je Prozess; weitere Aufrufe
// warten nur, bis der erste fertig ist. Blockiert hoechstens rund 3 s (Frist).
// Die Ports lauschen weiter, bis der Prozess endet. Ohne gestarteten Dienst
// ohne Wirkung. Die eigene App braucht den Aufruf nicht: jeder Weg zu
// [NSApp terminate:] fuehrt ueber applicationShouldTerminate: (menue.m).
// Eingebettet endet winits event_loop.exit() ohne diesen Weg - dann ruft
// der Client ihn selbst.
void qc_dienst_beenden(void);

// Was frueher in applicationDidFinishLaunching: stand: Sprache,
// Bearbeiten-Menue (eingebettet nur, wenn der Client keins hat),
// App-Nap-Vorsorge, Symbol in der Menueleiste. Die eigene App ruft es aus
// ihrem Delegate, eingebettet ruft es der Client aus resumed - nach
// qc_dienst_starten. Vorher und ein zweites Mal ohne Wirkung.
void qc_oberflaeche_fertig(void);

// Werkzeuge der Kommandozeile (--list, --formattest, --capture ohne --serve):
// laufen zu Ende und liefern den Exit-Code. Ohne Werkzeug-Schalter
// QC_KEIN_WERKZEUG, dann ist nichts geschehen.
#define QC_KEIN_WERKZEUG (-1)
int qc_werkzeug(int argc, const char *const argv[]);

#ifdef __cplusplus
}
#endif

#endif
