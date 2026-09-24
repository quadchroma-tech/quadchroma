// Symbol im Infobereich (Windows), Spezifikation 3.9.
//
// Ein eigener Faden betreibt ein nie sichtbares Fenster oberster Ebene mit
// Nachrichtenschleife; an ihm haengt das Symbol (Shell_NotifyIconW). Ein
// Fenster oberster Ebene und kein Nachrichtenfenster (HWND_MESSAGE), weil
// nur jene die Rundnachricht "TaskbarCreated" bekommen: startet Explorer neu,
// meldet der Faden das Symbol neu an.
//
// Der Fensterfaden (winit) spricht mit diesem Faden nur ueber
// PostMessageW/SendMessageTimeoutW an dessen Fenster und ueber einen
// geteilten Stand; zurueck geht es ueber den Rueckruf `befehl`, den main.rs
// mit einem EventLoopProxy belegt. Keiner wartet je auf den anderen, ausser
// beim Anlegen (auf das Fenster ohne Frist - es haengt nicht am Explorer -,
// auf die Anmeldung hoechstens WARTEN), beim Entfernen (hoechstens WARTEN),
// bei der Sprechblase (hoechstens FRAGEN_HINWEIS) und beim Selbsttest
// (bewusst synchron). Beendet wird der Faden nur ueber WM_BEENDEN, nie ueber
// WM_CLOSE: das koennte auch von aussen kommen und liesse die abgelegte App
// ohne Symbol zurueck.
//
// Bedienung (NOTIFYICON_VERSION_4):
//   - Linksklick (NIN_SELECT), Eingabe/Leertaste (NIN_KEYSELECT) und
//     Doppelklick: Fenster zeigen.
//   - Rechtsklick bzw. Umschalt+F10 (WM_CONTEXTMENU): Kontextmenue nach
//     tray::menue, "Oeffnen" fett als Vorgabe. Vorher SetForegroundWindow,
//     sonst schliesst das Menue nicht beim Klick daneben; danach WM_NULL
//     (bekannte Eigenheit von TrackPopupMenu).
//   - Tooltip ueber NIF_TIP mit NIF_SHOWTIP (Fassung 4 blendet ihn sonst aus).
//   - Die einmalige Sprechblase ueber NIF_INFO, ebenfalls mit NIF_SHOWTIP
//     (jedes NIM_MODIFY ohne das Kennzeichen blendet den Tooltip aus).

use crate::tray::{self, Befehl, Punkt, Stand};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_TIMEOUT, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SendMessageTimeoutW, SetForegroundWindow, SetMenuDefaultItem, TrackPopupMenu, TranslateMessage,
    HICON, HMENU, LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, MSG, SMTO_ABORTIFHUNG, SM_CXSMICON, SM_MENUDROPALIGNMENT,
    TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
    WM_DESTROY, WM_LBUTTONDBLCLK, WM_NULL, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// Fensterklasse des Symbolfensters (eigene, nicht die der Einzelinstanz).
const KLASSE: PCWSTR = w!("QuadChromaSymbol");
/// Rueckruf des Infobereichs an unser Fenster.
const WM_SYMBOL: u32 = WM_APP + 1;
/// Tooltip aus dem geteilten Stand anwenden; Antwort 1 = angewandt.
const WM_TOOLTIP: u32 = WM_APP + 2;
/// Sprechblase aus dem geteilten Stand zeigen; Antwort 1 = gezeigt.
const WM_HINWEIS: u32 = WM_APP + 3;
/// Symbol entfernen (Selbsttest); Antwort 1 = entfernt.
const WM_ENTFERNEN: u32 = WM_APP + 4;
/// Symbol entfernen und den Faden beenden (Drop). Eigene Nachricht statt
/// WM_CLOSE, das auch von aussen kommen kann (siehe Kopf).
const WM_BEENDEN: u32 = WM_APP + 5;
/// Eingabe- oder Leertaste auf dem Symbol (NIN_SELECT | NINF_KEY).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
/// Kennung des Symbols an unserem Fenster.
const SYMBOL_ID: u32 = 1;
/// Befehlsnummern im Kontextmenue; Hosts ab ID_HOST + Index im Menue.
const ID_OEFFNEN: u32 = 1;
const ID_BEENDEN: u32 = 2;
const ID_HOST: u32 = 100;
/// So lange wartet der Fensterfaden hoechstens auf die erste Anmeldung
/// bzw. auf das Ende des Fadens.
const WARTEN: Duration = Duration::from_secs(2);
/// So lange wartet hinweis() hoechstens auf die Antwort des Symbolfadens
/// (Shell_NotifyIconW kann haengen, wenn Explorer haengt). Das Fenster ist
/// dann schon verborgen, der Nutzer merkt davon nichts.
const FRAGEN_HINWEIS: u32 = 1000;
/// Frist fuer die synchronen Fragen des Selbsttests.
const FRAGEN_SELBSTTEST: u32 = 3000;

