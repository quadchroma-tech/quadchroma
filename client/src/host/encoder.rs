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

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::Mutex;

use ffmpeg_next as ffmpeg;
use ffmpeg::sys::*;
use rayon::prelude::*;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET};

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
}

const KEIN_BEFUND: Befund = Befund { vorhanden: false, hardware: false, encoder: "", vorlauf: 0 };

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

/// auto -> die Entscheidung aus messung.txt: "Null-Kopien" fuer den Ausgang
/// heisst d3d11, sonst entscheidet die Farbprobe zwischen bgra und yuv444;
/// ohne Datei oder ohne Entscheidung bleibt es bei bgra.
pub fn weg_entscheiden(wunsch: Weg, ausgang_idx: usize) -> Weg {
    if wunsch != Weg::Auto {
        WEG.store(weg_code(wunsch), Ordering::Relaxed);
        log(format!("Encoderweg: {} (Befehlszeile)", wunsch.name()));
        return wunsch;
    }
    let text = crate::einstellungen::datei_pfad("messung.txt").and_then(|p| std::fs::read_to_string(p).ok());
    let weg = match text {
        Some(t) if t.contains("=== Entscheidung ===") => {
            let ent = t.split("=== Entscheidung ===").nth(1).unwrap_or("");
            let null_kopien = ent.lines().any(|z| z.starts_with(&format!("Ausgang {ausgang_idx} ")) && z.contains("Null-Kopien"));
            let farb_bgra = ent.lines().any(|z| z.starts_with("Farbprobe:") && z.contains("bgra direkt bestanden"));
            let farb_444 = ent.lines().any(|z| z.starts_with("Farbprobe:") && z.contains("yuv444p bestanden"));
            let w = if null_kopien && farb_bgra {
                Weg::D3d11
            } else if farb_bgra {
                Weg::Bgra
            } else if farb_444 {
                Weg::Yuv444
            } else {
                Weg::Bgra
            };
            log(format!("Encoderweg: {} (aus messung.txt: Null-Kopien {}, Farbprobe bgra {}, yuv444p {})", w.name(),
                if null_kopien { "ja" } else { "nein" }, if farb_bgra { "bestanden" } else { "nicht bestanden" }, if farb_444 { "bestanden" } else { "nicht bestanden" }));
            w
        }
        _ => {
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
                befund[i] = Befund { vorhanden: true, hardware: true, encoder: k.encoder, vorlauf: 0 };
                log(format!("  {:<18} ja (Hardware, {} {}{})", k.name, k.encoder, pix_fmt_name(k.pix_fmt), if k.umrechnung { ", mit Umrechnung" } else { "" }));
            }
            Err(e) => {
                befund[i] = KEIN_BEFUND;
                if k.h264 {
                    match mf_probe(MF_H264) {
                        Ok(n) => {
                            befund[i] = Befund { vorhanden: true, hardware: false, encoder: MF_H264, vorlauf: n - 1 };
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
        b.ebene_fuellen(0, &y, 1920, 1080);
        b.ebene_fuellen(1, &uv, 1920, 540);
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
/// u8 umrechnung, u16 frei.
pub fn switch_senden(idx: usize) {
    let k = kandidat(idx);
    debug_assert!(im_protokoll(k), "{}: Codec nicht im Protokoll", k.name);
    let b = befund(idx);
    let p = [idx as u8, k.h264 as u8, k.chroma444 as u8, k.zehn_bit as u8, 1, umrechnung_fuer(idx, &b) as u8, 0, 0];
    netz::send_small(crate::protokoll_konst::MSG_SWITCH, &p);
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
    pub extra: &'a [(&'a str, &'a str)],
}

impl<'a> Oeffnung<'a> {
    pub fn vorgabe(w: i32, h: i32) -> Oeffnung<'a> {
        Oeffnung { encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_BGRA, profil: "rext", w, h, fps: 60, mbit: 50, delay: 0, preset: "p1", hw_frames: None, rgb_444: true, extra: &[] }
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

impl Sitzung {
    /// Sitzung oeffnen. Optionen wie im Plan: preset, tune ull, rc cbr,
    /// b = maxrate = mbit, bufsize ein Bild, GOP gross (verwaltet der Host:
    /// forced-idr), keine B-Bilder, kein Lookahead, BT.709 Vollbereich,
    /// Zeitbasis 1/1000000, KEIN GLOBAL_HEADER (Parametersaetze vor jedem IDR).
    /// Media Foundation: Software, CBR, Bildschirmuebertragung, geringe
    /// Verzoegerung; pict_type I erzwingt dort ebenfalls ein Schluesselbild.
    pub fn oeffnen(o: &Oeffnung) -> Result<Sitzung, String> {
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
            (*ctx).colorspace = AVColorSpace::AVCOL_SPC_BT709;
            (*ctx).color_primaries = AVColorPrimaries::AVCOL_PRI_BT709;
            (*ctx).color_trc = AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*ctx).color_range = AVColorRange::AVCOL_RANGE_JPEG;
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

    /// Ebene `i` zeilenweise fuellen (bytes_je_zeile aus der Quelle, Hoehe rows).
    pub fn ebene_fuellen(&mut self, i: usize, quelle: &[u8], bytes_je_zeile: usize, zeilen: usize) {
        unsafe {
            let ls = (*self.frame).linesize[i] as usize;
            let dst = (*self.frame).data[i];
            for y in 0..zeilen {
                std::ptr::copy_nonoverlapping(quelle.as_ptr().add(y * bytes_je_zeile), dst.add(y * ls), bytes_je_zeile);
            }
        }
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
    pub fn aus_bgra(&mut self, src: &[u8], w: usize, h: usize) -> Result<(), String> {
        self.schreibbar()?;
        let zeile = w * 4;
        match self.format() {
            AVPixelFormat::AV_PIX_FMT_BGRA => {
                self.ebene_fuellen(0, src, zeile, h);
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
}

impl Drop for Bild {
    fn drop(&mut self) {
        unsafe { av_frame_free(&mut self.frame) };
    }
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

/// BGRA auf die Haelfte (Mittel aus 2x2), fuer Ausgaenge breiter als 3840.
pub fn bgra_halbieren(src: &[u8], w: usize, h: usize, ziel: &mut Vec<u8>) {
    let (w2, h2) = (w / 2, h / 2);
    ziel.resize(w2 * h2 * 4, 0);
    ziel.par_chunks_mut(w2 * 4).enumerate().for_each(|(row, z)| {
        let a = &src[2 * row * w * 4..(2 * row + 1) * w * 4];
        let b = &src[(2 * row + 1) * w * 4..(2 * row + 2) * w * 4];
        for x in 0..w2 {
            for k in 0..4 {
                let i = 8 * x + k;
                z[4 * x + k] = ((a[i] as u32 + a[i + 4] as u32 + b[i] as u32 + b[i + 4] as u32 + 2) / 4) as u8;
            }
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
/// RENDER_TARGET, vier Texturen.
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
        (*fc).initial_pool_size = 4;
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

// ---------------------------------------------------------------- Betrieb

/// Woher das Bild fuer den Encoder kommt.
pub enum Quelle<'a> {
    /// BGRA im Systemspeicher (Staging-Kopie der Duplication).
    Ram(&'a [u8]),
    /// Eine Textur auf dem Geraet der Duplication (Null-Kopien-Weg).
    Textur(&'a ID3D11Texture2D),
    /// Ein fertiges Bild im Eingabeformat der Sitzung (Testbild).
    Fertig(&'a mut Bild),
    /// Das zuletzt gegebene Bild noch einmal (feste Bildrate).
    Wiederholung,
}

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
    pool: Option<Pool>,
    /// Fuer Texturquellen: der Kontext, der in den Pool kopiert.
    ctx: Option<ID3D11DeviceContext>,
    pub pix_fmt: AVPixelFormat,
    pub w: i32,
    pub h: i32,
    fps: i32,
    mbit: i32,
    gaming: bool,
    /// Bilder seit dem letzten IDR (der Host verwaltet die GOP selbst).
    seit_idr: i32,
    /// Bilder, deren Paket noch aussteht: pts -> (Aufnahmezeit, nachgelegt).
    unterwegs: VecDeque<(i64, u64, bool)>,
    /// Was der Encoder bauartbedingt zurueckhaelt - zaehlt nicht als Stau.
    vorlauf: usize,
    /// Ob schon ein Bild gegeben wurde (fuer Wiederholung ohne Vorbild).
    hat_bild: bool,
    fehler_gemeldet: bool,
}

impl Betrieb {
    /// Sitzung fuer einen Kandidaten oeffnen: Encoder und Eingabeformat aus
    /// Befund und Weg; fuer d3d11 der Pool auf dem Geraet der Duplication.
    pub fn oeffnen(idx: usize, w: i32, h: i32, weg: Weg, geraet: Option<(&ID3D11Device, &ID3D11DeviceContext)>) -> Result<Betrieb, String> {
        let k = kandidat(idx);
        let b = befund(idx);
        if !b.vorhanden {
            return Err(format!("{} ist auf diesem Rechner nicht vorhanden", k.name));
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
            ..Oeffnung::vorgabe(w, h)
        };
        // Scheitert das Oeffnen, raeumt Drop von `pool` auf.
        let sitzung = Sitzung::oeffnen(&o)?;
        let bild = if texturen { Bild::leer() } else { Bild::neu(pix_fmt, w, h)? };
        log(format!(
            "Encoder: Kandidat {idx} {}, {w}x{h}, {}, {mbit} Mbit/s, {fps} fps, Eingabe {}{}",
            k.name,
            if b.hardware { format!("Hardware ({})", b.encoder) } else { format!("Software ({})", b.encoder) },
            if texturen { "d3d11/bgra ohne Kopie".to_string() } else { pix_fmt_name(pix_fmt) },
            if umrechnung_fuer(idx, &b) { " (mit Umrechnung)" } else { "" }
        ) + &if b.vorlauf > 0 { format!(", Vorlauf {} Bilder", b.vorlauf) } else { String::new() });
        Ok(Betrieb { idx, sitzung, bild, pool, ctx, pix_fmt, w, h, fps, mbit, gaming, seit_idr: 0, unterwegs: VecDeque::new(), vorlauf: b.vorlauf, hat_bild: false, fehler_gemeldet: false })
    }

    pub fn texturen(&self) -> bool {
        self.pool.is_some()
    }

    /// Bilder, deren Paket noch aussteht - ohne den Vorlauf des Encoders.
    pub fn inflight(&self) -> usize {
        self.unterwegs.len().saturating_sub(self.vorlauf)
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
        let neu = Betrieb::oeffnen(self.idx, self.w, self.h, weg, geraet)?;
        let alt = std::mem::replace(self, neu);
        alt.schliessen();
        Z.force_key.store(true, Ordering::Relaxed);
        Ok(true)
    }

    /// Ein Bild in den Encoder und die fertigen Pakete auf die Leitung.
    /// t_cap ist die ECHTE Aufnahmezeit (bei Wiederholungen die des
    /// wiederholten Bildes), pts die fuer den Encoder aufsteigende Zeit.
    pub fn codieren(&mut self, quelle: Quelle, t_cap_us: u64, pts_us: i64, wiederholt: bool) -> Result<(), String> {
        let vollbild = Z.force_key.swap(false, Ordering::Relaxed) || self.seit_idr >= if self.gaming { self.fps } else { self.fps * 2 };
        let frame = match quelle {
            Quelle::Ram(bgra) => {
                if let Some(p) = self.pool.as_ref() {
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
            Quelle::Textur(tex) => {
                let Some(p) = self.pool.as_ref() else { return Err("Textur ohne Pool".into()) };
                let (t, idx) = pool_textur(p.pool, &mut self.bild)?;
                unsafe {
                    self.ctx.as_ref().unwrap().CopySubresourceRegion(&t, idx, 0, 0, 0, tex, 0, None);
                }
                self.hat_bild = true;
                self.bild.frame
            }
            Quelle::Fertig(b) => b.frame,
            Quelle::Wiederholung => {
                if !self.hat_bild {
                    return Ok(());
                }
                self.bild.frame
            }
        };
        if let Err(e) = self.sitzung.senden(frame, pts_us, vollbild) {
            if !self.fehler_gemeldet {
                self.fehler_gemeldet = true;
                log(format!("Encoder {}: {e}", kandidat(self.idx).name));
            }
            Z.enc_verworfen.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }
        self.seit_idr = if vollbild { 1 } else { self.seit_idr + 1 };
        self.unterwegs.push_back((pts_us, t_cap_us, wiederholt));
        self.pakete_abholen()
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
/// wie qc_testbild_start auf dem Mac sie im Aufnahmeformat anlegt. Fuer
/// Texturen (d3d11) und BGRA-Sitzungen aus dem BGRA-Testbild, sonst direkt
/// aus den Y/Cb/Cr-Ganzzahlen (dieselben Werte wie auf dem Mac).
pub fn testbilder(pix_fmt: AVPixelFormat, w: i32, h: i32) -> Result<Vec<Bild>, String> {
    use super::testbild;
    let mut aus = Vec::with_capacity(testbild::N);
    for k in 0..testbild::N {
        let mut b = Bild::neu(if pix_fmt == AVPixelFormat::AV_PIX_FMT_D3D11 { AVPixelFormat::AV_PIX_FMT_BGRA } else { pix_fmt }, w, h)?;
        match b.format() {
            AVPixelFormat::AV_PIX_FMT_BGRA => b.ebene_fuellen(0, &testbild::bgra(w, h, k), (w * 4) as usize, h as usize),
            AVPixelFormat::AV_PIX_FMT_YUV444P => {
                let [y, u, v] = testbild::yuv444p(w, h, k);
                b.ebene_fuellen(0, &y, w as usize, h as usize);
                b.ebene_fuellen(1, &u, w as usize, h as usize);
                b.ebene_fuellen(2, &v, w as usize, h as usize);
            }
            AVPixelFormat::AV_PIX_FMT_YUV444P16LE => {
                let [y, u, v] = testbild::yuv444p16(w, h, k);
                b.ebene_fuellen(0, &y, 2 * w as usize, h as usize);
                b.ebene_fuellen(1, &u, 2 * w as usize, h as usize);
                b.ebene_fuellen(2, &v, 2 * w as usize, h as usize);
            }
            AVPixelFormat::AV_PIX_FMT_NV12 => {
                let [y, uv] = testbild::nv12(w, h, k);
                b.ebene_fuellen(0, &y, w as usize, h as usize);
                b.ebene_fuellen(1, &uv, w as usize, (h / 2) as usize);
            }
            AVPixelFormat::AV_PIX_FMT_P010LE => {
                let [y, uv] = testbild::nv12(w, h, k);
                let f = |p: Vec<u8>| -> Vec<u8> { p.iter().flat_map(|&b| [0u8, b]).collect() };
                b.ebene_fuellen(0, &f(y), 2 * w as usize, h as usize);
                b.ebene_fuellen(1, &f(uv), 2 * w as usize, (h / 2) as usize);
            }
            f => return Err(format!("Testbild in {} nicht vorgesehen", pix_fmt_name(f))),
        }
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
        let mut halb = Vec::new();
        bgra_halbieren(&src, w as usize, h as usize, &mut halb);
        assert_eq!(halb.len(), (w * h) as usize);
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
}
