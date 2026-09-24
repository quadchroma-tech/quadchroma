// QuadChroma Host: Dateien ueber die Zwischenablage - Umsetzung. Siehe dateien.h.
//
// Ein Sender (Host -> Client) und ein Empfaenger (Client -> Host), je
// hoechstens eine Uebertragung zur Zeit, beide unabhaengig voneinander.
// Der Empfaenger prueft alles, was ankommt, als waere es feindlich: Pfade,
// Laengen, Reihenfolge, Groessen. Geschrieben wird nur in ein frisches
// Verzeichnis unter der Ablagebasis, jede Datei exklusiv angelegt, und
// Verknuepfungen werden weder angelegt noch verfolgt.
#define __STDC_WANT_LIB_EXT1__ 1      // fuer memset_s
#import "dateien.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

// Was hoechstens auf der Empfangswarteschlange warten darf, wie im Rust-Kern
// (client/src/dateien.rs, WARTEND_DATEN_MAX und WARTEND_ENDEN_MAX): die
// Datenbytes der Stuecke (je Stueck mindestens 1) bis FENSTER + STUECK_MAX -
// so viel hat ein Sender, der sein Fenster einhaelt, hoechstens unquittiert
// unterwegs, gleich in wie vielen Stuecken, auch einer, der nur VOR dem
// Stueck prueft und dann ein ganzes schickt. Die Zahl der Stuecke ist bewusst
// nicht eigens begrenzt (viele kleine Dateien sind erlaubt). Dazu hoechstens
// 8 Enden; ein neues Angebot leert die Warteschlange, es wartet also
// hoechstens eines. Darueber ist es ein Protokollfehler (Quittung 4).
#define QC_EMPFANG_DATEN_MAX  ((size_t)QC_DATEI_FENSTER + QC_DATEI_STUECK_MAX)
#define QC_EMPFANG_ENDEN_MAX  8u

// --------------------------------------------------------------- Helfer

static void speicher_loeschen(void *p, size_t n) {
    if (p && n) memset_s(p, n, 0, n);
}

static uint64_t jetzt_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000u + (uint64_t)t.tv_nsec / 1000000u;
}

static uint64_t unix_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (uint64_t)tv.tv_sec * 1000u + (uint64_t)tv.tv_usec / 1000u;
}

static void le16(uint8_t *p, uint16_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void le32(uint8_t *p, uint32_t v) { for (int i = 0; i < 4; i++) p[i] = (uint8_t)(v >> (8 * i)); }
static void le64(uint8_t *p, uint64_t v) { for (int i = 0; i < 8; i++) p[i] = (uint8_t)(v >> (8 * i)); }
static uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }
static uint32_t rd32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static uint64_t rd64(const uint8_t *p) { return (uint64_t)rd32(p) | ((uint64_t)rd32(p + 4) << 32); }

// ------------------------------------------------------------ Zustand

static qc_dateien_wege g_wege;                      // einmal gesetzt, danach nur gelesen
static dispatch_queue_t g_empfang_q = nil;          // "dateien-empfang"
static dispatch_queue_t g_senden_q = nil;           // "dateien-senden"
static void (*g_fertig)(NSArray<NSString *> *) = NULL;   // vor dem ersten Empfang gesetzt

static pthread_mutex_t g_basis_mtx = PTHREAD_MUTEX_INITIALIZER;
static NSString *g_basis = nil;                     // nil = Vorgabe; unter g_basis_mtx
static _Atomic uint32_t g_stillstand_ms = QC_DATEI_STILLSTAND_MS;
static _Atomic uint64_t g_reserve = QC_DATEI_PLATZRESERVE;

// ---------------------------------------------------------- Protokoll

static void zeile(NSString *fmt, ...) NS_FORMAT_FUNCTION(1, 2);
static void zeile(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    const char *c = s.UTF8String;
    if (!c) return;
    if (g_wege.protokoll) {
        g_wege.protokoll(c);
    } else {
        fprintf(stdout, "%s\n", c);
        fflush(stdout);
    }
}

// Groessen wie im Protokoll des Windows-Hosts: MB mit einer Nachkommastelle
// und deutschem Komma ("12,4 MB"), ebenso Sekunden.
static NSString *mb_text(uint64_t bytes) {
    NSString *s = [NSString stringWithFormat:@"%.1f MB", (double)bytes / 1e6];
    return [s stringByReplacingOccurrencesOfString:@"." withString:@","];
}

static NSString *sek_text(double s) {
    NSString *t = [NSString stringWithFormat:@"%.1f s", s];
    return [t stringByReplacingOccurrencesOfString:@"." withString:@","];
}

static NSString *zustand_text(int z) {
    switch (z) {
        case QC_QUITT_ZU_GROSS:      return @"zu gross oder zu viele Eintraege";
        case QC_QUITT_KEIN_PLATZ:    return @"zu wenig Platz";
        case QC_QUITT_UNGUELTIG:     return @"ungueltig";
        case QC_QUITT_SCHREIBFEHLER: return @"Schreibfehler";
        case QC_QUITT_ABGEBROCHEN:   return @"abgebrochen";
        default:                     return [NSString stringWithFormat:@"Zustand %d", z];
    }
}

static NSString *ende_text(int g) {
    switch (g) {
        case QC_ENDE_ABGEBROCHEN: return @"abgebrochen";
        case QC_ENDE_LESEFEHLER:  return @"Lesefehler beim Sender";
        case QC_ENDE_ZEIT:        return @"Zeitueberschreitung beim Sender";
        default:                  return [NSString stringWithFormat:@"Grund %d", g];
    }
}

// ------------------------------------------------------ Kodieren und Lesen

@implementation QCDateiEintrag
+ (instancetype)art:(uint8_t)art groesse:(uint64_t)groesse pfad:(NSString *)pfad {
    QCDateiEintrag *e = [[self alloc] init];
    e.art = art;
    e.groesse = groesse;
    e.pfad = [pfad dataUsingEncoding:NSUTF8StringEncoding];
    return e;
}
@end

size_t qc_datei_grenze(uint8_t typ) {
    switch (typ) {
        case QC_DATEI_ANGEBOT:   return QC_DATEI_ANGEBOT_MAX;
        case QC_DATEI_STUECK:    return QC_DATEI_STUECK_KOPF + QC_DATEI_STUECK_MAX;
        case QC_DATEI_ENDE:
        case QC_DATEI_QUITTUNG:
        case QC_IN_FAEHIGKEITEN: return 256;
        default:                 return 0;
    }
}

NSData *qc_datei_faehigkeiten_kodieren(uint32_t bits) {
    uint8_t b[4];
    le32(b, bits);
    return [NSData dataWithBytes:b length:sizeof b];
}

int qc_datei_faehigkeiten_lesen(const uint8_t *p, size_t n, uint32_t *bits) {
    if (!p || n < 4) return -1;
    if (bits) *bits = rd32(p);
    return 0;
}

NSData *qc_datei_angebot_kodieren(uint32_t kennung, NSArray<QCDateiEintrag *> *eintraege) {
    size_t n = 16;
    uint64_t gesamt = 0;
    for (QCDateiEintrag *e in eintraege) {
        if (e.pfad.length > 0xFFFF) return nil;
        n += 12 + e.pfad.length;
        if (e.art == 0) gesamt += e.groesse;
    }
    NSMutableData *d = [NSMutableData dataWithLength:n];
    uint8_t *p = d.mutableBytes;
    le32(p, kennung);
    le32(p + 4, (uint32_t)eintraege.count);
    le64(p + 8, gesamt);
    size_t o = 16;
    for (QCDateiEintrag *e in eintraege) {
        p[o] = e.art;
        p[o + 1] = 0;
        le16(p + o + 2, (uint16_t)e.pfad.length);
        le64(p + o + 4, e.groesse);
        if (e.pfad.length) memcpy(p + o + 12, e.pfad.bytes, e.pfad.length);
        o += 12 + e.pfad.length;
    }
    return d;
}

// Strenges UTF-8: keine ueberlangen Formen, keine Ersatzzeichen (Surrogate),
// nichts ueber U+10FFFF.
static int utf8_gueltig(const uint8_t *s, size_t n) {
    size_t i = 0;
    while (i < n) {
        uint8_t c = s[i];
        if (c < 0x80) { i++; continue; }
        size_t len;
        uint32_t cp, min;
        if ((c & 0xE0) == 0xC0)      { len = 2; cp = c & 0x1Fu; min = 0x80; }
        else if ((c & 0xF0) == 0xE0) { len = 3; cp = c & 0x0Fu; min = 0x800; }
        else if ((c & 0xF8) == 0xF0) { len = 4; cp = c & 0x07u; min = 0x10000; }
        else return 0;
        if (i + len > n) return 0;
        for (size_t k = 1; k < len; k++) {
            if ((s[i + k] & 0xC0) != 0x80) return 0;
            cp = (cp << 6) | (s[i + k] & 0x3Fu);
        }
        if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) return 0;
        i += len;
    }
    return 1;
}

// Pfad nach 2.5 pruefen (ohne Reihenfolge und Doppelte). nil = in Ordnung.
static NSString *pfad_pruefen(const uint8_t *s, size_t n) {
    if (!utf8_gueltig(s, n)) return @"kein gueltiges UTF-8";
    if (memchr(s, 0, n)) return @"NUL im Pfad";
    if (memchr(s, '\\', n)) return @"'\\' im Pfad";
    if (s[0] == '/') return @"absoluter Pfad";
    size_t tiefe = 0, anfang = 0;
    for (size_t i = 0; i <= n; i++) {
        if (i < n && s[i] != '/') continue;
        size_t l = i - anfang;
        if (l == 0) return @"leerer Bestandteil";
        if (l > QC_DATEI_BESTANDTEIL_MAX) return @"Bestandteil ueber 255 Byte";
        if ((l == 1 && s[anfang] == '.') || (l == 2 && s[anfang] == '.' && s[anfang + 1] == '.'))
            return @"Bestandteil '.' oder '..'";
        if (++tiefe > QC_DATEI_TIEFE_MAX) return @"mehr als 32 Stufen";
        anfang = i + 1;
    }
    return nil;
}

NSString *qc_datei_bereinigen(NSString *t) {
    NSMutableString *m = nil;
    for (NSUInteger i = 0; i < t.length; i++) {
        if ([t characterAtIndex:i] >= 0x20) continue;
        if (!m) m = [t mutableCopy];
        [m replaceCharactersInRange:NSMakeRange(i, 1) withString:@"_"];
    }
    return m ? [m copy] : t;
}

// Ist der Stamm (vor dem ersten '.', ohne Leerzeichen am Ende) ein
// Geraetename unter Windows? Wie geraetename() im Rust-Kern: ohne Ruecksicht
// auf die ASCII-Schreibweise; COM/LPT mit einer Ziffer 0-9 oder einer
// hochgestellten 1-3.
static BOOL windows_geraetename(NSString *name) {
    // NSLiteralSearch: ohne sie vergleicht rangeOfString: ganze
    // Zeichenfolgen und saehe einen Punkt vor einem kombinierenden Zeichen
    // ("COM1." und dahinter U+0301) nicht - der Rust-Kern trennt am Zeichen
    // '.'.
    NSUInteger punkt = [name rangeOfString:@"." options:NSLiteralSearch].location;
    NSString *stamm = punkt == NSNotFound ? name : [name substringToIndex:punkt];
    NSUInteger l = stamm.length;
    while (l && [stamm characterAtIndex:l - 1] == ' ') l--;
    stamm = [stamm substringToIndex:l];
    // Nur ASCII gross (wie to_ascii_uppercase); alles andere bleibt, wie es ist.
    unichar b[8];
    if (stamm.length > 7) return NO;
    [stamm getCharacters:b range:NSMakeRange(0, stamm.length)];
    for (NSUInteger i = 0; i < stamm.length; i++)
        if (b[i] >= 'a' && b[i] <= 'z') b[i] = (unichar)(b[i] - 'a' + 'A');
    NSString *gross = [NSString stringWithCharacters:b length:stamm.length];
    for (NSString *g in @[ @"CON", @"PRN", @"AUX", @"NUL", @"CONIN$", @"CONOUT$" ])
        if ([gross isEqualToString:g]) return YES;
    if (gross.length == 4 && ([gross hasPrefix:@"COM"] || [gross hasPrefix:@"LPT"])) {
        unichar z = b[3];
        return (z >= '0' && z <= '9') || z == 0x00B9 || z == 0x00B2 || z == 0x00B3;
    }
    return NO;
}

