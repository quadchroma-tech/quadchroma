// Zugangsphase auf der Client-Seite (Spezifikation Pairing v1, 3.5 und 8).
//
// Nach dem Handschlag des Bildkanals sagt der Host, ob er dieses Geraet
// kennt: "QCH1" (MAGIC) - die Sitzung beginnt wie bisher; "QCA1"
// (MAGIC_ZUGANG) - es folgt die Zugangsphase mit den Nachrichten 20-23
// (Kodieren und Lesen in zugang.rs). Der Client liest die 4 Byte mit Frist
// (KENNUNG_FRIST); kommt nichts, etwas Fremdes, oder schliesst der Host
// (aeltere Fassungen schliessen bei einem unbekannten Geraet), endet der
// Versuch mit einer Meldung (main.rs, Spezifikation 9.6).
//
// Die Zugangsphase selbst ist ein reiner Automat (`Automat`): er bekommt
// Nachrichten des Hosts, Eingaben des Nutzers und die Zeit und sagt, was
// hinausgeht und wie es endet - ohne Leitung und ohne Uhr pruefbar.
// `fuehren` treibt ihn an einer Leitung: alle TAKT fragt es nach dem Nutzer
// (Dialog im Fenster bzw. --passwort im Pruefmodus) und schaut an der
// Leitung, ob etwas anliegt (Secure::bereit, nur hineinschauen). Ein eigener
// Lesefaden ist so nicht noetig, und der Host kann jederzeit "Zulassen"
// melden, waehrend der Nutzer noch tippt.
//
// Regeln (3.5): Der Beweis geht nur hinaus, wenn der Nutzer ein Passwort
// eingibt - nie von selbst - und erst nach der Wartezeit der Drossel.
// Ergebnis 0 gilt nur mit richtigem host_proof; ohne ihn (oder ohne eigenen
// Beweis davor) bricht der Client ab und pinnt nichts. Gepinnt wird erst
// nach "QCH1" (8.1) - mit `pinnen`, durch den Aufrufer.
//
// Dazu, was der Client vor dem Verbinden weiss (`Vorwissen`): hosts.txt,
// VOR jeder Leitung gelesen, die Pruefung der gewaehlten ID nach Nachricht 2
// (8.2), die Flags fuer Nachricht 3 und die Frage, ob an einer bekannten
// Adresse ein neues Geraet antwortet (8.3 - kein Dauerfehler mehr, sondern
// ein Hinweis im Dialog).
//
// Erstkontakt (1.4, 3.5): Kennt der Client den Schluessel des Hosts nicht,
// setzt er Bit 0 in Nachricht 3 ("weise dich aus"). Ein Host, der ihn
// kennt, fuehrt dann trotzdem die Zugangsphase - und beweist mit 22/0 und
// host_proof, dass er das Passwort kennt (oder jemand klickt dort
// "Zulassen", 22/1 mit dem Vergleichscode auf beiden Seiten). Sagt er
// stattdessen gleich "QCH1", hat er sich nicht ausgewiesen: der Client
// bricht ab und pinnt nichts (main.rs, MsgHostUnverified).

use crate::protokoll_konst::{MAGIC, MAGIC_ZUGANG, NAME_FLAG_HOST_UNBEKANNT};
use crate::zugang::{self, BekannterHost, DateiFehler, Ergebnis, Hostliste, Nachricht, ZugangNoetig};
use std::path::Path;
use std::time::{Duration, Instant};

/// So lange wartet der Client nach dem Handschlag auf "QCH1" bzw. "QCA1" -
/// und nach der Annahme auf "QCH1". Ein Host, der dann noch schweigt,
/// antwortet nicht (MsgNoAnswer).
#[cfg(not(test))]
pub const KENNUNG_FRIST: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const KENNUNG_FRIST: Duration = Duration::from_millis(1500);

/// Laengste Stille in der Zugangsphase, bevor der Client aufgibt: die
/// Gesamtfrist des Hosts (120 s, danach sendet er Ergebnis 4) und etwas
/// Luft. Wartet der Nutzer auf "Zulassen", kommt so lange nichts. In
/// Tests kuerzer - aber nicht zu kurz: in einem Testbau ohne Optimierung
/// braucht PBKDF2 (100 000 Runden) je Seite gut 1,5 s, und die Stille zaehlt
/// ab der letzten Nachricht des Hosts, also samt Wartezeit der Drossel und
/// beiden Rechnungen fuer einen Beweis (bei 3 s scheiterten die
/// Passworttests dann je nach Last).
#[cfg(not(test))]
pub const RUHE_FRIST: Duration = Duration::from_secs(zugang::PHASE_FRIST.as_secs() + 10);
#[cfg(test)]
pub const RUHE_FRIST: Duration = Duration::from_secs(10);

/// Takt der Zugangsphase: so oft wird nach dem Nutzer gesehen.
pub const TAKT: Duration = Duration::from_millis(50);

/// Was der Host nach dem Handschlag als erstes schickt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kennung {
    /// "QCH1": bekannt, die Sitzung beginnt.
    Sitzung,
    /// "QCA1": unbekannt, es folgt die Zugangsphase.
    Zugang,
    /// Etwas anderes - kein QuadChroma-Host dieser Art.
    Fremd([u8; 4]),
}

pub fn kennung(b: &[u8; 4]) -> Kennung {
    if b == MAGIC {
        Kennung::Sitzung
    } else if b == MAGIC_ZUGANG {
        Kennung::Zugang
    } else {
        Kennung::Fremd(*b)
    }
}

/// Wo der Dialog gerade steht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lage {
    /// Wartet auf ein Passwort (oder auf "Zulassen" am Host).
    Eingabe,
    /// Ein Beweis ist unterwegs (oder wartet auf das Ende der Drossel).
    Pruefen,
    /// Das letzte Passwort war falsch; ein neues darf kommen.
    Falsch,
}

/// Was der Zugangsdialog zeigt (Spezifikation 9.3). Der Empfangsfaden legt
/// es in `Shared`, die Oberflaeche zeichnet daraus.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    /// Name des Hosts: aus Nachricht 20, sonst Bekanntgabe oder Adresse.
    pub name: String,
    /// ID des Hosts (aus seinem Schluessel).
    pub id: u32,
    /// Vergleichscode des Handschlags ("628 306").
    pub code: String,
    /// Am Host kann jemand "Zulassen" klicken (Bit 1 in Nachricht 20).
    pub zulassen: bool,
    /// An dieser Adresse war ein anderes Geraet gepinnt (8.3).
    pub neue_identitaet: bool,
    /// Dieser Client kennt den Host, doch der fragt nach Zugang, und hier
    /// liegt noch ein frueherer client.key mit anderem Schluessel: der Host
    /// kannte dieses Geraet wohl unter dem alten - der Dialog sagt dazu, dass
    /// es jetzt einen Schluessel je Rechner gibt (secure::GERAETESCHLUESSEL).
    pub schluessel_gewechselt: bool,
    pub lage: Lage,
    /// Vorher geht kein Beweis hinaus (Drossel des Hosts); None: sofort.
    pub frei_ab: Option<Instant>,
    /// Zaehlt die Antworten "falsch" - die Oberflaeche leert daran das Feld.
    pub runde: u32,
}

