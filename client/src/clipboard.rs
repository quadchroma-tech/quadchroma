// Zwischenablage auf der Windows-Seite.
//
// Dieselbe Datei dient zwei Rollen derselben Programmdatei, nie gleichzeitig:
// dem Windows-Client und der Windows-Host-Rolle (--host, host/mod.rs). In
// beiden meldet watch(), was der Benutzer hier kopiert hat, und set() legt
// ab, was von der Gegenseite kommt, ohne die eigene Ueberwachung auszuloesen.
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
// Gelesen wird nur mit Gegenueber ("kein Zuschauer, keine Arbeit"), wie auf
// dem Mac (clipboard_mac.rs): im Client waehrend einer Sitzung (`sitzung`),
// in der Host-Rolle, solange ein Zuschauer da ist. Ohne Gegenueber wird die
// Ablage nicht einmal geoeffnet - sonst stuende auch ohne jede Verbindung
// jede Passwortkopie als Zeile "verdeckt" im Protokoll. Was davor kopiert
// wurde, geht beim Sitzungsbeginn nicht nachtraeglich hinaus.
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
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::time::Duration;

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
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
    WNDCLASSW,
};

/// Obergrenze fuer einen Uebertragungsvorgang. Groesseres wird stillschweigend
/// verworfen statt abgeschnitten: ein halber Text ist schlimmer als keiner.
/// Gilt auch fuer den Block einer abgelegten Dateiliste.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Mehr oberste Eintraege einer Dateiliste liest der Waechter nicht. Mehr als
/// EINTRAEGE_MAX (10000, Spezifikation 2.6) koennen ohnehin nicht hinaus;
/// einer darueber reicht, damit der Sender "zu viele" erkennt und meldet,
/// statt still nur einen Teil zu schicken. Haelt ausserdem die Zeit bei
/// offener Ablage kurz.
const DATEIEN_MAX: usize = 10_000;

/// Groesse des Kopfes DROPFILES (packed(1)): pFiles, pt.x, pt.y, fNC, fWide.
const DROPFILES_GROESSE: usize = 20;
const _: () = assert!(std::mem::size_of::<DROPFILES>() == DROPFILES_GROESSE);

/// Format, aus dem Explorer liest, ob "Einfuegen" kopiert oder verschiebt
/// (CFSTR_PREFERREDDROPEFFECT). Wir legen immer DROPEFFECT_COPY ab: die
/// Dateien im Uebertragungsverzeichnis bleiben, wo sie sind.
const BEVORZUGTE_WIRKUNG: &str = "Preferred DropEffect";

/// Laufnummer unseres eigenen Schreibvorgangs. Zwei Werte, weil nicht sicher
/// belegt ist, ob Windows die Nummer schon beim SetClipboardData hochzaehlt oder
/// erst beim CloseClipboard; wir merken uns beide und lassen beide durchfallen.
/// Gilt fuer Text und Dateilisten gleichermassen (write_locked).
static OWN_SEQ_OPEN: AtomicU32 = AtomicU32::new(0);
static OWN_SEQ_CLOSED: AtomicU32 = AtomicU32::new(0);

/// Griff auf das Nachrichtenfenster des Ueberwachungsfadens. Zum Schreiben
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

/// Was der Benutzer kopiert hat. Dieselbe Art steht in clipboard_mac.rs,
/// damit die Aufrufer ohne Plattformweiche auskommen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inhalt {
    /// Text; Zeilenenden, wie Windows sie liefert (CRLF).
    Text(String),
    /// Eine Dateiliste (CF_HDROP): die obersten Pfade, wie Explorer sie
    /// ablegt. Ordner werden hier nicht aufgeloest.
    Dateien(Vec<PathBuf>),
}

impl Inhalt {
    fn leer(&self) -> bool {
        match self {
            Inhalt::Text(t) => t.is_empty(),
            Inhalt::Dateien(p) => p.is_empty(),
        }
    }
}

/// Sitzung des Clients beginnt (true, der Host hat angenommen) oder endet.
pub fn sitzung(an: bool) {
    SITZUNG.store(an, Ordering::Relaxed);
}

/// Darf der Waechter jetzt lesen? Im Client mit Sitzung, in der Host-Rolle
/// mit Zuschauer. Die Host-Rolle setzt `sitzung` nicht; ihren Zuschauer
/// kennt das Netzteil (netz::zuschauer_da), und im Client ist dort nie einer.
fn darf_lesen() -> bool {
    SITZUNG.load(Ordering::Relaxed) || crate::host::netz::zuschauer_da()
}

