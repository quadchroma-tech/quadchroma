// Eingaben des Windows-Hosts: Maus und Tastatur ueber SendInput.
//
// Ueber die Leitung kommen macOS-Virtual-Keycodes (Tabelle `mac_keycode`
// im Client, der Mac nimmt sie 1:1 als CGKeyCode). Das bleibt das
// Leitungsformat - ein Client darf den Host nicht erkennen -, der Windows-
// Host uebersetzt zurueck: macOS-Keycode -> Scancode (Set 1, mit E0-
// Kennzeichen). Die Leitung ist positionsbasiert, also Befehlstaste (55/54)
// -> Windows-Taste, Wahltaste (58/61) -> Alt: Cmd+C vom Mac-Client ist hier
// Win+C. Ein Schalter "Befehlstaste als Strg" ist eine spaetere Einstellung.
//
// Die mods-Bitmaske der Tastennachricht wird NICHT auf Ereignisse gelegt:
// die Umschalter kommen als eigene Tastendruecke (der Client schickt alle
// Tasten). Gehaltene Tasten werden mitgeschrieben und beim Trennen
// losgelassen - sonst haelt Windows sie fuer immer gedrueckt.
//
// Wiederholungen: eingespeiste Tasten wiederholt Windows nicht selbst (das
// tut bei einer echten Tastatur deren Treiber). Der Client schickt die
// Wiederholungen seines Systems als weitere Druecke (Merkmal
// TASTE_WIEDERHOLUNG); hier geht jeder ungefiltert als Druck durch SendInput
// - genau das liefert auch eine echte Tastatur, und Windows kennzeichnet
// ihn selbst als Wiederholung (WM_KEYDOWN, lParam Bit 30: war schon
// gedrueckt). Das Merkmal braucht es deshalb hier nicht.
//
// UIPI: SendInput erreicht keine Fenster mit hoeheren Rechten als der eigene
// Prozess. Die App laeuft jetzt erhoeht (Manifest requireAdministrator), also
// erreicht sie den Task-Manager, den Registrierungs-Editor und Installer im
// Sitzungskontext des Nutzers. Ausser Reichweite bleibt allein die echte
// UAC-Bestaetigung auf dem sicheren Desktop (dafuer braeuchte es einen
// SYSTEM-Dienst). Lehnt SendInput ab (sicherer Desktop oder gesperrter
// Bildschirm), wird das vermerkt und der Zuschauer bekommt einen kurzen
// Hinweis (Hoststatus 2/3) - umgangen wird nichts.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::{log, Z};
use crate::protokoll_konst::*;

/// Der Ausgang, auf den sich Bild und Maus beziehen (Desktop-Koordinaten).
struct Ausgang {
    links: i32,
    oben: i32,
    breite: i32,
    hoehe: i32,
}

struct Stand {
    ausgang: Ausgang,
    /// Zuletzt gesetzte Position (Desktop-Koordinaten), fuer Tasten und Rad.
    pos: (i32, i32),
    /// Bitmaske der gedrueckten Maustasten.
    buttons: u8,
    /// Welche Tasten wir selbst gedrueckt haben (macOS-Keycode).
    key_down: [bool; 256],
    /// Restakkumulation des Rads (Windows will ganze Einheiten).
    rad_rest: (f32, f32),
}

