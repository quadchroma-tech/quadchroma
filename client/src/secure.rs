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

fn config_dir() -> Result<PathBuf, String> {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "kein Ablageort fuer Einstellungen gefunden".to_string())?;
    let dir = PathBuf::from(base).join("QuadChroma");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Ordner: {e}"))?;
    Ok(dir)
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
