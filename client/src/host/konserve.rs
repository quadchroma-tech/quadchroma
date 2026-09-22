// Konservenquelle: ein Annex-B-Strom aus einer Datei als Bildquelle des
// Windows-Hosts (--konserve datei.hevc) - der Pruefweg fuer die VM ohne
// Karte. Kein Codec: der Leser zerlegt in NAL-Einheiten, sammelt je
// Zugriffseinheit und spielt sie im Takt der Zielrate in Schleife ueber den
// normalen Versand (Stempel, Stauregel, Vollbild-Warten). Ein Vollbild ist
// eine Zugriffseinheit mit IRAP (HEVC NAL 16-21) bzw. IDR (H.264 NAL 5);
// Parametersaetze davor gehoeren dazu.
//
// Die Eckdaten des Stroms (Groesse, Farbaufloesung, Bittiefe) liest der
// Software-Decoder von FFmpeg aus dem ersten Vollbild - einmal beim Laden.

use std::time::{Duration, Instant};

use ffmpeg_next as ffmpeg;

use super::{encoder, log, netz, now_us, Z};
use std::sync::atomic::Ordering;

pub struct Zugriffseinheit {
    pub data: Vec<u8>,
    pub key: bool,
}

pub struct Konserve {
    pub aus: Vec<Zugriffseinheit>,
    pub h264: bool,
    pub breite: u32,
    pub hoehe: u32,
    pub chroma444: bool,
    pub zehn_bit: bool,
}

/// Grenzen der NAL-Einheiten (Beginn der Nutzlast hinter dem Startcode, Ende).
fn nal_grenzen(d: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= d.len() {
        if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut aus = Vec::with_capacity(starts.len());
    for (n, &s) in starts.iter().enumerate() {
        let mut e = if n + 1 < starts.len() { starts[n + 1] - 3 } else { d.len() };
        // Ein vierstelliger Startcode (00 00 00 01) hat eine fuehrende Null,
        // die zur vorigen Einheit zu gehoeren scheint.
        while e > s && d[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            aus.push((s, e));
        }
    }
    aus
}

impl Konserve {
    pub fn laden(pfad: &str) -> Result<Konserve, String> {
        let d = std::fs::read(pfad).map_err(|e| format!("nicht lesbar: {e}"))?;
        let nals = nal_grenzen(&d);
        if nals.is_empty() {
            return Err("keine NAL-Einheiten (kein Annex-B-Strom?)".into());
        }
        // HEVC oder H.264? Der erste NAL-Kopf sagt es: HEVC hat das
        // forbidden_zero_bit und Typ 32-34 (VPS/SPS/PPS) in Bit 1-6;
        // H.264 traegt den Typ in den unteren fuenf Bits (7 = SPS).
        let b0 = d[nals[0].0];
        let hevc_typ = (b0 >> 1) & 0x3f;
        let h264 = !((32..=34).contains(&hevc_typ) && (b0 & 0x81) == 0) || pfad.ends_with(".h264") || pfad.ends_with(".264");

        let mut aus: Vec<Zugriffseinheit> = Vec::new();
        let mut akt: Vec<u8> = Vec::new();
        let mut key = false;
        let mut hat_vcl = false;
        let abschliessen = |akt: &mut Vec<u8>, key: &mut bool, hat_vcl: &mut bool, aus: &mut Vec<Zugriffseinheit>| {
            if *hat_vcl && !akt.is_empty() {
                aus.push(Zugriffseinheit { data: std::mem::take(akt), key: *key });
            } else {
                akt.clear();
            }
            *key = false;
            *hat_vcl = false;
        };
        for &(s, e) in &nals {
            let nal = &d[s..e];
            let (vcl, erstes_stueck, irap, neue_au_davor) = if h264 {
                let t = nal[0] & 0x1f;
                let vcl = (1..=5).contains(&t);
                // first_mb_in_slice == 0 ist der Exp-Golomb-Code "1": hoechstes Bit von Byte 1.
                let erstes = vcl && nal.len() > 1 && (nal[1] & 0x80) != 0;
                (vcl, erstes, t == 5, matches!(t, 6 | 7 | 8 | 9))
            } else {
                let t = (nal[0] >> 1) & 0x3f;
                let vcl = t < 32;
                let erstes = vcl && nal.len() > 2 && (nal[2] & 0x80) != 0;
                (vcl, erstes, (16..=21).contains(&t), matches!(t, 32..=35 | 39 | 41..=44))
            };
            if hat_vcl && (neue_au_davor || (vcl && erstes_stueck)) {
                abschliessen(&mut akt, &mut key, &mut hat_vcl, &mut aus);
            }
            akt.extend_from_slice(&[0, 0, 0, 1]);
            akt.extend_from_slice(nal);
            if vcl {
                hat_vcl = true;
                if irap {
                    key = true;
                }
            }
        }
        abschliessen(&mut akt, &mut key, &mut hat_vcl, &mut aus);
        let Some(erstes_vollbild) = aus.iter().position(|a| a.key) else {
            return Err("kein Vollbild im Strom".into());
        };
        // Ab dem ersten Vollbild; was davor liegt, ist ohne Bezugsbild wertlos.
        aus.drain(..erstes_vollbild);

        // Eckdaten aus dem ersten Vollbild, ueber den Software-Decoder.
        let mut dec = crate::software_decoder(h264)?;
        let mut bilder: Vec<ffmpeg::frame::Video> = Vec::new();
        let mut i = 0;
        while bilder.is_empty() && i < aus.len().min(16) {
            let mut p = ffmpeg::Packet::copy(&aus[i].data);
            p.set_pts(Some(i as i64));
            crate::decoder_fuettern(&mut dec, &p, &mut bilder);
            i += 1;
        }
        if bilder.is_empty() {
            // Vielleicht haelt der Decoder das Bild noch zurueck.
            let _ = dec.send_eof();
            let mut f = ffmpeg::frame::Video::empty();
            while dec.receive_frame(&mut f).is_ok() {
                bilder.push(f.clone());
            }
        }
        let Some(b) = bilder.first() else {
            return Err("Decoder liefert aus dem ersten Vollbild kein Bild".into());
        };
        let (chroma444, zehn_bit) = match b.format() {
            ffmpeg::format::Pixel::YUV444P => (true, false),
            ffmpeg::format::Pixel::YUV444P10LE | ffmpeg::format::Pixel::YUV444P12LE | ffmpeg::format::Pixel::YUV444P16LE => (true, true),
            ffmpeg::format::Pixel::YUV420P10LE | ffmpeg::format::Pixel::P010LE => (false, true),
            _ => (false, false),
        };
        let k = Konserve { aus, h264, breite: b.width(), hoehe: b.height(), chroma444, zehn_bit };
        let keys = k.aus.iter().filter(|a| a.key).count();
        log(format!(
            "Konserve: {} Zugriffseinheiten ({} Vollbilder), {}x{}, {}, {}, {} Bit, {:.1} MB",
            k.aus.len(), keys, k.breite, k.hoehe,
            if h264 { "H.264" } else { "HEVC" },
            if chroma444 { "4:4:4" } else { "4:2:0" },
            if zehn_bit { 10 } else { 8 },
            d.len() as f64 / 1e6
        ));
        Ok(k)
    }

    /// Der passende Eintrag der Kandidatentabelle (fuer Strominfo und
    /// Koennensliste).
    fn kandidat_idx(&self) -> usize {
        if self.h264 {
            4
        } else {
            match (self.chroma444, self.zehn_bit) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            }
        }
    }
}

