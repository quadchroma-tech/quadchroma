// Zugang: wer herein darf und womit er es beweist (Spezifikation Pairing v1,
// Abschnitte 1-4).
//
// Der gemeinsame Kern fuer den Client (Paket P4) und die Windows-Host-Rolle
// (Paket P5). Der Mac-Host rechnet dasselbe in C (host/zugang.c, Paket P2),
// mit denselben Pruefvektoren (Tests unten). Hier oeffnet nichts eine
// Leitung, startet einen Faden oder schreibt ins Protokoll: alles ist
// entweder reine Rechnung (ohne Netz testbar) oder liest und schreibt genau
// eine Datei, deren Pfad der Aufrufer uebergibt (`ablage_pfad` nennt den
// ueblichen). Protokollzeilen schreibt der Aufrufer; die Fehlertypen haben
// dafuer einen deutschen Text (Display).
//
// Oeffentliche Schnittstelle
// ==========================
//
// 1.2 Geraete-ID (Anzeige- und Suchschluessel; vertraut wird immer dem
//     vollen Schluessel)
//   geraete_id(pub32) -> u32                  SHA-256("QuadChroma/1 ID\0" || pub) mod 10^9
//   id_text(id) -> "581 729 911"              Anzeige mit fuehrenden Nullen ({i} der Texte)
//   id_ziffern(id) -> "581729911"             zum Kopieren
//   id_lesen(text) -> Option<u32>             genau 9 Ziffern, Leerzeichen/Bindestriche erlaubt
//
// 1.3 Geraetename
//   NAME_MAX = 40
//   name_kuerzen(text, max) -> &str           UTF-8-sicher auf max Byte
//   name_bereinigen(text) -> String           Steuer-/Richtungszeichen -> '?', getrimmt, <= 40 Byte
//   geraetename() -> String                   Name dieses Geraets fuer Nachricht 3, Bekanntgabe
//                                             und Nachricht 20: der eingestellte, sonst der
//                                             Rechnername - beides schon bereinigt
//   rechnername() -> String                   Rechnername des Systems (Windows: GetComputerNameExW,
//                                             Mac: SCDynamicStoreCopyComputerName, Rueckfall gethostname)
//   geraetename_pruefen(eingabe) -> Result<Option<String>, NameFehler>
//                                             Eingabe im Fenster "Geraetename": getrimmt, leer =
//                                             Rechnername (None), sonst 1-40 Byte ohne Steuerzeichen
//   geraetename_setzen(Option<String>)        gilt sofort (einstellungen.txt: geraetename=)
//
// 1.4 Handschlag-Nachricht 3 (Client -> Host)
//   nachricht3(name, flags) -> Vec<u8>        "QCN1" | u8 n | Name | u8 Flags - statt b"client"
//   nachricht3_name(nutzlast) -> Option<String>
//                                             None: alter Client oder kaputt -> IP anzeigen
//   nachricht3_flags(nutzlast) -> u8          NAME_FLAG_*; ohne Flag-Byte (aeltere Fassung) 0
//
// 2 Bekanntgabe (UDP 9003)
//   bekanntgabe(port, name, id, flags) -> Vec<u8>          mit Erweiterung (ext 1, ID, Flags)
//   bekanntgabe_lesen(paket) -> Option<Bekanntgabe>        id None: Host ohne Erweiterung ("-")
//
// 3.2 Zugangsnachrichten 20-23 (Bildkanal, nach MAGIC_ZUGANG "QCA1")
//   Nachricht::{Noetig(ZugangNoetig), Beweis([u8; 32]), Ergebnis(Ergebnis), Abbruch}
//   Ergebnis::{Passwort{host_beweis}, Zulassen, Falsch{warten_ms}, Abgelehnt, Schluss{warten_ms}}
//   Nachricht::kodieren() -> Vec<u8>          Kopf + Nutzlast, fertig fuer write_all
//   empfangen(|puffer| sock.read_exact(puffer)) -> Result<Nachricht, LeseFehler>
//                                             Kopf lesen, Typ/Laenge pruefen (vor dem
//                                             Anlegen des Puffers), Nutzlast lesen und pruefen
//   kopf_lesen(&[u8; 8]), nachricht_lesen(typ, nutzlast)   dieselben Schritte einzeln
//   Welche Richtung welche Nachricht senden darf, prueft der Aufrufer (ein
//   Host nimmt nur Beweis/Abbruch an, ein Client nur Noetig/Ergebnis).
//
// 3.3 Drossel und Grenzen (im Speicher, die Zeit kommt als Parameter)
//   Drossel::neu()
//     phase_beginnen(ip, schluessel, jetzt) -> Phase       warten_ms fuer Nachricht 20
//     beweis_werten(&mut phase, ip, schluessel, richtig, jetzt) -> Wertung
//                                             zu frueh = Fehlversuch; 5 Fehlversuche -> Schluss
//     warten / fehlversuch / erfolg / global_gesperrt / fehlversuche   die Bausteine dazu
//   Phase::warten_ms(jetzt), zu_frueh(jetzt), frist(), abgelaufen(jetzt)   120-s-Gesamtfrist
//   Wertung::ergebnis(host_beweis) -> Ergebnis                    fuer Nachricht 22
//   Plaetze::belegen / freigeben              hoechstens 4 Phasen, 1 je Schluessel, 2 je IP;
//                                             kein Platz: Ergebnis 4 mit BESETZT_WARTEN_MS
//   wartezeit(fehlversuche) -> Duration       0, 0, 5, 10, 20 ... hoechstens 300 s
//   millis(dauer) -> u32                      aufgerundet - wer so lange wartet, ist nie zu frueh
//
// 3.4 Kryptografie (HMAC nach RFC 2104 und PBKDF2 nach RFC 8018 ueber sha2)
//   passwort_norm(pw) -> Vec<u8>
//   passwort_schluessel(pw, host_pub) -> [u8; 32]           K; PBKDF2 mit 100 000 Runden - dauert
//   Schluesselcache::schluessel(pw, host_pub)               K nur neu, wenn sich etwas aendert (Host)
//   client_beweis(&K, hh), host_beweis(&K, hh) -> [u8; 32]  hh = handshake_hash des Bildkanals
//   beweis_pruefen(&K, hh, &erhalten), host_beweis_pruefen(&K, hh, &erhalten) -> bool
//   gleich(a, b) -> bool                      Vergleich in konstanter Zeit
//   hmac_sha256(schluessel, daten), pbkdf2_sha256(pw, salz, runden, ausgabe)
//
// 4.1/4.2 host-devices.txt (Host-Rolle)
//   Geraeteliste::laden(pfad)                 fehlt: leer; unlesbar/beschaedigt: Err - dann ist
//                                             niemand bekannt, und die Datei bleibt, wie sie ist
//   Geraeteliste::enthaelt / finden
//   geraet_eintragen(pfad, pub, name, datum) -> bool (true = neu)
//   geraet_entfernen(pfad, pub) -> bool, alle_geraete_entfernen(pfad)
//   geraeteliste_zuruecksetzen(pfad, stempel) -> Option<PathBuf>   alte Datei als .defekt-<stempel>
//   geraete_migrieren(ordner, datum) -> Migration          authorized.txt -> host-devices.txt
//
// 4.3 host-password.txt (Host-Rolle)
//   passwort_laden(pfad) -> Passwort          fehlt: Zufallspasswort anlegen (neu = true)
//   passwort_setzen(pfad, pw) -> String       prueft norm(pw) >= 8 Byte, schreibt atomar
//   passwort_zufall_setzen(pfad) -> String    "Neues Zufallspasswort"
//   passwort_pruefen(pw)                      nur die Pruefung (fuer das Passwortfenster)
//   zufallspasswort() -> "k7m-4wq-9tz", zufall(puffer)     CSPRNG des Systems
//
// 4.4 hosts.txt (Client)
//   Hostliste::laden(pfad), speichern(pfad)
//   Hostliste::nach_schluessel / nach_id / nach_adresse    neuester Eintrag zuerst
//   Hostliste::merken(BekannterHost::neu(pub, adresse, name)), vergessen(pub)
//   host_merken(pfad, host), host_vergessen(pfad, pub)   Lesen-Aendern-Schreiben in einem Zug
//   hosts_migrieren(ordner) -> Migration      known_hosts.txt -> hosts.txt
//
// Dazu
//   ablage_pfad(DATEINAME)                    Pfad im Ablageordner (secure::config_dir)
//   jetzt_lokal() -> Zeitpunkt, heute() -> "JJJJ-MM-TT", Zeitpunkt::stempel()
//   DateiFehler                               Unlesbar / KeinUtf8 / Beschaedigt / Schreiben;
//                                             From<DateiFehler> fuer secure::Fehler (Client-Meldungen)
//   zwischendateien_aufraeumen(pfad)          Reste *.neu eines abgebrochenen Laufs
//   hex(bytes), hex_lesen(text) -> Option<[u8; 32]>
//
// Nebenlaeufigkeit: Die Dateifunktionen schreiben atomar (Zwischendatei mit
// sync, dann umbenennen; unter Unix 0600) und serialisieren sich innerhalb
// des Prozesses je Dateiart selbst. Drossel und Plaetze sind reine Zustaende
// ohne eigene Sperre: der Aufrufer haelt sie hinter einem Mutex, nur kurz
// (keine Leitung und kein PBKDF2 unter der Sperre - K kommt aus dem
// Schluesselcache, der Beweis selbst ist ein HMAC).

// Die Schnittstelle ist fuer P4 (Client) und P5 (Host-Rolle) angelegt; bis
// beide sie benutzen, waere fast alles hier "ungenutzt".
#![allow(dead_code)]

use crate::protokoll_konst::{
    BEACON_EXT, BEACON_FLAG_ZULASSEN, BEACON_MAGIC, BEACON_VERSION, ERGEBNIS_ABGELEHNT, ERGEBNIS_FALSCH,
    ERGEBNIS_PASSWORT, ERGEBNIS_SCHLUSS, ERGEBNIS_ZULASSEN, NAME_FLAG_HOST_UNBEKANNT, NAME_KENNUNG, WEG_PASSWORT,
    WEG_ZULASSEN, ZUGANG_ABBRUCH, ZUGANG_BEWEIS, ZUGANG_ERGEBNIS, ZUGANG_FASSUNG, ZUGANG_NOETIG,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ------------------------------------------------------------ 1.2 Geraete-ID

/// Die ID hat 9 Dezimalstellen.
pub const ID_GRENZE: u32 = 1_000_000_000;

/// Geraete-ID aus dem oeffentlichen Schluessel (32 Byte):
/// SHA-256("QuadChroma/1 ID\0" || pub), die ersten 4 Byte big endian,
/// modulo 10^9. Nur Anzeige und Suche - vertraut wird dem Schluessel.
pub fn geraete_id(pub32: &[u8]) -> u32 {
    let mut h = Sha256::new();
    h.update(b"QuadChroma/1 ID\0");
    h.update(pub32);
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % ID_GRENZE
}

/// Anzeigeform "ddd ddd ddd" mit fuehrenden Nullen (5 -> "000 000 005").
pub fn id_text(id: u32) -> String {
    let id = id % ID_GRENZE;
    format!("{:03} {:03} {:03}", id / 1_000_000, id / 1_000 % 1_000, id % 1_000)
}

/// Die 9 Ziffern ohne Trenner - das, was "Kopieren" in die Ablage legt.
pub fn id_ziffern(id: u32) -> String {
    format!("{:09}", id % ID_GRENZE)
}

/// Liest eine eingegebene ID: genau 9 Ziffern, dazwischen (und aussen
/// herum) beliebig Leerraum und Bindestriche, sonst nichts. "581729911",
/// "581 729 911" und "581-729-911" ergeben dieselbe ID; alles andere
/// (8 oder 10 Ziffern, Punkte, Buchstaben) ist keine ID, sondern eine
/// Adresse oder ein Name.
pub fn id_lesen(text: &str) -> Option<u32> {
    let mut wert: u32 = 0;
    let mut ziffern = 0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            ziffern += 1;
            if ziffern > 9 {
                return None;
            }
            wert = wert * 10 + (c as u32 - '0' as u32);
        } else if !(c == '-' || c.is_whitespace()) {
            return None;
        }
    }
    (ziffern == 9).then_some(wert)
}

// ------------------------------------------------------------ 1.3 Geraetename

/// Hoechstens so viele Byte (UTF-8) hat ein Geraetename - in Nachricht 3,
/// in der Bekanntgabe, in Nachricht 20 und in den Listen.
pub const NAME_MAX: usize = 40;

/// Kuerzt auf hoechstens `max` Byte, nie mitten in einer UTF-8-Folge.
pub fn name_kuerzen(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut i = max;
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    &text[..i]
}

/// Zeichen, die in einem angezeigten Namen nichts verloren haben:
/// Steuerzeichen (Zeilenwechsel braechen die Listen) und die
/// Richtungszeichen, mit denen sich ein Name im Zulassen-Fenster anders
/// lesen liesse, als er ist.
fn gefaehrlich(c: char) -> bool {
    c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Ein Name, wie er angezeigt und gespeichert wird: gefaehrliche Zeichen
/// werden zu '?', Leerraum aussen faellt weg, dann auf NAME_MAX Byte
/// gekuerzt. Der Name ist unbeglaubigt - nur Anzeige.
pub fn name_bereinigen(text: &str) -> String {
    let t: String = text.chars().map(|c| if gefaehrlich(c) { '?' } else { c }).collect();
    name_kuerzen(t.trim(), NAME_MAX).trim_end().to_string()
}

/// Der eingestellte Geraetename (einstellungen.txt, geraetename=), schon
/// geprueft; None: es gilt der Rechnername des Systems. Ein Wert fuer den
/// ganzen Prozess - Client und Host-Rolle nennen sich gleich.
static EINGESTELLT: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Warum ein eingegebener Geraetename nicht gilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFehler {
    /// Mehr als NAME_MAX Byte UTF-8.
    ZuLang,
    /// Steuer- oder Richtungszeichen.
    Zeichen,
}

/// Eine Eingabe im Fenster "Geraetename" (und ein Wert aus
/// einstellungen.txt): aussen ohne Leerraum; leer heisst zurueck zum
/// Rechnernamen (Ok(None)); sonst 1 bis NAME_MAX Byte UTF-8 ohne Steuer-
/// und Richtungszeichen - dieselben, die name_bereinigen ersetzen wuerde.
pub fn geraetename_pruefen(eingabe: &str) -> Result<Option<String>, NameFehler> {
    let t = eingabe.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().any(gefaehrlich) {
        return Err(NameFehler::Zeichen);
    }
    if t.len() > NAME_MAX {
        return Err(NameFehler::ZuLang);
    }
    Ok(Some(t.to_string()))
}

/// Den Geraetenamen einstellen (None: Rechnername). Gilt sofort: die
/// Bekanntgabe liest ihn je Runde, Nachricht 3 je Verbindung. Ein Wert, der
/// die Pruefung nicht besteht, zaehlt wie None.
pub fn geraetename_setzen(name: Option<String>) {
    let name = name.and_then(|n| geraetename_pruefen(&n).ok().flatten());
    *EINGESTELLT.write().unwrap_or_else(|e| e.into_inner()) = name;
}

/// Der eingestellte Geraetename (None: es gilt der Rechnername).
pub fn geraetename_eingestellt() -> Option<String> {
    EINGESTELLT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Der Name dieses Geraets fuer Nachricht 3, Bekanntgabe und Nachricht 20:
/// der eingestellte (geraetename_setzen), sonst der Rechnername des
/// Systems. Nie leer, hoechstens 40 Byte.
pub fn geraetename() -> String {
    geraetename_eingestellt().unwrap_or_else(rechnername)
}

/// Der Rechnername des Systems, bereinigt und auf 40 Byte gekuerzt. Nie
/// leer.
pub fn rechnername() -> String {
    let roh = rechnername_system()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let n = name_bereinigen(&roh);
    if !n.is_empty() {
        return n;
    }
    if cfg!(windows) {
        "Windows".into()
    } else if cfg!(target_os = "macos") {
        "Mac".into()
    } else {
        "QuadChroma".into()
    }
}

/// Windows: der DNS-Rechnername in der Schreibweise, in der er vergeben
/// wurde (COMPUTERNAME waere der NetBIOS-Name in Grossbuchstaben).
#[cfg(windows)]
fn rechnername_system() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::SystemInformation::{ComputerNamePhysicalDnsHostname, GetComputerNameExW};
    let mut n: u32 = 0;
    // Erst die Laenge samt Nullzeichen erfragen; der Aufruf scheitert dabei
    // absichtlich (ERROR_MORE_DATA).
    let _ = unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, None, &mut n) };
    if n == 0 || n > 4096 {
        return None;
    }
    let mut puffer = vec![0u16; n as usize];
    unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, Some(PWSTR(puffer.as_mut_ptr())), &mut n) }.ok()?;
    let s = String::from_utf16_lossy(&puffer[..(n as usize).min(puffer.len())]);
    (!s.trim().is_empty()).then_some(s)
}

/// macOS: der Computername aus den Systemeinstellungen ("Roberts Mac mini"),
/// wie beim Mac-Host; Rueckfall gethostname().
#[cfg(target_os = "macos")]
fn rechnername_system() -> Option<String> {
    computername_mac().or_else(hostname_unix)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn rechnername_system() -> Option<String> {
    hostname_unix()
}

#[cfg(not(any(windows, unix)))]
fn rechnername_system() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn computername_mac() -> Option<String> {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "SystemConfiguration", kind = "framework")]
    extern "C" {
        fn SCDynamicStoreCopyComputerName(store: *const c_void, kodierung: *mut u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetCString(s: *const c_void, puffer: *mut c_char, groesse: isize, kodierung: u32) -> u8;
        fn CFRelease(p: *const c_void);
    }
    // kCFStringEncodingUTF8
    const UTF8: u32 = 0x0800_0100;
    unsafe {
        let s = SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut());
        if s.is_null() {
            return None;
        }
        let mut puffer = [0 as c_char; 1024];
        let ok = CFStringGetCString(s, puffer.as_mut_ptr(), puffer.len() as isize, UTF8);
        CFRelease(s);
        if ok == 0 {
            return None;
        }
        let t = CStr::from_ptr(puffer.as_ptr()).to_string_lossy().into_owned();
        (!t.trim().is_empty()).then_some(t)
    }
}

#[cfg(unix)]
fn hostname_unix() -> Option<String> {
    use std::ffi::{c_char, c_int, CStr};
    extern "C" {
        fn gethostname(name: *mut c_char, laenge: usize) -> c_int;
    }
    let mut puffer = [0 as c_char; 256];
    if unsafe { gethostname(puffer.as_mut_ptr(), puffer.len() - 1) } != 0 {
        return None;
    }
    let t = unsafe { CStr::from_ptr(puffer.as_ptr()) }.to_string_lossy().into_owned();
    (!t.trim().is_empty()).then_some(t)
}

// ------------------------------------------------- 1.4 Handschlag-Nachricht 3

/// Nutzlast von Nachricht 3: "QCN1" | u8 n | n Byte UTF-8-Name (n <= 40) |
/// u8 Flags. Der Name wird vorher bereinigt; aeltere Hosts uebergehen die
/// Nutzlast bzw. alles hinter dem Namen. Flags: NAME_FLAG_HOST_UNBEKANNT,
/// wenn dieser Client den Schluessel des Hosts nicht kennt (Bit 1-7 bleiben 0).
pub fn nachricht3(name: &str, flags: u8) -> Vec<u8> {
    let n = name_bereinigen(name);
    let mut v = Vec::with_capacity(6 + n.len());
    v.extend_from_slice(NAME_KENNUNG);
    v.push(n.len() as u8);
    v.extend_from_slice(n.as_bytes());
    v.push(flags & NAME_FLAG_HOST_UNBEKANNT);
    v
}

/// Der Name aus Nachricht 3, bereinigt. None, wenn die Kennung fehlt
/// (aelterer Client: b"client") oder die Nutzlast kaputt ist (Laenge 0 oder
/// ueber 40, abgeschnitten, kein UTF-8) - dann zeigt der Host die IP der
/// Gegenstelle. Bytes hinter dem Namen werden uebergangen (Flags, spaetere
/// Felder).
pub fn nachricht3_name(nutzlast: &[u8]) -> Option<String> {
    if nutzlast.len() < 5 || &nutzlast[..4] != NAME_KENNUNG {
        return None;
    }
    let n = nutzlast[4] as usize;
    if n == 0 || n > NAME_MAX || nutzlast.len() < 5 + n {
        return None;
    }
    let name = name_bereinigen(std::str::from_utf8(&nutzlast[5..5 + n]).ok()?);
    (!name.is_empty()).then_some(name)
}

/// Die Flags aus Nachricht 3 (NAME_FLAG_*): das Byte direkt hinter dem
/// Namen. 0, wenn es fehlt (aeltere Fassung: "QCN1" ohne Flags, b"client")
/// oder der Aufbau nicht stimmt (Kennung, Laenge ueber 40, abgeschnitten) -
/// ob der Name selbst taugt (UTF-8, nicht leer), spielt keine Rolle. Bytes
/// dahinter und unbekannte Bits werden uebergangen.
pub fn nachricht3_flags(nutzlast: &[u8]) -> u8 {
    if nutzlast.len() < 5 || &nutzlast[..4] != NAME_KENNUNG {
        return 0;
    }
    let n = nutzlast[4] as usize;
    match nutzlast.get(5 + n) {
        Some(f) if n <= NAME_MAX => f & NAME_FLAG_HOST_UNBEKANNT,
        _ => 0,
    }
}

