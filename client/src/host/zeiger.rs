// Zeigerform des Windows-Hosts: aus der Desktop Duplication (FrameInfo.
// PointerShapeBufferSize > 0 -> GetFramePointerShape) nach RGBA mit
// GERADER Deckkraft, Nachricht 49 wie zeiger.m auf dem Mac: Hash (FNV)
// ueber die ganze Nachricht, nur bei Aenderung senden, hoechstens 20 je
// Sekunde (die Wartekugel ist animiert), Einpassen ueber 256 Punkte,
// Massstab 1. Ein neu verbundener Zuschauer bekommt die Form erzwungen.
//
// Die Form geht in STROMPUNKTEN hinaus (ein Formpunkt = ein Punkt des
// Stroms, so zeichnet jeder Client sie): wird ein 4K-Desktop halbiert
// gestreamt, wird auch der Zeiger halbiert (maus::zeiger_skalieren) - vorher
// kam ein 64er-Zeiger eines 200-%-Desktops doppelt so gross wie das Bild an.
//
// Die Duplication liefert den Desktop OHNE Zeiger - die Projektregel "kein
// Zeiger im Video" gilt von selbst. Sichtbar und eingefangen entscheidet der
// Fangwaechter (maus.rs) aus GetCursorInfo (CURSOR_SHOWING, Form), GetClipCursor
// und GetCursorPos, gefragt je Runde der Aufnahme - auch waehrend die
// Duplication verloren ist (Vollbild-exklusiv, Modewechsel). Vorher kam die
// Sichtbarkeit nur aus FrameInfo.PointerPosition.Visible, und nur mit einer
// Zeigerbewegung (LastMouseUpdateTime != 0); nach einem Verlust der Aufnahme
// blieb der alte Stand stehen. Die Duplication bleibt der Rueckfall, wenn
// GetCursorInfo scheitert (sicherer Desktop). Hat die Duplication noch keine
// Form geliefert (Zeiger schon beim Start versteckt, Aufnahme verloren), baut
// der Host sie aus dem Zeiger von GetCursorInfo (GetIconInfo, GetDIBits). Ist
// dann noch immer keine da, geht fuer "versteckt" ein durchsichtiger
// Platzhalter hinaus (ein neuer Client zeigt fuer ihn den eigenen Pfeil).
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

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, MonitorFromWindow, ReleaseDC, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClipCursor, GetCursorInfo, GetForegroundWindow, GetIconInfo, GetSystemMetrics, GetWindowRect, CURSORINFO,
    CURSOR_SHOWING, HCURSOR, HICON, ICONINFO, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::{eingabe, log, netz};
use crate::maus::{self, FangUrteil, FangWaechter, ZeigerLage, ZeigerNachricht};
use crate::protokoll_konst::MSG_CURSOR;

/// Groesser schickt der Host keinen Zeiger; was groesser ist, wird eingepasst.
const MAX: u32 = 256;
/// Hoechstens so oft geht eine Form raus.
const MINDESTABSTAND: Duration = Duration::from_millis(50);
/// So oft fragt der Fangwaechter das System (GetCursorInfo und Co.).
const LAGE_TAKT: Duration = Duration::from_millis(8);
/// So viele Protokollzeilen zu Sichtbarkeit und Fang je Sitzung.
const LAGE_ZEILEN: u32 = 40;
/// Toleranz fuer "folgt nicht" in Bildpunkten (Rundung der 0..65535-Lage).
const FOLGT_TOLERANZ: i32 = 3;

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
    /// Die Form, wie die Duplication sie lieferte (Punkte des Desktops) ...
    roh: Option<Form>,
    /// ... und in Strompunkten (siehe massstab_setzen).
    form: Option<Form>,
    /// Strompunkte je Desktoppunkt: 1, halbiert 0,5.
    massstab: f64,
    /// `roh` stammt aus GetCursorInfo, nicht aus der Duplication - dann folgt
    /// sie dem Zeiger des Systems, bis die Duplication eine liefert.
    roh_aus_system: bool,
    /// Zu diesem Zeiger (HCURSOR) wurde zuletzt eine Form gebaut oder versucht.
    system_zeiger: usize,
    system_gemeldet: bool,
    /// Zuletzt ging der Platzhalter hinaus (keine Form).
    platzhalter_gesendet: bool,
    /// Sichtbarkeit laut Duplication - nur der Rueckfall.
    dd_sichtbar: bool,
    waechter: FangWaechter,
    urteil: FangUrteil,
    naechste_lage: Instant,
    lage_fehler_gemeldet: bool,
    lage_zeilen: u32,
    hash: u64,
    letzte_sendung: Instant,
    gross_gemeldet: bool,
    pub xor_gemeldet: bool,
}

