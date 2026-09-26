// Pruefprogramm fuer die Oberflaeche des Mac-Hosts, ohne etwas anzuzeigen und
// ohne TCC-Freigabe: Texte (29 Tabellen in der Reihenfolge des Clients, jede
// mit jedem Text, dieselben Platzhalter wie Englisch, Englisch und Deutsch
// wortgleich mit der Spezifikation, keine doppelten Texte), Sprachwahl,
// Platzhalter in einem Durchgang, Menue-Modell fuer verschiedene Zustaende
// (als Titelliste), Zustand aus dem Kern (Attrappe), NSMenu aus dem Modell,
// Symbol der Menueleiste, Pruefung im Passwort-Fenster, Warteschlange der
// Zulassen-Anfragen.
//
//   clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
//         -framework Foundation -framework AppKit -framework ServiceManagement \
//         host/menuetest.m host/menue.m host/texte.m -o build/menuetest
//   build/menuetest
//
// Den Kern (zugang.h) stellt dieser Pruefstand selbst als Attrappe - er bindet
// weder host/zugang.c noch einen Platzhalter. Rueckgabe: Zahl der Fehler.
#import <AppKit/AppKit.h>
#import "menue.h"
#import "texte.h"

static int g_fehler = 0;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

// ------------------------------------------------------------ Kern-Attrappe

static uint32_t f_id = 581729911;
static int f_pw_da = 1;
static char f_pw[64] = "k7m-4wq-9tz";
static int f_anzahl = 0;                // -1 = beschaedigt
static qc_geraet f_geraete[4];
static int f_verbunden = 0;
static char f_zuschauer[64] = "";
static int f_bildschirm = 1, f_bedienung = 1;
static int f_port = 0;                  // belegter Bildport, 0 = frei

uint32_t qc_zugang_eigene_id(void) { return f_id; }
int qc_zugang_passwort(char *puffer, size_t groesse) {
    if (!f_pw_da) return -1;
    strlcpy(puffer, f_pw, groesse);
    return 0;
}
int qc_zugang_passwort_setzen(const char *pw) { strlcpy(f_pw, pw, sizeof f_pw); return 0; }
int qc_zugang_passwort_zufall(void) { return 0; }
int qc_zugang_geraete(qc_geraet *liste, int max) {
    if (f_anzahl < 0) return -1;
    int n = f_anzahl < max ? f_anzahl : max;
    memcpy(liste, f_geraete, (size_t)n * sizeof *liste);
    return n;
}
int qc_zugang_geraet_entfernen(const uint8_t pub[32]) { return 0; }
int qc_zugang_alle_entfernen(void) { return 0; }
int qc_zugang_liste_zuruecksetzen(void) { return 0; }
void qc_zugang_entscheiden(uint64_t anfrage, int zulassen) {}
int qc_zustand_zuschauer(char *name, size_t groesse) {
    if (!f_verbunden) return 0;
    strlcpy(name, f_zuschauer, groesse);
    return 1;
}
int qc_zustand_bildschirmfreigabe(void) { return f_bildschirm; }
int qc_zustand_bedienungshilfen(void) { return f_bedienung; }
int qc_zustand_port_belegt(void) { return f_port; }

static qc_geraet geraet(uint8_t schluessel, uint32_t id, const char *name, const char *datum) {
    qc_geraet g;
    memset(&g, 0, sizeof g);
    memset(g.pub, schluessel, sizeof g.pub);
    g.id = id;
    strlcpy(g.name, name, sizeof g.name);
    strlcpy(g.datum, datum, sizeof g.datum);
    return g;
}

// ------------------------------------------------------------ Texte