// ------------------------------------------------------------ 2 Bekanntgabe

/// Hoechstens so gross darf ein Bekanntgabe-Paket sein: der Client liest in
/// einen Puffer von 128 Byte (unter Windows ginge ein groesseres ganz
/// verloren).
pub const BEKANNTGABE_MAX: usize = 128;

/// Was ein Host im Netz von sich sagt. Alles unbeglaubigt - nur Anzeige und
/// Suche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bekanntgabe {
    pub port: u16,
    pub name: String,
    /// None: Host ohne Erweiterung (aeltere Fassung) - der Client zeigt "-".
    pub id: Option<u32>,
    /// BEACON_FLAG_*; ohne Erweiterung 0.
    pub flags: u8,
}

impl Bekanntgabe {
    /// Kann am Host jemand "Zulassen" klicken?
    pub fn zulassen_moeglich(&self) -> bool {
        self.id.is_some() && self.flags & BEACON_FLAG_ZULASSEN != 0
    }
}

/// Paket der Bekanntgabe: "QCHB" | u8 1 | u16 Bildport LE | u8 nlen | Name
/// (bereinigt, <= 40 Byte) | u8 ext = 1 | u32 ID LE | u8 Flags. Bit 1-7 der
/// Flags bleiben 0. Hoechstens 54 Byte.
pub fn bekanntgabe(port: u16, name: &str, id: u32, flags: u8) -> Vec<u8> {
    let name = name_bereinigen(name);
    let mut v = Vec::with_capacity(8 + name.len() + 6);
    v.extend_from_slice(BEACON_MAGIC);
    v.push(BEACON_VERSION);
    v.extend_from_slice(&port.to_le_bytes());
    v.push(name.len() as u8);
    v.extend_from_slice(name.as_bytes());
    v.push(BEACON_EXT);
    v.extend_from_slice(&(id % ID_GRENZE).to_le_bytes());
    v.push(flags & BEACON_FLAG_ZULASSEN);
    v
}

/// Liest ein Bekanntgabe-Paket. None: keine Bekanntgabe (zu kurz, falsche
/// Kennung oder Fassung, Name ueber das Paketende). Eine fehlende, kaputte
/// oder unbekannte Erweiterung ergibt id None - das Paket gilt trotzdem.
pub fn bekanntgabe_lesen(paket: &[u8]) -> Option<Bekanntgabe> {
    if paket.len() < 8 || &paket[0..4] != BEACON_MAGIC || paket[4] != BEACON_VERSION {
        return None;
    }
    let port = u16::from_le_bytes([paket[5], paket[6]]);
    let nlen = paket[7] as usize;
    if paket.len() < 8 + nlen {
        return None;
    }
    let name = name_bereinigen(&String::from_utf8_lossy(&paket[8..8 + nlen]));
    let ext = &paket[8 + nlen..];
    let (id, flags) = match ext {
        [e, a, b, c, d, f, ..] if *e == BEACON_EXT => {
            let id = u32::from_le_bytes([*a, *b, *c, *d]);
            if id < ID_GRENZE {
                (Some(id), *f)
            } else {
                (None, 0)
            }
        }
        _ => (None, 0),
    };
    Some(Bekanntgabe { port, name, id, flags })
}

// --------------------------------------------- 3.2 Zugangsnachrichten 20-23

/// Obergrenze fuer die Laenge von Nachricht 20 beim Lesen des Kopfs. Die
/// Nutzlast wird bis dahin gelesen, damit eine spaetere Fassung als
/// "falsche Fassung" erkannt wird und nicht nur als "falsche Laenge"; gelten
/// laesst `nachricht_lesen` dann nur 8 + hoechstens 40 Byte.
const NOETIG_LESEGRENZE: usize = 1024;

/// Nachricht 20: der Host verlangt einen Zugangsbeweis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZugangNoetig {
    /// WEG_PASSWORT (immer) | WEG_ZULASSEN (eine Oberflaeche laeuft).
    pub wege: u8,
    /// Drossel: erst danach einen Beweis senden (0 = sofort).
    pub warten_ms: u32,
    /// Name des Hosts, bereinigt (<= 40 Byte).
    pub hostname: String,
}

impl ZugangNoetig {
    pub fn neu(zulassen_moeglich: bool, warten_ms: u32, hostname: &str) -> ZugangNoetig {
        let wege = WEG_PASSWORT | if zulassen_moeglich { WEG_ZULASSEN } else { 0 };
        ZugangNoetig { wege, warten_ms, hostname: name_bereinigen(hostname) }
    }

    pub fn passwort_moeglich(&self) -> bool {
        self.wege & WEG_PASSWORT != 0
    }

    pub fn zulassen_moeglich(&self) -> bool {
        self.wege & WEG_ZULASSEN != 0
    }
}

/// Nachricht 22: wie der Host entschieden hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ergebnis {
    /// 0: angenommen per Passwort; der Client prueft `host_beweis`, bevor er
    /// den Host pinnt.
    Passwort { host_beweis: [u8; 32] },
    /// 1: angenommen per Klick auf "Zulassen" (Erstkontakt ohne Beweis).
    Zulassen,
    /// 2: Passwort falsch; ein neuer Beweis fruehestens nach `warten_ms`.
    Falsch { warten_ms: u32 },
    /// 3: am Host abgelehnt - kein automatischer Neuversuch.
    Abgelehnt,
    /// 4: zu viele Versuche oder Frist abgelaufen; der Host schliesst.
    Schluss { warten_ms: u32 },
}

impl Ergebnis {
    /// Der Code im ersten Byte (ERGEBNIS_*).
    pub fn code(&self) -> u8 {
        match self {
            Ergebnis::Passwort { .. } => ERGEBNIS_PASSWORT,
            Ergebnis::Zulassen => ERGEBNIS_ZULASSEN,
            Ergebnis::Falsch { .. } => ERGEBNIS_FALSCH,
            Ergebnis::Abgelehnt => ERGEBNIS_ABGELEHNT,
            Ergebnis::Schluss { .. } => ERGEBNIS_SCHLUSS,
        }
    }

    /// Wartezeit (nur bei Falsch und Schluss, sonst 0).
    pub fn warten_ms(&self) -> u32 {
        match self {
            Ergebnis::Falsch { warten_ms } | Ergebnis::Schluss { warten_ms } => *warten_ms,
            _ => 0,
        }
    }
}

/// Eine Nachricht der Zugangsphase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nachricht {
    /// 20, Host -> Client.
    Noetig(ZugangNoetig),
    /// 21, Client -> Host: client_proof.
    Beweis([u8; 32]),
    /// 22, Host -> Client.
    Ergebnis(Ergebnis),
    /// 23, Client -> Host: der Nutzer hat abgebrochen.
    Abbruch,
}

/// Kopf: u8 Typ, u8 Flags = 0, u16 0, u32 Laenge LE.
fn kopf(typ: u8, laenge: usize) -> [u8; 8] {
    let l = (laenge as u32).to_le_bytes();
    [typ, 0, 0, 0, l[0], l[1], l[2], l[3]]
}

impl Nachricht {
    pub fn typ(&self) -> u8 {
        match self {
            Nachricht::Noetig(_) => ZUGANG_NOETIG,
            Nachricht::Beweis(_) => ZUGANG_BEWEIS,
            Nachricht::Ergebnis(_) => ZUGANG_ERGEBNIS,
            Nachricht::Abbruch => ZUGANG_ABBRUCH,
        }
    }

    /// Kopf und Nutzlast, fertig zum Senden.
    pub fn kodieren(&self) -> Vec<u8> {
        let mut n = Vec::with_capacity(40);
        match self {
            Nachricht::Noetig(z) => {
                let name = name_bereinigen(&z.hostname);
                n.push(ZUGANG_FASSUNG);
                n.push(z.wege);
                n.extend_from_slice(&[0, 0]);
                n.extend_from_slice(&z.warten_ms.to_le_bytes());
                n.extend_from_slice(name.as_bytes());
            }
            Nachricht::Beweis(b) => n.extend_from_slice(b),
            Nachricht::Ergebnis(e) => {
                n.push(e.code());
                n.extend_from_slice(&[0, 0, 0]);
                n.extend_from_slice(&e.warten_ms().to_le_bytes());
                if let Ergebnis::Passwort { host_beweis } = e {
                    n.extend_from_slice(host_beweis);
                }
            }
            Nachricht::Abbruch => {}
        }
        let mut v = kopf(self.typ(), n.len()).to_vec();
        v.extend_from_slice(&n);
        v
    }
}

/// Was beim Lesen einer Zugangsnachricht nicht passt. Jeder Fall ist ein
/// Protokollfehler: die Zugangsphase endet, die Leitung faellt zu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeseFehler {
    /// Kein Typ der Zugangsphase (20-23).
    Typ(u8),
    /// Laenge passt nicht zum Typ (oder zum Ergebnis).
    Laenge { typ: u8, laenge: usize },
    /// Nachricht 20 in einer unbekannten Fassung.
    Fassung(u8),
    /// Nachricht 22 mit unbekanntem Ergebnis.
    Ergebniscode(u8),
    /// Lesen an der Leitung scheiterte (Wortlaut des Aufrufers).
    Leitung(String),
}

impl std::fmt::Display for LeseFehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeseFehler::Typ(t) => write!(f, "Zugangsphase: unerwarteter Nachrichtentyp {t}"),
            LeseFehler::Laenge { typ, laenge } => {
                write!(f, "Zugangsphase: Nachricht {typ} mit unpassender Laenge {laenge}")
            }
            LeseFehler::Fassung(v) => write!(f, "Zugangsphase: unbekannte Fassung {v} (erwartet {ZUGANG_FASSUNG})"),
            LeseFehler::Ergebniscode(c) => write!(f, "Zugangsphase: unbekanntes Ergebnis {c}"),
            LeseFehler::Leitung(g) => write!(f, "Zugangsphase: {g}"),
        }
    }
}

/// Prueft einen Kopf: Typ der Zugangsphase und eine Laenge, die zu ihm
/// passt - BEVOR der Aufrufer einen Puffer dieser Laenge anlegt. Liefert
/// Typ und Laenge der Nutzlast. Flags und das freie Feld werden
/// uebergangen (gesendet werden sie als 0).
pub fn kopf_lesen(k: &[u8; 8]) -> Result<(u8, usize), LeseFehler> {
    let typ = k[0];
    let laenge = u32::from_le_bytes([k[4], k[5], k[6], k[7]]) as usize;
    let passt = match typ {
        ZUGANG_NOETIG => (8..=NOETIG_LESEGRENZE).contains(&laenge),
        ZUGANG_BEWEIS => laenge == 32,
        ZUGANG_ERGEBNIS => laenge == 8 || laenge == 40,
        ZUGANG_ABBRUCH => laenge == 0,
        _ => return Err(LeseFehler::Typ(typ)),
    };
    if passt {
        Ok((typ, laenge))
    } else {
        Err(LeseFehler::Laenge { typ, laenge })
    }
}

/// Liest die Nutzlast einer Zugangsnachricht mit allen Pruefungen: Laenge
/// je Typ (20: 8 + hoechstens 40, 21: 32, 22: 40 bei Ergebnis 0 und sonst 8,
/// 23: 0), Fassung von 20, Ergebniscode von 22. Die reservierten Bytes
/// werden uebergangen, der Hostname bereinigt.
pub fn nachricht_lesen(typ: u8, n: &[u8]) -> Result<Nachricht, LeseFehler> {
    let laenge = |ok: bool| if ok { Ok(()) } else { Err(LeseFehler::Laenge { typ, laenge: n.len() }) };
    let u32_bei = |i: usize| u32::from_le_bytes([n[i], n[i + 1], n[i + 2], n[i + 3]]);
    match typ {
        ZUGANG_NOETIG => {
            laenge(n.len() >= 8)?;
            if n[0] != ZUGANG_FASSUNG {
                return Err(LeseFehler::Fassung(n[0]));
            }
            laenge(n.len() <= 8 + NAME_MAX)?;
            Ok(Nachricht::Noetig(ZugangNoetig {
                wege: n[1],
                warten_ms: u32_bei(4),
                hostname: name_bereinigen(&String::from_utf8_lossy(&n[8..])),
            }))
        }
        ZUGANG_BEWEIS => {
            laenge(n.len() == 32)?;
            let mut b = [0u8; 32];
            b.copy_from_slice(n);
            Ok(Nachricht::Beweis(b))
        }
        ZUGANG_ERGEBNIS => {
            laenge(n.len() == 8 || n.len() == 40)?;
            let warten_ms = u32_bei(4);
            let e = match n[0] {
                ERGEBNIS_PASSWORT => {
                    laenge(n.len() == 40)?;
                    let mut host_beweis = [0u8; 32];
                    host_beweis.copy_from_slice(&n[8..40]);
                    Ergebnis::Passwort { host_beweis }
                }
                c @ (ERGEBNIS_ZULASSEN | ERGEBNIS_FALSCH | ERGEBNIS_ABGELEHNT | ERGEBNIS_SCHLUSS) => {
                    laenge(n.len() == 8)?;
                    match c {
                        ERGEBNIS_ZULASSEN => Ergebnis::Zulassen,
                        ERGEBNIS_FALSCH => Ergebnis::Falsch { warten_ms },
                        ERGEBNIS_ABGELEHNT => Ergebnis::Abgelehnt,
                        _ => Ergebnis::Schluss { warten_ms },
                    }
                }
                c => return Err(LeseFehler::Ergebniscode(c)),
            };
            Ok(Nachricht::Ergebnis(e))
        }
        ZUGANG_ABBRUCH => {
            laenge(n.is_empty())?;
            Ok(Nachricht::Abbruch)
        }
        t => Err(LeseFehler::Typ(t)),
    }
}

/// Liest eine ganze Zugangsnachricht ueber `lesen` (etwa
/// `|b| sock.read_exact(b)` einer gesicherten Leitung): Kopf, Pruefung,
/// Nutzlast, Pruefung. Fristen setzt der Aufrufer an der Leitung.
pub fn empfangen<E: std::fmt::Display>(
    mut lesen: impl FnMut(&mut [u8]) -> Result<(), E>,
) -> Result<Nachricht, LeseFehler> {
    let mut k = [0u8; 8];
    lesen(&mut k).map_err(|e| LeseFehler::Leitung(e.to_string()))?;
    let (typ, laenge) = kopf_lesen(&k)?;
    let mut n = vec![0u8; laenge];
    if laenge > 0 {
        lesen(&mut n).map_err(|e| LeseFehler::Leitung(e.to_string()))?;
    }
    nachricht_lesen(typ, &n)
}

// ------------------------------------------------------------ 3.3 Drossel

/// Fehlversuche ohne Wartezeit (je IP bzw. je Schluessel).
pub const DROSSEL_FREI: u32 = 2;
/// Wartezeit nach dem ersten gedrosselten Fehlversuch (dem dritten).
pub const DROSSEL_ERSTE: Duration = Duration::from_secs(5);
/// Obergrenze der Wartezeit.
pub const DROSSEL_HOECHSTENS: Duration = Duration::from_secs(300);
/// Ein Zaehler verfaellt so lange nach seinem letzten Fehlversuch.
pub const DROSSEL_VERFALL: Duration = Duration::from_secs(15 * 60);
/// Globale Regel: mehr als GLOBAL_GRENZE Fehlversuche in GLOBAL_FENSTER ->
/// jede weitere Zugangsphase wartet mindestens GLOBAL_WARTEN.
pub const GLOBAL_GRENZE: usize = 30;
pub const GLOBAL_FENSTER: Duration = Duration::from_secs(60);
pub const GLOBAL_WARTEN: Duration = Duration::from_secs(60);
/// Nach so vielen Fehlversuchen in EINER Verbindung: Ergebnis 4, schliessen.
pub const PHASE_FEHLVERSUCHE: u32 = 5;
/// Gesamtfrist einer Zugangsphase (danach Ergebnis 4).
pub const PHASE_FRIST: Duration = Duration::from_secs(120);
/// Wartezeit fuer Unbekannte, die keinen Platz mehr bekommen (Ergebnis 4).
pub const BESETZT_WARTEN_MS: u32 = 5000;
/// Hoechstens so viele Zaehler je Tabelle - wer mit vielen Adressen oder
/// Schluesseln kommt, verdraengt die aeltesten, fuellt aber nie den Speicher.
const DROSSEL_EINTRAEGE: usize = 4096;

/// Die Staffel: Fehlversuch 1-2: 0 s; ab dem 3.: 5 s, dann verdoppelt,
/// hoechstens 300 s. `fehlversuche` ist die Zahl einschliesslich des letzten.
pub fn wartezeit(fehlversuche: u32) -> Duration {
    if fehlversuche <= DROSSEL_FREI {
        return Duration::ZERO;
    }
    let stufe = (fehlversuche - DROSSEL_FREI - 1).min(16);
    (DROSSEL_ERSTE * (1u32 << stufe)).min(DROSSEL_HOECHSTENS)
}

/// Millisekunden fuer die Leitung, aufgerundet: ein Client, der genau so
/// lange wartet, kommt nie zu frueh.
pub fn millis(d: Duration) -> u32 {
    d.as_nanos().div_ceil(1_000_000).min(u32::MAX as u128) as u32
}

#[derive(Clone, Copy, Debug)]
struct Zaehler {
    fehlversuche: u32,
    letzter: Instant,
}

impl Zaehler {
    fn verfallen(&self, jetzt: Instant) -> bool {
        jetzt.saturating_duration_since(self.letzter) >= DROSSEL_VERFALL
    }

    /// Wie lange ab `jetzt` noch zu warten ist.
    fn rest(&self, jetzt: Instant) -> Duration {
        if self.verfallen(jetzt) {
            return Duration::ZERO;
        }
        (self.letzter + wartezeit(self.fehlversuche)).saturating_duration_since(jetzt)
    }
}

/// Einen Fehlversuch in einer Tabelle buchen; liefert die neue Zahl.
fn buchen<K: std::hash::Hash + Eq + Clone>(tabelle: &mut HashMap<K, Zaehler>, k: &K, jetzt: Instant) -> u32 {
    if !tabelle.contains_key(k) && tabelle.len() >= DROSSEL_EINTRAEGE {
        tabelle.retain(|_, z| !z.verfallen(jetzt));
        if tabelle.len() >= DROSSEL_EINTRAEGE {
            if let Some(alt) = tabelle.iter().min_by_key(|(_, z)| z.letzter).map(|(k, _)| k.clone()) {
                tabelle.remove(&alt);
            }
        }
    }
    let z = tabelle.entry(k.clone()).or_insert(Zaehler { fehlversuche: 0, letzter: jetzt });
    if z.verfallen(jetzt) {
        z.fehlversuche = 0;
    }
    z.fehlversuche = z.fehlversuche.saturating_add(1);
    z.letzter = jetzt;
    z.fehlversuche
}

/// Drossel der Zugangsbeweise (Spezifikation 3.3), im Speicher: je IP und je
/// Client-Schluessel getrennt gezaehlt, der groessere Wert gilt. Ein
/// richtiger Beweis setzt beide Zaehler zurueck; ein Zaehler verfaellt 15 min
/// nach seinem letzten Fehlversuch. Global: mehr als 30 Fehlversuche in 60 s
/// -> jede weitere Zugangsphase (und jeder weitere Fehlversuch) wartet
/// mindestens 60 s. Die Zeit kommt immer als Parameter - so ist alles ohne
/// Uhr pruefbar.
#[derive(Debug, Default)]
pub struct Drossel {
    je_ip: HashMap<IpAddr, Zaehler>,
    je_schluessel: HashMap<Vec<u8>, Zaehler>,
    /// Zeitpunkte der letzten Fehlversuche, hoechstens GLOBAL_GRENZE + 1.
    global: VecDeque<Instant>,
}

/// Eine laufende Zugangsphase (eine Verbindung).
#[derive(Clone, Copy, Debug)]
pub struct Phase {
    pub beginn: Instant,
    /// Ein Beweis vor diesem Zeitpunkt zaehlt als Fehlversuch.
    frei_ab: Instant,
    /// Fehlversuche in DIESER Verbindung.
    pub fehlversuche: u32,
}

impl Phase {
    /// Was als warten_ms hinausgeht (Nachricht 20 bzw. 22): die Restzeit.
    pub fn warten_ms(&self, jetzt: Instant) -> u32 {
        millis(self.frei_ab.saturating_duration_since(jetzt))
    }

    /// Kaeme ein Beweis jetzt zu frueh?
    pub fn zu_frueh(&self, jetzt: Instant) -> bool {
        jetzt < self.frei_ab
    }

    /// Ende der Gesamtfrist (120 s ab Beginn): danach Ergebnis 4.
    pub fn frist(&self) -> Instant {
        self.beginn + PHASE_FRIST
    }

    pub fn abgelaufen(&self, jetzt: Instant) -> bool {
        jetzt >= self.frist()
    }
}

/// Wie ein Beweis gewertet wurde - daraus wird Nachricht 22.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wertung {
    /// Richtig und nicht zu frueh: eintragen, Ergebnis 0 mit host_proof.
    Angenommen,
    /// Falsch (oder zu frueh): Ergebnis 2, weiterlesen.
    Falsch { warten_ms: u32 },
    /// Der fuenfte Fehlversuch dieser Verbindung: Ergebnis 4, schliessen.
    Schluss { warten_ms: u32 },
}

