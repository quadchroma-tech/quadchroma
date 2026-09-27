// Das Menue am Symbol der einen App (Windows): Client und Host-Rolle in
// einem Prozess, ein Symbol im Infobereich (Plan W5). Reine Rechnung - aus
// einem MenueStand werden Eintraege (tray::Eintrag) und eine Zuordnung der
// Nummern; ohne Fenster und ohne Shell testbar, auch auf dem Mac.
//
// Die eine App (Art::App), bei jedem Oeffnen frisch gebaut:
//   QuadChroma – <Geraetename>        nur Anzeige
//   <Zustand>                         "Bereit fuer Verbindungen", "Verbunden:
//                                     <Name>", "Port 9001 ist ... belegt",
//                                     "Freigabe ist aus" - nur Anzeige
//   QuadChroma oeffnen                fett: die Vorgabe (auch Linksklick)
//   Verbinden: <Host> ...             bis zu vier gefundene Hosts
//   Geraete-ID: 581 729 911           Klick kopiert die 9 Ziffern
//   Passwort: k7m-4wq-9tz             Klick kopiert (verdeckt)
//   Passwort aendern ... / Neues Zufallspasswort
//   Erlaubte Geraete >                je Geraet "<Name> - ID ... - seit ..."
//                                     > Entfernen; "Alle Geraete entfernen ..."
//   (beschaedigte Liste: "Geraeteliste beschaedigt" + "... zuruecksetzen")
//   Geraetename aendern ...
//   [x] Diesen PC freigeben           Freigabe an/aus (einstellungen.txt)
//   [x] Mit Windows starten           eine Verknuepfung im Autostart-Ordner
//   [ ] Ruhezustand verhindern, solange QuadChroma laeuft
//   Beenden                           mit Abschied an einen Zuschauer
//
// Die reine Host-Rolle (Art::NurHost, --nur-host fuer VM und Tests) zeigt
// nur Kopf, Zustand, ID, Passwort, Geraete und "Freigabe beenden" - wie die
// Host-Rolle vor der einen App.
//
// Die Geraete-ID und die Zeilen dazu stehen erst da, wenn der Einlass der
// Host-Rolle steht (id Some); kam sie nicht in Gang, sagt es die
// Zustandszeile.

// Auf dem Mac baut die Host-Engine dasselbe Menue in Objective-C
// (host/menue.m, qc_menue_modell; geprueft von host/menuetest.m) - dort
// braucht es von hier nur `Aktion` und `tooltip`; die Tests laufen trotzdem
// auf beiden Plattformen.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::strings::{Key, Lang};
use crate::tray::{self, Eintrag};
use crate::zugang::{self, Geraet};

// Befehlsnummern; gefundene Hosts ab NR_HOST, Geraete ab NR_GERAET, je plus
// ihre Stelle in der Zuordnung.
const NR_ID: u32 = 1;
const NR_PASSWORT: u32 = 2;
const NR_AENDERN: u32 = 3;
const NR_ZUFALL: u32 = 4;
const NR_ALLE: u32 = 5;
const NR_ZURUECKSETZEN: u32 = 6;
const NR_AUTOSTART: u32 = 7;
const NR_FREIGABE_BEENDEN: u32 = 8;
const NR_OEFFNEN: u32 = 9;
const NR_NAME: u32 = 10;
const NR_FREIGABE: u32 = 11;
const NR_RUHE: u32 = 12;
const NR_BEENDEN: u32 = 13;
const NR_HOST: u32 = 100;
const NR_GERAET: u32 = 1000;

/// Welche App das Menue traegt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    /// Die eine App: Client mit Fenster und Host-Rolle.
    App,
    /// Nur die Host-Rolle, ohne Fenster (--nur-host).
    NurHost,
}

/// Der Zustand der Freigabe, wie ihn die Zustandszeile nennt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freigabe {
    /// Ausgeschaltet (Menue oder Startbildschirm).
    Aus,
    /// An, niemand verbunden.
    Bereit,
    /// An, ein Zuschauer ist verbunden (sein Name).
    Verbunden(String),
    /// An, aber der Port liess sich nicht binden (neuer Versuch alle 5 s).
    PortBelegt(u16),
    /// An, aber die Host-Rolle kam nicht in Gang - die Zustandszeile sagt,
    /// woran es lag (Einzelheiten im Protokoll).
    Fehler(Startfehler),
}

