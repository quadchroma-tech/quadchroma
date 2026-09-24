// Symbol in der Menueleiste (macOS), Spezifikation 3.9.
//
// NSStatusBar systemStatusBar -> statusItemWithLength: mit dem Logo als
// einfarbigem Vorlagenbild (logo::vorlage, 18 pt, setTemplate:YES; geht das
// nicht, der Titel "QC") und einem NSMenu mit denselben Punkten wie unter
// Windows (tray::menue). Ein Klick auf das Symbol oeffnet das Menue - so ist
// es auf dem Mac ueblich; "Oeffnen" zeigt das Fenster.
//
// Die Menuepunkte zielen auf ein Objekt einer zur Laufzeit angelegten Klasse
// (objc_allocateClassPair/class_addMethod) mit drei Methoden, die hier als
// extern "C" stehen. Sie rufen den Rueckruf `befehl`, den main.rs mit einem
// EventLoopProxy belegt - daraus werden Benutzerereignisse fuer winit.
//
// Alles laeuft auf dem Hauptfaden, nach `resumed` (AppKit verlangt das), und
// alles ueber objc_msgSend von Hand wie in clipboard_mac.rs: keine Kiste.
//
// Die einmalige Hinweisblase (TrayStillRunningMac) ist ein NSPopover unter
// dem Symbol, das nach sechs Sekunden wieder zugeht. Eine Mitteilung ueber
// die Mitteilungszentrale ginge nur mit Rueckfrage des Systems.
//
// Die Aktivierungsart (`aktivierung`): verborgen Accessory (kein
// Dock-Symbol, nicht im Programmumschalter), sichtbar Regular.

use crate::tray::{self, Befehl, Punkt, Stand};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::time::{Duration, Instant};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> u8;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

// NSStatusBar, NSMenu, NSPopover und NSApplication kommen aus AppKit.
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

/// NSVariableStatusItemLength: so breit wie Bild bzw. Titel.
const LAENGE_VARIABEL: f64 = -1.0;
/// Kantenlaenge des Symbols in Punkten; das Bild hat die doppelte Aufloesung.
const PUNKTE: f64 = 18.0;
const BILDPUNKTE: u32 = 36;
/// NSApplicationActivationPolicyRegular / Accessory.
const REGULAR: isize = 0;
const ACCESSORY: isize = 1;
/// NSPopoverBehaviorTransient: geht beim Klick daneben zu.
const TRANSIENT: isize = 1;
/// NSRectEdgeMinY: unter dem Symbol.
const KANTE_UNTEN: usize = 1;
/// So lange steht die Hinweisblase.
const HINWEIS_DAUER: Duration = Duration::from_secs(6);
/// Name der Laufzeitklasse fuer die Menueziele.
const KLASSE: &CStr = c"QCMenueleisteZiel";

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    w: f64,
    h: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

// ------------------------------------------------------------------- FFI

fn klasse(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Ein objc_msgSend mit fester Signatur. Jede Form bekommt ihren eigenen
/// Funktionszeigertyp; so werden Gleitkommazahlen und Strukturen nach der
/// Aufrufkonvention der Plattform uebergeben.
macro_rules! senden {
    ($obj:expr, $sel:expr $(, $arg:expr => $typ:ty)* ; -> $ret:ty) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $typ)*) -> $ret = std::mem::transmute(objc_msgSend as *const c_void);
        f($obj, $sel $(, $arg)*)
    }};
}

unsafe fn msg_id(obj: Id, s: &CStr) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    senden!(obj, sel(s); -> Id)
}

unsafe fn msg_id_1(obj: Id, s: &CStr, a: Id) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    senden!(obj, sel(s), a => Id; -> Id)
}

unsafe fn msg_void_1(obj: Id, s: &CStr, a: Id) {
    if !obj.is_null() {
        senden!(obj, sel(s), a => Id; -> ())
    }
}

unsafe fn msg_void_bool(obj: Id, s: &CStr, b: bool) {
    if !obj.is_null() {
        senden!(obj, sel(s), b as u8 => u8; -> ())
    }
}

