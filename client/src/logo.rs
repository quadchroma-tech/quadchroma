// Das Programmsymbol: im Programm gezeichnet, ohne Ressourcendatei und
// ohne build.rs.
//
// Motiv: vier farbige Quadrate auf einer dunklen Kachel - "Quad Chroma",
// vier Felder in den Farben der Oberflaeche (ui.rs). Gebraucht wird es
//   - als Fenstersymbol (winit, Icon::from_rgba),
//   - als .ico-Datei im Ablageordner des Clients, auf die jede
//     Desktop-Verknuepfung zeigt (SetIconLocation) - die exe selbst hat kein
//     eingebettetes Symbol, sonst zeigte die Verknuepfung das Standardsymbol,
//   - spaeter als Symbol im Infobereich.
//
// Das ICO-Format wird hier von Hand geschrieben: kleine Groessen als
// 32-Bit-BMP (so liest sie jede Windows-Fassung), 256 px als PNG. Das PNG
// kommt ohne Kompressionsbibliothek aus: "stored"-Deflate, also Bloecke ohne
// Kompression, dazu CRC-32 und Adler-32 von Hand.

// Auf dem Mac gibt es keine Verknuepfung: dort bleibt ein Teil ungenutzt
// (Tests laufen trotzdem auf beiden Plattformen).
#![cfg_attr(not(windows), allow(dead_code))]

use crate::ui;

/// Die Groessen in der .ico-Datei. 256 steht als PNG darin, der Rest als BMP.
pub const ICO_GROESSEN: [u32; 6] = [16, 20, 24, 32, 48, 256];

/// Ab dieser Kantenlaenge steht ein Eintrag als PNG in der .ico-Datei.
const PNG_AB: u32 = 256;

/// Farbe der Kachel unter den vier Feldern: der Grund der Oberflaeche.
const KACHEL: u32 = ui::BG;

/// Die vier Felder, im Uhrzeigersinn ab oben links.
const FELDER: [u32; 4] = [ui::CYAN, ui::MAGENTA, ui::TEXT, ui::AMBER];

/// Das Logo als RGBA (je Bildpunkt vier Byte, Zeilen von oben nach unten,
/// Alpha nicht vormultipliziert), `groesse` mal `groesse` Bildpunkte.
///
/// Die Felder liegen auf ganzen Bildpunkten, damit auch 16 px scharf
/// bleiben; nur die abgerundeten Ecken werden geglaettet (4x4 Abtastungen
/// je Bildpunkt).
pub fn rgba(groesse: u32) -> Vec<u8> {
    let g = groesse.max(1);
    let n = g as i64;
    // Rand, Luecke und Feldgroesse in ganzen Bildpunkten. Die Luecke waechst
    // mit, bis die Summe aufgeht: 2*Rand + 2*Feld + Luecke = Groesse.
    let rand = (n / 8).max(1);
    let mut luecke = (n / 16).max(2).min(n);
    if (n - 2 * rand - luecke) % 2 != 0 {
        luecke += 1;
    }
    let feld = ((n - 2 * rand - luecke) / 2).max(1);
    let r_kachel = (g as f32 * 0.19).max(1.0);
    let r_feld = (g as f32 * 0.035).max(0.0);
    let links = [rand, rand + feld + luecke];

    let mut out = vec![0u8; (g * g * 4) as usize];
    const STUFEN: i32 = 4;
    for y in 0..g {
        for x in 0..g {
            // Farbe vormultipliziert aufsummieren, am Ende durch Deckung teilen.
            let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..STUFEN {
                for sx in 0..STUFEN {
                    let px = x as f32 + (sx as f32 + 0.5) / STUFEN as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / STUFEN as f32;
                    if !in_rundem_rechteck(px, py, 0.0, 0.0, g as f32, g as f32, r_kachel) {
                        continue;
                    }
                    let mut farbe = KACHEL;
                    for (i, f) in FELDER.iter().enumerate() {
                        // 0 oben links, 1 oben rechts, 2 unten rechts, 3 unten links
                        let (fx, fy) = match i {
                            0 => (links[0], links[0]),
                            1 => (links[1], links[0]),
                            2 => (links[1], links[1]),
                            _ => (links[0], links[1]),
                        };
                        if in_rundem_rechteck(px, py, fx as f32, fy as f32, feld as f32, feld as f32, r_feld) {
                            farbe = *f;
                        }
                    }
                    sr += (farbe >> 16) & 255;
                    sg += (farbe >> 8) & 255;
                    sb += farbe & 255;
                    sa += 1;
                }
            }
            let o = ((y * g + x) * 4) as usize;
            if sa > 0 {
                out[o] = (sr / sa) as u8;
                out[o + 1] = (sg / sa) as u8;
                out[o + 2] = (sb / sa) as u8;
                out[o + 3] = ((sa * 255) / (STUFEN * STUFEN) as u32) as u8;
            }
        }
    }
    out
}

