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
//
// Verteilen nach Herkunft (Verteiler): Client und Host-Rolle koennen sich
// einen Prozess teilen, die Ablage des Rechners aber gibt es nur einmal -
// also auch nur EINEN Waechter. Jede Rolle meldet sich mit ihrer Herkunft
// an (protokoll::Herkunft), dazu, ob sie gerade ein Gegenueber hat (der
// Client eine Sitzung, die Host-Rolle einen Zuschauer) und in welcher
// Sitzung. Gelesen wird, wenn irgendeine Rolle ein Gegenueber hat; eine
// Kopie des Nutzers geht an jede Rolle, die in diesem Augenblick eines hat,
// entdoppelt je Rolle. Was eine Rolle selbst ablegt (von ihrem Gegenueber),
// meldet der Waechter keiner Rolle - weder zurueck noch weiter: Ablegen
// kennzeichnet den Eintrag, und gekennzeichnete Eintraege liest der
// Waechter nie (clipboard.rs, clipboard_mac.rs). Eine Kette ueber diesen
// Rechner hinweg gibt es damit nicht.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::protokoll::Herkunft;

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

/// Was der Benutzer kopiert hat - fuer beide Waechter dieselbe Art
/// (clipboard.rs, clipboard_mac.rs), damit die Aufrufer ohne
/// Plattformweiche auskommen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inhalt {
    /// Text; unter Windows mit den Zeilenenden, wie Windows sie liefert
    /// (CRLF), auf dem Mac der Text des ersten Eintrags.
    Text(String),
    /// Eine Dateiliste: die obersten Pfade, wie Explorer (CF_HDROP) bzw.
    /// Finder (Dateiverweise) sie ablegen. Ordner werden nicht aufgeloest.
    Dateien(Vec<PathBuf>),
}

impl Inhalt {
    pub fn leer(&self) -> bool {
        match self {
            Inhalt::Text(t) => t.is_empty(),
            Inhalt::Dateien(p) => p.is_empty(),
        }
    }
}

/// Hat die Rolle gerade ein Gegenueber? Some(sitzung) ja - `sitzung`
/// aendert sich mit jedem neuen Gegenueber (dieselbe Dateiliste in einer
/// neuen Sitzung ist eine neue Kopie) -, None nein. Wird im Faden des
/// Waechters gerufen, ohne eine Sperre dieses Moduls.
pub type Gegenueber = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// Nimmt eine Kopie ab; laeuft im Faden des Waechters, ohne eine Sperre
/// dieses Moduls, und darf dort nicht lange arbeiten.
pub type Abnehmer = Arc<dyn Fn(Inhalt) + Send + Sync>;

/// Eine angemeldete Rolle.
struct Platz {
    gegenueber: Gegenueber,
    abnehmer: Abnehmer,
    doppel: Entdoppler,
}

/// Was aus einer Kopie fuer eine Rolle wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verteilt {
    /// Die Rolle hat sie bekommen.
    Gemeldet,
    /// Dieselbe Dateiliste gerade eben schon (Entdoppler) - uebergangen.
    Doppelt,
}

/// Die Abnehmer des einen Waechters, je Herkunft hoechstens einer.
pub struct Verteiler {
    plaetze: Mutex<[Option<Platz>; 2]>,
}

impl Verteiler {
    pub const fn neu() -> Verteiler {
        Verteiler { plaetze: Mutex::new([None, None]) }
    }

    fn sperre(&self) -> std::sync::MutexGuard<'_, [Option<Platz>; 2]> {
        self.plaetze.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Die Rolle `h` nimmt ab jetzt Kopien ab; eine fruehere Anmeldung
    /// derselben Rolle ist damit ersetzt.
    pub fn anmelden(&self, h: Herkunft, gegenueber: Gegenueber, abnehmer: Abnehmer) {
        self.sperre()[h.stelle()] = Some(Platz { gegenueber, abnehmer, doppel: Entdoppler::neu(DOPPEL_FRIST) });
    }

    /// Die angemeldeten Rollen mit ihren Rueckrufen - gerufen wird dann
    /// ausserhalb der Sperre.
    fn angemeldet(&self) -> Vec<(Herkunft, Gegenueber, Abnehmer)> {
        let p = self.sperre();
        [Herkunft::Client, Herkunft::Host]
            .into_iter()
            .filter_map(|h| p[h.stelle()].as_ref().map(|x| (h, x.gegenueber.clone(), x.abnehmer.clone())))
            .collect()
    }

    /// Die Rollen, die jetzt ein Gegenueber haben.
    pub fn mit_gegenueber(&self) -> Vec<Herkunft> {
        self.angemeldet().into_iter().filter(|(_, g, _)| g().is_some()).map(|(h, _, _)| h).collect()
    }

