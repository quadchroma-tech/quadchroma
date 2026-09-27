// Zwischenablage auf der Windows-Seite.
//
// Dieselbe Datei dient zwei Rollen derselben Programmdatei: dem
// Windows-Client und der Windows-Host-Rolle (host/mod.rs) - auch beiden
// zugleich in einem Prozess. Jede meldet sich mit ihrer Herkunft an
// (watch); es gibt aber nur EINEN Waechter mit EINEM Nachrichtenfenster, und
// er verteilt, was der Benutzer hier kopiert hat, an jede Rolle, die gerade
// ein Gegenueber hat (ablage_ruhe::Verteiler). set() und set_dateien() legen
// ab, was von der Gegenseite einer Rolle kommt, ohne die Ueberwachung
// auszuloesen - fuer keine der beiden Rollen: Abgelegtes geht weder zurueck
// noch an die andere Rolle weiter.
//
// Kanaele (Nachrichtentyp 48, UTF-8, siehe protokoll_konst.rs):
//   Client -> Host : IN_CLIP auf dem Eingabekanal (Port des Hosts + 1, 9002).
//   Host -> Client : MSG_CLIP auf dem Bildkanal (Port des Hosts, 9001).
// Im Client geht, was watch() meldet, also ueber den Eingabekanal hinaus und
// set() bekommt, was der Bildkanal bringt; in der Host-Rolle umgekehrt.
//
// Inhalt ist Text oder eine Dateiliste (CF_HDROP, etwa "Kopieren" im
// Explorer). Hier werden nur die Pfade gelesen bzw. abgelegt, nie Dateien:
// die Dateien selbst uebertraegt ein eigener Teil (dateien.rs, Nachrichten
// 50-53). set_dateien() legt eine empfangene Dateiliste so ab, dass
// "Einfuegen" im Explorer die Dateien kopiert.
//
// Das Melden laeuft ueber AddClipboardFormatListener und ein reines
// Nachrichtenfenster; Windows schickt dann WM_CLIPBOARDUPDATE. Kein Abfragen im
// Takt, kein SetClipboardViewer (dessen Kette bricht, sobald ein Glied abstuerzt).
//
// Gelesen wird nicht bei WM_CLIPBOARDUPDATE selbst, sondern erst, wenn die
// Ablage ablage_ruhe::RUHE lang ruhig war (SetTimer im Faden des Waechters,
// jede weitere Meldung schiebt ihn hinaus): eine Kopie loest oft mehrere
// Meldungen aus, und wer bei der ersten schon oeffnet, haelt die Ablage fest,
// waehrend die Quelle noch in OleFlushClipboard steckt - andere Programme
// scheitern dann kurz. Dieselbe Dateiliste geht innerhalb von
// ablage_ruhe::DOPPEL_FRIST nur einmal hinaus (Integrationstest, Befunde 1
// und 2).
//
// Gelesen wird nur mit Gegenueber ("kein Zuschauer, keine Arbeit"), wie auf
// dem Mac (clipboard_mac.rs): im Client waehrend einer Sitzung (`sitzung`),
// in der Host-Rolle, solange ein Zuschauer da ist - jede Rolle sagt das mit
// ihrer Anmeldung selbst (watch, `gegenueber`). Hat keine ein Gegenueber,
// wird die Ablage nicht einmal geoeffnet - sonst stuende auch ohne jede
// Verbindung jede Passwortkopie als Zeile "verdeckt" im Protokoll. Was davor
// kopiert wurde, geht beim Sitzungsbeginn nicht nachtraeglich hinaus. Die
// Zeilen des Waechters gehen ins Protokoll der Rollen, fuer die gelesen
// wurde (protokoll::zeile_als).
//
// Benoetigte Abhaengigkeit in Cargo.toml (Lizenz MIT OR Apache-2.0):
//
//   [target.'cfg(windows)'.dependencies]
//   windows = { version = "0.62", features = [
//       "Win32_Foundation",
//       "Win32_System_DataExchange",
//       "Win32_System_LibraryLoader",
//       "Win32_System_Memory",
//       "Win32_System_Ole",
//       "Win32_UI_Shell",                // DragQueryFileW, DROPFILES, HDROP
//       "Win32_UI_WindowsAndMessaging",
//       "Win32_System_Threading",        // nur die Tests: benannte Sperre
//   ] }
//
// Alle verwendeten Signaturen sind gegen die Bindungen von windows 0.62.2
// geprueft (microsoft.github.io/windows-docs-rs).

use std::cell::RefCell;
use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

pub use crate::ablage_ruhe::Inhalt;
use crate::ablage_ruhe::{Entpreller, Verteilt, Verteiler, RUHE};
use crate::protokoll::Herkunft;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, RegisterClassW,
    SetTimer, TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_TIMER,
    WNDCLASSW,
};

/// Obergrenze fuer einen Uebertragungsvorgang. Groesseres wird stillschweigend
/// verworfen statt abgeschnitten: ein halber Text ist schlimmer als keiner.
/// Gilt auch fuer den Block einer abgelegten Dateiliste.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Versuche, die Ablage zu bekommen: fuer Text und das Lesen des Waechters
/// kurz (ein verlorener Text ist billig, und der Waechter soll nicht haengen),
/// fuer eine empfangene Dateiliste lang - scheitert sie, verwirft der
/// Empfaenger eine vollstaendig uebertragene Sendung (bis 4 GB). Das Warten
/// blockiert dort nur den Schreibfaden des Empfaengers (Durchsicht [15]).
#[derive(Clone, Copy, Debug)]
struct Versuche {
    anzahl: u32,
    abstand: Duration,
}

/// Text ablegen und lesen: zehn Versuche im Abstand von 10 ms.
const VERSUCHE_KURZ: Versuche = Versuche { anzahl: 10, abstand: Duration::from_millis(10) };
/// Dateiliste ablegen: vierzig Versuche im Abstand von 50 ms, rund 2 s.
const VERSUCHE_DATEIEN: Versuche = Versuche { anzahl: 40, abstand: Duration::from_millis(50) };

/// Kennung des Zeitgebers, mit dem der Waechter auf Ruhe wartet.
const ZEITGEBER_RUHE: usize = 1;

/// Groesse des Kopfes DROPFILES (packed(1)): pFiles, pt.x, pt.y, fNC, fWide.
const DROPFILES_GROESSE: usize = 20;
const _: () = assert!(std::mem::size_of::<DROPFILES>() == DROPFILES_GROESSE);

/// Format, aus dem Explorer liest, ob "Einfuegen" kopiert oder verschiebt
/// (CFSTR_PREFERREDDROPEFFECT). Wir legen immer DROPEFFECT_COPY ab: die
/// Dateien im Uebertragungsverzeichnis bleiben, wo sie sind.
const BEVORZUGTE_WIRKUNG: &str = "Preferred DropEffect";

/// Laufnummern unserer eigenen Schreibvorgaenge, die letzten EIGENE_MAX.
/// Je Vorgang zwei Werte, weil nicht sicher belegt ist, ob Windows die
/// Nummer schon beim SetClipboardData hochzaehlt oder erst beim
/// CloseClipboard; wir merken uns beide und lassen beide durchfallen. Mehr
/// als ein Paar, weil zwei Rollen in einem Prozess fast zugleich ablegen
/// koennen: der eine Vorgang merkt seine zweite Nummer womoeglich erst,
/// nachdem der andere schon fertig ist - mit nur einem Paar ueberschriebe
/// er dessen Nummern. Gilt fuer Text und Dateilisten gleichermassen
/// (write_locked).
const EIGENE_MAX: usize = 8;

struct Eigene {
    nummern: [u32; EIGENE_MAX],
    naechste: usize,
}

static EIGENE: Mutex<Eigene> = Mutex::new(Eigene { nummern: [0; EIGENE_MAX], naechste: 0 });

/// Eine Laufnummer als eigene merken (0 heisst "nicht lesbar" und zaehlt nicht).
fn eigene_merken(seq: u32) {
    if seq == 0 {
        return;
    }
    let mut e = EIGENE.lock().unwrap_or_else(|e| e.into_inner());
    let i = e.naechste;
    e.nummern[i] = seq;
    e.naechste = (i + 1) % EIGENE_MAX;
}

/// Stammt diese Laufnummer von einem unserer Schreibvorgaenge?
fn eigene_nummer(seq: u32) -> bool {
    seq != 0 && EIGENE.lock().unwrap_or_else(|e| e.into_inner()).nummern.contains(&seq)
}

