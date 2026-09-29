// Die Encodermessung (`quadchroma.exe --encodermessung [--groessen 1920x1080,3840x2160]
// [--bilder 300] [--kandidaten 0,1,2,3,4] [--last N]`): die Wege in den
// Encoder gegeneinander, je Kandidat und Groesse, ohne Bildschirm; mit
// --last rechnen N Faeden daneben ohne Pause (der Prozessor wie neben einem
// Spiel).
//
// Laeuft ohne Duplication - also auch bei gesperrtem Bildschirm und ueber
// SSH (Sitzung 0): das Geraet entsteht auf der NVIDIA-Karte wie das der
// Duplication, die zwoelf Bilder des Testbilds (BGRA) liegen als Texturen
// darauf und spielen den Desktop. Je Bild genau das, was der Aufnahmefaden
// tut, mit dem echten Betrieb (encoder.rs):
//   bgra (nur 8 Bit; --encoderweg bgra, Ausgang nicht an der NVIDIA):
//        CopyResource in STAGING, Map, Zeilen in den Hauptspeicher,
//        codieren(Quelle::Ram) - BGRA in nvenc, der rechnet nach BT.601
//        begrenzt um
//   Prozessor (yuv444, der Rueckfall des Wegs auf der Karte): ebenso, dann
//        die Umrechnung auf dem Prozessor (aus_bgra, BT.709 voll)
//   Karte (d3d11, die Vorgabe an der NVIDIA): CopyResource in die Kopie,
//        codieren(Quelle::Textur) - der Wandler (Modus Zehn bzw. Acht) und
//        die CUDA-Bruecke
// Gemessen je Bild die Zeit von der Kopie bis zum fertigen Paket (nvenc
// synchron, delay 0), dazu die Prozessorzeit des Prozesses ueber den Lauf
// (GetProcessTimes) je Bild. Danach der Vergleich Prozessor gegen Karte:
// beide bekommen dieselben Bilder und rechnen dieselben Ebenen - die Pakete
// (SHA-256) und, wenn sie sich unterscheiden, die ersten zwoelf Bilder durch
// den Software-Decoder muessen gleich sein. Bei der ersten Groesse dazu die
// Nebenwege jeder Sitzung (Testbild, BGRA aus dem Hauptspeicher,
// Wiederholung), ebenfalls Prozessor gegen Karte, und die Farbe jedes Wegs:
// die Farbbalken nach dem Decoder gegen BT.709 voll, und so, wie ein Client
// sie zeigt (to_rgb mit dem VUI des Stroms), gegen das RGB des Testbilds.
//
// Alle Zeilen gehen auf die Konsole und nach
// %APPDATA%\QuadChroma\encodermessung.txt (nie nach messung.txt - die liest
// die Wegentscheidung).

use std::time::Instant;

use ffmpeg_next as ffmpeg;
use sha2::{Digest, Sha256};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use super::encoder::{self, Betrieb, Paket, Quelle, StromFarbe, Weg};
use super::{arg_wert, log, testbild, Z};
use std::sync::atomic::Ordering;

/// Eine Reihe von Zeiten in ms.
#[derive(Default, Clone)]
struct Reihe(Vec<f64>);

impl Reihe {
    fn sortiert(&self) -> Vec<f64> {
        let mut v = self.0.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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
    fn mittel(&self) -> f64 {
        if self.0.is_empty() { 0.0 } else { self.0.iter().sum::<f64>() / self.0.len() as f64 }
    }
}

/// Ergebnis eines Laufs (ein Kandidat, eine Groesse, ein Weg).
struct Lauf {
    eingabe: String,
    /// Wie lange Betrieb::oeffnen brauchte (ms) - bei 10 Bit auf der Karte
    /// mit Wandler, CUDA-Geraet und Bruecke.
    oeffnen_ms: f64,
    zeiten: Reihe,
    /// Prozessorzeit des Prozesses je Bild in ms.
    cpu_ms: f64,
    pakete: Vec<Paket>,
}

/// Die NVIDIA-Karte (VendorId 0x10de), sonst die erste Hardware-Karte.
fn karte() -> Result<(IDXGIAdapter1, String), String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| format!("CreateDXGIFactory1: {e}"))?;
    let mut erste: Option<(IDXGIAdapter1, String)> = None;
    let mut i = 0;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        i += 1;
        let Ok(d) = (unsafe { a.GetDesc1() }) else { continue };
        let name = String::from_utf16_lossy(&d.Description[..d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len())]);
        if (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
            continue;
        }
        if d.VendorId == 0x10de {
            return Ok((a, name));
        }
        erste.get_or_insert((a, name));
    }
    erste.ok_or_else(|| "keine Hardware-Karte".into())
}

