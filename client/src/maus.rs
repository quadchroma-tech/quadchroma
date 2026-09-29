// Maus und Zeiger fuer Spiele: reine Entscheidungen, auf beiden Plattformen
// uebersetzt und geprueft. Client und Windows-Host nehmen sie von hier; der
// Mac-Host hat dieselben Regeln in C (host/zeiger.m, qc_fang_schritt).
//
// Drei Dinge:
//
// 1. Zeigerform (MSG_CURSOR): Kopf lesen und schreiben, samt Byte 10
//    (Merkmale, Bit 0 ZEIGER_GEFANGEN). Beide Hosts schicken die Form in
//    STROMPUNKTEN (ein Formpunkt = ein Punkt des Stroms) - so haben es Clients
//    bis 0.2.0 immer angenommen.
// 2. Groesse: der Client zeichnet die Form genau im Massstab des Bildes -
//    Bildpunkte im Fenster je Strompunkt -, geteilt durch den Massstab seiner
//    Zeiger-Schnittstelle (macOS rechnet ein NSImage in Punkten, auf einem
//    Retina-Schirm also doppelt; Windows in Bildpunkten). Vorher wurde auf
//    eine ganze Zahl gerundet und auf dem Mac die Retina-Verdopplung
//    obendrauf gerechnet: auf einem Retina-Mac mit 1,5-fachem Bild kam der
//    Zeiger 2,7-mal so gross wie das Bild.
// 3. Einfangen: der Host erkennt, dass seine Anwendung die Maus einfaengt
//    (Zeiger versteckt und eingesperrt, zurueckgesetzt oder trotz Bewegung
//    versteckt), und sagt es dem Client im Merkmal. Der Client faengt dann
//    seinen eigenen Zeiger ein und schickt Bewegungen relativ (IN_MOVE_REL);
//    der Host speist sie relativ ein - Spiele mit Raw Input sehen sie, und am
//    Bildrand ist nicht mehr Schluss.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::protokoll_konst::ZEIGER_GEFANGEN;

// ------------------------------------------------------------ Zeigerform

/// Groesser schickt kein Host eine Form, und groesser baut der Client keinen
/// Zeiger (Windows zeigt groessere Zeiger nicht verlaesslich).
pub const ZEIGER_MAX: u32 = 256;
/// Laenge des Kopfes von MSG_CURSOR.
pub const ZEIGER_KOPF: usize = 12;

/// Eine Zeigerform, wie sie ueber die Leitung geht.
#[derive(Clone, Debug, PartialEq)]
pub struct ZeigerNachricht {
    pub w: u16,
    pub h: u16,
    pub hx: u16,
    pub hy: u16,
    /// Byte 8: soll der Client einen Zeiger zeigen? Verlaesslich nur von einem
    /// Host mit FAEHIG_MAUS; aeltere Mac-Hosts melden hier auch das
    /// Verstecken beim Tippen, das nie endet.
    pub sichtbar: bool,
    /// Byte 10 Bit 0: die Anwendung des Hosts hat die Maus eingefangen.
    pub gefangen: bool,
    /// RGBA mit gerader Deckkraft, w*h*4 Byte.
    pub rgba: Vec<u8>,
}

/// MSG_CURSOR lesen: Groesse plausibel, Bild vollstaendig, Hotspot im Bild -
/// alles andere ist kein Zeiger. Byte 9 (Massstab, immer 1) und Byte 11
/// werden uebergangen; ein Host bis 0.2.0 schickt in Byte 10 eine 0.
pub fn zeiger_lesen(p: &[u8]) -> Option<ZeigerNachricht> {
    if p.len() < ZEIGER_KOPF {
        return None;
    }
    let u16le = |o: usize| u16::from_le_bytes([p[o], p[o + 1]]);
    let (w, h, hx, hy) = (u16le(0), u16le(2), u16le(4), u16le(6));
    let n = w as usize * h as usize * 4;
    let passt = (1..=ZEIGER_MAX as u16).contains(&w) && (1..=ZEIGER_MAX as u16).contains(&h) && p.len() == ZEIGER_KOPF + n && hx < w && hy < h;
    if !passt {
        return None;
    }
    Some(ZeigerNachricht { w, h, hx, hy, sichtbar: p[8] != 0, gefangen: p[10] & ZEIGER_GEFANGEN != 0, rgba: p[ZEIGER_KOPF..].to_vec() })
}

/// MSG_CURSOR schreiben (Windows-Host; der Mac-Host schreibt denselben Kopf
/// in host/main.m zeiger_cb).
pub fn zeiger_kodieren(z: &ZeigerNachricht) -> Vec<u8> {
    let mut p = Vec::with_capacity(ZEIGER_KOPF + z.rgba.len());
    p.extend_from_slice(&z.w.to_le_bytes());
    p.extend_from_slice(&z.h.to_le_bytes());
    p.extend_from_slice(&z.hx.to_le_bytes());
    p.extend_from_slice(&z.hy.to_le_bytes());
    p.push(z.sichtbar as u8);
    // Massstab: 1 = ein Formpunkt je Strompunkt (seit jeher).
    p.push(1);
    p.push(if z.gefangen { ZEIGER_GEFANGEN } else { 0 });
    p.push(0);
    p.extend_from_slice(&z.rgba);
    p
}

/// Platzhalter fuer "versteckt", solange der Host noch keine Form kennt
/// (die Duplication liefert keine, wenn der Zeiger schon beim Start
/// versteckt war): ein durchsichtiger Punkt. Ein Client bis 0.2.0 zeigt ihn
/// als unsichtbaren Zeiger - bei einem wirklich versteckten Zeiger richtig.
pub fn platzhalter(gefangen: bool) -> ZeigerNachricht {
    ZeigerNachricht { w: 1, h: 1, hx: 0, hy: 0, sichtbar: false, gefangen, rgba: vec![0, 0, 0, 0] }
}

