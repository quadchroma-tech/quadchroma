// "Ruhezustand verhindern, solange QuadChroma laeuft" (Einstellung
// ruhezustand_verhindern, Haken am Symbol und Kaestchen im Startbildschirm).
//
// Windows: eine Energieanforderung des Prozesses (PowerCreateRequest mit
// einem Grund als Text, PowerSetRequest(PowerRequestSystemRequired)) - der
// Rechner schlaeft nicht von selbst ein, der Bildschirm darf trotzdem
// ausgehen. Anders als SetThreadExecutionState haengt sie an keinem Faden,
// sondern an ihrem Griff: sie gilt, bis PowerClearRequest oder das Ende des
// Prozesses sie aufhebt, und steht mit dem Grund in "powercfg /requests"
// (Abschnitt SYSTEM, Eintrag [PROCESS] ...\quadchroma.exe). Unabhaengig von
// Sitzungen; das Wachhalten waehrend einer Sitzung (host/aufnahme.rs,
// SetThreadExecutionState samt Bildschirm) bleibt, wie es ist.
//
// macOS: eine Zusicherung der Energieverwaltung (IOPMAssertionCreateWithName
// mit kIOPMAssertPreventUserIdleSystemSleep, Name = der Text des Haken) -
// der Mac schlaeft nicht von selbst ein, der Bildschirm darf ausgehen. Sie
// gilt, bis IOPMAssertionRelease oder das Ende des Prozesses sie aufhebt,
// und steht mit ihrem Namen in "pmset -g assertions". Das Wachhalten
// waehrend einer Sitzung (host/main.m, PreventUserIdleDisplaySleep) bleibt,
// wie es ist.
//
// Andere Plattformen: `setzen` meldet, dass es das nicht gibt.
//
// Lehnt das System ab, bleibt der Wunsch (einstellungen.txt), aber Haken und
// Kaestchen zeigen, was gilt - aus -, und daneben steht der Fehlercode des
// Systems (`abgelehnt`, Text PreventSleepRefused). Ein neuer Klick versucht es
// wieder.
//
// Die Entscheidung, was ein Wechsel am System tut, ist reine Rechnung
// (`schritt`) und laeuft auf jeder Plattform im Test.

// Ohne Windows und macOS ruft das niemand auf.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

/// Was ein gewuenschter Zustand am System aendert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schritt {
    /// Schon so.
    Nichts,
    /// Anforderung anlegen und setzen.
    Setzen,
    /// Anforderung aufheben und schliessen.
    Loesen,
}

/// Von `jetzt` (Anforderung gesetzt?) zu `soll`.
pub fn schritt(jetzt: bool, soll: bool) -> Schritt {
    match (jetzt, soll) {
        (false, true) => Schritt::Setzen,
        (true, false) => Schritt::Loesen,
        _ => Schritt::Nichts,
    }
}

/// Warum das System ablehnte: der Wortlaut fuers Protokoll und der kurze
/// Fehlercode fuer die Oberflaeche.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ablehnung {
    wortlaut: String,
    code: String,
}

/// Die Energieanforderung dieses Prozesses (hoechstens eine). Faellt sie
/// weg, ist sie aufgehoben.
#[derive(Default)]
pub struct Ruhesperre {
    /// Griff der gesetzten Anforderung (Windows).
    #[cfg(windows)]
    griff: Option<isize>,
    /// Nummer der gesetzten Zusicherung (macOS).
    #[cfg(target_os = "macos")]
    zusicherung: Option<u32>,
    /// Fehlercode des Systems, als es das letzte Setzen ablehnte (bis zum
    /// naechsten gelungenen Setzen oder bis "aus").
    abgelehnt: Option<String>,
}

/// Was Haken und Kaestchen zeigen: an, was gilt (`an`) - nicht, was
/// gewuenscht ist -, und einen Grund (den Fehlercode), wenn der Wunsch "an"
/// ist, das System aber ablehnte.
pub fn anzeige(wunsch: bool, an: bool, abgelehnt: Option<&str>) -> (bool, Option<String>) {
    (an, abgelehnt.filter(|_| wunsch && !an).map(str::to_string))
}

impl Ruhesperre {
    pub fn neu() -> Ruhesperre {
        Ruhesperre::default()
    }

    /// Ist die Anforderung gesetzt?
    pub fn an(&self) -> bool {
        #[cfg(windows)]
        {
            self.griff.is_some()
        }
        #[cfg(target_os = "macos")]
        {
            self.zusicherung.is_some()
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            false
        }
    }