/// Woran die Host-Rolle scheiterte (Zustandszeile am Symbol).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startfehler {
    /// host.key oder der Ablageordner taugt nicht (Code 5).
    Schluessel,
    /// Ein Schalter der Befehlszeile taugt nicht (Code 6: --konserve, 7:
    /// --encoderweg). Aus- und wieder einschalten versucht es neu.
    Schalter(&'static str),
    /// Sonst etwas (FFmpeg, kein Faden) - Einzelheiten in host-protokoll.txt.
    Sonst,
}

impl Startfehler {
    /// Aus dem Code der Host-Rolle (host::Dienst, Exit-Codes der reinen
    /// Host-Rolle).
    pub fn aus_code(code: u8) -> Startfehler {
        match code {
            5 => Startfehler::Schluessel,
            6 => Startfehler::Schalter("--konserve"),
            7 => Startfehler::Schalter("--encoderweg"),
            _ => Startfehler::Sonst,
        }
    }
}

impl Freigabe {
    /// Ist die Freigabe eingeschaltet (Haken bei "Diesen PC freigeben")?
    pub fn an(&self) -> bool {
        !matches!(self, Freigabe::Aus)
    }
}

/// Was die Host-Rolle zum Menue beitraegt (host::menue_teil).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostTeil {
    pub freigabe: Freigabe,
    /// Geraete-ID der Host-Rolle; None, solange ihr Einlass nicht steht.
    pub id: Option<u32>,
    /// Das Zugangspasswort; Err: Datei unlesbar.
    pub passwort: Result<String, ()>,
    /// Die erlaubten Geraete; Err: Liste unlesbar oder beschaedigt.
    pub geraete: Result<Vec<Geraet>, ()>,
}

/// Was das Menue zeigt, als Daten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenueStand {
    pub art: Art,
    /// Der Geraetename (zugang::geraetename).
    pub name: String,
    pub freigabe: Freigabe,
    /// Geraete-ID der Host-Rolle; None, solange ihr Einlass nicht steht.
    pub id: Option<u32>,
    /// Das Zugangspasswort; Err: Datei unlesbar.
    pub passwort: Result<String, ()>,
    /// Die erlaubten Geraete; Err: Liste unlesbar oder beschaedigt.
    pub geraete: Result<Vec<Geraet>, ()>,
    /// Gefundene Hosts: (Name, Adresse), in der Reihenfolge der Liste.
    pub hosts: Vec<(String, String)>,
    pub autostart: bool,
    /// Haken "Ruhezustand verhindern": was gilt (nicht der Wunsch).
    pub ruhe_verhindern: bool,
    /// Gewuenscht, aber vom System abgelehnt: sein Fehlercode (darunter
    /// steht eine Zeile PreventSleepRefused).
    pub ruhe_abgelehnt: Option<String>,
}

/// Was ein Menuepunkt ausloest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aktion {
    /// Das Fenster zeigen (auch Linksklick und Doppelklick).
    Oeffnen,
    /// Fenster zeigen und mit dieser Adresse verbinden.
    Verbinden(String),
    IdKopieren,
    PasswortKopieren,
    PasswortAendern,
    Zufallspasswort,
    Entfernen([u8; 32]),
    AlleEntfernen,
    ListeZuruecksetzen,
    /// Das Fenster "Geraetename" oeffnen.
    NameAendern,
    /// "Diesen PC freigeben" umschalten.
    Freigabe,
    /// "Mit Windows starten" umschalten.
    Autostart,
    /// "Ruhezustand verhindern" umschalten.
    RuheVerhindern,
    /// Nur die reine Host-Rolle: sie endet (Abschied Grund 1).
    FreigabeBeenden,
    /// Die App beenden (Abschied Grund 0).
    Beenden,
}

/// Wozu die Nummern der Hosts und Geraete in genau diesem Menue gehoeren.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zuordnung {
    hosts: Vec<String>,
    geraete: Vec<[u8; 32]>,
}

