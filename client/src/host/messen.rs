// Schritt 1: die Messung (`quadchroma.exe --messen [--output n] [--sekunden 10]`).
//
// Laeuft ohne Fenster; nur auf einem Rechner mit Bildschirm und NVIDIA-Karte
// sinnvoll (auf der VM liefert sie die Adapter-/Ausgangsliste und den
// Duplication-Fehler, mehr nicht). Alle Zeilen gehen auf die Konsole und
// nach %APPDATA%\QuadChroma\messung.txt.
//
// Je Ausgang (und, wenn er breiter als 3840 ist, zusaetzlich als
// 1080p-Ausschnitt oben links) ueber N Sekunden echte Bilder (der Benutzer
// bewegt derweil ein Fenster; Bilder ohne Inhalt zaehlen nicht):
//   A  Acquire      AcquireNextFrame -> Rueckkehr, dazu das Alter des Bildes
//   B  Kopie GPU    CopyResource in eine eigene Textur, ReleaseFrame
//   C  Staging      CopyResource -> STAGING, Map(READ), memcpy, Unmap
//   D1 nvenc bgra   send_frame(bgra aus dem Systemspeicher) + receive_packet
//   D2 nvenc 444    eigene Umrechnung BGRA -> yuv444p (BT.709 voll), dann nvenc
//   D3 nvenc d3d11  nur an der NVIDIA: hw_frames_ctx auf dem VORHANDENEN
//                   Geraet, CopySubresourceRegion in den Pool, send_frame(D3D11)
//   D4 Fremdkarte   Ausgang an der Intel: C, Upload in eine NVIDIA-Textur, D3
//   E  Kette        A -> Paket fertig je Weg, Median und 95-%-Wert
//   F  delay 0/2    D1 und D3 einmal synchron, einmal mit zwei Bildern unterwegs
//   G  Last         GetProcessTimes ueber den Lauf, in % eines Kerns
//   H  Farbprobe    12 Testbild-Bilder als bgra UND als yuv444p durch nvenc,
//                   mit dem hevc-Decoder zurueckgerechnet: Balken-Y 235/16,
//                   Cb/Cr innerhalb +-3 - sonst faellt Weg D1 weg
//   I  Vollbild     erstes Paket traegt VPS/SPS/PPS + IDR, AV_PKT_FLAG_KEY,
//                   forced-idr erzeugt ein IDR mitten im Lauf

use std::collections::VecDeque;
use std::time::Instant;

use ffmpeg_next as ffmpeg;
use ffmpeg::sys::*;
use windows::Win32::Graphics::Direct3D11::*;
// Ausdruecklich, weil ffmpeg::sys dieselben Namen als undurchsichtige
// Bindgen-Typen mitbringt - die ausdrueckliche Einfuhr hat Vorrang.
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use super::aufnahme::{self, Abholung, Ausgang, Duplication};
use super::encoder::{bgra_nach_yuv444, hw_geraet, hw_pool, pool_textur, Bild, Oeffnung, Paket, Sitzung};
use super::{arg_wert, log, testbild};

// ----------------------------------------------------------------- Helfer

/// Eine Reihe von Messwerten in Millisekunden.
#[derive(Default)]
struct Reihe(Vec<f64>);

impl Reihe {
    fn push(&mut self, ms: f64) {
        self.0.push(ms);
    }
    fn sortiert(&self) -> Vec<f64> {
        let mut v = self.0.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    }
    fn median(&self) -> f64 {
        let v = self.sortiert();
        if v.is_empty() { 0.0 } else { v[v.len() / 2] }
    }
    fn p95(&self) -> f64 {
        let v = self.sortiert();
        if v.is_empty() { 0.0 } else { v[(v.len() * 95 / 100).min(v.len() - 1)] }
    }
    fn n(&self) -> usize {
        self.0.len()
    }
    fn zeile(&self, name: &str) -> String {
        format!("  {:<28} n={:<5} Median {:>7.2} ms   95 % {:>7.2} ms", name, self.n(), self.median(), self.p95())
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn qpc() -> i64 {
    let mut v = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut v);
    }
    v
}

fn qpc_hz() -> i64 {
    let mut f = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    f.max(1)
}

/// NAL-Typen einer Zugriffseinheit (HEVC).
fn nal_typen(d: &[u8]) -> Vec<u8> {
    let mut aus = Vec::new();
    let mut i = 0;
    while i + 3 < d.len() {
        if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
            aus.push((d[i + 3] >> 1) & 0x3f);
            i += 3;
        } else {
            i += 1;
        }
    }
    aus
}

