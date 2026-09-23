// QuadChroma Client, erste Stufe: Strom annehmen, decodieren, anzeigen.
//
//   quadchroma <host>[:port]
//
// Das Bild kommt als HEVC 4:4:4 (8 oder 10 Bit, voller Wertebereich) an. Die
// Umwandlung nach RGB laeuft vorerst auf der CPU und ueber mehrere Kerne; das
// reicht fuer 1080p und laeuft auch auf Maschinen ohne brauchbare Grafik.

// Kein Konsolenfenster beim Doppelklick. Wird das Programm aus einer
// Eingabeaufforderung gestartet, haengen wir uns unten an deren Konsole -
// dann funktionieren --headless und --shot weiterhin wie gewohnt.
#![windows_subsystem = "windows"]

// Windows-Bau nur mit statischer C-Laufzeit (client/.cargo/config.toml):
// ohne sie braucht die exe VCRUNTIME140.dll, und die liegt nicht jedem
// Windows bei. Greift die Einstellung nicht (Bau von ausserhalb client\,
// oder RUSTFLAGS ersetzt sie), soll das hier auffallen und nicht erst beim
// Nutzer.
#[cfg(all(windows, target_env = "msvc", not(target_feature = "crt-static")))]
compile_error!("Windows-Bau ohne crt-static: client/.cargo/config.toml greift nicht (aus client\\ bauen, RUSTFLAGS nicht setzen)");

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;

mod discovery;
mod einstellungen;
mod noise;
mod protokoll_konst;
mod secure;
mod sps;
mod strings;
mod strings_asia;
mod strings_balt;
mod strings_east;
mod strings_more;
mod strings_north;
mod strings_west;
mod ui;

/// Ton und Zwischenablage: je Plattform eine Datei mit derselben
/// Schnittstelle; der Ringpuffer des Tons ist geteilt (audio_ring.rs). Auf
/// dem Mac laufen die Module unter denselben Namen `audio` und `clipboard`,
/// damit die Aufrufstellen ohne Weiche auskommen.
#[cfg(any(windows, target_os = "macos"))]
mod audio_ring;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod clipboard;
#[cfg(target_os = "macos")]
mod audio_mac;
#[cfg(target_os = "macos")]
mod clipboard_mac;
#[cfg(target_os = "macos")]
use audio_mac as audio;
#[cfg(target_os = "macos")]
use clipboard_mac as clipboard;
#[cfg(windows)]
mod anzeige;
/// Windows als Host: eigene Rolle in derselben Programmdatei (--host,
/// --list, --messen), ohne Fenster.
#[cfg(windows)]
mod host;
// Die Nachrichtenkennungen der Leitung liegen in EINEM Modul, das Client-
// und Host-Rolle teilen (protokoll_konst.rs).
use protokoll_konst::*;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, CustomCursor, Window, WindowId};

/// Eigene Uhr des Clients. Der Nullpunkt ist beliebig; fuer den Vergleich mit
/// dem Host zaehlt nur der Versatz, den der Zeitabgleich ausrechnet.
fn client_us() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Protokoll des Clients: die Decoderzeilen und FFmpegs eigene Meldungen.
///
/// FFmpeg schreibt seine Meldungen sonst auf sein eigenes stderr - das
/// fuehrt in einer Fensteranwendung nirgends hin, und selbst mit
/// angehaengter Konsole sieht die DLL sie nicht, weil ihre Laufzeit vor dem
/// Anhaengen gestartet ist. Die Klage eines cuvid-Decoders ueber eine Karte,
/// die das Profil nicht kann, waere damit unsichtbar. Hier laufen beide
/// Sorten in EINER Reihe zusammen, in der Reihenfolge, in der sie entstanden
/// sind: erst die Ursache, dann die Folge. Der Pruefmodus druckt sie, und
/// jede Zeile landet ausserdem in protokoll.txt im Ablageordner des Clients
/// (%APPDATA%\QuadChroma, auf dem Mac ~/Library/Application Support/
/// QuadChroma), das bei jedem Start neu beginnt - ein paar Zeilen je
/// Sitzung, mehr nicht.
mod protokoll {
    use ffmpeg_next as ffmpeg;
    use std::io::Write;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Mutex;

    static STUFE: AtomicI32 = AtomicI32::new(ffmpeg::sys::AV_LOG_WARNING);
    /// Zeilen seit dem letzten Abholen. Holt niemand ab (Fenstermodus),
    /// bleibt die Reihe bei 512 stehen - die Datei hat dann alles.
    static ZEILEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Die letzten Warnungen und Fehler von FFmpeg, fuer Rueckfallgruende.
    static FEHLER: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Eine FFmpeg-Meldung kann in Stuecken kommen; der Zeilenumbruch
    /// schliesst sie ab. Der Rest der angefangenen Zeile und FFmpegs
    /// Praefix-Zustand ("naechstes Stueck bekommt einen Absender") liegen
    /// unter EINER Sperre, die schon vor dem Formatieren genommen wird -
    /// FFmpeg ruft den Rueckruf aus jedem Faden, der etwas zu sagen hat,
    /// auch aus den Arbeitsfaeden eines Decoders, und genau so haelt es
    /// FFmpegs eigener Rueckruf.
    static OFFEN: Mutex<(String, std::os::raw::c_int)> = Mutex::new((String::new(), 1));
    /// Die Datei, beim ersten Schreiben geoeffnet und dabei geleert.
    static DATEI: Mutex<Option<std::fs::File>> = Mutex::new(None);

    fn datei_pfad() -> Option<std::path::PathBuf> {
        crate::einstellungen::datei_pfad("protokoll.txt")
    }

    /// Nur in die Datei - fuer die Taktzeilen des Pruefmodus, die auf der
    /// Konsole ohnehin stehen.
    pub fn nur_datei(text: &str) {
        let Ok(mut d) = DATEI.lock() else { return };
        if d.is_none() {
            *d = datei_pfad().and_then(|p| std::fs::File::create(p).ok());
        }
        if let Some(f) = d.as_mut() {
            let _ = writeln!(f, "{text}");
        }
    }

    /// Eine Zeile ins Protokoll: Datei und Reihe.
    pub fn zeile(text: String) {
        nur_datei(&text);
        if let Ok(mut z) = ZEILEN.lock() {
            if z.len() < 512 {
                z.push(text);
            }
        }
    }

    unsafe extern "C" fn rueckruf(
        ptr: *mut std::os::raw::c_void,
        stufe: std::os::raw::c_int,
        fmt: *const std::os::raw::c_char,
        vl: ffmpeg::sys::va_list,
    ) {
        if stufe > STUFE.load(Ordering::Relaxed) {
            return;
        }
        let mut puffer = [0 as std::os::raw::c_char; 1024];
        let text = {
            let Ok(mut offen) = OFFEN.lock() else { return };
            let (rest, praefix) = &mut *offen;
            let n = ffmpeg::sys::av_log_format_line2(
                ptr, stufe, fmt, vl, puffer.as_mut_ptr(), puffer.len() as std::os::raw::c_int, praefix,
            );
            if n <= 0 {
                return;
            }
            // Wie bei snprintf ist n die Soll-Laenge; was nicht in den Puffer
            // passte, ist abgeschnitten. Ob die Meldung abgeschlossen ist,
            // sagt FFmpeg selbst ueber den Praefix-Zustand: er wird aus dem
            // UNGEKUERZTEN Text bestimmt und kennt auch '\r' als Zeilenende.
            // Eine abgeschnittene Meldung gilt ebenfalls als abgeschlossen,
            // sonst klebte die naechste an ihr.
            let abgeschnitten = n as usize >= puffer.len();
            let n = (n as usize).min(puffer.len() - 1);
            rest.push_str(&String::from_utf8_lossy(std::slice::from_raw_parts(puffer.as_ptr() as *const u8, n)));
            if *praefix == 0 && !abgeschnitten {
                return;
            }
            let t = format!("FFmpeg: {}", rest.trim_end());
            rest.clear();
            *praefix = 1;
            t
        };
        if stufe <= ffmpeg::sys::AV_LOG_WARNING {
            if let Ok(mut f) = FEHLER.lock() {
                if f.len() >= 6 {
                    f.remove(0);
                }
                f.push(text.clone());
            }
        }
        zeile(text);
    }

    /// Einsammeln einschalten. Ausfuehrlich heisst bis AV_LOG_VERBOSE - da
    /// sagt cuvid, welche Formate er gewaehlt hat und was die Karte kann.
    pub fn einschalten(ausfuehrlich: bool) {
        let stufe = if ausfuehrlich { ffmpeg::sys::AV_LOG_VERBOSE } else { ffmpeg::sys::AV_LOG_WARNING };
        STUFE.store(stufe, Ordering::Relaxed);
        unsafe {
            ffmpeg::sys::av_log_set_level(stufe);
            ffmpeg::sys::av_log_set_callback(Some(rueckruf));
        }
    }

    /// Alle Zeilen seit dem letzten Abholen.
    pub fn abholen() -> Vec<String> {
        ZEILEN.lock().map(|mut z| std::mem::take(&mut *z)).unwrap_or_default()
    }

    /// Angesammelte Warnungen und Fehler verwerfen - vor einem Versuch, damit
    /// nur das im Grund landet, was dieser Versuch selbst gesagt hat.
    pub fn fehler_verwerfen() {
        if let Ok(mut f) = FEHLER.lock() {
            f.clear();
        }
    }

    /// Die letzten Warnungen und Fehler, ohne den Absender "[hevc_cuvid @
    /// 000001d4...] " - der steht im Grund ohnehin, und die Adresse sagt
    /// niemandem etwas. Gleiche Zeilen in Folge nur einmal.
    pub fn fehler_abholen() -> Vec<String> {
        let Ok(mut f) = FEHLER.lock() else { return Vec::new() };
        let mut aus: Vec<String> = Vec::new();
        for z in f.drain(..) {
            let z = z.trim_start_matches("FFmpeg: ");
            let kern = match (z.starts_with('['), z.find("] ")) {
                (true, Some(i)) => &z[i + 2..],
                _ => z,
            };
            if aus.last().map(|l| l == kern) != Some(true) {
                aus.push(kern.to_string());
            }
        }
        aus
    }
}

/// Urheber. Wird in der Fusszeile des Startbildschirms angezeigt.
const COPYRIGHT: &str = "© 2026 Robert Brandt";
/// Projektseite. Steht anklickbar neben der Urheberzeile.
const WEBSITE: &str = "quadchroma.tech";

/// Die Projektseite im Standardbrowser oeffnen. Wir starten nichts selbst,
/// sondern reichen die Adresse an das System weiter - das ist der einzige Weg,
/// der ohne zweites Programm auskommt.
#[cfg(windows)]
fn website_oeffnen() {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let ziel = HSTRING::from(format!("https://{WEBSITE}"));
    let verb = HSTRING::from("open");
    unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(ziel.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

#[cfg(not(windows))]
fn website_oeffnen() {}
/// So lange muss ESC gehalten werden, bis sich das Menue oeffnet.
const ESC_HALTEDAUER: Duration = Duration::from_secs(2);
/// So lange gilt ein nachgereichter ESC als gedrueckt.
const ESC_TIPPDAUER: Duration = Duration::from_millis(60);

/// Die Form des Mac-Zeigers, wie sie der Host zuletzt geschickt hat. Der
/// Zeiger selbst bleibt der von Windows (Regel des Projekts: kein Zeiger im
/// Video) - er bekommt nur das Aussehen des Macs: Pfeil, Hand, Ziehpfeile am
/// Fensterrand, Textcursor, Wartekugel.
#[derive(Clone)]
struct ZeigerForm {
    w: u16,
    h: u16,
    hx: u16,
    hy: u16,
    sichtbar: bool,
    rgba: Vec<u8>,
}

impl ZeigerForm {
    /// Kennung fuer den Vorrat schon gebauter Zeiger - die Wartekugel dreht
    /// sich durch ein Dutzend Formen, jede soll nur einmal gebaut werden.
    fn kennung(&self) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for b in self.rgba.iter().chain([self.w as u8, self.h as u8, self.hx as u8, self.hy as u8].iter()) {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        h
    }
}

/// So lange gilt ein Codecwunsch als "unterwegs", falls der Host nie
/// antwortet. Danach verschwindet der Hinweis von selbst.
const CODEC_WECHSEL_FRIST: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
struct StreamInfo {
    width: u32,
    height: u32,
    fps: u32,
    /// 1 = HEVC, 2 = H.264 (Byte 6 der Strominfo).
    codec: u8,
    /// Farbaufloesung: true = 4:4:4, false = 4:2:0 (aus Byte 7 der Strominfo).
    chroma444: bool,
    ten_bit: bool,
}

impl StreamInfo {
    /// Aus den acht Bytes der Strominfo. Format-Byte: 1 = 4:4:4 8 Bit,
    /// 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit, 4 = 4:2:0 10 Bit. Immer Vollbereich.
    fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 8 {
            return None;
        }
        Some(StreamInfo {
            width: u16::from_le_bytes([p[0], p[1]]) as u32,
            height: u16::from_le_bytes([p[2], p[3]]) as u32,
            fps: u16::from_le_bytes([p[4], p[5]]) as u32,
            codec: p[6],
            chroma444: matches!(p[7], 1 | 2),
            ten_bit: matches!(p[7], 2 | 4),
        })
    }

    /// Anzeigename des laufenden Codecs, etwa "HEVC 4:4:4 10" oder
    /// "H.264 4:2:0 8". Die einzige Stelle, an der der Name entsteht.
    fn codec_name(&self) -> String {
        let c = match self.codec { 2 => "H.264", 3 => "AV1", _ => "HEVC" };
        format!(
            "{c} {} {}",
            if self.chroma444 { "4:4:4" } else { "4:2:0" },
            if self.ten_bit { 10 } else { 8 }
        )
    }
}

/// Ein Eintrag der Koennensliste des Hosts (Nachricht 8).
#[derive(Clone, Debug, PartialEq)]
pub struct CodecEintrag {
    /// Index des Kandidaten auf dem Host; genau der wird zurueckgewuenscht.
    idx: u8,
    /// Kann dieser Mac das ueberhaupt codieren?
    available: bool,
    /// Laeuft es in der Video-Einheit statt auf der CPU? Wird mitgefuehrt,
    /// angezeigt aber noch nicht - auf dem Mac mini ist ohnehin alles Hardware.
    #[allow(dead_code)]
    hardware: bool,
    /// Braucht die Aufnahme einen Umrechnungsschritt (etwa 4:4:4 8 Bit)?
    conversion: bool,
    chroma444: bool,
    ten_bit: bool,
    name: String,
}

impl CodecEintrag {
    /// Welcher Codec hinter dem Eintrag steckt: 1 HEVC, 2 H.264, 3 AV1.
    /// Die Liste traegt kein eigenes Codec-Byte, also entscheidet der
    /// Name - die Namen kommen aus einer festen Tabelle auf dem Host.
    fn codec(&self) -> u8 {
        if self.name.starts_with("H.264") {
            2
        } else if self.name.starts_with("AV1") {
            3
        } else {
            1
        }
    }

    /// Passt der Eintrag zu dem, was gerade laeuft?
    fn passt_zu(&self, i: &StreamInfo) -> bool {
        self.codec() == i.codec && self.chroma444 == i.chroma444 && self.ten_bit == i.ten_bit
    }
}

/// Koennensliste aus den Nutzdaten lesen. Jede Laenge wird geprueft, denn die
/// Bytes kommen aus dem Netz: ein zu kurzer oder krummer Eintrag beendet die
/// Liste, statt das Programm zu beenden.
fn codecs_parsen(p: &[u8]) -> Vec<CodecEintrag> {
    let mut out = Vec::new();
    let Some(&anzahl) = p.first() else { return out };
    let mut o = 1usize;
    for _ in 0..anzahl {
        if o + 7 > p.len() {
            break;
        }
        let namelen = p[o + 6] as usize;
        let name_start = o + 7;
        let name_ende = name_start + namelen;
        if name_ende > p.len() {
            break;
        }
        let name = String::from_utf8_lossy(&p[name_start..name_ende]).into_owned();
        let mut e = CodecEintrag {
            idx: p[o],
            available: p[o + 1] != 0,
            hardware: p[o + 2] != 0,
            conversion: p[o + 3] != 0,
            chroma444: p[o + 4] != 0,
            ten_bit: p[o + 5] != 0,
            name,
        };
        // AV1 kann dieser Client nicht empfangen: Strominfo und Wechsel
        // (Nachricht 1 und 7) kennen nur HEVC und H.264, ein AV1-Strom kaeme
        // als HEVC beim Decoder an und bliebe schwarz. Bietet ein Host ihn
        // trotzdem an (ein aelterer Windows-Host mit AV1-faehiger NVENC),
        // gilt er hier als nicht verfuegbar - nicht waehlbar, nicht im
        // Benchmark.
        if e.codec() == 3 {
            e.available = false;
        }
        out.push(e);
        o = name_ende;
    }
    out
}

/// Nachricht 7: ab hier ein anderer Codec.
#[derive(Clone, Copy, Debug)]
struct CodecWechsel {
    idx: u8,
    is_h264: bool,
    chroma444: bool,
    ten_bit: bool,
    #[allow(dead_code)]
    full_range: bool,
    #[allow(dead_code)]
    conversion: bool,
}

impl CodecWechsel {
    fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 8 {
            return None;
        }
        Some(CodecWechsel {
            idx: p[0],
            is_h264: p[1] != 0,
            chroma444: p[2] != 0,
            ten_bit: p[3] != 0,
            full_range: p[4] != 0,
            conversion: p[5] != 0,
        })
    }
}

/// Auslastung des Hosts. Was fehlt, fehlt mit Absicht: die Video-Einheit
/// meldet ihre Auslastung nirgends, deshalb steht dort die Encoderzeit je
/// Bild statt einer erfundenen Prozentzahl.
#[derive(Clone, Copy, Default, Debug)]
struct HostLast {
    cpu: f32,
    cpu_eigen: f32,
    gpu: Option<f32>,
    druck: u16,
    ram_benutzt_mb: u32,
    ram_gesamt_mb: u32,
    eigen_mb: u32,
    encoder_ms: f32,
    host_fps: f32,
}

/// Zerlegte Verzoegerung vom Bildschirm des Hosts bis ins Fenster des Clients.
/// Alle Werte in Millisekunden, jeweils der Mittelwert der letzten Bilder.
#[derive(Clone, Copy, Default, Debug)]
struct Latenz {
    /// Versatz der beiden Uhren, Host minus Client, in Mikrosekunden.
    versatz_us: i64,
    /// Umlaufzeit der Messung. Sie ist die Fehlergrenze des Versatzes.
    umlauf_ms: f32,
    /// Aufnahme bis Encoder fertig.
    encoder_ms: f32,
    /// Encoder fertig bis vollstaendig empfangen.
    leitung_ms: f32,
    /// Decodieren und Umrechnen nach RGB.
    decoder_ms: f32,
    /// Summe: Aufnahme bis anzeigebereit (ohne das Glied Anzeige).
    gesamt_ms: f32,
    /// Wie viele Bilder in den Mittelwert eingegangen sind.
    bilder: u32,
    /// Anzeigebereit bis Uebergabe an GDI bzw. DXGI, gemessen vom
    /// Fensterfaden auf der Client-Uhr. Kein Teil des Zeitabgleichs, deshalb
    /// erst beim Lesen aus `Shared` eingetragen (siehe `Shared::latenz`).
    anzeige_ms: f32,
}

impl Latenz {
    /// Aufnahme bis Uebergabe ans Fenster: alle fuenf Glieder.
    fn bis_anzeige(&self) -> f32 {
        self.gesamt_ms + self.anzeige_ms
    }
}

/// Fertiges Bild in RGB, bereit zum Anzeigen.
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u32>, // 0x00RRGGBB, wie softbuffer es erwartet
    /// Client-Uhr bei der Ablage durch den Empfangsfaden: Anfang der
    /// Anzeigezeit.
    bereit_us: u64,
}

/// Was der Empfangsfaden ablegt: fertig gerechnetes RGB, oder - sobald die
/// Karte die Umrechnung uebernimmt - das rohe Decoderbild mit seinen Ebenen.
/// Ein rohes Bild kostet im Empfangsfaden keine Kopie: `ffmpeg::frame::Video`
/// laesst sich zwischen Faeden verschieben.
enum Bild {
    Rgb(Frame),
    Roh { bild: ffmpeg::frame::Video, bereit_us: u64 },
}

impl Bild {
    fn bereit_us(&self) -> u64 {
        match self {
            Bild::Rgb(f) => f.bereit_us,
            Bild::Roh { bereit_us, .. } => *bereit_us,
        }
    }
}

/// Eine Meldung fuer die Oberflaeche. Der Satz kommt aus den Sprachtabellen
/// (Schluessel; die Platzhalter {n}, {m} und {p} werden eingesetzt),
/// dahinter darf in Klammern ein technischer Anhang roh stehen - der
/// Wortlaut von FFmpeg oder vom System, den niemand uebersetzen kann.
/// `protokoll` ist derselbe Fall auf Deutsch mit allen Einzelheiten, fuer
/// protokoll.txt und den Pruefmodus.
#[derive(Clone, Debug, PartialEq)]
struct Meldung {
    key: strings::Key,
    werte: Vec<(&'static str, String)>,
    anhang: Option<String>,
    protokoll: String,
}

impl Meldung {
    fn neu(key: strings::Key, protokoll: impl Into<String>) -> Meldung {
        Meldung { key, werte: Vec::new(), anhang: None, protokoll: protokoll.into() }
    }

    /// Wert fuer einen Platzhalter ("{n}", "{m}", "{p}").
    fn mit(mut self, platzhalter: &'static str, wert: impl Into<String>) -> Meldung {
        self.werte.push((platzhalter, wert.into()));
        self
    }

    fn anhang(mut self, anhang: impl Into<String>) -> Meldung {
        self.anhang = Some(anhang.into());
        self
    }

    /// Ein Fehler, der sich mit dem naechsten Versuch nicht von selbst gibt:
    /// Pin geaendert, Ablage oder Schluesseldatei kaputt, Liste nicht
    /// les- oder schreibbar. Dann verbindet der Empfangsfaden nicht alle
    /// 2 s neu, sondern nimmt das Ziel zurueck (wie bei einer Abloesung);
    /// der Nutzer verbindet nach dem Beheben selbst wieder.
    fn dauerhaft(&self) -> bool {
        use strings::Key::*;
        matches!(self.key, HostKeyChanged | FileUnreadable | FileNotUtf8 | FileNotWritable | KeyFileDamaged | StorageUnavailable)
    }

    /// Der Text in dieser Sprache: Platzhalter ersetzt, Anhang in Klammern.
    fn text(&self, lang: &strings::Lang) -> String {
        let mut t = lang.get(self.key).to_string();
        for (p, w) in &self.werte {
            t = t.replace(p, w);
        }
        match &self.anhang {
            Some(a) => format!("{t} ({a})"),
            None => t,
        }
    }
}

/// Fehler von Leitung und Ablage in Worten der Oberflaeche. Der Wortlaut des
/// Systems bleibt als Anhang, wo er etwas sagt; alles andere steht mit
/// Einzelheiten im Protokoll.
impl From<secure::Fehler> for Meldung {
    fn from(f: secure::Fehler) -> Meldung {
        use secure::Fehler as F;
        use std::io::ErrorKind;
        use strings::Key::*;
        let protokoll = f.to_string();
        match f {
            F::Ablage(_) => Meldung::neu(StorageUnavailable, protokoll),
            F::SchluesselBeschaedigt { pfad, .. } => {
                Meldung::neu(KeyFileDamaged, protokoll).mit("{p}", pfad.display().to_string())
            }
            F::Unlesbar { pfad, grund } => {
                Meldung::neu(FileUnreadable, protokoll).mit("{p}", pfad.display().to_string()).anhang(grund)
            }
            F::KeinUtf8 { pfad } => Meldung::neu(FileNotUtf8, protokoll).mit("{p}", pfad.display().to_string()),
            F::Schreiben { pfad, grund } => {
                Meldung::neu(FileNotWritable, protokoll).mit("{p}", pfad.display().to_string()).anhang(grund)
            }
            F::FingerabdruckGeaendert { host, fingerabdruck, pfad } => Meldung::neu(HostKeyChanged, protokoll)
                .mit("{n}", host)
                .mit("{m}", fingerabdruck)
                .mit("{p}", pfad.display().to_string()),
            F::Adresse { addr, grund } => {
                let m = Meldung::neu(ErrorAddress, protokoll).mit("{n}", addr);
                match grund {
                    Some(g) => m.anhang(g),
                    None => m,
                }
            }
            F::Verbindung { art: ErrorKind::ConnectionRefused, .. } => Meldung::neu(ErrorConnectRefused, protokoll),
            F::Verbindung { art: ErrorKind::TimedOut | ErrorKind::WouldBlock, .. } => Meldung::neu(ErrorTimeout, protokoll),
            F::Verbindung { grund, .. } => Meldung::neu(ErrorNoConnection, protokoll).anhang(grund),
            F::Handschlag { frist: true, .. } => Meldung::neu(ErrorTimeout, protokoll),
            // Der Satz kommt aus der Tabelle; dahinter nur der Wortlaut des
            // Systems, wenn die Leitung selbst scheiterte - nicht der
            // deutsche Grund (der steht im Protokoll).
            F::Handschlag { system: Some(s), .. } => Meldung::neu(ErrorHandshake, protokoll).anhang(s),
            F::Handschlag { .. } => Meldung::neu(ErrorHandshake, protokoll),
        }
    }
}

/// Kein Decoder zu bauen: der Grund (deutsch, mit FFmpegs Worten) geht ins
/// Protokoll, die Oberflaeche sagt es mit ihrem Schluessel.
fn kein_decoder(grund: String) -> Meldung {
    Meldung::neu(strings::Key::ErrorNoDecoder, grund)
}

#[derive(Default)]
struct Shared {
    /// Griff an der Bildleitung, um sie beim Trennen von aussen zu kappen.
    abbruch: Option<std::net::TcpStream>,
    /// Adresse, mit der sich der Empfangsfaden verbinden soll. None = warten.
    target: Option<String>,
    connected: bool,
    frame: Option<Bild>,
    /// Zeichnet die Karte? Dann legt der Empfangsfaden rohe Bilder ab,
    /// statt sie auf der CPU umzurechnen. Setzt der Fensterfaden nach dem
    /// Aufbau der Swapchain; faellt die Karte weg, nimmt er es zurueck.
    gpu_pfad: bool,
    /// Anzeigezeit (Ablage bis hinter present), gleitender Mittelwert des
    /// Fensterfadens.
    anzeige_ms: f32,
    /// Praesentationen, die ausgelassen wurden, weil DXGI noch nicht bereit
    /// war. Auf dem CPU-Weg gibt es das nicht: bleibt null.
    ausgelassen: u64,
    /// Zuletzt empfangene Zeigerform und eine laufende Nummer dazu, damit der
    /// Fensterfaden sieht, ob er sie schon uebernommen hat.
    zeiger: Option<ZeigerForm>,
    zeiger_seq: u64,
    info: Option<StreamInfo>,
    decoded: u64,
    dropped: u64,
    /// Nutzlast aller Bildnachrichten seit dem Start, in Byte. Daraus
    /// rechnet der Benchmark die Datenrate, die wirklich ankommt.
    bytes_video: u64,
    last_decode_ms: f32,
    error: Option<Meldung>,
    /// Pruefsumme des Bildkanals und Schluessel seines Hosts, immer als Paar.
    /// Der Eingabekanal braucht die Pruefsumme, sonst laesst ihn der Host
    /// nicht herein - und den Schluessel, um zu pruefen, dass am anderen
    /// Ende wirklich derselbe Host sitzt.
    link: Option<(Vec<u8>, Vec<u8>)>,
    /// Vergleichscode und Fingerabdruck der Gegenstelle, zum Anzeigen.
    sas: Option<String>,
    peer_fp: Option<String>,
    first_time: bool,
    /// Fehler, fuer den es einen uebersetzten Text gibt. Hat Vorrang vor `error`.
    error_key: Option<strings::Key>,
    /// Was auf dem Host gerade gilt: Mbit/s, Bilder je Sekunde, Gaming-Schalter.
    /// Was beim Host gilt: Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton.
    settings: Option<(u32, u16, bool, bool, bool)>,
    /// Ton gewuenscht? Der Client haelt sich selbst daran - auch gegenueber
    /// einem Host, der den Schalter noch nicht kennt und weiter Ton schickt.
    ton: bool,
    /// Auslastung des Hosts, einmal je Sekunde.
    hostlast: Option<HostLast>,
    /// Zeitabgleich mit dem Host und die daraus gewonnene Latenzzerlegung.
    /// Der Versatz gilt nur so genau wie die halbe Umlaufzeit - deshalb steht
    /// die auch mit in der Anzeige.
    clock: Option<Latenz>,
    /// Koennensliste des Hosts, so wie sie nach dem Gruss ankam.
    codecs: Vec<CodecEintrag>,
    /// Index des Kandidaten, den der Host zuletzt per Nachricht 7 gemeldet
    /// hat. Vor dem ersten Wechsel unbekannt - dann entscheidet der Abgleich
    /// mit der Strominfo, welcher Eintrag gerade laeuft.
    codec_idx: Option<u8>,
    /// Seit wann ein Codecwunsch unterwegs ist. Gesetzt beim Klick, geloescht
    /// mit dem ersten Bild aus dem neuen Decoder. Solange steht das Bild
    /// still, und der Hinweis erklaert, warum.
    codec_wechsel: Option<Instant>,
    /// Gewuenschter Decoderpfad (Menue, Datei oder --decoder).
    decoder_wunsch: einstellungen::DecoderWunsch,
    /// Der Wunsch hat sich geaendert: der Empfangsfaden baut den Decoder
    /// beim naechsten Durchlauf neu, ohne die Verbindung zu trennen.
    decoder_wunsch_neu: bool,
    /// Welchen Weg der laufende Decoder wirklich nimmt. None = noch keiner.
    decoder_pfad: Option<DecoderPfad>,
    /// Warum die Karte nicht decodiert, obwohl sie gewuenscht war - fuer
    /// Anzeige und Protokoll. None, wenn sie laeuft oder gar nicht
    /// gewuenscht war.
    decoder_hinweis: Option<String>,
    /// DXGI-Index des Adapters, auf dem die Anzeige laeuft (Kandidat fuer
    /// D3D11VA bei Automatik). None: Software, WARP oder ohne Fenster. Der
    /// Fensterfaden setzt ihn beim Aufbau der Karte, vor der Verbindung.
    anzeige_adapter: Option<u32>,
}

impl Shared {
    /// Laeuft gerade ein Codecwechsel, den man dem Nutzer erklaeren sollte?
    fn wechsel_laeuft(&self) -> bool {
        self.codec_wechsel.map(|t| t.elapsed() < CODEC_WECHSEL_FRIST).unwrap_or(false)
    }

    /// Die Latenzzerlegung samt Anzeige-Glied. Der Empfangsfaden schreibt
    /// `clock` als Ganzes; die Anzeigezeit misst der Fensterfaden und haelt
    /// sie in einem eigenen Feld, damit die beiden sich nicht ueberschreiben.
    fn latenz(&self) -> Option<Latenz> {
        self.clock.map(|mut l| {
            l.anzeige_ms = self.anzeige_ms;
            l
        })
    }
}

/// Kernel- plus Nutzerzeit dieses Prozesses in 100-ns-Einheiten - die
/// Zaehler, aus denen auch der Task-Manager seine Prozentzahl rechnet.
#[cfg(windows)]
fn prozesszeit_100ns() -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut erstellt = FILETIME::default();
    let mut beendet = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut nutzer = FILETIME::default();
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut erstellt, &mut beendet, &mut kernel, &mut nutzer) }.ok()?;
    let als_u64 = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    Some(als_u64(kernel) + als_u64(nutzer))
}

/// Dasselbe auf dem Mac ueber getrusage(RUSAGE_SELF): Nutzer- plus
/// Systemzeit in Mikrosekunden, hier auf 100 ns gebracht. Die Strukturen
/// sind von Hand deklariert (sys/resource.h, arm64 und x86_64 gleich):
/// timeval ist time_t (64 Bit) plus suseconds_t (32 Bit, aufgefuellt).
#[cfg(target_os = "macos")]
fn prozesszeit_100ns() -> Option<u64> {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Timeval {
        sek: i64,
        usek: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rusage {
        nutzer: Timeval,
        system: Timeval,
        rest: [i64; 14],
    }
    extern "C" {
        fn getrusage(wer: std::os::raw::c_int, heraus: *mut Rusage) -> std::os::raw::c_int;
    }
    const RUSAGE_SELF: std::os::raw::c_int = 0;
    let mut r = Rusage::default();
    if unsafe { getrusage(RUSAGE_SELF, &mut r) } != 0 {
        return None;
    }
    let als_100ns = |t: Timeval| (t.sek.max(0) as u64) * 10_000_000 + (t.usek.max(0) as u64) * 10;
    Some(als_100ns(r.nutzer) + als_100ns(r.system))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn prozesszeit_100ns() -> Option<u64> {
    None
}

/// Gehoert diese Verbindungsmeldung ins Protokoll? Nur, wenn sie neu ist
/// oder seit der letzten gleichen ein Bild ankam (`decoded` zaehlt nur
/// hoch) - dann war dazwischen eine echte Sitzung.
fn neu_zu_melden(gemeldet: &mut Option<(Meldung, u64)>, e: &Meldung, decoded: u64) -> bool {
    if matches!(gemeldet, Some((m, n)) if m == e && *n == decoded) {
        return false;
    }
    *gemeldet = Some((e.clone(), decoded));
    true
}

/// Netz- und Decodierschleife. Laeuft in einem eigenen Faden und legt immer nur
/// das neueste Bild ab: lieber eines auslassen als Verzoegerung aufbauen.
fn stream_thread(shared: Arc<Mutex<Shared>>, input: Arc<Mutex<InputLink>>) {
    if let Err(e) = ffmpeg::init() {
        let m = Meldung::neu(strings::Key::ErrorFfmpegStart, format!("FFmpeg-Start fehlgeschlagen: {e}")).anhang(e.to_string());
        protokoll::zeile(m.protokoll.clone());
        shared.lock().unwrap().error = Some(m);
        return;
    }
    // Im Pruefmodus alles einsammeln, was FFmpeg zu sagen hat; im Fenster
    // nur Warnungen und Fehler - fuer die Datei und fuer den Grund, wenn
    // ein Hardware-Decoder schon beim Oeffnen scheitert.
    protokoll::einschalten(std::env::args().any(|a| a == "--headless"));

    // Zuletzt protokollierte Verbindungsmeldung und der Bildzaehler dazu:
    // dieselbe Meldung ohne ein einziges Bild dazwischen steht nur einmal im
    // Protokoll. `error` taugt dafuer nicht - run_session loescht es schon
    // nach dem Handschlag, also vor "nicht gekoppelt" und allem danach.
    let mut gemeldet: Option<(Meldung, u64)> = None;
    loop {
        let addr = { shared.lock().unwrap().target.clone() };
        let Some(addr) = addr else {
            // Ohne Ziel beginnt die Entdoppelung von vorn: verbindet der
            // Nutzer neu, steht der erste Fehler wieder im Protokoll.
            gemeldet = None;
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let ergebnis = run_session(&addr, &shared, &input);
        // Ohne Sitzung liest der Client die Zwischenablage nicht mehr.
        #[cfg(any(windows, target_os = "macos"))]
        clipboard::sitzung(false);
        match ergebnis {
            Ok(()) => {}
            Err(e) => {
                let mut s = shared.lock().unwrap();
                // Eine gewollte Trennung kappt die Leitung - der Lesefehler
                // danach ist kein Fehler und wird nicht angezeigt.
                if s.target.is_some() {
                    if neu_zu_melden(&mut gemeldet, &e, s.decoded) {
                        protokoll::zeile(format!("Verbindung: {}", e.protokoll));
                    }
                    // Was sich mit dem naechsten Versuch nicht gibt, wird
                    // nicht alle 2 s wiederholt: das Ziel geht zurueck, die
                    // Meldung bleibt stehen (siehe Meldung::dauerhaft).
                    if e.dauerhaft() {
                        protokoll::zeile(format!("Verbindung zu {addr} beendet: der Fehler bleibt bis zur Behebung - kein neuer Versuch"));
                        s.target = None;
                    }
                    s.error = Some(e);
                }
                s.connected = false;
            }
        }
        {
            let mut s = shared.lock().unwrap();
            s.abbruch = None;
            s.link = None;
            // Die Form geht, die Nummer zaehlt weiter: fiele sie auf null,
            // ueberspraenge der Fensterfaden nach dem Wiederverbinden genau die
            // Form mit der alten Nummer.
            s.zeiger = None;
            s.connected = false;
            s.codec_wechsel = None;
            s.codec_idx = None;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

use ffmpeg_next as ffmpeg;

/// Rolle einer Karte im Menue: die dedizierten Karten in der Reihenfolge
/// der Aufzaehlung (1 = "Grafikkarte", 2 = "Grafikkarte 2"), oder die
/// integrierte mit gemeinsamem Speicher. Erkannt in
/// `anzeige::karten_erkennen`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rolle {
    Grafikkarte(u8),
    Integriert,
}

impl Rolle {
    /// Fuer Protokoll und Statistik (die Knoepfe im Menue kommen aus den
    /// Sprachtabellen).
    pub fn name(self) -> String {
        match self {
            Rolle::Grafikkarte(1) => "Grafikkarte".into(),
            Rolle::Grafikkarte(n) => format!("Grafikkarte {n}"),
            Rolle::Integriert => "Integriert".into(),
        }
    }
}

/// Eine erkannte Karte: was DXGI ueber sie sagt, und ihre Rolle.
#[derive(Clone, Debug)]
pub struct Karte {
    /// Index in der Aufzaehlung von DXGI - derselbe wie bei --adapter n und
    /// als "device" fuer FFmpegs D3D11VA.
    pub index: u32,
    pub name: String,
    pub vendor: u32,
    pub speicher_mb: u64,
    pub hat_ausgang: bool,
    pub luid: i64,
    pub rolle: Rolle,
}

impl Karte {
    pub fn nvidia(&self) -> bool {
        self.vendor == 0x10de
    }
}

/// Die Karten des Rechners, einmal beim Start erkannt und danach fuer
/// beide Faeden gleich: der Fensterfaden baut daraus die Knoepfe, der
/// Empfangsfaden die Decoder.
static KARTEN: std::sync::OnceLock<Vec<Karte>> = std::sync::OnceLock::new();

fn karten() -> &'static [Karte] {
    KARTEN.get_or_init(|| {
        #[cfg(windows)]
        {
            anzeige::karten_erkennen()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    })
}

/// Die Karte zu einer Rolle, falls es sie gibt.
fn karte_mit(karten: &[Karte], rolle: Rolle) -> Option<&Karte> {
    karten.iter().find(|k| k.rolle == rolle)
}

/// Die Karte, die die Anzeige bei Automatik nimmt - dieselbe Regel wie in
/// `anzeige::Gpu::neu`: die erste mit Bildschirmausgang, sonst die erste
/// von NVIDIA, sonst die erste ueberhaupt.
fn karte_automatik(karten: &[Karte]) -> Option<&Karte> {
    karten.iter().find(|k| k.hat_ausgang).or_else(|| karten.iter().find(|k| k.nvidia())).or_else(|| karten.first())
}

/// Welchen Weg der Decoder tatsaechlich nimmt. Das ist das Ergebnis der
/// Wahl, nicht der Wunsch: bei Automatik kann alles herauskommen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecoderPfad {
    /// NVIDIA-Karte ueber die cuvid-Decoder von FFmpeg (hevc_cuvid, h264_cuvid),
    /// mit der LUID der Karte, auf der er laeuft - None, wenn sich das nicht
    /// sagen laesst. Damit unterscheidet `wunsch_passt` zwei NVIDIA-Karten.
    Nvdec(Option<i64>),
    /// Direct3D 11 Video (D3D11VA) auf der Karte dieser Rolle - fuer AMD und
    /// Intel, nur 4:2:0 und H.264. Die Bilder kommen von der Karte in den
    /// Hauptspeicher (Kopierstufe).
    D3d11va(Rolle),
    /// Der eingebaute Software-Decoder von FFmpeg auf der CPU.
    Software,
}

impl DecoderPfad {
    pub fn name(self) -> String {
        match self {
            DecoderPfad::Nvdec(_) => "NVDEC".into(),
            DecoderPfad::D3d11va(r) => format!("D3D11VA ({})", r.name()),
            DecoderPfad::Software => "Software".into(),
        }
    }

    pub fn hardware(self) -> bool {
        self != DecoderPfad::Software
    }
}

/// Ergebnis von `decoder_bauen`: der Decoder und alles, was man ueber ihn
/// wissen will, um es anzuzeigen und ins Protokoll zu schreiben.
struct DecoderBau {
    decoder: ffmpeg::decoder::Video,
    pfad: DecoderPfad,
    /// FFmpeg-Name des Decoders, etwa "hevc_cuvid" oder "hevc".
    codec: &'static str,
    /// Warum es nicht die Karte wurde, obwohl sie gewuenscht war. None, wenn
    /// sie laeuft oder Software ausdruecklich gewuenscht war.
    grund: Option<String>,
    /// Wofuer gebaut wurde: 4:4:4 (Some(true)), 4:2:0 (Some(false)) oder
    /// noch unbekannt (vor der ersten Strominfo). D3D11VA kann kein 4:4:4 -
    /// kommt die Strominfo mit einem anderen Wert, muss neu gebaut werden.
    chroma444: Option<bool>,
    /// Software nur, weil der Strom 4:4:4 ist und D3D11VA das nicht kann.
    /// Wird der Strom 4:2:0, lohnt ein neuer Bau.
    wegen_444: bool,
    /// Wie viele Pakete dieser Decoder schon bekommen hat. Steht im Grund,
    /// wenn er scheitert: "an Paket 1" heisst, er konnte den Strom nie lesen.
    pakete: u32,
    /// Hat er schon ein Bild geliefert? Ein Fehler davor heisst: die Karte
    /// kann das Profil nicht, oder der Treiber streikt - der Decoder taugt
    /// nicht. Ein Fehler danach ist erst einmal nur ein Aussetzer.
    hat_bild: bool,
    /// Fehler in Folge, ohne ein gutes Paket dazwischen. Einer ist ein
    /// Aussetzer (etwa cuvids Bildwarteschlange gerade voll), drei ein Defekt.
    fehler_folge: u8,
    /// Seit wann er ohne Bild ist: erst der Zeitpunkt des ersten Pakets,
    /// spaeter der des letzten Bildes. Ein Hardware-Decoder, der Pakete
    /// annimmt, ohne ein Bild zu liefern, meldet keinen Fehler - er
    /// schweigt. Nach einer Frist gilt das Schweigen als Defekt, ob er nun
    /// nie ein Bild geliefert hat oder mittendrin verstummt ist.
    ohne_bild_seit: Option<Instant>,
    /// Pakete seit dem letzten Bild (oder seit dem Bau).
    pakete_seit_bild: u32,
    /// Bilder in Folge, deren Format to_rgb nicht kennt. Ein Hardware-
    /// Decoder, der nur Unlesbares liefert, taugt so wenig wie einer, der
    /// schweigt - nur sieht man bei ihm ein dunkles Bild statt keines.
    format_fehler: u8,
    /// Wurde sein erstes Bild schon im Protokoll beschrieben?
    bild_gemeldet: bool,
}

/// So lange darf ein Hardware-Decoder Pakete schlucken, ohne ein Bild zu
/// liefern - gerechnet ab dem ersten Paket oder dem letzten Bild. Nach dem
/// Bau ist das erste Paket ein Schluesselbild, cuvid braucht danach
/// hoechstens ein weiteres, um es herauszugeben; wer nach dieser Frist noch
/// nichts hat, gibt auch nichts mehr.
const DECODER_STUMM_FRIST: Duration = Duration::from_millis(1500);
/// ... und mindestens so viele Pakete ohne Bild, damit ein stehendes Bild
/// bei niedriger Bildrate nicht als Schweigen durchgeht.
const DECODER_STUMM_PAKETE: u32 = 30;
/// So viele unlesbare Bilder in Folge, und der Hardware-Decoder wird ersetzt.
const DECODER_FORMAT_FEHLER: u8 = 3;

impl DecoderBau {
    /// Frisch gebaut: noch kein Paket gesehen, kein Bild, kein Fehler.
    fn neu(decoder: ffmpeg::decoder::Video, pfad: DecoderPfad, codec: &'static str, grund: Option<String>) -> Self {
        DecoderBau {
            decoder,
            pfad,
            codec,
            grund,
            chroma444: None,
            wegen_444: false,
            pakete: 0,
            hat_bild: false,
            fehler_folge: 0,
            ohne_bild_seit: None,
            pakete_seit_bild: 0,
            format_fehler: 0,
            bild_gemeldet: false,
        }
    }

    /// Ein Paket geht hinein.
    fn paket(&mut self) {
        self.pakete = self.pakete.wrapping_add(1);
        self.pakete_seit_bild = self.pakete_seit_bild.saturating_add(1);
        self.ohne_bild_seit.get_or_insert_with(Instant::now);
    }

    /// Ein Bild kam heraus.
    fn bild(&mut self) {
        self.hat_bild = true;
        self.pakete_seit_bild = 0;
        self.ohne_bild_seit = Some(Instant::now());
    }

    /// Schluckt dieser Hardware-Decoder Pakete, ohne Bilder zu liefern?
    fn stumm(&self) -> bool {
        self.pfad.hardware()
            && self.pakete_seit_bild >= DECODER_STUMM_PAKETE
            && self.ohne_bild_seit.map(|t| t.elapsed() >= DECODER_STUMM_FRIST).unwrap_or(false)
    }

    /// Wie lange er schon ohne Bild ist.
    fn stumm_seit(&self) -> Duration {
        self.ohne_bild_seit.map(|t| t.elapsed()).unwrap_or_default()
    }

    /// Eine Zeile fuer das Protokoll. Bei einem Rueckfall steht der Grund
    /// voran, so wie er auch in der Statistik steht: "D3D11VA (Integriert)
    /// scheitert: ... - Software (hevc)".
    fn meldung(&self) -> String {
        match &self.grund {
            None => format!("Decoder: {} ({})", self.pfad.name(), self.codec),
            Some(g) => format!("Decoder: {g} - {} ({})", self.pfad.name(), self.codec),
        }
    }
}

/// Software-Decoder fuer HEVC oder H.264, mit der Fadenkonfiguration der
/// Sitzung.
///
/// Auf Durchsatz trimmen: mehrere Bilder gleichzeitig decodieren. Ohne das
/// laeuft alles auf einem Kern und kostet rund 8 ms je Bild.
/// Bildparallelitaet ist schnell, aber sie haelt Bilder zurueck: der
/// Decoder gibt erst heraus, wenn genug Faeden gefuellt sind. Bei 16 Faeden
/// sind das rund 15 Bilder - bei 100 Bildern je Sekunde ueber 140 ms
/// Verzoegerung, die niemand sieht, weil die reine Rechenzeit klein bleibt.
/// Fuer eine Fernsteuerung ist das der falsche Handel, deshalb ist
/// Scheibenparallelitaet die Voreinstellung.
fn software_decoder(h264: bool) -> Result<ffmpeg::decoder::Video, String> {
    let id = if h264 { ffmpeg::codec::Id::H264 } else { ffmpeg::codec::Id::HEVC };
    let name = if h264 { "H.264" } else { "HEVC" };
    let codec = ffmpeg::decoder::find(id).ok_or_else(|| format!("kein {name}-Decoder"))?;
    let mut ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let art = match std::env::args().position(|a| a == "--faeden").and_then(|i| std::env::args().nth(i + 1)) {
        Some(v) if v == "bild" => ffmpeg::threading::Type::Frame,
        _ => ffmpeg::threading::Type::Slice,
    };
    ctx.set_threading(ffmpeg::threading::Config { kind: art, count: threads });
    ctx.decoder().video().map_err(|e| format!("Decoder ({name}): {e}"))
}

/// NVDEC-Decoder ueber cuvid. Ohne NVIDIA-Karte (oder ohne deren Treiber)
/// scheitert schon das Oeffnen: cuvid legt dabei sein CUDA-Geraet an. Auf
/// einer Karte, die das Profil nicht kann (etwa 4:4:4 auf einer alten),
/// geht das Oeffnen durch, und erst das erste Paket mit den Parametersaetzen
/// meldet den Fehler - den faengt die Empfangsschleife.
///
/// LOW_DELAY: cuvid haelt sonst bis zu vier Bilder in seiner Anzeigewarte-
/// schlange zurueck. Fuer eine Fernsteuerung ist jedes davon verlorene Zeit.
///
/// `gpu`: CUDA-Ordnungszahl der Karte (Option "gpu" von cuvid, siehe
/// `nvdec_ziel`). Ohne sie nimmt cuvid CUDA-Geraet 0 - bei zwei NVIDIA-Karten
/// also nicht unbedingt die gewaehlte.
fn nvdec_decoder(h264: bool, gpu: Option<i32>) -> Result<ffmpeg::decoder::Video, String> {
    let name = if h264 { "h264_cuvid" } else { "hevc_cuvid" };
    let codec = ffmpeg::decoder::find_by_name(name)
        .ok_or_else(|| format!("{name} fehlt in dieser FFmpeg-Fassung"))?;
    let mut ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
    ctx.set_flags(ffmpeg::codec::Flags::LOW_DELAY);
    let mut dec = ctx.decoder();
    // cuvid rechnet die Zeitstempel ueber die Paketzeitbasis um und warnt
    // bei jedem Oeffnen, wenn keine gesetzt ist - das waere die erste Zeile
    // in jedem Protokoll. Die pts sind hier Bildnummern; jede Basis, die
    // ganzzahlig hin und zurueck geht, ist recht.
    dec.set_packet_time_base(ffmpeg::Rational(1, 1_000_000));
    // FFmpegs eigene Worte zum Scheitern gehoeren in den Grund: "Operation
    // not permitted" sagt nichts, "Cannot load nvcuvid.dll" alles. Die
    // erste Zeile ersetzt den Fehlercode; alle stehen im Protokoll.
    protokoll::fehler_verwerfen();
    let offen = match gpu {
        Some(n) => {
            let mut optionen = ffmpeg::Dictionary::new();
            optionen.set("gpu", &n.to_string());
            dec.open_as_with(codec, optionen).and_then(|o| o.video())
        }
        None => dec.video(),
    };
    offen.map_err(|e| {
        let worte = protokoll::fehler_abholen();
        match worte.first() {
            Some(w) => format!("{name}: {w}"),
            None => format!("{name}: {e}"),
        }
    })
}

/// Die CUDA-Geraete in CUDAs eigener Reihenfolge, je Ordnungszahl die LUID
/// (None, wenn CUDA fuer dieses Geraet keine nennt, etwa im TCC-Modus).
/// Genau diese Ordnungszahl erwartet cuvid als "gpu". Sie ist NICHT der
/// Index bei DXGI: CUDA sortiert nach Leistung (CUDA_DEVICE_ORDER), und
/// CUDA_VISIBLE_DEVICES kann Karten ausblenden. Die Bruecke ist die LUID, die
/// DXGI fuer jede Karte nennt (`Karte::luid`).
///
/// nvcuda.dll kommt mit dem NVIDIA-Treiber und wird hier zur Laufzeit aus
/// dem Systemordner geladen, nie freigegeben - cuvid laedt dieselbe DLL
/// ohnehin. Fehlt sie, ist das ein Err, kein Absturz. Einmal je Prozess,
/// wie die Kartenerkennung.
#[cfg(windows)]
fn cuda_geraete() -> Result<&'static [Option<i64>], String> {
    static GERAETE: std::sync::OnceLock<Result<Vec<Option<i64>>, String>> = std::sync::OnceLock::new();
    match GERAETE.get_or_init(|| unsafe { cuda_geraete_lesen() }) {
        Ok(g) => Ok(g.as_slice()),
        Err(e) => Err(e.clone()),
    }
}

#[cfg(not(windows))]
fn cuda_geraete() -> Result<&'static [Option<i64>], String> {
    Err("nur unter Windows".into())
}

#[cfg(windows)]
unsafe fn cuda_geraete_lesen() -> Result<Vec<Option<i64>>, String> {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // CUresult und CUdevice sind int, 0 heisst Erfolg. CUDAAPI ist
    // __stdcall - auf x64 dasselbe wie C.
    type CuInit = unsafe extern "system" fn(u32) -> i32;
    type CuDeviceGetCount = unsafe extern "system" fn(*mut i32) -> i32;
    type CuDeviceGet = unsafe extern "system" fn(*mut i32, i32) -> i32;
    type CuDeviceGetLuid = unsafe extern "system" fn(*mut u8, *mut u32, i32) -> i32;
    let dll = LoadLibraryExW(w!("nvcuda.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
        .map_err(|e| format!("nvcuda.dll fehlt ({e})"))?;
    let (Some(init), Some(anzahl), Some(geraet), Some(luid)) = (
        GetProcAddress(dll, s!("cuInit")),
        GetProcAddress(dll, s!("cuDeviceGetCount")),
        GetProcAddress(dll, s!("cuDeviceGet")),
        GetProcAddress(dll, s!("cuDeviceGetLuid")),
    ) else {
        return Err("nvcuda.dll ohne cuDeviceGetLuid (Treiber zu alt)".into());
    };
    let init: CuInit = std::mem::transmute(init);
    let anzahl: CuDeviceGetCount = std::mem::transmute(anzahl);
    let geraet: CuDeviceGet = std::mem::transmute(geraet);
    let luid: CuDeviceGetLuid = std::mem::transmute(luid);
    let r = init(0);
    if r != 0 {
        return Err(format!("cuInit: CUDA-Fehler {r}"));
    }
    let mut n = 0i32;
    let r = anzahl(&mut n);
    if r != 0 {
        return Err(format!("cuDeviceGetCount: CUDA-Fehler {r}"));
    }
    let mut aus = Vec::new();
    for i in 0..n.max(0) {
        let mut d = 0i32;
        let mut bytes = [0u8; 8];
        let mut maske = 0u32;
        let ok = geraet(&mut d, i) == 0 && luid(bytes.as_mut_ptr(), &mut maske, d) == 0;
        aus.push(ok.then(|| luid_aus_bytes(bytes)));
    }
    Ok(aus)
}

/// Die LUID, wie CUDA sie liefert (8 Byte, Speicherbild der Windows-
/// Struktur LUID: LowPart, dann HighPart, beide little-endian), in der Form
/// von `Karte::luid` ((HighPart << 32) | LowPart, siehe anzeige.rs).
#[cfg(any(windows, test))]
fn luid_aus_bytes(bytes: [u8; 8]) -> i64 {
    i64::from_le_bytes(bytes)
}

/// Mit welcher CUDA-Ordnungszahl cuvid diese NVIDIA-Karte trifft: Some(n)
/// fuer die Option "gpu", None fuer CUDAs Vorgabe (Geraet 0), oder der
/// Grund, warum es die Karte nicht sicher treffen kann. Die Vorgabe ist nur
/// recht, wenn es ohnehin nur EINE NVIDIA-Karte gibt - dann ist sie die, und
/// scheitert cuvid, sagt es selbst warum (etwa "Cannot load nvcuvid.dll").
/// Bei zwei Karten gilt: lieber ehrlich auf Software als auf der falschen.
fn nvdec_ziel(karte: &Karte, karten: &[Karte], cuda: Result<&[Option<i64>], String>) -> Result<Option<i32>, String> {
    let einzige = karten.iter().filter(|k| k.nvidia()).count() <= 1;
    match cuda {
        Ok(g) => match g.iter().position(|l| *l == Some(karte.luid)) {
            Some(n) => Ok(Some(n as i32)),
            None if einzige => Ok(None),
            None => Err(format!("{} ist unter den {} CUDA-Geraeten nicht zu finden", karte.name, g.len())),
        },
        Err(_) if einzige => Ok(None),
        Err(e) => Err(format!("{} nicht als CUDA-Geraet zuzuordnen: {e}", karte.name)),
    }
}

/// Zuletzt protokollierte Zuordnung Karte (LUID) -> CUDA-Geraet.
static NVDEC_GEMELDET: Mutex<Option<(i64, i32)>> = Mutex::new(None);

/// Die Zuordnung "Karte ist CUDA-Geraet n" gehoert einmal ins Protokoll,
/// nicht bei jedem Neubau des Decoders. true, wenn sie sich seit der letzten
/// Meldung geaendert hat (und merkt sie sich dann).
fn zuordnung_neu(gemeldet: &Mutex<Option<(i64, i32)>>, luid: i64, n: i32) -> bool {
    let mut g = gemeldet.lock().unwrap_or_else(|e| e.into_inner());
    if *g == Some((luid, n)) {
        return false;
    }
    *g = Some((luid, n));
    true
}

/// Auf welcher Karte cuvid ohne "gpu" laeuft (Automatik): CUDA-Geraet 0.
/// Laesst sich das nicht sagen, aber es gibt nur eine NVIDIA-Karte, ist es
/// die. So passt ein NVDEC aus der Automatik zum Wunsch nach genau dieser
/// Karte, und der Wechsel dorthin baut nicht neu.
fn nvdec_vorgabe(karten: &[Karte], cuda: Result<&[Option<i64>], String>) -> Option<i64> {
    match cuda {
        Ok(g) if !g.is_empty() => g[0],
        _ => {
            let mut nv = karten.iter().filter(|k| k.nvidia());
            match (nv.next(), nv.next()) {
                (Some(k), None) => Some(k.luid),
                _ => None,
            }
        }
    }
}

/// Fehlercode von FFmpeg als Text - mit FFmpegs eigenen Worten dazu, falls
/// es welche gab (die erste Warnung oder der erste Fehler seit dem letzten
/// `fehler_verwerfen`).
fn ffmpeg_grund(was: &str, code: i32) -> String {
    let worte = protokoll::fehler_abholen();
    match worte.first() {
        Some(w) => format!("{was}: {w}"),
        None => format!("{was}: {}", ffmpeg::Error::from(code)),
    }
}

/// get_format-Rueckruf des D3D11VA-Decoders: die Karte, wenn FFmpeg sie
/// anbietet, sonst das erste Software-Format der Liste. Software wird
/// angeboten, wenn die Karte das Profil nicht kann (4:4:4, oder ein Treiber
/// ohne HEVC) - der Decoder rechnet dann auf der CPU weiter, und die
/// Empfangsschleife merkt das am Format des ersten Bildes.
unsafe extern "C" fn d3d11va_format(_ctx: *mut ffmpeg::sys::AVCodecContext, liste: *const ffmpeg::sys::AVPixelFormat) -> ffmpeg::sys::AVPixelFormat {
    use ffmpeg::sys::*;
    let mut p = liste;
    let mut erstes_software = AVPixelFormat::AV_PIX_FMT_NONE;
    while !p.is_null() && *p != AVPixelFormat::AV_PIX_FMT_NONE {
        if *p == AVPixelFormat::AV_PIX_FMT_D3D11 {
            return AVPixelFormat::AV_PIX_FMT_D3D11;
        }
        if erstes_software == AVPixelFormat::AV_PIX_FMT_NONE {
            let desc = av_pix_fmt_desc_get(*p);
            if !desc.is_null() && ((*desc).flags & AV_PIX_FMT_FLAG_HWACCEL as u64) == 0 {
                erstes_software = *p;
            }
        }
        p = p.add(1);
    }
    erstes_software
}

/// D3D11VA-Decoder auf der Karte dieser Rolle. FFmpeg legt dafuer ein
/// eigenes Direct3D-11-Geraet auf dem Adapter an ("device" = Index in der
/// Aufzaehlung von DXGI, dieselbe wie bei --adapter n); die Bilder liegen
/// danach als Texturen dieses Geraets vor und werden je Bild mit
/// av_hwframe_transfer_data in den Hauptspeicher geholt (NV12 bzw. P010) -
/// das ist die Kopierstufe. Null Kopien, also die Texturen direkt in die
/// Anzeige uebernehmen, waere eine spaetere Stufe: dafuer muessten Decoder
/// und Anzeige dasselbe Geraet teilen (AVD3D11VADeviceContext mit unserem
/// ID3D11Device fuellen statt av_hwdevice_ctx_create).
///
/// D3D11VA in FFmpeg 9 kann HEVC Main und Main10 (4:2:0) sowie H.264 -
/// kein 4:4:4 (dxva2.c kennt dafuer keinen Modus). Das prueft
/// `decoder_bauen` vorher; hier wird nur gebaut.
///
/// Scheitern kann schon das Geraet (kein Videodecoder auf dem Adapter, etwa
/// WARP) - das ist der Fehlercode von av_hwdevice_ctx_create, samt FFmpegs
/// Worten dazu. Ob der Treiber das Profil kann, zeigt sich erst am ersten
/// Paket, ueber `d3d11va_format`.
fn d3d11va_decoder(h264: bool, karte: &Karte) -> Result<ffmpeg::decoder::Video, String> {
    use ffmpeg::sys::*;
    let id = if h264 { ffmpeg::codec::Id::H264 } else { ffmpeg::codec::Id::HEVC };
    let name = if h264 { "H.264" } else { "HEVC" };
    let codec = ffmpeg::decoder::find(id).ok_or_else(|| format!("kein {name}-Decoder"))?;
    let geraet = std::ffi::CString::new(karte.index.to_string()).map_err(|e| e.to_string())?;
    protokoll::fehler_verwerfen();
    let mut hw: *mut AVBufferRef = std::ptr::null_mut();
    let r = unsafe { av_hwdevice_ctx_create(&mut hw, AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, geraet.as_ptr(), std::ptr::null_mut(), 0) };
    if r < 0 || hw.is_null() {
        return Err(ffmpeg_grund(&format!("D3D11-Geraet auf Adapter {} ({})", karte.index, karte.name), r));
    }
    let mut ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
    ctx.set_flags(ffmpeg::codec::Flags::LOW_DELAY);
    unsafe {
        let c = ctx.as_mut_ptr();
        // Der Kontext haelt seine eigene Referenz; unsere geht danach weg.
        (*c).hw_device_ctx = av_buffer_ref(hw);
        av_buffer_unref(&mut hw);
        (*c).get_format = Some(d3d11va_format);
    }
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(ffmpeg::Rational(1, 1_000_000));
    dec.video().map_err(|e| {
        let worte = protokoll::fehler_abholen();
        match worte.first() {
            Some(w) => format!("{name}-Decoder: {w}"),
            None => format!("{name}-Decoder: {e}"),
        }
    })
}

/// Die Bilder eines D3D11VA-Decoders ab `von` von der Karte in den
/// Hauptspeicher holen (Kopierstufe): jedes Bild wird durch einen neuen
/// Frame in NV12 bzw. P010 ersetzt, Zeitstempel und Kennzeichen bleiben.
/// Ein Bild, das nicht auf der Karte liegt, heisst: der Treiber kann das
/// Profil nicht, und FFmpeg hat auf der CPU weitergerechnet (siehe
/// `d3d11va_format`) - dann ist der eingebaute Software-Decoder mit seinen
/// Faeden die bessere Wahl, und der Aufrufer wechselt.
fn d3d11va_holen(bilder: &mut [ffmpeg::frame::Video], von: usize) -> Result<(), String> {
    use ffmpeg::sys::*;
    for bild in bilder.iter_mut().skip(von) {
        let auf_karte = unsafe { (*bild.as_ptr()).format == AVPixelFormat::AV_PIX_FMT_D3D11 as i32 };
        if !auf_karte {
            let worte = protokoll::fehler_abholen();
            return Err(match worte.first() {
                Some(w) => format!("der Treiber kann das Profil nicht ({w})"),
                None => format!("der Treiber kann das Profil nicht (Bild kommt als {:?})", bild.format()),
            });
        }
        let mut ziel = ffmpeg::frame::Video::empty();
        let r = unsafe { av_hwframe_transfer_data(ziel.as_mut_ptr(), bild.as_ptr(), 0) };
        if r < 0 {
            return Err(ffmpeg_grund("Bild von der Karte holen (av_hwframe_transfer_data)", r));
        }
        unsafe {
            av_frame_copy_props(ziel.as_mut_ptr(), bild.as_ptr());
        }
        *bild = ziel;
    }
    Ok(())
}

/// Den Hardware-Decoder durch Software ersetzen und den Grund festhalten.
/// Was FFmpeg dazu gesagt hat, steht im Protokoll direkt davor; in den
/// Grund kommt es nicht, der muss in eine Zeile der Statistik passen.
fn auf_software(bedarf: DecoderBedarf, grund: String) -> Result<DecoderBau, String> {
    protokoll::fehler_verwerfen();
    let d = software_decoder(bedarf.h264)?;
    let mut bau = DecoderBau::neu(d, DecoderPfad::Software, if bedarf.h264 { "h264" } else { "hevc" }, Some(grund));
    bau.chroma444 = bedarf.chroma444;
    Ok(bau)
}

/// Ein decodiertes Bild fuer das Protokoll beschreiben: Groesse und Format,
/// so wie der Decoder sie liefert - was to_rgb gleich zu sehen bekommt.
fn bild_beschreiben(codec: &str, bild: &ffmpeg::frame::Video) -> String {
    format!(
        "Erstes Bild aus {}: {}x{} {:?}, Zeilen {}/{}/{} Byte, Ebenen {}",
        codec, bild.width(), bild.height(), bild.format(),
        bild.stride(0),
        if bild.planes() > 1 { bild.stride(1) } else { 0 },
        if bild.planes() > 2 { bild.stride(2) } else { 0 },
        bild.planes()
    )
}

/// Was zum Bau eines Decoders ausser dem Wunsch noch zaehlt: der Codec, ob
/// der Strom 4:4:4 ist (None: noch nicht bekannt), und der Adapter, auf dem
/// die Anzeige laeuft (None: Software, WARP oder ohne Fenster).
#[derive(Clone, Copy)]
struct DecoderBedarf {
    h264: bool,
    chroma444: Option<bool>,
    anzeige_adapter: Option<u32>,
}

/// D3D11VA auf dieser Karte versuchen - oder gleich Software, wenn der
/// Strom 4:4:4 ist. Ergebnis: der fertige Bau, oder der Grund, warum nicht.
fn d3d11va_bau(bedarf: DecoderBedarf, karte: &Karte) -> Result<DecoderBau, String> {
    let sw_name = if bedarf.h264 { "h264" } else { "hevc" };
    let pfad = DecoderPfad::D3d11va(karte.rolle);
    if bedarf.chroma444 == Some(true) {
        return Err(format!("{} kann kein 4:4:4", pfad.name()));
    }
    match d3d11va_decoder(bedarf.h264, karte) {
        Ok(decoder) => {
            let mut bau = DecoderBau::neu(decoder, pfad, sw_name, None);
            bau.chroma444 = bedarf.chroma444;
            Ok(bau)
        }
        Err(e) => Err(format!("{} scheitert: {e}", pfad.name())),
    }
}

/// Decoder fuer HEVC oder H.264 nach Wunsch bauen. Wird beim Start, bei
/// jedem Codecwechsel und bei jedem Wechsel des Wunsches gerufen - der alte
/// Decoder wird dann einfach fallen gelassen.
///
/// Automatik: NVDEC ueber cuvid, wenn eine NVIDIA-Karte erkannt wurde (oder
/// die Erkennung nichts ergab - dann wie frueher einfach probieren); sonst
/// D3D11VA auf der Karte der Anzeige (ohne Anzeige auf der, die Automatik
/// naehme), wenn der Strom 4:2:0 oder H.264 ist; sonst Software.
/// Grafikkarte: NVIDIA ueber cuvid, jede andere ueber D3D11VA. Integriert:
/// D3D11VA. Software: gleich der eingebaute Decoder. Scheitert die Karte,
/// wird Software gebaut und der Grund festgehalten - bei ausdruecklichem
/// Kartenwunsch zeigt `decoder_melden` ihn auch als Fehler.
fn decoder_bauen(bedarf: DecoderBedarf, wunsch: einstellungen::DecoderWunsch) -> Result<DecoderBau, String> {
    use einstellungen::DecoderWunsch as W;
    let h264 = bedarf.h264;
    let sw_name = if h264 { "h264" } else { "hevc" };
    let hw_name = if h264 { "h264_cuvid" } else { "hevc_cuvid" };
    let karten = karten();
    let nvdec = |grund: &mut Option<String>, gpu: Option<i32>, luid: Option<i64>| -> Option<DecoderBau> {
        match nvdec_decoder(h264, gpu) {
            Ok(decoder) => Some(DecoderBau::neu(decoder, DecoderPfad::Nvdec(luid), hw_name, None)),
            Err(e) => {
                *grund = Some(format!("NVDEC nicht verfuegbar: {e}"));
                None
            }
        }
    };
    let mut grund: Option<String> = None;
    let mut wegen_444 = false;
    match wunsch {
        W::Software => {}
        W::Automatik => {
            if karten.is_empty() || karten.iter().any(|k| k.nvidia()) {
                if let Some(bau) = nvdec(&mut grund, None, nvdec_vorgabe(karten, cuda_geraete())) {
                    return Ok(bau);
                }
            }
            // Die Karte der Anzeige, sonst die, die die Anzeige bei
            // Automatik naehme.
            let karte = bedarf
                .anzeige_adapter
                .and_then(|i| karten.iter().find(|k| k.index == i))
                .or_else(|| karte_automatik(karten));
            match karte {
                Some(k) => match d3d11va_bau(bedarf, k) {
                    Ok(bau) => return Ok(bau),
                    Err(e) => {
                        wegen_444 = bedarf.chroma444 == Some(true);
                        grund = Some(match grund {
                            Some(g) => format!("{g}; {e}"),
                            None => e,
                        });
                    }
                },
                None => {
                    if grund.is_none() {
                        grund = Some("keine Grafikkarte erkannt".into());
                    }
                }
            }
        }
        W::Gpu | W::Gpu2 | W::Integriert => {
            let rolle = match wunsch {
                W::Gpu => Rolle::Grafikkarte(1),
                W::Gpu2 => Rolle::Grafikkarte(2),
                _ => Rolle::Integriert,
            };
            match karte_mit(karten, rolle) {
                // Genau diese Karte, nicht CUDAs Vorgabe - siehe nvdec_ziel.
                Some(k) if k.nvidia() => match nvdec_ziel(k, karten, cuda_geraete()) {
                    Ok(gpu) => {
                        if let Some(n) = gpu.filter(|&n| zuordnung_neu(&NVDEC_GEMELDET, k.luid, n)) {
                            protokoll::zeile(format!("NVDEC: {} ist CUDA-Geraet {n}", k.name));
                        }
                        if let Some(bau) = nvdec(&mut grund, gpu, Some(k.luid)) {
                            return Ok(bau);
                        }
                    }
                    Err(e) => {
                        grund = Some(format!("NVDEC nicht verfuegbar: {e}"));
                    }
                },
                Some(k) => match d3d11va_bau(bedarf, k) {
                    Ok(bau) => return Ok(bau),
                    Err(e) => {
                        wegen_444 = bedarf.chroma444 == Some(true);
                        grund = Some(e);
                    }
                },
                None => {
                    grund = Some(format!("D3D11VA ({}) scheitert: keine Karte mit dieser Rolle erkannt", rolle.name()));
                }
            }
        }
    }
    let decoder = software_decoder(h264)?;
    let mut bau = DecoderBau::neu(decoder, DecoderPfad::Software, sw_name, grund);
    bau.chroma444 = bedarf.chroma444;
    bau.wegen_444 = wegen_444;
    Ok(bau)
}

/// Passt der laufende Decoder zu diesem Wunsch, so dass ein Neubau nichts
/// aendern wuerde? Ein Neubau haelt das Bild bis zum naechsten
/// Schluesselbild an - den gibt es nur, wenn er etwas bringen kann.
fn wunsch_passt(wunsch: einstellungen::DecoderWunsch, pfad: DecoderPfad) -> bool {
    wunsch_passt_mit(wunsch, pfad, karten())
}

/// `wunsch_passt` mit gegebenen Karten. NVDEC passt zu einer Rolle nur, wenn
/// er auf genau der Karte dieser Rolle laeuft - zwei NVIDIA-Karten sind
/// zwei verschiedene Wuensche.
fn wunsch_passt_mit(wunsch: einstellungen::DecoderWunsch, pfad: DecoderPfad, karten: &[Karte]) -> bool {
    use einstellungen::DecoderWunsch as W;
    match wunsch {
        W::Software => pfad == DecoderPfad::Software,
        W::Automatik => pfad.hardware(),
        W::Gpu | W::Gpu2 | W::Integriert => {
            let rolle = match wunsch {
                W::Gpu => Rolle::Grafikkarte(1),
                W::Gpu2 => Rolle::Grafikkarte(2),
                _ => Rolle::Integriert,
            };
            match pfad {
                DecoderPfad::Nvdec(luid) => {
                    karte_mit(karten, rolle).map(|k| k.nvidia() && luid == Some(k.luid)).unwrap_or(false)
                }
                DecoderPfad::D3d11va(r) => r == rolle,
                DecoderPfad::Software => false,
            }
        }
    }
}

/// Muss der Decoder neu gebaut werden, weil die Strominfo jetzt sagt, ob
/// der Strom 4:4:4 ist? Nur, wenn das an der Wahl etwas aendert: D3D11VA
/// laeuft und der Strom ist 4:4:4 (geht nicht), oder Software laeuft nur
/// wegen 4:4:4 und der Strom ist es nicht mehr.
fn chroma_erzwingt_neubau(bau: &DecoderBau, chroma444: bool) -> bool {
    if bau.chroma444 == Some(chroma444) {
        return false;
    }
    match bau.pfad {
        DecoderPfad::D3d11va(_) => chroma444,
        DecoderPfad::Software => bau.wegen_444 && !chroma444,
        DecoderPfad::Nvdec(_) => false,
    }
}

/// Ergebnis eines frisch gebauten Decoders in `Shared` eintragen: Pfad,
/// Hinweis und die Protokollzeile. Bei ausdruecklichem Kartenwunsch wird
/// ein Rueckfall zusaetzlich als Fehler gezeigt - wer die Karte verlangt,
/// soll erfahren, dass er sie nicht bekommt.
fn decoder_melden(shared: &Arc<Mutex<Shared>>, bau: &DecoderBau, wunsch: einstellungen::DecoderWunsch) {
    protokoll::zeile(bau.meldung());
    if matches!(bau.pfad, DecoderPfad::Nvdec(_)) && aud_gewuenscht() {
        protokoll::zeile("NVDEC: Zugriffseinheiten-Begrenzer angehaengt".into());
    }
    if matches!(bau.pfad, DecoderPfad::Nvdec(_)) && bau.codec.starts_with("h264") && vui_gewuenscht() {
        protokoll::zeile("NVDEC: SPS um VUI ergaenzt (max_num_reorder_frames 0)".into());
    }
    let mut s = shared.lock().unwrap();
    s.decoder_pfad = Some(bau.pfad);
    s.decoder_hinweis = bau.grund.clone();
    if !matches!(wunsch, einstellungen::DecoderWunsch::Automatik | einstellungen::DecoderWunsch::Software) {
        if let Some(g) = &bau.grund {
            // Der Grund steht schon im Protokoll (bau.meldung) und in der
            // Decoderzeile der Statistik; die Oberflaeche sagt den Satz.
            s.error = Some(Meldung::neu(strings::Key::DecoderFallback, g.clone()));
        }
    }
}

/// Zugriffseinheiten-Begrenzer (AUD) als Annex-B-NAL: Startcode, dann bei
/// HEVC NAL-Typ 35 (Kopf 46 01) mit pic_type 2 = "I, P oder B" (010, dann
/// das Abschlussbit: 0x50), bei H.264 NAL-Typ 9 (Kopf 09) mit
/// primary_pic_type 7 = "beliebig" (111, Abschlussbit: 0xF0).
const AUD_HEVC: [u8; 7] = [0, 0, 0, 1, 0x46, 0x01, 0x50];
const AUD_H264: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];
const NAL_AUD_HEVC: u8 = 35;
const NAL_AUD_H264: u8 = 9;

/// Bekommt jede Zugriffseinheit fuer NVDEC einen AUD angehaengt? Ja, ausser
/// mit --ohne-aud - der Schalter ist zum Vergleich auf demselben Rechner da.
fn aud_gewuenscht() -> bool {
    !std::env::args().any(|a| a == "--ohne-aud")
}

/// Bekommt jedes H.264-SPS fuer NVDEC ein VUI mit max_num_reorder_frames 0?
/// Ja, ausser mit --ohne-vui - zum Vergleich auf demselben Rechner.
///
/// Der Mac-Host codiert ohne Bildumsortierung, sagt es aber nicht im SPS
/// (VideoToolbox schreibt kein VUI). cuvids H.264-Parser nimmt dann die
/// groesste Umsortierungstiefe des Levels an und haelt rund 16 Bilder
/// zurueck - gemessen 150-290 ms Decoderzeit. Siehe `sps.rs`.
fn vui_gewuenscht() -> bool {
    !std::env::args().any(|a| a == "--ohne-vui")
}

/// Typ des letzten NAL einer Annex-B-Zugriffseinheit: der Kopf hinter dem
/// letzten Startcode 00 00 01 (mit oder ohne fuehrender Null). HEVC traegt
/// den Typ in den Bits 1..6 des ersten Kopfbytes, H.264 in den Bits 0..4.
fn letzter_nal_typ(au: &[u8], h264: bool) -> Option<u8> {
    let mut i = au.len().checked_sub(4)?;
    loop {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            let kopf = au[i + 3];
            return Some(if h264 { kopf & 0x1f } else { (kopf >> 1) & 0x3f });
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// Die Zugriffseinheit mit angehaengtem AUD - fuer den cuvid-Decoder.
///
/// NVIDIAs Parser (cuvidParseVideoData) gibt ein Bild erst frei, wenn er
/// den Anfang des NAECHSTEN Bildes sieht: erst ein NAL, das kein Teil des
/// laufenden Bildes sein kann, schliesst es ab. Ohne Hilfe ist das das
/// erste Scheiben-NAL des naechsten Pakets - ein Bild Verzoegerung, bei 60
/// Bildern 16,7 ms Decoderzeit fuer nichts. FFmpegs cuviddec.c setzt das
/// Ende-Kennzeichen des Parsers (CUVID_PKT_ENDOFPICTURE) nie und hat auch
/// keine Option dafuer (geprueft an FFmpeg 9, Zweig release/9.0: nur
/// CUVID_PKT_TIMESTAMP, am Ende ENDOFSTREAM). Ein angehaengter AUD ist der
/// einzige Weg von aussen: er gehoert per Definition zum naechsten Bild,
/// also ist das laufende zu Ende - noch in diesem Aufruf. Ein AUD VORNE
/// (wie ihn manche Encoder setzen) hilft dem letzten Bild nicht; deshalb
/// hinten, und nur, wenn die Einheit nicht ohnehin mit einem endet. Fuer
/// die Software-Decoder ohne Belang, die schliessen scheibenparallel ab.
fn mit_aud(au: &[u8], h264: bool) -> std::borrow::Cow<'_, [u8]> {
    let (aud, typ): (&[u8], u8) = if h264 { (&AUD_H264, NAL_AUD_H264) } else { (&AUD_HEVC, NAL_AUD_HEVC) };
    if letzter_nal_typ(au, h264) == Some(typ) {
        return std::borrow::Cow::Borrowed(au);
    }
    let mut mit = Vec::with_capacity(au.len() + aud.len());
    mit.extend_from_slice(au);
    mit.extend_from_slice(aud);
    std::borrow::Cow::Owned(mit)
}

/// Ist das ein Fehler, der einen Hardware-Decoder als kaputt ausweist?
/// EAGAIN heisst nur "gerade nichts da", EOF "fertig" - beides ist normal.
fn decoder_defekt(e: &ffmpeg::Error) -> bool {
    !matches!(e, ffmpeg::Error::Other { errno: ffmpeg::util::error::EAGAIN } | ffmpeg::Error::Eof)
}

fn run_session(addr: &str, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) -> Result<(), Meldung> {
    // Erst der Handschlag, dann erst Nutzdaten. Vorher geht nichts ueber die
    // Leitung, was jemand mitlesen koennte.
    //
    // known_hosts.txt wird VOR dem Verbinden gelesen: ist sie unlesbar, geht
    // keine Leitung auf. Den Schluessel des Hosts prueft der Handschlag nach
    // Nachricht 2, bevor Nachricht 3 den eigenen zeigt - der Host nimmt
    // diesen Client erst mit Nachricht 3 an und loest dafuer den laufenden
    // Zuschauer ab. Mit falschem Pin kommt es dazu also nicht mehr.
    let pin = secure::HostPin::laden(addr)?;
    let mut sock = secure::Secure::connect_pruefend(addr, &noise::prologue_video(), |k| pin.pruefen(k))?;
    shared.lock().unwrap().abbruch = sock.abbruchgriff();
    // Erster Kontakt: der Schluessel kommt erst nach dem Handschlag in die Liste.
    let first = pin.eintragen(&sock.peer)?;
    let fp = sock.peer_fingerprint();
    {
        let mut s = shared.lock().unwrap();
        s.connected = true;
        s.error = None;
        s.error_key = None;
        s.link = Some((sock.handshake_hash.clone(), sock.peer.clone()));
        s.sas = Some(sock.sas.clone());
        s.peer_fp = Some(fp.clone());
        s.first_time = first;
        // Die Liste des vorigen Hosts hat hier nichts mehr zu suchen; die
        // neue kommt gleich nach dem Gruss.
        s.codecs.clear();
        s.codec_idx = None;
        s.codec_wechsel = None;
    }

    // Weist der Host das Geraet ab, macht er die Leitung gleich nach dem
    // Handschlag zu. Das ist kein Netzfehler, sondern eine Aussage - also
    // sagen wir es dem Nutzer auch so.
    let mut magic = [0u8; 4];
    if sock.read_exact(&mut magic).is_err() {
        shared.lock().unwrap().error_key = Some(strings::Key::NotPaired);
        return Err(Meldung::neu(
            strings::Key::NotPaired,
            "Host hat die Leitung nach dem Handschlag geschlossen - dieses Geraet ist dort nicht gekoppelt",
        ));
    }
    if &magic != MAGIC {
        return Err(Meldung::neu(strings::Key::ErrorProtocol, "Gegenstelle spricht ein anderes Protokoll"));
    }
    // Erst jetzt ist es eine Sitzung: der Host hat dieses Geraet angenommen.
    #[cfg(any(windows, target_os = "macos"))]
    clipboard::sitzung(true);

    // Der Host faengt immer mit HEVC an; alles Weitere sagt Nachricht 7.
    // Der Decoder ist eine eigene Variable, keine Leihgabe: bei einem Wechsel
    // wird sie schlicht neu zugewiesen, und der alte Decoder faellt weg.
    // Der Wunsch, mit dem hier gebaut wird, ist damit der geltende: wurde er
    // waehrend des Verbindens angeklickt, steht seine Flagge noch - und die
    // wuerde gleich im ersten Durchlauf denselben Decoder noch einmal bauen.
    // Wunsch und Flagge unter einem Griff lesen, damit die Flagge auch
    // wirklich zu diesem Wunsch gehoert.
    let mut wunsch = {
        let mut s = shared.lock().unwrap();
        s.decoder_wunsch_neu = false;
        s.decoder_wunsch
    };
    // Wofuer der Decoder gebaut ist: der Host faengt mit HEVC an, ob 4:4:4,
    // sagt erst die Strominfo - die entscheidet gleich, ob das passt (der
    // Host kann laengst auf einem anderen Codec stehen). Der Adapter der
    // Anzeige ist der Kandidat fuer D3D11VA bei Automatik.
    let mut bedarf = DecoderBedarf { h264: false, chroma444: None, anzeige_adapter: shared.lock().unwrap().anzeige_adapter };
    let mut bau = decoder_bauen(bedarf, wunsch).map_err(kein_decoder)?;
    decoder_melden(shared, &bau, wunsch);
    // Nach einem Wechsel darf nichts in den neuen Decoder, bevor das erste
    // Schluesselbild da ist - es traegt die Parametersaetze.
    let mut warte_auf_schluesselbild = false;
    // Das erste Bild aus dem neuen Decoder beendet den Hinweis "wird gewechselt".
    let mut nach_wechsel = false;
    // Steht in `shared.error` gerade ein Fehler aus DIESEM Bildpfad? Nur den
    // darf ein gutes Bild wieder loeschen - Fehler anderer Pfade (Ton,
    // Verbindung) bleiben stehen, statt hundertmal je Sekunde zu verschwinden.
    let mut bild_fehler = false;
    // Die erste Zeigerform je Sitzung einmal ins Protokoll.
    let mut zeiger_gemeldet = false;
    // Jeder Zugriffseinheit fuer NVDEC einen Begrenzer (AUD) anhaengen,
    // damit cuvids Parser das Bild sofort abschliesst - siehe `mit_aud`.
    // --ohne-aud laesst es zum Vergleich weg.
    let aud_anhang = aud_gewuenscht();
    // Jedes H.264-SPS fuer NVDEC um ein VUI mit max_num_reorder_frames 0
    // ergaenzen, damit cuvids Parser keine Bilder fuer eine Umsortierung
    // zurueckhaelt, die es nicht gibt - siehe `sps.rs`. --ohne-vui laesst
    // es zum Vergleich weg. Das erste Umschreiben kommt ins Protokoll.
    let vui_anhang = vui_gewuenscht();
    let mut vui_gemeldet = false;
    // --mitschnitt datei.hevc (Pruefmodus): die Zugriffseinheiten roh in
    // eine Datei, ohne unsere Koepfe, ab dem ersten Vollbild - als Konserve
    // fuer den Windows-Host (--konserve) oder zum Abspielen mit ffplay.
    let mut mitschnitt: Option<std::fs::File> = std::env::args()
        .position(|a| a == "--mitschnitt")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|p| match std::fs::File::create(&p) {
            Ok(f) => {
                protokoll::zeile(format!("Mitschnitt nach {p}"));
                Some(f)
            }
            Err(e) => {
                protokoll::zeile(format!("Mitschnitt nach {p} nicht moeglich: {e}"));
                None
            }
        });
    let mut mitschnitt_laeuft = false;

    let mut info: Option<StreamInfo> = None;
    #[cfg(any(windows, target_os = "macos"))]
    let mut sound: Option<audio::AudioOut> = None;
    // Steht in `shared.error` die Meldung "Ton nicht verfuegbar", waehrend
    // der Tonfaden weiter nach einem Geraet sucht? Dann loescht sie das
    // erste Tonpaket, das ein offenes Geraet vorfindet.
    #[cfg(any(windows, target_os = "macos"))]
    let mut ton_fehler = false;
    let mut pcm: Vec<f32> = Vec::new();
    let mut hdr = [0u8; 8];
    let mut payload: Vec<u8> = Vec::with_capacity(1 << 20);

    // Zeitabgleich und Latenzmittelung. Der beste Abgleich ist der mit der
    // kuerzesten Umlaufzeit - da ist am wenigsten Warterei drin, die sich
    // ungleich auf Hin- und Rueckweg verteilen koennte.
    let mut lat = Latenz::default();
    let mut letzter_abgleich = Instant::now() - Duration::from_secs(10);
    // Die Stempel warten in einem kleinen Ring, bis ihr Bild aus dem Decoder
    // kommt. Der Decoder arbeitet mit mehreren Faeden und gibt Bilder erst
    // Faeden spaeter heraus - wer einfach den zuletzt eingetroffenen Stempel
    // nimmt, misst deshalb eine viel zu kurze Verzoegerung.
    // Eintrag: Bildnummer, Aufnahmezeit, Encoderzeit, nachgelegt, Ankunftszeit.
    let mut ring: std::collections::VecDeque<(u16, u64, u64, bool, u64)> = std::collections::VecDeque::new();
    // Die letzten Umlaufzeiten. Ohne Fenster friert ein einzelner Gluecksfall
    // den Uhrenversatz fuer die ganze Sitzung ein und die Drift laeuft weg.
    let mut umlaeufe: Vec<(u64, i64)> = Vec::new();
    let mut mittel: Option<(f32, f32, f32, f32)> = None;

    loop {
        // Wurde die Trennung verlangt, ist hier Schluss - auch wenn der Host
        // gerade noch fleissig sendet.
        let wunsch_neu = {
            let mut s = shared.lock().unwrap();
            if s.target.is_none() {
                return Ok(());
            }
            if s.decoder_wunsch_neu {
                s.decoder_wunsch_neu = false;
                Some(s.decoder_wunsch)
            } else {
                None
            }
        };
        // Der Wunsch hat sich im Menue geaendert. Nur neu bauen, wenn er
        // wirklich ein anderer ist als der, mit dem gebaut wurde: eine
        // Flagge zum selben Wunsch (Klick auf das, was schon gilt) wuerde
        // sonst denselben Decoder noch einmal bauen und das Bild bis zum
        // naechsten Schluesselbild anhalten - auf einer Maschine ohne NVIDIA
        // bei jedem Klick. Und auch bei einem neuen Wunsch nur, wenn das
        // etwas aendern kann: wer bei Automatik schon auf NVDEC steht,
        // braucht fuer NVIDIA keinen Stillstand.
        if let Some(w) = wunsch_neu {
            if w != wunsch {
                wunsch = w;
                if !wunsch_passt(w, bau.pfad) {
                    bau = decoder_bauen(bedarf, wunsch).map_err(kein_decoder)?;
                    decoder_melden(shared, &bau, wunsch);
                    warte_auf_schluesselbild = true;
                    ring.clear();
                    mittel = None;
                }
            }
        }
        // Regelmaessig nachfragen: Uhren laufen auseinander, und beim ersten
        // Versuch steht der Eingabekanal oft noch gar nicht.
        let faellig = if lat.versatz_us == 0 { 1 } else { 5 };
        if letzter_abgleich.elapsed() >= Duration::from_secs(faellig) {
            letzter_abgleich = Instant::now();
            if let Ok(mut l) = input.try_lock() {
                let t1 = client_us();
                l.zeitfrage(t1);
            }
        }

        let verloren = |e: String| Meldung::neu(strings::Key::ConnectionLost, e);
        sock.read_exact(&mut hdr).map_err(|e| verloren(format!("Kopf: {e}")))?;
        let msg_type = hdr[0];
        let flags = hdr[1];
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        if len > 64 * 1024 * 1024 {
            return Err(Meldung::neu(strings::Key::ErrorProtocol, "unplausible Nachrichtenlaenge"));
        }
        payload.resize(len, 0);
        sock.read_exact(&mut payload).map_err(|e| verloren(format!("Daten: {e}")))?;

        match msg_type {
            MSG_INFO => {
                if let Some(i) = StreamInfo::parse(&payload) {
                    // Steht der Host schon auf einem anderen Codec als dem, fuer
                    // den der Decoder gebaut wurde, muss der Decoder jetzt
                    // passen. Sonst versucht ein HEVC-Decoder, H.264 zu lesen,
                    // und es kommt nie ein Bild - so geschehen heute Morgen.
                    // Ebenso, wenn erst jetzt feststeht, ob der Strom 4:4:4
                    // ist, und das an der Wahl etwas aendert (D3D11VA kann
                    // kein 4:4:4).
                    let h264 = i.codec == 2;
                    let neubau = h264 != bedarf.h264 || chroma_erzwingt_neubau(&bau, i.chroma444);
                    // Der Bedarf gilt ab jetzt auch fuer spaetere Baue (Wechsel
                    // des Wunsches), ob jetzt neu gebaut wird oder nicht.
                    bedarf.h264 = h264;
                    bedarf.chroma444 = Some(i.chroma444);
                    bau.chroma444 = Some(i.chroma444);
                    if neubau {
                        bau = decoder_bauen(bedarf, wunsch).map_err(kein_decoder)?;
                        decoder_melden(shared, &bau, wunsch);
                        warte_auf_schluesselbild = true;
                        nach_wechsel = true;
                        ring.clear();
                        mittel = None;
                    }
                    info = Some(i);
                    shared.lock().unwrap().info = Some(i);
                }
            }
            MSG_HOSTSTATUS => {
                // Der Host sagt selbst, ob er gerade ein Bild liefern kann -
                // etwa wenn sein Bildschirm weg ist. Sonst saehe das aus wie
                // eine tote Verbindung.
                if len >= 1 {
                    let mut s = shared.lock().unwrap();
                    if payload[0] == 1 {
                        s.error_key = Some(strings::Key::NoDisplay);
                    } else if s.error_key == Some(strings::Key::NoDisplay) {
                        s.error_key = None;
                    }
                }
            }
            MSG_ABGELOEST => {
                // Ein anderes Geraet hat die Sitzung uebernommen, der Host
                // macht gleich zu. Nicht von selbst neu verbinden - sonst
                // verdraengte dieser Client den neuen, und der ihn wieder:
                // zwei Clients loesten einander endlos ab. Das Ziel geht
                // zurueck; der Fensterfaden zeigt daraufhin den
                // Startbildschirm mit der Meldung.
                protokoll::zeile(format!(
                    "Sitzung mit {addr} von einem anderen Geraet uebernommen (Host meldet Abloesung) - keine automatische Neuverbindung"
                ));
                let mut s = shared.lock().unwrap();
                s.target = None;
                s.error_key = Some(strings::Key::SessionTakenOver);
                return Ok(());
            }
            MSG_CODECS => {
                let liste = codecs_parsen(&payload);
                shared.lock().unwrap().codecs = liste;
            }
            MSG_SWITCH => {
                // Ab hier spricht der Host einen anderen Codec. Der alte
                // Decoder wird fallen gelassen, ein neuer gebaut, und alles,
                // was noch zum alten Strom gehoerte, ist damit wertlos: die
                // wartenden Stempel und der Latenzmittelwert.
                let Some(w) = CodecWechsel::parse(&payload) else { continue };
                // Mit Bildparallelitaet (--faeden frame) gehen beim Wechsel bis zu
                // 15 zurueckgehaltene Bilder verloren; sauber leeren ist eine spaetere Verfeinerung.
                bedarf.h264 = w.is_h264;
                bedarf.chroma444 = Some(w.chroma444);
                bau = decoder_bauen(bedarf, wunsch).map_err(kein_decoder)?;
                decoder_melden(shared, &bau, wunsch);
                warte_auf_schluesselbild = true;
                nach_wechsel = true;
                ring.clear();
                mittel = None;
                // Die Strominfo kommt gleich hinterher; bis dahin gilt schon,
                // was der Wechsel selbst gesagt hat - dann zeigt das Menue
                // ohne Verzug den richtigen Eintrag als laufend.
                if let Some(i) = info.as_mut() {
                    i.codec = if w.is_h264 { 2 } else { 1 };
                    i.chroma444 = w.chroma444;
                    i.ten_bit = w.ten_bit;
                }
                let mut s = shared.lock().unwrap();
                s.info = info;
                s.codec_idx = Some(w.idx);
            }
            MSG_VIDEO => {
                // Jedes Byte zaehlt, auch das eines Bildes, das gleich
                // verworfen wird: die Leitung hat es getragen.
                shared.lock().unwrap().bytes_video += len as u64;
                // Nach einem Wechsel muss das erste Bild ein Schluesselbild
                // sein (Flaggenbit 0). Alles andere gehoert noch zum alten
                // Codec oder ist ohne Parametersaetze nicht decodierbar.
                if warte_auf_schluesselbild {
                    if flags & 1 == 0 {
                        continue;
                    }
                    warte_auf_schluesselbild = false;
                }
                if let Some(f) = mitschnitt.as_mut() {
                    use std::io::Write;
                    if flags & FLAG_KEY != 0 {
                        mitschnitt_laeuft = true;
                    }
                    if mitschnitt_laeuft && f.write_all(&payload).is_err() {
                        protokoll::zeile("Mitschnitt: Schreiben fehlgeschlagen, beendet".into());
                        mitschnitt = None;
                    }
                }
                // Ankunftszeit sofort nehmen, noch vor dem Decodieren.
                let t_empfangen = client_us();
                let t0 = Instant::now();
                // Die Bildnummer reist durch den Decoder mit, damit auf der
                // anderen Seite der richtige Stempel zum richtigen Bild passt.
                let seq = u16::from_le_bytes([hdr[2], hdr[3]]);
                // Ankunftszeit gehoert zu DIESEM Bild, nicht zu dem, das
                // gleich aus dem Decoder faellt - der haelt mehrere zurueck.
                if let Some(e) = ring.iter_mut().find(|(s, ..)| *s == seq) {
                    e.4 = t_empfangen;
                }
                // Nur auf dem NVDEC-Pfad; D3D11VA und Software bekommen
                // die Einheit, wie sie kam.
                let mut packet = if matches!(bau.pfad, DecoderPfad::Nvdec(_)) {
                    // Erst das SPS (nur H.264, nur wenn eines drin ist -
                    // sonst keine Kopie), dann der AUD hinten dran.
                    let mit_vui = if vui_anhang && bedarf.h264 { sps::h264_au_mit_vui(&payload) } else { None };
                    if let Some(neu) = &mit_vui {
                        if !vui_gemeldet {
                            vui_gemeldet = true;
                            let alt_len = sps::erstes_sps(&payload).map(|s| s.len()).unwrap_or(0);
                            let neu_len = alt_len + (neu.len() - payload.len());
                            protokoll::zeile(format!("NVDEC: SPS umgeschrieben, {alt_len} -> {neu_len} Byte (VUI mit max_num_reorder_frames 0)"));
                        }
                    }
                    let einheit: &[u8] = mit_vui.as_deref().unwrap_or(&payload);
                    if aud_anhang {
                        ffmpeg::Packet::copy(&mit_aud(einheit, bedarf.h264))
                    } else {
                        ffmpeg::Packet::copy(einheit)
                    }
                } else {
                    ffmpeg::Packet::copy(&payload)
                };
                packet.set_pts(Some(seq as i64));
                packet.set_dts(None);
                // Decodieren, und zwar so, dass ein Hardware-Decoder, der
                // nichts taugt, stumm durch Software ersetzt wird. Nichts
                // taugen heisst: er scheitert, bevor er je ein Bild geliefert
                // hat (Karte kann das Profil nicht, Treiber streikt), oder er
                // scheitert dreimal in Folge. Ein einzelner Fehler mitten in
                // der Sitzung - etwa AVERROR_EXTERNAL, weil cuvids
                // Bildwarteschlange gerade voll ist - kostet nur dieses Paket:
                // es wird verworfen, und bis zum naechsten Schluesselbild (der
                // Host schickt alle ein bis zwei Sekunden eines) geht nichts
                // mehr hinein. Die Karte bleibt; sonst waere sie nach einem
                // Aussetzer bis zum naechsten Codecwechsel verloren.
                // Und ein Hardware-Decoder, der alles annimmt und nichts
                // liefert, meldet gar keinen Fehler - so sah es auf der RTX
                // 3080 Ti aus: der Host sendete 115 Bilder je Sekunde ohne
                // Stau, der Client las alles, und es kam nie ein Bild.
                // Schweigen ueber die Frist hinaus zaehlt deshalb wie ein
                // Defekt, ob von Anfang an oder mittendrin.
                // Zwei Anlaeufe: der zweite nur nach einem Rueckfall, und nur,
                // wenn dieses Paket ein Schluesselbild ist - alles andere ist
                // fuer den frischen Decoder ohnehin wertlos. Im zweiten Anlauf
                // laeuft Software, und die faellt nie zurueck.
                let mut bilder: Vec<ffmpeg::frame::Video> = Vec::new();
                for _anlauf in 0..2 {
                    bau.paket();
                    let vorher = bilder.len();
                    let fehler = decoder_fuettern(&mut bau.decoder, &packet, &mut bilder);
                    // D3D11VA: die Bilder liegen auf der Karte und muessen
                    // erst in den Hauptspeicher (Kopierstufe). Geht das
                    // nicht - oder rechnet der Decoder in Wahrheit auf der
                    // CPU, weil der Treiber das Profil nicht kann -, taugt
                    // er so wenig wie ein cuvid, der Fehler wirft.
                    let mut holfehler: Option<String> = None;
                    if matches!(bau.pfad, DecoderPfad::D3d11va(_)) && bilder.len() > vorher {
                        if let Err(e) = d3d11va_holen(&mut bilder, vorher) {
                            bilder.truncate(vorher);
                            holfehler = Some(format!("{} scheitert: {e}", bau.pfad.name()));
                        }
                    }
                    if bilder.len() > vorher {
                        bau.bild();
                        // Das erste Bild jedes Decoders einmal beschreiben -
                        // hier, solange `bau` noch der Decoder ist, der es
                        // geliefert hat.
                        if !bau.bild_gemeldet {
                            bau.bild_gemeldet = true;
                            protokoll::zeile(bild_beschreiben(bau.codec, &bilder[vorher]));
                        }
                    }
                    let grund = match (fehler, holfehler) {
                        (_, Some(h)) => h,
                        (None, None) => {
                            bau.fehler_folge = 0;
                            if !bau.stumm() {
                                break;
                            }
                            format!(
                                "{} ({}) nimmt Pakete an, liefert aber kein Bild ({} Pakete, {:.1} s)",
                                bau.pfad.name(), bau.codec, bau.pakete_seit_bild, bau.stumm_seit().as_secs_f32()
                            )
                        }
                        (Some(e), None) => {
                            if !bau.pfad.hardware() || !decoder_defekt(&e) {
                                // Software-Decoder: ein kaputtes Paket ist ein
                                // kaputtes Paket, das naechste kommt gleich.
                                break;
                            }
                            bau.fehler_folge = bau.fehler_folge.saturating_add(1);
                            if bau.hat_bild && bau.fehler_folge < 3 {
                                // Ein Aussetzer, kein Defekt: Paket weg, Decoder bleibt.
                                warte_auf_schluesselbild = true;
                                break;
                            }
                            format!("{} ({}) scheitert an Paket {}: {e}", bau.pfad.name(), bau.codec, bau.pakete)
                        }
                    };
                    // Der Hardware-Decoder ist nichts wert: Software bauen
                    // und den Grund festhalten. Der Wunsch bleibt, wie er
                    // war - beim naechsten Codecwechsel wird die Karte wieder
                    // probiert, mit einem anderen Codec kann sie ja gehen.
                    bau = auf_software(bedarf, grund).map_err(kein_decoder)?;
                    decoder_melden(shared, &bau, wunsch);
                    ring.clear();
                    mittel = None;
                    if flags & 1 == 0 {
                        warte_auf_schluesselbild = true;
                        break;
                    }
                }
                // Das Format des letzten Bildes, bevor die Schleife die Bilder
                // verbraucht - der Rueckfall unten will es nennen.
                let letztes_format = bilder.last().map(|b| b.format());
                // Zeichnet die Karte, bleibt das Bild roh; einmal je Paket
                // nachsehen reicht.
                let gpu = shared.lock().unwrap().gpu_pfad;
                for decoded in bilder.drain(..) {
                    let pts = decoded.pts();
                    let (w, h) = (decoded.width(), decoded.height());
                    // Das Format entscheidet der decodierte Frame selbst, nicht
                    // die Strominfo. Ein unbekanntes Format ergibt ein dunkles
                    // Bild und eine Meldung - nie einen Absturz.
                    // `bereit_us` ist die Client-Uhr bei der Ablage: ab hier
                    // zaehlt das Glied Anzeige, das der Fensterfaden misst.
                    let (bild, fehler) = if gpu && ebenen_format(decoded.format()).is_some() {
                        (Bild::Roh { bild: decoded, bereit_us: client_us() }, None)
                    } else {
                        match to_rgb(&decoded) {
                            Ok(f) => (Bild::Rgb(Frame { bereit_us: client_us(), ..f }), None),
                            Err(e) => {
                                let m = Meldung::neu(strings::Key::ErrorPixelFormat, e).anhang(format!("{:?}", decoded.format()));
                                (Bild::Rgb(Frame { bereit_us: client_us(), ..dunkles_bild(w, h) }), Some(m))
                            }
                        }
                    };
                    let ms = t0.elapsed().as_secs_f32() * 1000.0;
                    let mut s = shared.lock().unwrap();
                    if s.frame.is_some() {
                        s.dropped += 1; // das vorige wurde nie gezeigt
                    }
                    s.frame = Some(bild);
                    s.decoded += 1;
                    s.last_decode_ms = ms;
                    if let Some(e) = fehler {
                        s.error = Some(e);
                        bild_fehler = true;
                        bau.format_fehler = bau.format_fehler.saturating_add(1);
                    } else {
                        bau.format_fehler = 0;
                        if bild_fehler {
                            s.error = None;
                            bild_fehler = false;
                        }
                    }
                    if nach_wechsel {
                        // Das erste Bild des neuen Codecs ist da; der Hinweis
                        // "wird gewechselt" hat seinen Dienst getan.
                        nach_wechsel = false;
                        s.codec_wechsel = None;
                    }

                    // Verzoegerung zerlegen. Geht nur, wenn der Zeitabgleich
                    // steht und der Stempel zu genau diesem Bild gefunden wird.
                    let passend = pts.and_then(|p| {
                        let seq = p as u16;
                        ring.iter().position(|(s, ..)| *s == seq).map(|i| {
                            let e = ring[i];
                            // Alles davor ist ueberholt und kann weg.
                            ring.drain(..=i);
                            e
                        })
                    });
                    if lat.versatz_us != 0 {
                        // Nachgelegte Bilder zeigen ein altes Standbild; ihre
                        // Verzoegerung sagt nichts ueber die Strecke aus.
                        if let Some((_, t_cap, t_enc, false, t_arr)) = passend {
                            let an_host = t_arr as i64 + lat.versatz_us;
                            let fertig_host = client_us() as i64 + lat.versatz_us;
                            let enc = (t_enc.saturating_sub(t_cap)) as f32 / 1000.0;
                            let net = (an_host - t_enc as i64) as f32 / 1000.0;
                            // Der Decoder-Anteil ist Warten PLUS Arbeit: mit
                            // mehreren Faeden haelt er Bilder zurueck, und
                            // genau das soll hier sichtbar werden. Auf dem
                            // CPU-Weg steckt auch die Umrechnung nach RGB
                            // darin; rechnet die Karte, wandert sie ins Glied
                            // Anzeige - der Decoder wirkt dann um die 1-2 ms
                            // schneller, ohne dass sich am Decodieren etwas
                            // geaendert haette.
                            let dec = (fertig_host - an_host) as f32 / 1000.0;
                            let ges = (fertig_host - t_cap as i64) as f32 / 1000.0;
                            // Gleitender Mittelwert, damit einzelne Ausreisser
                            // die Anzeige nicht springen lassen.
                            let a = 0.1;
                            mittel = Some(match mittel {
                                None => (enc, net, dec, ges),
                                Some((e, n, d, g)) => (
                                    e + (enc - e) * a,
                                    n + (net - n) * a,
                                    d + (dec - d) * a,
                                    g + (ges - g) * a,
                                ),
                            });
                            if let Some((e, n, d, g)) = mittel {
                                lat.encoder_ms = e;
                                lat.leitung_ms = n;
                                lat.decoder_ms = d;
                                lat.gesamt_ms = g;
                                lat.bilder = lat.bilder.saturating_add(1);
                                s.clock = Some(lat);
                            }
                        }
                    }
                }
                // Ein Hardware-Decoder, der Bilder in einem Format liefert,
                // das to_rgb nicht kennt, zeigt ein dunkles Bild mit Meldung.
                // Bleibt es dabei, ist Software mit einem lesbaren Format die
                // bessere Wahl - der Grund nennt das Format, damit es in den
                // Client eingebaut werden kann.
                if bau.pfad.hardware() && bau.format_fehler >= DECODER_FORMAT_FEHLER {
                    let format = letztes_format.map(|f| format!("{f:?}")).unwrap_or_default();
                    let grund = format!("{} ({}) liefert das Format {format}, das der Client nicht wandeln kann", bau.pfad.name(), bau.codec);
                    bau = auf_software(bedarf, grund).map_err(kein_decoder)?;
                    decoder_melden(shared, &bau, wunsch);
                    ring.clear();
                    mittel = None;
                    warte_auf_schluesselbild = true;
                }
            }
            MSG_TIME => {
                // Der Host hat unsere Uhrzeit zurueckgegeben und seine angehaengt.
                // Versatz und Umlaufzeit wie beim Zeitabgleich im Netz ueblich.
                if len >= 16 {
                    let t1 = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                    let t2 = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                    let t3 = client_us();
                    if t3 >= t1 {
                        let umlauf = t3 - t1;
                        let versatz = t2 as i64 - ((t1 + t3) / 2) as i64;
                        umlaeufe.push((umlauf, versatz));
                        if umlaeufe.len() > 32 {
                            umlaeufe.remove(0);
                        }
                        // Die Probe mit der kuerzesten Umlaufzeit ist die
                        // ehrlichste: dort steckt am wenigsten Warterei drin,
                        // die sich ungleich auf Hin- und Rueckweg verteilt.
                        if let Some(&(u, v)) = umlaeufe.iter().min_by_key(|(u, _)| *u) {
                            lat.versatz_us = v;
                            lat.umlauf_ms = u as f32 / 1000.0;
                            shared.lock().unwrap().clock = Some(lat);
                        }
                    }
                }
            }
            MSG_STAMP => {
                if len >= 24 {
                    let seq = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as u16;
                    let wiederholt = payload[4] & 1 != 0;
                    let t_cap = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                    let t_enc = u64::from_le_bytes(payload[16..24].try_into().unwrap());
                    ring.push_back((seq, t_cap, t_enc, wiederholt, 0));
                    while ring.len() > 128 {
                        ring.pop_front();
                    }
                }
            }
            MSG_LAST => {
                if len >= 26 && payload[0] == 1 {
                    let u16le = |o: usize| u16::from_le_bytes([payload[o], payload[o + 1]]);
                    let u32le = |o: usize| u32::from_le_bytes([payload[o], payload[o + 1], payload[o + 2], payload[o + 3]]);
                    let gpu_roh = u16le(6);
                    let l = HostLast {
                        cpu: u16le(2) as f32 / 10.0,
                        cpu_eigen: u16le(4) as f32 / 10.0,
                        // 0xffff heisst "nicht lesbar" - dann zeigen wir nichts
                        // an, statt eine Null zu behaupten.
                        gpu: if gpu_roh == 0xffff { None } else { Some(gpu_roh as f32 / 10.0) },
                        druck: u16le(8),
                        ram_benutzt_mb: u32le(10),
                        ram_gesamt_mb: u32le(14),
                        eigen_mb: u32le(18),
                        encoder_ms: u16le(22) as f32 / 10.0,
                        host_fps: u16le(24) as f32 / 10.0,
                    };
                    shared.lock().unwrap().hostlast = Some(l);
                }
            }
            MSG_SETTINGS => {
                if len >= 8 {
                    let mbit = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let fps = u16::from_le_bytes([payload[4], payload[5]]);
                    // Neuntes Byte: Ton. Ein aelterer Host schickt acht - dann gilt "an".
                    let ton = if len >= 9 { payload[8] != 0 } else { true };
                    shared.lock().unwrap().settings = Some((mbit, fps, payload[6] != 0, payload[7] != 0, ton));
                }
            }
            MSG_AUDIO_INFO => {
                #[cfg(any(windows, target_os = "macos"))]
                if len >= 8 {
                    let rate = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let ch = payload[4] as u16;
                    // Beide Hosts sagen das Format auch mitten in der Sitzung
                    // neu an. Der alte Ausgang rechnet mit dem alten Format um:
                    // zuerst freigeben - scheitert der Neubau, bleibt es lieber
                    // still als falsch.
                    sound = None;
                    // Der Satz kommt aus der Tabelle, ohne Anhang: der Grund
                    // ist eigener deutscher Text und steht im Protokoll.
                    let meldung = |e: &str| Meldung::neu(strings::Key::ErrorSound, format!("Ton: {e}"));
                    match audio::AudioOut::new(rate, ch) {
                        Ok((a, hinweis)) => {
                            sound = Some(a);
                            // Noch kein Geraet, der Tonfaden sucht weiter und
                            // hat den Grund schon protokolliert.
                            if let Some(e) = hinweis {
                                shared.lock().unwrap().error = Some(meldung(&e));
                                ton_fehler = true;
                            }
                        }
                        Err(e) => {
                            let m = meldung(&e);
                            protokoll::zeile(m.protokoll.clone());
                            shared.lock().unwrap().error = Some(m);
                        }
                    }
                }
            }
            MSG_AUDIO => {
                #[cfg(any(windows, target_os = "macos"))]
                if ton_fehler && sound.as_ref().is_some_and(|a| a.offen()) {
                    ton_fehler = false;
                    let mut s = shared.lock().unwrap();
                    if s.error.as_ref().is_some_and(|m| m.key == strings::Key::ErrorSound) {
                        s.error = None;
                    }
                }
                #[cfg(any(windows, target_os = "macos"))]
                if let Some(a) = sound.as_ref().filter(|_| shared.lock().unwrap().ton) {
                    // Der Empfangspuffer ist nicht ausgerichtet, deshalb Wert fuer Wert.
                    pcm.clear();
                    pcm.reserve(len / 4);
                    for c in payload.chunks_exact(4) {
                        pcm.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
                    }
                    a.push(&pcm);
                }
            }
            MSG_CLIP => {
                #[cfg(any(windows, target_os = "macos"))]
                if let Ok(text) = std::str::from_utf8(&payload) {
                    ablage_setzen(text.to_owned());
                }
            }
            MSG_CURSOR => {
                if len >= 12 {
                    let w = u16::from_le_bytes([payload[0], payload[1]]);
                    let h = u16::from_le_bytes([payload[2], payload[3]]);
                    let hx = u16::from_le_bytes([payload[4], payload[5]]);
                    let hy = u16::from_le_bytes([payload[6], payload[7]]);
                    let sichtbar = payload[8] != 0;
                    let n = w as usize * h as usize * 4;
                    // Nur, was zusammenpasst: Groesse plausibel, Bild vollstaendig,
                    // Hotspot im Bild. Alles andere ist kein Zeiger.
                    if (1..=256).contains(&w) && (1..=256).contains(&h) && len == 12 + n && hx < w && hy < h {
                        if !zeiger_gemeldet {
                            zeiger_gemeldet = true;
                            protokoll::zeile(format!("Zeigerform vom Host: {w}x{h}, Hotspot {hx},{hy}, sichtbar {sichtbar}"));
                        }
                        let mut s = shared.lock().unwrap();
                        s.zeiger = Some(ZeigerForm { w, h, hx, hy, sichtbar, rgba: payload[12..].to_vec() });
                        s.zeiger_seq = s.zeiger_seq.wrapping_add(1);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Text vom Host in die Zwischenablage - in einem eigenen Faden, nie im
/// Empfangsfaden: set() wartet auf die Sperre des Waechters (Mac) bzw. auf
/// die Ablage selbst (Windows: OpenClipboard mit Wiederholungen), und
/// solange lage der Bildkanal still - nach der 2-s-Stauregel der Hosts
/// floege der Zuschauer hinaus. Kommen mehrere Texte, waehrend einer
/// abgelegt wird, zaehlt nur der neueste.
#[cfg(any(windows, target_os = "macos"))]
fn ablage_setzen(text: String) {
    use std::sync::mpsc;
    static FADEN: std::sync::OnceLock<mpsc::Sender<String>> = std::sync::OnceLock::new();
    let tx = FADEN.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            while let Ok(mut t) = rx.recv() {
                while let Ok(neuer) = rx.try_recv() {
                    t = neuer;
                }
                clipboard::set(&t);
            }
        });
        tx
    });
    let _ = tx.send(text);
}

/// Ein Paket in den Decoder und alle fertigen Bilder heraus. Gibt den
/// ersten Fehler zurueck, der kein blosses "gerade nichts da" ist - der
/// Aufrufer entscheidet, ob das den Decoder disqualifiziert.
///
/// Ein EAGAIN beim Senden heisst bei den cuvid-Decodern: erst Bilder
/// abholen, dann noch einmal senden. Das Paket geht dabei nicht verloren.
fn decoder_fuettern(
    decoder: &mut ffmpeg::decoder::Video,
    packet: &ffmpeg::Packet,
    bilder: &mut Vec<ffmpeg::frame::Video>,
) -> Option<ffmpeg::Error> {
    let mut fehler = None;
    let mut abholen = |decoder: &mut ffmpeg::decoder::Video, bilder: &mut Vec<ffmpeg::frame::Video>| loop {
        let mut f = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut f) {
            Ok(()) => bilder.push(f),
            Err(e) => {
                if decoder_defekt(&e) {
                    fehler = Some(e);
                }
                break;
            }
        }
    };
    match decoder.send_packet(packet) {
        Ok(()) => {}
        Err(ffmpeg::Error::Other { errno: ffmpeg::util::error::EAGAIN }) => {
            abholen(decoder, bilder);
            if let Err(e) = decoder.send_packet(packet) {
                return Some(e);
            }
        }
        Err(e) => return Some(e),
    }
    abholen(decoder, bilder);
    fehler
}

/// YUV nach RGB, BT.709, voller Wertebereich.
///
/// Ganzzahlig in 16.16-Festkomma statt mit Kommazahlen, und zeilenweise auf
/// alle Kerne verteilt. Die Zeilen werden einmal als Ausschnitt geholt, damit
/// im inneren Teil keine Bereichspruefung mehr anfaellt.
#[inline(always)]
fn clamp8(v: i32) -> u32 {
    if v < 0 { 0 } else if v > 255 { 255 } else { v as u32 }
}

/// Eine Zeile umrechnen. SUB = 4:2:0 (je zwei Bildpunkte teilen sich einen
/// Farbwert), BITS = Breite der Werte: 8 (ein Byte), 10 (16 Bit LE, Wert in
/// den unteren zehn Bit, wie die Software-Decoder ihn liefern) oder 16 (16
/// Bit LE, Wert oben buendig, wie NVDEC ihn liefert - P010/P012/P016,
/// YUV444P16 und YUV444P10MSB/P12MSB). PAAR = Farbwerte als U/V-Paare in
/// EINER Ebene (NV12, P010, P012, P016); `ur` und `vr` zeigen dann in
/// dieselbe Ebene, `vr` um einen Wert versetzt. Als Konstanten, damit der
/// Compiler je Format eine eigene, verzweigungsfreie Schleife baut.
///
/// Die Anzeige braucht acht Bit: bei 10 Bit sind das die Bits 9..2, bei den
/// oben buendigen 16-Bit-Werten die Bits 15..8 - fuer 10-Bit-Inhalt (Wert
/// << 6) ist das dieselbe Zahl, nur ohne den Umweg ueber >> 6 und >> 2.
///
/// Bei 4:2:0 wird der naechstgelegene Farbwert genommen (Wiederholung) -
/// einfach und schnell. Eine weichere Farbaufwertung ist eine spaetere
/// Verfeinerung; sie aendert am Vergleich 4:4:4 gegen 4:2:0 nichts Wesentliches.
#[inline(always)]
fn zeile_rgb<const SUB: bool, const BITS: u8, const PAAR: bool>(out: &mut [u32], yr: &[u8], ur: &[u8], vr: &[u8]) {
    const CR_R: i32 = 103206; // 1.5748
    const CB_G: i32 = 12276;  // 0.1873
    const CR_G: i32 = 30681;  // 0.4681
    const CB_B: i32 = 121609; // 1.8556

    #[inline(always)]
    fn wert<const BITS: u8>(p: &[u8], i: usize) -> i32 {
        match BITS {
            8 => p[i] as i32,
            10 => (u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) >> 2) as i32,
            _ => (u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) >> 8) as i32,
        }
    }

    for (x, o) in out.iter_mut().enumerate() {
        let cx = if SUB { x >> 1 } else { x };
        // Bei Paaren liegt der U-Wert von Farbspalte cx an Stelle 2*cx, der
        // V-Wert direkt dahinter - `vr` ist schon um einen Wert versetzt.
        let ci = if PAAR { cx * 2 } else { cx };
        let y = wert::<BITS>(yr, x);
        let cb = wert::<BITS>(ur, ci) - 128;
        let cr = wert::<BITS>(vr, ci) - 128;
        let r = y + ((CR_R * cr) >> 16);
        let g = y - ((CB_G * cb + CR_G * cr) >> 16);
        let b = y + ((CB_B * cb) >> 16);
        *o = (clamp8(r) << 16) | (clamp8(g) << 8) | clamp8(b);
    }
}

/// Aufbau der Ebenen eines Decoderformats. EINE Tabelle fuer `to_rgb` und -
/// sobald die Karte umrechnet - fuer deren Texturen, damit beide dasselbe
/// Format auf dieselbe Weise lesen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EbenenFormat {
    /// 4:2:0: je zwei Bildpunkte in beiden Richtungen teilen sich einen Farbwert.
    pub sub: bool,
    /// Breite der Werte: 8 (ein Byte), 10 (16 Bit LE, Wert in den unteren
    /// zehn Bit) oder 16 (16 Bit LE, Wert oben buendig).
    pub bits: u8,
    /// U und V als Paare in EINER Ebene (NV12, P010, P012, P016).
    pub paar: bool,
}

impl EbenenFormat {
    /// Bytes je Wert in den Ebenen.
    pub fn bpp(&self) -> u32 {
        if self.bits == 8 { 1 } else { 2 }
    }

    /// Verschiebung, mit der aus einem Wert die acht Anzeigebits werden -
    /// dieselbe wie in `wert::<BITS>`: 8 Bit keine, 10 Bit LE zwei, oben
    /// buendige 16 Bit acht. Die Karte rechnet damit, `zeile_rgb` ueber
    /// die Konstante.
    pub fn schieb(&self) -> u32 {
        match self.bits { 8 => 0, 10 => 2, _ => 8 }
    }
}

/// Welche Decoderformate der Client lesen kann, und wie ihre Ebenen liegen.
///
/// Erkannt werden die planaren Formate der Software-Decoder (YUV444P,
/// YUV444P10LE, YUV420P, YUV420P10LE, dazu YUVJ420P/YUVJ444P - FFmpegs
/// alte Schreibweise fuer 8 Bit im vollen Wertebereich, die der
/// H.264-Decoder liefert) und die von NVDEC/cuvid: YUV444P
/// (4:4:4 8 Bit), YUV444P10MSB/YUV444P12MSB (4:4:4 10/12 Bit, oben buendig -
/// so nennt FFmpeg 9 die Formate, die aeltere Fassungen als YUV444P16
/// meldeten; das bleibt fuer die mit dabei), NV12 (4:2:0 8 Bit, U/V
/// verschraenkt) und P010LE/P012LE/P016LE (4:2:0 10/12/16 Bit, oben
/// buendig, verschraenkt). Alles andere: None.
pub fn ebenen_format(p: ffmpeg::format::Pixel) -> Option<EbenenFormat> {
    use ffmpeg::format::Pixel;
    // (4:2:0, Bits je Wert, U/V als Paare in einer Ebene)
    let (sub, bits, paar) = match p {
        // Die J-Formate sind FFmpegs alte Schreibweise fuer "voller
        // Wertebereich" - der H.264-Decoder liefert 8 Bit so, 4:2:0 wie 4:4:4. Die
        // Ebenen sind dieselben, und Vollbereich ist ohnehin, was wir
        // annehmen.
        Pixel::YUV444P | Pixel::YUVJ444P => (false, 8u8, false),
        Pixel::YUV444P10LE => (false, 10, false),
        // Die MSB-Formate legen den Wert oben buendig in 16 Bit ab - genau
        // wie YUV444P16, deshalb derselbe Lesepfad (Bits 15..8).
        Pixel::YUV444P16
        | Pixel::YUV444P16LE
        | Pixel::YUV444P10MSB
        | Pixel::YUV444P10MSBLE
        | Pixel::YUV444P12MSB
        | Pixel::YUV444P12MSBLE => (false, 16, false),
        Pixel::YUV420P | Pixel::YUVJ420P => (true, 8, false),
        Pixel::YUV420P10LE => (true, 10, false),
        Pixel::NV12 => (true, 8, true),
        Pixel::P010LE | Pixel::P012LE | Pixel::P016LE => (true, 16, true),
        _ => return None,
    };
    Some(EbenenFormat { sub, bits, paar })
}

/// Decodiertes Bild nach RGB. Das Format kommt aus dem Frame selbst
/// (Pixelformat des Decoders), nicht aus einer Flagge: nach einem
/// Codecwechsel waere jede Flagge fuer ein paar Bilder falsch, und der
/// Hardware-Decoder liefert andere Formate als der Software-Decoder.
///
/// Welche Formate gelesen werden, sagt `ebenen_format`; alles andere ist
/// ein Fehler mit Meldung, kein Absturz. Alle Stroeme sind Vollbereich (der
/// Host garantiert das), deshalb keine Bereichsdehnung.
fn to_rgb(src: &ffmpeg::frame::Video) -> Result<Frame, String> {
    let Some(fmt) = ebenen_format(src.format()) else {
        return Err(format!("Unbekanntes Bildformat vom Decoder: {:?}", src.format()));
    };
    let (sub, bits, paar) = (fmt.sub, fmt.bits, fmt.paar);
    let w = src.width() as usize;
    let h = src.height() as usize;
    if w == 0 || h == 0 {
        return Err("Decoder liefert ein leeres Bild".into());
    }
    if src.planes() < if paar { 2 } else { 3 } {
        return Err("Decoder liefert zu wenige Bildebenen".into());
    }
    // Breite der Farbebenen: bei 4:2:0 die Haelfte, aufgerundet. Bei Paaren
    // liegen U und V nebeneinander, die Zeile ist also doppelt so breit.
    let cw = if sub { (w + 1) / 2 } else { w };
    let ch = if sub { (h + 1) / 2 } else { h };
    let bpp = fmt.bpp() as usize;
    let cbreite = if paar { cw * 2 * bpp } else { cw * bpp };
    let yp = src.data(0);
    let up = src.data(1);
    let vp = if paar { up } else { src.data(2) };
    let ys = src.stride(0);
    let us = src.stride(1);
    let vs = if paar { us } else { src.stride(2) };
    // Reichen die Ebenen fuer das, was gleich gelesen wird? Sonst waere das
    // Zerlegen unten ein Absturz mitten im Empfangsfaden.
    if yp.len() < (h - 1) * ys + w * bpp
        || up.len() < (ch - 1) * us + cbreite
        || vp.len() < (ch - 1) * vs + cbreite
    {
        return Err("Bildebenen des Decoders sind zu klein".into());
    }

    let mut pixels = vec![0u32; w * h];
    pixels.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let crow = if sub { row >> 1 } else { row };
        let yr = &yp[row * ys..row * ys + w * bpp];
        let ur = &up[crow * us..crow * us + cbreite];
        // Bei Paaren: dieselbe Zeile, um einen Wert versetzt - der letzte
        // gelesene V-Wert liegt dann genau am Ende der Zeile, nie dahinter.
        let vr = if paar { &up[crow * us + bpp..crow * us + cbreite] } else { &vp[crow * vs..crow * vs + cbreite] };
        match (sub, bits, paar) {
            (false, 8, false) => zeile_rgb::<false, 8, false>(out, yr, ur, vr),
            (false, 10, false) => zeile_rgb::<false, 10, false>(out, yr, ur, vr),
            (false, _, false) => zeile_rgb::<false, 16, false>(out, yr, ur, vr),
            (true, 8, false) => zeile_rgb::<true, 8, false>(out, yr, ur, vr),
            (true, 10, false) => zeile_rgb::<true, 10, false>(out, yr, ur, vr),
            (true, 8, true) => zeile_rgb::<true, 8, true>(out, yr, ur, vr),
            (true, _, true) => zeile_rgb::<true, 16, true>(out, yr, ur, vr),
            // 4:4:4 mit Paaren und 4:2:0 planar 16 Bit erzeugt die Tabelle
            // oben nie; der Arm steht nur fuer die Vollstaendigkeit.
            _ => {}
        }
    });

    Ok(Frame { width: w as u32, height: h as u32, pixels, bereit_us: 0 })
}

/// Flaches dunkles Bild in Fenstergrundfarbe - was gezeigt wird, wenn das
/// Decoderformat nicht verstanden wurde. Besser als ein eingefrorenes altes
/// Bild, das so tut, als waere alles in Ordnung.
fn dunkles_bild(w: u32, h: u32) -> Frame {
    let (w, h) = (w.max(1), h.max(1));
    Frame { width: w, height: h, pixels: vec![ui::BG; (w * h) as usize], bereit_us: 0 }
}

// ------------------------------------------------------------------ Eingabe

/// Physische Taste nach macOS-Tastencode. Bewusst ueber die POSITION der Taste,
/// nicht ueber das Zeichen: So bleibt jedes Tastaturlayout richtig, weil der Mac
/// sein eigenes Layout darauf anwendet. Umlaute und AltGr funktionieren damit
/// ohne Sonderbehandlung.
fn mac_keycode(code: winit::keyboard::KeyCode) -> Option<u16> {
    use winit::keyboard::KeyCode as K;
    Some(match code {
        K::KeyA => 0, K::KeyS => 1, K::KeyD => 2, K::KeyF => 3, K::KeyH => 4, K::KeyG => 5,
        K::KeyZ => 6, K::KeyX => 7, K::KeyC => 8, K::KeyV => 9, K::KeyB => 11, K::KeyQ => 12,
        K::KeyW => 13, K::KeyE => 14, K::KeyR => 15, K::KeyY => 16, K::KeyT => 17,
        K::Digit1 => 18, K::Digit2 => 19, K::Digit3 => 20, K::Digit4 => 21, K::Digit6 => 22,
        K::Digit5 => 23, K::Equal => 24, K::Digit9 => 25, K::Digit7 => 26, K::Minus => 27,
        K::Digit8 => 28, K::Digit0 => 29, K::BracketRight => 30, K::KeyO => 31, K::KeyU => 32,
        K::BracketLeft => 33, K::KeyI => 34, K::KeyP => 35, K::Enter => 36, K::KeyL => 37,
        K::KeyJ => 38, K::Quote => 39, K::KeyK => 40, K::Semicolon => 41, K::Backslash => 42,
        K::Comma => 43, K::Slash => 44, K::KeyN => 45, K::KeyM => 46, K::Period => 47,
        K::Tab => 48, K::Space => 49, K::Backquote => 50, K::Backspace => 51, K::Escape => 53,
        // Die Extrataste neben der linken Umschalttaste auf deutschen Tastaturen
        K::IntlBackslash => 10,
        K::SuperLeft => 55, K::ShiftLeft => 56, K::CapsLock => 57, K::AltLeft => 58,
        K::ControlLeft => 59, K::ShiftRight => 60, K::AltRight => 61, K::ControlRight => 62,
        K::SuperRight => 54,
        K::NumpadDecimal => 65, K::NumpadMultiply => 67, K::NumpadAdd => 69,
        K::NumpadDivide => 75, K::NumpadEnter => 76, K::NumpadSubtract => 78,
        K::Numpad0 => 82, K::Numpad1 => 83, K::Numpad2 => 84, K::Numpad3 => 85, K::Numpad4 => 86,
        K::Numpad5 => 87, K::Numpad6 => 88, K::Numpad7 => 89, K::Numpad8 => 91, K::Numpad9 => 92,
        K::F1 => 122, K::F2 => 120, K::F3 => 99, K::F4 => 118, K::F5 => 96, K::F6 => 97,
        K::F7 => 98, K::F8 => 100, K::F9 => 101, K::F10 => 109, K::F11 => 103, K::F12 => 111,
        K::Home => 115, K::PageUp => 116, K::Delete => 117, K::End => 119, K::PageDown => 121,
        K::ArrowLeft => 123, K::ArrowRight => 124, K::ArrowDown => 125, K::ArrowUp => 126,
        _ => return None,
    })
}

/// Zweite, eigene Verbindung nur fuer Maus und Tastatur. Klein, dringend,
/// niemals hinter einem Bild in der Warteschlange.
///
/// Wer sendet, haelt die Sperre um diesen Kanal: der Fensterfaden bei jeder
/// Maus- und Tastennachricht und alle 2 ms, der Empfangsfaden fuer den
/// Zeitabgleich, der Faden der Zwischenablage. Deshalb wartet hier nichts
/// auf das Netz. Der Aufbau (Namensaufloesung, Verbindung, Handschlag)
/// laeuft in einem eigenen Faden; geschrieben wird in einem Schreibfaden,
/// der die Leitung besitzt, und `send` reiht nur ein. Sonst stuende bei
/// einem Eingabeport, der Verbindungen still verwirft, jedes Mal die ganze
/// Oberflaeche zwei Sekunden, und 4 MB Zwischenablage ueber langsames WLAN
/// hielten Fenster und Bildempfang an. Ein einziger Schreibfaden je
/// Leitung haelt die Reihenfolge - und damit die Folge der Nonces.
struct InputLink {
    /// Der stehende Kanal: Nachrichten gehen an seinen Schreibfaden.
    kanal: Option<Schreiber>,
    addr: String,
    last: (f32, f32),
    /// Eingereihte Nachrichten.
    sent: u64,
    /// Pruefsumme des Bildkanals und Schluessel seines Hosts. Ohne die
    /// Pruefsumme laesst der Host diesen Kanal nicht zu; der Schluessel muss
    /// zu dem passen, der am Eingabekanal antwortet.
    link: Option<(Vec<u8>, Vec<u8>)>,
    /// Welche Tasten gerade als gedrueckt gelten. Ohne diese Liste bleiben
    /// beim Fokusverlust Tasten auf dem Mac haengen - WASD laeuft dann gegen
    /// die Wand, und ein haengendes Strg macht aus jedem Klick einen Rechtsklick.
    gedrueckt: std::collections::HashSet<u16>,
    /// Wann zuletzt ein Verbindungsversuch zu Ende ging. Bremst die
    /// Wiederholungen auf einen je Sekunde - gezaehlt ab dem Ende, nicht ab
    /// dem Anfang: ein Versuch, der zwei Sekunden auf die Frist wartet, liesse
    /// sonst den naechsten gleich folgen.
    letzter_versuch: Option<Instant>,
    /// Wann zuletzt ein fremder Schluessel am Eingabeport im Protokoll stand -
    /// hoechstens alle zehn Sekunden eine Zeile, nicht eine je Versuch.
    fremd_gemeldet: Option<Instant>,
    /// Laufender Aufbau: sein Ergebnis kommt ueber diesen Kanal. Faellt er
    /// weg (neue Adresse, neue Bindung, Trennen), schliesst der Aufbaufaden
    /// eine fertige Leitung selbst wieder.
    aufbau: Option<std::sync::mpsc::Receiver<Aufbau>>,
}

/// Ergebnis eines Aufbaus: die Leitung oder der Fehler, dazu der
/// Fingerabdruck, falls am Eingabeport ein fremder Schluessel antwortete.
type Aufbau = (Result<secure::Secure, secure::Fehler>, Option<String>);

/// Schreibseite eines stehenden Eingabekanals.
struct Schreiber {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    /// Gesetzt, sobald der Schreibfaden an der Leitung gescheitert ist.
    kaputt: Arc<std::sync::atomic::AtomicBool>,
}

impl Schreiber {
    /// Uebernimmt die Leitung und startet ihren Schreibfaden. Er endet, wenn
    /// das Schreiben scheitert - oder wenn der Schreiber fallen gelassen ist
    /// und die Warteschlange leer: was schon eingereiht war, geht noch
    /// hinaus (etwa das Loslassen aller Tasten beim Trennen).
    fn neu(mut sock: secure::Secure) -> Schreiber {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let kaputt = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let k = kaputt.clone();
        std::thread::spawn(move || {
            while let Ok(b) = rx.recv() {
                if sock.write_all(&b).is_err() {
                    k.store(true, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
            }
        });
        Schreiber { tx, kaputt }
    }

    fn steht(&self) -> bool {
        !self.kaputt.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl InputLink {
    fn new(addr: String) -> Self {
        Self {
            kanal: None,
            addr,
            last: (0.5, 0.5),
            sent: 0,
            link: None,
            gedrueckt: std::collections::HashSet::new(),
            letzter_versuch: None,
            fremd_gemeldet: None,
            aufbau: None,
        }
    }

    /// Adresse wechseln, etwa wenn ein anderer Host gewaehlt wurde.
    fn set_addr(&mut self, addr: String) {
        if addr != self.addr {
            self.addr = addr;
            self.kanal = None;
            self.aufbau = None;
        }
    }

    /// Bindung an den Bildkanal setzen. Wechselt sie, wird neu verbunden.
    fn set_link(&mut self, link: Option<(Vec<u8>, Vec<u8>)>) {
        if self.link != link {
            self.link = link;
            self.kanal = None;
            self.aufbau = None;
        }
    }

    /// Steht der Kanal? Nur dann kommt an, was jetzt gesendet wird.
    fn steht(&self) -> bool {
        self.kanal.as_ref().is_some_and(Schreiber::steht)
    }

    /// Sorgt dafuer, dass der Kanal steht oder im Aufbau ist - ohne je zu
    /// warten. Ein fertiger Aufbau wird hier abgeholt.
    fn ensure(&mut self) {
        // Ist der Schreibfaden gescheitert, gleich neu aufbauen - wie frueher
        // nach einem Schreibfehler.
        if self.kanal.as_ref().is_some_and(|k| !k.steht()) {
            self.kanal = None;
        }
        if self.kanal.is_some() || self.addr.is_empty() || self.link.is_none() {
            return;
        }
        if let Some(rx) = &self.aufbau {
            let (ergebnis, fremd) = match rx.try_recv() {
                Ok(a) => a,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => (
                    Err(secure::Fehler::Handschlag { grund: "Aufbaufaden beendet".into(), frist: false, system: None }),
                    None,
                ),
            };
            self.aufbau = None;
            match ergebnis {
                Ok(s) => {
                    self.kanal = Some(Schreiber::neu(s));
                    self.letzter_versuch = None;
                }
                Err(_) => {
                    self.letzter_versuch = Some(Instant::now());
                    if let Some(fp) = fremd {
                        if self.fremd_gemeldet.map(|t| t.elapsed() >= Duration::from_secs(10)).unwrap_or(true) {
                            self.fremd_gemeldet = Some(Instant::now());
                            protokoll::zeile(format!(
                                "Eingabekanal: Gegenstelle {fp} ist nicht der Host des Bildkanals - vor Nachricht 3 abgebrochen"
                            ));
                        }
                    }
                }
            }
            return;
        }
        if let Some(t) = self.letzter_versuch {
            if t.elapsed() < Duration::from_secs(1) {
                return;
            }
        }
        let Some((hh, host)) = self.link.clone() else { return };
        self.letzter_versuch = Some(Instant::now());
        // Die Pruefsumme im Prologue weist nur die Sitzung aus, nicht den
        // Host: wer den Bild-Handschlag mitgelesen hat, kann sie ausrechnen.
        // Also muss der Schluessel am anderen Ende derselbe sein wie beim
        // Bildkanal - sonst gingen Tasten und Zwischenablage an einen
        // Fremden. Geprueft wird im Handschlag vor Nachricht 3: ein Fremder
        // bekommt nicht einmal den eigenen Schluessel zu sehen, und er wird
        // auch nicht als neuer Host gemerkt.
        let (tx, rx) = std::sync::mpsc::channel::<Aufbau>();
        let addr = self.addr.clone();
        let prologue = noise::prologue_input(&hh);
        std::thread::spawn(move || {
            let mut fremd = None;
            let r = secure::Secure::connect_pruefend(&addr, &prologue, |k| {
                if k == host.as_slice() {
                    return Ok(());
                }
                fremd = Some(noise::fingerprint(k));
                Err(secure::Fehler::Handschlag {
                    grund: "Gegenstelle ist nicht der Host des Bildkanals".into(),
                    frist: false,
                    system: None,
                })
            });
            // Hoert niemand mehr zu, faellt die Leitung hier mit `r` zu.
            let _ = tx.send((r, fremd));
        });
        self.aufbau = Some(rx);
    }

    /// Eine Nachricht einreihen. Steht der Kanal nicht, faellt sie weg - wie
    /// bisher bei einem Kanal, der nicht aufging.
    fn send(&mut self, t: u8, payload: &[u8]) {
        self.ensure();
        let Some(k) = self.kanal.as_ref().filter(|k| k.steht()) else { return };
        let mut buf = Vec::with_capacity(8 + payload.len());
        buf.push(t);
        buf.push(0);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        let eingereiht = k.tx.send(buf).is_ok();
        if eingereiht {
            self.sent += 1;
        } else {
            self.kanal = None;
        }
    }

    /// Frage zum Zeitabgleich. Die Antwort kommt ueber den Bildkanal zurueck.
    fn zeitfrage(&mut self, t1: u64) {
        self.send(IN_TIME, &t1.to_le_bytes());
    }

    /// Wunsch an den Host: Bitrate, Bildrate, Spielmodus, feste Bildrate, Ton.
    fn settings(&mut self, mbit: u32, fps: u16, gaming: bool, fixed: bool, ton: bool) {
        let mut p = [0u8; 9];
        p[0..4].copy_from_slice(&mbit.to_le_bytes());
        p[4..6].copy_from_slice(&fps.to_le_bytes());
        p[6] = gaming as u8;
        p[7] = fixed as u8;
        p[8] = ton as u8;
        self.send(IN_SETTINGS, &p);
    }

    /// Wunsch an den Host: auf diesen Kandidaten der Koennensliste wechseln.
    /// Die Antwort kommt ueber den Bildkanal als Nachricht 7.
    fn codec(&mut self, idx: u8) {
        self.send(IN_CODEC, &[idx]);
    }

    /// Wunsch an den Host: Testbild an oder aus. Ein Host, der die
    /// Nachricht nicht kennt, uebergeht sie - dann laeuft der Benchmark
    /// eben auf dem Bildschirminhalt.
    fn testbild(&mut self, an: bool) {
        self.send(IN_TESTBILD, &[an as u8]);
    }

    fn mouse_move(&mut self, nx: f32, ny: f32) {
        self.last = (nx, ny);
        let mut p = [0u8; 8];
        p[0..4].copy_from_slice(&nx.to_le_bytes());
        p[4..8].copy_from_slice(&ny.to_le_bytes());
        self.send(IN_MOVE, &p);
    }

    fn mouse_button(&mut self, button: u8, down: bool) {
        let mut p = [0u8; 12];
        p[0] = button;
        p[1] = down as u8;
        p[4..8].copy_from_slice(&self.last.0.to_le_bytes());
        p[8..12].copy_from_slice(&self.last.1.to_le_bytes());
        self.send(IN_BUTTON, &p);
    }

    fn key(&mut self, keycode: u16, down: bool, mods: u32) {
        if down {
            self.gedrueckt.insert(keycode);
        } else {
            self.gedrueckt.remove(&keycode);
        }
        let mut p = [0u8; 8];
        p[0..2].copy_from_slice(&keycode.to_le_bytes());
        p[2] = down as u8;
        p[4..8].copy_from_slice(&mods.to_le_bytes());
        self.send(IN_KEY, &p);
    }

    /// Alles loslassen, was noch als gedrueckt gilt. Wird aufgerufen, wenn das
    /// Fenster den Fokus verliert oder das Menue aufgeht - sonst bleiben die
    /// Tasten drueben haengen, und wir bekommen davon gar nichts mit.
    /// Eingabekanal schliessen. Der naechste Sendeversuch baut ihn neu auf.
    /// Was schon eingereiht ist, schreibt der Schreibfaden noch.
    fn trennen(&mut self) {
        self.kanal = None;
        self.aufbau = None;
        self.letzter_versuch = None;
    }

    fn alle_loslassen(&mut self) {
        let offen: Vec<u16> = self.gedrueckt.iter().copied().collect();
        for k in offen {
            let mut p = [0u8; 8];
            p[0..2].copy_from_slice(&k.to_le_bytes());
            p[2] = 0;
            p[4..8].copy_from_slice(&0u32.to_le_bytes());
            self.send(IN_KEY, &p);
        }
        self.gedrueckt.clear();
        for b in 0..3u8 {
            let mut p = [0u8; 12];
            p[0] = b;
            p[1] = 0;
            p[4..8].copy_from_slice(&self.last.0.to_le_bytes());
            p[8..12].copy_from_slice(&self.last.1.to_le_bytes());
            self.send(IN_BUTTON, &p);
        }
    }

    fn scroll(&mut self, dx: f32, dy: f32) {
        let mut p = [0u8; 8];
        p[0..4].copy_from_slice(&dx.to_le_bytes());
        p[4..8].copy_from_slice(&dy.to_le_bytes());
        self.send(IN_SCROLL, &p);
    }
}

// ---------------------------------------------------------------- Benchmark
//
// Misst den Weg Host -> Leitung -> Client der Reihe nach fuer Kombinationen
// aus Codec, Bildrate und Datenrate und sagt am Ende, welche davon taugt.
// Der Ablauf ist ein Zustandsautomat, den die Fensterschleife (alle 2 ms)
// oder der Takt des Pruefmodus anstoesst - er blockiert nie, das Bild
// laeuft weiter, das Menue darf offen bleiben. Angezeigt und gewertet wird
// nie ein Wunsch, sondern was der Host bestaetigt hat.

/// Datenraten und Bildraten, die der Reiter anbietet.
const BENCH_MBITS: [u32; 5] = [10, 25, 50, 100, 150];
const BENCH_FPSS: [u16; 2] = [60, 120];
/// So lange darf ein Codecwechsel oder eine Einstellung auf sich warten
/// lassen; danach gilt der Schritt als gescheitert.
const BENCH_FRIST: Duration = Duration::from_secs(8);
/// Ruhe nach dem Einstellen, bevor gemessen wird: die gleitenden Mittel
/// der Latenz sollen den neuen Zustand zeigen, nicht den alten.
const BENCH_EINSCHWINGEN: Duration = Duration::from_secs(1);
/// Abstand der Proben waehrend der Messung.
const BENCH_PROBE: Duration = Duration::from_secs(1);
/// So lang darf die ganze Kette (Aufnahme bis Uebergabe an die Anzeige)
/// sein, damit ein Schritt besteht - absolut, nicht in Bildern: Latenz in
/// Bildern zu messen bestraft hohe Bildraten (anderthalb Bilder sind bei
/// 120 fps 12,5 ms, bei 60 fps 25 ms - dieselbe Kette bestuende also nur
/// bei der niedrigeren Rate, obwohl sie sich gleich anfuehlt). 30 ms sind
/// fuer eine Fernsteuerung unauffaellig, egal wie viele Bilder darin liegen.
const BENCH_KETTE_MAX_MS: f32 = 30.0;

/// Was der Benchmark durchprobieren soll. Steht im Reiter und gilt fuer den
/// naechsten Lauf.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchKonfig {
    /// Kandidaten der Koennensliste, die NICHT mitlaufen sollen. Leer
    /// heisst: alle, die der Host kann - so bleibt die Vorgabe richtig, auch
    /// wenn die Liste erst nach dem Verbinden kommt.
    pub codecs_aus: Vec<u8>,
    pub mbits: Vec<u32>,
    pub fpss: Vec<u16>,
    /// Messzeit je Schritt in Sekunden (3..15).
    pub dauer_s: u32,
    /// Testbild auf dem Host fuer die Dauer des Laufs.
    pub testbild: bool,
}

impl BenchKonfig {
    fn vorgabe(dauer_s: u32, testbild: bool) -> Self {
        BenchKonfig {
            codecs_aus: Vec::new(),
            mbits: BENCH_MBITS.to_vec(),
            fpss: BENCH_FPSS.to_vec(),
            dauer_s: dauer_s.clamp(3, 15),
            testbild,
        }
    }

    /// Auswahl aus der Befehlszeile (Pruefmodus): "codecs:mbit:fps", jeder
    /// Teil eine Liste mit Kommas, ein leerer Teil laesst die Vorgabe
    /// stehen - "0,1:50,100:120" heisst Kandidaten 0 und 1, 50 und 100
    /// Mbit/s, nur 120 Bilder. So bleibt ein Pruefungslauf kurz.
    fn einschraenken(&mut self, text: &str, codecs: &[CodecEintrag]) {
        let teile: Vec<&str> = text.split(':').collect();
        let liste = |i: usize| -> Vec<u32> {
            teile.get(i).map(|t| t.split(',').filter_map(|v| v.trim().parse().ok()).collect()).unwrap_or_default()
        };
        let gewollt = liste(0);
        if !gewollt.is_empty() {
            self.codecs_aus = codecs.iter().filter(|e| !gewollt.contains(&(e.idx as u32))).map(|e| e.idx).collect();
        }
        let mbits: Vec<u32> = liste(1).into_iter().map(|m| m.clamp(2, 500)).collect();
        if !mbits.is_empty() {
            self.mbits = mbits;
        }
        let fpss: Vec<u16> = liste(2).into_iter().map(|f| f.clamp(10, 240) as u16).collect();
        if !fpss.is_empty() {
            self.fpss = fpss;
        }
    }

    /// Die Schrittliste: Codecs x Bildraten x Datenraten, in dieser
    /// Reihenfolge - ein Codec bleibt stehen, waehrend seine Raten
    /// durchlaufen; der Wechsel ist der teuerste Schritt.
    fn schritte(&self, codecs: &[CodecEintrag]) -> Vec<BenchSchritt> {
        let mut aus = Vec::new();
        for e in codecs.iter().filter(|e| e.available && !self.codecs_aus.contains(&e.idx)) {
            for &fps in &self.fpss {
                for &mbit in &self.mbits {
                    aus.push(BenchSchritt { idx: e.idx, name: e.name.clone(), qualitaet: qualitaet(e), mbit, fps });
                }
            }
        }
        aus
    }
}

/// Rang der Farbqualitaet eines Kandidaten, fuer die Empfehlung:
/// HEVC 4:4:4 10 Bit (4) > 4:4:4 8 Bit (3) > 4:2:0 10 Bit (2) > 4:2:0 8 Bit (1)
/// > H.264 (0). AV1 wird wie HEVC nach Farbaufloesung und Bittiefe eingeordnet.
fn qualitaet(e: &CodecEintrag) -> u8 {
    if e.codec() == 2 { 0 } else { 1 + e.ten_bit as u8 + 2 * e.chroma444 as u8 }
}

#[derive(Clone, Debug)]
struct BenchSchritt {
    idx: u8,
    name: String,
    qualitaet: u8,
    mbit: u32,
    fps: u16,
}

impl BenchSchritt {
    /// "HEVC 4:4:4 10 Bit · 50 Mbit/s · 120", wie in der Fortschrittszeile.
    fn beschreibung(&self) -> String {
        format!("{} · {} Mbit/s · {}", self.name, self.mbit, self.fps)
    }
}

/// Was bei einem Schritt herauskam. Alle Zeiten in Millisekunden, Mittel
/// ueber die Proben der Messzeit.
#[derive(Clone, Debug, Default)]
pub struct Ergebnis {
    pub idx: u8,
    pub codec: String,
    pub qualitaet: u8,
    pub mbit: u32,
    pub fps: u16,
    /// Codecwechsel oder Einstellungen kamen nicht zustande.
    pub gescheitert: bool,
    pub fps_gemessen: f32,
    pub mbit_gemessen: f32,
    /// Alle Glieder: Aufnahme bis Uebergabe ans Fenster.
    pub kette_ms: f32,
    pub encoder_ms: f32,
    pub leitung_ms: f32,
    pub decoder_ms: f32,
    pub anzeige_ms: f32,
    pub empfangen: u64,
    pub verworfen: u64,
    pub ausgelassen: u64,
    /// Encoderzeit je Bild, wie der Host sie meldet, und sein Budget 1000/fps.
    pub host_encoder_ms: f32,
    pub budget_ms: f32,
    pub host_cpu: f32,
    pub client_cpu: f32,
    pub bestanden: bool,
}

impl Ergebnis {
    /// Die Regel, nach der ein Schritt besteht - dieselbe, die der Tooltip
    /// im Reiter nennt:
    ///   - mindestens 95 % der Zielbildrate kommen an,
    ///   - mit Anzeige: unter 1 % der empfangenen Bilder wurden verworfen
    ///     (ohne Fenster wird nichts gezeigt, also auch nichts verworfen),
    ///   - die Kette ist hoechstens BENCH_KETTE_MAX_MS lang - absolut,
    ///     unabhaengig von der Bildrate (siehe dort).
    /// Die Encoderzeit des Hosts ist KEIN Kriterium: sie steckt in der
    /// Kette schon drin und wird nur angezeigt. Ohne eine einzige
    /// Latenzprobe (Zeitabgleich stand nicht) kann der Schritt nicht
    /// bestehen - eine Null waere keine Messung.
    fn pruefen(&mut self, mit_anzeige: bool, hat_latenz: bool) {
        self.bestanden = !self.gescheitert
            && hat_latenz
            && self.fps_gemessen >= 0.95 * self.fps as f32
            && (!mit_anzeige || (self.verworfen as f32) < 0.01 * self.empfangen.max(1) as f32)
            && self.kette_ms <= BENCH_KETTE_MAX_MS;
    }

    /// Eine Zeile fuer das Protokoll.
    fn zeile(&self, nr: usize, gesamt: usize, mit_anzeige: bool) -> String {
        if self.gescheitert {
            return format!(
                "Benchmark {nr}/{gesamt}: {}, {} fps, {} Mbit/s: gescheitert (Codecwechsel oder Einstellung blieb aus)",
                self.codec, self.fps, self.mbit
            );
        }
        format!(
            "Benchmark {nr}/{gesamt}: {}, {} fps, {} Mbit/s: {:.1} Bilder/s, {:.1} Mbit/s, Kette {:.1} ms (Encoder {:.1}, Leitung {:.1}, Decoder {:.1}, Anzeige {}), verworfen {}, ausgelassen {}, Host-Encoder {:.1}/{:.1} ms, Host-CPU {:.0} %, Client-CPU {:.1} % - {}",
            self.codec, self.fps, self.mbit, self.fps_gemessen, self.mbit_gemessen, self.kette_ms,
            self.encoder_ms, self.leitung_ms, self.decoder_ms,
            if mit_anzeige { format!("{:.1}", self.anzeige_ms) } else { "-".into() },
            if mit_anzeige { self.verworfen.to_string() } else { "-".into() },
            if mit_anzeige { self.ausgelassen.to_string() } else { "-".into() },
            self.host_encoder_ms, self.budget_ms, self.host_cpu, self.client_cpu,
            if self.bestanden { "bestanden" } else { "nicht bestanden" }
        )
    }

    /// Eine Zeile der Texttabelle (Datei und Konsole).
    fn tabellenzeile(&self, mit_anzeige: bool) -> String {
        let strich = |v: f32| if self.gescheitert { "-".to_string() } else { format!("{v:.1}") };
        let anzeige = if mit_anzeige && !self.gescheitert { format!("{:.1}", self.anzeige_ms) } else { "-".into() };
        let verworfen = if mit_anzeige && !self.gescheitert { self.verworfen.to_string() } else { "-".into() };
        format!(
            "{:<20} {:>8} {:>9} {:>9} {:>8} {:>7} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>8} {:>10}  {}",
            self.codec, self.fps, self.mbit,
            strich(self.fps_gemessen), strich(self.mbit_gemessen), strich(self.kette_ms),
            strich(self.encoder_ms), strich(self.leitung_ms), strich(self.decoder_ms), anzeige,
            verworfen,
            if self.gescheitert { "-".into() } else { format!("{:.1}/{:.1}", self.host_encoder_ms, self.budget_ms) },
            strich(self.host_cpu), strich(self.client_cpu),
            if self.gescheitert { "gescheitert" } else if self.bestanden { "bestanden" } else { "nicht bestanden" }
        )
    }

    fn tabellenkopf() -> String {
        format!(
            "{:<20} {:>8} {:>9} {:>9} {:>8} {:>7} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>8} {:>10}  {}",
            "Codec", "Soll-fps", "Soll-Mbit", "Bilder/s", "Mbit/s", "Kette", "Encoder", "Leitung", "Decoder", "Anzeige",
            "verworfen", "Host-Enc", "Host-CPU", "Client-CPU", "Ergebnis"
        )
    }
}

/// Die Empfehlung: die beste Farbqualitaet, die bei der hoechsten Bildrate
/// besteht, mit der hoechsten Datenrate, bei der sie noch besteht. Also
/// unter allen bestandenen Schritten der mit der groessten (Bildrate,
/// Qualitaet, Datenrate) - in dieser Rangfolge.
fn empfehlen(ergebnisse: &[Ergebnis]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, e) in ergebnisse.iter().enumerate().filter(|(_, e)| e.bestanden) {
        let besser = match best {
            None => true,
            Some(b) => {
                let o = &ergebnisse[b];
                (e.fps, e.qualitaet, e.mbit) > (o.fps, o.qualitaet, o.mbit)
            }
        };
        if besser {
            best = Some(i);
        }
    }
    best
}

/// Tabelle plus Empfehlung als Text - fuer benchmark.txt und die Konsole
/// des Pruefmodus.
fn bench_text(ergebnisse: &[Ergebnis], mit_anzeige: bool, abgebrochen: bool, adresse: &str) -> String {
    let mut t = String::new();
    t.push_str(&format!("QuadChroma Benchmark - Host {adresse}{}\n", if abgebrochen { " (abgebrochen)" } else { "" }));
    t.push_str("Zeiten in ms, Mittel ueber die Messzeit; Host-Enc = Encoderzeit je Bild / Budget 1000/fps\n\n");
    t.push_str(&Ergebnis::tabellenkopf());
    t.push('\n');
    for e in ergebnisse {
        t.push_str(&e.tabellenzeile(mit_anzeige));
        t.push('\n');
    }
    t.push('\n');
    match empfehlen(ergebnisse) {
        Some(i) => {
            let e = &ergebnisse[i];
            t.push_str(&format!(
                "Empfehlung: {}, {} Bilder/s, {} Mbit/s - Kette {:.1} ms\n",
                e.codec, e.fps, e.mbit, e.kette_ms
            ));
        }
        None => t.push_str("Empfehlung: kein Schritt hat bestanden.\n"),
    }
    t
}

/// Wo der Automat gerade steht.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BenchPhase {
    /// Codecwunsch unterwegs; warten auf Nachricht 7 und das erste Bild.
    Wechsel,
    /// Einstellungen unterwegs; warten auf die Bestaetigung des Hosts.
    Einstellen,
    Einschwingen,
    Messen,
    Fertig,
}

/// Ein Blick auf den Lauf, fuer das Menue - Abzug, keine Leihgabe, damit
/// das Zeichnen den Automaten nicht festhaelt.
pub struct BenchStand {
    pub laeuft: bool,
    pub abgebrochen: bool,
    pub pos: usize,
    pub gesamt: usize,
    pub schritt: String,
    pub phase: BenchPhase,
    pub ergebnisse: Vec<Ergebnis>,
    pub empfehlung: Option<Ergebnis>,
    pub mit_anzeige: bool,
}

pub struct Benchmark {
    schritte: Vec<BenchSchritt>,
    codecs: Vec<CodecEintrag>,
    pos: usize,
    phase: BenchPhase,
    /// Beginn der laufenden Phase.
    seit: Instant,
    dauer: Duration,
    testbild: bool,
    /// Laeuft ein Fenster? Ohne eines gibt es kein Glied Anzeige und nichts,
    /// das verworfen werden koennte.
    mit_anzeige: bool,
    adresse: String,
    /// Codec und Einstellungen vor dem Start - am Ende wieder gewuenscht.
    vorher_codec: Option<u8>,
    vorher_settings: Option<(u32, u16, bool, bool, bool)>,
    /// Ton bleibt, wie er war.
    ton: bool,
    /// Der Wunsch der laufenden Phase ist beim Host - oder wartet noch auf
    /// den Eingabekanal.
    gesendet: bool,
    /// Wechsel: Stand von `decoded`, als Nachricht 7 zum Ziel gesehen
    /// wurde. Erst ein Bild danach zaehlt als "angekommen".
    wechsel_gesehen: Option<u64>,
    /// Messen: decodiert, verworfen, ausgelassen, Bytes zu Beginn.
    start: (u64, u64, u64, u64),
    letzte_probe: Instant,
    /// Latenz (mit Anzeige), Hostlast, Client-CPU je Probe.
    proben: Vec<(Latenz, Option<HostLast>, f32)>,
    ergebnisse: Vec<Ergebnis>,
    empfehlung: Option<usize>,
    abgebrochen: bool,
}

impl Benchmark {
    /// Den Lauf vorbereiten: Schrittliste aus Konfiguration und
    /// Koennensliste, Ausgangslage merken. None, wenn nichts zu tun ist.
    fn neu(konfig: &BenchKonfig, shared: &Arc<Mutex<Shared>>, mit_anzeige: bool, adresse: &str) -> Option<Benchmark> {
        let s = shared.lock().unwrap();
        let codecs = s.codecs.clone();
        let schritte = konfig.schritte(&codecs);
        if schritte.is_empty() {
            return None;
        }
        // Was gerade laeuft: Nachricht 7, sonst der Abgleich mit der Strominfo.
        let vorher_codec = s
            .codec_idx
            .or_else(|| s.info.and_then(|i| codecs.iter().find(|e| e.passt_zu(&i)).map(|e| e.idx)));
        let ton = s.settings.map(|x| x.4).unwrap_or(s.ton);
        Some(Benchmark {
            schritte,
            codecs,
            pos: 0,
            phase: BenchPhase::Fertig,
            seit: Instant::now(),
            dauer: Duration::from_secs(konfig.dauer_s.clamp(3, 15) as u64),
            testbild: konfig.testbild,
            mit_anzeige,
            adresse: adresse.to_string(),
            vorher_codec,
            vorher_settings: s.settings,
            ton,
            gesendet: false,
            wechsel_gesehen: None,
            start: (0, 0, 0, 0),
            letzte_probe: Instant::now(),
            proben: Vec::new(),
            ergebnisse: Vec::new(),
            empfehlung: None,
            abgebrochen: false,
        })
    }

    fn laeuft(&self) -> bool {
        self.phase != BenchPhase::Fertig
    }

    fn schritt(&self) -> &BenchSchritt {
        &self.schritte[self.pos.min(self.schritte.len() - 1)]
    }

    /// Los: Testbild an, erster Schritt.
    fn starten(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        protokoll::zeile(format!(
            "Benchmark: {} Schritte, {} s je Schritt, Testbild {}",
            self.schritte.len(),
            self.dauer.as_secs(),
            if self.testbild { "an" } else { "aus" }
        ));
        if self.testbild {
            input.lock().unwrap().testbild(true);
        }
        self.schritt_beginnen(shared, input);
    }

    /// Einen Schritt anfangen: laeuft der Codec schon, gleich einstellen,
    /// sonst erst wechseln.
    fn schritt_beginnen(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        self.seit = Instant::now();
        self.gesendet = false;
        self.wechsel_gesehen = None;
        let idx = self.schritt().idx;
        let laeuft = {
            let s = shared.lock().unwrap();
            match (s.codec_idx, s.info) {
                (Some(i), _) => i == idx,
                (None, Some(i)) => self.codecs.iter().any(|e| e.idx == idx && e.passt_zu(&i)),
                (None, None) => false,
            }
        };
        self.phase = if laeuft { BenchPhase::Einstellen } else { BenchPhase::Wechsel };
        self.senden(shared, input);
    }

    /// Den Wunsch der laufenden Phase abschicken - nur, wenn der
    /// Eingabekanal steht; `send` wirft sonst stumm weg, und der Wunsch
    /// waere verloren. Sonst beim naechsten Takt noch einmal.
    fn senden(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        let (idx, mbit, fps) = {
            let s = self.schritt();
            (s.idx, s.mbit, s.fps)
        };
        let mut l = input.lock().unwrap();
        l.ensure();
        if !l.steht() {
            return;
        }
        match self.phase {
            BenchPhase::Wechsel => {
                // Wie der Klick im Menue: erst der Hinweis, dann der Wunsch.
                shared.lock().unwrap().codec_wechsel = Some(Instant::now());
                l.codec(idx);
            }
            // Spielmodus aus, feste Bildrate AN - sonst haengt die Bildrate
            // am Inhalt, und die Schritte waeren nicht vergleichbar.
            BenchPhase::Einstellen => l.settings(mbit, fps, false, true, self.ton),
            _ => {}
        }
        self.gesendet = true;
    }

    /// Ein Takt des Automaten. `client_cpu` ist die eigene Prozessorlast,
    /// wie der Aufrufer sie misst.
    fn takt(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>, client_cpu: f32) {
        if !self.laeuft() {
            return;
        }
        if !shared.lock().unwrap().connected {
            protokoll::zeile("Benchmark: Verbindung verloren, abgebrochen".into());
            self.abbrechen(shared, input, false);
            return;
        }
        if !self.gesendet {
            self.senden(shared, input);
        }
        match self.phase {
            BenchPhase::Wechsel => {
                let idx = self.schritt().idx;
                let (codec_idx, decoded, wechsel) = {
                    let s = shared.lock().unwrap();
                    (s.codec_idx, s.decoded, s.wechsel_laeuft())
                };
                if codec_idx == Some(idx) && self.wechsel_gesehen.is_none() {
                    self.wechsel_gesehen = Some(decoded);
                }
                if let Some(d0) = self.wechsel_gesehen {
                    if !wechsel && decoded > d0 {
                        self.phase = BenchPhase::Einstellen;
                        self.seit = Instant::now();
                        self.gesendet = false;
                        self.senden(shared, input);
                        return;
                    }
                }
                if self.seit.elapsed() > BENCH_FRIST {
                    self.gescheitert(shared, input);
                }
            }
            BenchPhase::Einstellen => {
                let ziel = {
                    let s = self.schritt();
                    (s.mbit, s.fps, false, true, self.ton)
                };
                if shared.lock().unwrap().settings == Some(ziel) {
                    self.phase = BenchPhase::Einschwingen;
                    self.seit = Instant::now();
                } else if self.seit.elapsed() > BENCH_FRIST {
                    self.gescheitert(shared, input);
                }
            }
            BenchPhase::Einschwingen => {
                if self.seit.elapsed() >= BENCH_EINSCHWINGEN {
                    let s = shared.lock().unwrap();
                    self.start = (s.decoded, s.dropped, s.ausgelassen, s.bytes_video);
                    drop(s);
                    self.proben.clear();
                    self.phase = BenchPhase::Messen;
                    self.seit = Instant::now();
                    self.letzte_probe = Instant::now();
                }
            }
            BenchPhase::Messen => {
                if self.letzte_probe.elapsed() >= BENCH_PROBE {
                    self.probe(shared, client_cpu);
                }
                if self.seit.elapsed() >= self.dauer {
                    if self.proben.is_empty() {
                        self.probe(shared, client_cpu);
                    }
                    self.auswerten(shared, input);
                }
            }
            BenchPhase::Fertig => {}
        }
    }

    fn probe(&mut self, shared: &Arc<Mutex<Shared>>, client_cpu: f32) {
        let (lat, hl) = {
            let s = shared.lock().unwrap();
            (s.latenz(), s.hostlast)
        };
        self.letzte_probe = Instant::now();
        self.proben.push((lat.unwrap_or_default(), hl, client_cpu));
    }

    /// Der Schritt kam nicht zustande: als gescheitert vermerken, weiter.
    fn gescheitert(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        let s = self.schritt().clone();
        let e = Ergebnis {
            idx: s.idx, codec: s.name, qualitaet: s.qualitaet, mbit: s.mbit, fps: s.fps,
            gescheitert: true, budget_ms: 1000.0 / s.fps.max(1) as f32,
            ..Ergebnis::default()
        };
        self.eintragen(e, shared, input);
    }

    /// Messzeit vorbei: Zaehlerdifferenzen und Mittel der Proben.
    fn auswerten(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        let sek = self.seit.elapsed().as_secs_f32().max(0.001);
        let (decoded, dropped, ausgelassen, bytes) = {
            let s = shared.lock().unwrap();
            (s.decoded, s.dropped, s.ausgelassen, s.bytes_video)
        };
        let empfangen = decoded.saturating_sub(self.start.0);
        // Nur Proben mit stehendem Zeitabgleich sagen etwas ueber die Kette.
        let mit_latenz: Vec<&Latenz> = self.proben.iter().map(|(l, _, _)| l).filter(|l| l.gesamt_ms > 0.0).collect();
        let mittel = |f: &dyn Fn(&Latenz) -> f32| -> f32 {
            if mit_latenz.is_empty() { 0.0 } else { mit_latenz.iter().map(|l| f(l)).sum::<f32>() / mit_latenz.len() as f32 }
        };
        let hosts: Vec<HostLast> = self.proben.iter().filter_map(|(_, h, _)| *h).collect();
        let host_mittel = |f: &dyn Fn(&HostLast) -> f32| -> f32 {
            if hosts.is_empty() { 0.0 } else { hosts.iter().map(|h| f(h)).sum::<f32>() / hosts.len() as f32 }
        };
        let client_cpu = self.proben.iter().map(|(_, _, c)| *c).sum::<f32>() / self.proben.len().max(1) as f32;
        let s = self.schritt().clone();
        let mut e = Ergebnis {
            idx: s.idx,
            codec: s.name,
            qualitaet: s.qualitaet,
            mbit: s.mbit,
            fps: s.fps,
            gescheitert: false,
            fps_gemessen: empfangen as f32 / sek,
            mbit_gemessen: bytes.saturating_sub(self.start.3) as f32 * 8.0 / sek / 1e6,
            kette_ms: mittel(&|l| l.bis_anzeige()),
            encoder_ms: mittel(&|l| l.encoder_ms),
            leitung_ms: mittel(&|l| l.leitung_ms),
            decoder_ms: mittel(&|l| l.decoder_ms),
            anzeige_ms: mittel(&|l| l.anzeige_ms),
            empfangen,
            verworfen: dropped.saturating_sub(self.start.1),
            ausgelassen: ausgelassen.saturating_sub(self.start.2),
            host_encoder_ms: host_mittel(&|h| h.encoder_ms),
            budget_ms: 1000.0 / s.fps.max(1) as f32,
            host_cpu: host_mittel(&|h| h.cpu),
            client_cpu,
            bestanden: false,
        };
        e.pruefen(self.mit_anzeige, !mit_latenz.is_empty());
        self.eintragen(e, shared, input);
    }

    /// Ergebnis festhalten, ins Protokoll, Empfehlung nachfuehren, und
    /// zum naechsten Schritt - oder zum Ende.
    fn eintragen(&mut self, e: Ergebnis, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
        protokoll::zeile(e.zeile(self.pos + 1, self.schritte.len(), self.mit_anzeige));
        self.ergebnisse.push(e);
        self.empfehlung = empfehlen(&self.ergebnisse);
        self.pos += 1;
        if self.pos < self.schritte.len() {
            self.schritt_beginnen(shared, input);
        } else {
            self.abschliessen(shared, input, true);
        }
    }

    /// Abbruch von aussen: Knopf, ESC, Verbindung weg. Mit
    /// `wiederherstellen` gehen Codec und Einstellungen von vor dem Start
    /// wieder an den Host; ohne (Trennen) nur das Testbild aus.
    fn abbrechen(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>, wiederherstellen: bool) {
        self.abgebrochen = true;
        self.abschliessen(shared, input, wiederherstellen);
    }

    /// Ende des Laufs: Testbild aus, Ausgangslage wieder wuenschen,
    /// Empfehlung, Datei.
    fn abschliessen(&mut self, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>, wiederherstellen: bool) {
        self.phase = BenchPhase::Fertig;
        self.empfehlung = empfehlen(&self.ergebnisse);
        if self.testbild {
            input.lock().unwrap().testbild(false);
        }
        if wiederherstellen {
            let jetzt = shared.lock().unwrap().codec_idx;
            if let Some(v) = self.vorher_codec {
                if jetzt != Some(v) {
                    shared.lock().unwrap().codec_wechsel = Some(Instant::now());
                    input.lock().unwrap().codec(v);
                }
            }
            if let Some(w) = self.vorher_settings {
                input.lock().unwrap().settings(w.0, w.1, w.2, w.3, w.4);
            }
        }
        protokoll::zeile(match (self.abgebrochen, self.empfehlung) {
            (true, _) => format!("Benchmark abgebrochen nach {} von {} Schritten", self.ergebnisse.len(), self.schritte.len()),
            (false, Some(i)) => {
                let e = &self.ergebnisse[i];
                format!("Benchmark fertig. Empfehlung: {}, {} Bilder/s, {} Mbit/s - Kette {:.1} ms", e.codec, e.fps, e.mbit, e.kette_ms)
            }
            (false, None) => "Benchmark fertig. Kein Schritt hat bestanden.".into(),
        });
        // Die ganze Tabelle in die Datei - bei jedem Lauf neu, auch nach
        // einem Abbruch: was gemessen wurde, ist gemessen.
        if !self.ergebnisse.is_empty() {
            if let Some(p) = einstellungen::datei_pfad("benchmark.txt") {
                std::fs::write(p, self.text()).ok();
            }
        }
    }

    fn text(&self) -> String {
        bench_text(&self.ergebnisse, self.mit_anzeige, self.abgebrochen, &self.adresse)
    }

    /// Der Abzug fuer das Menue.
    fn stand(&self) -> BenchStand {
        BenchStand {
            laeuft: self.laeuft(),
            abgebrochen: self.abgebrochen,
            pos: self.pos,
            gesamt: self.schritte.len(),
            schritt: self.schritt().beschreibung(),
            phase: self.phase,
            ergebnisse: self.ergebnisse.clone(),
            empfehlung: self.empfehlung.map(|i| self.ergebnisse[i].clone()),
            mit_anzeige: self.mit_anzeige,
        }
    }
}

/// Eigene Prozessorlast, dasselbe Mass wie der Task-Manager: Kernel- plus
/// Nutzerzeit des Prozesses seit der letzten Messung, geteilt durch
/// Wandzeit mal logische Kerne - "Prozent der Maschine". `zeiten` haelt
/// Prozesszeit und Zeitpunkt der letzten Messung und wird fortgeschrieben.
fn cpu_eigen_messen(zeiten: &mut (u64, Instant)) -> Option<f32> {
    let jetzt = prozesszeit_100ns()?;
    let (vorher, seit) = *zeiten;
    let wand = seit.elapsed().as_secs_f64() * 1e7;
    let kerne = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    *zeiten = (jetzt, Instant::now());
    if wand > 0.0 {
        Some((jetzt.saturating_sub(vorher) as f64 / (wand * kerne) * 100.0) as f32)
    } else {
        None
    }
}

// ------------------------------------------------------------------ Fenster

#[derive(PartialEq)]
enum Screen { Start, Session }

/// Wer ins Fenster zeichnet: die Karte ueber eine Flip-Swapchain, oder
/// softbuffer (GDI) - nie beides am selben Fenster, das ist von DXGI nicht
/// gedeckt. Entschieden wird beim Start; `Keine` bleibt nach einem
/// Geraeteverlust, den der Neubau nicht heilen konnte.
enum Anzeige {
    #[cfg(windows)]
    Gpu(anzeige::Gpu),
    Cpu {
        /// Wird nach dem Anlegen der Flaeche nicht mehr angefasst, muss
        /// aber so lange leben wie sie.
        #[allow(dead_code)]
        context: softbuffer::Context<Arc<Window>>,
        surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    },
    Keine,
}

/// Was auf dem Weg ueber die Karte schiefgehen kann: ein Aufruf (die Karte
/// lebt, der Fehler steht im Protokoll) oder das Geraet selbst.
#[cfg(windows)]
enum Ausfall {
    Fehler(String),
    GeraetWeg(String),
}

/// Der rohe Win32-Griff des Fensters, wie DXGI ihn braucht.
#[cfg(windows)]
fn fenster_hwnd(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Zwei Kaesten zu einem: der Ausschnitt der Oberflaeche, der in die Textur
/// muss - was beim letzten Mal dort stand (zu loeschen) und was jetzt neu
/// gezeichnet ist.
#[cfg(windows)]
fn kasten_vereinigen(a: Option<ui::Rect>, b: Option<ui::Rect>) -> Option<ui::Rect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            let (ex, ey) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
            Some(ui::Rect { x, y, w: ex - x, h: ey - y })
        }
        (a, None) => a,
        (None, b) => b,
    }
}

struct App {
    shared: Arc<Mutex<Shared>>,
    input: Arc<Mutex<InputLink>>,
    ui: ui::Ui,
    screen: Screen,
    hosts: Arc<Mutex<discovery::Hosts>>,
    addr_input: String,
    lang: &'static strings::Lang,
    show_overlay: bool,
    fps_hist: Vec<f32>,
    last_frame: Option<Frame>,
    quit: bool,
    mods: u32,
    fullscreen: bool,
    pixel_exact: bool,
    fps_count: u32,
    fps_shown: f32,
    fps_since: Instant,
    window: Option<Arc<Window>>,
    /// Wer zeichnet - Karte oder softbuffer, entschieden in `resumed`.
    anzeige: Anzeige,
    /// Aus --anzeige oder der Datei, und --adapter n.
    anzeige_wunsch: einstellungen::AnzeigeWunsch,
    adapter_wunsch: Option<u32>,
    /// Die erkannten Karten (einmal beim Start), fuer die Knoepfe im Menue.
    karten: Vec<Karte>,
    /// Welcher Rollen-Knopf der Anzeige wirklich gilt, entschieden in
    /// `resumed`: Automatik, wenn so gewuenscht und die Karte steht; sonst
    /// die Rolle der Karte, die zeichnet; Cpu, wenn softbuffer zeichnet.
    anzeige_aktiv: einstellungen::AnzeigeWunsch,
    /// Praesentation ohne Warten auf den Bildwechsel (ALLOW_TEARING). Vorerst
    /// genau dann, wenn DXGI es erlaubt; der Schalter im Menue kommt spaeter.
    sofort: bool,
    /// Weg ueber die Karte: Groesse des zuletzt hochgeladenen Bildes (der
    /// CPU-Weg haelt statt dessen `last_frame`).
    bild_da: Option<(u32, u32)>,
    /// Client-Uhr bei der Ablage des zuletzt hochgeladenen, noch nicht
    /// praesentierten Bildes - Anfang der Anzeigezeit.
    bereit_ausstehend: Option<u64>,
    /// Die Oberflaeche in Fenstergroesse, 0xTTRRGGBB ueber einem leeren
    /// (ganz durchsichtigen) Grund; nur der Kasten geht in die Textur.
    ui_puffer: Vec<u32>,
    /// Was beim letzten Mal gezeichnet wurde - beim naechsten Mal zu loeschen.
    ui_kasten_alt: Option<ui::Rect>,
    /// Masse, fuer die `ui_puffer` angelegt ist - nach Breite und Hoehe, nicht
    /// nach der Punktzahl (ein gedrehter Monitor hat dieselbe).
    ui_masse: (u32, u32),
    /// Oberflaeche gerade sichtbar (Bit 1 in Stufe 2).
    ui_an: bool,
    /// Wann die Oberflaeche zuletzt gerastert wurde - alle 33 ms reicht,
    /// das Video darunter laeuft mit voller Bildrate weiter.
    letzte_oberflaeche: Instant,
    /// Bild hochgeladen, aber Present ausgelassen, weil DXGI noch nicht
    /// bereit war: beim naechsten Takt wieder versuchen.
    praesentation_ausstehend: bool,
    /// Wann die Karte zuletzt verloren ging. Ein zweiter Verlust binnen
    /// zehn Sekunden heisst: aufgeben, nicht noch einmal bauen.
    geraet_verloren: Option<Instant>,
    /// Letzter Fehler der Karte, damit derselbe nicht je Bild ins Protokoll
    /// laeuft.
    letzter_gpu_fehler: Option<String>,
    shown: u64,
    /// Beim ersten Kontakt mit einem Host steht der Vergleichscode eine Weile
    /// gross im Bild - genau dann kann man ihn noch pruefen.
    banner_until: Option<Instant>,
    /// Gespeicherte Einstellungen. Was hier steht, ueberlebt den Neustart.
    cfg: einstellungen::Einstellungen,
    /// Fuer welchen Host die gespeicherten Werte schon geschickt wurden.
    /// Verhindert, dass wir sie in jedem Bild erneut senden.
    angewandt_fuer: Option<String>,
    /// Wann zuletzt gezeichnet wurde - Oberflaechen ohne neues Bild werden
    /// nur alle 33 ms neu gezeichnet.
    letzte_zeichnung: Instant,
    /// Seit wann ESC gehalten wird, und ob der Druck schon verbraucht ist.
    /// Ein kurzer Druck geht an den Host, ein langer oeffnet das Menue - und
    /// dann darf der Host ihn gerade NICHT sehen.
    esc_seit: Option<Instant>,
    esc_verbraucht: bool,
    /// Modifier im Moment des Druecks, nicht des Loslassens.
    esc_mods: u32,
    /// Wann das Loslassen des nachgereichten ESC faellig ist.
    esc_up_faellig: Option<Instant>,
    /// Nerd-Modus offen, und auf welcher Seite.
    hud_offen: bool,
    hud_reiter: u8,
    /// Verlauf der Gesamtverzoegerung, fuer die Kachel im Nerd-Modus.
    lat_hist: Vec<f32>,
    /// Wer zeichnet, fuer die Statistik: "Software" oder "D3D11 · <Adapter>".
    anzeige_name: String,
    /// Eigene Prozessorlast in Prozent der ganzen Maschine, und die
    /// Prozesszeit samt Zeitpunkt der letzten Messung.
    cpu_eigen: f32,
    cpu_zeiten: (u64, Instant),
    /// Bildwiederholrate des Monitors, auf dem das Fenster steht.
    monitor_hz: Option<f32>,
    /// Zeigerform: welche Nummer aus `Shared` gerade gilt, ob der Windows-Zeiger
    /// im Moment eine Mac-Form traegt, und der Vorrat schon gebauter Formen
    /// (Kennung -> Zeiger), damit die Wartekugel nicht je Bild neu gebaut wird.
    zeiger_seq_gezeigt: u64,
    zeiger_eigen: bool,
    zeiger_vorrat: Vec<(u64, CustomCursor)>,
    /// Massstab, mit dem die geltende Form gebaut wurde (siehe zeiger_massstab).
    zeiger_faktor: u32,
    /// Ist die Maus gerade im Fenster? Nur dann wird eine Form gesetzt.
    maus_im_fenster: bool,
    /// Der laufende oder zuletzt gelaufene Benchmark, und was der naechste
    /// durchprobieren soll.
    benchmark: Option<Benchmark>,
    bench_konfig: BenchKonfig,
    /// Ergebnistabelle im Reiter Benchmark: erste sichtbare Zeile, ob die
    /// Ansicht der neuesten Zeile folgt (bis der Nutzer rollt; wieder,
    /// sobald er ans Ende rollt oder der Lauf endet), und ob beim letzten
    /// Zeichnen ein Lauf lief (um sein Ende zu bemerken).
    bench_scroll: usize,
    bench_folgt: bool,
    bench_lief: bool,
}

/// Die Rolle einer Karte als Anzeigewunsch - fuer den Knopf, der gilt.
fn rolle_als_anzeige(r: Rolle) -> einstellungen::AnzeigeWunsch {
    use einstellungen::AnzeigeWunsch as W;
    match r {
        Rolle::Grafikkarte(1) => W::Gpu,
        Rolle::Grafikkarte(_) => W::Gpu2,
        Rolle::Integriert => W::Integriert,
    }
}

impl App {
    /// Welchen Adapter `Gpu::neu` nehmen soll: --adapter n schlaegt alles;
    /// sonst die Karte der gewuenschten Rolle. Gibt es die nicht, sagt das
    /// Protokoll es, und die Automatik von `Gpu::neu` entscheidet (None).
    fn adapter_index(&self) -> Option<u32> {
        use einstellungen::AnzeigeWunsch as W;
        if self.adapter_wunsch.is_some() {
            return self.adapter_wunsch;
        }
        let rolle = match self.anzeige_wunsch {
            W::Gpu => Rolle::Grafikkarte(1),
            W::Gpu2 => Rolle::Grafikkarte(2),
            W::Integriert => Rolle::Integriert,
            _ => return None,
        };
        match karte_mit(&self.karten, rolle) {
            Some(k) => Some(k.index),
            None => {
                protokoll::zeile(format!("Anzeige: keine Karte mit der Rolle {} erkannt - Automatik", rolle.name()));
                None
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        // Vollbild ist die Voreinstellung; F11 schaltet um und merkt sich das.
        let attrs = Window::default_attributes()
            .with_title("QuadChroma")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
            .with_fullscreen(if self.fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            });
        let window = Arc::new(el.create_window(attrs).expect("Fenster"));
        // Wer zeichnet: die Karte, wenn sie gewuenscht ist und geht - sonst
        // softbuffer. Nie beides am selben Fenster: erst wenn feststeht, dass
        // keine Swapchain daran haengt, kommt GDI.
        self.anzeige = Anzeige::Keine;
        self.anzeige_aktiv = einstellungen::AnzeigeWunsch::Cpu;
        #[cfg(windows)]
        {
            use einstellungen::AnzeigeWunsch as W;
            if self.anzeige_wunsch != W::Cpu {
                let s = window.inner_size();
                let adapter = self.adapter_index();
                let bau = fenster_hwnd(&window)
                    .ok_or_else(|| "kein Win32-Fenster".to_string())
                    .and_then(|h| anzeige::Gpu::neu(h, s.width, s.height, self.anzeige_wunsch == W::Warp, adapter));
                match bau {
                    Ok(g) => {
                        self.anzeige_name = format!("D3D11 · {}", g.adapter.name);
                        self.sofort = g.tearing;
                        // Welche Rolle zeichnet - fuer den hervorgehobenen
                        // Knopf und als Kandidat fuer D3D11VA.
                        let karte = self.karten.iter().find(|k| k.luid == g.adapter.luid);
                        self.anzeige_aktiv = match (self.anzeige_wunsch, karte) {
                            (W::Automatik, Some(_)) => W::Automatik,
                            (_, Some(k)) => rolle_als_anzeige(k.rolle),
                            (W::Warp, None) => W::Warp,
                            (_, None) => W::Automatik,
                        };
                        // Ab jetzt legt der Empfangsfaden rohe Bilder ab; die
                        // Umrechnung nach RGB macht Stufe 1 auf der Karte.
                        let mut sh = self.shared.lock().unwrap();
                        sh.gpu_pfad = true;
                        sh.anzeige_adapter = karte.map(|k| k.index);
                        drop(sh);
                        self.anzeige = Anzeige::Gpu(g);
                    }
                    Err(e) => {
                        protokoll::zeile(format!("Anzeige: Rueckfall auf Software: {e}"));
                        if !matches!(self.anzeige_wunsch, W::Automatik | W::Warp) {
                            let m = Meldung::neu(
                                strings::Key::ErrorGpuDisplay,
                                format!("Grafikkarte nicht nutzbar, Anzeige ueber Software: {e}"),
                            )
                            .anhang(e);
                            self.shared.lock().unwrap().error = Some(m);
                        }
                    }
                }
            }
        }
        if matches!(self.anzeige, Anzeige::Keine) {
            let context = softbuffer::Context::new(window.clone()).expect("Kontext");
            let surface = softbuffer::Surface::new(&context, window.clone()).expect("Flaeche");
            self.anzeige = Anzeige::Cpu { context, surface };
            self.anzeige_name = "Software".into();
            protokoll::zeile("Anzeige: Software".into());
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.draw(),

            WindowEvent::CursorMoved { position, .. } => {
                self.ui.mouse = (position.x as i32, position.y as i32);
                // Eine Bewegung im Fenster ist der sicherste Beleg dafuer,
                // dass die Maus drin ist - auch wenn CursorEntered ausblieb.
                self.maus_im_fenster = true;
                if self.screen == Screen::Start { return; }
                // Steht die Einstellungstafel offen, gehoert die Maus ihr und
                // nicht dem Mac - sonst klickt man dort zweimal gleichzeitig.
                if self.hud_offen || !self.bild_vorhanden() { return; }
                if let Some((nx, ny)) = self.maus_ins_bild(position.x, position.y) {
                    self.input.lock().unwrap().mouse_move(nx, ny);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                use winit::event::MouseButton;
                if self.screen == Screen::Start {
                    if button == MouseButton::Left && state == winit::event::ElementState::Pressed {
                        self.ui.click = true;
                    }
                    return;
                }
                if self.hud_offen || !self.bild_vorhanden() {
                    if button == MouseButton::Left && state == winit::event::ElementState::Pressed {
                        self.ui.click = true;
                    }
                    return;
                }
                let b = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                self.input.lock().unwrap().mouse_button(b, state == winit::event::ElementState::Pressed);
            }
            WindowEvent::CursorEntered { .. } => {
                self.maus_im_fenster = true;
            }
            WindowEvent::CursorLeft { .. } => {
                self.maus_im_fenster = false;
                // Beim Wiedereintritt wird die Form frisch angewandt.
                self.zeiger_seq_gezeigt = 0;
            }
            WindowEvent::Focused(false) => {
                // Das Fenster ist weg - Alt-Tab, Sperrbildschirm, ein Dialog.
                // Alles loslassen, was drueben noch gedrueckt ist, sonst laeuft
                // die Spielfigur dort bis in alle Ewigkeit gegen die Wand.
                self.input.lock().unwrap().alle_loslassen();
                self.mods = 0;
                self.esc_seit = None;
                self.esc_verbraucht = false;
                self.esc_up_faellig = None;
            }
            WindowEvent::ModifiersChanged(m) => {
                let st = m.state();
                self.mods = 0;
                if st.shift_key()   { self.mods |= MOD_SHIFT; }
                if st.control_key() { self.mods |= MOD_CTRL; }
                if st.alt_key()     { self.mods |= MOD_ALT; }
                if st.super_key()   { self.mods |= MOD_CMD; }
            }
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                use winit::keyboard::KeyCode as KC;
                let pressed = event.state == winit::event::ElementState::Pressed;

                // Startbildschirm: Adresse eintippen
                if self.screen == Screen::Start {
                    if !pressed { return; }
                    if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KC::Backspace => { self.addr_input.pop(); return; }
                            KC::Enter | KC::NumpadEnter => {
                                if !self.addr_input.is_empty() {
                                    let a = adresse_vollstaendig(&self.addr_input);
                                    self.input.lock().unwrap().set_addr(bump_port(&a, 1));
                                    let mut sh = self.shared.lock().unwrap();
                                    sh.target = Some(a);
                                    sh.error = None;
                                    sh.error_key = None;
                                    drop(sh);
                                    self.screen = Screen::Session;
                                }
                                return;
                            }
                            KC::Escape => { self.quit = true; return; }
                            _ => {}
                        }
                    }
                    if let Some(t) = &event.text {
                        for ch in t.chars() {
                            if !ch.is_control() && self.addr_input.len() < 64 { self.addr_input.push(ch); }
                        }
                    }
                    return;
                }

                // ESC wird zurueckgehalten. Erst beim Loslassen entscheidet
                // sich, was daraus wird: kurzer Druck -> geht an den Host,
                // langes Halten -> oeffnet das Menue und der Host sieht nichts.
                // Ob der Halt lang genug war, entscheidet die Fensterschleife,
                // nicht dieser Block - sonst ginge das Menue erst beim
                // Loslassen auf, also nie zur richtigen Zeit.
                if let winit::keyboard::PhysicalKey::Code(KC::Escape) = event.physical_key {
                    // Ist der Nerd-Modus offen, schliesst ESC ihn - und geht
                    // ganz sicher nicht an den Host.
                    if self.hud_offen {
                        if pressed && !event.repeat {
                            self.hud_offen = false;
                            self.esc_seit = None;
                            self.esc_verbraucht = true;
                            // Menue zu heisst: ein laufender Benchmark ist
                            // abgebrochen - niemand saehe ihn mehr.
                            self.benchmark_abbrechen(true);
                        }
                        return;
                    }
                    if pressed {
                        if !event.repeat && self.esc_seit.is_none() {
                            // Die Modifier werden HIER festgehalten, nicht beim
                            // Loslassen. Wer waehrend des Haltens Strg drueckt,
                            // darf den Zustand nicht mehr umdeuten koennen.
                            self.esc_mods = self.mods;
                            self.esc_seit = Some(Instant::now());
                            self.esc_verbraucht = false;
                            // Mit Modifier ist es eine Tastenkombination und
                            // kein Halten: sofort durchreichen.
                            if self.esc_mods != 0 {
                                self.esc_verbraucht = true;
                                if self.esc_mods & MOD_CTRL != 0 {
                                    self.verbindung_trennen();
                                } else if let Some(mac) = mac_keycode(KC::Escape) {
                                    let m = self.esc_mods;
                                    self.input.lock().unwrap().key(mac, true, m);
                                    self.esc_up_faellig = Some(Instant::now() + ESC_TIPPDAUER);
                                }
                            }
                        }
                    } else {
                        let verbraucht = self.esc_verbraucht;
                        self.esc_seit = None;
                        self.esc_verbraucht = false;
                        // Ein echter Tipper braucht eine messbare Dauer. Wuerden
                        // wir Druck und Loslassen unmittelbar nacheinander
                        // schicken, saehen Spiele, die den Tastenzustand je Bild
                        // abtasten, den Druck ueberhaupt nicht.
                        if !verbraucht && !is_synthetic {
                            if let Some(mac) = mac_keycode(KC::Escape) {
                                let m = self.esc_mods;
                                self.input.lock().unwrap().key(mac, true, m);
                                self.esc_up_faellig = Some(Instant::now() + ESC_TIPPDAUER);
                            }
                        }
                    }
                    return;
                }

                // Sitzung: eigene Tasten abfangen
                if pressed {
                    if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KC::F9 => {
                                self.show_overlay = !self.show_overlay;
                                self.cfg.overlay = self.show_overlay;
                                self.cfg.sichern();
                                return;
                            }
                            KC::F10 => {
                                // Dasselbe Menue wie das Halten von ESC - nur
                                // fuer alle, die sich eine Taste merken wollen.
                                self.hud_offen = !self.hud_offen;
                                if self.hud_offen {
                                    self.input.lock().unwrap().alle_loslassen();
                                    self.mods = 0;
                                    self.hud_reiter = 0;
                                } else {
                                    self.benchmark_abbrechen(true);
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                if let winit::keyboard::PhysicalKey::Code(c) = event.physical_key {
                    if c == KC::F9 || c == KC::F10 { return; }
                }
                if is_synthetic || event.repeat {
                    return; // Wiederholungen erzeugt der Mac selbst
                }
                // Steht die Tafel offen, gehoert die Tastatur ihr. Bei der Maus
                // war das schon so, bei der Tastatur fehlte es - jeder Tastendruck
                // im Menue landete zusaetzlich auf dem Mac. Bild auf/ab rollt
                // die Ergebnistabelle des Benchmarks um eine Seite.
                if self.hud_offen {
                    if pressed && self.hud_reiter == 4 {
                        if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                            let seite = self.ui.bench_sichtbar.saturating_sub(1).max(1) as i32;
                            match code {
                                KC::PageUp => self.bench_scrollen(-seite),
                                KC::PageDown => self.bench_scrollen(seite),
                                _ => {}
                            }
                        }
                    }
                    return;
                }
                if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                    use winit::keyboard::KeyCode;
                    // Tasten fuer das Fenster selbst, die nicht zum Mac gehen.
                    if event.state == winit::event::ElementState::Pressed {
                        if code == KeyCode::F11 {
                            // Waehrend ESC gehalten wird nicht umschalten: der
                            // Wechsel kostet Fokus und frisst das Loslassen.
                            if self.esc_seit.is_some() {
                                return;
                            }
                            self.fullscreen = !self.fullscreen;
                            self.cfg.vollbild = self.fullscreen;
                            self.cfg.sichern();
                            if let Some(w) = &self.window {
                                w.set_fullscreen(if self.fullscreen {
                                    Some(winit::window::Fullscreen::Borderless(None))
                                } else {
                                    None
                                });
                            }
                            return;
                        }
                        if code == KeyCode::F12 {
                            self.pixel_exact = !self.pixel_exact;
                            self.cfg.pixelgenau = self.pixel_exact;
                            self.cfg.sichern();
                            return;
                        }
                    }
                    if code == KeyCode::F11 || code == KeyCode::F12 {
                        return;
                    }
                    if let Some(mac) = mac_keycode(code) {
                        let down = event.state == winit::event::ElementState::Pressed;
                        let mods = self.mods;
                        self.input.lock().unwrap().key(mac, down, mods);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.screen == Screen::Session => {
                use winit::event::MouseScrollDelta;
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                // Steht das Menue offen, gehoert auch das Rad ihm und nicht
                // dem Host - bisher rollte jede Raste im Menue auf dem Mac
                // mit. Ueber der Ergebnistabelle des Benchmarks rollt es die
                // Tabelle: drei Zeilen je Raste (40 Punkte).
                if self.hud_offen {
                    let ueber_tabelle = self.ui.bench_tabelle.map(|r| r.hit(self.ui.mouse.0, self.ui.mouse.1)).unwrap_or(false);
                    if ueber_tabelle && dy != 0.0 {
                        let zeilen = ((dy.abs() / 40.0) * 3.0).round().max(1.0) as i32;
                        self.bench_scrollen(if dy > 0.0 { -zeilen } else { zeilen });
                    }
                    return;
                }
                self.input.lock().unwrap().scroll(dx, dy);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quit { el.exit(); return; }
        // Nicht mehr in Dauerschleife: alle zwei Millisekunden nachsehen (das
        // reicht fuer den ESC-Balken und fuer Bilder mit 240 je Sekunde) und
        // nur zeichnen, wenn es etwas zu zeichnen gibt - siehe unten.
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(2)));

        // Hat ein anderes Geraet die Sitzung uebernommen, nimmt der
        // Empfangsfaden das Ziel selbst zurueck (MSG_ABGELOEST, keine
        // Wiederverbindung). Dann zurueck zum Startbildschirm - die Meldung
        // steht dort, und verbinden geht wieder nur auf Wunsch. Den
        // Eingabekanal hat der Host schon zu und die Tasten losgelassen:
        // erst die Bindung loesen, dann geht beim Trennen nichts mehr hin.
        if self.screen == Screen::Session && self.shared.lock().unwrap().target.is_none() {
            self.input.lock().unwrap().set_link(None);
            self.verbindung_trennen();
        }

        // Nachgereichtes ESC-Loslassen.
        if let Some(t) = self.esc_up_faellig {
            if Instant::now() >= t {
                if let Some(mac) = mac_keycode(winit::keyboard::KeyCode::Escape) {
                    let m = self.esc_mods;
                    self.input.lock().unwrap().key(mac, false, m);
                }
                self.esc_up_faellig = None;
            }
        }

        // Ist ESC lange genug gehalten, geht das Menue auf - und zwar jetzt,
        // nicht erst beim Loslassen. Der Druck gilt damit als verbraucht und
        // erreicht den Host nie.
        if let Some(t) = self.esc_seit {
            if !self.esc_verbraucht && t.elapsed() >= ESC_HALTEDAUER {
                self.esc_verbraucht = true;
                self.input.lock().unwrap().alle_loslassen();
                self.mods = 0;
                self.hud_offen = true;
                self.hud_reiter = 0;
            }
        }
        // Der Eingabekanal haengt am Bildkanal. Sobald dessen Handschlag steht,
        // reichen wir die Bindung weiter; faellt er weg, trennt sich auch die
        // Eingabe - niemand soll Tastatur ohne Bild bekommen.
        let link = { self.shared.lock().unwrap().link.clone() };
        self.input.lock().unwrap().set_link(link);

        // Steht fest, mit welchem Host wir sprechen, schicken wir ihm einmal
        // die Werte, die beim letzten Mal fuer genau diesen Host galten.
        let fp = { self.shared.lock().unwrap().peer_fp.clone() };
        match (&fp, &self.angewandt_fuer) {
            (Some(f), keiner) if keiner.as_deref() != Some(f.as_str()) => {
                // Erst, wenn der Eingabekanal steht: er baut sich im
                // Hintergrund auf, und was vorher gesendet wird, faellt weg.
                let steht = {
                    let mut l = self.input.lock().unwrap();
                    l.ensure();
                    l.steht()
                };
                if steht {
                    if let Some(w) = self.cfg.fuer_host(f) {
                        self.input.lock().unwrap().settings(w.mbit, w.fps, w.gaming, w.fest, w.ton);
                        self.shared.lock().unwrap().ton = w.ton;
                    } else {
                        self.shared.lock().unwrap().ton = true;
                    }
                    self.angewandt_fuer = Some(f.clone());
                }
            }
            (None, _) => self.angewandt_fuer = None,
            _ => {}
        }
        // Der Benchmark arbeitet im selben Takt: nie blockierend, das Bild
        // laeuft weiter, das Menue zeigt den Fortschritt.
        if let Some(b) = self.benchmark.as_mut() {
            if b.laeuft() {
                b.takt(&self.shared, &self.input, self.cpu_eigen);
            }
        }
        // Zeigerform: ueber dem Bild traegt der Windows-Zeiger die Form des
        // Macs, ueber der Oberflaeche (Menue, Start, Warten) den eigenen Pfeil.
        // Nur solange die Maus im Fenster ist (Windows' Regel: ein Fenster
        // setzt den Zeiger nur ueber seiner eigenen Flaeche), und nur solange
        // der Host eine Form geliefert hat - reisst die Verbindung ab, nimmt
        // der Empfangsfaden sie weg, und hier faellt der Zeiger auf den Pfeil
        // zurueck, statt als "unsichtbar" ueber dem stehenden Bild zu bleiben.
        // Der Massstab folgt dem Bild: so gross, wie das Mac-Bild im Fenster
        // erscheint, so gross der Zeiger - 1:1 also so gross wie auf dem Mac.
        let im_bild = self.screen == Screen::Session && !self.hud_offen && self.bild_vorhanden() && self.maus_im_fenster;
        let faktor = self.zeiger_massstab();
        let (neu, form_da) = {
            let s = self.shared.lock().unwrap();
            let form_da = s.zeiger.is_some();
            let neu = if im_bild && form_da && (s.zeiger_seq != self.zeiger_seq_gezeigt || faktor != self.zeiger_faktor) {
                s.zeiger.clone().map(|z| (s.zeiger_seq, z))
            } else {
                None
            };
            (neu, form_da)
        };
        if im_bild && form_da {
            if let Some((seq, z)) = neu {
                self.zeiger_seq_gezeigt = seq;
                self.zeiger_faktor = faktor;
                self.zeiger_anwenden(el, &z, faktor);
            }
        } else if self.zeiger_eigen {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Default);
                w.set_cursor_visible(true);
            }
            self.zeiger_eigen = false;
            // Zurueck im Bild wird die Form wieder angewandt.
            self.zeiger_seq_gezeigt = 0;
        }

        // Zeichnen nur, wenn es etwas zu zeichnen gibt: ein neues Bild - oder
        // eine Oberflaeche, die sich bewegt (Startbildschirm, Wartebild, Menue,
        // Statistik, Banner, ESC-Balken, Lagemeldung des Hosts), und die kommt
        // mit 30 Bildern je Sekunde aus. Vorher lief die Schleife ohne Pause
        // und schrieb dasselbe Bild 150-mal je Sekunde ins Fenster - auf dem
        // Laptop ein gutes Viertel der gesamten Prozessorlast des Clients.
        let (neues_bild, lage, wechsel) = {
            let s = self.shared.lock().unwrap();
            (s.frame.is_some(), s.error_key.is_some(), s.wechsel_laeuft())
        };
        // Ein ausgelassenes Present (DXGI war noch nicht bereit) wird beim
        // naechsten Takt nachgeholt; der Codecwechsel-Hinweis muss auch ohne
        // neue Bilder erscheinen und wieder verschwinden.
        let oberflaeche = self.oberflaeche_sichtbar(lage, wechsel);
        if neues_bild || self.praesentation_ausstehend || (oberflaeche && self.letzte_zeichnung.elapsed() >= Duration::from_millis(33)) {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}

impl App {
    /// Die Sitzung wirklich beenden: Ziel zuruecknehmen, Tasten drueben
    /// loslassen, den Eingabekanal schliessen und die Bildleitung kappen,
    /// damit das blockierende Lesen aufwacht. Vorher blieb nach "Trennen"
    /// alles offen, und der Client decodierte weiter - mit voller Last.
    /// Dem Windows-Zeiger die Form des Mac-Zeigers geben. Jede Form wird nur
    /// einmal gebaut; der Vorrat haelt die letzten 32 (die Wartekugel hat ein
    /// Dutzend Bilder, ein Zeigerwechsel zwischen Pfeil, Hand und Textcursor
    /// soll nichts kosten).
    fn zeiger_anwenden(&mut self, el: &ActiveEventLoop, z: &ZeigerForm, faktor: u32) {
        let Some(w) = self.window.clone() else { return };
        let k = z.kennung().wrapping_mul(31).wrapping_add(faktor as u64);
        let zeiger = match self.zeiger_vorrat.iter().find(|(kk, _)| *kk == k) {
            Some((_, c)) => c.clone(),
            None => {
                // Ganzzahlig hochziehen, Punkt fuer Punkt: ein Zeiger ist eine
                // kleine Strichzeichnung, weich gefiltert saehe er verwaschen aus.
                let f = faktor.max(1);
                let (w2, h2) = (z.w as u32 * f, z.h as u32 * f);
                let rgba = if f == 1 {
                    z.rgba.clone()
                } else {
                    let mut aus = vec![0u8; (w2 * h2 * 4) as usize];
                    for y in 0..h2 as usize {
                        let qy = y / f as usize;
                        for x in 0..w2 as usize {
                            let q = (qy * z.w as usize + x / f as usize) * 4;
                            let z4 = (y * w2 as usize + x) * 4;
                            aus[z4..z4 + 4].copy_from_slice(&z.rgba[q..q + 4]);
                        }
                    }
                    aus
                };
                let Ok(quelle) = CustomCursor::from_rgba(rgba, w2 as u16, h2 as u16, z.hx * f as u16, z.hy * f as u16) else { return };
                let c = el.create_custom_cursor(quelle);
                if self.zeiger_vorrat.len() >= 32 {
                    self.zeiger_vorrat.remove(0);
                }
                self.zeiger_vorrat.push((k, c.clone()));
                c
            }
        };
        w.set_cursor(zeiger);
        // Die Sichtbarkeit des Mac-Zeigers wird NICHT uebernommen. macOS
        // blendet ihn beim Tippen aus und laesst ihn ausgeblendet, bis die
        // physische Maus sich bewegt - vom Client aus bewegt sich die aber
        // nie. Ergebnis war ein Windows-Zeiger, der ueber dem Bild einfach
        // verschwand ("Maus geht nicht"). Der Wert reist weiter mit und steht
        // im Protokoll; sichtbar ist der Zeiger hier immer.
        w.set_cursor_visible(true);
        let _ = z.sichtbar;
        self.zeiger_eigen = true;
    }

    /// Fensterpunkt -> Bildpunkt des Macs, normiert auf 0..1. Bezogen auf den
    /// Ausschnitt, in dem das Bild wirklich liegt (Letterbox, oder bei 1:1
    /// der sichtbare Teil) - nicht auf das Fenster. Vorher galt das Fenster,
    /// und sobald das Bild es nicht ausfuellte, klickte man auf dem Mac
    /// woanders. Ausserhalb des Bildes wird an den Rand geklemmt.
    fn maus_ins_bild(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let w = self.window.as_ref()?;
        let (fw, fh) = self.bild_da.or_else(|| self.last_frame.as_ref().map(|f| (f.width, f.height)))?;
        if fw == 0 || fh == 0 {
            return None;
        }
        let s = w.inner_size();
        let (rx, ry, rw, rh) = ziel_rechteck(s.width.max(1), s.height.max(1), fw, fh, self.pixel_exact);
        if rw == 0 || rh == 0 {
            return None;
        }
        let nx = ((x - rx as f64) / rw as f64).clamp(0.0, 1.0) as f32;
        let ny = ((y - ry as f64) / rh as f64).clamp(0.0, 1.0) as f32;
        Some((nx, ny))
    }

    /// Um wie viel das Mac-Bild im Fenster vergroessert erscheint, ganzzahlig
    /// gerundet - der Zeiger bekommt denselben Massstab. Begrenzt, damit die
    /// Form unter der Grenze von 256 Bildpunkten bleibt.
    fn zeiger_massstab(&self) -> u32 {
        let Some(w) = &self.window else { return 1 };
        let Some((fw, fh)) = self.bild_da.or_else(|| self.last_frame.as_ref().map(|f| (f.width, f.height))) else { return 1 };
        if fw == 0 || fh == 0 {
            return 1;
        }
        let s = w.inner_size();
        let (_, _, zw, _) = ziel_rechteck(s.width.max(1), s.height.max(1), fw, fh, self.pixel_exact);
        let mut f = ((zw as f32 / fw as f32) + 0.5).floor().clamp(1.0, 8.0) as u32;
        if let Some(z) = self.shared.lock().unwrap().zeiger.as_ref() {
            while f > 1 && (z.w as u32 * f > 256 || z.h as u32 * f > 256) {
                f -= 1;
            }
        }
        f
    }

    /// Einen laufenden Benchmark abbrechen. Mit `wiederherstellen` bekommt
    /// der Host Codec und Einstellungen von vor dem Start zurueck; beim
    /// Trennen geht nur noch das Testbild aus.
    fn benchmark_abbrechen(&mut self, wiederherstellen: bool) {
        if let Some(b) = self.benchmark.as_mut() {
            if b.laeuft() {
                b.abbrechen(&self.shared, &self.input, wiederherstellen);
            }
        }
    }

    /// Den Benchmark mit der Konfiguration des Reiters starten.
    fn benchmark_starten(&mut self) {
        if self.benchmark.as_ref().map(|b| b.laeuft()).unwrap_or(false) {
            return;
        }
        match Benchmark::neu(&self.bench_konfig, &self.shared, true, &self.addr_input) {
            Some(mut b) => {
                b.starten(&self.shared, &self.input);
                self.benchmark = Some(b);
                // Eine neue Tabelle: von vorn, und der neuesten Zeile nach.
                self.bench_scroll = 0;
                self.bench_folgt = true;
            }
            None => protokoll::zeile("Benchmark: nichts zu messen (kein Codec, keine Rate gewaehlt)".into()),
        }
    }

    /// Wie weit die Ergebnistabelle hoechstens rollen kann: Zeilen minus
    /// Platz (den Platz hat die letzte Zeichnung gemeldet).
    fn bench_scroll_max(&self) -> usize {
        let zeilen = self.benchmark.as_ref().map(|b| b.ergebnisse.len()).unwrap_or(0);
        zeilen.saturating_sub(self.ui.bench_sichtbar)
    }

    /// Die Tabelle um `delta` Zeilen rollen (negativ: nach oben), geklemmt.
    /// Wer rollt, loest das Nachfuehren - bis er wieder ganz unten steht.
    fn bench_scrollen(&mut self, delta: i32) {
        let max = self.bench_scroll_max();
        let neu = (self.bench_scroll as i32 + delta).clamp(0, max as i32) as usize;
        self.bench_scroll = neu;
        self.bench_folgt = neu >= max;
    }

    /// Vor dem Zeichnen: waehrend eines Laufs der neuesten Zeile folgen
    /// (wenn nicht gerade der Nutzer eine Stelle haelt), am Ende des Laufs
    /// wieder folgen, und nie ueber das Ende hinaus.
    fn bench_scroll_nachfuehren(&mut self) -> usize {
        let laeuft = self.benchmark.as_ref().map(|b| b.laeuft()).unwrap_or(false);
        if self.bench_lief && !laeuft {
            self.bench_folgt = true;
        }
        self.bench_lief = laeuft;
        let max = self.bench_scroll_max();
        self.bench_scroll = if self.bench_folgt { max } else { self.bench_scroll.min(max) };
        self.bench_scroll
    }

    /// Wunsch nach Datenrate, Bildrate, Spielmodus, fester Bildrate und Ton
    /// an den Host, und die Werte fuer diesen Host merken.
    fn stellen(&mut self, m: u32, f: u16, g: bool, fx: bool, ton: bool) {
        self.input.lock().unwrap().settings(m, f, g, fx, ton);
        self.shared.lock().unwrap().ton = ton;
        if let Some(fp) = &self.angewandt_fuer {
            let fp = fp.clone();
            self.cfg.host_merken(&fp, einstellungen::HostWerte { mbit: m, fps: f, gaming: g, fest: fx, ton });
        }
    }

    /// Wunsch nach einem Kandidaten der Koennensliste. Erst den Hinweis
    /// setzen, dann den Wunsch abschicken. Andersherum koennte der
    /// Empfangsfaden Nachricht 7 und das erste Bild dazwischen verarbeiten
    /// und den Hinweis loeschen, bevor er ueberhaupt steht - dann bliebe er
    /// bis zum Ablauf der Frist haengen.
    fn codec_wuenschen(&mut self, idx: u8) {
        self.shared.lock().unwrap().codec_wechsel = Some(Instant::now());
        self.input.lock().unwrap().codec(idx);
    }

    fn verbindung_trennen(&mut self) {
        // Ein laufender Benchmark endet mit der Verbindung; wiederherstellen
        // gibt es nichts mehr, nur das Testbild geht noch aus.
        self.benchmark_abbrechen(false);
        let griff = {
            let mut s = self.shared.lock().unwrap();
            s.target = None;
            s.abbruch.take()
        };
        {
            let mut l = self.input.lock().unwrap();
            l.alle_loslassen();
            l.trennen();
        }
        if let Some(g) = griff {
            let _ = g.shutdown(std::net::Shutdown::Both);
        }
        self.hud_offen = false;
        self.screen = Screen::Start;
        self.last_frame = None;
        self.bild_da = None;
        self.ui_kasten_alt = None;
        self.banner_until = None;
        if self.zeiger_eigen {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Default);
                w.set_cursor_visible(true);
            }
            self.zeiger_eigen = false;
        }
        self.zeiger_seq_gezeigt = 0;
        self.angewandt_fuer = None;
    }

    /// Steht ein Bild im Fenster? Solange nicht, zeigt die Sitzung den
    /// Wartebildschirm, und Maus und Tastatur bleiben beim Client.
    fn bild_vorhanden(&self) -> bool {
        self.last_frame.is_some() || self.bild_da.is_some()
    }

    /// Liegt gerade etwas ueber dem Bild, das sich bewegt und deshalb alle
    /// 33 ms neu gezeichnet werden will? Startbildschirm, Wartebild, Menue,
    /// Statistik, ESC-Balken, Lagemeldung, Banner, Codecwechsel-Hinweis.
    /// `lage` und `wechsel` kommen aus `Shared`, damit der Aufrufer die
    /// Sperre nur einmal nimmt.
    fn oberflaeche_sichtbar(&self, lage: bool, wechsel: bool) -> bool {
        self.screen != Screen::Session
            || !self.bild_vorhanden()
            || self.hud_offen
            || self.show_overlay
            || self.esc_seit.is_some()
            || lage
            || wechsel
            || self.banner_until.map(|t| Instant::now() < t).unwrap_or(false)
    }

    /// Einmal je Sekunde: Bildrate, Verlauf der Verzoegerung, Fenstertitel.
    /// Die Sekundenrechnung gehoert VOR jede Verzweigung des Zeichnens: stand
    /// sie am Ende, blieb die Bildrate stehen, sobald Menue oder Statistik
    /// offen waren - also genau dann, wenn man sie ablesen will.
    fn sekundentakt(&mut self, window: &Window) {
        if self.fps_since.elapsed() >= Duration::from_secs(1) {
            self.fps_shown = self.fps_count as f32 / self.fps_since.elapsed().as_secs_f32();
            self.fps_count = 0;
            self.fps_since = Instant::now();
            self.fps_hist.push(self.fps_shown);
            if self.fps_hist.len() > 240 {
                self.fps_hist.remove(0);
            }
            // Dieselbe Sekundenschleife fuehrt den Verlauf der Verzoegerung.
            // Ohne ihn haette die Kachel im Nerd-Modus keine Geschichte, und
            // eine erfundene waere schlimmer als gar keine.
            {
                let g = self.shared.lock().unwrap().latenz().filter(|l| l.gesamt_ms > 0.0).map(|l| l.bis_anzeige());
                if let Some(g) = g {
                    self.lat_hist.push(g);
                    if self.lat_hist.len() > 240 {
                        self.lat_hist.remove(0);
                    }
                }
            }
            // Eigene Prozessorlast, dasselbe Mass wie der Task-Manager. So
            // ist die Zahl direkt mit den 21,8 % vergleichbar, die der
            // Task-Manager auf dem Laptop fuer den CPU-Weg zeigte.
            if let Some(cpu) = cpu_eigen_messen(&mut self.cpu_zeiten) {
                self.cpu_eigen = cpu;
            }
            self.monitor_hz = window
                .current_monitor()
                .and_then(|m| m.refresh_rate_millihertz())
                .map(|mhz| mhz as f32 / 1000.0);
            // Die Sekundenzeile der Sitzung, nur in die Datei: die Basislinie,
            // gegen die jede spaetere Behauptung ueber die Anzeige gemessen
            // wird. "Kette" sind alle fuenf Glieder, wie die Latenz-Zeile.
            if self.screen == Screen::Session {
                let (anzeige_ms, kette, verworfen, ausgelassen) = {
                    let s = self.shared.lock().unwrap();
                    (
                        s.anzeige_ms,
                        s.latenz().filter(|l| l.gesamt_ms > 0.0).map(|l| l.bis_anzeige()),
                        s.dropped,
                        s.ausgelassen,
                    )
                };
                protokoll::nur_datei(&format!(
                    "{:.0}s | {:.0} B/s | Anzeige {:.1} ms | Kette {} ms | verworfen {} | ausgelassen {} | {} | Client-CPU {:.1} %",
                    client_us() as f64 / 1e6,
                    self.fps_shown,
                    anzeige_ms,
                    kette.map(|k| format!("{k:.1}")).unwrap_or_else(|| "-".into()),
                    verworfen,
                    ausgelassen,
                    if self.sofort { "Sofort" } else { "Sync" },
                    self.cpu_eigen
                ));
            }
            let (dec_ms, err) = {
                let s = self.shared.lock().unwrap();
                (s.last_decode_ms, s.error.as_ref().map(|m| m.text(self.lang)))
            };
            window.set_title(&match (&self.screen, err) {
                (Screen::Start, _) => "QuadChroma".to_string(),
                (_, Some(e)) => format!("QuadChroma - {e}"),
                _ => format!(
                    "QuadChroma - {:.0} {} - {:.1} ms{}",
                    self.fps_shown,
                    self.lang.get(strings::Key::Fps),
                    dec_ms,
                    if self.pixel_exact { " - 1:1" } else { "" }
                ),
            });
        }
    }

    /// Weiche: wer zeichnet.
    fn draw(&mut self) {
        match self.anzeige {
            Anzeige::Cpu { .. } => self.draw_cpu(),
            #[cfg(windows)]
            Anzeige::Gpu(_) => self.draw_gpu(),
            Anzeige::Keine => {}
        }
    }

    /// Der Weg ueber softbuffer: Bild auf der CPU einpassen, Oberflaeche
    /// darueber, per GDI ins Fenster.
    fn draw_cpu(&mut self) {
        // Die Flaeche kurz aus dem Zustand nehmen: solange ihr Puffer
        // beschrieben wird, brauchen die Zeichenschritte `&mut self`.
        let Some(window) = self.window.clone() else { return };
        match std::mem::replace(&mut self.anzeige, Anzeige::Keine) {
            Anzeige::Cpu { context, mut surface } => {
                self.draw_cpu_auf(&window, &mut surface);
                self.anzeige = Anzeige::Cpu { context, surface };
            }
            andere => self.anzeige = andere,
        }
    }

    /// Der Weg ueber die Karte. Die Karte wird fuer die Dauer des Zeichnens
    /// aus dem Zustand genommen (wie die Flaeche beim CPU-Weg); geht dabei
    /// das Geraet verloren, kommt sie nicht zurueck, sondern wird neu gebaut
    /// oder aufgegeben.
    #[cfg(windows)]
    fn draw_gpu(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let Anzeige::Gpu(mut g) = std::mem::replace(&mut self.anzeige, Anzeige::Keine) else { return };
        match self.draw_gpu_auf(&window, &mut g) {
            Ok(()) => self.anzeige = Anzeige::Gpu(g),
            Err(Ausfall::Fehler(e)) => {
                self.gpu_fehler_melden(e);
                self.anzeige = Anzeige::Gpu(g);
            }
            Err(Ausfall::GeraetWeg(grund)) => self.geraet_verloren_behandeln(g, grund),
        }
    }

    /// Einen Fehler der Karte ins Protokoll - denselben nur einmal, nicht
    /// je Bild.
    #[cfg(windows)]
    fn gpu_fehler_melden(&mut self, e: String) {
        if self.letzter_gpu_fehler.as_deref() != Some(e.as_str()) {
            protokoll::zeile(format!("Anzeige: {e}"));
            self.letzter_gpu_fehler = Some(e);
        }
    }

    /// Ein Fehler eines Aufrufs: war es das Geraet? Dann ist Schluss mit
    /// dieser Karte; sonst nur eine Protokollzeile, und es geht weiter.
    #[cfg(windows)]
    fn gpu_fehler(&mut self, g: &anzeige::Gpu, e: String) -> Result<(), Ausfall> {
        match g.geraet_weg() {
            Some(grund) => Err(Ausfall::GeraetWeg(grund)),
            None => {
                self.gpu_fehler_melden(e);
                Ok(())
            }
        }
    }

    /// Ein Durchlauf ueber die Karte: Groesse pruefen, Bild hochladen,
    /// Oberflaeche rastern und ihren Kasten hochladen, Stufe 2 auf den
    /// Backbuffer, Present - nur wenn DXGI bereit ist, sonst beim naechsten
    /// Takt. Klicks aus der Oberflaeche wirken NACH dem Praesentieren, und
    /// `ui.click` faellt nur, wenn die Oberflaeche in diesem Durchlauf
    /// gezeichnet wurde - sonst gingen Klicks zwischen zwei Zeichnungen
    /// verloren.
    #[cfg(windows)]
    fn draw_gpu_auf(&mut self, window: &Window, g: &mut anzeige::Gpu) -> Result<(), Ausfall> {
        let size = window.inner_size();
        let (ww, wh) = (size.width, size.height);
        if ww == 0 || wh == 0 {
            // Minimiert: nichts konfigurieren, nichts zeichnen - aber das
            // Bild abholen (sonst zaehlt jedes weitere als "verworfen" und
            // der Takt zeichnet alle 2 ms ins Leere) und die Sekunde fuehren.
            let _ = self.shared.lock().unwrap().frame.take();
            self.sekundentakt(window);
            return Ok(());
        }
        if (ww, wh) != (g.breite, g.hoehe) {
            if let Err(e) = g.groesse(ww, wh) {
                return Err(match g.geraet_weg() {
                    Some(grund) => Ausfall::GeraetWeg(grund),
                    None => Ausfall::Fehler(e),
                });
            }
        }
        let n = (ww as usize) * (wh as usize);
        if self.ui_masse != (ww, wh) {
            // Erste Zeichnung oder neue Groesse: ganz durchsichtiger Grund,
            // die Oberflaeche wird komplett neu gerastert. Nach den Massen,
            // nicht nach der Punktzahl - ein gedrehter Monitor hat dieselbe.
            self.ui_puffer = vec![0xff00_0000u32; n];
            self.ui_masse = (ww, wh);
            self.ui_kasten_alt = None;
            self.ui_an = false;
        }
        // Der Takt der Oberflaeche haengt an der Uhr (siehe draw_cpu_auf).
        // `jetzt` gilt fuer die ganze Zeichnung: die Oberflaeche traegt
        // denselben Zeitstempel wie die Zeichnung selbst, sonst laege sie um
        // die Rasterzeit hinter dem 33-ms-Takt und fiele jedes zweite Mal aus.
        let jetzt = Instant::now();
        self.ui.tick = client_us() / 8000;
        self.letzte_zeichnung = jetzt;

        // Neues Bild abholen: roh ueber Stufe 1, oder fertig als RGB
        // (dunkles Bild, unbekanntes Format). Ein rohes Bild faellt gleich
        // nach dem Upload - der Pufferpool des Decoders will es zurueck.
        let bild = self.shared.lock().unwrap().frame.take();
        if let Some(b) = bild {
            let bereit_us = b.bereit_us();
            let r = match b {
                Bild::Rgb(f) => g.bild_rgb(&f).map(|_| (f.width, f.height)),
                Bild::Roh { bild, .. } => g.bild_roh(&bild).map(|_| (bild.width(), bild.height())),
            };
            match r {
                Ok(groesse) => {
                    self.bild_da = Some(groesse);
                    self.bereit_ausstehend = Some(bereit_us);
                    self.fps_count += 1;
                    self.shown += 1;
                }
                Err(e) => self.gpu_fehler(g, e)?,
            }
        }
        // Erstkontakt: die Frist fuer das Banner beginnt mit dem ersten Bild.
        // Auf dem CPU-Weg setzt sie oberflaeche_zeichnen - das laeuft hier
        // aber nur, wenn schon etwas sichtbar ist, und ohne Frist waere das
        // Banner nie sichtbar geworden.
        if self.screen == Screen::Session && self.bild_vorhanden() && self.banner_until.is_none() {
            if self.shared.lock().unwrap().first_time {
                self.banner_until = Some(Instant::now() + Duration::from_secs(25));
            }
        }
        self.sekundentakt(window);

        // Oberflaeche: nur wenn etwas darueberliegt, und nur alle 33 ms -
        // das Video darunter laeuft mit voller Bildrate weiter, die
        // Oberflaeche wird nicht mehr je Videobild neu gezeichnet.
        let (lage, wechsel) = {
            let s = self.shared.lock().unwrap();
            (s.error_key.is_some(), s.wechsel_laeuft())
        };
        let sichtbar = self.oberflaeche_sichtbar(lage, wechsel);
        let mut nach = None;
        if sichtbar {
            if self.ui_kasten_alt.is_none() || jetzt.duration_since(self.letzte_oberflaeche) >= Duration::from_millis(33) {
                // Der Puffer gehoert waehrend des Zeichnens der Canvas, nicht
                // dem App-Zustand - sonst kaeme oberflaeche_zeichnen nicht an
                // `&mut self`.
                let mut puffer = std::mem::take(&mut self.ui_puffer);
                if let Some(k) = self.ui_kasten_alt {
                    // Was beim letzten Mal dort stand, wieder durchsichtig.
                    let (x0, x1) = (k.x.max(0) as usize, (k.x + k.w).clamp(0, ww as i32) as usize);
                    for y in k.y.max(0)..(k.y + k.h).clamp(0, wh as i32) {
                        let z = y as usize * ww as usize;
                        puffer[z + x0..z + x1.max(x0)].fill(0xff00_0000);
                    }
                }
                let kasten_neu = {
                    let mut c = ui::Canvas::neu(&mut puffer, ww as usize, wh as usize);
                    nach = Some(self.oberflaeche_zeichnen(&mut c, ww, wh));
                    c.kasten_nehmen()
                };
                let hochladen = match kasten_vereinigen(self.ui_kasten_alt, kasten_neu) {
                    Some(k) => g.oberflaeche_hochladen(&puffer, ww, wh, k),
                    None => Ok(()),
                };
                self.ui_puffer = puffer;
                self.ui_kasten_alt = kasten_neu;
                self.ui_an = kasten_neu.is_some();
                self.letzte_oberflaeche = jetzt;
                if let Err(e) = hochladen {
                    self.ui_an = false;
                    self.gpu_fehler(g, e)?;
                }
            }
        } else {
            self.ui_an = false;
        }

        // Rechteck des Bildes im Fenster, dieselbe Rechnung wie blit. Ohne
        // Bild (Start, Warten) bleibt die Grundfarbe, die Oberflaeche deckt.
        let rect = match (&self.screen, self.bild_da) {
            (Screen::Session, Some((fw, fh))) => Some(ziel_rechteck(ww, wh, fw, fh, self.pixel_exact)),
            _ => None,
        };

        // Present nur, wenn DXGI bereit ist - nie darauf warten. Sonst beim
        // naechsten Takt, und das Bild zaehlt als ausgelassen.
        let ergebnis = if g.bereit() {
            let p = g.zeichnen(rect, self.ui_an, self.sofort);
            // Hat DXGI das Tearing im Lauf zurueckgenommen, sagt es auch
            // die Sekundenzeile.
            if !g.tearing {
                self.sofort = false;
            }
            match p {
                anzeige::Praesentiert::Ok | anzeige::Praesentiert::Verdeckt => {
                    self.praesentation_ausstehend = false;
                    // Anzeigezeit: Ablage durch den Empfangsfaden bis hinter
                    // Present - Takt, Upload, zwei Stufen, Uebergabe an DXGI.
                    if let Some(t) = self.bereit_ausstehend.take() {
                        let ms = client_us().saturating_sub(t) as f32 / 1000.0;
                        let mut s = self.shared.lock().unwrap();
                        s.anzeige_ms = if s.anzeige_ms > 0.0 { s.anzeige_ms + (ms - s.anzeige_ms) * 0.1 } else { ms };
                    }
                    Ok(())
                }
                anzeige::Praesentiert::Fehler(e) => {
                    // Das Bild liegt hochgeladen da; der naechste Takt zeigt
                    // es (nach einem Tearing-Rueckfall dann bildsynchron).
                    self.praesentation_ausstehend = true;
                    Err(Ausfall::Fehler(e))
                }
                anzeige::Praesentiert::GeraetWeg(grund) => Err(Ausfall::GeraetWeg(grund)),
            }
        } else {
            // Ausgelassen zaehlt Bilder, nicht 2-ms-Takte: nur beim Uebergang.
            if !self.praesentation_ausstehend {
                self.shared.lock().unwrap().ausgelassen += 1;
            }
            self.praesentation_ausstehend = true;
            Ok(())
        };
        if let Some(n) = nach {
            self.nachwirkung(n);
            self.ui.click = false;
        }
        ergebnis
    }

    /// Die Karte ist weg (Treiberupdate, Standby, Win+Strg+Shift+B): Grund
    /// ins Protokoll, einmal neu auf demselben Fenster - eine Flip-Swapchain
    /// nach einer Flip-Swapchain ist erlaubt. Kommt der Verlust binnen zehn
    /// Sekunden wieder oder scheitert der Neubau, bleibt das Fenster stehen
    /// und der Nutzer bekommt gesagt, wie es weitergeht. Auf softbuffer am
    /// selben Fenster wird nicht gewechselt: das ist von DXGI nicht gedeckt.
    #[cfg(windows)]
    fn geraet_verloren_behandeln(&mut self, alt: anzeige::Gpu, grund: String) {
        protokoll::zeile(format!("Anzeige: Grafikkarte verloren: {grund}"));
        drop(alt);
        self.bild_da = None;
        self.bereit_ausstehend = None;
        self.ui_kasten_alt = None;
        self.ui_an = false;
        self.praesentation_ausstehend = false;
        let schon = self.geraet_verloren.map(|t| t.elapsed() < Duration::from_secs(10)).unwrap_or(false);
        self.geraet_verloren = Some(Instant::now());
        if !schon {
            if let Some(w) = self.window.clone() {
                use einstellungen::AnzeigeWunsch as W;
                let s = w.inner_size();
                let adapter = self.adapter_index();
                let bau = fenster_hwnd(&w)
                    .ok_or_else(|| "kein Win32-Fenster".to_string())
                    .and_then(|h| anzeige::Gpu::neu(h, s.width, s.height, self.anzeige_wunsch == W::Warp, adapter));
                match bau {
                    Ok(g) => {
                        protokoll::zeile("Anzeige: Karte neu aufgebaut".into());
                        self.sofort = g.tearing;
                        self.anzeige = Anzeige::Gpu(g);
                        return;
                    }
                    Err(e) => protokoll::zeile(format!("Anzeige: Neubau fehlgeschlagen: {e}")),
                }
            }
        }
        let meldung = Meldung::neu(strings::Key::ErrorGpuLost, "Grafikkarte verloren - bitte mit --anzeige cpu neu starten");
        protokoll::zeile(format!("Anzeige: {}", meldung.protokoll));
        self.anzeige = Anzeige::Keine;
        let titel = format!("QuadChroma - {}", meldung.text(self.lang));
        let mut s = self.shared.lock().unwrap();
        s.gpu_pfad = false;
        s.error = Some(meldung);
        drop(s);
        if let Some(w) = &self.window {
            w.set_title(&titel);
        }
    }

    fn draw_cpu_auf(&mut self, window: &Window, surface: &mut softbuffer::Surface<Arc<Window>, Arc<Window>>) {
        let size = window.inner_size();
        let (ww, wh) = (size.width.max(1), size.height.max(1));
        // Schlaegt das fehl, liefert buffer_mut() spaeter einen Puffer der alten
        // Groesse - und jeder Schreibzugriff darauf reisst das Fenster mit.
        if surface
            .resize(
                std::num::NonZeroU32::new(ww).unwrap(),
                std::num::NonZeroU32::new(wh).unwrap(),
            )
            .is_err()
        {
            return;
        }
        // Der Takt der Oberflaeche haengt an der Uhr, nicht an der Zahl der
        // Zeichnungen: seit nur noch bei Bedarf gezeichnet wird, liefe das
        // Suchband des Startbildschirms sonst je nach Bildrate anders schnell.
        self.ui.tick = client_us() / 8000;
        self.letzte_zeichnung = Instant::now();

        // Neues Bild abholen, sonst das letzte weiterverwenden.
        let bild = self.shared.lock().unwrap().frame.take();
        let mut bereit_us = None;
        if let Some(b) = bild {
            bereit_us = Some(b.bereit_us());
            self.last_frame = Some(match b {
                Bild::Rgb(f) => f,
                // Ein rohes Bild kommt hier nur an, wenn die Anzeige im
                // laufenden Betrieb von der Karte auf die CPU zurueckfaellt:
                // dann einmal auf dem Fensterfaden wandeln.
                Bild::Roh { bild, bereit_us } => Frame {
                    bereit_us,
                    ..to_rgb(&bild).unwrap_or_else(|_| dunkles_bild(bild.width(), bild.height()))
                },
            });
            self.fps_count += 1;
            self.shown += 1;
        }
        self.sekundentakt(window);

        let Ok(mut buf) = surface.buffer_mut() else { return };
        if self.screen == Screen::Session {
            if let Some(frame) = &self.last_frame {
                blit(&mut buf, ww, wh, frame, self.pixel_exact);
            }
        }
        let n = {
            let mut c = ui::Canvas::neu(&mut buf, ww as usize, wh as usize);
            self.oberflaeche_zeichnen(&mut c, ww, wh)
        };
        buf.present().ok();
        // Anzeigezeit: von der Ablage durch den Empfangsfaden bis hinter
        // present() - Warten auf den 2-ms-Takt, blit, Oberflaeche, BitBlt.
        // Reine Client-Uhr, ohne die halbe Umlaufzeit der Host-Glieder. Was
        // danach auf dem Panel ankommt, misst nur eine Fotodiode.
        if let Some(t) = bereit_us {
            let ms = client_us().saturating_sub(t) as f32 / 1000.0;
            let mut s = self.shared.lock().unwrap();
            s.anzeige_ms = if s.anzeige_ms > 0.0 { s.anzeige_ms + (ms - s.anzeige_ms) * 0.1 } else { ms };
        }
        self.nachwirkung(n);
        self.ui.click = false;
    }

    /// Alles, was ueber dem Bild liegt: Startbildschirm, Wartebildschirm,
    /// Lagemeldung, Statistik, Erstkontakt-Banner, Menue, Codecwechsel-
    /// Hinweis, ESC-Balken. Zeichnet nur; was ein Klick bewirkt, kommt als
    /// Nachwirkung zurueck und wird NACH dem Praesentieren ausgefuehrt.
    fn oberflaeche_zeichnen(&mut self, c: &mut ui::Canvas, ww: u32, wh: u32) -> Nachwirkung {
        let mut n = Nachwirkung { act: Action::None, hud: None, abbrechen: false };
        match self.screen {
            Screen::Start => {
                let hosts = self
                    .hosts
                    .lock()
                    .map(|h| h.list())
                    .unwrap_or_default();
                let err = {
                    let s = self.shared.lock().unwrap();
                    match s.error_key {
                        Some(k) => Some(self.lang.get(k).to_string()),
                        None => s.error.as_ref().map(|m| m.text(self.lang)),
                    }
                };
                n.act = start_screen(&mut self.ui, c, self.lang, &hosts, &self.addr_input, err.as_deref());
            }
            Screen::Session => {
                // Noch kein Bild da: Wartebildschirm zeichnen statt gar nichts.
                if !self.bild_vorhanden() {
                    let stand = {
                        let s = self.shared.lock().unwrap();
                        let f = match s.error_key {
                            Some(k) => Some(self.lang.get(k).to_string()),
                            None => s.error.as_ref().map(|m| m.text(self.lang)),
                        };
                        (s.connected, f, s.sas.clone(), s.info.is_some())
                    };
                    let adresse = self.addr_input.clone();
                    n.abbrechen = warte_screen(&mut self.ui, c, self.lang, &adresse, stand);
                    return n;
                }
                // Lage des Hosts ueber dem stehenden Bild, falls er selbst
                // gerade nichts liefern kann.
                if let Some(k) = { self.shared.lock().unwrap().error_key } {
                    let t = self.lang.get(k);
                    let tw = self.ui.text.width(t, 14, 1);
                    c.fill(ww as i32 / 2 - tw / 2 - 20, 16, tw + 40, 40, ui::BG, 200);
                    self.ui.text.draw_centered(c, ww as i32 / 2, 42, t, 14, ui::AMBER, 1);
                }
                if self.show_overlay {
                    let (stats, secure, lat, soll, hostlast, decoder, ausgelassen) = {
                        let s = self.shared.lock().unwrap();
                        (
                            (s.last_decode_ms, s.info, s.dropped, s.error.as_ref().map(|m| m.text(self.lang)), s.connected),
                            (s.sas.clone(), s.peer_fp.clone()),
                            s.latenz(),
                            s.settings.map(|x| x.1 as u32).or_else(|| s.info.map(|i| i.fps)),
                            s.hostlast,
                            (s.decoder_pfad, s.decoder_hinweis.clone()),
                            s.ausgelassen,
                        )
                    };
                    let hist = self.fps_hist.clone();
                    let client = ClientStand {
                        anzeige: self.anzeige_name.clone(),
                        cpu_eigen: self.cpu_eigen,
                        monitor_hz: self.monitor_hz,
                        ausgelassen,
                    };
                    overlay(&mut self.ui, c, self.lang, self.fps_shown, &hist, stats, secure,
                            lat, self.cfg.stats, self.cfg.nerd, soll, hostlast, decoder, &client);
                }
                // Erstkontakt: Code gross anzeigen, solange es noch zaehlt.
                let (first, sas) = {
                    let s = self.shared.lock().unwrap();
                    (s.first_time, s.sas.clone())
                };
                if first && self.banner_until.is_none() {
                    self.banner_until = Some(Instant::now() + Duration::from_secs(25));
                }
                if let (Some(until), Some(sas)) = (self.banner_until, sas) {
                    if Instant::now() < until {
                        let bw = 520.min(ww as i32 - 40);
                        // Neben der Statistiktafel, nicht darueber: die ist im
                        // Nerd-Modus breit und mit der Decoder-Zeile hoch.
                        let frei_ab = if self.show_overlay { 18 + if self.cfg.nerd { 620 } else { 300 } + 20 } else { 0 };
                        let bx = ((ww as i32 - bw) / 2).max(frei_ab).min((ww as i32 - bw - 18).max(0));
                        let t = self.lang.get(strings::Key::FirstContact);
                        // Der Satz ist je nach Sprache verschieden lang, also
                        // erst messen, dann die Tafel darauf zuschneiden.
                        let zeilen = umbruch(&mut self.ui, t, bw - 48, 13);
                        let bh = 62 + zeilen.len() as i32 * 18;
                        let by = wh as i32 * 3 / 4 - bh / 2;
                        c.panel(bx, by, bw, bh, ui::AMBER);
                        let mut ty = by + 28;
                        for z in &zeilen {
                            self.ui.text.draw_centered(c, bx + bw / 2, ty, z, 13, ui::TEXT, 1);
                            ty += 18;
                        }
                        self.ui.text.draw_centered(c, bx + bw / 2, ty + 24, &sas, 30, ui::AMBER, 6);
                    }
                }
                // Nerd-Modus. Liegt ueber allem, deshalb zuletzt gezeichnet.
                if self.hud_offen {
                    // Der geltende Decoderwunsch kommt aus `shared`, nicht aus
                    // der Datei: mit --decoder auf der Befehlszeile weichen die
                    // beiden voneinander ab, und das Menue soll zeigen, was
                    // wirklich gilt - sonst ist der falsche Knopf in Cyan, und
                    // ein Klick auf den richtigen bewirkt nichts.
                    let (lat, info, stell, secure, codecs, codec_idx, wechsel, decoder_wunsch, decoder_aktiv) = {
                        let sh = self.shared.lock().unwrap();
                        (
                            sh.latenz(), sh.info, sh.settings, (sh.sas.clone(), sh.peer_fp.clone()),
                            sh.codecs.clone(), sh.codec_idx, sh.wechsel_laeuft(), sh.decoder_wunsch, sh.decoder_pfad,
                        )
                    };
                    let gespeichert = self
                        .angewandt_fuer
                        .as_ref()
                        .map(|f| self.cfg.fuer_host(f).is_some())
                        .unwrap_or(false);
                    let adresse = self.addr_input.clone();
                    let hist = self.fps_hist.clone();
                    let lhist = self.lat_hist.clone();
                    let reiter = self.hud_reiter;
                    let fps_jetzt = self.fps_shown;
                    let bench_scroll = self.bench_scroll_nachfuehren();
                    let stand = HudStand {
                        vollbild: self.fullscreen,
                        pixelgenau: self.pixel_exact,
                        statistik: self.show_overlay,
                        nerd: self.cfg.nerd,
                        wahl: self.cfg.stats,
                        codecs,
                        codec_idx,
                        wechsel,
                        decoder: decoder_wunsch,
                        decoder_aktiv,
                        karten: self.karten.clone(),
                        anzeige_aktiv: self.anzeige_aktiv,
                        anzeige_gespeichert: self.cfg.anzeige,
                        anzeige_name: self.anzeige_name.clone(),
                        bench_konfig: self.bench_konfig.clone(),
                        // Der Abzug kostet je Zeichnung ein paar Dutzend
                        // Ergebnisse - nur, wenn der Reiter offen ist.
                        bench: if reiter == 4 { self.benchmark.as_ref().map(|b| b.stand()) } else { None },
                        bench_scroll,
                    };
                    n.hud = Some(hud(
                        &mut self.ui, c, self.lang, ww as i32, wh as i32, reiter,
                        lat, &lhist, fps_jetzt, &hist, info, stell, secure, &adresse, gespeichert,
                        &stand,
                    ));
                    return n;
                }

                // Codecwechsel unterwegs: der Host baut den Encoder um, das
                // Bild steht ein paar hundert Millisekunden. Ohne Hinweis
                // saehe das nach einem Haenger aus.
                if self.shared.lock().unwrap().wechsel_laeuft() {
                    let t = self.lang.get(strings::Key::CodecSwitching);
                    let bw = (self.ui.text.width(t, 12, 2) + 60).max(220).min(ww as i32 - 40);
                    let bh = 44i32;
                    let bx = ww as i32 / 2 - bw / 2;
                    let by = wh as i32 - 110;
                    c.panel(bx, by, bw, bh, ui::AMBER);
                    self.ui.text.draw_centered(c, bx + bw / 2, by + 27, t, 12, ui::AMBER, 2);
                }

                // Rueckmeldung beim Halten von ESC. Erst ab einer Weile, damit
                // ein normaler Tipper nichts aufblitzen laesst - und ueberhaupt,
                // weil fuenf Sekunden ohne jede Anzeige von "abgestuerzt" nicht
                // zu unterscheiden sind.
                if let Some(t) = self.esc_seit {
                    let v = t.elapsed();
                    if v >= Duration::from_millis(400) && !self.esc_verbraucht {
                        let anteil = (v.as_secs_f32() / ESC_HALTEDAUER.as_secs_f32()).min(1.0);
                        let (bw, bh) = (340i32, 58i32);
                        let bx = ww as i32 / 2 - bw / 2;
                        let by = wh as i32 - 110;
                        c.panel(bx, by, bw, bh, ui::CYAN);
                        self.ui.text.draw_centered(
                            c, bx + bw / 2, by + 24,
                            self.lang.get(strings::Key::HoldEscHint), 12, ui::TEXT, 2,
                        );
                        c.rect(bx + 20, by + 38, bw - 40, 5, ui::DIM, 120);
                        let breit = ((bw - 40) as f32 * anteil) as i32;
                        c.rect(bx + 20, by + 38, breit.max(1), 5, ui::CYAN, 240);
                    }
                }
            }
        }
        n
    }

    /// Die Folgen eines Klicks, nach dem Praesentieren: Verbinden, Beenden,
    /// Sprache, Projektseite, Trennen und alles aus dem Menue.
    fn nachwirkung(&mut self, n: Nachwirkung) {
        match n.act {
            Action::Connect(addr) => {
                let addr = adresse_vollstaendig(&addr);
                self.addr_input = addr.clone();
                let input_addr = bump_port(&addr, 1);
                self.input.lock().unwrap().set_addr(input_addr);
                let mut s = self.shared.lock().unwrap();
                s.target = Some(addr);
                s.error = None;
                s.error_key = None;
                drop(s);
                self.screen = Screen::Session;
            }
            Action::Quit => self.quit = true,
            Action::NextLang => {
                let all = strings::all();
                let i = all.iter().position(|l| l.code == self.lang.code).unwrap_or(0);
                self.lang = all[(i + 1) % all.len()];
            }
            Action::Website => website_oeffnen(),
            Action::None => {}
        }
        if n.abbrechen {
            self.verbindung_trennen();
        }
        let Some(a) = n.hud else { return };
        match a {
            HudAktion::Reiter(r) => self.hud_reiter = r,
            HudAktion::Schalter(id) => {
                match id {
                    SCH_VOLLBILD => {
                        self.fullscreen = !self.fullscreen;
                        self.cfg.vollbild = self.fullscreen;
                        if let Some(w) = &self.window {
                            w.set_fullscreen(if self.fullscreen {
                                Some(winit::window::Fullscreen::Borderless(None))
                            } else {
                                None
                            });
                        }
                    }
                    SCH_PIXELGENAU => {
                        self.pixel_exact = !self.pixel_exact;
                        self.cfg.pixelgenau = self.pixel_exact;
                    }
                    SCH_STATISTIK => {
                        self.show_overlay = !self.show_overlay;
                        self.cfg.overlay = self.show_overlay;
                    }
                    SCH_NERD => self.cfg.nerd = !self.cfg.nerd,
                    SCH_STAT_FPS => self.cfg.stats.fps = !self.cfg.stats.fps,
                    SCH_STAT_LATENZ => self.cfg.stats.latenz = !self.cfg.stats.latenz,
                    SCH_STAT_TEILE => self.cfg.stats.teile = !self.cfg.stats.teile,
                    SCH_STAT_AUFL => self.cfg.stats.aufloesung = !self.cfg.stats.aufloesung,
                    SCH_STAT_CODEC => self.cfg.stats.codec = !self.cfg.stats.codec,
                    SCH_STAT_VERW => self.cfg.stats.verworfen = !self.cfg.stats.verworfen,
                    SCH_STAT_CODE => self.cfg.stats.code = !self.cfg.stats.code,
                    _ => {}
                }
                self.cfg.sichern();
            }
            HudAktion::Trennen => self.verbindung_trennen(),
            HudAktion::Stellen(m, f, g, fx, ton) => self.stellen(m, f, g, fx, ton),
            HudAktion::Codec(idx) => self.codec_wuenschen(idx),
            // --- Benchmark: Konfiguration nur, solange keiner laeuft -----
            HudAktion::BenchCodec(idx) => {
                let aus = &mut self.bench_konfig.codecs_aus;
                match aus.iter().position(|&i| i == idx) {
                    Some(p) => { aus.remove(p); }
                    None => aus.push(idx),
                }
            }
            HudAktion::BenchMbit(m) => {
                let drin = self.bench_konfig.mbits.contains(&m);
                // Aus der festen Reihe neu aufbauen, damit die Reihenfolge
                // der Schritte immer die der Knoepfe ist.
                self.bench_konfig.mbits = BENCH_MBITS
                    .iter()
                    .copied()
                    .filter(|&x| if x == m { !drin } else { self.bench_konfig.mbits.contains(&x) })
                    .collect();
            }
            HudAktion::BenchFps(f) => {
                let drin = self.bench_konfig.fpss.contains(&f);
                self.bench_konfig.fpss = BENCH_FPSS
                    .iter()
                    .copied()
                    .filter(|&x| if x == f { !drin } else { self.bench_konfig.fpss.contains(&x) })
                    .collect();
            }
            HudAktion::BenchDauer(d) => {
                self.bench_konfig.dauer_s = (self.bench_konfig.dauer_s as i32 + d).clamp(3, 15) as u32;
            }
            HudAktion::BenchTestbild => self.bench_konfig.testbild = !self.bench_konfig.testbild,
            HudAktion::BenchStart => self.benchmark_starten(),
            HudAktion::BenchAbbruch => self.benchmark_abbrechen(true),
            HudAktion::BenchUebernehmen(idx, m, f) => {
                // Wie ein Klick auf den Codec und die Steller im Reiter
                // "Bild": Wunsch an den Host, Werte fuer diesen Host merken.
                let (jetzt, ton) = {
                    let s = self.shared.lock().unwrap();
                    (s.codec_idx, s.settings.map(|x| x.4).unwrap_or(s.ton))
                };
                if jetzt != Some(idx) {
                    self.codec_wuenschen(idx);
                }
                self.stellen(m, f, false, true, ton);
            }
            HudAktion::Decoder(w) => {
                // Merken, sichern, und dem Empfangsfaden Bescheid
                // geben - der baut den Decoder um, ohne die
                // Verbindung anzufassen. Verglichen wird mit dem
                // geltenden Wunsch aus `shared` (das Menue zeigt den, nicht
                // die Datei); erst der Klick macht ihn zur gespeicherten
                // Wahl - ein blosses --decoder schreibt nie in die Datei.
                let geltend = self.shared.lock().unwrap().decoder_wunsch;
                if geltend != w {
                    self.cfg.decoder = w;
                    self.cfg.sichern();
                    let mut sh = self.shared.lock().unwrap();
                    sh.decoder_wunsch = w;
                    sh.decoder_wunsch_neu = true;
                }
            }
            HudAktion::Anzeige(w) => {
                // Nur speichern: die Anzeige wird im Lauf nicht umgebaut
                // (Swapchain und softbuffer am selben Fenster vertragen sich
                // nicht). Das Menue zeigt "gilt ab dem naechsten Start",
                // solange Datei und laufende Anzeige auseinanderliegen.
                if self.cfg.anzeige != w {
                    self.cfg.anzeige = w;
                    self.cfg.sichern();
                    protokoll::zeile(format!("Anzeige: {} gespeichert, gilt ab dem naechsten Start", w.schluessel()));
                }
            }
            HudAktion::Nichts => {}
        }
    }
}

/// Was das Zeichnen der Oberflaeche nach sich zieht: Klicks, die erst NACH
/// dem Praesentieren ausgefuehrt werden.
struct Nachwirkung {
    /// Startbildschirm.
    act: Action,
    /// Menue, falls es offen war.
    hud: Option<HudAktion>,
    /// Wartebildschirm: "Trennen" gedrueckt.
    abbrechen: bool,
}

/// Was zwischen dem Klick auf einen Host und dem ersten Bild zu sehen ist.
/// Ohne diesen Schritt bleibt der Startbildschirm einfach stehen und das
/// Programm wirkt eingefroren - es tut in Wahrheit genau das Richtige, sagt es
/// nur niemandem. Gibt true zurueck, wenn abgebrochen werden soll.
fn warte_screen(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    addr: &str,
    stand: (bool, Option<String>, Option<String>, bool),
) -> bool {
    use strings::Key::*;
    let (verbunden, fehler, sas, hat_info) = stand;
    c.backdrop(u.tick);
    let cx = c.w as i32 / 2;
    let cy = c.h as i32 / 2;

    u.text.draw_centered(c, cx, cy - 80, "QUADCHROMA", 26, ui::CYAN, 8);
    u.text.draw_centered(c, cx, cy - 46, addr, 14, ui::TEXT, 2);

    // Der Stand in Worten. Jede Stufe hat ihren eigenen Text, damit man sieht,
    // wo es haengt, statt nur dass es haengt.
    let (text, farbe) = if let Some(e) = &fehler {
        (e.clone(), ui::AMBER)
    } else if hat_info {
        (lang.get(Connected).to_string(), ui::CYAN)
    } else if sas.is_some() {
        (format!("{} · {}", lang.get(Encryption), lang.get(EncryptionOn)), ui::CYAN)
    } else if verbunden {
        (lang.get(Connecting).to_string(), ui::DIM)
    } else {
        (lang.get(Connecting).to_string(), ui::DIM)
    };
    for (i, z) in umbruch(u, &text, c.w as i32 - 120, 14).iter().enumerate() {
        u.text.draw_centered(c, cx, cy + 4 + i as i32 * 20, z, 14, farbe, 1);
    }

    // Laufender Balken. Er zeigt nur, dass das Programm lebt - mehr soll er
    // auch nicht behaupten, denn wie weit es ist, weiss niemand.
    let bw = 320.min(c.w as i32 - 80);
    let bx = cx - bw / 2;
    let by = cy + 54;
    c.rect(bx, by, bw, 3, ui::DIM, 40);
    let t = (u.tick % 120) as f32 / 120.0;
    let lauf = ((t * std::f32::consts::PI * 2.0).sin() * 0.5 + 0.5) * (bw - 70) as f32;
    c.rect(bx + lauf as i32, by, 70, 3, ui::CYAN, 220);

    if let Some(s) = &sas {
        u.text.draw_centered(c, cx, cy + 96, &format!("{} {}", lang.get(SecuredWith), s), 12, ui::DIM, 1);
    }

    let br = ui::Rect { x: cx - 90, y: c.h as i32 - 90, w: 180, h: 40 };
    u.button(c, br, lang.get(Disconnect), ui::MAGENTA)
}

enum Action {
    None,
    Connect(String),
    Quit,
    NextLang,
    Website,
}

/// Fehlt der Port, ergaenzen wir den Standard. Frueher galt das nur fuer die
/// Befehlszeile - wer die Adresse eintippte, landete bei einem Verbindungs-
/// fehler, den niemand erklaeren konnte.
fn adresse_vollstaendig(a: &str) -> String {
    let a = a.trim();
    if a.is_empty() || a.contains(':') {
        a.to_string()
    } else {
        format!("{a}:9001")
    }
}

/// Port um n erhoehen: 9001 wird zu 9002 fuer den Eingabekanal.
fn bump_port(addr: &str, n: u16) -> String {
    match addr.rsplit_once(':') {
        Some((h, p)) => format!("{h}:{}", p.parse::<u16>().unwrap_or(9001).saturating_add(n)),
        None => format!("{addr}:{}", 9001 + n),
    }
}

/// Wo das Bild im Fenster liegt: (x, y, Breite, Hoehe). Bei 1:1 mittig und
/// unskaliert - ist das Bild groesser als das Fenster, liegt es oben links
/// an und ragt hinaus (blit beschneidet, die Karte spaeter ueber den
/// Viewport). Sonst eingepasst statt verzerrt: das Bild behaelt sein
/// Seitenverhaeltnis und bekommt Balken, wo das Fenster nicht passt. Vorher
/// wurde schlicht auf die Fenstergroesse gezogen - in einem nicht 16:9
/// grossen Fenster war der Mac-Bildschirm dadurch sichtbar verzerrt.
///
/// EINE Ganzzahlrechnung fuer blit und - sobald die Karte zeichnet - fuer
/// deren Stufe 2, damit beide das Bild an dieselbe Stelle legen.
pub fn ziel_rechteck(ww: u32, wh: u32, fw: u32, fh: u32, pixelgenau: bool) -> (i32, i32, u32, u32) {
    let (winw, winh, fw, fh) = (ww as usize, wh as usize, fw as usize, fh as usize);
    if fw == 0 || fh == 0 {
        return (0, 0, 0, 0);
    }
    if pixelgenau {
        let ox = winw.saturating_sub(fw) / 2;
        let oy = winh.saturating_sub(fh) / 2;
        return (ox as i32, oy as i32, fw as u32, fh as u32);
    }
    let breit = (winw * fh) >= (winh * fw);
    let (zw, zh) = if breit {
        let h = winh;
        ((fw * h + fh / 2) / fh, h)
    } else {
        let w = winw;
        (w, (fh * w + fw / 2) / fw)
    };
    let zw = zw.max(1).min(winw);
    let zh = zh.max(1).min(winh);
    let ox = (winw - zw) / 2;
    let oy = (winh - zh) / 2;
    (ox as i32, oy as i32, zw as u32, zh as u32)
}

fn blit(buf: &mut [u32], ww: u32, wh: u32, frame: &Frame, pixel_exact: bool) {
    if ww == frame.width && wh == frame.height {
        buf.copy_from_slice(&frame.pixels);
        return;
    }
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    if fw == 0 || fh == 0 {
        return;
    }
    let (ox, oy, zw, zh) = ziel_rechteck(ww, wh, frame.width, frame.height, pixel_exact);
    let (ox, oy, zw, zh) = (ox as usize, oy as usize, zw as usize, zh as usize);

    if pixel_exact {
        buf.fill(0x05070d);
        let cols = zw.min(ww as usize);
        let rows = zh.min(wh as usize);
        buf.par_chunks_mut(ww as usize).enumerate().for_each(|(y, row)| {
            if y >= oy && y < oy + rows {
                let sy = y - oy;
                row[ox..ox + cols].copy_from_slice(&frame.pixels[sy * fw..sy * fw + cols]);
            }
        });
        return;
    }

    let winw = ww as usize;
    buf.fill(0x05070d);

    // Weiche Abtastung in 16.16-Festkomma. Beim Verkleinern ist das der
    // Unterschied zwischen lesbarer und flimmernder Schrift - und Schrift ist
    // bei einer Fernsteuerung der halbe Inhalt.
    let sx_step = ((fw as u64) << 16) / zw as u64;
    let sy_step = ((fh as u64) << 16) / zh as u64;

    buf.par_chunks_mut(winw).enumerate().for_each(|(y, row)| {
        if y < oy || y >= oy + zh {
            return;
        }
        let fy = ((y - oy) as u64 * sy_step) as u64;
        let sy0 = (fy >> 16) as usize;
        let sy1 = (sy0 + 1).min(fh - 1);
        let wy = (fy & 0xffff) as u32;

        let z0 = sy0 * fw;
        let z1 = sy1 * fw;
        for x in 0..zw {
            let fx = x as u64 * sx_step;
            let sx0 = (fx >> 16) as usize;
            let sx1 = (sx0 + 1).min(fw - 1);
            let wx = (fx & 0xffff) as u32;

            let a = frame.pixels[z0 + sx0];
            let b = frame.pixels[z0 + sx1];
            let c = frame.pixels[z1 + sx0];
            let d = frame.pixels[z1 + sx1];
            row[ox + x] = mische(a, b, c, d, wx, wy);
        }
    });
}

/// Vier Bildpunkte gewichtet mischen. Die Gewichte kommen als 16-Bit-Anteile.
#[inline]
fn mische(a: u32, b: u32, c: u32, d: u32, wx: u32, wy: u32) -> u32 {
    let (wx, wy) = (wx as u64, wy as u64);
    let kanal = |sch: u32| -> u32 {
        let a = ((a >> sch) & 0xff) as u64;
        let b = ((b >> sch) & 0xff) as u64;
        let c = ((c >> sch) & 0xff) as u64;
        let d = ((d >> sch) & 0xff) as u64;
        let oben = a * (65536 - wx) + b * wx;
        let unten = c * (65536 - wx) + d * wx;
        (((oben * (65536 - wy) + unten * wy) >> 32) & 0xff) as u32
    };
    (kanal(16) << 16) | (kanal(8) << 8) | kanal(0)
}

/// Startbildschirm: Titel, gefundene Hosts, Adresse, Knoepfe.
fn start_screen(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    hosts: &[discovery::Host],
    addr: &str,
    error: Option<&str>,
) -> Action {
    use strings::Key::*;
    c.backdrop(u.tick);

    let cx = c.w as i32 / 2;
    let panel_w = 560.min(c.w as i32 - 60);
    let px = cx - panel_w / 2;

    // Alles als Block mittig setzen, damit unten kein totes Feld bleibt.
    let list_h = 34 * 4 + 52;
    let block_h = 150 + list_h + 40 + 66 + 44;
    let top = ((c.h as i32 - block_h) / 2).max(24);

    // Kopf
    u.text.draw_centered(c, cx, top + 52, "QUADCHROMA", 46, ui::CYAN, 10);
    c.glow_hline(cx - 160, top + 64, 320, ui::MAGENTA);
    u.text.draw_centered(c, cx, top + 88, lang.get(AppSubtitle), 13, ui::DIM, 2);

    // Hostliste
    let py = top + 130;
    c.panel(px, py, panel_w, list_h, ui::CYAN);
    let title = if hosts.is_empty() { lang.get(SearchingHosts) } else { lang.get(FoundHosts) };
    u.text.draw(c, px + 18, py + 28, title, 13, ui::DIM, 3);
    if hosts.is_empty() {
        let dots = (u.tick / 20) % 4;
        let s = ".".repeat(dots as usize);
        let tw = u.text.width(title, 13, 3);
        u.text.draw(c, px + 18 + tw + 8, py + 28, &s, 13, ui::CYAN, 3);
        u.text.draw_centered(c, cx, py + list_h / 2 + 16, lang.get(NoHostsFound), 13, ui::DIM, 1);
    }

    let mut action = Action::None;
    for (i, h) in hosts.iter().take(4).enumerate() {
        let r = ui::Rect { x: px + 12, y: py + 44 + i as i32 * 34, w: panel_w - 24, h: 30 };
        let sel = addr == h.addr.to_string();
        if u.row(c, r, &h.name, &h.addr.to_string(), sel) {
            action = Action::Connect(h.addr.to_string());
        }
    }

    // Adressfeld
    let fy = py + list_h + 36;
    u.field(c, ui::Rect { x: px, y: fy, w: panel_w, h: 40 }, addr, lang.get(HostAddress), true);

    // Knoepfe
    let by = fy + 62;
    let bw = (panel_w - 20) / 2;
    if u.button(c, ui::Rect { x: px, y: by, w: bw, h: 44 }, lang.get(Connect), ui::CYAN) && !addr.is_empty() {
        action = Action::Connect(addr.to_string());
    }
    if u.button(c, ui::Rect { x: px + bw + 20, y: by, w: bw, h: 44 }, lang.get(Quit), ui::MAGENTA) {
        action = Action::Quit;
    }

    // Meldungen mit Pfad oder Fingerabdruck sind laenger als eine Zeile -
    // umbrechen statt am Fensterrand abschneiden.
    if let Some(e) = error {
        for (i, z) in umbruch(u, e, c.w as i32 - 60, 13).iter().enumerate() {
            u.text.draw_centered(c, cx, by + 76 + i as i32 * 18, z, 13, ui::AMBER, 1);
        }
    }

    // Fussleiste: zwei Zeilen. Oben die Urheberzeile, darunter Version,
    // Tastenhinweise und die Sprache zum Durchschalten.
    let fy2 = c.h as i32 - 22;
    c.hline(0, fy2 - 38, c.w as i32, ui::CYAN, 30);
    // Urheberzeile und Projektseite nebeneinander, als Block mittig. Nur die
    // Adresse ist anklickbar; sie oeffnet den Browser des Systems.
    {
        let cw = u.text.width(COPYRIGHT, 11, 2);
        let ww_ = u.text.width(WEBSITE, 11, 2);
        let luecke = 22;
        let gesamt = cw + luecke + ww_;
        let x0 = cx - gesamt / 2;
        let wy = fy2 - 16;
        u.text.draw(c, x0, wy, COPYRIGHT, 11, ui::DIM, 2);
        let wx = x0 + cw + luecke;
        let r = ui::Rect { x: wx - 6, y: wy - 12, w: ww_ + 12, h: 18 };
        let hot = r.hit(u.mouse.0, u.mouse.1);
        u.text.draw(c, wx, wy, WEBSITE, 11, if hot { ui::CYAN } else { ui::DIM }, 2);
        c.hline(wx, wy + 4, ww_, if hot { ui::CYAN } else { ui::DIM }, if hot { 200 } else { 70 });
        if hot && u.click {
            action = Action::Website;
        }
    }
    u.text.draw(c, 20, fy2, "v0.1", 11, ui::DIM, 2);
    let hints = format!(
        "F9 {}   F10 {}   F11 {}   F12 {}",
        lang.get(ShowOverlay), lang.get(Settings), lang.get(Fullscreen), lang.get(PixelExact)
    );
    u.text.draw_centered(c, cx, fy2, &hints, 11, ui::DIM, 1);
    let lw = u.text.width(lang.name, 12, 2);
    let lr = ui::Rect { x: c.w as i32 - lw - 34, y: fy2 - 16, w: lw + 22, h: 22 };
    let hot = lr.hit(u.mouse.0, u.mouse.1);
    u.text.draw(c, lr.x + 10, fy2, lang.name, 12, if hot { ui::CYAN } else { ui::DIM }, 2);
    if hot {
        c.hline(lr.x, lr.y + lr.h, lr.w, ui::CYAN, 160);
        if u.click { action = Action::NextLang; }
    }
    action
}

/// Zahlen waehrend der Sitzung, oben links, halbtransparent.
/// Beschriftung links, Wert rechts. Passt beides nicht in eine Zeile, rutscht
/// der Wert eine Zeile tiefer - lieber zwei Zeilen als uebereinander gedruckt.
fn label_value(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    x: i32,
    w: i32,
    ty: i32,
    label: &str,
    value: &str,
    col: u32,
) -> i32 {
    let lw = u.text.width(label, 12, 1);
    let vw = u.text.width(value, 13, 1);
    u.text.draw(c, x + 16, ty, label, 12, ui::DIM, 1);
    if lw + vw + 44 <= w {
        u.text.draw(c, x + w - vw - 16, ty, value, 13, col, 1);
        22
    } else {
        u.text.draw(c, x + w - vw - 16, ty + 18, value, 13, col, 1);
        40
    }
}

/// Text auf eine Breite kuerzen, mit Auslassungszeichen am Ende - fuer
/// Spaltenkoepfe, die in einer Sprache laenger sind als ihre Spalte.
fn kuerzen(u: &mut ui::Ui, t: &str, breite: i32, size: u32, spacing: i32) -> String {
    if u.text.width(t, size, spacing) <= breite {
        return t.to_string();
    }
    let zeichen: Vec<char> = t.chars().collect();
    for n in (1..zeichen.len()).rev() {
        let probe: String = zeichen[..n].iter().collect::<String>().trim_end().to_string() + "…";
        if u.text.width(&probe, size, spacing) <= breite {
            return probe;
        }
    }
    "…".into()
}

/// Text in Zeilen schneiden, die in die vorgegebene Breite passen.
fn umbruch(u: &mut ui::Ui, t: &str, breite: i32, size: u32) -> Vec<String> {
    let mut zeilen = Vec::new();
    let mut line = String::new();
    for wort in t.split(' ') {
        let probe = if line.is_empty() { wort.to_string() } else { format!("{line} {wort}") };
        if u.text.width(&probe, size, 1) > breite && !line.is_empty() {
            zeilen.push(std::mem::take(&mut line));
            line = wort.to_string();
        } else {
            line = probe;
        }
    }
    if !line.is_empty() { zeilen.push(line); }
    zeilen
}


/// Farbe des Glieds "Anzeige" in der Latenzkette und des Monitor-Markers.
const FARBE_ANZEIGE: u32 = 0x7a8cff;

/// Was der Client selbst ueber seine Anzeige weiss - fuer die Statistik.
struct ClientStand {
    /// "Software", spaeter "D3D11 · <Adapter> · Sofort/Bildsynchron".
    anzeige: String,
    /// Eigene Prozessorlast in Prozent der ganzen Maschine.
    cpu_eigen: f32,
    /// Bildwiederholrate des Monitors, auf dem das Fenster steht.
    monitor_hz: Option<f32>,
    /// Ausgelassene Praesentationen (nur auf dem Weg ueber die Karte).
    ausgelassen: u64,
}

/// Die Latenzkette auf FESTER Millisekundenskala. Dadurch heisst die Laenge
/// des Balkens "wie langsam" und die Unterteilung "woran liegt es". Ein
/// Balken, der sich an den eigenen Wert anpasst, sieht bei 12 ms genauso aus
/// wie bei 120.
#[allow(clippy::too_many_arguments)]
fn kette(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    ix: i32,
    y: i32,
    iw: i32,
    l: Latenz,
    soll_fps: Option<u32>,
    monitor_hz: Option<f32>,
    s: f32,
) -> i32 {
    use strings::Key::*;
    let p = |v: i32| -> i32 { (v as f32 * s).round() as i32 };
    let sz = |v: u32| -> u32 { (v as f32 * s).round() as u32 };

    let ein_bild = soll_fps.map(|f| 1000.0 / f.max(1) as f32);
    let monitor_bild = monitor_hz.filter(|hz| *hz > 0.0).map(|hz| 1000.0 / hz);
    // Alle fuenf Glieder, bis zur Uebergabe ans Fenster.
    let gesamt = l.bis_anzeige();
    let ende = skalenende(
        (gesamt * 1.35).max(ein_bild.unwrap_or(0.0) * 1.5).max(monitor_bild.unwrap_or(0.0) * 1.2).max(10.0),
    );
    let spur_y = y + p(22);
    let spur_h = p(20);
    let spur_ende = ix + iw;
    u.text.draw_right(c, spur_ende, y + p(12), &format!("{ende:.0} ms"), sz(10), ui::DIM, p(1));
    c.rect(ix, spur_y, iw, spur_h, ui::DIM, 18);

    let je_ms = iw as f32 / ende;
    let rest = (l.gesamt_ms - (l.encoder_ms + l.leitung_ms + l.decoder_ms)).max(0.0);
    let teile = [
        (lang.get(Queue), rest, ui::DIM),
        (lang.get(EncodeTime), l.encoder_ms.max(0.0), ui::MAGENTA),
        (lang.get(NetworkTime), l.leitung_ms.max(0.0), ui::CYAN),
        (lang.get(DecodeTime), l.decoder_ms.max(0.0), ui::TEXT),
        (lang.get(DisplayStage), l.anzeige_ms.max(0.0), FARBE_ANZEIGE),
    ];
    let zu_langsam = ein_bild.map(|b| gesamt > b).unwrap_or(false);

    let mut cursor = ix;
    let mut abgeschnitten = false;
    for (_, wert, farbe) in teile.iter() {
        let bw = (wert * je_ms) as i32;
        // Das Ende der Spur ist hart, sonst ragt der Balken aus der Tafel.
        let sichtbar = (spur_ende - cursor).min(bw);
        if sichtbar > 1 {
            c.rect(cursor, spur_y, sichtbar - 2, spur_h, *farbe, 210);
            c.hline(cursor, spur_y, sichtbar - 2, *farbe, 255);
        }
        if bw > sichtbar { abgeschnitten = true; }
        cursor = (cursor + bw).min(spur_ende);
    }
    if abgeschnitten {
        // Ein stillschweigend gekappter Balken behauptet, es sei weniger.
        for d in 0..3 {
            c.vline(spur_ende - 1 - d, spur_y, spur_h, ui::AMBER, (255 - d * 60) as u32);
        }
    }
    if l.umlauf_ms > 0.0 {
        let mitte = (ix + (gesamt * je_ms) as i32).min(spur_ende);
        let halb = ((l.umlauf_ms / 2.0) * je_ms) as i32;
        let von = (mitte - halb).max(ix);
        let bis = (mitte + halb).min(spur_ende);
        if bis > von { c.rect(von, spur_y, bis - von, spur_h, ui::TEXT, 45); }
        c.vline(mitte, spur_y - p(4), spur_h + p(8), ui::TEXT, 255);
    }
    // Zwei Marken: ein Bild des Stroms (Magenta) und ein Bild des Monitors
    // (Anzeigefarbe). Bei 120 Bildern je Sekunde auf einem 60-Hz-Fenster ist
    // jedes zweite Bild verworfen - das ist kein Fehler, und die zweite Marke
    // sagt, warum. Die Beschriftung des linken Strichs steht links von ihm,
    // die des rechten rechts, damit sie sich auch bei nahen Raten nie
    // ueberdecken; mit nur einer Marke bleibt es wie bisher (rechts).
    let mut marken: Vec<(f32, String, u32)> = Vec::new();
    if let Some(b) = ein_bild {
        marken.push((b, format!("{} · {b:.1} ms", lang.get(OneFrame)), ui::MAGENTA));
    }
    if let Some(m) = monitor_bild {
        marken.push((m, format!("{} · {m:.1} ms", lang.get(Monitor)), FARBE_ANZEIGE));
    }
    let linke = if marken.len() == 2 && marken[1].0 < marken[0].0 { 1 } else { 0 };
    for (i, (ms, text, farbe)) in marken.iter().enumerate() {
        let mx = (ix + (ms * je_ms) as i32).min(spur_ende);
        c.glow_vline(mx, spur_y - p(5), spur_h + p(10), *farbe);
        let ty = spur_y + spur_h + p(26);
        if marken.len() == 2 && i == linke {
            let tw = u.text.width(text, sz(10), p(1));
            u.text.draw_right(c, (mx - p(6)).max(ix + tw), ty, text, sz(10), ui::DIM, p(1));
        } else {
            u.text.draw(c, (mx + p(6)).min(spur_ende - p(90)), ty, text, sz(10), ui::DIM, p(1));
        }
    }
    let schritt = if je_ms * 2.0 >= 14.0 { 2.0 } else { 10.0 };
    let mut t = 0.0;
    while t <= ende {
        let tx = (ix + (t * je_ms) as i32).min(spur_ende);
        c.vline(tx, spur_y + spur_h, p(4), ui::DIM, 120);
        u.text.draw_centered(c, tx, spur_y + spur_h + p(16), &format!("{t:.0}"), sz(9), ui::DIM, 0);
        t += schritt;
    }

    let spalte = iw / 5;
    let ly = spur_y + spur_h + p(46);
    for (i, (name, wert, farbe)) in teile.iter().enumerate() {
        let lx = ix + i as i32 * spalte;
        c.rect(lx, ly - p(9), p(9), p(9), *farbe, 230);
        let gross = zu_langsam && *wert > gesamt / 2.0;
        u.text.draw(c, lx + p(14), ly, name, sz(10), if gross { ui::AMBER } else { ui::DIM }, p(1));
        u.text.draw(c, lx + p(14), ly + p(17), &format!("{wert:.1} ms"), sz(12),
                    if gross { ui::AMBER } else { ui::TEXT }, p(1));
        if gross {
            u.text.draw(c, lx + p(14), ly + p(33), lang.get(Bottleneck), sz(9), ui::AMBER, p(2));
        }
    }
    (ly + p(44)) - y
}

/// Die Statistik im Bild. Welche Zeilen erscheinen, bestimmt der Benutzer im
/// Menue - manchem reicht die Bildrate. Ist der Nerd-Modus an, wird die Tafel
/// breiter und bekommt die Latenzkette dazu.
#[allow(clippy::too_many_arguments)]
fn overlay(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    fps: f32,
    hist: &[f32],
    stats: (f32, Option<StreamInfo>, u64, Option<String>, bool),
    secure: (Option<String>, Option<String>),
    lat: Option<Latenz>,
    wahl: einstellungen::StatWahl,
    nerd: bool,
    soll_fps: Option<u32>,
    hostlast: Option<HostLast>,
    decoder: (Option<DecoderPfad>, Option<String>),
    client: &ClientStand,
) {
    use strings::Key::*;
    let (dec_ms, info, dropped, err, connected) = stats;
    let (sas, _peer_fp) = secure;

    let mut rows: Vec<(&str, String, u32)> = Vec::new();
    if wahl.fps {
        rows.push((lang.get(Fps), format!("{fps:.0}"), if fps > 50.0 { ui::CYAN } else { ui::AMBER }));
    }
    if wahl.latenz {
        if let Some(l) = lat {
            if l.gesamt_ms > 0.0 {
                // Alle fuenf Glieder, bis zur Uebergabe ans Fenster.
                rows.push((
                    lang.get(Latency),
                    format!("{:.1} ±{:.1} ms", l.bis_anzeige(), l.umlauf_ms / 2.0),
                    if l.bis_anzeige() < 40.0 { ui::CYAN } else { ui::AMBER },
                ));
            }
        }
    }
    if wahl.teile {
        if let Some(l) = lat {
            if l.gesamt_ms > 0.0 && !nerd {
                rows.push((lang.get(EncodeTime), format!("{:.1} ms", l.encoder_ms), ui::TEXT));
                rows.push((lang.get(NetworkTime), format!("{:.1} ms", l.leitung_ms), ui::TEXT));
            }
        }
        rows.push((lang.get(DecodeTime), format!("{dec_ms:.1} ms"), ui::TEXT));
    }
    if let Some(i) = info {
        if wahl.aufloesung {
            rows.push((lang.get(Resolution), format!("{}x{}", i.width, i.height), ui::TEXT));
        }
        if wahl.codec {
            rows.push((lang.get(Codec), i.codec_name(), ui::TEXT));
        }
    }
    // Welcher Decoder das Bild macht: Hardware in Cyan. Im Nerd-Modus steht
    // dazu, warum es nicht die Karte wurde, falls sie gewuenscht war.
    if let Some(pfad) = decoder.0 {
        let txt = match (&decoder.1, nerd) {
            (Some(g), true) => format!("{} · {g}", pfad.name()),
            _ => pfad.name().to_string(),
        };
        rows.push((lang.get(DecoderLabel), txt, if pfad.hardware() { ui::CYAN } else { ui::TEXT }));
    }
    if wahl.verworfen {
        rows.push((lang.get(Dropped), format!("{dropped}"), ui::TEXT));
    }
    if wahl.code {
        if let Some(sas) = &sas {
            rows.push((lang.get(SecuredWith), sas.clone(), ui::CYAN));
        }
    }
    // Im Nerd-Modus dazu, was der Client selbst tut - und was der Mac
    // gerade zu tun hat.
    if nerd {
        rows.push((lang.get(DisplayLabel), client.anzeige.clone(), ui::TEXT));
        rows.push((lang.get(ClientCpu), format!("{:.1} %", client.cpu_eigen), ui::TEXT));
        // Kennt winit die Rate nicht, steht hier nichts.
        if let Some(hz) = client.monitor_hz {
            rows.push((lang.get(Monitor), format!("{hz:.0} Hz"), ui::TEXT));
        }
        rows.push((lang.get(Skipped), format!("{}", client.ausgelassen), ui::TEXT));
        if let Some(hl) = hostlast {
            rows.push((
                lang.get(HostCpu),
                format!("{:.0} %  ({:.0} % QuadChroma)", hl.cpu, hl.cpu_eigen),
                if hl.cpu > 85.0 { ui::AMBER } else { ui::TEXT },
            ));
            // Fehlt der Zaehler, steht hier nichts. Eine erfundene Null waere
            // schlimmer als eine fehlende Zeile.
            if let Some(g) = hl.gpu {
                rows.push((lang.get(HostGpu), format!("{g:.0} %"), ui::TEXT));
            }
            if hl.ram_gesamt_mb > 0 {
                rows.push((
                    lang.get(HostRam),
                    format!("{:.1} / {:.0} GB", hl.ram_benutzt_mb as f32 / 1024.0, hl.ram_gesamt_mb as f32 / 1024.0),
                    if hl.druck >= 4 { ui::AMBER } else { ui::TEXT },
                ));
            }
            // Die Video-Einheit meldet ihre Auslastung nirgends. Das hier ist
            // der ehrliche Ersatz: gebrauchte Zeit gegen verfuegbare Zeit.
            if hl.encoder_ms > 0.0 {
                let budget = soll_fps.map(|f| 1000.0 / f.max(1) as f32);
                let txt = match budget {
                    Some(b) => format!("{:.1} / {:.1} ms", hl.encoder_ms, b),
                    None => format!("{:.1} ms", hl.encoder_ms),
                };
                let eng = budget.map(|b| hl.encoder_ms > b).unwrap_or(false);
                rows.push((lang.get(EncodeTime), txt, if eng { ui::AMBER } else { ui::TEXT }));
            }
        }
    }

    let (x, y) = (18, 18);
    let w = if nerd { 620 } else { 300 };
    let mut need = 0;
    for (k, v, _) in &rows {
        need += if u.text.width(k, 12, 1) + u.text.width(v, 13, 1) + 44 <= w { 22 } else { 40 };
    }
    let kette_h = if nerd && lat.map(|l| l.gesamt_ms > 0.0).unwrap_or(false) { 150 } else { 0 };
    let h = 30 + need + kette_h + 42;
    if rows.is_empty() && kette_h == 0 {
        return;
    }
    c.panel(x, y, w, h, ui::CYAN);

    let mut ty = y + 30;
    for (k, v, col) in &rows {
        ty += label_value(u, c, x, w, ty, k, v, *col);
    }
    if kette_h > 0 {
        if let Some(l) = lat {
            kette(u, c, lang, x + 16, ty + 6, w - 32, l, soll_fps, client.monitor_hz, 1.0);
        }
    }

    u.spark(c, ui::Rect { x: x + 14, y: y + h - 30, w: w - 28, h: 20 }, hist, 130.0, ui::MAGENTA);

    if !connected || err.is_some() {
        let t = err.unwrap_or_else(|| lang.get(ConnectionLost).to_string());
        u.text.draw(c, x, y + h + 20, &t, 13, ui::AMBER, 1);
    }
}

/// Systemsprache, etwa "de-DE". Ohne Fremdbibliothek: Windows liefert sie ueber
/// die Umgebung, sonst nehmen wir Englisch.
fn system_language() -> String {
    for var in ["LANG", "LC_ALL", "LANGUAGE"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() { return v; }
        }
    }
    // Frueher wurde hier PowerShell gestartet. Das geschieht VOR dem Fenster
    // und ohne Frist: ist PowerShell langsam oder per Richtlinie gesperrt,
    // erscheint das Fenster spaet oder nie. Das System weiss es auch direkt.
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut puffer = [0u16; 85];
        let n = unsafe { GetUserDefaultLocaleName(&mut puffer) };
        if n > 1 {
            return String::from_utf16_lossy(&puffer[..(n as usize - 1)]);
        }
    }
    "en".into()
}

/// Zeichnet den Startbildschirm in eine Datei, damit man ihn ohne Bildschirm
/// begutachten kann. Format BMP, weil das ohne Fremdbibliothek geht.
fn screenshot(path: &str, w: usize, h: usize, lang: &'static strings::Lang, view: &str) {
    let mut buf = vec![0u32; w * h];
    let mut u = ui::Ui::new();
    u.tick = 40;
    u.mouse = (-1, -1);

    // Ansicht "sitzung": so sieht es waehrend der Uebertragung aus - ein
    // angedeutetes Bild, darauf die Zahlen und die Einstellungstafel. Ohne
    // Bildschirm ist das die einzige Moeglichkeit, die Oberflaeche zu pruefen.
    if view == "sitzung" || view == "nerd" {
        for y in 0..h {
            for x in 0..w {
                let a = (x * 255 / w) as u32;
                let b = (y * 160 / h) as u32;
                buf[y * w + x] = ((a / 3) << 16) | ((b / 2) << 8) | (40 + a / 5);
            }
        }
        let hist: Vec<f32> = (0..120).map(|i| 90.0 + 25.0 * ((i as f32) / 9.0).sin()).collect();
        let info = Some(StreamInfo { width: 1920, height: 1080, fps: 120, codec: 1, chroma444: true, ten_bit: true });
        let sas = Some("628 306".to_string());
        let fp = Some("9EB4-EC3D-6856-8AF6".to_string());
        {
            let mut c = ui::Canvas::neu(&mut buf, w, h);
            let probe = Latenz {
                versatz_us: 1, umlauf_ms: 0.4, encoder_ms: 4.2, leitung_ms: 2.6,
                decoder_ms: 2.1, gesamt_ms: 17.3, bilder: 900, anzeige_ms: 1.4,
            };
            let nerd = view == "nerd";
            let hl = HostLast {
                cpu: 30.0, cpu_eigen: 28.0, gpu: Some(0.0), druck: 1,
                ram_benutzt_mb: 9114, ram_gesamt_mb: 16384, eigen_mb: 310,
                encoder_ms: 11.4, host_fps: 118.0,
            };
            let client = ClientStand { anzeige: "Software".into(), cpu_eigen: 5.8, monitor_hz: Some(60.0), ausgelassen: 0 };
            overlay(&mut u, &mut c, lang, 98.0, &hist, (2.1, info, 3, None, true), (sas.clone(), fp.clone()),
                    Some(probe), einstellungen::StatWahl::default(), nerd, Some(120),
                    if nerd { Some(hl) } else { None }, (Some(DecoderPfad::Nvdec(None)), None), &client);
            let _ = &fp;
            let bw = 520.min(w as i32 - 40);
            let bx = (w as i32 - bw) / 2;
            let t = lang.get(strings::Key::FirstContact);
            let zeilen = umbruch(&mut u, t, bw - 48, 13);
            let bh = 62 + zeilen.len() as i32 * 18;
            let by = h as i32 * 3 / 4 - bh / 2;
            c.panel(bx, by, bw, bh, ui::AMBER);
            let mut ty = by + 28;
            for z in &zeilen {
                u.text.draw_centered(&mut c, bx + bw / 2, ty, z, 13, ui::TEXT, 1);
                ty += 18;
            }
            u.text.draw_centered(&mut c, bx + bw / 2, ty + 24, sas.as_ref().unwrap(), 30, ui::AMBER, 6);
        }
        write_bmp(path, w, h, &buf, lang);
        return;
    }

    // Ansicht "hud" und "hud2": der Nerd-Modus ueber einem angedeuteten Bild.
    if view.starts_with("hud") {
        for y in 0..h {
            for x in 0..w {
                let a = (x * 255 / w) as u32;
                let b = (y * 160 / h) as u32;
                buf[y * w + x] = ((a / 3) << 16) | ((b / 2) << 8) | (40 + a / 5);
            }
        }
        let l = Latenz {
            versatz_us: 1, umlauf_ms: 0.9, encoder_ms: 10.8, leitung_ms: 4.3,
            decoder_ms: 8.6, gesamt_ms: 27.4, bilder: 900, anzeige_ms: 1.4,
        };
        let lh: Vec<f32> = (0..200).map(|i| 24.0 + 5.0 * ((i as f32) / 11.0).sin()).collect();
        let fh: Vec<f32> = (0..200).map(|i| 104.0 + 9.0 * ((i as f32) / 7.0).cos()).collect();
        let info = Some(StreamInfo { width: 1920, height: 1080, fps: 120, codec: 1, chroma444: true, ten_bit: true });
        let mut c = ui::Canvas::neu(&mut buf, w, h);
        // Nachgestellte Koennensliste, wie sie der Mac mini schickt: AV1
        // fehlt ihm, 4:4:4 8 Bit und 4:2:0 10 Bit brauchen die Umrechnung.
        let eintrag = |idx: u8, name: &str, available: bool, conversion: bool, chroma444: bool, ten_bit: bool| CodecEintrag {
            idx, available, hardware: available, conversion, chroma444, ten_bit, name: name.to_string(),
        };
        let codecs = vec![
            eintrag(0, "HEVC 4:4:4 10 Bit", true, false, true, true),
            eintrag(1, "HEVC 4:4:4 8 Bit", true, true, true, false),
            eintrag(2, "HEVC 4:2:0 10 Bit", true, true, false, true),
            eintrag(3, "HEVC 4:2:0 8 Bit", true, false, false, false),
            eintrag(4, "H.264 High", true, false, false, false),
            eintrag(5, "AV1", false, false, false, false),
        ];
        // "hud5": der Benchmark mitten im Lauf - 30 von 40 Schritten, mehr
        // als in die Tabelle passen, damit Ausschnitt und Balken im Bild
        // sind (gerollt in die Mitte). Die Zahlen erfunden, aber so, wie
        // sie auf dem Mac mini aussehen: mit der Datenrate waechst die
        // Encoderzeit, bei 150 Mbit/s werden Bilder verworfen (ueber 1 %:
        // nicht bestanden), und H.264 ueber NVDEC hat eine Kette weit ueber
        // 30 ms - nicht bestanden, obwohl alle Bilder ankommen.
        let bench = if view == "hud5" {
            let mut ergebnisse = Vec::new();
            for (name, q, dec) in [("HEVC 4:4:4 10 Bit", 4u8, 1.9f32), ("HEVC 4:4:4 8 Bit", 3, 1.9), ("H.264 High", 0, 38.0)] {
                for fps in [60u16, 120] {
                    for &mbit in &BENCH_MBITS {
                        let enc = 4.5 + mbit as f32 / 40.0;
                        let mut e = Ergebnis {
                            idx: 4 - q, codec: name.into(), qualitaet: q, mbit, fps, gescheitert: false,
                            fps_gemessen: fps as f32 - 0.4 - mbit as f32 / 100.0,
                            mbit_gemessen: mbit as f32 * 0.93,
                            kette_ms: enc + 2.6 + dec + 1.1 + if fps == 120 { 0.8 } else { 5.8 },
                            encoder_ms: enc, leitung_ms: 2.6, decoder_ms: dec, anzeige_ms: 1.1,
                            empfangen: fps as u64 * 5, verworfen: if mbit >= 150 { 7 } else { 0 }, ausgelassen: 0,
                            host_encoder_ms: enc + 0.6, budget_ms: 1000.0 / fps as f32,
                            host_cpu: 24.0 + mbit as f32 / 10.0, client_cpu: 5.0 + mbit as f32 / 50.0,
                            bestanden: false,
                        };
                        e.pruefen(true, true);
                        ergebnisse.push(e);
                    }
                }
            }
            let empfehlung = empfehlen(&ergebnisse).map(|i| ergebnisse[i].clone());
            Some(BenchStand {
                laeuft: true, abgebrochen: false, pos: 30, gesamt: 40,
                schritt: "HEVC 4:2:0 10 Bit · 10 Mbit/s · 60".into(),
                phase: BenchPhase::Messen, ergebnisse, empfehlung, mit_anzeige: true,
            })
        } else {
            None
        };
        // Vorgetaeuschte Erkennung, wie sie der Alienware-Laptop ergibt: eine
        // NVIDIA-Karte ohne Ausgang und die integrierte von Intel mit
        // Ausgang - so sind alle Knoepfe im Bild, und der Tooltip zeigt den
        // Kartennamen. Die Anzeige laeuft auf der NVIDIA, gespeichert ist
        // Integriert: die Zeile "gilt ab dem naechsten Start" ist damit
        // ebenfalls zu sehen.
        let karten = vec![
            Karte { index: 0, name: "NVIDIA GeForce RTX 3080 Ti Laptop GPU".into(), vendor: 0x10de, speicher_mb: 16384, hat_ausgang: false, luid: 1, rolle: Rolle::Grafikkarte(1) },
            Karte { index: 1, name: "Intel(R) Iris(R) Xe Graphics".into(), vendor: 0x8086, speicher_mb: 128, hat_ausgang: true, luid: 2, rolle: Rolle::Integriert },
        ];
        let stand = HudStand {
            vollbild: true, pixelgenau: false, statistik: true, nerd: true,
            wahl: einstellungen::StatWahl::default(),
            codecs,
            codec_idx: None,
            wechsel: false,
            decoder: einstellungen::DecoderWunsch::Automatik,
            decoder_aktiv: Some(DecoderPfad::Nvdec(Some(1))),
            karten,
            anzeige_aktiv: einstellungen::AnzeigeWunsch::Gpu,
            anzeige_gespeichert: einstellungen::AnzeigeWunsch::Integriert,
            anzeige_name: "D3D11 · NVIDIA GeForce RTX 3080 Ti Laptop GPU".into(),
            bench_konfig: BenchKonfig::vorgabe(5, true),
            bench,
            // 30 Zeilen, rund 16 passen: 7 ist die Mitte des Rollwegs.
            bench_scroll: if view == "hud5" { 7 } else { 0 },
        };
        let reiter = match view { "hud2" | "hud2tip" => 1u8, "hud3" => 2, "hud4" => 3, "hud5" => 4, _ => 0 };
        // "hud2tip": die Maus steht ueber dem Knopf "Grafikkarte" der
        // Decoderzeile, damit der Tooltip samt Kartennamen im Bild ist und
        // sich ueber SSH pruefen laesst.
        if view == "hud2tip" {
            u.mouse = (280, 498);
        }
        let _ = hud(
            &mut u, &mut c, lang, w as i32, h as i32, reiter,
            Some(l), &lh, 104.0, &fh, info, Some((50, 120, false, true, true)),
            (Some("841 177".into()), Some("9EB4-EC3D-6856-8AF6".into())),
            "192.168.178.194:9001", true, &stand,
        );
        write_bmp(path, w, h, &buf, lang);
        return;
    }
    let hosts = vec![
        discovery::Host {
            name: "Mac-mini-von-Robert.local".into(),
            addr: "192.168.178.194:9001".parse().unwrap(),
            seen: Instant::now(),
        },
        discovery::Host {
            name: "studio.local".into(),
            addr: "192.168.178.60:9001".parse().unwrap(),
            seen: Instant::now(),
        },
    ];
    // "abgeloest" und "fingerabdruck": der Startbildschirm mit der Meldung,
    // wie sie nach Nachricht 10 bzw. bei geaendertem Host-Schluessel dasteht -
    // ueber dieselben Schluessel wie im Betrieb, in der Sprache der Ansicht.
    let meldung = match view {
        "abgeloest" => Some(lang.get(strings::Key::SessionTakenOver).to_string()),
        "fingerabdruck" => Some(
            Meldung::from(secure::Fehler::FingerabdruckGeaendert {
                host: "192.168.178.194".into(),
                fingerabdruck: "9EB4-EC3D-6856-8AF6".into(),
                pfad: std::path::PathBuf::from("C:\\Users\\Robert\\AppData\\Roaming\\QuadChroma\\known_hosts.txt"),
            })
            .text(lang),
        ),
        _ => None,
    };
    {
        let mut c = ui::Canvas::neu(&mut buf, w, h);
        let _ = start_screen(&mut u, &mut c, lang, &hosts, "192.168.178.194:9001", meldung.as_deref());
    }

    write_bmp(path, w, h, &buf, lang);
}

fn write_bmp(path: &str, w: usize, h: usize, buf: &[u32], lang: &'static strings::Lang) {
    let row = ((w * 3 + 3) / 4) * 4;
    let size = 54 + row * h;
    let mut f = Vec::with_capacity(size);
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&(size as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&(w as i32).to_le_bytes());
    f.extend_from_slice(&(h as i32).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&24u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        let mut line = Vec::with_capacity(row);
        for x in 0..w {
            let p = buf[y * w + x];
            line.push((p & 255) as u8);
            line.push(((p >> 8) & 255) as u8);
            line.push(((p >> 16) & 255) as u8);
        }
        line.resize(row, 0);
        f.extend_from_slice(&line);
    }
    std::fs::write(path, f).ok();
    println!("geschrieben: {path} ({w}x{h}, Sprache {})", lang.name);
}

fn main() {
    // An die Konsole des Aufrufers anhaengen, falls es eine gibt. Beim
    // Doppelklick gibt es keine, dann passiert hier einfach nichts.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    // Erstes Argument, das kein Schalter und kein Wert eines Schalters ist,
    // ist die Adresse. Ohne die zweite Haelfte wurde aus `--anzeige cpu` die
    // Adresse "cpu:9001" - und die echte Adresse dahinter ignoriert.
    const WERTIG: &[(&str, usize)] = &[
        ("--anzeige", 1), ("--adapter", 1), ("--decoder", 1), ("--codec", 1),
        ("--set", 1), ("--faeden", 1), ("--shot", 3), ("--anzeigetest", 1),
        ("--benchmark-auswahl", 1), ("--mitschnitt", 1),
        // Host-Rolle (host/mod.rs liest sie selbst; hier nur, damit ihre
        // Werte nie fuer eine Adresse gehalten werden)
        ("--output", 1), ("--fps", 1), ("--mbit", 1), ("--konserve", 1), ("--sekunden", 1), ("--encoderweg", 1),
    ];
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Windows als Host: --host [port], --list, --messen - ohne Fenster,
    // Abzweig VOR allem, was ein Fenster oder einen Client braucht.
    #[cfg(windows)]
    if args.iter().any(|a| a == "--host" || a == "--list" || a == "--messen") {
        let code = host::main_host(&args);
        std::process::exit(code);
    }
    // Auf dem Mac gibt es die Host-Rolle nicht (dort ist QuadChroma.app der
    // Host). Ohne diesen Zweig wuerde "--list" zur Adresse und ein Fenster
    // aufgehen, das auf eine Verbindung wartet.
    #[cfg(not(windows))]
    if args.iter().any(|a| a == "--host" || a == "--list" || a == "--messen") {
        eprintln!("Die Host-Rolle (--host, --list, --messen) gibt es nur auf Windows.");
        std::process::exit(2);
    }

    let mut addr = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some((_, n)) = WERTIG.iter().find(|(s, _)| *s == a.as_str()) {
            i += 1 + n;
            continue;
        }
        // --benchmark [dauer]: die Dauer ist wahlfrei - eine Zahl dahinter
        // gehoert zum Schalter, alles andere nicht.
        if a == "--benchmark" {
            i += 1;
            if args.get(i).map(|v| v.parse::<u32>().is_ok()).unwrap_or(false) {
                i += 1;
            }
            continue;
        }
        if a.starts_with("--") {
            i += 1;
            continue;
        }
        addr = adresse_vollstaendig(a);
        break;
    }

    let headless = std::env::args().any(|a| a == "--headless");

    // Gegentest der Verschluesselung: --noisetest 192.168.178.x:9100
    if let Some(i) = std::env::args().position(|a| a == "--noisetest") {
        let args: Vec<String> = std::env::args().collect();
        let a = args.get(i + 1).cloned().unwrap_or_default();
        match noise::selftest_against(&a) {
            Ok(()) => println!("Gegentest bestanden"),
            Err(e) => println!("Gegentest fehlgeschlagen: {e}"),
        }
        return;
    }

    // Decoderwahl ohne Verbindung pruefen: --decodertest baut fuer HEVC und
    // H.264 je einen Decoder mit jedem Wunsch und sagt, was herauskam. So
    // laesst sich der Rueckfall auf Software auch auf einer Maschine ohne
    // NVIDIA-Karte pruefen, ohne einen Host zu belaestigen.
    if std::env::args().any(|a| a == "--decodertest") {
        if let Err(e) = ffmpeg::init() {
            println!("FFmpeg-Start fehlgeschlagen: {e}");
            return;
        }
        protokoll::einschalten(true);
        use einstellungen::DecoderWunsch as W;
        // Erst die Erkennung - ihre Zeilen stehen im Protokoll.
        let anzahl = karten().len();
        for z in protokoll::abholen() {
            println!("{z}");
        }
        println!("Karten mit Rolle: {anzahl}");
        // Welche Karte hinter welcher CUDA-Ordnungszahl steht - die Zahl,
        // die cuvid als "gpu" bekommt (siehe nvdec_ziel).
        match cuda_geraete() {
            Ok(g) => {
                for (n, l) in g.iter().enumerate() {
                    let karte = l.and_then(|l| karten().iter().find(|k| k.luid == l));
                    println!("CUDA-Geraet {n}: {}", karte.map(|k| k.name.as_str()).unwrap_or("ohne Gegenstueck bei DXGI"));
                }
            }
            Err(e) => println!("CUDA-Geraete: {e}"),
        }
        // Das D3D11-Geraet von FFmpeg auf jedem Adapter probieren, auch auf
        // WARP - so sieht man auf einer Maschine ohne Karte, mit welchen
        // Worten av_hwdevice_ctx_create scheitert.
        #[cfg(windows)]
        if let Ok(liste) = anzeige::adapter_liste() {
            for (i, a) in liste.iter().enumerate() {
                let geraet = std::ffi::CString::new(i.to_string()).unwrap();
                protokoll::fehler_verwerfen();
                let mut hw: *mut ffmpeg::sys::AVBufferRef = std::ptr::null_mut();
                let r = unsafe {
                    ffmpeg::sys::av_hwdevice_ctx_create(&mut hw, ffmpeg::sys::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, geraet.as_ptr(), std::ptr::null_mut(), 0)
                };
                if r < 0 {
                    println!("D3D11VA-Geraet auf Adapter {i} ({}): {}", a.name, ffmpeg_grund("scheitert", r));
                } else {
                    println!("D3D11VA-Geraet auf Adapter {i} ({}): ok", a.name);
                    unsafe { ffmpeg::sys::av_buffer_unref(&mut hw) };
                }
                for z in protokoll::abholen() {
                    println!("    {z}");
                }
            }
        }
        for (h264, codec) in [(false, "HEVC"), (true, "H.264")] {
            for chroma444 in [Some(true), Some(false)] {
                if h264 && chroma444 == Some(true) {
                    continue;
                }
                for w in [W::Automatik, W::Software, W::Gpu, W::Gpu2, W::Integriert] {
                    let bedarf = DecoderBedarf { h264, chroma444, anzeige_adapter: None };
                    let was = format!("{codec}{} / Wunsch {}", if chroma444 == Some(true) { " 4:4:4" } else if !h264 { " 4:2:0" } else { "" }, w.schluessel());
                    match decoder_bauen(bedarf, w) {
                        Ok(bau) => println!("{was}: {}", bau.meldung()),
                        Err(e) => println!("{was}: Fehler: {e}"),
                    }
                    for z in protokoll::abholen() {
                        println!("    {z}");
                    }
                }
            }
        }
        return;
    }

    // Anzeige ueber die Karte ohne Fenster pruefen: --anzeigetest [verzeichnis]
    // rechnet jedes Decoderformat einmal auf der Karte und einmal auf der
    // CPU und vergleicht (Goldbildtest, Differenzbilder ins Verzeichnis).
    // Laeuft auch ohne Grafikkarte ueber WARP. Exit-Code 0 nur, wenn alles
    // innerhalb der Toleranz liegt - damit ein Skript es merkt.
    #[cfg(windows)]
    {
        if let Some(i) = std::env::args().position(|a| a == "--anzeigetest") {
            use std::io::Write;
            let args: Vec<String> = std::env::args().collect();
            let verzeichnis = args.get(i + 1).cloned().unwrap_or_else(|| ".".into());
            if let Err(e) = ffmpeg::init() {
                println!("FFmpeg-Start fehlgeschlagen: {e}");
                std::process::exit(2);
            }
            protokoll::einschalten(false);
            let bestanden = anzeige::anzeigetest(&verzeichnis);
            std::io::stdout().flush().ok();
            std::process::exit(if bestanden { 0 } else { 1 });
        }
    }

    // Bild der Oberflaeche schreiben und beenden: --shot datei.bmp [sprache]
    if let Some(i) = std::env::args().position(|a| a == "--shot") {
        let args: Vec<String> = std::env::args().collect();
        let path = args.get(i + 1).cloned().unwrap_or_else(|| "ui.bmp".into());
        let code = args.get(i + 2).cloned().unwrap_or_else(|| "de".into());
        let view = args.get(i + 3).cloned().unwrap_or_else(|| "start".into());
        let (w, h) = if view == "sitzung" || view == "nerd" || view.starts_with("hud") { (1280, 720) } else { (900, 700) };
        screenshot(&path, w, h, strings::pick(&code), &view);
        return;
    }

    // Ohne Adresse auf der Befehlszeile faengt das Programm beim Startbildschirm
    // an - dort sucht es Hosts im Netz und nimmt die Adresse entgegen. Der
    // Eingabekanal bekommt sie erst, wenn wirklich verbunden wird.
    let input_addr = if addr.is_empty() { String::new() } else { bump_port(&addr, 1) };

    let start_addr = addr.clone();
    let input = Arc::new(Mutex::new(InputLink::new(input_addr)));

    // Zwischenablage: Was hier kopiert wird, geht zum Host. Die Uebergabe
    // aus der Fensterschleife heraus ist bewusst nicht blockierend.
    #[cfg(any(windows, target_os = "macos"))]
    {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let link = input.clone();
        std::thread::spawn(move || {
            while let Ok(text) = rx.recv() {
                if let Ok(mut l) = link.lock() {
                    l.send(IN_CLIP, text.as_bytes());
                }
            }
        });
        clipboard::watch(move |text| { let _ = tx.send(text); });
    }

    // Gespeicherte Einstellungen. Sie bestimmen unter anderem, ob wir im
    // Vollbild starten - das ist die Voreinstellung. Schon hier geladen,
    // weil der Empfangsfaden den Decoderwunsch vom ersten Bild an kennen soll.
    let cfg = einstellungen::Einstellungen::laden();
    // Die Karten einmal erkennen, bevor irgendein Faden sie braucht: je
    // Adapter eine Zeile im Protokoll (Rolle, Name, Speicher, Ausgang).
    karten();
    // --decoder auto|software|gpu|gpu2|integriert erzwingt fuer diesen Lauf
    // einen Pfad, ohne die gespeicherte Wahl anzufassen (dazu die deutschen
    // und englischen Woerter und die alten Werte nvidia/nvdec/cuvid, siehe
    // `einstellungen::rolle_wort`).
    let decoder_wunsch = std::env::args()
        .position(|a| a == "--decoder")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| einstellungen::DecoderWunsch::aus(&v))
        .unwrap_or(cfg.decoder);
    // --anzeige auto|gpu|gpu2|integriert|cpu|warp ebenso fuer die Anzeige;
    // --adapter n nimmt genau den n-ten Adapter aus der Liste im Protokoll.
    let anzeige_wunsch = std::env::args()
        .position(|a| a == "--anzeige")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| einstellungen::AnzeigeWunsch::aus(&v))
        .unwrap_or(cfg.anzeige);
    let adapter_wunsch: Option<u32> = std::env::args()
        .position(|a| a == "--adapter")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok());

    // Ton ist an, bis jemand ihn abschaltet - Default waere "aus".
    let shared = Arc::new(Mutex::new(Shared { decoder_wunsch, ton: true, ..Shared::default() }));
    {
        let shared = shared.clone();
        let inp = input.clone();
        std::thread::spawn(move || stream_thread(shared, inp));
    }
    // Wurde eine Adresse mitgegeben, gleich verbinden.
    if !addr.is_empty() {
        shared.lock().unwrap().target = Some(addr.clone());
    }

    // Pruefmodus ohne Fenster: nur empfangen, decodieren, Zahlen ausgeben.
    if headless {
        use std::io::Write;
        // Einmaliger Stellwunsch aus der Befehlszeile, nur im Pruefmodus.
        let mut wish: Option<(u32, u16, bool, bool, bool)> = None;
        if let Some(i) = std::env::args().position(|a| a == "--set") {
            let args: Vec<String> = std::env::args().collect();
            if let Some(v) = args.get(i + 1) {
                let p: Vec<&str> = v.split(',').collect();
                if p.len() >= 3 {
                    wish = Some((
                        p[0].parse().unwrap_or(50),
                        p[1].parse().unwrap_or(60),
                        p[2] != "0",
                        p.get(3).map(|x| *x != "0").unwrap_or(false),
                        p.get(4).map(|x| *x != "0").unwrap_or(true),
                    ));
                }
            }
        }
        // Einmaliger Codecwunsch aus der Befehlszeile: --codec <idx> nennt den
        // Index des Kandidaten in der Koennensliste des Hosts (Nachricht 8).
        let mut codec_wunsch: Option<u8> = None;
        if let Some(i) = std::env::args().position(|a| a == "--codec") {
            let args: Vec<String> = std::env::args().collect();
            codec_wunsch = args.get(i + 1).and_then(|v| v.parse().ok());
        }
        // --benchmark [dauer]: derselbe Ablauf wie im Reiter, angetrieben
        // aus diesem Takt, Tabelle und Empfehlung auf die Konsole, danach
        // Schluss. Testbild an, ausser mit --ohne-testbild.
        let benchmark_dauer: Option<u32> = std::env::args().position(|a| a == "--benchmark").map(|i| {
            std::env::args().nth(i + 1).and_then(|v| v.parse().ok()).unwrap_or(5)
        });
        let ohne_testbild = std::env::args().any(|a| a == "--ohne-testbild");
        // --benchmark-auswahl codecs:mbit:fps grenzt den Lauf ein (siehe
        // BenchKonfig::einschraenken).
        let benchmark_auswahl: Option<String> = std::env::args()
            .position(|a| a == "--benchmark-auswahl")
            .and_then(|i| std::env::args().nth(i + 1));
        let mut bench: Option<Benchmark> = None;
        let mut cpu_zeiten = (prozesszeit_100ns().unwrap_or(0), Instant::now());
        let mut cpu_eigen = 0.0f32;
        println!("Decoderwunsch: {}", decoder_wunsch.schluessel());
        // Auch ohne Fenster zuhoeren, wer sich im Netz ausruft - jede neue
        // Adresse einmal als Zeile, damit sich die Bekanntgabe eines Hosts
        // ohne Startbildschirm pruefen laesst.
        let hosts = discovery::start(9003);
        let mut gefunden: std::collections::HashSet<String> = std::collections::HashSet::new();
        let start = Instant::now();
        let mut last = 0u64;
        loop {
            if let Ok(h) = hosts.lock() {
                for host in h.list() {
                    if gefunden.insert(host.addr.to_string()) {
                        println!("Host gefunden: {} ({})", host.name, host.addr);
                    }
                }
            }
            // Der Drei-Sekunden-Takt in Scheiben von 50 ms: dazwischen
            // arbeitet der Benchmark, der seine Fristen selbst misst.
            let takt_ende = Instant::now() + Duration::from_secs(3);
            while Instant::now() < takt_ende {
                std::thread::sleep(Duration::from_millis(50));
                let Some(dauer) = benchmark_dauer else { continue };
                match bench.as_mut() {
                    None => {
                        // Start, sobald Koennensliste, Strominfo (welcher
                        // Codec laeuft - sonst wuenschte der erste Schritt
                        // womoeglich den, der schon laeuft, und der Host
                        // antwortete nur "laeuft bereits"), Einstellungen,
                        // die erste Lastmeldung des Hosts (sie kommt je
                        // Sekunde; ohne sie fehlten dem ersten Schritt
                        // Encoderzeit und Host-CPU, und der Strom ist bis
                        // dahin auch erst angelaufen) und der Eingabekanal
                        // da sind - ohne den kaeme kein Wunsch an.
                        let (bereit, link) = {
                            let s = shared.lock().unwrap();
                            (
                                s.connected
                                    && !s.codecs.is_empty()
                                    && s.info.is_some()
                                    && s.settings.is_some()
                                    && s.hostlast.is_some(),
                                s.link.clone(),
                            )
                        };
                        if !bereit {
                            continue;
                        }
                        let steht = {
                            let mut l = input.lock().unwrap();
                            l.set_link(link);
                            l.ensure();
                            l.steht()
                        };
                        if !steht {
                            continue;
                        }
                        let mut konfig = BenchKonfig::vorgabe(dauer, !ohne_testbild);
                        if let Some(a) = &benchmark_auswahl {
                            let codecs = shared.lock().unwrap().codecs.clone();
                            konfig.einschraenken(a, &codecs);
                        }
                        match Benchmark::neu(&konfig, &shared, false, &addr) {
                            Some(mut b) => {
                                b.starten(&shared, &input);
                                println!("Benchmark gestartet: {} Schritte, {} s je Schritt", b.schritte.len(), konfig.dauer_s);
                                bench = Some(b);
                            }
                            None => {
                                println!("Benchmark: kein verfuegbarer Codec in der Koennensliste");
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(b) if b.laeuft() => {
                        b.takt(&shared, &input, cpu_eigen);
                        // Im Pruefmodus zeigt niemand Bilder an.
                        shared.lock().unwrap().frame = None;
                    }
                    Some(b) => {
                        for m in protokoll::abholen() {
                            println!("{m}");
                        }
                        println!();
                        print!("{}", b.text());
                        std::io::stdout().flush().ok();
                        std::process::exit(if b.abgebrochen { 1 } else { 0 });
                    }
                }
            }
            if let Some(cpu) = cpu_eigen_messen(&mut cpu_zeiten) {
                cpu_eigen = cpu;
            }
            let s = shared.lock().unwrap();
            let n = s.decoded;
            // Das Protokoll seit dem letzten Takt: jeder (Neu-)Bau des
            // Decoders eine Zeile (welcher Pfad, welcher FFmpeg-Decoder, und
            // warum nicht NVDEC, falls so), das erste Bild jedes Decoders,
            // und FFmpegs eigene Worte - im Pruefmodus bis zur Stufe
            // "ausfuehrlich", da sagt cuvid, was die Karte kann und welches
            // Format er gewaehlt hat. In der Reihenfolge des Entstehens.
            for m in protokoll::abholen() {
                println!("{m}");
            }
            let pfad = s.decoder_pfad.map(|p| p.name()).unwrap_or_else(|| "-".into());
            // Ohne Fenster gibt es kein Glied Anzeige - das steht auch so da.
            let lat = match s.clock {
                Some(l) if l.gesamt_ms > 0.0 => format!(
                    "Verzoegerung {:.1} ms (+/-{:.1}) = Encoder {:.1} + Leitung {:.1} + Decoder {:.1} | Anzeige -",
                    l.gesamt_ms, l.umlauf_ms / 2.0, l.encoder_ms, l.leitung_ms, l.decoder_ms
                ),
                Some(l) => format!("Zeitabgleich steht, Umlauf {:.1} ms", l.umlauf_ms),
                None => "Zeitabgleich laeuft noch".into(),
            };
            let hl = match s.hostlast {
                Some(h) => format!(
                    " | Host: CPU {:.0}% (eigen {:.0}%), GPU {}, RAM {:.1}/{:.0} GB, Encoder {:.1} ms, {:.0} Bilder/s",
                    h.cpu, h.cpu_eigen,
                    match h.gpu { Some(g) => format!("{g:.0}%"), None => "n/v".into() },
                    h.ram_benutzt_mb as f32 / 1024.0, h.ram_gesamt_mb as f32 / 1024.0,
                    h.encoder_ms, h.host_fps
                ),
                None => String::new(),
            };
            // Der laufende Codec steht in jeder Zeile, damit ein Wechsel im
            // Protokoll sichtbar wird - derselbe Name wie im Menue und Overlay.
            let codec = s.info.map(|i| i.codec_name()).unwrap_or_else(|| "?".into());
            let line = format!(
                "{:.0}s | decodiert {} ({:.1}/s) | Codec {} | Decoder {} | {}{} | Fehler {:?}",
                start.elapsed().as_secs_f32(), n, (n - last) as f32 / 3.0, codec, pfad, lat, hl,
                s.error.as_ref().map(|m| &m.protokoll)
            );
            println!("{line}");
            std::io::stdout().flush().ok();
            // Im Pruefmodus auch den Eingabekanal anstossen: Er darf nur
            // aufgehen, wenn der Bildkanal steht, und das wollen wir sehen.
            let link = s.link.clone();
            let cur = s.settings;
            drop(s);
            {
                let mut l = input.lock().unwrap();
                l.set_link(link);
                let t = start.elapsed().as_secs_f32();
                l.mouse_move(0.5 + 0.2 * t.sin(), 0.5 + 0.2 * t.cos());
                // --set mbit,fps,gaming stellt einmal um, damit sich die
                // Einstellungen auch ohne Fenster pruefen lassen. Erst, wenn
                // der Eingabekanal steht: er baut sich im Hintergrund auf,
                // und `send` wirft bis dahin stumm weg.
                if l.steht() {
                    if let Some(w) = wish.take() {
                        l.settings(w.0, w.1, w.2, w.3, w.4);
                        println!(
                            "Einstellung gewuenscht: {} Mbit/s, {} fps, Gaming {}, feste Bildrate {}, Ton {}",
                            w.0, w.1, w.2, w.3, w.4
                        );
                    }
                }
                // --codec <idx> wuenscht einmal einen Kandidaten. Ebenso erst,
                // wenn der Eingabekanal wirklich steht - sonst waere der
                // Wunsch verloren, bevor der Host ihn je sah.
                if l.steht() {
                    if let Some(idx) = codec_wunsch.take() {
                        l.codec(idx);
                        println!("Codecwunsch gesendet: {idx}");
                    }
                }
                println!("Eingabekanal: {} gesendet | Host meldet: {:?}", l.sent, cur);
            }
            let mut s = shared.lock().unwrap();
            protokoll::nur_datei(&line);
            s.frame = None; // im Pruefmodus nichts anzeigen, Speicher freigeben
            last = n;
        }
    }

    let sprache = match &cfg.sprache {
        Some(c) => strings::pick(c),
        None => strings::pick(&system_language()),
    };

    // Ohne Schrift zeichnet die Oberflaeche zwar, aber ohne ein einziges
    // Wort - das waere fuer den Benutzer nicht von einem Fehler zu
    // unterscheiden. Also lieber klar sagen, was fehlt.
    {
        let probe = ui::Ui::new();
        if !probe.text.ok() {
            #[cfg(windows)]
            unsafe {
                use windows::core::{HSTRING, PCWSTR};
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                let t = HSTRING::from("QuadChroma");
                let m = HSTRING::from(
                    "Es wurde keine verwendbare Schriftart gefunden. Ohne sie bliebe die Oberflaeche leer.",
                );
                MessageBoxW(None, PCWSTR(m.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | MB_ICONERROR);
            }
            // Auf dem Mac gibt es kein Meldungsfenster ohne AppKit-Anlauf;
            // die Konsole muss reichen.
            #[cfg(not(windows))]
            eprintln!("Es wurde keine verwendbare Schriftart gefunden. Ohne sie bliebe die Oberflaeche leer.");
            return;
        }
    }

    let el = EventLoop::new().expect("Ereignisschleife");
    let mut app = App {
        shared,
        input: input.clone(),
        ui: ui::Ui::new(),
        screen: if start_addr.is_empty() { Screen::Start } else { Screen::Session },
        hosts: discovery::start(9003),
        addr_input: start_addr,
        lang: sprache,
        fps_hist: Vec::new(),
        last_frame: None,
        quit: false,
        mods: 0,

        fps_count: 0,
        fps_shown: 0.0,
        fps_since: Instant::now(),
        window: None,
        anzeige: Anzeige::Keine,
        anzeige_wunsch,
        adapter_wunsch,
        karten: karten().to_vec(),
        anzeige_aktiv: einstellungen::AnzeigeWunsch::Cpu,
        sofort: false,
        bild_da: None,
        bereit_ausstehend: None,
        ui_puffer: Vec::new(),
        ui_kasten_alt: None,
        ui_masse: (0, 0),
        ui_an: false,
        letzte_oberflaeche: Instant::now(),
        praesentation_ausstehend: false,
        geraet_verloren: None,
        letzter_gpu_fehler: None,
        shown: 0,
        banner_until: None,
        angewandt_fuer: None,
        letzte_zeichnung: Instant::now(),
        esc_seit: None,
        esc_verbraucht: false,
        esc_mods: 0,
        esc_up_faellig: None,
        hud_offen: false,
        hud_reiter: 0,
        lat_hist: Vec::new(),
        anzeige_name: "Software".into(),
        cpu_eigen: 0.0,
        cpu_zeiten: (prozesszeit_100ns().unwrap_or(0), Instant::now()),
        monitor_hz: None,
        zeiger_seq_gezeigt: 0,
        zeiger_eigen: false,
        zeiger_vorrat: Vec::new(),
        zeiger_faktor: 1,
        maus_im_fenster: false,
        benchmark: None,
        bench_konfig: BenchKonfig::vorgabe(5, true),
        bench_scroll: 0,
        bench_folgt: true,
        bench_lief: false,
        fullscreen: cfg.vollbild,
        pixel_exact: cfg.pixelgenau,
        show_overlay: cfg.overlay,
        cfg,
    };
    el.run_app(&mut app).expect("Fenster");
}

// --------------------------------------------------------------------- Menue
//
// Das Pult, das sich oeffnet, wenn ESC zwei Sekunden gehalten wird. Drei
// Reiter: Bild, Anzeige, Verschluesselung. Es heisst bewusst nicht
// "Nerd-Modus" - der ist ein Schalter DARIN, naemlich die erweiterte
// Statistik. Und jede Funktion, die auf einer Taste liegt, hat hier auch
// einen Schalter: Tastenkuerzel merkt sich niemand.

pub enum HudAktion {
    Nichts,
    Reiter(u8),
    Trennen,
    /// Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton.
    Stellen(u32, u16, bool, bool, bool),
    Schalter(u8),
    /// Wunsch nach diesem Kandidaten der Koennensliste.
    Codec(u8),
    /// Anderer Decoderpfad gewuenscht.
    Decoder(einstellungen::DecoderWunsch),
    /// Andere Anzeige gewuenscht - wird gespeichert, gilt ab dem naechsten Start.
    Anzeige(einstellungen::AnzeigeWunsch),
    /// Benchmark: Kandidat an/aus, Datenrate an/aus, Bildrate an/aus,
    /// Dauer +/-1 s, Testbild an/aus, Start, Abbruch, Empfehlung
    /// uebernehmen (Kandidat, Mbit/s, Bilder/s).
    BenchCodec(u8),
    BenchMbit(u32),
    BenchFps(u16),
    BenchDauer(i32),
    BenchTestbild,
    BenchStart,
    BenchAbbruch,
    BenchUebernehmen(u8, u32, u16),
}

pub const SCH_VOLLBILD: u8 = 0;
pub const SCH_PIXELGENAU: u8 = 1;
pub const SCH_STATISTIK: u8 = 2;
pub const SCH_NERD: u8 = 3;
pub const SCH_STAT_FPS: u8 = 4;
pub const SCH_STAT_LATENZ: u8 = 5;
pub const SCH_STAT_TEILE: u8 = 6;
pub const SCH_STAT_AUFL: u8 = 7;
pub const SCH_STAT_CODEC: u8 = 8;
pub const SCH_STAT_VERW: u8 = 9;
pub const SCH_STAT_CODE: u8 = 10;

/// Naechste sinnvolle Stufe der Skala oberhalb von v.
fn skalenende(v: f32) -> f32 {
    for stufe in [10.0f32, 20.0, 30.0, 50.0, 80.0, 120.0, 200.0, 400.0] {
        if stufe >= v {
            return stufe;
        }
    }
    800.0
}

pub struct HudStand {
    pub vollbild: bool,
    pub pixelgenau: bool,
    pub statistik: bool,
    pub nerd: bool,
    pub wahl: einstellungen::StatWahl,
    /// Koennensliste des Hosts fuer die Codecwahl im Reiter "Bild".
    pub codecs: Vec<CodecEintrag>,
    /// Vom Host zuletzt gemeldeter Kandidat (Nachricht 7), falls bekannt.
    pub codec_idx: Option<u8>,
    /// Ein Codecwunsch ist unterwegs.
    pub wechsel: bool,
    /// Gewuenschter Decoderpfad und der, der wirklich laeuft.
    pub decoder: einstellungen::DecoderWunsch,
    pub decoder_aktiv: Option<DecoderPfad>,
    /// Die erkannten Karten - je Rolle ein Knopf in den Zeilen Anzeige und
    /// Decoder; Automatik und Prozessor gibt es immer.
    pub karten: Vec<Karte>,
    /// Anzeige: welcher Knopf wirklich gilt (der wird hervorgehoben), was
    /// in der Datei steht (weicht es ab: "gilt ab dem naechsten Start"),
    /// und der Name dessen, was zeichnet.
    pub anzeige_aktiv: einstellungen::AnzeigeWunsch,
    pub anzeige_gespeichert: einstellungen::AnzeigeWunsch,
    pub anzeige_name: String,
    /// Reiter "Benchmark": was der naechste Lauf probiert, und der
    /// laufende oder letzte Lauf.
    pub bench_konfig: BenchKonfig,
    pub bench: Option<BenchStand>,
    /// Erste sichtbare Zeile der Ergebnistabelle (der Stand lebt in der
    /// App; hier nur der Wert fuer diese Zeichnung).
    pub bench_scroll: usize,
}

/// Ein Rollen-Knopf im Menue - fuer Anzeige und Decoder derselbe Satz,
/// nur die Wuensche dahinter sind verschiedene Typen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RollenWahl {
    Automatik,
    Gpu,
    Gpu2,
    Integriert,
    Prozessor,
}

impl RollenWahl {
    fn als_anzeige(self) -> einstellungen::AnzeigeWunsch {
        use einstellungen::AnzeigeWunsch as W;
        match self {
            RollenWahl::Automatik => W::Automatik,
            RollenWahl::Gpu => W::Gpu,
            RollenWahl::Gpu2 => W::Gpu2,
            RollenWahl::Integriert => W::Integriert,
            RollenWahl::Prozessor => W::Cpu,
        }
    }

    fn als_decoder(self) -> einstellungen::DecoderWunsch {
        use einstellungen::DecoderWunsch as W;
        match self {
            RollenWahl::Automatik => W::Automatik,
            RollenWahl::Gpu => W::Gpu,
            RollenWahl::Gpu2 => W::Gpu2,
            RollenWahl::Integriert => W::Integriert,
            RollenWahl::Prozessor => W::Software,
        }
    }

    /// WARP hat keinen Knopf: None.
    fn von_anzeige(w: einstellungen::AnzeigeWunsch) -> Option<Self> {
        use einstellungen::AnzeigeWunsch as W;
        Some(match w {
            W::Automatik => RollenWahl::Automatik,
            W::Gpu => RollenWahl::Gpu,
            W::Gpu2 => RollenWahl::Gpu2,
            W::Integriert => RollenWahl::Integriert,
            W::Cpu => RollenWahl::Prozessor,
            W::Warp => return None,
        })
    }

    fn von_decoder(w: einstellungen::DecoderWunsch) -> Self {
        use einstellungen::DecoderWunsch as W;
        match w {
            W::Automatik => RollenWahl::Automatik,
            W::Gpu => RollenWahl::Gpu,
            W::Gpu2 => RollenWahl::Gpu2,
            W::Integriert => RollenWahl::Integriert,
            W::Software => RollenWahl::Prozessor,
        }
    }
}

/// Die Knoepfe einer Rollen-Zeile in ihrer Reihenfolge: Automatik, die
/// dedizierten Karten ("Grafikkarte", bei zweien "Grafikkarte 1" und
/// "Grafikkarte 2" - eine dritte bekommt keinen Knopf), Integriert, falls
/// erkannt, Prozessor. Zu jedem Knopf die Karte, falls es eine gibt.
fn rollen_knoepfe<'a>(karten: &'a [Karte], lang: &'static strings::Lang) -> Vec<(RollenWahl, String, Option<&'a Karte>)> {
    use strings::Key::*;
    let mut aus = vec![(RollenWahl::Automatik, lang.get(RoleAuto).to_string(), None)];
    let dedizierte = karten.iter().filter(|k| matches!(k.rolle, Rolle::Grafikkarte(_))).count();
    for (wahl, n) in [(RollenWahl::Gpu, 1u8), (RollenWahl::Gpu2, 2)] {
        if let Some(k) = karte_mit(karten, Rolle::Grafikkarte(n)) {
            let text = if dedizierte == 1 { lang.get(RoleGpu).to_string() } else { lang.get(RoleGpuN).replace("{n}", &n.to_string()) };
            aus.push((wahl, text, Some(k)));
        }
    }
    if let Some(k) = karte_mit(karten, Rolle::Integriert) {
        aus.push((RollenWahl::Integriert, lang.get(RoleIntegrated).to_string(), Some(k)));
    }
    aus.push((RollenWahl::Prozessor, lang.get(RoleCpu).to_string(), None));
    aus
}

/// Die erkannte Karte fuer den Tooltip: Name, Speicher (auf GB gerundet,
/// unter einem GB in MB - eine integrierte hat kaum eigenen), Ausgang.
fn karte_beschreibung(k: &Karte, lang: &'static strings::Lang) -> String {
    use strings::Key::*;
    let speicher = if k.speicher_mb >= 1024 {
        format!("{} GB", (k.speicher_mb + 512) / 1024)
    } else {
        format!("{} MB", k.speicher_mb)
    };
    format!("{} · {speicher} · {}", k.name, lang.get(if k.hat_ausgang { AdapterWithOutput } else { AdapterNoOutput }))
}

#[allow(clippy::too_many_arguments)]
fn hud(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    ww: i32,
    wh: i32,
    reiter: u8,
    lat: Option<Latenz>,
    lat_hist: &[f32],
    fps_jetzt: f32,
    fps_hist: &[f32],
    info: Option<StreamInfo>,
    stell: Option<(u32, u16, bool, bool, bool)>,
    secure: (Option<String>, Option<String>),
    adresse: &str,
    gespeichert: bool,
    stand: &HudStand,
) -> HudAktion {
    use strings::Key::*;
    let mut aktion = HudAktion::Nichts;
    // Tooltip: der Schalter, ueber dem die Maus gerade steht, merkt sich
    // seinen Text; gezeichnet wird er ganz am Ende, ueber allem anderen.
    let maus = u.mouse;
    let mut tip: Option<strings::Key> = None;
    // Ein zweiter, dynamischer Satz unter dem Text des Schluessels - die
    // erkannte Karte auf den Rollen-Knoepfen. Nur dort gesetzt.
    let mut tip_zusatz: Option<String> = None;
    // Die Ergebnistabelle meldet sich unten, wenn sie gezeichnet wird -
    // auf jedem anderen Reiter gibt es nichts zu rollen.
    u.bench_tabelle = None;
    u.bench_sichtbar = 0;

    let s: f32 = if wh >= 1800 { 2.0 } else if wh >= 1000 { 1.5 } else { 1.0 };
    let p = |v: i32| -> i32 { (v as f32 * s).round() as i32 };
    let sz = |v: u32| -> u32 { (v as f32 * s).round() as u32 };

    c.fill(0, 0, ww, wh, ui::BG, 205);
    let breite = (ww - p(80)).min(p(1100));
    let hoehe = (wh - p(80)).min(p(570));
    let x0 = (ww - breite) / 2;
    let y0 = (wh - hoehe) / 2;
    c.fill(x0, y0, breite, hoehe, 0x080c14, 225);
    c.panel(x0, y0, breite, hoehe, ui::CYAN);
    let rand = p(20);
    let ix = x0 + rand;
    let iw = breite - 2 * rand;

    // --- Kopf mit Reitern -------------------------------------------------
    u.text.draw(c, ix, y0 + p(30), "QUADCHROMA", sz(18), ui::CYAN, p(8));
    u.text.draw_right(c, x0 + breite - rand, y0 + p(30), adresse, sz(12), ui::DIM, p(1));
    let namen = [lang.get(TabPicture), lang.get(TabDisplay), lang.get(Encryption), lang.get(TabShortcuts), lang.get(TabBenchmark)];
    let mut rx = ix + u.text.width("QUADCHROMA", sz(18), p(8)) + p(40);
    for (i, n) in namen.iter().enumerate() {
        let bw = u.text.width(n, sz(12), p(2)) + p(28);
        let r = ui::Rect { x: rx, y: y0 + p(12), w: bw, h: p(26) };
        let aktiv = reiter == i as u8;
        let heiss = r.hit(u.mouse.0, u.mouse.1);
        if aktiv {
            c.rect(r.x, r.y, r.w, r.h, ui::CYAN, 26);
        }
        u.text.draw_centered(c, r.x + r.w / 2, r.y + r.h - p(8), n, sz(12),
                             if aktiv { ui::TEXT } else { ui::DIM }, p(2));
        if aktiv || heiss {
            c.hline(r.x, r.y + r.h, r.w, ui::CYAN, if aktiv { 255 } else { 110 });
        }
        if heiss && u.click {
            aktion = HudAktion::Reiter(i as u8);
        }
        rx += bw + p(8);
    }
    c.glow_hline(x0, y0 + p(44), breite, ui::MAGENTA);

    // --- Befund: die zwei Zahlen, waehrend man dreht ----------------------
    // Der Reiter "Benchmark" braucht die ganze Hoehe fuer seine Tabelle
    // und verzichtet auf die beiden Kacheln.
    let soll = stell.map(|x| x.1 as f32).or_else(|| info.map(|i| i.fps as f32)).unwrap_or(60.0);
    let kw = (iw - p(16)) / 2;
    for (kx, welche) in [(ix, 0usize), (ix + kw + p(16), 1usize)] {
        if reiter == 4 {
            break;
        }
        let (label, zahl, zusatz, farbe, hist, lo, hi) = if welche == 0 {
            let l = lat.unwrap_or_default();
            let (lo, hi) = if lat_hist.is_empty() {
                (0.0, 1.0)
            } else {
                let mn = lat_hist.iter().cloned().fold(f32::MAX, f32::min);
                let mx = lat_hist.iter().cloned().fold(0.0f32, f32::max);
                (mn * 0.95, (mx * 1.05).max(mn * 0.95 + 1.0))
            };
            // Alle fuenf Glieder, wie die Latenz-Zeile der Statistik und ihr Verlauf.
            (lang.get(Latency), format!("{:.1}", l.bis_anzeige()), format!("±{:.1} ms", l.umlauf_ms / 2.0),
             if l.gesamt_ms > 0.0 && l.bis_anzeige() < 40.0 { ui::CYAN } else { ui::AMBER }, lat_hist, lo, hi)
        } else {
            let mn = fps_hist.iter().cloned().fold(f32::MAX, f32::min).min(soll * 0.8);
            let mx = fps_hist.iter().cloned().fold(0.0f32, f32::max).max(soll * 1.05);
            (lang.get(Fps), format!("{fps_jetzt:.0}"), format!("/ {soll:.0}"),
             if fps_jetzt > 50.0 { ui::CYAN } else { ui::AMBER }, fps_hist,
             if mn.is_finite() { mn } else { 0.0 }, mx)
        };
        u.text.draw(c, kx + p(16), y0 + p(70), label, sz(11), ui::DIM, p(3));
        u.text.draw(c, kx + p(16), y0 + p(116), &zahl, sz(38), farbe, p(2));
        let zw = u.text.width(&zahl, sz(38), p(2));
        u.text.draw(c, kx + p(16) + zw + p(12), y0 + p(116), &zusatz, sz(12), ui::DIM, p(1));
        u.spark_range(c, ui::Rect { x: kx + p(16), y: y0 + p(124), w: kw - p(32), h: p(24) }, hist, lo, hi, farbe);
    }
    if reiter != 4 {
        c.hline(ix, y0 + p(166), iw, ui::DIM, 60);
    }

    let cy = y0 + p(186);
    // Fusszeile: schon hier bestimmt, damit die Codecknoepfe wissen, wo
    // Schluss ist, und nicht in die Trennlinie hineinwachsen.
    let fy = y0 + hoehe - p(22);
    match reiter {
        0 => {
            let (mbit, fps_soll, gaming, fest, ton) = stell.unwrap_or((0, 0, false, false, true));
            let stellen = |u: &mut ui::Ui, c: &mut ui::Canvas, sx: i32, sy: i32, label: &str, wert: String| -> i32 {
                u.text.draw(c, sx, sy + p(14), label, sz(11), ui::DIM, p(1));
                u.text.draw(c, sx, sy + p(40), &wert, sz(18), ui::TEXT, p(1));
                let rm = ui::Rect { x: sx + p(180), y: sy + p(16), w: p(34), h: p(30) };
                let rp = ui::Rect { x: sx + p(220), y: sy + p(16), w: p(34), h: p(30) };
                let minus = u.button(c, rm, "", ui::CYAN);
                let plus = u.button(c, rp, "", ui::CYAN);
                for d in 0..2 {
                    c.hline(rm.x + p(11), rm.y + rm.h / 2 + d, p(12), ui::CYAN, 255);
                    c.hline(rp.x + p(11), rp.y + rp.h / 2 + d, p(12), ui::CYAN, 255);
                    c.vline(rp.x + rp.w / 2 + d, rp.y + rp.h / 2 - p(6), p(12), ui::CYAN, 255);
                }
                if minus { -1 } else if plus { 1 } else { 0 }
            };
            if (ui::Rect { x: ix, y: cy, w: p(254), h: p(46) }).hit(maus.0, maus.1) {
                tip = Some(TipBitrate);
            }
            if (ui::Rect { x: ix + iw / 2, y: cy, w: p(254), h: p(46) }).hit(maus.0, maus.1) {
                tip = Some(TipFps);
            }
            let d = stellen(u, c, ix, cy, lang.get(MaxBitrate), format!("{mbit} Mbit/s"));
            if d != 0 {
                let schritt = if mbit >= 100 { 25 } else if mbit >= 30 { 10 } else { 5 };
                aktion = HudAktion::Stellen((mbit as i32 + d * schritt).clamp(2, 500) as u32, fps_soll, gaming, fest, ton);
            }
            let d = stellen(u, c, ix + iw / 2, cy, lang.get(MaxFps), format!("{fps_soll}"));
            if d != 0 {
                aktion = HudAktion::Stellen(mbit, (fps_soll as i32 + d * 10).clamp(10, 240) as u16, gaming, fest, ton);
            }
            let r_gaming = ui::Rect { x: ix, y: cy + p(70), w: iw / 2 - p(30), h: p(28) };
            let r_fest = ui::Rect { x: ix + iw / 2, y: cy + p(70), w: iw / 2 - p(30), h: p(28) };
            let r_ton = ui::Rect { x: ix, y: cy + p(104), w: iw / 2 - p(30), h: p(28) };
            if r_gaming.hit(maus.0, maus.1) { tip = Some(TipGaming); }
            if r_fest.hit(maus.0, maus.1) { tip = Some(FixedRateHint); }
            if r_ton.hit(maus.0, maus.1) { tip = Some(TipSound); }
            if u.toggle(c, r_gaming, lang.get(GamingMode), gaming) {
                aktion = HudAktion::Stellen(mbit, fps_soll, !gaming, fest, ton);
            }
            if u.toggle(c, r_fest, lang.get(FixedRate), fest) {
                aktion = HudAktion::Stellen(mbit, fps_soll, gaming, !fest, ton);
            }
            // Ton: unter dem Spielmodus, neben dem Hinweis zur festen Bildrate.
            // Aus heisst aus - beim Host (kein Paket mehr) und hier (nichts
            // mehr abgespielt, falls der Host den Schalter nicht kennt).
            if u.toggle(c, r_ton, lang.get(Sound), ton) {
                aktion = HudAktion::Stellen(mbit, fps_soll, gaming, fest, !ton);
            }
            for (i, z) in umbruch(u, lang.get(FixedRateHint), iw / 2 - p(40), sz(10)).iter().enumerate() {
                u.text.draw(c, ix + iw / 2, cy + p(104) + i as i32 * p(15), z, sz(10), ui::DIM, p(1));
            }
            if stand.wechsel {
                // Waehrend der Host umbaut, steht hier der Grund fuer den
                // kurzen Stillstand statt der alten Eckdaten.
                u.text.draw(c, ix, cy + p(146), lang.get(CodecSwitching), sz(11), ui::AMBER, p(1));
            } else if let Some(i) = info {
                u.text.draw(c, ix, cy + p(146), &format!("{}x{}  ·  {}", i.width, i.height, i.codec_name()),
                            sz(11), ui::DIM, p(1));
            }
            if gespeichert {
                u.text.draw_right(c, x0 + breite - rand, cy + p(146), lang.get(SavedForHost), sz(11), ui::DIM, p(1));
            }

            // --- Codecwahl: ein Knopf je Eintrag der Koennensliste ----------
            // Der laufende Eintrag in Cyan, die anderen gedaempft, was der
            // Mac nicht kann, noch dunkler und ohne Klick. Die Knoepfe
            // fliessen zeilenweise, damit auch ein schmales Fenster alle zeigt.
            if !stand.codecs.is_empty() {
                let oy = cy + p(176);
                u.text.draw(c, ix, oy, lang.get(Codec), sz(11), ui::DIM, p(3));
                let suffix = lang.get(CodecConverted);
                // Farbe fuer "nicht verfuegbar": noch stiller als DIM.
                const STUMM: u32 = 0x2a3542;
                let mut kx = ix;
                let mut ky = oy + p(10);
                let kh = p(30);
                let pitch = p(50);
                let luecke = p(10);
                // Solange weder Nachricht 7 noch eine Strominfo da war, weiss
                // niemand, welcher Eintrag laeuft - dann darf ein Klick auch
                // keinen Wunsch losschicken, sonst wechselt man "auf sich
                // selbst" und der Hinweis steht bis zum Ablauf der Frist.
                let bekannt = stand.codec_idx.is_some() || info.is_some();
                for e in &stand.codecs {
                    let aktuell = match (stand.codec_idx, info) {
                        (Some(idx), _) => idx == e.idx,
                        (None, Some(i)) => e.passt_zu(&i),
                        (None, None) => false,
                    };
                    let bw = u.text.width(&e.name, 15, 2) + p(28);
                    if kx + bw > ix + iw && kx > ix {
                        kx = ix;
                        ky += pitch;
                    }
                    // Was mit der Fusszeile kollidieren wuerde, wird schlicht
                    // nicht gezeichnet - lieber eine Zeile weniger als Salat.
                    if ky + kh + p(14) > fy - p(24) {
                        break;
                    }
                    let r = ui::Rect { x: kx, y: ky, w: bw, h: kh };
                    if r.hit(maus.0, maus.1) { tip = Some(TipCodec); }
                    if !e.available {
                        // Nur zeichnen, nicht bedienen: keine Hervorhebung
                        // beim Ueberfahren, kein Klick.
                        c.rect(r.x, r.y, r.w, r.h, STUMM, 10);
                        let cut = 10;
                        for (cx_, cy_, dx, dy) in [
                            (r.x, r.y, 1, 1), (r.x + r.w - 1, r.y, -1, 1),
                            (r.x, r.y + r.h - 1, 1, -1), (r.x + r.w - 1, r.y + r.h - 1, -1, -1),
                        ] {
                            for i in 0..cut {
                                c.px(cx_ + dx * i, cy_, STUMM, 255);
                                c.px(cx_, cy_ + dy * i, STUMM, 255);
                            }
                        }
                        u.text.draw_centered(c, r.x + r.w / 2, r.y + r.h / 2 + 5, &e.name, 15, STUMM, 2);
                    } else {
                        let farbe = if aktuell { ui::CYAN } else { ui::DIM };
                        if u.button(c, r, &e.name, farbe) && !aktuell && bekannt {
                            aktion = HudAktion::Codec(e.idx);
                        }
                    }
                    if e.conversion {
                        let sf = if e.available { ui::DIM } else { STUMM };
                        u.text.draw(c, r.x + p(4), r.y + r.h + p(12), suffix, sz(9), sf, p(1));
                    }
                    kx += bw + luecke;
                }
            }
        }
        1 => {
            // Alles, was sonst nur auf einer Taste liegt.
            let sp = iw / 2;
            let zeile = |u: &mut ui::Ui, c: &mut ui::Canvas, sx: i32, sy: i32, t: &str, an: bool, id: u8, akt: &mut HudAktion,
                         tipk: strings::Key, tip: &mut Option<strings::Key>| {
                let r = ui::Rect { x: sx, y: sy, w: sp - p(40), h: p(26) };
                if r.hit(maus.0, maus.1) { *tip = Some(tipk); }
                if u.toggle(c, r, t, an) {
                    *akt = HudAktion::Schalter(id);
                }
            };
            zeile(u, c, ix, cy, lang.get(Fullscreen), stand.vollbild, SCH_VOLLBILD, &mut aktion, TipFullscreen, &mut tip);
            zeile(u, c, ix, cy + p(34), lang.get(PixelExact), stand.pixelgenau, SCH_PIXELGENAU, &mut aktion, TipPixelExact, &mut tip);
            zeile(u, c, ix, cy + p(68), lang.get(ShowOverlay), stand.statistik, SCH_STATISTIK, &mut aktion, TipShowOverlay, &mut tip);
            zeile(u, c, ix, cy + p(102), lang.get(NerdMode), stand.nerd, SCH_NERD, &mut aktion, TipNerdMode, &mut tip);
            // --- Anzeige und Decoder: je eine Zeile Rollen-Knoepfe --------
            // Automatik und Prozessor gibt es immer, Grafikkarte(n) und
            // Integriert nur, wenn die Erkennung eine solche Karte fand. Der
            // Name der Karte steht im Tooltip, nie auf dem Knopf - AMD-Namen
            // sind ewig lang. Hervorgehoben ist, was wirklich gilt: bei der
            // Anzeige die Karte, die zeichnet (oder Automatik, wenn so
            // gewuenscht), beim Decoder der geltende Wunsch - dahinter steht,
            // was wirklich laeuft, und bei einem Rueckfall sieht man ihn
            // hier sofort. Passt eine Zeile nicht in die Spalte, fliesst sie
            // um (zwei dedizierte Karten).
            {
                let knoepfe = rollen_knoepfe(&stand.karten, lang);
                // Eine Zeile Knoepfe: der Knopf `aktiv` in Cyan. Liefert den
                // geklickten Knopf und die Unterkante der Zeile.
                let zeile_rollen = |u: &mut ui::Ui, c: &mut ui::Canvas, ky: i32, aktiv: Option<RollenWahl>,
                                    tips: &[(strings::Key, Option<String>)],
                                    tip: &mut Option<strings::Key>, tip_zusatz: &mut Option<String>| -> (Option<RollenWahl>, i32) {
                    let mut kx = ix;
                    let mut ky = ky;
                    let mut klick = None;
                    for (i, (wahl, text, _)) in knoepfe.iter().enumerate() {
                        // Enger gepolstert als die Codec-Knoepfe: vier bis
                        // fuenf muessen in die linke Spalte passen.
                        let bw = u.text.width(text, 15, 2) + p(14);
                        // Die rechte Spalte faengt bei ix + sp an.
                        if kx + bw > ix + sp - p(20) && kx > ix {
                            kx = ix;
                            ky += p(36);
                        }
                        let r = ui::Rect { x: kx, y: ky, w: bw, h: p(30) };
                        if r.hit(maus.0, maus.1) {
                            *tip = Some(tips[i].0);
                            *tip_zusatz = tips[i].1.clone();
                        }
                        let ist_aktiv = aktiv == Some(*wahl);
                        let farbe = if ist_aktiv { ui::CYAN } else { ui::DIM };
                        if u.button(c, r, text, farbe) && !ist_aktiv {
                            klick = Some(*wahl);
                        }
                        kx += bw + p(10);
                    }
                    (klick, ky + p(30))
                };
                let beschreibung = |k: Option<&Karte>| k.map(|k| karte_beschreibung(k, lang));

                // Zeile 1: die Anzeige. Was gilt, steht im Label.
                let oy = cy + p(140);
                let label = kuerzen(u, &format!("{} · {}", lang.get(DisplayLabel), stand.anzeige_name), sp - p(40), sz(11), p(3));
                u.text.draw(c, ix, oy, &label, sz(11), ui::DIM, p(3));
                let tips_anzeige: Vec<(strings::Key, Option<String>)> = knoepfe
                    .iter()
                    .map(|(wahl, _, karte)| match wahl {
                        RollenWahl::Automatik => (TipDisplayAuto, beschreibung(karte_automatik(&stand.karten))),
                        RollenWahl::Gpu | RollenWahl::Gpu2 => (TipDisplayGpu, beschreibung(*karte)),
                        RollenWahl::Integriert => (TipDisplayIntegrated, beschreibung(*karte)),
                        RollenWahl::Prozessor => (TipDisplayCpu, None),
                    })
                    .collect();
                let (klick, unten) = zeile_rollen(u, c, oy + p(10), RollenWahl::von_anzeige(stand.anzeige_aktiv), &tips_anzeige, &mut tip, &mut tip_zusatz);
                if let Some(w) = klick {
                    aktion = HudAktion::Anzeige(w.als_anzeige());
                }
                // Steht in der Datei etwas anderes als das, was laeuft, gilt
                // es erst beim naechsten Start - das steht dann hier.
                if stand.anzeige_gespeichert != stand.anzeige_aktiv {
                    let name = RollenWahl::von_anzeige(stand.anzeige_gespeichert)
                        .and_then(|w| knoepfe.iter().find(|(k, _, _)| *k == w).map(|(_, t, _)| t.clone()))
                        .unwrap_or_else(|| stand.anzeige_gespeichert.schluessel().to_string());
                    u.text.draw(c, ix, unten + p(14), &format!("{name} · {}", lang.get(NextStartHint)), sz(10), ui::AMBER, p(1));
                }

                // Zeile 2: der Decoder.
                let oy = unten + p(32);
                let label = match stand.decoder_aktiv {
                    Some(pf) => format!("{} · {}", lang.get(DecoderLabel), pf.name()),
                    None => lang.get(DecoderLabel).to_string(),
                };
                let label = kuerzen(u, &label, sp - p(40), sz(11), p(3));
                u.text.draw(c, ix, oy, &label, sz(11), ui::DIM, p(3));
                let tips_decoder: Vec<(strings::Key, Option<String>)> = knoepfe
                    .iter()
                    .map(|(wahl, _, karte)| match wahl {
                        RollenWahl::Automatik => (TipDecoderAuto, None),
                        RollenWahl::Gpu | RollenWahl::Gpu2 => {
                            // Nur NVIDIA decodiert 4:4:4 (NVDEC); alle anderen
                            // gehen ueber D3D11VA, und das kann nur 4:2:0.
                            let z = match karte {
                                Some(k) if k.nvidia() => Some(karte_beschreibung(k, lang)),
                                Some(k) => Some(format!("{}\n{}", karte_beschreibung(k, lang), lang.get(TipOnly420))),
                                None => None,
                            };
                            (TipDecoderGpu, z)
                        }
                        RollenWahl::Integriert => (TipDecoderIntegrated, beschreibung(*karte)),
                        RollenWahl::Prozessor => (TipDecoderCpu, None),
                    })
                    .collect();
                let (klick, _) = zeile_rollen(u, c, oy + p(10), Some(RollenWahl::von_decoder(stand.decoder)), &tips_decoder, &mut tip, &mut tip_zusatz);
                if let Some(w) = klick {
                    aktion = HudAktion::Decoder(w.als_decoder());
                }
            }
            let w = stand.wahl;
            // Ueberschrift ueber die rechte Spalte, nicht unter die linke.
            u.text.draw(c, ix + sp, cy - p(16), lang.get(ShowOverlay), sz(10), ui::DIM, p(3));
            zeile(u, c, ix + sp, cy, lang.get(Fps), w.fps, SCH_STAT_FPS, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(30), lang.get(Latency), w.latenz, SCH_STAT_LATENZ, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(60), lang.get(DecodeTime), w.teile, SCH_STAT_TEILE, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(90), lang.get(Resolution), w.aufloesung, SCH_STAT_AUFL, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(120), lang.get(Codec), w.codec, SCH_STAT_CODEC, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(150), lang.get(Dropped), w.verworfen, SCH_STAT_VERW, &mut aktion, TipStatRows, &mut tip);
            zeile(u, c, ix + sp, cy + p(180), lang.get(SecuredWith), w.code, SCH_STAT_CODE, &mut aktion, TipStatRows, &mut tip);
        }
        3 => {
            // Die Tasten, die der Client selbst abfaengt - alles andere geht
            // an den Mac. Jede davon ist auch ein Schalter im Menue; hier
            // steht sie zum Nachschlagen.
            let strg = if lang.code == "de" { "Strg+Esc" } else { "Ctrl+Esc" };
            let zeilen: [(&str, &str); 6] = [
                ("F9", lang.get(ShowOverlay)),
                ("F10", lang.get(ShortcutMenu)),
                ("ESC 2 s", lang.get(ShortcutMenu)),
                ("F11", lang.get(Fullscreen)),
                ("F12", lang.get(PixelExact)),
                (strg, lang.get(ShortcutBack)),
            ];
            let mut zy = cy;
            for (taste, was) in zeilen {
                u.text.draw(c, ix, zy + p(14), taste, sz(14), ui::CYAN, p(2));
                u.text.draw(c, ix + p(150), zy + p(14), was, sz(13), ui::TEXT, p(1));
                c.hline(ix, zy + p(26), iw, ui::DIM, 40);
                zy += p(34);
            }
            u.text.draw(c, ix, zy + p(14), "Win", sz(14), ui::CYAN, p(2));
            u.text.draw(c, ix + p(150), zy + p(14), lang.get(ShortcutWinKey), sz(13), ui::TEXT, p(1));
            zy += p(40);
            for (i, z) in umbruch(u, lang.get(ShortcutOthers), iw, sz(11)).iter().enumerate() {
                u.text.draw(c, ix, zy + p(14) + i as i32 * p(16), z, sz(11), ui::DIM, p(1));
            }
        }
        4 => {
            // --- Benchmark: oben die Konfiguration, darunter Fortschritt,
            // Tabelle und Empfehlung. Waehrend eines Laufs ist die
            // Konfiguration nur zu sehen, nicht zu bedienen.
            let konfig = &stand.bench_konfig;
            let laeuft = stand.bench.as_ref().map(|b| b.laeuft).unwrap_or(false);
            // Kleiner An/Aus-Knopf: Cyan heisst "laeuft mit".
            let knopf = |u: &mut ui::Ui, c: &mut ui::Canvas, r: ui::Rect, t: &str, an: bool| -> bool {
                let heiss = !laeuft && r.hit(maus.0, maus.1);
                let farbe = if an { ui::CYAN } else { ui::DIM };
                c.rect(r.x, r.y, r.w, r.h, farbe, if heiss { 34 } else if an { 22 } else { 8 });
                c.hline(r.x, r.y + r.h - 1, r.w, farbe, if an { 255 } else { 90 });
                u.text.draw_centered(c, r.x + r.w / 2, r.y + r.h / 2 + p(4), t, sz(11), if an { ui::TEXT } else { ui::DIM }, p(1));
                heiss && u.click
            };
            let zh = p(24);
            let by = y0 + p(58);
            // Zeile 1: die Kandidaten des Hosts, nur die verfuegbaren.
            u.text.draw(c, ix, by + p(16), lang.get(Codec), sz(11), ui::DIM, p(3));
            let mut kx = ix + p(96);
            for e in stand.codecs.iter().filter(|e| e.available) {
                let bw = u.text.width(&e.name, sz(11), p(1)) + p(18);
                if kx + bw > ix + iw {
                    break;
                }
                let r = ui::Rect { x: kx, y: by, w: bw, h: zh };
                if knopf(u, c, r, &e.name, !konfig.codecs_aus.contains(&e.idx)) {
                    aktion = HudAktion::BenchCodec(e.idx);
                }
                kx += bw + p(8);
            }
            // Zeile 2: Datenraten und Bildraten.
            let by2 = by + p(32);
            u.text.draw(c, ix, by2 + p(16), "Mbit/s", sz(11), ui::DIM, p(3));
            let mut kx = ix + p(96);
            for &m in &BENCH_MBITS {
                let r = ui::Rect { x: kx, y: by2, w: p(44), h: zh };
                if knopf(u, c, r, &m.to_string(), konfig.mbits.contains(&m)) {
                    aktion = HudAktion::BenchMbit(m);
                }
                kx += p(50);
            }
            kx += p(24);
            let fps_label = lang.get(BenchColFps);
            u.text.draw(c, kx, by2 + p(16), fps_label, sz(11), ui::DIM, p(3));
            kx += u.text.width(fps_label, sz(11), p(3)) + p(16);
            for &f in &BENCH_FPSS {
                let r = ui::Rect { x: kx, y: by2, w: p(44), h: zh };
                if knopf(u, c, r, &f.to_string(), konfig.fpss.contains(&f)) {
                    aktion = HudAktion::BenchFps(f);
                }
                kx += p(50);
            }
            // Zeile 3: Dauer, Testbild, Start.
            let by3 = by2 + p(32);
            let dauer_label = lang.get(BenchDuration);
            u.text.draw(c, ix, by3 + p(16), dauer_label, sz(11), ui::DIM, p(3));
            let mut kx = ix + u.text.width(dauer_label, sz(11), p(3)) + p(16);
            let rm = ui::Rect { x: kx, y: by3, w: p(28), h: zh };
            let rp = ui::Rect { x: kx + p(80), y: by3, w: p(28), h: zh };
            if (ui::Rect { x: ix, y: by3, w: rp.x + rp.w - ix, h: zh }).hit(maus.0, maus.1) {
                tip = Some(TipBenchDuration);
            }
            let minus = u.button(c, rm, "", if laeuft { ui::DIM } else { ui::CYAN });
            let plus = u.button(c, rp, "", if laeuft { ui::DIM } else { ui::CYAN });
            for d in 0..2 {
                c.hline(rm.x + p(9), rm.y + rm.h / 2 + d, p(10), ui::CYAN, 255);
                c.hline(rp.x + p(9), rp.y + rp.h / 2 + d, p(10), ui::CYAN, 255);
                c.vline(rp.x + rp.w / 2 + d, rp.y + rp.h / 2 - p(5), p(10), ui::CYAN, 255);
            }
            u.text.draw_centered(c, kx + p(54), by3 + p(16), &format!("{} s", konfig.dauer_s), sz(13), ui::TEXT, p(1));
            if !laeuft && minus {
                aktion = HudAktion::BenchDauer(-1);
            }
            if !laeuft && plus {
                aktion = HudAktion::BenchDauer(1);
            }
            kx = rp.x + rp.w + p(36);
            let r_test = ui::Rect { x: kx, y: by3, w: u.text.width(lang.get(BenchTestPattern), 14, 1) + p(64), h: zh };
            if r_test.hit(maus.0, maus.1) {
                tip = Some(TipBenchTestPattern);
            }
            if u.toggle(c, r_test, lang.get(BenchTestPattern), konfig.testbild) && !laeuft {
                aktion = HudAktion::BenchTestbild;
            }
            let r_start = ui::Rect { x: ix + iw - p(160), y: by3 - p(3), w: p(160), h: p(30) };
            if r_start.hit(maus.0, maus.1) {
                tip = Some(TipBenchStart);
            }
            let (start_text, start_farbe) = if laeuft { (lang.get(BenchAbort), ui::MAGENTA) } else { (lang.get(BenchStart), ui::CYAN) };
            if u.button(c, r_start, start_text, start_farbe) {
                aktion = if laeuft { HudAktion::BenchAbbruch } else { HudAktion::BenchStart };
            }

            // Fortschritt - oder, solange nichts lief, der Hinweis.
            let py = by3 + p(46);
            match &stand.bench {
                Some(b) if b.laeuft => {
                    let phase = match b.phase {
                        BenchPhase::Wechsel => lang.get(CodecSwitching),
                        BenchPhase::Einstellen => lang.get(BenchPhaseSettings),
                        BenchPhase::Einschwingen => lang.get(BenchPhaseSettle),
                        _ => lang.get(BenchPhaseMeasure),
                    };
                    let schritt = lang.get(BenchStep).replace("{n}", &(b.pos + 1).to_string()).replace("{m}", &b.gesamt.to_string());
                    u.text.draw(c, ix, py, &format!("{schritt} · {} · {phase}", b.schritt), sz(11), ui::TEXT, p(1));
                }
                Some(b) => {
                    // Wo die Datei liegt, je Plattform in der Schreibweise
                    // des Systems (siehe secure::config_dir).
                    #[cfg(target_os = "macos")]
                    const ABLAGE: &str = "~/Library/Application Support/QuadChroma/benchmark.txt";
                    #[cfg(not(target_os = "macos"))]
                    const ABLAGE: &str = "%APPDATA%\\QuadChroma\\benchmark.txt";
                    let t = format!(
                        "{} · {} · {ABLAGE}",
                        lang.get(if b.abgebrochen { BenchAborted } else { BenchDone }),
                        lang.get(BenchStep).replace("{n}", &b.ergebnisse.len().to_string()).replace("{m}", &b.gesamt.to_string())
                    );
                    u.text.draw(c, ix, py, &t, sz(11), ui::DIM, p(1));
                }
                None => {
                    let t = lang.get(if konfig.testbild { BenchHintPattern } else { BenchHint });
                    let zeilen = umbruch(u, t, iw, sz(10));
                    let r_hint = ui::Rect { x: ix, y: py - p(12), w: iw, h: zeilen.len() as i32 * p(15) };
                    if r_hint.hit(maus.0, maus.1) {
                        tip = Some(TipBenchHint);
                    }
                    for (i, z) in zeilen.iter().enumerate() {
                        u.text.draw(c, ix, py + i as i32 * p(15), z, sz(10), ui::DIM, p(1));
                    }
                }
            }

            // Tabelle: Codec links, dann elf Zahlenspalten rechtsbuendig.
            // Passen nicht alle Zeilen, zeigt sie den Ausschnitt ab
            // `bench_scroll` (das Mausrad darueber rollt ihn, die App klemmt
            // ihn) und rechts daneben einen Balken.
            if let Some(b) = &stand.bench {
                let ty0 = py + p(26);
                let ende = fy - p(24) - p(58);
                let zeile_h = p(15);
                let platz = ((ende - ty0 - p(18)) / zeile_h).max(0) as usize;
                u.bench_sichtbar = platz;
                u.bench_tabelle = Some(ui::Rect { x: ix, y: ty0 - p(10), w: iw + p(14), h: ende - ty0 + p(10) });
                let spalten: [&str; 11] = [
                    lang.get(BenchColFps), "Mbit/s", lang.get(BenchMeasured), lang.get(BenchChain),
                    lang.get(EncodeTime), lang.get(NetworkTime), lang.get(DecodeTime), lang.get(DisplayStage),
                    lang.get(BenchColDropped), lang.get(BenchColHostCpu), lang.get(ClientCpu),
                ];
                let cw = (iw - p(170)) / spalten.len() as i32;
                let sx = |i: usize| ix + p(170) + (i as i32 + 1) * cw;
                u.text.draw(c, ix, ty0, lang.get(Codec), sz(9), ui::DIM, p(1));
                for (i, n) in spalten.iter().enumerate() {
                    // Manche Sprache nennt die Client-CPU in drei Worten;
                    // was nicht in die Spalte passt, wird gekuerzt.
                    let t = kuerzen(u, n, cw - p(8), sz(9), p(1));
                    u.text.draw_right(c, sx(i), ty0, &t, sz(9), ui::DIM, p(1));
                }
                c.hline(ix, ty0 + p(5), iw, ui::DIM, 60);
                let zeilen = b.ergebnisse.len();
                let von = stand.bench_scroll.min(zeilen.saturating_sub(platz));
                let bis = (von + platz).min(zeilen);
                // Der Balken: schmal, gedaempftes Cyan, Griff so lang wie
                // der sichtbare Anteil - nur, wenn es etwas zu rollen gibt.
                if zeilen > platz {
                    let bx = ix + iw + p(6);
                    let bh = platz as i32 * zeile_h;
                    let by = ty0 + p(8);
                    c.fill(bx, by, p(4), bh, ui::CYAN, 28);
                    let gh = ((bh as i64 * platz as i64) / zeilen as i64).max(p(12) as i64) as i32;
                    let gy = by + ((bh - gh) as i64 * von as i64 / (zeilen - platz) as i64) as i32;
                    c.fill(bx, gy, p(4), gh, ui::CYAN, 130);
                }
                let mut ty = ty0 + p(18);
                for e in &b.ergebnisse[von..bis] {
                    let farbe = if e.bestanden { ui::CYAN } else if e.gescheitert { ui::AMBER } else { ui::DIM };
                    u.text.draw(c, ix, ty, &e.codec, sz(10), farbe, p(1));
                    let strich = |v: f32| if e.gescheitert { "-".to_string() } else { format!("{v:.1}") };
                    let werte: [String; 11] = [
                        e.fps.to_string(),
                        e.mbit.to_string(),
                        if e.gescheitert { lang.get(BenchFailed).to_string() } else { format!("{:.1}", e.fps_gemessen) },
                        strich(e.kette_ms),
                        strich(e.encoder_ms),
                        strich(e.leitung_ms),
                        strich(e.decoder_ms),
                        if b.mit_anzeige { strich(e.anzeige_ms) } else { "-".into() },
                        if b.mit_anzeige && !e.gescheitert { e.verworfen.to_string() } else { "-".into() },
                        if e.gescheitert { "-".into() } else { format!("{:.0} %", e.host_cpu) },
                        if e.gescheitert { "-".into() } else { format!("{:.0} %", e.client_cpu) },
                    ];
                    for (i, w) in werte.iter().enumerate() {
                        u.text.draw_right(c, sx(i), ty, w, sz(10), farbe, p(1));
                    }
                    ty += zeile_h;
                }

                // Empfehlung, mit Knopf zum Uebernehmen - der erst nach dem
                // Lauf greift; die Zeile selbst folgt schon jedem Schritt.
                let ey = fy - p(24) - p(30);
                c.hline(ix, ey - p(22), iw, ui::DIM, 60);
                match &b.empfehlung {
                    Some(e) => {
                        let t = format!(
                            "{}: {}, {} {}, {} Mbit/s – {} {:.1} ms",
                            lang.get(BenchRecommendation), e.codec, e.fps, lang.get(BenchColFps), e.mbit,
                            lang.get(BenchChain), e.kette_ms
                        );
                        u.text.draw(c, ix, ey, &t, sz(13), ui::CYAN, p(1));
                        let r_ok = ui::Rect { x: ix + iw - p(160), y: ey - p(20), w: p(160), h: p(30) };
                        if r_ok.hit(maus.0, maus.1) {
                            tip = Some(TipBenchApply);
                        }
                        if u.button(c, r_ok, lang.get(BenchApply), if laeuft { ui::DIM } else { ui::CYAN }) && !laeuft {
                            aktion = HudAktion::BenchUebernehmen(e.idx, e.mbit, e.fps);
                        }
                    }
                    None if !b.ergebnisse.is_empty() => {
                        u.text.draw(c, ix, ey, lang.get(BenchNoRecommendation), sz(12), ui::AMBER, p(1));
                    }
                    None => {}
                }
            }
        }
        _ => {
            u.text.draw(c, ix, cy, &format!("Noise XX · ChaCha20-Poly1305 · {}", lang.get(EncryptionOn)),
                        sz(14), ui::CYAN, p(1));
            if let Some(sas) = &secure.0 {
                u.text.draw(c, ix, cy + p(50), lang.get(SecuredWith), sz(11), ui::DIM, p(3));
                u.text.draw(c, ix, cy + p(90), sas, sz(28), ui::CYAN, p(6));
            }
            if let Some(fp) = &secure.1 {
                u.text.draw(c, ix + iw / 2, cy + p(50), lang.get(HostFingerprint), sz(11), ui::DIM, p(3));
                u.text.draw(c, ix + iw / 2, cy + p(86), fp, sz(15), ui::TEXT, p(2));
            }
            let r_trennen = ui::Rect { x: ix, y: cy + p(130), w: p(220), h: p(38) };
            if r_trennen.hit(maus.0, maus.1) { tip = Some(TipDisconnect); }
            if u.button(c, r_trennen, lang.get(Disconnect), ui::MAGENTA) {
                aktion = HudAktion::Trennen;
            }
        }
    }

    c.hline(ix, fy - p(24), iw, ui::DIM, 60);
    u.text.draw(c, ix, fy, &format!("ESC · {}", lang.get(Back)), sz(11), ui::DIM, p(3));
    if let Some(k) = tip {
        let text = match &tip_zusatz {
            Some(z) => format!("{}\n{z}", lang.get(k)),
            None => lang.get(k).to_string(),
        };
        tooltip(u, c, &text, maus, ww, wh, sz(11), p(1));
    }
    aktion
}

/// Ein Tooltip neben der Maus: sofort, ohne Wartezeit, ueber allem anderen.
/// Rechts unterhalb des Zeigers; wo das nicht passt, links bzw. oberhalb.
/// Ein Zeilenumbruch im Text erzwingt eine neue Zeile (der zweite Satz auf
/// den Rollen-Knoepfen); sonst wird auf die Breite umbrochen.
fn tooltip(u: &mut ui::Ui, c: &mut ui::Canvas, text: &str, maus: (i32, i32), ww: i32, wh: i32, size: u32, spacing: i32) {
    let innen = 360.min(ww - 40).max(120);
    let zeilen: Vec<String> = text.split('\n').flat_map(|t| umbruch(u, t, innen, size)).collect();
    let breite = zeilen.iter().map(|z| u.text.width(z, size, spacing)).max().unwrap_or(0) + 24;
    let zh = size as i32 + 5;
    let hoehe = zeilen.len() as i32 * zh + 18;
    let mut x = maus.0 + 18;
    let mut y = maus.1 + 22;
    if x + breite > ww - 8 { x = (maus.0 - breite - 10).max(8); }
    if y + hoehe > wh - 8 { y = (maus.1 - hoehe - 12).max(8); }
    c.fill(x, y, breite, hoehe, 0x0a0f18, 245);
    c.hline(x, y, breite, ui::CYAN, 120);
    c.hline(x, y + hoehe - 1, breite, ui::CYAN, 120);
    c.vline(x, y, hoehe, ui::CYAN, 120);
    c.vline(x + breite - 1, y, hoehe, ui::CYAN, 120);
    for (i, z) in zeilen.iter().enumerate() {
        u.text.draw(c, x + 12, y + 13 + i as i32 * zh + size as i32 / 2, z, size, ui::TEXT, spacing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein gemessener Schritt mit sonst unauffaelligen Werten.
    fn schritt(fps: u16, kette_ms: f32, fps_gemessen: f32, verworfen: u64, host_encoder_ms: f32) -> Ergebnis {
        Ergebnis {
            fps, kette_ms, fps_gemessen, verworfen, host_encoder_ms,
            empfangen: fps as u64 * 5,
            budget_ms: 1000.0 / fps as f32,
            ..Ergebnis::default()
        }
    }

    fn besteht(mut e: Ergebnis, mit_anzeige: bool, hat_latenz: bool) -> bool {
        e.pruefen(mit_anzeige, hat_latenz);
        e.bestanden
    }

    #[test]
    fn bench_regel_kette_absolut_nicht_in_bildern() {
        // 26 ms bei 120 fps sind gut drei Bilder - nach der alten Regel
        // (anderthalb Bilder) durchgefallen, nach der neuen bestanden.
        assert!(besteht(schritt(120, 26.0, 119.0, 0, 8.0), true, true));
        // Dieselbe Kette bei 60 fps ebenso: die Grenze haengt nicht an fps.
        assert!(besteht(schritt(60, 26.0, 59.0, 0, 8.0), true, true));
        // Genau die Grenze besteht, knapp darueber nicht.
        assert!(besteht(schritt(60, BENCH_KETTE_MAX_MS, 59.0, 0, 8.0), true, true));
        assert!(!besteht(schritt(60, BENCH_KETTE_MAX_MS + 0.1, 59.0, 0, 8.0), true, true));
        assert!(!besteht(schritt(120, 31.0, 120.0, 0, 8.0), true, true));
    }

    #[test]
    fn bench_regel_encoderzeit_zaehlt_nicht() {
        // Encoder weit ueber seinem Budget (8,3 ms bei 120 fps) - egal,
        // solange die Kette selbst kurz genug ist.
        assert!(besteht(schritt(120, 24.0, 118.0, 0, 15.0), true, true));
    }

    #[test]
    fn bench_regel_bilder_und_verworfene() {
        // 95 % der Zielbildrate: 57 von 60 bestehen, 56 nicht.
        assert!(besteht(schritt(60, 20.0, 57.0, 0, 8.0), true, true));
        assert!(!besteht(schritt(60, 20.0, 56.0, 0, 8.0), true, true));
        // Unter 1 % verworfen: 2 von 300 bestehen, 3 nicht - ohne Anzeige
        // zaehlt Verworfenes nicht.
        assert!(besteht(schritt(60, 20.0, 59.0, 2, 8.0), true, true));
        assert!(!besteht(schritt(60, 20.0, 59.0, 3, 8.0), true, true));
        assert!(besteht(schritt(60, 20.0, 59.0, 3, 8.0), false, true));
        // Ohne Latenzprobe kein Bestehen, gescheitert ebenso wenig.
        assert!(!besteht(schritt(60, 20.0, 59.0, 0, 8.0), true, false));
        let mut g = schritt(60, 20.0, 59.0, 0, 8.0);
        g.gescheitert = true;
        assert!(!besteht(g, true, true));
    }

    #[test]
    fn aud_wird_angehaengt() {
        // Eine HEVC-Zugriffseinheit: VPS-Kopf und eine Scheibe (Typ 1).
        let au = [0u8, 0, 0, 1, 0x40, 0x01, 0xAA, 0, 0, 1, 0x02, 0x01, 0xBB, 0xCC];
        let mit = mit_aud(&au, false);
        assert_eq!(&mit[..au.len()], &au[..]);
        assert_eq!(&mit[au.len()..], &[0, 0, 0, 1, 0x46, 0x01, 0x50]);
        // H.264: SPS (Typ 7) und eine IDR-Scheibe (Typ 5).
        let au = [0u8, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x65, 0xBB];
        let mit = mit_aud(&au, true);
        assert_eq!(&mit[..au.len()], &au[..]);
        assert_eq!(&mit[au.len()..], &[0, 0, 0, 1, 0x09, 0xF0]);
    }

    #[test]
    fn aud_nicht_doppelt() {
        // Endet die Einheit schon mit einem AUD, bleibt sie, wie sie ist.
        let mut au = vec![0u8, 0, 0, 1, 0x02, 0x01, 0xBB];
        au.extend_from_slice(&AUD_HEVC);
        assert_eq!(&*mit_aud(&au, false), &au[..]);
        let mut au = vec![0u8, 0, 0, 1, 0x65, 0xBB];
        au.extend_from_slice(&AUD_H264);
        assert_eq!(&*mit_aud(&au, true), &au[..]);
        // Ein AUD VORNE genuegt nicht - der Parser braucht das Ende des
        // letzten Bildes.
        let mut au = AUD_HEVC.to_vec();
        au.extend_from_slice(&[0, 0, 0, 1, 0x02, 0x01, 0xBB]);
        assert_eq!(mit_aud(&au, false).len(), au.len() + AUD_HEVC.len());
        // Leer oder zu kurz fuer einen Startcode: wird trotzdem ergaenzt.
        assert_eq!(mit_aud(&[], true).len(), AUD_H264.len());
    }

    #[test]
    fn letzter_nal_typ_liest_beide_kopfformen() {
        assert_eq!(letzter_nal_typ(&[0, 0, 1, 0x46, 0x01, 0x50], false), Some(35));
        assert_eq!(letzter_nal_typ(&[0, 0, 0, 1, 0x09, 0xF0], true), Some(9));
        assert_eq!(letzter_nal_typ(&[0, 0, 0, 1, 0x26, 0x01, 0x11, 0, 0, 1, 0x02, 0x01], false), Some(1));
        assert_eq!(letzter_nal_typ(&[1, 2, 3], false), None);
    }

    /// Schluessel aller Test-Hosts auf 127.0.0.1: die Tests teilen sich
    /// eine known_hosts.txt, und dort steht 127.0.0.1 nur einmal.
    fn test_host() -> (Vec<u8>, Vec<u8>) {
        static K: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
        K.get_or_init(|| noise::keypair().unwrap()).clone()
    }

    /// Ruft ensure, bis der Eingabekanal steht oder ein fremder Schluessel
    /// gemeldet ist - der Aufbau laeuft im Hintergrund.
    fn eingabe_abwarten(l: &mut InputLink) {
        let t0 = Instant::now();
        while !l.steht() && l.fremd_gemeldet.is_none() && t0.elapsed() < Duration::from_secs(10) {
            l.ensure();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Gegenstelle am Eingabeport mit richtigem Prologue, aber eigenem
    /// Schluessel (die Pruefsumme kennt jeder, der den Bild-Handschlag gesehen
    /// hat): der Client bricht vor Nachricht 3 ab, der Fremde bekommt den
    /// Schluessel des Clients also nie zu sehen, und nichts geht hinaus. Mit
    /// dem Schluessel des Bildhosts wird der Kanal genommen.
    #[test]
    fn eingabekanal_nur_zum_host_des_bildkanals() {
        use std::net::TcpListener;
        secure::test_identitaet();
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (fremd_priv, _) = noise::keypair().unwrap();
        let hh = vec![0x5a; 32];
        let gegenstelle = |k: Vec<u8>, hh: Vec<u8>| {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap().to_string();
            let t = std::thread::spawn(move || {
                let (s, _) = l.accept().unwrap();
                secure::Secure::accept(s, &noise::prologue_input(&hh), &k).is_ok()
            });
            (addr, t)
        };

        let (addr, t) = gegenstelle(fremd_priv, hh.clone());
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh.clone(), host_pub.clone())));
        eingabe_abwarten(&mut l);
        assert!(!t.join().unwrap(), "Nachricht 3 kam beim Fremden an");
        assert!(!l.steht());
        assert!(l.fremd_gemeldet.is_some());
        l.send(IN_CLIP, b"geheim");
        assert_eq!(l.sent, 0);

        let (addr, t) = gegenstelle(host_priv, hh.clone());
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh, host_pub)));
        eingabe_abwarten(&mut l);
        assert!(t.join().unwrap());
        assert!(l.steht());
    }

    /// Der Eingabekanal haelt niemanden auf, der seine Sperre nimmt: ein
    /// Handschlag, auf den keine Antwort kommt, laeuft im Hintergrund, und
    /// ein Host, der nichts abnimmt, staut nur den Schreibfaden - nicht
    /// `send`. Frueher stand dabei der Fensterfaden (2 s je Versuch, bzw. so
    /// lange, wie 4 MB Zwischenablage brauchen).
    #[test]
    fn eingabekanal_wartet_nie() {
        use std::net::TcpListener;
        secure::test_identitaet();
        let hh = vec![0x3c; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();

        // Stumm: nimmt die Verbindung an und antwortet nie.
        let stumm = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut l = InputLink::new(stumm.local_addr().unwrap().to_string());
        l.set_link(Some((hh.clone(), host_pub.clone())));
        let t0 = Instant::now();
        let mut laengster = Duration::ZERO;
        while t0.elapsed() < Duration::from_millis(3500) {
            let t = Instant::now();
            l.mouse_move(0.5, 0.5);
            laengster = laengster.max(t.elapsed());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(laengster < Duration::from_millis(100), "send wartete {laengster:?}");
        assert!(!l.steht());
        drop(stumm);

        // Nimmt nichts ab: der Handschlag gelingt, dann liest der Host nie.
        let taub = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = taub.local_addr().unwrap().to_string();
        let (fertig_tx, fertig_rx) = std::sync::mpsc::channel::<()>();
        let (hh2, k) = (hh.clone(), host_priv.clone());
        let host = std::thread::spawn(move || {
            let (s, _) = taub.accept().unwrap();
            let h = secure::Secure::accept(s, &noise::prologue_input(&hh2), &k).unwrap();
            // Leitung offen halten, bis der Test fertig ist.
            let _ = fertig_rx.recv();
            drop(h);
        });
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh, host_pub)));
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        let gross = vec![b'x'; 4 * 1024 * 1024];
        let t = Instant::now();
        for _ in 0..6 {
            l.send(IN_CLIP, &gross);
        }
        l.mouse_move(0.1, 0.2);
        assert!(t.elapsed() < Duration::from_millis(1000), "send wartete {:?}", t.elapsed());
        assert_eq!(l.sent, 7);
        let _ = fertig_tx.send(());
        host.join().unwrap();
    }

    /// Der Host meldet MSG_ABGELOEST: der Empfangsfaden nimmt das Ziel
    /// zurueck, die Meldung steht ueber ihren Schluessel da, und es gibt
    /// keine zweite Verbindung - auch nicht nach der Pause von 2 s, nach der
    /// ein getrennter Client sonst neu verbindet (und den neuen verdraengte).
    #[test]
    fn abgeloest_heisst_nicht_wiederverbinden() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        secure::test_identitaet();
        let (host_priv, _) = test_host();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let verbindungen = Arc::new(AtomicUsize::new(0));
        let v = verbindungen.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                v.fetch_add(1, Ordering::SeqCst);
                let Ok(mut h) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) else { continue };
                // Gruss wie beim echten Host, danach gleich die Abloesung.
                let mut m = MAGIC.to_vec();
                m.extend_from_slice(&[MSG_ABGELOEST, 0, 0, 0, 0, 0, 0, 0]);
                let _ = h.write_all(&m);
            }
        });
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        let t0 = Instant::now();
        while shared.lock().unwrap().target.is_some() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        {
            let s = shared.lock().unwrap();
            assert!(s.target.is_none(), "Ziel nicht zurueckgenommen");
            assert_eq!(s.error_key, Some(strings::Key::SessionTakenOver));
            assert_eq!(s.error, None);
        }
        // Frueher: nach 2 s die naechste Verbindung. Drei Sekunden zusehen.
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
        assert!(!shared.lock().unwrap().connected);
        assert_eq!(
            strings::pick("de").get(strings::Key::SessionTakenOver),
            "Ein anderes Gerät hat die Sitzung übernommen."
        );
    }

    /// Ein Host mit anderem Schluessel als dem Pin: der Client bricht im
    /// Handschlag vor Nachricht 3 ab - der Host kennt ihn danach nicht, hat
    /// ihn also auch nicht als Zuschauer angenommen und niemanden
    /// abgeloest -, und er versucht es nicht alle 2 s erneut: das Ziel geht
    /// zurueck, die Meldung bleibt.
    #[test]
    fn falscher_pin_ohne_nachricht_3_und_ohne_wiederholung() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        secure::test_identitaet();
        // 127.0.0.1 ist auf den gemeinsamen Test-Host gepinnt.
        secure::HostPin::laden("127.0.0.1:1").unwrap().eintragen(&test_host().1).unwrap();
        let (fremd_priv, _) = noise::keypair().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let verbindungen = Arc::new(AtomicUsize::new(0));
        let angenommen = Arc::new(AtomicUsize::new(0));
        let (v, a) = (verbindungen.clone(), angenommen.clone());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                v.fetch_add(1, Ordering::SeqCst);
                if secure::Secure::accept(s, &noise::prologue_video(), &fremd_priv).is_ok() {
                    a.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        let t0 = Instant::now();
        while shared.lock().unwrap().target.is_some() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        {
            let s = shared.lock().unwrap();
            assert!(s.target.is_none(), "Ziel nicht zurueckgenommen");
            assert_eq!(s.error.as_ref().map(|m| m.key), Some(strings::Key::HostKeyChanged));
        }
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
        assert_eq!(angenommen.load(Ordering::SeqCst), 0, "Nachricht 3 kam beim Host an");
    }

    /// Dieselbe Meldung ohne ein Bild dazwischen steht nur einmal im
    /// Protokoll - auch die, die erst nach dem Handschlag entsteht ("nicht
    /// gekoppelt"); nach einer echten Sitzung (Bilder) wieder.
    #[test]
    fn verbindungsfehler_nur_einmal_im_protokoll() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let e = Meldung::neu(strings::Key::NotPaired, "nicht gekoppelt");
        let f = Meldung::neu(strings::Key::ErrorProtocol, "anderes Protokoll");
        let mut gemeldet = None;
        assert!(neu_zu_melden(&mut gemeldet, &e, 0));
        assert!(!neu_zu_melden(&mut gemeldet, &e, 0));
        assert!(neu_zu_melden(&mut gemeldet, &e, 7), "nach einer Sitzung mit Bildern");
        assert!(!neu_zu_melden(&mut gemeldet, &e, 7));
        assert!(neu_zu_melden(&mut gemeldet, &f, 7));
        assert!(neu_zu_melden(&mut gemeldet, &e, 7));
        assert!(!e.dauerhaft() && !f.dauerhaft());

        // Echt: ein Host, der nach dem Handschlag zumacht. Der Client
        // versucht es weiter (kein Dauerfehler), protokolliert aber einmal.
        secure::test_identitaet();
        let (host_priv, _) = test_host();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let verbindungen = Arc::new(AtomicUsize::new(0));
        let v = verbindungen.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                v.fetch_add(1, Ordering::SeqCst);
                let _ = secure::Secure::accept(s, &noise::prologue_video(), &host_priv);
            }
        });
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        let t0 = Instant::now();
        while verbindungen.load(Ordering::SeqCst) < 3 && t0.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        let ziel = {
            let mut s = shared.lock().unwrap();
            assert_eq!(s.error_key, Some(strings::Key::NotPaired));
            s.target.take()
        };
        assert!(ziel.is_some(), "Ziel zurueckgenommen - nicht gekoppelt ist kein Dauerfehler");
        assert!(verbindungen.load(Ordering::SeqCst) >= 3);
        let text = std::fs::read_to_string(einstellungen::datei_pfad("protokoll.txt").unwrap()).unwrap();
        let zeilen = text.lines().filter(|z| z.starts_with("Verbindung: Host hat die Leitung nach dem Handschlag")).count();
        assert_eq!(zeilen, 1, "{text}");
    }

    /// Fehler von Leitung und Ablage erscheinen ueber Schluessel: in jeder
    /// Sprache der eigene Satz mit eingesetzten Werten; der deutsche Text mit
    /// Einzelheiten bleibt fuers Protokoll.
    #[test]
    fn meldungen_ueber_schluessel() {
        use secure::Fehler as F;
        use strings::Key::*;
        let pfad = std::path::PathBuf::from("/ablage/known_hosts.txt");
        let f = F::FingerabdruckGeaendert {
            host: "10.0.0.5".into(),
            fingerabdruck: "AAAA-BBBB-CCCC-DDDD".into(),
            pfad: pfad.clone(),
        };
        let m = Meldung::from(f.clone());
        assert_eq!(m.key, HostKeyChanged);
        assert_eq!(m.protokoll, f.to_string());
        assert_eq!(
            m.text(&strings::EN),
            "The fingerprint of 10.0.0.5 has changed (now AAAA-BBBB-CCCC-DDDD). Connection refused. \
             If the host was set up again, delete its line in /ablage/known_hosts.txt."
        );
        assert!(m.text(strings::pick("de")).starts_with("Der Fingerabdruck von 10.0.0.5 hat sich geändert (jetzt AAAA-BBBB-CCCC-DDDD)"));
        // Der Wortlaut des Systems bleibt als Anhang; der Satz davor ist uebersetzt.
        let m = Meldung::from(F::Unlesbar { pfad: pfad.clone(), grund: "Zugriff verweigert (os error 5)".into() });
        assert_eq!(
            m.text(&strings::EN),
            "/ablage/known_hosts.txt cannot be read – not connecting. (Zugriff verweigert (os error 5))"
        );
        // Jede Art hat ihren Schluessel, und kein Platzhalter bleibt offen.
        use std::io::ErrorKind as E;
        let faelle = [
            (F::Ablage("kein Ablageort".into()), StorageUnavailable),
            (F::SchluesselBeschaedigt { pfad: pfad.clone(), laenge: 10 }, KeyFileDamaged),
            (F::KeinUtf8 { pfad: pfad.clone() }, FileNotUtf8),
            (F::Schreiben { pfad: pfad.clone(), grund: "voll".into() }, FileNotWritable),
            (F::Adresse { addr: "x:9001".into(), grund: None }, ErrorAddress),
            (F::Verbindung { addr: "a".into(), art: E::ConnectionRefused, grund: "g".into() }, ErrorConnectRefused),
            (F::Verbindung { addr: "a".into(), art: E::TimedOut, grund: "g".into() }, ErrorTimeout),
            (F::Verbindung { addr: "a".into(), art: E::AddrNotAvailable, grund: "g".into() }, ErrorNoConnection),
            (F::Handschlag { grund: "Laenge: Frist fuer den Handschlag abgelaufen".into(), frist: true, system: None }, ErrorTimeout),
            (F::Handschlag { grund: "Decrypt".into(), frist: false, system: None }, ErrorHandshake),
        ];
        for (f, k) in faelle {
            let m = Meldung::from(f);
            assert_eq!(m.key, k);
            for lang in strings::all() {
                assert!(!m.text(lang).contains('{'), "{k:?} ({}): {}", lang.code, m.text(lang));
            }
        }
        // Handschlag abgebrochen: der Satz aus der Tabelle, dahinter nur der
        // Wortlaut des Systems - nicht der deutsche Grund.
        let m = Meldung::from(F::Handschlag {
            grund: "Laenge: An existing connection was forcibly closed by the remote host. (os error 10054)".into(),
            frist: false,
            system: Some("An existing connection was forcibly closed by the remote host. (os error 10054)".into()),
        });
        assert_eq!(
            m.text(&strings::EN),
            "The secure connection could not be established (An existing connection was forcibly closed by the remote host. (os error 10054))"
        );
        assert!(m.protokoll.starts_with("Handschlag: Laenge:"));
        // Dauerfehler: Pin und Ablage; Leitung und Handschlag nicht.
        let dauer = |f: F| Meldung::from(f).dauerhaft();
        assert!(dauer(F::FingerabdruckGeaendert { host: "h".into(), fingerabdruck: "f".into(), pfad: pfad.clone() }));
        assert!(dauer(F::KeinUtf8 { pfad: pfad.clone() }));
        assert!(dauer(F::Unlesbar { pfad: pfad.clone(), grund: "g".into() }));
        assert!(dauer(F::Schreiben { pfad: pfad.clone(), grund: "g".into() }));
        assert!(dauer(F::SchluesselBeschaedigt { pfad: pfad.clone(), laenge: 3 }));
        assert!(dauer(F::Ablage("x".into())));
        assert!(!dauer(F::Verbindung { addr: "a".into(), art: E::ConnectionRefused, grund: "g".into() }));
        assert!(!dauer(F::Handschlag { grund: "g".into(), frist: true, system: None }));
        // Alle neuen Schluessel stehen englisch und deutsch da.
        for k in [
            SessionTakenOver, HostKeyChanged, KeyFileDamaged, FileUnreadable, FileNotUtf8, FileNotWritable,
            StorageUnavailable, ErrorAddress, ErrorNoConnection, ErrorHandshake, ErrorFfmpegStart, ErrorSound,
            ErrorGpuDisplay, ErrorGpuLost, ErrorPixelFormat, DecoderFallback,
        ] {
            assert!(strings::EN.table.iter().any(|(x, _)| *x == k), "{k:?} fehlt englisch");
            assert!(strings::DE.table.iter().any(|(x, _)| *x == k), "{k:?} fehlt deutsch");
            assert_ne!(strings::EN.get(k), strings::DE.get(k), "{k:?}");
        }
    }

    /// Ein AV1-Eintrag der Koennensliste gilt nie als verfuegbar, auch wenn
    /// ein Host ihn anbietet: nicht waehlbar, nicht im Benchmark.
    #[test]
    fn av1_nie_verfuegbar() {
        let mut p = vec![3u8];
        for (idx, name, c444, zehn) in [(0u8, "HEVC 4:4:4 10 Bit", 1u8, 1u8), (4, "H.264 High", 0, 0), (5, "AV1", 0, 0)] {
            p.extend_from_slice(&[idx, 1, 1, 0, c444, zehn, name.len() as u8]);
            p.extend_from_slice(name.as_bytes());
        }
        let liste = codecs_parsen(&p);
        assert_eq!(
            liste.iter().map(|e| (e.idx, e.available)).collect::<Vec<_>>(),
            vec![(0, true), (4, true), (5, false)]
        );
        let schritte = BenchKonfig::vorgabe(5, true).schritte(&liste);
        assert!(!schritte.is_empty());
        assert!(schritte.iter().all(|s| s.idx != 5), "AV1 im Benchmark");
    }

    /// Zwei NVIDIA-Karten, wie DXGI sie meldet (luid 1 = Grafikkarte,
    /// luid 2 = Grafikkarte 2), dazu die integrierte.
    fn zwei_nvidia() -> Vec<Karte> {
        vec![
            Karte { index: 0, name: "NVIDIA A".into(), vendor: 0x10de, speicher_mb: 8192, hat_ausgang: true, luid: 1, rolle: Rolle::Grafikkarte(1) },
            Karte { index: 1, name: "NVIDIA B".into(), vendor: 0x10de, speicher_mb: 24576, hat_ausgang: false, luid: 2, rolle: Rolle::Grafikkarte(2) },
            Karte { index: 2, name: "Intel".into(), vendor: 0x8086, speicher_mb: 128, hat_ausgang: false, luid: 3, rolle: Rolle::Integriert },
        ]
    }

    #[test]
    fn nvdec_passt_nur_zur_eigenen_karte() {
        use einstellungen::DecoderWunsch as W;
        let k = zwei_nvidia();
        let auf = DecoderPfad::Nvdec;
        assert!(wunsch_passt_mit(W::Gpu, auf(Some(1)), &k));
        assert!(!wunsch_passt_mit(W::Gpu2, auf(Some(1)), &k));
        assert!(wunsch_passt_mit(W::Gpu2, auf(Some(2)), &k));
        assert!(!wunsch_passt_mit(W::Gpu, auf(Some(2)), &k));
        // Karte unbekannt: passt zu keiner ausdruecklichen Rolle, wohl aber
        // zur Automatik.
        assert!(!wunsch_passt_mit(W::Gpu, auf(None), &k));
        assert!(!wunsch_passt_mit(W::Gpu2, auf(None), &k));
        assert!(wunsch_passt_mit(W::Automatik, auf(None), &k));
        assert!(!wunsch_passt_mit(W::Integriert, auf(Some(3)), &k));
        assert!(!wunsch_passt_mit(W::Software, auf(Some(1)), &k));
        // D3D11VA wie bisher nach Rolle.
        assert!(wunsch_passt_mit(W::Integriert, DecoderPfad::D3d11va(Rolle::Integriert), &k));
        assert!(!wunsch_passt_mit(W::Gpu, DecoderPfad::D3d11va(Rolle::Integriert), &k));
    }

    #[test]
    fn nvdec_ziel_ueber_luid() {
        let k = zwei_nvidia();
        // CUDA sortiert die schnellere Karte B nach vorn: DXGI-Index und
        // CUDA-Ordnungszahl sind vertauscht.
        let cuda: &[Option<i64>] = &[Some(2), Some(1)];
        assert_eq!(nvdec_ziel(&k[0], &k, Ok(cuda)), Ok(Some(1)));
        assert_eq!(nvdec_ziel(&k[1], &k, Ok(cuda)), Ok(Some(0)));
        // Karte nicht unter den CUDA-Geraeten (ausgeblendet, TCC) oder
        // nvcuda.dll fehlt: bei zwei NVIDIA-Karten ein Grund statt der
        // falschen Karte.
        assert!(nvdec_ziel(&k[0], &k, Ok(&[Some(2), None])).is_err());
        assert!(nvdec_ziel(&k[0], &k, Err("nvcuda.dll fehlt".into())).is_err());
        // Nur eine NVIDIA-Karte: CUDAs Vorgabe ist sie, wie bisher.
        let eine = vec![k[0].clone(), k[2].clone()];
        assert_eq!(nvdec_ziel(&eine[0], &eine, Err("nvcuda.dll fehlt".into())), Ok(None));
        assert_eq!(nvdec_ziel(&eine[0], &eine, Ok(&[Some(1)])), Ok(Some(0)));
        // Automatik: CUDA-Geraet 0, sonst die einzige NVIDIA-Karte.
        assert_eq!(nvdec_vorgabe(&k, Ok(cuda)), Some(2));
        assert_eq!(nvdec_vorgabe(&k, Err("nvcuda.dll fehlt".into())), None);
        assert_eq!(nvdec_vorgabe(&eine, Err("nvcuda.dll fehlt".into())), Some(1));
        // Eine Karte aus der Automatik passt danach zum Wunsch nach genau ihr.
        let l = nvdec_vorgabe(&eine, Ok(&[Some(1)]));
        assert!(wunsch_passt_mit(einstellungen::DecoderWunsch::Gpu, DecoderPfad::Nvdec(l), &eine));
    }

    /// "NVDEC: <Karte> ist CUDA-Geraet n" steht nur bei einer neuen Zuordnung
    /// im Protokoll, nicht bei jedem Neubau.
    #[test]
    fn nvdec_zuordnung_einmal() {
        let gemeldet = Mutex::new(None);
        assert!(zuordnung_neu(&gemeldet, 1, 1));
        assert!(!zuordnung_neu(&gemeldet, 1, 1));
        assert!(!zuordnung_neu(&gemeldet, 1, 1));
        // Wechsel auf die andere Karte und zurueck: jedes Mal eine Zeile.
        assert!(zuordnung_neu(&gemeldet, 2, 0));
        assert!(zuordnung_neu(&gemeldet, 1, 1));
        // Dieselbe Karte unter anderer Ordnungszahl ist auch neu.
        assert!(zuordnung_neu(&gemeldet, 1, 0));
    }

    /// Die LUID aus CUDA (Speicherbild der Struktur) und die aus DXGI
    /// (anzeige.rs: (HighPart << 32) | LowPart) sind dieselbe Zahl.
    #[test]
    fn luid_wie_dxgi() {
        for (low, high) in [(0x89AB_CDEFu32, 0x12i32), (1, 0), (0xFFFF_FFFF, -1), (0x1234_5678, i32::MIN)] {
            let mut bytes = [0u8; 8];
            bytes[..4].copy_from_slice(&low.to_le_bytes());
            bytes[4..].copy_from_slice(&high.to_le_bytes());
            let dxgi = ((high as i64) << 32) | low as i64;
            assert_eq!(luid_aus_bytes(bytes), dxgi);
        }
    }
}