    /// Der Fehlercode des Systems, wenn es das letzte Setzen ablehnte.
    pub fn abgelehnt(&self) -> Option<&str> {
        self.abgelehnt.as_deref()
    }

    /// Den Ruhezustand verhindern (`soll`) oder wieder zulassen - sofort.
    /// `grund` steht in powercfg /requests bzw. pmset -g assertions.
    /// Liefert, was geschah; Err mit dem Wortlaut des Systems (der Zustand
    /// bleibt dann, wie er war). Lehnt das System das Setzen ab, merkt sich
    /// die Sperre den Fehlercode (`abgelehnt`); "aus" und ein gelungenes
    /// Setzen vergessen ihn.
    pub fn setzen(&mut self, soll: bool, grund: &str) -> Result<Schritt, String> {
        let s = schritt(self.an(), soll);
        match self.umstellen(s, grund) {
            Ok(()) => {
                if s == Schritt::Setzen || !soll {
                    self.abgelehnt = None;
                }
                Ok(s)
            }
            Err(a) => {
                if s == Schritt::Setzen {
                    self.abgelehnt = Some(a.code);
                } else if !soll {
                    self.abgelehnt = None;
                }
                Err(a.wortlaut)
            }
        }
    }

    fn umstellen(&mut self, s: Schritt, grund: &str) -> Result<(), Ablehnung> {
        #[cfg(windows)]
        match s {
            Schritt::Nichts => {}
            Schritt::Setzen => self.griff = Some(win::anfordern(grund)?),
            Schritt::Loesen => {
                if let Some(h) = self.griff.take() {
                    win::aufheben(h)?;
                }
            }
        }
        #[cfg(target_os = "macos")]
        match s {
            Schritt::Nichts => {}
            Schritt::Setzen => self.zusicherung = Some(mac::anfordern(grund)?),
            Schritt::Loesen => {
                if let Some(z) = self.zusicherung.take() {
                    mac::aufheben(z)?;
                }
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = grund;
            if s == Schritt::Setzen {
                return Err(Ablehnung { wortlaut: "Ruhezustand verhindern gibt es auf dieser Plattform nicht".into(), code: "-".into() });
            }
        }
        Ok(())
    }
}

impl Drop for Ruhesperre {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(h) = self.griff.take() {
            let _ = win::aufheben(h);
        }
        #[cfg(target_os = "macos")]
        if let Some(z) = self.zusicherung.take() {
            let _ = mac::aufheben(z);
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Ablehnung;
    use std::ffi::c_void;

    type CFStringRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, kodierung: u32, extern_: u8) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(typ: CFStringRef, stufe: u32, name: CFStringRef, nummer: *mut u32) -> i32;
        fn IOPMAssertionRelease(nummer: u32) -> i32;
    }

    /// kCFStringEncodingUTF8.
    const UTF8: u32 = 0x0800_0100;
    /// kIOPMAssertionLevelOn.
    const AN: u32 = 255;
    /// kIOPMAssertPreventUserIdleSystemSleep (ein CFSTR im Kopf IOPMLib.h).
    const TYP: &str = "PreventUserIdleSystemSleep";

    /// Ein CFString, den der Aufrufer freigibt.
    fn cf_text(t: &str) -> Result<CFStringRef, Ablehnung> {
        // SAFETY: Zeiger und Laenge beschreiben gueltiges UTF-8; CoreFoundation kopiert.
        let s = unsafe { CFStringCreateWithBytes(std::ptr::null(), t.as_ptr(), t.len() as isize, UTF8, 0) };
        if s.is_null() {
            Err(Ablehnung { wortlaut: format!("CFStringCreateWithBytes: {t:?} nicht anlegbar"), code: "CFString".into() })
        } else {
            Ok(s)
        }
    }

    /// Ein Fehler von IOKit: Wortlaut mit Funktion, Code allein.
    fn abgelehnt(was: &str, r: i32) -> Ablehnung {
        let code = format!("0x{:08x}", r as u32);
        Ablehnung { wortlaut: format!("{was}: {code}"), code }
    }

