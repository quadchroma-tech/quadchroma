// Das Programmsymbol: im Programm gezeichnet, ohne Bilddatei im Quelltext.
//
// Motiv: vier farbige Quadrate auf einer dunklen Kachel - "Quad Chroma",
// vier Felder in den Farben der Oberflaeche (ui.rs). Gebraucht wird es
//   - als Fenstersymbol (winit, Icon::from_rgba),
//   - als .ico-Datei im Ablageordner des Clients, auf die jede
//     Desktop-Verknuepfung zeigt (SetIconLocation),
//   - als eingebettetes Symbol der Windows-exe: res/quadchroma.ico im
//     Repository sind dieselben Bytes (der Test ico_schreiben_res erzeugt die
//     Datei, ico_res_gleich_logo wacht darueber), client/build.rs bindet sie
//     ueber res/quadchroma.rc ein. Die Verknuepfung zeigt weiter auf die
//     .ico im Ablageordner - das geht unabhaengig vom Symbol in der exe,
//   - als Symbol im Infobereich (Windows, tray_win.rs) und - einfarbig als
//     Vorlagenbild, siehe `vorlage` - in der Menueleiste des Macs
//     (tray_mac.rs),
//   - als Programmsymbol der Mac-App (Finder, Dock, Cmd+Tab) in eigener
//     Form nach Apples Raster, siehe `mac_symbol`: res/AppIcon.icns im
//     Repository, das Makefile legt es ins Bundle.
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