thread_local! {
    /// Empfaenger des Ueberwachungsfadens. Liegt im Faden selbst, weil die
    /// Fensterprozedur genau dort und nur dort aufgerufen wird.
    static SINK: RefCell<Option<Box<dyn Fn(Inhalt)>>> = RefCell::new(None);
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
/// der Normalfall, kein Grund zur Aufregung. Zehn Versuche im Abstand von 10 ms,
/// danach geben wir leise auf.
///
/// `owner` darf zum reinen Lesen None sein. Zum Schreiben muss ein Fenster
/// angegeben werden, siehe OWNER.
fn open_clipboard(owner: Option<HWND>) -> Option<ClipboardGuard> {
    for versuch in 0..10 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Some(ClipboardGuard);
        }
        if versuch < 9 {
            std::thread::sleep(Duration::from_millis(10));
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
/// erst danach (on_clipboard_update).
fn lesen() -> Option<Inhalt> {
    // Zum Lesen braucht es kein Besitzerfenster.
    let _guard = open_clipboard(None)?;

    // Bei offener Ablage fragen, damit Kennzeichen und Inhalt zum selben
    // Eintrag gehoeren.
    if verdeckt(format_vorhanden) {
        crate::protokoll::zeile("Zwischenablage: Eintrag ist als verdeckt markiert, nicht uebertragen".into());
        return None;
    }

    if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok() {
        let pfade = hdrop_pfade();
        if pfade.is_none() {
            crate::protokoll::zeile("Zwischenablage: Dateiliste nicht lesbar, nichts uebertragen".into());
        }
        return pfade.map(Inhalt::Dateien);
    }

    text_lesen().map(Inhalt::Text)
}

/// Die Pfade aus CF_HDROP, ueber DragQueryFileW. Nur bei offener Ablage
/// aufrufen: das Handle gehoert der Ablage und gilt nur, solange sie offen
/// ist. Deshalb auch nie DragFinish darauf - das gaebe einen Block frei, der
/// dem System gehoert. Gelesen werden nur die Pfade, keine Datei, damit die
/// Ablage so kurz wie moeglich offen bleibt. Hoechstens DATEIEN_MAX + 1
/// Eintraege. None, wenn einer der Pfade nicht zu lesen ist - lieber keine
/// Liste als eine halbe.
fn hdrop_pfade() -> Option<Vec<PathBuf>> {
    let handle = unsafe { GetClipboardData(CF_HDROP.0 as u32) }.ok()?;
    if handle.0.is_null() {
        return None;
    }
    let hdrop = HDROP(handle.0);

    // 0xFFFFFFFF fragt nach der Anzahl, ein Index ohne Puffer nach der Laenge
    // (in Zeichen, ohne NUL).
    let anzahl = unsafe { DragQueryFileW(hdrop, u32::MAX, None) } as usize;
    let anzahl = anzahl.min(DATEIEN_MAX + 1);
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

/// Schreibt Text in die Zwischenablage, ohne die eigene Ueberwachung auszuloesen.
/// Scheitert still: ein verlorener Kopiervorgang ist hinnehmbar, ein Absturz des
/// Clients waehrend einer laufenden Sitzung nicht.
pub fn set(text: &str) {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    if (units.len() + 1) * 2 > MAX_BYTES {
        return;
    }
    units.push(0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    let _ = block_ablegen(&bytes, CF_UNICODETEXT.0 as u32, &[]);
}

/// Legt eine Dateiliste in die Zwischenablage (CF_HDROP), so dass "Einfuegen"
/// im Explorer die Dateien kopiert ("Preferred DropEffect" = DROPEFFECT_COPY).
/// Wie bei set(): mit den Kennzeichen aus set_exclusion_formats und der
/// eigenen Laufnummer, der Waechter meldet die Liste also nicht zurueck.
/// `pfade` sind vollstaendige Pfade der obersten Eintraege; die Dateien
/// selbst werden hier nicht angefasst. true, wenn die Liste in der Ablage
/// liegt; false bei leerer oder ungueltiger Liste (dann bleibt die Ablage,
/// wie sie ist) oder wenn die Ablage nicht zu bekommen war.
// Aufrufer ist der Empfaenger der Dateiuebertragung (dateien.rs), der mit
// einem eigenen Paket dazukommt; bis dahin nur die Tests.
#[allow(dead_code)]
pub fn set_dateien(pfade: &[PathBuf]) -> bool {
    let Some(block) = dropfiles_bauen(pfade) else {
        return false;
    };
    block_ablegen(&block, CF_HDROP.0 as u32, &[(BEVORZUGTE_WIRKUNG, DROPEFFECT_COPY.0)])
}

/// Legt `daten` als Format `format` ab, dazu die DWORD-Formate `zusatz`.
/// Gemeinsamer Weg von set() und set_dateien(). true, wenn der Inhalt in der
/// Ablage liegt.
fn block_ablegen(daten: &[u8], format: u32, zusatz: &[(&str, u32)]) -> bool {
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
    let ok = write_locked(hmem, format, owner, zusatz);

    if eigenes_fenster {
        let _ = unsafe { DestroyWindow(owner) };
    }
    ok
}

/// Der eigentliche Schreibvorgang bei geoeffneter Ablage: `hmem` als
/// `format`, dazu `zusatz` und die Kennzeichen, dann die eigene Laufnummer.
/// Getrennt, damit der Aufrufer das Notfenster auf jedem Rueckweg wieder
/// abraeumen kann.
fn write_locked(hmem: HGLOBAL, format: u32, owner: HWND, zusatz: &[(&str, u32)]) -> bool {
    let Some(guard) = open_clipboard(Some(owner)) else {
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
    OWN_SEQ_OPEN.store(unsafe { GetClipboardSequenceNumber() }, Ordering::SeqCst);
    drop(guard);
    OWN_SEQ_CLOSED.store(unsafe { GetClipboardSequenceNumber() }, Ordering::SeqCst);
    true
}

// -------------------------------------------------------------- Ueberwachung

/// Wird aus der Fensterprozedur gerufen. Darf unter keinen Umstaenden in Panik
/// geraten, sonst reisst es den Faden und die Ueberwachung ist bis zum Neustart
/// des Clients tot. Keine Dateiarbeit hier: gelesen werden nur Pfade, und
/// die Ablage ist schon wieder zu, wenn `cb` sie bekommt.
fn on_clipboard_update() {
    let Some(inhalt) = nachsehen(darf_lesen(), unsafe { GetClipboardSequenceNumber() }, lesen) else {
        return;
    };
    if inhalt.leer() {
        return;
    }

    // Zeilenenden bleiben, wie Windows sie liefert (CRLF). Die Umsetzung gehoert,
    // wenn ueberhaupt, an die Stelle, die das Protokoll bedient.
    let _ = SINK.try_with(|s| {
        if let Ok(sink) = s.try_borrow() {
            if let Some(cb) = sink.as_ref() {
                cb(inhalt);
            }
        }
    });
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
    if seq != 0
        && (seq == OWN_SEQ_OPEN.load(Ordering::SeqCst)
            || seq == OWN_SEQ_CLOSED.load(Ordering::SeqCst))
    {
        return None; // unser eigener Schreibvorgang, nicht zuruecksenden
    }
    lesen()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        on_clipboard_update();
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

/// Startet die Ueberwachung in einem eigenen Faden. `cb` bekommt jeden Text
/// und jede Dateiliste, die der Benutzer auf der Windows-Seite mit
/// Gegenueber kopiert hat (siehe `darf_lesen`); eigene Schreibvorgaenge aus
/// `set` und `set_dateien` sind bereits herausgefiltert. `cb` laeuft im
/// Faden des Waechters und darf dort nicht lange arbeiten - Dateien lesen
/// oder senden gehoert in einen eigenen Faden.
pub fn watch(cb: impl Fn(Inhalt) + Send + 'static) {
    std::thread::spawn(move || {
        SINK.with(|s| *s.borrow_mut() = Some(Box::new(cb)));
        if let Err(e) = run_listener() {
            eprintln!("Zwischenablage: Ueberwachung nicht gestartet: {e}");
        }
    });
}

// --------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

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
        let _guard = open_clipboard(Some(fenster)).expect("Ablage nicht zu oeffnen");
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
        if let Some(_guard) = open_clipboard(Some(fenster)) {
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
        assert!(set_dateien(&pfade), "set_dateien() meldet Fehlschlag");
        assert_ne!(unsafe { GetClipboardSequenceNumber() }, vorher, "set_dateien() hat nichts geschrieben");
        {
            let _guard = open_clipboard(Some(fenster)).expect("Ablage nicht zu oeffnen");
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
        assert!(!set_dateien(&[]));
        assert!(!set_dateien(&[PathBuf::from(r"relativ\a.txt")]));
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

        assert!(set_dateien(&pfade), "set_dateien() meldet Fehlschlag");
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
}