/// Griff auf das Nachrichtenfenster des Ueberwachungsfadens (des einen
/// Waechters beider Rollen). Zum Schreiben
/// zwingend noetig: oeffnet man die Ablage ohne Besitzerfenster, dann setzt
/// EmptyClipboard den Besitzer auf NULL und SetClipboardData scheitert danach
/// (so dokumentiert bei EmptyClipboard). Die Ablage waere geleert und der neue
/// Inhalt nie angekommen.
///
/// UNGEPRUEFT: set() laeuft in der Regel in einem anderen Faden als der, der
/// dieses Fenster angelegt hat. Microsoft verbietet einen fadenfremden Griff bei
/// OpenClipboard nirgends und der Besitzer bekommt von uns nur
/// WM_DESTROYCLIPBOARD, das DefWindowProcW erledigt - ausdruecklich erlaubt ist
/// es aber auch nicht. Scheitert es doch, faellt set() still aus, ohne die
/// Ablage vorher zu leeren.
static OWNER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// Laeuft eine Sitzung des Clients? Setzt der Empfangsfaden (main.rs).
static SITZUNG: AtomicBool = AtomicBool::new(false);

/// Zaehlt jeden Sitzungswechsel des Clients: dieselbe Dateiliste in einer
/// neuen Sitzung ist eine neue Kopie (Entdoppler).
static SITZUNGS_WECHSEL: AtomicU64 = AtomicU64::new(0);

/// Die angemeldeten Rollen des einen Waechters.
static VERTEILER: Verteiler = Verteiler::neu();

/// Der Waechterfaden wird nur einmal gestartet, gleich wie viele Rollen
/// sich anmelden.
static WAECHTER: Once = Once::new();

/// Sitzung des Clients beginnt (true, der Host hat angenommen) oder endet.
pub fn sitzung(an: bool) {
    if SITZUNG.swap(an, Ordering::Relaxed) != an {
        SITZUNGS_WECHSEL.fetch_add(1, Ordering::Relaxed);
    }
}

/// Das Gegenueber des Clients fuer seine Anmeldung (watch): Some(Nummer des
/// Sitzungswechsels) waehrend einer Sitzung, sonst None.
pub fn client_gegenueber() -> Option<u64> {
    SITZUNG.load(Ordering::Relaxed).then(|| SITZUNGS_WECHSEL.load(Ordering::Relaxed))
}

/// Darf der Waechter jetzt lesen? Wenn irgendeine angemeldete Rolle ein
/// Gegenueber hat (der Client eine Sitzung, die Host-Rolle einen Zuschauer).
fn darf_lesen() -> bool {
    // Tests des Waechterfadens geben das Gegenueber je Faden vor, statt die
    // Sitzung aller Tests zu kippen.
    #[cfg(test)]
    if let Some(d) = tests::GEGENUEBER.with(|g| g.get()) {
        return d;
    }
    VERTEILER.jemand_da()
}

/// Eine Zeile des Waechters: ins Protokoll jeder Rolle, fuer die gerade
/// gelesen wird; ohne eine solche (Tests) in das des Fadens.
fn waechter_zeile(text: &str) {
    let rollen = VERTEILER.mit_gegenueber();
    if rollen.is_empty() {
        crate::protokoll::zeile(text.to_string());
    }
    for h in rollen {
        crate::protokoll::zeile_als(h, text.to_string());
    }
}

thread_local! {
    /// Entprellen der Meldungen (ablage_ruhe.rs), nur im Faden des
    /// Waechters - die Fensterprozedur laeuft genau dort und nur dort.
    /// Entdoppelt wird je Rolle beim Verteilen (ablage_ruhe::Verteiler).
    static RUHE_STAND: RefCell<Entpreller> = RefCell::new(Entpreller::neu(RUHE));
}

// ------------------------------------------------------------ Hilfsmittel

/// Rust-Zeichenkette als nullterminierte UTF-16-Folge. Der Puffer muss leben,
/// solange der daraus gebildete PCWSTR benutzt wird.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Schliesst die Zwischenablage, sobald der Geltungsbereich endet. Ohne das
/// bleibt sie bei jedem fruehen Rueckweg fuer das ganze System gesperrt.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Die Zwischenablage haelt immer nur ein Prozess zugleich; ein Fehlschlag ist
/// der Normalfall, kein Grund zur Aufregung. `versuche.anzahl` Versuche im
/// Abstand von `versuche.abstand` (siehe Versuche), danach geben wir leise auf.
///
/// `owner` darf zum reinen Lesen None sein. Zum Schreiben muss ein Fenster
/// angegeben werden, siehe OWNER.
fn open_clipboard(owner: Option<HWND>, versuche: Versuche) -> Option<ClipboardGuard> {
    for versuch in 0..versuche.anzahl {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Some(ClipboardGuard);
        }
        if versuch + 1 < versuche.anzahl {
            std::thread::sleep(versuche.abstand);
        }
    }
    None
}

/// Reines Nachrichtenfenster anlegen. Unsichtbar, ohne Rahmen, nur Empfaenger.
fn create_message_window() -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).map_err(|e| format!("Modulkennung: {e}"))?;
        let hinst = HINSTANCE::from(hinst);

        let class = wide("QuadChromaClipboard");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        // Ein zweiter Aufruf scheitert mit "Klasse existiert bereits". Das ist
        // kein Fehler, das Fenster laesst sich trotzdem anlegen.
        RegisterClassW(&wc);

        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(class.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE), // nur Nachrichten, nie sichtbar
            None,
            Some(hinst),
            None,
        )
        .map_err(|e| format!("Nachrichtenfenster: {e}"))
    }
}

// ---------------------------------------------------------------- DROPFILES

/// Baut den Block fuer CF_HDROP: DROPFILES (pFiles = 20, pt = 0, fNC = 0,
/// fWide = TRUE), danach jeder Pfad als UTF-16 mit NUL, am Ende ein weiteres
/// NUL. None bei leerer Liste, bei einem relativen (auch leeren) oder
/// NUL-haltigen Pfad oder wenn der Block MAX_BYTES reisst - dann bleibt die
/// Ablage, wie sie ist. Reine Logik, ohne Ablage.
fn dropfiles_bauen(pfade: &[PathBuf]) -> Option<Vec<u8>> {
    if pfade.is_empty() {
        return None;
    }
    let mut einheiten: Vec<u16> = Vec::new();
    for p in pfade {
        // Explorer versteht nur vollstaendige Pfade; ein relativer hiesse
        // "relativ zu irgendeinem Arbeitsverzeichnis".
        if !p.is_absolute() {
            return None;
        }
        let anfang = einheiten.len();
        einheiten.extend(p.as_os_str().encode_wide());
        if einheiten[anfang..].contains(&0) {
            return None;
        }
        einheiten.push(0);
        if DROPFILES_GROESSE + (einheiten.len() + 1) * 2 > MAX_BYTES {
            return None;
        }
    }
    einheiten.push(0); // Ende der Liste

    let mut block = Vec::with_capacity(DROPFILES_GROESSE + einheiten.len() * 2);
    block.extend_from_slice(&(DROPFILES_GROESSE as u32).to_le_bytes()); // pFiles
    block.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    block.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    block.extend_from_slice(&0i32.to_le_bytes()); // fNC
    block.extend_from_slice(&1i32.to_le_bytes()); // fWide: UTF-16
    for e in einheiten {
        block.extend_from_slice(&e.to_le_bytes());
    }
    Some(block)
}

// ------------------------------------------------------------------- Lesen

/// Kennzeichen, mit denen Programme unter Windows einen Eintrag als
/// vertraulich markieren (Passwortverwalter wie KeePass). Beide heissen: wer
/// die Ablage beobachtet, soll diesen Inhalt nicht verarbeiten - und genau
/// das tut watch(). ExcludeClipboardContentFromMonitorProcessing ist
/// Microsofts Name dafuer, "Clipboard Viewer Ignore" der aeltere, den
/// Ablageverwalter ebenso beachten. Gegenstueck zu
/// org.nspasteboard.ConcealedType auf dem Mac (host/clipboard.m,
/// clipboard_mac.rs): dieselbe Regel auf jeder Rolle, verdeckte Eintraege
/// bleiben auf dem Rechner, auf dem sie kopiert wurden - Text wie Dateien.
///
/// CanIncludeInClipboardHistory = 0 zaehlt bewusst nicht dazu: das sagt nur
/// "nicht in den Verlauf", nicht "vertraulich".
const VERDECKT: [&str; 2] = ["ExcludeClipboardContentFromMonitorProcessing", "Clipboard Viewer Ignore"];