// Wortgleich mit der Spezifikation (Pairing, Abschnitt 14) und den
// Schluesseln des Passwort-Fensters, wie in client/src/strings.rs: beide
// Sprachen mit Auslassungszeichen "…" und Gedankenstrich (" – ") wie ihr
// Bestand, Deutsch dazu mit Umlauten und „…“.
static const char *const SOLL_EN[QCTextAnzahl] = {
    [QCTextHostReady] = "Ready for connections",
    [QCTextHostConnected] = "Connected: {n}",
    [QCTextHostDeviceId] = "Device ID: {i}",
    [QCTextHostPassword] = "Password: {p}",
    [QCTextHostCopied] = "Copied",
    [QCTextHostChangePassword] = "Change password …",
    [QCTextHostRandomPassword] = "New random password",
    [QCTextHostNewPassword] = "New password (at least 8 characters)",
    [QCTextHostRepeatPassword] = "Repeat password",
    [QCTextHostPasswordsDiffer] = "The passwords do not match.",
    [QCTextHostPasswordShort] = "At least 8 characters, please.",
    [QCTextHostPasswordSaved] = "Password saved.",
    [QCTextHostPasswordUnreadable] = "Password file unreadable – new devices only via \"Allow\".",
    [QCTextHostDevices] = "Allowed devices",
    [QCTextHostNoDevices] = "No devices yet",
    [QCTextHostDeviceLine] = "{n} – ID {i} – since {d}",
    [QCTextHostRemove] = "Remove",
    [QCTextHostRemoveAll] = "Remove all devices …",
    [QCTextHostRemoveAllAsk] = "Remove all allowed devices? They will need the password again.",
    [QCTextHostListDamaged] = "Device list damaged",
    [QCTextHostListReset] = "Reset device list",
    [QCTextHostRequest] = "{n} (ID {i}) wants to control this computer.",
    [QCTextHostAllow] = "Allow",
    [QCTextHostDeny] = "Deny",
    [QCTextHostStartLogin] = "Start at login",
    [QCTextHostStartWindows] = "Start with Windows",
    [QCTextHostMoveToApps] = "Move QuadChroma to Applications first",
    [QCTextHostScreenMissing] = "Screen Recording not allowed – open System Settings …",
    [QCTextHostAccessMissing] = "Accessibility not allowed – open System Settings …",
    [QCTextHostPortBusy] = "Port {p} is used by another program",
    [QCTextHostQuit] = "Quit QuadChroma",
    [QCTextHostStopSharing] = "Stop sharing",
    [QCTextHostTooltip] = "QuadChroma – sharing this PC (ID {i})",
    [QCTextHostOk] = "OK",
    [QCTextAccessCode] = "Code: {c}",
    [QCTextAccessCancel] = "Cancel",
    [QCTextHostPasswordNotSaved] = "The password could not be saved.",
    [QCTextHostPasswordInvalid] = "The password contains characters that are not allowed.",
    [QCTextHostPasswordTitle] = "Change password",
};

static const char *const SOLL_DE[QCTextAnzahl] = {
    [QCTextHostReady] = "Bereit für Verbindungen",
    [QCTextHostConnected] = "Verbunden: {n}",
    [QCTextHostDeviceId] = "Geräte-ID: {i}",
    [QCTextHostPassword] = "Passwort: {p}",
    [QCTextHostCopied] = "Kopiert",
    [QCTextHostChangePassword] = "Passwort ändern …",
    [QCTextHostRandomPassword] = "Neues Zufallspasswort",
    [QCTextHostNewPassword] = "Neues Passwort (mindestens 8 Zeichen)",
    [QCTextHostRepeatPassword] = "Passwort wiederholen",
    [QCTextHostPasswordsDiffer] = "Die Passwörter stimmen nicht überein.",
    [QCTextHostPasswordShort] = "Bitte mindestens 8 Zeichen.",
    [QCTextHostPasswordSaved] = "Passwort gespeichert.",
    [QCTextHostPasswordUnreadable] = "Passwortdatei unlesbar – neue Geräte nur über „Zulassen“.",
    [QCTextHostDevices] = "Erlaubte Geräte",
    [QCTextHostNoDevices] = "Noch keine Geräte",
    [QCTextHostDeviceLine] = "{n} – ID {i} – seit {d}",
    [QCTextHostRemove] = "Entfernen",
    [QCTextHostRemoveAll] = "Alle Geräte entfernen …",
    [QCTextHostRemoveAllAsk] = "Alle erlaubten Geräte entfernen? Sie brauchen dann wieder das Passwort.",
    [QCTextHostListDamaged] = "Geräteliste beschädigt",
    [QCTextHostListReset] = "Geräteliste zurücksetzen",
    [QCTextHostRequest] = "{n} (ID {i}) möchte diesen Computer steuern.",
    [QCTextHostAllow] = "Zulassen",
    [QCTextHostDeny] = "Ablehnen",
    [QCTextHostStartLogin] = "Beim Anmelden starten",
    [QCTextHostStartWindows] = "Mit Windows starten",
    [QCTextHostMoveToApps] = "QuadChroma zuerst in den Ordner Programme bewegen",
    [QCTextHostScreenMissing] = "Bildschirmaufnahme nicht erlaubt – Systemeinstellungen öffnen …",
    [QCTextHostAccessMissing] = "Bedienungshilfen nicht erlaubt – Systemeinstellungen öffnen …",
    [QCTextHostPortBusy] = "Port {p} ist von einem anderen Programm belegt",
    [QCTextHostQuit] = "QuadChroma beenden",
    [QCTextHostStopSharing] = "Freigabe beenden",
    [QCTextHostTooltip] = "QuadChroma – Freigabe läuft (ID {i})",
    [QCTextHostOk] = "OK",
    [QCTextAccessCode] = "Code: {c}",
    [QCTextAccessCancel] = "Abbrechen",
    [QCTextHostPasswordNotSaved] = "Das Passwort konnte nicht gespeichert werden.",
    [QCTextHostPasswordInvalid] = "Das Passwort enthält unzulässige Zeichen.",
    [QCTextHostPasswordTitle] = "Passwort ändern",
};

// Die Platzhalter eines Textes ("{n}", "{i}" ...), sortiert.
static NSArray<NSString *> *platzhalter(const char *s) {
    NSMutableArray<NSString *> *a = [NSMutableArray array];
    for (const char *p = s; p && *p; p++)
        if (p[0] == '{' && p[1] && p[1] != '}' && p[2] == '}')
            [a addObject:[NSString stringWithFormat:@"{%c}", p[1]]];
    return [a sortedArrayUsingSelector:@selector(compare:)];
}