// Umrechnung BGRA -> yuv444p, FFmpeg-Geraet aus D3D11, Texturpool und
// Pooltextur liegen jetzt in encoder.rs (dort braucht sie auch der Betrieb).

// ------------------------------------------------------------------ Wege

/// Ein Encoderweg in der Messung.
struct Weg {
    name: String,
    sitzung: Sitzung,
    bild: Bild,
    /// Sendezeiten der Bilder, deren Paket noch aussteht (delay 2).
    unterwegs: VecDeque<Instant>,
    /// Zeit je Bild von send bis Paket.
    d: Reihe,
    /// Die ganze Kette (A + B [+ C] + D).
    kette: Reihe,
    pakete: usize,
    bytes: u64,
    fehler: Option<String>,
}

impl Weg {
    fn neu(name: &str, o: &Oeffnung, bild: Bild) -> Result<Weg, String> {
        crate::protokoll::fehler_verwerfen();
        let sitzung = Sitzung::oeffnen(o)?;
        Ok(Weg { name: name.into(), sitzung, bild, unterwegs: VecDeque::new(), d: Reihe::default(), kette: Reihe::default(), pakete: 0, bytes: 0, fehler: None })
    }

    /// Bild senden, Pakete abholen, Zeiten buchen. `vorlauf_ms` ist, was
    /// die Kette vor dem Encoder schon gekostet hat.
    fn durchlauf(&mut self, pts_us: i64, vollbild: bool, vorlauf_ms: f64) {
        if self.fehler.is_some() {
            return;
        }
        let t = Instant::now();
        if let Err(e) = self.sitzung.senden(self.bild.frame, pts_us, vollbild) {
            self.fehler = Some(e);
            return;
        }
        self.unterwegs.push_back(t);
        match self.sitzung.empfangen() {
            Ok(p) => {
                for pk in p {
                    if let Some(t0) = self.unterwegs.pop_front() {
                        let d = ms(t0);
                        self.d.push(d);
                        self.kette.push(vorlauf_ms + d);
                    }
                    self.pakete += 1;
                    self.bytes += pk.data.len() as u64;
                }
            }
            Err(e) => self.fehler = Some(e),
        }
    }

    fn bericht(&self) -> Vec<String> {
        if let Some(e) = &self.fehler {
            return vec![format!("  {:<28} nicht messbar: {e}", self.name)];
        }
        vec![
            self.d.zeile(&format!("D {}", self.name)),
            self.kette.zeile(&format!("E Kette {}", self.name)),
            format!("  {:<28} {} Pakete, {:.1} MB", "", self.pakete, self.bytes as f64 / 1e6),
        ]
    }
}

// ------------------------------------------------------------- Messlauf

struct Ergebnis {
    /// Median der Kette je Weg, fuer die Entscheidung.
    null_kopien: Option<f64>,
    prozessor: Option<f64>,
    staging: f64,
}

