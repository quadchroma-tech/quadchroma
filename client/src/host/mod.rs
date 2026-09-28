// Windows als Host - Rolle in derselben Programmdatei wie der Client.
//
//   quadchroma.exe                    die eine App (Client und Host-Rolle, ein
//                                     Symbol; main.rs), Freigabe wie eingestellt
//   quadchroma.exe --host [port] [--output n] [--fps N] [--mbit N] [--fest]
//                  [--konserve datei.hevc] [--encoderweg bgra|yuv444|d3d11|auto]
//                                     die eine App im Hintergrund, Freigabe an
//   quadchroma.exe --nur-host [port] [dieselben Schalter]
//                                     nur die Host-Rolle, ohne Fenster (VM, Tests)
//   quadchroma.exe --list
//   quadchroma.exe --messen [--output n] [--sekunden 10]
//
// Die eine App (Plan W6) faehrt die Rolle in ihrem eigenen Prozess: `Rolle`
// startet einen Faden, der den Dienst einrichtet und taktet; die App
// schaltet die Freigabe hierueber an und aus (einstellungen.txt: freigabe),
// reicht die Menuepunkte der Host-Rolle weiter und beendet sie mit
// Abschied. Solange die Freigabe an ist, haelt sie den Mutex
// Local\QuadChroma-Host (hoechstens eine Host-Rolle je Sitzung); die reine
// Host-Rolle (--nur-host, main_host) haelt ihn, solange sie laeuft - ein
// zweiter Start endet still, bevor er das Protokoll anfasst, und --list
// schreibt nur dann in host-protokoll.txt, wenn keine Freigabe laeuft.
//
// Oberflaeche (Spezifikation Pairing v1, 10): das Symbol im Infobereich
// (das der App oder das eigene der reinen Host-Rolle; Menue aus
// symbolmenue.rs, Aktionen in oberflaeche.rs), Zulassen-Anfragen und
// Passwortfenster (fenster.rs). Wer herein darf, entscheidet der Einlass
// (einlass.rs: host-devices.txt, host-password.txt, Passwortbeweis oder
// "Zulassen"). Konsole ueber AttachConsole wie der Client, jede Zeile
// ausserdem in %APPDATA%\QuadChroma\host-protokoll.txt. Das Protokoll auf
// der Leitung ist das des Mac-Hosts (host/main.m), Byte fuer Byte - ein
// Client darf den Host nicht erkennen.
//
// Die Rolle selbst ist ein Dienst (Dienst: einrichten, freigabe_an,
// freigabe_aus, takt, aktion, beenden) ohne process::exit. Beendet wird er
// ueber "Freigabe beenden" der reinen Host-Rolle (Grund 1), mit der App
// (Grund 0) oder von aussen (WM_CLOSE, WM_ENDSESSION am Symbol: Grund 0,
// der Symbolfaden verabschiedet selbst); "Freigabe aus" der App schliesst
// nur die Ports (Grund 1). Alle Wege schliessen erst die Ports
// (netz::stoppen), dann verabschieden sie den Zuschauer.
//
// Stand: Zuschauerplatz (Noise-Responder, Einlass, Bekanntgabe), Eingaben,
// Zwischenablage (Text und Dateien, netz.rs mit dateien.rs), Aufnahme
// (Desktop Duplication) mit Schrittmacher und Encoder im Betrieb (nvenc,
// ohne NVIDIA h264_mf in Software), Codecwechsel, Bildschirmwahl mit
// Umschalten (aufnahme.rs, Nachrichten 12 und 70), Testbild, Ton
// (WASAPI-Loopback), Zeigerform, Last, Wachhalten; die Konserve bleibt als
// Bildquelle waehlbar (--konserve); die Messung (--messen).

pub mod aufnahme;
pub mod eingabe;
pub mod einlass;
pub mod encoder;
pub mod fenster;
pub mod konserve;
pub mod messen;
pub mod netz;
pub mod oberflaeche;
pub mod takt;
pub mod testbild;
pub mod ton;
pub mod wandler;
pub mod zeiger;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::protokoll_konst::*;
use crate::{noise, protokoll, secure, symbolmenue, zugang};

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

/// Was FFmpeg inzwischen gesagt hat (ueber den Rueckruf des Clients, in der
/// Reihe des Hosts), als eigene Zeilen ins Hostprotokoll. Nur aus Faeden mit
/// Herkunft::Host rufen (protokoll::herkunft_setzen) - sonst holte es die
/// Reihe des Clients ab.
pub fn ffmpeg_zeilen() {
    debug_assert_eq!(protokoll::herkunft(), protokoll::Herkunft::Host, "ffmpeg_zeilen aus einem Faden ohne Herkunft Host");
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

/// Gehen die Ausgaben dieses Prozesses ins Leere - war die Standardausgabe
/// beim Start das Geraet NUL? So startet der Knopf "Diesen PC freigeben"
/// des Clients die Host-Rolle (Stdio::null, DETACHED_PROCESS). Gefragt wird
/// nach dem, was der Aufrufer beim Start mitgab (GetStartupInfoW), nicht
/// nach der Standardausgabe von jetzt: AttachConsole in main.rs kann sie
/// inzwischen ersetzt haben. Ein Start aus der Eingabeaufforderung (Konsole)
/// oder mit umgeleiteter Ausgabe (Datei, Pipe) zaehlt nicht.
fn ausgabe_ins_leere() -> bool {
    use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_CHAR};
    use windows::Win32::System::Console::{GetConsoleMode, CONSOLE_MODE};
    use windows::Win32::System::Threading::{GetStartupInfoW, STARTF_USESTDHANDLES, STARTUPINFOW};
    let mut si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    unsafe { GetStartupInfoW(&mut si) };
    let h = si.hStdOutput;
    if !si.dwFlags.contains(STARTF_USESTDHANDLES) || h.is_invalid() {
        return false;
    }
    // Ein Zeichengeraet, das keine Konsole ist: NUL.
    let mut modus = CONSOLE_MODE::default();
    unsafe { GetFileType(h) == FILE_TYPE_CHAR && GetConsoleMode(h, &mut modus).is_err() }
}

/// Sperre nehmen, auch nach einer Panik in einem anderen Faden.
fn sperre<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// So lange wartet das Beenden hoechstens auf den Abschied an den Zuschauer
/// (wie der Mac-Host); nimmt er ihn nicht ab, wird gekappt.
const ABSCHIED_FRIST: Duration = Duration::from_secs(3);

/// Der Prozess endet (das Symbol bekam WM_CLOSE von aussen oder
/// WM_ENDSESSION): danach oeffnet der Dienst keinen Port mehr - auch nicht
/// mit einem Neuversuch am belegten Port, der gerade faellig waere.
static PROZESS_ENDET: AtomicBool = AtomicBool::new(false);

/// Die Freigabe endet: erst keine neuen Zuschauer mehr (netz::stoppen:
/// Ports zu, Bekanntgabe aus), dann der verbundene mit Abschied (`grund`,
/// hoechstens ABSCHIED_FRIST). Aus jedem Faden; ohne Lauf und ohne
/// Zuschauer tut es nichts.
fn abschied(grund: u8) {
    netz::stoppen();
    netz::abschied_beim_beenden(grund, ABSCHIED_FRIST);
}

/// Die eine App endet von aussen (WM_CLOSE an ihr Symbol, Abmelden,
/// Herunterfahren, Restart Manager): Ports zu, ein Zuschauer erfaehrt es
/// (Abschied, Grund 0) - im aufrufenden Faden (dem des Symbols), bevor er
/// zurueckkehrt: nach WM_ENDSESSION endet der Prozess womoeglich gleich
/// danach. Ohne laufende Host-Rolle tut es nichts.
pub fn abschied_beim_prozessende() {
    PROZESS_ENDET.store(true, Ordering::SeqCst);
    abschied(HOST_ENDE_BEENDET);
}

/// Was das Menue ueber die Host-Rolle zeigt: Zustand der Freigabe, ID,
/// Passwort, Geraete. `e`: ihr Einlass (None, solange er nicht steht);
/// `an`: die Freigabe ist eingeschaltet; `fehler`: warum die Rolle nicht in
/// Gang kam (Code wie die Exit-Codes der reinen Host-Rolle, 0: kein Fehler).
pub fn menue_teil(e: Option<&einlass::Einlass>, an: bool, port: u16, port_belegt: bool, fehler: u8) -> symbolmenue::HostTeil {
    use symbolmenue::Freigabe as F;
    let freigabe = if !an {
        F::Aus
    } else if fehler != 0 {
        F::Fehler(symbolmenue::Startfehler::aus_code(fehler))
    } else if port_belegt {
        F::PortBelegt(port)
    } else if let Some(n) = netz::zuschauer_name() {
        F::Verbunden(n)
    } else {
        F::Bereit
    };
    match e {
        Some(e) => symbolmenue::HostTeil {
            freigabe,
            id: Some(e.id()),
            passwort: e.passwort().map_err(|_| ()),
            geraete: e.geraete().map(|l| l.geraete).map_err(|_| ()),
        },
        None => symbolmenue::HostTeil { freigabe, id: None, passwort: Err(()), geraete: Err(()) },
    }
}

