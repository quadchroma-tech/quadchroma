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
            "C:\\Windows\\Fonts\\segoeui.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/PingFang.ttc",
        ];
        let mut fonts = Vec::new();
        for path in candidates {
            if let Ok(data) = std::fs::read(path) {
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

#[derive(Clone, Copy, PartialEq)]
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
}

impl Ui {
    pub fn new() -> Self {
        Ui { text: Text::load(), mouse: (-1, -1), click: false, tick: 0 }
    }

    /// Knopf mit Eckklammern. Gibt true zurueck, wenn er in diesem Bild geklickt wurde.
    pub fn button(&mut self, c: &mut Canvas, r: Rect, label: &str, accent: u32) -> bool {
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
        let ty = r.y + r.h / 2 + 5;
        self.text.draw_centered(c, r.x + r.w / 2, ty, label, 15, if hot { 0xffffff } else { TEXT }, 2);
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