/// Platzhalter einer Vorlage in EINEM Durchgang ersetzen: eingesetzte Werte
/// werden nicht noch einmal durchsucht. Mit hintereinander gehaengten
/// .replace() ersetzte der zweite auch ein "{i}" oder "{d}", das in einem
/// (unbeglaubigten) Geraetenamen steht.
pub fn einsetzen(vorlage: &str, werte: &[(&str, &str)]) -> String {
    let mut aus = String::with_capacity(vorlage.len() + 32);
    let mut rest = vorlage;
    'aussen: while !rest.is_empty() {
        for (platz, wert) in werte {
            if let Some(r) = rest.strip_prefix(platz) {
                aus.push_str(wert);
                rest = r;
                continue 'aussen;
            }
        }
        let c = rest.chars().next().expect("rest ist nicht leer");
        aus.push(c);
        rest = &rest[c.len_utf8()..];
    }
    aus
}

/// Die Zustandszeile.
pub fn zustand(lang: &Lang, f: &Freigabe) -> String {
    match f {
        Freigabe::Aus => lang.get(Key::HostSharingIsOff).to_string(),
        Freigabe::Bereit => lang.get(Key::HostReady).to_string(),
        Freigabe::Verbunden(n) => einsetzen(lang.get(Key::HostConnected), &[("{n}", &tray::anzeigename(n))]),
        Freigabe::PortBelegt(p) => lang.get(Key::HostPortBusy).replace("{p}", &p.to_string()),
        Freigabe::Fehler(Startfehler::Schluessel) => lang.get(Key::ShareFailedKey).to_string(),
        Freigabe::Fehler(Startfehler::Schalter(s)) => lang.get(Key::ShareFailedSwitch).replace("{s}", s),
        Freigabe::Fehler(Startfehler::Sonst) => lang.get(Key::MsgShareFailed).to_string(),
    }
}

/// Der Tooltip: in einer Sitzung "QuadChroma – <Host>"; sonst mit
/// eingeschalteter Freigabe "QuadChroma – Freigabe laeuft (ID ...)", ohne
/// sie "QuadChroma".
pub fn tooltip(lang: &Lang, sitzung: Option<&str>, freigabe_an: bool, id: Option<u32>) -> String {
    let n = sitzung.map(tray::anzeigename).unwrap_or_default();
    if !n.is_empty() {
        return tray::tooltip(Some(&n));
    }
    match id {
        Some(id) if freigabe_an => lang.get(Key::HostTooltip).replace("{i}", &zugang::id_text(id)),
        _ => tray::tooltip(None),
    }
}

