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

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;

mod discovery;
mod einstellungen;
mod noise;
mod secure;
mod strings;
mod strings_asia;
mod strings_balt;
mod strings_east;
mod strings_more;
mod strings_north;
mod strings_west;
mod ui;

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod anzeige;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, CustomCursor, Window, WindowId};

const MAGIC: &[u8; 4] = b"QCH1";
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
/// jede Zeile landet ausserdem in %APPDATA%\QuadChroma\protokoll.txt, das
/// bei jedem Start neu beginnt - ein paar Zeilen je Sitzung, mehr nicht.
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
        let base = std::env::var("APPDATA").or_else(|_| std::env::var("HOME")).ok()?;
        let dir = std::path::PathBuf::from(base).join("QuadChroma");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join("protokoll.txt"))
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
const MSG_INFO: u8 = 1;
const MSG_SETTINGS: u8 = 3;
/// So lange muss ESC gehalten werden, bis sich das Menue oeffnet.
const ESC_HALTEDAUER: Duration = Duration::from_secs(2);
/// So lange gilt ein nachgereichter ESC als gedrueckt.
const ESC_TIPPDAUER: Duration = Duration::from_millis(60);

const MSG_TIME: u8 = 4;
const MSG_STAMP: u8 = 5;
const MSG_LAST: u8 = 6;
const MSG_HOSTSTATUS: u8 = 9;
const MSG_VIDEO: u8 = 2;
/// Ab hier neuer Codec: Decoder wegwerfen, das naechste Bild ist ein
/// Schluesselbild mit Parametersaetzen.
const MSG_SWITCH: u8 = 7;
/// Koennensliste des Hosts: welche Codecs er anbietet, mit Flaggen.
const MSG_CODECS: u8 = 8;
const MSG_AUDIO_INFO: u8 = 32;
const MSG_AUDIO: u8 = 33;
const MSG_CLIP: u8 = 48;
/// Zeigerform des Macs: 12 Byte Kopf (u16 Breite, u16 Hoehe, u16 Hotspot x,
/// u16 Hotspot y, u8 sichtbar, u8 Massstab, u16 frei), dann RGBA mit gerader
/// Deckkraft. Kommt nur, wenn sich die Form aendert.
const MSG_CURSOR: u8 = 49;

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
        out.push(CodecEintrag {
            idx: p[o],
            available: p[o + 1] != 0,
            hardware: p[o + 2] != 0,
            conversion: p[o + 3] != 0,
            chroma444: p[o + 4] != 0,
            ten_bit: p[o + 5] != 0,
            name,
        });
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
    last_decode_ms: f32,
    error: Option<String>,
    /// Pruefsumme des Bildkanals. Der Eingabekanal braucht sie, sonst laesst
    /// ihn der Host nicht herein.
    link: Option<Vec<u8>>,
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
    /// Warum NVDEC nicht laeuft, obwohl er gewuenscht war - fuer Anzeige
    /// und Protokoll. None, wenn er laeuft oder gar nicht gewuenscht war.
    decoder_hinweis: Option<String>,
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

#[cfg(not(windows))]
fn prozesszeit_100ns() -> Option<u64> {
    None
}

