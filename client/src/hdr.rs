// HDR10 (BT.2020, PQ): die eine Mathematik fuer beide Clients und beide Hosts.
//
// Reines Rust, alle Plattformen, und die Referenz fuer alle Tests. Dasselbe
// rechnen die Shader (MSL in anzeige_mac.rs, HLSL fuer anzeige.rs und den
// Windows-Host) und der C-Spiegel des Mac-Hosts (host/hdr.c) - mit denselben
// Konstanten; die gemeinsamen Pruefvektoren fuer Rust und C stehen in
// hdr_vektoren.txt.
//
// Inhalt:
// - Codes nach H.273 und die Farbe eines Bildes (Farbe).
// - Strominfo Fassung 1 (MSG_INFO, 24 Byte) und IN_ANZEIGE (Typ 71, 14 Byte)
//   als Bytes (InfoV1, Anzeige).
// - hdr_entscheiden: ob der Host HDR sendet, und wenn nicht, warum.
// - PQ (SMPTE ST 2084), sRGB, die Farbraum-Matrizen, Y'CbCr BT.2020-NCL.
// - Die Tonwertabbildung: relativ zum SDR-Weiss (Entscheidung E3), farbtreu
//   auf max(R, G, B); auf SDR-Zielen farbtreues Abschneiden (E4).
// - zeile_rgb_pq: der CPU-Weg (softbuffer) fuer PQ-Bilder, mit Tabellen.
// - sdr_aus_scrgb: ein Bildpunkt des HDR-Desktops (scRGB) als SDR fuer den
//   Windows-Host.
//
// Strominfo Fassung 1 (Host -> Client, Typ 1). Bytes 0-7 wie bisher: u16
// Breite, u16 Hoehe, u16 fps, u8 Codec (1 HEVC, 2 H.264), u8 Format (1 = 4:4:4
// 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit, 4 = 4:2:0 10 Bit). Dahinter:
//   [8] u8 Fassung = 1   [9] u8 Transfer (1 SDR, 16 PQ, 18 HLG reserviert)
//   [10] u8 Primaerfarben (1 BT.709, 9 BT.2020)   [11] u8 Matrix (1 BT.709,
//   9 BT.2020-NCL)   [12] u8 voller Bereich = 1   [13] u8 HDR-Grund (GRUND_*)
//   [14..15] u16 SDR-Weiss des Hosts in nit   [16..17] u16 Mastering max nit
//   [18..19] u16 Mastering min in 0,0001 nit   [20..21] u16 MaxCLL
//   [22..23] u16 MaxFALL - alle u16 little endian, 0 = unbekannt.
// Ein Host vor 0.2.0 sendet nur die acht Byte. Eine andere Fassung als 1
// liest der Client wie acht Byte: Farbe aus dem VUI, Metadaten nach Vorgabe
// (SDR-Weiss 203 nit, Spitze 1000 nit).
//
// IN_ANZEIGE (Client -> Host, Typ 71), 14 Byte, eine Zustandsnachricht:
//   [0] u8 Fassung = 1   [1] u8 Flags (Bit 0 Bildschirm HDR-faehig, Bit 1
//   Client kann HDR darstellen)   [2] u8 Wunsch (0 Automatisch, 1 Aus,
//   2 Immer)   [3] u8 frei = 0   [4..5] u16 SDR-Weiss nit (0 = unbekannt)
//   [6..7] u16 Spitze nit   [8..9] u16 Vollbild-Spitze nit
//   [10..11] u16 Kopfraum potentiell x100   [12..13] u16 Kopfraum aktuell x100.
//
// SWITCH (Typ 7) p[6]: Transfer nach H.273 (Hosts vor 0.2.0 schicken 0 = SDR),
// nur fuers Protokoll.
//
// Arbeitsraum der Abbildung: lineares Licht relativ zum SDR-Weiss der Quelle
// (1.0 = SDR-Weiss des Hosts, W_h). Hs = Spitze der Quelle / W_h, Hd = Kopfraum
// des Ziels (SDR-Schirm 1.0).
//
// Bis alle Schritte des HDR-Plans stehen, benutzt nicht jede Plattform jedes
// Stueck (der Mac-Client etwa kodiert keine Strominfo); die Tests benutzen alles.

#![allow(dead_code)]

use std::sync::OnceLock;

// ------------------------------------------------------ Codes nach H.273

/// Transfer BT.709 (SDR).
pub const TRANSFER_SDR: u8 = 1;
/// Transfer SMPTE ST 2084 (PQ, HDR10).
pub const TRANSFER_PQ: u8 = 16;
/// Transfer ARIB STD-B67 (HLG) - reserviert, nicht in 0.2.0.
pub const TRANSFER_HLG: u8 = 18;
pub const PRIMAER_709: u8 = 1;
pub const PRIMAER_2020: u8 = 9;
pub const MATRIX_709: u8 = 1;
pub const MATRIX_2020_NCL: u8 = 9;

// ------------------------------------------------------------ Protokoll

pub const INFO_FASSUNG: u8 = 1;
/// Laenge der Strominfo Fassung 1.
pub const INFO_LAENGE: usize = 24;
/// Laenge der Strominfo vor 0.2.0 (und der alte Teil von Fassung 1).
pub const INFO_LAENGE_ALT: usize = 8;
pub const ANZEIGE_FASSUNG: u8 = 1;
pub const ANZEIGE_LAENGE: usize = 14;

/// HDR-Grund (Byte 13 der Strominfo): HDR laeuft.
pub const GRUND_AKTIV: u8 = 0;
/// Client-Bildschirm SDR (bei Wunsch Automatisch) oder Wunsch Aus.
pub const GRUND_CLIENT_SDR: u8 = 1;
/// Der Codec ist nicht HEVC 10 Bit.
pub const GRUND_CODEC: u8 = 2;
/// Der aufgenommene Bildschirm des Hosts ist SDR.
pub const GRUND_HOST_SCHIRM_SDR: u8 = 3;
/// Betriebssystem oder Encoder des Hosts koennen kein HDR10.
pub const GRUND_HOST_KANN_NICHT: u8 = 4;
/// Der Client kann HDR nicht darstellen (Bit 1 in IN_ANZEIGE ist 0).
pub const GRUND_CLIENT_OHNE_DARSTELLUNG: u8 = 5;
/// Der Wechsel nach HDR schlug fehl; der Host blieb beim Kandidaten in SDR.
pub const GRUND_WECHSEL_GESCHEITERT: u8 = 6;
/// Der Client hat kein IN_ANZEIGE gesendet (etwa ein Client vor 0.2.0).
pub const GRUND_KEIN_IN_ANZEIGE: u8 = 7;

/// Wunsch in IN_ANZEIGE: HDR, wenn beide Seiten koennen.
pub const WUNSCH_AUTOMATISCH: u8 = 0;
/// Immer SDR.
pub const WUNSCH_AUS: u8 = 1;
/// HDR auch auf einem SDR-Schirm (der Client bildet ab).
pub const WUNSCH_IMMER: u8 = 2;

/// Flag Bit 0 in IN_ANZEIGE: der Bildschirm des Clients ist HDR-faehig.
pub const ANZEIGE_SCHIRM_HDR: u8 = 1;
/// Flag Bit 1: der Client kann HDR darstellen (GPU-Weg mit 10-Bit-Decode).
pub const ANZEIGE_DARSTELLUNG: u8 = 2;

/// SDR-Weiss einer Quelle ohne Angabe (BT.2408).
pub const SDR_WEISS_VORGABE_NIT: f32 = 203.0;
/// Spitze einer Quelle ohne MaxCLL und Mastering-Angabe.
pub const SPITZE_VORGABE_NIT: f32 = 1000.0;

// ---------------------------------------------------------------- Farbe

/// Die Farbe eines Bildes oder Stroms nach H.273.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Farbe {
    pub transfer: u8,
    pub primaer: u8,
    pub matrix: u8,
    pub voll: bool,
}

impl Farbe {
    /// SDR, wie jeder Strom vor 0.2.0: BT.709, voller Bereich.
    pub const SDR: Farbe = Farbe { transfer: TRANSFER_SDR, primaer: PRIMAER_709, matrix: MATRIX_709, voll: true };
    /// HDR10 auf der Leitung: PQ, BT.2020, BT.2020-NCL, voller Bereich.
    pub const PQ: Farbe = Farbe { transfer: TRANSFER_PQ, primaer: PRIMAER_2020, matrix: MATRIX_2020_NCL, voll: true };

    /// Aus dem VUI eines decodierten Bildes. Matrix und Bereich zaehlen nur bei
    /// PQ; alles andere ist SDR und bleibt fest BT.709 voll - der bgra-Weg des
    /// Windows-Hosts schreibt ein irrefuehrendes VUI (BT.601 begrenzt).
    pub fn aus_vui(transfer: u8, primaer: u8, matrix: u8, voll: bool) -> Farbe {
        if transfer == TRANSFER_PQ {
            Farbe { transfer, primaer, matrix, voll }
        } else {
            Farbe::SDR
        }
    }

    pub fn ist_pq(&self) -> bool {
        self.transfer == TRANSFER_PQ
    }

