// Synthetisches Testbild fuer den Benchmark.
//
// Der Benchmark des Clients misst Codecs, Datenraten und Bildraten der Reihe
// nach. Vergleichbar sind die Schritte nur, wenn jeder denselben Inhalt
// sieht - der Bildschirm des Macs ist das nie. Deshalb liefert der Host auf
// Wunsch statt der Aufnahme eine feste Schleife vorgerenderter Bilder:
// Farbbalken (Farbe), feine Streifen (Schaerfe, kostet Datenrate), ein
// Verlauf und ein wanderndes Quadrat (Bewegung). Vorgerendert, damit die
// Hostlast waehrend der Messung vom Encoder kommt und nicht vom Malen.
#ifndef QC_TESTBILD_H
#define QC_TESTBILD_H

#include <CoreVideo/CoreVideo.h>

/// Schleife im gegebenen Format und in der Groesse anlegen (oder neu anlegen,
/// wenn sich etwas geaendert hat). Nur auf der Aufnahmewarteschlange rufen.
void qc_testbild_start(int w, int h, OSType fmt);

/// Alles freigeben. Nur auf der Aufnahmewarteschlange rufen.
void qc_testbild_stop(void);

/// Das naechste Bild der Schleife. Gehoert dem Modul, nicht dem Aufrufer;
/// gueltig bis zum naechsten stop. NULL ohne Schleife.
CVPixelBufferRef qc_testbild_naechstes(void);

#endif
