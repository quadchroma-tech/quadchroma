// Einlass: wer nach dem Handschlag auf dem Bildkanal herein darf
// (Spezifikation Pairing v1, Abschnitte 3.1-3.4, 4.1-4.3 und 10.5).
//
// Nach dem Noise-Handschlag entscheidet `Einlass::pruefen`:
//   - Steht der Schluessel der Gegenstelle in host-devices.txt, geht es wie
//     bisher weiter (der Aufrufer sendet MAGIC "QCH1").
//   - Sonst folgt die Zugangsphase: "QCA1", Nachricht 20 (Wege, Wartezeit
//     der Drossel, Rechnername), dann wird gelesen - mit Gesamtfrist -, bis
//     ein richtiger Beweis (21) kommt, jemand am Host "Zulassen" oder
//     "Ablehnen" klickt, der Client abbricht (23 oder Leitung zu) oder die
//     Frist ablaeuft. Richtig -> eintragen, 22/0 mit host_proof, dann wie
//     bekannt. Falsch -> Drossel, 22/2 und weiterlesen; der fuenfte
//     Fehlversuch dieser Verbindung -> 22/4. Zulassen -> eintragen, 22/1.
//     Ablehnen -> 22/3. Frist -> 22/4.
//   - Hoechstens PLAETZE_GESAMT Zugangsphasen zugleich, eine je Schluessel,
//     zwei je IP (zugang::Plaetze); wer keinen Platz bekommt, erhaelt sofort
//     22/4 mit BESETZT_WARTEN_MS.
//
// Waehrend der Zugangsphase haelt niemand eine globale Sperre: die
// Geraeteliste wird nur kurz unter `liste` gelesen bzw. beschrieben,
// Drossel und Plaetze nur kurz unter `buch`, K (PBKDF2) nur unter `cache`.
// Ein bekanntes Geraet und der laufende Zuschauer merken von wartenden
// Unbekannten nichts.
//
// Warten auf zwei Seiten zugleich (Leitung und Oberflaeche): die Leitung
// wird nie auf gut Glueck gelesen - ein Lesen mit kurzer Frist mitten in
// einem verschluesselten Datensatz braeche den Strom. Stattdessen schaut
// `warten_auf_daten` mit kurzer Frist nach, ob Bytes da sind (peek, oder
// schon entschluesselter Klartext), und erst dann wird eine ganze Nachricht
// gelesen (mit LESEFRIST). Dazwischen fragt die Schleife die Entscheidung
// der Oberflaeche ab (Kanal, alle ABFRAGE).
//
// Die Oberflaeche (Zulassen-Fenster) haengt ueber `Oberflaeche` an; die
// Warteschlange der Anfragen fuehrt der Einlass selbst: gezeigt wird
// hoechstens eine, weitere warten, zieht sich die gezeigte zurueck, kommt
// die naechste. Tests setzen eine eigene Oberflaeche (Test-Haken).
//
// Geraeteliste und Passwortdatei: zugang.rs (atomar, beschaedigt ->
// niemand bekannt und nichts ueberschrieben).

use std::collections::VecDeque;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::log;
use super::netz::Drossel;
use crate::protokoll_konst::MAGIC_ZUGANG;
use crate::secure;
use crate::zugang::{self, DateiFehler, Ergebnis, Geraeteliste, Nachricht, PasswortFehler, Wertung, ZugangNoetig};

/// So oft schaut die Zugangsphase nach Leitung, Oberflaeche und Frist.
const ABFRAGE: Duration = Duration::from_millis(50);
/// Hat eine Nachricht einmal begonnen, muss sie binnen dieser Zeit ganz da
/// sein (ein Client schickt sie in einem Stueck).
const LESEFRIST: Duration = Duration::from_secs(10);
/// Frist fuer jedes Schreiben in der Zugangsphase (ein paar Dutzend Byte).
const SCHREIBFRIST: Duration = Duration::from_secs(5);

/// Protokollzeilen, die jede Verbindung ausloesen kann: je Art und Adresse
/// hoechstens eine alle 10 s (netz::Drossel).
static DROSSEL_NOETIG: Drossel = Drossel::neu("Zugang noetig");
static DROSSEL_FALSCH: Drossel = Drossel::neu("Zugang: Passwort falsch");
static DROSSEL_ENDE: Drossel = Drossel::neu("Zugang: ohne Ergebnis beendet");
static DROSSEL_BESETZT: Drossel = Drossel::neu("Zugang: kein Platz frei");
static DROSSEL_LISTE: Drossel = Drossel::neu("Geraeteliste nicht lesbar");
static DROSSEL_PASSWORT: Drossel = Drossel::neu("Zugangspasswort nicht lesbar");

/// Die Drosseln dieses Teils, fuer netz::drosseln_nachtragen.
pub(super) fn drosseln() -> [&'static Drossel; 6] {
    [&DROSSEL_NOETIG, &DROSSEL_FALSCH, &DROSSEL_ENDE, &DROSSEL_BESETZT, &DROSSEL_LISTE, &DROSSEL_PASSWORT]
}

/// Sperre nehmen, auch wenn ein anderer Faden unter ihr in Panik geraten
/// ist - die Zustaende hier bleibt jeder Schritt heil.
fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Eine Anfrage "Zulassen?" fuer die Oberflaeche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anfrage {
    /// Laufende Nummer (fuer entscheiden und schliessen).
    pub nr: u64,
    /// Name des Clients (Nachricht 3, sonst seine IP) - unbeglaubigt.
    pub name: String,
    /// Geraete-ID des Clients.
    pub id: u32,
    /// Vergleichscode des Handschlags ("628 306").
    pub code: String,
}

/// Was die Host-Rolle zeigen kann. Alle Aufrufe kommen aus Netzfaeden und
/// unter der Sperre der Anfragen: sie duerfen nicht warten und nie selbst
/// `Einlass::entscheiden` rufen (das tut die Oberflaeche aus ihrem eigenen
/// Faden, ohne eigene Sperre).
pub trait Oberflaeche: Send + Sync {
    /// Kann gerade jemand "Zulassen" klicken (Bit 1 in Nachricht 20 und in
    /// der Bekanntgabe)?
    fn vorhanden(&self) -> bool;
    /// Diese Anfrage zeigen (hoechstens eine zugleich).
    fn zeigen(&self, anfrage: &Anfrage);
    /// Die gezeigte Anfrage `nr` hat sich zurueckgezogen: Fenster schliessen,
    /// ohne zu antworten.
    fn schliessen(&self, nr: u64);
}

/// Die Warteschlange der Anfragen (unter `Einlass::anfragen`).
#[derive(Default)]
struct Anfragen {
    naechste: u64,
    /// Offene Anfragen in der Reihenfolge ihres Eintreffens, mit dem Weg
    /// zurueck zu ihrer Zugangsphase.
    offen: VecDeque<(Anfrage, mpsc::Sender<bool>)>,
    /// Die gerade gezeigte.
    gezeigt: Option<u64>,
}

/// Wie eine Pruefung ausging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ausgang {
    /// Stand schon in der Geraeteliste.
    Bekannt,
    /// Unbekannt, Passwort bewiesen (22/0 ist hinaus).
    Passwort,
    /// Unbekannt, am Host zugelassen (22/1 ist hinaus).
    Zugelassen,
    /// Nicht herein: abgelehnt, abgebrochen, Frist, zu viele Versuche,
    /// kein Platz, Leitung zu oder Protokollfehler.
    Draussen,
}

