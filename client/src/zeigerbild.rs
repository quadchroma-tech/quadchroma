// QuadChroma: der Massstab der Zeigerform - eine Regel fuer den Client und
// fuer den Windows-Host, der sie beim Einpassen fuer H.264 braucht.
//
// Vertrag (Nachricht 49): der Host schickt die Form in Bildpunkten DES
// STROMS - so gross, wie der Zeiger im gestreamten Bild erscheint. Der
// Mac-Host zeichnet sie im Massstab Strombildpunkte je Punkt (HiDPI "wie
// 1920x1080" auf einem 4K-Panel: 2, aus der 2x-Darstellung; skaliert "wie
// 2560x1440" am 4K-Panel: 1,5; "wie 1512x982" am Laptop, 3024x1964: 2), der
// Windows-Host liefert die Form der Duplication in Desktoppunkten, und die
// sind Strombildpunkte, denn der Strom ist nativ. Passt ein Host den Strom
// fuer H.264 ein (groesser als 4096 Bildpunkte je Seite oder 4096x2304),
// bringt er die Form ins selbe Mass.
//
// Der Client zeichnet sie genau so gross, wie das Bild im Fenster
// erscheint: Massstab = gezeigte Breite / Strombreite; auf dem Mac-Client
// geteilt durch den Backing-Faktor des Fensters, denn winit baut den Zeiger
// in Punkten (ein Bildpunkt der Form wird ein Punkt, auf Retina zwei
// Bildpunkte). Vergroessert ganzzahlig, Punkt fuer Punkt (1,5 -> 2, 1,4 ->
// 1): eine kleine Strichzeichnung bliebe gefiltert nicht scharf. Verkleinert
// genau, flaechengemittelt mit vormultiplizierter Deckkraft: 0,55 bleibt
// 0,55 - ein Runden auf "durch 2" halbierte einen Zeiger, der nur um den
// Faktor 1,82 kleiner erscheinen soll (3024 breiter Strom in einem 1663
// Bildpunkte breiten Bild).

/// Groesser als so viele Bildpunkte je Seite wird keine Form gebaut (der
/// Host passt groessere ein, beim Vergroessern gilt eine Stufe weniger).
pub const MAX: u32 = 256;
/// Kleiner als so viele Bildpunkte (die groessere Seite) wird eine Form beim
/// Verkleinern nicht - ein Bild in einem sehr kleinen Fenster soll noch
/// einen greifbaren Zeiger haben. Eine Form, die selbst kleiner ist, bleibt,
/// wie sie ist.
pub const MIN: u32 = 16;

/// Wie gross eine Form gezeichnet wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Massstab {
    /// Ganzzahlig vergroessert, Punkt fuer Punkt (1 = unveraendert), 1..8.
    Mal(u32),
    /// Verkleinert auf so viele Tausendstel (1..999), flaechengemittelt.
    Promille(u32),
}

impl Massstab {
    pub const EINS: Massstab = Massstab::Mal(1);

    /// Der Massstab fuer ein Bild, das `fw` Bildpunkte breit gestreamt wird
    /// und im Fenster `zw` Bildpunkte breit erscheint (ziel_rechteck), bei
    /// einem Backing-Faktor `backing` (Mac-Client: Bildpunkte je Punkt des
    /// Fensters, sonst 1) und einer Form `form` (Breite, Hoehe). Ohne Bild
    /// oder mit unbrauchbaren Werten: 1.
    pub fn fuer_bild(zw: u32, fw: u32, backing: f64, form: Option<(u16, u16)>) -> Massstab {
        if zw == 0 || fw == 0 || !backing.is_finite() || backing <= 0.0 {
            return Massstab::EINS;
        }
        Massstab::aus_faktor(zw as f64 / fw as f64 / backing, form)
    }

