// Gesicherte Leitung auf der Windows-Seite.
//
// Nach dem Handschlag laeuft der komplette bisherige Datenstrom durch diese
// Schicht: Stueck fuer Stueck verschluesselt, auf der anderen Seite wieder
// zusammengesetzt. Aufrufer sehen nur read_exact und write_all wie vorher.

use crate::noise;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const CHUNK_MAX: usize = 65519;

/// Was an Leitung und Ablage schiefgehen kann, so dass es der Nutzer zu
/// sehen bekommt. Die Oberflaeche macht daraus einen Satz in seiner Sprache
/// (main.rs, `Meldung`); `Display` ist der deutsche Text mit allen
/// Einzelheiten - fuers Protokoll, und fuer die Host-Rolle, die nur
/// protokolliert.
#[derive(Clone, Debug, PartialEq)]
pub enum Fehler {
    /// Kein Ablageordner (APPDATA/HOME fehlt, Ordner nicht anzulegen).
    Ablage(String),
    /// Schluesseldatei vorhanden, aber mit falscher Laenge (weder 32 noch 64
    /// Byte) - oder mit 64 Byte, deren hinterer Teil nicht der oeffentliche
    /// Schluessel zum vorderen ist.
    SchluesselBeschaedigt { pfad: PathBuf, laenge: usize },
    /// Datei vorhanden, aber nicht lesbar (Rechte, Sperre durch ein
    /// anderes Programm). `grund` ist der Wortlaut des Systems.
    Unlesbar { pfad: PathBuf, grund: String },
    /// Vertrauensliste vorhanden, aber kein UTF-8 (etwa als ANSI gespeichert).
    KeinUtf8 { pfad: PathBuf },
    /// Schreiben gescheitert (neuer Schluessel, neuer Eintrag).
    Schreiben { pfad: PathBuf, grund: String },
    /// Verbunden ueber eine Geraete-ID (Liste, Eingabe, Verknuepfung), aber
    /// der Schluessel der Gegenstelle ergibt eine andere ID (Spezifikation
    /// Pairing v1, 8.2) - oder dieselbe, doch zu ihr ist in hosts.txt ein
    /// anderer voller Schluessel gemerkt (`erwartet == gemeldet`: eine
    /// errechnete Kollision). Geprueft nach Nachricht 2, vor Nachricht 3.
    AnderesGeraet { addr: String, erwartet: u32, gemeldet: u32 },
    /// Unter der Adresse antwortet dieser Rechner selbst: der Schluessel der
    /// Gegenstelle ist der eigene host.key. Geprueft nach Nachricht 2, vor
    /// Nachricht 3 - kein Verbinden zu sich selbst.
    EigenerHost { addr: String },
    /// Die Adresse ergibt kein Ziel; `grund` ist der Wortlaut des Systems,
    /// None: aufgeloest, aber ohne eine einzige Adresse.
    Adresse { addr: String, grund: Option<String> },
    /// Die Verbindung kam nicht zustande; `art` sagt, warum (abgelehnt,
    /// keine Antwort, Netz nicht erreichbar ...).
    Verbindung { addr: String, art: std::io::ErrorKind, grund: String },
    /// Der Handschlag scheiterte; `frist`: die Gegenstelle hat nicht
    /// rechtzeitig geantwortet. `system` ist der Wortlaut des Systems, wenn
    /// die Leitung selbst scheiterte (zurueckgesetzt, abgebrochen) - der
    /// Anhang der Meldung; `grund` sagt dazu, an welcher Stelle.
    Handschlag { grund: String, frist: bool, system: Option<String> },
}

impl std::fmt::Display for Fehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fehler::Ablage(g) => write!(f, "{g}"),
            Fehler::SchluesselBeschaedigt { pfad, laenge: 64 } => write!(
                f,
                "{} ist beschaedigt (64 Byte, aber der oeffentliche Teil passt nicht zum privaten) und wird \
                 nicht ueberschrieben. Datei pruefen oder loeschen - dann entsteht ein neuer Schluessel.",
                pfad.display()
            ),
            Fehler::SchluesselBeschaedigt { pfad, laenge } => write!(
                f,
                "{} ist beschaedigt ({laenge} Byte statt 32 oder 64) und wird nicht ueberschrieben. \
                 Datei pruefen oder loeschen - dann entsteht ein neuer Schluessel.",
                pfad.display()
            ),
            Fehler::Unlesbar { pfad, grund } => write!(f, "{} nicht lesbar: {grund}", pfad.display()),
            Fehler::KeinUtf8 { pfad } => write!(f, "{} ist kein UTF-8", pfad.display()),
            Fehler::Schreiben { pfad, grund } => write!(f, "{} nicht zu schreiben: {grund}", pfad.display()),
            Fehler::AnderesGeraet { addr, erwartet, gemeldet } if erwartet == gemeldet => write!(
                f,
                "An {addr} antwortet ein anderes Geraet: ID {} wie gewaehlt, aber nicht der dazu in hosts.txt \
                 gemerkte Schluessel (moeglicher Angriff) - Abbruch vor Nachricht 3",
                crate::zugang::id_text(*erwartet)
            ),
            Fehler::AnderesGeraet { addr, erwartet, gemeldet } => write!(
                f,
                "An {addr} antwortet ein anderes Geraet (ID {} statt der gewaehlten {}) - Abbruch vor Nachricht 3",
                crate::zugang::id_text(*gemeldet),
                crate::zugang::id_text(*erwartet)
            ),
            Fehler::EigenerHost { addr } => write!(
                f,
                "An {addr} antwortet dieser Rechner selbst (Schluessel = eigener host.key) - Abbruch vor Nachricht 3, \
                 kein Verbinden zu sich selbst"
            ),
            Fehler::Adresse { addr, grund: Some(g) } => write!(f, "Adresse {addr}: {g}"),
            Fehler::Adresse { addr, grund: None } => write!(f, "Adresse {addr} ergibt kein Ziel"),
            Fehler::Verbindung { addr, grund, .. } => write!(f, "Verbindung zu {addr}: {grund}"),
            Fehler::Handschlag { grund, .. } => write!(f, "Handschlag: {grund}"),
        }
    }
}

/// Wer nur protokolliert (Host-Rolle, Freigabeliste), nimmt den Text.
impl From<Fehler> for String {
    fn from(f: Fehler) -> String {
        f.to_string()
    }
}

/// Der Wortlaut des Systems zu einem Fehler, in einer Zeile: Windows bricht
/// lange Meldungen mit CRLF um ("... wurde softwaregesteuert\r\ndurch den
/// Hostcomputer abgebrochen."), und so stuenden sie zerrissen im Protokoll
/// und in der Oberflaeche.
fn wortlaut(e: &std::io::Error) -> String {
    e.to_string().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Woran ein Handschlag erkennt, dass seine Frist ablief (siehe `Rahmen`).
const FRIST_ABGELAUFEN: &str = "Frist fuer den Handschlag abgelaufen";

/// Frist fuer den GANZEN Handschlag, nicht je Leseaufruf. Eine Frist je
/// Aufruf haelt ein Gegenueber, das alle zwei Sekunden ein Byte schickt,
/// jedes Mal ein - und bindet den Annehmenden damit stundenlang. Der
/// Anrufer liest nur Nachricht 2 und bleibt bei den bisherigen 3 s; der
/// Angerufene liest zwei Nachrichten mit einem Hin und Her dazwischen.
const FRIST_ANRUFER: Duration = Duration::from_secs(3);
#[cfg_attr(not(windows), allow(dead_code))]
const FRIST_ANNAHME: Duration = Duration::from_secs(5);

/// Rahmen des Handschlags: 2 Byte Laenge, dann die Nachricht. Alle Lese-
/// und Schreibaufrufe teilen sich eine Frist; jeder bekommt nur die Restzeit.
struct Rahmen<'a> {
    sock: &'a TcpStream,
    frist: Instant,
    /// Wortlaut des Systems, wenn Lesen oder Schreiben an der Leitung
    /// scheiterte (siehe Fehler::Handschlag).
    system: RefCell<Option<String>>,
}

