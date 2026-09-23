// Ton des Windows-Hosts: WASAPI-Loopback auf dem Standard-Ausgabegeraet ->
// float32 Stereo -> Nachricht 32 (Format, je Zuschauer und neuer Rate) und 33
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
// die Dauer eines Zuschauers und schliesst es danach. Geht das Geraet
// verloren (abgezogen, Format umgestellt) oder wechselt das Standard-
// Ausgabegeraet, wird das dann aktuelle neu geoeffnet, jede Sekunde ein
// Versuch, solange jemand zuschaut - wie der Client (audio.rs); eine andere
// Rate geht mit einer neuen 32 hinaus. Ohne Tongeraet (VM) steht der Grund
// einmal im Protokoll, der Host laeuft ohne Ton weiter und versucht es
// weiter, solange jemand zuschaut.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
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
/// Nach einem Verlust oder wenn kein Geraet aufgeht: Abstand zwischen zwei
/// Versuchen, solange jemand zuschaut.
const NEU_MS: u64 = 1000;
/// So oft wird nachgesehen, ob Windows inzwischen ein anderes
/// Standard-Ausgabegeraet hat.
const STANDARD_PRUEFEN: Duration = Duration::from_secs(1);
/// Lief ein Geraet mindestens so lange, gilt der naechste Verlust wieder als
/// neu und kommt ins Protokoll; kuerzer wird derselbe Grund nur gezaehlt.
const STABIL: Duration = Duration::from_secs(30);

/// Rate der Formatansage 32, die der Zuschauer zuletzt bekam (0 = noch
/// keine). Vom Annahmefaden je Zuschauer zurueckgesetzt.
static INFO_RATE: AtomicU32 = AtomicU32::new(0);

pub fn info_zuruecksetzen() {
    INFO_RATE.store(0, Ordering::Relaxed);
}

/// Ist eine Formatansage faellig? Ja fuer den ersten Ton je Zuschauer und
/// fuer jede neue Rate (anderes Geraet, Format umgestellt) - nicht bei
/// gleicher Rate: der Client baut seine Ausgabe bei jeder 32 neu auf.
/// Kanaele und Format sind immer dieselben (2, float32).
fn ansage_faellig(angesagt: &AtomicU32, rate: u32) -> bool {
    angesagt.swap(rate, Ordering::Relaxed) != rate
}