impl Wertung {
    /// Das Ergebnis fuer Nachricht 22; `host_beweis` gilt nur bei Angenommen.
    pub fn ergebnis(self, host_beweis: [u8; 32]) -> Ergebnis {
        match self {
            Wertung::Angenommen => Ergebnis::Passwort { host_beweis },
            Wertung::Falsch { warten_ms } => Ergebnis::Falsch { warten_ms },
            Wertung::Schluss { warten_ms } => Ergebnis::Schluss { warten_ms },
        }
    }
}

impl Drossel {
    pub fn neu() -> Drossel {
        Drossel::default()
    }

    fn global_aufraeumen(&mut self, jetzt: Instant) {
        while let Some(t) = self.global.front() {
            if jetzt.saturating_duration_since(*t) >= GLOBAL_FENSTER {
                self.global.pop_front();
            } else {
                break;
            }
        }
    }

    /// Gab es in den letzten 60 s mehr als 30 Fehlversuche?
    pub fn global_gesperrt(&mut self, jetzt: Instant) -> bool {
        self.global_aufraeumen(jetzt);
        self.global.len() > GLOBAL_GRENZE
    }

    /// Wie lange diese Gegenstelle ab `jetzt` warten muss, bevor ihr Beweis
    /// zaehlt: das Groessere aus IP- und Schluesselzaehler, bei globaler
    /// Sperre mindestens 60 s.
    pub fn warten(&mut self, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> Duration {
        let w = self.rest(ip, schluessel, jetzt);
        if self.global_gesperrt(jetzt) {
            w.max(GLOBAL_WARTEN)
        } else {
            w
        }
    }

    /// Einen Fehlversuch buchen (IP, Schluessel, global); liefert die neue
    /// Wartezeit ab `jetzt`.
    pub fn fehlversuch(&mut self, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> Duration {
        buchen(&mut self.je_ip, &ip, jetzt);
        buchen(&mut self.je_schluessel, &schluessel.to_vec(), jetzt);
        self.global_aufraeumen(jetzt);
        self.global.push_back(jetzt);
        while self.global.len() > GLOBAL_GRENZE + 1 {
            self.global.pop_front();
        }
        self.warten(ip, schluessel, jetzt)
    }

    /// Richtiger Beweis: beide Zaehler dieser Gegenstelle zurueck.
    pub fn erfolg(&mut self, ip: IpAddr, schluessel: &[u8]) {
        self.je_ip.remove(&ip);
        self.je_schluessel.remove(schluessel);
    }

    /// Was die Drossel dieser Adresse bzw. dieses Schluessels ab `jetzt`
    /// noch verlangt - das Groessere, ohne die globale Regel (wie
    /// qc_zugang_drossel_rest im Mac-Host). Die globale Sperre gilt beim
    /// Beginn einer Phase (Nachricht 20); gaelte sie auch beim Beweis, sperrte
    /// eine laufende Flut jeden rechtmaessigen Nutzer fuer immer aus.
    pub fn rest(&self, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> Duration {
        let a = self.je_ip.get(&ip).map(|z| z.rest(jetzt)).unwrap_or_default();
        let b = self.je_schluessel.get(schluessel).map(|z| z.rest(jetzt)).unwrap_or_default();
        a.max(b)
    }

    /// Kaeme ein Beweis dieser Phase jetzt zu frueh? Vor Ablauf dessen, was
    /// diese Verbindung als Wartezeit gesagt bekam - und ebenso, solange die
    /// Drossel ihrer Adresse oder ihres Schluessels jetzt noch laeuft (3.3:
    /// der groessere Wert gilt). Sonst riete eine zweite Verbindung derselben
    /// Adresse mit anderem Schluessel ungebremst weiter, waehrend die erste
    /// wartet - und ein richtiger Beweis kaeme in dieser Wartezeit durch.
    pub fn zu_frueh(&self, phase: &Phase, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> bool {
        phase.zu_frueh(jetzt) || !self.rest(ip, schluessel, jetzt).is_zero()
    }

    /// Die groessere der beiden Zahlen (fuer die Protokollzeile).
    pub fn fehlversuche(&self, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> u32 {
        let zahl = |z: Option<&Zaehler>| z.filter(|z| !z.verfallen(jetzt)).map(|z| z.fehlversuche).unwrap_or(0);
        zahl(self.je_ip.get(&ip)).max(zahl(self.je_schluessel.get(schluessel)))
    }

    /// Eine Zugangsphase beginnt: `phase.warten_ms(jetzt)` geht mit
    /// Nachricht 20 hinaus.
    pub fn phase_beginnen(&mut self, ip: IpAddr, schluessel: &[u8], jetzt: Instant) -> Phase {
        let w = self.warten(ip, schluessel, jetzt);
        Phase { beginn: jetzt, frei_ab: jetzt + w, fehlversuche: 0 }
    }

    /// Wertet einen Beweis (Nachricht 21). `richtig`: stimmte er
    /// (beweis_pruefen)? Kommt er zu frueh (`zu_frueh`: Wartezeit dieser
    /// Verbindung oder Drossel von Adresse bzw. Schluessel), zaehlt er als
    /// Fehlversuch, auch wenn er stimmt. Nach dem fuenften Fehlversuch in
    /// dieser Verbindung: Schluss.
    pub fn beweis_werten(
        &mut self,
        phase: &mut Phase,
        ip: IpAddr,
        schluessel: &[u8],
        richtig: bool,
        jetzt: Instant,
    ) -> Wertung {
        if richtig && !self.zu_frueh(phase, ip, schluessel, jetzt) {
            self.erfolg(ip, schluessel);
            return Wertung::Angenommen;
        }
        phase.fehlversuche += 1;
        let w = self.fehlversuch(ip, schluessel, jetzt);
        phase.frei_ab = jetzt + w;
        let warten_ms = millis(w);
        if phase.fehlversuche >= PHASE_FEHLVERSUCHE {
            Wertung::Schluss { warten_ms }
        } else {
            Wertung::Falsch { warten_ms }
        }
    }

    /// Ein Beweis, der sich nicht pruefen laesst, weil der Host kein lesbares
    /// Passwort hat (4.3: dann nur "Zulassen"). Der Fehler liegt beim Host,
    /// nicht beim Client: keine Drossel - wer das richtige Passwort kennt,
    /// soll nach dem Reparieren nicht minutenlang warten (wie der Mac-Host).
    /// Die Versuche dieser Verbindung zaehlen trotzdem, damit niemand die
    /// Phase in einer Schleife beschaeftigt; warten_ms ist 0.
    pub fn beweis_nicht_pruefbar(&mut self, phase: &mut Phase) -> Wertung {
        phase.fehlversuche += 1;
        if phase.fehlversuche >= PHASE_FEHLVERSUCHE {
            Wertung::Schluss { warten_ms: 0 }
        } else {
            Wertung::Falsch { warten_ms: 0 }
        }
    }
}

/// Hoechstens so viele Zugangsphasen gleichzeitig (je Port).
pub const PLAETZE_GESAMT: usize = 4;
/// ... und je Client-Schluessel bzw. je IP.
pub const PLAETZE_JE_SCHLUESSEL: usize = 1;
pub const PLAETZE_JE_IP: usize = 2;

/// Belegte Plaetze fuer Zugangsphasen (Spezifikation 3.2). Wer keinen Platz
/// bekommt, erhaelt sofort Ergebnis 4 mit BESETZT_WARTEN_MS. Reiner Zustand
/// hinter dem Mutex des Aufrufers; jedes erfolgreiche `belegen` braucht
/// genau ein `freigeben`.
#[derive(Debug, Default)]
pub struct Plaetze {
    belegt: Vec<(IpAddr, Vec<u8>)>,
}

impl Plaetze {
    pub fn belegen(&mut self, ip: IpAddr, schluessel: &[u8]) -> bool {
        let je_ip = self.belegt.iter().filter(|(i, _)| *i == ip).count();
        let je_schluessel = self.belegt.iter().filter(|(_, s)| s == schluessel).count();
        if self.belegt.len() >= PLAETZE_GESAMT || je_ip >= PLAETZE_JE_IP || je_schluessel >= PLAETZE_JE_SCHLUESSEL {
            return false;
        }
        self.belegt.push((ip, schluessel.to_vec()));
        true
    }

    pub fn freigeben(&mut self, ip: IpAddr, schluessel: &[u8]) {
        if let Some(i) = self.belegt.iter().position(|(a, s)| *a == ip && s == schluessel) {
            self.belegt.remove(i);
        }
    }

    pub fn belegt(&self) -> usize {
        self.belegt.len()
    }
}

// ------------------------------------------------------- 3.4 Kryptografie

/// PBKDF2-Runden fuer K.
pub const PBKDF2_RUNDEN: u32 = 100_000;

/// norm(pw): die UTF-8-Bytes wie eingegeben, ohne 0x20, 0x09, 0x0A, 0x0D und
/// '-'; A-Z werden a-z; sonst unveraendert (keine Unicode-Normalisierung,
/// kein Unicode-Kleinschreiben). "K7M 4WQ-9TZ" -> "k7m4wq9tz".
pub fn passwort_norm(pw: &str) -> Vec<u8> {
    pw.bytes().filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'-')).map(|b| b.to_ascii_lowercase()).collect()
}

/// HMAC-SHA256 nach RFC 2104 (Block 64 Byte), mit vorbereiteten inneren
/// und aeusseren Zustaenden - PBKDF2 rechnet damit je Runde nur zwei
/// Kompressionen statt vier.
#[derive(Clone)]
struct Hmac {
    innen: Sha256,
    aussen: Sha256,
}

impl Hmac {
    fn neu(schluessel: &[u8]) -> Hmac {
        let mut block = [0u8; 64];
        if schluessel.len() > 64 {
            block[..32].copy_from_slice(&Sha256::digest(schluessel));
        } else {
            block[..schluessel.len()].copy_from_slice(schluessel);
        }
        let mut ipad = [0u8; 64];
        let mut opad = [0u8; 64];
        for i in 0..64 {
            ipad[i] = block[i] ^ 0x36;
            opad[i] = block[i] ^ 0x5c;
        }
        let mut innen = Sha256::new();
        innen.update(ipad);
        let mut aussen = Sha256::new();
        aussen.update(opad);
        Hmac { innen, aussen }
    }

    fn rechnen(&self, teile: &[&[u8]]) -> [u8; 32] {
        let mut i = self.innen.clone();
        for t in teile {
            i.update(t);
        }
        let mut a = self.aussen.clone();
        a.update(i.finalize());
        let mut aus = [0u8; 32];
        aus.copy_from_slice(&a.finalize());
        aus
    }
}

/// HMAC-SHA256(schluessel, daten).
pub fn hmac_sha256(schluessel: &[u8], daten: &[u8]) -> [u8; 32] {
    Hmac::neu(schluessel).rechnen(&[daten])
}

/// PBKDF2-HMAC-SHA256 nach RFC 8018, Abschnitt 5.2: fuellt `ausgabe` (jede
/// Laenge) Block fuer Block. `runden` 0 zaehlt wie 1.
pub fn pbkdf2_sha256(passwort: &[u8], salz: &[u8], runden: u32, ausgabe: &mut [u8]) {
    let mac = Hmac::neu(passwort);
    for (i, block) in ausgabe.chunks_mut(32).enumerate() {
        let nummer = (i as u32 + 1).to_be_bytes();
        let mut u = mac.rechnen(&[salz, &nummer]);
        let mut t = u;
        for _ in 1..runden.max(1) {
            u = mac.rechnen(&[&u]);
            for (x, y) in t.iter_mut().zip(u) {
                *x ^= y;
            }
        }
        block.copy_from_slice(&t[..block.len()]);
    }
}

/// K = PBKDF2-HMAC-SHA256(norm(pw), "QuadChroma/1 pw\0" || host_pub32,
/// 100 000 Runden, 32 Byte). Rechnet spuerbar (Grossenordnung 50 ms in
/// einem Release-Bau) - nicht unter einer Sperre und nicht auf dem Zeichenfaden.
pub fn passwort_schluessel(pw: &str, host_pub: &[u8]) -> [u8; 32] {
    let mut salz = b"QuadChroma/1 pw\0".to_vec();
    salz.extend_from_slice(host_pub);
    let mut k = [0u8; 32];
    pbkdf2_sha256(&passwort_norm(pw), &salz, PBKDF2_RUNDEN, &mut k);
    k
}

/// client_proof = HMAC-SHA256(K, "QuadChroma/1 auth client\0" || hh).
pub fn client_beweis(k: &[u8; 32], hh: &[u8]) -> [u8; 32] {
    Hmac::neu(k).rechnen(&[b"QuadChroma/1 auth client\0", hh])
}

/// host_proof = HMAC-SHA256(K, "QuadChroma/1 auth host\0" || hh).
pub fn host_beweis(k: &[u8; 32], hh: &[u8]) -> [u8; 32] {
    Hmac::neu(k).rechnen(&[b"QuadChroma/1 auth host\0", hh])
}

/// Host: stimmt der empfangene client_proof? (konstante Zeit)
pub fn beweis_pruefen(k: &[u8; 32], hh: &[u8], erhalten: &[u8]) -> bool {
    gleich(&client_beweis(k, hh), erhalten)
}

/// Client: stimmt der host_proof aus Ergebnis 0? Wenn nicht: abbrechen,
/// NICHT pinnen (moeglicher Angriff). (konstante Zeit)
pub fn host_beweis_pruefen(k: &[u8; 32], hh: &[u8], erhalten: &[u8]) -> bool {
    gleich(&host_beweis(k, hh), erhalten)
}

/// Vergleich in konstanter Zeit (bei gleicher Laenge; die Laenge selbst ist
/// kein Geheimnis).
pub fn gleich(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    std::hint::black_box(d) == 0
}

/// K fuer das aktuelle Passwort (Host): neu gerechnet nur, wenn sich
/// norm(pw) oder der eigene Schluessel geaendert haben - ein geaendertes
/// Passwort verwirft den alten Wert also von selbst.
#[derive(Default)]
pub struct Schluesselcache {
    eintrag: Option<(Vec<u8>, Vec<u8>, [u8; 32])>,
}

impl Schluesselcache {
    pub fn schluessel(&mut self, pw: &str, host_pub: &[u8]) -> [u8; 32] {
        let norm = passwort_norm(pw);
        if let Some((n, p, k)) = &self.eintrag {
            if *n == norm && p == host_pub {
                return *k;
            }
        }
        let k = passwort_schluessel(pw, host_pub);
        self.eintrag = Some((norm, host_pub.to_vec(), k));
        k
    }

    pub fn verwerfen(&mut self) {
        self.eintrag = None;
    }
}

// ------------------------------------------------------- Dateien: Grundlagen

/// Dateinamen im Ablageordner (secure::config_dir). Das Praefix host- trennt
/// die Dateien der Host-Rolle von denen des Clients: auf dem Mac teilen sich
/// Mac-Host und Mac-Client den Ordner, unter Windows Client und Host-Rolle.
pub const GERAETE_DATEI: &str = "host-devices.txt";
pub const PASSWORT_DATEI: &str = "host-password.txt";
pub const HOSTS_DATEI: &str = "hosts.txt";
/// Vorgaenger, aus denen einmal uebernommen wird (4.2, 4.4).
pub const ALTE_FREIGABEN: &str = "authorized.txt";
pub const ALTE_HOSTS: &str = "known_hosts.txt";
/// Endung der uebernommenen Vorgaenger.
pub const MIGRIERT: &str = ".migriert";
/// Bildport, wenn eine alte Liste keinen kennt (known_hosts.txt).
pub const STANDARD_PORT: u16 = 9001;

/// Groesser ist keine Liste von Hand oder vom Programm - alles darueber gilt
/// als beschaedigt (und wird nicht erst ganz gelesen).
const LISTE_GRENZE: u64 = 1 << 20;
const PASSWORT_GRENZE: u64 = 4096;

/// Pfad einer Datei im Ablageordner (legt den Ordner bei Bedarf an).
pub fn ablage_pfad(name: &str) -> Result<PathBuf, String> {
    Ok(crate::secure::config_dir()?.join(name))
}

/// Was beim Lesen oder Schreiben einer dieser Dateien schiefgeht. `Display`
/// ist der deutsche Text fuers Protokoll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DateiFehler {
    /// Vorhanden, aber nicht zu lesen (Rechte, Sperre, Ordner an der Stelle).
    Unlesbar { pfad: PathBuf, grund: String },
    /// Vorhanden, aber kein UTF-8.
    KeinUtf8 { pfad: PathBuf },
    /// Lesbar, aber nicht im erwarteten Aufbau (Zeile, Groesse, Inhalt).
    Beschaedigt { pfad: PathBuf, grund: String },
    /// Schreiben oder Umbenennen gescheitert; die alte Datei gilt weiter.
    Schreiben { pfad: PathBuf, grund: String },
}

impl DateiFehler {
    /// Ein Fehler beim Lesen (nicht beim Schreiben) - fuer die Geraeteliste
    /// heisst das: "Geraeteliste beschaedigt" mit "Liste zuruecksetzen".
    pub fn beim_lesen(&self) -> bool {
        !matches!(self, DateiFehler::Schreiben { .. })
    }

    pub fn pfad(&self) -> &Path {
        match self {
            DateiFehler::Unlesbar { pfad, .. }
            | DateiFehler::KeinUtf8 { pfad }
            | DateiFehler::Beschaedigt { pfad, .. }
            | DateiFehler::Schreiben { pfad, .. } => pfad,
        }
    }
}

impl std::fmt::Display for DateiFehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DateiFehler::Unlesbar { pfad, grund } => write!(f, "{} nicht lesbar: {grund}", pfad.display()),
            DateiFehler::KeinUtf8 { pfad } => write!(f, "{} ist kein UTF-8", pfad.display()),
            DateiFehler::Beschaedigt { pfad, grund } => write!(f, "{} ist beschaedigt: {grund}", pfad.display()),
            DateiFehler::Schreiben { pfad, grund } => write!(f, "{} nicht zu schreiben: {grund}", pfad.display()),
        }
    }
}

/// Fuer den Client: dieselben Meldungen wie bei known_hosts.txt heute
/// (FileUnreadable, FileNotUtf8, FileNotWritable).
impl From<DateiFehler> for crate::secure::Fehler {
    fn from(f: DateiFehler) -> crate::secure::Fehler {
        use crate::secure::Fehler as F;
        match f {
            DateiFehler::Unlesbar { pfad, grund } => F::Unlesbar { pfad, grund },
            DateiFehler::KeinUtf8 { pfad } => F::KeinUtf8 { pfad },
            DateiFehler::Beschaedigt { pfad, grund } => F::Unlesbar { pfad, grund: format!("beschaedigt: {grund}") },
            DateiFehler::Schreiben { pfad, grund } => F::Schreiben { pfad, grund },
        }
    }
}

/// Der Wortlaut des Systems in einer Zeile (Windows bricht mit CRLF um).
fn wortlaut(e: &std::io::Error) -> String {
    e.to_string().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Liest eine Textdatei. None: sie fehlt. Groesser als `grenze`: beschaedigt.
/// Kein UTF-8: KeinUtf8. Ein fuehrendes BOM (Editor "UTF-8 mit BOM") wird
/// uebergangen.
fn text_lesen(pfad: &Path, grenze: u64) -> Result<Option<String>, DateiFehler> {
    let unlesbar = |e: std::io::Error| DateiFehler::Unlesbar { pfad: pfad.to_path_buf(), grund: wortlaut(&e) };
    let f = match std::fs::File::open(pfad) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unlesbar(e)),
    };
    let mut b = Vec::new();
    f.take(grenze + 1).read_to_end(&mut b).map_err(unlesbar)?;
    if b.len() as u64 > grenze {
        return Err(DateiFehler::Beschaedigt {
            pfad: pfad.to_path_buf(),
            grund: format!("groesser als {grenze} Byte"),
        });
    }
    let text = String::from_utf8(b).map_err(|_| DateiFehler::KeinUtf8 { pfad: pfad.to_path_buf() })?;
    Ok(Some(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text)))
}

