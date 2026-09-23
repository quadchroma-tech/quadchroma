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
    /// Schluesseldatei vorhanden, aber mit falscher Laenge.
    SchluesselBeschaedigt { pfad: PathBuf, laenge: usize },
    /// Datei vorhanden, aber nicht lesbar (Rechte, Sperre durch ein
    /// anderes Programm). `grund` ist der Wortlaut des Systems.
    Unlesbar { pfad: PathBuf, grund: String },
    /// Vertrauensliste vorhanden, aber kein UTF-8 (etwa als ANSI gespeichert).
    KeinUtf8 { pfad: PathBuf },
    /// Schreiben gescheitert (neuer Schluessel, neuer Eintrag).
    Schreiben { pfad: PathBuf, grund: String },
    /// Der Fingerabdruck einer bekannten Adresse hat sich geaendert;
    /// `fingerabdruck` ist der neue, `pfad` die Liste mit dem alten.
    FingerabdruckGeaendert { host: String, fingerabdruck: String, pfad: PathBuf },
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
            Fehler::SchluesselBeschaedigt { pfad, laenge } => write!(
                f,
                "{} ist beschaedigt ({laenge} statt 64 Byte) und wird nicht ueberschrieben. \
                 Datei pruefen oder loeschen - dann entsteht ein neuer Schluessel.",
                pfad.display()
            ),
            Fehler::Unlesbar { pfad, grund } => write!(f, "{} nicht lesbar: {grund}", pfad.display()),
            Fehler::KeinUtf8 { pfad } => write!(f, "{} ist kein UTF-8", pfad.display()),
            Fehler::Schreiben { pfad, grund } => write!(f, "{} nicht zu schreiben: {grund}", pfad.display()),
            Fehler::FingerabdruckGeaendert { host, fingerabdruck, pfad } => write!(
                f,
                "Der Fingerabdruck von {host} hat sich geaendert (jetzt {fingerabdruck}). Verbindung abgelehnt. \
                 Wenn der Host neu aufgesetzt wurde, den Eintrag in {} loeschen.",
                pfad.display()
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

pub struct Secure {
    sock: TcpStream,
    tx: snow::TransportState,
    inbuf: Vec<u8>,
    inpos: usize,
    /// Pruefsumme des Handschlags - bindet den Eingabekanal an den Bildkanal.
    pub handshake_hash: Vec<u8>,
    pub peer: Vec<u8>,
    pub sas: String,
}

impl Secure {
    /// Verbindet und fuehrt den Handschlag als Anrufer, OHNE den Schluessel
    /// der Gegenstelle zu pruefen - nur fuer Pruefstaende (Tests hier und in
    /// host/netz.rs). Der Client selbst verbindet ueber `connect_pruefend`.
    #[cfg(test)]
    pub fn connect(addr: &str, prologue: &[u8]) -> Result<Secure, Fehler> {
        Secure::connect_pruefend(addr, prologue, |_| Ok(()))
    }

    /// Verbindet und fuehrt den Handschlag als Anrufer. `pruefen` sieht den
    /// Schluessel der Gegenstelle nach Nachricht 2 und VOR Nachricht 3 (siehe
    /// noise::handshake_initiator): scheitert sie, geht der eigene Schluessel
    /// nie hinaus, die Leitung faellt zu, und ihr Fehler kommt unveraendert
    /// zurueck. Der Host sieht dann nur einen abgebrochenen Handschlag -
    /// angenommen, und damit ein laufender Zuschauer abgeloest, wird dort
    /// erst nach Nachricht 3.
    pub fn connect_pruefend(
        addr: &str,
        prologue: &[u8],
        pruefen: impl FnOnce(&[u8]) -> Result<(), Fehler>,
    ) -> Result<Secure, Fehler> {
        // Der eigene Schluessel zuerst: ist client.key beschaedigt oder die
        // Ablage weg, geht gar nicht erst eine Leitung auf - sonst oeffnete
        // jeder Versuch eine leere Verbindung, und der Host protokollierte sie.
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
        pruefen: impl FnOnce(&[u8]) -> Result<(), Fehler>,
    ) -> Result<Secure, Fehler> {
        // Waehrend des Handschlags gilt eine Frist. Danach wird sie wieder
        // aufgehoben: der Bildkanal darf beliebig lange still sein, ohne dass
        // daraus ein Fehler wird.
        let r = Rahmen::neu(&sock, FRIST_ANRUFER);
        // Der Handschlag kennt nur Text; der Fehler der Pruefung (Pin,
        // Ablage) wartet hier, damit er mit seiner Art zurueckkommt.
        let mut pruef_fehler: Option<Fehler> = None;
        let s = noise::handshake_initiator(priv_key, prologue, |b| r.recv(b), |d| r.send(d), |rs| {
            pruefen(rs).map_err(|f| {
                let text = f.to_string();
                pruef_fehler = Some(f);
                text
            })
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
        })
    }

    /// Nimmt eine angenommene Leitung als Angerufener (Host-Rolle) an: der
    /// Handschlag laeuft mit dem uebergebenen dauerhaften Schluessel des
    /// Hosts, nicht mit client.key. Rahmen wie bei `wrap`; die Frist
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
        })
    }

    /// Die Leitung selbst - fuer Socketoptionen der Host-Rolle (Sendepuffer).
    pub fn socket(&self) -> &TcpStream {
        &self.sock
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
        let mut done = 0;
        while done < dst.len() {
            if self.inpos == self.inbuf.len() {
                let mut l = [0u8; 2];
                (&self.sock).read_exact(&mut l).map_err(|e| format!("Laenge: {}", wortlaut(&e)))?;
                let n = u16::from_le_bytes(l) as usize;
                if n > CHUNK_MAX + 16 {
                    return Err("unplausible Datensatzlaenge".into());
                }
                let mut ct = vec![0u8; n];
                (&self.sock).read_exact(&mut ct).map_err(|e| format!("Daten: {}", wortlaut(&e)))?;
                let mut pt = vec![0u8; CHUNK_MAX];
                let got = self
                    .tx
                    .read_message(&ct, &mut pt)
                    .map_err(|_| "Datensatz nicht echt - abgebrochen".to_string())?;
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

// ------------------------------------------------------------ Schluesselablage

/// Der Ordner fuer alles, was der Client dauerhaft ablegt: client.key,
/// known_hosts.txt, einstellungen.txt, protokoll.txt, benchmark.txt.
/// Windows: %APPDATA%\QuadChroma (ohne APPDATA: unter HOME).
/// macOS: ~/Library/Application Support/QuadChroma - derselbe Ordner, in dem
/// der Mac-Host host.key und authorized.txt haelt; getrennte Dateinamen, keine
/// Kollision.
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
    Ok(std::env::temp_dir().join(format!("qc-test-{}", std::process::id())))
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

/// Dauerhafter eigener Schluessel. Entsteht beim ersten Start.
pub fn identity() -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    // Im Ablauf stehen privater und oeffentlicher Teil hintereinander, damit
    // beim Start nichts nachgerechnet werden muss.
    schluessel_laden(&config_dir().map_err(Fehler::Ablage)?.join("client.key"))
}

/// Liest einen Schluessel (64 Byte: privat, dann oeffentlich) oder legt ihn
/// an - aber NUR, wenn die Datei fehlt. Ist sie unlesbar oder hat sie die
/// falsche Laenge, bleibt sie liegen: ein stilles Ueberschreiben machte aus
/// einem Dateifehler eine neue Identitaet, und jede Kopplung waere weg.
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

/// Wie `schluessel_lesen`. Ist die Datei zu KURZ, wird nach `warten` einmal
/// neu gelesen, bevor sie als beschaedigt gilt: auf einem Dateisystem ohne
/// harte Verweise legt ein zweiter Prozess sie direkt an (siehe
/// `geheim_schreiben`), und wer genau dann liest, saehe sie halb. Zu lang
/// wird sie dabei nie - das ist gleich ein Fehler.
fn schluessel_lesen_mit(path: &Path, warten: impl FnOnce()) -> Result<Option<(Vec<u8>, Vec<u8>)>, Fehler> {
    let lesen = || match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Fehler::Unlesbar { pfad: path.to_path_buf(), grund: wortlaut(&e) }),
    };
    let mut b = lesen()?;
    if b.as_ref().is_some_and(|b| b.len() < 64) {
        warten();
        b = lesen()?;
    }
    match b {
        None => Ok(None),
        Some(b) if b.len() == 64 => Ok(Some((b[..32].to_vec(), b[32..].to_vec()))),
        Some(b) => Err(Fehler::SchluesselBeschaedigt { pfad: path.to_path_buf(), laenge: b.len() }),
    }
}

