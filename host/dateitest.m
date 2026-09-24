// Pruefprogramm fuer host/dateien.m: Pruefvektoren (Spezifikation 2.4),
// Kodieren und Lesen, Pfadregeln samt Bereinigung fuer macOS, Empfaenger und
// Sender (auch gegeneinander im Speicher), Fenster, Drossel ueber den
// Sendepuffer, Stillstand, Abbruch, Aufraeumen des Ablageverzeichnisses,
// Loeschen tiefer Uebertragungen (absolut ueber PATH_MAX).
//
//   clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
//         -framework Foundation host/dateitest.m host/dateien.m -o /tmp/dateitest
//   /tmp/dateitest
//
// Ohne Netz, ohne Zwischenablage, ohne den Rest des Hosts: die Wege
// (qc_dateien_wege) sind hier Attrappen, die aufzeichnen oder Sender und
// Empfaenger direkt verbinden. Alle Dateien liegen in einem frischen Ordner
// unter $TMPDIR, auch die Ablagebasis. Rueckgabe: Zahl der Fehler.

#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <limits.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include "dateien.h"

static int g_fehler = 0;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

static NSString *g_wurzel;                   // frischer Ordner unter $TMPDIR
#define SITZUNG 5ull

// ---------------------------------------------------------------- Attrappen

static NSMutableArray<NSString *> *g_zeilen;        // Protokoll
static NSMutableArray<NSArray *> *g_gesendet;       // @[@(typ), NSData]
static NSMutableArray<NSArray<NSString *> *> *g_fertig_listen;
static _Atomic int g_sitzung_gilt = 1;              // 0: senden liefert 0, Rueckstand -1
static _Atomic int g_rueckstand = 0;                // gemeldeter Sendepuffer
static _Atomic int g_spiel = 0;
static _Atomic int g_schleife = 0;                  // 1: Sender und Empfaenger verbunden
static _Atomic int g_verzoegern_ms = 0;             // Quittungen so spaet beim Sender
static _Atomic int g_halten = 0;                    // 1: Quittungen erreichen den Sender nicht
// Fenstermessung: was der Sender an Datenbytes hinausgab und was ihm davon
// quittiert zugestellt wurde. Unter @synchronized (g_gesendet).
static uint64_t g_stueck_bytes = 0, g_zugestellt = 0, g_max_unterwegs = 0;

static void protokoll(const char *z) {
    @synchronized (g_zeilen) { [g_zeilen addObject:@(z)]; }
    printf("         | %s\n", z);
}

static int zeilen_mit(NSString *was) {
    int n = 0;
    @synchronized (g_zeilen) {
        for (NSString *z in g_zeilen) if ([z rangeOfString:was].location != NSNotFound) n++;
    }
    return n;
}

static uint64_t rd64(const uint8_t *p) {
    uint64_t v = 0;
    for (int i = 7; i >= 0; i--) v = (v << 8) | p[i];
    return v;
}

static void zustellen(uint64_t sitzung, NSData *q) {
    @synchronized (g_gesendet) {
        uint64_t e = rd64((const uint8_t *)q.bytes + 8);
        if (e > g_zugestellt) g_zugestellt = e;
    }
    qc_senden_quittung(sitzung, q.bytes, q.length);
}

static int weg_senden(uint64_t sitzung, uint8_t typ, const void *d, size_t n) {
    if (!atomic_load(&g_sitzung_gilt)) return 0;
    NSData *data = [NSData dataWithBytes:d length:n];
    @synchronized (g_gesendet) {
        [g_gesendet addObject:@[ @(typ), data ]];
        if (typ == QC_DATEI_STUECK) {
            g_stueck_bytes += n - QC_DATEI_STUECK_KOPF;
            uint64_t unterwegs = g_stueck_bytes - g_zugestellt;
            if (unterwegs > g_max_unterwegs) g_max_unterwegs = unterwegs;
        }
    }
    if (!atomic_load(&g_schleife)) return 1;
    if (typ == QC_DATEI_ANGEBOT || typ == QC_DATEI_STUECK || typ == QC_DATEI_ENDE) {
        qc_empfang_nachricht(sitzung, 1, typ, data);
    } else if (typ == QC_DATEI_QUITTUNG) {
        if (atomic_load(&g_halten)) return 1;
        int v = atomic_load(&g_verzoegern_ms);
        if (v) dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)v * NSEC_PER_MSEC),
                              dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{ zustellen(sitzung, data); });
        else zustellen(sitzung, data);
    }
    return 1;
}

static int weg_rueckstand(uint64_t sitzung) {
    return atomic_load(&g_sitzung_gilt) ? atomic_load(&g_rueckstand) : -1;
}

static int weg_spiel(void) { return atomic_load(&g_spiel); }

static void fertig(NSArray<NSString *> *pfade) {
    @synchronized (g_fertig_listen) { [g_fertig_listen addObject:pfade]; }
}

static NSUInteger fertig_anzahl(void) {
    @synchronized (g_fertig_listen) { return g_fertig_listen.count; }
}

// Frische Aufzeichnung.
static void neu_aufzeichnen(void) {
    @synchronized (g_gesendet) {
        [g_gesendet removeAllObjects];
        g_stueck_bytes = g_zugestellt = g_max_unterwegs = 0;
    }
}

// Aufgezeichnete Nachrichten eines Typs.
static NSArray<NSData *> *nachrichten(uint8_t typ) {
    NSMutableArray<NSData *> *a = [NSMutableArray array];
    @synchronized (g_gesendet) {
        for (NSArray *m in g_gesendet) if ([m[0] intValue] == typ) [a addObject:m[1]];
    }
    return a;
}

static uint64_t stueck_summe(void) {
    uint64_t n = 0;
    for (NSData *d in nachrichten(QC_DATEI_STUECK)) n += d.length - QC_DATEI_STUECK_KOPF;
    return n;
}

// Quittungen als @[@(zustand), @(empfangen)].
static NSArray<NSArray<NSNumber *> *> *quittungen(void) {
    NSMutableArray *a = [NSMutableArray array];
    for (NSData *q in nachrichten(QC_DATEI_QUITTUNG)) {
        uint32_t k; uint8_t z; uint64_t e;
        if (qc_datei_quittung_lesen(q.bytes, q.length, &k, &z, &e) == 0) [a addObject:@[ @(z), @(e) ]];
    }
    return a;
}

static int letzter_zustand(void) {
    NSArray *q = quittungen();
    return q.count ? [q.lastObject[0] intValue] : -1;
}

// ------------------------------------------------------------------ Helfer

static NSData *hex(const char *s) {
    NSMutableData *d = [NSMutableData data];
    unsigned v;
    while (*s) {
        if (*s == ' ' || *s == '\n') { s++; continue; }
        sscanf(s, "%2x", &v);
        uint8_t b = (uint8_t)v;
        [d appendBytes:&b length:1];
        s += 2;
    }
    return d;
}

static QCDateiEintrag *E(uint8_t art, uint64_t groesse, NSString *pfad) {
    return [QCDateiEintrag art:art groesse:groesse pfad:pfad];
}

static QCDateiEintrag *Eroh(uint8_t art, uint64_t groesse, const char *p, size_t n) {
    QCDateiEintrag *e = [[QCDateiEintrag alloc] init];
    e.art = art;
    e.groesse = groesse;
    e.pfad = [NSData dataWithBytes:p length:n];
    return e;
}

// Ein Eintrag samt allen Elternordnern davor.
static NSArray<QCDateiEintrag *> *mit_eltern(uint8_t art, uint64_t groesse, NSString *pfad) {
    NSMutableArray *a = [NSMutableArray array];
    NSArray<NSString *> *t = [pfad componentsSeparatedByString:@"/"];
    for (NSUInteger i = 1; i < t.count; i++)
        [a addObject:E(1, 0, [[t subarrayWithRange:NSMakeRange(0, i)] componentsJoinedByString:@"/"])];
    [a addObject:E(art, groesse, pfad)];
    return a;
}

static int angebot_rc(NSData *d, NSString **grund, NSArray<QCDateiEintrag *> **aus) {
    uint32_t k; uint64_t g;
    NSString *w = nil;
    NSArray *e = nil;
    int rc = qc_datei_angebot_lesen(d.bytes, d.length, &k, &g, &e, &w);
    if (grund) *grund = w;
    if (aus) *aus = e;
    return rc;
}

static int liste_rc(NSArray<QCDateiEintrag *> *e, NSString **grund) {
    return angebot_rc(qc_datei_angebot_kodieren(7, e), grund, NULL);
}

static NSString *wiederholt(char c, NSUInteger n) {
    return [@"" stringByPaddingToLength:n withString:[NSString stringWithFormat:@"%c", c] startingAtIndex:0];
}

static NSData *zufall(size_t n) {
    NSMutableData *d = [NSMutableData dataWithLength:n];
    arc4random_buf(d.mutableBytes, n);
    return d;
}

static BOOL existiert(NSString *p) {
    struct stat st;
    return lstat(p.fileSystemRepresentation, &st) == 0;
}