/// Ist der Eintrag als vertraulich markiert? `vorhanden` fragt die Ablage
/// nach einem angemeldeten Format.
fn verdeckt(vorhanden: impl Fn(&str) -> bool) -> bool {
    VERDECKT.iter().any(|name| vorhanden(name))
}

/// Liegt das angemeldete Format `name` in der Ablage? Laesst es sich nicht
/// anmelden, gilt es als vorhanden - im Zweifel wird nichts gelesen.
fn format_vorhanden(name: &str) -> bool {
    let w = wide(name);
    let format = unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) };
    format == 0 || unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// Liest, was in der Ablage liegt: eine Dateiliste (CF_HDROP) oder Text
/// (CF_UNICODETEXT). None, wenn nichts davon anliegt, die Ablage nicht zu
/// bekommen war, der Eintrag als vertraulich markiert ist (siehe VERDECKT)
/// oder der Inhalt die Obergrenze reisst.
///
/// Die Dateiliste hat Vorrang vor Text: Explorer und andere legen neben den
/// Pfaden oft auch Namen als Text ab, und die sollen nicht als Text
/// hinausgehen. Liegt CF_HDROP an, ist aber nicht lesbar, geht gar nichts
/// hinaus.
///
/// Was set() und set_dateien() ablegen, traegt selbst
/// ExcludeClipboardContentFromMonitorProcessing und wird hier also nie
/// gelesen - auch dann nicht, wenn die Laufnummer in on_clipboard_update
/// einmal nicht greift.
///
/// Die Ablage ist nur fuer die Dauer dieses Aufrufs offen; gemeldet wird
/// erst danach (on_clipboard_update). Liegt weder eine Dateiliste noch Text
/// an (etwa ein Bild), wird sie gar nicht erst geoeffnet - fragen, ob ein
/// Format anliegt, geht ohne.
fn lesen() -> Option<Inhalt> {
    let da = |f: u32| unsafe { IsClipboardFormatAvailable(f) }.is_ok();
    if !da(CF_HDROP.0 as u32) && !da(CF_UNICODETEXT.0 as u32) {
        return None;
    }
    // Zum Lesen braucht es kein Besitzerfenster.
    let _guard = open_clipboard(None, VERSUCHE_KURZ)?;

    // Bei offener Ablage fragen, damit Kennzeichen und Inhalt zum selben
    // Eintrag gehoeren.
    if verdeckt(format_vorhanden) {
        waechter_zeile("Zwischenablage: Eintrag ist als verdeckt markiert, nicht uebertragen");
        return None;
    }

    if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok() {
        let pfade = hdrop_pfade();
        if pfade.is_none() {
            waechter_zeile("Zwischenablage: Dateiliste nicht lesbar, nichts uebertragen");
        }
        return pfade.map(Inhalt::Dateien);
    }

    text_lesen().map(Inhalt::Text)
}

/// Text aus der Ablage zum Einfuegen in ein eigenes Feld (Adresse und
/// Passwort, Strg+V). Anders als der Waechter liest es auch ohne Sitzung und
/// auch verdeckte Eintraege: der Nutzer fuegt selbst ein, und der Text
/// bleibt in diesem Programm - er geht nicht zum Host. None: kein Text
/// oder die Ablage war nicht zu bekommen.
pub fn text_einfuegen() -> Option<String> {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_err() {
        return None;
    }
    let _guard = open_clipboard(None, VERSUCHE_KURZ)?;
    text_lesen()
}

/// Die Pfade aus CF_HDROP, ueber DragQueryFileW. Nur bei offener Ablage
/// aufrufen: das Handle gehoert der Ablage und gilt nur, solange sie offen
/// ist. Deshalb auch nie DragFinish darauf - das gaebe einen Block frei, der
/// dem System gehoert. Gelesen werden nur die Pfade, keine Datei, damit die
/// Ablage so kurz wie moeglich offen bleibt. Hoechstens EINTRAEGE_MAX + 1
/// Eintraege: mehr als EINTRAEGE_MAX (Spezifikation 2.6) koennen ohnehin
/// nicht hinaus, einer darueber reicht, damit der Sender "zu viele" erkennt
/// und meldet, statt still nur einen Teil zu schicken. None, wenn einer der
/// Pfade nicht zu lesen ist - lieber keine Liste als eine halbe.
fn hdrop_pfade() -> Option<Vec<PathBuf>> {
    let handle = unsafe { GetClipboardData(CF_HDROP.0 as u32) }.ok()?;
    if handle.0.is_null() {
        return None;
    }
    let hdrop = HDROP(handle.0);

    // 0xFFFFFFFF fragt nach der Anzahl, ein Index ohne Puffer nach der Laenge
    // (in Zeichen, ohne NUL).
    let anzahl = unsafe { DragQueryFileW(hdrop, u32::MAX, None) } as usize;
    let anzahl = anzahl.min(crate::dateien::EINTRAEGE_MAX as usize + 1);
    let mut pfade = Vec::with_capacity(anzahl);
    for i in 0..anzahl as u32 {
        let laenge = unsafe { DragQueryFileW(hdrop, i, None) } as usize;
        if laenge == 0 {
            return None;
        }
        let mut puffer = vec![0u16; laenge + 1];
        let n = unsafe { DragQueryFileW(hdrop, i, Some(&mut puffer)) } as usize;
        if n == 0 || n > laenge {
            return None;
        }
        pfade.push(PathBuf::from(OsString::from_wide(&puffer[..n])));
    }
    if pfade.is_empty() {
        None
    } else {
        Some(pfade)
    }
}

/// Liest CF_UNICODETEXT. Nur bei offener Ablage aufrufen. None, wenn kein
/// Text anliegt oder er die Obergrenze reisst.
fn text_lesen() -> Option<String> {
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    if handle.0.is_null() {
        return None;
    }
    let hmem = HGLOBAL(handle.0);

    // Obergrenze der Suche aus der tatsaechlichen Blockgroesse, damit ein
    // fehlender Abschluss nicht ueber das Ende hinaus liest.
    let units_in_block = unsafe { GlobalSize(hmem) } / 2;
    if units_in_block == 0 {
        return None;
    }

    let src = unsafe { GlobalLock(hmem) } as *const u16;
    if src.is_null() {
        return None;
    }

    let mut len = 0usize;
    while len < units_in_block && unsafe { *src.add(len) } != 0 {
        len += 1;
    }

    let text = if (len + 1) * 2 > MAX_BYTES {
        None
    } else {
        // Bewusst die nachsichtige Umwandlung: eine halbe Ersatzzeichenfolge aus
        // irgendeinem Programm soll die Uebertragung nicht abwuergen.
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(src, len)
        }))
    };

    // Meldet einen Fehler, wenn der Sperrzaehler auf null faellt - das ist der
    // erwartete Ausgang und keine Stoerung.
    let _ = unsafe { GlobalUnlock(hmem) };
    text
}

// ----------------------------------------------------------------- Schreiben

/// Legt einen DWORD-Wert unter einem angemeldeten Format ab. Die Ablage muss
/// bereits offen und geleert sein. Bei Erfolg geht der Speicher an das System
/// ueber, bei Misserfolg geben wir ihn selbst frei.
fn put_dword(format_name: &str, value: u32) {
    let name = wide(format_name);
    let format = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
    if format == 0 {
        return;
    }

    let Ok(hmem) = (unsafe { GlobalAlloc(GMEM_MOVEABLE, 4) }) else {
        return;
    };
    let dst = unsafe { GlobalLock(hmem) } as *mut u32;
    if dst.is_null() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return;
    }
    unsafe { dst.write_unaligned(value) };
    let _ = unsafe { GlobalUnlock(hmem) };

    if unsafe { SetClipboardData(format, Some(HANDLE(hmem.0))) }.is_err() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
    }
}

/// Die drei Formate, mit denen Windows angewiesen wird, den Inhalt weder in den
/// Verlauf aufzunehmen noch in die Cloud zu schieben. Der Inhalt des fremden
/// Rechners hat dort nichts verloren.
///
/// UNGEPRUEFT: Microsoft schreibt fuer CanIncludeInClipboardHistory und
/// CanUploadToCloudClipboard ausdruecklich einen serialisierten DWORD 0 vor; fuer
/// ExcludeClipboardContentFromMonitorProcessing heisst es nur "irgendwelche
/// Daten". Wir legen dort denselben DWORD 0 ab.
fn set_exclusion_formats() {
    put_dword("ExcludeClipboardContentFromMonitorProcessing", 0);
    put_dword("CanIncludeInClipboardHistory", 0);
    put_dword("CanUploadToCloudClipboard", 0);
}

