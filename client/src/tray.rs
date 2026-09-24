// Schliessen legt die App ab: unter Windows in den Infobereich (Tray), auf
// dem Mac in die Menueleiste (Spezifikation 3.9).
//
// Hier steht, was beide Plattformen teilen und was sich ohne Shell pruefen
// laesst: die Entscheidung beim Schliessen, das Menue aus der Hostliste und
// der Text des Tooltips. Das Symbol selbst bauen tray_win.rs
// (Shell_NotifyIconW in einem eigenen Faden) und tray_mac.rs (NSStatusBar auf
// dem Hauptfaden); beide bieten dieselbe Schnittstelle `Symbol`:
//
//   Symbol::neu(befehl, &stand) -> Result<Symbol, String>
//   symbol.steht() -> bool          Symbol wirklich sichtbar angemeldet?
//   symbol.grund() -> Option<String> warum nicht (Protokoll, Selbsttest)
//   symbol.stand_setzen(&stand)     Menue und Tooltip erneuern
//   symbol.hinweis(titel, text) -> bool  einmalige Sprechblase bzw. Hinweisblase;
//                                   true nur, wenn sie wirklich gezeigt wurde
//   symbol.takt()                   aus der Ereignisschleife, etwa 2 je Sekunde
//   drop(symbol)                    Symbol entfernen
//
// `befehl` wird aufgerufen, wenn der Nutzer am Symbol etwas waehlt; main.rs
// macht daraus ein Benutzerereignis fuer winit (EventLoopProxy).
//
// Das Fenster selbst verbirgt und zeigt main.rs (App::schliessen,
// App::fenster_zeigen); hier wird nur entschieden.

use crate::discovery;
use crate::strings::{self, Key};

#[cfg(windows)]
pub use crate::tray_win::Symbol;
#[cfg(target_os = "macos")]
pub use crate::tray_mac::Symbol;

/// Wie der Ort heisst, in Protokollzeilen.
#[cfg(not(target_os = "macos"))]
pub const ORT: &str = "Infobereich";
#[cfg(target_os = "macos")]
pub const ORT: &str = "Menueleiste";

/// Der Text der einmaligen Sprechblase: unter Windows "Infobereich", auf dem
/// Mac "Menueleiste".
#[cfg(not(target_os = "macos"))]
pub const HINWEIS: Key = Key::TrayStillRunning;
#[cfg(target_os = "macos")]
pub const HINWEIS: Key = Key::TrayStillRunningMac;

/// So viele gefundene Hosts stehen hoechstens im Menue.
pub const HOSTS_MAX: usize = 4;

/// So viele Zeichen eines Hostnamens stehen hoechstens im Menue und im
/// Tooltip; der Rest wird mit "…" gekuerzt. Namen kommen aus der
/// Bekanntgabe (bis 255 Byte) und damit von fremder Hand.
pub const NAME_MAX: usize = 40;

/// Was der Nutzer am Symbol gewaehlt hat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Befehl {
    /// Fenster zeigen (Linksklick, Doppelklick, Menuepunkt "Oeffnen").
    Oeffnen,
    /// Fenster zeigen und mit dieser Adresse verbinden, wie ein Klick auf
    /// die Hostzeile des Startbildschirms.
    Verbinden(String),
    /// Programm beenden.
    Beenden,
}

/// Ein Punkt des Kontextmenues, in dieser Reihenfolge angezeigt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Punkt {
    /// "Oeffnen" - unter Windows fett (Vorgabe fuer den Doppelklick).
    Oeffnen(String),
    Trenner,
    /// "Verbinden: <Name>" fuer einen gefundenen Host.
    Verbinden { text: String, adresse: String },
    Beenden(String),
}

impl Punkt {
    /// Was die Wahl dieses Punktes bewirkt (Trenner: nichts).
    pub fn befehl(&self) -> Option<Befehl> {
        match self {
            Punkt::Oeffnen(_) => Some(Befehl::Oeffnen),
            Punkt::Trenner => None,
            Punkt::Verbinden { adresse, .. } => Some(Befehl::Verbinden(adresse.clone())),
            Punkt::Beenden(_) => Some(Befehl::Beenden),
        }
    }
}

