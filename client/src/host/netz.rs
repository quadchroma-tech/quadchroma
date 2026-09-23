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
//
// Sitzung: Der Eingabekanal gehoert zu genau einem Zuschauer. Geht der
// (ersetzt oder weg), kappt `Leitung::schliessen` auch dessen Eingabe, und
// die Eingabeschleife speist nichts mehr ein. Handschlaege laufen je
// Verbindung in einem eigenen Faden, mit Frist (secure.rs, FRIST_ANNAHME) und
// Obergrenze - eine stumme Verbindung sperrt damit niemanden mehr aus.
// Windows bricht ein blockierendes recv/send auf ein shutdown hin nicht ab,
// solange die Gegenstelle lebt, aber schweigt (eingefrorener Prozess): Der
// Faden des alten Kanals bleibt dann stehen, bis sie geht - harmlos, weil er
// vor jedem Einspeisen prueft, ob er noch gilt. Ist das Geraet ganz weg,
// endet er, wenn TCP das FIN aufgibt.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// Abbruchgriff des Eingabekanals dieses Zuschauers, mit seiner Nummer.
    /// Liegt unter derselben Sperre wie `offen`: wer schliesst, sieht einen
    /// eben eingetragenen Griff sicher, und wer eintragen will, sieht sicher,
    /// dass der Zuschauer schon weg ist.
    eingabe: Option<(u64, TcpStream)>,
}

/// Ein verbundener Zuschauer: Warteschlange zum Sendefaden und die Kennung
/// seines Handschlags, an die sich der Eingabekanal bindet.
pub struct Leitung {
    q: Mutex<Warteschlange>,
    cv: Condvar,
    /// Zweite Hand an der Bildleitung: `schliessen` kappt sie auch dann,
    /// wenn der Sendefaden in einem write haengt (Zuschauer eingefroren).
    bild_griff: Option<TcpStream>,
    pub peer: Vec<u8>,
    pub hh: Vec<u8>,
    pub ip: String,
}

impl Leitung {
    fn neu(bild_griff: Option<TcpStream>, peer: Vec<u8>, hh: Vec<u8>, ip: String) -> Leitung {
        Leitung {
            q: Mutex::new(Warteschlange { pakete: VecDeque::new(), bytes: 0, offen: true, eingabe: None }),
            cv: Condvar::new(),
            bild_griff,
            peer,
            hh,
            ip,
        }
    }