/// Legt eine Schluesseldatei an: erst vollstaendig unter einem Zwischennamen,
/// dann per hartem Verweis unter dem Zielnamen - ein Abbruch mittendrin
/// hinterlaesst nie einen halben Schluessel, und ein vorhandener wird nie
/// ersetzt: der harte Verweis scheitert dann mit AlreadyExists (wie O_EXCL
/// beim Mac-Host). Ein Umbenennen ersetzte eine Datei, die ein zweiter
/// Prozess inzwischen angelegt hat. Unter Unix nur fuer den Eigentuemer
/// lesbar (0600, wie host.key des Mac-Hosts); unter Windows gilt die geerbte
/// Liste (siehe config_dir).
fn geheim_schreiben(path: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.neu", std::process::id()));
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

/// Liest eine Vertrauensliste (known_hosts.txt, authorized.txt). Nur eine
/// FEHLENDE Datei gilt als leer. Jeder andere Fehler - keine Leserechte,
/// Sperre durch ein anderes Programm, kein UTF-8 (etwa als ANSI oder UTF-16
/// gespeichert) - ist ein Fehler: sonst saehe eine vorhandene, aber
/// unlesbare Liste aus wie der allererste Start, und der Erstkontakt nahme
/// die naechste fremde Gegenstelle auf. Ein fuehrendes BOM (Editor "UTF-8
/// mit BOM") wird uebergangen - sonst traefe die erste Zeile nie.
fn liste_lesen(path: &Path) -> Result<String, Fehler> {
    liste_lesen_falls_da(path).map(Option::unwrap_or_default)
}

/// Wie `liste_lesen`, aber eine fehlende Datei ergibt None statt eines
/// leeren Texts - fuer die Freigabeliste, bei der "leer" und "fehlt" nicht
/// dasselbe heissen.
fn liste_lesen_falls_da(path: &Path) -> Result<Option<String>, Fehler> {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Fehler::Unlesbar { pfad: path.to_path_buf(), grund: wortlaut(&e) }),
    };
    let text = String::from_utf8(b).map_err(|_| Fehler::KeinUtf8 { pfad: path.to_path_buf() })?;
    Ok(Some(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text)))
}