    /// Der Massstab zum Faktor `t`: ab 1 (auf Tausendstel gerundet)
    /// ganzzahlig gerundet, hoechstens 8 und so, dass die Form nicht ueber
    /// MAX waechst; darunter genau in Tausendsteln, mit einer Form nicht
    /// unter MIN. Unbrauchbar (0, negativ, NaN): 1.
    pub fn aus_faktor(t: f64, form: Option<(u16, u16)>) -> Massstab {
        if !t.is_finite() || t <= 0.0 {
            return Massstab::EINS;
        }
        let promille = (t * 1000.0).round();
        if promille >= 1000.0 {
            let mut f = (t + 0.5).floor().clamp(1.0, 8.0) as u32;
            if let Some((w, h)) = form {
                while f > 1 && (w as u32 * f > MAX || h as u32 * f > MAX) {
                    f -= 1;
                }
            }
            return Massstab::Mal(f);
        }
        let mut p = (promille as u32).max(1);
        if let Some((w, h)) = form {
            let seite = w.max(h) as u32;
            if seite > 0 {
                p = p.max((MIN.min(seite) * 1000).div_ceil(seite));
            }
        }
        if p >= 1000 { Massstab::EINS } else { Massstab::Promille(p) }
    }

    /// Fuer den Vorrat gebauter Zeiger: jeder Massstab eine andere Zahl.
    pub fn kennung(self) -> u64 {
        match self {
            Massstab::Mal(f) => f as u64,
            Massstab::Promille(p) => (1 << 16) | p as u64,
        }
    }

    /// Der Faktor als Zahl (fuers Protokoll).
    #[cfg_attr(not(any(test, windows)), allow(dead_code))]
    pub fn faktor(self) -> f64 {
        match self {
            Massstab::Mal(f) => f as f64,
            Massstab::Promille(p) => p as f64 / 1000.0,
        }
    }

    /// Die Groesse einer Form w x h in diesem Massstab (wie skalieren).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn groesse(self, w: u16, h: u16) -> (u16, u16) {
        match self {
            Massstab::Mal(f) => {
                let f = mal_begrenzt(f, w as usize, h as usize);
                ((w as usize * f) as u16, (h as usize * f) as u16)
            }
            Massstab::Promille(p) => (anteil(w as usize, p) as u16, anteil(h as usize, p) as u16),
        }
    }
}

/// Der Vergroesserungsfaktor, eine Stufe kleiner, solange die Form ueber MAX kaeme.
fn mal_begrenzt(f: u32, w: usize, h: usize) -> usize {
    let mut f = f.clamp(1, 8) as usize;
    while f > 1 && (w * f > MAX as usize || h * f > MAX as usize) {
        f -= 1;
    }
    f
}

/// n in p Tausendsteln, gerundet, mindestens 1.
fn anteil(n: usize, p: u32) -> usize {
    ((n * p as usize + 500) / 1000).max(1)
}

