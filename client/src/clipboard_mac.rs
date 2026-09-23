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
// Gelesen wird nach derselben Regel wie auf dem Mac-Host (read_first_text in
// host/clipboard.m): nur der erste Eintrag, und verdeckte Eintraege
// (Passwoerter) sowie Dateien bleiben auf diesem Rechner.
//
// Gelesen wird nur waehrend einer Sitzung (`sitzung`). Ohne sie zaehlt der
// Waechter nur den changeCount mit: ab macOS 15.4 kann jedes Lesen des
// Inhalts durch ein Programm die Rueckfrage des Systems ausloesen (siehe
// host/clipboard.m), und was ohne Sitzung kopiert wurde, braucht niemand -
// es geht auch beim Sitzungsbeginn nicht nachtraeglich hinaus. Das ist die
// Regel des Windows-Clients: dort zaehlt ebenfalls nur, was waehrend einer
// Sitzung kopiert wird (vorher faellt es mangels Eingabekanal weg).
//
// Was vom Host kommt, traegt org.nspasteboard.TransientType: Verwalter der
// Zwischenablage nehmen es dann nicht in ihren Verlauf auf - wie
// CanIncludeInClipboardHistory = 0 unter Windows (clipboard.rs). Der eigene
// Lesefilter bleibt davon unberuehrt: fluechtig heisst nicht verdeckt.
//
// Alles ueber objc_msgSend von Hand, keine Kiste: objc_getClass,
// sel_registerName, objc_msgSend (mit passendem Funktionszeiger-Typ je
// Aufruf) und ein Autorelease-Pool je Durchlauf. Gelinkt wird AppKit (fuer
// NSPasteboard und die Konstanten NSPasteboardTypeString und
// NSPasteboardTypeFileURL) und libobjc.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Obergrenze fuer einen Uebertragungsvorgang - wie auf Windows. Groesseres
/// wird stillschweigend verworfen statt abgeschnitten.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Abstand zwischen zwei Blicken auf den changeCount.
const TAKT: Duration = Duration::from_millis(300);

/// NSUTF8StringEncoding
const NS_UTF8: usize = 4;

/// Kennzeichen aus der Verabredung unter nspasteboard.org: Passwortverwalter
/// markieren damit Inhalte, die nicht mitgeschrieben werden sollen. Ein
/// solcher Eintrag geht nicht ueber die Leitung - wie auf dem Mac-Host.
/// UNGEPRUEFT: Branchenkonvention, keine Apple-Dokumentation.
const VERDECKT: &CStr = c"org.nspasteboard.ConcealedType";

/// Kennzeichen aus derselben Verabredung: der Inhalt ist fluechtig und
/// gehoert in keinen Verlauf. Setzt set() an allem, was vom Host kommt.
/// UNGEPRUEFT wie VERDECKT: Branchenkonvention.
const FLUECHTIG: &CStr = c"org.nspasteboard.TransientType";

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
    static NSPasteboardTypeFileURL: Id;
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

/// NSString aus einer C-Zeichenkette. Autoreleased: braucht einen Pool.
unsafe fn ns_text(s: &CStr) -> Id {
    let k = klasse(c"NSString");
    if k.is_null() {
        return std::ptr::null_mut();
    }
    msg_id_1(k, sel(c"stringWithUTF8String:"), s.as_ptr() as Id)
}