/// Was der Faden des Dienstes abarbeitet (Kanal aus dem Symbol, den
/// Fenstern und - in der einen App - aus main.rs).
enum Nachricht {
    /// Ein Menuepunkt der Host-Rolle (oder das Passwortfenster meldet
    /// "gespeichert").
    Aktion(oberflaeche::Aktion),
    /// Von aussen beendet (WM_CLOSE, WM_ENDSESSION am eigenen Symbol der
    /// reinen Host-Rolle): der Symbolfaden hat den Zuschauer schon
    /// verabschiedet.
    VonAussenBeendet,
    /// Die eine App: Freigabe an bzw. aus (Menue, Startbildschirm).
    FreigabeAn,
    FreigabeAus,
    /// Die eine App: der Geraetename ist neu (Nachricht 20).
    NameGeaendert,
    /// Die eine App: der Client spricht eine andere Sprache - Zulassen- und
    /// Passwortfenster, Rueckfragen und Hinweisblasen folgen ihr.
    Sprache(&'static crate::strings::Lang),
    /// Die eine App endet: Abschied mit Grund 0, dann Schluss.
    Beenden,
}

/// Laeuft der Dienst nach einem Schritt noch?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lage {
    Laeuft,
    /// Beendet (Freigabe beendet oder von aussen); der Zuschauer ist
    /// verabschiedet, die Ports sind zu, das Symbol ist weg.
    Beendet,
}

/// Das Symbol, an dem der Dienst haengt.
enum Symbolweg {
    /// Die reine Host-Rolle hat ihr eigenes (None: liess sich nicht anlegen).
    Eigenes(Option<crate::tray_win::Symbol>),
    /// Die eine App: ihr Symbol (main.rs) - ob es steht ("Zulassen" nur
    /// dann) und eine Sprechblase daran.
    App { steht: Arc<dyn Fn() -> bool + Send + Sync>, hinweis: Arc<dyn Fn(&str) + Send + Sync> },
}

impl Symbolweg {
    fn steht(&self) -> bool {
        match self {
            Symbolweg::Eigenes(s) => s.as_ref().is_some_and(|s| s.steht()),
            Symbolweg::App { steht, .. } => steht(),
        }
    }

    fn hinweis(&mut self, text: &str) {
        match self {
            Symbolweg::Eigenes(Some(s)) => {
                s.hinweis("QuadChroma", text);
            }
            Symbolweg::Eigenes(None) => {}
            Symbolweg::App { hinweis, .. } => hinweis(text),
        }
    }
}

/// Wie der Dienst eingerichtet wird: als reine Host-Rolle mit eigenem
/// Symbol (--nur-host) oder in der einen App an deren Symbol.
pub enum Art {
    NurHost,
    App { steht: Arc<dyn Fn() -> bool + Send + Sync>, hinweis: Arc<dyn Fn(&str) + Send + Sync> },
}

/// Die Host-Rolle als Dienst, ohne eigenen Prozess und ohne process::exit.
/// `einrichten` legt das Leichte an (Schluessel, Einlass, Symbol der reinen
/// Host-Rolle) - danach kennt das Menue ID, Passwort und Geraete;
/// `freigabe_an` bringt beim ersten Mal das Schwere in Gang (Bildschirme,
/// Encoder, Zwischenablage, Eingabe, Ton, Bildquelle) und oeffnet die Ports,
/// `freigabe_aus` schliesst sie wieder (mit Abschied an einen Zuschauer);
/// `takt` arbeitet einen Schritt ab (Nachrichten, Port-Neuversuch,
/// Taktzeile und Nachricht 6 alle fuenf Sekunden), `aktion` fuehrt einen
/// Menuepunkt aus, `beenden` verabschiedet den Zuschauer und schliesst die
/// Ports. Die reine Host-Rolle (main_host, --nur-host) faehrt ihn in einer
/// Schleife; die eine App in einem eigenen Faden (Rolle). `takt` und
/// `aktion` gehoeren in den Faden, der `einrichten` rief (Herkunft::Host
/// fuer die Protokollreihe). Hoechstens ein Dienst je Prozess: der Einlass
/// wird prozessweit eingerichtet, und Eingabe, Ton und Bildquelle laufen
/// nach dem ersten `freigabe_an`, bis der Prozess endet (ohne Zuschauer tun
/// sie nichts).
pub struct Dienst {
    port: u16,
    priv_key: Vec<u8>,
    einlass: Arc<einlass::Einlass>,
    lang: &'static crate::strings::Lang,
    zulassen: Arc<fenster::Zulassen>,
    symbol: Symbolweg,
    tx: std::sync::mpsc::Sender<Nachricht>,
    rx: std::sync::mpsc::Receiver<Nachricht>,
    port_belegt: Arc<AtomicBool>,
    /// Warum das Schwere beim letzten `freigabe_an` scheiterte (Code 6/7;
    /// 0: nicht gescheitert) - fuer die Zustandszeile am Symbol.
    startfehler: Arc<AtomicU8>,
    /// Die Argumente fuer das Schwere beim ersten `freigabe_an` (None:
    /// schon gestartet). Bleibt stehen, bis das Schwere gelang: ein spaeteres
    /// Einschalten versucht es dann neu.
    vorrat: Option<Vec<String>>,
    /// Das Schwere (`schweres_starten`; Tests setzen eine Attrappe).
    schwer: fn(&mut Dienst, &[String]) -> Result<(), i32>,
    /// Die Freigabe ist eingeschaltet.
    freigabe: bool,
    netz_laeuft: bool,
    /// Die eine App haelt den Mutex der Host-Rolle, solange die Freigabe an
    /// ist (die reine Host-Rolle haelt ihn in main_host).
    app: bool,
    instanz: Option<oberflaeche::Instanz>,
    /// Name dieses Mutex (Tests nehmen einen eigenen) und wie die Ports
    /// geoeffnet werden (Tests: auf Loopback).
    mutex: String,
    netz_starten: fn(u16, Vec<u8>) -> Result<(), String>,
    beendet: bool,
    // Taktzeile und Nachricht 6
    t0: Instant,
    vorher: Option<LastProbe>,
    last_frames: u64,
    last_bytes: u64,
    naechster_takt: Instant,
}