static STAND: Mutex<Stand> = Mutex::new(Stand {
    ausgang: Ausgang { links: 0, oben: 0, breite: 1920, hoehe: 1080 },
    pos: (0, 0),
    buttons: 0,
    key_down: [false; 256],
    rad_rest: (0.0, 0.0),
});
static UIPI_GEMELDET: AtomicBool = AtomicBool::new(false);
static UNBEKANNT_GEMELDET: AtomicBool = AtomicBool::new(false);
/// Liegt gerade ein Fenster mit hoeheren Rechten vorn, das die Eingaben
/// schluckt? Dann wurde dem Zuschauer der Hinweis geschickt (Hoststatus 2);
/// kommt eine Eingabe wieder durch, wird er zurueckgenommen (Hoststatus 3).
static EINGABE_BLOCKIERT: AtomicBool = AtomicBool::new(false);
/// Wann der Hinweis zuletzt hinausging (Hostuhr, us) - zur Drosselung.
static BLOCK_ZULETZT_US: AtomicU64 = AtomicU64::new(0);
/// Hoechstens alle drei Sekunden geht der Hinweis erneut hinaus, damit die
/// Leitung bei anhaltender Blockade nicht mit Statusnachrichten volllaeuft.
const BLOCK_MELDE_ABSTAND_US: u64 = 3_000_000;

/// Bild und Maus gehoeren auf denselben Ausgang - hier seine Geometrie.
pub fn ausgang_setzen(links: i32, oben: i32, breite: i32, hoehe: i32) {
    let mut s = STAND.lock().unwrap();
    s.ausgang = Ausgang { links, oben, breite: breite.max(1), hoehe: hoehe.max(1) };
}

pub fn start() {
    log("Eingaben: SendInput; Fenster mit hoeheren Rechten (UAC) bekommen keine Eingaben - das ist Windows so");
    rechte_melden();
}

/// Einmal beim Start: laeuft die App mit erhoehten Rechten? Mit dem Manifest
/// requireAdministrator ist sie es immer (Windows fragt beim Start ueber UAC).
/// Startet eine portable Kopie ohne dieses Manifest ohne Erhoehung, laeuft die
/// Host-Rolle trotzdem weiter - nur erreicht SendInput dann keine Fenster mit
/// hoeheren Rechten. Kein Selbst-Neustart, nur der Vermerk.
fn rechte_melden() {
    if erhoeht() {
        log("Rechte: erhoeht");
    } else {
        log("Rechte: normal - SendInput erreicht keine Fenster mit hoeheren Rechten");
    }
}

/// Ist der Prozess erhoeht (Token-Elevation)? Ohne lesbares Token: als nicht
/// erhoeht behandelt.
fn erhoeht() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE(std::ptr::null_mut());
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut info = TOKEN_ELEVATION::default();
        let mut laenge = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut info as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut laenge,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && info.TokenIsElevated != 0
    }
}

/// Windows nimmt gerade keine Eingaben an (SendInput abgelehnt): dem Zuschauer
/// sagen, dass ein Fenster mit hoeheren Rechten (die echte UAC-Bestaetigung
/// auf dem sicheren Desktop) vorn liegt - hoechstens alle paar Sekunden.
fn eingabe_blockiert_melden() {
    let erstmals = !EINGABE_BLOCKIERT.swap(true, Ordering::Relaxed);
    let jetzt = super::now_us();
    let zuletzt = BLOCK_ZULETZT_US.load(Ordering::Relaxed);
    if erstmals || jetzt.saturating_sub(zuletzt) >= BLOCK_MELDE_ABSTAND_US {
        BLOCK_ZULETZT_US.store(jetzt, Ordering::Relaxed);
        // Hoststatus 2: ein Fenster mit hoeheren Rechten liegt vorn.
        super::netz::hoststatus_senden(2);
    }
}

/// Eine Eingabe kam wieder durch: einen zuvor gesendeten Hinweis zuruecknehmen
/// (Hoststatus 3). Im Normalfall (nie blockiert) nur ein billiger Atomtausch.
fn eingabe_wieder_frei() {
    if EINGABE_BLOCKIERT.swap(false, Ordering::Relaxed) {
        super::netz::hoststatus_senden(3);
    }
}

// ------------------------------------------------------- Scancode-Tabelle

