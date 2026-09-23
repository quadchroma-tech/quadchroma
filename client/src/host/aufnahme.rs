// Aufnahme des Windows-Hosts: Ausgangsliste (DXGI), Ausgangswahl, die
// Desktop Duplication und der Aufnahmefaden im Betrieb - Bilder abholen,
// Schrittmacher, Encoder, Wiederherstellung nach Verlust, Zeigerform.
//
// Quelle der Liste: IDXGIFactory1::EnumAdapters1 -> IDXGIAdapter1::EnumOutputs
// -> IDXGIOutput::GetDesc (DeviceName, DesktopCoordinates, AttachedToDesktop).
// Auf einem Laptop ohne MUX haengen ALLE Ausgaenge an der integrierten Karte;
// die Liste zeigt das sofort. Ohne DXGI-Ausgang (WARP, RDP) bleibt fuer die
// Maus die GDI-Liste (EnumDisplayMonitors).
//
// Der Aufnahmefaden ist die Rolle von g_capq auf dem Mac: alles, was den
// Encoder anfasst (Bilder, Codecwechsel, Einstellungen, Testbild, Takt),
// laeuft hier nacheinander - deshalb braucht nichts davon eine Sperre.
// Kein Zuschauer, keine Arbeit: ohne Zuschauer gibt es keine Duplication,
// keinen Encoder und kein Wachhalten; der Faden wartet.
//
// Ausfall: DXGI_ERROR_ACCESS_LOST (Modewechsel, UAC-Bildschirm, Vollbild-
// exklusiv) -> Duplication abbauen, Nachricht 9 = 1, alle 2 s neu versuchen,
// bei Erfolg 9 = 0 und Vollbild erzwingen. Gemerkt wird der Geraetename des
// Ausgangs, nicht der Listenplatz: faellt er weg, wird ausgewichen, kommt er
// zurueck, gilt er wieder.
//
// Gedrehte Ausgaenge (Hochformat, 180 Grad, hochkantes Panel, das Windows
// quer betreibt): die Oberflaeche kommt ungedreht, gedreht wird beim
// Einlesen auf dem Prozessor (Drehung, bgra_drehen); der Strom hat die
// Groesse des Desktops, die Maus bleibt beim Desktop (DesktopCoordinates).

use std::ffi::c_void;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};

use super::encoder::{self, Betrieb, Bild, Quelle, Weg};
use super::takt::{Schrittmacher, INFLIGHT_AUFNAHME, INFLIGHT_TAKT};
use super::zeiger::Zeiger;
use super::{log, netz, Z};
use std::sync::atomic::Ordering;

#[derive(Clone, Debug)]
pub struct Ausgang {
    /// Platz in der Liste (--output n).
    pub index: usize,
    /// Geraetename (\\.\DISPLAYn) - das, was sich der Host merkt.
    pub name: String,
    pub links: i32,
    pub oben: i32,
    pub breite: i32,
    pub hoehe: i32,
    /// Adapter, an dem der Ausgang haengt (Index der DXGI-Aufzaehlung).
    pub karte: usize,
    pub karte_name: String,
    pub nvidia: bool,
    pub haupt: bool,
}

fn utf16_text(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

fn fehler(was: &str, e: windows::core::Error) -> String {
    format!("{was}: {} (0x{:08x})", e.message().trim(), e.code().0 as u32)
}

/// Alle Ausgaenge aller Adapter. Software-Adapter (WARP) haben keine.
pub fn ausgaenge() -> Result<Vec<Ausgang>, String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| fehler("CreateDXGIFactory1", e))?;
    let mut liste = Vec::new();
    let mut i = 0u32;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        let d = unsafe { a.GetDesc1() }.map_err(|e| fehler("GetDesc1", e))?;
        let karte_name = utf16_text(&d.Description);
        let mut j = 0u32;
        while let Ok(o) = unsafe { a.EnumOutputs(j) } {
            if let Ok(od) = unsafe { o.GetDesc() } {
                if od.AttachedToDesktop.as_bool() {
                    let r = od.DesktopCoordinates;
                    liste.push(Ausgang {
                        index: liste.len(),
                        name: utf16_text(&od.DeviceName),
                        links: r.left,
                        oben: r.top,
                        breite: r.right - r.left,
                        hoehe: r.bottom - r.top,
                        karte: i as usize,
                        karte_name: karte_name.clone(),
                        nvidia: d.VendorId == 0x10de,
                        haupt: r.left == 0 && r.top == 0,
                    });
                }
            }
            j += 1;
        }
        i += 1;
    }
    Ok(liste)
}

/// Karten und Ausgaenge ins Protokoll, Ausgaenge in `out`.
pub fn ausgaenge_melden(out: &mut Vec<Ausgang>) {
    // Die Kartenzeilen des Clients (Rolle, Speicher, Ausgang, UMA).
    for k in crate::karten() {
        log(format!(
            "Karte {}: {} - {}, {} MB, LUID {:x}, Ausgang {}",
            k.index, k.name, k.rolle.name(), k.speicher_mb, k.luid,
            if k.hat_ausgang { "ja" } else { "nein" }
        ));
    }
    for z in crate::protokoll::abholen() {
        log(z);
    }
    match ausgaenge() {
        Ok(liste) => {
            if liste.is_empty() {
                log("Ausgaenge: keine (Software-Adapter/WARP oder RDP-Sitzung) - Duplication nicht moeglich");
            }
            for a in &liste {
                log(format!(
                    "Ausgang {}: {} {}x{} bei ({},{})  an Karte {} ({}){}",
                    a.index, a.name, a.breite, a.hoehe, a.links, a.oben, a.karte, a.karte_name,
                    if a.haupt { "  (Hauptbildschirm)" } else { "" }
                ));
            }
            *out = liste;
        }
        Err(e) => log(format!("Ausgaenge: {e}")),
    }
}

/// --output n = Platz in der Liste; ohne Angabe der Hauptbildschirm, sonst
/// der erste.
pub fn ausgang_waehlen(liste: &[Ausgang], wunsch: Option<usize>) -> Option<Ausgang> {
    if let Some(n) = wunsch {
        match liste.get(n) {
            Some(a) => return Some(a.clone()),
            None => log(format!("Ausgang {n} nicht verfuegbar ({} in der Liste) - nehme den Hauptbildschirm", liste.len())),
        }
    }
    liste.iter().find(|a| a.haupt).or_else(|| liste.first()).cloned()
}

/// Groesse des Stroms aus der Bildgroesse (dw x dh): ab 3840 Breite wird
/// halbiert (wie beim Mac), immer gerade - ein ungerader Rand wird
/// abgeschnitten, nicht skaliert. Liefert Breite, Hoehe und ob halbiert
/// wird; die Aufnahme folgt diesem Plan, sie raet nie aus Abweichungen.
pub fn stromplan(dw: i32, dh: i32) -> (i32, i32, bool) {
    let halb = dw >= 3840;
    let (w, h) = if halb { (dw / 2, dh / 2) } else { (dw, dh) };
    (w & !1, h & !1, halb)
}

/// Vorlaeufige Stromgroesse fuer einen Ausgang (beim Start, vor der
/// Duplication); danach gilt der Anzeigemodus der Duplication.
pub fn stromgroesse(a: &Ausgang) -> (i32, i32) {
    let (w, h, _) = stromplan(a.breite, a.hoehe);
    (w, h)
}

/// w*4 Byte je Zeile aus einer Quelle mit Zeilenabstand `abstand` holen,
/// dicht gepackt nach `ziel`. Reichen Abstand oder Quelle nicht, kommt ein
/// Fehler statt eines Zugriffs hinter das Ende.
pub fn zeilen_holen(quelle: &[u8], abstand: usize, w: usize, h: usize, ziel: &mut Vec<u8>) -> Result<(), String> {
    let zeile = w * 4;
    if w == 0 || h == 0 || zeile > abstand || quelle.len() < abstand * (h - 1) + zeile {
        return Err(format!("Auslesen {w}x{h}: Quelle {} Byte bei Zeilenabstand {abstand} reicht nicht", quelle.len()));
    }
    ziel.resize(zeile * h, 0);
    for (y, z) in ziel.chunks_exact_mut(zeile).enumerate() {
        z.copy_from_slice(&quelle[y * abstand..y * abstand + zeile]);
    }
    Ok(())
}

// ---------------------------------------------------------------- Drehung

/// Drehung eines Ausgangs (DXGI_OUTDUPL_DESC::Rotation). Die Oberflaeche aus
/// AcquireNextFrame liegt immer ungedreht vor, der Desktop gedreht darin
/// (Microsoft, "Desktop Duplication API", Abschnitt "Rotating the desktop
/// image": Desktop 768x1024 bei 90 Grad -> Oberflaeche 1024x768). Die
/// Richtung steht im Beispiel DXGIDesktopDuplication (DisplayManager.cpp,
/// SetDirtyVert und SetMoveRect): bei ROTATE90 kommt der Desktoppunkt
/// (x, y) aus der Oberflaeche bei (y, H-1-x) - die Oberflaeche wird fuer den
/// Desktop um 90 Grad im Uhrzeigersinn gedreht, bei ROTATE270 gegen ihn.
/// WebRTC (dxgi_output_duplicator.cc) dreht ebenso (CLOCK_WISE_90 ->
/// libyuv kRotate90).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drehung {
    Keine,
    Grad90,
    Grad180,
    Grad270,
}

/// Zielzeilen je Streifen beim Drehen um 90/270 Grad: die Quelle wird
/// zeilenweise gelesen, geschrieben wird in so viele Zeilen zugleich.
const DREH_STREIFEN: usize = 16;

impl Drehung {
    pub fn aus_dxgi(r: DXGI_MODE_ROTATION) -> Drehung {
        match r {
            DXGI_MODE_ROTATION_ROTATE90 => Drehung::Grad90,
            DXGI_MODE_ROTATION_ROTATE180 => Drehung::Grad180,
            DXGI_MODE_ROTATION_ROTATE270 => Drehung::Grad270,
            _ => Drehung::Keine,
        }
    }