impl Dienst {
    /// Die reine Host-Rolle einrichten und starten (Argumente wie --host:
    /// Port, --output, --fps, --mbit, --fest, --konserve, --encoderweg).
    /// FFmpeg muss initialisiert sein, das Protokoll offen. Err: Exit-Code,
    /// wenn die Rolle nicht laufen kann (5 Schluessel oder Ablage, 6
    /// Konserve, 7 Encoderweg, 9 Port belegt ohne Oberflaeche).
    pub fn starten(args: &[String]) -> Result<Dienst, i32> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut d = Dienst::einrichten(args, Art::NurHost, tx, rx, Arc::new(AtomicBool::new(false)), Arc::new(AtomicU8::new(0)))?;
        d.freigabe_an()?;
        Ok(d)
    }

    /// Das Leichte einrichten: Port, Schluessel, Einlass (samt Uebernahme
    /// aus authorized.txt), Werte von der Befehlszeile, Zulassen-Fenster und
    /// bei der reinen Host-Rolle ihr Symbol. Oeffnet keinen Port und faengt
    /// keinen Bildschirm ab. Err: 5 (Schluessel oder Ablage).
    fn einrichten(
        args: &[String],
        art: Art,
        tx: std::sync::mpsc::Sender<Nachricht>,
        rx: std::sync::mpsc::Receiver<Nachricht>,
        port_belegt: Arc<AtomicBool>,
        startfehler: Arc<AtomicU8>,
    ) -> Result<Dienst, i32> {
        // Was dieser Faden ueber FFmpeg sagt, gehoert der Host-Rolle.
        protokoll::herkunft_setzen(protokoll::Herkunft::Host);

        let port = host_port(args);

        // Eigener dauerhafter Schluessel. Er entsteht beim ersten Start und
        // bleibt danach liegen, damit Gegenstellen den Host wiedererkennen.
        let (priv_key, pub_key) = match secure::host_identity() {
            Ok(k) => k,
            Err(e) => {
                log(format!("Schluessel konnte nicht angelegt werden - Abbruch: {e}"));
                return Err(5);
            }
        };
        // --pair und --forget gibt es nicht mehr (Spezifikation 6).
        for alt in ["--pair", "--forget"] {
            if args.iter().any(|a| a == alt) {
                log(format!("Unbekanntes Argument {alt} - uebergangen (neue Geraete kommen per Passwort oder \"Zulassen\" herein)"));
            }
        }

        // Einlass (Spezifikation 3, 4): Geraeteliste und Zugangspasswort im
        // Ablageordner; einmal aus authorized.txt uebernehmen.
        let ordner = match secure::config_dir() {
            Ok(o) => o,
            Err(e) => {
                log(format!("Kein Ablageordner - Abbruch: {e}"));
                return Err(5);
            }
        };
        match zugang::geraete_migrieren(&ordner, &zugang::heute()) {
            zugang::Migration::Keine => {}
            zugang::Migration::Uebernommen { anzahl, umbenannt } => log(format!(
                "Uebernahme: {anzahl} Geraete aus {} nach {}{}",
                zugang::ALTE_FREIGABEN,
                zugang::GERAETE_DATEI,
                match umbenannt {
                    Ok(()) => format!(", alte Liste heisst jetzt {}{}", zugang::ALTE_FREIGABEN, zugang::MIGRIERT),
                    Err(e) => format!(" - alte Liste nicht umbenannt ({e}), wird aber nicht noch einmal uebernommen"),
                }
            )),
            zugang::Migration::Fehler(e) => log(format!("Uebernahme aus {} gescheitert: {e} - naechster Start versucht es wieder", zugang::ALTE_FREIGABEN)),
        }
        for datei in [zugang::GERAETE_DATEI, zugang::PASSWORT_DATEI] {
            let n = zugang::zwischendateien_aufraeumen(&ordner.join(datei));
            if n > 0 {
                log(format!("{n} Zwischendateien von {datei} aus einem abgebrochenen Lauf entfernt"));
            }
        }
        // Der Name, den andere sehen: der eingestellte Geraetename
        // (einstellungen.txt), sonst der Rechnername.
        let name = zugang::geraetename();
        let einlass = Arc::new(einlass::Einlass::neu(
            ordner.join(zugang::GERAETE_DATEI),
            ordner.join(zugang::PASSWORT_DATEI),
            &pub_key,
            &name,
        ));
        let _ = einlass::einrichten(einlass.clone());
        if let Err(e) = einlass.passwort() {
            log(format!("Zugangspasswort: {e} - neue Geraete nur ueber \"Zulassen\" (Menue: Neues Zufallspasswort)"));
        }
        let erlaubt = match einlass.geraete() {
            Ok(l) => l.geraete.len().to_string(),
            Err(e) => format!("keins - {e}; niemand gilt als bekannt"),
        };
        log(format!(
            "Geraete-ID dieses Hosts: {}   Name: {name}{}   Fingerabdruck: {}   erlaubte Geraete: {erlaubt}",
            zugang::id_text(einlass.id()),
            if zugang::geraetename_eingestellt().is_some() { " (eingestellt)" } else { "" },
            noise::fingerprint(&pub_key)
        ));

        if let Some(f) = arg_zahl(args, "--fps") {
            Z.fps.store(f.clamp(10, 240), Ordering::Relaxed);
        }
        if let Some(m) = arg_zahl(args, "--mbit") {
            Z.mbit.store(m.clamp(2, 500), Ordering::Relaxed);
        }
        if args.iter().any(|a| a == "--fest" || a == "--fixed") {
            Z.fest.store(true, Ordering::Relaxed);
        }

        // Oberflaeche (Spezifikation 10): Zulassen-Fenster, deren Antwort an
        // den Einlass geht, und das Symbol im Infobereich - das eigene der
        // reinen Host-Rolle oder das der einen App. Die Sprache wie im
        // Fenster des Clients.
        let lang = match &crate::einstellungen::Einstellungen::laden().sprache {
            Some(c) => crate::strings::pick(c),
            None => crate::strings::pick(&crate::system_language()),
        };
        let zulassen = Arc::new(fenster::Zulassen::neu(
            lang,
            Arc::new(|nr, ja| {
                if let Some(e) = einlass::einlass() {
                    e.entscheiden(nr, ja);
                }
            }),
        ));
        einlass.oberflaeche_setzen(zulassen.clone());
        let app = matches!(art, Art::App { .. });
        let symbol = match art {
            Art::App { steht, hinweis } => Symbolweg::App { steht, hinweis },
            Art::NurHost => Symbolweg::Eigenes(eigenes_symbol(lang, port, &einlass, &port_belegt, &tx)),
        };
        let steht = symbol.steht();
        zulassen.vorhanden_setzen(steht);
        match &symbol {
            Symbolweg::Eigenes(Some(s)) if !steht => log(format!(
                "Infobereich: Symbol nicht angemeldet ({}) - \"Zulassen\" erst, wenn es steht",
                s.grund().unwrap_or_default()
            )),
            Symbolweg::Eigenes(Some(_)) => log("Infobereich: Symbol steht - \"Zulassen\" moeglich"),
            Symbolweg::Eigenes(None) => {}
            Symbolweg::App { .. } => log(format!(
                "Host-Rolle in der App: Symbol der App {}",
                if steht { "steht - \"Zulassen\" moeglich" } else { "steht (noch) nicht - \"Zulassen\" erst, wenn es steht" }
            )),
        }

        Ok(Dienst {
            port,
            priv_key,
            einlass,
            lang,
            zulassen,
            symbol,
            tx,
            rx,
            port_belegt,
            startfehler,
            vorrat: Some(args.to_vec()),
            schwer: Dienst::schweres_starten,
            freigabe: false,
            netz_laeuft: false,
            app,
            instanz: None,
            mutex: oberflaeche::MUTEX.to_string(),
            netz_starten: netz::start,
            beendet: false,
            t0: Instant::now(),
            vorher: last_probe(),
            last_frames: 0,
            last_bytes: 0,
            naechster_takt: Instant::now() + Duration::from_secs(5),
        })
    }

    /// Das Schwere, einmal je Prozess beim ersten `freigabe_an`:
    /// Bildschirme, Encoder, Konserve, Zwischenablage, Eingabe, Ton,
    /// Bildquelle. Err: 6 (Konserve), 7 (Encoderweg) - beides, bevor der
    /// erste Faden startet (Zwischenablage, Eingabe, Ton, Bildquelle): ein
    /// neuer Versuch faengt sauber von vorn an.
    fn schweres_starten(&mut self, args: &[String]) -> Result<(), i32> {
        // Bildschirm (Spezifikation Bildschirm 1.1-1.5): die Ausgaenge mit
        // Kennung und Name; der Wunsch aus bildschirm.txt, --output n pinnt
        // fuer diesen Lauf den Bildschirm am Listenplatz n (die Datei bleibt);
        // Ziel ist der Wunsch, sonst der Hauptbildschirm, sonst der erste.
        // Gemerkt wird die Kennung, nie der Listenplatz. Ohne DXGI-Ausgang
        // (WARP, RDP) kommt die Geometrie aus der GDI-Liste - fuer die Maus.
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
        // und der Wechsel); Begruessung (1) und Koennensliste (8) sagen ihm,
        // was laeuft.
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
                    return Err(6);
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
                    return Err(7);
                }
            },
            None => encoder::Weg::Auto,
        };

        // Empfangene Dateien frueherer Laeufe: aelter als 24 h weg (2.9),
        // dazu halb empfangene eines beendeten Prozesses (verwaiste Marke,
        // siehe dateien.rs). Die Host-Rolle hat ihre eigene Basis
        // (netz::host_ablage_basis).
        let alt = netz::host_ablage_aufraeumen();
        if alt > 0 {
            log(format!(
                "Dateien: {alt} Uebertragungen geloescht, aelter als 24 h oder verwaist ({})",
                netz::host_ablage_basis().display()
            ));
        }

        // Zwischenablage: was hier kopiert wird, geht zum Zuschauer - Text als
        // 48, eine Dateiliste ueber den Sender (50-52), beides auf dem
        // Bildkanal; neuer Inhalt bricht eine laufende Datei-Sendung ab. Was
        // von dort kommt, legt der Eingabefaden (Text) bzw. der Empfaenger der
        // Dateien ab (netz.rs). Der Waechter wartet dabei nie: das Senden der
        // Dateien laeuft in eigenen Faeden. Angemeldet als Host-Rolle, mit dem
        // Zuschauer als Gegenueber (der Client der App teilt sich den
        // Waechter).
        crate::clipboard::watch(protokoll::Herkunft::Host, netz::zuschauer_sitzung, |inhalt| match inhalt {
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
        // Encoder aus der Kandidatentabelle - beides erst, wenn jemand
        // zuschaut. Ohne Aufnahmefaden (Konserve, keine Bildquelle)
        // beantwortet der Eingabefaden einen Bildschirmwunsch (70) selbst mit
        // der unveraenderten Liste - niemand sonst holte ihn ab.
        if let Some(k) = konserve {
            aufnahme::ohne_aufnahme("Konserve als Bildquelle");
            konserve::abspielen(k);
        } else {
            match (&ausgang, startkandidat) {
                (Some(a), Some(_)) => {
                    let weg = encoder::weg_entscheiden(weg_cli, a.index);
                    aufnahme::start(wunsch, ausgaenge, weg_cli, weg);
                }
                (None, _) => {
                    aufnahme::ohne_aufnahme("kein DXGI-Ausgang");
                    log("Keine Bildquelle: kein DXGI-Ausgang fuer die Duplication (WARP/RDP) - ohne --konserve geht kein Bild raus");
                }
                (_, None) => {
                    aufnahme::ohne_aufnahme("kein Encoder");
                    log("Keine Bildquelle: kein Encoder auf diesem Rechner (weder nvenc noch h264_mf) - ohne --konserve geht kein Bild raus");
                }
            }
        }
        log(format!(
            "Strom: {}x{}, {} fps, {} Mbit/s, feste Bildrate {}",
            Z.info_w.load(Ordering::Relaxed),
            Z.info_h.load(Ordering::Relaxed),
            Z.fps.load(Ordering::Relaxed),
            Z.mbit.load(Ordering::Relaxed),
            if Z.fest.load(Ordering::Relaxed) { "an" } else { "aus" }
        ));
        Ok(())
    }

    /// Freigabe an: beim ersten Mal das Schwere starten; in der einen App den
    /// Mutex der Host-Rolle nehmen (haelt ihn schon ein anderer Prozess
    /// dieser Sitzung, wird wie am belegten Port alle 5 s neu versucht);
    /// dann die Ports oeffnen. Ist der Port belegt und gibt es eine
    /// Oberflaeche (Symbol, oder die App selbst), zeigt sie das, und es wird
    /// alle 5 s neu versucht; ohne endet die reine Host-Rolle wie bisher
    /// (Err 9). Err 6/7 aus dem Schweren: dann bleibt der Port zu - ohne
    /// Bildquelle, Eingabe, Ton und Zwischenablage gaebe es sonst einen
    /// halben Host -, die Freigabe gilt hier als aus (kein Neuversuch im
    /// Takt), und das naechste Einschalten versucht das Schwere von vorn.
    pub fn freigabe_an(&mut self) -> Result<(), i32> {
        if self.beendet || PROZESS_ENDET.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !self.freigabe {
            self.freigabe = true;
            if self.app {
                log("Freigabe: an");
            }
        }
        if let Some(args) = self.vorrat.clone() {
            if let Err(code) = (self.schwer)(self, &args) {
                self.freigabe = false;
                self.startfehler.store(code.clamp(1, 255) as u8, Ordering::SeqCst);
                log(format!(
                    "Freigabe: Host-Rolle nicht gestartet (Code {code}) - kein Port offen; aus- und wieder einschalten versucht es neu"
                ));
                return Err(code);
            }
            self.vorrat = None;
        }
        self.startfehler.store(0, Ordering::SeqCst);
        self.netz_versuchen(true)
    }

    /// Mutex (eine App) und Ports; `erster`: die Zeilen zum Anlauf.
    fn netz_versuchen(&mut self, erster: bool) -> Result<(), i32> {
        if self.netz_laeuft || PROZESS_ENDET.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.app && self.instanz.is_none() {
            match oberflaeche::einzelinstanz(&self.mutex) {
                Ok(Some(i)) => {
                    self.instanz = Some(i);
                    protokoll_nachholen();
                }
                Ok(None) => {
                    if erster {
                        log(format!(
                            "Freigabe: in dieser Sitzung laeuft schon eine Host-Rolle ({}) - neuer Versuch alle 5 s, das Menue zeigt Port {} belegt",
                            self.mutex, self.port
                        ));
                    }
                    self.port_belegt.store(true, Ordering::Relaxed);
                    return Ok(());
                }
                Err(e) => log(format!("Einzelinstanz nicht moeglich ({e}) - weiter ohne")),
            }
        }
        match (self.netz_starten)(self.port, self.priv_key.clone()) {
            Ok(()) => {
                self.netz_laeuft = true;
                self.port_belegt.store(false, Ordering::Relaxed);
                let p = self.port;
                log(format!("\n=== Dienst laeuft: Bild {p}, Eingabe {}, Bekanntgabe {} ===", p + 1, p + 2));
                Ok(())
            }
            Err(e) if self.app || self.symbol.steht() => {
                if erster {
                    log(format!("{e} - neuer Versuch alle 5 s, das Menue zeigt es"));
                }
                self.port_belegt.store(true, Ordering::Relaxed);
                // Den Mutex nicht halten, solange die Ports nicht offen sind:
                // eine andere Rolle darf sie haben.
                drop(self.instanz.take());
                Ok(())
            }
            Err(e) => {
                log(format!("{e}"));
                Err(9)
            }
        }
    }

    /// Freigabe aus (die eine App): keine neuen Zuschauer mehr, ein
    /// verbundener erfaehrt es (Abschied, `grund`), der Mutex ist frei.
    /// Eingabe, Ton und Bildquelle bleiben stehen - ohne Zuschauer tun sie
    /// nichts.
    pub fn freigabe_aus(&mut self, grund: u8) {
        if !self.freigabe {
            return;
        }
        self.freigabe = false;
        self.port_belegt.store(false, Ordering::Relaxed);
        if self.netz_laeuft {
            abschied(grund);
            self.netz_laeuft = false;
        }
        drop(self.instanz.take());
        log("Freigabe: aus");
    }

    /// Ein Schritt: wartet auf eine Nachricht - mit Freigabe hoechstens eine
    /// Sekunde (bis zum naechsten Fuenf-Sekunden-Takt), ohne bis zu einer
    /// Minute - und fuehrt sie aus, sieht nach, ob das Symbol steht
    /// ("Zulassen" moeglich), und erledigt im Takt den Neuversuch am
    /// belegten Port, die Zeilen von FFmpeg, die Drosseln und - nur mit
    /// Zuschauer - Taktzeile und Nachricht 6. Die Drosseln tragen immer
    /// nach, auch ohne Zuschauer: eine Flut kommt gerade dann, wenn keiner
    /// verbunden ist.
    pub fn takt(&mut self) -> Lage {
        if self.beendet {
            return Lage::Beendet;
        }
        let warten = if self.freigabe {
            self.naechster_takt.saturating_duration_since(Instant::now()).min(Duration::from_secs(1))
        } else {
            Duration::from_secs(60)
        };
        match self.rx.recv_timeout(warten) {
            Ok(Nachricht::Aktion(a)) => {
                if self.aktion(a) == Lage::Beendet {
                    return Lage::Beendet;
                }
            }
            Ok(Nachricht::VonAussenBeendet) => {
                // Abschied und Ports erledigte schon der Symbolfaden.
                self.beenden(HOST_ENDE_BEENDET);
                return Lage::Beendet;
            }
            Ok(Nachricht::FreigabeAn) => {
                // Scheitert das Schwere, sagt es freigabe_an im Protokoll und
                // in der Zustandszeile.
                let _ = self.freigabe_an();
            }
            Ok(Nachricht::FreigabeAus) => self.freigabe_aus(HOST_ENDE_FREIGABE_AUS),
            Ok(Nachricht::NameGeaendert) => {
                let n = zugang::geraetename();
                self.einlass.name_setzen(&n);
                log(format!("Geraetename: {n} - gilt fuer Bekanntgabe und Zugangsphase ab sofort"));
            }
            Ok(Nachricht::Sprache(l)) => self.sprache_setzen(l),
            Ok(Nachricht::Beenden) => {
                self.beenden(HOST_ENDE_BEENDET);
                return Lage::Beendet;
            }
            Err(_) => {}
        }
        self.zulassen.vorhanden_setzen(self.symbol.steht());
        if !self.freigabe || Instant::now() < self.naechster_takt {
            return Lage::Laeuft;
        }
        self.naechster_takt = Instant::now() + Duration::from_secs(5);
        if !self.netz_laeuft && self.netz_versuchen(false).is_ok() && self.netz_laeuft {
            log(format!("Port {} ist frei - Dienst laeuft", self.port));
        }
        ffmpeg_zeilen();
        netz::drosseln_nachtragen();
        let f = Z.sent_frames.load(Ordering::Relaxed);
        let b = Z.sent_bytes.load(Ordering::Relaxed);
        // Nur, wenn jemand zuschaut; sonst misst sich der Host selbst ohne Zweck.
        if !netz::zuschauer_da() {
            self.last_frames = f;
            self.last_bytes = b;
            self.vorher = last_probe();
            return Lage::Laeuft;
        }
        log(format!(
            "[{:.0} s] Bild: {} ({:.1}/s, {:.1} Mbit/s) | Ton: {} Pakete, {:.0} kB | Stau: {} | Encoder verworfen: {} | nachgelegt: {} | nachgeschoben: {} | zu schnell: {} | Encoder voll: {} | Ton verworfen: {}",
            self.t0.elapsed().as_secs_f32(),
            f,
            (f - self.last_frames) as f32 / 5.0,
            (b - self.last_bytes) as f64 * 8.0 / 5.0 / 1e6,
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
        if let (Some(v), Some(j)) = (self.vorher.as_ref(), jetzt.as_ref()) {
            let n = Z.enc_n.swap(0, Ordering::Relaxed);
            let summe = Z.enc_us.swap(0, Ordering::Relaxed);
            let enc_zehntel = if n > 0 { ((summe / n) / 100).min(u16::MAX as u64) as u16 } else { 0 };
            let host_fps_zehntel = (((f - self.last_frames) * 10) / 5).min(u16::MAX as u64) as u16;
            netz::send_small(MSG_LAST, &last_nachricht(v, j, enc_zehntel, host_fps_zehntel));
        }
        self.vorher = jetzt;
        self.last_frames = f;
        self.last_bytes = b;
        Lage::Laeuft
    }

    /// Die Sprache der Host-Rolle: Zulassen-Fenster (die naechsten),
    /// Passwortfenster, Rueckfragen und Hinweisblasen ("kopiert").
    fn sprache_setzen(&mut self, l: &'static crate::strings::Lang) {
        if std::ptr::eq(self.lang, l) {
            return;
        }
        self.lang = l;
        self.zulassen.sprache_setzen(l);
        log(format!("Sprache der Host-Rolle: {}", l.code));
    }

    /// Einen Menuepunkt ausfuehren. "Freigabe beenden" (nur die reine
    /// Host-Rolle) beendet den Dienst: der Zuschauer erfaehrt es (Abschied,
    /// Grund 1) und verbindet sich nicht von selbst neu.
    pub fn aktion(&mut self, a: oberflaeche::Aktion) -> Lage {
        use crate::strings::Key;
        use oberflaeche::Aktion;
        if self.beendet {
            return Lage::Beendet;
        }
        let (e, lang) = (&self.einlass, self.lang);
        let symbol = &mut self.symbol;
        let mut hinweis = |k: Key| symbol.hinweis(lang.get(k));
        match a {
            Aktion::IdKopieren => {
                crate::clipboard::set(&zugang::id_ziffern(e.id()));
                log("Geraete-ID in die Zwischenablage kopiert");
                hinweis(Key::HostCopied);
            }
            Aktion::PasswortKopieren => {
                if let Ok(pw) = e.passwort() {
                    // clipboard::set markiert den Eintrag als verdeckt: kein
                    // Verlauf, keine Cloud, und der Ablagewaechter schickt ihn
                    // an keine Rolle weiter.
                    crate::clipboard::set(&pw);
                    log("Zugangspasswort in die Zwischenablage kopiert (verdeckt)");
                    hinweis(Key::HostCopied);
                }
            }
            Aktion::PasswortAendern => {
                let (e2, tx2) = (e.clone(), self.tx.clone());
                fenster::passwort_aendern(
                    lang,
                    Box::new(move |pw| e2.passwort_setzen(pw)),
                    Box::new(move || {
                        let _ = tx2.send(Nachricht::Aktion(Aktion::PasswortGespeichert));
                    }),
                );
            }
            Aktion::PasswortGespeichert => hinweis(Key::HostPasswordSaved),
            Aktion::Zufallspasswort => {
                let _ = e.passwort_zufall();
            }
            Aktion::Entfernen(k) => {
                // Wer entfernt ist, bleibt nicht verbunden - er erfaehrt es
                // (Abschied, Grund 2) und verbindet sich nicht von selbst neu.
                if e.geraet_entfernen(&k).is_ok() {
                    netz::zuschauer_verabschieden(Some(&k), HOST_ENDE_ENTFERNT);
                }
            }
            Aktion::AlleEntfernen => {
                let e2 = e.clone();
                fenster::rueckfrage(
                    lang.get(Key::HostRemoveAllAsk),
                    Box::new(move || {
                        if e2.alle_entfernen().is_ok() {
                            netz::zuschauer_verabschieden(None, HOST_ENDE_ENTFERNT);
                        }
                    }),
                );
            }
            Aktion::ListeZuruecksetzen => {
                let _ = e.liste_zuruecksetzen();
            }
            Aktion::Beenden => {
                log("Freigabe beendet (Infobereich)");
                self.beenden(HOST_ENDE_FREIGABE_AUS);
                return Lage::Beendet;
            }
        }
        Lage::Laeuft
    }

    /// Den Dienst beenden: keine neuen Zuschauer mehr (Ports zu, Bekanntgabe
    /// aus), der verbundene erfaehrt es mit Abschied (`grund`, hoechstens
    /// ABSCHIED_FRIST), dann ist das eigene Symbol weg. Ein zweiter Aufruf
    /// tut nichts. Zwischenablage, Eingabe, Ton und Bildquelle bleiben
    /// stehen - ohne Zuschauer tun sie nichts.
    pub fn beenden(&mut self, grund: u8) {
        if self.beendet {
            return;
        }
        self.beendet = true;
        abschied(grund);
        self.netz_laeuft = false;
        drop(self.instanz.take());
        if let Symbolweg::Eigenes(s) = &mut self.symbol {
            drop(s.take());
        }
    }
}

/// Der Port von --host bzw. --nur-host (Vorgabe 9001).
pub fn host_port(args: &[String]) -> u16 {
    args.iter()
        .position(|a| a == "--host" || a == "--nur-host")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(9001)
}

/// Das eigene Symbol der reinen Host-Rolle: Menue nach symbolmenue
/// (Art::NurHost), bei jedem Oeffnen frisch gebaut; die Wahl geht als
/// Nachricht an den Dienst. WM_CLOSE von aussen, Abmelden, Herunterfahren,
/// ein Installationsprogramm (Restart Manager): beenden wie "Freigabe
/// beenden", aber mit Grund 0 (die Host-Rolle wurde beendet). Das laeuft im
/// Symbolfaden und verabschiedet selbst - nach WM_ENDSESSION endet der
/// Prozess womoeglich gleich nach der Rueckkehr -, dann erfaehrt es der
/// Faden des Dienstes und endet ebenfalls.
fn eigenes_symbol(
    lang: &'static crate::strings::Lang,
    port: u16,
    einlass: &Arc<einlass::Einlass>,
    port_belegt: &Arc<AtomicBool>,
    tx: &std::sync::mpsc::Sender<Nachricht>,
) -> Option<crate::tray_win::Symbol> {
    let zuordnung: Arc<Mutex<symbolmenue::Zuordnung>> = Arc::new(Mutex::new(symbolmenue::Zuordnung::default()));
    let (tx2, z) = (tx.clone(), zuordnung.clone());
    let befehl = Box::new(move |nr: u32| {
        let a = symbolmenue::aktion_zu(nr, &sperre(&z));
        if let Some(a) = a.as_ref().and_then(oberflaeche::Aktion::aus_menue) {
            let _ = tx2.send(Nachricht::Aktion(a));
        }
    });
    let (e, pb, z) = (einlass.clone(), port_belegt.clone(), zuordnung.clone());
    let menue = Box::new(move || {
        let h = menue_teil(Some(&e), true, port, pb.load(Ordering::Relaxed), 0);
        let stand = symbolmenue::MenueStand {
            art: symbolmenue::Art::NurHost,
            name: zugang::geraetename(),
            freigabe: h.freigabe,
            id: h.id,
            passwort: h.passwort,
            geraete: h.geraete,
            hosts: Vec::new(),
            autostart: false,
            autostart_gesperrt: false,
            ruhe_verhindern: false,
            ruhe_abgelehnt: None,
        };
        let (m, zu) = symbolmenue::menue(lang, &stand);
        *sperre(&z) = zu;
        m
    });
    let tx3 = tx.clone();
    let ende = Box::new(move |wie: &str| {
        log(format!("Host-Rolle wird beendet ({wie})"));
        abschied(HOST_ENDE_BEENDET);
        log("Host-Rolle beendet");
        let _ = tx3.send(Nachricht::VonAussenBeendet);
    });
    let tooltip = symbolmenue::tooltip(lang, None, true, Some(einlass.id()));
    match crate::tray_win::Symbol::neu(befehl, menue, ende, None, &tooltip) {
        Ok(s) => Some(s),
        Err(e) => {
            log(format!("Infobereich: kein Symbol ({e}) - ohne Oberflaeche, neue Geraete nur per Passwort"));
            None
        }
    }
}

// ------------------------------------------------------ Die Rolle in der App

/// Was die eine App ueber ihre Host-Rolle weiss, ohne auf deren Faden zu
/// warten (Menue, Startbildschirm).
struct RollenStand {
    einlass: std::sync::OnceLock<Arc<einlass::Einlass>>,
    port_belegt: Arc<AtomicBool>,
    /// Die Freigabe ist (vom Nutzer) eingeschaltet.
    freigabe: AtomicBool,
    /// Warum die Rolle nicht in Gang kam: 5 Schluessel oder Ablage, 6
    /// Konserve, 7 Encoderweg, 1 sonst (FFmpeg, kein Faden); 0 kein Fehler.
    /// Einzelheiten in host-protokoll.txt. Der Dienst setzt 6/7 und loescht
    /// sie, sobald ein neues Einschalten gelingt.
    fehler: Arc<AtomicU8>,
}

/// RollenStand::fehler: die Rolle kam aus einem anderen Grund als
/// Schluessel, Konserve oder Encoderweg nicht in Gang.
const FEHLER_SONST: u8 = 1;

/// Die Host-Rolle im Prozess der einen App (Plan W6): ein eigener Faden
/// richtet den Dienst ein und faehrt ihn, bis die App endet. Die App
/// schaltet die Freigabe hierueber an und aus (einstellungen.txt: freigabe),
/// reicht die Menuepunkte der Host-Rolle weiter, meldet einen neuen
/// Geraetenamen und beendet die Rolle beim Beenden (Abschied, Grund 0).
pub struct Rolle {
    tx: std::sync::mpsc::Sender<Nachricht>,
    stand: Arc<RollenStand>,
    port: u16,
    faden: Option<std::thread::JoinHandle<()>>,
}

impl Rolle {
    /// Den Faden der Host-Rolle starten. `args`: die Befehlszeile (Port
    /// hinter --host, --konserve, --output, ...); `freigabe`: gleich
    /// freigeben; `steht`/`hinweis`: das Symbol der App; `bereit` kommt,
    /// sobald der Einlass steht (ID bekannt - der Tooltip kann sie zeigen).
    /// Kehrt sofort zurueck - Schluessel, Einlass und Protokoll richtet der
    /// Faden ein.
    pub fn starten(
        args: Vec<String>,
        freigabe: bool,
        steht: Arc<dyn Fn() -> bool + Send + Sync>,
        hinweis: Arc<dyn Fn(&str) + Send + Sync>,
        bereit: Arc<dyn Fn() + Send + Sync>,
    ) -> Rolle {
        let (tx, rx) = std::sync::mpsc::channel();
        let port = host_port(&args);
        let stand = Arc::new(RollenStand {
            einlass: std::sync::OnceLock::new(),
            port_belegt: Arc::new(AtomicBool::new(false)),
            freigabe: AtomicBool::new(freigabe),
            fehler: Arc::new(AtomicU8::new(0)),
        });
        let (st, tx2) = (stand.clone(), tx.clone());
        let faden = std::thread::Builder::new()
            .name("qc-host-rolle".into())
            .spawn(move || rolle_laufen(args, st, steht, hinweis, bereit, tx2, rx))
            .map_err(|e| {
                protokoll::zeile(format!("Host-Rolle: kein Faden ({e}) - dieser PC ist nicht freigegeben"));
                stand.fehler.store(FEHLER_SONST, Ordering::SeqCst);
            })
            .ok();
        Rolle { tx, stand, port, faden }
    }

    /// Freigabe an oder aus (aus: ein Zuschauer erfaehrt es mit Grund 1).
    pub fn freigabe_setzen(&self, an: bool) {
        self.stand.freigabe.store(an, Ordering::SeqCst);
        let _ = self.tx.send(if an { Nachricht::FreigabeAn } else { Nachricht::FreigabeAus });
    }

    /// Ein Menuepunkt der Host-Rolle.
    pub fn aktion(&self, a: oberflaeche::Aktion) {
        let _ = self.tx.send(Nachricht::Aktion(a));
    }

    /// Der Geraetename ist neu (zugang::geraetename_setzen ist schon
    /// geschehen): Nachricht 20 ab der naechsten Zugangsphase.
    pub fn name_geaendert(&self) {
        let _ = self.tx.send(Nachricht::NameGeaendert);
    }

    /// Der Client spricht jetzt `lang`: die Fenster und Hinweise der
    /// Host-Rolle ebenso, ab sofort.
    pub fn sprache_setzen(&self, lang: &'static crate::strings::Lang) {
        let _ = self.tx.send(Nachricht::Sprache(lang));
    }

    /// Geraete-ID der Host-Rolle (None, solange ihr Einlass nicht steht).
    pub fn id(&self) -> Option<u32> {
        self.stand.einlass.get().map(|e| e.id())
    }

    /// Was das Menue ueber die Host-Rolle zeigt (siehe menue_teil) - als
    /// Abfrage fuer den Faden des Symbols, der das Menue bei jedem Oeffnen
    /// baut.
    pub fn menue_abfrage(&self) -> Arc<dyn Fn() -> symbolmenue::HostTeil + Send + Sync> {
        let (s, port) = (self.stand.clone(), self.port);
        Arc::new(move || {
            menue_teil(
                s.einlass.get().map(|e| e.as_ref()),
                s.freigabe.load(Ordering::SeqCst),
                port,
                s.port_belegt.load(Ordering::Relaxed),
                s.fehler.load(Ordering::SeqCst),
            )
        })
    }

    /// Die Rolle beenden: Ports zu, ein Zuschauer erfaehrt es (Abschied,
    /// Grund 0). Wartet hoechstens `frist` auf den Faden.
    pub fn beenden(&mut self, frist: Duration) {
        let _ = self.tx.send(Nachricht::Beenden);
        let Some(f) = self.faden.take() else { return };
        let bis = Instant::now() + frist;
        while !f.is_finished() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(10));
        }
        if f.is_finished() {
            let _ = f.join();
        } else {
            protokoll::zeile(format!("Host-Rolle endet nicht binnen {} s - die App endet trotzdem", frist.as_secs()));
        }
    }
}