/// macOS-Keycode -> (Scancode Set 1, E0-Kennzeichen). Umkehrung von
/// `mac_keycode` im Client plus W3C-Code -> Set 1.
pub fn scancode(mac: u16) -> Option<(u8, bool)> {
    Some(match mac {
        0 => (0x1E, false),   // A
        1 => (0x1F, false),   // S
        2 => (0x20, false),   // D
        3 => (0x21, false),   // F
        4 => (0x23, false),   // H
        5 => (0x22, false),   // G
        6 => (0x2C, false),   // Z (Position)
        7 => (0x2D, false),   // X
        8 => (0x2E, false),   // C
        9 => (0x2F, false),   // V
        10 => (0x56, false),  // IntlBackslash (Taste neben der linken Umschalttaste)
        11 => (0x30, false),  // B
        12 => (0x10, false),  // Q
        13 => (0x11, false),  // W
        14 => (0x12, false),  // E
        15 => (0x13, false),  // R
        16 => (0x15, false),  // Y (Position)
        17 => (0x14, false),  // T
        18 => (0x02, false),  // 1
        19 => (0x03, false),  // 2
        20 => (0x04, false),  // 3
        21 => (0x05, false),  // 4
        22 => (0x07, false),  // 6
        23 => (0x06, false),  // 5
        24 => (0x0D, false),  // =
        25 => (0x0A, false),  // 9
        26 => (0x08, false),  // 7
        27 => (0x0C, false),  // -
        28 => (0x09, false),  // 8
        29 => (0x0B, false),  // 0
        30 => (0x1B, false),  // ]
        31 => (0x18, false),  // O
        32 => (0x16, false),  // U
        33 => (0x1A, false),  // [
        34 => (0x17, false),  // I
        35 => (0x19, false),  // P
        36 => (0x1C, false),  // Enter
        37 => (0x26, false),  // L
        38 => (0x24, false),  // J
        39 => (0x28, false),  // '
        40 => (0x25, false),  // K
        41 => (0x27, false),  // ;
        42 => (0x2B, false),  // Backslash
        43 => (0x33, false),  // ,
        44 => (0x35, false),  // /
        45 => (0x31, false),  // N
        46 => (0x32, false),  // M
        47 => (0x34, false),  // .
        48 => (0x0F, false),  // Tab
        49 => (0x39, false),  // Leertaste
        50 => (0x29, false),  // `
        51 => (0x0E, false),  // Ruecktaste
        53 => (0x01, false),  // Esc
        54 => (0x5C, true),   // rechte Befehlstaste -> rechte Windows-Taste
        55 => (0x5B, true),   // linke Befehlstaste -> linke Windows-Taste
        56 => (0x2A, false),  // linke Umschalttaste
        57 => (0x3A, false),  // Feststelltaste
        58 => (0x38, false),  // linke Wahltaste -> Alt
        59 => (0x1D, false),  // linke Steuerung
        60 => (0x36, false),  // rechte Umschalttaste
        61 => (0x38, true),   // rechte Wahltaste -> AltGr
        62 => (0x1D, true),   // rechte Steuerung
        65 => (0x53, false),  // Ziffernblock .
        67 => (0x37, false),  // Ziffernblock *
        69 => (0x4E, false),  // Ziffernblock +
        75 => (0x35, true),   // Ziffernblock /
        76 => (0x1C, true),   // Ziffernblock Enter
        78 => (0x4A, false),  // Ziffernblock -
        82 => (0x52, false),  // Ziffernblock 0
        83 => (0x4F, false),  // 1
        84 => (0x50, false),  // 2
        85 => (0x51, false),  // 3
        86 => (0x4B, false),  // 4
        87 => (0x4C, false),  // 5
        88 => (0x4D, false),  // 6
        89 => (0x47, false),  // 7
        91 => (0x48, false),  // 8
        92 => (0x49, false),  // 9
        96 => (0x3F, false),  // F5
        97 => (0x40, false),  // F6
        98 => (0x41, false),  // F7
        99 => (0x3D, false),  // F3
        100 => (0x42, false), // F8
        101 => (0x43, false), // F9
        103 => (0x57, false), // F11
        109 => (0x44, false), // F10
        111 => (0x58, false), // F12
        115 => (0x47, true),  // Pos1
        116 => (0x49, true),  // Bild auf
        117 => (0x53, true),  // Entf
        118 => (0x3E, false), // F4
        119 => (0x4F, true),  // Ende
        120 => (0x3C, false), // F2
        121 => (0x51, true),  // Bild ab
        122 => (0x3B, false), // F1
        123 => (0x4B, true),  // Links
        124 => (0x4D, true),  // Rechts
        125 => (0x50, true),  // Ab
        126 => (0x48, true),  // Auf
        _ => return None,
    })
}