static void texte_pruefen(void) {
    printf("\n-- Texte\n");
    static const char *const codes[] = {
        "en", "de", "fr", "es", "it", "pt", "nl", "sv", "da", "nb", "fi", "is", "pl", "cs", "sk",
        "hu", "ro", "bg", "el", "et", "lv", "lt", "sl", "hr", "uk", "ru", "zh", "ja", "tr",
    };
    const int n = (int)(sizeof codes / sizeof codes[0]);
    pruefe(qc_texte_sprachen() == 29 && n == 29, "29 Sprachen");
    int reihenfolge = 1;
    for (int i = 0; i < n && i < qc_texte_sprachen(); i++)
        if (strcmp(qc_texte_code(i), codes[i]) != 0) reihenfolge = 0;
    pruefe(reihenfolge, "Codes und Reihenfolge wie client/src/strings_more.rs (ALL)");

    int fehlt = 0, leer = 0, platz = 0, namen = 1;
    for (int s = 0; s < qc_texte_sprachen(); s++) {
        if (!qc_texte_name(s) || !*qc_texte_name(s)) namen = 0;
        for (int t = 0; t < QCTextAnzahl; t++) {
            const char *x = qc_texte_eintrag(s, (QCText)t);
            if (!x) { fehlt++; printf("         fehlt: %s, Schluessel %d\n", qc_texte_code(s), t); continue; }
            if (!*x) { leer++; printf("         leer: %s, Schluessel %d\n", qc_texte_code(s), t); }
            if (![platzhalter(x) isEqualToArray:platzhalter(qc_texte_eintrag(0, (QCText)t))]) {
                platz++;
                printf("         Platzhalter anders als Englisch: %s, Schluessel %d\n", qc_texte_code(s), t);
            }
        }
    }
    pruefe(fehlt == 0, "jede Tabelle hat jeden Text");
    pruefe(leer == 0, "kein Text ist leer");
    pruefe(platz == 0, "jeder Text hat dieselben Platzhalter wie Englisch");
    pruefe(namen, "jede Sprache hat einen Namen");
    pruefe(qc_texte_eintrag(29, QCTextHostReady) == NULL && qc_texte_eintrag(0, QCTextAnzahl) == NULL,
           "ausserhalb der Tabellen: NULL");

    int en = 1, de = 1;
    for (int t = 0; t < QCTextAnzahl; t++) {
        if (!SOLL_EN[t] || !qc_texte_eintrag(0, (QCText)t) || strcmp(SOLL_EN[t], qc_texte_eintrag(0, (QCText)t))) {
            en = 0;
            printf("         en %d: \"%s\"\n", t, qc_texte_eintrag(0, (QCText)t));
        }
        if (!SOLL_DE[t] || !qc_texte_eintrag(1, (QCText)t) || strcmp(SOLL_DE[t], qc_texte_eintrag(1, (QCText)t))) {
            de = 0;
            printf("         de %d: \"%s\"\n", t, qc_texte_eintrag(1, (QCText)t));
        }
    }
    pruefe(en, "Englisch wortgleich mit der Spezifikation (mit \"…\" und \" – \" wie der Client)");
    pruefe(de, "Deutsch wortgleich mit der Spezifikation (Umlaute, typografische Zeichen)");

    // Wie im Client: innerhalb einer Sprache kein Text doppelt.
    int doppelt = 0;
    for (int s = 0; s < qc_texte_sprachen(); s++)
        for (int a = 0; a < QCTextAnzahl; a++)
            for (int b = a + 1; b < QCTextAnzahl; b++)
                if (qc_texte_eintrag(s, (QCText)a) && qc_texte_eintrag(s, (QCText)b) &&
                    !strcmp(qc_texte_eintrag(s, (QCText)a), qc_texte_eintrag(s, (QCText)b))) {
                    doppelt++;
                    printf("         doppelt: %s, Schluessel %d und %d\n", qc_texte_code(s), a, b);
                }
    pruefe(doppelt == 0, "kein Text doppelt innerhalb einer Sprache (alle 29)");

    // Die 27 weiteren Sprachen sind uebersetzt (Paket P6, wortgleich mit
    // client/src/strings*.rs): gleich wie Englisch nur OK und die Woerter,
    // die eine Sprache aus dem Englischen uebernimmt (italienisch
    // "Password", niederlaendisch "Code").
    int englisch = 0;
    for (int s = 2; s < qc_texte_sprachen(); s++)
        for (int t = 0; t < QCTextAnzahl; t++) {
            const char *x = qc_texte_eintrag(s, (QCText)t), *code = qc_texte_code(s);
            if (!x || strcmp(x, qc_texte_eintrag(0, (QCText)t))) continue;
            if (t == QCTextHostOk || (!strcmp(code, "it") && t == QCTextHostPassword) ||
                (!strcmp(code, "nl") && t == QCTextAccessCode)) continue;
            englisch++;
            printf("         noch englisch: %s, Schluessel %d\n", code, t);
        }
    pruefe(englisch == 0, "weitere Sprachen: uebersetzt");

    // Schreibweise wie in den Tabellen des Clients, Englisch eingeschlossen:
    // "…" statt "..." und Gedankenstrich statt " - "; genau die
    // Menuepunkte mit Fenster oder Folge enden auf " …"; der Hinweis zur
    // unlesbaren Passwortdatei nennt den Knopf "Zulassen" mit seinem Wort.
    int schreibweise = 0;
    for (int s = 0; s < qc_texte_sprachen(); s++) {
        for (int t = 0; t < QCTextAnzahl; t++) {
            const char *x = qc_texte_eintrag(s, (QCText)t);
            if (!x) continue;
            size_t l = strlen(x);
            int punkte = l >= 4 && !strcmp(x + l - 4, " \xE2\x80\xA6");
            int soll = t == QCTextHostChangePassword || t == QCTextHostRemoveAll ||
                       t == QCTextHostScreenMissing || t == QCTextHostAccessMissing;
            if (strstr(x, "...") || strstr(x, " - ") || punkte != soll) {
                schreibweise++;
                printf("         Schreibweise: %s, Schluessel %d: %s\n", qc_texte_code(s), t, x);
            }
        }
        const char *u = qc_texte_eintrag(s, QCTextHostPasswordUnreadable), *z = qc_texte_eintrag(s, QCTextHostAllow);
        if (!u || !z || !strstr(u, z)) {
            schreibweise++;
            printf("         \"Zulassen\" fehlt im Hinweis: %s\n", qc_texte_code(s));
        }
    }
    pruefe(schreibweise == 0, "Auslassungspunkte, Gedankenstrich und \"Zulassen\" wie im Client");

    printf("\n-- Sprachwahl\n");
    struct { NSArray<NSString *> *bevorzugt; const char *soll; } faelle[] = {
        { @[ @"de-DE" ], "de" }, { @[ @"de" ], "de" }, { @[ @"DE_at" ], "de" },
        { @[ @"en-GB" ], "en" }, { @[ @"pt-BR" ], "pt" }, { @[ @"zh-Hans-CN" ], "zh" },
        { @[ @"nb-NO" ], "nb" }, { @[ @"xx-YY", @"fr-FR" ], "fr" }, { @[ @"fr", @"de" ], "fr" },
        { @[ @"sr-Latn" ], "en" }, { @[], "en" }, { @[ @"uk-UA", @"ru" ], "uk" },
    };
    int alle = 1;
    for (size_t i = 0; i < sizeof faelle / sizeof faelle[0]; i++) {
        const char *ist = qc_texte_code(qc_texte_waehlen(faelle[i].bevorzugt));
        if (strcmp(ist, faelle[i].soll)) {
            alle = 0;
            printf("         %s -> %s, erwartet %s\n", [faelle[i].bevorzugt description].UTF8String, ist, faelle[i].soll);
        }
    }
    pruefe(alle, "erst der genaue Code, dann der Sprachteil, in der Reihenfolge der Vorlieben, sonst en");
    qc_texte_setzen(99);
    pruefe(qc_texte_aktuell() == 0, "ungueltige Sprache -> Englisch");

    printf("\n-- Platzhalter\n");
    qc_texte_setzen(0);
    pruefe([qc_text_mit(QCTextHostConnected, @{ @"n": @"{n}{i}", @"i": @"X" }) isEqualToString:@"Connected: {n}{i}"],
           "ein Durchgang: ein Name, der wie ein Platzhalter aussieht, bleibt stehen");
    pruefe([qc_text_mit(QCTextHostDeviceLine, @{ @"n": @"PC" }) isEqualToString:@"PC – ID {i} – since {d}"],
           "Platzhalter ohne Wert bleibt stehen");
    qc_texte_setzen(1);
    pruefe([qc_text_mit(QCTextHostRequest, @{ @"n": @"Büro-PC", @"i": @"581 729 911" })
               isEqualToString:@"Büro-PC (ID 581 729 911) möchte diesen Computer steuern."],
           "Anfrage-Text mit Name und ID (de)");
    pruefe([qc_text(QCTextAccessCancel) isEqualToString:@"Abbrechen"], "qc_text in der gesetzten Sprache");
}

