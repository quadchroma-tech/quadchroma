// Zuschauerplatz des Windows-Hosts: Annahme auf dem Bildkanal (port) und
// dem Eingabekanal (port + 1), Bekanntgabe per Rundruf (port + 2), Versand
// mit Stauregel, Kopplung.
//
// Ein Zuschauer zur Zeit; ein neuer ersetzt den alten (wie main.m). Der
// Bildkanal wird nur geschrieben, der Eingabekanal nur gelesen - Antworten
// (Zeit, Einstellungen) gehen ueber den Bildkanal zurueck.
//
// Stauregel: Windows kennt kein SO_NWRITE. Ersatz ist ein eigener
// Sendefaden mit Warteschlange und Byte-Budget (2 MB, Gaming 512 kB) bei
// einem Sendepuffer von 256 kB im Kernel: ein blockierender write im
// Sendefaden spiegelt die Leitung, die Warteschlange davor ist der Stau.
// Ueberschreitet Warteschlange + Kernelanteil das Budget, faellt das Bild
// weg und ein Vollbild wird erzwungen. send_small (Ton, Zwischenablage,
// Zeiger, Info, Einstellungen, Codecs, Switch, Zeit, Last, Hoststatus)
// geht an der Regel vorbei, aber durch dieselbe Warteschlange - die
// Reihenfolge Switch -> Info -> Vollbild bleibt damit erhalten.

use std::collections::VecDeque;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use super::{eingabe, encoder, log, now_us, Z};
use crate::protokoll_konst::*;
use crate::{noise, secure};

/// Ungesendete Bytes, ab denen Bilder verworfen werden.
const BACKLOG_LIMIT: usize = 2 * 1024 * 1024;
/// Sendepuffer im Kernel; zaehlt als geschaetzter Anteil zum Stau dazu.
const SNDBUF: usize = 256 * 1024;

struct Warteschlange {
    pakete: VecDeque<Vec<u8>>,
    bytes: usize,
    offen: bool,
}

/// Ein verbundener Zuschauer: Warteschlange zum Sendefaden und die Kennung
/// seines Handschlags, an die sich der Eingabekanal bindet.
pub struct Leitung {
    q: Mutex<Warteschlange>,
    cv: Condvar,
    pub peer: Vec<u8>,
    pub hh: Vec<u8>,
    pub ip: String,
}

impl Leitung {
    fn schliessen(&self) {
        let mut q = self.q.lock().unwrap();
        q.offen = false;
        self.cv.notify_all();
    }

    /// Ein Paket einreihen. Mit Budget: nur, wenn Warteschlange plus
    /// Kernelanteil darunter bleiben - sonst false (Stau).
    fn einreihen(&self, paket: Vec<u8>, budget: Option<usize>) -> bool {
        let mut q = self.q.lock().unwrap();
        if !q.offen {
            return false;
        }
        if let Some(b) = budget {
            if q.bytes + SNDBUF > b {
                return false;
            }
        }
        q.bytes += paket.len();
        q.pakete.push_back(paket);
        self.cv.notify_one();
        true
    }
}

static AKTUELL: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
static PRIV: OnceLock<Vec<u8>> = OnceLock::new();
static SEQ: AtomicU32 = AtomicU32::new(0);
/// Laufende Nummer der Zuschauer, fuer das Protokoll.
static NR: AtomicU64 = AtomicU64::new(0);

pub fn zuschauer_da() -> bool {
    AKTUELL.lock().unwrap().is_some()
}

fn aktuell() -> Option<Arc<Leitung>> {
    AKTUELL.lock().unwrap().clone()
}

