// Gesicherte Leitung auf der Windows-Seite.
//
// Nach dem Handschlag laeuft der komplette bisherige Datenstrom durch diese
// Schicht: Stueck fuer Stueck verschluesselt, auf der anderen Seite wieder
// zusammengesetzt. Aufrufer sehen nur read_exact und write_all wie vorher.

use crate::noise;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

pub const CHUNK_MAX: usize = 65519;

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
        sock.set_read_timeout(Some(Duration::from_secs(3))).ok();
        sock.set_write_timeout(Some(Duration::from_secs(3))).ok();
        let mut rd = sock.try_clone().map_err(|e| format!("Leitung: {e}"))?;
        let wr = sock.try_clone().map_err(|e| format!("Leitung: {e}"))?;

        let recv = |buf: &mut Vec<u8>| -> Result<(), String> {
            let mut l = [0u8; 2];
            rd.read_exact(&mut l).map_err(|e| format!("Laenge: {e}"))?;
            let n = u16::from_le_bytes(l) as usize;
            buf.resize(n, 0);
            rd.read_exact(buf).map_err(|e| format!("Daten: {e}"))
        };
        let send = |data: &[u8]| -> Result<(), String> {
            let mut out = Vec::with_capacity(2 + data.len());
            out.extend_from_slice(&(data.len() as u16).to_le_bytes());
            out.extend_from_slice(data);
            (&wr).write_all(&out).map_err(|e| format!("Senden: {e}"))
        };

        let s = noise::handshake_initiator(&priv_key, prologue, recv, send)?;
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
    /// Hosts, nicht mit client.key. Rahmen und Fristen wie bei `wrap`.
    pub fn accept(sock: TcpStream, prologue: &[u8], priv_key: &[u8]) -> Result<Secure, String> {
        sock.set_nodelay(true).ok();
        sock.set_read_timeout(Some(Duration::from_secs(3))).ok();
        sock.set_write_timeout(Some(Duration::from_secs(3))).ok();
        let mut rd = sock.try_clone().map_err(|e| format!("Leitung: {e}"))?;
        let wr = sock.try_clone().map_err(|e| format!("Leitung: {e}"))?;

        let recv = |buf: &mut Vec<u8>| -> Result<(), String> {
            let mut l = [0u8; 2];
            rd.read_exact(&mut l).map_err(|e| format!("Laenge: {e}"))?;
            let n = u16::from_le_bytes(l) as usize;
            buf.resize(n, 0);
            rd.read_exact(buf).map_err(|e| format!("Daten: {e}"))
        };
        let send = |data: &[u8]| -> Result<(), String> {
            let mut out = Vec::with_capacity(2 + data.len());
            out.extend_from_slice(&(data.len() as u16).to_le_bytes());
            out.extend_from_slice(data);
            (&wr).write_all(&out).map_err(|e| format!("Senden: {e}"))
        };

        let s = noise::handshake_responder(priv_key, prologue, recv, send)?;
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
pub fn config_dir() -> Result<PathBuf, String> {
    let dir = basis_ordner()?.join("QuadChroma");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Ordner: {e}"))?;
    Ok(dir)
}

#[cfg(target_os = "macos")]
fn basis_ordner() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "kein Ablageort fuer Einstellungen gefunden".to_string())?;
    Ok(PathBuf::from(home).join("Library").join("Application Support"))
}

#[cfg(not(target_os = "macos"))]
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
    let path = config_dir()?.join("client.key");
    if let Ok(b) = std::fs::read(&path) {
        if b.len() == 64 {
            return Ok((b[..32].to_vec(), b[32..].to_vec()));
        }
    }
    let (priv_key, pub_key) = noise::keypair()?;
    let mut both = priv_key.clone();
    both.extend_from_slice(&pub_key);
    std::fs::write(&path, &both).map_err(|e| format!("Schluessel schreiben: {e}"))?;
    Ok((priv_key, pub_key))
}

/// Merkt sich den Schluessel eines Hosts. Gibt Err zurueck, wenn sich der
/// Fingerabdruck einer bekannten Adresse geaendert hat - dann stimmt etwas nicht.
pub fn check_known_host(addr: &str, peer: &[u8]) -> Result<bool, String> {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).to_string();
    let path = config_dir()?.join("known_hosts.txt");
    let hex: String = peer.iter().map(|b| format!("{b:02x}")).collect();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
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
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("{host} {hex} {}\n", noise::fingerprint(peer)));
    std::fs::write(&path, text).map_err(|e| format!("known_hosts: {e}"))?;
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
    let path = config_dir()?.join("host.key");
    if let Ok(b) = std::fs::read(&path) {
        if b.len() == 64 {
            return Ok((b[..32].to_vec(), b[32..].to_vec()));
        }
    }
    let (priv_key, pub_key) = noise::keypair()?;
    let mut both = priv_key.clone();
    both.extend_from_slice(&pub_key);
    std::fs::write(&path, &both).map_err(|e| format!("Schluessel schreiben: {e}"))?;
    Ok((priv_key, pub_key))
}

fn hex(peer: &[u8]) -> String {
    peer.iter().map(|b| format!("{b:02x}")).collect()
}

fn authorized_path() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("authorized.txt"))
}

/// Ist diese Gegenstelle freigegeben? Verglichen werden die ersten 64
/// Zeichen der Zeile, wie auf dem Mac.
pub fn is_authorized(peer: &[u8]) -> bool {
    let Some(p) = authorized_path() else { return false };
    let h = hex(peer);
    std::fs::read_to_string(p).map(|t| t.lines().any(|l| l.starts_with(&h))).unwrap_or(false)
}

/// Gegenstelle aufnehmen. Schon bekannte Zeilen werden nicht verdoppelt.
pub fn authorize(peer: &[u8], name: &str) -> Result<(), String> {
    if is_authorized(peer) {
        return Ok(());
    }
    let p = authorized_path().ok_or("kein Ablageort")?;
    let mut text = std::fs::read_to_string(&p).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "{}  {}  {}\n",
        hex(peer),
        noise::fingerprint(peer),
        if name.is_empty() { "-" } else { name }
    ));
    std::fs::write(&p, text).map_err(|e| format!("authorized.txt: {e}"))
}

/// Anzahl der freigegebenen Gegenstellen.
pub fn authorized_count() -> usize {
    let Some(p) = authorized_path() else { return 0 };
    std::fs::read_to_string(p).map(|t| t.lines().filter(|l| l.len() > 64).count()).unwrap_or(0)
}

/// Alle Freigaben loeschen (--forget).
pub fn forget_all() {
    if let Some(p) = authorized_path() {
        let _ = std::fs::remove_file(p);
    }
}