// Zwei Baeume vergleichen: gleiche Namen, gleiche Arten, gleiche Bytes.
static BOOL gleich(NSString *a, NSString *b) {
    struct stat sa, sb;
    if (lstat(a.fileSystemRepresentation, &sa) || lstat(b.fileSystemRepresentation, &sb)) return NO;
    if (S_ISDIR(sa.st_mode) != S_ISDIR(sb.st_mode)) return NO;
    if (!S_ISDIR(sa.st_mode)) return [[NSData dataWithContentsOfFile:a] isEqualToData:[NSData dataWithContentsOfFile:b]];
    NSArray *ka = [[[NSFileManager defaultManager] contentsOfDirectoryAtPath:a error:nil] sortedArrayUsingSelector:@selector(compare:)];
    NSArray *kb = [[[NSFileManager defaultManager] contentsOfDirectoryAtPath:b error:nil] sortedArrayUsingSelector:@selector(compare:)];
    if (![ka isEqualToArray:kb]) return NO;
    for (NSString *k in ka)
        if (!gleich([a stringByAppendingPathComponent:k], [b stringByAppendingPathComponent:k])) return NO;
    return YES;
}

static NSString *basis(void) { return qc_dateien_basis(); }

// Uebertragungsverzeichnisse in der Basis, deren Name auf -<kennung> endet.
static int verzeichnisse_mit(uint32_t kennung) {
    int n = 0;
    NSString *ende = [NSString stringWithFormat:@"-%u", kennung];
    for (NSString *k in [[NSFileManager defaultManager] contentsOfDirectoryAtPath:basis() error:nil])
        if ([k hasSuffix:ende]) n++;
    return n;
}

static void empfang(uint8_t typ, NSData *d) { qc_empfang_nachricht(SITZUNG, 1, typ, d); }

static void stueck(uint32_t kennung, uint32_t eintrag, uint64_t versatz, NSData *daten) {
    empfang(QC_DATEI_STUECK, qc_datei_stueck_kodieren(kennung, eintrag, versatz, daten.bytes, daten.length));
}

static void ende(uint32_t kennung, uint8_t grund) { empfang(QC_DATEI_ENDE, qc_datei_ende_kodieren(kennung, grund)); }

static void schlafen(double s) { usleep((useconds_t)(s * 1e6)); }

// ------------------------------------------------------------- Pruefvektoren

static void vektoren_pruefen(void) {
    printf("\n-- Pruefvektoren (2.4)\n");
    uint32_t bits = 0;
    pruefe([qc_datei_faehigkeiten_kodieren(1) isEqualToData:hex("01 00 00 00")], "Faehigkeiten 1 kodiert");
    NSData *f6 = hex("01 00 00 00 ff ee");
    pruefe(qc_datei_faehigkeiten_lesen(f6.bytes, f6.length, &bits) == 0 && bits == 1,
           "Faehigkeiten gelesen, weitere Bytes uebergangen");
    pruefe(qc_datei_faehigkeiten_lesen(f6.bytes, 3, &bits) != 0, "Faehigkeiten mit 3 Byte: zu kurz");

    NSData *soll = hex("07 00 00 00  03 00 00 00  05 00 00 00 00 00 00 00"
                       "01 00 06 00  00 00 00 00 00 00 00 00  42 69 6c 64 65 72"
                       "00 00 0c 00  05 00 00 00 00 00 00 00  42 69 6c 64 65 72 2f 61 2e 74 78 74"
                       "00 00 05 00  00 00 00 00 00 00 00 00  62 2e 62 69 6e");
    NSData *ist = qc_datei_angebot_kodieren(7, @[ E(1, 0, @"Bilder"), E(0, 5, @"Bilder/a.txt"), E(0, 0, @"b.bin") ]);
    pruefe([ist isEqualToData:soll], "Angebot kodiert wie der Vektor");
    uint32_t k = 0; uint64_t g = 0;
    NSArray<QCDateiEintrag *> *e = nil;
    int rc = qc_datei_angebot_lesen(soll.bytes, soll.length, &k, &g, &e, NULL);
    pruefe(rc == 0 && k == 7 && g == 5 && e.count == 3 &&
           e[0].art == 1 && e[0].groesse == 0 && [e[0].teile isEqualToArray:@[ @"Bilder" ]] &&
           e[1].art == 0 && e[1].groesse == 5 && [e[1].teile isEqualToArray:(@[ @"Bilder", @"a.txt" ])] &&
           e[2].art == 0 && e[2].groesse == 0 && [e[2].teile isEqualToArray:@[ @"b.bin" ]],
           "Angebot gelesen: Kennung, Gesamt, Arten, Groessen, Pfade");

    NSData *st = hex("07 00 00 00  01 00 00 00  00 00 00 00 00 00 00 00  68 61 6c 6c 6f");
    pruefe([qc_datei_stueck_kodieren(7, 1, 0, "hallo", 5) isEqualToData:st], "Stueck kodiert wie der Vektor");
    uint32_t kk, ei; uint64_t vs; const uint8_t *dd; size_t dn;
    pruefe(qc_datei_stueck_lesen(st.bytes, st.length, &kk, &ei, &vs, &dd, &dn) == 0 && kk == 7 && ei == 1 && vs == 0 &&
           dn == 5 && memcmp(dd, "hallo", 5) == 0, "Stueck gelesen");
    pruefe(qc_datei_stueck_lesen(st.bytes, 16, &kk, &ei, &vs, &dd, &dn) != 0, "Stueck ohne Daten: zu kurz");

    NSData *en = hex("07 00 00 00 00");
    uint8_t gr = 9;
    pruefe([qc_datei_ende_kodieren(7, 0) isEqualToData:en], "Ende kodiert wie der Vektor");
    pruefe(qc_datei_ende_lesen(en.bytes, en.length, &kk, &gr) == 0 && kk == 7 && gr == 0 &&
           qc_datei_ende_lesen(en.bytes, 4, &kk, &gr) != 0, "Ende gelesen, mit 4 Byte zu kurz");

    NSData *qu = hex("07 00 00 00  01 00 00 00  05 00 00 00 00 00 00 00");
    uint8_t z; uint64_t em;
    pruefe([qc_datei_quittung_kodieren(7, 1, 5) isEqualToData:qu], "Quittung kodiert wie der Vektor");
    pruefe(qc_datei_quittung_lesen(qu.bytes, qu.length, &kk, &z, &em) == 0 && kk == 7 && z == 1 && em == 5 &&
           qc_datei_quittung_lesen(qu.bytes, 15, &kk, &z, &em) != 0, "Quittung gelesen, mit 15 Byte ungueltig");

    // Hin und zurueck mit Umlauten, Leerzeichen und tiefen Pfaden.
    NSArray *liste = @[ E(1, 0, @"Ürlaub 2026"), E(1, 0, @"Ürlaub 2026/Tag 1"), E(0, 123456789, @"Ürlaub 2026/Tag 1/Bild ä.jpg"),
                        E(0, 0, @"leer"), E(1, 0, @"x") ];
    NSData *hin = qc_datei_angebot_kodieren(0xfeedbeef, liste);
    rc = qc_datei_angebot_lesen(hin.bytes, hin.length, &k, &g, &e, NULL);
    BOOL zurueck = rc == 0 && k == 0xfeedbeef && g == 123456789 && e.count == liste.count;
    for (NSUInteger i = 0; zurueck && i < liste.count; i++)
        zurueck = e[i].art == [liste[i] art] && e[i].groesse == [liste[i] groesse] && [e[i].pfad isEqualToData:[liste[i] pfad]];
    pruefe(zurueck && [qc_datei_angebot_kodieren(0xfeedbeef, e) isEqualToData:hin], "Angebot hin und zurueck");
    pruefe(qc_datei_grenze(50) == 1048576 && qc_datei_grenze(51) == 16 + 49152 && qc_datei_grenze(52) == 256 &&
           qc_datei_grenze(53) == 256 && qc_datei_grenze(69) == 256 && qc_datei_grenze(48) == 0 && qc_datei_grenze(19) == 0,
           "Obergrenzen je Typ auf dem Eingabekanal (2.6)");
}

// --------------------------------------------------------------- Pfadregeln