/// Ein Messlauf auf einem Ausgang in einer Groesse.
fn messlauf(a: &Ausgang, dup: &Duplication, w: u32, h: u32, ausschnitt: bool, sekunden: f64, nvidia_adapter: Option<&IDXGIAdapter1>) -> Result<Ergebnis, String> {
    let hz = qpc_hz() as f64;
    log(format!(
        "\n--- Ausgang {}: {} {}x{}{} an Karte {} ({}), {} s ---",
        a.index, a.name, w, h, if ausschnitt { " (Ausschnitt oben links)" } else { "" }, a.karte, a.karte_name, sekunden
    ));
    log(format!("  Duplication: Format {}, Desktopbild im Systemspeicher: {}", dup.format.0, if dup.im_systemspeicher { "ja" } else { "nein" }));

    let kopie = dup.textur(w, h, false)?;
    let staging = dup.textur(w, h, true)?;
    let mut ram: Vec<u8> = Vec::new();
    let (wi, hi) = (w as i32, h as i32);
    let fps = 60;
    let mbit = 50;

    // Wege oeffnen. Ein Weg, der nicht aufgeht, wird als "nicht messbar"
    // gefuehrt - der Rest laeuft weiter.
    let mut wege: Vec<Weg> = Vec::new();
    let mut d3_pool: Option<(*mut AVBufferRef, *mut AVBufferRef)> = None;
    let mut d4: Option<(ID3D11Device, ID3D11DeviceContext, *mut AVBufferRef, *mut AVBufferRef)> = None;
    let mut d1_idx: Vec<usize> = Vec::new();
    let mut d2_idx: Option<usize> = None;
    let mut d3_idx: Vec<usize> = Vec::new();
    let mut d4_idx: Option<usize> = None;

    for delay in [0, 2] {
        let o = Oeffnung { pix_fmt: AVPixelFormat::AV_PIX_FMT_BGRA, fps, mbit, delay, ..Oeffnung::vorgabe(wi, hi) };
        match Bild::neu(AVPixelFormat::AV_PIX_FMT_BGRA, wi, hi).and_then(|b| Weg::neu(&format!("D1 nvenc bgra delay {delay}"), &o, b)) {
            Ok(wg) => {
                d1_idx.push(wege.len());
                wege.push(wg);
            }
            Err(e) => log(format!("  D1 nvenc bgra delay {delay}: nicht messbar: {e}")),
        }
    }
    {
        let o = Oeffnung { pix_fmt: AVPixelFormat::AV_PIX_FMT_YUV444P, fps, mbit, delay: 0, ..Oeffnung::vorgabe(wi, hi) };
        match Bild::neu(AVPixelFormat::AV_PIX_FMT_YUV444P, wi, hi).and_then(|b| Weg::neu("D2 nvenc yuv444p (CPU-Umrechnung)", &o, b)) {
            Ok(wg) => {
                d2_idx = Some(wege.len());
                wege.push(wg);
            }
            Err(e) => log(format!("  D2 nvenc yuv444p: nicht messbar: {e}")),
        }
    }
    if a.nvidia {
        match hw_geraet(&dup.device).and_then(|g| hw_pool(g, wi, hi).map(|p| (g, p))) {
            Ok((g, p)) => {
                d3_pool = Some((g, p));
                for delay in [0, 2] {
                    let o = Oeffnung { fps, mbit, delay, hw_frames: Some(p), ..Oeffnung::vorgabe(wi, hi) };
                    match Weg::neu(&format!("D3 nvenc d3d11 delay {delay}"), &o, Bild::leer()) {
                        Ok(wg) => {
                            d3_idx.push(wege.len());
                            wege.push(wg);
                        }
                        Err(e) => log(format!("  D3 nvenc d3d11 delay {delay}: nicht messbar: {e}")),
                    }
                }
            }
            Err(e) => log(format!("  D3 nvenc d3d11: kein FFmpeg-Geraet auf unserem D3D11-Geraet: {e}")),
        }
    } else if let Some(nv) = nvidia_adapter {
        // Ausgang an der Intel: der Weg ueber den Prozessor in eine NVIDIA-
        // Textur, zum Vergleich mit D1 (NVENC-eigener Upload).
        match crate::anzeige::geraet_bauen(Some(nv), false, false) {
            Ok((dev, ctx, _)) => match hw_geraet(&dev).and_then(|g| hw_pool(g, wi, hi).map(|p| (g, p))) {
                Ok((g, p)) => {
                    let o = Oeffnung { fps, mbit, delay: 0, hw_frames: Some(p), ..Oeffnung::vorgabe(wi, hi) };
                    match Weg::neu("D4 Fremdkarte Upload+nvenc d3d11", &o, Bild::leer()) {
                        Ok(wg) => {
                            d4_idx = Some(wege.len());
                            wege.push(wg);
                            d4 = Some((dev, ctx, g, p));
                        }
                        Err(e) => log(format!("  D4 Fremdkarte: nicht messbar: {e}")),
                    }
                }
                Err(e) => log(format!("  D4 Fremdkarte: kein FFmpeg-Geraet auf der NVIDIA: {e}")),
            },
            Err(e) => log(format!("  D4 Fremdkarte: kein Geraet auf der NVIDIA: {e}")),
        }
    } else {
        log("  D3/D4: Ausgang haengt nicht an einer NVIDIA-Karte und es gibt keine - nur der Prozessorweg");
    }

    // Der Lauf.
    let mut acquire = Reihe::default();
    let mut alter = Reihe::default();
    let mut kopie_gpu = Reihe::default();
    let mut staging_r = Reihe::default();
    let mut umrechnung = Reihe::default();
    let mut nur_zeiger = 0usize;
    let mut leer = 0usize;
    let mut bilder = 0usize;
    let mut y444 = vec![0u8; (w * h) as usize];
    let mut u444 = vec![0u8; (w * h) as usize];
    let mut v444 = vec![0u8; (w * h) as usize];
    let cpu0 = crate::prozesszeit_100ns().unwrap_or(0);
    let start = Instant::now();
    let ausschnitt_box = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
    let mut fehler_lauf: Option<String> = None;

    while start.elapsed().as_secs_f64() < sekunden {
        let t_a = Instant::now();
        let bild = match dup.abholen(100) {
            Ok(b) => b,
            Err(e) => {
                fehler_lauf = Some(aufnahme::dxgi_fehler_text(&e));
                break;
            }
        };
        let (textur, praesentiert) = match bild {
            Abholung::Nichts => {
                leer += 1;
                continue;
            }
            Abholung::NurZeiger(_) => {
                dup.freigeben();
                nur_zeiger += 1;
                continue;
            }
            Abholung::Bild { textur, praesentiert_qpc, .. } => (textur, praesentiert_qpc),
        };
        let a_ms = ms(t_a);
        acquire.push(a_ms);
        alter.push((qpc() - praesentiert) as f64 * 1000.0 / hz);

        // B: Kopie auf der Karte, dann sofort freigeben.
        let t_b = Instant::now();
        unsafe {
            if ausschnitt {
                dup.ctx.CopySubresourceRegion(&kopie, 0, 0, 0, 0, &textur, 0, Some(&ausschnitt_box));
            } else {
                dup.ctx.CopyResource(&kopie, &textur);
            }
        }
        dup.freigeben();
        drop(textur);
        let b_ms = ms(t_b);
        kopie_gpu.push(b_ms);
        bilder += 1;
        let pts = start.elapsed().as_micros() as i64;
        let vollbild = bilder == 1;

        // D3: Null-Kopien - Kopie in die Pooltextur, dann der Encoder.
        for &i in &d3_idx {
            let Some((_, pool)) = d3_pool else { break };
            let wg = &mut wege[i];
            if wg.fehler.is_some() {
                continue;
            }
            let t = Instant::now();
            match pool_textur(pool, &mut wg.bild) {
                Ok((pt, idx)) => unsafe {
                    dup.ctx.CopySubresourceRegion(&pt, idx, 0, 0, 0, &kopie, 0, None);
                },
                Err(e) => {
                    wg.fehler = Some(e);
                    continue;
                }
            }
            let kopie_ms = ms(t);
            wg.durchlauf(pts, vollbild, a_ms + b_ms + kopie_ms);
        }

        // C: Staging in den Hauptspeicher.
        let t_c = Instant::now();
        unsafe {
            dup.ctx.CopyResource(&staging, &kopie);
        }
        if let Err(e) = dup.auslesen(&staging, w, h, &mut ram) {
            fehler_lauf = Some(e);
            break;
        }
        let c_ms = ms(t_c);
        staging_r.push(c_ms);

        // D1: bgra aus dem Systemspeicher.
        for &i in &d1_idx {
            let wg = &mut wege[i];
            if wg.fehler.is_some() {
                continue;
            }
            if let Err(e) = wg.bild.schreibbar().and_then(|_| wg.bild.ebene_fuellen(0, &ram, (w * 4) as usize, h as usize)) {
                wg.fehler = Some(e);
                continue;
            }
            wg.durchlauf(pts, vollbild, a_ms + b_ms + c_ms);
        }
        // D2: eigene Umrechnung, dann yuv444p.
        if let Some(i) = d2_idx {
            let wg = &mut wege[i];
            if wg.fehler.is_none() {
                let t = Instant::now();
                bgra_nach_yuv444(&ram, w as usize, &mut y444, &mut u444, &mut v444);
                let u_ms = ms(t);
                umrechnung.push(u_ms);
                let gefuellt = wg.bild.schreibbar().and_then(|_| {
                    wg.bild.ebene_fuellen(0, &y444, w as usize, h as usize)?;
                    wg.bild.ebene_fuellen(1, &u444, w as usize, h as usize)?;
                    wg.bild.ebene_fuellen(2, &v444, w as usize, h as usize)
                });
                match gefuellt {
                    Ok(()) => {
                        wg.durchlauf(pts, vollbild, a_ms + b_ms + c_ms + u_ms);
                    }
                    Err(e) => wg.fehler = Some(e),
                }
            }
        }
        // D4: Upload in die NVIDIA-Textur, dann D3.
        if let (Some(i), Some((_, nctx, _, pool))) = (d4_idx, d4.as_ref()) {
            let wg = &mut wege[i];
            if wg.fehler.is_none() {
                let t = Instant::now();
                match pool_textur(*pool, &mut wg.bild) {
                    Ok((pt, idx)) => unsafe {
                        nctx.UpdateSubresource(&pt, idx, None, ram.as_ptr() as *const _, w * 4, 0);
                    },
                    Err(e) => {
                        wg.fehler = Some(e);
                        continue;
                    }
                }
                let up_ms = ms(t);
                wg.durchlauf(pts, vollbild, a_ms + b_ms + c_ms + up_ms);
            }
        }
    }
    let dauer = start.elapsed().as_secs_f64();
    let cpu1 = crate::prozesszeit_100ns().unwrap_or(0);
    let last = if dauer > 0.0 { (cpu1 - cpu0) as f64 / 1e7 / dauer * 100.0 } else { 0.0 };

    // Restpakete der Wege mit delay 2.
    for wg in wege.iter_mut() {
        if wg.fehler.is_none() {
            let rest = wg.sitzung.leeren();
            wg.pakete += rest.len();
        }
    }

    log(format!("  Bilder: {bilder} in {dauer:.1} s ({:.1}/s), nur Zeiger: {nur_zeiger}, leer: {leer}", bilder as f64 / dauer));
    if let Some(e) = &fehler_lauf {
        log(format!("  Lauf abgebrochen: {e}"));
    }
    if bilder == 0 {
        log("  Kein Bild mit Inhalt - waehrend der Messung muss sich auf dem Ausgang etwas bewegen (Fenster ziehen, Video)");
    }
    log(acquire.zeile("A Acquire"));
    log(alter.zeile("A Alter beim Abholen"));
    log(kopie_gpu.zeile("B Kopie GPU + Release"));
    log(staging_r.zeile("C Staging + memcpy"));
    if umrechnung.n() > 0 {
        log(umrechnung.zeile("D2 Umrechnung BGRA->444"));
    }
    for wg in &wege {
        for z in wg.bericht() {
            log(z);
        }
    }
    log(format!("  G Prozessorlast des Laufs: {last:.1} % eines Kerns"));

    // Aufraeumen: erst die Sitzungen (halten den Pool), dann Pool und Geraet.
    drop(wege);
    unsafe {
        if let Some((mut g, mut p)) = d3_pool {
            av_buffer_unref(&mut p);
            av_buffer_unref(&mut g);
        }
        if let Some((_, _, mut g, mut p)) = d4 {
            av_buffer_unref(&mut p);
            av_buffer_unref(&mut g);
        }
    }
    drop((kopie, staging));

    // Die Mediane der Wege liegen jetzt in MEDIANE (Drop von Weg); der
    // Aufrufer holt sie fuer die Entscheidung von dort.
    Ok(Ergebnis { null_kopien: None, prozessor: None, staging: staging_r.median() })
}