/// Gibt es an diesem Pfad etwas? Nur ein sicheres "fehlt" ist false - ist
/// die Frage selbst nicht zu beantworten (Rechte), gilt es als vorhanden.
fn vorhanden(pfad: &Path) -> bool {
    match std::fs::symlink_metadata(pfad) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Laufende Nummer der Zwischendateien dieses Prozesses.
static ZWISCHEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Zwischendatei neben `pfad`: "<name>.<pid>-<nr>.neu".
fn zwischenname(pfad: &Path) -> PathBuf {
    let name = pfad.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nr = ZWISCHEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    pfad.with_file_name(format!("{name}.{}-{nr}.neu", std::process::id()))
}

/// Legt `pfad` neu an (nie ueber eine vorhandene Datei), schreibt und
/// synchronisiert; unter Unix nur fuer den Eigentuemer (0600).
fn neu_schreiben(pfad: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(pfad)?;
    let r = f.write_all(inhalt).and_then(|_| f.sync_all());
    if r.is_err() {
        drop(f);
        let _ = std::fs::remove_file(pfad);
    }
    r
}

/// Ersetzt `pfad` atomar: erst vollstaendig in eine Zwischendatei (0600,
/// sync), dann umbenennen - ein Abbruch hinterlaesst nie eine halbe Liste.
/// Unter Windows ersetzt das Umbenennen eine vorhandene Datei ebenso.
fn atomar_schreiben(pfad: &Path, inhalt: &[u8]) -> Result<(), DateiFehler> {
    let tmp = zwischenname(pfad);
    let r = neu_schreiben(&tmp, inhalt).and_then(|_| std::fs::rename(&tmp, pfad));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(DateiFehler::Schreiben { pfad: pfad.to_path_buf(), grund: wortlaut(&e) });
    }
    // Auch den Ordnereintrag festschreiben (unter Windows nicht noetig und
    // nicht moeglich).
    #[cfg(unix)]
    if let Some(ordner) = pfad.parent() {
        if let Ok(d) = std::fs::File::open(ordner) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Legt `pfad` an, aber NUR, wenn es ihn noch nicht gibt: erst in eine
/// Zwischendatei, dann per hartem Verweis (scheitert mit AlreadyExists, wenn
/// ein zweiter Prozess schneller war - wie bei secure::geheim_schreiben).
/// Ohne harte Verweise (FAT, manche Freigaben) direkt und exklusiv.
fn exklusiv_anlegen(pfad: &Path, inhalt: &[u8]) -> std::io::Result<()> {
    let tmp = zwischenname(pfad);
    let r = neu_schreiben(&tmp, inhalt).and_then(|_| match std::fs::hard_link(&tmp, pfad) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => neu_schreiben(pfad, inhalt),
        x => x,
    });
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Entfernt Zwischendateien "<name>.*.neu" neben `pfad`, die ein
/// abgebrochener Lauf liegen liess (einmal beim Start aufrufen, solange noch
/// niemand schreibt). Liefert, wie viele es waren.
pub fn zwischendateien_aufraeumen(pfad: &Path) -> usize {
    let (Some(ordner), Some(name)) = (pfad.parent(), pfad.file_name().and_then(|n| n.to_str())) else {
        return 0;
    };
    let Ok(eintraege) = std::fs::read_dir(ordner) else { return 0 };
    let praefix = format!("{name}.");
    let mut n = 0;
    for e in eintraege.flatten() {
        let datei = e.file_name();
        let Some(datei) = datei.to_str() else { continue };
        if datei.starts_with(&praefix) && datei.ends_with(".neu") && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Sperre je Dateiart: Lesen-Aendern-Schreiben innerhalb des Prozesses
/// nacheinander. Eine vergiftete Sperre (Panik in einem anderen Faden) wird
/// weiter benutzt - die Datei selbst ist durch das atomare Schreiben heil.
fn sperren(m: &'static Mutex<()>) -> std::sync::MutexGuard<'static, ()> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
static GERAETE_SPERRE: Mutex<()> = Mutex::new(());
static PASSWORT_SPERRE: Mutex<()> = Mutex::new(());
static HOSTS_SPERRE: Mutex<()> = Mutex::new(());

/// 32 Byte als 64 Hexziffern (klein).
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 64 Hexziffern (gross oder klein) als Schluessel; sonst None.
pub fn hex_lesen(text: &str) -> Option<[u8; 32]> {
    let b = text.as_bytes();
    if b.len() != 64 || !b.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let wert = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
    let mut k = [0u8; 32];
    for (i, paar) in b.chunks(2).enumerate() {
        k[i] = wert(paar[0]) << 4 | wert(paar[1]);
    }
    Some(k)
}

/// Erstes Wort (bis zum Leerraum) und der Rest ohne fuehrenden Leerraum.
fn wort(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

/// "JJJJ-MM-TT" mit Monat 1-12 und Tag 1-31.
fn datum_gueltig(d: &str) -> bool {
    let b = d.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if ![0, 1, 2, 3, 5, 6, 8, 9].iter().all(|&i| b[i].is_ascii_digit()) {
        return false;
    }
    let zahl = |a: usize, e: usize| d[a..e].parse::<u32>().unwrap_or(0);
    (1..=12).contains(&zahl(5, 7)) && (1..=31).contains(&zahl(8, 10))
}

/// Ergebnis einer Uebernahme aus einer alten Liste (4.2, 4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Migration {
    /// Nichts zu tun: die neue Datei gibt es schon, oder die alte fehlt.
    Keine,
    /// `anzahl` Eintraege uebernommen. `umbenannt`: die alte Datei heisst
    /// jetzt *.migriert; Err: sie liess sich nicht umbenennen (Grund) - die
    /// neue Datei gilt trotzdem, und es wird nicht noch einmal uebernommen.
    Uebernommen { anzahl: usize, umbenannt: Result<(), String> },
    /// Die alte Datei war nicht zu lesen oder die neue nicht zu schreiben;
    /// nichts wurde angelegt oder umbenannt (der naechste Start versucht es
    /// wieder).
    Fehler(String),
}

/// Benennt die alte Datei nach der Uebernahme in *.migriert um.
fn alt_umbenennen(alt: &Path) -> Result<(), String> {
    let mut ziel = alt.as_os_str().to_owned();
    ziel.push(MIGRIERT);
    std::fs::rename(alt, PathBuf::from(ziel)).map_err(|e| format!("{}: {}", alt.display(), wortlaut(&e)))
}

// --------------------------------------------- 4.1 host-devices.txt (Host)

/// Ein erlaubtes Geraet. Zeile in host-devices.txt:
/// "<64 hex>  <JJJJ-MM-TT>  <Name>".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Geraet {
    pub schluessel: [u8; 32],
    /// Seit wann es erlaubt ist ("JJJJ-MM-TT").
    pub datum: String,
    /// Bereinigt, <= 40 Byte, nie leer ("-", wenn es keinen gab).
    pub name: String,
}

impl Geraet {
    pub fn id(&self) -> u32 {
        geraete_id(&self.schluessel)
    }

    fn zeile(&self) -> String {
        format!("{}  {}  {}\n", hex(&self.schluessel), self.datum, self.name)
    }
}

/// Name fuer die Listen: bereinigt, und "-" statt leer.
fn listenname(name: &str) -> String {
    let n = name_bereinigen(name);
    if n.is_empty() {
        "-".into()
    } else {
        n
    }
}

/// Die Geraeteliste des Hosts, wie sie gerade in der Datei steht.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Geraeteliste {
    pub geraete: Vec<Geraet>,
    /// Gab es die Datei? (Nur fuer die Startzeile im Protokoll.)
    pub vorhanden: bool,
}

impl Geraeteliste {
    /// Liest host-devices.txt. Fehlt sie: leer. Unlesbar, kein UTF-8,
    /// groesser als 1 MiB oder eine Zeile ohne gueltigen Aufbau: Err - dann
    /// ist niemand bekannt (Zugangsphase fuer alle), und keine Funktion hier
    /// ueberschreibt die Datei, bis `geraeteliste_zuruecksetzen` sie beiseite
    /// legt. Leerzeilen und #-Zeilen sind erlaubt, CRLF und BOM ebenso, der
    /// Schluessel in Gross- oder Kleinbuchstaben; ein doppelter Schluessel
    /// zaehlt einmal.
    pub fn laden(pfad: &Path) -> Result<Geraeteliste, DateiFehler> {
        match text_lesen(pfad, LISTE_GRENZE)? {
            None => Ok(Geraeteliste::default()),
            Some(text) => Geraeteliste::aus_text(&text)
                .map(|geraete| Geraeteliste { geraete, vorhanden: true })
                .map_err(|grund| DateiFehler::Beschaedigt { pfad: pfad.to_path_buf(), grund }),
        }
    }

    /// Die Eintraege eines Dateiinhalts; Err mit Zeilennummer und Grund.
    pub fn aus_text(text: &str) -> Result<Vec<Geraet>, String> {
        let mut geraete: Vec<Geraet> = Vec::new();
        for (nr, zeile) in text.lines().enumerate() {
            let t = zeile.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let fehler = |grund: &str| format!("Zeile {}: {grund}", nr + 1);
            let (h, rest) = wort(t);
            let schluessel = hex_lesen(h).ok_or_else(|| fehler("kein Schluessel aus 64 Hexziffern"))?;
            let (datum, rest) = wort(rest);
            if !datum_gueltig(datum) {
                return Err(fehler("kein Datum JJJJ-MM-TT"));
            }
            if geraete.iter().any(|g| g.schluessel == schluessel) {
                continue;
            }
            geraete.push(Geraet { schluessel, datum: datum.to_string(), name: listenname(rest) });
        }
        Ok(geraete)
    }

    /// Dateiinhalt: eine Zeile je Geraet.
    pub fn text(&self) -> String {
        self.geraete.iter().map(Geraet::zeile).collect()
    }

    pub fn finden(&self, schluessel: &[u8]) -> Option<&Geraet> {
        self.geraete.iter().find(|g| g.schluessel[..] == *schluessel)
    }

    /// Ist dieses Geraet erlaubt? (Der volle Schluessel, nie die ID.)
    pub fn enthaelt(&self, schluessel: &[u8]) -> bool {
        self.finden(schluessel).is_some()
    }
}

/// Traegt ein Geraet ein (Beweis richtig oder "Zulassen"). Ok(true): neu
/// eingetragen, Ok(false): war schon da (unveraendert). Eine unlesbare oder
/// beschaedigte Liste wird NICHT ueberschrieben (Err).
pub fn geraet_eintragen(pfad: &Path, schluessel: &[u8; 32], name: &str, datum: &str) -> Result<bool, DateiFehler> {
    let _s = sperren(&GERAETE_SPERRE);
    let mut liste = Geraeteliste::laden(pfad)?;
    if liste.enthaelt(schluessel) {
        return Ok(false);
    }
    liste.geraete.push(Geraet { schluessel: *schluessel, datum: datum.to_string(), name: listenname(name) });
    atomar_schreiben(pfad, liste.text().as_bytes())?;
    Ok(true)
}

/// Entfernt ein Geraet. Ok(false): es stand nicht in der Liste.
pub fn geraet_entfernen(pfad: &Path, schluessel: &[u8]) -> Result<bool, DateiFehler> {
    let _s = sperren(&GERAETE_SPERRE);
    let mut liste = Geraeteliste::laden(pfad)?;
    let vorher = liste.geraete.len();
    liste.geraete.retain(|g| g.schluessel[..] != *schluessel);
    if liste.geraete.len() == vorher {
        return Ok(false);
    }
    atomar_schreiben(pfad, liste.text().as_bytes())?;
    Ok(true)
}

/// "Alle entfernen": danach steht eine leere Liste da (nicht: keine Datei -
/// so wird authorized.txt auch nie noch einmal uebernommen). Eine
/// beschaedigte Liste bleibt unberuehrt (Err) - dafuer gibt es
/// `geraeteliste_zuruecksetzen`.
pub fn alle_geraete_entfernen(pfad: &Path) -> Result<(), DateiFehler> {
    let _s = sperren(&GERAETE_SPERRE);
    Geraeteliste::laden(pfad)?;
    atomar_schreiben(pfad, b"")
}

/// "Liste zuruecksetzen" bei beschaedigter Liste: die alte Datei bleibt als
/// "<name>.defekt-<stempel>" liegen (Zeitpunkt::stempel; gibt es den Namen
/// schon, mit -2, -3 ...), dann entsteht eine leere Liste. Liefert den
/// Namen der beiseitegelegten Datei (None: es gab keine).
pub fn geraeteliste_zuruecksetzen(pfad: &Path, stempel: &str) -> Result<Option<PathBuf>, DateiFehler> {
    let _s = sperren(&GERAETE_SPERRE);
    let mut beiseite = None;
    if vorhanden(pfad) {
        let name = pfad.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut ziel = pfad.with_file_name(format!("{name}.defekt-{stempel}"));
        let mut nr = 2;
        while vorhanden(&ziel) {
            ziel = pfad.with_file_name(format!("{name}.defekt-{stempel}-{nr}"));
            nr += 1;
        }
        std::fs::rename(pfad, &ziel)
            .map_err(|e| DateiFehler::Schreiben { pfad: pfad.to_path_buf(), grund: wortlaut(&e) })?;
        beiseite = Some(ziel);
    }
    atomar_schreiben(pfad, b"")?;
    Ok(beiseite)
}

// ------------------------------------------- 4.2 Uebernahme authorized.txt

/// Einmalige Uebernahme beim Start: existiert authorized.txt im `ordner` und
/// fehlt host-devices.txt, werden alle gueltigen Eintraege uebernommen
/// (Datum = `datum`, Name = der alte Name, meist die IP; ungueltige Zeilen
/// fallen weg), dann heisst authorized.txt authorized.txt.migriert. Beide
/// Formate (Mac und Windows) haben "<64 hex>  <Fingerabdruck>  <Name>".
/// Eine automatische Aufnahme des ersten Geraets gibt es danach nicht mehr.
pub fn geraete_migrieren(ordner: &Path, datum: &str) -> Migration {
    let _s = sperren(&GERAETE_SPERRE);
    let alt = ordner.join(ALTE_FREIGABEN);
    let neu = ordner.join(GERAETE_DATEI);
    if vorhanden(&neu) || !vorhanden(&alt) {
        return Migration::Keine;
    }
    let text = match text_lesen(&alt, LISTE_GRENZE) {
        Ok(Some(t)) => t,
        Ok(None) => return Migration::Keine,
        Err(e) => return Migration::Fehler(e.to_string()),
    };
    let mut liste = Geraeteliste { geraete: Vec::new(), vorhanden: false };
    for zeile in text.lines() {
        let (h, rest) = wort(zeile.trim());
        let Some(schluessel) = hex_lesen(h) else { continue };
        if liste.enthaelt(&schluessel) {
            continue;
        }
        // Der Fingerabdruck "XXXX-XXXX-XXXX-XXXX" steht vor dem Namen.
        let (fp, name) = wort(rest);
        let fingerabdruck =
            fp.len() == 19 && fp.split('-').all(|t| t.len() == 4 && t.bytes().all(|b| b.is_ascii_hexdigit()));
        let name = if fingerabdruck { name } else { rest };
        liste.geraete.push(Geraet { schluessel, datum: datum.to_string(), name: listenname(name) });
    }
    if let Err(e) = atomar_schreiben(&neu, liste.text().as_bytes()) {
        return Migration::Fehler(e.to_string());
    }
    Migration::Uebernommen { anzahl: liste.geraete.len(), umbenannt: alt_umbenennen(&alt) }
}

// ----------------------------------------------- 4.3 host-password.txt (Host)

/// Eigene Passwoerter: norm(pw) mindestens so viele Byte.
pub const PASSWORT_MIN: usize = 8;
/// ... und hoechstens so viele Byte wie eingegeben.
pub const PASSWORT_MAX: usize = 128;
/// Alphabet des Zufallspassworts: ohne i, l, o, 0, 1 (verwechselbar).
pub const ZUFALL_ALPHABET: &[u8; 31] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Warum ein Passwort nicht gesetzt wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswortFehler {
    /// norm(pw) kuerzer als 8 Byte (Text HostPasswordShort).
    ZuKurz,
    /// Laenger als 128 Byte.
    ZuLang,
    /// Enthaelt Steuerzeichen (die Datei hat genau eine Zeile).
    Steuerzeichen,
    /// Schreiben gescheitert; das alte Passwort gilt weiter.
    Datei(DateiFehler),
}

impl std::fmt::Display for PasswortFehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswortFehler::ZuKurz => write!(f, "Passwort zu kurz (mindestens {PASSWORT_MIN} Zeichen)"),
            PasswortFehler::ZuLang => write!(f, "Passwort zu lang (hoechstens {PASSWORT_MAX} Byte)"),
            PasswortFehler::Steuerzeichen => write!(f, "Passwort enthaelt Steuerzeichen"),
            PasswortFehler::Datei(e) => write!(f, "{e}"),
        }
    }
}

/// Die Regeln fuer ein Passwort (auch fuer das Passwortfenster).
pub fn passwort_pruefen(pw: &str) -> Result<(), PasswortFehler> {
    if pw.chars().any(char::is_control) {
        return Err(PasswortFehler::Steuerzeichen);
    }
    if pw.len() > PASSWORT_MAX {
        return Err(PasswortFehler::ZuLang);
    }
    if passwort_norm(pw).len() < PASSWORT_MIN {
        return Err(PasswortFehler::ZuKurz);
    }
    Ok(())
}

/// Das Zugangspasswort des Hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passwort {
    /// Klartext, so wie er angezeigt wird ("k7m-4wq-9tz").
    pub text: String,
    /// true: eben erst als Zufallspasswort angelegt (fuer die Protokollzeile).
    pub neu: bool,
}

/// Das Passwort aus dem Dateiinhalt: die erste Zeile, nach den Regeln von
/// `passwort_pruefen`; danach hoechstens Leerzeilen. Eine leere oder zu
/// kurze Datei ergibt KEIN Passwort (Err) - sonst kaeme man mit einer leeren
/// Eingabe herein.
fn passwort_aus_text(text: &str) -> Result<String, String> {
    let mut zeilen = text.lines();
    let pw = zeilen.next().unwrap_or("");
    if zeilen.any(|z| !z.trim().is_empty()) {
        return Err("mehr als eine Zeile".into());
    }
    passwort_pruefen(pw).map_err(|f| f.to_string())?;
    Ok(pw.to_string())
}

/// Liest host-password.txt. Fehlt sie, entsteht ein Zufallspasswort
/// (Passwort::neu) - exklusiv, nie ueber eine Datei, die ein zweiter
/// Prozess inzwischen angelegt hat. Unlesbar oder ungueltig: Err - dann
/// gibt es Zugang nur per "Zulassen", und die Oberflaeche bietet "Neues
/// Zufallspasswort" an (`passwort_zufall_setzen`).
pub fn passwort_laden(pfad: &Path) -> Result<Passwort, DateiFehler> {
    let _s = sperren(&PASSWORT_SPERRE);
    let lesen = || -> Result<Option<Passwort>, DateiFehler> {
        match text_lesen(pfad, PASSWORT_GRENZE)? {
            None => Ok(None),
            Some(t) => passwort_aus_text(&t)
                .map(|text| Some(Passwort { text, neu: false }))
                .map_err(|grund| DateiFehler::Beschaedigt { pfad: pfad.to_path_buf(), grund }),
        }
    };
    if let Some(p) = lesen()? {
        return Ok(p);
    }
    let schreiben = |grund: String| DateiFehler::Schreiben { pfad: pfad.to_path_buf(), grund };
    let pw = zufallspasswort().map_err(schreiben)?;
    match exklusiv_anlegen(pfad, format!("{pw}\n").as_bytes()) {
        Ok(()) => Ok(Passwort { text: pw, neu: true }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => lesen()?.ok_or_else(|| schreiben(wortlaut(&e))),
        Err(e) => Err(schreiben(wortlaut(&e))),
    }
}

/// Setzt ein eigenes Passwort (Passwortfenster). Leerzeichen und Tabs aussen
/// fallen weg (norm entfernt sie ohnehin); gespeichert wird der Rest so, wie
/// er eingegeben wurde. Liefert das gespeicherte Passwort.
pub fn passwort_setzen(pfad: &Path, pw: &str) -> Result<String, PasswortFehler> {
    let pw = pw.trim_matches([' ', '\t']);
    passwort_pruefen(pw)?;
    let _s = sperren(&PASSWORT_SPERRE);
    atomar_schreiben(pfad, format!("{pw}\n").as_bytes()).map_err(PasswortFehler::Datei)?;
    Ok(pw.to_string())
}

/// "Neues Zufallspasswort": erzeugt und speichert eins (ersetzt das alte).
pub fn passwort_zufall_setzen(pfad: &Path) -> Result<String, DateiFehler> {
    let pw = zufallspasswort().map_err(|grund| DateiFehler::Schreiben { pfad: pfad.to_path_buf(), grund })?;
    let _s = sperren(&PASSWORT_SPERRE);
    atomar_schreiben(pfad, format!("{pw}\n").as_bytes())?;
    Ok(pw)
}

/// Zufallspasswort: 9 Zeichen aus ZUFALL_ALPHABET (CSPRNG, ohne
/// Modulo-Schieflage), als "xxx-xxx-xxx" - rund 44,6 Bit.
pub fn zufallspasswort() -> Result<String, String> {
    let mut zeichen: Vec<u8> = Vec::with_capacity(9);
    while zeichen.len() < 9 {
        let mut b = [0u8; 32];
        zufall(&mut b)?;
        // 248 = 8 * 31: nur Bytes darunter, dann ist jedes Zeichen gleich wahrscheinlich.
        for x in b.iter().filter(|&&x| x < 248).take(9 - zeichen.len()) {
            zeichen.push(ZUFALL_ALPHABET[*x as usize % ZUFALL_ALPHABET.len()]);
        }
    }
    let s = String::from_utf8(zeichen).map_err(|_| "Zufallspasswort".to_string())?;
    Ok(format!("{}-{}-{}", &s[0..3], &s[3..6], &s[6..9]))
}

/// Fuellt `puffer` aus dem kryptografischen Zufall des Systems: Windows
/// BCryptGenRandom (Systemgenerator), sonst /dev/urandom. getrandom ist
/// keine direkte Abhaengigkeit (nur ueber snow), deshalb hier von Hand.
pub fn zufall(puffer: &mut [u8]) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let st = unsafe { BCryptGenRandom(None, puffer, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        if st.0 < 0 {
            return Err(format!("BCryptGenRandom: 0x{:08x}", st.0 as u32));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(puffer))
            .map_err(|e| format!("/dev/urandom: {}", wortlaut(&e)))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = puffer;
        Err("kein Zufallsgenerator des Systems".into())
    }
}

// --------------------------------------------------------- 4.4 hosts.txt (Client)

/// Ein bekannter (gepinnter) Host. Zeile in hosts.txt:
/// "<ID 9 Ziffern> <64 hex> <letzte Adresse ip:port> <Name>".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BekannterHost {
    /// Immer geraete_id(schluessel).
    pub id: u32,
    pub schluessel: [u8; 32],
    /// Zuletzt erreicht unter dieser Adresse ("ip:port", ohne Leerraum).
    pub adresse: String,
    /// Bereinigt, <= 40 Byte; ohne Namen die Adresse.
    pub name: String,
}