static void pfade_pruefen(void) {
    printf("\n-- Pfadregeln (2.5): feindliche Pfade lehnen das ganze Angebot ab\n");
    struct { const char *pfad; size_t n; const char *grund; const char *name; } boese[] = {
        { "..", 2, "'..'", "'..'" },
        { "a/../b", 6, "'..'", "'a/../b'" },
        { ".", 1, "'.'", "'.'" },
        { "/abs", 4, "absolut", "'/abs'" },
        { "a//b", 4, "leerer Bestandteil", "'a//b'" },
        { "a/", 2, "leerer Bestandteil", "'a/' (leerer letzter Bestandteil)" },
        { "a\\b", 3, "'\\'", "Rueckstrich" },
        { "a\0b", 3, "NUL", "NUL im Pfad" },
        { "\xff", 1, "UTF-8", "ungueltiges UTF-8" },
        { "\xc0\xaf", 2, "UTF-8", "ueberlanges UTF-8" },
        { "\xed\xa0\x80", 3, "UTF-8", "Ersatzzeichen (Surrogat) in UTF-8" },
    };
    for (size_t i = 0; i < sizeof boese / sizeof boese[0]; i++) {
        NSString *grund = nil;
        int rc = liste_rc(@[ Eroh(0, 0, boese[i].pfad, boese[i].n) ], &grund);
        char was[160];
        snprintf(was, sizeof was, "%s: Quittung 4 (%s)", boese[i].name, grund.UTF8String ?: "-");
        pruefe(rc == QC_QUITT_UNGUELTIG && [grund rangeOfString:@(boese[i].grund)].location != NSNotFound, was);
    }
    NSString *grund = nil;
    // Laengen: Pfad hoechstens 1024 Byte, Bestandteil hoechstens 255, 32 Stufen.
    NSString *b204 = wiederholt('p', 204), *b205 = wiederholt('p', 205);
    NSString *p1024 = [@[ b204, b204, b204, b204, b204 ] componentsJoinedByString:@"/"];
    NSString *p1025 = [@[ b205, b204, b204, b204, b204 ] componentsJoinedByString:@"/"];
    pruefe(p1024.length == 1024 && liste_rc(mit_eltern(0, 0, p1024), &grund) == 0, "Pfad mit 1024 Byte: angenommen");
    pruefe(liste_rc(mit_eltern(0, 0, p1025), &grund) == QC_QUITT_UNGUELTIG && [grund rangeOfString:@"Pfadlaenge"].location != NSNotFound,
           "Pfad mit 1025 Byte: abgelehnt");
    pruefe(liste_rc(@[ E(0, 0, wiederholt('b', 255)) ], &grund) == 0, "Bestandteil mit 255 Byte: angenommen");
    pruefe(liste_rc(@[ E(0, 0, wiederholt('b', 256)) ], &grund) == QC_QUITT_UNGUELTIG && [grund rangeOfString:@"255"].location != NSNotFound,
           "Bestandteil mit 256 Byte: abgelehnt");
    NSMutableArray *stufen = [NSMutableArray array];
    for (int i = 0; i < 33; i++) [stufen addObject:@"d"];
    NSString *tief33 = [stufen componentsJoinedByString:@"/"];
    NSString *tief32 = [[stufen subarrayWithRange:NSMakeRange(0, 32)] componentsJoinedByString:@"/"];
    pruefe(liste_rc(mit_eltern(0, 0, tief32), &grund) == 0, "32 Stufen: angenommen");
    pruefe(liste_rc(mit_eltern(0, 0, tief33), &grund) == QC_QUITT_UNGUELTIG && [grund rangeOfString:@"32 Stufen"].location != NSNotFound,
           "33 Stufen: abgelehnt");

    // Reihenfolge.
    pruefe(liste_rc(@[ E(0, 0, @"a/b.txt"), E(1, 0, @"a") ], &grund) == QC_QUITT_UNGUELTIG &&
           [grund rangeOfString:@"Elternordner"].location != NSNotFound, "Nachfahr vor dem Vorfahr: abgelehnt");
    pruefe(liste_rc(@[ E(0, 0, @"a"), E(0, 0, @"a/b.txt") ], &grund) == QC_QUITT_UNGUELTIG, "Elternteil ist eine Datei: abgelehnt");
    pruefe(liste_rc(@[ E(0, 0, @"x/y.txt") ], &grund) == QC_QUITT_UNGUELTIG, "Elternordner fehlt ganz: abgelehnt");

    // Doppelte, erst nach der Bereinigung und ohne Gross- und Kleinschreibung.
    pruefe(liste_rc(@[ E(0, 0, @"A.txt"), E(0, 0, @"a.txt") ], &grund) == QC_QUITT_UNGUELTIG &&
           [grund rangeOfString:@"doppelt"].location != NSNotFound, "A.txt und a.txt: doppelt");
    pruefe(liste_rc(@[ E(0, 0, @"\u00e9.txt"), E(0, 0, @"e\u0301.txt") ], &grund) == QC_QUITT_UNGUELTIG,
           "e mit Akzent als ein Zeichen und zusammengesetzt: doppelt (APFS)");
    pruefe(liste_rc(@[ Eroh(0, 0, "x\x01", 2), E(0, 0, @"x_") ], &grund) == QC_QUITT_UNGUELTIG,
           "doppelt erst nach der Bereinigung (x\\x01 und x_)");
    pruefe(liste_rc(@[ E(1, 0, @"Ordner"), E(0, 0, @"ordner/a"), ], &grund) == QC_QUITT_UNGUELTIG,
           "Elternordner mit anderer Schreibweise: abgelehnt (Reihenfolge verlangt ihn wortgleich)");

    // Groessen, Anzahl, Arten, Format.
    NSMutableData *summe = [qc_datei_angebot_kodieren(7, @[ E(0, 5, @"a") ]) mutableCopy];
    ((uint8_t *)summe.mutableBytes)[8] = 6;
    pruefe(angebot_rc(summe, &grund, NULL) == QC_QUITT_UNGUELTIG && [grund rangeOfString:@"Summe"].location != NSNotFound,
           "falsche Summe: abgelehnt");
    pruefe(angebot_rc(hex("07 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"), &grund, NULL) == QC_QUITT_UNGUELTIG,
           "Anzahl 0: abgelehnt (4)");
    pruefe(angebot_rc(hex("07 00 00 00 11 27 00 00 00 00 00 00 00 00 00 00"), &grund, NULL) == QC_QUITT_ZU_GROSS,
           "10001 Eintraege: zu viele (2)");
    pruefe(liste_rc(@[ E(0, QC_DATEI_GESAMT_MAX + 1, @"riesig") ], &grund) == QC_QUITT_ZU_GROSS, "ueber 4 GiB: zu gross (2)");
    pruefe(liste_rc(@[ E(0, QC_DATEI_GESAMT_MAX, @"genau4") ], &grund) == 0, "genau 4 GiB: angenommen");
    pruefe(liste_rc(@[ E(2, 0, @"a") ], &grund) == QC_QUITT_UNGUELTIG, "Art 2: abgelehnt");
    pruefe(liste_rc(@[ E(1, 3, @"a") ], &grund) == QC_QUITT_UNGUELTIG, "Ordner mit Groesse: abgelehnt");
    pruefe(liste_rc(@[ Eroh(0, 0, "", 0) ], &grund) == QC_QUITT_UNGUELTIG, "Pfadlaenge 0: abgelehnt");
    NSData *gut = qc_datei_angebot_kodieren(7, @[ E(0, 0, @"a.txt") ]);
    pruefe(angebot_rc([gut subdataWithRange:NSMakeRange(0, gut.length - 1)], &grund, NULL) == QC_QUITT_UNGUELTIG,
           "abgeschnitten: abgelehnt");
    NSMutableData *lang = [gut mutableCopy];
    [lang appendBytes:"x" length:1];
    pruefe(angebot_rc(lang, &grund, NULL) == QC_QUITT_UNGUELTIG, "Bytes nach dem letzten Eintrag: abgelehnt");
    pruefe(angebot_rc(qc_datei_angebot_kodieren(0, @[ E(0, 0, @"a") ]), &grund, NULL) == QC_QUITT_UNGUELTIG, "Kennung 0: abgelehnt");

    printf("\n-- Bereinigung fuer macOS\n");
    NSArray<QCDateiEintrag *> *aus = nil;
    int rc = angebot_rc(qc_datei_angebot_kodieren(7, @[ Eroh(0, 0, "a\x01" "b\x1f" ".txt", 8), E(0, 0, @"C:"), E(0, 0, @"CON"),
                                                       E(0, 0, @"aux.txt"), E(0, 0, @"a:b"), E(0, 0, @"x. ") ]), &grund, &aus);
    NSMutableArray<NSString *> *namen = [NSMutableArray array];
    for (QCDateiEintrag *x in aus) [namen addObject:[NSString stringWithFormat:@"'%@'", [x.teile componentsJoinedByString:@"/"]]];
    printf("         (bereinigt: %s)\n", [namen componentsJoinedByString:@" "].UTF8String);
    pruefe(rc == 0 && [aus[0].teile isEqualToArray:@[ @"a_b_.txt" ]], "Steuerzeichen werden zu '_'");
    pruefe(rc == 0 && [aus[1].teile isEqualToArray:@[ @"C:" ]] && [aus[4].teile isEqualToArray:@[ @"a:b" ]],
           "':' bleibt ('C:' ist unter macOS ein gewoehnlicher Name)");
    pruefe(rc == 0 && [aus[2].teile isEqualToArray:@[ @"CON" ]] && [aus[3].teile isEqualToArray:@[ @"aux.txt" ]] &&
           [aus[5].teile isEqualToArray:@[ @"x. " ]],
           "Geraetenamen und Punkt/Leerzeichen am Ende bleiben (nur Windows bereinigt sie)");
    pruefe([qc_datei_bereinigen(@"\t\n\x1f ") isEqualToString:@"___ "], "qc_datei_bereinigen: U+0000-U+001F, nicht U+0020");
}

// ---------------------------------------------------------------- Empfaenger

