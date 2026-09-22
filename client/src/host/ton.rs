// Ton des Windows-Hosts: WASAPI-Loopback auf dem Standard-Ausgabegeraet ->
// float32 Stereo -> Nachricht 32 (Format, einmal je Zuschauer) und 33
// (Pakete), wie audio.m auf dem Mac.
//
// Loopback liefert KEINE Pakete, solange nichts spielt - dann liefe der
// Ring des Clients leer und hinkte nach dem Wiedereinsetzen. Deshalb laeuft
// im selben Prozess ein stiller Wiedergabestrom auf demselben Endpunkt
// (IAudioRenderClient mit AUDCLNT_BUFFERFLAGS_SILENT); damit kommt der
// Loopback durchgehend. Dazu die Lueckenfuellung wie bridge_gap in audio.m
// ueber die QPC-Position der Pakete (Luecke groesser als ein Paket und
// hoechstens 5 s -> Stille derselben Laenge, einmal knacken statt fuer immer
// versetzt).
//
// Das Mischformat des geteilten Modus ist float32 (sonst wird gewandelt:
// int16/24/32), meist 48 kHz, 2 bis 8 Kanaele: gemischt wird IMMER auf
// Stereo (L = Kanal 0, R = Kanal 1, bei 5.1/7.1 Mitte und Rueckseite
// anteilig); die Rate bleibt die des Geraets, Nachricht 32 sagt sie, der
// Client rechnet um. ton=0 laesst den Abgriff laufen und sendet nichts.
//
// Kein Zuschauer, kein Abgriff: der Faden wartet, oeffnet das Geraet fuer
// die Dauer eines Zuschauers und schliesst es danach. Ohne Tongeraet (VM)
// steht der Grund einmal im Protokoll, der Host laeuft ohne Ton weiter.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};

use super::{log, netz, Z};
use crate::protokoll_konst::{MSG_AUDIO, MSG_AUDIO_INFO};

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_IEEE_FLOAT: GUID = GUID { data1: 3, data2: 0, data3: 0x10, data4: [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71] };
const SUBTYPE_PCM: GUID = GUID { data1: 1, data2: 0, data3: 0x10, data4: [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71] };

/// Laenge eines Stille-Stuecks (Rahmen) beim Fuellen einer Luecke.
const STILLE_RAHMEN: usize = 2048;
/// Laenger ist keine Luecke mehr, sondern eine Pause: nur neu einrasten.
const LUECKE_MAX_S: f64 = 5.0;
/// Puffer beider Stroeme in 100-ns-Einheiten (200 ms).
const PUFFER_HNS: i64 = 2_000_000;

/// Formatansage 32 schon raus? Vom Annahmefaden je Zuschauer zurueckgesetzt.
static INFO_GESENDET: AtomicBool = AtomicBool::new(false);

pub fn info_zuruecksetzen() {
    INFO_GESENDET.store(false, Ordering::Relaxed);
}

/// Ein Paket auf die Leitung (float32 Stereo verschachtelt). Ton aus:
/// nichts senden, die Formatansage bleibt fuer das erste Paket danach liegen.
fn senden(pcm: &[f32], rate: u32) {
    if !Z.ton.load(Ordering::Relaxed) || !netz::zuschauer_da() {
        return;
    }
    if !INFO_GESENDET.swap(true, Ordering::Relaxed) {
        let mut info = [0u8; 8];
        info[0..4].copy_from_slice(&rate.to_le_bytes());
        info[4] = 2;
        info[5] = 1; // float32, verschachtelt
        netz::send_small(MSG_AUDIO_INFO, &info);
    }
    let mut b = Vec::with_capacity(pcm.len() * 4);
    for v in pcm {
        b.extend_from_slice(&v.to_le_bytes());
    }
    Z.audio_packets.fetch_add(1, Ordering::Relaxed);
    Z.audio_bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
    netz::send_small(MSG_AUDIO, &b);
}

fn stille_senden(rahmen: usize, rate: u32) {
    let null = vec![0f32; STILLE_RAHMEN * 2];
    let mut rest = rahmen;
    while rest > 0 {
        let n = rest.min(STILLE_RAHMEN);
        senden(&null[..n * 2], rate);
        rest -= n;
    }
}

/// Das Geraeteformat: Rate, Kanaele, Bits je Wert, gueltige Bits, float?
struct Format {
    rate: u32,
    kanaele: usize,
    bits: u16,
    gueltig: u16,
    float: bool,
}