    /// Hat irgendeine Rolle ein Gegenueber? Nur dann liest der Waechter.
    pub fn jemand_da(&self) -> bool {
        self.angemeldet().iter().any(|(_, g, _)| g().is_some())
    }

    /// Eine Kopie des Nutzers an jede Rolle, die jetzt ein Gegenueber hat -
    /// eine Dateiliste nur, wenn sie fuer diese Rolle in dieser Sitzung
    /// nicht gerade eben schon ging (Entdoppler je Rolle); Text immer, er
    /// loest die Liste davor ab. Liefert, was fuer welche Rolle geschah;
    /// Rollen ohne Gegenueber fehlen darin.
    pub fn verteilen(&self, inhalt: &Inhalt, jetzt: Instant) -> Vec<(Herkunft, Verteilt)> {
        let mut aus = Vec::new();
        for (h, gegenueber, abnehmer) in self.angemeldet() {
            let Some(sitzung) = gegenueber() else { continue };
            let neu = {
                let mut p = self.sperre();
                // Inzwischen neu angemeldet: der neue Abnehmer bekommt die
                // Kopie beim naechsten Mal.
                let Some(platz) = p[h.stelle()].as_mut().filter(|x| Arc::ptr_eq(&x.abnehmer, &abnehmer)) else { continue };
                match inhalt {
                    Inhalt::Dateien(pfade) => platz.doppel.dateien(pfade, jetzt, sitzung),
                    Inhalt::Text(_) => {
                        platz.doppel.text();
                        true
                    }
                }
            };
            if neu {
                abnehmer(inhalt.clone());
                aus.push((h, Verteilt::Gemeldet));
            } else {
                aus.push((h, Verteilt::Doppelt));
            }
        }
        aus
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

    /// Ein Abnehmer zum Pruefen: sein Gegenueber (Sitzung oder keins) wird
    /// von aussen gestellt, was er bekommt, sammelt er.
    struct Pruefrolle {
        sitzung: Arc<Mutex<Option<u64>>>,
        bekommen: Arc<Mutex<Vec<Inhalt>>>,
    }

    impl Pruefrolle {
        fn anmelden(v: &Verteiler, h: Herkunft) -> Pruefrolle {
            let r = Pruefrolle { sitzung: Arc::new(Mutex::new(None)), bekommen: Arc::new(Mutex::new(Vec::new())) };
            let (s, b) = (r.sitzung.clone(), r.bekommen.clone());
            v.anmelden(h, Arc::new(move || *s.lock().unwrap()), Arc::new(move |i| b.lock().unwrap().push(i)));
            r
        }

        fn gegenueber(&self, s: Option<u64>) {
            *self.sitzung.lock().unwrap() = s;
        }

        fn nimm(&self) -> Vec<Inhalt> {
            std::mem::take(&mut *self.bekommen.lock().unwrap())
        }
    }

    /// Zwei Rollen an einem Waechter: gelesen wird, sobald eine ein
    /// Gegenueber hat; eine Kopie geht an jede Rolle mit Gegenueber und an
    /// keine ohne. Die Herkunft der Meldungen stimmt.
    #[test]
    fn verteilen_nach_herkunft() {
        let t0 = Instant::now();
        let v = Verteiler::neu();
        let text = Inhalt::Text("kopiert".into());
        // Niemand angemeldet: niemand da, nichts verteilt.
        assert!(!v.jemand_da());
        assert!(v.verteilen(&text, t0).is_empty());

        let client = Pruefrolle::anmelden(&v, Herkunft::Client);
        let host = Pruefrolle::anmelden(&v, Herkunft::Host);
        assert!(!v.jemand_da(), "ohne Gegenueber wird nicht gelesen");
        assert!(v.verteilen(&text, t0).is_empty());
        assert!(client.nimm().is_empty() && host.nimm().is_empty());

        // Nur die Host-Rolle hat einen Zuschauer.
        host.gegenueber(Some(1));
        assert!(v.jemand_da());
        assert_eq!(v.mit_gegenueber(), vec![Herkunft::Host]);
        assert_eq!(v.verteilen(&text, t0), vec![(Herkunft::Host, Verteilt::Gemeldet)]);
        assert_eq!(host.nimm(), vec![text.clone()]);
        assert!(client.nimm().is_empty(), "Client ohne Sitzung hat die Kopie bekommen");

        // Beide haben ein Gegenueber: beide bekommen sie.
        client.gegenueber(Some(7));
        assert_eq!(v.mit_gegenueber(), vec![Herkunft::Client, Herkunft::Host]);
        let t2 = Inhalt::Text("zweite".into());
        assert_eq!(
            v.verteilen(&t2, t0),
            vec![(Herkunft::Client, Verteilt::Gemeldet), (Herkunft::Host, Verteilt::Gemeldet)]
        );
        assert_eq!(client.nimm(), vec![t2.clone()]);
        assert_eq!(host.nimm(), vec![t2]);

        // Nur noch der Client.
        host.gegenueber(None);
        assert_eq!(v.mit_gegenueber(), vec![Herkunft::Client]);
        assert_eq!(v.verteilen(&text, t0), vec![(Herkunft::Client, Verteilt::Gemeldet)]);
        assert!(host.nimm().is_empty());
        assert_eq!(client.nimm(), vec![text]);
    }

    /// Entdoppelt wird je Rolle, mit ihrer eigenen Sitzung: dieselbe Liste
    /// gleich noch einmal geht an keine Rolle, die sie eben bekam - an eine
    /// Rolle, die sie noch nicht hatte (ihr Gegenueber kam eben dazu), schon;
    /// ein neues Gegenueber (neue Sitzung) bekommt sie ebenso.
    #[test]
    fn entdoppeln_je_rolle() {
        let t0 = Instant::now();
        let v = Verteiler::neu();
        let client = Pruefrolle::anmelden(&v, Herkunft::Client);
        let host = Pruefrolle::anmelden(&v, Herkunft::Host);
        let liste = Inhalt::Dateien(vec![PathBuf::from("/x/a.txt")]);
        client.gegenueber(Some(1));
        assert_eq!(v.verteilen(&liste, t0), vec![(Herkunft::Client, Verteilt::Gemeldet)]);
        host.gegenueber(Some(4));
        assert_eq!(
            v.verteilen(&liste, t0 + MS(300)),
            vec![(Herkunft::Client, Verteilt::Doppelt), (Herkunft::Host, Verteilt::Gemeldet)]
        );
        assert_eq!(client.nimm().len(), 1);
        assert_eq!(host.nimm().len(), 1);
        // Neuer Zuschauer an der Host-Rolle: neue Sitzung, neue Kopie.
        host.gegenueber(Some(5));
        assert_eq!(
            v.verteilen(&liste, t0 + MS(600)),
            vec![(Herkunft::Client, Verteilt::Doppelt), (Herkunft::Host, Verteilt::Gemeldet)]
        );
        // Nach der Frist fuer beide wieder.
        assert_eq!(
            v.verteilen(&liste, t0 + MS(2700)),
            vec![(Herkunft::Client, Verteilt::Gemeldet), (Herkunft::Host, Verteilt::Gemeldet)]
        );
    }

    /// Eine zweite Anmeldung derselben Rolle ersetzt die erste; die andere
    /// Rolle bleibt, wie sie war. Die Rueckrufe laufen ohne die Sperre des
    /// Verteilers: ein Abnehmer darf ihn selbst befragen.
    #[test]
    fn anmelden_ersetzt_und_ruft_ohne_sperre() {
        let t0 = Instant::now();
        let v = Arc::new(Verteiler::neu());
        let alt = Pruefrolle::anmelden(&v, Herkunft::Client);
        alt.gegenueber(Some(1));
        let host = Pruefrolle::anmelden(&v, Herkunft::Host);
        host.gegenueber(Some(1));
        let gefragt = Arc::new(Mutex::new(Vec::new()));
        let (v2, g2) = (v.clone(), gefragt.clone());
        v.anmelden(
            Herkunft::Client,
            Arc::new(|| Some(2)),
            Arc::new(move |i| {
                // Im Abnehmer den Verteiler fragen - ginge unter der Sperre nicht.
                g2.lock().unwrap().push((i, v2.mit_gegenueber()));
            }),
        );
        let text = Inhalt::Text("neu".into());
        assert_eq!(
            v.verteilen(&text, t0),
            vec![(Herkunft::Client, Verteilt::Gemeldet), (Herkunft::Host, Verteilt::Gemeldet)]
        );
        assert!(alt.nimm().is_empty(), "ersetzte Anmeldung bekam die Kopie");
        assert_eq!(host.nimm(), vec![text.clone()]);
        assert_eq!(*gefragt.lock().unwrap(), vec![(text, vec![Herkunft::Client, Herkunft::Host])]);
    }

    #[test]
    fn inhalt_leer() {
        assert!(Inhalt::Text(String::new()).leer());
        assert!(Inhalt::Dateien(Vec::new()).leer());
        assert!(!Inhalt::Text("a".into()).leer());
        assert!(!Inhalt::Dateien(vec![PathBuf::from("/a")]).leer());
    }
}