/// Kopf einer Nachricht.
fn kopf(typ: u8, flags: u8, reserviert: u16, len: usize) -> [u8; 8] {
    let mut h = [0u8; 8];
    h[0] = typ;
    h[1] = flags;
    h[2..4].copy_from_slice(&reserviert.to_le_bytes());
    h[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    h
}

/// Kleine Nachricht ueber die Bildverbindung. Umgeht bewusst die Vollbild-
/// Sperre und die Stauregel: Ton und Zwischenablage sind winzig und duerfen
/// nicht warten.
pub fn send_small(typ: u8, data: &[u8]) {
    let Some(l) = aktuell() else { return };
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&kopf(typ, 0, 0, data.len()));
    p.extend_from_slice(data);
    l.einreihen(p, None);
}

pub fn settings_senden() {
    let mut cur = [0u8; 9];
    cur[0..4].copy_from_slice(&Z.mbit.load(Ordering::Relaxed).to_le_bytes());
    cur[4..6].copy_from_slice(&(Z.fps.load(Ordering::Relaxed) as u16).to_le_bytes());
    cur[6] = Z.gaming.load(Ordering::Relaxed) as u8;
    cur[7] = Z.fest.load(Ordering::Relaxed) as u8;
    cur[8] = Z.ton.load(Ordering::Relaxed) as u8;
    send_small(MSG_SETTINGS, &cur);
}

pub fn strominfo_senden() {
    send_small(MSG_INFO, &super::strominfo());
}

pub fn hoststatus_senden(lage: u8) {
    send_small(MSG_HOSTSTATUS, &[lage, 0]);
}

/// Ergebnis eines Bildversands.
#[derive(PartialEq, Eq, Debug)]
pub enum Versand {
    Gesendet,
    /// Kein Zuschauer, oder der wartet noch auf ein Vollbild.
    Verworfen,
    /// Stau auf der Leitung: verworfen, Vollbild erzwungen.
    Stau,
}

/// Eine Zugriffseinheit samt Stempel (Nachricht 5) in EINEM Paket. Ein
/// frisch verbundener Zuschauer bekommt erst ab dem naechsten Vollbild
/// Daten; staut es sich, faellt das Bild weg und ein Vollbild wird
/// erzwungen - g_wait_key bleibt dabei stehen, wie auf dem Mac.
pub fn bild_senden(au: &[u8], key: bool, t_cap: u64, wiederholt: bool) -> Versand {
    let Some(l) = aktuell() else { return Versand::Verworfen };
    if Z.wait_key.load(Ordering::Relaxed) && !key {
        return Versand::Verworfen;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let t_enc = now_us();
    if !wiederholt && t_enc > t_cap {
        Z.enc_us.fetch_add(t_enc - t_cap, Ordering::Relaxed);
        Z.enc_n.fetch_add(1, Ordering::Relaxed);
    }
    let mut p = Vec::with_capacity(8 + 24 + 8 + au.len());
    p.extend_from_slice(&kopf(MSG_STAMP, 0, 0, 24));
    p.extend_from_slice(&seq.to_le_bytes());
    p.push(wiederholt as u8);
    p.extend_from_slice(&[0, 0, 0]);
    p.extend_from_slice(&t_cap.to_le_bytes());
    p.extend_from_slice(&t_enc.to_le_bytes());
    p.extend_from_slice(&kopf(MSG_VIDEO, if key { FLAG_KEY } else { 0 }, (seq & 0xffff) as u16, au.len()));
    p.extend_from_slice(au);
    let budget = if Z.gaming.load(Ordering::Relaxed) { BACKLOG_LIMIT / 4 } else { BACKLOG_LIMIT };
    if !l.einreihen(p, Some(budget)) {
        Z.stau.fetch_add(1, Ordering::Relaxed);
        Z.force_key.store(true, Ordering::Relaxed);
        return Versand::Stau;
    }
    // Erst wenn das Bild wirklich rausgeht, ist das Warten vorbei.
    Z.wait_key.store(false, Ordering::Relaxed);
    Z.sent_frames.fetch_add(1, Ordering::Relaxed);
    Z.sent_bytes.fetch_add(au.len() as u64, Ordering::Relaxed);
    Versand::Gesendet
}

/// Sendefaden eines Zuschauers: nimmt Pakete aus der Warteschlange und
/// schreibt sie verschluesselt auf die Leitung. Ein Fehler beim Schreiben
/// heisst: der Zuschauer ist weg.
fn sendefaden(l: Arc<Leitung>, mut sock: secure::Secure) {
    loop {
        let paket = {
            let mut q = l.q.lock().unwrap();
            while q.pakete.is_empty() && q.offen {
                q = l.cv.wait(q).unwrap();
            }
            match q.pakete.pop_front() {
                Some(p) => {
                    q.bytes -= p.len();
                    p
                }
                None => break,
            }
        };
        if let Err(e) = sock.write_all(&paket) {
            log(format!("Zuschauer weg: {e}"));
            l.schliessen();
            break;
        }
    }
    let mut a = AKTUELL.lock().unwrap();
    if a.as_ref().map(|x| Arc::ptr_eq(x, &l)).unwrap_or(false) {
        *a = None;
        drop(a);
        eingabe::alle_tasten_loslassen();
        log("Zuschauer getrennt - kein Zuschauer");
    }
}

/// Sendepuffer im Kernel klein halten, damit ein blockierender write die
/// Leitung spiegelt (siehe Stauregel oben).
fn sendepuffer_setzen(s: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_SNDBUF};
    let wert = (SNDBUF as i32).to_ne_bytes();
    unsafe {
        setsockopt(SOCKET(s.as_raw_socket() as usize), SOL_SOCKET, SO_SNDBUF, Some(&wert));
    }
}