// ---------------------------------------------------------------- Senden

fn senden(eingaben: &[INPUT]) {
    let n = unsafe { SendInput(eingaben, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != eingaben.len() {
        if !UIPI_GEMELDET.swap(true, Ordering::Relaxed) {
            log("Eingaben: SendInput nimmt gerade nichts an (Fenster mit hoeheren Rechten im Vordergrund oder gesperrter Bildschirm)");
        }
        // Auch der erhoehten App bleibt der sichere Desktop (UAC-Bestaetigung)
        // verwehrt - dem Zuschauer sagen, dass die Steuerung dort kurz nicht geht.
        eingabe_blockiert_melden();
    } else {
        Z.input_events.fetch_add(1, Ordering::Relaxed);
        eingabe_wieder_frei();
    }
}

fn maus(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, daten: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: daten as _, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Absolute Position auf dem virtuellen Desktop (0..65535 je Achse) fuer
/// einen Punkt in Desktop-Koordinaten.
fn absolut(x: i32, y: i32) -> (i32, i32) {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    let ax = (((x - vx) as i64 * 65535 + (vw as i64 / 2)) / vw as i64).clamp(0, 65535) as i32;
    let ay = (((y - vy) as i64 * 65535 + (vh as i64 / 2)) / vh as i64).clamp(0, 65535) as i32;
    (ax, ay)
}

/// nx/ny in 0..1 der Bildflaeche -> Desktop-Koordinaten des Ausgangs.
fn punkt(s: &Stand, nx: f32, ny: f32) -> (i32, i32) {
    let nx = if nx.is_finite() { nx.clamp(0.0, 1.0) } else { 0.5 };
    let ny = if ny.is_finite() { ny.clamp(0.0, 1.0) } else { 0.5 };
    let x = s.ausgang.links + ((nx * (s.ausgang.breite - 1) as f32).round() as i32);
    let y = s.ausgang.oben + ((ny * (s.ausgang.hoehe - 1) as f32).round() as i32);
    (x, y)
}

fn bewegung(s: &mut Stand, nx: f32, ny: f32) -> INPUT {
    let p = punkt(s, nx, ny);
    s.pos = p;
    let (ax, ay) = absolut(p.0, p.1);
    maus(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, ax, ay, 0)
}

fn inject_move(nx: f32, ny: f32) {
    let mut s = STAND.lock().unwrap();
    let e = bewegung(&mut s, nx, ny);
    drop(s);
    senden(&[e]);
}

fn inject_button(button: u8, down: bool, nx: f32, ny: f32) {
    let mut s = STAND.lock().unwrap();
    let m = bewegung(&mut s, nx, ny);
    let (flag, bit) = match (button, down) {
        (1, true) => (MOUSEEVENTF_RIGHTDOWN, 2u8),
        (1, false) => (MOUSEEVENTF_RIGHTUP, 2),
        (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 4),
        (2, false) => (MOUSEEVENTF_MIDDLEUP, 4),
        (_, true) => (MOUSEEVENTF_LEFTDOWN, 1),
        (_, false) => (MOUSEEVENTF_LEFTUP, 1),
    };
    if down {
        s.buttons |= bit;
    } else {
        s.buttons &= !bit;
    }
    drop(s);
    // Position vorher setzen, wie inject_button auf dem Mac; Doppelklicks
    // erkennt Windows selbst am Abstand der Klicks.
    senden(&[m, maus(flag, 0, 0, 0)]);
}

/// Rad: dy in Pixeln (der Client schickt LineDelta*40) -> Windows-Einheiten
/// (120 je Raste): mouseData = dy * 120 / 40, Rest wird gesammelt. Windows
/// zaehlt positiv = vom Benutzer weg (nach oben), wie winit und der Mac.
/// dx -> waagerechtes Rad; das Vorzeichen ist auf dem Laptop zu pruefen.
fn inject_scroll(dx: f32, dy: f32) {
    let mut s = STAND.lock().unwrap();
    s.rad_rest.0 += dx * 3.0;
    s.rad_rest.1 += dy * 3.0;
    let wx = s.rad_rest.0.trunc();
    let wy = s.rad_rest.1.trunc();
    s.rad_rest.0 -= wx;
    s.rad_rest.1 -= wy;
    drop(s);
    let mut e = Vec::new();
    if wy != 0.0 {
        e.push(maus(MOUSEEVENTF_WHEEL, 0, 0, wy as i32));
    }
    if wx != 0.0 {
        e.push(maus(MOUSEEVENTF_HWHEEL, 0, 0, wx as i32));
    }
    if !e.is_empty() {
        senden(&e);
    }
}

fn taste(sc: u8, e0: bool, down: bool) -> INPUT {
    let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
    if e0 {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: sc as u16, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn inject_key(keycode: u16, down: bool, _mods: u32) {
    let Some((sc, e0)) = scancode(keycode) else {
        if !UNBEKANNT_GEMELDET.swap(true, Ordering::Relaxed) {
            log(format!("Eingaben: macOS-Keycode {keycode} hat keinen Scancode - uebergangen (nur einmal gemeldet)"));
        }
        return;
    };
    // Mitschreiben, was gerade gedrueckt ist - das ist die Grundlage fuer das
    // Freigeben, wenn die Verbindung wegbricht.
    if (keycode as usize) < 256 {
        STAND.lock().unwrap().key_down[keycode as usize] = down;
    }
    senden(&[taste(sc, e0, down)]);
}

/// Alles loslassen, was noch als gedrueckt vermerkt ist: Tasten und Maustasten.
pub fn alle_tasten_loslassen() {
    let mut s = STAND.lock().unwrap();
    let mut e: Vec<INPUT> = Vec::new();
    let mut offen = 0;
    for k in 0..256u16 {
        if s.key_down[k as usize] {
            s.key_down[k as usize] = false;
            if let Some((sc, e0)) = scancode(k) {
                e.push(taste(sc, e0, false));
                offen += 1;
            }
        }
    }
    if s.buttons & 1 != 0 {
        e.push(maus(MOUSEEVENTF_LEFTUP, 0, 0, 0));
    }
    if s.buttons & 2 != 0 {
        e.push(maus(MOUSEEVENTF_RIGHTUP, 0, 0, 0));
    }
    if s.buttons & 4 != 0 {
        e.push(maus(MOUSEEVENTF_MIDDLEUP, 0, 0, 0));
    }
    let tasten = s.buttons.count_ones();
    s.buttons = 0;
    s.rad_rest = (0.0, 0.0);
    drop(s);
    if !e.is_empty() {
        senden(&e);
    }
    if offen > 0 || tasten > 0 {
        log(format!("Verbindung weg: {offen} haengende Taste(n) und {tasten} Maustaste(n) freigegeben"));
    }
}

/// Eine Eingabenachricht vom Client (16-19) ausfuehren.
pub fn verarbeiten(typ: u8, p: &[u8]) {
    let f32le = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
    match typ {
        IN_MOVE if p.len() >= 8 => inject_move(f32le(0), f32le(4)),
        IN_BUTTON if p.len() >= 12 => inject_button(p[0], p[1] != 0, f32le(4), f32le(8)),
        IN_SCROLL if p.len() >= 8 => inject_scroll(f32le(0), f32le(4)),
        IN_KEY if p.len() >= 8 => {
            let kc = u16::from_le_bytes([p[0], p[1]]);
            let mods = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
            inject_key(kc, p[2] != 0, mods);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::scancode;
    use winit::keyboard::KeyCode as K;

    /// Rundlauf: jeder Wert, den `mac_keycode` des Clients liefert, hat
    /// einen Scancode - sonst kaeme eine Taste des Clients hier nie an.
    #[test]
    fn jeder_mac_keycode_hat_einen_scancode() {
        let alle = [
            K::KeyA, K::KeyS, K::KeyD, K::KeyF, K::KeyH, K::KeyG, K::KeyZ, K::KeyX, K::KeyC, K::KeyV, K::KeyB,
            K::KeyQ, K::KeyW, K::KeyE, K::KeyR, K::KeyY, K::KeyT, K::Digit1, K::Digit2, K::Digit3, K::Digit4,
            K::Digit6, K::Digit5, K::Equal, K::Digit9, K::Digit7, K::Minus, K::Digit8, K::Digit0,
            K::BracketRight, K::KeyO, K::KeyU, K::BracketLeft, K::KeyI, K::KeyP, K::Enter, K::KeyL, K::KeyJ,
            K::Quote, K::KeyK, K::Semicolon, K::Backslash, K::Comma, K::Slash, K::KeyN, K::KeyM, K::Period,
            K::Tab, K::Space, K::Backquote, K::Backspace, K::Escape, K::IntlBackslash, K::SuperLeft,
            K::ShiftLeft, K::CapsLock, K::AltLeft, K::ControlLeft, K::ShiftRight, K::AltRight,
            K::ControlRight, K::SuperRight, K::NumpadDecimal, K::NumpadMultiply, K::NumpadAdd,
            K::NumpadDivide, K::NumpadEnter, K::NumpadSubtract, K::Numpad0, K::Numpad1, K::Numpad2,
            K::Numpad3, K::Numpad4, K::Numpad5, K::Numpad6, K::Numpad7, K::Numpad8, K::Numpad9, K::F1, K::F2,
            K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12, K::Home, K::PageUp,
            K::Delete, K::End, K::PageDown, K::ArrowLeft, K::ArrowRight, K::ArrowDown, K::ArrowUp,
        ];
        let mut n = 0;
        for k in alle {
            let mac = crate::mac_keycode(k).unwrap_or_else(|| panic!("{k:?} hat keinen macOS-Keycode"));
            assert!(scancode(mac).is_some(), "{k:?} (macOS {mac}) hat keinen Scancode");
            n += 1;
        }
        assert_eq!(n, 99);
    }

    /// Kein Scancode darf doppelt vergeben sein - ausser den Paaren, die auf
    /// Windows dieselbe Taste mit und ohne E0 sind (Alt/AltGr, Strg links/
    /// rechts, Enter/Ziffernblock-Enter, / und Ziffernblock-/, Ziffernblock
    /// gegen die Navigationstasten).
    #[test]
    fn scancodes_eindeutig() {
        let mut gesehen = std::collections::HashSet::new();
        for mac in 0..256u16 {
            if let Some(sc) = scancode(mac) {
                assert!(gesehen.insert(sc), "Scancode {:02X} E0 {} doppelt (macOS {mac})", sc.0, sc.1);
            }
        }
    }

    #[test]
    fn befehlstaste_ist_windows_taste() {
        assert_eq!(scancode(55), Some((0x5B, true)));
        assert_eq!(scancode(54), Some((0x5C, true)));
        assert_eq!(scancode(58), Some((0x38, false)));
        assert_eq!(scancode(61), Some((0x38, true)));
    }
}
