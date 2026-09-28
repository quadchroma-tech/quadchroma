// Synthetisches Testbild fuer den Benchmark.
//
// Der Benchmark des Clients misst Codecs, Datenraten und Bildraten der Reihe
// nach. Vergleichbar sind die Schritte nur, wenn jeder denselben Inhalt
// sieht - der Bildschirm des Macs ist das nie. Deshalb liefert der Host auf
// Wunsch statt der Aufnahme eine feste Schleife vorgerenderter Bilder:
// Farbbalken (Farbe), feine Streifen (Schaerfe, kostet Datenrate), ein
// Verlauf und ein wanderndes Quadrat (Bewegung). Vorgerendert, damit die
// Hostlast waehrend der Messung vom Encoder kommt und nicht vom Malen.
//
// HDR (pq = 1, nur xf44): dieselbe Anordnung in HDR10 - BT.2020, PQ,
// Y'CbCr BT.2020-NCL im vollen Bereich, als Anhaenge am Puffer (VideoToolbox
// rechnet dann nichts um). Farbbalken und Streifen sind die SDR-Farben mit
// SDR-Weiss = QC_TESTBILD_PQ_WEISS_NIT (203 nit, wie die Strominfo des Hosts),
// unten eine PQ-Rampe 0 bis 1000 nit, darauf das Quadrat in vierfacher
// SDR-Helligkeit - fuer Benchmark und Abnahme.
#ifndef QC_TESTBILD_H
#define QC_TESTBILD_H

#include <CoreVideo/CoreVideo.h>

#define QC_TESTBILD_PQ_WEISS_NIT 203.0
#define QC_TESTBILD_PQ_RAMPE_NIT 1000.0

/// Schleife im gegebenen Format und in der Groesse anlegen (oder neu anlegen,
/// wenn sich etwas geaendert hat). pq: HDR10 statt SDR (nur mit xf44; sonst
/// SDR). Nur auf der Aufnahmewarteschlange rufen.
void qc_testbild_start(int w, int h, OSType fmt, int pq);

/// Ein Bildpunkt des HDR-Testbilds als 10-Bit-Codes (Y', Cb, Cr; voller
/// Bereich, BT.2020-NCL) - fuer die Pruefstaende. k: Bild der Schleife.
void qc_testbild_punkt_pq(int x, int y, int w, int h, int k, int *yy, int *cb, int *cr);

/// Alles freigeben. Nur auf der Aufnahmewarteschlange rufen.
void qc_testbild_stop(void);

/// Das naechste Bild der Schleife. Gehoert dem Modul, nicht dem Aufrufer;
/// gueltig bis zum naechsten stop. NULL ohne Schleife.
CVPixelBufferRef qc_testbild_naechstes(void);

#endif