/// host-protokoll.txt ist offen (einmal je Prozess geoeffnet).
static PROTOKOLL_OFFEN: AtomicBool = AtomicBool::new(false);

// Wer host-protokoll.txt schreibt, haelt den Mutex protokoll_mutex() bis
// zum Ende seines Prozesses - die App auch mit Freigabe aus (sie oeffnet die
// Datei beim Start). Werkzeuge (--list) und eine reine Host-Rolle
// (--nur-host) beginnen die Datei nur neu, wenn niemand ihn haelt.
/// Tests nehmen einen eigenen Namen je Prozess (ihr Protokoll liegt ohnehin
/// in der Testablage).
fn protokoll_mutex() -> String {
    if cfg!(test) {
        format!("Local\\QuadChroma-Host-Protokoll-Test-{}", std::process::id())
    } else {
        "Local\\QuadChroma-Host-Protokoll".into()
    }
}

/// host-protokoll.txt oeffnen (neu beginnen), falls noch nicht geschehen -
/// aber nur, wenn kein anderer Prozess dieser Sitzung sie schreibt: neu
/// beginnen leerte sonst dessen Protokoll. true: sie ist (jetzt) offen.
fn protokoll_nachholen() -> bool {
    protokoll_nachholen_mit(&protokoll_mutex())
}