// Die Mediane der Wege bleiben nach dem Lauf in dieser Ablage, weil die
// Wege selbst am Ende des Laufs abgebaut werden.
thread_local! {
    static MEDIANE: std::cell::RefCell<Vec<(String, f64)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn d3_median() -> Option<f64> {
    MEDIANE.with(|m| m.borrow().iter().find(|(n, _)| n.starts_with("D3") && n.ends_with("delay 0")).map(|(_, v)| *v))
}

fn prozessor_median() -> Option<f64> {
    MEDIANE.with(|m| {
        let b = m.borrow();
        let d1 = b.iter().find(|(n, _)| n.starts_with("D1") && n.ends_with("delay 0")).map(|(_, v)| *v);
        let d2 = b.iter().find(|(n, _)| n.starts_with("D2")).map(|(_, v)| *v);
        match (d1, d2) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    })
}

impl Drop for Weg {
    fn drop(&mut self) {
        if self.fehler.is_none() && self.kette.n() > 0 {
            MEDIANE.with(|m| m.borrow_mut().push((self.name.clone(), self.kette.median())));
        }
    }
}

// ------------------------------------------------------------- Farbprobe

/// Y/Cb/Cr eines Punktes aus einem decodierten Bild (8 oder 10 Bit).
fn abtasten(b: &ffmpeg::frame::Video, x: usize, y: usize) -> Option<(u8, u8, u8)> {
    let zehn = matches!(b.format(), ffmpeg::format::Pixel::YUV444P10LE | ffmpeg::format::Pixel::YUV444P16LE);
    let wert = |ebene: usize| -> Option<u8> {
        let d = b.data(ebene);
        let ls = b.stride(ebene);
        if zehn {
            let i = y * ls + x * 2;
            let v = u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]);
            Some(if b.format() == ffmpeg::format::Pixel::YUV444P16LE { (v >> 8) as u8 } else { (v >> 2) as u8 })
        } else {
            d.get(y * ls + x).copied()
        }
    };
    Some((wert(0)?, wert(1)?, wert(2)?))
}