/// Haengt eine Zeile an eine Vertrauensliste an. Die vorhandenen Zeilen
/// werden nie neu geschrieben - ein Fehler beim Schreiben kann also keinen
/// bestehenden Eintrag kosten. `bisher` ist der gerade gelesene Inhalt; fehlt
/// ihm das letzte Zeilenende, kommt es vor die neue Zeile.
fn liste_anhaengen(path: &Path, bisher: &str, zeile: &str) -> std::io::Result<()> {
    liste_anhaengen_mit(path, bisher, zeile, |f, b| f.write_all(b).and_then(|_| f.sync_all()))
}

/// Wie `liste_anhaengen`; `schreiben` schreibt die neuen Bytes (Tests
/// schieben dort einen Schreibfehler ein). Scheitert es, wird die Datei auf
/// ihre alte Laenge zurueckgekuerzt, wie mit ftruncate beim Mac-Host: eine
/// halbe Zeile bliebe sonst stehen und zaehlte womoeglich als Eintrag.
fn liste_anhaengen_mit(
    path: &Path,
    bisher: &str,
    zeile: &str,
    schreiben: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    let alt = f.metadata()?.len();
    let mut neu = String::new();
    if !bisher.is_empty() && !bisher.ends_with('\n') {
        neu.push('\n');
    }
    neu.push_str(zeile);
    neu.push('\n');
    let r = schreiben(&mut f, neu.as_bytes());
    if r.is_err() {
        // Kuerzen ueber einen eigenen Griff zum Schreiben: einer zum
        // Anhaengen darf unter Windows die Laenge nicht setzen.
        drop(f);
        if let Ok(g) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = g.set_len(alt);
            let _ = g.sync_all();
        }
    }
    r
}

/// Host-Teil einer Adresse, so wie er in known_hosts.txt steht.
fn host_von(addr: &str) -> String {
    addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).to_string()
}

/// Der gemerkte Schluessel (Hex) eines Hosts. Es zaehlt die erste Zeile mit
/// diesem Host.
fn gemerkt(text: &str, host: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some(h), Some(k)) if h == host => Some(k.to_string()),
            _ => None,
        }
    })
}

/// Merkt sich den Schluessel eines Hosts. Gibt Err zurueck, wenn sich der
/// Fingerabdruck einer bekannten Adresse geaendert hat - dann stimmt etwas
/// nicht. Ebenso, wenn known_hosts.txt vorhanden, aber nicht lesbar ist.
/// Ok(true): erstmals gesehen und eingetragen.
fn known_host_pruefen(path: &Path, addr: &str, peer: &[u8]) -> Result<bool, Fehler> {
    let host = host_von(addr);
    let text = liste_lesen(path)?;
    match gemerkt(&text, &host) {
        Some(k) if k == hex(peer) => Ok(false), // bekannt und unveraendert
        Some(_) => Err(Fehler::FingerabdruckGeaendert {
            host,
            fingerabdruck: noise::fingerprint(peer),
            pfad: path.to_path_buf(),
        }),
        None => {
            liste_anhaengen(path, &text, &format!("{host} {} {}", hex(peer), noise::fingerprint(peer)))
                .map_err(|e| Fehler::Schreiben { pfad: path.to_path_buf(), grund: wortlaut(&e) })?;
            Ok(true) // erstmals gesehen
        }
    }
}

/// Was known_hosts.txt ueber einen Host weiss - gelesen, BEVOR eine Leitung
/// aufgeht. Ist die Liste unlesbar (Rechte, Sperre, kein UTF-8), wird gar
/// nicht erst verbunden. Ist der Host dort bekannt, prueft `pruefen` seinen
/// Schluessel noch im Handschlag, vor Nachricht 3 (Secure::connect_pruefend):
/// erst mit Nachricht 3 nimmt ein Host den Anrufer als Zuschauer an und
/// loest dafuer den laufenden ab - ein Client mit falschem Pin kommt so weit
/// nicht mehr. Beim ersten Kontakt traegt `eintragen` den Schluessel nach
/// dem Handschlag ein, wie bisher.
pub struct HostPin {
    pfad: PathBuf,
    addr: String,
    /// Der gemerkte Schluessel (Hex); None: erster Kontakt mit diesem Host.
    gemerkt: Option<String>,
}

