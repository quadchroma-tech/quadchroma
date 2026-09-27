// Symbol im Infobereich (Windows), Spezifikation 3.9 - das eine Symbol der
// einen App (Client und Host-Rolle in einem Prozess) bzw. der reinen
// Host-Rolle (--nur-host).
//
// Ein eigener Faden betreibt ein nie sichtbares Fenster oberster Ebene mit
// Nachrichtenschleife; an ihm haengt das Symbol (Shell_NotifyIconW). Ein
// Fenster oberster Ebene und kein Nachrichtenfenster (HWND_MESSAGE), weil
// nur jene die Rundnachricht "TaskbarCreated" bekommen: startet Explorer neu,
// meldet der Faden das Symbol neu an.
//
// Der Fensterfaden (winit) bzw. der Faden des Dienstes spricht mit diesem
// Faden nur ueber PostMessageW/SendMessageTimeoutW an dessen Fenster und
// ueber einen geteilten Stand; zurueck geht es ueber die Rueckrufe `befehl`
// (gewaehlte Nummer), `menue` (Eintraege bei jedem Oeffnen) und `ende`. Keiner
// wartet je auf den anderen, ausser beim Anlegen (auf das Fenster ohne Frist
// - es haengt nicht am Explorer -, auf die Anmeldung hoechstens WARTEN), beim
// Entfernen (hoechstens WARTEN), bei der Sprechblase (hoechstens
// FRAGEN_HINWEIS) und beim Selbsttest (bewusst synchron). Beendet wird der
// Faden nur ueber WM_BEENDEN (Drop). WM_CLOSE von aussen (ein anderes
// Programm, ein Test) und WM_ENDSESSION (Abmelden, Herunterfahren, ein
// Installationsprogramm ueber den Restart Manager) heissen dagegen: die App
// bzw. die Host-Rolle endet - ueber den Rueckruf `ende`, der einen Zuschauer
// verabschiedet und dem Rest des Programms das Ende meldet (main.rs bzw.
// host/mod.rs). (taskkill ohne /F erreicht das Symbolfenster nicht: es ist
// unsichtbar; mit sichtbarem Fenster legt WM_CLOSE die App nur ab.)
//
// Bedienung (NOTIFYICON_VERSION_4):
//   - Die eine App (`linksklick` gesetzt): Linksklick (NIN_SELECT),
//     Eingabe/Leertaste (NIN_KEYSELECT) und Doppelklick waehlen die Nummer
//     `linksklick` ("QuadChroma oeffnen"); Rechtsklick bzw. Umschalt+F10
//     (WM_CONTEXTMENU) oeffnet das Menue, "QuadChroma oeffnen" fett als
//     Vorgabe. Die reine Host-Rolle hat kein Fenster: dort oeffnet auch der
//     Linksklick das Menue.
//   - Vor dem Menue SetForegroundWindow, sonst schliesst es nicht beim Klick
//     daneben; danach WM_NULL (bekannte Eigenheit von TrackPopupMenu).
//   - Das Menue wird bei jedem Oeffnen frisch gebaut (Rueckruf `menue` im
//     Symbolfaden, aus tray::Eintrag: Untermenues, Haken, deaktivierte
//     Eintraege, eigene Nummern), damit Geraeteliste, Passwort und Zustand
//     stimmen; die gewaehlte Nummer geht an `befehl`.
//   - Tooltip ueber NIF_TIP mit NIF_SHOWTIP (Fassung 4 blendet ihn sonst aus).
//   - Die Sprechblase ueber NIF_INFO, ebenfalls mit NIF_SHOWTIP (jedes
//     NIM_MODIFY ohne das Kennzeichen blendet den Tooltip aus).