impl Zeiger {
    pub fn neu() -> Zeiger {
        Zeiger {
            roh: None,
            form: None,
            massstab: 1.0,
            roh_aus_system: false,
            system_zeiger: 0,
            system_gemeldet: false,
            platzhalter_gesendet: false,
            dd_sichtbar: true,
            waechter: FangWaechter::default(),
            urteil: FangUrteil { sichtbar: true, gefangen: false, grund: 0 },
            naechste_lage: Instant::now(),
            lage_fehler_gemeldet: false,
            lage_zeilen: 0,
            hash: 0,
            letzte_sendung: Instant::now() - Duration::from_secs(1),
            gross_gemeldet: false,
            xor_gemeldet: false,
        }
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
            self.roh = Some(f);
            self.roh_aus_system = false;
            self.form_in_strompunkten();
        }
    }

    /// Strompunkte je Desktoppunkt (Stromplan: halbiert ab 3840 Breite). Die
    /// Form folgt sofort.
    pub fn massstab_setzen(&mut self, massstab: f64) {
        let m = if massstab.is_finite() && massstab > 0.0 { massstab } else { 1.0 };
        if m != self.massstab {
            self.massstab = m;
            self.form_in_strompunkten();
        }
    }

    fn form_in_strompunkten(&mut self) {
        self.form = self.roh.as_ref().map(|f| {
            if self.massstab == 1.0 {
                Form { w: f.w, h: f.h, hx: f.hx, hy: f.hy, rgba: f.rgba.clone() }
            } else {
                let (w, h, hx, hy, rgba) = maus::zeiger_skalieren(f.w, f.h, f.hx, f.hy, &f.rgba, self.massstab);
                Form { w, h, hx, hy, rgba }
            }
        });
    }

    /// Sichtbarkeit laut Duplication (FrameInfo.PointerPosition.Visible) -
    /// gilt nur, wenn GetCursorInfo nichts sagt.
    pub fn sichtbar_setzen(&mut self, sichtbar: bool) {
        self.dd_sichtbar = sichtbar;
    }

    /// Den Fangwaechter mit der Lage des Systems fuettern (hoechstens alle
    /// LAGE_TAKT) und jeden Wechsel ins Protokoll schreiben.
    fn lage_pruefen(&mut self) {
        if Instant::now() < self.naechste_lage {
            return;
        }
        self.naechste_lage = Instant::now() + LAGE_TAKT;
        let jetzt = super::now_us();
        let (ein, ziel) = eingabe::zaehler();
        let lage = match lage_lesen(jetzt, ziel) {
            Some((l, zeiger)) => {
                // Ohne Form aus der Duplication: die des Systems, solange
                // der Zeiger sichtbar ist und sich die Form aendert.
                if (self.roh.is_none() || self.roh_aus_system) && !l.versteckt && zeiger != 0 && zeiger != self.system_zeiger {
                    self.system_zeiger = zeiger;
                    if let Some(f) = form_aus_system(HCURSOR(zeiger as *mut _)) {
                        if !self.system_gemeldet {
                            self.system_gemeldet = true;
                            log(format!("Zeigerform: aus GetCursorInfo ({}x{}, Hotspot {},{}) - die Duplication lieferte noch keine", f.w, f.h, f.hx, f.hy));
                        }
                        self.roh = Some(f);
                        self.roh_aus_system = true;
                        self.form_in_strompunkten();
                    }
                }
                l
            }
            None => {
                if !self.lage_fehler_gemeldet {
                    self.lage_fehler_gemeldet = true;
                    log("Zeiger: GetCursorInfo scheitert (sicherer Desktop?) - Sichtbarkeit aus der Duplication");
                }
                ZeigerLage { versteckt: !self.dd_sichtbar, eingesperrt: false, folgt_nicht: false, vollbild: false }
            }
        };
        let urteil = self.waechter.schritt(jetzt, lage, ein, maus::FANG_WINDOWS);
        // Jeder Wechsel, den der Client sieht, ins Protokoll.
        if urteil != self.urteil {
            self.lage_zeilen += 1;
            if self.lage_zeilen <= LAGE_ZEILEN {
                log(maus::urteil_text(&urteil, lage.versteckt));
            } else if self.lage_zeilen == LAGE_ZEILEN + 1 {
                log("Zeiger: weitere Wechsel ohne Protokollzeile");
            }
            self.urteil = urteil;
        }
    }

    /// Bei Aenderung (oder erzwungen) senden, hoechstens 20 je Sekunde.
    pub fn pruefen(&mut self) {
        self.lage_pruefen();
        let n = match self.form.as_ref() {
            Some(f) => ZeigerNachricht {
                w: f.w,
                h: f.h,
                hx: f.hx,
                hy: f.hy,
                sichtbar: self.urteil.sichtbar,
                gefangen: self.urteil.gefangen,
                rgba: f.rgba.clone(),
            },
            // Noch keine Form: versteckt (oder eingefangen) heisst der
            // Platzhalter, sichtbar bleibt der Pfeil des Clients - stand
            // vorher der Platzhalter, sagt ihn ein sichtbarer zurueck.
            None if !self.urteil.sichtbar || self.urteil.gefangen || self.platzhalter_gesendet => {
                ZeigerNachricht { sichtbar: self.urteil.sichtbar, ..maus::platzhalter(self.urteil.gefangen) }
            }
            None => return,
        };
        let platzhalter = self.form.is_none();
        let p = maus::zeiger_kodieren(&n);
        let h = fnv(&p, 1469598103934665603);
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
        self.platzhalter_gesendet = platzhalter;
        netz::send_small(MSG_CURSOR, &p);
    }
}