/// Die Eintraege des Menues und die Zuordnung seiner Nummern (fuer
/// `aktion_zu`).
pub fn menue(lang: &Lang, s: &MenueStand) -> (Vec<Eintrag>, Zuordnung) {
    let mut m = Vec::new();
    let mut z = Zuordnung::default();
    let app = s.art == Art::App;
    m.push(Eintrag::anzeige(format!("QuadChroma – {}", tray::anzeigename(&s.name))));
    m.push(Eintrag::anzeige(zustand(lang, &s.freigabe)));
    m.push(Eintrag::Trenner);
    if app {
        m.push(Eintrag::Punkt { text: lang.get(Key::TrayOpenApp).to_string(), nummer: NR_OEFFNEN, haken: false, aktiv: true, fett: true });
        for (name, adresse) in s.hosts.iter().take(tray::HOSTS_MAX) {
            let n = tray::anzeigename(name);
            let n = if n.is_empty() { adresse.clone() } else { n };
            m.push(Eintrag::punkt(einsetzen(lang.get(Key::TrayConnect), &[("{n}", &n)]), NR_HOST + z.hosts.len() as u32));
            z.hosts.push(adresse.clone());
        }
        m.push(Eintrag::Trenner);
    }
    if let Some(id) = s.id {
        m.push(Eintrag::punkt(lang.get(Key::HostDeviceId).replace("{i}", &zugang::id_text(id)), NR_ID));
        match &s.passwort {
            Ok(pw) => m.push(Eintrag::punkt(einsetzen(lang.get(Key::HostPassword), &[("{p}", pw)]), NR_PASSWORT)),
            Err(()) => m.push(Eintrag::anzeige(lang.get(Key::HostPasswordUnreadable))),
        }
        m.push(Eintrag::punkt(lang.get(Key::HostChangePassword), NR_AENDERN));
        m.push(Eintrag::punkt(lang.get(Key::HostRandomPassword), NR_ZUFALL));
        m.push(Eintrag::Trenner);
        match &s.geraete {
            Ok(liste) => {
                let mut unter = Vec::new();
                if liste.is_empty() {
                    unter.push(Eintrag::anzeige(lang.get(Key::HostNoDevices)));
                }
                for g in liste {
                    let id = zugang::id_text(g.id());
                    let zeile = einsetzen(lang.get(Key::HostDeviceLine), &[("{n}", &g.name), ("{i}", &id), ("{d}", &g.datum)]);
                    unter.push(Eintrag::Unter {
                        text: zeile,
                        eintraege: vec![Eintrag::punkt(lang.get(Key::HostRemove), NR_GERAET + z.geraete.len() as u32)],
                        aktiv: true,
                    });
                    z.geraete.push(g.schluessel);
                }
                if !liste.is_empty() {
                    unter.push(Eintrag::Trenner);
                    unter.push(Eintrag::punkt(lang.get(Key::HostRemoveAll), NR_ALLE));
                }
                m.push(Eintrag::Unter { text: lang.get(Key::HostDevices).to_string(), eintraege: unter, aktiv: true });
            }
            Err(()) => {
                m.push(Eintrag::anzeige(lang.get(Key::HostListDamaged)));
                m.push(Eintrag::punkt(lang.get(Key::HostListReset), NR_ZURUECKSETZEN));
            }
        }
        m.push(Eintrag::Trenner);
    }
    if app {
        m.push(Eintrag::punkt(lang.get(Key::DeviceNameChange), NR_NAME));
        m.push(Eintrag::schalter(lang.get(Key::StartShare), NR_FREIGABE, s.freigabe.an()));
        m.push(Eintrag::schalter(lang.get(Key::HostStartWindows), NR_AUTOSTART, s.autostart));
        m.push(Eintrag::schalter(lang.get(Key::PreventSleep), NR_RUHE, s.ruhe_verhindern));
        if let Some(c) = &s.ruhe_abgelehnt {
            m.push(Eintrag::anzeige(lang.get(Key::PreventSleepRefused).replace("{c}", c)));
        }
        m.push(Eintrag::Trenner);
        m.push(Eintrag::punkt(lang.get(Key::TrayQuit), NR_BEENDEN));
    } else {
        m.push(Eintrag::punkt(lang.get(Key::HostStopSharing), NR_FREIGABE_BEENDEN));
    }
    (m, z)
}

/// Die Aktion zur gewaehlten Nummer, aufgeloest gegen die Zuordnung des
/// Menues, aus dem gewaehlt wurde. 0 (nichts gewaehlt) und fremde Nummern
/// ergeben nichts.
pub fn aktion_zu(nr: u32, z: &Zuordnung) -> Option<Aktion> {
    Some(match nr {
        NR_ID => Aktion::IdKopieren,
        NR_PASSWORT => Aktion::PasswortKopieren,
        NR_AENDERN => Aktion::PasswortAendern,
        NR_ZUFALL => Aktion::Zufallspasswort,
        NR_ALLE => Aktion::AlleEntfernen,
        NR_ZURUECKSETZEN => Aktion::ListeZuruecksetzen,
        NR_AUTOSTART => Aktion::Autostart,
        NR_FREIGABE_BEENDEN => Aktion::FreigabeBeenden,
        NR_OEFFNEN => Aktion::Oeffnen,
        NR_NAME => Aktion::NameAendern,
        NR_FREIGABE => Aktion::Freigabe,
        NR_RUHE => Aktion::RuheVerhindern,
        NR_BEENDEN => Aktion::Beenden,
        n if n >= NR_GERAET => Aktion::Entfernen(*z.geraete.get((n - NR_GERAET) as usize)?),
        n if n >= NR_HOST => Aktion::Verbinden(z.hosts.get((n - NR_HOST) as usize)?.clone()),
        _ => return None,
    })
}

/// Die Nummer, die ein Linksklick (und Eingabe/Leertaste, Doppelklick) auf
/// das Symbol der einen App ausloest: "QuadChroma oeffnen".
pub const LINKSKLICK: u32 = NR_OEFFNEN;

#[cfg(test)]
mod tests {
    use super::*;

    fn geraet(n: u8, name: &str) -> Geraet {
        Geraet { schluessel: [n; 32], datum: "2026-09-26".into(), name: name.into() }
    }