/// Was beide Faeden teilen.
struct Geteilt {
    /// Menue und Tooltip, wie main.rs sie zuletzt gesetzt hat.
    stand: Mutex<Stand>,
    /// Titel und Text der naechsten Sprechblase.
    hinweis: Mutex<(String, String)>,
    /// Ist das Symbol gerade angemeldet? Nur dann darf Schliessen ablegen.
    steht: AtomicBool,
    /// Warum die letzte Anmeldung scheiterte.
    grund: Mutex<Option<String>>,
    /// Die erste Anmeldung ist versucht (gelungen oder nicht).
    versucht: AtomicBool,
}

/// Was nur der Symbolfaden braucht (je Faden einer, siehe FADEN).
struct Faden {
    geteilt: Arc<Geteilt>,
    befehl: Box<dyn Fn(Befehl) + Send>,
    symbol: HICON,
    /// Nummer der Rundnachricht "TaskbarCreated" (Explorer neu gestartet).
    taskbar_created: u32,
}

thread_local! {
    /// Zustand des Symbolfadens. Als Rc, damit der Fensterablauf ihn ohne
    /// offene Ausleihe benutzt: TrackPopupMenu ruft ihn verschachtelt auf.
    static FADEN: RefCell<Option<Rc<Faden>>> = const { RefCell::new(None) };
}

fn faden() -> Option<Rc<Faden>> {
    FADEN.with(|f| f.borrow().clone())
}

/// Das Symbol im Infobereich. Faellt es weg, wird es entfernt und der Faden
/// endet.
pub struct Symbol {
    fenster: isize,
    geteilt: Arc<Geteilt>,
    ende: mpsc::Receiver<()>,
    /// Zuletzt gesetzter Stand - nur Aenderungen gehen an den Faden.
    zuletzt: Stand,
}

impl Symbol {
    /// Faden und Fenster anlegen und das Symbol anmelden. Err nur, wenn es
    /// weder Faden noch Fenster gibt; scheitert nur die Anmeldung (kein
    /// Explorer), steht `steht()` auf false und `grund()` sagt warum - der
    /// Faden wartet dann auf "TaskbarCreated". Auf das Fenster wird ohne Frist
    /// gewartet: sein Anlegen haengt nicht am Explorer, und mit einer Frist
    /// liefe bei Ueberlast ein Faden ohne Besitzer weiter, dessen Symbol nie
    /// mehr entfernt wuerde (Durchsicht [4]).
    pub fn neu(befehl: Box<dyn Fn(Befehl) + Send>, stand: &Stand) -> Result<Symbol, String> {
        let geteilt = Arc::new(Geteilt {
            stand: Mutex::new(stand.clone()),
            hinweis: Mutex::new((String::new(), String::new())),
            steht: AtomicBool::new(false),
            grund: Mutex::new(None),
            versucht: AtomicBool::new(false),
        });
        let (bereit_tx, bereit_rx) = mpsc::sync_channel::<Result<isize, String>>(1);
        let (ende_tx, ende_rx) = mpsc::channel::<()>();
        let g = geteilt.clone();
        std::thread::Builder::new()
            .name("infobereich".into())
            .spawn(move || {
                faden_laufen(g, befehl, bereit_tx);
                let _ = ende_tx.send(());
            })
            .map_err(|e| format!("Faden: {e}"))?;
        // Das Fenster meldet der Faden sofort; die Anmeldung beim Explorer
        // kann dauern, wenn er haengt. Darauf wird hoechstens WARTEN lang
        // gewartet - danach steht das Symbol eben erst spaeter (steht()).
        let fenster = match bereit_rx.recv() {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("Symbolfaden endete ohne Fenster".into()),
        };
        let bis = std::time::Instant::now() + WARTEN;
        while !geteilt.versucht.load(Ordering::SeqCst) && std::time::Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !geteilt.versucht.load(Ordering::SeqCst) {
            if let Ok(mut g) = geteilt.grund.lock() {
                *g = Some(format!("Anmeldung beim Infobereich dauert laenger als {} s", WARTEN.as_secs()));
            }
        }
        Ok(Symbol { fenster, geteilt, ende: ende_rx, zuletzt: stand.clone() })
    }