impl Ausgang {
    /// Weiter mit MAGIC "QCH1" und der Sitzung?
    pub fn herein(self) -> bool {
        self != Ausgang::Draussen
    }
}

/// Drossel und Plaetze der Zugangsphasen.
#[derive(Default)]
struct Buch {
    drossel: zugang::Drossel,
    plaetze: zugang::Plaetze,
}

/// Der Einlass einer Host-Rolle (ein Port): Dateien, Schluessel, Zustand der
/// Zugangsphasen und die Oberflaeche.
pub struct Einlass {
    geraete: PathBuf,
    passwort: PathBuf,
    host_pub: Vec<u8>,
    id: u32,
    /// Rechnername fuer Nachricht 20 (zugang::geraetename).
    hostname: String,
    /// Gesamtfrist einer Zugangsphase (zugang::PHASE_FRIST, in Tests kuerzer).
    frist: Duration,
    buch: Mutex<Buch>,
    cache: Mutex<zugang::Schluesselcache>,
    /// Lesen, Pruefen und Eintragen der Geraeteliste - nur kurz.
    liste: Mutex<()>,
    anfragen: Mutex<Anfragen>,
    ui: Mutex<Option<Arc<dyn Oberflaeche>>>,
    /// "Geraeteliste beschaedigt" steht schon im Protokoll (einmal, bis sie
    /// wieder lesbar ist; jede Verbindung einzeln laeuft ueber DROSSEL_LISTE).
    liste_gemeldet: AtomicBool,
}

static EINLASS: OnceLock<Arc<Einlass>> = OnceLock::new();

/// Den Einlass des Dienstes einrichten (einmal, vor netz::start).
pub fn einrichten(e: Arc<Einlass>) -> Result<(), String> {
    EINLASS.set(e).map_err(|_| "Einlass schon eingerichtet".to_string())
}

/// Der Einlass des Dienstes (None vor `einrichten`).
pub fn einlass() -> Option<&'static Arc<Einlass>> {
    EINLASS.get()
}

/// Wie `warten_auf_daten` ausging.
#[derive(Debug, PartialEq, Eq)]
enum Warten {
    Daten,
    Nichts,
    Zu,
}

/// Liegt auf der Leitung etwas zum Lesen? Hoechstens `wie_lange` warten.
/// Liest nichts (peek), damit ein Datensatz nie halb verbraucht wird.
fn warten_auf_daten(sock: &secure::Secure, wie_lange: Duration) -> Warten {
    if sock.gepuffert() {
        return Warten::Daten;
    }
    let s = sock.socket();
    s.set_read_timeout(Some(wie_lange.max(Duration::from_millis(1)))).ok();
    match s.peek(&mut [0u8; 1]) {
        Ok(0) => Warten::Zu,
        Ok(_) => Warten::Daten,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {
            Warten::Nichts
        }
        Err(_) => Warten::Zu,
    }
}

/// Nachrichten der Zugangsphase in EINEM Schreibvorgang (ein Datensatz).
fn senden(sock: &mut secure::Secure, mit_kennung: bool, n: &Nachricht) -> Result<(), String> {
    let mut v = Vec::with_capacity(64);
    if mit_kennung {
        v.extend_from_slice(MAGIC_ZUGANG);
    }
    v.extend_from_slice(&n.kodieren());
    sock.write_all(&v)
}

/// Ein belegter Platz einer Zugangsphase; gibt sich beim Wegfallen frei.
struct PlatzGriff<'a> {
    e: &'a Einlass,
    ip: IpAddr,
    peer: Vec<u8>,
}

impl Drop for PlatzGriff<'_> {
    fn drop(&mut self) {
        self.e.buch().plaetze.freigeben(self.ip, &self.peer);
    }
}

/// Eine gestellte Anfrage; zieht sich beim Wegfallen zurueck (war sie schon
/// entschieden, geschieht nichts).
struct AnfrageGriff<'a> {
    e: &'a Einlass,
    nr: u64,
    rx: mpsc::Receiver<bool>,
}

impl Drop for AnfrageGriff<'_> {
    fn drop(&mut self) {
        self.e.zurueckziehen(self.nr);
    }
}

impl Einlass {
    /// `geraete`/`passwort`: Pfade von host-devices.txt und host-password.txt;
    /// `host_pub`: der oeffentliche Schluessel des Hosts (Salz von K, ID);
    /// `hostname`: der Name in Nachricht 20.
    pub fn neu(geraete: PathBuf, passwort: PathBuf, host_pub: &[u8], hostname: &str) -> Einlass {
        Einlass {
            geraete,
            passwort,
            host_pub: host_pub.to_vec(),
            id: zugang::geraete_id(host_pub),
            hostname: zugang::name_bereinigen(hostname),
            frist: zugang::PHASE_FRIST,
            buch: Mutex::new(Buch::default()),
            cache: Mutex::new(zugang::Schluesselcache::default()),
            liste: Mutex::new(()),
            anfragen: Mutex::new(Anfragen::default()),
            ui: Mutex::new(None),
            liste_gemeldet: AtomicBool::new(false),
        }
    }

    /// Mit anderer Gesamtfrist (Tests).
    #[cfg(test)]
    pub fn mit_frist(mut self, frist: Duration) -> Einlass {
        self.frist = frist;
        self
    }

    /// Die Oberflaeche anhaengen (Zulassen-Fenster bzw. Test-Haken).
    pub fn oberflaeche_setzen(&self, ui: Arc<dyn Oberflaeche>) {
        *sperre(&self.ui) = Some(ui);
    }