/// Netz- und Decodierschleife. Laeuft in einem eigenen Faden und legt immer nur
/// das neueste Bild ab: lieber eines auslassen als Verzoegerung aufbauen.
fn stream_thread(shared: Arc<Mutex<Shared>>, input: Arc<Mutex<InputLink>>) {
    if let Err(e) = ffmpeg::init() {
        shared.lock().unwrap().error = Some(format!("FFmpeg-Start fehlgeschlagen: {e}"));
        return;
    }
    // Im Pruefmodus alles einsammeln, was FFmpeg zu sagen hat; im Fenster
    // nur Warnungen und Fehler - fuer die Datei und fuer den Grund, wenn
    // ein Hardware-Decoder schon beim Oeffnen scheitert.
    protokoll::einschalten(std::env::args().any(|a| a == "--headless"));

    loop {
        let addr = { shared.lock().unwrap().target.clone() };
        let Some(addr) = addr else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        match run_session(&addr, &shared, &input) {
            Ok(()) => {}
            Err(e) => {
                let mut s = shared.lock().unwrap();
                // Eine gewollte Trennung kappt die Leitung - der Lesefehler
                // danach ist kein Fehler und wird nicht angezeigt.
                if s.target.is_some() {
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

/// Welchen Weg der Decoder tatsaechlich nimmt. Das ist das Ergebnis der
/// Wahl, nicht der Wunsch: bei Automatik kann beides herauskommen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecoderPfad {
    /// NVIDIA-Karte ueber die cuvid-Decoder von FFmpeg (hevc_cuvid, h264_cuvid).
    Nvdec,
    /// Der eingebaute Software-Decoder von FFmpeg auf der CPU.
    Software,
}

impl DecoderPfad {
    pub fn name(self) -> &'static str {
        match self {
            DecoderPfad::Nvdec => "NVDEC",
            DecoderPfad::Software => "Software",
        }
    }

    pub fn hardware(self) -> bool {
        self == DecoderPfad::Nvdec
    }
}

/// Ergebnis von `decoder_bauen`: der Decoder und alles, was man ueber ihn
/// wissen will, um es anzuzeigen und ins Protokoll zu schreiben.
struct DecoderBau {
    decoder: ffmpeg::decoder::Video,
    pfad: DecoderPfad,
    /// FFmpeg-Name des Decoders, etwa "hevc_cuvid" oder "hevc".
    codec: &'static str,
    /// Warum es nicht NVDEC wurde, obwohl er gewuenscht war. None, wenn er
    /// laeuft oder Software ausdruecklich gewuenscht war.
    grund: Option<String>,
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

    /// Eine Zeile fuer das Protokoll.
    fn meldung(&self) -> String {
        match &self.grund {
            None => format!("Decoder: {} ({})", self.pfad.name(), self.codec),
            Some(g) => format!("Decoder: {} ({}) - NVDEC nicht verfuegbar: {g}", self.pfad.name(), self.codec),
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
fn nvdec_decoder(h264: bool) -> Result<ffmpeg::decoder::Video, String> {
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
    dec.video().map_err(|e| {
        let worte = protokoll::fehler_abholen();
        match worte.first() {
            Some(w) => format!("{name}: {w}"),
            None => format!("{name}: {e}"),
        }
    })
}

/// Den Hardware-Decoder durch Software ersetzen und den Grund festhalten.
/// Was FFmpeg dazu gesagt hat, steht im Protokoll direkt davor; in den
/// Grund kommt es nicht, der muss in eine Zeile der Statistik passen.
fn auf_software(h264: bool, grund: String) -> Result<DecoderBau, String> {
    protokoll::fehler_verwerfen();
    let d = software_decoder(h264)?;
    Ok(DecoderBau::neu(d, DecoderPfad::Software, if h264 { "h264" } else { "hevc" }, Some(grund)))
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

/// Decoder fuer HEVC oder H.264 nach Wunsch bauen. Wird beim Start, bei
/// jedem Codecwechsel und bei jedem Wechsel des Wunsches gerufen - der alte
/// Decoder wird dann einfach fallen gelassen.
///
/// Reihenfolge bei Automatik und NVIDIA: erst der cuvid-Decoder des Codecs
/// (hevc_cuvid / h264_cuvid), scheitert der, der Software-Decoder ueber die
/// Codec-Kennung. Bei Software gleich der. Der Unterschied zwischen Automatik
/// und NVIDIA liegt beim Aufrufer: bei NVIDIA wird der Grund laut gemeldet.
fn decoder_bauen(h264: bool, wunsch: einstellungen::DecoderWunsch) -> Result<DecoderBau, String> {
    use einstellungen::DecoderWunsch as W;
    let sw_name = if h264 { "h264" } else { "hevc" };
    let hw_name = if h264 { "h264_cuvid" } else { "hevc_cuvid" };
    let grund = match wunsch {
        W::Software => None,
        W::Automatik | W::Nvidia => match nvdec_decoder(h264) {
            Ok(decoder) => {
                return Ok(DecoderBau::neu(decoder, DecoderPfad::Nvdec, hw_name, None));
            }
            Err(e) => Some(e),
        },
    };
    let decoder = software_decoder(h264)?;
    Ok(DecoderBau::neu(decoder, DecoderPfad::Software, sw_name, grund))
}

/// Ergebnis eines frisch gebauten Decoders in `Shared` eintragen: Pfad,
/// Hinweis und die Protokollzeile. Bei ausdruecklichem NVIDIA-Wunsch wird
/// ein Rueckfall zusaetzlich als Fehler gezeigt - wer die Karte verlangt,
/// soll erfahren, dass er sie nicht bekommt.
fn decoder_melden(shared: &Arc<Mutex<Shared>>, bau: &DecoderBau, wunsch: einstellungen::DecoderWunsch) {
    protokoll::zeile(bau.meldung());
    let mut s = shared.lock().unwrap();
    s.decoder_pfad = Some(bau.pfad);
    s.decoder_hinweis = bau.grund.clone();
    if wunsch == einstellungen::DecoderWunsch::Nvidia {
        if let Some(g) = &bau.grund {
            s.error = Some(format!("NVDEC nicht verfuegbar: {g}"));
        }
    }
}

/// Ist das ein Fehler, der einen Hardware-Decoder als kaputt ausweist?
/// EAGAIN heisst nur "gerade nichts da", EOF "fertig" - beides ist normal.
fn decoder_defekt(e: &ffmpeg::Error) -> bool {
    !matches!(e, ffmpeg::Error::Other { errno: ffmpeg::util::error::EAGAIN } | ffmpeg::Error::Eof)
}

fn run_session(addr: &str, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) -> Result<(), String> {
    // Erst der Handschlag, dann erst Nutzdaten. Vorher geht nichts ueber die
    // Leitung, was jemand mitlesen koennte.
    let mut sock = secure::Secure::connect(addr, &noise::prologue_video())?;
    shared.lock().unwrap().abbruch = sock.abbruchgriff();
    let first = secure::check_known_host(addr, &sock.peer)?;
    let fp = sock.peer_fingerprint();
    {
        let mut s = shared.lock().unwrap();
        s.connected = true;
        s.error = None;
        s.error_key = None;
        s.link = Some(sock.handshake_hash.clone());
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
        return Err(strings::EN.get(strings::Key::NotPaired).to_string());
    }
    if &magic != MAGIC {
        return Err("Gegenstelle spricht ein anderes Protokoll".into());
    }

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
    let mut bau = decoder_bauen(false, wunsch)?;
    decoder_melden(shared, &bau, wunsch);
    // Wofuer der Decoder gebaut ist. Die Strominfo entscheidet gleich, ob
    // das passt - der Host kann laengst auf einem anderen Codec stehen.
    let mut decoder_h264 = false;
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
    // Versuchsschalter: jedem Paket fuer einen Hardware-Decoder eine
    // Zugriffseinheiten-Grenze (AUD) ANHAENGEN. NVIDIAs Parser erkennt das
    // Ende eines Bildes erst am naechsten NAL, das kein Bildinhalt ist -
    // ohne AUD also erst am naechsten Paket, ein Bild Verzoegerung. FFmpegs
    // cuvid setzt das Ende-Kennzeichen des Parsers nie; die angehaengte AUD
    // ist der einzige Weg von aussen. Fuer den Software-Decoder ohne Belang.
    let aud_anhang = std::env::args().any(|a| a == "--aud");

    let mut info: Option<StreamInfo> = None;
    #[cfg(windows)]
    let mut sound: Option<audio::AudioOut> = None;
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
                let passt = match w {
                    einstellungen::DecoderWunsch::Software => bau.pfad == DecoderPfad::Software,
                    _ => bau.pfad == DecoderPfad::Nvdec,
                };
                if !passt {
                    bau = decoder_bauen(decoder_h264, wunsch)?;
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

        sock.read_exact(&mut hdr).map_err(|e| format!("Kopf: {e}"))?;
        let msg_type = hdr[0];
        let flags = hdr[1];
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        if len > 64 * 1024 * 1024 {
            return Err("unplausible Nachrichtenlaenge".into());
        }
        payload.resize(len, 0);
        sock.read_exact(&mut payload).map_err(|e| format!("Daten: {e}"))?;

        match msg_type {
            MSG_INFO => {
                if let Some(i) = StreamInfo::parse(&payload) {
                    // Steht der Host schon auf einem anderen Codec als dem, fuer
                    // den der Decoder gebaut wurde, muss der Decoder jetzt
                    // passen. Sonst versucht ein HEVC-Decoder, H.264 zu lesen,
                    // und es kommt nie ein Bild - so geschehen heute Morgen.
                    let h264 = i.codec == 2;
                    if h264 != decoder_h264 {
                        bau = decoder_bauen(h264, wunsch)?;
                        decoder_melden(shared, &bau, wunsch);
                        decoder_h264 = h264;
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
                bau = decoder_bauen(w.is_h264, wunsch)?;
                decoder_melden(shared, &bau, wunsch);
                decoder_h264 = w.is_h264;
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
                // Nach einem Wechsel muss das erste Bild ein Schluesselbild
                // sein (Flaggenbit 0). Alles andere gehoert noch zum alten
                // Codec oder ist ohne Parametersaetze nicht decodierbar.
                if warte_auf_schluesselbild {
                    if flags & 1 == 0 {
                        continue;
                    }
                    warte_auf_schluesselbild = false;
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
                let mut packet = if aud_anhang && bau.pfad.hardware() {
                    // AUD: HEVC NAL-Typ 35 mit pic_type "beliebig", H.264
                    // NAL-Typ 9 mit primary_pic_type 7 - jeweils samt
                    // Startcode und Abschlussbit.
                    const AUD_HEVC: [u8; 7] = [0, 0, 0, 1, 0x46, 0x01, 0x50];
                    const AUD_H264: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];
                    let aud: &[u8] = if decoder_h264 { &AUD_H264 } else { &AUD_HEVC };
                    let mut mit = Vec::with_capacity(payload.len() + aud.len());
                    mit.extend_from_slice(&payload);
                    mit.extend_from_slice(aud);
                    ffmpeg::Packet::copy(&mit)
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
                    let grund = match fehler {
                        None => {
                            bau.fehler_folge = 0;
                            if !bau.stumm() {
                                break;
                            }
                            format!(
                                "{} nimmt Pakete an, liefert aber kein Bild ({} Pakete, {:.1} s)",
                                bau.codec, bau.pakete_seit_bild, bau.stumm_seit().as_secs_f32()
                            )
                        }
                        Some(e) => {
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
                            format!("{} scheitert an Paket {}: {e}", bau.codec, bau.pakete)
                        }
                    };
                    // Der Hardware-Decoder ist nichts wert: Software bauen
                    // und den Grund festhalten. Der Wunsch bleibt, wie er
                    // war - beim naechsten Codecwechsel wird die Karte wieder
                    // probiert, mit einem anderen Codec kann sie ja gehen.
                    bau = auf_software(decoder_h264, grund)?;
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
                            Err(e) => (Bild::Rgb(Frame { bereit_us: client_us(), ..dunkles_bild(w, h) }), Some(e)),
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
                    let grund = format!("{} liefert das Format {format}, das der Client nicht wandeln kann", bau.codec);
                    bau = auf_software(decoder_h264, grund)?;
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
                #[cfg(windows)]
                if len >= 8 {
                    let rate = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let ch = payload[4] as u16;
                    match audio::AudioOut::new(rate, ch) {
                        Ok(a) => { sound = Some(a); }
                        Err(e) => { shared.lock().unwrap().error = Some(format!("Ton: {e}")); }
                    }
                }
            }
            MSG_AUDIO => {
                #[cfg(windows)]
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
                #[cfg(windows)]
                if let Ok(text) = std::str::from_utf8(&payload) {
                    clipboard::set(text);
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

const IN_MOVE: u8 = 16;
const IN_BUTTON: u8 = 17;
const IN_SCROLL: u8 = 18;
const IN_KEY: u8 = 19;
const IN_CLIP: u8 = 48;
const IN_SETTINGS: u8 = 64;
const IN_TIME: u8 = 65;
/// Codecwunsch: ein Byte, der Index aus der Koennensliste.
const IN_CODEC: u8 = 66;

// Umschalter als Bitmaske, damit der Mac denselben Zustand sieht wie Windows.
const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_ALT: u32 = 4;
const MOD_CMD: u32 = 8;

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
struct InputLink {
    sock: Option<secure::Secure>,
    addr: String,
    last: (f32, f32),
    sent: u64,
    /// Pruefsumme des Bildkanals. Ohne sie laesst der Host diesen Kanal nicht zu.
    link: Option<Vec<u8>>,
    /// Welche Tasten gerade als gedrueckt gelten. Ohne diese Liste bleiben
    /// beim Fokusverlust Tasten auf dem Mac haengen - WASD laeuft dann gegen
    /// die Wand, und ein haengendes Strg macht aus jedem Klick einen Rechtsklick.
    gedrueckt: std::collections::HashSet<u16>,
    /// Wann zuletzt ein Verbindungsversuch lief. Bremst die Wiederholungen,
    /// damit ein toter Host die Oberflaeche nicht im Sekundentakt anhaelt.
    letzter_versuch: Option<Instant>,
}

impl InputLink {
    fn new(addr: String) -> Self {
        Self {
            sock: None,
            addr,
            last: (0.5, 0.5),
            sent: 0,
            link: None,
            gedrueckt: std::collections::HashSet::new(),
            letzter_versuch: None,
        }
    }

    /// Adresse wechseln, etwa wenn ein anderer Host gewaehlt wurde.
    fn set_addr(&mut self, addr: String) {
        if addr != self.addr {
            self.addr = addr;
            self.sock = None;
        }
    }

    /// Bindung an den Bildkanal setzen. Wechselt sie, wird neu verbunden.
    fn set_link(&mut self, link: Option<Vec<u8>>) {
        if self.link != link {
            self.link = link;
            self.sock = None;
        }
    }

    fn ensure(&mut self) {
        if self.sock.is_some() || self.addr.is_empty() {
            return;
        }
        let Some(link) = self.link.as_ref() else { return };
        if let Some(t) = self.letzter_versuch {
            if t.elapsed() < Duration::from_secs(1) {
                return;
            }
        }
        self.letzter_versuch = Some(Instant::now());
        if let Ok(s) = secure::Secure::connect(&self.addr, &noise::prologue_input(link)) {
            self.sock = Some(s);
            self.letzter_versuch = None;
        }
    }

    fn send(&mut self, t: u8, payload: &[u8]) {
        self.ensure();
        let Some(sock) = self.sock.as_mut() else { return };
        let mut buf = Vec::with_capacity(8 + payload.len());
        buf.push(t);
        buf.push(0);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if sock.write_all(&buf).is_err() {
            self.sock = None;
        } else {
            self.sent += 1;
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
    fn trennen(&mut self) {
        self.sock = None;
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
        #[cfg(windows)]
        {
            use einstellungen::AnzeigeWunsch as W;
            if self.anzeige_wunsch != W::Cpu {
                let s = window.inner_size();
                let bau = fenster_hwnd(&window)
                    .ok_or_else(|| "kein Win32-Fenster".to_string())
                    .and_then(|h| anzeige::Gpu::neu(h, s.width, s.height, self.anzeige_wunsch == W::Warp, self.adapter_wunsch));
                match bau {
                    Ok(g) => {
                        self.anzeige_name = format!("D3D11 · {}", g.adapter.name);
                        self.sofort = g.tearing;
                        // Ab jetzt legt der Empfangsfaden rohe Bilder ab; die
                        // Umrechnung nach RGB macht Stufe 1 auf der Karte.
                        self.shared.lock().unwrap().gpu_pfad = true;
                        self.anzeige = Anzeige::Gpu(g);
                    }
                    Err(e) => {
                        protokoll::zeile(format!("Anzeige: Rueckfall auf Software: {e}"));
                        if self.anzeige_wunsch == W::Gpu {
                            self.shared.lock().unwrap().error = Some(format!("Grafikkarte nicht nutzbar, Anzeige ueber Software: {e}"));
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
                if let Some(w) = &self.window {
                    let s = w.inner_size();
                    let nx = position.x as f32 / s.width.max(1) as f32;
                    let ny = position.y as f32 / s.height.max(1) as f32;
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
                // im Menue landete zusaetzlich auf dem Mac.
                if self.hud_offen {
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
                if let Some(w) = self.cfg.fuer_host(f) {
                    self.input.lock().unwrap().settings(w.mbit, w.fps, w.gaming, w.fest, w.ton);
                    self.shared.lock().unwrap().ton = w.ton;
                } else {
                    self.shared.lock().unwrap().ton = true;
                }
                self.angewandt_fuer = Some(f.clone());
            }
            (None, _) => self.angewandt_fuer = None,
            _ => {}
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
        w.set_cursor_visible(z.sichtbar);
        self.zeiger_eigen = true;
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

    fn verbindung_trennen(&mut self) {
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
            // Eigene Prozessorlast, dasselbe Mass wie der Task-Manager: Kernel-
            // plus Nutzerzeit des Prozesses, geteilt durch Wandzeit mal
            // logische Kerne - "Prozent der Maschine". So ist die Zahl direkt
            // mit den 21,8 % vergleichbar, die der Task-Manager auf dem Laptop
            // fuer den CPU-Weg zeigte.
            if let Some(jetzt) = prozesszeit_100ns() {
                let (vorher, seit) = self.cpu_zeiten;
                let wand = seit.elapsed().as_secs_f64() * 1e7;
                let kerne = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
                if wand > 0.0 {
                    self.cpu_eigen = (jetzt.saturating_sub(vorher) as f64 / (wand * kerne) * 100.0) as f32;
                }
                self.cpu_zeiten = (jetzt, Instant::now());
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
                (s.last_decode_ms, s.error.clone())
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
            return Ok(()); // minimiert: nichts konfigurieren, nichts zeichnen
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
        if self.ui_puffer.len() != n {
            // Erste Zeichnung oder neue Groesse: ganz durchsichtiger Grund,
            // die Oberflaeche wird komplett neu gerastert.
            self.ui_puffer = vec![0xff00_0000u32; n];
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
                anzeige::Praesentiert::Fehler(e) => Err(Ausfall::Fehler(e)),
                anzeige::Praesentiert::GeraetWeg(grund) => Err(Ausfall::GeraetWeg(grund)),
            }
        } else {
            self.praesentation_ausstehend = true;
            self.shared.lock().unwrap().ausgelassen += 1;
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
                let bau = fenster_hwnd(&w)
                    .ok_or_else(|| "kein Win32-Fenster".to_string())
                    .and_then(|h| anzeige::Gpu::neu(h, s.width, s.height, self.anzeige_wunsch == W::Warp, self.adapter_wunsch));
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
        let meldung = "Grafikkarte verloren - bitte mit --anzeige cpu neu starten";
        protokoll::zeile(format!("Anzeige: {meldung}"));
        self.anzeige = Anzeige::Keine;
        let mut s = self.shared.lock().unwrap();
        s.gpu_pfad = false;
        s.error = Some(meldung.into());
        drop(s);
        if let Some(w) = &self.window {
            w.set_title(&format!("QuadChroma - {meldung}"));
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
                        None => s.error.clone(),
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
                            None => s.error.clone(),
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
                            (s.last_decode_ms, s.info, s.dropped, s.error.clone(), s.connected),
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
            HudAktion::Stellen(m, f, g, fx, ton) => {
                self.input.lock().unwrap().settings(m, f, g, fx, ton);
                self.shared.lock().unwrap().ton = ton;
                if let Some(fp) = &self.angewandt_fuer {
                    let fp = fp.clone();
                    self.cfg.host_merken(
                        &fp,
                        einstellungen::HostWerte { mbit: m, fps: f, gaming: g, fest: fx, ton },
                    );
                }
            }
            HudAktion::Codec(idx) => {
                // Erst den Hinweis setzen, dann den Wunsch abschicken.
                // Andersherum koennte der Empfangsfaden Nachricht 7
                // und das erste Bild dazwischen verarbeiten und den
                // Hinweis loeschen, bevor er ueberhaupt steht - dann
                // bliebe er bis zum Ablauf der Frist haengen.
                self.shared.lock().unwrap().codec_wechsel = Some(Instant::now());
                self.input.lock().unwrap().codec(idx);
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

    if let Some(e) = error {
        u.text.draw_centered(c, cx, by + 76, e, 13, ui::AMBER, 1);
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
                    if nerd { Some(hl) } else { None }, (Some(DecoderPfad::Nvdec), None), &client);
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
    if view == "hud" || view == "hud2" {
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
        let stand = HudStand {
            vollbild: true, pixelgenau: false, statistik: true, nerd: true,
            wahl: einstellungen::StatWahl::default(),
            codecs,
            codec_idx: None,
            wechsel: false,
            decoder: einstellungen::DecoderWunsch::Automatik,
            decoder_aktiv: Some(DecoderPfad::Nvdec),
        };
        let reiter = match view { "hud2" => 1u8, "hud3" => 2, _ => 0 };
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
    {
        let mut c = ui::Canvas::neu(&mut buf, w, h);
        let _ = start_screen(&mut u, &mut c, lang, &hosts, "192.168.178.194:9001", None);
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

    // Erstes Argument, das kein Schalter ist, ist die Adresse.
    let addr = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(|a| adresse_vollstaendig(&a))
        .unwrap_or_default();

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
        for (h264, codec) in [(false, "HEVC"), (true, "H.264")] {
            for w in [W::Automatik, W::Software, W::Nvidia] {
                match decoder_bauen(h264, w) {
                    Ok(bau) => println!("{codec} / Wunsch {}: {}", w.schluessel(), bau.meldung()),
                    Err(e) => println!("{codec} / Wunsch {}: Fehler: {e}", w.schluessel()),
                }
                for z in protokoll::abholen() {
                    println!("    {z}");
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

    // Zwischenablage: Was auf Windows kopiert wird, geht zum Mac. Die Uebergabe
    // aus der Fensterschleife heraus ist bewusst nicht blockierend.
    #[cfg(windows)]
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
    // --decoder auto|software|nvidia erzwingt fuer diesen Lauf einen Pfad,
    // ohne die gespeicherte Wahl anzufassen.
    let decoder_wunsch = std::env::args()
        .position(|a| a == "--decoder")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| einstellungen::DecoderWunsch::aus(&v))
        .unwrap_or(cfg.decoder);
    // --anzeige auto|gpu|cpu|warp ebenso fuer die Anzeige; --adapter n nimmt
    // genau den n-ten Adapter aus der Liste im Protokoll.
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
        println!("Decoderwunsch: {}", decoder_wunsch.schluessel());
        let start = Instant::now();
        let mut last = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(3));
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
            let pfad = s.decoder_pfad.map(|p| p.name()).unwrap_or("-");
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
                start.elapsed().as_secs_f32(), n, (n - last) as f32 / 3.0, codec, pfad, lat, hl, s.error
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
                // Einstellungen auch ohne Fenster pruefen lassen.
                if let Some(w) = wish.take() {
                    l.settings(w.0, w.1, w.2, w.3, w.4);
                    println!(
                        "Einstellung gewuenscht: {} Mbit/s, {} fps, Gaming {}, feste Bildrate {}, Ton {}",
                        w.0, w.1, w.2, w.3, w.4
                    );
                }
                // --codec <idx> wuenscht einmal einen Kandidaten. Erst, wenn der
                // Eingabekanal wirklich steht - `send` wirft sonst stumm weg,
                // und der Wunsch waere verloren, bevor der Host ihn je sah.
                if l.sock.is_some() {
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
        sofort: false,
        bild_da: None,
        bereit_ausstehend: None,
        ui_puffer: Vec::new(),
        ui_kasten_alt: None,
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
    let namen = [lang.get(TabPicture), lang.get(TabDisplay), lang.get(Encryption)];
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
    let soll = stell.map(|x| x.1 as f32).or_else(|| info.map(|i| i.fps as f32)).unwrap_or(60.0);
    let kw = (iw - p(16)) / 2;
    for (kx, welche) in [(ix, 0usize), (ix + kw + p(16), 1usize)] {
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
    c.hline(ix, y0 + p(166), iw, ui::DIM, 60);

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
            let d = stellen(u, c, ix, cy, lang.get(MaxBitrate), format!("{mbit} Mbit/s"));
            if d != 0 {
                let schritt = if mbit >= 100 { 25 } else if mbit >= 30 { 10 } else { 5 };
                aktion = HudAktion::Stellen((mbit as i32 + d * schritt).clamp(2, 500) as u32, fps_soll, gaming, fest, ton);
            }
            let d = stellen(u, c, ix + iw / 2, cy, lang.get(MaxFps), format!("{fps_soll}"));
            if d != 0 {
                aktion = HudAktion::Stellen(mbit, (fps_soll as i32 + d * 10).clamp(10, 240) as u16, gaming, fest, ton);
            }
            if u.toggle(c, ui::Rect { x: ix, y: cy + p(70), w: iw / 2 - p(30), h: p(28) }, lang.get(GamingMode), gaming) {
                aktion = HudAktion::Stellen(mbit, fps_soll, !gaming, fest, ton);
            }
            if u.toggle(c, ui::Rect { x: ix + iw / 2, y: cy + p(70), w: iw / 2 - p(30), h: p(28) }, lang.get(FixedRate), fest) {
                aktion = HudAktion::Stellen(mbit, fps_soll, gaming, !fest, ton);
            }
            // Ton: unter dem Spielmodus, neben dem Hinweis zur festen Bildrate.
            // Aus heisst aus - beim Host (kein Paket mehr) und hier (nichts
            // mehr abgespielt, falls der Host den Schalter nicht kennt).
            if u.toggle(c, ui::Rect { x: ix, y: cy + p(104), w: iw / 2 - p(30), h: p(28) }, lang.get(Sound), ton) {
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
            let zeile = |u: &mut ui::Ui, c: &mut ui::Canvas, sx: i32, sy: i32, t: &str, an: bool, id: u8, akt: &mut HudAktion| {
                if u.toggle(c, ui::Rect { x: sx, y: sy, w: sp - p(40), h: p(26) }, t, an) {
                    *akt = HudAktion::Schalter(id);
                }
            };
            zeile(u, c, ix, cy, lang.get(Fullscreen), stand.vollbild, SCH_VOLLBILD, &mut aktion);
            zeile(u, c, ix, cy + p(34), lang.get(PixelExact), stand.pixelgenau, SCH_PIXELGENAU, &mut aktion);
            zeile(u, c, ix, cy + p(68), lang.get(ShowOverlay), stand.statistik, SCH_STATISTIK, &mut aktion);
            zeile(u, c, ix, cy + p(102), lang.get(NerdMode), stand.nerd, SCH_NERD, &mut aktion);
            // --- Decoderwahl: drei Knoepfe, der gewuenschte in Cyan. Dahinter
            // steht, was wirklich laeuft - bei Automatik ist das die eigentliche
            // Auskunft, und bei einem Rueckfall sieht man ihn hier sofort.
            {
                use einstellungen::DecoderWunsch as W;
                let oy = cy + p(150);
                let label = match stand.decoder_aktiv {
                    Some(pf) => format!("{} · {}", lang.get(DecoderLabel), pf.name()),
                    None => lang.get(DecoderLabel).to_string(),
                };
                u.text.draw(c, ix, oy, &label, sz(11), ui::DIM, p(3));
                let mut kx = ix;
                let ky = oy + p(10);
                for (w, k) in [(W::Automatik, DecoderAuto), (W::Software, DecoderSoftware), (W::Nvidia, DecoderNvidia)] {
                    let name = lang.get(k);
                    let bw = u.text.width(name, 15, 2) + p(28);
                    // Die rechte Spalte faengt bei ix + sp an; was dort hinein
                    // ragen wuerde, wird nicht mehr gezeichnet.
                    if kx + bw > ix + sp - p(20) {
                        break;
                    }
                    let r = ui::Rect { x: kx, y: ky, w: bw, h: p(30) };
                    let farbe = if stand.decoder == w { ui::CYAN } else { ui::DIM };
                    if u.button(c, r, name, farbe) && stand.decoder != w {
                        aktion = HudAktion::Decoder(w);
                    }
                    kx += bw + p(10);
                }
            }
            let w = stand.wahl;
            // Ueberschrift ueber die rechte Spalte, nicht unter die linke.
            u.text.draw(c, ix + sp, cy - p(16), lang.get(ShowOverlay), sz(10), ui::DIM, p(3));
            zeile(u, c, ix + sp, cy, lang.get(Fps), w.fps, SCH_STAT_FPS, &mut aktion);
            zeile(u, c, ix + sp, cy + p(30), lang.get(Latency), w.latenz, SCH_STAT_LATENZ, &mut aktion);
            zeile(u, c, ix + sp, cy + p(60), lang.get(DecodeTime), w.teile, SCH_STAT_TEILE, &mut aktion);
            zeile(u, c, ix + sp, cy + p(90), lang.get(Resolution), w.aufloesung, SCH_STAT_AUFL, &mut aktion);
            zeile(u, c, ix + sp, cy + p(120), lang.get(Codec), w.codec, SCH_STAT_CODEC, &mut aktion);
            zeile(u, c, ix + sp, cy + p(150), lang.get(Dropped), w.verworfen, SCH_STAT_VERW, &mut aktion);
            zeile(u, c, ix + sp, cy + p(180), lang.get(SecuredWith), w.code, SCH_STAT_CODE, &mut aktion);
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
            if u.button(c, ui::Rect { x: ix, y: cy + p(130), w: p(220), h: p(38) }, lang.get(Disconnect), ui::MAGENTA) {
                aktion = HudAktion::Trennen;
            }
        }
    }

    c.hline(ix, fy - p(24), iw, ui::DIM, 60);
    u.text.draw(c, ix, fy, &format!("ESC · {}", lang.get(Back)), sz(11), ui::DIM, p(3));
    aktion
}