    /// Fuers Protokoll, etwa "PQ/BT.2020" oder "SDR/BT.709".
    pub fn text(&self) -> String {
        let t = match self.transfer {
            0 | TRANSFER_SDR => "SDR".to_string(),
            TRANSFER_PQ => "PQ".to_string(),
            TRANSFER_HLG => "HLG".to_string(),
            t => format!("Transfer {t}"),
        };
        let p = match self.primaer {
            PRIMAER_709 => "BT.709".to_string(),
            PRIMAER_2020 => "BT.2020".to_string(),
            p => format!("Primaerfarben {p}"),
        };
        format!("{t}/{p}{}", if self.voll { "" } else { " begrenzt" })
    }
}

/// Transfer aus SWITCH p[6] fuers Protokoll (0 von Hosts vor 0.2.0 = SDR).
pub fn transfer_text(t: u8) -> String {
    match t {
        0 | TRANSFER_SDR => "SDR".into(),
        TRANSFER_PQ => "PQ".into(),
        TRANSFER_HLG => "HLG".into(),
        t => format!("Transfer {t}"),
    }
}

// ------------------------------------------------- Strominfo Fassung 1

/// Bytes 8-23 der Strominfo Fassung 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InfoV1 {
    pub farbe: Farbe,
    pub grund: u8,
    pub sdr_weiss_nit: u16,
    pub master_max_nit: u16,
    /// In 0,0001 nit.
    pub master_min_zehntausendstel: u16,
    pub max_cll: u16,
    pub max_fall: u16,
}

impl InfoV1 {
    /// SDR mit diesem Grund, ohne Metadaten.
    pub fn sdr(grund: u8) -> InfoV1 {
        InfoV1 { farbe: Farbe::SDR, grund, sdr_weiss_nit: 0, master_max_nit: 0, master_min_zehntausendstel: 0, max_cll: 0, max_fall: 0 }
    }

    /// Die 16 Bytes hinter den alten acht, beginnend mit der Fassung.
    pub fn kodieren(&self) -> [u8; INFO_LAENGE - INFO_LAENGE_ALT] {
        let mut p = [0u8; INFO_LAENGE - INFO_LAENGE_ALT];
        p[0] = INFO_FASSUNG;
        p[1] = self.farbe.transfer;
        p[2] = self.farbe.primaer;
        p[3] = self.farbe.matrix;
        p[4] = self.farbe.voll as u8;
        p[5] = self.grund;
        for (i, v) in [self.sdr_weiss_nit, self.master_max_nit, self.master_min_zehntausendstel, self.max_cll, self.max_fall]
            .into_iter()
            .enumerate()
        {
            p[6 + 2 * i..8 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        p
    }

    /// Aus der ganzen Strominfo (ab Byte 0). None bei acht Byte (Host vor
    /// 0.2.0), zu kurz oder einer anderen Fassung als 1; Bytes hinter den 24
    /// werden uebergangen.
    pub fn lesen(info: &[u8]) -> Option<InfoV1> {
        if info.len() < INFO_LAENGE || info[8] != INFO_FASSUNG {
            return None;
        }
        let u16_bei = |i: usize| u16::from_le_bytes([info[i], info[i + 1]]);
        Some(InfoV1 {
            farbe: Farbe { transfer: info[9], primaer: info[10], matrix: info[11], voll: info[12] != 0 },
            grund: info[13],
            sdr_weiss_nit: u16_bei(14),
            master_max_nit: u16_bei(16),
            master_min_zehntausendstel: u16_bei(18),
            max_cll: u16_bei(20),
            max_fall: u16_bei(22),
        })
    }

    /// SDR-Weiss der Quelle (W_h) in nit, ohne Angabe 203.
    pub fn quell_weiss_nit(&self) -> f32 {
        if self.sdr_weiss_nit == 0 { SDR_WEISS_VORGABE_NIT } else { self.sdr_weiss_nit as f32 }
    }

    /// Spitze der Quelle in nit: MaxCLL, sonst Mastering max, sonst 1000.
    pub fn quell_spitze_nit(&self) -> f32 {
        if self.max_cll != 0 {
            self.max_cll as f32
        } else if self.master_max_nit != 0 {
            self.master_max_nit as f32
        } else {
            SPITZE_VORGABE_NIT
        }
    }

    /// Fuers Protokoll: "SDR/BT.709, Grund 7 (kein IN_ANZEIGE vom Client)"
    /// oder "PQ/BT.2020, Weiss 203 nit, Mastering 0.0050-1000 nit, MaxCLL 0, MaxFALL 0".
    pub fn text(&self) -> String {
        if self.farbe.ist_pq() {
            format!(
                "{}, Weiss {} nit, Mastering {:.4}-{} nit, MaxCLL {}, MaxFALL {}",
                self.farbe.text(),
                self.sdr_weiss_nit,
                self.master_min_zehntausendstel as f32 / 10000.0,
                self.master_max_nit,
                self.max_cll,
                self.max_fall
            )
        } else {
            format!("{}, Grund {} ({})", self.farbe.text(), self.grund, grund_text(self.grund))
        }
    }
}

// ----------------------------------------------------------- IN_ANZEIGE

/// Die Lage der Anzeige des Clients (IN_ANZEIGE).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Anzeige {
    /// ANZEIGE_SCHIRM_HDR | ANZEIGE_DARSTELLUNG; weitere Bits werden uebergangen.
    pub flags: u8,
    /// WUNSCH_*; ein unbekannter Wert gilt wie WUNSCH_AUS.
    pub wunsch: u8,
    pub sdr_weiss_nit: u16,
    pub spitze_nit: u16,
    pub vollbild_spitze_nit: u16,
    /// Kopfraum x100 (nur Diagnose in 0.2.0).
    pub kopfraum_potentiell: u16,
    pub kopfraum_aktuell: u16,
}

impl Anzeige {
    pub fn schirm_hdr(&self) -> bool {
        self.flags & ANZEIGE_SCHIRM_HDR != 0
    }

    pub fn darstellung(&self) -> bool {
        self.flags & ANZEIGE_DARSTELLUNG != 0
    }

    pub fn kodieren(&self) -> [u8; ANZEIGE_LAENGE] {
        let mut p = [0u8; ANZEIGE_LAENGE];
        p[0] = ANZEIGE_FASSUNG;
        p[1] = self.flags;
        p[2] = self.wunsch;
        for (i, v) in [self.sdr_weiss_nit, self.spitze_nit, self.vollbild_spitze_nit, self.kopfraum_potentiell, self.kopfraum_aktuell]
            .into_iter()
            .enumerate()
        {
            p[4 + 2 * i..6 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        p
    }

    /// None bei weniger als 14 Byte oder einer anderen Fassung als 1 - dann
    /// gilt es wie kein IN_ANZEIGE. Bytes dahinter werden uebergangen.
    pub fn lesen(p: &[u8]) -> Option<Anzeige> {
        if p.len() < ANZEIGE_LAENGE || p[0] != ANZEIGE_FASSUNG {
            return None;
        }
        let u16_bei = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        Some(Anzeige {
            flags: p[1],
            wunsch: p[2],
            sdr_weiss_nit: u16_bei(4),
            spitze_nit: u16_bei(6),
            vollbild_spitze_nit: u16_bei(8),
            kopfraum_potentiell: u16_bei(10),
            kopfraum_aktuell: u16_bei(12),
        })
    }
}

// ---------------------------------------------------------- Entscheidung

/// Kann dieser Kandidat HDR10 tragen? Nur HEVC 10 Bit: 0 (4:4:4) und 2
/// (4:2:0) - die Koennenslisten beider Hosts haben dieselbe Reihenfolge.
pub fn codec_kann_hdr(idx: u8) -> bool {
    idx == 0 || idx == 2
}

/// Die Entscheidung des Hosts: GRUND_AKTIV = HDR senden, sonst der Grund fuer
/// SDR. Der Host entscheidet allein; der Client meldet nur Schirm und Wunsch.
///
/// quelle_hdr: der aufgenommene Bildschirm ist HDR (Windows: Desktop in
/// G2084; Mac: EDR-Kopfraum > 1). host_kann: Betriebssystem und Encoder
/// koennen HDR10 mit diesem Kandidaten. anzeige: das zuletzt gelesene
/// IN_ANZEIGE dieser Sitzung.
///
/// HDR genau dann, wenn alles zusammenkommt: IN_ANZEIGE da, Wunsch Immer oder
/// Automatisch mit HDR-Schirm, Kandidat 0 oder 2, Host kann, Quelle HDR,
/// Client kann darstellen. Fehlt mehreres, gilt der erste Grund in dieser
/// Reihenfolge: kein IN_ANZEIGE (7), Wunsch Aus oder unbekannt (1), Codec (2),
/// Host kann nicht (4), Host-Schirm SDR (3), Client ohne Darstellung (5),
/// Client-Schirm SDR bei Automatisch (1). Der Codec kommt vor "Host kann",
/// weil host_kann je Kandidat gilt und fuer 8 Bit immer nein ist. Pruefvektoren:
/// die Zeilen "entscheiden" in hdr_vektoren.txt (auch fuer host/hdr.c).
pub fn hdr_entscheiden(quelle_hdr: bool, host_kann: bool, idx: u8, anzeige: Option<&Anzeige>) -> u8 {
    let Some(a) = anzeige else { return GRUND_KEIN_IN_ANZEIGE };
    // Ein unbekannter Wunsch gilt wie Aus: Unsicherheit ergibt SDR.
    if a.wunsch != WUNSCH_AUTOMATISCH && a.wunsch != WUNSCH_IMMER {
        return GRUND_CLIENT_SDR;
    }
    if !codec_kann_hdr(idx) {
        return GRUND_CODEC;
    }
    if !host_kann {
        return GRUND_HOST_KANN_NICHT;
    }
    if !quelle_hdr {
        return GRUND_HOST_SCHIRM_SDR;
    }
    if !a.darstellung() {
        return GRUND_CLIENT_OHNE_DARSTELLUNG;
    }
    if a.wunsch == WUNSCH_AUTOMATISCH && !a.schirm_hdr() {
        return GRUND_CLIENT_SDR;
    }
    GRUND_AKTIV
}

/// Der Grund als deutscher Text fuers Protokoll (wie qc_hdr_grund_text).
pub fn grund_text(grund: u8) -> &'static str {
    match grund {
        GRUND_AKTIV => "HDR aktiv",
        GRUND_CLIENT_SDR => "Client-Bildschirm SDR oder HDR aus",
        GRUND_CODEC => "Codec nicht HEVC 10 Bit",
        GRUND_HOST_SCHIRM_SDR => "Bildschirm des Hosts SDR",
        GRUND_HOST_KANN_NICHT => "Host kann kein HDR (System oder Encoder)",
        GRUND_CLIENT_OHNE_DARSTELLUNG => "Client kann HDR nicht darstellen",
        GRUND_WECHSEL_GESCHEITERT => "Wechsel nach HDR gescheitert",
        GRUND_KEIN_IN_ANZEIGE => "kein IN_ANZEIGE vom Client",
        _ => "unbekannter Grund",
    }
}

// -------------------------------------------------------------------- PQ

/// SMPTE ST 2084: m1 = 2610/16384.
pub const PQ_M1: f64 = 0.1593017578125;
/// m2 = 2523/4096 * 128.
pub const PQ_M2: f64 = 78.84375;
/// c1 = 3424/4096.
pub const PQ_C1: f64 = 0.8359375;
/// c2 = 2413/4096 * 32.
pub const PQ_C2: f64 = 18.8515625;
/// c3 = 2392/4096 * 32.
pub const PQ_C3: f64 = 18.6875;
/// Signal 1.0 = 10000 nit.
pub const PQ_SPITZE_NIT: f64 = 10000.0;

/// PQ-Signal 0..1 -> Pegel in nit (EOTF). Ausserhalb 0..1 geklemmt.
pub fn pq_eotf(e: f64) -> f64 {
    let e = if e > 0.0 { e.min(1.0) } else { 0.0 };
    let p = e.powf(1.0 / PQ_M2);
    let z = (p - PQ_C1).max(0.0);
    (z / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1) * PQ_SPITZE_NIT
}

/// Pegel in nit -> PQ-Signal 0..1 (inverse EOTF). Ausserhalb 0..10000 geklemmt.
pub fn pq_oetf(nit: f64) -> f64 {
    let y = nit / PQ_SPITZE_NIT;
    let y = if y > 0.0 { y.min(1.0) } else { 0.0 };
    let ym = y.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * ym) / (1.0 + PQ_C3 * ym)).powf(PQ_M2)
}