fn farbprobe_weg(name: &str, pix_fmt: AVPixelFormat, w: i32, h: i32) -> Result<bool, String> {
    crate::protokoll::fehler_verwerfen();
    let o = Oeffnung { pix_fmt, fps: 60, mbit: 50, delay: 0, ..Oeffnung::vorgabe(w, h) };
    let mut s = Sitzung::oeffnen(&o)?;
    let mut bild = Bild::neu(pix_fmt, w, h)?;
    let mut pakete: Vec<Paket> = Vec::new();
    for k in 0..testbild::N {
        bild.schreibbar()?;
        match pix_fmt {
            AVPixelFormat::AV_PIX_FMT_BGRA => bild.ebene_fuellen(0, &testbild::bgra(w, h, k), (w * 4) as usize, h as usize)?,
            _ => {
                let [y, u, v] = testbild::yuv444p(w, h, k);
                bild.ebene_fuellen(0, &y, w as usize, h as usize)?;
                bild.ebene_fuellen(1, &u, w as usize, h as usize)?;
                bild.ebene_fuellen(2, &v, w as usize, h as usize)?;
            }
        }
        s.senden(bild.frame, (k as i64) * 16_667, k == 0)?;
        pakete.extend(s.empfangen()?);
    }
    pakete.extend(s.leeren());
    if pakete.is_empty() {
        return Err("kein Paket".into());
    }
    let mut dec = crate::software_decoder(false)?;
    let mut bilder: Vec<ffmpeg::frame::Video> = Vec::new();
    for (i, p) in pakete.iter().enumerate() {
        let mut pk = ffmpeg::Packet::copy(&p.data);
        pk.set_pts(Some(i as i64));
        crate::decoder_fuettern(&mut dec, &pk, &mut bilder);
    }
    let _ = dec.send_eof();
    let mut f = ffmpeg::frame::Video::empty();
    while dec.receive_frame(&mut f).is_ok() {
        bilder.push(f.clone());
    }
    let Some(b) = bilder.first() else { return Err("Decoder liefert kein Bild".into()) };
    log(format!("  H {name}: {} Pakete, decodiert als {:?} {}x{}", pakete.len(), b.format(), b.width(), b.height()));
    let mut bestanden = true;
    for balken in 0..8 {
        let x = (balken * w as usize) / 8 + (w as usize) / 16;
        let y = (h as usize) / 6;
        let Some((yy, cb, cr)) = abtasten(b, x, y) else {
            log("    Bild nicht abtastbar (Format unbekannt)");
            return Ok(false);
        };
        let dy = yy as i32 - testbild::BALKEN_Y[balken] as i32;
        let dcb = cb as i32 - testbild::BALKEN_CB[balken] as i32;
        let dcr = cr as i32 - testbild::BALKEN_CR[balken] as i32;
        let ok = dy.abs() <= 3 && dcb.abs() <= 3 && dcr.abs() <= 3;
        bestanden &= ok;
        log(format!(
            "    Balken {balken}: Y {:>3} (soll {:>3}, {:+})  Cb {:>3} (soll {:>3}, {:+})  Cr {:>3} (soll {:>3}, {:+}) {}",
            yy, testbild::BALKEN_Y[balken], dy, cb, testbild::BALKEN_CB[balken], dcb, cr, testbild::BALKEN_CR[balken], dcr,
            if ok { "" } else { "<-- daneben" }
        ));
    }
    log(format!("  H {name}: {}", if bestanden { "bestanden (BT.709, Vollbereich)" } else { "NICHT bestanden - Farbmatrix oder Wertebereich weichen ab" }));
    Ok(bestanden)
}