use crate::tray::{self, Eintrag};
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
    HICON, HMENU, LR_DEFAULTCOLOR, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG,
    SMTO_ABORTIFHUNG, SM_CXSMICON, SM_MENUDROPALIGNMENT, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN,
    TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_LBUTTONDBLCLK, WM_NULL, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_POPUP,
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
    /// Der Tooltip, wie er zuletzt gesetzt wurde.
    tooltip: Mutex<String>,
    /// Titel und Text der naechsten Sprechblase.
    hinweis: Mutex<(String, String)>,
    /// Ist das Symbol gerade angemeldet? Nur dann darf Schliessen ablegen.
    steht: AtomicBool,
    /// Warum die letzte Anmeldung scheiterte.
    grund: Mutex<Option<String>>,
    /// Die erste Anmeldung ist versucht (gelungen oder nicht).
    versucht: AtomicBool,
}

/// Wie der Symbolfaden Wahlen weitergibt: `menue` liefert bei jedem
/// Oeffnen die Eintraege, `befehl` bekommt die gewaehlte Nummer, `linksklick`
/// ist die Nummer des Linksklicks (None: auch er oeffnet das Menue). `ende`
/// bekommt WM_CLOSE von aussen und WM_ENDSESSION (mit einem Wort, woher) und
/// kehrt erst zurueck, wenn der Abschied hinaus ist - nach WM_ENDSESSION
/// kann der Prozess gleich danach enden.
struct Art {
    befehl: Box<dyn Fn(u32) + Send>,
    menue: Box<dyn Fn() -> Vec<Eintrag> + Send>,
    ende: Box<dyn Fn(&str) + Send>,
    linksklick: Option<u32>,
}

/// Was nur der Symbolfaden braucht (je Faden einer, siehe FADEN).
struct Faden {
    geteilt: Arc<Geteilt>,
    art: Art,
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
    /// Zuletzt gesetzter Tooltip - nur Aenderungen gehen an den Faden.
    zuletzt: String,
}

impl Symbol {
    /// Faden und Fenster anlegen und das Symbol anmelden. `menue` liefert bei
    /// jedem Oeffnen die Eintraege (im Symbolfaden gerufen), `befehl` bekommt
    /// die gewaehlte Nummer (ebenfalls im Symbolfaden - lange Arbeit gehoert
    /// in einen anderen Faden), `linksklick` die Nummer des Linksklicks (None:
    /// er oeffnet das Menue). `ende` beendet App bzw. Host-Rolle, wenn Windows
    /// oder ein anderes Programm das verlangt (WM_CLOSE von aussen,
    /// WM_ENDSESSION); das Symbol ist dann schon abgemeldet.
    ///
    /// Err nur, wenn es weder Faden noch Fenster gibt; scheitert nur die
    /// Anmeldung (kein Explorer), steht `steht()` auf false und `grund()` sagt
    /// warum - der Faden wartet dann auf "TaskbarCreated". Auf das Fenster wird
    /// ohne Frist gewartet: sein Anlegen haengt nicht am Explorer, und mit
    /// einer Frist liefe bei Ueberlast ein Faden ohne Besitzer weiter, dessen
    /// Symbol nie mehr entfernt wuerde (Durchsicht [4]).
    pub fn neu(
        befehl: Box<dyn Fn(u32) + Send>,
        menue: Box<dyn Fn() -> Vec<Eintrag> + Send>,
        ende: Box<dyn Fn(&str) + Send>,
        linksklick: Option<u32>,
        tooltip: &str,
    ) -> Result<Symbol, String> {
        Symbol::neu_mit(Art { befehl, menue, ende, linksklick }, tooltip)
    }

