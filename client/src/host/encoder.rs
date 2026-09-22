// Encoder des Windows-Hosts: Kandidatentabelle (dieselben sechs Eintraege
// in derselben Reihenfolge wie g_kandidaten auf dem Mac), Befund beim Start
// (avcodec_open2 je Kandidat - gefragt, nicht geraten) und die nvenc-Sitzung
// ueber FFmpeg 9 (LGPL-Shared-Build, nvenc ist headers-only ueber ffnvcodec).
//
// Heute: Befund fuer --list/--host, Sitzungen fuer die Messung (--messen).
// Codecwechsel und Betrieb am Bild folgen (Schritt 5).
//
// Alles ueber die rohe C-Schnittstelle (ffmpeg::sys), weil die Optionen der
// Hardware-Encoder (preset, tune, delay, forced-idr) nur so erreichbar sind.

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use ffmpeg_next as ffmpeg;
use ffmpeg::sys::*;

use super::log;

pub struct Kandidat {
    pub name: &'static str,
    pub encoder: &'static str,
    /// Eingabeformat fuer Befund und Betrieb.
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

#[derive(Clone, Copy, Default)]
pub struct Befund {
    pub vorhanden: bool,
    pub hardware: bool,
}

static BEFUND: Mutex<[Befund; 6]> = Mutex::new([Befund { vorhanden: false, hardware: false }; 6]);
/// Konserve als Bildquelle: genau dieser Eintrag gilt als vorhanden (ohne
/// Hardware-Vermerk), ein Codecwechsel wird abgelehnt. -1 = keine Konserve.
static KONSERVE_IDX: AtomicI32 = AtomicI32::new(-1);

pub fn kandidat(idx: usize) -> &'static Kandidat {
    &KANDIDATEN[idx.min(KANDIDATEN.len() - 1)]
}

pub fn konserve_setzen(idx: usize) {
    KONSERVE_IDX.store(idx as i32, Ordering::Relaxed);
    super::Z.codec_id.store(idx as u32, Ordering::Relaxed);
    super::Z.konserve.store(true, Ordering::Relaxed);
}

pub fn pix_fmt_name(p: AVPixelFormat) -> String {
    unsafe {
        let n = av_get_pix_fmt_name(p);
        if n.is_null() { "?".into() } else { CStr::from_ptr(n).to_string_lossy().into_owned() }
    }
}

/// Befund beim Start: fuer jeden Kandidaten eine Sitzung mit 1920x1080 und
/// seinem Eingabeformat oeffnen - was hier nicht aufgeht, kann die Maschine
/// nicht, egal was in der Encoderliste steht. Ohne NVIDIA-Treiber (VM)
/// scheitert nvenc mit "Cannot load nvcuda.dll" - das steht dann da.
pub fn pruefen() {
    log("--- Was dieser Rechner codieren kann ---");
    let mut befund = BEFUND.lock().unwrap();
    for (i, k) in KANDIDATEN.iter().enumerate() {
        crate::protokoll::fehler_verwerfen();
        match Sitzung::oeffnen(&Oeffnung { encoder: k.encoder, pix_fmt: k.pix_fmt, profil: k.profil, ..Oeffnung::vorgabe(1920, 1080) }) {
            Ok(_) => {
                befund[i] = Befund { vorhanden: true, hardware: true };
                log(format!("  {:<18} ja (Hardware, {} {}{})", k.name, k.encoder, pix_fmt_name(k.pix_fmt), if k.umrechnung { ", mit Umrechnung" } else { "" }));
            }
            Err(e) => {
                befund[i] = Befund::default();
                log(format!("  {:<18} nein ({e})", k.name));
            }
        }
    }
}