    pub fn steht(&self) -> bool {
        self.geteilt.steht.load(Ordering::SeqCst)
    }

    pub fn grund(&self) -> Option<String> {
        self.geteilt.grund.lock().ok().and_then(|g| g.clone())
    }

    /// Neuer Stand: das Menue gilt ab dem naechsten Rechtsklick, der Tooltip
    /// wird sofort angewandt, wenn er sich geaendert hat.
    pub fn stand_setzen(&mut self, stand: &Stand) {
        if *stand == self.zuletzt {
            return;
        }
        let tooltip_neu = stand.tooltip != self.zuletzt.tooltip;
        if let Ok(mut s) = self.geteilt.stand.lock() {
            *s = stand.clone();
        }
        self.zuletzt = stand.clone();
        if tooltip_neu {
            self.posten(WM_TOOLTIP);
        }
    }

    /// Die Sprechblase (NIF_INFO) einmal zeigen. true nur, wenn das Symbol
    /// steht und der Infobereich sie angenommen hat (NIM_MODIFY gelungen) -
    /// nur dann vermerkt main.rs tray_hinweis=1.
    pub fn hinweis(&mut self, titel: &str, text: &str) -> bool {
        if !self.steht() {
            return false;
        }
        if let Ok(mut h) = self.geteilt.hinweis.lock() {
            *h = (titel.to_string(), text.to_string());
        }
        self.fragen(WM_HINWEIS, FRAGEN_HINWEIS)
    }

    /// Unter Windows gibt es nichts nachzufuehren (die Sprechblase schliesst
    /// das System).
    pub fn takt(&mut self) {}

    fn posten(&self, msg: u32) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.fenster as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }

    /// Synchron an den Faden, hoechstens `frist_ms`: Antwort 1 = gelungen.
    fn fragen(&self, msg: u32, frist_ms: u32) -> bool {
        let mut antwort = 0usize;
        let r = unsafe {
            SendMessageTimeoutW(HWND(self.fenster as *mut _), msg, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, frist_ms, Some(&mut antwort))
        };
        r.0 != 0 && antwort == 1
    }
}

impl Drop for Symbol {
    fn drop(&mut self) {
        // WM_BEENDEN entfernt das Symbol und schliesst das Fenster; der Faden
        // endet mit seiner Schleife. Nicht ewig warten: steht gerade ein
        // Kontextmenue offen, endet es erst mit dem Fenster.
        self.posten(WM_BEENDEN);
        let _ = self.ende.recv_timeout(WARTEN);
    }
}

/// Das Symbol als HICON in der Groesse des Infobereichs (SM_CXSMICON folgt
/// der Skalierung), gezeichnet von logo.rs.
fn symbol_bauen() -> Result<HICON, String> {
    let g = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
    let bild = crate::logo::ico_bmp(g, &crate::logo::rgba(g));
    unsafe { CreateIconFromResourceEx(&bild, true, 0x0003_0000, g as i32, g as i32, LR_DEFAULTCOLOR) }
        .map_err(|e| format!("Symbol {g} px: {}", e.message()))
}

/// Grundgeruest der Anmeldedaten: unser Fenster, unsere Kennung.
fn daten(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: SYMBOL_ID, ..Default::default() }
}

/// Text in ein festes UTF-16-Feld, gekuerzt, mit abschliessender Null.
fn feld_setzen(ziel: &mut [u16], text: &str) {
    let t = tray::utf16_kuerzen(text, ziel.len() - 1);
    ziel[..t.len()].copy_from_slice(&t);
    ziel[t.len()] = 0;
}

