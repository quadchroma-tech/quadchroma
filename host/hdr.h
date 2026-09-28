// HDR10 (BT.2020, PQ): der C-Spiegel von client/src/hdr.rs fuer den Mac-Host.
//
// Hier steht, was der Host in C braucht: die Bytes der Strominfo Fassung 1
// (MSG_INFO, 24 Byte), das Lesen von IN_ANZEIGE (Typ 71), die Entscheidung
// qc_hdr_entscheiden samt Grund, PQ (SMPTE ST 2084) und der Bau der
// Praefix-SEI mit MDCV (137) und CLL (144). Die Rechnung selbst ist die von
// client/src/hdr.rs; massgeblich sind die gemeinsamen Pruefvektoren in
// client/src/hdr_vektoren.txt. Die Rust-Tests (hdr::tests, nur auf dem Mac)
// rufen diese Funktionen aus libqchost.a und pruefen sie gegen dieselben
// Zeilen - wer hier etwas aendert, aendert es in hdr.rs ebenso.
//
// Ohne AppKit, ohne Netz, ohne Zustand: alle Funktionen sind fadensicher.
//
// Strominfo Fassung 1 (Host -> Client, Typ 1): Bytes 0-7 wie bisher (u16
// Breite, u16 Hoehe, u16 fps, u8 Codec, u8 Format), dahinter 16 Byte:
//   [8] u8 Fassung = 1   [9] u8 Transfer nach H.273 (1 SDR, 16 PQ, 18 HLG
//   reserviert)   [10] u8 Primaerfarben (1 BT.709, 9 BT.2020)   [11] u8 Matrix
//   (1 BT.709, 9 BT.2020-NCL)   [12] u8 voller Bereich = 1   [13] u8 HDR-Grund
//   (QC_HDR_GRUND_*)   [14..15] u16 SDR-Weiss des Hosts in nit
//   [16..17] u16 Mastering max nit   [18..19] u16 Mastering min in 0,0001 nit
//   [20..21] u16 MaxCLL   [22..23] u16 MaxFALL (alle u16 little endian,
//   0 = unbekannt).
// IN_ANZEIGE (Client -> Host, Typ 71), 14 Byte:
//   [0] u8 Fassung = 1   [1] u8 Flags (Bit 0 Bildschirm HDR-faehig, Bit 1
//   Client kann HDR darstellen)   [2] u8 Wunsch (0 Automatisch, 1 Aus,
//   2 Immer)   [3] u8 frei   [4..5] u16 SDR-Weiss nit   [6..7] u16 Spitze nit
//   [8..9] u16 Vollbild-Spitze nit   [10..11] u16 Kopfraum potentiell x100
//   [12..13] u16 Kopfraum aktuell x100.
// SWITCH (Typ 7) p[6]: Transfer nach H.273 (Hosts vor 0.2.0: 0 = SDR).
#ifndef QC_HDR_H
#define QC_HDR_H

#include <stddef.h>
#include <stdint.h>

// ---------------------------------------------------------------- Protokoll

#define QC_FAEHIG_HDR            4u     // Bit 2 in MSG_FAEHIGKEITEN: versteht IN_ANZEIGE, sendet INFO Fassung 1
#define QC_IN_ANZEIGE            71     // Client -> Host: Lage der Anzeige (14 Byte)

#define QC_HDR_INFO_FASSUNG      1
#define QC_HDR_INFO_LAENGE       24     // ganze Strominfo Fassung 1
#define QC_HDR_INFO_ALT          8      // Strominfo vor 0.2.0 (und Bytes 0-7 von Fassung 1)
#define QC_HDR_ANZEIGE_FASSUNG   1
#define QC_HDR_ANZEIGE_LAENGE    14

// Codes nach H.273.
#define QC_HDR_TRANSFER_SDR      1      // BT.709
#define QC_HDR_TRANSFER_PQ       16     // SMPTE ST 2084
#define QC_HDR_TRANSFER_HLG      18     // reserviert, nicht in 0.2.0
#define QC_HDR_PRIMAER_709       1
#define QC_HDR_PRIMAER_2020      9
#define QC_HDR_MATRIX_709        1
#define QC_HDR_MATRIX_2020_NCL   9

// HDR-Grund, Byte 13 der Strominfo.
#define QC_HDR_GRUND_AKTIV               0   // HDR laeuft
#define QC_HDR_GRUND_CLIENT_SDR          1   // Client-Schirm SDR oder Wunsch Aus
#define QC_HDR_GRUND_CODEC               2   // Codec nicht HEVC 10 Bit
#define QC_HDR_GRUND_HOST_SCHIRM_SDR     3   // aufgenommener Bildschirm des Hosts SDR
#define QC_HDR_GRUND_HOST_KANN_NICHT     4   // Betriebssystem oder Encoder
#define QC_HDR_GRUND_CLIENT_OHNE_DARSTELLUNG 5 // Client kann HDR nicht darstellen (Bit 1 = 0)
#define QC_HDR_GRUND_WECHSEL_GESCHEITERT 6   // der Wechsel nach HDR schlug fehl
#define QC_HDR_GRUND_KEIN_IN_ANZEIGE     7   // der Client hat IN_ANZEIGE nicht gesendet