fn textur(device: &ID3D11Device, w: u32, h: u32, usage: D3D11_USAGE, bind: u32, cpu: u32, daten: Option<&[u8]>) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    };
    let init = daten.map(|d| D3D11_SUBRESOURCE_DATA { pSysMem: d.as_ptr() as *const _, SysMemPitch: w * 4, SysMemSlicePitch: 0 });
    let mut t = None;
    unsafe { device.CreateTexture2D(&desc, init.as_ref().map(|i| i as *const _), Some(&mut t)) }.map_err(|e| format!("CreateTexture2D: {e}"))?;
    t.ok_or_else(|| "CreateTexture2D lieferte nichts".into())
}

/// STAGING in den Hauptspeicher (w*4 Byte je Zeile) - wie Duplication::auslesen.
fn auslesen(ctx: &ID3D11DeviceContext, staging: &ID3D11Texture2D, w: u32, h: u32, ziel: &mut Vec<u8>) -> Result<(), String> {
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m)) }.map_err(|e| format!("Map: {e}"))?;
    let r = if m.pData.is_null() {
        Err("Map: kein Zeiger".to_string())
    } else {
        let laenge = m.RowPitch as usize * (h as usize - 1) + w as usize * 4;
        let quelle = unsafe { std::slice::from_raw_parts(m.pData as *const u8, laenge) };
        super::aufnahme::zeilen_holen(quelle, m.RowPitch as usize, w as usize, h as usize, ziel)
    };
    unsafe { ctx.Unmap(staging, 0) };
    r
}