/// Die Form eines Zeigers des Systems (GetCursorInfo) als RGBA - fuer die
/// Zeit, bevor die Duplication eine liefert. GetIconInfo gibt Maske und
/// (bei Farbzeigern) Farbe als Bitmaps; GetDIBits liest beide als 32 Bit
/// von oben nach unten, form_aus_bitmaps macht daraus RGBA.
fn form_aus_system(zeiger: HCURSOR) -> Option<Form> {
    let mut ii = ICONINFO::default();
    // SAFETY: gueltiger Zeiger auf ICONINFO; die Bitmaps gehoeren danach uns.
    unsafe { GetIconInfo(HICON(zeiger.0), &mut ii) }.ok()?;
    let lesen = |bm: HBITMAP| -> Option<(usize, usize, Vec<u8>)> {
        if bm.is_invalid() {
            return None;
        }
        let mut b = BITMAP::default();
        // SAFETY: BITMAP ist so gross wie angegeben.
        let n = unsafe { GetObjectW(HGDIOBJ(bm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut _)) };
        let (w, h) = (b.bmWidth.max(0) as usize, b.bmHeight.unsigned_abs() as usize);
        if n == 0 || w == 0 || h == 0 || w > 1024 || h > 2048 {
            return None;
        }
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; w * h * 4];
        // SAFETY: Bildschirm-DC nur fuer GetDIBits, der Puffer fasst w*h*4.
        let zeilen = unsafe {
            let dc: HDC = GetDC(None);
            let z = GetDIBits(dc, bm, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
            ReleaseDC(None, dc);
            z
        };
        (zeilen == h as i32).then_some((w, h, px))
    };
    let maske = lesen(ii.hbmMask);
    let farbe = lesen(ii.hbmColor);
    // SAFETY: GetIconInfo legte die Bitmaps fuer uns an.
    unsafe {
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
    }
    let (mw, mh, mpx) = maske?;
    let f = match farbe {
        Some((w, h, fpx)) if w == mw && mh >= h => form_aus_bitmaps(w, h, Some(&fpx), &mpx, ii.xHotspot, ii.yHotspot),
        Some(_) => None,
        None => form_aus_bitmaps(mw, mh / 2, None, &mpx, ii.xHotspot, ii.yHotspot),
    }?;
    // Wie bei der Duplication: hoechstens MAX je Seite.
    if f.w as u32 > MAX || f.h as u32 > MAX {
        let (w, h, hx, hy, rgba) = maus::zeiger_skalieren(f.w, f.h, f.hx, f.hy, &f.rgba, MAX as f64 / f.w.max(f.h) as f64);
        return Some(Form { w, h, hx, hy, rgba });
    }
    Some(f)
}