// ------------------------------------------------------------ Hilfen

static void hilfen_pruefen(void) {
    printf("\n-- Anzeigeformen und Passwortpruefung\n");
    pruefe([qc_id_text(5) isEqualToString:@"000 000 005"], "ID 5 -> \"000 000 005\"");
    pruefe([qc_id_text(581729911) isEqualToString:@"581 729 911"], "ID 581729911 -> \"581 729 911\"");
    pruefe([qc_id_text(999999999) isEqualToString:@"999 999 999"], "groesste ID");
    pruefe([qc_code_text(628306) isEqualToString:@"628 306"] && [qc_code_text(7) isEqualToString:@"000 007"],
           "Vergleichscode \"ddd ddd\"");
    qc_texte_setzen(1);
    pruefe([qc_datum_text(@"2026-09-26") isEqualToString:@"26.09.2026"], "Datum deutsch: 26.09.2026");
    qc_texte_setzen(0);
    NSString *en = qc_datum_text(@"2026-09-26");
    pruefe([en containsString:@"Sep"] && [en containsString:@"26"] && [en containsString:@"2026"],
           "Datum englisch: Monatsname, Tag, Jahr");
    pruefe([qc_datum_text(@"kaputt") isEqualToString:@"kaputt"] && [qc_datum_text(@"") isEqualToString:@""],
           "unlesbares Datum bleibt, wie es ist");

    pruefe(qc_passwort_pruefen(@"k7m-4wq-9tz", @"k7m-4wq-9tz") == 0, "Zufallspasswort ist gut (norm 9 Byte)");
    pruefe(qc_passwort_pruefen(@"abcd efgh", @"abcd efgh") == 0, "8 Zeichen ohne Leerzeichen genuegen");
    pruefe(qc_passwort_pruefen(@"abc-def-g", @"abc-def-g") == 2, "norm ohne '-' unter 8 Byte: zu kurz");
    pruefe(qc_passwort_pruefen(@"Grüße", @"Grüße") == 2, "Umlaute zaehlen als Bytes: \"Grüße\" = 7 Byte, zu kurz");
    pruefe(qc_passwort_pruefen(@"Grüße!!", @"Grüße!!") == 0, "\"Grüße!!\" = 9 Byte, gut");
    pruefe(qc_passwort_pruefen(@"langes-passwort", @"langes-passwurt") == 1, "ungleich");
    pruefe(qc_passwort_pruefen(@"kurz", @"anders") == 2, "zu kurz geht vor ungleich");
    pruefe(qc_passwort_pruefen(@"langes passwort", nil) == 1, "zweites Feld fehlt: ungleich");
    // Grenze 128 Byte wie im Client (zugang::PASSWORT_MAX): ein laengeres
    // Passwort liesse sich dort gar nicht eingeben.
    NSString *p128 = [@"" stringByPaddingToLength:128 withString:@"a" startingAtIndex:0];
    NSString *p129 = [p128 stringByAppendingString:@"a"];
    pruefe(QC_ZUGANG_PW_MAX == 128, "QC_ZUGANG_PW_MAX ist 128 Byte wie PASSWORT_MAX im Client");
    pruefe(qc_passwort_pruefen(p128, p128) == 0, "128 Byte: gut (QC_ZUGANG_PW_MAX)");
    pruefe(qc_passwort_pruefen(p129, p129) == 3, "129 Byte: unzulaessig, nicht \"zu kurz\"");
    NSString *u = [@"" stringByPaddingToLength:64 withString:@"ü" startingAtIndex:0];
    pruefe(qc_passwort_pruefen(u, u) == 0, "64 x \"ü\" = 128 Byte: gut");
    u = [u stringByAppendingString:@"a"];
    pruefe(qc_passwort_pruefen(u, u) == 3, "129 Byte UTF-8: unzulaessig");
    pruefe(qc_passwort_pruefen(@"abcd\nefgh", @"abcd\nefgh") == 3 && qc_passwort_pruefen(@"abcdefgh\r", @"abcdefgh\r") == 3,
           "Zeilenumbruch (eingefuegt): unzulaessig");
    pruefe(qc_passwort_pruefen(p129, @"anders") == 3, "unzulaessig geht vor ungleich");
}

