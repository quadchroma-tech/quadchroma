// Zeigerform des Windows-Hosts: aus der Desktop Duplication (FrameInfo.
// PointerShapeBufferSize > 0 -> GetFramePointerShape) nach RGBA mit
// GERADER Deckkraft, Nachricht 49 wie zeiger.m auf dem Mac: Hash (FNV)
// ueber Bild und Rand, nur bei Aenderung senden, hoechstens 20 je Sekunde
// (die Wartekugel ist animiert), Einpassen ueber 256 Punkte, Massstab 1.
// Ein neu verbundener Zuschauer bekommt die Form erzwungen.
//
// Die Duplication liefert den Desktop OHNE Zeiger - die Projektregel "kein
// Zeiger im Video" gilt von selbst. Sichtbarkeit kommt aus
// FrameInfo.PointerPosition.Visible, sobald LastMouseUpdateTime != 0.
//
// Formen: MONOCHROME (1) ist doppelt hoch (AND-Maske, dann XOR-Maske), ein
// Bit je Punkt; AND=0/XOR=0 schwarz, AND=0/XOR=1 weiss, AND=1/XOR=0
// durchsichtig, AND=1/XOR=1 heisst "invertieren" - naeherungsweise schwarz
// deckend. COLOR (2) ist BGRA mit Deckkraft. MASKED_COLOR (4): Alpha 0 =
// Farbe zeichnen, 0xFF = XOR mit dem Bildschirm - naeherungsweise deckend.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
};

use super::{log, netz};
use crate::protokoll_konst::MSG_CURSOR;

/// Groesser schickt der Host keinen Zeiger; was groesser ist, wird eingepasst.
const MAX: u32 = 256;
/// Hoechstens so oft geht eine Form raus.
const MINDESTABSTAND: Duration = Duration::from_millis(50);

/// "Beim naechsten Mal auf jeden Fall senden" - vom Annahmefaden gesetzt.
static ERZWINGEN: AtomicBool = AtomicBool::new(false);

pub fn neu_senden() {
    ERZWINGEN.store(true, Ordering::Relaxed);
}

pub struct Form {
    pub w: u16,
    pub h: u16,
    pub hx: u16,
    pub hy: u16,
    pub rgba: Vec<u8>,
}

fn fnv(p: &[u8], mut h: u64) -> u64 {
    for &b in p {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Rohdaten der Duplication -> RGBA gerade Deckkraft. `invertiert_gemeldet`
/// vermerkt die Naeherung fuer XOR-Punkte einmal je Sitzung.
pub fn form_umrechnen(info: &DXGI_OUTDUPL_POINTER_SHAPE_INFO, daten: &[u8], xor_gemeldet: &mut bool) -> Option<Form> {
    let (w, pitch) = (info.Width as usize, info.Pitch as usize);
    if w == 0 || info.Height == 0 {
        return None;
    }
    let mut xor_gesehen = false;
    let (h, rgba) = if info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
        let h = (info.Height / 2) as usize;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let bit = 0x80u8 >> (x & 7);
                let and = daten.get(y * pitch + x / 8).map(|b| b & bit != 0).unwrap_or(true);
                let xor = daten.get((y + h) * pitch + x / 8).map(|b| b & bit != 0).unwrap_or(false);
                let i = (y * w + x) * 4;
                match (and, xor) {
                    (false, false) => out[i..i + 4].copy_from_slice(&[0, 0, 0, 255]),
                    (false, true) => out[i..i + 4].copy_from_slice(&[255, 255, 255, 255]),
                    (true, false) => {}
                    (true, true) => {
                        xor_gesehen = true;
                        out[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                    }
                }
            }
        }
        (h, out)
    } else if info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 || info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32 {
        let maskiert = info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32;
        let h = info.Height as usize;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let s = y * pitch + x * 4;
                let (b, g, r, a) = match daten.get(s..s + 4) {
                    Some(p) => (p[0], p[1], p[2], p[3]),
                    None => continue,
                };
                let i = (y * w + x) * 4;
                if maskiert {
                    // 0 = Farbe, 0xFF = XOR mit dem Bildschirm (deckend genaehert).
                    if a == 0xFF {
                        xor_gesehen = true;
                    }
                    out[i..i + 4].copy_from_slice(&[r, g, b, 255]);
                } else {
                    // Die Duplication liefert vormultiplizierte Farben; der
                    // Client will gerade.
                    let (r, g, b) = if a == 0 || a == 255 {
                        (r, g, b)
                    } else {
                        let f = |v: u8| ((v as u32 * 255) / a as u32).min(255) as u8;
                        (f(r), f(g), f(b))
                    };
                    out[i..i + 4].copy_from_slice(&[r, g, b, a]);
                }
            }
        }
        (h, out)
    } else {
        return None;
    };
    if xor_gesehen && !*xor_gemeldet {
        *xor_gemeldet = true;
        log("Zeigerform: Punkte mit Bildschirm-Invertierung werden deckend gezeichnet (Naeherung)");
    }
    // Einpassen statt verwerfen: der Zeiger wird ohnehin gezeichnet, also
    // notfalls kleiner - Hotspot im selben Mass.
    let (mut hx, mut hy) = (info.HotSpot.x.max(0) as usize, info.HotSpot.y.max(0) as usize);
    let (w2, h2, rgba) = if w as u32 > MAX || h as u32 > MAX {
        let f = (MAX as f64 / w as f64).min(MAX as f64 / h as f64);
        let (w2, h2) = ((w as f64 * f).ceil() as usize, (h as f64 * f).ceil() as usize);
        let mut klein = vec![0u8; w2 * h2 * 4];
        for y in 0..h2 {
            let sy = ((y as f64 / f) as usize).min(h - 1);
            for x in 0..w2 {
                let sx = ((x as f64 / f) as usize).min(w - 1);
                klein[(y * w2 + x) * 4..(y * w2 + x) * 4 + 4].copy_from_slice(&rgba[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4]);
            }
        }
        hx = (hx as f64 * f) as usize;
        hy = (hy as f64 * f) as usize;
        (w2, h2, klein)
    } else {
        (w, h, rgba)
    };
    Some(Form { w: w2 as u16, h: h2 as u16, hx: hx.min(w2 - 1) as u16, hy: hy.min(h2 - 1) as u16, rgba })
}

