// Einzelinstanz: hoechstens ein Client mit Fenster je Nutzersitzung.
//
// Startet ein zweiter (etwa per Doppelklick auf eine Desktop-Verknuepfung,
// waehrend die App schon laeuft), reicht er seine Adresse an den ersten
// weiter und endet. Der erste holt sein Fenster nach vorn und verbindet.
// Ohne das gaebe es zwei Clients mit zwei Symbolen, doppeltem
// Ablagewaechter und belegtem Bekanntgabe-Port - und die zweite Instanz
// wuerde die laufende Sitzung zum selben Host abloesen.
//
// Nutzlast: "QCE1" + Adresse in UTF-8 (leer = nur nach vorn holen), hoechstens
// 512 Byte; alles andere wird verworfen.
//
// Windows: ein benannter Mutex (Local\QuadChroma-Client) erkennt die erste
// Instanz. Sie betreibt in einem eigenen Faden ein nie sichtbares Fenster
// der Klasse QuadChromaEinzel; die zweite findet es mit FindWindowW und
// schickt WM_COPYDATA, nachdem sie mit AllowSetForegroundWindow(ASFW_ANY)
// ihr Recht auf den Vordergrund weitergegeben hat.
//
// macOS: ein Unix-Socket <config_dir>/einzel.sock (Ordner 0700). Wer die
// erste Instanz ist, entscheidet eine Sperre auf <config_dir>/einzel.lock
// (flock, faellt mit dem Prozess): der Halter entfernt eine verwaiste
// Socket-Datei und legt sie neu an, jeder andere verbindet und schickt die
// Nutzlast. Die Sperre schliesst den Wettlauf zweier gleichzeitiger Starts
// aus, bei dem sonst beide die Datei des anderen loeschen koennten.
//
// Was ankommt, geht ueber einen Kanal hinaus; main.rs reicht es als
// Benutzerereignis an die Ereignisschleife von winit weiter. Angenommen
// (Antwort 1) ist eine Adresse erst, wenn die Ereignisschleife sie uebernommen
// hat (Weitergabe::bestaetigen) - nicht schon, wenn sie im Kanal liegt: beim
// Beenden der ersten Instanz laeuft die Schleife nicht mehr, und die Adresse
// ginge verloren, obwohl der zweite Start "weitergereicht" meldete
// (Durchsicht [3]). Wer wartet, versucht in jeder Runde auch selbst die erste
// Instanz zu werden: endet die erste gerade, uebernimmt der neue Start, statt
// nach WARTEN aufzugeben.

use std::sync::mpsc;
use std::time::Duration;
#[cfg(any(windows, target_os = "macos"))]
use std::time::Instant;

/// Kennung am Anfang jeder Nutzlast (Fassung 1).
pub const KENNUNG: &[u8; 4] = b"QCE1";
/// Hoechstlaenge der ganzen Nutzlast samt Kennung.
pub const NUTZLAST_MAX: usize = 512;
/// So lange sucht eine zweite Instanz die erste, bevor sie aufgibt. Die
/// erste legt Fenster bzw. Socket kurz nach dem Mutex an; dazwischen
/// kann ein Start fallen.
pub const WARTEN: Duration = Duration::from_secs(5);
/// So lange wartet die erste Instanz, bis ihre Ereignisschleife eine
/// Weitergabe uebernommen hat; danach antwortet sie 0. Kuerzer als die
/// Frist des Absenders (3 s), damit er die Antwort noch liest.
pub const UEBERNAHME: Duration = Duration::from_secs(2);

/// Nutzlast fuer die Weitergabe. Err (mit Grund), wenn die Adresse so nicht
/// ankaeme: zu lang oder mit Steuerzeichen - die verwirft der Empfaenger
/// (nutzlast_lesen), und der Absender wartete sonst umsonst auf eine Antwort.
pub fn nutzlast(adresse: &str) -> Result<Vec<u8>, String> {
    let a = adresse.trim();
    if KENNUNG.len() + a.len() > NUTZLAST_MAX {
        return Err(format!("Adresse laenger als {} Byte", NUTZLAST_MAX - KENNUNG.len()));
    }
    if a.chars().any(|c| c.is_control()) {
        return Err("Adresse mit Steuerzeichen".into());
    }
    let mut v = Vec::with_capacity(KENNUNG.len() + a.len());
    v.extend_from_slice(KENNUNG);
    v.extend_from_slice(a.as_bytes());
    Ok(v)
}