fn farbprobe() -> (bool, bool) {
    log("\n--- H Farbprobe: Testbild 1920x1080 durch hevc_nvenc (rext, 4:4:4), zurueck durch den hevc-Decoder ---");
    let mut aus = (false, false);
    match farbprobe_weg("bgra direkt (NVENC rechnet um)", AVPixelFormat::AV_PIX_FMT_BGRA, 1920, 1080) {
        Ok(b) => aus.0 = b,
        Err(e) => log(format!("  H bgra: nicht messbar: {e}")),
    }
    match farbprobe_weg("yuv444p (eigene Umrechnung)", AVPixelFormat::AV_PIX_FMT_YUV444P, 1920, 1080) {
        Ok(b) => aus.1 = b,
        Err(e) => log(format!("  H yuv444p: nicht messbar: {e}")),
    }
    aus
}

// ------------------------------------------------------------- Vollbild

fn vollbildprobe() {
    log("\n--- I Vollbild: Parametersaetze vor jedem IDR, forced-idr mitten im Lauf ---");
    crate::protokoll::fehler_verwerfen();
    let (w, h) = (1920, 1080);
    let o = Oeffnung { pix_fmt: AVPixelFormat::AV_PIX_FMT_YUV444P, fps: 60, mbit: 50, delay: 0, ..Oeffnung::vorgabe(w, h) };
    let mut s = match Sitzung::oeffnen(&o) {
        Ok(s) => s,
        Err(e) => {
            log(format!("  I: nicht messbar: {e}"));
            return;
        }
    };
    let mut bild = match Bild::neu(AVPixelFormat::AV_PIX_FMT_YUV444P, w, h) {
        Ok(b) => b,
        Err(e) => {
            log(format!("  I: {e}"));
            return;
        }
    };
    let mut pakete: Vec<Paket> = Vec::new();
    for n in 0..40usize {
        if bild.schreibbar().is_err() {
            break;
        }
        let [y, u, v] = testbild::yuv444p(w, h, n % testbild::N);
        let gefuellt = bild
            .ebene_fuellen(0, &y, w as usize, h as usize)
            .and_then(|_| bild.ebene_fuellen(1, &u, w as usize, h as usize))
            .and_then(|_| bild.ebene_fuellen(2, &v, w as usize, h as usize));
        if let Err(e) = gefuellt.and_then(|_| s.senden(bild.frame, (n as i64) * 16_667, n == 0 || n == 20)) {
            log(format!("  I: {e}"));
            return;
        }
        match s.empfangen() {
            Ok(p) => pakete.extend(p),
            Err(e) => {
                log(format!("  I: {e}"));
                return;
            }
        }
    }
    pakete.extend(s.leeren());
    let pruefen = |i: usize, p: &Paket| {
        let t = nal_typen(&p.data);
        let ps = t.contains(&32) && t.contains(&33) && t.contains(&34);
        let idr = t.iter().any(|n| (16..=21).contains(n));
        log(format!(
            "  Paket {i}: {} Byte, KEY {}, VPS/SPS/PPS {}, IRAP {}, NAL-Typen {:?}",
            p.data.len(), if p.key { "ja" } else { "nein" }, if ps { "ja" } else { "nein" }, if idr { "ja" } else { "nein" }, t
        ));
        p.key && ps && idr
    };
    let mut ok = true;
    if let Some(p) = pakete.first() {
        ok &= pruefen(0, p);
    } else {
        log("  I: keine Pakete");
        return;
    }
    if let Some(p) = pakete.get(20) {
        ok &= pruefen(20, p);
    }
    if let Some(p) = pakete.get(21) {
        let t = nal_typen(&p.data);
        log(format!("  Paket 21 (Zwischenbild): KEY {}, NAL-Typen {:?}", if p.key { "ja" } else { "nein" }, t));
        ok &= !p.key;
    }
    log(format!("  I Vollbild: {}", if ok { "bestanden" } else { "NICHT bestanden" }));
}

