// Zeigerform des Windows-Hosts: aus der Desktop Duplication (FrameInfo.
// PointerShapeBufferSize > 0 -> GetFramePointerShape) nach RGBA mit
// GERADER Deckkraft, Nachricht 49 wie zeiger.m auf dem Mac: Hash (FNV)
// ueber Form, Massstab und Merkmale, nur bei Aenderung senden, hoechstens 20
// je Sekunde (die Wartekugel ist animiert), Einpassen ueber 256 Punkte. Ein
// neu verbundener Zuschauer bekommt die Form erzwungen.
//
// Die Duplication liefert den Desktop OHNE Zeiger - die Projektregel "kein
// Zeiger im Video" gilt von selbst. Sichtbar und eingefangen entscheidet der
// Fangwaechter (maus.rs) aus GetCursorInfo (CURSOR_SHOWING, Form), GetClipCursor
// und GetCursorPos, gefragt je Runde der Aufnahme - auch waehrend die
// Duplication verloren ist (Vollbild-exklusiv, Modewechsel). Vorher kam die
// Sichtbarkeit nur aus FrameInfo.PointerPosition.Visible, und nur mit einer
// Zeigerbewegung (LastMouseUpdateTime != 0); nach einem Verlust der Aufnahme
// blieb der alte Stand stehen. Die Duplication bleibt der Rueckfall, wenn
// GetCursorInfo scheitert (sicherer Desktop). "Versteckt" glaubt der Host
// nur, wenn eine Anwendung es gewesen sein kann (maus::VersteckGlaube): mit
// Maus (SM_MOUSEPRESENT), nachdem der Zeiger in dieser Sitzung einmal
// sichtbar war, und nicht mit dem Desktop oder der Taskleiste vorn - ein
// Rechner ohne Maus meldet den Zeiger immer versteckt. Hat die Duplication
// noch keine Form geliefert (Zeiger schon beim Start versteckt, Aufnahme
// verloren), baut der Host sie aus dem Zeiger von GetCursorInfo (GetIconInfo,
// GetDIBits). Ist dann noch immer keine da, geht fuer "versteckt" ein
// durchsichtiger Platzhalter hinaus (ein neuer Client zeigt fuer ihn den
// eigenen Pfeil).
//
// Massstab: die Form der Duplication hat Desktoppunkte, und die sind
// Bildpunkte des Stroms - der Strom ist nativ. Nur wenn er fuer H.264
// eingepasst ist (aufnahme::stromplan_codec, etwa 5120 -> 4096 breit), wird
// die Form beim Senden im selben Mass verkleinert (zeigerbild.rs,
// flaechengemittelt): der Client erwartet sie in Bildpunkten des Stroms.
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
    GetClassNameW, GetClipCursor, GetCursorInfo, GetForegroundWindow, GetIconInfo, GetSystemMetrics, GetWindowRect,
    CURSORINFO, CURSOR_SHOWING, HCURSOR, HICON, ICONINFO, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_MOUSEPRESENT,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::{eingabe, log, netz};
use crate::maus::{self, FangUrteil, FangWaechter, SystemZeiger, VersteckGlaube, ZeigerNachricht};
use crate::protokoll_konst::MSG_CURSOR;
use crate::zeigerbild::{self, Massstab};

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

/// Stand der Zeigerform auf dem Aufnahmefaden, einer je Sitzung.
pub struct Zeiger {
    /// Die Form in Desktoppunkten, wie die Duplication sie liefert - oder,
    /// solange die keine geliefert hat, aus GetCursorInfo.
    form: Option<Form>,
    /// Bildpunkte des Stroms je Desktoppunkt (1, eingepasst kleiner).
    massstab: Massstab,
    /// `form` stammt aus GetCursorInfo, nicht aus der Duplication - dann
    /// folgt sie dem Zeiger des Systems, bis die Duplication eine liefert.
    form_aus_system: bool,
    /// Zu diesem Zeiger (HCURSOR) wurde zuletzt eine Form gebaut oder versucht.
    system_zeiger: usize,
    system_gemeldet: bool,
    /// Zuletzt ging der Platzhalter hinaus (keine Form).
    platzhalter_gesendet: bool,
    /// Sichtbarkeit laut Duplication - nur der Rueckfall.
    dd_sichtbar: bool,
    /// Glaubt der Host "versteckt"? (maus::VersteckGlaube: nicht ohne Maus,
    /// nicht vor dem ersten sichtbaren Zeiger, nie mit dem Desktop vorn.)
    glaube: VersteckGlaube,
    /// Welche Zweifel schon im Protokoll stehen (Zweifel::bit).
    zweifel_gemeldet: u8,
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
            form: None,
            massstab: Massstab::EINS,
            form_aus_system: false,
            system_zeiger: 0,
            system_gemeldet: false,
            platzhalter_gesendet: false,
            dd_sichtbar: true,
            glaube: VersteckGlaube::default(),
            zweifel_gemeldet: 0,
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