impl<'a> Rahmen<'a> {
    fn neu(sock: &'a TcpStream, frist: Duration) -> Rahmen<'a> {
        Rahmen { sock, frist: Instant::now() + frist, system: RefCell::new(None) }
    }

    fn rest(&self) -> Result<Duration, String> {
        let r = self.frist.saturating_duration_since(Instant::now());
        if r.is_zero() {
            return Err(FRIST_ABGELAUFEN.into());
        }
        Ok(r)
    }

    /// Wie read_exact, aber vor jedem Stueck nur noch mit der Restzeit.
    fn lesen(&self, buf: &mut [u8], was: &str) -> Result<(), String> {
        let mut fertig = 0;
        while fertig < buf.len() {
            self.sock.set_read_timeout(Some(self.rest()?)).ok();
            match (&*self.sock).read(&mut buf[fertig..]) {
                Ok(0) => return Err(format!("{was}: Leitung zu")),
                Ok(n) => fertig += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err(format!("{was}: {FRIST_ABGELAUFEN}"))
                }
                Err(e) => {
                    let w = wortlaut(&e);
                    self.system.replace(Some(w.clone()));
                    return Err(format!("{was}: {w}"));
                }
            }
        }
        Ok(())
    }

    fn recv(&self, buf: &mut Vec<u8>) -> Result<(), String> {
        let mut l = [0u8; 2];
        self.lesen(&mut l, "Laenge")?;
        let n = u16::from_le_bytes(l) as usize;
        buf.resize(n, 0);
        self.lesen(buf, "Daten")
    }

    fn send(&self, data: &[u8]) -> Result<(), String> {
        let mut out = Vec::with_capacity(2 + data.len());
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        self.sock.set_write_timeout(Some(self.rest()?)).ok();
        (&*self.sock).write_all(&out).map_err(|e| {
            let w = wortlaut(&e);
            self.system.replace(Some(w.clone()));
            format!("Senden: {w}")
        })
    }
}

/// `buf` ganz von der Leitung lesen - der eine Lesepfad unter allen
/// Leseaufrufen von Secure (`lesen`, `read_exact`, `read_exact_bis`).
/// Ohne Frist (`bis` None) gilt die Lesefrist der Leitung (`lesefrist`), je
/// Leseaufruf. Mit Frist gilt EINE fuer das ganze Lesen: vor jedem Stueck
/// nur noch die Restzeit, wie `Rahmen::lesen` im Handschlag - sonst hielte
/// eine Gegenstelle, die alle paar Sekunden ein Byte schickt, den Leser
/// beliebig lange fest. Die Art des Fehlers bleibt erhalten: UnexpectedEof
/// heisst Leitung zu, WouldBlock/TimedOut Frist abgelaufen.
fn leitung_lesen(sock: &TcpStream, buf: &mut [u8], bis: Option<Instant>, was: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind as E};
    let abgelaufen = || Error::new(E::TimedOut, format!("{was}: Lesefrist abgelaufen"));
    let mut fertig = 0;
    while fertig < buf.len() {
        if let Some(bis) = bis {
            let rest = bis.saturating_duration_since(Instant::now());
            if rest.is_zero() {
                return Err(abgelaufen());
            }
            sock.set_read_timeout(Some(rest.max(Duration::from_millis(1)))).ok();
        }
        match (&*sock).read(&mut buf[fertig..]) {
            Ok(0) => return Err(Error::new(E::UnexpectedEof, format!("{was}: Leitung zu"))),
            Ok(n) => fertig += n,
            Err(e) if e.kind() == E::Interrupted => {}
            Err(e) if bis.is_some() && matches!(e.kind(), E::WouldBlock | E::TimedOut) => return Err(abgelaufen()),
            Err(e) => return Err(Error::new(e.kind(), format!("{was}: {}", wortlaut(&e)))),
        }
    }
    Ok(())
}

pub struct Secure {
    sock: TcpStream,
    tx: snow::TransportState,
    inbuf: Vec<u8>,
    inpos: usize,
    /// Pruefsumme des Handschlags - bindet den Eingabekanal an den Bildkanal.
    pub handshake_hash: Vec<u8>,
    pub peer: Vec<u8>,
    pub sas: String,
    /// Nutzlast von Handschlag-Nachricht 3 (nur beim Angerufenen, also in
    /// der Host-Rolle): der Name des Clients (zugang::nachricht3_name) und
    /// seine Flags (zugang::nachricht3_flags).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub nachricht3: Vec<u8>,
    /// Die Flags, die dieser Anrufer selbst in Nachricht 3 gesendet hat
    /// (NAME_FLAG_*; beim Angerufenen 0) - Bit 0: er kennt den Schluessel
    /// der Gegenstelle nicht, sie muss sich in der Zugangsphase ausweisen.
    pub flags3: u8,
}

impl Secure {
    /// Verbindet und fuehrt den Handschlag als Anrufer, OHNE den Schluessel
    /// der Gegenstelle zu pruefen - nur fuer Pruefstaende (Tests hier und in
    /// host/netz.rs). Der Client selbst verbindet ueber `connect_pruefend`.
    #[cfg(test)]
    pub fn connect(addr: &str, prologue: &[u8]) -> Result<Secure, Fehler> {
        Secure::connect_pruefend(addr, prologue, |_| Ok(0))
    }

    /// Verbindet und fuehrt den Handschlag als Anrufer. `pruefen` sieht den
    /// Schluessel der Gegenstelle nach Nachricht 2 und VOR Nachricht 3 (siehe
    /// noise::handshake_initiator): scheitert sie, geht der eigene Schluessel
    /// nie hinaus, die Leitung faellt zu, und ihr Fehler kommt unveraendert
    /// zurueck. Der Host sieht dann nur einen abgebrochenen Handschlag -
    /// angenommen, und damit ein laufender Zuschauer abgeloest, wird dort
    /// erst nach Nachricht 3. Gelingt sie, liefert sie die Flags fuer
    /// Nachricht 3 (NAME_FLAG_*, neben dem eigenen Namen; `flags3`).
    pub fn connect_pruefend(
        addr: &str,
        prologue: &[u8],
        pruefen: impl FnOnce(&[u8]) -> Result<u8, Fehler>,
    ) -> Result<Secure, Fehler> {
        // Der eigene Schluessel zuerst: ist der Geraeteschluessel (host.key)
        // beschaedigt oder die Ablage weg, geht gar nicht erst eine Leitung
        // auf - sonst oeffnete jeder Versuch eine leere Verbindung, und der
        // Host protokollierte sie.
        let (priv_key, _pub_key) = identity()?;
        // Mit Frist verbinden. Ohne sie haengt ein Aufruf an einer toten
        // Adresse gut zwanzig Sekunden.
        let adresse = |grund: Option<String>| Fehler::Adresse { addr: addr.to_string(), grund };
        let ziel = addr
            .to_socket_addrs()
            .map_err(|e| adresse(Some(wortlaut(&e))))?
            .next()
            .ok_or_else(|| adresse(None))?;
        let sock = TcpStream::connect_timeout(&ziel, Duration::from_secs(2)).map_err(|e| Fehler::Verbindung {
            addr: addr.to_string(),
            art: e.kind(),
            grund: wortlaut(&e),
        })?;
        sock.set_nodelay(true).ok();
        Secure::handschlag(sock, prologue, &priv_key, pruefen)
    }

    fn handschlag(
        sock: TcpStream,
        prologue: &[u8],
        priv_key: &[u8],
        pruefen: impl FnOnce(&[u8]) -> Result<u8, Fehler>,
    ) -> Result<Secure, Fehler> {
        // Waehrend des Handschlags gilt eine Frist. Danach wird sie wieder
        // aufgehoben: der Bildkanal darf beliebig lange still sein, ohne dass
        // daraus ein Fehler wird.
        let r = Rahmen::neu(&sock, FRIST_ANRUFER);
        // Der Handschlag kennt nur Text; der Fehler der Pruefung (Pin,
        // Ablage) wartet hier, damit er mit seiner Art zurueckkommt.
        let mut pruef_fehler: Option<Fehler> = None;
        let mut flags3 = 0;
        let s = noise::handshake_initiator(priv_key, prologue, |b| r.recv(b), |d| r.send(d), |rs| match pruefen(rs) {
            Ok(f) => {
                flags3 = f;
                Ok(eigene_nachricht3(f))
            }
            Err(f) => {
                let text = f.to_string();
                pruef_fehler = Some(f);
                Err(text)
            }
        });
        let s = match s {
            Ok(s) => s,
            Err(grund) => {
                return Err(match pruef_fehler {
                    Some(f) => f,
                    None => Fehler::Handschlag { frist: grund.contains(FRIST_ABGELAUFEN), system: r.system.take(), grund },
                })
            }
        };
        sock.set_read_timeout(None).ok();
        sock.set_write_timeout(None).ok();
        let sas = noise::sas(&s.handshake_hash);
        Ok(Secure {
            sock,
            tx: s.transport,
            inbuf: Vec::new(),
            inpos: 0,
            handshake_hash: s.handshake_hash,
            peer: s.remote_static,
            sas,
            nachricht3: Vec::new(),
            flags3,
        })
    }

