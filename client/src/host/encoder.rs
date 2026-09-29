// Encoder des Windows-Hosts: Kandidatentabelle (dieselben sechs Eintraege
// in derselben Reihenfolge wie g_kandidaten auf dem Mac), Befund beim Start
// (avcodec_open2 je Kandidat - gefragt, nicht geraten), die Sitzung ueber
// FFmpeg 9 (LGPL-Shared-Build, nvenc ist headers-only ueber ffnvcodec) und
// der Betrieb am Bild: Eingabeweg, Zugriffseinheit mit Stempel, Codecwechsel,
// Einstellungen im Betrieb.
//
// Ohne NVIDIA (VM) faellt Kandidat 4 auf h264_mf zurueck: der H.264-Encoder
// von Media Foundation oeffnet in Software (hw_encoding=0) und steht dann
// als "Software (MediaFoundation)" in der Koennensliste - so laeuft die
// ganze Kette Duplication -> Encoder -> Zuschauer auch ohne Karte.
//
// Eingabeweg (--encoderweg bgra|yuv444|d3d11|auto): BGRA direkt in nvenc
// (der rechnet auf der Karte nach YUV um; rgb_mode=yuv444 fuer 4:4:4, sonst
// ergibt nvenc aus RGB immer 4:2:0), yuv444p/nv12 aus eigener Umrechnung
// auf dem Prozessor, oder D3D11-Texturen ohne Kopie (hw_frames_ctx auf dem
// Geraet der Duplication). auto = die Entscheidung aus messung.txt, sonst
// bgra. 10-Bit-Kandidaten sind auf Windows immer eine Umrechnung (der
// Desktop ist 8 Bit), Media Foundation nimmt nur NV12.
//
// Alles ueber die rohe C-Schnittstelle (ffmpeg::sys), weil die Optionen der
// Hardware-Encoder (preset, tune, delay, forced-idr) nur so erreichbar sind.
//
// HDR10 (HDR-Plan 5.2): Kandidat 0 und 2 koennen in PQ laufen
// (StromFarbe::Pq2020) - VUI BT.2020-NCL / BT.2020 / SMPTE ST 2084 im vollen
// Bereich, und VOR avcodec_open2 die Seitendaten MASTERING_DISPLAY
// (BT.2020-Primaerfarben, D65, Spitze und Minimum des Host-Schirms) und
// CONTENT_LIGHT_LEVEL 0/0 am Kontext (decoded_side_data) - nur daraus schreibt
// nvenc die SEI 137/144 vor jedes Schluesselbild. Eingabe ist dann immer der
// Systemspeicher (YUV444P16LE bzw. P010 aus den PQ-Ebenen des Wandlers), nie
// der RGB-Weg: nvenc schreibt bei RGB-Eingang ein VUI BT.470BG begrenzt. Ob
// der Rechner das kann, fragt der Befund mit einer Probeoeffnung in PQ.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::Mutex;

use ffmpeg_next as ffmpeg;
use ffmpeg::sys::*;
use rayon::prelude::*;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BOX, D3D11_TEXTURE2D_DESC};

use super::{log, netz, Z};

pub struct Kandidat {
    pub name: &'static str,
    pub encoder: &'static str,
    /// Eingabeformat fuer Befund und den Prozessorweg.
    pub pix_fmt: AVPixelFormat,
    /// Profil-Option des Encoders (leer = Vorgabe).
    pub profil: &'static str,
    pub chroma444: bool,
    pub zehn_bit: bool,
    /// Der Desktop ist 8 Bit BGRA: alles, was nicht daraus ohne Umrechnung
    /// durch NVENC hervorgeht, traegt den Vermerk (wie beim Mac bei 444/8).
    pub umrechnung: bool,
    pub h264: bool,
}

pub const KANDIDATEN: [Kandidat; 6] = [
    Kandidat { name: "HEVC 4:4:4 10 Bit", encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_YUV444P16LE, profil: "rext", chroma444: true, zehn_bit: true, umrechnung: true, h264: false },
    Kandidat { name: "HEVC 4:4:4 8 Bit", encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_YUV444P, profil: "rext", chroma444: true, zehn_bit: false, umrechnung: false, h264: false },
    Kandidat { name: "HEVC 4:2:0 10 Bit", encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_P010LE, profil: "main10", chroma444: false, zehn_bit: true, umrechnung: true, h264: false },
    Kandidat { name: "HEVC 4:2:0 8 Bit", encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_NV12, profil: "main", chroma444: false, zehn_bit: false, umrechnung: false, h264: false },
    Kandidat { name: "H.264 High", encoder: "h264_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_NV12, profil: "high", chroma444: false, zehn_bit: false, umrechnung: false, h264: true },
    Kandidat { name: "AV1", encoder: "av1_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_NV12, profil: "", chroma444: false, zehn_bit: false, umrechnung: false, h264: false },
];

/// Rueckfall fuer Kandidat 4 ohne NVIDIA: Media Foundation in Software.
const MF_H264: &str = "h264_mf";

#[derive(Clone, Copy)]
pub struct Befund {
    pub vorhanden: bool,
    pub hardware: bool,
    /// Welcher Encoder den Kandidaten oeffnet (nvenc oder der MF-Rueckfall).
    pub encoder: &'static str,
    /// Bilder, die der Encoder bauartbedingt zurueckhaelt (Software-MFT: 16).
    pub vorlauf: usize,
    /// Die Probeoeffnung in PQ/BT.2020 mit HDR10-Seitendaten ging auf
    /// (nur Kandidat 0 und 2 auf nvenc).
    pub hdr: bool,
}

const KEIN_BEFUND: Befund = Befund { vorhanden: false, hardware: false, encoder: "", vorlauf: 0, hdr: false };

// ------------------------------------------------------------------ Farbe

/// Mastering-Angaben eines HDR10-Stroms (SEI 137, Strominfo Byte 16-19): die
/// Spitze in nit und das Minimum in 0,0001 nit. Die Primaerfarben sind immer
/// BT.2020 mit D65 - der Farbraum der Leitung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mastering {
    pub max_nit: u16,
    pub min_zehntausendstel: u16,
}

impl Mastering {
    /// Ohne Angabe des Schirms: 1000 nit und 0,005 nit wie beim Mac-Host.
    pub const VORGABE: Mastering = Mastering { max_nit: 1000, min_zehntausendstel: 50 };
}

/// Die Farbe einer Encoder-Sitzung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StromFarbe {
    /// BT.709, voller Bereich - wie jeder Strom vor 0.2.0.
    Sdr709,
    /// HDR10: BT.2020, PQ, BT.2020-NCL, voller Bereich, mit Mastering-Angaben.
    Pq2020(Mastering),
}

impl StromFarbe {
    pub fn ist_pq(&self) -> bool {
        matches!(self, StromFarbe::Pq2020(_))
    }

    /// Transfer nach H.273 (Switch 7 Byte 6, Strominfo Byte 9).
    pub fn transfer(&self) -> u8 {
        if self.ist_pq() { crate::hdr::TRANSFER_PQ } else { crate::hdr::TRANSFER_SDR }
    }

    pub fn text(&self) -> String {
        match self {
            StromFarbe::Sdr709 => "BT.709".into(),
            StromFarbe::Pq2020(m) => format!("HDR10 PQ/BT.2020, Mastering {:.4}-{} nit", m.min_zehntausendstel as f32 / 10000.0, m.max_nit),
        }
    }

    /// Das VUI der Sitzung: Matrix, Primaerfarben, Transfer, Bereich.
    fn vui(&self) -> (AVColorSpace, AVColorPrimaries, AVColorTransferCharacteristic, AVColorRange) {
        match self {
            StromFarbe::Sdr709 => (AVColorSpace::AVCOL_SPC_BT709, AVColorPrimaries::AVCOL_PRI_BT709, AVColorTransferCharacteristic::AVCOL_TRC_BT709, AVColorRange::AVCOL_RANGE_JPEG),
            StromFarbe::Pq2020(_) => (
                AVColorSpace::AVCOL_SPC_BT2020_NCL,
                AVColorPrimaries::AVCOL_PRI_BT2020,
                AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084,
                AVColorRange::AVCOL_RANGE_JPEG,
            ),
        }
    }
}

/// Die Mastering-Angaben des aufgenommenen Schirms (Z, beim Aufbau der
/// Duplication eingetragen); ohne Angabe die Vorgabe.
pub fn mastering() -> Mastering {
    let max = Z.hdr_master_max_nit.load(Ordering::Relaxed);
    let min = Z.hdr_master_min.load(Ordering::Relaxed);
    Mastering {
        max_nit: if max == 0 { Mastering::VORGABE.max_nit } else { max.min(u16::MAX as u32) as u16 },
        min_zehntausendstel: if min == 0 { Mastering::VORGABE.min_zehntausendstel } else { min.min(u16::MAX as u32) as u16 },
    }
}

/// Die Farbe fuer eine neue Sitzung: PQ mit den Metadaten des aufgenommenen
/// Schirms oder SDR.
pub fn stromfarbe(pq: bool) -> StromFarbe {
    if pq { StromFarbe::Pq2020(mastering()) } else { StromFarbe::Sdr709 }
}

/// AVMasteringDisplayMetadata (libavutil/mastering_display_metadata.h; die
/// Bindungen von ffmpeg-sys kennen den Kopf nicht): 88 Byte, wie konserve.rs
/// sie liest.
#[repr(C)]
#[derive(Clone, Copy)]
struct AvMastering {
    /// R, G, B je (x, y).
    display_primaries: [[AVRational; 2]; 3],
    white_point: [AVRational; 2],
    min_luminance: AVRational,
    max_luminance: AVRational,
    has_primaries: i32,
    has_luminance: i32,
}

/// AVContentLightMetadata: MaxCLL und MaxFALL (unsigned).
#[repr(C)]
#[derive(Clone, Copy)]
struct AvLichtpegel {
    max_cll: u32,
    max_fall: u32,
}

/// Nenner der Farbkoordinaten (0,00002 - die Einheit der SEI 137).
const FARBORT_NENNER: i32 = 50_000;
/// Nenner der Leuchtdichte (0,0001 nit - die Einheit der SEI 137).
const LEUCHT_NENNER: i32 = 10_000;

/// Die Mastering-Seitendaten: BT.2020-Primaerfarben (R 0,708/0,292, G
/// 0,170/0,797, B 0,131/0,046), Weisspunkt D65 (0,3127/0,3290), Leuchtdichte
/// min/max in 0,0001 nit.
fn av_mastering(m: &Mastering) -> AvMastering {
    let r = |num: i32, den: i32| AVRational { num, den };
    let ort = |x: f64, y: f64| [r((x * FARBORT_NENNER as f64).round() as i32, FARBORT_NENNER), r((y * FARBORT_NENNER as f64).round() as i32, FARBORT_NENNER)];
    AvMastering {
        display_primaries: [ort(0.708, 0.292), ort(0.170, 0.797), ort(0.131, 0.046)],
        white_point: ort(0.3127, 0.3290),
        min_luminance: r(m.min_zehntausendstel as i32, LEUCHT_NENNER),
        max_luminance: r(m.max_nit as i32 * LEUCHT_NENNER, LEUCHT_NENNER),
        has_primaries: 1,
        has_luminance: 1,
    }
}

/// Die HDR10-Seitendaten an den Kontext haengen (decoded_side_data), VOR
/// avcodec_open2: MASTERING_DISPLAY und CONTENT_LIGHT_LEVEL (0/0 = unbekannt).
unsafe fn hdr10_seitendaten(ctx: *mut AVCodecContext, m: &Mastering) -> Result<(), String> {
    let mdm = av_mastering(m);
    let cll = AvLichtpegel { max_cll: 0, max_fall: 0 };
    for (typ, daten, laenge) in [
        (AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA, &mdm as *const AvMastering as *const u8, std::mem::size_of::<AvMastering>()),
        (AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL, &cll as *const AvLichtpegel as *const u8, std::mem::size_of::<AvLichtpegel>()),
    ] {
        // Flag 1 = AV_FRAME_SIDE_DATA_FLAG_UNIQUE: eine vorhandene gleicher Art ersetzen.
        let sd = av_frame_side_data_new(&mut (*ctx).decoded_side_data, &mut (*ctx).nb_decoded_side_data, typ, laenge, 1);
        if sd.is_null() || (*sd).data.is_null() || (*sd).size < laenge {
            return Err(format!("HDR10-Seitendaten (Art {}) nicht anzulegen", typ as i32));
        }
        std::ptr::copy_nonoverlapping(daten, (*sd).data, laenge);
    }
    Ok(())
}

static BEFUND: Mutex<[Befund; 6]> = Mutex::new([KEIN_BEFUND; 6]);
/// Konserve als Bildquelle: genau dieser Eintrag gilt als vorhanden (ohne
/// Hardware-Vermerk), ein Codecwechsel wird abgelehnt. -1 = keine Konserve.
static KONSERVE_IDX: AtomicI32 = AtomicI32::new(-1);
/// Codecwunsch vom Client (Nachricht 66), den der Aufnahmefaden abholt.
static CODEC_WUNSCH: AtomicI32 = AtomicI32::new(-1);
/// Der geltende Eingabeweg (fuer den Umrechnungsvermerk der Koennensliste).
static WEG: AtomicU8 = AtomicU8::new(0);

pub fn kandidat(idx: usize) -> &'static Kandidat {
    &KANDIDATEN[idx.min(KANDIDATEN.len() - 1)]
}