    fn buch(&self) -> MutexGuard<'_, Buch> {
        sperre(&self.buch)
    }

    /// Geraete-ID dieses Hosts.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Kann am Host gerade jemand "Zulassen" klicken?
    pub fn zulassen_moeglich(&self) -> bool {
        sperre(&self.ui).as_ref().is_some_and(|u| u.vorhanden())
    }

    // ------------------------------------------------------ Geraeteliste

    /// Die Geraeteliste, wie sie gerade in der Datei steht (Oberflaeche).
    pub fn geraete(&self) -> Result<Geraeteliste, DateiFehler> {
        let _l = sperre(&self.liste);
        Geraeteliste::laden(&self.geraete)
    }

    /// Steht dieser Schluessel in der Liste? Eine unlesbare oder beschaedigte
    /// Liste heisst: niemand ist bekannt (Protokollzeile, gedrosselt).
    fn bekannt(&self, peer: &[u8], ip: IpAddr) -> bool {
        match self.geraete() {
            Ok(l) => {
                self.liste_gemeldet.store(false, Ordering::Relaxed);
                l.enthaelt(peer)
            }
            Err(e) => {
                if !self.liste_gemeldet.swap(true, Ordering::Relaxed) {
                    log(format!("Geraeteliste: {e} - niemand gilt als bekannt, jedes Geraet braucht Passwort oder \"Zulassen\"; die Datei bleibt, wie sie ist"));
                } else {
                    DROSSEL_LISTE.melden(Some(ip), || format!("Geraeteliste weiter nicht lesbar ({ip}): {e}"));
                }
                false
            }
        }
    }

    /// Traegt ein angenommenes Geraet ein (heutiges Datum). Scheitert das
    /// (Liste beschaedigt, Schreibfehler), gilt die Annahme nur fuer diese
    /// Sitzung - das steht im Protokoll.
    fn eintragen(&self, peer: &[u8], name: &str) {
        let Ok(schluessel) = <[u8; 32]>::try_from(peer) else {
            log(format!("Geraet {name} nicht eingetragen: Schluessel mit {} Byte", peer.len()));
            return;
        };
        let r = {
            let _l = sperre(&self.liste);
            zugang::geraet_eintragen(&self.geraete, &schluessel, name, &zugang::heute())
        };
        let id = zugang::id_text(zugang::geraete_id(peer));
        match r {
            Ok(true) => log(format!("Geraet eingetragen: {name}, ID {id}")),
            Ok(false) => log(format!("Geraet {name}, ID {id} stand schon in der Liste")),
            Err(e) => log(format!("Geraet {name}, ID {id} NICHT eingetragen ({e}) - zugelassen nur fuer diese Sitzung")),
        }
    }

    /// Ein Geraet entfernen (Oberflaeche). Ok(false): stand nicht darin.
    pub fn geraet_entfernen(&self, schluessel: &[u8]) -> Result<bool, DateiFehler> {
        let r = {
            let _l = sperre(&self.liste);
            zugang::geraet_entfernen(&self.geraete, schluessel)
        };
        let id = zugang::id_text(zugang::geraete_id(schluessel));
        match &r {
            Ok(true) => log(format!("Geraet entfernt: ID {id}")),
            Ok(false) => log(format!("Geraet ID {id} stand nicht (mehr) in der Liste")),
            Err(e) => log(format!("Geraet ID {id} nicht entfernt: {e}")),
        }
        r
    }

    /// "Alle Geraete entfernen" (Oberflaeche).
    pub fn alle_entfernen(&self) -> Result<(), DateiFehler> {
        let r = {
            let _l = sperre(&self.liste);
            zugang::alle_geraete_entfernen(&self.geraete)
        };
        match &r {
            Ok(()) => log("Alle Geraete entfernt"),
            Err(e) => log(format!("Geraete nicht entfernt: {e}")),
        }
        r
    }

    /// "Geraeteliste zuruecksetzen" bei beschaedigter Liste (Oberflaeche).
    pub fn liste_zuruecksetzen(&self) -> Result<(), DateiFehler> {
        let r = {
            let _l = sperre(&self.liste);
            zugang::geraeteliste_zuruecksetzen(&self.geraete, &zugang::jetzt_lokal().stempel())
        };
        match &r {
            Ok(Some(alt)) => log(format!("Geraeteliste zurueckgesetzt - die alte Datei liegt als {}", alt.display())),
            Ok(None) => log("Geraeteliste neu angelegt"),
            Err(e) => log(format!("Geraeteliste nicht zurueckgesetzt: {e}")),
        }
        self.liste_gemeldet.store(false, Ordering::Relaxed);
        r.map(|_| ())
    }

    // ---------------------------------------------------------- Passwort

    /// Das Zugangspasswort zum Anzeigen. Fehlt die Datei, entsteht ein
    /// Zufallspasswort (Protokollzeile). Err: unlesbar - Zugang nur per
    /// "Zulassen".
    pub fn passwort(&self) -> Result<String, DateiFehler> {
        let p = zugang::passwort_laden(&self.passwort)?;
        if p.neu {
            log("Zugangspasswort: neues Zufallspasswort angelegt (host-password.txt)");
        }
        Ok(p.text)
    }

    /// Eigenes Passwort setzen (Passwortfenster).
    pub fn passwort_setzen(&self, pw: &str) -> Result<(), PasswortFehler> {
        let r = zugang::passwort_setzen(&self.passwort, pw);
        match &r {
            Ok(_) => log("Zugangspasswort geaendert"),
            Err(e) => log(format!("Zugangspasswort nicht geaendert: {e}")),
        }
        r.map(|_| ())
    }

    /// "Neues Zufallspasswort".
    pub fn passwort_zufall(&self) -> Result<(), DateiFehler> {
        let r = zugang::passwort_zufall_setzen(&self.passwort);
        match &r {
            Ok(_) => log("Zugangspasswort: neues Zufallspasswort gesetzt"),
            Err(e) => log(format!("Neues Zufallspasswort nicht gesetzt: {e}")),
        }
        r.map(|_| ())
    }

    /// K fuer das aktuelle Passwort; None, wenn es keins gibt (unlesbar) -
    /// dann ist jeder Beweis falsch.
    fn schluessel(&self, ip: IpAddr) -> Option<[u8; 32]> {
        match self.passwort() {
            Ok(pw) => Some(sperre(&self.cache).schluessel(&pw, &self.host_pub)),
            Err(e) => {
                DROSSEL_PASSWORT.melden(Some(ip), || {
                    format!("Zugangspasswort nicht lesbar ({e}) - Beweis von {ip} gilt als falsch, Zugang nur ueber \"Zulassen\"")
                });
                None
            }
        }
    }

    // ---------------------------------------------------------- Anfragen

    /// Eine Anfrage stellen; gezeigt wird sie, sobald keine andere mehr
    /// gezeigt wird.
    fn anfrage_stellen(&self, name: &str, id: u32, code: &str) -> AnfrageGriff<'_> {
        let (tx, rx) = mpsc::channel();
        let mut a = sperre(&self.anfragen);
        a.naechste += 1;
        let nr = a.naechste;
        a.offen.push_back((Anfrage { nr, name: name.to_string(), id, code: code.to_string() }, tx));
        self.naechste_zeigen(&mut a);
        AnfrageGriff { e: self, nr, rx }
    }

    /// Unter der Sperre der Anfragen: ist keine gezeigt, die aelteste zeigen.
    fn naechste_zeigen(&self, a: &mut Anfragen) {
        if a.gezeigt.is_some() {
            return;
        }
        let Some((erste, _)) = a.offen.front() else { return };
        let ui = sperre(&self.ui).clone();
        if let Some(ui) = ui {
            a.gezeigt = Some(erste.nr);
            ui.zeigen(erste);
        }
    }

    /// Die Antwort der Oberflaeche auf Anfrage `nr` (true = Zulassen). Aus
    /// dem Faden der Oberflaeche, ohne deren eigene Sperre. false: diese
    /// Anfrage gibt es nicht mehr (zurueckgezogen, schon entschieden).
    pub fn entscheiden(&self, nr: u64, zulassen: bool) -> bool {
        let mut a = sperre(&self.anfragen);
        let Some(i) = a.offen.iter().position(|(x, _)| x.nr == nr) else { return false };
        let (_, tx) = a.offen.remove(i).expect("Stelle eben gefunden");
        let _ = tx.send(zulassen);
        if a.gezeigt == Some(nr) {
            a.gezeigt = None;
        }
        self.naechste_zeigen(&mut a);
        true
    }

    /// Die Zugangsphase endet ohne Entscheidung: Anfrage weg, ein offenes
    /// Fenster zu, die naechste zeigen.
    fn zurueckziehen(&self, nr: u64) {
        let mut a = sperre(&self.anfragen);
        let Some(i) = a.offen.iter().position(|(x, _)| x.nr == nr) else { return };
        a.offen.remove(i);
        if a.gezeigt == Some(nr) {
            a.gezeigt = None;
            if let Some(ui) = sperre(&self.ui).clone() {
                ui.schliessen(nr);
            }
        }
        self.naechste_zeigen(&mut a);
    }

    /// Die gerade gezeigte Anfrage (Tests, Oberflaeche).
    #[cfg(test)]
    pub fn gezeigt(&self) -> Option<u64> {
        sperre(&self.anfragen).gezeigt
    }

    // ------------------------------------------------------- Zugangsphase

    /// Entscheidet ueber eine eben angenommene Leitung (nach dem Handschlag,
    /// vor MAGIC). `ip`: Absender; `name`: Name aus Nachricht 3, sonst die
    /// IP. Bei `herein()` sendet der Aufrufer danach MAGIC "QCH1" und alles
    /// Weitere; sonst schliesst er die Leitung. Fristen an der Leitung sind
    /// danach wieder aufgehoben.
    pub fn pruefen(&self, sock: &mut secure::Secure, ip: IpAddr, name: &str) -> Ausgang {
        let peer = sock.peer.clone();
        if self.bekannt(&peer, ip) {
            return Ausgang::Bekannt;
        }
        sock.socket().set_write_timeout(Some(SCHREIBFRIST)).ok();
        let a = self.zugangsphase(sock, ip, name, &peer);
        sock.socket().set_read_timeout(None).ok();
        sock.socket().set_write_timeout(None).ok();
        a
    }

    fn zugangsphase(&self, sock: &mut secure::Secure, ip: IpAddr, name: &str, peer: &[u8]) -> Ausgang {
        let id = zugang::geraete_id(peer);
        let wer = format!("{name} ({ip}), ID {}", zugang::id_text(id));
        // Ein Platz, oder sofort 22/4 mit BESETZT_WARTEN_MS.
        let platz = {
            let mut b = self.buch();
            if b.plaetze.belegen(ip, peer) {
                Some(PlatzGriff { e: self, ip, peer: peer.to_vec() })
            } else {
                None
            }
        };
        let Some(_platz) = platz else {
            DROSSEL_BESETZT.melden(Some(ip), || {
                format!("Zugang: kein Platz fuer {wer} - schon zu viele Zugangsphasen (je Geraet eine, je Adresse zwei, zusammen {}), abgewiesen", zugang::PLAETZE_GESAMT)
            });
            let _ = senden(sock, true, &Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: zugang::BESETZT_WARTEN_MS }));
            return Ausgang::Draussen;
        };
        let beginn = Instant::now();
        let bis = beginn + self.frist;
        let mut phase = self.buch().drossel.phase_beginnen(ip, peer, beginn);
        let zulassen = self.zulassen_moeglich();
        let warten_ms = phase.warten_ms(beginn);
        DROSSEL_NOETIG.melden(Some(ip), || {
            format!(
                "Zugang noetig: {wer} ist unbekannt - Passwort{}, Wartezeit {warten_ms} ms",
                if zulassen { " oder \"Zulassen\" am Host" } else { " (kein \"Zulassen\": keine Oberflaeche)" }
            )
        });
        let noetig = Nachricht::Noetig(ZugangNoetig::neu(zulassen, warten_ms, &self.hostname));
        if let Err(e) = senden(sock, true, &noetig) {
            DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} - Nachricht 20 nicht gesendet: {e}"));
            return Ausgang::Draussen;
        }
        let anfrage = zulassen.then(|| self.anfrage_stellen(name, id, &sock.sas));
        let hh = sock.handshake_hash.clone();
        loop {
            // Die Oberflaeche hat entschieden?
            if let Some(a) = &anfrage {
                match a.rx.try_recv() {
                    Ok(true) => {
                        self.eintragen(peer, name);
                        self.buch().drossel.erfolg(ip, peer);
                        log(format!("Zugang: {wer} am Host zugelassen"));
                        return match senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Zulassen)) {
                            Ok(()) => Ausgang::Zugelassen,
                            Err(e) => {
                                log(format!("Zugang: {wer} - Ergebnis nicht gesendet: {e}"));
                                Ausgang::Draussen
                            }
                        };
                    }
                    Ok(false) => {
                        log(format!("Zugang: {wer} am Host abgelehnt"));
                        let _ = senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Abgelehnt));
                        return Ausgang::Draussen;
                    }
                    Err(_) => {}
                }
            }
            let jetzt = Instant::now();
            if jetzt >= bis {
                DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} - Frist von {} s abgelaufen, getrennt", self.frist.as_secs()));
                let _ = senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 0 }));
                return Ausgang::Draussen;
            }
            match warten_auf_daten(sock, (bis - jetzt).min(ABFRAGE)) {
                Warten::Nichts => continue,
                Warten::Zu => {
                    DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} hat die Leitung geschlossen"));
                    return Ausgang::Draussen;
                }
                Warten::Daten => {}
            }
            // Eine ganze Nachricht lesen: sie hat begonnen, der Rest folgt gleich.
            let rest = bis.saturating_duration_since(Instant::now());
            sock.socket().set_read_timeout(Some(LESEFRIST.min(rest + ABFRAGE).max(Duration::from_millis(10)))).ok();
            let n = zugang::empfangen(|b| sock.read_exact(b));
            match n {
                Ok(Nachricht::Beweis(beweis)) => {
                    let k = self.schluessel(ip);
                    let richtig = k.is_some_and(|k| zugang::beweis_pruefen(&k, &hh, &beweis));
                    let wertung = self.buch().drossel.beweis_werten(&mut phase, ip, peer, richtig, Instant::now());
                    let warten_ms = match (wertung, k) {
                        (Wertung::Angenommen, Some(k)) => {
                            self.eintragen(peer, name);
                            log(format!("Zugang: {wer} - Passwort richtig"));
                            let ergebnis = Ergebnis::Passwort { host_beweis: zugang::host_beweis(&k, &hh) };
                            return match senden(sock, false, &Nachricht::Ergebnis(ergebnis)) {
                                Ok(()) => Ausgang::Passwort,
                                Err(e) => {
                                    log(format!("Zugang: {wer} - Ergebnis nicht gesendet: {e}"));
                                    Ausgang::Draussen
                                }
                            };
                        }
                        (Wertung::Schluss { warten_ms }, _) => {
                            log(format!(
                                "Zugang: {wer} - {} Fehlversuche in dieser Verbindung, getrennt (naechster Versuch fruehestens in {warten_ms} ms)",
                                phase.fehlversuche
                            ));
                            let _ = senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms }));
                            return Ausgang::Draussen;
                        }
                        (Wertung::Falsch { warten_ms }, _) => warten_ms,
                        // Ohne K ist `richtig` false - angenommen wird dann nie.
                        (Wertung::Angenommen, None) => 0,
                    };
                    let zahl = self.buch().drossel.fehlversuche(ip, peer, Instant::now());
                    DROSSEL_FALSCH.melden(Some(ip), || {
                        format!("Zugang: {wer} - Passwort falsch (Fehlversuch {zahl}), naechster Versuch fruehestens in {warten_ms} ms")
                    });
                    if let Err(e) = senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms })) {
                        DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} - Ergebnis nicht gesendet: {e}"));
                        return Ausgang::Draussen;
                    }
                }
                Ok(Nachricht::Abbruch) => {
                    DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} hat abgebrochen"));
                    return Ausgang::Draussen;
                }
                Ok(andere) => {
                    DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} sendet Nachricht {} - nur 21 und 23 erlaubt, getrennt", andere.typ()));
                    return Ausgang::Draussen;
                }
                Err(e) => {
                    let frist = Instant::now() >= bis;
                    DROSSEL_ENDE.melden(Some(ip), || format!("Zugang: {wer} - {e}{}", if frist { " (Frist abgelaufen)" } else { "" }));
                    if frist {
                        let _ = senden(sock, false, &Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 0 }));
                    }
                    return Ausgang::Draussen;
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Tests

