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

// Was hoechstens auf der Empfangswarteschlange warten darf. Ein Sender, der
// sich an das Fenster haelt, hat nie mehr als 256 KiB unquittiert unterwegs;
// dazu kommt hoechstens ein Angebot. Wer mehr schickt, haelt sich nicht daran.
#define QC_EMPFANG_WARTEND_MAX (8u * 1024u * 1024u)

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

// Vergleichsschluessel fuer Doppelte: ohne Ruecksicht auf Gross- und
// Kleinschreibung - und auf die Unicode-Normalform, denn APFS unterscheidet
// ein e mit Akzent als ein Zeichen und als e plus kombinierenden Akzent
// ebenfalls nicht. Ein solches Paar schluege sonst erst beim exklusiven
// Anlegen fehl.
static NSString *schluessel(NSString *pfad) {
    return [[pfad precomposedStringWithCanonicalMapping] stringByFoldingWithOptions:NSCaseInsensitiveSearch locale:nil];
}

// Derselbe Schluessel beim Sender, fuer einen einzelnen Namen: erst so
// bereinigt, wie es jeder Empfaenger mindestens tut (Steuerzeichen zu '_'),
// sonst gingen "a\x01" und "a_" beide hinaus, und die Gegenseite lehnte das
// ganze Angebot als doppelt ab.
static NSString *sende_schluessel(NSString *name) {
    return schluessel(qc_datei_bereinigen(name));
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
        NSString *text = [[NSString alloc] initWithBytes:s length:pl encoding:NSUTF8StringEncoding];
        if (!text) ABLEHNEN(QC_QUITT_UNGUELTIG, @"Eintrag %u: kein gueltiges UTF-8", i);
        NSMutableArray<NSString *> *teile = [NSMutableArray array];
        for (NSString *t in [text componentsSeparatedByString:@"/"]) [teile addObject:qc_datei_bereinigen(t)];
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
    return b ?: [NSTemporaryDirectory() stringByAppendingPathComponent:@"QuadChroma-Ablage"];
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
// wird nur geloescht, nicht verfolgt. 0 = alles weg.
static int baum_loeschen(NSString *pfad) {
    NSString *eltern = pfad.stringByDeletingLastPathComponent, *name = pfad.lastPathComponent;
    if (!eltern.length || !name.length || [name isEqualToString:@"/"]) return -1;
    int dfd = open(eltern.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
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

void qc_dateien_aufraeumen(NSUInteger behalten, uint64_t hoechstalter_ms) {
    if (!behalten && !hoechstalter_ms) return;
    NSString *basis = qc_dateien_basis();
    DIR *d = opendir(basis.fileSystemRepresentation);
    if (!d) return;
    NSMutableArray<NSArray *> *funde = [NSMutableArray array];
    struct dirent *e;
    while ((e = readdir(d))) {
        uint64_t ms;
        if (!uebertragung_name(e->d_name, &ms)) continue;
        struct stat st;
        // Nur echte Ordner; eine Verknuepfung mit passendem Namen bleibt liegen.
        if (fstatat(dirfd(d), e->d_name, &st, AT_SYMLINK_NOFOLLOW) != 0 || !S_ISDIR(st.st_mode)) continue;
        [funde addObject:@[ @(ms), @(e->d_name) ]];
    }
    closedir(d);
    [funde sortUsingComparator:^NSComparisonResult(NSArray *a, NSArray *b) {
        NSComparisonResult r = [b[0] compare:a[0]];              // neueste zuerst
        return r != NSOrderedSame ? r : [b[1] compare:a[1]];
    }];
    uint64_t jetzt = unix_ms();
    for (NSUInteger i = 0; i < funde.count; i++) {
        uint64_t ms = [funde[i][0] unsignedLongLongValue];
        BOOL weg = (behalten && i >= behalten) || (hoechstalter_ms && ms + hoechstalter_ms < jetzt);
        if (!weg) continue;
        NSString *p = [basis stringByAppendingPathComponent:funde[i][1]];
        if (baum_loeschen(p) != 0) zeile(@"Dateien: Aufraeumen - %@ liess sich nicht ganz loeschen", p);
    }
}

void qc_dateien_aufraeumen_beim_start(void) {
    if (!g_empfang_q) return;
    dispatch_async(g_empfang_q, ^{
        @autoreleasepool { qc_dateien_aufraeumen(0, 24ull * 3600 * 1000); }
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
@property (nonatomic, strong) dispatch_source_t wache;
@end
@implementation QCEmpfang
@end

// Die laufende Uebertragung. Nur auf g_empfang_q.
static QCEmpfang *g_empfang = nil;
static _Atomic uint64_t g_empfang_wartend = 0;
static _Atomic int g_empfang_ueberlauf = 0;

static void quittung_an(uint64_t sitzung, uint32_t kennung, uint8_t zustand, uint64_t empfangen) {
    NSData *q = qc_datei_quittung_kodieren(kennung, zustand, empfangen);
    if (g_wege.senden) g_wege.senden(sitzung, QC_DATEI_QUITTUNG, q.bytes, q.length);
}

// Die laufende Uebertragung beenden; loeschen = ihr Verzeichnis mit.
static void empfang_beenden(BOOL loeschen) {
    QCEmpfang *E = g_empfang;
    if (!E) return;
    g_empfang = nil;
    if (E->fd >= 0) { close(E->fd); E->fd = -1; }
    if (E.wache) { dispatch_source_cancel(E.wache); E.wache = nil; }
    if (loeschen && baum_loeschen(E.ordner) != 0)
        zeile(@"Dateien: %@ liess sich nicht ganz loeschen", E.ordner);
}

static void naechste_datei(QCEmpfang *E) {
    while (E->index < E.eintraege.count &&
           (E.eintraege[E->index].art != 0 || E.eintraege[E->index].groesse == 0))
        E->index++;
}

// Den Elternordner eines Eintrags im Uebertragungsverzeichnis oeffnen,
// Bestandteil fuer Bestandteil und ohne je einer Verknuepfung zu folgen.
static int eltern_oeffnen(NSString *ordner, NSArray<NSString *> *teile) {
    int fd = open(ordner.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    for (NSUInteger i = 0; fd >= 0 && i + 1 < teile.count; i++) {
        int n = openat(fd, teile[i].fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        int err = errno;
        close(fd);
        errno = err;
        fd = n;
    }
    return fd;
}

static void protokollfehler(QCEmpfang *E, NSString *warum) {
    zeile(@"Dateien: abgebrochen (ungueltig: %@)", warum);
    quittung_an(E->sitzung, E->kennung, QC_QUITT_UNGUELTIG, E->empfangen);
    empfang_beenden(YES);
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
    // Dateien, jede exklusiv angelegt.
    NSString *ordner = [basis stringByAppendingPathComponent:
                        [NSString stringWithFormat:@"%llu-%u", unix_ms(), kennung]];
    if (mkdir(ordner.fileSystemRepresentation, 0700) != 0) {
        zeile(@"Dateien: abgelehnt (Verzeichnis %@ nicht anlegbar: %s)", ordner, strerror(errno));
        quittung_an(sitzung, kennung, QC_QUITT_SCHREIBFEHLER, 0);
        return;
    }
    for (QCDateiEintrag *e in eintraege) {
        int ok = -1, err = 0;
        int pfd = eltern_oeffnen(ordner, e.teile);
        if (pfd >= 0) {
            const char *name = e.teile.lastObject.fileSystemRepresentation;
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
            baum_loeschen(ordner);
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
    quittung_an(sitzung, kennung, QC_QUITT_LAEUFT, 0);
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
        E->fd = pfd >= 0 ? openat(pfd, e.teile.lastObject.fileSystemRepresentation, O_WRONLY | O_NOFOLLOW | O_CLOEXEC) : -1;
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
    // Empfaenger auf 64 KiB, hingen beide bis zum Stillstand.
    if (E->empfangen - E->quittiert > QC_DATEI_FENSTER_SPIEL - QC_DATEI_STUECK_MAX) {
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
    // Vollstaendig; jede Datei wurde mit ihrem letzten Byte geschlossen. Die
    // obersten Eintraege gehen als Dateiliste in die Ablage dieses Macs.
    NSMutableArray<NSString *> *oben = [NSMutableArray array];
    for (QCDateiEintrag *e in E.eintraege)
        if (e.teile.count == 1) [oben addObject:[E.ordner stringByAppendingPathComponent:e.teile[0]]];
    void (*fertig)(NSArray<NSString *> *) = g_fertig;
    if (fertig) fertig(oben);
    quittung_an(E->sitzung, E->kennung, QC_QUITT_FERTIG, E->empfangen);
    NSUInteger anzahl = E.eintraege.count;
    uint64_t bytes = E->empfangen;
    empfang_beenden(NO);                  // das Verzeichnis bleibt: daraus wird eingefuegt
    qc_dateien_aufraeumen(3, 0);
    zeile(@"Dateien: empfangen %lu Eintraege, %@ - in die Ablage gelegt", (unsigned long)anzahl, mb_text(bytes));
}

void qc_empfang_nachricht(uint64_t sitzung, uint64_t kanal, uint8_t typ, NSData *nutzlast) {
    if (!g_empfang_q || !nutzlast) return;
    uint64_t n = nutzlast.length;
    if (atomic_fetch_add(&g_empfang_wartend, n) + n > QC_EMPFANG_WARTEND_MAX) {
        // Die Gegenseite schickt weit mehr, als ihr Fenster erlaubt: nicht
        // weiter einreihen (der Speicher waere sonst ihr ausgeliefert),
        // sondern die Uebertragung als ungueltig beenden.
        atomic_fetch_sub(&g_empfang_wartend, n);
        if (!atomic_exchange(&g_empfang_ueberlauf, 1)) {
            dispatch_async(g_empfang_q, ^{
                @autoreleasepool {
                    atomic_store(&g_empfang_ueberlauf, 0);
                    if (g_empfang && g_empfang->sitzung == sitzung)
                        protokollfehler(g_empfang, @"Gegenseite haelt das Fenster nicht ein");
                }
            });
        }
        return;
    }
    dispatch_async(g_empfang_q, ^{
        @autoreleasepool {
            switch (typ) {
                case QC_DATEI_ANGEBOT: empfang_angebot(sitzung, kanal, nutzlast); break;
                case QC_DATEI_STUECK:  empfang_stueck(sitzung, nutzlast); break;
                case QC_DATEI_ENDE:    empfang_ende(sitzung, nutzlast); break;
                default: break;
            }
        }
        atomic_fetch_sub(&g_empfang_wartend, n);
    });
}

void qc_empfang_kanal_weg(uint64_t sitzung, uint64_t kanal) {
    if (!g_empfang_q) return;
    dispatch_async(g_empfang_q, ^{
        @autoreleasepool {
            if (g_empfang && g_empfang->sitzung == sitzung && g_empfang->kanal == kanal) {
                zeile(@"Dateien: abgebrochen (Eingabekanal getrennt)");
                empfang_beenden(YES);
            }
        }
    });
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
static uint64_t g_s_quittung_ms = 0;        // letzte Quittung, monoton

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
        if (empfangen > g_s_quittiert) g_s_quittiert = empfangen;
        if (zustand != QC_QUITT_LAEUFT && !g_s_zustand) g_s_zustand = zustand;
        g_s_quittung_ms = jetzt_ms();
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
        // Ton), wartet die Datei - so steht sie nie vor einem Bild an.
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

// Ein Eintrag samt Inhalt (Ordner rekursiv, nach Namen sortiert).
// 1 = weiter, 0 = ueber EINTRAEGE_MAX oder GESAMT_MAX, -1 = ueberholt.
static int auflisten(uint64_t gen, NSString *quelle, NSString *rel, NSString *name, NSUInteger tiefe,
                     NSMutableArray<QCDateiEintrag *> *liste, uint64_t *gesamt) {
    if (ueberholt(gen)) return -1;
    NSData *pfad = [rel dataUsingEncoding:NSUTF8StringEncoding];
    if (!pfad || pfad.length > QC_DATEI_PFAD_MAX || tiefe > QC_DATEI_TIEFE_MAX ||
        [name lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > QC_DATEI_BESTANDTEIL_MAX ||
        [name rangeOfString:@"\\"].location != NSNotFound) {
        // Die Gegenseite lehnte sonst das ganze Angebot ab.
        zeile(@"Dateien: uebersprungen (Name oder Pfad fuer die Gegenseite ungueltig): %@", quelle);
        return 1;
    }
    struct stat st;
    if (lstat(quelle.fileSystemRepresentation, &st) != 0) {
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
    [liste addObject:e];
    *gesamt += e.groesse;
    if (liste.count > QC_DATEI_EINTRAEGE_MAX || *gesamt > QC_DATEI_GESAMT_MAX) return 0;
    if (!e.art) return 1;
    NSError *fehler = nil;
    NSArray<NSString *> *kinder = [[NSFileManager defaultManager] contentsOfDirectoryAtPath:quelle error:&fehler];
    if (!kinder) {
        zeile(@"Dateien: Ordner nicht lesbar (%@), er geht leer hinaus: %@", fehler.localizedDescription, quelle);
        return 1;
    }
    kinder = [kinder sortedArrayUsingSelector:@selector(compare:)];
    NSMutableSet<NSString *> *gesehen = [NSMutableSet set];
    for (NSString *k in kinder) {
        NSString *kname = [k precomposedStringWithCanonicalMapping];
        // Auf einem Volume mit Gross- und Kleinschreibung koennen "A" und "a"
        // nebeneinander liegen, ebenso "a\x01" und "a_"; die Gegenseite
        // lehnte dann alles ab.
        NSString *ks = sende_schluessel(kname);
        if (![gesehen containsObject:ks]) {
            [gesehen addObject:ks];
        } else {
            zeile(@"Dateien: uebersprungen (Name nach Bereinigung oder bis auf Gross- und Kleinschreibung doppelt): %@",
                  [quelle stringByAppendingPathComponent:k]);
            continue;
        }
        int r = auflisten(gen, [quelle stringByAppendingPathComponent:k], [rel stringByAppendingFormat:@"/%@", kname],
                          kname, tiefe + 1, liste, gesamt);
        if (r <= 0) return r;
    }
    return 1;
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
        int fd = open(e.quelle.fileSystemRepresentation, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
        struct stat st;
        if (fd < 0 || fstat(fd, &st) != 0 || !S_ISREG(st.st_mode) ||
            fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK) != 0) {
            warum = [NSString stringWithFormat:@"Lesefehler bei %@: %s", e.quelle, fd < 0 ? strerror(errno) : "keine Datei mehr"];
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
        // Dateien aus mehreren Ordnern koennen gleich heissen: nur die erste.
        NSString *ks = sende_schluessel(name);
        if ([oben containsObject:ks]) {
            zeile(@"Dateien: doppelter Name %@ - nur der erste wird gesendet", name);
            continue;
        }
        [oben addObject:ks];
        r = auflisten(gen, p, name, name, 1, liste, &gesamt);
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
    if (!g_empfang_q) g_empfang_q = dispatch_queue_create("tech.quadchroma.dateien-empfang", DISPATCH_QUEUE_SERIAL);
    if (!g_senden_q) g_senden_q = dispatch_queue_create("tech.quadchroma.dateien-senden", DISPATCH_QUEUE_SERIAL);
}

void qc_dateien_sitzung_vorbei(uint64_t neue_sitzung) {
    // Den Sender wecken: er merkt am Rueckstand (-1), dass seine Sitzung weg
    // ist, und muss dafuer nicht erst eine Quittung abwarten.
    pthread_cond_broadcast(&g_s_cv);
    if (!g_empfang_q) return;
    dispatch_async(g_empfang_q, ^{
        @autoreleasepool {
            if (g_empfang && g_empfang->sitzung != neue_sitzung) {
                zeile(@"Dateien: abgebrochen (Zuschauer gewechselt oder weg)");
                empfang_beenden(YES);
            }
        }
    });
}

void qc_dateien_abwarten(void) {
    if (g_empfang_q) dispatch_sync(g_empfang_q, ^{});
    if (g_senden_q) dispatch_sync(g_senden_q, ^{});
    if (g_empfang_q) dispatch_sync(g_empfang_q, ^{});
}