    /// Zusicherung mit Namen anlegen; ihre Nummer.
    pub fn anfordern(grund: &str) -> Result<u32, Ablehnung> {
        let typ = cf_text(TYP)?;
        let name = match cf_text(grund) {
            Ok(n) => n,
            Err(e) => {
                // SAFETY: eben angelegt.
                unsafe { CFRelease(typ) };
                return Err(e);
            }
        };
        let mut nummer = 0u32;
        // SAFETY: gueltige CFStrings; das System kopiert den Namen.
        let r = unsafe { IOPMAssertionCreateWithName(typ, AN, name, &mut nummer) };
        // SAFETY: beide eben angelegt, nur hier benutzt.
        unsafe {
            CFRelease(typ);
            CFRelease(name);
        }
        if r != 0 {
            return Err(abgelehnt("IOPMAssertionCreateWithName", r));
        }
        Ok(nummer)
    }

    /// Zusicherung aufheben.
    pub fn aufheben(nummer: u32) -> Result<(), Ablehnung> {
        // SAFETY: nur eine Nummer; eine falsche meldet das System als Fehler.
        let r = unsafe { IOPMAssertionRelease(nummer) };
        if r != 0 {
            return Err(abgelehnt("IOPMAssertionRelease", r));
        }
        Ok(())
    }
}

/// Die Zeilen von "pmset -g assertions", die diesen Namen tragen (macOS;
/// fuer den Selbsttest und den Test). pmset gibt nur ASCII lesbar aus -
/// gesucht wird deshalb nach einem Namen ohne andere Zeichen. None: pmset
/// lief nicht.
#[cfg(target_os = "macos")]
pub fn pmset_zeilen(name: &str) -> Option<Vec<String>> {
    let aus = std::process::Command::new("/usr/bin/pmset").args(["-g", "assertions"]).output().ok()?;
    let text = String::from_utf8_lossy(&aus.stdout).into_owned();
    Some(text.lines().filter(|z| z.contains(name)).map(|z| z.trim().to_string()).collect())
}

#[cfg(windows)]
mod win {
    use super::Ablehnung;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
    use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

    /// POWER_REQUEST_CONTEXT_VERSION (Win32_System_SystemServices ist nicht
    /// eingebunden).
    const FASSUNG: u32 = 0;

    fn wortlaut(was: &str, e: windows::core::Error) -> Ablehnung {
        let code = format!("0x{:08x}", e.code().0 as u32);
        Ablehnung { wortlaut: format!("{was}: {} ({code})", e.message().trim()), code }
    }

    /// Anforderung mit Grund anlegen und setzen; der Griff als Zahl.
    pub fn anfordern(grund: &str) -> Result<isize, Ablehnung> {
        // Das System kopiert den Text beim Anlegen.
        let mut text: Vec<u16> = grund.encode_utf16().chain(std::iter::once(0)).collect();
        let ctx = REASON_CONTEXT {
            Version: FASSUNG,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 { SimpleReasonString: PWSTR(text.as_mut_ptr()) },
        };
        let h = unsafe { PowerCreateRequest(&ctx) }.map_err(|e| wortlaut("PowerCreateRequest", e))?;
        if let Err(e) = unsafe { PowerSetRequest(h, PowerRequestSystemRequired) } {
            unsafe {
                let _ = CloseHandle(h);
            }
            return Err(wortlaut("PowerSetRequest", e));
        }
        Ok(h.0 as isize)
    }