impl HostPin {
    pub fn laden(addr: &str) -> Result<HostPin, Fehler> {
        HostPin::laden_aus(config_dir().map_err(Fehler::Ablage)?.join("known_hosts.txt"), addr)
    }

    fn laden_aus(pfad: PathBuf, addr: &str) -> Result<HostPin, Fehler> {
        let text = liste_lesen(&pfad)?;
        let gemerkt = gemerkt(&text, &host_von(addr));
        if gemerkt.is_none() {
            // Erster Kontakt: eingetragen wird erst nach dem Handschlag. Laesst
            // sich die Liste gar nicht beschreiben (schreibgeschuetzt, keine
            // Rechte), soll das jetzt auffallen - nicht erst, wenn der Host
            // dieses Geraet schon angenommen hat. Eine fehlende Liste entsteht
            // dabei leer; leer und fehlend heissen hier dasselbe.
            let mut o = std::fs::OpenOptions::new();
            o.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            o.open(&pfad).map_err(|e| Fehler::Schreiben { pfad: pfad.clone(), grund: wortlaut(&e) })?;
        }
        Ok(HostPin { pfad, addr: addr.to_string(), gemerkt })
    }

    /// Im Handschlag, nach Nachricht 2: passt der Schluessel des Hosts zum
    /// Pin? Ohne Pin (erster Kontakt) passt jeder - dann zeigt die Kopplung
    /// den Vergleichscode.
    pub fn pruefen(&self, peer: &[u8]) -> Result<(), Fehler> {
        match &self.gemerkt {
            Some(k) if *k != hex(peer) => Err(Fehler::FingerabdruckGeaendert {
                host: host_von(&self.addr),
                fingerabdruck: noise::fingerprint(peer),
                pfad: self.pfad.clone(),
            }),
            _ => Ok(()),
        }
    }

    /// Nach dem Handschlag: beim ersten Kontakt den Schluessel eintragen.
    /// Ok(true): erstmals gesehen. Die Liste wird dafuer neu gelesen - hat ein
    /// zweites Fenster den Host inzwischen eingetragen, gilt dessen Zeile.
    pub fn eintragen(&self, peer: &[u8]) -> Result<bool, Fehler> {
        self.pruefen(peer)?;
        if self.gemerkt.is_some() {
            return Ok(false);
        }
        known_host_pruefen(&self.pfad, &self.addr, peer)
    }
}

// ------------------------------------------------- Ablage der Host-Rolle
//
// Der Windows-Host hat seinen eigenen dauerhaften Schluessel (host.key) und
// seine Liste freigegebener Gegenstellen (authorized.txt), beide unter
// %APPDATA%\QuadChroma neben client.key und known_hosts.txt - getrennte
// Dateien, keine Kollision. authorized.txt hat das Format des Macs:
// 64 Hex + zwei Leerzeichen + Fingerabdruck + zwei Leerzeichen + Name.
// host.key liegt hier wie client.key mit 64 Byte (privat, dann oeffentlich),
// damit der eigene Fingerabdruck ohne Nachrechnen im Protokoll stehen kann.

/// Dauerhafter Schluessel des Hosts. Entsteht beim ersten Start.
pub fn host_identity() -> Result<(Vec<u8>, Vec<u8>), Fehler> {
    schluessel_laden(&config_dir().map_err(Fehler::Ablage)?.join("host.key"))
}

fn hex(peer: &[u8]) -> String {
    peer.iter().map(|b| format!("{b:02x}")).collect()
}

fn authorized_path() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("authorized.txt"))
}