unsafe fn format_lesen(f: *const WAVEFORMATEX) -> Format {
    let tag = (*f).wFormatTag;
    let bits = (*f).wBitsPerSample;
    let cb = (*f).cbSize as usize;
    let mut gueltig = bits;
    let float = match tag {
        WAVE_FORMAT_IEEE_FLOAT => bits == 32,
        WAVE_FORMAT_EXTENSIBLE => {
            let zusatz = std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>();
            if cb >= zusatz {
                let ext = f as *const WAVEFORMATEXTENSIBLE;
                let sub = (*ext).SubFormat;
                gueltig = (*ext).Samples.wValidBitsPerSample;
                bits == 32 && sub == SUBTYPE_IEEE_FLOAT
            } else {
                false
            }
        }
        _ => false,
    };
    // Was nicht float ist, muss PCM sein - sonst kommt Rauschen auf die Leitung.
    // WAVEFORMATEX ist gepackt: Felder nur als Kopie lesen.
    let pcm = tag == WAVE_FORMAT_PCM || (tag == WAVE_FORMAT_EXTENSIBLE && cb >= 22 && {
        let sub = (*(f as *const WAVEFORMATEXTENSIBLE)).SubFormat;
        sub == SUBTYPE_PCM
    });
    Format { rate: (*f).nSamplesPerSec, kanaele: (*f).nChannels as usize, bits: if float || pcm { bits } else { 0 }, gueltig: if gueltig == 0 { bits } else { gueltig }, float }
}

/// Ein Rahmen (alle Kanaele) aus den Rohdaten als float.
#[inline]
unsafe fn wert(p: *const u8, f: &Format, i: usize) -> f32 {
    match (f.float, f.bits) {
        (true, 32) => *(p.add(i * 4) as *const f32),
        (false, 16) => *(p.add(i * 2) as *const i16) as f32 / 32768.0,
        (false, 24) => {
            let b = p.add(i * 3);
            let v = ((*b as i32) << 8 | (*b.add(1) as i32) << 16 | (*b.add(2) as i32) << 24) >> 8;
            v as f32 / 8388608.0
        }
        (false, 32) => {
            let v = *(p.add(i * 4) as *const i32);
            if f.gueltig == 24 { (v >> 8) as f32 / 8388608.0 } else { v as f32 / 2147483648.0 }
        }
        _ => 0.0,
    }
}

/// Auf Stereo mischen: L = 0, R = 1, Mitte (2) und Rueckseite (4/5) anteilig.
fn mischen(p: *const u8, f: &Format, rahmen: usize, aus: &mut Vec<f32>) {
    aus.clear();
    aus.reserve(rahmen * 2);
    let k = f.kanaele;
    for r in 0..rahmen {
        let v = |c: usize| unsafe { if c < k { wert(p, f, r * k + c) } else { 0.0 } };
        let (mut l, mut rr) = (v(0), if k >= 2 { v(1) } else { v(0) });
        if k >= 3 {
            l += 0.707 * v(2);
            rr += 0.707 * v(2);
        }
        if k >= 6 {
            l += 0.707 * v(4);
            rr += 0.707 * v(5);
        }
        aus.push(l.clamp(-1.0, 1.0));
        aus.push(rr.clamp(-1.0, 1.0));
    }
}