/// Die Form (RGBA mit gerader Deckkraft, w x h, Hotspot hx/hy) im Massstab
/// `m`: (RGBA, Breite, Hoehe, Hotspot x, Hotspot y). Vergroessert Punkt fuer
/// Punkt; verkleinert flaechengemittelt - jeder Zielpunkt mittelt genau die
/// Flaeche der Quelle, die er abdeckt, nach Deckkraft gewichtet (sonst
/// faerbte das Durchsichtige, oft schwarz oder gruen gespeichert, die Raender).
/// Der Hotspot folgt im selben Mass und bleibt in der Form. Eine Form, deren
/// Daten nicht zur Groesse passen, bleibt unveraendert.
pub fn skalieren(rgba: &[u8], w: u16, h: u16, hx: u16, hy: u16, m: Massstab) -> (Vec<u8>, u16, u16, u16, u16) {
    let (wq, hq) = (w as usize, h as usize);
    if wq == 0 || hq == 0 || rgba.len() < wq * hq * 4 {
        return (rgba.to_vec(), w, h, hx, hy);
    }
    let p = match m {
        Massstab::Mal(f) => {
            let f = mal_begrenzt(f, wq, hq);
            if f == 1 {
                return (rgba[..wq * hq * 4].to_vec(), w, h, hx, hy);
            }
            let (w2, h2) = (wq * f, hq * f);
            let mut aus = vec![0u8; w2 * h2 * 4];
            for y in 0..h2 {
                let qy = y / f;
                for x in 0..w2 {
                    let q = (qy * wq + x / f) * 4;
                    let z = (y * w2 + x) * 4;
                    aus[z..z + 4].copy_from_slice(&rgba[q..q + 4]);
                }
            }
            return (aus, w2 as u16, h2 as u16, hx.saturating_mul(f as u16), hy.saturating_mul(f as u16));
        }
        Massstab::Promille(p) => p,
    };
    let (w2, h2) = (anteil(wq, p), anteil(hq, p));
    if (w2, h2) == (wq, hq) {
        return (rgba[..wq * hq * 4].to_vec(), w, h, hx, hy);
    }
    let (sx, sy) = (wq as f64 / w2 as f64, hq as f64 / h2 as f64);
    let mut aus = vec![0u8; w2 * h2 * 4];
    for oy in 0..h2 {
        let (y0, y1) = (oy as f64 * sy, (oy + 1) as f64 * sy);
        for ox in 0..w2 {
            let (x0, x1) = (ox as f64 * sx, (ox + 1) as f64 * sx);
            let (mut r, mut g, mut b, mut a, mut flaeche) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for qy in (y0.floor() as usize)..(y1.ceil() as usize).min(hq) {
                let wy = (y1.min(qy as f64 + 1.0) - y0.max(qy as f64)).max(0.0);
                for qx in (x0.floor() as usize)..(x1.ceil() as usize).min(wq) {
                    let gewicht = wy * (x1.min(qx as f64 + 1.0) - x0.max(qx as f64)).max(0.0);
                    let q = (qy * wq + qx) * 4;
                    let deck = rgba[q + 3] as f64 / 255.0 * gewicht;
                    r += rgba[q] as f64 * deck;
                    g += rgba[q + 1] as f64 * deck;
                    b += rgba[q + 2] as f64 * deck;
                    a += deck;
                    flaeche += gewicht;
                }
            }
            if a > 0.0 && flaeche > 0.0 {
                let z = (oy * w2 + ox) * 4;
                aus[z] = (r / a).round().clamp(0.0, 255.0) as u8;
                aus[z + 1] = (g / a).round().clamp(0.0, 255.0) as u8;
                aus[z + 2] = (b / a).round().clamp(0.0, 255.0) as u8;
                aus[z + 3] = (a / flaeche * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    let hx2 = (((hx as f64 + 0.5) * w2 as f64 / wq as f64).floor() as usize).min(w2 - 1);
    let hy2 = (((hy as f64 + 0.5) * h2 as f64 / hq as f64).floor() as usize).min(h2 - 1);
    (aus, w2 as u16, h2 as u16, hx2 as u16, hy2 as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voll(w: u16, h: u16, px: [u8; 4]) -> Vec<u8> {
        px.repeat(w as usize * h as usize)
    }

    /// Die groessere Seite einer Form w x w im Massstab des Bildes.
    fn gezeigt(zw: u32, fw: u32, backing: f64, w: u16) -> u16 {
        let m = Massstab::fuer_bild(zw, fw, backing, Some((w, w)));
        let (_, w2, _, _, _) = skalieren(&voll(w, w, [1, 2, 3, 255]), w, w, 0, 0, m);
        assert_eq!(m.groesse(w, w).0, w2, "groesse und skalieren stimmen ueberein");
        w2
    }

    /// Die alten Faelle beider Zweige und die neuen, alle mit der einen Regel
    /// "gezeigte Groesse / Stromgroesse".
    #[test]
    fn massstab_folgt_dem_bild() {
        // Mac-Laptop "wie 1512x982": Strom 3024x1964, der Host schickt den
        // 32-Punkt-Zeiger mit 64 Bildpunkten. Im 1663 breiten Bild: 0,55 -
        // der Zeiger ist 35 Bildpunkte gross (32 Punkte * 1663/1512), nicht
        // 32 (das Runden 1,82 -> durch 2 halbierte ihn) und nicht 64.
        assert_eq!(Massstab::fuer_bild(1663, 3024, 1.0, Some((64, 64))), Massstab::Promille(550));
        assert_eq!(gezeigt(1663, 3024, 1.0, 64), 35);
        // Derselbe Strom 1:1 (Fenster so gross wie der Strom): 64.
        assert_eq!(gezeigt(3024, 3024, 1.0, 64), 64);
        // 4K-Strom in einem 1920 breiten Bild: halb so gross - Windows-Host
        // 4K bei 100 % (32 -> 16), bei 150 % (48 -> 24), HiDPI-Mac (64 -> 32).
        assert_eq!(Massstab::fuer_bild(1920, 3840, 1.0, Some((32, 32))), Massstab::Promille(500));
        assert_eq!(gezeigt(1920, 3840, 1.0, 32), 16);
        assert_eq!(gezeigt(1920, 3840, 1.0, 48), 24);
        assert_eq!(gezeigt(1920, 3840, 1.0, 64), 32);
        // 4K-Strom in einem 2560 breiten Bild: 0,67, nicht 1 und nicht 0,5.
        assert_eq!(gezeigt(2560, 3840, 1.0, 64), 43);
        assert_eq!(gezeigt(2880, 3840, 1.0, 32), 24);
        // HiDPI 2x am Mac-Host, 1:1 gezeigt: die 2x-Form unveraendert.
        assert_eq!(Massstab::fuer_bild(3840, 3840, 1.0, Some((64, 64))), Massstab::EINS);
        // Retina-Client (Backing 2): der 4K-Strom 1:1 auf 3840 Bildpunkten
        // sind 1920 Punkte - die 64er-Form wird 32 Punkte, also wieder 64
        // Bildpunkte; ein 1080p-Strom auf 2880 Bildpunkten (1,5) 24 Punkte
        // = 48 Bildpunkte; auf 3840 Bildpunkten (2) unveraendert.
        assert_eq!(gezeigt(3840, 3840, 2.0, 64), 32);
        assert_eq!(gezeigt(2880, 1920, 2.0, 32), 24);
        assert_eq!(Massstab::fuer_bild(3840, 1920, 2.0, Some((32, 32))), Massstab::EINS);
        assert_eq!(gezeigt(1663, 3024, 2.0, 64), 18);
        // Vergroessern ganzzahlig: 2 -> 2, 1,33 -> 1, 1,5 -> 2, nie ueber MAX.
        assert_eq!(Massstab::fuer_bild(3840, 1920, 1.0, Some((32, 32))), Massstab::Mal(2));
        assert_eq!(Massstab::fuer_bild(2560, 1920, 1.0, Some((32, 32))), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(2880, 1920, 1.0, Some((32, 32))), Massstab::Mal(2));
        assert_eq!(Massstab::fuer_bild(3840, 1920, 1.0, Some((200, 200))), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(20000, 1000, 1.0, Some((16, 16))), Massstab::Mal(8));
        assert_eq!(gezeigt(1916, 1920, 1.0, 32), 32, "0,998: dieselbe Groesse, unveraendert");
        // Sehr kleine Fenster: genau, aber nicht unter MIN.
        assert_eq!(gezeigt(960, 3840, 1.0, 64), 16);
        assert_eq!(Massstab::fuer_bild(640, 3840, 1.0, Some((64, 64))), Massstab::Promille(250));
        assert_eq!(Massstab::fuer_bild(960, 3840, 1.0, Some((16, 16))), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(960, 3840, 1.0, Some((12, 12))), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(960, 3840, 1.0, None), Massstab::Promille(250));
        // Ohne Bild, ohne Fenster, unbrauchbarer Backing-Faktor: 1.
        assert_eq!(Massstab::fuer_bild(0, 3840, 1.0, None), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(1920, 0, 1.0, None), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(1920, 3840, 0.0, None), Massstab::EINS);
        assert_eq!(Massstab::fuer_bild(1920, 3840, f64::NAN, None), Massstab::EINS);
        assert_eq!(Massstab::aus_faktor(f64::INFINITY, None), Massstab::EINS);
        // Jeder Massstab eine eigene Kennung.
        assert_ne!(Massstab::Mal(2).kennung(), Massstab::Promille(2).kennung());
        assert_ne!(Massstab::Promille(500).kennung(), Massstab::Promille(550).kennung());
        assert_eq!((Massstab::Mal(2).faktor(), Massstab::Promille(800).faktor()), (2.0, 0.8));
        // Der Windows-Host, fuer H.264 eingepasst (5120 -> 4096): 0,8, ohne Untergrenze.
        assert_eq!(Massstab::aus_faktor(4096.0 / 5120.0, None), Massstab::Promille(800));
        assert_eq!(Massstab::aus_faktor(3839.0 / 3840.0, None), Massstab::EINS, "ungerader Rand: 1");
    }

    /// Hochziehen Punkt fuer Punkt (Hotspot mit), hoechstens MAX je Seite;
    /// Verkleinern flaechengemittelt mit vormultiplizierter Deckkraft.
    #[test]
    fn skalieren_hoch_und_runter() {
        let rot = [255u8, 0, 0, 255];
        let leer = [0u8, 255, 0, 0];
        let mut px = Vec::new();
        for i in 0..4 {
            px.extend_from_slice(if i % 2 == 0 { &rot } else { &leer });
        }
        // 2x2 -> 4x4, Hotspot mit.
        let (rgba, w, h, hx, hy) = skalieren(&px, 2, 2, 1, 1, Massstab::Mal(2));
        assert_eq!((w, h, hx, hy), (4, 4, 2, 2));
        assert_eq!(&rgba[0..8], &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &leer);
        // Auf die Haelfte: das durchsichtige Gruen faerbt nichts, Deckkraft halb.
        let (rgba, w, h, hx, hy) = skalieren(&px, 2, 2, 1, 1, Massstab::Promille(500));
        assert_eq!((w, h, hx, hy), (1, 1, 0, 0));
        assert_eq!(rgba, vec![255, 0, 0, 128]);
        // 1:1 unveraendert.
        let (rgba, w, h, hx, hy) = skalieren(&px, 2, 2, 1, 1, Massstab::EINS);
        assert_eq!((rgba, w, h, hx, hy), (px.clone(), 2, 2, 1, 1));

        // 3x2 auf die Haelfte (2x1): jeder Zielpunkt mittelt genau
        // anderthalb Spalten. Zeile 0: rot, rot, weiss (deckend); Zeile 1:
        // durchsichtig schwarz, rot halb, weiss - links bleibt rein rot (das
        // schwarze Durchsichtige dunkelt nicht nach).
        let quelle = vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255, //
            0, 0, 0, 0, 255, 0, 0, 128, 255, 255, 255, 255,
        ];
        let (rgba, w, h, hx, hy) = skalieren(&quelle, 3, 2, 2, 1, Massstab::Promille(500));
        assert_eq!((w, h, hx, hy), (2, 1, 1, 0));
        assert_eq!(&rgba[0..4], &[255, 0, 0, 149]);
        assert_eq!(&rgba[4..8], &[255, 185, 185, 234]);
        // Ganz durchsichtig bleibt ganz durchsichtig.
        let (rgba, w, h, _, _) = skalieren(&[0u8; 64], 4, 4, 3, 3, Massstab::Promille(250));
        assert_eq!((rgba, w, h), (vec![0; 4], 1, 1));

        // Eine HiDPI-Form (64, Hotspot 8,8) auf 0,67: 43 Bildpunkte, Hotspot
        // mit; voll deckend bleibt voll. Auf die Haelfte mit Hotspot am Rand:
        // 32, Hotspot 31.
        let z = [10u8, 20, 30, 255].repeat(64 * 64);
        let (rgba, w, h, hx, hy) = skalieren(&z, 64, 64, 8, 8, Massstab::Promille(667));
        assert_eq!((w, h, hx, hy), (43, 43, 5, 5));
        assert!(rgba.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
        let (rgba, w, h, hx, hy) = skalieren(&z, 64, 64, 63, 63, Massstab::Promille(500));
        assert_eq!((w, h, hx, hy), (32, 32, 31, 31));
        assert_eq!(rgba.len(), 32 * 32 * 4);
        // Fuer H.264 eingepasst (Windows-Host, 5120 -> 4096 breit): 0,8.
        let (_, w, h, hx, hy) = skalieren(&z, 64, 64, 63, 0, Massstab::Promille(800));
        assert_eq!((w, h, hx, hy), (51, 51, 50, 0));

        // 100 Bildpunkte mal 3 waeren 300: eine Stufe kleiner.
        let (rgba, w, h, hx, hy) = skalieren(&vec![0u8; 100 * 100 * 4], 100, 100, 99, 0, Massstab::Mal(3));
        assert_eq!((w, h, hx, hy), (200, 200, 198, 0));
        assert_eq!(rgba.len(), 200 * 200 * 4);
        assert_eq!(Massstab::Mal(3).groesse(100, 100), (200, 200));
        // Zu kurze Daten: unveraendert zurueck, kein Lesen hinter dem Ende.
        let (rgba, w, h, _, _) = skalieren(&[1, 2, 3], 2, 2, 0, 0, Massstab::Promille(500));
        assert_eq!((rgba, w, h), (vec![1, 2, 3], 2, 2));
    }
}