    /// Zuschauer beenden: Warteschlange zu, Bildleitung und Eingabekanal
    /// gekappt. Kein Bild, keine Eingabe - ein abgeloester Zuschauer behaelt
    /// nichts, auch wenn er seine Leitungen selbst offen hielte.
    fn schliessen(&self) {
        let eingabe = {
            let mut q = self.q.lock().unwrap();
            q.offen = false;
            self.cv.notify_all();
            q.eingabe.take()
        };
        if let Some((_, s)) = eingabe {
            let _ = s.shutdown(Shutdown::Both);
            log("Eingabekanal gekappt: sein Zuschauer ist abgeloest oder weg");
        }
        if let Some(s) = &self.bild_griff {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    #[cfg(test)]
    fn offen(&self) -> bool {
        self.q.lock().unwrap().offen
    }

    /// Gilt Eingabekanal `nr` noch? Nur, solange sein Zuschauer da ist und
    /// kein neuerer Eingabekanal desselben Zuschauers ihn abgeloest hat.
    fn gilt(&self, nr: u64) -> bool {
        let q = self.q.lock().unwrap();
        q.offen && q.eingabe.as_ref().map(|(n, _)| *n == nr).unwrap_or(false)
    }

    /// Eingabekanal an diesen Zuschauer binden. false, wenn der Zuschauer
    /// inzwischen abgeloest ist (Wechsel waehrend des Handschlags). Ein
    /// frueherer Eingabekanal desselben Zuschauers wird gekappt und seine
    /// Tasten losgelassen. Unter EINSPEISEN aufrufen.
    fn eingabe_binden(&self, nr: u64, griff: TcpStream) -> bool {
        let alt = {
            let mut q = self.q.lock().unwrap();
            if !q.offen {
                return false;
            }
            q.eingabe.replace((nr, griff))
        };
        if let Some((_, s)) = alt {
            let _ = s.shutdown(Shutdown::Both);
            log("Eingabekanal gekappt: derselbe Zuschauer hat einen neuen aufgebaut");
            eingabe::alle_tasten_loslassen();
        }
        true
    }

    /// Eigenen Griff austragen, wenn er noch der eingetragene ist - sonst
    /// hielte die Kopie die Leitung offen, nachdem die Schleife sie verlassen
    /// hat. true: der Kanal galt bis zuletzt.
    fn eingabe_loesen(&self, nr: u64) -> bool {
        let mut q = self.q.lock().unwrap();
        if q.eingabe.as_ref().map(|(n, _)| *n == nr).unwrap_or(false) {
            q.eingabe = None;
            return true;
        }
        false
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
/// Laufende Nummer der Eingabekanaele (siehe Leitung::eingabe_loesen).
static EINGABE_NR: AtomicU64 = AtomicU64::new(0);
/// Entscheidung ueber die Freigabe samt Eintrag: seit die Handschlaege
/// nebeneinander laufen, kaemen sonst zwei Unbekannte zugleich durch das
/// eine Kopplungsfenster (oder den Erstkontakt).
static FREIGABE: Mutex<()> = Mutex::new(());
/// Einspeisen und Abloesen schliessen sich aus: Die Eingabeschleife prueft
/// unter dieser Sperre, ob ihr Kanal noch gilt, und fuehrt die Nachricht
/// aus; wer einen Zuschauer oder Eingabekanal abloest und danach alle Tasten
/// loslaesst, tut das ebenfalls darunter. Sonst koennte eine Taste des Alten
/// genau zwischen Pruefung und Loslassen gedrueckt werden und haengen
/// bleiben. Reihenfolge: erst EINSPEISEN, dann AKTUELL.
static EINSPEISEN: Mutex<()> = Mutex::new(());

/// Handschlaege, die gerade laufen - je Port hoechstens vier, je Absender
/// hoechstens zwei. Mit nur einem Annahmefaden genuegte eine einzige stumme
/// Verbindung, um jeden weiteren Zuschauer auszusperren; ohne Grenze banden
/// Verbindungen ohne Schluessel beliebig viele Faeden.
const HANDSCHLAEGE_MAX: usize = 4;
const HANDSCHLAEGE_JE_ABSENDER: usize = 2;

struct Plaetze {
    /// Absender der laufenden Handschlaege, und wann zuletzt eine Abweisung
    /// im Protokoll stand (hoechstens alle zehn Sekunden eine).
    laufend: Mutex<(Vec<IpAddr>, Option<Instant>)>,
}

static PLAETZE_BILD: Plaetze = Plaetze { laufend: Mutex::new((Vec::new(), None)) };
static PLAETZE_EINGABE: Plaetze = Plaetze { laufend: Mutex::new((Vec::new(), None)) };

/// Ein belegter Platz; gibt sich beim Wegfallen selbst frei.
struct Platz {
    p: &'static Plaetze,
    ip: IpAddr,
}

impl Plaetze {
    fn belegen(&'static self, ip: IpAddr, kanal: &str) -> Option<Platz> {
        let mut l = self.laufend.lock().unwrap();
        let je_absender = l.0.iter().filter(|x| **x == ip).count();
        if l.0.len() >= HANDSCHLAEGE_MAX || je_absender >= HANDSCHLAEGE_JE_ABSENDER {
            if l.1.map(|t| t.elapsed() >= Duration::from_secs(10)).unwrap_or(true) {
                l.1 = Some(Instant::now());
                log(format!(
                    "{kanal}: Verbindung von {ip} abgewiesen - schon {} Handschlaege offen ({je_absender} von dort)",
                    l.0.len()
                ));
            }
            return None;
        }
        l.0.push(ip);
        Some(Platz { p: self, ip })
    }
}

impl Drop for Platz {
    fn drop(&mut self) {
        let mut l = self.p.laufend.lock().unwrap();
        if let Some(i) = l.0.iter().position(|x| *x == self.ip) {
            l.0.swap_remove(i);
        }
    }
}

fn absender(s: &TcpStream) -> IpAddr {
    s.peer_addr().map(|a| a.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

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
    let _einspeisen = EINSPEISEN.lock().unwrap();
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
        // Der Handschlag laeuft in einem eigenen Faden: wer ihn nicht zu
        // Ende bringt, haelt nur seinen Platz, nicht die Annahme.
        let Some(platz) = PLAETZE_BILD.belegen(absender(&stream), "Bildkanal") else { continue };
        std::thread::spawn(move || bild_annehmen(stream, platz));
    }
}

fn bild_annehmen(stream: TcpStream, platz: Platz) {
    let ip = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let port = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
    sendepuffer_setzen(&stream);

    // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
    let sock = match secure::Secure::accept(stream, &noise::prologue_video(), PRIV.get().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            log(format!("Handschlag mit {ip} gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    let fp = sock.peer_fingerprint();
    let sas = sock.sas.clone();

    // Freigabe: bekannte Gegenstelle, oder das Kopplungsfenster steht
    // offen, oder es gibt noch gar keine Freigabe (Erstkontakt). Eine
    // vorhandene, aber unlesbare Liste ist KEIN Erstkontakt - dann wird
    // abgewiesen, ebenso wenn sich die neue Freigabe nicht speichern laesst.
    {
        let _freigabe = FREIGABE.lock().unwrap();
        let bekannt = match secure::is_authorized(&sock.peer) {
            Ok(b) => b,
            Err(e) => {
                log(format!("Abgewiesen: Gegenstelle {fp} von {ip} - {e}"));
                return;
            }
        };
        if !bekannt {
            let anzahl = match secure::freigaben_zaehlen() {
                Ok(n) => n,
                Err(e) => {
                    log(format!("Abgewiesen: Gegenstelle {fp} von {ip} - {e}"));
                    return;
                }
            };
            if Z.pair_open.load(Ordering::Relaxed) || anzahl == 0 {
                if let Err(e) = secure::authorize(&sock.peer, &ip) {
                    log(format!("Abgewiesen: Freigabe fuer {fp} ({ip}) konnte nicht gespeichert werden: {e}"));
                    return;
                }
                log(format!("Neue Gegenstelle gekoppelt: {fp} ({ip}), Vergleichscode {sas}"));
                Z.pair_open.store(false, Ordering::Relaxed);
            } else {
                log(format!(
                    "Abgewiesen: unbekannte Gegenstelle {fp} von {ip}. Host mit --pair starten, um sie aufzunehmen."
                ));
                // Die Leitung faellt mit `sock` zu - der Client deutet das
                // Ende nach dem Handschlag als "nicht gekoppelt".
                return;
            }
        }
    }

    // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
    // Fenstergroesse und Format kennt, bevor das erste Bild kommt.
    let mut hello = Vec::with_capacity(4 + 8 + 8);
    hello.extend_from_slice(MAGIC);
    hello.extend_from_slice(&kopf(MSG_INFO, 0, 0, 8));
    hello.extend_from_slice(&super::strominfo());

    let leitung = Arc::new(Leitung::neu(sock.abbruchgriff(), sock.peer.clone(), sock.handshake_hash.clone(), ip.clone()));
    // Ein neuer Zuschauer ersetzt den alten: dessen Bildleitung und
    // Eingabekanal werden gekappt, sein Sendefaden endet.
    {
        let _einspeisen = EINSPEISEN.lock().unwrap();
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
        let Some(platz) = PLAETZE_EINGABE.belegen(absender(&stream), "Eingabekanal") else { continue };
        std::thread::spawn(move || eingabe_annehmen(stream, bild, platz));
    }
}

fn eingabe_annehmen(stream: TcpStream, bild: Arc<Leitung>, platz: Platz) {
    let mut sock = match secure::Secure::accept(stream, &noise::prologue_input(&bild.hh), PRIV.get().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            log(format!("Eingabekanal: Handschlag gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    if sock.peer != bild.peer {
        log("Eingabekanal abgewiesen: andere Gegenstelle als beim Bild");
        return;
    }
    // Erst jetzt an den Zuschauer binden - und nur, wenn er noch der
    // aktuelle ist: ein Wechsel waehrend des Handschlags macht diese
    // Leitung wertlos. Ohne Abbruchgriff liesse sie sich nicht kappen.
    let nr = EINGABE_NR.fetch_add(1, Ordering::Relaxed);
    let einspeisen = EINSPEISEN.lock().unwrap();
    let Some(griff) = sock.abbruchgriff() else {
        log("Eingabekanal abgewiesen: Leitung nicht zu fassen");
        return;
    };
    if !bild.eingabe_binden(nr, griff) {
        log("Eingabekanal abgewiesen: Zuschauer inzwischen gewechselt");
        return;
    }
    drop(einspeisen);
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
        // Abgeloest? Dann nichts mehr einspeisen - auch nicht, was schon
        // entschluesselt im Puffer lag. Pruefen und Ausfuehren unter EINSPEISEN.
        let _einspeisen = EINSPEISEN.lock().unwrap();
        if !bild.gilt(nr) {
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
    // Galt der Kanal bis zuletzt, gibt er seine Tasten selbst frei. Sonst hat
    // das schon getan, wer ihn abgeloest hat - und ein spaet endender alter
    // Faden liesse sonst die Tasten des neuen Zuschauers los.
    let _einspeisen = EINSPEISEN.lock().unwrap();
    let galt = bild.eingabe_loesen(nr);
    log("Eingabekanal getrennt");
    if galt {
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
    // Die Startzeile zaehlt eine unlesbare Liste als 0 - hier steht, was das heisst.
    if let Err(e) = secure::freigaben_zaehlen() {
        log(format!("Freigabeliste: {e} - jede Gegenstelle wird abgewiesen, bis die Datei repariert ist"));
    }
    let bild = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("Bild-Port {port} nicht verfuegbar: {e}"))?;
    let eingabe = TcpListener::bind(("0.0.0.0", port + 1))
        .map_err(|e| format!("Eingabe-Port {} nicht verfuegbar: {e}", port + 1))?;
    std::thread::spawn(move || annahme_bild(bild));
    std::thread::spawn(move || annahme_eingabe(eingabe));
    std::thread::spawn(move || bekanntgabe(port));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// `schliessen` kappt Bildleitung und eingetragenen Eingabekanal: ein
    /// blockierendes Lesen auf der Gegenseite endet sofort, obwohl hier noch
    /// weitere Griffe an denselben Leitungen offen sind. Danach laesst sich
    /// kein Eingabekanal mehr eintragen.
    #[test]
    fn schliessen_kappt_eingabe_und_bild() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let mut ein_c = TcpStream::connect(addr).unwrap();
        let (ein_h, _) = l.accept().unwrap();
        let mut bild_c = TcpStream::connect(addr).unwrap();
        let (bild_h, _) = l.accept().unwrap();
        let leitung = Leitung::neu(bild_h.try_clone().ok(), Vec::new(), Vec::new(), String::new());
        assert!(leitung.eingabe_binden(1, ein_h.try_clone().unwrap()));
        ein_c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        bild_c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let faden = std::thread::spawn(move || {
            let mut b = [0u8; 1];
            let t0 = Instant::now();
            let e = ein_c.read(&mut b);
            let b2 = bild_c.read(&mut b);
            (matches!(e, Ok(0)) || e.is_err(), matches!(b2, Ok(0)) || b2.is_err(), t0.elapsed())
        });
        std::thread::sleep(Duration::from_millis(200));
        leitung.schliessen();
        let (ein_zu, bild_zu, dauer) = faden.join().unwrap();
        assert!(ein_zu && bild_zu);
        // Deutlich unter der Lesefrist von 5 s: gekappt, nicht abgelaufen.
        assert!(dauer < Duration::from_secs(2), "{dauer:?}");
        assert!(!leitung.eingabe_binden(2, ein_h.try_clone().unwrap()));
        assert!(!leitung.offen());
    }

    /// Liest vom Bildkanal, bis eine Zeitantwort kommt; gibt die
    /// zurueckgespiegelte Client-Uhrzeit zurueck.
    fn zeitantwort(s: &mut secure::Secure) -> Result<u64, String> {
        loop {
            let mut h = [0u8; 8];
            s.read_exact(&mut h)?;
            let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
            let mut p = vec![0u8; len];
            s.read_exact(&mut p)?;
            if h[0] == MSG_TIME && len >= 8 {
                return Ok(u64::from_le_bytes(p[0..8].try_into().unwrap()));
            }
        }
    }

    fn zeitfrage(s: &mut secure::Secure, t: u64) -> Result<(), String> {
        let mut m = kopf(IN_TIME, 0, 0, 8).to_vec();
        m.extend_from_slice(&t.to_le_bytes());
        s.write_all(&m)
    }

    fn bild_verbinden(addr: &str) -> secure::Secure {
        let t0 = Instant::now();
        let mut s = secure::Secure::connect(addr, &noise::prologue_video()).unwrap();
        let mut magic = [0u8; 4];
        s.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, MAGIC);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        s
    }

    /// Der ganze Zuschauerplatz auf Loopback: stumme Verbindungen halten die
    /// Annahme nicht auf, ein neuer Zuschauer kappt den Eingabekanal des
    /// alten und bekommt selbst einen funktionierenden, und je Absender
    /// laufen hoechstens zwei Handschlaege.
    #[test]
    fn zuschauerwechsel_und_parallele_annahme() {
        let (host_priv, _) = noise::keypair().unwrap();
        PRIV.set(host_priv).unwrap();
        secure::test_identitaet();
        let bild_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ein_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bild_addr = bild_l.local_addr().unwrap().to_string();
        let ein_addr = ein_l.local_addr().unwrap().to_string();
        std::thread::spawn(move || annahme_bild(bild_l));
        std::thread::spawn(move || annahme_eingabe(ein_l));

        // Zuschauer A, waehrend eine stumme Verbindung am Bildport haengt.
        let stumm = TcpStream::connect(&bild_addr).unwrap();
        let mut a = bild_verbinden(&bild_addr);
        drop(stumm);
        // Eingabekanal von A, ebenso an einer stummen Verbindung vorbei.
        let stumm = TcpStream::connect(&ein_addr).unwrap();
        let t0 = Instant::now();
        let mut a_ein = secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        drop(stumm);
        zeitfrage(&mut a_ein, 0xA1).unwrap();
        assert_eq!(zeitantwort(&mut a), Ok(0xA1));

        // Zuschauer B loest A ab: A verliert Bild UND Eingabe.
        let mut b = bild_verbinden(&bild_addr);
        a_ein.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        a.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t0 = Instant::now();
        assert!(a_ein.read_exact(&mut [0u8; 1]).is_err());
        assert!(zeitantwort(&mut a).is_err());
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        let _ = zeitfrage(&mut a_ein, 0xA2);
        // B bekommt einen eigenen, funktionierenden Eingabekanal; die
        // Frage von A kommt nie an.
        let mut b_ein = secure::Secure::connect(&ein_addr, &noise::prologue_input(&b.handshake_hash)).unwrap();
        zeitfrage(&mut b_ein, 0xB1).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB1));
        // Die alte Pruefsumme passt nicht mehr: A kommt nicht wieder herein.
        assert!(secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).is_err());

        // Zwei laufende Handschlaege von einem Absender: der dritte wird
        // sofort abgewiesen, nicht erst nach der Frist.
        let s1 = TcpStream::connect(&bild_addr).unwrap();
        let s2 = TcpStream::connect(&bild_addr).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        assert!(secure::Secure::connect(&bild_addr, &noise::prologue_video()).is_err());
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        // Geben sie auf, ist der Platz wieder frei.
        drop((s1, s2));
        std::thread::sleep(Duration::from_millis(200));
        let _c = bild_verbinden(&bild_addr);
    }
}