    /// Nimmt eine angenommene Leitung als Angerufener (Host-Rolle) an: der
    /// Handschlag laeuft mit dem uebergebenen dauerhaften Schluessel des
    /// Hosts (dem Geraeteschluessel). Rahmen wie bei `wrap`; die Frist
    /// (FRIST_ANNAHME) gilt fuer den ganzen Handschlag.
    pub fn accept(sock: TcpStream, prologue: &[u8], priv_key: &[u8]) -> Result<Secure, String> {
        sock.set_nodelay(true).ok();
        let r = Rahmen::neu(&sock, FRIST_ANNAHME);
        let s = noise::handshake_responder(priv_key, prologue, |b| r.recv(b), |d| r.send(d))?;
        sock.set_read_timeout(None).ok();
        sock.set_write_timeout(None).ok();
        let sas = noise::sas(&s.handshake_hash);
        Ok(Secure {
            sock,
            tx: s.transport,
            inbuf: Vec::new(),
            inpos: 0,
            handshake_hash: s.handshake_hash,
            peer: s.remote_static,
            sas,
            nachricht3: s.nachricht3,
            flags3: 0,
        })
    }

    /// Die Leitung selbst - fuer Socketoptionen der Host-Rolle (Sendepuffer).
    pub fn socket(&self) -> &TcpStream {
        &self.sock
    }

    /// Liegt schon entschluesselter, noch nicht gelesener Klartext bereit?
    /// Dann kommt das Lesen ohne die Leitung aus (siehe `bereit`).
    fn gepuffert(&self) -> bool {
        self.inpos < self.inbuf.len()
    }

    /// Wie `read_exact`, aber mit EINER Frist fuer das ganze Lesen (`bis`)
    /// statt einer je Leseaufruf (siehe `leitung_lesen`) - fuer die
    /// Zugangsphase der Host-Rolle (host/einlass.rs), in der ein
    /// unbeglaubigtes Gegenueber sonst mit einzelnen Bytes die Gesamtfrist
    /// aushebeln koennte. Nach einem Fehler taugt die Leitung nur noch zum
    /// Schliessen (ein Datensatz kann halb gelesen sein). Die Lesefrist der
    /// Leitung setzt der Aufrufer danach selbst.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn read_exact_bis(&mut self, dst: &mut [u8], bis: Instant) -> Result<(), String> {
        self.lesen_bis(dst, Some(bis)).map_err(|e| e.to_string())
    }

    /// Eine zweite Hand an derselben Leitung, um sie von aussen zu kappen.
    /// Ein blockierendes Lesen endet erst, wenn jemand den Socket schliesst.
    pub fn abbruchgriff(&self) -> Option<TcpStream> {
        self.sock.try_clone().ok()
    }

    pub fn peer_fingerprint(&self) -> String {
        noise::fingerprint(&self.peer)
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        let mut out = Vec::with_capacity(data.len() + 2 + 16);
        let mut buf = vec![0u8; CHUNK_MAX + 16];
        for chunk in data.chunks(CHUNK_MAX) {
            let n = self
                .tx
                .write_message(chunk, &mut buf)
                .map_err(|e| format!("Verschluesseln: {e:?}"))?;
            out.extend_from_slice(&(n as u16).to_le_bytes());
            out.extend_from_slice(&buf[..n]);
        }
        (&self.sock).write_all(&out).map_err(|e| format!("Senden: {}", wortlaut(&e)))
    }

    pub fn read_exact(&mut self, dst: &mut [u8]) -> Result<(), String> {
        self.lesen(dst).map_err(|e| e.to_string())
    }

    /// Frist fuer jedes folgende Lesen an der Leitung (None: ohne). Die
    /// Antwort des Hosts nach dem Handschlag und die Zugangsphase lesen mit
    /// Frist, die Sitzung danach wieder ohne - der Bildkanal darf beliebig
    /// lange still sein.
    pub fn lesefrist(&self, frist: Option<Duration>) {
        self.sock.set_read_timeout(frist).ok();
    }

    /// Liegt etwas zum Lesen an? true: entschluesselte Bytes warten schon,
    /// oder an der Leitung kommt binnen `warten` etwas an - dann liest
    /// `lesen` die Nachricht (mit der Frist aus `lesefrist`). false: in der
    /// Zeit kam nichts. Err: Leitung zu (UnexpectedEof) oder gestoert. Es
    /// wird nur hineingeschaut (peek): eine Wartezeit, die ablaeuft, zerreisst
    /// nie einen Datensatz - so kann die Zugangsphase zwischendurch nach dem
    /// Nutzer sehen, ohne einen eigenen Lesefaden.
    pub fn bereit(&self, warten: Duration) -> std::io::Result<bool> {
        use std::io::ErrorKind as E;
        if self.gepuffert() {
            return Ok(true);
        }
        let vorher = self.sock.read_timeout().ok().flatten();
        self.sock.set_read_timeout(Some(warten.max(Duration::from_millis(1))))?;
        let mut b = [0u8; 1];
        let r = self.sock.peek(&mut b);
        self.sock.set_read_timeout(vorher).ok();
        match r {
            Ok(0) => Err(std::io::Error::new(E::UnexpectedEof, "Leitung zu")),
            Ok(_) => Ok(true),
            Err(e) if matches!(e.kind(), E::WouldBlock | E::TimedOut | E::Interrupted) => Ok(false),
            Err(e) => Err(std::io::Error::new(e.kind(), wortlaut(&e))),
        }
    }

    /// Wie `read_exact`, aber mit der Art des Fehlers: UnexpectedEof heisst
    /// Leitung zu, WouldBlock bzw. TimedOut Frist abgelaufen (`lesefrist`),
    /// InvalidData ein Datensatz, der nicht echt oder unplausibel ist. Der
    /// Text ist derselbe wie bei `read_exact`.
    pub fn lesen(&mut self, dst: &mut [u8]) -> std::io::Result<()> {
        self.lesen_bis(dst, None)
    }

    /// Entschluesseln und in `dst` verteilen - fuer alle Leseaufrufe gleich;
    /// die Frist (None: die der Leitung, sonst eine fuer das Ganze) haengt
    /// an `leitung_lesen`.
    fn lesen_bis(&mut self, dst: &mut [u8], bis: Option<Instant>) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind};
        let mut done = 0;
        while done < dst.len() {
            if !self.gepuffert() {
                let mut l = [0u8; 2];
                leitung_lesen(&self.sock, &mut l, bis, "Laenge")?;
                let n = u16::from_le_bytes(l) as usize;
                if n > CHUNK_MAX + 16 {
                    return Err(Error::new(ErrorKind::InvalidData, "unplausible Datensatzlaenge"));
                }
                let mut ct = vec![0u8; n];
                leitung_lesen(&self.sock, &mut ct, bis, "Daten")?;
                let mut pt = vec![0u8; CHUNK_MAX];
                let got = self
                    .tx
                    .read_message(&ct, &mut pt)
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "Datensatz nicht echt - abgebrochen"))?;
                pt.truncate(got);
                self.inbuf = pt;
                self.inpos = 0;
                if got == 0 {
                    continue;
                }
            }
            let take = (self.inbuf.len() - self.inpos).min(dst.len() - done);
            dst[done..done + take].copy_from_slice(&self.inbuf[self.inpos..self.inpos + take]);
            self.inpos += take;
            done += take;
        }
        Ok(())
    }
}

/// Nachricht 3 ("QCN1", Spezifikation Pairing v1, 1.4) mit dem Namen dieses
/// Geraets - der Host zeigt ihn im Zulassen-Fenster und in seiner
/// Geraeteliste; unbeglaubigt, nur Anzeige. Der Name wird bei jedem Aufbau
/// (auch dem des Eingabekanals) neu gelesen: ein im Fenster "Geraetename"
/// geaenderter gilt ab der naechsten Verbindung, ohne Neustart
/// (zugang::geraetename). Die Flags haengen vom Schluessel der Gegenstelle
/// ab (connect_pruefend).
fn eigene_nachricht3(flags: u8) -> Vec<u8> {
    crate::zugang::nachricht3(&crate::zugang::geraetename(), flags)
}

// ------------------------------------------------------------ Schluesselablage