/// Alles, was das Symbol zeigt. main.rs rechnet es regelmaessig neu aus und
/// gibt es nur bei einer Aenderung weiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stand {
    pub menue: Vec<Punkt>,
    pub tooltip: String,
}

/// Den Stand aus Sprache, gefundenen Hosts und laufender Sitzung (Name oder
/// Adresse des Hosts; None auf dem Startbildschirm).
pub fn stand(lang: &strings::Lang, hosts: &[discovery::Host], sitzung: Option<&str>) -> Stand {
    Stand { menue: menue(lang, hosts), tooltip: tooltip(sitzung) }
}

/// Das Menue: Oeffnen, Trenner, bis zu vier gefundene Hosts, Trenner,
/// Beenden. Ohne Hosts steht nur ein Trenner zwischen Oeffnen und Beenden.
/// Die Hosts kommen in der Reihenfolge der Liste (nach Namen sortiert); ohne
/// Namen steht die Adresse da.
pub fn menue(lang: &strings::Lang, hosts: &[discovery::Host]) -> Vec<Punkt> {
    let mut m = vec![Punkt::Oeffnen(lang.get(Key::TrayOpen).to_string()), Punkt::Trenner];
    let mut mit_hosts = false;
    for h in hosts.iter().take(HOSTS_MAX) {
        let adresse = h.addr.to_string();
        let name = anzeigename(&h.name);
        let name = if name.is_empty() { adresse.clone() } else { name };
        m.push(Punkt::Verbinden { text: lang.get(Key::TrayConnect).replace("{n}", &name), adresse });
        mit_hosts = true;
    }
    if mit_hosts {
        m.push(Punkt::Trenner);
    }
    m.push(Punkt::Beenden(lang.get(Key::TrayQuit).to_string()));
    m
}

/// Der Tooltip: "QuadChroma", in einer Sitzung "QuadChroma – <Name bzw.
/// Adresse>".
pub fn tooltip(sitzung: Option<&str>) -> String {
    match sitzung.map(anzeigename) {
        Some(n) if !n.is_empty() => format!("QuadChroma – {n}"),
        _ => "QuadChroma".into(),
    }
}

/// Ein Name aus fremder Hand fuer Menue und Tooltip: Steuerzeichen werden
/// zu Leerzeichen (ein Tabulator truege unter Windows sonst ein Kuerzel ein),
/// aussen ohne Leerraum, hoechstens NAME_MAX Zeichen.
pub fn anzeigename(name: &str) -> String {
    let rein: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let rein = rein.trim();
    if rein.chars().count() <= NAME_MAX {
        return rein.to_string();
    }
    let mut kurz: String = rein.chars().take(NAME_MAX - 1).collect();
    kurz = kurz.trim_end().to_string();
    kurz.push('…');
    kurz
}

/// Text als UTF-16 mit hoechstens `max` Einheiten (ohne abschliessende
/// Null), fuer die festen Felder von NOTIFYICONDATAW (Tooltip 127, Hinweis
/// 255, Titel 63). Nie mitten in einem Ersatzpaar abgeschnitten.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn utf16_kuerzen(text: &str, max: usize) -> Vec<u16> {
    let mut v = Vec::with_capacity(max.min(text.len()));
    for c in text.chars() {
        let mut puffer = [0u16; 2];
        let teil = c.encode_utf16(&mut puffer);
        if v.len() + teil.len() > max {
            break;
        }
        v.extend_from_slice(teil);
    }
    v
}

/// Was beim Schliessen des Fensters geschieht.
#[derive(Debug, PartialEq, Eq)]
pub enum Schliessen {
    /// Fenster unsichtbar, App laeuft beim Symbol weiter. `trennen`: erst die
    /// laufende Sitzung beenden wie mit "Trennen". `hinweis`: die einmalige
    /// Sprechblase zeigen (und vermerken).
    Ablegen { trennen: bool, hinweis: bool },
    /// Programm beenden wie bisher.
    Beenden,
}