/// Schreibt Text in die Zwischenablage, ohne die eigene Ueberwachung
/// auszuloesen - fuer keine der beiden Rollen, gleich welche ablegt (der
/// Client, was sein Host schickt; die Host-Rolle, was ihr Zuschauer schickt
/// oder ihr Menue kopiert). Scheitert still: ein verlorener Kopiervorgang ist
/// hinnehmbar, ein Absturz waehrend einer laufenden Sitzung nicht.
pub fn set(text: &str) {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    if (units.len() + 1) * 2 > MAX_BYTES {
        return;
    }
    units.push(0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    let _ = block_ablegen(&bytes, CF_UNICODETEXT.0 as u32, &[], VERSUCHE_KURZ);
}

/// Legt eine Dateiliste in die Zwischenablage (CF_HDROP), so dass "Einfuegen"
/// im Explorer die Dateien kopiert ("Preferred DropEffect" = DROPEFFECT_COPY).
/// Wie bei set(): mit den Kennzeichen aus set_exclusion_formats und der
/// eigenen Laufnummer, der Waechter meldet die Liste also nicht zurueck.
/// `pfade` sind vollstaendige Pfade der obersten Eintraege; die Dateien
/// selbst werden hier nicht angefasst. true, wenn die Liste in der Ablage
/// liegt; false bei leerer oder ungueltiger Liste (dann bleibt die Ablage,
/// wie sie ist) oder wenn die Ablage nicht zu bekommen war. Aufrufer ist der
/// Empfaenger der Dateiuebertragung (dateien.rs) in seinem Schreibfaden;
/// `h` ist seine Rolle (dorthin geht die Zeile, wenn es scheitert).
///
/// Haelt ein anderes Programm (oder der eigene Waechter) die Ablage gerade
/// fest, wird laenger und mit Abstand wiederholt als bei Text
/// (VERSUCHE_DATEIEN, rund 2 s): ein Fehlschlag hiesse Quittung 5, und die
/// vollstaendig empfangene Uebertragung waere verworfen.
pub fn set_dateien(h: Herkunft, pfade: &[PathBuf]) -> bool {
    dateien_ablegen_mit(h, pfade, VERSUCHE_DATEIEN)
}

/// set_dateien mit gegebenen Versuchen (die Tests pruefen damit auch die
/// kurze Wiederholung).
fn dateien_ablegen_mit(h: Herkunft, pfade: &[PathBuf], versuche: Versuche) -> bool {
    let Some(block) = dropfiles_bauen(pfade) else {
        return false;
    };
    let ok = block_ablegen(&block, CF_HDROP.0 as u32, &[(BEVORZUGTE_WIRKUNG, DROPEFFECT_COPY.0)], versuche);
    if !ok {
        crate::protokoll::zeile_als(h, format!(
            "Zwischenablage: Dateiliste nicht abgelegt (Ablage belegt oder Schreiben gescheitert, {} Versuche in {} ms)",
            versuche.anzahl,
            (versuche.abstand * versuche.anzahl.saturating_sub(1)).as_millis()
        ));
    }
    ok
}

/// Legt `daten` als Format `format` ab, dazu die DWORD-Formate `zusatz`.
/// Gemeinsamer Weg von set() und set_dateien(). true, wenn der Inhalt in der
/// Ablage liegt.
fn block_ablegen(daten: &[u8], format: u32, zusatz: &[(&str, u32)], versuche: Versuche) -> bool {
    // Erst der Speicher, dann die Ablage oeffnen: die Zuteilung kann dauern und
    // solange soll kein anderes Programm ausgesperrt sein.
    let Ok(hmem) = (unsafe { GlobalAlloc(GMEM_MOVEABLE, daten.len()) }) else {
        return false;
    };
    let dst = unsafe { GlobalLock(hmem) } as *mut u8;
    if dst.is_null() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return false;
    }
    unsafe { ptr::copy_nonoverlapping(daten.as_ptr(), dst, daten.len()) };
    let _ = unsafe { GlobalUnlock(hmem) };

    // Besitzerfenster besorgen. Normalfall ist das Fenster des Ueberwachungs-
    // fadens. Laeuft der nicht, legen wir uns fuer diesen einen Vorgang ein
    // eigenes an: ohne Besitzer wuerde EmptyClipboard die Ablage leeren und
    // SetClipboardData danach scheitern - der Benutzer stuende mit leerer
    // Ablage da.
    let stored = OWNER.load(Ordering::SeqCst);
    let (owner, eigenes_fenster) = if stored.is_null() {
        match create_message_window() {
            Ok(h) => (h, true),
            Err(_) => {
                let _ = unsafe { GlobalFree(Some(hmem)) };
                return false;
            }
        }
    } else {
        (HWND(stored), false)
    };

    // Ab hier ueber einen Umweg, damit das Notfenster in jedem Fall wieder weg
    // ist. DestroyWindow gehoert in den Faden, der das Fenster angelegt hat -
    // das ist hier derselbe.
    let ok = write_locked(hmem, format, owner, zusatz, versuche);

    if eigenes_fenster {
        let _ = unsafe { DestroyWindow(owner) };
    }
    ok
}

/// Der eigentliche Schreibvorgang bei geoeffneter Ablage: `hmem` als
/// `format`, dazu `zusatz` und die Kennzeichen, dann die eigene Laufnummer.
/// Getrennt, damit der Aufrufer das Notfenster auf jedem Rueckweg wieder
/// abraeumen kann.
fn write_locked(hmem: HGLOBAL, format: u32, owner: HWND, zusatz: &[(&str, u32)], versuche: Versuche) -> bool {
    let Some(guard) = open_clipboard(Some(owner), versuche) else {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return false;
    };

    if unsafe { EmptyClipboard() }.is_err() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return false;
    }

    if unsafe { SetClipboardData(format, Some(HANDLE(hmem.0))) }.is_err() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return false;
    }
    // Ab hier gehoert der Block dem System. Nicht freigeben.

    for (name, wert) in zusatz {
        put_dword(name, *wert);
    }
    set_exclusion_formats();

    // Laufnummer noch bei offener Ablage merken und gleich nach dem Schliessen
    // erneut: WM_CLIPBOARDUPDATE kann uns erreichen, bevor der zweite Wert
    // steht, und dann muss der erste schon passen.
    eigene_merken(unsafe { GetClipboardSequenceNumber() });
    drop(guard);
    eigene_merken(unsafe { GetClipboardSequenceNumber() });
    true
}

// -------------------------------------------------------------- Ueberwachung

/// WM_CLIPBOARDUPDATE: noch nicht lesen, nur den Zeitgeber (neu) stellen -
/// gelesen wird, wenn die Ablage RUHE lang ruhig war (ruhe_abgelaufen).
/// SetTimer mit derselben Kennung ersetzt den laufenden Zeitgeber. Ohne
/// Gegenueber wird nichts vorgemerkt, und ein anstehendes Lesen faellt weg:
/// dieser Inhalt hat den vorigen ueberschrieben und wird nie gelesen - auch
/// nicht, wenn die Sitzung waehrend der Ruhe beginnt.
fn ablage_geaendert(hwnd: HWND) {
    let darf = darf_lesen();
    let _ = RUHE_STAND.try_with(|r| {
        if let Ok(mut r) = r.try_borrow_mut() {
            if darf {
                r.aenderung(Instant::now());
            } else {
                r.verwerfen();
            }
        }
    });
    unsafe {
        if darf {
            let _ = SetTimer(Some(hwnd), ZEITGEBER_RUHE, RUHE.as_millis() as u32, None);
        } else {
            let _ = KillTimer(Some(hwnd), ZEITGEBER_RUHE);
        }
    }
}

/// WM_TIMER des Ruhe-Zeitgebers: ist die Ruhe um, einmal lesen und melden;
/// sonst (der Zeitgeber kann um einen Takt der Systemuhr frueh kommen) fuer
/// den Rest neu stellen.
fn ruhe_abgelaufen(hwnd: HWND) {
    let jetzt = Instant::now();
    let (faellig, rest) = RUHE_STAND
        .try_with(|r| match r.try_borrow_mut() {
            Ok(mut r) => {
                let faellig = r.faellig(jetzt);
                (faellig, r.wartet().then(|| r.schlaf(jetzt, RUHE)))
            }
            Err(_) => (false, None),
        })
        .unwrap_or((false, None));
    match rest {
        Some(d) => unsafe {
            let _ = SetTimer(Some(hwnd), ZEITGEBER_RUHE, d.as_millis() as u32 + 1, None);
        },
        None => unsafe {
            let _ = KillTimer(Some(hwnd), ZEITGEBER_RUHE);
        },
    }
    if faellig {
        on_clipboard_update();
    }
}

