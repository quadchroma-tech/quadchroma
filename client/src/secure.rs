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

/// Frist fuer den GANZEN Handschlag, nicht je Leseaufruf. Eine Frist je
/// Aufruf haelt ein Gegenueber, das alle zwei Sekunden ein Byte schickt,
/// jedes Mal ein - und bindet den Annehmenden damit stundenlang. Der
/// Anrufer liest nur Nachricht 2 und bleibt bei den bisherigen 3 s; der
/// Angerufene liest zwei Nachrichten mit einem Hin und Her dazwischen.
const FRIST_ANRUFER: Duration = Duration::from_secs(3);
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
            return Err("Frist fuer den Handschlag abgelaufen".into());
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
                    return Err(format!("{was}: Frist fuer den Handschlag abgelaufen"))
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
    pub fn connect(addr: &str, prologue: &[u8]) -> Result<Secure, String> {
        // Mit Frist verbinden. Ohne sie haengt ein Aufruf an einer toten
        // Adresse gut zwanzig Sekunden - und wenn das im Fensterfaden
        // passiert, steht so lange die ganze Oberflaeche.
        let ziel = addr
            .to_socket_addrs()
            .map_err(|e| format!("Adresse {addr}: {e}"))?
            .next()
            .ok_or_else(|| format!("Adresse {addr} ergibt kein Ziel"))?;
        let sock = TcpStream::connect_timeout(&ziel, Duration::from_secs(2))
            .map_err(|e| format!("Verbindung zu {addr}: {e}"))?;
        sock.set_nodelay(true).ok();
        Secure::wrap(sock, prologue)
    }

    pub fn wrap(sock: TcpStream, prologue: &[u8]) -> Result<Secure, String> {
        let (priv_key, _pub_key) = identity()?;
        // Waehrend des Handschlags gilt eine Frist. Danach wird sie wieder
        // aufgehoben: der Bildkanal darf beliebig lange still sein, ohne dass
        // daraus ein Fehler wird.
        let r = Rahmen { sock: &sock, frist: Instant::now() + FRIST_ANRUFER };
        let s = noise::handshake_initiator(&priv_key, prologue, |b| r.recv(b), |d| r.send(d))?;
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
pub fn identity() -> Result<(Vec<u8>, Vec<u8>), String> {
    // Im Ablauf stehen privater und oeffentlicher Teil hintereinander, damit
    // beim Start nichts nachgerechnet werden muss.
    schluessel_laden(&config_dir()?.join("client.key"))
}

/// Liest einen Schluessel (64 Byte: privat, dann oeffentlich) oder legt ihn
/// an - aber NUR, wenn die Datei fehlt. Ist sie unlesbar oder hat sie die
/// falsche Laenge, bleibt sie liegen: ein stilles Ueberschreiben machte aus
/// einem Dateifehler eine neue Identitaet, und jede Kopplung waere weg.
fn schluessel_laden(path: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    match std::fs::read(path) {
        Ok(b) if b.len() == 64 => return Ok((b[..32].to_vec(), b[32..].to_vec())),
        Ok(b) => {
            return Err(format!(
                "{} ist beschaedigt ({} statt 64 Byte) und wird nicht ueberschrieben. \
                 Datei pruefen oder loeschen - dann entsteht ein neuer Schluessel.",
                path.display(),
                b.len()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} nicht lesbar: {e}", path.display())),
    }
    let (priv_key, pub_key) = noise::keypair()?;
    let mut both = priv_key.clone();
    both.extend_from_slice(&pub_key);
    geheim_schreiben(path, &both).map_err(|e| format!("Schluessel schreiben: {e}"))?;
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
fn liste_lesen(path: &Path) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("{name} nicht lesbar: {e}")),
    };
    let text = String::from_utf8(b).map_err(|_| format!("{name} ist kein UTF-8"))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
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
/// Ebenso, wenn known_hosts.txt vorhanden, aber nicht lesbar ist.
pub fn check_known_host(addr: &str, peer: &[u8]) -> Result<bool, String> {
    known_host_pruefen(&config_dir()?.join("known_hosts.txt"), addr, peer)
}

fn known_host_pruefen(path: &Path, addr: &str, peer: &[u8]) -> Result<bool, String> {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).to_string();
    let hex: String = peer.iter().map(|b| format!("{b:02x}")).collect();
    let text = liste_lesen(path).map_err(|e| format!("{e} - Verbindung abgelehnt"))?;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(h), Some(k)) = (it.next(), it.next()) else { continue };
        if h == host {
            if k == hex {
                return Ok(false); // bekannt und unveraendert
            }
            return Err(format!(
                "Der Fingerabdruck von {host} hat sich geaendert. Verbindung abgelehnt. \
                 Wenn der Host neu aufgesetzt wurde, den Eintrag in known_hosts.txt loeschen."
            ));
        }
    }
    liste_anhaengen(path, &text, &format!("{host} {hex} {}", noise::fingerprint(peer)))
        .map_err(|e| format!("known_hosts: {e}"))?;
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
pub fn host_identity() -> Result<(Vec<u8>, Vec<u8>), String> {
    schluessel_laden(&config_dir()?.join("host.key"))
}

