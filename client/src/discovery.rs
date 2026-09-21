// Hosts im lokalen Netz finden.
//
// Der Host ruft sich alle zwei Sekunden per Rundruf aus, wir hoeren zu und
// fuehren eine Liste. Kein Bonjour, kein Dienst, keine zusaetzliche
// Abhaengigkeit: ein UDP-Paket mit Name und Port reicht.
//
// Aufbau des Pakets: "QCHB" | u8 Version | u16 Bildport | u8 Namenslaenge | Name

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Host {
    pub name: String,
    pub addr: SocketAddr,
    pub seen: Instant,
}

#[derive(Default)]
pub struct Hosts {
    map: HashMap<String, Host>,
}

impl Hosts {
    /// Gefundene Hosts, neueste Meldung zuerst, ohne die seit zehn Sekunden stummen.
    pub fn list(&self) -> Vec<Host> {
        let mut v: Vec<Host> = self
            .map
            .values()
            .filter(|h| h.seen.elapsed() < Duration::from_secs(10))
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// Startet das Zuhoeren in einem eigenen Faden. Schlaegt das Binden fehl, etwa
/// weil schon ein anderer Client laeuft, bleibt die Liste einfach leer.
pub fn start(port: u16) -> Arc<Mutex<Hosts>> {
    let hosts = Arc::new(Mutex::new(Hosts::default()));
    let out = hosts.clone();

    std::thread::spawn(move || {
        let sock = match bind_reusable(port) {
            Ok(s) => s,
            Err(_) => return,
        };
        sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let mut buf = [0u8; 128];
        loop {
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => continue, // Zeitablauf ist der Normalfall
            };
            if n < 8 || &buf[0..4] != b"QCHB" || buf[4] != 1 {
                continue;
            }
            let video_port = u16::from_le_bytes([buf[5], buf[6]]);
            let nlen = buf[7] as usize;
            if n < 8 + nlen {
                continue;
            }
            let name = String::from_utf8_lossy(&buf[8..8 + nlen]).to_string();
            let addr = SocketAddr::new(from.ip(), video_port);
            let key = addr.to_string();
            if let Ok(mut h) = hosts.lock() {
                h.map.insert(
                    key,
                    Host { name, addr, seen: Instant::now() },
                );
            }
        }
    });

    out
}

fn bind_reusable(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(("0.0.0.0", port))
}
