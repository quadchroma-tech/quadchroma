// Hosts im lokalen Netz finden.
//
// Der Host ruft sich alle zwei Sekunden per Rundruf aus, wir hoeren zu und
// fuehren eine Liste. Kein Bonjour, kein Dienst, keine zusaetzliche
// Abhaengigkeit: ein UDP-Paket mit Name und Port reicht.
//
// Aufbau des Pakets: "QCHB" | u8 Version | u16 Bildport | u8 Namenslaenge |
// Name, dahinter (Pairing v1, Abschnitt 2) u8 ext = 1 | u32 Geraete-ID | u8
// Flags. Gelesen wird es in zugang::bekanntgabe_lesen; ohne Erweiterung
// (aeltere Hosts) gibt es keine ID, die Oberflaeche zeigt dann "-". Die
// Liste bleibt nach ip:port geschluesselt; die ID dient Anzeige und Suche -
// vertraut wird ihr nie (das tut erst der Schluessel im Handschlag).
//
// Selbstschutz: laeuft auf diesem Rechner ein Host, kommt seine eigene
// Bekanntgabe hier ebenfalls an. Seine ID (aus host.key, secure::
// eigener_host_schluessel, alle EIGEN_TAKT neu gelesen) blendet die Liste
// aus - man verbindet sich nicht mit sich selbst.

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

/// Ein gefundener Host samt dem, was seine Bekanntgabe ueber den Zugang
/// sagt.
#[derive(Clone, Debug)]
pub struct Gefunden {
    pub host: Host,
    /// Geraete-ID aus der Erweiterung; None bei aelteren Hosts.
    pub id: Option<u32>,
    /// BEACON_FLAG_* (Bit 0: am Host kann jemand "Zulassen" klicken).
    pub flags: u8,
}

#[derive(Default)]
pub struct Hosts {
    map: HashMap<String, Gefunden>,
    /// ID des Hosts auf diesem Rechner - diese Bekanntgabe zeigt die Liste
    /// nicht.
    eigene_id: Option<u32>,
}

/// So oft liest der Empfangsfaden host.key neu: die Host-Rolle kann nach
/// dem Client starten und ihren Schluessel erst dann anlegen.
const EIGEN_TAKT: Duration = Duration::from_secs(5);

impl Hosts {
    /// Gefundene Hosts, nach Namen sortiert, ohne die seit zehn Sekunden stummen.
    pub fn list(&self) -> Vec<Host> {
        self.liste().into_iter().map(|g| g.host).collect()
    }

    /// Wie `list`, samt ID und Flags jedes Hosts - ohne den Host auf diesem
    /// Rechner (eigene_id).
    pub fn liste(&self) -> Vec<Gefunden> {
        let mut v: Vec<Gefunden> = self
            .map
            .values()
            .filter(|g| g.host.seen.elapsed() < Duration::from_secs(10))
            .filter(|g| self.eigene_id.is_none() || g.id != self.eigene_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.host.name.cmp(&b.host.name).then(a.host.addr.cmp(&b.host.addr)));
        v
    }

    /// Der Host, der sich mit dieser ID meldet (der zuletzt gehoerte, falls
    /// sich zwei damit melden).
    pub fn mit_id(&self, id: u32) -> Option<Gefunden> {
        self.liste().into_iter().filter(|g| g.id == Some(id)).max_by_key(|g| g.host.seen)
    }

    /// Die ID des Hosts auf diesem Rechner (None: keiner).
    pub fn eigene_id_setzen(&mut self, id: Option<u32>) {
        self.eigene_id = id;
    }