/// Ein Paket auf die Leitung (float32 Stereo verschachtelt). Ton aus:
/// nichts senden, die Formatansage bleibt fuer das erste Paket danach liegen.
fn senden(pcm: &[f32], rate: u32) {
    if !Z.ton.load(Ordering::Relaxed) || !netz::zuschauer_da() {
        return;
    }
    if ansage_faellig(&INFO_RATE, rate) {
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

// ------------------------------------------------------------ Wiederaufbau

/// Was der Tonfaden mit einem Geraet tut. Abgetrennt wie im Client
/// (audio.rs), damit sich der Wiederaufbau ohne Tongeraet pruefen laesst
/// (Tests unten); das echte Geraet ist `Wasapi`.
trait Treiber {
    type Geraet;
    /// Das aktuelle Standard-Ausgabegeraet oeffnen und beide Stroeme
    /// starten. Liefert das Geraet und sein Format fuers Protokoll.
    fn oeffnen(&mut self) -> Result<(Self::Geraet, String), String>;
    /// Abgreifen und senden, solange `da()` gilt (dann None), oder bis das
    /// Geraet nicht mehr taugt (Some(grund)).
    fn pumpen(&mut self, geraet: &Self::Geraet, da: &dyn Fn() -> bool) -> Option<String>;
}

/// Der Ablauf fuer die Dauer eines Zuschauers: oeffnen, abgreifen, und wenn
/// das Geraet verloren geht (abgezogen, Format umgestellt, anderes
/// Standardgeraet) oder gar nicht erst aufgeht (kein Standard-
/// Ausgabegeraet), nach `pause` das dann aktuelle Standardgeraet neu oeffnen
/// - so lange, bis es klappt oder der Zuschauer geht. Kein Zuschauer, keine
/// Versuche. Ohne das bliebe der Ton nach einem Verlust bis zum naechsten
/// Zuschauer weg. Eine andere Rate sagt senden() von selbst neu an (32).
///
/// Protokoll ohne Flut, wie im Client: derselbe Fehler beim Oeffnen steht
/// einmal da (`zuletzt` ueberdauert den Zuschauer; erst ein Geraet, das
/// aufgeht, setzt ihn zurueck). Kommt derselbe Verlustgrund wie zuletzt,
/// bevor das Geraet `stabil` lang lief, wird er nur gezaehlt (auch die
/// Anmeldezeile entfaellt dann, ausser das Format ist ein anderes), und die
/// Zahl steht einmal da, wenn die Folge endet.
fn betreiben<T: Treiber>(
    t: &mut T,
    da: &dyn Fn() -> bool,
    pause: Duration,
    stabil: Duration,
    zuletzt: &mut Option<String>,
    melden: &mut dyn FnMut(String),
) {
    // Zuletzt protokollierter Verlustgrund und wie oft er seitdem still
    // wiederkam; Format des zuletzt gemeldeten Geraets.
    let mut folge: Option<(String, u32)> = None;
    let mut still = false;
    let mut gemeldet: Option<String> = None;
    while da() {
        let (geraet, format) = match t.oeffnen() {
            Ok(g) => g,
            Err(e) => {
                if zuletzt.as_deref() != Some(e.as_str()) {
                    melden(format!("Ton: {e} - der Host laeuft ohne Ton, neuer Versuch alle {} ms, solange jemand zuschaut", pause.as_millis()));
                    *zuletzt = Some(e);
                    // Wer den Fehler liest, soll auch lesen, dass es danach
                    // wieder ging.
                    still = false;
                }
                if warten(da, pause) {
                    break;
                }
                continue;
            }
        };
        *zuletzt = None;
        if !still || gemeldet.as_deref() != Some(format.as_str()) {
            melden(format!("Ton {}: {format}", if gemeldet.is_some() { "wieder angemeldet" } else { "angemeldet" }));
            gemeldet = Some(format);
        }
        let seit = Instant::now();
        let grund = t.pumpen(&geraet, da);
        drop(geraet);
        let Some(grund) = grund else {
            folge_abschliessen(&mut folge, melden);
            melden("Ton: Abgriff beendet (kein Zuschauer)".into());
            return;
        };
        if seit.elapsed() >= stabil {
            folge_abschliessen(&mut folge, melden);
        }
        still = match folge.as_mut() {
            Some((alt, n)) if *alt == grund => {
                *n += 1;
                true
            }
            _ => false,
        };
        if !still {
            folge_abschliessen(&mut folge, melden);
            melden(format!("Ton: {grund} - Abgriff wird neu geoeffnet"));
            folge = Some((grund, 0));
        }
        if warten(da, pause) {
            break;
        }
    }
    folge_abschliessen(&mut folge, melden);
}

/// Eine Folge gleicher Verluste abschliessen: stehen stille Wiederholungen
/// aus, ihre Zahl einmal ins Protokoll.
fn folge_abschliessen(folge: &mut Option<(String, u32)>, melden: &mut dyn FnMut(String)) {
    if let Some((grund, n)) = folge.take() {
        if n > 0 {
            melden(format!("Ton: derselbe Verlust ({grund}) noch {n}-mal, nicht einzeln protokolliert"));
        }
    }
}

/// `dauer` warten, in kleinen Schritten, damit das Ende des Zuschauers nicht
/// lange haengt. true, wenn keiner mehr da ist.
fn warten(da: &dyn Fn() -> bool, dauer: Duration) -> bool {
    let bis = Instant::now() + dauer;
    loop {
        if !da() {
            return true;
        }
        let jetzt = Instant::now();
        if jetzt >= bis {
            return false;
        }
        std::thread::sleep((bis - jetzt).min(Duration::from_millis(50)));
    }
}

// ------------------------------------------------------------------ WASAPI

/// Das echte Geraet: WASAPI-Loopback plus stiller Wiedergabestrom auf dem
/// Standard-Ausgabegeraet.
struct Wasapi;

/// Ein offenes Geraet mit beiden Stroemen. Drop haelt beide an.
struct Geraet {
    /// Fuer die Frage, ob Windows inzwischen ein anderes Standardgeraet hat.
    aufzaehler: IMMDeviceEnumerator,
    /// Kennung des Endpunkts; None, wenn Windows keine nennt - dann wird
    /// nicht verglichen.
    id: Option<String>,
    abgriff: IAudioClient,
    wiedergabe: IAudioClient,
    capture: IAudioCaptureClient,
    render: IAudioRenderClient,
    render_groesse: u32,
    f: Format,
}

impl Drop for Geraet {
    fn drop(&mut self) {
        unsafe {
            let _ = self.abgriff.Stop();
            let _ = self.wiedergabe.Stop();
        }
    }
}

impl Treiber for Wasapi {
    type Geraet = Geraet;

    fn oeffnen(&mut self) -> Result<(Geraet, String), String> {
        unsafe { oeffnen() }
    }

    fn pumpen(&mut self, g: &Geraet, da: &dyn Fn() -> bool) -> Option<String> {
        unsafe { pumpen(g, da) }
    }
}

/// Kennung eines Endpunkts als Text. Die Zeichenkette von GetId gehoert uns
/// und wird hier freigegeben.
unsafe fn endpunkt_id(geraet: &IMMDevice) -> Option<String> {
    let p = geraet.GetId().ok()?;
    if p.is_null() {
        return None;
    }
    let id = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const c_void));
    id
}