/// Ein Client im Kleinen fuer die Tests (auch netz.rs): Noise-Anrufer mit
/// eigenem Schluessel und frei waehlbarer Nutzlast in Nachricht 3, dieselbe
/// Rahmung wie secure.rs - unabhaengig vom Client-Code, der sich im Umbau
/// befindet.
#[cfg(test)]
pub(super) mod stub {
    use crate::noise;
    use crate::zugang::{self, LeseFehler, Nachricht};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    pub struct Stub {
        pub sock: TcpStream,
        tx: snow::TransportState,
        inbuf: Vec<u8>,
        inpos: usize,
        pub hh: Vec<u8>,
        pub host_pub: Vec<u8>,
        /// Vergleichscode dieses Handschlags.
        pub sas: String,
    }

    fn rahmen_lesen(s: &mut TcpStream) -> Result<Vec<u8>, String> {
        let mut l = [0u8; 2];
        s.read_exact(&mut l).map_err(|e| format!("Laenge: {e}"))?;
        let mut b = vec![0u8; u16::from_le_bytes(l) as usize];
        s.read_exact(&mut b).map_err(|e| format!("Daten: {e}"))?;
        Ok(b)
    }

    fn rahmen_schreiben(s: &mut TcpStream, d: &[u8]) -> Result<(), String> {
        let mut v = (d.len() as u16).to_le_bytes().to_vec();
        v.extend_from_slice(d);
        s.write_all(&v).map_err(|e| format!("Senden: {e}"))
    }

