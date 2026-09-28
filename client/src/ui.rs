// Oberflaeche. Alles selbst gezeichnet, direkt in den Bildpuffer.
//
// Optik: dunkler Grund, feines Raster, Neonlinien in Cyan und Magenta, Ecken
// als Klammern statt Rahmen, Schrift schmal und technisch. Keine Fremdbaukaesten,
// damit das Bild des Macs und die Oberflaeche im selben Puffer liegen und nichts
// dazwischenfunkt.
//
// Abhaengigkeit: fontdue = "0.9"   (MIT ODER Apache-2.0)

use fontdue::{Font, FontSettings};
use std::collections::HashMap;

pub const BG: u32 = 0x05070d;
pub const GRID: u32 = 0x0c1320;
pub const CYAN: u32 = 0x00e5ff;
pub const MAGENTA: u32 = 0xff2fb9;
pub const AMBER: u32 = 0xffb300;
pub const TEXT: u32 = 0xd7e3ec;
pub const DIM: u32 = 0x5d7183;
/// Gold gibt es nur fuer eines: HDR laeuft auf BEIDEN Seiten (Host sendet
/// PQ, der Client praesentiert in HDR/EDR). Gelber und heller als AMBER, das
/// die Warnfarbe bleibt.
pub const GOLD: u32 = 0xffd24a;
/// Heller Glanz: Funkelpunkte und die Oberkante des goldenen Verlaufs.
pub const GOLD_HELL: u32 = 0xfff4cf;
/// Tiefes, metallisches Gold: Unterkante des Verlaufs.
pub const GOLD_TIEF: u32 = 0xc08a1e;

/// Innenabstand des HDR-Schalters und die Breite seines Schiebers.
const HDR_SCHALTER_RAND: i32 = 10;
const HDR_SCHALTER_BREITE: i32 = 46;

/// Zwei Farben mischen, `t` 0..=255 (0 = a, 255 = b).
pub fn farbe_mischen(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(255);
    let k = |s: u32| -> u32 { ((((a >> s) & 255) * (255 - t) + ((b >> s) & 255) * t) / 255) << s };
    k(16) | k(8) | k(0)
}

/// Goldener Verlauf fuer Text in einem Kasten (`links`, `oben`, `breite`,
/// `hoehe`): oben hell, in der Mitte Gold, unten metallisch tief. Dazu ein
/// schraeger weisser Glanzstreifen, der mit `tick` von links nach rechts
/// ueber den Kasten wandert - einmal je 300 Takte (2,4 s), mit einer Pause
/// dazwischen, in der er ausserhalb liegt.
pub fn gold_verlauf(x: i32, y: i32, links: i32, oben: i32, breite: i32, hoehe: i32, tick: u64) -> u32 {
    let hoehe = hoehe.max(1);
    let t = ((y - oben).clamp(0, hoehe) * 255 / hoehe) as u32;
    let grund = if t < 110 {
        farbe_mischen(GOLD_HELL, GOLD, t * 255 / 110)
    } else {
        farbe_mischen(GOLD, GOLD_TIEF, (t - 110) * 255 / 145)
    };
    let weg = breite + hoehe + 160;
    let pos = (tick % 300) as i32 * weg / 300 - hoehe - 20;
    let abstand = ((x - links) + (oben + hoehe - y) / 2 - pos).abs();
    let band = (hoehe / 5).max(3);
    if abstand < band {
        farbe_mischen(grund, 0xffffff, (band - abstand) as u32 * 210 / band as u32)
    } else {
        grund
    }
}

