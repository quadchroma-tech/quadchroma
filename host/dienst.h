// Der Mac-Host als startbarer Dienst: die C-Schnittstelle der Host-Engine.
//
// Zwei Einstiege nutzen sie. QuadChroma.app ist die eine App: der
// Rust-Client baut die Engine ohne start.m als libqchost.a ein
// (client/build.rs, Deklarationen in client/src/host_mac.rs); dort dreht
// winit die Run-Loop, und die Engine laeuft eingebettet aus resumed an. Der
// fruehere eigene Host (start.m: Werkzeuge, sonst qc_dienst_starten und
// [NSApp run]) baut nur noch fuer die Pruefstaende (make host-allein).
//
// Die eine App (Client und Host in einem Prozess, Plan M4): der Client
// richtet in resumed mit qc_app_einrichten Oberflaeche, Schluessel und
// Zugang ein - das Symbol in der Menueleiste traegt dann auch seine Punkte
// (Rueckruf app) -, startet mit eingeschalteter Freigabe qc_dienst_starten
// und ruft qc_oberflaeche_fertig. Die Freigabe schaltet er mit
// qc_dienst_anhalten und qc_dienst_fortsetzen (bzw. dem ersten
// qc_dienst_starten) um.
//
// Faeden: qc_werkzeug, qc_app_einrichten, qc_dienst_starten,
// qc_oberflaeche_fertig und die Funktionen der Menueleiste nur auf dem
// Hauptfaden; qc_dienst_beenden, qc_dienst_anhalten, qc_dienst_fortsetzen
// und qc_dienst_name_setzen aus jedem Faden.
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
    // Nur die eine App: die Punkte des Clients im Menue des Symbols (QC_APP_*
    // unten, wert nur fuer VERBINDEN und NAME, gilt nur waehrend des
    // Aufrufs). Auf dem Hauptfaden. NULL: nur das Menue des Hosts.
    void (*app)(int was, const char *wert);
    // Nur die eine App: Pruefung im Fenster "Geraetename" (UTF-8), dieselbe
    // wie im Client: 0 gut (auch leer = Rechnername), 1 zu lang (ueber 40
    // Byte), 2 unzulaessige Zeichen. NULL: alles gilt als gut.
    int (*name_pruefen)(const char *eingabe);
} qc_dienst_cfg;

// Was der Rueckruf app meldet.
#define QC_APP_OEFFNEN    1   // "QuadChroma oeffnen": das Fenster des Clients
#define QC_APP_VERBINDEN  2   // "Verbinden: <Host>", wert = Adresse
#define QC_APP_FREIGABE   3   // Haken "Diesen Mac freigeben" umschalten
#define QC_APP_RUHE       4   // Haken "Ruhezustand verhindern" umschalten
#define QC_APP_NAME       5   // neuer Geraetename aus dem Fenster, wert = Eingabe (leer = Rechnername)
#define QC_APP_BEENDEN    6   // "QuadChroma beenden" (Menue, Cmd+Q)

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

// Nur die eine App: Oberflaeche (eingebettet, mit den Punkten des Clients),
// Schluessel und Zugang einrichten, ohne zu lauschen und ohne Rueckfrage
// des Systems - so zeigt das Menue ID, Passwort und Geraete auch bei
// ausgeschalteter Freigabe. Das Protokoll des Hosts geht dann nach
// host-protokoll.txt im Ablageordner (statt /tmp/quadchroma-m1.log). Danach qc_dienst_starten (Freigabe an) und
// qc_oberflaeche_fertig. Rueckgabe QC_DIENST_OK, _FADEN, _DATEI oder
// _DOPPELT (schon eingerichtet).
int qc_app_einrichten(const qc_dienst_cfg *cfg);

// Freigabe aus: ein Zuschauer bekommt den Abschied (Typ 13, Grund 1 "Freigabe
// ausgeschaltet"), Aufnahme und Encoder gehen ab, Bild-, Eingabe- und
// Bekanntgabeport schliessen. Wer gerade im Handschlag ist, wird nicht mehr
// Zuschauer. Zugang, Oberflaeche und Waechter bleiben. Kehrt sofort zurueck
// (die Arbeit laeuft auf einer eigenen Warteschlange). Ohne gestarteten
// Dienst ohne Wirkung.
void qc_dienst_anhalten(void);
// Freigabe wieder an nach qc_dienst_anhalten: die Ports lauschen wieder, die
// Bekanntgabe laeuft. Kehrt sofort zurueck. Ohne gestarteten Dienst ohne
// Wirkung - das erste Einschalten ist qc_dienst_starten.
void qc_dienst_fortsetzen(void);
// 1, sobald qc_dienst_starten gelungen ist.
int qc_dienst_gestartet(void);

// Der Geraetename (die eine App: geraetename= in einstellungen.txt) statt
// des Rechnernamens: in der Bekanntgabe ab ihrer naechsten Runde, in
// Nachricht 20 ab der naechsten Zugangsphase, im Kopf des Menues sofort.
// NULL oder leer: wieder der Rechnername aus den Systemeinstellungen.
void qc_dienst_name_setzen(const char *name);

// ------------------------------------------------ Menueleiste der einen App
// (menue.m, nur auf dem Hauptfaden)

// Was nur der Client weiss: sein Haken der Freigabe (der Wunsch - die
// Zustandszeile sagt, was wirklich ist), "Ruhezustand verhindern", die
// gefundenen Hosts fuer "Verbinden: <Host>" (hoechstens 4, name darf leer
// sein), der Tooltip und ob eine Sitzung laeuft (dann gehen Cmd+Q und Cmd+H
// an den Mac drueben statt an das Programmmenue). Die Texte gelten nur
// waehrend des Aufrufs. Das Menue wird nur neu gebaut, wenn sich etwas
// aendert.
typedef struct {
    int freigabe;
    int ruhe;
    int sitzung;
    const char *tooltip;
    int hosts;
    const char *const *host_namen;
    const char *const *host_adressen;
} qc_app_stand;
void qc_app_stand_setzen(const qc_app_stand *s);

// Die einmalige Hinweisblase unter dem Symbol (etwa "QuadChroma laeuft in
// der Menueleiste weiter."), sechs Sekunden. 1 = gezeigt; 0, wenn das
// Symbol nicht zu sehen ist (volle Menueleiste) oder es keins gibt.
int qc_menueleiste_hinweis(const char *text);
// 1 = das Symbol steht sichtbar in der Menueleiste.
int qc_menueleiste_steht(void);
// Das Fenster "Geraetename" (auch aus dem Startbildschirm: "Umbenennen").
void qc_geraetename_fenster(void);
// Fuer den Selbsttest des Clients (--menueleiste-selbsttest): das NSMenu des
// Symbols, NULL solange es kein Symbol gibt.
void *qc_menueleiste_menue(void);

// Werkzeuge der Kommandozeile (--list, --formattest, --capture ohne --serve):
// laufen zu Ende und liefern den Exit-Code. Ohne Werkzeug-Schalter
// QC_KEIN_WERKZEUG, dann ist nichts geschehen.
#define QC_KEIN_WERKZEUG (-1)
int qc_werkzeug(int argc, const char *const argv[]);

#ifdef __cplusplus
}
#endif

#endif