/// Liegt der Punkt in einem Rechteck mit abgerundeten Ecken (Radius r)?
fn in_rundem_rechteck(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return true;
    }
    // Abstand zur naechsten Eckmitte, nur in den Eckfeldern von Belang.
    let cx = px.clamp(x + r, x + w - r);
    let cy = py.clamp(y + r, y + h - r);
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy <= r * r
}

// ------------------------------------------------------------------- PNG

/// CRC-32 nach ISO 3309 (Polynom 0xEDB88320), wie PNG sie verlangt. Die
/// Bloecke rechnen fortlaufend ueber Art und Daten (`crc32_weiter`); diese
/// Form dient den Tests.
#[cfg(test)]
pub fn crc32(daten: &[u8]) -> u32 {
    crc32_weiter(0xffff_ffff, daten) ^ 0xffff_ffff
}

fn crc32_weiter(mut crc: u32, daten: &[u8]) -> u32 {
    for &b in daten {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    crc
}

/// Adler-32 ueber die unkomprimierten Daten, der Abschluss eines zlib-Stroms.
pub fn adler32(daten: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in daten {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Ein zlib-Strom aus "stored"-Bloecken (keine Kompression, je Block
/// hoechstens 65535 Byte).
pub fn zlib_stored(daten: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(daten.len() + daten.len() / 65535 * 5 + 16);
    // CMF 0x78: Deflate mit 32-KiB-Fenster; FLG 0x01: keine Voreinstellung,
    // Pruefbits so, dass 0x7801 durch 31 teilbar ist.
    out.extend_from_slice(&[0x78, 0x01]);
    let mut stuecke = daten.chunks(65535).peekable();
    if stuecke.peek().is_none() {
        // Auch ein leerer Strom braucht einen (letzten) Block.
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(s) = stuecke.next() {
        let letzter = stuecke.peek().is_none();
        out.push(if letzter { 1 } else { 0 }); // BFINAL, BTYPE 00
        let len = s.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(s);
    }
    out.extend_from_slice(&adler32(daten).to_be_bytes());
    out
}

fn png_block(out: &mut Vec<u8>, art: &[u8; 4], daten: &[u8]) {
    out.extend_from_slice(&(daten.len() as u32).to_be_bytes());
    out.extend_from_slice(art);
    out.extend_from_slice(daten);
    let crc = crc32_weiter(crc32_weiter(0xffff_ffff, art), daten) ^ 0xffff_ffff;
    out.extend_from_slice(&crc.to_be_bytes());
}

/// RGBA (nicht vormultipliziert, Zeilen von oben) als PNG, Farbtyp 6, 8 Bit.
pub fn png(breite: u32, hoehe: u32, rgba: &[u8]) -> Vec<u8> {
    let zeile = breite as usize * 4;
    let mut roh = Vec::with_capacity((zeile + 1) * hoehe as usize);
    for z in rgba.chunks(zeile).take(hoehe as usize) {
        roh.push(0); // Filter "keiner"
        roh.extend_from_slice(z);
    }
    let mut out = Vec::with_capacity(roh.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&breite.to_be_bytes());
    ihdr.extend_from_slice(&hoehe.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_block(&mut out, b"IHDR", &ihdr);
    png_block(&mut out, b"IDAT", &zlib_stored(&roh));
    png_block(&mut out, b"IEND", &[]);
    out
}

// ------------------------------------------------------------------- ICO

/// Ein Eintrag als 32-Bit-BMP, wie ihn ICO erwartet: BITMAPINFOHEADER mit
/// doppelter Hoehe (Farb- und Maskenteil), BGRA von unten nach oben, dahinter
/// die 1-Bit-UND-Maske (gesetzt = durchsichtig), Zeilen auf 32 Bit gefuellt.
pub fn ico_bmp(groesse: u32, rgba: &[u8]) -> Vec<u8> {
    let g = groesse as usize;
    let masken_zeile = g.div_ceil(32) * 4;
    let farbe = g * g * 4;
    let maske = masken_zeile * g;
    let mut out = Vec::with_capacity(40 + farbe + maske);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(groesse as i32).to_le_bytes());
    out.extend_from_slice(&((groesse * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((farbe + maske) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // Aufloesung, Farbtabelle: keine
    for y in (0..g).rev() {
        for x in 0..g {
            let o = (y * g + x) * 4;
            out.extend_from_slice(&[rgba[o + 2], rgba[o + 1], rgba[o], rgba[o + 3]]);
        }
    }
    for y in (0..g).rev() {
        let mut z = vec![0u8; masken_zeile];
        for x in 0..g {
            if rgba[(y * g + x) * 4 + 3] == 0 {
                z[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&z);
    }
    out
}

/// Die ganze .ico-Datei: Kopf (6 Byte), je Groesse ein Verzeichniseintrag
/// (16 Byte), danach die Bilder in derselben Reihenfolge.
pub fn ico(groessen: &[u32]) -> Vec<u8> {
    let bilder: Vec<(u32, Vec<u8>)> = groessen
        .iter()
        .map(|&g| {
            let rgba = rgba(g);
            (g, if g >= PNG_AB { png(g, g, &rgba) } else { ico_bmp(g, &rgba) })
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // 1 = Symbol
    out.extend_from_slice(&(bilder.len() as u16).to_le_bytes());
    let mut versatz = 6 + 16 * bilder.len() as u32;
    for (g, b) in &bilder {
        // 256 steht als 0 im Byte.
        let kante = if *g >= 256 { 0 } else { *g as u8 };
        out.extend_from_slice(&[kante, kante, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // Ebenen
        out.extend_from_slice(&32u16.to_le_bytes()); // Bit je Bildpunkt
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&versatz.to_le_bytes());
        versatz += b.len() as u32;
    }
    for (_, b) in &bilder {
        out.extend_from_slice(b);
    }
    out
}

/// Die .ico-Datei im Ablageordner des Clients schreiben bzw. erneuern und
/// ihren Pfad liefern. Erst in eine Nachbardatei, dann umbenennen: eine
/// Verknuepfung, die gerade gezeichnet wird, sieht nie eine halbe Datei.
pub fn ico_schreiben() -> Result<std::path::PathBuf, String> {
    let ziel = crate::einstellungen::datei_pfad("quadchroma.ico")
        .ok_or_else(|| "kein Ablageordner fuer das Symbol".to_string())?;
    let neu = ziel.with_extension("ico.neu");
    std::fs::write(&neu, ico(&ICO_GROESSEN)).map_err(|e| format!("{}: {e}", neu.display()))?;
    std::fs::rename(&neu, &ziel).map_err(|e| {
        let _ = std::fs::remove_file(&neu);
        format!("{}: {e}", ziel.display())
    })?;
    Ok(ziel)
}

/// Fenstersymbol in einer Groesse (winit rechnet es fuer die Titelleiste um).
pub fn fenster_symbol(groesse: u32) -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(rgba(groesse), groesse, groesse).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_bei(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn u32_bei(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
    fn u32_be(b: &[u8], o: usize) -> u32 {
        u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }

    /// Die bekannten Pruefwerte: CRC-32("123456789") und Adler-32("Wikipedia").
    #[test]
    fn pruefsummen() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(b""), 1);
        // Der Kopf eines zlib-Stroms muss durch 31 teilbar sein.
        let z = zlib_stored(b"abc");
        assert_eq!(((z[0] as u16) << 8 | z[1] as u16) % 31, 0);
    }

    /// "Stored"-Bloecke zurueck auspacken: Laenge, Einerkomplement, Daten,
    /// Adler am Ende - ueber mehrere Bloecke und fuer den leeren Strom.
    fn auspacken(z: &[u8]) -> Vec<u8> {
        let mut o = 2;
        let mut out = Vec::new();
        loop {
            let kopf = z[o];
            assert_eq!(kopf & 0b110, 0, "nur stored-Bloecke");
            let len = u16_bei(z, o + 1);
            assert_eq!(!len, u16_bei(z, o + 3));
            out.extend_from_slice(&z[o + 5..o + 5 + len as usize]);
            o += 5 + len as usize;
            if kopf & 1 == 1 {
                break;
            }
        }
        assert_eq!(u32_be(z, o), adler32(&out));
        assert_eq!(o + 4, z.len());
        out
    }

    #[test]
    fn zlib_hin_und_zurueck() {
        let gross: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(auspacken(&zlib_stored(&gross)), gross);
        assert_eq!(auspacken(&zlib_stored(b"")), b"");
        assert_eq!(auspacken(&zlib_stored(&gross[..65535])), &gross[..65535]);
    }

    /// PNG: Signatur, IHDR, IDAT mit genau den Zeilen samt Filterbyte, IEND,
    /// jede Pruefsumme stimmt.
    #[test]
    fn png_aufbau() {
        let rgba = rgba(256);
        let p = png(256, 256, &rgba);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let mut o = 8;
        let mut arten = Vec::new();
        let mut idat = Vec::new();
        while o < p.len() {
            let len = u32_be(&p, o) as usize;
            let art = &p[o + 4..o + 8];
            let daten = &p[o + 8..o + 8 + len];
            assert_eq!(u32_be(&p, o + 8 + len), crc32(&p[o + 4..o + 8 + len]), "CRC von {:?}", art);
            if art == b"IHDR" {
                assert_eq!(u32_be(daten, 0), 256);
                assert_eq!(u32_be(daten, 4), 256);
                assert_eq!(&daten[8..], &[8, 6, 0, 0, 0]);
            }
            if art == b"IDAT" {
                idat.extend_from_slice(daten);
            }
            arten.push(String::from_utf8_lossy(art).to_string());
            o += 12 + len;
        }
        assert_eq!(arten, ["IHDR", "IDAT", "IEND"]);
        let roh = auspacken(&idat);
        assert_eq!(roh.len(), 256 * (256 * 4 + 1));
        for (y, z) in roh.chunks(256 * 4 + 1).enumerate() {
            assert_eq!(z[0], 0);
            assert_eq!(&z[1..], &rgba[y * 1024..(y + 1) * 1024]);
        }
    }

    /// ICO: Kopf, je Groesse ein Eintrag mit richtiger Kante, Laenge und
    /// Versatz; die Bilder liegen lueckenlos hintereinander bis zum Ende;
    /// BMP-Eintraege mit doppelter Hoehe und Maske, 256 als PNG.
    #[test]
    fn ico_kopf_und_eintraege() {
        let d = ico(&ICO_GROESSEN);
        assert_eq!(u16_bei(&d, 0), 0);
        assert_eq!(u16_bei(&d, 2), 1);
        assert_eq!(u16_bei(&d, 4) as usize, ICO_GROESSEN.len());
        let mut erwartet = 6 + 16 * ICO_GROESSEN.len() as u32;
        for (i, &g) in ICO_GROESSEN.iter().enumerate() {
            let e = 6 + 16 * i;
            let kante = if g == 256 { 0 } else { g as u8 };
            assert_eq!(d[e], kante, "Breite {g}");
            assert_eq!(d[e + 1], kante, "Hoehe {g}");
            assert_eq!(d[e + 2], 0);
            assert_eq!(d[e + 3], 0);
            assert_eq!(u16_bei(&d, e + 4), 1);
            assert_eq!(u16_bei(&d, e + 6), 32);
            let len = u32_bei(&d, e + 8);
            let versatz = u32_bei(&d, e + 12);
            assert_eq!(versatz, erwartet, "Versatz {g}");
            let bild = &d[versatz as usize..(versatz + len) as usize];
            if g >= 256 {
                assert_eq!(&bild[..4], &[0x89, b'P', b'N', b'G']);
            } else {
                let masken_zeile = (g as usize).div_ceil(32) * 4;
                assert_eq!(len as usize, 40 + (g * g * 4) as usize + masken_zeile * g as usize, "Laenge {g}");
                assert_eq!(u32_bei(bild, 0), 40);
                assert_eq!(u32_bei(bild, 4), g);
                assert_eq!(u32_bei(bild, 8), 2 * g);
                assert_eq!(u16_bei(bild, 12), 1);
                assert_eq!(u16_bei(bild, 14), 32);
                assert_eq!(u32_bei(bild, 16), 0);
            }
            erwartet += len;
        }
        assert_eq!(erwartet as usize, d.len(), "Bilder lueckenlos bis zum Ende");
    }

    /// BMP-Eintrag: unterste Zeile zuerst, BGRA, Maske gesetzt genau dort,
    /// wo das Bild durchsichtig ist.
    #[test]
    fn bmp_eintrag_zeilen_und_maske() {
        let g = 16u32;
        let rgba = rgba(g);
        let b = ico_bmp(g, &rgba);
        // Oben links ist die abgerundete Ecke: durchsichtig. In der Datei
        // steht die oberste Zeile zuletzt.
        let letzte_zeile = 40 + (g * (g - 1) * 4) as usize;
        assert_eq!(rgba[3], 0);
        assert_eq!(b[letzte_zeile + 3], 0);
        let maske = 40 + (g * g * 4) as usize;
        let masken_zeile = 4;
        // Oberste Bildzeile = letzte Maskenzeile, Bit 7 des ersten Bytes.
        assert_eq!(b[maske + masken_zeile * (g as usize - 1)] & 0x80, 0x80);
        // Mitte der Kachel: deckend, also kein Maskenbit.
        let (x, y) = (8usize, 8usize);
        assert_eq!(rgba[(y * 16 + x) * 4 + 3], 255);
        let zeile_in_datei = 15 - y;
        assert_eq!(b[maske + masken_zeile * zeile_in_datei + x / 8] & (0x80 >> (x % 8)), 0);
        // Farbe umgedreht: BGRA.
        let o = (y * 16 + x) * 4;
        let od = 40 + (zeile_in_datei * 16 + x) * 4;
        assert_eq!([b[od], b[od + 1], b[od + 2], b[od + 3]], [rgba[o + 2], rgba[o + 1], rgba[o], rgba[o + 3]]);
    }

    /// Das Motiv: in jeder Groesse vier Felder in den vier Farben, die Mitte
    /// jedes Feldes deckend in genau seiner Farbe, die Ecken durchsichtig.
    #[test]
    fn logo_vier_felder() {
        for g in ICO_GROESSEN {
            let b = rgba(g);
            assert_eq!(b.len(), (g * g * 4) as usize);
            let farbe = |x: u32, y: u32| {
                let o = ((y * g + x) * 4) as usize;
                ((b[o] as u32) << 16 | (b[o + 1] as u32) << 8 | b[o + 2] as u32, b[o + 3])
            };
            assert_eq!(farbe(0, 0).1, 0, "Ecke bei {g}");
            let v = g / 4;
            let d = g - 1 - g / 4;
            assert_eq!(farbe(v, v), (ui::CYAN, 255), "oben links bei {g}");
            assert_eq!(farbe(d, v), (ui::MAGENTA, 255), "oben rechts bei {g}");
            assert_eq!(farbe(d, d), (ui::TEXT, 255), "unten rechts bei {g}");
            assert_eq!(farbe(v, d), (ui::AMBER, 255), "unten links bei {g}");
            // Zwischen den Feldern: die Kachel.
            assert_eq!(farbe(g / 2, g / 2), (KACHEL, 255), "Mitte bei {g}");
        }
    }
}