/// Ein 64-Bit-Mischer (splitmix64): feste Zufallszahlen fuer das Funkeln,
/// ohne Zustand - dasselbe Bild fuer denselben Takt.
fn mischen(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Wie hell Glanzpunkt `i` in Takt `tick` leuchtet (0.0..=1.0) und wo er
/// in dieser Runde sitzt (zwei Zufallszahlen fuer x und y). Jeder hat einen
/// eigenen Takt (60-109 Takte, bei 8 ms je Takt 0,5-0,9 s) und eine eigene
/// Phase; er leuchtet in der ersten Haelfte davon auf und wieder ab und
/// sitzt in jeder Runde an einer neuen Stelle - wie Glitzer, der das Licht
/// faengt.
pub fn glanz_stand(i: u32, tick: u64) -> (f32, u64) {
    let h = mischen(0x51_7CC1_B727_220A ^ i as u64);
    let periode = 60 + h % 50;
    let t = tick + (h >> 8) % periode;
    let runde = t / periode;
    let f = (t % periode) as f32 / periode as f32;
    let hell = if f < 0.5 { (f / 0.5 * std::f32::consts::PI).sin() } else { 0.0 };
    (hell, mischen(h ^ runde.wrapping_mul(0xD1B5_4A32_D192_ED03)))
}

// Ein Bildpunkt ist 0xTTRRGGBB: das oberste Byte ist die DURCHSICHT T
// (255 = ganz durchsichtig, 0 = deckend), die Farbe ist mit der Deckung
// vormultipliziert. Alle Farbkonstanten und jedes hingelegte Videobild
// haben T = 0; eine Mischung ueber einem deckenden Ziel bleibt deckend, und
// ihre RGB-Bytes sind dieselben wie ohne das T-Byte - deshalb aendert sich
// an den Schnappschuessen kein Bit, und softbuffer bekommt weiter eine Null
// im obersten Byte. Erst eine Oberflaeche, die ueber einem LEEREN Puffer
// (T = 255, Farbe 0) gezeichnet wird, traegt in T, wie viel vom Bild
// darunter noch durchscheint: Bild * T/255 + Farbe ist dann genau das, was
// die sequentielle Mischung hier ergeben haette.
#[inline(always)]
fn mix(dst: u32, src: u32, a: u32) -> u32 {
    if a == 0 { return dst; }
    if a >= 255 { return src; }
    let (dr, dg, db) = ((dst >> 16) & 255, (dst >> 8) & 255, dst & 255);
    let (sr, sg, sb) = ((src >> 16) & 255, (src >> 8) & 255, src & 255);
    // Die Durchsicht schrumpft mit jeder Schicht: was zu a/255 gedeckt
    // wird, laesst nur noch (255-a)/255 des Darunterliegenden durch.
    let t = ((dst >> 24) * (255 - a)) / 255;
    let r = (sr * a + dr * (255 - a)) / 255;
    let g = (sg * a + dg * (255 - a)) / 255;
    let b = (sb * a + db * (255 - a)) / 255;
    (t << 24) | (r << 16) | (g << 8) | b
}

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    /// Schmutzrechteck: die Vereinigung von allem, was seit dem letzten
    /// `kasten_nehmen` beschrieben wurde. Wer die Oberflaeche in eine
    /// Textur laedt, laedt nur das - nicht das ganze Fenster.
    pub kasten: Option<Rect>,
}

impl<'a> Canvas<'a> {
    pub fn neu(buf: &'a mut [u32], w: usize, h: usize) -> Self {
        Canvas { buf, w, h, kasten: None }
    }

    #[inline(always)]
    pub fn px(&mut self, x: i32, y: i32, color: u32, alpha: u32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h { return; }
        let i = y as usize * self.w + x as usize;
        self.buf[i] = mix(self.buf[i], color, alpha);
        self.merke_punkt(x, y);
    }

    /// Einen Punkt in den Kasten aufnehmen. Je Punkt vier Vergleiche -
    /// billig neben der Mischung, und damit ist jeder direkte px-Aufruf
    /// eines Bedienteils von selbst erfasst.
    #[inline(always)]
    fn merke_punkt(&mut self, x: i32, y: i32) {
        match &mut self.kasten {
            Some(k) => {
                if x < k.x { k.w += k.x - x; k.x = x; } else if x >= k.x + k.w { k.w = x - k.x + 1; }
                if y < k.y { k.h += k.y - y; k.y = y; } else if y >= k.y + k.h { k.h = y - k.y + 1; }
            }
            None => self.kasten = Some(Rect { x, y, w: 1, h: 1 }),
        }
    }

