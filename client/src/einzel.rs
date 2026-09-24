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
// Benutzerereignis an die Ereignisschleife von winit weiter.

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

/// Nutzlast fuer die Weitergabe. None, wenn die Adresse zu lang ist.
pub fn nutzlast(adresse: &str) -> Option<Vec<u8>> {
    let a = adresse.trim().as_bytes();
    if KENNUNG.len() + a.len() > NUTZLAST_MAX {
        return None;
    }
    let mut v = Vec::with_capacity(KENNUNG.len() + a.len());
    v.extend_from_slice(KENNUNG);
    v.extend_from_slice(a);
    Some(v)
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
    /// Fenster bzw. Socket; weitergereichte Adressen kommen ueber `eingang`.
    Erste { waechter: Waechter, eingang: mpsc::Receiver<String> },
    /// Eine andere Instanz laeuft und hat die Nutzlast angenommen.
    Weitergereicht,
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
        AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, PostMessageW,
        PostQuitMessage, RegisterClassW, SendMessageTimeoutW, TranslateMessage, ASFW_ANY, MSG, SMTO_ABORTIFHUNG, WM_CLOSE,
        WM_COPYDATA, WM_DESTROY, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
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
        static EINGANG: RefCell<Option<mpsc::Sender<String>>> = const { RefCell::new(None) };
    }

    /// Nimmt WM_COPYDATA an: Antwort 1 = angenommen, 0 = verworfen.
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
                let ok = EINGANG.with(|e| e.borrow().as_ref().map(|t| t.send(a).is_ok()).unwrap_or(false));
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
    /// die findet FindWindowW nicht), ohne Eintrag in der Taskleiste.
    fn fenster_anlegen(klasse: &str) -> Result<HWND, String> {
        unsafe {
            let hinst = HINSTANCE::from(GetModuleHandleW(PCWSTR::null()).map_err(|e| format!("Modulkennung: {}", e.message()))?);
            let name = HSTRING::from(klasse);
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: PCWSTR(name.as_ptr()), ..Default::default() };
            // Schon registriert (zweiter Waechter im selben Prozess, Tests):
            // kein Fehler, das Fenster laesst sich trotzdem anlegen.
            RegisterClassW(&wc);
            CreateWindowExW(WS_EX_TOOLWINDOW, &name, &name, WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None)
                .map_err(|e| format!("Fenster {klasse}: {}", e.message()))
        }
    }

    fn fenster_faden(klasse: String, tx: mpsc::Sender<String>, bereit: mpsc::SyncSender<Result<isize, String>>) {
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

    /// Erste Instanz werden oder an die erste weiterreichen.
    pub fn beanspruchen_mit(namen: &Namen, adresse: &str, warten: Duration) -> Start {
        let mutex = match unsafe { CreateMutexW(None, false, &HSTRING::from(namen.mutex.as_str())) } {
            Ok(h) => h,
            Err(e) => return Start::Ohne(format!("Mutex {}: {}", namen.mutex, e.message())),
        };
        // Gibt es ihn schon, liefert CreateMutexW trotzdem einen Griff und
        // setzt ERROR_ALREADY_EXISTS.
        let schon = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if !schon {
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
            return match ergebnis {
                Ok(fenster) => Start::Erste { waechter: Waechter { mutex: mutex.0 as isize, fenster }, eingang: rx },
                Err(e) => {
                    // Ohne Fenster kaeme keine Weitergabe an: den Mutex
                    // freigeben, sonst wartete jede weitere Instanz umsonst.
                    unsafe {
                        let _ = CloseHandle(mutex);
                    }
                    Start::Ohne(e)
                }
            };
        }
        unsafe {
            let _ = CloseHandle(mutex);
        }
        weiterreichen(&namen.klasse, adresse, warten)
    }

    fn weiterreichen(klasse: &str, adresse: &str, warten: Duration) -> Start {
        // Eine zu lange Adresse kann nicht mit: dann nur nach vorn holen.
        let daten = nutzlast(adresse).unwrap_or_else(|| KENNUNG.to_vec());
        // Dieser Prozess kam gerade durch den Nutzer zustande und darf den
        // Vordergrund setzen; das Recht geht an die erste Instanz weiter.
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
        }
        let klasse_w = HSTRING::from(klasse);
        let ende = Instant::now() + warten;
        loop {
            if let Ok(h) = unsafe { FindWindowW(&klasse_w, PCWSTR::null()) } {
                if senden(h, &daten) == Some(1) {
                    return Start::Weitergereicht;
                }
            }
            if Instant::now() >= ende {
                return Start::Unerreichbar(format!("keine Antwort von {klasse} binnen {} s", warten.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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

    pub fn beanspruchen_mit(namen: &Namen, adresse: &str, warten: Duration) -> Start {
        let sperre = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&namen.sperre) {
            Ok(f) => f,
            Err(e) => return Start::Ohne(format!("{}: {e}", namen.sperre.display())),
        };
        match sperre.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return weiterreichen(namen, adresse, warten),
            Err(std::fs::TryLockError::Error(e)) => return Start::Ohne(format!("Sperre {}: {e}", namen.sperre.display())),
        }
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
                let Ok(mut s) = s else { continue };
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                let mut b = Vec::new();
                // Etwas mehr als erlaubt lesen, damit eine zu lange Nutzlast ganz
                // abgenommen und dann verworfen wird.
                let _ = (&mut s).take(4096).read_to_end(&mut b);
                let ok = match nutzlast_lesen(&b) {
                    Some(a) => tx.send(a).is_ok(),
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

    fn weiterreichen(namen: &Namen, adresse: &str, warten: Duration) -> Start {
        let daten = nutzlast(adresse).unwrap_or_else(|| KENNUNG.to_vec());
        let ende = Instant::now() + warten;
        loop {
            if senden(&namen.socket, &daten) == Some(1) {
                return Start::Weitergereicht;
            }
            if Instant::now() >= ende {
                return Start::Unerreichbar(format!("keine Antwort ueber {} binnen {} s", namen.socket.display(), warten.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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

    #[test]
    fn nutzlast_hin_und_zurueck() {
        assert_eq!(nutzlast("10.0.0.5:9001").unwrap(), b"QCE110.0.0.5:9001");
        assert_eq!(nutzlast_lesen(b"QCE110.0.0.5:9001").as_deref(), Some("10.0.0.5:9001"));
        // Leer heisst: nur nach vorn holen.
        assert_eq!(nutzlast("").unwrap(), b"QCE1");
        assert_eq!(nutzlast_lesen(b"QCE1").as_deref(), Some(""));
        // Leerraum aussen faellt weg, UTF-8 bleibt.
        assert_eq!(nutzlast_lesen(nutzlast("  büro.local:9001 ").as_deref().unwrap()).as_deref(), Some("büro.local:9001"));
        // Grenze: genau 512 Byte gehen, 513 nicht - in beide Richtungen.
        let a508 = "a".repeat(NUTZLAST_MAX - 4);
        let n = nutzlast(&a508).unwrap();
        assert_eq!(n.len(), NUTZLAST_MAX);
        assert_eq!(nutzlast_lesen(&n).as_deref(), Some(a508.as_str()));
        assert_eq!(nutzlast(&"a".repeat(NUTZLAST_MAX - 3)), None);
        let mut zu_lang = b"QCE1".to_vec();
        zu_lang.extend(std::iter::repeat_n(b'a', NUTZLAST_MAX - 3));
        assert_eq!(nutzlast_lesen(&zu_lang), None);
        // Falsche Kennung, zu kurz, kein UTF-8, Steuerzeichen.
        assert_eq!(nutzlast_lesen(b"QCE2x"), None);
        assert_eq!(nutzlast_lesen(b"QCE"), None);
        assert_eq!(nutzlast_lesen(b""), None);
        assert_eq!(nutzlast_lesen(b"QCE1\xff\xfe"), None);
        assert_eq!(nutzlast_lesen(b"QCE1a\nb"), None);
        assert_eq!(nutzlast_lesen(b"QCE1a\0b"), None);
    }

    /// Unter Windows: Mutex und WM_COPYDATA zwischen zwei Faeden, mit eigenen
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
        let Start::Erste { waechter, eingang } = beanspruchen_mit(&namen, "", Duration::from_secs(2)) else {
            panic!("erste Instanz erwartet");
        };
        // Der zweite Start in einem anderen Faden: dieselben Namen.
        for adresse in ["10.0.0.5:9001", ""] {
            let n = namen.clone();
            let a = adresse.to_string();
            let zweiter = std::thread::spawn(move || matches!(beanspruchen_mit(&n, &a, Duration::from_secs(5)), Start::Weitergereicht));
            assert!(zweiter.join().unwrap(), "nicht weitergereicht: {adresse:?}");
            assert_eq!(eingang.recv_timeout(Duration::from_secs(5)).unwrap(), adresse);
        }
        // Falsche Kennung und Uebergroesse: Antwort 0, nichts kommt an.
        let h = unsafe { FindWindowW(&HSTRING::from(namen.klasse.as_str()), PCWSTR::null()) }.unwrap();
        assert_eq!(win::senden(h, b"XXXX10.0.0.5:9001"), Some(0));
        let mut gross = b"QCE1".to_vec();
        gross.extend(std::iter::repeat_n(b'a', NUTZLAST_MAX));
        assert_eq!(win::senden(h, &gross), Some(0));
        assert_eq!(win::senden(h, b"QCE110.0.0.6:9001"), Some(1));
        assert_eq!(eingang.recv_timeout(Duration::from_secs(5)).unwrap(), "10.0.0.6:9001");
        assert!(eingang.try_recv().is_err());
        // Waechter weg: Mutex frei, der naechste Start ist wieder der erste.
        drop(waechter);
        std::thread::sleep(Duration::from_millis(200));
        assert!(matches!(beanspruchen_mit(&namen, "", Duration::from_secs(1)), Start::Erste { .. }));
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
        let Start::Erste { waechter, eingang } = beanspruchen_mit(&namen, "", Duration::from_secs(2)) else {
            panic!("erste Instanz erwartet");
        };
        for adresse in ["10.0.0.5:9001", ""] {
            let n = namen.clone();
            let a = adresse.to_string();
            let zweiter = std::thread::spawn(move || matches!(beanspruchen_mit(&n, &a, Duration::from_secs(5)), Start::Weitergereicht));
            assert!(zweiter.join().unwrap(), "nicht weitergereicht: {adresse:?}");
            assert_eq!(eingang.recv_timeout(Duration::from_secs(5)).unwrap(), adresse);
        }
        assert_eq!(mac::senden(&namen.socket, b"XXXX10.0.0.5:9001"), Some(0));
        let mut gross = b"QCE1".to_vec();
        gross.extend(std::iter::repeat_n(b'a', NUTZLAST_MAX));
        assert_eq!(mac::senden(&namen.socket, &gross), Some(0));
        assert!(eingang.try_recv().is_err());
        // Waechter weg: Sperre frei, Socket-Datei weg, der naechste ist erster.
        drop(waechter);
        assert!(!namen.socket.exists());
        let wieder = beanspruchen_mit(&namen, "", Duration::from_secs(1));
        assert!(matches!(wieder, Start::Erste { .. }));
        drop(wieder);
        let _ = std::fs::remove_dir_all(&ordner);
    }
}
