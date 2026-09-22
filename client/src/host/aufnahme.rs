// Aufnahme des Windows-Hosts: Ausgangsliste (DXGI), Ausgangswahl und die
// Grundlage der Desktop Duplication - heute fuer --list und --messen, im
// Betrieb (Schritt 4) mit Aufnahmefaden, Wiederherstellung und Zeigerform.
//
// Quelle der Liste: IDXGIFactory1::EnumAdapters1 -> IDXGIAdapter1::EnumOutputs
// -> IDXGIOutput::GetDesc (DeviceName, DesktopCoordinates, AttachedToDesktop).
// Auf einem Laptop ohne MUX haengen ALLE Ausgaenge an der integrierten Karte;
// die Liste zeigt das sofort. Ohne DXGI-Ausgang (WARP, RDP) bleibt fuer die
// Maus die GDI-Liste (EnumDisplayMonitors).

use std::ffi::c_void;

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};

use super::log;

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
/// IDXGIOutput1::DuplicateOutput. HDR-Ausgaenge (R16G16B16A16_FLOAT) werden
/// vorerst abgelehnt.
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
    Ok(Duplication {
        device,
        ctx,
        dup,
        adapter,
        breite: desc.ModeDesc.Width,
        hoehe: desc.ModeDesc.Height,
        format,
        im_systemspeicher: desc.DesktopImageInSystemMemory.as_bool(),
    })
}

/// Ein abgeholtes Bild.
pub enum Abholung {
    /// Innerhalb der Frist kam nichts.
    Nichts,
    /// Nur der Zeiger hat sich bewegt (LastPresentTime == 0).
    NurZeiger,
    /// Ein neues Bild: Textur der Duplication (bis ReleaseFrame gueltig),
    /// Praesentationszeit (QPC) und ob eine Zeigerform mitkam.
    Bild { textur: ID3D11Texture2D, praesentiert_qpc: i64, zeigerform_bytes: u32 },
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
        if info.LastPresentTime == 0 {
            unsafe {
                let _ = self.dup.ReleaseFrame();
            }
            return Ok(Abholung::NurZeiger);
        }
        let textur: ID3D11Texture2D = match res.and_then(|r| r.cast().ok()) {
            Some(t) => t,
            None => {
                unsafe {
                    let _ = self.dup.ReleaseFrame();
                }
                return Ok(Abholung::Nichts);
            }
        };
        Ok(Abholung::Bild { textur, praesentiert_qpc: info.LastPresentTime, zeigerform_bytes: info.PointerShapeBufferSize })
    }

    pub fn freigeben(&self) {
        unsafe {
            let _ = self.dup.ReleaseFrame();
        }
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