/// Wird aus der Fensterprozedur gerufen, wenn die Ablage ruhig ist. Darf
/// unter keinen Umstaenden in Panik geraten, sonst reisst es den Faden und
/// die Ueberwachung ist bis zum Neustart des Programms tot. Keine
/// Dateiarbeit hier: gelesen werden nur Pfade, und die Ablage ist schon
/// wieder zu, wenn die Rollen den Inhalt bekommen (ablage_ruhe::Verteiler:
/// jede mit Gegenueber, dieselbe Dateiliste je Rolle nur einmal).
fn on_clipboard_update() {
    #[cfg(test)]
    tests::NACHGESEHEN.with(|n| n.set(n.get() + 1));
    let Some(inhalt) = nachsehen(darf_lesen(), unsafe { GetClipboardSequenceNumber() }, lesen) else {
        return;
    };
    if inhalt.leer() {
        return;
    }
    // Zeilenenden bleiben, wie Windows sie liefert (CRLF). Die Umsetzung gehoert,
    // wenn ueberhaupt, an die Stelle, die das Protokoll bedient.
    for (h, verteilt) in VERTEILER.verteilen(&inhalt, Instant::now()) {
        if verteilt == Verteilt::Doppelt {
            // Dieselbe Dateiliste gerade eben schon an diese Rolle: dieselbe Kopie.
            crate::protokoll::zeile_als(h, "Zwischenablage: dieselbe Dateiliste noch einmal gemeldet - uebergangen".into());
        }
    }
}