    pub fn grad(self) -> u32 {
        match self {
            Drehung::Keine => 0,
            Drehung::Grad90 => 90,
            Drehung::Grad180 => 180,
            Drehung::Grad270 => 270,
        }
    }

    /// Hoch- und Querformat vertauscht (90/270 Grad)?
    pub fn vertauscht(self) -> bool {
        matches!(self, Drehung::Grad90 | Drehung::Grad270)
    }

    /// Groesse des Desktops aus der Groesse der Oberflaeche (und umgekehrt).
    pub fn groesse<T>(self, w: T, h: T) -> (T, T) {
        if self.vertauscht() { (h, w) } else { (w, h) }
    }

    /// Woher der Desktoppunkt (x, y) in der Oberflaeche sw x sh kommt.
    fn quelle(self, sw: usize, sh: usize, x: usize, y: usize) -> (usize, usize) {
        match self {
            Drehung::Keine => (x, y),
            Drehung::Grad90 => (y, sh - 1 - x),
            Drehung::Grad180 => (sw - 1 - x, sh - 1 - y),
            Drehung::Grad270 => (sw - 1 - y, x),
        }
    }
}

/// Groesse der Oberflaeche aus AcquireNextFrame (so gross werden die eigenen
/// Texturen). Microsofts Tabelle nennt den Modus, "wie GDI oder DXGI ihn
/// liefern", gedreht (768x1024 bei 90 Grad), und WebRTC prueft ModeDesc
/// gegen DesktopCoordinates - ModeDesc waere dann bei 90/270 Grad zu
/// vertauschen. Verlassen wird sich darauf nicht: steht ModeDesc schon im
/// anderen Format (hoch/quer) als der Desktop, ist es die Oberflaeche
/// selbst. Kommt sie doch anders an, faengt oberflaeche_pruefen das ab.
pub fn oberflaeche_groesse(d: Drehung, modus_w: u32, modus_h: u32, desktop_w: i32, desktop_h: i32) -> (u32, u32) {
    if d.vertauscht() && (modus_w >= modus_h) == (desktop_w >= desktop_h) {
        (modus_h, modus_w)
    } else {
        (modus_w, modus_h)
    }
}

/// BGRA (dicht gepackt, sw x sh, ungedreht) so drehen, wie der Desktop
/// steht: das Ziel hat die Groesse d.groesse(sw, sh), dicht gepackt.
/// Zeilenparallel; bei 90/270 Grad in Streifen, damit die Quelle zeilenweise
/// gelesen wird statt spaltenweise. Reicht die Quelle nicht, ein Fehler.
pub fn bgra_drehen(src: &[u8], sw: usize, sh: usize, d: Drehung, ziel: &mut Vec<u8>) -> Result<(), String> {
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return Err(format!("Drehen {sw}x{sh}: Quelle {} Byte reicht nicht", src.len()));
    }
    let (gw, gh) = d.groesse(sw, sh);
    ziel.resize(gw * gh * 4, 0);
    let pixel = |z: &mut [u8], o: usize, x: usize, y: usize| {
        let (qx, qy) = d.quelle(sw, sh, x, y);
        let i = (qy * sw + qx) * 4;
        z[o..o + 4].copy_from_slice(&src[i..i + 4]);
    };
    match d {
        Drehung::Keine => ziel.copy_from_slice(&src[..sw * sh * 4]),
        Drehung::Grad180 => ziel.par_chunks_mut(gw * 4).enumerate().for_each(|(y, z)| {
            for x in 0..gw {
                pixel(z, x * 4, x, y);
            }
        }),
        Drehung::Grad90 | Drehung::Grad270 => ziel.par_chunks_mut(gw * 4 * DREH_STREIFEN).enumerate().for_each(|(s, z)| {
            let y0 = s * DREH_STREIFEN;
            let zeilen = z.len() / (gw * 4);
            for x in 0..gw {
                for dy in 0..zeilen {
                    pixel(z, (dy * gw + x) * 4, x, y0 + dy);
                }
            }
        }),
    }
    Ok(())
}

// ------------------------------------------------------------- GDI-Rueckfall

unsafe extern "system" fn monitor_cb(m: HMONITOR, _dc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
    let liste = &mut *(lp.0 as *mut Vec<(String, RECT, bool)>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        liste.push((utf16_text(&info.szDevice), info.monitorInfo.rcMonitor, (info.monitorInfo.dwFlags & 1) != 0));
    }
    BOOL(1)
}

/// Bildschirme laut GDI: (Geraetename, Rechteck, Hauptbildschirm).
pub fn gdi_monitore() -> Vec<(String, RECT, bool)> {
    let mut liste: Vec<(String, RECT, bool)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_cb), LPARAM(&mut liste as *mut _ as isize));
    }
    liste
}

/// Geometrie des Hauptbildschirms laut GDI (links, oben, Breite, Hoehe).
pub fn gdi_hauptbildschirm() -> Option<(i32, i32, i32, i32)> {
    let liste = gdi_monitore();
    let m = liste.iter().find(|m| m.2).or_else(|| liste.first())?;
    let r = m.1;
    log(format!("GDI-Bildschirm: {} {}x{} bei ({},{})", m.0, r.right - r.left, r.bottom - r.top, r.left, r.top));
    Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
}

// ----------------------------------------------------------- Duplication

/// Geraet auf dem Adapter des Ausgangs plus die Duplication darauf.
pub struct Duplication {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub dup: IDXGIOutputDuplication,
    pub adapter: IDXGIAdapter1,
    /// Groesse der Oberflaeche aus AcquireNextFrame (ungedreht) - so gross
    /// sind die eigenen Texturen. Den Desktop nennt desktop_groesse.
    pub breite: u32,
    pub hoehe: u32,
    pub drehung: Drehung,
    pub format: DXGI_FORMAT,
    pub im_systemspeicher: bool,
    /// Noch kein Bild abgeholt: das erste Abholen liefert den ganzen
    /// Desktop, auch wenn seit dem Aufbau nichts praesentiert wurde
    /// (LastPresentTime 0) - sonst bliebe ein stiller Desktop minutenlang
    /// ohne erstes Bild.
    erstes_offen: std::cell::Cell<bool>,
}

/// Klartext zu den DXGI-Fehlern, die hier vorkommen.
pub fn dxgi_fehler_text(e: &windows::core::Error) -> String {
    let c = e.code();
    if c == DXGI_ERROR_UNSUPPORTED {
        "Duplication auf diesem Ausgang nicht moeglich (RDP/kein Bildschirm/WARP)".into()
    } else if c == DXGI_ERROR_ACCESS_LOST {
        "Zugriff verloren (Modewechsel, UAC-Bildschirm oder Vollbild-exklusiv)".into()
    } else if c == DXGI_ERROR_ACCESS_DENIED {
        "Zugriff verweigert (geschuetzter Inhalt oder andere Sitzung)".into()
    } else if c == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE {
        "zur Zeit nicht verfuegbar (zu viele Duplications auf diesem Ausgang)".into()
    } else if c == DXGI_ERROR_WAIT_TIMEOUT {
        "Zeitablauf".into()
    } else {
        format!("{} (0x{:08x})", e.message().trim(), c.0 as u32)
    }
}

/// Duplication auf dem Ausgang mit diesem Listenplatz aufbauen: Geraet auf
/// seinem Adapter (D3D_DRIVER_TYPE_UNKNOWN mit dem IDXGIAdapter),
/// ID3D11Multithread an (Aufnahme- und Encoderfaden teilen das Geraet),
/// IDXGIOutput1::DuplicateOutput. HDR-Ausgaenge (R16G16B16A16_FLOAT) und
/// alles ausser BGRA 8 Bit werden vorerst abgelehnt.
pub fn duplication_aufbauen(ausgang: &Ausgang) -> Result<Duplication, String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| fehler("CreateDXGIFactory1", e))?;
    let adapter = unsafe { factory.EnumAdapters1(ausgang.karte as u32) }.map_err(|e| fehler("EnumAdapters1", e))?;
    let mut output: Option<IDXGIOutput> = None;
    let mut j = 0u32;
    while let Ok(o) = unsafe { adapter.EnumOutputs(j) } {
        if let Ok(d) = unsafe { o.GetDesc() } {
            if utf16_text(&d.DeviceName) == ausgang.name {
                output = Some(o);
                break;
            }
        }
        j += 1;
    }
    let output = output.ok_or_else(|| format!("Ausgang {} nicht mehr da", ausgang.name))?;
    let (device, ctx, _fl) = crate::anzeige::geraet_bauen(Some(&adapter), false, false)?;
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    let output1: IDXGIOutput1 = output.cast().map_err(|e| fehler("IDXGIOutput1", e))?;
    let dup = unsafe { output1.DuplicateOutput(&device) }.map_err(|e| format!("DuplicateOutput: {}", dxgi_fehler_text(&e)))?;
    let desc: DXGI_OUTDUPL_DESC = unsafe { dup.GetDesc() };
    let format = desc.ModeDesc.Format;
    if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
        return Err("HDR-Ausgang (R16G16B16A16_FLOAT) - vorerst nicht unterstuetzt".into());
    }
    if format != DXGI_FORMAT_B8G8R8A8_UNORM && format != DXGI_FORMAT_B8G8R8A8_UNORM_SRGB {
        return Err(format!("Desktopformat {} - vorerst nur BGRA 8 Bit", format.0));
    }
    // Gedreht (Hochformat, 180 Grad, auch ein hochkantes Panel, das Windows
    // quer betreibt): die Oberflaeche aus AcquireNextFrame liegt ungedreht
    // vor, der Desktop (DesktopCoordinates) gedreht. Die eigenen Texturen
    // haben die Groesse der Oberflaeche, gedreht wird beim Einlesen.
    let drehung = Drehung::aus_dxgi(desc.Rotation);
    let (breite, hoehe) = oberflaeche_groesse(drehung, desc.ModeDesc.Width, desc.ModeDesc.Height, ausgang.breite, ausgang.hoehe);
    Ok(Duplication {
        device,
        ctx,
        dup,
        adapter,
        breite,
        hoehe,
        drehung,
        format,
        im_systemspeicher: desc.DesktopImageInSystemMemory.as_bool(),
        erstes_offen: std::cell::Cell::new(true),
    })
}