/// Das Standard-Ausgabegeraet oeffnen: Abgriff (Loopback) und stiller
/// Wiedergabestrom auf demselben Endpunkt, beide im Mischformat, beide
/// gestartet. Err mit dem Grund, wenn das Geraet nicht aufgeht.
unsafe fn oeffnen() -> Result<(Geraet, String), String> {
    let aufzaehler: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("Geraeteliste: {e}"))?;
    let geraet = aufzaehler.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| format!("kein Standard-Ausgabegeraet: {e}"))?;
    let id = endpunkt_id(&geraet);

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
    let g = Geraet { aufzaehler, id, abgriff, wiedergabe, capture, render, render_groesse, f };

    // Stille vorlegen, dann beide starten.
    if let Ok(p) = g.render.GetBuffer(g.render_groesse) {
        if !p.is_null() {
            let _ = g.render.ReleaseBuffer(g.render_groesse, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
        }
    }
    g.wiedergabe.Start().map_err(|e| format!("Wiedergabe starten: {e}"))?;
    g.abgriff.Start().map_err(|e| format!("Abgriff starten: {e}"))?;
    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    let format = format!(
        "{} Hz, {} Kanaele ({}), gemischt auf 2 Kanaele float32 verschachtelt, stiller Wiedergabestrom laeuft",
        g.f.rate, g.f.kanaele, if g.f.float { "float32".to_string() } else { format!("{} Bit", g.f.bits) }
    );
    Ok((g, format))
}

