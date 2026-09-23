// Gesicherte Leitung auf der Windows-Seite.
//
// Nach dem Handschlag laeuft der komplette bisherige Datenstrom durch diese
// Schicht: Stueck fuer Stueck verschluesselt, auf der anderen Seite wieder
// zusammengesetzt. Aufrufer sehen nur read_exact und write_all wie vorher.

use crate::noise;
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
    /// rechtzeitig geantwortet.
    Handschlag { grund: String, frist: bool },
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
}

impl Rahmen<'_> {
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
                Err(e) => return Err(format!("{was}: {e}")),
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
        (&*self.sock).write_all(&out).map_err(|e| format!("Senden: {e}"))
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
    /// Verbindet und fuehrt den Handschlag als Anrufer.
    pub fn connect(addr: &str, prologue: &[u8]) -> Result<Secure, Fehler> {
        // Mit Frist verbinden. Ohne sie haengt ein Aufruf an einer toten
        // Adresse gut zwanzig Sekunden - und wenn das im Fensterfaden
        // passiert, steht so lange die ganze Oberflaeche.
        let adresse = |grund: Option<String>| Fehler::Adresse { addr: addr.to_string(), grund };
        let ziel = addr
            .to_socket_addrs()
            .map_err(|e| adresse(Some(e.to_string())))?
            .next()
            .ok_or_else(|| adresse(None))?;
        let sock = TcpStream::connect_timeout(&ziel, Duration::from_secs(2)).map_err(|e| Fehler::Verbindung {
            addr: addr.to_string(),
            art: e.kind(),
            grund: e.to_string(),
        })?;
        sock.set_nodelay(true).ok();
        Secure::wrap(sock, prologue)
    }

    pub fn wrap(sock: TcpStream, prologue: &[u8]) -> Result<Secure, Fehler> {
        let (priv_key, _pub_key) = identity()?;
        // Waehrend des Handschlags gilt eine Frist. Danach wird sie wieder
        // aufgehoben: der Bildkanal darf beliebig lange still sein, ohne dass
        // daraus ein Fehler wird.
        let r = Rahmen { sock: &sock, frist: Instant::now() + FRIST_ANRUFER };
        let s = noise::handshake_initiator(&priv_key, prologue, |b| r.recv(b), |d| r.send(d))
            .map_err(|grund| Fehler::Handschlag { frist: grund.contains(FRIST_ABGELAUFEN), grund })?;
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
        let r = Rahmen { sock: &sock, frist: Instant::now() + FRIST_ANNAHME };
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
        (&self.sock).write_all(&out).map_err(|e| format!("Senden: {e}"))
    }

    pub fn read_exact(&mut self, dst: &mut [u8]) -> Result<(), String> {
        let mut done = 0;
        while done < dst.len() {
            if self.inpos == self.inbuf.len() {
                let mut l = [0u8; 2];
                (&self.sock).read_exact(&mut l).map_err(|e| format!("Laenge: {e}"))?;
                let n = u16::from_le_bytes(l) as usize;
                if n > CHUNK_MAX + 16 {
                    return Err("unplausible Datensatzlaenge".into());
                }
                let mut ct = vec![0u8; n];
                (&self.sock).read_exact(&mut ct).map_err(|e| format!("Daten: {e}"))?;
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
    b.create(&dir).map_err(|e| format!("Ordner: {e}"))?;
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
    match std::fs::read(path) {
        Ok(b) if b.len() == 64 => return Ok((b[..32].to_vec(), b[32..].to_vec())),
        Ok(b) => return Err(Fehler::SchluesselBeschaedigt { pfad: path.to_path_buf(), laenge: b.len() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Fehler::Unlesbar { pfad: path.to_path_buf(), grund: e.to_string() }),
    }
    let schreiben = |grund: String| Fehler::Schreiben { pfad: path.to_path_buf(), grund };
    let (priv_key, pub_key) = noise::keypair().map_err(schreiben)?;
    let mut both = priv_key.clone();
    both.extend_from_slice(&pub_key);
    geheim_schreiben(path, &both).map_err(|e| schreiben(e.to_string()))?;
    Ok((priv_key, pub_key))
}

/// Legt eine Schluesseldatei an: erst unter einem Zwischennamen, dann
/// umbenannt - ein Abbruch mittendrin hinterlaesst nie einen halben
/// Schluessel. Unter Unix nur fuer den Eigentuemer lesbar (0600, wie host.key
/// des Mac-Hosts); unter Windows gilt die geerbte Liste (siehe config_dir).
fn geheim_schreiben(path: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.neu", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let r = o.open(&tmp).and_then(|mut f| {
        f.write_all(inhalt)?;
        f.sync_all()
    });
    let r = r.and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
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
        Err(e) => return Err(Fehler::Unlesbar { pfad: path.to_path_buf(), grund: e.to_string() }),
    };
    let text = String::from_utf8(b).map_err(|_| Fehler::KeinUtf8 { pfad: path.to_path_buf() })?;
    Ok(Some(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text)))
}

