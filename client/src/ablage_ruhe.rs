// Entprellen und Entdoppeln der Ablage-Meldungen, gemeinsam fuer
// clipboard.rs (Windows) und clipboard_mac.rs (Mac-Client). Reine Logik, ohne
// Ablage: die Waechter geben die Zeitpunkte hinein.
//
// Warum (Integrationstest 574cd3e, Befunde 1 und 2):
//   - Eine Kopie loest oft mehrere Meldungen aus (.NET-Kopien mit
//     SetFileDropList zwei, Programme mit verzoegertem Rendern mehr). Jede
//     Meldung las die Ablage, und jede Dateiliste brach die laufende Sendung
//     ab und startete eine neue - samt Anlegen und Loeschen eines
//     Uebertragungsverzeichnisses beim Empfaenger.
//   - Wer gleich bei der ersten Meldung liest, oeffnet die Ablage, waehrend
//     die Quelle noch in OleFlushClipboard steckt; andere Programme scheitern
//     dann kurz mit "Ablage belegt".
//
// Also: nach einer Meldung erst RUHE abwarten (jede weitere Meldung schiebt
// den Zeitpunkt hinaus), dann einmal lesen (Entpreller). Und eine Dateiliste,
// die gerade erst gemeldet wurde, innerhalb von DOPPEL_FRIST nicht noch
// einmal melden (Entdoppler) - Text dagegen immer, er ist klein und bricht
// nichts Teures ab.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// So lange muss die Ablage nach der letzten Meldung ruhig sein, bevor
/// gelesen wird.
pub const RUHE: Duration = Duration::from_millis(150);

/// Innerhalb dieser Zeit gilt dieselbe Dateiliste als dieselbe Kopie.
pub const DOPPEL_FRIST: Duration = Duration::from_secs(2);

/// Wartet auf Ruhe: `aenderung` bei jeder Meldung, `faellig` sagt, wann
/// gelesen werden darf (genau einmal je Ruhephase).
#[derive(Debug)]
pub struct Entpreller {
    ruhe: Duration,
    faellig: Option<Instant>,
}

impl Entpreller {
    pub fn neu(ruhe: Duration) -> Entpreller {
        Entpreller { ruhe, faellig: None }
    }

    /// Die Ablage hat sich geaendert: gelesen wird fruehestens `ruhe` danach.
    pub fn aenderung(&mut self, jetzt: Instant) {
        self.faellig = Some(jetzt + self.ruhe);
    }

    /// Darf jetzt gelesen werden? true genau einmal je Ruhephase; danach
    /// wieder erst nach einer neuen Aenderung.
    pub fn faellig(&mut self, jetzt: Instant) -> bool {
        match self.faellig {
            Some(t) if jetzt >= t => {
                self.faellig = None;
                true
            }
            _ => false,
        }
    }

    /// Ein anstehendes Lesen faellt weg: die Aenderung kam ohne Gegenueber
    /// und hat den Inhalt davor ueberschrieben - sie selbst wird nie gelesen.
    pub fn verwerfen(&mut self) {
        self.faellig = None;
    }

    /// Steht ein Lesen aus (Aenderung ohne folgendes `faellig`)? Braucht nur
    /// der Zeitgeber unter Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn wartet(&self) -> bool {
        self.faellig.is_some()
    }

    /// Wie lange hoechstens bis zum naechsten Blick: bis zur Faelligkeit,
    /// sonst `sonst` (der Takt des Waechters).
    pub fn schlaf(&self, jetzt: Instant, sonst: Duration) -> Duration {
        match self.faellig {
            Some(t) => t.saturating_duration_since(jetzt).min(sonst),
            None => sonst,
        }
    }
}

/// Laesst dieselbe Dateiliste innerhalb von `frist` nur einmal durch.
#[derive(Debug)]
pub struct Entdoppler {
    frist: Duration,
    /// Die zuletzt gemeldete Dateiliste, wann, und in welcher Sitzung
    /// (`sitzung` zaehlt der Waechter bei jedem Sitzungswechsel hoch).
    zuletzt: Option<(Vec<PathBuf>, Instant, u64)>,
}

impl Entdoppler {
    pub fn neu(frist: Duration) -> Entdoppler {
        Entdoppler { frist, zuletzt: None }
    }

