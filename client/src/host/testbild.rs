// Testbild des Windows-Hosts: Port von host/testbild.m, dieselben
// Ganzzahlen, damit Farbprobe und Benchmark auf beiden Hosts denselben
// Inhalt sehen. Zwoelf Bilder je Schleife, nahtlos.
//
// Oberes Drittel Farbbalken (Weiss, Gelb, Cyan, Gruen, Magenta, Rot, Blau,
// Schwarz im vollen Wertebereich), mittleres Drittel feine Streifen, die je
// Bild um zwei Punkte wandern (links grau, rechts Rot/Blau - da zeigt sich,
// was 4:4:4 wert ist), unteres Drittel ein Verlauf mit einem Quadrat, das
// eine Runde je Schleife laeuft.

pub const N: usize = 12;

pub const BALKEN_CB: [u8; 8] = [128, 16, 166, 54, 202, 90, 240, 128];
pub const BALKEN_CR: [u8; 8] = [128, 146, 16, 34, 222, 240, 110, 128];
pub const BALKEN_Y: [u8; 8] = [235, 210, 170, 145, 106, 81, 41, 16];

/// Ein Bildpunkt des k-ten Bildes, 8 Bit Y/Cb/Cr im vollen Wertebereich.
#[inline]
pub fn punkt(x: i32, y: i32, w: i32, h: i32, k: usize) -> (u8, u8, u8) {
    if y < h / 3 {
        let b = ((x * 8) / w) as usize;
        return (BALKEN_Y[b], BALKEN_CB[b], BALKEN_CR[b]);
    }
    if y < 2 * h / 3 {
        let hell = if (((x + 2 * k as i32) / 2) & 1) != 0 { 220 } else { 40 };
        return if x < w / 2 {
            (hell, 128, 128)
        } else if ((y / 8) & 1) == 0 {
            (hell, 90, 240)
        } else {
            (hell, 240, 110)
        };
    }
    let v = 16 + (x * 219) / (if w > 1 { w - 1 } else { 1 });
    let r = h / 10;
    let wink = 2.0 * std::f64::consts::PI * k as f64 / N as f64;
    let cx = w / 2 + (w as f64 / 4.0 * wink.cos()) as i32;
    let cy = 2 * h / 3 + h / 6 + (h as f64 / 12.0 * wink.sin()) as i32;
    if x >= cx - r / 2 && x < cx + r / 2 && y >= cy - r / 2 && y < cy + r / 2 {
        (200, 60, 220)
    } else {
        (v as u8, 128, 128)
    }
}

/// Drei Ebenen Y, Cb, Cr in voller Aufloesung, 8 Bit.
pub fn yuv444p(w: i32, h: i32, k: usize) -> [Vec<u8>; 3] {
    let n = (w * h) as usize;
    let mut y = vec![0u8; n];
    let mut u = vec![0u8; n];
    let mut v = vec![0u8; n];
    for yy in 0..h {
        for x in 0..w {
            let (a, b, c) = punkt(x, yy, w, h, k);
            let i = (yy * w + x) as usize;
            y[i] = a;
            u[i] = b;
            v[i] = c;
        }
    }
    [y, u, v]
}

/// Dieselben Ebenen in 16 Bit little endian, 8-Bit-Wert oben buendig
/// (v << 8) - wie Apple die 10 Bit ablegt ((v*4) << 6).
pub fn yuv444p16(w: i32, h: i32, k: usize) -> [Vec<u8>; 3] {
    let [y, u, v] = yuv444p(w, h, k);
    let f = |p: Vec<u8>| -> Vec<u8> { p.iter().flat_map(|&b| [0u8, b]).collect() };
    [f(y), f(u), f(v)]
}

/// NV12: Y-Ebene und verschraenkte CbCr-Ebene in halber Aufloesung.
pub fn nv12(w: i32, h: i32, k: usize) -> [Vec<u8>; 2] {
    let n = (w * h) as usize;
    let mut y = vec![0u8; n];
    let mut uv = vec![0u8; ((w / 2) * (h / 2) * 2) as usize];
    for yy in 0..h {
        for x in 0..w {
            y[(yy * w + x) as usize] = punkt(x, yy, w, h, k).0;
        }
    }
    for yy in 0..h / 2 {
        for x in 0..w / 2 {
            let (_, cb, cr) = punkt(2 * x, 2 * yy, w, h, k);
            let i = ((yy * (w / 2) + x) * 2) as usize;
            uv[i] = cb;
            uv[i + 1] = cr;
        }
    }
    [y, uv]
}

/// BGRA, aus Y/Cb/Cr mit BT.709 im vollen Wertebereich zurueckgerechnet -
/// dieselben Koeffizienten wie zeile_rgb im Client. Fuer die Farbprobe: was
/// NVENC daraus macht, muss wieder auf die Balkenwerte fuehren.
pub fn bgra(w: i32, h: i32, k: usize) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for yy in 0..h {
        for x in 0..w {
            let (y, cb, cr) = punkt(x, yy, w, h, k);
            let (y, cb, cr) = (y as f32, cb as f32 - 128.0, cr as f32 - 128.0);
            let r = (y + 1.5748 * cr).round().clamp(0.0, 255.0) as u8;
            let g = (y - 0.1873 * cb - 0.4681 * cr).round().clamp(0.0, 255.0) as u8;
            let b = (y + 1.8556 * cb).round().clamp(0.0, 255.0) as u8;
            let i = ((yy * w + x) * 4) as usize;
            out[i] = b;
            out[i + 1] = g;
            out[i + 2] = r;
            out[i + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balken_wie_auf_dem_mac() {
        // Erster Balken weiss, letzter schwarz, Streifen wandern je Bild um zwei Punkte.
        assert_eq!(punkt(0, 0, 1920, 1080, 0), (235, 128, 128));
        assert_eq!(punkt(1919, 0, 1920, 1080, 0), (16, 128, 128));
        assert_eq!(punkt(0, 500, 1920, 1080, 0).0, punkt(2, 500, 1920, 1080, 11).0);
        assert_eq!(yuv444p(64, 48, 3)[0].len(), 64 * 48);
        assert_eq!(nv12(64, 48, 3)[1].len(), 32 * 24 * 2);
        assert_eq!(bgra(64, 48, 0)[3], 255);
    }
}