NSString *qc_datei_bereinigen_windows(NSString *t) {
    NSMutableString *m = [t mutableCopy];
    for (NSUInteger i = 0; i < m.length; i++) {
        unichar c = [m characterAtIndex:i];
        if (c < 0x20 || (c < 0x80 && strchr("<>:\"|?*", (int)c)))
            [m replaceCharactersInRange:NSMakeRange(i, 1) withString:@"_"];
    }
    // Punkte und Leerzeichen am Ende: je Zeichen ein '_'.
    for (NSUInteger i = m.length; i > 0; i--) {
        unichar c = [m characterAtIndex:i - 1];
        if (c != '.' && c != ' ') break;
        [m replaceCharactersInRange:NSMakeRange(i - 1, 1) withString:@"_"];
    }
    if (windows_geraetename(m)) [m insertString:@"_" atIndex:0];
    return [m copy];
}

// Vergleichsschluessel fuer Doppelte: ohne Ruecksicht auf Gross- und
// Kleinschreibung - und auf die Unicode-Normalform, denn APFS unterscheidet
// ein e mit Akzent als ein Zeichen und als e plus kombinierenden Akzent
// ebenfalls nicht. Ein solches Paar schluege sonst erst beim exklusiven
// Anlegen fehl.
static NSString *schluessel(NSString *pfad) {
    return [[pfad precomposedStringWithCanonicalMapping] stringByFoldingWithOptions:NSCaseInsensitiveSearch locale:nil];
}

// Derselbe Schluessel beim Sender, fuer einen einzelnen Namen. Der Sender
// kennt das System der Gegenseite nicht (die Faehigkeiten tragen nur Bit 0),
// muss also so streng sein wie der strengste Empfaenger: erst nach der
// Windows-Bereinigung vergleichen, die die macOS-Bereinigung einschliesst.
// Sonst gingen etwa "a." und "a_", "x:y" und "x_y" oder "a\x01" und "a_"
// beide hinaus, und ein Windows-Empfaenger lehnte das ganze Angebot als
// doppelt ab.
static NSString *sende_schluessel(NSString *name) {
    return schluessel(qc_datei_bereinigen_windows(name));
}

static int angebot_pruefen(const uint8_t *p, size_t n, uint32_t *kennung_aus, uint64_t *gesamt_aus,
                           NSArray<QCDateiEintrag *> **aus, NSString **grund) {
#define ABLEHNEN(z, ...) do { *grund = [NSString stringWithFormat:__VA_ARGS__]; return (z); } while (0)
    if (n < 16) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Angebot zu kurz (%zu Byte)", n);
    if (n > QC_DATEI_ANGEBOT_MAX) ABLEHNEN(QC_QUITT_ZU_GROSS, @"Angebot ueber 1 MiB");
    uint32_t kennung = rd32(p), anzahl = rd32(p + 4);
    uint64_t gesamt = rd64(p + 8);
    *gesamt_aus = gesamt;
    if (kennung == 0) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Kennung 0");
    if (anzahl == 0) ABLEHNEN(QC_QUITT_UNGUELTIG, @"keine Eintraege");
    // Mehr als EINTRAEGE_MAX und mehr als GESAMT_MAX sind "zu gross oder zu
    // viele Eintraege" (Zustand 2 nach 2.3), nicht "ungueltig".
    if (anzahl > QC_DATEI_EINTRAEGE_MAX)
        ABLEHNEN(QC_QUITT_ZU_GROSS, @"%u Eintraege, hoechstens %u", anzahl, QC_DATEI_EINTRAEGE_MAX);
    if (gesamt > QC_DATEI_GESAMT_MAX) ABLEHNEN(QC_QUITT_ZU_GROSS, @"%@, hoechstens 4 GiB", mb_text(gesamt));
    NSMutableArray<QCDateiEintrag *> *liste = [NSMutableArray arrayWithCapacity:anzahl];
    NSMutableSet<NSData *> *ordner = [NSMutableSet set];     // rohe Pfade der bisherigen Ordner
    NSMutableSet<NSString *> *namen = [NSMutableSet set];    // Schluessel nach der Bereinigung
    uint64_t summe = 0;
    size_t o = 16;
    for (uint32_t i = 0; i < anzahl; i++) {
        if (n - o < 12) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u abgeschnitten", i);
        uint8_t art = p[o];
        uint16_t pl = rd16(p + o + 2);
        uint64_t groesse = rd64(p + o + 4);
        o += 12;
        if (art > 1) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: Art %u", i, art);
        if (art == 1 && groesse) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: Ordner mit Groesse %llu", i, groesse);
        if (pl == 0 || pl > QC_DATEI_PFAD_MAX) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: Pfadlaenge %u", i, pl);
        if (n - o < pl) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u abgeschnitten", i);
        const uint8_t *s = p + o;
        o += pl;
        NSString *warum = pfad_pruefen(s, pl);
        if (warum) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: %@", i, warum);
        // Vorfahr vor Nachfahr: der Elternpfad kam vorher, und zwar als Ordner.
        size_t eltern = pl;
        while (eltern > 0 && s[eltern - 1] != '/') eltern--;
        if (eltern && ![ordner containsObject:[NSData dataWithBytes:s length:eltern - 1]])
            ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: Elternordner fehlt davor", i);
        NSData *roh = [NSData dataWithBytes:s length:pl];
        if (art == 1) [ordner addObject:roh];
        // Die Bestandteile in NFC: so werden sie angelegt (siehe
        // eltern_oeffnen), und so gehen sie bei einer Ruecksendung wieder
        // hinaus. Zerlegt wird an den Bytes '/', wie in pfad_pruefen und im
        // Rust-Kern - nicht mit componentsSeparatedByString:, das ganze
        // Zeichenfolgen vergleicht und ein '/' vor einem kombinierenden
        // Zeichen ("a/", U+0301, "b") nicht als Trenner saehe: Der Eintrag galte
        // dann als oberster und landete als eigener Pfad in der Ablage.
        NSMutableArray<NSString *> *teile = [NSMutableArray array];
        for (size_t a = 0, b = 0; b <= pl; b++) {
            if (b < pl && s[b] != '/') continue;
            NSString *t = [[NSString alloc] initWithBytes:s + a length:b - a encoding:NSUTF8StringEncoding];
            if (!t) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: kein gueltiges UTF-8", i);
            [teile addObject:[qc_datei_bereinigen(t) precomposedStringWithCanonicalMapping]];
            a = b + 1;
        }
        // Erst nach der Bereinigung auf Doppelte pruefen.
        NSString *k = schluessel([teile componentsJoinedByString:@"/"]);
        if ([namen containsObject:k]) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: doppelt", i);
        [namen addObject:k];
        if (art == 0) summe = groesse > UINT64_MAX - summe ? UINT64_MAX : summe + groesse;
        QCDateiEintrag *e = [[QCDateiEintrag alloc] init];
        e.art = art;
        e.groesse = groesse;
        e.pfad = roh;
        e.teile = teile;
        [liste addObject:e];
    }
    if (o != n) ABLEHNEN(QC_QUITT_UNGUELTIG, @"%zu Byte nach dem letzten Eintrag", n - o);
    if (summe != gesamt) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Summe der Groessen %llu statt %llu", summe, gesamt);
    *aus = liste;
    return QC_QUITT_LAEUFT;
#undef ABLEHNEN
}

int qc_datei_angebot_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint64_t *gesamt,
                           NSArray<QCDateiEintrag *> **eintraege, NSString **grund) {
    uint64_t g = 0;
    NSArray<QCDateiEintrag *> *liste = nil;
    NSString *warum = nil;
    if (kennung) *kennung = p && n >= 4 ? rd32(p) : 0;
    int rc = p ? angebot_pruefen(p, n, kennung ? kennung : &(uint32_t){0}, &g, &liste, &warum)
               : QC_QUITT_UNGUELTIG;
    if (!p) warum = @"kein Angebot";
    if (gesamt) *gesamt = g;
    if (eintraege) *eintraege = rc == QC_QUITT_LAEUFT ? liste : nil;
    if (grund) *grund = warum;
    return rc;
}

NSData *qc_datei_stueck_kodieren(uint32_t kennung, uint32_t eintrag, uint64_t versatz,
                                 const void *daten, size_t n) {
    NSMutableData *d = [NSMutableData dataWithLength:QC_DATEI_STUECK_KOPF + n];
    uint8_t *p = d.mutableBytes;
    le32(p, kennung);
    le32(p + 4, eintrag);
    le64(p + 8, versatz);
    if (n) memcpy(p + QC_DATEI_STUECK_KOPF, daten, n);
    return d;
}

int qc_datei_stueck_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint32_t *eintrag,
                          uint64_t *versatz, const uint8_t **daten, size_t *dn) {
    if (!p || n < QC_DATEI_STUECK_KOPF + 1) return -1;
    *kennung = rd32(p);
    *eintrag = rd32(p + 4);
    *versatz = rd64(p + 8);
    *daten = p + QC_DATEI_STUECK_KOPF;
    *dn = n - QC_DATEI_STUECK_KOPF;
    return 0;
}

NSData *qc_datei_ende_kodieren(uint32_t kennung, uint8_t grund) {
    uint8_t b[5];
    le32(b, kennung);
    b[4] = grund;
    return [NSData dataWithBytes:b length:sizeof b];
}

int qc_datei_ende_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint8_t *grund) {
    if (!p || n < 5) return -1;
    *kennung = rd32(p);
    *grund = p[4];
    return 0;
}

NSData *qc_datei_quittung_kodieren(uint32_t kennung, uint8_t zustand, uint64_t empfangen) {
    uint8_t b[16] = {0};
    le32(b, kennung);
    b[4] = zustand;
    le64(b + 8, empfangen);
    return [NSData dataWithBytes:b length:sizeof b];
}

int qc_datei_quittung_lesen(const uint8_t *p, size_t n, uint32_t *kennung, uint8_t *zustand,
                            uint64_t *empfangen) {
    if (!p || n != 16) return -1;
    *kennung = rd32(p);
    *zustand = p[4];
    *empfangen = rd64(p + 8);
    return 0;
}

// ------------------------------------------------------ Ablageverzeichnis

void qc_dateien_basis_setzen(NSString *basis) {
    pthread_mutex_lock(&g_basis_mtx);
    g_basis = [basis copy];
    pthread_mutex_unlock(&g_basis_mtx);
}

NSString *qc_dateien_basis(void) {
    pthread_mutex_lock(&g_basis_mtx);
    NSString *b = g_basis;
    pthread_mutex_unlock(&g_basis_mtx);
    return b ?: [NSTemporaryDirectory() stringByAppendingPathComponent:@QC_DATEI_BASIS_NAME];
}

void qc_dateien_stillstand_setzen(uint32_t ms) { atomic_store(&g_stillstand_ms, ms ? ms : QC_DATEI_STILLSTAND_MS); }
void qc_dateien_platzreserve_setzen(uint64_t bytes) { atomic_store(&g_reserve, bytes); }
void qc_dateien_fertig_setzen(void (*fertig)(NSArray<NSString *> *)) { g_fertig = fertig; }

// Die Basis anlegen und pruefen: ein echter Ordner (keine Verknuepfung), der
// diesem Nutzer gehoert, Rechte 0700. nil = in Ordnung, sonst der Grund.
static NSString *basis_bereit(NSString *basis) {
    const char *p = basis.fileSystemRepresentation;
    if (mkdir(p, 0700) != 0 && errno != EEXIST)
        return [NSString stringWithFormat:@"Ablageverzeichnis %@ nicht anlegbar: %s", basis, strerror(errno)];
    struct stat st;
    if (lstat(p, &st) != 0)
        return [NSString stringWithFormat:@"Ablageverzeichnis %@ nicht lesbar: %s", basis, strerror(errno)];
    if (!S_ISDIR(st.st_mode))
        return [NSString stringWithFormat:@"Ablageverzeichnis %@ ist kein Ordner (oder eine Verknuepfung)", basis];
    if (st.st_uid != getuid())
        return [NSString stringWithFormat:@"Ablageverzeichnis %@ gehoert einem anderen Nutzer", basis];
    if ((st.st_mode & 07777) != 0700 && chmod(p, 0700) != 0)
        return [NSString stringWithFormat:@"Ablageverzeichnis %@: Rechte nicht auf 0700 setzbar: %s", basis, strerror(errno)];
    return nil;
}