impl BekannterHost {
    pub fn neu(schluessel: [u8; 32], adresse: &str, name: &str) -> BekannterHost {
        let adresse = adresse.trim().to_string();
        let name = match name_bereinigen(name) {
            n if n.is_empty() => name_bereinigen(&adresse),
            n => n,
        };
        BekannterHost { id: geraete_id(&schluessel), schluessel, adresse, name }
    }

    fn zeile(&self) -> String {
        format!("{} {} {} {}\n", id_ziffern(self.id), hex(&self.schluessel), self.adresse, self.name)
    }

    /// Eine Zeile lesen; None, wenn sie nicht passt (auch: ID und Schluessel
    /// passen nicht zusammen).
    fn aus_zeile(zeile: &str) -> Option<BekannterHost> {
        let (id, rest) = wort(zeile.trim());
        let (h, rest) = wort(rest);
        let (adresse, name) = wort(rest);
        if id.len() != 9 || !id.bytes().all(|b| b.is_ascii_digit()) || adresse.is_empty() {
            return None;
        }
        let schluessel = hex_lesen(h)?;
        let h = BekannterHost::neu(schluessel, adresse, name);
        (id_ziffern(h.id) == id).then_some(h)
    }
}

/// Die bekannten Hosts des Clients, neuester Eintrag zuerst.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hostliste {
    pub hosts: Vec<BekannterHost>,
    /// Zeilen, die keine gueltigen Eintraege sind (von Hand, von einer
    /// spaeteren Fassung): sie bleiben beim Speichern erhalten, am Ende.
    fremd: Vec<String>,
}

impl Hostliste {
    /// Liest hosts.txt. Fehlt sie: leer. Unlesbar oder kein UTF-8: Err (wie
    /// known_hosts.txt heute - dann wird nicht verbunden). Einzelne kaputte
    /// Zeilen werden uebergangen, aber nicht geloescht.
    pub fn laden(pfad: &Path) -> Result<Hostliste, DateiFehler> {
        Ok(text_lesen(pfad, LISTE_GRENZE)?.map(|t| Hostliste::aus_text(&t)).unwrap_or_default())
    }

    pub fn aus_text(text: &str) -> Hostliste {
        let mut liste = Hostliste::default();
        for zeile in text.lines() {
            if zeile.trim().is_empty() {
                continue;
            }
            match BekannterHost::aus_zeile(zeile) {
                Some(h) if liste.nach_schluessel(&h.schluessel).is_none() => liste.hosts.push(h),
                Some(_) => {}
                None => liste.fremd.push(zeile.trim_end().to_string()),
            }
        }
        liste
    }

    pub fn text(&self) -> String {
        let mut t: String = self.hosts.iter().map(BekannterHost::zeile).collect();
        for z in &self.fremd {
            t.push_str(z);
            t.push('\n');
        }
        t
    }

    /// Schreibt hosts.txt atomar (0600).
    pub fn speichern(&self, pfad: &Path) -> Result<(), DateiFehler> {
        atomar_schreiben(pfad, self.text().as_bytes())
    }

    /// Der gepinnte Eintrag zu diesem Schluessel (egal unter welcher Adresse).
    pub fn nach_schluessel(&self, schluessel: &[u8]) -> Option<&BekannterHost> {
        self.hosts.iter().find(|h| h.schluessel[..] == *schluessel)
    }

    /// Der neueste Eintrag mit dieser ID (nur Anzeige und Suche: die ID
    /// liefert die letzte Adresse, vertraut wird dem Schluessel).
    pub fn nach_id(&self, id: u32) -> Option<&BekannterHost> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Der neueste Eintrag, der zuletzt unter dieser Adresse ("ip:port",
    /// Gross-/Kleinschreibung egal) erreicht wurde.
    pub fn nach_adresse(&self, adresse: &str) -> Option<&BekannterHost> {
        let a = adresse.trim();
        self.hosts.iter().find(|h| h.adresse.eq_ignore_ascii_case(a))
    }

    /// Pinnt einen Host (erst NACH der Annahme, Spezifikation 8.1): ein
    /// Eintrag mit demselben Schluessel wird ersetzt (neue Adresse, neuer
    /// Name), der neue steht vorn. Ein anderer Eintrag unter derselben
    /// Adresse bleibt fuer seine ID stehen, `nach_adresse` findet aber den
    /// neuen (8.3). false: Adresse leer oder mit Leerraum - nicht gepinnt.
    pub fn merken(&mut self, host: BekannterHost) -> bool {
        if host.adresse.is_empty() || host.adresse.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        let host = BekannterHost::neu(host.schluessel, &host.adresse, &host.name);
        self.hosts.retain(|h| h.schluessel != host.schluessel);
        self.hosts.insert(0, host);
        true
    }

    /// Vergisst einen Host. false: er stand nicht in der Liste.
    pub fn vergessen(&mut self, schluessel: &[u8]) -> bool {
        let vorher = self.hosts.len();
        self.hosts.retain(|h| h.schluessel[..] != *schluessel);
        self.hosts.len() != vorher
    }
}

/// Pinnt einen Host in hosts.txt: lesen, `merken`, atomar schreiben - in
/// einem Zug unter der Sperre dieses Prozesses.
pub fn host_merken(pfad: &Path, host: BekannterHost) -> Result<(), DateiFehler> {
    let _s = sperren(&HOSTS_SPERRE);
    let mut liste = Hostliste::laden(pfad)?;
    if !liste.merken(host.clone()) {
        return Err(DateiFehler::Schreiben {
            pfad: pfad.to_path_buf(),
            grund: format!("ungueltige Adresse \"{}\"", host.adresse),
        });
    }
    liste.speichern(pfad)
}

/// Vergisst einen Host in hosts.txt. Ok(false): er stand nicht darin.
pub fn host_vergessen(pfad: &Path, schluessel: &[u8]) -> Result<bool, DateiFehler> {
    let _s = sperren(&HOSTS_SPERRE);
    let mut liste = Hostliste::laden(pfad)?;
    if !liste.vergessen(schluessel) {
        return Ok(false);
    }
    liste.speichern(pfad)?;
    Ok(true)
}

/// Einmalige Uebernahme aus known_hosts.txt ("<host> <64 hex> <FP>", host
/// ohne Port): existiert sie im `ordner` und fehlt hosts.txt, wird jeder
/// Schluessel einmal uebernommen (die erste Zeile gewinnt), mit der ID aus
/// dem Schluessel, der Adresse "<host>:9001" (IPv6 in Klammern) und dem
/// alten Host als Namen; dann heisst known_hosts.txt known_hosts.txt.migriert.
/// Die Einstellungen je Host bleiben am Fingerabdruck (einstellungen.txt).
pub fn hosts_migrieren(ordner: &Path) -> Migration {
    let _s = sperren(&HOSTS_SPERRE);
    let alt = ordner.join(ALTE_HOSTS);
    let neu = ordner.join(HOSTS_DATEI);
    if vorhanden(&neu) || !vorhanden(&alt) {
        return Migration::Keine;
    }
    let text = match text_lesen(&alt, LISTE_GRENZE) {
        Ok(Some(t)) => t,
        Ok(None) => return Migration::Keine,
        Err(e) => return Migration::Fehler(e.to_string()),
    };
    let mut liste = Hostliste::default();
    // Aelteste Zeile zuerst: sie soll vorn stehen, `merken` stellt aber nach
    // vorn - also von hinten her merken, dann gewinnt die erste Zeile.
    let mut eintraege = Vec::new();
    for zeile in text.lines() {
        let (host, rest) = wort(zeile.trim());
        let (h, _) = wort(rest);
        let Some(schluessel) = hex_lesen(h) else { continue };
        if host.is_empty() || eintraege.iter().any(|(_, s)| *s == schluessel) {
            continue;
        }
        eintraege.push((host.to_string(), schluessel));
    }
    for (host, schluessel) in eintraege.iter().rev() {
        let adresse = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{STANDARD_PORT}")
        } else {
            format!("{host}:{STANDARD_PORT}")
        };
        liste.merken(BekannterHost::neu(*schluessel, &adresse, host));
    }
    if let Err(e) = liste.speichern(&neu) {
        return Migration::Fehler(e.to_string());
    }
    Migration::Uebernommen { anzahl: liste.hosts.len(), umbenannt: alt_umbenennen(&alt) }
}

// --------------------------------------------------------------- Datum

/// Ein Zeitpunkt in Kalenderform (lokal unter Windows, sonst UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zeitpunkt {
    pub jahr: i32,
    pub monat: u32,
    pub tag: u32,
    pub stunde: u32,
    pub minute: u32,
    pub sekunde: u32,
}

impl Zeitpunkt {
    /// Aus Sekunden seit 1970 (UTC), nach dem Kalenderverfahren von Howard
    /// Hinnant (days_from_civil umgekehrt) - ohne Zeitzonen.
    pub fn aus_unix(sekunden: i64) -> Zeitpunkt {
        let tage = sekunden.div_euclid(86_400);
        let rest = sekunden.rem_euclid(86_400);
        let z = tage + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let tag = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let monat = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        let jahr = (yoe + era * 400 + i64::from(monat <= 2)) as i32;
        Zeitpunkt {
            jahr,
            monat,
            tag,
            stunde: (rest / 3_600) as u32,
            minute: (rest / 60 % 60) as u32,
            sekunde: (rest % 60) as u32,
        }
    }

    /// "JJJJ-MM-TT" - das Datum in host-devices.txt.
    pub fn datum(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.jahr, self.monat, self.tag)
    }

    /// "JJJJMMTT-hhmmss" - fuer Dateinamen (.defekt-<stempel>), ohne ':'.
    pub fn stempel(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.jahr, self.monat, self.tag, self.stunde, self.minute, self.sekunde
        )
    }
}

/// Jetzt, in lokaler Zeit (Windows: GetLocalTime). Auf anderen Systemen
/// UTC - dort schreibt nur der Mac-Host (in C) Geraetelisten.
#[cfg(windows)]
pub fn jetzt_lokal() -> Zeitpunkt {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    Zeitpunkt {
        jahr: st.wYear as i32,
        monat: st.wMonth as u32,
        tag: st.wDay as u32,
        stunde: st.wHour as u32,
        minute: st.wMinute as u32,
        sekunde: st.wSecond as u32,
    }
}

#[cfg(not(windows))]
pub fn jetzt_lokal() -> Zeitpunkt {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    Zeitpunkt::aus_unix(s)
}

/// Das heutige Datum "JJJJ-MM-TT" (fuer geraet_eintragen, geraete_migrieren).
pub fn heute() -> String {
    jetzt_lokal().datum()
}

