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

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};

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

/// Groesse des Stroms fuer einen Ausgang: ueber 3840 breit wird halbiert
/// (wie beim Mac), immer gerade.
pub fn stromgroesse(a: &Ausgang) -> (i32, i32) {
    let (w, h) = if a.breite >= 3840 { (a.breite / 2, a.hoehe / 2) } else { (a.breite, a.hoehe) };
    (w & !1, h & !1)
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
    pub breite: u32,
    pub hoehe: u32,
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
    Ok(Duplication {
        device,
        ctx,
        dup,
        adapter,
        breite: desc.ModeDesc.Width,
        hoehe: desc.ModeDesc.Height,
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

    /// STAGING-Textur in den Hauptspeicher lesen (BGRA, w*4 Byte je Zeile).
    pub fn auslesen(&self, staging: &ID3D11Texture2D, w: u32, h: u32, ziel: &mut Vec<u8>) -> Result<(), String> {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m)) }.map_err(|e| fehler("Map Staging", e))?;
        let zeile = (w * 4) as usize;
        ziel.resize(zeile * h as usize, 0);
        unsafe {
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(
                    (m.pData as *const u8).add(y * m.RowPitch as usize),
                    ziel.as_mut_ptr().add(y * zeile),
                    zeile,
                );
            }
            self.ctx.Unmap(staging, 0);
        }
        Ok(())
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
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sitzung(wunsch.as_ref(), weg)));
            if r.is_err() {
                log("Aufnahme: Faden abgestuerzt - neuer Anlauf mit dem naechsten Zuschauer");
                while netz::zuschauer_da() {
                    std::thread::sleep(Duration::from_millis(500));
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

/// Bild und Maus auf denselben Ausgang; Stromgroesse in Z. Liefert die
/// Stromgroesse und ob sie sich geaendert hat.
fn strom_anpassen(a: &Ausgang) -> ((i32, i32), bool) {
    let (w, h) = stromgroesse(a);
    super::eingabe::ausgang_setzen(a.links, a.oben, a.breite, a.hoehe);
    let alt = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);
    Z.info_w.store(w as u32, Ordering::Relaxed);
    Z.info_h.store(h as u32, Ordering::Relaxed);
    ((w, h), alt != (w, h))
}

/// Alles, was zur laufenden Aufnahme gehoert.
struct Aufnahme {
    dup: Duplication,
    /// STAGING zum Auslesen (Prozessorweg) bzw. DEFAULT-Kopie (Texturweg).
    staging: Option<ID3D11Texture2D>,
    kopie: Option<ID3D11Texture2D>,
    /// Bild im Hauptspeicher (Prozessorweg), ggf. halbiert.
    ram: Vec<u8>,
    ram_voll: Vec<u8>,
}

/// Eine Aufnahmesitzung fuer die Dauer eines Zuschauers.
fn sitzung(wunsch: Option<&Ausgang>, weg_wunsch: Weg) {
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};

    // Solange gestreamt wird, darf der Bildschirm nicht einschlafen; die
    // Fristen des Takts brauchen die Millisekunde.
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        timeBeginPeriod(1);
    }

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
    let mut weg = weg_wunsch;
    // Letztes echtes Bild: Aufnahmezeit, ob es codiert wurde (fuer die
    // Wiederholung ohne neue Umrechnung).
    let mut letztes: Option<(u64, bool)> = None;
    let (mut w, mut h) = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);

    while netz::zuschauer_da() {
        let fps = Z.fps.load(Ordering::Relaxed);

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
                Some(a) => match duplication_aufbauen(&a) {
                    Ok(d) => {
                        kein_bildschirm_gemeldet = false;
                        dup_fehler_gemeldet.clear();
                        let ((nw, nh), geaendert) = strom_anpassen(&a);
                        if geaendert || nw != w || nh != h {
                            w = nw;
                            h = nh;
                            if let Some(e) = enc.take() {
                                e.schliessen();
                            }
                            netz::strominfo_senden();
                        }
                        // Der Null-Kopien-Weg kennt noch keine Skalierung.
                        if weg == Weg::D3d11 && (d.breite as i32 != w || d.hoehe as i32 != h) {
                            log("Null-Kopien-Weg: Ausgang breiter als 3840 wird noch nicht auf der Karte skaliert - Prozessorweg (bgra)");
                            weg = Weg::Bgra;
                        }
                        let texturen = weg == Weg::D3d11;
                        let staging = if texturen { None } else { d.textur(d.breite, d.hoehe, true).ok() };
                        let kopie = if texturen { d.textur(d.breite, d.hoehe, false).ok() } else { None };
                        log(format!(
                            "Aufnahme {}: {} {}x{} an Karte {} ({}), Format {}, Desktopbild im Systemspeicher: {}, Strom {}x{}",
                            if verloren { "wiederhergestellt" } else { "gestartet" },
                            a.name, d.breite, d.hoehe, a.karte, a.karte_name, d.format.0,
                            if d.im_systemspeicher { "ja" } else { "nein" }, w, h
                        ));
                        // Texturen gehoeren zum Geraet: mit ihm faellt auch der Pool.
                        if enc.as_ref().map(|e| e.texturen()).unwrap_or(false) {
                            enc = None;
                        }
                        auf = Some(Aufnahme { dup: d, staging, kopie, ram: Vec::new(), ram_voll: Vec::new() });
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

        // 5. Testbild (Nachricht 68): an -> die zwoelf Bilder im Eingabeformat
        //    der Sitzung, aus -> wieder der Bildschirm.
        let tb = Z.testbild.load(Ordering::Relaxed);
        if tb != testbild_an {
            testbild_an = tb;
            if tb {
                log(format!("Testbild an: {} Bilder {w}x{h}", super::testbild::N));
            } else {
                log("Testbild aus");
                testbilder = None;
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
        let mut verlust: Option<String> = None;
        match auf.as_mut() {
            None => std::thread::sleep(Duration::from_millis(frist.min(50) as u64)),
            Some(a) => match a.dup.abholen(frist) {
                Ok(Abholung::Nichts) => {}
                Ok(Abholung::NurZeiger(zi)) => {
                    zeiger_auswerten(&a.dup, &zi, &mut zeiger, &mut zeiger_fehler_gemeldet);
                    a.dup.freigeben();
                }
                Ok(Abholung::Bild { textur, praesentiert_qpc, zeiger: zi }) => {
                    zeiger_auswerten(&a.dup, &zi, &mut zeiger, &mut zeiger_fehler_gemeldet);
                    // Kopie, dann SOFORT freigeben.
                    unsafe {
                        if let Some(s) = a.staging.as_ref() {
                            a.dup.ctx.CopyResource(s, &textur);
                        } else if let Some(k) = a.kopie.as_ref() {
                            a.dup.ctx.CopyResource(k, &textur);
                        }
                    }
                    a.dup.freigeben();
                    drop(textur);
                    // Testbild an: die Aufnahme laeuft weiter (der Wechsel
                    // zurueck soll keine Sekunde kosten), aber ihre Bilder
                    // gehen nicht in den Encoder.
                    if !testbild_an {
                        let t_cap = super::qpc_us(praesentiert_qpc);
                        // Das Bild wird in jedem Fall festgehalten - auch
                        // eines, das gleich wegfaellt: legt der Takt spaeter
                        // nach, soll es das neueste sein.
                        if let Some(s) = a.staging.as_ref() {
                            let (dw, dh) = (a.dup.breite, a.dup.hoehe);
                            if dw as i32 == w && dh as i32 == h {
                                if let Err(e) = a.dup.auslesen(s, dw, dh, &mut a.ram) {
                                    verlust = Some(e);
                                }
                            } else {
                                match a.dup.auslesen(s, dw, dh, &mut a.ram_voll) {
                                    Ok(()) => encoder::bgra_halbieren(&a.ram_voll, dw as usize, dh as usize, &mut a.ram),
                                    Err(e) => verlust = Some(e),
                                }
                            }
                        }
                        letztes = Some((t_cap, false));
                        if verlust.is_none() {
                            if !takt.schlitz_frei(t_cap as f64 / 1e6, fps) {
                                Z.zu_schnell.fetch_add(1, Ordering::Relaxed);
                            } else if let Some(e) = enc.as_mut() {
                                if e.inflight() >= INFLIGHT_AUFNAHME {
                                    Z.enc_stau.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    let pts = takt.pts_vorwaerts(t_cap as i64, fps);
                                    let q = match a.kopie.as_ref() {
                                        Some(k) => Quelle::Textur(k),
                                        None => Quelle::Ram(&a.ram),
                                    };
                                    if e.codieren(q, t_cap, pts, false).is_ok() {
                                        letztes = Some((t_cap, true));
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
            log(format!("Aufnahme verloren: {e} - Nachricht 9, neuer Versuch alle 2 s"));
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
        //    legt das letzte Bild nach, wenn seit 0,9/fps nichts kam.
        if takt.tick_faellig(fps) {
            if let Some(e) = enc.as_mut() {
                if testbild_an {
                    if let Some(tb) = testbilder.as_mut() {
                        if e.inflight() < INFLIGHT_TAKT {
                            let jetzt = super::now_us();
                            let pts = takt.pts_vorwaerts(jetzt as i64, fps);
                            let n = tb.len();
                            let b = &mut tb[testbild_i % n];
                            testbild_i += 1;
                            let _ = e.codieren(Quelle::Fertig(b), jetzt, pts, false);
                        }
                    }
                } else if Z.fest.load(Ordering::Relaxed) && auf.is_some() && takt.nachlegen_faellig(fps) {
                    if let Some((t_cap, codiert)) = letztes {
                        if e.inflight() < INFLIGHT_TAKT {
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
                                Z.repeats.fetch_add(1, Ordering::Relaxed);
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
    unsafe {
        timeEndPeriod(1);
        SetThreadExecutionState(ES_CONTINUOUS);
    }
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
