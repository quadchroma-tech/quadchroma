// Kleine Fenster der Host-Rolle, Win32 ueber das windows-Crate (keine
// weitere Abhaengigkeit): die Zulassen-Anfrage (Spezifikation Pairing v1,
// 10.3), "Passwort aendern" (10.4) und die Rueckfrage vor "Alle Geraete
// entfernen".
//
// Jedes Fenster hat seinen eigenen Faden mit Nachrichtenschleife
// (IsDialogMessageW: Tab zwischen den Feldern, Eingabe = OK bzw.
// "Zulassen", Esc = Abbrechen bzw. "Ablehnen"). Kein Netzfaden wartet je
// auf ein Fenster: die Zugangsphase (einlass.rs) gibt eine Anfrage ueber
// `Oberflaeche::zeigen` ab und bekommt die Antwort ueber
// `Einlass::entscheiden`, das der Fensterfaden ruft - ohne eigene Sperre.
// Zieht sich eine Anfrage zurueck (Abbruch, Leitung zu, Frist), schliesst
// `schliessen` das Fenster per PostMessageW, ohne zu antworten.
//
// Das Zulassen-Fenster liegt oben (WS_EX_TOPMOST) und macht mit Ton und
// Blinken auf sich aufmerksam, nimmt aber nicht von selbst den Fokus: sonst
// ginge ein Eingabe-Tastendruck, der eigentlich einem anderen Programm
// galt, als "Zulassen" durch. Ein Klick hinein, dann gilt Eingabe.
//
// Schrift und Masse folgen der Systemskalierung (die Host-Rolle ist
// Per-Monitor-DPI-bewusst, mod.rs): die Nachrichtenschrift des Systems in
// der DPI des Systems.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, DrawTextW, GetDC, GetStockObject, GetSysColorBrush, ReleaseDC, SelectObject,
    COLOR_BTNFACE, DEFAULT_GUI_FONT, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, HFONT, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FlashWindowEx, GetDlgItem,
    GetMessageW, GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, IsWindow, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    TranslateMessage, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, ES_AUTOHSCROLL, ES_PASSWORD, FLASHWINFO, FLASHW_ALL,
    FLASHW_TIMERNOFG, HMENU, IDCANCEL, IDOK, IDYES, MB_DEFBUTTON2, MB_ICONEXCLAMATION, MB_ICONQUESTION, MB_SETFOREGROUND,
    MB_TOPMOST, MB_YESNO, MSG, NONCLIENTMETRICSW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETNONCLIENTMETRICS, SW_SHOW,
    SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSW,
    WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT, WS_EX_TOPMOST, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use super::einlass::{Anfrage, Oberflaeche};
use super::log;
use crate::strings::{Key, Lang};
use crate::zugang;

const KLASSE_ZULASSEN: PCWSTR = w!("QuadChromaZulassen");
const KLASSE_PASSWORT: PCWSTR = w!("QuadChromaPasswort");
/// Die Anfrage hat sich zurueckgezogen: schliessen, ohne zu antworten.
const WM_ZURUECK: u32 = WM_APP + 20;
/// Kennungen der Felder (OK und Abbrechen sind IDOK und IDCANCEL, damit
/// IsDialogMessageW Eingabe und Esc richtig zuordnet).
const ID_OK: i32 = IDOK.0;
const ID_ABBRECHEN: i32 = IDCANCEL.0;
const ID_NEU: i32 = 10;
const ID_WIEDERHOLEN: i32 = 11;
const ID_MELDUNG: i32 = 12;
/// SS_NOPREFIX: "&" in Namen ist kein Tastenkuerzel.
const SS_NOPREFIX: u32 = 0x80;
/// EM_SETLIMITTEXT (Win32_UI_Controls ist nicht eingebunden).
const EM_SETLIMITTEXT: u32 = 0x00C5;
/// Hoechstens so viele UTF-16-Einheiten je Passwortfeld: 42 * 3 Byte bleibt
/// unter zugang::PASSWORT_MAX (128 Byte UTF-8).
const PASSWORT_ZEICHEN: usize = 42;

/// Sperre nehmen, auch nach einer Panik in einem anderen Faden.
fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(PCWSTR::null()).map(HINSTANCE::from).unwrap_or_default() }
}