/// Das Logo als einfarbige Vorlage fuer die Menueleiste des Macs: nur die
/// vier Felder, schwarz mit Deckung, ohne Kachel und ohne Farben. macOS
/// faerbt ein Vorlagenbild (setTemplate:YES) selbst passend zu hellem und
/// dunklem Hintergrund und wertet nur die Deckung aus. Der Rand ist
/// schmaler als bei `rgba`, weil keine Kachel das Motiv umgibt. Geglaettet
/// wie `rgba` (4x4 Abtastungen je Bildpunkt).
/// Nur auf dem Mac gebraucht (und in den Tests beider Plattformen).
#[cfg(any(target_os = "macos", test))]
pub fn vorlage(groesse: u32) -> Vec<u8> {
    let g = groesse.max(1);
    let n = g as i64;
    let rand = (n / 12).max(1);
    let mut luecke = (n / 9).max(1).min(n);
    if (n - 2 * rand - luecke) % 2 != 0 {
        luecke += 1;
    }
    let feld = ((n - 2 * rand - luecke) / 2).max(1);
    let r_feld = g as f32 * 0.06;
    let links = [rand, rand + feld + luecke];

    let mut out = vec![0u8; (g * g * 4) as usize];
    const STUFEN: u32 = 4;
    for y in 0..g {
        for x in 0..g {
            let mut treffer = 0u32;
            for sy in 0..STUFEN {
                for sx in 0..STUFEN {
                    let px = x as f32 + (sx as f32 + 0.5) / STUFEN as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / STUFEN as f32;
                    let drin = links.iter().any(|&fy| {
                        links.iter().any(|&fx| in_rundem_rechteck(px, py, fx as f32, fy as f32, feld as f32, feld as f32, r_feld))
                    });
                    treffer += drin as u32;
                }
            }
            // Farbe bleibt schwarz (0, 0, 0); nur die Deckung zaehlt.
            out[((y * g + x) * 4 + 3) as usize] = ((treffer * 255) / (STUFEN * STUFEN)) as u8;
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

// ------------------------------------------------------------- Mac-Symbol
//
// Das Programmsymbol der Mac-App (Finder, Dock, Cmd+Tab) im Stil von macOS 26:
// Apples Raster fuer Programmsymbole, 1024er Leinwand, darauf eine Kachel
// 824 x 824 ab (100, 100) mit "stetig" gerundeten Ecken, darunter ein weicher
// Schatten; auf der dunklen Kachel die vier Felder des Logos. Im Bundle liegt
// es als res/AppIcon.icns (iconutil); icns_schreiben_res erzeugt die Datei
// neu, icns_res_gleich_logo wacht darueber, dass sie zum Code hier passt.
// Im Programm selbst wird nichts davon gebraucht - daher nur in den Tests.
//
// Gerechnet wird nur mit + - * / und sqrt (IEEE-genau): die Bildpunkte sind
// auf jeder Plattform bitgleich, sonst liefe die Pruefsumme in
// icns_res_gleich_logo unter Windows auseinander. Kein powf, kein exp.

#[cfg(test)]
pub use mac::{mac_symbol, MAC_GROESSEN};

#[cfg(test)]
mod mac {
    use super::{in_rundem_rechteck, FELDER};
    use crate::ui;

    /// Kantenlaenge der Vorlage, auf die sich alle MAC_*-Masse beziehen.
    const MAC_RASTER: f32 = 1024.0;
    /// Die Kachel: Rand bis zur Kachel, Kantenlaenge.
    const MAC_KACHEL_RAND: f32 = 100.0;
    const MAC_KACHEL: f32 = 824.0;
    /// Ausdehnung einer Ecke ab der Kante. Die Ecke ist ein Superellipsen-
    /// Viertel mit Exponent 2,5 (u^2,5 + v^2,5 <= 1, x^2,5 = x*x*sqrt(x)): so
    /// laeuft sie wie Apples Maske ohne Knick in die Gerade ueber. An einem
    /// Systemsymbol unter macOS 27 nachgemessen: gerade Kante ab 350, halbe
    /// Deckung auf der Diagonale bei 162; dieses Viertel trifft beides auf 2 px.
    const MAC_ECKE: f32 = 258.0;
    /// Die vier Felder: Kante und Luecke (zusammen 512, mittig auf der Kachel),
    /// Eckradius als Anteil der Feldkante.
    const MAC_FELD: f32 = 232.0;
    const MAC_LUECKE: f32 = 48.0;
    const MAC_FELD_RUNDUNG: f32 = 0.12;
    /// Glanz der Felder: oben zu diesem Anteil nach Weiss gemischt, bis zur
    /// Feldmitte auslaufend; die untere Haelfte hat genau die Feldfarbe.
    const MAC_FELD_GLANZ: f32 = 0.22;
    /// Grund der Kachel: oben etwas heller, unten der Grund der Oberflaeche.
    const MAC_GRUND_OBEN: u32 = 0x161b28;
    /// Lichtkante (Glas): Breite in Rasterpunkten (mindestens ein halber
    /// Bildpunkt, damit auch 16 und 32 px auf dunklem Grund eine Kante haben),
    /// Deckung des Weiss ringsum und zusaetzlich zu den Ecken oben rechts und
    /// unten links hin.
    const MAC_KANTE: f32 = 5.0;
    const MAC_KANTE_GRUND: f32 = 0.10;
    const MAC_KANTE_GLANZ: f32 = 0.45;
    /// Schatten der Kachel (wie am Systemsymbol gemessen): Versatz nach unten,
    /// Radius je Kastenfilter (drei Durchgaenge je Richtung, etwa Gauss mit
    /// sigma 15), Deckung.
    const MAC_SCHATTEN_DY: f32 = 8.0;
    const MAC_SCHATTEN_WEICH: f32 = 15.0;
    const MAC_SCHATTEN_DECKUNG: f32 = 0.25;

    /// Die Groessen des Mac-Symbols in der .iconset-Mappe (je einmal).
    pub const MAC_GROESSEN: [u32; 7] = [16, 32, 64, 128, 256, 512, 1024];

    /// Liegt der Punkt in der Kachel (links oben bei `rand`, Kante `kante`,
    /// Eckausdehnung `ecke`)?
    fn in_mac_kachel(px: f32, py: f32, rand: f32, kante: f32, ecke: f32) -> bool {
        if px < rand || py < rand || px >= rand + kante || py >= rand + kante {
            return false;
        }
        // Wie weit der Punkt in einem Eckfeld liegt, als Anteil der Ecke.
        let u = (rand + ecke - px).max(px - (rand + kante - ecke)).max(0.0) / ecke;
        let v = (rand + ecke - py).max(py - (rand + kante - ecke)).max(0.0) / ecke;
        u * u * u.sqrt() + v * v * v.sqrt() <= 1.0
    }

    /// `a` nach `b` gemischt, Anteil `t` (0..1), je Kanal 0..255.
    fn mischen(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    }

    fn kanaele(farbe: u32) -> [f32; 3] {
        [((farbe >> 16) & 255) as f32, ((farbe >> 8) & 255) as f32, (farbe & 255) as f32]
    }

    /// Das Mac-Symbol als RGBA wie `rgba` (nicht vormultipliziert, Zeilen von
    /// oben), `groesse` mal `groesse` Bildpunkte. Die Felder liegen wie in
    /// `rgba` auf ganzen Bildpunkten (16 px bleiben scharf), Kachel und
    /// Feldecken sind geglaettet (4x4 Abtastungen je Bildpunkt).
    pub fn mac_symbol(groesse: u32) -> Vec<u8> {
        let g = groesse.max(1);
        let n = g as usize;
        let s = g as f32 / MAC_RASTER;
        let rand = MAC_KACHEL_RAND * s;
        let kante = MAC_KACHEL * s;
        let ecke = MAC_ECKE * s;
        let licht = (MAC_KANTE * s).max(0.5);
        let mitte = g as f32 / 2.0;
        // Felder und Luecke in ganzen Bildpunkten; die Luecke waechst um einen,
        // wenn sonst kein ganzzahliger Rand bliebe (wie in `rgba`).
        let gi = g as i64;
        let feld = ((MAC_FELD * s).round() as i64).max(1);
        let mut luecke = ((MAC_LUECKE * s).round() as i64).max(1);
        if (gi - 2 * feld - luecke) % 2 != 0 {
            luecke += 1;
        }
        let links0 = (gi - 2 * feld - luecke) / 2;
        let links = [links0 as f32, (links0 + feld + luecke) as f32];
        let feld = feld as f32;
        let r_feld = feld * MAC_FELD_RUNDUNG;
        let (oben, unten) = (kanaele(MAC_GRUND_OBEN), kanaele(ui::BG));
        let weiss = [255.0; 3];

        // Farbe eines Abtastpunkts in der Kachel.
        let farbe_bei = |px: f32, py: f32| -> [f32; 3] {
            let mut c = mischen(oben, unten, ((py - rand) / kante).clamp(0.0, 1.0));
            for (i, f) in FELDER.iter().enumerate() {
                // 0 oben links, 1 oben rechts, 2 unten rechts, 3 unten links
                let (fx, fy) = match i {
                    0 => (links[0], links[0]),
                    1 => (links[1], links[0]),
                    2 => (links[1], links[1]),
                    _ => (links[0], links[1]),
                };
                if in_rundem_rechteck(px, py, fx, fy, feld, feld, r_feld) {
                    let t = (py - fy) / feld;
                    let rest = (1.0 - 2.0 * t).max(0.0);
                    let glanz = rest * rest * MAC_FELD_GLANZ;
                    c = mischen(kanaele(*f), weiss, glanz);
                }
            }
            if !in_mac_kachel(px, py, rand + licht, kante - 2.0 * licht, ecke - licht) {
                // Lichtkante; am hellsten zu den Ecken oben rechts und unten
                // links hin (p = +-1 auf der Diagonale dorthin, 0 quer dazu).
                let (dx, dy) = (px - mitte, py - mitte);
                let r2 = dx * dx + dy * dy;
                let p2 = if r2 > 0.0 { (dx - dy) * (dx - dy) / (2.0 * r2) } else { 0.0 };
                c = mischen(c, weiss, MAC_KANTE_GRUND + MAC_KANTE_GLANZ * p2 * p2);
            }
            c
        };

        // Kachel: vormultiplizierte Farbe und Deckung je Bildpunkt; Schatten:
        // Treffer der nach unten versetzten Kachel (ganzzahlig, fuer den Filter).
        const STUFEN: u32 = 4;
        const PROBEN: u32 = STUFEN * STUFEN;
        let dy_schatten = MAC_SCHATTEN_DY * s;
        let mut kachel = vec![[0f32; 4]; n * n];
        let mut schatten = vec![0u64; n * n];
        for y in 0..n {
            for x in 0..n {
                let mut summe = [0f32; 4];
                let mut treffer_schatten = 0u64;
                for sy in 0..STUFEN {
                    for sx in 0..STUFEN {
                        let px = x as f32 + (sx as f32 + 0.5) / STUFEN as f32;
                        let py = y as f32 + (sy as f32 + 0.5) / STUFEN as f32;
                        if in_mac_kachel(px, py - dy_schatten, rand, kante, ecke) {
                            treffer_schatten += 1;
                        }
                        if in_mac_kachel(px, py, rand, kante, ecke) {
                            let c = farbe_bei(px, py);
                            summe = [summe[0] + c[0], summe[1] + c[1], summe[2] + c[2], summe[3] + 1.0];
                        }
                    }
                }
                kachel[y * n + x] = summe.map(|v| v / PROBEN as f32);
                schatten[y * n + x] = treffer_schatten;
            }
        }

        // Schatten weich: je Richtung dreimal ein Kastenfilter, ganzzahlig und
        // ohne Zwischenteilung (genau), am Ende einmal geteilt.
        let radius = (MAC_SCHATTEN_WEICH * s).round() as usize;
        let mut teiler = PROBEN as u64;
        if radius > 0 {
            let mut hilfe = vec![0u64; n * n];
            for _ in 0..3 {
                kasten(&schatten, &mut hilfe, n, radius, 1, n);
                kasten(&hilfe, &mut schatten, n, radius, n, 1);
                teiler *= ((2 * radius + 1) * (2 * radius + 1)) as u64;
            }
        }

        // Kachel ueber dem (schwarzen) Schatten.
        let mut out = vec![0u8; n * n * 4];
        let byte = |v: f32| (v.clamp(0.0, 255.0) + 0.5) as u8;
        for i in 0..n * n {
            let [r, gr, b, a_kachel] = kachel[i];
            let a_schatten = schatten[i] as f32 / teiler as f32 * MAC_SCHATTEN_DECKUNG;
            let a = a_kachel + a_schatten * (1.0 - a_kachel);
            if a > 0.0 {
                out[i * 4] = byte(r / a);
                out[i * 4 + 1] = byte(gr / a);
                out[i * 4 + 2] = byte(b / a);
                out[i * 4 + 3] = byte(a * 255.0);
            }
        }
        out
    }

    /// Ein Kastenfilter (Summe ueber 2*radius+1 Nachbarn, ausserhalb 0) ueber
    /// `n` Zeilen bzw. Spalten: `schritt` ist der Abstand zweier Nachbarn,
    /// `zeile` der zweier Zeilen (waagrecht: 1 und n, senkrecht: n und 1).
    fn kasten(ein: &[u64], aus: &mut [u64], n: usize, radius: usize, schritt: usize, zeile: usize) {
        for z in 0..n {
            let basis = z * zeile;
            let mut summe: u64 = (0..=radius.min(n - 1)).map(|k| ein[basis + k * schritt]).sum();
            for k in 0..n {
                aus[basis + k * schritt] = summe;
                if k + radius + 1 < n {
                    summe += ein[basis + (k + radius + 1) * schritt];
                }
                if k >= radius {
                    summe -= ein[basis + (k - radius) * schritt];
                }
            }
        }
    }
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
/// ihren Pfad liefern (ico_schreiben_nach).
pub fn ico_schreiben() -> Result<std::path::PathBuf, String> {
    let ziel = crate::einstellungen::datei_pfad("quadchroma.ico")
        .ok_or_else(|| "kein Ablageordner fuer das Symbol".to_string())?;
    ico_schreiben_nach(&ziel)
}

/// Die .ico-Datei nach `ziel` schreiben bzw. erneuern. Erst in eine
/// Nachbardatei (je Prozess ein eigener Name: Fenster und Befehlszeile
/// koennen gleichzeitig anlegen), dann umbenennen: eine Verknuepfung, die
/// gerade gezeichnet wird, sieht nie eine halbe Datei. Scheitert das
/// Erneuern (etwa weil Explorer die Datei gerade liest), taugt die
/// vorhandene weiter - sie ist dasselbe Logo -, statt dass die Verknuepfung
/// das Standardsymbol zeigt. Err nur, wenn es danach keine Datei gibt.
pub fn ico_schreiben_nach(ziel: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let neu = ziel.with_extension(format!("ico.{}.neu", std::process::id()));
    let erneuert = std::fs::write(&neu, ico(&ICO_GROESSEN))
        .map_err(|e| format!("{}: {e}", neu.display()))
        .and_then(|()| std::fs::rename(&neu, ziel).map_err(|e| format!("{}: {e}", ziel.display())));
    match erneuert {
        Ok(()) => Ok(ziel.to_path_buf()),
        Err(e) => {
            let _ = std::fs::remove_file(&neu);
            if std::fs::symlink_metadata(ziel).is_ok_and(|m| m.is_file() && m.len() > 0) {
                Ok(ziel.to_path_buf())
            } else {
                Err(e)
            }
        }
    }
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

    /// Die Vorlage fuer die Menueleiste: nur Deckung, schwarz; die vier
    /// Felder deckend, Ecken, Rand und das Kreuz dazwischen durchsichtig.
    #[test]
    fn vorlage_einfarbig_vier_felder() {
        for g in [18u32, 36, 54] {
            let b = vorlage(g);
            assert_eq!(b.len(), (g * g * 4) as usize);
            assert!(b.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0), "nur schwarz bei {g}");
            let deckung = |x: u32, y: u32| b[((y * g + x) * 4 + 3) as usize];
            assert_eq!(deckung(0, 0), 0, "Ecke bei {g}");
            assert_eq!(deckung(g - 1, g - 1), 0, "Ecke bei {g}");
            let v = g / 4;
            let d = g - 1 - g / 4;
            for (x, y) in [(v, v), (d, v), (d, d), (v, d)] {
                assert_eq!(deckung(x, y), 255, "Feld bei {x},{y} ({g})");
            }
            // Die Luecke zwischen den Feldern bleibt frei: waagrecht und
            // senkrecht durch die Mitte.
            assert_eq!(deckung(g / 2, v), 0, "senkrechte Luecke bei {g}");
            assert_eq!(deckung(v, g / 2), 0, "waagrechte Luecke bei {g}");
        }
        // Als PNG laesst sie sich wie das Logo schreiben (fuer NSImage).
        let p = png(36, 36, &vorlage(36));
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    /// Die .ico schreiben und erneuern; scheitert das Erneuern, gilt die
    /// vorhandene weiter (Phase-B-Hinweis f-oberflaeche) - ohne vorhandene
    /// ist es ein Fehler. Keine Nachbardatei bleibt liegen. Nur unter temp_dir.
    #[test]
    fn ico_erneuern_oder_vorhandene_nehmen() {
        let ordner = std::env::temp_dir().join(format!("qc-test-{}-ico", std::process::id()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let ziel = ordner.join("quadchroma.ico");
        assert_eq!(ico_schreiben_nach(&ziel).unwrap(), ziel);
        assert_eq!(std::fs::read(&ziel).unwrap(), ico(&ICO_GROESSEN));
        std::fs::write(&ziel, b"alt").unwrap();
        // Erneuern scheitert: unter Unix ist der Ordner schreibgeschuetzt,
        // unter Windows haelt ein Leser die Datei ohne Freigabe offen.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ordner, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        #[cfg(windows)]
        let sperre = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().read(true).share_mode(0).open(&ziel).unwrap()
        };
        let r = ico_schreiben_nach(&ziel);
        #[cfg(windows)]
        drop(sperre);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ordner, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(r.unwrap(), ziel, "vorhandene .ico nicht genommen");
        assert_eq!(std::fs::read(&ziel).unwrap(), b"alt");
        let namen: Vec<_> = std::fs::read_dir(&ordner).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(namen, vec![std::ffi::OsString::from("quadchroma.ico")], "Nachbardatei liegen geblieben");
        // Ohne vorhandene Datei (und ohne Ordner) ist es ein Fehler.
        assert!(ico_schreiben_nach(&ordner.join("fehlt").join("quadchroma.ico")).is_err());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// res/quadchroma.ico - das Symbol, das build.rs in die Windows-exe
    /// einbettet - sind genau die Bytes, die das Programm zur Laufzeit als
    /// .ico schreibt. Laufen beide auseinander, zeigen exe und Verknuepfung
    /// verschiedene Symbole; dann ico_schreiben_res ausfuehren.
    #[test]
    fn ico_res_gleich_logo() {
        let repo = include_bytes!("../res/quadchroma.ico");
        let jetzt = ico(&ICO_GROESSEN);
        // Kein assert_eq!: bei einer Abweichung gaebe es 280 KB Bytes aus.
        assert!(
            repo.as_slice() == jetzt.as_slice(),
            "res/quadchroma.ico ({} Byte) weicht vom Logo ({} Byte) ab - neu erzeugen: \
             QC_ICO_PFAD=res/quadchroma.ico cargo test --release ico_schreiben_res -- --ignored",
            repo.len(),
            jetzt.len()
        );
    }

    /// res/quadchroma.ico neu erzeugen. Absichtlich uebersprungen (#[ignore]):
    /// laeuft nur auf Wunsch und nur dann noetig, wenn sich Motiv, Farben
    /// (ui.rs) oder ICO_GROESSEN geaendert haben - ico_res_gleich_logo meldet
    /// das. Aufruf im Ordner client/ (Zielpfad relativ dazu oder absolut):
    ///   PowerShell: $env:QC_ICO_PFAD = "res\quadchroma.ico"; cargo test --release ico_schreiben_res -- --ignored
    ///   sh:         QC_ICO_PFAD=res/quadchroma.ico cargo test --release ico_schreiben_res -- --ignored
    /// Die Datei danach mit ins Repository nehmen.
    #[test]
    #[ignore = "schreibt die .ico nach QC_ICO_PFAD; nur ausdruecklich aufrufen"]
    fn ico_schreiben_res() {
        let pfad = std::path::PathBuf::from(
            std::env::var_os("QC_ICO_PFAD").expect("QC_ICO_PFAD (Zielpfad der .ico) ist nicht gesetzt"),
        );
        let daten = ico(&ICO_GROESSEN);
        std::fs::write(&pfad, &daten).unwrap_or_else(|e| panic!("{}: {e}", pfad.display()));
        println!("{} Byte -> {}", daten.len(), pfad.display());
    }

    /// Die Dateien der .iconset-Mappe fuer iconutil (Apples Namen) mit ihrer
    /// Kantenlaenge, dazu die Art, unter der iconutil sie in die .icns legt.
    const ICONSET: [(&str, u32, &[u8; 4]); 10] = [
        ("icon_16x16", 16, b"ic04"),
        ("icon_16x16@2x", 32, b"ic11"),
        ("icon_32x32", 32, b"ic05"),
        ("icon_32x32@2x", 64, b"ic12"),
        ("icon_128x128", 128, b"ic07"),
        ("icon_128x128@2x", 256, b"ic13"),
        ("icon_256x256", 256, b"ic08"),
        ("icon_256x256@2x", 512, b"ic14"),
        ("icon_512x512", 512, b"ic09"),
        ("icon_512x512@2x", 1024, b"ic10"),
    ];

    /// CRC-32 ueber die Bildpunkte des Mac-Symbols in allen MAC_GROESSEN.
    fn mac_symbol_pruefsumme() -> u32 {
        MAC_GROESSEN.iter().fold(0xffff_ffff, |crc, &g| crc32_weiter(crc, &mac_symbol(g))) ^ 0xffff_ffff
    }

    /// Die Eintraege einer .icns-Datei: Art und Daten, in Dateireihenfolge.
    fn icns_eintraege(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
        assert_eq!(&d[..4], b"icns");
        assert_eq!(u32_be(d, 4) as usize, d.len(), "Laenge im Kopf");
        let mut out = Vec::new();
        let mut o = 8;
        while o < d.len() {
            let len = u32_be(d, o + 4) as usize;
            assert!(len >= 8 && o + len <= d.len(), "Eintrag bei {o}");
            out.push(([d[o], d[o + 1], d[o + 2], d[o + 3]], &d[o + 8..o + len]));
            o += len;
        }
        out
    }

    /// Das Mac-Symbol: Ecken und Rand durchsichtig, die Kachelmitte deckend,
    /// die vier Felder in ihren Farben (untere Haelfte ohne Glanz: genau die
    /// Farbe), zwischen ihnen der Grund; der Schatten liegt unter der
    /// Kachel, nicht darueber.
    #[test]
    fn mac_symbol_motiv() {
        for g in MAC_GROESSEN {
            let b = mac_symbol(g);
            assert_eq!(b.len(), (g * g * 4) as usize);
            let px = |x: u32, y: u32| {
                let o = ((y * g + x) * 4) as usize;
                ((b[o] as u32) << 16 | (b[o + 1] as u32) << 8 | b[o + 2] as u32, b[o + 3])
            };
            assert_eq!(px(0, 0).1, 0, "Ecke bei {g}");
            assert_eq!(px(g - 1, 0).1, 0, "Ecke bei {g}");
            // Knapp innerhalb der Kachelecke (Rand 100/1024) ist noch nichts.
            let r = g * 100 / 1024;
            assert!(px(r, r).1 < 64, "Kachelecke bei {g}: {}", px(r, r).1);
            // Unter der Kachel mehr Schatten als darueber.
            if g >= 64 {
                let unter = px(g / 2, g - r + g / 128).1;
                let ueber = px(g / 2, r - 1 - g / 128).1;
                assert!(unter > ueber && ueber > 0, "Schatten bei {g}: unter {unter}, ueber {ueber}");
            }
            assert_eq!(px(g / 2, g / 2).1, 255, "Mitte bei {g}");
            // Felder, je bei etwa 3/4 ihrer Hoehe: dort ohne Glanz.
            let (v, d) = (g * 9 / 32, g - 1 - g * 9 / 32);
            let (vu, du) = (g * 27 / 64, g * 89 / 128);
            assert_eq!(px(v, vu), (ui::CYAN, 255), "oben links bei {g}");
            assert_eq!(px(d, vu), (ui::MAGENTA, 255), "oben rechts bei {g}");
            assert_eq!(px(d, du), (ui::TEXT, 255), "unten rechts bei {g}");
            assert_eq!(px(v, du), (ui::AMBER, 255), "unten links bei {g}");
            // Oben im Feld glaenzt es: Cyan bekommt Rot dazu (ab 64 px
            // liegt die Probe sicher in der oberen Feldhaelfte).
            if g >= 64 {
                assert!(px(v, g / 4 + 1).0 >> 16 > 0, "Glanz bei {g}");
            }
        }
    }

    /// Kleine Groessen bleiben scharf: die Felder beginnen und enden auf
    /// ganzen Bildpunkten, links und rechts gleich weit vom Rand.
    #[test]
    fn mac_symbol_felder_auf_ganzen_punkten() {
        for g in [16u32, 32] {
            let b = mac_symbol(g);
            let zeile = g * 89 / 128; // durch die unteren Felder, ohne Glanz
            let farben: Vec<u32> = (0..g)
                .map(|x| {
                    let o = ((zeile * g + x) * 4) as usize;
                    (b[o] as u32) << 16 | (b[o + 1] as u32) << 8 | b[o + 2] as u32
                })
                .collect();
            let amber: Vec<u32> = (0..g).filter(|&x| farben[x as usize] == ui::AMBER).collect();
            let text: Vec<u32> = (0..g).filter(|&x| farben[x as usize] == ui::TEXT).collect();
            assert!(!amber.is_empty() && amber.len() == text.len(), "{g}: {amber:?} {text:?}");
            assert_eq!(amber[0], g - 1 - text[text.len() - 1], "symmetrisch bei {g}");
            // Keine Mischfarbe an den Feldkanten.
            for x in [amber[0] - 1, amber[amber.len() - 1] + 1, text[0] - 1] {
                let c = farben[x as usize];
                assert!(c != ui::AMBER && c != ui::TEXT, "Kante bei {g}, x {x}");
            }
        }
    }

    /// res/AppIcon.icns - das Symbol im Bundle der Mac-App (Makefile) -
    /// passt zum Code: alle zehn Eintraege der .iconset-Mappe sind darin, und
    /// res/AppIcon.icns.crc, den icns_schreiben_res mit der Datei schreibt,
    /// nennt die Pruefsummen der Bildpunkte von heute und der Datei selbst.
    /// Laeuft auf beiden Plattformen (die Pixel sind bitgleich).
    #[test]
    fn icns_res_gleich_logo() {
        let icns = include_bytes!("../res/AppIcon.icns");
        let arten: Vec<[u8; 4]> = icns_eintraege(icns).iter().map(|(a, _)| *a).collect();
        for (name, _, art) in ICONSET {
            assert!(arten.contains(art), "{name} ({}) fehlt in res/AppIcon.icns", String::from_utf8_lossy(art));
        }
        let stempel = include_str!("../res/AppIcon.icns.crc");
        let wert = |schluessel: &str| {
            stempel
                .lines()
                .find_map(|z| z.trim().strip_prefix(schluessel).map(|w| w.trim().to_string()))
                .unwrap_or_else(|| panic!("res/AppIcon.icns.crc ohne '{schluessel}'"))
        };
        assert_eq!(wert("icns "), format!("{:08x}", crc32(icns)), "res/AppIcon.icns passt nicht zu res/AppIcon.icns.crc");
        assert_eq!(
            wert("logo "),
            format!("{:08x}", mac_symbol_pruefsumme()),
            "das Mac-Symbol (logo.rs, ui.rs) hat sich geaendert - res/AppIcon.icns neu erzeugen (Mac, ausserhalb \
             der Seatbelt-Sandbox, im Ordner client/): \
             QC_ICNS_PFAD=res/AppIcon.icns cargo test --release icns_schreiben_res -- --ignored"
        );
    }

    /// res/AppIcon.icns neu erzeugen (nur auf dem Mac, nur auf Wunsch wie
    /// ico_schreiben_res): die .iconset-Mappe aus mac_symbol in einen
    /// Ordner unter temp_dir, daraus mit iconutil die .icns nach
    /// QC_ICNS_PFAD, daneben <Pfad>.crc mit den Pruefsummen. iconutil
    /// scheitert in einer Seatbelt-Sandbox ("Failed to generate ICNS").
    /// Aufruf im Ordner client/:
    ///   QC_ICNS_PFAD=res/AppIcon.icns cargo test --release icns_schreiben_res -- --ignored
    /// Beide Dateien danach mit ins Repository nehmen.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "schreibt die .icns nach QC_ICNS_PFAD; nur ausdruecklich aufrufen"]
    fn icns_schreiben_res() {
        let pfad = std::path::PathBuf::from(
            std::env::var_os("QC_ICNS_PFAD").expect("QC_ICNS_PFAD (Zielpfad der .icns) ist nicht gesetzt"),
        );
        let ordner = std::env::temp_dir().join(format!("qc-test-{}-icns", std::process::id()));
        let mappe = ordner.join("AppIcon.iconset");
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&mappe).unwrap();
        for (name, g, _) in ICONSET {
            std::fs::write(mappe.join(format!("{name}.png")), png(g, g, &mac_symbol(g))).unwrap();
        }
        let ergebnis = std::process::Command::new("/usr/bin/iconutil")
            .args(["-c", "icns", "-o"])
            .arg(&pfad)
            .arg(&mappe)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&ordner);
        assert!(
            ergebnis.status.success(),
            "iconutil: {}{}",
            String::from_utf8_lossy(&ergebnis.stdout),
            String::from_utf8_lossy(&ergebnis.stderr)
        );
        let icns = std::fs::read(&pfad).unwrap();
        let arten: Vec<[u8; 4]> = icns_eintraege(&icns).iter().map(|(a, _)| *a).collect();
        for (name, _, art) in ICONSET {
            assert!(arten.contains(art), "iconutil hat {name} nicht als {} abgelegt", String::from_utf8_lossy(art));
        }
        let stempel = format!(
            "# Pruefsummen zu AppIcon.icns (logo.rs, Test icns_schreiben_res) - nicht von Hand aendern\n\
             logo {:08x}\nicns {:08x}\n",
            mac_symbol_pruefsumme(),
            crc32(&icns)
        );
        let crc = std::path::PathBuf::from(format!("{}.crc", pfad.display()));
        std::fs::write(&crc, stempel).unwrap();
        println!("{} Byte -> {}, {}", icns.len(), pfad.display(), crc.display());
    }
}
