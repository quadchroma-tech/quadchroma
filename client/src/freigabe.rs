// Die Host-Rolle der einen App, so wie main.rs sie sieht - auf jeder
// Plattform dieselbe Schnittstelle. Unter Windows steckt host::Rolle
// dahinter (der Dienst in einem eigenen Faden des App-Prozesses), auf dem
// Mac die eingebaute Host-Engine (host_mac::Dienst, auf dem Hauptfaden:
// sie laeuft erst mit `anlaufen` aus resumed an, weil AppKit dann steht).
// Anderswo liefert `starten` None, und die App ist nur Client.

// Auf dem Mac bleibt der Teil ungenutzt, den unter Windows der Faden des
// Symbols braucht (menue_abfrage, Abschied beim Prozessende).
#![cfg_attr(not(windows), allow(dead_code))]

use crate::symbolmenue;
use std::sync::Arc;
use std::time::Duration;

/// So lange wartet das Beenden der App hoechstens auf die Host-Rolle
/// (Abschied an einen Zuschauer hoechstens 3 s, dann die Ports).
pub const ENDE_FRIST: Duration = Duration::from_secs(5);

/// Die Host-Rolle im Prozess der App.
pub struct Freigabe {
    #[cfg(windows)]
    rolle: crate::host::Rolle,
    #[cfg(target_os = "macos")]
    dienst: crate::host_mac::Dienst,
}

impl Freigabe {
    /// Die Host-Rolle starten: `args` die Befehlszeile (Port hinter --host
    /// und die Schalter der Host-Rolle), `an` gleich freigeben, `steht` und
    /// `hinweis` das Symbol der App, `bereit` kommt, sobald die Rolle ihre ID
    /// kennt. None: gibt es auf dieser Plattform nicht. Auf dem Mac startet
    /// hier noch nichts (`anlaufen`); steht, hinweis und bereit braucht er
    /// nicht - Zulassen, Hinweise und die ID kennt die Engine selbst.
    #[allow(unused_variables)]
    pub fn starten(
        args: Vec<String>,
        an: bool,
        steht: Arc<dyn Fn() -> bool + Send + Sync>,
        hinweis: Arc<dyn Fn(&str) + Send + Sync>,
        bereit: Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Freigabe> {
        #[cfg(windows)]
        {
            Some(Freigabe { rolle: crate::host::Rolle::starten(args, an, steht, hinweis, bereit) })
        }
        #[cfg(target_os = "macos")]
        {
            Some(Freigabe { dienst: crate::host_mac::Dienst::neu(&args, an) })
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            None
        }
    }

    /// Aus resumed, auf dem Hauptfaden (macOS): die Engine einrichten, mit
    /// Freigabe den Dienst starten, Symbol und Programmmenue anlegen. `sprache`
    /// der Code der Sprache des Clients, `name` der eingestellte Geraetename.
    /// false: es gibt kein Symbol (die Engine liess sich nicht einrichten) -
    /// dann braucht die App ihr Fenster. Unter Windows laeuft die Rolle schon
    /// seit `starten` (true).
    #[allow(unused_variables)]
    pub fn anlaufen(&mut self, sprache: &str, name: Option<&str>) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.dienst.anlaufen(sprache, name)
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }

    /// Freigabe an oder aus.
    #[allow(unused_variables)]
    pub fn setzen(&mut self, an: bool) {
        #[cfg(windows)]
        self.rolle.freigabe_setzen(an);
        #[cfg(target_os = "macos")]
        self.dienst.setzen(an);
    }

    /// Einen Menuepunkt der Host-Rolle ausfuehren. false: der Punkt gehoert
    /// nicht der Host-Rolle (die App fuehrt ihn selbst aus).
    #[allow(unused_variables)]
    pub fn aktion(&self, a: &symbolmenue::Aktion) -> bool {
        #[cfg(windows)]
        if let Some(h) = crate::host::oberflaeche::Aktion::aus_menue(a) {
            self.rolle.aktion(h);
            return true;
        }
        false
    }

    /// Der Geraetename ist neu (schon gesetzt): Nachricht 20 ab sofort (auf
    /// dem Mac auch Bekanntgabe und Menue - die Engine hat eigene Wege).
    pub fn name_geaendert(&self) {
        #[cfg(windows)]
        self.rolle.name_geaendert();
        #[cfg(target_os = "macos")]
        crate::host_mac::Dienst::name_setzen(crate::zugang::geraetename_eingestellt().as_deref());
    }

    /// Geraete-ID der Host-Rolle (None, solange sie nicht eingerichtet ist).
    pub fn id(&self) -> Option<u32> {
        #[cfg(windows)]
        {
            self.rolle.id()
        }
        #[cfg(target_os = "macos")]
        {
            self.dienst.id()
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            None
        }
    }

    /// Was die Host-Rolle zum Menue beitraegt - als Abfrage fuer den Faden
    /// des Symbols.
    pub fn menue_abfrage(&self) -> Arc<dyn Fn() -> symbolmenue::HostTeil + Send + Sync> {
        #[cfg(windows)]
        {
            self.rolle.menue_abfrage()
        }
        #[cfg(not(windows))]
        {
            Arc::new(|| symbolmenue::HostTeil { freigabe: symbolmenue::Freigabe::Aus, id: None, passwort: Err(()), geraete: Err(()) })
        }
    }

    /// Die Host-Rolle beenden (Abschied an einen Zuschauer, Grund 0; Ports zu
    /// - auf dem Mac mit dem Ende des Prozesses).
    pub fn beenden(&mut self) {
        #[cfg(windows)]
        self.rolle.beenden(ENDE_FRIST);
        #[cfg(target_os = "macos")]
        self.dienst.beenden();
    }
}

/// Die App endet von aussen (WM_CLOSE an ihr Symbol, WM_ENDSESSION): der
/// Abschied an einen Zuschauer geht hinaus, bevor der aufrufende Faden
/// zurueckkehrt.
pub fn abschied_beim_prozessende() {
    #[cfg(windows)]
    crate::host::abschied_beim_prozessende();
}