/// Maske und Farbe eines Zeigers (BGRA, 32 Bit, oben zuerst) -> RGBA mit
/// gerader Deckkraft. Farbzeiger: traegt die Farbe Deckkraft, gilt sie; sonst
/// sagt die Maske, was durchsichtig ist (UND = 1), und Punkte mit UND = 1 und
/// Farbe invertieren - deckend genaehert wie bei der Duplication. Ohne Farbe
/// (monochrom) ist die Maske doppelt hoch: oben UND, unten XOR.
pub fn form_aus_bitmaps(w: usize, h: usize, farbe: Option<&[u8]>, maske: &[u8], hx: u32, hy: u32) -> Option<Form> {
    if w == 0 || h == 0 || w > u16::MAX as usize || h > u16::MAX as usize {
        return None;
    }
    let und = |x: usize, y: usize| maske.get((y * w + x) * 4).map(|&b| b != 0).unwrap_or(true);
    let mut out = vec![0u8; w * h * 4];
    match farbe {
        Some(f) => {
            if f.len() < w * h * 4 {
                return None;
            }
            let mit_alpha = f.chunks_exact(4).take(w * h).any(|p| p[3] != 0);
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    let (b, g, r, a) = (f[i], f[i + 1], f[i + 2], f[i + 3]);
                    let a = if mit_alpha {
                        a
                    } else if !und(x, y) || (r | g | b) != 0 {
                        255
                    } else {
                        0
                    };
                    if a != 0 {
                        out[i..i + 4].copy_from_slice(&[r, g, b, a]);
                    }
                }
            }
        }
        None => {
            if maske.len() < w * h * 2 * 4 {
                return None;
            }
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    let xor = maske[((y + h) * w + x) * 4] != 0;
                    match (und(x, y), xor) {
                        (false, false) => out[i..i + 4].copy_from_slice(&[0, 0, 0, 255]),
                        (false, true) => out[i..i + 4].copy_from_slice(&[255, 255, 255, 255]),
                        (true, false) => {}
                        (true, true) => out[i..i + 4].copy_from_slice(&[0, 0, 0, 255]),
                    }
                }
            }
        }
    }
    Some(Form { w: w as u16, h: h as u16, hx: (hx as usize).min(w - 1) as u16, hy: (hy as usize).min(h - 1) as u16, rgba: out })
}

/// Was das System ueber den Zeiger sagt: versteckt (kein CURSOR_SHOWING oder
/// keine Form - SetCursor(NULL)), eingesperrt (ClipCursor kleiner als der
/// virtuelle Bildschirm), folgt nicht (steht nicht, wo die letzte absolute
/// Bewegung ihn hinsetzte), Vollbild (das Fenster im Vordergrund deckt seinen
/// Bildschirm). Dazu der Zeiger (HCURSOR) selbst. None, wenn GetCursorInfo
/// scheitert.
fn lage_lesen(jetzt: u64, ziel: Option<(i32, i32, u64)>) -> Option<(ZeigerLage, usize)> {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    // SAFETY: ci ist gross genug und traegt cbSize.
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    let versteckt = ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid();
    let mut clip = RECT::default();
    // SAFETY: gueltiger Zeiger auf ein RECT.
    let eingesperrt = unsafe { GetClipCursor(&mut clip) }.is_ok() && maus::eingesperrt([clip.left, clip.top, clip.right, clip.bottom], virtueller_bildschirm());
    let folgt_nicht = maus::folgt_nicht(jetzt, (ci.ptScreenPos.x, ci.ptScreenPos.y), ziel, FOLGT_TOLERANZ);
    // Vollbild fragt nur die Bewegungsregel - also nur bei verstecktem Zeiger.
    let vollbild = versteckt && vordergrund_vollbild();
    Some((ZeigerLage { versteckt, eingesperrt, folgt_nicht, vollbild }, ci.hCursor.0 as usize))
}

/// Deckt das Fenster im Vordergrund seinen Bildschirm ganz (Vollbild, auch
/// randlos)? Ein maximiertes Fenster laesst die Taskleiste frei - nein.
fn vordergrund_vollbild() -> bool {
    // SAFETY: reine Abfragen mit gueltigen Zeigern auf eigene Strukturen.
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            return false;
        }
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return false;
        }
        let b = mi.rcMonitor;
        r.left <= b.left && r.top <= b.top && r.right >= b.right && r.bottom >= b.bottom
    }
}