impl Dialog {
    /// Sekunden bis zum naechsten erlaubten Versuch (aufgerundet); 0: jetzt.
    pub fn warten_s(&self, jetzt: Instant) -> u64 {
        match self.frei_ab {
            Some(t) if t > jetzt => {
                let rest = t - jetzt;
                rest.as_secs() + u64::from(rest.subsec_nanos() > 0)
            }
            _ => 0,
        }
    }

    /// Darf der Nutzer jetzt ein Passwort schicken?
    pub fn darf_senden(&self, jetzt: Instant) -> bool {
        self.lage != Lage::Pruefen && self.warten_s(jetzt) == 0
    }
}

/// Was der Nutzer im Dialog tut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Eingabe {
    Passwort(String),
    Abbrechen,
}

/// Wie die Zugangsphase endet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ausgang {
    /// Angenommen (per Passwort mit richtigem host_proof, oder per Klick auf
    /// "Zulassen") - jetzt muss "QCH1" folgen, dann wird gepinnt.
    Angenommen { per_passwort: bool },
    /// Ergebnis 3: am Host abgelehnt.
    Abgelehnt,
    /// Ergebnis 4: zu viele Versuche, Frist abgelaufen oder kein Platz.
    /// `fehlversuche`: wie oft der Host in dieser Phase "falsch" sagte.
    Schluss { warten_ms: u32, fehlversuche: u32 },
    /// Ergebnis 0, aber der host_proof stimmt nicht (oder es gab keinen
    /// eigenen Beweis): moeglicher Angriff - abbrechen, nicht pinnen.
    HostBeweisFalsch,
    /// Der Nutzer hat abgebrochen; Nachricht 23 ist hinaus.
    Abgebrochen,
    /// Der Host schwieg laenger als RUHE_FRIST.
    KeineAntwort,
    /// Der Host hat die Leitung sauber geschlossen (EOF), bevor er seine
    /// Kennung sagte - so antworten aeltere Fassungen einem unbekannten
    /// Geraet (siehe `kennung_lesen`).
    Geschlossen,
    /// Die Leitung fiel zu oder scheiterte (Wortlaut).
    Leitung(String),
    /// Eine Nachricht passte nicht (Wortlaut, deutsch).
    Protokoll(String),
}

/// Was ein Schritt des Automaten bewirkt: Bytes fuer die Leitung und/oder
/// das Ende der Phase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schritt {
    pub senden: Option<Vec<u8>>,
    pub ende: Option<Ausgang>,
}

impl Schritt {
    fn nichts() -> Schritt {
        Schritt::default()
    }
    fn ende(a: Ausgang) -> Schritt {
        Schritt { senden: None, ende: Some(a) }
    }
}

/// Der Automat der Zugangsphase eines Clients (eine Verbindung).
pub struct Automat {
    dialog: Dialog,
    host_pub: Vec<u8>,
    hh: Vec<u8>,
    /// K des zuletzt gesendeten Beweises - fuer den host_proof aus Ergebnis 0.
    k: Option<[u8; 32]>,
    /// Ein Passwort, das auf das Ende der Drossel wartet.
    zurueck: Option<String>,
    fehlversuche: u32,
}

impl Automat {
    /// Nachricht 20 ist da. `ersatzname`: Name, falls 20 keinen traegt
    /// (Bekanntgabe oder Adresse); `code` der Vergleichscode des Handschlags.
    pub fn neu(
        noetig: &ZugangNoetig,
        host_pub: &[u8],
        hh: &[u8],
        ersatzname: &str,
        code: &str,
        neue_identitaet: bool,
        jetzt: Instant,
    ) -> Automat {
        let name = if noetig.hostname.is_empty() { zugang::name_bereinigen(ersatzname) } else { noetig.hostname.clone() };
        let frei_ab = (noetig.warten_ms > 0).then(|| jetzt + Duration::from_millis(noetig.warten_ms as u64));
        Automat {
            dialog: Dialog {
                name,
                id: zugang::geraete_id(host_pub),
                code: code.to_string(),
                zulassen: noetig.zulassen_moeglich(),
                neue_identitaet,
                schluessel_gewechselt: false,
                lage: Lage::Eingabe,
                frei_ab,
                runde: 0,
            },
            host_pub: host_pub.to_vec(),
            hh: hh.to_vec(),
            k: None,
            zurueck: None,
            fehlversuche: 0,
        }
    }

    pub fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// Mit dem Hinweis auf den einen Geraeteschluessel (Dialog::
    /// schluessel_gewechselt).
    pub fn mit_schluesselhinweis(mut self, ja: bool) -> Automat {
        self.dialog.schluessel_gewechselt = ja;
        self
    }

    /// Den Beweis fuer dieses Passwort bauen (PBKDF2 - dauert spuerbar,
    /// deshalb nie im Fensterfaden) und als Nachricht 21 hinausgeben.
    fn beweisen(&mut self, pw: &str) -> Schritt {
        let k = zugang::passwort_schluessel(pw, &self.host_pub);
        let beweis = zugang::client_beweis(&k, &self.hh);
        self.k = Some(k);
        self.dialog.lage = Lage::Pruefen;
        Schritt { senden: Some(Nachricht::Beweis(beweis).kodieren()), ende: None }
    }

    /// Eine Eingabe des Nutzers.
    pub fn eingabe(&mut self, e: Eingabe, jetzt: Instant) -> Schritt {
        match e {
            Eingabe::Abbrechen => Schritt { senden: Some(Nachricht::Abbruch.kodieren()), ende: Some(Ausgang::Abgebrochen) },
            // Einer ist schon unterwegs (die Oberflaeche sperrt den Knopf
            // dann ohnehin), oder es ist nichts eingegeben.
            Eingabe::Passwort(_) if self.dialog.lage == Lage::Pruefen => Schritt::nichts(),
            Eingabe::Passwort(pw) if pw.is_empty() => Schritt::nichts(),
            // Vor dem Ende der Drossel zaehlte ein Beweis beim Host als
            // Fehlversuch: zurueckhalten, bis `takt` ihn freigibt.
            Eingabe::Passwort(pw) if self.dialog.frei_ab.is_some_and(|t| jetzt < t) => {
                self.zurueck = Some(pw);
                self.dialog.lage = Lage::Pruefen;
                Schritt::nichts()
            }
            Eingabe::Passwort(pw) => self.beweisen(&pw),
        }
    }