fn protokoll_nachholen_mit(mutex: &str) -> bool {
    if PROTOKOLL_OFFEN.load(Ordering::SeqCst) {
        return true;
    }
    if !protokoll_mutex_nehmen(mutex) {
        return false;
    }
    if !PROTOKOLL_OFFEN.swap(true, Ordering::SeqCst) {
        protokoll_oeffnen("host-protokoll.txt");
    }
    true
}

/// Den Mutex des Protokolls nehmen und bis zum Ende des Prozesses halten.
/// false: ein anderer Prozess haelt ihn (er schreibt host-protokoll.txt).
/// Geht es gar nicht (kein Mutex anlegbar), zaehlt das als frei - wie
/// bisher ohne Absprache.
fn protokoll_mutex_nehmen(mutex: &str) -> bool {
    match oberflaeche::einzelinstanz(mutex) {
        // Mit dem Prozess faellt er.
        Ok(Some(i)) => {
            std::mem::forget(i);
            true
        }
        Ok(None) => false,
        Err(e) => {
            log(format!("{mutex}: {e} - host-protokoll.txt ohne Absprache mit anderen Prozessen"));
            true
        }
    }
}

/// Fuer die Werkzeuge: host-protokoll.txt, wenn sie frei ist, sonst eine
/// eigene Datei `eigene` (neu begonnen) - die der App bleibt, wie sie ist.
/// `rolle_pruefen`: auch eine laufende Host-Rolle (Mutex der Freigabe)
/// zaehlt als Schreiber - eine fruehere Fassung haelt den Mutex des
/// Protokolls noch nicht (--list; --nur-host haelt den der Freigabe selbst).
fn protokoll_fuer_werkzeug(eigene: &str, rolle_pruefen: bool) {
    let rolle_fremd = rolle_pruefen && oberflaeche::laeuft(oberflaeche::MUTEX);
    if !rolle_fremd && protokoll_nachholen() {
        return;
    }
    protokoll_oeffnen(eigene);
    log(format!("host-protokoll.txt schreibt ein anderer Prozess dieser Sitzung (die App) - dieser Lauf schreibt nach {eigene}"));
}