fn annahme_bild(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let ip = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
        let port = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
        sendepuffer_setzen(&stream);

        // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
        let sock = match secure::Secure::accept(stream, &noise::prologue_video(), PRIV.get().unwrap()) {
            Ok(s) => s,
            Err(e) => {
                log(format!("Handschlag mit {ip} gescheitert ({e})"));
                continue;
            }
        };
        let fp = sock.peer_fingerprint();
        let sas = sock.sas.clone();

        // Freigabe: bekannte Gegenstelle, oder das Kopplungsfenster steht
        // offen, oder es gibt noch gar keine Freigabe (Erstkontakt).
        if !secure::is_authorized(&sock.peer) {
            if Z.pair_open.load(Ordering::Relaxed) || secure::authorized_count() == 0 {
                if let Err(e) = secure::authorize(&sock.peer, &ip) {
                    log(format!("Freigabe konnte nicht gespeichert werden: {e}"));
                }
                log(format!("Neue Gegenstelle gekoppelt: {fp} ({ip}), Vergleichscode {sas}"));
                Z.pair_open.store(false, Ordering::Relaxed);
            } else {
                log(format!(
                    "Abgewiesen: unbekannte Gegenstelle {fp} von {ip}. Host mit --pair starten, um sie aufzunehmen."
                ));
                // Die Leitung faellt mit `sock` zu - der Client deutet das
                // Ende nach dem Handschlag als "nicht gekoppelt".
                continue;
            }
        }

        // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
        // Fenstergroesse und Format kennt, bevor das erste Bild kommt.
        let mut hello = Vec::with_capacity(4 + 8 + 8);
        hello.extend_from_slice(MAGIC);
        hello.extend_from_slice(&kopf(MSG_INFO, 0, 0, 8));
        hello.extend_from_slice(&super::strominfo());

        let leitung = Arc::new(Leitung {
            q: Mutex::new(Warteschlange { pakete: VecDeque::new(), bytes: 0, offen: true }),
            cv: Condvar::new(),
            peer: sock.peer.clone(),
            hh: sock.handshake_hash.clone(),
            ip: ip.clone(),
        });
        // Ein neuer Zuschauer ersetzt den alten: dessen Sendefaden endet,
        // sobald seine Warteschlange zu ist, und nimmt die Leitung mit.
        {
            let mut a = AKTUELL.lock().unwrap();
            if let Some(alt) = a.take() {
                alt.schliessen();
                eingabe::alle_tasten_loslassen();
            }
            leitung.einreihen(hello, None);
            *a = Some(leitung.clone());
        }
        NR.fetch_add(1, Ordering::Relaxed);
        let l2 = leitung.clone();
        std::thread::spawn(move || sendefaden(l2, sock));

        Z.force_key.store(true, Ordering::Relaxed);
        Z.wait_key.store(true, Ordering::Relaxed);
        // Zeigerform und Tonformat gehen dem neuen Zuschauer erneut zu.
        super::zeiger::neu_senden();
        super::ton::info_zuruecksetzen();
        settings_senden();
        send_small(MSG_CODECS, &encoder::codecs_payload());
        log(format!("Zuschauer verbunden: {ip}:{port}, verschluesselt, Gegenstelle {fp}, Vergleichscode {sas}"));
    }
}