/// Ein Lauf: Kandidat idx, Weg weg, n Bilder nach zehn zum Einschwingen.
#[allow(clippy::too_many_arguments)]
fn lauf(idx: usize, weg: Weg, w: u32, h: u32, n: usize, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, quellen: &[ID3D11Texture2D], lastzeit: &dyn Fn() -> u64) -> Result<Lauf, String> {
    let t_auf = Instant::now();
    let mut b = Betrieb::oeffnen(idx, w as i32, h as i32, weg, Some((dev, ctx)), StromFarbe::Sdr709)?;
    let oeffnen_ms = t_auf.elapsed().as_secs_f64() * 1000.0;
    b.mitschnitt_an();
    let kopie = textur(dev, w, h, D3D11_USAGE_DEFAULT, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0, None)?;
    let staging = textur(dev, w, h, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32, None)?;
    let mut ram = Vec::new();
    let einschwingen = 10;
    let mut zeiten = Reihe::default();
    let mut cpu0 = 0u64;
    // Beide Wege beginnen mit demselben Vollbild.
    Z.force_key.store(true, Ordering::Relaxed);
    for i in 0..einschwingen + n {
        if i == einschwingen {
            cpu0 = crate::prozesszeit_100ns().unwrap_or(0).saturating_sub(lastzeit());
        }
        let q = &quellen[i % quellen.len()];
        let t0 = Instant::now();
        let pts = (i as i64) * 1_000_000 / 60;
        let t_cap = super::now_us();
        if b.texturen() {
            unsafe { ctx.CopyResource(&kopie, q) };
            b.codieren(Quelle::Textur(&kopie), t_cap, pts, false)?;
        } else {
            unsafe { ctx.CopyResource(&staging, q) };
            auslesen(ctx, &staging, w, h, &mut ram)?;
            b.codieren(Quelle::Ram(&ram), t_cap, pts, false)?;
        }
        if i >= einschwingen {
            zeiten.0.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let cpu1 = crate::prozesszeit_100ns().unwrap_or(0).saturating_sub(lastzeit());
    let eingabe = b.eingabe().text();
    let mut pakete = b.mitschnitt_nehmen();
    b.schliessen();
    // schliessen leert den Encoder in den Mitschnitt, der mit ihm faellt -
    // bei delay 0 steht dort nichts mehr aus.
    pakete.retain(|p| !p.data.is_empty());
    Ok(Lauf { eingabe, oeffnen_ms, zeiten, cpu_ms: cpu1.saturating_sub(cpu0) as f64 / 1e4 / n.max(1) as f64, pakete })
}

/// Die Nebenwege einer Sitzung: zwoelf Testbilder (Quelle::Fertig, im
/// Eingabeformat der Sitzung), drei BGRA-Bilder aus dem Hauptspeicher
/// (Quelle::Ram) und zwei Wiederholungen - auf beiden Wegen dieselben
/// Bilder, also dieselben Pakete.
fn nebenwege(idx: usize, weg: Weg, w: u32, h: u32, dev: &ID3D11Device, ctx: &ID3D11DeviceContext) -> Result<(String, Vec<Paket>), String> {
    let mut b = Betrieb::oeffnen(idx, w as i32, h as i32, weg, Some((dev, ctx)), StromFarbe::Sdr709)?;
    b.mitschnitt_an();
    let mut tb = encoder::testbilder(b.pix_fmt, w as i32, h as i32, StromFarbe::Sdr709)?;
    Z.force_key.store(true, Ordering::Relaxed);
    let mut i = 0i64;
    let mut pts = || {
        i += 1;
        i * 1_000_000 / 60
    };
    for t in tb.iter_mut() {
        b.codieren(Quelle::Fertig(t), super::now_us(), pts(), false)?;
    }
    for k in 0..3 {
        let bgra = testbild::bgra(w as i32, h as i32, 5 + k);
        b.codieren(Quelle::Ram(&bgra), super::now_us(), pts(), false)?;
    }
    for _ in 0..2 {
        b.codieren(Quelle::Wiederholung, super::now_us(), pts(), true)?;
    }
    let eingabe = b.eingabe().text();
    let pakete = b.mitschnitt_nehmen();
    b.schliessen();
    Ok((eingabe, pakete))
}

/// Die Pakete als ein Fingerabdruck.
fn fingerabdruck(p: &[Paket]) -> [u8; 32] {
    let mut h = Sha256::new();
    for x in p {
        h.update((x.data.len() as u64).to_le_bytes());
        h.update(&x.data);
    }
    h.finalize().into()
}

/// Die ersten `n` Bilder der Pakete durch den Software-Decoder.
fn decodieren(pakete: &[Paket], h264: bool, n: usize) -> Result<Vec<ffmpeg::frame::Video>, String> {
    let mut dec = crate::software_decoder(h264)?;
    let mut bilder = Vec::new();
    for (i, p) in pakete.iter().enumerate() {
        let mut pk = ffmpeg::Packet::copy(&p.data);
        pk.set_pts(Some(i as i64));
        crate::decoder_fuettern(&mut dec, &pk, &mut bilder);
        if bilder.len() >= n {
            break;
        }
    }
    let _ = dec.send_eof();
    let mut f = ffmpeg::frame::Video::empty();
    while bilder.len() < n && dec.receive_frame(&mut f).is_ok() {
        bilder.push(f.clone());
    }
    bilder.truncate(n);
    Ok(bilder)
}

/// Zwei decodierte Bilder Ebene fuer Ebene: (groesste Abweichung, Zahl
/// abweichender Werte); Err bei anderem Format oder anderer Groesse.
fn vergleichen(a: &ffmpeg::frame::Video, b: &ffmpeg::frame::Video) -> Result<(u32, usize), String> {
    if a.format() != b.format() || a.width() != b.width() || a.height() != b.height() {
        return Err(format!("{:?} {}x{} gegen {:?} {}x{}", a.format(), a.width(), a.height(), b.format(), b.width(), b.height()));
    }
    use ffmpeg::format::Pixel::*;
    let zwei = matches!(a.format(), YUV444P10LE | YUV420P10LE | YUV422P10LE | YUV444P12LE | YUV420P12LE | YUV444P16LE | YUV420P16LE);
    let mut max = 0u32;
    let mut anders = 0usize;
    for e in 0..a.planes() {
        let (pw, ph) = (a.plane_width(e) as usize, a.plane_height(e) as usize);
        let bytes = if zwei { 2 * pw } else { pw };
        for y in 0..ph {
            let za = &a.data(e)[y * a.stride(e)..y * a.stride(e) + bytes];
            let zb = &b.data(e)[y * b.stride(e)..y * b.stride(e) + bytes];
            if zwei {
                for (x, z) in za.chunks_exact(2).zip(zb.chunks_exact(2)) {
                    let d = (u16::from_le_bytes([x[0], x[1]]) as i32 - u16::from_le_bytes([z[0], z[1]]) as i32).unsigned_abs();
                    max = max.max(d);
                    anders += (d != 0) as usize;
                }
            } else {
                for (x, z) in za.iter().zip(zb) {
                    let d = (*x as i32 - *z as i32).unsigned_abs();
                    max = max.max(d);
                    anders += (d != 0) as usize;
                }
            }
        }
    }
    Ok((max, anders))
}

/// Gleichheit der beiden Wege als Text.
fn gleichheit(alt: &Lauf, neu: &Lauf, h264: bool) -> String {
    if fingerabdruck(&alt.pakete) == fingerabdruck(&neu.pakete) {
        return format!("Bitstrom gleich ({} Pakete, SHA-256)", alt.pakete.len());
    }
    let n = testbild::N;
    match (decodieren(&alt.pakete, h264, n), decodieren(&neu.pakete, h264, n)) {
        (Ok(a), Ok(b)) if !a.is_empty() && a.len() == b.len() => {
            let mut max = 0;
            let mut anders = 0;
            for (x, y) in a.iter().zip(&b) {
                match vergleichen(x, y) {
                    Ok((m, z)) => {
                        max = max.max(m);
                        anders += z;
                    }
                    Err(e) => return format!("Bitstrom anders, Bilder nicht vergleichbar: {e}"),
                }
            }
            format!("Bitstrom anders, die ersten {} Bilder decodiert: groesste Abweichung {max}, {anders} Werte anders", a.len())
        }
        (Ok(a), Ok(b)) => format!("Bitstrom anders, decodiert {} gegen {} Bilder", a.len(), b.len()),
        (Err(e), _) | (_, Err(e)) => format!("Bitstrom anders, Decoder: {e}"),
    }
}

/// Kernel- plus Nutzerzeit eines Fadens in 100 ns (GetThreadTimes).
fn faden_zeit(f: &std::thread::JoinHandle<u64>) -> u64 {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{FILETIME, HANDLE};
    let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let h = HANDLE(f.as_raw_handle());
    if unsafe { windows::Win32::System::Threading::GetThreadTimes(h, &mut a, &mut b, &mut k, &mut u) }.is_err() {
        return 0;
    }
    let z = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    z(k) + z(u)
}

/// Die Farbbalken des ersten Bildes (Testbild, BGRA) nach nvenc und dem
/// Software-Decoder: die Werte gegen die Balkenwerte des Testbilds (BT.709
/// voll - so kommt ein Strom dieses Hosts an), in 8-Bit-Stufen (10 Bit
/// durch 4); dazu das Bild so, wie ein Client es zeigt (to_rgb: Matrix und
/// Bereich aus dem VUI), gegen das RGB des Testbilds - das muss auch der
/// BT.601-Strom des bgra-Wegs treffen. Liefert (Text, Abweichung Y/Cb/Cr,
/// Abweichung RGB).
fn farbe(pakete: &[Paket], h264: bool, w: u32, h: u32) -> (String, i32, i32) {
    let bilder = match decodieren(pakete, h264, 1) {
        Ok(b) => b,
        Err(e) => return (format!("Farbe nicht pruefbar: {e}"), i32::MAX, i32::MAX),
    };
    let Some(b) = bilder.first() else { return ("Farbe nicht pruefbar: kein Bild".into(), i32::MAX, i32::MAX) };
    use ffmpeg::format::Pixel::*;
    let zehn = matches!(b.format(), YUV444P10LE | YUV420P10LE);
    let halb = matches!(b.format(), YUV420P | YUV420P10LE | YUVJ420P);
    let wert = |ebene: usize, x: usize, y: usize| -> Option<i32> {
        let (x, y) = if ebene > 0 && halb { (x / 2, y / 2) } else { (x, y) };
        let d = b.data(ebene);
        if zehn {
            let i = y * b.stride(ebene) + 2 * x;
            Some((u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]) as i32 + 2) / 4)
        } else {
            d.get(y * b.stride(ebene) + x).map(|v| *v as i32)
        }
    };
    let rgb = crate::to_rgb(b);
    let quelle = testbild::bgra(w as i32, h as i32, 0);
    let mut max = 0;
    let mut max_rgb = 0;
    let mut werte = Vec::new();
    let mut werte_rgb = Vec::new();
    for balken in 0..8usize {
        let (x, y) = (balken * w as usize / 8 + w as usize / 16, h as usize / 6);
        let (Some(yy), Some(cb), Some(cr)) = (wert(0, x, y), wert(1, x, y), wert(2, x, y)) else {
            return (format!("Farbe nicht pruefbar ({:?})", b.format()), i32::MAX, i32::MAX);
        };
        max = max
            .max((yy - testbild::BALKEN_Y[balken] as i32).abs())
            .max((cb - testbild::BALKEN_CB[balken] as i32).abs())
            .max((cr - testbild::BALKEN_CR[balken] as i32).abs());
        werte.push(format!("{yy}/{cb}/{cr} (soll {}/{}/{})", testbild::BALKEN_Y[balken], testbild::BALKEN_CB[balken], testbild::BALKEN_CR[balken]));
        let i = (y * w as usize + x) * 4;
        let soll = [quelle[i + 2] as i32, quelle[i + 1] as i32, quelle[i] as i32];
        match rgb.as_ref() {
            Ok(f) => {
                let p = f.pixels[y * f.width as usize + x];
                let ist = [((p >> 16) & 255) as i32, ((p >> 8) & 255) as i32, (p & 255) as i32];
                max_rgb = (0..3).map(|k| (ist[k] - soll[k]).abs()).fold(max_rgb, i32::max);
                werte_rgb.push(format!("{}/{}/{} (soll {}/{}/{})", ist[0], ist[1], ist[2], soll[0], soll[1], soll[2]));
            }
            Err(_) => max_rgb = i32::MAX,
        }
    }
    let rgb_text = match rgb {
        Ok(_) => format!(
            "; wie der Client es zeigt (Matrix und Bereich aus dem VUI): hoechstens {max_rgb} Stufen neben dem RGB des Testbilds{}",
            if max_rgb <= 3 { String::new() } else { format!(" - DANEBEN: R/G/B {}", werte_rgb.join(", ")) }
        ),
        Err(e) => format!("; to_rgb: {e}"),
    };
    let text = format!(
        "Farbbalken nach dem Decoder ({:?}, VUI Matrix {:?}, {:?}): hoechstens {max} Stufen neben BT.709 voll{}{rgb_text}",
        b.format(),
        b.color_space(),
        b.color_range(),
        if max <= 3 { String::new() } else { format!(" - DANEBEN: Y/Cb/Cr {}", werte.join(", ")) }
    );
    (text, max, max_rgb)
}