/// Schritt 0: oeffnen die Media-Foundation-Encoder ohne Karte in Software -
/// und codieren sie dann auch ein Bild? (hevc_mf oeffnet auf der VM, liefert
/// aber nichts.) Dann gaebe es dort einen echten H.264-Encoder fuer Kandidat 4.
pub fn mf_pruefen() {
    for name in ["h264_mf", "hevc_mf"] {
        crate::protokoll::fehler_verwerfen();
        let o = Oeffnung { encoder: name, pix_fmt: AVPixelFormat::AV_PIX_FMT_NV12, profil: "", extra: &[("hw_encoding", "0")], ..Oeffnung::vorgabe(1920, 1080) };
        let mut s = match Sitzung::oeffnen(&o) {
            Ok(s) => s,
            Err(e) => {
                log(format!("{name}: oeffnet in Software: nein ({e})"));
                continue;
            }
        };
        // Ein Testbild hinein, Sitzung leeren: kommt ein Paket?
        let codiert = Bild::neu(AVPixelFormat::AV_PIX_FMT_NV12, 1920, 1080)
            .and_then(|mut b| {
                let [y, uv] = super::testbild::nv12(1920, 1080, 0);
                b.ebene_fuellen(0, &y, 1920, 1080);
                b.ebene_fuellen(1, &uv, 1920, 540);
                s.senden(b.frame, 0, true)?;
                let mut p = s.empfangen()?;
                p.extend(s.leeren());
                Ok(p.len())
            });
        match codiert {
            Ok(n) if n > 0 => log(format!("{name}: oeffnet in Software: ja, codiert ein Bild: ja ({n} Paket)")),
            Ok(_) => log(format!("{name}: oeffnet in Software: ja, codiert ein Bild: NEIN (kein Paket)")),
            Err(e) => log(format!("{name}: oeffnet in Software: ja, codiert ein Bild: NEIN ({e})")),
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
        let (vorhanden, hardware) = if konserve >= 0 {
            (i as i32 == konserve, false)
        } else {
            (befund[i].vorhanden, befund[i].hardware)
        };
        let name = &k.name.as_bytes()[..k.name.len().min(31)];
        buf.push(i as u8);
        buf.push(vorhanden as u8);
        buf.push(hardware as u8);
        buf.push(k.umrechnung as u8);
        buf.push(k.chroma444 as u8);
        buf.push(k.zehn_bit as u8);
        buf.push(name.len() as u8);
        buf.extend_from_slice(name);
    }
    buf
}

/// Codecwunsch vom Client (Nachricht 66). Mit Konserve: abgelehnt. Der
/// Wechsel im Betrieb kommt mit dem Encoder (Schritt 5).
pub fn codec_wunsch(idx: usize) {
    if idx >= KANDIDATEN.len() {
        log(format!("Codecwunsch {idx} abgelehnt: kein solcher Kandidat"));
        return;
    }
    if KONSERVE_IDX.load(Ordering::Relaxed) >= 0 {
        log(format!("Codecwunsch {idx} ({}) abgelehnt: Konserve", KANDIDATEN[idx].name));
        return;
    }
    if !BEFUND.lock().unwrap()[idx].vorhanden {
        log(format!("Codecwunsch {idx} abgelehnt: auf diesem Rechner nicht vorhanden"));
        return;
    }
    log(format!("Codecwunsch {} vorgemerkt: der Wechsel im Betrieb folgt mit dem Encoder (Schritt 5)", KANDIDATEN[idx].name));
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
    pub extra: &'a [(&'a str, &'a str)],
}

impl<'a> Oeffnung<'a> {
    pub fn vorgabe(w: i32, h: i32) -> Oeffnung<'a> {
        Oeffnung { encoder: "hevc_nvenc", pix_fmt: AVPixelFormat::AV_PIX_FMT_BGRA, profil: "rext", w, h, fps: 60, mbit: 50, delay: 0, preset: "p1", hw_frames: None, extra: &[] }
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
                if !o.profil.is_empty() {
                    opt(ctx, "profile", o.profil);
                }
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
}

impl Drop for Bild {
    fn drop(&mut self) {
        unsafe { av_frame_free(&mut self.frame) };
    }
}