/// Was die Duplication zum Zeiger sagt (gueltig bis ReleaseFrame).
#[derive(Clone, Copy)]
pub struct ZeigerInfo {
    /// Groesse einer neuen Zeigerform (0 = unveraendert).
    pub form_bytes: u32,
    /// LastMouseUpdateTime != 0: die Sichtbarkeit gilt.
    pub maus_aktualisiert: bool,
    pub sichtbar: bool,
}

/// Ein abgeholtes Bild. Nach `NurZeiger` und `Bild` muss der Aufrufer
/// `freigeben` rufen - vorher darf er die Zeigerform abholen.
pub enum Abholung {
    /// Innerhalb der Frist kam nichts.
    Nichts,
    /// Nur der Zeiger hat sich bewegt oder seine Form geaendert (LastPresentTime == 0).
    NurZeiger(ZeigerInfo),
    /// Ein neues Bild: Textur der Duplication (bis ReleaseFrame gueltig),
    /// Praesentationszeit (QPC) und die Zeigerangaben.
    Bild { textur: ID3D11Texture2D, praesentiert_qpc: i64, zeiger: ZeigerInfo },
}

impl Duplication {
    /// Groesse des Desktops (gedreht wie er steht) - daraus der Strom.
    pub fn desktop_groesse(&self) -> (u32, u32) {
        self.drehung.groesse(self.breite, self.hoehe)
    }

    /// AcquireNextFrame mit Frist in ms. Der Aufrufer muss nach `Bild`
    /// SOFORT kopieren und `freigeben` rufen.
    pub fn abholen(&self, frist_ms: u32) -> Result<Abholung, windows::core::Error> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match unsafe { self.dup.AcquireNextFrame(frist_ms, &mut info, &mut res) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Abholung::Nichts),
            Err(e) => return Err(e),
        }
        let zeiger = ZeigerInfo {
            form_bytes: info.PointerShapeBufferSize,
            maus_aktualisiert: info.LastMouseUpdateTime != 0,
            sichtbar: info.PointerPosition.Visible.as_bool(),
        };
        if info.LastPresentTime == 0 && !self.erstes_offen.get() {
            return Ok(Abholung::NurZeiger(zeiger));
        }
        let textur: ID3D11Texture2D = match res.and_then(|r| r.cast().ok()) {
            Some(t) => t,
            None => return Ok(Abholung::NurZeiger(zeiger)),
        };
        self.erstes_offen.set(false);
        let praesentiert_qpc = if info.LastPresentTime != 0 {
            info.LastPresentTime
        } else {
            let mut q = 0i64;
            unsafe {
                let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut q);
            }
            q
        };
        Ok(Abholung::Bild { textur, praesentiert_qpc, zeiger })
    }

    pub fn freigeben(&self) {
        unsafe {
            let _ = self.dup.ReleaseFrame();
        }
    }

    /// Die neue Zeigerform (nur solange das Bild nicht freigegeben ist).
    pub fn zeigerform(&self, bytes: u32) -> Result<(DXGI_OUTDUPL_POINTER_SHAPE_INFO, Vec<u8>), windows::core::Error> {
        let mut daten = vec![0u8; bytes as usize];
        let mut noetig = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        unsafe { self.dup.GetFramePointerShape(bytes, daten.as_mut_ptr() as *mut c_void, &mut noetig, &mut info) }?;
        daten.truncate(noetig as usize);
        Ok((info, daten))
    }

    /// Eigene Textur in Bildgroesse (DEFAULT) oder als STAGING zum Lesen.
    pub fn textur(&self, w: u32, h: u32, staging: bool) -> Result<ID3D11Texture2D, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: self.format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
            BindFlags: if staging { 0 } else { D3D11_BIND_SHADER_RESOURCE.0 as u32 },
            CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }.map_err(|e| fehler("CreateTexture2D", e))?;
        tex.ok_or_else(|| "CreateTexture2D lieferte nichts".into())
    }

    /// STAGING-Textur in den Hauptspeicher lesen (BGRA, w*4 Byte je Zeile):
    /// die linke obere Ecke w x h - kleiner als die Textur schneidet ab,
    /// groesser ist ein Fehler.
    pub fn auslesen(&self, staging: &ID3D11Texture2D, w: u32, h: u32, ziel: &mut Vec<u8>) -> Result<(), String> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { staging.GetDesc(&mut d) };
        if w > d.Width || h > d.Height {
            return Err(format!("Auslesen {w}x{h} aus einer Textur {}x{}", d.Width, d.Height));
        }
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m)) }.map_err(|e| fehler("Map Staging", e))?;
        let r = if m.pData.is_null() {
            Err("Map Staging: kein Zeiger".to_string())
        } else {
            // Sicher gemappt: Hoehe-1 Zeilen zu RowPitch, dann die letzte Zeile.
            let laenge = m.RowPitch as usize * (d.Height as usize).saturating_sub(1) + d.Width as usize * 4;
            let quelle = unsafe { std::slice::from_raw_parts(m.pData as *const u8, laenge) };
            zeilen_holen(quelle, m.RowPitch as usize, w as usize, h as usize, ziel)
        };
        unsafe { self.ctx.Unmap(staging, 0) };
        r
    }
}

/// Roher Zeiger eines Geraets, mit einer zusaetzlichen Referenz - fuer
/// FFmpegs AVD3D11VADeviceContext, der sie beim Freigeben abgibt.
pub fn geraet_roh(device: &ID3D11Device) -> *mut c_void {
    let kopie = device.clone();
    let p = kopie.as_raw();
    std::mem::forget(kopie);
    p
}

// --------------------------------------------------------------- Betrieb

/// Der Aufnahmefaden. `wunsch` ist der beim Start gewaehlte Ausgang (sein
/// Geraetename zaehlt), `weg` der Eingabeweg des Encoders.
pub fn start(wunsch: Option<Ausgang>, weg: Weg) {
    std::thread::Builder::new()
        .name("quadchroma-aufnahme".into())
        .spawn(move || loop {
            while !netz::zuschauer_da() {
                std::thread::sleep(Duration::from_millis(100));
            }
            // Der naechste Zuschauer ist einer mit anderer Nummer - auch wenn
            // er den jetzigen ohne Luecke abloest und zuschauer_da() dabei
            // nie false wird. Gemerkt vor der Sitzung: wer schon waehrend
            // der abgestuerzten Sitzung abgeloest hat, bekommt gleich einen
            // neuen Anlauf.
            let nr = netz::zuschauer_nr();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sitzung(wunsch.as_ref(), weg)));
            if r.is_err() {
                // Wachhalten und Timerperiode hat Drop schon abgebaut.
                log("Aufnahme: Faden abgestuerzt - neuer Anlauf mit dem naechsten Zuschauer");
                // Dem Zuschauer sagen, dass kein Bild kommt - statt eines
                // stummen, stehenden Bildes.
                netz::hoststatus_senden(1);
                while netz::zuschauer_da() && netz::zuschauer_nr() == nr {
                    std::thread::sleep(Duration::from_millis(500));
                }
                // Ein Testbild ueberlebt den Zuschauer auch hier nicht.
                if Z.testbild.swap(false, Ordering::Relaxed) {
                    log("Testbild aus (Zuschauer weg)");
                }
            }
        })
        .ok();
}

/// Den gewuenschten Ausgang in der aktuellen Liste finden; ist er weg, wird
/// ausgewichen (Hauptbildschirm, sonst der erste), mit Vermerk.
fn ausgang_finden(wunsch: Option<&Ausgang>, ausgewichen: &mut bool) -> Option<Ausgang> {
    let liste = ausgaenge().unwrap_or_default();
    if let Some(w) = wunsch {
        if let Some(a) = liste.iter().find(|a| a.name == w.name) {
            if *ausgewichen {
                *ausgewichen = false;
                log(format!("Ausgang {} ist wieder da - gilt wieder", w.name));
            }
            return Some(a.clone());
        }
    }
    let ersatz = liste.iter().find(|a| a.haupt).or_else(|| liste.first()).cloned();
    if let (Some(w), Some(e)) = (wunsch, ersatz.as_ref()) {
        if !*ausgewichen {
            *ausgewichen = true;
            log(format!("Ausgang {} nicht da - weiche auf {} aus", w.name, e.name));
        }
    }
    ersatz
}

/// Bild und Maus auf denselben Ausgang; die Stromgroesse kommt aus dem
/// Anzeigemodus der Duplication (dw x dh) - das ist die Groesse der Bilder,
/// die wirklich ankommen - und steht danach in Z. Liefert den Plan (Breite,
/// Hoehe, halbiert) und ob sich die Stromgroesse geaendert hat.
fn strom_anpassen(a: &Ausgang, dw: i32, dh: i32) -> ((i32, i32, bool), bool) {
    let (w, h, halb) = stromplan(dw, dh);
    super::eingabe::ausgang_setzen(a.links, a.oben, a.breite, a.hoehe);
    let alt = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);
    Z.info_w.store(w as u32, Ordering::Relaxed);
    Z.info_h.store(h as u32, Ordering::Relaxed);
    ((w, h, halb), alt != (w, h))
}

/// Ist die Oberflaeche aus AcquireNextFrame so gross wie der Modus (und
/// damit wie die eigenen Texturen)? Sonst verwirft CopyResource die Kopie
/// still - lieber neu aufbauen.
fn oberflaeche_pruefen(t: &ID3D11Texture2D, dw: u32, dh: u32) -> Result<(), String> {
    let mut d = D3D11_TEXTURE2D_DESC::default();
    unsafe { t.GetDesc(&mut d) };
    if d.Width != dw || d.Height != dh {
        return Err(format!("Bildgroesse der Duplication {}x{} passt nicht zum Anzeigemodus {dw}x{dh}", d.Width, d.Height));
    }
    Ok(())
}