/// Der virtuelle Bildschirm als RECT (links, oben, rechts, unten).
fn virtueller_bildschirm() -> [i32; 4] {
    // SAFETY: reine Abfragen.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    [x, y, x + w, y + h]
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

    /// Halbierter Strom (4K-Desktop): die Form geht in Strompunkten hinaus,
    /// also halb so gross - auch wenn der Massstab erst nach der Form kommt.
    #[test]
    fn form_in_strompunkten() {
        let info = DXGI_OUTDUPL_POINTER_SHAPE_INFO { Type: 2, Width: 64, Height: 64, Pitch: 256, HotSpot: POINT { x: 10, y: 21 } };
        let mut z = Zeiger::neu();
        z.form_setzen(&info, &vec![255u8; 64 * 256]);
        assert_eq!(z.form.as_ref().map(|f| (f.w, f.h, f.hx, f.hy)), Some((64, 64, 10, 21)));
        z.massstab_setzen(0.5);
        assert_eq!(z.form.as_ref().map(|f| (f.w, f.h, f.hx, f.hy)), Some((32, 32, 5, 10)));
        assert!(z.form.as_ref().unwrap().rgba.iter().all(|&b| b == 255));
        z.massstab_setzen(1.0);
        assert_eq!(z.form.as_ref().map(|f| (f.w, f.h)), Some((64, 64)));
        z.massstab_setzen(f64::NAN);
        assert_eq!(z.form.as_ref().map(|f| (f.w, f.h)), Some((64, 64)));
    }

    /// Zeiger des Systems: Farbe mit Deckkraft, Farbe ohne Deckkraft (die
    /// Maske entscheidet, Invertieren deckend) und monochrom (Maske doppelt
    /// hoch) - wie die Formen der Duplication.
    #[test]
    fn form_aus_system_bitmaps() {
        // 2x1 Farbe mit Deckkraft: gilt so.
        let farbe = [10, 20, 30, 255, 0, 0, 0, 0];
        let maske = [0u8; 8];
        let f = form_aus_bitmaps(2, 1, Some(&farbe), &maske, 5, 0).unwrap();
        assert_eq!((f.w, f.h, f.hx, f.hy), (2, 1, 1, 0), "Hotspot im Bild");
        assert_eq!(f.rgba, vec![30, 20, 10, 255, 0, 0, 0, 0]);
        // Ohne Deckkraft: UND = 0 deckend, UND = 1 und schwarz durchsichtig,
        // UND = 1 mit Farbe (invertieren) deckend.
        let farbe = [10, 20, 30, 0, 0, 0, 0, 0, 255, 255, 255, 0];
        let maske = [0, 0, 0, 0, 255, 255, 255, 0, 255, 255, 255, 0];
        let f = form_aus_bitmaps(3, 1, Some(&farbe), &maske, 0, 0).unwrap();
        assert_eq!(f.rgba, vec![30, 20, 10, 255, 0, 0, 0, 0, 255, 255, 255, 255]);
        // Monochrom 2x1: Maske 2x2 (UND oben, XOR unten).
        let maske = [0, 0, 0, 0, 255, 255, 255, 0, /* XOR */ 255, 255, 255, 0, 0, 0, 0, 0];
        let f = form_aus_bitmaps(2, 1, None, &maske, 0, 0).unwrap();
        assert_eq!(f.rgba, vec![255, 255, 255, 255, 0, 0, 0, 0]);
        // Zu kurz: keine Form.
        assert!(form_aus_bitmaps(2, 2, None, &[0; 8], 0, 0).is_none());
    }

    /// GetCursorInfo auf dieser Maschine: eine Lage kommt (ausser auf dem
    /// sicheren Desktop), und ohne ClipCursor ist nichts eingesperrt.
    #[test]
    fn lage_des_systems() {
        if let Some((l, _)) = lage_lesen(super::super::now_us(), None) {
            assert!(!l.folgt_nicht, "ohne absolute Bewegung kein 'folgt nicht'");
            assert!(!l.eingesperrt, "niemand sperrt den Zeiger ein");
            let v = virtueller_bildschirm();
            assert!(v[2] > v[0] && v[3] > v[1], "{v:?}");
        }
    }
}