/// Der Ordner fuer alles, was die App dauerhaft ablegt: host.key (der
/// Geraeteschluessel beider Rollen), hosts.txt, einstellungen.txt,
/// protokoll.txt, benchmark.txt, dazu die Dateien der Host-Rolle
/// (host-devices.txt, host-password.txt, host-protokoll.txt). Ein frueherer
/// client.key bleibt dort unbenutzt liegen.
/// Windows: %APPDATA%\QuadChroma (ohne APPDATA: unter HOME).
/// macOS: ~/Library/Application Support/QuadChroma - derselbe Ordner fuer
/// die Host-Engine (qc_secure.c).
///
/// Rechte: Unter Unix wird ein neu angelegter Ordner nur fuer den Eigentuemer
/// geoeffnet (0700, wie beim Mac-Host); einen schon vorhandenen laesst der
/// Client, wie er ist. Unter Windows genuegt die vom Profilordner geerbte
/// Zugriffsliste (SYSTEM, Administratoren, der Nutzer) - eine eigene,
/// geschuetzte Liste braechte dort nichts ausser eigenem Fehlerrisiko.
pub fn config_dir() -> Result<PathBuf, String> {
    let dir = basis_ordner()?.join("QuadChroma");
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&dir).map_err(|e| format!("Ordner: {}", wortlaut(&e)))?;
    Ok(dir)
}

/// Tests fassen die echte Ablage nie an: auf dem Mac laege dort der
/// Schluessel des laufenden Hosts, und protokoll.txt wuerde geleert.
#[cfg(test)]
fn basis_ordner() -> Result<PathBuf, String> {
    Ok(std::env::temp_dir().join(test_lauf()))
}

/// Name des Testordners dieses Laufs: "qc-test-<pid>-<Startzeit in ms>".
/// Nur die pid reichte nicht: die Ordner bleiben liegen, Windows vergibt
/// pids neu, und ein neuer Lauf mit derselben pid fand dann eine alte
/// Vertrauensliste (damals known_hosts.txt) mit einem anderen Schluessel
/// fuer 127.0.0.1 - die Loopback-Tests scheiterten am Pin. Einmal je Prozess
/// bestimmt, damit alle Tests eines Laufs denselben Ordner teilen
/// (Testschluessel, hosts.txt).
#[cfg(test)]
pub fn test_lauf() -> &'static str {
    static LAUF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LAUF.get_or_init(|| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("qc-test-{}-{ms}", std::process::id())
    })
}

/// Ein eigener, frischer Ordner fuer einen Test: "<Lauf>-<bereich>-<name>-<n>"
/// unter dem Temp-Ordner, mit einer laufenden Nummer je Aufruf. So teilen
/// nebeneinander laufende Tests nie eine Datei (host-password.txt,
/// host-devices.txt, host.key) - auch nicht, wenn zwei denselben Namen
/// waehlten -, und nichts muss vorher geloescht werden: unter Windows bliebe
/// ein eben geloeschter Ordner sonst mitunter kurz "zum Loeschen vorgemerkt",
/// und Schreiben darin scheiterte mit "Zugriff verweigert" (os error 5).
#[cfg(test)]
pub fn test_ordner(bereich: &str, name: &str) -> PathBuf {
    static NUMMER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NUMMER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("{}-{bereich}-{name}-{n}", test_lauf()));
    std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display()));
    d
}

#[cfg(all(not(test), target_os = "macos"))]
fn basis_ordner() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "kein Ablageort fuer Einstellungen gefunden".to_string())?;
    Ok(PathBuf::from(home).join("Library").join("Application Support"))
}

#[cfg(all(not(test), not(target_os = "macos")))]
fn basis_ordner() -> Result<PathBuf, String> {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "kein Ablageort fuer Einstellungen gefunden".to_string())?;
    Ok(PathBuf::from(base))
}

/// Der Geraeteschluessel: EIN dauerhafter Schluessel je Rechner fuer beide
/// Rollen. Mit ihm nimmt die Host-Rolle Verbindungen an, und mit ihm ruft
/// der Client an - so hat ein Rechner genau eine Geraete-ID, dieselbe in der
/// Bekanntgabe, in "Dieser Computer" und in der Liste der erlaubten Geraete
/// eines anderen Rechners. Der Mac-Host (qc_secure.c) schreibt ihn mit 32
/// Byte, Rust mit 64; beide lesen beide Formen.
pub const GERAETESCHLUESSEL: &str = "host.key";

/// Der fruehere eigene Schluessel der Client-Rolle (bis zur einen App hatte
/// ein Rechner zwei Identitaeten). Er bleibt liegen und wird weder benutzt
/// noch geloescht noch umgeschrieben - nur gelesen, um nach dem Wechsel
/// einen Hinweis zu geben (`frueherer_client_schluessel_abweichend`).
pub const FRUEHERER_CLIENT_SCHLUESSEL: &str = "client.key";

/// Die Datei, mit deren Schluessel dieser Prozess als Client anruft. Im
/// Betrieb der Geraeteschluessel. In Tests eine eigene Datei: Client und
/// Host-Rolle eines Testlaufs teilen sich die Ablage und sollen zwei Geraete
/// sein - mit einem Schluessel griffe der Selbstschutz (EigenerHost beim
/// Anrufer, der Einlass weist den eigenen Schluessel ab).
#[cfg(not(test))]
const ANRUF_SCHLUESSEL: &str = GERAETESCHLUESSEL;
#[cfg(test)]
const ANRUF_SCHLUESSEL: &str = "test-client.key";

/// Dauerhafter eigener Schluessel, mit dem der Client anruft: der
/// Geraeteschluessel (host.key). Entsteht beim ersten Start - durch die
/// Rolle, die ihn zuerst braucht - und wird nie neu geschrieben.
pub fn identity() -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    // Im Ablauf stehen privater und oeffentlicher Teil hintereinander, damit
    // beim Start nichts nachgerechnet werden muss.
    schluessel_laden(&config_dir().map_err(Fehler::Ablage)?.join(ANRUF_SCHLUESSEL))
}

/// Liegt noch ein frueherer client.key, dessen Schluessel NICHT der
/// Geraeteschluessel ist? Dann kannten andere Rechner dieses Geraet unter dem
/// alten Schluessel und fragen einmal neu nach Passwort oder Zulassen - der
/// Zugangsdialog sagt es dazu. Fehlt eine der Dateien oder taugt sie nicht:
/// false. Nur gelesen, nie angelegt.
pub fn frueherer_client_schluessel_abweichend() -> bool {
    config_dir().is_ok_and(|d| alter_schluessel_abweichend(&d))
}

/// Wie `frueherer_client_schluessel_abweichend`, fuer einen Ordner.
fn alter_schluessel_abweichend(ordner: &Path) -> bool {
    match (
        oeffentlich_lesen(&ordner.join(FRUEHERER_CLIENT_SCHLUESSEL)),
        oeffentlich_lesen(&ordner.join(GERAETESCHLUESSEL)),
    ) {
        (Some(alt), Some(geraet)) => alt != geraet,
        _ => false,
    }
}

/// Liest einen Schluessel (siehe `schluessel_lesen_mit`: 64 Byte privat und
/// oeffentlich, oder 32 Byte nur privat) oder legt ihn mit 64 Byte an - aber
/// NUR, wenn die Datei fehlt. Eine vorhandene Datei wird nie neu
/// geschrieben: ist sie unlesbar oder hat sie die falsche Laenge, bleibt sie
/// liegen - ein stilles Ueberschreiben machte aus einem Dateifehler eine
/// neue Identitaet, und jede Kopplung waere weg.
fn schluessel_laden(path: &Path) -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    schluessel_laden_mit(path, noise::keypair)
}

/// Wie `schluessel_laden`; `erzeugen` liefert das neue Paar. Legt ein zweiter
/// Prozess die Datei an, waehrend dieser hier erzeugt (gleichzeitiger
/// Erststart mit derselben Ablage), gilt dessen Schluessel: der eigene
/// stuende nirgends, und eine Kopplung damit waere beim naechsten Start weg.
fn schluessel_laden_mit(
    path: &Path,
    erzeugen: impl FnOnce() -> Result<(Vec<u8>, Vec<u8>), String>,
) -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    if let Some(k) = schluessel_lesen(path)? {
        return Ok(k);
    }
    let schreiben = |grund: String| Fehler::Schreiben { pfad: path.to_path_buf(), grund };
    let (priv_key, pub_key) = erzeugen().map_err(schreiben)?;
    let mut both = priv_key.clone();
    both.extend_from_slice(&pub_key);
    match geheim_schreiben(path, &both) {
        Ok(()) => Ok((priv_key, pub_key)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            schluessel_lesen(path)?.ok_or_else(|| schreiben(wortlaut(&e)))
        }
        Err(e) => Err(schreiben(wortlaut(&e))),
    }
}