/// Alles abholen, was der Abgriff hat, und senden - bis der Zuschauer geht
/// (None) oder das Geraet nicht mehr taugt (Some(grund)): Abgriff verloren
/// (AUDCLNT_E_DEVICE_INVALIDATED: abgezogen, Format umgestellt) oder
/// Windows hat ein anderes Standard-Ausgabegeraet - abgegriffen wuerde sonst
/// ein Geraet, das niemand mehr hoert.
unsafe fn pumpen(g: &Geraet, da: &dyn Fn() -> bool) -> Option<String> {
    let f = &g.f;
    let mut pcm: Vec<f32> = Vec::new();
    let mut erwartet_qpc: Option<u64> = None;
    let paket_s = 0.02f64; // Loopback-Pakete sind 10 ms, die Toleranz ist ein Paket plus etwas
    let mut geprueft = Instant::now();
    while da() {
        if let Some(alt) = g.id.as_ref().filter(|_| geprueft.elapsed() >= STANDARD_PRUEFEN) {
            geprueft = Instant::now();
            if let Ok(d) = g.aufzaehler.GetDefaultAudioEndpoint(eRender, eConsole) {
                if endpunkt_id(&d).is_some_and(|neu| neu != *alt) {
                    return Some("anderes Standard-Ausgabegeraet".into());
                }
            }
        }
        // Der stille Strom bleibt voll.
        if let Ok(belegt) = g.wiedergabe.GetCurrentPadding() {
            let frei = g.render_groesse.saturating_sub(belegt);
            if frei > g.render_groesse / 2 {
                if let Ok(p) = g.render.GetBuffer(frei) {
                    if !p.is_null() {
                        let _ = g.render.ReleaseBuffer(frei, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
                    }
                }
            }
        }
        // Alles abholen, was der Abgriff hat.
        loop {
            let n = match g.capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(e) => return Some(format!("Abgriff verloren: {e}")),
            };
            if n == 0 {
                break;
            }
            let mut daten: *mut u8 = std::ptr::null_mut();
            let mut rahmen = 0u32;
            let mut flags = 0u32;
            let mut qpc = 0u64;
            if let Err(e) = g.capture.GetBuffer(&mut daten, &mut rahmen, &mut flags, None, Some(&mut qpc)) {
                return Some(format!("Abgriff verloren: {e}"));
            }
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
                    mischen(daten, f, rahmen_n, &mut pcm);
                }
                senden(&pcm, f.rate);
                erwartet_qpc = Some(qpc + (rahmen_n as u64 * 10_000_000) / f.rate as u64);
            }
            let _ = g.capture.ReleaseBuffer(rahmen);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// Der Tonfaden: wartet auf einen Zuschauer, greift ab, solange er da ist,
/// und oeffnet nach einem Verlust neu (siehe `betreiben`).
pub fn start() {
    std::thread::Builder::new()
        .name("quadchroma-ton".into())
        .spawn(|| {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            // Derselbe Fehler beim Oeffnen steht ueber alle Zuschauer hinweg
            // einmal da - bis wieder ein Geraet aufging.
            let mut zuletzt: Option<String> = None;
            loop {
                while !netz::zuschauer_da() {
                    std::thread::sleep(Duration::from_millis(200));
                }
                // Der naechste Zuschauer ist einer mit anderer Nummer - auch
                // wenn er ohne Luecke abloest (wie in aufnahme::start).
                let nr = netz::zuschauer_nr();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    betreiben(&mut Wasapi, &netz::zuschauer_da, Duration::from_millis(NEU_MS), STABIL, &mut zuletzt, &mut |z| log(z))
                }));
                if r.is_err() {
                    log("Ton: Abgriff abgestuerzt - der Host laeuft ohne Ton bis zum naechsten Zuschauer");
                    while netz::zuschauer_da() && netz::zuschauer_nr() == nr {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Vorgetaeuschtes Geraet: jeder Versuch zu oeffnen nimmt das naechste
    /// Ergebnis aus `oeffnen` (leer: "kein Standard-Ausgabegeraet"), jedes
    /// Pumpen den naechsten Verlustgrund aus `verluste` (leer: der Zuschauer
    /// geht, `weg` wird gesetzt).
    struct Attrappe {
        oeffnen: VecDeque<Result<String, String>>,
        verluste: VecDeque<String>,
        versuche: usize,
        /// Geraete, die gerade offen sind - beim naechsten Versuch muss das
        /// alte zu sein.
        offen: Rc<Cell<usize>>,
        offen_beim_versuch: Vec<usize>,
        weg: Rc<Cell<bool>>,
    }

    struct Stueck(Rc<Cell<usize>>);

    impl Drop for Stueck {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    impl Attrappe {
        fn neu(oeffnen: Vec<Result<&str, &str>>, verluste: Vec<&str>) -> Attrappe {
            Attrappe {
                oeffnen: oeffnen.into_iter().map(|r| r.map(|s| s.to_string()).map_err(|e| e.to_string())).collect(),
                verluste: verluste.into_iter().map(|v| v.to_string()).collect(),
                versuche: 0,
                offen: Rc::new(Cell::new(0)),
                offen_beim_versuch: Vec::new(),
                weg: Rc::new(Cell::new(false)),
            }
        }
    }

    impl Treiber for Attrappe {
        type Geraet = Stueck;

        fn oeffnen(&mut self) -> Result<(Stueck, String), String> {
            self.versuche += 1;
            self.offen_beim_versuch.push(self.offen.get());
            let format = self.oeffnen.pop_front().unwrap_or_else(|| Err("kein Standard-Ausgabegeraet".into()))?;
            self.offen.set(self.offen.get() + 1);
            Ok((Stueck(self.offen.clone()), format))
        }

        fn pumpen(&mut self, _geraet: &Stueck, da: &dyn Fn() -> bool) -> Option<String> {
            assert!(da(), "gepumpt ohne Zuschauer");
            let v = self.verluste.pop_front();
            if v.is_none() {
                self.weg.set(true);
            }
            v
        }
    }

    /// Ein Zuschauer, der bleibt, bis die Attrappe keine Verluste mehr hat.
    fn lauf(t: &mut Attrappe, stabil: Duration, zuletzt: &mut Option<String>) -> Vec<String> {
        let weg = t.weg.clone();
        let da = move || !weg.get();
        let mut zeilen = Vec::new();
        betreiben(t, &da, Duration::from_millis(1), stabil, zuletzt, &mut |z| zeilen.push(z));
        zeilen
    }

    fn kein_geraet(ms: u64) -> String {
        format!("Ton: kein Standard-Ausgabegeraet - der Host laeuft ohne Ton, neuer Versuch alle {ms} ms, solange jemand zuschaut")
    }

    /// Kein Zuschauer, keine Arbeit: nicht ein einziger Versuch.
    #[test]
    fn kein_zuschauer_keine_versuche() {
        let mut t = Attrappe::neu(vec![Ok("48000 Hz")], vec![]);
        let mut zeilen = Vec::new();
        betreiben(&mut t, &|| false, Duration::from_millis(1), STABIL, &mut None, &mut |z| zeilen.push(z));
        assert_eq!(t.versuche, 0);
        assert!(zeilen.is_empty());
    }

    /// Ohne Tongeraet (Bau-VM): der Host laeuft weiter, versucht es
    /// regelmaessig neu, solange der Zuschauer da ist, und schreibt den
    /// Grund einmal - auch fuer den naechsten Zuschauer nicht noch einmal.
    /// Geht der Zuschauer, hoeren die Versuche auf.
    #[test]
    fn kein_geraet_wird_regelmaessig_neu_versucht() {
        let mut t = Attrappe::neu(vec![], vec![]);
        let fragen = Cell::new(0usize);
        let da = || {
            fragen.set(fragen.get() + 1);
            fragen.get() <= 40
        };
        let mut zeilen = Vec::new();
        let mut zuletzt = None;
        betreiben(&mut t, &da, Duration::from_millis(1), STABIL, &mut zuletzt, &mut |z| zeilen.push(z));
        assert!(t.versuche >= 10, "nur {} Versuche", t.versuche);
        assert_eq!(zeilen, vec![kein_geraet(1)]);
        assert_eq!(zuletzt.as_deref(), Some("kein Standard-Ausgabegeraet"));
        // Nach dem Ende des Zuschauers: keine weiteren Versuche.
        let versuche = t.versuche;
        assert!(!da());
        assert_eq!(t.versuche, versuche);

        // Der naechste Zuschauer: wieder Versuche, aber keine neue Zeile.
        fragen.set(0);
        zeilen.clear();
        betreiben(&mut t, &da, Duration::from_millis(1), STABIL, &mut zuletzt, &mut |z| zeilen.push(z));
        assert!(t.versuche > versuche);
        assert!(zeilen.is_empty(), "{zeilen:?}");
    }

    /// Abgriff verloren: altes Geraet zu, neu oeffnen, bis es klappt; ein
    /// Oeffnungsfehler steht einmal da, das neue Format auch. Nie zwei
    /// Geraete zugleich offen.
    #[test]
    fn verlust_oeffnet_neu() {
        const V: &str = "Abgriff verloren: Das Audiogeraet wurde fuer ungueltig erklaert. (0x88890004)";
        let mut t = Attrappe::neu(
            vec![Ok("48000 Hz"), Err("kein Standard-Ausgabegeraet"), Err("kein Standard-Ausgabegeraet"), Ok("44100 Hz")],
            vec![V],
        );
        let mut zuletzt = None;
        let zeilen = lauf(&mut t, Duration::ZERO, &mut zuletzt);
        assert_eq!(t.versuche, 4);
        assert_eq!(t.offen_beim_versuch, vec![0; 4]);
        assert_eq!(t.offen.get(), 0, "Geraet nach dem Zuschauer noch offen");
        assert_eq!(zuletzt, None, "ein Geraet ging auf - der Fehler darf wieder gemeldet werden");
        assert_eq!(
            zeilen,
            vec![
                "Ton angemeldet: 48000 Hz".to_string(),
                format!("Ton: {V} - Abgriff wird neu geoeffnet"),
                kein_geraet(1),
                "Ton wieder angemeldet: 44100 Hz".to_string(),
                "Ton: Abgriff beendet (kein Zuschauer)".to_string(),
            ]
        );
    }

    /// Anderes Standardgeraet: neu oeffnen, auch mehrmals hintereinander.
    #[test]
    fn anderes_standardgeraet_oeffnet_neu() {
        let mut t = Attrappe::neu(vec![Ok("48000 Hz"), Ok("48000 Hz"), Ok("96000 Hz")], vec!["anderes Standard-Ausgabegeraet", "anderes Standard-Ausgabegeraet"]);
        let zeilen = lauf(&mut t, Duration::ZERO, &mut None);
        assert_eq!(t.versuche, 3);
        assert_eq!(t.offen_beim_versuch, vec![0; 3]);
        assert_eq!(zeilen.iter().filter(|z| z.ends_with("Abgriff wird neu geoeffnet")).count(), 2);
        assert_eq!(zeilen.iter().filter(|z| z.starts_with("Ton wieder angemeldet")).count(), 2);
        assert_eq!(zeilen.last().unwrap(), "Ton: Abgriff beendet (kein Zuschauer)");
    }

    /// Ein Geraet, das aufgeht, aber immer gleich wieder verloren geht: der
    /// Grund steht einmal da, die Wiederholungen als Zahl am Ende der Folge.
    /// Ein anderes Format wird trotzdem gemeldet.
    #[test]
    fn gleicher_verlust_nur_gezaehlt() {
        const G: &str = "Abgriff verloren: 0x88890004";
        let neu = format!("Ton: {G} - Abgriff wird neu geoeffnet");
        let mut t = Attrappe::neu(vec![Ok("48000 Hz"); 6], vec![G; 5]);
        let zeilen = lauf(&mut t, Duration::from_secs(3600), &mut None);
        assert_eq!(t.versuche, 6);
        assert_eq!(
            zeilen,
            vec![
                "Ton angemeldet: 48000 Hz".to_string(),
                neu.clone(),
                "Ton wieder angemeldet: 48000 Hz".to_string(),
                format!("Ton: derselbe Verlust ({G}) noch 4-mal, nicht einzeln protokolliert"),
                "Ton: Abgriff beendet (kein Zuschauer)".to_string(),
            ]
        );

        let mut t = Attrappe::neu(vec![Ok("48000 Hz"), Ok("48000 Hz"), Ok("48000 Hz"), Ok("44100 Hz")], vec![G; 3]);
        let zeilen = lauf(&mut t, Duration::from_secs(3600), &mut None);
        assert_eq!(
            zeilen,
            vec![
                "Ton angemeldet: 48000 Hz".to_string(),
                neu,
                "Ton wieder angemeldet: 48000 Hz".to_string(),
                "Ton wieder angemeldet: 44100 Hz".to_string(),
                format!("Ton: derselbe Verlust ({G}) noch 2-mal, nicht einzeln protokolliert"),
                "Ton: Abgriff beendet (kein Zuschauer)".to_string(),
            ]
        );
    }

    /// Nachricht 32 fuer den ersten Ton je Zuschauer und fuer jede neue
    /// Rate - nicht bei gleicher Rate (der Client baut bei jeder 32 seine
    /// Ausgabe neu auf).
    #[test]
    fn formatansage_bei_neuer_rate() {
        let angesagt = AtomicU32::new(0);
        assert!(ansage_faellig(&angesagt, 48_000));
        assert!(!ansage_faellig(&angesagt, 48_000));
        assert!(ansage_faellig(&angesagt, 44_100), "anderes Geraet mit anderer Rate");
        assert!(!ansage_faellig(&angesagt, 44_100));
        angesagt.store(0, Ordering::Relaxed); // neuer Zuschauer
        assert!(ansage_faellig(&angesagt, 44_100));
    }

    /// Am echten WASAPI: ohne Tongeraet (Bau-VM) kein Abbruch, sondern
    /// regelmaessige Versuche mit einer Zeile; mit Tongeraet laeuft der
    /// Abgriff, bis der Zuschauer geht. Haengen darf beides nicht.
    #[test]
    fn echtes_geraet_oder_regelmaessiger_versuch() {
        struct Zaehler(Wasapi, usize);
        impl Treiber for Zaehler {
            type Geraet = Geraet;
            fn oeffnen(&mut self) -> Result<(Geraet, String), String> {
                self.1 += 1;
                self.0.oeffnen()
            }
            fn pumpen(&mut self, g: &Geraet, da: &dyn Fn() -> bool) -> Option<String> {
                self.0.pumpen(g, da)
            }
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let start = Instant::now();
        let ende = start + Duration::from_millis(600);
        let da = move || Instant::now() < ende;
        let mut t = Zaehler(Wasapi, 0);
        let mut zeilen = Vec::new();
        let mut zuletzt = None;
        betreiben(&mut t, &da, Duration::from_millis(50), STABIL, &mut zuletzt, &mut |z| zeilen.push(z));
        assert!(start.elapsed() < Duration::from_secs(5), "haengt: {:?}", start.elapsed());
        match zuletzt {
            Some(e) => {
                println!("Tongeraet: {e} - {} Versuche in 600 ms", t.1);
                assert!(t.1 >= 5, "nur {} Versuche", t.1);
                assert_eq!(zeilen.len(), 1, "{zeilen:?}");
                assert!(zeilen[0].contains(&e));
            }
            None => {
                println!("Tongeraet offen: {zeilen:?}");
                assert!(zeilen.iter().any(|z| z.starts_with("Ton angemeldet")));
            }
        }
    }
}