    /// Ein Paket einer Gegenstelle eintragen. false: keine Bekanntgabe.
    pub fn eintragen(&mut self, paket: &[u8], von: SocketAddr) -> bool {
        let Some(b) = crate::zugang::bekanntgabe_lesen(paket) else { return false };
        let addr = SocketAddr::new(von.ip(), b.port);
        let host = Host { name: b.name, addr, seen: Instant::now() };
        self.map.insert(addr.to_string(), Gefunden { host, id: b.id, flags: b.flags });
        true
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
        let mut buf = [0u8; crate::zugang::BEKANNTGABE_MAX];
        let mut eigen_gelesen: Option<Instant> = None;
        loop {
            if eigen_gelesen.is_none_or(|t| t.elapsed() >= EIGEN_TAKT) {
                eigen_gelesen = Some(Instant::now());
                let id = crate::secure::eigener_host_schluessel().map(|k| crate::zugang::geraete_id(&k));
                if let Ok(mut h) = hosts.lock() {
                    h.eigene_id_setzen(id);
                }
            }
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => continue, // Zeitablauf ist der Normalfall
            };
            if let Ok(mut h) = hosts.lock() {
                h.eintragen(&buf[..n], from);
            }
        }
    });

    out
}

fn bind_reusable(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(("0.0.0.0", port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mit Erweiterung: ID und Flags; ohne (aelterer Host): keine ID, der
    /// Eintrag gilt trotzdem. Geschluesselt nach ip:port des Bildkanals.
    #[test]
    fn bekanntgabe_mit_und_ohne_id() {
        let mut h = Hosts::default();
        let von: SocketAddr = "192.168.178.194:50000".parse().unwrap();
        assert!(h.eintragen(&crate::zugang::bekanntgabe(9001, "Mac mini", 581_729_911, 1), von));
        let mut alt = b"QCHB\x01".to_vec();
        alt.extend_from_slice(&9101u16.to_le_bytes());
        alt.push(6);
        alt.extend_from_slice(b"studio");
        assert!(h.eintragen(&alt, "192.168.178.60:40000".parse().unwrap()));
        assert!(!h.eintragen(b"QCHX\x01\x29\x23\x00", von));
        let l = h.liste();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].host.name.as_str(), l[0].id, l[0].flags), ("Mac mini", Some(581_729_911), 1));
        assert_eq!(l[0].host.addr.to_string(), "192.168.178.194:9001");
        assert_eq!((l[1].host.name.as_str(), l[1].id), ("studio", None));
        assert_eq!(l[1].host.addr.to_string(), "192.168.178.60:9101");
        assert_eq!(h.mit_id(581_729_911).map(|g| g.host.addr.to_string()).as_deref(), Some("192.168.178.194:9001"));
        assert!(h.mit_id(5).is_none());
        assert_eq!(h.list().len(), 2);
    }

    /// Selbstschutz: die eigene Bekanntgabe (ID des Hosts auf diesem
    /// Rechner) zeigt die Liste nicht, auch nicht ueber die ID; Hosts ohne
    /// ID bleiben. Faellt die eigene ID weg, ist er wieder da.
    #[test]
    fn eigene_bekanntgabe_ausgeblendet() {
        let mut h = Hosts::default();
        let hier: SocketAddr = "192.168.178.60:50000".parse().unwrap();
        assert!(h.eintragen(&crate::zugang::bekanntgabe(9001, "Dieser PC", 123_456_789, 1), hier));
        assert!(h.eintragen(&crate::zugang::bekanntgabe(9001, "Mac mini", 581_729_911, 1), "192.168.178.194:5".parse().unwrap()));
        let mut alt = b"QCHB\x01".to_vec();
        alt.extend_from_slice(&9101u16.to_le_bytes());
        alt.push(6);
        alt.extend_from_slice(b"studio");
        assert!(h.eintragen(&alt, "192.168.178.61:40000".parse().unwrap()));
        assert_eq!(h.liste().len(), 3);
        h.eigene_id_setzen(Some(123_456_789));
        let namen: Vec<String> = h.list().into_iter().map(|x| x.name).collect();
        assert_eq!(namen, vec!["Mac mini".to_string(), "studio".to_string()]);
        assert!(h.mit_id(123_456_789).is_none());
        assert!(h.mit_id(581_729_911).is_some());
        h.eigene_id_setzen(None);
        assert_eq!(h.liste().len(), 3);
    }
}