/// Masse in Bildpunkten fuer die DPI des Systems.
struct Mass {
    dpi: u32,
}

impl Mass {
    fn neu() -> Mass {
        Mass { dpi: unsafe { GetDpiForSystem() }.max(96) }
    }

    fn px(&self, dip: i32) -> i32 {
        dip * self.dpi as i32 / 96
    }
}

/// Die Nachrichtenschrift des Systems in dieser DPI (Rueckfall: die
/// Standardschrift). Der Aufrufer gibt sie mit DeleteObject frei.
fn schrift(m: &Mass) -> (HFONT, bool) {
    unsafe {
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let ok = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut core::ffi::c_void),
            0,
            m.dpi,
        )
        .is_ok();
        if ok {
            let f = CreateFontIndirectW(&ncm.lfMessageFont);
            if !f.is_invalid() {
                return (f, true);
            }
        }
        (HFONT(GetStockObject(DEFAULT_GUI_FONT).0), false)
    }
}

/// Hoehe eines umbrochenen Texts in dieser Breite.
fn text_hoehe(font: HFONT, text: &str, breite: i32) -> i32 {
    unsafe {
        let dc = GetDC(None);
        let alt = SelectObject(dc, HGDIOBJ(font.0));
        let mut r = RECT { left: 0, top: 0, right: breite, bottom: 0 };
        let mut t: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(dc, &mut t, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(dc, alt);
        ReleaseDC(None, dc);
        r.bottom - r.top
    }
}

/// Ein Kindfenster (Feld) anlegen und mit der Schrift versehen.
#[allow(clippy::too_many_arguments)]
fn feld(eltern: HWND, klasse: PCWSTR, text: &str, stil: u32, ex: WINDOW_EX_STYLE, r: (i32, i32, i32, i32), id: i32, font: HFONT) -> Option<HWND> {
    unsafe {
        let h = CreateWindowExW(
            ex,
            klasse,
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(stil),
            r.0,
            r.1,
            r.2,
            r.3,
            Some(eltern),
            Some(HMENU(id as isize as *mut core::ffi::c_void)),
            Some(hinst()),
            None,
        )
        .ok()?;
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        Some(h)
    }
}

/// Fenster oberster Ebene mit diesem Innenmass anlegen, mittig auf dem
/// Hauptbildschirm (noch unsichtbar).
fn rahmen(klasse: PCWSTR, wndproc: windows::Win32::UI::WindowsAndMessaging::WNDPROC, titel: &str, innen: (i32, i32), ex: WINDOW_EX_STYLE) -> Result<HWND, String> {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: wndproc,
            hInstance: hinst(),
            lpszClassName: klasse,
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        };
        // Schon registriert (zweites Fenster im selben Prozess): kein Fehler.
        RegisterClassW(&wc);
        let stil = WS_CAPTION | WS_SYSMENU;
        let ex = ex | WS_EX_CONTROLPARENT;
        let mut r = RECT { left: 0, top: 0, right: innen.0, bottom: innen.1 };
        let _ = AdjustWindowRectEx(&mut r, stil, false, ex);
        let (b, h) = (r.right - r.left, r.bottom - r.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - b).max(0) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h).max(0) / 3;
        CreateWindowExW(ex, klasse, &HSTRING::from(titel), stil, x, y, b, h, None, None, Some(hinst()), None)
            .map_err(|e| format!("Fenster: {}", e.message()))
    }
}