/// Eine weitergereichte Adresse (leer = nur nach vorn holen). Die
/// Ereignisschleife bestaetigt die Uebernahme; faellt die Weitergabe
/// unbestaetigt weg (Schleife beendet), antwortet die erste Instanz 0.
#[derive(Debug)]
pub struct Weitergabe {
    pub adresse: String,
    quittung: mpsc::SyncSender<()>,
}

impl Weitergabe {
    /// Uebernommen: der zweite Start bekommt 1 und endet.
    pub fn bestaetigen(self) {
        let _ = self.quittung.send(());
    }
}

/// Eine Adresse an die Ereignisschleife geben und auf die Uebernahme warten
/// (hoechstens `frist`). true = uebernommen.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn annehmen(tx: &mpsc::Sender<Weitergabe>, adresse: String, frist: Duration) -> bool {
    let (quittung, rx) = mpsc::sync_channel(1);
    if tx.send(Weitergabe { adresse, quittung }).is_err() {
        return false;
    }
    rx.recv_timeout(frist).is_ok()
}

/// Nutzlast lesen: die Adresse (leer = nur nach vorn holen), oder None bei
/// falscher Kennung, zu grosser Laenge, ungueltigem UTF-8 oder Steuerzeichen.
pub fn nutzlast_lesen(b: &[u8]) -> Option<String> {
    if b.len() > NUTZLAST_MAX || b.len() < KENNUNG.len() || &b[..KENNUNG.len()] != KENNUNG {
        return None;
    }
    let a = std::str::from_utf8(&b[KENNUNG.len()..]).ok()?;
    if a.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(a.trim().to_string())
}

/// Ergebnis des Starts.
pub enum Start {
    /// Diese Instanz ist die erste. Der Waechter haelt Mutex bzw. Sperre und
    /// Fenster bzw. Socket; weitergereichte Adressen kommen ueber `eingang`
    /// und wollen bestaetigt sein (Weitergabe::bestaetigen).
    Erste { waechter: Waechter, eingang: mpsc::Receiver<Weitergabe> },
    /// Eine andere Instanz laeuft und hat die Nutzlast uebernommen. Mit
    /// Grund: die Adresse war so nicht weiterzugeben (nutzlast), die andere
    /// Instanz ist nur nach vorn geholt.
    Weitergereicht(Option<String>),
    /// Einzelinstanz nicht moeglich (Grund) - normal weiterlaufen.
    Ohne(String),
    /// Eine andere Instanz laeuft, hat aber nicht geantwortet (Grund).
    Unerreichbar(String),
}