    impl Stub {
        /// Verbinden und den Handschlag als Anrufer fuehren; `nachricht3`
        /// ist die Nutzlast von Nachricht 3 (b"client" wie ein alter Client,
        /// zugang::nachricht3(name) wie ein neuer).
        pub fn verbinden(addr: &str, priv_key: &[u8], nachricht3: &[u8]) -> Result<Stub, String> {
            let mut sock = TcpStream::connect(addr).map_err(|e| format!("Verbindung: {e}"))?;
            sock.set_nodelay(true).ok();
            sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
            let params: snow::params::NoiseParams = noise::PATTERN.parse().map_err(|e| format!("{e:?}"))?;
            let prolog = noise::prologue_video();
            let mut hs = snow::Builder::new(params)
                .local_private_key(priv_key)
                .and_then(|b| b.prologue(&prolog))
                .and_then(|b| b.build_initiator())
                .map_err(|e| format!("{e:?}"))?;
            let mut buf = vec![0u8; 65535];
            let n = hs.write_message(&[], &mut buf).map_err(|e| format!("{e:?}"))?;
            rahmen_schreiben(&mut sock, &buf[..n])?;
            let m2 = rahmen_lesen(&mut sock)?;
            let mut p = vec![0u8; 65535];
            hs.read_message(&m2, &mut p).map_err(|e| format!("Nachricht 2: {e:?}"))?;
            let n = hs.write_message(nachricht3, &mut buf).map_err(|e| format!("{e:?}"))?;
            rahmen_schreiben(&mut sock, &buf[..n])?;
            let hh = hs.get_handshake_hash().to_vec();
            let host_pub = hs.get_remote_static().map(|k| k.to_vec()).unwrap_or_default();
            let tx = hs.into_transport_mode().map_err(|e| format!("{e:?}"))?;
            let sas = noise::sas(&hh);
            Ok(Stub { sock, tx, inbuf: Vec::new(), inpos: 0, hh, host_pub, sas })
        }

        pub fn schreiben(&mut self, d: &[u8]) -> Result<(), String> {
            let mut buf = vec![0u8; d.len() + 64];
            let n = self.tx.write_message(d, &mut buf).map_err(|e| format!("{e:?}"))?;
            rahmen_schreiben(&mut self.sock, &buf[..n])
        }

        pub fn lesen(&mut self, ziel: &mut [u8]) -> Result<(), String> {
            let mut fertig = 0;
            while fertig < ziel.len() {
                if self.inpos == self.inbuf.len() {
                    let ct = rahmen_lesen(&mut self.sock)?;
                    let mut pt = vec![0u8; ct.len()];
                    let n = self.tx.read_message(&ct, &mut pt).map_err(|e| format!("{e:?}"))?;
                    pt.truncate(n);
                    self.inbuf = pt;
                    self.inpos = 0;
                }
                let k = (self.inbuf.len() - self.inpos).min(ziel.len() - fertig);
                ziel[fertig..fertig + k].copy_from_slice(&self.inbuf[self.inpos..self.inpos + k]);
                self.inpos += k;
                fertig += k;
            }
            Ok(())
        }

        /// Die ersten 4 Byte nach dem Handschlag: "QCH1" oder "QCA1".
        pub fn kennung(&mut self) -> Result<[u8; 4], String> {
            let mut m = [0u8; 4];
            self.lesen(&mut m)?;
            Ok(m)
        }

        pub fn nachricht(&mut self) -> Result<Nachricht, LeseFehler> {
            let mut k = [0u8; 8];
            self.lesen(&mut k).map_err(LeseFehler::Leitung)?;
            let (typ, laenge) = zugang::kopf_lesen(&k)?;
            let mut n = vec![0u8; laenge];
            self.lesen(&mut n).map_err(LeseFehler::Leitung)?;
            zugang::nachricht_lesen(typ, &n)
        }

        pub fn senden(&mut self, n: &Nachricht) -> Result<(), String> {
            self.schreiben(&n.kodieren())
        }

        /// Beweis mit diesem Passwort fuer diese Sitzung.
        pub fn beweis(&self, pw: &str) -> [u8; 32] {
            let k = zugang::passwort_schluessel(pw, &self.host_pub);
            zugang::client_beweis(&k, &self.hh)
        }