// Wunsch und Flags in IN_ANZEIGE.
#define QC_HDR_WUNSCH_AUTOMATISCH  0
#define QC_HDR_WUNSCH_AUS          1
#define QC_HDR_WUNSCH_IMMER        2
#define QC_HDR_ANZEIGE_SCHIRM_HDR  1u   // Bit 0: Bildschirm des Clients HDR-faehig
#define QC_HDR_ANZEIGE_DARSTELLUNG 2u   // Bit 1: Client kann HDR darstellen

// ------------------------------------------------------------- Strominfo

// Die Farbe des Stroms und die Metadaten fuer Bytes 8-23.
typedef struct {
    uint8_t transfer, primaer, matrix, voll, grund;
    uint16_t sdr_weiss_nit;
    uint16_t master_max_nit;
    uint16_t master_min_zehntausendstel;   // in 0,0001 nit
    uint16_t max_cll, max_fall;
} qc_hdr_info;

// SDR (BT.709, voll) mit diesem Grund, alle Metadaten 0.
void qc_hdr_info_sdr(qc_hdr_info *i, uint8_t grund);

// Bytes 8-23 der Strominfo (16 Byte, beginnend mit der Fassung).
void qc_hdr_info_kodieren(const qc_hdr_info *i, uint8_t out[QC_HDR_INFO_LAENGE - QC_HDR_INFO_ALT]);

// ------------------------------------------------------------- IN_ANZEIGE

typedef struct {
    uint8_t flags, wunsch;
    uint16_t sdr_weiss_nit, spitze_nit, vollbild_spitze_nit;
    uint16_t kopfraum_potentiell, kopfraum_aktuell;   // x100
} qc_hdr_anzeige;

// 1 = gelesen: mindestens 14 Byte und Fassung 1 (weitere Bytes werden
// uebergangen). Sonst 0 und *a bleibt unberuehrt - dann gilt es wie kein
// IN_ANZEIGE.
int qc_hdr_anzeige_lesen(const uint8_t *p, size_t n, qc_hdr_anzeige *a);

// ----------------------------------------------------------- Entscheidung

// Kann dieser Kandidat HDR10 tragen? Nur HEVC 10 Bit: 0 (4:4:4) und 2 (4:2:0).
int qc_hdr_codec_kann(int idx);

// Die Entscheidung des Hosts: QC_HDR_GRUND_AKTIV (0) = HDR senden, sonst der
// Grund fuer SDR. quelle_hdr: der aufgenommene Bildschirm ist HDR;
// host_kann: Betriebssystem und Encoder koennen HDR10 mit diesem Kandidaten;
// a: das zuletzt gelesene IN_ANZEIGE oder NULL. Reihenfolge der Gruende wie
// in hdr.rs: 7, Wunsch Aus oder unbekannt 1, 2, 4, 3, 5, Client-Schirm SDR
// bei Automatisch 1.
int qc_hdr_entscheiden(int quelle_hdr, int host_kann, int idx, const qc_hdr_anzeige *a);

// Der Grund als deutscher Text fuer das Protokoll (wie hdr::grund_text).
const char *qc_hdr_grund_text(int grund);

// -------------------------------------------------------------------- PQ

// SMPTE ST 2084: Pegel in nit -> Signal 0..1, und zurueck.
double qc_hdr_pq_aus_nit(double nit);
double qc_hdr_nit_aus_pq(double e);
// PQ-Code (10 Bit, voller Bereich), gerundet.
int qc_hdr_pq_code10(double nit);

// ------------------------------------------------------------------- SEI

// Werte fuer mastering_display_colour_volume (H.265 D.2.28) und
// content_light_level_info (D.2.35). Primaerfarben in der Reihenfolge
// Gruen, Blau, Rot (wie ST 2086 und x265), in 0,00002; Leuchtdichte in
// 0,0001 nit.
typedef struct {
    uint16_t x[3], y[3];
    uint16_t weiss_x, weiss_y;
    uint32_t max_lum, min_lum;
    uint16_t max_cll, max_fall;
} qc_hdr_sei_werte;

// Primaerfarben und Weisspunkt D65: bt2020 = 0 fuer Display P3, 1 fuer BT.2020.
void qc_hdr_sei_primaer(qc_hdr_sei_werte *w, int bt2020);

// Praefix-SEI-NAL (Typ 39) mit 137 und 144, mit Emulationsschutz, ohne
// Startcode und ohne Laengenpraefix. Rueckgabe: Laenge, 0 wenn platz nicht
// reicht (64 Byte reichen immer).
size_t qc_hdr_sei_bauen(const qc_hdr_sei_werte *w, uint8_t *out, size_t platz);

#endif