// ----------------------------------------------------------------- Groesse

/// Massstab fuer die Zeiger-Schnittstelle des Clients: so viele Einheiten der
/// Schnittstelle je Formpunkt. `bild_px`: Breite des Bildes im Fenster in
/// Bildpunkten des Bildschirms; `strom_px`: Breite des Stroms (ein Formpunkt
/// ist ein Strompunkt); `api_skala`: Bildpunkte je Einheit der Schnittstelle
/// - auf dem Mac der backingScaleFactor des Fensters (winit baut das NSImage
/// in Punkten: ein Bild mit w Punkten erscheint auf Retina 2w Bildpunkte
/// gross), unter Windows 1 (ein Zeiger aus CreateIconIndirect hat seine
/// Bildpunkte). Damit ist der Zeiger genau so gross, wie er im Bild waere.
pub fn zeiger_skala(bild_px: u32, strom_px: u32, api_skala: f64) -> f64 {
    if bild_px == 0 || strom_px == 0 {
        return 1.0;
    }
    let api = if api_skala.is_finite() && api_skala > 0.0 { api_skala } else { 1.0 };
    (bild_px as f64 / strom_px as f64 / api).clamp(1.0 / 16.0, 16.0)
}

/// Zielgroesse einer Seite: gerundet, mindestens 1.
fn seite(n: u16, skala: f64) -> u32 {
    ((n as f64 * skala).round() as u32).max(1)
}