/// Traegt der Eintrag diesen Typ? [item availableTypeFromArray:@[typ]]
unsafe fn hat_typ(item: Id, typ: Id) -> bool {
    let k = klasse(c"NSArray");
    if k.is_null() || typ.is_null() {
        return false;
    }
    let liste = msg_id_1(k, sel(c"arrayWithObject:"), typ);
    !liste.is_null() && !msg_id_1(item, sel(c"availableTypeFromArray:"), liste).is_null()
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

/// Laeuft eine Sitzung? Nur dann liest der Waechter den Inhalt.
static SITZUNG: AtomicBool = AtomicBool::new(false);

/// Sitzung beginnt (true, der Host hat angenommen) oder endet (false).
pub fn sitzung(an: bool) {
    SITZUNG.store(an, Ordering::Relaxed);
}

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

/// Text des ERSTEN Eintrags auf dem Brett. Bewusst ueber pasteboardItems
/// und nicht ueber [b stringForType:]: der bequeme Weg auf Brettebene haengt
/// bei mehreren Eintraegen deren Text aneinander. None, wenn der Eintrag
/// verdeckt ist, eine Datei ist, nichts Textartiges traegt oder die
/// Obergrenze reisst. Laeuft im eigenen Pool: die Rueckgaben sind
/// autoreleased.
fn lesen(b: Id) -> Option<String> {
    let _pool = Pool::neu();
    unsafe {
        let eintraege = msg_id(b, sel(c"pasteboardItems"));
        if eintraege.is_null() {
            return None;
        }
        let item = msg_id(eintraege, sel(c"firstObject"));
        if item.is_null() {
            return None;
        }
        let anzahl = msg_int(eintraege, sel(c"count"));
        if anzahl > 1 {
            crate::protokoll::zeile(format!("Zwischenablage: {anzahl} Eintraege, nur der erste wird uebertragen"));
        }
        // Verdeckte Inhalte (Passwoerter) bleiben auf diesem Rechner. Laesst
        // sich das Kennzeichen nicht einmal anlegen, wird nichts gelesen -
        // im Zweifel lieber keine Kopie als ein Passwort auf dem Host.
        let verdeckt = ns_text(VERDECKT);
        if verdeckt.is_null() {
            return None;
        }
        if hat_typ(item, verdeckt) {
            crate::protokoll::zeile("Zwischenablage: Eintrag ist als verdeckt markiert, nicht uebertragen".into());
            return None;
        }
        // Dateien und Ordner uebertragen wir nicht - nur der Name oder Pfad
        // waere sinnlos, weil er auf der anderen Seite nichts bedeutet.
        if hat_typ(item, NSPasteboardTypeFileURL) {
            return None;
        }
        let s = msg_id_1(item, sel(c"stringForType:"), NSPasteboardTypeString);
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
    if let Some(zaehler) = ablegen(b, text) {
        if let Ok(mut e) = EIGEN.lock() {
            *e = zaehler;
        }
    }
}

/// Legt `text` als einzigen Eintrag auf `b`, als fluechtig gekennzeichnet
/// (FLUECHTIG). Gibt den changeCount nach dem Leeren und nach dem letzten
/// Schreiben zurueck - beide muss der Waechter als eigenen Vorgang erkennen.
fn ablegen(b: Id, text: &str) -> Option<(isize, isize)> {
    let _pool = Pool::neu();
    unsafe {
        let ns_klasse = klasse(c"NSString");
        if ns_klasse.is_null() {
            return None;
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
            return None;
        }
        let nach_leeren = msg_int(b, sel(c"clearContents"));
        let ok = msg_bool_2(b, sel(c"setString:forType:"), s, NSPasteboardTypeString);
        let _ = msg_id(s, sel(c"release"));
        // Das Kennzeichen an denselben (ersten) Eintrag, mit leerem Wert -
        // gezaehlt wird der Stand danach, damit auch dieser Schritt als
        // eigener Vorgang gilt.
        let fluechtig = ns_text(FLUECHTIG);
        let leer = ns_text(c"");
        if ok && !fluechtig.is_null() && !leer.is_null() {
            let _ = msg_bool_2(b, sel(c"setString:forType:"), leer, fluechtig);
        }
        let nach_setzen = if ok { change_count(b) } else { nach_leeren };
        Some((nach_leeren, nach_setzen))
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

/// Ein Blick des Waechters auf `b`: hat sich der changeCount seit `zuletzt`
/// bewegt, und stammt die Aenderung nicht von uns, wird gelesen - aber nur
/// mit laufender Sitzung. Ohne sie wird nur mitgezaehlt; eine Aenderung von
/// vorher holt auch der Sitzungsbeginn nicht nach.
fn nachsehen(b: Id, zuletzt: &mut isize, sitzung: bool, lesen: impl FnOnce(Id) -> Option<String>) -> Option<String> {
    let jetzt = change_count(b);
    if jetzt == *zuletzt {
        return None;
    }
    *zuletzt = jetzt;
    if eigener_vorgang(jetzt) || !sitzung {
        return None; // unser eigener Schreibvorgang, oder niemand zum Senden
    }
    lesen(b)
}

/// Startet die Ueberwachung in einem eigenen Faden. `cb` bekommt jeden Text,
/// den der Benutzer auf dem Mac waehrend einer Sitzung kopiert hat; eigene
/// Schreibvorgaenge aus `set` sind bereits herausgefiltert. Was beim Start
/// oder ohne Sitzung auf das Brett kam, wird nicht gemeldet - wie auf
/// Windows, wo erst die naechste Aenderung in einer Sitzung zaehlt.
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
                nachsehen(b, &mut zuletzt, SITZUNG.load(Ordering::Relaxed), lesen)
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

    /// Dieselbe Leseregel wie auf dem Mac-Host: ein verdeckter Eintrag geht
    /// nicht hinaus, eine Datei auch nicht, und von mehreren Eintraegen
    /// zaehlt nur der erste. Geschrieben wird auf ein eigenes, namenloses
    /// Brett - die allgemeine Zwischenablage (und damit ein laufender Host)
    /// bekommt davon nichts mit.
    #[test]
    fn verdeckt_bleibt_hier() {
        let _pool = Pool::neu();
        unsafe {
            let b = msg_id(klasse(c"NSPasteboard"), sel(c"pasteboardWithUniqueName"));
            assert!(!b.is_null(), "kein eigenes Brett");
            let text_typ = NSPasteboardTypeString;
            let verdeckt = ns_text(VERDECKT);
            let datei = NSPasteboardTypeFileURL;

            // Je Eintrag ein Text und wahlweise ein zweiter Typ dazu; alle
            // Eintraege gehen in EINEM writeObjects: aufs Brett.
            let schreiben = |eintraege: &[(&CStr, Option<(Id, &CStr)>)]| {
                let mut items: Vec<Id> = Vec::new();
                for (text, zusatz) in eintraege {
                    let item = msg_id(msg_id(klasse(c"NSPasteboardItem"), sel(c"alloc")), sel(c"init"));
                    assert!(msg_bool_2(item, sel(c"setString:forType:"), ns_text(text), text_typ));
                    if let Some((typ, wert)) = zusatz {
                        assert!(msg_bool_2(item, sel(c"setString:forType:"), ns_text(wert), *typ));
                    }
                    items.push(item);
                }
                let liste: unsafe extern "C" fn(Id, Sel, *const Id, usize) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                let liste = liste(klasse(c"NSArray"), sel(c"arrayWithObjects:count:"), items.as_ptr(), items.len());
                let _ = msg_int(b, sel(c"clearContents"));
                let schreib: unsafe extern "C" fn(Id, Sel, Id) -> u8 = std::mem::transmute(objc_msgSend as *const c_void);
                assert!(schreib(b, sel(c"writeObjects:"), liste) != 0, "writeObjects: scheitert");
                for i in items {
                    let _ = msg_id(i, sel(c"release"));
                }
            };

            schreiben(&[(c"geheim123", Some((verdeckt, c"")))]);
            assert_eq!(lesen(b), None, "verdeckter Eintrag wurde gelesen");

            schreiben(&[(c"offen", None)]);
            assert_eq!(lesen(b).as_deref(), Some("offen"));

            schreiben(&[(c"eins", None), (c"zwei", None)]);
            assert_eq!(lesen(b).as_deref(), Some("eins"));

            schreiben(&[(c"probe.txt", Some((datei, c"file:///tmp/probe.txt")))]);
            assert_eq!(lesen(b), None, "Datei wurde als Text gelesen");

            let _ = msg_id(b, sel(c"releaseGlobally"));
        }
    }

    /// Ohne Sitzung zaehlt der Waechter nur mit und liest nichts - auch beim
    /// Sitzungsbeginn nicht nachtraeglich. Eine neue Kopie in der Sitzung
    /// wird gelesen. Eigenes, namenloses Brett.
    #[test]
    fn ohne_sitzung_wird_nicht_gelesen() {
        let _pool = Pool::neu();
        unsafe {
            let b = msg_id(klasse(c"NSPasteboard"), sel(c"pasteboardWithUniqueName"));
            assert!(!b.is_null(), "kein eigenes Brett");
            let mut zuletzt = change_count(b);
            let gelesen = std::cell::Cell::new(0);
            let zaehlend = |b: Id| {
                gelesen.set(gelesen.get() + 1);
                lesen(b)
            };

            assert!(ablegen(b, "kopiert ohne Sitzung").is_some());
            assert_eq!(nachsehen(b, &mut zuletzt, false, zaehlend), None);
            assert_eq!(gelesen.get(), 0, "ohne Sitzung gelesen");
            assert_eq!(zuletzt, change_count(b), "Aenderung nicht mitgezaehlt");

            // Die Sitzung beginnt: die Kopie von vorher bleibt hier.
            assert_eq!(nachsehen(b, &mut zuletzt, true, zaehlend), None);
            assert_eq!(gelesen.get(), 0, "alte Kopie beim Sitzungsbeginn gelesen");

            assert!(ablegen(b, "kopiert in der Sitzung").is_some());
            assert_eq!(nachsehen(b, &mut zuletzt, true, zaehlend).as_deref(), Some("kopiert in der Sitzung"));
            assert_eq!(gelesen.get(), 1);

            let _ = msg_id(b, sel(c"releaseGlobally"));
        }
    }

    /// Was vom Host kommt, liegt als ein Eintrag mit Text und dem
    /// Kennzeichen FLUECHTIG auf dem Brett; der zurueckgegebene Zaehler ist
    /// der Stand nach dem letzten Schreiben (sonst erkennte der Waechter den
    /// eigenen Vorgang nicht). Der Lesefilter nimmt fluechtige Eintraege
    /// weiter an - fluechtig heisst nicht verdeckt.
    #[test]
    fn vom_host_fluechtig() {
        let _pool = Pool::neu();
        unsafe {
            let b = msg_id(klasse(c"NSPasteboard"), sel(c"pasteboardWithUniqueName"));
            assert!(!b.is_null(), "kein eigenes Brett");
            let (nach_leeren, nach_setzen) = ablegen(b, "von drueben").expect("ablegen");
            assert_eq!(nach_setzen, change_count(b));
            assert!(nach_setzen >= nach_leeren);
            let eintraege = msg_id(b, sel(c"pasteboardItems"));
            assert_eq!(msg_int(eintraege, sel(c"count")), 1);
            let item = msg_id(eintraege, sel(c"firstObject"));
            assert!(hat_typ(item, ns_text(FLUECHTIG)), "Kennzeichen fehlt");
            assert!(hat_typ(item, NSPasteboardTypeString));
            assert_eq!(lesen(b).as_deref(), Some("von drueben"));
            let _ = msg_id(b, sel(c"releaseGlobally"));
        }
    }
}
