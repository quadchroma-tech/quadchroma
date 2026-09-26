// Windows als Host - Rolle in derselben Programmdatei wie der Client.
//
//   quadchroma.exe --host [port] [--output n] [--fps N] [--mbit N] [--fest]
//                  [--pair] [--forget] [--konserve datei.hevc]
//                  [--encoderweg bgra|yuv444|d3d11|auto]
//   quadchroma.exe --list
//   quadchroma.exe --messen [--output n] [--sekunden 10]
//
// Ohne Fenster; Konsole ueber AttachConsole wie der Client, jede Zeile
// ausserdem in %APPDATA%\QuadChroma\host-protokoll.txt. Das Protokoll auf
// der Leitung ist das des Mac-Hosts (host/main.m), Byte fuer Byte - ein
// Client darf den Host nicht erkennen.
//
// Stand: Zuschauerplatz (Noise-Responder, Kopplung, Bekanntgabe), Eingaben,
// Zwischenablage (Text und Dateien, netz.rs mit dateien.rs), Aufnahme
// (Desktop Duplication) mit Schrittmacher und Encoder im Betrieb (nvenc,
// ohne NVIDIA h264_mf in Software), Codecwechsel, Bildschirmwahl mit
// Umschalten (aufnahme.rs, Nachrichten 12 und 70), Testbild, Ton
// (WASAPI-Loopback), Zeigerform, Last, Wachhalten; die Konserve bleibt als
// Bildquelle waehlbar (--konserve); die Messung (--messen).

pub mod aufnahme;
pub mod eingabe;
pub mod encoder;
pub mod konserve;
pub mod messen;
pub mod netz;
pub mod takt;
pub mod testbild;
pub mod ton;
pub mod zeiger;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::protokoll_konst::*;
use crate::{noise, protokoll, secure};

// ------------------------------------------------------------------ Zustand
//
// Wie in main.m: alles, was mehrere Faeden lesen, liegt in Atomics. Der
// Client schickt Wuensche, der Host setzt sie um und meldet zurueck, was gilt.

pub struct Zustand {
    pub fps: AtomicU32,
    pub mbit: AtomicU32,
    pub gaming: AtomicBool,
    pub fest: AtomicBool,
    pub ton: AtomicBool,
    /// Kopplungsfenster offen (--pair): die naechste unbekannte Gegenstelle
    /// wird aufgenommen.
    pub pair_open: AtomicBool,
    /// Das naechste Bild soll ein Vollbild sein.
    pub force_key: AtomicBool,
    /// Frisch verbundener Zuschauer wartet auf ein Vollbild.
    pub wait_key: AtomicBool,
    pub sent_frames: AtomicU64,
    pub sent_bytes: AtomicU64,
    /// Bilder, die wegen Stau auf der Leitung gar nicht erst in den Encoder
    /// gingen (netz::stau_vor_dem_encoder) - dazu die seltenen, die schon
    /// codiert ueber der harten Grenze wegfielen (eigene Protokollzeile).
    pub stau: AtomicU64,
    pub repeats: AtomicU64,
    /// Ohne feste Bildrate nachgeschoben, damit ein Encoder mit Vorlauf
    /// (h264_mf) die echten Bilder herausgibt (takt::vorlauf_nachschieben).
    pub nachgeschoben: AtomicU64,
    pub zu_schnell: AtomicU64,
    pub enc_stau: AtomicU64,
    pub enc_verworfen: AtomicU64,
    pub audio_packets: AtomicU64,
    pub audio_bytes: AtomicU64,
    /// Tonpakete, die an der Grenze fuer wartenden Ton wegfielen (Leitung
    /// langsamer als der Ton, netz::ton_grenze).
    pub ton_verworfen: AtomicU64,
    pub input_events: AtomicU64,
    /// Eckdaten des Stroms - immer aus dem laufenden Kandidaten abgeleitet.
    pub info_w: AtomicU32,
    pub info_h: AtomicU32,
    pub codec_id: AtomicU32,
    /// Encoderzeit je Bild (Summe us, Anzahl) fuer Nachricht 6.
    pub enc_us: AtomicU64,
    pub enc_n: AtomicU64,
    pub testbild: AtomicBool,
    /// Bildquelle ist eine Konserve (kein Encoder, kein Codecwechsel).
    pub konserve: AtomicBool,
    /// Die Bildschirme des Hosts mit Wunsch und gestreamtem Eintrag
    /// (Nachricht 12) - vom Aufnahmefaden gepflegt, vom Netzfaden fuer die
    /// Begruessung gelesen.
    pub bildschirme: Mutex<crate::bildschirm::Bildschirme>,
}