/// Kennt das Protokoll den Codec? Strominfo (Nachricht 1) und Codecwechsel
/// (Nachricht 7) unterscheiden nur HEVC und H.264 - AV1 kaeme beim
/// Zuschauer als HEVC an und bliebe schwarz. Bis Protokoll und Client AV1
/// kennen, gilt er als nicht vorhanden.
pub fn im_protokoll(k: &Kandidat) -> bool {
    if k.h264 { k.encoder.starts_with("h264_") } else { k.encoder.starts_with("hevc_") }
}

pub fn befund(idx: usize) -> Befund {
    BEFUND.lock().unwrap()[idx.min(KANDIDATEN.len() - 1)]
}

pub fn konserve_setzen(idx: usize) {
    KONSERVE_IDX.store(idx as i32, Ordering::Relaxed);
    super::Z.codec_id.store(idx as u32, Ordering::Relaxed);
    super::Z.konserve.store(true, Ordering::Relaxed);
}

/// Startkandidat: 0 wie der Mac, wenn vorhanden, sonst der erste vorhandene.
pub fn startkandidat() -> Option<usize> {
    let b = BEFUND.lock().unwrap();
    if b[0].vorhanden { Some(0) } else { b.iter().position(|x| x.vorhanden) }
}

pub fn pix_fmt_name(p: AVPixelFormat) -> String {
    unsafe {
        let n = av_get_pix_fmt_name(p);
        if n.is_null() { "?".into() } else { CStr::from_ptr(n).to_string_lossy().into_owned() }
    }
}

// ------------------------------------------------------------ Eingabeweg

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weg {
    Bgra,
    Yuv444,
    D3d11,
    Auto,
}

impl Weg {
    pub fn aus_text(t: &str) -> Option<Weg> {
        match t {
            "bgra" => Some(Weg::Bgra),
            "yuv444" => Some(Weg::Yuv444),
            "d3d11" => Some(Weg::D3d11),
            "auto" => Some(Weg::Auto),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Weg::Bgra => "bgra (NVENC rechnet um)",
            Weg::Yuv444 => "yuv444 (Umrechnung auf dem Prozessor)",
            Weg::D3d11 => "d3d11 (Texturen ohne Kopie)",
            Weg::Auto => "auto",
        }
    }
}

/// Was messung.txt fuer einen Ausgang entschieden hat: (Null-Kopien,
/// Farbprobe bgra bestanden, Farbprobe yuv444p bestanden); None ohne den
/// Abschnitt "=== Entscheidung ===". Es zaehlt nur die Zeile des Ausgangs in
/// voller Groesse - der Strom ist nativ. Eine Messung von frueher fuehrt fuer
/// Ausgaenge ab 3840 Breite zusaetzlich einen 1080p-Ausschnitt ("Ausgang 0
/// (1920x1080 Ausschnitt): Null-Kopien ..."), gemessen, als der Strom dort
/// noch halbiert wurde; der sagt ueber 4K nichts.
pub fn messung_lesen(text: &str, ausgang_idx: usize) -> Option<(bool, bool, bool)> {
    let ent = text.split("=== Entscheidung ===").nth(1)?;
    let kopf = format!("Ausgang {ausgang_idx} ");
    let null_kopien = ent.lines().any(|z| z.starts_with(&kopf) && !z.contains("Ausschnitt") && z.contains("Null-Kopien"));
    let farb_bgra = ent.lines().any(|z| z.starts_with("Farbprobe:") && z.contains("bgra direkt bestanden"));
    let farb_444 = ent.lines().any(|z| z.starts_with("Farbprobe:") && z.contains("yuv444p bestanden"));
    Some((null_kopien, farb_bgra, farb_444))
}

/// Der Weg aus der Messung: Null-Kopien (mit bestandener Farbprobe bgra)
/// heisst d3d11, sonst entscheidet die Farbprobe zwischen bgra und yuv444;
/// ohne bestandene Probe bgra. Die Groesse des Ausgangs spielt keine Rolle.
pub fn weg_aus_messung(null_kopien: bool, farb_bgra: bool, farb_444: bool) -> Weg {
    if null_kopien && farb_bgra {
        Weg::D3d11
    } else if farb_bgra {
        Weg::Bgra
    } else if farb_444 {
        Weg::Yuv444
    } else {
        Weg::Bgra
    }
}

/// auto -> die Entscheidung aus messung.txt (messung_lesen, weg_aus_messung);
/// ohne Datei oder ohne Entscheidung bleibt es bei bgra.
pub fn weg_entscheiden(wunsch: Weg, ausgang_idx: usize) -> Weg {
    if wunsch != Weg::Auto {
        WEG.store(weg_code(wunsch), Ordering::Relaxed);
        log(format!("Encoderweg: {} (Befehlszeile)", wunsch.name()));
        return wunsch;
    }
    let text = crate::einstellungen::datei_pfad("messung.txt").and_then(|p| std::fs::read_to_string(p).ok());
    let weg = match text.as_deref().and_then(|t| messung_lesen(t, ausgang_idx)) {
        Some((null_kopien, farb_bgra, farb_444)) => {
            let w = weg_aus_messung(null_kopien, farb_bgra, farb_444);
            log(format!("Encoderweg: {} (aus messung.txt: Null-Kopien {}, Farbprobe bgra {}, yuv444p {})", w.name(),
                if null_kopien { "ja" } else { "nein" }, if farb_bgra { "bestanden" } else { "nicht bestanden" }, if farb_444 { "bestanden" } else { "nicht bestanden" }));
            w
        }
        None => {
            log("Encoderweg: bgra (keine Entscheidung in messung.txt - erst --messen laufen lassen)");
            Weg::Bgra
        }
    };
    WEG.store(weg_code(weg), Ordering::Relaxed);
    weg
}

fn weg_code(w: Weg) -> u8 {
    match w {
        Weg::Bgra => 0,
        Weg::Yuv444 => 1,
        Weg::D3d11 => 2,
        Weg::Auto => 0,
    }
}

/// Eingabeformat eines Kandidaten auf dem Prozessorweg (fuer Befund, Betrieb
/// und Testbild). 10 Bit und Media Foundation sind immer Systemspeicher.
fn eingabeformat(idx: usize, weg: Weg, encoder: &str) -> (AVPixelFormat, bool) {
    let k = kandidat(idx);
    if encoder == MF_H264 {
        return (AVPixelFormat::AV_PIX_FMT_NV12, false);
    }
    if k.zehn_bit {
        return (k.pix_fmt, false);
    }
    match weg {
        Weg::Bgra => (AVPixelFormat::AV_PIX_FMT_BGRA, false),
        Weg::D3d11 => (AVPixelFormat::AV_PIX_FMT_BGRA, true),
        _ => (k.pix_fmt, false),
    }
}

/// Nimmt der Encoder dieses Kandidaten auf diesem Weg Texturen? Danach
/// richtet sich die Aufnahme (DEFAULT-Kopie oder STAGING) - nicht nach dem
/// Weg allein: 10 Bit und Media Foundation nehmen auch bei d3d11 den
/// Systemspeicher.
pub fn texturweg(idx: usize, weg: Weg) -> bool {
    eingabeformat(idx, weg, befund(idx).encoder).1
}

/// Umrechnungsvermerk fuer die Koennensliste: die Tabelle, plus alles, was
/// auf dem gewaehlten Weg ueber den Prozessor geht.
fn umrechnung_fuer(idx: usize, b: &Befund) -> bool {
    let k = kandidat(idx);
    k.umrechnung || b.encoder == MF_H264 || WEG.load(Ordering::Relaxed) == 1
}

// ---------------------------------------------------------------- Befund

/// Befund beim Start: fuer jeden Kandidaten eine Sitzung mit 1920x1080 und
/// seinem Eingabeformat oeffnen - was hier nicht aufgeht, kann die Maschine
/// nicht, egal was in der Encoderliste steht. Ohne NVIDIA-Treiber (VM)
/// scheitert nvenc mit "Cannot load nvcuda.dll" - das steht dann da; fuer
/// Kandidat 4 wird dann h264_mf in Software probiert (oeffnen UND ein Bild
/// codieren, hevc_mf oeffnet zwar, liefert aber nichts).
pub fn pruefen() {
    log("--- Was dieser Rechner codieren kann ---");
    let mut befund = BEFUND.lock().unwrap();
    for (i, k) in KANDIDATEN.iter().enumerate() {
        if !im_protokoll(k) {
            befund[i] = KEIN_BEFUND;
            log(format!("  {:<18} nein (Protokoll und Client kennen {} noch nicht)", k.name, k.name));
            continue;
        }
        crate::protokoll::fehler_verwerfen();
        match Sitzung::oeffnen(&Oeffnung { encoder: k.encoder, pix_fmt: k.pix_fmt, profil: k.profil, rgb_444: k.chroma444, ..Oeffnung::vorgabe(1920, 1080) }) {
            Ok(_) => {
                befund[i] = Befund { vorhanden: true, hardware: true, encoder: k.encoder, vorlauf: 0, hdr: false };
                log(format!("  {:<18} ja (Hardware, {} {}{})", k.name, k.encoder, pix_fmt_name(k.pix_fmt), if k.umrechnung { ", mit Umrechnung" } else { "" }));
                if crate::hdr::codec_kann_hdr(i as u8) {
                    // HDR10: dieselbe Sitzung in PQ/BT.2020 mit Seitendaten.
                    crate::protokoll::fehler_verwerfen();
                    let o = Oeffnung { encoder: k.encoder, pix_fmt: k.pix_fmt, profil: k.profil, rgb_444: k.chroma444, farbe: StromFarbe::Pq2020(Mastering::VORGABE), ..Oeffnung::vorgabe(1920, 1080) };
                    match Sitzung::oeffnen(&o) {
                        Ok(_) => {
                            befund[i].hdr = true;
                            log(format!("  {:<18}   HDR10: ja (PQ/BT.2020, SEI 137/144 aus den Seitendaten)", ""));
                        }
                        Err(e) => log(format!("  {:<18}   HDR10: nein ({e})", "")),
                    }
                }
            }
            Err(e) => {
                befund[i] = KEIN_BEFUND;
                if k.h264 {
                    match mf_probe(MF_H264) {
                        Ok(n) => {
                            befund[i] = Befund { vorhanden: true, hardware: false, encoder: MF_H264, vorlauf: n - 1, hdr: false };
                            log(format!("  {:<18} ja (Software (MediaFoundation), {MF_H264} nv12, Vorlauf {} Bilder; nvenc: {e})", k.name, n - 1));
                            continue;
                        }
                        Err(m) => log(format!("  {:<18} nein ({e}; {MF_H264}: {m})", k.name)),
                    }
                } else {
                    log(format!("  {:<18} nein ({e})", k.name));
                }
            }
        }
    }
}

/// Media-Foundation-Encoder in Software oeffnen und Testbilder codieren.
/// Liefert, nach wie vielen Bildern das erste Paket kam: der Software-MFT
/// von Windows haelt 16 Bilder zurueck (gemessen, unabhaengig von
/// rate_control, scenario, AVLowLatencyMode, Bilddauer oder Wartezeit) -
/// diesen Vorlauf muss der Schrittmacher dem Encoder zugestehen.
/// 0 = oeffnet, codiert aber nichts.
fn mf_probe(name: &'static str) -> Result<usize, String> {
    crate::protokoll::fehler_verwerfen();
    let o = Oeffnung { encoder: name, pix_fmt: AVPixelFormat::AV_PIX_FMT_NV12, profil: "", ..Oeffnung::vorgabe(1920, 1080) };
    let mut s = Sitzung::oeffnen(&o).map_err(|e| format!("oeffnet nicht: {e}"))?;
    let mut b = Bild::neu(AVPixelFormat::AV_PIX_FMT_NV12, 1920, 1080)?;
    let mut erstes = 0usize;
    for k in 0..40usize {
        b.schreibbar()?;
        let [y, uv] = super::testbild::nv12(1920, 1080, k % super::testbild::N);
        b.ebene_fuellen(0, &y, 1920, 1080)?;
        b.ebene_fuellen(1, &uv, 1920, 540)?;
        s.senden(b.frame, (k as i64) * 16_667, k == 0)?;
        if !s.empfangen()?.is_empty() {
            erstes = k + 1;
            break;
        }
    }
    let rest = s.leeren();
    if erstes == 0 && rest.is_empty() {
        return Err("oeffnet, codiert aber kein Bild".into());
    }
    if erstes == 0 {
        return Err(format!("liefert erst beim Leeren ({} Pakete) - untauglich fuer den Betrieb", rest.len()));
    }
    Ok(erstes)
}

/// Schritt 0: oeffnen die Media-Foundation-Encoder ohne Karte in Software -
/// und codieren sie dann auch? (hevc_mf oeffnet auf der VM, nimmt aber kein
/// Bild an - 80004005.)
pub fn mf_pruefen() {
    for name in ["h264_mf", "hevc_mf"] {
        match mf_probe(name) {
            Ok(n) => log(format!("{name}: oeffnet in Software: ja, codiert: ja, erstes Paket nach {n} Bildern (Vorlauf {})", n - 1)),
            Err(e) if e.starts_with("oeffnet nicht") => log(format!("{name}: oeffnet in Software: nein ({e})")),
            Err(e) => log(format!("{name}: oeffnet in Software: ja, codiert: NEIN ({e})")),
        }
    }
}