/// Soll der Takt (ohne Testbild) das letzte Bild hineingeben? Mit fester
/// Bildrate, wenn seit 0,9/fps nichts kam (Grund ""). Ohne sie nur einmal:
/// aus einem Grund `nachholen` ("Testbild aus", "neuer Zuschauer"), fuer
/// einen frischen Encoder, der noch kein Bild hat (Start, Codecwechsel,
/// Neustart), oder wenn das neueste Bild nicht in den Encoder kam (Raster
/// "zu schnell", Encoder voll, Stau) und seitdem keines mehr (`offen_faellig`,
/// ohne Protokollzeile) - sonst bliebe beim Zuschauer ein Zwischenstand
/// stehen, bis sich der Desktop wieder aendert. Ohne feste Bildrate, mit
/// Bild im Encoder und ohne ausgelassenes: nichts.
fn nachlegen_grund(nachholen: Option<&'static str>, hat_bild: bool, fest_faellig: bool, offen_faellig: bool) -> Option<&'static str> {
    if let Some(grund) = nachholen {
        Some(grund)
    } else if !hat_bild {
        Some("frischer Encoder")
    } else if fest_faellig || offen_faellig {
        Some("")
    } else {
        None
    }
}

/// Kam das neueste Bild (Aufnahmezeit `t_cap`, Hostuhr in us) nicht in den
/// Encoder, und ist seitdem eine ganze Bildzeit ohne neueres vergangen? Dann
/// ist die Bewegung zu Ende, und der Takt legt es nach. Waehrend einer
/// Bewegung kommt das naechste Bild frueher (bei 144 Hz alle 7 ms) - dort
/// soll nichts ueber die Zielrate hinaus nachgelegt werden.
fn ausgelassenes_faellig(letztes: Option<(u64, bool)>, jetzt_us: u64, fps: u32) -> bool {
    matches!(letztes, Some((t, false)) if jetzt_us.saturating_sub(t) >= 1_000_000 / fps.max(1) as u64)
}

/// Buchfuehrung der Verlustmeldungen: derselbe Verlust ohne ein gutes Bild
/// dazwischen (etwa eine Oberflaeche, die dauerhaft nicht zum Modus passt)
/// kommt einmal ins Protokoll, samt der Wiederherstellung danach - nicht
/// alle 2 s zwei Zeilen. Das erste gute Bild beendet den Verlust.
#[derive(Default)]
struct Verlustmeldung {
    letzter: String,
    /// Wie oft sich `letzter` seitdem still wiederholt hat.
    still: u32,
}

impl Verlustmeldung {
    /// Ein Verlust; true = ins Protokoll (neu oder ein anderer als zuletzt).
    fn verlust(&mut self, e: &str) -> bool {
        if !self.letzter.is_empty() && self.letzter == e {
            self.still += 1;
            false
        } else {
            self.letzter = e.to_string();
            self.still = 0;
            true
        }
    }

    /// Die Aufnahme steht wieder; true = ins Protokoll (nicht nach einem
    /// still wiederholten Verlust).
    fn aufgebaut(&self) -> bool {
        self.still == 0
    }

    /// Ein gutes Bild: der Verlust ist vorbei. Liefert, wie oft er sich
    /// still wiederholt hat, falls ueberhaupt (fuer eine Abschlusszeile).
    fn bild_ok(&mut self) -> Option<u32> {
        if self.letzter.is_empty() {
            return None;
        }
        let n = self.still;
        self.letzter.clear();
        self.still = 0;
        (n > 0).then_some(n)
    }
}

/// Alles, was zur laufenden Aufnahme gehoert.
struct Aufnahme {
    dup: Duplication,
    /// STAGING zum Auslesen (Prozessorweg) bzw. DEFAULT-Kopie (Texturweg).
    staging: Option<ID3D11Texture2D>,
    kopie: Option<ID3D11Texture2D>,
    /// Plan aus stromplan: halbieren (ab 3840) oder nur abschneiden.
    halb: bool,
    /// Bild im Hauptspeicher (Prozessorweg), in Stromgroesse.
    ram: Vec<u8>,
    ram_voll: Vec<u8>,
    /// Gedrehter Ausgang: das ganze Bild, wie der Desktop steht.
    ram_gedreht: Vec<u8>,
}

impl Aufnahme {
    /// Die Aufnahme auf einer Duplication, mit der Quelle, die der Encoder
    /// nimmt (Texturen oder Systemspeicher).
    fn neu(dup: Duplication, texturen: bool, halb: bool) -> Result<Aufnahme, String> {
        let mut a = Aufnahme { dup, staging: None, kopie: None, halb, ram: Vec::new(), ram_voll: Vec::new(), ram_gedreht: Vec::new() };
        a.quelle_anlegen(texturen, 0, 0)?;
        Ok(a)
    }

    /// Die Quelle fuer den Encoder anlegen - DEFAULT-Kopie (Texturweg) oder
    /// STAGING zum Auslesen, in Groesse des Modus -, die andere faellt weg.
    /// Traegt die alte schon ein Bild, kommt es mit (auf der Karte kopiert,
    /// beim Wechsel auf den Prozessorweg auch gleich ausgelesen): ein neuer
    /// Encoder soll nicht auf die naechste Aenderung am Desktop warten.
    /// Liefert, ob das Bild mitkam.
    fn quelle_anlegen(&mut self, texturen: bool, w: i32, h: i32) -> Result<bool, String> {
        let (dw, dh) = (self.dup.breite, self.dup.hoehe);
        if texturen {
            let k = self.dup.textur(dw, dh, false)?;
            let mit = match self.staging.take() {
                Some(s) => {
                    unsafe { self.dup.ctx.CopyResource(&k, &s) };
                    true
                }
                None => false,
            };
            self.kopie = Some(k);
            self.ram = Vec::new();
            self.ram_voll = Vec::new();
            self.ram_gedreht = Vec::new();
            Ok(mit)
        } else {
            let s = self.dup.textur(dw, dh, true)?;
            let alt = self.kopie.take();
            if let Some(k) = alt.as_ref() {
                unsafe { self.dup.ctx.CopyResource(&s, k) };
            }
            self.staging = Some(s);
            if alt.is_none() {
                return Ok(false);
            }
            self.einlesen(w, h)?;
            Ok(true)
        }
    }

    /// Das Bild aus der STAGING-Textur in den Hauptspeicher, in
    /// Stromgroesse w x h: halbiert oder abgeschnitten, wie der Plan es
    /// sagt - nie aus einer Annahme. Gedreht: erst die ganze Oberflaeche
    /// lesen und drehen, dann nach Plan.
    fn einlesen(&mut self, w: i32, h: i32) -> Result<(), String> {
        let Some(s) = self.staging.as_ref() else { return Ok(()) };
        let (dw, dh) = (self.dup.breite, self.dup.hoehe);
        let d = self.dup.drehung;
        if d != Drehung::Keine {
            self.dup.auslesen(s, dw, dh, &mut self.ram_voll)?;
            bgra_drehen(&self.ram_voll, dw as usize, dh as usize, d, &mut self.ram_gedreht)?;
            let (gw, gh) = d.groesse(dw as usize, dh as usize);
            if self.halb {
                encoder::bgra_halbieren(&self.ram_gedreht, gw, gh, w as usize, h as usize, &mut self.ram)
            } else {
                zeilen_holen(&self.ram_gedreht, gw * 4, w as usize, h as usize, &mut self.ram)
            }
        } else if self.halb {
            self.dup.auslesen(s, dw, dh, &mut self.ram_voll)?;
            encoder::bgra_halbieren(&self.ram_voll, dw as usize, dh as usize, w as usize, h as usize, &mut self.ram)
        } else {
            self.dup.auslesen(s, w as u32, h as u32, &mut self.ram)
        }
    }
}

/// Wachhalten und 1-ms-Timerperiode fuer die Dauer einer Aufnahmesitzung.
/// Der Abbau steckt in Drop, damit er auch nach einer Panik in sitzung()
/// laeuft (catch_unwind in start, derselbe Faden) - sonst bliebe der
/// Bildschirm ohne Zuschauer wach, und jede weitere Panik forderte die
/// Timerperiode erneut an, ohne sie je abzugeben.
struct Wachhalten;

impl Wachhalten {
    fn an() -> Wachhalten {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
            timeBeginPeriod(1);
        }
        Wachhalten
    }
}

impl Drop for Wachhalten {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

/// Eine Aufnahmesitzung fuer die Dauer eines Zuschauers.
fn sitzung(wunsch: Option<&Ausgang>, weg_wunsch: Weg) {
    // Solange gestreamt wird, darf der Bildschirm nicht einschlafen; die
    // Fristen des Takts brauchen die Millisekunde.
    let wach = Wachhalten::an();

    let mut auf: Option<Aufnahme> = None;
    let mut enc: Option<Betrieb> = None;
    let mut takt = Schrittmacher::neu();
    let mut zeiger = Zeiger::neu();
    let mut testbilder: Option<Vec<Bild>> = None;
    let mut testbild_i = 0usize;
    let mut testbild_an = false;
    let mut naechster_versuch = Instant::now();
    let mut naechster_enc_versuch = Instant::now();
    let mut ausgewichen = false;
    let mut verloren = false;
    let mut kein_bildschirm_gemeldet = false;
    let mut dup_fehler_gemeldet = String::new();
    let mut enc_fehler_gemeldet = String::new();
    let mut zeiger_fehler_gemeldet = false;
    let mut verlustmeldung = Verlustmeldung::default();
    let mut weg = weg_wunsch;
    // Letztes echtes Bild: Aufnahmezeit, ob es codiert wurde (fuer die
    // Wiederholung ohne neue Umrechnung).
    let mut letztes: Option<(u64, bool)> = None;
    // Nach "Testbild aus" oder fuer einen neuen Zuschauer: das letzte
    // Desktopbild einmal nachlegen, auch ohne feste Bildrate (mit Grund).
    let mut nachholen: Option<&'static str> = None;
    let (mut w, mut h) = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);
    let mut zuschauer = netz::zuschauer_nr();