/// Die Form in der Groesse fuer die Schnittstelle. Bei ganzzahligem
/// Vergroessern Punkt fuer Punkt (scharf, wie bisher), sonst als
/// Flaechenmittel mit vormultiplizierter Deckkraft (kein Farbsaum an
/// durchsichtigen Raendern). Hoechstens ZEIGER_MAX je Seite - dann kleiner,
/// der Hotspot im selben Mass. Liefert (w, h, hx, hy, rgba).
pub fn zeiger_skalieren(w: u16, h: u16, hx: u16, hy: u16, rgba: &[u8], skala: f64) -> (u16, u16, u16, u16, Vec<u8>) {
    let skala = if skala.is_finite() && skala > 0.0 { skala } else { 1.0 };
    let (w0, h0) = (w as usize, h as usize);
    if w0 == 0 || h0 == 0 || rgba.len() < w0 * h0 * 4 {
        return (w, h, hx, hy, rgba.to_vec());
    }
    let mut s = skala;
    let groesste = w.max(h) as f64;
    if groesste * s > ZEIGER_MAX as f64 {
        s = ZEIGER_MAX as f64 / groesste;
    }
    let (w2, h2) = (seite(w, s).min(ZEIGER_MAX) as usize, seite(h, s).min(ZEIGER_MAX) as usize);
    let hx2 = ((hx as f64 * w2 as f64 / w0 as f64).floor() as usize).min(w2 - 1);
    let hy2 = ((hy as f64 * h2 as f64 / h0 as f64).floor() as usize).min(h2 - 1);
    if (w2, h2) == (w0, h0) {
        return (w, h, hx, hy, rgba[..w0 * h0 * 4].to_vec());
    }
    let mut aus = vec![0u8; w2 * h2 * 4];
    // Ganzzahlig vergroessert: Punkt fuer Punkt.
    if w2 % w0 == 0 && h2 % h0 == 0 && w2 / w0 == h2 / h0 {
        let f = w2 / w0;
        for y in 0..h2 {
            for x in 0..w2 {
                let q = ((y / f) * w0 + x / f) * 4;
                aus[(y * w2 + x) * 4..(y * w2 + x) * 4 + 4].copy_from_slice(&rgba[q..q + 4]);
            }
        }
        return (w2 as u16, h2 as u16, hx2 as u16, hy2 as u16, aus);
    }
    let sx = anteile(w0, w2);
    let sy = anteile(h0, h2);
    for (y, zy) in sy.iter().enumerate() {
        for (x, zx) in sx.iter().enumerate() {
            let mut summe = [0f64; 4];
            for &(qy, gy) in zy {
                for &(qx, gx) in zx {
                    let q = (qy * w0 + qx) * 4;
                    let g = gx * gy;
                    let a = rgba[q + 3] as f64 / 255.0;
                    summe[0] += rgba[q] as f64 * a * g;
                    summe[1] += rgba[q + 1] as f64 * a * g;
                    summe[2] += rgba[q + 2] as f64 * a * g;
                    summe[3] += a * g;
                }
            }
            let i = (y * w2 + x) * 4;
            if summe[3] > 1e-6 {
                for k in 0..3 {
                    aus[i + k] = (summe[k] / summe[3]).round().clamp(0.0, 255.0) as u8;
                }
                aus[i + 3] = (summe[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (w2 as u16, h2 as u16, hx2 as u16, hy2 as u16, aus)
}

/// Je Zielpunkt die Quellpunkte, die er ueberdeckt, mit ihrem Anteil
/// (Summe 1): Ziel i deckt [i*n/m, (i+1)*n/m) der Quelle.
fn anteile(n: usize, m: usize) -> Vec<Vec<(usize, f64)>> {
    let f = n as f64 / m as f64;
    (0..m)
        .map(|i| {
            let (a, b) = (i as f64 * f, (i + 1) as f64 * f);
            let mut v = Vec::new();
            let mut q = a.floor() as usize;
            while (q as f64) < b && q < n {
                let anteil = (b.min(q as f64 + 1.0) - a.max(q as f64)) / (b - a);
                if anteil > 0.0 {
                    v.push((q, anteil));
                }
                q += 1;
            }
            v
        })
        .collect()
}

// ------------------------------------------------ Relative Bewegung (72)

/// Hoechstens so viele Zaehler je Nachricht und Achse - mehr ist kein
/// Mausweg, sondern Unfug.
pub const REL_GRENZE: f32 = 16384.0;

/// IN_MOVE_REL: f32 dx, f32 dy (little endian).
pub fn rel_kodieren(dx: f32, dy: f32) -> [u8; 8] {
    let mut p = [0u8; 8];
    p[0..4].copy_from_slice(&dx.to_le_bytes());
    p[4..8].copy_from_slice(&dy.to_le_bytes());
    p
}

/// IN_MOVE_REL lesen: mindestens acht Byte (dahinter bleibt Platz fuer
/// spaetere Fassungen), endliche Werte, auf REL_GRENZE geklemmt.
pub fn rel_lesen(p: &[u8]) -> Option<(f32, f32)> {
    if p.len() < 8 {
        return None;
    }
    let dx = f32::from_le_bytes([p[0], p[1], p[2], p[3]]);
    let dy = f32::from_le_bytes([p[4], p[5], p[6], p[7]]);
    if !dx.is_finite() || !dy.is_finite() {
        return None;
    }
    Some((dx.clamp(-REL_GRENZE, REL_GRENZE), dy.clamp(-REL_GRENZE, REL_GRENZE)))
}

/// Bruchteile sammeln, ganze Zaehler abgeben: SendInput und CGEvent wollen
/// ganze Zahlen, ein Trackpad liefert Bruchteile - ohne den Rest ginge eine
/// langsame Bewegung ganz verloren.
#[derive(Clone, Copy, Debug, Default)]
pub struct RelRest {
    x: f64,
    y: f64,
}

impl RelRest {
    /// Leer - fuer statische Zustaende (const).
    pub const NEU: RelRest = RelRest { x: 0.0, y: 0.0 };

    pub fn nehmen(&mut self, dx: f32, dy: f32) -> (i32, i32) {
        self.x += dx as f64;
        self.y += dy as f64;
        let (gx, gy) = (self.x.trunc(), self.y.trunc());
        self.x -= gx;
        self.y -= gy;
        (gx as i32, gy as i32)
    }

    pub fn leeren(&mut self) {
        *self = RelRest::default();
    }
}

// ------------------------------------------------- Host: Einfangen erkennen

/// Was der Host in einem Durchgang ueber seinen Zeiger weiss.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZeigerLage {
    /// Das System zeigt keinen Zeiger (Windows: GetCursorInfo ohne
    /// CURSOR_SHOWING oder ohne Form; Mac: CGCursorIsVisible falsch).
    pub versteckt: bool,
    /// Eingesperrt auf weniger als den ganzen Desktop (Windows: ClipCursor,
    /// siehe `eingesperrt`). Der Mac kennt das nicht.
    pub eingesperrt: bool,
    /// Der Zeiger steht nicht dort, wohin ihn die letzte absolute Bewegung
    /// setzte (die Anwendung setzt ihn zurueck, auf dem Mac auch: sie hat
    /// ihn abgekoppelt) - nur, wenn die Bewegung 30 ms bis 2 s her ist.
    pub folgt_nicht: bool,
    /// Das Fenster im Vordergrund deckt seinen Bildschirm ganz (Vollbild,
    /// auch randlos) - Spiele ja, Zeichenprogramme mit eigenem Pinselzeiger
    /// meist nicht. Nur die Bewegungsregel fragt danach.
    pub vollbild: bool,
}

/// Was die Eingabe des Hosts bisher eingespeist hat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EingabeZaehler {
    /// Eingespeiste Mausbewegungen seit dem Start, absolut und relativ.
    pub bewegungen: u64,
    /// Wann zuletzt eine Taste gedrueckt wurde (Hostuhr, us; 0 = nie).
    pub letzte_taste_us: u64,
}

/// Wie ein Host seinen Zeiger beurteilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FangArt {
    /// Verstecken beim Tippen endet mit einer eingespeisten Bewegung
    /// (Windows ja: "Zeiger beim Tippen ausblenden" zeigt ihn bei jeder
    /// Bewegung wieder; macOS nein: erst die echte Maus des Macs holt ihn
    /// zurueck).
    pub tippen_endet_mit_bewegung: bool,
    /// Versteckt trotz Bewegung reicht zum Einfangen, wenn das Fenster im
    /// Vordergrund Vollbild ist (Windows ja - fuer Spiele, die den Zeiger
    /// weder einsperren noch zuruecksetzen; auf dem Mac bliebe der beim
    /// Tippen versteckte Zeiger versteckt - nein).
    pub bewegung_regel: bool,
    /// "Versteckt" nur melden, wenn eingefangen (Mac: CGCursorIsVisible
    /// meldet auch das Verstecken beim Tippen, das nie endet - frueher der
    /// Grund, warum der Client die Sichtbarkeit ganz ueberging).
    pub versteckt_nur_mit_fang: bool,
}

pub const FANG_WINDOWS: FangArt = FangArt { tippen_endet_mit_bewegung: true, bewegung_regel: true, versteckt_nur_mit_fang: false };
/// Die Regeln des Mac-Hosts - dort in C (QC_FANG_MAC in host/zeiger.m); hier
/// fuer die Pruefung derselben Faelle.
#[allow(dead_code)]
pub const FANG_MAC: FangArt = FangArt { tippen_endet_mit_bewegung: false, bewegung_regel: false, versteckt_nur_mit_fang: true };

/// Gruende im Urteil (fuer das Protokoll).
pub const GRUND_EINGESPERRT: u8 = 1;
pub const GRUND_FOLGT_NICHT: u8 = 2;
pub const GRUND_BEWEGUNG: u8 = 4;

/// Eine Taste bis so lange vor dem Verstecken heisst "beim Tippen versteckt".
pub const TIPPEN_US: u64 = 1_000_000;
/// Versteckt trotz Bewegung: so viele eingespeiste Bewegungen ...
pub const BEWEGUNGEN_MIN: u64 = 3;
/// ... ueber mindestens diese Zeit.
pub const VERSTECKT_MIN_US: u64 = 150_000;
/// So lange muss ein Grund stehen, bevor eingefangen gilt ...
pub const FANG_AN_US: u64 = 50_000;
/// ... und so lange der Zeiger wieder sichtbar sein, bevor frei gilt - ein
/// Spiel, das ihn fuer ein Bild zeigt, loest den Fang nicht.
pub const FANG_AUS_US: u64 = 100_000;

/// Das Urteil eines Durchgangs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FangUrteil {
    /// Soll der Client einen Zeiger zeigen (MSG_CURSOR Byte 8)?
    pub sichtbar: bool,
    /// Hat die Anwendung die Maus eingefangen (Byte 10 Bit 0)?
    pub gefangen: bool,
    /// GRUND_* des Fangs (0 = frei).
    pub grund: u8,
}

/// Der Waechter: ein Zustand je Sitzung, `schritt` je Durchgang (Windows:
/// je Runde der Aufnahme, Mac: alle 50 ms in zeiger.m). Einmal eingefangen,
/// bleibt es, bis der Zeiger FANG_AUS_US lang wieder sichtbar ist - im
/// relativen Weg gibt es keine absolute Bewegung mehr, an der "folgt nicht"
/// sich zeigen koennte.
#[derive(Clone, Debug, Default)]
pub struct FangWaechter {
    versteckt_seit: Option<u64>,
    bewegungen_basis: u64,
    taste_vor_verstecken: bool,
    grund_seit: Option<u64>,
    sichtbar_seit: Option<u64>,
    gefangen: bool,
    grund: u8,
}

impl FangWaechter {
    pub fn schritt(&mut self, jetzt: u64, lage: ZeigerLage, ein: EingabeZaehler, art: FangArt) -> FangUrteil {
        if !lage.versteckt {
            self.versteckt_seit = None;
            self.grund_seit = None;
            if self.gefangen {
                let seit = *self.sichtbar_seit.get_or_insert(jetzt);
                if jetzt.saturating_sub(seit) >= FANG_AUS_US {
                    self.gefangen = false;
                    self.grund = 0;
                    self.sichtbar_seit = None;
                }
            }
            return FangUrteil { sichtbar: !self.gefangen, gefangen: self.gefangen, grund: self.grund };
        }
        self.sichtbar_seit = None;
        let seit = match self.versteckt_seit {
            Some(s) => s,
            None => {
                self.versteckt_seit = Some(jetzt);
                self.bewegungen_basis = ein.bewegungen;
                self.taste_vor_verstecken = ein.letzte_taste_us != 0 && jetzt.saturating_sub(ein.letzte_taste_us) < TIPPEN_US;
                jetzt
            }
        };
        let bewegt = ein.bewegungen.saturating_sub(self.bewegungen_basis);
        // Beim Tippen versteckt - ausser (Windows) die Bewegungen seither
        // haetten das laengst beendet.
        let tippen = self.taste_vor_verstecken && !(art.tippen_endet_mit_bewegung && bewegt >= BEWEGUNGEN_MIN);
        let mut grund = 0u8;
        if lage.eingesperrt {
            grund |= GRUND_EINGESPERRT;
        }
        if lage.folgt_nicht {
            grund |= GRUND_FOLGT_NICHT;
        }
        if art.bewegung_regel && lage.vollbild && !tippen && bewegt >= BEWEGUNGEN_MIN && jetzt.saturating_sub(seit) >= VERSTECKT_MIN_US {
            grund |= GRUND_BEWEGUNG;
        }
        if !self.gefangen {
            if grund != 0 {
                let s = *self.grund_seit.get_or_insert(jetzt);
                if jetzt.saturating_sub(s) >= FANG_AN_US {
                    self.gefangen = true;
                    self.grund = grund;
                }
            } else {
                self.grund_seit = None;
            }
        }
        let sichtbar = if self.gefangen {
            false
        } else if art.versteckt_nur_mit_fang {
            true
        } else {
            tippen
        };
        FangUrteil { sichtbar, gefangen: self.gefangen, grund: self.grund }
    }
}

/// Sperrt ClipCursor den Zeiger auf weniger als den virtuellen Bildschirm?
/// Rechtecke wie RECT (links, oben, rechts, unten). Ohne Sperre liefert
/// GetClipCursor den ganzen virtuellen Bildschirm; zwei Punkte Spiel fuer
/// Rundungen.
pub fn eingesperrt(clip: [i32; 4], virtuell: [i32; 4]) -> bool {
    let (cw, ch) = (clip[2] - clip[0], clip[3] - clip[1]);
    let (vw, vh) = (virtuell[2] - virtuell[0], virtuell[3] - virtuell[1]);
    cw >= 0 && ch >= 0 && vw > 0 && vh > 0 && (cw + 2 < vw || ch + 2 < vh)
}

/// Folgt der Zeiger der letzten absoluten Bewegung nicht? `ziel`: wohin sie
/// ihn setzte und wann (us); nur 30 ms bis 2 s danach gilt das Urteil.
pub fn folgt_nicht(jetzt: u64, pos: (i32, i32), ziel: Option<(i32, i32, u64)>, toleranz: i32) -> bool {
    let Some((x, y, t)) = ziel else { return false };
    let alter = jetzt.saturating_sub(t);
    (30_000..=2_000_000).contains(&alter) && ((pos.0 - x).abs() > toleranz || (pos.1 - y).abs() > toleranz)
}

/// Eine Zeile fuers Protokoll des Hosts.
pub fn urteil_text(u: &FangUrteil, versteckt: bool) -> String {
    let mut gruende = Vec::new();
    if u.grund & GRUND_EINGESPERRT != 0 {
        gruende.push("eingesperrt");
    }
    if u.grund & GRUND_FOLGT_NICHT != 0 {
        gruende.push("folgt der Maus nicht");
    }
    if u.grund & GRUND_BEWEGUNG != 0 {
        gruende.push("versteckt trotz Bewegung");
    }
    format!(
        "Zeiger: {}{}, an den Client: {}",
        if versteckt { "versteckt" } else { "sichtbar" },
        if u.gefangen { format!(" - Maus eingefangen ({})", gruende.join(", ")) } else { String::new() },
        if u.sichtbar { "zeigen" } else { "nicht zeigen" }
    )
}

// ------------------------------------------------------- Client: Fangweg

/// Was der Nutzer mit F8 gesagt hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    /// Der Host entscheidet (Merkmal ZEIGER_GEFANGEN).
    Auto,
    /// Eingefangen, auch ohne den Host - bis F8, Menue oder Fokusverlust.
    An,
    /// Frei, obwohl der Host eingefangen meldet - bis sich seine Lage aendert.
    Aus { host_war: bool },
}