    /// Ein Rechteck in den Kasten aufnehmen, auf die Flaeche beschnitten.
    /// Fuer alles, was am px vorbei schreibt (fill, backdrop).
    pub fn merke(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = x.saturating_add(w).min(self.w as i32);
        let y1 = y.saturating_add(h).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 { return; }
        self.kasten = Some(match self.kasten {
            None => Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 },
            Some(k) => {
                let (nx, ny) = (k.x.min(x0), k.y.min(y0));
                let (ex, ey) = ((k.x + k.w).max(x1), (k.y + k.h).max(y1));
                Rect { x: nx, y: ny, w: ex - nx, h: ey - ny }
            }
        });
    }

    /// Den Kasten abholen und leeren - fuer den Upload der Oberflaeche in
    /// die Textur der Karte.
    pub fn kasten_nehmen(&mut self) -> Option<Rect> {
        self.kasten.take()
    }

    /// Grosse Flaeche mischen. Anders als rect() wird einmal am Rand
    /// beschnitten statt je Bildpunkt geprueft, und die Mischung laeuft ueber
    /// maskierte Multiplikation statt ueber drei Ganzzahldivisionen. Fuer den
    /// Schleier des HUD ueber einem 4K-Bild ist das der Unterschied zwischen
    /// spuerbar und unsichtbar.
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).clamp(0, self.w as i32) as usize;
        let y1 = (y + h).clamp(0, self.h as i32) as usize;
        if x1 <= x0 || y1 <= y0 { return; }
        self.merke(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32);
        let a = alpha.min(255);
        let k = 256 - a;
        let cr = ((color & 0x00ff00ff) * a >> 8) & 0x00ff00ff;
        let cg = ((color & 0x0000ff00) * a >> 8) & 0x0000ff00;
        let breite = self.w;
        for yy in y0..y1 {
            let zeile = &mut self.buf[yy * breite + x0..yy * breite + x1];
            for p in zeile.iter_mut() {
                let rb = ((*p & 0x00ff00ff) * k >> 8) & 0x00ff00ff;
                let g = ((*p & 0x0000ff00) * k >> 8) & 0x0000ff00;
                // Die Durchsicht mit demselben Faktor wie die Farbe; bei
                // einem deckenden Ziel (T = 0) bleibt sie null.
                let t = (((*p >> 24) * k) >> 8) << 24;
                *p = t | (rb + cr) | (g + cg);
            }
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u32) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                self.px(xx, yy, color, alpha);
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u32, alpha: u32) {
        for xx in x..x + w { self.px(xx, y, color, alpha); }
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: u32, alpha: u32) {
        for yy in y..y + h { self.px(x, yy, color, alpha); }
    }

    /// Linie mit weichem Schein daneben - macht den Neon-Eindruck.
    pub fn glow_hline(&mut self, x: i32, y: i32, w: i32, color: u32) {
        self.hline(x, y, w, color, 255);
        self.hline(x, y - 1, w, color, 70);
        self.hline(x, y + 1, w, color, 70);
        self.hline(x, y - 2, w, color, 25);
        self.hline(x, y + 2, w, color, 25);
    }

    pub fn glow_vline(&mut self, x: i32, y: i32, h: i32, color: u32) {
        self.vline(x, y, h, color, 255);
        self.vline(x - 1, y, h, color, 70);
        self.vline(x + 1, y, h, color, 70);
    }

    /// Panel: dunkle Flaeche, Eckklammern statt Rahmen.
    pub fn panel(&mut self, x: i32, y: i32, w: i32, h: i32, accent: u32) {
        self.rect(x, y, w, h, 0x080c14, 225);
        let c = 18.min(w / 3).min(h / 3);
        // vier Ecken
        for (cx, cy, dx, dy) in [(x, y, 1, 1), (x + w - 1, y, -1, 1), (x, y + h - 1, 1, -1), (x + w - 1, y + h - 1, -1, -1)] {
            for i in 0..c {
                self.px(cx + dx * i, cy, accent, 255);
                self.px(cx, cy + dy * i, accent, 255);
            }
        }
        // dezente Kante dazwischen
        self.hline(x + c, y, w - 2 * c, accent, 40);
        self.hline(x + c, y + h - 1, w - 2 * c, accent, 40);
        self.vline(x, y + c, h - 2 * c, accent, 40);
        self.vline(x + w - 1, y + c, h - 2 * c, accent, 40);
    }

    /// Eckklammern um ein Rechteck, `laenge` je Schenkel.
    pub fn klammern(&mut self, x: i32, y: i32, w: i32, h: i32, farbe: u32, alpha: u32, laenge: i32) {
        let l = laenge.min(w / 3).min(h / 3).max(1);
        for (cx, cy, dx, dy) in [(x, y, 1, 1), (x + w - 1, y, -1, 1), (x, y + h - 1, 1, -1), (x + w - 1, y + h - 1, -1, -1)] {
            for i in 0..l {
                self.px(cx + dx * i, cy, farbe, alpha);
                self.px(cx, cy + dy * i, farbe, alpha);
            }
        }
    }

    /// Ein Glanzpunkt: ein kleiner Stern mit vier langen und vier kurzen
    /// Strahlen und einem goldenen Hof, `r` die Laenge der langen Strahlen,
    /// `staerke` 0..=255.
    pub fn glanz(&mut self, x: i32, y: i32, r: i32, staerke: u32) {
        let s = staerke.min(255);
        if s == 0 || r <= 0 {
            return;
        }
        // Hof: ein warmer Schein um den Kern.
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                let n = dx.abs() + dy.abs();
                if n > 0 && n <= 3 {
                    self.px(x + dx, y + dy, GOLD, s * (4 - n as u32) / 8);
                }
            }
        }
        // Lange Strahlen nach oben, unten, links, rechts; zur Spitze duenner.
        for d in 1..=r {
            let a = s * (r + 1 - d) as u32 / (r + 1) as u32;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                self.px(x + dx * d, y + dy * d, if d <= r / 2 { 0xffffff } else { GOLD_HELL }, a);
            }
        }
        // Kurze Strahlen schraeg.
        let rd = (r / 2).max(1);
        for d in 1..=rd {
            let a = s * (rd + 1 - d) as u32 / (rd + 1) as u32 * 2 / 3;
            for (dx, dy) in [(1, 1), (-1, 1), (1, -1), (-1, -1)] {
                self.px(x + dx * d, y + dy * d, GOLD_HELL, a);
            }
        }
        self.px(x, y, 0xffffff, s);
    }

    /// Funkeln ueber einem Rechteck: `n` Glanzpunkte nach glanz_stand, die
    /// Strahlen bis `groesse` lang (mit dem Leuchten wachsend). Eine reine
    /// Funktion von `tick` - Schnappschuesse zeigen dasselbe Bild fuer
    /// denselben Takt. `deckung` 0..=255 blendet alles (Abzeichen).
    /// `saat` trennt mehrere Funkelflaechen voneinander.
    pub fn funkeln(&mut self, r: Rect, tick: u64, n: u32, groesse: i32, deckung: u32, saat: u32) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        for i in 0..n {
            let (hell, p) = glanz_stand(saat.wrapping_mul(97).wrapping_add(i), tick);
            if hell <= 0.03 {
                continue;
            }
            let x = r.x + (p % r.w as u64) as i32;
            let y = r.y + ((p >> 24) % r.h as u64) as i32;
            let staerke = (hell * 255.0) as u32 * deckung.min(255) / 255;
            let laenge = ((groesse as f32) * (0.55 + 0.45 * hell)).round() as i32;
            self.glanz(x, y, laenge, staerke);
        }
    }

    /// Hintergrund: Grundton, Raster, und ein langsam wanderndes Band.
    pub fn backdrop(&mut self, tick: u64) {
        self.buf.fill(BG);
        self.merke(0, 0, self.w as i32, self.h as i32);
        let step = 28;
        let mut x = 0;
        while x < self.w as i32 { self.vline(x, 0, self.h as i32, GRID, 255); x += step; }
        let mut y = 0;
        while y < self.h as i32 { self.hline(0, y, self.w as i32, GRID, 255); y += step; }

        // Scanband
        let band = ((tick / 2) % (self.h as u64 + 120)) as i32 - 60;
        for i in -30..30i32 {
            let a = (30 - i.abs()) as u32 * 2;
            self.hline(0, band + i, self.w as i32, CYAN, a / 6);
        }
    }
}