pub static Z: Zustand = Zustand {
    fps: AtomicU32::new(120),
    mbit: AtomicU32::new(150),
    gaming: AtomicBool::new(false),
    fest: AtomicBool::new(false),
    ton: AtomicBool::new(true),
    pair_open: AtomicBool::new(false),
    force_key: AtomicBool::new(false),
    wait_key: AtomicBool::new(false),
    sent_frames: AtomicU64::new(0),
    sent_bytes: AtomicU64::new(0),
    stau: AtomicU64::new(0),
    repeats: AtomicU64::new(0),
    nachgeschoben: AtomicU64::new(0),
    zu_schnell: AtomicU64::new(0),
    enc_stau: AtomicU64::new(0),
    enc_verworfen: AtomicU64::new(0),
    audio_packets: AtomicU64::new(0),
    audio_bytes: AtomicU64::new(0),
    ton_verworfen: AtomicU64::new(0),
    input_events: AtomicU64::new(0),
    info_w: AtomicU32::new(0),
    info_h: AtomicU32::new(0),
    codec_id: AtomicU32::new(0),
    enc_us: AtomicU64::new(0),
    enc_n: AtomicU64::new(0),
    testbild: AtomicBool::new(false),
    konserve: AtomicBool::new(false),
    bildschirme: Mutex::new(crate::bildschirm::Bildschirme { wunsch: None, eintraege: Vec::new() }),
};

// ---------------------------------------------------------------- Protokoll

/// Groesse, ab der die Protokolldatei neu beginnt: der bisherige Teil wird
/// zu <name>.alt.txt (ein aelterer faellt dabei weg). Auf der Platte liegen
/// so hoechstens zweimal 8 MB - auch wenn der Host tagelang laeuft oder
/// jemand seine Ports mit Verbindungen bestreicht.
const PROTOKOLL_GRENZE: u64 = 8 * 1024 * 1024;

/// Die offene Protokolldatei und wie viel schon darin steht.
struct Protokolldatei {
    datei: std::fs::File,
    pfad: std::path::PathBuf,
    geschrieben: u64,
}

static DATEI: Mutex<Option<Protokolldatei>> = Mutex::new(None);

/// Protokolldatei oeffnen (wird bei jedem Start neu begonnen).
fn protokoll_oeffnen(name: &str) {
    if let Some(p) = crate::einstellungen::datei_pfad(name) {
        if let Ok(f) = std::fs::File::create(&p) {
            *DATEI.lock().unwrap() = Some(Protokolldatei { datei: f, pfad: p, geschrieben: 0 });
        }
    }
}

/// Eine Zeile auf die Konsole und in die Datei.
pub fn log(text: impl AsRef<str>) {
    use std::io::Write;
    let t = text.as_ref();
    println!("{t}");
    let _ = std::io::stdout().flush();
    if let Ok(mut d) = DATEI.lock() {
        zeile_schreiben(&mut d, t, PROTOKOLL_GRENZE);
    }
}

/// Eine Zeile in die Datei; braechte sie die Datei ueber `grenze`, beginnt
/// vorher eine neue (umschichten).
fn zeile_schreiben(d: &mut Option<Protokolldatei>, t: &str, grenze: u64) {
    use std::io::Write;
    let laenge = t.len() as u64 + 1;
    if let Some(p) = d.take() {
        *d = if p.geschrieben > 0 && p.geschrieben + laenge > grenze { umschichten(p, grenze) } else { Some(p) };
    }
    if let Some(p) = d.as_mut() {
        if writeln!(p.datei, "{t}").is_ok() {
            p.geschrieben += laenge;
        }
    }
}