        /// Hat der Host die Leitung zu? (Lesen endet ohne Daten.)
        pub fn zu(&mut self) -> bool {
            self.sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut b = [0u8; 1];
            matches!(self.sock.read(&mut b), Ok(0) | Err(_))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stub::Stub;
    use super::*;
    use crate::noise;
    use crate::protokoll_konst::{MAGIC, WEG_PASSWORT, WEG_ZULASSEN};
    use std::net::TcpListener;

    const PW: &str = "k7m-4wq-9tz";

    /// Eigener leerer Ordner je Test (die Tests laufen nebeneinander).
    fn ordner(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{}-einlass-{name}", secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Ein Host im Kleinen: eigener Schluessel, eigener Einlass, eigener
    /// Port; jede Verbindung laeuft wie in netz::bild_annehmen (Handschlag,
    /// Name aus Nachricht 3, pruefen, bei Annahme MAGIC) und meldet ihren
    /// Ausgang.
    struct Host {
        addr: String,
        einlass: Arc<Einlass>,
        ausgaenge: mpsc::Receiver<Ausgang>,
        ordner: PathBuf,
    }

    impl Host {
        fn neu(name: &str, frist: Duration) -> Host {
            let ordner = ordner(name);
            let (host_priv, host_pub) = noise::keypair().unwrap();
            let e = Einlass::neu(ordner.join(zugang::GERAETE_DATEI), ordner.join(zugang::PASSWORT_DATEI), &host_pub, "Testhost")
                .mit_frist(frist);
            zugang::passwort_setzen(&ordner.join(zugang::PASSWORT_DATEI), PW).unwrap();
            let einlass = Arc::new(e);
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap().to_string();
            let (tx, ausgaenge) = mpsc::channel();
            let e2 = einlass.clone();
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(s) = s else { continue };
                    let (e, tx, hp) = (e2.clone(), tx.clone(), host_priv.clone());
                    std::thread::spawn(move || {
                        let ip = s.peer_addr().unwrap().ip();
                        let Ok(mut sock) = secure::Secure::accept(s, &noise::prologue_video(), &hp) else { return };
                        let name = zugang::nachricht3_name(&sock.nachricht3).unwrap_or_else(|| ip.to_string());
                        let a = e.pruefen(&mut sock, ip, &name);
                        if a.herein() {
                            sock.write_all(MAGIC).unwrap();
                        }
                        let _ = tx.send(a);
                        // Die Leitung bleibt offen, bis der Client geht.
                        if a.herein() {
                            let _ = sock.read_exact(&mut [0u8; 1]);
                        }
                    });
                }
            });
            Host { addr, einlass, ausgaenge, ordner }
        }

        fn ausgang(&self) -> Ausgang {
            self.ausgaenge.recv_timeout(Duration::from_secs(10)).expect("kein Ausgang")
        }

        fn liste(&self) -> Geraeteliste {
            Geraeteliste::laden(&self.ordner.join(zugang::GERAETE_DATEI)).unwrap()
        }
    }

    /// Ein neuer Client-Schluessel (privat, oeffentlich).
    fn client() -> (Vec<u8>, Vec<u8>) {
        noise::keypair().unwrap()
    }

    /// Kennung QCA1 und Nachricht 20 lesen.
    fn noetig(s: &mut Stub) -> ZugangNoetig {
        assert_eq!(&s.kennung().unwrap(), MAGIC_ZUGANG);
        match s.nachricht().unwrap() {
            Nachricht::Noetig(n) => n,
            n => panic!("statt 20: {n:?}"),
        }
    }

    fn ergebnis(s: &mut Stub) -> Ergebnis {
        match s.nachricht().unwrap() {
            Nachricht::Ergebnis(e) => e,
            n => panic!("statt 22: {n:?}"),
        }
    }

    /// Test-Haken statt Fenster: zeigt nichts, meldet gezeigte und
    /// geschlossene Anfragen ueber Kanaele.
    struct Haken {
        gezeigt: Mutex<mpsc::Sender<Anfrage>>,
        geschlossen: Mutex<mpsc::Sender<u64>>,
    }

    impl Oberflaeche for Haken {
        fn vorhanden(&self) -> bool {
            true
        }
        fn zeigen(&self, a: &Anfrage) {
            let _ = sperre(&self.gezeigt).send(a.clone());
        }
        fn schliessen(&self, nr: u64) {
            let _ = sperre(&self.geschlossen).send(nr);
        }
    }

    fn haken(e: &Einlass) -> (mpsc::Receiver<Anfrage>, mpsc::Receiver<u64>) {
        let (gt, gr) = mpsc::channel();
        let (st, sr) = mpsc::channel();
        e.oberflaeche_setzen(Arc::new(Haken { gezeigt: Mutex::new(gt), geschlossen: Mutex::new(st) }));
        (gr, sr)
    }

    /// Bekannt: sofort QCH1, keine Zugangsphase.
    #[test]
    fn bekanntes_geraet_kommt_ohne_zugang_herein() {
        let h = Host::neu("bekannt", zugang::PHASE_FRIST);
        let (cp, cpub) = client();
        let k: [u8; 32] = cpub.clone().try_into().unwrap();
        zugang::geraet_eintragen(&h.ordner.join(zugang::GERAETE_DATEI), &k, "Alt", "2026-01-02").unwrap();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        assert_eq!(&s.kennung().unwrap(), MAGIC);
        assert_eq!(h.ausgang(), Ausgang::Bekannt);
        // Der Eintrag bleibt, wie er war.
        assert_eq!(h.liste().finden(&cpub).unwrap().datum, "2026-01-02");
    }