/// Liest eine Schluesseldatei. None: sie fehlt.
fn schluessel_lesen(path: &Path) -> Result<Option<(Vec<u8>, Vec<u8>)>, Fehler> {
    schluessel_lesen_mit(path, || std::thread::sleep(Duration::from_millis(200)))
}

/// Wie `schluessel_lesen`. Zwei Formen gelten: 64 Byte (privat, dann
/// oeffentlich - so legt Rust host.key an, und so lag frueher client.key)
/// und 32 Byte (nur privat - so schreibt der Mac-Host host.key,
/// qc_secure.c); zu 32 Byte wird der oeffentliche errechnet, die Datei
/// bleibt, wie sie ist. Bei 64 Byte muss der hintere Teil der oeffentliche
/// zum vorderen sein (wie im Lader des Mac-Hosts) - sonst ist die Datei
/// beschaedigt und bleibt liegen. Ist die Datei sonst zu KURZ, wird nach `warten`
/// einmal neu gelesen, bevor sie als beschaedigt gilt: auf einem
/// Dateisystem ohne harte Verweise legt ein zweiter Prozess sie direkt an
/// (siehe `geheim_schreiben`), und wer genau dann liest, saehe sie halb.
/// Sieht er dabei genau die ersten 32 Byte, ist das schon der ganze private
/// Schluessel - das Paar stimmt also auch dann. Zu lang wird sie nie - das
/// ist gleich ein Fehler.
fn schluessel_lesen_mit(path: &Path, warten: impl FnOnce()) -> Result<Option<(Vec<u8>, Vec<u8>)>, Fehler> {
    let lesen = || match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Fehler::Unlesbar { pfad: path.to_path_buf(), grund: wortlaut(&e) }),
    };
    let mut b = lesen()?;
    if b.as_ref().is_some_and(|b| b.len() < 64 && b.len() != 32) {
        warten();
        b = lesen()?;
    }
    let beschaedigt = |laenge| Fehler::SchluesselBeschaedigt { pfad: path.to_path_buf(), laenge };
    match b {
        None => Ok(None),
        Some(b) if b.len() == 64 => match noise::oeffentlich(&b[..32]) {
            Some(oeff) if oeff == b[32..] => Ok(Some((b[..32].to_vec(), oeff))),
            _ => Err(beschaedigt(64)),
        },
        Some(b) if b.len() == 32 => match noise::oeffentlich(&b) {
            Some(oeff) => Ok(Some((b, oeff))),
            None => Err(beschaedigt(32)),
        },
        Some(b) => Err(beschaedigt(b.len())),
    }
}