// ------------------------------------------------------------ Eingabe-Teil
// Zweite Verbindung, nur fuer Maus, Tastatur und Zwischenablage. Getrennt
// vom Bild, damit eine Mausbewegung nie hinter einem Vollbild haengt.

fn annahme_eingabe(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        stream.set_nodelay(true).ok();

        // Der Eingabekanal darf erst aufmachen, wenn der Bildkanal steht: sein
        // Prologue enthaelt dessen Pruefsumme. Wer die nicht kennt, kommt hier
        // nicht durch - damit kann niemand nur die Tastatur uebernehmen.
        let Some(bild) = aktuell() else {
            log("Eingabekanal abgewiesen: kein Bildkanal offen");
            continue;
        };
        let mut sock = match secure::Secure::accept(stream, &noise::prologue_input(&bild.hh), PRIV.get().unwrap()) {
            Ok(s) => s,
            Err(e) => {
                log(format!("Eingabekanal: Handschlag gescheitert ({e})"));
                continue;
            }
        };
        if sock.peer != bild.peer {
            log("Eingabekanal abgewiesen: andere Gegenstelle als beim Bild");
            continue;
        }
        log("Eingabekanal verbunden, verschluesselt");

        let mut hdr = [0u8; 8];
        let mut payload: Vec<u8> = Vec::new();
        loop {
            if sock.read_exact(&mut hdr).is_err() {
                break;
            }
            let typ = hdr[0];
            let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
            // Text kann gross sein (bis 4 MB); alles andere ist winzig.
            let grenze = if typ == IN_CLIP { 4 * 1024 * 1024 } else { 256 };
            if len > grenze {
                break;
            }
            payload.resize(len, 0);
            if len > 0 && sock.read_exact(&mut payload).is_err() {
                break;
            }
            match typ {
                IN_MOVE | IN_BUTTON | IN_SCROLL | IN_KEY => eingabe::verarbeiten(typ, &payload),
                IN_CLIP => {
                    if let Ok(text) = std::str::from_utf8(&payload) {
                        crate::clipboard::set(text);
                    }
                }
                IN_TIME => {
                    // Zeitabgleich: die Frage traegt die Uhrzeit des Clients,
                    // die Antwort gibt sie zurueck und haengt die des Hosts an.
                    if len >= 8 {
                        let mut out = [0u8; 16];
                        out[0..8].copy_from_slice(&payload[0..8]);
                        out[8..16].copy_from_slice(&now_us().to_le_bytes());
                        send_small(MSG_TIME, &out);
                    }
                }
                IN_SETTINGS => {
                    if len >= 8 {
                        let m = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                        let f = u16::from_le_bytes([payload[4], payload[5]]) as u32;
                        // Ein Client ohne das neunte Byte will Ton.
                        let ton = if len >= 9 { payload[8] != 0 } else { true };
                        super::apply_settings(m, f, payload[6] != 0, payload[7] != 0, ton);
                    }
                }
                IN_TESTBILD => {
                    // Der Aufnahmefaden setzt es um und meldet es; mit einer
                    // Konserve gibt es keinen Encoder, der es speisen koennte.
                    if len >= 1 {
                        let an = payload[0] != 0;
                        if Z.konserve.load(Ordering::Relaxed) {
                            log(format!("Testbild {}: mit Konserve nicht moeglich", if an { "an" } else { "aus" }));
                        } else {
                            Z.testbild.store(an, Ordering::Relaxed);
                        }
                    }
                }
                IN_CODEC => {
                    if len >= 1 {
                        encoder::codec_wunsch(payload[0] as usize);
                    }
                }
                _ => {}
            }
        }
        log("Eingabekanal getrennt");
        eingabe::alle_tasten_loslassen();
    }
}