/// Die Lage des Clients fuer die Entscheidung.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FangLage {
    /// Der Host kennt den relativen Weg (FAEHIG_MAUS in dieser Sitzung).
    pub host_kann: bool,
    /// Der Host meldet eingefangen.
    pub host_gefangen: bool,
    /// Sitzung mit Bild, Menue und Zugangsdialog zu, Fenster nicht abgelegt.
    pub im_bild: bool,
    /// Das Fenster hat den Fokus.
    pub fokus: bool,
    /// Der Zeiger steht ueber dem Fenster.
    pub maus_im_fenster: bool,
    /// Gerade eingefangen (dann steht der Zeiger ohnehin im Fenster).
    pub aktiv: bool,
    /// Nach einem Fokusverlust faengt erst ein Klick ins Bild (oder F8)
    /// wieder ein - sonst griffe der Fang schon beim Zuruecktabben.
    pub klick_noetig: bool,
}

/// Soll der Client einfangen (Zeiger fest und versteckt, Bewegungen relativ)?
/// Nie ohne einen Host, der es kann: ein aelterer uebergeht IN_MOVE_REL, und
/// die Maus waere tot.
pub fn fang_soll(l: &FangLage, hand: Hand) -> bool {
    if !l.host_kann || !l.im_bild || !l.fokus {
        return false;
    }
    if !l.aktiv && !l.maus_im_fenster {
        return false;
    }
    match hand {
        Hand::An => true,
        Hand::Aus { .. } => false,
        Hand::Auto => l.host_gefangen && !l.klick_noetig,
    }
}