/// Die Konserve im Takt der Zielrate in Schleife ueber den normalen Versand.
/// Wartet der Zuschauer auf ein Vollbild oder wurde eines erzwungen, springt
/// die Schleife zum naechsten Vollbild - das ist bei einer Konserve das
/// "Vollbild erzwingen".
pub fn abspielen(k: Konserve) {
    let idx = k.kandidat_idx();
    encoder::konserve_setzen(idx);
    Z.info_w.store(k.breite, Ordering::Relaxed);
    Z.info_h.store(k.hoehe, Ordering::Relaxed);
    log(format!("Konserve laeuft als Kandidat {idx} ({}) in Schleife, Codecwechsel wird abgelehnt", encoder::kandidat(idx).name));
    std::thread::spawn(move || {
        let n = k.aus.len();
        let mut i = 0usize;
        let mut naechster = Instant::now();
        loop {
            let fps = Z.fps.load(Ordering::Relaxed).max(1);
            let takt = Duration::from_micros(1_000_000 / fps as u64);
            naechster += takt;
            let jetzt = Instant::now();
            if naechster > jetzt {
                std::thread::sleep(naechster - jetzt);
            } else if jetzt - naechster > Duration::from_millis(500) {
                // Weit hinterher (kein Zuschauer, Rechner schlief): Raster neu.
                naechster = jetzt;
            }
            if !netz::zuschauer_da() {
                continue;
            }
            if Z.force_key.swap(false, Ordering::Relaxed) || Z.wait_key.load(Ordering::Relaxed) {
                // Zum naechsten Vollbild springen.
                let mut j = i;
                for _ in 0..n {
                    if k.aus[j].key {
                        break;
                    }
                    j = (j + 1) % n;
                }
                i = j;
            }
            let au = &k.aus[i];
            netz::bild_senden(&au.data, au.key, now_us(), false);
            i = (i + 1) % n;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::nal_grenzen;

    #[test]
    fn startcodes_werden_gefunden() {
        let d = [0, 0, 0, 1, 0x40, 1, 0xAA, 0, 0, 1, 0x42, 1, 0xBB, 0xCC, 0, 0, 0, 1, 0x26, 1, 0xDD];
        let g = nal_grenzen(&d);
        assert_eq!(g, vec![(4, 7), (10, 14), (18, 21)]);
    }
}