// ------------------------------------------------------------------ Lauf

pub fn laufen(args: &[String]) -> i32 {
    // Per-Monitor-DPI setzt main_host schon fuer alle Hostrollen.
    let sekunden: f64 = arg_wert(args, "--sekunden").and_then(|v| v.parse().ok()).unwrap_or(10.0);
    let wunsch: Option<usize> = arg_wert(args, "--output").and_then(|v| v.parse().ok());
    log(format!("=== QuadChroma Messung, {sekunden} s je Lauf ==="));

    let mut ausgaenge = Vec::new();
    aufnahme::ausgaenge_melden(&mut ausgaenge);
    super::encoder::pruefen();
    super::ffmpeg_zeilen();

    // NVIDIA-Adapter fuer D4 (Ausgang an der Intel).
    let factory: Option<IDXGIFactory1> = unsafe { CreateDXGIFactory1() }.ok();
    let nvidia: Option<IDXGIAdapter1> = factory.as_ref().and_then(|f| {
        let mut i = 0;
        while let Ok(a) = unsafe { f.EnumAdapters1(i) } {
            if unsafe { a.GetDesc1() }.map(|d| d.VendorId == 0x10de).unwrap_or(false) {
                return Some(a);
            }
            i += 1;
        }
        None
    });

    let mut entscheidungen: Vec<String> = Vec::new();
    let ziele: Vec<Ausgang> = match wunsch {
        Some(n) => ausgaenge.iter().filter(|a| a.index == n).cloned().collect(),
        None => ausgaenge.clone(),
    };
    if ziele.is_empty() {
        log("Kein Ausgang zu messen - Duplication nicht moeglich (WARP/RDP oder --output ausserhalb der Liste)");
    }
    for a in &ziele {
        let dup = match aufnahme::duplication_aufbauen(a) {
            Ok(d) => d,
            Err(e) => {
                log(format!("Ausgang {}: {e}", a.index));
                entscheidungen.push(format!("Ausgang {}: nicht messbar ({e})", a.index));
                continue;
            }
        };
        let w = dup.breite;
        let h = dup.hoehe;
        let mut laeufe: Vec<(String, Ergebnis)> = Vec::new();
        MEDIANE.with(|m| m.borrow_mut().clear());
        match messlauf(a, &dup, w, h, false, sekunden, nvidia.as_ref()) {
            Ok(mut e) => {
                e.null_kopien = d3_median();
                e.prozessor = prozessor_median();
                laeufe.push((format!("{w}x{h}"), e));
            }
            Err(e) => log(format!("  Lauf {w}x{h}: {e}")),
        }
        if w >= 3840 {
            MEDIANE.with(|m| m.borrow_mut().clear());
            match messlauf(a, &dup, 1920, 1080, true, sekunden, nvidia.as_ref()) {
                Ok(mut e) => {
                    e.null_kopien = d3_median();
                    e.prozessor = prozessor_median();
                    laeufe.push(("1920x1080 Ausschnitt".into(), e));
                }
                Err(e) => log(format!("  Lauf 1080p-Ausschnitt: {e}")),
            }
        }
        // Entscheidung je Ausgang (Abschnitt 3.3): Null-Kopien, wenn D3 da
        // und schneller als D1 und D2; sonst Prozessor. Bildrate aus dem
        // Budget: 8,3 ms fuer 120, 16,7 ms fuer 60.
        for (groesse, e) in &laeufe {
            let deckel = |ms: f64| if ms <= 8.3 { 120 } else if ms <= 16.7 { 60 } else { 30 };
            let satz = match (e.null_kopien, e.prozessor) {
                (Some(nk), Some(pz)) if nk < pz => format!("Ausgang {} ({groesse}): Null-Kopien, {nk:.1} ms je Bild, hoechstens {} fps", a.index, deckel(nk)),
                (Some(nk), None) => format!("Ausgang {} ({groesse}): Null-Kopien, {nk:.1} ms je Bild, hoechstens {} fps", a.index, deckel(nk)),
                (_, Some(pz)) => {
                    let mut s = format!("Ausgang {} ({groesse}): Prozessor, {pz:.1} ms je Bild, hoechstens {} fps", a.index, deckel(pz));
                    if e.staging > 6.0 && w < 3840 {
                        s.push_str(" - Staging allein ueber 6 ms: fuer 120 fps ungeeignet");
                    }
                    if w >= 3840 && pz > 16.0 && !groesse.contains("Ausschnitt") {
                        s.push_str(" (4K ueber den Prozessor: hoechstens 60 Bilder/s)");
                    }
                    s
                }
                (None, None) => format!("Ausgang {} ({groesse}): kein Encoderweg messbar", a.index),
            };
            entscheidungen.push(satz);
        }
    }

    let (h_bgra, h_444) = farbprobe();
    vollbildprobe();
    super::ffmpeg_zeilen();

    log("\n=== Entscheidung ===");
    for e in &entscheidungen {
        log(e);
    }
    log(format!(
        "Farbprobe: bgra direkt {}, yuv444p {} -> {}",
        if h_bgra { "bestanden" } else { "nicht bestanden" },
        if h_444 { "bestanden" } else { "nicht bestanden" },
        if h_bgra { "BGRA direkt in NVENC ist der Hauptweg fuer 4:4:4 8 Bit" } else if h_444 { "eigene Umrechnung BGRA->yuv444p noetig (Weg D2)" } else { "kein 4:4:4-Weg bestanden" }
    ));
    log("Datei: %APPDATA%\\QuadChroma\\messung.txt");
    0
}