/// Nachrichtenschleife eines Fensters, bis es zu ist.
fn schleife(hwnd: HWND) {
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Text eines Felds.
fn feldtext(hwnd: HWND, id: i32) -> String {
    unsafe {
        let Ok(f) = GetDlgItem(Some(hwnd), id) else { return String::new() };
        let n = GetWindowTextLengthW(f).max(0) as usize;
        let mut b = vec![0u16; n + 1];
        let k = GetWindowTextW(f, &mut b).max(0) as usize;
        let s = String::from_utf16_lossy(&b[..k.min(b.len())]);
        b.iter_mut().for_each(|x| *x = 0);
        s
    }
}

fn feldtext_setzen(hwnd: HWND, id: i32, text: &str) {
    unsafe {
        if let Ok(f) = GetDlgItem(Some(hwnd), id) {
            let _ = SetWindowTextW(f, &HSTRING::from(text));
        }
    }
}

/// Befehlsnummer aus WM_COMMAND.
fn befehl(wp: WPARAM) -> i32 {
    (wp.0 & 0xffff) as i32
}

// ------------------------------------------------------ Zulassen (10.3)

/// Was das Zulassen-Fenster in seinem Faden weiss.
struct ZulassenDaten {
    antwort: Option<bool>,
    zurueck: bool,
}

thread_local! {
    static ZULASSEN: RefCell<ZulassenDaten> = const { RefCell::new(ZulassenDaten { antwort: None, zurueck: false }) };
}

unsafe extern "system" fn zulassen_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let schliessen = |antwort: Option<bool>, zurueck: bool| {
        ZULASSEN.with(|d| {
            let mut d = d.borrow_mut();
            if d.antwort.is_none() && !d.zurueck {
                d.antwort = antwort;
                d.zurueck = zurueck;
            }
        });
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    };
    match msg {
        WM_COMMAND => {
            match befehl(wp) {
                ID_OK => schliessen(Some(true), false),
                ID_ABBRECHEN => schliessen(Some(false), false),
                _ => {}
            }
            LRESULT(0)
        }
        // Schliessen ueber das Kreuz heisst "Ablehnen".
        WM_CLOSE => {
            schliessen(Some(false), false);
            LRESULT(0)
        }
        WM_ZURUECK => {
            schliessen(None, true);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Ein offenes (oder eben entstehendes) Zulassen-Fenster.
#[derive(Default)]
struct Offen {
    /// 0, solange das Fenster noch entsteht.
    hwnd: isize,
    /// Zurueckgezogen, bevor das Fenster stand.
    zurueck: bool,
}

/// Die Zulassen-Anfragen als Fenster (einlass::Oberflaeche). `antwort`
/// bekommt Nummer und Entscheidung (true = Zulassen) aus dem Fensterfaden;
/// in der Host-Rolle ist das Einlass::entscheiden.
pub struct Zulassen {
    lang: &'static Lang,
    /// Kann jemand klicken? (Das Symbol im Infobereich steht - ohne
    /// angemeldete Sitzung mit Explorer gibt es niemanden.)
    vorhanden: AtomicBool,
    antwort: Arc<dyn Fn(u64, bool) + Send + Sync>,
    offen: Arc<Mutex<HashMap<u64, Offen>>>,
}

impl Zulassen {
    pub fn neu(lang: &'static Lang, antwort: Arc<dyn Fn(u64, bool) + Send + Sync>) -> Zulassen {
        Zulassen { lang, vorhanden: AtomicBool::new(false), antwort, offen: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Ob "Zulassen" moeglich ist (aus dem Takt der Host-Rolle).
    pub fn vorhanden_setzen(&self, ja: bool) {
        self.vorhanden.store(ja, Ordering::Relaxed);
    }

    /// Das Fenster zu Anfrage `nr`, sobald es steht (Tests).
    #[cfg(test)]
    fn fenster_von(&self, nr: u64) -> Option<HWND> {
        sperre(&self.offen).get(&nr).filter(|o| o.hwnd != 0).map(|o| HWND(o.hwnd as *mut _))
    }

    #[cfg(test)]
    fn offen_zahl(&self) -> usize {
        sperre(&self.offen).len()
    }
}

/// Die Texte des Zulassen-Fensters: Frage und Code.
fn zulassen_texte(lang: &Lang, a: &Anfrage) -> (String, String) {
    let id = zugang::id_text(a.id);
    (
        lang.get(Key::HostRequest).replace("{n}", &a.name).replace("{i}", &id),
        lang.get(Key::AccessCode).replace("{c}", &a.code),
    )
}

/// Das Zulassen-Fenster bauen (unsichtbar).
fn zulassen_bauen(lang: &Lang, a: &Anfrage) -> Result<(HWND, HFONT, bool), String> {
    let m = Mass::neu();
    let (font, eigen) = schrift(&m);
    let (frage, code) = zulassen_texte(lang, a);
    let rand = m.px(16);
    let breite = m.px(380);
    let knopf_b = m.px(100);
    let knopf_h = m.px(28);
    let h_frage = text_hoehe(font, &frage, breite).max(m.px(20));
    let h_code = text_hoehe(font, &code, breite).max(m.px(20));
    let innen = (breite + 2 * rand, rand + h_frage + m.px(8) + h_code + m.px(20) + knopf_h + rand);
    let hwnd = rahmen(KLASSE_ZULASSEN, Some(zulassen_proc), "QuadChroma", innen, WS_EX_TOPMOST)?;
    let mut y = rand;
    feld(hwnd, w!("STATIC"), &frage, SS_NOPREFIX, WINDOW_EX_STYLE(0), (rand, y, breite, h_frage), -1, font);
    y += h_frage + m.px(8);
    feld(hwnd, w!("STATIC"), &code, SS_NOPREFIX, WINDOW_EX_STYLE(0), (rand, y, breite, h_code), -1, font);
    y += h_code + m.px(20);
    let x_ab = innen.0 - rand - knopf_b;
    let x_zu = x_ab - m.px(8) - knopf_b;
    let zu = feld(hwnd, w!("BUTTON"), lang.get(Key::HostAllow), WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, WINDOW_EX_STYLE(0), (x_zu, y, knopf_b, knopf_h), ID_OK, font);
    feld(hwnd, w!("BUTTON"), lang.get(Key::HostDeny), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, WINDOW_EX_STYLE(0), (x_ab, y, knopf_b, knopf_h), ID_ABBRECHEN, font);
    if let Some(zu) = zu {
        unsafe {
            let _ = SetFocus(Some(zu));
        }
    }
    Ok((hwnd, font, eigen))
}

/// Der Faden eines Zulassen-Fensters.
fn zulassen_faden(lang: &'static Lang, a: Anfrage, offen: Arc<Mutex<HashMap<u64, Offen>>>, antwort: Arc<dyn Fn(u64, bool) + Send + Sync>) {
    ZULASSEN.with(|d| *d.borrow_mut() = ZulassenDaten { antwort: None, zurueck: false });
    let (hwnd, font, eigen) = match zulassen_bauen(lang, &a) {
        Ok(x) => x,
        Err(e) => {
            // Ohne Fenster bleibt die Anfrage offen, bis die Zugangsphase
            // endet (Passwort, Abbruch, Frist).
            log(format!("Zulassen-Fenster fuer {} nicht angelegt: {e}", a.name));
            sperre(&offen).remove(&a.nr);
            return;
        }
    };
    let weiter = {
        let mut o = sperre(&offen);
        match o.get_mut(&a.nr) {
            Some(x) if !x.zurueck => {
                x.hwnd = hwnd.0 as isize;
                true
            }
            _ => {
                o.remove(&a.nr);
                false
            }
        }
    };
    if weiter {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONEXCLAMATION);
            let fw = FLASHWINFO {
                cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                hwnd,
                dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            let _ = FlashWindowEx(&fw);
        }
        log(format!("Zulassen-Fenster offen: {} (ID {})", a.name, zugang::id_text(a.id)));
        schleife(hwnd);
    } else {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    if eigen {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
    sperre(&offen).remove(&a.nr);
    let d = ZULASSEN.with(|d| std::mem::replace(&mut *d.borrow_mut(), ZulassenDaten { antwort: None, zurueck: false }));
    if let (Some(ja), false) = (d.antwort, d.zurueck) {
        antwort(a.nr, ja);
    }
}

impl Oberflaeche for Zulassen {
    fn vorhanden(&self) -> bool {
        self.vorhanden.load(Ordering::Relaxed)
    }

    fn zeigen(&self, a: &Anfrage) {
        sperre(&self.offen).insert(a.nr, Offen::default());
        let (lang, a2, offen, antwort) = (self.lang, a.clone(), self.offen.clone(), self.antwort.clone());
        let r = std::thread::Builder::new().name("zulassen".into()).spawn(move || zulassen_faden(lang, a2, offen, antwort));
        if let Err(e) = r {
            log(format!("Zulassen-Fenster: kein Faden ({e})"));
            sperre(&self.offen).remove(&a.nr);
        }
    }

    fn schliessen(&self, nr: u64) {
        let mut o = sperre(&self.offen);
        if let Some(x) = o.get_mut(&nr) {
            if x.hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(x.hwnd as *mut _)), WM_ZURUECK, WPARAM(0), LPARAM(0));
                }
            } else {
                x.zurueck = true;
            }
        }
    }
}

// ------------------------------------------------ Passwort aendern (10.4)

/// Die Pruefung im Passwortfenster: beide gleich, dann die Regeln aus
/// zugang.rs. Err: der Text, der im Fenster erscheint.
pub fn passwort_eingabe_pruefen(neu: &str, wieder: &str) -> Result<(), Key> {
    if neu != wieder {
        return Err(Key::HostPasswordsDiffer);
    }
    match zugang::passwort_pruefen(neu.trim_matches([' ', '\t'])) {
        Ok(()) => Ok(()),
        // Zu lang verhindert die Feldgrenze; Steuerzeichen lassen sich
        // kaum eingeben (nur einfuegen) - fuer beides gibt es noch keinen
        // eigenen Text.
        Err(_) => Err(Key::HostPasswordShort),
    }
}

/// Was das Passwortfenster in seinem Faden weiss.
struct PasswortDaten {
    lang: &'static Lang,
    setzen: Box<dyn Fn(&str) -> Result<(), zugang::PasswortFehler>>,
    gespeichert: bool,
}

thread_local! {
    static PASSWORT: RefCell<Option<PasswortDaten>> = const { RefCell::new(None) };
}

/// Das offene Passwortfenster (0: keins) - es gibt hoechstens eines.
static PASSWORT_FENSTER: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn passwort_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match befehl(wp) {
                ID_OK => {
                    let mut neu = feldtext(hwnd, ID_NEU);
                    let mut wieder = feldtext(hwnd, ID_WIEDERHOLEN);
                    let fertig = PASSWORT.with(|p| {
                        let mut p = p.borrow_mut();
                        let Some(d) = p.as_mut() else { return true };
                        let meldung = match passwort_eingabe_pruefen(&neu, &wieder) {
                            Err(k) => Some(d.lang.get(k)),
                            Ok(()) => match (d.setzen)(&neu) {
                                Ok(()) => {
                                    d.gespeichert = true;
                                    None
                                }
                                Err(zugang::PasswortFehler::ZuKurz) => Some(d.lang.get(Key::HostPasswordShort)),
                                // Schreibfehler: dafuer gibt es noch keinen
                                // eigenen Text - die Zeile im Protokoll nennt
                                // den Grund.
                                Err(_) => Some(d.lang.get(Key::HostPasswordUnreadable)),
                            },
                        };
                        match meldung {
                            Some(t) => {
                                feldtext_setzen(hwnd, ID_MELDUNG, t);
                                false
                            }
                            None => true,
                        }
                    });
                    // Den Klartext nicht laenger als noetig im Speicher halten.
                    unsafe {
                        neu.as_mut_vec().iter_mut().for_each(|b| *b = 0);
                        wieder.as_mut_vec().iter_mut().for_each(|b| *b = 0);
                    }
                    if fertig {
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                }
                ID_ABBRECHEN => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Die Felder leeren, bevor sie verschwinden.
            feldtext_setzen(hwnd, ID_NEU, "");
            feldtext_setzen(hwnd, ID_WIEDERHOLEN, "");
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Titel aus "Passwort aendern ...": ohne die Auslassungspunkte des Menues.
fn ohne_punkte(t: &str) -> &str {
    t.trim_end_matches(['.', '…', ' '])
}

fn passwort_bauen(lang: &Lang) -> Result<(HWND, HFONT, bool), String> {
    let m = Mass::neu();
    let (font, eigen) = schrift(&m);
    let rand = m.px(16);
    let breite = m.px(320);
    let zeile = m.px(20);
    let feld_h = m.px(24);
    let knopf_b = m.px(100);
    let knopf_h = m.px(28);
    let h_neu = text_hoehe(font, lang.get(Key::HostNewPassword), breite).max(zeile);
    let h_wieder = text_hoehe(font, lang.get(Key::HostRepeatPassword), breite).max(zeile);
    let h_meldung = 2 * zeile;
    let innen = (
        breite + 2 * rand,
        rand + h_neu + m.px(4) + feld_h + m.px(10) + h_wieder + m.px(4) + feld_h + m.px(8) + h_meldung + m.px(8) + knopf_h + rand,
    );
    let hwnd = rahmen(KLASSE_PASSWORT, Some(passwort_proc), ohne_punkte(lang.get(Key::HostChangePassword)), innen, WINDOW_EX_STYLE(0))?;
    let passwort_stil = WS_TABSTOP.0 | (ES_PASSWORD | ES_AUTOHSCROLL) as u32;
    let mut y = rand;
    feld(hwnd, w!("STATIC"), lang.get(Key::HostNewPassword), SS_NOPREFIX, WINDOW_EX_STYLE(0), (rand, y, breite, h_neu), -1, font);
    y += h_neu + m.px(4);
    let neu = feld(hwnd, w!("EDIT"), "", passwort_stil, WS_EX_CLIENTEDGE, (rand, y, breite, feld_h), ID_NEU, font);
    y += feld_h + m.px(10);
    feld(hwnd, w!("STATIC"), lang.get(Key::HostRepeatPassword), SS_NOPREFIX, WINDOW_EX_STYLE(0), (rand, y, breite, h_wieder), -1, font);
    y += h_wieder + m.px(4);
    let wieder = feld(hwnd, w!("EDIT"), "", passwort_stil, WS_EX_CLIENTEDGE, (rand, y, breite, feld_h), ID_WIEDERHOLEN, font);
    y += feld_h + m.px(8);
    feld(hwnd, w!("STATIC"), "", SS_NOPREFIX, WINDOW_EX_STYLE(0), (rand, y, breite, h_meldung), ID_MELDUNG, font);
    y += h_meldung + m.px(8);
    let x_ab = innen.0 - rand - knopf_b;
    let x_ok = x_ab - m.px(8) - knopf_b;
    feld(hwnd, w!("BUTTON"), lang.get(Key::HostOk), WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, WINDOW_EX_STYLE(0), (x_ok, y, knopf_b, knopf_h), ID_OK, font);
    feld(hwnd, w!("BUTTON"), lang.get(Key::AccessCancel), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, WINDOW_EX_STYLE(0), (x_ab, y, knopf_b, knopf_h), ID_ABBRECHEN, font);
    for f in [neu, wieder].into_iter().flatten() {
        unsafe {
            SendMessageW(f, EM_SETLIMITTEXT, Some(WPARAM(PASSWORT_ZEICHEN)), Some(LPARAM(0)));
        }
    }
    if let Some(neu) = neu {
        unsafe {
            let _ = SetFocus(Some(neu));
        }
    }
    Ok((hwnd, font, eigen))
}

/// "Passwort aendern ...": das Fenster in einem eigenen Faden oeffnen (ist
/// es schon offen, nach vorn holen). `setzen` speichert (Einlass::
/// passwort_setzen); `gespeichert` kommt nach dem Speichern, wenn das
/// Fenster zu ist (Sprechblase "Passwort gespeichert.").
pub fn passwort_aendern(
    lang: &'static Lang,
    setzen: Box<dyn Fn(&str) -> Result<(), zugang::PasswortFehler> + Send>,
    gespeichert: Box<dyn FnOnce() + Send>,
) {
    let offen = PASSWORT_FENSTER.load(Ordering::SeqCst);
    if offen != 0 && unsafe { IsWindow(Some(HWND(offen as *mut _))) }.as_bool() {
        unsafe {
            let _ = SetForegroundWindow(HWND(offen as *mut _));
        }
        return;
    }
    let r = std::thread::Builder::new().name("passwortfenster".into()).spawn(move || {
        let (hwnd, font, eigen) = match passwort_bauen(lang) {
            Ok(x) => x,
            Err(e) => {
                log(format!("Passwortfenster nicht angelegt: {e}"));
                return;
            }
        };
        PASSWORT.with(|p| *p.borrow_mut() = Some(PasswortDaten { lang, setzen, gespeichert: false }));
        PASSWORT_FENSTER.store(hwnd.0 as isize, Ordering::SeqCst);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        schleife(hwnd);
        PASSWORT_FENSTER.store(0, Ordering::SeqCst);
        if eigen {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
        let ok = PASSWORT.with(|p| p.borrow_mut().take().is_some_and(|d| d.gespeichert));
        if ok {
            gespeichert();
        }
    });
    if let Err(e) = r {
        log(format!("Passwortfenster: kein Faden ({e})"));
    }
}

// ---------------------------------------------------------- Rueckfrage

/// Eine Ja/Nein-Rueckfrage in eigenem Faden (Nein ist die Vorgabe); bei Ja
/// laeuft `ja` in diesem Faden.
pub fn rueckfrage(text: &str, ja: Box<dyn FnOnce() + Send>) {
    let text = text.to_string();
    let r = std::thread::Builder::new().name("rueckfrage".into()).spawn(move || {
        let a = unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(text),
                w!("QuadChroma"),
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND,
            )
        };
        if a == IDYES {
            ja();
        }
    });
    if let Err(e) = r {
        log(format!("Rueckfrage: kein Faden ({e})"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn warten_bis(mut f: impl FnMut() -> bool) -> bool {
        let bis = Instant::now() + Duration::from_secs(5);
        while Instant::now() < bis {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn anfrage(nr: u64) -> Anfrage {
        Anfrage { nr, name: "Büro-PC & Co".into(), id: 581_729_911, code: "628 306".into() }
    }

    /// Texte des Fensters: Name, ID ("ddd ddd ddd"), Code.
    #[test]
    fn zulassen_texte_mit_name_id_code() {
        let de = crate::strings::pick("de");
        let (frage, code) = zulassen_texte(de, &anfrage(1));
        assert_eq!(frage, "Büro-PC & Co (ID 581 729 911) möchte diesen Computer steuern.");
        assert_eq!(code, "Code: 628 306");
        let (frage, _) = zulassen_texte(crate::strings::pick("en"), &anfrage(1));
        assert_eq!(frage, "Büro-PC & Co (ID 581 729 911) wants to control this computer.");
    }

    /// Das echte Fenster (auch ohne angemeldete Sitzung zu bauen): ein
    /// "Klick" auf Zulassen (WM_COMMAND IDOK, wie ihn der Knopf schickt)
    /// antwortet true und schliesst; zurueckgezogen schliesst es ohne
    /// Antwort - auch dann, wenn es noch gar nicht stand; Schliessen ueber
    /// das Kreuz heisst Ablehnen.
    #[test]
    fn zulassen_fenster_antwortet_und_zieht_zurueck() {
        let (tx, rx) = mpsc::channel::<(u64, bool)>();
        let tx = Mutex::new(tx);
        let z = Zulassen::neu(crate::strings::pick("de"), Arc::new(move |nr, ja| {
            let _ = sperre(&tx).send((nr, ja));
        }));
        z.vorhanden_setzen(true);
        assert!(Oberflaeche::vorhanden(&z));
        // Zulassen.
        z.zeigen(&anfrage(1));
        assert!(warten_bis(|| z.fenster_von(1).is_some()), "Fenster 1 entstand nicht");
        let h = z.fenster_von(1).unwrap();
        // Die Knoepfe tragen die Texte der Sprache.
        let mut b = [0u16; 64];
        let n = unsafe { GetWindowTextW(GetDlgItem(Some(h), ID_OK).unwrap(), &mut b) } as usize;
        assert_eq!(String::from_utf16_lossy(&b[..n]), "Zulassen");
        let n = unsafe { GetWindowTextW(GetDlgItem(Some(h), ID_ABBRECHEN).unwrap(), &mut b) } as usize;
        assert_eq!(String::from_utf16_lossy(&b[..n]), "Ablehnen");
        unsafe { PostMessageW(Some(h), WM_COMMAND, WPARAM(ID_OK as usize), LPARAM(0)).unwrap() };
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok((1, true)));
        assert!(warten_bis(|| !unsafe { IsWindow(Some(h)) }.as_bool()), "Fenster 1 blieb offen");
        // Zurueckgezogen: zu, ohne Antwort.
        z.zeigen(&anfrage(2));
        assert!(warten_bis(|| z.fenster_von(2).is_some()));
        let h = z.fenster_von(2).unwrap();
        z.schliessen(2);
        assert!(warten_bis(|| !unsafe { IsWindow(Some(h)) }.as_bool()), "zurueckgezogenes Fenster blieb offen");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "Antwort trotz Rueckzug");
        // Zurueckgezogen, bevor es stand.
        z.zeigen(&anfrage(3));
        z.schliessen(3);
        assert!(warten_bis(|| z.offen_zahl() == 0), "Eintrag 3 blieb");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        // Das Kreuz: Ablehnen.
        z.zeigen(&anfrage(4));
        assert!(warten_bis(|| z.fenster_von(4).is_some()));
        unsafe { PostMessageW(z.fenster_von(4), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap() };
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok((4, false)));
    }

    #[test]
    fn passwort_eingabe_regeln() {
        assert_eq!(passwort_eingabe_pruefen("abcdefgh", "abcdefgh"), Ok(()));
        assert_eq!(passwort_eingabe_pruefen("abcdefgh", "abcdefgi"), Err(Key::HostPasswordsDiffer));
        // norm zaehlt: Bindestriche und Leerzeichen fallen weg.
        assert_eq!(passwort_eingabe_pruefen("ab-cd ef-g", "ab-cd ef-g"), Err(Key::HostPasswordShort));
        assert_eq!(passwort_eingabe_pruefen("k7m-4wq-9tz", "k7m-4wq-9tz"), Ok(()));
        assert_eq!(passwort_eingabe_pruefen("", ""), Err(Key::HostPasswordShort));
        assert_eq!(passwort_eingabe_pruefen("mit\ttab-abcdefgh", "mit\ttab-abcdefgh"), Err(Key::HostPasswordShort));
        assert_eq!(ohne_punkte("Passwort ändern …"), "Passwort ändern");
        assert_eq!(ohne_punkte("Change password ..."), "Change password");
    }

    /// Das echte Passwortfenster: ungleiche Eingaben zeigen den Text und
    /// speichern nichts; gleiche und lange genug speichern, schliessen das
    /// Fenster und melden "gespeichert". Ein zweites Oeffnen, waehrend es
    /// offen ist, legt kein zweites an.
    #[test]
    fn passwort_fenster_prueft_und_speichert() {
        let (tx, rx) = mpsc::channel::<String>();
        let (fertig_tx, fertig_rx) = mpsc::channel::<()>();
        let de = crate::strings::pick("de");
        passwort_aendern(
            de,
            Box::new(move |pw| {
                let _ = tx.send(pw.to_string());
                Ok(())
            }),
            Box::new(move || {
                let _ = fertig_tx.send(());
            }),
        );
        assert!(warten_bis(|| PASSWORT_FENSTER.load(Ordering::SeqCst) != 0), "Passwortfenster entstand nicht");
        let h = HWND(PASSWORT_FENSTER.load(Ordering::SeqCst) as *mut _);
        // Zweites Oeffnen: dasselbe Fenster.
        passwort_aendern(de, Box::new(|_| Ok(())), Box::new(|| {}));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(PASSWORT_FENSTER.load(Ordering::SeqCst), h.0 as isize);
        // Ungleich.
        feldtext_setzen(h, ID_NEU, "geheim-123");
        feldtext_setzen(h, ID_WIEDERHOLEN, "geheim-124");
        unsafe { SendMessageW(h, WM_COMMAND, Some(WPARAM(ID_OK as usize)), Some(LPARAM(0))) };
        assert_eq!(feldtext(h, ID_MELDUNG), "Die Passwörter stimmen nicht überein.");
        assert!(rx.try_recv().is_err());
        // Zu kurz.
        feldtext_setzen(h, ID_NEU, "kurz");
        feldtext_setzen(h, ID_WIEDERHOLEN, "kurz");
        unsafe { SendMessageW(h, WM_COMMAND, Some(WPARAM(ID_OK as usize)), Some(LPARAM(0))) };
        assert_eq!(feldtext(h, ID_MELDUNG), "Bitte mindestens 8 Zeichen.");
        // Richtig.
        feldtext_setzen(h, ID_NEU, "Geheim 1234");
        feldtext_setzen(h, ID_WIEDERHOLEN, "Geheim 1234");
        unsafe { PostMessageW(Some(h), WM_COMMAND, WPARAM(ID_OK as usize), LPARAM(0)).unwrap() };
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("Geheim 1234"));
        assert!(fertig_rx.recv_timeout(Duration::from_secs(5)).is_ok(), "gespeichert nicht gemeldet");
        assert!(warten_bis(|| !unsafe { IsWindow(Some(h)) }.as_bool()));
        assert_eq!(PASSWORT_FENSTER.load(Ordering::SeqCst), 0);
    }
}