/// Tests, die den eingestellten Geraetenamen setzen oder sich auf seinen
/// Wert verlassen, laufen nacheinander - er gilt fuer den ganzen Prozess.
#[cfg(test)]
pub fn name_test_sperre() -> std::sync::MutexGuard<'static, ()> {
    static SPERRE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SPERRE.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// Pruefvektoren der Spezifikation 3.4: host_pub32 = 0x01..0x20,
    /// hh = 32 x 0xAA. Dieselben Werte prueft der Mac-Host (host/zugangtest).
    fn host_pub() -> [u8; 32] {
        std::array::from_fn(|i| i as u8 + 1)
    }
    const HH: [u8; 32] = [0xaa; 32];

    fn aus_hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Eigener leerer Ordner je Test (die Tests laufen nebeneinander), mit
    /// der Kennung des Laufs wie in secure.rs.
    fn ordner(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{}-zugang-{name}", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lesen(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[cfg(unix)]
    fn nur_eigentuemer(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600, "{}", p.display());
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 178, n))
    }

    // ------------------------------------------------------------ ID

    #[test]
    fn id_pruefvektor() {
        assert_eq!(geraete_id(&host_pub()), 581_729_911);
        assert_eq!(id_text(geraete_id(&host_pub())), "581 729 911");
        assert_eq!(id_ziffern(581_729_911), "581729911");
        // Weitere Werte, mit Python (hashlib) gegengerechnet.
        assert_eq!(geraete_id(&[0u8; 32]), 138_912_047);
        assert_eq!(geraete_id(&[0xffu8; 32]), 309_912_962);
    }

    #[test]
    fn id_anzeige_mit_nullen() {
        assert_eq!(id_text(5), "000 000 005");
        assert_eq!(id_text(0), "000 000 000");
        assert_eq!(id_text(999_999_999), "999 999 999");
        assert_eq!(id_text(12_345_678), "012 345 678");
        assert_eq!(id_ziffern(5), "000000005");
        // Nie mehr als 9 Stellen, auch bei einem Wert ausserhalb.
        assert_eq!(id_text(1_000_000_005), "000 000 005");
        for k in 0..200u8 {
            assert!(geraete_id(&[k; 32]) < ID_GRENZE);
        }
    }

    #[test]
    fn id_eingabe() {
        for t in ["581729911", "581 729 911", "581-729-911", "  581 729 911 ", "581  729\t911", "58-17-29-91-1"] {
            assert_eq!(id_lesen(t), Some(581_729_911), "{t:?}");
        }
        assert_eq!(id_lesen("000000005"), Some(5));
        assert_eq!(id_lesen("000 000 005"), Some(5));
        for t in [
            "",
            "58172991",
            "5817299111",
            "581.729.911",
            "581 729 91a",
            "abc",
            "192.168.1.5",
            "studio.local",
            "581729911:9001",
            "٥٨١٧٢٩٩١١",
            "５８１７２９９１１",
        ] {
            assert_eq!(id_lesen(t), None, "{t:?}");
        }
        // Hin und zurueck fuer jede Anzeige.
        for id in [0, 5, 581_729_911, 999_999_999] {
            assert_eq!(id_lesen(&id_text(id)), Some(id));
            assert_eq!(id_lesen(&id_ziffern(id)), Some(id));
        }
    }

    // ------------------------------------------------------------ Name

    #[test]
    fn name_utf8_sicher_gekuerzt() {
        assert_eq!(name_kuerzen("kurz", 40), "kurz");
        let a = "a".repeat(50);
        assert_eq!(name_kuerzen(&a, 40).len(), 40);
        // Der Umlaut hat 2 Byte: 1 + 20*2 = 41 Byte -> 39 Byte, nie mitten im Zeichen.
        let s = format!("a{}", "ä".repeat(20));
        assert_eq!(name_kuerzen(&s, 40), format!("a{}", "ä".repeat(19)));
        // 4-Byte-Zeichen: 10 * 4 = 40 passt genau, 11 nicht.
        assert_eq!(name_kuerzen(&"😀".repeat(10), 40), "😀".repeat(10));
        assert_eq!(name_kuerzen(&"😀".repeat(11), 40), "😀".repeat(10));
        assert_eq!(name_kuerzen(&format!("abc{}", "😀".repeat(10)), 40), format!("abc{}", "😀".repeat(9)));
        for n in 0..=45 {
            let t = format!("{}{}", "x".repeat(n % 3), "€".repeat(n));
            let k = name_kuerzen(&t, 40);
            assert!(k.len() <= 40 && t.starts_with(k));
        }
    }

    #[test]
    fn name_bereinigt() {
        assert_eq!(name_bereinigen("  Roberts Mac mini  "), "Roberts Mac mini");
        assert_eq!(name_bereinigen("a\nb\tc\u{7f}"), "a?b?c?");
        // Richtungszeichen koennten einen Namen im Zulassen-Fenster umdrehen.
        assert_eq!(name_bereinigen("PC\u{202e}gnp.exe"), "PC?gnp.exe");
        assert_eq!(name_bereinigen("\u{2066}x\u{2069}"), "?x?");
        assert_eq!(name_bereinigen("Grüße aus Köln"), "Grüße aus Köln");
        let lang = name_bereinigen(&"Ü".repeat(30));
        assert_eq!(lang, "Ü".repeat(20));
        // Kein Leerraum am Ende nach dem Kuerzen.
        assert_eq!(name_bereinigen(&format!("{} b", "a".repeat(39))), "a".repeat(39));
        assert_eq!(name_bereinigen(""), "");
    }

    #[test]
    fn eigener_geraetename() {
        let n = rechnername();
        assert!(!n.is_empty() && n.len() <= NAME_MAX, "{n:?}");
        assert!(!n.chars().any(gefaehrlich), "{n:?}");
        assert_eq!(n, name_bereinigen(&n));
    }

    /// Die Eingabe im Fenster "Geraetename": getrimmt; leer (auch nur
    /// Leerraum) heisst Rechnername; 1 bis 40 Byte UTF-8 - gezaehlt in Byte,
    /// nicht in Zeichen; Steuer- und Richtungszeichen nie, auch nicht innen.
    #[test]
    fn geraetename_eingabe() {
        assert_eq!(geraetename_pruefen("  Büro-PC \t"), Ok(Some("Büro-PC".into())));
        assert_eq!(geraetename_pruefen(""), Ok(None));
        assert_eq!(geraetename_pruefen(" \t\n "), Ok(None));
        assert_eq!(geraetename_pruefen("x"), Ok(Some("x".into())));
        assert_eq!(geraetename_pruefen(&"a".repeat(40)), Ok(Some("a".repeat(40))));
        assert_eq!(geraetename_pruefen(&"a".repeat(41)), Err(NameFehler::ZuLang));
        // 20 Zeichen, aber 40 bzw. 42 Byte.
        assert_eq!(geraetename_pruefen(&"ß".repeat(20)), Ok(Some("ß".repeat(20))));
        assert_eq!(geraetename_pruefen(&format!("{}ab", "ß".repeat(20))), Err(NameFehler::ZuLang));
        // Aussen getrimmt zaehlt nicht mit.
        assert_eq!(geraetename_pruefen(&format!("  {}  ", "a".repeat(40))), Ok(Some("a".repeat(40))));
        for boese in ["a\u{7}b", "Zeile\nzwei", "Tab\tinnen", "rechts\u{202e}links", "x\u{2066}y", "\u{200f}PC"] {
            assert_eq!(geraetename_pruefen(boese), Err(NameFehler::Zeichen), "{boese:?}");
        }
        // Was angenommen wird, aendert name_bereinigen nicht mehr.
        for gut in ["Büro-PC", "Roberts Mac mini", "PC {i} {n}", "日本語のパソコン"] {
            let n = geraetename_pruefen(gut).unwrap().unwrap();
            assert_eq!(n, name_bereinigen(&n));
        }
    }

    /// Der eingestellte Name gilt sofort fuer geraetename(); None und ein
    /// ungueltiger Wert fuehren zum Rechnernamen zurueck. Haelt die Sperre
    /// der Namenstests (der Wert gilt fuer den ganzen Prozess).
    #[test]
    fn geraetename_eingestellt_gilt_sofort() {
        let _s = name_test_sperre();
        let vorher = geraetename_eingestellt();
        geraetename_setzen(Some("Wohnzimmer".into()));
        assert_eq!(geraetename(), "Wohnzimmer");
        geraetename_setzen(Some("  Büro  ".into()));
        assert_eq!(geraetename(), "Büro");
        geraetename_setzen(Some("a\u{7}".into()));
        assert_eq!(geraetename(), rechnername());
        assert_eq!(geraetename_eingestellt(), None);
        geraetename_setzen(Some("x".repeat(41)));
        assert_eq!(geraetename(), rechnername());
        geraetename_setzen(None);
        assert_eq!(geraetename(), rechnername());
        geraetename_setzen(vorher);
    }

    // ------------------------------------------------------ Nachricht 3

    #[test]
    fn nachricht3_name_hin_und_zurueck() {
        let n = nachricht3("Roberts PC", 0);
        assert_eq!(&n[..5], b"QCN1\x0a");
        assert_eq!(&n[5..], b"Roberts PC\x00");
        assert_eq!(nachricht3_name(&n).as_deref(), Some("Roberts PC"));
        assert_eq!(nachricht3_flags(&n), 0);
        // Zu lang: auf 40 Byte gekuerzt, UTF-8-sicher.
        let n = nachricht3(&"ß".repeat(30), NAME_FLAG_HOST_UNBEKANNT);
        assert_eq!(n[4], 40);
        assert_eq!(n.len(), 46);
        assert_eq!(nachricht3_name(&n), Some("ß".repeat(20)));
        assert_eq!(nachricht3_flags(&n), NAME_FLAG_HOST_UNBEKANNT);
        // Spaetere Felder hinter den Flags werden uebergangen.
        let mut n = nachricht3("PC", NAME_FLAG_HOST_UNBEKANNT);
        n.extend_from_slice(b"\x01\x02\x03");
        assert_eq!(nachricht3_name(&n).as_deref(), Some("PC"));
        assert_eq!(nachricht3_flags(&n), NAME_FLAG_HOST_UNBEKANNT);
        // Steuerzeichen kommen nicht durch.
        let mut n = b"QCN1\x03".to_vec();
        n.extend_from_slice(b"a\nb");
        assert_eq!(nachricht3_name(&n).as_deref(), Some("a?b"));
    }

    #[test]
    fn nachricht3_alt_oder_kaputt() {
        let mut ueber40 = b"QCN1\x29".to_vec();
        ueber40.extend_from_slice(&[b'a'; 41]);
        for n in [
            &b"client"[..],
            b"",
            b"QCN1",
            b"QCN1\x00",
            b"QCN1\x05abc",
            b"QCN2\x02ab",
            b"qcn1\x02ab",
            b"QCN1\x02\xff\xfe",
            b"QCN1\x02  ",
            &ueber40,
        ] {
            assert_eq!(nachricht3_name(n), None, "{n:?}");
        }
    }

    /// Bit 0 hinter dem Namen: "der Client kennt den Schluessel des Hosts
    /// nicht". Die alte Form ohne Flag-Byte (und b"client") heisst 0,
    /// unbekannte Bits und Bytes dahinter werden uebergangen; ein kaputter
    /// Aufbau heisst 0, ein unbrauchbarer Name allein nicht.
    #[test]
    fn nachricht3_flags_lesen() {
        let f = NAME_FLAG_HOST_UNBEKANNT;
        assert_eq!(f, 1);
        assert_eq!(nachricht3("Laptop", 0).last(), Some(&0));
        assert_eq!(nachricht3("Laptop", f).last(), Some(&1));
        // Nur Bit 0 geht hinaus, nur Bit 0 wird gelesen.
        assert_eq!(nachricht3("Laptop", 0xff).last(), Some(&1));
        assert_eq!(nachricht3_flags(b"QCN1\x02PC\xff"), f);
        assert_eq!(nachricht3_flags(b"QCN1\x02PC\xfe"), 0);
        // Alte Form: ohne Flag-Byte.
        assert_eq!(nachricht3_flags(b"QCN1\x02PC"), 0);
        assert_eq!(nachricht3_name(b"QCN1\x02PC").as_deref(), Some("PC"));
        assert_eq!(nachricht3_flags(b"client"), 0);
        assert_eq!(nachricht3_flags(b""), 0);
        // Bytes hinter den Flags: uebergangen.
        assert_eq!(nachricht3_flags(b"QCN1\x02PC\x01zukunft"), f);
        // Der Name taugt nicht (leer, kein UTF-8), der Aufbau schon: die
        // Flags gelten - der Host zeigt dann die Adresse.
        assert_eq!(nachricht3_flags(b"QCN1\x00\x01"), f);
        assert_eq!(nachricht3_flags(b"QCN1\x02\xff\xfe\x01"), f);
        assert_eq!(nachricht3_name(b"QCN1\x02\xff\xfe\x01"), None);
        // Kaputter Aufbau: 0.
        let mut ueber40 = b"QCN1\x29".to_vec();
        ueber40.extend_from_slice(&[b'a'; 41]);
        ueber40.push(1);
        for n in [&b"QCN1"[..], b"QCN1\x05abc\x01", b"QCN2\x02ab\x01", b"qcn1\x02ab\x01", &ueber40] {
            assert_eq!(nachricht3_flags(n), 0, "{n:?}");
        }
    }

    // ------------------------------------------------------ Bekanntgabe

    #[test]
    fn bekanntgabe_hin_und_zurueck() {
        let p = bekanntgabe(9001, "Roberts Mac mini", 581_729_911, BEACON_FLAG_ZULASSEN);
        assert_eq!(&p[..8], b"QCHB\x01\x29\x23\x10");
        assert_eq!(p.len(), 8 + 16 + 6);
        let b = bekanntgabe_lesen(&p).unwrap();
        assert_eq!(b, Bekanntgabe { port: 9001, name: "Roberts Mac mini".into(), id: Some(581_729_911), flags: 1 });
        assert!(b.zulassen_moeglich());
        let b = bekanntgabe_lesen(&bekanntgabe(9101, "PC", 5, 0)).unwrap();
        assert_eq!((b.port, b.id, b.flags, b.zulassen_moeglich()), (9101, Some(5), 0, false));
        // Bit 1-7 gehen nie hinaus.
        assert_eq!(*bekanntgabe(9001, "PC", 5, 0xff).last().unwrap(), 1);
        // Laengster Name: 40 Byte, das Paket bleibt weit unter 128 Byte.
        let p = bekanntgabe(9001, &"x".repeat(100), 999_999_999, 1);
        assert_eq!(p.len(), 54);
        assert!(p.len() <= BEKANNTGABE_MAX);
        assert_eq!(bekanntgabe_lesen(&p).unwrap().name, "x".repeat(40));
    }

    /// Der alte Empfaenger (discovery.rs) liest das neue Paket unveraendert:
    /// Kennung, Fassung 1, Port, Name - der Anhang liegt hinter dem Namen.
    #[test]
    fn bekanntgabe_fuer_alte_clients_lesbar() {
        let p = bekanntgabe(9001, "Studio", 42, 1);
        assert!(p.len() >= 8 && &p[0..4] == b"QCHB" && p[4] == 1);
        let nlen = p[7] as usize;
        assert!(p.len() >= 8 + nlen);
        assert_eq!(u16::from_le_bytes([p[5], p[6]]), 9001);
        assert_eq!(String::from_utf8_lossy(&p[8..8 + nlen]), "Studio");
        // Und das alte Paket (ohne Anhang) liest der neue Empfaenger: ID "-".
        let alt = [&b"QCHB\x01\x29\x23\x06"[..], b"Studio"].concat();
        assert_eq!(
            bekanntgabe_lesen(&alt),
            Some(Bekanntgabe { port: 9001, name: "Studio".into(), id: None, flags: 0 })
        );
    }

    #[test]
    fn bekanntgabe_kaputt() {
        let gut = bekanntgabe(9001, "PC", 7, 1);
        // Anhang kaputt oder unbekannt: Paket gilt, ID fehlt.
        for ext in [&[][..], &[1u8, 7, 0, 0][..], &[2u8, 7, 0, 0, 0, 1][..], &[1u8, 0x00, 0xca, 0x9a, 0x3b, 1][..]] {
            let p = [&gut[..10], ext].concat();
            let b = bekanntgabe_lesen(&p).unwrap();
            assert_eq!((b.id, b.flags, b.zulassen_moeglich()), (None, 0, false), "{ext:?}");
        }
        // Mehr Bytes hinter dem Anhang schaden nicht.
        let p = [&gut[..], &[9, 9, 9]].concat();
        assert_eq!(bekanntgabe_lesen(&p).unwrap().id, Some(7));
        // Keine Bekanntgabe.
        for p in
            [&b""[..], b"QCHB\x01\x29", b"QCHX\x01\x29\x23\x00", b"QCHB\x02\x29\x23\x00", b"QCHB\x01\x29\x23\x05abc"]
        {
            assert_eq!(bekanntgabe_lesen(p), None, "{p:?}");
        }
    }

    // ------------------------------------------------- Zugangsnachrichten

    fn alle_nachrichten() -> Vec<Nachricht> {
        vec![
            Nachricht::Noetig(ZugangNoetig::neu(true, 5000, "Roberts Mac mini")),
            Nachricht::Noetig(ZugangNoetig::neu(false, 0, "")),
            Nachricht::Beweis([7; 32]),
            Nachricht::Ergebnis(Ergebnis::Passwort { host_beweis: [9; 32] }),
            Nachricht::Ergebnis(Ergebnis::Zulassen),
            Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms: 10_000 }),
            Nachricht::Ergebnis(Ergebnis::Abgelehnt),
            Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 5000 }),
            Nachricht::Abbruch,
        ]
    }

    /// Liest aus einem Puffer wie eine Leitung; am Ende "Leitung zu".
    fn leitung(daten: &[u8]) -> impl FnMut(&mut [u8]) -> Result<(), String> + '_ {
        let mut pos = 0;
        move |b: &mut [u8]| {
            if pos + b.len() > daten.len() {
                return Err("Leitung zu".into());
            }
            b.copy_from_slice(&daten[pos..pos + b.len()]);
            pos += b.len();
            Ok(())
        }
    }

    #[test]
    fn nachrichten_hin_und_zurueck() {
        for n in alle_nachrichten() {
            let b = n.kodieren();
            assert_eq!(b[0], n.typ());
            assert_eq!(&b[1..4], &[0, 0, 0], "Flags und frei");
            assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize, b.len() - 8);
            assert_eq!(empfangen(leitung(&b)).unwrap(), n);
        }
        // Mehrere hintereinander auf einer Leitung.
        let alles: Vec<u8> = alle_nachrichten().iter().flat_map(|n| n.kodieren()).collect();
        let mut l = leitung(&alles);
        for n in alle_nachrichten() {
            assert_eq!(empfangen(&mut l).unwrap(), n);
        }
        assert!(matches!(empfangen(&mut l), Err(LeseFehler::Leitung(_))));
    }

    #[test]
    fn nachrichten_bytegenau() {
        let n = Nachricht::Noetig(ZugangNoetig::neu(true, 0x0102_0304, "Mac")).kodieren();
        assert_eq!(n, [&[20u8, 0, 0, 0, 11, 0, 0, 0, 1, 3, 0, 0, 4, 3, 2, 1][..], b"Mac"].concat());
        let n = Nachricht::Ergebnis(Ergebnis::Falsch { warten_ms: 5000 }).kodieren();
        assert_eq!(n, [22, 0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0, 0x88, 0x13, 0, 0]);
        let n = Nachricht::Ergebnis(Ergebnis::Passwort { host_beweis: [0xee; 32] }).kodieren();
        assert_eq!(n.len(), 48);
        assert_eq!(&n[..16], &[22, 0, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(Nachricht::Beweis([1; 32]).kodieren()[..8], [21, 0, 0, 0, 32, 0, 0, 0]);
        assert_eq!(Nachricht::Abbruch.kodieren(), [23, 0, 0, 0, 0, 0, 0, 0]);
        // Wege: Passwort immer, Zulassen nur mit Oberflaeche.
        let z = ZugangNoetig::neu(false, 0, "x");
        assert!(z.passwort_moeglich() && !z.zulassen_moeglich());
        assert!(ZugangNoetig::neu(true, 0, "x").zulassen_moeglich());
        // Der Hostname wird hoechstens 40 Byte lang.
        let n = Nachricht::Noetig(ZugangNoetig { wege: 3, warten_ms: 0, hostname: "h".repeat(60) }).kodieren();
        assert_eq!(n.len(), 8 + 8 + 40);
        match empfangen(leitung(&n)).unwrap() {
            Nachricht::Noetig(z) => assert_eq!(z.hostname, "h".repeat(40)),
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn kopf_falsch() {
        let k = |typ: u8, l: u32| {
            let b = l.to_le_bytes();
            [typ, 0, 0, 0, b[0], b[1], b[2], b[3]]
        };
        for t in [0u8, 1, 2, 16, 19, 24, 48, 255] {
            assert_eq!(kopf_lesen(&k(t, 0)), Err(LeseFehler::Typ(t)));
        }
        for (t, l) in [
            (20u8, 7u32),
            (20, 1025),
            (20, u32::MAX),
            (21, 31),
            (21, 33),
            (21, 0),
            (22, 0),
            (22, 9),
            (22, 39),
            (22, 41),
            (23, 1),
        ] {
            assert_eq!(kopf_lesen(&k(t, l)), Err(LeseFehler::Laenge { typ: t, laenge: l as usize }), "{t} {l}");
        }
        for (t, l) in [(20u8, 8u32), (20, 48), (21, 32), (22, 8), (22, 40), (23, 0)] {
            assert_eq!(kopf_lesen(&k(t, l)), Ok((t, l as usize)));
        }
        // Flags und das freie Feld stoeren nicht.
        assert_eq!(kopf_lesen(&[23, 0xff, 0xff, 0xff, 0, 0, 0, 0]), Ok((23, 0)));
    }

    #[test]
    fn nutzlast_falsch() {
        // Falsche Fassung - auch wenn die Nachricht laenger ist (spaetere Fassung).
        let mut n = vec![2u8, 1, 0, 0, 0, 0, 0, 0];
        assert_eq!(nachricht_lesen(20, &n), Err(LeseFehler::Fassung(2)));
        n.extend_from_slice(&[b'x'; 100]);
        assert_eq!(nachricht_lesen(20, &n), Err(LeseFehler::Fassung(2)));
        // Fassung 1 mit Hostname ueber 40 Byte.
        n[0] = 1;
        assert_eq!(nachricht_lesen(20, &n), Err(LeseFehler::Laenge { typ: 20, laenge: 108 }));
        assert_eq!(nachricht_lesen(20, &[1, 1, 0, 0]), Err(LeseFehler::Laenge { typ: 20, laenge: 4 }));
        // Ergebnis 0 ohne Beweis, Ergebnis 1-4 mit Beweisbytes.
        assert_eq!(nachricht_lesen(22, &[0; 8]), Err(LeseFehler::Laenge { typ: 22, laenge: 8 }));
        for c in 1..=4u8 {
            let mut b = [0u8; 40];
            b[0] = c;
            assert_eq!(nachricht_lesen(22, &b), Err(LeseFehler::Laenge { typ: 22, laenge: 40 }), "{c}");
        }
        // Unbekanntes Ergebnis.
        assert_eq!(nachricht_lesen(22, &[5, 0, 0, 0, 0, 0, 0, 0]), Err(LeseFehler::Ergebniscode(5)));
        assert_eq!(nachricht_lesen(22, &[0xff; 40]), Err(LeseFehler::Ergebniscode(0xff)));
        // Beweis mit falscher Laenge, Abbruch mit Nutzlast, fremder Typ.
        assert_eq!(nachricht_lesen(21, &[0; 31]), Err(LeseFehler::Laenge { typ: 21, laenge: 31 }));
        assert_eq!(nachricht_lesen(23, &[0]), Err(LeseFehler::Laenge { typ: 23, laenge: 1 }));
        assert_eq!(nachricht_lesen(2, &[]), Err(LeseFehler::Typ(2)));
        // Warten wird bei 2 und 4 gelesen, bei den anderen uebergangen.
        let e = nachricht_lesen(22, &[3, 0, 0, 0, 1, 0, 0, 0]).unwrap();
        assert_eq!(e, Nachricht::Ergebnis(Ergebnis::Abgelehnt));
        let e = nachricht_lesen(22, &[4, 9, 9, 9, 0x10, 0x27, 0, 0]).unwrap();
        assert_eq!(e, Nachricht::Ergebnis(Ergebnis::Schluss { warten_ms: 10_000 }));
        // Kaputter Hostname wird lesbar gemacht, nicht abgelehnt.
        let n = [&[1u8, 3, 0, 0, 0, 0, 0, 0][..], b"a\xffb\n"].concat();
        match nachricht_lesen(20, &n).unwrap() {
            Nachricht::Noetig(z) => assert_eq!(z.hostname, "a\u{fffd}b?"),
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn abgeschnitten() {
        for n in alle_nachrichten() {
            let b = n.kodieren();
            for l in 0..b.len() {
                assert!(matches!(empfangen(leitung(&b[..l])), Err(LeseFehler::Leitung(_))), "{n:?} bei {l}");
            }
        }
        // Ein Kopf mit riesiger Laenge legt keinen riesigen Puffer an.
        let b = [20u8, 0, 0, 0, 0xff, 0xff, 0xff, 0x7f];
        assert!(matches!(empfangen(leitung(&b)), Err(LeseFehler::Laenge { .. })));
    }

    // ------------------------------------------------------- Kryptografie

    #[test]
    fn hmac_rfc4231() {
        // Testfall 1
        assert_eq!(
            hmac_sha256(&[0x0b; 20], b"Hi There").to_vec(),
            aus_hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );
        // Testfall 2
        assert_eq!(
            hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
            aus_hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // Testfall 6: Schluessel laenger als ein Block (wird erst gehasht).
        assert_eq!(
            hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First").to_vec(),
            aus_hex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
        );
    }

    #[test]
    fn pbkdf2_rfc_vektoren() {
        // RFC 7914, Abschnitt 11 (PBKDF2-HMAC-SHA256, 64 Byte = zwei Bloecke).
        let mut aus = [0u8; 64];
        pbkdf2_sha256(b"passwd", b"salt", 1, &mut aus);
        assert_eq!(
            aus.to_vec(),
            aus_hex(
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
                 49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
            )
        );
        // Bekannte Werte fuer "password"/"salt" (Python hashlib gegengerechnet).
        for (runden, soll) in [
            (1, "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"),
            (2, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"),
            (4096, "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"),
        ] {
            let mut aus = [0u8; 32];
            pbkdf2_sha256(b"password", b"salt", runden, &mut aus);
            assert_eq!(aus.to_vec(), aus_hex(soll), "{runden}");
        }
        // Laenge, die kein Vielfaches von 32 ist (zweiter Block angeschnitten).
        let mut aus = [0u8; 40];
        pbkdf2_sha256(b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, &mut aus);
        assert_eq!(
            aus.to_vec(),
            aus_hex("348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9")
        );
    }

    #[test]
    fn norm_pruefvektoren() {
        assert_eq!(passwort_norm("K7M 4WQ-9TZ"), b"k7m4wq9tz");
        assert_eq!(passwort_norm("k7m-4wq-9tz"), b"k7m4wq9tz");
        assert_eq!(passwort_norm("Grüße, Mac!"), b"gr\xc3\xbc\xc3\x9fe,mac!");
        assert_eq!(passwort_norm(" a\tb\nc\rd-e "), b"abcde");
        // Kein Unicode-Kleinschreiben, keine Normalisierung.
        assert_eq!(passwort_norm("ÄÖÜ"), "ÄÖÜ".as_bytes());
        assert_eq!(passwort_norm("e\u{301}"), "e\u{301}".as_bytes());
        assert_eq!(passwort_norm("\u{a0}"), "\u{a0}".as_bytes());
        assert_eq!(passwort_norm("AZaz09@[`{"), b"azaz09@[`{");
    }

    #[test]
    fn schluessel_und_beweise_pruefvektoren() {
        let p = host_pub();
        let k = passwort_schluessel("k7m-4wq-9tz", &p);
        assert_eq!(k.to_vec(), aus_hex("0d49cfceed95c9e251f41acead7e619c3983ab763249052d022a2efe03149842"));
        assert_eq!(passwort_schluessel("K7M 4WQ-9TZ", &p), k);
        assert_eq!(
            passwort_schluessel("Grüße, Mac!", &p).to_vec(),
            aus_hex("bb389a45a9dc1f7217d3e8da30cc95e2912ce4a1a8465ad7dd5d6c0013e74dc0")
        );
        let c = client_beweis(&k, &HH);
        let h = host_beweis(&k, &HH);
        assert_eq!(c.to_vec(), aus_hex("647398d505263b3af01ba1a04d74aeb467a774588902489bca97c8d3544e3fc0"));
        assert_eq!(h.to_vec(), aus_hex("2f9d511fa97017f1d5b60806aa5ff4d781766e386b92f79d77391e4090cef55d"));
        assert!(beweis_pruefen(&k, &HH, &c));
        assert!(host_beweis_pruefen(&k, &HH, &h));
        // Die beiden Richtungen sind nicht vertauschbar.
        assert!(!beweis_pruefen(&k, &HH, &h));
        assert!(!host_beweis_pruefen(&k, &HH, &c));
        // Anderer Handschlag, anderer Beweis (kanalgebunden).
        assert!(!beweis_pruefen(&k, &[0xab; 32], &c));
        // Anderer Host-Schluessel: anderes K.
        let mut q = p;
        q[31] ^= 1;
        assert_ne!(passwort_schluessel("k7m-4wq-9tz", &q), k);
        // Anderes Passwort: anderes K.
        assert_ne!(passwort_schluessel("k7m-4wq-9ty", &p), k);
    }

    #[test]
    fn vergleich_konstant() {
        assert!(gleich(b"", b""));
        assert!(gleich(&[1, 2, 3], &[1, 2, 3]));
        assert!(!gleich(&[1, 2, 3], &[1, 2, 4]));
        assert!(!gleich(&[0x80, 2, 3], &[0, 2, 3]));
        assert!(!gleich(&[1, 2, 3], &[1, 2]));
        assert!(!gleich(&[0; 32], &[0; 33]));
        let a = [0x5a; 32];
        for i in 0..32 {
            for bit in 0..8 {
                let mut b = a;
                b[i] ^= 1 << bit;
                assert!(!gleich(&a, &b));
            }
        }
    }

    #[test]
    fn cache_rechnet_nur_bei_aenderung() {
        let p = host_pub();
        let mut c = Schluesselcache::default();
        let k = c.schluessel("k7m-4wq-9tz", &p);
        assert_eq!(k, passwort_schluessel("k7m-4wq-9tz", &p));
        // Gleiches norm(pw): der Wert kommt aus dem Cache, ohne neue Rechnung
        // (ein untergeschobener Wert kommt unveraendert zurueck).
        c.eintrag = Some((passwort_norm("k7m-4wq-9tz"), p.to_vec(), [0x42; 32]));
        assert_eq!(c.schluessel("K7M 4WQ 9TZ", &p), [0x42; 32]);
        c.eintrag = Some((passwort_norm("k7m-4wq-9tz"), p.to_vec(), k));
        // Anderes Passwort oder anderer Host-Schluessel: neu gerechnet.
        let k2 = c.schluessel("anderes-passwort", &p);
        assert_eq!(k2, passwort_schluessel("anderes-passwort", &p));
        assert_ne!(k2, k);
        assert_eq!(c.schluessel("k7m-4wq-9tz", &[7; 32]), passwort_schluessel("k7m-4wq-9tz", &[7; 32]));
        c.verwerfen();
        assert_eq!(c.schluessel("k7m-4wq-9tz", &p), k);
    }

    // ------------------------------------------------------------ Drossel

    #[test]
    fn staffel() {
        let s: Vec<u64> = (0..=12).map(|f| wartezeit(f).as_secs()).collect();
        assert_eq!(s, [0, 0, 0, 5, 10, 20, 40, 80, 160, 300, 300, 300, 300]);
        assert_eq!(wartezeit(u32::MAX), DROSSEL_HOECHSTENS);
        assert_eq!(millis(Duration::from_secs(5)), 5000);
        assert_eq!(millis(Duration::from_nanos(4_999_000_001)), 5000);
        assert_eq!(millis(Duration::ZERO), 0);
        assert_eq!(millis(Duration::from_secs(u64::MAX)), u32::MAX);
    }

    #[test]
    fn drossel_je_ip_und_schluessel() {
        let t0 = Instant::now();
        let s = |x: u8| [x; 32];
        let mut d = Drossel::neu();
        assert_eq!(d.warten(ip(1), &s(1), t0), Duration::ZERO);
        // Zwei Fehlversuche kosten nichts, der dritte 5 s.
        assert_eq!(d.fehlversuch(ip(1), &s(1), t0), Duration::ZERO);
        assert_eq!(d.fehlversuch(ip(1), &s(2), t0), Duration::ZERO);
        assert_eq!(d.fehlversuch(ip(1), &s(3), t0), Duration::from_secs(5));
        // Die IP hat drei, jeder Schluessel einen: der groessere Wert gilt,
        // auch fuer einen ganz neuen Schluessel von dieser IP.
        assert_eq!(d.warten(ip(1), &s(9), t0), Duration::from_secs(5));
        assert_eq!(d.warten(ip(2), &s(1), t0), Duration::ZERO);
        assert_eq!(d.fehlversuche(ip(1), &s(9), t0), 3);
        // Derselbe Schluessel von wechselnden IPs wird ebenso gebremst.
        for n in 10..12 {
            d.fehlversuch(ip(n), &s(7), t0);
        }
        assert_eq!(d.fehlversuch(ip(12), &s(7), t0), Duration::from_secs(5));
        assert_eq!(d.warten(ip(13), &s(7), t0), Duration::from_secs(5));
        // Die Wartezeit laeuft ab.
        assert_eq!(d.warten(ip(1), &s(9), t0 + Duration::from_secs(3)), Duration::from_secs(2));
        assert_eq!(d.warten(ip(1), &s(9), t0 + Duration::from_secs(5)), Duration::ZERO);
        // Verdoppelt bis 300 s.
        let mut w = Duration::ZERO;
        for i in 0..10 {
            w = d.fehlversuch(ip(1), &s(1), t0 + Duration::from_secs(i));
        }
        assert_eq!(d.fehlversuche(ip(1), &s(1), t0 + Duration::from_secs(9)), 13);
        assert_eq!(w, Duration::from_secs(300));
        // Richtiger Beweis: beide Zaehler dieser Gegenstelle zurueck.
        d.erfolg(ip(1), &s(1));
        assert_eq!(d.warten(ip(1), &s(1), t0 + Duration::from_secs(10)), Duration::ZERO);
        assert_eq!(d.fehlversuche(ip(1), &s(1), t0), 0);
        // ... der Schluessel 7 bleibt gebremst (anderer Zaehler).
        assert_eq!(d.fehlversuche(ip(13), &s(7), t0), 3);
    }

    #[test]
    fn drossel_verfaellt_nach_15_minuten() {
        let t0 = Instant::now();
        let mut d = Drossel::neu();
        for _ in 0..8 {
            d.fehlversuch(ip(1), &[1; 32], t0);
        }
        assert_eq!(d.warten(ip(1), &[1; 32], t0), Duration::from_secs(160));
        let spaeter = t0 + DROSSEL_VERFALL - Duration::from_secs(1);
        assert_eq!(d.fehlversuche(ip(1), &[1; 32], spaeter), 8);
        let t1 = t0 + DROSSEL_VERFALL;
        assert_eq!(d.fehlversuche(ip(1), &[1; 32], t1), 0);
        assert_eq!(d.warten(ip(1), &[1; 32], t1), Duration::ZERO);
        // Nach dem Verfall zaehlt es von vorn.
        assert_eq!(d.fehlversuch(ip(1), &[1; 32], t1), Duration::ZERO);
        assert_eq!(d.fehlversuche(ip(1), &[1; 32], t1), 1);
        // Der Verfall rechnet ab dem LETZTEN Fehlversuch.
        let t2 = t1 + Duration::from_secs(600);
        d.fehlversuch(ip(1), &[1; 32], t2);
        assert_eq!(d.fehlversuche(ip(1), &[1; 32], t1 + DROSSEL_VERFALL), 2);
    }

    #[test]
    fn drossel_global() {
        let t0 = Instant::now();
        let mut d = Drossel::neu();
        // 30 Fehlversuche in 60 s von 30 Adressen: noch keine Sperre.
        for i in 0..30u8 {
            d.fehlversuch(ip(i), &[i; 32], t0 + Duration::from_secs(i as u64));
        }
        let t = t0 + Duration::from_secs(30);
        assert!(!d.global_gesperrt(t));
        assert_eq!(d.warten(ip(200), &[200; 32], t), Duration::ZERO);
        // Der 31.: jede weitere Zugangsphase wartet 60 s.
        d.fehlversuch(ip(31), &[31; 32], t);
        assert!(d.global_gesperrt(t));
        assert_eq!(d.warten(ip(200), &[200; 32], t), GLOBAL_WARTEN);
        assert_eq!(d.phase_beginnen(ip(200), &[200; 32], t).warten_ms(t), 60_000);
        // Faellt der aelteste aus dem Fenster, endet die Sperre.
        let t = t0 + GLOBAL_FENSTER;
        assert!(!d.global_gesperrt(t));
        assert_eq!(d.warten(ip(200), &[200; 32], t), Duration::ZERO);
    }

    #[test]
    fn drossel_tabellen_begrenzt() {
        let t0 = Instant::now();
        let mut d = Drossel::neu();
        for i in 0..(DROSSEL_EINTRAEGE as u32 + 500) {
            let a = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i));
            d.fehlversuch(a, &i.to_le_bytes(), t0 + Duration::from_millis(i as u64));
        }
        assert!(d.je_ip.len() <= DROSSEL_EINTRAEGE && d.je_schluessel.len() <= DROSSEL_EINTRAEGE);
        // Verdraengt wurden die aeltesten, der neueste steht.
        let neu = DROSSEL_EINTRAEGE as u32 + 499;
        assert_eq!(d.fehlversuche(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + neu)), &neu.to_le_bytes(), t0), 1);
        assert_eq!(d.fehlversuche(IpAddr::V4(Ipv4Addr::from(0x0a00_0000)), &0u32.to_le_bytes(), t0), 0);
    }

    #[test]
    fn phase_ablauf() {
        let t0 = Instant::now();
        let k = [4u8; 32];
        let mut d = Drossel::neu();
        let mut p = d.phase_beginnen(ip(1), &k, t0);
        assert_eq!(p.warten_ms(t0), 0);
        assert!(!p.zu_frueh(t0));
        // Falsch, falsch: ohne Wartezeit; der dritte kostet 5 s.
        assert_eq!(d.beweis_werten(&mut p, ip(1), &k, false, t0), Wertung::Falsch { warten_ms: 0 });
        assert_eq!(d.beweis_werten(&mut p, ip(1), &k, false, t0), Wertung::Falsch { warten_ms: 0 });
        assert_eq!(d.beweis_werten(&mut p, ip(1), &k, false, t0), Wertung::Falsch { warten_ms: 5000 });
        assert_eq!(p.warten_ms(t0 + Duration::from_secs(1)), 4000);
        // Ein RICHTIGER Beweis vor Ablauf zaehlt als Fehlversuch.
        let t1 = t0 + Duration::from_secs(4);
        assert!(p.zu_frueh(t1));
        assert_eq!(d.beweis_werten(&mut p, ip(1), &k, true, t1), Wertung::Falsch { warten_ms: 10_000 });
        // Nach Ablauf: angenommen, und die Zaehler sind zurueck.
        let t2 = t1 + Duration::from_secs(10);
        assert_eq!(d.beweis_werten(&mut p, ip(1), &k, true, t2), Wertung::Angenommen);
        assert_eq!(d.fehlversuche(ip(1), &k, t2), 0);
        assert_eq!(d.phase_beginnen(ip(1), &k, t2).warten_ms(t2), 0);
        // Fuenf Fehlversuche in einer Verbindung: Schluss.
        let mut p = d.phase_beginnen(ip(2), &[5; 32], t2);
        let mut t = t2;
        let mut letzte = Wertung::Angenommen;
        for i in 0..PHASE_FEHLVERSUCHE {
            t += Duration::from_secs(400);
            letzte = d.beweis_werten(&mut p, ip(2), &[5; 32], false, t);
            if i + 1 < PHASE_FEHLVERSUCHE {
                assert!(matches!(letzte, Wertung::Falsch { .. }), "{i}: {letzte:?}");
            }
        }
        assert_eq!(letzte, Wertung::Schluss { warten_ms: 20_000 });
        assert_eq!(p.fehlversuche, PHASE_FEHLVERSUCHE);
        // Die naechste Verbindung erbt die Wartezeit der Drossel.
        let p = d.phase_beginnen(ip(2), &[6; 32], t + Duration::from_secs(1));
        assert_eq!(p.warten_ms(t + Duration::from_secs(1)), 19_000);
        // Gesamtfrist 120 s.
        assert!(!p.abgelaufen(p.beginn + Duration::from_secs(119)));
        assert!(p.abgelaufen(p.beginn + PHASE_FRIST));
        // Wertung -> Ergebnis.
        assert_eq!(Wertung::Angenommen.ergebnis([1; 32]), Ergebnis::Passwort { host_beweis: [1; 32] });
        assert_eq!(Wertung::Falsch { warten_ms: 3 }.ergebnis([1; 32]), Ergebnis::Falsch { warten_ms: 3 });
        assert_eq!(Wertung::Schluss { warten_ms: 4 }.ergebnis([1; 32]).code(), ERGEBNIS_SCHLUSS);
    }

    /// Zwei Phasen derselben Adresse mit verschiedenen Schluesseln (zwei je
    /// IP sind erlaubt): muss die Adresse laut Drossel warten, gilt das auch
    /// fuer die zweite, deren Phase nichts davon sagte (3.3: der groessere
    /// Wert gilt; wie qc_zugang_drossel_rest im Mac-Host). Sonst riete sie
    /// abwechselnd weiter - und ein richtiger Beweis kaeme in der Wartezeit
    /// durch und setzte die Zaehler zurueck.
    #[test]
    fn drossel_gilt_fuer_parallele_phasen() {
        let t0 = Instant::now();
        let (a, b) = ([7u8; 32], [8u8; 32]);
        let mut d = Drossel::neu();
        let mut pa = d.phase_beginnen(ip(1), &a, t0);
        let mut pb = d.phase_beginnen(ip(1), &b, t0);
        for _ in 0..3 {
            d.beweis_werten(&mut pa, ip(1), &a, false, t0);
        }
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(d.rest(ip(1), &b, t1), Duration::from_secs(4), "die Adresse wartet noch 4 s");
        assert!(!pb.zu_frueh(t1), "die Phase von B allein sagt nichts");
        assert!(d.zu_frueh(&pb, ip(1), &b, t1));
        // Richtig, aber in der Wartezeit der Adresse: Fehlversuch (der
        // vierte der Adresse: 10 s), nicht angenommen.
        assert_eq!(d.beweis_werten(&mut pb, ip(1), &b, true, t1), Wertung::Falsch { warten_ms: 10_000 });
        assert_eq!(d.fehlversuche(ip(1), &b, t1), 4, "die Zaehler blieben stehen");
        // Falsch in der Wartezeit: ebenso gezaehlt, die Wartezeit waechst.
        let t2 = t1 + Duration::from_secs(2);
        assert_eq!(d.beweis_werten(&mut pb, ip(1), &b, false, t2), Wertung::Falsch { warten_ms: 20_000 });
        // Nach Ablauf: angenommen, beide Zaehler zurueck.
        let t3 = t2 + Duration::from_secs(20);
        assert!(!d.zu_frueh(&pb, ip(1), &b, t3));
        assert_eq!(d.beweis_werten(&mut pb, ip(1), &b, true, t3), Wertung::Angenommen);
        assert_eq!(d.rest(ip(1), &b, t3), Duration::ZERO);
        // Die globale Sperre zaehlt beim Beweis nicht mit (nur beim Beginn
        // einer Phase), sonst sperrte eine Flut jeden rechtmaessigen Nutzer aus.
        let mut g = Drossel::neu();
        let mut p = g.phase_beginnen(ip(100), &[9; 32], t0);
        for i in 0..=GLOBAL_GRENZE as u8 {
            g.fehlversuch(ip(i), &[i; 32], t0);
        }
        assert!(g.global_gesperrt(t0));
        assert_eq!(g.rest(ip(100), &[9; 32], t0), Duration::ZERO);
        assert_eq!(g.beweis_werten(&mut p, ip(100), &[9; 32], true, t0), Wertung::Angenommen);
    }

    /// Kein lesbares Passwort am Host: der Beweis ist nicht pruefbar - das
    /// liegt am Host, also keine Drossel und warten_ms 0; die Versuche der
    /// Verbindung zaehlen trotzdem (der fuenfte ist Schluss).
    #[test]
    fn nicht_pruefbar_ohne_drossel() {
        let t0 = Instant::now();
        let k = [3u8; 32];
        let mut d = Drossel::neu();
        let mut p = d.phase_beginnen(ip(1), &k, t0);
        for _ in 1..PHASE_FEHLVERSUCHE {
            assert_eq!(d.beweis_nicht_pruefbar(&mut p), Wertung::Falsch { warten_ms: 0 });
        }
        assert_eq!(d.beweis_nicht_pruefbar(&mut p), Wertung::Schluss { warten_ms: 0 });
        assert_eq!(p.fehlversuche, PHASE_FEHLVERSUCHE);
        assert_eq!(d.fehlversuche(ip(1), &k, t0), 0);
        assert_eq!(d.warten(ip(1), &k, t0), Duration::ZERO);
        assert!(!d.global_gesperrt(t0));
    }

    #[test]
    fn plaetze_begrenzt() {
        let mut p = Plaetze::default();
        assert!(p.belegen(ip(1), &[1; 32]));
        // Derselbe Schluessel hoechstens einmal, auch von anderer IP.
        assert!(!p.belegen(ip(2), &[1; 32]));
        assert!(p.belegen(ip(1), &[2; 32]));
        // Hoechstens 2 je IP.
        assert!(!p.belegen(ip(1), &[3; 32]));
        assert!(p.belegen(ip(2), &[3; 32]));
        assert!(p.belegen(ip(3), &[4; 32]));
        // Hoechstens 4 insgesamt.
        assert_eq!(p.belegt(), 4);
        assert!(!p.belegen(ip(4), &[5; 32]));
        p.freigeben(ip(1), &[1; 32]);
        assert!(p.belegen(ip(4), &[5; 32]));
        // Freigeben von etwas, das nicht belegt ist, aendert nichts.
        p.freigeben(ip(9), &[9; 32]);
        assert_eq!(p.belegt(), 4);
        p.freigeben(ip(1), &[2; 32]);
        assert!(p.belegen(ip(1), &[1; 32]));
    }

    // --------------------------------------------------- host-devices.txt

    #[test]
    fn geraeteliste_eintragen_entfernen() {
        let d = ordner("geraete");
        let pfad = d.join(GERAETE_DATEI);
        let l = Geraeteliste::laden(&pfad).unwrap();
        assert!(l.geraete.is_empty() && !l.vorhanden);
        let a = host_pub();
        assert!(geraet_eintragen(&pfad, &a, "Roberts PC", "2026-09-26").unwrap());
        assert_eq!(lesen(&pfad), format!("{}  2026-09-26  Roberts PC\n", hex(&a)));
        #[cfg(unix)]
        nur_eigentuemer(&pfad);
        // Schon da: nichts verdoppelt, nichts geaendert.
        assert!(!geraet_eintragen(&pfad, &a, "anders", "2027-01-01").unwrap());
        assert!(geraet_eintragen(&pfad, &[0xbb; 32], "Name mit\nZeilenwechsel", "2026-09-27").unwrap());
        assert!(geraet_eintragen(&pfad, &[0xcc; 32], "", "2026-09-28").unwrap());
        let l = Geraeteliste::laden(&pfad).unwrap();
        assert!(l.vorhanden);
        assert_eq!(l.geraete.len(), 3);
        assert_eq!(l.geraete[1].name, "Name mit?Zeilenwechsel");
        assert_eq!(l.geraete[2].name, "-");
        assert!(l.enthaelt(&a) && l.enthaelt(&[0xbb; 32]) && !l.enthaelt(&[0xdd; 32]));
        let g = l.finden(&a).unwrap();
        assert_eq!((g.id(), g.datum.as_str(), g.name.as_str()), (581_729_911, "2026-09-26", "Roberts PC"));
        assert_eq!(lesen(&pfad).lines().count(), 3);
        // Entfernen.
        assert!(geraet_entfernen(&pfad, &[0xbb; 32]).unwrap());
        assert!(!geraet_entfernen(&pfad, &[0xbb; 32]).unwrap());
        let l = Geraeteliste::laden(&pfad).unwrap();
        assert_eq!(l.geraete.iter().map(|g| g.schluessel[0]).collect::<Vec<_>>(), [1, 0xcc]);
        // Alle entfernen: eine leere Liste bleibt stehen.
        alle_geraete_entfernen(&pfad).unwrap();
        assert_eq!(lesen(&pfad), "");
        let l = Geraeteliste::laden(&pfad).unwrap();
        assert!(l.vorhanden && l.geraete.is_empty());
        // Keine Zwischendatei bleibt liegen.
        let reste: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(reste.len(), 1, "{reste:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn geraeteliste_lesen_tolerant() {
        let a = hex(&host_pub()).to_uppercase();
        let b = hex(&[0xbb; 32]);
        let text = format!(
            "\u{feff}# erlaubte Geraete\r\n{a}  2026-09-26  Roberts PC\r\n\r\n   \n{b}\t2026-01-31\tMac  mini  \n{b}  2026-02-01  doppelt\n{a}  2026-09-26\n"
        );
        let g = Geraeteliste::aus_text(text.strip_prefix('\u{feff}').unwrap()).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].schluessel, host_pub());
        assert_eq!(g[0].name, "Roberts PC");
        assert_eq!((g[1].datum.as_str(), g[1].name.as_str()), ("2026-01-31", "Mac  mini"));
        let d = ordner("geraete-tolerant");
        let pfad = d.join(GERAETE_DATEI);
        std::fs::write(&pfad, &text).unwrap();
        assert_eq!(Geraeteliste::laden(&pfad).unwrap().geraete, g);
        // Zeile ohne Namen: "-".
        let g = Geraeteliste::aus_text(&format!("{b} 2026-01-31")).unwrap();
        assert_eq!(g[0].name, "-");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn geraeteliste_beschaedigt_bleibt_liegen() {
        let d = ordner("geraete-kaputt");
        let pfad = d.join(GERAETE_DATEI);
        let a = hex(&host_pub());
        for (inhalt, grund) in [
            (format!("{a}  2026-09-26  PC\nkaputt\n"), "Zeile 2"),
            (format!("{}  2026-09-26  PC\n", &a[..63]), "Zeile 1"),
            (format!("{a}0  2026-09-26  PC\n"), "Schluessel"),
            (format!("{a}  26.09.2026  PC\n"), "Datum"),
            (format!("{a}  2026-13-01  PC\n"), "Datum"),
            (format!("{a}  2026-09-32  PC\n"), "Datum"),
            (format!("{a}\n"), "Datum"),
            (format!("  x{a}  2026-09-26  PC\n"), "Schluessel"),
        ] {
            std::fs::write(&pfad, &inhalt).unwrap();
            match Geraeteliste::laden(&pfad) {
                Err(DateiFehler::Beschaedigt { grund: g, .. }) => assert!(g.contains(grund), "{inhalt:?}: {g}"),
                x => panic!("{inhalt:?}: {x:?}"),
            }
            // Weder Eintragen noch Entfernen noch "alle entfernen" ueberschreiben sie.
            assert!(geraet_eintragen(&pfad, &[0xbb; 32], "neu", "2026-09-26").unwrap_err().beim_lesen());
            assert!(geraet_entfernen(&pfad, &host_pub()).is_err());
            assert!(alle_geraete_entfernen(&pfad).is_err());
            assert_eq!(lesen(&pfad), inhalt);
        }
        // Kein UTF-8 und zu gross.
        std::fs::write(&pfad, b"\xff\xfe1\x002\x00").unwrap();
        assert!(matches!(Geraeteliste::laden(&pfad), Err(DateiFehler::KeinUtf8 { .. })));
        std::fs::write(&pfad, vec![b'#'; LISTE_GRENZE as usize + 1]).unwrap();
        assert!(matches!(Geraeteliste::laden(&pfad), Err(DateiFehler::Beschaedigt { .. })));
        // Ein Ordner an ihrer Stelle: unlesbar.
        std::fs::remove_file(&pfad).unwrap();
        std::fs::create_dir(&pfad).unwrap();
        assert!(matches!(Geraeteliste::laden(&pfad), Err(DateiFehler::Unlesbar { .. })));
        std::fs::remove_dir(&pfad).unwrap();
        // Zuruecksetzen legt die alte beiseite und beginnt leer.
        let kaputt = format!("{a}  2026-09-26  PC\nkaputt\n");
        std::fs::write(&pfad, &kaputt).unwrap();
        let beiseite = geraeteliste_zuruecksetzen(&pfad, "20260926-163000").unwrap().unwrap();
        assert_eq!(beiseite, d.join("host-devices.txt.defekt-20260926-163000"));
        assert_eq!(lesen(&beiseite), kaputt);
        assert_eq!(lesen(&pfad), "");
        assert!(Geraeteliste::laden(&pfad).unwrap().geraete.is_empty());
        // Noch einmal mit demselben Stempel: kein Ueberschreiben.
        std::fs::write(&pfad, "wieder kaputt\n").unwrap();
        let zweite = geraeteliste_zuruecksetzen(&pfad, "20260926-163000").unwrap().unwrap();
        assert_eq!(zweite, d.join("host-devices.txt.defekt-20260926-163000-2"));
        assert_eq!(lesen(&beiseite), kaputt);
        assert_eq!(lesen(&zweite), "wieder kaputt\n");
        // Ohne Datei: nichts beiseite, leere Liste.
        std::fs::remove_file(&pfad).unwrap();
        assert_eq!(geraeteliste_zuruecksetzen(&pfad, "x").unwrap(), None);
        assert_eq!(lesen(&pfad), "");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn geraete_aus_authorized_txt() {
        let d = ordner("migration-geraete");
        let a = hex(&host_pub());
        let b = hex(&[0xbb; 32]).to_uppercase();
        let c = hex(&[0xcc; 32]);
        let fp = crate::noise::fingerprint(&host_pub());
        let alt = format!(
            "# Kommentar\n{a}  {fp}  192.168.178.20\n\nMuell\n{b}  9EB4-EC3D-6856-8AF6  Studio Mac\n{a}  {fp}  doppelt\n{c}\n{c}0  zu lang\n"
        );
        std::fs::write(d.join(ALTE_FREIGABEN), &alt).unwrap();
        let m = geraete_migrieren(&d, "2026-09-26");
        assert_eq!(m, Migration::Uebernommen { anzahl: 3, umbenannt: Ok(()) });
        let l = Geraeteliste::laden(&d.join(GERAETE_DATEI)).unwrap();
        let namen: Vec<_> = l.geraete.iter().map(|g| (g.schluessel[0], g.name.as_str(), g.datum.as_str())).collect();
        assert_eq!(
            namen,
            [(1, "192.168.178.20", "2026-09-26"), (0xbb, "Studio Mac", "2026-09-26"), (0xcc, "-", "2026-09-26")]
        );
        assert!(!d.join(ALTE_FREIGABEN).exists());
        assert_eq!(lesen(&d.join("authorized.txt.migriert")), alt);
        #[cfg(unix)]
        nur_eigentuemer(&d.join(GERAETE_DATEI));
        // Ein zweiter Start findet nichts mehr zu tun.
        assert_eq!(geraete_migrieren(&d, "2026-09-27"), Migration::Keine);
        // Liegt wieder eine authorized.txt da, die neue Liste aber auch: keine Uebernahme.
        std::fs::write(d.join(ALTE_FREIGABEN), format!("{c}  x  neu\n")).unwrap();
        assert_eq!(geraete_migrieren(&d, "2026-09-27"), Migration::Keine);
        assert!(d.join(ALTE_FREIGABEN).exists());
        assert_eq!(Geraeteliste::laden(&d.join(GERAETE_DATEI)).unwrap().geraete.len(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn geraete_migration_grenzfaelle() {
        // Ohne authorized.txt: nichts, und keine neue Datei.
        let d = ordner("migration-geraete-leer");
        assert_eq!(geraete_migrieren(&d, "2026-09-26"), Migration::Keine);
        assert!(!d.join(GERAETE_DATEI).exists());
        // Leere authorized.txt: leere Liste (keine Erstkontakt-Aufnahme mehr).
        std::fs::write(d.join(ALTE_FREIGABEN), "").unwrap();
        assert_eq!(geraete_migrieren(&d, "2026-09-26"), Migration::Uebernommen { anzahl: 0, umbenannt: Ok(()) });
        assert_eq!(lesen(&d.join(GERAETE_DATEI)), "");
        // Unlesbare authorized.txt: Fehler, nichts angelegt, nichts umbenannt.
        let d = ordner("migration-geraete-kaputt");
        std::fs::write(d.join(ALTE_FREIGABEN), b"\xff\xff").unwrap();
        assert!(matches!(geraete_migrieren(&d, "2026-09-26"), Migration::Fehler(_)));
        assert!(!d.join(GERAETE_DATEI).exists());
        assert!(d.join(ALTE_FREIGABEN).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    // --------------------------------------------------- host-password.txt

    fn zufallsform(pw: &str) {
        let b = pw.as_bytes();
        assert_eq!(b.len(), 11, "{pw}");
        assert_eq!((b[3], b[7]), (b'-', b'-'), "{pw}");
        for (i, c) in b.iter().enumerate() {
            if i != 3 && i != 7 {
                assert!(ZUFALL_ALPHABET.contains(c), "{pw}");
            }
        }
        assert_eq!(passwort_norm(pw).len(), 9);
        passwort_pruefen(pw).unwrap();
    }

    #[test]
    fn zufallspasswort_form_und_streuung() {
        let mut gesehen = std::collections::HashSet::new();
        let mut zeichen = std::collections::HashSet::new();
        for _ in 0..300 {
            let pw = zufallspasswort().unwrap();
            zufallsform(&pw);
            zeichen.extend(pw.bytes().filter(|&c| c != b'-'));
            gesehen.insert(pw);
        }
        assert_eq!(gesehen.len(), 300, "Wiederholung bei 44 Bit");
        // 2700 Zeichen: jedes der 31 kommt vor.
        assert_eq!(zeichen.len(), ZUFALL_ALPHABET.len());
        for verwechselbar in [b'i', b'l', b'o', b'0', b'1'] {
            assert!(!ZUFALL_ALPHABET.contains(&verwechselbar));
        }
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        zufall(&mut a).unwrap();
        zufall(&mut b).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0; 32]);
    }

    #[test]
    fn passwortdatei() {
        let d = ordner("passwort");
        let pfad = d.join(PASSWORT_DATEI);
        // Fehlt: Zufallspasswort, angelegt und gemerkt.
        let p = passwort_laden(&pfad).unwrap();
        assert!(p.neu);
        zufallsform(&p.text);
        assert_eq!(lesen(&pfad), format!("{}\n", p.text));
        #[cfg(unix)]
        nur_eigentuemer(&pfad);
        let q = passwort_laden(&pfad).unwrap();
        assert_eq!(q, Passwort { text: p.text.clone(), neu: false });
        // Eigenes Passwort: norm mindestens 8 Byte.
        assert_eq!(passwort_setzen(&pfad, "abc def g"), Err(PasswortFehler::ZuKurz));
        assert_eq!(passwort_setzen(&pfad, "--------"), Err(PasswortFehler::ZuKurz));
        assert_eq!(passwort_setzen(&pfad, "abcd\nefgh"), Err(PasswortFehler::Steuerzeichen));
        assert_eq!(passwort_setzen(&pfad, "abcd\tefgh"), Err(PasswortFehler::Steuerzeichen));
        assert_eq!(passwort_setzen(&pfad, &"x".repeat(129)), Err(PasswortFehler::ZuLang));
        assert_eq!(lesen(&pfad), format!("{}\n", p.text), "nichts geschrieben");
        assert_eq!(passwort_setzen(&pfad, "  Mein Passwort-1 \t").unwrap(), "Mein Passwort-1");
        assert_eq!(lesen(&pfad), "Mein Passwort-1\n");
        #[cfg(unix)]
        nur_eigentuemer(&pfad);
        assert_eq!(passwort_laden(&pfad).unwrap().text, "Mein Passwort-1");
        assert_eq!(passwort_setzen(&pfad, "ab-cd-ef-gh").unwrap(), "ab-cd-ef-gh");
        assert_eq!(passwort_setzen(&pfad, "Grüße, Mac!").unwrap(), "Grüße, Mac!");
        assert_eq!(passwort_laden(&pfad).unwrap().text, "Grüße, Mac!");
        // Neues Zufallspasswort ersetzt es.
        let z = passwort_zufall_setzen(&pfad).unwrap();
        zufallsform(&z);
        assert_eq!(passwort_laden(&pfad).unwrap(), Passwort { text: z.clone(), neu: false });
        // CRLF, BOM und Leerzeilen danach sind in Ordnung.
        std::fs::write(&pfad, "\u{feff}k7m-4wq-9tz\r\n\r\n").unwrap();
        assert_eq!(passwort_laden(&pfad).unwrap().text, "k7m-4wq-9tz");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn passwortdatei_ungueltig_gibt_kein_passwort() {
        let d = ordner("passwort-kaputt");
        let pfad = d.join(PASSWORT_DATEI);
        // Leer oder zu kurz: KEIN Passwort - sonst kaeme man mit einer leeren
        // Eingabe herein. Die Datei bleibt, wie sie ist.
        for inhalt in [&b""[..], b"\n", b"kurz\n", b"a-b-c-d-e-f-g\n", b"zwei\nZeilen hier\n", b"\xff\xfe"] {
            std::fs::write(&pfad, inhalt).unwrap();
            let e = passwort_laden(&pfad).unwrap_err();
            assert!(e.beim_lesen(), "{inhalt:?}: {e}");
            assert_eq!(std::fs::read(&pfad).unwrap(), inhalt);
        }
        std::fs::write(&pfad, vec![b'a'; PASSWORT_GRENZE as usize + 1]).unwrap();
        assert!(matches!(passwort_laden(&pfad), Err(DateiFehler::Beschaedigt { .. })));
        // "Neues Zufallspasswort" repariert sie.
        let z = passwort_zufall_setzen(&pfad).unwrap();
        assert_eq!(passwort_laden(&pfad).unwrap().text, z);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Zwei Faeden legen gleichzeitig an: beide sehen am Ende dasselbe
    /// Passwort, und es steht in der Datei.
    #[test]
    fn passwort_gleichzeitig_angelegt() {
        let d = ordner("passwort-parallel");
        let pfad = d.join(PASSWORT_DATEI);
        let faeden: Vec<_> = (0..4)
            .map(|_| {
                let p = pfad.clone();
                std::thread::spawn(move || passwort_laden(&p).unwrap())
            })
            .collect();
        let ergebnisse: Vec<Passwort> = faeden.into_iter().map(|f| f.join().unwrap()).collect();
        assert_eq!(ergebnisse.iter().filter(|p| p.neu).count(), 1);
        for p in &ergebnisse {
            assert_eq!(p.text, ergebnisse[0].text);
        }
        assert_eq!(lesen(&pfad), format!("{}\n", ergebnisse[0].text));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn passwort_regeln() {
        assert_eq!(passwort_pruefen("12345678"), Ok(()));
        assert_eq!(passwort_pruefen("1234 567"), Err(PasswortFehler::ZuKurz));
        assert_eq!(passwort_pruefen("äöüä"), Ok(()), "8 Byte UTF-8");
        assert_eq!(passwort_pruefen("äöü"), Err(PasswortFehler::ZuKurz));
        assert_eq!(passwort_pruefen(&"x".repeat(128)), Ok(()));
        assert_eq!(passwort_pruefen("12345678\u{7}"), Err(PasswortFehler::Steuerzeichen));
    }

    // ------------------------------------------------------------ hosts.txt

    fn host(k: u8, adresse: &str, name: &str) -> BekannterHost {
        BekannterHost::neu([k; 32], adresse, name)
    }

    #[test]
    fn hostliste_format_und_suche() {
        let d = ordner("hosts");
        let pfad = d.join(HOSTS_DATEI);
        assert_eq!(Hostliste::laden(&pfad).unwrap(), Hostliste::default());
        host_merken(&pfad, BekannterHost::neu(host_pub(), "192.168.178.20:9001", "Roberts Mac mini")).unwrap();
        assert_eq!(lesen(&pfad), format!("581729911 {} 192.168.178.20:9001 Roberts Mac mini\n", hex(&host_pub())));
        #[cfg(unix)]
        nur_eigentuemer(&pfad);
        host_merken(&pfad, host(2, "studio.local:9001", "Studio")).unwrap();
        let l = Hostliste::laden(&pfad).unwrap();
        assert_eq!(l.hosts.len(), 2);
        // Neuester zuerst.
        assert_eq!(l.hosts[0].name, "Studio");
        assert_eq!(l.nach_schluessel(&host_pub()).unwrap().adresse, "192.168.178.20:9001");
        assert_eq!(l.nach_id(581_729_911).unwrap().name, "Roberts Mac mini");
        assert_eq!(l.nach_adresse("STUDIO.local:9001").unwrap().schluessel, [2; 32]);
        assert!(l.nach_adresse("studio.local").is_none());
        assert!(l.nach_id(1).is_none() && l.nach_schluessel(&[9; 32]).is_none());
        // Derselbe Host unter neuer Adresse (DHCP): ein Eintrag, neue Adresse, vorn.
        host_merken(&pfad, BekannterHost::neu(host_pub(), "192.168.178.31:9001", "Roberts Mac mini")).unwrap();
        let l = Hostliste::laden(&pfad).unwrap();
        assert_eq!(l.hosts.len(), 2);
        assert_eq!(l.hosts[0].adresse, "192.168.178.31:9001");
        assert!(l.nach_adresse("192.168.178.20:9001").is_none());
        // Ein anderes Geraet unter einer bekannten Adresse (8.3): der alte
        // Eintrag bleibt fuer seine ID, die Adresse fuehrt zum neuen.
        host_merken(&pfad, host(3, "studio.local:9001", "Neues Studio")).unwrap();
        let l = Hostliste::laden(&pfad).unwrap();
        assert_eq!(l.hosts.len(), 3);
        assert_eq!(l.nach_adresse("studio.local:9001").unwrap().name, "Neues Studio");
        assert_eq!(l.nach_id(geraete_id(&[2; 32])).unwrap().name, "Studio");
        // Vergessen.
        assert!(host_vergessen(&pfad, &[2; 32]).unwrap());
        assert!(!host_vergessen(&pfad, &[2; 32]).unwrap());
        assert_eq!(Hostliste::laden(&pfad).unwrap().hosts.len(), 2);
        // Ungueltige Adresse: nicht gepinnt.
        assert!(host_merken(&pfad, host(4, "zwei worte:9001", "x")).is_err());
        assert!(host_merken(&pfad, host(4, "", "x")).is_err());
        assert_eq!(Hostliste::laden(&pfad).unwrap().hosts.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hostliste_tolerant() {
        let a = hex(&host_pub());
        let b = hex(&[2; 32]);
        let id_b = id_ziffern(geraete_id(&[2; 32]));
        let text = format!(
            "581729911 {a} 10.0.0.5:9001 Mac  mit  Leerzeichen\r\n\
             # Kommentar\n\
             123456789 {b} 10.0.0.6:9001 falsche ID\n\
             {id_b} {b} 10.0.0.7:9001\n\
             {id_b} {} 10.0.0.8:9001 doppelt\n\
             kaputt\n\n",
            b.to_uppercase()
        );
        let l = Hostliste::aus_text(&text);
        assert_eq!(l.hosts.len(), 2);
        assert_eq!(l.hosts[0].name, "Mac  mit  Leerzeichen");
        // Ohne Namen: die Adresse.
        assert_eq!(l.hosts[1].name, "10.0.0.7:9001");
        // Fremde Zeilen bleiben beim Speichern erhalten (hinten).
        let t = l.text();
        assert!(t.contains("# Kommentar\n") && t.contains("falsche ID") && t.contains("kaputt\n"), "{t}");
        assert!(!t.contains("doppelt"));
        assert_eq!(Hostliste::aus_text(&t), l);
        // Kein UTF-8: Fehler (wie known_hosts.txt heute).
        let d = ordner("hosts-kaputt");
        let pfad = d.join(HOSTS_DATEI);
        std::fs::write(&pfad, b"\xff").unwrap();
        assert!(matches!(Hostliste::laden(&pfad), Err(DateiFehler::KeinUtf8 { .. })));
        assert!(host_merken(&pfad, host(5, "a:1", "a")).is_err());
        assert_eq!(std::fs::read(&pfad).unwrap(), b"\xff");
        // Fuer die Meldungen des Clients: dieselben Fehlerarten wie heute.
        let f: crate::secure::Fehler = DateiFehler::KeinUtf8 { pfad: pfad.clone() }.into();
        assert_eq!(f, crate::secure::Fehler::KeinUtf8 { pfad: pfad.clone() });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hosts_aus_known_hosts_txt() {
        let d = ordner("migration-hosts");
        let a = hex(&host_pub());
        let fp = crate::noise::fingerprint(&host_pub());
        let alt = format!(
            "192.168.178.20 {a} {fp}\n\
             studio.local {} X\n\
             roberts-mac.local {a} {fp}\n\
             [fe80::1] {} X\n\
             fe80::2 {} X\n\
             kaputt\n\
             10.0.0.9 1234 X\n",
            hex(&[2; 32]),
            hex(&[3; 32]),
            hex(&[4; 32])
        );
        std::fs::write(d.join(ALTE_HOSTS), &alt).unwrap();
        assert_eq!(hosts_migrieren(&d), Migration::Uebernommen { anzahl: 4, umbenannt: Ok(()) });
        let l = Hostliste::laden(&d.join(HOSTS_DATEI)).unwrap();
        let zeilen: Vec<_> = l.hosts.iter().map(|h| (h.schluessel[0], h.adresse.as_str(), h.name.as_str())).collect();
        assert_eq!(
            zeilen,
            [
                (1, "192.168.178.20:9001", "192.168.178.20"),
                (2, "studio.local:9001", "studio.local"),
                (3, "[fe80::1]:9001", "[fe80::1]"),
                (4, "[fe80::2]:9001", "fe80::2"),
            ]
        );
        assert_eq!(l.hosts[0].id, 581_729_911);
        assert!(!d.join(ALTE_HOSTS).exists());
        assert_eq!(lesen(&d.join("known_hosts.txt.migriert")), alt);
        #[cfg(unix)]
        nur_eigentuemer(&d.join(HOSTS_DATEI));
        assert_eq!(hosts_migrieren(&d), Migration::Keine);
        // hosts.txt schon da: known_hosts.txt bleibt unberuehrt.
        std::fs::write(d.join(ALTE_HOSTS), &alt).unwrap();
        assert_eq!(hosts_migrieren(&d), Migration::Keine);
        assert!(d.join(ALTE_HOSTS).exists());
        // Unlesbar: nichts angelegt.
        let d = ordner("migration-hosts-kaputt");
        std::fs::write(d.join(ALTE_HOSTS), b"\xff").unwrap();
        assert!(matches!(hosts_migrieren(&d), Migration::Fehler(_)));
        assert!(!d.join(HOSTS_DATEI).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ------------------------------------------------------------ Uebriges

    #[test]
    fn zeitpunkte() {
        let z = |s: i64| {
            let t = Zeitpunkt::aus_unix(s);
            format!("{} {:02}:{:02}:{:02}", t.datum(), t.stunde, t.minute, t.sekunde)
        };
        // Mit Python (datetime, UTC) gegengerechnet.
        assert_eq!(z(0), "1970-01-01 00:00:00");
        assert_eq!(z(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(z(1_709_164_800), "2024-02-29 00:00:00");
        assert_eq!(z(1_790_380_800), "2026-09-26 00:00:00");
        assert_eq!(z(1_790_445_599), "2026-09-26 17:59:59");
        assert_eq!(z(4_102_444_800), "2100-01-01 00:00:00");
        assert_eq!(z(-86_400), "1969-12-31 00:00:00");
        assert_eq!(Zeitpunkt::aus_unix(1_790_445_599).stempel(), "20260926-175959");
        let h = heute();
        assert!(datum_gueltig(&h), "{h}");
        assert!(h.as_str() >= "2026-01-01");
    }

    #[test]
    fn zwischendateien_weg() {
        let d = ordner("zwischen");
        let pfad = d.join(GERAETE_DATEI);
        std::fs::write(d.join("host-devices.txt.4711-0.neu"), "x").unwrap();
        std::fs::write(d.join("host-devices.txt.4711-1.neu"), "x").unwrap();
        std::fs::write(d.join("hosts.txt.4711-0.neu"), "x").unwrap();
        std::fs::write(&pfad, "").unwrap();
        assert_eq!(zwischendateien_aufraeumen(&pfad), 2);
        assert!(d.join("hosts.txt.4711-0.neu").exists() && pfad.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hex_hin_und_zurueck() {
        let k = host_pub();
        assert_eq!(hex(&k), "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20");
        assert_eq!(hex_lesen(&hex(&k)), Some(k));
        assert_eq!(hex_lesen(&hex(&k).to_uppercase()), Some(k));
        assert_eq!(hex_lesen(&hex(&k)[..62]), None);
        assert_eq!(hex_lesen(&format!("{}g", &hex(&k)[..63])), None);
        assert_eq!(hex_lesen(&format!("{}ä", &hex(&k)[..62])), None);
    }
}