/// Die Freigabeliste, einmal gelesen. Wer ueber eine Gegenstelle
/// entscheidet, liest sie genau einmal und entscheidet auf diesem Stand -
/// Pruefen, Erstkontakt und Eintragen sehen dieselbe Datei. Nur die
/// Host-Rolle (Windows) braucht sie.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Freigaben {
    pfad: PathBuf,
    text: String,
    /// Gab es die Datei? Nur wenn nicht, ist es ein Erstkontakt.
    vorhanden: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Freigaben {
    /// Liest authorized.txt. Err, wenn sie vorhanden, aber nicht lesbar ist
    /// (Rechte, Sperre, kein UTF-8) - dann entscheidet der Aufrufer auf
    /// Abweisen, nicht auf Erstkontakt. Ohne Ablageort ebenfalls Err.
    pub fn lesen() -> Result<Freigaben, String> {
        let p = authorized_path().ok_or("kein Ablageort fuer authorized.txt")?;
        Freigaben::lesen_aus(p)
    }

    fn lesen_aus(pfad: PathBuf) -> Result<Freigaben, String> {
        let gelesen = liste_lesen_falls_da(&pfad)?;
        let vorhanden = gelesen.is_some();
        Ok(Freigaben { pfad, text: gelesen.unwrap_or_default(), vorhanden })
    }

    /// Ist diese Gegenstelle freigegeben? Verglichen werden die ersten 64
    /// Zeichen der Zeile ohne Ruecksicht auf Gross- und Kleinschreibung, wie
    /// mit strncasecmp auf dem Mac - `anzahl` zaehlt solche Zeilen ohnehin
    /// als gueltig.
    pub fn enthaelt(&self, peer: &[u8]) -> bool {
        let h = hex(peer);
        self.text.lines().any(|l| l.len() >= h.len() && l.as_bytes()[..h.len()].eq_ignore_ascii_case(h.as_bytes()))
    }

    /// Gueltige Eintraege: Zeilen, die mit 64 Hexziffern beginnen.
    pub fn anzahl(&self) -> usize {
        self.text
            .lines()
            .filter(|l| l.len() >= 64 && l.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit))
            .count()
    }

    /// Darf die naechste unbekannte Gegenstelle ohne --pair herein? Nur,
    /// wenn es authorized.txt noch gar nicht gibt. Ist sie vorhanden, aber
    /// ohne einen einzigen gueltigen Eintrag (leer, nur Kommentare, von Hand
    /// verdorben, beim Schreiben abgeschnitten), ist das KEIN Erststart: Err
    /// mit dem Grund. Neu koppeln geht dann mit --pair oder nach Loeschen.
    pub fn erstkontakt(&self) -> Result<bool, String> {
        if !self.vorhanden {
            return Ok(true);
        }
        if self.anzahl() == 0 {
            return Err("authorized.txt ist vorhanden, enthaelt aber keine gueltige Freigabe - kein Erstkontakt, \
                 neue Gegenstellen nur mit --pair (oder Datei loeschen)"
                .into());
        }
        Ok(false)
    }

    /// Gegenstelle aufnehmen. Schon bekannte Zeilen werden nicht verdoppelt;
    /// die vorhandenen Zeilen bleiben unberuehrt (angehaengt, nicht neu
    /// geschrieben). Err heisst: nicht gespeichert - dann nicht zulassen.
    pub fn aufnehmen(&self, peer: &[u8], name: &str) -> Result<(), String> {
        self.aufnehmen_mit(peer, name, |f, b| f.write_all(b).and_then(|_| f.sync_all()))
    }

    /// Wie `aufnehmen`; `schreiben` wie bei `liste_anhaengen_mit`.
    fn aufnehmen_mit(
        &self,
        peer: &[u8],
        name: &str,
        schreiben: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    ) -> Result<(), String> {
        if self.enthaelt(peer) {
            return Ok(());
        }
        let zeile = format!("{}  {}  {}", hex(peer), noise::fingerprint(peer), if name.is_empty() { "-" } else { name });
        liste_anhaengen_mit(&self.pfad, &self.text, &zeile, schreiben).map_err(|e| {
            // War die Liste vorher nicht da, darf ein gescheitertes Speichern
            // keine leere Datei hinterlassen - sonst waere der Erstkontakt
            // verloren (der Mac-Host wertet eine leere Liste ohnehin als
            // Erstkontakt). Geloescht wird erst hier, mit geschlossenem Griff:
            // eine offene Datei loescht Windows nicht. Und nur, wenn sie nach
            // dem Zurueckkuerzen leer ist: hat ein zweiter Host-Prozess mit
            // derselben Ablage sie inzwischen angelegt und beschrieben, bleibt
            // seine Zeile stehen.
            let leer = std::fs::metadata(&self.pfad).map(|m| m.len() == 0).unwrap_or(false);
            if !self.vorhanden && leer {
                let _ = std::fs::remove_file(&self.pfad);
            }
            format!("authorized.txt: {}", wortlaut(&e))
        })
    }
}

/// Anzahl der freigegebenen Gegenstellen, nur fuer die Startzeile im
/// Protokoll (unlesbar: 0). Entscheidungen laufen ueber `Freigaben`.
pub fn authorized_count() -> usize {
    Freigaben::lesen().map(|f| f.anzahl()).unwrap_or(0)
}

/// Alle Freigaben loeschen (--forget). Scheitert das Loeschen, gelten die
/// bisherigen Freigaben weiter - ein verlorenes Geraet kaeme also weiter
/// herein. Dann bricht die Host-Rolle hier ab (Rueckgabe 8), statt mit der
/// alten Liste weiterzulaufen und "geloescht" zu melden.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn forget_all() {
    if let Err(e) = freigaben_loeschen() {
        let zeile = format!("Freigaben NICHT geloescht: {e} - Abbruch, die bisherigen Gegenstellen waeren weiter freigegeben.");
        #[cfg(windows)]
        crate::host::log(zeile);
        #[cfg(not(windows))]
        eprintln!("{zeile}");
        std::process::exit(8);
    }
}