// Ein gueltiges Angebot mit Ordnern, einer leeren Datei und einer Datei ueber
// mehrere Stuecke. Die Inhalte je Eintrag in *inhalte (Ordner: NSNull).
static NSData *beispiel(uint32_t kennung, NSArray **inhalte) {
    NSData *gross = zufall(150000);
    *inhalte = @[ [NSNull null], [@"hallo" dataUsingEncoding:NSUTF8StringEncoding], [NSData data], gross,
                  [NSNull null], [@"abc" dataUsingEncoding:NSUTF8StringEncoding] ];
    return qc_datei_angebot_kodieren(kennung, @[ E(1, 0, @"Bilder"), E(0, 5, @"Bilder/a.txt"), E(0, 0, @"b.bin"),
                                                 E(0, 150000, @"gross.bin"), E(1, 0, @"Bilder/sub"), E(0, 3, @"Bilder/sub/c.txt") ]);
}

// Alle Stuecke eines Beispiels in der richtigen Reihenfolge.
static void alle_stuecke(uint32_t kennung, NSArray *inhalte) {
    for (NSUInteger i = 0; i < inhalte.count; i++) {
        if (inhalte[i] == [NSNull null]) continue;
        NSData *d = inhalte[i];
        for (NSUInteger o = 0; o < d.length; o += QC_DATEI_STUECK_MAX) {
            NSUInteger n = MIN((NSUInteger)QC_DATEI_STUECK_MAX, d.length - o);
            stueck(kennung, (uint32_t)i, o, [d subdataWithRange:NSMakeRange(o, n)]);
        }
    }
}

static void empfaenger_pruefen(void) {
    printf("\n-- Empfaenger: vollstaendige Uebertragung\n");
    atomic_store(&g_schleife, 0);
    neu_aufzeichnen();
    NSArray *inhalte;
    NSUInteger f0 = fertig_anzahl();
    empfang(QC_DATEI_ANGEBOT, beispiel(11, &inhalte));
    qc_dateien_abwarten();
    NSString *ordner = qc_empfang_ordner();
    struct stat st;
    BOOL rechte = ordner && stat(basis().fileSystemRepresentation, &st) == 0 && (st.st_mode & 0777) == 0700 &&
                  stat(ordner.fileSystemRepresentation, &st) == 0 && (st.st_mode & 0777) == 0700;
    BOOL leer_da = ordner && existiert([ordner stringByAppendingPathComponent:@"Bilder/sub/c.txt"]) &&
                   existiert([ordner stringByAppendingPathComponent:@"b.bin"]);
    printf("         (Uebertragungsverzeichnis %s)\n", ordner.lastPathComponent.UTF8String ?: "-");
    pruefe(ordner && [ordner.lastPathComponent hasSuffix:@"-11"] && [ordner.stringByDeletingLastPathComponent isEqualToString:basis()],
           "frisches Verzeichnis <unix-ms>-<kennung> unter der Basis");
    pruefe(rechte, "Basis und Uebertragungsverzeichnis mit Rechten 0700");
    pruefe(leer_da, "nach dem Angebot stehen alle Ordner und leeren Dateien schon da");
    NSArray *q = quittungen();
    pruefe(q.count == 1 && [q[0] isEqualToArray:(@[ @0, @0 ])], "sofort Quittung 0 mit empfangen 0");
    alle_stuecke(11, inhalte);
    ende(11, QC_ENDE_VOLLSTAENDIG);
    qc_dateien_abwarten();
    q = quittungen();
    NSMutableArray<NSString *> *qt = [NSMutableArray array];
    for (NSArray *x in q) [qt addObject:[NSString stringWithFormat:@"%@/%@", x[0], x[1]]];
    printf("         (Quittungen Zustand/empfangen: %s)\n", [qt componentsJoinedByString:@" "].UTF8String);
    BOOL abstaende = q.count >= 3;
    for (NSUInteger i = 1; abstaende && i + 1 < q.count; i++)
        abstaende = [q[i][0] intValue] == 0 && [q[i][1] unsignedLongLongValue] - [q[i - 1][1] unsignedLongLongValue] < QC_DATEI_QUITTUNG_ALLE + QC_DATEI_STUECK_MAX;
    pruefe(abstaende && [q.lastObject isEqualToArray:(@[ @1, @150008 ])],
           "Quittungen spaetestens je 64 KiB, am Ende Zustand 1 mit allen 150008 Byte");
    NSArray<NSString *> *liste = nil;
    @synchronized (g_fertig_listen) { liste = g_fertig_listen.lastObject; }
    NSArray *oben = ordner ? @[ [ordner stringByAppendingPathComponent:@"Bilder"], [ordner stringByAppendingPathComponent:@"b.bin"],
                                [ordner stringByAppendingPathComponent:@"gross.bin"] ] : @[];
    pruefe(fertig_anzahl() == f0 + 1 && [liste isEqualToArray:oben], "\"fertig\" bekommt die obersten Eintraege als Pfade");
    BOOL bytes = [[NSData dataWithContentsOfFile:[ordner stringByAppendingPathComponent:@"Bilder/a.txt"]] isEqualToData:inhalte[1]] &&
                 [[NSData dataWithContentsOfFile:[ordner stringByAppendingPathComponent:@"gross.bin"]] isEqualToData:inhalte[3]] &&
                 [[NSData dataWithContentsOfFile:[ordner stringByAppendingPathComponent:@"Bilder/sub/c.txt"]] isEqualToData:inhalte[5]] &&
                 [[NSData dataWithContentsOfFile:[ordner stringByAppendingPathComponent:@"b.bin"]] length] == 0;
    pruefe(bytes && qc_empfang_ordner() == nil && existiert(ordner), "Bytes stimmen; das Verzeichnis bleibt zum Einfuegen");
    pruefe(zeilen_mit(@"Dateien: empfangen 6 Eintraege, 0,2 MB - in die Ablage gelegt") == 1, "Protokollzeile wie beim Windows-Host");

    printf("\n-- Empfaenger: Stuecke ausser der Reihe, mit Luecke, doppelt, ueber die Groesse\n");
    struct { const char *name; uint32_t eintrag; uint64_t versatz; size_t n; int vorher; } faelle[] = {
        { "falscher Eintrag (Ordner statt Datei)", 0, 0, 5, 0 },
        { "Datei uebersprungen (erst Eintrag 3)", 3, 0, 5, 0 },
        { "Luecke im Versatz", 1, 2, 3, 0 },
        { "dasselbe Stueck doppelt", 1, 0, 5, 1 },
        { "ueber die angekuendigte Groesse", 1, 0, 6, 0 },
    };
    for (size_t i = 0; i < sizeof faelle / sizeof faelle[0]; i++) {
        uint32_t k = 20 + (uint32_t)i;
        neu_aufzeichnen();
        empfang(QC_DATEI_ANGEBOT, beispiel(k, &inhalte));
        if (faelle[i].vorher) stueck(k, 1, 0, inhalte[1]);
        stueck(k, faelle[i].eintrag, faelle[i].versatz, zufall(faelle[i].n));
        qc_dateien_abwarten();
        NSUInteger nq = quittungen().count;
        stueck(k, 3, 0, zufall(100));                 // Nachzuegler: bleibt ohne Antwort
        qc_dateien_abwarten();
        char was[160];
        snprintf(was, sizeof was, "%s: Quittung 4, Verzeichnis geloescht, Nachzuegler ohne Antwort", faelle[i].name);
        pruefe(letzter_zustand() == QC_QUITT_UNGUELTIG && verzeichnisse_mit(k) == 0 && quittungen().count == nq &&
               qc_empfang_ordner() == nil, was);
    }

    printf("\n-- Empfaenger: Abbrueche\n");
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(30, &inhalte));
    stueck(30, 1, 0, inhalte[1]);
    ende(30, QC_ENDE_VOLLSTAENDIG);
    qc_dateien_abwarten();
    pruefe(letzter_zustand() == QC_QUITT_UNGUELTIG && verzeichnisse_mit(30) == 0, "Ende vor dem letzten Stueck: Quittung 4, geloescht");

    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(31, &inhalte));
    stueck(31, 1, 0, inhalte[1]);
    ende(31, QC_ENDE_ABGEBROCHEN);
    qc_dateien_abwarten();
    pruefe(quittungen().count == 1 && verzeichnisse_mit(31) == 0 && zeilen_mit(@"abgebrochen (Gegenseite: abgebrochen)") >= 1,
           "Ende mit Grund 1: Verzeichnis geloescht, keine weitere Quittung");

    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(32, &inhalte));
    qc_dateien_abwarten();
    NSString *erstes = qc_empfang_ordner();
    empfang(QC_DATEI_ANGEBOT, beispiel(33, &inhalte));
    qc_dateien_abwarten();
    pruefe(erstes && !existiert(erstes) && verzeichnisse_mit(33) == 1, "neues Angebot mitten in einer Uebertragung: die alte wird geloescht");
    alle_stuecke(33, inhalte);
    ende(33, QC_ENDE_VOLLSTAENDIG);
    qc_dateien_abwarten();
    pruefe(letzter_zustand() == QC_QUITT_FERTIG, "die neue laeuft danach vollstaendig durch");

    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(34, &inhalte));
    stueck(34, 1, 0, inhalte[1]);
    qc_dateien_abwarten();
    qc_empfang_nachricht(SITZUNG + 1, 1, QC_DATEI_STUECK, qc_datei_stueck_kodieren(34, 3, 0, "x", 1));
    qc_dateien_abwarten();
    BOOL fremd_still = quittungen().count == 1 && verzeichnisse_mit(34) == 1;
    qc_dateien_sitzung_vorbei(SITZUNG + 1);
    qc_dateien_abwarten();
    pruefe(fremd_still, "Stueck aus einer anderen Sitzung: still uebergangen");
    pruefe(verzeichnisse_mit(34) == 0 && qc_empfang_ordner() == nil && quittungen().count == 1,
           "Sitzung vorbei (Zuschauerwechsel): verworfen und geloescht, ohne Quittung");

    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(35, &inhalte));
    qc_dateien_abwarten();
    qc_empfang_kanal_weg(SITZUNG, 2);                  // ein anderer Kanal: nichts
    qc_dateien_abwarten();
    BOOL bleibt = verzeichnisse_mit(35) == 1;
    qc_empfang_kanal_weg(SITZUNG, 1);
    qc_dateien_abwarten();
    pruefe(bleibt && verzeichnisse_mit(35) == 0, "Eingabekanal der Uebertragung getrennt: geloescht (ein fremder Kanal nicht)");

    printf("\n-- Empfaenger: quittiert, sobald mehr als 16 KiB offen sind\n");
    // Ein Sender, der nur ganze Stuecke ins Fenster setzt, hat im Spielmodus
    // (64 KiB) hoechstens ein Stueck (48 KiB) unterwegs - darauf muss eine
    // Quittung kommen, sonst haengen beide bis zum Stillstand.
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(40, &inhalte));
    stueck(40, 1, 0, inhalte[1]);                                            // 5
    stueck(40, 3, 0, [inhalte[3] subdataWithRange:NSMakeRange(0, 16379)]);  // 16384 offen
    qc_dateien_abwarten();
    NSUInteger bei_16384 = quittungen().count;
    stueck(40, 3, 16379, [inhalte[3] subdataWithRange:NSMakeRange(16379, 1)]);          // 16385
    stueck(40, 3, 16380, [inhalte[3] subdataWithRange:NSMakeRange(16380, QC_DATEI_STUECK_MAX)]);
    qc_dateien_abwarten();
    q = quittungen();
    pruefe(bei_16384 == 1 && [q isEqualToArray:(@[ @[ @0, @0 ], @[ @0, @16385 ], @[ @0, @(16385 + QC_DATEI_STUECK_MAX) ] ])],
           "16384 Byte offen: noch keine Quittung; ab 16385 sofort - auch ein ganzes Stueck (48 KiB) wird quittiert");
    ende(40, QC_ENDE_ABGEBROCHEN);
    qc_dateien_abwarten();

    printf("\n-- Empfaenger: Stillstand (Frist hier 300 ms)\n");
    qc_dateien_stillstand_setzen(300);
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(36, &inhalte));
    stueck(36, 1, 0, inhalte[1]);
    qc_dateien_abwarten();
    schlafen(0.15);
    BOOL noch = verzeichnisse_mit(36) == 1;
    schlafen(0.6);
    qc_dateien_abwarten();
    q = quittungen();
    pruefe(noch && [q.lastObject isEqualToArray:(@[ @6, @5 ])] && verzeichnisse_mit(36) == 0,
           "kein Stueck in der Frist: Quittung 6 mit dem Stand, Verzeichnis geloescht");
    qc_dateien_stillstand_setzen(0);

    printf("\n-- Empfaenger: Platz und Ablageverzeichnis\n");
    neu_aufzeichnen();
    qc_dateien_platzreserve_setzen(UINT64_MAX / 2);
    empfang(QC_DATEI_ANGEBOT, beispiel(37, &inhalte));
    qc_dateien_abwarten();
    qc_dateien_platzreserve_setzen(QC_DATEI_PLATZRESERVE);
    pruefe(letzter_zustand() == QC_QUITT_KEIN_PLATZ && verzeichnisse_mit(37) == 0 && zeilen_mit(@"zu wenig Platz") >= 1,
           "bliebe weniger als die Reserve frei: Quittung 3, nichts angelegt");
    NSString *alt = basis();
    NSString *ziel = [g_wurzel stringByAppendingPathComponent:@"woanders"];
    NSString *verkn = [g_wurzel stringByAppendingPathComponent:@"basis-verknuepfung"];
    mkdir(ziel.fileSystemRepresentation, 0700);
    symlink(ziel.fileSystemRepresentation, verkn.fileSystemRepresentation);
    qc_dateien_basis_setzen(verkn);
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, beispiel(38, &inhalte));
    qc_dateien_abwarten();
    qc_dateien_basis_setzen(alt);
    NSArray *drin = [[NSFileManager defaultManager] contentsOfDirectoryAtPath:ziel error:nil];
    pruefe(letzter_zustand() == QC_QUITT_SCHREIBFEHLER && drin.count == 0,
           "Basis ist eine Verknuepfung: abgelehnt (Quittung 5), ihr Ziel bleibt unberuehrt");
    NSData *boese = qc_datei_angebot_kodieren(39, @[ E(0, 1, @"../ausbruch.txt") ]);
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, boese);
    qc_dateien_abwarten();
    pruefe(letzter_zustand() == QC_QUITT_UNGUELTIG && !existiert([basis().stringByDeletingLastPathComponent stringByAppendingPathComponent:@"ausbruch.txt"]) &&
           verzeichnisse_mit(39) == 0, "feindliches Angebot (../): Quittung 4, nichts angelegt");
}