    /// Die Zeit vergeht: ein zurueckgehaltenes Passwort geht hinaus, sobald
    /// die Drossel es erlaubt.
    pub fn takt(&mut self, jetzt: Instant) -> Schritt {
        if self.dialog.frei_ab.is_some_and(|t| jetzt < t) {
            return Schritt::nichts();
        }
        match self.zurueck.take() {
            Some(pw) => self.beweisen(&pw),
            None => Schritt::nichts(),
        }
    }

    /// Eine Nachricht des Hosts.
    pub fn nachricht(&mut self, n: Nachricht, jetzt: Instant) -> Schritt {
        let e = match n {
            Nachricht::Ergebnis(e) => e,
            andere => {
                return Schritt::ende(Ausgang::Protokoll(format!(
                    "Zugangsphase: Nachricht {} kommt vom Host nicht in Frage",
                    andere.typ()
                )))
            }
        };
        match e {
            Ergebnis::Passwort { host_beweis } => {
                // Nur mit eigenem Beweis und passendem host_proof - sonst
                // koennte jeder "angenommen" sagen, und der Client pinnte ihn.
                let echt = self.k.is_some_and(|k| zugang::host_beweis_pruefen(&k, &self.hh, &host_beweis));
                Schritt::ende(if echt { Ausgang::Angenommen { per_passwort: true } } else { Ausgang::HostBeweisFalsch })
            }
            // "Zulassen" gibt es nur, wenn der Host es in Nachricht 20
            // angeboten hat (Bit 1) - ein echter Host stellt die Anfrage
            // sonst gar nicht. Ohne diese Pruefung koennte ein falscher Host
            // den Passwortweg umgehen: kein Zulassen anbieten (der Dialog
            // fragt nur nach dem Passwort), den Beweis nehmen und statt
            // 22/0 mit host_proof einfach 22/1 sagen - der Client pinnte ihn,
            // ohne dass etwas bewiesen waere. Ging schon ein Beweis hinaus,
            // ist das der Fall "Host konnte das Passwort nicht bestaetigen".
            Ergebnis::Zulassen if !self.dialog.zulassen => Schritt::ende(if self.k.is_some() || self.fehlversuche > 0 {
                Ausgang::HostBeweisFalsch
            } else {
                Ausgang::Protokoll("Zugangsphase: Ergebnis 1 (zugelassen), obwohl Nachricht 20 kein Zulassen anbot".into())
            }),
            Ergebnis::Zulassen => Schritt::ende(Ausgang::Angenommen { per_passwort: false }),
            Ergebnis::Falsch { warten_ms } => {
                self.fehlversuche += 1;
                self.k = None;
                self.dialog.lage = Lage::Falsch;
                self.dialog.runde += 1;
                self.dialog.frei_ab = (warten_ms > 0).then(|| jetzt + Duration::from_millis(warten_ms as u64));
                Schritt::nichts()
            }
            Ergebnis::Abgelehnt => Schritt::ende(Ausgang::Abgelehnt),
            Ergebnis::Schluss { warten_ms } => {
                Schritt::ende(Ausgang::Schluss { warten_ms, fehlversuche: self.fehlversuche })
            }
        }
    }
}

/// Die Leitung der Zugangsphase - im Betrieb die gesicherte Bildleitung,
/// in Tests ein Drehbuch.
pub trait Leitung {
    fn senden(&mut self, b: &[u8]) -> Result<(), String>;
    /// Liegt binnen `warten` etwas zum Lesen an? Err: Leitung zu/gestoert.
    fn bereit(&mut self, warten: Duration) -> std::io::Result<bool>;
    fn lesen(&mut self, b: &mut [u8]) -> std::io::Result<()>;
}

impl Leitung for crate::secure::Secure {
    fn senden(&mut self, b: &[u8]) -> Result<(), String> {
        self.write_all(b)
    }
    fn bereit(&mut self, warten: Duration) -> std::io::Result<bool> {
        crate::secure::Secure::bereit(self, warten)
    }
    fn lesen(&mut self, b: &mut [u8]) -> std::io::Result<()> {
        crate::secure::Secure::lesen(self, b)
    }
}

/// Liest die ersten 4 Byte nach dem Handschlag (bzw. nach der Annahme).
/// Err(Ausgang::KeineAntwort): Frist abgelaufen; Err(Ausgang::Geschlossen):
/// der Host hat sauber zugemacht (EOF - so antwortet eine aeltere Fassung
/// einem unbekannten Geraet); Err(Ausgang::Leitung): Leitung gestoert
/// (zurueckgesetzt, Datensatz nicht echt ...). Die Frist setzt der Aufrufer
/// an der Leitung.
pub fn kennung_lesen(l: &mut impl Leitung) -> Result<Kennung, Ausgang> {
    use std::io::ErrorKind as E;
    let mut b = [0u8; 4];
    match l.lesen(&mut b) {
        Ok(()) => Ok(kennung(&b)),
        Err(e) if matches!(e.kind(), E::WouldBlock | E::TimedOut) => Err(Ausgang::KeineAntwort),
        Err(e) if e.kind() == E::UnexpectedEof => Err(Ausgang::Geschlossen),
        Err(e) => Err(Ausgang::Leitung(e.to_string())),
    }
}

/// Liest Nachricht 20 - die erste nach "QCA1". Hat der Host keinen Platz
/// frei (zu viele Zugangsphasen, Spezifikation 3.2), kommt statt 20 gleich
/// Ergebnis 4 mit Wartezeit, dann macht er zu (Mac- und Windows-Host:
/// "QCA1", 22/4) - das endet wie jedes Ergebnis 4 ("zu viele Versuche", kein
/// Neuversuch), nicht als Protokollfehler. Ebenso eine sofortige Ablehnung
/// (22/3). Eine Annahme ohne Nachricht 20 gibt es nicht.
pub fn noetig_lesen(l: &mut impl Leitung) -> Result<ZugangNoetig, Ausgang> {
    match zugang::empfangen(|b| l.lesen(b)) {
        Ok(Nachricht::Noetig(n)) => Ok(n),
        Ok(Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms })) => Err(Ausgang::Schluss { warten_ms, fehlversuche: 0 }),
        Ok(Nachricht::Ergebnis(Ergebnis::Abgelehnt)) => Err(Ausgang::Abgelehnt),
        Ok(andere) => Err(Ausgang::Protokoll(format!("Zugangsphase: Nachricht {} statt 20", andere.typ()))),
        Err(zugang::LeseFehler::Leitung(g)) => Err(Ausgang::Leitung(g)),
        Err(e) => Err(Ausgang::Protokoll(e.to_string())),
    }
}