// ----------------------------------------------------------------- Schrift

/// fontdue sammelt die Zeichenzuordnung aus ALLEN cmap-Untertabellen der
/// Reihe nach in eine Karte - die letzte gewinnt. Apple-Schriften (Menlo,
/// Monaco) fuehren hinter der Unicode-Tabelle noch eine Mac-Roman-Tabelle
/// (Plattform 1), und die ueberschreibt fuer alle Zeichen unter 256 die
/// Unicode-Zuordnung: aus "oe" wird ein Zirkumflex, aus dem Mittelpunkt ein
/// Summenzeichen. Abhilfe ohne Fremdcode: die Eintragsliste der cmap-Tabelle
/// in der geladenen Kopie so umsortieren, dass die Unicode-Tabellen zuletzt
/// kommen (Plattform 0, oder 3 mit Kodierung 1 oder 10). Nur die 8-Byte-
/// Eintraege werden bewegt, die Tabellen selbst bleiben, wo sie sind. Fuer
/// eine Sammlung (.ttc) gilt die erste Schrift - dieselbe, die fontdue nimmt.
/// Passt etwas nicht ins Schema, bleibt die Datei unveraendert. Auf Windows
/// aendert das nichts: dort gewinnt schon heute die Unicode-Tabelle.
fn cmap_unicode_zuletzt(daten: &mut [u8]) {
    let u16_bei = |d: &[u8], o: usize| -> Option<u16> { d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]])) };
    let u32_bei = |d: &[u8], o: usize| -> Option<u32> { d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])) };
    let Some(magic) = u32_bei(daten, 0) else { return };
    // Sammlung: 'ttcf', Zahl der Schriften bei 8, Versatz der ersten bei 12.
    let start = if magic == 0x7474_6366 { u32_bei(daten, 12).map(|v| v as usize) } else { Some(0) };
    let Some(start) = start else { return };
    let Some(n) = u16_bei(daten, start + 4) else { return };
    let mut cmap = None;
    for i in 0..n as usize {
        let r = start + 12 + 16 * i;
        if daten.get(r..r + 4) == Some(b"cmap") {
            cmap = u32_bei(daten, r + 8).map(|v| v as usize);
            break;
        }
    }
    let Some(cmap) = cmap else { return };
    let Some(anzahl) = u16_bei(daten, cmap + 2) else { return };
    let liste = cmap + 4;
    let ende = liste + 8 * anzahl as usize;
    let Some(bereich) = daten.get(liste..ende) else { return };
    let mut eintraege: Vec<[u8; 8]> = bereich
        .chunks(8)
        .map(|c| {
            let mut e = [0u8; 8];
            e.copy_from_slice(c);
            e
        })
        .collect();
    let unicode = |e: &[u8; 8]| -> bool {
        let plattform = u16::from_be_bytes([e[0], e[1]]);
        let kodierung = u16::from_be_bytes([e[2], e[3]]);
        plattform == 0 || (plattform == 3 && (kodierung == 1 || kodierung == 10))
    };
    // Stabil: Unicode ans Ende, Reihenfolge innerhalb der Gruppen bleibt.
    eintraege.sort_by_key(|e| unicode(e));
    for (i, e) in eintraege.iter().enumerate() {
        daten[liste + 8 * i..liste + 8 * i + 8].copy_from_slice(e);
    }
}

pub struct Text {
    fonts: Vec<Font>,
    cache: HashMap<(char, u32, usize), (fontdue::Metrics, Vec<u8>)>,
}

