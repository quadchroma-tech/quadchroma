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
// macOS (IOPMAssertion) folgt mit der einen App auf dem Mac; bis dahin
// meldet `setzen` dort, dass es das nicht gibt.
//
// Die Entscheidung, was ein Wechsel am System tut, ist reine Rechnung
// (`schritt`) und laeuft auf jeder Plattform im Test.

// Unter macOS ruft das noch niemand auf.
#![cfg_attr(not(windows), allow(dead_code))]

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

/// Die Energieanforderung dieses Prozesses (hoechstens eine). Faellt sie
/// weg, ist sie aufgehoben.
#[derive(Default)]
pub struct Ruhesperre {
    /// Griff der gesetzten Anforderung (Windows).
    #[cfg(windows)]
    griff: Option<isize>,
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
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// Den Ruhezustand verhindern (`soll`) oder wieder zulassen - sofort.
    /// `grund` steht in powercfg /requests. Liefert, was geschah; Err mit
    /// dem Wortlaut des Systems (der Zustand bleibt dann, wie er war).
    pub fn setzen(&mut self, soll: bool, grund: &str) -> Result<Schritt, String> {
        let s = schritt(self.an(), soll);
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
        #[cfg(not(windows))]
        {
            let _ = grund;
            if s == Schritt::Setzen {
                return Err("Ruhezustand verhindern gibt es auf dieser Plattform noch nicht".into());
            }
        }
        Ok(s)
    }
}

impl Drop for Ruhesperre {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(h) = self.griff.take() {
            let _ = win::aufheben(h);
        }
    }
}

#[cfg(windows)]
mod win {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
    use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

    /// POWER_REQUEST_CONTEXT_VERSION (Win32_System_SystemServices ist nicht
    /// eingebunden).
    const FASSUNG: u32 = 0;

    fn wortlaut(was: &str, e: windows::core::Error) -> String {
        format!("{was}: {} (0x{:08x})", e.message().trim(), e.code().0 as u32)
    }

    /// Anforderung mit Grund anlegen und setzen; der Griff als Zahl.
    pub fn anfordern(grund: &str) -> Result<isize, String> {
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
    pub fn aufheben(h: isize) -> Result<(), String> {
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

    /// Ohne Windows gibt es (noch) nichts zu setzen; aus bleibt aus.
    #[cfg(not(windows))]
    #[test]
    fn ohne_windows_nicht_verfuegbar() {
        let mut r = Ruhesperre::neu();
        assert!(r.setzen(true, "x").is_err());
        assert!(!r.an());
        assert_eq!(r.setzen(false, "x"), Ok(Schritt::Nichts));
    }
}