// ------------------------------------------------------------------- Sender

static NSString *quelle(NSString *rel) { return [[g_wurzel stringByAppendingPathComponent:@"quelle"] stringByAppendingPathComponent:rel]; }

static void schreiben(NSString *rel, NSData *d) {
    NSString *p = quelle(rel);
    [[NSFileManager defaultManager] createDirectoryAtPath:p.stringByDeletingLastPathComponent withIntermediateDirectories:YES
                                               attributes:nil error:nil];
    [d writeToFile:p atomically:NO];
}

// Pfade des zuletzt gesendeten Angebots.
static NSArray<NSString *> *angebot_pfade(uint32_t *kennung) {
    NSData *a = nachrichten(QC_DATEI_ANGEBOT).lastObject;
    NSArray<QCDateiEintrag *> *e = nil;
    uint32_t k = 0; uint64_t g;
    if (!a || qc_datei_angebot_lesen(a.bytes, a.length, &k, &g, &e, NULL) != 0) return nil;
    if (kennung) *kennung = k;
    NSMutableArray *p = [NSMutableArray array];
    for (QCDateiEintrag *x in e) [p addObject:[x.teile componentsJoinedByString:@"/"]];
    return p;
}

static int letztes_ende(void) {
    NSData *e = nachrichten(QC_DATEI_ENDE).lastObject;
    return e ? ((const uint8_t *)e.bytes)[4] : -1;
}

static void senden(NSArray<NSString *> *rel) {
    NSMutableArray *p = [NSMutableArray array];
    for (NSString *r in rel) [p addObject:quelle(r)];
    qc_senden_starten(SITZUNG, p, @"192.0.2.9");
}