/// Koennensliste (Nachricht 8): u8 n, je Eintrag u8 idx, u8 vorhanden,
/// u8 hardware, u8 umrechnung, u8 444, u8 10bit, u8 Namenslaenge, Name.
pub fn codecs_payload() -> Vec<u8> {
    let befund = *BEFUND.lock().unwrap();
    let konserve = KONSERVE_IDX.load(Ordering::Relaxed);
    let mut buf = Vec::with_capacity(256);
    buf.push(KANDIDATEN.len() as u8);
    for (i, k) in KANDIDATEN.iter().enumerate() {
        let (vorhanden, hardware, umrechnung) = if konserve >= 0 {
            (i as i32 == konserve, false, false)
        } else {
            (befund[i].vorhanden, befund[i].hardware, umrechnung_fuer(i, &befund[i]))
        };
        let name = &k.name.as_bytes()[..k.name.len().min(31)];
        buf.push(i as u8);
        buf.push(vorhanden as u8);
        buf.push(hardware as u8);
        buf.push(umrechnung as u8);
        buf.push(k.chroma444 as u8);
        buf.push(k.zehn_bit as u8);
        buf.push(name.len() as u8);
        buf.extend_from_slice(name);
    }
    buf
}

/// Typ 7: u8 idx, u8 h264, u8 chroma444, u8 zehn_bit, u8 vollbereich (immer 1),
/// u8 umrechnung, u8 Transfer nach H.273 (1 SDR, 16 PQ), u8 frei.
pub fn switch_nutzlast(idx: usize, transfer: u8) -> [u8; 8] {
    let k = kandidat(idx);
    let b = befund(idx);
    [idx as u8, k.h264 as u8, k.chroma444 as u8, k.zehn_bit as u8, 1, umrechnung_fuer(idx, &b) as u8, transfer, 0]
}

/// Switch 7 fuer diesen Kandidaten in dieser Farbe (Transfer nach H.273).
pub fn switch_senden(idx: usize, transfer: u8) {
    let k = kandidat(idx);
    debug_assert!(im_protokoll(k), "{}: Codec nicht im Protokoll", k.name);
    netz::send_small(crate::protokoll_konst::MSG_SWITCH, &switch_nutzlast(idx, transfer));
}

/// Codecwunsch vom Client (Nachricht 66). Mit Konserve: abgelehnt. Sonst
/// wird er vorgemerkt und vom Aufnahmefaden zwischen zwei Bildern
/// ausgefuehrt - nie mitten in einer Sitzung.
pub fn codec_wunsch(idx: usize) {
    if idx >= KANDIDATEN.len() {
        log(format!("Codecwunsch {idx} abgelehnt: kein solcher Kandidat"));
        return;
    }
    if KONSERVE_IDX.load(Ordering::Relaxed) >= 0 {
        log(format!("Codecwunsch {idx} ({}) abgelehnt: Konserve", KANDIDATEN[idx].name));
        return;
    }
    if !befund(idx).vorhanden {
        log(format!("Codecwunsch {idx} abgelehnt: auf diesem Rechner nicht vorhanden"));
        return;
    }
    CODEC_WUNSCH.store(idx as i32, Ordering::Relaxed);
}

/// Der Aufnahmefaden holt den vorgemerkten Wunsch ab.
pub fn codec_wunsch_abholen() -> Option<usize> {
    let w = CODEC_WUNSCH.swap(-1, Ordering::Relaxed);
    if w >= 0 { Some(w as usize) } else { None }
}

// ------------------------------------------------------------------ Sitzung

/// Was eine Sitzung braucht.
pub struct Oeffnung<'a> {
    pub encoder: &'a str,
    pub pix_fmt: AVPixelFormat,
    pub profil: &'a str,
    pub w: i32,
    pub h: i32,
    pub fps: i32,
    pub mbit: i32,
    /// nvenc "delay": 0 = synchron, 2 = bis zwei Bilder unterwegs.
    pub delay: i32,
    pub preset: &'a str,
    /// Texturen der Karte statt Systemspeicher (Null-Kopien-Weg).
    pub hw_frames: Option<*mut AVBufferRef>,
    /// RGB-Eingabe soll 4:4:4 ergeben (nvenc rgb_mode=yuv444; Vorgabe waere 4:2:0).
    pub rgb_444: bool,
    /// SDR (BT.709) oder HDR10 (PQ/BT.2020 mit Seitendaten).
    pub farbe: StromFarbe,
    pub extra: &'a [(&'a str, &'a str)],
}

impl<'a> Oeffnung<'a> {
    pub fn vorgabe(w: i32, h: i32) -> Oeffnung<'a> {
        Oeffnung {
            encoder: "hevc_nvenc",
            pix_fmt: AVPixelFormat::AV_PIX_FMT_BGRA,
            profil: "rext",
            w,
            h,
            fps: 60,
            mbit: 50,
            delay: 0,
            preset: "p1",
            hw_frames: None,
            rgb_444: true,
            farbe: StromFarbe::Sdr709,
            extra: &[],
        }
    }
}

pub struct Paket {
    pub data: Vec<u8>,
    pub key: bool,
    pub pts: i64,
}

pub struct Sitzung {
    ctx: *mut AVCodecContext,
    pkt: *mut AVPacket,
    pub name: String,
}

unsafe impl Send for Sitzung {}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

unsafe fn opt(ctx: *mut AVCodecContext, key: &str, val: &str) {
    let k = cstr(key);
    let v = cstr(val);
    av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
}

/// VUI des Kontexts nach der Farbe, bei HDR10 dazu die Seitendaten - alles
/// VOR avcodec_open2 (danach liest nvenc sie nicht mehr).
unsafe fn farbe_setzen(ctx: *mut AVCodecContext, farbe: &StromFarbe) -> Result<(), String> {
    let (matrix, primaer, transfer, bereich) = farbe.vui();
    (*ctx).colorspace = matrix;
    (*ctx).color_primaries = primaer;
    (*ctx).color_trc = transfer;
    (*ctx).color_range = bereich;
    match farbe {
        StromFarbe::Sdr709 => Ok(()),
        StromFarbe::Pq2020(m) => hdr10_seitendaten(ctx, m),
    }
}

impl Sitzung {
    /// Sitzung oeffnen. Optionen wie im Plan: preset, tune ull, rc cbr,
    /// b = maxrate = mbit, bufsize ein Bild, GOP gross (verwaltet der Host:
    /// forced-idr), keine B-Bilder, kein Lookahead, BT.709 Vollbereich (bei
    /// HDR10 BT.2020/PQ mit Seitendaten, farbe_setzen), Zeitbasis 1/1000000,
    /// KEIN GLOBAL_HEADER (Parametersaetze vor jedem IDR).
    /// Media Foundation: Software, CBR, Bildschirmuebertragung, geringe
    /// Verzoegerung; pict_type I erzwingt dort ebenfalls ein Schluesselbild.
    pub fn oeffnen(o: &Oeffnung) -> Result<Sitzung, String> {
        if o.farbe.ist_pq() && !matches!(o.pix_fmt, AVPixelFormat::AV_PIX_FMT_YUV444P16LE | AVPixelFormat::AV_PIX_FMT_P010LE) {
            return Err(format!("{}: HDR10 nur mit 10-Bit-Ebenen (YUV444P16LE, P010), nicht {}", o.encoder, pix_fmt_name(o.pix_fmt)));
        }
        unsafe {
            let name = cstr(o.encoder);
            let codec = avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(format!("{}: nicht in dieser FFmpeg-Fassung", o.encoder));
            }
            let ctx = avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err(format!("{}: kein Kontext", o.encoder));
            }
            (*ctx).width = o.w;
            (*ctx).height = o.h;
            (*ctx).pix_fmt = if o.hw_frames.is_some() { AVPixelFormat::AV_PIX_FMT_D3D11 } else { o.pix_fmt };
            (*ctx).time_base = AVRational { num: 1, den: 1_000_000 };
            (*ctx).framerate = AVRational { num: o.fps.max(1), den: 1 };
            (*ctx).bit_rate = o.mbit as i64 * 1_000_000;
            (*ctx).rc_max_rate = o.mbit as i64 * 1_000_000;
            (*ctx).rc_buffer_size = (o.mbit as i64 * 1_000_000 / o.fps.max(1) as i64) as i32;
            (*ctx).gop_size = 1 << 20;
            (*ctx).max_b_frames = 0;
            if let Err(e) = farbe_setzen(ctx, &o.farbe) {
                let mut c = ctx;
                avcodec_free_context(&mut c);
                return Err(format!("{}: {e}", o.encoder));
            }
            if let Some(hw) = o.hw_frames {
                (*ctx).hw_frames_ctx = av_buffer_ref(hw);
            }
            if o.encoder.ends_with("_nvenc") {
                opt(ctx, "preset", o.preset);
                opt(ctx, "tune", "ull");
                opt(ctx, "rc", "cbr");
                opt(ctx, "delay", &o.delay.to_string());
                opt(ctx, "forced-idr", "1");
                opt(ctx, "rc-lookahead", "0");
                opt(ctx, "zerolatency", "1");
                opt(ctx, "rgb_mode", if o.rgb_444 { "yuv444" } else { "yuv420" });
                if !o.profil.is_empty() {
                    opt(ctx, "profile", o.profil);
                }
            } else if o.encoder.ends_with("_mf") {
                // Software: ehrlich als solche gemeldet, und auf der VM der
                // einzige Weg. GOP begrenzt, falls der MFT das erzwungene
                // Schluesselbild nicht annimmt.
                opt(ctx, "hw_encoding", "0");
                opt(ctx, "rate_control", "cbr");
                opt(ctx, "scenario", "display_remoting");
                (*ctx).flags |= AV_CODEC_FLAG_LOW_DELAY as i32;
                (*ctx).gop_size = o.fps.max(1) * 2;
            }
            for (k, v) in o.extra {
                opt(ctx, k, v);
            }
            let r = avcodec_open2(ctx, codec, std::ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                avcodec_free_context(&mut c);
                return Err(crate::ffmpeg_grund(o.encoder, r));
            }
            let pkt = av_packet_alloc();
            Ok(Sitzung { ctx, pkt, name: o.encoder.to_string() })
        }
    }

    pub fn ctx(&self) -> *mut AVCodecContext {
        self.ctx
    }

    /// nvenc stellt Bitrate, Maxrate und VBV im Betrieb um (reconfig_encoder
    /// beim naechsten Bild, mit erzwungenem IDR); alle anderen brauchen einen
    /// Neustart.
    pub fn kann_umstellen(&self) -> bool {
        self.name.ends_with("_nvenc")
    }

    pub fn bitrate_setzen(&mut self, mbit: i32, fps: i32) {
        unsafe {
            (*self.ctx).bit_rate = mbit as i64 * 1_000_000;
            (*self.ctx).rc_max_rate = mbit as i64 * 1_000_000;
            (*self.ctx).rc_buffer_size = (mbit as i64 * 1_000_000 / fps.max(1) as i64) as i32;
        }
    }

    /// Ein Bild hinein. `vollbild` erzwingt ein IDR (forced-idr + pict_type I).
    pub fn senden(&mut self, frame: *mut AVFrame, pts_us: i64, vollbild: bool) -> Result<(), String> {
        unsafe {
            (*frame).pts = pts_us;
            (*frame).pict_type = if vollbild { AVPictureType::AV_PICTURE_TYPE_I } else { AVPictureType::AV_PICTURE_TYPE_NONE };
            let r = avcodec_send_frame(self.ctx, frame);
            if r < 0 {
                return Err(crate::ffmpeg_grund(&format!("{} nimmt Bild nicht an", self.name), r));
            }
        }
        Ok(())
    }

    /// Alle fertigen Pakete abholen (leer, wenn der Encoder noch arbeitet).
    pub fn empfangen(&mut self) -> Result<Vec<Paket>, String> {
        let mut aus = Vec::new();
        unsafe {
            loop {
                let r = avcodec_receive_packet(self.ctx, self.pkt);
                if r == AVERROR(EAGAIN) || r == AVERROR_EOF {
                    break;
                }
                if r < 0 {
                    return Err(crate::ffmpeg_grund(&format!("{} liefert kein Paket", self.name), r));
                }
                let d = std::slice::from_raw_parts((*self.pkt).data, (*self.pkt).size as usize).to_vec();
                let key = ((*self.pkt).flags & AV_PKT_FLAG_KEY) != 0;
                aus.push(Paket { data: d, key, pts: (*self.pkt).pts });
                av_packet_unref(self.pkt);
            }
        }
        Ok(aus)
    }

    /// Sitzung leeren: alles, was noch drin ist, kommt heraus. Danach nimmt
    /// sie nichts mehr an.
    pub fn leeren(&mut self) -> Vec<Paket> {
        unsafe {
            avcodec_send_frame(self.ctx, std::ptr::null());
        }
        self.empfangen().unwrap_or_default()
    }
}

impl Drop for Sitzung {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&mut self.ctx);
            av_packet_free(&mut self.pkt);
        }
    }
}

// --------------------------------------------------------------- Bilder

/// Ein AVFrame im Systemspeicher, wiederverwendbar.
pub struct Bild {
    pub frame: *mut AVFrame,
}

unsafe impl Send for Bild {}

impl Bild {
    pub fn neu(pix_fmt: AVPixelFormat, w: i32, h: i32) -> Result<Bild, String> {
        unsafe {
            let f = av_frame_alloc();
            if f.is_null() {
                return Err("kein AVFrame".into());
            }
            (*f).format = pix_fmt as i32;
            (*f).width = w;
            (*f).height = h;
            let r = av_frame_get_buffer(f, 0);
            if r < 0 {
                let mut f = f;
                av_frame_free(&mut f);
                return Err(crate::ffmpeg_grund("av_frame_get_buffer", r));
            }
            Ok(Bild { frame: f })
        }
    }

    /// Ein leerer Rahmen fuer Texturen aus einem hw_frames-Pool.
    pub fn leer() -> Bild {
        Bild { frame: unsafe { av_frame_alloc() } }
    }