/// Das Symbol anmelden (NIM_ADD, danach Fassung 4).
fn anmelden(hwnd: HWND, f: &Faden) -> Result<(), String> {
    let mut d = daten(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    d.uCallbackMessage = WM_SYMBOL;
    d.hIcon = f.symbol;
    let tooltip = f.geteilt.stand.lock().map(|s| s.tooltip.clone()).unwrap_or_else(|_| "QuadChroma".into());
    feld_setzen(&mut d.szTip, &tooltip);
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &d) }.as_bool();
    if !ok {
        // Laut Dokumentation kann NIM_ADD an einer Zeitueberschreitung
        // scheitern, obwohl das Symbol angekommen ist: dann gelingt NIM_MODIFY.
        let zeit = unsafe { GetLastError() } == ERROR_TIMEOUT;
        if !(zeit && unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) }.as_bool()) {
            return Err("Shell_NotifyIconW(NIM_ADD) gescheitert - kein Infobereich (laeuft Explorer?)".into());
        }
    }
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
    }
    Ok(())
}

/// Anmelden und das Ergebnis im geteilten Stand vermerken.
fn anmelden_vermerken(hwnd: HWND, f: &Faden) -> Result<(), String> {
    let r = anmelden(hwnd, f);
    f.geteilt.steht.store(r.is_ok(), Ordering::SeqCst);
    if let Ok(mut g) = f.geteilt.grund.lock() {
        *g = r.as_ref().err().cloned();
    }
    r
}

fn abmelden(hwnd: HWND, f: &Faden) -> bool {
    let d = daten(hwnd);
    let ok = unsafe { Shell_NotifyIconW(NIM_DELETE, &d) }.as_bool();
    f.geteilt.steht.store(false, Ordering::SeqCst);
    ok
}

fn tooltip_anwenden(hwnd: HWND, f: &Faden) -> bool {
    let mut d = daten(hwnd);
    d.uFlags = NIF_TIP | NIF_SHOWTIP;
    let tooltip = f.geteilt.stand.lock().map(|s| s.tooltip.clone()).unwrap_or_default();
    feld_setzen(&mut d.szTip, &tooltip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) }.as_bool()
}

fn hinweis_zeigen(hwnd: HWND, f: &Faden) -> bool {
    let (titel, text) = f.geteilt.hinweis.lock().map(|h| h.clone()).unwrap_or_default();
    let mut d = daten(hwnd);
    // NIF_SHOWTIP bei jedem NIM_MODIFY (Fassung 4), dazu der geltende
    // Tooltip - sonst bleibt er nach der Sprechblase aus, bis er sich aendert.
    d.uFlags = NIF_INFO | NIF_TIP | NIF_SHOWTIP;
    let tooltip = f.geteilt.stand.lock().map(|s| s.tooltip.clone()).unwrap_or_default();
    feld_setzen(&mut d.szTip, &tooltip);
    feld_setzen(&mut d.szInfoTitle, &titel);
    feld_setzen(&mut d.szInfo, &text);
    d.dwInfoFlags = NIIF_INFO;
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) }.as_bool()
}

/// Menuetext fuer Windows: "&" leitet sonst ein Tastenkuerzel ein.
fn menuetext(t: &str) -> HSTRING {
    HSTRING::from(t.replace('&', "&&"))
}

/// Befehlsnummer des Menuepunkts an Stelle `i` (None: Trenner). Hosts
/// tragen ID_HOST plus ihre Stelle im Menue; befehl_zu loest genau so auf.
fn menue_nummer(i: usize, p: &Punkt) -> Option<u32> {
    match p {
        Punkt::Oeffnen(_) => Some(ID_OEFFNEN),
        Punkt::Trenner => None,
        Punkt::Verbinden { .. } => Some(ID_HOST + i as u32),
        Punkt::Beenden(_) => Some(ID_BEENDEN),
    }
}