    fn stand(art: Art) -> MenueStand {
        MenueStand {
            art,
            name: "Büro-PC".into(),
            freigabe: Freigabe::Bereit,
            id: Some(581_729_911),
            passwort: Ok("k7m-4wq-9tz".into()),
            geraete: Ok(vec![geraet(1, "Laptop"), geraet(2, "Wohnzimmer")]),
            hosts: Vec::new(),
            autostart: true,
            ruhe_verhindern: false,
            ruhe_abgelehnt: None,
        }
    }

    fn texte(e: &[Eintrag]) -> Vec<String> {
        e.iter()
            .map(|x| match x {
                Eintrag::Punkt { text, .. } | Eintrag::Unter { text, .. } => text.clone(),
                Eintrag::Trenner => "-".into(),
            })
            .collect()
    }

    /// Stelle des Eintrags mit diesem Text.
    fn stelle(m: &[Eintrag], t: &str) -> usize {
        texte(m).iter().position(|x| x == t).unwrap_or_else(|| panic!("{t} fehlt in {:?}", texte(m)))
    }

    fn nummer(e: &Eintrag) -> u32 {
        match e {
            Eintrag::Punkt { nummer, .. } => *nummer,
            e => panic!("kein Punkt: {e:?}"),
        }
    }

    fn haken(e: &Eintrag) -> bool {
        matches!(e, Eintrag::Punkt { haken: true, .. })
    }

    /// Die eine App, Freigabe an, niemand verbunden, keine Hosts: das ganze
    /// Menue auf Deutsch; "QuadChroma oeffnen" ist die Vorgabe, Kopf und
    /// Zustand nur Anzeige, die drei Schalter mit ihren Haken.
    #[test]
    fn app_bereit() {
        let de = crate::strings::pick("de");
        let (m, z) = menue(de, &stand(Art::App));
        assert_eq!(
            texte(&m),
            [
                "QuadChroma – Büro-PC",
                "Bereit für Verbindungen",
                "-",
                "QuadChroma öffnen",
                "-",
                "Geräte-ID: 581 729 911",
                "Passwort: k7m-4wq-9tz",
                "Passwort ändern …",
                "Neues Zufallspasswort",
                "-",
                "Erlaubte Geräte",
                "-",
                "Gerätename ändern …",
                "Diesen PC freigeben",
                "Mit Windows starten",
                "Ruhezustand verhindern, solange QuadChroma läuft",
                "-",
                "Beenden",
            ]
        );
        assert!(matches!(&m[0], Eintrag::Punkt { aktiv: false, nummer: 0, .. }));
        assert!(matches!(&m[1], Eintrag::Punkt { aktiv: false, nummer: 0, .. }));
        assert!(matches!(&m[3], Eintrag::Punkt { fett: true, aktiv: true, .. }));
        assert_eq!(aktion_zu(nummer(&m[3]), &z), Some(Aktion::Oeffnen));
        assert_eq!(nummer(&m[3]), LINKSKLICK);
        assert!(haken(&m[13]) && haken(&m[14]) && !haken(&m[15]));
        let erwartet = [
            (5, Aktion::IdKopieren),
            (6, Aktion::PasswortKopieren),
            (7, Aktion::PasswortAendern),
            (8, Aktion::Zufallspasswort),
            (12, Aktion::NameAendern),
            (13, Aktion::Freigabe),
            (14, Aktion::Autostart),
            (15, Aktion::RuheVerhindern),
            (17, Aktion::Beenden),
        ];
        for (i, a) in erwartet {
            assert_eq!(aktion_zu(nummer(&m[i]), &z), Some(a), "Stelle {i}");
        }
        // Die Geraete mit Untermenue "Entfernen" auf ihren Schluessel.
        let Eintrag::Unter { eintraege, .. } = &m[10] else { panic!("{:?}", m[10]) };
        let id = |n: u8| zugang::id_text(zugang::geraete_id(&[n; 32]));
        assert_eq!(
            texte(eintraege),
            [
                format!("Laptop – ID {} – seit 2026-09-26", id(1)),
                format!("Wohnzimmer – ID {} – seit 2026-09-26", id(2)),
                "-".to_string(),
                "Alle Geräte entfernen …".to_string(),
            ]
        );
        for (i, e) in eintraege[..2].iter().enumerate() {
            let Eintrag::Unter { eintraege: u, .. } = e else { panic!() };
            assert_eq!(texte(u), ["Entfernen"]);
            assert_eq!(aktion_zu(nummer(&u[0]), &z), Some(Aktion::Entfernen([i as u8 + 1; 32])));
        }
        assert_eq!(aktion_zu(nummer(&eintraege[3]), &z), Some(Aktion::AlleEntfernen));
    }