/// Treibt den Automaten an der Leitung, bis die Phase endet. `bedienung`
/// bekommt je TAKT den Stand des Dialogs (zum Anzeigen) und liefert die
/// Eingabe des Nutzers, falls eine vorliegt. `ruhe`: laengste Stille des
/// Hosts (RUHE_FRIST). Nachrichten liest es mit der Frist, die an der
/// Leitung gilt (KENNUNG_FRIST, vom Aufrufer gesetzt).
pub fn fuehren(
    l: &mut impl Leitung,
    automat: &mut Automat,
    mut bedienung: impl FnMut(&Dialog) -> Option<Eingabe>,
    ruhe: Duration,
) -> Ausgang {
    let mut zuletzt = Instant::now();
    loop {
        let mut schritte = Vec::with_capacity(2);
        if let Some(e) = bedienung(automat.dialog()) {
            schritte.push(automat.eingabe(e, Instant::now()));
        }
        schritte.push(automat.takt(Instant::now()));
        for s in schritte {
            if let Some(b) = s.senden {
                if let Err(e) = l.senden(&b) {
                    // Der Abbruch ist beim Host angekommen oder nicht - die
                    // Leitung ist so oder so zu, der Nutzer wollte gehen.
                    return if s.ende == Some(Ausgang::Abgebrochen) { Ausgang::Abgebrochen } else { Ausgang::Leitung(e) };
                }
            }
            if let Some(a) = s.ende {
                return a;
            }
        }
        match l.bereit(TAKT) {
            Ok(true) => {
                let n = match zugang::empfangen(|b| l.lesen(b)) {
                    Ok(n) => n,
                    Err(zugang::LeseFehler::Leitung(g)) => return Ausgang::Leitung(g),
                    Err(e) => return Ausgang::Protokoll(e.to_string()),
                };
                zuletzt = Instant::now();
                let s = automat.nachricht(n, zuletzt);
                if let Some(a) = s.ende {
                    return a;
                }
            }
            Ok(false) if zuletzt.elapsed() >= ruhe => return Ausgang::KeineAntwort,
            Ok(false) => {}
            Err(e) => return Ausgang::Leitung(e.to_string()),
        }
    }
}

// ------------------------------------------------------------ hosts.txt

/// Was der Client vor dem Verbinden ueber das Ziel weiss.
pub struct Vorwissen {
    /// Die bekannten Hosts, VOR der Leitung gelesen.
    pub liste: Hostliste,
    /// Die Adresse, unter der verbunden wird ("ip:port").
    pub adresse: String,
    /// Verbunden ueber eine ID (Liste, Eingabe, Verknuepfung): diese.
    pub erwartet: Option<u32>,
    /// Der Schluessel des Hosts, der dieses Geraet seit der Wahl durch den
    /// Nutzer schon angenommen hat (Shared::angenommen) - er gilt beim
    /// Wiederverbinden als gepinnt, auch wenn hosts.txt ihn nicht (mehr)
    /// fuehrt (Schreiben scheiterte, Datei geloescht). Sonst saehe der Host
    /// Bit 0, verlangte die Zugangsphase, und der Client zoege sie zurueck
    /// (keine Anfrage ohne den Nutzer).
    pub angenommen: Option<Vec<u8>>,
}

impl Vorwissen {
    /// hosts.txt lesen. Unlesbar oder kein UTF-8: Err - dann wird nicht
    /// verbunden (wie known_hosts.txt bisher).
    pub fn laden(pfad: &Path, adresse: &str, erwartet: Option<u32>) -> Result<Vorwissen, DateiFehler> {
        Ok(Vorwissen { liste: Hostliste::laden(pfad)?, adresse: adresse.to_string(), erwartet, angenommen: None })
    }

    /// Die Flags fuer Nachricht 3 (1.4), nach Nachricht 2: Bit 0
    /// (NAME_FLAG_HOST_UNBEKANNT), wenn dieser Schluessel nicht gepinnt ist -
    /// weder in hosts.txt noch als der Host, der dieses Geraet eben schon
    /// angenommen hat. Dann muss der Host sich in der Zugangsphase ausweisen,
    /// auch wenn er dieses Geraet kennt.
    pub fn flags3(&self, peer: &[u8]) -> u8 {
        if self.bekannt(peer).is_some() || self.angenommen.as_deref() == Some(peer) {
            0
        } else {
            NAME_FLAG_HOST_UNBEKANNT
        }
    }

    /// Im Handschlag nach Nachricht 2 (8.2): beim Verbinden ueber eine ID
    /// muss der Schluessel genau diese ID ergeben. Sonst Abbruch, bevor
    /// Nachricht 3 den eigenen Schluessel zeigt.
    ///
    /// Die ID ist nur ein Suchschluessel (30 Bit; 1.2: "vertraut wird immer
    /// dem vollen Schluessel") - einen anderen Schluessel mit derselben ID
    /// kann man sich errechnen. Steht zu der gewaehlten ID schon ein
    /// Schluessel in hosts.txt, muss es deshalb einer der gemerkten sein;
    /// ein fremder mit derselben ID ist ebenso "ein anderes Geraet". Ein
    /// echter Host mit neuem Schluessel hat auch eine neue ID (bis auf einen
    /// Zufall von 1 zu 10^9) und faellt schon am ersten Vergleich.
    pub fn pruefen(&self, peer: &[u8]) -> Result<(), crate::secure::Fehler> {
        let Some(id) = self.erwartet else { return Ok(()) };
        let gemeldet = zugang::geraete_id(peer);
        let fremd_mit_id = || self.bekannt(peer).is_none() && self.liste.hosts.iter().any(|h| h.id == id);
        if gemeldet != id || fremd_mit_id() {
            return Err(crate::secure::Fehler::AnderesGeraet { addr: self.adresse.clone(), erwartet: id, gemeldet });
        }
        Ok(())
    }

    /// Der gepinnte Eintrag zu diesem Schluessel, egal unter welcher Adresse.
    pub fn bekannt(&self, peer: &[u8]) -> Option<&BekannterHost> {
        self.liste.nach_schluessel(peer)
    }