/// PQ-Code (10 Bit, voller Bereich) eines Pegels, gerundet.
pub fn pq_code10(nit: f64) -> u16 {
    (pq_oetf(nit) * 1023.0).round() as u16
}

// ------------------------------------------------------------------ sRGB

/// sRGB-Signal 0..1 -> lineares Licht (stueckweise, IEC 61966-2-1).
pub fn srgb_eotf(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Lineares Licht -> sRGB-Signal (stueckweise).
pub fn srgb_oetf(l: f64) -> f64 {
    if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

/// Lineares Licht 0..1 -> sRGB 8 Bit, gerundet (ausserhalb geklemmt).
pub fn srgb8(l: f32) -> u8 {
    let l = if l > 0.0 { l.min(1.0) } else { 0.0 };
    (srgb_oetf(l as f64) * 255.0).round() as u8
}

// ------------------------------------------------------------- Matrizen

/// Zeilenweise: aus = m * ein.
pub type Matrix = [[f32; 3]; 3];

/// Lineares RGB BT.709 -> BT.2020 (BT.2087), beide D65.
pub const M_709_NACH_2020: Matrix = [
    [0.627_403_9, 0.329_283_04, 0.043_313_066],
    [0.069_097_29, 0.919_540_4, 0.011_362_316],
    [0.016_391_439, 0.088_013_31, 0.895_595_25],
];
/// BT.2020 -> BT.709 (die Umkehrung).
pub const M_2020_NACH_709: Matrix = [
    [1.660_491, -0.587_641_14, -0.072_849_863],
    [-0.124_550_48, 1.132_899_9, -0.008_349_423],
    [-0.018_150_763, -0.100_578_9, 1.118_729_7],
];
/// BT.2020 -> Display P3 (D65), fuer die EDR-Schicht des Mac-Clients.
/// (Die Zeilen 0.753833 0.198597 0.047570 ... im Plan, Abschnitt 4.5, sind die
/// Gegenrichtung P3 -> 2020.)
pub const M_2020_NACH_P3: Matrix = [
    [1.343_578_3, -0.282_179_67, -0.061_398_58],
    [-0.065_297_45, 1.075_787_9, -0.010_490_463],
    [0.002_821_787, -0.019_598_495, 1.016_776_7],
];
/// Display P3 -> BT.2020 (was VideoToolbox mit der P3-PQ-Aufnahme rechnet).
pub const M_P3_NACH_2020: Matrix = [
    [0.753_833_03, 0.198_597_37, 0.047_569_6],
    [0.045_743_85, 0.941_777_2, 0.012_478_931],
    [-0.001_210_34, 0.017_601_717, 0.983_608_6],
];
/// BT.709 -> Display P3, fuer Oberflaeche und Zeiger auf der EDR-Schicht.
pub const M_709_NACH_P3: Matrix = [
    [0.822_461_97, 0.177_538_03, 0.0],
    [0.033_194_2, 0.966_805_8, 0.0],
    [0.017_082_631, 0.072_397_44, 0.910_519_9],
];

pub fn mal(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// ------------------------------------------------ Y'CbCr BT.2020 (NCL)

/// Luma-Gewichte BT.2020: Kr, Kb (Kg = 1 - Kr - Kb).
pub const KR_2020: f32 = 0.2627;
pub const KB_2020: f32 = 0.0593;
/// R' = Y' + 2 (1 - Kr) Cr.
pub const CR_R_2020: f32 = 1.4746;
/// G' = Y' - 2 Kb (1 - Kb) / Kg Cb - 2 Kr (1 - Kr) / Kg Cr.
pub const CB_G_2020: f32 = 0.164_553_13;
pub const CR_G_2020: f32 = 0.571_353_1;
/// B' = Y' + 2 (1 - Kb) Cb.
pub const CB_B_2020: f32 = 1.8814;

/// Y' 0..1, Cb/Cr -0.5..0.5 -> R'G'B' (nicht geklemmt).
pub fn ycbcr_nach_rgb_2020(y: f32, cb: f32, cr: f32) -> [f32; 3] {
    [y + CR_R_2020 * cr, y - CB_G_2020 * cb - CR_G_2020 * cr, y + CB_B_2020 * cb]
}

/// R'G'B' -> Y' 0..1, Cb/Cr -0.5..0.5 (fuer den Host und die Tests).
pub fn rgb_nach_ycbcr_2020(rgb: [f32; 3]) -> [f32; 3] {
    let y = KR_2020 * rgb[0] + (1.0 - KR_2020 - KB_2020) * rgb[1] + KB_2020 * rgb[2];
    [y, (rgb[2] - y) / CB_B_2020, (rgb[0] - y) / CR_R_2020]
}

// ----------------------------------------------------- Tonwertabbildung

/// Das Knie: bis zum SDR-Weiss (rel 1.0) bleibt alles unberuehrt.
pub const KNIE: f32 = 1.0;

/// Wie eine Quelle auf ein Ziel abgebildet wird.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Abbildung {
    /// SDR-Weiss der Quelle in nit (W_h).
    pub weiss_nit: f32,
    /// Spitze der Quelle relativ zu W_h (Hs).
    pub hs: f32,
    /// Kopfraum des Ziels (Hd): 1.0 = SDR-Schirm.
    pub hd: f32,
}

impl Abbildung {
    /// Aus der Strominfo (None: Vorgaben 203 / 1000 nit) auf ein Ziel mit
    /// diesem Kopfraum.
    pub fn neu(info: Option<&InfoV1>, hd: f32) -> Abbildung {
        let w = info.map_or(SDR_WEISS_VORGABE_NIT, InfoV1::quell_weiss_nit);
        let spitze = info.map_or(SPITZE_VORGABE_NIT, InfoV1::quell_spitze_nit);
        Abbildung { weiss_nit: w, hs: spitze / w, hd }
    }

    /// Auf einen SDR-Schirm (Hd = 1).
    pub fn sdr(info: Option<&InfoV1>) -> Abbildung {
        Abbildung::neu(info, 1.0)
    }
}

/// Die Abbildung eines Wertes m = max(R, G, B) (relativ zum SDR-Weiss).
///
/// Hs <= Hd: Identitaet. Sonst Knie k = 1, e = m - k, S = Hs - k, D = Hd - k;
/// D <= 0 (SDR-Ziel): m' = min(m, Hd), farbtreues Abschneiden; sonst fuer
/// m > k: m' = k + e (1 + e D / S^2) / (1 + e / D) - stetig mit stetiger
/// Ableitung am Knie, monoton, bildet Hs genau auf Hd ab. Werte ueber Hs (eine
/// zu kleine MaxCLL) und ueber Hd werden in jedem Fall bei Hd abgeschnitten.
pub fn abbilden(m: f32, hs: f32, hd: f32) -> f32 {
    if hs <= hd {
        return m.min(hd);
    }
    let d = hd - KNIE;
    if d <= 0.0 {
        return m.min(hd);
    }
    if m <= KNIE {
        return m;
    }
    let e = m - KNIE;
    let s = hs - KNIE;
    (KNIE + e * (1.0 + e * d / (s * s)) / (1.0 + e / d)).min(hd)
}

/// Farbtreu: RGB *= m' / m mit m = max(R, G, B). Negative Anteile (ausserhalb
/// des Farbraums) bleiben fuer das Klemmen danach.
pub fn rgb_abbilden(rgb: [f32; 3], hs: f32, hd: f32) -> [f32; 3] {
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if !(m > 0.0) {
        return rgb;
    }
    let f = abbilden(m, hs, hd) / m;
    [rgb[0] * f, rgb[1] * f, rgb[2] * f]
}

/// Lineares Licht BT.2020 relativ zum SDR-Weiss auf einem SDR-Ziel:
/// Abbildung mit Hd = 1, BT.2020 -> BT.709, klemmen, sRGB, 8 Bit gerundet.
/// Ein SDR-Inhalt, den der Host in PQ verpackt hat, kommt dabei unveraendert
/// heraus (Abbildung unterhalb des Knies = Identitaet).
pub fn rel2020_nach_srgb8(rel: [f32; 3], hs: f32) -> [u8; 3] {
    let a = rgb_abbilden(rel, hs, 1.0);
    let s = mal(&M_2020_NACH_709, a);
    [srgb8(s[0]), srgb8(s[1]), srgb8(s[2])]
}

/// Ein PQ-Bildpunkt (Y' 0..1, Cb/Cr -0.5..0.5, BT.2020-NCL) auf einem SDR-Ziel -
/// die Rechnung ohne Tabellen, die Referenz fuer zeile_rgb_pq und die Shader.
pub fn pq_nach_srgb8(y: f32, cb: f32, cr: f32, ab: &Abbildung) -> [u8; 3] {
    let e = ycbcr_nach_rgb_2020(y, cb, cr);
    let rel = e.map(|c| (pq_eotf(c as f64) / ab.weiss_nit as f64) as f32);
    let a = rgb_abbilden(rel, ab.hs, ab.hd.min(1.0));
    let s = mal(&M_2020_NACH_709, a);
    [srgb8(s[0]), srgb8(s[1]), srgb8(s[2])]
}

/// Ein Bildpunkt des HDR-Desktops (scRGB: linear, BT.709-Primaerfarben,
/// 1.0 = 80 nit) als SDR-sRGB 8 Bit - der Windows-Host mit HDR-Desktop, wenn
/// er SDR sendet. weiss_scrgb = SDRWhiteLevel / 1000 (der scRGB-Wert des
/// SDR-Weiss; 1.0 = 80 nit). rel = scRGB / weiss_scrgb, Abbildung mit Hd = 1
/// (farbtreu beim SDR-Weiss abgeschnitten), klemmen, sRGB, gerundet.
pub fn sdr_aus_scrgb(rgb: [f32; 3], weiss_scrgb: f32) -> [u8; 3] {
    let inv = 1.0 / weiss_scrgb;
    let rel = [rgb[0] * inv, rgb[1] * inv, rgb[2] * inv];
    let a = rgb_abbilden(rel, f32::INFINITY, 1.0);
    [srgb8(a[0]), srgb8(a[1]), srgb8(a[2])]
}

// ------------------------------------------------ CPU-Weg mit Tabellen

/// Stuetzstellen der PQ-Tabelle (Signal 0..1 in so vielen Schritten, dazwischen
/// linear): der Fehler bleibt weit unter einem halben 10-Bit-Code.
const PQ_TABELLE_SCHRITTE: usize = 4096;

struct Tabellen {
    /// Pegel in nit an den Stuetzstellen.
    pq_nit: Vec<f32>,
    /// sRGB: lineares Licht an den Grenzen zwischen zwei 8-Bit-Werten
    /// ((k + 0,5) / 255 zurueckgerechnet) - wer darunter liegt, rundet ab.
    srgb_grenzen: [f32; 255],
}

impl Tabellen {
    #[inline(always)]
    fn pq_nit(&self, e: f32) -> f32 {
        let f = if e > 0.0 { e.min(1.0) } else { 0.0 } * PQ_TABELLE_SCHRITTE as f32;
        let i = (f as usize).min(PQ_TABELLE_SCHRITTE - 1);
        let t = f - i as f32;
        self.pq_nit[i] + (self.pq_nit[i + 1] - self.pq_nit[i]) * t
    }

    /// Lineares Licht -> sRGB 8 Bit (wie srgb8, aber ohne pow).
    #[inline(always)]
    fn srgb8(&self, l: f32) -> u32 {
        self.srgb_grenzen.partition_point(|&g| g <= l) as u32
    }
}

fn tabellen() -> &'static Tabellen {
    static T: OnceLock<Tabellen> = OnceLock::new();
    T.get_or_init(|| {
        let pq_nit = (0..=PQ_TABELLE_SCHRITTE).map(|i| pq_eotf(i as f64 / PQ_TABELLE_SCHRITTE as f64) as f32).collect();
        let mut srgb_grenzen = [0f32; 255];
        for (k, g) in srgb_grenzen.iter_mut().enumerate() {
            *g = srgb_eotf((k as f64 + 0.5) / 255.0) as f32;
        }
        Tabellen { pq_nit, srgb_grenzen }
    })
}

/// Ein Wert der Ebene als 10-Bit-Code: 8 Bit (nur der Vollstaendigkeit
/// halber, PQ in 8 Bit gibt es auf der Leitung nicht) << 2, 10 Bit LE in den
/// unteren Bits, 16 Bit oben buendig >> 6 - wie `wert` in main.rs.
#[inline(always)]
fn code10<const BITS: u8>(p: &[u8], i: usize) -> i32 {
    match BITS {
        8 => (p[i] as i32) << 2,
        10 => (u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) & 0x3ff) as i32,
        _ => (u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) >> 6) as i32,
    }
}

