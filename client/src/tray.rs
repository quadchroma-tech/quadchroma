// Schliessen legt die App ab: unter Windows in den Infobereich (Tray), auf
// dem Mac in die Menueleiste (Spezifikation 3.9).
//
// Hier steht, was beide Plattformen teilen und was sich ohne Shell pruefen
// laesst: die Entscheidung beim Schliessen und der Text des Tooltips, dazu
// `Eintrag`, der Baustein eines allgemeinen Menues (Untermenues, Haken,
// deaktivierte Zeilen, eigene Nummern). Unter Windows baut die eine App
// (Client und Host-Rolle, ein Symbol) ihr Menue ganz aus Eintraegen
// (symbolmenue.rs), und tray_win.rs zeigt es (Shell_NotifyIconW in einem
// eigenen Faden). Auf dem Mac ist das eine Symbol das der eingebauten
// Host-Engine (host/menue.m, Menue aus demselben Aufbau wie symbolmenue.rs,
// Texte in host/texte.m); host_mac::Symbol gibt ihm den Stand des Clients
// und fragt, ob es steht. Beide melden die Wahl als Benutzerereignis
// (Benutzer::Menue) an winit.
//
// Das Fenster selbst verbirgt und zeigt main.rs (App::schliessen,
// App::fenster_zeigen); hier wird nur entschieden.

use crate::strings::Key;

#[cfg(windows)]
pub use crate::tray_win::Symbol;
#[cfg(target_os = "macos")]
pub use crate::host_mac::Symbol;

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

/// Ein Eintrag eines allgemeinen Menues (Symbol der einen App unter
/// Windows, symbolmenue.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Eintrag {
    /// Waehlbarer (oder deaktivierter) Punkt mit eigener Befehlsnummer
    /// (nicht 0). `haken`: mit Haken davor; `fett`: Vorgabe des Menues.
    Punkt { text: String, nummer: u32, haken: bool, aktiv: bool, fett: bool },
    /// Untermenue.
    Unter { text: String, eintraege: Vec<Eintrag>, aktiv: bool },
    Trenner,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Eintrag {
    /// Ein gewoehnlicher, waehlbarer Punkt.
    pub fn punkt(text: impl Into<String>, nummer: u32) -> Eintrag {
        Eintrag::Punkt { text: text.into(), nummer, haken: false, aktiv: true, fett: false }
    }

    /// Ein waehlbarer Punkt mit Haken (gesetzt oder nicht).
    pub fn schalter(text: impl Into<String>, nummer: u32, an: bool) -> Eintrag {
        Eintrag::Punkt { text: text.into(), nummer, haken: an, aktiv: true, fett: false }
    }

    /// Eine Zeile, die nur etwas anzeigt (deaktiviert, ohne Befehl).
    pub fn anzeige(text: impl Into<String>) -> Eintrag {
        Eintrag::Punkt { text: text.into(), nummer: 0, haken: false, aktiv: false, fett: false }
    }
}

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
    pub fn steht(&self) -> bool {
        false
    }
    pub fn grund(&self) -> Option<String> {
        None
    }
    pub fn hinweis(&mut self, _titel: &str, _text: &str) -> bool {
        false
    }
    pub fn takt(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings;

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
    fn fremde_namen_werden_entschaerft() {
        // Steuerzeichen (etwa ein Tabulator, der unter Windows ein Kuerzel
        // einleitet) werden zu Leerzeichen, zu lange Namen gekuerzt.
        assert_eq!(anzeigename("  a\tb\nc\u{7}  "), "a b c");
        let lang = "x".repeat(300);
        let k = anzeigename(&lang);
        assert_eq!(k.chars().count(), NAME_MAX);
        assert!(k.ends_with('…'));
        assert_eq!(anzeigename(&"y".repeat(NAME_MAX)), "y".repeat(NAME_MAX));
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