/// Die volle Datei wird zu <name>.alt.txt, eine neue beginnt mit einem
/// Hinweis darauf. Laesst sie sich nicht umbenennen (etwa weil jemand die
/// alte offen haelt), beginnt die Datei selbst von vorn - die Grenze bleibt
/// hart. Geht auch das nicht, schreibt nur noch die Konsole.
fn umschichten(p: Protokolldatei, grenze: u64) -> Option<Protokolldatei> {
    use std::io::Write;
    let Protokolldatei { datei, pfad, .. } = p;
    drop(datei);
    let alt = pfad.with_extension("alt.txt");
    let name = alt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let hinweis = match std::fs::rename(&pfad, &alt) {
        Ok(()) => format!("Protokoll ueber {} MB - der vorige Teil steht in {name}", grenze >> 20),
        Err(e) => format!("Protokoll ueber {} MB - neu begonnen, der vorige Teil liess sich nicht nach {name} umbenennen: {e}", grenze >> 20),
    };
    let datei = std::fs::File::create(&pfad).ok()?;
    let mut neu = Protokolldatei { datei, pfad, geschrieben: 0 };
    if writeln!(neu.datei, "{hinweis}").is_ok() {
        neu.geschrieben = hinweis.len() as u64 + 1;
    }
    Some(neu)
}

/// Was FFmpeg inzwischen gesagt hat (ueber den Rueckruf des Clients), als
/// eigene Zeilen ins Hostprotokoll.
pub fn ffmpeg_zeilen() {
    for z in protokoll::abholen() {
        log(z);
    }
}

// --------------------------------------------------------------------- Uhr

/// Uhr des Hosts in Mikrosekunden: der Hochleistungszaehler ab dem ersten
/// Aufruf. Nachricht 4 (Zeitantwort), Nachricht 5 (Stempel) und die
/// Praesentationszeit der Duplication (LastPresentTime, ebenfalls QPC)
/// lesen dieselbe Uhr.
pub fn now_us() -> u64 {
    use windows::Win32::System::Performance::QueryPerformanceCounter;
    let mut q = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut q);
    }
    qpc_us(q)
}

/// Ein QPC-Stand in Mikrosekunden der Hostuhr.
pub fn qpc_us(q: i64) -> u64 {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    static START: OnceLock<(i64, i64)> = OnceLock::new();
    let (q0, hz) = *START.get_or_init(|| {
        let (mut q0, mut hz) = (0i64, 1i64);
        unsafe {
            let _ = QueryPerformanceCounter(&mut q0);
            let _ = QueryPerformanceFrequency(&mut hz);
        }
        (q0, hz.max(1))
    });
    ((q - q0).max(0) as u128 * 1_000_000 / hz as u128) as u64
}

// ------------------------------------------------------------- Befehlszeile