/// Drei 10-Bit-Codes -> Y' 0..1, Cb/Cr um 0 (H.273: voll (D - 512) / 1023,
/// begrenzt Y (D - 64) / 876, C (D - 512) / 896).
#[inline(always)]
pub fn normieren10(y: i32, cb: i32, cr: i32, begrenzt: bool) -> (f32, f32, f32) {
    if begrenzt {
        ((y - 64) as f32 / 876.0, (cb - 512) as f32 / 896.0, (cr - 512) as f32 / 896.0)
    } else {
        (y as f32 / 1023.0, (cb - 512) as f32 / 1023.0, (cr - 512) as f32 / 1023.0)
    }
}

/// Eine Zeile eines PQ-Bildes (BT.2020-NCL) nach 0x00RRGGBB fuer einen
/// SDR-Schirm - der CPU-Weg (softbuffer), wenn die Karte nicht zeichnet. SUB,
/// BITS, PAAR und BEGRENZT wie bei `zeile_rgb` in main.rs. Dieselbe Rechnung
/// wie pq_nach_srgb8, nur PQ und sRGB ueber Tabellen; Abweichung hoechstens
/// ein Wert.
#[inline(always)]
pub fn zeile_rgb_pq<const SUB: bool, const BITS: u8, const PAAR: bool, const BEGRENZT: bool>(
    out: &mut [u32],
    yr: &[u8],
    ur: &[u8],
    vr: &[u8],
    ab: &Abbildung,
) {
    let t = tabellen();
    let inv_w = 1.0 / ab.weiss_nit;
    let hd = ab.hd.min(1.0);
    for (x, o) in out.iter_mut().enumerate() {
        let cx = if SUB { x >> 1 } else { x };
        let ci = if PAAR { cx * 2 } else { cx };
        let (y, cb, cr) = normieren10(code10::<BITS>(yr, x), code10::<BITS>(ur, ci), code10::<BITS>(vr, ci), BEGRENZT);
        let e = ycbcr_nach_rgb_2020(y, cb, cr);
        let rel = [t.pq_nit(e[0]) * inv_w, t.pq_nit(e[1]) * inv_w, t.pq_nit(e[2]) * inv_w];
        let s = mal(&M_2020_NACH_709, rgb_abbilden(rel, ab.hs, hd));
        *o = (t.srgb8(s[0]) << 16) | (t.srgb8(s[1]) << 8) | t.srgb8(s[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VEKTOREN: &str = include_str!("hdr_vektoren.txt");

    /// Die Zeilen einer Art aus hdr_vektoren.txt, in Felder zerlegt.
    fn zeilen(art: &str) -> Vec<Vec<&'static str>> {
        let v: Vec<Vec<&str>> = VEKTOREN
            .lines()
            .map(|z| z.split('#').next().unwrap_or("").split_whitespace().collect::<Vec<_>>())
            .filter(|f| f.first() == Some(&art))
            .map(|f| f[1..].to_vec())
            .collect();
        assert!(!v.is_empty(), "keine Zeilen \"{art}\" in hdr_vektoren.txt");
        v
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn zahl<T: std::str::FromStr>(s: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        s.parse().unwrap()
    }

    /// "-" oder "flags/wunsch" aus einer Zeile "entscheiden".
    fn anzeige_aus(s: &str) -> Option<Anzeige> {
        let (f, w) = s.split_once('/')?;
        let a = Anzeige { flags: zahl(f), wunsch: zahl(w), sdr_weiss_nit: 240, spitze_nit: 1000, ..Anzeige::default() };
        // Ueber die Bytes, wie auf der Leitung.
        Anzeige::lesen(&a.kodieren())
    }

    fn info_aus(f: &[&str]) -> InfoV1 {
        InfoV1 {
            farbe: Farbe { transfer: zahl(f[0]), primaer: zahl(f[1]), matrix: zahl(f[2]), voll: f[3] == "1" },
            grund: zahl(f[4]),
            sdr_weiss_nit: zahl(f[5]),
            master_max_nit: zahl(f[6]),
            master_min_zehntausendstel: zahl(f[7]),
            max_cll: zahl(f[8]),
            max_fall: zahl(f[9]),
        }
    }

    fn anzeige_voll_aus(f: &[&str]) -> Anzeige {
        Anzeige {
            flags: zahl(f[0]),
            wunsch: zahl(f[1]),
            sdr_weiss_nit: zahl(f[2]),
            spitze_nit: zahl(f[3]),
            vollbild_spitze_nit: zahl(f[4]),
            kopfraum_potentiell: zahl(f[5]),
            kopfraum_aktuell: zahl(f[6]),
        }
    }

    /// PQ: die Pegel der Tabelle (80/100/203/240/1000/10000 nit =
    /// 497/520/594/612/769/1023 und mehr), und jeder 10-Bit-Code kommt ueber
    /// Pegel und zurueck auf hoechstens 0,5 Code genau heraus - exakt gerechnet
    /// wie ueber die Tabelle des CPU-Wegs.
    #[test]
    fn pq_pegel_und_rundreise() {
        for f in zeilen("pq") {
            assert_eq!(pq_code10(zahl(f[0])), zahl::<u16>(f[1]), "{} nit", f[0]);
        }
        let t = tabellen();
        let mut schlimmste = (0f64, 0f64);
        for c in 0..=1023u32 {
            let e = c as f64 / 1023.0;
            let exakt = pq_oetf(pq_eotf(e)) * 1023.0 - c as f64;
            let tab = pq_oetf(t.pq_nit(e as f32) as f64) * 1023.0 - c as f64;
            schlimmste = (schlimmste.0.max(exakt.abs()), schlimmste.1.max(tab.abs()));
        }
        // Code 0 kommt als 0,0007 zurueck: PQ(0 nit) ist c1^m2, nicht 0.
        assert!(schlimmste.0 < 1e-3, "exakt {}", schlimmste.0);
        assert!(schlimmste.1 <= 0.5, "Tabelle {}", schlimmste.1);
        // Ausserhalb geklemmt, nie NaN.
        assert_eq!(pq_eotf(-1.0), pq_eotf(0.0));
        assert_eq!(pq_eotf(2.0), 10000.0);
        assert_eq!(pq_oetf(-5.0), pq_oetf(0.0));
        assert_eq!(pq_code10(20000.0), 1023);
        assert!(pq_eotf(f64::NAN).is_finite() && pq_oetf(f64::NAN).is_finite());
    }

    fn produkt(a: &Matrix, b: &Matrix) -> Matrix {
        let mut m = [[0f32; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                m[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
            }
        }
        m
    }

    fn nahe(a: &Matrix, b: &Matrix, tol: f32, was: &str) {
        for r in 0..3 {
            for c in 0..3 {
                assert!((a[r][c] - b[r][c]).abs() <= tol, "{was}: [{r}][{c}] {} statt {}", a[r][c], b[r][c]);
            }
        }
    }

    /// Hin und zurueck ergibt die Einheit, Weiss bleibt Weiss (D65 in allen
    /// Raeumen), und 709 -> P3 ist dasselbe wie 709 -> 2020 -> P3.
    #[test]
    fn matrizen_ergeben_die_einheit() {
        let eins: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        nahe(&produkt(&M_2020_NACH_709, &M_709_NACH_2020), &eins, 1e-5, "2020->709 * 709->2020");
        nahe(&produkt(&M_709_NACH_2020, &M_2020_NACH_709), &eins, 1e-5, "709->2020 * 2020->709");
        nahe(&produkt(&M_2020_NACH_P3, &M_P3_NACH_2020), &eins, 1e-5, "2020->P3 * P3->2020");
        nahe(&produkt(&M_P3_NACH_2020, &M_2020_NACH_P3), &eins, 1e-5, "P3->2020 * 2020->P3");
        nahe(&produkt(&M_2020_NACH_P3, &M_709_NACH_2020), &M_709_NACH_P3, 1e-5, "709->P3");
        for (m, was) in [
            (&M_709_NACH_2020, "709->2020"),
            (&M_2020_NACH_709, "2020->709"),
            (&M_2020_NACH_P3, "2020->P3"),
            (&M_P3_NACH_2020, "P3->2020"),
            (&M_709_NACH_P3, "709->P3"),
        ] {
            for w in mal(m, [1.0, 1.0, 1.0]) {
                assert!((w - 1.0).abs() < 1e-5, "{was}: Weiss {w}");
            }
        }
        // Die Werte im Plan (Abschnitt 4.5) fuer 2020 -> 709, auf sechs Stellen.
        let plan: Matrix = [[1.660491, -0.587641, -0.072850], [-0.124550, 1.132900, -0.008349], [-0.018151, -0.100579, 1.118730]];
        nahe(&M_2020_NACH_709, &plan, 1e-6, "2020->709 wie im Plan");
    }

    /// Y'CbCr BT.2020-NCL: die Koeffizienten folgen aus Kr und Kb, hin und
    /// zurueck bleibt alles, Grau hat Cb = Cr = 0.
    #[test]
    fn ycbcr_2020_rundreise() {
        let kg = 1.0 - KR_2020 - KB_2020;
        assert!((CR_R_2020 - 2.0 * (1.0 - KR_2020)).abs() < 1e-6);
        assert!((CB_B_2020 - 2.0 * (1.0 - KB_2020)).abs() < 1e-6);
        assert!((CB_G_2020 - 2.0 * KB_2020 * (1.0 - KB_2020) / kg).abs() < 1e-6);
        assert!((CR_G_2020 - 2.0 * KR_2020 * (1.0 - KR_2020) / kg).abs() < 1e-6);
        for rgb in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.3, 0.6, 0.9], [0.58, 0.58, 0.58]] {
            let [y, cb, cr] = rgb_nach_ycbcr_2020(rgb);
            let zurueck = ycbcr_nach_rgb_2020(y, cb, cr);
            for k in 0..3 {
                assert!((zurueck[k] - rgb[k]).abs() < 1e-5, "{rgb:?} -> {zurueck:?}");
            }
            let rand = 0.5 + 1e-6;
            assert!((-rand..=rand).contains(&cb) && (-rand..=rand).contains(&cr), "{rgb:?}: Cb {cb} Cr {cr}");
            if rgb[0] == rgb[1] && rgb[1] == rgb[2] {
                assert!(cb.abs() < 1e-6 && cr.abs() < 1e-6);
            }
        }
    }

    /// Die Abbildung: Identitaet bis zum Knie, stetig mit Steigung 1 am Knie,
    /// monoton, Hs landet genau auf Hd, darueber abgeschnitten; D <= 0 (SDR-Ziel)
    /// schneidet bei Hd ab; Hs <= Hd laesst alles (bis Hd) stehen; farbtreu
    /// (Verhaeltnisse der Kanaele bleiben).
    #[test]
    fn abbildung_stetig_monoton_und_farbtreu() {
        for (hs, hd) in [(4.926f32, 2.0f32), (4.926, 4.0), (49.26, 1.6), (10.0, 1.11), (1000.0 / 240.0, 1015.0 / 240.0 - 0.5)] {
            let mut vorher = 0.0f32;
            let n = 20000;
            for i in 0..=n {
                let m = hs * 1.2 * i as f32 / n as f32;
                let f = abbilden(m, hs, hd);
                if m <= KNIE {
                    assert_eq!(f, m, "Identitaet bis zum Knie: Hs {hs} Hd {hd} m {m}");
                }
                assert!(f >= vorher, "monoton: Hs {hs} Hd {hd} m {m}: {f} < {vorher}");
                assert!(f - vorher <= 1.2 * hs / n as f32 + 1e-5, "Sprung bei m {m}: {vorher} -> {f}");
                assert!(f <= hd + 1e-6);
                vorher = f;
            }
            assert!((abbilden(hs, hs, hd) - hd).abs() < 1e-4, "f(Hs) = {} statt Hd {hd}", abbilden(hs, hs, hd));
            assert_eq!(abbilden(hs * 3.0, hs, hd), hd);
            // Steigung am Knie: 1 von beiden Seiten (rechts nur genaehert).
            let h = 1e-3;
            let steigung = (abbilden(KNIE + h, hs, hd) - abbilden(KNIE, hs, hd)) / h;
            assert!((steigung - 1.0).abs() < 0.02, "Steigung am Knie {steigung}");
        }
        // SDR-Ziel (D = 0) und darunter: abschneiden.
        for hd in [1.0f32, 0.8] {
            for m in [0.0f32, 0.5, 0.8, 1.0, 1.5, 7.0] {
                assert_eq!(abbilden(m, 4.926, hd), m.min(hd));
            }
        }
        // Hs <= Hd: Identitaet (bis Hd).
        for m in [0.0f32, 0.7, 1.0, 2.5, 3.9] {
            assert_eq!(abbilden(m, 3.0, 4.0), m);
        }
        assert_eq!(abbilden(5.0, 3.0, 4.0), 4.0);
        // Farbtreu: die Verhaeltnisse bleiben, der groesste Kanal wird m'.
        let rgb = [3.0f32, 1.5, 0.3];
        let a = rgb_abbilden(rgb, 4.926, 1.0);
        assert!((a[0] - 1.0).abs() < 1e-6 && (a[1] - 0.5).abs() < 1e-6 && (a[2] - 0.1).abs() < 1e-6, "{a:?}");
        let a = rgb_abbilden(rgb, 4.926, 2.0);
        assert!((a[1] / a[0] - 0.5).abs() < 1e-6 && (a[2] / a[0] - 0.1).abs() < 1e-6);
        assert_eq!(rgb_abbilden([0.2, 0.5, 0.9], 4.926, 1.0), [0.2, 0.5, 0.9]);
        assert_eq!(rgb_abbilden([-0.1, 0.0, -0.2], 4.926, 1.0), [-0.1, 0.0, -0.2]);
    }

    /// Gerundet auf FP16 (half), wie die Desktop-Textur des Windows-Hosts die
    /// Werte haelt: 10 Bit Mantisse, naechster Wert, bei Gleichstand gerade.
    fn f16_runden(x: f32) -> f32 {
        if x == 0.0 || !x.is_finite() {
            return x;
        }
        let exp = ((x.to_bits() >> 23) & 0xff) as i32 - 127;
        if exp < -14 {
            // Subnormal: Schritt 2^-24.
            let q = 2f32.powi(-24);
            return (x / q).round_ties_even() * q;
        }
        let b = x.to_bits();
        let r = b + 0x0fff + ((b >> 13) & 1);
        f32::from_bits(r & !0x1fff)
    }

    /// SDR aus scRGB (Windows-Host mit HDR-Desktop): jeder sRGB-Wert v, als
    /// scRGB mit SDR-Weiss 1,0 / 2,5 / 3,0 / 5,0 (80 / 200 / 240 / 400 nit)
    /// und auf FP16 gerundet, ergibt wieder v - in allen drei Kanaelen. Dazu:
    /// ueber Weiss farbtreu abgeschnitten, negative Anteile auf 0.
    #[test]
    fn sdr_aus_scrgb_ergibt_die_sdr_werte() {
        assert_eq!(f16_runden(1.0), 1.0);
        assert_eq!(f16_runden(1.0 + 1.0 / 4096.0), 1.0, "unter dem halben Schritt");
        assert_eq!(f16_runden(1.0 + 3.0 / 2048.0), 1.0 + 2.0 / 1024.0, "Gleichstand zur geraden Mantisse");
        let mut abweichungen = Vec::new();
        for weiss in [1.0f32, 2.5, 3.0, 5.0] {
            for v in 0..=255u8 {
                let lin = srgb_eotf(v as f64 / 255.0) as f32;
                let px = f16_runden(lin * weiss);
                let rgb = [px, f16_runden(lin * weiss * 0.5), px];
                let aus = sdr_aus_scrgb(rgb, weiss);
                if aus[0] != v || aus[2] != v {
                    abweichungen.push((weiss, v, aus));
                }
            }
        }
        assert!(abweichungen.is_empty(), "{} Abweichungen: {:?}", abweichungen.len(), &abweichungen[..abweichungen.len().min(8)]);
        // Ueber dem SDR-Weiss: farbtreu auf Weiss (2:1:0,5 bleibt 2:1:0,5 im Licht).
        let a = sdr_aus_scrgb([6.0, 3.0, 1.5], 3.0);
        assert_eq!(a, [255, srgb8(0.5), srgb8(0.25)]);
        assert_eq!(sdr_aus_scrgb([-0.2, 3.0, -0.01], 3.0), [0, 255, 0]);
        assert_eq!(sdr_aus_scrgb([0.0, 0.0, 0.0], 2.5), [0, 0, 0]);
    }

    /// Ein SDR-Wert v, wie ihn ein Host mit W_h in PQ verpackt (Y' eines Grau).
    fn pq_grau(v: u8, weiss_nit: f64) -> u16 {
        pq_code10(srgb_eotf(v as f64 / 255.0) * weiss_nit)
    }

    fn ebene16(werte: &[u16]) -> Vec<u8> {
        werte.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// Der CPU-Weg: SDR-Grau in PQ (W_h 203 und 240) kommt exakt als dasselbe
    /// sRGB heraus (bei W_h 80 oder 100 nit reicht der 10-Bit-Code fuer zwei,
    /// drei Werte nicht - das liegt an der Rundung des Codes, nicht an der
    /// Rechnung); 1000 nit auf dem SDR-Schirm sind Weiss; die Tabellen
    /// weichen von der exakten Rechnung hoechstens um einen Wert ab; alle
    /// Ebenen-Arten (planar 10 Bit, Paare oben buendig, 4:2:0, begrenzt) lesen
    /// dieselben Bildpunkte.
    #[test]
    fn zeile_rgb_pq_wie_die_referenz() {
        for weiss in [203.0f32, 240.0] {
            let info = InfoV1 { farbe: Farbe::PQ, sdr_weiss_nit: weiss as u16, ..InfoV1::sdr(GRUND_AKTIV) };
            let ab = Abbildung::sdr(Some(&info));
            let y: Vec<u16> = (0..=255u8).map(|v| pq_grau(v, weiss as f64)).collect();
            let c = vec![512u16; 256];
            let mut out = vec![0u32; 256];
            zeile_rgb_pq::<false, 10, false, false>(&mut out, &ebene16(&y), &ebene16(&c), &ebene16(&c), &ab);
            for v in 0..=255usize {
                assert_eq!(out[v], (v as u32) * 0x010101, "Grau {v} bei {weiss} nit");
                assert_eq!(pq_nach_srgb8(y[v] as f32 / 1023.0, 0.0, 0.0, &ab), [v as u8; 3], "Referenz, Grau {v}");
            }
        }
        // 1000 nit und mehr: Weiss.
        let ab = Abbildung::sdr(None);
        let mut out = [0u32; 2];
        zeile_rgb_pq::<false, 10, false, false>(&mut out, &ebene16(&[769, 1023]), &ebene16(&[512, 512]), &ebene16(&[512, 512]), &ab);
        assert_eq!(out, [0xffffff; 2]);

        // Tabellen gegen die exakte Rechnung, auf einem Gitter von Codes.
        let mut schlimmste = 0i32;
        let mut abweichend = 0usize;
        let schritte: Vec<u16> = (0..=1023).step_by(31).collect();
        for &yc in &schritte {
            for &cb in &schritte {
                let cr: Vec<u16> = schritte.clone();
                let n = cr.len();
                let mut out = vec![0u32; n];
                zeile_rgb_pq::<false, 10, false, false>(&mut out, &ebene16(&vec![yc; n]), &ebene16(&vec![cb; n]), &ebene16(&cr), &ab);
                for (i, &crc) in cr.iter().enumerate() {
                    let (y, u, v) = normieren10(yc as i32, cb as i32, crc as i32, false);
                    let r = pq_nach_srgb8(y, u, v, &ab);
                    let o = out[i];
                    let got = [(o >> 16) as u8, (o >> 8) as u8, o as u8];
                    for k in 0..3 {
                        let d = (got[k] as i32 - r[k] as i32).abs();
                        schlimmste = schlimmste.max(d);
                        abweichend += (d > 0) as usize;
                    }
                }
            }
        }
        assert!(schlimmste <= 1, "Tabelle weicht um {schlimmste} ab");
        assert!(abweichend * 100 < schritte.len().pow(3) * 3, "{abweichend} Abweichungen");

        // Ebenen-Arten: 2x2 Bildpunkte, 4:2:0 teilt einen Farbwert.
        let yc = [pq_grau(40, 203.0), pq_grau(128, 203.0), pq_grau(200, 203.0), 769];
        let (cb, cr) = (600u16, 450u16);
        let erwartet = |i: usize| {
            let (y, u, v) = normieren10(yc[i] as i32, cb as i32, cr as i32, false);
            let r = pq_nach_srgb8(y, u, v, &ab);
            ((r[0] as u32) << 16) | ((r[1] as u32) << 8) | r[2] as u32
        };
        let nahe_px = |a: u32, b: u32| (0..3).all(|k| (((a >> (8 * k)) & 0xff) as i32 - ((b >> (8 * k)) & 0xff) as i32).abs() <= 1);
        // planar 4:4:4 10 Bit LE
        let mut out = [0u32; 4];
        zeile_rgb_pq::<false, 10, false, false>(&mut out, &ebene16(&yc), &ebene16(&[cb; 4]), &ebene16(&[cr; 4]), &ab);
        let planar = out;
        for i in 0..4 {
            assert!(nahe_px(planar[i], erwartet(i)), "planar {i}");
        }
        // Paare oben buendig (xf44 / YUV444P10MSB): dieselben Bildpunkte.
        let paare: Vec<u16> = (0..4).flat_map(|_| [cb << 6, cr << 6]).collect();
        let pb = ebene16(&paare);
        zeile_rgb_pq::<false, 16, true, false>(&mut out, &ebene16(&yc.map(|v| v << 6)), &pb, &pb[2..], &ab);
        assert_eq!(out, planar, "Paare oben buendig");
        // 4:2:0 mit Paaren (P010): ein Farbwert fuer zwei Bildpunkte.
        let pb = ebene16(&[cb << 6, cr << 6, cb << 6, cr << 6]);
        zeile_rgb_pq::<true, 16, true, false>(&mut out, &ebene16(&yc.map(|v| v << 6)), &pb, &pb[2..], &ab);
        assert_eq!(out, planar, "P010");
        // Begrenzter Bereich: 64..940 wird 0..1023.
        let dehnen = |v: u16, c: bool| -> u16 {
            if c { ((v as f32 - 512.0) * 896.0 / 1023.0 + 512.0).round() as u16 } else { (v as f32 * 876.0 / 1023.0 + 64.0).round() as u16 }
        };
        let yb: Vec<u16> = yc.iter().map(|&v| dehnen(v, false)).collect();
        zeile_rgb_pq::<false, 10, false, true>(&mut out, &ebene16(&yb), &ebene16(&[dehnen(cb, true); 4]), &ebene16(&[dehnen(cr, true); 4]), &ab);
        for i in 0..4 {
            assert!(nahe_px(out[i], planar[i]), "begrenzt {i}: {:06x} statt {:06x}", out[i], planar[i]);
        }
    }

    /// Die Wahrheitstabelle aus hdr_vektoren.txt: alle Kombinationen von
    /// Quelle, Host, Kandidat und IN_ANZEIGE (ueber die Bytes gelesen).
    #[test]
    fn entscheiden_wie_die_tabelle() {
        let z = zeilen("entscheiden");
        assert_eq!(z.len(), 2 * 2 * 6 * 17);
        for f in z {
            let anzeige = anzeige_aus(f[3]);
            assert_eq!(anzeige.is_some(), f[3] != "-", "{f:?}");
            let g = hdr_entscheiden(f[0] == "1", f[1] == "1", zahl(f[2]), anzeige.as_ref());
            assert_eq!(g, zahl::<u8>(f[4]), "entscheiden {}", f.join(" "));
        }
        for g in 0..=7u8 {
            assert_ne!(grund_text(g), grund_text(8), "Grund {g} ohne Text");
        }
    }

    /// Strominfo Fassung 1 und IN_ANZEIGE: Bytes wie in den Pruefvektoren,
    /// hin und zurueck.
    #[test]
    fn info_und_anzeige_wie_die_vektoren() {
        for f in zeilen("info") {
            let i = info_aus(&f);
            assert_eq!(i.kodieren().to_vec(), hex(f[10]), "info {}", f.join(" "));
            let mut ganz = vec![0u8; 8];
            ganz.extend_from_slice(&i.kodieren());
            assert_eq!(InfoV1::lesen(&ganz), Some(i));
        }
        for f in zeilen("anzeige") {
            let a = anzeige_voll_aus(&f);
            assert_eq!(a.kodieren().to_vec(), hex(f[7]), "anzeige {}", f.join(" "));
            assert_eq!(Anzeige::lesen(&hex(f[7])), Some(a));
        }
        assert_eq!(InfoV1::sdr(GRUND_KEIN_IN_ANZEIGE).kodieren(), [1, 1, 1, 1, 1, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// Laengen und Fassungen: acht Byte (Host vor 0.2.0), zu kurz, Fassung 0
    /// oder 2 ergeben None; Bytes hinter den 24 bzw. 14 werden uebergangen.
    #[test]
    fn info_und_anzeige_lesen_defensiv() {
        let mut info = vec![0x80, 0x07, 0x38, 0x04, 0x3c, 0x00, 0x01, 0x02];
        assert_eq!(InfoV1::lesen(&info), None, "acht Byte");
        let v1 = InfoV1 { farbe: Farbe::PQ, grund: 0, sdr_weiss_nit: 203, master_max_nit: 1000, master_min_zehntausendstel: 50, max_cll: 0, max_fall: 0 };
        info.extend_from_slice(&v1.kodieren());
        assert_eq!(InfoV1::lesen(&info), Some(v1));
        assert_eq!(InfoV1::lesen(&info[..23]), None, "23 Byte");
        let mut laenger = info.clone();
        laenger.extend_from_slice(&[9; 12]);
        assert_eq!(InfoV1::lesen(&laenger), Some(v1), "Bytes dahinter");
        for fassung in [0u8, 2, 255] {
            let mut i = info.clone();
            i[8] = fassung;
            assert_eq!(InfoV1::lesen(&i), None, "Fassung {fassung}");
        }
        assert_eq!(v1.quell_weiss_nit(), 203.0);
        assert_eq!(v1.quell_spitze_nit(), 1000.0);
        let leer = InfoV1 { farbe: Farbe::PQ, ..InfoV1::sdr(0) };
        assert_eq!((leer.quell_weiss_nit(), leer.quell_spitze_nit()), (203.0, 1000.0));
        let mit_cll = InfoV1 { max_cll: 600, sdr_weiss_nit: 240, ..v1 };
        assert_eq!(Abbildung::neu(Some(&mit_cll), 2.0), Abbildung { weiss_nit: 240.0, hs: 600.0 / 240.0, hd: 2.0 });
        assert_eq!(Abbildung::sdr(None), Abbildung { weiss_nit: 203.0, hs: 1000.0 / 203.0, hd: 1.0 });

        let a = Anzeige { flags: 3, wunsch: 0, sdr_weiss_nit: 240, spitze_nit: 1015, vollbild_spitze_nit: 600, kopfraum_potentiell: 423, kopfraum_aktuell: 400 };
        let p = a.kodieren();
        assert_eq!(p[3], 0, "frei");
        assert_eq!(Anzeige::lesen(&p[..13]), None);
        let mut laenger = p.to_vec();
        laenger.push(7);
        assert_eq!(Anzeige::lesen(&laenger), Some(a));
        for fassung in [0u8, 2] {
            let mut q = p;
            q[0] = fassung;
            assert_eq!(Anzeige::lesen(&q), None, "Fassung {fassung}");
        }
        assert!(a.schirm_hdr() && a.darstellung());
        let nur_bit7 = Anzeige { flags: 0x80, ..a };
        assert!(!nur_bit7.schirm_hdr() && !nur_bit7.darstellung());
    }

    /// Die Farbe aus dem VUI: nur PQ traegt Matrix und Bereich weiter, alles
    /// andere ist SDR BT.709 voll (auch das BT.601-begrenzt-VUI des bgra-Wegs).
    #[test]
    fn farbe_aus_dem_vui() {
        assert_eq!(Farbe::aus_vui(16, 9, 9, true), Farbe::PQ);
        assert_eq!(Farbe::aus_vui(16, 9, 9, false), Farbe { voll: false, ..Farbe::PQ });
        assert_eq!(Farbe::aus_vui(6, 5, 6, false), Farbe::SDR);
        assert_eq!(Farbe::aus_vui(2, 2, 2, false), Farbe::SDR);
        assert_eq!(Farbe::aus_vui(18, 9, 9, true), Farbe::SDR, "HLG nicht in 0.2.0");
        assert_eq!(Farbe::PQ.text(), "PQ/BT.2020");
        assert_eq!(Farbe::SDR.text(), "SDR/BT.709");
        assert_eq!(transfer_text(0), "SDR");
        assert_eq!(transfer_text(16), "PQ");
    }

    // ------------------------------------------ der C-Spiegel (host/hdr.c)

    /// Die Funktionen aus host/hdr.c (in libqchost.a, nur auf dem Mac).
    #[cfg(target_os = "macos")]
    mod c {
        use std::os::raw::{c_char, c_int};

        #[repr(C)]
        pub struct Info {
            pub transfer: u8,
            pub primaer: u8,
            pub matrix: u8,
            pub voll: u8,
            pub grund: u8,
            pub sdr_weiss_nit: u16,
            pub master_max_nit: u16,
            pub master_min_zehntausendstel: u16,
            pub max_cll: u16,
            pub max_fall: u16,
        }

        #[repr(C)]
        #[derive(Default)]
        pub struct Anzeige {
            pub flags: u8,
            pub wunsch: u8,
            pub sdr_weiss_nit: u16,
            pub spitze_nit: u16,
            pub vollbild_spitze_nit: u16,
            pub kopfraum_potentiell: u16,
            pub kopfraum_aktuell: u16,
        }

        #[repr(C)]
        #[derive(Default)]
        pub struct SeiWerte {
            pub x: [u16; 3],
            pub y: [u16; 3],
            pub weiss_x: u16,
            pub weiss_y: u16,
            pub max_lum: u32,
            pub min_lum: u32,
            pub max_cll: u16,
            pub max_fall: u16,
        }

        extern "C" {
            pub fn qc_hdr_info_sdr(i: *mut Info, grund: u8);
            pub fn qc_hdr_info_kodieren(i: *const Info, out: *mut u8);
            pub fn qc_hdr_anzeige_lesen(p: *const u8, n: usize, a: *mut Anzeige) -> c_int;
            pub fn qc_hdr_codec_kann(idx: c_int) -> c_int;
            pub fn qc_hdr_entscheiden(quelle_hdr: c_int, host_kann: c_int, idx: c_int, a: *const Anzeige) -> c_int;
            pub fn qc_hdr_grund_text(grund: c_int) -> *const c_char;
            pub fn qc_hdr_pq_aus_nit(nit: f64) -> f64;
            pub fn qc_hdr_nit_aus_pq(e: f64) -> f64;
            pub fn qc_hdr_pq_code10(nit: f64) -> c_int;
            pub fn qc_hdr_sei_primaer(w: *mut SeiWerte, bt2020: c_int);
            pub fn qc_hdr_sei_bauen(w: *const SeiWerte, out: *mut u8, platz: usize) -> usize;
        }
    }

    /// Der C-Spiegel rechnet wie hdr.rs: dieselben Zeilen aus
    /// hdr_vektoren.txt (Entscheidung ueber gelesene Bytes, Strominfo,
    /// IN_ANZEIGE, PQ), dieselben Texte, dieselben Grenzfaelle beim Lesen.
    #[cfg(target_os = "macos")]
    #[test]
    fn c_spiegel_wie_die_vektoren() {
        use std::ffi::CStr;
        for f in zeilen("entscheiden") {
            let g = match anzeige_aus(f[3]) {
                None => unsafe { c::qc_hdr_entscheiden((f[0] == "1") as i32, (f[1] == "1") as i32, zahl(f[2]), std::ptr::null()) },
                Some(a) => {
                    let bytes = a.kodieren();
                    let mut ca = c::Anzeige::default();
                    assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(bytes.as_ptr(), bytes.len(), &mut ca) }, 1);
                    unsafe { c::qc_hdr_entscheiden((f[0] == "1") as i32, (f[1] == "1") as i32, zahl(f[2]), &ca) }
                }
            };
            assert_eq!(g, zahl::<i32>(f[4]), "C: entscheiden {}", f.join(" "));
        }
        for idx in 0..8 {
            assert_eq!(unsafe { c::qc_hdr_codec_kann(idx) } != 0, codec_kann_hdr(idx as u8), "idx {idx}");
        }
        for g in 0..=9u8 {
            let t = unsafe { CStr::from_ptr(c::qc_hdr_grund_text(g as i32)) }.to_str().unwrap();
            assert_eq!(t, grund_text(g), "Grund {g}");
        }
        for f in zeilen("info") {
            let r = info_aus(&f);
            let ci = c::Info {
                transfer: r.farbe.transfer,
                primaer: r.farbe.primaer,
                matrix: r.farbe.matrix,
                voll: r.farbe.voll as u8,
                grund: r.grund,
                sdr_weiss_nit: r.sdr_weiss_nit,
                master_max_nit: r.master_max_nit,
                master_min_zehntausendstel: r.master_min_zehntausendstel,
                max_cll: r.max_cll,
                max_fall: r.max_fall,
            };
            let mut out = [0xaau8; 16];
            unsafe { c::qc_hdr_info_kodieren(&ci, out.as_mut_ptr()) };
            assert_eq!(out.to_vec(), hex(f[10]), "C: info {}", f.join(" "));
        }
        let mut ci = c::Info { transfer: 0, primaer: 0, matrix: 0, voll: 0, grund: 0, sdr_weiss_nit: 9, master_max_nit: 9, master_min_zehntausendstel: 9, max_cll: 9, max_fall: 9 };
        unsafe { c::qc_hdr_info_sdr(&mut ci, GRUND_KEIN_IN_ANZEIGE) };
        let mut out = [0u8; 16];
        unsafe { c::qc_hdr_info_kodieren(&ci, out.as_mut_ptr()) };
        assert_eq!(out, InfoV1::sdr(GRUND_KEIN_IN_ANZEIGE).kodieren(), "C: SDR-Strominfo");
        for f in zeilen("anzeige") {
            let r = anzeige_voll_aus(&f);
            let b = hex(f[7]);
            let mut ca = c::Anzeige::default();
            assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(b.as_ptr(), b.len(), &mut ca) }, 1);
            assert_eq!(
                (ca.flags, ca.wunsch, ca.sdr_weiss_nit, ca.spitze_nit, ca.vollbild_spitze_nit, ca.kopfraum_potentiell, ca.kopfraum_aktuell),
                (r.flags, r.wunsch, r.sdr_weiss_nit, r.spitze_nit, r.vollbild_spitze_nit, r.kopfraum_potentiell, r.kopfraum_aktuell),
                "C: anzeige {}",
                f.join(" ")
            );
        }
        // Zu kurz, falsche Fassung, NULL: 0, und die Struktur bleibt.
        let gut = Anzeige { flags: 3, ..Anzeige::default() }.kodieren();
        let mut ca = c::Anzeige { flags: 0x55, ..c::Anzeige::default() };
        assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(gut.as_ptr(), 13, &mut ca) }, 0);
        let mut f2 = gut;
        f2[0] = 2;
        assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(f2.as_ptr(), 14, &mut ca) }, 0);
        assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(std::ptr::null(), 14, &mut ca) }, 0);
        assert_eq!(ca.flags, 0x55);
        let mut lang = gut.to_vec();
        lang.extend_from_slice(&[1, 2, 3]);
        assert_eq!(unsafe { c::qc_hdr_anzeige_lesen(lang.as_ptr(), lang.len(), &mut ca) }, 1);
        assert_eq!(ca.flags, 3);

        for f in zeilen("pq") {
            assert_eq!(unsafe { c::qc_hdr_pq_code10(zahl(f[0])) }, zahl::<i32>(f[1]), "C: pq {} nit", f[0]);
        }
        for i in 0..=2000 {
            let e = i as f64 / 2000.0;
            let nit = pq_eotf(e);
            assert!((unsafe { c::qc_hdr_nit_aus_pq(e) } - nit).abs() <= 1e-9 * nit.max(1.0), "C: EOTF bei {e}");
            assert!((unsafe { c::qc_hdr_pq_aus_nit(nit) } - pq_oetf(nit)).abs() < 1e-12, "C: OETF bei {nit} nit");
        }
        for x in [-1.0, f64::NAN, 20000.0] {
            assert_eq!(unsafe { c::qc_hdr_pq_aus_nit(x) }, pq_oetf(x), "C: OETF bei {x}");
            assert_eq!(unsafe { c::qc_hdr_nit_aus_pq(x / 10000.0) }, pq_eotf(x / 10000.0), "C: EOTF bei {x}");
        }
    }

    /// Die Praefix-SEI aus host/hdr.c: byte-genau wie die Pruefvektoren, und
    /// mit dem Bitleser aus sps.rs zurueckgelesen (Kopf, 137 mit 24 Byte, 144
    /// mit 4 Byte, Abschlussbits; der Emulationsschutz haelt jede 00 00 0x-Folge
    /// auf).
    #[cfg(target_os = "macos")]
    #[test]
    fn c_sei_byte_genau_und_zurueckgelesen() {
        use crate::sps::{entschuetzen, BitLeser};
        for f in zeilen("sei") {
            let mut w = c::SeiWerte::default();
            unsafe { c::qc_hdr_sei_primaer(&mut w, (f[0] == "2020") as i32) };
            w.max_lum = zahl(f[1]);
            w.min_lum = zahl(f[2]);
            w.max_cll = zahl(f[3]);
            w.max_fall = zahl(f[4]);
            let mut nal = [0u8; 64];
            let n = unsafe { c::qc_hdr_sei_bauen(&w, nal.as_mut_ptr(), nal.len()) };
            let nal = &nal[..n];
            assert_eq!(nal.to_vec(), hex(f[5]), "sei {}", f.join(" "));
            // Kein Startcode und keine ungeschuetzte 00 00 0x-Folge in der Nutzlast.
            assert!(nal[2..].windows(3).all(|t| !(t[0] == 0 && t[1] == 0 && t[2] <= 2)), "{nal:02x?}");

            // Kopf: forbidden 0, Typ 39, Schicht 0, tid+1 = 1.
            let mut l = BitLeser::neu(&nal[..2]);
            assert_eq!((l.u(1), l.u(6), l.u(6), l.u(3)), (Some(0), Some(39), Some(0), Some(1)));
            let rbsp = entschuetzen(&nal[2..]);
            let mut l = BitLeser::neu(&rbsp);
            assert_eq!((l.u(8), l.u(8)), (Some(137), Some(24)));
            let prim: Vec<u32> = (0..8).map(|_| l.u(16).unwrap()).collect();
            let erwartet: Vec<u32> = w.x.iter().zip(w.y.iter()).flat_map(|(&x, &y)| [x as u32, y as u32]).chain([w.weiss_x as u32, w.weiss_y as u32]).collect();
            assert_eq!(prim, erwartet);
            assert_eq!((l.u(32), l.u(32)), (Some(w.max_lum), Some(w.min_lum)));
            assert_eq!((l.u(8), l.u(8)), (Some(144), Some(4)));
            assert_eq!((l.u(16), l.u(16)), (Some(w.max_cll as u32), Some(w.max_fall as u32)));
            // rbsp_trailing_bits und nichts dahinter.
            assert_eq!((l.u(8), l.u(1)), (Some(0x80), None));
        }
        // Primaerfarben: P3 und BT.2020 in 0,00002, Reihenfolge G, B, R.
        let mut w = c::SeiWerte::default();
        unsafe { c::qc_hdr_sei_primaer(&mut w, 0) };
        assert_eq!((w.x, w.y, w.weiss_x, w.weiss_y), ([13250, 7500, 34000], [34500, 3000, 16000], 15635, 16450));
        unsafe { c::qc_hdr_sei_primaer(&mut w, 1) };
        assert_eq!((w.x, w.y), ([8500, 6550, 35400], [39850, 2300, 14600]));
        // Zu wenig Platz: 0, nichts geschrieben ueber das Ende hinaus.
        let mut klein = [0u8; 40];
        assert_eq!(unsafe { c::qc_hdr_sei_bauen(&w, klein.as_mut_ptr(), klein.len()) }, 0);
    }
}