/// Stand der Zeigerform auf dem Aufnahmefaden.
pub struct Zeiger {
    form: Option<Form>,
    sichtbar: bool,
    hash: u64,
    letzte_sendung: Instant,
    gross_gemeldet: bool,
    pub xor_gemeldet: bool,
}

impl Zeiger {
    pub fn neu() -> Zeiger {
        Zeiger { form: None, sichtbar: true, hash: 0, letzte_sendung: Instant::now() - Duration::from_secs(1), gross_gemeldet: false, xor_gemeldet: false }
    }

    pub fn form_setzen(&mut self, info: &DXGI_OUTDUPL_POINTER_SHAPE_INFO, daten: &[u8]) {
        let mut xor = self.xor_gemeldet;
        let f = form_umrechnen(info, daten, &mut xor);
        self.xor_gemeldet = xor;
        if let Some(f) = f {
            if (info.Width > MAX || info.Height > MAX) && !self.gross_gemeldet {
                self.gross_gemeldet = true;
                log(format!("Zeigerform: Zeiger mit {}x{} Punkten wird auf {}x{} verkleinert", info.Width, info.Height, f.w, f.h));
            }
            self.form = Some(f);
        }
    }

    pub fn sichtbar_setzen(&mut self, sichtbar: bool) {
        self.sichtbar = sichtbar;
    }

    /// Bei Aenderung (oder erzwungen) senden, hoechstens 20 je Sekunde.
    pub fn pruefen(&mut self) {
        let Some(f) = self.form.as_ref() else { return };
        let mut h = fnv(&f.rgba, 1469598103934665603);
        h = fnv(&[f.w as u8, f.h as u8, f.hx as u8, f.hy as u8, self.sichtbar as u8, 0], h);
        // Das Zeichen "auf jeden Fall senden" wird immer verbraucht, sonst
        // ginge dieselbe Form beim naechsten Mal noch einmal raus.
        let erzwingen = ERZWINGEN.swap(false, Ordering::Relaxed);
        if h == self.hash && !erzwingen {
            return;
        }
        if !erzwingen && self.letzte_sendung.elapsed() < MINDESTABSTAND {
            // Zu frueh: die Kennung bleibt die alte, der naechste Aufruf sendet.
            return;
        }
        self.hash = h;
        self.letzte_sendung = Instant::now();
        let mut p = Vec::with_capacity(12 + f.rgba.len());
        p.extend_from_slice(&f.w.to_le_bytes());
        p.extend_from_slice(&f.h.to_le_bytes());
        p.extend_from_slice(&f.hx.to_le_bytes());
        p.extend_from_slice(&f.hy.to_le_bytes());
        p.push(self.sichtbar as u8);
        p.push(1);
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&f.rgba);
        netz::send_small(MSG_CURSOR, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::POINT;

    #[test]
    fn monochrom_und_farbe() {
        // 8x2 monochrom: Zeile 0 AND=0 XOR=0 -> schwarz; Zeile 1 AND=1 -> durchsichtig.
        let info = DXGI_OUTDUPL_POINTER_SHAPE_INFO { Type: 1, Width: 8, Height: 4, Pitch: 1, HotSpot: POINT { x: 1, y: 1 } };
        let daten = [0x00, 0xFF, 0x00, 0x00];
        let mut xg = false;
        let f = form_umrechnen(&info, &daten, &mut xg).unwrap();
        assert_eq!((f.w, f.h), (8, 2));
        assert_eq!(&f.rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(f.rgba[8 * 4 + 3], 0);
        // Farbe: vormultipliziert -> gerade.
        let info = DXGI_OUTDUPL_POINTER_SHAPE_INFO { Type: 2, Width: 1, Height: 1, Pitch: 4, HotSpot: POINT { x: 0, y: 0 } };
        let f = form_umrechnen(&info, &[64, 0, 0, 128], &mut xg).unwrap();
        assert_eq!(&f.rgba[0..4], &[0, 0, 127, 128]);
        // Zu gross wird eingepasst.
        let info = DXGI_OUTDUPL_POINTER_SHAPE_INFO { Type: 2, Width: 512, Height: 512, Pitch: 2048, HotSpot: POINT { x: 512, y: 0 } };
        let f = form_umrechnen(&info, &vec![255u8; 512 * 2048], &mut xg).unwrap();
        assert_eq!((f.w, f.h, f.hx), (256, 256, 255));
    }
}