    /// Anforderung aufheben und den Griff schliessen (auch wenn das Aufheben
    /// scheitert - mit dem Griff faellt die Anforderung ohnehin).
    pub fn aufheben(h: isize) -> Result<(), Ablehnung> {
        let h = HANDLE(h as *mut _);
        let r = unsafe { PowerClearRequest(h, PowerRequestSystemRequired) }.map_err(|e| wortlaut("PowerClearRequest", e));
        unsafe {
            let _ = CloseHandle(h);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jeder Wechsel am System genau einmal: setzen nur aus "aus", loesen nur
    /// aus "an"; derselbe Zustand noch einmal tut nichts.
    #[test]
    fn schritt_je_zustand() {
        assert_eq!(schritt(false, true), Schritt::Setzen);
        assert_eq!(schritt(true, false), Schritt::Loesen);
        assert_eq!(schritt(true, true), Schritt::Nichts);
        assert_eq!(schritt(false, false), Schritt::Nichts);
    }

    /// Windows: die Anforderung wird gesetzt und wieder aufgehoben - an, noch
    /// einmal an (nichts), aus, noch einmal aus (nichts), an - und mit dem
    /// Fallenlassen wieder weg.
    #[cfg(windows)]
    #[test]
    fn anforderung_setzen_und_aufheben() {
        let mut r = Ruhesperre::neu();
        assert!(!r.an());
        assert_eq!(r.setzen(true, "QuadChroma Test"), Ok(Schritt::Setzen));
        assert!(r.an());
        assert_eq!(r.setzen(true, "QuadChroma Test"), Ok(Schritt::Nichts));
        assert_eq!(r.setzen(false, ""), Ok(Schritt::Loesen));
        assert!(!r.an());
        assert_eq!(r.setzen(false, ""), Ok(Schritt::Nichts));
        assert_eq!(r.setzen(true, "QuadChroma Test"), Ok(Schritt::Setzen));
        drop(r);
    }

    /// macOS: die Zusicherung steht mit ihrem Namen und ihrer Art in pmset
    /// -g assertions, solange sie gilt - an, noch einmal an (nichts), aus
    /// (weg), noch einmal aus (nichts), an, und mit dem Fallenlassen weg.
    #[cfg(target_os = "macos")]
    #[test]
    fn zusicherung_in_pmset() {
        let name = format!("QuadChroma Test {}", std::process::id());
        let mut r = Ruhesperre::neu();
        assert!(!r.an());
        assert_eq!(r.setzen(true, &name), Ok(Schritt::Setzen));
        assert!(r.an());
        let z = pmset_zeilen(&name).expect("pmset");
        assert!(z.iter().any(|l| l.contains("PreventUserIdleSystemSleep")), "{z:?}");
        assert_eq!(r.setzen(true, &name), Ok(Schritt::Nichts));
        assert_eq!(r.setzen(false, ""), Ok(Schritt::Loesen));
        assert!(!r.an());
        assert_eq!(pmset_zeilen(&name).expect("pmset"), Vec::<String>::new());
        assert_eq!(r.setzen(false, ""), Ok(Schritt::Nichts));
        assert_eq!(r.setzen(true, &name), Ok(Schritt::Setzen));
        drop(r);
        assert_eq!(pmset_zeilen(&name).expect("pmset"), Vec::<String>::new());
    }

    /// Was Haken und Kaestchen zeigen: was gilt, und einen Grund nur, wenn
    /// "an" gewuenscht ist, aber nicht gilt.
    #[test]
    fn anzeige_zeigt_was_gilt() {
        assert_eq!(anzeige(true, true, None), (true, None));
        assert_eq!(anzeige(true, false, Some("0xe00002c2")), (false, Some("0xe00002c2".into())));
        assert_eq!(anzeige(false, false, Some("0xe00002c2")), (false, None));
        assert_eq!(anzeige(true, true, Some("alt")), (true, None));
        assert_eq!(anzeige(true, false, None), (false, None));
    }

    /// Lehnt das System ab, merkt sich die Sperre den Code; "aus" vergisst
    /// ihn, und ein gelungenes Setzen ebenso. Geprueft mit einer Sperre,
    /// deren Setzen scheitern muss: auf den Plattformen mit Energieverwaltung
    /// direkt am Zustand (das System lehnt im Test nicht ab).
    #[test]
    fn abgelehnt_gemerkt_und_vergessen() {
        let mut r = Ruhesperre::neu();
        r.abgelehnt = Some("0x1".into());
        assert_eq!(r.abgelehnt(), Some("0x1"));
        assert_eq!(r.setzen(false, ""), Ok(Schritt::Nichts));
        assert_eq!(r.abgelehnt(), None, "aus vergisst den Grund");
        #[cfg(any(windows, target_os = "macos"))]
        {
            r.abgelehnt = Some("0x1".into());
            assert_eq!(r.setzen(true, "QuadChroma Test"), Ok(Schritt::Setzen));
            assert_eq!(r.abgelehnt(), None, "gelungenes Setzen vergisst den Grund");
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            assert!(r.setzen(true, "x").is_err());
            assert_eq!(r.abgelehnt(), Some("-"));
        }
    }

    /// Ohne Windows und macOS gibt es nichts zu setzen; aus bleibt aus.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn ohne_windows_nicht_verfuegbar() {
        let mut r = Ruhesperre::neu();
        assert!(r.setzen(true, "x").is_err());
        assert!(!r.an());
        assert_eq!(r.setzen(false, "x"), Ok(Schritt::Nichts));
    }
}