/// Legt eine Schluesseldatei an: erst vollstaendig unter einem Zwischennamen,
/// dann per hartem Verweis unter dem Zielnamen - ein Abbruch mittendrin
/// hinterlaesst nie einen halben Schluessel, und ein vorhandener wird nie
/// ersetzt: der harte Verweis scheitert dann mit AlreadyExists (wie O_EXCL
/// beim Mac-Host). Ein Umbenennen ersetzte eine Datei, die ein zweiter
/// Prozess inzwischen angelegt hat. Unter Unix nur fuer den Eigentuemer
/// lesbar (0600, wie host.key des Mac-Hosts); unter Windows gilt die geerbte
/// Liste (siehe config_dir). Der Zwischenname ist je Aufruf eigen (pid und
/// laufende Nummer): in der einen App brauchen Client und Host-Rolle den
/// Geraeteschluessel womoeglich zugleich - mit einem Namen je Prozess
/// loeschte der eine Faden die Zwischendatei des anderen, und der harte
/// Verweis zeigte auf einen Schluessel, den keiner der beiden in der Hand
/// haelt.
fn geheim_schreiben(path: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    static NUMMER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nr = NUMMER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_file_name(format!("{name}.{}-{nr}.neu", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let r = exklusiv_schreiben(&tmp, inhalt).and_then(|_| match std::fs::hard_link(&tmp, path) {
        // Dateisystem ohne harte Verweise (FAT, manche Freigaben): direkt
        // und exklusiv anlegen. Das ersetzt ebenso nie etwas; nur ein Abbruch
        // mitten im Schreiben hinterliesse dort eine zu kurze Datei, die
        // `schluessel_lesen` dann als beschaedigt meldet. Wer genau waehrend
        // des Schreibens liest, wartet dort kurz und liest noch einmal.
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => exklusiv_schreiben(path, inhalt),
        x => x,
    });
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Legt `path` neu an (nie ueber eine vorhandene Datei) und schreibt
/// `inhalt` samt sync. Scheitert das Schreiben, verschwindet die
/// angefangene Datei wieder.
fn exklusiv_schreiben(path: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    let r = f.write_all(inhalt).and_then(|_| f.sync_all());
    if r.is_err() {
        drop(f);
        let _ = std::fs::remove_file(path);
    }
    r
}

// ------------------------------------------------- Ablage der Host-Rolle
//
// Die Host-Rolle nimmt mit dem Geraeteschluessel an (host.key unter
// %APPDATA%\QuadChroma bzw. ~/Library/Application Support/QuadChroma) -
// demselben, mit dem der Client anruft. Rust legt ihn mit 64 Byte an
// (privat, dann oeffentlich); gelesen werden auch 32 Byte, wie sie der
// Mac-Host schreibt (nur privat). Die erlaubten Geraete (host-devices.txt,
// frueher authorized.txt) und das Zugangspasswort fuehrt zugang.rs.

/// Dauerhafter Schluessel des Hosts: der Geraeteschluessel. Entsteht beim
/// ersten Start.
pub fn host_identity() -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    schluessel_laden(&config_dir().map_err(Fehler::Ablage)?.join(GERAETESCHLUESSEL))
}

/// Der oeffentliche Schluessel des Hosts auf diesem Rechner, falls es ihn
/// gibt: host.key im Ablageordner - von der Windows-Host-Rolle (64 Byte)
/// oder vom Mac-Host (32 Byte) geschrieben. Nur gelesen, nie angelegt; fehlt
/// die Datei oder taugt sie nicht, None. Fuer den Selbstschutz des Clients
/// (eigene ID nicht in der Hostliste, kein Verbinden zu sich selbst).
pub fn eigener_host_schluessel() -> Option<Vec<u8>> {
    oeffentlich_lesen(&config_dir().ok()?.join(GERAETESCHLUESSEL))
}

/// Der oeffentliche Teil einer Schluesseldatei, nur gelesen (None: fehlt,
/// unlesbar, beschaedigt).
fn oeffentlich_lesen(pfad: &Path) -> Option<Vec<u8>> {
    schluessel_lesen(pfad).ok().flatten().map(|(_, oeff)| oeff)
}

/// Eigener Schluessel fuer Tests, einmal je Lauf angelegt. Nebeneinander
/// laufende Tests legten sonst womoeglich zwei an, und einer hielte einen
/// Schluessel in der Hand, der nicht mehr in der Datei steht.
#[cfg(test)]
pub fn test_identitaet() -> (Vec<u8>, Vec<u8>) {
    static K: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
    K.get_or_init(|| identity().expect("Testschluessel")).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Eigener leerer Ordner je Test - die Tests laufen nebeneinander. Mit
    /// der Kennung des Laufs (test_lauf), damit ein Ordner eines frueheren
    /// Laufs mit derselben pid nicht hineinspielt.
    fn ordner(name: &str) -> PathBuf {
        test_ordner("secure", name)
    }

    /// Die Ablage der Tests ist je Prozess frisch: nicht der Ordner, den ein
    /// frueherer Lauf mit derselben pid hinterliess (samt einer hosts.txt
    /// mit einem anderen Schluessel fuer 127.0.0.1), aber innerhalb eines
    /// Laufs immer derselbe - auch fuer ordner(name).
    #[test]
    fn testablage_je_lauf_frisch() {
        let pid_ordner = std::env::temp_dir().join(format!("qc-test-{}", std::process::id()));
        let b = basis_ordner().unwrap();
        assert_ne!(b, pid_ordner);
        assert_eq!(b, basis_ordner().unwrap());
        let name = b.file_name().unwrap().to_str().unwrap().to_string();
        let praefix = format!("qc-test-{}-", std::process::id());
        let ms: u128 = name.strip_prefix(&praefix).and_then(|z| z.parse().ok()).expect(&name);
        let jetzt = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        assert!(ms <= jetzt && jetzt - ms < 24 * 3_600_000, "{name}");
        assert!(config_dir().unwrap().starts_with(&b));
        assert!(!config_dir().unwrap().starts_with(&pid_ordner));
        let o = ordner("lauf");
        let o2 = ordner("lauf");
        assert!(o.file_name().unwrap().to_str().unwrap().starts_with(&format!("{name}-secure-lauf-")), "{}", o.display());
        assert_ne!(o, o2, "zwei Aufrufe, ein Ordner");
        assert!(o.is_dir() && o2.is_dir());
        let _ = std::fs::remove_dir_all(&o);
        let _ = std::fs::remove_dir_all(&o2);
    }

    /// Nachricht 3 traegt den Namen, der jetzt gilt: nach einem Wechsel im
    /// Fenster "Geraetename" sofort den neuen, ohne Einstellung den
    /// Rechnernamen.
    #[test]
    fn nachricht3_mit_eingestelltem_namen() {
        let _s = crate::zugang::name_test_sperre();
        let vorher = crate::zugang::geraetename_eingestellt();
        crate::zugang::geraetename_setzen(Some("Wohnzimmer-PC".into()));
        let n = eigene_nachricht3(crate::protokoll_konst::NAME_FLAG_HOST_UNBEKANNT);
        assert_eq!(crate::zugang::nachricht3_name(&n).as_deref(), Some("Wohnzimmer-PC"));
        assert_eq!(crate::zugang::nachricht3_flags(&n), crate::protokoll_konst::NAME_FLAG_HOST_UNBEKANNT);
        crate::zugang::geraetename_setzen(Some("Büro".into()));
        assert_eq!(crate::zugang::nachricht3_name(&eigene_nachricht3(0)).as_deref(), Some("Büro"));
        crate::zugang::geraetename_setzen(None);
        assert_eq!(crate::zugang::nachricht3_name(&eigene_nachricht3(0)), Some(crate::zugang::rechnername()));
        crate::zugang::geraetename_setzen(vorher);
    }

    #[test]
    fn beschaedigter_schluessel_bleibt_liegen() {
        let d = ordner("key");
        let p = d.join("client.key");
        std::fs::write(&p, [7u8; 10]).unwrap();
        assert!(schluessel_laden(&p).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), vec![7u8; 10]);
        // Fehlt die Datei, entsteht sie - danach bleibt es derselbe Schluessel.
        let q = d.join("host.key");
        let k = schluessel_laden(&q).unwrap();
        assert_eq!(schluessel_laden(&q).unwrap(), k);
        assert_eq!(std::fs::read(&q).unwrap().len(), 64);
        // Kein Zwischenname bleibt liegen.
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn schluessel_nur_fuer_den_eigentuemer() {
        use std::os::unix::fs::PermissionsExt;
        test_identitaet();
        let d = config_dir().unwrap();
        let modus = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(modus(&d), 0o700);
        assert_eq!(modus(&d.join(ANRUF_SCHLUESSEL)), 0o600);
    }

    /// Eine Identitaet je Rechner (X1): im Betrieb ruft der Client mit dem
    /// Geraeteschluessel an, den auch die Host-Rolle nimmt - nicht mehr mit
    /// client.key. Nur Tests nehmen fuer den Anrufer eine eigene Datei (zwei
    /// Geraete in einer Ablage), und die ist weder host.key noch client.key.
    #[test]
    fn ein_geraeteschluessel_fuer_beide_rollen() {
        assert_eq!(GERAETESCHLUESSEL, "host.key");
        assert_eq!(FRUEHERER_CLIENT_SCHLUESSEL, "client.key");
        assert_ne!(ANRUF_SCHLUESSEL, GERAETESCHLUESSEL);
        assert_ne!(ANRUF_SCHLUESSEL, FRUEHERER_CLIENT_SCHLUESSEL);
        // Ein Windows-Checkout kann CRLF liefern (core.autocrlf): auf LF bringen.
        let quelle = include_str!("secure.rs").replace("\r\n", "\n");
        let betrieb = quelle.split("#[cfg(not(test))]\nconst ANRUF_SCHLUESSEL").nth(1).expect("Betriebswert");
        assert!(betrieb.trim_start().starts_with(": &str = GERAETESCHLUESSEL;"), "{}", &betrieb[..60]);
    }

    /// Der Hinweis nach dem Wechsel: nur, wenn ein frueherer client.key
    /// liegt und einen ANDEREN Schluessel traegt als host.key. Fehlt eine
    /// der Dateien oder ist sie beschaedigt: kein Hinweis. Nichts wird
    /// angelegt oder veraendert.
    #[test]
    fn frueherer_client_schluessel_nur_gelesen() {
        let d = ordner("alt-client");
        let alt = d.join(FRUEHERER_CLIENT_SCHLUESSEL);
        let geraet = d.join(GERAETESCHLUESSEL);
        assert!(!alter_schluessel_abweichend(&d));
        assert!(!alt.exists() && !geraet.exists(), "etwas angelegt");
        let (p1, o1) = noise::keypair().unwrap();
        let (p2, _) = noise::keypair().unwrap();
        std::fs::write(&alt, [p1.clone(), o1.clone()].concat()).unwrap();
        assert!(!alter_schluessel_abweichend(&d), "ohne host.key");
        assert!(!geraet.exists());
        // Derselbe Schluessel (etwa von Hand kopiert, 32 Byte wie beim Mac):
        // nichts hat sich geaendert.
        std::fs::write(&geraet, &p1).unwrap();
        assert!(!alter_schluessel_abweichend(&d));
        std::fs::remove_file(&geraet).unwrap();
        std::fs::write(&geraet, &p2).unwrap();
        assert!(alter_schluessel_abweichend(&d));
        assert_eq!(std::fs::read(&geraet).unwrap(), p2);
        assert_eq!(std::fs::read(&alt).unwrap(), [p1, o1].concat());
        std::fs::write(&alt, [5u8; 20]).unwrap();
        assert!(!alter_schluessel_abweichend(&d), "beschaedigter client.key");
        assert_eq!(std::fs::read(&alt).unwrap(), [5u8; 20]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn handschlag_ueber_den_rahmen() {
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (_, client_pub) = test_identitaet();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let faden = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut h = Secure::accept(s, &noise::prologue_video(), &host_priv).unwrap();
            let mut b = [0u8; 5];
            h.read_exact(&mut b).unwrap();
            h.write_all(b"zurueck").unwrap();
            (h.peer.clone(), h.handshake_hash.clone(), b)
        });
        let mut c = Secure::connect(&addr.to_string(), &noise::prologue_video()).unwrap();
        c.write_all(b"hallo").unwrap();
        let mut b = [0u8; 7];
        c.read_exact(&mut b).unwrap();
        let (peer, hh, gelesen) = faden.join().unwrap();
        assert_eq!(&b, b"zurueck");
        assert_eq!(&gelesen, b"hallo");
        assert_eq!(c.peer, host_pub);
        assert_eq!(peer, client_pub);
        assert_eq!(c.handshake_hash, hh);
    }

    /// `bereit` schaut nur hinein: ohne Daten false nach der Wartezeit, mit
    /// Daten true, und `lesen` bekommt sie danach ganz (auch was schon
    /// entschluesselt wartet). Macht die Gegenstelle zu: UnexpectedEof. Eine
    /// abgelaufene `lesefrist` meldet sich als Frist, nicht als Ende.
    #[test]
    fn bereit_und_lesefrist() {
        use std::io::ErrorKind as E;
        test_identitaet();
        // Eine Gegenstelle, die schickt, was der Kanal bringt, und zumacht,
        // wenn er endet.
        let gegenstelle = || {
            let (host_priv, _) = noise::keypair().unwrap();
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap().to_string();
            let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
            let faden = std::thread::spawn(move || {
                let (s, _) = l.accept().unwrap();
                let mut h = Secure::accept(s, &noise::prologue_video(), &host_priv).unwrap();
                while let Ok(d) = rx.recv() {
                    h.write_all(&d).unwrap();
                }
            });
            (Secure::connect(&addr, &noise::prologue_video()).unwrap(), tx, faden)
        };
        let (mut c, tx, faden) = gegenstelle();
        let t0 = Instant::now();
        assert!(!c.bereit(Duration::from_millis(80)).unwrap());
        assert!(t0.elapsed() >= Duration::from_millis(60));
        tx.send(b"abcdef".to_vec()).unwrap();
        assert!(c.bereit(Duration::from_secs(5)).unwrap());
        let mut b = [0u8; 4];
        c.lesen(&mut b).unwrap();
        assert_eq!(&b, b"abcd");
        assert!(c.bereit(Duration::from_millis(1)).unwrap(), "der Rest wartet entschluesselt");
        let mut b = [0u8; 2];
        c.lesen(&mut b).unwrap();
        assert_eq!(&b, b"ef");
        drop(tx);
        faden.join().unwrap();
        assert_eq!(c.bereit(Duration::from_secs(5)).unwrap_err().kind(), E::UnexpectedEof);
        assert_eq!(c.lesen(&mut [0u8; 1]).unwrap_err().kind(), E::UnexpectedEof);

        let (mut c, tx, faden) = gegenstelle();
        c.lesefrist(Some(Duration::from_millis(100)));
        let e = c.lesen(&mut [0u8; 1]).unwrap_err();
        assert!(matches!(e.kind(), E::WouldBlock | E::TimedOut), "{e:?}");
        drop(tx);
        faden.join().unwrap();
    }

    /// Scheitert die Pruefung im Handschlag, kommt ihr Fehler zurueck - und
    /// beim Angerufenen keine Nachricht 3: sein Handschlag scheitert, er
    /// kennt den Anrufer nicht.
    #[test]
    fn pruefung_scheitert_vor_nachricht_3() {
        let (host_priv, host_pub) = noise::keypair().unwrap();
        test_identitaet();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let faden = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            Secure::accept(s, &noise::prologue_video(), &host_priv).map(|h| h.peer)
        });
        let mut gesehen = Vec::new();
        let r = Secure::connect_pruefend(&addr.to_string(), &noise::prologue_video(), |k| {
            gesehen = k.to_vec();
            Err(Fehler::AnderesGeraet { addr: "127.0.0.1".into(), erwartet: 5, gemeldet: crate::zugang::geraete_id(k) })
        });
        assert!(matches!(r, Err(Fehler::AnderesGeraet { erwartet: 5, .. })));
        assert_eq!(gesehen, host_pub);
        let beim_host = faden.join().unwrap();
        assert!(beim_host.is_err(), "Nachricht 3 kam beim Host an");
    }

    /// Ein zweiter Prozess legt den Schluessel an, waehrend dieser erzeugt:
    /// dessen Schluessel gilt, nichts wird ersetzt, kein Zwischenname bleibt.
    #[test]
    fn schluessel_wettlauf_beim_erststart() {
        let d = ordner("key-wettlauf");
        let p = d.join("client.key");
        let fremd = paar64();
        let k = schluessel_laden_mit(&p, || {
            std::fs::write(&p, &fremd).unwrap();
            noise::keypair()
        })
        .unwrap();
        assert_eq!((k.0.as_slice(), k.1.as_slice()), (&fremd[..32], &fremd[32..]));
        assert_eq!(std::fs::read(&p).unwrap(), fremd);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        // Legt ihn der andere mit 32 Byte an (wie der Mac-Host), gilt ebenso
        // dessen Schluessel.
        std::fs::remove_file(&p).unwrap();
        let k = schluessel_laden_mit(&p, || {
            std::fs::write(&p, &fremd[..32]).unwrap();
            noise::keypair()
        })
        .unwrap();
        assert_eq!((k.0.as_slice(), k.1.as_slice()), (&fremd[..32], &fremd[32..]));
        assert_eq!(std::fs::read(&p).unwrap(), &fremd[..32]);
        // Direkt: vorhandenes Ziel heisst AlreadyExists, Inhalt bleibt.
        std::fs::write(&p, &fremd).unwrap();
        let e = geheim_schreiben(&p, &[1u8; 64]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), fremd);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    }

    /// Eine zu kurze Schluesseldatei wird einmal neu gelesen, bevor sie als
    /// beschaedigt gilt: ohne harte Verweise legt ein zweiter Prozess sie
    /// direkt an, und wer mitten darin liest, saehe sie halb.
    #[test]
    fn halber_schluessel_wird_nachgelesen() {
        let d = ordner("key-halb");
        let p = d.join("client.key");
        let ganz = paar64();
        std::fs::write(&p, &ganz[..20]).unwrap();
        let k = schluessel_lesen_mit(&p, || std::fs::write(&p, &ganz).unwrap()).unwrap();
        assert_eq!(k, Some((ganz[..32].to_vec(), ganz[32..].to_vec())));
        // Bleibt sie kurz, ist sie beschaedigt - und bleibt liegen.
        std::fs::write(&p, &ganz[..20]).unwrap();
        let mut gewartet = false;
        let e = schluessel_lesen_mit(&p, || gewartet = true).unwrap_err();
        assert!(gewartet);
        assert_eq!(e, Fehler::SchluesselBeschaedigt { pfad: p.clone(), laenge: 20 });
        // Zu lang: gleich beschaedigt, ohne zu warten.
        std::fs::write(&p, [1u8; 65]).unwrap();
        let e = schluessel_lesen_mit(&p, || panic!("gewartet")).unwrap_err();
        assert_eq!(e, Fehler::SchluesselBeschaedigt { pfad: p.clone(), laenge: 65 });
        // Fehlt sie, ist das kein Fehler.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(schluessel_lesen_mit(&p, || panic!("gewartet")), Ok(None));
    }

    /// Ein gueltiges Paar in der Form, wie Rust es schreibt: privat, dann
    /// oeffentlich (64 Byte).
    fn paar64() -> Vec<u8> {
        let (p, o) = noise::keypair().unwrap();
        [p, o].concat()
    }

    /// 64 Byte, deren hinterer Teil nicht der oeffentliche Schluessel zum
    /// vorderen ist: beschaedigt wie im Lader des Mac-Hosts (qc_secure.c) -
    /// die Datei bleibt Byte fuer Byte, nichts wird neu erzeugt, und fuer den
    /// Selbstschutz gibt es keinen oeffentlichen Schluessel.
    #[test]
    fn schluessel_mit_64_byte_passt_zusammen() {
        let d = ordner("key-64");
        let p = d.join("host.key");
        let mut b = paar64();
        std::fs::write(&p, &b).unwrap();
        assert_eq!(schluessel_laden_mit(&p, || panic!("neu erzeugt")).unwrap(), (b[..32].to_vec(), b[32..].to_vec()));
        b[40] ^= 1;
        std::fs::write(&p, &b).unwrap();
        let e = schluessel_laden_mit(&p, || panic!("neu erzeugt")).unwrap_err();
        assert_eq!(e, Fehler::SchluesselBeschaedigt { pfad: p.clone(), laenge: 64 });
        assert!(e.to_string().contains("passt nicht zum privaten"), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), b, "Datei veraendert");
        assert_eq!(schluessel_lesen_mit(&p, || panic!("gewartet")), Err(e));
        assert_eq!(oeffentlich_lesen(&p), None);
        // Ein anderer oeffentlicher Schluessel dahinter (etwa zwei Dateien
        // zusammengesetzt): ebenso beschaedigt.
        let fremd = paar64();
        std::fs::write(&p, [&b[..32], &fremd[32..]].concat()).unwrap();
        assert!(matches!(schluessel_laden_mit(&p, || panic!("neu erzeugt")), Err(Fehler::SchluesselBeschaedigt { laenge: 64, .. })));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Mehrere Erzeuger zugleich in EINEM Prozess (die eine App: Client und
    /// Host-Rolle brauchen den Geraeteschluessel womoeglich im selben
    /// Augenblick zum ersten Mal): alle halten danach denselben Schluessel,
    /// er steht in der Datei, und kein Zwischenname bleibt liegen.
    #[test]
    fn schluessel_wettlauf_in_einem_prozess() {
        let d = ordner("key-faeden");
        let p = d.join("host.key");
        for runde in 0..40 {
            let _ = std::fs::remove_file(&p);
            let los = std::sync::Barrier::new(4);
            let k: Vec<_> = std::thread::scope(|s| {
                let h: Vec<_> = (0..4)
                    .map(|_| {
                        s.spawn(|| {
                            los.wait();
                            schluessel_laden(&p)
                        })
                    })
                    .collect();
                h.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let datei = std::fs::read(&p).unwrap();
            for r in &k {
                let (privat, oeff) = r.as_ref().unwrap_or_else(|e| panic!("Runde {runde}: {e}"));
                assert_eq!([privat.clone(), oeff.clone()].concat(), datei, "Runde {runde}");
            }
            assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "Runde {runde}: Zwischenname liegt");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Die eine App auf dem Mac: der Rust-Teil (64 Byte) und der Lader der
    /// Host-Engine (qc_secure.c, 32 Byte) legen dieselbe fehlende Datei
    /// zugleich an. Wer verliert, liest die Datei des anderen (EEXIST bzw.
    /// AlreadyExists) - beide halten denselben Schluessel, und nichts wird
    /// ueberschrieben.
    #[cfg(target_os = "macos")]
    #[test]
    fn schluessel_wettlauf_rust_und_mac_host() {
        extern "C" {
            fn qc_identity_load_pfad(path: *const std::os::raw::c_char, privat: *mut u8, oeff: *mut u8) -> std::os::raw::c_int;
        }
        let d = ordner("key-rust-c");
        let p = d.join("host.key");
        let c_pfad = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
        let (mut rust_zuerst, mut c_zuerst) = (0, 0);
        for runde in 0..60u64 {
            let _ = std::fs::remove_file(&p);
            let los = std::sync::Barrier::new(2);
            let (rust, c) = std::thread::scope(|s| {
                let rust = s.spawn(|| {
                    los.wait();
                    schluessel_laden(&p)
                });
                let c = s.spawn(|| {
                    let (mut privat, mut oeff) = ([0u8; 32], [0u8; 32]);
                    los.wait();
                    // Jede zweite Runde spaeter: sonst legt C fast immer
                    // zuerst an (der Rust-Teil wartet auf sync_all). So
                    // liest mal der eine, mal der andere die Datei des anderen.
                    std::thread::sleep(Duration::from_micros((runde % 2) * (runde % 7 + 1) * 8000));
                    // SAFETY: Pfad mit Nullbyte; der Lader schreibt je genau 32 Byte.
                    let r = unsafe { qc_identity_load_pfad(c_pfad.as_ptr(), privat.as_mut_ptr(), oeff.as_mut_ptr()) };
                    (r, privat, oeff)
                });
                (rust.join().unwrap(), c.join().unwrap())
            });
            let (privat, oeff) = rust.unwrap_or_else(|e| panic!("Runde {runde}: Rust {e}"));
            assert_eq!(c.0, 0, "Runde {runde}: C-Lader");
            assert_eq!((privat.as_slice(), oeff.as_slice()), (&c.1[..], &c.2[..]), "Runde {runde}");
            let datei = std::fs::read(&p).unwrap();
            match datei.len() {
                64 => rust_zuerst += 1,
                32 => c_zuerst += 1,
                n => panic!("Runde {runde}: {n} Byte"),
            }
            assert_eq!(&datei[..32], &privat[..], "Runde {runde}");
            assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "Runde {runde}: Zwischenname liegt");
        }
        println!("Rust zuerst: {rust_zuerst}, C zuerst: {c_zuerst}");
        assert!(rust_zuerst > 0 && c_zuerst > 0, "nur eine Seite legte an: Rust {rust_zuerst}, C {c_zuerst}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Eine Schluesseldatei mit 32 Byte (nur privat), wie sie der Mac-Host als
    /// host.key schreibt: der oeffentliche Schluessel wird errechnet - mit dem
    /// Pruefvektor aus RFC 7748 (6.1) derselbe wie im Mac-Host (qc_pubkey,
    /// zugangtest.c), und damit dieselbe Geraete-ID. Die Datei bleibt Byte
    /// fuer Byte, wie sie war, und laden legt nichts daneben an. 64 Byte
    /// gelten weiter wie bisher.
    #[test]
    fn schluessel_mit_32_byte_wie_der_mac_host() {
        let privat = hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let oeff = hex32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(noise::oeffentlich(&privat), Some(oeff.clone()));
        assert_eq!(noise::oeffentlich(&privat[..31]), None);
        assert_eq!(crate::zugang::geraete_id(&oeff), 828_873_450);
        assert_eq!(crate::zugang::id_text(crate::zugang::geraete_id(&oeff)), "828 873 450");
        let d = ordner("key-32");
        let p = d.join("host.key");
        std::fs::write(&p, &privat).unwrap();
        let vorher = std::fs::metadata(&p).unwrap().modified().unwrap();
        let k = schluessel_laden_mit(&p, || panic!("neu erzeugt")).unwrap();
        assert_eq!(k, (privat.clone(), oeff.clone()));
        assert_eq!(std::fs::read(&p).unwrap(), privat, "Datei veraendert");
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), vorher);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        // Gelesen ohne zu warten - 32 Byte sind kein halber Schluessel.
        assert_eq!(schluessel_lesen_mit(&p, || panic!("gewartet")), Ok(Some((privat.clone(), oeff.clone()))));
        // 64 Byte wie bisher: privat, dann oeffentlich, ebenso unveraendert.
        let mut beide = privat.clone();
        beide.extend_from_slice(&oeff);
        std::fs::write(&p, &beide).unwrap();
        assert_eq!(schluessel_laden_mit(&p, || panic!("neu erzeugt")).unwrap(), (privat, oeff));
        assert_eq!(std::fs::read(&p).unwrap(), beide);
        // 33 Byte: beschaedigt, nicht ueberschrieben.
        std::fs::write(&p, [3u8; 33]).unwrap();
        assert_eq!(
            schluessel_laden_mit(&p, || panic!("neu erzeugt")),
            Err(Fehler::SchluesselBeschaedigt { pfad: p.clone(), laenge: 33 })
        );
        assert_eq!(std::fs::read(&p).unwrap(), [3u8; 33]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Der eigene Host-Schluessel wird nur gelesen (in einem eigenen Ordner
    /// geprueft - host.key der Testablage gehoert verbinden_zu_sich_selbst):
    /// fehlt die Datei, None, und nichts wird angelegt; liegt ein Schluessel
    /// da (32 oder 64 Byte), kommt sein oeffentlicher Teil; ist sie
    /// beschaedigt, None, und sie bleibt.
    #[test]
    fn eigener_host_schluessel_nur_gelesen() {
        let d = ordner("eigen");
        let p = d.join("host.key");
        assert_eq!(oeffentlich_lesen(&p), None);
        assert!(!p.exists(), "host.key angelegt");
        let privat = hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let oeff = hex32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        std::fs::write(&p, &privat).unwrap();
        assert_eq!(oeffentlich_lesen(&p), Some(oeff.clone()));
        assert_eq!(std::fs::read(&p).unwrap(), privat);
        let mut beide = privat.clone();
        beide.extend_from_slice(&oeff);
        std::fs::write(&p, &beide).unwrap();
        assert_eq!(oeffentlich_lesen(&p), Some(oeff));
        std::fs::write(&p, [1u8; 40]).unwrap();
        assert_eq!(oeffentlich_lesen(&p), None);
        assert_eq!(std::fs::read(&p).unwrap(), [1u8; 40]);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn hex32(t: &str) -> Vec<u8> {
        (0..32).map(|i| u8::from_str_radix(&t[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// Der Wortlaut des Systems steht in einer Zeile.
    #[test]
    fn wortlaut_in_einer_zeile() {
        let e = std::io::Error::other("Eine bestehende Verbindung wurde softwaregesteuert\r\ndurch den Hostcomputer abgebrochen.");
        assert_eq!(wortlaut(&e), "Eine bestehende Verbindung wurde softwaregesteuert durch den Hostcomputer abgebrochen.");
    }

    /// Weder ein stummes noch ein troepfelndes Gegenueber haelt die Annahme
    /// laenger als FRIST_ANNAHME fest - auch wenn jedes Byte die alte Frist
    /// je Leseaufruf (3 s) eingehalten haette.
    #[test]
    fn annahme_frist_gilt_fuer_den_ganzen_handschlag() {
        let (host_priv, _) = noise::keypair().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let _stumm = TcpStream::connect(addr).unwrap();
        let (s1, _) = l.accept().unwrap();
        let mut tropf = TcpStream::connect(addr).unwrap();
        let (s2, _) = l.accept().unwrap();
        std::thread::spawn(move || {
            // 65535 Byte angesagt, dann alle 0,9 s eines.
            let _ = tropf.write_all(&[0xff, 0xff]);
            for _ in 0..12 {
                std::thread::sleep(Duration::from_millis(900));
                if tropf.write_all(&[0]).is_err() {
                    break;
                }
            }
        });
        let annehmen = |s: TcpStream, k: Vec<u8>| {
            std::thread::spawn(move || {
                let t0 = Instant::now();
                let r = Secure::accept(s, &noise::prologue_video(), &k);
                (r.is_err(), t0.elapsed())
            })
        };
        let a = annehmen(s1, host_priv.clone());
        let b = annehmen(s2, host_priv);
        for (fehler, dauer) in [a.join().unwrap(), b.join().unwrap()] {
            assert!(fehler);
            assert!(dauer >= FRIST_ANNAHME - Duration::from_millis(100), "{dauer:?}");
            assert!(dauer <= FRIST_ANNAHME + Duration::from_millis(500), "{dauer:?}");
        }
    }
}