/// Der Befehl zur gewaehlten Nummer (TrackPopupMenu mit TPM_RETURNCMD),
/// aufgeloest gegen genau die Liste, aus der das Menue gebaut war. 0 (nichts
/// gewaehlt) und fremde Nummern ergeben nichts.
fn befehl_zu(punkte: &[Punkt], gewaehlt: u32) -> Option<Befehl> {
    match gewaehlt {
        0 => None,
        ID_OEFFNEN => Some(Befehl::Oeffnen),
        ID_BEENDEN => Some(Befehl::Beenden),
        n if n >= ID_HOST => punkte
            .get((n - ID_HOST) as usize)
            .filter(|p| matches!(p, Punkt::Verbinden { .. }))
            .and_then(|p| p.befehl()),
        _ => None,
    }
}

/// Das Kontextmenue aus den Punkten bauen: Nummern nach menue_nummer,
/// "Oeffnen" fett (die Vorgabe, die auch der Doppelklick ausloest). Der
/// Aufrufer gibt es mit DestroyMenu frei.
fn menue_bauen(punkte: &[Punkt]) -> Option<HMENU> {
    unsafe {
        let menue = CreatePopupMenu().ok()?;
        for (i, p) in punkte.iter().enumerate() {
            let nummer = menue_nummer(i, p).unwrap_or(0) as usize;
            let _ = match p {
                Punkt::Oeffnen(t) | Punkt::Beenden(t) => AppendMenuW(menue, MF_STRING, nummer, &menuetext(t)),
                Punkt::Verbinden { text, .. } => AppendMenuW(menue, MF_STRING, nummer, &menuetext(text)),
                Punkt::Trenner => AppendMenuW(menue, MF_SEPARATOR, 0, PCWSTR::null()),
            };
        }
        let _ = SetMenuDefaultItem(menue, ID_OEFFNEN, 0);
        Some(menue)
    }
}