// So tief steigt das Loeschen hoechstens unter die Wurzel hinab. Eine
// Uebertragung hat hoechstens TIEFE_MAX Stufen unter ihrem Verzeichnis; je
// Stufe bleibt beim Loeschen ein Deskriptor offen.
#define QC_LOESCHEN_TIEFE_MAX ((int)(2 * QC_DATEI_TIEFE_MAX))

static int eintrag_loeschen(int dfd, const char *name, dev_t geraet, int tiefe);

// Den Inhalt des offenen Ordners fd loeschen. Erst alle Namen lesen, dann
// loeschen: Loeschen waehrend readdir kann Eintraege ueberspringen. 0 = alles
// weg, sonst der erste errno.
static int inhalt_loeschen(int fd, dev_t geraet, int tiefe) {
    int kopie = dup(fd);
    DIR *d = kopie >= 0 ? fdopendir(kopie) : NULL;
    if (!d) {
        int err = errno;
        if (kopie >= 0) close(kopie);
        return err;
    }
    char **namen = NULL;
    size_t anzahl = 0, platz = 0;
    int fehler = 0;
    struct dirent *e;
    while ((e = readdir(d))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        if (anzahl == platz) {
            size_t neu = platz ? platz * 2 : 64;
            char **n = realloc(namen, neu * sizeof *namen);
            if (!n) { fehler = ENOMEM; break; }
            namen = n;
            platz = neu;
        }
        if (!(namen[anzahl] = strdup(e->d_name))) { fehler = ENOMEM; break; }
        anzahl++;
    }
    closedir(d);                                      // schliesst die Kopie, fd bleibt
    for (size_t i = 0; i < anzahl; i++) {
        int r = eintrag_loeschen(fd, namen[i], geraet, tiefe);
        if (r && !fehler) fehler = r;
        free(namen[i]);
    }
    free(namen);
    return fehler;
}

// Den Eintrag `name` im Ordner dfd loeschen, einen Ordner samt Inhalt. Alles
// relativ ueber Deskriptoren (openat, unlinkat): so gibt es keine Grenze fuer
// die Laenge des ganzen Pfads - ein Uebertragungsverzeichnis mit einem
// erlaubten relativen Pfad von 1024 Byte liegt absolut ueber PATH_MAX, und
// dort scheitern unlink/rmdir/opendir mit vollem Pfad. Kein chdir (der Host
// hat viele Faeden). Eine Verknuepfung verschwindet selbst, ihr Ziel bleibt;
// in einen Ordner auf einem anderen Geraet wird nicht abgestiegen.
// 0 = weg (oder schon weg), sonst errno.
static int eintrag_loeschen(int dfd, const char *name, dev_t geraet, int tiefe) {
    struct stat st;
    if (fstatat(dfd, name, &st, AT_SYMLINK_NOFOLLOW) != 0) return errno == ENOENT ? 0 : errno;
    if (!S_ISDIR(st.st_mode)) return unlinkat(dfd, name, 0) == 0 || errno == ENOENT ? 0 : errno;
    if (st.st_dev != geraet) return EXDEV;
    if (tiefe >= QC_LOESCHEN_TIEFE_MAX) return ELOOP;
    int fehler = 0;
    int fd = openat(dfd, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd >= 0) {
        fehler = inhalt_loeschen(fd, geraet, tiefe + 1);
        close(fd);
    } else if (errno == ELOOP || errno == ENOTDIR) {
        // Zwischen fstatat und openat gegen etwas anderes getauscht: es
        // selbst loeschen, nie hineinfolgen.
        return unlinkat(dfd, name, 0) == 0 || errno == ENOENT ? 0 : errno;
    } else {
        fehler = errno;                               // nicht lesbar: vielleicht leer
    }
    if (unlinkat(dfd, name, AT_REMOVEDIR) == 0 || errno == ENOENT) return 0;
    return fehler ? fehler : errno;
}