/// Eine Tonsitzung fuer die Dauer eines Zuschauers. Liefert Err mit dem
/// Grund, wenn das Geraet nicht aufgeht.
unsafe fn sitzung() -> Result<(), String> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let aufzaehler: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("Geraeteliste: {e}"))?;
    let geraet = aufzaehler.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| format!("kein Standard-Ausgabegeraet: {e}"))?;

    // Abgriff (Loopback) und stiller Wiedergabestrom auf demselben Endpunkt,
    // beide im Mischformat.
    let abgriff: IAudioClient = geraet.Activate(CLSCTX_ALL, None).map_err(|e| format!("Geraet oeffnen: {e}"))?;
    let mix = abgriff.GetMixFormat().map_err(|e| format!("Geraeteformat: {e}"))?;
    if mix.is_null() {
        return Err("Geraet nennt kein Format".into());
    }
    let f = format_lesen(mix);
    if f.rate == 0 || f.kanaele == 0 || !(f.float || matches!(f.bits, 16 | 24 | 32)) {
        CoTaskMemFree(Some(mix as *const c_void));
        return Err(format!("Geraeteformat nicht verwendbar ({} Hz, {} Kanaele, {} Bit)", f.rate, f.kanaele, f.bits));
    }
    let r = abgriff.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, PUFFER_HNS, 0, mix, None);
    let wiedergabe: Result<IAudioClient, String> = geraet.Activate(CLSCTX_ALL, None).map_err(|e| format!("Geraet oeffnen (Wiedergabe): {e}"));
    let r2 = wiedergabe.as_ref().ok().map(|w| w.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, PUFFER_HNS, 0, mix, None));
    CoTaskMemFree(Some(mix as *const c_void));
    r.map_err(|e| format!("Loopback einrichten: {e}"))?;
    let wiedergabe = wiedergabe?;
    r2.unwrap().map_err(|e| format!("stillen Wiedergabestrom einrichten: {e}"))?;

    let capture: IAudioCaptureClient = abgriff.GetService().map_err(|e| format!("Abgriff: {e}"))?;
    let render: IAudioRenderClient = wiedergabe.GetService().map_err(|e| format!("Wiedergabe: {e}"))?;
    let render_groesse = wiedergabe.GetBufferSize().map_err(|e| format!("Puffergroesse: {e}"))?;

    // Stille vorlegen, dann beide starten.
    if let Ok(p) = render.GetBuffer(render_groesse) {
        if !p.is_null() {
            let _ = render.ReleaseBuffer(render_groesse, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
        }
    }
    wiedergabe.Start().map_err(|e| format!("Wiedergabe starten: {e}"))?;
    abgriff.Start().map_err(|e| format!("Abgriff starten: {e}"))?;
    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    log(format!(
        "Ton angemeldet: {} Hz, {} Kanaele ({}), gemischt auf 2 Kanaele float32 verschachtelt, stiller Wiedergabestrom laeuft",
        f.rate, f.kanaele, if f.float { "float32".to_string() } else { format!("{} Bit", f.bits) }
    ));

    let mut pcm: Vec<f32> = Vec::new();
    let mut erwartet_qpc: Option<u64> = None;
    let paket_s = 0.02f64; // Loopback-Pakete sind 10 ms, die Toleranz ist ein Paket plus etwas
    while netz::zuschauer_da() {
        // Der stille Strom bleibt voll.
        if let Ok(belegt) = wiedergabe.GetCurrentPadding() {
            let frei = render_groesse.saturating_sub(belegt);
            if frei > render_groesse / 2 {
                if let Ok(p) = render.GetBuffer(frei) {
                    if !p.is_null() {
                        let _ = render.ReleaseBuffer(frei, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
                    }
                }
            }
        }
        // Alles abholen, was der Abgriff hat.
        loop {
            let n = capture.GetNextPacketSize().map_err(|e| format!("Abgriff verloren: {e}"))?;
            if n == 0 {
                break;
            }
            let mut daten: *mut u8 = std::ptr::null_mut();
            let mut rahmen = 0u32;
            let mut flags = 0u32;
            let mut qpc = 0u64;
            capture.GetBuffer(&mut daten, &mut rahmen, &mut flags, None, Some(&mut qpc)).map_err(|e| format!("Abgriff verloren: {e}"))?;
            let rahmen_n = rahmen as usize;
            if rahmen_n > 0 {
                // Luecke? (QPC-Position in 100-ns-Einheiten)
                if let Some(erw) = erwartet_qpc {
                    let luecke = (qpc as f64 - erw as f64) / 1e7;
                    if luecke > paket_s && luecke <= LUECKE_MAX_S {
                        log(format!("Tonluecke {:.0} ms, mit Stille aufgefuellt", luecke * 1000.0));
                        stille_senden((luecke * f.rate as f64 + 0.5) as usize, f.rate);
                    }
                }
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || daten.is_null() {
                    pcm.clear();
                    pcm.resize(rahmen_n * 2, 0.0);
                } else {
                    mischen(daten, &f, rahmen_n, &mut pcm);
                }
                senden(&pcm, f.rate);
                erwartet_qpc = Some(qpc + (rahmen_n as u64 * 10_000_000) / f.rate as u64);
            }
            let _ = capture.ReleaseBuffer(rahmen);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = abgriff.Stop();
    let _ = wiedergabe.Stop();
    Ok(())
}

/// Der Tonfaden: wartet auf einen Zuschauer, greift ab, solange er da ist.
pub fn start() {
    std::thread::Builder::new()
        .name("quadchroma-ton".into())
        .spawn(|| {
            let mut letzter_fehler = String::new();
            loop {
                while !netz::zuschauer_da() {
                    std::thread::sleep(Duration::from_millis(200));
                }
                let r = std::panic::catch_unwind(|| unsafe { sitzung() });
                match r {
                    Ok(Ok(())) => log("Ton: Abgriff beendet (kein Zuschauer)"),
                    Ok(Err(e)) => {
                        if e != letzter_fehler {
                            log(format!("Ton: {e} - der Host laeuft ohne Ton"));
                            letzter_fehler = e;
                        }
                        // Nicht je Sekunde neu probieren: erst mit dem naechsten Zuschauer.
                        while netz::zuschauer_da() {
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                    Err(_) => {
                        log("Ton: Abgriff abgestuerzt - der Host laeuft ohne Ton");
                        while netz::zuschauer_da() {
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
            }
        })
        .ok();
}
