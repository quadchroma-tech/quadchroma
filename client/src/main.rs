// QuadChroma Client, erste Stufe: Strom annehmen, decodieren, anzeigen.
//
//   quadchroma <host>[:port]
//
// Das Bild kommt als HEVC 4:4:4 (8 oder 10 Bit, voller Wertebereich) an. Die
// Umwandlung nach RGB und das Einpassen ins Fenster laufen auf der Karte -
// Direct3D 11 unter Windows (anzeige.rs), Metal auf dem Mac (anzeige_mac.rs) -
// oder ohne sie auf der CPU ueber mehrere Kerne (softbuffer); die Umrechnung
// ist auf beiden Wegen bitgleich.

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

/// Dateien ueber die Zwischenablage: Protokoll, Sender, Empfaenger und
/// Ablageverzeichnis, gemeinsam fuer Client und Windows-Host-Rolle.
mod bildschirm;
mod dateien;
mod discovery;
mod einstellungen;
/// Einzelinstanz: ein zweiter Start reicht seine Adresse an den ersten weiter.
mod einzel;
/// Das Programmsymbol, im Programm gezeichnet, und seine .ico-Datei.
mod logo;
mod noise;
mod protokoll_konst;
mod secure;
mod sps;
/// Der Decoder des Mac-Clients: VideoToolbox direkt (Annex B nach
/// Formatbeschreibung und Laengenpraefix, VTDecompressionSession). Der reine
/// Teil (Zerlegen, Parametersaetze, Formatwahl) wird ueberall geprueft.
mod vt_decoder;
mod strings;
mod strings_asia;
mod strings_balt;
mod strings_east;
mod strings_more;
mod strings_north;
mod strings_west;
mod ui;
/// Desktop-Verknuepfung je Host (anlegen nur unter Windows).
mod verknuepfung;
/// Zugang (Pairing v1): Geraete-ID, Passwortbeweis, Zugangsnachrichten,
/// Drossel, Geraeteliste, Passwortdatei und hosts.txt - Client und
/// Windows-Host-Rolle.
mod zugang;
/// Zugangsphase des Clients: Antwort des Hosts nach dem Handschlag,
/// Passwortbeweis bzw. "Zulassen", Pinnen in hosts.txt erst nach der Annahme.
mod zugangsphase;
/// Schliessen legt die App ab: Symbol im Infobereich (Windows, tray_win.rs)
/// bzw. in der Menueleiste (macOS, das Symbol der Host-Engine, host_mac.rs);
/// gemeinsame Logik in tray.rs.
mod tray;
/// Das Menue am Symbol der einen App (Client und Host-Rolle, Windows; auf
/// dem Mac baut host/menue.m dasselbe Menue).
mod symbolmenue;
/// Die Host-Rolle der einen App, fuer main.rs auf jeder Plattform gleich.
mod freigabe;
/// "Ruhezustand verhindern, solange QuadChroma laeuft".
mod ruhezustand;
#[cfg(windows)]
mod tray_win;
/// Die Host-Engine des Mac (host/, von build.rs als libqchost.a
/// hineingebaut): die C-Schnittstelle des Dienstes aus host/dienst.h, das
/// eine Symbol in der Menueleiste und die Host-Rolle der einen App.
#[cfg(target_os = "macos")]
mod host_mac;

/// Ton und Zwischenablage: je Plattform eine Datei mit derselben
/// Schnittstelle; der Ringpuffer des Tons ist geteilt (audio_ring.rs). Auf
/// dem Mac laufen die Module unter denselben Namen `audio` und `clipboard`,
/// damit die Aufrufstellen ohne Weiche auskommen.
#[cfg(any(windows, target_os = "macos"))]
mod audio_ring;
/// Entprellen und Entdoppeln der Ablage-Meldungen, fuer beide Waechter.
#[cfg(any(windows, target_os = "macos"))]
mod ablage_ruhe;
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
/// Anzeige ueber die Grafikkarte: Direct3D 11 unter Windows (anzeige.rs),
/// Metal auf dem Mac (anzeige_mac.rs) - dieselbe Schnittstelle unter
/// demselben Namen, wie bei Ton und Zwischenablage.
#[cfg(windows)]
mod anzeige;
#[cfg(target_os = "macos")]
mod anzeige_mac;
#[cfg(target_os = "macos")]
use anzeige_mac as anzeige;
/// Gemeinsame Teile der Goldbildtests beider Anzeigen (--anzeigetest).
#[cfg(any(windows, target_os = "macos"))]
mod anzeigeprobe;
/// Windows als Host: die Host-Rolle in derselben Programmdatei - in der
/// einen App als eigener Faden (freigabe.rs), allein ohne Fenster mit
/// --nur-host, --list, --messen.
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

/// Protokoll des Clients: die Decoderzeilen und die eigenen Meldungen der
/// Decoder-Bibliothek - unter Windows FFmpeg, auf dem Mac VideoToolbox (das
/// nichts von selbst meldet; was der Decoder dort zu sagen hat, reicht
/// vt_decoder.rs ueber `bibliothek_sagt` ein).
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
///
/// Zwei Rollen, zwei Reihen: der Rueckruf von FFmpeg gilt fuer den ganzen
/// Prozess, Client und Host-Rolle koennen sich aber einen Prozess teilen.
/// Jede Zeile gehoert deshalb einer Herkunft - der des Fadens, der sie sagt
/// (`herkunft_setzen`; ungesetzt die des Prozesses, `standard_setzen`:
/// der Client - auch in der einen App -, im reinen Host-Prozess
/// --nur-host/--list/--messen der Host). Die
/// Faeden der Host-Rolle, die FFmpeg benutzen (Dienst, Aufnahme mit
/// Encoder, Messung), setzen Herkunft::Host; ihre Zeilen, Warnungen und
/// Fehler landen in der Reihe des Hosts, die der Host abholt und in
/// host-protokoll.txt schreibt - nie in protokoll.txt des Clients und nie in
/// seinen Rueckfallgruenden. Faeden ohne eigene Herkunft (etwa die, die
/// FFmpeg selbst fuer einen Software-Decoder anlegt) zaehlen zur Herkunft
/// des Prozesses; die Encoder der Host-Rolle legen keine an (nvenc,
/// h264_mf). Geteilte Teile, die fuer beide Rollen sprechen (der
/// Ablagewaechter), nennen die Herkunft ausdruecklich (`zeile_als`).
mod protokoll {
    #[cfg(windows)]
    use ffmpeg_next as ffmpeg;
    use std::cell::Cell;
    use std::os::raw::c_int;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::sync::Mutex;

    /// Wem eine Zeile gehoert - und wer in der Zwischenablage etwas ablegt
    /// oder abnimmt (clipboard.rs, ablage_ruhe.rs): der Client oder die
    /// Host-Rolle.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Herkunft {
        Client,
        Host,
    }

    impl Herkunft {
        /// Platz der Herkunft in Tabellen je Rolle.
        pub const fn stelle(self) -> usize {
            match self {
                Herkunft::Client => 0,
                Herkunft::Host => 1,
            }
        }
    }

    thread_local! {
        /// Herkunft dieses Fadens (herkunft_setzen); None: die des Prozesses.
        static FADEN: Cell<Option<Herkunft>> = const { Cell::new(None) };
    }

    /// Herkunft der Faeden ohne eigene: im reinen Host-Prozess der Host.
    static PROZESS_HOST: AtomicBool = AtomicBool::new(false);

    fn prozess() -> Herkunft {
        if PROZESS_HOST.load(Ordering::Relaxed) {
            Herkunft::Host
        } else {
            Herkunft::Client
        }
    }

    /// Herkunft der Faeden ohne eigene festlegen - nur ein Prozess, der
    /// allein die Host-Rolle spielt (--nur-host, --list, --messen), setzt den
    /// Host: dann gehen auch die Zeilen des Ablagewaechters, der Netzfaeden
    /// und der Faeden, die FFmpeg selbst anlegt, in die Reihe des Hosts.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn standard_setzen(h: Herkunft) {
        PROZESS_HOST.store(h == Herkunft::Host, Ordering::Relaxed);
    }

    /// Herkunft des aufrufenden Fadens.
    pub fn herkunft() -> Herkunft {
        FADEN.with(|f| f.get()).unwrap_or_else(prozess)
    }

    /// Ab jetzt gehoert, was dieser Faden sagt und abholt, zu `h`. Die Faeden
    /// der Host-Rolle setzen Herkunft::Host, bevor sie FFmpeg anfassen.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub fn herkunft_setzen(h: Herkunft) {
        FADEN.with(|f| f.set(Some(h)));
    }

    /// Stufen wie FFmpegs AV_LOG_*: Warnung und ausfuehrlich - unter
    /// Windows die Zahlen aus FFmpeg selbst, auf dem Mac dieselben.
    #[cfg(windows)]
    pub const WARNUNG: c_int = ffmpeg::sys::AV_LOG_WARNING;
    #[cfg(windows)]
    const AUSFUEHRLICH: c_int = ffmpeg::sys::AV_LOG_VERBOSE;
    #[cfg(not(windows))]
    pub const WARNUNG: c_int = 24;
    #[cfg(not(windows))]
    const AUSFUEHRLICH: c_int = 40;

    /// Wie die Zeilen der Bibliothek beginnen.
    #[cfg(windows)]
    const BIBLIOTHEK: &str = "FFmpeg: ";
    #[cfg(not(windows))]
    const BIBLIOTHEK: &str = "VideoToolbox: ";

    /// Die Reihe einer Herkunft: Stufe, Zeilen, letzte Warnungen, die
    /// angefangene Zeile.
    struct Reihe {
        stufe: AtomicI32,
        /// Zeilen seit dem letzten Abholen. Holt niemand ab (Fenstermodus),
        /// bleibt die Reihe bei 512 stehen - beim Client hat dann die Datei
        /// alles; der Host holt im Takt ab.
        zeilen: Mutex<Vec<String>>,
        /// Die letzten Warnungen und Fehler der Bibliothek, fuer
        /// Rueckfallgruende.
        fehler: Mutex<Vec<String>>,
        /// Eine FFmpeg-Meldung kann in Stuecken kommen; der Zeilenumbruch
        /// schliesst sie ab. Der Rest der angefangenen Zeile und FFmpegs
        /// Praefix-Zustand ("naechstes Stueck bekommt einen Absender") liegen
        /// unter EINER Sperre, die schon vor dem Formatieren genommen wird -
        /// FFmpeg ruft den Rueckruf aus jedem Faden, der etwas zu sagen hat,
        /// auch aus den Arbeitsfaeden eines Decoders, und genau so haelt es
        /// FFmpegs eigener Rueckruf. Je Herkunft eine, damit die Stuecke der
        /// einen Rolle nie an einer Zeile der anderen kleben.
        #[cfg_attr(not(windows), allow(dead_code))]
        offen: Mutex<(String, c_int)>,
    }

    impl Reihe {
        const fn neu() -> Reihe {
            Reihe {
                stufe: AtomicI32::new(WARNUNG),
                zeilen: Mutex::new(Vec::new()),
                fehler: Mutex::new(Vec::new()),
                offen: Mutex::new((String::new(), 1)),
            }
        }
    }

    static REIHEN: [Reihe; 2] = [Reihe::neu(), Reihe::neu()];

    fn reihe(h: Herkunft) -> &'static Reihe {
        &REIHEN[h.stelle()]
    }

    /// Die Datei, beim ersten Schreiben geoeffnet und dabei geleert.
    static DATEI: Mutex<Option<std::fs::File>> = Mutex::new(None);

    fn datei_pfad() -> Option<std::path::PathBuf> {
        crate::einstellungen::datei_pfad("protokoll.txt")
    }

    /// Nur in die Datei des Clients - fuer die Taktzeilen des Pruefmodus,
    /// die auf der Konsole ohnehin stehen.
    pub fn nur_datei(text: &str) {
        let Ok(mut d) = DATEI.lock() else { return };
        if d.is_none() {
            *d = datei_pfad().and_then(|p| std::fs::File::create(p).ok());
        }
        if let Some(f) = d.as_mut() {
            let _ = writeln!(f, "{text}");
        }
    }

    /// Eine Zeile ins Protokoll der Herkunft dieses Fadens.
    pub fn zeile(text: String) {
        zeile_als(herkunft(), text);
    }

    /// Eine Zeile ins Protokoll von `h`: beim Client Datei und Reihe, beim
    /// Host nur seine Reihe (er schreibt sie beim Abholen in seine Datei).
    pub fn zeile_als(h: Herkunft, text: String) {
        if h == Herkunft::Client {
            nur_datei(&text);
        }
        if let Ok(mut z) = reihe(h).zeilen.lock() {
            if z.len() < 512 {
                z.push(text);
            }
        }
    }

    #[cfg(windows)]
    unsafe extern "C" fn rueckruf(
        ptr: *mut std::os::raw::c_void,
        stufe: c_int,
        fmt: *const std::os::raw::c_char,
        vl: ffmpeg::sys::va_list,
    ) {
        // Der Faden, der spricht, bestimmt die Reihe. try_with: auch in
        // einem Faden, der gerade endet, darf FFmpeg noch etwas sagen.
        let h = FADEN.try_with(|f| f.get()).ok().flatten().unwrap_or_else(prozess);
        let r = reihe(h);
        if stufe > r.stufe.load(Ordering::Relaxed) {
            return;
        }
        let mut puffer = [0 as std::os::raw::c_char; 1024];
        let text = {
            let Ok(mut offen) = r.offen.lock() else { return };
            let (rest, praefix) = &mut *offen;
            let n = ffmpeg::sys::av_log_format_line2(
                ptr, stufe, fmt, vl, puffer.as_mut_ptr(), puffer.len() as c_int, praefix,
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
            let t = format!("{BIBLIOTHEK}{}", rest.trim_end());
            rest.clear();
            *praefix = 1;
            t
        };
        melden(h, stufe, text);
    }

    /// Eine fertige Zeile der Bibliothek in die Reihe von `h`; Warnungen und
    /// Fehler ausserdem in die Liste fuer Rueckfallgruende.
    fn melden(h: Herkunft, stufe: c_int, text: String) {
        if stufe <= WARNUNG {
            if let Ok(mut f) = reihe(h).fehler.lock() {
                if f.len() >= 6 {
                    f.remove(0);
                }
                f.push(text.clone());
            }
        }
        zeile_als(h, text);
    }

    /// Was der Decoder auf dem Mac (VideoToolbox, das selbst nichts meldet)
    /// zu sagen hat, auf demselben Weg wie FFmpegs Meldungen unter Windows:
    /// in die Reihe der Herkunft dieses Fadens, soweit ihre Stufe es
    /// erlaubt, Warnungen und Fehler auch in die Rueckfallgruende.
    #[cfg(not(windows))]
    pub fn bibliothek_sagt(stufe: c_int, text: &str) {
        let h = herkunft();
        if stufe > reihe(h).stufe.load(Ordering::Relaxed) {
            return;
        }
        melden(h, stufe, format!("{BIBLIOTHEK}{text}"));
    }

    /// Einsammeln einschalten, fuer die Herkunft dieses Fadens. Ausfuehrlich
    /// heisst bis AV_LOG_VERBOSE - da sagt cuvid, welche Formate er gewaehlt
    /// hat und was die Karte kann. FFmpeg selbst laesst durch, was die
    /// gespraechigere der beiden Reihen will; jede Reihe nimmt nur, was ihre
    /// eigene Stufe erlaubt.
    pub fn einschalten(ausfuehrlich: bool) {
        let stufe = if ausfuehrlich { AUSFUEHRLICH } else { WARNUNG };
        reihe(herkunft()).stufe.store(stufe, Ordering::Relaxed);
        #[cfg(windows)]
        {
            let hoechste = REIHEN.iter().map(|r| r.stufe.load(Ordering::Relaxed)).max().unwrap_or(stufe);
            unsafe {
                ffmpeg::sys::av_log_set_level(hoechste);
                ffmpeg::sys::av_log_set_callback(Some(rueckruf));
            }
        }
    }

    /// Alle Zeilen der eigenen Herkunft seit dem letzten Abholen.
    pub fn abholen() -> Vec<String> {
        reihe(herkunft()).zeilen.lock().map(|mut z| std::mem::take(&mut *z)).unwrap_or_default()
    }

    /// Angesammelte Warnungen und Fehler der eigenen Herkunft verwerfen - vor
    /// einem Versuch, damit nur das im Grund landet, was dieser Versuch
    /// selbst gesagt hat.
    pub fn fehler_verwerfen() {
        if let Ok(mut f) = reihe(herkunft()).fehler.lock() {
            f.clear();
        }
    }

    /// Die letzten Warnungen und Fehler der eigenen Herkunft, ohne den
    /// Absender "[hevc_cuvid @ 000001d4...] " - der steht im Grund ohnehin,
    /// und die Adresse sagt niemandem etwas. Gleiche Zeilen in Folge nur
    /// einmal.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub fn fehler_abholen() -> Vec<String> {
        let Ok(mut f) = reihe(herkunft()).fehler.lock() else { return Vec::new() };
        let mut aus: Vec<String> = Vec::new();
        for z in f.drain(..) {
            let z = z.trim_start_matches(BIBLIOTHEK);
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

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Eine Meldung ueber FFmpegs eigenen Weg (av_log), wie sie ein Codec
        /// macht - im aufrufenden Faden. Auf dem Mac der Weg, auf dem der
        /// Decoder (VideoToolbox) meldet.
        #[cfg(windows)]
        fn ffmpeg_sagt(stufe: c_int, text: &str) {
            let t = std::ffi::CString::new(format!("{text}\n")).unwrap();
            unsafe { ffmpeg::sys::av_log(std::ptr::null_mut(), stufe, c"%s".as_ptr(), t.as_ptr()) };
        }
        #[cfg(not(windows))]
        fn ffmpeg_sagt(stufe: c_int, text: &str) {
            bibliothek_sagt(stufe, text);
        }

        /// AV_LOG_ERROR.
        const FEHLER: c_int = 16;

        fn enthaelt(v: &[String], t: &str) -> bool {
            v.iter().any(|z| z.contains(t))
        }

        /// Beide Tests stellen Stufen um - nacheinander, sonst stellte der
        /// eine dem anderen die Stufe des Hosts mitten im Test zurueck.
        static STUFEN: Mutex<()> = Mutex::new(());

        /// Zwei Rollen in einem Prozess: was ein Faden der Host-Rolle sagt
        /// (eigene Zeilen wie FFmpegs Meldungen), landet in der Reihe des
        /// Hosts und bei seinen Rueckfallgruenden - nie beim Client, und
        /// umgekehrt; zeile_als nennt die Herkunft ausdruecklich. Die Reihen
        /// teilt sich der ganze Testlauf; deshalb Zeilen mit eigener
        /// Kennung, und gefragt wird nur nach ihnen.
        #[test]
        fn zwei_reihen_je_herkunft() {
            let _stufen = STUFEN.lock().unwrap_or_else(|e| e.into_inner());
            let k = format!("{}-{:?}", std::process::id(), std::time::Instant::now());
            einschalten(false);

            let k2 = k.clone();
            let (host_reihe, host_fehler, nachher) = std::thread::spawn(move || {
                herkunft_setzen(Herkunft::Host);
                einschalten(false);
                zeile(format!("Reihentest Host {k2}"));
                ffmpeg_sagt(FEHLER, &format!("Reihentest FFmpeg Host {k2}"));
                let fehler = fehler_abholen();
                let reihe = abholen();
                // Einmal abgeholt ist abgeholt.
                (reihe, fehler, abholen())
            })
            .join()
            .unwrap();

            let k2 = k.clone();
            let (client_reihe, client_fehler) = std::thread::spawn(move || {
                zeile(format!("Reihentest Client {k2}"));
                ffmpeg_sagt(FEHLER, &format!("Reihentest FFmpeg Client {k2}"));
                zeile_als(Herkunft::Host, format!("Reihentest ausdruecklich Host {k2}"));
                (abholen(), fehler_abholen())
            })
            .join()
            .unwrap();
            let host_spaeter = std::thread::spawn(|| {
                herkunft_setzen(Herkunft::Host);
                abholen()
            })
            .join()
            .unwrap();

            assert!(enthaelt(&host_reihe, &format!("Reihentest Host {k}")), "{host_reihe:?}");
            assert!(enthaelt(&host_reihe, &format!("{BIBLIOTHEK}Reihentest FFmpeg Host {k}")), "{host_reihe:?}");
            assert!(enthaelt(&host_fehler, &format!("Reihentest FFmpeg Host {k}")), "{host_fehler:?}");
            assert!(!enthaelt(&host_reihe, &format!("Client {k}")), "{host_reihe:?}");
            assert!(!enthaelt(&nachher, &k), "zweimal abgeholt: {nachher:?}");
            assert!(enthaelt(&client_reihe, &format!("Reihentest Client {k}")), "{client_reihe:?}");
            assert!(enthaelt(&client_reihe, &format!("{BIBLIOTHEK}Reihentest FFmpeg Client {k}")), "{client_reihe:?}");
            assert!(enthaelt(&client_fehler, &format!("Reihentest FFmpeg Client {k}")), "{client_fehler:?}");
            assert!(!enthaelt(&client_reihe, &format!("Host {k}")), "{client_reihe:?}");
            assert!(!enthaelt(&client_fehler, &format!("Host {k}")), "{client_fehler:?}");
            assert!(enthaelt(&host_spaeter, &format!("Reihentest ausdruecklich Host {k}")), "{host_spaeter:?}");
        }

        /// Die Stufe gilt je Reihe: ausfuehrlich nur beim Host - eine
        /// VERBOSE-Meldung eines Client-Fadens bleibt draussen, die des Hosts
        /// kommt an. Danach stehen beide wieder auf Warnungen.
        #[test]
        fn stufe_je_herkunft() {
            let _stufen = STUFEN.lock().unwrap_or_else(|e| e.into_inner());
            let k = format!("{}-{:?}", std::process::id(), std::time::Instant::now());
            let (h, c) = std::thread::spawn(move || {
                herkunft_setzen(Herkunft::Host);
                einschalten(true);
                ffmpeg_sagt(AUSFUEHRLICH, &format!("Stufentest Host {k}"));
                let k2 = k.clone();
                let c = std::thread::spawn(move || {
                    ffmpeg_sagt(AUSFUEHRLICH, &format!("Stufentest Client {k2}"));
                    abholen()
                })
                .join()
                .unwrap();
                let h = abholen();
                einschalten(false);
                (h.into_iter().filter(|z| z.contains(&k)).collect::<Vec<_>>(), c.into_iter().filter(|z| z.contains(&k)).collect::<Vec<_>>())
            })
            .join()
            .unwrap();
            assert_eq!(h.len(), 1, "ausfuehrliche Meldung des Hosts fehlt: {h:?}");
            assert!(c.is_empty(), "ausfuehrliche Meldung beim Client trotz Stufe Warnung: {c:?}");
        }
    }
}

/// Urheber. Wird in der Fusszeile des Startbildschirms angezeigt.
const COPYRIGHT: &str = "© 2026 Robert Brandt";
/// Projektseite. Steht anklickbar neben der Urheberzeile. Solange es
/// quadchroma.tech noch nicht gibt, ist das die GitHub-Seite des Projekts.
const WEBSITE: &str = "github.com/quadchroma-tech";
/// Wohin der Klick fuehrt (nur Windows oeffnet den Browser).
#[cfg_attr(not(windows), allow(dead_code))]
const WEBSITE_URL: &str = "https://github.com/quadchroma-tech/quadchroma";
/// FFmpeg-Hinweis (LGPL 2.1 Abschnitt 6 und Checkliste auf ffmpeg.org/legal):
/// zeigt das Programm im Betrieb Urheberhinweise, gehoert der von FFmpeg
/// dazu, samt Verweis auf den Lizenztext - THIRD_PARTY_NOTICES.txt liegt
/// jeder Weitergabe bei. Wie COPYRIGHT ein Rechtshinweis, nicht uebersetzt.
/// Nur unter Windows: der Mac-Client decodiert mit VideoToolbox und enthaelt
/// kein FFmpeg.
const FFMPEG_HINWEIS: Option<&str> =
    if cfg!(windows) { Some("Uses libraries from the FFmpeg project under the LGPLv2.1 · THIRD_PARTY_NOTICES.txt") } else { None };

/// Die Projektseite im Standardbrowser oeffnen. Wir starten nichts selbst,
/// sondern reichen die Adresse an das System weiter - das ist der einzige Weg,
/// der ohne zweites Programm auskommt.
#[cfg(windows)]
fn website_oeffnen() {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let ziel = HSTRING::from(WEBSITE_URL);
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

/// Wie ein Bildschirm des Hosts im Menue heisst: sein Name, sonst seine
/// Kennung (ein Host ohne Namen fuer den Bildschirm schickt einen leeren).
fn bildschirm_anzeigename(e: &bildschirm::BildschirmEintrag) -> &str {
    if e.name.is_empty() { &e.kennung } else { &e.name }
}

/// Der Knopftext eines Bildschirms: "<Name> · <Breite>×<Hoehe> · <Hz> Hz",
/// ohne den Hz-Teil, wenn der Host die Bildrate nicht kennt (0).
fn bildschirm_knopftext(e: &bildschirm::BildschirmEintrag) -> String {
    let mut t = format!("{} · {}×{}", bildschirm_anzeigename(e), e.breite, e.hoehe);
    if e.hz > 0 {
        t.push_str(&format!(" · {} Hz", e.hz));
    }
    t
}

/// Hat der Host mit dieser Liste einen Wunsch beantwortet? Ja, wenn der
/// gestreamte Eintrag zum Wunsch passt (bei Automatik: der Hauptbildschirm),
/// und ja, wenn der gewuenschte Bildschirm gar nicht angeschlossen ist -
/// dann streamt der Host den Ausweichplatz, und mehr passiert erst, wenn
/// der Bildschirm zurueckkommt.
fn bildschirm_wunsch_beantwortet(liste: &bildschirm::Bildschirme) -> bool {
    match liste.wunsch.as_deref() {
        Some(k) => liste.gestreamt().is_some_and(|e| e.kennung == k) || liste.gewuenschter().is_none(),
        None => liste.gestreamt().is_some_and(|e| e.haupt),
    }
}

/// Die Protokollzeile zu einer Liste (auch im Pruefmodus auf der Konsole):
/// "Bildschirme des Hosts (Wunsch <Kennung|Automatik>): <Name> <Kennung>
/// <B>x<H> <Hz> Hz [Haupt] [gestreamt]; ...".
fn bildschirme_zeile(liste: &bildschirm::Bildschirme) -> String {
    let eintraege: Vec<String> = liste
        .eintraege
        .iter()
        .map(|e| {
            let mut t = String::new();
            if !e.name.is_empty() {
                t.push_str(&e.name);
                t.push(' ');
            }
            t.push_str(&format!("{} {}x{} {} Hz", e.kennung, e.breite, e.hoehe, e.hz));
            if e.haupt {
                t.push_str(" [Haupt]");
            }
            if e.gestreamt {
                t.push_str(" [gestreamt]");
            }
            t
        })
        .collect();
    format!(
        "Bildschirme des Hosts (Wunsch {}): {}",
        liste.wunsch.as_deref().unwrap_or("Automatik"),
        if eintraege.is_empty() { "keine".to_string() } else { eintraege.join("; ") }
    )
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

/// Das Bild, wie es aus dem Decoder kommt: unter Windows ein Frame von
/// FFmpeg, auf dem Mac ein CVPixelBuffer von VideoToolbox (vt_decoder.rs).
/// `to_rgb` und die Protokollzeilen lesen beide ueber `Ebenenbild`.
#[cfg(windows)]
type Dekoderbild = ffmpeg::frame::Video;
#[cfg(target_os = "macos")]
type Dekoderbild = vt_decoder::Bild;

/// Was der Empfangsfaden ablegt: fertig gerechnetes RGB, oder - sobald die
/// Karte die Umrechnung uebernimmt - das rohe Decoderbild mit seinen Ebenen.
/// Ein rohes Bild kostet im Empfangsfaden keine Kopie: FFmpegs Frame wie der
/// CVPixelBuffer lassen sich zwischen Faeden verschieben. Unter Windows
/// zeichnet dann Direct3D 11, auf dem Mac Metal (der CVPixelBuffer geht als
/// IOSurface ohne Kopie auf die Karte); zeichnet softbuffer, kommt nur RGB an.
enum Bild {
    Rgb(Frame),
    Roh { bild: Dekoderbild, bereit_us: u64 },
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
    /// Bleibt, auch wenn der Schluessel es allein nicht sagt (siehe
    /// `bleibend`).
    bleibt: bool,
}

impl Meldung {
    fn neu(key: strings::Key, protokoll: impl Into<String>) -> Meldung {
        Meldung { key, werte: Vec::new(), anhang: None, protokoll: protokoll.into(), bleibt: false }
    }

    /// Diese Meldung bleibt (siehe `dauerhaft`), auch wenn ihr Schluessel
    /// sonst einen Neuversuch erlaubt: ein Protokollfehler rund um die
    /// Zugangsphase gibt sich mit dem naechsten Versuch nicht - jeder neue
    /// Versuch oeffnete beim Host nur eine neue Zugangsphase samt Anfrage
    /// (Spezifikation Pairing v1, 3.5 und 9.6: kein Endlos-Neuversuch).
    /// Derselbe Schluessel mitten in einer Sitzung darf weiter neu verbinden.
    fn bleibend(mut self) -> Meldung {
        self.bleibt = true;
        self
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
    /// Ablage oder Schluesseldatei kaputt, Liste kein UTF-8 oder nicht
    /// schreibbar - jeder Ausgang des Zugangs (Spezifikation Pairing v1,
    /// 9.6: abgelehnt, zu viele Versuche, keine Antwort, Host veraltet,
    /// Host-Beweis falsch, Host ohne Ausweis, anderes Geraet unter der ID,
    /// ID nicht gefunden) - der Abschied des Hosts (MSG_HOST_ENDE) und das
    /// Ziel "dieser Rechner selbst".
    /// Dann verbindet der Empfangsfaden nicht alle 2 s neu, sondern nimmt das
    /// Ziel zurueck (wie bei einer Abloesung); der Startbildschirm zeigt die
    /// Meldung, und der Nutzer verbindet selbst wieder.
    ///
    /// "Nicht lesbar" gehoert nicht dazu: das ist oft nur eine kurze Sperre
    /// (Virenscanner, Sicherung), und client.key wie hosts.txt werden VOR dem
    /// Verbinden gelesen - ein neuer Versuch alle 2 s oeffnet also keine
    /// Leitung, solange die Sperre besteht, der Host merkt nichts davon, und
    /// im Protokoll steht es dank der Entdoppelung einmal. Ist die Sperre
    /// binnen NEUVERSUCH_FRIST weg, verbindet der Client von selbst.
    ///
    /// Dazu jede Meldung, die `bleibend` markiert ist (Protokollfehler rund
    /// um die Zugangsphase).
    fn dauerhaft(&self) -> bool {
        use strings::Key::*;
        self.bleibt
            || matches!(
                self.key,
                FileNotUtf8
                    | FileNotWritable
                    | KeyFileDamaged
                    | StorageUnavailable
                    | MsgRefused
                    | MsgNoAnswer
                    | MsgTooManyAttempts
                    | MsgHostOutdated
                    | MsgHostProofBad
                    | MsgHostUnverified
                    | MsgOtherDevice
                    | MsgIdNotFound
                    | MsgDeviceRemoved
                    | MsgHostQuit
                    | MsgHostSharingOff
                    | MsgHostRemovedYou
                    | MsgSelf
            )
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
            F::AnderesGeraet { .. } => Meldung::neu(MsgOtherDevice, protokoll),
            F::EigenerHost { .. } => Meldung::neu(MsgSelf, protokoll),
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
    /// Verbunden ueber eine Geraete-ID (Liste, Eingabe, Verknuepfung): die
    /// gewaehlte ID. Der Handschlag prueft nach Nachricht 2, dass der
    /// Schluessel des Hosts sie ergibt (Spezifikation Pairing v1, 8.2), und
    /// meldet sich der Host inzwischen unter einer anderen Adresse, verbindet
    /// der naechste Versuch dorthin. Gilt nur zusammen mit `target`.
    ziel_id: Option<u32>,
    /// Name des Ziels aus der Bekanntgabe (oder hosts.txt) - fuer Meldungen
    /// und den Eintrag in hosts.txt, bis der Host selbst einen nennt.
    ziel_name: Option<String>,
    /// Die Liste der Bekanntgaben, damit der Empfangsfaden ein Ziel mit ID
    /// unter seiner neuen Adresse findet. None in Tests.
    bekanntgaben: Option<Arc<Mutex<discovery::Hosts>>>,
    /// Zu diesem Ziel lief seit der Wahl durch den Nutzer (`App::verbinden`)
    /// schon eine angenommene Sitzung: der Schluessel dieses Hosts. Sagt der
    /// Host beim Wiederverbinden danach "QCA1" (dort entfernt, Liste
    /// zurueckgesetzt), stellt der Client keine Zugangsanfrage von selbst:
    /// der Nutzer hat nicht darum gebeten, und am Host ginge ein
    /// Zulassen-Fenster auf, das niemand erwartet. Er zieht sie sofort
    /// zurueck (23), und die Meldung bleibt. Der Schluessel gilt beim
    /// Wiederverbinden als gepinnt (Vorwissen::angenommen), auch wenn
    /// hosts.txt sich nicht schreiben liess.
    angenommen: Option<Vec<u8>>,
    /// Zugangsphase laeuft: was der Dialog zeigt (Empfangsfaden -> Fenster).
    zugang: Option<zugangsphase::Dialog>,
    /// Eingabe des Nutzers im Zugangsdialog (Fenster -> Empfangsfaden).
    zugang_eingabe: Option<zugangsphase::Eingabe>,
    /// Zaehlt jede Aenderung an hosts.txt durch den Empfangsfaden - das
    /// Fenster liest die Liste (Haken an bekannten Hosts) dann neu.
    hosts_stand: u64,
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
    /// Bildschirme des Hosts, wie sie mit MSG_BILDSCHIRME zuletzt ankamen,
    /// und sein Wunsch dazu (None = Automatik, der Host folgt seinem
    /// Hauptbildschirm). Je Sitzung zurueckgesetzt wie `codecs`.
    bildschirme: Vec<bildschirm::BildschirmEintrag>,
    bildschirm_wunsch: Option<String>,
    /// Der Host kennt die Bildschirmwahl: Bit 1 (FAEHIG_BILDSCHIRM) in
    /// MSG_FAEHIGKEITEN DIESER Sitzung. Ohne das Bit zeigt das Menue keine
    /// Bildschirmzeile, und kein IN_BILDSCHIRM geht hinaus.
    host_bildschirmwahl: bool,
    /// Ein Bildschirmwunsch ist unterwegs: seit wann, und welcher (None =
    /// Automatik). Geloescht, sobald eine Liste kommt, deren gestreamter
    /// Eintrag zu diesem Wunsch passt (oder die ihn als nicht angeschlossen
    /// beantwortet), sonst nach CODEC_WECHSEL_FRIST.
    bildschirm_wechsel: Option<(Instant, Option<String>)>,
    /// Wie oft in dieser Sitzung ein Decoder gebaut wurde - Pruefnaht: eine
    /// Strominfo mit anderer Groesse muss einen neuen Bau ergeben.
    decoder_baue: u32,
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
    /// Laufende Nummer der Sitzung, run_session zaehlt sie beim Aufbau hoch.
    /// Sender und Empfaenger melden ihren Stand nur, solange sie gilt: ein
    /// spaetes "abgebrochen" der vorigen Sitzung gehoert nicht in die neue.
    sitzung_nr: u64,
    /// Der Host kann Dateien: MSG_FAEHIGKEITEN mit FAEHIG_DATEIEN in DIESER
    /// Sitzung (Spezifikation 2.2). Zurueck bei jedem Sitzungsende.
    host_dateien: bool,
    /// MSG_FAEHIGKEITEN kam in dieser Sitzung an (mit oder ohne Dateien).
    /// Solange nicht, und hoechstens DATEI_ANLAUF nach dem Sitzungsbeginn,
    /// wird eine Kopie vorgemerkt statt FilesPeerOld gemeldet.
    faehigkeiten_da: bool,
    /// Beginn dieser Sitzung (Handschlag steht), fuer DATEI_ANLAUF.
    sitzung_seit: Option<Instant>,
    /// Eine Kopie, die auf MSG_FAEHIGKEITEN wartet (Integrationstest 574cd3e,
    /// Befund 3); hoechstens eine, die neueste gilt.
    datei_vorgemerkt: Option<DateiVormerkung>,
    /// FilesPeerOld stand in dieser Sitzung schon da (einmal je Sitzung).
    host_zu_alt_gemeldet: bool,
    /// Die laufende Datei-Sendung Client -> Host, hoechstens eine. Fallen
    /// lassen bricht sie ab (Griff::drop); DATEI_QUITTUNG vom Host geht an sie.
    datei_senden: Option<DateiSendung>,
    /// Was die Zeile ueber dem Bild zu Dateiuebertragungen zeigt.
    datei_stand: DateiStaende,
    /// Pruefnaht: Basis und Ablage-Aktion fuer empfangene Dateien. None in
    /// der Produktion (dateien::ablage_basis(), clipboard::set_dateien);
    /// Tests setzen einen eigenen Ordner und einen Rekorder, damit weder die
    /// echte Ablage noch die Basis anderer Tests angefasst wird.
    datei_ablage: Option<DateiAblage>,
    /// Pruefnaht: Frist fuer die Neuversuche (NEUVERSUCH_FRIST); None in der
    /// Produktion. Tests setzen eine kurze.
    neuversuch_frist: Option<Duration>,
}

/// Wohin empfangene Dateien gehen und wie sie in die Ablage kommen.
#[derive(Clone)]
struct DateiAblage {
    basis: std::path::PathBuf,
    ablegen: Arc<dyn Fn(Vec<std::path::PathBuf>) -> bool + Send + Sync>,
}

impl DateiAblage {
    /// Die der Produktion: Basis im Temp-Ordner, Dateiliste in die eigene Ablage.
    fn produktion() -> DateiAblage {
        #[cfg(any(windows, target_os = "macos"))]
        let ablegen: Arc<dyn Fn(Vec<std::path::PathBuf>) -> bool + Send + Sync> =
            Arc::new(|p: Vec<std::path::PathBuf>| clipboard::set_dateien(protokoll::Herkunft::Client, &p));
        #[cfg(not(any(windows, target_os = "macos")))]
        let ablegen: Arc<dyn Fn(Vec<std::path::PathBuf>) -> bool + Send + Sync> = Arc::new(|_| false);
        DateiAblage { basis: dateien::ablage_basis(), ablegen }
    }
}

/// Eine laufende Sendung Client -> Host: ihr Griff und der Eingabekanal, an
/// den sie gebunden ist. Fallen lassen bricht sie ab (Griff::drop).
struct DateiSendung {
    griff: dateien::Griff,
    /// Nummer des Eingabekanals (InputLink::kanal_nr), auf dem ihr erstes
    /// Paket hinausging; 0, solange keins hinaus ist. datei_weg setzt sie,
    /// datei_kanal_pruefen vergleicht sie mit dem stehenden Kanal.
    kanal: Arc<std::sync::atomic::AtomicU64>,
}

impl DateiSendung {
    fn kennung(&self) -> u32 {
        self.griff.kennung()
    }

    /// Nutzlast einer DATEI_QUITTUNG vom Host; kehrt sofort zurueck.
    fn quittung(&self, nutzlast: &[u8]) {
        self.griff.quittung(nutzlast);
    }

    /// Der Eingabekanal, an den sie gebunden ist (None: noch keiner).
    fn gebunden(&self) -> Option<u64> {
        Some(self.kanal.load(std::sync::atomic::Ordering::SeqCst)).filter(|&k| k != 0)
    }
}

/// Eine Kopie, die auf die Faehigkeiten des Hosts wartet: die Pfade, bis
/// wann sie wartet, und die Sitzung, in der sie entstand.
#[derive(Debug)]
struct DateiVormerkung {
    pfade: Vec<std::path::PathBuf>,
    bis: Instant,
    sitzung: u64,
}

/// Anlauf einer Sitzung fuer Dateien (Integrationstest 574cd3e, Befund 3):
/// so lange wartet eine Kopie direkt nach dem Verbinden auf MSG_FAEHIGKEITEN
/// (vorgemerkt statt FilesPeerOld), und so lange wartet eine neue Sendung auf
/// einen Eingabekanal, der noch im Aufbau ist (Voll statt Weg). Der Host
/// schickt die Faehigkeiten gleich nach Gruss und Strominfo; wer sie nach
/// DATEI_ANLAUF nicht hat, ist ein aelterer Host.
const DATEI_ANLAUF: Duration = Duration::from_secs(5);

/// So lange bleibt die Zeile nach dem Ergebnis einer Uebertragung stehen.
const DATEI_NACHLAUF: Duration = Duration::from_secs(6);

/// Eine Richtung der Dateiuebertragung fuer die Anzeige: der letzte Stand
/// und, sobald er ein Ergebnis traegt, seit wann.
#[derive(Clone, Debug)]
struct DateiAnzeige {
    stand: dateien::Stand,
    ergebnis_seit: Option<Instant>,
}

/// Die Zeilen ueber dem Bild: je Richtung eine, weil Senden und Empfangen
/// zugleich laufen duerfen (Spezifikation 2.7 Schritt 6).
#[derive(Clone, Debug, Default)]
struct DateiStaende {
    senden: Option<DateiAnzeige>,
    empfangen: Option<DateiAnzeige>,
}

impl DateiStaende {
    /// Einen Stand uebernehmen, zugeordnet nach der Kennung: Kennungen
    /// steigen je Sender (Spezifikation 2.3). Der Stand einer aelteren
    /// Uebertragung - etwa das "abgebrochen" eines Senders, den ein neuer
    /// abgeloest hat und der sich erst meldet, wenn der neue schon laeuft -
    /// ueberschreibt den neueren nicht. Kennung 0 (FilesPeerOld) gilt immer.
    fn setzen(&mut self, st: dateien::Stand) {
        let platz = match st.richtung {
            dateien::Richtung::Senden => &mut self.senden,
            dateien::Richtung::Empfangen => &mut self.empfangen,
        };
        if let Some(alt) = platz.as_ref() {
            if st.kennung != 0 && alt.stand.kennung != 0 && st.kennung < alt.stand.kennung {
                return;
            }
        }
        let ergebnis_seit = if st.laeuft() { None } else { Some(Instant::now()) };
        *platz = Some(DateiAnzeige { stand: st, ergebnis_seit });
    }

    /// Was jetzt zu sehen ist: laufende Uebertragungen und Ergebnisse der
    /// letzten DATEI_NACHLAUF, Empfangen vor Senden.
    fn sichtbare(&self, jetzt: Instant) -> Vec<&dateien::Stand> {
        [&self.empfangen, &self.senden]
            .into_iter()
            .flatten()
            .filter(|a| a.ergebnis_seit.is_none_or(|t| jetzt.saturating_duration_since(t) < DATEI_NACHLAUF))
            .map(|a| &a.stand)
            .collect()
    }

    fn sichtbar(&self, jetzt: Instant) -> bool {
        !self.sichtbare(jetzt).is_empty()
    }
}

/// Text und Farbe der Zeile fuer einen Stand. Groessen in MB (10^6) mit
/// einer Nachkommastelle, wie die Zahlen der Statistik.
fn datei_zeile(st: &dateien::Stand, lang: &strings::Lang) -> (String, u32) {
    use dateien::{Ergebnis, Richtung};
    use strings::Key::*;
    let mb = |b: u64| format!("{:.1}", b as f64 / 1e6);
    let senden = st.richtung == Richtung::Senden;
    let (m, farbe) = match &st.ergebnis {
        Ergebnis::Laeuft => (
            Meldung::neu(if senden { FilesSending } else { FilesReceiving }, "")
                .mit("{n}", mb(st.bytes))
                .mit("{m}", format!("{} MB", mb(st.gesamt))),
            ui::TEXT,
        ),
        Ergebnis::Fertig => (
            Meldung::neu(if senden { FilesSent } else { FilesReady }, "").mit("{n}", st.oberste.to_string()),
            ui::CYAN,
        ),
        Ergebnis::Abgebrochen(a) => {
            (Meldung::neu(FilesAborted, a.text()).mit("{n}", lang.get(abbruch_schluessel(a))), ui::AMBER)
        }
        Ergebnis::ZuGross => (Meldung::neu(FilesTooLarge, ""), ui::AMBER),
        Ergebnis::GegenseiteZuAlt => (Meldung::neu(FilesPeerOld, ""), ui::AMBER),
    };
    (m.text(lang), farbe)
}

/// Der Abbruchgrund in Worten der Oberflaeche (Durchsicht [13]): ein
/// Schluessel je haeufigem Grund. Einzelheiten - Pfade, Systemtexte, der
/// genaue Zustand - stehen nur im Protokoll (Abbruch::text, die Zeile des
/// Kerns). Die Gruende gelten in beide Richtungen: "zu wenig Speicherplatz"
/// meldet beim Senden die Gegenseite (Quittung 3), beim Empfangen lehnt diese
/// Seite selbst ab.
fn abbruch_schluessel(a: &dateien::Abbruch) -> strings::Key {
    use dateien::Abbruch as A;
    use strings::Key::*;
    match a {
        A::Hier => FilesAbortLocal,
        A::Verbindung => FilesAbortConnection,
        A::Lesefehler(_) => FilesAbortRead,
        A::Zeitueberschreitung => FilesAbortTimeout,
        A::Quittung(z) => match *z {
            dateien::ZUSTAND_KEIN_PLATZ => FilesAbortNoSpace,
            dateien::ZUSTAND_SCHREIBFEHLER => FilesAbortWrite,
            // Quittung 6: der Empfaenger hat aufgegeben (Stillstand u. ae.).
            dateien::ZUSTAND_ABGEBROCHEN => FilesAbortPeer,
            // 4 (ungueltig), 1 zu frueh oder ein unbekannter Zustand.
            _ => FilesAbortInvalid,
        },
        A::Ende(g) => match *g {
            dateien::GRUND_ABGEBROCHEN => FilesAbortPeer,
            dateien::GRUND_LESEFEHLER => FilesAbortRead,
            dateien::GRUND_ZEIT => FilesAbortTimeout,
            _ => FilesAbortInvalid,
        },
        A::Abgelehnt(z, _) if *z == dateien::ZUSTAND_KEIN_PLATZ => FilesAbortNoSpace,
        A::Abgelehnt(..) | A::Ungueltig(_) => FilesAbortInvalid,
        A::Schreibfehler(_) => FilesAbortWrite,
        A::Ablage => FilesAbortClipboard,
    }
}

/// Die schmalen Zeilen zu Dateiuebertragungen unten mittig ueber dem Bild,
/// je sichtbarer Richtung eine. Derselbe Aufruf fuer Prozessor- und
/// Grafikweg (oberflaeche_zeichnen) und fuer --shot.
fn datei_zeilen_zeichnen(u: &mut ui::Ui, c: &mut ui::Canvas, ww: u32, wh: u32, zeilen: &[(String, u32)]) {
    const GROESSE: u32 = 13;
    const HOEHE: i32 = 26;
    const ABSTAND: i32 = 6;
    let n = zeilen.len() as i32;
    let mut y = wh as i32 - 22 - n * HOEHE - (n - 1).max(0) * ABSTAND;
    let breite = (ww as i32 - 80).max(120);
    for (t, farbe) in zeilen {
        let t = kuerzen(u, t, breite - 32, GROESSE, 1);
        let tw = u.text.width(&t, GROESSE, 1);
        c.fill(ww as i32 / 2 - tw / 2 - 16, y, tw + 32, HOEHE, ui::BG, 200);
        u.text.draw_centered(c, ww as i32 / 2, y + 18, &t, GROESSE, *farbe, 1);
        y += HOEHE + ABSTAND;
    }
}

impl Shared {
    /// Sitzungsende fuer Dateien: die Faehigkeit des Hosts gilt nicht mehr,
    /// eine vorgemerkte Kopie faellt weg, die laufende Sendung kommt heraus -
    /// fallen lassen (ausserhalb der Sperre) bricht sie ab.
    fn dateien_zuruecksetzen(&mut self) -> Option<DateiSendung> {
        self.host_dateien = false;
        self.faehigkeiten_da = false;
        self.datei_vorgemerkt = None;
        self.datei_senden.take()
    }

    /// Stehen die Faehigkeiten des Hosts noch aus, obwohl sie bald kommen
    /// muessten (Sitzung juenger als DATEI_ANLAUF)? Dann wird eine Kopie
    /// vorgemerkt statt FilesPeerOld gemeldet.
    fn faehigkeiten_offen(&self) -> bool {
        self.link.is_some() && !self.faehigkeiten_da && self.sitzung_seit.is_some_and(|t| t.elapsed() < DATEI_ANLAUF)
    }

    /// Laeuft gerade ein Codecwechsel, den man dem Nutzer erklaeren sollte?
    fn wechsel_laeuft(&self) -> bool {
        self.codec_wechsel.map(|t| t.elapsed() < CODEC_WECHSEL_FRIST).unwrap_or(false)
    }

    /// Laeuft gerade ein Bildschirmwechsel? Dieselbe Frist wie beim Codec:
    /// antwortet der Host nie, verschwindet der Hinweis von selbst.
    fn bildschirm_wechsel_laeuft(&self) -> bool {
        self.bildschirm_wechsel.as_ref().map(|(t, _)| t.elapsed() < CODEC_WECHSEL_FRIST).unwrap_or(false)
    }

    /// Sitzungsanfang und -ende fuer die Bildschirmwahl: Liste, Wunsch,
    /// Faehigkeit und Hinweis des vorigen Hosts haben in der naechsten
    /// Sitzung nichts mehr zu suchen.
    fn bildschirme_zuruecksetzen(&mut self) {
        self.bildschirme.clear();
        self.bildschirm_wunsch = None;
        self.host_bildschirmwahl = false;
        self.bildschirm_wechsel = None;
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

/// Eine Sitzung zu `addr` endete mit `e`: anzeigen, protokollieren, bei
/// einem Dauerfehler das Ziel zuruecknehmen. Nur, solange das Ziel noch
/// `addr` ist - eine gewollte Trennung kappt die Leitung (der Lesefehler
/// danach ist kein Fehler), und hat der Nutzer inzwischen einen anderen Host
/// gewaehlt, gehoert der Fehler nicht zu dem, und dessen Ziel bleibt stehen.
fn fehler_verbuchen(s: &mut Shared, addr: &str, e: Meldung, gemeldet: &mut Option<(Meldung, u64)>) {
    s.connected = false;
    if s.target.as_deref() != Some(addr) {
        return;
    }
    if neu_zu_melden(gemeldet, &e, s.decoded) {
        protokoll::zeile(format!("Verbindung: {}", e.protokoll));
    }
    // Was sich mit dem naechsten Versuch nicht gibt, wird nicht alle 2 s
    // wiederholt: das Ziel geht zurueck, die Meldung bleibt stehen (siehe
    // Meldung::dauerhaft).
    if e.dauerhaft() {
        protokoll::zeile(format!("Verbindung zu {addr} beendet: der Fehler bleibt bis zur Behebung - kein neuer Versuch"));
        s.target = None;
    }
    s.error = Some(e);
}

/// So lange versucht der Empfangsfaden hoechstens, ein Ziel (wieder) zu
/// erreichen - nach einem Abbruch ohne Abschied des Hosts, oder wenn die
/// erste Verbindung nicht zustande kommt. Danach gibt er auf: das Ziel geht
/// zurueck, der Startbildschirm zeigt "Verbindung verloren" (stand schon eine
/// Sitzung) bzw. den letzten Fehler. Vorher lief das endlos, und der Client
/// blieb auf "Verbinde neu" stehen, wenn der Host nie wiederkam.
const NEUVERSUCH_FRIST: Duration = Duration::from_secs(30);

/// Eine Reihe von Versuchen zu einem Ziel ohne stehende Sitzung.
#[derive(Clone, Debug, PartialEq)]
struct Neuversuche {
    /// Das Ziel, zu dem die Reihe gehoert (eine neue Adresse desselben
    /// Hosts, die der Empfangsfaden ueber die ID findet, fuehrt sie fort).
    ziel: String,
    /// Seit wann: Ende der letzten Sitzung, sonst Beginn des ersten Versuchs.
    seit: Instant,
    /// Stand in dieser Reihe schon eine Sitzung (dann heisst Aufgeben
    /// "Verbindung verloren")?
    nach_sitzung: bool,
}

/// Nach einem Durchlauf von run_session: die Reihe der Versuche fortfuehren
/// und nach `frist` aufgeben. `beginn` ist der Beginn dieses Durchlaufs,
/// `sitzung`: in ihm stand eine Sitzung (sie ist eben zu Ende gegangen - ab
/// jetzt zaehlt die Frist neu). Gilt das Ziel nicht mehr (getrennt, anderes
/// Ziel, eine Meldung, die bleibt), endet die Reihe ohne Zutun. true: eben
/// aufgegeben - Ziel zurueck, die Meldung bleibt.
fn neuversuch_buchen(
    s: &mut Shared,
    addr: &str,
    reihe: &mut Option<Neuversuche>,
    beginn: Instant,
    sitzung: bool,
    jetzt: Instant,
    frist: Duration,
) -> bool {
    if s.target.as_deref() != Some(addr) {
        *reihe = None;
        return false;
    }
    match reihe {
        Some(r) if !sitzung && r.ziel == addr => {}
        _ => {
            *reihe = Some(Neuversuche {
                ziel: addr.to_string(),
                seit: if sitzung { jetzt } else { beginn },
                nach_sitzung: sitzung,
            })
        }
    }
    let Some(r) = reihe.as_ref() else { return false };
    let dauer = jetzt.saturating_duration_since(r.seit);
    if dauer < frist {
        return false;
    }
    let zeile = format!(
        "Verbindung zu {addr}: seit {} s keine Verbindung - aufgegeben, kein neuer Versuch",
        dauer.as_secs()
    );
    protokoll::zeile(zeile.clone());
    let m = match s.error.take() {
        Some(e) if !r.nach_sitzung => e.bleibend(),
        _ => Meldung::neu(strings::Key::ConnectionLost, zeile).bleibend(),
    };
    s.target = None;
    s.error_key = None;
    s.error = Some(m);
    *reihe = None;
    true
}

/// Netz- und Decodierschleife. Laeuft in einem eigenen Faden und legt immer nur
/// das neueste Bild ab: lieber eines auslassen als Verzoegerung aufbauen.
fn stream_thread(shared: Arc<Mutex<Shared>>, input: Arc<Mutex<InputLink>>) {
    // VideoToolbox (Mac) braucht keinen Start.
    #[cfg(windows)]
    if let Err(e) = ffmpeg::init() {
        let m = Meldung::neu(strings::Key::ErrorFfmpegStart, format!("FFmpeg-Start fehlgeschlagen: {e}")).anhang(e.to_string());
        protokoll::zeile(m.protokoll.clone());
        shared.lock().unwrap().error = Some(m);
        return;
    }
    // Im Pruefmodus alles einsammeln, was die Decoder-Bibliothek zu sagen
    // hat; im Fenster nur Warnungen und Fehler - fuer die Datei und fuer den
    // Grund, wenn ein Hardware-Decoder schon beim Oeffnen scheitert.
    protokoll::einschalten(std::env::args().any(|a| a == "--headless"));

    // Zuletzt protokollierte Verbindungsmeldung und der Bildzaehler dazu:
    // dieselbe Meldung ohne ein einziges Bild dazwischen steht nur einmal im
    // Protokoll. `error` taugt dafuer nicht - run_session loescht es schon
    // nach dem Handschlag, also vor der Antwort des Hosts und allem danach.
    let mut gemeldet: Option<(Meldung, u64)> = None;
    // Die laufende Reihe von Versuchen ohne Sitzung (NEUVERSUCH_FRIST).
    let mut reihe: Option<Neuversuche> = None;
    loop {
        let (addr, ziel_id, bekanntgaben) = {
            let s = shared.lock().unwrap();
            (s.target.clone(), s.ziel_id, s.bekanntgaben.clone())
        };
        let Some(mut addr) = addr else {
            // Ohne Ziel beginnt die Entdoppelung von vorn: verbindet der
            // Nutzer neu, steht der erste Fehler wieder im Protokoll. Ebenso
            // die Frist der Neuversuche.
            gemeldet = None;
            reihe = None;
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        // Ziel ueber eine ID: meldet sich der Host inzwischen nur noch unter
        // einer anderen Adresse (neue IP vom Router), gilt die neue - solange
        // er sich (auch) unter der bisherigen meldet, bleibt sie (ein Host
        // mit zwei Netzkarten ruft unter beiden). Vertraut wird der
        // Bekanntgabe dabei nicht - der Handschlag prueft die ID am
        // Schluessel (8.2).
        if let (Some(id), Some(b)) = (ziel_id, bekanntgaben) {
            let (neu, bisher) = match b.lock() {
                Ok(h) => (
                    h.mit_id(id).map(|g| g.host.addr.to_string()),
                    h.liste().iter().any(|g| g.id == Some(id) && g.host.addr.to_string().eq_ignore_ascii_case(&addr)),
                ),
                Err(_) => (None, true),
            };
            if let Some(neu) = neu.filter(|_| !bisher) {
                let mut s = shared.lock().unwrap();
                if s.target.as_deref() == Some(addr.as_str()) {
                    protokoll::zeile(format!("Ziel ID {} meldet sich jetzt unter {neu} (vorher {addr})", zugang::id_text(id)));
                    s.target = Some(neu.clone());
                    drop(s);
                    input.lock().unwrap().set_addr(bump_port(&neu, 1));
                    // Derselbe Host: die Reihe der Versuche laeuft weiter.
                    if let Some(r) = reihe.as_mut().filter(|r| r.ziel == addr) {
                        r.ziel = neu.clone();
                    }
                    addr = neu;
                }
            }
        }
        let (beginn, nr_vorher) = (Instant::now(), shared.lock().unwrap().sitzung_nr);
        let ergebnis = run_session(&addr, &shared, &input);
        // Ohne Sitzung liest der Client die Zwischenablage nicht mehr.
        #[cfg(any(windows, target_os = "macos"))]
        clipboard::sitzung(false);
        if let Err(e) = ergebnis {
            fehler_verbuchen(&mut shared.lock().unwrap(), &addr, e, &mut gemeldet);
        }
        let datei = {
            let mut s = shared.lock().unwrap();
            s.abbruch = None;
            s.link = None;
            // Ein offener Zugangsdialog gehoert zu dieser Leitung.
            s.zugang = None;
            s.zugang_eingabe = None;
            // Die Form geht, die Nummer zaehlt weiter: fiele sie auf null,
            // ueberspraenge der Fensterfaden nach dem Wiederverbinden genau die
            // Form mit der alten Nummer.
            s.zeiger = None;
            s.connected = false;
            s.codec_wechsel = None;
            s.codec_idx = None;
            s.bildschirme_zuruecksetzen();
            // Dateien: Sender ab, Faehigkeit zurueck (das tat schon das Ende
            // von run_session; der Empfaenger fiel mit ihr weg).
            let datei = s.dateien_zuruecksetzen();
            // Nicht endlos neu versuchen (NEUVERSUCH_FRIST).
            let frist = s.neuversuch_frist.unwrap_or(NEUVERSUCH_FRIST);
            let sitzung = s.sitzung_nr != nr_vorher;
            neuversuch_buchen(&mut s, &addr, &mut reihe, beginn, sitzung, Instant::now(), frist);
            datei
        };
        drop(datei);
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(windows)]
use ffmpeg_next as ffmpeg;

/// Der Decoder und sein Fehler: unter Windows FFmpeg (NVDEC ueber cuvid,
/// D3D11VA, Software), auf dem Mac VideoToolbox mit oder ohne Hardware.
#[cfg(windows)]
type Dekoder = ffmpeg::decoder::Video;
#[cfg(windows)]
type DekoderFehler = ffmpeg::Error;
#[cfg(target_os = "macos")]
type Dekoder = vt_decoder::Decoder;
#[cfg(target_os = "macos")]
type DekoderFehler = vt_decoder::Fehler;

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
    #[cfg_attr(not(windows), allow(dead_code))]
    Nvdec(Option<i64>),
    /// Direct3D 11 Video (D3D11VA) auf der Karte dieser Rolle - fuer AMD und
    /// Intel, nur 4:2:0 und H.264. Die Bilder kommen von der Karte in den
    /// Hauptspeicher (Kopierstufe).
    #[cfg_attr(not(windows), allow(dead_code))]
    D3d11va(Rolle),
    /// Mac: VideoToolbox in der Media-Engine (Hardware, auch HEVC 4:4:4).
    #[cfg_attr(windows, allow(dead_code))]
    VideoToolbox,
    /// Der Decoder auf der CPU: unter Windows der eingebaute von FFmpeg, auf
    /// dem Mac VideoToolbox ohne Hardware.
    Software,
}

impl DecoderPfad {
    pub fn name(self) -> String {
        match self {
            DecoderPfad::Nvdec(_) => "NVDEC".into(),
            DecoderPfad::D3d11va(r) => format!("D3D11VA ({})", r.name()),
            DecoderPfad::VideoToolbox => "VideoToolbox".into(),
            DecoderPfad::Software if cfg!(target_os = "macos") => "VideoToolbox (Software)".into(),
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
    decoder: Dekoder,
    pfad: DecoderPfad,
    /// FFmpeg-Name des Decoders, etwa "hevc_cuvid" oder "hevc" (auf dem Mac
    /// "hevc" oder "h264").
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
    fn neu(decoder: Dekoder, pfad: DecoderPfad, codec: &'static str, grund: Option<String>) -> Self {
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
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg_attr(not(windows), allow(dead_code))]
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
#[cfg(windows)]
static NVDEC_GEMELDET: Mutex<Option<(i64, i32)>> = Mutex::new(None);

/// Die Zuordnung "Karte ist CUDA-Geraet n" gehoert einmal ins Protokoll,
/// nicht bei jedem Neubau des Decoders. true, wenn sie sich seit der letzten
/// Meldung geaendert hat (und merkt sie sich dann).
#[cfg_attr(not(windows), allow(dead_code))]
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
#[cfg_attr(not(windows), allow(dead_code))]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
    let d = software_dekoder(bedarf.h264)?;
    let mut bau = DecoderBau::neu(d, DecoderPfad::Software, if bedarf.h264 { "h264" } else { "hevc" }, Some(grund));
    bau.chroma444 = bedarf.chroma444;
    Ok(bau)
}

/// Der Decoder auf der CPU: FFmpegs eingebauter mit seinen Faeden.
#[cfg(windows)]
fn software_dekoder(h264: bool) -> Result<Dekoder, String> {
    software_decoder(h264)
}

/// Der Decoder auf der CPU: VideoToolbox ohne Hardware.
#[cfg(target_os = "macos")]
fn software_dekoder(h264: bool) -> Result<Dekoder, String> {
    Ok(vt_decoder::Decoder::neu(h264, false))
}

/// Ein decodiertes Bild fuer das Protokoll beschreiben: Groesse und Format,
/// so wie der Decoder sie liefert - was to_rgb gleich zu sehen bekommt.
fn bild_beschreiben(codec: &str, bild: &impl Ebenenbild) -> String {
    format!(
        "Erstes Bild aus {}: {}x{} {}, Zeilen {}/{}/{} Byte, Ebenen {}",
        codec, bild.breite(), bild.hoehe(), bild.format_name(),
        bild.zeilenlaenge(0),
        if bild.ebenenzahl() > 1 { bild.zeilenlaenge(1) } else { 0 },
        if bild.ebenenzahl() > 2 { bild.zeilenlaenge(2) } else { 0 },
        bild.ebenenzahl()
    )
}

/// Was zum Bau eines Decoders ausser dem Wunsch noch zaehlt: der Codec, ob
/// der Strom 4:4:4 ist (None: noch nicht bekannt), und der Adapter, auf dem
/// die Anzeige laeuft (None: Software, WARP oder ohne Fenster).
#[derive(Clone, Copy)]
struct DecoderBedarf {
    h264: bool,
    chroma444: Option<bool>,
    #[cfg_attr(not(windows), allow(dead_code))]
    anzeige_adapter: Option<u32>,
}

/// D3D11VA auf dieser Karte versuchen - oder gleich Software, wenn der
/// Strom 4:4:4 ist. Ergebnis: der fertige Bau, oder der Grund, warum nicht.
#[cfg(windows)]
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
#[cfg(windows)]
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

/// Decoder auf dem Mac: VideoToolbox in der Media-Engine - bei jedem Wunsch
/// ausser Prozessor (Karten zum Waehlen gibt es dort nicht). Die Media-Engine
/// kann HEVC 4:4:4 wie 4:2:0 und H.264, also braucht es keinen Blick auf
/// den Strom. Ob sie diesen Strom wirklich kann, zeigt erst das erste Paket
/// mit Parametersaetzen: dann entsteht die Sitzung, und scheitert sie, faellt
/// die Empfangsschleife wie bei NVDEC auf den Prozessor zurueck
/// (`auf_software`, VideoToolbox ohne Hardware).
#[cfg(target_os = "macos")]
fn decoder_bauen(bedarf: DecoderBedarf, wunsch: einstellungen::DecoderWunsch) -> Result<DecoderBau, String> {
    let hardware = wunsch != einstellungen::DecoderWunsch::Software;
    let pfad = if hardware { DecoderPfad::VideoToolbox } else { DecoderPfad::Software };
    let codec = if bedarf.h264 { "h264" } else { "hevc" };
    let mut bau = DecoderBau::neu(vt_decoder::Decoder::neu(bedarf.h264, hardware), pfad, codec, None);
    bau.chroma444 = bedarf.chroma444;
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
                DecoderPfad::VideoToolbox | DecoderPfad::Software => false,
            }
        }
    }
}

/// Muss der Decoder neu gebaut werden, weil die Strominfo jetzt sagt, ob
/// der Strom 4:4:4 ist? Nur, wenn das an der Wahl etwas aendert: D3D11VA
/// laeuft und der Strom ist 4:4:4 (geht nicht), oder Software laeuft nur
/// wegen 4:4:4 und der Strom ist es nicht mehr. VideoToolbox kann beides.
fn chroma_erzwingt_neubau(bau: &DecoderBau, chroma444: bool) -> bool {
    if bau.chroma444 == Some(chroma444) {
        return false;
    }
    match bau.pfad {
        DecoderPfad::D3d11va(_) => chroma444,
        DecoderPfad::Software => bau.wegen_444 && !chroma444,
        DecoderPfad::Nvdec(_) | DecoderPfad::VideoToolbox => false,
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
    s.decoder_baue = s.decoder_baue.saturating_add(1);
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
#[cfg_attr(not(windows), allow(dead_code))]
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
#[cfg(windows)]
fn decoder_defekt(e: &DekoderFehler) -> bool {
    !matches!(e, ffmpeg::Error::Other { errno: ffmpeg::util::error::EAGAIN } | ffmpeg::Error::Eof)
}

/// VideoToolbox kennt kein "gerade nichts da": jeder Fehler ist einer.
#[cfg(target_os = "macos")]
fn decoder_defekt(_e: &DekoderFehler) -> bool {
    true
}

/// Die Zugangsphase nach "QCA1" (Spezifikation Pairing v1, 3.5): Nachricht
/// 20 lesen, den Dialog zeigen (`Shared::zugang`), die Eingaben des Nutzers
/// (`Shared::zugang_eingabe`) an den Automaten geben, bis der Host
/// entscheidet - nach der Annahme folgt "QCH1". Ok(Some(name)): angenommen,
/// mit dem Namen, den der Host in Nachricht 20 nennt. Ok(None): der Nutzer
/// hat abgebrochen (Nachricht 23 ist hinaus). Jeder andere Ausgang ist eine
/// Meldung, die bleibt - kein automatischer Neuversuch (9.6).
fn zugang_durchlaufen(
    sock: &mut secure::Secure,
    shared: &Arc<Mutex<Shared>>,
    vorwissen: &zugangsphase::Vorwissen,
    name: &str,
    addr: &str,
) -> Result<Option<String>, Meldung> {
    use zugangsphase::{Ausgang, Eingabe, Kennung};
    let noetig = zugangsphase::noetig_lesen(sock).map_err(|a| zugang_meldung(a, name, addr))?;
    let neue_identitaet = vorwissen.neue_identitaet(&sock.peer);
    let mut automat = zugangsphase::Automat::neu(
        &noetig,
        &sock.peer,
        &sock.handshake_hash,
        name,
        &sock.sas,
        neue_identitaet,
        Instant::now(),
    );
    let d = automat.dialog().clone();
    protokoll::zeile(format!(
        "Zugang noetig: {} ({addr}, ID {}, Vergleichscode {}) {} - {}{}{}",
        d.name,
        zugang::id_text(d.id),
        d.code,
        if sock.flags3 & NAME_FLAG_HOST_UNBEKANNT != 0 {
            "ist hier noch nicht gemerkt (Bit 0 in Nachricht 3: der Host weist sich aus)"
        } else {
            "kennt dieses Geraet noch nicht"
        },
        if d.zulassen { "Passwort oder Zulassen am Host" } else { "nur Passwort" },
        if noetig.warten_ms > 0 { format!(", Beweis fruehestens in {} ms", noetig.warten_ms) } else { String::new() },
        if neue_identitaet { ", neue Identitaet unter bekannter Adresse" } else { "" }
    ));
    let mut zuletzt: Option<(zugangsphase::Lage, u32)> = None;
    let ausgang = zugangsphase::fuehren(
        sock,
        &mut automat,
        |d| {
            // Protokoll: jedes "falsch" einmal (ohne das Passwort).
            if d.lage == zugangsphase::Lage::Falsch && zuletzt != Some((d.lage, d.runde)) {
                protokoll::zeile(format!(
                    "Zugang: Passwort falsch ({}. Mal){}",
                    d.runde,
                    match d.warten_s(Instant::now()) {
                        0 => String::new(),
                        s => format!(", naechster Versuch fruehestens in {s} s"),
                    }
                ));
            }
            zuletzt = Some((d.lage, d.runde));
            let mut s = shared.lock().unwrap();
            // Getrennt (Nutzer, anderes Ziel, Fenster geschlossen): wie Abbrechen.
            if s.target.as_deref() != Some(addr) {
                s.zugang = None;
                return Some(Eingabe::Abbrechen);
            }
            if s.zugang.as_ref() != Some(d) {
                s.zugang = Some(d.clone());
            }
            let e = s.zugang_eingabe.take();
            if matches!(e, Some(Eingabe::Passwort(_))) && d.lage != zugangsphase::Lage::Pruefen {
                protokoll::zeile(format!("Zugang: Passwort fuer {} eingegeben - Beweis an den Host", d.name));
            }
            e
        },
        zugangsphase::RUHE_FRIST,
    );
    shared.lock().unwrap().zugang = None;
    let name = automat.dialog().name.clone();
    match ausgang {
        Ausgang::Angenommen { per_passwort } => protokoll::zeile(format!(
            "Zugang: {name} hat dieses Geraet angenommen ({})",
            if per_passwort { "Passwort, Beweis des Hosts stimmt" } else { "Zulassen am Host" }
        )),
        Ausgang::Abgebrochen => {
            protokoll::zeile(format!("Zugang: vom Nutzer abgebrochen ({name})"));
            return Ok(None);
        }
        andere => return Err(zugang_meldung(andere, &name, addr)),
    }
    // Nach der Annahme folgt "QCH1", dann alles wie bisher.
    match zugangsphase::kennung_lesen(sock) {
        Ok(Kennung::Sitzung) => Ok(Some(name)),
        Ok(k) => Err(Meldung::neu(strings::Key::ErrorProtocol, format!("Zugang: nach der Annahme {k:?} statt QCH1")).bleibend()),
        Err(a) => Err(zugang_meldung(a, &name, addr)),
    }
}

/// Ein Ausgang der Zugangsphase als Meldung fuer den Startbildschirm
/// (Spezifikation 9.6); der deutsche Text mit Einzelheiten geht ins
/// Protokoll.
fn zugang_meldung(a: zugangsphase::Ausgang, name: &str, addr: &str) -> Meldung {
    use strings::Key::*;
    use zugangsphase::Ausgang as A;
    match a {
        A::Abgelehnt => Meldung::neu(MsgRefused, format!("Zugang: {name} ({addr}) hat abgelehnt")).mit("{n}", name),
        // Ergebnis 4 nach Fehlversuchen oder mit Wartezeit (auch: kein
        // Platz frei) heisst "zu viele Versuche"; ohne beides lief die Frist
        // des Hosts ab - niemand hat geantwortet.
        A::Schluss { warten_ms, fehlversuche } if warten_ms > 0 || fehlversuche > 0 => Meldung::neu(
            MsgTooManyAttempts,
            format!("Zugang: {name} ({addr}) beendet - zu viele Versuche ({fehlversuche} falsch, erneut fruehestens in {warten_ms} ms)"),
        ),
        A::Schluss { .. } => {
            Meldung::neu(MsgNoAnswer, format!("Zugang: {name} ({addr}) beendet - Frist abgelaufen")).mit("{n}", name)
        }
        A::HostBeweisFalsch => Meldung::neu(
            MsgHostProofBad,
            format!(
                "Zugang: {name} ({addr}) konnte das Passwort nicht bestaetigen (Beweis des Hosts falsch) - \
                 moeglicher Angriff, abgebrochen, nichts gemerkt"
            ),
        )
        .mit("{n}", name),
        A::KeineAntwort => Meldung::neu(MsgNoAnswer, format!("Zugang: keine Antwort von {name} ({addr})")).mit("{n}", name),
        A::Geschlossen => {
            Meldung::neu(MsgNoAnswer, format!("Zugang: {name} ({addr}) hat die Leitung geschlossen")).mit("{n}", name)
        }
        A::Leitung(g) => {
            Meldung::neu(MsgNoAnswer, format!("Zugang: Leitung zu {name} ({addr}) unterbrochen - {g}")).mit("{n}", name)
        }
        // Eine Nachricht, die nicht passt (falsche Fassung, unerwarteter
        // Typ): der naechste Versuch saehe dasselbe - und oeffnete am Host
        // nur eine neue Anfrage. Bleibt also (9.6).
        A::Protokoll(g) => Meldung::neu(ErrorProtocol, format!("{g} ({addr})")).bleibend(),
        A::Abgebrochen | A::Angenommen { .. } => {
            Meldung::neu(ErrorProtocol, format!("Zugang: unerwarteter Ausgang {a:?} ({addr})")).bleibend()
        }
    }
}

/// Der Abschied des Hosts (MSG_HOST_ENDE) als Meldung fuer den
/// Startbildschirm, mit seinem Namen; ein unbekannter Grund gilt wie
/// "beendet".
fn host_ende_meldung(grund: u8, name: &str, addr: &str) -> Meldung {
    use strings::Key::*;
    let (key, was) = match grund {
        HOST_ENDE_FREIGABE_AUS => (MsgHostSharingOff, "hat die Freigabe ausgeschaltet".to_string()),
        HOST_ENDE_ENTFERNT => (MsgHostRemovedYou, "hat dieses Geraet entfernt".to_string()),
        HOST_ENDE_BEENDET => (MsgHostQuit, "wurde beendet".to_string()),
        g => (MsgHostQuit, format!("endet (unbekannter Grund {g}, gilt wie beendet)")),
    };
    Meldung::neu(
        key,
        format!("Sitzung mit {name} ({addr}) zu Ende: der Host {was} (Abschied, Grund {grund}) - keine automatische Neuverbindung"),
    )
    .mit("{n}", name)
}

fn run_session(addr: &str, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) -> Result<(), Meldung> {
    // Erst der Handschlag, dann erst Nutzdaten. Vorher geht nichts ueber die
    // Leitung, was jemand mitlesen koennte.
    //
    // hosts.txt wird VOR dem Verbinden gelesen: ist sie unlesbar, geht keine
    // Leitung auf. Beim Verbinden ueber eine Geraete-ID prueft der Handschlag
    // nach Nachricht 2, dass der Schluessel des Hosts genau diese ID ergibt,
    // bevor Nachricht 3 den eigenen zeigt (Spezifikation Pairing v1, 8.2) -
    // der Host nimmt diesen Client erst mit Nachricht 3 an und loest dafuer
    // den laufenden Zuschauer ab. Ein anderes Geraet unter der ID kommt so
    // weit nicht. Ist der Schluessel des Hosts nicht gepinnt, traegt
    // Nachricht 3 Bit 0 (1.4): der Host muss sich in der Zugangsphase
    // ausweisen, auch wenn er dieses Geraet kennt.
    let (ziel_id, ziel_name, angenommen) = {
        let s = shared.lock().unwrap();
        (s.ziel_id, s.ziel_name.clone(), s.angenommen.clone())
    };
    let hosts_pfad = zugang::ablage_pfad(zugang::HOSTS_DATEI).map_err(|g| Meldung::from(secure::Fehler::Ablage(g)))?;
    let mut vorwissen =
        zugangsphase::Vorwissen::laden(&hosts_pfad, addr, ziel_id).map_err(|f| Meldung::from(secure::Fehler::from(f)))?;
    vorwissen.angenommen = angenommen;
    // Selbstschutz: antwortet unter der Adresse dieser Rechner selbst (der
    // Schluessel des Hosts ist unser eigener host.key), bricht der Client
    // nach Nachricht 2 ab, bevor Nachricht 3 ihn zeigt - "Das ist dieser
    // Computer.", kein Neuversuch.
    let eigen = secure::eigener_host_schluessel();
    let mut sock = secure::Secure::connect_pruefend(addr, &noise::prologue_video(), |k| {
        if eigen.as_deref() == Some(k) {
            return Err(secure::Fehler::EigenerHost { addr: addr.to_string() });
        }
        vorwissen.pruefen(k).map(|()| vorwissen.flags3(k))
    })?;
    shared.lock().unwrap().abbruch = sock.abbruchgriff();
    let fp = sock.peer_fingerprint();
    // Wie der Host in Meldungen und in hosts.txt heisst, bis er selbst einen
    // Namen nennt (Nachricht 20): aus der Bekanntgabe, sonst der gemerkte,
    // sonst die Adresse.
    let name = ziel_name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| vorwissen.bekannt(&sock.peer).map(|h| h.name.clone()))
        .unwrap_or_else(|| addr.to_string());
    {
        let mut s = shared.lock().unwrap();
        s.error = None;
        s.error_key = None;
        s.sas = Some(sock.sas.clone());
        s.peer_fp = Some(fp.clone());
    }
    // Kennt der Host dieses Geraet? "QCH1": ja, die Sitzung beginnt. "QCA1":
    // nein - Zugangsphase (Passwort oder "Zulassen"). Ein Host, der nach dem
    // Handschlag zumacht, ist eine aeltere Fassung, die dieses Geraet nicht
    // kennt; einer, der schweigt, antwortet nicht. Beides ist kein Netzfehler,
    // sondern eine Aussage - und wird nicht alle 2 s wiederholt.
    sock.lesefrist(Some(zugangsphase::KENNUNG_FRIST));
    let name = match zugangsphase::kennung_lesen(&mut sock) {
        // Bit 0 war gesetzt - dieser Client kennt den Schluessel des Hosts
        // nicht -, und der Host sagt trotzdem gleich "QCH1": er hat sich
        // nicht ausgewiesen, weder mit dem Beweis des Passworts (22/0) noch
        // ueber "Zulassen" mit dem Vergleichscode (22/1). Ein Geraet, das
        // sich nur als der Host ausgibt, kaeme so herein und wuerde
        // gemerkt. Also abbrechen, nichts pinnen, kein Neuversuch (9.6).
        Ok(zugangsphase::Kennung::Sitzung) if sock.flags3 & NAME_FLAG_HOST_UNBEKANNT != 0 => {
            return Err(Meldung::neu(
                strings::Key::MsgHostUnverified,
                format!(
                    "{addr}: {name} (ID {}, Fingerabdruck {fp}) antwortet mit QCH1, obwohl dieser Client seinen Schluessel \
                     nicht kennt (Bit 0 in Nachricht 3) - der Host hat sich nicht ausgewiesen (weder Passwortbeweis \
                     noch Zulassen); abgebrochen, nichts gemerkt",
                    zugang::id_text(zugang::geraete_id(&sock.peer))
                ),
            )
            .mit("{n}", name));
        }
        // Gepinnt (oder eben in dieser Wahl angenommen): die Sitzung beginnt.
        Ok(zugangsphase::Kennung::Sitzung) => name,
        // Wiederverbinden nach einer angenommenen Sitzung, und der Host
        // kennt dieses Geraet nicht mehr: keine Anfrage ohne den Nutzer
        // (siehe Shared::angenommen). Nachricht 23 zieht sie am Host gleich
        // zurueck; verbindet der Nutzer selbst neu, kommt der Dialog.
        Ok(zugangsphase::Kennung::Zugang) if shared.lock().unwrap().angenommen.is_some() => {
            let _ = sock.write_all(&zugang::Nachricht::Abbruch.kodieren());
            return Err(Meldung::neu(
                strings::Key::MsgDeviceRemoved,
                format!(
                    "Zugang: {name} ({addr}) kennt dieses Geraet nach der Sitzung nicht mehr (am Host entfernt?) - \
                     keine Anfrage ohne den Nutzer, zurueckgezogen"
                ),
            )
            .mit("{n}", name));
        }
        Ok(zugangsphase::Kennung::Zugang) => match zugang_durchlaufen(&mut sock, shared, &vorwissen, &name, addr)? {
            Some(n) => n,
            // Der Nutzer hat abgebrochen: zurueck zum Startbildschirm, ohne Meldung.
            None => return Ok(()),
        },
        // Nach einem Handschlag mit dem Prolog von QuadChroma etwas anderes
        // als QCH1/QCA1: eine Fassung, die dieser Client nicht versteht.
        // Das gibt sich mit dem naechsten Versuch nicht (9.6).
        Ok(zugangsphase::Kennung::Fremd(k)) => {
            return Err(Meldung::neu(
                strings::Key::ErrorProtocol,
                format!("Gegenstelle spricht ein anderes Protokoll (Kennung {k:02x?} statt QCH1/QCA1)"),
            )
            .bleibend())
        }
        Err(zugangsphase::Ausgang::KeineAntwort) => {
            return Err(Meldung::neu(
                strings::Key::MsgNoAnswer,
                format!("{addr}: keine Antwort nach dem Handschlag (Frist {} s)", zugangsphase::KENNUNG_FRIST.as_secs_f32()),
            )
            .mit("{n}", name))
        }
        // Sauber zugemacht (EOF) beim Wiederverbinden nach einer
        // angenommenen Sitzung (Shared::angenommen): dieser Host hat eben
        // noch "QCH1" gesagt - eine aeltere Fassung, die dieses Geraet nicht
        // kennt, ist das nicht. Eher ging gerade etwas nicht (Aufnahme
        // startet nicht, Verbindung verdraengt, Host startet neu): ein
        // Leitungsfehler, der naechste Versuch in 2 s darf. Kennt der Host
        // das Geraet nicht mehr, sagt er beim naechsten Mal "QCA1"
        // (MsgDeviceRemoved, oben).
        Err(zugangsphase::Ausgang::Geschlossen) if shared.lock().unwrap().angenommen.is_some() => {
            return Err(Meldung::neu(
                strings::Key::ConnectionLost,
                format!(
                    "{addr} hat die Leitung nach dem Handschlag geschlossen (weder QCH1 noch QCA1) - nach einer \
                     angenommenen Sitzung kein Zeichen einer aelteren Fassung, neuer Versuch"
                ),
            ));
        }
        // Sauber zugemacht (EOF): so antwortet eine aeltere Fassung einem
        // Geraet, das sie nicht kennt.
        Err(zugangsphase::Ausgang::Geschlossen) => {
            return Err(Meldung::neu(
                strings::Key::MsgHostOutdated,
                format!(
                    "{addr} hat die Leitung nach dem Handschlag geschlossen - aeltere QuadChroma-Fassung, \
                     die dieses Geraet nicht kennt (weder QCH1 noch QCA1)"
                ),
            )
            .mit("{n}", name))
        }
        // Zurueckgesetzt, Datensatz nicht echt, sonst gestoert: kein Beleg
        // fuer eine alte Fassung (etwa ein Host, der gerade neu startet) -
        // ein Leitungsfehler wie jeder andere, der naechste Versuch darf.
        Err(a) => {
            let grund = match a {
                zugangsphase::Ausgang::Leitung(g) => g,
                andere => format!("{andere:?}"),
            };
            return Err(Meldung::neu(strings::Key::ConnectionLost, format!("{addr}: Leitung nach dem Handschlag gestoert - {grund}")));
        }
    };
    sock.lesefrist(None);
    // Erst jetzt, nach der Annahme, wird der Host gepinnt (8.1) - mit ID,
    // Schluessel, Adresse und Namen. Scheitert das Schreiben, laeuft die
    // Sitzung trotzdem: der Host hat entschieden, der Eintrag ist nur das
    // Gedaechtnis dieses Geraets (Haken, Suche ueber die ID).
    match zugangsphase::pinnen(&hosts_pfad, &sock.peer, addr, &name) {
        Ok(true) => {
            protokoll::zeile(format!(
                "hosts.txt: {name} (ID {}) unter {addr} gemerkt",
                zugang::id_text(zugang::geraete_id(&sock.peer))
            ));
            shared.lock().unwrap().hosts_stand += 1;
        }
        Ok(false) => {}
        Err(e) => protokoll::zeile(format!("hosts.txt: {name} nicht gemerkt - {e}")),
    }
    // Dateien: je Sitzung eine Nummer, gegen die Sender und Empfaenger ihren
    // Stand melden; die Faehigkeit des Hosts gilt erst mit MSG_FAEHIGKEITEN
    // dieser Sitzung.
    let (sitzung, alt) = {
        let mut s = shared.lock().unwrap();
        s.connected = true;
        s.angenommen = Some(sock.peer.clone());
        s.error = None;
        s.error_key = None;
        // Die Bindung fuer den Eingabekanal erst jetzt: vor der Annahme
        // wiese ihn der Host ohnehin ab (und protokollierte jeden Versuch).
        s.link = Some((sock.handshake_hash.clone(), sock.peer.clone()));
        // Die Liste des vorigen Hosts hat hier nichts mehr zu suchen; die
        // neue kommt gleich nach dem Gruss. Ebenso seine Strominfo: bis die
        // neue da ist, zeigen Statistik und Wartebild sonst die Masse des
        // vorigen Hosts.
        s.codecs.clear();
        s.codec_idx = None;
        s.codec_wechsel = None;
        s.info = None;
        s.bildschirme_zuruecksetzen();
        s.decoder_baue = 0;
        s.sitzung_nr += 1;
        s.sitzung_seit = Some(Instant::now());
        s.host_zu_alt_gemeldet = false;
        s.datei_stand = DateiStaende::default();
        (s.sitzung_nr, s.dateien_zuruecksetzen())
    };
    drop(alt);
    // Ab hier raeumt jedes Ende der Sitzung die Dateiuebertragung ab.
    let _ende = SitzungsEnde(shared);
    let hh = sock.handshake_hash.clone();
    // Der Empfaenger fuer Dateien vom Host, erst beim ersten Angebot angelegt;
    // faellt er mit dieser Funktion weg, bricht er ab und loescht, was halb da ist.
    let mut empfaenger: Option<dateien::Empfaenger> = None;

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
    #[cfg(windows)]
    let aud_anhang = aud_gewuenscht();
    // Jedes H.264-SPS fuer NVDEC um ein VUI mit max_num_reorder_frames 0
    // ergaenzen, damit cuvids Parser keine Bilder fuer eine Umsortierung
    // zurueckhaelt, die es nicht gibt - siehe `sps.rs`. --ohne-vui laesst
    // es zum Vergleich weg. Das erste Umschreiben kommt ins Protokoll.
    #[cfg(windows)]
    let vui_anhang = vui_gewuenscht();
    #[cfg(windows)]
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
            // Auch ein anderes Ziel beendet diese Sitzung: ein zweiter Start
            // mit anderer Adresse (einzel.rs) trennt und verbindet sofort
            // neu - fiel das Trennen in den Handschlag, bevor der
            // Abbruchgriff stand, liefe die alte Sitzung sonst weiter.
            if s.target.as_deref() != Some(addr) {
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
        // Ist der Eingabekanal der laufenden Datei-Sendung verloren, bricht
        // sie hier ab - je Nachricht geprueft, und der Host schickt laufend
        // welche (Bilder, auch wiederholte, und Ton).
        datei_kanal_pruefen(shared, input);
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
                    // Eine andere Groesse mitten in der Sitzung: der Host hat
                    // den Bildschirm gewechselt (Spezifikation Bildschirmwahl
                    // 1.4 und 3.1). Dann wie nach Nachricht 7: Decoder neu und
                    // erst mit dem naechsten Vollbild weiter - die alten
                    // Parametersaetze taugen nicht mehr, und ein Hardware-
                    // Decoder, der drei Bilder in Folge nicht lesen kann,
                    // fiele sonst still auf Software zurueck. So ist der
                    // Client auch gegen einen Host robust, der nur MSG_INFO
                    // schickt und kein MSG_SWITCH davor.
                    let groesse_neu = info.is_some_and(|alt| (alt.width, alt.height) != (i.width, i.height));
                    if groesse_neu {
                        let alt = info.unwrap_or(i);
                        protokoll::zeile(format!(
                            "Strominfo: neue Groesse {}x{} (vorher {}x{}) - Decoder neu, warte auf Vollbild",
                            i.width, i.height, alt.width, alt.height
                        ));
                    }
                    // Der Bedarf gilt ab jetzt auch fuer spaetere Baue (Wechsel
                    // des Wunsches), ob jetzt neu gebaut wird oder nicht.
                    bedarf.h264 = h264;
                    bedarf.chroma444 = Some(i.chroma444);
                    bau.chroma444 = Some(i.chroma444);
                    if neubau || groesse_neu {
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
            MSG_HOST_ENDE => {
                // Der Host verabschiedet sich (App beendet, Freigabe aus,
                // dieses Geraet entfernt) und macht gleich zu. Wie bei der
                // Abloesung kein Neuversuch: das Ziel geht zurueck, der
                // Startbildschirm zeigt den Grund, verbunden wird erst wieder
                // auf Wunsch.
                let grund = payload.first().copied().unwrap_or(HOST_ENDE_BEENDET);
                let m = host_ende_meldung(grund, &name, addr);
                protokoll::zeile(m.protokoll.clone());
                let mut s = shared.lock().unwrap();
                if s.target.as_deref() == Some(addr) {
                    s.target = None;
                    s.error_key = None;
                    s.error = Some(m);
                }
                return Ok(());
            }
            MSG_CODECS => {
                let liste = codecs_parsen(&payload);
                shared.lock().unwrap().codecs = liste;
            }
            MSG_BILDSCHIRME => {
                // Die Bildschirme des Hosts samt seinem Wunsch (Spezifikation
                // Bildschirmwahl 2.2). Krumme Bytes oder eine fremde Fassung
                // werden uebergangen - die letzte gute Liste bleibt stehen.
                if let Some(liste) = bildschirm::bildschirme_lesen(&payload) {
                    protokoll::zeile(bildschirme_zeile(&liste));
                    let mut s = shared.lock().unwrap();
                    // Der Hinweis "wird gewechselt" endet, sobald der Host den
                    // Wunsch beantwortet hat: der gestreamte Eintrag passt zu
                    // ihm, oder er ist gar nicht angeschlossen (Ausweichplatz -
                    // mehr passiert erst, wenn er zurueckkommt).
                    if let Some((_, gewollt)) = &s.bildschirm_wechsel {
                        if *gewollt == liste.wunsch && bildschirm_wunsch_beantwortet(&liste) {
                            s.bildschirm_wechsel = None;
                        }
                    }
                    s.bildschirm_wunsch = liste.wunsch;
                    s.bildschirme = liste.eintraege;
                }
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
                #[cfg(windows)]
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
                #[cfg(windows)]
                {
                    packet.set_pts(Some(seq as i64));
                    packet.set_dts(None);
                }
                // VideoToolbox bekommt die Einheit, wie sie kam; der Umbau
                // (Parametersaetze, Laengenpraefix) steckt im Decoder.
                #[cfg(target_os = "macos")]
                let packet = vt_decoder::Paket { daten: &payload, pts: seq as i64 };
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
                let mut bilder: Vec<Dekoderbild> = Vec::new();
                for _anlauf in 0..2 {
                    bau.paket();
                    let vorher = bilder.len();
                    let fehler = decoder_fuettern(&mut bau.decoder, &packet, &mut bilder);
                    // D3D11VA: die Bilder liegen auf der Karte und muessen
                    // erst in den Hauptspeicher (Kopierstufe). Geht das
                    // nicht - oder rechnet der Decoder in Wahrheit auf der
                    // CPU, weil der Treiber das Profil nicht kann -, taugt
                    // er so wenig wie ein cuvid, der Fehler wirft.
                    #[cfg_attr(not(windows), allow(unused_mut))]
                    let mut holfehler: Option<String> = None;
                    #[cfg(windows)]
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
                let letztes_format = bilder.last().map(|b| b.format_name());
                // Zeichnet die Karte, bleibt das Bild roh; einmal je Paket
                // nachsehen reicht.
                let gpu = shared.lock().unwrap().gpu_pfad;
                for decoded in bilder.drain(..) {
                    let pts = decoded.zeitstempel();
                    let (w, h) = (decoded.breite(), decoded.hoehe());
                    // Das Format entscheidet der decodierte Frame selbst, nicht
                    // die Strominfo. Ein unbekanntes Format ergibt ein dunkles
                    // Bild und eine Meldung - nie einen Absturz.
                    // `bereit_us` ist die Client-Uhr bei der Ablage: ab hier
                    // zaehlt das Glied Anzeige, das der Fensterfaden misst.
                    let (bild, fehler) = if gpu && decoded.ebenen().is_some() {
                        (Bild::Roh { bild: decoded, bereit_us: client_us() }, None)
                    } else {
                        match to_rgb(&decoded) {
                            Ok(f) => (Bild::Rgb(Frame { bereit_us: client_us(), ..f }), None),
                            Err(e) => {
                                let m = Meldung::neu(strings::Key::ErrorPixelFormat, e).anhang(decoded.format_name());
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
                    let format = letztes_format.unwrap_or_default();
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
            MSG_FAEHIGKEITEN => {
                // Ein aelterer Client uebergeht Typ 11; hier gilt er fuer
                // diese Sitzung (Spezifikation 2.2).
                // Eine vorgemerkte Kopie holt die Weiterleitung der Ablage ab
                // (vorgemerkte_dateien) - nicht hier: dateien_senden fragt
                // das Dateisystem, und der Empfangsfaden darf nicht warten.
                if let Some(bits) = dateien::faehigkeiten_lesen(&payload) {
                    let kann = dateien::kann_dateien(bits);
                    // Bit 1: der Host kennt die Bildschirmwahl (Spezifikation
                    // Bildschirmwahl 2.1). Erst damit zeigt das Menue die
                    // Zeile, und erst damit geht ein Wunsch hinaus.
                    let bildschirmwahl = bits & FAEHIG_BILDSCHIRM != 0;
                    {
                        let mut s = shared.lock().unwrap();
                        s.host_dateien = kann;
                        s.host_bildschirmwahl = bildschirmwahl;
                        s.faehigkeiten_da = true;
                    }
                    protokoll::zeile(format!(
                        "Host meldet Faehigkeiten {bits:#x}: Dateien {}, Bildschirmwahl {}",
                        if kann { "ja" } else { "nein" },
                        if bildschirmwahl { "ja" } else { "nein" }
                    ));
                }
            }
            DATEI_ANGEBOT | DATEI_STUECK | DATEI_ENDE => {
                // Nur einreihen: geschrieben wird im Faden des Empfaengers,
                // nie hier (die Hosts werfen einen Zuschauer hinaus, der 2 s
                // nichts abnimmt). Der Puffer `payload` bleibt hier, der
                // Empfaenger bekommt eine Kopie.
                let e = empfaenger.get_or_insert_with(|| {
                    let ablage = shared.lock().unwrap().datei_ablage.clone().unwrap_or_else(DateiAblage::produktion);
                    let ablegen = ablage.ablegen;
                    dateien::Empfaenger::neu(
                        ablage.basis,
                        quittungs_weg(input, hh.clone()),
                        move |pfade| ablegen(pfade),
                        datei_melder(shared, sitzung),
                    )
                });
                e.nachricht(msg_type, payload.to_vec());
            }
            DATEI_QUITTUNG => {
                // An die laufende Sendung; Griff::quittung kehrt sofort
                // zurueck und uebergeht fremde Kennungen.
                if let Some(d) = shared.lock().unwrap().datei_senden.as_ref() {
                    d.quittung(&payload);
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

// ------------------------------------------------------ Dateien (Spezifikation 3.4)

/// Raeumt am Ende von run_session auf, auf jedem Weg hinaus (Fehler,
/// Trennen, Abloesung, Zielwechsel): eine laufende Datei-Sendung bricht ab,
/// die Faehigkeit des Hosts gilt nicht mehr. Der Empfaenger ist eine lokale
/// Variable von run_session und bricht mit ihr ab (Drop).
struct SitzungsEnde<'a>(&'a Arc<Mutex<Shared>>);

impl Drop for SitzungsEnde<'_> {
    fn drop(&mut self) {
        let griff = match self.0.lock() {
            Ok(mut s) => {
                // Die Bildschirmwahl gilt nur je Sitzung, wie die Dateien.
                s.bildschirme_zuruecksetzen();
                s.dateien_zuruecksetzen()
            }
            Err(_) => None,
        };
        // Ausserhalb der Sperre: Griff::drop bricht ab und kehrt sofort zurueck.
        drop(griff);
    }
}

/// Meldungen von Sender und Empfaenger einer Sitzung: die Zeile ins
/// Protokoll, der Stand in die Anzeige - nur, solange die Sitzung gilt.
/// Laeuft in deren Faeden, nie unter einer ihrer Sperren.
fn datei_melder(shared: &Arc<Mutex<Shared>>, sitzung: u64) -> impl Fn(dateien::Ereignis) + Send + 'static {
    let shared = shared.clone();
    move |e: dateien::Ereignis| {
        if let Some(z) = e.zeile {
            protokoll::zeile(z);
        }
        if let Some(st) = e.stand {
            if let Ok(mut s) = shared.lock() {
                if s.sitzung_nr == sitzung {
                    s.datei_stand.setzen(st);
                }
            }
        }
    }
}

/// Weg der Quittungen des Empfaengers (Host -> Client): DATEI_QUITTUNG ueber
/// das normale send des Eingabekanals, also mit Vorrang vor Dateien. Er
/// nimmt nur die Sperre des Eingabekanals, kurz, im Faden des Empfaengers -
/// nie `shared` darunter, nie im Empfangsfaden. Voll, solange der Kanal fuer
/// DIESE Sitzung nicht steht (Aufbau, Neuaufbau nach einem Fehler): der
/// Empfaenger versucht es nach VOLL_WARTEN mit dem neuesten Stand wieder.
fn quittungs_weg(input: &Arc<Mutex<InputLink>>, sitzung: Vec<u8>) -> Arc<dyn dateien::Weg> {
    let input = input.clone();
    Arc::new(move |typ: u8, n: &[u8]| {
        let Ok(mut l) = input.lock() else { return dateien::Gesendet::Weg };
        if l.sitzung() != Some(sitzung.as_slice()) {
            return dateien::Gesendet::Voll;
        }
        if l.einreihen(typ, n) {
            dateien::Gesendet::Ja
        } else {
            dateien::Gesendet::Voll
        }
    })
}

/// Weg des Senders Client -> Host: der Eingabekanal mit Nachrang
/// (InputLink::datei_senden). Gebunden an die Sitzung und an den Kanal, auf
/// dem das erste Paket hinausging (`kanal`, siehe DateiSendung): wechselt
/// eins davon, gilt Weg - auf einem neuen Kanal kaeme der Rest ohne sein
/// Angebot an (Spezifikation 2.7 Schritt 5, Verlust des Eingabekanals).
/// Ausgenommen ist das Ende eines Abbruchs (Grund ungleich 0): es geht auch
/// auf den neuen Kanal derselben Sitzung, damit der Empfaenger des Hosts
/// gleich verwirft, statt STILLSTAND abzuwarten ("sofern die Leitung
/// steht"); kennt er die Kennung nicht mehr, uebergeht er es. Steht gerade
/// kein Kanal (Neuaufbau), gilt fuer dieses Ende Voll - der Sender versucht
/// es hoechstens ENDE_FRIST lang, und jeder Versuch treibt den Aufbau.
///
/// Anlauf (Integrationstest 574cd3e, Befund 3): Solange noch kein Paket
/// hinaus ist und die Sendung juenger als DATEI_ANLAUF ist, heisst ein
/// Eingabekanal, der noch nicht steht (im Aufbau, oder im Pruefmodus noch
/// nicht an diese Sitzung gebunden), Voll statt Weg - direkt nach dem
/// Verbinden brach eine Kopie sonst mit "Verbindung weg" ab. Jeder Versuch
/// treibt den Aufbau.
fn datei_weg(
    input: &Arc<Mutex<InputLink>>,
    sitzung: Vec<u8>,
    kanal: Arc<std::sync::atomic::AtomicU64>,
) -> Arc<dyn dateien::Weg> {
    datei_weg_bis(input, sitzung, kanal, Instant::now() + DATEI_ANLAUF)
}

/// datei_weg mit dem Ende des Anlaufs (die Tests setzen es selbst).
fn datei_weg_bis(
    input: &Arc<Mutex<InputLink>>,
    sitzung: Vec<u8>,
    kanal: Arc<std::sync::atomic::AtomicU64>,
    anlauf_bis: Instant,
) -> Arc<dyn dateien::Weg> {
    use std::sync::atomic::Ordering;
    let input = input.clone();
    Arc::new(move |typ: u8, n: &[u8]| {
        let Ok(mut l) = input.lock() else { return dateien::Gesendet::Weg };
        let gebunden = kanal.load(Ordering::SeqCst);
        let im_anlauf = gebunden == 0 && Instant::now() < anlauf_bis;
        match l.sitzung() {
            Some(s) if s == sitzung.as_slice() => {}
            None if im_anlauf => return dateien::Gesendet::Voll,
            _ => return dateien::Gesendet::Weg,
        }
        l.ensure();
        let abbruch_ende =
            typ == DATEI_ENDE && dateien::Ende::lesen(n).is_some_and(|e| e.grund != dateien::GRUND_VOLLSTAENDIG);
        match l.kanal_nr() {
            Some(nr) if gebunden == 0 || gebunden == nr => {
                let r = l.datei_senden(typ, n);
                if r == dateien::Gesendet::Ja {
                    kanal.store(nr, Ordering::SeqCst);
                }
                r
            }
            Some(_) if abbruch_ende => l.datei_senden(typ, n),
            None if abbruch_ende || im_anlauf => dateien::Gesendet::Voll,
            _ => dateien::Gesendet::Weg,
        }
    })
}

/// Verlust des Eingabekanals (Spezifikation 2.7 Schritt 5; Pflicht der Rolle
/// laut dateien.rs): steht der Kanal nicht mehr, auf dem die laufende
/// Sendung ihr erstes Paket schickte (Schreibfehler, Trennen, neuer Kanal),
/// bricht sie sofort ab. Ihr Sender merkte es sonst erst beim naechsten
/// Paket - und wartet er gerade auf Quittungen, die ueber den toten Kanal
/// nie kommen, erst nach STILLSTAND (30 s). Sein Ende 1 geht auf dem neuen
/// Kanal hinaus, sobald der steht (datei_weg). Die Sperren nacheinander,
/// nie verschachtelt; `input` nur mit try_lock (Aufruf je Nachricht in
/// run_session) - ist sie belegt, prueft der naechste Aufruf. Eine Sendung,
/// deren Faden schon zu Ende ist (quittiert oder abgebrochen), bleibt
/// unberuehrt: sie ist nicht mehr abzubrechen, und eine Protokollzeile
/// "abgebrochen" fuer eine gelungene Uebertragung fuehrte in die Irre.
fn datei_kanal_pruefen(shared: &Mutex<Shared>, input: &Mutex<InputLink>) {
    let Some((kennung, gebunden)) = shared.lock().ok().and_then(|s| {
        s.datei_senden.as_ref().filter(|d| d.griff.laeuft()).and_then(|d| Some((d.kennung(), d.gebunden()?)))
    }) else {
        return;
    };
    let jetzt = match input.try_lock() {
        Ok(l) => l.kanal_nr(),
        Err(_) => return,
    };
    if jetzt == Some(gebunden) {
        return;
    }
    let alt = shared.lock().ok().and_then(|mut s| {
        if s.datei_senden.as_ref().is_some_and(|d| d.kennung() == kennung) {
            s.datei_senden.take()
        } else {
            None
        }
    });
    if alt.is_some() {
        protokoll::zeile(format!("Dateien: Eingabekanal {gebunden} verloren - Sendung {kennung} abgebrochen"));
    }
    // Ausserhalb der Sperre: Griff::drop bricht ab und kehrt sofort zurueck.
    drop(alt);
}

/// Kopierter Text (Waechter): eine laufende Datei-Sendung bricht ab (neuer
/// Inhalt, Spezifikation 2.7 Schritt 5), eine vorgemerkte Kopie faellt weg,
/// der Text geht wie bisher als IN_CLIP.
fn text_senden(text: &str, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
    let alt = shared.lock().ok().and_then(|mut s| {
        s.datei_vorgemerkt = None;
        s.datei_senden.take()
    });
    drop(alt);
    if let Ok(mut l) = input.lock() {
        l.send(IN_CLIP, text.as_bytes());
    }
}

/// Kopierte Dateien (Waechter, Spezifikation 2.7 und 3.4). Stammen sie aus
/// einem Empfang, geht nichts hinaus und nichts bricht ab (Widerhall). Sonst
/// endet eine laufende Sendung; kann der Host Dateien, startet eine neue,
/// sonst steht einmal je Sitzung FilesPeerOld da. Direkt nach dem Verbinden,
/// solange MSG_FAEHIGKEITEN noch aussteht (Shared::faehigkeiten_offen), wird
/// die Liste vorgemerkt; vorgemerkte_dateien schickt sie los, sobald die
/// Faehigkeiten da sind, oder meldet nach DATEI_ANLAUF FilesPeerOld. Laeuft
/// im Faden der Weiterleitung, nie im Faden des Waechters; Sender::starten
/// kehrt sofort zurueck, Auflisten und Lesen laufen in dessen eigenem Faden.
/// Der Griff liegt in `shared`, bevor die Sperre faellt - eine Quittung des
/// Hosts, die run_session gleich danach bekommt, findet ihn also schon. Unter
/// `shared` wird keine andere Sperre genommen.
fn dateien_senden(pfade: Vec<std::path::PathBuf>, shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
    let basis = shared
        .lock()
        .ok()
        .and_then(|s| s.datei_ablage.as_ref().map(|a| a.basis.clone()))
        .unwrap_or_else(dateien::ablage_basis);
    if dateien::aus_eigener_ablage(&pfade, &basis) {
        protokoll::zeile("Dateien: nicht gesendet, sie stammen aus einem Empfang".into());
        return;
    }
    let Ok(mut s) = shared.lock() else { return };
    // Neuer Inhalt: eine vorgemerkte Kopie gilt nicht mehr.
    s.datei_vorgemerkt = None;
    if s.faehigkeiten_offen() {
        let alt = s.datei_senden.take();
        let n = pfade.len();
        let sitzung = s.sitzung_nr;
        s.datei_vorgemerkt = Some(DateiVormerkung { pfade, bis: Instant::now() + DATEI_ANLAUF, sitzung });
        drop(s);
        drop(alt);
        protokoll::zeile(format!(
            "Dateien: {n} Eintraege vorgemerkt - die Faehigkeiten des Hosts stehen noch aus (hoechstens {} s)",
            DATEI_ANLAUF.as_secs()
        ));
        return;
    }
    let mut alt = s.datei_senden.take();
    let mut zu_alt = false;
    match (s.link.as_ref().map(|(hh, _)| hh.clone()), s.host_dateien) {
        (Some(hh), true) => {
            let melden = datei_melder(shared, s.sitzung_nr);
            let kanal = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let weg = datei_weg(input, hh, kanal.clone());
            // Das Fenster fragt der Sender vor jedem Stueck, so wirkt ein
            // Wechsel in den Spielmodus waehrend der Uebertragung. Nur
            // try_lock: der Faden des Senders wartet nie auf `shared`
            // (run_session haelt es, waehrend es Griff::quittung ruft); ist
            // es belegt, gilt der zuletzt gesehene Stand.
            let zuletzt = std::sync::atomic::AtomicU64::new(dateien::fenster(s.settings.is_some_and(|x| x.2)));
            let spiel = Arc::downgrade(shared);
            let fenster = move || {
                use std::sync::atomic::Ordering::Relaxed;
                if let Some(m) = spiel.upgrade() {
                    if let Ok(s) = m.try_lock() {
                        zuletzt.store(dateien::fenster(s.settings.is_some_and(|x| x.2)), Relaxed);
                    }
                }
                zuletzt.load(Relaxed)
            };
            // Die vorige Sendung bricht ab; die neue wartet in ihrem eigenen
            // Faden auf deren Ende 1, bevor ihr Angebot hinausgeht.
            let vorgaenger = alt.take().map(|d| d.griff);
            let griff =
                dateien::Sender::starten_nach(vorgaenger, pfade, weg, fenster, melden, dateien::Vorgaben::default());
            s.datei_senden = Some(DateiSendung { griff, kanal });
        }
        // Der Host hat in dieser Sitzung keine Dateien gemeldet (aelterer
        // Stand): nichts senden - er trennte sonst den Eingabekanal -, und
        // einmal je Sitzung sagen, warum.
        (Some(_), false) if !s.host_zu_alt_gemeldet => {
            s.host_zu_alt_gemeldet = true;
            s.datei_stand.setzen(dateien::Stand::gegenseite_zu_alt(dateien::Richtung::Senden));
            zu_alt = true;
        }
        _ => {}
    }
    drop(s);
    // In den anderen Zweigen bricht die vorige Sendung hier ab (ausserhalb der
    // Sperre; kehrt sofort zurueck). Im ersten hat starten_nach sie schon
    // unter `shared` abgebrochen - unschaedlich, Griff::abbrechen nimmt nur
    // die Blattsperre des Griffs.
    drop(alt);
    if zu_alt {
        protokoll::zeile(dateien::ZEILE_HOST_ZU_ALT.into());
    }
}

/// Eine vorgemerkte Kopie (dateien_senden) weiterreichen: sind die
/// Faehigkeiten des Hosts da oder ist DATEI_ANLAUF um, geht sie wieder durch
/// dateien_senden - dann startet der Sender bzw. steht FilesPeerOld da (nach
/// dem Anlauf gilt der Host als aelter). Gehoert sie zu einer vergangenen
/// Sitzung, faellt sie weg. Laeuft im Faden der Weiterleitung, im kurzen Takt,
/// solange etwas vorgemerkt ist (VORMERK_TAKT).
fn vorgemerkte_dateien(shared: &Arc<Mutex<Shared>>, input: &Arc<Mutex<InputLink>>) {
    let pfade = {
        let Ok(mut s) = shared.lock() else { return };
        let Some(v) = s.datei_vorgemerkt.as_ref() else { return };
        if v.sitzung != s.sitzung_nr || s.link.is_none() {
            s.datei_vorgemerkt = None;
            return;
        }
        if !s.faehigkeiten_da && Instant::now() < v.bis {
            return;
        }
        s.datei_vorgemerkt.take().map(|v| v.pfade)
    };
    if let Some(p) = pfade {
        dateien_senden(p, shared, input);
    }
}

/// So oft sieht die Weiterleitung nach einer vorgemerkten Kopie.
const VORMERK_TAKT: Duration = Duration::from_millis(20);

/// Liegt eine Kopie vorgemerkt? Dann wartet die Weiterleitung nur
/// VORMERK_TAKT auf den naechsten Inhalt.
fn vormerk_takt(shared: &Mutex<Shared>) -> Option<Duration> {
    shared.lock().ok().and_then(|s| s.datei_vorgemerkt.as_ref().map(|_| VORMERK_TAKT))
}

/// Ein Paket in den Decoder und alle fertigen Bilder heraus. Gibt den
/// ersten Fehler zurueck, der kein blosses "gerade nichts da" ist - der
/// Aufrufer entscheidet, ob das den Decoder disqualifiziert.
///
/// Ein EAGAIN beim Senden heisst bei den cuvid-Decodern: erst Bilder
/// abholen, dann noch einmal senden. Das Paket geht dabei nicht verloren.
#[cfg(windows)]
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

/// Dasselbe mit VideoToolbox: synchron, jedes Paket liefert sein Bild sofort
/// (siehe vt_decoder.rs).
#[cfg(target_os = "macos")]
fn decoder_fuettern(decoder: &mut Dekoder, packet: &vt_decoder::Paket, bilder: &mut Vec<Dekoderbild>) -> Option<DekoderFehler> {
    decoder.fuettern(packet.daten, packet.pts, bilder).err()
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
/// VideoToolbox liefert auch 4:4:4 mit Paaren (xf44, 444f: SUB=false,
/// PAAR=true) - dieselbe Schleife, nur ohne Halbierung der Farbspalte.
///
/// Bei 4:2:0 wird der naechstgelegene Farbwert genommen (Wiederholung) -
/// einfach und schnell. Eine weichere Farbaufwertung ist eine spaetere
/// Verfeinerung; sie aendert am Vergleich 4:4:4 gegen 4:2:0 nichts Wesentliches.
///
/// BEGRENZT = begrenzter Wertebereich (Y 16..235, Cb/Cr 16..240): die acht
/// Anzeigebits werden vor der Matrix auf den vollen Bereich gedehnt, in
/// 16.16 mit Rundung (Y: (y - 16) * 255/219, Cb/Cr: c * 255/224 um den
/// Nullpunkt). Das kommt nur aus VideoToolbox (x444, 444v, x420, 420v -
/// wenn die Formatbeschreibung keinen vollen Bereich meldet); die Formate
/// von FFmpeg sind alle voll. Die Metal-Anzeige (anzeige_mac.rs) rechnet
/// dieselben Schritte mit denselben Konstanten.
#[inline(always)]
fn zeile_rgb<const SUB: bool, const BITS: u8, const PAAR: bool, const BEGRENZT: bool>(out: &mut [u32], yr: &[u8], ur: &[u8], vr: &[u8]) {
    const CR_R: i32 = 103206; // 1.5748
    const CB_G: i32 = 12276;  // 0.1873
    const CR_G: i32 = 30681;  // 0.4681
    const CB_B: i32 = 121609; // 1.8556
    const Y_DEHNEN: i32 = 76309; // 255/219
    const C_DEHNEN: i32 = 74606; // 255/224

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
        let mut y = wert::<BITS>(yr, x);
        let mut cb = wert::<BITS>(ur, ci) - 128;
        let mut cr = wert::<BITS>(vr, ci) - 128;
        if BEGRENZT {
            y = ((y - 16) * Y_DEHNEN + 32768) >> 16;
            cb = (cb * C_DEHNEN + 32768) >> 16;
            cr = (cr * C_DEHNEN + 32768) >> 16;
        }
        let r = y + ((CR_R * cr) >> 16);
        let g = y - ((CB_G * cb + CR_G * cr) >> 16);
        let b = y + ((CB_B * cb) >> 16);
        *o = (clamp8(r) << 16) | (clamp8(g) << 8) | clamp8(b);
    }
}

/// Aufbau der Ebenen eines Decoderformats. EINE Tabelle fuer `to_rgb` und -
/// sobald die Karte umrechnet - fuer deren Texturen, damit beide dasselbe
/// Format auf dieselbe Weise lesen. Unter Windows aus FFmpegs Pixelformat
/// (`ebenen_format`), auf dem Mac aus dem Format des CVPixelBuffers
/// (`vt_decoder::ebenen`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EbenenFormat {
    /// 4:2:0: je zwei Bildpunkte in beiden Richtungen teilen sich einen Farbwert.
    pub sub: bool,
    /// Breite der Werte: 8 (ein Byte), 10 (16 Bit LE, Wert in den unteren
    /// zehn Bit) oder 16 (16 Bit LE, Wert oben buendig).
    pub bits: u8,
    /// U und V als Paare in EINER Ebene (NV12, P010, P012, P016; auf dem
    /// Mac alle Formate von VideoToolbox, auch 4:4:4).
    pub paar: bool,
    /// Begrenzter Wertebereich (Y 16..235): nur die Formate von VideoToolbox
    /// ohne "f" (x444, 444v, x420, 420v); FFmpegs Formate sind alle voll.
    pub begrenzt: bool,
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
#[cfg(windows)]
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
    Some(EbenenFormat { sub, bits, paar, begrenzt: false })
}

/// Decodiertes Bild nach RGB. Das Format kommt aus dem Frame selbst
/// (Pixelformat des Decoders), nicht aus einer Flagge: nach einem
/// Codecwechsel waere jede Flagge fuer ein paar Bilder falsch, und der
/// Hardware-Decoder liefert andere Formate als der Software-Decoder.
///
/// Welche Formate gelesen werden, sagt `Ebenenbild::ebenen`; alles andere
/// ist ein Fehler mit Meldung, kein Absturz. Die Stroeme der Hosts sind
/// Vollbereich (der Host garantiert das); gedehnt wird nur, wenn
/// VideoToolbox ein Format im begrenzten Bereich liefert (`begrenzt`).
fn to_rgb(src: &impl Ebenenbild) -> Result<Frame, String> {
    let Some(fmt) = src.ebenen() else {
        return Err(format!("Unbekanntes Bildformat vom Decoder: {}", src.format_name()));
    };
    let (sub, bits, paar, begrenzt) = (fmt.sub, fmt.bits, fmt.paar, fmt.begrenzt);
    let w = src.breite() as usize;
    let h = src.hoehe() as usize;
    if w == 0 || h == 0 {
        return Err("Decoder liefert ein leeres Bild".into());
    }
    if src.ebenenzahl() < if paar { 2 } else { 3 } {
        return Err("Decoder liefert zu wenige Bildebenen".into());
    }
    // Breite der Farbebenen: bei 4:2:0 die Haelfte, aufgerundet. Bei Paaren
    // liegen U und V nebeneinander, die Zeile ist also doppelt so breit.
    let cw = if sub { (w + 1) / 2 } else { w };
    let ch = if sub { (h + 1) / 2 } else { h };
    let bpp = fmt.bpp() as usize;
    let cbreite = if paar { cw * 2 * bpp } else { cw * bpp };
    let yp = src.daten(0);
    let up = src.daten(1);
    let vp = if paar { up } else { src.daten(2) };
    let ys = src.zeilenlaenge(0);
    let us = src.zeilenlaenge(1);
    let vs = if paar { us } else { src.zeilenlaenge(2) };
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
        match (sub, bits, paar, begrenzt) {
            (false, 8, false, false) => zeile_rgb::<false, 8, false, false>(out, yr, ur, vr),
            (false, 10, false, false) => zeile_rgb::<false, 10, false, false>(out, yr, ur, vr),
            (false, _, false, false) => zeile_rgb::<false, 16, false, false>(out, yr, ur, vr),
            (true, 8, false, false) => zeile_rgb::<true, 8, false, false>(out, yr, ur, vr),
            (true, 10, false, false) => zeile_rgb::<true, 10, false, false>(out, yr, ur, vr),
            (true, 8, true, false) => zeile_rgb::<true, 8, true, false>(out, yr, ur, vr),
            (true, _, true, false) => zeile_rgb::<true, 16, true, false>(out, yr, ur, vr),
            // 4:4:4 mit Paaren: nur VideoToolbox (444f, xf44).
            (false, 8, true, false) => zeile_rgb::<false, 8, true, false>(out, yr, ur, vr),
            (false, _, true, false) => zeile_rgb::<false, 16, true, false>(out, yr, ur, vr),
            // Begrenzter Bereich: nur VideoToolbox, immer mit Paaren
            // (444v, x444, 420v, x420).
            (false, 8, true, true) => zeile_rgb::<false, 8, true, true>(out, yr, ur, vr),
            (false, _, true, true) => zeile_rgb::<false, 16, true, true>(out, yr, ur, vr),
            (true, 8, true, true) => zeile_rgb::<true, 8, true, true>(out, yr, ur, vr),
            (true, _, true, true) => zeile_rgb::<true, 16, true, true>(out, yr, ur, vr),
            // 4:2:0 planar 16 Bit und planar im begrenzten Bereich erzeugt
            // keine der Tabellen; der Arm steht nur fuer die Vollstaendigkeit.
            _ => {}
        }
    });

    Ok(Frame { width: w as u32, height: h as u32, pixels, bereit_us: 0 })
}

/// Was `to_rgb` und die Protokollzeilen von einem decodierten Bild brauchen:
/// Masse, Bildnummer, Ebenenaufbau und die Ebenen selbst - unter Windows
/// von FFmpegs Frame, auf dem Mac vom CVPixelBuffer aus VideoToolbox.
trait Ebenenbild {
    fn breite(&self) -> u32;
    fn hoehe(&self) -> u32;
    /// Die Bildnummer, die mit dem Paket hineinging.
    fn zeitstempel(&self) -> Option<i64>;
    /// Der Aufbau der Ebenen; None fuer ein Format, das der Client nicht liest.
    fn ebenen(&self) -> Option<EbenenFormat>;
    /// Das Format, wie der Decoder es nennt, fuer Meldungen und Protokoll.
    fn format_name(&self) -> String;
    fn ebenenzahl(&self) -> usize;
    /// Die Bytes einer Ebene (Zeilen im Abstand `zeilenlaenge`).
    fn daten(&self, ebene: usize) -> &[u8];
    fn zeilenlaenge(&self, ebene: usize) -> usize;
}

#[cfg(windows)]
impl Ebenenbild for ffmpeg::frame::Video {
    fn breite(&self) -> u32 {
        self.width()
    }
    fn hoehe(&self) -> u32 {
        self.height()
    }
    fn zeitstempel(&self) -> Option<i64> {
        self.pts()
    }
    fn ebenen(&self) -> Option<EbenenFormat> {
        ebenen_format(self.format())
    }
    fn format_name(&self) -> String {
        format!("{:?}", self.format())
    }
    fn ebenenzahl(&self) -> usize {
        self.planes()
    }
    fn daten(&self, ebene: usize) -> &[u8] {
        self.data(ebene)
    }
    fn zeilenlaenge(&self, ebene: usize) -> usize {
        self.stride(ebene)
    }
}

#[cfg(target_os = "macos")]
impl Ebenenbild for vt_decoder::Bild {
    fn breite(&self) -> u32 {
        vt_decoder::Bild::breite(self)
    }
    fn hoehe(&self) -> u32 {
        vt_decoder::Bild::hoehe(self)
    }
    fn zeitstempel(&self) -> Option<i64> {
        Some(self.pts())
    }
    fn ebenen(&self) -> Option<EbenenFormat> {
        vt_decoder::ebenen(self.format())
    }
    fn format_name(&self) -> String {
        vt_decoder::fourcc_text(self.format())
    }
    fn ebenenzahl(&self) -> usize {
        vt_decoder::Bild::ebenenzahl(self)
    }
    fn daten(&self, ebene: usize) -> &[u8] {
        vt_decoder::Bild::daten(self, ebene)
    }
    fn zeilenlaenge(&self, ebene: usize) -> usize {
        vt_decoder::Bild::zeilenlaenge(self, ebene)
    }
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
    /// Zustandsnachrichten (siehe NACHREICHEN), die ohne stehenden Kanal
    /// kamen - je Art die letzte, fertig gerahmt. Sie gehen als erste an den
    /// naechsten Kanal dieser Sitzung.
    nachreichen: Vec<Vec<u8>>,
    /// Frist fuer den Schreibfaden, siehe SCHREIBFRIST (Tests kuerzen sie).
    schreibfrist: Duration,
    /// Laufende Nummer des stehenden Kanals, je neuem Kanal eins mehr. Der
    /// Datei-Sender bindet sich daran: auf einem neuen Kanal ginge der Rest
    /// einer Uebertragung ohne ihr Angebot hinaus (Spezifikation 2.7 Schritt
    /// 5, Verlust des Eingabekanals).
    kanal_nr: u64,
}

/// Ergebnis eines Aufbaus: die Leitung oder der Fehler, dazu der
/// Fingerabdruck, falls am Eingabeport ein fremder Schluessel antwortete.
type Aufbau = (Result<secure::Secure, secure::Fehler>, Option<String>);

/// Nachrichten, die einen Zustand setzen statt ein Ereignis zu melden:
/// Faehigkeiten, Einstellungen, Codec, Testbild, Zwischenablage,
/// Bildschirmwunsch. Steht der
/// Kanal gerade nicht (Aufbau im Hintergrund, Schreibfaden gescheitert),
/// wird je Art die letzte gemerkt und nachgereicht, sobald er steht -
/// frueher baute das naechste `send` den Kanal selbst auf und lieferte sie
/// so aus. Maus, Tasten und Zeitfragen nicht: eine alte Lage oder Frage ist
/// wertlos (Latenz vor Bandbreite), und gedrueckte Tasten und Maustasten
/// gibt jeder Host selbst frei, wenn ein Eingabekanal endet oder ersetzt
/// wird (host/main.m alle_tasten_loslassen, host/netz.rs eingabe_binden und
/// Ende der Eingabeschleife) - ein verlorenes Loslassen laesst also nichts
/// haengen. IN_FAEHIGKEITEN geht ohnehin als erste Nachricht auf jedem neu
/// stehenden Kanal hinaus (siehe ensure); gemerkt wird sie trotzdem, falls
/// sie jemand ohne Kanal sendet.
const NACHREICHEN: [u8; 6] = [IN_FAEHIGKEITEN, IN_SETTINGS, IN_CODEC, IN_TESTBILD, IN_CLIP, IN_BILDSCHIRM];

/// Was dieser Client kann (Spezifikation 2.2), als erste Nachricht auf
/// jedem Eingabekanal: Dateien Fassung 1. Ein aelterer Host uebergeht sie
/// (hoechstens 256 Byte); ein neuerer schickt DATEI_* nur an einen
/// Eingabekanal, der sie gemeldet hat.
fn faehigkeiten_rahmen() -> Vec<u8> {
    eingabe_rahmen(IN_FAEHIGKEITEN, &dateien::faehigkeiten_kodieren(FAEHIG_DATEIEN))
}

/// Sendepuffer des Eingabekanals im Kernel. Klein, damit dort hoechstens
/// 64 KiB Dateidaten vor einer Taste liegen (Nachrang, Spezifikation 3.4);
/// fuer Tasten und Maus reicht er um Groessenordnungen.
const EINGABE_SNDBUF: i32 = 64 * 1024;

fn eingabe_sendepuffer(s: &std::net::TcpStream) {
    sendepuffer_setzen(s, EINGABE_SNDBUF);
}

#[cfg(windows)]
fn sendepuffer_setzen(s: &std::net::TcpStream, groesse: i32) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_SNDBUF};
    let wert = groesse.to_ne_bytes();
    // SAFETY: gueltiger Socket (lebt mit `s`), der Wert ist ein i32 wie verlangt.
    unsafe {
        setsockopt(SOCKET(s.as_raw_socket() as usize), SOL_SOCKET, SO_SNDBUF, Some(&wert));
    }
}

#[cfg(target_os = "macos")]
fn sendepuffer_setzen(s: &std::net::TcpStream, groesse: i32) {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn setsockopt(
            fd: std::os::raw::c_int,
            ebene: std::os::raw::c_int,
            name: std::os::raw::c_int,
            wert: *const std::os::raw::c_void,
            laenge: u32,
        ) -> std::os::raw::c_int;
    }
    // sys/socket.h (macOS): SOL_SOCKET 0xffff, SO_SNDBUF 0x1001
    const SOL_SOCKET: std::os::raw::c_int = 0xffff;
    const SO_SNDBUF: std::os::raw::c_int = 0x1001;
    let wert: std::os::raw::c_int = groesse;
    // SAFETY: gueltiger Deskriptor (lebt mit `s`), Zeiger und Laenge eines c_int.
    unsafe {
        setsockopt(
            s.as_raw_fd(),
            SOL_SOCKET,
            SO_SNDBUF,
            &wert as *const std::os::raw::c_int as *const std::os::raw::c_void,
            std::mem::size_of::<std::os::raw::c_int>() as u32,
        );
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn sendepuffer_setzen(_s: &std::net::TcpStream, _groesse: i32) {}

/// SO_SNDBUF eines Sockets zuruecklesen (nur fuer den Test).
#[cfg(all(test, windows))]
fn sendepuffer_lesen(s: &std::net::TcpStream) -> Option<i32> {
    use std::os::windows::io::AsRawSocket;
    use windows::core::PSTR;
    use windows::Win32::Networking::WinSock::{getsockopt, SOCKET, SOL_SOCKET, SO_SNDBUF};
    let mut wert = [0u8; 4];
    let mut laenge = 4i32;
    // SAFETY: gueltiger Socket, Puffer und Laenge passen zueinander.
    let r = unsafe {
        getsockopt(SOCKET(s.as_raw_socket() as usize), SOL_SOCKET, SO_SNDBUF, PSTR(wert.as_mut_ptr()), &mut laenge)
    };
    (r == 0).then(|| i32::from_ne_bytes(wert))
}

#[cfg(all(test, target_os = "macos"))]
fn sendepuffer_lesen(s: &std::net::TcpStream) -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn getsockopt(
            fd: std::os::raw::c_int,
            ebene: std::os::raw::c_int,
            name: std::os::raw::c_int,
            wert: *mut std::os::raw::c_void,
            laenge: *mut u32,
        ) -> std::os::raw::c_int;
    }
    let mut wert: std::os::raw::c_int = 0;
    let mut laenge = std::mem::size_of::<std::os::raw::c_int>() as u32;
    // SAFETY: gueltiger Deskriptor, Zeiger und Laenge eines c_int.
    let r = unsafe {
        getsockopt(s.as_raw_fd(), 0xffff, 0x1001, &mut wert as *mut std::os::raw::c_int as *mut std::os::raw::c_void, &mut laenge)
    };
    (r == 0).then_some(wert)
}

/// So lange darf ein Schreibaufruf des Eingabekanals ohne Fortschritt
/// haengen, dann gilt der Kanal als kaputt und wird neu aufgebaut. Ohne
/// Frist hinge der Schreibfaden an einer Leitung, die stockt, ohne dass der
/// Host sie schliesst, bis TCP aufgibt (Minuten), und alles dahinter
/// wartete mit. Geschrieben wird in Stuecken zu hoechstens einem
/// Noise-Datensatz (64 KB) - die Frist heisst also "kein Fortschritt", nicht
/// "4 MB Zwischenablage in 5 s".
const SCHREIBFRIST: Duration = Duration::from_secs(5);

/// So viele eingereihte Nachrichten nimmt der Schreibfaden hoechstens auf
/// einmal, um darin Mausbewegungen zusammenzufassen.
const STAPEL_MAX: usize = 4096;

/// Rahmen einer Eingabenachricht: Art, frei, frei (u16), Laenge (u32), Nutzlast.
fn eingabe_rahmen(t: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.push(t);
    buf.push(0);
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Aufeinanderfolgende Mausbewegungen eines Stapels: nur die letzte bleibt -
/// die Lage ist absolut. Nach einem Stocken gingen sonst alle alten Lagen
/// einzeln hinaus, und der Zeiger fuehre drueben den ganzen Weg nach (frueher
/// fasste Windows WM_MOUSEMOVE zusammen, solange das blockierende Schreiben
/// den Fensterfaden aufhielt). Alles andere bleibt in seiner Reihenfolge:
/// Klicks tragen ihre eigene Lage, und eine Bewegung zwischen zwei Klicks
/// (Ziehen) bleibt stehen.
fn bewegungen_zusammenfassen(stapel: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut aus: Vec<Vec<u8>> = Vec::with_capacity(stapel.len());
    for b in stapel {
        match aus.last_mut() {
            Some(l) if b.first() == Some(&IN_MOVE) && l.first() == Some(&IN_MOVE) => *l = b,
            _ => aus.push(b),
        }
    }
    aus
}

/// Was der Schreibfaden eines Eingabekanals abzuarbeiten hat: Eingaben der
/// Reihe nach, dazu hoechstens EIN Datei-Paket mit Nachrang.
#[derive(Default)]
struct Warteschlange {
    eingaben: std::collections::VecDeque<Vec<u8>>,
    /// Das wartende Datei-Paket (DATEI_*, fertig gerahmt). Ist der Platz
    /// belegt, gilt fuer das naechste Gesendet::Voll.
    datei: Option<Vec<u8>>,
    /// Der Schreiber ist verworfen: der Faden endet.
    zu: bool,
}

impl Warteschlange {
    /// Ein Datei-Paket auf den einen Platz: Ja, oder Voll, solange dort noch
    /// eines wartet; Weg, wenn der Schreiber verworfen ist. Der Rahmen
    /// entsteht erst, wenn der Platz frei ist (`bauen`): der Sender versucht
    /// es bei Voll alle 2 ms wieder, und je Versuch 48 KiB zu kopieren und
    /// wegzuwerfen hiesse Arbeit unter der Sperre `input`, die der
    /// Fensterfaden fuer jede Taste braucht (Durchsicht [10]).
    fn datei_platz(&mut self, bauen: impl FnOnce() -> Vec<u8>) -> dateien::Gesendet {
        if self.zu {
            return dateien::Gesendet::Weg;
        }
        if self.datei.is_some() {
            return dateien::Gesendet::Voll;
        }
        self.datei = Some(bauen());
        dateien::Gesendet::Ja
    }
}

/// Schreibseite eines stehenden Eingabekanals.
struct Schreiber {
    q: Arc<(Mutex<Warteschlange>, std::sync::Condvar)>,
    /// Gesetzt, sobald der Schreibfaden an der Leitung gescheitert ist.
    kaputt: Arc<std::sync::atomic::AtomicBool>,
    /// Zweiter Griff an der Leitung: beim Verwerfen wird sie hierueber
    /// gekappt, auch wenn der Schreibfaden gerade in einem Schreibaufruf haengt.
    griff: Option<std::net::TcpStream>,
}

impl Schreiber {
    /// Uebernimmt die Leitung und startet ihren Schreibfaden. Er nimmt, was
    /// an Eingaben eingereiht ist, fasst Mausbewegungen zusammen und schreibt
    /// es in Stuecken mit `frist` (siehe SCHREIBFRIST). Erst wenn keine
    /// Eingabe mehr wartet, schreibt er hoechstens ein Datei-Paket und sieht
    /// danach wieder nach Eingaben: eine Taste wartet so hoechstens auf das
    /// eine Paket, das gerade hinausgeht (bis 48 KiB), und auf das, was davon
    /// im Sendepuffer des Kernels liegt (EINGABE_SNDBUF). Er endet, wenn das
    /// Schreiben scheitert oder der Schreiber verworfen ist; was dann noch
    /// eingereiht ist, faellt weg - der Host gibt beim Ende des Kanals ohnehin
    /// alle Tasten frei.
    fn neu(mut sock: secure::Secure, frist: Duration) -> Schreiber {
        let q: Arc<(Mutex<Warteschlange>, std::sync::Condvar)> = Arc::default();
        let kaputt = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (k, q2) = (kaputt.clone(), q.clone());
        sock.socket().set_write_timeout(Some(frist)).ok();
        eingabe_sendepuffer(sock.socket());
        let griff = sock.abbruchgriff();
        std::thread::spawn(move || loop {
            let (stapel, datei) = {
                let (m, cv) = &*q2;
                let mut w = m.lock().unwrap_or_else(|e| e.into_inner());
                while !w.zu && w.eingaben.is_empty() && w.datei.is_none() {
                    w = cv.wait(w).unwrap_or_else(|e| e.into_inner());
                }
                if w.zu {
                    return;
                }
                if w.eingaben.is_empty() {
                    (Vec::new(), w.datei.take())
                } else {
                    let n = w.eingaben.len().min(STAPEL_MAX);
                    (w.eingaben.drain(..n).collect::<Vec<_>>(), None)
                }
            };
            for b in bewegungen_zusammenfassen(stapel).iter().chain(datei.iter()) {
                // Je Stueck ein Datensatz, wie write_all am Stueck ihn
                // bilden wuerde: auf der Leitung dieselben Bytes.
                for stueck in b.chunks(secure::CHUNK_MAX) {
                    if sock.write_all(stueck).is_err() {
                        k.store(true, std::sync::atomic::Ordering::Relaxed);
                        // Gleich zu, nicht erst mit dem naechsten ensure:
                        // der zweite Griff hielte die Leitung sonst offen,
                        // und der Host saehe das Ende (und gaebe die
                        // Tasten frei) erst spaeter.
                        let _ = sock.socket().shutdown(std::net::Shutdown::Both);
                        return;
                    }
                }
            }
        });
        Schreiber { q, kaputt, griff }
    }

    fn steht(&self) -> bool {
        !self.kaputt.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Eine fertig gerahmte Eingabe einreihen; zurueck kommt sie, wenn der
    /// Faden nicht mehr schreibt.
    fn einreihen(&self, b: Vec<u8>) -> Result<(), Vec<u8>> {
        let (m, cv) = &*self.q;
        let mut w = m.lock().unwrap_or_else(|e| e.into_inner());
        if w.zu || !self.steht() {
            return Err(b);
        }
        w.eingaben.push_back(b);
        cv.notify_one();
        Ok(())
    }

    /// Ein Datei-Paket auf den einen Platz: Ja, oder Voll, solange dort noch
    /// eines wartet; Weg, wenn der Faden nicht mehr schreibt. Gerahmt wird
    /// erst, wenn es hineinkommt (Warteschlange::datei_platz) - die eine
    /// Kopie je Paket liegt unter der kurzen Sperre der Warteschlange.
    fn datei_einreihen(&self, typ: u8, nutzlast: &[u8]) -> dateien::Gesendet {
        let (m, cv) = &*self.q;
        let mut w = m.lock().unwrap_or_else(|e| e.into_inner());
        if !self.steht() {
            return dateien::Gesendet::Weg;
        }
        let r = w.datei_platz(|| eingabe_rahmen(typ, nutzlast));
        if r == dateien::Gesendet::Ja {
            cv.notify_one();
        }
        r
    }
}

impl Drop for Schreiber {
    /// Verworfen (Trennen, neue Bindung, neue Adresse, gescheitert): die
    /// Leitung sofort kappen. Ein Schreibfaden, der gerade an einer stockenden
    /// Leitung haengt, kehrt damit gleich zurueck, statt samt Leitung und
    /// Warteschlange weiterzuleben; der Host sieht das Ende des Kanals und
    /// gibt die Tasten frei.
    fn drop(&mut self) {
        {
            let (m, cv) = &*self.q;
            let mut w = m.lock().unwrap_or_else(|e| e.into_inner());
            w.zu = true;
            w.eingaben.clear();
            w.datei = None;
            cv.notify_all();
        }
        if let Some(g) = self.griff.take() {
            let _ = g.shutdown(std::net::Shutdown::Both);
        }
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
            nachreichen: Vec::new(),
            schreibfrist: SCHREIBFRIST,
            kanal_nr: 0,
        }
    }

    /// Adresse wechseln, etwa wenn ein anderer Host gewaehlt wurde. Was fuer
    /// den alten nachzureichen war, faellt weg.
    fn set_addr(&mut self, addr: String) {
        if addr != self.addr {
            self.addr = addr;
            self.kanal = None;
            self.aufbau = None;
            self.nachreichen.clear();
        }
    }

    /// Bindung an den Bildkanal setzen. Wechselt sie, wird neu verbunden; was
    /// fuer die alte Sitzung nachzureichen war, faellt weg. Liefert, ob
    /// darunter Einstellungen waren - die hat der Host dann nie bekommen.
    fn set_link(&mut self, link: Option<(Vec<u8>, Vec<u8>)>) -> bool {
        if self.link == link {
            return false;
        }
        self.link = link;
        self.kanal = None;
        self.aufbau = None;
        let einstellungen = self.nachreichen.iter().any(|b| b.first() == Some(&IN_SETTINGS));
        self.nachreichen.clear();
        einstellungen
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
                    let k = Schreiber::neu(s, self.schreibfrist);
                    // Zuerst, was dieser Client kann - auf jedem neuen Kanal,
                    // der Host merkt es sich je Eingabekanal -, dann, was
                    // ohne Kanal kam und einen Zustand setzt.
                    let nachzureichen = std::mem::take(&mut self.nachreichen);
                    let vorab = std::iter::once(faehigkeiten_rahmen())
                        .chain(nachzureichen.into_iter().filter(|b| b.first() != Some(&IN_FAEHIGKEITEN)));
                    for b in vorab {
                        if k.einreihen(b).is_ok() {
                            self.sent += 1;
                        }
                    }
                    self.kanal = Some(k);
                    self.kanal_nr += 1;
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
            // Nachricht 3 ohne Flags: den Host des Bildkanals kennt dieser
            // Client schon (er hat ihn eben angenommen).
            let r = secure::Secure::connect_pruefend(&addr, &prologue, |k| {
                if k == host.as_slice() {
                    return Ok(0);
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

    /// Eine Nachricht einreihen. Steht der Kanal nicht, faellt sie weg - bis
    /// auf Zustandsnachrichten: die kommen nach, sobald er steht (siehe
    /// NACHREICHEN).
    fn send(&mut self, t: u8, payload: &[u8]) {
        self.einreihen(t, payload);
    }

    /// Wie `send`; liefert, ob die Nachricht beim stehenden Kanal eingereiht
    /// ist (sonst gemerkt, falls NACHREICHEN, oder weg).
    fn einreihen(&mut self, t: u8, payload: &[u8]) -> bool {
        self.ensure();
        let mut buf = eingabe_rahmen(t, payload);
        if let Some(k) = self.kanal.as_ref().filter(|k| k.steht()) {
            match k.einreihen(buf) {
                Ok(()) => {
                    self.sent += 1;
                    return true;
                }
                Err(b) => {
                    buf = b;
                    self.kanal = None;
                }
            }
        }
        if NACHREICHEN.contains(&t) {
            self.nachreichen.retain(|b| b.first() != Some(&t));
            self.nachreichen.push(buf);
        }
        false
    }

    /// Ein Datei-Paket (DATEI_ANGEBOT, _STUECK, _ENDE) mit Nachrang: Ja =
    /// auf dem einen Platz des Schreibfadens, der erst alle wartenden
    /// Eingaben schreibt; Voll = dort wartet noch eines, spaeter noch einmal;
    /// Weg = kein stehender Kanal (die Uebertragung bricht ab). Wartet nie.
    fn datei_senden(&mut self, typ: u8, nutzlast: &[u8]) -> dateien::Gesendet {
        self.ensure();
        match self.kanal.as_ref().filter(|k| k.steht()) {
            Some(k) => {
                let r = k.datei_einreihen(typ, nutzlast);
                if r == dateien::Gesendet::Ja {
                    self.sent += 1;
                }
                r
            }
            None => dateien::Gesendet::Weg,
        }
    }

    /// Nummer des stehenden Kanals, None ohne.
    fn kanal_nr(&self) -> Option<u64> {
        self.steht().then_some(self.kanal_nr)
    }

    /// Pruefsumme des Bildkanals, an den dieser Eingabekanal gebunden ist.
    fn sitzung(&self) -> Option<&[u8]> {
        self.link.as_ref().map(|(hh, _)| hh.as_slice())
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

    /// Wunsch an den Host: diesen Bildschirm streamen (None = Automatik,
    /// der Host folgt seinem Hauptbildschirm). Die Antwort kommt ueber den
    /// Bildkanal als Nachricht 12; die Bytes macht bildschirm.rs. Nur fuer
    /// einen Host mit FAEHIG_BILDSCHIRM - das prueft der Aufrufer
    /// (bildschirm_wunsch_senden).
    fn bildschirm(&mut self, wunsch: Option<&str>) {
        self.send(IN_BILDSCHIRM, &bildschirm::wunsch_kodieren(wunsch));
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

    /// Eingabekanal schliessen. Der naechste Sendeversuch baut ihn neu auf.
    /// Die Leitung wird sofort gekappt (siehe Drop fuer Schreiber); der Host
    /// gibt dabei alles frei, was ueber sie gedrueckt wurde. Nachzureichendes
    /// faellt weg.
    fn trennen(&mut self) {
        self.kanal = None;
        self.aufbau = None;
        self.letzter_versuch = None;
        self.nachreichen.clear();
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

/// Ein Bildschirmwunsch an den Host (None = Automatik), aus dem Menue oder
/// dem Pruefmodus. Nur, wenn der Host die Wahl in dieser Sitzung gemeldet
/// hat (Bit 1 seiner Faehigkeiten) - ein aelterer Host bekommt nie Typ 70.
/// Erst den Hinweis setzen, dann den Wunsch abschicken (wie
/// codec_wuenschen: sonst koennte die Antwort den Hinweis loeschen, bevor
/// er steht). `shared` und `input` werden nacheinander genommen, nie
/// ineinander. Liefert, ob der Wunsch hinausging (oder zum Nachreichen
/// gemerkt ist).
fn bildschirm_wunsch_senden(shared: &Mutex<Shared>, input: &Mutex<InputLink>, wunsch: Option<String>) -> bool {
    {
        let mut s = shared.lock().unwrap();
        if !s.host_bildschirmwahl {
            return false;
        }
        s.bildschirm_wechsel = Some((Instant::now(), wunsch.clone()));
    }
    input.lock().unwrap().bildschirm(wunsch.as_deref());
    true
}

/// Je Durchlauf des Fensterfadens (about_to_wait): Der Eingabekanal haengt am
/// Bildkanal. Sobald dessen Handschlag steht, reichen wir die Bindung
/// weiter; faellt er weg, trennt sich auch die Eingabe - niemand soll
/// Tastatur ohne Bild bekommen. Steht fest, mit welchem Host wir sprechen,
/// gelten einmal die Werte, die beim letzten Mal fuer genau diesen Host
/// galten: lokal sofort (ein gespeichertes "Ton aus" auch dann, wenn der
/// Eingabekanal ausbleibt), zum Host ueber den Eingabekanal - steht der
/// noch nicht, reicht InputLink sie nach, sobald er steht (NACHREICHEN).
/// Bindung und Host kommen aus einem Blick auf `shared`: run_session setzt
/// beide zugleich. Wechselt die Sitzung, bevor die Einstellungen hinaus
/// waren, gelten sie in der neuen noch einmal. `angewandt_fuer`: fuer
/// welchen Host das schon geschehen ist.
fn gespeicherte_werte_anwenden(
    cfg: &einstellungen::Einstellungen,
    shared: &Mutex<Shared>,
    input: &Mutex<InputLink>,
    angewandt_fuer: &mut Option<String>,
) {
    let (link, fp) = {
        let s = shared.lock().unwrap();
        (s.link.clone(), s.peer_fp.clone())
    };
    let bildkanal = link.is_some();
    {
        let mut l = input.lock().unwrap();
        if l.set_link(link) {
            *angewandt_fuer = None;
        }
        // Den Aufbau treiben, solange der Kanal nicht steht - nie wartend,
        // hoechstens ein Versuch je Sekunde. Sonst kaeme Nachzureichendes
        // erst mit der naechsten Eingabe oder Zeitfrage an.
        if bildkanal && !l.steht() {
            l.ensure();
        }
    }
    match (&fp, &*angewandt_fuer) {
        (Some(f), keiner) if bildkanal && keiner.as_deref() != Some(f.as_str()) => {
            let werte = cfg.fuer_host(f);
            shared.lock().unwrap().ton = werte.map_or(true, |w| w.ton);
            if let Some(w) = werte {
                input.lock().unwrap().settings(w.mbit, w.fps, w.gaming, w.fest, w.ton);
            }
            *angewandt_fuer = Some(f.clone());
        }
        (None, _) => *angewandt_fuer = None,
        _ => {}
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

/// Ereignisse von ausserhalb des Fensterfadens an die Ereignisschleife
/// (winit-Benutzerereignisse ueber einen EventLoopProxy).
#[derive(Debug)]
pub enum Benutzer {
    /// Ein zweiter Start hat seine Adresse weitergereicht (leer = nur nach
    /// vorn holen), siehe einzel.rs. Die Schleife bestaetigt die Uebernahme;
    /// erst dann endet der zweite Start mit "weitergereicht".
    Einzel(einzel::Weitergabe),
    /// Wahl am Symbol der einen App: ein Punkt aus symbolmenue (Windows),
    /// auf dem Mac der Rueckruf der Menueleiste (host_mac.rs).
    Menue(symbolmenue::Aktion),
    /// Die Host-Rolle will eine Sprechblase am Symbol zeigen.
    Hinweis(String),
    /// Die Host-Rolle steht (ihre ID ist bekannt): Tooltip erneuern.
    #[cfg_attr(not(windows), allow(dead_code))]
    RolleBereit,
    /// Das Fenster "Geraetename" meldet einen neuen Namen (None: der
    /// Rechnername des Systems).
    Name(Option<String>),
    /// Die App endet von aussen (WM_CLOSE an ihr Symbol, Abmelden,
    /// Herunterfahren); der Abschied an einen Zuschauer ist schon hinaus.
    #[cfg_attr(not(windows), allow(dead_code))]
    Ende(String),
}

/// Was ein weitergereichter Start bewirkt (siehe `einzel_folge`).
#[derive(Debug, PartialEq)]
enum EinzelFolge {
    /// Ohne Adresse: nur das Fenster nach vorn.
    NachVorn,
    /// Die Sitzung laeuft schon zu dieser Adresse: bestehen lassen.
    Bleibt(String),
    /// Keine Sitzung: verbinden.
    Verbinden(String),
    /// Sitzung zu einem anderen Host: trennen, dann verbinden.
    Wechseln(String),
}

/// Entscheidung fuer einen weitergereichten Start. `sitzung` ist die
/// Adresse der laufenden (oder im Aufbau befindlichen) Sitzung, None auf
/// dem Startbildschirm. Verglichen wird mit ergaenztem Port und ohne
/// Ruecksicht auf die Schreibweise des Namens.
fn einzel_folge(sitzung: Option<&str>, adresse: &str) -> EinzelFolge {
    let adresse = adresse.trim();
    if adresse.is_empty() {
        return EinzelFolge::NachVorn;
    }
    let neu = adresse_vollstaendig(adresse);
    match sitzung {
        None => EinzelFolge::Verbinden(neu),
        Some(alt) if adresse_vollstaendig(alt).eq_ignore_ascii_case(&neu) => EinzelFolge::Bleibt(neu),
        Some(_) => EinzelFolge::Wechseln(neu),
    }
}

/// Knoepfe fuer die Desktop-Verknuepfung gibt es nur unter Windows. Als
/// Konstante statt cfg, damit derselbe Code auf beiden Plattformen gebaut
/// (und geprueft) wird.
const MIT_VERKNUEPFUNG: bool = cfg!(windows);

/// So lange steht das Ergebnis einer Verknuepfung im Meldungsbereich.
const VERKNUEPFUNG_ANZEIGE: Duration = Duration::from_secs(6);

/// Die eine App - Client und Host-Rolle in einem Prozess, ein Symbol - unter
/// Windows (Plan W5-W7) und auf dem Mac (M4): der Umschalter "Diesen PC
/// freigeben" bzw. "Diesen Mac freigeben" im Startbildschirm, die Zeile
/// "Dieser Computer" mit dem Knopf "Umbenennen" und das Kaestchen
/// "Ruhezustand verhindern". Als Konstante statt cfg, damit derselbe Code
/// auf allen Plattformen gebaut (und geprueft) wird.
const MIT_FREIGABE: bool = cfg!(any(windows, target_os = "macos"));

/// Der Umschalter der Freigabe heisst auf dem Mac "Diesen Mac freigeben".
const FREIGABE_TEXT: strings::Key = if cfg!(target_os = "macos") { strings::Key::StartShareMac } else { strings::Key::StartShare };

/// Laeuft die App ohne Fenster an (Plan W7)? Nur die eine App;
/// mit einem Ziel (Adresse, ID, Verknuepfung) nie; ohne Symbol (tray=aus)
/// nie - es gaebe keinen Weg zur App; sonst bei Autostart und --host immer
/// und bei jedem Start nach dem allerersten (einstellungen.txt:
/// fenster_gezeigt).
fn start_im_hintergrund(eine_app: bool, mit_ziel: bool, tray: bool, hintergrund_start: bool, fenster_gezeigt: bool) -> bool {
    eine_app && !mit_ziel && tray && (hintergrund_start || fenster_gezeigt)
}

/// Kommentar der Verknuepfung "Mit Windows starten".
#[cfg_attr(not(windows), allow(dead_code))]
fn autostart_beschreibung(lang: &strings::Lang) -> String {
    format!("QuadChroma – {}", lang.get(strings::Key::AppSubtitle))
}

/// Den Ruhezustand verhindern (`an`) oder wieder zulassen, mit
/// Protokollzeile; der Grund in powercfg /requests ist der Text des
/// Kaestchens. Liefert, ob die Anforderung jetzt gilt.
fn ruhe_setzen(r: &mut ruhezustand::Ruhesperre, an: bool, lang: &strings::Lang) -> bool {
    match r.setzen(an, lang.get(strings::Key::PreventSleep)) {
        Ok(ruhezustand::Schritt::Setzen) => protokoll::zeile("Ruhezustand: verhindert, solange QuadChroma laeuft".into()),
        Ok(ruhezustand::Schritt::Loesen) => protokoll::zeile("Ruhezustand: wieder erlaubt".into()),
        Ok(ruhezustand::Schritt::Nichts) => {}
        Err(e) => protokoll::zeile(format!("Ruhezustand: nicht umgestellt - {e}")),
    }
    r.an()
}

/// --ruhe-selbsttest (macOS): die Zusicherung "Ruhezustand verhindern" setzen,
/// in pmset -g assertions nachsehen (Art PreventUserIdleSystemSleep, Name
/// mit dem Text des Hakens), aufheben und wieder nachsehen. Ohne Fenster,
/// Host und Aufnahme. Rueckgabe 0 = bestanden; 2 auf anderen Plattformen
/// (unter Windows zeigt powercfg /requests die Anforderung, als Administrator).
fn ruhe_selbsttest() -> i32 {
    #[cfg(target_os = "macos")]
    {
        // Nur ASCII: pmset gibt andere Zeichen im Namen verstuemmelt aus
        // (die Zusicherung selbst traegt den Text richtig, auch "läuft").
        let grund = format!("{} - Selbsttest {}", strings::EN.get(strings::Key::PreventSleep), std::process::id());
        let mut r = ruhezustand::Ruhesperre::neu();
        if let Err(e) = r.setzen(true, &grund) {
            eprintln!("Ruhezustand-Selbsttest: nicht gesetzt - {e}");
            return 1;
        }
        let an = ruhezustand::pmset_zeilen(&grund);
        for z in an.iter().flatten() {
            println!("pmset: {z}");
        }
        let gesetzt = an.as_ref().is_some_and(|z| z.iter().any(|l| l.contains("PreventUserIdleSystemSleep")));
        let geloest = r.setzen(false, "").is_ok() && !r.an();
        let weg = ruhezustand::pmset_zeilen(&grund).is_some_and(|z| z.is_empty());
        println!(
            "Ruhezustand-Selbsttest: {} - nach dem Aufheben {}",
            if gesetzt { "in pmset -g assertions (PreventUserIdleSystemSleep)" } else { "NICHT in pmset -g assertions" },
            if geloest && weg { "weg" } else { "NOCH DA" }
        );
        if gesetzt && geloest && weg {
            0
        } else {
            1
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Den Ruhezustand-Selbsttest (--ruhe-selbsttest) gibt es nur unter macOS; unter Windows zeigt powercfg /requests die Anforderung.");
        2
    }
}

/// Was das Menue am Symbol der einen App aus dem Fensterfaden braucht; der
/// Symbolfaden baut es bei jedem Oeffnen daraus (samt Hostliste,
/// Geraetename, Autostart und dem Teil der Host-Rolle, die er selbst liest).
#[cfg_attr(not(windows), allow(dead_code))]
struct MenueQuelle {
    lang: &'static strings::Lang,
    ruhe_verhindern: bool,
}

/// Einfuegen in eigene Felder: Strg+V unter Windows, Cmd+V auf dem Mac.
const MOD_EINFUEGEN: u32 = if cfg!(target_os = "macos") { MOD_CMD } else { MOD_CTRL };

/// Text aus der Zwischenablage fuer ein eigenes Feld (Adresse, Passwort).
fn einfuegen_holen() -> Option<String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        clipboard::text_einfuegen()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// So oft wird der Stand des Symbols (Menue, Tooltip) neu berechnet.
const TRAY_TAKT: Duration = Duration::from_millis(500);

/// Takt der Ereignisschleife, solange das Fenster abgelegt ist: nichts zu
/// zeichnen, nur Symbol und Benutzerereignisse ("kein Zuschauer, keine
/// Arbeit").
const VERBORGEN_TAKT: Duration = Duration::from_millis(250);

/// macOS: so lange nach dem Verlassen des Vollbilds wird das Fenster
/// verborgen. Das Vollbild ist dort ein eigener Space; ein verborgenes
/// Fenster darin liesse einen leeren schwarzen Space zurueck. Der Uebergang
/// dauert etwa eine halbe Sekunde.
#[cfg(target_os = "macos")]
const VOLLBILD_VERLASSEN: Duration = Duration::from_millis(1200);

/// Wer ins Fenster zeichnet: die Karte - unter Windows ueber eine
/// Flip-Swapchain, auf dem Mac ueber einen CAMetalLayer - oder softbuffer
/// (GDI bzw. CoreGraphics) - nie beides am selben Fenster, das ist von DXGI
/// nicht gedeckt. Entschieden wird beim Start; `Keine` bleibt nach einem
/// Geraeteverlust, den der Neubau nicht heilen konnte.
enum Anzeige {
    #[cfg(any(windows, target_os = "macos"))]
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
#[cfg(any(windows, target_os = "macos"))]
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

/// Die NSView des Fensters, an die Metal seine Schicht haengt.
#[cfg(target_os = "macos")]
fn fenster_ansicht(window: &Window) -> Option<*mut std::ffi::c_void> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        _ => None,
    }
}

/// Die Karte am Fenster anlegen: unter Windows Direct3D 11 am HWND (`warp`:
/// Software-Rasterizer, `adapter`: --adapter bzw. die Karte der Rolle), auf
/// dem Mac Metal an der NSView.
#[cfg(any(windows, target_os = "macos"))]
fn gpu_am_fenster(window: &Window, warp: bool, adapter: Option<u32>) -> Result<anzeige::Gpu, String> {
    let s = window.inner_size();
    #[cfg(windows)]
    {
        fenster_hwnd(window)
            .ok_or_else(|| "kein Win32-Fenster".to_string())
            .and_then(|h| anzeige::Gpu::neu(h, s.width, s.height, warp, adapter))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (warp, adapter);
        fenster_ansicht(window)
            .ok_or_else(|| "keine AppKit-Ansicht".to_string())
            .and_then(|v| anzeige::Gpu::neu(v, s.width, s.height))
    }
}

/// Zwei Kaesten zu einem: der Ausschnitt der Oberflaeche, der in die Textur
/// muss - was beim letzten Mal dort stand (zu loeschen) und was jetzt neu
/// gezeichnet ist.
#[cfg(any(windows, target_os = "macos"))]
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
    /// Die Sprachwahl auf dem Startbildschirm ist offen.
    sprachwahl: bool,
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
    /// Zugangsdialog (Spezifikation Pairing v1, 9.3): das eingetippte
    /// Passwort, die Schreibmarke (in Zeichen), ob es lesbar gezeigt wird,
    /// und die Runde des Dialogs, zu der es gehoert - nach "falsch" wird es
    /// geleert. Es bleibt im Fensterfaden; hinaus geht nur der Beweis.
    zugang_pw: String,
    zugang_caret: usize,
    zugang_zeigen: bool,
    zugang_runde: Option<u32>,
    /// Bekannte Hosts aus hosts.txt (Haken im Startbildschirm, Suche ueber
    /// die ID), und zu welchem `Shared::hosts_stand` sie gelesen wurden.
    bekannte: zugang::Hostliste,
    bekannte_stand: Option<u64>,
    /// Start ueber eine ID ohne Adresse (Befehlszeile): seit wann auf ihre
    /// Bekanntgabe gewartet wird.
    id_ausstehend: Option<(u32, Instant)>,
    /// Die Host-Rolle der einen App (Windows: eigener Faden; Mac: die
    /// eingebaute Engine, angelaufen in resumed).
    rolle: Option<freigabe::Freigabe>,
    /// Die App lief ohne Fenster an (Autostart, --host, spaetere Starts):
    /// Fenster und Renderer entstehen erst mit "QuadChroma oeffnen".
    hintergrund: bool,
    /// Ob das Symbol steht - die Host-Rolle fragt hierueber ("Zulassen").
    #[cfg_attr(not(windows), allow(dead_code))]
    symbol_steht: Arc<std::sync::OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>>,
    /// Was der Symbolfaden fuer das Menue braucht, und die Zuordnung der
    /// Nummern des zuletzt gezeigten Menues.
    #[cfg_attr(not(windows), allow(dead_code))]
    menue_quelle: Arc<Mutex<MenueQuelle>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    menue_zuordnung: Arc<Mutex<symbolmenue::Zuordnung>>,
    /// "Ruhezustand verhindern, solange QuadChroma laeuft".
    ruhe: ruhezustand::Ruhesperre,
    /// Gespeicherte Einstellungen. Was hier steht, ueberlebt den Neustart.
    cfg: einstellungen::Einstellungen,
    /// Fuer welchen Host die gespeicherten Werte schon geschickt wurden.
    /// Verhindert, dass wir sie in jedem Bild erneut senden.
    angewandt_fuer: Option<String>,
    /// Wann zuletzt gezeichnet wurde - Oberflaechen ohne neues Bild werden
    /// nur alle 33 ms neu gezeichnet.
    letzte_zeichnung: Instant,
    /// Lag beim letzten Takt Oberflaeche ueber dem Bild? Verschwindet sie,
    /// wird einmal neu gezeichnet (siehe neu_zeichnen).
    oberflaeche_vorher: bool,
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
    /// Ergebnis der letzten Desktop-Verknuepfung und seit wann es steht -
    /// 6 s im Meldungsbereich des Startbildschirms bzw. im Reiter.
    verknuepfung_meldung: Option<(Meldung, Instant)>,
    /// Symbol im Infobereich bzw. in der Menueleiste. None bei tray=aus oder
    /// wenn es sich nicht anlegen liess - dann beendet Schliessen.
    symbol: Option<tray::Symbol>,
    /// Weg der Befehle des Symbols in die Ereignisschleife.
    proxy: winit::event_loop::EventLoopProxy<Benutzer>,
    /// Das Fenster ist abgelegt (unsichtbar): nichts zeichnen, langsamer Takt.
    verborgen: bool,
    /// Wann der Stand des Symbols zuletzt berechnet wurde.
    tray_takt: Instant,
    /// macOS: wann das Fenster nach dem Verlassen des Vollbilds verborgen wird.
    #[cfg(target_os = "macos")]
    verbergen_faellig: Option<Instant>,
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

impl ApplicationHandler<Benutzer> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        // Mac: die Host-Engine jetzt, auf dem Hauptfaden mit laufendem
        // AppKit - Oberflaeche, Schluessel, Zugang; mit Freigabe der Dienst;
        // dann das eine Symbol und das Programmmenue. Einmal (anlaufen merkt
        // es sich). Unter Windows laeuft die Rolle schon.
        let mut symbol_moeglich = true;
        if let Some(r) = self.rolle.as_mut() {
            // Die Wahlen am Symbol kommen als Benutzerereignisse.
            #[cfg(target_os = "macos")]
            host_mac::ziel_setzen(self.proxy.clone());
            symbol_moeglich = r.anlaufen(self.lang.code, self.cfg.geraetename.as_deref());
        }
        // Im Hintergrund (die eine App nach dem allerersten Start, Autostart,
        // --host): kein Fenster und kein Renderer, bis "QuadChroma oeffnen".
        if self.window.is_none() && !self.hintergrund {
            self.fenster_anlegen(el);
        }
        // Das Symbol im Infobereich bzw. in der Menueleiste: von Anfang an,
        // damit das Schliessen einen Weg zurueck hat (und der Tooltip die
        // Sitzung zeigt). Auf dem Mac verlangt AppKit den Hauptfaden nach
        // dem Start der Ereignisschleife - also hier; dort gehoert das
        // Symbol der Host-Engine und steht auch mit tray=aus (dann beendet
        // Schliessen trotzdem, tray::beim_schliessen) - es braucht den Stand
        // des Clients.
        if self.symbol.is_none() && (self.cfg.tray || cfg!(target_os = "macos")) && symbol_moeglich {
            self.symbol_anlegen();
        }
        // Liess sich kein Symbol anlegen, gaebe es ohne Fenster keinen Weg zur
        // App: dann doch das Fenster (steht es nur noch nicht, weil Explorer
        // fehlt, kommt es mit "TaskbarCreated").
        if self.window.is_none() && self.symbol.is_none() {
            protokoll::zeile(format!("{}: kein Symbol - das Fenster geht auf", tray::ORT));
            self.fenster_anlegen(el);
        }
    }

    /// Benutzerereignisse (siehe `Benutzer`). Eine Weitergabe, die kommt,
    /// waehrend das Programm schon endet, bleibt unbestaetigt: der zweite
    /// Start bekommt 0 und wird selbst erste Instanz, sobald diese weg ist
    /// (einzel.rs) - die Adresse geht nicht verloren.
    fn user_event(&mut self, el: &ActiveEventLoop, ereignis: Benutzer) {
        match ereignis {
            Benutzer::Einzel(w) if self.quit => {
                protokoll::zeile(format!("Einzelinstanz: zweiter Start mit {:?} waehrend des Beendens - nicht uebernommen", w.adresse));
            }
            Benutzer::Einzel(w) => {
                self.einzel_empfangen(el, &w.adresse);
                w.bestaetigen();
            }
            Benutzer::Menue(a) => self.menue_aktion(el, a),
            Benutzer::Hinweis(text) => {
                if let Some(s) = self.symbol.as_mut() {
                    s.hinweis("QuadChroma", &text);
                }
            }
            Benutzer::Name(n) => self.geraetename_setzen(n),
            Benutzer::RolleBereit => {
                if self.symbol.is_some() {
                    self.symbol_nachfuehren_jetzt();
                }
            }
            Benutzer::Ende(wie) => {
                protokoll::zeile(format!("{}: QuadChroma wird beendet ({wie})", tray::ORT));
                self.quit = true;
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.fenster_ereignis(el, event);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.vor_dem_warten(el);
    }
}

impl App {
    /// Fenster und Renderer anlegen - beim Start, oder im Hintergrund erst
    /// mit "QuadChroma oeffnen" (bzw. einem zweiten Start der exe).
    fn fenster_anlegen(&mut self, el: &ActiveEventLoop) {
        self.hintergrund = false;
        // Vollbild ist die Voreinstellung; F11 schaltet um und merkt sich das.
        // Das Symbol zeichnet logo.rs; die Titelleiste nimmt 32 px (Windows
        // rechnet herunter), die Taskleiste 48 px. Auf dem Mac setzt winit
        // kein Fenstersymbol - dort bleibt es beim Symbol des Programms.
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title("QuadChroma")
            .with_window_icon(logo::fenster_symbol(32))
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
            .with_fullscreen(if self.fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            });
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_taskbar_icon(logo::fenster_symbol(48));
        }
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
                            // Ohne Anhang: `e` ist eigener deutscher Text
                            // (anzeige.rs) und steht schon im Protokoll.
                            let m = Meldung::neu(
                                strings::Key::ErrorGpuDisplay,
                                format!("Grafikkarte nicht nutzbar, Anzeige ueber Software: {e}"),
                            );
                            self.shared.lock().unwrap().error = Some(m);
                        }
                    }
                }
            }
        }
        // Mac: Metal, ausser der Prozessor ist ausdruecklich gewuenscht. Eine
        // Wahl zwischen Karten gibt es dort nicht (ein Geraet); jeder andere
        // Wunsch heisst Metal, und der Knopf Automatik gilt.
        #[cfg(target_os = "macos")]
        {
            use einstellungen::AnzeigeWunsch as W;
            if self.anzeige_wunsch != W::Cpu {
                match gpu_am_fenster(&window, false, None) {
                    Ok(g) => {
                        self.anzeige_name = format!("Metal · {}", g.adapter.name);
                        self.sofort = g.tearing;
                        self.anzeige_aktiv = W::Automatik;
                        // Ab jetzt legt der Empfangsfaden rohe Bilder ab; die
                        // Umrechnung nach RGB macht Stufe 1 auf der Karte.
                        self.shared.lock().unwrap().gpu_pfad = true;
                        self.anzeige = Anzeige::Gpu(g);
                    }
                    Err(e) => {
                        protokoll::zeile(format!("Anzeige: Rueckfall auf Software: {e}"));
                        if !matches!(self.anzeige_wunsch, W::Automatik | W::Warp) {
                            let m = Meldung::neu(
                                strings::Key::ErrorGpuDisplay,
                                format!("Metal nicht nutzbar, Anzeige ueber Software: {e}"),
                            );
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
        self.ui_kasten_alt = None;
        // Nur der allererste Start oeffnet das Fenster von selbst; ab jetzt
        // bleibt jeder Start im Infobereich.
        if !self.cfg.fenster_gezeigt {
            self.cfg.fenster_gezeigt = true;
            self.cfg.sichern();
        }
        // Mac: die App startet als Accessory (nur das Symbol); mit Fenster
        // ist sie ein normales Programm (Dock, Programmmenue) und vorn.
        #[cfg(target_os = "macos")]
        if !self.verborgen {
            host_mac::aktivierung(true);
            if let Some(w) = &self.window {
                w.focus_window();
            }
        }
    }

    fn fenster_ereignis(&mut self, el: &ActiveEventLoop, event: WindowEvent) {
        match event {
            // Schliessen legt die App ab (Infobereich bzw. Menueleiste);
            // ohne Symbol oder mit tray=aus beendet es wie bisher.
            WindowEvent::CloseRequested => self.schliessen(el),
            // Abgelegt wird nichts gezeichnet.
            WindowEvent::RedrawRequested => {
                if !self.verborgen {
                    self.draw();
                }
            }
            // Verdeckt praesentiert Metal nicht: CoreAnimation zeigt dort
            // nichts, und nextDrawable koennte warten (anzeige_mac.rs; ein
            // Fenster, das schon verdeckt aufging, erkennt die Anzeige
            // selbst). Wieder sichtbar, nachdem ein Bild ausfiel: das
            // letzte Bild einmal neu praesentieren. Es wurde verdeckt
            // hochgeladen, aber nie gezeigt, und AppKit zeichnet eine Ansicht
            // mit Schicht beim Aufdecken nicht neu - sonst bliebe stehen, was
            // vor dem Verdecken dort war, bis der Host ein neues Bild schickt
            // (bei stillem Bildschirm nie).
            #[cfg(target_os = "macos")]
            WindowEvent::Occluded(verdeckt) => {
                if let Anzeige::Gpu(g) = &mut self.anzeige {
                    if g.verdeckt(verdeckt) {
                        self.praesentation_ausstehend = true;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
            }

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

                // Startbildschirm: Adresse, Name oder Geraete-ID eintippen
                if self.screen == Screen::Start {
                    if !pressed { return; }
                    if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KC::Backspace => { self.addr_input.pop(); return; }
                            KC::Enter | KC::NumpadEnter => {
                                let text = self.addr_input.clone();
                                self.eingabe_verbinden(&text);
                                return;
                            }
                            // Einfuegen (Strg+V bzw. Cmd+V): etwa eine kopierte
                            // ID; ohne Zeilenwechsel und Steuerzeichen.
                            KC::KeyV if self.mods & MOD_EINFUEGEN != 0 => {
                                if let Some(t) = einfuegen_holen() {
                                    for ch in t.trim().chars().filter(|c| !c.is_control()) {
                                        if self.addr_input.len() + ch.len_utf8() > 64 { break; }
                                        self.addr_input.push(ch);
                                    }
                                }
                                return;
                            }
                            // Esc schliesst zuerst eine offene Sprachwahl.
                            KC::Escape if self.sprachwahl => { self.sprachwahl = false; return; }
                            // Esc: in der einen App ablegen wie das Schliessen -
                            // die Freigabe laeuft weiter (ohne Symbol: Ende);
                            // sonst beenden wie bisher.
                            KC::Escape if MIT_FREIGABE => { self.schliessen(el); return; }
                            KC::Escape => { self.quit = true; return; }
                            // Cmd+W schliesst auf dem Startbildschirm wie das
                            // rote Knoepfchen (winit legt dafuer keinen
                            // Menuepunkt an). In der Sitzung gehoert Cmd+W
                            // dem Mac drueben.
                            #[cfg(target_os = "macos")]
                            KC::KeyW if self.mods & MOD_CMD != 0 => { self.schliessen(el); return; }
                            _ => {}
                        }
                    }
                    // Cmd+Taste ist auf dem Mac ein Kuerzel, kein Text.
                    if cfg!(target_os = "macos") && self.mods & MOD_CMD != 0 { return; }
                    if let Some(t) = &event.text {
                        for ch in t.chars() {
                            if !ch.is_control() && self.addr_input.len() < 64 { self.addr_input.push(ch); }
                        }
                    }
                    return;
                }

                // Zugangsdialog offen: die Tastatur gehoert ihm - nichts geht
                // an den Host, auch ESC und die F-Tasten nicht (9.3).
                if self.zugang_offen() {
                    self.zugang_taste(&event);
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

    fn vor_dem_warten(&mut self, el: &ActiveEventLoop) {
        if self.quit { el.exit(); return; }
        // Symbol: Menue (gefundene Hosts, Sprache) und Tooltip (Sitzung).
        self.symbol_nachfuehren();
        // Ohne Fenster (die eine App im Hintergrund): nichts zeichnen, nichts
        // nachsehen - warten, bis ein Benutzerereignis kommt (Symbol,
        // Einzelinstanz, Host-Rolle).
        if self.window.is_none() {
            // Mac: das Menue der Menueleiste baut menue.m aus dem zuletzt
            // gegebenen Stand - der langsame Takt fuehrt die gefundenen Hosts
            // nach (symbol_nachfuehren, hoechstens alle 500 ms).
            if cfg!(target_os = "macos") {
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + TRAY_TAKT));
            } else {
                el.set_control_flow(ControlFlow::Wait);
            }
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(t) = self.verbergen_faellig {
            if Instant::now() >= t {
                self.verbergen_faellig = None;
                if self.verborgen {
                    if let Some(w) = &self.window {
                        w.set_visible(false);
                    }
                    host_mac::aktivierung(false);
                }
            }
        }
        // Abgelegt: nichts zeichnen, nichts nachsehen - nur auf
        // Benutzerereignisse (Symbol, Einzelinstanz) warten. Eine Sitzung
        // gibt es dann nicht (Schliessen hat getrennt). Unter Windows baut
        // der Symbolfaden das Menue selbst - es gibt nichts nachzufuehren; auf
        // dem Mac fuehrt der langsame Takt das Menue der Menueleiste nach
        // (gefundene Hosts).
        if self.verborgen {
            if cfg!(windows) {
                el.set_control_flow(ControlFlow::Wait);
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + VERBORGEN_TAKT));
            }
            return;
        }
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
        //
        // Hat der Empfangsfaden das Ziel ueber seine ID unter einer neuen
        // Adresse gefunden, zeigt das Wartebild die neue.
        if self.screen == Screen::Session && self.id_ausstehend.is_none() {
            let ziel = self.shared.lock().unwrap().target.clone();
            match ziel {
                None => {
                    self.input.lock().unwrap().set_link(None);
                    self.verbindung_trennen();
                }
                Some(t) if t != self.addr_input => self.addr_input = t,
                Some(_) => {}
            }
        }
        // Der Zugangsdialog haelt die Tastatur - also muss er zu sehen sein:
        // steht noch ein Bild einer vorigen Sitzung, weicht es dem
        // Wartebildschirm, auf dem er liegt (sonst tippte der Nutzer
        // ungesehen in ein Passwortfeld). Ist er zu, ist auch sein Passwort
        // vergessen - ein spaeterer Dialog beginnt mit einem leeren Feld.
        if self.screen == Screen::Session {
            let offen = self.shared.lock().unwrap().zugang.is_some();
            if offen && self.bild_vorhanden() {
                self.last_frame = None;
                self.bild_da = None;
                self.ui_kasten_alt = None;
            }
            if !offen && (self.zugang_runde.is_some() || !self.zugang_pw.is_empty()) {
                self.zugang_leeren();
            }
        }
        // Start ueber eine ID ohne Adresse: verbinden, sobald sie sich im Netz
        // meldet; nach ID_SUCHE zurueck zum Startbildschirm mit Meldung (9.2).
        if let Some((id, seit)) = self.id_ausstehend {
            let g = self.hosts.lock().ok().and_then(|h| h.mit_id(id));
            if let Some(g) = g {
                self.verbinden(Ziel { adresse: g.host.addr.to_string(), id: Some(id), name: Some(g.host.name) });
            } else if seit.elapsed() >= ID_SUCHE {
                self.id_ausstehend = None;
                self.screen = Screen::Start;
                self.meldung_zeigen(
                    Meldung::neu(strings::Key::MsgIdNotFound, format!("Start: ID {} meldet sich nicht im Netz", zugang::id_text(id)))
                        .mit("{i}", zugang::id_text(id)),
                );
            }
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
        // Bindung des Eingabekanals und die fuer diesen Host gespeicherten Werte.
        gespeicherte_werte_anwenden(&self.cfg, &self.shared, &self.input, &mut self.angewandt_fuer);
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
        // Die Zeile zu Dateien zaehlt wie die Lagemeldung: solange sie
        // sichtbar ist, wird neu gezeichnet (Fortschritt, dann 6 s Ergebnis).
        let (neues_bild, lage, wechsel) = {
            let s = self.shared.lock().unwrap();
            (
                s.frame.is_some(),
                s.error_key.is_some() || s.datei_stand.sichtbar(Instant::now()),
                s.wechsel_laeuft() || s.bildschirm_wechsel_laeuft(),
            )
        };
        // Ein ausgelassenes Present (DXGI war noch nicht bereit) wird beim
        // naechsten Takt nachgeholt; der Codecwechsel-Hinweis muss auch ohne
        // neue Bilder erscheinen und wieder verschwinden.
        let oberflaeche = self.oberflaeche_sichtbar(lage, wechsel);
        let vorher = std::mem::replace(&mut self.oberflaeche_vorher, oberflaeche);
        if neu_zeichnen(neues_bild, self.praesentation_ausstehend, oberflaeche, vorher, self.letzte_zeichnung.elapsed()) {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}

/// Ist ein Neuzeichnen faellig? Bei einem neuen Bild und einem
/// ausgelassenen Present immer; solange Oberflaeche ueber dem Bild liegt,
/// alle 33 ms; und einmal in dem Takt, in dem sie verschwindet (`vorher`
/// sichtbar, jetzt nicht) - sonst bliebe etwa die Zeile zu Dateien nach ihren
/// 6 s stehen, bis der Host das naechste Bild schickt, und bei stillem
/// Bildschirm tut er das nicht (Durchsicht [11]). Dasselbe gilt fuer Banner
/// und Codecwechsel-Hinweis.
fn neu_zeichnen(neues_bild: bool, ausstehend: bool, oberflaeche: bool, vorher: bool, seit_letzter: Duration) -> bool {
    neues_bild || ausstehend || (vorher && !oberflaeche) || (oberflaeche && seit_letzter >= Duration::from_millis(33))
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

    /// Wunsch nach einem Bildschirm des Hosts (None = Automatik), aus dem
    /// Menue. Reihenfolge wie beim Codec: erst der Hinweis, dann der Wunsch.
    fn bildschirm_wuenschen(&mut self, wunsch: Option<String>) {
        bildschirm_wunsch_senden(&self.shared, &self.input, wunsch);
    }

    fn verbindung_trennen(&mut self) {
        self.trennen(true);
    }

    /// Wie `verbindung_trennen`. `kappen` false laesst die Bildleitung
    /// offen: beim Abbrechen im Zugangsdialog schickt der Empfangsfaden noch
    /// Nachricht 23 und schliesst dann selbst (er sieht binnen eines Takts,
    /// dass das Ziel weg ist).
    fn trennen(&mut self, kappen: bool) {
        // Ein laufender Benchmark endet mit der Verbindung; wiederherstellen
        // gibt es nichts mehr, nur das Testbild geht noch aus.
        self.benchmark_abbrechen(false);
        self.id_ausstehend = None;
        let (griff, datei) = {
            let mut s = self.shared.lock().unwrap();
            s.target = None;
            s.angenommen = None;
            // Eine laufende Datei-Sendung sofort ab (der Empfaenger bricht
            // mit dem Ende von run_session ab).
            (s.abbruch.take(), s.dateien_zuruecksetzen())
        };
        drop(datei);
        {
            let mut l = self.input.lock().unwrap();
            l.alle_loslassen();
            l.trennen();
        }
        if let Some(g) = griff.filter(|_| kappen) {
            let _ = g.shutdown(std::net::Shutdown::Both);
        }
        self.hud_offen = false;
        self.screen = Screen::Start;
        self.last_frame = None;
        self.bild_da = None;
        self.ui_kasten_alt = None;
        self.zugang_leeren();
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

    /// Mit einem Ziel verbinden: Port ergaenzen, Eingabekanal und Ziel samt
    /// ID und Name setzen, alte Meldungen weg, in die Sitzung. Derselbe Weg
    /// fuer den Klick auf dem Startbildschirm, das Symbol und die Weitergabe
    /// eines zweiten Starts.
    fn verbinden(&mut self, ziel: Ziel) {
        let addr = adresse_vollstaendig(&ziel.adresse);
        self.addr_input = addr.clone();
        let input_addr = bump_port(&addr, 1);
        self.input.lock().unwrap().set_addr(input_addr);
        let mut s = self.shared.lock().unwrap();
        s.target = Some(addr);
        s.ziel_id = ziel.id;
        s.ziel_name = ziel.name;
        s.angenommen = None;
        s.error = None;
        s.error_key = None;
        s.zugang_eingabe = None;
        drop(s);
        self.id_ausstehend = None;
        self.zugang_leeren();
        self.screen = Screen::Session;
    }

    /// Was im Adressfeld steht, verbinden (Knopf, Enter): eine Geraete-ID
    /// wird unter den gefundenen Hosts und in hosts.txt gesucht (9.2); ist
    /// sie nirgends, steht die Meldung auf dem Startbildschirm.
    fn eingabe_verbinden(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let eigene_id = secure::eigener_host_schluessel().map(|k| zugang::geraete_id(&k));
        if let Some(m) = eigene_id_eingegeben(text, eigene_id) {
            self.meldung_zeigen(m);
            return;
        }
        self.bekannte_nachladen(true);
        let gefunden = self.hosts.lock().map(|h| h.liste()).unwrap_or_default();
        match ziel_aus_eingabe(text, &gefunden, &self.bekannte) {
            Ok(z) => self.verbinden(z),
            Err(m) => self.meldung_zeigen(m),
        }
    }

    /// Eine Meldung auf dem Startbildschirm (ohne Verbindung), samt Protokoll.
    fn meldung_zeigen(&mut self, m: Meldung) {
        protokoll::zeile(m.protokoll.clone());
        let mut s = self.shared.lock().unwrap();
        s.error_key = None;
        s.error = Some(m);
    }

    /// hosts.txt neu lesen, wenn der Empfangsfaden sie geaendert hat (oder
    /// `erzwingen`) - fuer die Haken der Hostliste und die Suche ueber die ID.
    fn bekannte_nachladen(&mut self, erzwingen: bool) {
        let stand = self.shared.lock().unwrap().hosts_stand;
        if !erzwingen && self.bekannte_stand == Some(stand) {
            return;
        }
        self.bekannte_stand = Some(stand);
        self.bekannte = zugang::ablage_pfad(zugang::HOSTS_DATEI)
            .ok()
            .and_then(|p| zugang::Hostliste::laden(&p).ok())
            .unwrap_or_default();
    }

    /// Ein Ziel aus Adresse und/oder ID mit dem, was gerade im Netz und in
    /// hosts.txt steht (siehe `ziel_bilden`).
    fn ziel(&mut self, adresse: &str, id: Option<u32>) -> Result<Ziel, Meldung> {
        self.bekannte_nachladen(true);
        let gefunden = self.hosts.lock().map(|h| h.liste()).unwrap_or_default();
        ziel_bilden(adresse, id, &gefunden, &self.bekannte)
    }

    /// Ein zweiter Start hat sich gemeldet (einzel.rs): Fenster sichtbar und
    /// nach vorn; mit Adresse (und ID, aus einer Verknuepfung) verbinden wie
    /// ein Klick auf "Verbinden". Eine Sitzung zu einem anderen Host wird
    /// vorher getrennt, eine zum selben bleibt bestehen - der Doppelklick auf
    /// die Verknuepfung soll sie nicht abloesen.
    fn einzel_empfangen(&mut self, el: &ActiveEventLoop, text: &str) {
        self.fenster_zeigen(el);
        let (adresse, id) = ziel_lesen(text);
        if adresse.is_empty() && id.is_none() {
            protokoll::zeile("Einzelinstanz: zweiter Start ohne Adresse - Fenster nach vorn".into());
            return;
        }
        let ziel = match self.ziel(&adresse, id) {
            Ok(z) => z,
            Err(m) => {
                if self.screen == Screen::Start {
                    self.meldung_zeigen(m);
                } else {
                    protokoll::zeile(format!("Einzelinstanz: zweiter Start - {}", m.protokoll));
                }
                return;
            }
        };
        let sitzung = (self.screen == Screen::Session).then_some(self.addr_input.as_str());
        match einzel_folge(sitzung, &ziel.adresse) {
            EinzelFolge::NachVorn => {
                protokoll::zeile("Einzelinstanz: zweiter Start ohne Adresse - Fenster nach vorn".into());
            }
            EinzelFolge::Bleibt(a) => {
                protokoll::zeile(format!("Einzelinstanz: zweiter Start mit {a} - Sitzung dorthin laeuft schon"));
            }
            EinzelFolge::Verbinden(a) => {
                protokoll::zeile(format!("Einzelinstanz: zweiter Start mit {a} - verbinde"));
                self.verbinden(ziel);
            }
            EinzelFolge::Wechseln(a) => {
                protokoll::zeile(format!("Einzelinstanz: zweiter Start mit {a} - trenne {} und verbinde neu", self.addr_input));
                self.verbindung_trennen();
                self.verbinden(ziel);
            }
        }
    }

    /// Den Zugangsdialog vergessen: Passwort, Schreibmarke, "Anzeigen".
    fn zugang_leeren(&mut self) {
        self.zugang_pw.clear();
        self.zugang_caret = 0;
        self.zugang_zeigen = false;
        self.zugang_runde = None;
    }

    /// Ist der Zugangsdialog offen? Dann gehoert ihm die Tastatur.
    fn zugang_offen(&self) -> bool {
        self.screen == Screen::Session && self.shared.lock().unwrap().zugang.is_some()
    }

    /// "Verbinden" im Zugangsdialog (Knopf oder Enter): das Passwort geht an
    /// den Empfangsfaden, der daraus den Beweis baut - nur mit Passwort und
    /// nach der Wartezeit der Drossel.
    fn zugang_senden(&mut self) {
        let mut s = self.shared.lock().unwrap();
        let Some(d) = s.zugang.as_ref() else { return };
        if self.zugang_pw.is_empty() || !d.darf_senden(Instant::now()) {
            return;
        }
        s.zugang_eingabe = Some(zugangsphase::Eingabe::Passwort(self.zugang_pw.clone()));
    }

    /// "Abbrechen" im Zugangsdialog (oder Esc): zurueck zum Startbildschirm,
    /// ohne Meldung. Nachricht 23 schickt der Empfangsfaden - deshalb wird die
    /// Leitung hier nicht gekappt.
    fn zugang_abbrechen(&mut self) {
        self.shared.lock().unwrap().zugang_eingabe = Some(zugangsphase::Eingabe::Abbrechen);
        self.trennen(false);
    }

    /// Text an der Schreibmarke ins Passwortfeld (Tippen, Einfuegen). Ohne
    /// Steuerzeichen - ein eingefuegter Zeilenwechsel gehoert nicht dazu -
    /// und hoechstens PASSWORT_MAX Byte.
    fn zugang_einfuegen(&mut self, t: &str) {
        for ch in t.chars().filter(|c| !c.is_control()) {
            if self.zugang_pw.len() + ch.len_utf8() > zugang::PASSWORT_MAX {
                break;
            }
            let i = self.zugang_pw.char_indices().nth(self.zugang_caret).map(|(i, _)| i).unwrap_or(self.zugang_pw.len());
            self.zugang_pw.insert(i, ch);
            self.zugang_caret += 1;
        }
    }

    /// Eine Taste im offenen Zugangsdialog (9.3): Tippen, Einfuegen,
    /// Schreibmarke, Enter = Verbinden, Esc = Abbrechen. Nichts davon geht
    /// an den Host - auch kein ESC-Halten fuer das Menue.
    fn zugang_taste(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{KeyCode as KC, PhysicalKey};
        if event.state != winit::event::ElementState::Pressed {
            return;
        }
        let n = self.zugang_pw.chars().count();
        self.zugang_caret = self.zugang_caret.min(n);
        let byte = |s: &str, i: usize| s.char_indices().nth(i).map(|(b, _)| b).unwrap_or(s.len());
        if let PhysicalKey::Code(code) = event.physical_key {
            match code {
                KC::Enter | KC::NumpadEnter => return self.zugang_senden(),
                KC::Escape => return self.zugang_abbrechen(),
                KC::Backspace => {
                    if self.zugang_caret > 0 {
                        self.zugang_caret -= 1;
                        let i = byte(&self.zugang_pw, self.zugang_caret);
                        self.zugang_pw.remove(i);
                    }
                    return;
                }
                KC::Delete => {
                    if self.zugang_caret < n {
                        let i = byte(&self.zugang_pw, self.zugang_caret);
                        self.zugang_pw.remove(i);
                    }
                    return;
                }
                KC::ArrowLeft => {
                    self.zugang_caret = self.zugang_caret.saturating_sub(1);
                    return;
                }
                KC::ArrowRight => {
                    self.zugang_caret = (self.zugang_caret + 1).min(n);
                    return;
                }
                KC::Home => {
                    self.zugang_caret = 0;
                    return;
                }
                KC::End => {
                    self.zugang_caret = n;
                    return;
                }
                KC::KeyV if self.mods & MOD_EINFUEGEN != 0 => {
                    if let Some(t) = einfuegen_holen() {
                        self.zugang_einfuegen(&t);
                    }
                    return;
                }
                _ => {}
            }
        }
        // Cmd+Taste ist auf dem Mac ein Kuerzel, kein Text.
        if cfg!(target_os = "macos") && self.mods & MOD_CMD != 0 {
            return;
        }
        if let Some(t) = &event.text {
            let t = t.to_string();
            self.zugang_einfuegen(&t);
        }
    }

    /// "Diesen PC freigeben" (Startbildschirm, Menue): die Freigabe der
    /// einen App an bzw. aus, gemerkt in einstellungen.txt. Aus heisst: ein
    /// Zuschauer erfaehrt es (Abschied, Grund 1), die Ports gehen zu.
    fn freigabe_umschalten(&mut self) {
        let Some(r) = self.rolle.as_mut() else { return };
        self.cfg.freigabe = !self.cfg.freigabe;
        self.cfg.sichern();
        r.setzen(self.cfg.freigabe);
        protokoll::zeile(format!("Freigabe: {}", if self.cfg.freigabe { "an" } else { "aus" }));
        self.symbol_nachfuehren_jetzt();
    }

    /// "Ruhezustand verhindern" (Startbildschirm, Menue): sofort wirksam,
    /// gemerkt in einstellungen.txt.
    fn ruhe_umschalten(&mut self) {
        let an = ruhe_setzen(&mut self.ruhe, !self.cfg.ruhe_verhindern, self.lang);
        // Gemerkt wird der Wunsch; laesst das System ihn nicht zu, sagt es das
        // Protokoll, und der Haken folgt dem, was gilt.
        self.cfg.ruhe_verhindern = an;
        self.cfg.sichern();
        if let Ok(mut q) = self.menue_quelle.lock() {
            q.ruhe_verhindern = an;
        }
        self.symbol_nachfuehren_jetzt();
    }

    /// "Mit Windows starten" (Menue): die eine Verknuepfung im
    /// Autostart-Ordner an bzw. aus.
    fn autostart_umschalten(&mut self) {
        #[cfg(windows)]
        {
            let an = !verknuepfung::autostart_an(None);
            match verknuepfung::autostart_setzen(None, an, &autostart_beschreibung(self.lang)) {
                Ok(()) => protokoll::zeile(if an {
                    "Mit Windows starten: an (Verknuepfung im Autostart-Ordner)".into()
                } else {
                    "Mit Windows starten: aus".into()
                }),
                Err(f) => protokoll::zeile(format!("Mit Windows starten nicht umgestellt: {f}")),
            }
        }
    }

    /// "Geraetename aendern ..." (Menue) bzw. "Umbenennen" (Startbildschirm):
    /// das Fenster oeffnen; der neue Name kommt als Benutzerereignis zurueck
    /// (auf dem Mac aus dem Fenster der Menueleiste, host/menue.m).
    fn geraetename_fenster(&mut self) {
        #[cfg(target_os = "macos")]
        host_mac::geraetename_fenster();
        #[cfg(windows)]
        {
            let proxy = Mutex::new(self.proxy.clone());
            host::fenster::geraetename_aendern(
                self.lang,
                &zugang::geraetename(),
                &zugang::rechnername(),
                Box::new(move |n| {
                    if let Ok(p) = proxy.lock() {
                        let _ = p.send_event(Benutzer::Name(n));
                    }
                }),
            );
        }
    }

    /// Ein neuer Geraetename (None: der Rechnername): gemerkt, und er gilt
    /// sofort - Bekanntgabe ab der naechsten Runde, Nachricht 3 ab der
    /// naechsten Verbindung, Nachricht 20 ab der naechsten Zugangsphase,
    /// Menue und Startbildschirm gleich.
    fn geraetename_setzen(&mut self, name: Option<String>) {
        let name = name.and_then(|n| zugang::geraetename_pruefen(&n).ok().flatten());
        if name == self.cfg.geraetename {
            return;
        }
        self.cfg.geraetename = name.clone();
        self.cfg.sichern();
        zugang::geraetename_setzen(name);
        if let Some(r) = self.rolle.as_ref() {
            r.name_geaendert();
        }
        protokoll::zeile(format!(
            "Geraetename: {}{}",
            zugang::geraetename(),
            if self.cfg.geraetename.is_none() { " (Rechnername)" } else { "" }
        ));
    }

    /// Die Geraete-ID, die andere von diesem Computer sehen: die der
    /// Host-Rolle (host.key).
    fn eigene_id(&self) -> Option<u32> {
        self.rolle
            .as_ref()
            .and_then(|r| r.id())
            .or_else(|| secure::eigener_host_schluessel().map(|k| zugang::geraete_id(&k)))
    }

    /// Ein Punkt am Symbol der einen App.
    fn menue_aktion(&mut self, el: &ActiveEventLoop, a: symbolmenue::Aktion) {
        use symbolmenue::Aktion as A;
        match a {
            A::Oeffnen => self.tray_befehl(el, tray::Befehl::Oeffnen),
            A::Verbinden(adresse) => self.tray_befehl(el, tray::Befehl::Verbinden(adresse)),
            A::Beenden => self.tray_befehl(el, tray::Befehl::Beenden),
            A::Freigabe => self.freigabe_umschalten(),
            A::Autostart => self.autostart_umschalten(),
            A::RuheVerhindern => self.ruhe_umschalten(),
            A::NameAendern => self.geraetename_fenster(),
            a => {
                let erledigt = self.rolle.as_ref().is_some_and(|r| r.aktion(&a));
                if !erledigt {
                    protokoll::zeile(format!("{}: {a:?} ohne Host-Rolle - uebergangen", tray::ORT));
                }
            }
        }
    }

    /// Das Fenster sichtbar und nach vorn - auch aus dem Infobereich bzw.
    /// der Menueleiste. Derselbe Weg fuer Einzelinstanz und Symbol. Lief die
    /// App im Hintergrund an, entstehen Fenster und Renderer erst jetzt.
    fn fenster_zeigen(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            self.fenster_anlegen(el);
            protokoll::zeile(format!("{}: Fenster geoeffnet", tray::ORT));
        }
        let war_verborgen = self.verborgen;
        self.verborgen = false;
        #[cfg(target_os = "macos")]
        {
            self.verbergen_faellig = None;
            // Erst wieder ein normales Programm (Dock, Menueleiste), dann
            // das Fenster - sonst bekaeme es keinen Fokus.
            host_mac::aktivierung(true);
        }
        if let Some(w) = &self.window {
            w.set_visible(true);
            w.set_minimized(false);
            // macOS: das Vollbild wurde beim Ablegen verlassen (siehe
            // fenster_verbergen) - wieder hinein, wenn es gewuenscht ist.
            #[cfg(target_os = "macos")]
            if war_verborgen && self.fullscreen && w.fullscreen().is_none() {
                w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
            w.focus_window();
        }
        if war_verborgen {
            // Die Oberflaeche ganz neu zeichnen, nicht nur den Kasten.
            self.ui_kasten_alt = None;
            protokoll::zeile(format!("{}: Fenster wieder sichtbar", tray::ORT));
        }
    }

    /// Das Fenster ablegen: unsichtbar, die App laeuft beim Symbol weiter.
    fn fenster_verbergen(&mut self) {
        self.verborgen = true;
        self.hud_offen = false;
        self.esc_seit = None;
        self.esc_verbraucht = false;
        let Some(w) = self.window.clone() else { return };
        // macOS: aus dem Vollbild (eigener Space) erst heraus, verborgen wird
        // danach in about_to_wait.
        #[cfg(target_os = "macos")]
        if w.fullscreen().is_some() {
            w.set_fullscreen(None);
            self.verbergen_faellig = Some(Instant::now() + VOLLBILD_VERLASSEN);
            return;
        }
        w.set_visible(false);
        #[cfg(target_os = "macos")]
        host_mac::aktivierung(false);
    }

    /// Schliessen des Fensters (X, Alt+F4, Cmd+W): ablegen statt beenden,
    /// siehe tray::beim_schliessen. Eine laufende Sitzung wird getrennt wie
    /// mit "Trennen" - kein Zuschauer, keine Arbeit.
    fn schliessen(&mut self, el: &ActiveEventLoop) {
        let steht = self.symbol.as_ref().map(|s| s.steht()).unwrap_or(false);
        let sitzung = self.screen == Screen::Session;
        match tray::beim_schliessen(self.cfg.tray, steht, sitzung, self.cfg.tray_hinweis) {
            tray::Schliessen::Beenden => {
                // Ab hier uebernimmt die Schleife keine Weitergabe mehr (user_event).
                self.quit = true;
                el.exit();
            }
            tray::Schliessen::Ablegen { trennen, hinweis } => {
                if trennen {
                    protokoll::zeile(format!("{}: Fenster geschlossen - trenne {}", tray::ORT, self.addr_input));
                    self.verbindung_trennen();
                }
                self.fenster_verbergen();
                protokoll::zeile(format!("{}: Fenster abgelegt, QuadChroma laeuft weiter", tray::ORT));
                // Vermerkt wird der Hinweis nur, wenn er wirklich zu sehen
                // war - sonst kommt er beim naechsten Ablegen noch einmal.
                if hinweis {
                    let gezeigt = self.symbol.as_mut().is_some_and(|s| s.hinweis("QuadChroma", self.lang.get(tray::HINWEIS)));
                    if gezeigt {
                        self.cfg.tray_hinweis = true;
                        self.cfg.sichern();
                    } else {
                        protokoll::zeile(format!("{}: Hinweis nicht gezeigt - kommt beim naechsten Ablegen noch einmal", tray::ORT));
                    }
                }
            }
        }
    }

    /// Eine Wahl am Symbol.
    fn tray_befehl(&mut self, el: &ActiveEventLoop, befehl: tray::Befehl) {
        match befehl {
            tray::Befehl::Oeffnen => self.fenster_zeigen(el),
            tray::Befehl::Verbinden(adresse) => {
                // Wie ein Klick auf die Hostzeile (mit der ID, die der Host
                // unter dieser Adresse meldet): eine Sitzung zum selben Host
                // bleibt, eine zu einem anderen wird vorher getrennt.
                self.fenster_zeigen(el);
                let id = self
                    .hosts
                    .lock()
                    .map(|h| h.liste())
                    .unwrap_or_default()
                    .into_iter()
                    .find(|g| g.host.addr.to_string() == adresse)
                    .and_then(|g| g.id);
                let ziel = self.ziel(&adresse, id).unwrap_or_else(|_| Ziel { adresse: adresse.clone(), ..Ziel::default() });
                let sitzung = (self.screen == Screen::Session).then_some(self.addr_input.as_str());
                match einzel_folge(sitzung, &ziel.adresse) {
                    EinzelFolge::NachVorn => {}
                    EinzelFolge::Bleibt(a) => {
                        protokoll::zeile(format!("{}: Verbinden mit {a} - Sitzung dorthin laeuft schon", tray::ORT));
                    }
                    EinzelFolge::Verbinden(a) => {
                        protokoll::zeile(format!("{}: verbinde mit {a}", tray::ORT));
                        self.verbinden(ziel);
                    }
                    EinzelFolge::Wechseln(a) => {
                        protokoll::zeile(format!("{}: trenne {} und verbinde mit {a}", tray::ORT, self.addr_input));
                        self.verbindung_trennen();
                        self.verbinden(ziel);
                    }
                }
            }
            tray::Befehl::Beenden => {
                protokoll::zeile(format!("{}: Beenden", tray::ORT));
                self.quit = true;
            }
        }
    }

    /// Name bzw. Adresse der laufenden Sitzung fuer den Tooltip (None auf
    /// dem Startbildschirm).
    fn sitzung_anzeige(&self) -> Option<String> {
        (self.screen == Screen::Session).then(|| {
            let name = self.host_name(&self.addr_input);
            if name.is_empty() { self.addr_input.clone() } else { name }
        })
    }

    /// Was der Client zum Menue der Menueleiste beitraegt (macOS): Haken
    /// der Freigabe und des Ruhezustands, bis zu vier gefundene Hosts (Name
    /// entschaerft, sonst leer), Tooltip, laufende Sitzung.
    #[cfg(target_os = "macos")]
    fn symbol_stand(&self) -> host_mac::Stand {
        let hosts = self
            .hosts
            .lock()
            .map(|h| h.liste())
            .unwrap_or_default()
            .into_iter()
            .take(tray::HOSTS_MAX)
            .map(|g| (tray::anzeigename(&g.host.name), g.host.addr.to_string()))
            .collect();
        host_mac::Stand {
            freigabe: self.cfg.freigabe,
            ruhe: self.cfg.ruhe_verhindern,
            sitzung: self.screen == Screen::Session,
            tooltip: self.tooltip_jetzt(),
            hosts,
        }
    }

    /// Der Tooltip der einen App jetzt: in einer Sitzung der Host, sonst mit
    /// Freigabe die eigene ID.
    fn tooltip_jetzt(&self) -> String {
        let an = self.cfg.freigabe && self.rolle.is_some();
        symbolmenue::tooltip(self.lang, self.sitzung_anzeige().as_deref(), an, self.eigene_id())
    }

    /// Protokollzeile zum frisch angelegten Symbol.
    fn symbol_gemeldet(s: &tray::Symbol) {
        if s.steht() {
            protokoll::zeile(format!("{}: Symbol angelegt", tray::ORT));
        } else {
            protokoll::zeile(format!(
                "{}: Symbol (noch) nicht angemeldet ({}) - Schliessen beendet, bis es steht",
                tray::ORT,
                s.grund().unwrap_or_default()
            ));
        }
    }

    /// Das Symbol (macOS): das der Host-Engine, angelegt mit
    /// qc_oberflaeche_fertig in resumed; hier bekommt es den Stand des
    /// Clients. Seine Wahlen kommen ueber host_mac als Benutzerereignisse.
    #[cfg(target_os = "macos")]
    fn symbol_anlegen(&mut self) {
        let mut s = host_mac::Symbol::neu();
        s.stand_setzen(&self.symbol_stand());
        App::symbol_gemeldet(&s);
        self.symbol = Some(s);
    }

    /// Ohne Windows und macOS gibt es kein Symbol.
    #[cfg(not(any(windows, target_os = "macos")))]
    fn symbol_anlegen(&mut self) {}

    /// Das eine Symbol der App (Windows): Linksklick oeffnet das Fenster,
    /// Rechtsklick das Menue (symbolmenue, bei jedem Oeffnen frisch gebaut -
    /// auch ohne Fenster, waehrend der Fensterfaden wartet). Die Wahl kommt
    /// als Benutzerereignis; WM_CLOSE von aussen und WM_ENDSESSION beenden
    /// die App, der Abschied an einen Zuschauer geht vorher im Symbolfaden
    /// hinaus.
    #[cfg(windows)]
    fn symbol_anlegen(&mut self) {
        let sperre = |m: &Mutex<symbolmenue::Zuordnung>| m.lock().map(|z| z.clone()).unwrap_or_default();
        let (proxy, z) = (Mutex::new(self.proxy.clone()), self.menue_zuordnung.clone());
        let befehl = Box::new(move |nr: u32| {
            let a = symbolmenue::aktion_zu(nr, &sperre(&z));
            if let (Some(a), Ok(p)) = (a, proxy.lock()) {
                let _ = p.send_event(Benutzer::Menue(a));
            }
        });
        let (quelle, hosts, z) = (self.menue_quelle.clone(), self.hosts.clone(), self.menue_zuordnung.clone());
        let host_teil = self.rolle.as_ref().map(|r| r.menue_abfrage());
        let menue = Box::new(move || {
            let (lang, ruhe_verhindern) = quelle.lock().map(|q| (q.lang, q.ruhe_verhindern)).unwrap_or((&strings::EN, false));
            let h = match &host_teil {
                Some(f) => f(),
                None => symbolmenue::HostTeil { freigabe: symbolmenue::Freigabe::Aus, id: None, passwort: Err(()), geraete: Err(()) },
            };
            let hosts = hosts
                .lock()
                .map(|h| h.liste())
                .unwrap_or_default()
                .into_iter()
                .map(|g| (g.host.name, g.host.addr.to_string()))
                .collect();
            let stand = symbolmenue::MenueStand {
                art: symbolmenue::Art::App,
                name: zugang::geraetename(),
                freigabe: h.freigabe,
                id: h.id,
                passwort: h.passwort,
                geraete: h.geraete,
                hosts,
                autostart: verknuepfung::autostart_an(None),
                ruhe_verhindern,
            };
            let (m, neu) = symbolmenue::menue(lang, &stand);
            if let Ok(mut alt) = z.lock() {
                *alt = neu;
            }
            m
        });
        let ende_proxy = Mutex::new(self.proxy.clone());
        let ende = Box::new(move |wie: &str| {
            // Nach WM_ENDSESSION endet der Prozess womoeglich gleich nach der
            // Rueckkehr: der Abschied muss vorher hinaus.
            freigabe::abschied_beim_prozessende();
            if let Ok(p) = ende_proxy.lock() {
                let _ = p.send_event(Benutzer::Ende(wie.to_string()));
            }
        });
        match tray::Symbol::neu(befehl, menue, ende, Some(symbolmenue::LINKSKLICK), &self.tooltip_jetzt()) {
            Ok(s) => {
                App::symbol_gemeldet(&s);
                let _ = self.symbol_steht.set(s.steht_abfrage());
                self.symbol = Some(s);
            }
            Err(e) => protokoll::zeile(format!("{}: kein Symbol ({e}) - Schliessen beendet das Programm", tray::ORT)),
        }
    }

    /// Etwa zweimal je Sekunde: Stand des Symbols erneuern (nur Aenderungen
    /// gehen hinaus) und dem Symbol seinen Takt geben.
    fn symbol_nachfuehren(&mut self) {
        if self.symbol.is_none() || self.tray_takt.elapsed() < TRAY_TAKT {
            return;
        }
        self.symbol_nachfuehren_jetzt();
    }

    /// Stand des Symbols sofort erneuern (nach einem Umschalten).
    fn symbol_nachfuehren_jetzt(&mut self) {
        self.tray_takt = Instant::now();
        #[cfg(windows)]
        {
            if let Ok(mut q) = self.menue_quelle.lock() {
                q.lang = self.lang;
                q.ruhe_verhindern = self.cfg.ruhe_verhindern;
            }
            let t = self.tooltip_jetzt();
            if let Some(s) = self.symbol.as_mut() {
                s.tooltip_setzen(&t);
                s.takt();
            }
        }
        #[cfg(target_os = "macos")]
        {
            let stand = self.symbol_stand();
            if let Some(s) = self.symbol.as_mut() {
                s.stand_setzen(&stand);
                s.takt();
            }
        }
    }

    /// Desktop-Verknuepfung fuer einen Host anlegen; das Ergebnis steht 6 s
    /// im Meldungsbereich bzw. im Reiter. Laeuft im Fensterfaden: dort hat
    /// winit COM (STA) schon eingerichtet.
    fn verknuepfung_anlegen(&mut self, adresse: &str, id: Option<u32>, name: &str) {
        use strings::Key::*;
        let adresse = adresse_vollstaendig(adresse);
        #[cfg(windows)]
        let ergebnis = verknuepfung::verknuepfung_anlegen(None, &adresse, id, name, self.lang);
        #[cfg(not(windows))]
        let ergebnis: Result<std::path::PathBuf, String> = {
            let _ = (name, id);
            Err("nur unter Windows".into())
        };
        let m = match ergebnis {
            Ok(p) => {
                let zeile = format!("Verknuepfung angelegt: {}", p.display());
                protokoll::zeile(zeile.clone());
                let datei = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Meldung::neu(DesktopShortcutCreated, zeile).mit("{n}", datei)
            }
            Err(e) => {
                let zeile = format!("Verknuepfung fuer {adresse} nicht angelegt: {e}");
                protokoll::zeile(zeile.clone());
                Meldung::neu(DesktopShortcutFailed, zeile).mit("{n}", e)
            }
        };
        self.verknuepfung_meldung = Some((m, Instant::now()));
    }

    /// Das Ergebnis der letzten Verknuepfung als Text und Farbe, solange es
    /// stehen soll: angelegt in Cyan, gescheitert in Amber.
    fn verknuepfung_hinweis(&self) -> Option<(String, u32)> {
        let (m, seit) = self.verknuepfung_meldung.as_ref()?;
        if seit.elapsed() >= VERKNUEPFUNG_ANZEIGE {
            return None;
        }
        let farbe = if m.key == strings::Key::DesktopShortcutFailed { ui::AMBER } else { ui::CYAN };
        Some((m.text(self.lang), farbe))
    }

    /// Der Name, unter dem sich ein Host mit dieser Adresse gerade meldet -
    /// sonst leer (dann steht die Adresse im Dateinamen).
    fn host_name(&self, adresse: &str) -> String {
        let a = adresse_vollstaendig(adresse);
        self.hosts
            .lock()
            .map(|h| h.list())
            .unwrap_or_default()
            .into_iter()
            .find(|h| h.addr.to_string().eq_ignore_ascii_case(&a))
            .map(|h| h.name)
            .unwrap_or_default()
    }

    /// Steht ein Bild im Fenster? Solange nicht, zeigt die Sitzung den
    /// Wartebildschirm, und Maus und Tastatur bleiben beim Client.
    fn bild_vorhanden(&self) -> bool {
        self.last_frame.is_some() || self.bild_da.is_some()
    }

    /// Liegt gerade etwas ueber dem Bild, das sich bewegt und deshalb alle
    /// 33 ms neu gezeichnet werden will? Startbildschirm, Wartebild, Menue,
    /// Statistik, ESC-Balken, Lagemeldung bzw. Zeile zu Dateien, Banner,
    /// Codecwechsel-Hinweis.
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
            #[cfg(any(windows, target_os = "macos"))]
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
    #[cfg(any(windows, target_os = "macos"))]
    fn draw_gpu(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let Anzeige::Gpu(mut g) = std::mem::replace(&mut self.anzeige, Anzeige::Keine) else { return };
        match self.draw_gpu_auf(&window, &mut g) {
            // Mac: am sichtbaren Fenster kam nie ein Bild auf dem Schirm an
            // (anzeige_mac.rs, Waechter) - dann waere auch die Oberflaeche
            // samt Knopf "Prozessor" unsichtbar. softbuffer uebernimmt.
            #[cfg(target_os = "macos")]
            Ok(()) if g.nie_auf_dem_schirm() => {
                drop(g);
                self.auf_software_wechseln("Metal zeigt nichts - am sichtbaren Fenster kam kein Bild auf dem Schirm an");
            }
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
    #[cfg(any(windows, target_os = "macos"))]
    fn gpu_fehler_melden(&mut self, e: String) {
        if self.letzter_gpu_fehler.as_deref() != Some(e.as_str()) {
            protokoll::zeile(format!("Anzeige: {e}"));
            self.letzter_gpu_fehler = Some(e);
        }
    }

    /// Ein Fehler eines Aufrufs: war es das Geraet? Dann ist Schluss mit
    /// dieser Karte; sonst nur eine Protokollzeile, und es geht weiter.
    #[cfg(any(windows, target_os = "macos"))]
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
    #[cfg(any(windows, target_os = "macos"))]
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
        // Unter Windows nur bei neuer Groesse; auf dem Mac jedes Mal, denn
        // dort zaehlt auch der Massstab des Bildschirms (billig, wenn sich
        // nichts geaendert hat).
        if cfg!(target_os = "macos") || (ww, wh) != (g.breite, g.hoehe) {
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
                Bild::Roh { bild, .. } => g.bild_roh(&bild).map(|_| (Ebenenbild::breite(&bild), Ebenenbild::hoehe(&bild))),
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
            (s.error_key.is_some() || s.datei_stand.sichtbar(jetzt), s.wechsel_laeuft() || s.bildschirm_wechsel_laeuft())
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
    /// und der Nutzer bekommt gesagt, wie es weitergeht. Unter Windows wird
    /// nicht auf softbuffer am selben Fenster gewechselt: das ist von DXGI
    /// nicht gedeckt. Auf dem Mac schon - Metal haengt dort als eigene
    /// Unterschicht an der Ansicht, die mit dem Drop herausfaellt, und
    /// softbuffer haengt seine an (--anzeige-selbsttest prueft das).
    #[cfg(any(windows, target_os = "macos"))]
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
                #[cfg(windows)]
                let adapter = self.adapter_index();
                #[cfg(not(windows))]
                let adapter = None;
                let bau = gpu_am_fenster(&w, self.anzeige_wunsch == W::Warp, adapter);
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
        #[cfg(target_os = "macos")]
        self.auf_software_wechseln(&format!("Grafikkarte verloren, kein Neubau: {grund}"));
        #[cfg(windows)]
        self.anzeige_aufgeben();
    }

    /// Keine Anzeige mehr: das Fenster bleibt stehen, Titel und Meldung sagen,
    /// wie es weitergeht (Neustart mit --anzeige cpu).
    #[cfg(any(windows, target_os = "macos"))]
    fn anzeige_aufgeben(&mut self) {
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

    /// Mac: von Metal auf softbuffer am selben Fenster - wenn der Waechter
    /// anschlaegt (kein Bild kam je auf dem Schirm an) oder die Karte weg ist
    /// und kein Neubau gelingt. Die Metal-Schicht ist mit dem Drop der
    /// Anzeige schon aus der Ansicht; softbuffer haengt seine eigene an. Gilt
    /// fuer diesen Lauf, der naechste Start versucht Metal wieder. Ab jetzt
    /// legt der Empfangsfaden fertige RGB-Bilder ab; ein rohes, das schon
    /// liegt, wandelt draw_cpu_auf. Das Bild, das nur die Karte hatte, ist
    /// weg - bis zum naechsten vom Host steht der Wartebildschirm.
    #[cfg(target_os = "macos")]
    fn auf_software_wechseln(&mut self, grund: &str) {
        use einstellungen::AnzeigeWunsch as W;
        protokoll::zeile(format!("Anzeige: Rueckfall auf Software: {grund}"));
        self.bild_da = None;
        self.bereit_ausstehend = None;
        self.ui_kasten_alt = None;
        self.ui_an = false;
        self.praesentation_ausstehend = false;
        self.sofort = false;
        self.shared.lock().unwrap().gpu_pfad = false;
        let Some(window) = self.window.clone() else {
            self.anzeige = Anzeige::Keine;
            return;
        };
        let bau = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()).map(|surface| (context, surface)));
        match bau {
            Ok((context, surface)) => {
                self.anzeige = Anzeige::Cpu { context, surface };
                self.anzeige_aktiv = W::Cpu;
                self.anzeige_name = "Software".into();
                protokoll::zeile("Anzeige: Software".into());
                // Wie beim Start: still bei Automatik, sonst mit Meldung.
                if !matches!(self.anzeige_wunsch, W::Automatik | W::Warp) {
                    let m = Meldung::neu(strings::Key::ErrorGpuDisplay, format!("Metal nicht nutzbar, Anzeige ueber Software: {grund}"));
                    self.shared.lock().unwrap().error = Some(m);
                }
                window.request_redraw();
            }
            Err(e) => {
                protokoll::zeile(format!("Anzeige: softbuffer am selben Fenster gescheitert: {e}"));
                self.anzeige_aufgeben();
            }
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
                    ..to_rgb(&bild).unwrap_or_else(|_| dunkles_bild(bild.breite(), bild.hoehe()))
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

    /// Alles, was ueber dem Bild liegt: Startbildschirm, Wartebildschirm samt
    /// Zugangsdialog, Lagemeldung, Statistik, Zeile zu Dateien, Menue,
    /// Codecwechsel-Hinweis, ESC-Balken. Zeichnet nur; was ein Klick bewirkt, kommt als
    /// Nachwirkung zurueck und wird NACH dem Praesentieren ausgefuehrt.
    fn oberflaeche_zeichnen(&mut self, c: &mut ui::Canvas, ww: u32, wh: u32) -> Nachwirkung {
        let mut n = Nachwirkung { act: Action::None, hud: None, abbrechen: false, zugang: ZugangAktion::Nichts };
        match self.screen {
            Screen::Start => {
                self.bekannte_nachladen(false);
                let gefunden = self.hosts.lock().map(|h| h.liste()).unwrap_or_default();
                let zeilen = hostzeilen(&gefunden, &self.bekannte);
                let err = {
                    let s = self.shared.lock().unwrap();
                    match s.error_key {
                        Some(k) => Some(self.lang.get(k).to_string()),
                        None => s.error.as_ref().map(|m| m.text(self.lang)),
                    }
                };
                let hinweis = self.verknuepfung_hinweis();
                let name = zugang::geraetename();
                let dieser = (MIT_FREIGABE && self.rolle.is_some()).then(|| DieserComputer {
                    freigabe: self.cfg.freigabe,
                    name: &name,
                    id: self.eigene_id(),
                    ruhe_verhindern: self.cfg.ruhe_verhindern,
                });
                n.act = start_screen(
                    &mut self.ui, c, self.lang, &zeilen, &self.addr_input, err.as_deref(),
                    hinweis.as_ref().map(|(t, f)| (t.as_str(), *f)), self.sprachwahl, dieser.as_ref(),
                );
            }
            Screen::Session => {
                // Noch kein Bild da: Wartebildschirm zeichnen statt gar nichts.
                if !self.bild_vorhanden() {
                    let (stand, dialog) = {
                        let s = self.shared.lock().unwrap();
                        let f = match s.error_key {
                            Some(k) => Some(self.lang.get(k).to_string()),
                            None => s.error.as_ref().map(|m| m.text(self.lang)),
                        };
                        ((s.connected, f, s.sas.clone(), s.info.is_some()), s.zugang.clone())
                    };
                    // Offener Zugangsdialog: er liegt ueber allem und bekommt
                    // Maus und Klick allein (wie die Sprachwahl).
                    let (maus, klick) = (self.ui.mouse, self.ui.click);
                    if dialog.is_some() {
                        self.ui.mouse = (-10_000, -10_000);
                        self.ui.click = false;
                    }
                    let adresse = match self.id_ausstehend {
                        Some((id, _)) => self.lang.get(strings::Key::StartId).replace("{i}", &zugang::id_text(id)),
                        None => self.addr_input.clone(),
                    };
                    n.abbrechen = warte_screen(&mut self.ui, c, self.lang, &adresse, stand);
                    self.ui.mouse = maus;
                    self.ui.click = klick;
                    if let Some(d) = dialog {
                        // Nach "falsch" ist das Feld wieder leer.
                        if self.zugang_runde != Some(d.runde) {
                            if self.zugang_runde.is_some() {
                                self.zugang_pw.clear();
                                self.zugang_caret = 0;
                            }
                            self.zugang_runde = Some(d.runde);
                        }
                        n.zugang = zugang_zeichnen(
                            &mut self.ui, c, self.lang, &d, &self.zugang_pw, self.zugang_caret, self.zugang_zeigen, Instant::now(),
                        );
                    }
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
                // Das Erstkontakt-Banner mit dem Vergleichscode entfaellt
                // (Spezifikation Pairing v1, 5): den Code zeigt der
                // Zugangsdialog, solange er etwas entscheidet - beim Zulassen
                // am Host. Danach steht er im Wartebild, in der Statistik und
                // im Reiter "Verschluesselung".
                // Dateien: schmale Zeile unten mittig, solange eine
                // Uebertragung laeuft und 6 s nach ihrem Ergebnis.
                let zeilen: Vec<(String, u32)> = {
                    let s = self.shared.lock().unwrap();
                    s.datei_stand.sichtbare(Instant::now()).into_iter().map(|st| datei_zeile(st, self.lang)).collect()
                };
                if !zeilen.is_empty() {
                    datei_zeilen_zeichnen(&mut self.ui, c, ww, wh, &zeilen);
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
                    let (bildschirmwahl, bildschirme, bildschirm_wunsch, bildschirm_wechsel) = {
                        let sh = self.shared.lock().unwrap();
                        (sh.host_bildschirmwahl, sh.bildschirme.clone(), sh.bildschirm_wunsch.clone(), sh.bildschirm_wechsel_laeuft())
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
                        bildschirmwahl,
                        bildschirme,
                        bildschirm_wunsch,
                        bildschirm_wechsel,
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
                        verknuepfung: self.verknuepfung_hinweis(),
                    };
                    n.hud = Some(hud(
                        &mut self.ui, c, self.lang, ww as i32, wh as i32, reiter,
                        lat, &lhist, fps_jetzt, &hist, info, stell, secure, &adresse, gespeichert,
                        &stand,
                    ));
                    return n;
                }

                // Codec- oder Bildschirmwechsel unterwegs: der Host baut den
                // Encoder um bzw. startet den Strom neu, das Bild steht ein
                // paar hundert Millisekunden. Ohne Hinweis saehe das nach
                // einem Haenger aus.
                let hinweis = {
                    let s = self.shared.lock().unwrap();
                    if s.wechsel_laeuft() {
                        Some(strings::Key::CodecSwitching)
                    } else if s.bildschirm_wechsel_laeuft() {
                        Some(strings::Key::ScreenSwitching)
                    } else {
                        None
                    }
                };
                if let Some(k) = hinweis {
                    let t = self.lang.get(k);
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
            Action::Connect(text) => self.eingabe_verbinden(&text),
            Action::Host(ziel) => self.verbinden(ziel),
            Action::Verknuepfung { adresse, id, name } => self.verknuepfung_anlegen(&adresse, id, &name),
            Action::FreigabeUmschalten => self.freigabe_umschalten(),
            Action::NameAendern => self.geraetename_fenster(),
            Action::RuheUmschalten => self.ruhe_umschalten(),
            Action::Quit => self.quit = true,
            Action::Sprachwahl => self.sprachwahl = !self.sprachwahl,
            Action::Sprache(code) => {
                // Gewaehlt wird aus der Liste, gemerkt in einstellungen.txt -
                // vorher galt nach jedem Start wieder die Systemsprache.
                self.lang = strings::pick(code);
                self.sprachwahl = false;
                self.cfg.sprache = Some(code.to_string());
                self.cfg.sichern();
                // Die Menueleiste spricht dieselbe Sprache.
                #[cfg(target_os = "macos")]
                {
                    host_mac::sprache_setzen(code);
                    if let Some(s) = self.symbol.as_mut() {
                        s.neu_zeigen();
                    }
                    self.symbol_nachfuehren_jetzt();
                }
            }
            Action::Website => website_oeffnen(),
            Action::None => {}
        }
        match n.zugang {
            ZugangAktion::Nichts => {}
            ZugangAktion::Senden => self.zugang_senden(),
            ZugangAktion::Abbrechen => self.zugang_abbrechen(),
            ZugangAktion::Zeigen => self.zugang_zeigen = !self.zugang_zeigen,
            ZugangAktion::Marke(i) => self.zugang_caret = i,
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
            HudAktion::Verknuepfung => {
                // Name aus der Bekanntgabe, falls die Adresse passt, sonst
                // steht die Adresse im Dateinamen. Die ID ist die des
                // verbundenen Hosts - aus seinem Schluessel.
                let adresse = self.addr_input.clone();
                let name = self.host_name(&adresse);
                let id = self.shared.lock().unwrap().link.as_ref().map(|(_, k)| zugang::geraete_id(k));
                self.verknuepfung_anlegen(&adresse, id, &name);
            }
            HudAktion::Stellen(m, f, g, fx, ton) => self.stellen(m, f, g, fx, ton),
            HudAktion::Codec(idx) => self.codec_wuenschen(idx),
            HudAktion::Bildschirm(w) => self.bildschirm_wuenschen(w),
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
    /// Zugangsdialog, falls er offen war.
    zugang: ZugangAktion,
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
    /// Was im Adressfeld steht: Adresse, Name oder Geraete-ID (9.2).
    Connect(String),
    /// Klick auf eine Hostzeile: Adresse, ID (falls der Host eine meldet) und Name.
    Host(Ziel),
    /// Desktop-Verknuepfung fuer diesen Host anlegen (nur Windows).
    Verknuepfung { adresse: String, id: Option<u32>, name: String },
    /// "Diesen PC freigeben" (die eine App): die Freigabe umschalten.
    FreigabeUmschalten,
    /// "Umbenennen" neben "Dieser Computer": das Fenster "Geraetename".
    NameAendern,
    /// Das Kaestchen "Ruhezustand verhindern".
    RuheUmschalten,
    Quit,
    /// Sprachwahl oeffnen bzw. schliessen.
    Sprachwahl,
    /// Diese Sprache nehmen (Code wie in strings.rs).
    Sprache(&'static str),
    Website,
}

/// Wohin verbunden wird: Adresse mit Port; beim Verbinden ueber eine ID
/// (Liste, Eingabe, Verknuepfung) die ID, die der Handschlag am Schluessel
/// prueft (8.2); und der Name aus Bekanntgabe bzw. hosts.txt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Ziel {
    adresse: String,
    id: Option<u32>,
    name: Option<String>,
}

/// Ein Ziel aus Adresse und/oder ID (Spezifikation Pairing v1, 9.2). Mit ID
/// zuerst der Host, der sich im Netz damit meldet, sonst die gegebene
/// Adresse (Verknuepfung), sonst die zuletzt bekannte aus hosts.txt - sonst
/// "nicht gefunden". Ohne ID die Adresse mit ergaenztem Port, samt dem
/// Namen, unter dem sie sich meldet.
fn ziel_bilden(adresse: &str, id: Option<u32>, gefunden: &[discovery::Gefunden], bekannte: &zugang::Hostliste) -> Result<Ziel, Meldung> {
    let adresse = adresse.trim();
    let name_bei =
        |a: &str| gefunden.iter().find(|g| g.host.addr.to_string().eq_ignore_ascii_case(a)).map(|g| g.host.name.clone());
    let Some(id) = id else {
        let a = adresse_vollstaendig(adresse);
        return Ok(Ziel { name: name_bei(&a), adresse: a, id: None });
    };
    if let Some(g) = gefunden.iter().filter(|g| g.id == Some(id)).max_by_key(|g| g.host.seen) {
        return Ok(Ziel { adresse: g.host.addr.to_string(), id: Some(id), name: Some(g.host.name.clone()) });
    }
    let bekannt = bekannte.nach_id(id);
    if !adresse.is_empty() {
        let a = adresse_vollstaendig(adresse);
        let name = name_bei(&a).or_else(|| bekannt.map(|h| h.name.clone()));
        return Ok(Ziel { adresse: a, id: Some(id), name });
    }
    match bekannt {
        Some(h) => Ok(Ziel { adresse: h.adresse.clone(), id: Some(id), name: Some(h.name.clone()) }),
        None => Err(Meldung::neu(
            strings::Key::MsgIdNotFound,
            format!("Verbinden: ID {} meldet sich nicht im Netz und steht nicht in hosts.txt", zugang::id_text(id)),
        )
        .mit("{i}", zugang::id_text(id))),
    }
}

/// Selbstschutz beim Eintippen: ist die Eingabe die ID des Hosts auf diesem
/// Rechner (`eigene`), gibt es statt einer Suche die Meldung "Das ist dieser
/// Computer." - die eigene Bekanntgabe blendet die Liste ohnehin aus, man
/// saehe sonst "nicht gefunden".
fn eigene_id_eingegeben(text: &str, eigene: Option<u32>) -> Option<Meldung> {
    let id = zugang::id_lesen(text)?;
    (Some(id) == eigene).then(|| {
        Meldung::neu(
            strings::Key::MsgSelf,
            format!("Verbinden: ID {} ist dieser Rechner selbst - kein Verbinden zu sich selbst", zugang::id_text(id)),
        )
    })
}

/// Was im Adressfeld steht: neun Ziffern (mit oder ohne Leerzeichen) sind
/// eine Geraete-ID, alles andere eine Adresse oder ein Name.
fn ziel_aus_eingabe(text: &str, gefunden: &[discovery::Gefunden], bekannte: &zugang::Hostliste) -> Result<Ziel, Meldung> {
    match zugang::id_lesen(text) {
        Some(id) => ziel_bilden("", Some(id), gefunden, bekannte),
        None => ziel_bilden(text, None, gefunden, bekannte),
    }
}

/// Was im Zugangsdialog geklickt wurde.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZugangAktion {
    Nichts,
    Senden,
    Abbrechen,
    /// "Anzeigen": Passwort lesbar bzw. wieder als Punkte.
    Zeigen,
    /// Klick ins Feld: die Schreibmarke an diese Stelle (in Zeichen).
    Marke(usize),
}

/// Abstand der Punkte im maskierten Passwortfeld.
const PUNKT_ABSTAND: i32 = 14;

/// Ein Punkt des maskierten Passworts, Mitte bei (x, y).
fn punkt(c: &mut ui::Canvas, x: i32, y: i32, farbe: u32) {
    for dy in -4i32..=4 {
        for dx in -4i32..=4 {
            let d = dx * dx + dy * dy;
            if d <= 12 {
                c.px(x + dx, y + dy, farbe, 255);
            } else if d <= 20 {
                c.px(x + dx, y + dy, farbe, 110);
            }
        }
    }
}

/// Der Zugangsdialog (Spezifikation Pairing v1, 9.3) - modal wie die
/// Sprachwahl: ein Schleier ueber allem, darunter reagiert nichts (der
/// Aufrufer zeichnet es blind). Titel, Erklaerung, bei neuer Identitaet der
/// Hinweis dazu, bei "Zulassen" die Bitte samt Vergleichscode, das
/// Passwortfeld (Punkte oder lesbar, Schreibmarke, Knopf "Anzeigen"), der
/// Stand (wird geprueft, falsch, Wartezeit mit Countdown) und die Knoepfe
/// Verbinden und Abbrechen. Verbinden geht nur mit Passwort und nach der
/// Wartezeit; ein Klick neben die Tafel tut nichts.
#[allow(clippy::too_many_arguments)]
fn zugang_zeichnen(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    d: &zugangsphase::Dialog,
    pw: &str,
    marke: usize,
    zeigen: bool,
    jetzt: Instant,
) -> ZugangAktion {
    use strings::Key::*;
    let (w, h) = (c.w as i32, c.h as i32);
    let tw = 560.min(w - 32);
    let rand = 28;
    let innen = tw - 2 * rand;
    let titel = kuerzen(u, &lang.get(AccessTitle).replace("{n}", &d.name), innen, 18, 1);
    let text = umbruch(u, &lang.get(AccessText).replace("{n}", &d.name), innen, 13);
    // Die ID mit geschuetzten Leerzeichen: der Umbruch soll sie nicht
    // zerreissen.
    let neu = if d.neue_identitaet {
        umbruch(u, &lang.get(AccessNewIdentity).replace("{i}", &zugang::id_text(d.id).replace(' ', "\u{a0}")), innen, 13)
    } else {
        Vec::new()
    };
    let bitte = if d.zulassen { umbruch(u, &lang.get(AccessOrAllow).replace("{n}", &d.name), innen, 13) } else { Vec::new() };
    let warten = d.warten_s(jetzt);
    let mut stand: Vec<(String, u32)> = Vec::new();
    match d.lage {
        zugangsphase::Lage::Pruefen => stand.push((lang.get(AccessChecking).to_string(), ui::DIM)),
        zugangsphase::Lage::Falsch => stand.push((lang.get(AccessWrong).to_string(), ui::AMBER)),
        zugangsphase::Lage::Eingabe => {}
    }
    if warten > 0 {
        stand.push((lang.get(AccessWait).replace("{s}", &warten.to_string()), ui::AMBER));
    }
    // Hoehe aus dem Inhalt: der Text ist je Sprache verschieden lang.
    let zeile = 19;
    let mut th = 26 + 24 + 16;
    th += text.len() as i32 * zeile;
    if !neu.is_empty() {
        th += 8 + neu.len() as i32 * zeile;
    }
    if d.zulassen {
        th += 10 + bitte.len() as i32 * zeile + 30;
    }
    th += 34 + 40 + 14 + 2 * zeile + 18 + 44 + 26;
    let tx = (w - tw) / 2;
    let ty = ((h - th) / 2).max(8);
    c.fill(0, 0, w, h, ui::BG, 170);
    // Deckend unter der Tafel: das Wartebild darf nicht durchscheinen.
    c.fill(tx, ty, tw, th, ui::BG, 255);
    c.panel(tx, ty, tw, th, ui::CYAN);
    let x = tx + rand;
    let mut y = ty + 26 + 18;
    u.text.draw(c, x, y, &titel, 18, ui::CYAN, 1);
    y += 16;
    for z in &text {
        y += zeile;
        u.text.draw(c, x, y, z, 13, ui::TEXT, 1);
    }
    if !neu.is_empty() {
        y += 8;
        for z in &neu {
            y += zeile;
            u.text.draw(c, x, y, z, 13, ui::AMBER, 1);
        }
    }
    if d.zulassen {
        y += 10;
        for z in &bitte {
            y += zeile;
            u.text.draw(c, x, y, z, 13, ui::DIM, 1);
        }
        y += 30;
        u.text.draw(c, x, y, &lang.get(AccessCode).replace("{c}", &d.code), 17, ui::CYAN, 3);
    }

    // Passwortfeld: Beschriftung darueber wie bei Ui::field.
    y += 34;
    let feld = ui::Rect { x, y, w: innen, h: 40 };
    let mut aktion = ZugangAktion::Nichts;
    c.rect(feld.x, feld.y, feld.w, feld.h, 0x0a1018, 200);
    c.glow_hline(feld.x, feld.y + feld.h - 1, feld.w, ui::CYAN);
    u.text.draw(c, feld.x + 2, feld.y - 6, lang.get(AccessPassword), 11, ui::DIM, 2);
    // "Anzeigen" bzw. "Verbergen", solange das Passwort lesbar steht; die
    // Breite passt fuer beide, damit der Knopf beim Umschalten nicht springt.
    let sw = (u.text.width(lang.get(AccessShow), 12, 1).max(u.text.width(lang.get(AccessHide), 12, 1)) + 24).clamp(70, 150);
    let knopf = ui::Rect { x: feld.x + feld.w - sw - 4, y: feld.y + 4, w: sw, h: feld.h - 8 };
    let knopf_text = lang.get(if zeigen { AccessHide } else { AccessShow });
    if u.button_mit(c, knopf, knopf_text, if zeigen { ui::CYAN } else { ui::DIM }, 12, 1) {
        aktion = ZugangAktion::Zeigen;
    }
    // Wo jede Zeichengrenze liegt (in Bildpunkten ab Textanfang): Punkte
    // im festen Abstand, lesbar nach der Schrift.
    let zeichen: Vec<char> = pw.chars().collect();
    let marke = marke.min(zeichen.len());
    let grenzen: Vec<i32> = (0..=zeichen.len())
        .map(|i| {
            if zeigen {
                u.text.width(&zeichen[..i].iter().collect::<String>(), 16, 1)
            } else {
                i as i32 * PUNKT_ABSTAND
            }
        })
        .collect();
    let x0 = feld.x + 12;
    let platz = knopf.x - 8 - x0;
    // Der sichtbare Ausschnitt beginnt so, dass die Schreibmarke im Feld bleibt.
    let mut von = 0;
    while von < marke && grenzen[marke] - grenzen[von] > platz - 4 {
        von += 1;
    }
    let mitte = feld.y + feld.h / 2;
    for i in von..zeichen.len() {
        let rechts = grenzen[i + 1] - grenzen[von];
        if rechts > platz {
            break;
        }
        let links = x0 + grenzen[i] - grenzen[von];
        if zeigen {
            u.text.draw(c, links, mitte + 6, &zeichen[i].to_string(), 16, ui::TEXT, 1);
        } else {
            punkt(c, links + PUNKT_ABSTAND / 2 - 2, mitte, ui::TEXT);
        }
    }
    if (u.tick / 30) % 2 == 0 {
        c.rect(x0 + grenzen[marke] - grenzen[von], feld.y + 9, 2, feld.h - 18, ui::CYAN, 230);
    }
    let textfeld = ui::Rect { x: feld.x, y: feld.y, w: knopf.x - feld.x, h: feld.h };
    if u.click && textfeld.hit(u.mouse.0, u.mouse.1) {
        let mx = u.mouse.0 - x0 + grenzen[von];
        let naechste = (0..grenzen.len()).min_by_key(|&i| (grenzen[i] - mx).abs()).unwrap_or(0);
        aktion = ZugangAktion::Marke(naechste);
    }
    y += feld.h + 14;

    // Stand: wird geprueft, falsch, Wartezeit.
    for (t, farbe) in &stand {
        y += zeile;
        u.text.draw(c, x, y, t, 13, *farbe, 1);
    }
    y += (2 - stand.len() as i32).max(0) * zeile + 18;

    // Knoepfe
    let bw = (innen - 20) / 2;
    let verbinden = ui::Rect { x, y, w: bw, h: 44 };
    if d.darf_senden(jetzt) && !pw.is_empty() {
        if u.button(c, verbinden, lang.get(Connect), ui::CYAN) {
            aktion = ZugangAktion::Senden;
        }
    } else {
        knopf_aus(u, c, verbinden, lang.get(Connect), 15, 2);
    }
    if u.button(c, ui::Rect { x: x + bw + 20, y, w: innen - bw - 20, h: 44 }, lang.get(AccessCancel), ui::MAGENTA) {
        aktion = ZugangAktion::Abbrechen;
    }
    aktion
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

/// Hoehe der Hostliste auf dem Startbildschirm (vier Zeilen und Kopf).
const START_LISTE_H: i32 = 34 * 4 + 52;

/// Hoehe der zwei Zeilen der einen App unter den Knoepfen: "Dieser
/// Computer" mit "Umbenennen" und "Ruhezustand verhindern".
const EINE_APP_H: i32 = 52;

/// Lage der Hostliste auf dem Startbildschirm: (links, Breite, oben der
/// Tafel). Alles als Block mittig, damit unten kein totes Feld bleibt.
fn start_rahmen(w: i32, h: i32) -> (i32, i32, i32) {
    let cx = w / 2;
    let panel_w = 560.min(w - 60);
    let block_h = 150 + START_LISTE_H + 40 + 66 + 44 + if MIT_FREIGABE { EINE_APP_H } else { 0 };
    let top = ((h - block_h) / 2).max(24);
    (cx - panel_w / 2, panel_w, top + 130)
}

/// Schriftgroesse und Laufweite des kleinen Knopfs in der Hostzeile.
const ZEILENKNOPF_SCHRIFT: (u32, i32) = (12, 1);

/// Zeile `i` der Hostliste und - unter Windows - ihr Knopf fuer die
/// Desktop-Verknuepfung rechts daneben. Die Zeile wird um den Knopf
/// schmaler, damit ein Klick auf ihn nicht zugleich verbindet. Auch fuer
/// `--shot starttip`, das die Maus auf den Knopf der ersten Zeile stellt.
fn start_zeile(u: &mut ui::Ui, w: i32, h: i32, lang: &'static strings::Lang, i: usize) -> (ui::Rect, Option<ui::Rect>) {
    let (px, panel_w, py) = start_rahmen(w, h);
    let zeile = ui::Rect { x: px + 12, y: py + 44 + i as i32 * 34, w: panel_w - 24, h: 30 };
    if !MIT_VERKNUEPFUNG {
        return (zeile, None);
    }
    let (g, lw) = ZEILENKNOPF_SCHRIFT;
    let kw = (u.text.width(lang.get(strings::Key::DesktopShortcut), g, lw) + 28).clamp(84, 150);
    let knopf = ui::Rect { x: zeile.x + zeile.w - kw, y: zeile.y, w: kw, h: zeile.h };
    (ui::Rect { w: zeile.w - kw - 8, ..zeile }, Some(knopf))
}

/// Eine Zeile der Hostliste auf dem Startbildschirm (Spezifikation Pairing
/// v1, 9.1): Name links, rechts die ID (ohne ID "-"), bekannte Hosts mit
/// Haken, die Adresse im Tooltip.
#[derive(Clone, Debug, PartialEq)]
struct Hostzeile {
    name: String,
    adresse: String,
    id: Option<u32>,
    /// In hosts.txt gemerkt: ueber die ID, bei Hosts ohne ID ueber die Adresse.
    bekannt: bool,
}

/// Die Zeilen aus den Bekanntgaben und hosts.txt. Der Haken ist nur
/// Anzeige: vertraut wird im Handschlag dem Schluessel, nie der ID.
fn hostzeilen(gefunden: &[discovery::Gefunden], bekannte: &zugang::Hostliste) -> Vec<Hostzeile> {
    gefunden
        .iter()
        .map(|g| {
            let adresse = g.host.addr.to_string();
            let bekannt = match g.id {
                Some(id) => bekannte.nach_id(id).is_some(),
                None => bekannte.nach_adresse(&adresse).is_some(),
            };
            let name = if g.host.name.trim().is_empty() { adresse.clone() } else { g.host.name.clone() };
            Hostzeile { name, adresse, id: g.id, bekannt }
        })
        .collect()
}

/// Breite des Haken-Symbols samt Abstand in der Hostzeile.
const HAKEN_BREITE: i32 = 18;

/// Kleiner Haken (bekannter Host), links oben bei (x, y), etwa 11 x 9
/// Bildpunkte - gezeichnet, nicht aus der Schrift (nicht jede hat U+2713).
fn haken(c: &mut ui::Canvas, x: i32, y: i32, farbe: u32) {
    for i in 0..4 {
        c.px(x + i, y + 4 + i, farbe, 255);
        c.px(x + i, y + 5 + i, farbe, 255);
    }
    for i in 0..8 {
        c.px(x + 3 + i, y + 7 - i, farbe, 255);
        c.px(x + 3 + i, y + 8 - i, farbe, 255);
    }
}

/// Ein Kaestchen (14 x 14) links oben bei (x, y), mit Haken, wenn `an`.
fn kaestchen(c: &mut ui::Canvas, x: i32, y: i32, an: bool, farbe: u32) {
    c.rect(x, y, 14, 14, farbe, if an { 40 } else { 12 });
    c.hline(x, y, 14, farbe, 200);
    c.hline(x, y + 13, 14, farbe, 200);
    c.vline(x, y, 14, farbe, 200);
    c.vline(x + 13, y, 14, farbe, 200);
    if an {
        haken(c, x + 2, y + 2, farbe);
    }
}

/// Ein Knopf, der einen Zustand umschaltet (die Freigabe): dieselbe Form wie
/// `Ui::button_mit`, darin ein Kaestchen - mit Haken, wenn an - und der Text.
/// true, wenn er in diesem Bild geklickt wurde.
fn umschalter(u: &mut ui::Ui, c: &mut ui::Canvas, r: ui::Rect, label: &str, an: bool, size: u32, spacing: i32) -> bool {
    let geklickt = u.button_mit(c, r, "", ui::CYAN, size, spacing);
    let hot = r.hit(u.mouse.0, u.mouse.1);
    let t = kuerzen(u, label, r.w - 16 - 14 - 8, size, spacing);
    let tw = u.text.width(&t, size, spacing);
    let x0 = r.x + ((r.w - (14 + 8 + tw)) / 2).max(8);
    kaestchen(c, x0, r.y + r.h / 2 - 7, an, ui::CYAN);
    u.text.draw(c, x0 + 14 + 8, r.y + r.h / 2 + size as i32 / 3, &t, size, if hot { 0xffffff } else { ui::TEXT }, spacing);
    geklickt
}

/// Was der Startbildschirm der einen App zusaetzlich zeigt (Windows): den
/// Umschalter "Diesen PC freigeben", die Zeile "Dieser Computer" und das
/// Kaestchen "Ruhezustand verhindern".
struct DieserComputer<'a> {
    freigabe: bool,
    name: &'a str,
    id: Option<u32>,
    ruhe_verhindern: bool,
}

/// Die Zeilen der einen App unter den Knoepfen (ab `y`): links "Dieser
/// Computer: <Name> · <ID>", rechts "Umbenennen"; darunter das Kaestchen
/// "Ruhezustand verhindern" - die ganze Zeile ist klickbar.
fn dieser_computer_zeichnen(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    d: &DieserComputer,
    px: i32,
    panel_w: i32,
    y: i32,
) -> Action {
    use strings::Key::*;
    let mut action = Action::None;
    let (g, lw) = ZEILENKNOPF_SCHRIFT;
    let kw = (u.text.width(lang.get(DeviceNameRename), g, lw) + 28).clamp(84, 160).min(panel_w / 3);
    let knopf = ui::Rect { x: px + panel_w - kw, y, w: kw, h: 24 };
    let id = d.id.map(zugang::id_text).unwrap_or_else(|| "-".into());
    let text = symbolmenue::einsetzen(lang.get(ThisComputer), &[("{n}", &tray::anzeigename(d.name)), ("{i}", &id)]);
    let text = kuerzen(u, &text, knopf.x - px - 12, 13, 1);
    u.text.draw(c, px + 2, y + 17, &text, 13, ui::TEXT, 1);
    let t = kuerzen(u, lang.get(DeviceNameRename), kw - 16, g, lw);
    if u.button_mit(c, knopf, &t, ui::CYAN, g, lw) {
        action = Action::NameAendern;
    }
    let zeile = ui::Rect { x: px, y: y + 28, w: panel_w, h: 24 };
    let hot = zeile.hit(u.mouse.0, u.mouse.1);
    kaestchen(c, px + 2, zeile.y + 5, d.ruhe_verhindern, if hot || d.ruhe_verhindern { ui::CYAN } else { ui::DIM });
    let t = kuerzen(u, lang.get(PreventSleep), panel_w - 28, 13, 1);
    u.text.draw(c, px + 26, zeile.y + 17, &t, 13, if hot { 0xffffff } else { ui::TEXT }, 1);
    if hot && u.click {
        action = Action::RuheUmschalten;
    }
    action
}

/// Ein Knopf, der gerade nicht geht: dieselbe Form wie `Ui::button_mit`,
/// aber blass (Klammern und Schrift) und ohne Reaktion auf Maus und Klick.
fn knopf_aus(u: &mut ui::Ui, c: &mut ui::Canvas, r: ui::Rect, label: &str, size: u32, spacing: i32) {
    c.rect(r.x, r.y, r.w, r.h, ui::DIM, 8);
    let cut = 10;
    for (cx, cy, dx, dy) in [
        (r.x, r.y, 1, 1),
        (r.x + r.w - 1, r.y, -1, 1),
        (r.x, r.y + r.h - 1, 1, -1),
        (r.x + r.w - 1, r.y + r.h - 1, -1, -1),
    ] {
        for i in 0..cut {
            c.px(cx + dx * i, cy, ui::DIM, 150);
            c.px(cx, cy + dy * i, ui::DIM, 150);
        }
    }
    let ty = r.y + r.h / 2 + size as i32 / 3;
    u.text.draw_centered(c, r.x + r.w / 2, ty, label, size, ui::DIM, spacing);
}

/// Startbildschirm: Titel, gefundene Hosts, Adresse, Knoepfe. `hinweis` ist
/// das Ergebnis einer Desktop-Verknuepfung (Text, Farbe); es steht, solange
/// es gilt, im Meldungsbereich statt einer Fehlermeldung. `dieser`: nur in
/// der einen App (Windows) Some - der Umschalter "Diesen PC freigeben"
/// zwischen Verbinden und Beenden, darunter "Dieser Computer" mit
/// "Umbenennen" und das Kaestchen "Ruhezustand verhindern".
#[allow(clippy::too_many_arguments)]
fn start_screen(
    u: &mut ui::Ui,
    c: &mut ui::Canvas,
    lang: &'static strings::Lang,
    hosts: &[Hostzeile],
    addr: &str,
    error: Option<&str>,
    hinweis: Option<(&str, u32)>,
    sprachwahl: bool,
    dieser: Option<&DieserComputer>,
) -> Action {
    use strings::Key::*;
    c.backdrop(u.tick);
    // Offene Sprachwahl: sie liegt ueber allem und bekommt Maus und Klick
    // allein - darunter reagiert nichts, bis sie wieder zu ist.
    let (echte_maus, echter_klick) = (u.mouse, u.click);
    if sprachwahl {
        u.mouse = (-10_000, -10_000);
        u.click = false;
    }

    let cx = c.w as i32 / 2;
    let (px, panel_w, py) = start_rahmen(c.w as i32, c.h as i32);
    let list_h = START_LISTE_H;
    let top = py - 130;
    // Tooltip wie im Menue: gemerkt, wenn die Maus darueber steht, gezeichnet
    // ganz am Ende ueber allem.
    let maus = u.mouse;
    let mut tip: Option<String> = None;

    // Kopf
    u.text.draw_centered(c, cx, top + 52, "QUADCHROMA", 46, ui::CYAN, 10);
    c.glow_hline(cx - 160, top + 64, 320, ui::MAGENTA);
    u.text.draw_centered(c, cx, top + 88, lang.get(AppSubtitle), 13, ui::DIM, 2);

    // Hostliste
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
        let (r, knopf) = start_zeile(u, c.w as i32, c.h as i32, lang, i);
        let sel = addr == h.adresse || (h.id.is_some() && zugang::id_lesen(addr) == h.id);
        // Rechts die ID statt der Adresse (die steht im Tooltip), davor bei
        // bekannten Hosts der Haken. Die Zeile ist um den Knopf schmaler:
        // ein langer Name wird gekuerzt, statt in die ID zu laufen (Masse
        // wie in Ui::row).
        let rechts = lang.get(StartId).replace("{i}", &h.id.map(zugang::id_text).unwrap_or_else(|| "-".into()));
        let rw = u.text.width(&rechts, 13, 1);
        let platz = r.w - 14 - 12 - rw - 12 - if h.bekannt { HAKEN_BREITE } else { 0 };
        let name = kuerzen(u, &h.name, platz, 15, 1);
        if u.row(c, r, &name, &rechts, sel) {
            action = Action::Host(Ziel { adresse: h.adresse.clone(), id: h.id, name: Some(h.name.clone()) });
        }
        if h.bekannt {
            haken(c, r.x + r.w - rw - 12 - HAKEN_BREITE + 2, r.y + r.h / 2 - 5, ui::CYAN);
        }
        if r.hit(maus.0, maus.1) {
            tip = Some(h.adresse.clone());
        }
        if let Some(k) = knopf {
            let (g, lw) = ZEILENKNOPF_SCHRIFT;
            let t = kuerzen(u, lang.get(DesktopShortcut), k.w - 16, g, lw);
            if k.hit(maus.0, maus.1) {
                tip = Some(lang.get(TipDesktopShortcut).to_string());
            }
            if u.button_mit(c, k, &t, ui::CYAN, g, lw) {
                action = Action::Verknuepfung { adresse: h.adresse.clone(), id: h.id, name: h.name.clone() };
            }
        }
    }

    // Adressfeld: IP, Name oder Geraete-ID (9.2).
    let fy = py + list_h + 36;
    u.field(c, ui::Rect { x: px, y: fy, w: panel_w, h: 40 }, addr, lang.get(HostAddress), true);

    // Knoepfe; in der einen App dazu der Umschalter "Diesen PC freigeben"
    // zwischen Verbinden und Beenden (9.5), in kleinerer Schrift - der Text
    // ist laenger.
    let by = fy + 62;
    let (bw, freigabe_knopf) = match dieser {
        None => ((panel_w - 20) / 2, None),
        Some(d) => {
            // Platz fuer Kaestchen und Text; hoechstens die halbe Leiste - in
            // schmalen Fenstern (unter 400 Punkten) auch schmaler als 150,
            // der Text wird gekuerzt. Kein clamp: dessen Untergrenze laege
            // dann ueber der Obergrenze.
            let breite = u.text.width(lang.get(FREIGABE_TEXT), 13, 1) + 14 + 8;
            let sw = (breite + 40).max(150).min((panel_w / 2 - 20).max(0));
            ((panel_w - sw - 40) / 2, Some((sw, d.freigabe)))
        }
    };
    if u.button(c, ui::Rect { x: px, y: by, w: bw, h: 44 }, lang.get(Connect), ui::CYAN) && !addr.is_empty() {
        action = Action::Connect(addr.to_string());
    }
    let (quit_x, quit_w) = match freigabe_knopf {
        None => (px + bw + 20, bw),
        Some((sw, an)) => {
            let r = ui::Rect { x: px + bw + 20, y: by, w: sw, h: 44 };
            if r.hit(maus.0, maus.1) {
                tip = Some(lang.get(if an { StartSharing } else { HostSharingIsOff }).to_string());
            }
            if umschalter(u, c, r, lang.get(FREIGABE_TEXT), an, 13, 1) {
                action = Action::FreigabeUmschalten;
            }
            let x = r.x + sw + 20;
            (x, px + panel_w - x)
        }
    };
    if u.button(c, ui::Rect { x: quit_x, y: by, w: quit_w, h: 44 }, lang.get(Quit), ui::MAGENTA) {
        action = Action::Quit;
    }
    // Die eine App: "Dieser Computer" und "Ruhezustand verhindern".
    let meldung_y = match dieser {
        Some(d) => {
            let a = dieser_computer_zeichnen(u, c, lang, d, px, panel_w, by + 44 + 10);
            if !matches!(a, Action::None) {
                action = a;
            }
            by + 76 + EINE_APP_H
        }
        None => by + 76,
    };

    // Meldungen mit Pfad oder Fingerabdruck sind laenger als eine Zeile -
    // umbrechen statt am Fensterrand abschneiden. Das Ergebnis einer
    // Verknuepfung ist neuer als jede stehende Meldung und hat 6 s Vorrang.
    let meldung = match (hinweis, error) {
        (Some((t, f)), _) => Some((t, f)),
        (None, Some(e)) => Some((e, ui::AMBER)),
        (None, None) => None,
    };
    if let Some((e, farbe)) = meldung {
        for (i, z) in umbruch(u, e, c.w as i32 - 60, 13).iter().enumerate() {
            u.text.draw_centered(c, cx, meldung_y + i as i32 * 18, z, 13, farbe, 1);
        }
    }

    // Fussleiste: Oben die Urheberzeile mit Projektseite und FFmpeg-Hinweis
    // (nur unter Windows), darunter Version, Tastenhinweise und die Sprache
    // (Klick oeffnet die Wahl). Passt die Urheberzeile nicht in eine Zeile,
    // steht der FFmpeg-Hinweis in einer eigenen Zeile darunter, und die
    // Trennlinie rueckt nach oben.
    let fy2 = c.h as i32 - 22;
    {
        let cw = u.text.width(COPYRIGHT, 11, 2);
        let ww_ = u.text.width(WEBSITE, 11, 2);
        let hinweis = FFMPEG_HINWEIS.unwrap_or("");
        let fw = if hinweis.is_empty() { 0 } else { u.text.width(hinweis, 11, 2) };
        let luecke = 22;
        let einzeilig = hinweis.is_empty() || cw + luecke + ww_ + luecke + fw <= c.w as i32 - 40;
        let gesamt = cw + luecke + ww_ + if einzeilig && !hinweis.is_empty() { luecke + fw } else { 0 };
        let x0 = cx - gesamt / 2;
        // Eigene Zeile(n): in sehr schmalen Fenstern umgebrochen statt
        // links und rechts abgeschnitten.
        let hinweis_zeilen = if einzeilig {
            Vec::new()
        } else if fw <= c.w as i32 - 40 {
            vec![hinweis.to_string()]
        } else {
            umbruch(u, hinweis, c.w as i32 - 40, 11)
        };
        let n = hinweis_zeilen.len() as i32;
        let wy = if einzeilig { fy2 - 16 } else { fy2 - 31 - (n - 1) * 14 };
        c.hline(0, wy - 22, c.w as i32, ui::CYAN, 30);
        if einzeilig && !hinweis.is_empty() {
            u.text.draw(c, x0 + cw + luecke + ww_ + luecke, wy, hinweis, 11, ui::DIM, 2);
        }
        let abstand = if n > 1 { 1 } else { 2 };
        for (i, z) in hinweis_zeilen.iter().enumerate() {
            u.text.draw_centered(c, cx, wy + 15 + i as i32 * 14, z, 11, ui::DIM, abstand);
        }
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
    let lw = u.text.width(lang.name, 12, 2);
    let lr = ui::Rect { x: c.w as i32 - lw - 34, y: fy2 - 16, w: lw + 22, h: 22 };
    // Die Tastenhinweise nur, wenn sie zwischen Version und Sprache passen -
    // in schmalen Fenstern lagen sie sonst ueber dem Sprachknopf.
    let hw = u.text.width(&hints, 11, 1);
    if cx - hw / 2 > 20 + u.text.width("v0.1", 11, 2) + 12 && cx + hw / 2 < lr.x - 4 {
        u.text.draw_centered(c, cx, fy2, &hints, 11, ui::DIM, 1);
    }
    let hot = lr.hit(u.mouse.0, u.mouse.1);
    u.text.draw(c, lr.x + 10, fy2, lang.name, 12, if hot { ui::CYAN } else { ui::DIM }, 2);
    if hot {
        c.hline(lr.x, lr.y + lr.h, lr.w, ui::CYAN, 160);
        if u.click { action = Action::Sprachwahl; }
    }
    if sprachwahl {
        u.mouse = echte_maus;
        u.click = echter_klick;
        return sprachwahl_zeichnen(u, c, lang);
    }
    if let Some(t) = tip {
        tooltip(u, c, &t, maus, c.w as i32, c.h as i32, 11, 1);
    }
    action
}

/// Spalten und Zeilen der Sprachwahl: gern vier Spalten, weniger, wenn die
/// Breite nicht reicht, mehr, wenn die Hoehe sonst nicht reicht - aber nie
/// breiter als der Platz.
fn sprachwahl_raster(platz_w: i32, platz_h: i32, anzahl: i32, zelle_w: i32, zelle_h: i32) -> (i32, i32) {
    let max_breite = (platz_w / zelle_w.max(1)).max(1);
    let zeilen_hoch = (platz_h / zelle_h.max(1)).max(1);
    let fuer_hoehe = (anzahl + zeilen_hoch - 1) / zeilen_hoch;
    let spalten = 4.min(max_breite).max(fuer_hoehe).min(max_breite).max(1);
    (spalten, (anzahl + spalten - 1) / spalten)
}

/// Die Sprachwahl: alle Sprachen auf einen Blick, jede in ihrem eigenen
/// Namen, die aktuelle in Cyan. Ein Klick waehlt, ein Klick daneben (oder
/// Esc) schliesst ohne Wechsel. Frueher ging es nur reihum durch alle.
fn sprachwahl_zeichnen(u: &mut ui::Ui, c: &mut ui::Canvas, lang: &'static strings::Lang) -> Action {
    let alle = strings::all();
    let groesse = 13;
    let zelle_w = alle.iter().map(|l| u.text.width(l.name, groesse, 1)).max().unwrap_or(80) + 30;
    let zelle_h = 30;
    let (rand, titel_h) = (22, 46);
    let (spalten, zeilen) = sprachwahl_raster(
        c.w as i32 - 32 - 2 * rand,
        c.h as i32 - 16 - titel_h - rand,
        alle.len() as i32,
        zelle_w,
        zelle_h,
    );
    let pw = spalten * zelle_w + 2 * rand;
    let ph = titel_h + zeilen * zelle_h + rand;
    let px = (c.w as i32 - pw) / 2;
    let py = ((c.h as i32 - ph) / 2).max(8);
    c.fill(0, 0, c.w as i32, c.h as i32, ui::BG, 170);
    // Deckend unter der Tafel: die Hostliste darf nicht durchscheinen.
    c.fill(px, py, pw, ph, ui::BG, 255);
    c.panel(px, py, pw, ph, ui::CYAN);
    u.text.draw(c, px + rand, py + 30, lang.get(strings::Key::Language), 12, ui::DIM, 2);
    let mut action = Action::None;
    for (i, l) in alle.iter().enumerate() {
        let (sp, ze) = (i as i32 % spalten, i as i32 / spalten);
        let r = ui::Rect { x: px + rand + sp * zelle_w, y: py + titel_h + ze * zelle_h, w: zelle_w - 8, h: zelle_h - 6 };
        let hot = r.hit(u.mouse.0, u.mouse.1);
        let aktuell = l.code == lang.code;
        if hot {
            c.fill(r.x, r.y, r.w, r.h, ui::CYAN, 26);
        }
        let farbe = if aktuell { ui::CYAN } else if hot { ui::TEXT } else { ui::DIM };
        u.text.draw(c, r.x + 10, r.y + r.h - 8, l.name, groesse, farbe, 1);
        if aktuell {
            c.hline(r.x + 10, r.y + r.h - 2, u.text.width(l.name, groesse, 1), ui::CYAN, 160);
        }
        if hot && u.click {
            action = Action::Sprache(l.code);
        }
    }
    let tafel = ui::Rect { x: px, y: py, w: pw, h: ph };
    if u.click && !tafel.hit(u.mouse.0, u.mouse.1) {
        action = Action::Sprachwahl;
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

    // Ansicht "dateien": die Zeile zu Dateiuebertragungen unten mittig ueber
    // einem angedeuteten Bild - ein Empfang laeuft, eine Sendung ist gerade
    // fertig (beide Richtungen zugleich, wie es vorkommen kann). Ansicht
    // "dateienfehler": dieselben Zeilen mit uebersetzten Abbruchgruenden
    // (Empfang: Zeitueberschreitung, Sendung: Gegenseite ohne Platz).
    if view == "dateien" || view == "dateienfehler" {
        for y in 0..h {
            for x in 0..w {
                let a = (x * 255 / w) as u32;
                let b = (y * 160 / h) as u32;
                buf[y * w + x] = ((a / 3) << 16) | ((b / 2) << 8) | (40 + a / 5);
            }
        }
        use dateien::{Abbruch, Ergebnis, Richtung, Stand};
        let fehler = view == "dateienfehler";
        let staende = [
            Stand {
                richtung: Richtung::Empfangen, kennung: 7, bytes: 3_100_000, gesamt: 12_400_000,
                eintraege: 5, dateien: 4, oberste: 2,
                ergebnis: if fehler { Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung) } else { Ergebnis::Laeuft },
            },
            Stand {
                richtung: Richtung::Senden, kennung: 3, bytes: 2_600_000, gesamt: 2_600_000,
                eintraege: 3, dateien: 3, oberste: 3,
                ergebnis: if fehler {
                    Ergebnis::Abgebrochen(Abbruch::Quittung(dateien::ZUSTAND_KEIN_PLATZ))
                } else {
                    Ergebnis::Fertig
                },
            },
        ];
        let zeilen: Vec<(String, u32)> = staende.iter().map(|s| datei_zeile(s, lang)).collect();
        {
            let mut c = ui::Canvas::neu(&mut buf, w, h);
            datei_zeilen_zeichnen(&mut u, &mut c, w as u32, h as u32, &zeilen);
        }
        write_bmp(path, w, h, &buf, lang);
        return;
    }

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
        // Nachgestellte Bildschirmliste, wie sie der Mac mini schickt
        // (Spezifikation Bildschirmwahl 2.4): Monitor und virtueller
        // Bildschirm, Automatik, der Monitor ist Haupt und wird gestreamt.
        let bildschirme = vec![
            bildschirm::BildschirmEintrag {
                kennung: "v1138-m1234-s0".into(), name: "X27 X1".into(), breite: 1920, hoehe: 1080, hz: 120, haupt: true, gestreamt: true,
            },
            bildschirm::BildschirmEintrag {
                kennung: "v0-m0-s0".into(), name: "Virtuell 16:9".into(), breite: 1920, hoehe: 1080, hz: 240, haupt: false, gestreamt: false,
            },
        ];
        let stand = HudStand {
            vollbild: true, pixelgenau: false, statistik: true, nerd: true,
            wahl: einstellungen::StatWahl::default(),
            codecs,
            codec_idx: None,
            wechsel: false,
            bildschirmwahl: true,
            bildschirme,
            bildschirm_wunsch: None,
            bildschirm_wechsel: false,
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
            verknuepfung: None,
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
    // Ansichten des Zugangsdialogs (Spezifikation Pairing v1, 9.3) ueber dem
    // Wartebild: "zugang" (Passwort halb getippt, Zulassen moeglich, Code),
    // "zugangfalsch" (Passwort falsch, Feld leer), "zugangwarten" (Drossel:
    // Countdown, Verbinden aus), "zugangneu" (neue Identitaet an bekannter
    // Adresse, nur Passwort, lesbar gezeigt, wird geprueft).
    if view.starts_with("zugang") {
        let jetzt = Instant::now();
        // Die Schreibmarke blinkt: in diesem Takt steht sie.
        u.tick = 70;
        let mut d = zugangsphase::Dialog {
            name: "Roberts Mac mini".into(),
            id: 581_729_911,
            code: "628 306".into(),
            zulassen: true,
            neue_identitaet: false,
            lage: zugangsphase::Lage::Eingabe,
            frei_ab: None,
            runde: 0,
        };
        let (mut pw, mut zeigen) = ("k7m4wq", false);
        match view {
            "zugangfalsch" => {
                d.lage = zugangsphase::Lage::Falsch;
                d.runde = 1;
                pw = "";
            }
            "zugangwarten" => {
                d.lage = zugangsphase::Lage::Falsch;
                d.runde = 3;
                d.frei_ab = Some(jetzt + Duration::from_secs(20));
                pw = "k7m-4wq";
            }
            "zugangneu" => {
                d.zulassen = false;
                d.neue_identitaet = true;
                d.lage = zugangsphase::Lage::Pruefen;
                pw = "k7m-4wq-9tz";
                zeigen = true;
            }
            _ => {}
        }
        let mut c = ui::Canvas::neu(&mut buf, w, h);
        let (maus, klick) = (u.mouse, u.click);
        u.mouse = (-10_000, -10_000);
        u.click = false;
        let _ = warte_screen(&mut u, &mut c, lang, "192.168.178.194:9001", (false, None, Some("628 306".into()), false));
        u.mouse = maus;
        u.click = klick;
        let _ = zugang_zeichnen(&mut u, &mut c, lang, &d, pw, pw.chars().count(), zeigen, jetzt);
        drop(c);
        write_bmp(path, w, h, &buf, lang);
        return;
    }
    // Die Hostliste: ein Host mit ID, schon bekannt (Haken), und ein
    // aelterer ohne ID ("-").
    let hosts = vec![
        Hostzeile {
            name: "Roberts Mac mini".into(),
            adresse: "192.168.178.194:9001".into(),
            id: Some(581_729_911),
            bekannt: true,
        },
        Hostzeile { name: "studio.local".into(), adresse: "192.168.178.60:9001".into(), id: None, bekannt: false },
    ];
    // "abgeloest" und "fingerabdruck": der Startbildschirm mit der Meldung,
    // wie sie nach Nachricht 10 bzw. bei einem anderen Geraet unter der
    // gewaehlten ID dasteht (frueher: geaenderter Host-Schluessel) - ueber
    // dieselben Schluessel wie im Betrieb, in der Sprache der Ansicht.
    // "veraltet", "abgelehnt" und "idfehlt": weitere Meldungen der
    // Zugangsphase (9.6); "entfernt": der Host hat dieses Geraet entfernt
    // (beim Wiederverbinden); "freigabefehler": "Diesen PC freigeben" konnte
    // die Host-Rolle nicht starten; "beendet": der Host hat sich
    // verabschiedet (MSG_HOST_ENDE, Grund 0).
    let meldung = match view {
        "abgeloest" => Some(lang.get(strings::Key::SessionTakenOver).to_string()),
        "fingerabdruck" => Some(
            Meldung::from(secure::Fehler::AnderesGeraet { addr: "192.168.178.194:9001".into(), erwartet: 581_729_911, gemeldet: 5 })
                .text(lang),
        ),
        "veraltet" => Some(Meldung::neu(strings::Key::MsgHostOutdated, "").mit("{n}", "studio.local").text(lang)),
        "abgelehnt" => Some(Meldung::neu(strings::Key::MsgRefused, "").mit("{n}", "Roberts Mac mini").text(lang)),
        "idfehlt" => Some(Meldung::neu(strings::Key::MsgIdNotFound, "").mit("{i}", "123 456 789").text(lang)),
        "entfernt" => Some(Meldung::neu(strings::Key::MsgDeviceRemoved, "").mit("{n}", "Roberts Mac mini").text(lang)),
        "freigabefehler" => Some(lang.get(strings::Key::MsgShareFailed).to_string()),
        "beendet" => Some(host_ende_meldung(HOST_ENDE_BEENDET, "Roberts Mac mini", "192.168.178.194:9001").text(lang)),
        _ => None,
    };
    // "starttip": die Maus steht ueber dem Knopf "Verknuepfung" der ersten
    // Zeile, damit sein Tooltip im Bild ist (nur unter Windows gibt es ihn).
    // "startzeile": ueber der ersten Zeile selbst - der Tooltip zeigt die
    // Adresse.
    if view == "starttip" {
        if let (_, Some(k)) = start_zeile(&mut u, w as i32, h as i32, lang, 0) {
            u.mouse = (k.x + k.w / 2, k.y + k.h / 2);
        }
    }
    if view == "startzeile" {
        let (r, _) = start_zeile(&mut u, w as i32, h as i32, lang, 0);
        u.mouse = (r.x + 60, r.y + r.h / 2);
    }
    // In der einen App (Windows) der Umschalter "Diesen PC freigeben", die
    // Zeile "Dieser Computer" und "Ruhezustand verhindern"; "startfreigabe"
    // zeigt die Freigabe an und den Ruhezustand verhindert, sonst beides
    // aus.
    let an = view == "startfreigabe";
    let dieser = MIT_FREIGABE.then_some(DieserComputer { freigabe: an, name: "Büro-PC", id: Some(581_729_911), ruhe_verhindern: an });
    {
        let mut c = ui::Canvas::neu(&mut buf, w, h);
        let _ = start_screen(
            &mut u,
            &mut c,
            lang,
            &hosts,
            "192.168.178.194:9001",
            meldung.as_deref(),
            None,
            view == "sprachwahl",
            dieser.as_ref(),
        );
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

/// Die Groesse hinter "@" einer --shot-Ansicht, "BxH" in Pixeln; None, wenn
/// sie nicht lesbar ist (dann gilt die Vorgabe der Ansicht). Begrenzt auf
/// 320..=7680 je Seite, damit ein Tippfehler keinen Riesenpuffer anlegt.
fn shot_groesse(g: &str) -> Option<(usize, usize)> {
    let (b, h) = g.split_once('x')?;
    let (b, h): (usize, usize) = (b.parse().ok()?, h.parse().ok()?);
    ((320..=7680).contains(&b) && (320..=7680).contains(&h)).then_some((b, h))
}

/// Schalter mit Werten: ihre Werte sind nie die Adresse. Ohne diese Liste
/// wurde aus `--anzeige cpu` die Adresse "cpu:9001" - und die echte Adresse
/// dahinter ignoriert.
const WERTIG: &[(&str, usize)] = &[
    ("--anzeige", 1), ("--adapter", 1), ("--decoder", 1), ("--codec", 1), ("--bildschirm", 1),
    ("--set", 1), ("--faeden", 1), ("--shot", 3), ("--anzeigetest", 1),
    ("--benchmark-auswahl", 1), ("--mitschnitt", 1),
    // Desktop-Verknuepfung: --verknuepfung <adresse> [--name <name>]
    // [--ordner <verzeichnis>] legt nur an und verbindet nie.
    ("--verknuepfung", 1), ("--name", 1), ("--ordner", 1),
    // Host-Rolle (host/mod.rs liest sie selbst; hier nur, damit ihre
    // Werte nie fuer eine Adresse gehalten werden)
    ("--output", 1), ("--fps", 1), ("--mbit", 1), ("--konserve", 1), ("--sekunden", 1), ("--encoderweg", 1),
    // Host-Engine des Mac (host_mac::host_schalter reicht sie weiter)
    ("--out", 1), ("--display", 1), ("--capture", 2),
    // Ziel aus einer Verknuepfung (Pairing v1, 9.4): --verbinden <adresse>
    // --id <id>; --passwort <pw> nur im Pruefmodus.
    ("--verbinden", 1), ("--id", 1), ("--passwort", 1),
];

/// Erstes Argument, das kein Schalter und kein Wert eines Schalters ist -
/// so, wie es dasteht.
fn erstes_argument(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some((_, n)) = WERTIG.iter().find(|(s, _)| *s == a.as_str()) {
            i += 1 + n;
            continue;
        }
        // --benchmark [dauer], --host [port], --nur-host [port], --serve
        // [port]: die Zahl ist wahlfrei - eine Zahl dahinter gehoert zum
        // Schalter, alles andere nicht.
        if a == "--benchmark" || a == "--host" || a == "--nur-host" || a == "--serve" {
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
        return Some(a);
    }
    None
}

/// Erstes Argument, das kein Schalter und kein Wert eines Schalters ist:
/// die Adresse, mit Port ergaenzt; sonst leer.
#[cfg(test)]
fn adresse_aus_argumenten(args: &[String]) -> String {
    erstes_argument(args).map(adresse_vollstaendig).unwrap_or_default()
}

/// Das Ziel von der Befehlszeile: `--verbinden <adresse>` (Verknuepfung)
/// oder das erste Argument ohne "--" (Adresse oder Name, Port ergaenzt),
/// dazu `--id <id>`. Neun Ziffern (auch mit Leerzeichen oder Bindestrichen)
/// sind eine Geraete-ID, keine Adresse (9.2) - dann fehlt die Adresse und
/// wird ueber die ID gesucht.
fn ziel_aus_argumenten(args: &[String]) -> (String, Option<u32>) {
    let wert = |s: &str| args.iter().position(|a| a == s).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--"));
    let id = wert("--id").and_then(|v| zugang::id_lesen(v));
    let roh = wert("--verbinden").map(String::as_str).or_else(|| erstes_argument(args)).unwrap_or("").trim();
    match zugang::id_lesen(roh) {
        Some(i) => (String::new(), Some(i)),
        None => (adresse_vollstaendig(roh), id),
    }
}

/// Ein Ziel als Text fuer die Weitergabe an die laufende App (einzel.rs):
/// "<adresse>", mit ID "<adresse>#<9 Ziffern>", nur ID "#<9 Ziffern>".
fn ziel_text(adresse: &str, id: Option<u32>) -> String {
    match id {
        Some(id) => format!("{}#{}", adresse.trim(), zugang::id_ziffern(id)),
        None => adresse.trim().to_string(),
    }
}

/// Gegenstueck zu `ziel_text`: Adresse (vielleicht leer) und ID.
fn ziel_lesen(t: &str) -> (String, Option<u32>) {
    match t.trim().rsplit_once('#') {
        Some((a, i)) if i.len() == 9 && i.bytes().all(|b| b.is_ascii_digit()) => (a.trim().to_string(), zugang::id_lesen(i)),
        _ => (t.trim().to_string(), None),
    }
}

/// So lange sucht ein Start mit einer unbekannten ID ihre Bekanntgabe
/// (Hosts rufen alle 2 s).
const ID_SUCHE: Duration = Duration::from_secs(5);

/// Einmal beim Start: known_hosts.txt nach hosts.txt uebernehmen
/// (Spezifikation Pairing v1, 4.4) und Reste eines abgebrochenen Schreibens
/// wegraeumen. Die Einstellungen je Host bleiben am Fingerabdruck.
fn hosts_vorbereiten() {
    let Ok(ordner) = secure::config_dir() else { return };
    let weg = zugang::zwischendateien_aufraeumen(&ordner.join(zugang::HOSTS_DATEI));
    if weg > 0 {
        protokoll::zeile(format!("hosts.txt: {weg} Zwischendateien eines abgebrochenen Schreibens entfernt"));
    }
    match zugang::hosts_migrieren(&ordner) {
        zugang::Migration::Keine => {}
        zugang::Migration::Uebernommen { anzahl, umbenannt } => protokoll::zeile(format!(
            "hosts.txt: {anzahl} bekannte Hosts aus known_hosts.txt uebernommen{}",
            match umbenannt {
                Ok(()) => " - die alte Datei heisst jetzt known_hosts.txt.migriert".to_string(),
                Err(e) => format!(" - alte Datei nicht umbenannt ({e})"),
            }
        )),
        zugang::Migration::Fehler(e) => {
            protokoll::zeile(format!("hosts.txt: Uebernahme aus known_hosts.txt gescheitert - {e} (naechster Start versucht es wieder)"))
        }
    }
}

/// Die Zugangsphase im Pruefmodus (--headless): bei jeder Wende des Dialogs
/// sofort die Zeilen, die der Empfangsfaden dazu protokolliert hat (sonst
/// kaemen sie erst mit dem naechsten Drei-Sekunden-Takt), und --passwort
/// geht genau einmal hinaus, sobald die Drossel es erlaubt. Ohne --passwort
/// wartet der Client auf "Zulassen" am Host.
struct ZugangPruefmodus {
    passwort: Option<String>,
    gesendet: bool,
    zuletzt: Option<(zugangsphase::Lage, u32)>,
}

impl ZugangPruefmodus {
    fn neu(passwort: Option<String>) -> ZugangPruefmodus {
        ZugangPruefmodus { passwort, gesendet: false, zuletzt: None }
    }

    fn takt(&mut self, shared: &Mutex<Shared>) {
        use std::io::Write;
        let mut s = shared.lock().unwrap();
        let Some(d) = s.zugang.clone() else {
            if self.zuletzt.take().is_some() {
                // Die Phase ist vorbei: was der Empfangsfaden dazu sagt, jetzt.
                for z in protokoll::abholen() {
                    println!("{z}");
                }
                std::io::stdout().flush().ok();
            }
            return;
        };
        let jetzt = Instant::now();
        if self.zuletzt != Some((d.lage, d.runde)) {
            for z in protokoll::abholen() {
                println!("{z}");
            }
            if self.zuletzt.is_none() && self.passwort.is_none() {
                println!("Zugang: kein --passwort - warte auf \"Zulassen\" am Host");
            }
            self.zuletzt = Some((d.lage, d.runde));
            std::io::stdout().flush().ok();
        }
        if !self.gesendet && d.darf_senden(jetzt) {
            if let Some(pw) = self.passwort.clone() {
                s.zugang_eingabe = Some(zugangsphase::Eingabe::Passwort(pw));
                self.gesendet = true;
            }
        }
    }
}

/// `--verknuepfung`: anlegen, Pfad bzw. Fehler ausgeben, Rueckgabewert 0/1;
/// auf dem Mac gibt es das nicht (2). Laeuft vor allem anderen und ohne
/// Protokolldatei - eine laufende App behaelt ihr protokoll.txt.
fn verknuepfung_befehlszeile(a: Result<verknuepfung::Aufruf, String>) -> i32 {
    #[cfg(not(windows))]
    {
        let _ = a;
        eprintln!("Die Desktop-Verknuepfung (--verknuepfung) gibt es nur unter Windows.");
        2
    }
    #[cfg(windows)]
    {
        let a = match a {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Verknuepfung nicht angelegt: {e}");
                return 1;
            }
        };
        // Der Kommentar der Verknuepfung in der Sprache, die auch das
        // Fenster naehme.
        let cfg = einstellungen::Einstellungen::laden();
        let lang = match &cfg.sprache {
            Some(c) => strings::pick(c),
            None => strings::pick(&system_language()),
        };
        // COM (STA) richtet verknuepfung_anlegen selbst ein.
        match verknuepfung::verknuepfung_anlegen(a.ordner.as_deref(), &adresse_vollstaendig(&a.adresse), a.id, &a.name, lang) {
            Ok(p) => {
                println!("{}", p.display());
                0
            }
            Err(e) => {
                eprintln!("Verknuepfung nicht angelegt: {e}");
                1
            }
        }
    }
}

/// --decodertest unter Windows: die Karten, die CUDA-Geraete, das
/// D3D11-Geraet je Adapter und je Codec und Wunsch der Decoder, der
/// herauskommt.
#[cfg(windows)]
fn decodertest() {
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
}

/// --decodertest auf dem Mac: der Hardware-Encoder codiert eine kurze Probe
/// (HEVC Main 4:4:4 10, 1920x1080, das Muster aus vt_decoder::probe), und
/// VideoToolbox decodiert sie mit und ohne Hardware - je Weg die Zeile der
/// Sitzung, das Format des Ergebnisses und die Zeit je Bild. Startet weder
/// Host noch Aufnahme.
#[cfg(target_os = "macos")]
fn decodertest() {
    protokoll::einschalten(true);
    let (breite, hoehe, anzahl) = (1920, 1080, 60);
    let probe = match vt_decoder::probe::hevc_444_10(breite, hoehe, anzahl) {
        Ok(p) => p,
        Err(e) => {
            println!("Probe HEVC 4:4:4 10 {breite}x{hoehe}: {e}");
            return;
        }
    };
    println!("Probe: {} Bilder HEVC Main 4:4:4 10 {breite}x{hoehe} aus dem Hardware-Encoder", probe.einheiten.len());
    for (hardware, weg) in [(true, "Hardware"), (false, "Prozessor")] {
        let mut d = vt_decoder::Decoder::neu(false, hardware);
        let mut bilder = Vec::new();
        let mut zahl = 0usize;
        let t0 = Instant::now();
        let mut fehler = None;
        for (i, au) in probe.einheiten.iter().enumerate() {
            let vorher = bilder.len();
            if let Err(e) = d.fuettern(au, i as i64, &mut bilder) {
                fehler = Some(format!("Paket {i}: {e}"));
                break;
            }
            // Wie im Empfangsfaden bleibt nur das neueste Bild; die anderen
            // gehen gleich an den Pufferpool des Decoders zurueck.
            zahl += bilder.len() - vorher;
            if bilder.len() > 1 {
                bilder.drain(..bilder.len() - 1);
            }
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.0 / probe.einheiten.len() as f32;
        for z in protokoll::abholen() {
            println!("    {z}");
        }
        match (fehler, bilder.last()) {
            (Some(e), _) => println!("VideoToolbox {weg}: {e}"),
            (None, Some(b)) => {
                let umrechnung = to_rgb(b).map(|f| format!("to_rgb {}x{} ok", f.width, f.height)).unwrap_or_else(|e| format!("to_rgb: {e}"));
                println!("VideoToolbox {weg}: {zahl} Bilder, {ms:.2} ms je Bild, Ausgabe {} ({umrechnung})", b.format_name());
            }
            (None, None) => println!("VideoToolbox {weg}: kein Bild"),
        }
    }
}

fn main() {
    // An die Konsole des Aufrufers anhaengen, falls es eine gibt. Beim
    // Doppelklick gibt es keine, dann passiert hier einfach nichts.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Windows, nur die Host-Rolle: --nur-host [port] (VM und Tests), --list,
    // --messen - ohne Fenster und ohne Client, Abzweig VOR allem, was ein
    // Fenster oder einen Client braucht. --host ist dagegen die eine App im
    // Hintergrund mit Freigabe an (weiter unten).
    #[cfg(windows)]
    if args.iter().any(|a| a == "--nur-host" || a == "--list" || a == "--messen") {
        let code = host::main_host(&args);
        std::process::exit(code);
    }
    // Mac: die Werkzeuge der eingebauten Host-Engine (--list, --formattest,
    // --capture ohne --serve; die Ziele make list/capture/permissions) laufen
    // zu Ende - wie frueher in QuadChroma.app, vor allem anderen. Sie fragen
    // nach der Bildschirmaufnahme.
    #[cfg(target_os = "macos")]
    if let Some(code) = host_mac::werkzeug(&args) {
        std::process::exit(code);
    }
    // Die reine Host-Rolle ohne Fenster (--nur-host, --messen) gibt es nur
    // unter Windows; auf dem Mac ist die Host-Rolle Teil der einen App.
    // Ohne diesen Zweig wuerde der Schalter zur Adresse und ein Fenster
    // aufgehen, das auf eine Verbindung wartet.
    #[cfg(not(windows))]
    if args.iter().any(|a| a == "--host" || a == "--nur-host" || a == "--list" || a == "--messen") {
        eprintln!("Die reine Host-Rolle (--host, --nur-host, --messen) gibt es nur unter Windows; auf dem Mac gibt die App selbst frei (Menueleiste).");
        std::process::exit(2);
    }

    // Desktop-Verknuepfung ohne Fenster und ohne Verbindung.
    if let Some(a) = verknuepfung::aufruf(&args) {
        std::process::exit(verknuepfung_befehlszeile(a));
    }

    // Selbsttest des Symbols: anlegen, aendern, entfernen, Ende - vor
    // Einzelinstanz, Ablagewaechter, Bekanntgabe, Protokolldatei und jedem
    // Netz (auch vor --noisetest). Unter Windows der Infobereich, auf dem Mac
    // die Menueleiste; den jeweils anderen Schalter gibt es auf dieser
    // Plattform nicht (Rueckgabe 2). Beide haben keinen Wert und stehen
    // deshalb nicht in WERTIG; als Adresse gelten sie nie (beginnen mit --).
    if args.iter().any(|a| a == "--tray-selbsttest") {
        #[cfg(windows)]
        std::process::exit(tray_win::selbsttest());
        #[cfg(not(windows))]
        {
            eprintln!("Den Infobereich-Selbsttest (--tray-selbsttest) gibt es nur unter Windows.");
            std::process::exit(2);
        }
    }
    // Mac: das eine Symbol der App mit der eingebauten Host-Engine - ohne
    // Dienst, ohne Aufnahme, in einem Wegwerf-HOME (host_mac::selbsttest).
    if args.iter().any(|a| a == "--menueleiste-selbsttest") {
        #[cfg(target_os = "macos")]
        std::process::exit(host_mac::selbsttest());
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("Den Menueleisten-Selbsttest (--menueleiste-selbsttest) gibt es nur unter macOS.");
            std::process::exit(2);
        }
    }
    // "Ruhezustand verhindern" am System: setzen, im Bericht des Systems
    // nachsehen (pmset -g assertions bzw. powercfg /requests), aufheben,
    // wieder nachsehen. Ohne Fenster, ohne Host, ohne Aufnahme.
    if args.iter().any(|a| a == "--ruhe-selbsttest") {
        std::process::exit(ruhe_selbsttest());
    }
    // Selbsttest der Metal-Anzeige an einem echten Fenster von winit
    // (anzeige_mac.rs): Bild auf dem Schirm, Schicht passend zur Ansicht,
    // Verdecken und Aufdecken, Rueckfall auf softbuffer. Ebenso vor allem
    // anderen; weder Host noch Aufnahme noch Netz. Rueckgabe 0, 1 oder 3
    // (nicht pruefbar), 2 auf den anderen Plattformen.
    if args.iter().any(|a| a == "--anzeige-selbsttest") {
        #[cfg(target_os = "macos")]
        {
            use std::io::Write;
            protokoll::einschalten(false);
            let code = anzeige::fenster_selbsttest();
            std::io::stdout().flush().ok();
            std::process::exit(code);
        }
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("Den Anzeige-Selbsttest (--anzeige-selbsttest) gibt es nur unter macOS.");
            std::process::exit(2);
        }
    }

    // Das Ziel: Adresse (auch Name) und/oder Geraete-ID - aus der
    // Befehlszeile oder einer Verknuepfung ("--verbinden <adresse> --id <id>").
    let (addr, start_id) = ziel_aus_argumenten(&args);

    let headless = std::env::args().any(|a| a == "--headless");
    // --passwort <pw>: nur im Pruefmodus - beantwortet die Zugangsphase
    // einmal mit diesem Passwort (Spezifikation Pairing v1, 3.5). Im Fenster
    // gibt es das nie: dort tippt der Nutzer selbst.
    let pruef_passwort: Option<String> = args
        .iter()
        .position(|a| a == "--passwort")
        .and_then(|i| args.get(i + 1))
        .filter(|v| !v.starts_with("--"))
        .cloned();
    if pruef_passwort.is_some() && !headless {
        eprintln!("--passwort gilt nur mit --headless - uebergangen.");
    }

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
    // NVIDIA-Karte pruefen, ohne einen Host zu belaestigen. Auf dem Mac
    // decodiert VideoToolbox eine Probe des Hardware-Encoders (siehe
    // `decodertest`).
    if std::env::args().any(|a| a == "--decodertest") {
        decodertest();
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
    // Auf dem Mac dasselbe ueber Metal: jedes Format von VideoToolbox gegen
    // den CPU-Weg, dann die Bildzeit bei 1440p und 120 Hz. Ohne Fenster,
    // ohne Host und ohne Aufnahme.
    #[cfg(target_os = "macos")]
    {
        if let Some(i) = std::env::args().position(|a| a == "--anzeigetest") {
            use std::io::Write;
            let args: Vec<String> = std::env::args().collect();
            let verzeichnis = args.get(i + 1).cloned().unwrap_or_else(|| ".".into());
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
        // "ansicht@BxH" zeichnet in dieser Groesse statt der Vorgabe - fuer
        // die Pruefung des Menues bei 1920x1080, wo alles 1,5-fach skaliert.
        let (view, groesse) = match view.split_once('@') {
            Some((v, g)) => (v.to_string(), shot_groesse(g)),
            None => (view, None),
        };
        let (w, h) = groesse.unwrap_or(
            if view == "sitzung" || view == "nerd" || view.starts_with("dateien") || view.starts_with("hud") {
                (1280, 720)
            } else {
                (900, 700)
            },
        );
        screenshot(&path, w, h, strings::pick(&code), &view);
        return;
    }

    // Einzelinstanz: hoechstens ein Client mit Fenster je Nutzersitzung.
    // Ein zweiter Start reicht seine Adresse an den ersten weiter und endet -
    // vor allem anderen: vor Ablagewaechter, Bekanntgabe-Port und
    // protokoll.txt (das der erste Schreiber leert). Der Pruefmodus
    // (--headless) und alle Wege ohne Fenster oben sind ausgenommen.
    //
    // Die eine App (Windows): --host startet sie im Hintergrund mit Freigabe
    // an (so ruft sie die Verknuepfung "Mit Windows starten" frueherer
    // Fassungen), --hintergrund im Hintergrund mit der Freigabe, wie sie
    // eingestellt ist (die heutige Verknuepfung). Laeuft die App schon, endet
    // ein solcher Start still - er oeffnet kein Fenster.
    let host_start = cfg!(windows) && args.iter().any(|a| a == "--host");
    let hintergrund_start = MIT_FREIGABE && (host_start || args.iter().any(|a| a == "--hintergrund"));
    if hintergrund_start && !headless && einzel::laeuft_schon() {
        println!("QuadChroma laeuft schon - der Start im Hintergrund endet.");
        std::process::exit(0);
    }
    let mut einzel_waechter = None;
    let mut einzel_eingang = None;
    let mut einzel_ohne = None;
    if !headless {
        let weiter = ziel_text(&addr, start_id);
        match einzel::beanspruchen(&weiter) {
            einzel::Start::Erste { waechter, eingang } => {
                einzel_waechter = Some(waechter);
                einzel_eingang = Some(eingang);
            }
            einzel::Start::Weitergereicht(None) => {
                if weiter.is_empty() {
                    println!("QuadChroma laeuft schon - nach vorn geholt.");
                } else {
                    println!("QuadChroma laeuft schon - {weiter} weitergereicht.");
                }
                std::process::exit(0);
            }
            // Die Adresse kaeme so nicht an (zu lang, Steuerzeichen): die
            // laufende App ist nur nach vorn geholt, die Adresse verworfen.
            einzel::Start::Weitergereicht(Some(g)) => {
                eprintln!("QuadChroma laeuft schon - nach vorn geholt, Adresse nicht weitergereicht ({g}).");
                std::process::exit(1);
            }
            einzel::Start::Unerreichbar(g) => {
                eprintln!("QuadChroma laeuft schon, antwortet aber nicht ({g}).");
                std::process::exit(1);
            }
            einzel::Start::Ohne(g) => einzel_ohne = Some(g),
        }
    }

    // hosts.txt: einmal known_hosts.txt uebernehmen (Spezifikation 4.4).
    hosts_vorbereiten();
    // Wer sich im Netz meldet - fuer Startbildschirm, Symbol, Pruefmodus und
    // den Empfangsfaden (ein Ziel mit ID unter neuer Adresse). Ein Faden,
    // ein Port.
    let bekanntgaben = discovery::start(9003);

    // Ziel mit ID: fehlt die Adresse (nur "--id" oder eine ID als Argument),
    // gilt die zuletzt bekannte aus hosts.txt; der Name von dort, bis der
    // Host selbst einen nennt. Ist die ID ganz unbekannt, wartet der Start
    // kurz auf ihre Bekanntgabe (Fenster: id_ausstehend, Pruefmodus unten).
    let mut addr = addr;
    let mut start_name: Option<String> = None;
    if let Some(id) = start_id {
        let bekannt = zugang::ablage_pfad(zugang::HOSTS_DATEI)
            .ok()
            .and_then(|p| zugang::Hostliste::laden(&p).ok())
            .and_then(|l| l.nach_id(id).cloned());
        if let Some(h) = bekannt {
            if addr.is_empty() {
                addr = h.adresse.clone();
            }
            start_name = Some(h.name);
        }
    }
    let mut id_ausstehend = match start_id {
        Some(id) if addr.is_empty() => Some((id, Instant::now())),
        _ => None,
    };
    if headless {
        if let Some((id, seit)) = id_ausstehend.take() {
            while addr.is_empty() && seit.elapsed() < ID_SUCHE {
                if let Some(g) = bekanntgaben.lock().ok().and_then(|h| h.mit_id(id)) {
                    addr = g.host.addr.to_string();
                    start_name = Some(g.host.name);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if addr.is_empty() {
                let m = Meldung::neu(strings::Key::MsgIdNotFound, format!("ID {} weder im Netz noch in hosts.txt", zugang::id_text(id)))
                    .mit("{i}", zugang::id_text(id));
                println!("Fehler: {:?}: {}", m.key, m.text(&strings::EN));
                std::process::exit(1);
            }
            println!("ID {} gefunden: {addr}", zugang::id_text(id));
        }
    }

    // Ohne Adresse auf der Befehlszeile faengt das Programm beim Startbildschirm
    // an - dort sucht es Hosts im Netz und nimmt die Adresse entgegen. Der
    // Eingabekanal bekommt sie erst, wenn wirklich verbunden wird.
    let input_addr = if addr.is_empty() { String::new() } else { bump_port(&addr, 1) };

    let start_addr = addr.clone();
    let input = Arc::new(Mutex::new(InputLink::new(input_addr)));

    // Gespeicherte Einstellungen. Sie bestimmen unter anderem, ob wir im
    // Vollbild starten - das ist die Voreinstellung. Schon hier geladen,
    // weil der Empfangsfaden den Decoderwunsch vom ersten Bild an kennen soll.
    let mut cfg = einstellungen::Einstellungen::laden();
    // Der Name, unter dem andere dieses Geraet sehen (Nachricht 3,
    // Bekanntgabe): der eingestellte, sonst der Rechnername - vor dem
    // ersten Verbindungsaufbau.
    zugang::geraetename_setzen(cfg.geraetename.clone());
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
    let shared = Arc::new(Mutex::new(Shared {
        decoder_wunsch,
        ton: true,
        bekanntgaben: Some(bekanntgaben.clone()),
        ..Shared::default()
    }));
    // Empfangene Dateien von frueher: was aelter als 24 h ist, geht - in einem
    // eigenen Faden, damit bis zu 4 GB bzw. 10 000 Eintraege je Verzeichnis
    // den Start nicht aufhalten. Neue Uebertragungen stoert das nicht: ihr
    // Verzeichnis ist juenger als 24 h.
    let _ = std::thread::Builder::new().name("qc-dateien-aufraeumen".into()).spawn(|| {
        let weg = dateien::aufraeumen_beim_start();
        if weg > 0 {
            protokoll::zeile(format!("Dateien: {weg} alte Uebertragungsverzeichnisse beim Start geloescht"));
        }
    });

    // Zwischenablage: Was hier kopiert wird, geht zum Host - Text als
    // IN_CLIP, Dateien ueber den Sender (dateien.rs), nur mit Gegenueber
    // (der Waechter liest ohne Sitzung nicht; angemeldet als Client, mit der
    // Sitzung als Gegenueber - eine Host-Rolle im selben Prozess meldet sich
    // am selben Waechter an). Der Waechter reicht nur
    // weiter (unter Windows laeuft er im Faden seines Fensters); Sperren und
    // Senden stehen im eigenen Faden der Weiterleitung, der Reihe nach.
    // Liegt eine Kopie vorgemerkt (Faehigkeiten des Hosts stehen noch aus),
    // sieht die Weiterleitung im kurzen Takt nach ihr.
    #[cfg(any(windows, target_os = "macos"))]
    {
        use std::sync::mpsc::RecvTimeoutError;
        let (tx, rx) = std::sync::mpsc::channel::<clipboard::Inhalt>();
        let (link, sh) = (input.clone(), shared.clone());
        std::thread::spawn(move || loop {
            let inhalt = match vormerk_takt(&sh) {
                None => match rx.recv() {
                    Ok(i) => Some(i),
                    Err(_) => break,
                },
                Some(takt) => match rx.recv_timeout(takt) {
                    Ok(i) => Some(i),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            };
            match inhalt {
                Some(clipboard::Inhalt::Text(text)) => text_senden(&text, &sh, &link),
                Some(clipboard::Inhalt::Dateien(pfade)) => dateien_senden(pfade, &sh, &link),
                None => {}
            }
            vorgemerkte_dateien(&sh, &link);
        });
        clipboard::watch(protokoll::Herkunft::Client, clipboard::client_gegenueber, move |inhalt| {
            let _ = tx.send(inhalt);
        });
    }

    {
        let shared = shared.clone();
        let inp = input.clone();
        std::thread::spawn(move || stream_thread(shared, inp));
    }
    // Wurde eine Adresse mitgegeben, gleich verbinden - mit ID (Verknuepfung)
    // prueft der Handschlag sie am Schluessel des Hosts.
    if !addr.is_empty() {
        let mut s = shared.lock().unwrap();
        s.target = Some(addr.clone());
        s.ziel_id = start_id;
        s.ziel_name = start_name.clone();
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
        // Einmaliger Bildschirmwunsch aus der Befehlszeile: --bildschirm
        // <Kennung|auto> (Spezifikation Bildschirmwahl 3.1). Aeusseres Some:
        // ein Wunsch liegt vor; inneres None: Automatik.
        let mut bildschirm_wunsch: Option<Option<String>> = std::env::args()
            .position(|a| a == "--bildschirm")
            .and_then(|i| std::env::args().nth(i + 1))
            .map(|v| if v == "auto" { None } else { Some(v) });
        // --benchmark [dauer]: derselbe Ablauf wie im Reiter, angetrieben
        // aus diesem Takt, Tabelle und Empfehlung auf die Konsole, danach
        // Schluss. Testbild an, ausser mit --ohne-testbild.
        let benchmark_dauer: Option<u32> = std::env::args().position(|a| a == "--benchmark").map(|i| {
            std::env::args().nth(i + 1).and_then(|v| v.parse().ok()).unwrap_or(5)
        });
        let ohne_testbild = std::env::args().any(|a| a == "--ohne-testbild");
        // --eingabeprobe: Diagnose ohne Fenster. Sobald der Eingabekanal
        // steht, gehen zwei Mausbewegungen hinaus (Bildmitte, dann rechts
        // unten, im Abstand von einer Sekunde), danach endet das Programm.
        // Ob sie ankamen, zeigt die Zeigerposition auf dem Host.
        let eingabeprobe = std::env::args().any(|a| a == "--eingabeprobe");
        const PROBEN: [(f32, f32); 2] = [(0.5, 0.5), (0.75, 0.75)];
        let mut probe_stand = 0usize;
        let mut probe_zeit: Option<Instant> = None;
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
        let hosts = bekanntgaben.clone();
        let mut gefunden: std::collections::HashSet<String> = std::collections::HashSet::new();
        let start = Instant::now();
        let mut last = 0u64;
        // Zugangsphase im Pruefmodus: Zustandszeilen, und --passwort geht
        // genau einmal hinaus (ein falsches zu wiederholen hiesse nur,
        // die Drossel des Hosts zu fuettern).
        let mut zugang_pruef = ZugangPruefmodus::neu(pruef_passwort);
        loop {
            if let Ok(h) = hosts.lock() {
                for g in h.liste() {
                    if gefunden.insert(g.host.addr.to_string()) {
                        let id = g.id.map(zugang::id_text).unwrap_or_else(|| "-".into());
                        let zulassen = if g.flags & BEACON_FLAG_ZULASSEN != 0 { ", Zulassen moeglich" } else { "" };
                        println!("Host gefunden: {} ({}, ID {id}{zulassen})", g.host.name, g.host.addr);
                    }
                }
            }
            // Der Drei-Sekunden-Takt in Scheiben von 50 ms: dazwischen
            // arbeitet der Benchmark, der seine Fristen selbst misst.
            let takt_ende = Instant::now() + Duration::from_secs(3);
            while Instant::now() < takt_ende {
                std::thread::sleep(Duration::from_millis(50));
                zugang_pruef.takt(&shared);
                // --bildschirm <Kennung|auto> wuenscht einmal einen Bildschirm:
                // sobald der Eingabekanal steht und der Host die Wahl gemeldet
                // hat (Bit 1) - ein aelterer Host bekommt nie Typ 70, das sagt
                // die Zeile dann statt des Wunsches. In der 50-ms-Scheibe,
                // nicht erst im Takt: die Eingabeprobe wartet auf den Wechsel,
                // und die endet frueher als der naechste Takt.
                if let Some(w) = bildschirm_wunsch.as_ref() {
                    if input.lock().unwrap().steht() {
                        if bildschirm_wunsch_senden(&shared, &input, w.clone()) {
                            println!("Bildschirmwunsch gesendet: {}", w.as_deref().unwrap_or("auto"));
                            bildschirm_wunsch = None;
                        } else {
                            let s = shared.lock().unwrap();
                            let ohne_wahl = s.faehigkeiten_da && !s.host_bildschirmwahl;
                            drop(s);
                            if ohne_wahl {
                                println!("Bildschirmwunsch nicht gesendet: der Host kennt keine Bildschirmwahl");
                                bildschirm_wunsch = None;
                            }
                        }
                    }
                }
                if eingabeprobe && probe_stand < PROBEN.len() {
                    // Mit --bildschirm erst, wenn der Wunsch hinaus ist und der
                    // Host ihn beantwortet hat (oder die Frist verstrichen
                    // ist): die Probe soll auf dem gewuenschten Bildschirm
                    // landen, nicht auf dem davor.
                    let wechsel = bildschirm_wunsch.is_some() || shared.lock().unwrap().bildschirm_wechsel_laeuft();
                    let faellig = !wechsel && probe_zeit.map_or(true, |t| t.elapsed() >= Duration::from_secs(1));
                    let mut l = input.lock().unwrap();
                    if faellig && l.steht() {
                        let (nx, ny) = PROBEN[probe_stand];
                        l.mouse_move(nx, ny);
                        drop(l);
                        println!("Eingabeprobe: Mausbewegung nach ({nx}, {ny}) eingereiht");
                        probe_stand += 1;
                        probe_zeit = Some(Instant::now());
                        if probe_stand == PROBEN.len() {
                            std::thread::sleep(Duration::from_secs(1));
                            println!("Eingabeprobe beendet");
                            std::process::exit(0);
                        }
                    }
                }
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
            // Ohne Verbindung keine Zahlen von gestern: Verzoegerung und
            // Hostlast gelten nur fuer eine stehende Sitzung.
            let lat = match s.clock {
                _ if !s.connected => "nicht verbunden".into(),
                Some(l) if l.gesamt_ms > 0.0 => format!(
                    "Verzoegerung {:.1} ms (+/-{:.1}) = Encoder {:.1} + Leitung {:.1} + Decoder {:.1} | Anzeige -",
                    l.gesamt_ms, l.umlauf_ms / 2.0, l.encoder_ms, l.leitung_ms, l.decoder_ms
                ),
                Some(l) => format!("Zeitabgleich steht, Umlauf {:.1} ms", l.umlauf_ms),
                None => "Zeitabgleich laeuft noch".into(),
            };
            let hl = match s.hostlast {
                Some(h) if s.connected => format!(
                    " | Host: CPU {:.0}% (eigen {:.0}%), GPU {}, RAM {:.1}/{:.0} GB, Encoder {:.1} ms, {:.0} Bilder/s",
                    h.cpu, h.cpu_eigen,
                    match h.gpu { Some(g) => format!("{g:.0}%"), None => "n/v".into() },
                    h.ram_benutzt_mb as f32 / 1024.0, h.ram_gesamt_mb as f32 / 1024.0,
                    h.encoder_ms, h.host_fps
                ),
                _ => String::new(),
            };
            // Der Fehler, wie ihn die Oberflaeche zeigt - auf Englisch, mit
            // seinem Schluessel davor; ein Schluessel ohne Meldung (Abloesung,
            // nicht gekoppelt, kein Bildschirm) hat wie im Fenster Vorrang.
            // Die deutschen Einzelheiten stehen in den Protokollzeilen darueber.
            let fehler = match s.error_key {
                Some(k) => Some(format!("{k:?}: {}", strings::EN.get(k))),
                None => s.error.as_ref().map(|m| format!("{:?}: {}", m.key, m.text(&strings::EN))),
            };
            // Der laufende Codec steht in jeder Zeile, damit ein Wechsel im
            // Protokoll sichtbar wird - derselbe Name wie im Menue und Overlay.
            let codec = s.info.map(|i| i.codec_name()).unwrap_or_else(|| "?".into());
            // Dazu die Masse des Stroms: ein Bildschirmwechsel des Hosts wird
            // so in der Zeile sichtbar, auch wenn der Codec bleibt.
            let strom = s.info.map(|i| format!("{}x{}@{}", i.width, i.height, i.fps)).unwrap_or_else(|| "?".into());
            let line = format!(
                "{:.0}s | decodiert {} ({:.1}/s) | Codec {} | Strom {} | Decoder {} | {}{} | Fehler {:?}",
                start.elapsed().as_secs_f32(), n, (n - last) as f32 / 3.0, codec, strom, pfad, lat, hl, fehler
            );
            println!("{line}");
            std::io::stdout().flush().ok();
            // Endete der Versuch mit einer Meldung, die bleibt (etwa ein
            // Ausgang des Zugangs: abgelehnt, zu viele Versuche, keine
            // Antwort), gibt es ohne Ziel nichts mehr zu tun.
            if s.target.is_none() && s.error.as_ref().is_some_and(Meldung::dauerhaft) {
                println!("Verbindung beendet, die Meldung bleibt - Ende des Pruefmodus");
                std::process::exit(1);
            }
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

    if let Some(g) = einzel_ohne {
        protokoll::zeile(format!("Einzelinstanz: nicht moeglich ({g}) - weiter ohne"));
    }
    // Die eine App: im Hintergrund anlaufen - ohne Fenster und Renderer, bis
    // "QuadChroma oeffnen" -, wenn es nicht der allererste Start ist oder
    // Autostart bzw. --host es so wollen. Mit einem Ziel, mit tray=aus (kein
    // Symbol, kein Weg zurueck) und beim allerersten Start geht das Fenster
    // auf.
    let im_hintergrund = start_im_hintergrund(
        MIT_FREIGABE,
        !start_addr.is_empty() || id_ausstehend.is_some(),
        cfg.tray,
        hintergrund_start,
        cfg.fenster_gezeigt,
    );
    // Mac: winit ohne sein Standardmenue (das Programmmenue und die Tasten
    // des Bearbeiten-Menues baut host/menue.m), als Accessory - nur das
    // Symbol in der Menueleiste, bis ein Fenster aufgeht (dann Regular, siehe
    // host_mac::aktivierung). Im Hintergrund nimmt die App beim Start auch
    // niemandem den Fokus.
    #[cfg(target_os = "macos")]
    let el = {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        EventLoop::<Benutzer>::with_user_event()
            .with_default_menu(false)
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_activate_ignoring_other_apps(!im_hintergrund)
            .build()
            .expect("Ereignisschleife")
    };
    #[cfg(not(target_os = "macos"))]
    let el = EventLoop::<Benutzer>::with_user_event().build().expect("Ereignisschleife");
    // Weitergereichte Adressen eines zweiten Starts als Benutzerereignis in
    // die Schleife. Was vor ihrem Anlauf kam, wartet im Kanal. Ist die
    // Schleife zu, faellt die Weitergabe unbestaetigt weg (der zweite Start
    // bekommt 0), und dieser Faden endet.
    if let Some(eingang) = einzel_eingang {
        let proxy = el.create_proxy();
        std::thread::spawn(move || {
            while let Ok(w) = eingang.recv() {
                if proxy.send_event(Benutzer::Einzel(w)).is_err() {
                    break;
                }
            }
        });
    }
    let proxy = el.create_proxy();

    // Die Verknuepfung "Mit Windows starten" frueherer Fassungen (die
    // Host-Rolle mit --host) weicht der einen (--hintergrund).
    #[cfg(windows)]
    match verknuepfung::autostart_migrieren(None, &autostart_beschreibung(sprache)) {
        Ok(true) => protokoll::zeile("Mit Windows starten: die alte Verknuepfung der Freigabe ist durch die der App ersetzt".into()),
        Ok(false) => {}
        Err(e) => protokoll::zeile(format!("Mit Windows starten: alte Verknuepfung nicht ersetzt - {e}")),
    }
    // --host: dieser PC ist freigegeben (die alte Verknuepfung wollte es so).
    if host_start && !cfg.freigabe {
        cfg.freigabe = true;
        cfg.sichern();
    }
    // Die Host-Rolle im eigenen Faden. Das Symbol der App entsteht erst in
    // `resumed`; die Rolle fragt ueber symbol_steht, ob es steht ("Zulassen"
    // nur dann), und laesst Sprechblasen ueber ein Benutzerereignis zeigen.
    let symbol_steht: Arc<std::sync::OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>> = Arc::new(std::sync::OnceLock::new());
    let rolle = {
        let st = symbol_steht.clone();
        let hp = Mutex::new(el.create_proxy());
        let bp = Mutex::new(el.create_proxy());
        freigabe::Freigabe::starten(
            args.clone(),
            cfg.freigabe,
            Arc::new(move || st.get().is_some_and(|f| f())),
            Arc::new(move |text: &str| {
                if let Ok(p) = hp.lock() {
                    let _ = p.send_event(Benutzer::Hinweis(text.to_string()));
                }
            }),
            Arc::new(move || {
                if let Ok(p) = bp.lock() {
                    let _ = p.send_event(Benutzer::RolleBereit);
                }
            }),
        )
    };
    if rolle.is_some() {
        protokoll::zeile(format!(
            "Freigabe: {}{} - {}, Protokoll in host-protokoll.txt",
            if cfg.freigabe { "an" } else { "aus" },
            if host_start { " (--host)" } else { "" },
            if cfg!(target_os = "macos") { "eingebaute Host-Engine, laeuft mit der Ereignisschleife an" } else { "Host-Rolle im eigenen Faden" }
        ));
    }
    if im_hintergrund {
        protokoll::zeile(format!("Start im Hintergrund{} - das Fenster kommt mit \"QuadChroma oeffnen\"", if hintergrund_start { " (Autostart bzw. --host)" } else { "" }));
    }
    // Ruhezustand verhindern, wenn eingestellt - bis zum Ende oder bis zum
    // Ausschalten.
    let mut ruhe = ruhezustand::Ruhesperre::neu();
    if cfg.ruhe_verhindern {
        ruhe_setzen(&mut ruhe, true, sprache);
    }
    let menue_quelle = Arc::new(Mutex::new(MenueQuelle { lang: sprache, ruhe_verhindern: cfg.ruhe_verhindern }));

    let mut app = App {
        shared,
        input: input.clone(),
        ui: ui::Ui::new(),
        screen: if start_addr.is_empty() && id_ausstehend.is_none() { Screen::Start } else { Screen::Session },
        hosts: bekanntgaben,
        addr_input: start_addr,
        lang: sprache,
        sprachwahl: false,
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
        zugang_pw: String::new(),
        zugang_caret: 0,
        zugang_zeigen: false,
        zugang_runde: None,
        bekannte: zugang::Hostliste::default(),
        bekannte_stand: None,
        id_ausstehend,
        rolle,
        hintergrund: im_hintergrund,
        symbol_steht,
        menue_quelle,
        menue_zuordnung: Arc::new(Mutex::new(symbolmenue::Zuordnung::default())),
        ruhe,
        angewandt_fuer: None,
        letzte_zeichnung: Instant::now(),
        oberflaeche_vorher: false,
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
        verknuepfung_meldung: None,
        symbol: None,
        proxy,
        verborgen: false,
        tray_takt: Instant::now(),
        #[cfg(target_os = "macos")]
        verbergen_faellig: None,
        fullscreen: cfg.vollbild,
        pixel_exact: cfg.pixelgenau,
        show_overlay: cfg.overlay,
        cfg,
    };
    el.run_app(&mut app).expect("Fenster");
    // Die Host-Rolle verabschiedet einen Zuschauer (Grund 0) und schliesst
    // die Ports - vor allem anderen, solange er noch zuhoert.
    if let Some(r) = app.rolle.as_mut() {
        r.beenden();
    }
    // Das Symbol vor dem Prozessende entfernen (sonst bliebe es unter
    // Windows bis zur naechsten Mausbewegung darueber stehen); mit der App
    // faellt auch die Energieanforderung (Ruhezustand verhindern).
    drop(app);
    // Erst jetzt: bis hierher ist dies die erste Instanz.
    drop(einzel_waechter);
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
    /// Desktop-Verknuepfung fuer den verbundenen Host (nur Windows).
    Verknuepfung,
    /// Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton.
    Stellen(u32, u16, bool, bool, bool),
    Schalter(u8),
    /// Wunsch nach diesem Kandidaten der Koennensliste.
    Codec(u8),
    /// Wunsch nach diesem Bildschirm des Hosts (Kennung), None = Automatik.
    Bildschirm(Option<String>),
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
    /// Bildschirmwahl im Reiter "Bild": kennt der Host sie (Bit 1 seiner
    /// Faehigkeiten - sonst keine Zeile), seine Liste, sein Wunsch (None =
    /// Automatik) und ob ein Bildschirmwunsch unterwegs ist.
    pub bildschirmwahl: bool,
    pub bildschirme: Vec<bildschirm::BildschirmEintrag>,
    pub bildschirm_wunsch: Option<String>,
    pub bildschirm_wechsel: bool,
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
    /// Ergebnis der letzten Desktop-Verknuepfung (Text, Farbe), solange es
    /// stehen soll - im Reiter Verschluesselung unter den Knoepfen.
    pub verknuepfung: Option<(String, u32)>,
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

/// Die Reiter des ESC-Menues, in ihrer Reihenfolge (Index = Reiter).
fn hud_reiternamen(lang: &'static strings::Lang) -> [&'static str; 5] {
    use strings::Key::*;
    [lang.get(TabPicture), lang.get(TabDisplay), lang.get(Encryption), lang.get(TabShortcuts), lang.get(TabBenchmark)]
}

/// Lage des Kopfes im ESC-Menue: die Reiter mit ihrer Aufschrift (in der
/// letzten Stufe gekuerzt) und der Knopf Trennen (fest rechts, auf jedem
/// Reiter, die Adresse des Hosts darunter). `logo`: ob der Schriftzug
/// "QUADCHROMA" links vor den Reitern steht.
struct HudKopf {
    reiter: [ui::Rect; 5],
    namen: [String; 5],
    trennen: ui::Rect,
    logo: bool,
}

/// Dieselbe Rechnung wie in `hud` (Tafel, Rand, Massstab), nur die Lage.
/// Das Fenster hat keine Mindestgroesse: reicht der Platz nicht, weicht der
/// Kopf stufenweise aus, bis die Reiter links vor Trennen enden - erst
/// engere Reiter, dann ohne den Schriftzug, zuletzt gleich breite Reiter mit
/// gekuerzter Aufschrift. Trennen behaelt immer seinen ganzen Text.
fn hud_kopf(u: &mut ui::Ui, lang: &'static strings::Lang, ww: i32, wh: i32) -> HudKopf {
    let s: f32 = if wh >= 1800 { 2.0 } else if wh >= 1000 { 1.5 } else { 1.0 };
    let p = |v: i32| -> i32 { (v as f32 * s).round() as i32 };
    let sz = |v: u32| -> u32 { (v as f32 * s).round() as u32 };
    let breite = (ww - p(80)).min(p(1100));
    let hoehe = (wh - p(80)).min(p(570));
    let x0 = (ww - breite) / 2;
    let y0 = (wh - hoehe) / 2;
    let rand = p(20);
    let ix = x0 + rand;
    let rechts = x0 + breite - rand;
    let namen = hud_reiternamen(lang);
    let textbreiten = namen.map(|n| u.text.width(n, sz(12), p(2)));
    let trenntext = u.text.width(lang.get(strings::Key::Disconnect), sz(12), p(2));
    let logo_w = u.text.width("QUADCHROMA", sz(18), p(8)) + p(40);
    let leer = ui::Rect { x: 0, y: 0, w: 0, h: 0 };
    // Die Stufen: Schriftzug ja/nein, Innenabstand der Knoepfe, Luecke.
    for (logo, innen, luecke) in [(true, p(28), p(8)), (true, p(14), p(4)), (false, p(14), p(4))] {
        let tw = trenntext + innen;
        let trennen = ui::Rect { x: rechts - tw, y: y0 + p(12), w: tw, h: p(26) };
        let mut reiter = [leer; 5];
        let mut rx = ix + if logo { logo_w } else { 0 };
        for (r, &w) in reiter.iter_mut().zip(textbreiten.iter()) {
            *r = ui::Rect { x: rx, y: y0 + p(12), w: w + innen, h: p(26) };
            rx += w + innen + luecke;
        }
        // rx steht jetzt eine Luecke hinter dem letzten Reiter.
        if rx <= trennen.x {
            return HudKopf { reiter, namen: namen.map(str::to_string), trennen, logo };
        }
    }
    // Letzte Stufe: die Breite links vor Trennen zu gleichen Teilen, jede
    // Aufschrift auf ihren Teil gekuerzt (mit Auslassungszeichen).
    let (innen, luecke) = (p(14), p(4));
    let tw = trenntext + innen;
    let trennen = ui::Rect { x: (rechts - tw).max(ix), y: y0 + p(12), w: tw, h: p(26) };
    let bw = ((trennen.x - ix - 5 * luecke) / 5).max(0);
    let mut reiter = [leer; 5];
    for (i, r) in reiter.iter_mut().enumerate() {
        *r = ui::Rect { x: ix + i as i32 * (bw + luecke), y: y0 + p(12), w: bw, h: p(26) };
    }
    let namen = namen.map(|n| kuerzen(u, n, bw - innen, sz(12), p(2)));
    HudKopf { reiter, namen, trennen, logo: false }
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
    let kopf = hud_kopf(u, lang, ww, wh);
    if kopf.logo {
        u.text.draw(c, ix, y0 + p(30), "QUADCHROMA", sz(18), ui::CYAN, p(8));
    }
    for (i, (n, &r)) in kopf.namen.iter().zip(kopf.reiter.iter()).enumerate() {
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
    }
    // Trennen steht fest rechts im Kopf, auf jedem Reiter; die Adresse des
    // Hosts buendig darunter, knapp unter der Kopflinie - neben den Reitern
    // reicht der Platz in vielen Sprachen nicht fuer beides.
    if kopf.trennen.hit(maus.0, maus.1) {
        tip = Some(TipDisconnect);
    }
    if u.button_mit(c, kopf.trennen, lang.get(Disconnect), ui::MAGENTA, sz(12), p(2)) {
        aktion = HudAktion::Trennen;
    }
    let a = kuerzen(u, adresse, iw / 3, sz(11), p(1));
    u.text.draw_right(c, kopf.trennen.x + kopf.trennen.w, y0 + p(62), &a, sz(11), ui::DIM, p(1));
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

            // --- Bildschirmwahl: "Automatisch" und ein Knopf je Bildschirm --
            // Nur, wenn der Host die Wahl kennt (Bit 1 seiner Faehigkeiten).
            // Der gestreamte Eintrag in Cyan; bei Automatik dazu
            // "Automatisch", bei festem Wunsch der gewuenschte Eintrag, falls
            // er angeschlossen ist. Rechts in Amber: der laufende Wechsel,
            // sonst der Ausweichplatz, wenn der gewuenschte Bildschirm fehlt.
            // Die Knoepfe fliessen zeilenweise wie die Codecknoepfe; der
            // Codecblock rueckt darunter, der Fusszeilenschutz bleibt.
            // Ohne die Zeile steht der Codecblock, wo er immer stand.
            let mut oy = cy + p(176);
            if stand.bildschirmwahl {
                let ly = cy + p(170);
                let lw = u.text.width(lang.get(ScreenLabel), sz(11), p(3));
                u.text.draw(c, ix, ly, lang.get(ScreenLabel), sz(11), ui::DIM, p(3));
                let gestreamt = stand.bildschirme.iter().find(|e| e.gestreamt);
                let fehlt = stand
                    .bildschirm_wunsch
                    .as_deref()
                    .filter(|w| !stand.bildschirme.iter().any(|e| e.kennung == *w));
                let hinweis: Option<String> = if stand.bildschirm_wechsel {
                    Some(lang.get(ScreenSwitching).to_string())
                } else {
                    fehlt.map(|w| {
                        let m = gestreamt.map(bildschirm_anzeigename).unwrap_or("?");
                        lang.get(ScreenFallback).replace("{n}", w).replace("{m}", m)
                    })
                };
                if let Some(h) = hinweis {
                    let t = kuerzen(u, &h, iw - lw - p(24), sz(11), p(1));
                    u.text.draw_right(c, x0 + breite - rand, ly, &t, sz(11), ui::AMBER, p(1));
                }
                if (ui::Rect { x: ix, y: ly - p(12), w: iw, h: p(16) }).hit(maus.0, maus.1) {
                    tip = Some(TipScreen);
                }
                // Knopf: Text, in Cyan, schon der Wunsch (dann kein Klick),
                // und was ein Klick wuenscht (None = Automatik).
                let auto = stand.bildschirm_wunsch.is_none();
                let mut knoepfe: Vec<(String, bool, bool, Option<String>)> =
                    vec![(lang.get(ScreenAuto).to_string(), auto, auto, None)];
                for e in &stand.bildschirme {
                    let gewuenscht = stand.bildschirm_wunsch.as_deref() == Some(e.kennung.as_str());
                    knoepfe.push((bildschirm_knopftext(e), e.gestreamt || gewuenscht, gewuenscht, Some(e.kennung.clone())));
                }
                let mut kx = ix;
                let mut ky = ly + p(10);
                let kh = p(30);
                let pitch = p(40);
                let luecke = p(10);
                let mut letzte = ky;
                for (text, cyan, ist_wunsch, wunsch) in knoepfe {
                    let bw = u.text.width(&text, 15, 2) + p(28);
                    if kx + bw > ix + iw && kx > ix {
                        kx = ix;
                        ky += pitch;
                    }
                    if ky + kh + p(14) > fy - p(24) {
                        break;
                    }
                    let r = ui::Rect { x: kx, y: ky, w: bw, h: kh };
                    if r.hit(maus.0, maus.1) { tip = Some(TipScreen); }
                    if u.button(c, r, &text, if cyan { ui::CYAN } else { ui::DIM }) && !ist_wunsch {
                        aktion = HudAktion::Bildschirm(wunsch);
                    }
                    letzte = ky;
                    kx += bw + luecke;
                }
                oy = letzte + kh + p(22);
            }

            // --- Codecwahl: ein Knopf je Eintrag der Koennensliste ----------
            // Der laufende Eintrag in Cyan, die anderen gedaempft, was der
            // Mac nicht kann, noch dunkler und ohne Klick. Die Knoepfe
            // fliessen zeilenweise, damit auch ein schmales Fenster alle zeigt.
            if !stand.codecs.is_empty() {
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
                        // Auf dem Mac zeichnet Metal (anzeige_mac.rs).
                        RollenWahl::Automatik => (
                            if cfg!(target_os = "macos") { TipDisplayAutoMac } else { TipDisplayAuto },
                            beschreibung(karte_automatik(&stand.karten)),
                        ),
                        RollenWahl::Gpu | RollenWahl::Gpu2 => (TipDisplayGpu, beschreibung(*karte)),
                        RollenWahl::Integriert => (TipDisplayIntegrated, beschreibung(*karte)),
                        RollenWahl::Prozessor => (if cfg!(target_os = "macos") { TipDisplayCpuMac } else { TipDisplayCpu }, None),
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
                        // Auf dem Mac decodiert VideoToolbox (vt_decoder.rs).
                        RollenWahl::Automatik => (if cfg!(target_os = "macos") { TipDecoderAutoMac } else { TipDecoderAuto }, None),
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
            // Die erste Zeile beginnt unter der Adresse des Hosts (Grundlinie
            // y0 + p(62), rechts unter Trennen) - sonst schreibt eine lange
            // Adresse oder ein schmales Fenster in die Codec-Knoepfe.
            let by = y0 + p(70);
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
            // Desktop-Verknuepfung fuer diesen Host (nur Windows); Trennen
            // steht im Kopf. Das Ergebnis steht 6 s darunter.
            if MIT_VERKNUEPFUNG {
                let t = lang.get(DesktopShortcut);
                let bw = p(220).max(u.text.width(t, 15, 2) + p(40));
                let r_verkn = ui::Rect { x: ix, y: cy + p(130), w: bw, h: p(38) };
                if r_verkn.hit(maus.0, maus.1) { tip = Some(TipDesktopShortcut); }
                if u.button(c, r_verkn, t, ui::CYAN) {
                    aktion = HudAktion::Verknuepfung;
                }
                if let Some((text, farbe)) = &stand.verknuepfung {
                    for (i, z) in umbruch(u, text, iw, sz(12)).iter().enumerate() {
                        u.text.draw(c, ix, r_verkn.y + r_verkn.h + p(34) + i as i32 * p(18), z, sz(12), *farbe, p(1));
                    }
                }
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
    /// Sprachwahl: vier Spalten, wenn Platz ist; schmale Fenster bekommen
    /// weniger, niedrige mehr - nie mehr Spalten, als in die Breite passen.
    #[test]
    fn sprachwahl_raster_passt_sich_an() {
        assert_eq!(sprachwahl_raster(1800, 900, 29, 200, 30), (4, 8));
        assert_eq!(sprachwahl_raster(820, 500, 29, 200, 30), (4, 8));
        assert_eq!(sprachwahl_raster(450, 900, 29, 200, 30), (2, 15));
        assert_eq!(sprachwahl_raster(1800, 200, 29, 200, 30), (5, 6));
        assert_eq!(sprachwahl_raster(100, 100, 29, 200, 30), (1, 29));
    }

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

    /// Der Decoder-Unterbau fuer die Sitzungstests: FFmpeg unter Windows;
    /// VideoToolbox (Mac) braucht keinen Start.
    fn decoder_bereit() {
        #[cfg(windows)]
        ffmpeg::init().unwrap();
    }

    /// Kann dieser Rechner die Bilder der Scheinhosts decodieren? Unter
    /// Windows immer (FFmpeg). Auf dem Mac nur, wenn VideoToolbox erreichbar
    /// ist - etwa in einer Sandbox ohne Zugang zu seinen Diensten nicht;
    /// dann uebergehen die Tests, die Bilder zaehlen, sich selbst.
    fn decodieren_moeglich() -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut d = vt_decoder::Decoder::neu(true, false);
            let mut b = Vec::new();
            match d.fuettern(&hex(BILD_64X64), 0, &mut b) {
                Ok(()) if !b.is_empty() => {}
                r => {
                    eprintln!("uebersprungen: VideoToolbox decodiert das Probebild nicht ({r:?})");
                    return false;
                }
            }
        }
        true
    }

    /// Ein gemeinsamer Schluessel fuer die Schein-Hosts auf 127.0.0.1 (aus
    /// der Zeit, als die Tests sich eine known_hosts.txt teilten, in der
    /// 127.0.0.1 nur einmal stand; heute pinnt hosts.txt nach Schluessel).
    /// Er steht von Anfang an in hosts.txt dieses Testlaufs: die
    /// Schein-Hosts sagen gleich "QCH1", und das nimmt der Client nur von
    /// einem Host an, dessen Schluessel er kennt (sonst Bit 0 in Nachricht 3,
    /// Spezifikation Pairing v1, 1.4).
    fn test_host() -> (Vec<u8>, Vec<u8>) {
        static K: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
        K.get_or_init(|| {
            let k = noise::keypair().unwrap();
            vorher_pinnen(&k.1, "127.0.0.1:1");
            k
        })
        .clone()
    }

    /// Diesen Host in hosts.txt des Testlaufs eintragen, als waere er schon
    /// einmal angenommen worden.
    fn vorher_pinnen(host_pub: &[u8], addr: &str) {
        secure::test_identitaet();
        let p = zugang::ablage_pfad(zugang::HOSTS_DATEI).unwrap();
        zugangsphase::pinnen(&p, host_pub, addr, "Vorher").unwrap();
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
        // 1 (IN_FAEHIGKEITEN als erste auf dem Kanal) + 6 + 1
        assert_eq!(l.sent, 8);
        let _ = fertig_tx.send(());
        host.join().unwrap();
    }

    /// Ein Eingabekanal-Host fuer die Tests unten: nimmt eine Verbindung an
    /// (nach `vorher`), wartet auf das Zeichen und liest dann Nachrichten,
    /// bis eine mit der Art `ende` kam oder die Leitung zu ist. Ergebnis:
    /// Art und Nutzlast jeder Nachricht, dazu, ob die Leitung zuging.
    fn eingabe_host(
        hh: Vec<u8>,
        k: Vec<u8>,
        vorher: Duration,
        ende: u8,
    ) -> (String, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<(Vec<(u8, Vec<u8>)>, bool)>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (los_tx, los_rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            std::thread::sleep(vorher);
            let (s, _) = l.accept().unwrap();
            let mut h = secure::Secure::accept(s, &noise::prologue_input(&hh), &k).unwrap();
            let _ = los_rx.recv();
            let mut gelesen = Vec::new();
            loop {
                let mut kopf = [0u8; 8];
                if h.read_exact(&mut kopf).is_err() {
                    return (gelesen, true);
                }
                let mut p = vec![0u8; u32::from_le_bytes(kopf[4..8].try_into().unwrap()) as usize];
                if h.read_exact(&mut p).is_err() {
                    return (gelesen, true);
                }
                gelesen.push((kopf[0], p));
                if kopf[0] == ende {
                    return (gelesen, false);
                }
            }
        });
        (addr, los_tx, t)
    }

    /// Aufeinanderfolgende Bewegungen: nur die letzte; dazwischen liegende
    /// Klicks und Tasten trennen die Laeufe, die Reihenfolge bleibt.
    #[test]
    fn bewegungen_werden_zusammengefasst() {
        let b = |t: u8, x: u8| eingabe_rahmen(t, &[x]);
        let stapel = vec![
            b(IN_MOVE, 1), b(IN_MOVE, 2), b(IN_BUTTON, 3), b(IN_MOVE, 4), b(IN_MOVE, 5), b(IN_MOVE, 6),
            b(IN_KEY, 7), b(IN_MOVE, 8), b(IN_CLIP, 9), b(IN_MOVE, 10),
        ];
        let aus: Vec<(u8, u8)> = bewegungen_zusammenfassen(stapel).iter().map(|r| (r[0], r[8])).collect();
        assert_eq!(
            aus,
            vec![(IN_MOVE, 2), (IN_BUTTON, 3), (IN_MOVE, 6), (IN_KEY, 7), (IN_MOVE, 8), (IN_CLIP, 9), (IN_MOVE, 10)]
        );
    }

    /// Stockt die Leitung, gehen danach nicht alle alten Lagen einzeln
    /// hinaus, sondern nur die letzte - Zwischenablage und Tasten bleiben
    /// vollstaendig und in Reihenfolge. Und trennen() kappt die Leitung
    /// sofort, statt die Warteschlange noch abzuarbeiten.
    #[test]
    fn eingabekanal_nach_stocken_nur_die_letzte_lage() {
        secure::test_identitaet();
        let hh = vec![0x71; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let gross = vec![b'x'; 4 * 1024 * 1024];

        let (addr, los, host) = eingabe_host(hh.clone(), host_priv.clone(), Duration::ZERO, IN_KEY);
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh.clone(), host_pub.clone())));
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        // Verstopfen: der Host liest noch nicht, der Schreibfaden haengt.
        for _ in 0..6 {
            l.send(IN_CLIP, &gross);
        }
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..2000 {
            l.mouse_move(i as f32 / 2000.0, 0.25);
        }
        l.key(4, true, 0);
        let _ = los.send(());
        let (gelesen, zu) = host.join().unwrap();
        assert!(!zu);
        let arten: Vec<u8> = gelesen.iter().map(|(a, _)| *a).collect();
        let bewegungen = arten.iter().filter(|a| **a == IN_MOVE).count();
        assert!(bewegungen <= 3, "{bewegungen} Bewegungen kamen einzeln an");
        assert_eq!(arten.iter().filter(|a| **a == IN_CLIP).count(), 6);
        assert!(gelesen.iter().filter(|(a, _)| *a == IN_CLIP).all(|(_, p)| *p == gross));
        let mut erwartet = (1999.0f32 / 2000.0).to_le_bytes().to_vec();
        erwartet.extend_from_slice(&0.25f32.to_le_bytes());
        assert_eq!(gelesen.iter().rev().find(|(a, _)| *a == IN_MOVE).map(|(_, p)| p.clone()), Some(erwartet));
        assert_eq!(arten.last(), Some(&IN_KEY));

        // trennen() mit voller Warteschlange: der Host sieht das Ende, bevor
        // alles Eingereihte bei ihm ist.
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, IN_KEY);
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh, host_pub)));
        eingabe_abwarten(&mut l);
        for _ in 0..6 {
            l.send(IN_CLIP, &gross);
        }
        l.key(5, true, 0);
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        l.trennen();
        let _ = los.send(());
        let (gelesen, zu) = host.join().unwrap();
        assert!(zu, "Leitung nicht gekappt");
        assert!(gelesen.len() < 6, "Warteschlange nach dem Trennen noch abgearbeitet ({} Nachrichten)", gelesen.len());
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    /// Nimmt der Host nichts mehr ab, gilt der Kanal nach der Schreibfrist
    /// als kaputt, statt dass der Schreibfaden unbegrenzt haengt.
    #[test]
    fn eingabekanal_schreibfrist() {
        secure::test_identitaet();
        let hh = vec![0x72; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, IN_KEY);
        let mut l = InputLink::new(addr);
        l.schreibfrist = Duration::from_millis(300);
        l.set_link(Some((hh, host_pub)));
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        let gross = vec![b'x'; 4 * 1024 * 1024];
        for _ in 0..6 {
            l.send(IN_CLIP, &gross);
        }
        let t0 = Instant::now();
        while l.steht() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!l.steht(), "Schreibfaden haengt ohne Frist");
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        let _ = los.send(());
        let (_, zu) = host.join().unwrap();
        assert!(zu);
    }

    /// Einstellungen, Codec, Testbild und Zwischenablage, die ohne stehenden
    /// Kanal kamen, gehen hinaus, sobald er steht - je Art die letzte.
    /// Bewegungen und Tasten aus dieser Zeit fallen weg. Eine neue Bindung
    /// verwirft, was fuer die alte gemerkt war.
    #[test]
    fn zustand_wird_nachgereicht() {
        secure::test_identitaet();
        let hh = vec![0x73; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        // Der Host nimmt erst nach 300 ms an: so lange steht der Kanal sicher nicht.
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::from_millis(300), IN_KEY);
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh.clone(), host_pub.clone())));
        l.testbild(true);
        l.settings(10, 60, false, false, true);
        l.mouse_move(0.3, 0.3);
        l.key(7, true, 0);
        l.codec(2);
        l.settings(20, 120, true, true, false);
        // Der Bildschirmwunsch ist ebenfalls ein Zustand (Spezifikation
        // Bildschirmwahl 2.3): der letzte je Art wird gemerkt.
        l.bildschirm(Some("v1138-m1234-s0"));
        l.bildschirm(Some("v0-m0-s0"));
        assert!(!l.steht());
        assert_eq!(l.sent, 0);
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        // Die Faehigkeiten vorweg, dann die vier gemerkten Zustaende.
        assert_eq!(l.sent, 5);
        l.key(9, true, 0);
        let _ = los.send(());
        let (gelesen, _) = host.join().unwrap();
        let mut einst = 20u32.to_le_bytes().to_vec();
        einst.extend_from_slice(&120u16.to_le_bytes());
        einst.extend_from_slice(&[1, 1, 0]);
        let mut taste = 9u16.to_le_bytes().to_vec();
        taste.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
        assert_eq!(
            gelesen,
            vec![
                (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]),
                (IN_TESTBILD, vec![1]),
                (IN_CODEC, vec![2]),
                (IN_SETTINGS, einst),
                (IN_BILDSCHIRM, bildschirm::wunsch_kodieren(Some("v0-m0-s0"))),
                (IN_KEY, taste)
            ]
        );

        // Neue Bindung: nichts von der alten wird nachgereicht.
        let mut l = InputLink::new("127.0.0.1:1".into());
        l.set_link(Some((hh.clone(), host_pub.clone())));
        l.codec(3);
        assert_eq!(l.nachreichen.len(), 1);
        l.set_link(Some((vec![0x74; 32], host_pub)));
        assert!(l.nachreichen.is_empty());
        l.codec(3);
        l.trennen();
        assert!(l.nachreichen.is_empty());
    }

    /// Ein fuer diesen Host gespeichertes "Ton aus" gilt lokal, sobald der
    /// Bildkanal steht - auch wenn der Eingabekanal ausbleibt. Die
    /// Einstellungen liegen dann zum Nachreichen bereit, einmal je Host.
    /// Wechselt die Sitzung, bevor sie hinaus waren, kommen sie in der neuen
    /// noch einmal. Frueher wartete das alles auf den Eingabekanal.
    #[test]
    fn gespeicherte_werte_gelten_ohne_eingabekanal() {
        let mut cfg = einstellungen::Einstellungen::default();
        let w = einstellungen::HostWerte { mbit: 30, fps: 60, gaming: false, fest: true, ton: false };
        cfg.hosts.insert("fp-a".into(), w);
        let shared = Mutex::new(Shared { ton: true, ..Shared::default() });
        // Ohne Adresse baut sich der Eingabekanal nie auf.
        let input = Mutex::new(InputLink::new(String::new()));
        let mut angewandt = None;
        let einst = eingabe_rahmen(IN_SETTINGS, &[30, 0, 0, 0, 60, 0, 0, 1, 0]);

        // Noch kein Bildkanal: nichts.
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert_eq!(angewandt, None);
        assert!(shared.lock().unwrap().ton);

        // Der Bildkanal steht (run_session setzt Bindung und Host zugleich).
        {
            let mut s = shared.lock().unwrap();
            s.link = Some((vec![1; 32], vec![2; 32]));
            s.peer_fp = Some("fp-a".into());
        }
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert!(!input.lock().unwrap().steht());
        assert!(!shared.lock().unwrap().ton, "gespeichertes Ton aus gilt lokal nicht");
        assert_eq!(angewandt.as_deref(), Some("fp-a"));
        assert_eq!(input.lock().unwrap().nachreichen, vec![einst.clone()]);
        // Einmal je Host: der naechste Durchlauf schickt nichts dazu, und
        // ein Ton an aus dem Menue bleibt stehen.
        shared.lock().unwrap().ton = true;
        input.lock().unwrap().nachreichen.clear();
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert!(input.lock().unwrap().nachreichen.is_empty());
        assert!(shared.lock().unwrap().ton);

        // Die Einstellungen liegen wieder bereit, dann faellt die Sitzung weg
        // und eine neue kommt (derselbe Host): in ihr noch einmal.
        input.lock().unwrap().settings(w.mbit, w.fps, w.gaming, w.fest, w.ton);
        shared.lock().unwrap().link = None;
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert!(input.lock().unwrap().nachreichen.is_empty());
        shared.lock().unwrap().link = Some((vec![3; 32], vec![2; 32]));
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert_eq!(angewandt.as_deref(), Some("fp-a"));
        assert_eq!(input.lock().unwrap().nachreichen, vec![einst]);
        assert!(!shared.lock().unwrap().ton);

        // Ein Host ohne gespeicherte Werte: Ton an, nichts zu senden.
        {
            let mut s = shared.lock().unwrap();
            s.link = Some((vec![4; 32], vec![5; 32]));
            s.peer_fp = Some("fp-b".into());
        }
        gespeicherte_werte_anwenden(&cfg, &shared, &input, &mut angewandt);
        assert_eq!(angewandt.as_deref(), Some("fp-b"));
        assert!(shared.lock().unwrap().ton);
        assert!(input.lock().unwrap().nachreichen.is_empty());
    }

    /// Ein Fehler zaehlt nur fuer das Ziel, zu dem er gehoert: hat der
    /// Nutzer inzwischen einen anderen Host gewaehlt, bleibt dessen Ziel
    /// stehen, auch bei einem Dauerfehler des alten; nach einer gewollten
    /// Trennung (kein Ziel) wird nichts angezeigt.
    #[test]
    fn fehler_gilt_nur_fuer_das_eigene_ziel() {
        let dauer = Meldung::from(secure::Fehler::AnderesGeraet { addr: "10.0.0.5:9001".into(), erwartet: 1, gemeldet: 2 });
        assert!(dauer.dauerhaft());
        let mut s = Shared { target: Some("10.0.0.6:9001".into()), connected: true, ..Shared::default() };
        let mut gemeldet = None;
        fehler_verbuchen(&mut s, "10.0.0.5:9001", dauer.clone(), &mut gemeldet);
        assert_eq!(s.target.as_deref(), Some("10.0.0.6:9001"));
        assert_eq!(s.error, None);
        assert!(gemeldet.is_none());
        assert!(!s.connected);
        s.target = None;
        fehler_verbuchen(&mut s, "10.0.0.5:9001", dauer.clone(), &mut gemeldet);
        assert_eq!(s.error, None);
        s.target = Some("10.0.0.5:9001".into());
        fehler_verbuchen(&mut s, "10.0.0.5:9001", dauer.clone(), &mut gemeldet);
        assert_eq!(s.target, None);
        assert_eq!(s.error, Some(dauer));
    }

    /// Selbstschutz beim Eintippen: die eigene ID gibt "Das ist dieser
    /// Computer.", mit oder ohne Leerzeichen; eine andere ID, eine Adresse
    /// oder gar kein eigener Host nicht.
    #[test]
    fn eigene_id_eingetippt() {
        let m = eigene_id_eingegeben("123 456 789", Some(123_456_789)).unwrap();
        assert_eq!(m.key, strings::Key::MsgSelf);
        assert!(m.dauerhaft());
        assert_eq!(m.text(strings::pick("de")), "Das ist dieser Computer.");
        assert!(eigene_id_eingegeben("123456789", Some(123_456_789)).is_some());
        assert!(eigene_id_eingegeben("123 456 780", Some(123_456_789)).is_none());
        assert!(eigene_id_eingegeben("10.0.0.5", Some(123_456_789)).is_none());
        assert!(eigene_id_eingegeben("123 456 789", None).is_none());
    }

    /// Verbinden zu einem Host, dessen Schluessel unser eigener host.key ist
    /// (dieser Rechner selbst): Abbruch nach Nachricht 2 - der Host sieht
    /// Nachricht 3 nie, nimmt also niemanden an und loest niemanden ab -,
    /// "Das ist dieser Computer." bleibt stehen, kein Neuversuch.
    #[test]
    fn verbinden_zu_sich_selbst_abgewiesen() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        secure::test_identitaet();
        // Der eigene Host dieses Testlaufs: sein host.key liegt in der
        // Testablage (nie der echte).
        let (host_priv, host_pub) = secure::host_identity().unwrap();
        assert_eq!(secure::eigener_host_schluessel(), Some(host_pub.clone()));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (verbindungen, nachricht3) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (v, n3) = (verbindungen.clone(), nachricht3.clone());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                v.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut h) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) {
                    n3.fetch_add(1, Ordering::SeqCst);
                    let _ = h.write_all(MAGIC);
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
        std::thread::sleep(Duration::from_millis(2500));
        {
            let s = shared.lock().unwrap();
            assert!(s.target.is_none(), "Ziel nicht zurueckgenommen");
            let m = s.error.as_ref().expect("keine Meldung");
            assert_eq!(m.key, strings::Key::MsgSelf, "{}", m.protokoll);
            assert!(m.dauerhaft());
            assert_eq!(s.sitzung_nr, 0);
        }
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
        assert_eq!(nachricht3.load(Ordering::SeqCst), 0, "der Host hat Nachricht 3 gesehen");
    }

    /// Die Frist der Neuversuche als reine Buchfuehrung: eine Reihe ohne
    /// Sitzung zaehlt ab dem ersten Versuch und endet nach der Frist mit dem
    /// letzten Fehler, der dann bleibt; eine Sitzung, die eben zu Ende ging,
    /// beginnt die Frist neu, und Aufgeben heisst dann "Verbindung verloren".
    /// Ein anderes Ziel oder keins beendet die Reihe ohne Meldung.
    #[test]
    fn neuversuche_enden_nach_der_frist() {
        let frist = Duration::from_secs(30);
        let t0 = Instant::now();
        let s_ = |n: u64| t0 + Duration::from_secs(n);
        let a = "10.0.0.5:9001";
        let abgelehnt = Meldung::neu(strings::Key::ErrorConnectRefused, "abgelehnt");
        let mut s = Shared { target: Some(a.into()), error: Some(abgelehnt.clone()), ..Shared::default() };
        let mut reihe = None;
        // Erste Verbindung kommt nicht zustande.
        assert!(!neuversuch_buchen(&mut s, a, &mut reihe, s_(0), false, s_(1), frist));
        assert_eq!(reihe.as_ref().map(|r| (r.seit, r.nach_sitzung)), Some((s_(0), false)));
        assert!(!neuversuch_buchen(&mut s, a, &mut reihe, s_(28), false, s_(29), frist));
        assert_eq!(s.target.as_deref(), Some(a));
        assert!(neuversuch_buchen(&mut s, a, &mut reihe, s_(30), false, s_(31), frist));
        assert_eq!(s.target, None);
        assert!(reihe.is_none());
        let m = s.error.clone().unwrap();
        assert_eq!(m.key, strings::Key::ErrorConnectRefused);
        assert!(m.dauerhaft(), "der letzte Fehler bleibt");

        // Eine Sitzung stand und ging eben zu Ende: die Frist beginnt dann.
        let mut s = Shared { target: Some(a.into()), ..Shared::default() };
        let mut reihe = Some(Neuversuche { ziel: a.into(), seit: s_(0), nach_sitzung: false });
        assert!(!neuversuch_buchen(&mut s, a, &mut reihe, s_(1), true, s_(100), frist));
        assert_eq!(reihe.as_ref().map(|r| (r.seit, r.nach_sitzung)), Some((s_(100), true)));
        s.error = Some(Meldung::neu(strings::Key::ConnectionLost, "Kopf: zurueckgesetzt"));
        assert!(!neuversuch_buchen(&mut s, a, &mut reihe, s_(120), false, s_(125), frist));
        assert!(neuversuch_buchen(&mut s, a, &mut reihe, s_(128), false, s_(131), frist));
        let m = s.error.clone().unwrap();
        assert_eq!((m.key, s.target.clone()), (strings::Key::ConnectionLost, None));
        assert!(m.dauerhaft());
        assert!(m.protokoll.contains("seit 31 s keine Verbindung - aufgegeben"), "{}", m.protokoll);

        // Getrennt oder anderes Ziel: die Reihe endet, nichts wird gemeldet.
        let mut s = Shared { target: Some("10.0.0.6:9001".into()), ..Shared::default() };
        let mut reihe = Some(Neuversuche { ziel: a.into(), seit: s_(0), nach_sitzung: true });
        assert!(!neuversuch_buchen(&mut s, a, &mut reihe, s_(40), false, s_(41), frist));
        assert!(reihe.is_none());
        assert_eq!((s.target.as_deref(), s.error.is_none()), (Some("10.0.0.6:9001"), true));
        // Eine neue Adresse desselben Ziels (ueber die ID) beginnt keine neue
        // Reihe - das fuehrt stream_thread, indem es `ziel` mitzieht.
        let b = "10.0.0.7:9001";
        let mut s = Shared { target: Some(b.into()), ..Shared::default() };
        let mut reihe = Some(Neuversuche { ziel: b.into(), seit: s_(0), nach_sitzung: true });
        assert!(neuversuch_buchen(&mut s, b, &mut reihe, s_(30), false, s_(31), frist));
    }

    /// Verbindungsabbruch ohne Abschied, und der Host kommt nicht wieder
    /// (der Port lehnt ab): der Empfangsfaden versucht es nur bis zur Frist
    /// (hier 3 s statt 30 s), dann ist das Ziel zurueck und "Verbindung
    /// verloren" bleibt stehen - kein Endlos-"Verbinde neu".
    #[test]
    fn abbruch_ohne_abschied_gibt_nach_der_frist_auf() {
        use std::net::TcpListener;
        secure::test_identitaet();
        let (host_priv, _) = test_host();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            // Danach ist niemand mehr da: jeder neue Versuch wird abgelehnt.
            drop(l);
            let Ok(mut h) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) else { return };
            let _ = h.write_all(MAGIC);
            std::thread::sleep(Duration::from_millis(300));
        });
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr),
            neuversuch_frist: Some(Duration::from_secs(3)),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        // Erst steht die Sitzung ...
        let t0 = Instant::now();
        while shared.lock().unwrap().sitzung_nr == 0 && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(shared.lock().unwrap().sitzung_nr, 1, "keine Sitzung");
        // ... dann geht sie verloren, und nach Frist plus einem Takt ist Schluss.
        let t1 = Instant::now();
        while shared.lock().unwrap().target.is_some() && t1.elapsed() < Duration::from_secs(12) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let dauer = t1.elapsed();
        let s = shared.lock().unwrap();
        assert!(s.target.is_none(), "nach {dauer:?} immer noch Neuversuche");
        assert!(dauer >= Duration::from_secs(3) && dauer < Duration::from_secs(9), "{dauer:?}");
        let m = s.error.as_ref().expect("keine Meldung");
        assert_eq!(m.key, strings::Key::ConnectionLost, "{}", m.protokoll);
        assert!(m.dauerhaft());
        assert_eq!(s.sitzung_nr, 1);
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

    /// Der Host verabschiedet sich (MSG_HOST_ENDE): je Grund die passende
    /// Meldung mit dem Namen des Hosts, das Ziel geht zurueck, und es gibt
    /// keine zweite Verbindung - auch nicht nach der Pause von 2 s. Ein
    /// unbekannter Grund und eine leere Nutzlast gelten wie "beendet".
    #[test]
    fn abschied_des_hosts_ohne_neuversuch() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        secure::test_identitaet();
        let (host_priv, _) = test_host();
        let faelle: [(&[u8], strings::Key); 5] = [
            (&[HOST_ENDE_BEENDET], strings::Key::MsgHostQuit),
            (&[HOST_ENDE_FREIGABE_AUS], strings::Key::MsgHostSharingOff),
            (&[HOST_ENDE_ENTFERNT, 0xee], strings::Key::MsgHostRemovedYou),
            (&[9], strings::Key::MsgHostQuit),
            (&[], strings::Key::MsgHostQuit),
        ];
        let mut laeufe = Vec::new();
        for (nutzlast, soll) in faelle {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap().to_string();
            let verbindungen = Arc::new(AtomicUsize::new(0));
            let (v, hp, n) = (verbindungen.clone(), host_priv.clone(), nutzlast.to_vec());
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(s) = s else { continue };
                    v.fetch_add(1, Ordering::SeqCst);
                    let Ok(mut h) = secure::Secure::accept(s, &noise::prologue_video(), &hp) else { continue };
                    // Gruss wie beim echten Host, danach gleich der Abschied.
                    let mut m = MAGIC.to_vec();
                    m.extend_from_slice(&[MSG_HOST_ENDE, 0, 0, 0, n.len() as u8, 0, 0, 0]);
                    m.extend_from_slice(&n);
                    let _ = h.write_all(&m);
                }
            });
            let shared = Arc::new(Mutex::new(Shared {
                decoder_wunsch: einstellungen::DecoderWunsch::Software,
                target: Some(addr),
                ziel_name: Some("Studio".into()),
                ..Shared::default()
            }));
            let input = Arc::new(Mutex::new(InputLink::new(String::new())));
            {
                let (s, i) = (shared.clone(), input.clone());
                std::thread::spawn(move || stream_thread(s, i));
            }
            laeufe.push((shared, verbindungen, soll));
        }
        let t0 = Instant::now();
        while laeufe.iter().any(|(s, _, _)| s.lock().unwrap().target.is_some()) && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        for (shared, _, soll) in &laeufe {
            let s = shared.lock().unwrap();
            assert!(s.target.is_none(), "Ziel nicht zurueckgenommen ({soll:?})");
            assert_eq!(s.error_key, None);
            let m = s.error.as_ref().expect("keine Meldung");
            assert_eq!(m.key, *soll, "{}", m.protokoll);
            assert!(m.dauerhaft());
            assert_eq!(m.text(strings::pick("en")), strings::EN.get(*soll).replace("{n}", "Studio"));
        }
        // Frueher: nach 2 s die naechste Verbindung. Drei Sekunden zusehen.
        std::thread::sleep(Duration::from_secs(3));
        for (shared, verbindungen, soll) in &laeufe {
            assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden ({soll:?})");
            assert!(!shared.lock().unwrap().connected);
        }
        assert_eq!(
            host_ende_meldung(HOST_ENDE_BEENDET, "Studio", "10.0.0.5:9001").text(strings::pick("de")),
            "Studio wurde beendet."
        );
        assert_eq!(
            host_ende_meldung(HOST_ENDE_ENTFERNT, "Studio", "10.0.0.5:9001").text(strings::pick("de")),
            "Studio hat dieses Gerät entfernt."
        );
    }

    /// Verbunden ueber eine Geraete-ID, aber unter der Adresse antwortet ein
    /// Host mit anderem Schluessel (Spezifikation Pairing v1, 8.2): der
    /// Client bricht im Handschlag vor Nachricht 3 ab - der Host kennt ihn
    /// danach nicht, hat ihn also auch nicht als Zuschauer angenommen und
    /// niemanden abgeloest -, und er versucht es nicht alle 2 s erneut: das
    /// Ziel geht zurueck, die Meldung bleibt.
    #[test]
    fn andere_id_ohne_nachricht_3_und_ohne_wiederholung() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        secure::test_identitaet();
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
            // Gewaehlt war der gemeinsame Test-Host - geantwortet hat ein anderer.
            ziel_id: Some(zugang::geraete_id(&test_host().1)),
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
            assert_eq!(s.error.as_ref().map(|m| m.key), Some(strings::Key::MsgOtherDevice));
            assert_eq!(
                s.error.as_ref().unwrap().text(&strings::EN),
                "A different device answers at this address."
            );
        }
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
        assert_eq!(angenommen.load(Ordering::SeqCst), 0, "Nachricht 3 kam beim Host an");
    }

    /// Dieselbe Meldung ohne ein Bild dazwischen steht nur einmal im
    /// Protokoll; nach einer echten Sitzung (Bilder) wieder.
    #[test]
    fn verbindungsfehler_nur_einmal_im_protokoll() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let e = Meldung::neu(strings::Key::ErrorHandshake, "Handschlag: Laenge: Leitung zu");
        let f = Meldung::neu(strings::Key::ErrorProtocol, "anderes Protokoll");
        let mut gemeldet = None;
        assert!(neu_zu_melden(&mut gemeldet, &e, 0));
        assert!(!neu_zu_melden(&mut gemeldet, &e, 0));
        assert!(neu_zu_melden(&mut gemeldet, &e, 7), "nach einer Sitzung mit Bildern");
        assert!(!neu_zu_melden(&mut gemeldet, &e, 7));
        assert!(neu_zu_melden(&mut gemeldet, &f, 7));
        assert!(neu_zu_melden(&mut gemeldet, &e, 7));
        assert!(!e.dauerhaft() && !f.dauerhaft());

        // Echt: eine Gegenstelle, die Nachricht 1 liest und dann zumacht -
        // der Handschlag scheitert, jedes Mal gleich. Der Client versucht es
        // weiter (kein Dauerfehler), protokolliert aber einmal.
        secure::test_identitaet();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let verbindungen = Arc::new(AtomicUsize::new(0));
        let v = verbindungen.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                v.fetch_add(1, Ordering::SeqCst);
                let mut n = [0u8; 2];
                if s.read_exact(&mut n).is_ok() {
                    let mut m = vec![0u8; u16::from_le_bytes(n) as usize];
                    let _ = s.read_exact(&mut m);
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
        while verbindungen.load(Ordering::SeqCst) < 3 && t0.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        let (ziel, protokoll_zeile) = {
            let mut s = shared.lock().unwrap();
            assert_eq!(s.error.as_ref().map(|m| m.key), Some(strings::Key::ErrorHandshake));
            (s.target.take(), format!("Verbindung: {}", s.error.as_ref().unwrap().protokoll))
        };
        assert!(ziel.is_some(), "Ziel zurueckgenommen - ein gescheiterter Handschlag ist kein Dauerfehler");
        assert!(verbindungen.load(Ordering::SeqCst) >= 3);
        let text = std::fs::read_to_string(einstellungen::datei_pfad("protokoll.txt").unwrap()).unwrap();
        let zeilen = text.lines().filter(|z| *z == protokoll_zeile).count();
        assert_eq!(zeilen, 1, "{protokoll_zeile}\n{text}");
    }

    /// Fehler von Leitung und Ablage erscheinen ueber Schluessel: in jeder
    /// Sprache der eigene Satz mit eingesetzten Werten; der deutsche Text mit
    /// Einzelheiten bleibt fuers Protokoll.
    #[test]
    fn meldungen_ueber_schluessel() {
        use secure::Fehler as F;
        use strings::Key::*;
        let pfad = std::path::PathBuf::from("/ablage/hosts.txt");
        let f = F::AnderesGeraet { addr: "10.0.0.5:9001".into(), erwartet: 581_729_911, gemeldet: 5 };
        let m = Meldung::from(f.clone());
        assert_eq!(m.key, MsgOtherDevice);
        assert_eq!(m.protokoll, f.to_string());
        assert!(m.protokoll.contains("ID 000 000 005 statt der gewaehlten 581 729 911"), "{}", m.protokoll);
        assert_eq!(m.text(&strings::EN), "A different device answers at this address.");
        assert_eq!(m.text(strings::pick("de")), "An dieser Adresse antwortet ein anderes Gerät.");
        // Der Wortlaut des Systems bleibt als Anhang; der Satz davor ist uebersetzt.
        let m = Meldung::from(F::Unlesbar { pfad: pfad.clone(), grund: "Zugriff verweigert (os error 5)".into() });
        assert_eq!(
            m.text(&strings::EN),
            "/ablage/hosts.txt cannot be read – not connecting. (Zugriff verweigert (os error 5))"
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
        // Dauerfehler: anderes Geraet unter der ID und Ablage; Leitung und
        // Handschlag nicht.
        let dauer = |f: F| Meldung::from(f).dauerhaft();
        assert!(dauer(F::AnderesGeraet { addr: "a".into(), erwartet: 1, gemeldet: 2 }));
        assert!(dauer(F::KeinUtf8 { pfad: pfad.clone() }));
        // Nicht lesbar ist oft nur eine kurze Sperre - und gelesen wird vor
        // dem Verbinden, ein neuer Versuch stoert also niemanden.
        assert!(!dauer(F::Unlesbar { pfad: pfad.clone(), grund: "g".into() }));
        assert!(dauer(F::Schreiben { pfad: pfad.clone(), grund: "g".into() }));
        assert!(dauer(F::SchluesselBeschaedigt { pfad: pfad.clone(), laenge: 3 }));
        assert!(dauer(F::Ablage("x".into())));
        assert!(!dauer(F::Verbindung { addr: "a".into(), art: E::ConnectionRefused, grund: "g".into() }));
        assert!(!dauer(F::Handschlag { grund: "g".into(), frist: true, system: None }));
        // Alle neuen Schluessel stehen englisch und deutsch da.
        for k in [
            SessionTakenOver, KeyFileDamaged, FileUnreadable, FileNotUtf8, FileNotWritable,
            StorageUnavailable, ErrorAddress, ErrorNoConnection, ErrorHandshake, ErrorFfmpegStart, ErrorSound,
            ErrorGpuDisplay, ErrorGpuLost, ErrorPixelFormat, DecoderFallback,
            FilesSending, FilesReceiving, FilesReady, FilesSent, FilesAborted, FilesTooLarge, FilesPeerOld,
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

    fn argumente(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// --verknuepfung und seine Werte sind nie die Verbindungsadresse - und
    /// die uebrige Auswertung bleibt, wie sie war.
    #[test]
    fn verknuepfung_nie_als_adresse() {
        assert_eq!(adresse_aus_argumenten(&argumente(&["--verknuepfung", "10.0.0.5"])), "");
        assert_eq!(
            adresse_aus_argumenten(&argumente(&["--verknuepfung", "10.0.0.5:9001", "--name", "Test Host.local", "--ordner", "C:\\qc\\desk"])),
            ""
        );
        assert_eq!(adresse_aus_argumenten(&argumente(&["--ordner", "d", "--name", "n", "--verknuepfung", "h"])), "");
        // Wie bisher: die erste freie Angabe, Port ergaenzt; Werte anderer
        // Schalter und --benchmark mit Zahl zaehlen nicht.
        assert_eq!(adresse_aus_argumenten(&argumente(&["10.0.0.5"])), "10.0.0.5:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--anzeige", "cpu", "h:9101"])), "h:9101");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--benchmark", "5", "--headless", "h"])), "h:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--benchmark", "h"])), "h:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--shot", "a.bmp", "de", "start"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&[])), "");
    }

    /// Der Port hinter --host bzw. --nur-host ist nie die Adresse: die eine
    /// App mit --host 9111 verbindet sich nicht mit "9111:9001". Ohne Zahl
    /// dahinter bleibt die naechste Angabe, was sie ist.
    #[test]
    fn host_port_nie_als_adresse() {
        assert_eq!(adresse_aus_argumenten(&argumente(&["--host", "9111"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--host", "9111", "--konserve", "p.hevc"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--nur-host", "9101", "--output", "1"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--host"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--host", "h"])), "h:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--hintergrund"])), "");
    }

    /// --bildschirm und sein Wert (Kennung oder "auto") sind nie die
    /// Verbindungsadresse (Spezifikation Bildschirmwahl 3.1, WERTIG).
    #[test]
    fn bildschirm_nie_als_adresse() {
        assert_eq!(adresse_aus_argumenten(&argumente(&["--bildschirm", "v0-m0-s0"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--bildschirm", "auto"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--headless", "--bildschirm", "v0-m0-s0", "h"])), "h:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["h:9101", "--bildschirm", "auto"])), "h:9101");
        assert!(WERTIG.iter().any(|(s, n)| *s == "--bildschirm" && *n == 1));
        // --shot: die Groesse hinter "@" der Ansicht.
        assert_eq!(shot_groesse("1920x1080"), Some((1920, 1080)));
        assert_eq!(shot_groesse("1280x720"), Some((1280, 720)));
        assert_eq!(shot_groesse("gross"), None);
        assert_eq!(shot_groesse("10x10"), None);
        assert_eq!(shot_groesse("1920x"), None);
    }

    /// Die Schalter der Host-Engine des Mac (die eine App reicht sie ihr
    /// weiter) sind nie die Adresse: --serve mit wahlfreiem Port, --out,
    /// --display und --capture mit ihren Werten.
    #[test]
    fn mac_host_schalter_nie_als_adresse() {
        assert_eq!(adresse_aus_argumenten(&argumente(&["--serve", "9101", "--fps", "120", "--fest"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--serve", "studio.local"])), "studio.local:9001");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--out", "1920x1080", "--display", "1"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--serve", "--capture", "10", "/tmp/x.hevc", "h"])), "h:9001");
    }

    /// Die Selbsttests des Symbols (--tray-selbsttest, --menueleiste-
    /// selbsttest) und des Ruhezustands haben keinen Wert: nie eine
    /// Adresse, und eine Angabe dahinter gehoert nicht zu ihnen.
    #[test]
    fn symbol_selbsttest_nie_als_adresse() {
        assert_eq!(adresse_aus_argumenten(&argumente(&["--tray-selbsttest"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--menueleiste-selbsttest"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--ruhe-selbsttest"])), "");
        assert_eq!(adresse_aus_argumenten(&argumente(&["--tray-selbsttest", "h"])), "h:9001");
        assert!(!WERTIG.iter().any(|(s, _)| s.contains("selbsttest")));
    }

    /// Ein anderes Ziel beendet die laufende Sitzung sofort - auch wenn der
    /// Host weiter sendet und das Trennen den Abbruchgriff verpasst hat
    /// (ein zweiter Start mit anderer Adresse trennt und verbindet in einem
    /// Zug). Frueher endete die Sitzung nur ohne Ziel.
    #[test]
    fn zielwechsel_beendet_die_sitzung() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        secure::test_identitaet();
        decoder_bereit();
        let (host_priv, _) = test_host();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let halt = Arc::new(AtomicBool::new(false));
        let h = halt.clone();
        std::thread::spawn(move || {
            let Ok((s, _)) = l.accept() else { return };
            let Ok(mut sock) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) else { return };
            if sock.write_all(MAGIC).is_err() {
                return;
            }
            // Weiter senden, was der Client uebergeht (Typ 0xEE ist frei),
            // damit seine Schleife laeuft.
            while !h.load(Ordering::SeqCst) {
                if sock.write_all(&[0xEE, 0, 0, 0, 0, 0, 0, 0]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr.clone()),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let (s, i, a) = (shared.clone(), input.clone(), addr.clone());
            std::thread::spawn(move || {
                let _ = tx.send(run_session(&a, &s, &i));
            });
        }
        let t0 = Instant::now();
        while !shared.lock().unwrap().connected && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(shared.lock().unwrap().connected, "keine Sitzung");
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "Sitzung endete zu frueh");
        // Anderes Ziel, der Abbruchgriff bleibt unberuehrt.
        shared.lock().unwrap().target = Some("127.0.0.1:1".into());
        let r = rx.recv_timeout(Duration::from_secs(3));
        halt.store(true, Ordering::SeqCst);
        assert!(matches!(r, Ok(Ok(()))), "Sitzung lief nach dem Zielwechsel weiter: {r:?}");
    }

    /// Hexbytes, wie sie in der Spezifikation stehen.
    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// Zwei winzige H.264-Vollbilder (x264 Baseline, ohne SEI): 64x64 und
    /// 96x64, je SPS, PPS und IDR - fuer einen Scheinhost, der Bilder schickt.
    const BILD_64X64: &str = "00 00 00 01 67 42 c0 0a dc 42 6c 04 40 00 00 03 00 40 00 00 0f 23 c4 89 e0 \
        00 00 00 01 68 ce 0f c8 00 00 00 01 65 88 84 3a 11 8a 00 02 18 f1 c0 00 40 f6 38 00 08 79 49 c9 c9 \
        d7 5d 75 d7 5d 75 d7 5d 75 e0";
    const BILD_96X64: &str = "00 00 00 01 67 42 c0 0a dc 62 6c 04 40 00 00 03 00 40 00 00 0f 23 c4 89 e0 \
        00 00 00 01 68 ce 0f c8 00 00 00 01 65 88 84 3a 11 8a 00 02 31 71 c0 00 43 ca 38 00 08 05 c9 c9 c9 \
        c9 c9 d7 5d 75 d7 5d 75 d7 5d 75 d7 5d 75 d7 5e";

    /// Die Liste aus den Pruefvektoren der Spezifikation (Bildschirmwahl
    /// 2.4): "X27 X1" (Haupt, gestreamt) und "Virtuell 16:9", Automatik.
    const BILDSCHIRME_2_4: &str = "01 02 00 00 \
        0e 06 80 07 38 04 78 00 03 00 76 31 31 33 38 2d 6d 31 32 33 34 2d 73 30 58 32 37 20 58 31 \
        08 0d 80 07 38 04 f0 00 00 00 76 30 2d 6d 30 2d 73 30 56 69 72 74 75 65 6c 6c 20 31 36 3a 39";

    /// Ein Scheinhost am Bildkanal: nimmt einen Client an, schickt MAGIC und
    /// danach alles, was der Test ueber den Sender reicht; faellt der
    /// Sender, geht die Leitung zu. Liefert Adresse und Sender.
    fn scheinhost_bild() -> (String, std::sync::mpsc::Sender<Vec<u8>>) {
        use std::net::TcpListener;
        let (host_priv, _) = test_host();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let Ok((s, _)) = l.accept() else { return };
            let Ok(mut sock) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) else { return };
            if sock.write_all(MAGIC).is_err() {
                return;
            }
            while let Ok(m) = rx.recv() {
                if sock.write_all(&m).is_err() {
                    return;
                }
            }
        });
        (addr, tx)
    }

    /// run_session gegen den Scheinhost, im eigenen Faden; Software-Decoder.
    fn sitzung_starten(addr: &str) -> (Arc<Mutex<Shared>>, Arc<Mutex<InputLink>>, std::sync::mpsc::Receiver<Result<(), Meldung>>) {
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr.to_string()),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let (s, i, a) = (shared.clone(), input.clone(), addr.to_string());
            std::thread::spawn(move || {
                let _ = tx.send(run_session(&a, &s, &i));
            });
        }
        (shared, input, rx)
    }

    /// Eine Strominfo mit dieser Groesse: 30 fps, H.264, 4:2:0 8 Bit.
    fn strominfo(w: u16, h: u16) -> Vec<u8> {
        let mut p = w.to_le_bytes().to_vec();
        p.extend_from_slice(&h.to_le_bytes());
        p.extend_from_slice(&30u16.to_le_bytes());
        p.extend_from_slice(&[2, 3, 1, 0]);
        eingabe_rahmen(MSG_INFO, &p)
    }

    /// Eine Bildnachricht mit Bildnummer und Vollbild-Flagge.
    fn bildnachricht(bild: &[u8], vollbild: bool, seq: u16) -> Vec<u8> {
        let mut m = vec![MSG_VIDEO, if vollbild { FLAG_KEY } else { 0 }];
        m.extend_from_slice(&seq.to_le_bytes());
        m.extend_from_slice(&(bild.len() as u32).to_le_bytes());
        m.extend_from_slice(bild);
        m
    }

    /// Eine Strominfo mit anderer Groesse mitten in der Sitzung
    /// (Spezifikation Bildschirmwahl 3.1): der Decoder wird neu gebaut, und
    /// bis zum naechsten Vollbild wird nichts decodiert - auch nicht ein
    /// Bild, das der alte Decoder gelesen haette. Gleiche Groesse noch einmal:
    /// kein neuer Bau. Gegenprobe: ohne die Regel zaehlt `decoder_baue` nach
    /// der zweiten Strominfo nicht hoch, und das Bild ohne Flagge wird
    /// decodiert (decodiert 4 statt 3).
    #[test]
    fn strominfo_mit_neuer_groesse_baut_den_decoder_neu() {
        secure::test_identitaet();
        decoder_bereit();
        if !decodieren_moeglich() {
            return;
        }
        let (addr, tx) = scheinhost_bild();
        let (shared, _input, ende) = sitzung_starten(&addr);
        let frist = Duration::from_secs(10);
        let decodiert = || shared.lock().unwrap().decoded;
        let baue = || shared.lock().unwrap().decoder_baue;
        let (b64, b96) = (hex(BILD_64X64), hex(BILD_96X64));
        // Strominfo 64x64 H.264: der Decoder (erst HEVC) wird fuer H.264 neu
        // gebaut - der zweite Bau der Sitzung.
        tx.send(strominfo(64, 64)).unwrap();
        assert!(warten_bis(frist, || baue() == 2), "kein H.264-Decoder: {} Baue", baue());
        // Ein Vollbild, dann dasselbe Bild ohne Flagge: beide werden decodiert.
        tx.send(bildnachricht(&b64, true, 1)).unwrap();
        tx.send(bildnachricht(&b64, false, 2)).unwrap();
        assert!(warten_bis(frist, || decodiert() == 2), "decodiert {}", decodiert());
        // Neue Groesse bei gleichem Codec: ein neuer Decoder ...
        tx.send(strominfo(96, 64)).unwrap();
        assert!(warten_bis(frist, || baue() == 3), "kein neuer Decoder nach der Groessenaenderung: {} Baue", baue());
        // ... und bis zum naechsten Vollbild nichts: das Bild ohne Flagge
        // faellt weg, das Vollbild danach kommt an. Die Lagenachricht
        // dahinter zeigt, dass alles davor verarbeitet ist.
        tx.send(bildnachricht(&b96, false, 3)).unwrap();
        tx.send(bildnachricht(&b96, true, 4)).unwrap();
        tx.send(eingabe_rahmen(MSG_HOSTSTATUS, &[1, 0])).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().error_key == Some(strings::Key::NoDisplay)));
        assert_eq!(decodiert(), 3, "ein Bild ohne Vollbild-Flagge nach der Groessenaenderung wurde decodiert");
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.info.map(|i| (i.width, i.height)), Some((96, 64)));
            match &s.frame {
                Some(Bild::Rgb(f)) => assert_eq!((f.width, f.height), (96, 64)),
                _ => panic!("kein Bild aus dem neuen Decoder"),
            }
        }
        // Dieselbe Groesse noch einmal: kein neuer Bau, nichts wartet.
        tx.send(strominfo(96, 64)).unwrap();
        tx.send(bildnachricht(&b96, false, 5)).unwrap();
        tx.send(eingabe_rahmen(MSG_HOSTSTATUS, &[0, 0])).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().error_key.is_none()));
        assert_eq!(baue(), 3);
        assert_eq!(decodiert(), 4);
        drop(tx);
        assert!(ende.recv_timeout(frist).is_ok(), "Sitzung endete nicht mit der Leitung");
    }

    /// Bildschirmwahl ueber die Sitzung (Spezifikation Bildschirmwahl 2.1,
    /// 2.2, 3.1): ohne Bit 1 der Faehigkeiten geht kein Wunsch hinaus; mit
    /// Bit 1 landet die Liste der Pruefvektoren in Shared; ein Wunsch setzt
    /// den Hinweis und legt Typ 70 zum Nachreichen bereit; die Antwort des
    /// Hosts beendet den Hinweis erst, wenn der gestreamte Eintrag zum
    /// Wunsch passt (oder der Wunsch nicht angeschlossen ist), eine fremde
    /// Antwort nicht; krumme Bytes aendern nichts; die Strominfo des vorigen
    /// Hosts ist zu Beginn weg; am Ende ist alles zurueckgesetzt.
    #[test]
    fn bildschirme_ueber_die_sitzung() {
        secure::test_identitaet();
        decoder_bereit();
        let (addr, tx) = scheinhost_bild();
        let shared_vor = Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr.clone()),
            // Reste eines vorigen Hosts: Strominfo, Liste, Faehigkeit.
            info: Some(StreamInfo { width: 1920, height: 1080, fps: 120, codec: 1, chroma444: true, ten_bit: true }),
            host_bildschirmwahl: true,
            bildschirm_wunsch: Some("alt".into()),
            ..Shared::default()
        };
        let shared = Arc::new(Mutex::new(shared_vor));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        let (ende_tx, ende) = std::sync::mpsc::channel();
        {
            let (s, i, a) = (shared.clone(), input.clone(), addr.clone());
            std::thread::spawn(move || {
                let _ = ende_tx.send(run_session(&a, &s, &i));
            });
        }
        let frist = Duration::from_secs(10);
        let liste = |wunsch: Option<&str>, gestreamt: usize| {
            let mut b = bildschirm::bildschirme_lesen(&hex(BILDSCHIRME_2_4)).unwrap();
            b.wunsch = wunsch.map(str::to_string);
            for (i, e) in b.eintraege.iter_mut().enumerate() {
                e.gestreamt = i == gestreamt;
            }
            eingabe_rahmen(MSG_BILDSCHIRME, &bildschirm::bildschirme_kodieren(&b))
        };
        let wunsch_gemerkt = |erwartet: Option<&str>| {
            let l = input.lock().unwrap();
            let w = l.nachreichen.iter().find(|b| b.first() == Some(&IN_BILDSCHIRM)).map(|b| b[8..].to_vec());
            assert_eq!(w, erwartet.map(|e| bildschirm::wunsch_kodieren(Some(e))));
        };
        let laeuft = || shared.lock().unwrap().bildschirm_wechsel_laeuft();
        let lage = |b: u8| eingabe_rahmen(MSG_HOSTSTATUS, &[b, 0]);
        let lage_abwarten = |b: u8| {
            let soll = (b == 1).then_some(strings::Key::NoDisplay);
            assert!(warten_bis(frist, || shared.lock().unwrap().error_key == soll), "Lage {b} kam nicht an");
        };

        // Sitzungsbeginn: die Reste des vorigen Hosts sind weg.
        assert!(warten_bis(frist, || shared.lock().unwrap().connected), "keine Sitzung");
        {
            let s = shared.lock().unwrap();
            assert!(s.info.is_none(), "Strominfo des vorigen Hosts steht noch");
            assert!(!s.host_bildschirmwahl && s.bildschirm_wunsch.is_none() && s.bildschirme.is_empty());
        }
        // Ohne Bit 1 kein Wunsch: nichts gemerkt, kein Hinweis.
        assert!(!bildschirm_wunsch_senden(&shared, &input, Some("v0-m0-s0".into())));
        assert!(!laeuft());
        wunsch_gemerkt(None);

        // Faehigkeiten 3: Dateien und Bildschirmwahl.
        tx.send(eingabe_rahmen(MSG_FAEHIGKEITEN, &[3, 0, 0, 0])).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().host_bildschirmwahl), "Bit 1 kam nicht an");
        assert!(shared.lock().unwrap().host_dateien);
        // Die Liste der Pruefvektoren, byte-genau wie der Mac-Host sie schickt.
        tx.send(eingabe_rahmen(MSG_BILDSCHIRME, &hex(BILDSCHIRME_2_4))).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().bildschirme.len() == 2), "Liste kam nicht an");
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.bildschirm_wunsch, None);
            let e = &s.bildschirme;
            assert_eq!((e[0].kennung.as_str(), e[0].name.as_str(), e[0].breite, e[0].hoehe, e[0].hz), ("v1138-m1234-s0", "X27 X1", 1920, 1080, 120));
            assert!(e[0].haupt && e[0].gestreamt);
            assert_eq!((e[1].kennung.as_str(), e[1].name.as_str(), e[1].hz), ("v0-m0-s0", "Virtuell 16:9", 240));
            assert!(!e[1].haupt && !e[1].gestreamt);
        }

        // Wunsch aus dem Menue: Hinweis steht, Typ 70 liegt zum Nachreichen
        // bereit (es gibt keinen Eingabekanal), mit den Bytes aus 2.4.
        assert!(bildschirm_wunsch_senden(&shared, &input, Some("v0-m0-s0".into())));
        assert!(laeuft());
        wunsch_gemerkt(Some("v0-m0-s0"));
        // Antwort mit dem Wunsch, aber noch dem alten Strom: der Hinweis bleibt.
        tx.send(liste(Some("v0-m0-s0"), 0)).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().bildschirm_wunsch.as_deref() == Some("v0-m0-s0")));
        assert!(laeuft(), "Hinweis endete, bevor der Strom auf dem Wunsch lief");
        // Der Strom laeuft auf dem Wunsch: Hinweis weg.
        tx.send(liste(Some("v0-m0-s0"), 1)).unwrap();
        assert!(warten_bis(frist, || !laeuft()), "Hinweis blieb, obwohl der Wunsch gestreamt wird");
        assert!(shared.lock().unwrap().bildschirme[1].gestreamt);

        // Wunsch auf einen Bildschirm, der nicht angeschlossen ist: der Host
        // antwortet mit dem Wunsch und dem unveraenderten Strom - damit ist
        // er beantwortet (Ausweichplatz), der Hinweis geht.
        assert!(bildschirm_wunsch_senden(&shared, &input, Some("v9-m9-s9".into())));
        assert!(laeuft());
        tx.send(liste(Some("v9-m9-s9"), 1)).unwrap();
        assert!(warten_bis(frist, || shared.lock().unwrap().bildschirm_wunsch.as_deref() == Some("v9-m9-s9")));
        assert!(!laeuft(), "Hinweis blieb nach der Antwort 'nicht angeschlossen'");

        // Wunsch Automatik unterwegs; eine Liste mit einem anderen Wunsch
        // (der Host hat noch nicht geantwortet) beendet den Hinweis nicht.
        assert!(bildschirm_wunsch_senden(&shared, &input, None));
        {
            let l = input.lock().unwrap();
            let w = l.nachreichen.iter().find(|b| b.first() == Some(&IN_BILDSCHIRM)).map(|b| b[8..].to_vec());
            assert_eq!(w, Some(vec![0]));
        }
        tx.send(liste(Some("v9-m9-s9"), 1)).unwrap();
        tx.send(lage(1)).unwrap();
        lage_abwarten(1);
        assert!(laeuft(), "eine fremde Antwort beendete den Hinweis");
        // Automatik beantwortet: gestreamt ist der Hauptbildschirm.
        tx.send(liste(None, 0)).unwrap();
        assert!(warten_bis(frist, || !laeuft()), "Hinweis blieb nach der Antwort auf Automatik");
        assert_eq!(shared.lock().unwrap().bildschirm_wunsch, None);

        // Krumme Bytes (fremde Fassung) aendern nichts an der Liste.
        let mut kaputt = hex(BILDSCHIRME_2_4);
        kaputt[0] = 2;
        tx.send(eingabe_rahmen(MSG_BILDSCHIRME, &kaputt)).unwrap();
        tx.send(lage(0)).unwrap();
        lage_abwarten(0);
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.bildschirme.len(), 2);
            assert!(s.bildschirme[0].gestreamt && s.bildschirm_wunsch.is_none());
        }

        // Sitzungsende: Liste, Wunsch, Faehigkeit und Hinweis sind zurueck.
        assert!(bildschirm_wunsch_senden(&shared, &input, Some("v0-m0-s0".into())));
        drop(tx);
        assert!(ende.recv_timeout(frist).is_ok(), "Sitzung endete nicht mit der Leitung");
        {
            let s = shared.lock().unwrap();
            assert!(!s.host_bildschirmwahl && s.bildschirme.is_empty() && s.bildschirm_wunsch.is_none());
            assert!(s.bildschirm_wechsel.is_none());
        }
        assert!(!bildschirm_wunsch_senden(&shared, &input, None));
    }

    /// Die Zeile zur Liste im Protokoll und die Knopftexte im Menue.
    #[test]
    fn bildschirm_zeile_und_knopftexte() {
        let mut b = bildschirm::bildschirme_lesen(&hex(BILDSCHIRME_2_4)).unwrap();
        assert_eq!(
            bildschirme_zeile(&b),
            "Bildschirme des Hosts (Wunsch Automatik): X27 X1 v1138-m1234-s0 1920x1080 120 Hz [Haupt] [gestreamt]; \
             Virtuell 16:9 v0-m0-s0 1920x1080 240 Hz"
        );
        assert_eq!(bildschirm_knopftext(&b.eintraege[0]), "X27 X1 · 1920×1080 · 120 Hz");
        // Ohne Namen steht die Kennung, ohne Hz kein Hz-Teil.
        b.wunsch = Some("v0-m0-s0".into());
        b.eintraege[1].name.clear();
        b.eintraege[1].hz = 0;
        assert_eq!(bildschirm_knopftext(&b.eintraege[1]), "v0-m0-s0 · 1920×1080");
        assert_eq!(bildschirm_anzeigename(&b.eintraege[1]), "v0-m0-s0");
        assert!(bildschirme_zeile(&b).starts_with("Bildschirme des Hosts (Wunsch v0-m0-s0): X27 X1 "));
        assert!(bildschirme_zeile(&b).ends_with("; v0-m0-s0 1920x1080 0 Hz"));
        assert_eq!(bildschirme_zeile(&bildschirm::Bildschirme::default()), "Bildschirme des Hosts (Wunsch Automatik): keine");
        // Beantwortet: gestreamt passt zum Wunsch, oder der Wunsch fehlt;
        // bei Automatik der Hauptbildschirm.
        assert!(!bildschirm_wunsch_beantwortet(&b));
        b.eintraege[0].gestreamt = false;
        b.eintraege[1].gestreamt = true;
        assert!(bildschirm_wunsch_beantwortet(&b));
        b.wunsch = Some("v9-m9-s9".into());
        assert!(bildschirm_wunsch_beantwortet(&b));
        b.wunsch = None;
        assert!(!bildschirm_wunsch_beantwortet(&b));
        b.eintraege[1].haupt = true;
        assert!(bildschirm_wunsch_beantwortet(&b));
    }

    /// Weitergereichter Start: ohne Adresse nur nach vorn; zum selben Host
    /// (Port ergaenzt, Schreibweise egal) bleibt die Sitzung; zu einem
    /// anderen trennen und neu; ohne Sitzung verbinden.
    #[test]
    fn einzel_folge_entscheidet() {
        use EinzelFolge::*;
        assert_eq!(einzel_folge(None, ""), NachVorn);
        assert_eq!(einzel_folge(Some("h:9001"), "  "), NachVorn);
        assert_eq!(einzel_folge(None, "10.0.0.5"), Verbinden("10.0.0.5:9001".into()));
        assert_eq!(einzel_folge(Some("10.0.0.5"), "10.0.0.5:9001"), Bleibt("10.0.0.5:9001".into()));
        assert_eq!(einzel_folge(Some("Studio.local:9001"), "studio.LOCAL"), Bleibt("studio.LOCAL:9001".into()));
        assert_eq!(einzel_folge(Some("10.0.0.5:9001"), "10.0.0.6:9001"), Wechseln("10.0.0.6:9001".into()));
        assert_eq!(einzel_folge(Some("10.0.0.5:9001"), "10.0.0.5:9101"), Wechseln("10.0.0.5:9101".into()));
    }

    /// Die Hostzeile wird um den Knopf schmaler (unter Windows): ein Klick
    /// auf den Knopf trifft die Zeile nicht, und beide liegen in der Tafel.
    #[test]
    fn hostzeile_und_knopf_ueberlappen_nicht() {
        let mut u = ui::Ui::new();
        for lang in strings::all() {
            for i in 0..4 {
                let (zeile, knopf) = start_zeile(&mut u, 900, 700, lang, i);
                let (px, panel_w, _) = start_rahmen(900, 700);
                assert!(zeile.x >= px && zeile.x + zeile.w <= px + panel_w);
                match knopf {
                    Some(k) => {
                        assert!(MIT_VERKNUEPFUNG);
                        assert!(zeile.x + zeile.w < k.x, "{}: Zeile reicht in den Knopf", lang.code);
                        assert!(k.x + k.w <= px + panel_w - 12);
                        assert!(!zeile.hit(k.x + k.w / 2, k.y + k.h / 2));
                        assert_eq!((k.y, k.h), (zeile.y, zeile.h));
                    }
                    None => assert!(!MIT_VERKNUEPFUNG),
                }
            }
        }
    }

    /// Ein HudStand ohne Sitzungsdaten fuer die Tests des ESC-Menues; die
    /// Codecs des Hosts nach Wunsch.
    fn hud_stand_leer(codecs: Vec<CodecEintrag>) -> HudStand {
        HudStand {
            vollbild: false, pixelgenau: false, statistik: false, nerd: false,
            wahl: einstellungen::StatWahl::default(),
            codecs, codec_idx: None, wechsel: false,
            bildschirmwahl: false, bildschirme: Vec::new(), bildschirm_wunsch: None, bildschirm_wechsel: false,
            decoder: einstellungen::DecoderWunsch::Automatik, decoder_aktiv: None,
            karten: Vec::new(),
            anzeige_aktiv: einstellungen::AnzeigeWunsch::Automatik,
            anzeige_gespeichert: einstellungen::AnzeigeWunsch::Automatik,
            anzeige_name: String::new(),
            bench_konfig: BenchKonfig::vorgabe(5, true), bench: None, bench_scroll: 0,
            verknuepfung: None,
        }
    }

    /// Trennen steht im Kopf des ESC-Menues, auf jedem Reiter, rechts neben
    /// den Reitern - kein Ueberlappen in jeder Sprache und Groesse, auch in
    /// schmalen und in hohen, schmalen Fenstern (dort weicht der Kopf aus):
    /// ein Klick auf Trennen trennt, ein Klick auf den rechten Rand eines
    /// Reiters waehlt den Reiter. Ab 1280 x 720 (16:9) bleibt der Kopf, wie
    /// er war: Schriftzug, ganze Aufschriften. Im Reiter Verschluesselung, wo
    /// der Knopf frueher stand, trennt ein Klick nicht mehr.
    #[test]
    fn trennen_im_kopf_auf_jedem_reiter() {
        let mut u = ui::Ui::new();
        let stand = hud_stand_leer(Vec::new());
        let groessen = [
            (1280, 720), (1920, 1080), (2560, 1440), (3840, 2160),
            (1024, 576), (800, 600), (640, 480), (900, 1100), (1400, 1900),
        ];
        let mut ausgewichen = 0;
        for (ww, wh) in groessen {
            let mut buf = vec![0u32; (ww * wh) as usize];
            for lang in strings::all() {
                let k = hud_kopf(&mut u, lang, ww, wh);
                let wo = format!("{} {ww}x{wh}", lang.code);
                for i in 0..4 {
                    assert!(k.reiter[i].x + k.reiter[i].w < k.reiter[i + 1].x, "{wo}: Reiter {i} reicht in den naechsten");
                }
                let letzter = k.reiter[4];
                assert!(letzter.x + letzter.w < k.trennen.x, "{wo}: Reiter reichen in Trennen");
                assert!(k.reiter[0].x > 0 && k.trennen.x + k.trennen.w < ww, "{wo}: Kopf ragt aus dem Fenster");
                for (n, voll) in k.namen.iter().zip(hud_reiternamen(lang)) {
                    assert!(n == voll || n.ends_with('…'), "{wo}: Aufschrift {n:?} statt {voll:?}");
                }
                if ww >= 1280 && ww * 9 == wh * 16 {
                    assert!(k.logo, "{wo}: Schriftzug fehlt");
                    assert!(k.namen.iter().zip(hud_reiternamen(lang)).all(|(n, v)| n == v), "{wo}: gekuerzt");
                }
                if !k.logo {
                    ausgewichen += 1;
                }
                let klick = |u: &mut ui::Ui, c: &mut ui::Canvas, reiter: u8, x: i32, y: i32| -> HudAktion {
                    u.mouse = (x, y);
                    u.click = true;
                    hud(u, c, lang, ww, wh, reiter, None, &[], 0.0, &[], None, None, (None, None),
                        "192.168.178.194:9001", false, &stand)
                };
                for reiter in 0..5u8 {
                    let mut c = ui::Canvas::neu(&mut buf, ww as usize, wh as usize);
                    let a = klick(&mut u, &mut c, reiter, k.trennen.x + k.trennen.w / 2, k.trennen.y + k.trennen.h / 2);
                    assert!(matches!(a, HudAktion::Trennen), "{wo} Reiter {reiter}");
                }
                // Der rechte Rand jedes Reiters, waehrend Benchmark offen ist.
                for (i, r) in k.reiter.iter().enumerate() {
                    let mut c = ui::Canvas::neu(&mut buf, ww as usize, wh as usize);
                    let a = klick(&mut u, &mut c, 4, r.x + r.w - 1, r.y + r.h / 2);
                    assert!(matches!(a, HudAktion::Reiter(n) if n as usize == i), "{wo}: Rand von Reiter {i}");
                }
            }
        }
        // Die Ausweichstufen kommen in diesen Groessen auch wirklich dran
        // (ohne geladene Schrift ist jeder Text null breit).
        assert!(ausgewichen > 0 || !u.text.ok());
        // Wo der Knopf frueher stand (Reiter Verschluesselung, unter dem
        // Vergleichscode): kein Trennen mehr.
        let (ww, wh) = (1280, 720);
        let mut buf = vec![0u32; (ww * wh) as usize];
        let mut c = ui::Canvas::neu(&mut buf, ww as usize, wh as usize);
        u.mouse = (110 + 110, 75 + 186 + 130 + 19);
        u.click = true;
        let a = hud(&mut u, &mut c, strings::pick("de"), ww, wh, 2, None, &[], 0.0, &[], None, None, (None, None), "h:9001", false, &stand);
        assert!(!matches!(a, HudAktion::Trennen));
    }

    /// Reiter Benchmark: die Adresse des Hosts unter Trennen und die Reihe der
    /// Codec-Knoepfe kommen sich nicht in die Quere - das Band der Adresse
    /// sieht mit und ohne Codecs gleich aus, auch wenn die Knoepfe bis an
    /// den rechten Rand reichen und die Adresse lang ist.
    #[test]
    fn benchmark_codecs_unter_der_adresse() {
        let mut u = ui::Ui::new();
        let codecs: Vec<CodecEintrag> = (0..12u8)
            .map(|i| CodecEintrag {
                idx: i, available: true, hardware: true, conversion: false, chroma444: true, ten_bit: true,
                name: format!("HEVC 4:4:4 {i}"),
            })
            .collect();
        let adresse = "arbeitszimmer-rechner-gpu.fritz.box:9001";
        for (ww, wh) in [(1024, 576), (1280, 720), (1920, 1080)] {
            let s: f32 = if wh >= 1000 { 1.5 } else { 1.0 };
            let p = |v: i32| (v as f32 * s).round() as i32;
            let y0 = (wh - (wh - p(80)).min(p(570))) / 2;
            let band = |u: &mut ui::Ui, adresse: &str, stand: &HudStand| -> Vec<u32> {
                let mut buf = vec![0u32; (ww * wh) as usize];
                let mut c = ui::Canvas::neu(&mut buf, ww as usize, wh as usize);
                u.mouse = (0, 0);
                u.click = false;
                hud(u, &mut c, strings::pick("de"), ww, wh, 4, None, &[], 0.0, &[], None, None, (None, None),
                    adresse, false, stand);
                drop(c);
                buf[((y0 + p(47)) * ww) as usize..((y0 + p(68)) * ww) as usize].to_vec()
            };
            let ohne = band(&mut u, adresse, &hud_stand_leer(Vec::new()));
            let mit = band(&mut u, adresse, &hud_stand_leer(codecs.clone()));
            assert!(ohne == mit, "{ww}x{wh}: Codec-Knoepfe im Band der Adresse");
            // Die Adresse steht wirklich in diesem Band.
            let leer = band(&mut u, "", &hud_stand_leer(Vec::new()));
            assert!(leer != ohne, "{ww}x{wh}: Adresse nicht im Band");
        }
    }

    // ------------------------------------------ Dateien (Spezifikation 3.4)

    /// Bis die Bedingung gilt, hoechstens `frist`.
    fn warten_bis(frist: Duration, f: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < frist {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        f()
    }

    /// Liest eine Nachricht (Kopf und Nutzlast) von einer Leitung.
    fn nachricht_lesen(h: &mut secure::Secure) -> Option<(u8, Vec<u8>)> {
        let mut kopf = [0u8; 8];
        h.read_exact(&mut kopf).ok()?;
        let mut p = vec![0u8; u32::from_le_bytes(kopf[4..8].try_into().unwrap()) as usize];
        h.read_exact(&mut p).ok()?;
        Some((kopf[0], p))
    }

    /// Eigener leerer Ordner je Test unter temp_dir, mit der Kennung des Laufs.
    fn test_ordner(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("{}-{name}", secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Nachrang im Schreiber (Spezifikation 3.4): Datei-Pakete stauen sich,
    /// solange der Host nicht liest, bis der eine Platz belegt bleibt (Voll).
    /// Eine Taste und eine Quittung, die DANACH eingereiht werden, gehen vor
    /// dem wartenden Datei-Paket hinaus - vor ihnen liegt nur das eine Paket,
    /// das der Schreibfaden gerade schrieb. Ohne stehenden Kanal gilt Weg.
    #[test]
    fn datei_pakete_haben_nachrang() {
        secure::test_identitaet();
        let hh = vec![0x76; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let mut ohne = InputLink::new(String::new());
        assert_eq!(ohne.datei_senden(DATEI_STUECK, &[0; 20]), dateien::Gesendet::Weg);

        // Der Host liest erst auf das Zeichen, dann bis zur Taste und bis
        // alle `erwartet` Datei-Pakete da sind.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (los, los_rx) = std::sync::mpsc::channel::<usize>();
        let (hh2, k) = (hh.clone(), host_priv);
        let host = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut h = secure::Secure::accept(s, &noise::prologue_input(&hh2), &k).unwrap();
            let erwartet = los_rx.recv().unwrap();
            let (mut gelesen, mut taste, mut pakete) = (Vec::new(), false, 0usize);
            while !(taste && pakete >= erwartet) {
                let Some((a, p)) = nachricht_lesen(&mut h) else { break };
                taste |= a == IN_KEY;
                pakete += (a == DATEI_STUECK) as usize;
                gelesen.push((a, p));
            }
            gelesen
        });
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh, host_pub)));
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        let paket = |i: u32| {
            let mut p = vec![0u8; dateien::STUECK_KOPF + dateien::STUECK_MAX];
            p[..4].copy_from_slice(&i.to_le_bytes());
            p
        };
        let (mut n, mut voll_seit, t0) = (0u32, None::<Instant>, Instant::now());
        loop {
            match l.datei_senden(DATEI_STUECK, &paket(n)) {
                dateien::Gesendet::Ja => {
                    n += 1;
                    voll_seit = None;
                }
                dateien::Gesendet::Voll => {
                    if voll_seit.get_or_insert_with(Instant::now).elapsed() >= Duration::from_millis(300) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                dateien::Gesendet::Weg => panic!("Kanal weg nach {n} Paketen"),
            }
            assert!(t0.elapsed() < Duration::from_secs(30) && n < 20_000, "kein Stau nach {n} Paketen");
        }
        // Der Platz haelt Paket n-1, der Schreibfaden haengt in n-2.
        assert!(n >= 2);
        assert_eq!(l.datei_senden(DATEI_STUECK, &paket(n)), dateien::Gesendet::Voll);
        l.key(9, true, 0);
        l.send(DATEI_QUITTUNG, &dateien::Quittung { kennung: 5, zustand: 0, empfangen: 7 }.kodieren());
        los.send(n as usize).unwrap();
        let gelesen = host.join().unwrap();
        assert_eq!(gelesen[0], (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]));
        let nummer = |p: &Vec<u8>| u32::from_le_bytes(p[..4].try_into().unwrap());
        let alle: Vec<u32> = gelesen.iter().filter(|(a, _)| *a == DATEI_STUECK).map(|(_, p)| nummer(p)).collect();
        assert_eq!(alle, (0..n).collect::<Vec<_>>(), "Datei-Pakete fehlen oder ausser der Reihe");
        let arten: Vec<u8> = gelesen.iter().map(|(a, _)| *a).collect();
        let taste = arten.iter().position(|a| *a == IN_KEY).expect("Taste fehlt");
        assert_eq!(arten.get(taste + 1), Some(&DATEI_QUITTUNG), "Quittung nicht gleich hinter der Taste");
        let danach: Vec<u32> =
            gelesen[taste..].iter().filter(|(a, _)| *a == DATEI_STUECK).map(|(_, p)| nummer(p)).collect();
        assert_eq!(danach, vec![n - 1], "hinter der Taste noch {} Datei-Pakete", danach.len());
    }

    /// IN_FAEHIGKEITEN geht als erste Nachricht auf JEDEM neu stehenden
    /// Kanal hinaus, vor allem Nachgereichten - auch auf dem, der nach einem
    /// Abriss neu steht. Ohne Kanal gesendet, wird sie gemerkt (NACHREICHEN),
    /// geht aber nie doppelt.
    #[test]
    fn faehigkeiten_zuerst_auf_jedem_kanal() {
        use std::net::TcpListener;
        secure::test_identitaet();
        let hh = vec![0x77; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let lauscher = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = lauscher.local_addr().unwrap().to_string();
        let (hh2, k) = (hh.clone(), host_priv);
        let host = std::thread::spawn(move || {
            // Erst nach 300 ms annehmen: so lange steht der Kanal sicher nicht.
            std::thread::sleep(Duration::from_millis(300));
            let mut runden = Vec::new();
            for runde in 0..2 {
                let (s, _) = lauscher.accept().unwrap();
                let mut h = secure::Secure::accept(s, &noise::prologue_input(&hh2), &k).unwrap();
                // Runde 1 bis zu den Einstellungen, Runde 2 nur die erste
                // Nachricht; danach ist die Leitung zu.
                let mut gelesen = Vec::new();
                while let Some((a, p)) = nachricht_lesen(&mut h) {
                    gelesen.push((a, p));
                    if runde == 1 || a == IN_SETTINGS {
                        break;
                    }
                }
                runden.push(gelesen);
            }
            runden
        });
        let mut l = InputLink::new(addr);
        l.set_link(Some((hh, host_pub)));
        l.send(IN_FAEHIGKEITEN, &dateien::faehigkeiten_kodieren(FAEHIG_DATEIEN));
        l.settings(10, 60, false, false, true);
        assert!(!l.steht());
        assert_eq!(l.nachreichen.len(), 2);
        eingabe_abwarten(&mut l);
        assert!(l.steht());
        assert_eq!(l.kanal_nr(), Some(1));
        // Der Host schliesst nach den Einstellungen: ein Schreibversuch
        // scheitert, und ein neuer Kanal steht.
        let t0 = Instant::now();
        while l.kanal_nr < 2 && t0.elapsed() < Duration::from_secs(10) {
            l.mouse_move(0.5, 0.5);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(l.kanal_nr, 2);
        let runden = host.join().unwrap();
        let faehig = (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]);
        assert_eq!(runden[0], vec![faehig.clone(), (IN_SETTINGS, vec![10, 0, 0, 0, 60, 0, 0, 0, 1])]);
        assert_eq!(runden[1], vec![faehig]);
    }

    /// Der Socket des Eingabekanals hat einen Sendepuffer von 64 KiB: so
    /// liegen im Kernel hoechstens 64 KiB Dateidaten vor einer Taste. Der
    /// Socket steht vorher auf 8 KiB, damit der Test auch dort etwas prueft,
    /// wo 64 KiB schon die Vorgabe des Systems ist (Windows; Phase-B-Hinweis
    /// f-client): erst Schreiber::neu setzt die 64 KiB.
    #[test]
    fn eingabekanal_sendepuffer_64_kib() {
        secure::test_identitaet();
        let hh = vec![0x79; 32];
        let (host_priv, _) = noise::keypair().unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (los, los_rx) = std::sync::mpsc::channel::<()>();
        let hh2 = hh.clone();
        let host = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let h = secure::Secure::accept(s, &noise::prologue_input(&hh2), &host_priv).unwrap();
            let _ = los_rx.recv();
            drop(h);
        });
        let sock = secure::Secure::connect_pruefend(&addr, &noise::prologue_input(&hh), |_| Ok(0)).expect("Eingabekanal");
        sendepuffer_setzen(sock.socket(), 8 * 1024);
        assert_eq!(sendepuffer_lesen(sock.socket()), Some(8 * 1024), "8 KiB nicht gesetzt");
        let k = Schreiber::neu(sock, SCHREIBFRIST);
        let griff = k.griff.as_ref().expect("kein Griff");
        assert_eq!(sendepuffer_lesen(griff), Some(EINGABE_SNDBUF));
        drop(k);
        let _ = los.send(());
        let _ = host.join();
    }

    /// Die Wege der Dateiuebertragung sind an die Sitzung gebunden, der des
    /// Senders auch an den Eingabekanal seines ersten Pakets: nach einem
    /// neuen Kanal oder in einer anderen Sitzung gilt Weg (der Rest einer
    /// Uebertragung kaeme sonst ohne Angebot an) - nur das Ende eines
    /// Abbruchs darf noch auf den neuen Kanal, und ohne stehenden Kanal
    /// wartet es (Voll). Quittungen warten (Voll), solange der Kanal dieser
    /// Sitzung nicht steht, und gehen dann ueber das normale send.
    #[test]
    fn datei_wege_binden_an_sitzung_und_kanal() {
        secure::test_identitaet();
        let hh = vec![0x7a; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, DATEI_ENDE);
        let input = Arc::new(Mutex::new(InputLink::new(addr)));
        let quittungen = quittungs_weg(&input, hh.clone());
        let q = dateien::Quittung { kennung: 3, zustand: 0, empfangen: 0 }.kodieren();
        assert_eq!(quittungen.senden(DATEI_QUITTUNG, &q), dateien::Gesendet::Voll, "ohne Bindung");
        {
            let mut l = input.lock().unwrap();
            l.set_link(Some((hh.clone(), host_pub.clone())));
            eingabe_abwarten(&mut l);
            assert!(l.steht());
        }
        assert_eq!(quittungen.senden(DATEI_QUITTUNG, &q), dateien::Gesendet::Ja);
        assert_eq!(quittungs_weg(&input, vec![1; 32]).senden(DATEI_QUITTUNG, &q), dateien::Gesendet::Voll, "fremde Sitzung");
        let fremd = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let fremder_weg = datei_weg(&input, vec![1; 32], fremd);
        assert_eq!(fremder_weg.senden(DATEI_ANGEBOT, &[1]), dateien::Gesendet::Weg, "fremde Sitzung");

        let kanal = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let weg = datei_weg(&input, hh.clone(), kanal.clone());
        assert_eq!(weg.senden(DATEI_ANGEBOT, &[1]), dateien::Gesendet::Ja);
        assert_eq!(kanal.load(std::sync::atomic::Ordering::SeqCst), 1, "nicht an Kanal 1 gebunden");
        assert!(warten_bis(Duration::from_secs(3), || weg.senden(DATEI_STUECK, &[2]) == dateien::Gesendet::Ja));
        let ende = |grund: u8| dateien::Ende { kennung: 9, grund }.kodieren();
        // Ein neuer Kanal (hier nur seine Nummer): Angebot, Stueck und ein
        // vollstaendiges Ende sind Weg, das Ende eines Abbruchs geht hinaus.
        input.lock().unwrap().kanal_nr += 1;
        assert_eq!(weg.senden(DATEI_STUECK, &[3]), dateien::Gesendet::Weg);
        assert_eq!(weg.senden(DATEI_ANGEBOT, &[5]), dateien::Gesendet::Weg);
        assert_eq!(weg.senden(DATEI_ENDE, &ende(dateien::GRUND_VOLLSTAENDIG)), dateien::Gesendet::Weg);
        assert!(warten_bis(Duration::from_secs(3), || {
            weg.senden(DATEI_ENDE, &ende(dateien::GRUND_ABGEBROCHEN)) == dateien::Gesendet::Ja
        }));
        assert_eq!(kanal.load(std::sync::atomic::Ordering::SeqCst), 1, "Bindung gewechselt");
        let _ = los.send(());
        let (gelesen, _) = host.join().unwrap();
        assert_eq!(
            gelesen,
            vec![
                (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]),
                (DATEI_QUITTUNG, q),
                (DATEI_ANGEBOT, vec![1]),
                (DATEI_STUECK, vec![2]),
                (DATEI_ENDE, ende(dateien::GRUND_ABGEBROCHEN)),
            ]
        );
        // Ohne stehenden Kanal: das Ende eines Abbruchs wartet, alles andere ist Weg.
        input.lock().unwrap().trennen();
        assert_eq!(weg.senden(DATEI_ENDE, &ende(dateien::GRUND_ABGEBROCHEN)), dateien::Gesendet::Voll);
        assert_eq!(weg.senden(DATEI_STUECK, &[6]), dateien::Gesendet::Weg);
        assert_eq!(weg.senden(DATEI_ENDE, &ende(dateien::GRUND_VOLLSTAENDIG)), dateien::Gesendet::Weg);
    }

    /// Verlust des Eingabekanals, die Regel allein: datei_kanal_pruefen laesst
    /// eine Sendung stehen, solange ihr Kanal steht oder sie noch an keinen
    /// gebunden ist, und bricht sie ab, sobald ein anderer Kanal steht oder
    /// keiner. Ist die Sperre des Eingabekanals belegt, entscheidet sie nichts.
    #[test]
    fn kanalverlust_nach_kanalnummer() {
        use std::sync::atomic::AtomicU64;
        secure::test_identitaet();
        let hh = vec![0x7d; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, IN_KEY);
        let input = Mutex::new(InputLink::new(addr));
        {
            let mut l = input.lock().unwrap();
            l.set_link(Some((hh, host_pub)));
            eingabe_abwarten(&mut l);
            assert_eq!(l.kanal_nr(), Some(1));
        }
        // Ein Griff, dessen Faden laeuft (sein Angebot bekommt immer Voll) -
        // oder, mit `laeuft` false, gleich endet (nichts zu senden): die
        // Regel sieht nur Griff und Bindung.
        let quelle = test_ordner("kanalverlust-nummer");
        let datei = quelle.join("a.txt");
        std::fs::write(&datei, b"hallo").unwrap();
        let sendung_mit = |k: u64, laeuft: bool| {
            let voll = Arc::new(|_: u8, _: &[u8]| dateien::Gesendet::Voll);
            let pfade = if laeuft { vec![datei.clone()] } else { Vec::new() };
            let griff = dateien::Sender::starten(pfade, voll, dateien::FENSTER, |_| {});
            if !laeuft {
                assert!(griff.abwarten(Duration::from_secs(5)), "Sender ohne Pfade endet nicht");
            }
            DateiSendung { griff, kanal: Arc::new(AtomicU64::new(k)) }
        };
        let sendung = |k: u64| sendung_mit(k, true);
        let shared = Mutex::new(Shared::default());
        let kennung = |s: &Mutex<Shared>| s.lock().unwrap().datei_senden.as_ref().map(|d| d.kennung());

        // Gebunden an den stehenden Kanal, und noch ungebunden: bleibt.
        for k in [1, 0] {
            let d = sendung(k);
            let nr = d.kennung();
            shared.lock().unwrap().datei_senden = Some(d);
            datei_kanal_pruefen(&shared, &input);
            assert_eq!(kennung(&shared), Some(nr), "Bindung {k}");
        }
        // Ein neuer Kanal steht (hier nur seine Nummer): ab.
        shared.lock().unwrap().datei_senden = Some(sendung(1));
        input.lock().unwrap().kanal_nr = 2;
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), None, "neuer Kanal");
        // Eine Sendung, deren Faden schon zu Ende ist, bleibt unberuehrt -
        // sie ist nicht mehr abzubrechen (Phase-B-Hinweis f-client).
        let fertig = sendung_mit(1, false);
        let nr = fertig.kennung();
        shared.lock().unwrap().datei_senden = Some(fertig);
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), Some(nr), "beendete Sendung als abgebrochen entnommen");
        // Schon an den neuen gebunden: bleibt.
        let d = sendung(2);
        let nr = d.kennung();
        shared.lock().unwrap().datei_senden = Some(d);
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), Some(nr));
        // Kein Kanal steht: ab; eine ungebundene bleibt.
        input.lock().unwrap().trennen();
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), None, "ohne Kanal");
        let d = sendung(0);
        let nr = d.kennung();
        shared.lock().unwrap().datei_senden = Some(d);
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), Some(nr));
        // Ist input belegt, wird nichts entschieden (try_lock).
        shared.lock().unwrap().datei_senden = Some(sendung(1));
        {
            let _belegt = input.lock().unwrap();
            datei_kanal_pruefen(&shared, &input);
            assert!(kennung(&shared).is_some(), "unter belegter Sperre entschieden");
        }
        datei_kanal_pruefen(&shared, &input);
        assert_eq!(kennung(&shared), None);
        let _ = los.send(());
        let _ = host.join();
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// Sender und Empfaenger melden ihren Stand nur, solange ihre Sitzung
    /// gilt: ein spaetes Ergebnis der vorigen Sitzung gehoert nicht in die
    /// Anzeige der neuen.
    #[test]
    fn datei_melder_nur_fuer_die_eigene_sitzung() {
        use dateien::{Ereignis, Ergebnis, Richtung, Stand};
        let shared = Arc::new(Mutex::new(Shared { sitzung_nr: 4, ..Shared::default() }));
        let alt = datei_melder(&shared, 4);
        let stand = |kennung: u32| Ereignis {
            stand: Some(Stand {
                richtung: Richtung::Empfangen,
                kennung,
                bytes: 5,
                gesamt: 5,
                eintraege: 1,
                dateien: 1,
                oberste: 1,
                ergebnis: Ergebnis::Fertig,
            }),
            zeile: None,
        };
        alt(stand(11));
        assert_eq!(shared.lock().unwrap().datei_stand.empfangen.as_ref().map(|d| d.stand.kennung), Some(11));
        // Neue Sitzung: die Anzeige beginnt leer, der alte Melder schweigt.
        {
            let mut s = shared.lock().unwrap();
            s.sitzung_nr = 5;
            s.datei_stand = DateiStaende::default();
        }
        alt(stand(12));
        assert!(shared.lock().unwrap().datei_stand.empfangen.is_none(), "Stand der vorigen Sitzung angezeigt");
        datei_melder(&shared, 5)(stand(13));
        assert_eq!(shared.lock().unwrap().datei_stand.empfangen.as_ref().map(|d| d.stand.kennung), Some(13));
    }

    /// Kopierte Dateien, waehrend der Host in dieser Sitzung keine
    /// Faehigkeit "Dateien" gemeldet hat: nichts geht hinaus (ein aelterer
    /// Host trennte sonst den Eingabekanal), einmal je Sitzung FilesPeerOld.
    /// Mit der Faehigkeit geht das Angebot hinaus.
    #[test]
    fn ohne_host_dateien_wird_nichts_gesendet() {
        secure::test_identitaet();
        let hh = vec![0x78; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, DATEI_ANGEBOT);
        let _ = los.send(());
        let input = Arc::new(Mutex::new(InputLink::new(addr)));
        let shared = Arc::new(Mutex::new(Shared { link: Some((hh.clone(), host_pub.clone())), ..Shared::default() }));
        {
            let mut l = input.lock().unwrap();
            l.set_link(Some((hh, host_pub)));
            eingabe_abwarten(&mut l);
            assert!(l.steht());
        }
        let quelle = test_ordner("ohne-faehigkeit");
        let datei = quelle.join("a.txt");
        std::fs::write(&datei, b"hallo").unwrap();

        dateien_senden(vec![datei.clone()], &shared, &input);
        let erst = {
            let s = shared.lock().unwrap();
            assert!(s.datei_senden.is_none(), "Sender gestartet");
            assert!(s.host_zu_alt_gemeldet);
            let a = s.datei_stand.senden.clone().expect("keine Meldung");
            assert_eq!(a.stand.ergebnis, dateien::Ergebnis::GegenseiteZuAlt);
            a.ergebnis_seit
        };
        std::thread::sleep(Duration::from_millis(20));
        dateien_senden(vec![datei.clone()], &shared, &input);
        assert_eq!(shared.lock().unwrap().datei_stand.senden.as_ref().unwrap().ergebnis_seit, erst, "zweimal gemeldet");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(input.lock().unwrap().sent, 1, "ausser den Faehigkeiten ging etwas hinaus");

        shared.lock().unwrap().host_dateien = true;
        dateien_senden(vec![datei], &shared, &input);
        let (gelesen, _) = host.join().unwrap();
        let arten: Vec<u8> = gelesen.iter().map(|(a, _)| *a).collect();
        assert_eq!(arten, vec![IN_FAEHIGKEITEN, DATEI_ANGEBOT]);
        let a = dateien::Angebot::lesen(&gelesen[1].1).unwrap();
        assert_eq!((a.eintraege.len(), a.gesamt), (1, 5));
        assert!(shared.lock().unwrap().datei_senden.is_some());
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// Neuer Inhalt bricht eine laufende Sendung ab (Spezifikation 2.7
    /// Schritt 5): neue Dateien starten eine neue (die alte schickt Ende 1),
    /// neuer Text beendet sie und geht als IN_CLIP hinaus. Die Anzeige bleibt
    /// bei der neuesten Kennung. Dateien aus der eigenen Ablage (Widerhall)
    /// brechen nichts ab.
    #[test]
    fn neuer_inhalt_bricht_die_sendung_ab() {
        secure::test_identitaet();
        let hh = vec![0x7b; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, IN_CLIP);
        let _ = los.send(());
        let input = Arc::new(Mutex::new(InputLink::new(addr)));
        let quelle = test_ordner("neuer-inhalt");
        let basis = quelle.join("ablage");
        let shared = Arc::new(Mutex::new(Shared {
            link: Some((hh.clone(), host_pub.clone())),
            host_dateien: true,
            datei_ablage: Some(DateiAblage { basis: basis.clone(), ablegen: Arc::new(|_| true) }),
            ..Shared::default()
        }));
        {
            let mut l = input.lock().unwrap();
            l.set_link(Some((hh, host_pub)));
            eingabe_abwarten(&mut l);
            assert!(l.steht());
        }
        // 1 MB: ohne Quittung bleibt die Sendung nach dem ersten Fenster stehen.
        let a = quelle.join("a.bin");
        std::fs::write(&a, vec![7u8; 1_000_000]).unwrap();
        let kennung = || shared.lock().unwrap().datei_senden.as_ref().map(|g| g.kennung());
        dateien_senden(vec![a.clone()], &shared, &input);
        let k1 = kennung().expect("keine Sendung");
        assert!(warten_bis(Duration::from_secs(5), || {
            shared.lock().unwrap().datei_stand.senden.as_ref().is_some_and(|d| d.stand.kennung == k1)
        }));
        dateien_senden(vec![a.clone()], &shared, &input);
        let k2 = kennung().expect("keine neue Sendung");
        assert!(k2 > k1);
        // Widerhall: eine Liste aus der eigenen Ablage laesst die Sendung stehen.
        let eigen = basis.join("1-1").join("x.txt");
        std::fs::create_dir_all(eigen.parent().unwrap()).unwrap();
        std::fs::write(&eigen, b"x").unwrap();
        dateien_senden(vec![eigen], &shared, &input);
        assert_eq!(kennung(), Some(k2));
        // Das spaete "abgebrochen" von k1 verdraengt k2 nicht aus der Anzeige.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(shared.lock().unwrap().datei_stand.senden.as_ref().map(|d| d.stand.kennung), Some(k2));
        text_senden("neuer Text", &shared, &input);
        assert_eq!(kennung(), None, "Text hat die Sendung nicht abgebrochen");
        let (gelesen, _) = host.join().unwrap();
        assert_eq!(gelesen.last(), Some(&(IN_CLIP, b"neuer Text".to_vec())));
        let enden: Vec<dateien::Ende> =
            gelesen.iter().filter(|(t, _)| *t == DATEI_ENDE).filter_map(|(_, p)| dateien::Ende::lesen(p)).collect();
        assert!(enden.contains(&dateien::Ende { kennung: k1, grund: dateien::GRUND_ABGEBROCHEN }), "{enden:?}");
        assert!(warten_bis(Duration::from_secs(5), || {
            shared.lock().unwrap().datei_stand.senden.as_ref().is_some_and(|d| {
                d.stand.kennung == k2 && d.stand.ergebnis == dateien::Ergebnis::Abgebrochen(dateien::Abbruch::Hier)
            })
        }));
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// run_session mit Scheinhost, Dateien in beide Richtungen:
    /// MSG_FAEHIGKEITEN setzt host_dateien, IN_FAEHIGKEITEN kommt als erste
    /// Nachricht auf dem Eingabekanal. Host -> Client: Angebot und Stuecke
    /// kommen an, werden quittiert (0, dann 1) und liegen danach im
    /// Ablageverzeichnis; die Ablage bekommt die obersten Eintraege. Client ->
    /// Host: Angebot, Stuecke und Ende gehen ueber den Eingabekanal, die
    /// Quittungen kommen ueber den Bildkanal. Dann das Sitzungsende mitten in
    /// zwei Uebertragungen: der Empfaenger loescht, was halb da ist, der
    /// Sender bricht ab, host_dateien ist wieder aus.
    #[test]
    fn dateien_ueber_die_sitzung() {
        use dateien::{Angebot, Eintrag, EintragArt, Ende, Ergebnis, Quittung, Stueck};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        secure::test_identitaet();
        decoder_bereit();
        let (host_priv, _) = test_host();
        let bild_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let eingabe_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bild_addr = bild_l.local_addr().unwrap().to_string();
        let eingabe_addr = eingabe_l.local_addr().unwrap().to_string();

        // Bildkanal des Scheinhosts: Gruss und Faehigkeiten, dann was der
        // Test schickt; endet der Kanal des Tests, ist die Leitung zu.
        let (hh_tx, hh_rx) = mpsc::channel::<Vec<u8>>();
        let (bild_tx, bild_rx) = mpsc::channel::<Vec<u8>>();
        let k = host_priv.clone();
        std::thread::spawn(move || {
            let (s, _) = bild_l.accept().unwrap();
            let mut h = secure::Secure::accept(s, &noise::prologue_video(), &k).unwrap();
            hh_tx.send(h.handshake_hash.clone()).unwrap();
            let mut m = MAGIC.to_vec();
            m.extend_from_slice(&eingabe_rahmen(MSG_FAEHIGKEITEN, &[1, 0, 0, 0]));
            if h.write_all(&m).is_err() {
                return;
            }
            while let Ok(r) = bild_rx.recv() {
                if h.write_all(&r).is_err() {
                    return;
                }
            }
        });
        // Eingabekanal des Scheinhosts: alles Gelesene an den Test.
        let (ein_tx, ein_rx) = mpsc::channel::<(u8, Vec<u8>)>();
        let k = host_priv.clone();
        std::thread::spawn(move || {
            let hh = hh_rx.recv().unwrap();
            let (s, _) = eingabe_l.accept().unwrap();
            let mut h = secure::Secure::accept(s, &noise::prologue_input(&hh), &k).unwrap();
            while let Some(n) = nachricht_lesen(&mut h) {
                if ein_tx.send(n).is_err() {
                    return;
                }
            }
        });

        let basis = test_ordner("dateien-sitzung").join("ablage");
        let abgelegt: Arc<Mutex<Vec<Vec<std::path::PathBuf>>>> = Arc::default();
        let a2 = abgelegt.clone();
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(bild_addr.clone()),
            datei_ablage: Some(DateiAblage {
                basis: basis.clone(),
                ablegen: Arc::new(move |p| {
                    a2.lock().unwrap().push(p);
                    true
                }),
            }),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(eingabe_addr)));
        // Wie der Fensterfaden: die Bindung des Eingabekanals nachfuehren.
        let halt = Arc::new(AtomicBool::new(false));
        {
            let (s, i, h) = (shared.clone(), input.clone(), halt.clone());
            std::thread::spawn(move || {
                while !h.load(Ordering::SeqCst) {
                    let link = s.lock().unwrap().link.clone();
                    {
                        let mut l = i.lock().unwrap();
                        l.set_link(link);
                        l.ensure();
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        let (ende_tx, ende_rx) = mpsc::channel();
        {
            let (s, i, a) = (shared.clone(), input.clone(), bild_addr.clone());
            std::thread::spawn(move || {
                let _ = ende_tx.send(run_session(&a, &s, &i));
            });
        }
        let frist = Duration::from_secs(10);
        let empfangen = || ein_rx.recv_timeout(frist).expect("nichts auf dem Eingabekanal");
        // Die naechste Nachricht dieser Art (Zeitfragen dazwischen uebergehen).
        let naechste = |art: u8| loop {
            let (a, p) = empfangen();
            if a == art {
                return p;
            }
        };
        let quittung_hin = |q: Quittung| eingabe_rahmen(DATEI_QUITTUNG, &q.kodieren());

        // Faehigkeiten in beide Richtungen.
        assert!(warten_bis(frist, || shared.lock().unwrap().host_dateien), "MSG_FAEHIGKEITEN kam nicht an");
        assert_eq!(empfangen(), (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]));

        // Host -> Client: ein Ordner mit einer Datei ueber drei Stuecke, eine kleine Datei.
        let inhalt_a: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let angebot = Angebot {
            kennung: 41,
            gesamt: 100_005,
            eintraege: vec![
                Eintrag { art: EintragArt::Ordner, pfad: "ordner".into(), groesse: 0 },
                Eintrag { art: EintragArt::Datei, pfad: "ordner/a.bin".into(), groesse: 100_000 },
                Eintrag { art: EintragArt::Datei, pfad: "b.txt".into(), groesse: 5 },
            ],
        };
        bild_tx.send(eingabe_rahmen(DATEI_ANGEBOT, &angebot.kodieren())).unwrap();
        let q = Quittung::lesen(&naechste(DATEI_QUITTUNG)).unwrap();
        assert_eq!(q, Quittung { kennung: 41, zustand: 0, empfangen: 0 });
        let mut versatz = 0u64;
        for teil in inhalt_a.chunks(dateien::STUECK_MAX) {
            let s = Stueck { kennung: 41, eintrag: 1, versatz, daten: teil };
            bild_tx.send(eingabe_rahmen(DATEI_STUECK, &s.kodieren())).unwrap();
            versatz += teil.len() as u64;
        }
        let s = Stueck { kennung: 41, eintrag: 2, versatz: 0, daten: b"hallo" };
        bild_tx.send(eingabe_rahmen(DATEI_STUECK, &s.kodieren())).unwrap();
        bild_tx.send(eingabe_rahmen(DATEI_ENDE, &Ende { kennung: 41, grund: 0 }.kodieren())).unwrap();
        let schluss = loop {
            let q = Quittung::lesen(&naechste(DATEI_QUITTUNG)).unwrap();
            if q.zustand != 0 {
                break q;
            }
        };
        assert_eq!(schluss, Quittung { kennung: 41, zustand: 1, empfangen: 100_005 });
        let pfade = abgelegt.lock().unwrap().last().cloned().expect("nicht abgelegt");
        assert_eq!(pfade.len(), 2);
        assert!(pfade.iter().all(|p| p.starts_with(&basis)), "{pfade:?}");
        assert_eq!(std::fs::read(pfade[0].join("a.bin")).unwrap(), inhalt_a);
        assert_eq!(std::fs::read(&pfade[1]).unwrap(), b"hallo");
        let stand = |f: fn(&DateiStaende) -> Option<&DateiAnzeige>| {
            f(&shared.lock().unwrap().datei_stand).map(|a| a.stand.clone())
        };
        assert!(warten_bis(frist, || stand(|d| d.empfangen.as_ref())
            .is_some_and(|s| s.ergebnis == Ergebnis::Fertig && s.oberste == 2)));

        // Client -> Host: eine Datei von 70 000 Byte.
        let quelle = test_ordner("dateien-sitzung-quelle");
        let inhalt_c: Vec<u8> = (0..70_000u32).map(|i| (i * 13 % 241) as u8).collect();
        let c = quelle.join("c.bin");
        std::fs::write(&c, &inhalt_c).unwrap();
        dateien_senden(vec![c.clone()], &shared, &input);
        let an = Angebot::lesen(&naechste(DATEI_ANGEBOT)).unwrap();
        assert_eq!((an.gesamt, an.eintraege.len(), an.eintraege[0].pfad.as_str()), (70_000, 1, "c.bin"));
        bild_tx.send(quittung_hin(Quittung { kennung: an.kennung, zustand: 0, empfangen: 0 })).unwrap();
        let mut daten = Vec::new();
        loop {
            let (a, p) = empfangen();
            match a {
                DATEI_STUECK => {
                    let s = Stueck::lesen(&p).unwrap();
                    assert_eq!((s.kennung, s.eintrag, s.versatz), (an.kennung, 0, daten.len() as u64));
                    daten.extend_from_slice(s.daten);
                }
                DATEI_ENDE => {
                    assert_eq!(Ende::lesen(&p).unwrap(), Ende { kennung: an.kennung, grund: 0 });
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(daten, inhalt_c);
        bild_tx.send(quittung_hin(Quittung { kennung: an.kennung, zustand: 1, empfangen: 70_000 })).unwrap();
        assert!(warten_bis(frist, || stand(|d| d.senden.as_ref())
            .is_some_and(|s| s.ergebnis == Ergebnis::Fertig && s.kennung == an.kennung)));

        // Zwei halbe Uebertragungen, dann schliesst der Host den Bildkanal.
        let halb = Angebot {
            kennung: 42,
            gesamt: 100_000,
            eintraege: vec![Eintrag { art: EintragArt::Datei, pfad: "d.bin".into(), groesse: 100_000 }],
        };
        bild_tx.send(eingabe_rahmen(DATEI_ANGEBOT, &halb.kodieren())).unwrap();
        assert_eq!(Quittung::lesen(&naechste(DATEI_QUITTUNG)).unwrap().kennung, 42);
        let s = Stueck { kennung: 42, eintrag: 0, versatz: 0, daten: &inhalt_a[..dateien::STUECK_MAX] };
        bild_tx.send(eingabe_rahmen(DATEI_STUECK, &s.kodieren())).unwrap();
        let verzeichnisse = |endung: &str| -> Vec<String> {
            std::fs::read_dir(&basis)
                .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n.ends_with(endung))
                .collect()
        };
        assert!(warten_bis(frist, || verzeichnisse("-42").iter().any(|v| {
            std::fs::metadata(basis.join(v).join("d.bin")).map(|m| m.len()).unwrap_or(0) == dateien::STUECK_MAX as u64
        })));
        dateien_senden(vec![c], &shared, &input);
        let an2 = Angebot::lesen(&naechste(DATEI_ANGEBOT)).unwrap();
        assert!(shared.lock().unwrap().datei_senden.is_some());
        drop(bild_tx);
        let r = ende_rx.recv_timeout(frist).expect("run_session endet nicht");
        assert!(r.is_err(), "{r:?}");
        {
            let s = shared.lock().unwrap();
            assert!(!s.host_dateien, "host_dateien blieb");
            assert!(s.datei_senden.is_none(), "Sender lief weiter");
        }
        assert!(warten_bis(frist, || verzeichnisse("-42").is_empty() && verzeichnisse("-42.laeuft").is_empty()),
            "halbes Verzeichnis blieb: {:?}", verzeichnisse(""));
        assert_eq!(verzeichnisse("-41").len(), 1, "das fertige ging mit");
        assert!(warten_bis(frist, || stand(|d| d.senden.as_ref())
            .is_some_and(|s| s.kennung == an2.kennung && matches!(s.ergebnis, Ergebnis::Abgebrochen(_)))));
        assert!(warten_bis(frist, || stand(|d| d.empfangen.as_ref())
            .is_some_and(|s| s.kennung == 42 && matches!(s.ergebnis, Ergebnis::Abgebrochen(_)))));
        halt.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_dir_all(basis.parent().unwrap());
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// Verlust des Eingabekanals mitten in einer Sendung (Spezifikation 2.7
    /// Schritt 5), ueber run_session mit Scheinhost: Kanal 1 liest ein fast
    /// volles Fenster Datei-Daten, quittiert nichts und schliesst. Der Sender
    /// wartet dann auf Quittungen, die nie kommen, und ruft seinen Weg nicht
    /// mehr auf - trotzdem ist die Sendung ab, kurz nachdem der Client den
    /// Verlust bemerkt (hier an Mausbewegungen wie im Fensterfaden), nicht
    /// erst nach STILLSTAND (30 s). Auf dem neuen Kanal geht nach
    /// IN_FAEHIGKEITEN von ihr nur noch das Ende 1 hinaus, kein Stueck.
    #[test]
    fn kanalverlust_bricht_die_sendung_ab() {
        use dateien::{Ende, Ergebnis};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        secure::test_identitaet();
        decoder_bereit();
        let (host_priv, _) = test_host();
        let bild_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let eingabe_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bild_addr = bild_l.local_addr().unwrap().to_string();
        let eingabe_addr = eingabe_l.local_addr().unwrap().to_string();
        let halt = Arc::new(AtomicBool::new(false));

        // Bildkanal: Gruss und Faehigkeiten, dann alle 10 ms eine Nachricht,
        // die run_session uebergeht (wie die Bilder, die ein Host wiederholt).
        let (hh_tx, hh_rx) = mpsc::channel::<Vec<u8>>();
        {
            let (k, h) = (host_priv.clone(), halt.clone());
            std::thread::spawn(move || {
                let (s, _) = bild_l.accept().unwrap();
                let mut c = secure::Secure::accept(s, &noise::prologue_video(), &k).unwrap();
                hh_tx.send(c.handshake_hash.clone()).unwrap();
                let mut m = MAGIC.to_vec();
                m.extend_from_slice(&eingabe_rahmen(MSG_FAEHIGKEITEN, &[1, 0, 0, 0]));
                if c.write_all(&m).is_err() {
                    return;
                }
                while !h.load(Ordering::SeqCst) {
                    if c.write_all(&eingabe_rahmen(0xee, &[])).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
        }
        // Eingabekanal: Kanal 1 liest Datei-Daten bis kurz vor das Fenster
        // und schliesst ohne Quittung; Kanal 2 reicht alles an den Test.
        let (ein_tx, ein_rx) = mpsc::channel::<(u8, Vec<u8>)>();
        let (zu_tx, zu_rx) = mpsc::channel::<Instant>();
        {
            let k = host_priv.clone();
            std::thread::spawn(move || {
                let hh = hh_rx.recv().unwrap();
                let (s, _) = eingabe_l.accept().unwrap();
                let mut c = secure::Secure::accept(s, &noise::prologue_input(&hh), &k).unwrap();
                let mut daten = 0usize;
                while let Some((a, p)) = nachricht_lesen(&mut c) {
                    if a == DATEI_STUECK {
                        daten += p.len() - dateien::STUECK_KOPF;
                        if daten >= 250_000 {
                            break;
                        }
                    }
                }
                let _ = c.socket().shutdown(std::net::Shutdown::Both);
                drop(c);
                let _ = zu_tx.send(Instant::now());
                let (s, _) = eingabe_l.accept().unwrap();
                let mut c = secure::Secure::accept(s, &noise::prologue_input(&hh), &k).unwrap();
                while let Some(n) = nachricht_lesen(&mut c) {
                    if ein_tx.send(n).is_err() {
                        return;
                    }
                }
            });
        }

        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(bild_addr.clone()),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(eingabe_addr)));
        // Wie der Fensterfaden: Bindung nachfuehren, Aufbau treiben, dazu alle
        // 50 ms eine Mausbewegung - erst ein Schreibversuch zeigt, dass eine
        // Leitung tot ist.
        {
            let (s, i, h) = (shared.clone(), input.clone(), halt.clone());
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !h.load(Ordering::SeqCst) {
                    let link = s.lock().unwrap().link.clone();
                    {
                        let mut l = i.lock().unwrap();
                        l.set_link(link);
                        l.ensure();
                        if n % 10 == 0 {
                            l.mouse_move(0.5, 0.5);
                        }
                    }
                    n += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        let (ende_tx, ende_rx) = mpsc::channel();
        {
            let (s, i, a) = (shared.clone(), input.clone(), bild_addr.clone());
            std::thread::spawn(move || {
                let _ = ende_tx.send(run_session(&a, &s, &i));
            });
        }
        let frist = Duration::from_secs(10);
        assert!(warten_bis(frist, || shared.lock().unwrap().host_dateien), "MSG_FAEHIGKEITEN kam nicht an");
        assert!(warten_bis(frist, || input.lock().unwrap().steht()), "Eingabekanal steht nicht");

        // 1 MB: nach dem ersten Fenster wartet der Sender auf Quittungen.
        let quelle = test_ordner("kanalverlust");
        let a = quelle.join("a.bin");
        std::fs::write(&a, vec![5u8; 1_000_000]).unwrap();
        dateien_senden(vec![a], &shared, &input);
        let kennung = shared.lock().unwrap().datei_senden.as_ref().map(|g| g.kennung()).expect("keine Sendung");
        let zu = zu_rx.recv_timeout(frist).expect("Kanal 1 kam nicht bis kurz vor das Fenster");

        let abgebrochen = || {
            shared.lock().unwrap().datei_stand.senden.as_ref().is_some_and(|d| {
                d.stand.kennung == kennung && matches!(d.stand.ergebnis, Ergebnis::Abgebrochen(_))
            })
        };
        assert!(
            warten_bis(Duration::from_secs(5), abgebrochen),
            "Sendung {:?} nach dem Verlust des Eingabekanals noch nicht abgebrochen",
            zu.elapsed()
        );
        assert!(shared.lock().unwrap().datei_senden.is_none(), "Griff blieb stehen");

        // Kanal 2: zuerst die Faehigkeiten, von der Sendung nur ihr Ende 1.
        // Eine Frist fuer alles: die Mausbewegungen kommen laufend.
        let bis = Instant::now() + Duration::from_secs(5);
        let mut gelesen: Vec<(u8, Vec<u8>)> = Vec::new();
        while !gelesen.iter().any(|(a, _)| *a == DATEI_ENDE) {
            match ein_rx.recv_timeout(bis.saturating_duration_since(Instant::now())) {
                Ok(n) => gelesen.push(n),
                Err(_) => panic!(
                    "kein Ende auf dem neuen Kanal, dort kam: {:?}",
                    gelesen.iter().map(|(a, _)| *a).collect::<std::collections::BTreeSet<u8>>()
                ),
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        while let Ok(n) = ein_rx.try_recv() {
            gelesen.push(n);
        }
        assert_eq!(gelesen[0], (IN_FAEHIGKEITEN, vec![1, 0, 0, 0]));
        let datei: Vec<&(u8, Vec<u8>)> =
            gelesen.iter().filter(|(a, _)| (DATEI_ANGEBOT..=DATEI_QUITTUNG).contains(a)).collect();
        assert_eq!(datei.len(), 1, "auf dem neuen Kanal: {:?}", datei.iter().map(|(a, _)| *a).collect::<Vec<_>>());
        assert_eq!(Ende::lesen(&datei[0].1), Some(Ende { kennung, grund: dateien::GRUND_ABGEBROCHEN }));

        halt.store(true, Ordering::SeqCst);
        let r = ende_rx.recv_timeout(frist).expect("run_session endet nicht");
        assert!(r.is_err(), "{r:?}");
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// Die Anzeige: Staende nach der Kennung (ein spaetes "abgebrochen" des
    /// abgeloesten Senders ueberschreibt den neuen nicht), laufend immer,
    /// Ergebnisse 6 s sichtbar; die Texte in jeder Sprache ohne offenen
    /// Platzhalter, Groessen in MB mit einer Nachkommastelle.
    #[test]
    fn datei_zeile_nach_kennung_und_nachlauf() {
        use dateien::{Abbruch, Ergebnis, Richtung, Stand};
        let st = |richtung, kennung, ergebnis| Stand {
            richtung,
            kennung,
            bytes: 1_260_000,
            gesamt: 12_400_000,
            eintraege: 4,
            dateien: 3,
            oberste: 2,
            ergebnis,
        };
        let mut d = DateiStaende::default();
        let jetzt = Instant::now();
        assert!(!d.sichtbar(jetzt));
        d.setzen(st(Richtung::Senden, 5, Ergebnis::Laeuft));
        d.setzen(st(Richtung::Senden, 4, Ergebnis::Abgebrochen(Abbruch::Hier)));
        assert_eq!(d.senden.as_ref().map(|a| (a.stand.kennung, a.stand.laeuft())), Some((5, true)));
        assert!(d.sichtbar(jetzt + Duration::from_secs(600)), "laufend verschwindet");
        d.setzen(st(Richtung::Senden, 5, Ergebnis::Fertig));
        let t = d.senden.as_ref().unwrap().ergebnis_seit.unwrap();
        assert!(d.sichtbar(t + Duration::from_millis(5900)));
        assert!(!d.sichtbar(t + Duration::from_millis(6100)));
        // FilesPeerOld (Kennung 0) gilt immer, danach wieder jede Sendung.
        d.setzen(Stand::gegenseite_zu_alt(Richtung::Senden));
        assert_eq!(d.senden.as_ref().unwrap().stand.ergebnis, Ergebnis::GegenseiteZuAlt);
        d.setzen(st(Richtung::Senden, 6, Ergebnis::Laeuft));
        assert_eq!(d.senden.as_ref().unwrap().stand.kennung, 6);
        // Beide Richtungen zugleich: zwei Zeilen, Empfangen zuerst.
        d.setzen(st(Richtung::Empfangen, 9, Ergebnis::Laeuft));
        let jetzt = Instant::now();
        assert_eq!(d.sichtbare(jetzt).iter().map(|s| s.richtung).collect::<Vec<_>>(), vec![Richtung::Empfangen, Richtung::Senden]);

        let de = strings::pick("de");
        let en = strings::pick("en");
        assert_eq!(datei_zeile(&st(Richtung::Senden, 1, Ergebnis::Laeuft), de).0, "Dateien werden gesendet: 1.3 von 12.4 MB");
        assert_eq!(datei_zeile(&st(Richtung::Empfangen, 1, Ergebnis::Laeuft), en).0, "Receiving files: 1.3 of 12.4 MB");
        assert_eq!(datei_zeile(&st(Richtung::Empfangen, 1, Ergebnis::Fertig), de), ("Dateien bereit zum Einfügen: 2".to_string(), ui::CYAN));
        assert_eq!(datei_zeile(&st(Richtung::Senden, 1, Ergebnis::Fertig), de).0, "Dateien übertragen: 2");
        let (t, f) = datei_zeile(&st(Richtung::Senden, 1, Ergebnis::Abgebrochen(Abbruch::Quittung(3))), de);
        assert_eq!((t.as_str(), f), ("Dateiübertragung abgebrochen: zu wenig Speicherplatz", ui::AMBER));
        let alle = [
            Ergebnis::Laeuft,
            Ergebnis::Fertig,
            Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung),
            Ergebnis::Abgebrochen(Abbruch::Hier),
            Ergebnis::Abgebrochen(Abbruch::Quittung(3)),
            Ergebnis::ZuGross,
            Ergebnis::GegenseiteZuAlt,
        ];
        for lang in strings::all() {
            for e in &alle {
                for r in [Richtung::Senden, Richtung::Empfangen] {
                    let (t, _) = datei_zeile(&st(r, 1, e.clone()), lang);
                    assert!(!t.is_empty() && !t.contains('{'), "{} {e:?}: {t}", lang.code);
                }
            }
        }
    }

    /// Abbruchgruende in der Datei-Zeile (Durchsicht [13]): jeder Grund des
    /// Kerns kommt als uebersetzter Schluessel, in jeder Sprache ohne
    /// Platzhalter; Einzelheiten (Pfade, Systemtexte, der deutsche
    /// Protokolltext) erscheinen nie - auf Englisch steht kein deutsches Wort.
    #[test]
    fn abbruchgruende_uebersetzt() {
        use dateien::{Abbruch, Ergebnis, Richtung, Stand};
        use strings::Key::*;
        let geheim = "/Users/geheim/Datei.bin: No such file".to_string();
        let faelle = [
            (Abbruch::Hier, FilesAbortLocal),
            (Abbruch::Verbindung, FilesAbortConnection),
            (Abbruch::Lesefehler(geheim.clone()), FilesAbortRead),
            (Abbruch::Zeitueberschreitung, FilesAbortTimeout),
            (Abbruch::Quittung(dateien::ZUSTAND_FERTIG), FilesAbortInvalid),
            (Abbruch::Quittung(dateien::ZUSTAND_KEIN_PLATZ), FilesAbortNoSpace),
            (Abbruch::Quittung(dateien::ZUSTAND_UNGUELTIG), FilesAbortInvalid),
            (Abbruch::Quittung(dateien::ZUSTAND_SCHREIBFEHLER), FilesAbortWrite),
            (Abbruch::Quittung(dateien::ZUSTAND_ABGEBROCHEN), FilesAbortPeer),
            (Abbruch::Quittung(99), FilesAbortInvalid),
            (Abbruch::Ende(dateien::GRUND_ABGEBROCHEN), FilesAbortPeer),
            (Abbruch::Ende(dateien::GRUND_LESEFEHLER), FilesAbortRead),
            (Abbruch::Ende(dateien::GRUND_ZEIT), FilesAbortTimeout),
            (Abbruch::Ende(99), FilesAbortInvalid),
            (Abbruch::Abgelehnt(dateien::ZUSTAND_KEIN_PLATZ, geheim.clone()), FilesAbortNoSpace),
            (Abbruch::Abgelehnt(dateien::ZUSTAND_UNGUELTIG, geheim.clone()), FilesAbortInvalid),
            (Abbruch::Ungueltig(geheim.clone()), FilesAbortInvalid),
            (Abbruch::Schreibfehler(geheim.clone()), FilesAbortWrite),
            (Abbruch::Ablage, FilesAbortClipboard),
        ];
        for (a, k) in &faelle {
            assert_eq!(abbruch_schluessel(a), *k, "{a:?}");
            for r in [Richtung::Senden, Richtung::Empfangen] {
                let st = Stand {
                    richtung: r,
                    kennung: 1,
                    bytes: 0,
                    gesamt: 0,
                    eintraege: 1,
                    dateien: 1,
                    oberste: 1,
                    ergebnis: Ergebnis::Abgebrochen(a.clone()),
                };
                for lang in strings::all() {
                    let (t, farbe) = datei_zeile(&st, lang);
                    assert_eq!(farbe, ui::AMBER);
                    assert_eq!(t, lang.get(FilesAborted).replace("{n}", lang.get(*k)), "{} {a:?}", lang.code);
                    assert!(!t.contains("geheim") && !t.contains(&a.text()), "{} {a:?}: {t}", lang.code);
                }
                let en = datei_zeile(&st, strings::pick("en")).0;
                for wort in ["Gegenseite", "abgebrochen", "Zeitueberschreitung", "Verbindung", "Lesefehler", "Schreibfehler", "Ablage"] {
                    assert!(!en.contains(wort), "{a:?}: {en}");
                }
            }
        }
        assert_eq!(
            datei_zeile(
                &Stand {
                    richtung: Richtung::Senden,
                    kennung: 1,
                    bytes: 0,
                    gesamt: 0,
                    eintraege: 1,
                    dateien: 1,
                    oberste: 1,
                    ergebnis: Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung)
                },
                strings::pick("en")
            )
            .0,
            "File transfer aborted: timeout, no progress"
        );
    }

    /// Neu gezeichnet wird bei einem neuen Bild, einem ausgelassenen Present,
    /// alle 33 ms, solange Oberflaeche sichtbar ist - und genau einmal, wenn
    /// sie verschwindet (Durchsicht [11]: sonst bliebe die Datei-Zeile bei
    /// stillem Bildschirm stehen).
    #[test]
    fn neu_zeichnen_beim_verschwinden() {
        let ms = Duration::from_millis;
        // Nichts los: nicht zeichnen.
        assert!(!neu_zeichnen(false, false, false, false, ms(1000)));
        assert!(neu_zeichnen(true, false, false, false, ms(0)));
        assert!(neu_zeichnen(false, true, false, false, ms(0)));
        // Sichtbar: im 33-ms-Takt.
        assert!(!neu_zeichnen(false, false, true, true, ms(10)));
        assert!(neu_zeichnen(false, false, true, true, ms(33)));
        // Der Takt, in dem sie verschwindet: einmal, sofort.
        assert!(neu_zeichnen(false, false, false, true, ms(1)));
        // Danach wieder nichts. So, wie about_to_wait `vorher` fuehrt:
        let mut vorher = false;
        let mut zeichnungen = 0;
        for sichtbar in [true, true, true, false, false, false] {
            let v = std::mem::replace(&mut vorher, sichtbar);
            zeichnungen += neu_zeichnen(false, false, sichtbar, v, ms(40)) as u32;
        }
        assert_eq!(zeichnungen, 4, "drei im Takt und eine beim Verschwinden");
    }

    /// Der Datei-Platz des Schreibers (Durchsicht [10]): gerahmt wird nur,
    /// was hineinkommt - bei Voll entsteht kein Rahmen, und das wartende
    /// Paket bleibt unberuehrt. Ein verworfener Schreiber meldet Weg.
    #[test]
    fn datei_rahmen_erst_bei_freiem_platz() {
        let mut w = Warteschlange::default();
        let gebaut = std::cell::Cell::new(0u32);
        let bauen = |n: u8| {
            gebaut.set(gebaut.get() + 1);
            eingabe_rahmen(DATEI_STUECK, &[n; 64])
        };
        assert_eq!(w.datei_platz(|| bauen(1)), dateien::Gesendet::Ja);
        for _ in 0..100 {
            assert_eq!(w.datei_platz(|| bauen(2)), dateien::Gesendet::Voll);
        }
        assert_eq!(gebaut.get(), 1, "Rahmen bei Voll gebaut");
        assert_eq!(w.datei.as_deref(), Some(eingabe_rahmen(DATEI_STUECK, &[1; 64]).as_slice()));
        w.datei = None;
        assert_eq!(w.datei_platz(|| bauen(3)), dateien::Gesendet::Ja);
        assert_eq!(gebaut.get(), 2);
        w.zu = true;
        w.datei = None;
        assert_eq!(w.datei_platz(|| bauen(4)), dateien::Gesendet::Weg);
        assert_eq!(gebaut.get(), 2);
    }

    /// Kopie direkt nach dem Verbinden (Integrationstest 574cd3e, Befund 3):
    /// solange MSG_FAEHIGKEITEN noch aussteht, wird vorgemerkt statt
    /// FilesPeerOld gemeldet; mit den Faehigkeiten geht das Angebot hinaus.
    /// Kommen sie binnen DATEI_ANLAUF nicht, steht FilesPeerOld da (der Host
    /// ist aelter); ist die Sitzung schon aelter, sofort. Neuer Text und eine
    /// neue Sitzung verwerfen die vorgemerkte Kopie.
    #[test]
    fn kopie_vor_den_faehigkeiten_wird_vorgemerkt() {
        secure::test_identitaet();
        let hh = vec![0x7e; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::ZERO, DATEI_ANGEBOT);
        let _ = los.send(());
        let input = Arc::new(Mutex::new(InputLink::new(addr)));
        {
            let mut l = input.lock().unwrap();
            l.set_link(Some((hh.clone(), host_pub.clone())));
            eingabe_abwarten(&mut l);
            assert!(l.steht());
        }
        let quelle = test_ordner("vorgemerkt");
        let datei = quelle.join("a.txt");
        std::fs::write(&datei, b"hallo").unwrap();
        let neu = || Shared {
            link: Some((hh.clone(), host_pub.clone())),
            sitzung_nr: 3,
            sitzung_seit: Some(Instant::now()),
            ..Shared::default()
        };
        let shared = Arc::new(Mutex::new(neu()));

        dateien_senden(vec![datei.clone()], &shared, &input);
        {
            let s = shared.lock().unwrap();
            assert!(s.datei_senden.is_none(), "Sender vor den Faehigkeiten gestartet");
            assert!(s.datei_stand.senden.is_none(), "FilesPeerOld vor Ablauf des Anlaufs");
            assert!(!s.host_zu_alt_gemeldet);
            assert_eq!(s.datei_vorgemerkt.as_ref().map(|v| v.pfade.clone()), Some(vec![datei.clone()]));
        }
        assert_eq!(vormerk_takt(&shared), Some(VORMERK_TAKT));
        // Noch keine Faehigkeiten, Anlauf nicht um: bleibt vorgemerkt.
        vorgemerkte_dateien(&shared, &input);
        assert!(shared.lock().unwrap().datei_vorgemerkt.is_some());
        // Die Faehigkeiten kommen: das Angebot geht hinaus.
        {
            let mut s = shared.lock().unwrap();
            s.faehigkeiten_da = true;
            s.host_dateien = true;
        }
        vorgemerkte_dateien(&shared, &input);
        assert!(shared.lock().unwrap().datei_vorgemerkt.is_none());
        assert!(shared.lock().unwrap().datei_senden.is_some(), "vorgemerkte Kopie nicht gesendet");
        assert_eq!(vormerk_takt(&shared), None);
        let (gelesen, _) = host.join().unwrap();
        let arten: Vec<u8> = gelesen.iter().map(|(a, _)| *a).collect();
        assert_eq!(arten, vec![IN_FAEHIGKEITEN, DATEI_ANGEBOT]);

        // Die Faehigkeiten bleiben aus: nach dem Anlauf FilesPeerOld, einmal.
        let shared = Arc::new(Mutex::new(neu()));
        dateien_senden(vec![datei.clone()], &shared, &input);
        assert!(shared.lock().unwrap().datei_vorgemerkt.is_some());
        {
            // Die Zeit vorstellen: Sitzung und Vormerkung sind DATEI_ANLAUF alt.
            let mut s = shared.lock().unwrap();
            s.sitzung_seit = Some(Instant::now() - DATEI_ANLAUF - Duration::from_millis(1));
            s.datei_vorgemerkt.as_mut().unwrap().bis = Instant::now();
        }
        vorgemerkte_dateien(&shared, &input);
        {
            let s = shared.lock().unwrap();
            assert!(s.datei_vorgemerkt.is_none());
            assert!(s.datei_senden.is_none());
            assert!(s.host_zu_alt_gemeldet);
            assert_eq!(s.datei_stand.senden.as_ref().map(|a| a.stand.ergebnis.clone()), Some(dateien::Ergebnis::GegenseiteZuAlt));
        }
        // Eine aeltere Sitzung ohne Faehigkeiten: sofort FilesPeerOld.
        let shared = Arc::new(Mutex::new(Shared {
            sitzung_seit: Some(Instant::now() - DATEI_ANLAUF - Duration::from_millis(1)),
            ..neu()
        }));
        dateien_senden(vec![datei.clone()], &shared, &input);
        assert!(shared.lock().unwrap().datei_vorgemerkt.is_none());
        assert!(shared.lock().unwrap().host_zu_alt_gemeldet, "aelterer Host nicht sofort gemeldet");
        // Neuer Text bzw. eine neue Sitzung verwerfen die Vormerkung.
        let shared = Arc::new(Mutex::new(neu()));
        dateien_senden(vec![datei.clone()], &shared, &input);
        text_senden("neu", &shared, &input);
        assert!(shared.lock().unwrap().datei_vorgemerkt.is_none(), "Text hat die Vormerkung nicht verworfen");
        dateien_senden(vec![datei.clone()], &shared, &input);
        {
            let mut s = shared.lock().unwrap();
            s.sitzung_nr += 1;
            s.faehigkeiten_da = true;
            s.host_dateien = true;
        }
        vorgemerkte_dateien(&shared, &input);
        let s = shared.lock().unwrap();
        assert!(s.datei_vorgemerkt.is_none() && s.datei_senden.is_none(), "Vormerkung der vorigen Sitzung gesendet");
        drop(s);
        let _ = std::fs::remove_dir_all(&quelle);
    }

    /// Eine neue Sendung, waehrend der Eingabekanal noch im Aufbau ist
    /// (Integrationstest 574cd3e, Befund 3): Voll statt Weg - auch, solange
    /// er im Pruefmodus noch gar nicht an die Sitzung gebunden ist -, bis
    /// er steht; dann geht das Angebot hinaus. Nach dem Anlauf gilt Weg wie
    /// bisher, und ebenso, sobald ein Paket hinaus ist (Verlust des Kanals).
    #[test]
    fn datei_weg_wartet_auf_den_aufbau() {
        use std::sync::atomic::{AtomicU64, Ordering};
        secure::test_identitaet();
        let hh = vec![0x7f; 32];
        let (host_priv, host_pub) = noise::keypair().unwrap();
        // Der Host nimmt erst nach 600 ms an: so lange steht der Kanal nicht.
        let (addr, los, host) = eingabe_host(hh.clone(), host_priv, Duration::from_millis(600), DATEI_ANGEBOT);
        let input = Arc::new(Mutex::new(InputLink::new(addr)));
        let kanal = Arc::new(AtomicU64::new(0));
        let weg = datei_weg(&input, hh.clone(), kanal.clone());
        // Noch nicht an die Sitzung gebunden (Pruefmodus: alle 3 s).
        assert_eq!(weg.senden(DATEI_ANGEBOT, &[1]), dateien::Gesendet::Voll, "ohne Bindung");
        input.lock().unwrap().set_link(Some((hh.clone(), host_pub.clone())));
        // Im Aufbau: Voll, nie Weg, bis der Kanal steht.
        let t0 = Instant::now();
        let mut voll = 0;
        loop {
            match weg.senden(DATEI_ANGEBOT, &[1]) {
                dateien::Gesendet::Ja => break,
                dateien::Gesendet::Voll => voll += 1,
                dateien::Gesendet::Weg => panic!("Weg im Aufbau nach {:?}", t0.elapsed()),
            }
            assert!(t0.elapsed() < Duration::from_secs(4), "Kanal steht nicht");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(voll > 0, "der Kanal stand sofort - der Test prueft nichts");
        assert_eq!(kanal.load(Ordering::SeqCst), 1);
        let _ = los.send(());
        let (gelesen, _) = host.join().unwrap();
        assert_eq!(gelesen.iter().map(|(a, _)| *a).collect::<Vec<_>>(), vec![IN_FAEHIGKEITEN, DATEI_ANGEBOT]);
        // Gebunden und der Kanal weg: Weg (Verlust des Eingabekanals).
        input.lock().unwrap().trennen();
        assert_eq!(weg.senden(DATEI_STUECK, &[2]), dateien::Gesendet::Weg);
        // Nach dem Anlauf: ohne Bindung bzw. ohne Kanal Weg wie bisher.
        let ohne = Arc::new(Mutex::new(InputLink::new("127.0.0.1:1".into())));
        let abgelaufen = datei_weg_bis(&ohne, hh.clone(), Arc::new(AtomicU64::new(0)), Instant::now());
        assert_eq!(abgelaufen.senden(DATEI_ANGEBOT, &[1]), dateien::Gesendet::Weg);
        ohne.lock().unwrap().set_link(Some((hh, host_pub)));
        assert_eq!(abgelaufen.senden(DATEI_ANGEBOT, &[1]), dateien::Gesendet::Weg);
    }

    // ---------------------------- Zugang (Spezifikation Pairing v1, 3.5, 8, 9)

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// Wie sich der Test-Host der Zugangsphase verhaelt.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Zugangshost {
        /// Kennt jeden: "QCH1" - auch wenn der Client Bit 0 setzt (ein Host,
        /// der sich nicht ausweist).
        Bekannt,
        /// Kennt diesen Client: ohne Bit 0 in Nachricht 3 "QCH1"; mit Bit 0
        /// (der Client kennt den Host nicht) die Zugangsphase wie Passwort.
        /// Meldet "Bit 0" bzw. "ohne Bit 0".
        KenntClient,
        /// Unbekannt, 20 ohne "Zulassen" (Bit 1 aus) - und nach 300 ms
        /// trotzdem 22/1, ohne dass ein Beweis kam.
        ZulassenUngefragt,
        /// Unbekannt: "QCA1", 20; prueft Beweise gegen das Passwort.
        Passwort,
        /// Wie Passwort, aber Ergebnis 0 mit falschem host_proof.
        FalscherBeweis,
        /// Unbekannt, "Zulassen" moeglich: nach 300 ms Ergebnis 1.
        Zulassen,
        /// Nach 300 ms Ergebnis 3.
        Ablehnen,
        /// Nach 300 ms Ergebnis 4 ohne Wartezeit (Frist des Hosts abgelaufen).
        Frist,
        /// Aeltere Fassung: macht nach dem Handschlag zu.
        Alt,
        /// Schweigt nach dem Handschlag.
        Stumm,
        /// Kein Platz frei: "QCA1", gleich 22/4 mit 5000 ms statt 20, dann
        /// zu - so antworten Mac- und Windows-Host.
        Voll,
        /// Drossel: 20 mit 600 ms Wartezeit, "falsch" mit 700 ms; meldet
        /// jeden Beweis, der vor Ablauf kommt ("zu frueh").
        Drossel,
        /// Spricht nach dem Handschlag etwas Fremdes ("QCX9").
        Fremd,
        /// Ein falscher Host: bietet kein Zulassen an (Bit 1 aus), nimmt den
        /// Beweis und sagt 22/1 statt 22/0 mit host_proof.
        ZulassenOhneAngebot,
        /// Erste Verbindung: "QCH1", eine Sitzung, die er nach 300 ms kappt
        /// (am Host entfernt); jede weitere: "QCA1", 20 mit Zulassen.
        Entfernt,
        /// Schickt nach dem Handschlag einen Datensatz, der nicht echt ist.
        Kaputt,
        /// Erste Verbindung: "QCH1", eine Sitzung, die er nach 300 ms kappt;
        /// die zweite macht er nach dem Handschlag zu (Aufnahme startet
        /// nicht, verdraengt ...); ab der dritten wieder "QCH1".
        Aussetzer,
    }

    /// Ein kleiner Host fuer die Zugangsphase mit eigenem Schluessel: nimmt
    /// jede Verbindung an und folgt `art`. Nach der Annahme ("QCH1") meldet
    /// er gleich die Abloesung - die Sitzung endet ohne Neuversuch, und der
    /// Test sieht am Ziel, dass sie vorbei ist. Was er erlebt ("falsch",
    /// "richtig", Nachrichten des Clients), meldet er ueber den Kanal.
    fn zugangshost(art: Zugangshost, passwort: &'static str) -> (String, Vec<u8>, mpsc::Receiver<String>, Arc<AtomicUsize>) {
        use std::net::TcpListener;
        use zugang::{Ergebnis, Nachricht};
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel::<String>();
        let zaehler = Arc::new(AtomicUsize::new(0));
        let (z, hp) = (zaehler.clone(), host_pub.clone());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                let nummer = z.fetch_add(1, Ordering::SeqCst);
                let Ok(mut h) = secure::Secure::accept(s, &noise::prologue_video(), &host_priv) else {
                    let _ = tx.send("Handschlag gescheitert".into());
                    continue;
                };
                let annahme = |h: &mut secure::Secure| {
                    let mut m = MAGIC.to_vec();
                    m.extend_from_slice(&[MSG_ABGELOEST, 0, 0, 0, 0, 0, 0, 0]);
                    let _ = h.write_all(&m);
                };
                let ergebnis = |h: &mut secure::Secure, e: Ergebnis| {
                    let _ = h.write_all(&Nachricht::Ergebnis(e).kodieren());
                };
                let bit0 = zugang::nachricht3_flags(&h.nachricht3) & NAME_FLAG_HOST_UNBEKANNT != 0;
                if art == Zugangshost::KenntClient {
                    let _ = tx.send(if bit0 { "Bit 0" } else { "ohne Bit 0" }.into());
                }
                match art {
                    Zugangshost::Bekannt => annahme(&mut h),
                    Zugangshost::KenntClient if !bit0 => annahme(&mut h),
                    Zugangshost::Alt => continue,
                    Zugangshost::Stumm => std::thread::sleep(zugangsphase::KENNUNG_FRIST + Duration::from_millis(500)),
                    Zugangshost::Fremd => {
                        let _ = h.write_all(b"QCX9");
                    }
                    Zugangshost::Kaputt => {
                        let mut roh: &std::net::TcpStream = h.socket();
                        let _ = std::io::Write::write_all(&mut roh, &[4, 0, 1, 2, 3, 4]);
                    }
                    Zugangshost::Voll => {
                        let mut m = MAGIC_ZUGANG.to_vec();
                        m.extend_from_slice(&Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 5000 }).kodieren());
                        let _ = h.write_all(&m);
                    }
                    Zugangshost::Entfernt | Zugangshost::Aussetzer if nummer == 0 => {
                        let _ = h.write_all(MAGIC);
                        std::thread::sleep(Duration::from_millis(300));
                        continue;
                    }
                    Zugangshost::Aussetzer if nummer == 1 => continue,
                    Zugangshost::Aussetzer => annahme(&mut h),
                    _ => {
                        let mut m = MAGIC_ZUGANG.to_vec();
                        let zulassen = matches!(art, Zugangshost::Zulassen | Zugangshost::Entfernt);
                        let warten_ms = if art == Zugangshost::Drossel { 600 } else { 0 };
                        let noetig = zugang::ZugangNoetig::neu(zulassen, warten_ms, "Testhost");
                        m.extend_from_slice(&Nachricht::Noetig(noetig).kodieren());
                        let _ = h.write_all(&m);
                        // Vorher ist ein Beweis zu frueh (Drossel).
                        let mut frei_ab = Instant::now() + Duration::from_millis(warten_ms as u64);
                        match art {
                            Zugangshost::Zulassen => {
                                std::thread::sleep(Duration::from_millis(300));
                                ergebnis(&mut h, Ergebnis::Zulassen);
                                annahme(&mut h);
                            }
                            Zugangshost::Ablehnen => {
                                std::thread::sleep(Duration::from_millis(300));
                                ergebnis(&mut h, Ergebnis::Abgelehnt);
                            }
                            Zugangshost::Frist => {
                                std::thread::sleep(Duration::from_millis(300));
                                ergebnis(&mut h, Ergebnis::Schluss { warten_ms: 0 });
                            }
                            Zugangshost::ZulassenUngefragt => {
                                std::thread::sleep(Duration::from_millis(300));
                                ergebnis(&mut h, Ergebnis::Zulassen);
                                annahme(&mut h);
                            }
                            _ => loop {
                                match zugang::empfangen(|b| h.lesen(b)) {
                                    Ok(Nachricht::Beweis(_)) if art == Zugangshost::ZulassenOhneAngebot => {
                                        let _ = tx.send("Beweis".into());
                                        ergebnis(&mut h, Ergebnis::Zulassen);
                                        annahme(&mut h);
                                        break;
                                    }
                                    Ok(Nachricht::Beweis(b)) => {
                                        if Instant::now() < frei_ab {
                                            let _ = tx.send("zu frueh".into());
                                        }
                                        let k = zugang::passwort_schluessel(passwort, &hp);
                                        if !zugang::beweis_pruefen(&k, &h.handshake_hash, &b) {
                                            let _ = tx.send("falsch".into());
                                            let warten_ms = if art == Zugangshost::Drossel { 700 } else { 0 };
                                            ergebnis(&mut h, Ergebnis::Falsch { warten_ms });
                                            frei_ab = Instant::now() + Duration::from_millis(warten_ms as u64);
                                            continue;
                                        }
                                        let _ = tx.send("richtig".into());
                                        let host_beweis = if art == Zugangshost::FalscherBeweis {
                                            [0x55; 32]
                                        } else {
                                            zugang::host_beweis(&k, &h.handshake_hash)
                                        };
                                        ergebnis(&mut h, Ergebnis::Passwort { host_beweis });
                                        if matches!(art, Zugangshost::Passwort | Zugangshost::Drossel | Zugangshost::KenntClient) {
                                            annahme(&mut h);
                                        }
                                        break;
                                    }
                                    Ok(n) => {
                                        let _ = tx.send(format!("{n:?}"));
                                        break;
                                    }
                                    Err(e) => {
                                        let _ = tx.send(format!("Ende: {e}"));
                                        break;
                                    }
                                }
                            },
                        }
                    }
                }
                // Offen halten, bis der Client geht: macht der Host zu, bevor
                // der Client gelesen hat, koennte das System die Daten mit
                // einem Reset verwerfen.
                h.lesefrist(Some(Duration::from_secs(5)));
                let mut b = [0u8; 1];
                let _ = h.lesen(&mut b);
            }
        });
        (addr, host_pub, rx, zaehler)
    }

    /// Der Client gegen einen Host, im Empfangsfaden wie im Betrieb.
    /// `nutzer` spielt den Nutzer am Dialog: es sieht ihn alle 20 ms und
    /// darf eine Eingabe liefern (Abbrechen nimmt wie im Fenster auch das
    /// Ziel zurueck). Endet, sobald das Ziel weg ist (hoechstens 20 s);
    /// liefert den gemeinsamen Stand und alle Dialogstaende, die zu sehen
    /// waren.
    fn zugang_durchspielen(
        addr: &str,
        ziel_id: Option<u32>,
        mut nutzer: impl FnMut(&zugangsphase::Dialog) -> Option<zugangsphase::Eingabe>,
    ) -> (Arc<Mutex<Shared>>, Vec<zugangsphase::Dialog>) {
        secure::test_identitaet();
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr.to_string()),
            ziel_id,
            ziel_name: Some("aus der Bekanntgabe".into()),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        let mut gesehen: Vec<zugangsphase::Dialog> = Vec::new();
        let t0 = Instant::now();
        while shared.lock().unwrap().target.is_some() && t0.elapsed() < Duration::from_secs(20) {
            {
                let mut s = shared.lock().unwrap();
                if let Some(d) = s.zugang.clone() {
                    if gesehen.last() != Some(&d) {
                        gesehen.push(d.clone());
                    }
                    if s.zugang_eingabe.is_none() {
                        if let Some(e) = nutzer(&d) {
                            if e == zugangsphase::Eingabe::Abbrechen {
                                s.target = None;
                            }
                            s.zugang_eingabe = Some(e);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(shared.lock().unwrap().target.is_none(), "Ziel nach 20 s noch da");
        (shared, gesehen)
    }

    /// Der Eintrag dieses Hosts in hosts.txt, falls gepinnt.
    fn gepinnt(host_pub: &[u8]) -> Option<zugang::BekannterHost> {
        let p = zugang::ablage_pfad(zugang::HOSTS_DATEI).unwrap();
        zugang::Hostliste::laden(&p).unwrap().nach_schluessel(host_pub).cloned()
    }

    fn fehler_von(s: &Arc<Mutex<Shared>>) -> Option<Meldung> {
        s.lock().unwrap().error.clone()
    }

    /// Bekannt ("QCH1") und hier gepinnt: Sitzung ohne Dialog, danach der
    /// Eintrag erneuert - mit ID, Adresse und dem Namen aus der Bekanntgabe.
    /// Mit der richtigen ID als Ziel geht es ebenso.
    #[test]
    fn zugang_bekannter_host_wird_gepinnt() {
        let (addr, host_pub, _ereignisse, verbindungen) = zugangshost(Zugangshost::Bekannt, "");
        vorher_pinnen(&host_pub, "127.0.0.1:2");
        assert_eq!(gepinnt(&host_pub).map(|h| h.name), Some("Vorher".into()));
        let (s, dialoge) = zugang_durchspielen(&addr, Some(zugang::geraete_id(&host_pub)), |_| None);
        assert!(dialoge.is_empty());
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert!(s.lock().unwrap().hosts_stand >= 1);
        let h = gepinnt(&host_pub).expect("nicht gepinnt");
        assert_eq!((h.id, h.adresse.as_str(), h.name.as_str()), (zugang::geraete_id(&host_pub), addr.as_str(), "aus der Bekanntgabe"));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1);
    }

    /// Erstkontakt (1.4, 3.5): der Client kennt den Schluessel des Hosts
    /// nicht und setzt Bit 0 - der Host sagt trotzdem gleich "QCH1", ohne
    /// sich mit Passwort oder "Zulassen" auszuweisen. Abbruch mit
    /// MsgHostUnverified, nichts gepinnt, kein Neuversuch.
    #[test]
    fn zugang_host_ohne_ausweis_wird_nicht_angenommen() {
        let (addr, host_pub, _, verbindungen) = zugangshost(Zugangshost::Bekannt, "");
        let (s, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(dialoge.is_empty());
        let m = fehler_von(&s).expect("keine Meldung");
        assert_eq!(m.key, strings::Key::MsgHostUnverified, "{}", m.protokoll);
        assert_eq!(
            m.text(&strings::EN),
            "aus der Bekanntgabe did not prove its identity. The connection was stopped for safety."
        );
        assert!(m.dauerhaft());
        assert!(m.protokoll.contains("Bit 0"), "{}", m.protokoll);
        assert!(!s.lock().unwrap().connected);
        assert!(gepinnt(&host_pub).is_none());
        std::thread::sleep(Duration::from_millis(2300));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
    }

    /// Der Host kennt diesen Client, der Client ihn aber nicht (etwa
    /// hosts.txt geloescht): Bit 0 in Nachricht 3, also die Zugangsphase
    /// trotz bekanntem Geraet - mit Passwort und host_proof, danach
    /// gepinnt. Beim Wiederverbinden ist er gepinnt: kein Bit 0, gleich
    /// "QCH1", kein Dialog. Wieder vergessen: wieder Bit 0 und Passwort.
    #[test]
    fn zugang_bekannter_client_verlangt_ausweis_des_hosts() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::KenntClient, "k7m-4wq-9tz");
        let passwort = |d: &zugangsphase::Dialog| {
            (d.lage == zugangsphase::Lage::Eingabe).then(|| zugangsphase::Eingabe::Passwort("k7m-4wq-9tz".into()))
        };
        let (s, dialoge) = zugang_durchspielen(&addr, None, passwort);
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["Bit 0", "richtig"]);
        assert_eq!((dialoge[0].name.as_str(), dialoge[0].zulassen), ("Testhost", false));
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert_eq!(gepinnt(&host_pub).map(|h| h.name), Some("Testhost".into()));

        let (s, dialoge) = zugang_durchspielen(&addr, None, passwort);
        assert!(dialoge.is_empty(), "{dialoge:?}");
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["ohne Bit 0"]);
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));

        let p = zugang::ablage_pfad(zugang::HOSTS_DATEI).unwrap();
        let k: [u8; 32] = host_pub.clone().try_into().unwrap();
        zugang::host_vergessen(&p, &k).unwrap();
        assert!(gepinnt(&host_pub).is_none());
        let (s, dialoge) = zugang_durchspielen(&addr, None, passwort);
        assert!(!dialoge.is_empty());
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["Bit 0", "richtig"]);
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert!(gepinnt(&host_pub).is_some());
        assert_eq!(verbindungen.load(Ordering::SeqCst), 3);
    }

    /// 22/1, obwohl Nachricht 20 kein "Zulassen" anbot (Bit 1 aus), und ohne
    /// dass ein Beweis hinausging: ein Protokollfehler - nichts gepinnt, die
    /// Meldung bleibt, kein Neuversuch.
    #[test]
    fn zugang_zulassen_ungefragt_ist_protokollfehler() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::ZulassenUngefragt, "");
        let (s, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(!dialoge.is_empty() && !dialoge[0].zulassen);
        let m = fehler_von(&s).expect("keine Meldung");
        assert_eq!(m.key, strings::Key::ErrorProtocol, "{}", m.protokoll);
        assert!(m.protokoll.contains("Ergebnis 1"), "{}", m.protokoll);
        assert!(m.dauerhaft());
        assert!(gepinnt(&host_pub).is_none());
        assert_eq!(ereignisse.try_iter().count(), 0, "ein Beweis ging hinaus");
        std::thread::sleep(Duration::from_millis(2300));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
    }

    /// Unbekannt, erst ein falsches, dann das richtige Passwort: der Dialog
    /// zeigt "falsch", der Host prueft beide Beweise, sein host_proof stimmt
    /// - gepinnt mit dem Namen aus Nachricht 20.
    #[test]
    fn zugang_passwort_falsch_dann_richtig() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::Passwort, "k7m-4wq-9tz");
        let mut runde_gesendet = None;
        let (s, dialoge) = zugang_durchspielen(&addr, None, |d| {
            if d.lage == zugangsphase::Lage::Pruefen || runde_gesendet == Some(d.runde) {
                return None;
            }
            runde_gesendet = Some(d.runde);
            let pw = if d.runde == 0 { "k7m-4wq-9ty" } else { "K7M 4WQ 9TZ" };
            Some(zugangsphase::Eingabe::Passwort(pw.into()))
        });
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["falsch", "richtig"]);
        let erster = &dialoge[0];
        assert_eq!((erster.name.as_str(), erster.zulassen, erster.lage), ("Testhost", false, zugangsphase::Lage::Eingabe));
        assert_eq!(erster.id, zugang::geraete_id(&host_pub));
        assert!(dialoge.iter().any(|d| d.lage == zugangsphase::Lage::Falsch && d.runde == 1));
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert_eq!(gepinnt(&host_pub).map(|h| h.name), Some("Testhost".into()));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1);
    }

    /// "Zulassen" am Host, ohne dass der Nutzer etwas eingibt: kein Beweis
    /// geht hinaus, der Dialog zeigt den Vergleichscode des Handschlags, und
    /// der Host wird gepinnt (Erstkontakt ohne Beweis).
    #[test]
    fn zugang_zulassen_ohne_beweis() {
        let (addr, host_pub, ereignisse, _) = zugangshost(Zugangshost::Zulassen, "");
        let (s, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(dialoge[0].zulassen);
        assert_eq!(Some(dialoge[0].code.clone()), s.lock().unwrap().sas.clone());
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert!(gepinnt(&host_pub).is_some());
        assert_eq!(ereignisse.try_iter().count(), 0, "ein Beweis ging hinaus");
    }

    /// Ergebnis 0 mit falschem host_proof: Abbruch, nichts gepinnt, Meldung
    /// "konnte das Passwort nicht bestaetigen", kein Neuversuch.
    #[test]
    fn zugang_falscher_host_beweis_pinnt_nicht() {
        let (addr, host_pub, _, verbindungen) = zugangshost(Zugangshost::FalscherBeweis, "geheim123");
        let (s, _) = zugang_durchspielen(&addr, None, |d| {
            (d.lage == zugangsphase::Lage::Eingabe).then(|| zugangsphase::Eingabe::Passwort("geheim123".into()))
        });
        let m = fehler_von(&s).expect("keine Meldung");
        assert_eq!(m.key, strings::Key::MsgHostProofBad);
        assert_eq!(m.text(&strings::EN), "Testhost could not confirm the password. The connection was stopped for safety.");
        assert!(gepinnt(&host_pub).is_none());
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
    }

    /// Abgelehnt, Frist, aelterer Host, schweigender Host, kein Platz frei
    /// (22/4 statt 20), fremde Kennung: je eine Meldung, die bleibt, nichts
    /// gepinnt, kein Neuversuch.
    #[test]
    fn zugang_meldungen_ohne_neuversuch() {
        use strings::Key::*;
        for (art, key, en) in [
            (Zugangshost::Ablehnen, MsgRefused, "Testhost refused the connection."),
            (Zugangshost::Frist, MsgNoAnswer, "No answer from Testhost. Please try again."),
            (Zugangshost::Alt, MsgHostOutdated, "aus der Bekanntgabe uses an older QuadChroma version. Please update it there."),
            (Zugangshost::Stumm, MsgNoAnswer, "No answer from aus der Bekanntgabe. Please try again."),
            (Zugangshost::Voll, MsgTooManyAttempts, "Too many attempts. Please wait a moment and try again."),
            (Zugangshost::Fremd, ErrorProtocol, "The other side speaks a different protocol"),
        ] {
            let (addr, host_pub, _, verbindungen) = zugangshost(art, "");
            let (s, _) = zugang_durchspielen(&addr, None, |_| None);
            let m = fehler_von(&s).unwrap_or_else(|| panic!("{art:?}: keine Meldung"));
            assert_eq!(m.key, key, "{art:?}: {}", m.protokoll);
            assert_eq!(m.text(&strings::EN), en, "{art:?}");
            assert!(m.dauerhaft());
            assert!(gepinnt(&host_pub).is_none(), "{art:?}");
            std::thread::sleep(Duration::from_millis(2300));
            assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "{art:?}: neu verbunden");
        }
    }

    /// Drossel (Spezifikation 11, "falsches Passwort (Drossel)"): 20 mit
    /// Wartezeit und "falsch" mit Wartezeit. Der Nutzer tippt jedes Mal
    /// sofort - der Client haelt den Beweis zurueck, bis die Wartezeit um
    /// ist (ein Beweis davor zaehlte beim Host als Fehlversuch); der Dialog
    /// zeigt sie.
    #[test]
    fn zugang_drossel_haelt_den_beweis_zurueck() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::Drossel, "k7m-4wq-9tz");
        let mut runde_gesendet = None;
        let (s, dialoge) = zugang_durchspielen(&addr, None, |d| {
            if d.lage == zugangsphase::Lage::Pruefen || runde_gesendet == Some(d.runde) {
                return None;
            }
            runde_gesendet = Some(d.runde);
            let pw = if d.runde == 0 { "falsch-123" } else { "k7m-4wq-9tz" };
            Some(zugangsphase::Eingabe::Passwort(pw.into()))
        });
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["falsch", "richtig"], "Beweis vor Ablauf der Drossel");
        assert!(dialoge[0].frei_ab.is_some(), "Wartezeit aus Nachricht 20 nicht im Dialog");
        assert!(dialoge.iter().any(|d| d.lage == zugangsphase::Lage::Falsch && d.frei_ab.is_some()));
        assert_eq!(s.lock().unwrap().error_key, Some(strings::Key::SessionTakenOver));
        assert!(gepinnt(&host_pub).is_some());
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1);
    }

    /// Ein falscher Host bietet kein "Zulassen" an, nimmt den Beweis und sagt
    /// 22/1 statt 22/0 mit host_proof: kein "angenommen" - Meldung "konnte
    /// das Passwort nicht bestaetigen", nichts gepinnt, kein Neuversuch.
    #[test]
    fn zugang_zulassen_ohne_angebot_pinnt_nicht() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::ZulassenOhneAngebot, "");
        let (s, _) = zugang_durchspielen(&addr, None, |d| {
            (d.lage == zugangsphase::Lage::Eingabe).then(|| zugangsphase::Eingabe::Passwort("geheim123".into()))
        });
        assert_eq!(ereignisse.try_iter().collect::<Vec<_>>(), vec!["Beweis"]);
        assert_eq!(fehler_von(&s).map(|m| m.key), Some(strings::Key::MsgHostProofBad));
        assert!(gepinnt(&host_pub).is_none());
        std::thread::sleep(Duration::from_millis(2300));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1, "neu verbunden");
    }

    /// Am Host entfernt: die laufende Sitzung reisst ab, beim Wiederverbinden
    /// sagt der Host "QCA1". Der Client stellt keine Anfrage von selbst -
    /// kein Dialog, Nachricht 23 sofort, Meldung "kennt dieses Geraet nicht
    /// mehr", die bleibt.
    #[test]
    fn zugang_nach_entfernen_keine_anfrage_von_selbst() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::Entfernt, "");
        vorher_pinnen(&host_pub, &addr);
        let (s, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(dialoge.is_empty(), "Zugangsdialog ohne Nutzer: {dialoge:?}");
        assert_eq!(ereignisse.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("Abbruch"));
        let m = fehler_von(&s).expect("keine Meldung");
        assert_eq!(m.key, strings::Key::MsgDeviceRemoved, "{}", m.protokoll);
        let t = m.text(&strings::DE);
        assert!(t.contains(" kennt dieses Gerät nicht mehr. ") && !t.contains('{'), "{t}");
        assert!(gepinnt(&host_pub).is_some(), "die erste Sitzung war angenommen");
        std::thread::sleep(Duration::from_millis(2300));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 2);
    }

    /// Wiederverbinden nach einer angenommenen Sitzung, und der Host macht
    /// einmal nach dem Handschlag zu (etwa weil seine Aufnahme gerade nicht
    /// startet): das ist kein alter Host - er hat eben noch "QCH1" gesagt.
    /// Keine dauerhafte Meldung "bitte aktualisieren", sondern ein neuer
    /// Versuch nach 2 s, und der kommt wieder herein.
    #[test]
    fn zugang_aussetzer_nach_sitzung_ist_kein_alter_host() {
        let (addr, host_pub, _, verbindungen) = zugangshost(Zugangshost::Aussetzer, "");
        vorher_pinnen(&host_pub, &addr);
        let (s, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(dialoge.is_empty(), "{dialoge:?}");
        assert_eq!(verbindungen.load(Ordering::SeqCst), 3, "kein Neuversuch nach dem Aussetzer");
        let s = s.lock().unwrap();
        assert_eq!(s.error_key, Some(strings::Key::SessionTakenOver), "{:?}", s.error.as_ref().map(|m| &m.protokoll));
        assert!(gepinnt(&host_pub).is_some());
    }

    /// Eine Leitung, die nach dem Handschlag gestoert ist (hier: ein
    /// Datensatz, der nicht echt ist), ist kein Beleg fuer einen alten Host:
    /// "Verbindung verloren", und der Client versucht es weiter.
    #[test]
    fn zugang_gestoerte_leitung_ist_kein_alter_host() {
        let (addr, host_pub, _, verbindungen) = zugangshost(Zugangshost::Kaputt, "");
        secure::test_identitaet();
        let shared = Arc::new(Mutex::new(Shared {
            decoder_wunsch: einstellungen::DecoderWunsch::Software,
            target: Some(addr.clone()),
            ..Shared::default()
        }));
        let input = Arc::new(Mutex::new(InputLink::new(String::new())));
        {
            let (s, i) = (shared.clone(), input.clone());
            std::thread::spawn(move || stream_thread(s, i));
        }
        // Bis der zweite Versuch laeuft und eine Meldung steht (jeder
        // Versuch loescht sie nach dem Handschlag kurz).
        let t0 = Instant::now();
        let mut s = loop {
            let s = shared.lock().unwrap();
            if (verbindungen.load(Ordering::SeqCst) >= 2 && s.error.is_some()) || t0.elapsed() >= Duration::from_secs(10) {
                break s;
            }
            drop(s);
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(verbindungen.load(Ordering::SeqCst) >= 2, "kein Neuversuch");
        assert_eq!(s.target.as_deref(), Some(addr.as_str()));
        let m = s.error.clone().expect("keine Meldung");
        assert_eq!(m.key, strings::Key::ConnectionLost, "{}", m.protokoll);
        assert!(!m.dauerhaft());
        assert!(gepinnt(&host_pub).is_none());
        s.target = None;
    }

    /// Abbrechen im Dialog: Nachricht 23 kommt beim Host an, keine Meldung,
    /// nichts gepinnt, kein Neuversuch.
    #[test]
    fn zugang_abbruch_sendet_23() {
        let (addr, host_pub, ereignisse, verbindungen) = zugangshost(Zugangshost::Passwort, "x");
        let (s, _) = zugang_durchspielen(&addr, None, |_| Some(zugangsphase::Eingabe::Abbrechen));
        assert_eq!(ereignisse.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("Abbruch"));
        assert_eq!(fehler_von(&s), None);
        assert!(gepinnt(&host_pub).is_none());
        std::thread::sleep(Duration::from_millis(2300));
        assert_eq!(verbindungen.load(Ordering::SeqCst), 1);
    }

    /// 8.3: unter der Adresse war ein anderes Geraet gepinnt - kein
    /// Dauerfehler, sondern die Zugangsphase mit dem Hinweis "neue
    /// Identitaet"; danach findet die Adresse den neuen, der alte Eintrag
    /// bleibt fuer seine ID.
    #[test]
    fn zugang_neue_identitaet_an_bekannter_adresse() {
        let (addr, host_pub, _, _) = zugangshost(Zugangshost::Zulassen, "");
        let alt = [0x77u8; 32];
        let pfad = zugang::ablage_pfad(zugang::HOSTS_DATEI).unwrap();
        zugangsphase::pinnen(&pfad, &alt, &addr, "Frueher").unwrap();
        let (_, dialoge) = zugang_durchspielen(&addr, None, |_| None);
        assert!(dialoge[0].neue_identitaet);
        let liste = zugang::Hostliste::laden(&pfad).unwrap();
        assert_eq!(liste.nach_adresse(&addr).map(|h| h.schluessel.to_vec()), Some(host_pub.clone()));
        assert_eq!(liste.nach_id(zugang::geraete_id(&alt)).map(|h| h.name.as_str()), Some("Frueher"));
    }

    /// Ziele von der Befehlszeile: Adresse, Verknuepfung mit ID, ID allein
    /// (als Argument oder --id) - und --passwort ist nie die Adresse.
    #[test]
    fn ziel_von_der_befehlszeile() {
        assert_eq!(ziel_aus_argumenten(&argumente(&["10.0.0.5"])), ("10.0.0.5:9001".to_string(), None));
        assert_eq!(
            ziel_aus_argumenten(&argumente(&["--verbinden", "10.0.0.5:9101", "--id", "581729911"])),
            ("10.0.0.5:9101".to_string(), Some(581_729_911))
        );
        assert_eq!(ziel_aus_argumenten(&argumente(&["581 729 911"])), (String::new(), Some(581_729_911)));
        assert_eq!(ziel_aus_argumenten(&argumente(&["--id", "000000005"])), (String::new(), Some(5)));
        assert_eq!(ziel_aus_argumenten(&argumente(&["--headless", "--passwort", "geheim", "10.0.0.5"])), ("10.0.0.5:9001".to_string(), None));
        assert_eq!(ziel_aus_argumenten(&argumente(&["--headless", "--passwort", "geheim"])), (String::new(), None));
        // Weitergabe an die laufende App (einzel.rs) hin und zurueck.
        for (a, i) in [("10.0.0.5:9001", None), ("10.0.0.5:9001", Some(581_729_911)), ("", Some(5)), ("host#1", None)] {
            assert_eq!(ziel_lesen(&ziel_text(a, i)), (a.to_string(), i), "{a} {i:?}");
        }
        assert_eq!(ziel_text("h:1", Some(5)), "h:1#000000005");
    }

    fn gefunden(name: &str, addr: &str, id: Option<u32>) -> discovery::Gefunden {
        discovery::Gefunden {
            host: discovery::Host { name: name.into(), addr: addr.parse().unwrap(), seen: Instant::now() },
            id,
            flags: 0,
        }
    }

    /// 9.2: eine ID sucht zuerst im Netz, dann die letzte Adresse aus
    /// hosts.txt; sonst "nicht gefunden". Eine Adresse bleibt eine Adresse
    /// (ohne ID - 8.3), mit dem Namen, unter dem sie sich meldet.
    #[test]
    fn ziel_suche_ueber_id() {
        let mut bekannte = zugang::Hostliste::default();
        let k = [0x42u8; 32];
        let id_k = zugang::geraete_id(&k);
        bekannte.merken(zugang::BekannterHost::neu(k, "10.0.0.9:9001", "Buero"));
        let netz = vec![gefunden("Mac", "10.0.0.5:9001", Some(581_729_911)), gefunden("Alt", "10.0.0.6:9001", None)];
        let z = ziel_aus_eingabe("581 729 911", &netz, &bekannte).unwrap();
        assert_eq!(z, Ziel { adresse: "10.0.0.5:9001".into(), id: Some(581_729_911), name: Some("Mac".into()) });
        let z = ziel_aus_eingabe(&zugang::id_ziffern(id_k), &netz, &bekannte).unwrap();
        assert_eq!(z, Ziel { adresse: "10.0.0.9:9001".into(), id: Some(id_k), name: Some("Buero".into()) });
        let m = ziel_aus_eingabe("123-456-789", &netz, &bekannte).unwrap_err();
        assert_eq!(m.key, strings::Key::MsgIdNotFound);
        assert_eq!(m.text(&strings::EN), "No device with ID 123 456 789 found in the network.");
        assert!(m.dauerhaft());
        let z = ziel_aus_eingabe("10.0.0.6", &netz, &bekannte).unwrap();
        assert_eq!(z, Ziel { adresse: "10.0.0.6:9001".into(), id: None, name: Some("Alt".into()) });
        // Verknuepfung: Adresse und ID; meldet sich die ID anderswo, gilt das.
        let z = ziel_bilden("10.0.0.1:9001", Some(581_729_911), &netz, &bekannte).unwrap();
        assert_eq!(z.adresse, "10.0.0.5:9001");
        let z = ziel_bilden("10.0.0.1:9001", Some(7), &netz, &bekannte).unwrap();
        assert_eq!((z.adresse.as_str(), z.id), ("10.0.0.1:9001", Some(7)));
    }

    /// 9.1: je Host Name, ID (ohne Erweiterung keine) und der Haken fuer
    /// bekannte - ueber die ID, bei Hosts ohne ID ueber die Adresse.
    #[test]
    fn hostzeilen_mit_id_und_haken() {
        let mut bekannte = zugang::Hostliste::default();
        let k = [0x43u8; 32];
        bekannte.merken(zugang::BekannterHost::neu(k, "10.0.0.6:9001", "Alt"));
        let netz = vec![
            gefunden("Mac", "10.0.0.5:9001", Some(zugang::geraete_id(&k))),
            gefunden("", "10.0.0.6:9001", None),
            gefunden("Fremd", "10.0.0.7:9001", Some(5)),
        ];
        let z = hostzeilen(&netz, &bekannte);
        assert_eq!(z.iter().map(|z| z.bekannt).collect::<Vec<_>>(), vec![true, true, false]);
        assert_eq!(z[1].name, "10.0.0.6:9001");
        assert_eq!(z[2].id, Some(5));
    }

    /// Die Texte des Zugangs: Platzhalter gefuellt, Meldungen bleiben (kein
    /// Neuversuch, 9.6), und EN wie DE vorhanden.
    #[test]
    fn zugangstexte_und_dauer() {
        use strings::Key::*;
        for k in [
            MsgRefused,
            MsgNoAnswer,
            MsgTooManyAttempts,
            MsgHostOutdated,
            MsgHostProofBad,
            MsgHostUnverified,
            MsgOtherDevice,
            MsgIdNotFound,
            MsgDeviceRemoved,
        ] {
            let m = Meldung::neu(k, "x").mit("{n}", "Mac").mit("{i}", "581 729 911");
            assert!(m.dauerhaft(), "{k:?}");
            for lang in strings::all() {
                assert!(!m.text(lang).contains('{'), "{k:?} ({})", lang.code);
            }
            assert_ne!(strings::EN.get(k), strings::DE.get(k), "{k:?}");
        }
        assert_eq!(strings::pick("de").get(MsgHostOutdated).replace("{n}", "Mac"), "Mac verwendet eine ältere QuadChroma-Version. Bitte dort aktualisieren.");
    }

    /// W7: der allererste Start oeffnet das Fenster, jeder spaetere bleibt im
    /// Infobereich; Autostart und --host immer im Hintergrund; mit einem Ziel
    /// oder ohne Symbol (tray=aus) immer mit Fenster; auf dem Mac (noch keine
    /// eine App) immer mit Fenster.
    #[test]
    fn erster_start_mit_fenster_spaetere_im_hintergrund() {
        assert!(!start_im_hintergrund(true, false, true, false, false), "allererster Start ohne Fenster");
        assert!(start_im_hintergrund(true, false, true, false, true), "spaeterer Start mit Fenster");
        assert!(start_im_hintergrund(true, false, true, true, false), "Autostart vor dem ersten Fenster mit Fenster");
        assert!(start_im_hintergrund(true, false, true, true, true));
        assert!(!start_im_hintergrund(true, true, true, true, true), "Start mit Ziel ohne Fenster");
        assert!(!start_im_hintergrund(true, false, false, true, true), "tray=aus ohne Fenster");
        assert!(!start_im_hintergrund(false, false, true, true, true), "ohne die eine App im Hintergrund");
    }

    /// Ein Decoderbild aus eigenen Ebenen, fuer `to_rgb` ohne Decoder.
    struct Ebenenprobe {
        fmt: EbenenFormat,
        w: u32,
        h: u32,
        ebenen: Vec<Vec<u8>>,
        zeilen: Vec<usize>,
    }

    impl Ebenenbild for Ebenenprobe {
        fn breite(&self) -> u32 {
            self.w
        }
        fn hoehe(&self) -> u32 {
            self.h
        }
        fn zeitstempel(&self) -> Option<i64> {
            None
        }
        fn ebenen(&self) -> Option<EbenenFormat> {
            Some(self.fmt)
        }
        fn format_name(&self) -> String {
            "Probe".into()
        }
        fn ebenenzahl(&self) -> usize {
            self.ebenen.len()
        }
        fn daten(&self, ebene: usize) -> &[u8] {
            &self.ebenen[ebene]
        }
        fn zeilenlaenge(&self, ebene: usize) -> usize {
            self.zeilen[ebene]
        }
    }

    /// Begrenzter Wertebereich (nur VideoToolbox: 420v, x420, 444v, x444):
    /// Y 16 wird Schwarz, 235 Weiss, dazwischen gerundet gedehnt; der volle
    /// Bereich rechnet wie bisher (16 bleibt 16). 8 und 16 Bit (oben
    /// buendig) ergeben dasselbe.
    #[test]
    fn begrenzter_bereich_wird_gedehnt() {
        let probe = |begrenzt: bool, bits: u8, y: [u16; 4], uv: [u16; 2]| -> Vec<u32> {
            let (bpp, sch) = if bits == 8 { (1, 0) } else { (2, 8) };
            let bytes = |w: &[u16]| -> Vec<u8> {
                w.iter().flat_map(|&v| if bpp == 1 { vec![v as u8] } else { (v << sch).to_le_bytes().to_vec() }).collect()
            };
            let p = Ebenenprobe {
                fmt: EbenenFormat { sub: true, bits, paar: true, begrenzt },
                w: 2,
                h: 2,
                ebenen: vec![bytes(&y), bytes(&uv)],
                zeilen: vec![2 * bpp, 2 * bpp],
            };
            to_rgb(&p).expect("to_rgb").pixels
        };
        for bits in [8u8, 16] {
            assert_eq!(probe(true, bits, [16, 235, 126, 16], [128, 128]), vec![0, 0xffffff, 0x808080, 0], "{bits} Bit");
            assert_eq!(probe(false, bits, [16, 235, 126, 16], [128, 128]), vec![0x101010, 0xebebeb, 0x7e7e7e, 0x101010], "{bits} Bit");
            // Cr 240 ist der Rand des begrenzten Bereichs: gedehnt 128 statt
            // 112 ueber dem Nullpunkt (Y 81 -> 76; R 76 + 201, abgeschnitten;
            // G 76 - 59; B 76).
            assert_eq!(probe(true, bits, [81, 81, 81, 81], [128, 240]), vec![0xff114c; 4], "{bits} Bit");
        }
    }
}