// ------------------------------------------------------------ Modell

static QCMenueZustand *zustand_grund(void) {
    QCMenueZustand *z = [[QCMenueZustand alloc] init];
    z.eigeneId = 581729911;
    z.passwort = @"k7m-4wq-9tz";
    qc_geraet g[2] = { geraet(0x11, 5, "Windows-PC", "2026-09-26"), geraet(0x22, 123456789, "Büro-Laptop", "2026-01-02") };
    z.geraeteAnzahl = 2;
    z.geraete = [NSData dataWithBytes:g length:sizeof g];
    z.bildschirm = YES;
    z.bedienung = YES;
    z.anmelden = QCAnmeldenNichtInProgramme;
    return z;
}

static int titel_gleich(NSArray<NSString *> *ist, NSArray<NSString *> *soll) {
    if ([ist isEqualToArray:soll]) return 1;
    printf("         ist:\n");
    for (NSString *s in ist) printf("           |%s\n", s.UTF8String);
    printf("         erwartet:\n");
    for (NSString *s in soll) printf("           |%s\n", s.UTF8String);
    return 0;
}

static QCMenuePunkt *suche(NSArray<QCMenuePunkt *> *m, QCAktion a) {
    for (QCMenuePunkt *p in m) {
        if (p.aktion == a) return p;
        QCMenuePunkt *q = p.unter ? suche(p.unter, a) : nil;
        if (q) return q;
    }
    return nil;
}

