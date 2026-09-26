// Kryptoschicht der Windows-Seite.
//
// Dieselbe Spezifikation wie der Host: Noise_XX_25519_ChaChaPoly_SHA256, der
// Client faengt an. Hier uebernimmt das snow die Arbeit, auf dem Mac ist es
// eine eigene Umsetzung derselben Spezifikation - deshalb der Gegentest:
// beide Seiten muessen dieselbe Pruefsumme und denselben Vergleichscode sehen.
//
// Abhaengigkeit: snow = "0.10"  (Apache-2.0 ODER MIT), sha2 = "0.10" (MIT ODER Apache-2.0)

use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, TransportState};

pub const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// Prologue je Kanal. Der Eingabekanal bindet sich zusaetzlich an die
/// Pruefsumme des Bildkanals, damit er allein nichts wert ist.
pub fn prologue_video() -> Vec<u8> {
    b"QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256".to_vec()
}

pub fn prologue_input(h1: &[u8]) -> Vec<u8> {
    let mut v = b"QuadChroma/1 input Noise_XX_25519_ChaChaPoly_SHA256".to_vec();
    v.extend_from_slice(h1);
    v
}

/// Sechsstelliger Vergleichscode, identisch berechnet wie auf dem Mac.
pub fn sas(handshake_hash: &[u8]) -> String {
    let mut h = Sha256::new();
    let mut label = b"QuadChroma/1 SAS".to_vec();
    label.push(0);
    h.update(&label);
    h.update(handshake_hash);
    let d = h.finalize();
    let v = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{:03} {:03}", v / 1000, v % 1000)
}