/// Der Faden der Host-Rolle in der einen App.
fn rolle_laufen(
    args: Vec<String>,
    stand: Arc<RollenStand>,
    steht: Arc<dyn Fn() -> bool + Send + Sync>,
    hinweis: Arc<dyn Fn(&str) + Send + Sync>,
    bereit: Arc<dyn Fn() + Send + Sync>,
    tx: std::sync::mpsc::Sender<Nachricht>,
    rx: std::sync::mpsc::Receiver<Nachricht>,
) {
    protokoll::herkunft_setzen(protokoll::Herkunft::Host);
    // Schreibt noch ein anderer Prozess host-protokoll.txt (eine reine
    // Host-Rolle, ein Werkzeug - oder eine fruehere Fassung, die nur den
    // Mutex der Freigabe haelt), holt die Rolle es nach, sobald sie die
    // Ports hat (netz_versuchen).
    if !oberflaeche::laeuft(oberflaeche::MUTEX) {
        protokoll_nachholen();
    }
    if let Err(e) = ffmpeg_next::init() {
        log(format!("FFmpeg-Start fehlgeschlagen: {e} - dieser PC wird nicht freigegeben"));
        stand.fehler.store(FEHLER_SONST, Ordering::SeqCst);
        return;
    }
    // Die Reihe der Host-Rolle: Warnungen und Fehler von FFmpeg.
    protokoll::einschalten(false);
    log("Host-Rolle im Prozess der App");
    let mut d = match Dienst::einrichten(&args, Art::App { steht, hinweis }, tx, rx, stand.port_belegt.clone(), stand.fehler.clone()) {
        Ok(d) => d,
        Err(code) => {
            log(format!("Host-Rolle nicht eingerichtet (Code {code}) - dieser PC wird nicht freigegeben"));
            stand.fehler.store(code.clamp(1, 255) as u8, Ordering::SeqCst);
            return;
        }
    };
    let _ = stand.einlass.set(d.einlass.clone());
    bereit();
    if stand.freigabe.load(Ordering::SeqCst) {
        // Scheitert es, steht der Grund schon in stand.fehler (der Dienst
        // setzt ihn) und im Protokoll.
        let _ = d.freigabe_an();
    } else {
        log("Freigabe: aus (einstellungen.txt) - kein Port offen");
    }
    while d.takt() == Lage::Laeuft {}
    log("Host-Rolle beendet");
}