    /// Vor dem Befuellen: der Encoder darf den alten Puffer noch halten.
    pub fn schreibbar(&mut self) -> Result<(), String> {
        let r = unsafe { av_frame_make_writable(self.frame) };
        if r < 0 { Err(crate::ffmpeg_grund("av_frame_make_writable", r)) } else { Ok(()) }
    }

    pub fn format(&self) -> AVPixelFormat {
        unsafe { std::mem::transmute((*self.frame).format) }
    }

    /// Ebene `i` zeilenweise fuellen (bytes_je_zeile aus der Quelle, Hoehe
    /// zeilen). Vorher geprueft: die Quelle reicht, die Zeile passt in den
    /// Zeilenabstand, die Ebene hat so viele Zeilen - sonst ein Fehler statt
    /// eines Zugriffs hinter das Ende.
    pub fn ebene_fuellen(&mut self, i: usize, quelle: &[u8], bytes_je_zeile: usize, zeilen: usize) -> Result<(), String> {
        let (hat, ls) = unsafe {
            let f = self.frame;
            if i >= 4 || !(*f).hw_frames_ctx.is_null() || (*f).data[i].is_null() {
                (0, 0)
            } else {
                (ebene_zeilen(self.format(), i, (*f).height.max(0) as usize), (*f).linesize[i].max(0) as usize)
            }
        };
        ebene_pruefen(i, quelle.len(), bytes_je_zeile, zeilen, ls, hat)?;
        unsafe {
            let dst = (*self.frame).data[i];
            for y in 0..zeilen {
                std::ptr::copy_nonoverlapping(quelle.as_ptr().add(y * bytes_je_zeile), dst.add(y * ls), bytes_je_zeile);
            }
        }
        Ok(())
    }

    /// Ebene `i` als Zeilen (Schrittweite = linesize), fuer die parallele Umrechnung.
    fn ebene_mut(&mut self, i: usize, zeilen: usize) -> (&mut [u8], usize) {
        unsafe {
            let ls = (*self.frame).linesize[i] as usize;
            (std::slice::from_raw_parts_mut((*self.frame).data[i], ls * zeilen), ls)
        }
    }

    /// BGRA (w*4 Byte je Zeile) in dieses Bild bringen - in seinem Format:
    /// Kopie fuer BGRA, sonst Umrechnung BT.709 voller Wertebereich (die
    /// Umkehrung von zeile_rgb im Client), zeilenparallel. 10-Bit-Formate
    /// legen den 8-Bit-Wert oben buendig ab (v << 8), wie das Testbild.
    /// Passt die Quelle nicht (zu kurz, groesser als der Rahmen), kommt ein
    /// Fehler - vor jedem Zugriff.
    pub fn aus_bgra(&mut self, src: &[u8], w: usize, h: usize) -> Result<(), String> {
        let (fw, fh, hw) = unsafe { ((*self.frame).width.max(0) as usize, (*self.frame).height.max(0) as usize, !(*self.frame).hw_frames_ctx.is_null()) };
        if hw {
            return Err("Umrechnung in eine Textur nicht vorgesehen".into());
        }
        bgra_pruefen(src.len(), w, h, fw, fh)?;
        let zeile = w * 4;
        self.schreibbar()?;
        match self.format() {
            AVPixelFormat::AV_PIX_FMT_BGRA => {
                self.ebene_fuellen(0, src, zeile, h)?;
            }
            AVPixelFormat::AV_PIX_FMT_YUV444P | AVPixelFormat::AV_PIX_FMT_YUV444P16LE => {
                let sechzehn = self.format() == AVPixelFormat::AV_PIX_FMT_YUV444P16LE;
                let (ye, ly) = self.ebene_mut(0, h);
                let ye = ye as *mut [u8];
                let (ue, lu) = self.ebene_mut(1, h);
                let ue = ue as *mut [u8];
                let (ve, lv) = self.ebene_mut(2, h);
                // Drei getrennte Ebenen desselben Rahmens: die Zeiger ueberlappen nicht.
                let (ye, ue) = unsafe { (&mut *ye, &mut *ue) };
                ye.par_chunks_mut(ly).zip(ue.par_chunks_mut(lu)).zip(ve.par_chunks_mut(lv)).enumerate().for_each(|(row, ((yz, uz), vz))| {
                    let s = &src[row * zeile..row * zeile + zeile];
                    for x in 0..w {
                        let (y, cb, cr) = ycbcr(s[x * 4 + 2], s[x * 4 + 1], s[x * 4]);
                        if sechzehn {
                            yz[2 * x] = 0;
                            yz[2 * x + 1] = y;
                            uz[2 * x] = 0;
                            uz[2 * x + 1] = cb;
                            vz[2 * x] = 0;
                            vz[2 * x + 1] = cr;
                        } else {
                            yz[x] = y;
                            uz[x] = cb;
                            vz[x] = cr;
                        }
                    }
                });
            }
            AVPixelFormat::AV_PIX_FMT_NV12 | AVPixelFormat::AV_PIX_FMT_P010LE => {
                let sechzehn = self.format() == AVPixelFormat::AV_PIX_FMT_P010LE;
                let (ye, ly) = self.ebene_mut(0, h);
                let ye = ye as *mut [u8];
                let (uv, luv) = self.ebene_mut(1, h / 2);
                let ye = unsafe { &mut *ye };
                ye.par_chunks_mut(ly).enumerate().for_each(|(row, yz)| {
                    let s = &src[row * zeile..row * zeile + zeile];
                    for x in 0..w {
                        let (y, _, _) = ycbcr(s[x * 4 + 2], s[x * 4 + 1], s[x * 4]);
                        if sechzehn {
                            yz[2 * x] = 0;
                            yz[2 * x + 1] = y;
                        } else {
                            yz[x] = y;
                        }
                    }
                });
                // Farbe in halber Aufloesung: der Punkt oben links je Vierergruppe,
                // wie das Testbild (nv12) es tut.
                uv.par_chunks_mut(luv).enumerate().for_each(|(row, uz)| {
                    let s = &src[2 * row * zeile..2 * row * zeile + zeile];
                    for x in 0..w / 2 {
                        let (_, cb, cr) = ycbcr(s[8 * x + 2], s[8 * x + 1], s[8 * x]);
                        if sechzehn {
                            uz[4 * x] = 0;
                            uz[4 * x + 1] = cb;
                            uz[4 * x + 2] = 0;
                            uz[4 * x + 3] = cr;
                        } else {
                            uz[2 * x] = cb;
                            uz[2 * x + 1] = cr;
                        }
                    }
                });
            }
            f => return Err(format!("Umrechnung nach {} nicht vorgesehen", pix_fmt_name(f))),
        }
        Ok(())
    }

    /// Ebene `i` zeilenparallel fuellen - wie ebene_fuellen, mit denselben
    /// Pruefungen vorher (die PQ-Ebenen sind bis 6 Byte je Punkt gross).
    fn ebene_parallel_fuellen(&mut self, i: usize, quelle: &[u8], bytes_je_zeile: usize, zeilen: usize) -> Result<(), String> {
        let (hat, ls) = unsafe {
            let f = self.frame;
            if i >= 4 || !(*f).hw_frames_ctx.is_null() || (*f).data[i].is_null() {
                (0, 0)
            } else {
                (ebene_zeilen(self.format(), i, (*f).height.max(0) as usize), (*f).linesize[i].max(0) as usize)
            }
        };
        ebene_pruefen(i, quelle.len(), bytes_je_zeile, zeilen, ls, hat)?;
        if zeilen == 0 || bytes_je_zeile == 0 {
            return Ok(());
        }
        let (ziel, ls) = self.ebene_mut(i, zeilen);
        ziel.par_chunks_mut(ls).zip(quelle.par_chunks(bytes_je_zeile)).for_each(|(z, q)| z[..bytes_je_zeile].copy_from_slice(q));
        Ok(())
    }

    /// Die PQ-Ebenen des Wandlers (u16 LE, 10 Bit oben buendig, dicht
    /// gepackt) in dieses Bild: YUV444P16LE Y, Cb, Cr je w x h; P010 Y (w x
    /// h) und die CbCr-Paare (w/2 x h/2). w x h muss die Groesse des Bildes
    /// sein; eine zu kurze Quelle ist ein Fehler vor jedem Zugriff.
    pub fn aus_ebenen16(&mut self, daten: &[u8], w: usize, h: usize) -> Result<(), String> {
        let (fw, fh, hw) = unsafe { ((*self.frame).width.max(0) as usize, (*self.frame).height.max(0) as usize, !(*self.frame).hw_frames_ctx.is_null()) };
        if hw {
            return Err("Ebenen in eine Textur nicht vorgesehen".into());
        }
        if (w, h) != (fw, fh) || w == 0 || h == 0 {
            return Err(format!("Ebenen {w}x{h} passen nicht zum Rahmen {fw}x{fh}"));
        }
        let noetig = ebenen16_bytes(self.format(), w, h).ok_or_else(|| format!("Ebenen in {} nicht vorgesehen", pix_fmt_name(self.format())))?;
        if daten.len() < noetig {
            return Err(format!("Ebenen zu kurz: {} statt {noetig} Byte ({w}x{h} {})", daten.len(), pix_fmt_name(self.format())));
        }
        self.schreibbar()?;
        let n = w * h * 2;
        if self.format() == AVPixelFormat::AV_PIX_FMT_YUV444P16LE {
            for i in 0..3 {
                self.ebene_parallel_fuellen(i, &daten[i * n..(i + 1) * n], 2 * w, h)?;
            }
        } else {
            self.ebene_parallel_fuellen(0, &daten[..n], 2 * w, h)?;
            self.ebene_parallel_fuellen(1, &daten[n..noetig], 2 * w, h / 2)?;
        }
        Ok(())
    }
}

/// Bytes der dicht gepackten PQ-Ebenen fuer ein Format (None: kein
/// Ebenenformat des HDR10-Wegs). P010 braucht gerade Groessen.
fn ebenen16_bytes(f: AVPixelFormat, w: usize, h: usize) -> Option<usize> {
    match f {
        AVPixelFormat::AV_PIX_FMT_YUV444P16LE => Some(w * h * 6),
        AVPixelFormat::AV_PIX_FMT_P010LE if w % 2 == 0 && h % 2 == 0 => Some(w * h * 3),
        _ => None,
    }
}

impl Drop for Bild {
    fn drop(&mut self) {
        unsafe { av_frame_free(&mut self.frame) };
    }
}

/// Zeilen der Ebene `i` eines Rahmens im Format `f` mit der Hoehe h, fuer
/// die Formate, die der Host fuellt (0 = keine solche Ebene): die
/// Farbebene von NV12/P010 hat die halbe Hoehe, aufgerundet wie bei
/// av_frame_get_buffer.
fn ebene_zeilen(f: AVPixelFormat, i: usize, h: usize) -> usize {
    use AVPixelFormat::*;
    match (f, i) {
        (AV_PIX_FMT_BGRA, 0) => h,
        (AV_PIX_FMT_YUV444P | AV_PIX_FMT_YUV444P16LE, 0..=2) => h,
        (AV_PIX_FMT_NV12 | AV_PIX_FMT_P010LE, 0) => h,
        (AV_PIX_FMT_NV12 | AV_PIX_FMT_P010LE, 1) => h.div_ceil(2),
        _ => 0,
    }
}

/// Vor jeder Kopie in eine Ebene: die Zeile passt in den Zeilenabstand, die
/// Ebene hat so viele Zeilen, die Quelle reicht.
fn ebene_pruefen(i: usize, quelle: usize, bytes_je_zeile: usize, zeilen: usize, abstand: usize, hat: usize) -> Result<(), String> {
    if bytes_je_zeile > abstand || zeilen > hat {
        return Err(format!("Ebene {i}: {zeilen} Zeilen zu {bytes_je_zeile} Byte passen nicht in den Rahmen ({hat} Zeilen zu {abstand} Byte)"));
    }
    if quelle < bytes_je_zeile * zeilen {
        return Err(format!("Ebene {i}: Quelle zu kurz: {quelle} statt {} Byte", bytes_je_zeile * zeilen));
    }
    Ok(())
}

/// Vor der Umrechnung aus BGRA: w x h passt in den Rahmen fw x fh, und die
/// Quelle reicht fuer w x h - im Pruefbericht kam bei 150 % Skalierung ein
/// halb so grosses Bild fuer einen 1280x720-Strom an.
fn bgra_pruefen(quelle: usize, w: usize, h: usize, fw: usize, fh: usize) -> Result<(), String> {
    if w == 0 || h == 0 || w > fw || h > fh {
        return Err(format!("Bild {w}x{h} passt nicht in den Rahmen {fw}x{fh}"));
    }
    if quelle < w * h * 4 {
        return Err(format!("Quelle zu kurz: {quelle} statt {} Byte ({w}x{h} BGRA)", w * h * 4));
    }
    Ok(())
}

/// RGB -> Y/Cb/Cr, BT.709, voller Wertebereich, ganzzahlig (Koeffizienten
/// mal 256: 54/183/19 fuer Y, 138 = 256/1.8556, 163 = 256/1.5748).
#[inline]
fn ycbcr(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = (54 * r + 183 * g + 19 * b + 128) >> 8;
    let cb = (((b - y) * 138 + 128) >> 8) + 128;
    let cr = (((r - y) * 163 + 128) >> 8) + 128;
    (y.clamp(0, 255) as u8, cb.clamp(0, 255) as u8, cr.clamp(0, 255) as u8)
}

/// BGRA -> yuv444p in drei getrennte Puffer (fuer die Messung D2).
pub fn bgra_nach_yuv444(src: &[u8], w: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let zeile = w * 4;
    y.par_chunks_mut(w).zip(u.par_chunks_mut(w)).zip(v.par_chunks_mut(w)).enumerate().for_each(|(row, ((yz, uz), vz))| {
        let s = &src[row * zeile..row * zeile + zeile];
        for x in 0..w {
            let (a, b, c) = ycbcr(s[x * 4 + 2], s[x * 4 + 1], s[x * 4]);
            yz[x] = a;
            uz[x] = b;
            vz[x] = c;
        }
    });
}