fn hex(peer: &[u8]) -> String {
    peer.iter().map(|b| format!("{b:02x}")).collect()
}

fn authorized_path() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("authorized.txt"))
}

/// Die Freigabeliste; ohne Ablageort ein Fehler, keine leere Liste.
fn freigaben_lesen() -> Result<(PathBuf, String), String> {
    let p = authorized_path().ok_or("kein Ablageort fuer authorized.txt")?;
    let text = liste_lesen(&p)?;
    Ok((p, text))
}

fn freigegeben_in(text: &str, peer: &[u8]) -> bool {
    let h = hex(peer);
    text.lines().any(|l| l.starts_with(&h))
}

fn freigaben_in(text: &str) -> usize {
    text.lines().filter(|l| l.len() > 64).count()
}

/// Ist diese Gegenstelle freigegeben? Verglichen werden die ersten 64
/// Zeichen der Zeile, wie auf dem Mac. Err, wenn die Liste vorhanden, aber
/// nicht lesbar ist - der Aufrufer weist dann ab.
pub fn is_authorized(peer: &[u8]) -> Result<bool, String> {
    freigaben_lesen().map(|(_, t)| freigegeben_in(&t, peer))
}

/// Gegenstelle aufnehmen. Schon bekannte Zeilen werden nicht verdoppelt.
/// Ist die Liste nicht lesbar, wird NICHTS geschrieben.
pub fn authorize(peer: &[u8], name: &str) -> Result<(), String> {
    let (p, text) = freigaben_lesen()?;
    freigabe_anhaengen(&p, &text, peer, name)
}

fn freigabe_anhaengen(p: &Path, text: &str, peer: &[u8], name: &str) -> Result<(), String> {
    if freigegeben_in(text, peer) {
        return Ok(());
    }
    let zeile = format!("{}  {}  {}", hex(peer), noise::fingerprint(peer), if name.is_empty() { "-" } else { name });
    liste_anhaengen(p, text, &zeile).map_err(|e| format!("authorized.txt: {e}"))
}

/// Anzahl der freigegebenen Gegenstellen. Err, wenn die Liste vorhanden,
/// aber nicht lesbar ist - dann ist es KEIN Erstkontakt.
pub fn freigaben_zaehlen() -> Result<usize, String> {
    freigaben_lesen().map(|(_, t)| freigaben_in(&t))
}

/// Anzahl der freigegebenen Gegenstellen, nur fuer die Startzeile im
/// Protokoll (unlesbar: 0). Entscheidungen laufen ueber `freigaben_zaehlen`.
pub fn authorized_count() -> usize {
    freigaben_zaehlen().unwrap_or(0)
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
        assert!(e.contains("geaendert"), "{e}");
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
        // is_authorized, freigaben_zaehlen und authorize lesen alle ueber
        // liste_lesen - scheitert es, gibt es weder Erstkontakt noch Schreiben.
        assert!(liste_lesen(&p).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), inhalt);

        // Fehlt die Datei: Erstkontakt, dann genau eine Zeile.
        let q = d.join("neu.txt");
        let t = liste_lesen(&q).unwrap();
        assert_eq!((freigaben_in(&t), freigegeben_in(&t, &A)), (0, false));
        freigabe_anhaengen(&q, &t, &A, "10.0.0.5").unwrap();
        let t = liste_lesen(&q).unwrap();
        assert_eq!((freigaben_in(&t), freigegeben_in(&t, &A)), (1, true));
        // Schon bekannt: keine zweite Zeile. Neuer: angehaengt.
        freigabe_anhaengen(&q, &t, &A, "10.0.0.5").unwrap();
        freigabe_anhaengen(&q, &t, &B, "").unwrap();
        let t = liste_lesen(&q).unwrap();
        assert_eq!(freigaben_in(&t), 2);
        assert!(t.lines().next().unwrap().starts_with(&hex(&A)));
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