static void modell_pruefen(void) {
    printf("\n-- Menue-Modell\n");
    qc_texte_setzen(1);
    QCMenueZustand *z = zustand_grund();
    NSArray<QCMenuePunkt *> *m = qc_menue_modell(z);
    pruefe(titel_gleich(qc_menue_titel(m), @[
        @"# QuadChroma",
        @"(Bereit für Verbindungen)",
        @"---",
        @"Geräte-ID: 581 729 911",
        @"Passwort: k7m-4wq-9tz",
        @"Passwort ändern …",
        @"Neues Zufallspasswort",
        @"---",
        @"Erlaubte Geräte",
        @"  Windows-PC – ID 000 000 005 – seit 26.09.2026",
        @"    Entfernen",
        @"  Büro-Laptop – ID 123 456 789 – seit 02.01.2026",
        @"    Entfernen",
        @"  ---",
        @"  Alle Geräte entfernen …",
        @"---",
        @"(Beim Anmelden starten)",
        @"(QuadChroma zuerst in den Ordner Programme bewegen)",
        @"---",
        @"QuadChroma beenden",
    ]), "de, bereit, zwei Geraete, alle Freigaben, App nicht in /Applications");

    QCMenuePunkt *weg = suche(m, QCAktionGeraetEntfernen);
    uint8_t pub[32];
    memset(pub, 0x11, sizeof pub);
    pruefe(weg && [weg.daten isEqualToData:[NSData dataWithBytes:pub length:32]],
           "\"Entfernen\" traegt den Schluessel seines Geraets");
    pruefe(suche(m, QCAktionBeenden).taste && [suche(m, QCAktionBeenden).taste isEqualToString:@"q"], "Beenden mit Cmd+Q");
    pruefe(suche(m, QCAktionIdKopieren).aktiv && suche(m, QCAktionPasswortKopieren).aktiv, "ID und Passwort anklickbar (kopieren)");

    qc_texte_setzen(0);
    z.zuschauer = @"Robert's PC";
    z.passwort = nil;
    z.geraeteAnzahl = -1;
    z.geraete = nil;
    z.bildschirm = NO;
    z.bedienung = NO;
    z.anmelden = QCAnmeldenAn;
    m = qc_menue_modell(z);
    pruefe(titel_gleich(qc_menue_titel(m), @[
        @"# QuadChroma",
        @"(Connected: Robert's PC)",
        @"---",
        @"Device ID: 581 729 911",
        @"(Password file unreadable – new devices only via \"Allow\".)",
        @"Change password …",
        @"New random password",
        @"---",
        @"(Device list damaged)",
        @"Reset device list",
        @"---",
        @"[x] Start at login",
        @"Screen Recording not allowed – open System Settings …",
        @"Accessibility not allowed – open System Settings …",
        @"---",
        @"Quit QuadChroma",
    ]), "en, verbunden, Passwort unlesbar, Liste beschaedigt, Freigaben fehlen, Anmelden an");
    pruefe(suche(m, QCAktionBildschirmFreigabe) && suche(m, QCAktionBedienungshilfen) && suche(m, QCAktionListeZuruecksetzen),
           "Freigaben und Zuruecksetzen haben ihre Aktionen");
    pruefe(!suche(m, QCAktionPasswortKopieren), "unlesbares Passwort ist nicht kopierbar");

    // Bildport von einem anderen Programm belegt: dann kann niemand
    // verbinden - die Zustandszeile sagt das statt "Bereit".
    z = zustand_grund();
    z.portBelegt = 9001;
    NSArray<NSString *> *t = qc_menue_titel(qc_menue_modell(z));
    pruefe([t[1] isEqualToString:@"(Port 9001 is used by another program)"] && ![t containsObject:@"(Ready for connections)"] &&
           [t[2] isEqualToString:@"---"], "Port belegt: Zustandszeile statt \"Ready\", keine zweite Zeile");
    z.zuschauer = @"PC";
    t = qc_menue_titel(qc_menue_modell(z));
    pruefe([t[1] isEqualToString:@"(Connected: PC)"] && [t[2] isEqualToString:@"---"], "verbunden geht vor Port belegt");

    z = zustand_grund();
    z.geraeteAnzahl = 0;
    z.geraete = nil;
    z.anmelden = QCAnmeldenAus;
    t = qc_menue_titel(qc_menue_modell(z));
    pruefe([t containsObject:@"Allowed devices"] && [t containsObject:@"  (No devices yet)"] &&
           ![t containsObject:@"  Remove all devices …"], "leere Liste: \"No devices yet\", kein \"Remove all\"");
    pruefe([t containsObject:@"Start at login"] && ![t containsObject:@"(Move QuadChroma to Applications first)"],
           "in /Applications, aus: anklickbar, ohne Haken, ohne Hinweis");
    z.anmelden = QCAnmeldenFreigabeNoetig;
    pruefe([qc_menue_titel(qc_menue_modell(z)) containsObject:@"[-] Start at login"],
           "in den Systemeinstellungen abgeschaltet: halber Haken");

    // Unbeglaubigte Namen aus dem Netz: ein Name, der wie ein Platzhalter
    // aussieht, und einer ohne abschliessende 0 (volle 41 Byte).
    qc_geraet g[2] = { geraet(0x33, 7, "{i}", "2026-09-26"), geraet(0x44, 8, "", "") };
    memset(g[1].name, 'A', sizeof g[1].name);
    z.geraeteAnzahl = 2;
    z.geraete = [NSData dataWithBytes:g length:sizeof g];
    t = qc_menue_titel(qc_menue_modell(z));
    NSString *zeile = nil, *voll = nil;
    NSString *a41 = [@"" stringByPaddingToLength:41 withString:@"A" startingAtIndex:0];
    NSString *voll_soll = [NSString stringWithFormat:@"  %@ – ID 000 000 008 – since ?", a41];
    for (NSString *s in t) {
        if ([s hasPrefix:@"  {i} – ID 000 000 007 – since "]) zeile = s;
        if ([s isEqualToString:voll_soll]) voll = s;
    }
    pruefe(zeile != nil, "Geraetename \"{i}\" bleibt stehen, wie er ist");
    pruefe(voll != nil, "Name ohne abschliessende 0: hoechstens 41 Byte, leeres Datum als \"?\"");

    // Mehr Geraete angekuendigt, als Bytes da sind: nur die vorhandenen.
    z.geraeteAnzahl = 5;
    z.geraete = [NSData dataWithBytes:g length:sizeof g[0]];
    t = qc_menue_titel(qc_menue_modell(z));
    int zeilen = 0;
    for (NSString *s in t) if ([s hasSuffix:@"Remove"]) zeilen++;
    pruefe(zeilen == 1, "Anzahl groesser als die Daten: nur die vorhandenen Geraete");
}