impl Text {
    /// Laedt eine schmale technische Schrift und Ausweichschriften fuer
    /// chinesische und japanische Zeichen. Fehlt eine, wird sie uebersprungen.
    pub fn load() -> Self {
        let candidates = [
            "C:\\Windows\\Fonts\\consola.ttf",   // Consolas: Latein, Kyrillisch, Griechisch
            "C:\\Windows\\Fonts\\segoeui.ttf",    // deckt Zeichen ab, die Consolas fehlen
            "C:\\Windows\\Fonts\\msyh.ttc",      // Microsoft YaHei: Chinesisch
            "C:\\Windows\\Fonts\\YuGothM.ttc",   // Yu Gothic: Japanisch
            "C:\\Windows\\Fonts\\meiryo.ttc",
            "/System/Library/Fonts/Menlo.ttc",                        // Mac: Latein, Kyrillisch, Griechisch
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",   // Mac: Chinesisch, Japanisch (TrueType)
            "/System/Library/Fonts/PingFang.ttc",                     // Mac: CFF, fontdue kann es meist nicht laden
        ];
        let mut fonts = Vec::new();
        for path in candidates {
            if let Ok(mut data) = std::fs::read(path) {
                cmap_unicode_zuletzt(&mut data);
                if let Ok(f) = Font::from_bytes(data.as_slice(), FontSettings::default()) {
                    fonts.push(f);
                }
            }
        }
        Text { fonts, cache: HashMap::new() }
    }

    pub fn ok(&self) -> bool { !self.fonts.is_empty() }

    fn glyph(&mut self, ch: char, size: u32) -> Option<(fontdue::Metrics, Vec<u8>, usize)> {
        let idx = self
            .fonts
            .iter()
            .position(|f| f.lookup_glyph_index(ch) != 0)
            .unwrap_or(0);
        if self.fonts.is_empty() { return None; }
        let key = (ch, size, idx);
        if !self.cache.contains_key(&key) {
            let (m, bm) = self.fonts[idx].rasterize(ch, size as f32);
            self.cache.insert(key, (m, bm));
        }
        let (m, bm) = self.cache.get(&key)?;
        Some((*m, bm.clone(), idx))
    }

    pub fn width(&mut self, s: &str, size: u32, spacing: i32) -> i32 {
        let mut w = 0;
        for ch in s.chars() {
            if let Some((m, _, _)) = self.glyph(ch, size) {
                w += m.advance_width as i32 + spacing;
            }
        }
        w
    }

    /// Zeichnet Text. spacing weitet die Laufweite - das macht den technischen Look.
    pub fn draw(&mut self, c: &mut Canvas, x: i32, y: i32, s: &str, size: u32, color: u32, spacing: i32) -> i32 {
        let mut pen = x;
        for ch in s.chars() {
            let Some((m, bm, _)) = self.glyph(ch, size) else { continue };
            let gx = pen + m.xmin;
            let gy = y - m.height as i32 - m.ymin;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = bm[row * m.width + col] as u32;
                    if a > 8 { c.px(gx + col as i32, gy + row as i32, color, a); }
                }
            }
            pen += m.advance_width as i32 + spacing;
        }
        pen
    }

    /// Wie `draw`, aber mit einer Farbe je Bildpunkt: `farbe(x, y)` liefert
    /// Farbe und Deckung (0..=255), verrechnet mit der Deckung der Glyphe -
    /// fuer den goldenen Verlauf samt wanderndem Glanz.
    pub fn draw_mit(&mut self, c: &mut Canvas, x: i32, y: i32, s: &str, size: u32, spacing: i32, farbe: impl Fn(i32, i32) -> (u32, u32)) -> i32 {
        let mut pen = x;
        for ch in s.chars() {
            let Some((m, bm, _)) = self.glyph(ch, size) else { continue };
            let gx = pen + m.xmin;
            let gy = y - m.height as i32 - m.ymin;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = bm[row * m.width + col] as u32;
                    if a > 8 {
                        let (px, py) = (gx + col as i32, gy + row as i32);
                        let (f, d) = farbe(px, py);
                        c.px(px, py, f, a * d.min(255) / 255);
                    }
                }
            }
            pen += m.advance_width as i32 + spacing;
        }
        pen
    }

    /// Rechtsbuendig setzen. Spart an gut zwanzig Stellen das Ausmessen davor.
    pub fn draw_right(&mut self, c: &mut Canvas, x_right: i32, y: i32, s: &str, size: u32, color: u32, spacing: i32) {
        let w = self.width(s, size, spacing);
        self.draw(c, x_right - w, y, s, size, color, spacing);
    }

    pub fn draw_centered(&mut self, c: &mut Canvas, cx: i32, y: i32, s: &str, size: u32, color: u32, spacing: i32) {
        let w = self.width(s, size, spacing);
        self.draw(c, cx - w / 2, y, s, size, color, spacing);
    }
}

// ----------------------------------------------------------------- Bedienteile

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

impl Rect {
    pub fn hit(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && my >= self.y && mx < self.x + self.w && my < self.y + self.h
    }
}