// Einen Baum loeschen, ohne je einer Verknuepfung zu folgen. Auch die Wurzel
// wird nur geloescht, nicht verfolgt, und ihr Elternordner (die Basis) wird
// nur geoeffnet, wenn er selbst keine Verknuepfung ist (O_NOFOLLOW) - ist die
// Basis inzwischen eine, bleibt ihr Ziel unberuehrt. 0 = alles weg.
static int baum_loeschen(NSString *pfad) {
    NSString *eltern = pfad.stringByDeletingLastPathComponent, *name = pfad.lastPathComponent;
    if (!eltern.length || !name.length || [name isEqualToString:@"/"]) return -1;
    int dfd = open(eltern.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (dfd < 0) return -1;
    struct stat st;
    int r = fstat(dfd, &st) == 0 ? eintrag_loeschen(dfd, name.fileSystemRepresentation, st.st_dev, 0) : errno;
    close(dfd);
    return r ? -1 : 0;
}

// "<unix-ms>-<kennung>": nur so heissen Uebertragungen. *ms = der Zeitpunkt.
static BOOL uebertragung_name(const char *n, uint64_t *ms) {
    const char *s = n;
    uint64_t v = 0;
    int ziffern = 0;
    while (*s >= '0' && *s <= '9') {
        if (++ziffern > 19) return NO;
        v = v * 10 + (uint64_t)(*s++ - '0');
    }
    if (!ziffern || *s++ != '-') return NO;
    ziffern = 0;
    while (*s >= '0' && *s <= '9') { if (++ziffern > 10) return NO; s++; }
    if (!ziffern || *s) return NO;
    *ms = v;
    return YES;
}

// Aufraeumen (2.9). ausser: Name eines Uebertragungsverzeichnisses, das auf
// keinen Fall geloescht wird - das eben in die Ablage gelegte. Es zaehlt als
// eines der `behalten`: Sortiert wird nach der Wanduhr im Namen, und springt
// die Uhr zurueck, staende das eben fertige sonst als aeltestes da und
// verschwaende, waehrend die Ablage noch darauf zeigt.
// Die Basis wird ohne Verknuepfung geoeffnet (O_NOFOLLOW), und alles darunter
// wird relativ zu ihrem Deskriptor gelesen und geloescht: Ist die Basis eine
// Verknuepfung, wird nichts angefasst, auch nicht in ihrem Ziel.
// Marken (<name>.laeuft, eine gewoehnliche Datei): Ein Verzeichnis mit Marke
// laeuft noch und wird weder gezaehlt noch geloescht (wie im Rust-Kern) -
// ausser nach hoechstalter_ms oder mit `unfertige` (beim Start: dann ist es
// eine Waise). Eine Marke geht mit ihrem Verzeichnis, nach hoechstalter_ms
// und mit `unfertige` auch allein.
static NSString *laufender_name(void);

static void aufraeumen_ausser(NSUInteger behalten, uint64_t hoechstalter_ms, NSString *ausser, BOOL unfertige) {
    if (!behalten && !hoechstalter_ms && !unfertige) return;
    NSString *basis = qc_dateien_basis();
    int bfd = open(basis.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (bfd < 0) {
        // Noch keine Basis: nichts zu tun. Sonst eine Verknuepfung (ELOOP),
        // etwas anderes als ein Ordner (ENOTDIR) oder nicht lesbar.
        if (errno != ENOENT)
            zeile(@"Dateien: Aufraeumen ausgelassen - %@ ist kein Ordner, eine Verknuepfung oder nicht lesbar (%s)",
                  basis, strerror(errno));
        return;
    }
    struct stat bst;
    if (fstat(bfd, &bst) != 0 || bst.st_uid != getuid()) {
        zeile(@"Dateien: Aufraeumen ausgelassen - %@ gehoert einem anderen Nutzer", basis);
        close(bfd);
        return;
    }
    int kopie = dup(bfd);
    DIR *d = kopie >= 0 ? fdopendir(kopie) : NULL;
    if (!d) {
        if (kopie >= 0) close(kopie);
        close(bfd);
        return;
    }
    uint64_t jetzt = unix_ms();
    BOOL (^alt)(uint64_t) = ^BOOL(uint64_t ms) { return hoechstalter_ms && ms + hoechstalter_ms < jetzt; };
    const size_t endung = strlen(QC_DATEI_MARKE_ENDUNG);
    BOOL ausser_da = NO;
    NSMutableArray<NSArray *> *funde = [NSMutableArray array];      // @[ms, Name] der Verzeichnisse
    NSMutableDictionary<NSString *, NSNumber *> *marken = [NSMutableDictionary dictionary];   // Stamm -> ms
    struct dirent *e;
    while ((e = readdir(d))) {
        uint64_t ms;
        struct stat st;
        size_t l = strlen(e->d_name);
        if (l > endung && !strcmp(e->d_name + l - endung, QC_DATEI_MARKE_ENDUNG)) {
            // "<unix-ms>-<kennung>" hat hoechstens 19 + 1 + 10 Zeichen.
            char stamm[32];
            if (l - endung >= sizeof stamm) continue;
            memcpy(stamm, e->d_name, l - endung);
            stamm[l - endung] = 0;
            // Nur eine gewoehnliche Datei ist eine Marke.
            if (uebertragung_name(stamm, &ms) && fstatat(bfd, e->d_name, &st, AT_SYMLINK_NOFOLLOW) == 0 &&
                S_ISREG(st.st_mode))
                marken[@(stamm)] = @(ms);
            continue;
        }
        if (!uebertragung_name(e->d_name, &ms)) continue;
        // Nur echte Ordner; eine Verknuepfung mit passendem Namen bleibt liegen.
        if (fstatat(bfd, e->d_name, &st, AT_SYMLINK_NOFOLLOW) != 0 || !S_ISDIR(st.st_mode)) continue;
        [funde addObject:@[ @(ms), @(e->d_name) ]];
    }
    closedir(d);                                      // schliesst die Kopie, bfd bleibt
    NSMutableArray<NSArray *> *fertige = [NSMutableArray array];
    NSUInteger waisen = 0;
    for (NSArray *f in funde) {
        NSString *name = f[1];
        BOOL markiert = marken[name] != nil;
        if (ausser && [name isEqualToString:ausser]) {
            ausser_da = !markiert;                    // zaehlt nur fertig als eines der `behalten`
            continue;
        }
        if (!markiert) { [fertige addObject:f]; continue; }
        // Laeuft (noch): nur als Waise beim Start oder nach dem Hoechstalter.
        if (!unfertige && !alt([f[0] unsignedLongLongValue])) continue;
        int r = eintrag_loeschen(bfd, name.UTF8String, bst.st_dev, 0);
        if (r) zeile(@"Dateien: Aufraeumen - %@ liess sich nicht ganz loeschen (%s)",
                     [basis stringByAppendingPathComponent:name], strerror(r));
        else if (unfertige) waisen++;
    }
    // Marken: die der eben geloeschten Waisen und alte; die der Ausnahme nie.
    for (NSString *stamm in marken) {
        if (ausser && [stamm isEqualToString:ausser]) continue;
        if (!unfertige && !alt([marken[stamm] unsignedLongLongValue])) continue;
        NSString *m = [stamm stringByAppendingString:@QC_DATEI_MARKE_ENDUNG];
        // Liess sich das Verzeichnis nicht ganz loeschen, bleibt die Marke:
        // sonst zaehlte der Rest beim naechsten Aufraeumen als fertig.
        struct stat st;
        if (fstatat(bfd, stamm.UTF8String, &st, AT_SYMLINK_NOFOLLOW) == 0 && S_ISDIR(st.st_mode)) continue;
        eintrag_loeschen(bfd, m.UTF8String, bst.st_dev, 0);
    }
    if (waisen)
        zeile(@"Dateien: %lu unfertige Uebertragung(en) eines frueheren Laufs geloescht", (unsigned long)waisen);
    [fertige sortUsingComparator:^NSComparisonResult(NSArray *a, NSArray *b) {
        NSComparisonResult r = [b[0] compare:a[0]];              // neueste zuerst
        return r != NSOrderedSame ? r : [b[1] compare:a[1]];
    }];
    // So viele der uebrigen bleiben; 0 bei `behalten` heisst: keine Obergrenze.
    NSUInteger andere = !behalten ? NSUIntegerMax : behalten - (ausser_da ? 1 : 0);
    for (NSUInteger i = 0; i < fertige.count; i++) {
        uint64_t ms = [fertige[i][0] unsignedLongLongValue];
        BOOL weg = i >= andere || alt(ms);
        if (!weg) continue;
        NSString *name = fertige[i][1];
        int r = eintrag_loeschen(bfd, name.UTF8String, bst.st_dev, 0);
        if (r) zeile(@"Dateien: Aufraeumen - %@ liess sich nicht ganz loeschen (%s)",
                     [basis stringByAppendingPathComponent:name], strerror(r));
    }
    close(bfd);
}

void qc_dateien_aufraeumen(NSUInteger behalten, uint64_t hoechstalter_ms) {
    aufraeumen_ausser(behalten, hoechstalter_ms, nil, NO);
}

void qc_dateien_aufraeumen_beim_start(void) {
    if (!g_empfang_q) return;
    dispatch_async(g_empfang_q, ^{
        @autoreleasepool {
            // Auf g_empfang_q: laeuft hier schon eine Uebertragung (der
            // Aufruf kam spaet), bleibt sie samt Marke.
            aufraeumen_ausser(0, 24ull * 3600 * 1000, laufender_name(), YES);
        }
    });
}

// -------------------------------------------------------------- Empfaenger

@interface QCEmpfang : NSObject {
@public
    uint64_t sitzung, kanal;
    uint32_t kennung;
    uint64_t gesamt, empfangen, quittiert;
    NSUInteger index;          // naechste Datei mit Inhalt
    uint64_t versatz;          // davon schon geschrieben
    int fd;                    // offen, solange sie beschrieben wird
    uint64_t letzte_ms;        // letztes Stueck (monoton)
}
@property (nonatomic, strong) NSArray<QCDateiEintrag *> *eintraege;
@property (nonatomic, copy) NSString *ordner;
@property (nonatomic, copy) NSString *marke;          // <ordner>.laeuft, solange sie laeuft
@property (nonatomic, strong) dispatch_source_t wache;
@end
@implementation QCEmpfang
@end

// Die laufende Uebertragung. Nur auf g_empfang_q.
static QCEmpfang *g_empfang = nil;

// Name ihres Verzeichnisses in der Basis, sonst nil. Nur auf g_empfang_q.
static NSString *laufender_name(void) { return g_empfang.ordner.lastPathComponent; }

// Eingang des Empfaengers: eine eigene Warteschlange (FIFO) vor g_empfang_q,
// damit sie sich nach Datenbytes begrenzen und von einem neuen Angebot leeren
// laesst - auf g_empfang_q eingereihte Bloecke liessen sich nicht mehr
// zurueckholen. Abgeholt wird je Block genau ein Eintrag, der naechste Block
// kommt danach wieder hinten an; so laufen Wache und Pruefstandsaufrufe
// dazwischen. Auch "Eingabekanal weg" und "Sitzung vorbei" laufen hier
// durch, damit sie Nachrichten, die vor ihnen kamen, nicht ueberholen.
enum {
    EINGANG_UEBERLAUF = 0,      // Warteschlange uebergelaufen: Quittung 4
    EINGANG_KANAL_WEG = 1,
    EINGANG_VORBEI    = 2,      // sitzung = die neue Sitzung
    // sonst QC_DATEI_ANGEBOT, QC_DATEI_STUECK, QC_DATEI_ENDE
};
@interface QCEingang : NSObject {
@public
    uint8_t art;
    uint64_t sitzung, kanal;
}
@property (nonatomic, strong) NSData *nutzlast;
@property (nonatomic, copy) NSString *grund;          // nur beim Ueberlauf
@end
@implementation QCEingang
@end

static pthread_mutex_t g_eingang_mtx = PTHREAD_MUTEX_INITIALIZER;
// Alles unter g_eingang_mtx; darunter wird nichts anderes gerufen.
static NSMutableArray<QCEingang *> *g_eingang = nil;
static size_t g_eingang_daten = 0;          // Datenbytes wartender Stuecke (je Stueck mindestens 1)
static unsigned g_eingang_enden = 0;        // wartende Enden
static int g_eingang_ueberlauf = 0;         // bis zum naechsten Angebot wird nichts angenommen
static int g_eingang_geplant = 0;           // ein Abholer steht auf g_empfang_q
static int g_empfang_angehalten = 0;        // nur Pruefstaende: g_empfang_q angehalten

// Was ein wartendes Stueck zaehlt: seine Daten, mindestens 1 (auch ein
// kaputtes ohne Daten).
static size_t stueck_daten(NSData *d) {
    return d.length > QC_DATEI_STUECK_KOPF + 1 ? d.length - QC_DATEI_STUECK_KOPF : 1;
}

// Wartet nichts mehr im Eingang? Der Empfaenger quittiert dann sofort, was
// offen ist - so bleibt kein Sender mit kleinem Fenster haengen, gleich wie
// er seine Stuecke schneidet (wie im Rust-Kern).
static BOOL eingang_leer(void) {
    pthread_mutex_lock(&g_eingang_mtx);
    BOOL leer = g_eingang.count == 0;
    pthread_mutex_unlock(&g_eingang_mtx);
    return leer;
}

// 1 = gesendet, 0 = die Sitzung ist vorbei (oder es gibt keinen Weg).
static int quittung_an(uint64_t sitzung, uint32_t kennung, uint8_t zustand, uint64_t empfangen) {
    NSData *q = qc_datei_quittung_kodieren(kennung, zustand, empfangen);
    return g_wege.senden ? g_wege.senden(sitzung, QC_DATEI_QUITTUNG, q.bytes, q.length) : 0;
}

// Die Marke einer Uebertragung entfernen - wie das Verzeichnis nur relativ
// zur Basis, ohne ihr zu folgen, falls sie inzwischen eine Verknuepfung ist.
// Scheitert es, steht es im Protokoll: Mit Marke gilt das Verzeichnis als
// unfertig, und der naechste Start loescht es als Waise.
static void marke_entfernen(NSString *marke) {
    if (marke && baum_loeschen(marke) != 0)
        zeile(@"Dateien: Marke %@ liess sich nicht loeschen - der naechste Start raeumt das Verzeichnis daneben weg", marke);
}

// Ein Uebertragungsverzeichnis verwerfen (loeschen). Die Marke geht nur mit,
// wenn nichts davon uebrig blieb: sonst zaehlte der Rest beim Aufraeumen als
// fertige Uebertragung - mit Marke raeumt ihn der naechste Start als Waise
// weg. Fuer jeden Weg, auf dem ein Empfang verworfen wird (auch das
// gescheiterte Anlegen in empfang_angebot).
static void verzeichnis_verwerfen(NSString *ordner, NSString *marke) {
    if (baum_loeschen(ordner) != 0) {
        zeile(@"Dateien: %@ liess sich nicht ganz loeschen", ordner);
        return;
    }
    marke_entfernen(marke);
}

// Die laufende Uebertragung beenden; loeschen = ihr Verzeichnis mit
// (verzeichnis_verwerfen). Sonst geht nur die Marke, falls sie noch da ist
// (empfang_ende entfernt sie schon vor dem Ablegen).
static void empfang_beenden(BOOL loeschen) {
    QCEmpfang *E = g_empfang;
    if (!E) return;
    g_empfang = nil;
    if (E->fd >= 0) { close(E->fd); E->fd = -1; }
    if (E.wache) { dispatch_source_cancel(E.wache); E.wache = nil; }
    if (loeschen) verzeichnis_verwerfen(E.ordner, E.marke);
    else marke_entfernen(E.marke);
}

static void naechste_datei(QCEmpfang *E) {
    while (E->index < E.eintraege.count &&
           (E.eintraege[E->index].art != 0 || E.eintraege[E->index].groesse == 0))
        E->index++;
}

// Den Elternordner eines Eintrags im Uebertragungsverzeichnis oeffnen,
// Bestandteil fuer Bestandteil und ohne je einer Verknuepfung zu folgen.
// Namen im Uebertragungsverzeichnis (hier und beim Anlegen und Oeffnen der
// Dateien): die Bestandteile, wie angebot_pruefen sie liefert (bereinigt,
// NFC), als UTF-8 - nicht ueber fileSystemRepresentation, das zerlegt (NFD).
// APFS legt die Bytes so an, wie sie kommen; ein zerlegter Name ginge bei
// einer Ruecksendung (Mac -> Windows) zerlegt hinaus und staende dort neben
// dem gleich aussehenden.
static int eltern_oeffnen(NSString *ordner, NSArray<NSString *> *teile) {
    int fd = open(ordner.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    for (NSUInteger i = 0; fd >= 0 && i + 1 < teile.count; i++) {
        int n = openat(fd, teile[i].UTF8String, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        int err = errno;
        close(fd);
        errno = err;
        fd = n;
    }
    return fd;
}

// Quittung 4 und verwerfen; `text` steht so in der Protokollzeile.
static void ungueltig_beenden(QCEmpfang *E, NSString *text) {
    zeile(@"Dateien: abgebrochen (%@)", text);
    quittung_an(E->sitzung, E->kennung, QC_QUITT_UNGUELTIG, E->empfangen);
    empfang_beenden(YES);
}

static void protokollfehler(QCEmpfang *E, NSString *warum) {
    ungueltig_beenden(E, [@"ungueltig: " stringByAppendingString:warum]);
}

static void schreibfehler(QCEmpfang *E, QCDateiEintrag *e, int err) {
    zeile(@"Dateien: abgebrochen (Schreibfehler bei %@: %s)", [e.teile componentsJoinedByString:@"/"], strerror(err));
    quittung_an(E->sitzung, E->kennung, QC_QUITT_SCHREIBFEHLER, E->empfangen);
    empfang_beenden(YES);
}

static void empfang_wache(QCEmpfang *E) {
    if (!E || E != g_empfang) return;
    // Sicherheitsnetz zu qc_dateien_sitzung_vorbei: gibt es die Sitzung nicht
    // mehr, kommt auch nichts mehr.
    if (g_wege.rueckstand && g_wege.rueckstand(E->sitzung) < 0) {
        zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
        empfang_beenden(YES);
        return;
    }
    uint32_t frist = atomic_load(&g_stillstand_ms);
    if (jetzt_ms() - E->letzte_ms >= frist) {
        zeile(@"Dateien: abgebrochen (seit %@ kein Stueck)", sek_text(frist / 1000.0));
        quittung_an(E->sitzung, E->kennung, QC_QUITT_ABGEBROCHEN, E->empfangen);
        empfang_beenden(YES);
    }
}

static void empfang_angebot(uint64_t sitzung, uint64_t kanal, NSData *d) {
    // Ein Nachzuegler einer Sitzung, die es nicht mehr gibt (der Eingabefaden
    // las ihn noch vor dem Wechsel): nichts anlegen, niemandem antworten -
    // und vor allem die laufende Uebertragung der aktuellen Sitzung nicht
    // verwerfen, deren Sender sonst ohne Quittung bis zum Stillstand wartete.
    if (g_wege.rueckstand && g_wege.rueckstand(sitzung) < 0) return;
    if (g_empfang) {
        zeile(@"Dateien: abgebrochen (neues Angebot vor dem Ende der laufenden Uebertragung)");
        empfang_beenden(YES);
    }
    uint32_t kennung = 0;
    uint64_t gesamt = 0;
    NSArray<QCDateiEintrag *> *eintraege = nil;
    NSString *grund = nil;
    int rc = qc_datei_angebot_lesen(d.bytes, d.length, &kennung, &gesamt, &eintraege, &grund);
    if (rc != QC_QUITT_LAEUFT) {
        zeile(@"Dateien: abgelehnt (%@)", grund);
        quittung_an(sitzung, kennung, (uint8_t)rc, 0);
        return;
    }
    NSString *basis = qc_dateien_basis();
    NSString *fehler = basis_bereit(basis);
    if (fehler) {
        zeile(@"Dateien: abgelehnt (%@)", fehler);
        quittung_an(sitzung, kennung, QC_QUITT_SCHREIBFEHLER, 0);
        return;
    }
    NSDictionary *fs = [[NSFileManager defaultManager] attributesOfFileSystemForPath:basis error:nil];
    NSNumber *frei_n = fs[NSFileSystemFreeSize];
    uint64_t frei = frei_n ? frei_n.unsignedLongLongValue : 0, reserve = atomic_load(&g_reserve);
    if (frei < gesamt || frei - gesamt < reserve) {
        zeile(@"Dateien: abgelehnt (zu wenig Platz: %@ frei, %@ angeboten, %@ sollen frei bleiben)",
              mb_text(frei), mb_text(gesamt), mb_text(reserve));
        quittung_an(sitzung, kennung, QC_QUITT_KEIN_PLATZ, 0);
        return;
    }
    // Frisches Verzeichnis je Uebertragung; darin alle Ordner und leeren
    // Dateien, jede exklusiv angelegt. Zuerst die Marke daneben: Endet der
    // Host mitten im Empfang, erkennt der naechste Start daran die Waise
    // (qc_dateien_aufraeumen_beim_start) - ohne Marke laege sie 24 h.
    NSString *name_u = [NSString stringWithFormat:@"%llu-%u", unix_ms(), kennung];
    NSString *ordner = [basis stringByAppendingPathComponent:name_u];
    NSString *marke = [ordner stringByAppendingString:@QC_DATEI_MARKE_ENDUNG];
    int mfd = open(marke.fileSystemRepresentation, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (mfd < 0) {
        zeile(@"Dateien: abgelehnt (Marke %@ nicht anlegbar: %s)", marke, strerror(errno));
        quittung_an(sitzung, kennung, QC_QUITT_SCHREIBFEHLER, 0);
        return;
    }
    close(mfd);
    if (mkdir(ordner.fileSystemRepresentation, 0700) != 0) {
        zeile(@"Dateien: abgelehnt (Verzeichnis %@ nicht anlegbar: %s)", ordner, strerror(errno));
        baum_loeschen(marke);
        quittung_an(sitzung, kennung, QC_QUITT_SCHREIBFEHLER, 0);
        return;
    }
    for (QCDateiEintrag *e in eintraege) {
        int ok = -1, err = 0;
        int pfd = eltern_oeffnen(ordner, e.teile);
        if (pfd >= 0) {
            const char *name = e.teile.lastObject.UTF8String;
            if (e.art == 1) {
                ok = mkdirat(pfd, name, 0755);
            } else {
                int f = openat(pfd, name, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0644);
                if (f >= 0) { close(f); ok = 0; }
            }
        }
        err = errno;
        if (pfd >= 0) close(pfd);
        if (ok != 0) {
            verzeichnis_verwerfen(ordner, marke);
            zeile(@"Dateien: abgelehnt (Schreibfehler bei %@: %s)", [e.teile componentsJoinedByString:@"/"], strerror(err));
            quittung_an(sitzung, kennung, QC_QUITT_SCHREIBFEHLER, 0);
            return;
        }
    }
    QCEmpfang *E = [[QCEmpfang alloc] init];
    E->sitzung = sitzung;
    E->kanal = kanal;
    E->kennung = kennung;
    E->gesamt = gesamt;
    E->fd = -1;
    E->letzte_ms = jetzt_ms();
    E.eintraege = eintraege;
    E.ordner = ordner;
    E.marke = marke;
    naechste_datei(E);
    // Stillstand: im Takt nachsehen, ob noch Stuecke kommen.
    uint64_t takt = atomic_load(&g_stillstand_ms) / 4;
    if (takt > 1000) takt = 1000;
    if (takt < 10) takt = 10;
    dispatch_source_t t = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, g_empfang_q);
    if (t) {
        dispatch_source_set_timer(t, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(takt * NSEC_PER_MSEC)),
                                  takt * NSEC_PER_MSEC, takt * NSEC_PER_MSEC / 4);
        __weak QCEmpfang *schwach = E;
        dispatch_source_set_event_handler(t, ^{ @autoreleasepool { empfang_wache(schwach); } });
        dispatch_resume(t);
        E.wache = t;
    }
    g_empfang = E;
    // Ging schon die Quittung 0 nicht hinaus, ist die Sitzung vorbei: gleich
    // verwerfen, nicht erst auf die Wache warten - und ohne die Zeile
    // "empfange", wie im Rust-Kern (dort nur, wenn die Uebertragung nach
    // der Quittung noch besteht).
    if (!quittung_an(sitzung, kennung, QC_QUITT_LAEUFT, 0)) {
        zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
        empfang_beenden(YES);
        return;
    }
    // Wie im Rust-Kern (und damit wie beim Windows-Host) nach der Quittung 0.
    zeile(@"Dateien: empfange %lu Eintraege, %@", (unsigned long)eintraege.count, mb_text(gesamt));
}

static void empfang_stueck(uint64_t sitzung, NSData *d) {
    QCEmpfang *E = g_empfang;
    // Nichts laeuft (mehr), oder es gehoert zu einer anderen Sitzung: ein
    // Nachzuegler einer verworfenen Uebertragung. Er wird still uebergangen.
    if (!E || E->sitzung != sitzung) return;
    uint32_t kennung, eintrag;
    uint64_t versatz;
    const uint8_t *daten;
    size_t dn;
    if (qc_datei_stueck_lesen(d.bytes, d.length, &kennung, &eintrag, &versatz, &daten, &dn) != 0) {
        if (d.length >= 4 && rd32(d.bytes) != E->kennung) return;
        protokollfehler(E, [NSString stringWithFormat:@"Stueck mit %lu Byte", (unsigned long)d.length]);
        return;
    }
    if (kennung != E->kennung) return;
    QCDateiEintrag *e = E->index < E.eintraege.count ? E.eintraege[E->index] : nil;
    if (!e || eintrag != E->index || versatz != E->versatz || dn > QC_DATEI_STUECK_MAX || versatz + dn > e.groesse) {
        protokollfehler(E, [NSString stringWithFormat:@"Stueck ausser der Reihe: Eintrag %u, Versatz %llu, %zu Byte - "
                                                      "erwartet Eintrag %lu, Versatz %llu",
                                                      eintrag, versatz, dn, (unsigned long)E->index, E->versatz]);
        return;
    }
    if (E->fd < 0) {
        int pfd = eltern_oeffnen(E.ordner, e.teile);
        E->fd = pfd >= 0 ? openat(pfd, e.teile.lastObject.UTF8String, O_WRONLY | O_NOFOLLOW | O_CLOEXEC) : -1;
        int err = errno;
        if (pfd >= 0) close(pfd);
        if (E->fd < 0) { schreibfehler(E, e, err); return; }
    }
    size_t weg = 0;
    while (weg < dn) {
        ssize_t w = write(E->fd, daten + weg, dn - weg);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) { schreibfehler(E, e, w < 0 ? errno : EIO); return; }
        weg += (size_t)w;
    }
    E->versatz += dn;
    E->empfangen += dn;
    E->letzte_ms = jetzt_ms();
    if (E->versatz == e.groesse) {
        int r = close(E->fd);
        E->fd = -1;
        if (r != 0) { schreibfehler(E, e, errno); return; }
        E->index++;
        E->versatz = 0;
        naechste_datei(E);
    }
    // Quittiert wird, sobald mehr als FENSTER_SPIEL - STUECK_MAX (16 KiB)
    // offen sind - "spaetestens je 64 KiB" (2.6) ist damit erfuellt. Grund:
    // Ein Sender, der nur ganze Stuecke ins Fenster setzt, steht im
    // Spielmodus schon mit einem Stueck (48 KiB) unterwegs; wartete der
    // Empfaenger auf 64 KiB, hingen beide bis zum Stillstand. Zusaetzlich,
    // wie im Rust-Kern, sobald im Eingang nichts mehr wartet: dann steht der
    // Sender womoeglich am Fenster, gleich wie er seine Stuecke schneidet.
    // Quittungen gehen hier nie wegen eines Staus verloren: der Weg (main.m,
    // send_small_sitzung) blockiert, bis sie im Sendepuffer liegen, und
    // scheitert nur, wenn die Sitzung vorbei ist.
    uint64_t offen = E->empfangen - E->quittiert;
    if (offen > QC_DATEI_FENSTER_SPIEL - QC_DATEI_STUECK_MAX || (offen && eingang_leer())) {
        E->quittiert = E->empfangen;
        quittung_an(E->sitzung, E->kennung, QC_QUITT_LAEUFT, E->empfangen);
    }
}

static void empfang_ende(uint64_t sitzung, NSData *d) {
    QCEmpfang *E = g_empfang;
    if (!E || E->sitzung != sitzung) return;
    uint32_t kennung;
    uint8_t grund;
    if (qc_datei_ende_lesen(d.bytes, d.length, &kennung, &grund) != 0) {
        if (d.length >= 4 && rd32(d.bytes) != E->kennung) return;
        protokollfehler(E, [NSString stringWithFormat:@"Ende mit %lu Byte", (unsigned long)d.length]);
        return;
    }
    if (kennung != E->kennung) return;
    if (grund != QC_ENDE_VOLLSTAENDIG) {
        zeile(@"Dateien: abgebrochen (Gegenseite: %@)", ende_text(grund));
        empfang_beenden(YES);
        return;
    }
    if (E->index < E.eintraege.count) {
        protokollfehler(E, @"Ende vor dem letzten Stueck");
        return;
    }
    // Vollstaendig; jede Datei wurde mit ihrem letzten Byte geschlossen.
    // Erst die Marke weg (wie im Rust-Kern), dann in die Ablage: Endet der
    // Host dazwischen, bleibt ein fertiges Verzeichnis, das das Aufraeumen
    // spaeter mitnimmt - nie eines, auf das die Ablage zeigt und das der
    // naechste Start als Waise loeschte.
    marke_entfernen(E.marke);
    E.marke = nil;
    // Die obersten Eintraege gehen als Dateiliste in die Ablage dieses Macs.
    NSMutableArray<NSString *> *oben = [NSMutableArray array];
    for (QCDateiEintrag *e in E.eintraege)
        if (e.teile.count == 1) [oben addObject:[E.ordner stringByAppendingPathComponent:e.teile[0]]];
    void (*fertig)(NSArray<NSString *> *) = g_fertig;
    if (fertig) fertig(oben);
    quittung_an(E->sitzung, E->kennung, QC_QUITT_FERTIG, E->empfangen);
    NSUInteger anzahl = E.eintraege.count;
    uint64_t bytes = E->empfangen;
    NSString *eben = E.ordner.lastPathComponent;
    empfang_beenden(NO);                  // das Verzeichnis bleibt: daraus wird eingefuegt
    aufraeumen_ausser(3, 0, eben, NO);    // das eben abgelegte bleibt auf jeden Fall
    zeile(@"Dateien: empfangen %lu Eintraege, %@ - in die Ablage gelegt", (unsigned long)anzahl, mb_text(bytes));
}

// Einen Eintrag verarbeiten. Nur auf g_empfang_q.
static void eingang_verarbeiten(QCEingang *e) {
    switch (e->art) {
        case QC_DATEI_ANGEBOT: empfang_angebot(e->sitzung, e->kanal, e.nutzlast); break;
        case QC_DATEI_STUECK:  empfang_stueck(e->sitzung, e.nutzlast); break;
        case QC_DATEI_ENDE:    empfang_ende(e->sitzung, e.nutzlast); break;
        case EINGANG_UEBERLAUF:
            // Die Gegenseite schickt mehr, als ihr Fenster erlaubt. Beendet
            // wird die laufende Uebertragung dieser Sitzung, gleich welche
            // Kennung das Stueck trug - sonst stuende sie still, bis der
            // Stillstand greift, denn bis zum naechsten Angebot wird nichts
            // mehr angenommen. Die Zeile lautet wie im Rust-Kern
            // ("Dateien: abgebrochen (mehr als das Fenster unquittiert)").
            if (g_empfang && g_empfang->sitzung == e->sitzung) ungueltig_beenden(g_empfang, e.grund);
            break;
        case EINGANG_KANAL_WEG:
            if (g_empfang && g_empfang->sitzung == e->sitzung && g_empfang->kanal == e->kanal) {
                zeile(@"Dateien: abgebrochen (Eingabekanal getrennt)");
                empfang_beenden(YES);
            }
            break;
        case EINGANG_VORBEI:
            if (g_empfang && g_empfang->sitzung != e->sitzung) {
                zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
                empfang_beenden(YES);
            }
            break;
        default: break;
    }
}

static void eingang_abholen(void);

static void abholer_planen(void) {
    dispatch_async(g_empfang_q, ^{ @autoreleasepool { eingang_abholen(); } });
}

// Den aeltesten Eintrag holen und verarbeiten; wartet danach noch mehr, kommt
// der naechste Abholer hinten an g_empfang_q. g_eingang_geplant faellt erst
// nach der Verarbeitung: solange es gesetzt ist, ist noch etwas zu tun
// (darauf wartet qc_dateien_abwarten).
static void eingang_abholen(void) {
    QCEingang *e = nil;
    pthread_mutex_lock(&g_eingang_mtx);
    if (g_eingang.count) {
        e = g_eingang[0];
        [g_eingang removeObjectAtIndex:0];
        if (e->art == QC_DATEI_STUECK) g_eingang_daten -= stueck_daten(e.nutzlast);
        else if (e->art == QC_DATEI_ENDE) g_eingang_enden--;
    }
    pthread_mutex_unlock(&g_eingang_mtx);
    if (e) eingang_verarbeiten(e);
    pthread_mutex_lock(&g_eingang_mtx);
    BOOL weiter = g_eingang.count > 0;
    if (!weiter) g_eingang_geplant = 0;
    pthread_mutex_unlock(&g_eingang_mtx);
    if (weiter) abholer_planen();
}

// Hinten anhaengen; 1 = es muss ein Abholer geplant werden. Unter g_eingang_mtx.
static int einreihen_gesperrt(QCEingang *e) {
    if (!g_eingang) g_eingang = [NSMutableArray array];
    [g_eingang addObject:e];
    if (g_eingang_geplant) return 0;
    g_eingang_geplant = 1;
    return 1;
}

static QCEingang *eingang_neu(uint8_t art, uint64_t sitzung, uint64_t kanal) {
    QCEingang *e = [[QCEingang alloc] init];
    e->art = art;
    e->sitzung = sitzung;
    e->kanal = kanal;
    return e;
}

void qc_empfang_nachricht(uint64_t sitzung, uint64_t kanal, uint8_t typ, NSData *nutzlast) {
    if (!g_empfang_q || !nutzlast) return;
    if (typ != QC_DATEI_ANGEBOT && typ != QC_DATEI_STUECK && typ != QC_DATEI_ENDE) return;
    QCEingang *e = eingang_neu(typ, sitzung, kanal);
    e.nutzlast = nutzlast;
    // Was ein Angebot verwirft, wird erst nach der Sperre freigegeben.
    NS_VALID_UNTIL_END_OF_SCOPE NSMutableArray<QCEingang *> *alt = nil;
    int planen = 0;
    pthread_mutex_lock(&g_eingang_mtx);
    switch (typ) {
        case QC_DATEI_ANGEBOT:
            // Der Sender hat alles davor aufgegeben: was aus seiner Sitzung
            // (oder einer aelteren) noch an Angeboten, Stuecken, Enden und
            // Ueberlaeufen wartet, faellt weg. Was aus einer neueren Sitzung
            // wartet, bleibt: Ein verspaetetes Angebot einer vergangenen
            // Sitzung (der Eingabefaden las es noch vor dem Wechsel) nimmt der
            // aktuellen nichts - empfang_angebot uebergeht es dann. Kanal-
            // und Sitzungsereignisse bleiben in ihrer Reihenfolge. Die Zaehler
            // gelten danach fuer das, was bleibt. Die Sperre nach einem
            // Ueberlauf faellt mit jedem Angebot; danach begrenzt wieder die
            // Menge (ein wartender Ueberlauf einer neueren Sitzung bleibt und
            // beendet deren Uebertragung trotzdem).
            alt = g_eingang;
            g_eingang = [NSMutableArray array];
            g_eingang_daten = 0;
            g_eingang_enden = 0;
            for (QCEingang *x in alt) {
                BOOL datei = x->art == QC_DATEI_ANGEBOT || x->art == QC_DATEI_STUECK || x->art == QC_DATEI_ENDE ||
                             x->art == EINGANG_UEBERLAUF;
                if (datei && x->sitzung <= sitzung) continue;
                [g_eingang addObject:x];
                if (x->art == QC_DATEI_STUECK) g_eingang_daten += stueck_daten(x.nutzlast);
                else if (x->art == QC_DATEI_ENDE) g_eingang_enden++;
            }
            g_eingang_ueberlauf = 0;
            planen = einreihen_gesperrt(e);
            break;
        case QC_DATEI_STUECK: {
            if (g_eingang_ueberlauf) break;
            size_t d = stueck_daten(nutzlast);
            if (g_eingang_daten + d > QC_EMPFANG_DATEN_MAX) {
                // Nicht weiter einreihen (der Speicher waere sonst der
                // Gegenseite ausgeliefert), sondern die Uebertragung als
                // ungueltig beenden; bis zum naechsten Angebot wird nichts
                // mehr angenommen.
                g_eingang_ueberlauf = 1;
                QCEingang *u = eingang_neu(EINGANG_UEBERLAUF, sitzung, kanal);
                u.grund = @"mehr als das Fenster unquittiert";
                planen = einreihen_gesperrt(u);
            } else {
                g_eingang_daten += d;
                planen = einreihen_gesperrt(e);
            }
            break;
        }
        case QC_DATEI_ENDE:
            if (g_eingang_ueberlauf) break;
            if (g_eingang_enden >= QC_EMPFANG_ENDEN_MAX) {
                g_eingang_ueberlauf = 1;
                QCEingang *u = eingang_neu(EINGANG_UEBERLAUF, sitzung, kanal);
                u.grund = @"zu viele Enden";
                planen = einreihen_gesperrt(u);
            } else {
                g_eingang_enden++;
                planen = einreihen_gesperrt(e);
            }
            break;
    }
    pthread_mutex_unlock(&g_eingang_mtx);
    alt = nil;
    if (planen) abholer_planen();
}

void qc_empfang_kanal_weg(uint64_t sitzung, uint64_t kanal) {
    if (!g_empfang_q) return;
    pthread_mutex_lock(&g_eingang_mtx);
    int planen = einreihen_gesperrt(eingang_neu(EINGANG_KANAL_WEG, sitzung, kanal));
    pthread_mutex_unlock(&g_eingang_mtx);
    if (planen) abholer_planen();
}

void qc_empfang_anhalten(int an) {
    if (!g_empfang_q) return;
    pthread_mutex_lock(&g_eingang_mtx);
    if (an && !g_empfang_angehalten) dispatch_suspend(g_empfang_q);
    if (!an && g_empfang_angehalten) dispatch_resume(g_empfang_q);
    g_empfang_angehalten = an ? 1 : 0;
    pthread_mutex_unlock(&g_eingang_mtx);
}

NSString *qc_empfang_ordner(void) {
    if (!g_empfang_q) return nil;
    __block NSString *o = nil;
    dispatch_sync(g_empfang_q, ^{ o = g_empfang.ordner; });
    return o;
}

// ------------------------------------------------------------------ Sender

static pthread_mutex_t g_s_mtx = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_s_cv = PTHREAD_COND_INITIALIZER;
// Jede neue Sendung und jeder Abbruch zaehlt hoch; eine Sendung, deren
// Nummer nicht mehr die aktuelle ist, endet.
static _Atomic uint64_t g_s_generation = 0;
static _Atomic int g_s_laeuft = 0;
static _Atomic uint32_t g_kennung_zaehler = 0;
// Die laufende Sendung, wie die Quittungen sie sehen. Unter g_s_mtx.
static uint64_t g_s_sitzung = 0;
static uint32_t g_s_kennung = 0;            // 0 = keine
static uint64_t g_s_gesendet = 0;           // Datenbytes, die hinausgingen (oder gerade gehen)
static uint64_t g_s_quittiert = 0;
static int g_s_zustand = 0;                 // erster Zustand ungleich 0
static int g_s_angenommen = 0;              // die erste Quittung (0/0 nach dem Angebot) ist da
static uint64_t g_s_quittung_ms = 0;        // letzter Fortschritt (Start, erste Quittung, mehr quittiert), monoton

enum { WARTEN_OK, WARTEN_UEBERHOLT, WARTEN_WEG, WARTEN_ZEIT, WARTEN_ZUSTAND };

static BOOL ueberholt(uint64_t gen) { return atomic_load(&g_s_generation) != gen; }

static void s_warten(void) {
    struct timespec kurz = { 0, 50 * 1000 * 1000 };
    pthread_cond_timedwait_relative_np(&g_s_cv, &g_s_mtx, &kurz);
}

void qc_senden_quittung(uint64_t sitzung, const uint8_t *p, size_t n) {
    uint32_t kennung;
    uint8_t zustand;
    uint64_t empfangen;
    if (qc_datei_quittung_lesen(p, n, &kennung, &zustand, &empfangen) != 0) return;
    pthread_mutex_lock(&g_s_mtx);
    if (g_s_kennung && kennung == g_s_kennung && sitzung == g_s_sitzung) {
        // Nie mehr als quittiert werten, als hinausging - sonst oeffnete eine
        // falsche Quittung das Fenster beliebig weit.
        if (empfangen > g_s_gesendet) empfangen = g_s_gesendet;
        // Nur Fortschritt erneuert die Stillstandsfrist (wie im Rust-Sender,
        // Griff::quittung): die erste Quittung nach dem Angebot oder mehr
        // quittiert als bisher. Eine Wiederholung desselben Standes haelt den
        // Sender nicht am Leben.
        if (!g_s_angenommen || empfangen > g_s_quittiert) {
            g_s_angenommen = 1;
            if (empfangen > g_s_quittiert) g_s_quittiert = empfangen;
            g_s_quittung_ms = jetzt_ms();
        }
        if (zustand != QC_QUITT_LAEUFT && !g_s_zustand) g_s_zustand = zustand;
        pthread_cond_broadcast(&g_s_cv);
    }
    pthread_mutex_unlock(&g_s_mtx);
}

void qc_senden_abbrechen(void) {
    atomic_fetch_add(&g_s_generation, 1);
    pthread_cond_broadcast(&g_s_cv);
}

int qc_senden_laeuft(void) { return atomic_load(&g_s_laeuft); }

// Warten, bis im Fenster Platz ist und im Sendepuffer hoechstens 128 KiB
// liegen. *frei = so viele Datenbytes duerfen jetzt hinaus.
static int platz_abwarten(uint64_t gen, uint64_t sitzung, uint64_t gesendet, uint64_t *frei, int *zustand) {
    uint32_t frist = atomic_load(&g_stillstand_ms);
    for (;;) {
        if (ueberholt(gen)) return WARTEN_UEBERHOLT;
        uint64_t fenster = g_wege.spielmodus && g_wege.spielmodus() ? QC_DATEI_FENSTER_SPIEL : QC_DATEI_FENSTER;
        pthread_mutex_lock(&g_s_mtx);
        uint64_t unterwegs = gesendet - g_s_quittiert;
        int z = g_s_zustand;
        BOOL still = jetzt_ms() - g_s_quittung_ms >= frist;
        BOOL voll = unterwegs >= fenster;
        if (!z && !still && voll) s_warten();
        pthread_mutex_unlock(&g_s_mtx);
        if (z) { *zustand = z; return WARTEN_ZUSTAND; }
        if (still) return WARTEN_ZEIT;
        int r = g_wege.rueckstand ? g_wege.rueckstand(sitzung) : 0;
        if (r < 0) return WARTEN_WEG;
        if (voll) continue;
        // Nachrang: liegt schon mehr als 128 KiB im Sendepuffer (Bilder,
        // Ton), wartet die Datei - so steht sie nie vor einem Bild an. Die
        // 2 ms halten nur, weil "dateien-senden" nicht mit UTILITY laeuft
        // (siehe qc_dateien_einrichten).
        if ((uint64_t)r > QC_DATEI_RUECKSTAND_MAX) { usleep(QC_DATEI_WARTEN_US); continue; }
        *frei = fenster - unterwegs;
        return WARTEN_OK;
    }
}

// Nach dem ENDE: auf die Quittung warten, hoechstens die Stillstandsfrist.
static int ende_abwarten(uint64_t gen, uint64_t sitzung, int *zustand) {
    uint32_t frist = atomic_load(&g_stillstand_ms);
    for (;;) {
        if (ueberholt(gen)) return WARTEN_UEBERHOLT;
        pthread_mutex_lock(&g_s_mtx);
        if (!g_s_zustand && jetzt_ms() - g_s_quittung_ms < frist) s_warten();
        int z = g_s_zustand;
        BOOL still = jetzt_ms() - g_s_quittung_ms >= frist;
        pthread_mutex_unlock(&g_s_mtx);
        if (z) { *zustand = z; return WARTEN_ZUSTAND; }
        if (still) return WARTEN_ZEIT;
        if (g_wege.rueckstand && g_wege.rueckstand(sitzung) < 0) return WARTEN_WEG;
    }
}

// Liegt einer der Pfade im eigenen Ablageverzeichnis? Dann stammt die Liste
// aus einem Empfang (Schutz gegen Widerhall, zusaetzlich zur Zaehlung).
static BOOL aus_eigenem_empfang(NSArray<NSString *> *pfade) {
    NSString *basis = qc_dateien_basis();
    char echt[PATH_MAX];
    NSMutableArray<NSString *> *wurzeln = [NSMutableArray arrayWithObject:basis.stringByStandardizingPath];
    if (realpath(basis.fileSystemRepresentation, echt)) [wurzeln addObject:@(echt)];
    for (NSString *p in pfade) {
        NSMutableArray<NSString *> *formen = [NSMutableArray arrayWithObject:p.stringByStandardizingPath];
        char r[PATH_MAX];
        if (realpath(p.fileSystemRepresentation, r)) [formen addObject:@(r)];
        for (NSString *f in formen)
            for (NSString *w in wurzeln)
                if ([f isEqualToString:w] || [f hasPrefix:[w stringByAppendingString:@"/"]]) return YES;
    }
    return NO;
}

// Die Namen im offenen Ordner fd, nach Namen sortiert (wie bisher mit
// compare:), je @[NSString, NSData mit den rohen Bytes samt NUL]. fd bleibt
// offen. nil = nicht lesbar (errno gesetzt).
static NSArray<NSArray *> *ordner_namen(int fd) {
    int kopie = dup(fd);
    DIR *d = kopie >= 0 ? fdopendir(kopie) : NULL;
    if (!d) {
        int err = errno;
        if (kopie >= 0) close(kopie);
        errno = err;
        return nil;
    }
    NSFileManager *fm = [NSFileManager defaultManager];
    NSMutableArray<NSArray *> *kinder = [NSMutableArray array];
    struct dirent *de;
    while ((de = readdir(d))) {
        if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) continue;
        size_t l = strlen(de->d_name);
        NSString *k = [fm stringWithFileSystemRepresentation:de->d_name length:l];
        if (!k) continue;
        [kinder addObject:@[ k, [NSData dataWithBytes:de->d_name length:l + 1] ]];
    }
    closedir(d);                                      // schliesst die Kopie, fd bleibt
    [kinder sortUsingComparator:^NSComparisonResult(NSArray *a, NSArray *b) { return [a[0] compare:b[0]]; }];
    return kinder;
}

// Ein Eintrag samt Inhalt (Ordner rekursiv, nach Namen sortiert).
// oben: der oberste Pfad, so wie der Nutzer ihn kopiert hat; kette: die
// rohen Namen darunter bis zu diesem Eintrag (je mit NUL, leer fuer einen
// obersten). dfd >= 0: der Eintrag heisst kette.lastObject im schon
// geoeffneten Elternordner dfd und wird relativ dazu angesehen (fstatat,
// openat, beides ohne einer Verknuepfung zu folgen). dfd < 0 nur fuer die
// obersten Pfade: die kommen per lstat ueber den ganzen Pfad.
// So stammt alles unter einem obersten Ordner nachweislich aus diesem Baum:
// Ein Ordner wird ueber einen Deskriptor gelesen, dessen st_dev/st_ino die
// eben gesehenen sind; wurde er dazwischen getauscht (etwa gegen eine
// Verknuepfung auf einen fremden Ordner), geht er leer hinaus. st_dev/st_ino
// jeder Datei und den Weg zu ihr (oben, kette) merkt sich der Eintrag fuer
// das Lesen (zum_lesen_oeffnen) - dort wird genauso geoeffnet wie hier.
// 1 = weiter, 0 = ueber EINTRAEGE_MAX oder GESAMT_MAX, -1 = ueberholt.
static int auflisten(uint64_t gen, int dfd, NSString *oben, NSArray<NSData *> *kette, NSString *quelle, NSString *rel,
                     NSString *name, NSUInteger tiefe, NSMutableArray<QCDateiEintrag *> *liste, uint64_t *gesamt) {
    if (ueberholt(gen)) return -1;
    NSData *pfad = [rel dataUsingEncoding:NSUTF8StringEncoding];
    // NSLiteralSearch: ein '\' vor einem kombinierenden Zeichen ist sonst
    // keiner, und die Gegenseite lehnte das ganze Angebot ab.
    if (!pfad || pfad.length > QC_DATEI_PFAD_MAX || tiefe > QC_DATEI_TIEFE_MAX ||
        [name lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > QC_DATEI_BESTANDTEIL_MAX ||
        [name rangeOfString:@"\\" options:NSLiteralSearch].location != NSNotFound) {
        // Die Gegenseite lehnte sonst das ganze Angebot ab.
        zeile(@"Dateien: uebersprungen (Name oder Pfad fuer die Gegenseite ungueltig): %@", quelle);
        return 1;
    }
    const char *roh = dfd >= 0 ? kette.lastObject.bytes : NULL;
    struct stat st;
    int rc = dfd >= 0 ? fstatat(dfd, roh, &st, AT_SYMLINK_NOFOLLOW) : lstat(quelle.fileSystemRepresentation, &st);
    if (rc != 0) {
        zeile(@"Dateien: uebersprungen (nicht lesbar: %s): %@", strerror(errno), quelle);
        return 1;
    }
    if (S_ISLNK(st.st_mode)) {
        zeile(@"Dateien: uebersprungen (symbolische Verknuepfung): %@", quelle);
        return 1;
    }
    if (!S_ISREG(st.st_mode) && !S_ISDIR(st.st_mode)) {
        zeile(@"Dateien: uebersprungen (besondere Datei): %@", quelle);
        return 1;
    }
    QCDateiEintrag *e = [[QCDateiEintrag alloc] init];
    e.art = S_ISDIR(st.st_mode) ? 1 : 0;
    e.groesse = e.art ? 0 : (uint64_t)st.st_size;
    e.pfad = pfad;
    e.quelle = quelle;
    e.oben = oben;
    e.kette = kette;
    e.geraet = (uint64_t)st.st_dev;
    e.knoten = (uint64_t)st.st_ino;
    [liste addObject:e];
    *gesamt += e.groesse;
    if (liste.count > QC_DATEI_EINTRAEGE_MAX || *gesamt > QC_DATEI_GESAMT_MAX) return 0;
    if (!e.art) return 1;
    int fd = dfd >= 0 ? openat(dfd, roh, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                      : open(quelle.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    struct stat ost;
    NSArray<NSArray *> *kinder = nil;
    BOOL getauscht = NO;
    int err = 0;
    if (fd < 0 || fstat(fd, &ost) != 0) err = errno;
    else if (ost.st_dev != st.st_dev || ost.st_ino != st.st_ino) getauscht = YES;
    else if (!(kinder = ordner_namen(fd))) err = errno;
    if (!kinder) {
        zeile(@"Dateien: Ordner nicht lesbar (%s), er geht leer hinaus: %@",
              getauscht ? "seit dem Ansehen getauscht" : strerror(err), quelle);
        if (fd >= 0) close(fd);
        return 1;
    }
    NSMutableSet<NSString *> *gesehen = [NSMutableSet set];
    int r = 1;
    for (NSArray *kind in kinder) {
        NSString *k = kind[0];
        NSString *kname = [k precomposedStringWithCanonicalMapping];
        // Auf einem Volume mit Gross- und Kleinschreibung koennen "A" und "a"
        // nebeneinander liegen, ebenso "a." und "a_" oder "a\x01" und "a_";
        // die Gegenseite lehnte dann alles ab.
        NSString *ks = sende_schluessel(kname);
        if (![gesehen containsObject:ks]) {
            [gesehen addObject:ks];
        } else {
            zeile(@"Dateien: uebersprungen (Name nach Bereinigung oder bis auf Gross- und Kleinschreibung doppelt): %@",
                  [quelle stringByAppendingPathComponent:k]);
            continue;
        }
        r = auflisten(gen, fd, oben, [kette arrayByAddingObject:kind[1]], [quelle stringByAppendingPathComponent:k],
                      [rel stringByAppendingFormat:@"/%@", kname], kname, tiefe + 1, liste, gesamt);
        if (r <= 0) break;
    }
    close(fd);
    return r;
}

// Die Datei eines Eintrags zum Lesen oeffnen, auf demselben Weg wie beim
// Auflisten: den obersten Pfad so, wie der Nutzer ihn kopiert hat, darunter
// Bestandteil fuer Bestandteil die rohen Namen relativ zum Ordner davor
// (openat), keiner Verknuepfung folgend (O_NOFOLLOW auf jeder Stufe). Nicht
// ueber den ganzen Pfad (e.quelle): Der kann absolut ueber PATH_MAX liegen,
// obwohl der relative die erlaubten 1024 Byte hat - dann scheiterte das
// Oeffnen immer (ENAMETOOLONG), und die ganze Sendung endete mit Grund 2.
// Und die Namen sind dieselben Bytes wie beim Auflisten, nicht erst ueber
// fileSystemRepresentation umgewandelt (zerlegt) - auf einem Laufwerk, das
// die Normalform unterscheidet, faende das die Datei sonst nicht.
// O_NONBLOCK: siehe senden_daten. -1 = nicht zu oeffnen (errno gesetzt).
static int zum_lesen_oeffnen(QCDateiEintrag *e) {
    const int datei = O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK;
    const int ordner = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
    NSArray<NSData *> *kette = e.kette;
    if (!kette.count) return open(e.oben.fileSystemRepresentation, datei);
    int fd = open(e.oben.fileSystemRepresentation, ordner);
    for (NSUInteger i = 0; fd >= 0 && i + 1 < kette.count; i++) {
        int n = openat(fd, kette[i].bytes, ordner);
        int err = errno;
        close(fd);
        errno = err;
        fd = n;
    }
    if (fd < 0) return -1;
    int f = openat(fd, kette.lastObject.bytes, datei);
    int err = errno;
    close(fd);
    errno = err;
    return f;
}

static int senden_daten(uint64_t gen, uint64_t sitzung, uint32_t kennung, NSArray<QCDateiEintrag *> *liste,
                        NSData *angebot) {
    if (!g_wege.senden) return 0;
    // Auch das Angebot (bis 1 MiB am Stueck) hat Nachrang: erst wenn im
    // Sendepuffer hoechstens 128 KiB liegen. Es ist noch nichts unterwegs,
    // also gibt es kein ENDE, wenn es hier endet.
    {
        uint64_t frei = 0;
        int z = 0;
        switch (platz_abwarten(gen, sitzung, 0, &frei, &z)) {
            case WARTEN_OK: break;
            case WARTEN_UEBERHOLT:
                zeile(@"Dateien: abgebrochen (neuer Inhalt in der Ablage)");
                return 0;
            case WARTEN_ZEIT:
                zeile(@"Dateien: abgebrochen (seit %@ Sendepuffer ueber 128 KiB)",
                      sek_text(atomic_load(&g_stillstand_ms) / 1000.0));
                return 0;
            case WARTEN_ZUSTAND:
                zeile(@"Dateien: abgebrochen (Gegenseite: %@)", zustand_text(z));
                return 0;
            default:
                zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
                return 0;
        }
    }
    if (!g_wege.senden(sitzung, QC_DATEI_ANGEBOT, angebot.bytes, angebot.length)) {
        zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
        return 0;
    }
    size_t cap = QC_DATEI_STUECK_KOPF + QC_DATEI_STUECK_MAX;
    uint8_t *puffer = malloc(cap);
    NSString *warum = nil;
    BOOL abgelehnt = NO, ende_senden = YES;
    uint8_t grund = QC_ENDE_ABGEBROCHEN;
    uint64_t gesendet = 0;
    if (!puffer) warum = @"kein Speicher";
    for (NSUInteger i = 0; i < liste.count && !warum; i++) {
        QCDateiEintrag *e = liste[i];
        if (e.art != 0 || e.groesse == 0) continue;
        // O_NONBLOCK: Wurde die Datei seit dem Auflisten gegen eine FIFO
        // getauscht, blockierte open sonst die Warteschlange auf Dauer. Erst
        // nach der Pruefung auf eine gewoehnliche Datei wieder blockierend.
        // Geoeffnet wird Stufe fuer Stufe ohne Verknuepfung
        // (zum_lesen_oeffnen); ein Ordner darueber, der seit dem Auflisten
        // gegen eine Verknuepfung getauscht wurde, scheitert schon dort.
        // Wurde er gegen einen anderen echten Ordner getauscht, laege dort
        // eine fremde Datei gleichen Namens. Deshalb muss die offene Datei
        // die beim Auflisten gesehene sein (st_dev/st_ino), sonst Grund 2 -
        // von einer anderen geht kein Byte hinaus.
        int fd = zum_lesen_oeffnen(e);
        struct stat st;
        NSString *fehler = nil;
        if (fd < 0)
            fehler = @(strerror(errno));
        else if (fstat(fd, &st) != 0 || !S_ISREG(st.st_mode))
            fehler = @"keine Datei mehr";
        else if ((uint64_t)st.st_dev != e.geraet || (uint64_t)st.st_ino != e.knoten)
            fehler = @"nicht mehr die aufgelistete Datei (seit dem Auflisten verlegt oder getauscht)";
        else if (fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK) != 0)
            fehler = @(strerror(errno));
        if (fehler) {
            warum = [NSString stringWithFormat:@"Lesefehler bei %@: %@", e.quelle, fehler];
            grund = QC_ENDE_LESEFEHLER;
            if (fd >= 0) close(fd);
            break;
        }
        uint64_t versatz = 0;
        while (versatz < e.groesse && !warum) {
            uint64_t frei = 0;
            int z = 0;
            int w = platz_abwarten(gen, sitzung, gesendet, &frei, &z);
            if (w == WARTEN_UEBERHOLT) { warum = @"neuer Inhalt in der Ablage"; break; }
            if (w == WARTEN_WEG) { warum = @"Zuschauer gewechselt oder weg"; ende_senden = NO; break; }
            if (w == WARTEN_ZEIT) {
                warum = [NSString stringWithFormat:@"seit %@ keine Quittung", sek_text(atomic_load(&g_stillstand_ms) / 1000.0)];
                grund = QC_ENDE_ZEIT;
                break;
            }
            if (w == WARTEN_ZUSTAND) {
                // Eine Quittung ungleich 0 beendet ohne ENDE.
                warum = [NSString stringWithFormat:@"Gegenseite: %@", zustand_text(z)];
                abgelehnt = z == QC_QUITT_ZU_GROSS || z == QC_QUITT_KEIN_PLATZ || z == QC_QUITT_UNGUELTIG;
                ende_senden = NO;
                break;
            }
            // So viel, wie in das Fenster passt: mit einem kleineren Rest
            // fuellt der Sender es genau, nie darueber hinaus.
            uint64_t rest = e.groesse - versatz;
            size_t n = (size_t)(rest < QC_DATEI_STUECK_MAX ? rest : QC_DATEI_STUECK_MAX);
            if (n > frei) n = (size_t)frei;
            size_t gelesen = 0;
            int lesefehler = 0;
            while (gelesen < n) {
                ssize_t r = pread(fd, puffer + QC_DATEI_STUECK_KOPF + gelesen, n - gelesen, (off_t)(versatz + gelesen));
                if (r < 0 && errno == EINTR) continue;
                if (r < 0) lesefehler = errno;
                if (r <= 0) break;
                gelesen += (size_t)r;
            }
            if (gelesen < n) {
                warum = lesefehler ? [NSString stringWithFormat:@"Lesefehler bei %@: %s", e.quelle, strerror(lesefehler)]
                                   : [NSString stringWithFormat:@"Lesefehler: %@ ist kuerzer als angekuendigt", e.quelle];
                grund = QC_ENDE_LESEFEHLER;
                break;
            }
            le32(puffer, kennung);
            le32(puffer + 4, (uint32_t)i);
            le64(puffer + 8, versatz);
            pthread_mutex_lock(&g_s_mtx);
            g_s_gesendet = gesendet + n;       // vorher: eine schnelle Quittung darf schon zaehlen
            pthread_mutex_unlock(&g_s_mtx);
            // Ein Stueck je Aufruf: g_send_mtx wird nur fuer dieses gehalten.
            if (!g_wege.senden(sitzung, QC_DATEI_STUECK, puffer, QC_DATEI_STUECK_KOPF + n)) {
                warum = @"Zuschauer gewechselt oder weg";
                ende_senden = NO;
                break;
            }
            versatz += n;
            gesendet += n;
        }
        close(fd);
    }
    if (puffer) { speicher_loeschen(puffer, cap); free(puffer); }
    if (warum) {
        if (ende_senden) {
            NSData *ende = qc_datei_ende_kodieren(kennung, grund);
            g_wege.senden(sitzung, QC_DATEI_ENDE, ende.bytes, ende.length);
        }
        zeile(@"Dateien: %@ (%@)", abgelehnt ? @"abgelehnt" : @"abgebrochen", warum);
        return 0;
    }
    NSData *ende = qc_datei_ende_kodieren(kennung, QC_ENDE_VOLLSTAENDIG);
    if (!g_wege.senden(sitzung, QC_DATEI_ENDE, ende.bytes, ende.length)) {
        zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
        return 0;
    }
    int z = 0;
    int w = ende_abwarten(gen, sitzung, &z);
    if (w == WARTEN_ZUSTAND && z == QC_QUITT_FERTIG) return 1;
    if (w == WARTEN_ZUSTAND)
        zeile(@"Dateien: %@ (Gegenseite: %@)",
              z == QC_QUITT_ZU_GROSS || z == QC_QUITT_KEIN_PLATZ || z == QC_QUITT_UNGUELTIG ? @"abgelehnt" : @"abgebrochen",
              zustand_text(z));
    else if (w == WARTEN_ZEIT)
        zeile(@"Dateien: abgebrochen (seit %@ keine Quittung nach dem Ende)", sek_text(atomic_load(&g_stillstand_ms) / 1000.0));
    else if (w == WARTEN_WEG)
        zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
    else
        zeile(@"Dateien: abgebrochen (neuer Inhalt in der Ablage, vor der letzten Quittung)");
    return 0;
}

static void senden_lauf(uint64_t gen, uint64_t sitzung, NSArray<NSString *> *pfade, NSString *an) {
    if (ueberholt(gen)) return;
    if (aus_eigenem_empfang(pfade)) {
        zeile(@"Dateien: nicht gesendet (stammen aus einem Empfang dieses Hosts)");
        return;
    }
    NSMutableArray<QCDateiEintrag *> *liste = [NSMutableArray array];
    NSMutableSet<NSString *> *oben = [NSMutableSet set];
    uint64_t gesamt = 0;
    int r = 1;
    for (NSString *p in pfade) {
        NSString *name = [p.lastPathComponent precomposedStringWithCanonicalMapping];
        if (!name.length || [name isEqualToString:@"/"]) {
            zeile(@"Dateien: uebersprungen (kein Name): %@", p);
            continue;
        }
        // Dateien aus mehreren Ordnern koennen gleich heissen, auch erst nach
        // der Windows-Bereinigung ("t." und "t_"): nur die erste.
        NSString *ks = sende_schluessel(name);
        if ([oben containsObject:ks]) {
            zeile(@"Dateien: doppelter Name %@ - nur der erste wird gesendet", name);
            continue;
        }
        [oben addObject:ks];
        r = auflisten(gen, -1, p, @[], p, name, name, 1, liste, &gesamt);
        if (r <= 0) break;
    }
    if (r < 0) return;
    if (r == 0) {
        zeile(@"Dateien: abgelehnt (mehr als 4 GB oder 10000 Eintraege) - nichts gesendet");
        return;
    }
    if (!liste.count) {
        zeile(@"Dateien: nichts zu senden");
        return;
    }
    uint32_t kennung;
    do kennung = atomic_fetch_add(&g_kennung_zaehler, 1) + 1; while (!kennung);
    NSData *angebot = qc_datei_angebot_kodieren(kennung, liste);
    if (!angebot || angebot.length > QC_DATEI_ANGEBOT_MAX) {
        zeile(@"Dateien: abgelehnt (Liste der Eintraege ueber 1 MiB) - nichts gesendet");
        return;
    }
    pthread_mutex_lock(&g_s_mtx);
    g_s_sitzung = sitzung;
    g_s_kennung = kennung;
    g_s_gesendet = 0;
    g_s_quittiert = 0;
    g_s_zustand = 0;
    g_s_angenommen = 0;
    g_s_quittung_ms = jetzt_ms();
    pthread_mutex_unlock(&g_s_mtx);
    atomic_store(&g_s_laeuft, 1);
    zeile(@"Dateien: sende %lu Eintraege, %@ an %@", (unsigned long)liste.count, mb_text(gesamt), an);
    uint64_t t0 = jetzt_ms();
    int ok = senden_daten(gen, sitzung, kennung, liste, angebot);
    pthread_mutex_lock(&g_s_mtx);
    g_s_kennung = 0;                      // spaete Quittungen gehoeren zu nichts mehr
    pthread_mutex_unlock(&g_s_mtx);
    atomic_store(&g_s_laeuft, 0);
    if (ok) zeile(@"Dateien: gesendet und quittiert (%@ in %@)", mb_text(gesamt), sek_text((jetzt_ms() - t0) / 1000.0));
}

void qc_senden_starten(uint64_t sitzung, NSArray<NSString *> *pfade, NSString *an) {
    if (!g_senden_q || !pfade.count) return;
    // Bricht eine laufende Sendung ab; die neue kommt auf der seriellen
    // Warteschlange erst danach dran.
    uint64_t gen = atomic_fetch_add(&g_s_generation, 1) + 1;
    pthread_cond_broadcast(&g_s_cv);
    NSArray<NSString *> *kopie = [pfade copy];
    NSString *wer = [an copy] ?: @"?";
    dispatch_async(g_senden_q, ^{
        @autoreleasepool { senden_lauf(gen, sitzung, kopie, wer); }
    });
}

// ---------------------------------------------------------------- Betrieb

void qc_dateien_einrichten(const qc_dateien_wege *wege) {
    if (wege) g_wege = *wege;
    // Latenz vor Bandbreite: Beide Warteschlangen laufen mit niedriger
    // Prioritaet, damit Plattenarbeit und Stuecke Aufnahme, Encoder und Ton
    // nicht verdraengen. Ohne eigene Klasse erbten die Bloecke die des
    // Aufrufers (Eingabefaden, Warteschlange der Ablage).
    // - "dateien-empfang": QOS_CLASS_UTILITY (Prioritaet 20). Er wartet nie
    //   auf eine Uhr, nur auf Arbeit und auf send().
    // - "dateien-senden": QOS_CLASS_DEFAULT mit relativer Prioritaet -15
    //   (gemessen 21, knapp ueber UTILITY, weit unter DEFAULT mit 31). Nicht
    //   UTILITY: Dort legt macOS Zeitgeber zusammen, und die 2 ms, die der
    //   Sender bei vollem Sendepuffer wartet (Spezifikation 3.6), dauern
    //   rund 10-12 ms statt knapp 3 ms - im LAN, wo der Sendepuffer die
    //   Menge begrenzt, fiele der Durchsatz Host -> Client auf ein Drittel
    //   bis die Haelfte.
    dispatch_queue_attr_t empfang =
        dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_UTILITY, 0);
    dispatch_queue_attr_t senden =
        dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_DEFAULT, QOS_MIN_RELATIVE_PRIORITY);
    if (!g_empfang_q) g_empfang_q = dispatch_queue_create("tech.quadchroma.dateien-empfang", empfang);
    if (!g_senden_q) g_senden_q = dispatch_queue_create("tech.quadchroma.dateien-senden", senden);
}

void qc_dateien_sitzung_vorbei(uint64_t neue_sitzung) {
    // Den Sender wecken: er merkt am Rueckstand (-1), dass seine Sitzung weg
    // ist, und muss dafuer nicht erst eine Quittung abwarten.
    pthread_cond_broadcast(&g_s_cv);
    if (!g_empfang_q) return;
    // Hinter allem, was noch von der alten Sitzung wartet (das laeuft ins
    // Leere oder in die Uebertragung, die hier gleich endet). Nur ein Griff an
    // g_eingang_mtx, kein Freigeben unter g_send_mtx.
    pthread_mutex_lock(&g_eingang_mtx);
    int planen = einreihen_gesperrt(eingang_neu(EINGANG_VORBEI, neue_sitzung, 0));
    pthread_mutex_unlock(&g_eingang_mtx);
    if (planen) abholer_planen();
}

void qc_dateien_abwarten(void) {
    for (;;) {
        if (g_empfang_q) dispatch_sync(g_empfang_q, ^{});
        if (g_senden_q) dispatch_sync(g_senden_q, ^{});
        if (g_empfang_q) dispatch_sync(g_empfang_q, ^{});
        // Abgeholt wird ein Eintrag je Block: erst fertig, wenn kein
        // Abholer mehr ansteht.
        pthread_mutex_lock(&g_eingang_mtx);
        int ruhig = !g_eingang_geplant;
        pthread_mutex_unlock(&g_eingang_mtx);
        if (ruhig) return;
    }
}