/// Fingerabdruck eines oeffentlichen Schluessels, wie ihn der Host anzeigt.
pub fn fingerprint(pubkey: &[u8]) -> String {
    let d = Sha256::digest(pubkey);
    format!(
        "{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}",
        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

pub struct Session {
    pub transport: TransportState,
    pub handshake_hash: Vec<u8>,
    pub remote_static: Vec<u8>,
    /// Nutzlast von Nachricht 3, wie sie beim Angerufenen ankam (Name des
    /// Clients, zugang::nachricht3_name); beim Anrufer leer.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub nachricht3: Vec<u8>,
}

/// Fuehrt den Handschlag als Anrufer ueber eine beliebige Leitung. Die beiden
/// Funktionen lesen und schreiben je eine Nachricht mit 2-Byte-Laenge davor.
///
/// `pruefen` bekommt den dauerhaften Schluessel der Gegenstelle, sobald
/// Nachricht 2 gelesen ist ("e, ee, s, es": der Schluessel ist da, und die
/// Gegenstelle hat bewiesen, dass sie ihn besitzt) - und VOR Nachricht 3.
/// Scheitert die Pruefung, geht Nachricht 3 nicht hinaus: die Gegenstelle
/// lernt den eigenen Schluessel nie kennen, und ein Host nimmt diesen
/// Anrufer gar nicht erst als Zuschauer an (und loest dafuer niemanden ab).
///
/// Gelingt sie, liefert sie die Nutzlast, die verschluesselt mit Nachricht 3
/// hinausgeht - sie darf also vom Schluessel abhaengen: der Client schickt
/// "QCN1" mit seinem Geraetenamen und Flags (zugang::nachricht3), Bit 0,
/// wenn er diesen Schluessel nicht kennt. Aeltere Fassungen schickten
/// b"client" - Hosts, die sie nicht kennen, uebergehen sie.
pub fn handshake_initiator<R, W, P>(
    static_key: &[u8],
    prologue: &[u8],
    mut recv: R,
    mut send: W,
    pruefen: P,
) -> Result<Session, String>
where
    R: FnMut(&mut Vec<u8>) -> Result<(), String>,
    W: FnMut(&[u8]) -> Result<(), String>,
    P: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
{
    let params = PATTERN.parse().map_err(|e| format!("Muster: {e:?}"))?;
    let mut hs: HandshakeState = Builder::new(params)
        .local_private_key(static_key)
        .map_err(|e| format!("Schluessel: {e:?}"))?
        .prologue(prologue)
        .map_err(|e| format!("Prologue: {e:?}"))?
        .build_initiator()
        .map_err(|e| format!("Aufbau: {e:?}"))?;

    let mut buf = vec![0u8; 65535];
    let n = hs.write_message(&[], &mut buf).map_err(|e| format!("Nachricht 1: {e:?}"))?;
    send(&buf[..n])?;

    let mut incoming = Vec::new();
    recv(&mut incoming)?;
    let mut payload = vec![0u8; 65535];
    hs.read_message(&incoming, &mut payload).map_err(|e| format!("Nachricht 2: {e:?}"))?;

    // Wer am anderen Ende sitzt, steht jetzt fest - erst pruefen, dann den
    // eigenen Schluessel zeigen.
    let nutzlast3 = pruefen(hs.get_remote_static().unwrap_or_default())?;

    let n = hs.write_message(&nutzlast3, &mut buf).map_err(|e| format!("Nachricht 3: {e:?}"))?;
    send(&buf[..n])?;

    let handshake_hash = hs.get_handshake_hash().to_vec();
    let remote_static = hs.get_remote_static().map(|k| k.to_vec()).unwrap_or_default();
    let transport = hs.into_transport_mode().map_err(|e| format!("Umschalten: {e:?}"))?;
    Ok(Session { transport, handshake_hash, remote_static, nachricht3: Vec::new() })
}

/// Fuehrt den Handschlag als Angerufener (Host-Rolle) ueber eine beliebige
/// Leitung - das Gegenstueck zu qc_chan_accept auf dem Mac: Nachricht 1
/// lesen, Nachricht 2 ohne Nutzlast schreiben, Nachricht 3 lesen, dann
/// Pruefsumme, Gegenschluessel und die Nutzlast von Nachricht 3 festhalten
/// (Name des Clients, "QCN1" - aeltere Clients senden "client") und in den
/// Betrieb umschalten.
pub fn handshake_responder<R, W>(
    static_key: &[u8],
    prologue: &[u8],
    mut recv: R,
    mut send: W,
) -> Result<Session, String>
where
    R: FnMut(&mut Vec<u8>) -> Result<(), String>,
    W: FnMut(&[u8]) -> Result<(), String>,
{
    let params = PATTERN.parse().map_err(|e| format!("Muster: {e:?}"))?;
    let mut hs: HandshakeState = Builder::new(params)
        .local_private_key(static_key)
        .map_err(|e| format!("Schluessel: {e:?}"))?
        .prologue(prologue)
        .map_err(|e| format!("Prologue: {e:?}"))?
        .build_responder()
        .map_err(|e| format!("Aufbau: {e:?}"))?;

    let mut incoming = Vec::new();
    let mut payload = vec![0u8; 65535];
    recv(&mut incoming)?;
    hs.read_message(&incoming, &mut payload).map_err(|e| format!("Nachricht 1: {e:?}"))?;

    let mut buf = vec![0u8; 65535];
    let n = hs.write_message(&[], &mut buf).map_err(|e| format!("Nachricht 2: {e:?}"))?;
    send(&buf[..n])?;

    recv(&mut incoming)?;
    let n3 = hs.read_message(&incoming, &mut payload).map_err(|e| format!("Nachricht 3: {e:?}"))?;
    let nachricht3 = payload[..n3].to_vec();

    let handshake_hash = hs.get_handshake_hash().to_vec();
    let remote_static = hs.get_remote_static().map(|k| k.to_vec()).unwrap_or_default();
    let transport = hs.into_transport_mode().map_err(|e| format!("Umschalten: {e:?}"))?;
    Ok(Session { transport, handshake_hash, remote_static, nachricht3 })
}

/// Erzeugt ein langlebiges Schluesselpaar.
pub fn keypair() -> Result<(Vec<u8>, Vec<u8>), String> {
    let params = PATTERN.parse().map_err(|e| format!("Muster: {e:?}"))?;
    let kp = Builder::new(params).generate_keypair().map_err(|e| format!("Schluessel: {e:?}"))?;
    Ok((kp.private, kp.public))
}

/// Gegentest gegen den Mac: Handschlag, Pruefsumme, ein verschluesselter Satz
/// hin und zurueck.
pub fn selftest_against(addr: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let (priv_key, pub_key) = keypair()?;
    println!("Client-Fingerabdruck: {}", fingerprint(&pub_key));

    let sock = TcpStream::connect(addr).map_err(|e| format!("Verbindung: {e}"))?;
    sock.set_nodelay(true).ok();
    let mut rd = sock.try_clone().map_err(|e| format!("Leitung: {e}"))?;

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
        (&sock).write_all(&out).map_err(|e| format!("Senden: {e}"))
    };

    let mut s = handshake_initiator(&priv_key, &prologue_video(), recv, send, |_| Ok(b"client".to_vec()))?;
    println!("Handschlag fertig");
    println!("Vergleichscode: {}", sas(&s.handshake_hash));
    println!("Host-Fingerabdruck: {}", fingerprint(&s.remote_static));

    // Ein verschluesselter Satz hin und zurueck.
    let mut buf = vec![0u8; 4096];
    let n = s.transport.write_message(b"Gruss von Windows", &mut buf).map_err(|e| format!("{e:?}"))?;
    let mut out = Vec::new();
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.extend_from_slice(&buf[..n]);
    (&sock).write_all(&out).map_err(|e| format!("Senden: {e}"))?;

    let mut l = [0u8; 2];
    (&sock).read_exact(&mut l).map_err(|e| format!("Antwortlaenge: {e}"))?;
    let n = u16::from_le_bytes(l) as usize;
    let mut ct = vec![0u8; n];
    (&sock).read_exact(&mut ct).map_err(|e| format!("Antwort: {e}"))?;
    let mut pt = vec![0u8; 4096];
    let n = s.transport.read_message(&ct, &mut pt).map_err(|e| format!("Entschluesseln: {e:?}"))?;
    println!("entschluesselt empfangen: {}", String::from_utf8_lossy(&pt[..n]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Anrufer und Angerufener ueber zwei Kanaele im Speicher. Die Pruefung
    /// sieht nach Nachricht 2 genau den Schluessel des Angerufenen. Scheitert
    /// sie, bricht der Anrufer mit ihrem Grund ab, und beim Angerufenen kommt
    /// keine Nachricht 3 an - er kennt den Anrufer also nicht.
    #[test]
    fn pruefung_vor_nachricht_3() {
        for gilt in [true, false] {
            let (host_priv, host_pub) = keypair().unwrap();
            let (client_priv, client_pub) = keypair().unwrap();
            let (zum_host, beim_host) = mpsc::channel::<Vec<u8>>();
            let (zum_client, beim_client) = mpsc::channel::<Vec<u8>>();
            let host = std::thread::spawn(move || {
                handshake_responder(
                    &host_priv,
                    &prologue_video(),
                    |b| {
                        *b = beim_host.recv().map_err(|_| "Leitung zu".to_string())?;
                        Ok(())
                    },
                    |d| zum_client.send(d.to_vec()).map_err(|_| "Leitung zu".to_string()),
                )
                .map(|s| s.remote_static)
            });
            let mut gesehen = Vec::new();
            let r = handshake_initiator(
                &client_priv,
                &prologue_video(),
                |b| {
                    *b = beim_client.recv().map_err(|_| "Leitung zu".to_string())?;
                    Ok(())
                },
                |d| zum_host.send(d.to_vec()).map_err(|_| "Leitung zu".to_string()),
                |rs| {
                    gesehen = rs.to_vec();
                    if gilt { Ok(b"client".to_vec()) } else { Err("fremder Host".into()) }
                },
            );
            // Leitung zu: wartet der Angerufene noch auf Nachricht 3, endet
            // sein Lesen jetzt.
            drop(zum_host);
            assert_eq!(gesehen, host_pub);
            let beim_host = host.join().unwrap();
            if gilt {
                assert_eq!(r.unwrap().remote_static, host_pub);
                assert_eq!(beim_host.unwrap(), client_pub);
            } else {
                assert_eq!(r.err().as_deref(), Some("fremder Host"));
                assert_eq!(beim_host.err().as_deref(), Some("Leitung zu"), "Nachricht 3 kam an");
            }
        }
    }

    /// Nachricht 3 traegt die Nutzlast des Anrufers (Pairing v1: "QCN1" mit
    /// dem Geraetenamen und Flags) - verschluesselt; der Angerufene bekommt
    /// genau sie (Session.nachricht3, Name des Clients), der Anrufer hat
    /// keine. Die Pruefung nach Nachricht 2 bestimmt sie: sie sieht den
    /// Schluessel des Angerufenen (hier: unbekannt, also Bit 0).
    #[test]
    fn nutzlast_in_nachricht_3() {
        let (host_priv, host_pub) = keypair().unwrap();
        let (client_priv, _) = keypair().unwrap();
        let (zum_host, beim_host) = mpsc::channel::<Vec<u8>>();
        let (zum_client, beim_client) = mpsc::channel::<Vec<u8>>();
        let host = std::thread::spawn(move || {
            handshake_responder(
                &host_priv,
                &prologue_video(),
                |b| {
                    *b = beim_host.recv().map_err(|_| "Leitung zu".to_string())?;
                    Ok(())
                },
                |d| zum_client.send(d.to_vec()).map_err(|_| "Leitung zu".to_string()),
            )
            .map(|s| s.nachricht3)
        });
        let flag = crate::protokoll_konst::NAME_FLAG_HOST_UNBEKANNT;
        let nutzlast = crate::zugang::nachricht3("B\u{fc}ro-PC", flag);
        let anrufer = handshake_initiator(
            &client_priv,
            &prologue_video(),
            |b| {
                *b = beim_client.recv().map_err(|_| "Leitung zu".to_string())?;
                Ok(())
            },
            |d| zum_host.send(d.to_vec()).map_err(|_| "Leitung zu".to_string()),
            |rs| Ok(crate::zugang::nachricht3("B\u{fc}ro-PC", if rs == host_pub.as_slice() { flag } else { 0 })),
        )
        .unwrap();
        let empfangen = host.join().unwrap().unwrap();
        assert_eq!(empfangen, nutzlast);
        assert_eq!(crate::zugang::nachricht3_name(&empfangen).as_deref(), Some("B\u{fc}ro-PC"));
        assert_eq!(crate::zugang::nachricht3_flags(&empfangen), flag);
        assert!(anrufer.nachricht3.is_empty(), "der Anrufer hat keine Nutzlast von Nachricht 3");
    }
}