unsafe fn msg_void_int(obj: Id, s: &CStr, n: isize) {
    if !obj.is_null() {
        senden!(obj, sel(s), n => isize; -> ())
    }
}

unsafe fn msg_void_size(obj: Id, s: &CStr, g: NSSize) {
    if !obj.is_null() {
        senden!(obj, sel(s), g => NSSize; -> ())
    }
}

unsafe fn msg_int(obj: Id, s: &CStr) -> isize {
    if obj.is_null() {
        return 0;
    }
    senden!(obj, sel(s); -> isize)
}

unsafe fn msg_bool(obj: Id, s: &CStr) -> bool {
    if obj.is_null() {
        return false;
    }
    senden!(obj, sel(s); -> u8) != 0
}

unsafe fn msg_bool_int(obj: Id, s: &CStr, n: isize) -> bool {
    if obj.is_null() {
        return false;
    }
    senden!(obj, sel(s), n => isize; -> u8) != 0
}

/// NSString aus Rust-Text (autoreleased: braucht einen Pool).
unsafe fn ns_text(t: &str) -> Id {
    let c = CString::new(t.replace('\0', " ")).unwrap_or_default();
    msg_id_1(klasse(c"NSString"), c"stringWithUTF8String:", c.as_ptr() as Id)
}

/// NSString zurueck nach Rust (fuer den Selbsttest).
unsafe fn rust_text(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = senden!(s, sel(c"UTF8String"); -> *const c_char);
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

/// Ein Autorelease-Pool fuer die Dauer eines Geltungsbereichs.
struct Pool(*mut c_void);

impl Pool {
    fn neu() -> Pool {
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

// ------------------------------------------------------------ Laufzeitklasse

/// Was die Menueziele brauchen. Nur der Hauptfaden fasst es an.
struct Zustand {
    befehl: Box<dyn Fn(Befehl) + Send>,
    /// Adresse je Menuepunkt "Verbinden", nach der Nummer (tag) des Punktes.
    adressen: Vec<String>,
}

thread_local! {
    static ZUSTAND: RefCell<Option<Zustand>> = const { RefCell::new(None) };
}

/// Den Rueckruf rufen. Kein Panic ueber die Grenze zu Objective-C: ist der
/// Zustand gerade ausgeliehen oder weg, geschieht nichts.
fn weitergeben(b: Befehl) {
    ZUSTAND.with(|z| {
        if let Ok(z) = z.try_borrow() {
            if let Some(z) = z.as_ref() {
                (z.befehl)(b);
            }
        }
    });
}

extern "C" fn ziel_oeffnen(_this: Id, _cmd: Sel, _sender: Id) {
    weitergeben(Befehl::Oeffnen);
}

extern "C" fn ziel_verbinden(_this: Id, _cmd: Sel, sender: Id) {
    let tag = unsafe { msg_int(sender, c"tag") };
    let adresse = ZUSTAND.with(|z| {
        z.try_borrow().ok().and_then(|z| z.as_ref().and_then(|z| usize::try_from(tag).ok().and_then(|i| z.adressen.get(i).cloned())))
    });
    if let Some(a) = adresse {
        weitergeben(Befehl::Verbinden(a));
    }
}

extern "C" fn ziel_beenden(_this: Id, _cmd: Sel, _sender: Id) {
    weitergeben(Befehl::Beenden);
}

/// Die Klasse der Menueziele: einmal je Prozess angelegt, danach gefunden.
fn ziel_klasse() -> Id {
    let da = klasse(KLASSE);
    if !da.is_null() {
        return da;
    }
    unsafe {
        let k = objc_allocateClassPair(klasse(c"NSObject"), KLASSE.as_ptr(), 0);
        if k.is_null() {
            return std::ptr::null_mut();
        }
        // "v@:@": void, self, _cmd, sender
        let typ = c"v@:@".as_ptr();
        type Methode = extern "C" fn(Id, Sel, Id);
        class_addMethod(k, sel(c"qcOeffnen:"), ziel_oeffnen as Methode as *const c_void, typ);
        class_addMethod(k, sel(c"qcVerbinden:"), ziel_verbinden as Methode as *const c_void, typ);
        class_addMethod(k, sel(c"qcBeenden:"), ziel_beenden as Methode as *const c_void, typ);
        objc_registerClassPair(k);
        k
    }
}

// ------------------------------------------------------------------ Symbol

/// Das Symbol in der Menueleiste. Faellt es weg, wird es entfernt.
pub struct Symbol {
    item: Id,
    menue: Id,
    ziel: Id,
    popover: Id,
    popover_seit: Option<Instant>,
    zuletzt: Stand,
}

/// Das Logo als Vorlagenbild (NSImage, autoreleased), oder null.
unsafe fn vorlagenbild() -> Id {
    let png = crate::logo::png(BILDPUNKTE, BILDPUNKTE, &crate::logo::vorlage(BILDPUNKTE));
    let k = klasse(c"NSData");
    if k.is_null() {
        return std::ptr::null_mut();
    }
    let daten = senden!(k, sel(c"dataWithBytes:length:"), png.as_ptr() as *const c_void => *const c_void, png.len() => usize; -> Id);
    let bild = msg_id_1(msg_id(klasse(c"NSImage"), c"alloc"), c"initWithData:", daten);
    if bild.is_null() {
        return std::ptr::null_mut();
    }
    msg_void_size(bild, c"setSize:", NSSize { w: PUNKTE, h: PUNKTE });
    msg_void_bool(bild, c"setTemplate:", true);
    msg_id(bild, c"autorelease")
}

impl Symbol {
    /// Statusobjekt, Menue und Ziel anlegen. Nur auf dem Hauptfaden.
    pub fn neu(befehl: Box<dyn Fn(Befehl) + Send>, stand: &Stand) -> Result<Symbol, String> {
        let _pool = Pool::neu();
        unsafe {
            let leiste = msg_id(klasse(c"NSStatusBar"), c"systemStatusBar");
            if leiste.is_null() {
                return Err("NSStatusBar nicht verfuegbar".into());
            }
            let item = senden!(leiste, sel(c"statusItemWithLength:"), LAENGE_VARIABEL => f64; -> Id);
            if item.is_null() {
                return Err("statusItemWithLength: lieferte nichts".into());
            }
            // Die Leiste haelt das Objekt nicht selbst fest.
            msg_id(item, c"retain");
            let knopf = msg_id(item, c"button");
            if knopf.is_null() {
                msg_void_1(leiste, c"removeStatusItem:", item);
                msg_id(item, c"release");
                return Err("Statusobjekt ohne Knopf".into());
            }
            let bild = vorlagenbild();
            if bild.is_null() {
                msg_void_1(knopf, c"setTitle:", ns_text("QC"));
            } else {
                msg_void_1(knopf, c"setImage:", bild);
            }
            let k = ziel_klasse();
            let ziel = msg_id(msg_id(k, c"alloc"), c"init");
            let menue = msg_id_1(msg_id(klasse(c"NSMenu"), c"alloc"), c"initWithTitle:", ns_text("QuadChroma"));
            if ziel.is_null() || menue.is_null() {
                msg_void_1(leiste, c"removeStatusItem:", item);
                msg_id(item, c"release");
                msg_id(ziel, c"release");
                msg_id(menue, c"release");
                return Err("Menue oder Menueziel nicht anlegbar".into());
            }
            msg_void_bool(menue, c"setAutoenablesItems:", false);
            msg_void_1(item, c"setMenu:", menue);
            ZUSTAND.with(|z| *z.borrow_mut() = Some(Zustand { befehl, adressen: Vec::new() }));
            let mut s = Symbol { item, menue, ziel, popover: std::ptr::null_mut(), popover_seit: None, zuletzt: stand.clone() };
            s.anwenden(stand);
            Ok(s)
        }
    }

    /// Steht das Symbol? Auf dem Mac, sobald es angelegt ist.
    pub fn steht(&self) -> bool {
        !self.item.is_null()
    }

    pub fn grund(&self) -> Option<String> {
        None
    }

    /// Menue und Tooltip erneuern, wenn sich etwas geaendert hat.
    pub fn stand_setzen(&mut self, stand: &Stand) {
        if *stand != self.zuletzt {
            let _pool = Pool::neu();
            self.anwenden(stand);
            self.zuletzt = stand.clone();
        }
    }

    fn anwenden(&mut self, stand: &Stand) {
        let mut adressen = Vec::new();
        unsafe {
            senden!(self.menue, sel(c"removeAllItems"); -> ());
            for p in &stand.menue {
                let (text, aktion, tag) = match p {
                    Punkt::Trenner => {
                        let t = msg_id(klasse(c"NSMenuItem"), c"separatorItem");
                        msg_void_1(self.menue, c"addItem:", t);
                        continue;
                    }
                    Punkt::Oeffnen(t) => (t.as_str(), c"qcOeffnen:", 0),
                    Punkt::Verbinden { text, adresse } => {
                        adressen.push(adresse.clone());
                        (text.as_str(), c"qcVerbinden:", adressen.len() as isize - 1)
                    }
                    Punkt::Beenden(t) => (t.as_str(), c"qcBeenden:", 0),
                };
                let roh = msg_id(klasse(c"NSMenuItem"), c"alloc");
                if roh.is_null() {
                    continue;
                }
                let eintrag = senden!(roh, sel(c"initWithTitle:action:keyEquivalent:"),
                    ns_text(text) => Id, sel(aktion) => Sel, ns_text("") => Id; -> Id);
                msg_void_1(eintrag, c"setTarget:", self.ziel);
                msg_void_int(eintrag, c"setTag:", tag);
                msg_void_1(self.menue, c"addItem:", eintrag);
                msg_id(eintrag, c"release");
            }
            let knopf = msg_id(self.item, c"button");
            msg_void_1(knopf, c"setToolTip:", ns_text(&stand.tooltip));
        }
        ZUSTAND.with(|z| {
            if let Ok(mut z) = z.try_borrow_mut() {
                if let Some(z) = z.as_mut() {
                    z.adressen = adressen;
                }
            }
        });
    }

    /// Die Hinweisblase unter dem Symbol (NSPopover), sechs Sekunden lang.
    /// Steht das Symbol nicht auf dem Bildschirm (etwa hinter der Kamera-
    /// aussparung verdraengt), bleibt sie aus.
    pub fn hinweis(&mut self, titel: &str, text: &str) {
        let _ = titel; // Der Text nennt QuadChroma schon.
        let _pool = Pool::neu();
        self.hinweis_schliessen();
        unsafe {
            let knopf = msg_id(self.item, c"button");
            if knopf.is_null() || msg_id(knopf, c"window").is_null() {
                return;
            }
            let pop = msg_id(msg_id(klasse(c"NSPopover"), c"alloc"), c"init");
            if pop.is_null() {
                return;
            }
            msg_void_int(pop, c"setBehavior:", TRANSIENT);
            let zeile = msg_id_1(klasse(c"NSTextField"), c"labelWithString:", ns_text(text));
            let vc = msg_id(msg_id(klasse(c"NSViewController"), c"alloc"), c"init");
            if zeile.is_null() || vc.is_null() {
                msg_id(vc, c"release");
                msg_id(pop, c"release");
                return;
            }
            let g = senden!(zeile, sel(c"fittingSize"); -> NSSize);
            let rahmen = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { w: g.w + 28.0, h: g.h + 20.0 } };
            let ansicht = senden!(msg_id(klasse(c"NSView"), c"alloc"), sel(c"initWithFrame:"), rahmen => NSRect; -> Id);
            if ansicht.is_null() {
                msg_id(vc, c"release");
                msg_id(pop, c"release");
                return;
            }
            senden!(zeile, sel(c"setFrameOrigin:"), NSPoint { x: 14.0, y: 10.0 } => NSPoint; -> ());
            msg_void_1(ansicht, c"addSubview:", zeile);
            msg_void_1(vc, c"setView:", ansicht);
            msg_void_1(pop, c"setContentViewController:", vc);
            msg_void_size(pop, c"setContentSize:", rahmen.size);
            // Leeres Rechteck heisst: die ganze Flaeche des Knopfes.
            let leer = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { w: 0.0, h: 0.0 } };
            senden!(pop, sel(c"showRelativeToRect:ofView:preferredEdge:"), leer => NSRect, knopf => Id, KANTE_UNTEN => usize; -> ());
            msg_id(ansicht, c"release");
            msg_id(vc, c"release");
            self.popover = pop;
            self.popover_seit = Some(Instant::now());
        }
    }

    fn hinweis_schliessen(&mut self) {
        if !self.popover.is_null() {
            unsafe {
                msg_void_1(self.popover, c"performClose:", std::ptr::null_mut());
                msg_id(self.popover, c"release");
            }
            self.popover = std::ptr::null_mut();
        }
        self.popover_seit = None;
    }

    /// Aus der Ereignisschleife: die Hinweisblase nach sechs Sekunden zu.
    pub fn takt(&mut self) {
        if self.popover_seit.map(|t| t.elapsed() >= HINWEIS_DAUER).unwrap_or(false) {
            let _pool = Pool::neu();
            self.hinweis_schliessen();
        }
    }
}

impl Drop for Symbol {
    fn drop(&mut self) {
        let _pool = Pool::neu();
        self.hinweis_schliessen();
        unsafe {
            let leiste = msg_id(klasse(c"NSStatusBar"), c"systemStatusBar");
            msg_void_1(self.item, c"setMenu:", std::ptr::null_mut());
            msg_void_1(leiste, c"removeStatusItem:", self.item);
            msg_id(self.item, c"release");
            msg_id(self.menue, c"release");
            msg_id(self.ziel, c"release");
        }
        self.item = std::ptr::null_mut();
        ZUSTAND.with(|z| {
            if let Ok(mut z) = z.try_borrow_mut() {
                *z = None;
            }
        });
    }
}

/// Aktivierungsart des Programms: sichtbar Regular (Dock-Symbol, Menueleiste
/// des Programms), verborgen Accessory (nur das Symbol in der Menueleiste).
pub fn aktivierung(sichtbar: bool) {
    let _pool = Pool::neu();
    unsafe {
        let app = msg_id(klasse(c"NSApplication"), c"sharedApplication");
        msg_bool_int(app, c"setActivationPolicy:", if sichtbar { REGULAR } else { ACCESSORY });
    }
}

/// Die RunLoop kurz laufen lassen (Selbsttest): so zeichnet AppKit das
/// Symbol wirklich.
unsafe fn laufen(sekunden: f64) {
    let bis = senden!(klasse(c"NSDate"), sel(c"dateWithTimeIntervalSinceNow:"), sekunden => f64; -> Id);
    let schleife = msg_id(klasse(c"NSRunLoop"), c"currentRunLoop");
    msg_void_1(schleife, c"runUntilDate:", bis);
}

/// --menueleiste-selbsttest: Statusobjekt anlegen, pruefen (Bild, Menue,
/// Ziele, Tooltip, Aenderung, Hinweisblase), entfernen, Ende. Das Symbol
/// steht weniger als eine Sekunde da. Keine Aufnahme, keine Ablage, kein
/// Netz - also keine Rueckfrage des Systems. Rueckgabe 0 = bestanden.
pub fn selbsttest() -> i32 {
    let _pool = Pool::neu();
    let start = Instant::now();
    unsafe {
        let app = msg_id(klasse(c"NSApplication"), c"sharedApplication");
        if app.is_null() {
            eprintln!("Menueleisten-Selbsttest: keine NSApplication");
            return 1;
        }
        // Accessory: kein Dock-Symbol fuer den kurzen Lauf.
        msg_bool_int(app, c"setActivationPolicy:", ACCESSORY);
    }
    let lang = crate::strings::pick("de");
    let host = crate::discovery::Host { name: "Selbsttest".into(), addr: "127.0.0.1:9".parse().unwrap(), seen: Instant::now() };
    let stand = tray::stand(lang, std::slice::from_ref(&host), None);
    let erhalten = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Befehl>::new()));
    let e2 = erhalten.clone();
    let mut s = match Symbol::neu(Box::new(move |b| e2.lock().unwrap().push(b)), &stand) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Menueleisten-Selbsttest: Statusobjekt nicht angelegt ({e})");
            return 1;
        }
    };
    let mut fehler = Vec::<String>::new();
    unsafe {
        let knopf = msg_id(s.item, c"button");
        let bild = msg_id(knopf, c"image");
        if bild.is_null() {
            if rust_text(msg_id(knopf, c"title")) != "QC" {
                fehler.push("weder Bild noch Titel".into());
            } else {
                println!("Menueleisten-Selbsttest: Titel \"QC\" statt Bild");
            }
        } else if !msg_bool(bild, c"isTemplate") {
            fehler.push("Bild ist kein Vorlagenbild".into());
        }
        let n = msg_int(s.menue, c"numberOfItems");
        if n != stand.menue.len() as isize {
            fehler.push(format!("Menue hat {n} statt {} Punkte", stand.menue.len()));
        }
        let titel: Vec<String> = (0..n)
            .map(|i| rust_text(msg_id(senden!(s.menue, sel(c"itemAtIndex:"), i => isize; -> Id), c"title")))
            .collect();
        if titel.first().map(String::as_str) != Some("Öffnen") || titel.get(2).map(String::as_str) != Some("Verbinden: Selbsttest") {
            fehler.push(format!("Menuetexte: {titel:?}"));
        }
        if rust_text(msg_id(knopf, c"toolTip")) != "QuadChroma" {
            fehler.push("Tooltip".into());
        }
        // Die Laufzeitklasse: Ziele antworten, Nummern fuehren zur Adresse.
        // Jeder waehlbare Punkt muss genau den Befehl liefern, den das
        // Menuemodell fuer ihn vorsieht (Oeffnen, Verbinden mit
        // 127.0.0.1:9, Beenden).
        for (i, p) in stand.menue.iter().enumerate() {
            let Some(soll) = p.befehl() else { continue };
            senden!(s.menue, sel(c"performActionForItemAtIndex:"), i as isize => isize; -> ());
            let ist = erhalten.lock().unwrap().pop();
            if ist.as_ref() != Some(&soll) {
                fehler.push(format!("Punkt {i}: {ist:?} statt {soll:?}"));
            }
        }
        if !stand.menue.iter().any(|p| p.befehl() == Some(Befehl::Verbinden("127.0.0.1:9".into()))) {
            fehler.push("kein Punkt fuer den Host".into());
        }
        // Kurz laufen lassen, damit AppKit das Symbol zeichnet.
        laufen(0.3);
        let fenster = msg_id(knopf, c"window");
        let sichtbar = !fenster.is_null() && msg_bool(fenster, c"isVisible");
        println!(
            "Menueleisten-Selbsttest: Statusobjekt angelegt, {}",
            if sichtbar { "sichtbar in der Menueleiste" } else { "vom System nicht angezeigt (Menueleiste voll?)" }
        );
    }
    // Aendern: Stand einer Sitzung ohne gefundene Hosts.
    let neu = tray::stand(lang, &[], Some("Selbsttest"));
    s.stand_setzen(&neu);
    unsafe {
        let n = msg_int(s.menue, c"numberOfItems");
        if n != neu.menue.len() as isize {
            fehler.push(format!("nach dem Aendern {n} statt {} Punkte", neu.menue.len()));
        }
        if rust_text(msg_id(msg_id(s.item, c"button"), c"toolTip")) != "QuadChroma – Selbsttest" {
            fehler.push("Tooltip nach dem Aendern".into());
        }
    }
    // Die Hinweisblase einmal kurz.
    s.hinweis("QuadChroma", lang.get(tray::HINWEIS));
    if !s.popover.is_null() {
        unsafe {
            laufen(0.2);
            println!(
                "Menueleisten-Selbsttest: Hinweisblase {}",
                if msg_bool(s.popover, c"isShown") { "gezeigt" } else { "nicht gezeigt" }
            );
        }
    } else {
        println!("Menueleisten-Selbsttest: Hinweisblase ausgelassen (Symbol ohne Fenster)");
    }
    drop(s);
    let dauer = start.elapsed();
    if !fehler.is_empty() {
        for f in &fehler {
            eprintln!("Menueleisten-Selbsttest: FEHLER {f}");
        }
        return 1;
    }
    println!("Menueleisten-Selbsttest: Statusobjekt entfernt - bestanden ({} ms)", dauer.as_millis());
    0
}