    /// Freigabe aus: Zustand "Freigabe ist aus", kein Haken bei "Diesen PC
    /// freigeben" - ID, Passwort und Geraete bleiben zu sehen (man kann sie
    /// auch ohne Freigabe verwalten). Ruhezustand verhindern mit Haken.
    #[test]
    fn app_freigabe_aus() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::App);
        s.freigabe = Freigabe::Aus;
        s.ruhe_verhindern = true;
        s.autostart = false;
        let (m, z) = menue(de, &s);
        assert_eq!(texte(&m)[1], "Freigabe ist aus");
        let f = stelle(&m, "Diesen PC freigeben");
        assert!(!haken(&m[f]));
        assert_eq!(aktion_zu(nummer(&m[f]), &z), Some(Aktion::Freigabe));
        assert!(!haken(&m[stelle(&m, "Mit Windows starten")]));
        assert!(haken(&m[stelle(&m, "Ruhezustand verhindern, solange QuadChroma läuft")]));
        assert!(!texte(&m).iter().any(|t| t.starts_with("Nicht aktiv")), "{:?}", texte(&m));
        // Lehnt das System ab: kein Haken, darunter der Grund mit dem Code.
        let mut abgelehnt = s.clone();
        abgelehnt.ruhe_verhindern = false;
        abgelehnt.ruhe_abgelehnt = Some("0x80070005".into());
        let (ma, _) = menue(de, &abgelehnt);
        let r = stelle(&ma, "Ruhezustand verhindern, solange QuadChroma läuft");
        assert!(!haken(&ma[r]));
        assert_eq!(texte(&ma)[r + 1], "Nicht aktiv: vom System abgelehnt (0x80070005)");
        assert!(matches!(ma[r + 1], Eintrag::Punkt { aktiv: false, .. }));
        stelle(&m, "Geräte-ID: 581 729 911");
        let en = crate::strings::pick("en");
        assert_eq!(texte(&menue(en, &s).0)[1], "Sharing is off");
    }

    /// Port belegt: der Zustand nennt den Port, die Freigabe bleibt an
    /// (neuer Versuch alle 5 s). Kam die Host-Rolle gar nicht in Gang (kein
    /// Einlass), stehen weder ID noch Passwort noch Geraete da.
    #[test]
    fn app_port_belegt_und_fehler() {
        let de = crate::strings::pick("de");
        let en = crate::strings::pick("en");
        let mut s = stand(Art::App);
        s.freigabe = Freigabe::PortBelegt(9001);
        assert_eq!(texte(&menue(de, &s).0)[1], "Port 9001 ist von einem anderen Programm belegt");
        assert_eq!(texte(&menue(en, &s).0)[1], "Port 9001 is used by another program");
        assert!(haken(&menue(de, &s).0[stelle(&menue(de, &s).0, "Diesen PC freigeben")]));
        s.freigabe = Freigabe::Fehler(Startfehler::Sonst);
        s.id = None;
        assert_eq!(texte(&menue(de, &s).0)[1], "Die Freigabe dieses PCs konnte nicht gestartet werden.");
        // Die Zustandszeile sagt, woran es lag.
        s.freigabe = Freigabe::Fehler(Startfehler::aus_code(5));
        assert_eq!(texte(&menue(en, &s).0)[1], "Sharing could not start: host.key or its folder cannot be used.");
        s.freigabe = Freigabe::Fehler(Startfehler::aus_code(6));
        assert_eq!(texte(&menue(en, &s).0)[1], "Sharing could not start: --konserve is not usable. Turn sharing off and on to retry.");
        s.freigabe = Freigabe::Fehler(Startfehler::aus_code(7));
        assert!(texte(&menue(de, &s).0)[1].starts_with("Die Freigabe konnte nicht starten: --encoderweg ist nicht nutzbar."));
        assert_eq!(Startfehler::aus_code(1), Startfehler::Sonst);
        s.freigabe = Freigabe::Fehler(Startfehler::Sonst);
        let (m, z) = menue(de, &s);
        assert!(!texte(&m).iter().any(|t| t.starts_with("Geräte-ID") || t.starts_with("Passwort") || t == "Erlaubte Geräte"), "{:?}", texte(&m));
        assert!(haken(&m[stelle(&m, "Diesen PC freigeben")]));
        assert_eq!(z, Zuordnung::default());
    }

    /// Beschaedigte Geraeteliste und unlesbares Passwort: je eine Anzeige,
    /// dazu "Geraeteliste zuruecksetzen"; ohne Geraete "Noch keine Geraete".
    #[test]
    fn app_liste_beschaedigt() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::App);
        s.geraete = Err(());
        s.passwort = Err(());
        let (m, z) = menue(de, &s);
        let p = stelle(&m, "Passwortdatei unlesbar – neue Geräte nur über „Zulassen“.");
        assert!(matches!(&m[p], Eintrag::Punkt { aktiv: false, .. }));
        let d = stelle(&m, "Geräteliste beschädigt");
        assert!(matches!(&m[d], Eintrag::Punkt { aktiv: false, .. }));
        assert_eq!(texte(&m)[d + 1], "Geräteliste zurücksetzen");
        assert_eq!(aktion_zu(nummer(&m[d + 1]), &z), Some(Aktion::ListeZuruecksetzen));
        s.geraete = Ok(Vec::new());
        let (m, z) = menue(de, &s);
        let Eintrag::Unter { eintraege, .. } = &m[stelle(&m, "Erlaubte Geräte")] else { panic!() };
        assert_eq!(texte(eintraege), ["Noch keine Geräte"]);
        assert!(matches!(&eintraege[0], Eintrag::Punkt { aktiv: false, .. }));
        assert!(z.geraete.is_empty());
    }

    /// Ein Zuschauer ist verbunden: sein Name in der Zustandszeile (fremde
    /// Hand: entschaerft und gekuerzt), und der Tooltip bleibt beim Host.
    #[test]
    fn app_zuschauer_verbunden() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::App);
        s.freigabe = Freigabe::Verbunden("Laptop\tvon {n}".into());
        assert_eq!(texte(&menue(de, &s).0)[1], "Verbunden: Laptop von {n}");
        s.freigabe = Freigabe::Verbunden("y".repeat(100));
        let t = texte(&menue(de, &s).0)[1].clone();
        assert!(t.ends_with('…') && t.chars().count() <= "Verbunden: ".chars().count() + tray::NAME_MAX, "{t}");
    }

    /// Gefundene Hosts: hoechstens vier, in der Reihenfolge der Liste, ohne
    /// Namen die Adresse; jede Nummer fuehrt zu genau ihrer Adresse.
    #[test]
    fn app_mit_hosts() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::App);
        s.hosts = vec![
            ("Mac-mini-von-Robert.local".into(), "192.168.178.194:9001".into()),
            ("".into(), "192.168.178.60:9001".into()),
            ("böse\tname".into(), "10.0.0.3:9001".into()),
            ("d".into(), "10.0.0.4:9101".into()),
            ("e".into(), "10.0.0.5:9001".into()),
        ];
        let (m, z) = menue(de, &s);
        let t = texte(&m);
        assert_eq!(
            &t[3..9],
            [
                "QuadChroma öffnen",
                "Verbinden: Mac-mini-von-Robert.local",
                "Verbinden: 192.168.178.60:9001",
                "Verbinden: böse name",
                "Verbinden: d",
                "-",
            ]
        );
        assert!(!t.iter().any(|x| x == "Verbinden: e"));
        for (i, adresse) in [(4, "192.168.178.194:9001"), (5, "192.168.178.60:9001"), (7, "10.0.0.4:9101")] {
            assert_eq!(aktion_zu(nummer(&m[i]), &z), Some(Aktion::Verbinden(adresse.into())));
        }
        for l in crate::strings::all() {
            let (m, _) = menue(l, &s);
            assert!(texte(&m)[4].contains("Mac-mini-von-Robert.local"), "{}: {:?}", l.code, texte(&m)[4]);
            assert!(texte(&m)[3].contains("QuadChroma"), "{}", l.code);
        }
    }

    /// Die reine Host-Rolle (--nur-host): ohne Fenster, Hosts, Name,
    /// Schalter und Beenden der App - am Ende "Freigabe beenden".
    #[test]
    fn nur_host() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::NurHost);
        s.hosts = vec![("Mac".into(), "10.0.0.1:9001".into())];
        let (m, z) = menue(de, &s);
        assert_eq!(
            texte(&m),
            [
                "QuadChroma – Büro-PC",
                "Bereit für Verbindungen",
                "-",
                "Geräte-ID: 581 729 911",
                "Passwort: k7m-4wq-9tz",
                "Passwort ändern …",
                "Neues Zufallspasswort",
                "-",
                "Erlaubte Geräte",
                "-",
                "Freigabe beenden",
            ]
        );
        assert_eq!(aktion_zu(nummer(&m[10]), &z), Some(Aktion::FreigabeBeenden));
        assert!(z.hosts.is_empty());
    }

    /// Nummern, die zu keinem Punkt gehoeren, ergeben nichts - auch eine
    /// Host- oder Geraetenummer hinter dem Ende der Zuordnung.
    #[test]
    fn fremde_nummern() {
        let z = Zuordnung { hosts: vec!["h:1".into()], geraete: vec![[9u8; 32]] };
        assert_eq!(aktion_zu(0, &z), None);
        assert_eq!(aktion_zu(99, &z), None);
        assert_eq!(aktion_zu(NR_HOST, &z), Some(Aktion::Verbinden("h:1".into())));
        assert_eq!(aktion_zu(NR_HOST + 1, &z), None);
        assert_eq!(aktion_zu(NR_GERAET, &z), Some(Aktion::Entfernen([9u8; 32])));
        assert_eq!(aktion_zu(NR_GERAET + 1, &z), None);
        // Alle festen Nummern sind verschieden und unter NR_HOST.
        let fest = [
            NR_ID, NR_PASSWORT, NR_AENDERN, NR_ZUFALL, NR_ALLE, NR_ZURUECKSETZEN, NR_AUTOSTART, NR_FREIGABE_BEENDEN, NR_OEFFNEN,
            NR_NAME, NR_FREIGABE, NR_RUHE, NR_BEENDEN,
        ];
        let verschieden: std::collections::HashSet<_> = fest.iter().collect();
        assert_eq!(verschieden.len(), fest.len());
        assert!(fest.iter().all(|&n| n > 0 && n < NR_HOST));
    }

    /// Tooltip: in einer Sitzung der Host, sonst mit Freigabe die eigene ID,
    /// ohne sie nur "QuadChroma".
    #[test]
    fn tooltip_je_zustand() {
        let de = crate::strings::pick("de");
        assert_eq!(tooltip(de, None, true, Some(581_729_911)), "QuadChroma – Freigabe läuft (ID 581 729 911)");
        assert_eq!(tooltip(de, None, false, Some(581_729_911)), "QuadChroma");
        assert_eq!(tooltip(de, None, true, None), "QuadChroma");
        assert_eq!(tooltip(de, Some("studio.local"), true, Some(5)), "QuadChroma – studio.local");
        assert_eq!(tooltip(de, Some("  "), true, Some(5)), "QuadChroma – Freigabe läuft (ID 000 000 005)");
    }

    /// Platzhalter in einem Durchgang: ein "{i}" oder "{d}" im Namen bleibt,
    /// wie es ist - im Menue wie in der Vorlage.
    #[test]
    fn einsetzen_in_einem_durchgang() {
        let de = crate::strings::pick("de");
        let mut s = stand(Art::App);
        s.geraete = Ok(vec![geraet(1, "Falle {i} {d} {n}")]);
        let (m, _) = menue(de, &s);
        let Eintrag::Unter { eintraege, .. } = &m[stelle(&m, "Erlaubte Geräte")] else { panic!() };
        let id = zugang::id_text(zugang::geraete_id(&[1u8; 32]));
        assert_eq!(texte(eintraege)[0], format!("Falle {{i}} {{d}} {{n}} – ID {id} – seit 2026-09-26"));
        assert_eq!(einsetzen("{a}{b}{a}x", &[("{a}", "{b}"), ("{b}", "1")]), "{b}1{b}x");
        assert_eq!(einsetzen("ohne", &[("{n}", "x")]), "ohne");
        assert_eq!(einsetzen("Grüße {n}!", &[("{n}", "Ä")]), "Grüße Ä!");
        assert_eq!(einsetzen("{n", &[("{n}", "x")]), "{n");
    }
}