    /// 8.3: unter dieser Adresse war ein anderes Geraet gepinnt, und dieser
    /// Schluessel ist nirgends gepinnt - kein Dauerfehler mehr, der Dialog
    /// sagt es dazu.
    pub fn neue_identitaet(&self, peer: &[u8]) -> bool {
        self.bekannt(peer).is_none() && self.liste.nach_adresse(&self.adresse).is_some_and(|h| h.schluessel[..] != *peer)
    }
}

/// Pinnt den Host NACH der Annahme (8.1): ID, Schluessel, Adresse und Name
/// in hosts.txt; ein Eintrag mit demselben Schluessel wird ersetzt, einer
/// mit anderem Schluessel unter derselben Adresse bleibt fuer seine ID
/// stehen (8.3). Ok(false): der Eintrag stand schon genau so vorn - dann
/// wird nichts geschrieben.
pub fn pinnen(pfad: &Path, peer: &[u8], adresse: &str, name: &str) -> Result<bool, DateiFehler> {
    let schluessel: [u8; 32] = peer.try_into().map_err(|_| DateiFehler::Schreiben {
        pfad: pfad.to_path_buf(),
        grund: format!("Schluessel mit {} statt 32 Byte", peer.len()),
    })?;
    let neu = BekannterHost::neu(schluessel, adresse, name);
    if Hostliste::laden(pfad)?.hosts.first() == Some(&neu) {
        return Ok(false);
    }
    zugang::host_merken(pfad, neu)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Pruefvektoren der Spezifikation 3.4: host_pub32 = 0x01..0x20,
    /// hh = 32 x 0xAA, Passwort "k7m-4wq-9tz".
    fn host_pub() -> [u8; 32] {
        std::array::from_fn(|i| i as u8 + 1)
    }
    const HH: [u8; 32] = [0xaa; 32];

    fn aus_hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn client_proof() -> Vec<u8> {
        aus_hex("647398d505263b3af01ba1a04d74aeb467a774588902489bca97c8d3544e3fc0")
    }

    fn host_proof() -> [u8; 32] {
        aus_hex("2f9d511fa97017f1d5b60806aa5ff4d781766e386b92f79d77391e4090cef55d").try_into().unwrap()
    }

    fn automat(zulassen: bool, warten_ms: u32, jetzt: Instant) -> Automat {
        let n = ZugangNoetig::neu(zulassen, warten_ms, "Roberts Mac mini");
        Automat::neu(&n, &host_pub(), &HH, "10.0.0.5:9001", "628 306", false, jetzt)
    }

    fn beweis_bytes(b: &[u8]) -> Vec<u8> {
        let mut v = vec![crate::protokoll_konst::ZUGANG_BEWEIS, 0, 0, 0, 32, 0, 0, 0];
        v.extend_from_slice(b);
        v
    }

    #[test]
    fn kennungen() {
        assert_eq!(kennung(b"QCH1"), Kennung::Sitzung);
        assert_eq!(kennung(b"QCA1"), Kennung::Zugang);
        assert_eq!(kennung(b"QCX1"), Kennung::Fremd(*b"QCX1"));
    }

    /// Das Passwort des Pruefvektors, wie eingetippt (Grossbuchstaben,
    /// Leerzeichen): hinaus geht genau client_proof; Ergebnis 0 mit dem
    /// host_proof des Vektors heisst angenommen.
    #[test]
    fn passwort_nach_pruefvektor() {
        let t = Instant::now();
        let mut a = automat(true, 0, t);
        let d = a.dialog().clone();
        assert_eq!((d.name.as_str(), d.id, d.code.as_str(), d.zulassen, d.lage), ("Roberts Mac mini", 581_729_911, "628 306", true, Lage::Eingabe));
        assert!(d.darf_senden(t));
        let s = a.eingabe(Eingabe::Passwort("K7M 4WQ-9TZ".into()), t);
        assert_eq!(s.senden, Some(beweis_bytes(&client_proof())));
        assert_eq!(s.ende, None);
        assert_eq!(a.dialog().lage, Lage::Pruefen);
        assert!(!a.dialog().darf_senden(t));
        // Ein zweites Passwort, waehrend einer unterwegs ist: nichts.
        assert_eq!(a.eingabe(Eingabe::Passwort("x".into()), t), Schritt::default());
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Passwort { host_beweis: host_proof() }), t);
        assert_eq!(s.ende, Some(Ausgang::Angenommen { per_passwort: true }));
    }

    /// Ergebnis 0 mit falschem host_proof - oder ganz ohne eigenen Beweis -
    /// ist kein "angenommen": moeglicher Angriff, nicht pinnen.
    #[test]
    fn host_beweis_falsch() {
        let t = Instant::now();
        let mut a = automat(false, 0, t);
        a.eingabe(Eingabe::Passwort("k7m-4wq-9tz".into()), t);
        let mut falsch = host_proof();
        falsch[31] ^= 1;
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Passwort { host_beweis: falsch }), t);
        assert_eq!(s.ende, Some(Ausgang::HostBeweisFalsch));
        let mut a = automat(true, 0, t);
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Passwort { host_beweis: host_proof() }), t);
        assert_eq!(s.ende, Some(Ausgang::HostBeweisFalsch));
    }

    /// Falsch mit Wartezeit: der Dialog zeigt "falsch" und zaehlt herunter;
    /// ein Passwort davor wird zurueckgehalten und geht erst mit dem Ende
    /// der Drossel hinaus - nie zu frueh (das zaehlte als Fehlversuch).
    #[test]
    fn falsch_mit_drossel() {
        let t = Instant::now();
        let mut a = automat(false, 0, t);
        a.eingabe(Eingabe::Passwort("erstes".into()), t);
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms: 5000 }), t);
        assert_eq!(s, Schritt::default());
        let d = a.dialog().clone();
        assert_eq!((d.lage, d.runde), (Lage::Falsch, 1));
        assert_eq!(d.warten_s(t), 5);
        assert_eq!(d.warten_s(t + Duration::from_millis(4001)), 1);
        assert_eq!(d.warten_s(t + Duration::from_secs(5)), 0);
        assert!(!d.darf_senden(t + Duration::from_secs(4)));
        assert!(d.darf_senden(t + Duration::from_secs(5)));
        // Zu frueh eingegeben: zurueckgehalten.
        let s = a.eingabe(Eingabe::Passwort("k7m-4wq-9tz".into()), t + Duration::from_secs(1));
        assert_eq!(s, Schritt::default());
        assert_eq!(a.dialog().lage, Lage::Pruefen);
        assert_eq!(a.takt(t + Duration::from_millis(4999)), Schritt::default());
        let s = a.takt(t + Duration::from_secs(5));
        assert_eq!(s.senden, Some(beweis_bytes(&client_proof())));
        assert_eq!(a.takt(t + Duration::from_secs(6)), Schritt::default());
        // Falsch ohne Wartezeit: sofort wieder frei.
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms: 0 }), t + Duration::from_secs(6));
        assert_eq!(s, Schritt::default());
        assert_eq!((a.dialog().runde, a.dialog().frei_ab), (2, None));
        let s = a.nachricht(Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 60_000 }), t);
        assert_eq!(s.ende, Some(Ausgang::Schluss { warten_ms: 60_000, fehlversuche: 2 }));
    }

    /// Nachricht 20 mit Wartezeit (Drossel schon beim Beginn).
    #[test]
    fn wartezeit_schon_in_nachricht_20() {
        let t = Instant::now();
        let a = automat(true, 2500, t);
        assert_eq!(a.dialog().warten_s(t), 3);
        assert_eq!(a.dialog().lage, Lage::Eingabe);
        assert!(!a.dialog().darf_senden(t));
    }

    #[test]
    fn zulassen_ablehnen_abbrechen() {
        let t = Instant::now();
        let mut a = automat(true, 0, t);
        assert_eq!(a.nachricht(Nachricht::Ergebnis(Ergebnis::Zulassen), t).ende, Some(Ausgang::Angenommen { per_passwort: false }));
        let mut a = automat(true, 0, t);
        assert_eq!(a.nachricht(Nachricht::Ergebnis(Ergebnis::Abgelehnt), t).ende, Some(Ausgang::Abgelehnt));
        let mut a = automat(true, 0, t);
        let s = a.eingabe(Eingabe::Abbrechen, t);
        assert_eq!(s.senden, Some(vec![crate::protokoll_konst::ZUGANG_ABBRUCH, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(s.ende, Some(Ausgang::Abgebrochen));
        // Leere Eingabe: nichts.
        let mut a = automat(true, 0, t);
        assert_eq!(a.eingabe(Eingabe::Passwort(String::new()), t), Schritt::default());
        assert_eq!(a.dialog().lage, Lage::Eingabe);
        // Nachrichten, die nur ein Client sendet (oder 20 ein zweites Mal).
        for n in [Nachricht::Beweis([0; 32]), Nachricht::Abbruch, Nachricht::Noetig(ZugangNoetig::neu(false, 0, "x"))] {
            let mut a = automat(true, 0, t);
            assert!(matches!(a.nachricht(n, t).ende, Some(Ausgang::Protokoll(_))));
        }
    }

    /// 22/1 gilt nur, wenn Nachricht 20 "Zulassen" anbot (Bit 1). Sonst ist
    /// es kein "angenommen": ohne eigenen Beweis ein Protokollfehler, nach
    /// einem Beweis (auch einem, den der Host "falsch" nannte) der Fall
    /// "Host konnte das Passwort nicht bestaetigen" - nichts wird gepinnt.
    /// Mit Bit 1 darf "Zulassen" auch kommen, waehrend ein Beweis unterwegs
    /// ist (am Host wurde geklickt, bevor er den Beweis pruefte).
    #[test]
    fn zulassen_nur_wenn_angeboten() {
        let t = Instant::now();
        let zugelassen = || Nachricht::Ergebnis(Ergebnis::Zulassen);
        let mut a = automat(false, 0, t);
        assert!(matches!(a.nachricht(zugelassen(), t).ende, Some(Ausgang::Protokoll(_))));
        let mut a = automat(false, 0, t);
        assert!(a.eingabe(Eingabe::Passwort("k7m-4wq-9tz".into()), t).senden.is_some());
        assert_eq!(a.nachricht(zugelassen(), t).ende, Some(Ausgang::HostBeweisFalsch));
        let mut a = automat(false, 0, t);
        a.eingabe(Eingabe::Passwort("falsch".into()), t);
        assert_eq!(a.nachricht(Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms: 0 }), t), Schritt::default());
        assert_eq!(a.nachricht(zugelassen(), t).ende, Some(Ausgang::HostBeweisFalsch));
        let mut a = automat(true, 0, t);
        a.eingabe(Eingabe::Passwort("k7m-4wq-9tz".into()), t);
        assert_eq!(a.nachricht(zugelassen(), t).ende, Some(Ausgang::Angenommen { per_passwort: false }));
    }

    /// Ohne Namen in Nachricht 20 steht der Ersatzname (Adresse) da.
    #[test]
    fn name_aus_20_oder_ersatz() {
        let n = ZugangNoetig::neu(false, 0, "");
        let a = Automat::neu(&n, &host_pub(), &HH, "10.0.0.5:9001", "1", true, Instant::now());
        assert_eq!(a.dialog().name, "10.0.0.5:9001");
        assert!(a.dialog().neue_identitaet);
        // Der Hinweis auf den einen Geraeteschluessel ist aus, bis ihn der
        // Aufrufer setzt; er aendert sonst nichts am Dialog.
        assert!(!a.dialog().schluessel_gewechselt);
        let vorher = a.dialog().clone();
        let a = a.mit_schluesselhinweis(true);
        assert!(a.dialog().schluessel_gewechselt);
        assert_eq!(Dialog { schluessel_gewechselt: false, ..a.dialog().clone() }, vorher);
    }

    /// Eine Leitung nach Drehbuch: was hereinkommt, bestimmt eine Antwort
    /// auf das, was hinausging (oder liegt von Anfang an bereit). Ohne
    /// Antwort meldet `bereit` nach der Wartezeit "nichts".
    struct Drehbuch<F: FnMut(&[u8]) -> Vec<u8>> {
        herein: VecDeque<u8>,
        hinaus: Vec<Vec<u8>>,
        antwort: F,
        zu: bool,
    }

    impl<F: FnMut(&[u8]) -> Vec<u8>> Leitung for Drehbuch<F> {
        fn senden(&mut self, b: &[u8]) -> Result<(), String> {
            self.hinaus.push(b.to_vec());
            let a = (self.antwort)(b);
            self.herein.extend(a);
            Ok(())
        }
        fn bereit(&mut self, warten: Duration) -> std::io::Result<bool> {
            if !self.herein.is_empty() {
                return Ok(true);
            }
            if self.zu {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Leitung zu"));
            }
            std::thread::sleep(warten);
            Ok(false)
        }
        fn lesen(&mut self, b: &mut [u8]) -> std::io::Result<()> {
            if self.herein.len() < b.len() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Leitung zu"));
            }
            for x in b.iter_mut() {
                *x = self.herein.pop_front().unwrap();
            }
            Ok(())
        }
    }

    fn ergebnis(e: Ergebnis) -> Vec<u8> {
        Nachricht::Ergebnis(e).kodieren()
    }

    /// Erst falsch, dann richtig: zwei Beweise hinaus, der Nutzer tippt
    /// nach "falsch" neu (die Oberflaeche erkennt es an der Runde).
    #[test]
    fn fuehren_falsch_dann_richtig() {
        let richtig = client_proof();
        let mut l = Drehbuch {
            herein: VecDeque::new(),
            hinaus: Vec::new(),
            antwort: move |b: &[u8]| {
                if b[8..] == richtig[..] {
                    ergebnis(Ergebnis::Passwort { host_beweis: host_proof() })
                } else {
                    ergebnis(Ergebnis::Falsch { warten_ms: 0 })
                }
            },
            zu: false,
        };
        let mut a = automat(true, 0, Instant::now());
        let mut runde = None;
        let aus = fuehren(
            &mut l,
            &mut a,
            |d| {
                if runde == Some(d.runde) || d.lage == Lage::Pruefen {
                    return None;
                }
                runde = Some(d.runde);
                Some(Eingabe::Passwort(if d.runde == 0 { "falsch".into() } else { "k7m-4wq-9tz".into() }))
            },
            Duration::from_secs(5),
        );
        assert_eq!(aus, Ausgang::Angenommen { per_passwort: true });
        assert_eq!(l.hinaus.len(), 2);
        assert_eq!(l.hinaus[1], beweis_bytes(&client_proof()));
    }

    /// "Zulassen" kommt, waehrend der Nutzer nichts eingibt; Stille endet
    /// nach der Ruhefrist; eine zugefallene Leitung und ein Abbruch.
    #[test]
    fn fuehren_zulassen_stille_leitung_abbruch() {
        let neu = || Drehbuch { herein: VecDeque::new(), hinaus: Vec::new(), antwort: |_: &[u8]| Vec::new(), zu: false };
        let mut l = neu();
        l.herein.extend(ergebnis(Ergebnis::Zulassen));
        let mut a = automat(true, 0, Instant::now());
        assert_eq!(fuehren(&mut l, &mut a, |_| None, Duration::from_secs(5)), Ausgang::Angenommen { per_passwort: false });
        assert!(l.hinaus.is_empty(), "ohne Eingabe geht kein Beweis hinaus");

        let mut l = neu();
        let mut a = automat(true, 0, Instant::now());
        let t0 = Instant::now();
        assert_eq!(fuehren(&mut l, &mut a, |_| None, Duration::from_millis(300)), Ausgang::KeineAntwort);
        assert!(t0.elapsed() >= Duration::from_millis(300));

        let mut l = neu();
        l.zu = true;
        let mut a = automat(true, 0, Instant::now());
        assert!(matches!(fuehren(&mut l, &mut a, |_| None, Duration::from_secs(5)), Ausgang::Leitung(_)));

        let mut l = neu();
        let mut a = automat(true, 0, Instant::now());
        assert_eq!(fuehren(&mut l, &mut a, |_| Some(Eingabe::Abbrechen), Duration::from_secs(5)), Ausgang::Abgebrochen);
        assert_eq!(l.hinaus, vec![Nachricht::Abbruch.kodieren()]);

        // Eine kaputte Nachricht (unbekannter Typ) beendet die Phase.
        let mut l = neu();
        l.herein.extend([99u8, 0, 0, 0, 0, 0, 0, 0]);
        let mut a = automat(true, 0, Instant::now());
        assert!(matches!(fuehren(&mut l, &mut a, |_| None, Duration::from_secs(5)), Ausgang::Protokoll(_)));
    }

    /// Eine Leitung, an der jedes Lesen mit dieser Art scheitert.
    struct Gestoert(std::io::ErrorKind);

    impl Leitung for Gestoert {
        fn senden(&mut self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn bereit(&mut self, _: Duration) -> std::io::Result<bool> {
            Ok(true)
        }
        fn lesen(&mut self, _: &mut [u8]) -> std::io::Result<()> {
            Err(std::io::Error::new(self.0, "gestoert"))
        }
    }

    #[test]
    fn kennung_und_noetig_lesen() {
        use std::io::ErrorKind as E;
        let mut l = Drehbuch { herein: VecDeque::new(), hinaus: Vec::new(), antwort: |_: &[u8]| Vec::new(), zu: false };
        l.herein.extend(*b"QCA1");
        l.herein.extend(Nachricht::Noetig(ZugangNoetig::neu(true, 7, "Mac")).kodieren());
        assert_eq!(kennung_lesen(&mut l), Ok(Kennung::Zugang));
        let n = noetig_lesen(&mut l).unwrap();
        assert_eq!((n.hostname.as_str(), n.warten_ms, n.zulassen_moeglich()), ("Mac", 7, true));
        // Sauber zu (EOF): Geschlossen - eine aeltere Fassung. Gestoert
        // (zurueckgesetzt, Datensatz nicht echt): Leitung. Frist: keine Antwort.
        assert_eq!(kennung_lesen(&mut l), Err(Ausgang::Geschlossen));
        for art in [E::ConnectionReset, E::InvalidData, E::ConnectionAborted] {
            assert!(matches!(kennung_lesen(&mut Gestoert(art)), Err(Ausgang::Leitung(_))), "{art:?}");
        }
        for art in [E::WouldBlock, E::TimedOut] {
            assert_eq!(kennung_lesen(&mut Gestoert(art)), Err(Ausgang::KeineAntwort), "{art:?}");
        }
        // Kein Platz am Host: statt 20 gleich 22/4 mit Wartezeit (so senden
        // es Mac- und Windows-Host) - ein Ergebnis 4, kein Protokollfehler.
        // Ebenso eine sofortige Ablehnung. Eine Annahme ohne 20: Protokoll.
        l.herein.extend(ergebnis(Ergebnis::Schluss { warten_ms: 5000 }));
        assert_eq!(noetig_lesen(&mut l), Err(Ausgang::Schluss { warten_ms: 5000, fehlversuche: 0 }));
        l.herein.extend(ergebnis(Ergebnis::Abgelehnt));
        assert_eq!(noetig_lesen(&mut l), Err(Ausgang::Abgelehnt));
        for e in [Ergebnis::Zulassen, Ergebnis::Falsch { warten_ms: 0 }, Ergebnis::Passwort { host_beweis: host_proof() }] {
            l.herein.extend(ergebnis(e));
            assert!(matches!(noetig_lesen(&mut l), Err(Ausgang::Protokoll(_))), "{e:?}");
        }
        assert!(matches!(noetig_lesen(&mut l), Err(Ausgang::Leitung(_))));
    }

    fn ordner(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("{}-phase-{name}", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 8.2: ueber eine ID verbunden, aber der Schluessel ergibt eine andere
    /// ID - Abbruch vor Nachricht 3. 8.3: an einer bekannten Adresse
    /// antwortet ein nirgends gepinnter Schluessel - neue Identitaet.
    #[test]
    fn vorwissen_id_und_neue_identitaet() {
        let d = ordner("vorwissen");
        let p = d.join("hosts.txt");
        let (a, b) = ([0xaau8; 32], [0xbbu8; 32]);
        let v = Vorwissen::laden(&p, "10.0.0.5:9001", Some(zugang::geraete_id(&a))).unwrap();
        assert_eq!(v.pruefen(&a), Ok(()));
        assert_eq!(
            v.pruefen(&b),
            Err(crate::secure::Fehler::AnderesGeraet {
                addr: "10.0.0.5:9001".into(),
                erwartet: zugang::geraete_id(&a),
                gemeldet: zugang::geraete_id(&b)
            })
        );
        assert!(!v.neue_identitaet(&a), "leere Liste: nichts ist neu");
        // Gemerkt ist zu dieser ID ein anderer voller Schluessel (errechnete
        // Kollision; hier nachgestellt mit einem Eintrag, der b die ID von a
        // gibt): dieselbe ID reicht nicht.
        let mut kollision = Vorwissen {
            liste: Hostliste::default(),
            adresse: "10.0.0.5:9001".into(),
            erwartet: Some(zugang::geraete_id(&a)),
            angenommen: None,
        };
        kollision.liste.hosts.push(BekannterHost { id: zugang::geraete_id(&a), schluessel: b, adresse: "10.0.0.5:9001".into(), name: "Mac".into() });
        assert_eq!(
            kollision.pruefen(&a),
            Err(crate::secure::Fehler::AnderesGeraet {
                addr: "10.0.0.5:9001".into(),
                erwartet: zugang::geraete_id(&a),
                gemeldet: zugang::geraete_id(&a)
            })
        );
        // Ist a selbst (auch) gemerkt, gilt es.
        kollision.liste.hosts.push(BekannterHost::neu(a, "10.0.0.7:9001", "Mac"));
        assert_eq!(kollision.pruefen(&a), Ok(()));
        assert!(pinnen(&p, &a, "10.0.0.5:9001", "Mac").unwrap());
        let v = Vorwissen::laden(&p, "10.0.0.5:9001", None).unwrap();
        assert_eq!(v.pruefen(&b), Ok(()), "ohne ID prueft der Handschlag nichts");
        let v = Vorwissen::laden(&p, "10.0.0.5:9001", Some(zugang::geraete_id(&a))).unwrap();
        assert_eq!(v.pruefen(&a), Ok(()), "gemerkt unter dieser ID");
        assert!(v.bekannt(&a).is_some());
        assert!(!v.neue_identitaet(&a));
        assert!(v.neue_identitaet(&b));
        // Unter einer anderen Adresse bekannt: nicht "neu".
        assert!(pinnen(&p, &b, "10.0.0.6:9001", "PC").unwrap());
        let v = Vorwissen::laden(&p, "10.0.0.5:9001", None).unwrap();
        assert!(!v.neue_identitaet(&b));
        // Kaputte Liste (kein UTF-8): nicht verbinden.
        std::fs::write(&p, b"\xff\xfe").unwrap();
        assert!(Vorwissen::laden(&p, "10.0.0.5:9001", None).is_err());
    }

    /// Bit 0 in Nachricht 3 (1.4): gesetzt, solange der Schluessel des Hosts
    /// nicht gepinnt ist - egal, unter welcher Adresse er gemerkt ist; der
    /// Host, der dieses Geraet eben angenommen hat, gilt auch ohne Eintrag
    /// als gepinnt.
    #[test]
    fn flags_fuer_nachricht_3() {
        let d = ordner("flags3");
        let p = d.join("hosts.txt");
        let (a, b) = ([0x31u8; 32], [0x32u8; 32]);
        let v = Vorwissen::laden(&p, "10.0.0.5:9001", None).unwrap();
        assert_eq!(v.flags3(&a), NAME_FLAG_HOST_UNBEKANNT, "leere Liste");
        assert!(pinnen(&p, &a, "10.0.0.9:9001", "Mac").unwrap());
        let mut v = Vorwissen::laden(&p, "10.0.0.5:9001", Some(zugang::geraete_id(&a))).unwrap();
        assert_eq!(v.flags3(&a), 0, "gepinnt, auch unter anderer Adresse");
        assert_eq!(v.flags3(&b), NAME_FLAG_HOST_UNBEKANNT);
        v.angenommen = Some(b.to_vec());
        assert_eq!(v.flags3(&b), 0, "eben angenommen");
        assert_eq!(v.flags3(&[0x33u8; 32]), NAME_FLAG_HOST_UNBEKANNT);
    }

    /// Pinnen schreibt nur, wenn sich etwas aendert; ein neuer Schluessel
    /// unter derselben Adresse steht danach vorn, der alte bleibt fuer
    /// seine ID.
    #[test]
    fn pinnen_nur_bei_aenderung() {
        let d = ordner("pinnen");
        let p = d.join("hosts.txt");
        let (a, b) = ([0x11u8; 32], [0x22u8; 32]);
        assert!(pinnen(&p, &a, "10.0.0.5:9001", "Mac").unwrap());
        let vorher = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(!pinnen(&p, &a, "10.0.0.5:9001", "Mac").unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), vorher);
        assert!(pinnen(&p, &a, "10.0.0.5:9001", "Mac mini").unwrap());
        assert!(pinnen(&p, &b, "10.0.0.5:9001", "Neu").unwrap());
        let l = Hostliste::laden(&p).unwrap();
        assert_eq!(l.nach_adresse("10.0.0.5:9001").map(|h| h.schluessel), Some(b));
        assert_eq!(l.nach_id(zugang::geraete_id(&a)).map(|h| h.name.as_str()), Some("Mac mini"));
        assert_eq!(l.hosts.len(), 2);
        assert!(pinnen(&p, &[1u8; 31], "10.0.0.5:9001", "x").is_err());
    }
}