    /// Unbekannt + richtiges Passwort: QCA1, 20 (nur Passwort, ohne
    /// Oberflaeche), Beweis, 22/0 mit einem host_proof, den der Client
    /// prueft, dann QCH1. Eingetragen mit dem Namen aus Nachricht 3; beim
    /// naechsten Mal bekannt.
    #[test]
    fn unbekannt_mit_richtigem_passwort() {
        let h = Host::neu("richtig", zugang::PHASE_FRIST);
        let (cp, cpub) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, &zugang::nachricht3("Büro-PC")).unwrap();
        let n = noetig(&mut s);
        assert_eq!(n.wege, WEG_PASSWORT, "ohne Oberflaeche kein Zulassen");
        assert_eq!(n.warten_ms, 0);
        assert_eq!(n.hostname, "Testhost");
        // Eingabe mit Grossbuchstaben und Leerzeichen: norm macht sie gleich.
        let b = s.beweis("K7M 4WQ 9TZ");
        s.senden(&Nachricht::Beweis(b)).unwrap();
        match ergebnis(&mut s) {
            Ergebnis::Passwort { host_beweis } => {
                let k = zugang::passwort_schluessel(PW, &s.host_pub);
                assert!(zugang::host_beweis_pruefen(&k, &s.hh, &host_beweis), "host_proof falsch");
                // Mit einem anderen Passwort stimmt er nicht.
                let k2 = zugang::passwort_schluessel("anderes-passwort", &s.host_pub);
                assert!(!zugang::host_beweis_pruefen(&k2, &s.hh, &host_beweis));
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(&s.kennung().unwrap(), MAGIC);
        assert_eq!(h.ausgang(), Ausgang::Passwort);
        let l = h.liste();
        let g = l.finden(&cpub).expect("nicht eingetragen");
        assert_eq!(g.name, "Büro-PC");
        assert_eq!(g.datum, zugang::heute());
        drop(s);
        // Beim naechsten Mal: bekannt.
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        assert_eq!(&s.kennung().unwrap(), MAGIC);
        assert_eq!(h.ausgang(), Ausgang::Bekannt);
    }

    /// Falsches Passwort und Drossel: Fehlversuch 1 und 2 ohne Wartezeit,
    /// ab dem dritten 5 s, dann verdoppelt; ein richtiger Beweis vor Ablauf
    /// der Wartezeit zaehlt als Fehlversuch; der fuenfte Fehlversuch der
    /// Verbindung beendet sie mit 22/4. Die Drossel gilt ueber die
    /// Verbindung hinaus (Nachricht 20 der naechsten traegt die Wartezeit).
    /// Ein alter Client (b"client") erscheint mit seiner IP.
    #[test]
    fn falsches_passwort_und_drossel() {
        let h = Host::neu("falsch", zugang::PHASE_FRIST);
        let (cp, cpub) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        let mut wartezeiten = Vec::new();
        for _ in 0..3 {
            s.senden(&Nachricht::Beweis(s.beweis("falsch-falsch"))).unwrap();
            match ergebnis(&mut s) {
                Ergebnis::Falsch { warten_ms } => wartezeiten.push(warten_ms),
                e => panic!("{e:?}"),
            }
        }
        assert_eq!(wartezeiten[..2], [0, 0]);
        assert!((4_900..=5_000).contains(&wartezeiten[2]), "{wartezeiten:?}");
        // Richtig, aber zu frueh: Fehlversuch 4, Wartezeit 10 s.
        s.senden(&Nachricht::Beweis(s.beweis(PW))).unwrap();
        match ergebnis(&mut s) {
            Ergebnis::Falsch { warten_ms } => assert!((9_900..=10_000).contains(&warten_ms), "{warten_ms}"),
            e => panic!("zu frueher Beweis: {e:?}"),
        }
        // Der fuenfte: Schluss.
        s.senden(&Nachricht::Beweis(s.beweis("falsch-falsch"))).unwrap();
        match ergebnis(&mut s) {
            Ergebnis::Schluss { warten_ms } => assert!((19_900..=20_000).contains(&warten_ms), "{warten_ms}"),
            e => panic!("{e:?}"),
        }
        assert!(s.zu(), "Leitung nach 22/4 noch offen");
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert!(!h.liste().enthaelt(&cpub));
        // Neue Verbindung: die Wartezeit steht schon in Nachricht 20.
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        let n = noetig(&mut s);
        assert!(n.warten_ms > 15_000, "{}", n.warten_ms);
        s.senden(&Nachricht::Abbruch).unwrap();
        assert!(s.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
    }

    /// Zulassen per Test-Haken: Bit 1 in Nachricht 20, die Anfrage traegt
    /// Name, ID und den Vergleichscode des Handschlags; nach "Zulassen"
    /// kommen 22/1 und QCH1, das Geraet steht in der Liste.
    #[test]
    fn zulassen_am_host() {
        let h = Host::neu("zulassen", zugang::PHASE_FRIST);
        let (gezeigt, _geschlossen) = haken(&h.einlass);
        let (cp, cpub) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, &zugang::nachricht3("Laptop")).unwrap();
        let n = noetig(&mut s);
        assert_eq!(n.wege, WEG_PASSWORT | WEG_ZULASSEN);
        let a = gezeigt.recv_timeout(Duration::from_secs(5)).expect("keine Anfrage gezeigt");
        assert_eq!((a.name.as_str(), a.id, a.code.as_str()), ("Laptop", zugang::geraete_id(&cpub), s.sas.as_str()));
        assert_eq!(h.einlass.gezeigt(), Some(a.nr));
        assert!(h.einlass.entscheiden(a.nr, true));
        assert_eq!(ergebnis(&mut s), Ergebnis::Zulassen);
        assert_eq!(&s.kennung().unwrap(), MAGIC);
        assert_eq!(h.ausgang(), Ausgang::Zugelassen);
        assert_eq!(h.liste().finden(&cpub).unwrap().name, "Laptop");
        // Eine zweite Antwort auf dieselbe Anfrage geht ins Leere.
        assert!(!h.einlass.entscheiden(a.nr, false));
        assert_eq!(h.einlass.gezeigt(), None);
    }

    /// Ablehnen: 22/3, Leitung zu, nicht eingetragen.
    #[test]
    fn ablehnen_am_host() {
        let h = Host::neu("ablehnen", zugang::PHASE_FRIST);
        let (gezeigt, _geschlossen) = haken(&h.einlass);
        let (cp, cpub) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        let a = gezeigt.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(a.name, "127.0.0.1", "alter Client: Name ist die IP");
        assert!(h.einlass.entscheiden(a.nr, false));
        assert_eq!(ergebnis(&mut s), Ergebnis::Abgelehnt);
        assert!(s.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert!(!h.liste().enthaelt(&cpub));
    }

    /// Abbruch (23) und Leitung zu ziehen die Anfrage zurueck; es wird nur
    /// eine zugleich gezeigt, die naechste rueckt nach.
    #[test]
    fn abbruch_zieht_die_anfrage_zurueck() {
        let h = Host::neu("abbruch", zugang::PHASE_FRIST);
        let (gezeigt, geschlossen) = haken(&h.einlass);
        let (cp1, _) = client();
        let (cp2, _) = client();
        let mut s1 = Stub::verbinden(&h.addr, &cp1, &zugang::nachricht3("Erster")).unwrap();
        noetig(&mut s1);
        let a1 = gezeigt.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut s2 = Stub::verbinden(&h.addr, &cp2, &zugang::nachricht3("Zweiter")).unwrap();
        noetig(&mut s2);
        // Die zweite wartet, solange die erste gezeigt wird.
        assert!(gezeigt.recv_timeout(Duration::from_millis(300)).is_err(), "zwei Anfragen zugleich gezeigt");
        assert_eq!(a1.name, "Erster");
        s1.senden(&Nachricht::Abbruch).unwrap();
        assert!(s1.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert_eq!(geschlossen.recv_timeout(Duration::from_secs(5)), Ok(a1.nr), "Fenster nicht geschlossen");
        let a2 = gezeigt.recv_timeout(Duration::from_secs(5)).expect("zweite Anfrage nicht nachgerueckt");
        assert_eq!(a2.name, "Zweiter");
        // Leitung zu (ohne 23) zieht ebenso zurueck.
        drop(s2);
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert_eq!(geschlossen.recv_timeout(Duration::from_secs(5)), Ok(a2.nr));
        assert_eq!(h.einlass.gezeigt(), None);
        // Eine verspaetete Antwort auf die zurueckgezogene geht ins Leere.
        assert!(!h.einlass.entscheiden(a1.nr, true));
    }

    /// Frist (hier 1 s statt 120 s): 22/4 ohne Wartezeit, Leitung zu,
    /// Anfrage zurueckgezogen.
    #[test]
    fn frist_beendet_die_zugangsphase() {
        let h = Host::neu("frist", Duration::from_secs(1));
        let (gezeigt, geschlossen) = haken(&h.einlass);
        let (cp, _) = client();
        let t0 = Instant::now();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        let a = gezeigt.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ergebnis(&mut s), Ergebnis::Schluss { warten_ms: 0 });
        let dauer = t0.elapsed();
        assert!(dauer >= Duration::from_millis(950) && dauer < Duration::from_secs(4), "{dauer:?}");
        assert!(s.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert_eq!(geschlossen.recv_timeout(Duration::from_secs(5)), Ok(a.nr));
    }

    /// Der Beweis ist an die Sitzung gebunden: ein mitgeschnittener
    /// client_proof gilt in einer anderen Verbindung nicht, und ein
    /// host_proof aus der einen passt nicht zum Handschlag der anderen
    /// (der Client wuerde ihn verwerfen).
    #[test]
    fn beweise_gelten_nur_fuer_ihre_sitzung() {
        let h = Host::neu("sitzung", zugang::PHASE_FRIST);
        let (cp1, _) = client();
        let mut s1 = Stub::verbinden(&h.addr, &cp1, b"client").unwrap();
        noetig(&mut s1);
        let b1 = s1.beweis(PW);
        s1.senden(&Nachricht::Beweis(b1)).unwrap();
        let hb1 = match ergebnis(&mut s1) {
            Ergebnis::Passwort { host_beweis } => host_beweis,
            e => panic!("{e:?}"),
        };
        assert_eq!(h.ausgang(), Ausgang::Passwort);
        let (cp2, cpub2) = client();
        let mut s2 = Stub::verbinden(&h.addr, &cp2, b"client").unwrap();
        noetig(&mut s2);
        let k = zugang::passwort_schluessel(PW, &s2.host_pub);
        assert!(!zugang::host_beweis_pruefen(&k, &s2.hh, &hb1), "host_proof einer anderen Sitzung akzeptiert");
        s2.senden(&Nachricht::Beweis(b1)).unwrap();
        assert!(matches!(ergebnis(&mut s2), Ergebnis::Falsch { .. }), "wiederholter Beweis angenommen");
        s2.senden(&Nachricht::Abbruch).unwrap();
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        assert!(!h.liste().enthaelt(&cpub2));
    }

    /// Grenzen: je Schluessel eine Zugangsphase, je Adresse zwei - wer
    /// darueber kommt, bekommt sofort 22/4 mit 5 s. Ein bekanntes Geraet
    /// kommt waehrenddessen ohne Warten herein (keine globale Sperre).
    #[test]
    fn grenzen_und_keine_globale_sperre() {
        let h = Host::neu("grenzen", zugang::PHASE_FRIST);
        let (cp1, _) = client();
        let (cp2, _) = client();
        let (cp3, _) = client();
        let mut s1 = Stub::verbinden(&h.addr, &cp1, b"client").unwrap();
        noetig(&mut s1);
        // Derselbe Schluessel ein zweites Mal.
        let mut doppelt = Stub::verbinden(&h.addr, &cp1, b"client").unwrap();
        assert_eq!(&doppelt.kennung().unwrap(), MAGIC_ZUGANG);
        assert_eq!(ergebnis(&mut doppelt), Ergebnis::Schluss { warten_ms: zugang::BESETZT_WARTEN_MS });
        assert!(doppelt.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        // Zweite Phase von 127.0.0.1: geht; die dritte nicht.
        let mut s2 = Stub::verbinden(&h.addr, &cp2, b"client").unwrap();
        noetig(&mut s2);
        let mut s3 = Stub::verbinden(&h.addr, &cp3, b"client").unwrap();
        assert_eq!(&s3.kennung().unwrap(), MAGIC_ZUGANG);
        assert_eq!(ergebnis(&mut s3), Ergebnis::Schluss { warten_ms: zugang::BESETZT_WARTEN_MS });
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        // Ein bekanntes Geraet kommt trotz zweier wartender Phasen sofort.
        let (bp, bpub) = client();
        let k: [u8; 32] = bpub.try_into().unwrap();
        zugang::geraet_eintragen(&h.ordner.join(zugang::GERAETE_DATEI), &k, "Bekannt", "2026-09-01").unwrap();
        let t0 = Instant::now();
        let mut b = Stub::verbinden(&h.addr, &bp, b"client").unwrap();
        assert_eq!(&b.kennung().unwrap(), MAGIC);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        assert_eq!(h.ausgang(), Ausgang::Bekannt);
        // Ist ein Platz wieder frei, geht es wieder.
        s1.senden(&Nachricht::Abbruch).unwrap();
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        let mut s3 = Stub::verbinden(&h.addr, &cp3, b"client").unwrap();
        noetig(&mut s3);
        drop((s2, s3));
    }

    /// Beschaedigte Geraeteliste: niemand ist bekannt, auch ein Geraet, das
    /// darin stuende; ein richtiges Passwort laesst es fuer diese Sitzung
    /// herein, die Datei bleibt aber unangetastet. Nach "zuruecksetzen"
    /// liegt die alte Datei daneben, und der Eintrag gelingt.
    #[test]
    fn beschaedigte_liste_heisst_niemand_bekannt() {
        let h = Host::neu("beschaedigt", zugang::PHASE_FRIST);
        let (cp, cpub) = client();
        let pfad = h.ordner.join(zugang::GERAETE_DATEI);
        let kaputt = format!("{}  2026-09-01  Alt\nkaputte Zeile\n", zugang::hex(&cpub));
        std::fs::write(&pfad, &kaputt).unwrap();
        assert!(h.einlass.geraete().is_err());
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        s.senden(&Nachricht::Beweis(s.beweis(PW))).unwrap();
        assert!(matches!(ergebnis(&mut s), Ergebnis::Passwort { .. }));
        assert_eq!(&s.kennung().unwrap(), MAGIC);
        assert_eq!(h.ausgang(), Ausgang::Passwort);
        assert_eq!(std::fs::read_to_string(&pfad).unwrap(), kaputt, "beschaedigte Liste ueberschrieben");
        h.einlass.liste_zuruecksetzen().unwrap();
        let alt: Vec<_> = std::fs::read_dir(&h.ordner)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("host-devices.txt.defekt-"))
            .collect();
        assert_eq!(alt.len(), 1);
        assert!(h.einlass.geraete().unwrap().geraete.is_empty());
    }

    /// Unlesbares Passwort: jeder Beweis ist falsch (auch der "richtige").
    #[test]
    fn ohne_passwort_nur_zulassen() {
        let h = Host::neu("ohne-passwort", zugang::PHASE_FRIST);
        std::fs::write(h.ordner.join(zugang::PASSWORT_DATEI), "kurz\n").unwrap();
        assert!(h.einlass.passwort().is_err());
        let (cp, _) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        s.senden(&Nachricht::Beweis(s.beweis("kurz"))).unwrap();
        assert!(matches!(ergebnis(&mut s), Ergebnis::Falsch { .. }));
        s.senden(&Nachricht::Abbruch).unwrap();
        assert_eq!(h.ausgang(), Ausgang::Draussen);
        // Ein neues Zufallspasswort macht es wieder moeglich.
        h.einlass.passwort_zufall().unwrap();
        let pw = h.einlass.passwort().unwrap();
        assert_eq!(pw.len(), 11);
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        s.senden(&Nachricht::Beweis(s.beweis(&pw))).unwrap();
        assert!(matches!(ergebnis(&mut s), Ergebnis::Passwort { .. }));
        assert_eq!(h.ausgang(), Ausgang::Passwort);
    }

    /// Protokollfehler: eine Nachricht, die nur der Host sendet, beendet die
    /// Phase.
    #[test]
    fn falsche_richtung_beendet_die_phase() {
        let h = Host::neu("richtung", zugang::PHASE_FRIST);
        let (cp, _) = client();
        let mut s = Stub::verbinden(&h.addr, &cp, b"client").unwrap();
        noetig(&mut s);
        s.senden(&Nachricht::Ergebnis(Ergebnis::Zulassen)).unwrap();
        assert!(s.zu());
        assert_eq!(h.ausgang(), Ausgang::Draussen);
    }
}