/// Wert hinter einem Schalter.
pub fn arg_wert(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn arg_zahl(args: &[String], name: &str) -> Option<u32> {
    arg_wert(args, name).and_then(|v| v.parse().ok())
}

// -------------------------------------------------------------- Einstellungen

/// Einstellungen im laufenden Betrieb (Nachricht 64 vom Client). Bitrate,
/// Bildrate und die Schalter werden uebernommen und mit Nachricht 3
/// bestaetigt; der Encoder (folgt) liest sie beim naechsten Bild.
pub fn apply_settings(mbit: u32, fps: u32, gaming: bool, fixed: bool, ton: bool) {
    let mbit = mbit.clamp(2, 500);
    let fps = fps.clamp(10, 240);
    Z.mbit.store(mbit, Ordering::Relaxed);
    Z.fps.store(fps, Ordering::Relaxed);
    Z.gaming.store(gaming, Ordering::Relaxed);
    Z.fest.store(fixed, Ordering::Relaxed);
    Z.ton.store(ton, Ordering::Relaxed);
    Z.force_key.store(true, Ordering::Relaxed);
    netz::settings_senden();
    log(format!(
        "Einstellungen: {mbit} Mbit/s, {fps} fps, Gaming {}, feste Bildrate {}, Ton {}",
        if gaming { "an" } else { "aus" },
        if fixed { "an" } else { "aus" },
        if ton { "an" } else { "aus" }
    ));
}

/// Strominfo (Nachricht 1), immer aus dem laufenden Kandidaten abgeleitet.
pub fn strominfo() -> [u8; 8] {
    let mut p = [0u8; 8];
    let w = Z.info_w.load(Ordering::Relaxed) as u16;
    let h = Z.info_h.load(Ordering::Relaxed) as u16;
    let f = Z.fps.load(Ordering::Relaxed) as u16;
    p[0..2].copy_from_slice(&w.to_le_bytes());
    p[2..4].copy_from_slice(&h.to_le_bytes());
    p[4..6].copy_from_slice(&f.to_le_bytes());
    let k = encoder::kandidat(Z.codec_id.load(Ordering::Relaxed) as usize);
    debug_assert!(encoder::im_protokoll(k), "{}: Codec nicht im Protokoll", k.name);
    p[6] = if k.h264 { 2 } else { 1 };
    // 1 = 4:4:4 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit, 4 = 4:2:0 10 Bit; alles Vollbereich
    p[7] = if k.chroma444 { if k.zehn_bit { 2 } else { 1 } } else if k.zehn_bit { 4 } else { 3 };
    p
}

// ------------------------------------------------------------------- Last

/// Auslastung des Hosts fuer Nachricht 6, Fassung 1 (28 Byte), dieselben
/// Felder wie last.m: cpu, cpu_eigen, gpu (0xffff = nicht lesbar), druck,
/// ram_benutzt, ram_gesamt, eigen_mb, enc_zehntel_ms, host_fps_zehntel.
struct LastProbe {
    idle: u64,
    kernel: u64,
    user: u64,
    eigen: u64,
    wann: Instant,
}

fn filetime_u64(f: windows::Win32::Foundation::FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn last_probe() -> Option<LastProbe> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    Some(LastProbe {
        idle: filetime_u64(idle),
        kernel: filetime_u64(kernel),
        user: filetime_u64(user),
        eigen: crate::prozesszeit_100ns().unwrap_or(0),
        wann: Instant::now(),
    })
}

fn last_nachricht(vorher: &LastProbe, jetzt: &LastProbe, enc_zehntel: u16, host_fps_zehntel: u16) -> [u8; 28] {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    // Kernelzeit enthaelt die Leerlaufzeit; Gesamt = kernel + user.
    let gesamt = (jetzt.kernel + jetzt.user).saturating_sub(vorher.kernel + vorher.user);
    let leer = jetzt.idle.saturating_sub(vorher.idle);
    let cpu_promille: u16 = if gesamt > 0 { ((gesamt - leer.min(gesamt)) * 1000 / gesamt) as u16 } else { 0 };
    // Eigene Last in Promille eines Kerns.
    let dt_100ns = jetzt.wann.duration_since(vorher.wann).as_nanos() as u64 / 100;
    let eigen_promille: u16 = if dt_100ns > 0 {
        (jetzt.eigen.saturating_sub(vorher.eigen) * 1000 / dt_100ns).min(u16::MAX as u64) as u16
    } else {
        0
    };
    let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let (ram_benutzt, ram_gesamt) = if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() {
        (((ms.ullTotalPhys - ms.ullAvailPhys) >> 20) as u32, (ms.ullTotalPhys >> 20) as u32)
    } else {
        (0, 0)
    };
    let mut pmc = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    let eigen_mb = if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) }.is_ok() {
        (pmc.WorkingSetSize >> 20) as u32
    } else {
        0
    };
    let mut b = [0u8; 28];
    b[0] = 1;
    b[2..4].copy_from_slice(&cpu_promille.to_le_bytes());
    b[4..6].copy_from_slice(&eigen_promille.to_le_bytes());
    b[6..8].copy_from_slice(&0xffffu16.to_le_bytes()); // GPU: nicht lesbar (spaeter PDH)
    b[8..10].copy_from_slice(&0u16.to_le_bytes()); // Druck: kennt Windows so nicht
    b[10..14].copy_from_slice(&ram_benutzt.to_le_bytes());
    b[14..18].copy_from_slice(&ram_gesamt.to_le_bytes());
    b[18..22].copy_from_slice(&eigen_mb.to_le_bytes());
    b[22..24].copy_from_slice(&enc_zehntel.to_le_bytes());
    b[24..26].copy_from_slice(&host_fps_zehntel.to_le_bytes());
    b
}

// ---------------------------------------------------------------- Einstieg

