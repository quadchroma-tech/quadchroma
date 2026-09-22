// Zwischenablage auf der Mac-Seite des Clients.
//
// Dieselbe Schnittstelle wie clipboard.rs (Windows): watch() meldet, was der
// Benutzer hier kopiert hat, set() legt Text ab, ohne die eigene Ueberwachung
// auszuloesen. Nur Text, wie bisher.
//
// NSPasteboard schickt keine Nachricht bei Aenderungen; es gibt nur den
// changeCount. Deshalb fragt ein Faden alle 300 ms nach und liest, sobald die
// Zahl gestiegen ist. Nach dem eigenen Schreiben merken wir uns die neue Zahl
// (wie OWN_SEQ in clipboard.rs), damit das eigene set() nicht als Kopie des
// Benutzers zurueckgemeldet wird. Waechter und set() laufen unter EINER
// Sperre, damit der Waechter nie zwischen clearContents und setString liest.
//
// Alles ueber objc_msgSend von Hand, keine Kiste: objc_getClass,
// sel_registerName, objc_msgSend (mit passendem Funktionszeiger-Typ je
// Aufruf) und ein Autorelease-Pool je Durchlauf. Gelinkt wird AppKit (fuer
// NSPasteboard und die Konstante NSPasteboardTypeString) und libobjc.

use std::ffi::{c_char, c_void, CStr};
use std::sync::Mutex;
use std::time::Duration;

/// Obergrenze fuer einen Uebertragungsvorgang - wie auf Windows. Groesseres
/// wird stillschweigend verworfen statt abgeschnitten.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Abstand zwischen zwei Blicken auf den changeCount.
const TAKT: Duration = Duration::from_millis(300);

/// NSUTF8StringEncoding
const NS_UTF8: usize = 4;

// ------------------------------------------------------------------- FFI

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    static NSPasteboardTypeString: Id;
}

fn klasse(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

/// [obj sel] -> id
unsafe fn msg_id(obj: Id, s: Sel) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s)
}

/// [obj sel] -> NSInteger
unsafe fn msg_int(obj: Id, s: Sel) -> isize {
    let f: unsafe extern "C" fn(Id, Sel) -> isize = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s)
}

/// [obj sel:arg] -> id
unsafe fn msg_id_1(obj: Id, s: Sel, a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a)
}

/// [obj sel:arg] -> NSUInteger
unsafe fn msg_uint_1(obj: Id, s: Sel, a: usize) -> usize {
    let f: unsafe extern "C" fn(Id, Sel, usize) -> usize = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a)
}

/// [obj sel:a forType:b] -> BOOL
unsafe fn msg_bool_2(obj: Id, s: Sel, a: Id, b: Id) -> bool {
    let f: unsafe extern "C" fn(Id, Sel, Id, Id) -> u8 = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a, b) != 0
}

/// [NSString alloc] initWithBytes:length:encoding: -> id (eigene Zaehlung)
unsafe fn msg_init_bytes(obj: Id, s: Sel, bytes: *const c_void, len: usize, enc: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, *const c_void, usize, usize) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, bytes, len, enc)
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

// ------------------------------------------------------------ Zustand

/// changeCount nach dem eigenen Schreiben: einmal nach clearContents, einmal
/// nach setString - je nachdem, wann der Waechter hinschaut, muss einer
/// von beiden passen.
static EIGEN: Mutex<(isize, isize)> = Mutex::new((-1, -1));

/// Serialisiert set() gegen den Waechter (siehe Kopf).
static SPERRE: Mutex<()> = Mutex::new(());

/// Das allgemeine Brett. Null, wenn AppKit nicht da ist - dann tut alles
/// hier still nichts.
fn brett() -> Id {
    let k = klasse(c"NSPasteboard");
    if k.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { msg_id(k, sel(c"generalPasteboard")) }
}

fn change_count(b: Id) -> isize {
    unsafe { msg_int(b, sel(c"changeCount")) }
}

// ------------------------------------------------------------------- Lesen