static void zustand_pruefen(void) {
    printf("\n-- Zustand aus dem Kern (Attrappe)\n");
    f_id = 5;
    f_pw_da = 1;
    strlcpy(f_pw, "abc-def-ghk", sizeof f_pw);
    f_anzahl = 1;
    f_geraete[0] = geraet(0x55, 42, "Tablet", "2026-09-01");
    f_verbunden = 1;
    strlcpy(f_zuschauer, "Wohnzimmer", sizeof f_zuschauer);
    f_bildschirm = 0;
    f_bedienung = 1;
    QCMenueZustand *z = qc_menue_zustand_lesen();
    const qc_geraet *g = z.geraete.bytes;
    pruefe(z.eigeneId == 5 && [z.passwort isEqualToString:@"abc-def-ghk"], "ID und Passwort");
    pruefe(z.geraeteAnzahl == 1 && z.geraete.length == sizeof(qc_geraet) && g[0].id == 42 && !strcmp(g[0].name, "Tablet"),
           "Geraeteliste");
    pruefe([z.zuschauer isEqualToString:@"Wohnzimmer"], "verbundener Zuschauer mit Namen");
    pruefe(!z.bildschirm && z.bedienung, "Freigaben");
    pruefe(z.anmelden == QCAnmeldenNichtInProgramme, "Pruefstand liegt nicht in /Applications: Anmelden gesperrt");
    pruefe(z.portBelegt == 0, "Port frei");
    f_port = 9001;
    pruefe(qc_menue_zustand_lesen().portBelegt == 9001, "belegter Port kommt im Zustand an (qc_zustand_port_belegt)");
    f_port = 0;

    f_pw_da = 0;
    f_anzahl = -1;
    f_verbunden = 0;
    z = qc_menue_zustand_lesen();
    pruefe(z.passwort == nil && z.geraeteAnzahl == -1 && z.geraete == nil && z.zuschauer == nil,
           "unlesbares Passwort, beschaedigte Liste, niemand verbunden");
    f_pw_da = 1;
    f_pw[0] = (char)0xC3;   // kaputtes UTF-8
    f_pw[1] = 0;
    pruefe(qc_menue_zustand_lesen().passwort == nil, "Passwort mit kaputtem UTF-8 gilt als unlesbar");
    pruefe(qc_ui_vorhanden() == 0, "ohne gestartete Oberflaeche: kein Zulassen moeglich");
}

// ------------------------------------------------------------ NSMenu, Symbol

