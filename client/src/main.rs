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
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const MAGIC: &[u8; 4] = b"QCH1";
/// Eigene Uhr des Clients. Der Nullpunkt ist beliebig; fuer den Vergleich mit
/// dem Host zaehlt nur der Versatz, den der Zeitabgleich ausrechnet.
fn client_us() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
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
    /// Summe: Aufnahme bis anzeigebereit.
    gesamt_ms: f32,
    /// Wie viele Bilder in den Mittelwert eingegangen sind.
    bilder: u32,
}

/// Fertiges Bild in RGB, bereit zum Anzeigen.
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u32>, // 0x00RRGGBB, wie softbuffer es erwartet
}

#[derive(Default)]
struct Shared {
    /// Adresse, mit der sich der Empfangsfaden verbinden soll. None = warten.
    target: Option<String>,
    connected: bool,
    frame: Option<Frame>,
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
    settings: Option<(u32, u16, bool, bool)>,
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
}

impl Shared {
    /// Laeuft gerade ein Codecwechsel, den man dem Nutzer erklaeren sollte?
    fn wechsel_laeuft(&self) -> bool {
        self.codec_wechsel.map(|t| t.elapsed() < CODEC_WECHSEL_FRIST).unwrap_or(false)
    }
}

/// Netz- und Decodierschleife. Laeuft in einem eigenen Faden und legt immer nur
/// das neueste Bild ab: lieber eines auslassen als Verzoegerung aufbauen.
fn stream_thread(shared: Arc<Mutex<Shared>>, input: Arc<Mutex<InputLink>>) {
    if let Err(e) = ffmpeg::init() {
        shared.lock().unwrap().error = Some(format!("FFmpeg-Start fehlgeschlagen: {e}"));
        return;
    }

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
                s.error = Some(e);
                s.connected = false;
            }
        }
        {
            let mut s = shared.lock().unwrap();
            s.link = None;
            s.connected = false;
            s.codec_wechsel = None;
            s.codec_idx = None;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

use ffmpeg_next as ffmpeg;

/// Decoder fuer HEVC oder H.264 bauen, mit der Fadenkonfiguration der
/// Sitzung. Wird beim Start und bei jedem Codecwechsel gerufen - der alte
/// Decoder wird dann einfach fallen gelassen.
///
/// Auf Durchsatz trimmen: mehrere Bilder gleichzeitig decodieren. Ohne das
/// laeuft alles auf einem Kern und kostet rund 8 ms je Bild.
/// Bildparallelitaet ist schnell, aber sie haelt Bilder zurueck: der
/// Decoder gibt erst heraus, wenn genug Faeden gefuellt sind. Bei 16 Faeden
/// sind das rund 15 Bilder - bei 100 Bildern je Sekunde ueber 140 ms
/// Verzoegerung, die niemand sieht, weil die reine Rechenzeit klein bleibt.
/// Fuer eine Fernsteuerung ist das der falsche Handel, deshalb ist
/// Scheibenparallelitaet die Voreinstellung.
fn decoder_bauen(h264: bool) -> Result<ffmpeg::decoder::Video, String> {
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

fn run_session(addr: &str, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) -> Result<(), String> {
    // Erst der Handschlag, dann erst Nutzdaten. Vorher geht nichts ueber die
    // Leitung, was jemand mitlesen koennte.
    let mut sock = secure::Secure::connect(addr, &noise::prologue_video())?;
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
    let mut decoder = decoder_bauen(false)?;
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
                        decoder = decoder_bauen(h264)?;
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
                decoder = decoder_bauen(w.is_h264)?;
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
                let mut packet = ffmpeg::Packet::copy(&payload);
                packet.set_pts(Some(seq as i64));
                packet.set_dts(None);
                if decoder.send_packet(&packet).is_err() {
                    continue;
                }
                let mut decoded = ffmpeg::frame::Video::empty();
                while decoder.receive_frame(&mut decoded).is_ok() {
                    // Das Format entscheidet der decodierte Frame selbst, nicht
                    // die Strominfo. Ein unbekanntes Format ergibt ein dunkles
                    // Bild und eine Meldung - nie einen Absturz.
                    let (frame, fehler) = match to_rgb(&decoded) {
                        Ok(f) => (f, None),
                        Err(e) => (dunkles_bild(decoded.width(), decoded.height()), Some(e)),
                    };
                    let ms = t0.elapsed().as_secs_f32() * 1000.0;
                    let mut s = shared.lock().unwrap();
                    if s.frame.is_some() {
                        s.dropped += 1; // das vorige wurde nie gezeigt
                    }
                    s.frame = Some(frame);
                    s.decoded += 1;
                    s.last_decode_ms = ms;
                    if let Some(e) = fehler {
                        s.error = Some(e);
                        bild_fehler = true;
                    } else if bild_fehler {
                        s.error = None;
                        bild_fehler = false;
                    }
                    if nach_wechsel {
                        // Das erste Bild des neuen Codecs ist da; der Hinweis
                        // "wird gewechselt" hat seinen Dienst getan.
                        nach_wechsel = false;
                        s.codec_wechsel = None;
                    }

                    // Verzoegerung zerlegen. Geht nur, wenn der Zeitabgleich
                    // steht und der Stempel zu genau diesem Bild gefunden wird.
                    let passend = decoded.pts().and_then(|p| {
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
                            // genau das soll hier sichtbar werden.
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
                    shared.lock().unwrap().settings = Some((mbit, fps, payload[6] != 0, payload[7] != 0));
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
                if let Some(a) = &sound {
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
            _ => {}
        }
    }
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
/// Farbwert), ZEHN = 10 Bit je Wert. Als Konstanten, damit der Compiler je
/// Format eine eigene, verzweigungsfreie Schleife baut.
///
/// Bei 4:2:0 wird der naechstgelegene Farbwert genommen (Wiederholung) -
/// einfach und schnell. Eine weichere Farbaufwertung ist eine spaetere
/// Verfeinerung; sie aendert am Vergleich 4:4:4 gegen 4:2:0 nichts Wesentliches.
#[inline(always)]
fn zeile_rgb<const SUB: bool, const ZEHN: bool>(out: &mut [u32], yr: &[u8], ur: &[u8], vr: &[u8]) {
    const CR_R: i32 = 103206; // 1.5748
    const CB_G: i32 = 12276;  // 0.1873
    const CR_G: i32 = 30681;  // 0.4681
    const CB_B: i32 = 121609; // 1.8556

    for (x, o) in out.iter_mut().enumerate() {
        let cx = if SUB { x >> 1 } else { x };
        let (y, cb, cr) = if ZEHN {
            // 10 Bit auf 8 Bit: die oberen acht Bit reichen fuer die Anzeige.
            (
                (u16::from_le_bytes([yr[x * 2], yr[x * 2 + 1]]) >> 2) as i32,
                (u16::from_le_bytes([ur[cx * 2], ur[cx * 2 + 1]]) >> 2) as i32 - 128,
                (u16::from_le_bytes([vr[cx * 2], vr[cx * 2 + 1]]) >> 2) as i32 - 128,
            )
        } else {
            (yr[x] as i32, ur[cx] as i32 - 128, vr[cx] as i32 - 128)
        };
        let r = y + ((CR_R * cr) >> 16);
        let g = y - ((CB_G * cb + CR_G * cr) >> 16);
        let b = y + ((CB_B * cb) >> 16);
        *o = (clamp8(r) << 16) | (clamp8(g) << 8) | clamp8(b);
    }
}

/// Decodiertes Bild nach RGB. Das Format kommt aus dem Frame selbst
/// (Pixelformat des Decoders), nicht aus einer Flagge: nach einem
/// Codecwechsel waere jede Flagge fuer ein paar Bilder falsch. Erkannt werden
/// YUV444P10LE, YUV444P, YUV420P10LE und YUV420P; alles andere ist ein Fehler
/// mit Meldung, kein Absturz. Alle Stroeme sind Vollbereich (der Host
/// garantiert das), deshalb keine Bereichsdehnung.
fn to_rgb(src: &ffmpeg::frame::Video) -> Result<Frame, String> {
    use ffmpeg::format::Pixel;
    let (sub, zehn) = match src.format() {
        Pixel::YUV444P => (false, false),
        Pixel::YUV444P10LE => (false, true),
        Pixel::YUV420P => (true, false),
        Pixel::YUV420P10LE => (true, true),
        f => return Err(format!("Unbekanntes Bildformat vom Decoder: {f:?}")),
    };
    let w = src.width() as usize;
    let h = src.height() as usize;
    if w == 0 || h == 0 {
        return Err("Decoder liefert ein leeres Bild".into());
    }
    if src.planes() < 3 {
        return Err("Decoder liefert zu wenige Bildebenen".into());
    }
    // Breite der Farbebenen: bei 4:2:0 die Haelfte, aufgerundet.
    let cw = if sub { (w + 1) / 2 } else { w };
    let ch = if sub { (h + 1) / 2 } else { h };
    let bpp = if zehn { 2 } else { 1 };
    let (yp, up, vp) = (src.data(0), src.data(1), src.data(2));
    let (ys, us, vs) = (src.stride(0), src.stride(1), src.stride(2));
    // Reichen die Ebenen fuer das, was gleich gelesen wird? Sonst waere das
    // Zerlegen unten ein Absturz mitten im Empfangsfaden.
    if yp.len() < (h - 1) * ys + w * bpp
        || up.len() < (ch - 1) * us + cw * bpp
        || vp.len() < (ch - 1) * vs + cw * bpp
    {
        return Err("Bildebenen des Decoders sind zu klein".into());
    }

    let mut pixels = vec![0u32; w * h];
    pixels.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let crow = if sub { row >> 1 } else { row };
        let yr = &yp[row * ys..row * ys + w * bpp];
        let ur = &up[crow * us..crow * us + cw * bpp];
        let vr = &vp[crow * vs..crow * vs + cw * bpp];
        match (sub, zehn) {
            (false, false) => zeile_rgb::<false, false>(out, yr, ur, vr),
            (false, true) => zeile_rgb::<false, true>(out, yr, ur, vr),
            (true, false) => zeile_rgb::<true, false>(out, yr, ur, vr),
            (true, true) => zeile_rgb::<true, true>(out, yr, ur, vr),
        }
    });

    Ok(Frame { width: w as u32, height: h as u32, pixels })
}

/// Flaches dunkles Bild in Fenstergrundfarbe - was gezeigt wird, wenn das
/// Decoderformat nicht verstanden wurde. Besser als ein eingefrorenes altes
/// Bild, das so tut, als waere alles in Ordnung.
fn dunkles_bild(w: u32, h: u32) -> Frame {
    let (w, h) = (w.max(1), h.max(1));
    Frame { width: w, height: h, pixels: vec![ui::BG; (w * h) as usize] }
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

    /// Wunsch an den Host: Bitrate, Bildrate, Spielmodus, feste Bildrate.
    fn settings(&mut self, mbit: u32, fps: u16, gaming: bool, fixed: bool) {
        let mut p = [0u8; 8];
        p[0..4].copy_from_slice(&mbit.to_le_bytes());
        p[4..6].copy_from_slice(&fps.to_le_bytes());
        p[6] = gaming as u8;
        p[7] = fixed as u8;
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
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    shown: u64,
    /// Beim ersten Kontakt mit einem Host steht der Vergleichscode eine Weile
    /// gross im Bild - genau dann kann man ihn noch pruefen.
    banner_until: Option<Instant>,
    /// Gespeicherte Einstellungen. Was hier steht, ueberlebt den Neustart.
    cfg: einstellungen::Einstellungen,
    /// Fuer welchen Host die gespeicherten Werte schon geschickt wurden.
    /// Verhindert, dass wir sie in jedem Bild erneut senden.
    angewandt_fuer: Option<String>,
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
        let context = softbuffer::Context::new(window.clone()).expect("Kontext");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("Flaeche");
        self.context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.draw(),

            WindowEvent::CursorMoved { position, .. } => {
                self.ui.mouse = (position.x as i32, position.y as i32);
                if self.screen == Screen::Start { return; }
                // Steht die Einstellungstafel offen, gehoert die Maus ihr und
                // nicht dem Mac - sonst klickt man dort zweimal gleichzeitig.
                if self.hud_offen || self.last_frame.is_none() { return; }
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
                if self.hud_offen || self.last_frame.is_none() {
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
                                    let mut sh = self.shared.lock().unwrap();
                                    sh.target = None;
                                    drop(sh);
                                    self.input.lock().unwrap().alle_loslassen();
                                    self.screen = Screen::Start;
                                    self.last_frame = None;
                                    self.banner_until = None;
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
        el.set_control_flow(ControlFlow::Poll);

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
                    self.input.lock().unwrap().settings(w.mbit, w.fps, w.gaming, w.fest);
                }
                self.angewandt_fuer = Some(f.clone());
            }
            (None, _) => self.angewandt_fuer = None,
            _ => {}
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl App {
    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface.as_mut()) else {
            return;
        };
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
        self.ui.tick += 1;

        // Neues Bild abholen, sonst das letzte weiterverwenden.
        if let Some(f) = self.shared.lock().unwrap().frame.take() {
            self.last_frame = Some(f);
            self.fps_count += 1;
            self.shown += 1;
        }

        // Die Sekundenrechnung gehoert HIERHER, vor jede Verzweigung: stand
        // sie am Ende, blieb die Bildrate stehen, sobald Menue oder Statistik
        // offen waren - also genau dann, wenn man sie ablesen will.
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
                let g = self.shared.lock().unwrap().clock.map(|l| l.gesamt_ms).unwrap_or(0.0);
                if g > 0.0 {
                    self.lat_hist.push(g);
                    if self.lat_hist.len() > 240 {
                        self.lat_hist.remove(0);
                    }
                }
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
                let Ok(mut buf) = surface.buffer_mut() else { return };
                let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                let act = start_screen(&mut self.ui, &mut c, self.lang, &hosts, &self.addr_input, err.as_deref());
                buf.present().ok();
                match act {
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
            }
            Screen::Session => {
                // Noch kein Bild da: Wartebildschirm zeichnen statt gar nichts.
                if self.last_frame.is_none() {
                    let stand = {
                        let s = self.shared.lock().unwrap();
                        let f = match s.error_key {
                            Some(k) => Some(self.lang.get(k).to_string()),
                            None => s.error.clone(),
                        };
                        (s.connected, f, s.sas.clone(), s.info.is_some())
                    };
                    let Ok(mut buf) = surface.buffer_mut() else { return };
                    let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                    let adresse = self.addr_input.clone();
                    let abbrechen = warte_screen(&mut self.ui, &mut c, self.lang, &adresse, stand);
                    buf.present().ok();
                    self.ui.click = false;
                    if abbrechen {
                        let mut s = self.shared.lock().unwrap();
                        s.target = None;
                        drop(s);
                        self.screen = Screen::Start;
                        self.banner_until = None;
                    }
                    return;
                }
                let Some(frame) = self.last_frame.as_ref() else {
                    self.ui.click = false;
                    return;
                };
                let Ok(mut buf) = surface.buffer_mut() else { return };
                blit(&mut buf, ww, wh, frame, self.pixel_exact);
                // Lage des Hosts ueber dem stehenden Bild, falls er selbst
                // gerade nichts liefern kann.
                if let Some(k) = { self.shared.lock().unwrap().error_key } {
                    let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                    let t = self.lang.get(k);
                    let tw = self.ui.text.width(t, 14, 1);
                    c.fill(ww as i32 / 2 - tw / 2 - 20, 16, tw + 40, 40, ui::BG, 200);
                    self.ui.text.draw_centered(&mut c, ww as i32 / 2, 42, t, 14, ui::AMBER, 1);
                }
                if self.show_overlay {
                    let (stats, secure, lat, soll, hostlast) = {
                        let s = self.shared.lock().unwrap();
                        (
                            (s.last_decode_ms, s.info, s.dropped, s.error.clone(), s.connected),
                            (s.sas.clone(), s.peer_fp.clone()),
                            s.clock,
                            s.settings.map(|x| x.1 as u32).or_else(|| s.info.map(|i| i.fps)),
                            s.hostlast,
                        )
                    };
                    let hist = self.fps_hist.clone();
                    let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                    overlay(&mut self.ui, &mut c, self.lang, self.fps_shown, &hist, stats, secure,
                            lat, self.cfg.stats, self.cfg.nerd, soll, hostlast);
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
                        let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                        let bw = 520.min(ww as i32 - 40);
                        let bx = (ww as i32 - bw) / 2;
                        let t = self.lang.get(strings::Key::FirstContact);
                        // Der Satz ist je nach Sprache verschieden lang, also
                        // erst messen, dann die Tafel darauf zuschneiden.
                        let zeilen = umbruch(&mut self.ui, t, bw - 48, 13);
                        let bh = 62 + zeilen.len() as i32 * 18;
                        let by = wh as i32 * 3 / 4 - bh / 2;
                        c.panel(bx, by, bw, bh, ui::AMBER);
                        let mut ty = by + 28;
                        for z in &zeilen {
                            self.ui.text.draw_centered(&mut c, bx + bw / 2, ty, z, 13, ui::TEXT, 1);
                            ty += 18;
                        }
                        self.ui.text.draw_centered(&mut c, bx + bw / 2, ty + 24, &sas, 30, ui::AMBER, 6);
                    }
                }
                // Nerd-Modus. Liegt ueber allem, deshalb zuletzt gezeichnet.
                if self.hud_offen {
                    let (lat, info, stell, secure, codecs, codec_idx, wechsel) = {
                        let sh = self.shared.lock().unwrap();
                        (
                            sh.clock, sh.info, sh.settings, (sh.sas.clone(), sh.peer_fp.clone()),
                            sh.codecs.clone(), sh.codec_idx, sh.wechsel_laeuft(),
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
                    };
                    let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                    let a = hud(
                        &mut self.ui, &mut c, self.lang, ww as i32, wh as i32, reiter,
                        lat, &lhist, fps_jetzt, &hist, info, stell, secure, &adresse, gespeichert,
                        &stand,
                    );
                    buf.present().ok();
                    self.ui.click = false;
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
                        HudAktion::Trennen => {
                            self.shared.lock().unwrap().target = None;
                            self.input.lock().unwrap().alle_loslassen();
                            self.hud_offen = false;
                            self.screen = Screen::Start;
                            self.last_frame = None;
                            self.banner_until = None;
                        }
                        HudAktion::Stellen(m, f, g, fx) => {
                            self.input.lock().unwrap().settings(m, f, g, fx);
                            if let Some(fp) = &self.angewandt_fuer {
                                let fp = fp.clone();
                                self.cfg.host_merken(
                                    &fp,
                                    einstellungen::HostWerte { mbit: m, fps: f, gaming: g, fest: fx },
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
                        HudAktion::Nichts => {}
                    }
                    return;
                }

                // Codecwechsel unterwegs: der Host baut den Encoder um, das
                // Bild steht ein paar hundert Millisekunden. Ohne Hinweis
                // saehe das nach einem Haenger aus.
                if self.shared.lock().unwrap().wechsel_laeuft() {
                    let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                    let t = self.lang.get(strings::Key::CodecSwitching);
                    let bw = (self.ui.text.width(t, 12, 2) + 60).max(220).min(ww as i32 - 40);
                    let bh = 44i32;
                    let bx = ww as i32 / 2 - bw / 2;
                    let by = wh as i32 - 110;
                    c.panel(bx, by, bw, bh, ui::AMBER);
                    self.ui.text.draw_centered(&mut c, bx + bw / 2, by + 27, t, 12, ui::AMBER, 2);
                }

                // Rueckmeldung beim Halten von ESC. Erst ab einer Weile, damit
                // ein normaler Tipper nichts aufblitzen laesst - und ueberhaupt,
                // weil fuenf Sekunden ohne jede Anzeige von "abgestuerzt" nicht
                // zu unterscheiden sind.
                if let Some(t) = self.esc_seit {
                    let v = t.elapsed();
                    if v >= Duration::from_millis(400) && !self.esc_verbraucht {
                        let anteil = (v.as_secs_f32() / ESC_HALTEDAUER.as_secs_f32()).min(1.0);
                        let mut c = ui::Canvas { buf: &mut buf, w: ww as usize, h: wh as usize };
                        let (bw, bh) = (340i32, 58i32);
                        let bx = ww as i32 / 2 - bw / 2;
                        let by = wh as i32 - 110;
                        c.panel(bx, by, bw, bh, ui::CYAN);
                        self.ui.text.draw_centered(
                            &mut c, bx + bw / 2, by + 24,
                            self.lang.get(strings::Key::HoldEscHint), 12, ui::TEXT, 2,
                        );
                        c.rect(bx + 20, by + 38, bw - 40, 5, ui::DIM, 120);
                        let breit = ((bw - 40) as f32 * anteil) as i32;
                        c.rect(bx + 20, by + 38, breit.max(1), 5, ui::CYAN, 240);
                    }
                }

                buf.present().ok();
            }
        }
        self.ui.click = false;

    }
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

fn blit(buf: &mut [u32], ww: u32, wh: u32, frame: &Frame, pixel_exact: bool) {
    if ww == frame.width && wh == frame.height {
        buf.copy_from_slice(&frame.pixels);
        return;
    }
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    if fw == 0 || fh == 0 {
        return;
    }

    if pixel_exact {
        let ox = (ww as usize).saturating_sub(fw) / 2;
        let oy = (wh as usize).saturating_sub(fh) / 2;
        buf.fill(0x05070d);
        let cols = fw.min(ww as usize);
        let rows = fh.min(wh as usize);
        buf.par_chunks_mut(ww as usize).enumerate().for_each(|(y, row)| {
            if y >= oy && y < oy + rows {
                let sy = y - oy;
                row[ox..ox + cols].copy_from_slice(&frame.pixels[sy * fw..sy * fw + cols]);
            }
        });
        return;
    }

    // Einpassen statt verzerren: das Bild behaelt sein Seitenverhaeltnis und
    // bekommt Balken, wo das Fenster nicht passt. Vorher wurde schlicht auf
    // die Fenstergroesse gezogen - in einem nicht 16:9 grossen Fenster war
    // der Mac-Bildschirm dadurch sichtbar verzerrt.
    let (winw, winh) = (ww as usize, wh as usize);
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
    s: f32,
) -> i32 {
    use strings::Key::*;
    let p = |v: i32| -> i32 { (v as f32 * s).round() as i32 };
    let sz = |v: u32| -> u32 { (v as f32 * s).round() as u32 };

    let ein_bild = soll_fps.map(|f| 1000.0 / f.max(1) as f32);
    let ende = skalenende((l.gesamt_ms * 1.35).max(ein_bild.unwrap_or(0.0) * 1.5).max(10.0));
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
    ];
    let zu_langsam = ein_bild.map(|b| l.gesamt_ms > b).unwrap_or(false);

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
        let mitte = (ix + (l.gesamt_ms * je_ms) as i32).min(spur_ende);
        let halb = ((l.umlauf_ms / 2.0) * je_ms) as i32;
        let von = (mitte - halb).max(ix);
        let bis = (mitte + halb).min(spur_ende);
        if bis > von { c.rect(von, spur_y, bis - von, spur_h, ui::TEXT, 45); }
        c.vline(mitte, spur_y - p(4), spur_h + p(8), ui::TEXT, 255);
    }
    if let Some(b) = ein_bild {
        let bx = (ix + (b * je_ms) as i32).min(spur_ende);
        c.glow_vline(bx, spur_y - p(5), spur_h + p(10), ui::MAGENTA);
        u.text.draw(c, (bx + p(6)).min(spur_ende - p(90)), spur_y + spur_h + p(26),
                    &format!("{} · {b:.1} ms", lang.get(OneFrame)), sz(10), ui::DIM, p(1));
    }
    let schritt = if je_ms * 2.0 >= 14.0 { 2.0 } else { 10.0 };
    let mut t = 0.0;
    while t <= ende {
        let tx = (ix + (t * je_ms) as i32).min(spur_ende);
        c.vline(tx, spur_y + spur_h, p(4), ui::DIM, 120);
        u.text.draw_centered(c, tx, spur_y + spur_h + p(16), &format!("{t:.0}"), sz(9), ui::DIM, 0);
        t += schritt;
    }

    let spalte = iw / 4;
    let ly = spur_y + spur_h + p(46);
    for (i, (name, wert, farbe)) in teile.iter().enumerate() {
        let lx = ix + i as i32 * spalte;
        c.rect(lx, ly - p(9), p(9), p(9), *farbe, 230);
        let gross = zu_langsam && *wert > l.gesamt_ms / 2.0;
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
                rows.push((
                    lang.get(Latency),
                    format!("{:.1} ±{:.1} ms", l.gesamt_ms, l.umlauf_ms / 2.0),
                    if l.gesamt_ms < 40.0 { ui::CYAN } else { ui::AMBER },
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
    if wahl.verworfen {
        rows.push((lang.get(Dropped), format!("{dropped}"), ui::TEXT));
    }
    if wahl.code {
        if let Some(sas) = &sas {
            rows.push((lang.get(SecuredWith), sas.clone(), ui::CYAN));
        }
    }
    // Im Nerd-Modus dazu, was der Mac gerade zu tun hat.
    if nerd {
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
            kette(u, c, lang, x + 16, ty + 6, w - 32, l, soll_fps, 1.0);
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
            let mut c = ui::Canvas { buf: &mut buf, w, h };
            let probe = Latenz {
                versatz_us: 1, umlauf_ms: 0.4, encoder_ms: 4.2, leitung_ms: 2.6,
                decoder_ms: 2.1, gesamt_ms: 17.3, bilder: 900,
            };
            let nerd = view == "nerd";
            let hl = HostLast {
                cpu: 30.0, cpu_eigen: 28.0, gpu: Some(0.0), druck: 1,
                ram_benutzt_mb: 9114, ram_gesamt_mb: 16384, eigen_mb: 310,
                encoder_ms: 11.4, host_fps: 118.0,
            };
            overlay(&mut u, &mut c, lang, 98.0, &hist, (2.1, info, 3, None, true), (sas.clone(), fp.clone()),
                    Some(probe), einstellungen::StatWahl::default(), nerd, Some(120),
                    if nerd { Some(hl) } else { None });
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
            decoder_ms: 8.6, gesamt_ms: 27.4, bilder: 900,
        };
        let lh: Vec<f32> = (0..200).map(|i| 24.0 + 5.0 * ((i as f32) / 11.0).sin()).collect();
        let fh: Vec<f32> = (0..200).map(|i| 104.0 + 9.0 * ((i as f32) / 7.0).cos()).collect();
        let info = Some(StreamInfo { width: 1920, height: 1080, fps: 120, codec: 1, chroma444: true, ten_bit: true });
        let mut c = ui::Canvas { buf: &mut buf, w, h };
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
        };
        let reiter = match view { "hud2" => 1u8, "hud3" => 2, _ => 0 };
        let _ = hud(
            &mut u, &mut c, lang, w as i32, h as i32, reiter,
            Some(l), &lh, 104.0, &fh, info, Some((50, 120, false, true)),
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
        let mut c = ui::Canvas { buf: &mut buf, w, h };
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

    let shared = Arc::new(Mutex::new(Shared::default()));
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
        let mut wish: Option<(u32, u16, bool, bool)> = None;
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
        let start = Instant::now();
        let mut last = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let s = shared.lock().unwrap();
            let n = s.decoded;
            let lat = match s.clock {
                Some(l) if l.gesamt_ms > 0.0 => format!(
                    "Verzoegerung {:.1} ms (+/-{:.1}) = Encoder {:.1} + Leitung {:.1} + Decoder {:.1}",
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
                "{:.0}s | decodiert {} ({:.1}/s) | Codec {} | {}{} | Fehler {:?}",
                start.elapsed().as_secs_f32(), n, (n - last) as f32 / 3.0, codec, lat, hl, s.error
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
                    l.settings(w.0, w.1, w.2, w.3);
                    println!(
                        "Einstellung gewuenscht: {} Mbit/s, {} fps, Gaming {}, feste Bildrate {}",
                        w.0, w.1, w.2, w.3
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
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("C:\\qc\\client.log") {
                writeln!(f, "{line}").ok();
            }
            s.frame = None; // im Pruefmodus nichts anzeigen, Speicher freigeben
            last = n;
        }
    }

    // Gespeicherte Einstellungen. Sie bestimmen unter anderem, ob wir im
    // Vollbild starten - das ist die Voreinstellung.
    let cfg = einstellungen::Einstellungen::laden();
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
        surface: None,
        context: None,
        shown: 0,
        banner_until: None,
        angewandt_fuer: None,
        esc_seit: None,
        esc_verbraucht: false,
        esc_mods: 0,
        esc_up_faellig: None,
        hud_offen: false,
        hud_reiter: 0,
        lat_hist: Vec::new(),
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
    Stellen(u32, u16, bool, bool),
    Schalter(u8),
    /// Wunsch nach diesem Kandidaten der Koennensliste.
    Codec(u8),
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
    stell: Option<(u32, u16, bool, bool)>,
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
            (lang.get(Latency), format!("{:.1}", l.gesamt_ms), format!("±{:.1} ms", l.umlauf_ms / 2.0),
             if l.gesamt_ms > 0.0 && l.gesamt_ms < 40.0 { ui::CYAN } else { ui::AMBER }, lat_hist, lo, hi)
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
            let (mbit, fps_soll, gaming, fest) = stell.unwrap_or((0, 0, false, false));
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
                aktion = HudAktion::Stellen((mbit as i32 + d * schritt).clamp(2, 500) as u32, fps_soll, gaming, fest);
            }
            let d = stellen(u, c, ix + iw / 2, cy, lang.get(MaxFps), format!("{fps_soll}"));
            if d != 0 {
                aktion = HudAktion::Stellen(mbit, (fps_soll as i32 + d * 10).clamp(10, 240) as u16, gaming, fest);
            }
            if u.toggle(c, ui::Rect { x: ix, y: cy + p(70), w: iw / 2 - p(30), h: p(28) }, lang.get(GamingMode), gaming) {
                aktion = HudAktion::Stellen(mbit, fps_soll, !gaming, fest);
            }
            if u.toggle(c, ui::Rect { x: ix + iw / 2, y: cy + p(70), w: iw / 2 - p(30), h: p(28) }, lang.get(FixedRate), fest) {
                aktion = HudAktion::Stellen(mbit, fps_soll, gaming, !fest);
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