fn groessen(args: &[String]) -> Vec<(u32, u32)> {
    let text = arg_wert(args, "--groessen").unwrap_or_else(|| "1920x1080,3840x2160".into());
    text.split(',')
        .filter_map(|g| {
            let (w, h) = g.trim().split_once('x')?;
            let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
            (w >= 64 && h >= 64 && w <= 8192 && h <= 8192).then_some((w & !1, h & !1))
        })
        .collect()
}

pub fn laufen(args: &[String]) -> i32 {
    let n: usize = arg_wert(args, "--bilder").and_then(|v| v.parse().ok()).unwrap_or(300).clamp(10, 100_000);
    let kandidaten: Vec<usize> = arg_wert(args, "--kandidaten")
        .map(|t| t.split(',').filter_map(|x| x.trim().parse().ok()).filter(|&i| i < encoder::KANDIDATEN.len()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4]);
    let kerne = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    // --last N: N Faeden rechnen waehrenddessen ohne Pause (normale
    // Prioritaet) - so ungefaehr steht der Prozessor neben einem Spiel.
    let last: usize = arg_wert(args, "--last").and_then(|v| v.parse().ok()).unwrap_or(0).min(256);
    let halt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lastfaeden: Vec<_> = (0..last)
        .map(|_| {
            let halt = halt.clone();
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !halt.load(Ordering::Relaxed) {
                    for i in 0..10_000u64 {
                        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
                    }
                }
                x
            })
        })
        .collect();
    // Die Prozessorzeit der Lastfaeden zaehlt nicht zum Lauf.
    let lastzeit = || -> u64 { lastfaeden.iter().map(faden_zeit).sum() };
    log(format!(
        "=== QuadChroma Encodermessung: die Wege in den Encoder (bgra, Prozessor, Karte), {n} Bilder je Lauf, {kerne} logische Kerne{} ===",
        if last > 0 { format!(", {last} Lastfaeden daneben") } else { String::new() }
    ));
    super::encoder::pruefen();
    super::ffmpeg_zeilen();
    let (adapter, name) = match karte() {
        Ok(k) => k,
        Err(e) => {
            log(format!("Keine Karte: {e}"));
            return 3;
        }
    };
    let (dev, ctx) = match crate::anzeige::geraet_bauen(Some(&adapter), false, false) {
        Ok((d, c, _)) => (d, c),
        Err(e) => {
            log(format!("Kein Geraet auf {name}: {e}"));
            return 3;
        }
    };
    if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    log(format!("Karte: {name}; Einstellungen des Hosts: {} fps, {} Mbit/s (Voreinstellung)", Z.fps.load(Ordering::Relaxed), Z.mbit.load(Ordering::Relaxed)));
    let mut tafel: Vec<String> = Vec::new();
    for (nr, (w, h)) in groessen(args).into_iter().enumerate() {
        let erste_groesse = nr == 0;
        log(format!("\n--- {w}x{h} ---"));
        let quellen: Result<Vec<ID3D11Texture2D>, String> =
            (0..testbild::N).map(|k| textur(&dev, w, h, D3D11_USAGE_DEFAULT, 0, 0, Some(&testbild::bgra(w as i32, h as i32, k)))).collect();
        let quellen = match quellen {
            Ok(q) => q,
            Err(e) => {
                log(format!("  Testbild als Textur: {e}"));
                continue;
            }
        };
        for &idx in &kandidaten {
            let k = encoder::kandidat(idx);
            let befund = encoder::befund(idx);
            if !befund.vorhanden {
                log(format!("  Kandidat {idx} {}: auf diesem Rechner nicht vorhanden", k.name));
                continue;
            }
            // Die Wege dieses Kandidaten, jeder nur einmal: bgra nur, wo nvenc
            // dort selbst umrechnet (8 Bit) - bei 10 Bit ist bgra schon die
            // Umrechnung auf dem Prozessor.
            let eingabe = |weg: Weg| encoder::eingabe_waehlen(idx, weg, befund.encoder, false, false);
            let mut wege: Vec<(Weg, &str)> = vec![(Weg::Yuv444, "Prozessor"), (Weg::D3d11, "Karte")];
            if eingabe(Weg::Bgra) != eingabe(Weg::Yuv444) {
                wege.insert(0, (Weg::Bgra, "bgra"));
            }
            let mut ergebnis: Vec<(&str, Option<Lauf>)> = Vec::new();
            for &(weg, was) in &wege {
                match lauf(idx, weg, w, h, n, &dev, &ctx, &quellen, &lastzeit) {
                    Ok(l) => {
                        log(format!(
                            "  Kandidat {idx} {:<18} {was:<9} ({}, geoeffnet in {:.0} ms): Median {:>6.2} ms, 95 % {:>6.2} ms, Mittel {:>6.2} ms je Bild, Prozessorzeit {:>6.2} ms je Bild ({:.0} % eines Kerns bei 60 Bildern/s, {:.1} % aller Kerne)",
                            k.name,
                            l.eingabe,
                            l.oeffnen_ms,
                            l.zeiten.median(),
                            l.zeiten.p95(),
                            l.zeiten.mittel(),
                            l.cpu_ms,
                            l.cpu_ms * 60.0 / 10.0,
                            l.cpu_ms * 60.0 / 10.0 / kerne as f64
                        ));
                        ergebnis.push((was, Some(l)));
                    }
                    Err(e) => {
                        log(format!("  Kandidat {idx} {:<18} {was:<9}: nicht messbar: {e}", k.name));
                        ergebnis.push((was, None));
                    }
                }
            }
            if erste_groesse {
                match (nebenwege(idx, Weg::Yuv444, w, h, &dev, &ctx), nebenwege(idx, Weg::D3d11, w, h, &dev, &ctx)) {
                    (Ok((_, a)), Ok((e, b))) => log(format!(
                        "  Kandidat {idx} {:<18} Nebenwege ({e}: Testbild, BGRA aus dem Hauptspeicher, Wiederholung): {}",
                        k.name,
                        if !a.is_empty() && fingerabdruck(&a) == fingerabdruck(&b) { format!("Bitstrom gleich wie auf dem Prozessor ({} Pakete)", a.len()) } else { format!("Bitstrom ANDERS ({} gegen {} Pakete)", a.len(), b.len()) }
                    )),
                    (Err(e), _) | (_, Err(e)) => log(format!("  Kandidat {idx} {:<18} Nebenwege: {e}", k.name)),
                }
                for (was, l) in &ergebnis {
                    if let Some(l) = l {
                        let (text, _, _) = farbe(&l.pakete, k.h264, w, h);
                        log(format!("  Kandidat {idx} {:<18} {was:<9} {text}", k.name));
                    }
                }
            }
            let finden = |name: &str| ergebnis.iter().find(|(was, _)| *was == name).and_then(|(_, l)| l.as_ref());
            let zelle = |name: &str| match finden(name) {
                Some(l) => format!("{name} {:>6.2} / {:>6.2} ms, Prozessor {:>6.2} ms", l.zeiten.median(), l.zeiten.p95(), l.cpu_ms),
                None => format!("{name} -"),
            };
            if let (Some(prozessor), Some(karte)) = (finden("Prozessor"), finden("Karte")) {
                let g = gleichheit(prozessor, karte, k.h264);
                log(format!("  Kandidat {idx} {:<18} Prozessor gegen Karte: {g}", k.name));
                let mut zeile = format!("{w}x{h} | {:<18} | ", k.name);
                if finden("bgra").is_some() {
                    zeile += &format!("{} | ", zelle("bgra"));
                }
                zeile += &format!("{} | {} | {}", zelle("Prozessor"), zelle("Karte"), if g.starts_with("Bitstrom gleich") { "gleich".to_string() } else { g });
                tafel.push(zeile);
            }
        }
    }
    log("\n=== Uebersicht (Median / 95 % je Bild von der Kopie bis zum Paket; Prozessorzeit des Prozesses je Bild; Prozessor gegen Karte) ===");
    for z in &tafel {
        log(z);
    }
    halt.store(true, Ordering::Relaxed);
    for f in lastfaeden {
        let _ = f.join();
    }
    log("Datei: %APPDATA%\\QuadChroma\\encodermessung.txt");
    0
}