/// Das Kontextmenue an der Stelle (x, y) zeigen und den gewaehlten Befehl
/// liefern. Gebaut aus dem Stand dieses Augenblicks; die Wahl wird gegen
/// genau diese Liste aufgeloest.
fn menue_zeigen(hwnd: HWND, f: &Faden, x: i32, y: i32) -> Option<Befehl> {
    let punkte = f.geteilt.stand.lock().map(|s| s.menue.clone()).unwrap_or_default();
    unsafe {
        let menue = menue_bauen(&punkte)?;
        let ausrichtung = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
        let _ = SetForegroundWindow(hwnd);
        let gewaehlt = TrackPopupMenu(menue, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | ausrichtung, x, y, None, hwnd, None).0 as u32;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menue);
        befehl_zu(&punkte, gewaehlt)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(f) = faden() else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    if msg == f.taskbar_created && f.taskbar_created != 0 {
        // Explorer neu gestartet: das alte Symbol ist weg, neu anmelden.
        let _ = anmelden_vermerken(hwnd, &f);
        return LRESULT(0);
    }
    match msg {
        WM_SYMBOL => {
            // Fassung 4: Ereignis im unteren Wort von lParam, Ankerpunkt in
            // wParam (x unten, y oben, je mit Vorzeichen).
            let ereignis = (lp.0 as u32) & 0xffff;
            match ereignis {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => (f.befehl)(Befehl::Oeffnen),
                WM_CONTEXTMENU => {
                    let x = (wp.0 & 0xffff) as u16 as i16 as i32;
                    let y = ((wp.0 >> 16) & 0xffff) as u16 as i16 as i32;
                    if let Some(b) = menue_zeigen(hwnd, &f, x, y) {
                        (f.befehl)(b);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TOOLTIP => LRESULT(tooltip_anwenden(hwnd, &f) as isize),
        WM_HINWEIS => LRESULT(hinweis_zeigen(hwnd, &f) as isize),
        WM_ENTFERNEN => LRESULT(abmelden(hwnd, &f) as isize),
        WM_BEENDEN => {
            if f.geteilt.steht.load(Ordering::SeqCst) {
                abmelden(hwnd, &f);
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        // Von aussen (etwa taskkill ohne /F an ein sichtbares Fenster):
        // uebergehen - das Symbol ist der Weg zurueck zur abgelegten App.
        WM_CLOSE => LRESULT(0),
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Der Symbolfaden: Fenster anlegen, Symbol anmelden, Nachrichtenschleife.
fn faden_laufen(geteilt: Arc<Geteilt>, befehl: Box<dyn Fn(Befehl) + Send>, bereit: mpsc::SyncSender<Result<isize, String>>) {
    let symbol = match symbol_bauen() {
        Ok(s) => s,
        Err(e) => {
            let _ = bereit.send(Err(e));
            return;
        }
    };
    let hwnd = unsafe {
        let hinst = match GetModuleHandleW(PCWSTR::null()) {
            Ok(h) => HINSTANCE::from(h),
            Err(e) => {
                let _ = DestroyIcon(symbol);
                let _ = bereit.send(Err(format!("Modulkennung: {}", e.message())));
                return;
            }
        };
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: KLASSE, ..Default::default() };
        // Schon registriert (zweites Symbol im selben Prozess): kein Fehler.
        RegisterClassW(&wc);
        match CreateWindowExW(WS_EX_TOOLWINDOW, KLASSE, KLASSE, WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None) {
            Ok(h) => h,
            Err(e) => {
                let _ = DestroyIcon(symbol);
                let _ = bereit.send(Err(format!("Symbolfenster: {}", e.message())));
                return;
            }
        }
    };
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    let f = Rc::new(Faden { geteilt, befehl, symbol, taskbar_created });
    FADEN.with(|z| *z.borrow_mut() = Some(f.clone()));
    let _ = bereit.send(Ok(hwnd.0 as isize));
    // Scheitert die erste Anmeldung (kein Explorer), laeuft der Faden
    // trotzdem weiter: "TaskbarCreated" holt sie nach.
    let _ = anmelden_vermerken(hwnd, &f);
    f.geteilt.versucht.store(true, Ordering::SeqCst);
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
    if f.geteilt.steht.load(Ordering::SeqCst) {
        abmelden(hwnd, &f);
    }
    FADEN.with(|z| *z.borrow_mut() = None);
    unsafe {
        let _ = DestroyIcon(f.symbol);
    }
}

/// --tray-selbsttest: Symbol anlegen, aendern (Tooltip; die Sprechblase
/// bleibt aus, sie gehoert dem echten Ablegen), entfernen. Rueckgabe 0 =
/// alles gelungen, 1 = nicht. Lauscht nicht im Netz und startet keine Bekanntgabe; ohne
/// Explorer (etwa in Sitzung 0 ueber ssh) endet er sauber mit 1.
pub fn selbsttest() -> i32 {
    let lang = crate::strings::pick("de");
    let stand = tray::stand(lang, &[], None);
    let mut s = match Symbol::neu(Box::new(|_| {}), &stand) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Infobereich-Selbsttest: kein Symbolfenster ({e})");
            return 1;
        }
    };
    if !s.steht() {
        eprintln!(
            "Infobereich-Selbsttest: Symbol nicht angelegt ({})",
            s.grund().unwrap_or_else(|| "ohne Grund".into())
        );
        return 1;
    }
    println!("Infobereich-Selbsttest: Symbol angelegt");
    // Aendern: Tooltip einer Sitzung, synchron gefragt.
    let neu = tray::stand(lang, &[], Some("Selbsttest"));
    if let Ok(mut st) = s.geteilt.stand.lock() {
        *st = neu.clone();
    }
    s.zuletzt = neu;
    if !s.fragen(WM_TOOLTIP, FRAGEN_SELBSTTEST) {
        eprintln!("Infobereich-Selbsttest: Tooltip nicht geaendert (NIM_MODIFY gescheitert)");
        return 1;
    }
    println!("Infobereich-Selbsttest: Tooltip geaendert");
    if !s.fragen(WM_ENTFERNEN, FRAGEN_SELBSTTEST) {
        eprintln!("Infobereich-Selbsttest: Symbol nicht entfernt (NIM_DELETE gescheitert)");
        return 1;
    }
    if s.steht() {
        eprintln!("Infobereich-Selbsttest: Symbol steht nach dem Entfernen noch");
        return 1;
    }
    println!("Infobereich-Selbsttest: Symbol entfernt - bestanden");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuDefaultItem, GetMenuItemCount, GetMenuItemID, GET_MENU_DEFAULT_ITEM_FLAGS};

    fn host(name: &str, addr: &str) -> crate::discovery::Host {
        crate::discovery::Host { name: name.into(), addr: addr.parse().unwrap(), seen: std::time::Instant::now() }
    }

    /// Menuenummer -> Befehl als reine Logik: jeder waehlbare Punkt liefert
    /// genau seinen Befehl - bei mehreren Hosts der richtige Host.
    #[test]
    fn menuenummer_fuehrt_zum_befehl() {
        let hosts = [host("A", "10.0.0.1:9001"), host("B", "10.0.0.2:9001"), host("C", "10.0.0.3:9101")];
        for n in 0..=hosts.len() {
            let punkte = tray::menue(crate::strings::pick("de"), &hosts[..n]);
            let mut waehlbar = 0;
            for (i, p) in punkte.iter().enumerate() {
                match menue_nummer(i, p) {
                    Some(nr) => {
                        waehlbar += 1;
                        assert_eq!(befehl_zu(&punkte, nr), p.befehl(), "{n} Hosts, Punkt {i}: {p:?}");
                    }
                    None => assert_eq!(*p, Punkt::Trenner),
                }
            }
            assert_eq!(waehlbar, n + 2);
            // Nichts gewaehlt, fremde Nummern, ein Trenner: nichts.
            assert_eq!(befehl_zu(&punkte, 0), None);
            assert_eq!(befehl_zu(&punkte, 99), None);
            assert_eq!(befehl_zu(&punkte, ID_HOST + punkte.len() as u32), None);
            assert_eq!(befehl_zu(&punkte, ID_HOST + 1), None, "Trenner als Host aufgeloest");
        }
    }

    /// Der Symbolfaden endet nur ueber WM_BEENDEN (Drop), nicht ueber ein
    /// WM_CLOSE von aussen - die abgelegte App behielte sonst kein Symbol.
    /// Ohne Infobereich (Sitzung 0 ueber ssh) gilt die Sprechblase nicht als
    /// gezeigt, tray_hinweis bliebe also offen. Mit Infobereich (Sitzung mit
    /// Explorer) erscheint das Symbol kurz, die Sprechblase wird ausgelassen.
    #[test]
    fn symbolfaden_endet_nur_ueber_beenden() {
        let stand = tray::stand(crate::strings::pick("de"), &[], None);
        let mut s = Symbol::neu(Box::new(|_| {}), &stand).expect("Symbolfenster");
        unsafe {
            let _ = PostMessageW(Some(HWND(s.fenster as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        assert!(
            s.ende.recv_timeout(Duration::from_millis(300)).is_err(),
            "WM_CLOSE von aussen hat den Symbolfaden beendet"
        );
        if !s.steht() {
            assert!(!s.hinweis("QuadChroma", "Test"), "Sprechblase ohne Symbol als gezeigt gemeldet");
        }
        s.posten(WM_BEENDEN);
        assert!(s.ende.recv_timeout(WARTEN).is_ok(), "WM_BEENDEN hat den Symbolfaden nicht beendet");
    }

    /// Das echte Menue (CreatePopupMenu, ohne Shell - geht auch in Sitzung 0):
    /// jede Stelle traegt die Nummer, die zu ihrem Befehl fuehrt; "Oeffnen"
    /// ist die Vorgabe.
    #[test]
    fn echtes_menue_nummern() {
        let hosts = [host("A", "10.0.0.1:9001"), host("B", "10.0.0.2:9001"), host("C", "10.0.0.3:9101"), host("D", "10.0.0.4:9001")];
        let punkte = tray::menue(crate::strings::pick("de"), &hosts);
        let menue = menue_bauen(&punkte).expect("Menue");
        unsafe {
            assert_eq!(GetMenuItemCount(Some(menue)), punkte.len() as i32);
            for (i, p) in punkte.iter().enumerate() {
                let id = GetMenuItemID(menue, i as i32);
                match p {
                    Punkt::Trenner => assert_eq!(id, 0, "Stelle {i}"),
                    _ => assert_eq!(befehl_zu(&punkte, id), p.befehl(), "Stelle {i}: {p:?}"),
                }
            }
            assert_eq!(GetMenuDefaultItem(menue, 0, GET_MENU_DEFAULT_ITEM_FLAGS(0)), ID_OEFFNEN);
            let _ = DestroyMenu(menue);
        }
    }
}