pub struct Ui {
    pub text: Text,
    pub mouse: (i32, i32),
    pub click: bool,
    pub tick: u64,
    /// Wo die Ergebnistabelle des Benchmarks zuletzt gezeichnet wurde und
    /// wie viele Zeilen hineinpassen - damit das Mausrad weiss, ob es
    /// darueber steht und wie weit es rollen darf. None, wenn der Reiter
    /// nicht offen war.
    pub bench_tabelle: Option<Rect>,
    pub bench_sichtbar: usize,
    /// Wo eine Geraeteliste (Startbildschirm, Reiter "Computer") zuletzt
    /// gezeichnet wurde, wie viele Zeilen hineinpassen, wie viele es gibt und
    /// wie hoch eine Zeile samt Abstand ist (Bildpunkte) - fuer Mausrad und
    /// Trackpad. None, wenn keine zu sehen war.
    pub geraeteliste: Option<Rect>,
    pub geraete_sichtbar: usize,
    pub geraete_anzahl: usize,
    pub geraete_zeilenhoehe: i32,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            text: Text::load(),
            mouse: (-1, -1),
            click: false,
            tick: 0,
            bench_tabelle: None,
            bench_sichtbar: 0,
            geraeteliste: None,
            geraete_sichtbar: 0,
            geraete_anzahl: 0,
            geraete_zeilenhoehe: 34,
        }
    }

    /// Knopf mit Eckklammern. Gibt true zurueck, wenn er in diesem Bild geklickt wurde.
    pub fn button(&mut self, c: &mut Canvas, r: Rect, label: &str, accent: u32) -> bool {
        self.button_mit(c, r, label, accent, 15, 2)
    }

    /// Derselbe Knopf mit eigener Schriftgroesse und Laufweite - etwa der
    /// kleine Knopf in einer Hostzeile. Mit 15/2 ist er bitgleich zu `button`.
    pub fn button_mit(&mut self, c: &mut Canvas, r: Rect, label: &str, accent: u32, size: u32, spacing: i32) -> bool {
        let hot = r.hit(self.mouse.0, self.mouse.1);
        let a = if hot { 34 } else { 10 };
        c.rect(r.x, r.y, r.w, r.h, accent, a);
        let cut = 10;
        for (cx, cy, dx, dy) in [
            (r.x, r.y, 1, 1),
            (r.x + r.w - 1, r.y, -1, 1),
            (r.x, r.y + r.h - 1, 1, -1),
            (r.x + r.w - 1, r.y + r.h - 1, -1, -1),
        ] {
            for i in 0..cut {
                c.px(cx + dx * i, cy, accent, 255);
                c.px(cx, cy + dy * i, accent, 255);
            }
        }
        if hot {
            c.hline(r.x + cut, r.y, r.w - 2 * cut, accent, 120);
            c.hline(r.x + cut, r.y + r.h - 1, r.w - 2 * cut, accent, 120);
        }
        // Grundlinie etwa ein Drittel der Schrifthoehe unter der Mitte
        // (bei 15 genau die bisherigen 5 Bildpunkte).
        let ty = r.y + r.h / 2 + size as i32 / 3;
        self.text.draw_centered(c, r.x + r.w / 2, ty, label, size, if hot { 0xffffff } else { TEXT }, spacing);
        hot && self.click
    }

    /// Zeile in einer Liste, etwa ein gefundener Host.
    pub fn row(&mut self, c: &mut Canvas, r: Rect, left: &str, right: &str, selected: bool) -> bool {
        let hot = r.hit(self.mouse.0, self.mouse.1);
        if selected || hot {
            c.rect(r.x, r.y, r.w, r.h, CYAN, if selected { 26 } else { 14 });
        }
        c.vline(r.x, r.y, r.h, if selected { CYAN } else { DIM }, if selected { 255 } else { 120 });
        self.text.draw(c, r.x + 14, r.y + r.h / 2 + 5, left, 15, if selected { 0xffffff } else { TEXT }, 1);
        let rw = self.text.width(right, 13, 1);
        self.text.draw(c, r.x + r.w - rw - 12, r.y + r.h / 2 + 4, right, 13, DIM, 1);
        hot && self.click
    }

    /// Eingabefeld. Zeichnet nur; die Tasten verarbeitet der Aufrufer.
    pub fn field(&mut self, c: &mut Canvas, r: Rect, value: &str, label: &str, focused: bool) {
        let accent = if focused { CYAN } else { DIM };
        c.rect(r.x, r.y, r.w, r.h, 0x0a1018, 200);
        c.glow_hline(r.x, r.y + r.h - 1, r.w, accent);
        self.text.draw(c, r.x + 2, r.y - 6, label, 11, DIM, 2);
        let shown = if focused && (self.tick / 30) % 2 == 0 {
            format!("{value}_")
        } else {
            value.to_string()
        };
        self.text.draw(c, r.x + 10, r.y + r.h / 2 + 6, &shown, 16, TEXT, 1);
    }

    /// Schalter mit zwei Zustaenden.
    pub fn toggle(&mut self, c: &mut Canvas, r: Rect, label: &str, on: bool) -> bool {
        let hot = r.hit(self.mouse.0, self.mouse.1);
        let accent = if on { CYAN } else { DIM };
        self.text.draw(c, r.x, r.y + r.h / 2 + 5, label, 14, TEXT, 1);
        let sw = 42;
        let sx = r.x + r.w - sw;
        c.rect(sx, r.y + r.h / 2 - 7, sw, 14, accent, if on { 60 } else { 20 });
        c.rect(if on { sx + sw - 14 } else { sx }, r.y + r.h / 2 - 7, 14, 14, accent, 255);
        if hot { c.hline(r.x, r.y + r.h - 1, r.w, accent, 90); }
        hot && self.click
    }

    /// Der HDR-Schalter im Reiter Bild: wie `toggle`, aber groesser - "HDR"
    /// als Titel, daneben der Zustand in einer Zeile (`zustand`, schon auf
    /// den Platz gekuerzt), rechts der Schalter. `gold`: HDR laeuft auf
    /// beiden Seiten - dann leuchtet die Zeile golden (Grund, Klammern,
    /// Titel im Verlauf mit wanderndem Glanz, Schalter), und Glanzpunkte
    /// funkeln ueber Titel und Schalter, animiert ueber `self.tick`. Sonst
    /// ist er gerahmt wie ein Knopf, in Cyan (an) oder grau (aus). Gibt
    /// true bei einem Klick zurueck. Wie breit der Zustand sein darf, sagt
    /// `hdr_schalter_zustand_breite` - der Aufrufer kuerzt vorher.
    pub fn hdr_schalter(&mut self, c: &mut Canvas, r: Rect, zustand: &str, zustand_farbe: u32, an: bool, gold: bool) -> bool {
        let hot = r.hit(self.mouse.0, self.mouse.1);
        let tick = self.tick;
        let mitte = r.y + r.h / 2;
        // Immer gerahmt, damit der Schalter unter den anderen auffaellt; in
        // der Farbe seines Zustands.
        let accent = if gold { GOLD } else if an { CYAN } else { DIM };
        c.rect(r.x, r.y, r.w, r.h, accent, if gold { 16 } else { 8 });
        c.klammern(r.x, r.y, r.w, r.h, accent, if gold { 255 } else { 150 }, 10);
        if gold {
            // Der Rahmen atmet: sein Schein schwillt alle 1,6 s an und ab.
            let puls = ((tick % 200) as f32 / 200.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let a = (50.0 + 60.0 * puls) as u32;
            for (dy, teil) in [(0, 1u32), (-1, 3), (-2, 8)] {
                c.hline(r.x + 10, r.y + dy, r.w - 20, GOLD, a / teil);
                c.hline(r.x + 10, r.y + r.h - 1 - dy, r.w - 20, GOLD, a / teil);
            }
            for (dx, teil) in [(-1, 3u32), (-2, 8)] {
                c.vline(r.x + dx, r.y + 4, r.h - 8, GOLD, a / teil);
                c.vline(r.x + r.w - 1 - dx, r.y + 4, r.h - 8, GOLD, a / teil);
            }
        }
        let tx = r.x + HDR_SCHALTER_RAND;
        let ty = mitte + 6;
        let tende = if gold {
            let tw = self.text.width("HDR", 16, 3);
            self.text.draw_mit(c, tx, ty, "HDR", 16, 3, |x, y| (gold_verlauf(x, y, tx, ty - 12, tw, 13, tick), 255))
        } else {
            self.text.draw(c, tx, ty, "HDR", 16, TEXT, 3)
        };
        let zx = self.hdr_schalter_zustand_x(r);
        self.text.draw(c, zx, mitte + 4, zustand, 11, zustand_farbe, 1);
        let (sw, sh) = (HDR_SCHALTER_BREITE, 16);
        let sx = r.x + r.w - sw - HDR_SCHALTER_RAND;
        c.rect(sx, mitte - sh / 2, sw, sh, accent, if an { 60 } else { 20 });
        let kx = if an { sx + sw - sh } else { sx };
        c.rect(kx, mitte - sh / 2, sh, sh, accent, 255);
        if gold {
            // Der Knopf glaenzt: eine helle Oberkante, eine tiefe Unterkante.
            c.hline(kx + 2, mitte - sh / 2 + 2, sh - 4, GOLD_HELL, 230);
            c.hline(kx + 2, mitte + sh / 2 - 2, sh - 4, GOLD_TIEF, 200);
            c.funkeln(Rect { x: tx - 6, y: r.y + 2, w: tende - tx + 10, h: r.h - 4 }, tick, 6, 6, 255, 1);
            c.funkeln(Rect { x: sx - 6, y: r.y + 2, w: sw + 12, h: r.h - 4 }, tick, 4, 5, 255, 2);
            c.funkeln(Rect { x: r.x + 4, y: r.y, w: r.w - 8, h: r.h }, tick, 4, 3, 200, 6);
        }
        if hot {
            c.hline(r.x, r.y + r.h - 1, r.w, accent, 90);
        }
        hot && self.click
    }

    /// Wo der Zustand im HDR-Schalter beginnt, und wie breit er hoechstens
    /// sein darf (bis vor den Schalter).
    pub fn hdr_schalter_zustand_x(&mut self, r: Rect) -> i32 {
        r.x + HDR_SCHALTER_RAND + self.text.width("HDR", 16, 3) + 14
    }

    pub fn hdr_schalter_zustand_breite(&mut self, r: Rect) -> i32 {
        r.x + r.w - HDR_SCHALTER_BREITE - HDR_SCHALTER_RAND - 12 - self.hdr_schalter_zustand_x(r)
    }

    /// Das goldene "HDR"-Abzeichen oben mittig ueber dem Bild, wenn HDR auf
    /// beiden Seiten aktiv wird (fuer rund 3 s, der Aufrufer blendet ueber
    /// `deckung` 0..=255 ein und aus): dunkle, warme Tafel mit goldenen
    /// Klammern, "HDR" gross im goldenen Verlauf mit wanderndem Glanz,
    /// darunter `unterzeile` (etwa "HDR10 · PQ"), und Glanzpunkte, die
    /// ueber Titel und Tafel funkeln - alles nach `self.tick`.
    pub fn hdr_abzeichen(&mut self, c: &mut Canvas, ww: u32, deckung: u32, unterzeile: &str) {
        let d = deckung.min(255);
        if d == 0 {
            return;
        }
        let tick = self.tick;
        let (gross, lauf) = (44u32, 10i32);
        let tw = self.text.width("HDR", gross, lauf) - lauf;
        let uw = self.text.width(unterzeile, 11, 3) - 3;
        let w = tw.max(uw) + 84;
        let h = 96;
        let x = ww as i32 / 2 - w / 2;
        let y = 28;
        c.fill(x, y, w, h, 0x0d0a04, 215 * d / 255);
        c.klammern(x, y, w, h, GOLD, d, 16);
        c.hline(x + 16, y, w - 32, GOLD, 60 * d / 255);
        c.hline(x + 16, y + h - 1, w - 32, GOLD, 60 * d / 255);
        let tx = ww as i32 / 2 - tw / 2;
        let ty = y + 58;
        self.text.draw_mit(c, tx, ty, "HDR", gross, lauf, |px, py| (gold_verlauf(px, py, tx, ty - 33, tw, 34, tick), d));
        // Ein goldener Strich mit Schein zwischen Titel und Unterzeile.
        let sy = ty + 9;
        for (dy, a) in [(0, 200u32), (-1, 60), (1, 60), (-2, 18), (2, 18)] {
            c.hline(tx - 6, sy + dy, tw + 12, GOLD, a * d / 255);
        }
        let ux = ww as i32 / 2 - uw / 2;
        self.text.draw_mit(c, ux, y + h - 12, unterzeile, 11, 3, |_, _| (GOLD_HELL, 220 * d / 255));
        c.funkeln(Rect { x: tx - 14, y: ty - 44, w: tw + 28, h: 50 }, tick, 7, 8, d, 3);
        c.funkeln(Rect { x: x + 4, y: y + 4, w: w - 8, h: h - 8 }, tick, 5, 4, d, 4);
    }

    /// Verlauf in einem gewaehlten Wertebereich statt ab null. Eine Reihe von
    /// 95 bis 120 Bildern sieht vom Nullpunkt aus wie eine gerade Wand - man
    /// sieht genau die Schwankung nicht, um die es geht.
    pub fn spark_range(&mut self, c: &mut Canvas, r: Rect, data: &[f32], lo: f32, hi: f32, color: u32) {
        c.rect(r.x, r.y, r.w, r.h, 0x060a11, 180);
        if data.is_empty() || hi <= lo { return; }
        let n = data.len().min(r.w as usize);
        for i in 0..n {
            let v = data[data.len() - n + i].clamp(lo, hi);
            let anteil = (v - lo) / (hi - lo);
            let hh = (anteil * r.h as f32) as i32;
            let x = r.x + r.w - n as i32 + i as i32;
            let deckung = if i + 1 == n { 255 } else { 150 };
            c.vline(x, r.y + r.h - hh, hh.max(1), color, deckung);
        }
    }

    /// Tastaturfokus: Eckklammern und ein Unterstrich. Ohne das waere das HUD
    /// im Vollbild nur mit der Maus bedienbar - und geoeffnet wurde es gerade
    /// mit der Tastatur. Noch ungenutzt: die Tastaturbedienung des Nerd-Modus
    /// steht aus, die Funktion wartet hier darauf.
    #[allow(dead_code)]
    pub fn focus(&mut self, c: &mut Canvas, r: Rect, accent: u32) {
        let k = 10;
        for (cx, cy, dx, dy) in [
            (r.x, r.y, 1, 1),
            (r.x + r.w - 1, r.y, -1, 1),
            (r.x, r.y + r.h - 1, 1, -1),
            (r.x + r.w - 1, r.y + r.h - 1, -1, -1),
        ] {
            for i in 0..k {
                c.px(cx + dx * i, cy, accent, 255);
                c.px(cx, cy + dy * i, accent, 255);
            }
        }
        c.hline(r.x, r.y + r.h, r.w, accent, 200);
    }

    /// Kleiner Verlaufsgraph, etwa fuer die Bildrate.
    pub fn spark(&mut self, c: &mut Canvas, r: Rect, data: &[f32], max: f32, color: u32) {
        c.rect(r.x, r.y, r.w, r.h, 0x060a11, 180);
        if data.is_empty() || max <= 0.0 { return; }
        let n = data.len().min(r.w as usize);
        for i in 0..n {
            let v = data[data.len() - n + i].clamp(0.0, max);
            let hh = (v / max * r.h as f32) as i32;
            let x = r.x + r.w - n as i32 + i as i32;
            c.vline(x, r.y + r.h - hh, hh, color, 150);
            c.px(x, r.y + r.h - hh, color, 255);
        }
    }
}