static void menue_pruefen(void) {
    printf("\n-- NSMenu aus dem Modell\n");
    qc_texte_setzen(1);
    QCMenueZustand *z = zustand_grund();
    z.anmelden = QCAnmeldenAn;
    NSArray<QCMenuePunkt *> *m = qc_menue_modell(z);
    NSMenu *menue = qc_menue_bauen(m, nil);
    pruefe(menue.numberOfItems == (NSInteger)m.count, "ein Menuepunkt je Modellpunkt");
    pruefe(menue.itemArray[0].isSectionHeader && [menue.itemArray[0].title isEqualToString:@"QuadChroma"], "Kopf als Abschnittstitel");
    pruefe(!menue.itemArray[1].enabled && [menue.itemArray[1].title isEqualToString:@"Bereit für Verbindungen"],
           "Zustandszeile gesperrt");
    pruefe(menue.itemArray[2].isSeparatorItem, "Trennlinie");
    NSMenuItem *geraete = nil, *anm = nil, *ende = nil;
    for (NSMenuItem *it in menue.itemArray) {
        if ([it.title isEqualToString:@"Erlaubte Geräte"]) geraete = it;
        if (it.tag == QCAktionAnmelden) anm = it;
        if (it.tag == QCAktionBeenden) ende = it;
    }
    pruefe(geraete.submenu.numberOfItems == 4 && geraete.submenu.itemArray[0].submenu.numberOfItems == 1,
           "Untermenue Geraete: zwei Geraete mit je \"Entfernen\", Trennlinie, \"Alle entfernen\"");
    NSMenuItem *weg = geraete.submenu.itemArray[0].submenu.itemArray[0];
    pruefe(weg.tag == QCAktionGeraetEntfernen && [weg.representedObject isKindOfClass:[NSData class]] &&
           [(NSData *)weg.representedObject length] == 32 && weg.action == @selector(menueAktion:),
           "\"Entfernen\": Aktion und Schluessel am Menuepunkt");
    pruefe(anm.state == NSControlStateValueOn && anm.enabled, "Anmelden an: Haken");
    pruefe([ende.keyEquivalent isEqualToString:@"q"] && ende.action == @selector(menueAktion:), "Beenden mit Cmd+Q");
    pruefe(!menue.autoenablesItems && !geraete.submenu.autoenablesItems, "gesperrt heisst gesperrt (keine Automatik)");

    printf("\n-- Symbol der Menueleiste\n");
    for (int verbunden = 0; verbunden < 2; verbunden++) {
        NSImage *bild = qc_menue_symbol(verbunden);
        pruefe(bild.isTemplate && NSEqualSizes(bild.size, NSMakeSize(18, 18)), "Vorlagenbild, 18 pt");
        NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:36 pixelsHigh:36
                                                                     bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES
                                                                          isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace
                                                                       bytesPerRow:0 bitsPerPixel:0];
        rep.size = NSMakeSize(18, 18);
        [NSGraphicsContext saveGraphicsState];
        [NSGraphicsContext setCurrentContext:[NSGraphicsContext graphicsContextWithBitmapImageRep:rep]];
        [bild drawInRect:NSMakeRect(0, 0, 18, 18)];
        [NSGraphicsContext restoreGraphicsState];
        // Bildpunkte (2x, von oben): Mitte oben links, Luecke, Rahmen und Mitte unten rechts.
        CGFloat links_oben = [rep colorAtX:9 y:9].alphaComponent;
        CGFloat luecke = [rep colorAtX:18 y:18].alphaComponent;
        CGFloat rahmen = [rep colorAtX:21 y:26].alphaComponent;
        CGFloat mitte = [rep colorAtX:26 y:26].alphaComponent;
        printf("         (%s: oben links %.2f, Luecke %.2f, Rahmen %.2f, Mitte unten rechts %.2f)\n",
               verbunden ? "verbunden" : "bereit", links_oben, luecke, rahmen, mitte);
        pruefe(links_oben > 0.9 && luecke < 0.1 && rahmen > 0.5, "vier Felder mit Luecke, das vierte als Rahmen");
        pruefe(verbunden ? mitte > 0.5 : mitte < 0.1,
               verbunden ? "verbunden: Punkt im vierten Feld" : "bereit: viertes Feld hohl (anders als der Client)");
    }
}

// ------------------------------------------------------------ Anfragen

static QCAnfrage *anfrage(uint64_t nr) {
    QCAnfrage *a = [[QCAnfrage alloc] init];
    a.nr = nr;
    a.name = [NSString stringWithFormat:@"Geraet %llu", nr];
    return a;
}

static void anfragen_pruefen(void) {
    printf("\n-- Warteschlange der Zulassen-Anfragen\n");
    QCAnfragen *w = [[QCAnfragen alloc] init];
    pruefe([w neu:anfrage(1)] && [w erste].nr == 1, "erste Anfrage wird gezeigt");
    pruefe(![w neu:anfrage(2)] && [w erste].nr == 1 && [w anzahl] == 2, "zweite wartet, die erste bleibt im Fenster");
    pruefe(![w neu:anfrage(2)] && [w anzahl] == 2, "doppelte Nummer zaehlt nicht");
    pruefe(![w weg:2] && [w erste].nr == 1 && [w anzahl] == 1, "Rueckzug einer wartenden: das Fenster bleibt");
    pruefe(![w neu:anfrage(3)], "dritte wartet");
    pruefe([w weg:1] && [w erste].nr == 3, "Rueckzug (oder Antwort) der gezeigten: die naechste kommt");
    pruefe(![w weg:99] && [w anzahl] == 1, "unbekannte Nummer: nichts");
    pruefe([w weg:3] && [w erste] == nil && [w anzahl] == 0, "letzte weg: Fenster zu");
    pruefe([w neu:anfrage(4)], "danach wieder: neue Anfrage wird sofort gezeigt");
}

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IONBF, 0);
        texte_pruefen();
        hilfen_pruefen();
        modell_pruefen();
        zustand_pruefen();
        menue_pruefen();
        anfragen_pruefen();
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