/// F8: eingefangen -> frei (bis der Host seine Lage aendert), frei -> eingefangen.
pub fn hand_umschalten(aktiv: bool, host_gefangen: bool) -> Hand {
    if aktiv {
        Hand::Aus { host_war: host_gefangen }
    } else {
        Hand::An
    }
}

/// Ein "Aus" gilt nur, bis der Host seine Lage aendert - faengt das Spiel
/// danach wieder ein, entscheidet wieder der Host.
pub fn hand_nachfuehren(hand: Hand, host_gefangen: bool) -> Hand {
    match hand {
        Hand::Aus { host_war } if host_war != host_gefangen => Hand::Auto,
        h => h,
    }
}

/// Zeigt der Client ueber dem Bild einen Zeiger? Nie im Fang. Sonst, wie der
/// Host es meldet - nur ein Host mit FAEHIG_MAUS meldet es verlaesslich, ein
/// aelterer bekommt wie bisher immer einen. Meldet der Host "eingefangen",
/// der Client faengt aber (noch) nicht ein (F8 aus, Klick nach Fokusverlust
/// ausstehend), steht der Zeiger da - sonst suchte man ihn.
pub fn zeiger_zeigen(host_kann: bool, sichtbar: bool, host_gefangen: bool, fang: bool) -> bool {
    !fang && (!host_kann || sichtbar || host_gefangen)
}

/// Relative Bewegungen des Clients sammeln (DeviceEvent::MouseMotion kommt
/// mit bis zu 1000 je Sekunde); gesendet wird einmal je Runde der
/// Ereignisschleife und vor jeder Maustaste.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sammler {
    dx: f64,
    dy: f64,
}

impl Sammler {
    pub fn dazu(&mut self, dx: f64, dy: f64) {
        if dx.is_finite() && dy.is_finite() {
            self.dx += dx;
            self.dy += dy;
        }
    }

    /// Das Gesammelte, falls es etwas gibt.
    pub fn nehmen(&mut self) -> Option<(f32, f32)> {
        if self.dx == 0.0 && self.dy == 0.0 {
            return None;
        }
        let d = (self.dx as f32, self.dy as f32);
        *self = Sammler::default();
        Some(d)
    }