    fn neu_mit(art: Art, tooltip: &str) -> Result<Symbol, String> {
        let geteilt = Arc::new(Geteilt {
            tooltip: Mutex::new(tooltip.to_string()),
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
                faden_laufen(g, art, bereit_tx);
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
        Ok(Symbol { fenster, geteilt, ende: ende_rx, zuletzt: tooltip.to_string() })
    }

    pub fn steht(&self) -> bool {
        self.geteilt.steht.load(Ordering::SeqCst)
    }

    pub fn grund(&self) -> Option<String> {
        self.geteilt.grund.lock().ok().and_then(|g| g.clone())
    }

    /// Wer sonst wissen will, ob das Symbol steht (der Dienst der Host-Rolle:
    /// "Zulassen" nur mit Symbol), fragt hierueber - auch aus einem anderen
    /// Faden und nachdem es erst spaeter angemeldet wurde.
    pub fn steht_abfrage(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let g = self.geteilt.clone();
        Arc::new(move || g.steht.load(Ordering::SeqCst))
    }

    /// Neuer Tooltip, sofort angewandt, wenn er sich geaendert hat.
    pub fn tooltip_setzen(&mut self, tooltip: &str) {
        if tooltip == self.zuletzt {
            return;
        }
        if let Ok(mut s) = self.geteilt.tooltip.lock() {
            *s = tooltip.to_string();
        }
        self.zuletzt = tooltip.to_string();
        self.posten(WM_TOOLTIP);
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
    let tooltip = f.geteilt.tooltip.lock().map(|s| s.clone()).unwrap_or_else(|_| "QuadChroma".into());
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
    let tooltip = f.geteilt.tooltip.lock().map(|s| s.clone()).unwrap_or_default();
    feld_setzen(&mut d.szTip, &tooltip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) }.as_bool()
}

fn hinweis_zeigen(hwnd: HWND, f: &Faden) -> bool {
    let (titel, text) = f.geteilt.hinweis.lock().map(|h| h.clone()).unwrap_or_default();
    let mut d = daten(hwnd);
    // NIF_SHOWTIP bei jedem NIM_MODIFY (Fassung 4), dazu der geltende
    // Tooltip - sonst bleibt er nach der Sprechblase aus, bis er sich aendert.
    d.uFlags = NIF_INFO | NIF_TIP | NIF_SHOWTIP;
    let tooltip = f.geteilt.tooltip.lock().map(|s| s.clone()).unwrap_or_default();
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

/// Ein Menue aus allgemeinen Eintraegen bauen, Untermenues eingeschlossen
/// (sie gehoeren danach dem Menue und fallen mit dessen DestroyMenu). Der
/// Aufrufer gibt es mit DestroyMenu frei.
fn eintraege_bauen(eintraege: &[Eintrag]) -> Option<HMENU> {
    unsafe {
        let menue = CreatePopupMenu().ok()?;
        eintraege_anhaengen(menue, eintraege);
        Some(menue)
    }
}

/// Das echte Menue aus Eintraegen fuer Tests anderer Teile (host/oberflaeche.rs).
#[cfg(test)]
pub fn menue_fuer_test(eintraege: &[Eintrag]) -> Option<HMENU> {
    eintraege_bauen(eintraege)
}

fn eintraege_anhaengen(menue: HMENU, eintraege: &[Eintrag]) {
    let grau = |aktiv: bool| if aktiv { MENU_ITEM_FLAGS(0) } else { MF_GRAYED };
    for e in eintraege {
        unsafe {
            match e {
                Eintrag::Punkt { text, nummer, haken, aktiv, fett } => {
                    let haken = if *haken { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
                    let _ = AppendMenuW(menue, MF_STRING | haken | grau(*aktiv), *nummer as usize, &menuetext(text));
                    if *fett && *nummer != 0 {
                        let _ = SetMenuDefaultItem(menue, *nummer, 0);
                    }
                }
                Eintrag::Unter { text, eintraege, aktiv } => {
                    let Ok(unter) = CreatePopupMenu() else { continue };
                    eintraege_anhaengen(unter, eintraege);
                    if AppendMenuW(menue, MF_STRING | MF_POPUP | grau(*aktiv), unter.0 as usize, &menuetext(text)).is_err() {
                        let _ = DestroyMenu(unter);
                    }
                }
                Eintrag::Trenner => {
                    let _ = AppendMenuW(menue, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
    }
}

/// Ein Menue an der Stelle (x, y) zeigen und die gewaehlte Nummer liefern
/// (0: nichts gewaehlt).
fn nummer_waehlen(hwnd: HWND, eintraege: &[Eintrag], x: i32, y: i32) -> u32 {
    let Some(menue) = eintraege_bauen(eintraege) else { return 0 };
    unsafe {
        let ausrichtung = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
        let _ = SetForegroundWindow(hwnd);
        let gewaehlt = TrackPopupMenu(menue, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | ausrichtung, x, y, None, hwnd, None).0 as u32;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menue);
        gewaehlt
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
            let x = (wp.0 & 0xffff) as u16 as i16 as i32;
            let y = ((wp.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let art = &f.art;
            let linksklick = matches!(ereignis, NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK);
            match art.linksklick {
                Some(nr) if linksklick => (art.befehl)(nr),
                _ if linksklick || ereignis == WM_CONTEXTMENU => {
                    // WM_LBUTTONDBLCLK ohne eigene Nummer: das Menue kam schon
                    // mit dem ersten Klick (NIN_SELECT).
                    if ereignis != WM_LBUTTONDBLCLK {
                        let eintraege = (art.menue)();
                        let nr = nummer_waehlen(hwnd, &eintraege, x, y);
                        if nr != 0 {
                            (art.befehl)(nr);
                        }
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
        // Von aussen (ein anderes Programm, ein Test): App bzw. Host-Rolle
        // enden darauf, mit Abschied an einen Zuschauer; ebenso beim
        // Abmelden, Herunterfahren oder wenn ein Installationsprogramm sie
        // ueber den Restart Manager schliesst (WM_ENDSESSION - nach dessen
        // Rueckkehr endete der Prozess ohnehin, der Abschied muss vorher
        // hinaus). Der Faden selbst endet erst mit WM_BEENDEN.
        WM_CLOSE | WM_ENDSESSION => {
            if msg == WM_CLOSE || wp.0 != 0 {
                if f.geteilt.steht.load(Ordering::SeqCst) {
                    abmelden(hwnd, &f);
                }
                (f.art.ende)(if msg == WM_CLOSE { "WM_CLOSE von aussen" } else { sitzungsende(lp) });
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Warum WM_ENDSESSION kommt, fuer das Protokoll: lParam traegt
/// ENDSESSION_CLOSEAPP (0x1, ein Installationsprogramm ueber den Restart
/// Manager) oder ENDSESSION_LOGOFF (0x80000000); ohne beides faehrt Windows
/// herunter.
fn sitzungsende(lp: LPARAM) -> &'static str {
    let bits = lp.0 as u32;
    if bits & 0x1 != 0 {
        "ein Installationsprogramm schliesst die App (Restart Manager)"
    } else if bits & 0x8000_0000 != 0 {
        "Abmelden"
    } else {
        "Herunterfahren"
    }
}

/// Der Symbolfaden: Fenster anlegen, Symbol anmelden, Nachrichtenschleife.
fn faden_laufen(geteilt: Arc<Geteilt>, art: Art, bereit: mpsc::SyncSender<Result<isize, String>>) {
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
    let f = Rc::new(Faden { geteilt, art, symbol, taskbar_created });
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
    let mut s = match Symbol::neu(Box::new(|_| {}), Box::new(Vec::new), Box::new(|_| {}), Some(1), "QuadChroma") {
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
    let neu = tray::tooltip(Some("Selbsttest"));
    if let Ok(mut st) = s.geteilt.tooltip.lock() {
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

    /// Der Grund von WM_ENDSESSION fuers Protokoll: Restart Manager
    /// (ENDSESSION_CLOSEAPP), Abmelden (ENDSESSION_LOGOFF), sonst
    /// Herunterfahren.
    #[test]
    fn sitzungsende_nach_lparam() {
        assert_eq!(sitzungsende(LPARAM(1)), "ein Installationsprogramm schliesst die App (Restart Manager)");
        assert_eq!(sitzungsende(LPARAM(0x8000_0000)), "Abmelden");
        assert_eq!(sitzungsende(LPARAM(0)), "Herunterfahren");
    }

    /// Ein Symbol mit Kanaelen fuer gewaehlte Nummern und das Ende von aussen.
    fn test_symbol(linksklick: Option<u32>) -> (Symbol, mpsc::Receiver<u32>, mpsc::Receiver<String>) {
        let (nr_tx, nr_rx) = mpsc::channel();
        let (ende_tx, ende_rx) = mpsc::channel();
        let nr_tx = Mutex::new(nr_tx);
        let ende_tx = Mutex::new(ende_tx);
        let s = Symbol::neu(
            Box::new(move |nr| {
                let _ = nr_tx.lock().unwrap().send(nr);
            }),
            Box::new(Vec::new),
            Box::new(move |wie| {
                let _ = ende_tx.lock().unwrap().send(wie.to_string());
            }),
            linksklick,
            "QuadChroma",
        )
        .expect("Symbolfenster");
        (s, nr_rx, ende_rx)
    }

    /// Die eine App: Linksklick, Eingabe/Leertaste und Doppelklick auf das
    /// Symbol waehlen "QuadChroma oeffnen" (die Nummer `linksklick`), ohne
    /// ein Menue zu zeigen. Geht auch ohne Infobereich (Sitzung 0 ueber ssh):
    /// es zaehlt die Nachricht an das Fenster.
    #[test]
    fn linksklick_oeffnet() {
        let (s, nr, _) = test_symbol(Some(crate::symbolmenue::LINKSKLICK));
        for ereignis in [NIN_SELECT, NIN_KEYSELECT, WM_LBUTTONDBLCLK] {
            unsafe {
                let _ = PostMessageW(Some(HWND(s.fenster as *mut _)), WM_SYMBOL, WPARAM(0), LPARAM(ereignis as isize));
            }
            assert_eq!(nr.recv_timeout(WARTEN), Ok(crate::symbolmenue::LINKSKLICK), "Ereignis 0x{ereignis:x}");
        }
        drop(s);
    }

    /// WM_CLOSE von aussen und WM_ENDSESSION (wParam 1) melden das Ende ueber
    /// `ende` - mit dem Grund fuers Protokoll -, beenden den Symbolfaden aber
    /// nicht: das tut erst WM_BEENDEN (Drop). WM_ENDSESSION mit wParam 0 (die
    /// Sitzung endet doch nicht) meldet nichts. Ohne Infobereich gilt die
    /// Sprechblase nicht als gezeigt, tray_hinweis bliebe also offen.
    #[test]
    fn ende_von_aussen_und_beenden() {
        let (mut s, _, ende) = test_symbol(Some(1));
        let posten = |msg: u32, wp: usize, lp: isize| unsafe {
            let _ = PostMessageW(Some(HWND(s.fenster as *mut _)), msg, WPARAM(wp), LPARAM(lp));
        };
        posten(WM_CLOSE, 0, 0);
        assert_eq!(ende.recv_timeout(WARTEN).as_deref(), Ok("WM_CLOSE von aussen"));
        posten(WM_ENDSESSION, 0, 0);
        posten(WM_ENDSESSION, 1, 1);
        assert_eq!(ende.recv_timeout(WARTEN).as_deref(), Ok("ein Installationsprogramm schliesst die App (Restart Manager)"));
        assert!(ende.recv_timeout(Duration::from_millis(200)).is_err(), "WM_ENDSESSION mit wParam 0 als Ende gemeldet");
        assert!(s.ende.recv_timeout(Duration::from_millis(300)).is_err(), "WM_CLOSE von aussen hat den Symbolfaden beendet");
        if !s.steht() {
            assert!(!s.hinweis("QuadChroma", "Test"), "Sprechblase ohne Symbol als gezeigt gemeldet");
        }
        s.posten(WM_BEENDEN);
        assert!(s.ende.recv_timeout(WARTEN).is_ok(), "WM_BEENDEN hat den Symbolfaden nicht beendet");
    }

    /// Wer wissen will, ob das Symbol steht, fragt ueber steht_abfrage - das
    /// ist derselbe Stand wie steht(), auch aus einem anderen Faden.
    #[test]
    fn steht_abfrage_wie_steht() {
        let (s, _, _) = test_symbol(None);
        let abfrage = s.steht_abfrage();
        let steht = s.steht();
        assert_eq!(std::thread::spawn(move || abfrage()).join().unwrap(), steht);
    }
}