/// Per-Monitor-DPI (v2) fuer den ganzen Prozess. Ohne sie meldet DXGI bei
/// 125/150 % Skalierung virtualisierte Ausgangsgroessen (GetDesc), die
/// Duplication aber den echten Anzeigemodus; die Maus rechnet dann ebenfalls
/// in anderen Punkten. Liefert die Zeile fuers Protokoll.
fn dpi_bewusst() -> String {
    use windows::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetDpiForSystem, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe {
        match SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) {
            Ok(()) => {
                let dpi = GetDpiForSystem();
                format!("DPI-Awareness: Per-Monitor v2 gesetzt (System {dpi} dpi = {} %)", dpi * 100 / 96)
            }
            // Zugriff verweigert heisst auch "war schon gesetzt".
            Err(_) if AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool() => {
                "DPI-Awareness: Per-Monitor v2 (war schon gesetzt)".to_string()
            }
            Err(e) => format!(
                "DPI-Awareness nicht gesetzt: {} (0x{:08x}) - bei Skalierung koennen Groessen virtualisiert sein",
                e.message().trim(),
                e.code().0 as u32
            ),
        }
    }
}

/// Rollenwahl: --list, --messen oder --host. Rueckgabe ist der Exit-Code.
pub fn main_host(args: &[String]) -> i32 {
    // Vor allem anderen, das Ausgaenge, Bildschirm oder Maus anfasst -
    // fuer alle Hostrollen.
    let dpi = dpi_bewusst();
    if let Err(e) = ffmpeg_next::init() {
        println!("FFmpeg-Start fehlgeschlagen: {e}");
        return 2;
    }
    // FFmpegs Meldungen laufen ueber den Rueckruf des Clients in eine Reihe;
    // der Host holt sie ab und schreibt sie in sein eigenes Protokoll.
    protokoll::einschalten(false);

    if args.iter().any(|a| a == "--messen") {
        protokoll_oeffnen("messung.txt");
        log(&dpi);
        return messen::laufen(args);
    }

    protokoll_oeffnen("host-protokoll.txt");
    log(&dpi);

    if args.iter().any(|a| a == "--list") {
        aufnahme::ausgaenge_melden(&mut Vec::new());
        encoder::pruefen();
        encoder::mf_pruefen();
        ffmpeg_zeilen();
        return 0;
    }

    // --host [port]
    let port: u16 = args
        .iter()
        .position(|a| a == "--host")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(9001);

    // Eigener dauerhafter Schluessel. Er entsteht beim ersten Start und
    // bleibt danach liegen, damit Gegenstellen den Host wiedererkennen.
    let (priv_key, pub_key) = match secure::host_identity() {
        Ok(k) => k,
        Err(e) => {
            log(format!("Schluessel konnte nicht angelegt werden - Abbruch: {e}"));
            return 5;
        }
    };
    if args.iter().any(|a| a == "--forget") {
        secure::forget_all();
        log("Alle Freigaben geloescht.");
    }
    log(format!(
        "Fingerabdruck dieses Hosts: {}   freigegebene Gegenstellen: {}",
        noise::fingerprint(&pub_key),
        secure::authorized_count()
    ));
    if args.iter().any(|a| a == "--pair") {
        Z.pair_open.store(true, Ordering::Relaxed);
        log("Kopplung offen: die naechste unbekannte Gegenstelle wird aufgenommen.");
    }

    if let Some(f) = arg_zahl(args, "--fps") {
        Z.fps.store(f.clamp(10, 240), Ordering::Relaxed);
    }
    if let Some(m) = arg_zahl(args, "--mbit") {
        Z.mbit.store(m.clamp(2, 500), Ordering::Relaxed);
    }
    if args.iter().any(|a| a == "--fest" || a == "--fixed") {
        Z.fest.store(true, Ordering::Relaxed);
    }

    // Bildschirm (Spezifikation Bildschirm 1.1-1.5): die Ausgaenge mit
    // Kennung und Name; der Wunsch aus bildschirm.txt, --output n pinnt fuer
    // diesen Lauf den Bildschirm am Listenplatz n (die Datei bleibt); Ziel
    // ist der Wunsch, sonst der Hauptbildschirm, sonst der erste. Gemerkt
    // wird die Kennung, nie der Listenplatz. Ohne DXGI-Ausgang (WARP, RDP)
    // kommt die Geometrie aus der GDI-Liste - fuer die Maus.
    let mut ausgaenge = Vec::new();
    aufnahme::ausgaenge_melden(&mut ausgaenge);
    let gespeichert = aufnahme::wunsch_laden();
    let pin = arg_zahl(args, "--output").and_then(|n| match ausgaenge.get(n as usize) {
        Some(a) => {
            log(format!("--output {n}: Bildschirm {} gilt fuer diesen Lauf (bildschirm.txt bleibt)", a.bezeichnung()));
            Some(a.kennung.clone())
        }
        None => {
            log(format!("Ausgang {n} nicht verfuegbar ({} in der Liste) - kein Pin, es gilt bildschirm.txt bzw. der Hauptbildschirm", ausgaenge.len()));
            None
        }
    });
    let wunsch = pin.or(gespeichert);
    let ziel = aufnahme::ziel_waehlen(&ausgaenge, wunsch.as_deref());
    aufnahme::bildschirme_setzen(&ausgaenge, wunsch.as_deref(), ziel.as_ref().map(|(a, _)| a.kennung.as_str()));
    let ausgang = ziel.as_ref().map(|(a, _)| a.clone());
    match &ziel {
        Some((a, wahl)) => {
            log(format!(
                "Ausgang gewaehlt: {} {}x{} bei ({},{}) an Karte {}{}, Kennung {} ({}), {} Hz{}",
                a.name, a.breite, a.hoehe, a.links, a.oben, a.karte,
                if a.haupt { " (Hauptbildschirm)" } else { "" },
                a.kennung, a.anzeigename, a.hz,
                match wahl {
                    aufnahme::Wahl::Wunsch => " - gewuenschter Bildschirm".to_string(),
                    aufnahme::Wahl::Ausweich => format!(" - Ausweichplatz, {} nicht angeschlossen", wunsch.as_deref().unwrap_or("?")),
                    aufnahme::Wahl::Automatik => String::new(),
                }
            ));
            eingabe::ausgang_setzen(a.links, a.oben, a.breite, a.hoehe);
            let (w, h) = aufnahme::stromgroesse(a);
            Z.info_w.store(w as u32, Ordering::Relaxed);
            Z.info_h.store(h as u32, Ordering::Relaxed);
        }
        None => {
            log("Kein DXGI-Ausgang - Maus bezieht sich auf den GDI-Hauptbildschirm, Bild nur aus einer Konserve");
            if let Some((l, o, w, h)) = aufnahme::gdi_hauptbildschirm() {
                eingabe::ausgang_setzen(l, o, w, h);
                Z.info_w.store((w & !1) as u32, Ordering::Relaxed);
                Z.info_h.store((h & !1) as u32, Ordering::Relaxed);
            } else {
                Z.info_w.store(1920, Ordering::Relaxed);
                Z.info_h.store(1080, Ordering::Relaxed);
            }
        }
    }

    // Was dieser Rechner codieren kann - gefragt, nicht geraten. Der
    // Startkandidat ist 0 wie beim Mac, sonst der erste vorhandene. Ein
    // Codecwechsel (66) gilt danach fuer den Host, auch fuer den naechsten
    // Zuschauer - wie beim Mac (g_codec_id in main.m setzen nur der Start
    // und der Wechsel); Begruessung (1) und Koennensliste (8) sagen ihm, was
    // laeuft.
    encoder::pruefen();
    ffmpeg_zeilen();
    let startkandidat = encoder::startkandidat();
    if let Some(i) = startkandidat {
        Z.codec_id.store(i as u32, Ordering::Relaxed);
    }

    // Konserve: ein Annex-B-Strom als Bildquelle (Pruefweg ohne Karte).
    let konserve = match arg_wert(args, "--konserve") {
        Some(p) => match konserve::Konserve::laden(&p) {
            Ok(k) => Some(k),
            Err(e) => {
                log(format!("Konserve {p}: {e}"));
                return 6;
            }
        },
        None => None,
    };

    // Eingabeweg des Encoders: --encoderweg bgra|yuv444|d3d11|auto.
    let weg_cli = match arg_wert(args, "--encoderweg") {
        Some(t) => match encoder::Weg::aus_text(&t) {
            Some(w) => w,
            None => {
                log(format!("--encoderweg {t}: unbekannt (bgra, yuv444, d3d11, auto)"));
                return 7;
            }
        },
        None => encoder::Weg::Auto,
    };

    // Empfangene Dateien frueherer Laeufe: aelter als 24 h weg (2.9), dazu
    // halb empfangene eines beendeten Prozesses (verwaiste Marke, siehe
    // dateien.rs). Die Host-Rolle hat ihre eigene Basis
    // (netz::host_ablage_basis).
    let alt = netz::host_ablage_aufraeumen();
    if alt > 0 {
        log(format!(
            "Dateien: {alt} Uebertragungen geloescht, aelter als 24 h oder verwaist ({})",
            netz::host_ablage_basis().display()
        ));
    }

    // Zuschauerplatz: Bild, Eingabe, Bekanntgabe.
    if let Err(e) = netz::start(port, priv_key) {
        log(format!("{e}"));
        return 9;
    }
    // Zwischenablage: was hier kopiert wird, geht zum Zuschauer - Text als
    // 48, eine Dateiliste ueber den Sender (50-52), beides auf dem
    // Bildkanal; neuer Inhalt bricht eine laufende Datei-Sendung ab. Was von
    // dort kommt, legt der Eingabefaden (Text) bzw. der Empfaenger der
    // Dateien ab (netz.rs). Der Waechter wartet dabei nie: das Senden der
    // Dateien laeuft in eigenen Faeden.
    crate::clipboard::watch(|inhalt| match inhalt {
        crate::clipboard::Inhalt::Text(text) => {
            netz::datei_sendung_abbrechen();
            netz::send_small(MSG_CLIP, text.as_bytes());
        }
        crate::clipboard::Inhalt::Dateien(pfade) => netz::dateien_senden(pfade),
    });
    eingabe::start();
    // Ton: Abgriff nur mit Zuschauer; ohne Tongeraet steht der Grund einmal da.
    ton::start();
    // Bildquelle: die Konserve bestimmt die Eckdaten des Stroms - vor der
    // Zeile dazu. Sonst die Aufnahme des gewaehlten Ausgangs mit dem
    // Encoder aus der Kandidatentabelle - beides erst, wenn jemand zuschaut.
    if let Some(k) = konserve {
        konserve::abspielen(k);
    } else {
        match (&ausgang, startkandidat) {
            (Some(a), Some(_)) => {
                let weg = encoder::weg_entscheiden(weg_cli, a.index);
                aufnahme::start(wunsch, ausgaenge, weg_cli, weg);
            }
            (None, _) => log("Keine Bildquelle: kein DXGI-Ausgang fuer die Duplication (WARP/RDP) - ohne --konserve geht kein Bild raus"),
            (_, None) => log("Keine Bildquelle: kein Encoder auf diesem Rechner (weder nvenc noch h264_mf) - ohne --konserve geht kein Bild raus"),
        }
    }
    log(format!("\n=== Dienst laeuft: Bild {port}, Eingabe {}, Bekanntgabe {} ===", port + 1, port + 2));
    log(format!(
        "Strom: {}x{}, {} fps, {} Mbit/s, feste Bildrate {}",
        Z.info_w.load(Ordering::Relaxed),
        Z.info_h.load(Ordering::Relaxed),
        Z.fps.load(Ordering::Relaxed),
        Z.mbit.load(Ordering::Relaxed),
        if Z.fest.load(Ordering::Relaxed) { "an" } else { "aus" }
    ));

    // Alle fuenf Sekunden eine Zeile mit dem Stand und Nachricht 6 - nur,
    // wenn jemand zuschaut; sonst misst sich der Host selbst ohne Zweck.
    // Die Drosseln tragen in diesem Takt immer nach, auch ohne Zuschauer:
    // eine Flut kommt gerade dann, wenn keiner verbunden ist.
    let t0 = Instant::now();
    let mut vorher = last_probe();
    let mut last_frames = 0u64;
    let mut last_bytes = 0u64;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        ffmpeg_zeilen();
        netz::drosseln_nachtragen();
        let f = Z.sent_frames.load(Ordering::Relaxed);
        let b = Z.sent_bytes.load(Ordering::Relaxed);
        if !netz::zuschauer_da() {
            last_frames = f;
            last_bytes = b;
            vorher = last_probe();
            continue;
        }
        log(format!(
            "[{:.0} s] Bild: {} ({:.1}/s, {:.1} Mbit/s) | Ton: {} Pakete, {:.0} kB | Stau: {} | Encoder verworfen: {} | nachgelegt: {} | nachgeschoben: {} | zu schnell: {} | Encoder voll: {} | Ton verworfen: {}",
            t0.elapsed().as_secs_f32(),
            f,
            (f - last_frames) as f32 / 5.0,
            (b - last_bytes) as f64 * 8.0 / 5.0 / 1e6,
            Z.audio_packets.load(Ordering::Relaxed),
            Z.audio_bytes.load(Ordering::Relaxed) as f64 / 1000.0,
            Z.stau.load(Ordering::Relaxed),
            Z.enc_verworfen.load(Ordering::Relaxed),
            Z.repeats.load(Ordering::Relaxed),
            Z.nachgeschoben.load(Ordering::Relaxed),
            Z.zu_schnell.load(Ordering::Relaxed),
            Z.enc_stau.load(Ordering::Relaxed),
            Z.ton_verworfen.load(Ordering::Relaxed),
        ));
        let jetzt = last_probe();
        if let (Some(v), Some(j)) = (vorher.as_ref(), jetzt.as_ref()) {
            let n = Z.enc_n.swap(0, Ordering::Relaxed);
            let summe = Z.enc_us.swap(0, Ordering::Relaxed);
            let enc_zehntel = if n > 0 { ((summe / n) / 100).min(u16::MAX as u64) as u16 } else { 0 };
            let host_fps_zehntel = (((f - last_frames) * 10) / 5).min(u16::MAX as u64) as u16;
            netz::send_small(MSG_LAST, &last_nachricht(v, j, enc_zehntel, host_fps_zehntel));
        }
        vorher = jetzt;
        last_frames = f;
        last_bytes = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Das Protokoll waechst nicht ohne Ende (Integrationstest: jede
    /// Muell-Verbindung eine Zeile, ohne Grenze): ueber der Grenze wird die
    /// Datei zu <name>.alt.txt, und eine neue beginnt mit einem Hinweis.
    /// Auf der Platte liegen nie mehr als zweimal die Grenze, und die
    /// neueste Zeile steht immer in der Datei selbst.
    #[test]
    fn protokoll_hat_eine_obergrenze() {
        let ordner = std::env::temp_dir().join(format!("qc-protokoll-{}", std::process::id()));
        std::fs::create_dir_all(&ordner).unwrap();
        let pfad = ordner.join("host-protokoll.txt");
        let alt = ordner.join("host-protokoll.alt.txt");
        let mut d = Some(Protokolldatei { datei: std::fs::File::create(&pfad).unwrap(), pfad: pfad.clone(), geschrieben: 0 });
        let grenze = 1000;
        let mut umgeschichtet = 0;
        for i in 0..500 {
            let zeile = format!("Handschlag mit 10.0.{}.{} gescheitert", i / 250, i % 250);
            zeile_schreiben(&mut d, &zeile, grenze);
            let neu = std::fs::metadata(&pfad).unwrap().len();
            let vorher = std::fs::metadata(&alt).map(|m| m.len()).unwrap_or(0);
            assert!(neu <= grenze && vorher <= grenze, "Zeile {i}: {neu} + {vorher} Byte");
            if neu < zeile.len() as u64 * 2 + 70 {
                umgeschichtet += 1;
            }
            assert!(std::fs::read_to_string(&pfad).unwrap().ends_with(&format!("{zeile}\n")), "Zeile {i} fehlt");
        }
        drop(d);
        assert!(umgeschichtet > 5, "nie umgeschichtet - Probe ohne Wert");
        let text = std::fs::read_to_string(&pfad).unwrap();
        assert!(text.starts_with("Protokoll ueber ") && text.lines().next().unwrap().ends_with("der vorige Teil steht in host-protokoll.alt.txt"), "{text}");
        let vorher = std::fs::read_to_string(&alt).unwrap();
        assert!(vorher.lines().count() > 10, "{vorher}");
        std::fs::remove_dir_all(&ordner).ok();
    }
}