/// Entscheidung beim Schliessen. `tray_an`: einstellungen.txt erlaubt das
/// Ablegen (tray=aus verbietet es). `symbol_steht`: das Symbol ist wirklich
/// angemeldet - ohne es gaebe es keinen Weg zurueck, dann wird beendet.
/// `sitzung`: eine Sitzung laeuft oder wird aufgebaut. `hinweis_gezeigt`:
/// tray_hinweis=1 steht schon in einstellungen.txt.
pub fn beim_schliessen(tray_an: bool, symbol_steht: bool, sitzung: bool, hinweis_gezeigt: bool) -> Schliessen {
    if !tray_an || !symbol_steht {
        return Schliessen::Beenden;
    }
    Schliessen::Ablegen { trennen: sitzung, hinweis: !hinweis_gezeigt }
}

/// Ohne Windows und macOS gibt es kein Symbol: Schliessen beendet dort wie
/// bisher (beim_schliessen mit symbol_steht = false).
#[cfg(not(any(windows, target_os = "macos")))]
pub struct Symbol;

#[cfg(not(any(windows, target_os = "macos")))]
impl Symbol {
    pub fn neu(_befehl: Box<dyn Fn(Befehl) + Send>, _stand: &Stand) -> Result<Symbol, String> {
        Err("kein Infobereich auf dieser Plattform".into())
    }
    pub fn steht(&self) -> bool {
        false
    }
    pub fn grund(&self) -> Option<String> {
        None
    }
    pub fn stand_setzen(&mut self, _stand: &Stand) {}
    pub fn hinweis(&mut self, _titel: &str, _text: &str) -> bool {
        false
    }
    pub fn takt(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn host(name: &str, addr: &str) -> discovery::Host {
        discovery::Host { name: name.into(), addr: addr.parse().unwrap(), seen: Instant::now() }
    }

    #[test]
    fn entscheidung_beim_schliessen() {
        // Symbol steht, keine Sitzung, Hinweis noch nie gezeigt.
        assert_eq!(beim_schliessen(true, true, false, false), Schliessen::Ablegen { trennen: false, hinweis: true });
        // Mit Sitzung: erst trennen.
        assert_eq!(beim_schliessen(true, true, true, false), Schliessen::Ablegen { trennen: true, hinweis: true });
        // Hinweis schon gezeigt: nicht noch einmal.
        assert_eq!(beim_schliessen(true, true, true, true), Schliessen::Ablegen { trennen: true, hinweis: false });
        // Rueckfall: tray=aus oder kein Symbol - beenden wie bisher, auch
        // mitten in einer Sitzung.
        for sitzung in [false, true] {
            for hinweis in [false, true] {
                assert_eq!(beim_schliessen(false, true, sitzung, hinweis), Schliessen::Beenden);
                assert_eq!(beim_schliessen(true, false, sitzung, hinweis), Schliessen::Beenden);
                assert_eq!(beim_schliessen(false, false, sitzung, hinweis), Schliessen::Beenden);
            }
        }
    }

    #[test]
    fn menue_aus_der_hostliste() {
        let de = strings::pick("de");
        // Ohne Hosts: Oeffnen, ein Trenner, Beenden.
        assert_eq!(
            menue(de, &[]),
            vec![Punkt::Oeffnen("Öffnen".into()), Punkt::Trenner, Punkt::Beenden("Beenden".into())]
        );
        // Mit Hosts: hoechstens vier, in der Reihenfolge der Liste, ohne Namen
        // die Adresse, danach ein zweiter Trenner.
        let hosts = [
            host("Mac-mini-von-Robert.local", "192.168.178.194:9001"),
            host("", "192.168.178.60:9001"),
            host("c", "10.0.0.3:9001"),
            host("d", "10.0.0.4:9101"),
            host("e", "10.0.0.5:9001"),
        ];
        let m = menue(de, &hosts);
        assert_eq!(m.len(), 2 + 4 + 2);
        assert_eq!(m[0], Punkt::Oeffnen("Öffnen".into()));
        assert_eq!(m[1], Punkt::Trenner);
        assert_eq!(
            m[2],
            Punkt::Verbinden { text: "Verbinden: Mac-mini-von-Robert.local".into(), adresse: "192.168.178.194:9001".into() }
        );
        assert_eq!(m[3], Punkt::Verbinden { text: "Verbinden: 192.168.178.60:9001".into(), adresse: "192.168.178.60:9001".into() });
        assert_eq!(m[5], Punkt::Verbinden { text: "Verbinden: d".into(), adresse: "10.0.0.4:9101".into() });
        assert_eq!(m[6], Punkt::Trenner);
        assert_eq!(m[7], Punkt::Beenden("Beenden".into()));
        assert!(!m.iter().any(|p| matches!(p, Punkt::Verbinden { adresse, .. } if adresse == "10.0.0.5:9001")));
        // Was die Punkte bewirken.
        assert_eq!(m[0].befehl(), Some(Befehl::Oeffnen));
        assert_eq!(m[1].befehl(), None);
        assert_eq!(m[5].befehl(), Some(Befehl::Verbinden("10.0.0.4:9101".into())));
        assert_eq!(m[7].befehl(), Some(Befehl::Beenden));
        // Englisch, und in jeder Sprache steht der Name im Text.
        let en = menue(strings::pick("en"), &hosts[..1]);
        assert_eq!(en[0], Punkt::Oeffnen("Open".into()));
        assert_eq!(en[2], Punkt::Verbinden { text: "Connect: Mac-mini-von-Robert.local".into(), adresse: "192.168.178.194:9001".into() });
        assert_eq!(en[4], Punkt::Beenden("Quit".into()));
        for l in strings::all() {
            match &menue(l, &hosts[..1])[2] {
                Punkt::Verbinden { text, .. } => assert!(text.contains("Mac-mini-von-Robert.local"), "{}: {text}", l.code),
                p => panic!("{}: {p:?}", l.code),
            }
        }
    }

    #[test]
    fn fremde_namen_werden_entschaerft() {
        // Steuerzeichen (etwa ein Tabulator, der unter Windows ein Kuerzel
        // einleitet) werden zu Leerzeichen, zu lange Namen gekuerzt.
        assert_eq!(anzeigename("  a\tb\nc\u{7}  "), "a b c");
        let lang = "x".repeat(300);
        let k = anzeigename(&lang);
        assert_eq!(k.chars().count(), NAME_MAX);
        assert!(k.ends_with('…'));
        assert_eq!(anzeigename(&"y".repeat(NAME_MAX)), "y".repeat(NAME_MAX));
        let m = menue(strings::pick("de"), &[host("böse\tname", "10.0.0.1:9001")]);
        assert_eq!(m[2], Punkt::Verbinden { text: "Verbinden: böse name".into(), adresse: "10.0.0.1:9001".into() });
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(tooltip(None), "QuadChroma");
        assert_eq!(tooltip(Some("studio.local")), "QuadChroma – studio.local");
        assert_eq!(tooltip(Some("192.168.178.194:9001")), "QuadChroma – 192.168.178.194:9001");
        // Leer oder nur Leerraum: wie ohne Sitzung.
        assert_eq!(tooltip(Some("  ")), "QuadChroma");
        // Ein langer Name wird gekuerzt und passt so auch in die 127
        // Zeichen von szTip.
        let t = tooltip(Some(&"n".repeat(500)));
        assert!(t.chars().count() <= "QuadChroma – ".chars().count() + NAME_MAX);
        assert!(utf16_kuerzen(&t, 127).len() == t.encode_utf16().count());
        // Der ganze Stand in einem.
        let s = stand(strings::pick("en"), &[], Some("studio.local"));
        assert_eq!(s.tooltip, "QuadChroma – studio.local");
        assert_eq!(s.menue.len(), 3);
    }

    #[test]
    fn utf16_ohne_halbe_zeichen() {
        assert_eq!(utf16_kuerzen("abc", 2), vec![b'a' as u16, b'b' as u16]);
        assert_eq!(utf16_kuerzen("abc", 10).len(), 3);
        // "😀" braucht zwei Einheiten: bei Platz fuer drei bleibt es draussen.
        let v = utf16_kuerzen("ab😀", 3);
        assert_eq!(v, vec![b'a' as u16, b'b' as u16]);
        assert_eq!(utf16_kuerzen("ab😀", 4).len(), 4);
        assert!(utf16_kuerzen("", 5).is_empty());
    }

    #[test]
    fn hinweis_je_plattform() {
        let de = strings::pick("de");
        #[cfg(target_os = "macos")]
        assert_eq!(de.get(HINWEIS), "QuadChroma läuft in der Menüleiste weiter.");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(de.get(HINWEIS), "QuadChroma läuft im Infobereich weiter.");
    }
}