    pub fn leeren(&mut self) {
        *self = Sammler::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(w: u16, h: u16) -> Vec<u8> {
        (0..w as usize * h as usize).flat_map(|i| [i as u8, 0, 0, 255]).collect()
    }

    #[test]
    fn zeiger_rundlauf_und_alter_host() {
        let z = ZeigerNachricht { w: 2, h: 2, hx: 1, hy: 0, sichtbar: false, gefangen: true, rgba: form(2, 2) };
        let p = zeiger_kodieren(&z);
        assert_eq!(&p[..12], &[2, 0, 2, 0, 1, 0, 0, 0, 0, 1, 1, 0]);
        assert_eq!(zeiger_lesen(&p), Some(z.clone()));
        // Host bis 0.2.0: Byte 10 ist 0 - nie eingefangen.
        let mut alt = p.clone();
        alt[10] = 0;
        alt[8] = 1;
        let a = zeiger_lesen(&alt).unwrap();
        assert!(a.sichtbar && !a.gefangen);
        // Unfug wird verworfen.
        assert_eq!(zeiger_lesen(&p[..p.len() - 1]), None, "Bild unvollstaendig");
        let mut hot = p.clone();
        hot[4] = 2;
        assert_eq!(zeiger_lesen(&hot), None, "Hotspot ausserhalb");
        assert_eq!(zeiger_lesen(&[0u8; 12]), None, "0x0");
        assert_eq!(zeiger_lesen(&[0u8; 5]), None);
        // Der Platzhalter ist ein gueltiger, durchsichtiger Zeiger.
        let pl = zeiger_lesen(&zeiger_kodieren(&platzhalter(true))).unwrap();
        assert!(!pl.sichtbar && pl.gefangen && pl.rgba == [0, 0, 0, 0]);
    }

    /// Der Massstab folgt dem Bild, geteilt durch die Schnittstelle - nie
    /// doppelt. Die Faelle aus dem Feld.
    #[test]
    fn zeiger_skala_faelle() {
        let nah = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Mac mini an 4K ohne HiDPI (Massstab 1), Vollbild, Strom 1920: 2.
        assert!(nah(zeiger_skala(3840, 1920, 1.0), 2.0));
        // Derselbe Schirm im Fenster 2560 breit: 1,33 - vorher gerundet 1.
        assert!(nah(zeiger_skala(2560, 1920, 1.0), 2560.0 / 1920.0));
        // Retina-MacBook (1440x900 Punkte = 2880x1800), Strom 1920x1080: das
        // Bild ist 2880 breit, 1,5-fach; winit rechnet in Punkten -> 0,75
        // Einheiten je Formpunkt = 1,5 Bildpunkte. Vorher: gerundet 2, dann
        // Retina noch einmal 2 -> 4 Bildpunkte je Formpunkt (2,7-mal zu gross).
        let s = zeiger_skala(2880, 1920, 2.0);
        assert!(nah(s, 0.75));
        assert!(nah(s * 2.0, 2880.0 / 1920.0), "auf dem Schirm genau der Bildmassstab");
        // Retina 4K mit "sieht aus wie 1920x1080": Bild 3840, Einheit 2 -> 1.
        assert!(nah(zeiger_skala(3840, 1920, 2.0), 1.0));
        // Windows-Client 1:1 und bei 150 % Anzeige (Bildpunkte, Einheit 1).
        assert!(nah(zeiger_skala(1920, 1920, 1.0), 1.0));
        assert!(nah(zeiger_skala(2880, 1920, 1.0), 1.5));
        // Unfug faellt auf 1 zurueck bzw. wird begrenzt.
        assert!(nah(zeiger_skala(0, 1920, 1.0), 1.0));
        assert!(nah(zeiger_skala(1920, 1920, f64::NAN), 1.0));
        assert!(nah(zeiger_skala(100_000, 10, 1.0), 16.0));
    }

    #[test]
    fn zeiger_skalieren_ganzzahlig_gemittelt_begrenzt() {
        // 2x ganzzahlig: Punkt fuer Punkt, Hotspot mal 2.
        let q = form(3, 2);
        let (w, h, hx, hy, r) = zeiger_skalieren(3, 2, 2, 1, &q, 2.0);
        assert_eq!((w, h, hx, hy), (6, 4, 4, 2));
        assert_eq!(&r[(3 * 6 + 5) * 4..(3 * 6 + 5) * 4 + 4], &q[(3 + 2) * 4..(3 + 2) * 4 + 4]);
        // 1: unveraendert.
        assert_eq!(zeiger_skalieren(3, 2, 2, 1, &q, 1.0), (3, 2, 2, 1, q.clone()));
        // Halb (4K-Desktop, halbierter Strom): 2x2 gemittelt, deckend bleibt deckend.
        let weiss = vec![255u8; 4 * 4 * 4];
        let (w, h, hx, hy, r) = zeiger_skalieren(4, 4, 3, 1, &weiss, 0.5);
        assert_eq!((w, h, hx, hy), (2, 2, 1, 0));
        assert!(r.iter().all(|&b| b == 255));
        // Durchsichtige Nachbarn faerben nicht ab: halb deckend weiss, keine
        // dunkle Kante (vormultipliziert gemittelt).
        let mut halb = vec![0u8; 2 * 1 * 4];
        halb[0..4].copy_from_slice(&[255, 255, 255, 255]);
        let (_, _, _, _, r) = zeiger_skalieren(2, 1, 0, 0, &halb, 0.5);
        assert_eq!(r, vec![255, 255, 255, 128]);
        // 1,5-fach: 32 -> 48, Hotspot 8,9 -> 12,13.
        let q = form(32, 32);
        let (w, h, hx, hy, r) = zeiger_skalieren(32, 32, 8, 9, &q, 1.5);
        assert_eq!((w, h, hx, hy, r.len()), (48, 48, 12, 13, 48 * 48 * 4));
        // Nie ueber 256: 64 bei Faktor 8 -> 256, Hotspot mit.
        let q = form(64, 32);
        let (w, h, hx, _, _) = zeiger_skalieren(64, 32, 63, 0, &q, 8.0);
        assert_eq!((w, h, hx), (256, 128, 252));
    }

    #[test]
    fn rel_nachricht_und_rest() {
        let p = rel_kodieren(-3.5, 120.25);
        assert_eq!(rel_lesen(&p), Some((-3.5, 120.25)));
        let mut laenger = p.to_vec();
        laenger.extend_from_slice(&[9, 9]);
        assert_eq!(rel_lesen(&laenger), Some((-3.5, 120.25)), "Bytes dahinter bleiben spaeteren Fassungen");
        assert_eq!(rel_lesen(&p[..7]), None);
        assert_eq!(rel_lesen(&rel_kodieren(f32::NAN, 1.0)), None);
        assert_eq!(rel_lesen(&rel_kodieren(1e9, -1e9)), Some((REL_GRENZE, -REL_GRENZE)));
        // Bruchteile gehen nicht verloren, auch nicht rueckwaerts.
        let mut r = RelRest::default();
        let mut summe = (0, 0);
        for _ in 0..10 {
            let (x, y) = r.nehmen(0.3, -0.3);
            summe.0 += x;
            summe.1 += y;
        }
        assert_eq!(summe, (3, -3));
        assert_eq!(r.nehmen(2.0, 0.0), (2, 0));
    }

    #[test]
    fn eingesperrt_und_folgt_nicht() {
        let v = [-928, -2160, 2912, 1080]; // Alienware: 1920x1080 und 4K darueber
        assert!(!eingesperrt(v, v), "keine Sperre: ganzer virtueller Bildschirm");
        assert!(eingesperrt([0, 0, 1920, 1080], v), "Vollbildspiel auf DISPLAY1");
        assert!(eingesperrt([960, 540, 961, 541], v), "Mitte, ein Punkt");
        let einer = [0, 0, 1920, 1080];
        assert!(!eingesperrt(einer, einer), "ein Bildschirm, Spiel sperrt auf ihn: nicht erkennbar");
        assert!(!eingesperrt([0, 0, 1919, 1079], einer), "Rundung");
        // folgt nicht: nur 30 ms bis 2 s nach der Bewegung.
        let ziel = Some((100, 100, 1_000_000));
        assert!(!folgt_nicht(1_010_000, (960, 540), ziel, 3), "zu frueh");
        assert!(folgt_nicht(1_050_000, (960, 540), ziel, 3));
        assert!(!folgt_nicht(1_050_000, (102, 99), ziel, 3), "innerhalb der Toleranz");
        assert!(!folgt_nicht(3_100_000, (960, 540), ziel, 3), "zu alt");
        assert!(!folgt_nicht(1_050_000, (960, 540), None, 3));
    }

    const MS: u64 = 1000;

    /// Ein Durchgang alle 10 ms mit gleicher Lage bis `bis`.
    fn laufen(w: &mut FangWaechter, von: u64, bis: u64, lage: ZeigerLage, ein: &mut EingabeZaehler, bewegt: bool, art: FangArt) -> FangUrteil {
        let mut t = von;
        let mut u = w.schritt(t, lage, *ein, art);
        while t < bis {
            t += 10 * MS;
            if bewegt {
                ein.bewegungen += 1;
            }
            u = w.schritt(t, lage, *ein, art);
        }
        u
    }

    #[test]
    fn waechter_windows() {
        let art = FANG_WINDOWS;
        let v = ZeigerLage { versteckt: true, vollbild: true, ..Default::default() };
        let frei = ZeigerLage::default();
        // Desktop: sichtbar, nie gefangen.
        let mut w = FangWaechter::default();
        let mut ein = EingabeZaehler::default();
        let u = laufen(&mut w, 0, 500 * MS, frei, &mut ein, true, art);
        assert_eq!(u, FangUrteil { sichtbar: true, gefangen: false, grund: 0 });
        // Spiel versteckt den Zeiger (ohne Sperre): sofort "nicht zeigen",
        // gefangen erst, wenn er trotz Bewegung versteckt bleibt.
        let u = w.schritt(510 * MS, v, ein, art);
        assert_eq!(u, FangUrteil { sichtbar: false, gefangen: false, grund: 0 });
        let u = laufen(&mut w, 510 * MS, 600 * MS, v, &mut ein, false, art);
        assert!(!u.gefangen, "ohne Bewegung nicht gefangen");
        let u = laufen(&mut w, 600 * MS, 800 * MS, v, &mut ein, true, art);
        assert!(u.gefangen && !u.sichtbar && u.grund == GRUND_BEWEGUNG, "{u:?}");
        // Bleibt gefangen, auch ohne weitere Bewegung (relativer Weg).
        let u = laufen(&mut w, 800 * MS, 2000 * MS, v, &mut ein, false, art);
        assert!(u.gefangen);
        // Ein Bild lang sichtbar loest nicht, 100 ms schon.
        assert!(w.schritt(2010 * MS, frei, ein, art).gefangen);
        assert!(w.schritt(2060 * MS, frei, ein, art).gefangen);
        let u = w.schritt(2110 * MS, frei, ein, art);
        assert_eq!(u, FangUrteil { sichtbar: true, gefangen: false, grund: 0 });

        // Nicht im Vollbild (Zeichenprogramm mit eigenem Pinselzeiger):
        // versteckt trotz Bewegung faengt nicht, "nicht zeigen" gilt.
        let mut w = FangWaechter::default();
        let fenster = ZeigerLage { versteckt: true, ..Default::default() };
        let u = laufen(&mut w, 0, 2000 * MS, fenster, &mut ein, true, art);
        assert_eq!(u, FangUrteil { sichtbar: false, gefangen: false, grund: 0 });

        // Eingesperrt und versteckt: nach 50 ms gefangen, ohne Bewegung.
        let mut w = FangWaechter::default();
        let gesperrt = ZeigerLage { versteckt: true, eingesperrt: true, folgt_nicht: false, vollbild: false };
        assert!(!w.schritt(0, gesperrt, ein, art).gefangen);
        let u = w.schritt(60 * MS, gesperrt, ein, art);
        assert!(u.gefangen && u.grund == GRUND_EINGESPERRT);
        // Eingesperrt, aber sichtbar (Strategiespiel mit Randscrollen): frei.
        let mut w = FangWaechter::default();
        let u = laufen(&mut w, 0, 500 * MS, ZeigerLage { versteckt: false, eingesperrt: true, folgt_nicht: true, vollbild: true }, &mut ein, true, art);
        assert!(!u.gefangen && u.sichtbar);
        // Spiel setzt den versteckten Zeiger zurueck: folgt nicht.
        let mut w = FangWaechter::default();
        let u = laufen(&mut w, 0, 100 * MS, ZeigerLage { versteckt: true, eingesperrt: false, folgt_nicht: true, vollbild: false }, &mut ein, false, art);
        assert!(u.gefangen && u.grund == GRUND_FOLGT_NICHT);
    }

    /// "Zeiger beim Tippen ausblenden": beim Tippen versteckt heisst nicht
    /// eingefangen, und der Client behaelt seinen Zeiger.
    #[test]
    fn waechter_tippen() {
        let v = ZeigerLage { versteckt: true, vollbild: true, ..Default::default() };
        // Windows: getippt, dann versteckt - Zeiger bleibt beim Client.
        let mut w = FangWaechter::default();
        let mut ein = EingabeZaehler { bewegungen: 10, letzte_taste_us: 1_000 * MS };
        let u = w.schritt(1_200 * MS, v, ein, FANG_WINDOWS);
        assert_eq!(u, FangUrteil { sichtbar: true, gefangen: false, grund: 0 });
        // Bewegt sich die Maus und der Zeiger bleibt trotzdem versteckt,
        // war es nicht das Tippen (Windows zeigt ihn bei Bewegung wieder).
        let u = laufen(&mut w, 1_200 * MS, 1_500 * MS, v, &mut ein, true, FANG_WINDOWS);
        assert!(u.gefangen && !u.sichtbar, "{u:?}");
        // Mac: das Verstecken beim Tippen endet nie durch eingespeiste
        // Bewegung - weder gefangen noch "nicht zeigen".
        let mut w = FangWaechter::default();
        let mut ein = EingabeZaehler { bewegungen: 10, letzte_taste_us: 1_000 * MS };
        let u = laufen(&mut w, 1_200 * MS, 5_000 * MS, v, &mut ein, true, FANG_MAC);
        assert_eq!(u, FangUrteil { sichtbar: true, gefangen: false, grund: 0 });
        // Mac ohne Tippen, versteckt trotz Bewegung: auch nicht (keine
        // Bewegungsregel, der Zeiger bleibt fuer den Client sichtbar) ...
        let mut w = FangWaechter::default();
        let mut ein = EingabeZaehler::default();
        let u = laufen(&mut w, 0, 2_000 * MS, v, &mut ein, true, FANG_MAC);
        assert_eq!(u, FangUrteil { sichtbar: true, gefangen: false, grund: 0 });
        // ... aber abgekoppelt (folgt nicht): gefangen und versteckt.
        let u = laufen(&mut w, 2_000 * MS, 2_100 * MS, ZeigerLage { versteckt: true, folgt_nicht: true, ..Default::default() }, &mut ein, true, FANG_MAC);
        assert_eq!(u, FangUrteil { sichtbar: false, gefangen: true, grund: GRUND_FOLGT_NICHT });
        // Taste lange vor dem Verstecken zaehlt nicht als Tippen.
        let mut w = FangWaechter::default();
        let mut ein = EingabeZaehler { bewegungen: 0, letzte_taste_us: 1 };
        let u = w.schritt(3_000 * MS, v, ein, FANG_WINDOWS);
        assert!(!u.sichtbar);
        let u = laufen(&mut w, 3_000 * MS, 3_300 * MS, v, &mut ein, true, FANG_WINDOWS);
        assert!(u.gefangen);
    }

    #[test]
    fn client_fang_entscheidung() {
        let l = FangLage { host_kann: true, host_gefangen: true, im_bild: true, fokus: true, maus_im_fenster: true, aktiv: false, klick_noetig: false };
        assert!(fang_soll(&l, Hand::Auto));
        // Aelterer Host (kein FAEHIG_MAUS): nie, auch nicht mit F8.
        assert!(!fang_soll(&FangLage { host_kann: false, ..l }, Hand::An));
        // Menue, Fokus weg, kein Bild: nie.
        assert!(!fang_soll(&FangLage { im_bild: false, ..l }, Hand::An));
        assert!(!fang_soll(&FangLage { fokus: false, ..l }, Hand::Auto));
        // Maus ausserhalb: erst beim Eintritt - ausser schon eingefangen.
        assert!(!fang_soll(&FangLage { maus_im_fenster: false, ..l }, Hand::Auto));
        assert!(fang_soll(&FangLage { maus_im_fenster: false, aktiv: true, ..l }, Hand::Auto));
        // Nach Fokusverlust erst ein Klick - F8 geht immer.
        assert!(!fang_soll(&FangLage { klick_noetig: true, ..l }, Hand::Auto));
        assert!(fang_soll(&FangLage { klick_noetig: true, ..l }, Hand::An));
        // Host frei: nur mit F8.
        let frei = FangLage { host_gefangen: false, ..l };
        assert!(!fang_soll(&frei, Hand::Auto));
        assert!(fang_soll(&frei, Hand::An));
        // F8 im Fang: frei, bis der Host seine Lage aendert.
        let h = hand_umschalten(true, true);
        assert!(!fang_soll(&l, h));
        assert_eq!(hand_nachfuehren(h, true), h);
        assert_eq!(hand_nachfuehren(h, false), Hand::Auto, "Spiel gibt frei: wieder Automatik");
        assert_eq!(hand_umschalten(false, false), Hand::An);
    }

    #[test]
    fn client_zeiger_zeigen() {
        // Aelterer Host: immer (sein "versteckt" ist beim Mac das Tippen).
        assert!(zeiger_zeigen(false, false, false, false));
        // Neuer Host: wie gemeldet; im Fang nie.
        assert!(zeiger_zeigen(true, true, false, false));
        assert!(!zeiger_zeigen(true, false, false, false), "Spiel versteckt: kein doppelter Zeiger");
        assert!(!zeiger_zeigen(true, true, false, true));
        assert!(zeiger_zeigen(true, false, true, false), "gefangen gemeldet, aber nicht eingefangen: Zeiger da");
    }

    #[test]
    fn sammler() {
        let mut s = Sammler::default();
        assert_eq!(s.nehmen(), None);
        s.dazu(1.5, -2.0);
        // Ein Paar mit NaN faellt ganz weg.
        s.dazu(0.5, f64::NAN);
        s.dazu(1.0, 1.0);
        assert_eq!(s.nehmen(), Some((2.5, -1.0)));
        assert_eq!(s.nehmen(), None);
    }
}