static void sender_pruefen(void) {
    printf("\n-- Sender und Empfaenger gegeneinander (im Speicher)\n");
    qc_dateien_stillstand_setzen(5000);
    schreiben(@"Ordner/b.txt", [@"zehn Bytes" dataUsingEncoding:NSUTF8StringEncoding]);
    schreiben(@"Ordner/a.txt", [NSData data]);
    schreiben(@"Ordner/sub/tief.bin", zufall(300000));
    schreiben(@"einzeln.bin", zufall(100000));
    mkdir(quelle(@"leer").fileSystemRepresentation, 0700);
    symlink("../einzeln.bin", quelle(@"Ordner/link").fileSystemRepresentation);
    atomic_store(&g_schleife, 1);
    neu_aufzeichnen();
    NSUInteger f0 = fertig_anzahl();
    senden(@[ @"Ordner", @"einzeln.bin", @"leer" ]);
    qc_dateien_abwarten();
    NSArray *pfade = angebot_pfade(NULL);
    printf("         (Angebot: %s)\n", [pfade componentsJoinedByString:@", "].UTF8String);
    pruefe([pfade isEqualToArray:(@[ @"Ordner", @"Ordner/a.txt", @"Ordner/b.txt", @"Ordner/sub", @"Ordner/sub/tief.bin",
                                     @"einzeln.bin", @"leer" ])],
           "Angebot: Vorfahr vor Nachfahr, nach Namen sortiert, Verknuepfung ausgelassen");
    pruefe(zeilen_mit(@"uebersprungen (symbolische Verknuepfung)") == 1, "die Verknuepfung steht im Protokoll");
    NSArray<NSString *> *liste = nil;
    @synchronized (g_fertig_listen) { liste = g_fertig_listen.lastObject; }
    BOOL da = fertig_anzahl() == f0 + 1 && liste.count == 3 &&
              gleich(liste[0], quelle(@"Ordner")) == NO &&           // dort fehlt die Verknuepfung
              !existiert([liste[0] stringByAppendingPathComponent:@"link"]);
    unlink(quelle(@"Ordner/link").fileSystemRepresentation);
    da = da && gleich(liste[0], quelle(@"Ordner")) && gleich(liste[1], quelle(@"einzeln.bin")) && gleich(liste[2], quelle(@"leer"));
    pruefe(da, "beim Empfaenger: derselbe Baum, Byte fuer Byte, leere Datei und leerer Ordner inklusive");
    pruefe(zeilen_mit(@"Dateien: sende 7 Eintraege, 0,4 MB an 192.0.2.9") == 1 &&
           zeilen_mit(@"Dateien: gesendet und quittiert (0,4 MB in ") == 1, "Protokollzeilen wie beim Windows-Host");
    pruefe(g_max_unterwegs <= QC_DATEI_FENSTER, "nie mehr als das Fenster unquittiert unterwegs");

    printf("\n-- Fenster (Quittungen 20 ms verzoegert)\n");
    schreiben(@"eine.mb", zufall(1000000));
    atomic_store(&g_verzoegern_ms, 20);
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    qc_dateien_abwarten();
    printf("         (hoechstens %llu Byte unquittiert unterwegs)\n", g_max_unterwegs);
    pruefe(g_max_unterwegs == QC_DATEI_FENSTER && letzter_zustand() == QC_QUITT_FERTIG,
           "genau das Fenster (256 KiB) wird ausgeschoepft, nie mehr - und die Uebertragung wird fertig");
    atomic_store(&g_spiel, 1);
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    qc_dateien_abwarten();
    atomic_store(&g_spiel, 0);
    printf("         (Spielmodus: hoechstens %llu Byte unquittiert unterwegs)\n", g_max_unterwegs);
    pruefe(g_max_unterwegs == QC_DATEI_FENSTER_SPIEL && letzter_zustand() == QC_QUITT_FERTIG,
           "Spielmodus: 64 KiB, und mit Quittungen je 64 KiB haengt nichts");
    atomic_store(&g_verzoegern_ms, 0);

    printf("\n-- Sender ohne Quittungen (Stillstand hier 400 ms)\n");
    atomic_store(&g_schleife, 0);
    qc_dateien_stillstand_setzen(400);
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    qc_dateien_abwarten();
    pruefe(stueck_summe() == QC_DATEI_FENSTER && letztes_ende() == QC_ENDE_ZEIT && zeilen_mit(@"keine Quittung") >= 1,
           "genau 256 KiB hinaus, dann ENDE mit Grund 3");
    atomic_store(&g_spiel, 1);
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    qc_dateien_abwarten();
    atomic_store(&g_spiel, 0);
    pruefe(stueck_summe() == QC_DATEI_FENSTER_SPIEL && letztes_ende() == QC_ENDE_ZEIT, "im Spielmodus genau 64 KiB");
    qc_dateien_stillstand_setzen(5000);

    printf("\n-- Nachrang: Sendepuffer ueber 128 KiB\n");
    atomic_store(&g_schleife, 1);
    atomic_store(&g_rueckstand, 200 * 1024);
    neu_aufzeichnen();
    senden(@[ @"einzeln.bin" ]);
    schlafen(0.3);
    NSUInteger waehrend = nachrichten(QC_DATEI_STUECK).count, angebote = nachrichten(QC_DATEI_ANGEBOT).count;
    atomic_store(&g_rueckstand, 100 * 1024);
    qc_dateien_abwarten();
    atomic_store(&g_rueckstand, 0);
    pruefe(angebote == 0 && waehrend == 0 && nachrichten(QC_DATEI_ANGEBOT).count == 1 && letzter_zustand() == QC_QUITT_FERTIG,
           "solange mehr als 128 KiB im Sendepuffer liegen, geht weder das Angebot noch ein Stueck hinaus; danach laeuft es durch");

    printf("\n-- Abbrueche beim Sender\n");
    // Der Empfaenger bekommt alles, seine Quittungen erreichen den Sender
    // nicht: der steht sicher am vollen Fenster, wenn der Abbruch kommt.
    atomic_store(&g_halten, 1);
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);
    qc_senden_abbrechen();
    qc_dateien_abwarten();
    uint32_t k = 0;
    angebot_pfade(&k);
    schlafen(0.1);
    qc_dateien_abwarten();
    pruefe(letztes_ende() == QC_ENDE_ABGEBROCHEN && verzeichnisse_mit(k) == 0 && zeilen_mit(@"abgebrochen (neuer Inhalt in der Ablage)") >= 1,
           "neuer Inhalt in der Ablage: ENDE mit Grund 1, der Empfaenger loescht");

    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);
    atomic_store(&g_sitzung_gilt, 0);
    qc_dateien_sitzung_vorbei(SITZUNG + 1);
    qc_dateien_abwarten();
    atomic_store(&g_sitzung_gilt, 1);
    angebot_pfade(&k);
    pruefe(letztes_ende() == -1 && verzeichnisse_mit(k) == 0 && zeilen_mit(@"abgebrochen (Zuschauer gewechselt oder weg)") >= 1,
           "Sitzung vorbei: der Sender hoert auf (ohne ENDE, die Leitung ist weg), der Empfaenger loescht");
    atomic_store(&g_halten, 0);

    // Wird eine Datei waehrend des Sendens kuerzer: ENDE mit Grund 2.
    atomic_store(&g_schleife, 0);
    schreiben(@"schrumpft.bin", zufall(600000));
    neu_aufzeichnen();
    senden(@[ @"schrumpft.bin" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);
    truncate(quelle(@"schrumpft.bin").fileSystemRepresentation, 100000);
    angebot_pfade(&k);
    NSData *qq = qc_datei_quittung_kodieren(k, 0, QC_DATEI_FENSTER);
    qc_senden_quittung(SITZUNG, qq.bytes, qq.length);
    qc_dateien_abwarten();
    pruefe(letztes_ende() == QC_ENDE_LESEFEHLER && zeilen_mit(@"kuerzer als angekuendigt") == 1,
           "Datei kuerzer als angekuendigt: ENDE mit Grund 2");

    // Wird eine Datei nach dem Auflisten gegen eine FIFO getauscht, darf das
    // Oeffnen die Warteschlange nicht blockieren (niemand schreibt hinein).
    schreiben(@"vorne.bin", zufall(300000));
    schreiben(@"wird-fifo", [@"x" dataUsingEncoding:NSUTF8StringEncoding]);
    neu_aufzeichnen();
    senden(@[ @"vorne.bin", @"wird-fifo" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);   // steht am Fenster
    NSString *fifo = quelle(@"wird-fifo");
    unlink(fifo.fileSystemRepresentation);
    mkfifo(fifo.fileSystemRepresentation, 0600);
    angebot_pfade(&k);
    for (int i = 0; i < 200 && letztes_ende() == -1; i++) {
        NSData *weiter = qc_datei_quittung_kodieren(k, 0, stueck_summe());
        qc_senden_quittung(SITZUNG, weiter.bytes, weiter.length);
        schlafen(0.01);
    }
    BOOL haengt = letztes_ende() == -1;
    if (haengt) {
        // Nur wenn es doch haengt: den Sender mit einem Schreiber freigeben,
        // damit der Pruefstand weiterlaeuft.
        int w = open(fifo.fileSystemRepresentation, O_WRONLY | O_NONBLOCK);
        if (w >= 0) close(w);
    }
    qc_dateien_abwarten();
    unlink(fifo.fileSystemRepresentation);
    pruefe(!haengt && letztes_ende() == QC_ENDE_LESEFEHLER && zeilen_mit(@"keine Datei mehr") == 1,
           "Datei nach dem Auflisten gegen eine FIFO getauscht: kein Haengen, ENDE mit Grund 2");

    // Eine Quittung ungleich 0 beendet ohne ENDE; eine fremde Kennung zaehlt nicht.
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);
    angebot_pfade(&k);
    NSData *fremd = qc_datei_quittung_kodieren(k + 100, 0, QC_DATEI_FENSTER);
    qc_senden_quittung(SITZUNG, fremd.bytes, fremd.length);
    schlafen(0.1);
    uint64_t nach_fremd = stueck_summe();
    NSData *drei = qc_datei_quittung_kodieren(k, QC_QUITT_KEIN_PLATZ, 0);
    qc_senden_quittung(SITZUNG, drei.bytes, drei.length);
    qc_dateien_abwarten();
    pruefe(nach_fremd == QC_DATEI_FENSTER && letztes_ende() == -1 && zeilen_mit(@"abgelehnt (Gegenseite: zu wenig Platz)") == 1,
           "Quittung mit fremder Kennung zaehlt nicht; Quittung 3 beendet ohne ENDE");

    // Eine Quittung ueber dem, was hinausging, zaehlt nur bis dorthin: das
    // Fenster oeffnet sich um genau 256 KiB, nicht beliebig weit (und die
    // Rechnung laeuft nicht ueber, sonst stuende der Sender).
    neu_aufzeichnen();
    senden(@[ @"eine.mb" ]);
    for (int i = 0; i < 100 && stueck_summe() < QC_DATEI_FENSTER; i++) schlafen(0.01);
    angebot_pfade(&k);
    NSData *zuviel = qc_datei_quittung_kodieren(k, 0, 1ull << 40);
    qc_senden_quittung(SITZUNG, zuviel.bytes, zuviel.length);
    for (int i = 0; i < 100 && stueck_summe() < 2 * QC_DATEI_FENSTER; i++) schlafen(0.01);
    schlafen(0.1);
    uint64_t nach_zuviel = stueck_summe();
    qc_senden_abbrechen();
    qc_dateien_abwarten();
    printf("         (nach der ueberhoehten Quittung %llu Byte hinaus)\n", nach_zuviel);
    pruefe(nach_zuviel == 2 * QC_DATEI_FENSTER && letztes_ende() == QC_ENDE_ABGEBROCHEN,
           "Quittung ueber dem Gesendeten: gilt nur bis zum Gesendeten, das Fenster oeffnet sich um genau 256 KiB");

    printf("\n-- Was der Sender gar nicht erst sendet\n");
    schreiben(@"x/dup.txt", [@"x" dataUsingEncoding:NSUTF8StringEncoding]);
    schreiben(@"y/dup.txt", [@"y" dataUsingEncoding:NSUTF8StringEncoding]);
    atomic_store(&g_schleife, 1);
    neu_aufzeichnen();
    senden(@[ @"x/dup.txt", @"y/dup.txt" ]);
    qc_dateien_abwarten();
    NSData *erstes = nil;
    @synchronized (g_fertig_listen) { erstes = [NSData dataWithContentsOfFile:[g_fertig_listen.lastObject firstObject]]; }
    pruefe([angebot_pfade(NULL) isEqualToArray:@[ @"dup.txt" ]] && [erstes isEqualToData:[@"x" dataUsingEncoding:NSUTF8StringEncoding]] &&
           zeilen_mit(@"doppelter Name dup.txt") == 1, "doppelte oberste Namen: nur der erste, mit Protokollzeile");

    // Namen, die erst nach der Bereinigung der Gegenseite gleich sind
    // ("a\x01" wird dort zu "a_"): nur der erste geht hinaus, sonst lehnte
    // der Empfaenger das ganze Angebot als doppelt ab.
    schreiben(@"steuer/a\x01", [@"1" dataUsingEncoding:NSUTF8StringEncoding]);
    schreiben(@"steuer/a_", [@"2" dataUsingEncoding:NSUTF8StringEncoding]);
    neu_aufzeichnen();
    senden(@[ @"steuer" ]);
    qc_dateien_abwarten();
    NSArray<NSString *> *steuer = angebot_pfade(NULL);
    printf("         (Angebot: %s)\n", [steuer componentsJoinedByString:@", "].UTF8String ?: "abgelehnt");
    pruefe(steuer.count == 2 && letzter_zustand() == QC_QUITT_FERTIG && zeilen_mit(@"nach Bereinigung") == 1,
           "Namen nur nach der Bereinigung doppelt (a\\x01, a_): nur der erste, mit Protokollzeile; der Empfaenger nimmt an");

    // Was aus einem Empfang stammt (liegt in der eigenen Basis), geht nicht zurueck.
    NSString *empfangen = nil;
    @synchronized (g_fertig_listen) { empfangen = [g_fertig_listen.lastObject firstObject]; }
    neu_aufzeichnen();
    if (empfangen) qc_senden_starten(SITZUNG, @[ empfangen ], @"192.0.2.9");
    qc_dateien_abwarten();
    pruefe(empfangen && nachrichten(QC_DATEI_ANGEBOT).count == 0 && zeilen_mit(@"stammen aus einem Empfang") == 1,
           "Dateien aus dem eigenen Ablageverzeichnis: nichts gesendet (kein Widerhall)");

    // Grenzen: ueber 4 GiB (duenn besetzte Datei) und ueber 10000 Eintraege.
    NSString *riesig = quelle(@"riesig.bin");
    [[NSData data] writeToFile:riesig atomically:NO];
    truncate(riesig.fileSystemRepresentation, (off_t)QC_DATEI_GESAMT_MAX + 1);
    neu_aufzeichnen();
    senden(@[ @"riesig.bin" ]);
    qc_dateien_abwarten();
    unlink(riesig.fileSystemRepresentation);
    int zeilen_gross = zeilen_mit(@"mehr als 4 GB oder 10000 Eintraege");
    pruefe(nachrichten(QC_DATEI_ANGEBOT).count == 0 && zeilen_gross == 1, "ueber 4 GiB: nichts gesendet, eine Protokollzeile");
    mkdir(quelle(@"viele").fileSystemRepresentation, 0700);
    for (int i = 0; i < 10000; i++) {
        int fd = open([quelle(@"viele") stringByAppendingFormat:@"/%05d", i].fileSystemRepresentation, O_WRONLY | O_CREAT, 0600);
        if (fd >= 0) close(fd);
    }
    neu_aufzeichnen();
    senden(@[ @"viele" ]);                             // 10001 Eintraege mit dem Ordner
    qc_dateien_abwarten();
    pruefe(nachrichten(QC_DATEI_ANGEBOT).count == 0 && zeilen_mit(@"mehr als 4 GB oder 10000 Eintraege") == zeilen_gross + 1,
           "10001 Eintraege: nichts gesendet");
    unlink([quelle(@"viele") stringByAppendingPathComponent:@"00000"].fileSystemRepresentation);
    neu_aufzeichnen();
    senden(@[ @"viele" ]);                             // genau 10000
    qc_dateien_abwarten();
    pruefe(nachrichten(QC_DATEI_ANGEBOT).count == 1 && letzter_zustand() == QC_QUITT_FERTIG, "genau 10000 Eintraege: gesendet");
    [[NSFileManager defaultManager] removeItemAtPath:quelle(@"viele") error:nil];
    atomic_store(&g_schleife, 0);
    qc_dateien_stillstand_setzen(0);
}

