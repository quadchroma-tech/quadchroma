// Zwischenablage auf der Windows-Seite.
//
// Zwei Richtungen, beide ueber die Eingabeverbindung (Port 9002, Nachrichtentyp
// 48, UTF-8):
//   Windows -> Mac : watch() meldet, was der Benutzer hier kopiert hat.
//   Mac -> Windows : set() legt Text ab, ohne die eigene Ueberwachung auszuloesen.
//
// Das Melden laeuft ueber AddClipboardFormatListener und ein reines
// Nachrichtenfenster; Windows schickt dann WM_CLIPBOARDUPDATE. Kein Abfragen im
// Takt, kein SetClipboardViewer (dessen Kette bricht, sobald ein Glied abstuerzt).
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
//       "Win32_UI_WindowsAndMessaging",
//   ] }
//
// Alle verwendeten Signaturen sind gegen die Bindungen von windows 0.62.2
// geprueft (microsoft.github.io/windows-docs-rs).

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
    WNDCLASSW,
};

/// Obergrenze fuer einen Uebertragungsvorgang. Groesseres wird stillschweigend
/// verworfen statt abgeschnitten: ein halber Text ist schlimmer als keiner.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Laufnummer unseres eigenen Schreibvorgangs. Zwei Werte, weil nicht sicher
/// belegt ist, ob Windows die Nummer schon beim SetClipboardData hochzaehlt oder
/// erst beim CloseClipboard; wir merken uns beide und lassen beide durchfallen.
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

thread_local! {
    /// Empfaenger des Ueberwachungsfadens. Liegt im Faden selbst, weil die
    /// Fensterprozedur genau dort und nur dort aufgerufen wird.
    static SINK: RefCell<Option<Box<dyn Fn(String)>>> = RefCell::new(None);
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

// ------------------------------------------------------------------- Lesen

/// Liest CF_UNICODETEXT. Gibt None zurueck, wenn nichts Textartiges anliegt, die
/// Ablage nicht zu bekommen war oder der Inhalt die Obergrenze reisst.
fn read_text() -> Option<String> {
    // Zum Lesen braucht es kein Besitzerfenster.
    let _guard = open_clipboard(None)?;

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

    // Erst der Speicher, dann die Ablage oeffnen: die Zuteilung kann dauern und
    // solange soll kein anderes Programm ausgesperrt sein.
    let bytes = units.len() * 2;
    let Ok(hmem) = (unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }) else {
        return;
    };
    let dst = unsafe { GlobalLock(hmem) } as *mut u16;
    if dst.is_null() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return;
    }
    unsafe { ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len()) };
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
                return;
            }
        }
    } else {
        (HWND(stored), false)
    };

    // Ab hier ueber einen Umweg, damit das Notfenster in jedem Fall wieder weg
    // ist. DestroyWindow gehoert in den Faden, der das Fenster angelegt hat -
    // das ist hier derselbe.
    write_locked(hmem, owner);

    if eigenes_fenster {
        let _ = unsafe { DestroyWindow(owner) };
    }
}

/// Der eigentliche Schreibvorgang bei geoeffneter Ablage. Getrennt, damit der
/// Aufrufer das Notfenster auf jedem Rueckweg wieder abraeumen kann.
fn write_locked(hmem: HGLOBAL, owner: HWND) {
    let Some(guard) = open_clipboard(Some(owner)) else {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return;
    };

    if unsafe { EmptyClipboard() }.is_err() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return;
    }

    if unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))) }.is_err() {
        let _ = unsafe { GlobalFree(Some(hmem)) };
        return;
    }
    // Ab hier gehoert der Block dem System. Nicht freigeben.

    set_exclusion_formats();

    // Laufnummer noch bei offener Ablage merken und gleich nach dem Schliessen
    // erneut: WM_CLIPBOARDUPDATE kann uns erreichen, bevor der zweite Wert
    // steht, und dann muss der erste schon passen.
    OWN_SEQ_OPEN.store(unsafe { GetClipboardSequenceNumber() }, Ordering::SeqCst);
    drop(guard);
    OWN_SEQ_CLOSED.store(unsafe { GetClipboardSequenceNumber() }, Ordering::SeqCst);
}

// -------------------------------------------------------------- Ueberwachung

/// Wird aus der Fensterprozedur gerufen. Darf unter keinen Umstaenden in Panik
/// geraten, sonst reisst es den Faden und die Ueberwachung ist bis zum Neustart
/// des Clients tot.
fn on_clipboard_update() {
    let seq = unsafe { GetClipboardSequenceNumber() };
    // Null bedeutet, dass wir die Nummer nicht lesen duerfen; dann greift die
    // Schleifensperre nicht und wir melden lieber einmal zu viel.
    if seq != 0
        && (seq == OWN_SEQ_OPEN.load(Ordering::SeqCst)
            || seq == OWN_SEQ_CLOSED.load(Ordering::SeqCst))
    {
        return; // unser eigener Schreibvorgang, nicht zuruecksenden
    }

    let Some(text) = read_text() else { return };
    if text.is_empty() {
        return;
    }

    // Zeilenenden bleiben, wie Windows sie liefert (CRLF). Die Umsetzung gehoert,
    // wenn ueberhaupt, an die Stelle, die das Protokoll bedient.
    let _ = SINK.try_with(|s| {
        if let Ok(sink) = s.try_borrow() {
            if let Some(cb) = sink.as_ref() {
                cb(text);
            }
        }
    });
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

/// Startet die Ueberwachung in einem eigenen Faden. `cb` bekommt jeden Text, den
/// der Benutzer auf der Windows-Seite kopiert hat; eigene Schreibvorgaenge aus
/// `set` sind bereits herausgefiltert.
pub fn watch(cb: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        SINK.with(|s| *s.borrow_mut() = Some(Box::new(cb)));
        if let Err(e) = run_listener() {
            eprintln!("Zwischenablage: Ueberwachung nicht gestartet: {e}");
        }
    });
}