/// Mit den Namen dieser Plattform beanspruchen.
pub fn beanspruchen(adresse: &str) -> Start {
    #[cfg(windows)]
    {
        beanspruchen_mit(&Namen::vorgabe(), adresse, WARTEN)
    }
    #[cfg(target_os = "macos")]
    {
        match Namen::vorgabe() {
            Ok(n) => beanspruchen_mit(&n, adresse, WARTEN),
            Err(e) => Start::Ohne(e),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = adresse;
        Start::Ohne("Einzelinstanz gibt es nur unter Windows und macOS".into())
    }
}

/// Laeuft in dieser Sitzung schon eine erste Instanz? Fragt nur, schickt
/// ihr nichts - fuer Starts im Hintergrund (Autostart, --host): sie enden
/// dann still, statt das Fenster der laufenden zu oeffnen. Auf dem Mac gibt
/// es diese Starts nicht (false).
pub fn laeuft_schon() -> bool {
    #[cfg(windows)]
    {
        win::laeuft(&Namen::vorgabe())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
pub use win::{beanspruchen_mit, Namen, Waechter};

#[cfg(windows)]
mod win {
    use super::*;
    use std::cell::RefCell;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW,
        GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW, SendMessageTimeoutW, TranslateMessage, ASFW_ANY, MSG,
        MSGFLT_ALLOW, SMTO_ABORTIFHUNG, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    /// Namen von Mutex und Fensterklasse. Tests nehmen eigene, damit sie nie
    /// an eine laufende App geraten.
    #[derive(Clone, Debug)]
    pub struct Namen {
        pub mutex: String,
        pub klasse: String,
    }

    impl Namen {
        pub fn vorgabe() -> Namen {
            Namen { mutex: "Local\\QuadChroma-Client".into(), klasse: "QuadChromaEinzel".into() }
        }
    }

    /// Haelt die erste Instanz: den Mutex und das Fenster. Beim Fallenlassen
    /// geht das Fenster zu (sein Faden endet) und der Mutex wird frei.
    pub struct Waechter {
        mutex: isize,
        fenster: isize,
    }

    impl Drop for Waechter {
        fn drop(&mut self) {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.fenster as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                let _ = CloseHandle(HANDLE(self.mutex as *mut _));
            }
        }
    }

    thread_local! {
        /// Der Kanal zur Ereignisschleife - je Fensterfaden einer.
        static EINGANG: RefCell<Option<mpsc::Sender<Weitergabe>>> = const { RefCell::new(None) };
    }

    /// Nimmt WM_COPYDATA an: Antwort 1 = von der Ereignisschleife
    /// uebernommen, 0 = verworfen oder nicht uebernommen (annehmen).
    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_COPYDATA => {
                let cds = lp.0 as *const COPYDATASTRUCT;
                if cds.is_null() {
                    return LRESULT(0);
                }
                // Die Daten gehoeren dem Absender und gelten nur waehrend
                // dieses Aufrufs - also sofort kopieren (nutzlast_lesen).
                let (n, p) = unsafe { ((*cds).cbData as usize, (*cds).lpData as *const u8) };
                if p.is_null() || n > NUTZLAST_MAX {
                    return LRESULT(0);
                }
                let b = unsafe { std::slice::from_raw_parts(p, n) };
                let Some(a) = nutzlast_lesen(b) else { return LRESULT(0) };
                // Den Kanal klonen statt die Ausleihe ueber das Warten zu halten.
                let tx = EINGANG.with(|e| e.borrow().clone());
                let ok = tx.is_some_and(|t| annehmen(&t, a, UEBERNAHME));
                LRESULT(ok as isize)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    /// Ein nie sichtbares Fenster oberster Ebene (kein Nachrichtenfenster:
    /// die findet FindWindowW nicht), ohne Eintrag in der Taskleiste. Laeuft
    /// die erste Instanz erhoeht, liesse UIPI WM_COPYDATA eines gewoehnlich
    /// gestarteten zweiten nicht durch - also fuer dieses Fenster erlauben
    /// (die Nutzlast wird ohnehin wie feindlich geprueft, nutzlast_lesen).
    fn fenster_anlegen(klasse: &str) -> Result<HWND, String> {
        unsafe {
            let hinst = HINSTANCE::from(GetModuleHandleW(PCWSTR::null()).map_err(|e| format!("Modulkennung: {}", e.message()))?);
            let name = HSTRING::from(klasse);
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: PCWSTR(name.as_ptr()), ..Default::default() };
            // Schon registriert (zweiter Waechter im selben Prozess, Tests):
            // kein Fehler, das Fenster laesst sich trotzdem anlegen.
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, &name, &name, WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None)
                .map_err(|e| format!("Fenster {klasse}: {}", e.message()))?;
            // Scheitert das, bleibt es beim Filter des Systems: ein Randfall
            // (nur bei erhoehter erster Instanz), kein Grund aufzugeben.
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
            Ok(hwnd)
        }
    }

    fn fenster_faden(klasse: String, tx: mpsc::Sender<Weitergabe>, bereit: mpsc::SyncSender<Result<isize, String>>) {
        EINGANG.with(|e| *e.borrow_mut() = Some(tx));
        let hwnd = match fenster_anlegen(&klasse) {
            Ok(h) => h,
            Err(e) => {
                let _ = bereit.send(Err(e));
                return;
            }
        };
        let _ = bereit.send(Ok(hwnd.0 as isize));
        let mut msg = MSG::default();
        loop {
            // 0 = WM_QUIT, -1 = Fehler: beides beendet die Schleife.
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 <= 0 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        EINGANG.with(|e| *e.borrow_mut() = None);
    }

    /// Erste Instanz werden oder an die erste weiterreichen. Wer weiterreicht,
    /// versucht in jeder Runde auch selbst die erste zu werden: endet die
    /// erste gerade, uebernimmt dieser Start (Durchsicht [3]).
    pub fn beanspruchen_mit(namen: &Namen, adresse: &str, warten: Duration) -> Start {
        // Eine Adresse, die so nicht ankaeme: nur nach vorn holen, und sagen
        // warum (statt WARTEN lang auf eine Antwort zu hoffen).
        let (daten, verworfen) = match nutzlast(adresse) {
            Ok(d) => (d, None),
            Err(g) => (KENNUNG.to_vec(), Some(g)),
        };
        let klasse_w = HSTRING::from(namen.klasse.as_str());
        let ende = Instant::now() + warten;
        let mut vordergrund = false;
        loop {
            if let Some(start) = erste_werden(namen) {
                return start;
            }
            if !vordergrund {
                // Dieser Prozess kam gerade durch den Nutzer zustande und darf
                // den Vordergrund setzen; das Recht geht an die erste Instanz.
                unsafe {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                }
                vordergrund = true;
            }
            if let Ok(h) = unsafe { FindWindowW(&klasse_w, PCWSTR::null()) } {
                if senden(h, &daten) == Some(1) {
                    return Start::Weitergereicht(verworfen);
                }
            }
            if Instant::now() >= ende {
                return Start::Unerreichbar(format!("keine Antwort von {} binnen {} s", namen.klasse, warten.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Den Mutex anlegen. Some(Erste bzw. Ohne), wenn dieser Start die erste
    /// Instanz ist (oder die Einzelinstanz nicht geht); None, wenn eine andere
    /// den Mutex haelt.
    /// Haelt jemand den Mutex dieser Namen (die erste Instanz)? Verweigert
    /// das System den Zugriff (erste Instanz mit erhoehten Rechten), besteht
    /// er ebenfalls.
    pub fn laeuft(namen: &Namen) -> bool {
        use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
        use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(namen.mutex.as_str())) } {
            Ok(h) => {
                unsafe {
                    let _ = CloseHandle(h);
                }
                true
            }
            Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
        }
    }

    fn erste_werden(namen: &Namen) -> Option<Start> {
        let mutex = match unsafe { CreateMutexW(None, false, &HSTRING::from(namen.mutex.as_str())) } {
            Ok(h) => h,
            Err(e) => return Some(Start::Ohne(format!("Mutex {}: {}", namen.mutex, e.message()))),
        };
        // Gibt es ihn schon, liefert CreateMutexW trotzdem einen Griff und
        // setzt ERROR_ALREADY_EXISTS.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let (bereit_tx, bereit_rx) = mpsc::sync_channel(1);
        let klasse = namen.klasse.clone();
        let faden = std::thread::Builder::new()
            .name("einzelinstanz".into())
            .spawn(move || fenster_faden(klasse, tx, bereit_tx));
        let ergebnis = match faden {
            Ok(_) => bereit_rx.recv().unwrap_or_else(|_| Err("Fensterfaden beendet".into())),
            Err(e) => Err(format!("Faden: {e}")),
        };
        Some(match ergebnis {
            Ok(fenster) => Start::Erste { waechter: Waechter { mutex: mutex.0 as isize, fenster }, eingang: rx },
            Err(e) => {
                // Ohne Fenster kaeme keine Weitergabe an: den Mutex
                // freigeben, sonst wartete jede weitere Instanz umsonst.
                unsafe {
                    let _ = CloseHandle(mutex);
                }
                Start::Ohne(e)
            }
        })
    }

    /// WM_COPYDATA an das Fenster; die Antwort, oder None bei Zeitablauf.
    pub(super) fn senden(h: HWND, daten: &[u8]) -> Option<usize> {
        let cds = COPYDATASTRUCT { dwData: 0x5143_4531, cbData: daten.len() as u32, lpData: daten.as_ptr() as *mut _ };
        let mut antwort = 0usize;
        let r = unsafe {
            SendMessageTimeoutW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 3000, Some(&mut antwort))
        };
        (r.0 != 0).then_some(antwort)
    }
}

// ------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
pub use mac::{beanspruchen_mit, Namen, Waechter};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Socket und Sperrdatei. Tests nehmen einen eigenen Temp-Ordner.
    #[derive(Clone, Debug)]
    pub struct Namen {
        pub socket: PathBuf,
        pub sperre: PathBuf,
    }

    impl Namen {
        pub fn vorgabe() -> Result<Namen, String> {
            let d = crate::secure::config_dir()?;
            Ok(Namen { socket: d.join("einzel.sock"), sperre: d.join("einzel.lock") })
        }
    }

    /// Haelt die erste Instanz: die Sperre und den lauschenden Socket. Beim
    /// Fallenlassen endet der Faden, und die Socket-Datei geht weg.
    pub struct Waechter {
        _sperre: std::fs::File,
        socket: PathBuf,
        halt: Arc<AtomicBool>,
    }

    impl Drop for Waechter {
        fn drop(&mut self) {
            self.halt.store(true, Ordering::SeqCst);
            // Das blockierende accept aufwecken.
            let _ = UnixStream::connect(&self.socket);
            let _ = std::fs::remove_file(&self.socket);
        }
    }

    /// Erste Instanz werden oder an die erste weiterreichen. Wer weiterreicht,
    /// versucht in jeder Runde auch selbst die Sperre zu bekommen: endet die
    /// erste gerade, uebernimmt dieser Start (Durchsicht [3]).
    pub fn beanspruchen_mit(namen: &Namen, adresse: &str, warten: Duration) -> Start {
        let sperre = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&namen.sperre) {
            Ok(f) => f,
            Err(e) => return Start::Ohne(format!("{}: {e}", namen.sperre.display())),
        };
        // Eine Adresse, die so nicht ankaeme: nur nach vorn holen, und sagen
        // warum (statt WARTEN lang auf eine Antwort zu hoffen).
        let (daten, verworfen) = match nutzlast(adresse) {
            Ok(d) => (d, None),
            Err(g) => (KENNUNG.to_vec(), Some(g)),
        };
        let ende = Instant::now() + warten;
        loop {
            match sperre.try_lock() {
                Ok(()) => return erste(namen, sperre),
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(e)) => {
                    return Start::Ohne(format!("Sperre {}: {e}", namen.sperre.display()))
                }
            }
            if senden(&namen.socket, &daten) == Some(1) {
                return Start::Weitergereicht(verworfen);
            }
            if Instant::now() >= ende {
                return Start::Unerreichbar(format!(
                    "keine Antwort ueber {} binnen {} s",
                    namen.socket.display(),
                    warten.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Die Sperre ist unser: Socket anlegen und den Lauscher starten.
    fn erste(namen: &Namen, sperre: std::fs::File) -> Start {
        // Wir sind die erste Instanz: eine Socket-Datei, die noch daliegt,
        // ist verwaist (ihr Prozess ist weg, sonst hielte er die Sperre).
        let _ = std::fs::remove_file(&namen.socket);
        let lauscher = match UnixListener::bind(&namen.socket) {
            Ok(l) => l,
            Err(e) => return Start::Ohne(format!("{}: {e}", namen.socket.display())),
        };
        let (tx, rx) = mpsc::channel();
        let halt = Arc::new(AtomicBool::new(false));
        let h = halt.clone();
        let faden = std::thread::Builder::new().name("einzelinstanz".into()).spawn(move || {
            for s in lauscher.incoming() {
                if h.load(Ordering::SeqCst) {
                    break;
                }
                // Ein dauerhafter Fehler (etwa keine Deskriptoren mehr) gaebe
                // sonst eine Schleife unter Volllast: kurz warten.
                let Ok(mut s) = s else {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                let mut b = Vec::new();
                // Etwas mehr als erlaubt lesen, damit eine zu lange Nutzlast ganz
                // abgenommen und dann verworfen wird.
                let _ = (&mut s).take(4096).read_to_end(&mut b);
                let ok = match nutzlast_lesen(&b) {
                    Some(a) => annehmen(&tx, a, UEBERNAHME),
                    None => false,
                };
                let _ = s.write_all(&[ok as u8]);
            }
        });
        if let Err(e) = faden {
            let _ = std::fs::remove_file(&namen.socket);
            return Start::Ohne(format!("Faden: {e}"));
        }
        Start::Erste { waechter: Waechter { _sperre: sperre, socket: namen.socket.clone(), halt }, eingang: rx }
    }

    /// Nutzlast schicken, Schreibseite schliessen, ein Byte Antwort lesen.
    pub(super) fn senden(socket: &std::path::Path, daten: &[u8]) -> Option<u8> {
        let mut s = UnixStream::connect(socket).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        s.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
        s.write_all(daten).ok()?;
        s.shutdown(std::net::Shutdown::Write).ok()?;
        let mut a = [0u8; 1];
        s.read_exact(&mut a).ok()?;
        Some(a[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(windows, target_os = "macos"))]
    use std::sync::atomic::{AtomicU8, Ordering};
    #[cfg(any(windows, target_os = "macos"))]
    use std::sync::Arc;

    #[test]
    fn nutzlast_hin_und_zurueck() {
        assert_eq!(nutzlast("10.0.0.5:9001").unwrap(), b"QCE110.0.0.5:9001");
        assert_eq!(nutzlast_lesen(b"QCE110.0.0.5:9001").as_deref(), Some("10.0.0.5:9001"));
        // Leer heisst: nur nach vorn holen.
        assert_eq!(nutzlast("").unwrap(), b"QCE1");
        assert_eq!(nutzlast_lesen(b"QCE1").as_deref(), Some(""));
        // Leerraum aussen faellt weg, UTF-8 bleibt.
        assert_eq!(nutzlast_lesen(&nutzlast("  büro.local:9001 ").unwrap()).as_deref(), Some("büro.local:9001"));
        // Grenze: genau 512 Byte gehen, 513 nicht - in beide Richtungen.
        let a508 = "a".repeat(NUTZLAST_MAX - 4);
        let n = nutzlast(&a508).unwrap();
        assert_eq!(n.len(), NUTZLAST_MAX);
        assert_eq!(nutzlast_lesen(&n).as_deref(), Some(a508.as_str()));
        assert!(nutzlast(&"a".repeat(NUTZLAST_MAX - 3)).is_err());
        let mut zu_lang = b"QCE1".to_vec();
        zu_lang.extend(std::iter::repeat_n(b'a', NUTZLAST_MAX - 3));
        assert_eq!(nutzlast_lesen(&zu_lang), None);
        // Steuerzeichen verwirft schon der Absender, mit Grund - sie kaemen
        // beim Empfaenger ohnehin nicht an.
        for a in ["10.0.0.5\n:9001", "a\u{0}b", "x\u{7f}", "a\u{85}b"] {
            let e = nutzlast(a).expect_err(a);
            assert!(e.contains("Steuerzeichen"), "{a:?}: {e}");
            assert_eq!(nutzlast_lesen(&[KENNUNG.as_slice(), a.as_bytes()].concat()), None, "{a:?}");
        }
        // Falsche Kennung, zu kurz, kein UTF-8, Steuerzeichen.
        assert_eq!(nutzlast_lesen(b"QCE2x"), None);
        assert_eq!(nutzlast_lesen(b"QCE"), None);
        assert_eq!(nutzlast_lesen(b""), None);
        assert_eq!(nutzlast_lesen(b"QCE1\xff\xfe"), None);
        assert_eq!(nutzlast_lesen(b"QCE1a\nb"), None);
        assert_eq!(nutzlast_lesen(b"QCE1a\0b"), None);
    }

    /// Wie die Ereignisschleife mit Weitergaben umgeht (Modus): 0 bestaetigen
    /// und dem Test melden, 1 liegen lassen (unbestaetigt, bis zur Frist), 2
    /// unbestaetigt fallen lassen (die Schleife ist zu), 3 ganz aufhoeren
    /// (die Weiterleitung endet, der Kanal ist zu).
    #[cfg(any(windows, target_os = "macos"))]
    fn schleife(eingang: mpsc::Receiver<Weitergabe>) -> (Arc<AtomicU8>, mpsc::Receiver<String>) {
        let modus = Arc::new(AtomicU8::new(0));
        let (tx, rx) = mpsc::channel();
        let m = modus.clone();
        std::thread::spawn(move || {
            let mut liegen = Vec::new();
            loop {
                let Ok(w) = eingang.recv_timeout(Duration::from_millis(20)) else {
                    if m.load(Ordering::SeqCst) == 3 {
                        return;
                    }
                    continue;
                };
                match m.load(Ordering::SeqCst) {
                    0 => {
                        let a = w.adresse.clone();
                        w.bestaetigen();
                        let _ = tx.send(a);
                    }
                    1 => liegen.push(w),
                    _ => drop(w),
                }
            }
        });
        (modus, rx)
    }

    /// Die Pruefungen der Durchsicht [3], fuer beide Plattformen: angenommen
    /// ist nur, was die Schleife uebernimmt; unbestaetigt heisst 0 (nach der
    /// Frist bzw. sofort, wenn die Schleife weg ist); ein Start, der waehrend
    /// des Beendens wartet, wird selbst erste Instanz, sobald der Waechter
    /// faellt - deutlich vor WARTEN. `senden` schickt eine rohe Nutzlast an
    /// die erste Instanz und liefert die Antwort.
    #[cfg(any(windows, target_os = "macos"))]
    fn einzel_ablauf(namen: Namen, senden: impl Fn(&[u8]) -> Option<usize>) {
        let Start::Erste { waechter, eingang } = beanspruchen_mit(&namen, "", Duration::from_secs(2)) else {
            panic!("erste Instanz erwartet");
        };
        let (modus, uebernommen) = schleife(eingang);
        // Der zweite Start in einem anderen Faden: dieselben Namen.
        let zweiter = |adresse: &str| {
            let (n, a) = (namen.clone(), adresse.to_string());
            std::thread::spawn(move || match beanspruchen_mit(&n, &a, WARTEN) {
                Start::Weitergereicht(g) => Ok(g),
                Start::Erste { .. } => Err("erste".to_string()),
                Start::Unerreichbar(g) => Err(g),
                Start::Ohne(g) => Err(g),
            })
        };
        for adresse in ["10.0.0.5:9001", ""] {
            assert_eq!(zweiter(adresse).join().unwrap(), Ok(None), "nicht weitergereicht: {adresse:?}");
            assert_eq!(uebernommen.recv_timeout(Duration::from_secs(5)).unwrap(), adresse);
        }
        // Eine Adresse mit Steuerzeichen: nur nach vorn, mit Grund.
        let r = zweiter("10.0.0.5\t:9001").join().unwrap();
        assert!(matches!(&r, Ok(Some(g)) if g.contains("Steuerzeichen")), "{r:?}");
        assert_eq!(uebernommen.recv_timeout(Duration::from_secs(5)).unwrap(), "");
        // Falsche Kennung und Uebergroesse: Antwort 0, nichts kommt an.
        assert_eq!(senden(b"XXXX10.0.0.5:9001"), Some(0));
        let mut gross = b"QCE1".to_vec();
        gross.extend(std::iter::repeat_n(b'a', NUTZLAST_MAX));
        assert_eq!(senden(&gross), Some(0));
        assert_eq!(senden(b"QCE110.0.0.6:9001"), Some(1));
        assert_eq!(uebernommen.recv_timeout(Duration::from_secs(5)).unwrap(), "10.0.0.6:9001");
        assert!(uebernommen.try_recv().is_err());

        // Die Schleife uebernimmt nicht (liegen lassen): 0 nach der Frist.
        modus.store(1, Ordering::SeqCst);
        let t0 = Instant::now();
        assert_eq!(senden(b"QCE110.0.0.7:9001"), Some(0), "unbestaetigt angenommen");
        assert!(t0.elapsed() >= UEBERNAHME - Duration::from_millis(100), "Frist nicht abgewartet: {:?}", t0.elapsed());
        // Die Schleife ist zu (Weitergabe faellt unbestaetigt weg): gleich 0.
        modus.store(2, Ordering::SeqCst);
        let t0 = Instant::now();
        assert_eq!(senden(b"QCE110.0.0.8:9001"), Some(0), "nach dem Ende der Schleife angenommen");
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        // Die Weiterleitung endet ganz (Programmende laeuft): weiter 0.
        modus.store(3, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(senden(b"QCE110.0.0.9:9001"), Some(0));

        // Ein Start waehrend des Beendens wartet - und wird selbst erste
        // Instanz, sobald der Waechter faellt, statt WARTEN abzusitzen.
        let (n, t0) = (namen.clone(), Instant::now());
        let wartend = std::thread::spawn(move || match beanspruchen_mit(&n, "10.0.0.10:9001", WARTEN) {
            Start::Erste { waechter, .. } => Ok((t0.elapsed(), waechter)),
            Start::Weitergereicht(_) => Err("weitergereicht".to_string()),
            Start::Unerreichbar(g) | Start::Ohne(g) => Err(g),
        });
        std::thread::sleep(Duration::from_millis(400));
        drop(waechter);
        let (dauer, neu) = wartend.join().unwrap().expect("wartender Start wurde nicht erste Instanz");
        assert!(dauer < Duration::from_secs(3), "erst nach {dauer:?} erste Instanz");
        drop(neu);
    }

    /// Unter Windows: Mutex und WM_COPYDATA zwischen Faeden, mit eigenen
    /// Namen (nie die der laufenden App).
    #[cfg(windows)]
    #[test]
    fn einzel_wm_copydata_zwischen_zwei_faeden() {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        let namen = Namen {
            mutex: format!("Local\\QuadChroma-Test-{}", std::process::id()),
            klasse: format!("QuadChromaEinzelTest{}", std::process::id()),
        };
        let klasse = namen.klasse.clone();
        einzel_ablauf(namen.clone(), move |b| {
            let h = unsafe { FindWindowW(&HSTRING::from(klasse.as_str()), PCWSTR::null()) }.ok()?;
            win::senden(h, b)
        });
        // Alles weg: Mutex frei, der naechste Start ist wieder der erste.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!win::laeuft(&namen), "Mutex nach dem Ende noch da");
        let erste = beanspruchen_mit(&namen, "", Duration::from_secs(1));
        assert!(matches!(erste, Start::Erste { .. }));
        // Ein Start im Hintergrund fragt nur - und sieht die erste Instanz.
        assert!(win::laeuft(&namen));
        drop(erste);
        assert!(!win::laeuft(&namen));
    }

    /// Unter macOS: Socket und Sperre unter einem Temp-Ordner, eine
    /// verwaiste Socket-Datei wird ersetzt.
    #[cfg(target_os = "macos")]
    #[test]
    fn einzel_unix_socket_unter_temp() {
        use std::os::unix::fs::PermissionsExt;
        let ordner = std::env::temp_dir().join(format!("qc-einzel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        std::fs::set_permissions(&ordner, std::fs::Permissions::from_mode(0o700)).unwrap();
        let namen = Namen { socket: ordner.join("einzel.sock"), sperre: ordner.join("einzel.lock") };
        // Verwaist: gebunden und fallen gelassen - die Datei bleibt liegen,
        // Verbinden scheitert.
        drop(std::os::unix::net::UnixListener::bind(&namen.socket).unwrap());
        assert!(namen.socket.exists());
        assert_eq!(mac::senden(&namen.socket, b"QCE1"), None);
        let socket = namen.socket.clone();
        einzel_ablauf(namen.clone(), move |b| mac::senden(&socket, b).map(usize::from));
        // Alles weg: Sperre frei, Socket-Datei weg, der naechste ist erster.
        assert!(!namen.socket.exists());
        let wieder = beanspruchen_mit(&namen, "", Duration::from_secs(1));
        assert!(matches!(wieder, Start::Erste { .. }));
        drop(wieder);
        let _ = std::fs::remove_dir_all(&ordner);
    }
}