    /// Soll diese Dateiliste gemeldet werden? Nein, wenn genau dieselbe
    /// Liste in derselben Sitzung vor weniger als `frist` gemeldet wurde.
    /// Die Frist laeuft ab der gemeldeten Liste, nicht ab der verworfenen.
    pub fn dateien(&mut self, pfade: &[PathBuf], jetzt: Instant, sitzung: u64) -> bool {
        if let Some((liste, seit, s)) = &self.zuletzt {
            if *s == sitzung && liste.as_slice() == pfade && jetzt.saturating_duration_since(*seit) < self.frist {
                return false;
            }
        }
        self.zuletzt = Some((pfade.to_vec(), jetzt, sitzung));
        true
    }

    /// Text wurde gemeldet: er hat die Dateiliste abgeloest, dieselbe Liste
    /// danach ist eine neue Kopie.
    pub fn text(&mut self) {
        self.zuletzt = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// Mehrere Meldungen dicht hintereinander ergeben EIN Lesen, RUHE nach
    /// der letzten; ohne neue Meldung kein zweites.
    #[test]
    fn entprellen_liest_einmal_nach_ruhe() {
        let t0 = Instant::now();
        let mut e = Entpreller::neu(RUHE);
        assert!(!e.faellig(t0 + MS(1000)), "ohne Aenderung faellig");
        assert!(!e.wartet());
        e.aenderung(t0);
        assert!(e.wartet());
        e.aenderung(t0 + MS(40));
        e.aenderung(t0 + MS(100));
        assert!(!e.faellig(t0 + MS(150)), "vor der Ruhe nach der letzten Meldung gelesen");
        assert!(!e.faellig(t0 + MS(249)));
        assert!(e.faellig(t0 + MS(250)));
        assert!(!e.wartet());
        assert!(!e.faellig(t0 + MS(260)), "zweimal gelesen");
        assert!(!e.faellig(t0 + MS(5000)));
        // Die naechste Kopie: wieder genau einmal.
        e.aenderung(t0 + MS(6000));
        assert!(e.faellig(t0 + MS(6150)));
        assert!(!e.faellig(t0 + MS(6151)));
        // Verworfen: kein Lesen, bis zur naechsten Aenderung.
        e.aenderung(t0 + MS(7000));
        e.verwerfen();
        assert!(!e.wartet());
        assert!(!e.faellig(t0 + MS(8000)));
    }

    /// Der Takt des Waechters wird bis zur Faelligkeit verkuerzt.
    #[test]
    fn entprellen_schlaf() {
        let t0 = Instant::now();
        let mut e = Entpreller::neu(RUHE);
        assert_eq!(e.schlaf(t0, MS(300)), MS(300));
        e.aenderung(t0);
        assert_eq!(e.schlaf(t0 + MS(100), MS(300)), MS(50));
        assert_eq!(e.schlaf(t0 + MS(400), MS(300)), Duration::ZERO);
        assert_eq!(e.schlaf(t0, MS(20)), MS(20));
    }

    /// Dieselbe Liste innerhalb der Frist nur einmal; eine andere Liste,
    /// dieselbe nach der Frist, nach einem Text oder in einer neuen Sitzung
    /// wieder.
    #[test]
    fn entdoppeln_dateilisten() {
        let t0 = Instant::now();
        let a = vec![PathBuf::from("/x/a.txt"), PathBuf::from("/x/b")];
        let b = vec![PathBuf::from("/x/a.txt")];
        let mut d = Entdoppler::neu(DOPPEL_FRIST);
        assert!(d.dateien(&a, t0, 1));
        assert!(!d.dateien(&a, t0 + MS(300), 1), "zweite Meldung derselben Kopie durchgelassen");
        assert!(!d.dateien(&a, t0 + MS(1999), 1));
        // Die Frist laeuft ab der gemeldeten Liste.
        assert!(d.dateien(&a, t0 + MS(2000), 1));
        // Eine andere Liste gilt sofort.
        assert!(d.dateien(&b, t0 + MS(2100), 1));
        assert!(d.dateien(&a, t0 + MS(2200), 1));
        // Nach einem Text ist dieselbe Liste eine neue Kopie.
        d.text();
        assert!(d.dateien(&a, t0 + MS(2300), 1));
        // Neue Sitzung: auch.
        assert!(d.dateien(&a, t0 + MS(2400), 2));
        assert!(!d.dateien(&a, t0 + MS(2500), 2));
    }
}