/// Loescht authorized.txt. Fehlt sie, ist das Erfolg; jeder andere Fehler
/// heisst: die alten Freigaben gelten weiter.
#[cfg_attr(not(windows), allow(dead_code))]
fn freigaben_loeschen() -> Result<(), String> {
    datei_loeschen(&authorized_path().ok_or("kein Ablageort fuer authorized.txt")?)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn datei_loeschen(p: &Path) -> Result<(), String> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {}", p.display(), wortlaut(&e))),
    }
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

    /// Eigener leerer Ordner je Test - die Tests laufen nebeneinander.
    fn ordner(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qc-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const A: [u8; 32] = [0xaa; 32];
    const B: [u8; 32] = [0xbb; 32];
    const C: [u8; 32] = [0xcc; 32];

    #[test]
    fn known_hosts_unlesbar_ist_kein_erstkontakt() {
        let d = ordner("kh-latin1");
        let p = d.join("known_hosts.txt");
        // Zwei gueltige Pins und eine Kommentarzeile in Latin-1 (0xE4 = ae).
        let mut inhalt = format!("10.0.0.5 {} x\n10.0.0.6 {} y\n", hex(&A), hex(&B)).into_bytes();
        inhalt.extend_from_slice(b"# Rechner im B\xe4ro\n");
        std::fs::write(&p, &inhalt).unwrap();
        // Frueher: leere Liste, Erstkontakt, Datei mit nur dem Fremden ueberschrieben.
        assert!(known_host_pruefen(&p, "10.0.0.5:9001", &C).is_err());
        assert!(known_host_pruefen(&p, "10.0.0.7:9001", &C).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), inhalt);
    }

    #[test]
    fn known_hosts_mit_bom() {
        let d = ordner("kh-bom");
        let p = d.join("known_hosts.txt");
        std::fs::write(&p, format!("\u{feff}10.0.0.5 {} x\n", hex(&A))).unwrap();
        let e = known_host_pruefen(&p, "10.0.0.5:9001", &C).unwrap_err();
        assert_eq!(
            e,
            Fehler::FingerabdruckGeaendert { host: "10.0.0.5".into(), fingerabdruck: noise::fingerprint(&C), pfad: p.clone() }
        );
        assert!(e.to_string().contains("geaendert"), "{e}");
        assert_eq!(known_host_pruefen(&p, "10.0.0.5:9001", &A), Ok(false));
    }

    #[test]
    fn known_hosts_neu_und_angehaengt() {
        let d = ordner("kh-neu");
        let p = d.join("known_hosts.txt");
        assert_eq!(known_host_pruefen(&p, "10.0.0.5:9001", &A), Ok(true));
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1);
        // Zwei Pins, der zweite ohne Zeilenende; ein dritter Host kommt dazu.
        let alt = format!("10.0.0.5 {} x\n10.0.0.6 {} y", hex(&A), hex(&B));
        std::fs::write(&p, &alt).unwrap();
        assert_eq!(known_host_pruefen(&p, "10.0.0.7:9001", &C), Ok(true));
        let neu = std::fs::read_to_string(&p).unwrap();
        assert!(neu.starts_with(&format!("{alt}\n")));
        assert_eq!(neu.lines().count(), 3);
        assert_eq!(known_host_pruefen(&p, "10.0.0.7:9001", &C), Ok(false));
        assert_eq!(known_host_pruefen(&p, "10.0.0.6:9001", &B), Ok(false));
    }

    #[cfg(unix)]
    #[test]
    fn known_hosts_nur_schreibbar() {
        use std::os::unix::fs::PermissionsExt;
        let d = ordner("kh-0200");
        let p = d.join("known_hosts.txt");
        let inhalt = format!("10.0.0.5 {} x\n", hex(&A));
        std::fs::write(&p, &inhalt).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o200)).unwrap();
        let r = known_host_pruefen(&p, "10.0.0.6:9001", &C);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), inhalt);
    }

    #[test]
    fn freigaben_unlesbar_heisst_abweisen() {
        let d = ordner("auth-ff");
        let p = d.join("authorized.txt");
        // Ein gueltiger Eintrag, dessen Namensfeld ein 0xFF-Byte traegt.
        let mut inhalt = format!("{}  {}  ", hex(&A), noise::fingerprint(&A)).into_bytes();
        inhalt.extend_from_slice(b"Rechner\xff\n");
        std::fs::write(&p, &inhalt).unwrap();
        // Pruefen, Erstkontakt und Eintragen haengen alle an diesem einen
        // Lesen - scheitert es, gibt es weder Erstkontakt noch Schreiben.
        assert!(Freigaben::lesen_aus(p.clone()).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), inhalt);

        // Fehlt die Datei: Erstkontakt, dann genau eine Zeile.
        let q = d.join("neu.txt");
        let f = Freigaben::lesen_aus(q.clone()).unwrap();
        assert_eq!((f.anzahl(), f.enthaelt(&A), f.erstkontakt()), (0, false, Ok(true)));
        f.aufnehmen(&A, "10.0.0.5").unwrap();
        let f = Freigaben::lesen_aus(q.clone()).unwrap();
        assert_eq!((f.anzahl(), f.enthaelt(&A), f.erstkontakt()), (1, true, Ok(false)));
        // Schon bekannt: keine zweite Zeile. Neuer: angehaengt.
        f.aufnehmen(&A, "10.0.0.5").unwrap();
        f.aufnehmen(&B, "").unwrap();
        let f = Freigaben::lesen_aus(q.clone()).unwrap();
        assert_eq!(f.anzahl(), 2);
        assert!(f.text.lines().next().unwrap().starts_with(&hex(&A)));
    }

    /// Vorhanden, lesbar, aber ohne gueltigen Eintrag: kein Erstkontakt.
    /// Aufnehmen (der Weg von --pair) geht trotzdem, und die Zeilen davor
    /// bleiben stehen.
    #[test]
    fn freigaben_ohne_gueltigen_eintrag_sind_kein_erstkontakt() {
        let d = ordner("auth-leer");
        let p = d.join("authorized.txt");
        let kaputt = format!("# Freigaben\n{}\n\n", &hex(&A)[..40]);
        for inhalt in ["", "\n", "\u{feff}", "# nur ein Kommentar, lang genug fuer 64 Zeichen ........................\n", &kaputt] {
            std::fs::write(&p, inhalt).unwrap();
            let f = Freigaben::lesen_aus(p.clone()).unwrap();
            assert_eq!(f.anzahl(), 0, "{inhalt:?}");
            let e = f.erstkontakt().unwrap_err();
            assert!(e.contains("keine gueltige Freigabe"), "{e}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), inhalt);
        }
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        f.aufnehmen(&B, "10.0.0.6").unwrap();
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        assert!(f.text.starts_with(&kaputt));
        assert_eq!((f.anzahl(), f.enthaelt(&B), f.erstkontakt()), (1, true, Ok(false)));
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
        assert_eq!(modus(&d.join("client.key")), 0o600);
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

    /// Der Pin wird VOR dem Verbinden gelesen und im Handschlag vor
    /// Nachricht 3 geprueft; beim ersten Kontakt erst danach eingetragen.
    #[test]
    fn pin_vor_dem_verbinden() {
        let d = ordner("pin");
        let p = d.join("known_hosts.txt");
        // Unlesbar (kein UTF-8): gar nicht erst verbinden, Datei bleibt.
        let mut kaputt = format!("10.0.0.5 {} x\n", hex(&A)).into_bytes();
        kaputt.extend_from_slice(b"# B\xe4ro\n");
        std::fs::write(&p, &kaputt).unwrap();
        assert_eq!(HostPin::laden_aus(p.clone(), "10.0.0.5:9001").err(), Some(Fehler::KeinUtf8 { pfad: p.clone() }));
        assert_eq!(std::fs::read(&p).unwrap(), kaputt);

        // Bekannter Host: der eigene Schluessel passt, ein anderer nicht;
        // eintragen schreibt nichts.
        let inhalt = format!("10.0.0.5 {} x\n", hex(&A));
        std::fs::write(&p, &inhalt).unwrap();
        let pin = HostPin::laden_aus(p.clone(), "10.0.0.5:9001").unwrap();
        assert_eq!(pin.pruefen(&A), Ok(()));
        assert_eq!(
            pin.pruefen(&C),
            Err(Fehler::FingerabdruckGeaendert { host: "10.0.0.5".into(), fingerabdruck: noise::fingerprint(&C), pfad: p.clone() })
        );
        assert_eq!(pin.eintragen(&A), Ok(false));
        assert!(pin.eintragen(&C).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), inhalt);

        // Erster Kontakt: jeder Schluessel passt, eingetragen wird erst
        // mit `eintragen`.
        let pin = HostPin::laden_aus(p.clone(), "10.0.0.6:9001").unwrap();
        assert_eq!(pin.pruefen(&B), Ok(()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), inhalt);
        assert_eq!(pin.eintragen(&B), Ok(true));
        assert_eq!(known_host_pruefen(&p, "10.0.0.6:9001", &B), Ok(false));

        // Fehlt die Liste, entsteht sie leer; der erste Kontakt geht.
        let q = d.join("neu.txt");
        let pin = HostPin::laden_aus(q.clone(), "10.0.0.7:9001").unwrap();
        assert_eq!(std::fs::read(&q).unwrap(), b"");
        assert_eq!(pin.eintragen(&C), Ok(true));

        // Schreibgeschuetzt: ein bekannter Host geht weiter, ein neuer wird
        // schon vor dem Verbinden abgelehnt - nicht erst, wenn der Host
        // dieses Geraet angenommen hat.
        let mut rechte = std::fs::metadata(&p).unwrap().permissions();
        rechte.set_readonly(true);
        std::fs::set_permissions(&p, rechte.clone()).unwrap();
        let bekannt = HostPin::laden_aus(p.clone(), "10.0.0.5:9001").map(|p| p.gemerkt.is_some());
        let neu = HostPin::laden_aus(p.clone(), "10.0.0.9:9001").err();
        rechte.set_readonly(false);
        std::fs::set_permissions(&p, rechte).unwrap();
        assert_eq!(bekannt, Ok(true));
        assert!(matches!(neu, Some(Fehler::Schreiben { .. })), "{neu:?}");
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
        let pfad = PathBuf::from("/ablage/known_hosts.txt");
        let mut gesehen = Vec::new();
        let r = Secure::connect_pruefend(&addr.to_string(), &noise::prologue_video(), |k| {
            gesehen = k.to_vec();
            Err(Fehler::FingerabdruckGeaendert { host: "127.0.0.1".into(), fingerabdruck: noise::fingerprint(k), pfad: pfad.clone() })
        });
        assert!(matches!(r, Err(Fehler::FingerabdruckGeaendert { .. })));
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
        let fremd = [9u8; 64];
        let k = schluessel_laden_mit(&p, || {
            std::fs::write(&p, fremd).unwrap();
            noise::keypair()
        })
        .unwrap();
        assert_eq!((k.0.as_slice(), k.1.as_slice()), (&fremd[..32], &fremd[32..]));
        assert_eq!(std::fs::read(&p).unwrap(), fremd);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        // Direkt: vorhandenes Ziel heisst AlreadyExists, Inhalt bleibt.
        let e = geheim_schreiben(&p, &[1u8; 64]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), fremd);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    }

    /// Ein Schreibfehler mitten im Anhaengen hinterlaesst nichts: eine
    /// vorhandene Liste hat danach ihre alte Laenge, eine vorher fehlende
    /// ist wieder weg - der Erstkontakt bleibt erhalten.
    #[test]
    fn anhaengen_scheitert_ohne_reste() {
        let d = ordner("auth-rest");
        let halb = |f: &mut std::fs::File, b: &[u8]| -> std::io::Result<()> {
            f.write_all(&b[..b.len() / 2])?;
            Err(std::io::Error::other("Platte voll"))
        };
        let p = d.join("authorized.txt");
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        assert!(f.aufnehmen_mit(&A, "10.0.0.5", halb).is_err());
        assert!(!p.exists(), "leere oder halbe Liste blieb stehen");
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        assert_eq!(f.erstkontakt(), Ok(true));

        let alt = format!("{}  {}  x\n", hex(&A), noise::fingerprint(&A));
        std::fs::write(&p, &alt).unwrap();
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        assert!(f.aufnehmen_mit(&B, "10.0.0.6", halb).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), alt);

        // Beim Lesen fehlte die Liste, dann legt ein zweiter Host-Prozess sie
        // mit seiner Zeile an, und erst danach scheitert das eigene
        // Anhaengen: seine Liste bleibt stehen, nur der halbe eigene Teil geht.
        std::fs::remove_file(&p).unwrap();
        let f = Freigaben::lesen_aus(p.clone()).unwrap();
        std::fs::write(&p, &alt).unwrap();
        assert!(f.aufnehmen_mit(&B, "10.0.0.6", halb).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), alt);
    }

    /// Eine zu kurze Schluesseldatei wird einmal neu gelesen, bevor sie als
    /// beschaedigt gilt: ohne harte Verweise legt ein zweiter Prozess sie
    /// direkt an, und wer mitten darin liest, saehe sie halb.
    #[test]
    fn halber_schluessel_wird_nachgelesen() {
        let d = ordner("key-halb");
        let p = d.join("client.key");
        let ganz: Vec<u8> = (0..64u8).collect();
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

    /// Der Wortlaut des Systems steht in einer Zeile.
    #[test]
    fn wortlaut_in_einer_zeile() {
        let e = std::io::Error::other("Eine bestehende Verbindung wurde softwaregesteuert\r\ndurch den Hostcomputer abgebrochen.");
        assert_eq!(wortlaut(&e), "Eine bestehende Verbindung wurde softwaregesteuert durch den Hostcomputer abgebrochen.");
    }

    /// Grossbuchstaben im Hex zaehlen wie auf dem Mac (strncasecmp).
    #[test]
    fn freigabe_ohne_ruecksicht_auf_schreibweise() {
        let d = ordner("auth-gross");
        let p = d.join("authorized.txt");
        std::fs::write(&p, format!("{}  x\n", hex(&A).to_uppercase())).unwrap();
        let f = Freigaben::lesen_aus(p).unwrap();
        assert_eq!((f.anzahl(), f.enthaelt(&A), f.enthaelt(&B)), (1, true, false));
    }

    /// --forget: fehlt die Liste, ist das Erfolg; laesst sie sich nicht
    /// loeschen, ist es ein Fehler (dann bricht die Host-Rolle ab).
    #[test]
    fn freigaben_loeschen_meldet_fehler() {
        let d = ordner("forget");
        let p = d.join("authorized.txt");
        assert_eq!(datei_loeschen(&p), Ok(()));
        std::fs::write(&p, "x\n").unwrap();
        assert_eq!(datei_loeschen(&p), Ok(()));
        assert!(!p.exists());
        // Nicht zu loeschen: unter Windows eine Datei, die ein anderes
        // Programm ohne FILE_SHARE_DELETE offen haelt (Virenscanner,
        // Sicherung - eine schreibgeschuetzte loescht remove_file dort
        // inzwischen trotzdem), unter Unix ein Ordner ohne Schreibrecht.
        std::fs::write(&p, "x\n").unwrap();
        #[cfg(windows)]
        let r = {
            use std::os::windows::fs::OpenOptionsExt;
            let _offen = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
            datei_loeschen(&p)
        };
        #[cfg(unix)]
        let r = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
            let r = datei_loeschen(&p);
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
            r
        };
        assert!(r.is_err(), "{r:?}");
        assert!(p.exists());
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