/// Haengt eine Zeile an eine Vertrauensliste an. Die vorhandenen Zeilen
/// werden nie neu geschrieben - ein Fehler beim Schreiben kann also keinen
/// bestehenden Eintrag kosten. `bisher` ist der gerade gelesene Inhalt; fehlt
/// ihm das letzte Zeilenende, kommt es vor die neue Zeile.
fn liste_anhaengen(path: &Path, bisher: &str, zeile: &str) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    let mut neu = String::new();
    if !bisher.is_empty() && !bisher.ends_with('\n') {
        neu.push('\n');
    }
    neu.push_str(zeile);
    neu.push('\n');
    f.write_all(neu.as_bytes())?;
    f.sync_all()
}

/// Merkt sich den Schluessel eines Hosts. Gibt Err zurueck, wenn sich der
/// Fingerabdruck einer bekannten Adresse geaendert hat - dann stimmt etwas nicht.
/// Ebenso, wenn known_hosts.txt vorhanden, aber nicht lesbar ist: dann wird
/// nicht verbunden.
pub fn check_known_host(addr: &str, peer: &[u8]) -> Result<bool, Fehler> {
    known_host_pruefen(&config_dir().map_err(Fehler::Ablage)?.join("known_hosts.txt"), addr, peer)
}

fn known_host_pruefen(path: &Path, addr: &str, peer: &[u8]) -> Result<bool, Fehler> {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).to_string();
    let hex: String = peer.iter().map(|b| format!("{b:02x}")).collect();
    let text = liste_lesen(path)?;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(h), Some(k)) = (it.next(), it.next()) else { continue };
        if h == host {
            if k == hex {
                return Ok(false); // bekannt und unveraendert
            }
            return Err(Fehler::FingerabdruckGeaendert {
                host,
                fingerabdruck: noise::fingerprint(peer),
                pfad: path.to_path_buf(),
            });
        }
    }
    liste_anhaengen(path, &text, &format!("{host} {hex} {}", noise::fingerprint(peer)))
        .map_err(|e| Fehler::Schreiben { pfad: path.to_path_buf(), grund: e.to_string() })?;
    Ok(true) // erstmals gesehen
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
    /// Zeichen der Zeile, wie auf dem Mac.
    pub fn enthaelt(&self, peer: &[u8]) -> bool {
        let h = hex(peer);
        self.text.lines().any(|l| l.starts_with(&h))
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
        if self.enthaelt(peer) {
            return Ok(());
        }
        let zeile = format!("{}  {}  {}", hex(peer), noise::fingerprint(peer), if name.is_empty() { "-" } else { name });
        liste_anhaengen(&self.pfad, &self.text, &zeile).map_err(|e| format!("authorized.txt: {e}"))
    }
}

/// Anzahl der freigegebenen Gegenstellen, nur fuer die Startzeile im
/// Protokoll (unlesbar: 0). Entscheidungen laufen ueber `Freigaben`.
pub fn authorized_count() -> usize {
    Freigaben::lesen().map(|f| f.anzahl()).unwrap_or(0)
}

/// Alle Freigaben loeschen (--forget).
pub fn forget_all() {
    if let Some(p) = authorized_path() {
        let _ = std::fs::remove_file(p);
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