// ------------------------------------------------------------ Bekanntgabe
// Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, damit Clients
// ihn ohne eingetippte Adresse finden. An die Rundrufadresse JEDER aktiven
// IPv4-Netzwerkkarte, wie auf dem Mac - die allgemeine 255.255.255.255 ist
// nur der Rueckfall, falls sich die Karten nicht abfragen lassen.

/// Rundrufadressen aller aktiven IPv4-Karten (Unicast + Praefixlaenge ->
/// Broadcast), ohne Loopback.
fn rundruf_adressen() -> Vec<std::net::Ipv4Addr> {
    use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    let mut adressen = Vec::new();
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        let r = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if r == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if r != 0 {
            return adressen;
        }
        break;
    }
    let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !a.is_null() {
            let ad = &*a;
            // OperStatus 1 = IfOperStatusUp; IfType 24 = Loopback.
            if ad.OperStatus.0 == 1 && ad.IfType != 24 {
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    let ua = &*u;
                    let sa = ua.Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let ip = u32::from_be(sin.sin_addr.S_un.S_addr);
                        let praefix = ua.OnLinkPrefixLength as u32;
                        if praefix < 32 {
                            let maske = if praefix == 0 { 0 } else { !0u32 << (32 - praefix) };
                            adressen.push(std::net::Ipv4Addr::from(ip | !maske));
                        }
                    }
                    u = ua.Next;
                }
            }
            a = ad.Next;
        }
    }
    adressen
}

fn bekanntgabe(port: u16) {
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) else { return };
    sock.set_broadcast(true).ok();
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into());
    let mut nlen = name.len().min(40);
    while !name.is_char_boundary(nlen) {
        nlen -= 1;
    }
    let mut pkt = Vec::with_capacity(8 + nlen);
    pkt.extend_from_slice(BEACON_MAGIC);
    pkt.push(BEACON_VERSION);
    pkt.extend_from_slice(&port.to_le_bytes());
    pkt.push(nlen as u8);
    pkt.extend_from_slice(&name.as_bytes()[..nlen]);
    let mut erste = true;
    loop {
        let mut ziele = rundruf_adressen();
        if ziele.is_empty() {
            ziele.push(std::net::Ipv4Addr::BROADCAST);
        }
        let mut gesendet = 0;
        for z in &ziele {
            if sock.send_to(&pkt, (*z, port + 2)).is_ok() {
                gesendet += 1;
            }
        }
        if erste {
            log(format!("Bekanntgabe: an {gesendet} Netze, Port {}", port + 2));
            erste = false;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Annahmefaeden und Bekanntgabe starten.
pub fn start(port: u16, priv_key: Vec<u8>) -> Result<(), String> {
    PRIV.set(priv_key).ok();
    let bild = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("Bild-Port {port} nicht verfuegbar: {e}"))?;
    let eingabe = TcpListener::bind(("0.0.0.0", port + 1))
        .map_err(|e| format!("Eingabe-Port {} nicht verfuegbar: {e}", port + 1))?;
    std::thread::spawn(move || annahme_bild(bild));
    std::thread::spawn(move || annahme_eingabe(eingabe));
    std::thread::spawn(move || bekanntgabe(port));
    Ok(())
}
