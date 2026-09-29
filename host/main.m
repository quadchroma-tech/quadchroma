// QuadChroma Host.
//   M1: Bildschirm ohne Mauszeiger aufnehmen, in Hardware als HEVC 4:4:4 codieren.
//   M2: den Strom ueber TCP ausliefern statt in eine Datei zu schreiben.
//
// Aufrufe:
//   quadchroma-host                                   Host auf Port 9001 (Doppelklick im Finder)
//   quadchroma-host --list
//   quadchroma-host --capture <sekunden> <datei.hevc> [optionen]
//   quadchroma-host --serve [port]                    [optionen]
//
// Optionen: --display N  --out BxH  --fps N  --mbit N  --fest
//
// Wer hereinkommt, entscheidet zugang.h: bekannte Geraete (host-devices.txt)
// sofort, neue mit dem Zugangspasswort oder per Klick "Zulassen" am Host.
//
// Der Codec laesst sich im Betrieb wechseln (Nachricht 66 vom Client); der
// Start erfolgt immer mit Kandidat 0, HEVC 4:4:4 10 Bit.
// Der Bildschirm ebenso (Nachricht 70, Liste als Nachricht 12): ohne Wunsch
// folgt der Host dem Hauptbildschirm, mit Wunsch dem gewuenschten, siehe
// bildschirm.h. --display N pinnt fuer diesen Lauf den Listenplatz N.
//
// QuadChroma.app ist die eine App (Client und Host, der Rust-Client mit
// dieser Engine eingebaut); die Freigaben haengen an ihrem Bundle. Es laeuft
// hoechstens ein Host je Nutzer (host-instanz.lock). Ausgaben der einen App
// in host-protokoll.txt im Ablageordner, sonst in /tmp/quadchroma-m1.log.
//
// Diese Datei ist der Dienst, kein Programm: der Rust-Client baut die
// Engine ohne start.m ein und startet sie eingebettet (qc_app_einrichten,
// qc_dienst_starten, qc_dienst_anhalten/..._fortsetzen); main in start.m
// ist der fruehere eigene Host (make host-allein, nur fuer Pruefstaende).
// Die Schnittstelle steht in dienst.h.
//
// Im Dienstbetrieb gehoert der Hauptfaden AppKit ([NSApp run] in start.m,
// eingebettet die Run-Loop von winit): Symbol in der Menueleiste, Zulassen-
// und Passwort-Fenster (menue.m, Texte in texte.m). Das Regelmaessige laeuft
// auf dem Dienst-Takt (dienst_takt_starten) und dem Zustandstakt
// (zustand_takt_starten), beide auf eigenen Warteschlangen.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreGraphics/CoreGraphics.h>
#include <dlfcn.h>
#include <pthread.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <errno.h>
#include <stdatomic.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <sys/stat.h>
#include <sys/sysctl.h>
#include <time.h>
#import "audio.h"
#import "clipboard.h"
#import "dateien.h"
#import "bildschirm.h"
#import "zeiger.h"
#import "testbild.h"
#include "qc_secure.h"
#include "qc_annahme.h"
#import "last.h"
#import "menue.h"
#include "dienst.h"
#import <IOKit/pwr_mgt/IOPMLib.h>
#import <SystemConfiguration/SystemConfiguration.h>
#include <fcntl.h>
#include <poll.h>
#include "zugang.h"
#include "hdr.h"

// ------------------------------------------------------------------ Logging

// Das Protokoll hat eine Obergrenze. Ueber g_log_grenze wandert die Datei nach
// g_log_alt_pfad (ein frueheres .alt.log faellt dabei weg), und eine neue
// beginnt: hoechstens das Doppelte auf der Platte, auch wenn jemand aus dem
// Netz die Ports flutet, und die juengsten Zeilen bleiben erhalten. Laesst
// sich die Datei nicht umbenennen, wird sie geleert - die Grenze haelt in
// jedem Fall. Gezaehlt werden die Zeilen von logf_; die wenigen aus audio.m
// und clipboard.m, die die Datei selbst oeffnen, landen in derselben,
// zaehlen aber nicht mit.
static FILE *g_log = NULL;
static const char *g_log_pfad = "/tmp/quadchroma-m1.log";
static const char *g_log_alt_pfad = "/tmp/quadchroma-m1.alt.log";
static long g_log_grenze = 8L * 1024 * 1024;     // nur der Pruefstand setzt sie herab
static long g_log_bytes = 0;                      // unter g_log_mtx
// Nur um g_log und seinen Zaehler; darunter wird keine andere Sperre genommen.
static pthread_mutex_t g_log_mtx = PTHREAD_MUTEX_INITIALIZER;

// Beim Start und unter g_log_mtx. modus: "a" haengt an, "w" beginnt leer.
static void log_oeffnen(const char *modus) {
    g_log = fopen(g_log_pfad, modus);
    struct stat st;
    g_log_bytes = g_log && fstat(fileno(g_log), &st) == 0 ? (long)st.st_size : 0;
}

static void logf_(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    const char *c = s.UTF8String;
    fprintf(stdout, "%s\n", c);
    fflush(stdout);
    pthread_mutex_lock(&g_log_mtx);
    if (g_log && g_log_bytes >= g_log_grenze) {
        fclose(g_log);
        // Scheitert das Umbenennen - etwa weil in /tmp schon ein .alt.log
        // liegt, das einem anderen Nutzer gehoert (Sticky-Bit) -, beginnt die
        // Datei leer. Sonst hielte die Grenze nicht, und jede weitere Zeile
        // oeffnete die Datei neu und schriebe den Hinweis noch einmal.
        int umbenannt = rename(g_log_pfad, g_log_alt_pfad) == 0;
        int fehler = errno;
        log_oeffnen(umbenannt ? "a" : "w");
        if (g_log) {
            int n = umbenannt
                ? fprintf(g_log, "Protokoll war ueber %ld KB - die Zeilen davor stehen in %s\n",
                          g_log_grenze / 1024, g_log_alt_pfad)
                : fprintf(g_log, "Protokoll war ueber %ld KB und liess sich nicht nach %s umbenennen (%s) - "
                          "die Zeilen davor sind verworfen\n", g_log_grenze / 1024, g_log_alt_pfad, strerror(fehler));
            if (n > 0) g_log_bytes += n;
        }
    }
    if (g_log) {
        int n = fprintf(g_log, "%s\n", c);
        if (n > 0) g_log_bytes += n;
        fflush(g_log);
    }
    pthread_mutex_unlock(&g_log_mtx);
}

// Namen (Client aus Nachricht 3, Rechnername, Geraeteliste) sind UTF-8. Nicht
// ueber "%s" ins Format: das liest NSString in der Standardkodierung (hier
// MacRoman), aus jedem Umlaut und jedem typografischen Apostroph (U+2019 in
// "Robert's MacBook", wie macOS Rechner ab Werk nennt) wuerde Zeichensalat.
// Also als NSString ueber "%@". Kein gueltiges UTF-8 (sollte bei gesaeuberten
// Namen nicht vorkommen): dann eben MacRoman, verloren geht nichts.
static NSString *utf8(const char *s) {
    if (!s) return @"";
    return [[NSString alloc] initWithUTF8String:s] ?: [[NSString alloc] initWithCString:s encoding:NSMacOSRomanStringEncoding];
}

// Zeilen, die jeder im Netz ohne Anmeldung ausloesen kann - gescheiterter
// Handschlag, unbekannte Gegenstelle, Eingabekanal ohne Bild -, gehen
// gedrosselt ins Protokoll. Sonst schriebe jede Verbindung eine Zeile, und
// eine Flut aus dem Netz fuellte die Platte (gemessen: rund 2500 Zeilen je
// Sekunde, seit die Handschlaege nebeneinander laufen) - und die echten
// Zeilen gingen darin unter. Gedrosselt wird je Art und Adresse: die erste
// Zeile einer Adresse kommt sofort, danach hoechstens alle QC_MELDEN_S
// Sekunden eine, mit der Zahl der dazwischen unterdrueckten. So geht die
// Zeile eines eigenen Geraets nicht in der Flut eines anderen unter. Je Art
// merkt sich die Drossel QC_DROSSEL_ADRESSEN Adressen; was darueber hinaus
// von weiteren Adressen kommt, solange die gemerkten noch Unterdruecktes
// offen haben, zaehlt nur noch gemeinsam ("von anderen Adressen"). Mehr als
// QC_DROSSEL_ADRESSEN + 1 Zeilen je Art und Frist gibt es also nie, auch
// nicht bei einer Flut von vielen Adressen. Was erst nach einer Freigabe
// geschieht (gekoppelt, Zuschauer verbunden), bleibt ungedrosselt.
#define QC_DROSSEL_ADRESSEN 4
typedef struct {
    int belegt;
    char von[INET_ADDRSTRLEN];        // "" = ohne Adresse (Ton)
    int64_t zuletzt_s;                // wann zuletzt eine Zeile fuer sie durchkam (monotone Uhr)
    long weitere;                     // seitdem unterdrueckt
} qc_drossel_platz;
typedef struct {
    const char *art;                  // fuer die Sammelzeile
    pthread_mutex_t m;
    qc_drossel_platz platz[QC_DROSSEL_ADRESSEN];
    int64_t sonst_s;                  // letzte Sammelzeile fuer die uebrigen Adressen
    long sonst;                       // seitdem unterdrueckt, ohne eigenen Platz
    char sonst_von[INET_ADDRSTRLEN];  // die letzte davon
} qc_drossel;
#define QC_DROSSEL(name) { .art = name, .m = PTHREAD_MUTEX_INITIALIZER, .sonst_s = -QC_MELDEN_S }

static qc_drossel d_bild_handschlag = QC_DROSSEL("Bildkanal: Handschlag gescheitert");
static qc_drossel d_liste_defekt    = QC_DROSSEL("Geraeteliste nicht lesbar oder beschaedigt");
// Zugangsphase (zugang.h): alles, was ein Unbekannter ausloesen kann. Was erst
// nach Passwort oder Klick geschieht (angenommen, zugelassen, abgelehnt,
// eingetragen), bleibt ungedrosselt.
static qc_drossel d_zugang          = QC_DROSSEL("Zugang noetig");
static qc_drossel d_zugang_voll     = QC_DROSSEL("Zugang: kein Platz");
static qc_drossel d_zugang_falsch   = QC_DROSSEL("Zugang: Passwort falsch");
static qc_drossel d_zugang_abbruch  = QC_DROSSEL("Zugang: abgebrochen");
static qc_drossel d_zugang_frist    = QC_DROSSEL("Zugang: Frist abgelaufen");
static qc_drossel d_selbst          = QC_DROSSEL("Abgewiesen: eigener Schluessel");
static qc_drossel d_ein_ohne_bild   = QC_DROSSEL("Eingabekanal abgewiesen: kein Bildkanal offen");
static qc_drossel d_ein_handschlag  = QC_DROSSEL("Eingabekanal: Handschlag gescheitert");
static qc_drossel d_ein_fremd       = QC_DROSSEL("Eingabekanal abgewiesen: andere Gegenstelle als beim Bild");
static qc_drossel d_ton_verworfen   = QC_DROSSEL("Ton verworfen: Leitung langsamer als der Ton");
static qc_drossel *const g_drosseln[] = {
    &d_bild_handschlag, &d_liste_defekt, &d_zugang, &d_zugang_voll, &d_zugang_falsch,
    &d_zugang_abbruch, &d_zugang_frist, &d_selbst,
    &d_ein_ohne_bild, &d_ein_handschlag, &d_ein_fremd, &d_ton_verworfen,
};

static int64_t mono_s(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec;
}

// Wie logf_, aber ueber die Drossel d. von: Adresse der Gegenstelle oder NULL.
// Formatiert wird nur, was durchkommt - eine Flut kostet so je Verbindung
// nur einen Griff an die Sperre und einen Blick auf ein paar Adressen.
static void logf_gedrosselt(qc_drossel *d, const char *von, NSString *fmt, ...) {
    int64_t t = mono_s();
    const char *a = von ? von : "";
    long vorher = 0;
    BOOL durch = NO;
    pthread_mutex_lock(&d->m);
    qc_drossel_platz *p = NULL, *frei = NULL;
    for (int i = 0; i < QC_DROSSEL_ADRESSEN; i++) {
        qc_drossel_platz *q = &d->platz[i];
        if (q->belegt && strcmp(q->von, a) == 0) { p = q; break; }
        // Ein Platz wird frei, wenn seine Adresse nichts mehr offen hat und
        // ihre Frist um ist - vorher zaehlt sie noch gegen die Obergrenze.
        if (!frei && (!q->belegt || (!q->weitere && t - q->zuletzt_s >= QC_MELDEN_S))) frei = q;
    }
    if (p) {
        durch = t - p->zuletzt_s >= QC_MELDEN_S;
        if (durch) {
            vorher = p->weitere;
            p->weitere = 0;
            p->zuletzt_s = t;
        } else {
            p->weitere++;
        }
    } else if (frei) {
        frei->belegt = 1;
        strlcpy(frei->von, a, sizeof frei->von);
        frei->zuletzt_s = t;
        frei->weitere = 0;
        durch = YES;
    } else {
        d->sonst++;
        strlcpy(d->sonst_von, a, sizeof d->sonst_von);
    }
    pthread_mutex_unlock(&d->m);
    if (!durch) return;
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    if (vorher)
        logf_(@"%@ - dazu %ld weitere%s seit der letzten Meldung", s, vorher,
              a[0] ? " von dieser Adresse" : "");
    else
        logf_(@"%@", s);
}

// Unterdrueckte Zeilen bleiben nicht liegen, auch wenn danach keine derselben
// Art und Adresse mehr kommt: der Dienst ruft das alle fuenf Sekunden, und
// nach der Frist geht die Zahl als Sammelzeile hinaus.
static void drosseln_nachtragen(void) {
    int64_t t = mono_s();
    for (size_t i = 0; i < sizeof g_drosseln / sizeof g_drosseln[0]; i++) {
        qc_drossel *d = g_drosseln[i];
        long n[QC_DROSSEL_ADRESSEN + 1] = {0};
        char von[QC_DROSSEL_ADRESSEN + 1][INET_ADDRSTRLEN];
        pthread_mutex_lock(&d->m);
        for (int k = 0; k < QC_DROSSEL_ADRESSEN; k++) {
            qc_drossel_platz *q = &d->platz[k];
            if (q->belegt && q->weitere && t - q->zuletzt_s >= QC_MELDEN_S) {
                n[k] = q->weitere;
                memcpy(von[k], q->von, sizeof von[k]);
                q->weitere = 0;
                q->zuletzt_s = t;
            }
        }
        if (d->sonst && t - d->sonst_s >= QC_MELDEN_S) {
            n[QC_DROSSEL_ADRESSEN] = d->sonst;
            memcpy(von[QC_DROSSEL_ADRESSEN], d->sonst_von, sizeof von[0]);
            d->sonst = 0;
            d->sonst_s = t;
        }
        pthread_mutex_unlock(&d->m);
        for (int k = 0; k < QC_DROSSEL_ADRESSEN; k++)
            if (n[k]) logf_(@"%s: %ld weitere%s%s seit der letzten Meldung", d->art, n[k], von[k][0] ? " von " : "", von[k]);
        if (n[QC_DROSSEL_ADRESSEN])
            logf_(@"%s: %ld weitere von anderen Adressen seit der letzten Meldung, zuletzt von %s", d->art,
                  n[QC_DROSSEL_ADRESSEN], von[QC_DROSSEL_ADRESSEN][0] ? von[QC_DROSSEL_ADRESSEN] : "?");
    }
}

// ------------------------------------------------- Undokumentierte Profile

static CFStringRef vt_sym(const char *name) {
    CFStringRef *p = (CFStringRef *)dlsym(RTLD_DEFAULT, name);
    return p ? *p : NULL;
}

static BOOL profile_supported(VTCompressionSessionRef s, CFStringRef profile) {
    if (!profile) return NO;
    CFDictionaryRef d = NULL;
    if (VTSessionCopySupportedPropertyDictionary(s, &d) != noErr || !d) return NO;
    BOOL ok = NO;
    CFDictionaryRef info = CFDictionaryGetValue(d, kVTCompressionPropertyKey_ProfileLevel);
    if (info && CFGetTypeID(info) == CFDictionaryGetTypeID()) {
        CFArrayRef list = CFDictionaryGetValue(info, kVTPropertySupportedValueListKey);
        if (list) {
            for (CFIndex i = 0; i < CFArrayGetCount(list); i++) {
                CFStringRef v = CFArrayGetValueAtIndex(list, i);
                if (v && CFStringCompare(v, profile, 0) == kCFCompareEqualTo) { ok = YES; break; }
            }
        }
    }
    CFRelease(d);
    return ok;
}

// --------------------------------------------------------------- Netzwerk

// Ein Zuschauer zur Zeit. Mehrere Sitzungen kommen spaeter, wenn das Protokoll steht.
// Ungesendete Bytes, ab denen Bilder gar nicht erst in den Encoder gehen
// (stau_vor_dem_encoder). 2 MB, weil ein einzelnes Vollbild selbst fast so
// gross wird - aus dem Hostprotokoll: im Stau des Spielmodus bei 500 Mbit/s
// ging nur noch ein Vollbild nach dem anderen raus, je 1,2-2,0 MB -, und
// nach einem Vollbild soll auf einer gesunden Leitung kein Bild ausfallen.
// Auf einer zu langsamen Leitung sind das hoechstens 2 MB Warteschlange, so
// viel wie vorher der volle Sendepuffer. Der Sendepuffer (tune_socket) ist
// doppelt so gross, damit die Regel ueberhaupt greifen kann.
#define QC_BACKLOG_LIMIT (2 * 1024 * 1024)
// Stau ohne jeden Fortschritt: so lange, dann gilt der Zuschauer als weg -
// dieselbe Frist wie SO_SNDTIMEO in tune_socket. Die greift im Stau nicht
// mehr, weil dann nichts mehr codiert und gesendet wird; ohne diese Frist
// bliebe ein eingefrorener Zuschauer (Ton aus) fuer immer eingetragen.
// Geprueft wird vor jedem Bild, das in den Encoder soll, und zusaetzlich im
// 5-s-Takt des Dienstes: bei stillem Bildschirm kommt kein Bild mehr an.
#define QC_STAU_FRIST_US (2 * 1000000ull)
// Abnahme in Schueben: macOS meldet freien Platz beim Empfaenger nicht
// laufend, sondern erst, wenn dort ein grosser Teil des Empfangspuffers frei
// ist (gemessen am 29.09.2026, macOS 27.0.1, Leser mit 1 Mbit/s ueber
// Loopback: rund 220 KB alle 1,8-2,1 s, SO_RCVBUF wird dabei uebergangen).
// Ein lebender, langsamer Zuschauer nimmt dann laenger als QC_STAU_FRIST_US
// scheinbar nichts ab. Deshalb gilt im Stau das Doppelte der laengsten
// Luecke zwischen zwei Abnahmen, solange ununterbrochen mehr als
// QC_SCHUB_AB im Kernel lag - mindestens QC_STAU_FRIST_US, hoechstens
// QC_STAU_FRIST_MAX_US. Wer nie etwas abnahm, ist nach QC_STAU_FRIST_US weg.
#define QC_SCHUB_AB (64 * 1024)
#define QC_STAU_FRIST_MAX_US (8 * 1000000ull)

static _Atomic int g_client_fd = -1;
static _Atomic int g_force_key = 0;
static _Atomic int g_wait_key = 0;   // frisch verbundener Zuschauer wartet auf ein Vollbild
static _Atomic long g_sent_frames = 0, g_skipped_backlog = 0;
static pthread_mutex_t g_send_mtx = PTHREAD_MUTEX_INITIALIZER;
static _Atomic long long g_sent_bytes = 0;

// --- Sicherheitsschicht -------------------------------------------------
// Jeder Kanal bekommt seinen eigenen Handschlag. Der Eingabekanal bindet sich
// ueber die Pruefsumme an den Bildkanal, damit er allein nichts wert ist.
static uint8_t g_id_priv[32], g_id_pub[32];
static qc_chan *g_vid = NULL;               // durch g_send_mtx geschuetzt
static _Atomic int g_vid_ready = 0;
static uint8_t g_vid_hh[QC_HASHLEN];        // Pruefsumme des Bildkanals
static uint8_t g_vid_peer[32];              // Schluessel des verbundenen Clients
static char g_last_sas[8] = {0};
// Der Eingabekanal gilt nur, solange sein Bildkanal lebt. Jeder Wechsel und
// jeder Verlust des Bildkanals beginnt eine neue Sitzung und bricht den
// Eingabekanal der alten ab.
static uint64_t g_sitzung = 0;              // durch g_send_mtx geschuetzt
// Laufender Stau: seit wann ohne Fortschritt (Hostuhr in us, 0 = kein Stau),
// dazu Rueckstand und gesendete Bytes beim letzten Blick. Durch g_send_mtx
// geschuetzt.
static uint64_t g_stau_seit = 0;
static int g_stau_rueckstand = 0;
static uint64_t g_stau_gesendet = 0;
// Abnahme in Schueben (QC_SCHUB_AB): Rueckstand und gesendete Bytes beim
// letzten Blick, Zeit der letzten Abnahme oder des letzten Blicks mit wenig
// Rueckstand (0 = noch keiner), des letzten Blicks ohne Abnahme danach
// (0 = keiner), die laengste Luecke und seit wann ununterbrochen wenig im
// Kernel liegt (0 = gerade nicht). Unter g_send_mtx.
static int g_schub_rueckstand = 0;
static uint64_t g_schub_gesendet = 0;
static uint64_t g_schub_zuletzt = 0;
static uint64_t g_schub_ohne = 0;
static uint64_t g_schub_luecke = 0;
static uint64_t g_schub_frei_seit = 0;
static _Atomic int g_in_fd = -1;            // Eingabekanal der laufenden Sitzung; gesetzt unter g_send_mtx
static uint64_t g_in_kanal = 0;             // seine Nummer; unter g_send_mtx
// Dateien (dateien.m): was der aktuelle Eingabekanal an Faehigkeiten gemeldet
// hat (IN_FAEHIGKEITEN). Gilt nur fuer diese Sitzung und diesen Kanal - ein
// neuer Zuschauer oder ein neuer Eingabekanal muss sie neu melden. Unter
// g_send_mtx.
static uint64_t g_faehig_sitzung = 0, g_faehig_kanal = 0;
static uint32_t g_faehig_bits = 0;
// HDR (hdr.h): das zuletzt gelesene IN_ANZEIGE und die Sitzung, in der es
// kam - es gilt nur fuer diese (anzeige_jetzt). Unter g_send_mtx.
static qc_hdr_anzeige g_anzeige;
static uint64_t g_anzeige_sitzung = 0;
static int g_anzeige_da = 0;
// HDR-Grund der zuletzt gesendeten Strominfo (Byte 13): ergibt eine neue
// Entscheidung einen anderen, geht eine neue Strominfo hinaus
// (hdr_neu_entscheiden).
static _Atomic int g_hdr_grund_gesendet = QC_HDR_GRUND_KEIN_IN_ANZEIGE;
static char g_vid_ip[INET_ADDRSTRLEN] = {0};  // Adresse des Zuschauers, fuers Protokoll; unter g_send_mtx
// Wer hereinkommt, steht in host-devices.txt (zugang.c, mit eigener kleiner
// Sperre nur fuer das Lesen und Eintragen). Eine Zugangsphase haelt keine
// Sperre, solange sie auf Passwort oder Klick wartet.

// Zustand fuer die Oberflaeche (qc_zustand_*). Der Name des Zuschauers hat
// eine eigene Sperre, ein Blatt: g_send_mtx kann bis zu 2 s haengen, das
// Menue darf das nicht. Gesetzt, bevor g_client_fd ihn als verbunden zeigt.
static pthread_mutex_t g_zustand_mtx = PTHREAD_MUTEX_INITIALIZER;
static char g_zuschauer_name[QC_ZUGANG_NAME_MAX + 1] = {0};
// Bildport, den ein anderes Programm belegt (0 = keiner): der Host wartet
// dann, statt zu enden, und versucht es alle 3 s erneut.
static _Atomic int g_port_belegt = 0;
// Die eine App hat die Freigabe ausgeschaltet (qc_dienst_anhalten): die
// Ports sind zu, und wer schon im Handschlag war, wird danach nicht mehr
// Zuschauer. 0 im Pruefstand und in der eigenen App.
static _Atomic int g_freigabe_aus = 0;
// Gesamtfrist einer Zugangsphase; der Pruefstand verkuerzt sie.
static int g_zugang_frist_ms = QC_ZUGANG_FRIST_MS;

// Was sich im Betrieb verstellen laesst. Der Client schickt Wuensche, der Host
// setzt sie um und meldet zurueck, was wirklich gilt.
extern VTCompressionSessionRef g_session;
static void set_i32(VTCompressionSessionRef s, CFStringRef key, int32_t v);

static SCStream *g_stream = nil;
static SCStreamConfiguration *g_cfg = nil;
static _Atomic int g_cur_mbit = 0, g_cur_fps = 0, g_cur_gaming = 0, g_cur_fixed = 0;
// Ton uebertragen? Sein Wunsch: als Schalter im Menue. Aus heisst: der
// Abgriff laeuft weiter, aber es geht kein Paket auf die Leitung - das
// spart die rund 3 Mbit/s des unverdichteten Tons.
static _Atomic int g_cur_ton = 1;

// Feste Bildrate: normalerweise liefert die Aufnahme nur dann ein Bild, wenn
// sich etwas geaendert hat. Mit diesem Schalter legt der Host das zuletzt
// gesehene Bild im Takt noch einmal nach, sodass die Rate steht, auch wenn der
// Bildschirm still ist. Alles hier unten wird ausschliesslich auf der
// Aufnahmewarteschlange angefasst - deshalb braucht es keine Sperre.
static CVPixelBufferRef g_last_pb = NULL;
static CMTime g_last_pts;
static uint64_t g_last_cap_us = 0;   // echte Aufnahmezeit des zuletzt gesehenen Bildes
// Ankunft des zuletzt gesehenen Bildes beim Grabber (Hostuhr, us). Fuer die
// Frage "kam seit einer Bildzeit nichts Neues?" zaehlt die Ankunft, nicht die
// Aufnahmezeit: ScreenCaptureKit liefert ein Bild einige Millisekunden nach
// seiner Aufnahme, mit der Aufnahmezeit reichte der Takt sonst mitten in einer
// Bewegung das aeltere Bild nach und verdraengte damit das neuere.
static uint64_t g_last_ankunft_us = 0;
// g_last_pb ist ein umgerechnetes Behelfsbild (letztes_bild_angleichen, nach
// einem Codecwechsel mit anderem Aufnahmeformat): das erste echte Bild danach
// geht als Vollbild in den Encoder.
static int g_behelf = 0;
static _Atomic long g_repeats = 0;
// Das zuletzt gesehene Bild ist noch nicht in den Encoder gegangen: es fiel
// als zu schnell, im Stau oder bei vollem Encoder weg. Ohne feste Bildrate
// liefert die Aufnahme bei stillem Bildschirm nichts mehr nach - der Takt
// reicht es dann nach (fixed_tick), ebenso fuer einen Zuschauer, der noch auf
// sein erstes Vollbild wartet (g_wait_key). Gesetzt von der Aufnahme,
// geloescht, wenn es codiert ist.
static _Atomic int g_bild_offen = 0;
static _Atomic long g_nachgereicht = 0;
// Schrittmacher. ScreenCaptureKit haelt sich nicht an minimumFrameInterval:
// gemessen 142-149 echte Bilder je Sekunde bei Ziel 120 - und jedes davon
// ging durch Encoder, Leitung und Decoder. Feste Zeitschlitze im Zielabstand;
// ein Bild bekommt den naechsten freien Schlitz oder faellt weg.
static double g_schlitz = 0;                 // Hostuhr in Sekunden: naechster freier Schlitz
static _Atomic long g_zu_schnell = 0;        // verworfen, weil schneller als die Zielrate
// Bilder im Encoder, fuer die noch keine Ausgabe kam. Ein langsamer Encoder
// (H.264 bei 500 Mbit/s: 30 Bilder/s) bekam sonst 120 Bilder je Sekunde in
// die Warteschlange gelegt - eine halbe Sekunde Verzoegerung statt Auslassern.
static _Atomic int g_inflight = 0;
// So viele duerfen es sein; ein echtes Bild darueber faellt weg ("Encoder
// voll"), der Takt legt schon ab dieser Zahl nichts nach. Zwei: eins in
// Arbeit, das naechste wartet - der Encoder laeuft nie leer. Bei nativem 4K
// ist der Encoder der Engpass (M1, jedes Bild anders, hoechstens zwei offen:
// HEVC 4:4:4 10 Bit 3840x2160 85 Bilder/s, 11,8 ms je Bild); mit drei offenen
// wartete jedes Bild hinter zwei anderen, rund 12 ms mehr Verzoegerung fuer
// keinen Durchsatz.
#define QC_ENCODER_OFFEN_MAX 2
static _Atomic long g_enc_stau = 0;          // verworfen, weil der Encoder noch voll war
static dispatch_queue_t g_capq = NULL;
static dispatch_source_t g_tick = NULL;

// g_stream gehoert der Lebenslauf-Warteschlange (g_lifeq): nur von dort wird
// er gesetzt und ausgetragen, nacheinander mit Auf- und Abbau. Geschrieben
// wird er dabei auf der Aufnahmewarteschlange, weil Codecwechsel und
// Einstellungen ihn dort lesen - so sieht keiner einen halb getauschten Strom.
static void stream_setzen(SCStream *st) {
    dispatch_sync(g_capq, ^{ g_stream = st; });
}

static const char *QC_PRO_VIDEO = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
static const char *QC_PRO_INPUT = "QuadChroma/1 input Noise_XX_25519_ChaChaPoly_SHA256";

// --- Protokoll ---------------------------------------------------------
// Beim Verbinden: 4 Byte Kennung "QCH1".
// Danach Nachrichten: u8 Typ, u8 Flags, u16 frei, u32 Laenge (little endian).
//   Typ 1 = Strominfo Fassung 1 (24 Byte): u16 Breite, u16 Hoehe, u16 FPS, u8 Codec, u8 Format,
//           dann Fassung, Farbe, HDR-Grund und Metadaten (hdr.h; vor 0.2.0 nur die acht Byte)
//   Typ 2 = Bilddaten (Annex-B), Flag Bit 0 = Vollbild
#define QC_MAGIC      "QCH1"
#define QC_MSG_INFO   1
#define QC_MSG_VIDEO  2
#define QC_FLAG_KEY   0x01

typedef struct __attribute__((packed)) {
    uint8_t type, flags;
    uint16_t reserved;
    uint32_t len;
} qc_hdr;

typedef struct {
    const char *name;
    CMVideoCodecType codec;
    const char *profil;
    int zehn_bit;
    int chroma444;
} qc_codec_kandidat;

static const qc_codec_kandidat g_kandidaten[] = {
    { "HEVC 4:4:4 10 Bit", kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main44410_AutoLevel", 1, 1 },
    { "HEVC 4:4:4 8 Bit",  kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main444_AutoLevel",   0, 1 },
    { "HEVC 4:2:0 10 Bit", kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main10_AutoLevel",    1, 0 },
    { "HEVC 4:2:0 8 Bit",  kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main_AutoLevel",      0, 0 },
    { "H.264 High",        kCMVideoCodecType_H264, "kVTProfileLevel_H264_High_AutoLevel",      0, 0 },
    { "AV1",               'av01',                 "kVTProfileLevel_AV1_Main_AutoLevel",       0, 0 },
};

#define QC_KANDIDATEN (sizeof g_kandidaten / sizeof g_kandidaten[0])

// hdr: dieser Kandidat kann HDR10 (BT.2020, PQ) - macOS 15, Apple Silicon,
// HEVC 10 Bit, und der Encoder nimmt die PQ-Eigenschaften (codecs_pruefen).
typedef struct {
    int vorhanden;
    int hardware;
    int hdr;
} qc_codec_befund;

static qc_codec_befund g_befund[QC_KANDIDATEN];

// --- Codecwahl ------------------------------------------------------------
// Welcher Kandidat gerade codiert. Alles, was der Client ueber den Strom
// wissen muss, leitet sich hieraus ab - nicht aus dem Zustand beim Start.
static _Atomic int g_codec_id = 0;

// --- Stromgroesse und H.264 -------------------------------------------------
// Gestreamt wird nativ (stromgroesse_fuer). Die einzige Ausnahme ist H.264:
// ist die native Groesse groesser als seine Grenze (bildschirm.h: 4096 je
// Seite, 4096x2304), wird der Strom darin eingepasst, bei gleichem
// Seitenverhaeltnis - sonst oeffnete der Encoder gar nicht (VideoToolbox
// H.264 auf dem M1: -12903 ab 4097 Bildpunkten je Seite), ein Zuschauer, der
// nur H.264 kann, bekaeme kein Bild, und ein Bildschirmwechsel oder eine
// neue Aufloesung bei laufendem H.264 endete in Wiederholungen ohne Ende.
// Start, Wiederherstellung, Bildschirmwechsel und Codecwechsel nehmen alle
// dieselbe Regel (groesse_fuer_codec). Die Grenze bestaetigt codecs_pruefen
// mit dem Encoder selbst.
static qc_h264_grenze g_h264_grenze = { QC_H264_MAX_SEITE, QC_H264_MAX_MB };
// Die native Stromgroesse des laufenden Stroms (vor dem Einpassen fuer
// H.264) und die Breite des gestreamten Bildschirms in Punkten - fuer einen
// Codecwechsel, der die Stromgroesse aendert, und den Zeigermassstab.
// Gesetzt mit jedem Strom und jeder Bewertung desselben Bildschirms.
static _Atomic int g_strom_nativ_w = 0, g_strom_nativ_h = 0;
static _Atomic size_t g_zeiger_punkte = 0;

// --- HDR (HDR-Plan 5.1) ----------------------------------------------------
// Die Farbe ist die zweite Dimension des Codecwechsels: Kandidat und Farbe
// zusammen bestimmen Aufnahme, Encoder, SWITCH und Strominfo.
// Farbe des laufenden Encoders: 1 = HDR10 (BT.2020, PQ, SEI 137/144 vor jedem
// Vollbild), 0 = SDR (BT.709). Gesetzt von encoder_start zusammen mit
// g_session - was auf der Leitung ist, nicht was gewuenscht ist.
static _Atomic int g_farbe_pq = 0;
// Der aufgenommene Bildschirm kann HDR (EDR-Kopfraum potentiell ueber 1.0).
// Gesetzt auf g_lifeq (quelle_setzen).
static _Atomic int g_quelle_hdr = 0;
// Der letzte Wechsel nach HDR scheiterte: SDR mit Grund 6, bis sich die Lage
// aendert (neuer Zuschauer, anderes IN_ANZEIGE, HDR am Bildschirm an/aus,
// anderer Bildschirm) - sonst versuchte der Host es bei jeder Gelegenheit neu.
static _Atomic int g_hdr_gescheitert = 0;
// Das erste Bild nach dem Start oder einem Farbwechsel: Format und Anhaenge
// ins Protokoll (was ScreenCaptureKit wirklich liefert).
static _Atomic int g_anhaenge_loggen = 0;

// --- Aufnahme wiederherstellen ----------------------------------------
// Faellt der Bildschirm weg (Monitor aus, Displayschlaf, Neuerkennung),
// beendet ScreenCaptureKit die Aufnahme. Ein Fernsteuerungs-Host muss das
// ueberleben: er wartet, bis wieder ein Bildschirm da ist, und baut die
// Aufnahme dann selbst neu auf. Der Client erfaehrt derweil, woran es liegt.
static id g_grab = nil;                       // der Empfaenger der Aufnahme, wird wiederverwendet
static _Atomic int g_fixed_gewollt = 0;       // was der Benutzer will, unabhaengig vom Ausfall
static int g_kein_bildschirm_gemeldet = 0;
static IOPMAssertionID g_wach = kIOPMNullAssertionID;
// Ohne Bildschirmaufnahme-Freigabe endet der Host nicht mehr (Spezifikation
// 7.4): ein Zuschauer kommt trotzdem herein, bekommt Hoststatus 1 wie bei
// einem fehlenden Bildschirm, und die Wiederherstellung versucht es alle 3 s,
// bis die Freigabe da ist. Dann bekommt er SWITCH und INFO, als haette er den
// Bildschirm gewechselt (seine Begruessung kannte nur Ersatzmasse).
static _Atomic int g_ohne_aufnahme = 0;
// Freigabe fuer die Bildschirmaufnahme. Ersetzbar, damit der Pruefstand den
// Weg ohne Freigabe fahren kann.
static BOOL tcc_bildschirm_system(void) { return CGPreflightScreenCaptureAccess(); }
static BOOL (*g_tcc_bildschirm)(void) = tcc_bildschirm_system;
static void aufnahme_wiederherstellen(void);
static void hoststatus_senden(uint8_t lage);
static void bildschirme_senden(void);
// Kein Zuschauer, keine Arbeit: Aufnahme und Encoder leben nur, solange
// jemand verbunden ist. Auf- und Abbau laufen streng nacheinander auf einer
// eigenen Warteschlange, damit ein gehender und ein kommender Zuschauer sich
// nicht ins Gehege kommen.
static dispatch_queue_t g_lifeq = NULL;
static void stream_herunterfahren_anstossen(void);
static BOOL stream_hochfahren_sync(void);
// Zuschauer zwischen dem Hochfahren der Aufnahme und dem Eintragen. Auch sie
// brauchen Aufnahme und Encoder: ein Abbau, der in diesem Fenster laeuft (der
// Vorgaenger ging genau jetzt - Senden gescheitert, Stau), hielte sonst den
// Strom an, und der Neue saesse eingetragen, aber ohne Bild da.
static _Atomic int g_anmeldend = 0;

// Braucht noch jemand Aufnahme und Encoder? Wer zuschaut oder gerade
// eingetragen wird. Erst g_anmeldend, dann g_client_fd lesen: bild_verbindung
// traegt erst ein und zaehlt dann ab - so entgeht keiner beiden Blicken.
static BOOL zuschauer_braucht_strom(void);

// Aufnahmeformat je Kandidat. ScreenCaptureKit kennt kein 4:4:4 mit 8 Bit,
// also muss VideoToolbox dort umrechnen - und das steht dann dran, denn eine
// stille Umrechnung verfaelscht genau den Vergleich, um den es geht.
static OSType pixfmt_fuer(int idx) {
    switch (idx) {
        case 0: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44, ohne Kopie
        case 1: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44 -> 8 Bit, Umrechnung
        case 2: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44 -> 4:2:0, Umrechnung (Vollbereich bleibt)
        case 3: return kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;  // 420f, ohne Kopie
        case 4: return kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;  // 420f, ohne Kopie
        default: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    }
}
static int umrechnung_fuer(int idx) { return idx == 1 || idx == 2; }
static int ist_h264(int idx) { return g_kandidaten[idx].codec == kCMVideoCodecType_H264; }

// Die Stromgroesse fuer Kandidat idx aus der nativen nw x nh: nativ, bei
// H.264 in dessen Grenze eingepasst (siehe g_h264_grenze). melden: die
// Zeile dazu, einmal je Groesse - nicht bei jeder Bewertung und nicht fuer
// --list. Aus jedem Faden.
static void groesse_fuer_codec(int nw, int nh, int idx, int melden, int *w, int *h) {
    *w = nw;
    *h = nh;
    if (!ist_h264(idx) || !qc_h264_einpassen(nw, nh, g_h264_grenze, w, h) || !melden) return;
    static _Atomic uint64_t gemeldet = 0;
    uint64_t k = ((uint64_t)(uint16_t)nw << 48) | ((uint64_t)(uint16_t)nh << 32) |
                 ((uint64_t)(uint16_t)*w << 16) | (uint64_t)(uint16_t)*h;
    if (atomic_exchange(&gemeldet, k) != k)
        logf_(@"Stromgroesse: %dx%d ist fuer H.264 zu gross (hoechstens %d Bildpunkte je Seite und %d Makrobloecke) - "
              @"eingepasst auf %dx%d bei gleichem Seitenverhaeltnis, die einzige Ausnahme von nativ",
              nw, nh, g_h264_grenze.max_seite, g_h264_grenze.max_mb, *w, *h);
}

// --- HDR: Aufnahme, Metadaten, Faehigkeit -----------------------------------
//
// Wie ScreenCaptureKit HDR liefert, misst scripts/hdrprobe.m (HDR-Plan G0).
// Vorgabe: Dynamikumfang lokal, xf44, Display P3 mit PQ und Matrix BT.709 -
// VideoToolbox rechnet dann P3 nach BT.2020 um (steht in SWITCH p[5] und im
// Protokoll). Gemessen am 29.09.2026 (macOS 27, M1, X27 X1 und virtueller
// Bildschirm, HDR-Video laeuft): kanonisch (wie Apples Voreinstellung
// HDRStreamCanonicalDisplay) kappt bei rund 120 nit, lokal liefert die
// Spitzen (bis 2471 nit); SDR-Weiss liegt in beiden bei 100 nit. Fuer die
// Abnahme umstellbar ueber die Umgebungsvariable QC_HDR_AUFNAHME: lokal-p3
// (Vorgabe), kanonisch-p3, lokal-2100, kanonisch-2100 (BT.2100 PQ mit Matrix
// BT.2020: ohne Umrechnung).
typedef struct { int lokal, bt2100; } qc_hdr_aufnahme_art;

static qc_hdr_aufnahme_art hdr_aufnahme_art(void) {
    static qc_hdr_aufnahme_art art = { 1, 0 };
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{
        const char *e = getenv("QC_HDR_AUFNAHME");
        if (e && *e) {
            art.lokal = strstr(e, "kanonisch") == NULL;
            art.bt2100 = strstr(e, "2100") != NULL;
        }
    });
    return art;
}

static const char *hdr_aufnahme_text(void) {
    qc_hdr_aufnahme_art a = hdr_aufnahme_art();
    return a.lokal ? (a.bt2100 ? "lokal, BT.2100 PQ, Matrix BT.2020" : "lokal, Display P3 PQ, Matrix BT.709")
                   : (a.bt2100 ? "kanonisch, BT.2100 PQ, Matrix BT.2020" : "kanonisch, Display P3 PQ, Matrix BT.709");
}

// Rechnet VideoToolbox die HDR-Aufnahme um (Display P3 -> BT.2020)?
static int hdr_p3_umrechnung(void) { return !hdr_aufnahme_art().bt2100; }

// SDR-Weiss der HDR-Aufnahme in nit - wohin ScreenCaptureKit das Weiss eines
// SDR-Fensters im PQ-Signal legt. BT.2408 und VideoToolbox (SDR -> PQ, im
// Versuch gemessen: Code 594) nehmen 203, ScreenCaptureKit aber 100 (G0 am
// 29.09.2026: Code 520 auf beiden Bildschirmen, kanonisch wie lokal). Fuer
// die Abnahme ueber QC_HDR_SDR_WEISS (50..1000) umstellbar.
static uint16_t hdr_sdr_weiss_nit(void) {
    static uint16_t weiss = 100;
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{
        const char *e = getenv("QC_HDR_SDR_WEISS");
        long v = e ? strtol(e, NULL, 10) : 0;
        if (v >= 50 && v <= 1000) weiss = (uint16_t)v;
    });
    return weiss;
}

// Mastering-Angaben des Stroms (SEI 137, VideoToolbox, Strominfo): der
// Bildschirm eines Macs, Display P3 mit D65, 1000 nit Spitze, 0,005 nit
// Schwarz. MaxCLL und MaxFALL sind bei einem Bildschirmstrom unbekannt: 0.
#define QC_HDR_MASTER_MAX_NIT 1000u
#define QC_HDR_MASTER_MIN     50u          // 0,005 nit in 0,0001 nit

static void hdr_sei_werte(qc_hdr_sei_werte *w) {
    memset(w, 0, sizeof *w);
    qc_hdr_sei_primaer(w, 0);
    w->max_lum = QC_HDR_MASTER_MAX_NIT * 10000u;
    w->min_lum = QC_HDR_MASTER_MIN;
    w->max_cll = 0;
    w->max_fall = 0;
}

// Die Strominfo-Farbe fuer HDR10, wie sie auf der Leitung ist.
static void hdr_info_pq(qc_hdr_info *i) {
    memset(i, 0, sizeof *i);
    i->transfer = QC_HDR_TRANSFER_PQ;
    i->primaer = QC_HDR_PRIMAER_2020;
    i->matrix = QC_HDR_MATRIX_2020_NCL;
    i->voll = 1;
    i->grund = QC_HDR_GRUND_AKTIV;
    i->sdr_weiss_nit = hdr_sdr_weiss_nit();
    i->master_max_nit = QC_HDR_MASTER_MAX_NIT;
    i->master_min_zehntausendstel = QC_HDR_MASTER_MIN;
}

// Aufnahme fuer SDR oder HDR einstellen: Dynamikumfang, Farbraum, Matrix.
// Das Pixelformat bleibt Sache von pixfmt_fuer (xf44 auch bei HDR - 4:4:4
// 10 Bit ohne Kopie). SDR ist der Stand von frueher: sRGB, Matrix nach
// Vorgabe von ScreenCaptureKit.
static void aufnahme_farbe_setzen(SCStreamConfiguration *cfg, int pq) {
    if (!cfg) return;
    if (pq) {
        qc_hdr_aufnahme_art a = hdr_aufnahme_art();
        if (@available(macOS 15.0, *))
            cfg.captureDynamicRange = a.lokal ? SCCaptureDynamicRangeHDRLocalDisplay : SCCaptureDynamicRangeHDRCanonicalDisplay;
        cfg.colorSpaceName = a.bt2100 ? kCGColorSpaceITUR_2100_PQ : kCGColorSpaceDisplayP3_PQ;
        cfg.colorMatrix = a.bt2100 ? kCVImageBufferYCbCrMatrix_ITU_R_2020 : kCVImageBufferYCbCrMatrix_ITU_R_709_2;
    } else {
        if (@available(macOS 15.0, *)) cfg.captureDynamicRange = SCCaptureDynamicRangeSDR;
        cfg.colorSpaceName = kCGColorSpaceSRGB;
        // Die Matrix bleibt, wie immer schon, bei der Vorgabe von
        // ScreenCaptureKit (nicht gesetzt); nur nach HDR wird sie ausdruecklich
        // BT.709 - das, was der Strom im VUI sagt.
        if (cfg.colorMatrix) cfg.colorMatrix = kCVImageBufferYCbCrMatrix_ITU_R_709_2;
    }
}

// Ist die Aufnahme auf HDR eingestellt? (Am Farbraum abgelesen.)
static int aufnahme_ist_pq(SCStreamConfiguration *cfg) {
    CFStringRef cs = cfg ? cfg.colorSpaceName : NULL;
    return cs && (CFEqual(cs, kCGColorSpaceDisplayP3_PQ) || CFEqual(cs, kCGColorSpaceITUR_2100_PQ));
}

// Kann dieser Mac ueberhaupt HDR aufnehmen? ScreenCaptureKit liefert HDR ab
// macOS 15 und nur auf Apple Silicon (auch unter Rosetta: hw.optional.arm64).
static int hdr_system_kann(void) {
    if (@available(macOS 15.0, *)) {
        int arm = 0;
        size_t n = sizeof arm;
        if (sysctlbyname("hw.optional.arm64", &arm, &n, NULL, 0) != 0) arm = 0;
        return arm != 0;
    }
    return 0;
}

// Format und Farbangaben eines Bildes, fuers Protokoll:
// "xf44 1920x1080, P3_D65 / SMPTE_ST_2084_PQ / ITU_R_709_2".
static NSString *anhaenge_text(CVPixelBufferRef pb) {
    if (!pb) return @"(kein Bild)";
    OSType t = CVPixelBufferGetPixelFormatType(pb);
    uint32_t be = CFSwapInt32HostToBig(t);
    CFTypeRef keys[3] = { kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrixKey };
    NSMutableArray<NSString *> *w = [NSMutableArray array];
    for (int i = 0; i < 3; i++) {
        CFTypeRef v = CVBufferCopyAttachment(pb, (CFStringRef)keys[i], NULL);
        [w addObject:v && CFGetTypeID(v) == CFStringGetTypeID() ? (__bridge NSString *)v : @"-"];
        if (v) CFRelease(v);
    }
    return [NSString stringWithFormat:@"%.4s %zux%zu, %@ / %@ / %@", (char *)&be, CVPixelBufferGetWidth(pb),
            CVPixelBufferGetHeight(pb), w[0], w[1], w[2]];
}

static int g_info_w = 0, g_info_h = 0, g_info_fps = 0;

// Codecwechsel im Betrieb. Laeuft ausschliesslich auf der Aufnahmewarteschlange;
// die Eingabe reicht den Wunsch nur dorthin weiter.
static void codec_wechseln(int idx);
static void zeiger_massstab_nachfuehren(size_t punkte, int strom_w);
// Bildschirmwunsch (Nachricht 70). Laeuft ausschliesslich auf der
// Lebenslauf-Warteschlange; die Eingabe reicht ihn nur dorthin weiter.
static void bildschirm_wunsch_setzen(NSString *kennung);

// Alle Zeiten des Hosts kommen von derselben Uhr wie die Bildzeitstempel der
// Aufnahme. Nur so lassen sich Aufnahme, Encoder und Versand vergleichen.
static uint64_t now_us(void) {
    CMTime t = CMClockGetTime(CMClockGetHostTimeClock());
    if (!CMTIME_IS_VALID(t)) return 0;
    CMTime us = CMTimeConvertScale(t, 1000000, kCMTimeRoundingMethod_Default);
    return (uint64_t)us.value;
}

static uint64_t cmtime_us(CMTime t) {
    if (!CMTIME_IS_VALID(t)) return 0;
    CMTime us = CMTimeConvertScale(t, 1000000, kCMTimeRoundingMethod_Default);
    return (uint64_t)us.value;
}

static _Atomic unsigned g_seq = 0;
static _Atomic int g_formattest = 0;
// Mittlere Encoderzeit je Bild in Mikrosekunden. Die Video-Einheit meldet ihre
// Auslastung nirgends; das hier ist die Groesse, die statt dessen etwas sagt.
static _Atomic long long g_enc_us = 0;
static _Atomic long g_enc_n = 0;

// Gegen halboffene Leichen: eine Gegenstelle, die ohne Abschied verschwindet
// (Deckel zu, WLAN weg), faellt nach rund 16 s Stille auf - 10 s warten, dann
// drei Proben im Abstand von 2 s.
static void keepalive_setzen(int fd) {
    int one = 1, idle = 10, intv = 2, cnt = 3;
    setsockopt(fd, SOL_SOCKET, SO_KEEPALIVE, &one, sizeof one);
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPALIVE, &idle, sizeof idle);
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPINTVL, &intv, sizeof intv);
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPCNT, &cnt, sizeof cnt);
}

static void tune_socket(int fd) {
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    // 4 MB Sendepuffer: die Stauregel (QC_BACKLOG_LIMIT) soll die Grenze
    // sein, nicht ein blockierendes Senden. SO_NWRITE kann nie ueber die
    // Puffergroesse steigen - mit 2 MB griff die 2-MB-Regel ohne Spielmodus
    // nie. Unter der Regel liegen hoechstens 2 MB plus die Bilder, die schon
    // im Encoder stecken, im Kernel; mehr Verzoegerung als vorher entsteht nicht.
    int snd = 1 << 22;
    setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &snd, sizeof snd);
    int lowat = 64 * 1024;                   // nicht mehr als das im Kernel stauen lassen
    setsockopt(fd, IPPROTO_TCP, TCP_NOTSENT_LOWAT, &lowat, sizeof lowat);
    keepalive_setzen(fd);
    // Eine Gegenstelle, die nichts mehr annimmt (eingefroren, Deckel zu),
    // darf den Versand nicht fuer immer anhalten: er laeuft unter g_send_mtx,
    // und dahinter wartet auch der naechste Zuschauer. Zwei Sekunden ohne
    // jeden Fortschritt, dann gilt sie als weg.
    struct timeval sto = { .tv_sec = 2, .tv_usec = 0 };
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &sto, sizeof sto);
}

// Kein Bild, keine Eingabe: neue Sitzung, und der Eingabekanal der alten wird
// abgebrochen. Nur unter g_send_mtx rufen. shutdown statt close: der
// Eingabefaden kehrt aus seinem Lesen zurueck und schliesst seinen fd selbst,
// ebenfalls unter g_send_mtx - so trifft das shutdown nie eine Nummer, die
// inzwischen einer anderen Verbindung gehoert.
static void eingabe_abbrechen(void) {
    g_sitzung++;
    int alt = atomic_exchange(&g_in_fd, -1);
    if (alt >= 0) shutdown(alt, SHUT_RDWR);
    // Laufende Dateiuebertragungen gehoeren zur alten Sitzung: verwerfen.
    // Reiht nur ein, blockiert nie.
    qc_dateien_sitzung_vorbei(g_sitzung);
}

static int backlog_bytes(int fd) {
    int n = 0; socklen_t len = sizeof n;
    if (getsockopt(fd, SOL_SOCKET, SO_NWRITE, &n, &len) != 0) return 0;
    return n;
}


// Ton im Dauerstau. Ton geht an der Stauregel vorbei - er ist klein und soll
// nicht warten. Liegt die Leitung aber unter der Tonrate (unverdichtet rund
// 3 Mbit/s: VPN, schwaches WLAN), fuellt er allein den Sendepuffer bis oben:
// kein Bild kaeme mehr durch die Stauregel, und der Zuschauer bliebe (er
// nimmt ja ab) mit stehendem Bild und einem Ton, der immer weiter nachhinkt.
// Deshalb faellt Ton weg, wenn der Rueckstand QC_STAU_FRIST_US lang ohne
// Unterbrechung ueber der Staugrenze liegt. Liegt er nur kurz darueber - nach
// einem grossen Vollbild, oder im gewoehnlichen Stau, in dem die Stauregel
// ein Bild durchlaesst, sobald er darunter faellt -, bleibt der Ton ganz. Die
// Luecken ueberbrueckt der Tonpuffer des Clients. Nur unter g_send_mtx.
static uint64_t g_ton_stau_seit = 0;          // 0 = Rueckstand gerade unter der Grenze
static int ton_verwerfen(int fd) {
    int grenze = atomic_load(&g_cur_gaming) ? QC_BACKLOG_LIMIT / 4 : QC_BACKLOG_LIMIT;
    if (backlog_bytes(fd) <= grenze) { g_ton_stau_seit = 0; return 0; }
    uint64_t jetzt = now_us();
    if (!g_ton_stau_seit) g_ton_stau_seit = jetzt;
    return jetzt - g_ton_stau_seit >= QC_STAU_FRIST_US;
}

// Eine kleine Nachricht an den eingetragenen Zuschauer fd. Nur unter
// g_send_mtx. 0 = gesendet; sonst ist er jetzt ausgetragen.
static int klein_senden_gesperrt(int fd, uint8_t type, const void *data, size_t len) {
    qc_hdr h = { .type = type, .flags = 0, .reserved = 0, .len = (uint32_t)len };
    struct iovec iov[2];
    iov[0].iov_base = &h;   iov[0].iov_len = sizeof h;
    iov[1].iov_base = (void *)data; iov[1].iov_len = len;
    if (qc_chan_send(g_vid, iov, len ? 2 : 1) == 0) return 0;
    logf_(@"Zuschauer weg: Senden gescheitert (%s)", strerror(errno));
    atomic_store(&g_vid_ready, 0);
    atomic_store(&g_client_fd, -1);
    close(fd);
    eingabe_abbrechen();
    qc_chan_free(g_vid);
    g_vid = NULL;
    stream_herunterfahren_anstossen();
    qc_ui_zustand_geaendert();
    return -1;
}

// Kleine Nachricht ueber die Bildverbindung. Umgeht bewusst die Vollbild-Sperre
// und die Stauregel: Ton und Zwischenablage sind winzig und duerfen nicht warten.
// ton: Ton, der im Dauerstau wegfaellt (ton_verwerfen). Rueckgabe 1 = deshalb
// verworfen, sonst 0 (gesendet oder niemand da).
static int send_small_bis(uint8_t type, const void *data, size_t len, int ton) {
    int verworfen = 0;
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (fd >= 0 && atomic_load(&g_vid_ready) && ton && ton_verwerfen(fd)) {
        verworfen = 1;
    } else if (fd >= 0 && atomic_load(&g_vid_ready)) {
        (void)klein_senden_gesperrt(fd, type, data, len);
    }
    pthread_mutex_unlock(&g_send_mtx);
    return verworfen;
}

static void send_small(uint8_t type, const void *data, size_t len) {
    (void)send_small_bis(type, data, len, 0);
}

// Wie send_small, aber an eine Sitzung gebunden: nach einem Zuschauerwechsel
// geht nichts mehr hinaus - Quittungen und Dateistuecke gehoeren nur zu dem
// Zuschauer, mit dem die Uebertragung begann. Jeder Aufruf haelt g_send_mtx
// nur fuer diese eine Nachricht (ein Dateistueck: hoechstens 48 KiB).
// 1 = gesendet, 0 = die Sitzung ist vorbei.
static int send_small_sitzung(uint64_t sitzung, uint8_t type, const void *data, size_t len) {
    int gesendet = 0;
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (sitzung == g_sitzung && fd >= 0 && atomic_load(&g_vid_ready) && g_vid)
        gesendet = klein_senden_gesperrt(fd, type, data, len) == 0;
    pthread_mutex_unlock(&g_send_mtx);
    return gesendet;
}

// Ungesendete Bytes im Sendepuffer des Zuschauers dieser Sitzung; -1 = die
// Sitzung ist vorbei. Der Dateisender drosselt sich damit selbst: Dateidaten
// zaehlen in SO_NWRITE mit und loesten sonst die Stauregel fuer Bilder aus.
static int rueckstand_sitzung(uint64_t sitzung) {
    int r = -1;
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (sitzung == g_sitzung && fd >= 0 && atomic_load(&g_vid_ready)) r = backlog_bytes(fd);
    pthread_mutex_unlock(&g_send_mtx);
    return r;
}

#define QC_MSG_SETTINGS   3    // Host -> Client: was gerade gilt
#define QC_MSG_TIME       4    // Host -> Client: Antwort auf den Zeitabgleich
#define QC_MSG_STAMP      5    // Host -> Client: Zeitstempel zum naechsten Bild
#define QC_MSG_LAST       6    // Host -> Client: Auslastung des Hosts
#define QC_MSG_SWITCH     7    // Host -> Client: ab hier neuer Codec, Decoder zuruecksetzen
#define QC_MSG_CODECS     8    // Host -> Client: was dieser Mac codieren kann
#define QC_IN_CODEC       66   // Client -> Host: Codecwunsch (u8 Index)
#define QC_IN_TESTBILD    68   // Client -> Host: Testbild (u8: 1 an, 0 aus) fuer den Benchmark
// 12 MSG_BILDSCHIRME (Host -> Client) und 70 IN_BILDSCHIRM (Client -> Host):
// Bildschirmliste und -wunsch, siehe bildschirm.h.

// Testbild statt Aufnahme: solange es an ist, verwirft die Aufnahme ihre
// Bilder, und der Takt speist die vorgerenderte Schleife in Zielrate.
static _Atomic int g_testbild = 0;
#define QC_MSG_HOSTSTATUS 9    // Host -> Client: u8 Lage (0 = in Ordnung, 1 = kein Bildschirm)
#define QC_MSG_ABGELOEST  10   // Host -> Client: ein anderer Zuschauer hat uebernommen, Laenge 0, letzte Nachricht
// 13 QC_MSG_HOST_ENDE (Host -> Client): Abschied mit u8 Grund, letzte
// Nachricht vor dem Schliessen - Kodierung und Gruende in zugang.h.
#define QC_IN_TIME        65   // Client -> Host: Frage zum Zeitabgleich
#define QC_IN_SETTINGS    64   // Client -> Host: was gewuenscht wird
static void hoststatus_senden(uint8_t lage) {
    uint8_t b[2] = { lage, 0 };
    send_small(QC_MSG_HOSTSTATUS, b, sizeof b);
}

#define QC_MSG_AUDIO_INFO 32
#define QC_MSG_AUDIO      33
#define QC_MSG_CLIP       48
// Host -> Client: Zeigerform. 12 Byte Kopf (u16 Breite, u16 Hoehe, u16 Hotspot x,
// u16 Hotspot y, u8 sichtbar, u8 Massstab (1 = Punkte), u16 frei), dann RGBA.
#define QC_MSG_CURSOR     49

static _Atomic int g_audio_info_sent = 0;
static _Atomic long g_audio_packets = 0;
static _Atomic long long g_audio_bytes = 0;
static _Atomic long g_audio_verworfen = 0;      // im Dauerstau (ton_verwerfen)
// Was zuletzt angesagt wurde. Nur im Ton-Rueckruf angefasst, und der laeuft
// auf der seriellen Ton-Warteschlange.
static uint32_t g_audio_info_rate = 0;
static uint8_t g_audio_info_ch = 0;

static void audio_cb(const float *pcm, size_t frames, uint32_t rate, uint8_t channels) {
    if (atomic_load(&g_client_fd) < 0) return;
    // Ton abgeschaltet: nichts auf die Leitung. Die Formatansage bleibt
    // fuer das erste Paket nach dem Wiedereinschalten liegen.
    if (!atomic_load(&g_cur_ton)) return;
    // Angesagt wird fuer jeden neuen Zuschauer und immer dann, wenn
    // ScreenCaptureKit Rate oder Kanalzahl wechselt - sonst rechnete der
    // Client mit der alten Rate um, und der Ton klaenge zu hoch oder zu tief.
    int neu = !atomic_exchange(&g_audio_info_sent, 1);
    if (neu || rate != g_audio_info_rate || channels != g_audio_info_ch) {
        if (!neu) logf_(@"Tonformat gewechselt: %u Hz, %u Kanaele - neu angesagt", rate, (unsigned)channels);
        g_audio_info_rate = rate;
        g_audio_info_ch = channels;
        uint8_t info[8] = {0};
        memcpy(info, &rate, 4);
        info[4] = channels;
        info[5] = 1;                 // 1 = float32, verschachtelt
        send_small(QC_MSG_AUDIO_INFO, info, sizeof info);
    }
    size_t bytes = frames * channels * sizeof(float);
    if (send_small_bis(QC_MSG_AUDIO, pcm, bytes, 1)) {
        atomic_fetch_add(&g_audio_verworfen, 1);
        logf_gedrosselt(&d_ton_verworfen, NULL, @"Ton verworfen: Rueckstand seit %llu s ueber der Staugrenze - Leitung langsamer als der Ton",
                        QC_STAU_FRIST_US / 1000000ull);
        return;
    }
    atomic_fetch_add(&g_audio_packets, 1);
    atomic_fetch_add(&g_audio_bytes, (long long)bytes);
}

static void clip_cb(const char *utf8, size_t len) {
    // Neuer Inhalt in der Ablage: eine laufende Dateisendung ist ueberholt.
    qc_senden_abbrechen();
    send_small(QC_MSG_CLIP, utf8, len);
}

// ----------------------------------------------------------------- Dateien
// Dateien ueber die Zwischenablage (dateien.m). Hier nur die Bindung an
// Sitzung und Leitung.

// Hat der aktuelle Eingabekanal dieser Sitzung FAEHIG_DATEIEN gemeldet?
// Nur unter g_send_mtx.
static int dateien_faehig_gesperrt(void) {
    return (g_faehig_bits & QC_FAEHIG_DATEIEN) && g_faehig_sitzung == g_sitzung &&
           g_faehig_kanal == g_in_kanal && atomic_load(&g_in_fd) >= 0;
}

static int dateien_spielmodus(void) { return atomic_load(&g_cur_gaming); }

// Die Zeile ist UTF-8 (Pfade mit Umlauten). Nicht ueber "%s": das liest
// NSString in der Standardkodierung (hier MacRoman), aus jedem Umlaut
// wuerden zwei falsche Zeichen.
static void dateien_log(const char *zeile) {
    NSString *s = [NSString stringWithUTF8String:zeile];
    logf_(@"%@", s ?: @"Dateien: (Protokollzeile nicht in UTF-8)");
}

static void dateien_einrichten(void) {
    static const qc_dateien_wege wege = { send_small_sitzung, rueckstand_sitzung, dateien_spielmodus, dateien_log };
    qc_dateien_einrichten(&wege);
}

// Fuer welche Sitzung "aelterer Client" schon im Protokoll steht (einmal je
// Sitzung). Nur auf der Warteschlange der Ablage.
static uint64_t g_dateien_alt_gemeldet = UINT64_MAX;

// Der Nutzer hat am Mac Dateien kopiert (Warteschlange der Ablage, nur mit
// Zuschauer gelesen). Gesendet wird nur an einen Zuschauer, dessen aktueller
// Eingabekanal Dateien kann - ein aelterer Client wuerde sonst Nachrichten
// bekommen, die er nicht kennt, und ohne Quittungen liefe nichts.
static void clip_dateien_cb(NSArray<NSString *> *pfade) {
    char ip[INET_ADDRSTRLEN];
    pthread_mutex_lock(&g_send_mtx);
    uint64_t sitzung = g_sitzung;
    int da = atomic_load(&g_client_fd) >= 0 && atomic_load(&g_vid_ready);
    int kanal = atomic_load(&g_in_fd) >= 0;
    int faehig = da && dateien_faehig_gesperrt();
    memcpy(ip, g_vid_ip, sizeof ip);
    pthread_mutex_unlock(&g_send_mtx);
    if (!faehig) {
        // Neuer Inhalt: eine laufende Sendung ist auch dann ueberholt.
        qc_senden_abbrechen();
        if (da && !kanal) {
            // Ohne Eingabekanal weiss niemand, was der Client kann (und es
            // kaemen keine Quittungen). Das ist kein aelterer Client - der
            // Hinweis dafuer bleibt fuer diese Sitzung unverbraucht.
            logf_(@"Dateien: nicht gesendet (Zuschauer ohne Eingabekanal)");
        } else if (da && g_dateien_alt_gemeldet != sitzung) {
            g_dateien_alt_gemeldet = sitzung;
            logf_(@"Dateien: Zuschauer kann keine Dateien empfangen (aelterer Client)");
        }
        return;
    }
    qc_senden_starten(sitzung, pfade, @(ip));
}

// Eine Nachricht 50-53 oder 69 vom Eingabekanal `kanal` der Sitzung
// `sitzung`, schon ganz gelesen. Hier geschieht keine Plattenarbeit: der
// Empfaenger reiht nur ein, die Quittung setzt nur Zahlen.
static void dateien_nachricht(uint64_t sitzung, uint64_t kanal, uint8_t typ, NSData *nutzlast) {
    switch (typ) {
        case QC_IN_FAEHIGKEITEN: {
            uint32_t bits = 0;
            if (qc_datei_faehigkeiten_lesen(nutzlast.bytes, nutzlast.length, &bits) != 0) break;
            pthread_mutex_lock(&g_send_mtx);
            int gilt = sitzung == g_sitzung && kanal == g_in_kanal;
            if (gilt) {
                g_faehig_sitzung = sitzung;
                g_faehig_kanal = kanal;
                g_faehig_bits = bits;
            }
            pthread_mutex_unlock(&g_send_mtx);
            if (gilt) logf_(@"Eingabekanal: Client meldet Faehigkeiten %08x%s", bits,
                            (bits & QC_FAEHIG_DATEIEN) ? " (Dateien)" : "");
            break;
        }
        case QC_DATEI_QUITTUNG:
            qc_senden_quittung(sitzung, nutzlast.bytes, nutzlast.length);
            break;
        default:
            qc_empfang_nachricht(sitzung, kanal, typ, nutzlast);
            break;
    }
}

// Zeigerform: Kopf und Bild in einem Stueck, damit send_small sie unter einem
// Griff schickt - sie darf nie zwischen zwei Bildhaelften landen.
static void zeiger_cb(uint16_t w, uint16_t h, uint16_t hx, uint16_t hy, int sichtbar, const uint8_t *rgba) {
    size_t n = (size_t)w * h * 4;
    uint8_t *p = malloc(12 + n);
    if (!p) return;
    memset(p, 0, 12);
    memcpy(p, &w, 2); memcpy(p + 2, &h, 2); memcpy(p + 4, &hx, 2); memcpy(p + 6, &hy, 2);
    p[8] = (uint8_t)(sichtbar ? 1 : 0);
    p[9] = 1;
    memcpy(p + 12, rgba, n);
    send_small(QC_MSG_CURSOR, p, 12 + n);
    free(p);
}

static int zeiger_aktiv(void) {
    return atomic_load(&g_client_fd) >= 0;
}

static void zeiger_log(const char *text) {
    logf_(@"%s", text);
}

// Die HDR-Entscheidung dieses Hosts (qc_hdr_entscheiden) fuer den
// Kandidaten idx und das IN_ANZEIGE des Zuschauers (NULL: noch keins -
// Grund 7). Die Quelle ist HDR, wenn der aufgenommene Bildschirm es kann
// (g_quelle_hdr); der Host kann es, wenn der Kandidat es kann (g_befund,
// darin macOS 15 und Apple Silicon). Scheiterte der letzte Wechsel nach HDR,
// bleibt es bei SDR mit Grund 6.
static int hdr_grund_fuer_idx(int idx, const qc_hdr_anzeige *a) {
    int host_kann = idx >= 0 && idx < (int)QC_KANDIDATEN && g_befund[idx].hdr;
    int g = qc_hdr_entscheiden(atomic_load(&g_quelle_hdr), host_kann, idx, a);
    if (g == QC_HDR_GRUND_AKTIV && atomic_load(&g_hdr_gescheitert)) g = QC_HDR_GRUND_WECHSEL_GESCHEITERT;
    return g;
}

static int hdr_grund_fuer(const qc_hdr_anzeige *a) {
    return hdr_grund_fuer_idx(atomic_load(&g_codec_id), a);
}

// Eckdaten des Stroms, immer aus dem AKTUELLEN Codec abgeleitet: die alten
// acht Byte, dahinter Fassung 1 (hdr.h) mit der Farbe, die gerade auf der
// Leitung ist. In HDR10 mit den Metadaten (SDR-Weiss, Mastering) und Grund 0;
// in SDR mit dem Grund aus der Entscheidung fuer dieses IN_ANZEIGE (NULL: die
// Begruessung - der Neue hat noch nichts gemeldet). Ergaebe die Entscheidung
// HDR, waehrend noch SDR laeuft, steht der Wechsel aus oder scheiterte: Grund 6.
static void strominfo_fuellen(uint8_t p[QC_HDR_INFO_LAENGE], const qc_hdr_anzeige *a) {
    int idx = atomic_load(&g_codec_id);
    uint16_t w16 = (uint16_t)g_info_w, h16 = (uint16_t)g_info_h, f16 = (uint16_t)g_info_fps;
    memcpy(p + 0, &w16, 2); memcpy(p + 2, &h16, 2); memcpy(p + 4, &f16, 2);
    p[6] = ist_h264(idx) ? 2 : 1;                 // 1 = HEVC, 2 = H.264
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    // 1 = 4:4:4 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit, 4 = 4:2:0 10 Bit; alles Vollbereich
    p[7] = k->chroma444 ? (k->zehn_bit ? 2 : 1) : (k->zehn_bit ? 4 : 3);
    qc_hdr_info farbe;
    if (atomic_load(&g_farbe_pq)) {
        hdr_info_pq(&farbe);
    } else {
        int g = hdr_grund_fuer(a);
        qc_hdr_info_sdr(&farbe, (uint8_t)(g == QC_HDR_GRUND_AKTIV ? QC_HDR_GRUND_WECHSEL_GESCHEITERT : g));
    }
    qc_hdr_info_kodieren(&farbe, p + QC_HDR_INFO_ALT);
}

// Nach einem IN_ANZEIGE neu entscheiden (steht bei strominfo_senden).
static void hdr_neu_entscheiden(const char *anlass);

// Das IN_ANZEIGE der laufenden Sitzung nach *a; 0, wenn sie keins hat.
// Nimmt g_send_mtx.
static int anzeige_jetzt(qc_hdr_anzeige *a) {
    pthread_mutex_lock(&g_send_mtx);
    int da = g_anzeige_da && g_anzeige_sitzung == g_sitzung;
    if (da) *a = g_anzeige;
    pthread_mutex_unlock(&g_send_mtx);
    return da;
}

// Koennensliste: was dieser Mac wirklich codiert, mit Hardware- und
// Umrechnungsflagge. Der Client zeigt nur an, was hier drinsteht.
static void codecs_senden(void) {
    uint8_t buf[512];
    size_t n = 0;
    buf[n++] = (uint8_t)QC_KANDIDATEN;
    for (size_t i = 0; i < QC_KANDIDATEN && n + 8 + 32 < sizeof buf; i++) {
        const qc_codec_kandidat *k = &g_kandidaten[i];
        size_t nl = strlen(k->name);
        if (nl > 31) nl = 31;
        buf[n++] = (uint8_t)i;
        buf[n++] = (uint8_t)g_befund[i].vorhanden;
        buf[n++] = (uint8_t)g_befund[i].hardware;
        buf[n++] = (uint8_t)umrechnung_fuer((int)i);
        buf[n++] = (uint8_t)k->chroma444;
        buf[n++] = (uint8_t)k->zehn_bit;
        buf[n++] = (uint8_t)nl;
        memcpy(buf + n, k->name, nl);
        n += nl;
    }
    send_small(QC_MSG_CODECS, buf, n);
}

// Annahme: Handschlaege laufen je Verbindung in einem eigenen Faden (siehe
// qc_annahme.h). Bis zu 32 gleichzeitig je Port, 4 je Adresse; ist alles
// belegt, weicht der aelteste Handschlag dem neuen. Faeden sind billig, und
// ein Zuschauer braucht je Port nur einen Platz fuer ein paar Millisekunden.
#define QC_HANDSCHLAEGE        32
#define QC_HANDSCHLAEGE_JE_IP  4
// Warteschlange im Kernel: die Annahme ist schnell, aber bei Andrang soll eine
// neue Verbindung nicht schon dort verworfen werden.
#define QC_LISTEN_WARTESCHLANGE 32

static void annahme_andrang(long verdraengt, const struct sockaddr_in *verdraengt_von,
                            long abgewiesen, const struct sockaddr_in *abgewiesen_von, void *ctx) {
    char ip[INET_ADDRSTRLEN] = {0};
    if (verdraengt) {
        inet_ntop(AF_INET, &verdraengt_von->sin_addr, ip, sizeof ip);
        logf_(@"%s: Andrang - %ld laufende(r) Handschlag/Handschlaege fuer neuere Verbindungen abgebrochen, zuletzt der von %s",
              (const char *)ctx, verdraengt, ip);
    }
    if (abgewiesen) {
        inet_ntop(AF_INET, &abgewiesen_von->sin_addr, ip, sizeof ip);
        logf_(@"%s: Andrang - %ld Verbindung(en) sofort geschlossen (zu viele Faeden), zuletzt von %s",
              (const char *)ctx, abgewiesen, ip);
    }
}

// So lange darf die Abloese-Nachricht hoechstens auf Platz im Sendepuffer
// warten. Sie braucht nur 28 Byte; bei einem lebenden Zuschauer macht jede
// Quittung Platz (LAN rund 1 ms, WLAN einige 10 ms). Wer in 100 ms nichts
// abnimmt, ist eingefroren - dann wird trotzdem geschlossen, und g_send_mtx
// haengt nicht fest.
#define QC_ABLOESUNG_MS 100

// Den bisherigen Zuschauer abloesen, weil ein neuer, gekoppelter kommt. Er
// bekommt als letzte Nachricht auf seinem Bildkanal Typ 10 - sonst verbaende
// sich sein Client nach zwei Sekunden von selbst neu und loeste seinerseits
// den neuen ab, endlos hin und her. Danach sind Bild- und Eingabekanal zu.
// Nur unter g_send_mtx rufen; neu_fd ist die Verbindung des Neuen.
// Rueckgabe: -1 = es gab keinen, 1 = Nachricht abgeschickt, 0 = kam nicht an.
static int zuschauer_abloesen(int neu_fd, char fp_alt[24]) {
    int alt = atomic_exchange(&g_client_fd, -1);
    atomic_store(&g_vid_ready, 0);
    int gemeldet = -1;
    if (alt >= 0 && alt != neu_fd) {
        if (g_vid) {
            qc_fingerprint(g_vid_peer, fp_alt);
            // Derselbe Weg wie send_small, nur mit kurzer Frist statt der
            // zwei Sekunden aus tune_socket. Die Frist bleibt am Socket, er
            // wird gleich geschlossen.
            struct timeval kurz = { .tv_sec = 0, .tv_usec = QC_ABLOESUNG_MS * 1000 };
            setsockopt(alt, SOL_SOCKET, SO_SNDTIMEO, &kurz, sizeof kurz);
            qc_hdr h = { .type = QC_MSG_ABGELOEST, .flags = 0, .reserved = 0, .len = 0 };
            struct iovec iov = { .iov_base = &h, .iov_len = sizeof h };
            gemeldet = qc_chan_send(g_vid, &iov, 1) == 0;
        }
        // close laesst den Kernel alles noch Gepufferte samt dieser letzten
        // Nachricht zustellen, bevor die Verbindung endet.
        close(alt);
    }
    eingabe_abbrechen();
    qc_chan_free(g_vid);
    g_vid = NULL;
    return gemeldet;
}

// Der Abschied des Hosts (Typ 13 mit Grund, zugang.h) an den Zuschauer auf
// fd, mit derselben kurzen Frist wie die Abloese-Nachricht - danach schliesst
// der Aufrufer. Der Client zeigt den Grund und verbindet sich nicht von
// selbst neu. Nur unter g_send_mtx rufen, solange g_vid zu fd gehoert.
// Rueckgabe: 1 = abgeschickt, 0 = kam nicht an.
static int host_ende_senden(int fd, uint8_t grund) {
    struct timeval kurz = { .tv_sec = 0, .tv_usec = QC_ABLOESUNG_MS * 1000 };
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &kurz, sizeof kurz);
    uint8_t m[QC_HOST_ENDE_LAENGE];
    struct iovec iov = { .iov_base = m, .iov_len = qc_host_ende_kodieren(m, grund) };
    return qc_chan_send(g_vid, &iov, 1) == 0;
}

// ------------------------------------------------------------ Rechnername
// Der Name dieses Macs, wie ihn die Systemeinstellungen zeigen ("Roberts Mac
// mini"), sonst der Hostname - UTF-8-sicher auf 40 Byte. Die Bekanntgabe
// frischt ihn auf. Bekanntgabe, Nachricht 20 und Kopf des Menues tragen den
// Geraetenamen (geraetename): den eingestellten (qc_dienst_name_setzen, die
// eine App: geraetename= in einstellungen.txt), sonst diesen.
static pthread_mutex_t g_rechnername_mtx = PTHREAD_MUTEX_INITIALIZER;
static char g_rechnername[QC_ZUGANG_NAME_MAX + 1] = {0};
static char g_geraetename[QC_ZUGANG_NAME_MAX + 1] = {0};   // leer = der Rechnername

static void rechnername_auffrischen(void) {
    char roh[1024] = {0};
    CFStringRef cn = SCDynamicStoreCopyComputerName(NULL, NULL);
    if (cn) {
        if (!CFStringGetCString(cn, roh, sizeof roh, kCFStringEncodingUTF8)) roh[0] = 0;
        CFRelease(cn);
    }
    if (!roh[0]) gethostname(roh, sizeof roh - 1);
    char n[QC_ZUGANG_NAME_MAX + 1];
    qc_zugang_name_saeubern(roh, strlen(roh), n, QC_ZUGANG_NAME_MAX);
    pthread_mutex_lock(&g_rechnername_mtx);
    memcpy(g_rechnername, n, sizeof n);
    pthread_mutex_unlock(&g_rechnername_mtx);
}

static void rechnername(char out[QC_ZUGANG_NAME_MAX + 1]) {
    pthread_mutex_lock(&g_rechnername_mtx);
    BOOL leer = !g_rechnername[0];
    pthread_mutex_unlock(&g_rechnername_mtx);
    if (leer) rechnername_auffrischen();
    pthread_mutex_lock(&g_rechnername_mtx);
    memcpy(out, g_rechnername, sizeof g_rechnername);
    pthread_mutex_unlock(&g_rechnername_mtx);
}

// Der Name, unter dem andere Geraete diesen Mac sehen.
static void geraetename(char out[QC_ZUGANG_NAME_MAX + 1]) {
    pthread_mutex_lock(&g_rechnername_mtx);
    BOOL eigen = g_geraetename[0] != 0;
    if (eigen) memcpy(out, g_geraetename, sizeof g_geraetename);
    pthread_mutex_unlock(&g_rechnername_mtx);
    if (!eigen) rechnername(out);
}

// dienst.h. Gilt sofort: die Bekanntgabe liest den Namen je Runde,
// Nachricht 20 je Zugangsphase, das Menue beim naechsten Lesen (gleich
// angestossen). Der Client hat den Namen schon geprueft (1-40 Byte UTF-8,
// ohne Steuer- und Richtungszeichen); hier wird nur noch gesaeubert wie jeder
// fremde Name.
void qc_dienst_name_setzen(const char *name) {
    char n[QC_ZUGANG_NAME_MAX + 1] = {0};
    if (name && *name) qc_zugang_name_saeubern(name, strlen(name), n, QC_ZUGANG_NAME_MAX);
    pthread_mutex_lock(&g_rechnername_mtx);
    BOOL anders = strcmp(n, g_geraetename) != 0;
    memcpy(g_geraetename, n, sizeof n);
    pthread_mutex_unlock(&g_rechnername_mtx);
    if (!anders) return;
    char jetzt[QC_ZUGANG_NAME_MAX + 1];
    geraetename(jetzt);
    logf_(@"Geraetename: \"%@\"%@", utf8(jetzt), n[0] ? @"" : @" (Rechnername)");
    qc_ui_zustand_geaendert();
}

// ------------------------------------------------ Zustand fuer die Oberflaeche

int qc_zustand_zuschauer(char *name, size_t groesse) {
    if (atomic_load(&g_client_fd) < 0) {
        if (name && groesse) name[0] = 0;
        return 0;
    }
    pthread_mutex_lock(&g_zustand_mtx);
    if (name && groesse) snprintf(name, groesse, "%s", g_zuschauer_name);
    pthread_mutex_unlock(&g_zustand_mtx);
    return 1;
}

int qc_zustand_geraetename(char *name, size_t groesse) {
    char n[QC_ZUGANG_NAME_MAX + 1];
    geraetename(n);
    if (name && groesse) snprintf(name, groesse, "%s", n);
    return 0;
}

int qc_zustand_rechnername(char *name, size_t groesse) {
    char n[QC_ZUGANG_NAME_MAX + 1];
    rechnername(n);
    if (name && groesse) snprintf(name, groesse, "%s", n);
    return 0;
}

int qc_zustand_bildschirmfreigabe(void) { return g_tcc_bildschirm() ? 1 : 0; }
int qc_zustand_bedienungshilfen(void) { return AXIsProcessTrusted() ? 1 : 0; }
int qc_zustand_port_belegt(void) { return atomic_load(&g_port_belegt); }

// Ein Geraet wurde aus der Liste entfernt (zugang.c, pub NULL = alle): eine
// laufende Sitzung dieses Schluessels endet mit dem Abschied (Typ 13, Grund
// 2 "Geraet entfernt"). Der Client zeigt das und verbindet sich nicht von
// selbst neu; ein aelterer verbindet sich neu und landet in der
// Zugangsphase. Aus einem Faden der Oberflaeche, nie der Main Queue
// (g_send_mtx kann bis zu 2 s haengen).
//
// Wer gerade hereinkommt, ist hier noch nicht zu sehen: bild_verbindung hat
// ihn in der Liste gefunden (oder eben eingetragen), faehrt aber noch die
// Aufnahme hoch. Dafuer zaehlt g_entfernt_zaehler jedes Entfernen, unter
// g_send_mtx; bild_verbindung vergleicht beim Eintragen des Zuschauers mit
// dem Stand vor seinem Blick in die Liste und sieht bei einem Unterschied
// noch einmal nach.
static _Atomic uint64_t g_entfernt_zaehler = 0;

static void zuschauer_entfernt(const uint8_t *pub) {
    pthread_mutex_lock(&g_send_mtx);
    atomic_fetch_add(&g_entfernt_zaehler, 1);
    int fd = atomic_load(&g_client_fd);
    BOOL treffer = fd >= 0 && g_vid && (!pub || memcmp(g_vid_peer, pub, 32) == 0);
    int gemeldet = 0;
    if (treffer) {
        atomic_store(&g_vid_ready, 0);
        atomic_store(&g_client_fd, -1);
        gemeldet = host_ende_senden(fd, QC_HOST_ENDE_ENTFERNT);
        shutdown(fd, SHUT_RDWR);
        close(fd);
        eingabe_abbrechen();
        qc_chan_free(g_vid);
        g_vid = NULL;
        stream_herunterfahren_anstossen();
    }
    pthread_mutex_unlock(&g_send_mtx);
    if (treffer) {
        logf_(@"Zuschauer getrennt: sein Geraet wurde aus der Liste entfernt%s",
              gemeldet ? "" : " - der Abschied kam nicht an");
        qc_ui_zustand_geaendert();
    }
}

// ------------------------------------------------------------ Zugangsphase
// Ein Geraet, das nicht in host-devices.txt steht (oder die Liste ist
// beschaedigt), bekommt nach dem Handschlag "QCA1" statt "QCH1" und die
// Nachrichten 20-23 (zugang.h, Spezifikation 3.2): es beweist das
// Zugangspasswort, oder jemand am Host klickt "Zulassen". Ebenso ein
// bekanntes Geraet mit Bit 0 in Nachricht 3 (1.4): es kennt den Schluessel
// dieses Hosts nicht, und erst der host_proof in 22/0 (oder "Zulassen" mit
// dem Vergleichscode) weist den Host ihm gegenueber aus. Bis dahin
// geschieht nichts, was einen laufenden Zuschauer beruehrt: keine Abloesung,
// keine Aufnahme, kein g_vid_hh - also auch kein Eingabekanal. Gewartet wird
// ohne jede Sperre; wach wird die Phase durch den Client (poll auf den
// Socket, auch sein Verschwinden), durch eine Entscheidung am Host (Pipe)
// oder durch die Frist.

static int64_t mono_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static int zugang_senden(qc_chan *c, const void *p, size_t n) {
    if (!n) return -1;
    struct iovec iov = { .iov_base = (void *)p, .iov_len = n };
    return qc_chan_send(c, &iov, 1);
}

static int zugang_ergebnis(qc_chan *c, uint8_t ergebnis, uint32_t warten_ms, const uint8_t *host_proof) {
    uint8_t m[QC_ZUGANG_ERGEBNIS_MAX];
    size_t n = qc_zugang_ergebnis_kodieren(m, sizeof m, ergebnis, warten_ms, host_proof);
    int r = zugang_senden(c, m, n);
    qc_wipe(m, sizeof m);
    return r;
}

// Eintragen nach Passwort oder Klick. Laesst sich die Liste nicht schreiben
// (beschaedigt, Platte), kommt das Geraet fuer diese Sitzung trotzdem herein -
// es hat sich ausgewiesen; beim naechsten Mal fragt der Host eben wieder. Ein
// Geraet, das schon in der Liste stand (Bit 0), bleibt, wie es eingetragen ist.
static void zugang_eintragen(qc_chan *chan, const char *name, const char *id_text, const char *ip, const char *weg,
                             int bekannt) {
    if (qc_zugang_eintragen(chan->peer, name) == 0)
        logf_(@"Zugang: %@ (ID %s, %s) %s und %s", utf8(name), id_text, ip, weg,
              bekannt ? "war schon eingetragen (Host auf Wunsch des Clients ausgewiesen)" : "eingetragen");
    else
        logf_(@"Zugang: %@ (ID %s, %s) %s - Eintrag in host-devices.txt liess sich nicht speichern, "
               "diese Sitzung laeuft trotzdem", utf8(name), id_text, ip, weg);
}

// Liest n Byte bis zum Zeitpunkt bis (monotone Uhr). 0 = gelesen, 1 = die
// Frist lief ab, -1 = die Verbindung ist zu oder kaputt.
static int zugang_lesen(qc_chan *c, void *p, size_t n, int64_t bis) {
    int64_t rest = bis - mono_ms();             // bis liegt hoechstens QC_ZUGANG_NACHRICHT_MS voraus
    if (qc_chan_read_frist(c, p, n, rest < 1 ? 1 : (int)rest) == 0) return 0;
    return mono_ms() >= bis ? 1 : -1;
}

// YES = zugelassen: 22/0 oder 22/1 ist hinaus, der Aufrufer macht weiter wie
// bei einem bekannten Geraet ("QCH1"). NO = nicht: alles Noetige ist gesagt,
// der Aufrufer schliesst. bekannt: das Geraet steht schon in der Liste und ist
// nur hier, weil es mit Bit 0 in Nachricht 3 den Ausweis des Hosts verlangt.
static BOOL zugang_phase(qc_chan *chan, const struct sockaddr_in *peer, const char *ip, const char *name_c, int bekannt) {
    uint32_t cid = qc_zugang_id(chan->peer);
    char id_text[12];
    qc_zugang_id_text(cid, id_text);
    uint32_t adr = peer->sin_addr.s_addr;
    NSString *name = utf8(name_c);

    // Grenzen: 4 Phasen gleichzeitig, 2 je Adresse, 1 je Schluessel.
    int platz = qc_zugang_phase_beginnen(adr, chan->peer);
    if (platz < 0) {
        uint8_t m[4 + QC_ZUGANG_ERGEBNIS_MAX];
        memcpy(m, QC_ZUGANG_KENNUNG, 4);
        size_t n = qc_zugang_ergebnis_kodieren(m + 4, sizeof m - 4, QC_ERGEBNIS_SCHLUSS, QC_ZUGANG_VOLL_WARTEN_MS, NULL);
        zugang_senden(chan, m, 4 + n);
        logf_gedrosselt(&d_zugang_voll, ip, @"Zugang: kein Platz fuer %@ (ID %s, %s) - zu viele Zugangsphasen, geschlossen",
                        name, id_text, ip);
        return NO;
    }

    int64_t jetzt = mono_ms();
    uint32_t warten = qc_zugang_drossel_warten(adr, chan->peer, jetzt);
    int64_t frueh_bis = jetzt + warten;          // ein Beweis davor zaehlt als Fehlversuch
    int64_t frist = jetzt + g_zugang_frist_ms;
    uint8_t wege = QC_ZUGANG_WEG_PASSWORT | (qc_ui_vorhanden() ? QC_ZUGANG_WEG_ZULASSEN : 0);
    char host[QC_ZUGANG_NAME_MAX + 1];
    geraetename(host);
    uint8_t m[4 + QC_ZUGANG_NOETIG_MAX];
    memcpy(m, QC_ZUGANG_KENNUNG, 4);
    size_t n = 4 + qc_zugang_noetig_kodieren(m + 4, sizeof m - 4, wege, warten, host);
    if (zugang_senden(chan, m, n) != 0) {
        qc_zugang_phase_ende(platz);
        return NO;
    }
    logf_gedrosselt(&d_zugang, ip, @"Zugang noetig: %@ (ID %s) von %s%s%s%@", name, id_text, ip,
                    bekannt ? " - bekannt, kennt diesen Host aber nicht (Bit 0 in Nachricht 3), der Host weist sich aus" : "",
                    wege & QC_ZUGANG_WEG_ZULASSEN ? " - Passwort oder Zulassen" : " - Passwort",
                    warten ? [[NSString alloc] initWithFormat:@", Drossel %u s", (warten + 999) / 1000] : @"");

    // Die Anfrage an die Oberflaeche; eine Entscheidung weckt die Phase ueber die Pipe.
    int weck[2] = { -1, -1 };
    uint64_t anfrage = 0;
    if ((wege & QC_ZUGANG_WEG_ZULASSEN) && pipe(weck) == 0) {
        for (int i = 0; i < 2; i++) {
            fcntl(weck[i], F_SETFL, O_NONBLOCK);
            fcntl(weck[i], F_SETFD, FD_CLOEXEC);
        }
        anfrage = qc_zugang_anfrage_stellen(name_c, cid, qc_zugang_code(chan->hh), weck[1]);
    }

    BOOL zugelassen = NO;
    int fehl = 0;
    for (;;) {
        int stand = anfrage ? qc_zugang_anfrage_stand(anfrage) : -1;
        if (stand == 1) {
            if (zugang_ergebnis(chan, QC_ERGEBNIS_ZUGELASSEN, 0, NULL) == 0) {
                zugang_eintragen(chan, name_c, id_text, ip, "am Host zugelassen", bekannt);
                zugelassen = YES;
            } else {
                logf_(@"Zugang: %@ (ID %s, %s) am Host zugelassen, aber nicht mehr erreichbar", name, id_text, ip);
            }
            break;
        }
        if (stand == 0) {
            zugang_ergebnis(chan, QC_ERGEBNIS_ABGELEHNT, 0, NULL);
            logf_(@"Zugang: %@ (ID %s, %s) am Host abgelehnt", name, id_text, ip);
            break;
        }
        jetzt = mono_ms();
        if (jetzt >= frist) {
            zugang_ergebnis(chan, QC_ERGEBNIS_SCHLUSS, qc_zugang_drossel_warten(adr, chan->peer, jetzt), NULL);
            logf_gedrosselt(&d_zugang_frist, ip, @"Zugang: Frist fuer %@ (ID %s, %s) abgelaufen - geschlossen", name, id_text, ip);
            break;
        }
        // Warten auf den Client, die Entscheidung oder die Frist. Schon
        // entschluesselte Bytes meldet poll nicht - dann gleich lesen.
        if (qc_chan_gepuffert(chan) == 0) {
            struct pollfd pf[2] = { { .fd = chan->fd, .events = POLLIN, .revents = 0 },
                                    { .fd = weck[0], .events = POLLIN, .revents = 0 } };
            int64_t rest = frist - jetzt;
            int r = poll(pf, weck[0] >= 0 ? 2 : 1, rest > 60000 ? 60000 : (int)rest);
            if (r < 0 && errno != EINTR) break;
            if (r <= 0) continue;
            if (weck[0] >= 0 && pf[1].revents) {
                uint8_t b[16];
                while (read(weck[0], b, sizeof b) > 0) {}
            }
            // POLLHUP/POLLERR ohne POLLIN: weg - das sagt gleich das Lesen.
            if (!(pf[0].revents & (POLLIN | POLLHUP | POLLERR))) continue;
        }
        // Eine Nachricht des Clients: Kopf und Beweis zusammen in hoechstens
        // QC_ZUGANG_NACHRICHT_MS und nie ueber die Gesamtfrist hinaus - wer
        // tropfenweise sendet, haelt die Phase nicht laenger auf. Laeuft die
        // Frist dabei ab, ist der halbe Datensatz verloren; der Client bekommt
        // noch 22/4 (senden geht, siehe qc_chan_read_frist), dann ist Schluss.
        int64_t nachricht_bis = mono_ms() + QC_ZUGANG_NACHRICHT_MS;
        if (nachricht_bis > frist) nachricht_bis = frist;
        qc_hdr h;
        uint8_t beweis[32], host_proof[32];
        int gelesen = zugang_lesen(chan, &h, sizeof h, nachricht_bis);
        if (gelesen == 0 && h.type == QC_ZUGANG_ABBRUCH && h.len == 0) {
            logf_gedrosselt(&d_zugang_abbruch, ip, @"Zugang: %@ (ID %s, %s) hat abgebrochen", name, id_text, ip);
            break;
        }
        if (gelesen == 0 && (h.type != QC_ZUGANG_BEWEIS || h.len != 32)) {
            logf_gedrosselt(&d_zugang_abbruch, ip, @"Zugang: %@ (ID %s, %s) schickt Unerwartetes (Typ %u, %u Byte) - geschlossen",
                            name, id_text, ip, h.type, h.len);
            break;
        }
        if (gelesen == 0) gelesen = zugang_lesen(chan, beweis, sizeof beweis, nachricht_bis);
        if (gelesen > 0) {
            jetzt = mono_ms();
            zugang_ergebnis(chan, QC_ERGEBNIS_SCHLUSS, qc_zugang_drossel_warten(adr, chan->peer, jetzt), NULL);
            logf_gedrosselt(&d_zugang_frist, ip, @"Zugang: Frist fuer %@ (ID %s, %s) abgelaufen%s - geschlossen", name, id_text, ip,
                            jetzt >= frist ? "" : ", Nachricht kam nicht vollstaendig");
            break;
        }
        if (gelesen < 0) {
            logf_gedrosselt(&d_zugang_abbruch, ip, @"Zugang: %@ (ID %s, %s) hat die Verbindung beendet", name, id_text, ip);
            break;
        }
        jetzt = mono_ms();
        // Zu frueh ist ein Beweis vor Ablauf dessen, was diese Verbindung als
        // Wartezeit gesagt bekam - und ebenso, solange die Drossel ihrer
        // Adresse oder ihres Schluessels jetzt noch laeuft (3.3: der groessere
        // Wert gilt). Sonst riete eine zweite Verbindung derselben Adresse mit
        // anderem Schluessel ungebremst weiter, waehrend die erste wartet.
        BOOL zu_frueh = jetzt < frueh_bis || qc_zugang_drossel_rest(adr, chan->peer, jetzt) > 0;
        // Zu frueh wird gar nicht erst gerechnet - so kostet Raten nichts ausser Zeit.
        int ok = zu_frueh ? 0 : qc_zugang_pruefen(chan->hh, beweis, host_proof);
        qc_wipe(beweis, sizeof beweis);
        if (ok == 1) {
            qc_zugang_drossel_erfolg(adr, chan->peer);
            // Das Fenster am Host schliesst, bevor die Sitzung beginnt.
            if (anfrage) { qc_zugang_anfrage_zurueckziehen(anfrage); anfrage = 0; }
            if (zugang_ergebnis(chan, QC_ERGEBNIS_PASSWORT, 0, host_proof) == 0) {
                zugang_eintragen(chan, name_c, id_text, ip, "mit Passwort angenommen", bekannt);
                zugelassen = YES;
            }
            qc_wipe(host_proof, sizeof host_proof);
            break;
        }
        fehl++;
        BOOL schluss = fehl >= QC_ZUGANG_VERSUCHE;
        if (ok < 0) {
            // Kein lesbares Passwort (4.3: dann nur Zulassen): der Fehler liegt
            // beim Host, nicht beim Client. Also keine Drossel - wer das
            // richtige Passwort kennt, soll nach dem Reparieren nicht erst
            // Minuten warten. Die Versuche dieser Verbindung zaehlen trotzdem,
            // damit niemand die Phase in einer Schleife beschaeftigt.
            warten = 0;
            logf_gedrosselt(&d_zugang_falsch, ip, @"Zugang: Beweis von %@ (ID %s, %s) nicht pruefbar - kein lesbares Passwort, "
                            "nur Zulassen moeglich; keine Drossel, Versuch %d in dieser Verbindung%s",
                            name, id_text, ip, fehl, schluss ? ", zu viele Versuche - geschlossen" : "");
        } else {
            warten = qc_zugang_drossel_fehler(adr, chan->peer, jetzt);
            frueh_bis = jetzt + warten;
            logf_gedrosselt(&d_zugang_falsch, ip, @"Zugang: %s fuer %@ (ID %s, %s), Fehlversuch %d in dieser Verbindung - Drossel %u s%s",
                            zu_frueh ? "Beweis vor Ablauf der Wartezeit" : "Passwort falsch",
                            name, id_text, ip, fehl, (warten + 999) / 1000, schluss ? ", zu viele Versuche - geschlossen" : "");
        }
        if (zugang_ergebnis(chan, schluss ? QC_ERGEBNIS_SCHLUSS : QC_ERGEBNIS_FALSCH, warten, NULL) != 0 || schluss) break;
    }
    // Zurueckgezogen (Abbruch, EOF, Frist ...): das Fenster am Host schliesst.
    if (anfrage) qc_zugang_anfrage_zurueckziehen(anfrage);
    if (weck[0] >= 0) { close(weck[0]); close(weck[1]); }
    qc_zugang_phase_ende(platz);
    return zugelassen;
}

// Ein Name aus Nachricht 3 mit neun oder mehr Ziffern koennte eine
// Geraete-ID vortaeuschen ("Roberts Mac (ID 123 456 789)"): im
// Zulassen-Fenster stuende die falsche ID dann vor der echten - und die ID
// ist das, woran man am Mac das Geraet erkennt. Er gilt wie ein fehlender
// (die Windows-Host-Rolle ebenso, einlass::anzeigename). Gezaehlt werden
// Dezimalziffern jeder Schrift, auch Vollbreite; Rechnernamen haben so
// viele kaum je.
static BOOL name_taeuscht_id_vor(const char *name) {
    NSString *s = name ? [NSString stringWithUTF8String:name] : nil;
    NSData *d = [s dataUsingEncoding:NSUTF32LittleEndianStringEncoding];
    if (!d) return NO;
    NSCharacterSet *ziffern = [NSCharacterSet decimalDigitCharacterSet];
    const uint8_t *b = d.bytes;
    int n = 0;
    for (NSUInteger i = 0; i + 4 <= d.length; i += 4) {
        UTF32Char c = (UTF32Char)b[i] | (UTF32Char)b[i + 1] << 8 | (UTF32Char)b[i + 2] << 16 | (UTF32Char)b[i + 3] << 24;
        if ([ziffern longCharacterIsMember:c]) n++;
    }
    return n >= 9;
}

static void bild_verbindung(qc_platz *platz, int fd, const struct sockaddr_in *von, void *ctx) {
    struct sockaddr_in peer = *von;
    tune_socket(fd);
    char ip[INET_ADDRSTRLEN] = {0};
    inet_ntop(AF_INET, &peer.sin_addr, ip, sizeof ip);

    // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
    // Den fd erst nach qc_platz_frei schliessen (siehe qc_annahme.h).
    qc_chan *chan = malloc(sizeof *chan);
    if (!chan) { qc_platz_frei(platz); close(fd); return; }
    int hr = qc_chan_accept(chan, fd, g_id_priv,
                            (const uint8_t *)QC_PRO_VIDEO, strlen(QC_PRO_VIDEO));
    if (qc_platz_frei(platz)) {
        // Fuer eine neuere Verbindung verdraengt: schon gezaehlt und gemeldet.
        qc_chan_free(chan);
        close(fd);
        return;
    }
    if (hr != 0) {
        logf_gedrosselt(&d_bild_handschlag, ip, @"Handschlag mit %s gescheitert (%d)", ip, hr);
        qc_chan_free(chan);
        close(fd);
        return;
    }

    char fp[24], sas[8], id_text[12];
    qc_fingerprint(chan->peer, fp);
    qc_sas(chan->hh, sas);
    qc_zugang_id_text(qc_zugang_id(chan->peer), id_text);

    // Selbstschutz: der eigene Schluessel dieses Hosts ist dieser Rechner
    // selbst - nie herein, auch nicht aus der Liste. "QCA1" und gleich 22/3
    // wie ein Nein am Host (wie die Windows-Host-Rolle); ein neuer Client
    // bricht schon nach Nachricht 2 ab ("Das ist dieser Computer.").
    if (memcmp(chan->peer, g_id_pub, 32) == 0) {
        uint8_t m[4 + QC_ZUGANG_ERGEBNIS_MAX];
        memcpy(m, QC_ZUGANG_KENNUNG, 4);
        size_t n = qc_zugang_ergebnis_kodieren(m + 4, sizeof m - 4, QC_ERGEBNIS_ABGELEHNT, 0, NULL);
        zugang_senden(chan, m, 4 + n);
        logf_gedrosselt(&d_selbst, ip, @"Abgewiesen: %s meldet sich mit dem eigenen Schluessel dieses Hosts - Verbindung zu sich selbst", ip);
        qc_chan_free(chan);
        close(fd);
        return;
    }

    // Wer ist das? Der Name kommt aus Nachricht 3 (unbeglaubigt, nur
    // Anzeige; einer, der eine ID vortaeuscht, zaehlt nicht), sonst aus der
    // Liste, sonst ist es die Adresse. Bekannte Geraete kommen sofort
    // herein, alle anderen - auch wenn die Liste beschaedigt ist - muessen
    // sich in der Zugangsphase ausweisen. Die Liste ist dafuer nur fuer
    // diesen einen Blick gesperrt. Verlangt ein bekanntes Geraet mit Bit 0
    // in Nachricht 3, dass sich der Host ausweist (es kennt dessen
    // Schluessel nicht), durchlaeuft es die Zugangsphase ebenso.
    char name[QC_ZUGANG_NAME_MAX + 1], gespeichert[QC_ZUGANG_NAME_MAX + 1];
    qc_zugang_name_lesen(chan->nutzlast3, chan->nutzlast3_len, name);
    int ausweis = (qc_zugang_name_flags(chan->nutzlast3, chan->nutzlast3_len) & QC_ZUGANG_N3_HOST_UNBEKANNT) != 0;
    if (name[0] && name_taeuscht_id_vor(name)) name[0] = 0;
    uint64_t entfernt_stand = atomic_load(&g_entfernt_zaehler);   // vor dem Blick in die Liste
    int bekannt = qc_zugang_bekannt(chan->peer, gespeichert);
    if (!name[0]) snprintf(name, sizeof name, "%s", bekannt > 0 && gespeichert[0] ? gespeichert : ip);
    if (bekannt < 0)
        logf_gedrosselt(&d_liste_defekt, ip, @"Geraeteliste host-devices.txt nicht lesbar oder beschaedigt - %s (%s) muss sich ausweisen",
                        fp, ip);
    if ((bekannt <= 0 || ausweis) && !zugang_phase(chan, &peer, ip, name, bekannt > 0)) {
        qc_chan_free(chan);
        close(fd);
        return;
    }

    // Kein Zuschauer, keine Arbeit: Aufnahme und Encoder entstehen erst
    // jetzt. Schlaegt das fehl - Freigabe fehlt, kein Bildschirm (Monitor
    // aus, KVM umgeschaltet), ScreenCaptureKit oder Encoder streiken -,
    // kommt der Zuschauer trotzdem herein (g_ohne_aufnahme): "QCH1" mit
    // Ersatzmassen, dann Hoststatus 1, und die Wiederherstellung versucht es
    // alle 3 s, bis ein Bild da ist - wie die Windows-Host-Rolle. Nach dem
    // Handschlag still zuzumachen hiesse fuer den Client "aeltere Fassung,
    // bitte aktualisieren" (Pairing v1, 9.6) - dauerhaft und falsch.
    // Ab hier zaehlt dieser Zuschauer als unterwegs (g_anmeldend), bis er
    // eingetragen ist oder aufgibt - jeder Weg unten zieht ihn wieder ab.
    atomic_fetch_add(&g_anmeldend, 1);
    BOOL ohne_aufnahme = NO, ohne_freigabe = NO;
    if (stream_hochfahren_sync()) {
        atomic_store(&g_ohne_aufnahme, 0);
    } else {
        ohne_aufnahme = YES;
        ohne_freigabe = !g_tcc_bildschirm();
    }

    // Wurde seit dem Blick in die Liste ein Geraet entfernt, hat
    // zuschauer_entfernt diesen hier nicht gesehen - vielleicht war es
    // genau seins ("Alle entfernen" trifft auch ein gerade zugelassenes).
    // Dann noch einmal nachsehen, ausserhalb der Sperre (Datei). Unter ihr
    // gilt der Stand erst, wenn seitdem nichts mehr entfernt wurde: jedes
    // spaetere Entfernen sieht ihn dann als Zuschauer und trennt ihn selbst.
    pthread_mutex_lock(&g_send_mtx);
    while (atomic_load(&g_entfernt_zaehler) != entfernt_stand) {
        entfernt_stand = atomic_load(&g_entfernt_zaehler);
        pthread_mutex_unlock(&g_send_mtx);
        if (qc_zugang_bekannt(chan->peer, NULL) != 1) {
            atomic_fetch_sub(&g_anmeldend, 1);
            stream_herunterfahren_anstossen();
            logf_(@"Zuschauer %@ (ID %s, %s) abgewiesen: sein Geraet wurde eben aus der Liste entfernt", utf8(name), id_text, ip);
            qc_chan_free(chan);
            close(fd);
            return;
        }
        pthread_mutex_lock(&g_send_mtx);
    }
    // Die Freigabe ging aus, waehrend er hereinkam: er wird nicht mehr
    // Zuschauer, sondern bekommt nach der Kennung "QCH1" den Abschied wie ein
    // laufender (Grund 1) - so zeigt ihn der Client und verbindet sich nicht
    // von selbst neu. qc_dienst_anhalten setzt den Merker, bevor es unter
    // derselben Sperre den laufenden verabschiedet - einer von beiden sieht
    // ihn also.
    if (atomic_load(&g_freigabe_aus)) {
        struct timeval kurz = { .tv_sec = 0, .tv_usec = QC_ABLOESUNG_MS * 1000 };
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &kurz, sizeof kurz);
        uint8_t ende[4 + QC_HOST_ENDE_LAENGE];
        memcpy(ende, QC_MAGIC, 4);
        struct iovec iov_ende = { .iov_base = ende, .iov_len = 4 + qc_host_ende_kodieren(ende + 4, QC_HOST_ENDE_FREIGABE_AUS) };
        int gemeldet = qc_chan_send(chan, &iov_ende, 1) == 0;
        pthread_mutex_unlock(&g_send_mtx);
        atomic_fetch_sub(&g_anmeldend, 1);
        stream_herunterfahren_anstossen();
        logf_(@"Zuschauer %@ (ID %s, %s) abgewiesen: die Freigabe ist aus%s", utf8(name), id_text, ip,
              gemeldet ? "" : " - der Abschied kam nicht an");
        qc_chan_free(chan);
        close(fd);
        return;
    }

    uint8_t hello[4 + sizeof(qc_hdr) + QC_HDR_INFO_LAENGE];
    // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
    // Fenstergroesse und Format kennt, bevor das erste Bild kommt. Erst unter
    // der Sperre gefuellt: ein Codecwechsel, der gerade fertig wird, steht
    // dann entweder schon hier drin, oder sein SWITCH kommt danach beim Neuen an.
    memcpy(hello, QC_MAGIC, 4);
    qc_hdr h = { .type = QC_MSG_INFO, .flags = 0, .reserved = 0, .len = QC_HDR_INFO_LAENGE };
    memcpy(hello + 4, &h, sizeof h);
    // Der Neue hat noch kein IN_ANZEIGE geschickt: Grund 7. Ein gescheiterter
    // Wechsel nach HDR galt dem Vorgaenger - der Neue faengt frisch an. Laeuft
    // der Strom (Abloesung) noch in HDR10, sagt die Begruessung das; der
    // Wechsel nach SDR folgt unten (hdr_neu_entscheiden).
    atomic_store(&g_hdr_gescheitert, 0);
    strominfo_fuellen(hello + 4 + sizeof h, NULL);
    atomic_store(&g_hdr_grund_gesendet, hello[4 + sizeof h + 13]);
    char fp_alt[24] = {0};
    int abgeloest = zuschauer_abloesen(fd, fp_alt);
    g_vid = chan;
    g_stau_seit = 0;                    // ein Stau des Vorgaengers zaehlt nicht fuer ihn
    g_schub_zuletzt = 0;
    g_schub_ohne = 0;
    g_schub_luecke = 0;
    g_schub_frei_seit = 0;
    g_ton_stau_seit = 0;
    memcpy(g_vid_hh, chan->hh, QC_HASHLEN);
    memcpy(g_vid_peer, chan->peer, 32);
    memcpy(g_last_sas, sas, sizeof g_last_sas);
    struct iovec iov = { .iov_base = hello, .iov_len = sizeof hello };
    int sent = qc_chan_send(g_vid, &iov, 1);
    int testbild_aus = 0;
    if (sent == 0) {
        // Der Neue faengt beim naechsten Vollbild an und bekommt den Ton neu
        // angesagt - gesetzt, bevor er als bereit gilt. Sonst erbte er beim
        // Abloesen fuer einen Augenblick den Stand des Vorgaengers: ein
        // Zwischenbild ohne Kopfdaten, Ton ohne Ansage. Laeuft der Strom schon
        // und ist der Bildschirm still, reicht ihm der Takt das zuletzt
        // gesehene Bild als Vollbild nach (fixed_tick, solange g_wait_key).
        atomic_store(&g_force_key, 1);
        atomic_store(&g_wait_key, 1);
        atomic_store(&g_audio_info_sent, 0);
        // Ein Testbild ueberlebt den Zuschauer nicht, auch keine Abloesung:
        // der Neue faengt mit dem Bildschirm an. Den Schalter schon hier, damit
        // der Takt ihm keinen Balken mehr schickt; Aufraeumen auf g_capq unten.
        testbild_aus = atomic_exchange(&g_testbild, 0);
        strlcpy(g_vid_ip, ip, sizeof g_vid_ip);
        pthread_mutex_lock(&g_zustand_mtx);
        memcpy(g_zuschauer_name, name, sizeof g_zuschauer_name);
        pthread_mutex_unlock(&g_zustand_mtx);
        if (ohne_aufnahme) atomic_store(&g_ohne_aufnahme, 1);
        atomic_store(&g_client_fd, fd);
        atomic_store(&g_vid_ready, 1);
    } else {
        qc_chan_free(g_vid);
        g_vid = NULL;
    }
    // Eingetragen oder nicht: ab hier entscheidet g_client_fd. Erst abziehen,
    // dann den Abbau anstossen - sonst saehe er den Neuen noch als unterwegs.
    atomic_fetch_sub(&g_anmeldend, 1);
    // Der alte Zuschauer ist schon getrennt, der neue kam nicht an: niemand
    // schaut zu, also laufen auch Aufnahme und Encoder nicht weiter.
    if (sent != 0) stream_herunterfahren_anstossen();
    pthread_mutex_unlock(&g_send_mtx);
    qc_ui_zustand_geaendert();
    if (abgeloest >= 0)
        logf_(@"Bisheriger Zuschauer %s abgeloest und getrennt%s", fp_alt,
              abgeloest ? "" : " - die Abloese-Nachricht kam nicht an");
    if (sent != 0) {
        logf_(@"Zuschauer %s: Begruessung liess sich nicht senden", ip);
        close(fd);
        return;
    }
    if (testbild_aus) {
        // Nach einem noch eingereihten "Testbild an" des Vorgaengers - die
        // Warteschlange ist seriell, also bleibt es aus.
        dispatch_async(g_capq, ^{ atomic_store(&g_testbild, 0); qc_testbild_stop(); });
        logf_(@"Testbild aus (neuer Zuschauer)");
    }
    // Uebernimmt er einen Strom in HDR10 (Abloesung), entscheidet der Host fuer
    // ihn neu: ohne sein IN_ANZEIGE ist das SDR (Grund 7), mit SWITCH und
    // Strominfo - jede Sitzung beginnt in SDR.
    if (atomic_load(&g_farbe_pq) && g_capq) dispatch_async(g_capq, ^{ hdr_neu_entscheiden("neuer Zuschauer"); });

    qc_zeiger_neu_senden();
    {
        uint8_t cur[9] = {0};
        uint32_t m = (uint32_t)atomic_load(&g_cur_mbit);
        uint16_t f = (uint16_t)atomic_load(&g_cur_fps);
        memcpy(cur, &m, 4); memcpy(cur + 4, &f, 2);
        cur[6] = (uint8_t)atomic_load(&g_cur_gaming);
        cur[7] = (uint8_t)atomic_load(&g_cur_fixed);
        cur[8] = (uint8_t)atomic_load(&g_cur_ton);
        send_small(QC_MSG_SETTINGS, cur, sizeof cur);
    }
    codecs_senden();
    // Was dieser Host kann: Dateien (Fassung 1), Bildschirmwahl und die
    // HDR-Aushandlung (IN_ANZEIGE, Strominfo Fassung 1). Aeltere Clients
    // uebergehen Typ 11 und 12; ein Client ohne Bit 1 schickt nie einen
    // Bildschirmwunsch, einer ohne Bit 2 nie IN_ANZEIGE.
    {
        NSData *f = qc_datei_faehigkeiten_kodieren(QC_FAEHIG_DATEIEN | QC_FAEHIG_BILDSCHIRM | QC_FAEHIG_HDR);
        send_small(QC_MSG_FAEHIGKEITEN, f.bytes, f.length);
    }
    bildschirme_senden();
    // chan gehoert jetzt dem Versand (g_vid) und kann schon wieder frei sein.
    logf_(@"Zuschauer verbunden: %@ (ID %s) %s:%d, verschluesselt, Gegenstelle %s, Vergleichscode %s",
          utf8(name), id_text, ip, ntohs(peer.sin_port), fp, sas);
    if (ohne_aufnahme) {
        // Wie bei einem fehlenden Bildschirm: Hoststatus 1, und die
        // Wiederherstellung fragt alle 3 s nach, bis die Freigabe bzw. der
        // Bildschirm da ist und die Aufnahme laeuft.
        if (ohne_freigabe)
            logf_(@"Bildschirmaufnahme nicht freigegeben - Zuschauer bekommt Hoststatus 1, Aufnahme startet mit der Freigabe");
        else
            logf_(@"Aufnahme laesst sich nicht starten - Zuschauer %s bekommt Hoststatus 1, neuer Versuch alle 3 s", ip);
        hoststatus_senden(1);
        if (g_lifeq)
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), g_lifeq, ^{ aufnahme_wiederherstellen(); });
    }
}

// Die Annahme von Bild- und Eingabeport, zum Anhalten (qc_dienst_anhalten).
// Nur auf dem Weg des Dienstes (dienst_starten, dienst_lauschen_beenden)
// angefasst, nacheinander.
static qc_annahme *g_annahme_bild = NULL, *g_annahme_eingabe = NULL;

// melden: eine Zeile, wenn bind scheitert (der Zustandstakt versucht es
// wiederholt und meldet nur den Wechsel).
static int start_server(int port, BOOL melden) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    a.sin_port = htons((uint16_t)port);
    if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0) {
        if (melden) logf_(@"bind fehlgeschlagen: %s", strerror(errno));
        close(fd);
        return -1;
    }
    if (listen(fd, QC_LISTEN_WARTESCHLANGE) != 0) { logf_(@"listen fehlgeschlagen: %s", strerror(errno)); close(fd); return -1; }
    qc_annahme_cfg cfg = { QC_HANDSCHLAEGE, QC_HANDSCHLAEGE_JE_IP, bild_verbindung, annahme_andrang, (void *)"Bildkanal" };
    qc_annahme *an = qc_annahme_neu(fd, &cfg);
    if (!an) { logf_(@"Annahme fuer Port %d nicht startbar", port); close(fd); return -1; }
    g_annahme_bild = an;
    return fd;
}

// ------------------------------------------------------------ Bekanntgabe
// Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, damit Clients ihn
// ohne eingetippte Adresse finden. Winziges Paket, kein Dienst, keine Abhaengigkeit.
//
// Aufbau: "QCHB" | u8 Version | u16 Bildport | u8 Namenslaenge | Name (UTF-8)
//          | u8 ext = 1 | u32 Geraete-ID | u8 Flags (Bit 0: Zulassen moeglich)
// Aeltere Clients lesen nur bis zum Namen (zugang.h, Spezifikation 2).

// Jede Bekanntgabe gehoert zu einer Runde der Freigabe (g_bekanntgabe_gen):
// qc_dienst_anhalten zaehlt weiter, und der Faden der alten Runde endet vor
// seinem naechsten Paket - hoechstens zwei Sekunden spaeter, ohne noch etwas
// zu senden.
static _Atomic unsigned g_bekanntgabe_gen = 0;

typedef struct { int port; unsigned gen; } qc_bekanntgabe_arg;

static void *beacon_thread(void *arg) {
    qc_bekanntgabe_arg ba = *(qc_bekanntgabe_arg *)arg;
    free(arg);
    int port = ba.port;
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) return NULL;
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &on, sizeof on);
    uint32_t id = qc_zugang_eigene_id();

    // An die Rundrufadresse JEDER aktiven Netzwerkkarte senden. Die allgemeine
    // 255.255.255.255 verlaesst den Mac nicht zuverlaessig, die Netzadresse
    // (zum Beispiel 192.168.178.255) dagegen schon. Das Paket entsteht jede
    // Runde neu: ob jemand "Zulassen" klicken kann, aendert sich mit der
    // Oberflaeche, und den Rechnernamen kann man in den Systemeinstellungen
    // umbenennen (alle 30 s nachgelesen).
    for (unsigned runde = 0; atomic_load(&g_bekanntgabe_gen) == ba.gen; runde++) {
        if (runde % 15 == 0) rechnername_auffrischen();
        char name[QC_ZUGANG_NAME_MAX + 1];
        geraetename(name);
        uint8_t pkt[QC_BEKANNTGABE_MAX];
        size_t len = qc_zugang_bekanntgabe(pkt, sizeof pkt, (uint16_t)port, name, id,
                                           qc_ui_vorhanden() ? QC_BEKANNTGABE_ZULASSEN : 0);
        if (atomic_load(&g_bekanntgabe_gen) != ba.gen) break;
        struct ifaddrs *list = NULL;
        int gesendet = 0;
        if (getifaddrs(&list) == 0) {
            for (struct ifaddrs *ia = list; ia; ia = ia->ifa_next) {
                if (!ia->ifa_addr || ia->ifa_addr->sa_family != AF_INET) continue;
                if (!(ia->ifa_flags & IFF_UP) || !(ia->ifa_flags & IFF_BROADCAST)) continue;
                if (ia->ifa_flags & IFF_LOOPBACK) continue;
                struct sockaddr_in *b = (struct sockaddr_in *)ia->ifa_dstaddr;
                if (!b) continue;
                struct sockaddr_in to = *b;
                to.sin_family = AF_INET;
                to.sin_port = htons((uint16_t)(port + 2));
                if (sendto(fd, pkt, len, 0, (struct sockaddr *)&to, sizeof to) > 0) gesendet++;
            }
            freeifaddrs(list);
        }
        if (runde == 0) {
            char id_text[12];
            qc_zugang_id_text(id, id_text);
            logf_(@"Bekanntgabe: an %d Netze, Port %d, als \"%@\" (ID %s)", gesendet, port + 2, utf8(name), id_text);
        }
        usleep(2000 * 1000);
    }
    close(fd);
    logf_(@"Bekanntgabe: beendet (Freigabe aus)");
    return NULL;
}

static void start_beacon(int port) {
    qc_bekanntgabe_arg *a = malloc(sizeof *a);
    if (!a) return;
    a->port = port;
    a->gen = atomic_load(&g_bekanntgabe_gen);
    pthread_t t;
    if (pthread_create(&t, NULL, beacon_thread, a) != 0) { free(a); return; }
    pthread_detach(t);
}

// ------------------------------------------------------------ Eingabe-Teil
// Zweite Verbindung, nur fuer Maus und Tastatur. Getrennt vom Bild, damit eine
// Mausbewegung nie hinter einem Vollbild in der Warteschlange haengt.
//
// Nachrichten vom Client (gleicher 8-Byte-Kopf):
//   16 MAUS_BEWEGUNG : f32 x, f32 y            (0..1, Anteil der Bildbreite/-hoehe)
//   17 MAUS_TASTE    : u8 taste, u8 gedrueckt, u16 frei, f32 x, f32 y
//   18 RAD           : f32 dx, f32 dy          (Pixel)
//   19 TASTE         : u16 keycode, u8 gedrueckt, u8 Merkmale, u32 Umschalter
//                      Merkmale Bit 0: Wiederholung (QC_TASTE_WIEDERHOLUNG)
//   50-53, 69        : Dateien und Faehigkeiten (dateien.h), eigene Obergrenzen
//   70 BILDSCHIRM    : Bildschirmwunsch (bildschirm.h), hoechstens 65 Byte
#define QC_IN_MOVE    16
#define QC_IN_BUTTON  17
#define QC_IN_SCROLL  18
#define QC_IN_KEY     19

// Merkmal im vierten Byte der Tastennachricht: der Client haelt die Taste
// und sein System wiederholt sie. Der Mac wiederholt eingespeiste Tasten
// nicht selbst, also kommt die Wiederholung von dort - mit Verzoegerung und
// Rate des Clients. Aeltere Clients schicken hier 0 und keine Wiederholungen,
// aeltere Hosts uebergehen das Byte und sehen einen weiteren Druck.
#define QC_TASTE_WIEDERHOLUNG 1

// Einspeisen und das Mitschreiben dessen, was gedrueckt ist, laufen unter
// g_inject_mtx. Beim Abloesen eines Eingabekanals arbeiten kurz zwei Faeden
// (der alte beendet sein letztes Ereignis und gibt frei, der neue speist
// schon ein) - die Sperre reiht sie hintereinander. Sie wird nie zusammen mit
// g_send_mtx gehalten.
static pthread_mutex_t g_inject_mtx = PTHREAD_MUTEX_INITIALIZER;
static CGEventSourceRef g_evsrc = NULL;
static CGEventFlags g_mods = 0;
static uint64_t g_mods_kanal = 0;         // welcher Eingabekanal g_mods zuletzt setzte
static CGPoint g_pos = {0, 0};
static int g_buttons = 0;                 // Bitmaske der gedrueckten Maustasten
static uint64_t g_button_kanal[3] = {0};  // wer sie gedrueckt hat: links, rechts, Mitte
// Der Bildschirm, auf dem die Maus laeuft: immer der gestreamte. Geschrieben
// auf der Lebenslauf-Warteschlange bei jedem Aufnahmestart und Wechsel,
// gelesen im Eingabefaden - deshalb atomar.
static _Atomic CGDirectDisplayID g_input_display = 0;
static _Atomic long g_input_events = 0;

static CGPoint to_display_point(float nx, float ny) {
    CGRect b = CGDisplayBounds(atomic_load(&g_input_display));
    if (nx < 0) nx = 0; if (nx > 1) nx = 1;
    if (ny < 0) ny = 0; if (ny > 1) ny = 1;
    return CGPointMake(b.origin.x + nx * b.size.width, b.origin.y + ny * b.size.height);
}

static void post(CGEventType type, CGMouseButton btn) {
    CGEventRef e = CGEventCreateMouseEvent(g_evsrc, type, g_pos, btn);
    if (!e) return;
    if (g_mods) CGEventSetFlags(e, g_mods);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

static void inject_move(float nx, float ny) {
    g_pos = to_display_point(nx, ny);
    CGEventType t = kCGEventMouseMoved;
    CGMouseButton b = kCGMouseButtonLeft;
    if (g_buttons & 1) { t = kCGEventLeftMouseDragged; b = kCGMouseButtonLeft; }
    else if (g_buttons & 2) { t = kCGEventRightMouseDragged; b = kCGMouseButtonRight; }
    else if (g_buttons & 4) { t = kCGEventOtherMouseDragged; b = kCGMouseButtonCenter; }
    post(t, b);
}

static void inject_button(uint64_t kanal, int button, int down, float nx, float ny) {
    g_pos = to_display_point(nx, ny);
    CGEventType t;
    CGMouseButton b;
    switch (button) {
        case 1: b = kCGMouseButtonRight;  t = down ? kCGEventRightMouseDown : kCGEventRightMouseUp; break;
        case 2: b = kCGMouseButtonCenter; t = down ? kCGEventOtherMouseDown : kCGEventOtherMouseUp; break;
        default: b = kCGMouseButtonLeft;  t = down ? kCGEventLeftMouseDown  : kCGEventLeftMouseUp;  break;
    }
    int bit = button == 1 ? 1 : button == 2 ? 2 : 0;
    if (down) { g_buttons |= (1 << bit); g_button_kanal[bit] = kanal; }
    else      { g_buttons &= ~(1 << bit); g_button_kanal[bit] = 0; }

    CGEventRef e = CGEventCreateMouseEvent(g_evsrc, t, g_pos, b);
    if (!e) return;
    // Doppelklicks: macOS erwartet den Zaehler im Ereignis selbst.
    static double last_t = 0; static int clicks = 0; static CGPoint last_p = {0, 0};
    if (down) {
        double now = CFAbsoluteTimeGetCurrent();
        double dist = fabs(now - last_t);
        if (dist < 0.4 && fabs(last_p.x - g_pos.x) < 5 && fabs(last_p.y - g_pos.y) < 5) clicks++;
        else clicks = 1;
        last_t = now; last_p = g_pos;
    }
    CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks < 1 ? 1 : clicks);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

static void inject_scroll(float dx, float dy) {
    CGEventRef e = CGEventCreateScrollWheelEvent2(g_evsrc, kCGScrollEventUnitPixel, 2,
                                                  (int32_t)dy, (int32_t)dx, 0);
    if (!e) return;
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

// Welche Tasten wir selbst gedrueckt haben, und ueber welchen Eingabekanal
// (0 = losgelassen). Reisst die Verbindung ab, geben wir sie frei - sonst
// haelt der Mac sie fuer immer gedrueckt, und ein haengendes Strg macht aus
// jedem weiteren Klick einen Rechtsklick. Nur unter g_inject_mtx.
static uint64_t g_key_down[256] = {0};

// Wohin Tastenereignisse gehen: ins System. Der Pruefstand (hosttest.m)
// lenkt sie auf sich um - er soll pruefen, was hinausginge, und nicht auf
// dem eigenen Mac tippen. Nur unter g_inject_mtx.
static void tasten_posten_system(CGEventRef e) { CGEventPost(kCGHIDEventTap, e); }
static void (*g_tasten_posten)(CGEventRef e) = tasten_posten_system;

// wiederholung: der Client meldet eine Wiederholung seines Systems
// (QC_TASTE_WIEDERHOLUNG). Ist die Taste hier gedrueckt, geht sie als keyDown
// mit kCGKeyboardEventAutorepeat hinaus - so, wie der Mac eine gehaltene
// Taste selbst wiederholt: kein zweiter Druck, kein Loslassen dazwischen.
// Ist sie es nicht (freigegeben, weil ein Eingabekanal ablief, oder das
// Loslassen kam zuerst), ist es ein gewoehnlicher Druck und wird
// mitgeschrieben wie jeder - so bleibt nichts haengen, was niemand freigibt.
static void inject_key(uint64_t kanal, uint16_t keycode, int down, int wiederholung, uint32_t mods) {
    // Umschalter aus der Client-Sicht uebernehmen: so sieht der Mac genau den
    // Zustand, den der Benutzer an seiner Tastatur haelt.
    CGEventFlags f = 0;
    if (mods & 1) f |= kCGEventFlagMaskShift;
    if (mods & 2) f |= kCGEventFlagMaskControl;
    if (mods & 4) f |= kCGEventFlagMaskAlternate;
    if (mods & 8) f |= kCGEventFlagMaskCommand;
    g_mods = f;
    g_mods_kanal = kanal;

    BOOL autorepeat = down && wiederholung && keycode < 256 && g_key_down[keycode] != 0;
    CGEventRef e = CGEventCreateKeyboardEvent(g_evsrc, (CGKeyCode)keycode, down ? true : false);
    if (!e) return;
    CGEventSetFlags(e, f);
    if (autorepeat) CGEventSetIntegerValueField(e, kCGKeyboardEventAutorepeat, 1);
    g_tasten_posten(e);
    CFRelease(e);

    // Mitschreiben, was gerade gedrueckt ist - das ist die Grundlage fuer das
    // Freigeben, wenn die Verbindung wegbricht.
    if (keycode < 256) g_key_down[keycode] = down ? kanal : 0;
    atomic_fetch_add(&g_input_events, 1);
}

// Ein Ereignis aus dem Eingabekanal einspeisen - nur, solange dieser Kanal
// noch der aktuelle ist. NO = abgeloest, nichts eingespeist. Ein Ereignis,
// das beim Abbrechen schon hinter der Pruefung war, geht noch durch; was es
// drueckt, traegt die Nummer dieses Kanals, und sein Faden gibt es beim
// Beenden mit alle_tasten_loslassen wieder frei.
static BOOL einspeisen(int fd, uint64_t kanal, uint8_t typ, const uint8_t *payload, uint32_t len) {
    pthread_mutex_lock(&g_inject_mtx);
    BOOL aktuell = atomic_load(&g_in_fd) == fd;
    float f[2];
    if (aktuell) switch (typ) {
        case QC_IN_MOVE:
            if (len >= 8) { memcpy(f, payload, 8); inject_move(f[0], f[1]); }
            break;
        case QC_IN_BUTTON:
            if (len >= 12) { memcpy(f, payload + 4, 8); inject_button(kanal, payload[0], payload[1], f[0], f[1]); }
            break;
        case QC_IN_SCROLL:
            if (len >= 8) { memcpy(f, payload, 8); inject_scroll(f[0], f[1]); }
            break;
        case QC_IN_KEY:
            if (len >= 8) {
                uint16_t kc; uint32_t mods;
                memcpy(&kc, payload, 2);
                memcpy(&mods, payload + 4, 4);
                inject_key(kanal, kc, payload[2], payload[3] & QC_TASTE_WIEDERHOLUNG, mods);
            }
            break;
        default: break;
    }
    pthread_mutex_unlock(&g_inject_mtx);
    return aktuell;
}


// Einstellungen im laufenden Betrieb. Bitrate und Bildrate gehen ohne
// Unterbrechung; der Gaming-Schalter zieht die Zuegel straffer: haeufigere
// Vollbilder und ein kleinerer erlaubter Stau, damit nichts auflaeuft.
static void apply_settings(int mbit, int fps, int gaming, int fixed, int ton) {
    if (mbit < 2) mbit = 2;
    if (mbit > 500) mbit = 500;
    if (fps < 10) fps = 10;
    if (fps > 240) fps = 240;

    // Encoder-Sitzung und Aufnahmekonfiguration gehoeren der Aufnahmewarteschlange:
    // dort tauscht der Codecwechsel die Sitzung aus, also darf sie nur dort
    // angefasst werden - sonst setzen wir hier Eigenschaften auf einer Sitzung,
    // die gerade freigegeben wird.
    dispatch_async(g_capq ?: dispatch_get_main_queue(), ^{
        if (g_session) {
            set_i32(g_session, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
            set_i32(g_session, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
            set_i32(g_session, kVTCompressionPropertyKey_MaxKeyFrameInterval, gaming ? fps : fps * 2);
        }
        if (g_stream && g_cfg) {
            g_cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
            [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
                if (e) logf_(@"Bildrate konnte nicht geaendert werden: %@", e.localizedDescription);
            }];
        }
    });
    atomic_store(&g_cur_mbit, mbit);
    atomic_store(&g_cur_fps, fps);
    atomic_store(&g_cur_gaming, gaming);
    atomic_store(&g_cur_fixed, fixed);
    atomic_store(&g_fixed_gewollt, fixed);
    atomic_store(&g_cur_ton, ton ? 1 : 0);
    atomic_store(&g_force_key, 1);

    // Der Takt fuer die feste Bildrate haengt an der eingestellten Rate.
    if (g_tick) {
        uint64_t iv = (uint64_t)(NSEC_PER_SEC / (uint64_t)fps);
        dispatch_source_set_timer(g_tick, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv),
                                  iv, iv / 10);
    }

    // Neuntes Byte: Ton. Aeltere Clients lesen nur acht und stoeren sich
    // nicht an einem mehr.
    uint8_t out[9] = {0};
    uint32_t m = (uint32_t)mbit; uint16_t f = (uint16_t)fps;
    memcpy(out, &m, 4); memcpy(out + 4, &f, 2);
    out[6] = (uint8_t)gaming;
    out[7] = (uint8_t)fixed;
    out[8] = (uint8_t)(ton ? 1 : 0);
    send_small(QC_MSG_SETTINGS, out, sizeof out);
    logf_(@"Einstellungen: %d Mbit/s, %d fps, Gaming %@, feste Bildrate %@, Ton %@",
          mbit, fps, gaming ? @"an" : @"aus", fixed ? @"an" : @"aus", ton ? @"an" : @"aus");
}

// Einstellungen des Zuschauers der Reihe nach, aber nicht auf der Main Queue:
// apply_settings endet mit send_small (g_send_mtx; wer das Senden haelt, kann
// bis zu 2 s haengen), und die Main Queue gehoert der Menueleiste - Menue und
// Zulassen-Fenster stuenden so lange. apply_settings fasst nichts an, was den
// Hauptfaden braucht (Encoder und Aufnahme ueber g_capq, der Rest atomar).
static dispatch_queue_t einstellungen_q(void) {
    static dispatch_queue_t q;
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{ q = dispatch_queue_create("tech.quadchroma.einstellungen", DISPATCH_QUEUE_SERIAL); });
    return q;
}

// Alles loslassen, was dieser Eingabekanal noch als gedrueckt hinterlassen
// hat: Tasten und Maustasten. Was ein neuerer Kanal inzwischen drueckt,
// bleibt gedrueckt.
static void alle_tasten_loslassen(uint64_t kanal) {
    static const struct { CGEventType typ; CGMouseButton taste; } maus_los[3] = {
        { kCGEventLeftMouseUp,  kCGMouseButtonLeft },
        { kCGEventRightMouseUp, kCGMouseButtonRight },
        { kCGEventOtherMouseUp, kCGMouseButtonCenter },
    };
    pthread_mutex_lock(&g_inject_mtx);
    int offen = 0;
    for (int k = 0; k < 256; k++) {
        if (g_key_down[k] == kanal) {
            g_key_down[k] = 0;
            offen++;
            CGEventRef e = CGEventCreateKeyboardEvent(g_evsrc, (CGKeyCode)k, false);
            if (e) {
                CGEventSetFlags(e, 0);
                g_tasten_posten(e);
                CFRelease(e);
            }
        }
    }
    // Eine gedrueckte Maustaste machte sonst jede Bewegung des naechsten
    // Zuschauers zum Ziehen, bis der einmal klickt.
    for (int i = 0; i < 3; i++) {
        if ((g_buttons & (1 << i)) && g_button_kanal[i] == kanal) {
            g_buttons &= ~(1 << i);
            g_button_kanal[i] = 0;
            offen++;
            CGEventRef e = CGEventCreateMouseEvent(g_evsrc, maus_los[i].typ, g_pos, maus_los[i].taste);
            if (e) {
                CGEventPost(kCGHIDEventTap, e);
                CFRelease(e);
            }
        }
    }
    if (g_mods_kanal == kanal) g_mods = 0;
    pthread_mutex_unlock(&g_inject_mtx);
    if (offen) logf_(@"Verbindung weg: %d haengende Taste(n) freigegeben", offen);
}

static void eingabe_verbindung(qc_platz *platz, int fd, const struct sockaddr_in *von, void *ctx) {
    uint8_t payload[256];
    char ip[INET_ADDRSTRLEN] = {0};
    inet_ntop(AF_INET, &von->sin_addr, ip, sizeof ip);
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    keepalive_setzen(fd);

    // Der Eingabekanal darf erst aufmachen, wenn der Bildkanal steht: sein
    // Prologue enthaelt dessen Pruefsumme. Wer die nicht kennt, kommt hier
    // nicht durch - damit kann niemand nur die Tastatur uebernehmen.
    // Den fd immer erst nach qc_platz_frei schliessen (siehe qc_annahme.h).
    if (!atomic_load(&g_vid_ready)) {
        logf_gedrosselt(&d_ein_ohne_bild, ip, @"Eingabekanal abgewiesen: kein Bildkanal offen (%s)", ip);
        qc_platz_frei(platz);
        close(fd);
        return;
    }
    uint8_t pro[256];
    size_t prolen = strlen(QC_PRO_INPUT);
    memcpy(pro, QC_PRO_INPUT, prolen);
    pthread_mutex_lock(&g_send_mtx);
    memcpy(pro + prolen, g_vid_hh, QC_HASHLEN);
    uint8_t expect[32];
    memcpy(expect, g_vid_peer, 32);
    uint64_t sitzung = g_sitzung;
    pthread_mutex_unlock(&g_send_mtx);
    prolen += QC_HASHLEN;

    qc_chan *in = malloc(sizeof *in);
    if (!in) { qc_platz_frei(platz); close(fd); return; }
    int hr = qc_chan_accept(in, fd, g_id_priv, pro, prolen);
    if (qc_platz_frei(platz)) {
        // Fuer eine neuere Verbindung verdraengt: schon gezaehlt und gemeldet.
        qc_chan_free(in);
        close(fd);
        return;
    }
    if (hr != 0) {
        logf_gedrosselt(&d_ein_handschlag, ip, @"Eingabekanal: Handschlag mit %s gescheitert (%d)", ip, hr);
        qc_chan_free(in);
        close(fd);
        return;
    }
    if (memcmp(in->peer, expect, 32) != 0) {
        logf_gedrosselt(&d_ein_fremd, ip, @"Eingabekanal abgewiesen: andere Gegenstelle als beim Bild (%s)", ip);
        qc_chan_free(in);
        close(fd);
        return;
    }
    // Waehrend des Handschlags kann der Zuschauer gewechselt haben oder
    // weg sein; dann gehoert dieser Kanal zu einer Sitzung, die es nicht
    // mehr gibt. Sonst loest er einen aelteren Eingabekanal derselben
    // Sitzung ab (der Client hat neu verbunden).
    // Jeder angenommene Kanal bekommt eine eigene Nummer: danach richtet
    // sich, welche gedrueckten Tasten er am Ende freigibt.
    static uint64_t kanaele = 0;        // durch g_send_mtx geschuetzt
    uint64_t kanal = 0;
    pthread_mutex_lock(&g_send_mtx);
    BOOL aktuell = sitzung == g_sitzung && atomic_load(&g_vid_ready);
    if (aktuell) {
        kanal = ++kanaele;
        g_in_kanal = kanal;             // seine Faehigkeiten meldet er gleich selbst
        int alt = atomic_exchange(&g_in_fd, fd);
        if (alt >= 0) shutdown(alt, SHUT_RDWR);
    }
    pthread_mutex_unlock(&g_send_mtx);
    if (!aktuell) {
        logf_(@"Eingabekanal abgewiesen: Zuschauer inzwischen gewechselt");
        qc_chan_free(in);
        close(fd);
        return;
    }
    logf_(@"Eingabekanal verbunden, verschluesselt");

    for (;;) {
        qc_hdr h;
        if (qc_chan_read(in, &h, sizeof h) != 0) break;
        if (h.type == QC_MSG_CLIP) {
            // Text kann gross sein, deshalb hier ein eigener Puffer.
            if (h.len > 4 * 1024 * 1024) break;
            char *big = h.len ? malloc(h.len + 1) : NULL;
            BOOL ok = !h.len || (big && qc_chan_read(in, big, h.len) == 0);
            // Abgeloest: der Text geht nicht mehr in die Ablage dieses Macs.
            if (ok && atomic_load(&g_in_fd) != fd) ok = NO;
            if (ok && big) { big[h.len] = 0; qc_clip_set(big, h.len); }
            // Die Ablage kann Geheimes tragen: nichts davon bleibt im Speicher liegen.
            if (big) { qc_wipe(big, h.len + 1); free(big); }
            if (!ok) break;
            continue;
        }
        size_t grenze = qc_datei_grenze(h.type);
        if (grenze) {
            // Dateien und Faehigkeiten (50-53, 69): eigene Obergrenzen, vor
            // der 256-Byte-Regel - wer darueber liegt, verliert den Kanal wie
            // bisher. Gelesen wird hier, ausserhalb aller Sperren; auf die
            // Platte schreibt die Warteschlange "dateien-empfang", nie dieser
            // Faden, damit Maus und Tastatur nicht auf die Platte warten.
            if (h.len > grenze) break;
            uint8_t *d = malloc(h.len ? h.len : 1);
            BOOL ok = d && (!h.len || qc_chan_read(in, d, h.len) == 0);
            // Abgeloest: gehoert nicht mehr zu dieser Sitzung.
            if (ok && atomic_load(&g_in_fd) != fd) ok = NO;
            if (!ok) {
                if (d) { qc_wipe(d, h.len); free(d); }
                break;
            }
            @autoreleasepool {
                // Dateiinhalte bleiben so wenig im Speicher liegen wie die Ablage.
                NSData *nutzlast = [[NSData alloc] initWithBytesNoCopy:d length:h.len
                                                           deallocator:^(void *b, NSUInteger l) { qc_wipe(b, l); free(b); }];
                dateien_nachricht(sitzung, kanal, h.type, nutzlast);
            }
            continue;
        }
        if (h.len > sizeof payload) break;
        if (h.len && qc_chan_read(in, payload, h.len) != 0) break;
        // Abgeloest (neuer Zuschauer, Bild weg, neuerer Eingabekanal):
        // was jetzt noch kommt, wird nicht mehr eingespeist.
        if (h.type == QC_IN_MOVE || h.type == QC_IN_BUTTON || h.type == QC_IN_SCROLL || h.type == QC_IN_KEY) {
            if (!einspeisen(fd, kanal, h.type, payload, h.len)) break;
            continue;
        }
        if (atomic_load(&g_in_fd) != fd) break;
        switch (h.type) {
            case QC_IN_TIME:
                // Zeitabgleich: die Frage traegt die Uhrzeit des Clients,
                // die Antwort gibt sie zurueck und haengt die des Hosts an.
                // Der Client rechnet daraus Versatz und Umlaufzeit aus.
                if (h.len >= 8) {
                    uint64_t t_client;
                    memcpy(&t_client, payload, 8);
                    uint8_t out[16];
                    uint64_t t_host = now_us();
                    memcpy(out, &t_client, 8);
                    memcpy(out + 8, &t_host, 8);
                    send_small(QC_MSG_TIME, out, sizeof out);
                }
                break;
            case QC_IN_SETTINGS:
                if (h.len >= 8) {
                    uint32_t m; uint16_t f;
                    memcpy(&m, payload, 4);
                    memcpy(&f, payload + 4, 2);
                    int g = payload[6] ? 1 : 0;
                    int fx = payload[7] ? 1 : 0;
                    // Ein Client ohne das neunte Byte will Ton.
                    int ton = h.len >= 9 ? (payload[8] ? 1 : 0) : 1;
                    dispatch_async(einstellungen_q(), ^{
                        apply_settings((int)m, (int)f, g, fx, ton);
                    });
                }
                break;
            case QC_IN_TESTBILD:
                // Testbild fuer den Benchmark. Schleife und Schalter
                // gehoeren der Aufnahmewarteschlange - dort laeuft der
                // Takt, der die Bilder holt.
                if (h.len >= 1 && g_capq) {
                    int an = payload[0] ? 1 : 0;
                    dispatch_async(g_capq, ^{
                        if (an) {
                            // Kommt das erst nach einer Abloesung dran, war es
                            // der Wunsch des Vorgaengers: der Neue faengt mit
                            // dem Bildschirm an.
                            pthread_mutex_lock(&g_send_mtx);
                            BOOL gilt = sitzung == g_sitzung;
                            pthread_mutex_unlock(&g_send_mtx);
                            if (!gilt) return;
                            // In der Farbe des laufenden Encoders: in HDR10 die
                            // PQ-Fassung (testbild.h).
                            int pq = atomic_load(&g_farbe_pq);
                            qc_testbild_start(g_info_w, g_info_h, pixfmt_fuer(atomic_load(&g_codec_id)), pq);
                            atomic_store(&g_testbild, 1);
                            logf_(@"Testbild an: %dx%d, Schleife fuer den Benchmark%s", g_info_w, g_info_h, pq ? ", HDR10" : "");
                        } else {
                            atomic_store(&g_testbild, 0);
                            qc_testbild_stop();
                            logf_(@"Testbild aus");
                        }
                    });
                }
                break;
            case QC_IN_CODEC:
                // Codecwunsch. Nur weiterreichen - der Wechsel selbst
                // gehoert auf die Aufnahmewarteschlange, nie hierher.
                if (h.len >= 1) {
                    int idx = payload[0];
                    if (g_capq) dispatch_async(g_capq, ^{ codec_wechseln(idx); });
                    else logf_(@"Codecwunsch %d verworfen: Aufnahme laeuft noch nicht", idx);
                }
                break;
            case QC_IN_BILDSCHIRM: {
                // Bildschirmwunsch. Ebenfalls nur weiterreichen, auf die
                // Lebenslauf-Warteschlange, der der Strom gehoert. Keine
                // Sitzungspruefung: der Wunsch gilt hostweit, der letzte
                // gewinnt. Ungueltiges wird uebergangen, der Kanal bleibt.
                NSString *kennung = nil;
                if (qc_bildschirm_wunsch_lesen(payload, h.len, &kennung) != 0) {
                    logf_(@"Bildschirmwunsch ungueltig (%u Byte) - uebergangen", h.len);
                    break;
                }
                if (g_lifeq) dispatch_async(g_lifeq, ^{ bildschirm_wunsch_setzen(kennung); });
                else logf_(@"Bildschirmwunsch %@ verworfen: Aufnahme laeuft noch nicht", kennung ?: @"Automatik");
                break;
            }
            case QC_IN_ANZEIGE: {
                // Lage der Anzeige des Clients (hdr.h): fuer diese Sitzung
                // merken und neu entscheiden, auf der Aufnahmewarteschlange.
                // Unlesbares (zu kurz, andere Fassung) gilt wie kein
                // IN_ANZEIGE, der Kanal bleibt.
                qc_hdr_anzeige a;
                if (!qc_hdr_anzeige_lesen(payload, h.len, &a)) {
                    logf_(@"IN_ANZEIGE ungueltig (%u Byte) - uebergangen", h.len);
                    break;
                }
                pthread_mutex_lock(&g_send_mtx);
                int gilt = sitzung == g_sitzung;
                if (gilt) {
                    // Eine neue Lage (Flags oder Wunsch; Pegel und Kopfraum
                    // allein nicht) darf HDR noch einmal versuchen.
                    int vorher = g_anzeige_da && g_anzeige_sitzung == sitzung;
                    if (!vorher || g_anzeige.flags != a.flags || g_anzeige.wunsch != a.wunsch)
                        atomic_store(&g_hdr_gescheitert, 0);
                    g_anzeige = a;
                    g_anzeige_sitzung = sitzung;
                    g_anzeige_da = 1;
                }
                pthread_mutex_unlock(&g_send_mtx);
                if (!gilt) break;
                logf_(@"IN_ANZEIGE: Schirm %s, Darstellung %s, Wunsch %d, Weiss %u nit, Spitze %u nit, Kopfraum %.2f/%.2f",
                      (a.flags & QC_HDR_ANZEIGE_SCHIRM_HDR) ? "HDR" : "SDR", (a.flags & QC_HDR_ANZEIGE_DARSTELLUNG) ? "ja" : "nein",
                      a.wunsch, a.sdr_weiss_nit, a.spitze_nit, a.kopfraum_potentiell / 100.0, a.kopfraum_aktuell / 100.0);
                if (g_capq) dispatch_async(g_capq, ^{ hdr_neu_entscheiden("IN_ANZEIGE"); });
                else hdr_neu_entscheiden("IN_ANZEIGE");
                break;
            }
            default: break;
        }
    }
    logf_(@"Eingabekanal getrennt");
    // Eine Uebertragung, die ueber diesen Kanal kam, bekommt keine Stuecke mehr.
    qc_empfang_kanal_weg(sitzung, kanal);
    // Austragen und schliessen unter derselben Sperre wie das Abbrechen:
    // so kann eingabe_abbrechen nie eine schon neu vergebene Nummer treffen.
    pthread_mutex_lock(&g_send_mtx);
    int selbst = fd;
    atomic_compare_exchange_strong(&g_in_fd, &selbst, -1);
    close(fd);
    pthread_mutex_unlock(&g_send_mtx);
    alle_tasten_loslassen(kanal);
    qc_chan_free(in);
    qc_wipe(payload, sizeof payload);
}

static int start_input_server(int port, CGDirectDisplayID display) {
    atomic_store(&g_input_display, display);
    // Einmal je Prozess - auch wenn die Freigabe aus- und wieder angeht.
    if (!g_evsrc) {
        g_evsrc = CGEventSourceCreate(kCGEventSourceStateCombinedSessionState);
        if (g_evsrc) CGEventSourceSetLocalEventsSuppressionInterval(g_evsrc, 0.0);
    }

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    a.sin_port = htons((uint16_t)port);
    if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0 || listen(fd, QC_LISTEN_WARTESCHLANGE) != 0) {
        logf_(@"Eingabe-Port %d nicht verfuegbar: %s", port, strerror(errno));
        close(fd);
        return -1;
    }
    qc_annahme_cfg cfg = { QC_HANDSCHLAEGE, QC_HANDSCHLAEGE_JE_IP, eingabe_verbindung, annahme_andrang, (void *)"Eingabekanal" };
    qc_annahme *an = qc_annahme_neu(fd, &cfg);
    if (!an) {
        logf_(@"Eingabe-Port %d: Annahme nicht startbar", port);
        close(fd);
        return -1;
    }
    g_annahme_eingabe = an;
    return fd;
}

// ------------------------------------------------------------- Encoder-Teil

typedef struct {
    FILE *out;
    int nal_len;
    BOOL wrote_ps;
    long encoded;
    long dropped;
    long long bytes;
    OSStatus first_err;
} EncStats;

static EncStats g_stats = {0};
VTCompressionSessionRef g_session = NULL;
// Encoder-Fehler je Sitzung. Wird in encoder_start auf 0 gesetzt; gemeldet wird
// nur der ERSTE Fehler einer Sitzung, sonst kaeme je Bild eine Zeile. Atomar,
// weil encode_buffer (g_capq) und enc_cb (VideoToolbox-Thread) beide zaehlen.
static _Atomic int g_enc_fehler = 0;

static const uint8_t kStartCode[4] = {0, 0, 0, 1};

// HDR10: die Praefix-SEI mit MDCV (137) und CLL (144), gebaut in
// encoder_start (hdr.c, mit Emulationsschutz). VideoToolbox schreibt diese
// Angaben nicht in den Strom (gemessen: nur eine SEI vom Typ 5); fuer
// --capture-Dateien und fremde Spieler steht sie deshalb hinter den
// Parametersaetzen jedes Vollbilds. Geschrieben nur, solange keine Sitzung
// laeuft (encoder_start); gelesen im Rueckruf des Encoders.
static uint8_t g_sei[64];
static size_t g_sei_n = 0;

// Sammelt Parametersaetze und Bilddaten in eine Zugriffseinheit und schickt sie
// in EINEM Aufruf weg: weniger Systemaufrufe, keine halben Bilder auf der Leitung.
#define QC_MAX_IOV 256

// Merker, die ein Bild vom Encoder bis zum Versand begleiten (Begleitzettel,
// QC_ZETTEL). WIEDERHOLT: ein altes Bild, noch einmal in den Encoder gegeben
// (feste Bildrate) oder nachgereicht (stiller Bildschirm) - es zaehlt nicht
// in die Latenzmessung, sein Alter sagt nichts ueber die Strecke. TESTBILD:
// aus der Testbild-Schleife, nicht vom Bildschirm.
#define QC_BILD_WIEDERHOLT 1u
#define QC_BILD_TESTBILD   2u

static void emit_access_unit(CMSampleBufferRef sb, BOOL keyframe, uint64_t t_cap_us, int merker) {
    struct iovec iov[QC_MAX_IOV];
    int wiederholt = (merker & QC_BILD_WIEDERHOLT) != 0;

    // Vor jedem Bild geht ein Zeitstempel raus: wann es aufgenommen wurde und
    // wann der Encoder fertig war. Beides auf der Uhr des Hosts. Der Client
    // rechnet das mit seinem Zeitabgleich in echte Millisekunden um. Stempel
    // und Bild gehen in einem Rutsch raus, damit sie nicht auseinanderfallen.
    uint8_t stamp[24];
    uint32_t seq = atomic_fetch_add(&g_seq, 1);
    // Die Aufnahmezeit kommt vom Begleitzettel des Bildes, nicht aus dem
    // Zeitstempel des Encoders: der ist bei nachgelegten Bildern die
    // Nachlegezeit und wuerde die Messung schoenen.
    uint64_t t_cap = t_cap_us ? t_cap_us : cmtime_us(CMSampleBufferGetPresentationTimeStamp(sb));
    uint64_t t_enc = now_us();
    if (!wiederholt && t_cap && t_enc > t_cap) {
        atomic_fetch_add(&g_enc_us, (long long)(t_enc - t_cap));
        atomic_fetch_add(&g_enc_n, 1);
    }
    memcpy(stamp + 0, &seq, 4);
    memset(stamp + 4, 0, 4);
    stamp[4] = (uint8_t)(wiederholt ? 1 : 0);
    memcpy(stamp + 8, &t_cap, 8);
    memcpy(stamp + 16, &t_enc, 8);
    qc_hdr shdr = { .type = QC_MSG_STAMP, .flags = 0, .reserved = 0, .len = sizeof stamp };

    qc_hdr hdr = { .type = QC_MSG_VIDEO, .flags = (uint8_t)(keyframe ? QC_FLAG_KEY : 0),
                   .reserved = (uint16_t)(seq & 0xffff), .len = 0 };
    iov[0].iov_base = &shdr; iov[0].iov_len = sizeof shdr;
    iov[1].iov_base = stamp; iov[1].iov_len = sizeof stamp;
    int cnt = 3;                 // iov[2] bleibt fuer den Bildkopf reserviert
    iov[2].iov_base = &hdr;
    iov[2].iov_len = sizeof hdr;
    size_t total = 0;

    if (keyframe) {
        // Parametersaetze (VPS/SPS/PPS bei HEVC, SPS/PPS bei H.264) vor jedes
        // Vollbild. Welche Abfrage gilt, entscheidet der laufende Codec.
        CMFormatDescriptionRef fd = CMSampleBufferGetFormatDescription(sb);
        size_t count = 0; int nal = 4;
        BOOL h264 = ist_h264(atomic_load(&g_codec_id)) ? YES : NO;
        OSStatus ps_st = !fd ? -1
                       : h264 ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, 0, NULL, NULL, &count, &nal)
                              : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, 0, NULL, NULL, &count, &nal);
        if (ps_st == noErr) {
            g_stats.nal_len = nal;
            for (size_t i = 0; i < count && cnt + 2 < QC_MAX_IOV; i++) {
                const uint8_t *ps = NULL; size_t psz = 0;
                OSStatus r = h264 ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, i, &ps, &psz, NULL, NULL)
                                  : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, i, &ps, &psz, NULL, NULL);
                if (r == noErr && ps && psz) {
                    iov[cnt].iov_base = (void *)kStartCode; iov[cnt].iov_len = 4; cnt++;
                    iov[cnt].iov_base = (void *)ps;         iov[cnt].iov_len = psz; cnt++;
                    total += 4 + psz;
                }
            }
        }
        // HDR10: hinter VPS/SPS/PPS die eigene Praefix-SEI (137 + 144), vor
        // den Daten des Bildes.
        if (!h264 && atomic_load(&g_farbe_pq) && g_sei_n && cnt + 2 < QC_MAX_IOV) {
            iov[cnt].iov_base = (void *)kStartCode; iov[cnt].iov_len = 4; cnt++;
            iov[cnt].iov_base = g_sei;              iov[cnt].iov_len = g_sei_n; cnt++;
            total += 4 + g_sei_n;
        }
    }

    CMBlockBufferRef bb = CMSampleBufferGetDataBuffer(sb);
    size_t len = 0; char *data = NULL;
    if (!bb || CMBlockBufferGetDataPointer(bb, 0, NULL, &len, &data) != noErr) return;
    int nl = g_stats.nal_len > 0 ? g_stats.nal_len : 4;
    size_t off = 0;
    while (off + (size_t)nl <= len && cnt + 2 < QC_MAX_IOV) {
        uint32_t n = 0;
        for (int i = 0; i < nl; i++) n = (n << 8) | (uint8_t)data[off + i];
        if (off + nl + n > len) break;
        iov[cnt].iov_base = (void *)kStartCode;        iov[cnt].iov_len = 4; cnt++;
        iov[cnt].iov_base = (void *)(data + off + nl); iov[cnt].iov_len = n; cnt++;
        total += 4 + n;
        off += (size_t)nl + n;
    }
    if (cnt <= 3) return;
    hdr.len = (uint32_t)total;

    // In die Datei geht der reine Bitstrom ohne unsere Koepfe, damit man sie
    // direkt abspielen kann.
    if (g_stats.out) {
        for (int i = 3; i < cnt; i++) fwrite(iov[i].iov_base, 1, iov[i].iov_len, g_stats.out);
        g_stats.bytes += total;
    }

    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (fd >= 0) {
        // Ein frisch verbundener Zuschauer bekommt erst ab dem naechsten Vollbild
        // Daten, sonst faengt er mitten in einem Bild ohne Kopfdaten an.
        if (!atomic_load(&g_vid_ready)) { pthread_mutex_unlock(&g_send_mtx); return; }
        if (atomic_load(&g_wait_key) && !keyframe) { pthread_mutex_unlock(&g_send_mtx); return; }
        // Ein Testbild-Rahmen, der noch im Encoder steckte oder gerade in ihn
        // ging, als ein neuer Zuschauer das Testbild abschaltete
        // (bild_verbindung), gehoert nicht zu ihm - auch nicht als Vollbild
        // (die kommen im Testbild alle zwei Sekunden von selbst). Er wartet
        // weiter, und das naechste Bild vom Bildschirm wird ein Vollbild.
        if (atomic_load(&g_wait_key) && (merker & QC_BILD_TESTBILD) && !atomic_load(&g_testbild)) {
            atomic_store(&g_force_key, 1);
            pthread_mutex_unlock(&g_send_mtx);
            return;
        }
        // Gegen Stau wird VOR dem Encoder verworfen (stau_vor_dem_encoder):
        // ein codiertes Bild geht immer raus. Fiele es hier weg, fehlte den
        // folgenden Zwischenbildern ihr Bezug, das naechste Bild muesste ein
        // Vollbild sein - und auf einer knappen Leitung kaemen dann nur noch
        // Vollbilder: so geschehen im Spielmodus bei 500 Mbit/s, rund 20
        // Bilder je Sekunde, jedes ein Vollbild (Hostprotokoll).
        // Erst wenn das Bild wirklich rausgeht, ist das Warten vorbei.
        atomic_store(&g_wait_key, 0);
        if (qc_chan_send(g_vid, iov, cnt) != 0) {
            logf_(@"Zuschauer weg: %s", strerror(errno));
            atomic_store(&g_vid_ready, 0);
            atomic_store(&g_client_fd, -1);
            stream_herunterfahren_anstossen();
            close(fd);
            eingabe_abbrechen();
            qc_chan_free(g_vid);
            g_vid = NULL;
            pthread_mutex_unlock(&g_send_mtx);
            qc_ui_zustand_geaendert();
            return;
        }
        atomic_fetch_add(&g_sent_frames, 1);
        atomic_fetch_add(&g_sent_bytes, (long long)total);
    }
    pthread_mutex_unlock(&g_send_mtx);
}

static void enc_cb(void *ref, void *src, OSStatus status, VTEncodeInfoFlags flags, CMSampleBufferRef sb) {
    (void)ref;
    uint64_t zettel = (uint64_t)(uintptr_t)src;
    uint64_t t_cap_us = zettel >> 2;
    int merker = (int)(zettel & 3u);
    // Jeder Rueckruf schliesst ein Bild ab - geliefert, verworfen oder mit
    // Fehler. Unter null darf der Zaehler nie (Rueckrufe einer alten Sitzung).
    if (atomic_fetch_sub(&g_inflight, 1) <= 0) atomic_store(&g_inflight, 0);
    if (flags & kVTEncodeInfo_FrameDropped) g_stats.dropped++;
    if (status != noErr) {
        if (!g_stats.first_err) g_stats.first_err = status;
        // Im Dienstbetrieb sieht sonst niemand, dass der Encoder nach einem
        // Wechsel nichts mehr liefert. Einmal je Sitzung, nicht je Bild.
        if (atomic_fetch_add(&g_enc_fehler, 1) == 0)
            logf_(@"Encoder %s meldet Fehler beim Codieren (%d)", g_kandidaten[atomic_load(&g_codec_id)].name, (int)status);
        return;
    }
    if (!sb) return;

    CFArrayRef att = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    BOOL keyframe = YES;
    if (att && CFArrayGetCount(att) > 0) {
        CFDictionaryRef d = CFArrayGetValueAtIndex(att, 0);
        keyframe = !CFDictionaryContainsKey(d, kCMSampleAttachmentKey_NotSync);
    }
    if (!g_stats.wrote_ps) { keyframe = YES; g_stats.wrote_ps = YES; }

    emit_access_unit(sb, keyframe, t_cap_us, merker);
    g_stats.encoded++;
}

static void set_i32(VTCompressionSessionRef s, CFStringRef key, int32_t v) {
    CFNumberRef n = CFNumberCreate(NULL, kCFNumberSInt32Type, &v);
    VTSessionSetProperty(s, key, n);
    CFRelease(n);
}

// ---------------------------------------------------------- Was kann dieser Mac?
//
// Nicht raten, sondern fragen: fuer jeden Kandidaten wird eine Sitzung
// aufgemacht und der Encoder selbst befragt, ob er das Profil anbietet - einmal
// mit der Forderung nach Hardware, einmal ohne. Was hier nicht auftaucht, kann
// die Maschine nicht, egal was in den Kopfdateien steht.



// Einen Kandidaten in w x h pruefen. hw_pflicht = 1 verlangt einen
// Hardware-Encoder.
static int kandidat_pruefen(const qc_codec_kandidat *k, int hw_pflicht, int *ist_hw, int w, int h) {
    CFMutableDictionaryRef spec = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (hw_pflicht)
        CFDictionarySetValue(spec, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);

    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, w, h, k->codec, spec, NULL, NULL, NULL, NULL, &s);
    CFRelease(spec);
    if (st != noErr || !s) return 0;

    int ok = 0;
    CFStringRef profil = vt_sym(k->profil);
    if (profil && profile_supported(s, profil)) {
        // Anbieten allein reicht nicht - es muss sich auch setzen lassen.
        ok = (VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, profil) == noErr);
    }
    if (ok && ist_hw) {
        CFBooleanRef hw = NULL;
        if (VTSessionCopyProperty(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, NULL, &hw) == noErr && hw) {
            *ist_hw = CFBooleanGetValue(hw);
            CFRelease(hw);
        }
    }
    VTCompressionSessionInvalidate(s);
    CFRelease(s);
    return ok;
}

// Die HDR10-Eigenschaften einer Sitzung: BT.2020, PQ, BT.2020-NCL, dazu
// Mastering (ST 2086) und Lichtpegel (MaxCLL/MaxFALL) als Bytes. noErr nur,
// wenn der Encoder alles nimmt.
static OSStatus hdr_eigenschaften_setzen(VTCompressionSessionRef s) {
    OSStatus st = VTSessionSetProperty(s, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_2020);
    if (st == noErr) st = VTSessionSetProperty(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ);
    if (st == noErr) st = VTSessionSetProperty(s, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_2020);
    if (st != noErr) return st;
    // ST 2086 wie in der SEI: G, B, R, Weiss (x, y in 0,00002), dann max und
    // min Leuchtdichte in 0,0001 nit - alles big endian.
    qc_hdr_sei_werte w;
    hdr_sei_werte(&w);
    uint8_t m[24], c[4];
    size_t o = 0;
    for (int i = 0; i < 3; i++) {
        m[o++] = (uint8_t)(w.x[i] >> 8); m[o++] = (uint8_t)w.x[i];
        m[o++] = (uint8_t)(w.y[i] >> 8); m[o++] = (uint8_t)w.y[i];
    }
    m[o++] = (uint8_t)(w.weiss_x >> 8); m[o++] = (uint8_t)w.weiss_x;
    m[o++] = (uint8_t)(w.weiss_y >> 8); m[o++] = (uint8_t)w.weiss_y;
    for (int k = 3; k >= 0; k--) m[o++] = (uint8_t)(w.max_lum >> (8 * k));
    for (int k = 3; k >= 0; k--) m[o++] = (uint8_t)(w.min_lum >> (8 * k));
    c[0] = (uint8_t)(w.max_cll >> 8); c[1] = (uint8_t)w.max_cll;
    c[2] = (uint8_t)(w.max_fall >> 8); c[3] = (uint8_t)w.max_fall;
    CFDataRef dm = CFDataCreate(NULL, m, sizeof m), dc = CFDataCreate(NULL, c, sizeof c);
    st = VTSessionSetProperty(s, kVTCompressionPropertyKey_MasteringDisplayColorVolume, dm);
    if (st == noErr) st = VTSessionSetProperty(s, kVTCompressionPropertyKey_ContentLightLevelInfo, dc);
    CFRelease(dm);
    CFRelease(dc);
    return st;
}

// Kann dieser Kandidat HDR10? Eine Probesitzung (wie beim Codecwechsel mit
// oder ohne Hardware-Pflicht) muss Profil und HDR-Eigenschaften nehmen.
static int kandidat_hdr_pruefen(const qc_codec_kandidat *k, int hw_pflicht) {
    CFMutableDictionaryRef spec = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (hw_pflicht)
        CFDictionarySetValue(spec, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);
    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, 1920, 1080, k->codec, spec, NULL, NULL, NULL, NULL, &s);
    CFRelease(spec);
    if (st != noErr || !s) return 0;
    CFStringRef profil = vt_sym(k->profil);
    int ok = profil && VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, profil) == noErr &&
             hdr_eigenschaften_setzen(s) == noErr;
    VTCompressionSessionInvalidate(s);
    CFRelease(s);
    return ok;
}

static void codecs_pruefen(void) {
    logf_(@"--- Was dieser Mac codieren kann ---");
    int system_hdr = hdr_system_kann();
    for (size_t i = 0; i < QC_KANDIDATEN; i++) {
        const qc_codec_kandidat *k = &g_kandidaten[i];
        g_befund[i].hdr = 0;
        // AV1 nie anbieten, egal was der Encoder kann: Strominfo und
        // Codecwechsel kennen nur HEVC und H.264 - der Host saehe AV1 als
        // HEVC an und laese HEVC-Parametersaetze -, und der Client hat
        // keinen AV1-Weg. Bisher verhinderte das nur das fehlende Profil.
        if (k->codec == 'av01') {
            g_befund[i].vorhanden = 0;
            g_befund[i].hardware = 0;
            logf_(@"  %-18s nein  (nicht angeboten: Protokoll und Client kennen AV1 noch nicht)", k->name);
            continue;
        }
        int hw = 0;
        int da = kandidat_pruefen(k, 1, &hw, 1920, 1080);   // erst mit Hardware-Pflicht
        if (da) {
            g_befund[i].vorhanden = 1;
            g_befund[i].hardware = hw;
        } else {
            hw = 0;
            da = kandidat_pruefen(k, 0, &hw, 1920, 1080);   // dann ohne
            g_befund[i].vorhanden = da;
            g_befund[i].hardware = da ? hw : 0;
        }
        // H.264 auch in der Groesse, bis zu der der Host es einsetzt: seine
        // Grenze (groessere Stroeme passt er ein). Angeboten wird es nur
        // ehrlich, wenn der Encoder dort auch aufgeht - sonst gilt 1920x1080
        // als Grenze, und groessere Stroeme werden darauf eingepasst.
        NSString *h264_grenze = @"";
        if (g_befund[i].vorhanden && ist_h264((int)i)) {
            int gw = QC_H264_MAX_SEITE, gh = QC_H264_MAX_MB * 256 / QC_H264_MAX_SEITE, hw_g = 0;
            g_h264_grenze = (qc_h264_grenze){ QC_H264_MAX_SEITE, QC_H264_MAX_MB };
            if (kandidat_pruefen(k, g_befund[i].hardware, &hw_g, gw, gh)) {
                h264_grenze = [NSString stringWithFormat:@", bis %dx%d geprueft - groessere Bildschirme werden eingepasst", gw, gh];
            } else {
                g_h264_grenze = (qc_h264_grenze){ 1920, 8160 };
                h264_grenze = [NSString stringWithFormat:@", oeffnet nicht in %dx%d - Grenze 1920x1080, groessere Bildschirme werden eingepasst", gw, gh];
            }
        }
        // HDR10 nur mit HEVC 10 Bit (Kandidat 0 und 2), und nur, wenn auch
        // die Aufnahme HDR liefern kann.
        if (g_befund[i].vorhanden && system_hdr && qc_hdr_codec_kann((int)i))
            g_befund[i].hdr = kandidat_hdr_pruefen(k, g_befund[i].hardware);
        logf_(@"  %-18s %@%@%@%@", k->name,
              g_befund[i].vorhanden ? @"ja " : @"nein",
              g_befund[i].vorhanden ? (g_befund[i].hardware ? @"(Hardware)" : @"(Software)") : @"",
              g_befund[i].hdr ? @", HDR10 (BT.2020/PQ)" : @"", h264_grenze);
    }
    if (!system_hdr) logf_(@"  HDR10: nein - die Aufnahme in HDR braucht macOS 15 und Apple Silicon");
}

// Encoder fuer einen Kandidaten aus g_kandidaten oeffnen. Codec, Profil und
// Aufnahmeformat kommen aus der Tabelle; das Profil muss der Encoder selbst
// anbieten, sonst brechen wir ab statt still etwas anderes zu liefern.
// g_session wird erst gesetzt, wenn die Sitzung vollstaendig steht - bei einem
// Fehlschlag bleibt sie so, wie sie war (beim Codecwechsel: NULL).
// pq: HDR10 - BT.2020/PQ/BT.2020 im VUI, Mastering und Lichtpegel, dazu die
// eigene SEI vor jedem Vollbild. Nimmt der Encoder das nicht: NO, kein
// stilles SDR. g_farbe_pq folgt der Sitzung.
static BOOL encoder_start(int idx, int w, int h, int fps, int mbit, int pq) {
    if (idx < 0 || idx >= (int)QC_KANDIDATEN) return NO;
    if (pq && (ist_h264(idx) || !qc_hdr_codec_kann(idx))) {
        logf_(@"HDR10 mit %s nicht moeglich - nur HEVC 10 Bit", g_kandidaten[idx].name);
        return NO;
    }
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    OSType pixfmt = pixfmt_fuer(idx);
    // Eine neue Sitzung faengt leer an; Rueckrufe der alten kommen nicht mehr.
    atomic_store(&g_inflight, 0);

    CFMutableDictionaryRef spec = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    // Hardware verlangen, wenn die Pruefung beim Start welche gefunden hat. Sonst
    // ist der Kandidat nur in Software da - und genau so steht er in der Koennensliste.
    if (g_befund[idx].hardware)
        CFDictionarySetValue(spec, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);

    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, w, h, k->codec, spec, NULL, NULL, enc_cb, NULL, &s);
    CFRelease(spec);
    if (st != noErr || !s) { logf_(@"Encoder-Session fuer %s fehlgeschlagen (%d)", k->name, (int)st); return NO; }

    CFStringRef want = vt_sym(k->profil);
    if (!profile_supported(s, want)) {
        logf_(@"Profil %s wird von diesem Encoder nicht angeboten - Abbruch statt stiller Ersatzausgabe", k->profil);
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
        return NO;
    }
    st = VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, want);
    if (st != noErr) {
        logf_(@"Profil %s abgelehnt (%d)", k->profil, (int)st);
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
        return NO;
    }

    VTSessionSetProperty(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_AllowOpenGOP, kCFBooleanFalse);
    if (pq) {
        // HDR10. Die Bilder der Aufnahme tragen ihre Farbe als Anhaenge
        // (Display P3 PQ, Matrix BT.709) - VideoToolbox rechnet sie nach
        // BT.2020 um; das Testbild kommt schon in BT.2020.
        st = hdr_eigenschaften_setzen(s);
        if (st != noErr) {
            logf_(@"HDR10-Eigenschaften fuer %s abgelehnt (%d) - kein HDR", k->name, (int)st);
            VTCompressionSessionInvalidate(s);
            CFRelease(s);
            return NO;
        }
    } else {
        VTSessionSetProperty(s, kVTCompressionPropertyKey_ColorPrimaries,   kCVImageBufferColorPrimaries_ITU_R_709_2);
        VTSessionSetProperty(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
        VTSessionSetProperty(s, kVTCompressionPropertyKey_YCbCrMatrix,      kCVImageBufferYCbCrMatrix_ITU_R_709_2);
    }
    set_i32(s, kVTCompressionPropertyKey_MaxFrameDelayCount, 1);
    set_i32(s, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
    set_i32(s, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
    set_i32(s, kVTCompressionPropertyKey_MaxKeyFrameInterval, fps * 2);
    VTCompressionSessionPrepareToEncodeFrames(s);

    CFBooleanRef hw = NULL;
    VTSessionCopyProperty(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, NULL, &hw);
    BOOL is_hw = hw && CFBooleanGetValue(hw);
    if (hw) CFRelease(hw);

    atomic_store(&g_enc_fehler, 0);   // neue Sitzung, neue Meldung erlaubt
    // Die SEI fuer HDR10 steht, bevor die Sitzung ihr erstes Bild liefern kann.
    if (pq) {
        qc_hdr_sei_werte sw;
        hdr_sei_werte(&sw);
        g_sei_n = qc_hdr_sei_bauen(&sw, g_sei, sizeof g_sei);
    } else {
        g_sei_n = 0;
    }
    atomic_store(&g_farbe_pq, pq ? 1 : 0);
    g_session = s;
    logf_(@"Encoder: Kandidat %d %s, %dx%d, Hardware: %@, %d Mbit/s, Eingabe %.4s%@%@",
          idx, k->name, w, h, is_hw ? @"ja" : @"NEIN", mbit,
          (char *)&(uint32_t){CFSwapInt32HostToBig(pixfmt)},
          umrechnung_fuer(idx) ? @" (Umrechnung durch VideoToolbox)" : @"",
          !pq ? @", SDR (BT.709)"
              : [NSString stringWithFormat:@", HDR10 (BT.2020/PQ, Mastering P3 %u nit, SDR-Weiss %u nit, SEI 137/144 %zu Byte)%@",
                 QC_HDR_MASTER_MAX_NIT, (unsigned)hdr_sdr_weiss_nit(), g_sei_n,
                 hdr_p3_umrechnung() ? @", Aufnahme Display P3 -> BT.2020 durch VideoToolbox" : @""]);
    return YES;
}

// ------------------------------------------------------------ Codecwechsel
//
// Alles hier laeuft AUSSCHLIESSLICH auf der Aufnahmewarteschlange g_capq -
// derselben, auf der Bilder in den Encoder gehen und der feste Takt nachlegt.
// Deshalb kann waehrend des Wechsels kein Bild in eine halb abgebaute Sitzung
// laufen, und deshalb braucht nichts hier eine Sperre.
//
// Reihenfolge auf der Leitung: letzte Zugriffseinheit des alten Codecs,
// dann SWITCH (Typ 7), dann Strominfo (Typ 1), dann das erste Vollbild des
// neuen Codecs mit Parametersaetzen. SWITCH geht ERST raus, wenn die neue
// Sitzung wirklich steht (Erfolgszweig von codec_wechsel_abschliessen) - so
// baut der Client seinen Decoder genau einmal um, und scheitert der Wechsel,
// bekommt er gar kein SWITCH und behaelt den alten Decoder. Dass SWITCH
// trotzdem genau zwischen alt und neu landet, liegt daran, dass CompleteFrames
// erst zurueckkommt, wenn der alte Encoder alles abgeliefert hat, dass die
// neue Sitzung Bilder nur ueber g_capq bekommt - und codec_wechsel_abschliessen
// laeuft selbst auf g_capq, sendet SWITCH also, bevor dort das naechste Bild
// drankommt. send_small nimmt dieselbe Sperre wie der Bildversand.

// Die Farbe (SDR oder HDR10) ist die zweite Dimension desselben Wechsels:
// Aufnahme umstellen (Pixelformat und/oder Farbraum per updateConfiguration),
// neue Encoder-Sitzung, SWITCH mit dem Transfer in p[6], Strominfo, Vollbild.
// Ein Farbwechsel ohne anderen Kandidaten ist ein Wechsel auf denselben
// Kandidaten in der anderen Farbe. Scheitert der Weg nach HDR, laeuft es in
// SDR weiter (g_hdr_gescheitert, Grund 6).

static int g_wechsel_aktiv = 0;      // nur auf g_capq: ein Wechsel ist unterwegs
static int g_wechsel_wunsch = -1;    // nur auf g_capq: waehrenddessen eingegangener naechster Wunsch
// Nur auf g_capq: waehrend eines Wechsels aenderte sich die HDR-Lage - danach
// neu entscheiden. Dazu die Sperre fuer Farbwechsel (HDR-Plan 3: hoechstens
// einer je 2 s; jeder kostet ein Vollbild und beim Client den Umbau der
// Anzeige), und ob die Entscheidung nach der Sperre schon eingereiht ist.
static int g_farbe_offen = 0;
static uint64_t g_farbe_gewechselt_us = 0;
static int g_farbe_nachher = 0;
#define QC_FARBE_SPERRE_US (2ull * 1000000ull)

static void strominfo_senden(void) {
    uint8_t p[QC_HDR_INFO_LAENGE];
    qc_hdr_anzeige a;
    int da = anzeige_jetzt(&a);
    strominfo_fuellen(p, da ? &a : NULL);
    atomic_store(&g_hdr_grund_gesendet, p[13]);
    send_small(QC_MSG_INFO, p, sizeof p);
}

static const char *farbe_text(int pq) { return pq ? "HDR10" : "SDR"; }

// Soll Kandidat idx jetzt in HDR10 laufen? Die Entscheidung fuer das
// IN_ANZEIGE der laufenden Sitzung. Nimmt g_send_mtx (anzeige_jetzt) - nie
// unter g_send_mtx rufen.
static int hdr_soll(int idx) {
    qc_hdr_anzeige a;
    int da = anzeige_jetzt(&a);
    return hdr_grund_fuer_idx(idx, da ? &a : NULL) == QC_HDR_GRUND_AKTIV;
}

static void wechsel_ausfuehren(int idx, int pq);

// Neu entscheiden: nach einem IN_ANZEIGE, fuer einen neuen Zuschauer, wenn
// am Bildschirm des Hosts HDR an- oder ausging, nach einem Wechsel, der
// dabei im Weg war. Ergibt das eine andere Farbe als die laufende, folgt der
// Farbwechsel (hoechstens einer je 2 s, sonst danach). Sonst bekommt der
// Zuschauer nur dann eine neue Strominfo, wenn sich ihr HDR-Grund aendert -
// dieselbe Groesse, derselbe Codec, der Client baut dafuer nichts um. Auf
// g_capq, wie Codecwechsel: so landet die Strominfo nie zwischen einem SWITCH
// und dessen eigener Strominfo.
static void hdr_neu_entscheiden(const char *anlass) {
    if (atomic_load(&g_client_fd) < 0) return;
    if (g_wechsel_aktiv) { g_farbe_offen = 1; return; }
    qc_hdr_anzeige a;
    int da = anzeige_jetzt(&a);
    int idx = atomic_load(&g_codec_id);
    int grund = hdr_grund_fuer_idx(idx, da ? &a : NULL);
    int soll = grund == QC_HDR_GRUND_AKTIV;
    int ist = atomic_load(&g_farbe_pq);
    if (soll != ist && g_session && g_stream && g_cfg) {
        uint64_t jetzt = now_us();
        if (g_farbe_gewechselt_us && jetzt - g_farbe_gewechselt_us < QC_FARBE_SPERRE_US) {
            if (!g_farbe_nachher) {
                g_farbe_nachher = 1;
                uint64_t rest = QC_FARBE_SPERRE_US - (jetzt - g_farbe_gewechselt_us);
                logf_(@"HDR-Entscheidung (%s): %s, Grund %d (%s) - der letzte Farbwechsel ist keine 2 s her, in %.1f s",
                      anlass, farbe_text(soll), grund, qc_hdr_grund_text(grund), rest / 1e6);
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(rest * NSEC_PER_USEC)), g_capq, ^{
                    g_farbe_nachher = 0;
                    hdr_neu_entscheiden("nach der Sperre");
                });
            }
            return;
        }
        logf_(@"HDR-Entscheidung (%s): %s, Grund %d (%s) - Farbwechsel %s -> %s", anlass, farbe_text(soll), grund,
              qc_hdr_grund_text(grund), farbe_text(ist), farbe_text(soll));
        wechsel_ausfuehren(idx, soll);
        return;
    }
    // Was die Strominfo jetzt sagen wuerde (wie strominfo_fuellen).
    int neu = ist ? QC_HDR_GRUND_AKTIV : (soll ? QC_HDR_GRUND_WECHSEL_GESCHEITERT : grund);
    int alt = atomic_load(&g_hdr_grund_gesendet);
    if (neu == alt) return;
    logf_(@"HDR-Entscheidung (%s): %s, Grund %d (%s) - vorher Grund %d (%s), neue Strominfo", anlass, farbe_text(ist), neu,
          qc_hdr_grund_text(neu), alt, qc_hdr_grund_text(alt));
    strominfo_senden();
}

// Typ 7: u8 idx, u8 h264, u8 chroma444, u8 zehn_bit, u8 vollbereich (immer 1),
// u8 umrechnung, u8 Transfer nach H.273 (1 SDR, 16 PQ), u8 frei. Umrechnung
// heisst: VideoToolbox rechnet das Aufnahmeformat um (4:4:4 10 Bit nach 8 Bit
// oder 4:2:0) - oder in HDR10 die Farbe der Aufnahme (Display P3 nach BT.2020).
static void switch_senden(int idx) {
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    int pq = atomic_load(&g_farbe_pq);
    uint8_t p[8] = {0};
    p[0] = (uint8_t)idx;
    p[1] = (uint8_t)ist_h264(idx);
    p[2] = (uint8_t)k->chroma444;
    p[3] = (uint8_t)k->zehn_bit;
    p[4] = 1;
    p[5] = (uint8_t)(umrechnung_fuer(idx) || (pq && hdr_p3_umrechnung()));
    p[6] = pq ? QC_HDR_TRANSFER_PQ : QC_HDR_TRANSFER_SDR;
    send_small(QC_MSG_SWITCH, p, sizeof p);
}

// Was der Zuschauer gerade eingestellt hat, auf die frische Sitzung uebertragen.
// Dieselben Aufrufe wie in apply_settings, aber ohne deren Meldung und ohne
// Aufnahmekonfiguration - die ist von einem Codecwechsel nicht betroffen.
static void encoder_einstellungen_nachziehen(void) {
    int fps = atomic_load(&g_cur_fps), mbit = atomic_load(&g_cur_mbit), gaming = atomic_load(&g_cur_gaming);
    if (!g_session || fps <= 0 || mbit <= 0) return;
    set_i32(g_session, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
    set_i32(g_session, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
    set_i32(g_session, kVTCompressionPropertyKey_MaxKeyFrameInterval, gaming ? fps : fps * 2);
}

// Wechsel beendet - ob gelungen oder nicht. Kam waehrenddessen ein weiterer
// Wunsch herein, wird er jetzt angestossen, statt verloren zu gehen; sonst
// eine HDR-Lage, die waehrenddessen kam.
static void codec_wechsel_fertig(void) {
    g_wechsel_aktiv = 0;
    int n = g_wechsel_wunsch;
    g_wechsel_wunsch = -1;
    int farbe = g_farbe_offen;
    g_farbe_offen = 0;
    if (n >= 0) dispatch_async(g_capq, ^{ codec_wechseln(n); });
    else if (farbe) dispatch_async(g_capq, ^{ hdr_neu_entscheiden("nach dem Wechsel"); });
}

// Der Abschluss eines Wechsels kommt spaeter auf g_capq an (nach
// updateConfiguration). Ging der Zuschauer inzwischen, und hat ein neuer den
// Strom schon frisch aufgebaut - mit eigener Sitzung im Format von
// g_codec_id -, gehoert dieser Wechsel zu einem Strom, den es nicht mehr
// gibt: keine zweite Sitzung daneben (die erste liefe ungebremst weiter),
// kein SWITCH an den Neuen. YES = verworfen, der Wechsel ist beendet.
static BOOL codec_wechsel_ueberholt(int idx) {
    if (!g_session) return NO;
    logf_(@"Codecwechsel auf %s verworfen: der Strom wurde inzwischen neu aufgebaut", g_kandidaten[idx].name);
    codec_wechsel_fertig();
    return YES;
}

// Ging der Zuschauer waehrend des Umstellens, fand sein Abbau womoeglich noch
// keine Sitzung vor - die eben gebaute liefe dann fuer niemanden. Also noch
// einmal anstossen; der Abbau prueft selbst, ob inzwischen wieder jemand da
// ist, und laeuft auf g_capq erst nach diesem Block.
static void codec_ohne_zuschauer_abbauen(void) {
    if (atomic_load(&g_client_fd) < 0) stream_herunterfahren_anstossen();
}

// Der Weg von SDR nach HDR10 scheiterte (die Aufnahme lehnte ab, oder der
// Encoder ging in HDR10 nicht auf): ab jetzt SDR mit Grund 6, bis sich die
// Lage aendert. Das gilt sofort, nicht erst, wenn der alte Codec wieder
// laeuft - laesst auch der sich nicht oeffnen, faende der vorgemerkte
// Codecwunsch sonst wieder HDR10 und versuchte denselben Weg ohne Ende. Galt
// der Wechsel einem anderen Kandidaten, gilt der Codecwunsch weiter - danach
// in SDR, ohne erneuten Versuch in HDR10. Nur auf g_capq.
static void hdr_wechsel_gescheitert(int idx, int pq, int alt, int alt_pq) {
    if (!pq || alt_pq) return;
    atomic_store(&g_hdr_gescheitert, 1);
    logf_(@"Wechsel nach HDR10 gescheitert - nur noch SDR (Grund 6), bis sich die Lage aendert");
    if (idx != alt && g_wechsel_wunsch < 0) g_wechsel_wunsch = idx;
}

// Schritt h: der neue Kandidat liess sich nicht oeffnen, der alte kommt zurueck
// - in seiner alten Farbe. Ein SWITCH gab es nicht, der Client decodiert
// weiter mit dem alten Codec - nur die Strominfo geht noch einmal raus (nach
// einem gescheiterten Weg nach HDR10 mit Grund 6).
static void codec_alt_aufbauen(int alt, int alt_pq) {
    if (codec_wechsel_ueberholt(alt)) return;
    if (encoder_start(alt, g_info_w, g_info_h, atomic_load(&g_cur_fps), atomic_load(&g_cur_mbit), alt_pq)) {
        encoder_einstellungen_nachziehen();
        atomic_store(&g_force_key, 1);
        strominfo_senden();
        logf_(@"Alter Codec laeuft wieder: %s, %s", g_kandidaten[alt].name, farbe_text(alt_pq));
        codec_ohne_zuschauer_abbauen();
    } else {
        logf_(@"Auch der alte Codec %s laesst sich nicht mehr oeffnen - es kommt kein Bild mehr", g_kandidaten[alt].name);
    }
    codec_wechsel_fertig();
}

// Schritt g: neue Sitzung bauen, SWITCH ansagen, Einstellungen nachziehen,
// Strominfo melden. Die Reihenfolge ist Absicht: erst g_codec_id, damit
// emit_access_unit die Parametersaetze des neuen Codecs abfragt; dann SWITCH -
// noch kann kein Bild in der neuen Sitzung sein, weil Bilder nur ueber g_capq
// hineingehen und wir gerade darauf laufen; dann Vollbild erzwingen und die
// Strominfo hinterher. alt_fmt und alt_aufnahme_pq: wie die Aufnahme vor dem
// Wechsel eingestellt war (fuer den Rueckweg). alt_w x alt_h und neu_w x
// neu_h: die Stromgroesse vorher und fuer den neuen Kandidaten - bei H.264
// womoeglich eingepasst (groesse_fuer_codec); die Strominfo traegt sie, der
// Zeiger bekommt ihren Massstab.
static void codec_wechsel_abschliessen(int idx, int pq, int alt, int alt_pq, OSType alt_fmt, int alt_aufnahme_pq,
                                       BOOL aufnahme_geaendert, int alt_w, int alt_h, int neu_w, int neu_h) {
    if (codec_wechsel_ueberholt(idx)) return;
    g_info_w = neu_w;
    g_info_h = neu_h;
    if (encoder_start(idx, g_info_w, g_info_h, atomic_load(&g_cur_fps), atomic_load(&g_cur_mbit), pq)) {
        atomic_store(&g_codec_id, idx);
        if (neu_w != alt_w || neu_h != alt_h) zeiger_massstab_nachfuehren(atomic_load(&g_zeiger_punkte), neu_w);
        if (pq != alt_pq) {
            g_farbe_gewechselt_us = now_us();
            atomic_store(&g_anhaenge_loggen, 1);
        }
        // Das Testbild folgt dem Aufnahmeformat und der Farbe des neuen Codecs.
        if (atomic_load(&g_testbild)) qc_testbild_start(g_info_w, g_info_h, pixfmt_fuer(idx), pq);
        switch_senden(idx);
        encoder_einstellungen_nachziehen();
        atomic_store(&g_force_key, 1);
        strominfo_senden();
        logf_(@"Codec gewechselt: %s, %s", g_kandidaten[idx].name, farbe_text(pq));
        codec_ohne_zuschauer_abbauen();
        codec_wechsel_fertig();
        return;
    }

    logf_(@"Codecwechsel auf %s (%s) fehlgeschlagen - baue %s (%s) wieder auf", g_kandidaten[idx].name, farbe_text(pq),
          g_kandidaten[alt].name, farbe_text(alt_pq));
    hdr_wechsel_gescheitert(idx, pq, alt, alt_pq);
    g_info_w = alt_w;
    g_info_h = alt_h;
    // Kein SWITCH: der Client hat noch seinen alten Decoder und behaelt ihn.
    // Ohne Strom (inzwischen abgebaut) kaeme der Abschluss von
    // updateConfiguration nie - eine Nachricht an nil tut nichts -, und der
    // Wechsel bliebe fuer immer unterwegs; jeder weitere Wunsch wuerde nur
    // noch vorgemerkt. Dann gleich zurueck, das Format baut der naechste
    // Zuschauer ohnehin aus g_codec_id.
    if (aufnahme_geaendert) {
        g_cfg.pixelFormat = alt_fmt;
        aufnahme_farbe_setzen(g_cfg, alt_aufnahme_pq);
        g_cfg.width = (size_t)alt_w;
        g_cfg.height = (size_t)alt_h;
    }
    if (aufnahme_geaendert && g_stream) {
        [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
            if (e) logf_(@"Aufnahmeformat liess sich nicht zuruecksetzen: %@", e.localizedDescription);
            dispatch_async(g_capq, ^{ codec_alt_aufbauen(alt, alt_pq); });
        }];
    } else {
        codec_alt_aufbauen(alt, alt_pq);
    }
}

// Der Wechsel auf (idx, pq) selbst - Kandidat, Farbe oder beides. Nur auf
// g_capq, kein Wechsel unterwegs, Aufnahme und g_cfg da (codec_wechseln und
// hdr_neu_entscheiden pruefen das).
static void wechsel_ausfuehren(int idx, int pq) {
    int alt = atomic_load(&g_codec_id);
    int alt_pq = atomic_load(&g_farbe_pq);
    g_wechsel_aktiv = 1;
    if (idx != alt)
        logf_(@"Codecwechsel: %s -> %s%@", g_kandidaten[alt].name, g_kandidaten[idx].name,
              pq != alt_pq ? [NSString stringWithFormat:@", Farbe %s -> %s", farbe_text(alt_pq), farbe_text(pq)] : @"");
    else
        logf_(@"Farbwechsel: %s -> %s (%s)", farbe_text(alt_pq), farbe_text(pq), g_kandidaten[idx].name);

    // b) Das letzte Bild bleibt liegen, auch wenn sich das Aufnahmeformat
    //    aendert: bei stillem Bildschirm ist es womoeglich das einzige, das
    //    der Zuschauer nach dem Wechsel bekommen kann - dass ScreenCaptureKit
    //    nach updateConfiguration von selbst ein neues liefert, ist nicht
    //    belegt. Im alten Format geht es trotzdem nie in den neuen Encoder:
    //    waehrend des Wechsels gibt es keine Sitzung, danach rechnet der Takt
    //    es einmal um (letztes_bild_angleichen), und encode_buffer weist
    //    fremde Formate ab. Frueher wurde es hier freigegeben; dann wartete
    //    der Zuschauer bis zur naechsten Aenderung, und ein Bild, das waehrend
    //    des Umstellens noch im alten Format kam, lag danach fest und wurde
    //    bei jedem Taktschlag abgewiesen. Ein Bild in der alten Farbe rechnet
    //    VideoToolbox selbst um (Anhaenge; SDR-Weiss landet bei 203 nit).
    OSType alt_fmt = g_cfg.pixelFormat;
    OSType neu_fmt = pixfmt_fuer(idx);
    int alt_aufnahme_pq = aufnahme_ist_pq(g_cfg);
    BOOL fmt_geaendert = (alt_fmt != neu_fmt);
    // Die Stromgroesse fuer den neuen Kandidaten: nativ, bei H.264 in dessen
    // Grenze eingepasst. Aendert sie sich (5K-Bildschirm, HEVC <-> H.264),
    // stellt die Aufnahme mit um, und die Strominfo nach dem SWITCH traegt
    // die neuen Masse - wie bei einem Bildschirmwechsel.
    int alt_w = g_info_w, alt_h = g_info_h, neu_w = alt_w, neu_h = alt_h;
    int nw = atomic_load(&g_strom_nativ_w), nh = atomic_load(&g_strom_nativ_h);
    if (nw > 0 && nh > 0) groesse_fuer_codec(nw, nh, idx, 1, &neu_w, &neu_h);
    BOOL groesse_geaendert = neu_w != alt_w || neu_h != alt_h;
    BOOL aufnahme_geaendert = fmt_geaendert || alt_aufnahme_pq != pq || groesse_geaendert;

    if (g_session) {
        // c) Alles, was der alte Encoder noch hat, abliefern lassen. Danach feuert
        //    kein enc_cb der alten Sitzung mehr.
        VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);

        // d) Sitzung aushaengen, dann abbauen. Ab hier verwirft encode_buffer
        //    ankommende Bilder, bis die neue Sitzung steht - gewollt.
        VTCompressionSessionRef s = g_session;
        g_session = NULL;
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
    }

    // e) Der Zuschauer wartet wieder auf ein Vollbild, das erste neue Bild wird
    //    erzwungen, und die NAL-Laenge kommt frisch aus den neuen Parametersaetzen.
    //    Das SWITCH (Typ 7) kommt NICHT hier, sondern erst, wenn die neue Sitzung
    //    steht - siehe codec_wechsel_abschliessen.
    atomic_store(&g_wait_key, 1);
    atomic_store(&g_force_key, 1);
    g_stats.nal_len = 4;

    // f) Aufnahme umstellen, falls noetig (Pixelformat, Farbe). Das geht im
    //    Betrieb (gemessen mit --formattest); weiter geht es erst, wenn die
    //    Aufnahme umgestellt hat.
    if (aufnahme_geaendert) {
        if (fmt_geaendert) {
            uint32_t a = CFSwapInt32HostToBig(alt_fmt), n = CFSwapInt32HostToBig(neu_fmt);
            logf_(@"Aufnahmeformat: %.4s -> %.4s", (char *)&a, (char *)&n);
        }
        if (alt_aufnahme_pq != pq)
            logf_(@"Aufnahme: %s -> %s%s%s", farbe_text(alt_aufnahme_pq), farbe_text(pq), pq ? " - " : "", pq ? hdr_aufnahme_text() : "");
        if (groesse_geaendert)
            logf_(@"Stromgroesse: %dx%d -> %dx%d (%s)", alt_w, alt_h, neu_w, neu_h,
                  ist_h264(idx) ? "fuer H.264 eingepasst" : "wieder nativ");
        g_cfg.pixelFormat = neu_fmt;
        aufnahme_farbe_setzen(g_cfg, pq);
        g_cfg.width = (size_t)neu_w;
        g_cfg.height = (size_t)neu_h;
        [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
            if (e) {
                // Die Aufnahme liefert weiter das alte Format - dann bleibt es
                // beim alten Codec in der alten Farbe. Der Client hat kein
                // SWITCH bekommen und muss von nichts erfahren (ausser Grund 6,
                // wenn es HDR werden sollte).
                logf_(@"Aufnahmeformat liess sich nicht umstellen: %@ - bleibe bei %s, %s", e.localizedDescription,
                      g_kandidaten[alt].name, farbe_text(alt_pq));
                dispatch_async(g_capq, ^{
                    g_cfg.pixelFormat = alt_fmt;
                    aufnahme_farbe_setzen(g_cfg, alt_aufnahme_pq);
                    g_cfg.width = (size_t)alt_w;
                    g_cfg.height = (size_t)alt_h;
                    hdr_wechsel_gescheitert(idx, pq, alt, alt_pq);
                    codec_alt_aufbauen(alt, alt_pq);
                });
                return;
            }
            dispatch_async(g_capq, ^{
                codec_wechsel_abschliessen(idx, pq, alt, alt_pq, alt_fmt, alt_aufnahme_pq, aufnahme_geaendert,
                                           alt_w, alt_h, neu_w, neu_h);
            });
        }];
    } else {
        codec_wechsel_abschliessen(idx, pq, alt, alt_pq, alt_fmt, alt_aufnahme_pq, aufnahme_geaendert, alt_w, alt_h, neu_w, neu_h);
    }
}

// Ein Codecwunsch (Nachricht 66). Die Farbe entscheidet der Host fuer den
// neuen Kandidaten selbst (hdr_soll): HDR10 nur mit HEVC 10 Bit.
static void codec_wechseln(int idx) {
    // a) Nur, was die Pruefung beim Start als vorhanden gemeldet hat.
    if (idx < 0 || idx >= (int)QC_KANDIDATEN || !g_befund[idx].vorhanden) {
        logf_(@"Codecwunsch %d abgelehnt: %@", idx,
              (idx < 0 || idx >= (int)QC_KANDIDATEN) ? @"kein solcher Kandidat" : @"auf diesem Mac nicht vorhanden");
        return;
    }
    int alt = atomic_load(&g_codec_id);
    if (g_wechsel_aktiv) {
        // Ein Wechsel laeuft schon (die Aufnahme stellt gerade um). Den Wunsch
        // merken und danach ausfuehren - nie zwei Wechsel ineinander.
        g_wechsel_wunsch = idx;
        logf_(@"Codecwunsch %s vorgemerkt, ein Wechsel laeuft noch", g_kandidaten[idx].name);
        return;
    }
    int pq = hdr_soll(idx);
    // "Laeuft bereits" gilt nur, wenn wirklich eine Sitzung laeuft. Nach einem
    // doppelten Fehlschlag (neuer Codec kaputt, alter liess sich nicht wieder
    // oeffnen) steht g_codec_id noch auf dem alten - der darf dann neu versucht werden.
    if (idx == alt && pq == atomic_load(&g_farbe_pq) && g_session) {
        logf_(@"Codecwunsch %s: laeuft bereits", g_kandidaten[idx].name);
        return;
    }
    // Ohne Aufnahme geht nichts. Eine fehlende Encoder-Sitzung ist dagegen kein
    // Hinderungsgrund: genau dann (nach dem doppelten Fehlschlag) muss der
    // Zuschauer noch auf einen dritten Kandidaten ausweichen koennen.
    if (!g_stream || !g_cfg) {
        logf_(@"Codecwunsch %s abgelehnt: keine laufende Aufnahme", g_kandidaten[idx].name);
        return;
    }
    wechsel_ausfuehren(idx, pq);
}

// ------------------------------------------------------------ Aufnahme-Teil

@interface Grabber : NSObject <SCStreamOutput, SCStreamDelegate>
@property (nonatomic) long framesIn;
@property (nonatomic) long framesSkipped;
@end

// Ein Bild in den Encoder geben. Beide Wege - frisch aufgenommen und im Takt
// nachgelegt - laufen hier zusammen, damit sie sich nicht auseinander
// entwickeln koennen.
// Der Begleitzettel reist als Zahl mit, nicht als Zeiger: die echte
// Aufnahmezeit in Mikrosekunden, nach links geschoben, in den untersten zwei
// Bits die Merker QC_BILD_*. So haengt am Bild kein Speicher, der beim
// Verwerfen oder bei einem Fehler liegen bleiben koennte.
#define QC_ZETTEL(t_cap_us, merker) ((void *)(uintptr_t)(((uint64_t)(t_cap_us) << 2) | ((merker) & 3u)))

// Stauregel. Liegt beim Zuschauer mehr als QC_BACKLOG_LIMIT (Spielmodus: ein
// Viertel) ungesendet im Kernel, geht das naechste Bild gar nicht erst in den
// Encoder - wie beim vollen Encoder: lieber ein Auslasser als eine
// Warteschlange. Der Encoder sieht dann nur weniger Bilder, jedes codierte
// hat sein Bezugsbild, und es braucht kein erzwungenes Vollbild.
// Nimmt der Zuschauer im Stau so lange gar nichts ab (stau_frist_us), gilt
// er als weg; sonst liefe fuer eine eingefrorene Gegenstelle alles weiter.
// YES = dieses Bild auslassen.

// Die Abstaende, in denen der Zuschauer abnimmt, solange mehr als
// QC_SCHUB_AB im Kernel liegt (siehe dort). Bei jedem Blick, auch ohne Stau.
// Nur unter g_send_mtx.
static void schuebe_verfolgen(int rueckstand, uint64_t gesendet, uint64_t jetzt) {
    if (rueckstand <= QC_SCHUB_AB) {
        // Kaum etwas liegt im Kernel: der Zuschauer haelt nichts zurueck, eine
        // Luecke beginnt fruehestens jetzt. Ein Schub, der den Rueckstand kurz
        // leert, beendet also nur die laufende Luecke. Bleibt es so lange so
        // wie die hoechste Frist, zaehlen alte Luecken nicht mehr.
        if (!g_schub_frei_seit) g_schub_frei_seit = jetzt;
        else if (jetzt - g_schub_frei_seit >= QC_STAU_FRIST_MAX_US) g_schub_luecke = 0;
        g_schub_zuletzt = jetzt;
        g_schub_ohne = 0;
    } else {
        g_schub_frei_seit = 0;
        int64_t abgenommen = (int64_t)(gesendet - g_schub_gesendet) - ((int64_t)rueckstand - g_schub_rueckstand);
        if (abgenommen > 0) {
            // Als Luecke zaehlt nur, was ein Blick ohne Abnahme auch gesehen
            // hat - nicht die Zeit, in der niemand hinsah (stiller Bildschirm:
            // nur der 5-s-Takt blickt, und jeder Blick sieht eine Abnahme).
            if (g_schub_zuletzt && g_schub_ohne > g_schub_zuletzt && g_schub_ohne - g_schub_zuletzt > g_schub_luecke)
                g_schub_luecke = g_schub_ohne - g_schub_zuletzt;
            g_schub_zuletzt = jetzt;
            g_schub_ohne = 0;
        } else {
            g_schub_ohne = jetzt;
        }
    }
    g_schub_gesendet = gesendet;
    g_schub_rueckstand = rueckstand;
}

// Frist im Stau: das Doppelte der laengsten Luecke zwischen zwei Abnahmen,
// mindestens QC_STAU_FRIST_US, hoechstens QC_STAU_FRIST_MAX_US.
static uint64_t stau_frist_us(void) {
    uint64_t f = 2 * g_schub_luecke;
    return f < QC_STAU_FRIST_US ? QC_STAU_FRIST_US : f > QC_STAU_FRIST_MAX_US ? QC_STAU_FRIST_MAX_US : f;
}

static BOOL stau_vor_dem_encoder(void) {
    BOOL stau = NO;
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (fd >= 0 && atomic_load(&g_vid_ready) && g_vid) {
        int rueckstand = backlog_bytes(fd);
        schuebe_verfolgen(rueckstand, g_vid->gesendet, now_us());
        if (rueckstand > (atomic_load(&g_cur_gaming) ? QC_BACKLOG_LIMIT / 4 : QC_BACKLOG_LIMIT)) {
            stau = YES;
            // Fortschritt heisst: seit dem letzten Blick hat die Gegenstelle
            // etwas abgenommen - was in den Puffer ging, weniger dem, was dort
            // jetzt mehr liegt. Am Rueckstand allein laesst sich das nicht
            // ablesen: Ton, Zwischenablage und Zeiger gehen auch im Stau
            // hinaus (send_small) und heben ihn, auch wenn die Gegenstelle die
            // ganze Zeit liest. Und so zaehlt auch ein Blick nach langer Pause
            // (stiller Bildschirm) richtig: abgenommen ist abgenommen.
            uint64_t jetzt = now_us();
            uint64_t gesendet = g_vid->gesendet;
            int64_t abgenommen = (int64_t)(gesendet - g_stau_gesendet) - ((int64_t)rueckstand - g_stau_rueckstand);
            int neu = !g_stau_seit || abgenommen > 0;
            g_stau_gesendet = gesendet;
            g_stau_rueckstand = rueckstand;
            if (neu) {
                g_stau_seit = jetzt;
            } else if (jetzt - g_stau_seit >= stau_frist_us()) {
                logf_(@"Zuschauer weg: nimmt seit %.1f s nichts mehr ab (%d Byte im Stau, laengste Luecke vorher %.1f s)",
                      (double)(jetzt - g_stau_seit) / 1e6, rueckstand, (double)g_schub_luecke / 1e6);
                atomic_store(&g_vid_ready, 0);
                atomic_store(&g_client_fd, -1);
                stream_herunterfahren_anstossen();
                close(fd);
                eingabe_abbrechen();
                qc_chan_free(g_vid);
                g_vid = NULL;
                g_stau_seit = 0;
                qc_ui_zustand_geaendert();
            }
        } else {
            g_stau_seit = 0;
        }
    }
    pthread_mutex_unlock(&g_send_mtx);
    return stau;
}

// Die Frist auch ohne neues Bild pruefen: bei stillem Bildschirm kommt nichts
// in encode_buffer an, und ein eingefrorener Zuschauer (Ton aus) bliebe sonst
// eingetragen - Aufnahme, Encoder und die Wachhalte-Zusicherung liefen fuer
// niemanden weiter. Codiert wird hier nichts.
static void stau_frist_pruefen(void) {
    (void)stau_vor_dem_encoder();
}

// YES = das Bild ging in den Encoder.
static BOOL encode_buffer(CVPixelBufferRef pb, CMTime pts, uint64_t t_cap_us, int merker) {
    if (!pb || !g_session) return NO;
    // Rund um einen Formatwechsel der Aufnahme kann noch ein Bild im alten
    // Format eintreffen. Das gehoert nicht in den neuen Encoder - verwerfen,
    // und sagen, dass es passiert ist (einmal je Format, nicht je Bild).
    // Bleibt es als letztes Bild liegen, rechnet der Takt es um
    // (letztes_bild_angleichen).
    OSType ist = CVPixelBufferGetPixelFormatType(pb);
    OSType soll = pixfmt_fuer(atomic_load(&g_codec_id));
    if (ist != soll) {
        static OSType gemeldet = 0;
        if (gemeldet != ist) {
            gemeldet = ist;
            uint32_t a = CFSwapInt32HostToBig(ist), b = CFSwapInt32HostToBig(soll);
            logf_(@"Bild im Format %.4s verworfen, Encoder erwartet %.4s", (char *)&a, (char *)&b);
        }
        return NO;
    }
    // Vor dem Abholen eines erzwungenen Vollbilds: das kommt dann mit dem
    // naechsten Bild, das wirklich codiert wird.
    if (stau_vor_dem_encoder()) { atomic_fetch_add(&g_skipped_backlog, 1); return NO; }
    NSDictionary *opts = nil;
    if (atomic_exchange(&g_force_key, 0))
        opts = @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES};
    OSStatus st = VTCompressionSessionEncodeFrame(g_session, pb, pts, kCMTimeInvalid,
                                                  (__bridge CFDictionaryRef)opts,
                                                  QC_ZETTEL(t_cap_us, merker), NULL);
    if (st == noErr) atomic_fetch_add(&g_inflight, 1);
    if (st != noErr) {
        if (!g_stats.first_err) g_stats.first_err = st;
        // Im Dienstbetrieb wuerde ein Encoder, der nach dem Wechsel kein Bild
        // annimmt, sonst stumm bleiben. Einmal je Sitzung, nicht je Bild.
        if (atomic_fetch_add(&g_enc_fehler, 1) == 0)
            logf_(@"Encoder %s nimmt Bild nicht an (%d)", g_kandidaten[atomic_load(&g_codec_id)].name, (int)st);
    }
    return st == noErr;
}

// Ein Bild in ein anderes Aufnahmeformat umrechnen, mit VideoToolbox - wie
// die Umrechnung, die der Encoder fuer die Kandidaten 1 und 2 selbst macht.
// Farbangaben (Matrix, Primaerfarben, Uebertragung) setzt VTPixelTransfer am
// Ziel passend zum Quellbild. Rueckgabe: neuer Puffer (+1) oder NULL.
static CVPixelBufferRef bild_umrechnen(CVPixelBufferRef pb, OSType fmt) {
    NSDictionary *attr = @{ (id)kCVPixelBufferIOSurfacePropertiesKey: @{} };
    CVPixelBufferRef neu = NULL;
    if (CVPixelBufferCreate(NULL, CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb), fmt,
                            (__bridge CFDictionaryRef)attr, &neu) != kCVReturnSuccess || !neu)
        return NULL;
    VTPixelTransferSessionRef ts = NULL;
    OSStatus st = VTPixelTransferSessionCreate(NULL, &ts);
    if (st == noErr) st = VTPixelTransferSessionTransferImage(ts, pb, neu);
    if (ts) { VTPixelTransferSessionInvalidate(ts); CFRelease(ts); }
    if (st != noErr) { CVPixelBufferRelease(neu); return NULL; }
    return neu;
}

// Das festgehaltene Bild in das Format des laufenden Encoders bringen, bevor
// der Takt es nachlegt. Nach einem Codecwechsel mit anderem Aufnahmeformat
// (etwa xf44 -> 420f) liegt dort noch ein Bild im alten Format: das letzte vor
// dem Umstellen oder eines, das die Aufnahme waehrend des Umstellens noch
// lieferte. encode_buffer weist es ab; ohne Umrechnung wartete der Zuschauer
// bei stillem Bildschirm auf sein Vollbild bis zur naechsten Aenderung, und
// jeder Taktschlag versuchte es erneut. Umgerechnet wird einmal, danach passt
// das Format. Kommt ein echtes Bild, ersetzt es den Behelf und geht als
// Vollbild hinaus (g_behelf) - nach einem Wechsel auf 4:4:4 soll der
// Zuschauer nicht auf einem Bild mit Farbe aus 4:2:0 weiterbauen.
// Nur auf g_capq. NO = kein Bild im passenden Format da.
static BOOL letztes_bild_angleichen(void) {
    if (!g_last_pb) return NO;
    OSType ist = CVPixelBufferGetPixelFormatType(g_last_pb);
    OSType soll = pixfmt_fuer(atomic_load(&g_codec_id));
    if (ist == soll) return YES;
    CVPixelBufferRef neu = bild_umrechnen(g_last_pb, soll);
    uint32_t a = CFSwapInt32HostToBig(ist), b = CFSwapInt32HostToBig(soll);
    if (neu) logf_(@"Takt: letztes Bild von %.4s nach %.4s umgerechnet (Aufnahmeformat gewechselt, kein neueres Bild)",
                   (char *)&a, (char *)&b);
    else     logf_(@"Takt: letztes Bild liess sich nicht von %.4s nach %.4s umrechnen - verworfen, das naechste kommt von der Aufnahme",
                   (char *)&a, (char *)&b);
    CVPixelBufferRelease(g_last_pb);
    g_last_pb = neu;
    g_behelf = neu != NULL;
    return neu != NULL;
}

// Ist fuer ein Bild zur Zeit t (Hostuhr, Sekunden) ein Schlitz frei? Wenn ja,
// ist er damit belegt. Das Raster ist fest: der naechste Schlitz liegt eine
// Bildzeit hinter dem vorigen, nicht hinter diesem Bild - sonst wuerde ein
// Strom mit 148 Bildern je Sekunde auf jedes zweite Bild halbiert statt auf
// 120 gedeckelt. Nach einer Pause faengt das Raster bei diesem Bild neu an.
// Ein Zehntel Bildzeit zu frueh ist noch recht (Zitter der Aufnahme).
static int schlitz_frei(double t) {
    int fps = atomic_load(&g_cur_fps);
    if (fps <= 0) return 1;
    double T = 1.0 / (double)fps;
    if (t + 0.1 * T < g_schlitz) return 0;
    g_schlitz = (t > g_schlitz + T) ? t + T : g_schlitz + T;
    return 1;
}

// Der Takt. Laeuft auf derselben Warteschlange wie die Aufnahme, kommt ihr
// also nie in die Quere. Kam seit dem letzten Schlag ein echtes Bild, passiert
// hier nichts - nachgelegt wird nur, was sonst ausfallen wuerde.
// Ohne feste Bildrate reicht er nur nach, was beim Zuschauer noch fehlt
// (g_bild_offen): das letzte Bild einer Bewegung, das als zu schnell, im Stau
// oder bei vollem Encoder wegfiel - danach ist der Bildschirm still, und es
// kaeme sonst nie -, und fuer einen neuen Zuschauer das zuletzt gesehene.
static void fixed_tick(void) {
    int testbild = atomic_load(&g_testbild);
    int fest = atomic_load(&g_cur_fixed);
    int wartet = atomic_load(&g_wait_key);      // Zuschauer ohne erstes Vollbild
    if (!testbild && !fest && !wartet && !atomic_load(&g_bild_offen)) return;
    // Ohne Zuschauer wird nichts nachgelegt. Sonst laeuft der Encoder mit
    // voller Rate fuer niemanden - gemessen: 24 % Last im Leerlauf.
    if (atomic_load(&g_client_fd) < 0) return;
    if (!g_session) return;
    if (!testbild && !g_last_pb) return;
    int fps = atomic_load(&g_cur_fps);
    if (fps <= 0) return;

    CMTime now = CMClockGetTime(CMClockGetHostTimeClock());
    double seit = CMTimeGetSeconds(CMTimeSubtract(now, g_last_pts));
    if (seit < 0.9 / (double)fps) return;
    // Haengt der Encoder noch an frueheren Bildern, wird nichts nachgelegt -
    // ein Auslasser ist billiger als eine wachsende Warteschlange.
    if (atomic_load(&g_inflight) >= QC_ENCODER_OFFEN_MAX) return;
    // Nach einem Codecwechsel mit anderem Aufnahmeformat: das letzte Bild
    // einmal umrechnen, statt es bei jedem Schlag abweisen zu lassen.
    if (!testbild && !letztes_bild_angleichen()) return;

    // Zeitstempel muss immer vorwaerts gehen, sonst weist der Encoder das Bild ab.
    if (CMTIME_COMPARE_INLINE(now, <=, g_last_pts))
        now = CMTimeAdd(g_last_pts, CMTimeMake(1, (int32_t)fps));

    if (testbild) {
        // Benchmark: das naechste Bild der Schleife, als echtes Bild mit
        // seiner Entstehungszeit = jetzt. Kein Nachlegen, kein Raster.
        CVPixelBufferRef pb = qc_testbild_naechstes();
        if (pb) {
            encode_buffer(pb, now, cmtime_us(now), QC_BILD_TESTBILD);
            g_last_pts = now;
        }
        return;
    }
    if (!fest) {
        // Waehrend einer Bewegung kommt alle paar Millisekunden ein neueres
        // Bild; erst wenn eine Bildzeit lang keins kam, ist dieses das letzte.
        // Es ist ein echtes Bild, also belegt es einen Schlitz im Raster.
        if (now_us() < g_last_ankunft_us + 1000000ull / (uint64_t)fps) return;
        // Wer auf sein erstes Vollbild wartet, bekommt eins - aber eins zur
        // Zeit: steckt schon ein Bild im Encoder, kommt es gleich an.
        if (wartet && atomic_load(&g_inflight) > 0) return;
        // Im Stau gar nicht erst versuchen: sonst zaehlte jeder Schlag als
        // ausgelassenes Bild ("Stau" im 5-s-Protokoll, bis zu fps je
        // Sekunde), obwohl nur ein einziges Bild wartet. Die Frist prueft
        // dieser Blick mit.
        if (stau_vor_dem_encoder()) return;
        double schlitz_vorher = g_schlitz;
        if (!schlitz_frei(CMTimeGetSeconds(now))) return;
        if (wartet) atomic_store(&g_force_key, 1);
        // Als wiederholt gekennzeichnet: die Aufnahmezeit ist die echte, das
        // Bild aber alt - fuer einen neuen Zuschauer bei stillem Bildschirm
        // womoeglich Minuten. In der Latenzmessung (Host: Encoderzeit, Client:
        // Gesamtverzoegerung) staende sonst dieses Alter als Verzoegerung.
        if (encode_buffer(g_last_pb, now, g_last_cap_us, QC_BILD_WIEDERHOLT)) {
            atomic_store(&g_bild_offen, 0);
            atomic_fetch_add(&g_nachgereicht, 1);
            g_last_pts = now;
        } else {
            // Im Stau geblieben: der Schlitz gehoert dann dem naechsten
            // echten Bild, nicht diesem Versuch.
            g_schlitz = schlitz_vorher;
        }
        return;
    }
    // Wichtig: der Zettel traegt die ECHTE Aufnahmezeit des wiederholten
    // Bildes, nicht die Nachlegezeit. Sonst sieht die Messung auf der anderen
    // Seite aus, als waere jedes Bild blitzschnell unterwegs gewesen.
    if (encode_buffer(g_last_pb, now, g_last_cap_us, QC_BILD_WIEDERHOLT)) atomic_store(&g_bild_offen, 0);
    g_last_pts = now;
    // Das Raster bleibt unberuehrt: es zaehlt nur echte Bilder. Schoebe ein
    // nachgelegtes Bild es weiter, fiele ein echtes kurz danach als "zu
    // schnell" weg - Inhalt verloere gegen Wiederholung. Gemessen: 75 statt
    // 120 Bilder je Sekunde, ein Drittel davon Wiederholungen.
    atomic_fetch_add(&g_repeats, 1);
}

@implementation Grabber

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    CFArrayRef arr = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (arr && CFArrayGetCount(arr) > 0) {
        CFDictionaryRef d = CFArrayGetValueAtIndex(arr, 0);
        CFNumberRef st = CFDictionaryGetValue(d, (__bridge CFStringRef)SCStreamFrameInfoStatus);
        int v = -1;
        if (st) CFNumberGetValue(st, kCFNumberIntType, &v);
        if (v != SCFrameStatusComplete) { self.framesSkipped++; return; }
    }
    CVImageBufferRef pb = CMSampleBufferGetImageBuffer(sb);
    if (atomic_exchange(&g_formattest, 0)) logf_(@"   angekommen: %@", anhaenge_text(pb));
    if (!pb) return;
    // Nach dem Start und nach jedem Farbwechsel: was die Aufnahme wirklich
    // liefert (Format, Primaerfarben, Transfer, Matrix).
    if (atomic_exchange(&g_anhaenge_loggen, 0))
        logf_(@"Aufnahme: erstes Bild %@ (Encoder %s)", anhaenge_text(pb), farbe_text(atomic_load(&g_farbe_pq)));
    // Ohne Encoder nur waehrend eines Codecwechsels festhalten. Sonst ist der
    // Strom abgebaut, und ein Nachzuegler der anhaltenden Aufnahme (stopCapture
    // mit Frist) laege nach dem Aufraeumen wieder in g_last_pb - bis zur
    // naechsten Sitzung.
    if (!g_session && !g_wechsel_aktiv) return;
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
    if (!CMTIME_IS_VALID(pts)) pts = CMClockGetTime(CMClockGetHostTimeClock());

    // Die echte Aufnahmezeit wird VOR jeder Korrektur festgehalten. Die
    // Korrektur unten dient nur dem Encoder, der aufsteigende Zeitstempel
    // verlangt - sie darf die Messung nicht beschoenigen.
    uint64_t t_cap = cmtime_us(pts);
    if (g_last_pb && CMTIME_COMPARE_INLINE(pts, <=, g_last_pts))
        pts = CMTimeAdd(g_last_pts, CMTimeMake(1, 1000));

    // Das Bild wird in jedem Fall festgehalten - auch eines, das gleich
    // wegfaellt: legt der Takt spaeter nach, soll es das neueste sein. Der
    // Puffer gehoert der Aufnahme; ohne eigene Referenz wird er unter uns
    // wiederverwendet.
    if (g_last_pb != pb) {
        CVPixelBufferRef alt = g_last_pb;
        g_last_pb = CVPixelBufferRetain(pb);
        if (alt) CVPixelBufferRelease(alt);
    }
    g_last_cap_us = t_cap;
    g_last_ankunft_us = now_us();
    // Noch nicht beim Zuschauer. Geht es gleich in den Encoder, ist der
    // Merker wieder weg; sonst reicht der Takt es nach. Auch waehrend des
    // Testbilds und eines Codecwechsels: danach soll das Neueste kommen.
    atomic_store(&g_bild_offen, 1);
    if (!g_session) return;
    // Testbild an: die Aufnahme laeuft weiter (der Wechsel zurueck soll
    // keine Sekunde kosten), aber ihre Bilder gehen nicht in den Encoder.
    if (atomic_load(&g_testbild)) return;
    // Das erste echte Bild nach einem umgerechneten Behelf wird ein Vollbild
    // (siehe letztes_bild_angleichen). Faellt es gleich als zu schnell weg,
    // bleibt die Forderung stehen, und der Takt reicht es als Vollbild nach.
    if (g_behelf && CVPixelBufferGetPixelFormatType(pb) == pixfmt_fuer(atomic_load(&g_codec_id))) {
        g_behelf = 0;
        atomic_store(&g_force_key, 1);
    }
    self.framesIn++;

    // Schneller als die Zielrate: weg damit. Und ist der Encoder noch mit
    // aelteren Bildern beschaeftigt, ebenfalls - lieber ein Auslasser als
    // eine Warteschlange, die jede Bewegung um Sekunden verspaetet.
    if (!schlitz_frei(CMTimeGetSeconds(pts))) { atomic_fetch_add(&g_zu_schnell, 1); return; }
    if (atomic_load(&g_inflight) >= QC_ENCODER_OFFEN_MAX) { atomic_fetch_add(&g_enc_stau, 1); return; }

    if (encode_buffer(pb, pts, t_cap, 0)) atomic_store(&g_bild_offen, 0);
    g_last_pts = pts;
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    logf_(@"Aufnahme gestoppt: %@ (Code %ld)", error.localizedDescription, (long)error.code);
    // Ohne Lebenslauf-Warteschlange (Pruefmodus --formattest) gibt es keinen
    // eingetragenen Strom und nichts wiederherzustellen.
    if (!g_lifeq) return;
    // Austragen auf der Lebenslauf-Warteschlange, der g_stream gehoert - und
    // nur, wenn es der laufende Strom ist: der spaete Bescheid eines alten
    // darf einen inzwischen neu gestarteten nicht austragen.
    dispatch_async(g_lifeq, ^{
        if (stream != g_stream) return;
        // Ohne Aufnahme darf der Takt nicht weiterlaufen, sonst sendet der Host
        // bis in alle Ewigkeit dasselbe eingefrorene Bild.
        atomic_store(&g_cur_fixed, 0);
        dispatch_sync(g_capq, ^{
            if (g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }
        });
        // Solange keine Aufnahme laeuft, gibt es auch keinen Codecwechsel.
        stream_setzen(nil);
        // Ohne Zuschauer gibt es nichts wiederherzustellen - der naechste baut
        // neu auf. Einer, der gerade eingetragen wird, hat sein Hochfahren
        // aber schon hinter sich und braucht die Wiederherstellung.
        if (!zuschauer_braucht_strom()) return;
        hoststatus_senden(1);
        // Nicht sofort: direkt nach dem Abriss ist der Bildschirm meist noch
        // weg. Auf g_lifeq, nicht auf der Hauptwarteschlange: der Weg zum
        // neuen Strom laeuft ganz dort (und der Pruefstand dreht keine Run-Loop).
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), g_lifeq, ^{
            aufnahme_wiederherstellen();
        });
    });
}

@end

// ------------------------------------------------------------------- Helfer
//
// Bildschirmwahl (bildschirm.h). Der Host streamt genau einen Bildschirm: in
// der Automatik den Hauptbildschirm, mit Wunsch (Nachricht 70, --display) den
// gewuenschten - faellt der weg, den Hauptbildschirm als Ausweichplatz, bis er
// zurueck ist. Neu bewertet wird beim Start, bei jedem Wunsch, bei jeder
// Aenderung der Bildschirmkonfiguration (Rueckruf von CoreGraphics, entprellt)
// und bei jedem Aufnahmestart. Zwei Monitore am Mac haben das noetig
// gemacht: ScreenCaptureKit sortiert seine Liste nicht stabil, und die
// displayID gilt nur je Sitzung - der Schluessel ist die stabile Kennung.
// Alles hier gehoert der Lebenslauf-Warteschlange g_lifeq (beim Start vor den
// Warteschlangen: dem Hauptfaden); Encoderarbeit laeuft per dispatch_sync auf
// g_capq. Nichts davon laeuft auf der Hauptwarteschlange - der Pruefstand
// dreht keine Run-Loop -, Wiederholungen gehen per dispatch_after auf g_lifeq.

// Der gewuenschte Bildschirm an seiner stabilen Kennung, nil = Automatik.
static NSString *g_display_wunsch = nil;
// --display N: nach der ersten Wahl wird die Kennung des Bildschirms am
// Listenplatz N zum Wunsch fuer diesen Lauf (bildschirm.txt bleibt). -1 = keiner.
static int g_display_pin = -1;
// Der gestreamte Bildschirm; ohne Strom das Ziel des naechsten Starts. 0 = keiner.
static CGDirectDisplayID g_display_id = 0;
static QCBildschirm *g_display_aktuell = nil;
// Die zuletzt geholte Liste.
static NSArray<QCBildschirm *> *g_bildschirme = nil;
// --out BxH: feste Stromgroesse, hat Vorrang vor der Bildschirmgroesse.
static int g_out_fest_w = 0, g_out_fest_h = 0;
// Nachricht 12, fertig kodiert: geschrieben auf g_lifeq, gelesen von jedem,
// der sie senden will (die Begruessung im Annahmefaden). Eigene kleine
// Sperre, ein Blatt - darunter wird keine andere genommen.
static pthread_mutex_t g_bildschirm_mtx = PTHREAD_MUTEX_INITIALIZER;
static NSData *g_bildschirm_payload = nil;
// Ein Bildschirmwechsel wartet auf einen laufenden Codecwechsel: seit wann
// (Hostuhr in us, 0 = wartet nicht), auf welchen Bildschirm, aus welchem
// Anlass (fuer den Grund in der Zeile), und ob die Wiederholung schon
// eingereiht ist (genau eine, nicht eine je Wunsch). Nur auf g_lifeq. Das
// Warten endet mit dem vollzogenen Wechsel - oder sobald eine Bewertung
// keinen Wechsel mehr braucht (das Ziel ist wieder der gestreamte
// Bildschirm, der Zuschauer ist weg): bliebe es stehen, bekaeme kein
// spaeterer Wunsch mehr Antwort und Zeile, und ein spaeterer Wechsel wuerde
// nach 5 s erzwungen, mitten in einem frischen Codecwechsel.
static uint64_t g_bildschirm_wartet_seit = 0;
static CGDirectDisplayID g_bildschirm_wartet_ziel = 0;
static int g_bildschirm_wartet_anlass = 0;
static BOOL g_bildschirm_wartet_eingereiht = NO;

// Anlass einer Neubewertung. WARTEN ist die Wiederholung eines wartenden
// Wechsels: sie antwortet keinem Wunsch und schreibt keine Wahl-Zeile.
enum { QC_ANLASS_START = 0, QC_ANLASS_KONFIG = 1, QC_ANLASS_WUNSCH = 2, QC_ANLASS_WARTEN = 3 };
#define QC_BILDSCHIRM_ENTPRELLEN_MS 300
#define QC_BILDSCHIRM_WARTEN_MS     200
#define QC_BILDSCHIRM_WARTEN_MAX_US (5ull * 1000000ull)

static QCBildschirm *bildschirm_neu_bewerten(int anlass);

// Strombau, ersetzbar (Pruefstand: FakeStrom). Liefert den fertig
// konfigurierten, noch nicht gestarteten Strom, sonst nil mit Fehlertext.
typedef SCStream *(*qc_strom_fabrik)(QCBildschirm *b, SCStreamConfiguration *cfg, id grab,
                                     dispatch_queue_t q, NSString **fehler);

static SCStream *strom_bauen_sck(QCBildschirm *b, SCStreamConfiguration *cfg, id grab,
                                 dispatch_queue_t q, NSString **fehler) {
    if (!b.sc) { if (fehler) *fehler = @"kein SCDisplay zum Bildschirm"; return nil; }
    SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:b.sc excludingWindows:@[]];
    SCStream *st = [[SCStream alloc] initWithFilter:f configuration:cfg delegate:grab];
    NSError *err = nil;
    if (![st addStreamOutput:grab type:SCStreamOutputTypeScreen sampleHandlerQueue:q error:&err]) {
        if (fehler) *fehler = [NSString stringWithFormat:@"Ausgabe nicht anmeldbar (%@)", err.localizedDescription];
        return nil;
    }
    qc_audio_attach(st, audio_cb);
    return st;
}

static qc_strom_fabrik g_strom_fabrik = strom_bauen_sck;
// Nur der Pruefstand ruft das (er bindet main.m ein).
__attribute__((unused)) static void qc_strom_fabrik_setzen(qc_strom_fabrik f) { g_strom_fabrik = f ?: strom_bauen_sck; }

// Stromgroesse fuer einen Bildschirm, bei jedem Strombau: --out hat Vorrang,
// sonst nativ (Produktentscheidung 2026-09-29, keine Halbierung mehr) - genau
// die Pixel des Modus, bei HiDPI die Pixel hinter den Punkten ("wie
// 1920x1080" auf einem 4K-Panel: 3840x2160), ein skalierter HiDPI-Modus
// hoechstens mit der Aufloesung des Panels; immer gerade
// (qc_stromgroesse_nativ). Ohne Codec: fuer H.264 kommt das Einpassen dazu
// (stromgroesse_codec).
static void stromgroesse_fuer(QCBildschirm *b, int *w, int *h) {
    int ow = g_out_fest_w, oh = g_out_fest_h;
    if (ow > 0 && oh > 0) {
        *w = ow & ~1;
        *h = oh & ~1;
        return;
    }
    qc_stromgroesse_nativ(b.w, b.h, b.punkte_w, b.punkte_h, b.nativ_w, b.nativ_h, w, h);
}

// Stromgroesse fuer einen Bildschirm und Kandidat idx: stromgroesse_fuer,
// bei H.264 eingepasst (groesse_fuer_codec). Der Pruefstand rechnet damit.
__attribute__((unused)) static void stromgroesse_codec(QCBildschirm *b, int idx, int melden, int *w, int *h) {
    int nw = 0, nh = 0;
    stromgroesse_fuer(b, &nw, &nh);
    groesse_fuer_codec(nw, nh, idx, melden, w, h);
}

// Fuer die Protokollzeilen: " (nativ <B>x<H>, fuer H.264 eingepasst)", wenn
// der laufende Strom fuer H.264 kleiner ist als nativ.
static NSString *eingepasst_text(void) {
    int nw = atomic_load(&g_strom_nativ_w), nh = atomic_load(&g_strom_nativ_h);
    return (nw > 0 && (nw != g_info_w || nh != g_info_h)) ? [NSString stringWithFormat:@" (nativ %dx%d, fuer H.264 eingepasst)", nw, nh] : @"";
}

static NSString *bildschirm_text(QCBildschirm *b) {
    if (!b) return @"keiner";
    return [NSString stringWithFormat:@"Kennung %u (%@) \"%@\"", b.displayID, b.kennung, b.name ?: @""];
}

// Die Pixel des Bildschirms fuer die Protokollzeilen, bei HiDPI mit den
// Punkten ("3840x2160 (HiDPI, wie 1920x1080)"), und die native Aufloesung,
// wenn der Modus mehr Pixel hat als das Panel.
static NSString *bildschirm_masse(QCBildschirm *b) {
    NSMutableString *t = [NSMutableString stringWithFormat:@"%zux%zu", b.w, b.h];
    BOOL hidpi = b.punkte_w && b.punkte_h && (b.punkte_w != b.w || b.punkte_h != b.h);
    BOOL ueber_panel = b.nativ_w && b.nativ_h && (b.w > b.nativ_w || b.h > b.nativ_h);
    if (hidpi || ueber_panel) {
        [t appendString:@" ("];
        if (hidpi) [t appendFormat:@"HiDPI, wie %zux%zu", b.punkte_w, b.punkte_h];
        if (ueber_panel) [t appendFormat:@"%@Panel %zux%zu", hidpi ? @", " : @"", b.nativ_w, b.nativ_h];
        [t appendString:@")"];
    }
    return t;
}

// Die Breite eines Bildschirms in Punkten; ohne Punkte (Attrappen) gilt ein
// Punkt je Pixel.
static size_t zeiger_punkte(QCBildschirm *b) { return b.punkte_w ? b.punkte_w : b.w; }

// Der Zeiger im Massstab des Stroms (zeiger.h): Bildpunkte des Stroms je
// Punkt des Bildschirms (punkte breit) - bei HiDPI nativ gestreamt 2, bei
// --out oder fuer H.264 eingepasst kleiner. So kommt die Form in
// Bildpunkten des Stroms, wie der Client sie erwartet (zeigerbild.rs). Eine
// Zeile, wenn er sich aendert. Aus jedem Faden (Strombau auf g_lifeq,
// Codecwechsel auf g_capq).
static void zeiger_massstab_nachfuehren(size_t punkte, int strom_w) {
    atomic_store(&g_zeiger_punkte, punkte);
    double m = (punkte && strom_w > 0) ? (double)strom_w / (double)punkte : 1.0;
    double alt = qc_zeiger_massstab();
    qc_zeiger_massstab_setzen(m);
    if (fabs(qc_zeiger_massstab() - alt) >= 0.0005)
        logf_(@"Zeigerform: Massstab %.2f (Strom %d Bildpunkte breit, Bildschirm %zu Punkte)", qc_zeiger_massstab(), strom_w, punkte);
}

// Nachricht 12 aus dem Stand neu kodieren. Auf g_lifeq (oder vor den Warteschlangen).
static void bildschirm_zustand_nachfuehren(void) {
    NSData *p = qc_bildschirme_kodieren(g_bildschirme, g_display_wunsch, g_display_id);
    pthread_mutex_lock(&g_bildschirm_mtx);
    g_bildschirm_payload = p;
    pthread_mutex_unlock(&g_bildschirm_mtx);
}

// Kann der aufgenommene Bildschirm HDR (EDR-Kopfraum potentiell ueber 1.0)?
// Auf g_lifeq. Eine Aenderung - anderer Bildschirm, HDR am Bildschirm an
// oder aus - gibt einem gescheiterten HDR-Wechsel eine neue Chance. anlass:
// im laufenden Strom (sonst NULL) - dann eine Zeile und neu entscheiden.
static void quelle_setzen(BOOL hdr, double edr, const char *anlass) {
    int neu = hdr ? 1 : 0;
    int alt = atomic_exchange(&g_quelle_hdr, neu);
    if (alt == neu) return;
    atomic_store(&g_hdr_gescheitert, 0);
    if (!anlass) return;
    logf_(@"Bildschirm des Hosts: HDR %s (EDR-Kopfraum %.2f) - %s", neu ? "an" : "aus", edr, anlass);
    if (g_capq && atomic_load(&g_client_fd) >= 0) dispatch_async(g_capq, ^{ hdr_neu_entscheiden("HDR am Host"); });
}

// Der EDR-Vorrat (bildschirm.m, im 5-s-Takt auf dem Hauptfaden aufgefrischt)
// fuer den gestreamten Bildschirm: so faellt HDR an/aus am Host auch dann
// auf, wenn CoreGraphics keine Konfigurationsaenderung meldet. Auf g_lifeq.
static void quelle_pruefen(void) {
    if (!g_stream || !g_display_id) return;
    double p = 0, a = 0;
    if (!qc_bildschirm_edr(g_display_id, &p, &a)) return;
    quelle_setzen(p > 1.0, p, "EDR-Kopfraum geaendert");
}

// Die Liste an den Zuschauer. Aus jedem Faden, nie unter g_send_mtx.
static void bildschirme_senden(void) {
    pthread_mutex_lock(&g_bildschirm_mtx);
    NSData *p = g_bildschirm_payload;
    pthread_mutex_unlock(&g_bildschirm_mtx);
    if (!p) p = qc_bildschirme_kodieren(nil, nil, 0);
    send_small(QC_MSG_BILDSCHIRME, p.bytes, p.length);
}

// Strom fuer den Bildschirm ziel bauen und starten; kein Strom darf laufen
// (g_stream nil). Auf g_lifeq. Die Stromgroesse folgt dem Bildschirm: aendert
// sie sich (oder gibt es noch keinen Encoder), entsteht der Encoder neu, auf
// g_capq wie beim Codecwechsel, und g_cfg bekommt die neuen Masse; ein
// liegengebliebenes Bild in der alten Groesse ginge sonst ungeprueft in den
// neuen Encoder. wechsel: der Zuschauer erfaehrt es in jedem Fall - SWITCH
// mit dem laufenden Codec, damit er den Decoder neu baut, dann INFO mit den
// Massen; danach kommt das erste Bild als Vollbild. Bei anderer Groesse mit
// laufendem Encoder (Wiederherstellung auf einem anderen Bildschirm) ebenso.
// Beides auf g_capq, damit zwischen SWITCH und dem Vollbild-Merker kein Bild
// hinausgeht; CompleteFrames holt vorher die Bilder aus dem Encoder, deren
// Rueckruf sonst hinter dem SWITCH landen koennte. Das letzte Bild des alten
// Bildschirms faellt bei jedem Wechsel weg, auch bei gleicher Groesse: der
// Takt reichte es sonst als erstes Vollbild des neuen nach, bevor
// ScreenCaptureKit das erste echte liefert (ein Bild vom falschen Bildschirm).
// YES = laeuft (g_stream gesetzt, Maus folgt); NO = nicht gestartet, mit Zeile.
static BOOL strom_fuer_bildschirm_starten(QCBildschirm *ziel, BOOL wechsel) {
    int nw = 0, nh = 0;
    stromgroesse_fuer(ziel, &nw, &nh);
    __block int w = nw, h = nh;
    // Kann der neue Bildschirm HDR? Davon haengt die Farbe des Stroms ab.
    quelle_setzen(ziel.hdr, ziel.edr_potentiell, NULL);
    __block BOOL enc = YES;
    dispatch_sync(g_capq, ^{
        int codec = atomic_load(&g_codec_id);
        // Nativ, bei H.264 in dessen Grenze eingepasst - mit dem Kandidaten,
        // der gerade gilt, auf g_capq gelesen wie beim Codecwechsel.
        groesse_fuer_codec(nw, nh, codec, 1, &w, &h);
        atomic_store(&g_strom_nativ_w, nw);
        atomic_store(&g_strom_nativ_h, nh);
        int fps = atomic_load(&g_cur_fps), mbit = atomic_load(&g_cur_mbit);
        BOOL neue_groesse = (w != g_info_w || h != g_info_h);
        // Die Farbe fuer diesen Bildschirm und den laufenden Zuschauer: ohne
        // dessen IN_ANZEIGE (neuer Zuschauer) immer SDR. Weicht sie von der
        // laufenden ab, entsteht der Encoder neu und der Zuschauer erfaehrt es.
        int pq = hdr_soll(codec);
        BOOL farbe_neu = pq != atomic_load(&g_farbe_pq);
        BOOL ansagen = wechsel || ((neue_groesse || farbe_neu) && g_session != NULL);
        if (neue_groesse || wechsel) {
            if (g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }
            g_behelf = 0;
            atomic_store(&g_bild_offen, 0);
        }
        if (neue_groesse || !g_session || farbe_neu) {
            if (g_session) {
                VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);
                VTCompressionSessionRef s = g_session;
                g_session = NULL;
                VTCompressionSessionInvalidate(s);
                CFRelease(s);
            }
            g_info_w = w;
            g_info_h = h;
            enc = encoder_start(codec, w, h, fps, mbit, pq);
            if (!enc && pq) {
                // HDR10 geht nicht: in SDR weiter, Grund 6.
                atomic_store(&g_hdr_gescheitert, 1);
                logf_(@"HDR10 laesst sich nicht starten - SDR (Grund 6), bis sich die Lage aendert");
                pq = 0;
                enc = encoder_start(codec, w, h, fps, mbit, 0);
            }
            if (enc && farbe_neu && ansagen) g_farbe_gewechselt_us = now_us();
            // Das Testbild folgt der Stromgroesse und der Farbe.
            if (enc && atomic_load(&g_testbild)) qc_testbild_start(w, h, pixfmt_fuer(codec), pq);
        } else if (ansagen) {
            VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);
        }
        if (g_cfg) {
            g_cfg.width = (size_t)w;
            g_cfg.height = (size_t)h;
            g_cfg.pixelFormat = pixfmt_fuer(codec);
            aufnahme_farbe_setzen(g_cfg, atomic_load(&g_farbe_pq));
            if (fps > 0) g_cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
        }
        atomic_store(&g_anhaenge_loggen, 1);
        // Ohne Encoder keine Ansage: der Client baute sonst den Decoder um
        // und bekaeme kein Bild, bis die Wiederherstellung greift.
        if (ansagen && enc) {
            switch_senden(codec);
            strominfo_senden();
        }
        atomic_store(&g_wait_key, 1);
        atomic_store(&g_force_key, 1);
        g_stats.nal_len = 4;
    });
    if (!enc) { logf_(@"Encoder laesst sich nicht starten"); return NO; }

    NSString *fehler = nil;
    SCStream *st = g_strom_fabrik(ziel, g_cfg, g_grab, g_capq, &fehler);
    if (!st) { logf_(@"Aufnahme: %@", fehler ?: @"Strom laesst sich nicht bauen"); return NO; }
    // Auf den Start hier warten, auf der Lebenslauf-Warteschlange, statt den
    // Strom im Rueckruf einzutragen: dort liefe es an Auf- und Abbau vorbei.
    // Ein Abbau saehe keinen Strom und liesse ihn ohne Zuschauer laufen; ein
    // neuer Zuschauer baute einen zweiten auf - zwei Stroeme am selben
    // Tonabgriff, doppelter Ton.
    __block BOOL gestartet = NO;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [st startCaptureWithCompletionHandler:^(NSError *e) {
        if (e) logf_(@"Aufnahme: Start misslungen (%@, Code %ld)", e.localizedDescription, (long)e.code);
        else gestartet = YES;
        dispatch_semaphore_signal(sem);
    }];
    if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC)) != 0) {
        // Kaeme der Start doch noch, liefe dieser Strom unbemerkt neben dem
        // naechsten - mit Ton am selben Abgriff.
        logf_(@"Aufnahme: Start meldet sich nicht");
        [st stopCaptureWithCompletionHandler:^(NSError *x) { (void)x; }];
        return NO;
    }
    if (!gestartet) return NO;               // nach dem Signal gelesen, also fertig geschrieben
    // Waehrend des Starts gegangen: sein Abbau ist schon durch oder steht
    // hinter uns an und kennt diesen Strom nicht - also selbst anhalten.
    if (!zuschauer_braucht_strom()) {
        dispatch_semaphore_t halt = dispatch_semaphore_create(0);
        [st stopCaptureWithCompletionHandler:^(NSError *x) { (void)x; dispatch_semaphore_signal(halt); }];
        dispatch_semaphore_wait(halt, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
        logf_(@"Aufnahme nicht gestartet: kein Zuschauer mehr");
        return NO;
    }
    stream_setzen(st);
    g_display_id = ziel.displayID;
    g_display_aktuell = ziel;
    // Die Maus folgt dem Bild - immer, nicht nur beim Programmstart. Sie
    // rechnet in Punkten (CGDisplayBounds) und ist deshalb von der
    // Stromgroesse unabhaengig; der Zeiger nicht: er bekommt den Massstab.
    atomic_store(&g_input_display, ziel.displayID);
    zeiger_massstab_nachfuehren(zeiger_punkte(ziel), w);
    atomic_store(&g_cur_fixed, atomic_load(&g_fixed_gewollt));
    // Solange gestreamt wird, darf der Bildschirm nicht einschlafen.
    if (g_wach == kIOPMNullAssertionID)
        IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
                                    CFSTR("QuadChroma streamt diesen Bildschirm"), &g_wach);
    return YES;
}

// Das Warten ist zu Ende (Wechsel vollzogen oder hinfaellig). Auf g_lifeq.
static void bildschirm_warten_beenden(void) {
    g_bildschirm_wartet_seit = 0;
    g_bildschirm_wartet_ziel = 0;
    g_bildschirm_wartet_anlass = 0;
}

// Die Wiederholung eines wartenden Wechsels einreihen: nach
// QC_BILDSCHIRM_WARTEN_MS auf g_lifeq neu bewerten - genau einmal, gleich
// wie viele Wuensche waehrend des Wartens kommen. Auf g_lifeq.
static void bildschirm_warten_einreihen(void) {
    if (g_bildschirm_wartet_eingereiht) return;
    g_bildschirm_wartet_eingereiht = YES;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, QC_BILDSCHIRM_WARTEN_MS * NSEC_PER_MSEC), g_lifeq, ^{
        g_bildschirm_wartet_eingereiht = NO;
        bildschirm_neu_bewerten(QC_ANLASS_WARTEN);
    });
}

// Der Wechsel im laufenden Betrieb (Spezifikation 1.4): alten Strom anhalten,
// Groesse und Encoder nachziehen, SWITCH und INFO, neuer Strom, Maus, Liste,
// Zeile. Auf g_lifeq. anlass: was die Bewertung ausgeloest hat; ein Wunsch
// bekommt seine Antwort (die Liste) schon beim Warten, jeder Wunsch.
static void bildschirm_wechseln(QCBildschirm *neu, NSString *grund, int anlass) {
    // Laeuft ein Codecwechsel (auf g_capq), wartet der Bildschirmwechsel: nach
    // QC_BILDSCHIRM_WARTEN_MS noch einmal bewerten, hoechstens 5 s ab dem
    // Beginn dieses Wartens, dann trotzdem.
    __block int codec_aktiv = 0;
    dispatch_sync(g_capq, ^{ codec_aktiv = g_wechsel_aktiv; });
    uint64_t jetzt = now_us();
    if (codec_aktiv) {
        if (!g_bildschirm_wartet_seit) g_bildschirm_wartet_seit = jetzt;
        if (g_bildschirm_wartet_ziel != neu.displayID) {
            // Ein neues Ziel: Zeile, und sein Anlass bestimmt den Grund. Hat
            // die Wiederholung selbst ein anderes Ziel gefunden (die Liste
            // hat sich waehrend des Wartens geaendert), gilt das wie eine
            // Konfigurationsaenderung.
            g_bildschirm_wartet_ziel = neu.displayID;
            g_bildschirm_wartet_anlass = anlass == QC_ANLASS_WARTEN ? QC_ANLASS_KONFIG : anlass;
            logf_(@"Bildschirmwechsel auf %@ wartet auf den laufenden Codecwechsel", bildschirm_text(neu));
        }
        // Die Antwort auf einen Wunsch geht gleich hinaus, mit dem noch
        // gestreamten Bildschirm; nach dem Wechsel kommt die Liste erneut.
        if (anlass == QC_ANLASS_WUNSCH) { bildschirm_zustand_nachfuehren(); bildschirme_senden(); }
        if (jetzt - g_bildschirm_wartet_seit < QC_BILDSCHIRM_WARTEN_MAX_US) {
            bildschirm_warten_einreihen();
            return;
        }
        logf_(@"Codecwechsel nach 5 s nicht fertig - Bildschirmwechsel trotzdem");
    }
    bildschirm_warten_beenden();
    QCBildschirm *alt = g_display_aktuell;
    // 1. Alten Strom anhalten. Ab hier gibt es keinen Strom: ein Codecwunsch
    //    wird abgelehnt, ein spaeter Bescheid des alten Stroms uebergangen.
    SCStream *st = g_stream;
    stream_setzen(nil);
    if (st) {
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [st stopCaptureWithCompletionHandler:^(NSError *e) { (void)e; dispatch_semaphore_signal(sem); }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
    }
    // 2. bis 5.: Groesse, Encoder, SWITCH, INFO, neuer Strom, Maus.
    if (!strom_fuer_bildschirm_starten(neu, YES)) {
        // Wie bei einem Bildschirmverlust: Hoststatus 1, Wiederholung. Das
        // Ziel bleibt, die Liste meldet den Stand. Ohne Zuschauer bleibt es
        // beim Ziel; der naechste Zuschauer baut den Strom neu.
        BOOL zuschauer = zuschauer_braucht_strom();
        logf_(@"Bildschirmwechsel: %@ -> %@ (%@) nicht vollzogen%@", bildschirm_text(alt), bildschirm_text(neu), grund,
              zuschauer ? @" - Ausweichweg wie beim Bildschirmverlust" : @"");
        g_display_id = neu.displayID;
        g_display_aktuell = neu;
        bildschirm_zustand_nachfuehren();
        bildschirme_senden();
        if (zuschauer) {
            hoststatus_senden(1);
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), g_lifeq, ^{ aufnahme_wiederherstellen(); });
        }
        return;
    }
    // 6. und 7.: Liste und Zeile.
    bildschirm_zustand_nachfuehren();
    bildschirme_senden();
    logf_(@"Bildschirmwechsel: %@ -> %@ (%@)", bildschirm_text(alt), bildschirm_text(neu), grund);
}

// Neu bewerten (Spezifikation 1.2): Liste holen, Wahl (Wunsch, sonst
// Hauptbildschirm, sonst der erste), bei Abweichung vom gestreamten
// Bildschirm wechseln, sonst nur die Liste melden, wenn sie sich geaendert
// hat oder ein Wunsch die Antwort verlangt. Ohne Strom werden nur Ziel und
// Liste nachgefuehrt; der Strom laeuft erst mit dem naechsten Zuschauer. Auf
// g_lifeq, oder beim Start vor den Warteschlangen. Rueckgabe: das Ziel, nil =
// kein Bildschirm. Steht kein Wechsel (mehr) an, endet hier ein etwaiges
// Warten auf den Codecwechsel.
static QCBildschirm *bildschirm_neu_bewerten(int anlass) {
    BOOL wunsch = anlass == QC_ANLASS_WUNSCH;           // bekommt Antwort und Zeilen
    BOOL wiederholung = anlass == QC_ANLASS_WARTEN;     // weder Antwort noch Wahl-Zeile
    NSArray<QCBildschirm *> *liste = qc_bildschirme_holen();
    if (!liste) {
        logf_(@"Bildschirme nicht abrufbar - der Stand bleibt");
        if (wunsch) bildschirme_senden();
        if (wiederholung && g_bildschirm_wartet_seit) bildschirm_warten_einreihen();   // das Warten geht weiter
        return g_stream ? g_display_aktuell : nil;
    }
    // --display N: einmal, nach der ersten Wahl. Der Wunsch fuer diesen Lauf
    // ist dann die Kennung des Bildschirms an Platz N; bildschirm.txt bleibt.
    if (g_display_pin >= 0) {
        if ((NSUInteger)g_display_pin < liste.count) {
            g_display_wunsch = liste[g_display_pin].kennung;
            logf_(@"Bildschirm --display %d: %@ gilt als Wunsch fuer diesen Lauf",
                  g_display_pin, bildschirm_text(liste[g_display_pin]));
        } else {
            logf_(@"Bildschirm %d nicht verfuegbar (%lu in der Liste) - %@", g_display_pin,
                  (unsigned long)liste.count, g_display_wunsch ? @"der Wunsch aus bildschirm.txt gilt" : @"Automatik");
        }
        g_display_pin = -1;
    }
    int art = QC_WAHL_KEINER;
    QCBildschirm *ziel = qc_bildschirm_wahl(liste, g_display_wunsch, &art);
    BOOL liste_neu = ![liste isEqualToArray:g_bildschirme];
    g_bildschirme = liste;
    if (wunsch && g_display_wunsch && art != QC_WAHL_WUNSCH)
        logf_(@"Bildschirmwunsch: %@ - nicht angeschlossen, Ausweichplatz %@", g_display_wunsch, bildschirm_text(ziel));
    if (!ziel) {
        bildschirm_warten_beenden();
        if (g_stream) {
            // ScreenCaptureKit meldet gerade keinen Bildschirm, der Strom
            // laeuft aber: der Stand bleibt, sonst baute die naechste
            // Bewertung mit gefuellter Liste den Strom auf demselben
            // Bildschirm unnoetig neu. Faellt der Bildschirm wirklich weg,
            // endet der Strom (didStopWithError), und die Wiederherstellung
            // bewertet neu.
            if (liste_neu) logf_(@"Kein Bildschirm in der Liste - der laufende Strom bleibt");
            bildschirm_zustand_nachfuehren();
            if (liste_neu || wunsch) bildschirme_senden();
            return g_display_aktuell;
        }
        BOOL ziel_neu = g_display_id != 0;
        g_display_id = 0;
        g_display_aktuell = nil;
        bildschirm_zustand_nachfuehren();
        if (liste_neu || ziel_neu || wunsch) bildschirme_senden();
        return nil;
    }
    BOOL ziel_neu = ziel.displayID != g_display_id;
    // Die Zeile je Ziel einmal: nicht fuer die Wiederholungen des Wartens und
    // nicht erneut fuer ein Ziel, auf das der Wechsel schon wartet.
    if (ziel_neu && !wiederholung && ziel.displayID != g_bildschirm_wartet_ziel)
        logf_(@"Bildschirm gewaehlt: %@, %zux%zu Pixel, %.0f Hz%@%@", bildschirm_text(ziel), ziel.w, ziel.h, ziel.hz,
              ziel.haupt ? @" (Hauptbildschirm)" : @"",
              (g_display_wunsch && art != QC_WAHL_WUNSCH) ? @" - Ausweichplatz" : @"");
    if (g_stream && ziel_neu) {
        // Der Grund: bei der Wiederholung eines wartenden Wechsels der
        // Anlass, aus dem er wartet.
        int wirksam = wiederholung ? g_bildschirm_wartet_anlass : anlass;
        NSString *grund;
        if (art == QC_WAHL_WUNSCH)
            grund = wirksam == QC_ANLASS_WUNSCH ? @"Wunsch des Zuschauers" : @"zurueck zum gewuenschten Bildschirm";
        else if (g_display_wunsch)
            grund = @"Ausweichplatz";
        else
            // Ohne Wunsch ist das Ziel der Hauptbildschirm - entweder, weil
            // der Zuschauer gerade auf Automatik gestellt hat, oder weil
            // der Hauptbildschirm ein anderer wurde.
            grund = wirksam == QC_ANLASS_WUNSCH ? @"Wunsch des Zuschauers: Automatik" : @"Hauptbildschirm gewechselt";
        bildschirm_wechseln(ziel, grund, anlass);
        return ziel;
    }
    // Derselbe Bildschirm in einer anderen Aufloesung (Modus umgestellt,
    // HiDPI an oder aus): der Strom folgt, denn gestreamt wird nativ - wie
    // ein Wechsel auf denselben Bildschirm, mit neuem Encoder, SWITCH, INFO
    // und Vollbild. Frueher blieb die Groesse, und ScreenCaptureKit skalierte
    // den neuen Modus in die alte.
    // Verglichen wird mit dem Kandidaten, der laeuft (bei H.264 die
    // eingepasste Groesse), auf g_capq gelesen - Kandidat und Stromgroesse
    // aendert ein Codecwechsel dort in einem Zug.
    if (g_stream && g_capq) {
        int nw = 0, nh = 0;
        stromgroesse_fuer(ziel, &nw, &nh);
        __block int sw = 0, sh = 0, iw = 0, ih = 0;
        dispatch_sync(g_capq, ^{
            groesse_fuer_codec(nw, nh, atomic_load(&g_codec_id), 0, &sw, &sh);
            iw = g_info_w;
            ih = g_info_h;
        });
        if (sw != iw || sh != ih) {
            bildschirm_wechseln(ziel, [NSString stringWithFormat:@"Aufloesung geaendert: Strom %dx%d -> %dx%d",
                                       iw, ih, sw, sh], anlass);
            return ziel;
        }
        // Dieselbe Stromgroesse, womoeglich aber eine andere native (ein
        // anderer Modus, fuer H.264 auf dieselbe Groesse eingepasst): ein
        // spaeterer Codecwechsel rechnet mit der neuen.
        if (ziel.displayID == g_display_id) {
            atomic_store(&g_strom_nativ_w, nw);
            atomic_store(&g_strom_nativ_h, nh);
        }
    }
    // Kein Wechsel (mehr) noetig - auch ein wartender ist damit hinfaellig.
    bildschirm_warten_beenden();
    if (!g_stream) g_display_id = ziel.displayID;       // Ziel des naechsten Starts
    g_display_aktuell = ziel;
    // Derselbe Bildschirm, aber vielleicht HDR an oder aus - oder ein Modus
    // mit denselben Pixeln und anderen Punkten (HiDPI "wie 1920x1080" gegen
    // 3840x2160 nativ): dann nur ein anderer Zeigermassstab.
    if (g_stream && ziel.displayID == g_display_id) {
        quelle_setzen(ziel.hdr, ziel.edr_potentiell, "Bildschirmkonfiguration");
        zeiger_massstab_nachfuehren(zeiger_punkte(ziel), g_info_w);
    }
    bildschirm_zustand_nachfuehren();
    if (liste_neu || ziel_neu || wunsch) bildschirme_senden();
    return ziel;
}

// Ein Wunsch vom Eingabekanal (Nachricht 70). Auf g_lifeq. Gilt hostweit,
// ueberschreibt --display und den gemerkten Wunsch, wird in bildschirm.txt
// gemerkt; die Antwort ist die Liste - auch bei einem Bildschirm, der gerade
// nicht angeschlossen ist (Ausweichplatz, bis er da ist).
static void bildschirm_wunsch_setzen(NSString *kennung) {
    g_display_wunsch = kennung;
    g_display_pin = -1;
    int r = qc_bildschirm_wunsch_speichern(kennung);
    logf_(@"Bildschirmwunsch: %@%@", kennung ?: @"Automatik",
          r == 0 ? @"" : @" - bildschirm.txt liess sich nicht schreiben");
    bildschirm_neu_bewerten(QC_ANLASS_WUNSCH);
}

// Aenderung der Bildschirmkonfiguration (Monitor dazu oder weg, Hauptbildschirm
// umgestellt): entprellt, dann auf g_lifeq neu bewerten. Aus jedem Faden.
static _Atomic unsigned g_bildschirm_folge = 0;
static void bildschirm_konfiguration_geaendert(void) {
    unsigned f = atomic_fetch_add(&g_bildschirm_folge, 1) + 1;
    if (!g_lifeq) return;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, QC_BILDSCHIRM_ENTPRELLEN_MS * NSEC_PER_MSEC), g_lifeq, ^{
        if (atomic_load(&g_bildschirm_folge) != f) return;   // ein spaeterer Aufruf hat die Frist neu gesetzt
        bildschirm_neu_bewerten(QC_ANLASS_KONFIG);
    });
}

// Rueckruf von CoreGraphics, auf dem Hauptfaden (die Dienstschleife dreht
// dort die Run-Loop). Er kommt vor und nach jeder Aenderung; gezaehlt wird
// nur das Danach. Die Namen (AppKit) werden gleich hier aufgefrischt.
static void bildschirm_rueckruf(CGDirectDisplayID d, CGDisplayChangeSummaryFlags flags, void *ctx) {
    (void)d; (void)ctx;
    if (flags & kCGDisplayBeginConfigurationFlag) return;
    qc_bildschirm_namen_auffrischen();
    bildschirm_konfiguration_geaendert();
}

static void list_displays(void) {
    NSArray<QCBildschirm *> *liste = qc_bildschirme_holen();
    if (!liste) { logf_(@"Inhalte nicht abrufbar"); return; }
    int i = 0;
    for (QCBildschirm *b in liste) {
        int sw = 0, sh = 0, hw = 0, hh = 0;
        stromgroesse_fuer(b, &sw, &sh);
        qc_h264_einpassen(sw, sh, g_h264_grenze, &hw, &hh);
        logf_(@"Display %d: id=%u, Kennung %@, Name \"%@\", %@ Pixel, %.0f Hz%@, Strom %dx%d%@, EDR-Kopfraum %.2f (jetzt %.2f)%@",
              i++, b.displayID, b.kennung, b.name, bildschirm_masse(b), b.hz, b.haupt ? @"  (Hauptbildschirm)" : @"",
              sw, sh, (hw != sw || hh != sh) ? [NSString stringWithFormat:@" (mit H.264 %dx%d)", hw, hh] : @"",
              b.edr_potentiell, b.edr_aktuell, b.hdr ? @" - HDR" : @"");
    }
}

// Aufnahme nach einem Bildschirmverlust neu aufbauen. Auf g_lifeq; kommt
// verzoegert ueber dispatch_after aus didStopWithError, einem gescheiterten
// Wechsel oder von hier (alle 3 s, solange kein Bildschirm da ist). Der
// Zielbildschirm wird neu bewertet: ein Ausweichplatz kann eine andere
// Groesse haben, dann bekommt der Zuschauer SWITCH und INFO.
static void wiederherstellen_spaeter(void) {
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), g_lifeq, ^{ aufnahme_wiederherstellen(); });
}

static void aufnahme_wiederherstellen(void) {
    if (g_stream) return;
    // Nur solange jemand zuschaut (oder gerade eingetragen wird). Geht der
    // Zuschauer waehrend des Wartens, endet die Kette hier.
    if (!zuschauer_braucht_strom()) return;
    QCBildschirm *ziel = bildschirm_neu_bewerten(QC_ANLASS_START);
    if (!ziel) {
        if (!g_kein_bildschirm_gemeldet) {
            g_kein_bildschirm_gemeldet = 1;
            logf_(@"Kein Bildschirm - warte auf seine Rueckkehr");
        }
        wiederherstellen_spaeter();
        return;
    }
    g_kein_bildschirm_gemeldet = 0;
    // Kam der Zuschauer ohne Aufnahme herein (keine Freigabe), kennt er nur
    // Ersatzmasse: er bekommt SWITCH und INFO wie bei einem Bildschirmwechsel.
    BOOL ansagen = atomic_load(&g_ohne_aufnahme) != 0;
    if (!strom_fuer_bildschirm_starten(ziel, ansagen)) {
        if (zuschauer_braucht_strom()) { logf_(@"Aufnahme: neuer Versuch in 3 s"); wiederherstellen_spaeter(); }
        return;
    }
    atomic_store(&g_ohne_aufnahme, 0);
    bildschirm_zustand_nachfuehren();
    bildschirme_senden();
    hoststatus_senden(0);
    logf_(@"Aufnahme wiederhergestellt: Bildschirm %@, %.0f Hz, %@ -> %dx%d%@", bildschirm_masse(ziel), ziel.hz, bildschirm_text(ziel),
          g_info_w, g_info_h, eingepasst_text());
}

// Aufnahme und Encoder fuer einen Zuschauer aufbauen. Wird vom Faden der
// Verbindung gerufen, bevor er die Begruessung schickt - die Verbindung
// dauert dadurch einen Moment laenger, dafuer arbeitet der Mac ohne
// Zuschauer gar nicht.
static BOOL stream_hochfahren_sync(void) {
    __block BOOL ok = NO;
    dispatch_sync(g_lifeq, ^{
        if (g_stream) { ok = YES; return; }
        QCBildschirm *ziel = bildschirm_neu_bewerten(QC_ANLASS_START);
        if (!ziel) { logf_(@"Kein Bildschirm - Aufnahme kann nicht starten"); return; }
        if (!strom_fuer_bildschirm_starten(ziel, NO)) return;
        bildschirm_zustand_nachfuehren();
        logf_(@"Aufnahme gestartet: Bildschirm %@, %.0f Hz, %@ -> %dx%d%@%@",
              bildschirm_masse(ziel), ziel.hz, bildschirm_text(ziel), g_info_w, g_info_h, eingepasst_text(),
              ziel.hdr ? [NSString stringWithFormat:@", Bildschirm HDR-faehig (EDR-Kopfraum %.2f)", ziel.edr_potentiell] : @"");
        ok = YES;
    });
    return ok;
}

static BOOL zuschauer_braucht_strom(void) {
    if (atomic_load(&g_anmeldend) > 0) return YES;
    return atomic_load(&g_client_fd) >= 0;
}

// Der Zuschauer ist weg: Aufnahme anhalten, Encoder abbauen, Bildschirm
// freigeben. Darf aus jedem Faden angestossen werden, auch unter g_send_mtx -
// hier wird nur eingereiht, gearbeitet wird spaeter und nacheinander.
// Ob wirklich niemand mehr da ist, entscheidet erst der Block: ist inzwischen
// wieder jemand da oder unterwegs, bleibt alles stehen. Wer unterwegs aufgibt,
// stoesst den Abbau selbst noch einmal an.
static void stream_herunterfahren_anstossen(void) {
    if (!g_lifeq) return;
    dispatch_async(g_lifeq, ^{
        if (zuschauer_braucht_strom()) return;
        if (!g_stream && !g_session) return;
        if (g_stream) {
            SCStream *st = g_stream;
            stream_setzen(nil);
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [st stopCaptureWithCompletionHandler:^(NSError *e) { (void)e; dispatch_semaphore_signal(sem); }];
            dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
        }
        dispatch_sync(g_capq, ^{
            if (g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }
            if (g_session) {
                VTCompressionSessionRef alt = g_session;
                g_session = NULL;
                VTCompressionSessionCompleteFrames(alt, kCMTimeInvalid);
                VTCompressionSessionInvalidate(alt);
                CFRelease(alt);
            }
            // Ohne Encoder ist nichts auf der Leitung: die naechste
            // Begruessung sagt SDR, bis ein Encoder anderes meldet.
            atomic_store(&g_farbe_pq, 0);
        });
        atomic_store(&g_cur_fixed, 0);
        if (g_wach != kIOPMNullAssertionID) { IOPMAssertionRelease(g_wach); g_wach = kIOPMNullAssertionID; }
        // Ein Testbild ueberlebt den Zuschauer nicht - die naechste Sitzung
        // faengt mit dem Bildschirm an.
        if (atomic_load(&g_testbild) && g_capq) {
            dispatch_sync(g_capq, ^{ atomic_store(&g_testbild, 0); qc_testbild_stop(); });
            logf_(@"Testbild aus (Zuschauer weg)");
        }
        logf_(@"Aufnahme angehalten: kein Zuschauer");
    });
}

// --------------------------------------------------------- Dienst und Zustand
//
// Annahme, Eingabe und Bekanntgabe. Ist der Bildport belegt (ein anderes
// Programm), endet der Host nicht mehr (frueher Exit 9): der Zustand geht an
// die Oberflaeche (qc_zustand_port_belegt), und der Zustandstakt versucht es
// alle 3 s erneut. Die Bekanntgabe startet erst mit der Annahme - sonst
// fuehrte sie Clients zu dem fremden Programm.
static int g_dienst_port = 9001;
static CGDirectDisplayID g_dienst_display = 0;
static _Atomic int g_dienst_laeuft = 0;

// Die serielle Warteschlange des Zustandstakts. Auf ihr laufen auch
// Anhalten und Fortsetzen der Freigabe (qc_dienst_anhalten, ..._fortsetzen):
// Lauschen, Anhalten und der Wiederholversuch des Takts kommen so nie
// gleichzeitig.
static dispatch_queue_t g_zustandq = NULL;

static dispatch_queue_t zustandq(void) {
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{ g_zustandq = dispatch_queue_create("tech.quadchroma.zustand", DISPATCH_QUEUE_SERIAL); });
    return g_zustandq;
}

static BOOL dienst_starten(void) {
    if (atomic_load(&g_dienst_laeuft) || atomic_load(&g_freigabe_aus)) return YES;
    int belegt = atomic_load(&g_port_belegt);
    if (start_server(g_dienst_port, !belegt) < 0) {
        if (!belegt) {
            atomic_store(&g_port_belegt, g_dienst_port);
            logf_(@"Port %d ist belegt (anderes Programm?) - der Host wartet und versucht es alle 3 s erneut", g_dienst_port);
            qc_ui_zustand_geaendert();
        }
        return NO;
    }
    start_input_server(g_dienst_port + 1, g_dienst_display);
    start_beacon(g_dienst_port);
    atomic_store(&g_dienst_laeuft, 1);
    if (atomic_exchange(&g_port_belegt, 0)) {
        logf_(@"Port %d wieder frei - der Dienst laeuft", g_dienst_port);
        qc_ui_zustand_geaendert();
    }
    return YES;
}

// Alle 3 s, auf einer eigenen seriellen Warteschlange (nie der Main Queue):
// Freigaben nachsehen (Spezifikation 7.4 - der Host endet ohne sie nicht mehr,
// die Oberflaeche zeigt den Stand), einen belegten Port erneut versuchen.
// Eine Aenderung bekommt eine Zeile und geht an die Oberflaeche. Ein
// Zuschauer, der ohne Aufnahme wartet, bekommt seine Aufnahme ueber die
// Wiederherstellung, die ebenfalls alle 3 s nachfragt.
static int g_zustand_bild = -1, g_zustand_ax = -1;      // nur im Zustandstakt

static void zustand_takt(void) {
    int b = qc_zustand_bildschirmfreigabe(), a = qc_zustand_bedienungshilfen();
    BOOL anders = NO;
    if (b != g_zustand_bild) {
        if (g_zustand_bild >= 0) logf_(@"Bildschirmaufnahme-Freigabe jetzt %@", b ? @"erteilt" : @"entzogen");
        g_zustand_bild = b;
        anders = YES;
    }
    if (a != g_zustand_ax) {
        if (g_zustand_ax >= 0) logf_(@"Bedienungshilfen-Freigabe jetzt %@", a ? @"erteilt" : @"entzogen");
        g_zustand_ax = a;
        anders = YES;
    }
    if (!atomic_load(&g_dienst_laeuft) && !atomic_load(&g_freigabe_aus)) dienst_starten();
    if (anders) qc_ui_zustand_geaendert();
}

static dispatch_source_t g_zustand_quelle = NULL;

static void zustand_takt_starten(void) {
    if (g_zustand_quelle) return;
    g_zustand_quelle = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, zustandq());
    if (!g_zustand_quelle) return;
    dispatch_source_set_timer(g_zustand_quelle, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC),
                              3 * NSEC_PER_SEC, NSEC_PER_SEC / 4);
    dispatch_source_set_event_handler(g_zustand_quelle, ^{ zustand_takt(); });
    dispatch_resume(g_zustand_quelle);
}

// Bedienungshilfen (Spezifikation 7.4): beim ersten Start einmal nachfragen -
// das System zeigt dann seinen Dialog. Danach steht der Stand nur noch im
// Menue; sonst kaeme der Dialog bei jedem Anmelden wieder. Gemerkt als leere
// Datei host-bedienungshilfen-gefragt im Ablageordner.
static void bedienungshilfen_einmal_fragen(void) {
    if (AXIsProcessTrusted()) return;
    char pfad[1200];
    if (qc_config_path("host-bedienungshilfen-gefragt", pfad, sizeof pfad) != 0) return;
    int fd = open(pfad, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) return;                 // schon gefragt (oder nicht schreibbar: dann lieber nie als jedes Mal)
    close(fd);
    AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{ (__bridge id)kAXTrustedCheckOptionPrompt: @YES });
    logf_(@"Bedienungshilfen fehlen - einmal nachgefragt (Systemdialog)");
}

// Zeilen aus zugang.c (Migration, Entfernen, Passwort) ins Protokoll. Sie
// tragen Geraetenamen in UTF-8 - also nicht ueber "%s" (siehe utf8).
static void zugang_zeile(const char *z) { logf_(@"%@", utf8(z)); }

// Stromgroesse ohne Bildschirmliste (keine Freigabe, kein Monitor beim Start):
// aus dem Hauptbildschirm wie stromgroesse_fuer (nativ), ohne Modus
// 1920x1080. Sie steht nur in der Begruessung eines Zuschauers, der ohne
// Aufnahme hereinkommt; mit der Aufnahme bekommt er die echten Masse (SWITCH
// und INFO).
static void ersatzgroesse(int *w, int *h) {
    int ow = g_out_fest_w, oh = g_out_fest_h;
    if (ow > 0 && oh > 0) {
        *w = ow & ~1;
        *h = oh & ~1;
        return;
    }
    qc_bildschirm_modus m;
    if (!qc_bildschirm_modus_lesen(CGMainDisplayID(), &m) || !m.pw || !m.ph) { m.pw = 1920; m.ph = 1080; m.punkte_w = m.punkte_h = m.nativ_w = m.nativ_h = 0; }
    qc_stromgroesse_nativ(m.pw, m.ph, m.punkte_w, m.punkte_h, m.nativ_w, m.nativ_h, w, h);
}

// Wert hinter einem Schalter (versatz 1 = gleich dahinter). Fehlt er - Ende
// der Zeile oder schon der naechste Schalter -, gilt der Standard, und eine
// Zeile sagt es; frueher endete das mit einer NSRangeException.
static NSString *wert_nach(NSArray<NSString *> *args, NSString *schalter, NSInteger versatz) {
    NSInteger i = [args indexOfObject:schalter];
    if (i == NSNotFound) return nil;
    for (NSInteger k = 1; k <= versatz; k++) {
        if (i + k >= (NSInteger)args.count || [args[i + k] hasPrefix:@"--"]) {
            logf_(@"%@: Wert %ld fehlt - der Standard gilt", schalter, (long)versatz);
            return nil;
        }
    }
    return args[i + versatz];
}

// Was nicht (mehr) bekannt ist, wird nur protokolliert. --pair und --forget
// gibt es nicht mehr: neue Geraete kommen mit dem Zugangspasswort oder per
// "Zulassen" herein, entfernt wird im Menue.
static void argumente_pruefen(NSArray<NSString *> *args) {
    NSSet<NSString *> *bekannt = [NSSet setWithArray:@[ @"--serve", @"--list", @"--capture", @"--formattest",
                                                        @"--display", @"--fps", @"--mbit", @"--out", @"--fest", @"--fixed",
                                                        @"--hdr" ]];
    for (NSUInteger i = 1; i < args.count; i++) {
        NSString *a = args[i];
        // Werte, und was macOS selbst anhaengt (-psn_..., -NSDocumentRevisions...).
        if (![a hasPrefix:@"--"] || [bekannt containsObject:a]) continue;
        if ([a isEqualToString:@"--pair"] || [a isEqualToString:@"--forget"])
            logf_(@"Unbekanntes Argument %@ - uebergangen (entfallen: neue Geraete kommen mit dem Zugangspasswort "
                   "oder per Zulassen am Host herein, entfernt wird im Menue)", a);
        else
            logf_(@"Unbekanntes Argument %@ - uebergangen", a);
    }
}

// --fest (oder --fixed): feste Bildrate von Anfang an. Ohne die Angabe ist sie
// aus (MANUAL.txt: "Vorgabe ... aus"), bis der Client etwas anderes wuenscht.
// Frueher stand hier nur die erste Zuweisung im if (Klammern fehlten):
// g_fixed_gewollt war immer 1, und jeder neue Strom lief mit fester Bildrate,
// bei einem Client ohne eigene Einstellung die ganze Sitzung.
static void fest_einlesen(NSArray<NSString *> *args) {
    if ([args containsObject:@"--fest"] || [args containsObject:@"--fixed"]) {
        atomic_store(&g_cur_fixed, 1);
        atomic_store(&g_fixed_gewollt, 1);
    }
}

// ------------------------------------------------------------ Dienst-Takt
// Was im Dienstbetrieb regelmaessig anfaellt - nachgetragene Drosselzeilen,
// Stau-Frist, Statistikzeile, Auslastung (Typ 6) an den Zuschauer -, lief
// frueher in einer 5-s-Schleife in main, die dazu die Run-Loop drehte. Jetzt
// dreht [NSApp run] (eingebettet winit) die Run-Loop (Menueleiste, siehe
// menue.h), und der Takt ist ein Dispatch-Timer auf einer eigenen seriellen
// Warteschlange - nicht auf der Main Queue: send_small und stau_frist_pruefen
// nehmen g_send_mtx, und wer das Senden haelt, kann bis zu 2 s haengen
// (SO_SNDTIMEO); die Oberflaeche stuende so lange. Nur die Namen der Bildschirme (NSScreen, Hauptfaden)
// werden auf der Main Queue nachgezogen. Zwischenablage und Zeigerform fragen
// wie bisher ueber ihre eigenen Timer ab. hosttest ruft dienst_takt_starten
// direkt, mit kurzem Takt und ohne Run-Loop.
#define QC_TAKT_S 5.0
static dispatch_queue_t g_taktq = NULL;
static dispatch_source_t g_takt = NULL, g_takt_namen = NULL;
// Nur auf g_taktq angefasst:
static double g_takt_s = QC_TAKT_S;
static CFAbsoluteTime g_takt_t0 = 0;
static long g_takt_bilder = 0, g_takt_bilder2 = 0;   // Bezugspunkte fuer Zeile bzw. gemeldete Bildrate
static long long g_takt_bytes = 0;

// Eine Runde des Takts, auf g_taktq. (Einen belegten Bildport meldet und
// versucht der Kern selbst, ueber qc_zustand_port_belegt - nicht dieser Takt.)
static void dienst_takt_schritt(void) {
    drosseln_nachtragen();
    if (atomic_load(&g_client_fd) >= 0) stau_frist_pruefen();
    long f = atomic_load(&g_sent_frames);
    long long b = atomic_load(&g_sent_bytes);
    const double dt = g_takt_s;
    if (atomic_load(&g_client_fd) >= 0)
        logf_(@"[%.0f s] Bild: %ld (%.1f/s, %.1f Mbit/s) | Ton: %ld Pakete, %.0f kB, %ld verworfen | Stau: %ld | Encoder verworfen: %ld | nachgelegt: %ld | nachgereicht: %ld | zu schnell: %ld | Encoder voll: %ld",
              CFAbsoluteTimeGetCurrent() - g_takt_t0, f, (f - g_takt_bilder) / dt,
              (b - g_takt_bytes) * 8.0 / dt / 1e6,
              atomic_load(&g_audio_packets), atomic_load(&g_audio_bytes) / 1000.0,
              atomic_load(&g_audio_verworfen),
              atomic_load(&g_skipped_backlog), g_stats.dropped,
              atomic_load(&g_repeats), atomic_load(&g_nachgereicht), atomic_load(&g_zu_schnell), atomic_load(&g_enc_stau));
    g_takt_bilder = f; g_takt_bytes = b;

    // Auslastung des Hosts. Nur wenn jemand zuschaut - sonst misst
    // der Mac sich selbst ohne Zweck.
    if (atomic_load(&g_client_fd) >= 0) {
        qc_last l;
        qc_last_probe(&l);
        long n = atomic_exchange(&g_enc_n, 0);
        long long summe = atomic_exchange(&g_enc_us, 0);
        // Gedeckelt: ueber 6,5 s liefe das Feld sonst ueber und zeigte wenig.
        long long zehntel = n ? (summe / n) / 100 : 0;
        uint16_t enc_zehntel = (uint16_t)(zehntel > 65535 ? 65535 : zehntel);
        double fps_zehntel = (f - g_takt_bilder2) * 10.0 / dt;
        uint16_t host_fps_zehntel = (uint16_t)(fps_zehntel > 65535 ? 65535 : fps_zehntel);
        g_takt_bilder2 = f;

        uint8_t buf[28] = {0};
        buf[0] = 1;                       // Fassung
        memcpy(buf + 2,  &l.cpu_promille, 2);
        memcpy(buf + 4,  &l.cpu_eigen_promille, 2);
        memcpy(buf + 6,  &l.gpu_promille, 2);
        memcpy(buf + 8,  &l.druck, 2);
        memcpy(buf + 10, &l.ram_benutzt_mb, 4);
        memcpy(buf + 14, &l.ram_gesamt_mb, 4);
        memcpy(buf + 18, &l.eigen_mb, 4);
        memcpy(buf + 22, &enc_zehntel, 2);
        memcpy(buf + 24, &host_fps_zehntel, 2);
        send_small(QC_MSG_LAST, buf, sizeof buf);
    }
}

// Startet den Takt (alle `sekunden`) und das Nachziehen der Bildschirmnamen.
// Die erste Runde kommt nach einem vollen Takt, wie frueher nach dem ersten
// runUntilDate.
static void dienst_takt_starten(double sekunden) {
    if (!g_taktq) g_taktq = dispatch_queue_create("tech.quadchroma.takt", DISPATCH_QUEUE_SERIAL);
    dispatch_sync(g_taktq, ^{
        g_takt_s = sekunden;
        g_takt_t0 = CFAbsoluteTimeGetCurrent();
        g_takt_bilder = g_takt_bilder2 = atomic_load(&g_sent_frames);
        g_takt_bytes = atomic_load(&g_sent_bytes);
    });
    uint64_t iv = (uint64_t)(sekunden * NSEC_PER_SEC);
    g_takt = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, g_taktq);
    if (g_takt) {
        dispatch_source_set_timer(g_takt, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv), iv, iv / 20);
        dispatch_source_set_event_handler(g_takt, ^{ @autoreleasepool { dienst_takt_schritt(); } });
        dispatch_resume(g_takt);
    }
    // Namen der Bildschirme (AppKit, Hauptfaden) nachziehen: NSScreen kennt
    // einen neuen Monitor womoeglich erst kurz nach dem Rueckruf.
    g_takt_namen = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    if (g_takt_namen) {
        dispatch_source_set_timer(g_takt_namen, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv), iv, iv / 20);
        // Dabei auch der EDR-Kopfraum: HDR am gestreamten Bildschirm an oder
        // aus faellt so spaetestens nach einem Takt auf (quelle_pruefen).
        dispatch_source_set_event_handler(g_takt_namen, ^{ @autoreleasepool {
            qc_bildschirm_namen_auffrischen();
            if (g_lifeq) dispatch_async(g_lifeq, ^{ quelle_pruefen(); });
        } });
        dispatch_resume(g_takt_namen);
    }
}

// Nur der Pruefstand ruft das (er bindet main.m ein). Kehrt zurueck, wenn
// keine Runde mehr laeuft.
__attribute__((unused)) static void dienst_takt_anhalten(void) {
    if (g_takt) { dispatch_source_cancel(g_takt); g_takt = NULL; }
    if (g_takt_namen) { dispatch_source_cancel(g_takt_namen); g_takt_namen = NULL; }
    if (g_taktq) dispatch_sync(g_taktq, ^{});
}

// Beenden (Menue, Cmd+Q, SIGTERM/SIGINT, Abmelden; menue.m ruft es ausserhalb
// der Main Queue): der Zuschauer bekommt als letzte Nachricht den Abschied
// (Typ 13, Grund 0 "App beendet"), dann werden Bild- und Eingabekanal
// geschlossen. Sein Client zeigt "QuadChroma wurde auf <Host> beendet."
// (MsgHostQuit, englisch "QuadChroma was closed on <Host>.") und verbindet sich
// nicht von selbst neu; ein aelterer sieht nur das Ende der Verbindung und
// versucht es wie bisher erneut. Bewusst nicht Typ 10: den deutet der Client
// als "ein anderes Geraet hat die Sitzung uebernommen".
// Den Zuschauer mit dem Abschied (Typ 13, grund) trennen: Bild- und
// Eingabekanal zu. Rueckgabe -1 = es gab keinen, 1 = gemeldet, 0 = der
// Abschied kam nicht an; fp bekommt seinen Fingerabdruck. vorher laeuft
// unter g_send_mtx, bevor der Zuschauer gelesen wird (Anhalten: der Merker
// fuer alle, die gerade hereinkommen).
static int zuschauer_verabschieden(uint8_t grund, char fp[24], void (^vorher)(void)) {
    int gemeldet = 0;
    pthread_mutex_lock(&g_send_mtx);
    if (vorher) vorher();
    int alt = atomic_exchange(&g_client_fd, -1);
    atomic_store(&g_vid_ready, 0);
    if (alt >= 0) {
        if (g_vid) {
            qc_fingerprint(g_vid_peer, fp);
            gemeldet = host_ende_senden(alt, grund);
        }
        // shutdown weckt auch einen Faden, der gerade auf diesem Socket liest;
        // der Abschied liegt schon im Kernel und geht vor dem FIN hinaus.
        shutdown(alt, SHUT_RDWR);
        close(alt);
    }
    eingabe_abbrechen();
    qc_chan_free(g_vid);
    g_vid = NULL;
    pthread_mutex_unlock(&g_send_mtx);
    return alt >= 0 ? gemeldet : -1;
}

static void host_abschied(void) {
    char fp[24] = {0};
    int r = zuschauer_verabschieden(QC_HOST_ENDE_BEENDET, fp, nil);
    if (r >= 0)
        logf_(@"Beenden: Verbindung zum Zuschauer %s geschlossen%s", fp,
              r ? " (Abschied gemeldet)" : " - der Abschied kam nicht an");
    logf_(@"Host beendet");
}

// Freigabe aus (qc_dienst_anhalten), auf g_zustandq: der Zuschauer bekommt
// den Abschied mit Grund 1, Aufnahme und Encoder gehen ab, Bild- und
// Eingabeport schliessen, die Bekanntgabe verstummt. Alles andere - Zugang,
// Oberflaeche, Warteschlangen, Waechter - bleibt fuer das Wiedereinschalten.
static void dienst_lauschen_beenden(void) {
    char fp[24] = {0};
    int r = zuschauer_verabschieden(QC_HOST_ENDE_FREIGABE_AUS, fp, ^{ atomic_store(&g_freigabe_aus, 1); });
    if (r >= 0) {
        logf_(@"Freigabe aus: Verbindung zum Zuschauer %s geschlossen%s", fp,
              r ? " (Abschied gemeldet)" : " - der Abschied kam nicht an");
        stream_herunterfahren_anstossen();
    }
    qc_annahme_stoppen(g_annahme_bild);
    qc_annahme_stoppen(g_annahme_eingabe);
    g_annahme_bild = g_annahme_eingabe = NULL;
    atomic_fetch_add(&g_bekanntgabe_gen, 1);
    atomic_store(&g_dienst_laeuft, 0);
    atomic_store(&g_port_belegt, 0);
    logf_(@"Freigabe aus: Bild-, Eingabe- und Bekanntgabeport zu");
    qc_ui_zustand_geaendert();
}

static void ui_protokoll(NSString *zeile) {
    logf_(@"%@", zeile);
}

// Schwache Standardfassung der Oberflaeche (wie die qc_ui_*-Rueckrufe in
// zugang.h): der Host bindet menue.m und bekommt deren Fassung; Pruefstaende,
// die main.m ohne menue.m einbinden (hosttest), bauen auch so.
__attribute__((weak)) void qc_oberflaeche_starten(const qc_oberflaeche_cfg *cfg) { (void)cfg; }

// ------------------------------------------------------------ Start
// Die Einstiege (dienst.h): main in start.m fuer die eigene App, der
// Rust-Client fuer den eingebetteten Dienst. Hier stehen Dienst und
// Werkzeuge mit ihrem gemeinsamen Vorlauf.

// Ab hier hat qc_dienst_starten etwas gestartet (Zugang, Warteschlangen,
// Annahme); jeder weitere Aufruf liefert QC_DIENST_DOPPELT.
static _Atomic int g_dienst_gestartet = 0;

// Das Protokoll einmal je Prozess oeffnen (anhaengen).
static void protokoll_oeffnen(void) {
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{
        pthread_mutex_lock(&g_log_mtx);
        if (!g_log) log_oeffnen("a");
        pthread_mutex_unlock(&g_log_mtx);
    });
}

// argv als Liste wie [[NSProcessInfo processInfo] arguments]; ohne argv nur
// ein Programmname (eingebettet: keine Schalter).
static NSArray<NSString *> *argumente_aus(int argc, const char *const argv[]) {
    NSMutableArray<NSString *> *a = [NSMutableArray array];
    for (int i = 0; argv && i < argc; i++) [a addObject:utf8(argv[i])];
    if (!a.count) [a addObject:@"quadchroma-host"];
    return a;
}

// Eigener dauerhafter Schluessel. Er entsteht beim ersten Start und bleibt
// danach liegen, damit Gegenstellen den Host wiedererkennen. 0 = geladen.
static int schluessel_laden(void) {
    int schluessel = qc_identity_load(g_id_priv, g_id_pub);
    if (schluessel == -2) {
        // Nie still einen neuen anlegen: das waere ein anderer Host (neue
        // ID), den kein Client kennt - jeder verlangte seinen Ausweis (Bit 0
        // in Nachricht 3) und braeuchte das Passwort noch einmal.
        logf_(@"Schluessel host.key ist vorhanden, aber nicht lesbar oder beschaedigt - Abbruch. "
               "Ein neuer Schluessel waere eine neue Identitaet; die Datei erst entfernen, "
               "wenn alle Clients diesen Host mit dem Passwort neu erlauben sollen.");
        return QC_DIENST_DATEI;
    }
    if (schluessel != 0) {
        logf_(@"Schluessel konnte nicht angelegt werden - Abbruch.");
        return QC_DIENST_DATEI;
    }
    return 0;
}

// Die Schalter fuer Kenner. outPath und seconds nur mit --capture.
static void optionen_lesen(NSArray<NSString *> *args, int *fps, int *mbit, int *outW, int *outH, int *port,
                           double *seconds, NSString **outPath) {
    *fps = 120; *mbit = 150; *outW = 0; *outH = 0; *port = 9001;
    *seconds = 0;
    *outPath = nil;
    fest_einlesen(args);
    NSString *w;
    if ([args containsObject:@"--capture"]) {
        // Wie die Voreinstellungen im Makefile (make capture).
        *seconds = (w = wert_nach(args, @"--capture", 1)) ? w.doubleValue : 10;
        *outPath = wert_nach(args, @"--capture", 2) ?: @"/tmp/qc.hevc";
    }
    NSInteger srvIdx = [args indexOfObject:@"--serve"];
    if (srvIdx != NSNotFound && srvIdx + 1 < (NSInteger)args.count && ![args[srvIdx + 1] hasPrefix:@"--"])
        *port = [args[srvIdx + 1] intValue];
    if ((w = wert_nach(args, @"--display", 1))) g_display_pin = w.intValue;
    if ((w = wert_nach(args, @"--fps", 1))) *fps = w.intValue;
    if ((w = wert_nach(args, @"--mbit", 1))) *mbit = w.intValue;
    if ((w = wert_nach(args, @"--out", 1))) sscanf(w.UTF8String, "%dx%d", outW, outH);
    if (*fps <= 0) { logf_(@"--fps %d ungueltig - 120", *fps); *fps = 120; }
    if (*mbit <= 0) { logf_(@"--mbit %d ungueltig - 150", *mbit); *mbit = 150; }
    if (*outW > 0 && *outH > 0) { g_out_fest_w = *outW; g_out_fest_h = *outH; }
}

// Die Datei fuer --capture. 0 = keine verlangt oder offen.
static int ausgabe_oeffnen(NSString *outPath) {
    if (!outPath) return 0;
    g_stats.out = fopen(outPath.UTF8String, "wb");
    if (!g_stats.out) { logf_(@"Ausgabedatei nicht schreibbar: %@", outPath); return QC_DIENST_DATEI; }
    return 0;
}

// Der gemerkte Wunsch (bildschirm.txt), dann die erste Wahl - auf dem
// Hauptfaden, vor den Warteschlangen. --display N ueberstimmt den gemerkten
// Wunsch fuer diesen Lauf, ohne die Datei zu aendern; eine kaputte Datei wird
// nie still ersetzt.
static QCBildschirm *erste_wahl(void) {
    NSString *gemerkt = nil;
    int r = qc_bildschirm_wunsch_laden(&gemerkt);
    if (r < 0) logf_(@"Bildschirmwahl: bildschirm.txt unlesbar - Automatik");
    else if (r == 0 && gemerkt) { g_display_wunsch = gemerkt; logf_(@"Bildschirmwahl: Wunsch %@ aus bildschirm.txt", gemerkt); }
    return bildschirm_neu_bewerten(QC_ANLASS_START);
}

// Start immer mit Kandidat 0 (HEVC 4:4:4 10 Bit); das Aufnahmeformat gehoert
// zum Kandidaten, nicht zur Kommandozeile.
#define QC_START_KANDIDAT 0

// Stand vor dem ersten Bild: Laengenpraefix, Werte der Begruessung, Kandidat.
static void start_stand(int outW, int outH, int fps) {
    g_stats.nal_len = 4;
    g_info_w = outW; g_info_h = outH; g_info_fps = fps;
    atomic_store(&g_codec_id, QC_START_KANDIDAT);
}

// Aufnahmeeinstellung, Empfaenger, Warteschlangen und Taktgeber der festen
// Bildrate - fuer Dienst und --capture gleich.
static void aufnahme_einrichten(int outW, int outH, int fps, int mbit) {
    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    cfg.width = outW;
    cfg.height = outH;
    cfg.pixelFormat = pixfmt_fuer(QC_START_KANDIDAT);
    cfg.showsCursor = NO;
    if (@available(macOS 15.0, *)) cfg.showMouseClicks = NO;
    cfg.colorSpaceName = kCGColorSpaceSRGB;
    cfg.queueDepth = 8;
    qc_audio_configure(cfg);   // setzt capturesAudio und das Tonformat
    cfg.preservesAspectRatio = YES;
    cfg.captureResolution = SCCaptureResolutionBest;
    cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
    cfg.scalesToFit = YES;

    Grabber *grab = [[Grabber alloc] init];
    g_grab = grab;
    g_cfg = cfg;
    atomic_store(&g_cur_mbit, mbit);
    atomic_store(&g_cur_fps, fps);

    dispatch_queue_t q = dispatch_queue_create("tech.quadchroma.capture", DISPATCH_QUEUE_SERIAL);
    g_capq = q;
    g_lifeq = dispatch_queue_create("tech.quadchroma.lebenslauf", DISPATCH_QUEUE_SERIAL);

    // Taktgeber fuer die feste Bildrate. Er laeuft immer mit, tut aber nichts,
    // solange der Schalter aus ist oder niemand zuschaut - so kostet er nichts
    // und ist sofort da, wenn im Betrieb umgeschaltet wird.
    g_tick = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, q);
    if (g_tick) {
        uint64_t iv = (uint64_t)(NSEC_PER_SEC / (uint64_t)(fps > 0 ? fps : 60));
        dispatch_source_set_timer(g_tick, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv), iv, iv / 10);
        dispatch_source_set_event_handler(g_tick, ^{ fixed_tick(); });
        dispatch_resume(g_tick);
    }
}

// Pruefmodus: nimmt die Aufnahme einen Formatwechsel im Betrieb an?
// Das ist nirgends dokumentiert und entscheidet, ob die Codecwahl ohne
// Neustart des Stroms geht. Also messen statt annehmen.
static int formattest_laufen(void) {
    QCBildschirm *b = qc_bildschirm_wahl(qc_bildschirme_holen(), nil, NULL);
    SCDisplay *d = b.sc;
    if (!d) return 4;
    // In der Groesse, die der Dienst ohne --out streamen wuerde (nativ).
    int fw = 0, fh = 0;
    stromgroesse_fuer(b, &fw, &fh);
    logf_(@"Formattest: Bildschirm %@, %@ -> %dx%d", bildschirm_text(b), bildschirm_masse(b), fw, fh);
    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    cfg.width = (size_t)fw; cfg.height = (size_t)fh;
    cfg.captureResolution = SCCaptureResolutionBest;
    cfg.pixelFormat = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    cfg.showsCursor = NO;
    cfg.minimumFrameInterval = CMTimeMake(1, 60);
    SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:d excludingWindows:@[]];
    Grabber *g = [[Grabber alloc] init];
    SCStream *st = [[SCStream alloc] initWithFilter:f configuration:cfg delegate:g];
    dispatch_queue_t q2 = dispatch_queue_create("tech.quadchroma.formattest", DISPATCH_QUEUE_SERIAL);
    NSError *e2 = nil;
    [st addStreamOutput:g type:SCStreamOutputTypeScreen sampleHandlerQueue:q2 error:&e2];
    dispatch_semaphore_t sem2 = dispatch_semaphore_create(0);
    [st startCaptureWithCompletionHandler:^(NSError *x) {
        logf_(@"Start: %@", x ? x.localizedDescription : @"ok");
        dispatch_semaphore_signal(sem2);
    }];
    dispatch_semaphore_wait(sem2, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC));
    atomic_store(&g_formattest, 1);
    [NSThread sleepForTimeInterval:1.5];

    OSType ziele[] = { kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                       kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange,
                       kCVPixelFormatType_32BGRA };
    for (int zi = 0; zi < 3; zi++) {
        uint32_t be = CFSwapInt32HostToBig(ziele[zi]);
        logf_(@"--- wechsle auf %.4s ---", (char *)&be);
        cfg.pixelFormat = ziele[zi];
        dispatch_semaphore_t s3 = dispatch_semaphore_create(0);
        [st updateConfiguration:cfg completionHandler:^(NSError *x) {
            logf_(@"updateConfiguration: %@", x ? x.localizedDescription : @"ohne Fehler");
            dispatch_semaphore_signal(s3);
        }];
        dispatch_semaphore_wait(s3, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
        atomic_store(&g_formattest, 1);
        [NSThread sleepForTimeInterval:1.5];
    }
    // HDR im Betrieb an und aus (HDR-Plan 5.1): nimmt die Aufnahme den
    // Wechsel des Dynamikumfangs per updateConfiguration an, und welche
    // Anhaenge tragen die Bilder danach - in beiden Richtungen.
    if (@available(macOS 15.0, *)) {
        struct { const char *name; SCCaptureDynamicRange dr; CFStringRef cs, cm; } hdr_ziele[] = {
            { "HDR kanonisch, Display P3 PQ, Matrix BT.709", SCCaptureDynamicRangeHDRCanonicalDisplay, kCGColorSpaceDisplayP3_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2 },
            { "HDR lokal, Display P3 PQ, Matrix BT.709", SCCaptureDynamicRangeHDRLocalDisplay, kCGColorSpaceDisplayP3_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2 },
            { "HDR kanonisch, BT.2100 PQ, Matrix BT.2020", SCCaptureDynamicRangeHDRCanonicalDisplay, kCGColorSpaceITUR_2100_PQ, kCVImageBufferYCbCrMatrix_ITU_R_2020 },
            { "SDR, sRGB, Matrix BT.709 (zurueck)", SCCaptureDynamicRangeSDR, kCGColorSpaceSRGB, kCVImageBufferYCbCrMatrix_ITU_R_709_2 },
        };
        cfg.pixelFormat = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
        for (size_t zi = 0; zi < sizeof hdr_ziele / sizeof hdr_ziele[0]; zi++) {
            logf_(@"--- wechsle auf xf44, %s ---", hdr_ziele[zi].name);
            cfg.captureDynamicRange = hdr_ziele[zi].dr;
            cfg.colorSpaceName = hdr_ziele[zi].cs;
            cfg.colorMatrix = hdr_ziele[zi].cm;
            dispatch_semaphore_t s3 = dispatch_semaphore_create(0);
            [st updateConfiguration:cfg completionHandler:^(NSError *x) {
                logf_(@"updateConfiguration: %@", x ? x.localizedDescription : @"ohne Fehler");
                dispatch_semaphore_signal(s3);
            }];
            dispatch_semaphore_wait(s3, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
            atomic_store(&g_formattest, 1);
            [NSThread sleepForTimeInterval:1.5];
        }
    } else {
        logf_(@"--- HDR: braucht macOS 15 - uebersprungen ---");
    }
    [st stopCaptureWithCompletionHandler:^(NSError *x) { (void)x; }];
    [NSThread sleepForTimeInterval:0.5];
    return 0;
}

// ------------------------------------------------------------ Werkzeuge

int qc_werkzeug(int argc, const char *const argv[]) { @autoreleasepool {
    NSArray<NSString *> *args = argumente_aus(argc, argv);
    BOOL do_list = [args containsObject:@"--list"];
    BOOL formattest = [args containsObject:@"--formattest"];
    // --capture neben --serve ist kein Werkzeug: dann schreibt der Dienst den
    // Strom zusaetzlich in die Datei (qc_dienst_starten).
    BOOL capture = [args containsObject:@"--capture"] && ![args containsObject:@"--serve"];
    if (!do_list && !formattest && !capture) return QC_KEIN_WERKZEUG;
    protokoll_oeffnen();
    argumente_pruefen(args);
    int r = schluessel_laden();
    if (r) return r;

    // Ohne Freigabe fuer die Bildschirmaufnahme einmal nachfragen; die
    // Werkzeuge brauchen sie sofort.
    if (!g_tcc_bildschirm()) {
        logf_(@"Bildschirmaufnahme nicht freigegeben, frage nach.");
        CGRequestScreenCaptureAccess();
        return 3;
    }
    // Die Namen der Bildschirme kommen von AppKit, auf dem Hauptfaden.
    qc_bildschirm_namen_auffrischen();
    if (do_list) { list_displays(); codecs_pruefen(); return 0; }
    if (formattest) return formattest_laufen();
    codecs_pruefen();

    int fps, mbit, outW, outH, port;
    double seconds;
    NSString *outPath;
    optionen_lesen(args, &fps, &mbit, &outW, &outH, &port, &seconds, &outPath);
    QCBildschirm *display = erste_wahl();
    if (!display) { logf_(@"Kein Bildschirm - Abbruch"); return 4; }
    stromgroesse_fuer(display, &outW, &outH);
    size_t pxW = display.w, pxH = display.h;
    if ((r = ausgabe_oeffnen(outPath))) return r;
    start_stand(outW, outH, fps);
    // --hdr: die Datei in HDR10 (BT.2020/PQ, SEI 137/144) - nur wenn Mac und
    // Encoder es koennen; der Bildschirm muss es nicht (dann SDR-Inhalt in PQ).
    int pq = [args containsObject:@"--hdr"];
    if (pq && !g_befund[QC_START_KANDIDAT].hdr) {
        logf_(@"--hdr: dieser Mac kann kein HDR10 (macOS 15, Apple Silicon, Encoder) - Aufnahme in SDR");
        pq = 0;
    }
    logf_(@"\n=== Aufnahme %.1f s: %@ (%zux%zu Pixel) -> %dx%d, %d fps%s ===",
          seconds, bildschirm_text(display), pxW, pxH, outW, outH, fps, pq ? ", HDR10" : "");
    if (!encoder_start(QC_START_KANDIDAT, outW, outH, fps, mbit, pq)) { if (g_stats.out) fclose(g_stats.out); return 6; }
    aufnahme_einrichten(outW, outH, fps, mbit);
    aufnahme_farbe_setzen(g_cfg, pq);
    atomic_store(&g_anhaenge_loggen, 1);
    Grabber *grab = g_grab;

    // Aufnahme in eine Datei: sofort loslegen.
    if (!display.sc) { logf_(@"Kein SCDisplay zum Bildschirm"); return 7; }
    SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display.sc excludingWindows:@[]];
    SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:g_cfg delegate:grab];
    g_stream = stream;
    NSError *err = nil;
    if (![stream addStreamOutput:grab type:SCStreamOutputTypeScreen sampleHandlerQueue:g_capq error:&err]) {
        logf_(@"Ausgabe konnte nicht angemeldet werden: %@", err.localizedDescription);
        return 7;
    }
    qc_audio_attach(stream, audio_cb);
    __block BOOL started = NO;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [stream startCaptureWithCompletionHandler:^(NSError *e) {
        if (e) logf_(@"Start fehlgeschlagen: %@ (Code %ld)", e.localizedDescription, (long)e.code);
        else started = YES;
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 15ull * NSEC_PER_SEC));
    if (!started) { if (g_stats.out) fclose(g_stats.out); return 8; }

    // Einmal blind messen, wie im Dienst.
    { qc_last vorlauf; qc_last_probe(&vorlauf); }

    NSDate *t0 = [NSDate date];
    [[NSRunLoop currentRunLoop] runUntilDate:[t0 dateByAddingTimeInterval:seconds]];
    double dt = -[t0 timeIntervalSinceNow];
    dispatch_semaphore_t ende = dispatch_semaphore_create(0);
    [g_stream stopCaptureWithCompletionHandler:^(NSError *e) { (void)e; dispatch_semaphore_signal(ende); }];
    dispatch_semaphore_wait(ende, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
    if (g_session) VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);
    if (g_stats.out) fclose(g_stats.out);

    logf_(@"Bilder aufgenommen: %ld (unveraendert uebersprungen: %ld)", grab.framesIn, grab.framesSkipped);
    logf_(@"Bilder codiert: %ld, verworfen: %ld, erster Fehler: %d", g_stats.encoded, g_stats.dropped, (int)g_stats.first_err);
    logf_(@"Tempo: %.1f Bilder/s ueber %.1f s", g_stats.encoded / dt, dt);
    logf_(@"Datenmenge: %.1f MB, entspricht %.1f Mbit/s", g_stats.bytes / 1e6, g_stats.bytes * 8.0 / dt / 1e6);
    return 0;
}}

// ------------------------------------------------------------ Dienst

// Der Abschied des Dienstes, einmal je Prozess - ob ihn die Oberflaeche
// (applicationShouldTerminate:, Beobachter des Beendens) oder der Client
// (qc_dienst_beenden) zuerst anstoesst; wer spaeter kommt, wartet auf den
// ersten.
static void dienst_abschied(void) {
    static dispatch_once_t einmal;
    dispatch_once(&einmal, ^{ host_abschied(); });
}

// Was qc_app_einrichten und qc_dienst_starten gemeinsam vorbereiten, einmal
// je Prozess: Protokoll, Schalter, Schluessel, Zugang, Oberflaeche. Kein
// Lauschen, keine Rueckfrage des Systems, keine Aufnahme.
static _Atomic int g_eingerichtet = 0;
static NSArray<NSString *> *g_dienst_args = nil;   // nur auf dem Hauptfaden
static int g_dienst_eingebettet = 0;

static int einrichten(const qc_dienst_cfg *dc) {
    if (atomic_load(&g_eingerichtet)) return QC_DIENST_OK;
    const int eingebettet = dc && dc->eingebettet;
    NSArray<NSString *> *args = argumente_aus(dc ? dc->argc : 0, dc ? dc->argv : NULL);
    argumente_pruefen(args);
    int r = schluessel_laden();
    if (r) return r;
    g_dienst_args = args;
    g_dienst_eingebettet = eingebettet;
    atomic_store(&g_eingerichtet, 1);

    // Zugang (zugang.h): Migration aus authorized.txt, Zugangspasswort
    // anlegen, wenn es fehlt. Vor der Annahme - und in der einen App schon
    // ohne Freigabe: ihr Menue zeigt ID, Passwort und Geraete immer.
    qc_zugang_protokoll_setzen(zugang_zeile);
    qc_zugang_entfernt_setzen(zuschauer_entfernt);
    qc_zugang_start(g_id_pub);
    {
        char fp[24], id_text[12];
        qc_fingerprint(g_id_pub, fp);
        qc_zugang_id_text(qc_zugang_eigene_id(), id_text);
        int anz = qc_zugang_geraete(NULL, 0);
        if (anz < 0)
            logf_(@"Geraete-ID dieses Hosts: %s (Fingerabdruck %s)   Geraeteliste host-devices.txt NICHT LESBAR ODER "
                   "BESCHAEDIGT - jedes Geraet braucht Passwort oder Zulassen, bis sie im Menue zurueckgesetzt ist", id_text, fp);
        else
            logf_(@"Geraete-ID dieses Hosts: %s (Fingerabdruck %s)   erlaubte Geraete: %d", id_text, fp, anz);
    }

    // Die Oberflaeche vor der Annahme: ab hier ist "Zulassen" moeglich
    // (qc_ui_vorhanden) - schon in der ersten Bekanntgabe (Flag) und in
    // Nachricht 20 an einen Client, der gleich beim Start verbindet.
    // Anfragen, bevor die Run-Loop laeuft, warten auf der Main Queue.
    qc_oberflaeche_cfg ui = { .abschied = dienst_abschied, .protokoll = ui_protokoll,
                              .eingebettet = eingebettet, .oeffnen = dc ? dc->oeffnen : NULL,
                              .app = dc ? dc->app : NULL, .name_pruefen = dc ? dc->name_pruefen : NULL };
    qc_oberflaeche_starten(&ui);
    return QC_DIENST_OK;
}

// Die eine App schreibt das Protokoll des Hosts wie die Host-Rolle unter
// Windows in ihren Ablageordner: host-protokoll.txt, ueber der Grenze
// host-protokoll.alt.txt - nicht mehr nach /tmp. Nur solange es noch nicht
// offen ist.
static void protokoll_in_ablage(void) {
    static char pfad[1200], alt[1200];
    if (qc_config_path("host-protokoll.txt", pfad, sizeof pfad) != 0 ||
        qc_config_path("host-protokoll.alt.txt", alt, sizeof alt) != 0)
        return;
    pthread_mutex_lock(&g_log_mtx);
    if (!g_log) {
        g_log_pfad = pfad;
        g_log_alt_pfad = alt;
    }
    pthread_mutex_unlock(&g_log_mtx);
}

int qc_app_einrichten(const qc_dienst_cfg *cfg) { @autoreleasepool {
    if (cfg && cfg->app) protokoll_in_ablage();
    protokoll_oeffnen();
    if (![NSThread isMainThread]) {
        logf_(@"Dienst: Einrichten nicht auf dem Hauptfaden - nichts eingerichtet");
        return QC_DIENST_FADEN;
    }
    if (atomic_load(&g_eingerichtet)) return QC_DIENST_DOPPELT;
    logf_(@"Die eine App: Oberflaeche, Schluessel und Zugang eingerichtet - lauschen erst mit der Freigabe");
    return einrichten(cfg);
}}

int qc_dienst_starten(const qc_dienst_cfg *dc) { @autoreleasepool {
    protokoll_oeffnen();
    if (![NSThread isMainThread]) {
        logf_(@"Dienst: Start nicht auf dem Hauptfaden - nicht gestartet");
        return QC_DIENST_FADEN;
    }
    if (atomic_load(&g_dienst_gestartet)) return QC_DIENST_DOPPELT;
    // Schon eingerichtet (die eine App): deren Schalter und Rueckrufe gelten.
    const int eingebettet = atomic_load(&g_eingerichtet) ? g_dienst_eingebettet : dc && dc->eingebettet;
    NSArray<NSString *> *args = atomic_load(&g_eingerichtet) ? g_dienst_args
                                                              : argumente_aus(dc ? dc->argc : 0, dc ? dc->argv : NULL);
    if (eingebettet)
        logf_(@"Host-Dienst im Client-Prozess (eingebettet)");
    else if (![args containsObject:@"--serve"])
        // Ohne Modus - so startet der Finder die App per Doppelklick, und so
        // startet sie beim Anmelden - laeuft der Host wie mit --serve auf dem
        // Standard-Port; die uebrigen Schalter (--fps ...) gelten wie gewohnt.
        logf_(@"Host-Modus (ohne Modus-Argument) auf dem Standard-Port");

    // Hoechstens ein Host je Nutzer (Spezifikation 7.5): ein zweiter Start -
    // Doppelklick, Anmeldeobjekt, "open -n" - endet still. Die Werkzeuge
    // (qc_werkzeug) duerfen daneben laufen. Die eine App faengt ihren
    // zweiten Start schon selbst ab (einzel.rs); hier trifft sie nur einen
    // fremden Host, etwa eine aeltere QuadChroma.app - dann bleibt ihre
    // Freigabe aus, und das Menue zeigt den Port als belegt.
    int instanz = qc_zugang_einzelinstanz();
    if (instanz == 0) {
        if (eingebettet) {
            int p = 9001;
            NSInteger i = [args indexOfObject:@"--serve"];
            if (i != NSNotFound && i + 1 < (NSInteger)args.count && ![args[i + 1] hasPrefix:@"--"] &&
                args[i + 1].intValue > 0)
                p = args[i + 1].intValue;
            atomic_store(&g_port_belegt, p);
            qc_ui_zustand_geaendert();
        }
        logf_(eingebettet ? @"Es laeuft schon ein QuadChroma-Host fuer diesen Nutzer - der Dienst im Client startet nicht"
                          : @"Es laeuft schon ein QuadChroma-Host fuer diesen Nutzer - dieser Start endet");
        return QC_DIENST_LAEUFT_SCHON;
    }
    if (instanz < 0) logf_(@"Einzelinstanz: host-instanz.lock laesst sich nicht sperren - der Host laeuft trotzdem");

    int r = einrichten(dc);
    if (r) return r;
    int fps, mbit, outW, outH, port;
    double seconds;
    NSString *outPath;
    optionen_lesen(args, &fps, &mbit, &outW, &outH, &port, &seconds, &outPath);
    (void)seconds;
    if ((r = ausgabe_oeffnen(outPath))) return r;
    atomic_store(&g_dienst_gestartet, 1);
    atomic_store(&g_freigabe_aus, 0);

    // Ohne Freigabe fuer die Bildschirmaufnahme einmal nachfragen. Der Host
    // laeuft weiter (Spezifikation 7.4), zeigt den Stand im Menue und nimmt
    // auf, sobald sie erteilt ist.
    if (!g_tcc_bildschirm()) {
        logf_(@"Bildschirmaufnahme nicht freigegeben, frage nach.");
        CGRequestScreenCaptureAccess();
        logf_(@"Der Host laeuft ohne Bildschirmaufnahme weiter - sie startet, sobald die Freigabe erteilt ist");
    }
    // Die Namen der Bildschirme kommen von AppKit, auf dem Hauptfaden.
    qc_bildschirm_namen_auffrischen();
    codecs_pruefen();
    QCBildschirm *display = erste_wahl();
    // Der Host wartet ohne Bildschirmliste (keine Freigabe, kein Monitor
    // beim Anmelden) - die Aufnahme entsteht ohnehin erst mit einem Zuschauer.
    if (!display) logf_(@"Kein Bildschirm abrufbar (Freigabe fehlt oder kein Monitor) - der Host laeuft und wartet");
    if (display) stromgroesse_fuer(display, &outW, &outH);
    else ersatzgroesse(&outW, &outH);
    size_t pxW = display.w, pxH = display.h;
    double hz = display.hz;

    // Keine Aufnahme, kein Encoder, bis sich jemand meldet.
    start_stand(outW, outH, fps);
    aufnahme_einrichten(outW, outH, fps, mbit);

    // Dateien: empfangene gehen als Dateiliste in die Ablage; Reste
    // frueherer Laeufe, aelter als 24 h, und jede unfertige Uebertragung
    // (Marke .laeuft, der vorige Lauf endete mitten im Empfang) raeumt die
    // Empfangswarteschlange weg - vor der Annahme unten, also bevor ein
    // Empfang beginnen kann.
    dateien_einrichten();
    qc_dateien_fertig_setzen(qc_clip_set_dateien);
    qc_dateien_aufraeumen_beim_start();
    qc_clip_dateien(clip_dateien_cb);
    qc_clip_bedingung(zeiger_aktiv);   // Inhalt nur mit Zuschauer lesen
    qc_clip_start(clip_cb);
    qc_zeiger_start(zeiger_cb, zeiger_aktiv, zeiger_log);
    logf_(@"Zeigerform: Abfrage alle 50 ms, nur mit Zuschauer");
    // Die Annahme zuletzt. Ihre Faeden laufen sofort los; ein Client, der
    // beim Neustart des Hosts schon wartet und neu verbindet, kann durch
    // Handschlag und Freigabe sein, bevor dieser Start weiter unten ankaeme.
    // bild_verbindung braucht dann g_lifeq und g_capq
    // (stream_hochfahren_sync; dispatch_sync auf eine NULL-Warteschlange
    // endet mit SIGSEGV), g_cfg, g_grab, den Zielbildschirm und die Werte
    // g_cur_*; apply_settings aus dem Eingabekanal liest g_tick. Frueher
    // startete die Annahme vor all dem.
    // Aenderungen der Bildschirmkonfiguration meldet CoreGraphics auf dem
    // Hauptfaden (dessen Run-Loop dreht [NSApp run] bzw. winit); der Host
    // bewertet dann entprellt neu und folgt dem Hauptbildschirm bzw.
    // kehrt zum gewuenschten zurueck - ohne Neustart, ohne neuen Zuschauer.
    // Der Wunsch fuer die Zeile unten wird hier gelesen: sobald die
    // Annahme laeuft, gehoert g_display_wunsch der Lebenslauf-Warteschlange.
    NSString *wunsch_text = g_display_wunsch ? [NSString stringWithFormat:@" - Wunsch %@", g_display_wunsch]
                                             : @" - Automatik (folgt dem Hauptbildschirm)";
    CGDisplayRegisterReconfigurationCallback(bildschirm_rueckruf, NULL);
    g_dienst_port = port;
    g_dienst_display = display ? display.displayID : CGMainDisplayID();
    // Die Oberflaeche steht schon (einrichten); ab hier lauschen die Ports.
    BOOL laeuft = dienst_starten();
    qc_ui_zustand_geaendert();
    zustand_takt_starten();
    BOOL ax = AXIsProcessTrusted();
    bedienungshilfen_einmal_fragen();
    if (laeuft) logf_(@"\n=== Dienst laeuft: Bild %d, Eingabe %d, Bekanntgabe %d ===", port, port + 1, port + 2);
    logf_(@"Bildschirm %@: %@, %.0f Hz -> %dx%d, %d fps%@", bildschirm_text(display),
          display ? bildschirm_masse(display) : [NSString stringWithFormat:@"%zux%zu", pxW, pxH], hz, outW, outH, fps, wunsch_text);
    logf_(@"Bedienungshilfen-Freigabe (fuer Maus und Tastatur): %@", ax ? @"erteilt" : @"FEHLT - Eingaben werden ignoriert");

    // Einmal blind messen, damit die erste echte Meldung schon eine Differenz
    // hat und nicht mit Nullen anfaengt.
    { qc_last vorlauf; qc_last_probe(&vorlauf); }

    // Der Takt (alle fuenf Sekunden Stand, Auslastung, Stau-Frist) laeuft auf
    // eigener Warteschlange; der Hauptfaden gehoert AppKit - Menueleiste,
    // Zulassen-Fenster, Zeigerform, Zwischenablage, Bildschirmrueckrufe.
    dienst_takt_starten(QC_TAKT_S);
    return QC_DIENST_OK;
}}

// Freigabe aus (die eine App): siehe dienst.h und dienst_lauschen_beenden.
void qc_dienst_anhalten(void) {
    if (!atomic_load(&g_dienst_gestartet)) return;
    logf_(@"Freigabe aus: der Zuschauer bekommt den Abschied (Grund 1), die Ports gehen zu");
    dispatch_async(zustandq(), ^{ @autoreleasepool { dienst_lauschen_beenden(); } });
}

// Freigabe wieder an: nur Annahme und Bekanntgabe, alles andere steht noch.
// Ist der Port inzwischen belegt, versucht es der Zustandstakt wie beim Start.
void qc_dienst_fortsetzen(void) {
    if (!atomic_load(&g_dienst_gestartet)) return;
    dispatch_async(zustandq(), ^{ @autoreleasepool {
        atomic_store(&g_freigabe_aus, 0);
        if (dienst_starten())
            logf_(@"Freigabe an: Bild %d, Eingabe %d, Bekanntgabe %d", g_dienst_port, g_dienst_port + 1, g_dienst_port + 2);
        qc_ui_zustand_geaendert();
    }});
}

int qc_dienst_gestartet(void) { return atomic_load(&g_dienst_gestartet); }

void qc_dienst_beenden(void) {
    if (!atomic_load(&g_dienst_gestartet)) return;
    dispatch_semaphore_t fertig = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        dienst_abschied();
        dispatch_semaphore_signal(fertig);
    });
    // Frist wie beim Beenden ueber das Menue: ein haengender Zuschauer
    // (Sendefrist 2 s) haelt das Ende nicht auf.
    if (dispatch_semaphore_wait(fertig, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC)) != 0)
        logf_(@"Beenden: der Abschied ist nach 3 s nicht fertig - das Beenden geht weiter");
}