    /// Der Massstab fuer einen Strom, der `strom_w` Bildpunkte breit ist, von
    /// einem Desktop, der `desktop_w` breit ist (wie er steht): nativ 1, fuer
    /// H.264 eingepasst kleiner. Eine Aenderung schickt die Form neu, mit
    /// einer Zeile.
    pub fn massstab_setzen(&mut self, strom_w: i32, desktop_w: u32) {
        let m = if strom_w > 0 && desktop_w > 0 { Massstab::aus_faktor(strom_w as f64 / desktop_w as f64, None) } else { Massstab::EINS };
        if m != self.massstab {
            self.massstab = m;
            log(format!("Zeigerform: Massstab {:.2} (Strom {strom_w} Bildpunkte breit, Desktop {desktop_w})", m.faktor()));
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
            self.form = Some(f);
            self.form_aus_system = false;
        }
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
        let system = match system_lesen(jetzt, ziel) {
            Some((s, zeiger)) => {
                // Ohne Form aus der Duplication: die des Systems, solange
                // der Zeiger sichtbar ist und sich die Form aendert.
                if (self.form.is_none() || self.form_aus_system) && s.zeigt && zeiger != 0 && zeiger != self.system_zeiger {
                    self.system_zeiger = zeiger;
                    if let Some(f) = form_aus_system(HCURSOR(zeiger as *mut _)) {
                        if !self.system_gemeldet {
                            self.system_gemeldet = true;
                            log(format!("Zeigerform: aus GetCursorInfo ({}x{}, Hotspot {},{}) - die Duplication lieferte noch keine", f.w, f.h, f.hx, f.hy));
                        }
                        self.form = Some(f);
                        self.form_aus_system = true;
                    }
                }
                s
            }
            None => {
                if !self.lage_fehler_gemeldet {
                    self.lage_fehler_gemeldet = true;
                    log("Zeiger: GetCursorInfo scheitert (sicherer Desktop?) - Sichtbarkeit aus der Duplication");
                }
                SystemZeiger { zeigt: self.dd_sichtbar, maus_da: maus_angeschlossen(), ..Default::default() }
            }
        };
        let (lage, zweifel) = self.glaube.lage(system);
        if let Some(z) = zweifel {
            if self.zweifel_gemeldet & z.bit() == 0 {
                self.zweifel_gemeldet |= z.bit();
                log(z.text());
            }
        }
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

    /// Bei Aenderung (oder erzwungen) senden, hoechstens 20 je Sekunde - im
    /// Massstab des Stroms. Die Kennung kommt aus der ungeskalierten Form,
    /// dem Massstab und den Merkmalen; skaliert wird nur, was hinausgeht.
    pub fn pruefen(&mut self) {
        self.lage_pruefen();
        let (sichtbar, gefangen) = (self.urteil.sichtbar, self.urteil.gefangen);
        // Noch keine Form: versteckt (oder eingefangen) heisst der
        // Platzhalter, sichtbar bleibt der Pfeil des Clients - stand vorher
        // der Platzhalter, sagt ihn ein sichtbarer zurueck.
        let platzhalter = match self.form {
            Some(_) => false,
            None if !sichtbar || gefangen || self.platzhalter_gesendet => true,
            None => return,
        };
        let mut h = 1469598103934665603;
        if let Some(f) = self.form.as_ref() {
            h = fnv(&f.rgba, h);
            h = fnv(&[f.w.to_le_bytes(), f.h.to_le_bytes(), f.hx.to_le_bytes(), f.hy.to_le_bytes()].concat(), h);
        }
        h = fnv(&[sichtbar as u8, gefangen as u8, platzhalter as u8], h);
        h = fnv(&self.massstab.kennung().to_le_bytes(), h);
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
        let p = match self.form.as_ref() {
            Some(f) => nachricht(f, self.massstab, sichtbar, gefangen),
            None => maus::zeiger_kodieren(&ZeigerNachricht { sichtbar, ..maus::platzhalter(gefangen) }),
        };
        netz::send_small(MSG_CURSOR, &p);
    }

    /// Der geltende Massstab (fuer die Pruefung).
    #[cfg(test)]
    fn massstab(&self) -> Massstab {
        self.massstab
    }
}

/// Nachricht 49 fuer eine Form (Desktoppunkte) im Massstab `m` des Stroms:
/// Kopf wie maus::zeiger_kodieren (sichtbar, Massstab 1, Merkmal
/// "eingefangen"), dann RGBA - in Bildpunkten des Stroms, wie der Client sie
/// erwartet (zeigerbild.rs).
fn nachricht(f: &Form, m: Massstab, sichtbar: bool, gefangen: bool) -> Vec<u8> {
    let (rgba, w, h, hx, hy) = if m == Massstab::EINS {
        (f.rgba.clone(), f.w, f.h, f.hx, f.hy)
    } else {
        zeigerbild::skalieren(&f.rgba, f.w, f.h, f.hx, f.hy, m)
    };
    maus::zeiger_kodieren(&ZeigerNachricht { w, h, hx, hy, sichtbar, gefangen, rgba })
}

/// Hoechstens MAX je Seite, wie bei der Duplication: flaechengemittelt
/// verkleinert (zeigerbild.rs), abgerundet, damit keine Seite ueber MAX kommt.
fn einpassen(f: Form) -> Form {
    let seite = f.w.max(f.h) as u32;
    if seite <= MAX {
        return f;
    }
    let (rgba, w, h, hx, hy) = zeigerbild::skalieren(&f.rgba, f.w, f.h, f.hx, f.hy, Massstab::Promille((MAX * 1000 / seite).max(1)));
    Form { w, h, hx, hy, rgba }
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
    Some(einpassen(f))
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

/// Was das System ueber den Zeiger sagt, roh (maus::SystemZeiger): zeigt es
/// einen (CURSOR_SHOWING und eine Form - SetCursor(NULL) versteckt auch),
/// ist eine Maus angeschlossen, eingesperrt (ClipCursor kleiner als der
/// virtuelle Bildschirm), folgt nicht (steht nicht, wo die letzte absolute
/// Bewegung ihn hinsetzte) und - nur ohne Zeiger - was vorn liegt: Vollbild
/// (das Fenster im Vordergrund deckt seinen Bildschirm) oder die Oberflaeche
/// selbst (Desktop, Taskleiste). Dazu der Zeiger (HCURSOR). None, wenn
/// GetCursorInfo scheitert.
fn system_lesen(jetzt: u64, ziel: Option<(i32, i32, u64)>) -> Option<(SystemZeiger, usize)> {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    // SAFETY: ci ist gross genug und traegt cbSize.
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    let zeigt = ci.flags.0 & CURSOR_SHOWING.0 != 0 && !ci.hCursor.is_invalid();
    let mut clip = RECT::default();
    // SAFETY: gueltiger Zeiger auf ein RECT.
    let eingesperrt = unsafe { GetClipCursor(&mut clip) }.is_ok() && maus::eingesperrt([clip.left, clip.top, clip.right, clip.bottom], virtueller_bildschirm());
    let folgt_nicht = maus::folgt_nicht(jetzt, (ci.ptScreenPos.x, ci.ptScreenPos.y), ziel, FOLGT_TOLERANZ);
    let maus_da = maus_angeschlossen();
    // Was vorn liegt, fragen nur die Regeln fuer einen versteckten Zeiger,
    // der geglaubt werden kann (mit Maus).
    let (vollbild, oberflaeche_vorn) = if zeigt || !maus_da { (false, false) } else { vordergrund() };
    let s = SystemZeiger { zeigt, maus_da, oberflaeche_vorn, eingesperrt, folgt_nicht, vollbild };
    Some((s, ci.hCursor.0 as usize))
}

/// Ist eine Maus angeschlossen? Ohne (SM_MOUSEPRESENT 0) steht der Zaehler
/// von ShowCursor auf -1, und Windows zeigt nie einen Zeiger.
fn maus_angeschlossen() -> bool {
    // SAFETY: reine Abfrage.
    unsafe { GetSystemMetrics(SM_MOUSEPRESENT) != 0 }
}

/// Was vorn liegt: deckt das Fenster im Vordergrund seinen Bildschirm ganz
/// (Vollbild, auch randlos - ein maximiertes Fenster laesst die Taskleiste
/// frei: nein), und ist es die Oberflaeche selbst (Desktop Progman/WorkerW,
/// Taskleiste - maus::oberflaeche_klasse)? Der Desktop deckt den ganzen
/// Bildschirm, zaehlt aber nie als Vollbild.
fn vordergrund() -> (bool, bool) {
    // SAFETY: reine Abfragen mit gueltigen Zeigern auf eigene Strukturen.
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return (false, false);
        }
        let mut name = [0u16; 64];
        let n = GetClassNameW(h, &mut name).max(0) as usize;
        let oberflaeche = maus::oberflaeche_klasse(&String::from_utf16_lossy(&name[..n.min(name.len())]));
        if oberflaeche {
            return (false, true);
        }
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            return (false, false);
        }
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return (false, false);
        }
        let b = mi.rcMonitor;
        (r.left <= b.left && r.top <= b.top && r.right >= b.right && r.bottom >= b.bottom, false)
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

    /// Nativ geht die Form unveraendert hinaus; fuer H.264 eingepasst (5120
    /// -> 4096 breit) im selben Mass verkleinert, Hotspot mit - in
    /// Bildpunkten des Stroms, wie der Client sie erwartet. Das Merkmal
    /// "eingefangen" steht in Byte 10, auch skaliert; eine Form, die vor dem
    /// Massstab kam, geht im neuen Massstab hinaus.
    #[test]
    fn form_im_massstab_des_stroms() {
        let f = Form { w: 48, h: 48, hx: 47, hy: 0, rgba: vec![255u8; 48 * 48 * 4] };
        let p = nachricht(&f, Massstab::EINS, true, false);
        assert_eq!((&p[0..8], p[8], p[9], p[10], p.len()), (&[48u8, 0, 48, 0, 47, 0, 0, 0][..], 1, 1, 0, 12 + 48 * 48 * 4));
        let mut z = Zeiger::neu();
        z.massstab_setzen(3840, 3840);
        assert_eq!(z.massstab(), Massstab::EINS);
        z.massstab_setzen(3838, 3839);
        assert_eq!(z.massstab(), Massstab::EINS, "ungerader Rand");
        z.massstab_setzen(4096, 5120);
        assert_eq!(z.massstab(), Massstab::Promille(800));
        let p = nachricht(&f, z.massstab(), false, true);
        let (w, h, hx) = (u16::from_le_bytes([p[0], p[1]]), u16::from_le_bytes([p[2], p[3]]), u16::from_le_bytes([p[4], p[5]]));
        assert_eq!((w, h, hx, p[8], p[10], p.len()), (38, 38, 37, 0, crate::protokoll_konst::ZEIGER_GEFANGEN, 12 + 38 * 38 * 4));
        assert!(p[12..].iter().all(|&b| b == 255));
        let n = maus::zeiger_lesen(&p).unwrap();
        assert!(n.gefangen && !n.sichtbar && (n.w, n.h) == (38, 38), "der Client liest, was der Host schreibt");
        // Form aus der Duplication (Desktoppunkte), der Massstab danach.
        let info = DXGI_OUTDUPL_POINTER_SHAPE_INFO { Type: 2, Width: 64, Height: 64, Pitch: 256, HotSpot: POINT { x: 10, y: 21 } };
        let mut z = Zeiger::neu();
        z.form_setzen(&info, &vec![255u8; 64 * 256]);
        assert_eq!(z.form.as_ref().map(|f| (f.w, f.h, f.hx, f.hy)), Some((64, 64, 10, 21)), "die Form bleibt in Desktoppunkten");
        z.massstab_setzen(4096, 5120);
        let p = nachricht(z.form.as_ref().unwrap(), z.massstab(), true, false);
        assert_eq!(&p[0..8], &[51u8, 0, 51, 0, 8, 0, 17, 0]);
    }

    /// Zeiger des Systems: Farbe mit Deckkraft, Farbe ohne Deckkraft (die
    /// Maske entscheidet, Invertieren deckend) und monochrom (Maske doppelt
    /// hoch) - wie die Formen der Duplication. Zu gross wird eingepasst,
    /// keine Seite ueber MAX.
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
        // Einpassen: 512 -> 256, und eine Hoehe, bei der gerundet 257 herauskaeme.
        for (w, h) in [(512u16, 512u16), (300, 1500), (256, 256), (1024, 2048)] {
            let f = einpassen(Form { w, h, hx: w - 1, hy: 0, rgba: vec![255u8; w as usize * h as usize * 4] });
            assert!(f.w.max(f.h) as u32 <= MAX && f.w.max(f.h) as u32 >= MAX - 2, "{w}x{h} -> {}x{}", f.w, f.h);
            assert!(f.hx < f.w && f.rgba.len() == f.w as usize * f.h as usize * 4);
        }
    }

    /// GetCursorInfo auf dieser Maschine: eine Lage kommt (ausser auf dem
    /// sicheren Desktop), ohne ClipCursor ist nichts eingesperrt, und was
    /// vorn liegt, wird nur ohne Zeiger gefragt.
    #[test]
    fn lage_des_systems() {
        if let Some((s, _)) = system_lesen(super::super::now_us(), None) {
            assert!(!s.folgt_nicht, "ohne absolute Bewegung kein 'folgt nicht'");
            assert!(!s.eingesperrt, "niemand sperrt den Zeiger ein");
            assert!(!s.zeigt || (!s.vollbild && !s.oberflaeche_vorn), "{s:?}");
            let v = virtueller_bildschirm();
            assert!(v[2] > v[0] && v[3] > v[1], "{v:?}");
        }
    }
}