// ---------------------------------------------- FFmpeg-Geraet aus D3D11

/// AVHWDeviceContext (D3D11VA) aus einem VORHANDENEN Geraet: FFmpeg legt
/// keines an, sondern nimmt unseres (mit zusaetzlicher Referenz).
pub fn hw_geraet(device: &ID3D11Device) -> Result<*mut AVBufferRef, String> {
    unsafe {
        let r = av_hwdevice_ctx_alloc(AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
        if r.is_null() {
            return Err("av_hwdevice_ctx_alloc".into());
        }
        let ctx = (*r).data as *mut AVHWDeviceContext;
        let d3d = (*ctx).hwctx as *mut AVD3D11VADeviceContext;
        (*d3d).device = super::aufnahme::geraet_roh(device) as *mut _;
        let e = av_hwdevice_ctx_init(r);
        if e < 0 {
            let mut r = r;
            av_buffer_unref(&mut r);
            return Err(crate::ffmpeg_grund("av_hwdevice_ctx_init", e));
        }
        Ok(r)
    }
}

/// Texturpool fuer den Null-Kopien-Weg: Format D3D11, sw_format BGRA,
/// RENDER_TARGET, sechs Texturen (bis zu drei beim Encoder, das zuletzt
/// gegebene Bild fuer die Wiederholung, das Testbild und eine Reserve).
pub fn hw_pool(geraet: *mut AVBufferRef, w: i32, h: i32) -> Result<*mut AVBufferRef, String> {
    unsafe {
        let r = av_hwframe_ctx_alloc(geraet);
        if r.is_null() {
            return Err("av_hwframe_ctx_alloc".into());
        }
        let fc = (*r).data as *mut AVHWFramesContext;
        (*fc).format = AVPixelFormat::AV_PIX_FMT_D3D11;
        (*fc).sw_format = AVPixelFormat::AV_PIX_FMT_BGRA;
        (*fc).width = w;
        (*fc).height = h;
        (*fc).initial_pool_size = 6;
        let d3d = (*fc).hwctx as *mut AVD3D11VAFramesContext;
        (*d3d).BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
        let e = av_hwframe_ctx_init(r);
        if e < 0 {
            let mut r = r;
            av_buffer_unref(&mut r);
            return Err(crate::ffmpeg_grund("av_hwframe_ctx_init", e));
        }
        Ok(r)
    }
}

/// Eine Textur aus dem Pool in `bild` holen; liefert Textur und Index.
pub fn pool_textur(pool: *mut AVBufferRef, bild: &mut Bild) -> Result<(ID3D11Texture2D, u32), String> {
    if bild.frame.is_null() || pool.is_null() {
        return Err("Pooltextur: kein Rahmen oder kein Pool".into());
    }
    unsafe {
        av_frame_unref(bild.frame);
        let e = av_hwframe_get_buffer(pool, bild.frame, 0);
        if e < 0 {
            return Err(crate::ffmpeg_grund("av_hwframe_get_buffer", e));
        }
        let roh = (*bild.frame).data[0] as *mut std::ffi::c_void;
        let idx = (*bild.frame).data[1] as usize as u32;
        let t = ID3D11Texture2D::from_raw_borrowed(&roh).ok_or("Pooltextur fehlt")?.clone();
        Ok((t, idx))
    }
}

/// Ein Bild im Systemspeicher (BGRA in Poolgroesse, das Testbild) in eine
/// Textur des Pools laden; `rahmen` haelt danach diese Textur. Eine
/// Sitzung mit hw_frames_ctx nimmt nur Texturen - nvenc liest bei ihr
/// frame->hw_frames_ctx, ohne zu pruefen, ob es ihn gibt.
pub fn pool_hochladen(pool: *mut AVBufferRef, ctx: &ID3D11DeviceContext, quelle: &Bild, rahmen: &mut Bild) -> Result<(), String> {
    let (w, h) = unsafe {
        let fc = (*pool).data as *const AVHWFramesContext;
        ((*fc).width, (*fc).height)
    };
    if !rahmen_passt(quelle.frame, None, AVPixelFormat::AV_PIX_FMT_BGRA, w, h) {
        return Err(format!("Bild fuer den Pool ({w}x{h} BGRA) passt nicht"));
    }
    let (t, idx) = pool_textur(pool, rahmen)?;
    unsafe {
        ctx.UpdateSubresource(&t, idx, None, (*quelle.frame).data[0] as *const _, (*quelle.frame).linesize[0] as u32, 0);
    }
    Ok(())
}

/// Passt ein Rahmen zur Sitzung? Mit Pool nur Texturen aus genau diesem
/// Pool, ohne Pool nur Systemspeicher im Eingabeformat - immer in der
/// Groesse der Sitzung. Was nicht passt, erreicht avcodec_send_frame nie.
fn rahmen_passt(frame: *const AVFrame, pool: Option<*mut AVBufferRef>, pix_fmt: AVPixelFormat, w: i32, h: i32) -> bool {
    unsafe {
        if frame.is_null() || (*frame).data[0].is_null() || (*frame).width != w || (*frame).height != h {
            return false;
        }
        let hw = (*frame).hw_frames_ctx;
        match pool {
            Some(p) => !hw.is_null() && !p.is_null() && (*hw).data == (*p).data && (*frame).format == AVPixelFormat::AV_PIX_FMT_D3D11 as i32,
            None => hw.is_null() && (*frame).format == pix_fmt as i32,
        }
    }
}

// ---------------------------------------------------------------- Betrieb

/// Woher das Bild fuer den Encoder kommt.
pub enum Quelle<'a> {
    /// BGRA im Systemspeicher (Staging-Kopie der Duplication).
    Ram(&'a [u8]),
    /// Eine Textur auf dem Geraet der Duplication (Null-Kopien-Weg).
    Textur(&'a ID3D11Texture2D),
    /// Die PQ-Ebenen des Wandlers (HDR10: YUV444P16LE bzw. P010, u16 dicht
    /// gepackt, Stromgroesse).
    Ebenen16(&'a [u8]),
    /// Ein fertiges Bild im Eingabeformat der Sitzung (Testbild).
    Fertig(&'a mut Bild),
    /// Das zuletzt gegebene Bild noch einmal (feste Bildrate).
    Wiederholung,
    /// Wie Wiederholung, aber nur, damit ein Encoder mit Vorlauf die echten
    /// Bilder herausgibt (takt::vorlauf_nachschieben): zaehlt selbst nicht
    /// als ausstehend - ausser es wird ein erzwungenes Vollbild.
    Nachschub,
}

/// Antwort von codieren auf eine Wiederholung, bevor diese Sitzung je ein
/// Bild hatte (frisch geoeffnet): nichts gesendet, nichts zu zaehlen.
pub const KEIN_VORBILD: &str = "kein Bild fuer die Wiederholung";

/// Geraet und Texturpool fuer den Null-Kopien-Weg. Wird als Feld NACH
/// Sitzung und Bild abgebaut - die halten Referenzen auf den Pool.
struct Pool {
    geraet: *mut AVBufferRef,
    pool: *mut AVBufferRef,
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            av_buffer_unref(&mut self.pool);
            av_buffer_unref(&mut self.geraet);
        }
    }
}

/// Die laufende Encoder-Sitzung mit Eingabebild, Pool und Buchfuehrung -
/// die Rolle von g_session auf dem Mac. Lebt nur auf dem Aufnahmefaden.
pub struct Betrieb {
    pub idx: usize,
    sitzung: Sitzung,
    /// Eingabebild (Systemspeicher) bzw. leerer Rahmen fuer den Pool.
    bild: Bild,
    /// Leerer Rahmen fuer das Testbild im Pool - `bild` behaelt derweil das
    /// letzte Desktopbild fuer die Wiederholung.
    tb_rahmen: Bild,
    pool: Option<Pool>,
    /// Fuer Texturquellen: der Kontext, der in den Pool kopiert.
    ctx: Option<ID3D11DeviceContext>,
    pub pix_fmt: AVPixelFormat,
    pub w: i32,
    pub h: i32,
    /// SDR oder HDR10 - bleibt bei einem Neustart (Einstellungen) erhalten.
    farbe: StromFarbe,
    fps: i32,
    mbit: i32,
    gaming: bool,
    /// Bilder seit dem letzten IDR (der Host verwaltet die GOP selbst).
    seit_idr: i32,
    /// Bilder, deren Paket noch aussteht: pts -> (Aufnahmezeit, nachgelegt,
    /// echt). Echt = neuer Inhalt oder erzwungenes Vollbild - das muss beim
    /// Zuschauer ankommen; ein Nachschub fuer den Vorlauf muss es nicht.
    unterwegs: VecDeque<(i64, u64, bool, bool)>,
    /// Was der Encoder bauartbedingt zurueckhaelt - zaehlt nicht als Stau.
    vorlauf: usize,
    /// Ob schon ein Bild gegeben wurde (fuer Wiederholung ohne Vorbild).
    hat_bild: bool,
    fehler_gemeldet: bool,
}

impl Betrieb {
    /// Sitzung fuer einen Kandidaten oeffnen: Encoder und Eingabeformat aus
    /// Befund und Weg; fuer d3d11 der Pool auf dem Geraet der Duplication.
    /// In PQ nur Kandidaten mit HDR10 im Befund (0 und 2 auf nvenc) - die
    /// nehmen immer den Systemspeicher (die Ebenen des Wandlers).
    pub fn oeffnen(idx: usize, w: i32, h: i32, weg: Weg, geraet: Option<(&ID3D11Device, &ID3D11DeviceContext)>, farbe: StromFarbe) -> Result<Betrieb, String> {
        let k = kandidat(idx);
        let b = befund(idx);
        if !b.vorhanden {
            return Err(format!("{} ist auf diesem Rechner nicht vorhanden", k.name));
        }
        if farbe.ist_pq() && !b.hdr {
            return Err(format!("{} kann auf diesem Rechner kein HDR10", k.name));
        }
        let fps = Z.fps.load(Ordering::Relaxed) as i32;
        let mbit = Z.mbit.load(Ordering::Relaxed) as i32;
        let gaming = Z.gaming.load(Ordering::Relaxed);
        let (pix_fmt, texturen) = eingabeformat(idx, weg, b.encoder);
        let mut pool: Option<Pool> = None;
        let mut ctx = None;
        if texturen {
            let Some((dev, dc)) = geraet else { return Err("Null-Kopien-Weg ohne Geraet".into()) };
            let g = hw_geraet(dev)?;
            let p = match hw_pool(g, w, h) {
                Ok(p) => p,
                Err(e) => {
                    unsafe {
                        let mut g = g;
                        av_buffer_unref(&mut g);
                    }
                    return Err(e);
                }
            };
            pool = Some(Pool { geraet: g, pool: p });
            ctx = Some(dc.clone());
        }
        crate::protokoll::fehler_verwerfen();
        let o = Oeffnung {
            encoder: b.encoder,
            pix_fmt,
            profil: if b.encoder == MF_H264 { "" } else { k.profil },
            fps,
            mbit,
            delay: 0,
            hw_frames: pool.as_ref().map(|p| p.pool),
            rgb_444: k.chroma444,
            farbe,
            ..Oeffnung::vorgabe(w, h)
        };
        // Scheitert das Oeffnen, raeumt Drop von `pool` auf.
        let sitzung = Sitzung::oeffnen(&o)?;
        let bild = if texturen { Bild::leer() } else { Bild::neu(pix_fmt, w, h)? };
        log(format!(
            "Encoder: Kandidat {idx} {}, {w}x{h}, {}, {mbit} Mbit/s, {fps} fps, Eingabe {}{}, {}",
            k.name,
            if b.hardware { format!("Hardware ({})", b.encoder) } else { format!("Software ({})", b.encoder) },
            if texturen {
                "d3d11/bgra ohne Kopie".to_string()
            } else if farbe.ist_pq() {
                format!("{} (PQ-Ebenen des Wandlers)", pix_fmt_name(pix_fmt))
            } else {
                pix_fmt_name(pix_fmt)
            },
            if umrechnung_fuer(idx, &b) && !farbe.ist_pq() { " (mit Umrechnung)" } else { "" },
            farbe.text()
        ) + &if b.vorlauf > 0 { format!(", Vorlauf {} Bilder", b.vorlauf) } else { String::new() });
        Ok(Betrieb {
            idx,
            sitzung,
            bild,
            tb_rahmen: Bild::leer(),
            pool,
            ctx,
            pix_fmt,
            w,
            h,
            farbe,
            fps,
            mbit,
            gaming,
            seit_idr: 0,
            unterwegs: VecDeque::new(),
            vorlauf: b.vorlauf,
            hat_bild: false,
            fehler_gemeldet: false,
        })
    }

    pub fn texturen(&self) -> bool {
        self.pool.is_some()
    }

    /// Die Farbe dieser Sitzung (SDR oder HDR10).
    pub fn farbe(&self) -> StromFarbe {
        self.farbe
    }

    /// Was die Aufnahme liefern muss: Some(4:4:4) fuer die PQ-Ebenen des
    /// Wandlers, None fuer den Bildweg BGRA (Systemspeicher oder Textur).
    pub fn pq_art(&self) -> Option<bool> {
        self.farbe.ist_pq().then(|| kandidat(self.idx).chroma444)
    }

    /// Hat diese Sitzung ein Bild, das sie wiederholen kann? Eine frisch
    /// geoeffnete hat keines - auch wenn ihre Vorgaengerin es hatte.
    pub fn hat_bild(&self) -> bool {
        self.hat_bild
    }

    /// Testbild aus: die Pooltextur, die `tb_rahmen` noch haelt, geht an
    /// den Pool zurueck (der Encoder haelt seine eigene Referenz, solange er
    /// sie braucht). Ohne Pool haelt `tb_rahmen` nichts.
    pub fn testbild_freigeben(&mut self) {
        if self.pool.is_some() {
            unsafe { av_frame_unref(self.tb_rahmen.frame) };
        }
    }

    /// Bilder, deren Paket noch aussteht - ohne den Vorlauf des Encoders.
    pub fn inflight(&self) -> usize {
        self.unterwegs.len().saturating_sub(self.vorlauf)
    }

    /// Bilder, die der Encoder bauartbedingt zurueckhaelt (0 bei nvenc).
    pub fn vorlauf(&self) -> usize {
        self.vorlauf
    }

    /// Echte Bilder (neuer Inhalt, erzwungenes Vollbild), deren Paket noch
    /// nicht heraus ist - mit Vorlauf stecken sie fest, bis weitere Bilder
    /// nachkommen.
    pub fn ausstehend(&self) -> usize {
        self.unterwegs.iter().filter(|u| u.3).count()
    }

    /// Was der Zuschauer eingestellt hat, auf die Sitzung uebertragen:
    /// Bitrate bei nvenc im Betrieb (reconfig), sonst und bei einer neuen
    /// Bildrate ein Neustart mit Vollbild. Liefert Ok(true), wenn die
    /// Sitzung neu ist.
    pub fn einstellungen_nachziehen(&mut self, weg: Weg, geraet: Option<(&ID3D11Device, &ID3D11DeviceContext)>) -> Result<bool, String> {
        let fps = Z.fps.load(Ordering::Relaxed) as i32;
        let mbit = Z.mbit.load(Ordering::Relaxed) as i32;
        self.gaming = Z.gaming.load(Ordering::Relaxed);
        if fps == self.fps && mbit == self.mbit {
            return Ok(false);
        }
        if fps == self.fps && self.sitzung.kann_umstellen() {
            self.sitzung.bitrate_setzen(mbit, fps);
            self.mbit = mbit;
            Z.force_key.store(true, Ordering::Relaxed);
            log(format!("Encoder: Bitrate im Betrieb umgestellt auf {mbit} Mbit/s (nvenc reconfig, Vollbild folgt)"));
            return Ok(false);
        }
        log(format!("Encoder: Neustart fuer {mbit} Mbit/s, {fps} fps ({})", if self.sitzung.kann_umstellen() { "Bildrate laesst sich nur so umstellen" } else { "dieser Encoder stellt nicht im Betrieb um" }));
        let neu = Betrieb::oeffnen(self.idx, self.w, self.h, weg, geraet, self.farbe)?;
        let alt = std::mem::replace(self, neu);
        alt.schliessen();
        Z.force_key.store(true, Ordering::Relaxed);
        Ok(true)
    }

    /// Ein Bild in den Encoder und die fertigen Pakete auf die Leitung.
    /// t_cap ist die ECHTE Aufnahmezeit (bei Wiederholungen die des
    /// wiederholten Bildes), pts die fuer den Encoder aufsteigende Zeit.
    pub fn codieren(&mut self, quelle: Quelle, t_cap_us: u64, pts_us: i64, wiederholt: bool) -> Result<(), String> {
        // Erst der Rahmen, dann das Vollbild: kehrt codieren vorher zurueck,
        // bleibt force_key fuer das naechste Bild stehen. Eine Wiederholung
        // ohne Vorbild ist kein gesendetes Bild (und nichts fuers Protokoll).
        let nachschub = matches!(quelle, Quelle::Nachschub);
        let frame = match quelle {
            Quelle::Wiederholung | Quelle::Nachschub if !self.hat_bild => return Err(KEIN_VORBILD.into()),
            Quelle::Wiederholung | Quelle::Nachschub => self.bild.frame,
            q => match self.rahmen(q) {
                Ok(f) => f,
                Err(e) => return Err(self.verworfen(e)),
            },
        };
        if !rahmen_passt(frame, self.pool.as_ref().map(|p| p.pool), self.pix_fmt, self.w, self.h) {
            return Err(self.verworfen("Bild passt nicht zur Sitzung (Systemspeicher/Textur, Format oder Groesse)".into()));
        }
        let erzwungen = Z.force_key.swap(false, Ordering::Relaxed);
        let vollbild = erzwungen || self.seit_idr >= if self.gaming { self.fps } else { self.fps * 2 };
        if let Err(e) = self.sitzung.senden(frame, pts_us, vollbild) {
            if vollbild {
                Z.force_key.store(true, Ordering::Relaxed);
            }
            return Err(self.verworfen(e));
        }
        self.seit_idr = if vollbild { 1 } else { self.seit_idr + 1 };
        // Ein erzwungenes Vollbild muss heraus, auch als Nachschub - sonst
        // steckte die Anforderung (neuer Zuschauer, Stau) im Vorlauf fest.
        self.unterwegs.push_back((pts_us, t_cap_us, wiederholt, !nachschub || erzwungen));
        self.pakete_abholen()
    }

    /// Den Rahmen fuer die Sitzung bereitstellen, in dem, was sie nimmt:
    /// Systemspeicher im Eingabeformat oder eine Textur aus ihrem Pool.
    fn rahmen(&mut self, quelle: Quelle) -> Result<*mut AVFrame, String> {
        Ok(match quelle {
            Quelle::Ram(bgra) => {
                if self.farbe.ist_pq() {
                    // SDR-Werte in einem PQ-Strom waeren falsche Farben.
                    return Err("BGRA an eine HDR10-Sitzung (die Aufnahme ist noch nicht im Modus PQ)".into());
                }
                self.hat_bild = false;
                if let Some(p) = self.pool.as_ref() {
                    let noetig = (self.w * self.h * 4) as usize;
                    if bgra.len() < noetig {
                        return Err(format!("Quelle zu kurz: {} statt {noetig} Byte ({}x{} BGRA)", bgra.len(), self.w, self.h));
                    }
                    let (t, idx) = pool_textur(p.pool, &mut self.bild)?;
                    unsafe {
                        self.ctx.as_ref().unwrap().UpdateSubresource(&t, idx, None, bgra.as_ptr() as *const _, (self.w * 4) as u32, 0);
                    }
                } else {
                    self.bild.aus_bgra(bgra, self.w as usize, self.h as usize)?;
                }
                self.hat_bild = true;
                self.bild.frame
            }
            Quelle::Ebenen16(ebenen) => {
                if self.pool.is_some() || !self.farbe.ist_pq() {
                    return Err("PQ-Ebenen an eine Sitzung ohne HDR10".into());
                }
                self.hat_bild = false;
                self.bild.aus_ebenen16(ebenen, self.w as usize, self.h as usize)?;
                self.hat_bild = true;
                self.bild.frame
            }
            Quelle::Textur(tex) => {
                let Some(p) = self.pool.as_ref() else { return Err("Textur ohne Pool".into()) };
                let mut d = D3D11_TEXTURE2D_DESC::default();
                unsafe { tex.GetDesc(&mut d) };
                if (d.Width as i32) < self.w || (d.Height as i32) < self.h {
                    return Err(format!("Textur {}x{} kleiner als der Strom {}x{}", d.Width, d.Height, self.w, self.h));
                }
                self.hat_bild = false;
                let (t, idx) = pool_textur(p.pool, &mut self.bild)?;
                // Nur der Ausschnitt in Stromgroesse: ein ungerader Rand faellt weg.
                let kasten = D3D11_BOX { left: 0, top: 0, front: 0, right: self.w as u32, bottom: self.h as u32, back: 1 };
                unsafe {
                    self.ctx.as_ref().unwrap().CopySubresourceRegion(&t, idx, 0, 0, 0, tex, 0, Some(&kasten));
                }
                self.hat_bild = true;
                self.bild.frame
            }
            // Das Testbild liegt immer im Systemspeicher; eine Sitzung mit
            // Pool bekommt es als Textur, `bild` bleibt das letzte Desktopbild.
            Quelle::Fertig(b) => match self.pool.as_ref() {
                Some(p) => {
                    pool_hochladen(p.pool, self.ctx.as_ref().unwrap(), b, &mut self.tb_rahmen)?;
                    self.tb_rahmen.frame
                }
                None => b.frame,
            },
            Quelle::Wiederholung | Quelle::Nachschub => self.bild.frame,
        })
    }

    /// Ein Bild, das nicht in den Encoder kam: einmal ins Protokoll, es
    /// zaehlt als verworfen.
    fn verworfen(&mut self, e: String) -> String {
        if !self.fehler_gemeldet {
            self.fehler_gemeldet = true;
            log(format!("Encoder {}: {e}", kandidat(self.idx).name));
        }
        Z.enc_verworfen.fetch_add(1, Ordering::Relaxed);
        e
    }

    /// Fertige Pakete an den Zuschauer, jedes mit dem Stempel seines Bildes.
    pub fn pakete_abholen(&mut self) -> Result<(), String> {
        let pakete = match self.sitzung.empfangen() {
            Ok(p) => p,
            Err(e) => {
                if !self.fehler_gemeldet {
                    self.fehler_gemeldet = true;
                    log(format!("Encoder {}: {e}", kandidat(self.idx).name));
                }
                return Err(e);
            }
        };
        for p in pakete {
            self.paket_senden(&p);
        }
        Ok(())
    }

    fn paket_senden(&mut self, p: &Paket) {
        // Das Paket gehoert zum Bild mit derselben pts; aeltere ohne Paket
        // hat der Encoder verworfen.
        let mut zettel = None;
        while let Some(u) = self.unterwegs.pop_front() {
            if u.0 == p.pts {
                zettel = Some(u);
                break;
            }
            if u.0 > p.pts {
                self.unterwegs.push_front(u);
                break;
            }
            Z.enc_verworfen.fetch_add(1, Ordering::Relaxed);
        }
        let (t_cap, wiederholt) = zettel.map(|z| (z.1, z.2)).unwrap_or((super::now_us(), false));
        netz::bild_senden(&p.data, p.key, t_cap, wiederholt);
    }

    /// Sitzung leeren (die letzten Pakete gehen noch raus) und abbauen.
    /// Drop raeumt in Feldreihenfolge auf: Sitzung, Bild, dann der Pool.
    pub fn schliessen(mut self) {
        let rest = self.sitzung.leeren();
        for p in rest {
            self.paket_senden(&p);
        }
    }
}

/// Die zwoelf Testbilder im Eingabeformat der Sitzung, vorgerechnet - so
/// wie qc_testbild_start auf dem Mac sie im Aufnahmeformat anlegt. Immer im
/// Systemspeicher: fuer BGRA-Sitzungen und den Null-Kopien-Weg (dessen
/// pix_fmt ist BGRA; codieren laedt das Bild in eine Textur des Pools) aus
/// dem BGRA-Testbild, sonst direkt aus den Y/Cb/Cr-Ganzzahlen (dieselben
/// Werte wie auf dem Mac). Eine HDR10-Sitzung bekommt dieselben Bilder in PQ
/// (testbilder_pq) - sonst saehe der Zuschauer SDR-Werte als PQ gedeutet.
pub fn testbilder(pix_fmt: AVPixelFormat, w: i32, h: i32, farbe: StromFarbe) -> Result<Vec<Bild>, String> {
    use super::testbild;
    if farbe.ist_pq() {
        let weiss = Z.hdr_weiss_nit.load(Ordering::Relaxed);
        return testbilder_pq(pix_fmt, w, h, if weiss == 0 { crate::hdr::SDR_WEISS_VORGABE_NIT as f64 } else { weiss as f64 });
    }
    let mut aus = Vec::with_capacity(testbild::N);
    for k in 0..testbild::N {
        let mut b = Bild::neu(pix_fmt, w, h)?;
        match b.format() {
            AVPixelFormat::AV_PIX_FMT_BGRA => b.ebene_fuellen(0, &testbild::bgra(w, h, k), (w * 4) as usize, h as usize)?,
            AVPixelFormat::AV_PIX_FMT_YUV444P => {
                let [y, u, v] = testbild::yuv444p(w, h, k);
                b.ebene_fuellen(0, &y, w as usize, h as usize)?;
                b.ebene_fuellen(1, &u, w as usize, h as usize)?;
                b.ebene_fuellen(2, &v, w as usize, h as usize)?;
            }
            AVPixelFormat::AV_PIX_FMT_YUV444P16LE => {
                let [y, u, v] = testbild::yuv444p16(w, h, k);
                b.ebene_fuellen(0, &y, 2 * w as usize, h as usize)?;
                b.ebene_fuellen(1, &u, 2 * w as usize, h as usize)?;
                b.ebene_fuellen(2, &v, 2 * w as usize, h as usize)?;
            }
            AVPixelFormat::AV_PIX_FMT_NV12 => {
                let [y, uv] = testbild::nv12(w, h, k);
                b.ebene_fuellen(0, &y, w as usize, h as usize)?;
                b.ebene_fuellen(1, &uv, w as usize, (h / 2) as usize)?;
            }
            AVPixelFormat::AV_PIX_FMT_P010LE => {
                let [y, uv] = testbild::nv12(w, h, k);
                let f = |p: Vec<u8>| -> Vec<u8> { p.iter().flat_map(|&b| [0u8, b]).collect() };
                b.ebene_fuellen(0, &f(y), 2 * w as usize, h as usize)?;
                b.ebene_fuellen(1, &f(uv), 2 * w as usize, (h / 2) as usize)?;
            }
            f => return Err(format!("Testbild in {} nicht vorgesehen", pix_fmt_name(f))),
        }
        aus.push(b);
    }
    Ok(aus)
}

/// Ein Testbildpunkt (Y/Cb/Cr BT.709 voll, 8 Bit, wie testbild::punkt) als
/// HDR10-Codes (10 Bit, voll, nicht verschoben): SDR-Inhalt mit dem SDR-Weiss
/// `weiss_nit` - R'G'B' (sRGB) -> linear * Weiss -> BT.2020 -> PQ -> Y'CbCr
/// BT.2020-NCL. So bildet der Client ihn auf SDR genau zurueck (hdr.rs).
pub fn pq_codes_aus_709(p: (u8, u8, u8), weiss_nit: f64) -> [u16; 3] {
    use crate::hdr;
    let y = p.0 as f64 / 255.0;
    let cb = (p.1 as f64 - 128.0) / 255.0;
    let cr = (p.2 as f64 - 128.0) / 255.0;
    // BT.709: R = Y + 2 (1 - Kr) Cr, G = Y - 2 Kb (1 - Kb) / Kg Cb - 2 Kr (1 - Kr) / Kg Cr, B = Y + 2 (1 - Kb) Cb.
    let rgb = [y + 1.5748 * cr, y - 0.187_324 * cb - 0.468_124 * cr, y + 1.8556 * cb].map(|v| v.clamp(0.0, 1.0));
    let lin = rgb.map(|v| hdr::srgb_eotf(v) as f32);
    let l2020 = hdr::mal(&hdr::M_709_NACH_2020, lin).map(|v| v.max(0.0));
    let e = l2020.map(|v| hdr::pq_oetf(v as f64 * weiss_nit) as f32);
    let c = hdr::rgb_nach_ycbcr_2020(e);
    let code = |v: f32, versatz: f32| (v * 1023.0 + versatz).round().clamp(0.0, 1023.0) as u16;
    [code(c[0], 0.0), code(c[1], 512.0), code(c[2], 512.0)]
}

/// Die zwoelf Testbilder in PQ (YUV444P16LE oder P010): testbild::punkt als
/// SDR-Inhalt mit diesem SDR-Weiss, 10 Bit oben buendig. Die Farben des
/// Testbilds sind wenige - die Codes kommen aus einer Tafel (gefuellt aus
/// dem ersten Bild), gerechnet wird zeilenparallel.
pub fn testbilder_pq(pix_fmt: AVPixelFormat, w: i32, h: i32, weiss_nit: f64) -> Result<Vec<Bild>, String> {
    use super::testbild;
    let (wu, hu) = (w.max(0) as usize, h.max(0) as usize);
    let bytes = ebenen16_bytes(pix_fmt, wu, hu).ok_or_else(|| format!("Testbild in PQ als {} nicht vorgesehen", pix_fmt_name(pix_fmt)))?;
    let schluessel = |p: (u8, u8, u8)| ((p.0 as u32) << 16) | ((p.1 as u32) << 8) | p.2 as u32;
    let mut tafel: std::collections::HashMap<u32, [u16; 3]> = std::collections::HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let p = testbild::punkt(x, y, w, h, 0);
            tafel.entry(schluessel(p)).or_insert_with(|| pq_codes_aus_709(p, weiss_nit));
        }
    }
    let codes = |p: (u8, u8, u8)| tafel.get(&schluessel(p)).copied().unwrap_or_else(|| pq_codes_aus_709(p, weiss_nit));
    let mut aus = Vec::with_capacity(testbild::N);
    for k in 0..testbild::N {
        let mut punkte = vec![[0u16; 3]; wu * hu];
        punkte.par_chunks_mut(wu.max(1)).enumerate().for_each(|(y, z)| {
            for (x, c) in z.iter_mut().enumerate() {
                *c = codes(testbild::punkt(x as i32, y as i32, w, h, k));
            }
        });
        let mut e = vec![0u8; bytes];
        let schreiben = |e: &mut [u8], i: usize, v: u16| e[2 * i..2 * i + 2].copy_from_slice(&(v << 6).to_le_bytes());
        let n = wu * hu;
        if pix_fmt == AVPixelFormat::AV_PIX_FMT_YUV444P16LE {
            for (i, c) in punkte.iter().enumerate() {
                for (ebene, &v) in c.iter().enumerate() {
                    schreiben(&mut e, ebene * n + i, v);
                }
            }
        } else {
            for (i, c) in punkte.iter().enumerate() {
                schreiben(&mut e, i, c[0]);
            }
            // CbCr je 2x2: das Mittel der vier Codes.
            for by in 0..hu / 2 {
                for bx in 0..wu / 2 {
                    let i = |dx: usize, dy: usize| (2 * by + dy) * wu + 2 * bx + dx;
                    let mittel = |k: usize| ((punkte[i(0, 0)][k] as u32 + punkte[i(1, 0)][k] as u32 + punkte[i(0, 1)][k] as u32 + punkte[i(1, 1)][k] as u32 + 2) / 4) as u16;
                    let j = n + 2 * (by * (wu / 2) + bx);
                    schreiben(&mut e, j, mittel(1));
                    schreiben(&mut e, j + 1, mittel(2));
                }
            }
        }
        let mut b = Bild::neu(pix_fmt, w, h)?;
        b.aus_ebenen16(&e, wu, hu)?;
        aus.push(b);
    }
    Ok(aus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umrechnung_trifft_die_balken() {
        // Das BGRA-Testbild zurueck nach Y/Cb/Cr muss die Balkenwerte treffen (+-2).
        let (w, h) = (64, 48);
        let src = super::super::testbild::bgra(w, h, 0);
        let mut y = vec![0u8; (w * h) as usize];
        let mut u = vec![0u8; (w * h) as usize];
        let mut v = vec![0u8; (w * h) as usize];
        bgra_nach_yuv444(&src, w as usize, &mut y, &mut u, &mut v);
        for b in 0..8 {
            let x = (b * w as usize) / 8 + 2;
            let i = 4 * w as usize + x;
            assert!((y[i] as i32 - super::super::testbild::BALKEN_Y[b] as i32).abs() <= 2, "Y Balken {b}");
            assert!((u[i] as i32 - super::super::testbild::BALKEN_CB[b] as i32).abs() <= 2, "Cb Balken {b}");
            assert!((v[i] as i32 - super::super::testbild::BALKEN_CR[b] as i32).abs() <= 2, "Cr Balken {b}");
        }
    }

    // Die Tests kommen ohne FFmpeg-Aufrufe aus: die Testdatei darf die
    // FFmpeg-DLLs nicht importieren (sie liegen beim Testlauf nicht im PATH).

    #[test]
    fn zu_kurze_quelle_ist_ein_fehler() {
        // Der Fall aus dem Pruefbericht: 1920x1080 bei 150 % - Strom
        // 1280x720, die Quelle aber nur 960x540 (halbiert). Fehler statt
        // Lesen (BGRA) oder Panik (Umrechnung).
        assert!(bgra_pruefen(960 * 540 * 4, 1280, 720, 1280, 720).is_err());
        assert!(bgra_pruefen(1280 * 720 * 4, 1282, 720, 1280, 720).is_err(), "breiter als der Rahmen");
        assert!(bgra_pruefen(1280 * 720 * 4, 1280, 722, 1280, 720).is_err(), "hoeher als der Rahmen");
        assert!(bgra_pruefen(0, 0, 720, 1280, 720).is_err());
        bgra_pruefen(1280 * 720 * 4, 1280, 720, 1280, 720).unwrap();
        bgra_pruefen(1280 * 720 * 4, 1278, 718, 1280, 720).unwrap();
        // Ebenen: Quelle zu kurz, Zeile laenger als der Zeilenabstand, mehr
        // Zeilen als die Ebene hat.
        assert!(ebene_pruefen(0, 960 * 540 * 4, 1280 * 4, 720, 1280 * 4, 720).is_err());
        assert!(ebene_pruefen(0, 4096 * 48, 4096, 48, 64, 48).is_err());
        assert!(ebene_pruefen(1, 64 * 48, 64, 48, 64, 24).is_err());
        ebene_pruefen(0, 1280 * 720 * 4, 1280 * 4, 720, 1280 * 4 + 64, 720).unwrap();
        // Zeilen je Ebene der Formate, die der Host fuellt.
        use AVPixelFormat::*;
        assert_eq!(ebene_zeilen(AV_PIX_FMT_BGRA, 0, 720), 720);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_BGRA, 1, 720), 0);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_YUV444P16LE, 2, 720), 720);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_YUV444P, 3, 720), 0);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_NV12, 1, 720), 360);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_P010LE, 1, 721), 361);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_NV12, 2, 720), 0);
        assert_eq!(ebene_zeilen(AV_PIX_FMT_D3D11, 0, 720), 0);
    }

    #[test]
    fn rahmen_passt_nur_zur_sitzung() {
        // Ein Bild im Systemspeicher erreicht eine Sitzung mit Pool nie
        // (nvenc wuerde frame->hw_frames_ctx lesen), eine Textur nur die
        // Sitzung ihres Pools, und ohne Pool zaehlen Format und Groesse.
        let mut puffer = vec![0u8; 64 * 48 * 4];
        let mut ram: AVFrame = unsafe { std::mem::zeroed() };
        ram.format = AVPixelFormat::AV_PIX_FMT_BGRA as i32;
        ram.width = 64;
        ram.height = 48;
        ram.data[0] = puffer.as_mut_ptr();
        let bgra = AVPixelFormat::AV_PIX_FMT_BGRA;
        assert!(rahmen_passt(&ram, None, bgra, 64, 48));
        assert!(!rahmen_passt(&ram, Some(std::ptr::null_mut()), bgra, 64, 48));
        assert!(!rahmen_passt(&ram, None, AVPixelFormat::AV_PIX_FMT_NV12, 64, 48));
        assert!(!rahmen_passt(&ram, None, bgra, 66, 48));
        assert!(!rahmen_passt(std::ptr::null(), None, bgra, 64, 48));
        let (mut a, mut b) = (1u8, 2u8);
        let mut pool_a: AVBufferRef = unsafe { std::mem::zeroed() };
        let mut pool_b: AVBufferRef = unsafe { std::mem::zeroed() };
        pool_a.data = &mut a;
        pool_b.data = &mut b;
        let mut tex: AVFrame = unsafe { std::mem::zeroed() };
        tex.format = AVPixelFormat::AV_PIX_FMT_D3D11 as i32;
        tex.width = 64;
        tex.height = 48;
        tex.data[0] = puffer.as_mut_ptr();
        tex.hw_frames_ctx = &mut pool_a;
        assert!(rahmen_passt(&tex, Some(&mut pool_a), bgra, 64, 48));
        assert!(!rahmen_passt(&tex, Some(&mut pool_b), bgra, 64, 48), "anderer Pool");
        assert!(!rahmen_passt(&tex, None, bgra, 64, 48), "Textur an eine Sitzung ohne Pool");
        let mut leer: AVFrame = unsafe { std::mem::zeroed() };
        leer.format = AVPixelFormat::AV_PIX_FMT_BGRA as i32;
        leer.width = 64;
        leer.height = 48;
        assert!(!rahmen_passt(&leer, None, bgra, 64, 48), "ohne Daten");
    }

    #[test]
    fn nur_codecs_des_protokolls() {
        // Strominfo und Codecwechsel kennen nur HEVC (h264 = false) und
        // H.264 (h264 = true); AV1 darf nie als vorhanden gelten.
        for k in KANDIDATEN.iter() {
            if im_protokoll(k) {
                assert!((k.encoder.starts_with("hevc_") && !k.h264) || (k.encoder.starts_with("h264_") && k.h264), "{}", k.name);
            }
        }
        assert!(KANDIDATEN.iter().filter(|k| k.encoder == "av1_nvenc").all(|k| !im_protokoll(k)));
        assert_eq!(KANDIDATEN.iter().filter(|k| im_protokoll(k)).count(), 5);
    }

    #[test]
    fn messung_zaehlt_nur_die_volle_groesse() {
        // Eine Messung von frueher an einem 4K-Ausgang: voll auf dem
        // Prozessor, der 1080p-Ausschnitt ohne Kopie. Der Strom ist nativ -
        // es gilt die volle Groesse, also kein d3d11.
        let alt = "Kopf\n=== Entscheidung ===\n\
            Ausgang 0 (3840x2160): Prozessor, 14.2 ms je Bild, hoechstens 60 fps\n\
            Ausgang 0 (1920x1080 Ausschnitt): Null-Kopien, 2.9 ms je Bild, hoechstens 120 fps\n\
            Ausgang 1 (2560x1440): Null-Kopien, 3.1 ms je Bild, hoechstens 120 fps\n\
            Farbprobe: bgra direkt bestanden, yuv444p bestanden -> BGRA direkt in NVENC ist der Hauptweg fuer 4:4:4 8 Bit\n";
        assert_eq!(messung_lesen(alt, 0), Some((false, true, true)));
        assert_eq!(messung_lesen(alt, 1), Some((true, true, true)));
        assert_eq!(messung_lesen(alt, 2), Some((false, true, true)));
        // Eine neue Messung: 4K ohne Kopie -> d3d11 auch fuer 4K.
        let neu = "=== Entscheidung ===\n\
            Ausgang 0 (3840x2160): Null-Kopien, 4.8 ms je Bild, hoechstens 120 fps\n\
            Ausgang 10 (1920x1080): Null-Kopien, 2.0 ms je Bild, hoechstens 120 fps\n\
            Farbprobe: bgra direkt bestanden, yuv444p nicht bestanden -> ...\n";
        assert_eq!(messung_lesen(neu, 0), Some((true, true, false)));
        assert_eq!(messung_lesen(neu, 1), Some((false, true, false)), "Ausgang 10 ist nicht Ausgang 1");
        assert_eq!(messung_lesen("keine Entscheidung", 0), None);
        assert_eq!(weg_aus_messung(true, true, true), Weg::D3d11);
        assert_eq!(weg_aus_messung(true, false, true), Weg::Yuv444);
        assert_eq!(weg_aus_messung(false, true, false), Weg::Bgra);
        assert_eq!(weg_aus_messung(false, false, false), Weg::Bgra);
    }

    #[test]
    fn texturweg_folgt_dem_encoder() {
        // Die Aufnahme liefert Texturen nur, wenn der Encoder sie auch
        // nimmt: 8 Bit ueber nvenc auf dem Weg d3d11 - nicht 10 Bit, nicht
        // Media Foundation, nicht die anderen Wege. texturweg ist genau
        // eingabeformat(..).1 mit dem Encoder aus dem Befund; getestet wird
        // mit dem Encodernamen direkt, ohne den gemeinsamen BEFUND
        // anzufassen (die Tests laufen parallel).
        let tex = |idx: usize, weg: Weg, enc: &str| eingabeformat(idx, weg, enc).1;
        assert!(!tex(0, Weg::D3d11, "hevc_nvenc"));
        assert!(tex(1, Weg::D3d11, "hevc_nvenc"));
        assert!(!tex(2, Weg::D3d11, "hevc_nvenc"));
        assert!(tex(3, Weg::D3d11, "hevc_nvenc"));
        assert!(!tex(4, Weg::D3d11, MF_H264));
        assert!(tex(4, Weg::D3d11, "h264_nvenc"));
        assert!(!tex(1, Weg::Bgra, "hevc_nvenc"));
        assert!(!tex(1, Weg::Yuv444, "hevc_nvenc"));
        assert!(!tex(1, Weg::Auto, "hevc_nvenc"));
        // Media Foundation nimmt immer NV12 im Systemspeicher.
        assert!(eingabeformat(4, Weg::D3d11, MF_H264) == (AVPixelFormat::AV_PIX_FMT_NV12, false));
    }

    /// Das VUI einer Sitzung: SDR BT.709 voll wie bisher, HDR10 BT.2020-NCL
    /// / BT.2020 / SMPTE ST 2084 voll - die Codes 9/16/9 aus dem Plan. Und
    /// der Transfer fuer Switch 7 (Byte 6).
    #[test]
    fn vui_und_transfer_nach_der_farbe() {
        let pq = StromFarbe::Pq2020(Mastering::VORGABE);
        let (m, p, t, r) = pq.vui();
        assert_eq!((m as u32, p as u32, t as u32), (9, 9, 16));
        assert!(r == AVColorRange::AVCOL_RANGE_JPEG);
        let (m, p, t, r) = StromFarbe::Sdr709.vui();
        assert_eq!((m as u32, p as u32, t as u32), (1, 1, 1));
        assert!(r == AVColorRange::AVCOL_RANGE_JPEG);
        assert_eq!((pq.transfer(), StromFarbe::Sdr709.transfer()), (crate::hdr::TRANSFER_PQ, crate::hdr::TRANSFER_SDR));
        assert!(pq.ist_pq() && !StromFarbe::Sdr709.ist_pq());
        assert_eq!(pq.text(), "HDR10 PQ/BT.2020, Mastering 0.0050-1000 nit");
        // Switch 7: Kandidat 0 in PQ traegt 16 in Byte 6, sonst wie bisher.
        let s = switch_nutzlast(0, crate::hdr::TRANSFER_PQ);
        assert_eq!((s[0], s[1], s[2], s[3], s[4], s[6], s[7]), (0, 0, 1, 1, 1, 16, 0));
        assert_eq!(switch_nutzlast(2, crate::hdr::TRANSFER_SDR)[6], 1);
    }

    /// Die Mastering-Seitendaten: 88 Byte wie AVMasteringDisplayMetadata
    /// (konserve.rs liest sie mit denselben Versaetzen zurueck),
    /// BT.2020-Primaerfarben und D65 in 0,00002, Leuchtdichte in 0,0001 nit.
    #[test]
    fn mastering_wie_die_sei_137() {
        assert_eq!(std::mem::size_of::<AvMastering>(), 88);
        assert_eq!(std::mem::size_of::<AvLichtpegel>(), 8);
        let a = av_mastering(&Mastering { max_nit: 1015, min_zehntausendstel: 123 });
        let paar = |p: [AVRational; 2]| (p[0].num, p[0].den, p[1].num, p[1].den);
        assert_eq!(paar(a.display_primaries[0]), (35400, 50000, 14600, 50000), "Rot");
        assert_eq!(paar(a.display_primaries[1]), (8500, 50000, 39850, 50000), "Gruen");
        assert_eq!(paar(a.display_primaries[2]), (6550, 50000, 2300, 50000), "Blau");
        assert_eq!(paar(a.white_point), (15635, 50000, 16450, 50000), "D65");
        assert_eq!((a.max_luminance.num, a.max_luminance.den), (1015 * 10000, 10000));
        assert_eq!((a.min_luminance.num, a.min_luminance.den), (123, 10000));
        assert_eq!((a.has_primaries, a.has_luminance), (1, 1));
        let bytes = unsafe { std::slice::from_raw_parts(&a as *const AvMastering as *const u8, 88) };
        assert_eq!(super::super::konserve::mastering_lesen(bytes), (1015, 123));
    }

    /// Die HDR10-Seitendaten landen am Kontext (decoded_side_data), genau
    /// je einmal - auch wenn sie zweimal gesetzt werden (UNIQUE) - und mit
    /// den Bytes von AvMastering und 0/0 fuer CLL. Ein Kontext ohne Codec
    /// genuegt; nvenc braucht es dafuer nicht.
    #[test]
    fn seitendaten_am_kontext() {
        unsafe {
            let mut ctx = avcodec_alloc_context3(std::ptr::null());
            assert!(!ctx.is_null());
            let m = Mastering { max_nit: 1000, min_zehntausendstel: 50 };
            farbe_setzen(ctx, &StromFarbe::Pq2020(m)).expect("Seitendaten");
            farbe_setzen(ctx, &StromFarbe::Pq2020(m)).expect("Seitendaten zweimal");
            assert!((*ctx).color_trc == AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084);
            assert!((*ctx).colorspace == AVColorSpace::AVCOL_SPC_BT2020_NCL);
            assert!((*ctx).color_primaries == AVColorPrimaries::AVCOL_PRI_BT2020);
            let n = (*ctx).nb_decoded_side_data as usize;
            assert_eq!(n, 2, "je Art genau eine");
            let sd = std::slice::from_raw_parts((*ctx).decoded_side_data, n);
            let art = |i: usize| (*sd[i]).type_;
            let daten = |i: usize| std::slice::from_raw_parts((*sd[i]).data, (*sd[i]).size);
            let i_mdcv = (0..n).find(|&i| art(i) == AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA).expect("MDCV");
            let i_cll = (0..n).find(|&i| art(i) == AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL).expect("CLL");
            assert_eq!(super::super::konserve::mastering_lesen(daten(i_mdcv)), (1000, 50));
            assert_eq!(super::super::konserve::lichtpegel_lesen(daten(i_cll)), (0, 0));
            avcodec_free_context(&mut ctx);
            // SDR: nur das VUI, keine Seitendaten.
            let mut ctx = avcodec_alloc_context3(std::ptr::null());
            farbe_setzen(ctx, &StromFarbe::Sdr709).unwrap();
            assert_eq!((*ctx).nb_decoded_side_data, 0);
            assert!((*ctx).color_trc == AVColorTransferCharacteristic::AVCOL_TRC_BT709);
            avcodec_free_context(&mut ctx);
        }
        // HDR10 nur mit den 10-Bit-Ebenen: eine BGRA-Oeffnung wird vorher abgelehnt.
        let o = Oeffnung { farbe: StromFarbe::Pq2020(Mastering::VORGABE), ..Oeffnung::vorgabe(64, 48) };
        assert!(Sitzung::oeffnen(&o).err().unwrap().contains("HDR10 nur mit 10-Bit-Ebenen"));
    }

    /// Die PQ-Ebenen des Wandlers in ein Bild: YUV444P16LE drei Ebenen, P010
    /// Y und die Paare in halber Hoehe - Byte fuer Byte, auch bei
    /// Zeilenrest (linesize > 2w). Falsche Groesse oder zu kurze Quelle ist
    /// ein Fehler vor jedem Zugriff.
    #[test]
    fn ebenen16_in_das_bild() {
        let (w, h) = (6usize, 4usize);
        for pf in [AVPixelFormat::AV_PIX_FMT_YUV444P16LE, AVPixelFormat::AV_PIX_FMT_P010LE] {
            let n = ebenen16_bytes(pf, w, h).unwrap();
            let daten: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
            let mut b = Bild::neu(pf, w as i32, h as i32).unwrap();
            b.aus_ebenen16(&daten, w, h).unwrap();
            let ebenen: Vec<(usize, usize)> = if pf == AVPixelFormat::AV_PIX_FMT_YUV444P16LE { vec![(2 * w, h); 3] } else { vec![(2 * w, h), (2 * w, h / 2)] };
            let mut o = 0;
            for (i, (zeile, zeilen)) in ebenen.into_iter().enumerate() {
                let ls = unsafe { (*b.frame).linesize[i] as usize };
                for y in 0..zeilen {
                    let z = unsafe { std::slice::from_raw_parts((*b.frame).data[i].add(y * ls), zeile) };
                    assert_eq!(z, &daten[o + y * zeile..o + (y + 1) * zeile], "{} Ebene {i} Zeile {y}", pix_fmt_name(pf));
                }
                o += zeile * zeilen;
            }
            assert_eq!(o, n);
            assert!(b.aus_ebenen16(&daten[..n - 1], w, h).is_err(), "zu kurz");
            assert!(b.aus_ebenen16(&daten, w - 2, h).is_err(), "andere Groesse");
        }
        assert_eq!(ebenen16_bytes(AVPixelFormat::AV_PIX_FMT_P010LE, 5, 4), None, "P010 nur gerade");
        assert_eq!(ebenen16_bytes(AVPixelFormat::AV_PIX_FMT_NV12, 6, 4), None);
        let mut b = Bild::neu(AVPixelFormat::AV_PIX_FMT_NV12, 6, 4).unwrap();
        assert!(b.aus_ebenen16(&[0u8; 256], 6, 4).is_err());
    }

    /// Das Testbild in PQ: Grau ist SDR-Inhalt mit dem SDR-Weiss (Y' =
    /// pq_code10(sRGB * Weiss), Cb = Cr = 512), Weiss 235 bei 203 nit; die
    /// zwoelf Bilder in YUV444P16LE und P010 tragen genau diese Codes oben
    /// buendig (Balken oben links und die Farbbalken).
    #[test]
    fn testbild_in_pq() {
        for v in [16u8, 40, 128, 220, 235, 255] {
            let grau = pq_codes_aus_709((v, 128, 128), 203.0);
            let soll = crate::hdr::pq_code10(crate::hdr::srgb_eotf(v as f64 / 255.0) * 203.0);
            assert!((grau[0] as i32 - soll as i32).abs() <= 1, "Grau {v}: Y {} statt {soll}", grau[0]);
            assert_eq!((grau[1], grau[2]), (512, 512), "Grau {v}");
        }
        assert_eq!(pq_codes_aus_709((0, 128, 128), 203.0)[0], crate::hdr::pq_code10(0.0));
        // Die Farbbalken: Rot hat Cr ueber 512, Blau Cb ueber 512.
        let rot = pq_codes_aus_709((81, 90, 240), 203.0);
        let blau = pq_codes_aus_709((41, 240, 110), 203.0);
        assert!(rot[2] > 600 && blau[1] > 600, "{rot:?} {blau:?}");
        let (w, h) = (64i32, 48i32);
        for pf in [AVPixelFormat::AV_PIX_FMT_YUV444P16LE, AVPixelFormat::AV_PIX_FMT_P010LE] {
            let bilder = testbilder_pq(pf, w, h, 203.0).unwrap();
            assert_eq!(bilder.len(), super::super::testbild::N);
            for b in &bilder {
                assert_eq!(b.format(), pf);
                for balken in 0..8usize {
                    let x = balken * w as usize / 8 + 3;
                    let soll = pq_codes_aus_709(super::super::testbild::punkt(x as i32, 4, w, h, 0), 203.0);
                    let lesen = |ebene: usize, xx: usize, yy: usize, paar: usize| unsafe {
                        let ls = (*b.frame).linesize[ebene] as usize;
                        let p = (*b.frame).data[ebene].add(yy * ls + xx * 2 + paar * 2);
                        u16::from_le_bytes([*p, *p.add(1)])
                    };
                    assert_eq!(lesen(0, x, 4, 0), soll[0] << 6, "Y Balken {balken}");
                    if pf == AVPixelFormat::AV_PIX_FMT_YUV444P16LE {
                        assert_eq!((lesen(1, x, 4, 0), lesen(2, x, 4, 0)), (soll[1] << 6, soll[2] << 6), "CbCr Balken {balken}");
                    } else {
                        // Paar (Cb, Cr) fuer den Block (x/2, 4/2).
                        assert_eq!((lesen(1, x / 2 * 2, 2, 0), lesen(1, x / 2 * 2, 2, 1)), (soll[1] << 6, soll[2] << 6), "CbCr Balken {balken}");
                    }
                }
            }
        }
        assert!(testbilder_pq(AVPixelFormat::AV_PIX_FMT_NV12, w, h, 203.0).is_err());
    }
}