/// Text vom Brett. None, wenn nichts Textartiges anliegt oder der Inhalt
/// die Obergrenze reisst. Laeuft im eigenen Pool: die NSString-Rueckgaben
/// sind autoreleased.
fn lesen(b: Id) -> Option<String> {
    let _pool = Pool::neu();
    unsafe {
        let s = msg_id_1(b, sel(c"stringForType:"), NSPasteboardTypeString);
        if s.is_null() {
            return None;
        }
        let len = msg_uint_1(s, sel(c"lengthOfBytesUsingEncoding:"), NS_UTF8);
        if len == 0 || len > MAX_BYTES {
            return None;
        }
        let p = msg_id(s, sel(c"UTF8String")) as *const u8;
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p, len);
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

// ----------------------------------------------------------------- Schreiben

/// Schreibt Text auf das Brett, ohne die eigene Ueberwachung auszuloesen.
/// Scheitert still: ein verlorener Kopiervorgang ist hinnehmbar, ein Absturz
/// des Clients waehrend einer laufenden Sitzung nicht.
pub fn set(text: &str) {
    if text.len() > MAX_BYTES {
        return;
    }
    let _sperre = SPERRE.lock().unwrap_or_else(|e| e.into_inner());
    let b = brett();
    if b.is_null() {
        return;
    }
    let _pool = Pool::neu();
    unsafe {
        let ns_klasse = klasse(c"NSString");
        if ns_klasse.is_null() {
            return;
        }
        // initWithBytes statt stringWithUTF8String: kein Abschneiden an
        // einem NUL im Text, und die Laenge steht fest.
        let roh = msg_id(ns_klasse, sel(c"alloc"));
        let s = msg_init_bytes(
            roh,
            sel(c"initWithBytes:length:encoding:"),
            text.as_ptr() as *const c_void,
            text.len(),
            NS_UTF8,
        );
        if s.is_null() {
            return;
        }
        let nach_leeren = msg_int(b, sel(c"clearContents"));
        let ok = msg_bool_2(b, sel(c"setString:forType:"), s, NSPasteboardTypeString);
        let _ = msg_id(s, sel(c"release"));
        let nach_setzen = if ok { change_count(b) } else { nach_leeren };
        if let Ok(mut e) = EIGEN.lock() {
            *e = (nach_leeren, nach_setzen);
        }
    }
}

/// Stammt dieser changeCount von unserem eigenen set()?
fn eigener_vorgang(count: isize) -> bool {
    EIGEN
        .lock()
        .map(|e| count == e.0 || count == e.1)
        .unwrap_or(false)
}

// -------------------------------------------------------------- Ueberwachung

/// Startet die Ueberwachung in einem eigenen Faden. `cb` bekommt jeden Text,
/// den der Benutzer auf dem Mac kopiert hat; eigene Schreibvorgaenge aus `set`
/// sind bereits herausgefiltert. Was beim Start schon auf dem Brett liegt,
/// wird nicht gemeldet - wie auf Windows, wo erst die naechste Aenderung zaehlt.
pub fn watch(cb: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        let b = brett();
        if b.is_null() {
            eprintln!("Zwischenablage: Ueberwachung nicht gestartet: NSPasteboard fehlt");
            return;
        }
        let mut zuletzt = change_count(b);
        loop {
            std::thread::sleep(TAKT);
            let text = {
                let _sperre = SPERRE.lock().unwrap_or_else(|e| e.into_inner());
                let jetzt = change_count(b);
                if jetzt == zuletzt {
                    continue;
                }
                zuletzt = jetzt;
                if eigener_vorgang(jetzt) {
                    continue; // unser eigener Schreibvorgang, nicht zuruecksenden
                }
                lesen(b)
            };
            if let Some(t) = text {
                if !t.is_empty() {
                    cb(t);
                }
            }
        }
    });
}

// --------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Setzen laesst den changeCount steigen, Lesen liefert denselben Text,
    /// und der neue Zaehler gilt als eigener Vorgang. Der vorherige Inhalt
    /// des Bretts wird danach zurueckgelegt.
    #[test]
    fn setzen_zaehlen_lesen() {
        let b = brett();
        assert!(!b.is_null(), "NSPasteboard fehlt");
        let vorher = lesen(b);
        let c0 = change_count(b);

        let probe = "QuadChroma Probe: Zwischenablage \u{e4}\u{f6}\u{fc} \u{1F600}\nzweite Zeile";
        set(probe);
        let c1 = change_count(b);
        assert!(c1 > c0, "changeCount steigt nicht: {c0} -> {c1}");
        assert!(eigener_vorgang(c1), "eigener Vorgang nicht erkannt");
        assert_eq!(lesen(b).as_deref(), Some(probe));

        // Zweites Setzen: wieder ein Schritt weiter, alte Zahl gilt nicht mehr.
        set("zweite Probe");
        let c2 = change_count(b);
        assert!(c2 > c1);
        assert!(eigener_vorgang(c2));
        assert!(!eigener_vorgang(c1));
        assert_eq!(lesen(b).as_deref(), Some("zweite Probe"));

        if let Some(v) = vorher {
            set(&v);
        }
    }
}