// --------------------------------------------------------------- Aufraeumen

static void aufraeumen_pruefen(void) {
    printf("\n-- Aufraeumen des Ablageverzeichnisses\n");
    NSString *alt = basis();
    NSString *b = [g_wurzel stringByAppendingPathComponent:@"aufraeumen"];
    NSString *draussen = [g_wurzel stringByAppendingPathComponent:@"draussen"];
    mkdir(b.fileSystemRepresentation, 0700);
    mkdir(draussen.fileSystemRepresentation, 0700);
    [[@"bleibt" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:[draussen stringByAppendingPathComponent:@"datei"] atomically:NO];
    for (int i = 1; i <= 5; i++) mkdir([b stringByAppendingFormat:@"/%d00-%d", i, i].fileSystemRepresentation, 0700);
    mkdir([b stringByAppendingPathComponent:@"fremd"].fileSystemRepresentation, 0700);
    symlink(draussen.fileSystemRepresentation, [b stringByAppendingPathComponent:@"600-6"].fileSystemRepresentation);
    symlink([draussen stringByAppendingPathComponent:@"datei"].fileSystemRepresentation,
            [b stringByAppendingPathComponent:@"100-1/zeiger"].fileSystemRepresentation);
    qc_dateien_basis_setzen(b);
    qc_dateien_aufraeumen(3, 0);
    NSArray *rest = [[[NSFileManager defaultManager] contentsOfDirectoryAtPath:b error:nil] sortedArrayUsingSelector:@selector(compare:)];
    printf("         (danach: %s)\n", [rest componentsJoinedByString:@" "].UTF8String);
    pruefe([rest isEqualToArray:(@[ @"300-3", @"400-4", @"500-5", @"600-6", @"fremd" ])],
           "die drei neuesten bleiben; fremde Namen und Verknuepfungen bleiben unberuehrt");
    pruefe(existiert([draussen stringByAppendingPathComponent:@"datei"]),
           "eine Verknuepfung in einem geloeschten Verzeichnis wird nicht verfolgt: ihr Ziel bleibt");
    uint64_t jetzt = (uint64_t)([NSDate date].timeIntervalSince1970 * 1000);
    NSString *vorgestern = [b stringByAppendingFormat:@"/%llu-7", jetzt - 25ull * 3600 * 1000];
    NSString *vorhin = [b stringByAppendingFormat:@"/%llu-8", jetzt - 3600ull * 1000];
    mkdir(vorgestern.fileSystemRepresentation, 0700);
    mkdir(vorhin.fileSystemRepresentation, 0700);
    qc_dateien_aufraeumen_beim_start();
    qc_dateien_abwarten();
    pruefe(!existiert(vorgestern) && existiert(vorhin) && existiert([b stringByAppendingPathComponent:@"fremd"]),
           "beim Start: aelter als 24 h geloescht, juengere bleiben");
    qc_dateien_basis_setzen(alt);
}

// ------------------------------------------------------------- Tiefe Pfade

// Gibt es den Eintrag unter ordner/teile? Bestandteil fuer Bestandteil ueber
// openat, denn der ganze Pfad liegt hier ueber PATH_MAX.
static BOOL tief_da(NSString *ordner, NSArray<NSString *> *teile) {
    int fd = open(ordner.fileSystemRepresentation, O_RDONLY | O_DIRECTORY);
    for (NSUInteger i = 0; fd >= 0 && i + 1 < teile.count; i++) {
        int n = openat(fd, teile[i].UTF8String, O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
        close(fd);
        fd = n;
    }
    struct stat st;
    BOOL da = fd >= 0 && fstatat(fd, teile.lastObject.UTF8String, &st, AT_SYMLINK_NOFOLLOW) == 0;
    if (fd >= 0) close(fd);
    return da;
}

// Legt unter ordner eine Kette von Ordnern an (ueber mkdirat, beliebig lang).
static BOOL tief_anlegen(NSString *ordner, NSArray<NSString *> *teile) {
    if (mkdir(ordner.fileSystemRepresentation, 0700) != 0) return NO;
    int fd = open(ordner.fileSystemRepresentation, O_RDONLY | O_DIRECTORY);
    for (NSUInteger i = 0; fd >= 0 && i < teile.count; i++) {
        int n = mkdirat(fd, teile[i].UTF8String, 0700) == 0 ? openat(fd, teile[i].UTF8String, O_RDONLY | O_DIRECTORY) : -1;
        close(fd);
        fd = n;
    }
    BOOL ok = fd >= 0 && close(fd) == 0;
    return ok;
}

// Wie viele "liess sich nicht ganz loeschen" seit dem letzten Aufruf - so
// prueft jeder Schritt nur seine eigenen.
static int g_loesch_stand = 0;
static int loeschfehler_neu(void) {
    int n = zeilen_mit(@"nicht ganz loeschen"), neu = n - g_loesch_stand;
    g_loesch_stand = n;
    return neu;
}

static NSUInteger eintraege_in(NSString *ordner) {
    return [[NSFileManager defaultManager] contentsOfDirectoryAtPath:ordner error:nil].count;
}

static void tiefe_pfade_pruefen(void) {
    printf("\n-- Tiefe Pfade: relativ bis 1024 Byte, absolut ueber PATH_MAX\n");
    NSString *alt = basis();
    NSString *b = [g_wurzel stringByAppendingPathComponent:@"tief"];
    qc_dateien_basis_setzen(b);
    atomic_store(&g_schleife, 0);
    loeschfehler_neu();
    // 4 Bestandteile zu 255 Byte: 1023 Byte relativ, erlaubt nach 2.5.
    NSString *A = wiederholt('A', 255), *B = wiederholt('B', 255), *C = wiederholt('C', 255), *D = wiederholt('D', 255);
    NSArray<NSString *> *teile = @[ A, B, C, D ];
    NSString *rel = [teile componentsJoinedByString:@"/"];
    NSArray *liste = @[ E(1, 0, A), E(1, 0, [@[ A, B ] componentsJoinedByString:@"/"]),
                        E(1, 0, [@[ A, B, C ] componentsJoinedByString:@"/"]), E(0, 5, rel) ];

    // Abbruch (ENDE mit Grund 1): das ganze Verzeichnis muss weg.
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, qc_datei_angebot_kodieren(60, liste));
    qc_dateien_abwarten();
    NSString *o = qc_empfang_ordner();
    NSUInteger laenge = [[o stringByAppendingPathComponent:rel] lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
    BOOL angelegt = o && tief_da(o, teile);
    printf("         (relativ %lu Byte, absolut %lu Byte, PATH_MAX %d)\n",
           (unsigned long)[rel lengthOfBytesUsingEncoding:NSUTF8StringEncoding], (unsigned long)laenge, PATH_MAX);
    ende(60, QC_ENDE_ABGEBROCHEN);
    qc_dateien_abwarten();
    pruefe(loeschfehler_neu() == 0 && letzter_zustand() == QC_QUITT_LAEUFT && angelegt && laenge > PATH_MAX &&
           !existiert(o) && verzeichnisse_mit(60) == 0,
           "Abbruch mit Grund 1: das Uebertragungsverzeichnis ist ganz geloescht, auch ueber PATH_MAX");

    // Protokollfehler mitten drin: ebenfalls ganz geloescht.
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, qc_datei_angebot_kodieren(61, liste));
    stueck(61, 3, 1, [@"xx" dataUsingEncoding:NSUTF8StringEncoding]);
    qc_dateien_abwarten();
    pruefe(loeschfehler_neu() == 0 && letzter_zustand() == QC_QUITT_UNGUELTIG && verzeichnisse_mit(61) == 0,
           "Stueck ausser der Reihe: Quittung 4, das tiefe Verzeichnis ist ganz geloescht");

    // Vier vollstaendige Uebertragungen: nach jeder bleiben die drei neuesten.
    BOOL fertig_alle = YES;
    for (uint32_t k = 62; k < 66; k++) {
        neu_aufzeichnen();
        empfang(QC_DATEI_ANGEBOT, qc_datei_angebot_kodieren(k, liste));
        stueck(k, 3, 0, [@"hallo" dataUsingEncoding:NSUTF8StringEncoding]);
        ende(k, QC_ENDE_VOLLSTAENDIG);
        qc_dateien_abwarten();
        fertig_alle = fertig_alle && letzter_zustand() == QC_QUITT_FERTIG;
        schlafen(0.005);
    }
    printf("         (danach in der Basis: %lu)\n", (unsigned long)eintraege_in(b));
    pruefe(loeschfehler_neu() == 0 && fertig_alle && eintraege_in(b) == 3 && verzeichnisse_mit(62) == 0 &&
           verzeichnisse_mit(65) == 1,
           "nach vier vollstaendigen tiefen Uebertragungen bleiben genau die drei neuesten");

    // 32 Stufen (das Hoechste, was ein Angebot haben darf): ganz geloescht.
    NSMutableArray<NSString *> *stufen = [NSMutableArray array];
    for (int i = 0; i < 32; i++) [stufen addObject:[NSString stringWithFormat:@"s%d", i]];
    neu_aufzeichnen();
    empfang(QC_DATEI_ANGEBOT, qc_datei_angebot_kodieren(66, mit_eltern(0, 1, [stufen componentsJoinedByString:@"/"])));
    qc_dateien_abwarten();
    NSString *o32 = qc_empfang_ordner();
    BOOL da32 = o32 && tief_da(o32, stufen);
    ende(66, QC_ENDE_ABGEBROCHEN);
    qc_dateien_abwarten();
    pruefe(loeschfehler_neu() == 0 && da32 && verzeichnisse_mit(66) == 0,
           "32 Stufen: angelegt und beim Abbruch ganz geloescht");

    // Beim Start: ein Rest aelter als 24 h, tiefer als PATH_MAX, geht weg.
    uint64_t jetzt = (uint64_t)([NSDate date].timeIntervalSince1970 * 1000);
    NSString *rest = [b stringByAppendingFormat:@"/%llu-67", jetzt - 25ull * 3600 * 1000];
    NSArray<NSString *> *lang = @[ A, B, C, D, A, B ];
    BOOL rest_da = tief_anlegen(rest, lang) && tief_da(rest, lang);
    qc_dateien_aufraeumen_beim_start();
    qc_dateien_abwarten();
    pruefe(loeschfehler_neu() == 0 && rest_da && !existiert(rest) && verzeichnisse_mit(65) == 1,
           "beim Start: ein alter Rest mit ueber 1500 Byte Tiefe wird ganz geloescht, die juengeren bleiben");

    // Zum Schluss alles weg (auch damit unter $TMPDIR nichts liegen bleibt).
    schlafen(0.005);
    qc_dateien_aufraeumen(0, 1);
    pruefe(loeschfehler_neu() == 0 && eintraege_in(b) == 0, "Aufraeumen nach Alter entfernt auch die tiefen Uebertragungen");

    qc_dateien_basis_setzen(alt);
}

// ---------------------------------------------------------------------- Lauf

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IONBF, 0);
        const char *tmp = getenv("TMPDIR");
        NSString *vorlage = [[NSString stringWithUTF8String:tmp && *tmp ? tmp : "/tmp"]
                             stringByAppendingPathComponent:@"qc-dateitest-XXXXXX"];
        char w[PATH_MAX];
        strlcpy(w, vorlage.fileSystemRepresentation, sizeof w);
        if (!mkdtemp(w)) { printf("kein Pruefordner\n"); return 1; }
        g_wurzel = @(w);
        printf("Pruefordner %s\n", w);
        g_zeilen = [NSMutableArray array];
        g_gesendet = [NSMutableArray array];
        g_fertig_listen = [NSMutableArray array];
        static const qc_dateien_wege wege = { weg_senden, weg_rueckstand, weg_spiel, protokoll };
        qc_dateien_einrichten(&wege);
        qc_dateien_basis_setzen([g_wurzel stringByAppendingPathComponent:@"ablage"]);
        qc_dateien_fertig_setzen(fertig);
        // Der Stand der Sitzung ist SITZUNG, wie ihn main.m meldete.
        qc_dateien_sitzung_vorbei(SITZUNG);
        vektoren_pruefen();
        pfade_pruefen();
        empfaenger_pruefen();
        sender_pruefen();
        aufraeumen_pruefen();
        tiefe_pfade_pruefen();
        qc_dateien_abwarten();
        [[NSFileManager defaultManager] removeItemAtPath:g_wurzel error:nil];
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
