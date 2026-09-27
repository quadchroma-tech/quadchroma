// Oberflaechentexte des Mac-Hosts (Menueleiste, Zulassen-Fenster,
// Passwort-Fenster). Nur ueber diese Schluessel - kein Text steht im Code.
//
// 29 Sprachen mit denselben Codes und in derselben Reihenfolge wie die
// Tabellen des Clients (client/src/strings_more.rs, ALL). Die Schluessel
// QCTextHost... entsprechen den Host-Schluesseln Host... in
// client/src/strings.rs (Spezifikation Pairing, Abschnitt 14); wo ein Text
// gleich dem eines Client-Schluessels ist (QCTextAccessCode, QCTextAccessCancel),
// gilt dieselbe Uebersetzung; ebenso fuer die Texte des Passwort-Fensters
// (QCTextHostPasswordNotSaved, ...Invalid, ...Title). Neue Schluessel kommen
// ans Ende des Enums und in jede Tabelle - der Pruefstand menuetest prueft,
// dass jede Tabelle jeden Text hat und dieselben Platzhalter wie Englisch;
// der Client-Test
// zugang_texte_wie_mac_host (client/src/strings.rs) prueft, dass jeder Text
// mit Client-Gegenstueck in allen 29 Sprachen wortgleich dort steht.
//
// Platzhalter: {n} Name, {i} ID ("ddd ddd ddd"), {c} Code ("628 306"),
// {s} Sekunden, {p} Passwort bzw. Port, {d} Datum.
#import <Foundation/Foundation.h>

typedef enum QCText {
    QCTextHostReady,
    QCTextHostConnected,
    QCTextHostDeviceId,
    QCTextHostPassword,
    QCTextHostCopied,
    QCTextHostChangePassword,
    QCTextHostRandomPassword,
    QCTextHostNewPassword,
    QCTextHostRepeatPassword,
    QCTextHostPasswordsDiffer,
    QCTextHostPasswordShort,
    QCTextHostPasswordSaved,
    QCTextHostPasswordUnreadable,
    QCTextHostDevices,
    QCTextHostNoDevices,
    QCTextHostDeviceLine,
    QCTextHostRemove,
    QCTextHostRemoveAll,
    QCTextHostRemoveAllAsk,
    QCTextHostListDamaged,
    QCTextHostListReset,
    QCTextHostRequest,
    QCTextHostAllow,
    QCTextHostDeny,
    QCTextHostStartLogin,
    QCTextHostStartWindows,       // nur Windows-Host-Rolle; hier ungenutzt, der Vollstaendigkeit halber
    QCTextHostMoveToApps,
    QCTextHostScreenMissing,
    QCTextHostAccessMissing,
    QCTextHostPortBusy,
    QCTextHostQuit,
    QCTextHostStopSharing,        // nur Windows-Host-Rolle; hier ungenutzt
    QCTextHostTooltip,            // nur Windows-Host-Rolle; hier ungenutzt
    QCTextHostOk,
    // Nur im Mac-Host, gleicher Text wie der Client-Schluessel gleichen Namens:
    QCTextAccessCode,             // Code im Zulassen-Fenster
    QCTextAccessCancel,           // Abbrechen im Passwort-Fenster und bei der Rueckfrage
    // Passwort-Fenster, gleicher Text wie der Client-Schluessel gleichen
    // Namens (die Windows-Host-Rolle braucht sie ebenso):
    QCTextHostPasswordNotSaved,   // qc_zugang_passwort_setzen meldet einen Schreibfehler
    QCTextHostPasswordInvalid,    // zu lang oder Zeilenumbruch (qc_passwort_pruefen 3, Kern -1)
    QCTextHostPasswordTitle,      // Titel des Passwort-Fensters
    // Die eine App (eingebettet im Client): die Punkte des Clients im Menue,
    // die Kopfzeile ohne Freigabe und das Fenster "Geraetename" - gleicher
    // Text wie der Client-Schluessel gleichen Namens.
    QCTextTrayOpenApp,            // "QuadChroma oeffnen"
    QCTextTrayConnect,            // "Verbinden: {n}" je gefundenem Host
    QCTextHostSharingIsOff,       // Zustandszeile, wenn die Freigabe aus ist
    QCTextStartShareMac,          // Haken "Diesen Mac freigeben"
    QCTextDeviceNameChange,       // Menuepunkt, oeffnet das Fenster "Geraetename"
    QCTextDeviceNameTitle,        // dessen Titel
    QCTextDeviceNameLabel,        // Beschriftung des Felds
    QCTextDeviceNameHint,         // leer heisst Rechnername ({n})
    QCTextDeviceNameTooLong,      // mehr als 40 Byte
    QCTextDeviceNameInvalid,      // Steuer- oder Richtungszeichen
    QCTextPreventSleep,           // Haken "Ruhezustand verhindern, solange QuadChroma laeuft"
    QCTextAnzahl
} QCText;

// Anzahl der Sprachen und ihre Codes/Namen in Tabellenreihenfolge (0 = en).
int qc_texte_sprachen(void);
const char *qc_texte_code(int sprache);
const char *qc_texte_name(int sprache);

// Roher Eintrag einer Tabelle, NULL wenn er fehlt (nur fuer den Pruefstand;
// die Oberflaeche nimmt qc_text, das auf Englisch zurueckfaellt).
const char *qc_texte_eintrag(int sprache, QCText t);

// Sprachwahl: je bevorzugter Sprache (in Reihenfolge) erst der genaue Code,
// dann der Teil vor '-'/'_'; passt keine, Englisch. Rueckgabe: Tabellenplatz.
int qc_texte_waehlen(NSArray<NSString *> *bevorzugt);
// Waehlt nach [NSLocale preferredLanguages] und setzt die Sprache.
void qc_texte_systemsprache(void);
// Setzt die Sprache (Tabellenplatz); ungueltig -> Englisch.
void qc_texte_setzen(int sprache);
// Setzt die Sprache nach ihrem Code ("de", "pt", wie die Tabellen des
// Clients) - so folgen die Texte der Oberflaeche der Sprachwahl des Clients
// (die eine App). Danach waehlt qc_texte_systemsprache nicht mehr neu.
// Rueckgabe: Tabellenplatz, -1 fuer einen unbekannten Code (dann bleibt die
// Sprache, wie sie war).
int qc_texte_setzen_code(const char *code);
int qc_texte_aktuell(void);

// Text in der aktuellen Sprache (fehlt er dort: Englisch).
NSString *qc_text(QCText t);
// Dasselbe mit ersetzten Platzhaltern: werte bildet "n", "i", "c", "s", "p",
// "d" auf den Wert ab. Ein Durchgang: ein Wert, der selbst wie ein
// Platzhalter aussieht (ein Geraetename "{i}"), bleibt stehen, wie er ist.
NSString *qc_text_mit(QCText t, NSDictionary<NSString *, NSString *> *werte);