    while netz::zuschauer_da() {
        let fps = Z.fps.load(Ordering::Relaxed);

        // 0. Zuschauerwechsel ohne Pause (Abloesung): die Sitzung laeuft
        //    weiter, der Neue braucht aber, was der Alte schon hatte - die
        //    Meldung eines laufenden Verlusts (sonst wartete er ohne Hinweis
        //    auf ein Bild) und ein erstes Bild auch bei stillem Desktop (das
        //    Vollbild dafuer hat die Annahme schon angefordert). Ein Testbild
        //    hat die Annahme schon ausgeschaltet.
        let nr = netz::zuschauer_nr();
        if nr != zuschauer {
            zuschauer = nr;
            if verloren {
                netz::hoststatus_senden(1);
            }
            if letztes.is_some() {
                nachholen = Some("neuer Zuschauer");
            }
        }

        // 1. Duplication aufbauen oder wiederherstellen, alle 2 s.
        if auf.is_none() && Instant::now() >= naechster_versuch {
            naechster_versuch = Instant::now() + Duration::from_secs(2);
            match ausgang_finden(wunsch, &mut ausgewichen) {
                None => {
                    if !kein_bildschirm_gemeldet {
                        kein_bildschirm_gemeldet = true;
                        log("Kein Bildschirm - warte auf seine Rueckkehr");
                        netz::hoststatus_senden(1);
                        verloren = true;
                    }
                }
                // Die Quelle (Textur oder STAGING) richtet sich nach dem, was
                // der Encoder des laufenden Kandidaten nimmt, nicht nach dem
                // Weg allein; der Null-Kopien-Weg kennt noch keine Skalierung
                // und keine Drehung.
                Some(a) => match duplication_aufbauen(&a).and_then(move |d| {
                    let (gw, gh) = d.desktop_groesse();
                    let (_, _, halb) = stromplan(gw as i32, gh as i32);
                    let weg_hier = if weg == Weg::D3d11 && (halb || d.drehung != Drehung::Keine) { Weg::Bgra } else { weg };
                    Aufnahme::neu(d, encoder::texturweg(Z.codec_id.load(Ordering::Relaxed) as usize, weg_hier), halb)
                }) {
                    Ok(neu) => {
                        kein_bildschirm_gemeldet = false;
                        dup_fehler_gemeldet.clear();
                        let d = &neu.dup;
                        let (gw, gh) = d.desktop_groesse();
                        let ((nw, nh, halb), geaendert) = strom_anpassen(&a, gw as i32, gh as i32);
                        if geaendert || nw != w || nh != h {
                            w = nw;
                            h = nh;
                            if let Some(e) = enc.take() {
                                e.schliessen();
                            }
                            netz::strominfo_senden();
                        }
                        if weg == Weg::D3d11 && halb {
                            log("Null-Kopien-Weg: Ausgang ab 3840 Breite wird noch nicht auf der Karte skaliert - Prozessorweg (bgra)");
                            weg = Weg::Bgra;
                        } else if weg == Weg::D3d11 && d.drehung != Drehung::Keine {
                            log("Null-Kopien-Weg: gedrehter Ausgang wird noch nicht auf der Karte gedreht - Prozessorweg (bgra)");
                            weg = Weg::Bgra;
                        }
                        if verlustmeldung.aufgebaut() {
                            if (a.breite, a.hoehe) != (gw as i32, gh as i32) {
                                log(format!("Ausgang {} meldet {}x{}, der Anzeigemodus ist {}x{} - der Strom folgt dem Anzeigemodus", a.name, a.breite, a.hoehe, gw, gh));
                            }
                            log(format!(
                                "Aufnahme {}: {} {}x{}{} an Karte {} ({}), Format {}, Desktopbild im Systemspeicher: {}, Strom {}x{}{}, Quelle {}",
                                if verloren { "wiederhergestellt" } else { "gestartet" },
                                a.name, gw, gh,
                                if d.drehung != Drehung::Keine { format!(" gedreht {} Grad (Oberflaeche {}x{}, gedreht wird auf dem Prozessor)", d.drehung.grad(), d.breite, d.hoehe) } else { String::new() },
                                a.karte, a.karte_name, d.format.0,
                                if d.im_systemspeicher { "ja" } else { "nein" }, w, h,
                                if halb { " (halbiert)" } else if (w, h) != (gw as i32, gh as i32) { " (ungerader Rand abgeschnitten)" } else { "" },
                                if neu.kopie.is_some() { "Textur" } else { "Prozessorweg" }
                            ));
                        }
                        // Texturen gehoeren zum Geraet: mit ihm faellt auch der Pool.
                        if enc.as_ref().map(|e| e.texturen()).unwrap_or(false) {
                            enc = None;
                        }
                        auf = Some(neu);
                        letztes = None;
                        if verloren {
                            verloren = false;
                            netz::hoststatus_senden(0);
                        }
                        Z.force_key.store(true, Ordering::Relaxed);
                        Z.wait_key.store(true, Ordering::Relaxed);
                    }
                    Err(e) => {
                        if dup_fehler_gemeldet != e {
                            dup_fehler_gemeldet = e.clone();
                            log(format!("Aufnahme: {e} - neuer Versuch alle 2 s"));
                        }
                        if !verloren {
                            verloren = true;
                            netz::hoststatus_senden(1);
                        }
                    }
                },
            }
        }

        // 2. Encoder oeffnen, sobald die Aufnahme steht.
        if auf.is_some() && enc.is_none() && Instant::now() >= naechster_enc_versuch {
            let a = auf.as_ref().unwrap();
            let idx = Z.codec_id.load(Ordering::Relaxed) as usize;
            match Betrieb::oeffnen(idx, w, h, weg, Some((&a.dup.device, &a.dup.ctx))) {
                Ok(b) => {
                    enc = Some(b);
                    enc_fehler_gemeldet.clear();
                    testbilder = None;
                    Z.force_key.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    if enc_fehler_gemeldet != e {
                        enc_fehler_gemeldet = e.clone();
                        log(format!("Encoder laesst sich nicht starten: {e} - neuer Versuch alle 2 s"));
                    }
                    naechster_enc_versuch = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        // 3. Codecwunsch (Nachricht 66): zwischen zwei Bildern, nie mittendrin.
        if let Some(idx) = encoder::codec_wunsch_abholen() {
            codec_wechseln(idx, &mut enc, auf.as_ref(), w, h, weg);
            testbilder = None;
        }

        // 4. Einstellungen (Nachricht 64) auf die Sitzung.
        let mut enc_kaputt = false;
        if let (Some(e), Some(a)) = (enc.as_mut(), auf.as_ref()) {
            match e.einstellungen_nachziehen(weg, Some((&a.dup.device, &a.dup.ctx))) {
                Ok(true) => testbilder = None,
                Ok(false) => {}
                Err(err) => {
                    log(format!("Encoder nach Einstellungen nicht mehr zu oeffnen: {err}"));
                    enc_kaputt = true;
                }
            }
        }
        if enc_kaputt {
            enc = None;
            naechster_enc_versuch = Instant::now() + Duration::from_secs(2);
        }

        // 4b. Nach Start, Codecwechsel oder Neustart: die Aufnahme liefert,
        //     was der Encoder nimmt (Textur oder Systemspeicher) - ein Bild,
        //     das schon da ist, kommt mit. Und eine frisch geoeffnete Sitzung
        //     hat nichts zu wiederholen: das letzte Bild geht noch einmal
        //     ganz hinein, statt dass der Takt ins Leere wiederholt.
        let mut verlust: Option<String> = None;
        if let (Some(e), Some(a)) = (enc.as_ref(), auf.as_mut()) {
            if a.kopie.is_some() != e.texturen() {
                match a.quelle_anlegen(e.texturen(), w, h) {
                    Ok(mit) => {
                        log(format!(
                            "Aufnahme: Quelle auf {} umgestellt (Kandidat {} {} nimmt {}){}",
                            if e.texturen() { "Textur" } else { "Prozessorweg" },
                            e.idx, encoder::kandidat(e.idx).name,
                            if e.texturen() { "Texturen" } else { "Systemspeicher" },
                            if mit { ", letztes Bild mitgenommen" } else { "" }
                        ));
                        letztes = if mit { letztes.map(|(t, _)| (t, false)) } else { None };
                    }
                    Err(err) => verlust = Some(err),
                }
            }
            if !e.hat_bild() {
                letztes = letztes.map(|(t, _)| (t, false));
            }
        }

        // 5. Testbild (Nachricht 68): an -> die zwoelf Bilder im Eingabeformat
        //    der Sitzung, aus -> wieder der Bildschirm (das letzte Bild der
        //    Aufnahme, nicht ein Testbild noch einmal).
        let tb = Z.testbild.load(Ordering::Relaxed);
        if tb != testbild_an {
            testbild_an = tb;
            if tb {
                log(format!("Testbild an: {} Bilder {w}x{h}", super::testbild::N));
                nachholen = None;
            } else {
                log("Testbild aus");
                testbilder = None;
                // Die Pooltextur des letzten Testbilds geht an den Pool zurueck.
                if let Some(e) = enc.as_mut() {
                    e.testbild_freigeben();
                }
                // Die Aufnahme lief weiter: das neueste Desktopbild liegt in
                // STAGING bzw. in der Kopie. Im Prozessorweg traegt der
                // Hauptspeicher noch das Bild von vor dem Testbild - also
                // jetzt einlesen, damit der Takt das aktuelle nachlegt.
                if let (Some(a), Some(_)) = (auf.as_mut(), letztes) {
                    if verlust.is_none() {
                        if let Err(err) = a.einlesen(w, h) {
                            verlust = Some(err);
                        }
                    }
                }
                letztes = letztes.map(|(t, _)| (t, false));
                nachholen = letztes.is_some().then_some("Testbild aus");
            }
            Z.force_key.store(true, Ordering::Relaxed);
        }
        if testbild_an && testbilder.is_none() {
            if let Some(e) = enc.as_ref() {
                match encoder::testbilder(e.pix_fmt, w, h) {
                    Ok(b) => testbilder = Some(b),
                    Err(err) => {
                        log(format!("Testbild: {err}"));
                        Z.testbild.store(false, Ordering::Relaxed);
                    }
                }
            }
        }

        // 6. Ein Bild abholen - hoechstens bis zum naechsten Schlag des Takts.
        let frist = takt.frist_ms();
        match auf.as_mut() {
            None => std::thread::sleep(Duration::from_millis(frist.min(50) as u64)),
            Some(_) if verlust.is_some() => {}
            Some(a) => match a.dup.abholen(frist) {
                Ok(Abholung::Nichts) => {}
                Ok(Abholung::NurZeiger(zi)) => {
                    zeiger_auswerten(&a.dup, &zi, &mut zeiger, &mut zeiger_fehler_gemeldet);
                    a.dup.freigeben();
                }
                Ok(Abholung::Bild { textur, praesentiert_qpc, zeiger: zi }) => {
                    zeiger_auswerten(&a.dup, &zi, &mut zeiger, &mut zeiger_fehler_gemeldet);
                    // Kopie, dann SOFORT freigeben - nur, wenn die Oberflaeche
                    // so gross ist wie die eigenen Texturen.
                    let passt = oberflaeche_pruefen(&textur, a.dup.breite, a.dup.hoehe);
                    if passt.is_ok() {
                        unsafe {
                            if let Some(s) = a.staging.as_ref() {
                                a.dup.ctx.CopyResource(s, &textur);
                            } else if let Some(k) = a.kopie.as_ref() {
                                a.dup.ctx.CopyResource(k, &textur);
                            }
                        }
                    }
                    a.dup.freigeben();
                    drop(textur);
                    if let Some(n) = passt.as_ref().ok().and_then(|_| verlustmeldung.bild_ok()) {
                        log(format!("Aufnahme laeuft wieder (derselbe Verlust hatte sich {n}-mal wiederholt)"));
                    }
                    if let Err(e) = passt {
                        verlust = Some(e);
                    } else if testbild_an {
                        // Testbild an: die Aufnahme laeuft weiter (der Wechsel
                        // zurueck soll keine Sekunde kosten), aber ihre Bilder
                        // gehen nicht in den Encoder. Gemerkt wird nur, dass
                        // die Kopie ein neueres Bild traegt - "Testbild aus"
                        // liest es ein.
                        letztes = Some((super::qpc_us(praesentiert_qpc), false));
                    } else {
                        let t_cap = super::qpc_us(praesentiert_qpc);
                        // Das Bild wird in jedem Fall festgehalten - auch
                        // eines, das gleich wegfaellt: legt der Takt spaeter
                        // nach, soll es das neueste sein.
                        if let Err(e) = a.einlesen(w, h) {
                            verlust = Some(e);
                        }
                        letztes = Some((t_cap, false));
                        if verlust.is_none() {
                            if !takt.schlitz_frei(t_cap as f64 / 1e6, fps) {
                                Z.zu_schnell.fetch_add(1, Ordering::Relaxed);
                            } else if let Some(e) = enc.as_mut() {
                                if e.inflight() >= INFLIGHT_AUFNAHME {
                                    Z.enc_stau.fetch_add(1, Ordering::Relaxed);
                                } else if netz::stau_vor_dem_encoder() {
                                    // Die Leitung staut: das Bild geht gar
                                    // nicht erst in den Encoder (festgehalten
                                    // ist es schon), und fuer den Takt zaehlt
                                    // es wie eines, das hineinging - wie
                                    // g_last_pts auf dem Mac.
                                    takt.ausgelassen();
                                } else {
                                    let pts = takt.pts_vorwaerts(t_cap as i64, fps);
                                    let q = match a.kopie.as_ref() {
                                        Some(k) => Quelle::Textur(k),
                                        None => Quelle::Ram(&a.ram),
                                    };
                                    if e.codieren(q, t_cap, pts, false).is_ok() {
                                        letztes = Some((t_cap, true));
                                        nachholen = None;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => verlust = Some(dxgi_fehler_text(&e)),
            },
        }
        if let Some(e) = verlust {
            if verlustmeldung.verlust(&e) {
                log(format!("Aufnahme verloren: {e} - Nachricht 9, neuer Versuch alle 2 s"));
            }
            if enc.as_ref().map(|x| x.texturen()).unwrap_or(false) {
                enc = None;
            }
            auf = None;
            letztes = None;
            if !verloren {
                verloren = true;
                netz::hoststatus_senden(1);
            }
            naechster_versuch = Instant::now() + Duration::from_secs(2);
        }

        // 7. Der Takt: Testbild als echte Bilder in Zielrate; feste Bildrate
        //    legt das letzte Bild nach, wenn seit 0,9/fps nichts kam. Auch
        //    ohne feste Bildrate geht das letzte Bild EINMAL hinein, wenn
        //    der Encoder frisch ist (Start, Codecwechsel, Neustart) oder das
        //    Testbild endet - sonst sieht der Zuschauer bei stillem Desktop
        //    bis zur naechsten Aenderung nichts bzw. die Balken.
        if takt.tick_faellig(fps) {
            if let Some(e) = enc.as_mut() {
                if testbild_an {
                    if let Some(tb) = testbilder.as_mut() {
                        if e.inflight() >= INFLIGHT_TAKT {
                            // Encoder noch voll: dieser Schlag faellt aus.
                        } else if netz::stau_vor_dem_encoder() {
                            takt.ausgelassen();
                        } else {
                            let jetzt = super::now_us();
                            let pts = takt.pts_vorwaerts(jetzt as i64, fps);
                            let n = tb.len();
                            let b = &mut tb[testbild_i % n];
                            testbild_i += 1;
                            let _ = e.codieren(Quelle::Fertig(b), jetzt, pts, false);
                        }
                    }
                } else if let Some(einmal) = auf.as_ref().and(nachlegen_grund(
                    nachholen,
                    e.hat_bild(),
                    Z.fest.load(Ordering::Relaxed) && takt.nachlegen_faellig(fps),
                    ausgelassenes_faellig(letztes, super::now_us(), fps) && takt.nachlegen_faellig(fps),
                )) {
                    if let Some((t_cap, codiert)) = letztes {
                        if e.inflight() >= INFLIGHT_TAKT {
                            // Encoder noch voll: dieser Schlag faellt aus.
                        } else if netz::stau_vor_dem_encoder() {
                            // Ein Grund "einmal" bleibt stehen: nachgelegt
                            // wird, sobald die Leitung wieder frei ist.
                            takt.ausgelassen();
                        } else {
                            let pts = takt.pts_vorwaerts(super::now_us() as i64, fps);
                            let a = auf.as_ref().unwrap();
                            let q = if codiert {
                                Quelle::Wiederholung
                            } else if let Some(k) = a.kopie.as_ref() {
                                Quelle::Textur(k)
                            } else {
                                Quelle::Ram(&a.ram)
                            };
                            // Der Stempel traegt die ECHTE Aufnahmezeit des
                            // wiederholten Bildes; das Raster bleibt unberuehrt.
                            if e.codieren(q, t_cap, pts, true).is_ok() {
                                letztes = Some((t_cap, true));
                                nachholen = None;
                                Z.repeats.fetch_add(1, Ordering::Relaxed);
                                if !einmal.is_empty() {
                                    log(format!("Takt: letztes Bild nachgelegt ({einmal})"));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 8. Was der Encoder inzwischen fertig hat (bei asynchronen Encodern).
        if let Some(e) = enc.as_mut() {
            let _ = e.pakete_abholen();
        }
        // 9. Zeigerform bei Aenderung.
        zeiger.pruefen();
    }

    // Abbau: kein Zuschauer, keine Arbeit.
    if let Some(e) = enc.take() {
        e.schliessen();
    }
    drop(testbilder);
    drop(auf);
    if Z.testbild.swap(false, Ordering::Relaxed) {
        log("Testbild aus (Zuschauer weg)");
    }
    drop(wach);
    log("Aufnahme angehalten: kein Zuschauer");
}

fn zeiger_auswerten(dup: &Duplication, zi: &ZeigerInfo, zeiger: &mut Zeiger, fehler_gemeldet: &mut bool) {
    if zi.form_bytes > 0 {
        match dup.zeigerform(zi.form_bytes) {
            Ok((info, daten)) => zeiger.form_setzen(&info, &daten),
            Err(e) => {
                if !*fehler_gemeldet {
                    *fehler_gemeldet = true;
                    log(format!("Zeigerform: GetFramePointerShape: {}", dxgi_fehler_text(&e)));
                }
            }
        }
    }
    if zi.maus_aktualisiert {
        zeiger.sichtbar_setzen(zi.sichtbar);
    }
}

/// Codecwechsel wie main.m: alte Sitzung leeren (ihre letzten Pakete gehen
/// noch raus), neue oeffnen, ERST DANN Switch 7, Vollbild erzwingen,
/// Strominfo 1. Scheitert die neue, kommt die alte zurueck - ohne Switch.
fn codec_wechseln(idx: usize, enc: &mut Option<Betrieb>, auf: Option<&Aufnahme>, w: i32, h: i32, weg: Weg) {
    let alt = Z.codec_id.load(Ordering::Relaxed) as usize;
    let Some(a) = auf else {
        log(format!("Codecwunsch {} abgelehnt: keine laufende Aufnahme", encoder::kandidat(idx).name));
        return;
    };
    if idx == alt && enc.is_some() {
        log(format!("Codecwunsch {}: laeuft bereits", encoder::kandidat(idx).name));
        return;
    }
    log(format!("Codecwechsel: {} -> {}", encoder::kandidat(alt).name, encoder::kandidat(idx).name));
    if let Some(e) = enc.take() {
        e.schliessen();
    }
    // Der Zuschauer wartet wieder auf ein Vollbild; das SWITCH kommt erst,
    // wenn die neue Sitzung steht.
    Z.wait_key.store(true, Ordering::Relaxed);
    Z.force_key.store(true, Ordering::Relaxed);
    let geraet = Some((&a.dup.device, &a.dup.ctx));
    match Betrieb::oeffnen(idx, w, h, weg, geraet) {
        Ok(b) => {
            Z.codec_id.store(idx as u32, Ordering::Relaxed);
            *enc = Some(b);
            encoder::switch_senden(idx);
            Z.force_key.store(true, Ordering::Relaxed);
            netz::strominfo_senden();
            log(format!("Codec gewechselt: {}", encoder::kandidat(idx).name));
        }
        Err(e) => {
            log(format!("Codecwechsel auf {} fehlgeschlagen ({e}) - baue {} wieder auf", encoder::kandidat(idx).name, encoder::kandidat(alt).name));
            match Betrieb::oeffnen(alt, w, h, weg, geraet) {
                Ok(b) => {
                    *enc = Some(b);
                    Z.force_key.store(true, Ordering::Relaxed);
                    netz::strominfo_senden();
                    log(format!("Alter Codec laeuft wieder: {}", encoder::kandidat(alt).name));
                }
                Err(e2) => log(format!("Auch der alte Codec {} laesst sich nicht mehr oeffnen ({e2}) - es kommt kein Bild mehr", encoder::kandidat(alt).name)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stromplan_halbiert_nur_ab_3840() {
        // Halbiert wird nach Plan (ab 3840 Breite), ein ungerader Rand nur
        // abgeschnitten - nie "weicht ab, also halbieren".
        assert_eq!(stromplan(1920, 1080), (1920, 1080, false));
        assert_eq!(stromplan(1367, 769), (1366, 768, false));
        assert_eq!(stromplan(1080, 1920), (1080, 1920, false));
        assert_eq!(stromplan(5120, 1440), (2560, 720, true));
        assert_eq!(stromplan(3842, 2160), (1920, 1080, true));
        assert_eq!(stromplan(3840, 2160), (1920, 1080, true));
        assert_eq!(stromplan(3839, 2160), (3838, 2160, false));
    }

    #[test]
    fn plan_und_quelle_ergeben_genau_die_stromgroesse() {
        // Fuer jeden Plan liefert der Weg der Aufnahme (Abschneiden beim
        // Auslesen bzw. Halbieren) genau w*h*4 Byte - mit einem
        // Zeilenabstand wie bei einer gemappten STAGING-Textur.
        for (dw, dh) in [(1920usize, 1080usize), (1367, 769), (5120, 1440), (3842, 2160), (3840, 2160), (1080, 1920)] {
            let (w, h, halb) = stromplan(dw as i32, dh as i32);
            let (w, h) = (w as usize, h as usize);
            let abstand = (dw * 4 + 255) & !255;
            let gemappt = vec![9u8; abstand * dh];
            let mut ram = Vec::new();
            if halb {
                let mut voll = Vec::new();
                zeilen_holen(&gemappt, abstand, dw, dh, &mut voll).unwrap();
                encoder::bgra_halbieren(&voll, dw, dh, w, h, &mut ram).unwrap();
            } else {
                zeilen_holen(&gemappt, abstand, w, h, &mut ram).unwrap();
            }
            assert_eq!(ram.len(), w * h * 4, "{dw}x{dh}");
        }
    }

    #[test]
    fn takt_legt_einmal_nach_ohne_feste_bildrate() {
        // Ohne feste Bildrate, mit Bild im Encoder und ohne ausgelassenes
        // Bild: nichts (wie main.m).
        assert_eq!(nachlegen_grund(None, true, false, false), None);
        // Frischer Encoder (Start, Codecwechsel, Neustart), Testbild aus bzw.
        // neuer Zuschauer: einmal, auch ohne feste Bildrate.
        assert_eq!(nachlegen_grund(None, false, false, false), Some("frischer Encoder"));
        assert_eq!(nachlegen_grund(Some("Testbild aus"), true, false, false), Some("Testbild aus"));
        assert_eq!(nachlegen_grund(Some("Testbild aus"), false, true, true), Some("Testbild aus"));
        assert_eq!(nachlegen_grund(Some("neuer Zuschauer"), true, false, false), Some("neuer Zuschauer"));
        // Feste Bildrate: regulaer nachlegen, ohne Protokollzeile.
        assert_eq!(nachlegen_grund(None, true, true, false), Some(""));
        // Ohne feste Bildrate: das ausgelassene letzte Bild einer Bewegung,
        // ebenfalls ohne Protokollzeile.
        assert_eq!(nachlegen_grund(None, true, false, true), Some(""));
    }

    /// Das Endbild einer Bewegung, das als "zu schnell" (Raster), wegen
    /// Encoder-Grenze oder Stau nicht in den Encoder kam, legt der Takt nach -
    /// aber erst, wenn eine ganze Bildzeit kein neueres kam, also nie mitten
    /// in einer Bewegung. Ein codiertes Bild wird ohne feste Bildrate nicht
    /// wiederholt.
    #[test]
    fn ausgelassenes_endbild_wird_nachgelegt() {
        let fps = 60;
        let bildzeit = 1_000_000 / fps as u64;
        let t = 5_000_000;
        assert!(!ausgelassenes_faellig(None, t + bildzeit, fps));
        assert!(!ausgelassenes_faellig(Some((t, true)), t + 10 * bildzeit, fps), "codiert: nichts nachzulegen");
        assert!(!ausgelassenes_faellig(Some((t, false)), t + bildzeit - 1, fps), "Bewegung laeuft vielleicht noch");
        assert!(ausgelassenes_faellig(Some((t, false)), t + bildzeit, fps));
        // Pruefer-Probe: Bewegungen aus 2 bis 39 Bildern bei 144 Hz, Ziel 60
        // fps, nach einer Pause. Ohne Nachlegen blieb bei 23 von 38 das
        // Endbild ungesendet; jetzt bekommt jede ihr Endbild, und nachgelegt
        // wird hoechstens einmal je Bewegung und erst nach ihrem Ende.
        let hz = 144.0;
        let mut ohne_endbild_vorher = 0;
        for n in 2..=39u32 {
            let mut takt = Schrittmacher::neu();
            let beginn = 100.0 + n as f64;
            let mut letztes: Option<(u64, bool)> = None;
            for k in 0..n {
                let t_s = beginn + k as f64 / hz;
                let t_us = (t_s * 1e6) as u64;
                // Waehrend der Bewegung: nie nachlegen, das naechste Bild kommt vor Ablauf einer Bildzeit.
                if let Some(l) = letztes {
                    assert!(!ausgelassenes_faellig(Some(l), t_us - 1, fps), "n {n}, Bild {k}");
                }
                letztes = Some((t_us, takt.schlitz_frei(t_s, fps)));
            }
            let (t_end, codiert) = letztes.unwrap();
            if !codiert {
                ohne_endbild_vorher += 1;
            }
            // Nach der Bewegung: genau dann faellig, wenn das Endbild fehlt.
            assert_eq!(ausgelassenes_faellig(letztes, t_end + bildzeit, fps), !codiert, "n {n}");
        }
        assert_eq!(ohne_endbild_vorher, 23, "Probe des Pruefers nicht getroffen");
    }

    #[test]
    fn derselbe_verlust_kommt_einmal_ins_protokoll() {
        // Bleibt ein Verlust bestehen (Neuaufbau alle 2 s, gleich wieder
        // verloren), gibt es je eine Zeile fuer Verlust und Wiederherstellung,
        // danach Ruhe - bis ein gutes Bild kommt oder ein anderer Verlust.
        let mut m = Verlustmeldung::default();
        assert!(m.aufgebaut(), "erster Start");
        assert!(m.bild_ok().is_none(), "gutes Bild ohne Verlust: keine Zeile");
        assert!(m.verlust("Bildgroesse passt nicht"));
        assert!(m.aufgebaut(), "erste Wiederherstellung wird gemeldet");
        for _ in 0..5 {
            assert!(!m.verlust("Bildgroesse passt nicht"));
            assert!(!m.aufgebaut());
        }
        assert!(m.verlust("Zugriff verloren"), "ein anderer Verlust wird gemeldet");
        assert!(m.aufgebaut());
        assert!(!m.verlust("Zugriff verloren"));
        assert_eq!(m.bild_ok(), Some(1), "Abschlusszeile mit der Zahl der stillen Wiederholungen");
        assert!(m.bild_ok().is_none());
        // Nach einem guten Bild ist derselbe Verlust wieder eine Meldung wert.
        assert!(m.verlust("Zugriff verloren"));
        assert!(m.aufgebaut());
        assert_eq!(m.bild_ok(), None, "ohne stille Wiederholung keine Abschlusszeile");
    }

    #[test]
    fn auslesen_ueber_das_ende_ist_ein_fehler() {
        // Zu kurze Quelle oder eine Zeile laenger als der Zeilenabstand:
        // Fehler statt Lesen hinter dem Ende.
        let mut ziel = Vec::new();
        let quelle = vec![1u8; 1280 * 4 * 720];
        assert!(zeilen_holen(&quelle, 1280 * 4, 1920, 1080, &mut ziel).is_err());
        assert!(zeilen_holen(&quelle, 1280 * 4, 1280, 721, &mut ziel).is_err());
        assert!(zeilen_holen(&quelle, 1280 * 4, 0, 720, &mut ziel).is_err());
        zeilen_holen(&quelle, 1280 * 4, 1280, 720, &mut ziel).unwrap();
        assert_eq!(ziel.len(), 1280 * 720 * 4);
        // Die letzte Zeile braucht nur ihre eigenen Bytes, nicht den ganzen Abstand.
        zeilen_holen(&quelle[..1280 * 4 * 719 + 100 * 4], 1280 * 4, 100, 720, &mut ziel).unwrap();
        assert_eq!(ziel.len(), 100 * 720 * 4);
    }

    /// Testbild: jeder Punkt traegt seine Lage (x, y) in der Oberflaeche.
    fn lagebild(w: usize, h: usize) -> Vec<u8> {
        let mut b = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                b.extend_from_slice(&[x as u8, y as u8, (x >> 8) as u8, (y >> 8) as u8]);
            }
        }
        b
    }

    fn punkt(b: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [b[i], b[i + 1], b[i + 2], b[i + 3]]
    }

    #[test]
    fn drehung_ecken_alle_vier() {
        // Oberflaeche 5x3, die Ecken: oben links (OL), oben rechts (OR),
        // unten links (UL), unten rechts (UR). 90 Grad = im Uhrzeigersinn
        // (MS-Beispiel DisplayManager.cpp, SetDirtyVert): OL landet oben
        // rechts im Desktop, OR unten rechts, UR unten links, UL oben links.
        let (sw, sh) = (5usize, 3usize);
        let src = lagebild(sw, sh);
        let (ol, or, ul, ur) = (punkt(&src, sw, 0, 0), punkt(&src, sw, 4, 0), punkt(&src, sw, 0, 2), punkt(&src, sw, 4, 2));
        let mut z = Vec::new();

        bgra_drehen(&src, sw, sh, Drehung::Keine, &mut z).unwrap();
        assert_eq!(z, src);

        bgra_drehen(&src, sw, sh, Drehung::Grad90, &mut z).unwrap();
        assert_eq!(Drehung::Grad90.groesse(sw, sh), (3, 5));
        assert_eq!(z.len(), 3 * 5 * 4);
        assert_eq!(punkt(&z, 3, 2, 0), ol, "90: OL -> oben rechts");
        assert_eq!(punkt(&z, 3, 2, 4), or, "90: OR -> unten rechts");
        assert_eq!(punkt(&z, 3, 0, 4), ur, "90: UR -> unten links");
        assert_eq!(punkt(&z, 3, 0, 0), ul, "90: UL -> oben links");

        bgra_drehen(&src, sw, sh, Drehung::Grad180, &mut z).unwrap();
        assert_eq!(z.len(), 5 * 3 * 4);
        assert_eq!(punkt(&z, 5, 4, 2), ol, "180: OL -> unten rechts");
        assert_eq!(punkt(&z, 5, 0, 2), or, "180: OR -> unten links");
        assert_eq!(punkt(&z, 5, 0, 0), ur, "180: UR -> oben links");
        assert_eq!(punkt(&z, 5, 4, 0), ul, "180: UL -> oben rechts");

        bgra_drehen(&src, sw, sh, Drehung::Grad270, &mut z).unwrap();
        assert_eq!(z.len(), 3 * 5 * 4);
        assert_eq!(punkt(&z, 3, 0, 4), ol, "270: OL -> unten links");
        assert_eq!(punkt(&z, 3, 0, 0), or, "270: OR -> oben links");
        assert_eq!(punkt(&z, 3, 2, 0), ur, "270: UR -> oben rechts");
        assert_eq!(punkt(&z, 3, 2, 4), ul, "270: UL -> unten rechts");
    }

    #[test]
    fn drehung_jeder_punkt_wie_im_ms_beispiel() {
        // Jeder Punkt der Oberflaeche (xs, ys) landet im Desktop dort, wo
        // DisplayManager.cpp (SetDirtyVert: DestDirty.left = Width -
        // Dirty->bottom, DestDirty.top = Dirty->left fuer ROTATE90 usw.) ihn
        // hinlegt - auch ueber mehrere Streifen mit Rest (37 x 21).
        for (sw, sh) in [(37usize, 21usize), (21, 37), (1, 1), (2, 1), (16, 33)] {
            let src = lagebild(sw, sh);
            for d in [Drehung::Keine, Drehung::Grad90, Drehung::Grad180, Drehung::Grad270] {
                let mut z = Vec::new();
                bgra_drehen(&src, sw, sh, d, &mut z).unwrap();
                let (gw, gh) = d.groesse(sw, sh);
                assert_eq!(z.len(), gw * gh * 4);
                for ys in 0..sh {
                    for xs in 0..sw {
                        let (xd, yd) = match d {
                            Drehung::Keine => (xs, ys),
                            Drehung::Grad90 => (sh - 1 - ys, xs),
                            Drehung::Grad180 => (sw - 1 - xs, sh - 1 - ys),
                            Drehung::Grad270 => (ys, sw - 1 - xs),
                        };
                        assert_eq!(punkt(&z, gw, xd, yd), punkt(&src, sw, xs, ys), "{d:?} {sw}x{sh} ({xs},{ys})");
                    }
                }
            }
            // Hin und zurueck ist das Bild wieder dasselbe.
            let (mut a, mut b) = (Vec::new(), Vec::new());
            bgra_drehen(&src, sw, sh, Drehung::Grad90, &mut a).unwrap();
            bgra_drehen(&a, sh, sw, Drehung::Grad270, &mut b).unwrap();
            assert_eq!(b, src);
            bgra_drehen(&src, sw, sh, Drehung::Grad180, &mut a).unwrap();
            bgra_drehen(&a, sw, sh, Drehung::Grad180, &mut b).unwrap();
            assert_eq!(b, src);
        }
        // Zu kurze Quelle: Fehler statt Lesen hinter dem Ende.
        let mut z = Vec::new();
        assert!(bgra_drehen(&vec![0u8; 5 * 3 * 4 - 1], 5, 3, Drehung::Grad90, &mut z).is_err());
        assert!(bgra_drehen(&[], 0, 3, Drehung::Grad180, &mut z).is_err());
    }

    #[test]
    fn oberflaeche_ist_ungedreht() {
        // Desktop 1080x1920 bei 90/270 Grad: die Oberflaeche ist 1920x1080 -
        // ob ModeDesc gedreht (1080x1920) oder schon ungedreht (1920x1080)
        // dasteht. Hochkantes Panel quer betrieben (Desktop 1920x1200):
        // Oberflaeche 1200x1920. 0/180 Grad: ModeDesc, wie er ist.
        for d in [Drehung::Grad90, Drehung::Grad270] {
            assert_eq!(oberflaeche_groesse(d, 1080, 1920, 1080, 1920), (1920, 1080));
            assert_eq!(oberflaeche_groesse(d, 1920, 1080, 1080, 1920), (1920, 1080));
            assert_eq!(oberflaeche_groesse(d, 1920, 1200, 1920, 1200), (1200, 1920));
            assert_eq!(oberflaeche_groesse(d, 1200, 1920, 1920, 1200), (1200, 1920));
            // Skalierung ohne DPI-Awareness verkleinert DesktopCoordinates,
            // das Format (hoch/quer) bleibt.
            assert_eq!(oberflaeche_groesse(d, 1080, 1920, 720, 1280), (1920, 1080));
        }
        for d in [Drehung::Keine, Drehung::Grad180] {
            assert_eq!(oberflaeche_groesse(d, 1920, 1080, 1920, 1080), (1920, 1080));
            assert_eq!(oberflaeche_groesse(d, 1367, 769, 1367, 769), (1367, 769));
        }
    }

    #[test]
    fn gedreht_ergeben_plan_und_quelle_genau_die_stromgroesse() {
        // Wie plan_und_quelle_ergeben_genau_die_stromgroesse, mit Drehung:
        // gemappte Oberflaeche -> drehen -> abschneiden bzw. halbieren.
        for (sw, sh, d) in [
            (1920usize, 1080usize, Drehung::Grad90),
            (1920, 1080, Drehung::Grad270),
            (1367, 769, Drehung::Grad90),
            (3840, 2160, Drehung::Grad180),
            (1200, 1920, Drehung::Grad90),
            (2160, 3840, Drehung::Grad270),
        ] {
            let (gw, gh) = d.groesse(sw, sh);
            let (w, h, halb) = stromplan(gw as i32, gh as i32);
            let (w, h) = (w as usize, h as usize);
            let abstand = (sw * 4 + 255) & !255;
            let gemappt = vec![9u8; abstand * sh];
            let (mut voll, mut gedreht, mut ram) = (Vec::new(), Vec::new(), Vec::new());
            zeilen_holen(&gemappt, abstand, sw, sh, &mut voll).unwrap();
            bgra_drehen(&voll, sw, sh, d, &mut gedreht).unwrap();
            if halb {
                encoder::bgra_halbieren(&gedreht, gw, gh, w, h, &mut ram).unwrap();
            } else {
                zeilen_holen(&gedreht, gw * 4, w, h, &mut ram).unwrap();
            }
            assert_eq!(ram.len(), w * h * 4, "{sw}x{sh} {d:?}");
        }
    }

    #[test]
    fn wachhalten_endet_auch_nach_einer_panik() {
        // SetThreadExecutionState gilt je Faden und liefert den vorigen
        // Zustand: waehrend der Sitzung wach, nach einer Panik in ihr wieder
        // nur ES_CONTINUOUS. Eigener Faden, weil die Tests parallel laufen.
        // Auf der Bau-VM (ssh, Sitzung 0) kommt ES_DISPLAY_REQUIRED nicht
        // zurueck, ES_SYSTEM_REQUIRED schon.
        use windows::Win32::System::Power::EXECUTION_STATE;
        std::thread::spawn(|| {
            let mut waehrend = EXECUTION_STATE(0);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _wach = Wachhalten::an();
                waehrend = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) };
                // resume_unwind ruft den Panik-Haken nicht: keine Zeile im Testlauf.
                std::panic::resume_unwind(Box::new("Probe"));
            }));
            assert!(r.is_err());
            assert!(waehrend.0 & ES_CONTINUOUS.0 != 0 && waehrend.0 & ES_SYSTEM_REQUIRED.0 != 0, "waehrend der Sitzung nicht wach: {waehrend:?}");
            let danach = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            assert_eq!(danach, ES_CONTINUOUS, "nach der Panik noch wach");
        })
        .join()
        .unwrap();
    }
}