/// Nach einer Aenderung der Ablage: gelesen wird nur mit Gegenueber (`darf`,
/// siehe `darf_lesen`) - ohne wird die Ablage weder geoeffnet noch
/// protokolliert - und nur, wenn die Aenderung nicht von unserem eigenen
/// set() bzw. set_dateien() stammt (`seq`).
fn nachsehen(darf: bool, seq: u32, lesen: impl FnOnce() -> Option<Inhalt>) -> Option<Inhalt> {
    if !darf {
        return None;
    }
    // Null bedeutet, dass wir die Nummer nicht lesen duerfen; dann greift die
    // Schleifensperre nicht und wir melden lieber einmal zu viel.
    if eigene_nummer(seq) {
        return None; // unser eigener Schreibvorgang (gleich welcher Rolle), nicht melden
    }
    lesen()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        ablage_geaendert(hwnd);
        return LRESULT(0);
    }
    if msg == WM_TIMER && wp.0 == ZEITGEBER_RUHE {
        ruhe_abgelaufen(hwnd);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Fenster anlegen, an der Zwischenablage anmelden, Nachrichtenschleife fahren.
/// Kehrt nur zurueck, wenn die Schleife endet oder etwas schiefgegangen ist.
fn run_listener() -> Result<(), String> {
    let hwnd = create_message_window()?;

    unsafe { AddClipboardFormatListener(hwnd) }
        .map_err(|e| format!("Anmeldung an der Zwischenablage: {e}"))?;

    // Ab jetzt kann set() dieses Fenster als Besitzer benutzen.
    OWNER.store(hwnd.0, Ordering::SeqCst);

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // 0 = WM_QUIT, -1 = Fehler. Beides beendet die Schleife; ein
        // Weiterlaufen bei -1 waere eine Endlosschleife.
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    OWNER.store(ptr::null_mut(), Ordering::SeqCst);
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

/// Meldet die Rolle `h` beim Waechter an und startet ihn beim ersten Mal in
/// einem eigenen Faden (einer fuer alle Rollen; eine zweite Anmeldung
/// derselben Rolle ersetzt die erste). `gegenueber` sagt, ob die Rolle
/// gerade ein Gegenueber hat und in welcher Sitzung (ablage_ruhe::Gegenueber;
/// der Client nimmt client_gegenueber, die Host-Rolle ihren Zuschauer).
/// `cb` bekommt jeden Text und jede Dateiliste, die der Benutzer auf der
/// Windows-Seite kopiert, solange diese Rolle ein Gegenueber hat; eigene
/// Schreibvorgaenge aus `set` und `set_dateien` (gleich welcher Rolle) sind
/// bereits herausgefiltert. Beide laufen im Faden des Waechters und duerfen
/// dort nicht lange arbeiten - Dateien lesen oder senden gehoert in einen
/// eigenen Faden.
pub fn watch(
    h: Herkunft,
    gegenueber: impl Fn() -> Option<u64> + Send + Sync + 'static,
    cb: impl Fn(Inhalt) + Send + Sync + 'static,
) {
    VERTEILER.anmelden(h, Arc::new(gegenueber), Arc::new(cb));
    WAECHTER.call_once(|| {
        std::thread::spawn(|| {
            if let Err(e) = run_listener() {
                eprintln!("Zwischenablage: Ueberwachung nicht gestartet: {e}");
            }
        });
    });
}

// --------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PostMessageW, PM_REMOVE};

    thread_local! {
        /// Wie oft on_clipboard_update in diesem Faden nachsah (Entprellen).
        pub(super) static NACHGESEHEN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        /// Vorgabe fuer darf_lesen in diesem Faden (None: wie im Betrieb).
        pub(super) static GEGENUEBER: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }

    /// Sperre fuer alle Tests an der echten Ablage der Sitzung. Benannt und
    /// systemweit, weil auf der Bau-VM mehrere Testlaeufe (auch aus anderen
    /// Ordnern) dieselbe Ablage der ssh-Sitzung teilen; innerhalb eines Laufs
    /// serialisiert sie die parallelen Testfaeden. Eine verlassene Sperre
    /// (abgestuerzter Lauf) gilt als genommen.
    struct AblageSperre(HANDLE);

    impl AblageSperre {
        fn nehmen() -> AblageSperre {
            let name = wide(r"Global\QuadChromaAblageTest");
            let h = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.expect("Sperre anlegen");
            let r = unsafe { WaitForSingleObject(h, 180_000) };
            assert!(r == WAIT_OBJECT_0 || r == WAIT_ABANDONED, "Sperre nicht bekommen: {}", r.0);
            AblageSperre(h)
        }
    }

    impl Drop for AblageSperre {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseMutex(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Gegenstueck zu dropfiles_bauen, nur fuer die Pruefung: Kopf pruefen,
    /// dann UTF-16-Pfade bis zum doppelten NUL. None bei kaputtem Block
    /// (zu kurz, ANSI, pFiles daneben, ohne Abschluss, leere Liste).
    fn dropfiles_zerlegen(block: &[u8]) -> Option<Vec<PathBuf>> {
        if block.len() < DROPFILES_GROESSE {
            return None;
        }
        let wert = |i: usize| u32::from_le_bytes([block[i], block[i + 1], block[i + 2], block[i + 3]]);
        let p_files = wert(0) as usize;
        if wert(16) == 0 || p_files < DROPFILES_GROESSE || p_files > block.len() {
            return None;
        }
        let einheiten: Vec<u16> =
            block[p_files..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let mut pfade = Vec::new();
        let mut anfang = 0;
        for (i, &e) in einheiten.iter().enumerate() {
            if e == 0 {
                if i == anfang {
                    return if pfade.is_empty() { None } else { Some(pfade) };
                }
                pfade.push(PathBuf::from(OsString::from_wide(&einheiten[anfang..i])));
                anfang = i + 1;
            }
        }
        None // kein doppeltes NUL: Block abgeschnitten
    }

    /// UTF-16 mit NUL als Bytes, wie CF_UNICODETEXT es will.
    fn text_bytes(t: &str) -> Vec<u8> {
        wide(t).iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    /// Spielt den Benutzer: legt die Formate `formate` (Format, Bytes) und
    /// die Kennzeichen `marken` in EINEM Vorgang ab, an set()/set_dateien()
    /// vorbei.
    fn nutzer_kopiert(fenster: HWND, formate: &[(u32, Vec<u8>)], marken: &[&str]) {
        let _guard = open_clipboard(Some(fenster), VERSUCHE_KURZ).expect("Ablage nicht zu oeffnen");
        unsafe { EmptyClipboard() }.expect("EmptyClipboard");
        for (format, daten) in formate {
            let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, daten.len()) }.expect("GlobalAlloc");
            let dst = unsafe { GlobalLock(hmem) } as *mut u8;
            assert!(!dst.is_null());
            unsafe { ptr::copy_nonoverlapping(daten.as_ptr(), dst, daten.len()) };
            let _ = unsafe { GlobalUnlock(hmem) };
            unsafe { SetClipboardData(*format, Some(HANDLE(hmem.0))) }.expect("SetClipboardData");
        }
        for marke in marken {
            put_dword(marke, 0);
        }
    }

    /// Ablage leeren und das Testfenster abbauen.
    fn aufraeumen(fenster: HWND) {
        if let Some(_guard) = open_clipboard(Some(fenster), VERSUCHE_KURZ) {
            let _ = unsafe { EmptyClipboard() };
        }
        let _ = unsafe { DestroyWindow(fenster) };
    }

    /// Roher Block eines Formats. Nur bei offener Ablage.
    fn roh_lesen(format: u32) -> Option<Vec<u8>> {
        let handle = unsafe { GetClipboardData(format) }.ok()?;
        let hmem = HGLOBAL(handle.0);
        let groesse = unsafe { GlobalSize(hmem) };
        let src = unsafe { GlobalLock(hmem) } as *const u8;
        if src.is_null() {
            return None;
        }
        let v = unsafe { std::slice::from_raw_parts(src, groesse) }.to_vec();
        let _ = unsafe { GlobalUnlock(hmem) };
        Some(v)
    }

    /// Echte Testdateien unter temp_dir, eigener Ordner je Lauf und Test:
    /// eine Datei, ein Ordner mit Leerzeichen, eine Datei mit Umlauten.
    fn testdateien(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let ordner = std::env::temp_dir().join(format!("qc-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ordner);
        let unter = ordner.join("Unterordner mit Leerzeichen");
        std::fs::create_dir_all(&unter).expect("Testordner");
        let a = ordner.join("a.txt");
        std::fs::write(&a, b"hallo").expect("Testdatei");
        let b = ordner.join("\u{e4}\u{f6}\u{fc} \u{20ac}.bin");
        std::fs::write(&b, [0u8, 1, 2]).expect("Testdatei");
        (ordner, vec![a, unter, b])
    }

    /// Ohne Gegenueber liest der Waechter nicht - die Ablage wird gar nicht
    /// erst angefasst; mit Gegenueber schon.
    #[test]
    fn ohne_sitzung_wird_nicht_gelesen() {
        let gelesen = std::cell::Cell::new(0);
        let lesen = || {
            gelesen.set(gelesen.get() + 1);
            Some(Inhalt::Text("kopiert".to_string()))
        };
        assert_eq!(nachsehen(false, 0, lesen), None);
        assert_eq!(gelesen.get(), 0, "ohne Gegenueber gelesen");
        assert_eq!(nachsehen(true, 0, lesen), Some(Inhalt::Text("kopiert".into())));
        assert_eq!(gelesen.get(), 1);
    }

    #[test]
    fn verdeckt_nach_namen() {
        assert!(!verdeckt(|_| false));
        assert!(verdeckt(|n| n == "ExcludeClipboardContentFromMonitorProcessing"));
        assert!(verdeckt(|n| n == "Clipboard Viewer Ignore"));
        assert!(!verdeckt(|n| n == "CanIncludeInClipboardHistory" || n == "CanUploadToCloudClipboard"));
    }

    /// DROPFILES als reine Logik: Kopf, Pfade mit NUL, Abschluss, Laenge;
    /// hin und zurueck; was abgelehnt wird.
    #[test]
    fn dropfiles_bauen_und_zerlegen() {
        let pfade = vec![
            PathBuf::from(r"C:\Temp\a.txt"),
            PathBuf::from(r"C:\Temp\Ordner mit Leerzeichen"),
            PathBuf::from("C:\\Temp\\\u{e4}\u{f6}\u{fc} \u{20ac} \u{1F600}.bin"),
            PathBuf::from(r"\\server\freigabe\b.doc"),
        ];
        let block = dropfiles_bauen(&pfade).expect("bauen");
        // pFiles = 20, pt = (0, 0), fNC = 0, fWide = 1
        assert_eq!(&block[..20], &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        let einheiten: usize = pfade.iter().map(|p| p.as_os_str().encode_wide().count() + 1).sum();
        assert_eq!(block.len(), 20 + (einheiten + 1) * 2);
        let erster = text_bytes(r"C:\Temp\a.txt");
        assert_eq!(&block[20..20 + erster.len()], &erster[..], "erster Pfad nicht direkt hinter dem Kopf");
        assert_eq!(&block[block.len() - 4..], &[0, 0, 0, 0], "Abschluss fehlt");
        assert_eq!(dropfiles_zerlegen(&block), Some(pfade.clone()));

        let einer = vec![PathBuf::from(r"D:\x")];
        let b = dropfiles_bauen(&einer).expect("bauen");
        assert_eq!(b.len(), 20 + (4 + 1 + 1) * 2);
        assert_eq!(dropfiles_zerlegen(&b), Some(einer));

        // Abgelehnt: leere Liste, leerer, relativer, NUL-haltiger Pfad, zu gross.
        assert_eq!(dropfiles_bauen(&[]), None);
        assert_eq!(dropfiles_bauen(&[PathBuf::new()]), None);
        assert_eq!(dropfiles_bauen(&[PathBuf::from(r"relativ\a.txt")]), None);
        assert_eq!(dropfiles_bauen(&[PathBuf::from(r"C:\a"), PathBuf::from("C:\\a\u{0}b")]), None);
        let lang = PathBuf::from(format!("C:\\{}", "x".repeat(MAX_BYTES / 2)));
        assert_eq!(dropfiles_bauen(&[lang]), None);

        // Zerlegen lehnt kaputte Bloecke ab.
        assert_eq!(dropfiles_zerlegen(&block[..10]), None, "zu kurz");
        let mut ansi = block.clone();
        ansi[16] = 0;
        assert_eq!(dropfiles_zerlegen(&ansi), None, "ANSI");
        let mut daneben = block.clone();
        daneben[0] = 0xFF;
        daneben[1] = 0xFF;
        assert_eq!(dropfiles_zerlegen(&daneben), None, "pFiles hinter dem Ende");
        assert_eq!(dropfiles_zerlegen(&block[..block.len() - 2]), None, "ohne Abschluss");
    }

    /// Am echten Windows: ein Eintrag mit Kennzeichen bleibt hier, ohne geht
    /// er hinaus, und was set() ablegt, liest lesen() nie. Schreibt auf die
    /// Zwischenablage der Sitzung, in der der Test laeuft - auf der Bau-VM
    /// ueber ssh, also nicht auf die der Konsole, an der womoeglich ein Host
    /// lauscht. Am Ende ist die Ablage leer. Protokolliert wird unter
    /// cfg(test) in einen eigenen Ordner je Lauf (secure.rs, basis_ordner).
    #[test]
    fn verdeckt_bleibt_hier() {
        let _sperre = AblageSperre::nehmen();
        let fenster = create_message_window().expect("Nachrichtenfenster");
        let ablegen = |text: &str, marken: &[&str]| {
            nutzer_kopiert(fenster, &[(CF_UNICODETEXT.0 as u32, text_bytes(text))], marken);
        };

        ablegen("offen", &[]);
        assert_eq!(lesen(), Some(Inhalt::Text("offen".into())));

        ablegen("geheim123", &["ExcludeClipboardContentFromMonitorProcessing"]);
        assert_eq!(lesen(), None, "verdeckter Eintrag wurde gelesen");

        ablegen("geheim456", &["Clipboard Viewer Ignore"]);
        assert_eq!(lesen(), None, "verdeckter Eintrag (Clipboard Viewer Ignore) wurde gelesen");

        // Nur "nicht in den Verlauf" ist nicht vertraulich.
        ablegen("verlauf", &["CanIncludeInClipboardHistory"]);
        assert_eq!(lesen(), Some(Inhalt::Text("verlauf".into())));

        // Was von der Gegenseite kommt, traegt das Kennzeichen selbst. Dass
        // set() wirklich geschrieben hat, zeigen Laufnummer und Textformat.
        let vorher = unsafe { GetClipboardSequenceNumber() };
        set("von drueben");
        assert_ne!(unsafe { GetClipboardSequenceNumber() }, vorher, "set() hat nichts geschrieben");
        assert!(unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok());
        assert_eq!(lesen(), None, "eigener Schreibvorgang wurde gelesen");

        aufraeumen(fenster);
    }

    /// Am echten Windows: kopiert der Benutzer Dateien (CF_HDROP), meldet
    /// lesen() die Pfade - ueber DragQueryFileW - und nicht den Text, den
    /// Programme oft daneben ablegen. Ohne Dateiliste bleibt es beim Text.
    #[test]
    fn dateiliste_vor_text() {
        let _sperre = AblageSperre::nehmen();
        let (ordner, pfade) = testdateien("dateiliste_vor_text");
        let fenster = create_message_window().expect("Nachrichtenfenster");
        let block = dropfiles_bauen(&pfade).expect("bauen");

        nutzer_kopiert(fenster, &[(CF_HDROP.0 as u32, block.clone())], &[]);
        assert_eq!(lesen(), Some(Inhalt::Dateien(pfade.clone())), "Dateiliste allein");

        nutzer_kopiert(
            fenster,
            &[(CF_UNICODETEXT.0 as u32, text_bytes("a.txt")), (CF_HDROP.0 as u32, block.clone())],
            &[],
        );
        assert_eq!(lesen(), Some(Inhalt::Dateien(pfade.clone())), "Text statt Dateiliste gemeldet");

        nutzer_kopiert(fenster, &[(CF_UNICODETEXT.0 as u32, text_bytes("nur Text"))], &[]);
        assert_eq!(lesen(), Some(Inhalt::Text("nur Text".into())));

        aufraeumen(fenster);
        let _ = std::fs::remove_dir_all(ordner);
    }

    /// Am echten Windows: eine verdeckte Dateiliste bleibt hier - weder die
    /// Pfade noch der Text daneben gehen hinaus.
    #[test]
    fn verdeckte_dateiliste_bleibt_hier() {
        let _sperre = AblageSperre::nehmen();
        let (ordner, pfade) = testdateien("verdeckte_dateiliste");
        let fenster = create_message_window().expect("Nachrichtenfenster");
        let block = dropfiles_bauen(&pfade).expect("bauen");

        for marke in VERDECKT {
            nutzer_kopiert(
                fenster,
                &[(CF_HDROP.0 as u32, block.clone()), (CF_UNICODETEXT.0 as u32, text_bytes("a.txt"))],
                &[marke],
            );
            assert_eq!(lesen(), None, "verdeckte Dateiliste ({marke}) wurde gelesen");
        }

        aufraeumen(fenster);
        let _ = std::fs::remove_dir_all(ordner);
    }

    /// Am echten Windows: set_dateien() legt CF_HDROP mit genau den Pfaden
    /// ab (zurueckgelesen ueber DragQueryFileW und roh), dazu "Preferred
    /// DropEffect" = DROPEFFECT_COPY und die Kennzeichen. Eine leere oder
    /// ungueltige Liste laesst die Ablage, wie sie ist.
    #[test]
    fn set_dateien_legt_ab() {
        let _sperre = AblageSperre::nehmen();
        let (ordner, pfade) = testdateien("set_dateien_legt_ab");
        let fenster = create_message_window().expect("Nachrichtenfenster");

        let vorher = unsafe { GetClipboardSequenceNumber() };
        assert!(set_dateien(Herkunft::Client, &pfade), "set_dateien() meldet Fehlschlag");
        assert_ne!(unsafe { GetClipboardSequenceNumber() }, vorher, "set_dateien() hat nichts geschrieben");
        {
            let _guard = open_clipboard(Some(fenster), VERSUCHE_KURZ).expect("Ablage nicht zu oeffnen");
            let wirkung = wide(BEVORZUGTE_WIRKUNG);
            let wirkung = unsafe { RegisterClipboardFormatW(PCWSTR(wirkung.as_ptr())) };
            let wert = roh_lesen(wirkung).expect("Preferred DropEffect fehlt");
            assert_eq!(&wert[..4], &DROPEFFECT_COPY.0.to_le_bytes(), "Preferred DropEffect ist nicht COPY");
            for kennzeichen in
                ["ExcludeClipboardContentFromMonitorProcessing", "CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"]
            {
                assert!(format_vorhanden(kennzeichen), "Kennzeichen {kennzeichen} fehlt");
            }
            assert_eq!(hdrop_pfade(), Some(pfade.clone()), "DragQueryFileW liest andere Pfade");
            let roh = roh_lesen(CF_HDROP.0 as u32).expect("CF_HDROP fehlt");
            assert_eq!(dropfiles_zerlegen(&roh), Some(pfade.clone()), "Block weicht ab");
        }

        // Nichts Gueltiges: Ablage bleibt, wie sie ist.
        let vorher = unsafe { GetClipboardSequenceNumber() };
        assert!(!set_dateien(Herkunft::Client, &[]));
        assert!(!set_dateien(Herkunft::Client, &[PathBuf::from(r"relativ\a.txt")]));
        assert_eq!(unsafe { GetClipboardSequenceNumber() }, vorher, "ungueltige Liste hat die Ablage angefasst");
        assert!(unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok());

        aufraeumen(fenster);
        let _ = std::fs::remove_dir_all(ordner);
    }

    /// Am echten Windows: was set_dateien() ablegt, meldet der Waechter nicht
    /// zurueck - doppelt gesichert ueber die Laufnummer (nachsehen) und das
    /// eigene Kennzeichen (lesen). Beide Schichten werden einzeln geprueft.
    #[test]
    fn set_dateien_ohne_widerhall() {
        let _sperre = AblageSperre::nehmen();
        let (ordner, pfade) = testdateien("set_dateien_ohne_widerhall");
        let fenster = create_message_window().expect("Nachrichtenfenster");

        assert!(set_dateien(Herkunft::Client, &pfade), "set_dateien() meldet Fehlschlag");
        let seq = unsafe { GetClipboardSequenceNumber() };
        let laufnummer_greift = nachsehen(true, seq, || Some(Inhalt::Text("gelesen".into()))).is_none();
        let kennzeichen_greift = lesen().is_none();
        assert!(
            laufnummer_greift && kennzeichen_greift,
            "Widerhall: Laufnummer greift {laufnummer_greift}, Kennzeichen greift {kennzeichen_greift}"
        );

        aufraeumen(fenster);
        let _ = std::fs::remove_dir_all(ordner);
    }

    /// Haelt die Ablage in einem eigenen Faden (mit eigenem Fenster) fuer
    /// `dauer` offen; kehrt zurueck, sobald sie offen ist. Der Griff wartet
    /// auf das Ende des Fadens.
    fn ablage_halten(dauer: Duration) -> std::thread::JoinHandle<()> {
        let (offen_tx, offen_rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let fenster = create_message_window().expect("Nachrichtenfenster");
            let guard = open_clipboard(Some(fenster), VERSUCHE_DATEIEN).expect("Ablage nicht zu halten");
            offen_tx.send(()).unwrap();
            std::thread::sleep(dauer);
            drop(guard);
            let _ = unsafe { DestroyWindow(fenster) };
        });
        offen_rx.recv_timeout(Duration::from_secs(5)).expect("Ablage nicht geoeffnet");
        h
    }

    /// Am echten Windows (Durchsicht [15]): haelt ein anderer die Ablage
    /// 300 ms fest, scheitert die kurze Wiederholung des Textes - die
    /// Dateiliste aber wartet (bis rund 2 s) und liegt danach in der Ablage.
    #[test]
    fn set_dateien_wartet_auf_belegte_ablage() {
        let _sperre = AblageSperre::nehmen();
        let (ordner, pfade) = testdateien("set_dateien_wartet");
        let fenster = create_message_window().expect("Nachrichtenfenster");

        let halter = ablage_halten(Duration::from_millis(400));
        let t0 = Instant::now();
        assert!(!dateien_ablegen_mit(Herkunft::Client, &pfade, VERSUCHE_KURZ), "kurze Wiederholung trotz belegter Ablage gelungen");
        assert!(t0.elapsed() < Duration::from_millis(350), "kurze Wiederholung dauerte {:?}", t0.elapsed());
        halter.join().unwrap();

        let halter = ablage_halten(Duration::from_millis(300));
        let t0 = Instant::now();
        assert!(set_dateien(Herkunft::Client, &pfade), "Dateiliste trotz 300 ms belegter Ablage verworfen");
        let dauer = t0.elapsed();
        halter.join().unwrap();
        assert!(dauer >= Duration::from_millis(200), "nicht gewartet ({dauer:?}) - war die Ablage belegt?");
        {
            let _guard = open_clipboard(Some(fenster), VERSUCHE_KURZ).expect("Ablage nicht zu oeffnen");
            assert_eq!(hdrop_pfade(), Some(pfade.clone()), "andere Pfade in der Ablage");
        }

        aufraeumen(fenster);
        let _ = std::fs::remove_dir_all(ordner);
    }

    /// Die Nachrichten dieses Fadens abarbeiten, `dauer` lang.
    fn pumpen(dauer: Duration) {
        let bis = Instant::now() + dauer;
        let mut msg = MSG::default();
        while Instant::now() < bis {
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Im Faden des Waechters (Integrationstest Befunde 1 und 2): drei
    /// Meldungen dicht hintereinander ergeben EIN Nachsehen, und zwar erst,
    /// wenn die Ablage RUHE lang ruhig war - nicht bei der ersten Meldung.
    /// Ohne Gegenueber wird nichts vorgemerkt, und eine Meldung ohne
    /// Gegenueber verwirft ein anstehendes Nachsehen. Die Meldungen kommen
    /// von Hand (PostMessageW), das Gegenueber gibt der Test vor; die Sperre,
    /// weil nachsehen() dann die echte Ablage liest.
    #[test]
    fn waechter_entprellt_im_fensterfaden() {
        let _sperre = AblageSperre::nehmen();
        let fenster = create_message_window().expect("Nachrichtenfenster");
        NACHGESEHEN.with(|n| n.set(0));
        let gegenueber = |an: bool| GEGENUEBER.with(|g| g.set(Some(an)));
        let melden = || unsafe {
            PostMessageW(Some(fenster), WM_CLIPBOARDUPDATE, WPARAM(0), LPARAM(0)).expect("PostMessageW");
        };
        let nachgesehen = || NACHGESEHEN.with(|n| n.get());
        gegenueber(true);
        melden();
        pumpen(Duration::from_millis(40));
        melden();
        pumpen(Duration::from_millis(40));
        melden();
        pumpen(Duration::from_millis(60));
        assert_eq!(nachgesehen(), 0, "vor der Ruhe nachgesehen");
        pumpen(RUHE + Duration::from_millis(250));
        assert_eq!(nachgesehen(), 1, "nicht genau einmal nachgesehen");
        pumpen(Duration::from_millis(300));
        assert_eq!(nachgesehen(), 1, "ohne neue Meldung noch einmal nachgesehen");
        // Die naechste Kopie: wieder genau einmal.
        melden();
        pumpen(RUHE + Duration::from_millis(250));
        assert_eq!(nachgesehen(), 2);
        // Ohne Gegenueber: nichts, auch nicht, wenn es waehrend der Ruhe kommt.
        gegenueber(false);
        melden();
        pumpen(Duration::from_millis(50));
        gegenueber(true);
        pumpen(RUHE + Duration::from_millis(250));
        assert_eq!(nachgesehen(), 2, "Kopie ohne Gegenueber nachgesehen");
        // Eine Kopie ohne Gegenueber ueberschreibt eine anstehende.
        melden();
        pumpen(Duration::from_millis(50));
        gegenueber(false);
        melden();
        pumpen(Duration::from_millis(50));
        gegenueber(true);
        pumpen(RUHE + Duration::from_millis(250));
        assert_eq!(nachgesehen(), 2, "ueberschriebene Kopie nachgesehen");
        GEGENUEBER.with(|g| g.set(None));
        let _ = unsafe { DestroyWindow(fenster) };
    }

    /// Eigene Laufnummern: jede gemerkte gilt als eigene, auch wenn ein
    /// zweiter Vorgang (die andere Rolle) dazwischen seine gemerkt hat; 0
    /// ("nicht lesbar") nie.
    #[test]
    fn eigene_laufnummern_beider_rollen() {
        let basis = 0xF000_0000u32 | (std::process::id() & 0xFFFF) << 8;
        // Vorgang A merkt die erste Nummer, B beide, dann A die zweite.
        eigene_merken(basis + 1);
        eigene_merken(basis + 3);
        eigene_merken(basis + 4);
        eigene_merken(basis + 2);
        for n in 1..=4 {
            assert!(eigene_nummer(basis + n), "Nummer {n} nicht als eigene erkannt");
        }
        assert!(!eigene_nummer(basis + 5));
        eigene_merken(0);
        assert!(!eigene_nummer(0));
    }

    /// Wartet bis zu `frist`, bis `pruefen` gilt.
    fn warten_bis(frist: Duration, pruefen: impl Fn() -> bool) -> bool {
        let bis = Instant::now() + frist;
        while Instant::now() < bis {
            if pruefen() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        pruefen()
    }

    /// Am echten Windows: Client und Host-Rolle melden sich an EINEM
    /// Waechter an. Eine Kopie des Nutzers geht an jede Rolle mit
    /// Gegenueber und an keine ohne; was eine Rolle selbst ablegt, bekommt
    /// keine - weder zurueck noch die andere. Die Meldung an das Fenster des
    /// Waechters schickt der Test zusaetzlich selbst (in der ssh-Sitzung der
    /// Bau-VM ist nicht sicher, dass Windows sie schickt); doppelt schadet
    /// nicht, der Waechter entprellt. Am Ende haben beide Rollen kein
    /// Gegenueber mehr, damit der Waechter fuer die anderen Tests wieder
    /// nicht liest.
    #[test]
    fn ein_waechter_fuer_beide_rollen() {
        use std::sync::atomic::AtomicBool;
        let _sperre = AblageSperre::nehmen();
        let fenster = create_message_window().expect("Nachrichtenfenster");
        let client_da = Arc::new(AtomicBool::new(true));
        let host_da = Arc::new(AtomicBool::new(true));
        let client_bekam = Arc::new(Mutex::new(Vec::<Inhalt>::new()));
        let host_bekam = Arc::new(Mutex::new(Vec::<Inhalt>::new()));
        let (d, b) = (client_da.clone(), client_bekam.clone());
        watch(Herkunft::Client, move || d.load(Ordering::SeqCst).then_some(1), move |i| b.lock().unwrap().push(i));
        let (d, b) = (host_da.clone(), host_bekam.clone());
        watch(Herkunft::Host, move || d.load(Ordering::SeqCst).then_some(1), move |i| b.lock().unwrap().push(i));
        assert!(warten_bis(Duration::from_secs(5), || !OWNER.load(Ordering::SeqCst).is_null()), "Waechter laeuft nicht");
        let waechter = HWND(OWNER.load(Ordering::SeqCst));
        let kopieren = |text: &str| {
            nutzer_kopiert(fenster, &[(CF_UNICODETEXT.0 as u32, text_bytes(text))], &[]);
            unsafe { PostMessageW(Some(waechter), WM_CLIPBOARDUPDATE, WPARAM(0), LPARAM(0)).expect("PostMessageW") };
        };
        let bekam = |v: &Arc<Mutex<Vec<Inhalt>>>, t: &str| v.lock().unwrap().contains(&Inhalt::Text(t.into()));

        kopieren("an beide Rollen");
        let beide = warten_bis(Duration::from_secs(3), || bekam(&client_bekam, "an beide Rollen") && bekam(&host_bekam, "an beide Rollen"));

        // Was eine Rolle ablegt, meldet der Waechter keiner.
        set("von einem Gegenueber");
        let seq = unsafe { GetClipboardSequenceNumber() };
        unsafe { PostMessageW(Some(waechter), WM_CLIPBOARDUPDATE, WPARAM(0), LPARAM(0)).expect("PostMessageW") };
        std::thread::sleep(RUHE + Duration::from_millis(400));
        let widerhall = bekam(&client_bekam, "von einem Gegenueber") || bekam(&host_bekam, "von einem Gegenueber");

        // Nur noch der Client hat ein Gegenueber.
        host_da.store(false, Ordering::SeqCst);
        kopieren("nur an den Client");
        let client_allein = warten_bis(Duration::from_secs(3), || bekam(&client_bekam, "nur an den Client"));
        std::thread::sleep(Duration::from_millis(200));
        let host_trotzdem = bekam(&host_bekam, "nur an den Client");

        client_da.store(false, Ordering::SeqCst);
        aufraeumen(fenster);
        assert!(beide, "Kopie nicht an beide Rollen: Client {:?}, Host {:?}", client_bekam.lock().unwrap(), host_bekam.lock().unwrap());
        assert!(!widerhall, "eigenes Ablegen gemeldet (Laufnummer {seq} eigen: {})", eigene_nummer(seq));
        assert!(client_allein, "Kopie nicht beim Client: {:?}", client_bekam.lock().unwrap());
        assert!(!host_trotzdem, "Host-Rolle ohne Zuschauer bekam die Kopie");
    }
}