/// Rollenwahl der reinen Host-Prozesse: --list, --messen oder --nur-host.
/// Rueckgabe ist der Exit-Code; das Beenden des Prozesses bleibt dem
/// Aufrufer (main.rs).
pub fn main_host(args: &[String]) -> i32 {
    // Einzelinstanz der Freigabe (Spezifikation 10.1) - vor allem anderen,
    // auch vor der Protokolldatei: ein zweiter Start leerte sonst das
    // Protokoll des laufenden.
    let freigabe = !args.iter().any(|a| a == "--list" || a == "--messen");
    let mut instanz_fehler = None;
    let _instanz = if freigabe {
        match oberflaeche::einzelinstanz(oberflaeche::MUTEX) {
            Ok(Some(i)) => Some(i),
            Ok(None) => {
                println!("Die Freigabe laeuft in dieser Sitzung schon ({}) - dieser Start endet.", oberflaeche::MUTEX);
                return 0;
            }
            Err(e) => {
                instanz_fehler = Some(e);
                None
            }
        }
    } else {
        None
    };
    if freigabe && ausgabe_ins_leere() {
        // Ohne Leser gestartet (Ausgaben nach NUL): haengt dieser Prozess
        // trotzdem an einer geerbten Konsole (AttachConsole in main.rs),
        // endete er mit ihr. Also loslassen; das Protokoll steht in
        // host-protokoll.txt.
        unsafe {
            let _ = windows::Win32::System::Console::FreeConsole();
        }
    }
    // Vor allem anderen, das Ausgaenge, Bildschirm oder Maus anfasst -
    // fuer alle Hostrollen.
    let dpi = dpi_bewusst();
    if let Err(e) = ffmpeg_next::init() {
        println!("FFmpeg-Start fehlgeschlagen: {e}");
        return 2;
    }
    // FFmpegs Meldungen laufen ueber den Rueckruf des Clients in die Reihe
    // des Hosts; der Host holt sie ab und schreibt sie in sein eigenes
    // Protokoll. Dieser Prozess spielt nur die Host-Rolle: auch Faeden ohne
    // eigene Herkunft (Ablagewaechter, Netz, FFmpegs eigene) zaehlen zu ihr.
    protokoll::standard_setzen(protokoll::Herkunft::Host);
    protokoll::herkunft_setzen(protokoll::Herkunft::Host);
    protokoll::einschalten(false);

    if args.iter().any(|a| a == "--messen") {
        protokoll_oeffnen("messung.txt");
        log(&dpi);
        return messen::laufen(args);
    }

    if args.iter().any(|a| a == "--list") {
        // --list laeuft ohne Einzelinstanz (neben der App erlaubt). Schreibt
        // die App host-protokoll.txt (auch mit Freigabe aus), bekommt --list
        // eine eigene Datei - neu beginnen leerte sonst ihr Protokoll.
        protokoll_fuer_werkzeug("host-liste.txt", true);
        log(&dpi);
        aufnahme::ausgaenge_melden(&mut Vec::new());
        encoder::pruefen();
        encoder::mf_pruefen();
        ffmpeg_zeilen();
        return 0;
    }

    // --nur-host: laeuft die App (Freigabe aus - sonst haette sie den Mutex
    // der Host-Rolle, und dieser Start waere oben schon zu Ende), gehoert ihr
    // host-protokoll.txt.
    protokoll_fuer_werkzeug("host-protokoll-nur-host.txt", false);
    log(&dpi);
    if let Some(e) = instanz_fehler {
        log(format!("Einzelinstanz nicht moeglich ({e}) - weiter ohne"));
    }
    // Der Geraetename, den andere sehen, gilt auch fuer die reine Host-Rolle.
    zugang::geraetename_setzen(crate::einstellungen::Einstellungen::laden().geraetename);

    // --nur-host: der Dienst, bis er beendet ist (Menue oder von aussen).
    let mut dienst = match Dienst::starten(args) {
        Ok(d) => d,
        Err(code) => return code,
    };
    while dienst.takt() == Lage::Laeuft {}
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wie der Knopf "Diesen PC freigeben" startet (Ausgaben nach NUL,
    /// DETACHED_PROCESS; danach holt main.rs mit AttachConsole die Konsole
    /// des Elternprozesses): `ausgabe_ins_leere` sagt ja, und die Rolle
    /// laesst die Konsole los. Mit einer Ausgabe, die jemand liest (Pipe),
    /// sagt sie nein. Der Kindprozess ist dieser Test selbst (QC_TEST_AUSGABE).
    #[test]
    fn start_ohne_ausgabe_braucht_keine_konsole() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        if let Some(ziel) = std::env::var_os("QC_TEST_AUSGABE") {
            unsafe {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
            std::fs::write(ziel, if ausgabe_ins_leere() { "leer" } else { "gelesen" }).unwrap();
            return;
        }
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let ordner = std::env::temp_dir().join(format!("{}-ausgabe", secure::test_lauf()));
        std::fs::create_dir_all(&ordner).unwrap();
        let lauf = |name: &str, ausgabe: Stdio| {
            let datei = ordner.join(name);
            let _ = std::fs::remove_file(&datei);
            let r = Command::new(std::env::current_exe().unwrap())
                .args(["host::tests::start_ohne_ausgabe_braucht_keine_konsole", "--exact", "--test-threads=1"])
                .env("QC_TEST_AUSGABE", &datei)
                .stdin(Stdio::null())
                .stdout(ausgabe)
                .stderr(Stdio::null())
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .output()
                .unwrap();
            assert!(r.status.success(), "{name}: {:?}", r.status);
            std::fs::read_to_string(&datei).unwrap_or_default()
        };
        assert_eq!(lauf("null.txt", Stdio::null()), "leer");
        assert_eq!(lauf("pipe.txt", Stdio::piped()), "gelesen");
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Das Protokoll waechst nicht ohne Ende (Integrationstest: jede
    /// Muell-Verbindung eine Zeile, ohne Grenze): ueber der Grenze wird die
    /// Datei zu <name>.alt.txt, und eine neue beginnt mit einem Hinweis.
    /// Auf der Platte liegen nie mehr als zweimal die Grenze, und die
    /// neueste Zeile steht immer in der Datei selbst.
    #[test]
    fn protokoll_hat_eine_obergrenze() {
        let ordner = secure::test_ordner("protokoll", "obergrenze");
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

    /// Ein Dienst ohne einrichten (kein Symbol, keine Aufnahme, nicht der
    /// prozessweite Einlass): Einlass mit eigenem Ordner, Zuschauerplatz auf
    /// Loopback. `app`: wie in der einen App (eigener Mutex-Name, Freigabe
    /// noch aus); sonst die reine Host-Rolle mit schon offenen Ports.
    fn test_dienst(name: &str, app: bool) -> Dienst {
        let ordner = secure::test_ordner("dienst", name);
        let (priv_key, pub_key) = noise::keypair().unwrap();
        let einlass = Arc::new(einlass::Einlass::neu(
            ordner.join(zugang::GERAETE_DATEI),
            ordner.join(zugang::PASSWORT_DATEI),
            &pub_key,
            "Testhost",
        ));
        let lang = crate::strings::pick("de");
        let zulassen = Arc::new(fenster::Zulassen::neu(lang, Arc::new(|_, _| {})));
        let port = netz::start_loopback(&priv_key);
        if app {
            netz::stoppen();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        Dienst {
            port,
            priv_key,
            einlass,
            lang,
            zulassen,
            symbol: Symbolweg::Eigenes(None),
            tx,
            rx,
            port_belegt: Arc::new(AtomicBool::new(false)),
            startfehler: Arc::new(AtomicU8::new(0)),
            vorrat: None,
            schwer: |_, _| Ok(()),
            freigabe: !app,
            netz_laeuft: !app,
            app,
            instanz: None,
            mutex: format!("Local\\QuadChroma-Host-Test-Dienst-{}-{name}", std::process::id()),
            netz_starten: netz::start_loopback_an,
            beendet: false,
            t0: Instant::now(),
            vorher: None,
            last_frames: 0,
            last_bytes: 0,
            naechster_takt: Instant::now() + Duration::from_secs(5),
        }
    }

    /// Nimmt der Port Verbindungen an?
    fn offen(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(6)).is_ok()
    }

    /// W6: die Freigabe der einen App an, aus, wieder an - ueber Nachrichten
    /// wie aus dem Menue. An: Ports offen, der Mutex der Host-Rolle gehalten;
    /// aus: Ports zu, Mutex frei, der Dienst laeuft weiter. Haelt ein anderer
    /// den Mutex (eine zweite Host-Rolle der Sitzung), bleiben die Ports zu,
    /// das Menue zeigt "belegt", und im Takt wird neu versucht, bis er frei
    /// ist. Beenden (die App endet) schliesst alles.
    #[test]
    fn freigabe_an_aus_an() {
        let _platz = netz::platz_pruefung();
        let mut d = test_dienst("freigabe", true);
        let (port, mutex) = (d.port, d.mutex.clone());
        assert!(!netz::laeuft() && !oberflaeche::laeuft(&mutex));
        let tx = d.tx.clone();
        tx.send(Nachricht::FreigabeAn).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert!(netz::laeuft() && offen(port), "Freigabe an: Port zu");
        assert!(oberflaeche::laeuft(&mutex), "Freigabe an ohne Mutex");
        assert_eq!(menue_teil(Some(&d.einlass), d.freigabe, port, d.port_belegt.load(Ordering::Relaxed), 0).freigabe, symbolmenue::Freigabe::Bereit);
        tx.send(Nachricht::FreigabeAus).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert!(!netz::laeuft() && !offen(port), "Freigabe aus: Port offen");
        assert!(!oberflaeche::laeuft(&mutex), "Freigabe aus: Mutex gehalten");
        assert_eq!(menue_teil(Some(&d.einlass), d.freigabe, port, false, 0).freigabe, symbolmenue::Freigabe::Aus);
        // Ein Menuepunkt wirkt auch ohne Freigabe.
        assert_eq!(d.aktion(oberflaeche::Aktion::Zufallspasswort), Lage::Laeuft);
        assert!(d.einlass.passwort().is_ok());

        // Eine andere Host-Rolle haelt den Mutex: kein Port, "belegt".
        let fremd = oberflaeche::einzelinstanz(&mutex).unwrap().expect("Mutex");
        tx.send(Nachricht::FreigabeAn).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert!(!netz::laeuft());
        assert!(d.port_belegt.load(Ordering::Relaxed));
        assert_eq!(menue_teil(Some(&d.einlass), d.freigabe, port, true, 0).freigabe, symbolmenue::Freigabe::PortBelegt(port));
        drop(fremd);
        d.naechster_takt = Instant::now();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert!(netz::laeuft() && offen(port), "Neuversuch im Takt: Port zu");
        assert!(!d.port_belegt.load(Ordering::Relaxed));
        assert!(oberflaeche::laeuft(&mutex));

        tx.send(Nachricht::Beenden).unwrap();
        assert_eq!(d.takt(), Lage::Beendet);
        assert!(!netz::laeuft() && !oberflaeche::laeuft(&mutex));
        assert_eq!(d.takt(), Lage::Beendet);
    }

    /// host-protokoll.txt gehoert dem, der den Mutex des Protokolls haelt:
    /// haelt ihn ein anderer (hier dieser Test anstelle der App), bekommen
    /// Werkzeuge und --nur-host ihn nicht - sie schreiben eine eigene
    /// Datei; ist er frei, nimmt ihn der Erste und behaelt ihn.
    #[test]
    fn protokoll_nur_fuer_den_halter() {
        let name = format!("Local\\QuadChroma-Host-Protokoll-Probe-{}", std::process::id());
        let app = oberflaeche::einzelinstanz(&name).unwrap().expect("Mutex frei");
        assert!(!protokoll_mutex_nehmen(&name), "die App haelt ihn - das Werkzeug darf nicht");
        drop(app);
        assert!(protokoll_mutex_nehmen(&name), "frei - das Werkzeug nimmt ihn");
        assert!(oberflaeche::laeuft(&name), "gehalten bis zum Ende des Prozesses");
        assert!(!protokoll_mutex_nehmen(&name), "ein zweiter bekommt ihn nicht");
        assert_ne!(protokoll_mutex(), "Local\\QuadChroma-Host-Protokoll", "Tests teilen den Mutex nicht mit der App");
    }

    /// Die Sprache des Clients wechselt: die Host-Rolle folgt sofort -
    /// Hinweise, Rueckfragen, Passwortfenster und die naechsten
    /// Zulassen-Fenster.
    #[test]
    fn sprache_folgt_dem_client() {
        let _platz = netz::platz_pruefung();
        let mut d = test_dienst("sprache", true);
        assert_eq!((d.lang.code, d.zulassen.sprache().code), ("de", "de"));
        d.tx.send(Nachricht::Sprache(crate::strings::pick("fr"))).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!((d.lang.code, d.zulassen.sprache().code), ("fr", "fr"));
        d.tx.send(Nachricht::Sprache(crate::strings::pick("en"))).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!((d.lang.code, d.zulassen.sprache().code), ("en", "en"));
    }

    /// Scheitert das Schwere beim ersten Einschalten (hier eine Attrappe mit
    /// Code 7), bleibt der Port zu - auch im Takt danach -, die
    /// Zustandszeile nennt den Grund, und das naechste Einschalten versucht
    /// das Schwere von vorn: gelingt es, oeffnen die Ports, und der Grund ist
    /// weg. Ein Ausschalten dazwischen stoert nicht.
    #[test]
    fn schweres_scheitert_dann_neuer_versuch() {
        use symbolmenue::{Freigabe as F, Startfehler};
        static VERSUCHE: AtomicU32 = AtomicU32::new(0);
        fn schwer(_: &mut Dienst, args: &[String]) -> Result<(), i32> {
            assert_eq!(args, ["--encoderweg", "x"]);
            if VERSUCHE.fetch_add(1, Ordering::SeqCst) == 0 { Err(7) } else { Ok(()) }
        }
        let _platz = netz::platz_pruefung();
        let mut d = test_dienst("schwer", true);
        d.vorrat = Some(vec!["--encoderweg".into(), "x".into()]);
        d.schwer = schwer;
        let port = d.port;
        let teil = |d: &Dienst| menue_teil(Some(&d.einlass), true, port, d.port_belegt.load(Ordering::Relaxed), d.startfehler.load(Ordering::SeqCst)).freigabe;
        d.tx.send(Nachricht::FreigabeAn).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!(VERSUCHE.load(Ordering::SeqCst), 1);
        assert!(!netz::laeuft() && !offen(port), "halber Host: Port offen ohne das Schwere");
        assert_eq!(teil(&d), F::Fehler(Startfehler::Schalter("--encoderweg")));
        assert!(d.vorrat.is_some(), "das Schwere gilt als erledigt");
        // Der Takt oeffnet den Port nicht nachtraeglich (eine Nachricht ohne
        // Wirkung, sonst wartete der Takt ohne Freigabe eine Minute).
        d.naechster_takt = Instant::now();
        d.tx.send(Nachricht::Sprache(d.lang)).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert!(!netz::laeuft(), "Takt oeffnete den Port ohne das Schwere");
        // Aus und wieder an: neuer Versuch, der gelingt.
        d.tx.send(Nachricht::FreigabeAus).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        d.tx.send(Nachricht::FreigabeAn).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!(VERSUCHE.load(Ordering::SeqCst), 2);
        assert!(netz::laeuft() && offen(port), "zweiter Versuch: Port zu");
        assert!(d.vorrat.is_none());
        assert_eq!(teil(&d), F::Bereit);
        // Ein drittes Einschalten startet das Schwere nicht noch einmal.
        d.tx.send(Nachricht::FreigabeAus).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        d.tx.send(Nachricht::FreigabeAn).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!(VERSUCHE.load(Ordering::SeqCst), 2);
        d.tx.send(Nachricht::Beenden).unwrap();
        assert_eq!(d.takt(), Lage::Beendet);
    }

    /// Ein neuer Geraetename (die App meldet ihn) gilt ab der naechsten
    /// Zugangsphase - der Einlass traegt ihn sofort, ohne neuen Dienst.
    #[test]
    fn neuer_geraetename_im_dienst() {
        let _platz = netz::platz_pruefung();
        let _name = zugang::name_test_sperre();
        let vorher = zugang::geraetename_eingestellt();
        let mut d = test_dienst("name", true);
        assert_eq!(d.einlass.name(), "Testhost");
        zugang::geraetename_setzen(Some("Neuer Name".into()));
        d.tx.send(Nachricht::NameGeaendert).unwrap();
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!(d.einlass.name(), "Neuer Name");
        zugang::geraetename_setzen(vorher);
    }

    /// Was das Menue ueber die Host-Rolle zeigt: aus, Fehler, belegt, bereit;
    /// ohne Einlass weder ID noch Passwort noch Geraete.
    #[test]
    fn menue_teil_je_zustand() {
        use symbolmenue::Freigabe as F;
        let _platz = netz::platz_pruefung();
        let d = test_dienst("menueteil", true);
        let e = Some(d.einlass.as_ref());
        assert_eq!(menue_teil(e, false, 9001, true, 5).freigabe, F::Aus);
        assert_eq!(menue_teil(e, true, 9001, true, 5).freigabe, F::Fehler(symbolmenue::Startfehler::Schluessel));
        assert_eq!(menue_teil(e, true, 9001, false, 6).freigabe, F::Fehler(symbolmenue::Startfehler::Schalter("--konserve")));
        assert_eq!(menue_teil(e, true, 9001, false, FEHLER_SONST).freigabe, F::Fehler(symbolmenue::Startfehler::Sonst));
        assert_eq!(menue_teil(e, true, 9001, true, 0).freigabe, F::PortBelegt(9001));
        let h = menue_teil(e, true, 9001, false, 0);
        assert!(matches!(h.freigabe, F::Bereit | F::Verbunden(_)));
        assert_eq!(h.id, Some(d.einlass.id()));
        // Fehlt die Datei, entsteht ein Zufallspasswort - und steht im Menue.
        assert_eq!(h.passwort, d.einlass.passwort().map_err(|_| ()));
        assert!(h.passwort.is_ok());
        assert_eq!(h.geraete, Ok(Vec::new()));
        assert_eq!(menue_teil(None, true, 9001, false, 0).id, None);
    }

    /// W1: der Dienst endet, ohne den Prozess zu beenden. "Freigabe beenden"
    /// (aktion) schliesst die Ports und meldet Beendet; ein anderer Punkt
    /// laesst ihn laufen. Das Ende von aussen kommt als Nachricht aus dem
    /// Symbolfaden, der selbst schon verabschiedet hat - takt meldet dann
    /// Beendet. Danach bleiben takt und aktion bei Beendet, und ein zweites
    /// Beenden tut nichts.
    #[test]
    fn dienst_endet_ohne_prozessende() {
        let _platz = netz::platz_pruefung();
        let mut d = test_dienst("menue", false);
        assert!(netz::laeuft());
        assert_eq!(d.aktion(oberflaeche::Aktion::Zufallspasswort), Lage::Laeuft);
        assert!(d.einlass.passwort().is_ok(), "Zufallspasswort nicht angelegt");
        assert_eq!(d.takt(), Lage::Laeuft);
        assert_eq!(d.aktion(oberflaeche::Aktion::Beenden), Lage::Beendet);
        assert!(!netz::laeuft(), "Ports nach \"Freigabe beenden\" noch offen");
        assert!(std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], d.port).into(), Duration::from_secs(6)).is_err());
        assert_eq!(d.takt(), Lage::Beendet);
        assert_eq!(d.aktion(oberflaeche::Aktion::IdKopieren), Lage::Beendet, "Menuepunkt nach dem Ende ausgefuehrt");
        d.beenden(HOST_ENDE_BEENDET);

        // Von aussen: wie der Rueckruf `ende` im Symbolfaden - erst der
        // Abschied, dann die Nachricht.
        let mut d = test_dienst("aussen", false);
        assert!(netz::laeuft());
        let tx = d.tx.clone();
        let t0 = Instant::now();
        std::thread::spawn(move || {
            abschied(HOST_ENDE_BEENDET);
            let _ = tx.send(Nachricht::VonAussenBeendet);
        });
        let mut lage = Lage::Laeuft;
        while lage == Lage::Laeuft && t0.elapsed() < Duration::from_secs(10) {
            lage = d.takt();
        }
        assert_eq!(lage, Lage::Beendet);
        assert!(t0.elapsed() < Duration::from_secs(3), "Ende von aussen erst nach {:?}", t0.elapsed());
        assert!(!netz::laeuft());
    }
}
