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
// bei Erfolg 9 = 0 und Vollbild erzwingen.
//
// Bildschirmwahl (Spezifikation Bildschirm, 1.1-1.5): der Host streamt in
// der Automatik den Hauptbildschirm (MONITORINFOF_PRIMARY) und folgt ihm;
// ein Wunsch des Zuschauers (Nachricht 70, Kennung aus der Liste 12) macht
// einen Bildschirm fest - faellt er weg, gilt der Hauptbildschirm als
// Ausweichplatz, kommt er zurueck, wechselt der Host von selbst zurueck.
// Die Kennung ist die Geraete-ID des Monitors (ACR0501), nie der
// Listenplatz; der Wunsch steht in bildschirm.txt. Der Aufnahmefaden zaehlt
// die Ausgaenge alle 2 s neu auf und bewertet neu (ziel_waehlen); ein
// Wechsel laeuft wie ein Codecwechsel zwischen zwei Bildern: Duplication
// neu, Switch 7, Info 1, Vollbild, Nachricht 12 mit dem neuen Stand.
//
// Stromgroesse (stromplan): nativ - genau die Pixel des Anzeigemodus, wie
// die Duplication sie liefert, auch 3840x2160 und groesser; die Skalierung
// von Windows (150 %) aendert daran nichts. Nichts wird halbiert, nur ein
// ungerader Rand faellt weg. Damit braucht kein Weg einen Skalierer, und
// ein 4K-Ausgang nimmt dieselben Wege wie ein kleinerer (bgra in NVENC,
// Null-Kopien, wo er erlaubt ist).
//
// Gedrehte Ausgaenge (Hochformat, 180 Grad, hochkantes Panel, das Windows
// quer betreibt): die Oberflaeche kommt ungedreht, gedreht wird beim
// Einlesen auf dem Prozessor (Drehung, bgra_drehen); der Strom hat die
// Groesse des Desktops, die Maus bleibt beim Desktop (DesktopCoordinates).
//
// HDR-Desktop ("HDR verwenden"): IDXGIOutput6::GetDesc1 meldet ColorSpace
// G2084/P2020; die Duplication (IDXGIOutput5::DuplicateOutput1 mit FP16 und
// BGRA 8 Bit) liefert dann FP16 in scRGB. Der Wandler (wandler.rs) bildet
// jedes solche Bild auf der Karte nach SDR ab - relativ zum SDR-Weiss aus
// DisplayConfig, alle 2 s nachgefuehrt - und ersetzt damit die Textur der
// Duplication; ab da laeuft alles wie auf einem SDR-Desktop. Das Format wird
// je Bild geprueft (ein Vollbildprogramm kann auch BGRA liefern), ein
// fremdes ist ein Verlust wie ein Modewechsel.
//
// HDR10 (HDR-Plan 5.2): entscheidet der Host auf HDR (mod.rs hdr_grund - der
// Desktop ist HDR, der Zuschauer will und kann, Kandidat 0 oder 2 und nvenc
// kann PQ), laeuft der Encoder in PQ/BT.2020 und die Aufnahme im Modus PQ:
// der Wandler rechnet jedes Bild (FP16, auch BGRA) auf der Karte in die
// 10-Bit-Ebenen des Encoders (YUV444P16LE bzw. P010), gedreht schon dort;
// eingelesen werden nur noch die Ebenen. Ein Farbwechsel (SDR
// <-> PQ) laeuft wie ein Codecwechsel zwischen zwei Bildern (strom_wechseln:
// Encoder neu, Switch 7 mit dem Transfer, Strominfo 1, Vollbild), hoechstens
// einer je 2 s; das letzte Bild rechnet der Wandler im neuen Modus nach.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use windows::core::{Interface, BOOL, PCWSTR};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ACTIVE,
    ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use super::encoder::{self, Betrieb, Bild, Quelle, Weg};
use super::takt::{Schrittmacher, INFLIGHT_AUFNAHME, INFLIGHT_TAKT};
use super::wandler::{self, Eingang, PqPlan, Wandler};
use super::zeiger::Zeiger;
use super::{log, netz, Z};
use crate::bildschirm::{self, BildschirmEintrag, Bildschirme};
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq)]
pub struct Ausgang {
    /// Platz in der Liste (--output n).
    pub index: usize,
    /// Geraetename (\\.\DISPLAYn) - der Schluessel zu DXGI und GDI.
    pub name: String,
    /// Stabile Kennung (1.3): der zweite Teil der Geraete-ID des Monitors
    /// (MONITOR\ACR0501\{...}\0001 -> ACR0501), bei Gleichheit in der Liste
    /// mit -<Listenplatz>; ohne Geraete-ID der Geraetename. Das merkt sich
    /// der Host (Wunsch, bildschirm.txt), nie den Listenplatz.
    pub kennung: String,
    /// Anzeigename fuer die Liste: der Monitorname aus QueryDisplayConfig,
    /// sonst die DeviceString des Monitors, sonst der Geraetename.
    pub anzeigename: String,
    /// Bildwiederholrate des Anzeigemodus (0 = unbekannt).
    pub hz: u16,
    pub links: i32,
    pub oben: i32,
    pub breite: i32,
    pub hoehe: i32,
    /// Adapter, an dem der Ausgang haengt (Index der DXGI-Aufzaehlung).
    pub karte: usize,
    pub karte_name: String,
    pub nvidia: bool,
    /// Hauptbildschirm laut GDI (MONITORINFOF_PRIMARY), nicht die Lage (0,0).
    pub haupt: bool,
}

impl Ausgang {
    /// Kennung und Name fuers Protokoll: ACR0501 (X27 X1).
    pub fn bezeichnung(&self) -> String {
        format!("{} ({})", self.kennung, self.anzeigename)
    }
}

fn utf16_text(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

fn utf16_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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
                        kennung: String::new(),
                        anzeigename: String::new(),
                        hz: 0,
                        links: r.left,
                        oben: r.top,
                        breite: r.right - r.left,
                        hoehe: r.bottom - r.top,
                        karte: i as usize,
                        karte_name: karte_name.clone(),
                        nvidia: d.VendorId == 0x10de,
                        // Nur der Rueckfall, falls GDI den Ausgang nicht kennt.
                        haupt: r.left == 0 && r.top == 0,
                    });
                }
            }
            j += 1;
        }
        i += 1;
    }
    // Kennung, Name, Bildrate und Hauptbildschirm (1.3) - alles ueber den
    // Geraetenamen verknuepft.
    let gdi = gdi_monitore();
    let namen = anzeigenamen();
    for a in &mut liste {
        if let Some(m) = gdi.iter().find(|m| m.0 == a.name) {
            a.haupt = m.2;
        }
        let (geraete_id, geraete_string) = monitor_geraet(&a.name);
        a.kennung = kennung_aus_geraete_id(geraete_id.as_deref().unwrap_or(""), &a.name);
        let freundlich = namen.iter().find(|n| n.0 == a.name).map(|n| n.1.as_str());
        a.anzeigename = name_waehlen(freundlich, geraete_string.as_deref(), &a.name);
        a.hz = bildrate(&a.name);
    }
    kennungen_eindeutig(&mut liste);
    Ok(liste)
}

/// Geraete-ID und DeviceString des Monitors an einem Ausgang
/// (EnumDisplayDevicesW mit dem Geraetenamen des Ausgangs): der erste
/// aktive Monitor, sonst der erste. Leere Felder sind None.
fn monitor_geraet(name: &str) -> (Option<String>, Option<String>) {
    let wname = utf16_null(name);
    let mut erster: Option<(Option<String>, Option<String>)> = None;
    for i in 0..8u32 {
        let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR(wname.as_ptr()), i, &mut dd, 0) }.as_bool() {
            break;
        }
        let nicht_leer = |s: String| if s.trim().is_empty() { None } else { Some(s) };
        let eintrag = (nicht_leer(utf16_text(&dd.DeviceID)), nicht_leer(utf16_text(&dd.DeviceString)));
        if dd.StateFlags.0 & DISPLAY_DEVICE_ACTIVE.0 != 0 {
            return eintrag;
        }
        erster.get_or_insert(eintrag);
    }
    erster.unwrap_or((None, None))
}

/// Die aktiven Anzeigepfade aus QueryDisplayConfig, je Pfad mit dem
/// Geraetenamen seiner Quelle (\\.\DISPLAYn - der Schluessel zu DXGI und
/// GDI). Leer, wenn Windows nichts liefert. Grundlage fuer Monitornamen und
/// SDR-Weiss - und fuer die Schirmerkennung des Clients (anzeige.rs).
pub(crate) fn anzeigepfade() -> Vec<(String, DISPLAYCONFIG_PATH_INFO)> {
    let mut out = Vec::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut pfade: Vec<DISPLAYCONFIG_PATH_INFO> = (0..np).map(|_| Default::default()).collect();
        let mut modi: Vec<DISPLAYCONFIG_MODE_INFO> = (0..nm).map(|_| Default::default()).collect();
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, pfade.as_mut_ptr(), &mut nm, modi.as_mut_ptr(), None) != ERROR_SUCCESS {
            return out;
        }
        pfade.truncate(np as usize);
        for p in pfade {
            let mut quelle = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            quelle.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut quelle.header) != 0 {
                continue;
            }
            out.push((utf16_text(&quelle.viewGdiDeviceName), p));
        }
    }
    out
}

/// Der Anzeigename am Ziel eines Pfads (monitorFriendlyDeviceName, leer bei
/// manchem eingebauten Panel); None, wenn Windows nichts liefert.
pub(crate) fn ziel_anzeigename(p: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut ziel = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    ziel.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
        adapterId: p.targetInfo.adapterId,
        id: p.targetInfo.id,
    };
    (unsafe { DisplayConfigGetDeviceInfo(&mut ziel.header) } == 0).then(|| utf16_text(&ziel.monitorFriendlyDeviceName))
}

/// Die Monitornamen aus QueryDisplayConfig: je aktivem Pfad (Geraetename
/// der Quelle, Anzeigename des Ziels). Leer, wenn Windows nichts liefert.
fn anzeigenamen() -> Vec<(String, String)> {
    anzeigepfade().into_iter().filter_map(|(quelle, p)| ziel_anzeigename(&p).map(|n| (quelle, n))).collect()
}

/// SDRWhiteLevel am Ziel eines Pfads (1000 = 80 nit): das Weiss, mit dem
/// Windows SDR-Inhalt in einen HDR-Desktop legt (Schieber
/// "SDR-Inhaltshelligkeit"). DisplayConfigGetDeviceInfo(GET_SDR_WHITE_LEVEL);
/// None, wenn Windows ihn nicht nennt.
pub(crate) fn ziel_sdr_weiss(p: &DISPLAYCONFIG_PATH_INFO) -> Option<u32> {
    let mut weiss = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
    weiss.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
        size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
        adapterId: p.targetInfo.adapterId,
        id: p.targetInfo.id,
    };
    (unsafe { DisplayConfigGetDeviceInfo(&mut weiss.header) } == 0).then_some(weiss.SDRWhiteLevel)
}

/// "HDR verwenden" am Ziel eines Pfads laut Advanced Color
/// (GET_ADVANCED_COLOR_INFO): an und nicht bloss erzwungen - erzwungen ist
/// es bei WCG auf einem SDR-Schirm (automatische Farbverwaltung), das ist
/// kein HDR. None, wenn Windows es nicht nennt.
pub(crate) fn ziel_hdr(p: &DISPLAYCONFIG_PATH_INFO) -> Option<bool> {
    let mut farbe = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
    farbe.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
        adapterId: p.targetInfo.adapterId,
        id: p.targetInfo.id,
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut farbe.header) } != 0 {
        return None;
    }
    // Bit 1 advancedColorEnabled, Bit 2 wideColorEnforced.
    let bits = unsafe { farbe.Anonymous.value };
    Some(bits & 0b010 != 0 && bits & 0b100 == 0)
}

/// SDRWhiteLevel des Bildschirms an diesem Ausgang (1000 = 80 nit), am Ziel
/// des ersten Pfads seiner Quelle, der ihn nennt (ziel_sdr_weiss); None, wenn
/// Windows ihn nicht nennt.
pub(crate) fn sdr_weiss_lesen(name: &str) -> Option<u32> {
    anzeigepfade().into_iter().filter(|(quelle, _)| quelle == name).find_map(|(_, p)| ziel_sdr_weiss(&p))
}

/// Bildwiederholrate des laufenden Anzeigemodus eines Ausgangs
/// (EnumDisplaySettingsW, dmDisplayFrequency); 0 und 1 heissen dort
/// "Vorgabe des Geraets" - hier unbekannt (0).
fn bildrate(name: &str) -> u16 {
    let wname = utf16_null(name);
    let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    if !unsafe { EnumDisplaySettingsW(PCWSTR(wname.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() {
        return 0;
    }
    match dm.dmDisplayFrequency {
        0 | 1 => 0,
        hz => hz.min(u16::MAX as u32) as u16,
    }
}

/// Die Kennung (1.3) aus der Geraete-ID des Monitors: der zweite Teil von
/// MONITOR\ACR0501\{4d36e96e-...}\0001 (ACR0501). Ohne brauchbare
/// Geraete-ID gilt der Geraetename (\\.\DISPLAYn). Steuerzeichen fallen weg,
/// hoechstens 64 Byte (bildschirm::kennung_bereinigen).
pub fn kennung_aus_geraete_id(geraete_id: &str, name: &str) -> String {
    let teil = geraete_id.split('\\').filter(|t| !t.trim().is_empty()).nth(1).unwrap_or("");
    let k = bildschirm::kennung_bereinigen(teil.trim());
    if k.is_empty() {
        bildschirm::kennung_bereinigen(name)
    } else {
        k
    }
}

/// Zwei Ausgaenge mit derselben Kennung (zwei gleiche Monitore) bekommen
/// -<Listenplatz> angehaengt, beide - sonst hinge die Kennung am Zufall der
/// Reihenfolge.
pub fn kennungen_eindeutig(liste: &mut [Ausgang]) {
    let doppelt: Vec<String> = liste
        .iter()
        .filter(|a| liste.iter().filter(|b| b.kennung == a.kennung).count() > 1)
        .map(|a| a.kennung.clone())
        .collect();
    for a in liste.iter_mut() {
        if doppelt.contains(&a.kennung) {
            // Der Anhang muss in die 64 Byte passen (gekuerzt an einer
            // Zeichengrenze), sonst blieben beide Kennungen gleich.
            let anhang = format!("-{}", a.index);
            let mut basis = a.kennung.clone();
            while basis.len() + anhang.len() > bildschirm::KENNUNG_MAX {
                basis.pop();
            }
            a.kennung = bildschirm::kennung_bereinigen(&format!("{basis}{anhang}"));
        }
    }
}

/// Der Anzeigename (1.3): der Monitorname aus QueryDisplayConfig, sonst die
/// DeviceString des Monitors, sonst der Geraetename - der erste, der nach
/// dem Bereinigen (bildschirm::name_bereinigen) nicht leer ist.
pub fn name_waehlen(freundlich: Option<&str>, geraete_string: Option<&str>, name: &str) -> String {
    for k in [freundlich, geraete_string, Some(name)] {
        if let Some(k) = k {
            let n = bildschirm::name_bereinigen(k.trim());
            if !n.is_empty() {
                return n;
            }
        }
    }
    String::new()
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
                    "Ausgang {}: {} {}x{} bei ({},{})  an Karte {} ({}){}  Kennung {} ({}), {} Hz",
                    a.index, a.name, a.breite, a.hoehe, a.links, a.oben, a.karte, a.karte_name,
                    if a.haupt { "  (Hauptbildschirm)" } else { "" },
                    a.kennung, a.anzeigename, a.hz
                ));
            }
            *out = liste;
        }
        Err(e) => log(format!("Ausgaenge: {e}")),
    }
}

// ------------------------------------------------------------ Bildschirmwahl

/// Wie das Ziel zustande kam (ziel_waehlen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wahl {
    /// Der gewuenschte Bildschirm ist da und gilt.
    Wunsch,
    /// Es gibt einen Wunsch, aber der Bildschirm ist nicht angeschlossen:
    /// Hauptbildschirm (sonst der erste) als Ausweichplatz.
    Ausweich,
    /// Kein Wunsch: der Hauptbildschirm, sonst der erste.
    Automatik,
}

/// Das Ziel (1.2): der Wunsch, wenn er angeschlossen ist, sonst der
/// Hauptbildschirm, sonst der erste in der Liste, sonst keiner. Rein - ohne
/// DXGI pruefbar.
pub fn ziel_waehlen(liste: &[Ausgang], wunsch: Option<&str>) -> Option<(Ausgang, Wahl)> {
    if let Some(w) = wunsch {
        if let Some(a) = liste.iter().find(|a| a.kennung == w) {
            return Some((a.clone(), Wahl::Wunsch));
        }
    }
    let a = liste.iter().find(|a| a.haupt).or_else(|| liste.first())?;
    Some((a.clone(), if wunsch.is_some() { Wahl::Ausweich } else { Wahl::Automatik }))
}

/// Der Grund eines Bildschirmwechsels fuers Protokoll (1.4 Schritt 7):
/// nach einem neuen Wunsch "Wunsch des Zuschauers" (auf Automatik gestellt:
/// "Wunsch des Zuschauers: Automatik" - dieselben Worte wie beim Mac-Host);
/// ohne neuen Wunsch je nachdem, wie das neue Ziel zustande kam.
pub fn wechsel_grund(neu: Wahl, wunsch_neu: bool) -> &'static str {
    match neu {
        Wahl::Ausweich => "Ausweichplatz",
        Wahl::Automatik if wunsch_neu => "Wunsch des Zuschauers: Automatik",
        _ if wunsch_neu => "Wunsch des Zuschauers",
        Wahl::Wunsch => "zurueck zum gewuenschten Bildschirm",
        Wahl::Automatik => "Hauptbildschirm gewechselt",
    }
}

/// Die Liste fuer Nachricht 12 aus den Ausgaengen: Wunsch (None =
/// Automatik) und je Ausgang Kennung, Name, Groesse, Hz, Hauptbildschirm;
/// `gestreamt` traegt genau der Ausgang mit dieser Kennung.
pub fn als_bildschirme(liste: &[Ausgang], wunsch: Option<&str>, gestreamt: Option<&str>) -> Bildschirme {
    Bildschirme {
        wunsch: wunsch.map(str::to_string),
        eintraege: liste
            .iter()
            .map(|a| BildschirmEintrag {
                kennung: a.kennung.clone(),
                name: a.anzeigename.clone(),
                breite: a.breite.clamp(0, u16::MAX as i32) as u16,
                hoehe: a.hoehe.clamp(0, u16::MAX as i32) as u16,
                hz: a.hz,
                haupt: a.haupt,
                gestreamt: gestreamt == Some(a.kennung.as_str()),
            })
            .collect(),
    }
}

/// Die Liste in Z fuer die Begruessung eines Zuschauers (Nachricht 12 nach
/// Nachricht 11) - der Aufnahmefaden haelt sie danach aktuell.
pub fn bildschirme_setzen(liste: &[Ausgang], wunsch: Option<&str>, gestreamt: Option<&str>) {
    *Z.bildschirme.lock().unwrap_or_else(|e| e.into_inner()) = als_bildschirme(liste, wunsch, gestreamt);
}

/// Der Bildschirmwunsch des Zuschauers (Nachricht 70), bis der
/// Aufnahmefaden ihn zwischen zwei Bildern abholt - wie CODEC_WUNSCH.
/// Aeusseres Some = ein neuer Wunsch liegt vor, inneres None = Automatik.
/// Der letzte Wunsch gewinnt.
static BILDSCHIRM_WUNSCH: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Warum kein Aufnahmefaden laeuft (Konserve als Bildquelle, kein
/// DXGI-Ausgang, kein Encoder): dann holt niemand einen Wunsch aus dem
/// Postfach, und der Eingabefaden beantwortet ihn selbst mit der
/// unveraenderten Liste (2.3) - der Zuschauer wartet sonst 5 s auf einen
/// Wechsel, der nie kommt.
static OHNE_AUFNAHME: OnceLock<&'static str> = OnceLock::new();

/// Vom Start (mod.rs), wenn kein Aufnahmefaden gestartet wird.
pub fn ohne_aufnahme(grund: &'static str) {
    let _ = OHNE_AUFNAHME.set(grund);
}

/// Ein Wunsch vom Eingabekanal (netz::eingabe_lesen): nur vormerken, der
/// Wechsel selbst laeuft im Aufnahmefaden. Ohne Aufnahmefaden geht sofort
/// die unveraenderte Liste zurueck, der Wunsch wird nicht gespeichert.
pub fn bildschirm_wunsch(wunsch: Option<String>) {
    if let Some(grund) = OHNE_AUFNAHME.get() {
        log(format!("Bildschirmwunsch: {} - {grund}, kein Wechsel moeglich", wunsch.as_deref().unwrap_or("Automatik")));
        netz::bildschirme_senden();
        return;
    }
    *BILDSCHIRM_WUNSCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(wunsch);
}

/// Der Aufnahmefaden holt den vorgemerkten Wunsch ab.
pub fn bildschirm_wunsch_abholen() -> Option<Option<String>> {
    BILDSCHIRM_WUNSCH.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Was in bildschirm.txt steht (1.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gespeichert {
    /// Keine Datei: Automatik, ohne Vermerk.
    Fehlt,
    Automatik,
    Kennung(String),
    /// Vorhanden, aber nicht genau eine Zeile "auto" oder eine gueltige
    /// Kennung: Automatik, mit Vermerk - die Datei wird nie still ersetzt.
    Unlesbar,
}

/// bildschirm.txt lesen: genau eine Zeile, "auto" oder die Kennung
/// (Leerzeilen und Zeilenende am Rand sind erlaubt).
pub fn wunsch_laden_aus(pfad: &Path) -> Gespeichert {
    let bytes = match std::fs::read(pfad) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Gespeichert::Fehlt,
        Err(_) => return Gespeichert::Unlesbar,
    };
    let Ok(text) = std::str::from_utf8(&bytes) else { return Gespeichert::Unlesbar };
    let mut zeilen = text.lines().map(str::trim).filter(|z| !z.is_empty());
    let Some(z) = zeilen.next() else { return Gespeichert::Unlesbar };
    if zeilen.next().is_some() {
        return Gespeichert::Unlesbar;
    }
    if z == "auto" {
        return Gespeichert::Automatik;
    }
    if bildschirm::kennung_bereinigen(z) != z {
        return Gespeichert::Unlesbar;
    }
    Gespeichert::Kennung(z.to_string())
}

/// bildschirm.txt schreiben: erst eine temporaere Datei daneben, dann
/// umbenennen - nie eine halbe Zeile.
pub fn wunsch_speichern_nach(pfad: &Path, wunsch: Option<&str>) -> Result<(), String> {
    let zeile = format!("{}\n", wunsch.unwrap_or("auto"));
    let tmp = pfad.with_extension(format!("{}.neu", std::process::id()));
    std::fs::write(&tmp, zeile).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, pfad).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", pfad.display())
    })
}

/// Eine temporaere Datei bildschirm.<pid>.neu, die ein abgestuerzter Lauf
/// zwischen Schreiben und Umbenennen liegen liess, raeumt der naechste
/// Start weg. Liefert, wie viele es waren.
pub fn wunsch_reste_aufraeumen(pfad: &Path) -> usize {
    let Some(dir) = pfad.parent() else { return 0 };
    let Ok(eintraege) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in eintraege.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("bildschirm.") && name.ends_with(".neu") && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Der gespeicherte Wunsch beim Start (None = Automatik), mit Vermerk im
/// Protokoll - auch fuer eine kaputte Datei, die dann liegen bleibt.
pub fn wunsch_laden() -> Option<String> {
    let Some(p) = crate::einstellungen::datei_pfad("bildschirm.txt") else { return None };
    let reste = wunsch_reste_aufraeumen(&p);
    if reste > 0 {
        log(format!("Bildschirmwahl: {reste} liegengebliebene temporaere Datei(en) bildschirm.*.neu entfernt"));
    }
    match wunsch_laden_aus(&p) {
        Gespeichert::Fehlt => None,
        Gespeichert::Automatik => {
            log("Bildschirmwahl: Automatik (bildschirm.txt)");
            None
        }
        Gespeichert::Kennung(k) => {
            log(format!("Bildschirmwahl: {k} (bildschirm.txt)"));
            Some(k)
        }
        Gespeichert::Unlesbar => {
            log("Bildschirmwahl: bildschirm.txt unlesbar - Automatik");
            None
        }
    }
}

/// Den Wunsch speichern (bei jedem neuen Wunsch, 1.5); scheitert das, steht
/// es im Protokoll, der Wunsch gilt trotzdem.
fn wunsch_speichern(wunsch: Option<&str>) {
    let Some(p) = crate::einstellungen::datei_pfad("bildschirm.txt") else {
        log("Bildschirmwahl: kein Ablageordner - Wunsch nicht gespeichert");
        return;
    };
    if let Err(e) = wunsch_speichern_nach(&p, wunsch) {
        log(format!("Bildschirmwahl: Wunsch nicht gespeichert ({e})"));
    }
}

/// Was eine Neubewertung ergab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bewertung {
    /// Das Ziel ist ein anderer Bildschirm als vorher (auch von oder nach
    /// "keiner"): eine laufende Duplication faellt, der Aufbau nimmt das
    /// neue Ziel.
    gewechselt: bool,
    /// Nachricht 12 ist faellig (Liste, Ziel oder Wunsch geaendert).
    melden: bool,
    /// Der Grund im Protokoll, wenn von einem Bildschirm auf einen anderen
    /// gewechselt wurde.
    grund: Option<&'static str>,
}

/// Was der Aufnahmefaden ueber die Bildschirme weiss - der Zustand der
/// Betriebsart (1.1): Wunsch, letzte Liste, Ziel. Lebt ueber die Sitzungen
/// hinweg im Aufnahmefaden; ohne Zuschauer wird er nur nachgefuehrt.
pub struct Bildschirmstand {
    /// Kennung des gewuenschten Bildschirms, None = Automatik.
    wunsch: Option<String>,
    /// Die zuletzt aufgezaehlten Ausgaenge.
    liste: Vec<Ausgang>,
    /// Das bestimmte Ziel - gestreamt, sobald seine Duplication steht.
    ziel: Option<Ausgang>,
    wahl: Option<Wahl>,
    /// Wann die Ausgaenge das naechste Mal aufgezaehlt werden (alle 2 s).
    naechste_pruefung: Instant,
    /// Eingabeweg des Encoders von der Befehlszeile, und die Entscheidung
    /// daraus (encoder::weg_entscheiden) fuer den Ausgang `weg_fuer` - ein
    /// anderer Ausgang bekommt eine eigene.
    weg_cli: Weg,
    weg: Weg,
    weg_fuer: Option<String>,
    /// Nachricht 12 nach einem Wechsel bei laufendem Strom steht noch aus:
    /// sie geht erst nach dem Aufbau des neuen Stroms hinaus (1.4 Schritt
    /// 6), der Aufnahmefaden holt sie nach (meldung_nachholen).
    meldung_offen: bool,
    /// Der zuletzt gemeldete Fehler beim Aufzaehlen der Ausgaenge (einmal
    /// im Protokoll, die letzte Liste gilt solange weiter); leer = keiner.
    aufzaehl_fehler: String,
}

/// Abstand zweier Aufzaehlungen der Ausgaenge im Betrieb.
const PRUEFTAKT: Duration = Duration::from_secs(2);

impl Bildschirmstand {
    /// Der Stand beim Start: Wunsch (bildschirm.txt bzw. --output), die
    /// Liste vom Start und die Wegentscheidung fuer das erste Ziel.
    pub fn neu(wunsch: Option<String>, liste: Vec<Ausgang>, weg_cli: Weg, weg: Weg) -> Bildschirmstand {
        let (ziel, wahl) = match ziel_waehlen(&liste, wunsch.as_deref()) {
            Some((a, w)) => (Some(a), Some(w)),
            None => (None, None),
        };
        let weg_fuer = ziel.as_ref().map(|a| a.kennung.clone());
        Bildschirmstand { wunsch, liste, ziel, wahl, naechste_pruefung: Instant::now() + PRUEFTAKT, weg_cli, weg, weg_fuer, meldung_offen: false, aufzaehl_fehler: String::new() }
    }

    /// Das Ziel, dessen Duplication der Aufnahmefaden aufbaut.
    fn ziel(&self) -> Option<Ausgang> {
        self.ziel.clone()
    }

    /// Die naechste Aufzaehlung sofort (nach einem Verlust, vor einem Aufbau).
    fn pruefung_faellig(&mut self) {
        self.naechste_pruefung = Instant::now();
    }

    /// Ein neuer Encoderweg, wenn `a` ein anderer Ausgang ist als der, fuer
    /// den die Entscheidung fiel (der Weg hing sonst am Startausgang); fuer
    /// denselben Ausgang None - die Sitzung behaelt ihren Weg, auch einen
    /// abgesenkten.
    fn weg_neu_fuer(&mut self, a: &Ausgang) -> Option<Weg> {
        if self.weg_fuer.as_deref() == Some(a.kennung.as_str()) {
            return None;
        }
        self.weg = encoder::weg_entscheiden(self.weg_cli, a.index);
        self.weg_fuer = Some(a.kennung.clone());
        Some(self.weg)
    }

    /// Wunsch abholen, alle 2 s (oder sofort nach pruefung_faellig) die
    /// Ausgaenge neu aufzaehlen und bewerten; Nachricht 12 bei Aenderung -
    /// nach einem Wechsel bei laufendem Strom (`strom_laeuft`: die
    /// Duplication steht) erst nach dem Aufbau des neuen, wie 1.4 es reiht
    /// (meldung_nachholen im Aufnahmefaden).
    fn nachfuehren(&mut self, strom_laeuft: bool) -> Bewertung {
        let wunsch_neu = bildschirm_wunsch_abholen();
        if wunsch_neu.is_none() && Instant::now() < self.naechste_pruefung {
            return Bewertung::default();
        }
        self.naechste_pruefung = Instant::now() + PRUEFTAKT;
        if let Some(w) = wunsch_neu.as_ref() {
            self.wunsch = w.clone();
            wunsch_speichern(self.wunsch.as_deref());
        }
        let liste = self.liste_aufgezaehlt(ausgaenge());
        let alt = self.ziel.as_ref().map(|a| a.bezeichnung());
        let b = self.bewerten_mit(liste, wunsch_neu.is_some());
        if let (Some(g), Some(alt), Some(neu)) = (b.grund, alt, self.ziel.as_ref()) {
            log(format!(
                "Bildschirmwechsel: {alt} -> {} ({g}){}",
                neu.bezeichnung(),
                if strom_laeuft { "" } else { " - ohne laufenden Strom nur das Ziel, gilt ab dem naechsten Aufbau" }
            ));
        }
        if self.meldung_faellig(b, strom_laeuft) {
            self.liste_melden();
        }
        b
    }

    /// Eine frische Aufzaehlung uebernehmen. Scheitert sie (DXGI-Factory,
    /// Adapterbeschreibung), gilt die letzte Liste weiter: ein
    /// voruebergehender Fehler ist kein leerer Bildschirmsatz und wirft
    /// keinen gesunden Strom weg. Der Fehler steht einmal im Protokoll, die
    /// naechste gute Liste vergisst ihn.
    fn liste_aufgezaehlt(&mut self, ergebnis: Result<Vec<Ausgang>, String>) -> Vec<Ausgang> {
        match ergebnis {
            Ok(l) => {
                if !self.aufzaehl_fehler.is_empty() {
                    self.aufzaehl_fehler.clear();
                    log("Ausgaenge: wieder aufzaehlbar");
                }
                l
            }
            Err(e) => {
                if self.aufzaehl_fehler != e {
                    log(format!("Ausgaenge: {e} - die letzte Liste gilt weiter"));
                    self.aufzaehl_fehler = e;
                }
                self.liste.clone()
            }
        }
    }

    /// Geht Nachricht 12 jetzt hinaus? Nach einem Wechsel bei laufendem
    /// Strom nicht: sie wartet auf den Aufbau des neuen (1.4 Schritt 6,
    /// nach Switch 7 und Info 1), sonst bei jeder Aenderung - und holt
    /// dabei eine noch offene mit nach.
    fn meldung_faellig(&mut self, b: Bewertung, strom_laeuft: bool) -> bool {
        if b.gewechselt && strom_laeuft {
            self.meldung_offen = true;
            return false;
        }
        b.melden || std::mem::take(&mut self.meldung_offen)
    }

    /// Die nach einem Wechsel zurueckgestellte Nachricht 12 - nach dem
    /// Aufbau des neuen Stroms, auch einem gescheiterten (dann steht das
    /// Ziel als gestreamt in der Liste, 2.2: "Ziel bestimmt").
    fn meldung_nachholen(&mut self) {
        if std::mem::take(&mut self.meldung_offen) {
            self.liste_melden();
        }
    }

    /// Neu bewerten (1.2) mit dieser Liste: Ziel = Wunsch, sonst
    /// Hauptbildschirm, sonst der erste. Rein bis auf die Zeilen zum Wunsch -
    /// so ohne DXGI pruefbar.
    fn bewerten_mit(&mut self, liste: Vec<Ausgang>, wunsch_neu: bool) -> Bewertung {
        let liste_geaendert = liste != self.liste;
        let (ziel, wahl) = match ziel_waehlen(&liste, self.wunsch.as_deref()) {
            Some((a, w)) => (Some(a), Some(w)),
            None => (None, None),
        };
        let gewechselt = ziel.as_ref().map(|a| &a.kennung) != self.ziel.as_ref().map(|a| &a.kennung);
        if wunsch_neu {
            match (self.wunsch.as_deref(), wahl) {
                (Some(w), Some(Wahl::Ausweich)) => log(format!(
                    "Bildschirmwunsch: {w} - nicht angeschlossen, Ausweichplatz {}",
                    ziel.as_ref().map(|a| a.bezeichnung()).unwrap_or_else(|| "keiner".into())
                )),
                (Some(w), _) => log(format!("Bildschirmwunsch: {w}{}", if gewechselt { "" } else { " (laeuft bereits)" })),
                (None, _) => log(format!("Bildschirmwunsch: Automatik{}", if gewechselt { "" } else { " (laeuft bereits)" })),
            }
        }
        // Der Grund gilt nur fuer einen Wechsel von einem Bildschirm auf
        // einen anderen (nicht von oder nach "keiner"); die Zeile dazu
        // schreibt nachfuehren.
        let grund = match (gewechselt, self.ziel.is_some() && ziel.is_some(), wahl) {
            (true, true, Some(w)) => Some(wechsel_grund(w, wunsch_neu)),
            _ => None,
        };
        self.liste = liste;
        self.ziel = ziel;
        self.wahl = wahl;
        Bewertung { gewechselt, melden: liste_geaendert || gewechselt || wunsch_neu, grund }
    }

    /// Den Stand nach Z (fuer die Begruessung) und als Nachricht 12 an den
    /// Zuschauer.
    fn liste_melden(&self) {
        bildschirme_setzen(&self.liste, self.wunsch.as_deref(), self.ziel.as_ref().map(|a| a.kennung.as_str()));
        netz::bildschirme_senden();
    }
}

/// Groesse des Stroms aus der Bildgroesse (dw x dh, der Desktop so, wie er
/// steht - bei einem gedrehten Ausgang also gedreht): nativ, genau diese
/// Pixel, auch ab 3840 Breite - nie halbiert (Entscheidung vom 29.09.2026:
/// "QC soll nativ uebertragen"). Immer gerade: ein ungerader Rand wird
/// abgeschnitten, nicht skaliert. Der Windows-Host kennt keine Vorgabe
/// der Stromgroesse (--out gibt es nur beim Mac-Host); kleiner wird der
/// Strom nur ueber eine kleinere Aufloesung des Ausgangs.
pub fn stromplan(dw: i32, dh: i32) -> (i32, i32) {
    (dw.max(0) & !1, dh.max(0) & !1)
}

/// Vorlaeufige Stromgroesse fuer einen Ausgang (beim Start, vor der
/// Duplication); danach gilt der Anzeigemodus der Duplication.
pub fn stromgroesse(a: &Ausgang) -> (i32, i32) {
    stromplan(a.breite, a.hoehe)
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

    /// Woher der Desktoppunkt (x, y) in der Oberflaeche sw x sh kommt
    /// (dasselbe rechnet der Wandler im Modus PQ auf der Karte).
    pub(crate) fn quelle(self, sw: usize, sh: usize, x: usize, y: usize) -> (usize, usize) {
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
        liste.push((utf16_text(&info.szDevice), info.monitorInfo.rcMonitor, (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0));
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

/// Was DXGI (IDXGIOutput6::GetDesc1) und DisplayConfig ueber die Farbe
/// eines Ausgangs sagen.
#[derive(Clone, Copy, Debug)]
pub struct Farblage {
    /// "HDR verwenden" ist an: ColorSpace G2084/P2020. Die Duplication
    /// liefert dann FP16 (scRGB), der Wandler macht SDR daraus.
    pub hdr: bool,
    /// GetDesc1 war zu haben (IDXGIOutput6, ab Windows 10 1703).
    pub desc1: bool,
    pub farbraum: DXGI_COLOR_SPACE_TYPE,
    pub bits: u32,
    pub spitze_nit: f32,
    pub vollbild_nit: f32,
    pub min_nit: f32,
    /// SDRWhiteLevel laut DisplayConfig (1000 = 80 nit), None = unbekannt.
    pub sdr_weiss: Option<u32>,
}

impl Farblage {
    /// Fuers Protokoll: Farbraum, Bits, Helligkeiten, SDR-Weiss.
    pub fn zeile(&self) -> String {
        let weiss = match self.sdr_weiss {
            Some(l) => format!("SDR-Weiss {:.0} nit ({l})", wandler::sdr_weiss_nit(l)),
            None => "SDR-Weiss unbekannt".into(),
        };
        if !self.desc1 {
            return format!("keine Farbangaben (IDXGIOutput6 fehlt), {weiss}");
        }
        format!(
            "{} (ColorSpace {}, {} Bit, Spitze {:.0} nit, Vollbild {:.0} nit, min {:.4} nit), {weiss}",
            if self.hdr { "HDR" } else { "SDR" },
            self.farbraum.0, self.bits, self.spitze_nit, self.vollbild_nit, self.min_nit
        )
    }
}

/// Die HDR10-Metadaten eines HDR-Desktops fuer Strominfo und Encoder: SDR-Weiss
/// in nit (aus SDRWhiteLevel, ohne Angabe 80 nit), Mastering max in nit und
/// min in 0,0001 nit aus GetDesc1 - ohne Angabe 1000 nit und 0,005 nit wie
/// beim Mac-Host. Auf u16 gerundet und geklemmt.
pub fn hdr_metadaten(f: &Farblage) -> (u16, encoder::Mastering) {
    let u16_aus = |v: f32| -> u16 { if v.is_finite() { v.round().clamp(0.0, u16::MAX as f32) as u16 } else { 0 } };
    let weiss = u16_aus(wandler::sdr_weiss_nit(wandler::sdr_weiss_pruefen(f.sdr_weiss)));
    let max_nit = if f.spitze_nit >= 1.0 { u16_aus(f.spitze_nit) } else { encoder::Mastering::VORGABE.max_nit };
    let min = u16_aus(f.min_nit * 10_000.0);
    let min_zehntausendstel = if f.min_nit > 0.0 && min > 0 { min } else { encoder::Mastering::VORGABE.min_zehntausendstel };
    (weiss, encoder::Mastering { max_nit, min_zehntausendstel })
}

/// Die Farblage eines Ausgangs (Geraetename fuer DisplayConfig).
fn farblage(output: &IDXGIOutput, name: &str) -> Farblage {
    let d = output.cast::<IDXGIOutput6>().ok().and_then(|o| unsafe { o.GetDesc1() }.ok());
    Farblage {
        hdr: d.is_some_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020),
        desc1: d.is_some(),
        farbraum: d.map(|d| d.ColorSpace).unwrap_or_default(),
        bits: d.map(|d| d.BitsPerColor).unwrap_or(0),
        spitze_nit: d.map(|d| d.MaxLuminance).unwrap_or(0.0),
        vollbild_nit: d.map(|d| d.MaxFullFrameLuminance).unwrap_or(0.0),
        min_nit: d.map(|d| d.MinLuminance).unwrap_or(0.0),
        sdr_weiss: sdr_weiss_lesen(name),
    }
}

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
    /// Format der eigenen Texturen und der Bilder aus `abholen`: das des
    /// Desktops, auf einem HDR-Desktop B8G8R8A8_UNORM (der Wandler liefert
    /// SDR).
    pub format: DXGI_FORMAT,
    /// Format des Desktops laut Duplication (ModeDesc).
    pub desktop_format: DXGI_FORMAT,
    pub farbe: Farblage,
    pub im_systemspeicher: bool,
    /// Noch kein Bild abgeholt: das erste Abholen liefert den ganzen
    /// Desktop, auch wenn seit dem Aufbau nichts praesentiert wurde
    /// (LastPresentTime 0) - sonst bliebe ein stiller Desktop minutenlang
    /// ohne erstes Bild.
    erstes_offen: Cell<bool>,
    /// Geraetename des Ausgangs (\\.\DISPLAYn), fuer das SDR-Weiss.
    geraetename: String,
    /// Wandler fuer FP16-Bilder: steht nach dem Aufbau eines HDR-Desktops,
    /// sonst entsteht er mit dem ersten FP16-Bild.
    wandler: RefCell<Option<Wandler>>,
    /// Naechste Pruefung des SDR-Weiss (alle 2 s, solange ein Wandler steht).
    weiss_pruefung: Cell<Instant>,
    /// Format des letzten Bildes und wie oft es gewechselt hat (Protokoll).
    bildformat: Cell<Option<DXGI_FORMAT>>,
    formatwechsel: Cell<u32>,
    /// Modus PQ (HDR10-Strom): der Plan der Ebenen, nach dem der Wandler
    /// jedes Bild rechnet; None = Bildweg BGRA wie bisher.
    pq: Cell<Option<PqPlan>>,
}

/// Eigener Fehlercode: ein Bild, das der Bildweg nicht nehmen kann (fremdes
/// Format, Wandler gescheitert). Kundenbit gesetzt - kein Code von Windows;
/// den Grund haelt GRUND_BILD fest, dxgi_fehler_text setzt ihn ein.
const BILD_UNBRAUCHBAR: windows::core::HRESULT = windows::core::HRESULT(0xA051_0001u32 as i32);

thread_local! {
    static GRUND_BILD: RefCell<String> = const { RefCell::new(String::new()) };
}

fn bild_unbrauchbar(grund: String) -> windows::core::Error {
    GRUND_BILD.with(|g| *g.borrow_mut() = grund);
    windows::core::Error::from_hresult(BILD_UNBRAUCHBAR)
}

/// Klartext zu den DXGI-Fehlern, die hier vorkommen.
pub fn dxgi_fehler_text(e: &windows::core::Error) -> String {
    let c = e.code();
    if c == BILD_UNBRAUCHBAR {
        format!("Bild der Duplication nicht verwendbar: {}", GRUND_BILD.with(|g| g.borrow().clone()))
    } else if c == DXGI_ERROR_UNSUPPORTED {
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
/// IDXGIOutput5::DuplicateOutput1 - auf einem HDR-Desktop mit FP16 und BGRA
/// 8 Bit, sonst nur mit BGRA 8 Bit (wie DuplicateOutput); ohne
/// IDXGIOutput5, oder wenn DuplicateOutput1 scheitert, DuplicateOutput. Ein
/// HDR-Desktop (FP16) bekommt gleich den Wandler nach SDR; andere Formate
/// als BGRA 8 Bit und FP16 werden abgelehnt.
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
    let farbe = farblage(&output, &ausgang.name);
    // Auf einem HDR-Desktop FP16 zuerst: das ist, was der Desktop wirklich
    // traegt (scRGB), die Abbildung nach SDR macht der Wandler - nicht eine
    // Umrechnung von Windows, deren Weiss niemand kennt. BGRA bleibt in der
    // Liste fuer Vollbildprogramme, die 8 Bit praesentieren.
    let formate: &[DXGI_FORMAT] = if farbe.hdr {
        &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM]
    } else {
        &[DXGI_FORMAT_B8G8R8A8_UNORM]
    };
    let alt = || unsafe { output1.DuplicateOutput(&device) };
    let dup = match output.cast::<IDXGIOutput5>() {
        Ok(o5) => match unsafe { o5.DuplicateOutput1(&device, 0, formate) } {
            Ok(d) => d,
            Err(e1) => match alt() {
                Ok(d) => {
                    log(format!("DuplicateOutput1: {} - Aufnahme ueber DuplicateOutput", dxgi_fehler_text(&e1)));
                    d
                }
                Err(e) => return Err(format!("DuplicateOutput: {} (DuplicateOutput1: {})", dxgi_fehler_text(&e), dxgi_fehler_text(&e1))),
            },
        },
        Err(_) => alt().map_err(|e| format!("DuplicateOutput: {}", dxgi_fehler_text(&e)))?,
    };
    let desc: DXGI_OUTDUPL_DESC = unsafe { dup.GetDesc() };
    let desktop_format = desc.ModeDesc.Format;
    // Die eigenen Texturen nehmen BGRA 8 Bit - auf einem HDR-Desktop das,
    // was der Wandler liefert.
    let format = match wandler::eingang_art(desktop_format) {
        Eingang::Bgra => desktop_format,
        Eingang::Fp16 => DXGI_FORMAT_B8G8R8A8_UNORM,
        Eingang::Fremd => return Err(format!("Desktopformat {} - weder BGRA 8 Bit noch FP16 (HDR)", wandler::format_name(desktop_format))),
    };
    let wandler = if farbe.hdr || desktop_format == DXGI_FORMAT_R16G16B16A16_FLOAT {
        Some(Wandler::neu(&device, &ctx, wandler::sdr_weiss_pruefen(farbe.sdr_weiss))?)
    } else {
        None
    };
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
        desktop_format,
        farbe,
        im_systemspeicher: desc.DesktopImageInSystemMemory.as_bool(),
        erstes_offen: Cell::new(true),
        geraetename: ausgang.name.clone(),
        wandler: RefCell::new(wandler),
        weiss_pruefung: Cell::new(Instant::now() + Duration::from_secs(2)),
        bildformat: Cell::new(None),
        formatwechsel: Cell::new(0),
        pq: Cell::new(None),
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
    /// Ein neues Bild: Textur der Duplication (bis ReleaseFrame gueltig) -
    /// bei einem FP16-Bild die SDR-Textur des Wandlers (bis zum naechsten
    /// Abholen gueltig), immer im BGRA-Format der eigenen Texturen -,
    /// Praesentationszeit (QPC) und die Zeigerangaben.
    Bild { textur: ID3D11Texture2D, praesentiert_qpc: i64, zeiger: ZeigerInfo },
}

impl Duplication {
    /// Groesse des Desktops (gedreht wie er steht) - daraus der Strom.
    pub fn desktop_groesse(&self) -> (u32, u32) {
        self.drehung.groesse(self.breite, self.hoehe)
    }

    /// Das Format fuers Protokoll: das der eigenen Texturen, auf einem
    /// HDR-Desktop dazu das des Desktops.
    pub fn format_text(&self) -> String {
        if self.desktop_format == self.format {
            wandler::format_name(self.format)
        } else {
            format!("{} (Desktop {}, gewandelt nach SDR)", wandler::format_name(self.format), wandler::format_name(self.desktop_format))
        }
    }

    /// Kann dieser Desktop HDR10 liefern? "HDR verwenden" ist an UND die
    /// Duplication liefert FP16 - ueber DuplicateOutput (ohne IDXGIOutput5)
    /// kaeme nur das, was Windows selbst nach SDR abbildet.
    pub fn quelle_hdr(&self) -> bool {
        self.farbe.hdr && wandler::eingang_art(self.desktop_format) == Eingang::Fp16
    }

    /// Der Plan des Modus PQ (None: Bildweg BGRA).
    pub fn pq(&self) -> Option<PqPlan> {
        self.pq.get()
    }

    /// Den Wandler anlegen, falls noch keiner steht (erstes FP16-Bild ohne
    /// erkannten HDR-Desktop, oder Modus PQ auf einem BGRA-Desktop).
    fn wandler_sichern(&self) -> Result<(), String> {
        if self.wandler.borrow().is_some() {
            return Ok(());
        }
        let weiss = wandler::sdr_weiss_pruefen(sdr_weiss_lesen(&self.geraetename));
        *self.wandler.borrow_mut() = Some(Wandler::neu(&self.device, &self.ctx, weiss)?);
        Ok(())
    }

    /// Modus PQ an (mit diesem Plan) oder aus. Der Wandler entsteht, falls
    /// noch keiner steht.
    fn pq_setzen(&self, plan: Option<PqPlan>) -> Result<(), String> {
        if plan.is_some() {
            self.wandler_sichern()?;
        }
        self.pq.set(plan);
        Ok(())
    }

    /// Nach einem Wechsel in den Modus PQ: das Bild, das der Wandler noch
    /// haelt, nach dem Plan rechnen. false, wenn er keines haelt.
    fn pq_nachrechnen(&self) -> Result<bool, String> {
        let Some(plan) = self.pq.get() else { return Ok(false) };
        match self.wandler.borrow_mut().as_mut() {
            Some(w) => w.pq_rechnen(&plan),
            None => Ok(false),
        }
    }

    /// Die Ebenen des zuletzt gerechneten Bildes (dicht gepackt, u16).
    fn pq_auslesen(&self, ziel: &mut Vec<u8>) -> Result<(), String> {
        match self.wandler.borrow().as_ref() {
            Some(w) => w.pq_auslesen(ziel),
            None => Err("Modus PQ ohne Wandler".into()),
        }
    }

    /// Nach einem Wechsel zurueck nach SDR: das Bild, das der Wandler noch
    /// haelt, als BGRA (in Oberflaechengroesse); None, wenn er keines haelt.
    fn sdr_nachrechnen(&self) -> Result<Option<ID3D11Texture2D>, String> {
        match self.wandler.borrow_mut().as_mut() {
            Some(w) => w.sdr_rechnen().map_err(|e| fehler("Wandler SDR", e)),
            None => Ok(None),
        }
    }

    /// Alle 2 s, solange ein Wandler steht: das SDR-Weiss neu lesen (der
    /// Schieber "SDR-Inhaltshelligkeit" aendert es ohne Modewechsel). Vor
    /// AcquireNextFrame, nie zwischen Abholen und Freigeben. Das neue Weiss
    /// geht auch in die Strominfo (Z.hdr_weiss_nit) - laeuft der Strom in
    /// PQ, bekommt der Zuschauer gleich eine neue (der Client gleicht sein
    /// SDR-Weiss daran an).
    fn sdr_weiss_nachfuehren(&self) {
        let mut w = self.wandler.borrow_mut();
        let Some(w) = w.as_mut() else { return };
        let jetzt = Instant::now();
        if jetzt < self.weiss_pruefung.get() {
            return;
        }
        self.weiss_pruefung.set(jetzt + Duration::from_secs(2));
        let neu = wandler::sdr_weiss_pruefen(sdr_weiss_lesen(&self.geraetename));
        if neu != w.sdr_weiss() {
            log(format!(
                "HDR-Desktop {}: SDR-Weiss {:.0} -> {:.0} nit",
                self.geraetename,
                wandler::sdr_weiss_nit(w.sdr_weiss()),
                wandler::sdr_weiss_nit(neu)
            ));
            w.sdr_weiss_setzen(neu);
            if self.quelle_hdr() {
                let nit = wandler::sdr_weiss_nit(neu).round() as u32;
                if Z.hdr_weiss_nit.swap(nit, Ordering::Relaxed) != nit && Z.farbe.load(Ordering::Relaxed) == crate::hdr::TRANSFER_PQ {
                    netz::strominfo_senden();
                }
            }
        }
    }

    /// Das Bild so, wie der Bildweg es nimmt (BGRA 8 Bit): BGRA unveraendert,
    /// FP16 (HDR-Desktop) durch den Wandler nach SDR. Geprueft wird je Bild -
    /// ein Vollbildprogramm kann auf einem HDR-Desktop auch BGRA liefern;
    /// ein fremdes Format ist ein Verlust (Neuaufbau wie nach einem
    /// Modewechsel). Im Modus PQ geht jedes Bild (FP16 oder BGRA) durch den
    /// Wandler in die Ebenen; zurueck kommt dann das Bild der Duplication
    /// selbst (fuer die Groessenpruefung) - in den Bildweg kopiert wird es
    /// nicht mehr, eingelesen werden die Ebenen.
    fn bild_fuer_den_bildweg(&self, t: ID3D11Texture2D) -> Result<ID3D11Texture2D, windows::core::Error> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { t.GetDesc(&mut d) };
        let art = wandler::eingang_art(d.Format);
        if let Some(plan) = self.pq.get() {
            self.bildformat_melden(d.Format, art);
            if art == Eingang::Fremd {
                return Err(bild_unbrauchbar(format!("Format {} ist weder BGRA 8 Bit noch FP16", wandler::format_name(d.Format))));
            }
            self.wandler_sichern().map_err(|e| bild_unbrauchbar(format!("Modus PQ ohne Wandler ({e})")))?;
            let mut w = self.wandler.borrow_mut();
            let Some(w) = w.as_mut() else { return Err(bild_unbrauchbar("Modus PQ ohne Wandler".into())) };
            w.aufnehmen(&t)?;
            if !w.pq_rechnen(&plan).map_err(|e| bild_unbrauchbar(format!("Wandler PQ: {e}")))? {
                return Err(bild_unbrauchbar("Wandler PQ: kein Bild aufgenommen".into()));
            }
            return Ok(t);
        }
        if art == Eingang::Fp16 {
            // FP16, ohne dass der Aufbau HDR erkannt hatte: der Wandler jetzt.
            self.wandler_sichern().map_err(|e| bild_unbrauchbar(format!("FP16 ohne Wandler ({e})")))?;
        }
        self.bildformat_melden(d.Format, art);
        match art {
            Eingang::Bgra => {
                // Das neueste Bild geht am Wandler vorbei: was er haelt, ist
                // fuer einen Farbwechsel nicht mehr das neueste.
                if let Some(w) = self.wandler.borrow_mut().as_mut() {
                    w.eingang_vergessen();
                }
                Ok(t)
            }
            Eingang::Fp16 => match self.wandler.borrow_mut().as_mut() {
                Some(w) => w.nach_sdr(&t),
                None => Err(bild_unbrauchbar("FP16 ohne Wandler".into())),
            },
            Eingang::Fremd => Err(bild_unbrauchbar(format!("Format {} ist weder BGRA 8 Bit noch FP16", wandler::format_name(d.Format)))),
        }
    }

    /// Das Format der Bilder ins Protokoll, wenn es sich aendert - das erste
    /// nur, wenn es nicht das angekuendigte BGRA ist. Nach acht Wechseln
    /// schweigt es.
    fn bildformat_melden(&self, f: DXGI_FORMAT, art: Eingang) {
        let vorher = self.bildformat.replace(Some(f));
        if vorher == Some(f) || (vorher.is_none() && art == Eingang::Bgra && f == self.desktop_format) {
            return;
        }
        let n = self.formatwechsel.get();
        self.formatwechsel.set(n + 1);
        if n < 8 {
            let was = match (art, self.pq.get()) {
                (Eingang::Fremd, _) => " - nicht verwendbar".into(),
                (_, Some(p)) => format!(" - Wandler nach PQ (HDR10, {})", p.text()),
                (Eingang::Bgra, None) => String::new(),
                (Eingang::Fp16, None) => format!(
                    " (scRGB) - Wandler nach SDR, SDR-Weiss {:.0} nit",
                    wandler::sdr_weiss_nit(self.wandler.borrow().as_ref().map(|w| w.sdr_weiss()).unwrap_or(wandler::SDR_WEISS_VORGABE))
                ),
            };
            log(format!("Aufnahme {}: Bilder im Format {}{was}", self.geraetename, wandler::format_name(f)));
        } else if n == 8 {
            log(format!("Aufnahme {}: das Bildformat wechselt oft - weitere Wechsel ohne Protokollzeile", self.geraetename));
        }
    }

    /// AcquireNextFrame mit Frist in ms. Der Aufrufer muss nach `Bild`
    /// SOFORT kopieren und `freigeben` rufen.
    pub fn abholen(&self, frist_ms: u32) -> Result<Abholung, windows::core::Error> {
        self.sdr_weiss_nachfuehren();
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
        // Gleich nach AcquireNextFrame: ein FP16-Bild kopiert der Wandler
        // jetzt, der Aufrufer bekommt sein SDR-Bild.
        let textur = match self.bild_fuer_den_bildweg(textur) {
            Ok(t) => t,
            Err(e) => {
                self.freigeben();
                return Err(e);
            }
        };
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

/// Der Aufnahmefaden. `wunsch` ist die Kennung des gewuenschten Bildschirms
/// (None = Automatik), `liste` die Ausgaenge vom Start, `weg_cli` der
/// Eingabeweg des Encoders von der Befehlszeile und `weg` die Entscheidung
/// daraus fuer das erste Ziel.
pub fn start(wunsch: Option<String>, liste: Vec<Ausgang>, weg_cli: Weg, weg: Weg) {
    let mut stand = Bildschirmstand::neu(wunsch, liste, weg_cli, weg);
    bildschirme_setzen(&stand.liste, stand.wunsch.as_deref(), stand.ziel.as_ref().map(|a| a.kennung.as_str()));
    std::thread::Builder::new()
        .name("quadchroma-aufnahme".into())
        .spawn(move || {
            // Was der Encoder ueber FFmpeg sagt, gehoert der Host-Rolle
            // (protokoll in main.rs: Herkunft des Fadens).
            crate::protokoll::herkunft_setzen(crate::protokoll::Herkunft::Host);
            loop {
                while !netz::zuschauer_da() {
                    std::thread::sleep(Duration::from_millis(100));
                    // Ohne Zuschauer nur den Stand nachfuehren (Wunsch, Liste,
                    // Ziel); der Strom laeuft erst mit dem naechsten (1.2).
                    stand.nachfuehren(false);
                }
                // Der naechste Zuschauer ist einer mit anderer Nummer - auch wenn
                // er den jetzigen ohne Luecke abloest und zuschauer_da() dabei
                // nie false wird. Gemerkt vor der Sitzung: wer schon waehrend
                // der abgestuerzten Sitzung abgeloest hat, bekommt gleich einen
                // neuen Anlauf.
                let nr = netz::zuschauer_nr();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sitzung(&mut stand)));
                if r.is_err() {
                    // Wachhalten und Timerperiode hat Drop schon abgebaut.
                    log("Aufnahme: Faden abgestuerzt - neuer Anlauf mit dem naechsten Zuschauer");
                    // Dem Zuschauer sagen, dass kein Bild kommt - statt eines
                    // stummen, stehenden Bildes. Nur dem, fuer den die Sitzung
                    // lief: hat inzwischen ein anderer abgeloest, bekommt der
                    // gleich einen neuen Anlauf (die Schleife unten endet
                    // sofort), und dessen Sitzung schickt 0 nur nach einem
                    // eigenen Verlust - mit 1 saehe er dauerhaft "kein
                    // Bildschirm" ueber dem laufenden Bild.
                    netz::hoststatus_senden_an(nr, 1);
                    while netz::zuschauer_da() && netz::zuschauer_nr() == nr {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    // Ein Testbild ueberlebt den Zuschauer auch hier nicht.
                    if Z.testbild.swap(false, Ordering::Relaxed) {
                        log("Testbild aus (Zuschauer weg)");
                    }
                }
            }
        })
        .ok();
}

/// Bild und Maus auf denselben Ausgang; die Stromgroesse kommt aus dem
/// Anzeigemodus der Duplication (dw x dh) - das ist die Groesse der Bilder,
/// die wirklich ankommen - und steht danach in Z. Liefert die Stromgroesse
/// und ob sie sich geaendert hat.
fn strom_anpassen(a: &Ausgang, dw: i32, dh: i32) -> ((i32, i32), bool) {
    let (w, h) = stromplan(dw, dh);
    super::eingabe::ausgang_setzen(a.links, a.oben, a.breite, a.hoehe);
    let alt = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);
    Z.info_w.store(w as u32, Ordering::Relaxed);
    Z.info_h.store(h as u32, Ordering::Relaxed);
    ((w, h), alt != (w, h))
}

/// Der Encoderweg fuer eine neue Aufnahme: der Null-Kopien-Weg (d3d11)
/// kennt keine Drehung - ein gedrehter Ausgang nimmt dann den Prozessorweg
/// (bgra, gedreht wird beim Einlesen). Die Groesse spielt keine Rolle mehr:
/// frueher schickte ein Ausgang ab 3840 Breite ihn ebenfalls auf den
/// Prozessor, weil nur dort halbiert wurde; der Strom ist jetzt nativ, und
/// die Textur geht ohne Skalierer in den Pool.
pub fn weg_fuer_aufnahme(weg: Weg, drehung: Drehung) -> Weg {
    if weg == Weg::D3d11 && drehung != Drehung::Keine { Weg::Bgra } else { weg }
}

/// Bleibt der stehende Encoder ueber einen Aufbau der Duplication bei
/// gleicher Stromgroesse hinweg? Nur einer auf dem Prozessorweg, und nur,
/// wenn auch die neue Aufnahme den Prozessorweg nimmt. Ein Pool (Texturweg)
/// gehoert zum Geraet der alten Duplication. Und nimmt die neue Aufnahme
/// Texturen - fuer den neuen Ausgang wurde Null-Kopien entschieden (3.3) -,
/// soll der Encoder gleich so laufen, nicht erst nach dem naechsten Codec-
/// oder Einstellungswechsel (Schritt 4b stellte sonst die Quelle auf den
/// stehenden Prozessorweg-Encoder um: kein toter Strom, aber der alte Weg).
pub fn encoder_bleibt(enc_texturen: bool, aufnahme_texturen: bool) -> bool {
    !enc_texturen && !aufnahme_texturen
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
/// Bild im Encoder und ohne ausgelassenes: nichts - haelt der Encoder es
/// noch zurueck (Vorlauf), schiebt der Takt danach nach
/// (takt::vorlauf_nachschieben).
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

/// Kam das neueste Bild (`letztes`: Aufnahmezeit, codiert) nicht in den
/// Encoder, und ist seit seiner ANKUNFT beim Aufnahmefaden (`ankunft_us`,
/// Hostuhr in us) eine ganze Bildzeit ohne neueres vergangen? Dann ist die
/// Bewegung zu Ende, und der Takt legt es nach. Waehrend einer Bewegung
/// kommt das naechste Bild frueher (bei 144 Hz alle 7 ms) - dort soll nichts
/// ueber die Zielrate hinaus nachgelegt werden.
///
/// Gezaehlt wird - wie beim Mac-Host seit b3e9055 - ab der Ankunft, nicht
/// ab der Aufnahmezeit (LastPresentTime der Duplication). Der Faden holt
/// erst ab, wenn er mit dem Vorigen fertig ist (Encoder, Einlesen,
/// Drehen); was dazwischen praesentiert wurde, liefert
/// AcquireNextFrame zusammen, mit der Zeit der juengsten Praesentation. Die
/// liegt also bis zu einem Bildabstand der Quelle vor dem Abholen, und bis
/// zum Takt kommt noch das Einlesen dieses Bildes dazu. Bei Zielraten nahe
/// der Quelle (Voreinstellung 120 an einem 144-Hz-Schirm: Bildzeit 8,3 ms,
/// Bildabstand 6,9 ms) ist das mehr als eine Bildzeit; ab der Aufnahmezeit
/// gerechnet legte der Takt dann mitten in einer Bewegung das aeltere Bild
/// nach, waehrend das neuere schon wartete. Die Aufnahmezeit bleibt fuer
/// den Stempel.
fn ausgelassenes_faellig(letztes: Option<(u64, bool)>, ankunft_us: u64, jetzt_us: u64, fps: u32) -> bool {
    matches!(letztes, Some((_, false)) if jetzt_us.saturating_sub(ankunft_us) >= 1_000_000 / fps.max(1) as u64)
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
    /// Bild im Hauptspeicher (Prozessorweg), in Stromgroesse.
    ram: Vec<u8>,
    /// Gedrehter Ausgang: die ganze Oberflaeche, ungedreht.
    ram_voll: Vec<u8>,
    /// Gedrehter Ausgang mit ungeradem Rand: das ganze Bild, wie der
    /// Desktop steht (sonst dreht es direkt in `ram`).
    ram_gedreht: Vec<u8>,
    /// Modus PQ: die Ebenen des Wandlers (u16, dicht gepackt) in Stromgroesse.
    ram16: Vec<u8>,
}

impl Aufnahme {
    /// Die Aufnahme auf einer Duplication, mit der Quelle, die der Encoder
    /// nimmt (Texturen oder Systemspeicher).
    fn neu(dup: Duplication, texturen: bool) -> Result<Aufnahme, String> {
        let mut a = Aufnahme { dup, staging: None, kopie: None, ram: Vec::new(), ram_voll: Vec::new(), ram_gedreht: Vec::new(), ram16: Vec::new() };
        a.quelle_anlegen(texturen, None, 0, 0)?;
        Ok(a)
    }

    /// Modus PQ der Aufnahme: Some(4:4:4) bzw. None fuer den Bildweg BGRA -
    /// verglichen mit Betrieb::pq_art.
    fn pq_art(&self) -> Option<bool> {
        self.dup.pq().map(|p| p.chroma444)
    }

    /// Das festgehaltene Bild als Quelle fuer den Encoder: die Ebenen (Modus
    /// PQ), die Kopie (Texturweg) oder BGRA im Hauptspeicher.
    fn quelle(&self) -> Quelle<'_> {
        if self.dup.pq().is_some() {
            Quelle::Ebenen16(&self.ram16)
        } else if let Some(k) = self.kopie.as_ref() {
            Quelle::Textur(k)
        } else {
            Quelle::Ram(&self.ram)
        }
    }

    /// Die Quelle fuer den Encoder anlegen - DEFAULT-Kopie (Texturweg) oder
    /// STAGING zum Auslesen, in Groesse des Modus -, die andere faellt weg.
    /// Traegt die alte schon ein Bild, kommt es mit (auf der Karte kopiert,
    /// beim Wechsel auf den Prozessorweg auch gleich ausgelesen): ein neuer
    /// Encoder soll nicht auf die naechste Aenderung am Desktop warten.
    /// Liefert, ob das Bild mitkam.
    ///
    /// `pq`: Modus PQ fuer einen HDR10-Encoder (Some(4:4:4)) - dann gibt es
    /// weder Kopie noch STAGING, der Wandler rechnet die Ebenen in
    /// Stromgroesse w x h. Beim Wechsel zwischen PQ und SDR kommt das Bild
    /// aus dem, was der Wandler noch haelt (im neuen Modus nachgerechnet).
    fn quelle_anlegen(&mut self, texturen: bool, pq: Option<bool>, w: i32, h: i32) -> Result<bool, String> {
        let (dw, dh) = (self.dup.breite, self.dup.hoehe);
        if let Some(chroma444) = pq {
            let plan = PqPlan { w: w.max(0) as u32, h: h.max(0) as u32, drehung: self.dup.drehung, chroma444 };
            self.staging = None;
            self.kopie = None;
            self.ram = Vec::new();
            self.ram_voll = Vec::new();
            self.ram_gedreht = Vec::new();
            self.dup.pq_setzen(Some(plan))?;
            if !self.dup.pq_nachrechnen()? {
                return Ok(false);
            }
            self.dup.pq_auslesen(&mut self.ram16)?;
            return Ok(true);
        }
        if self.dup.pq().is_some() {
            // Zurueck aus dem Modus PQ: das letzte Bild haelt nur noch der
            // Wandler - als BGRA nachgerechnet in die neue Quelle.
            self.dup.pq_setzen(None)?;
            self.ram16 = Vec::new();
            let t = self.dup.textur(dw, dh, !texturen)?;
            let bild = self.dup.sdr_nachrechnen()?;
            if let Some(b) = bild.as_ref() {
                unsafe { self.dup.ctx.CopyResource(&t, b) };
            }
            if texturen {
                self.kopie = Some(t);
            } else {
                self.staging = Some(t);
                if bild.is_some() {
                    self.einlesen(w, h)?;
                }
            }
            return Ok(bild.is_some());
        }
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
    /// Stromgroesse w x h (nativ; ein ungerader Rand faellt beim Auslesen
    /// weg). Gedreht: erst die ganze Oberflaeche lesen, dann drehen
    /// (gedreht_in_stromgroesse). Im Modus PQ die Ebenen des Wandlers.
    fn einlesen(&mut self, w: i32, h: i32) -> Result<(), String> {
        if self.dup.pq().is_some() {
            // Modus PQ: gedreht und umgerechnet hat schon der Wandler.
            return self.dup.pq_auslesen(&mut self.ram16);
        }
        let Some(s) = self.staging.as_ref() else { return Ok(()) };
        let d = self.dup.drehung;
        if d == Drehung::Keine {
            return self.dup.auslesen(s, w as u32, h as u32, &mut self.ram);
        }
        let (dw, dh) = (self.dup.breite, self.dup.hoehe);
        self.dup.auslesen(s, dw, dh, &mut self.ram_voll)?;
        gedreht_in_stromgroesse(&self.ram_voll, dw as usize, dh as usize, d, w as usize, h as usize, &mut self.ram_gedreht, &mut self.ram)
    }
}

/// Gedrehter Ausgang auf dem Prozessorweg: die ganze Oberflaeche `voll`
/// (sw x sh, ungedreht, dicht gepackt) so drehen, wie der Desktop steht, und
/// in Stromgroesse w x h nach `ram` legen. Hat der Desktop gerade Masse
/// (der Regelfall, der Strom ist dann genau so gross), dreht sie direkt in
/// `ram` - bei 4K im Hochformat eine Kopie von 33 MB je Bild weniger;
/// sonst ueber `zwischen`, und der ungerade Rand faellt weg.
#[allow(clippy::too_many_arguments)]
pub fn gedreht_in_stromgroesse(voll: &[u8], sw: usize, sh: usize, d: Drehung, w: usize, h: usize, zwischen: &mut Vec<u8>, ram: &mut Vec<u8>) -> Result<(), String> {
    let (gw, gh) = d.groesse(sw, sh);
    if (gw, gh) == (w, h) {
        return bgra_drehen(voll, sw, sh, d, ram);
    }
    bgra_drehen(voll, sw, sh, d, zwischen)?;
    zeilen_holen(zwischen, gw * 4, w, h, ram)
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

/// Eine Aufnahmesitzung fuer die Dauer eines Zuschauers. `stand` ist die
/// Bildschirmwahl des Hosts (Wunsch, Liste, Ziel), die ueber die Sitzung
/// hinaus gilt.
fn sitzung(stand: &mut Bildschirmstand) {
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
    let mut verloren = false;
    let mut kein_bildschirm_gemeldet = false;
    let mut dup_fehler_gemeldet = String::new();
    let mut enc_fehler_gemeldet = String::new();
    let mut zeiger_fehler_gemeldet = false;
    let mut verlustmeldung = Verlustmeldung::default();
    let mut weg = stand.weg;
    // Kennung des Bildschirms, dessen Duplication in dieser Sitzung zuletzt
    // stand: ein anderer beim naechsten Aufbau ist ein Bildschirmwechsel
    // (1.4) - Switch 7, Info 1, Vollbild, auch bei gleicher Groesse.
    let mut gestreamt: Option<String> = None;
    // Letztes echtes Bild: Aufnahmezeit, ob es codiert wurde (fuer die
    // Wiederholung ohne neue Umrechnung).
    let mut letztes: Option<(u64, bool)> = None;
    // Wann zuletzt ein neues Bild ankam (Hostuhr, us) - fuer den Nachschub
    // bei Vorlauf und fuer das Nachlegen eines ausgelassenen Bildes
    // (ausgelassenes_faellig): waehrend einer Bewegung schieben die neuen
    // Bilder selbst. Die Ankunft ist das Abholen, nicht LastPresentTime.
    let mut letzte_ankunft_us = 0u64;
    // Nach "Testbild aus" oder fuer einen neuen Zuschauer: das letzte
    // Desktopbild einmal nachlegen, auch ohne feste Bildrate (mit Grund).
    let mut nachholen: Option<&'static str> = None;
    let (mut w, mut h) = (Z.info_w.load(Ordering::Relaxed) as i32, Z.info_h.load(Ordering::Relaxed) as i32);
    let mut zuschauer = netz::zuschauer_nr();
    // HDR (Plan 3): ob die Farbe des Stroms neu zu entscheiden ist (Anstoss
    // vom Netzfaden ueber Z.hdr_neu, neuer Zuschauer, andere Farblage, nach
    // jedem Oeffnen eines Encoders), und wann der naechste Farbwechsel
    // fruehestens darf (einer je 2 s).
    let mut farbe_faellig = false;
    let mut naechster_farbwechsel = Instant::now();
    Z.hdr_gescheitert.store(-1, Ordering::Relaxed);

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
            // Der Neue hat noch kein IN_ANZEIGE: ein PQ-Strom des Alten geht
            // an ihn nicht weiter (Grund 7), bis er seine Anzeige meldet.
            Z.hdr_gescheitert.store(-1, Ordering::Relaxed);
            farbe_faellig = true;
        }

        // 1. Bildschirm (1.2): den Wunsch des Zuschauers (Nachricht 70)
        //    abholen, alle 2 s - und vor jedem Aufbau - die Ausgaenge neu
        //    aufzaehlen und das Ziel bestimmen; Aenderungen gehen als
        //    Nachricht 12 hinaus - nach einem Wechsel bei stehender
        //    Duplication erst nach dem Aufbau des neuen Stroms (1b). Weicht
        //    das Ziel vom laufenden ab, faellt die Duplication hier,
        //    zwischen zwei Bildern; der Aufbau darunter nimmt das neue Ziel
        //    (1.4).
        let aufbau_faellig = auf.is_none() && Instant::now() >= naechster_versuch;
        if aufbau_faellig {
            stand.pruefung_faellig();
        }
        let bewertung = stand.nachfuehren(auf.is_some());
        if bewertung.gewechselt && auf.is_some() {
            // Der alte Strom haelt an; sein letztes Bild gehoert nicht zum
            // neuen Bildschirm.
            auf = None;
            letztes = None;
            naechster_versuch = Instant::now();
        }

        // 1b. Duplication aufbauen oder wiederherstellen, alle 2 s - auf dem
        //     Ziel aus Schritt 1.
        if auf.is_none() && Instant::now() >= naechster_versuch {
            naechster_versuch = Instant::now() + Duration::from_secs(2);
            match stand.ziel() {
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
                // Weg allein; der Null-Kopien-Weg kennt noch keine Drehung
                // (weg_fuer_aufnahme). Ein anderer Bildschirm als der zuletzt
                // gestreamte ist ein Wechsel (1.4): der Encoderweg wird fuer
                // ihn neu entschieden, und nach dem Aufbau gehen Switch 7
                // (mit dem laufenden Codec - der Zuschauer baut den Decoder
                // neu) und Info 1 hinaus, dann ein Vollbild.
                Some(a) => {
                    let wechsel = gestreamt.as_deref().is_some_and(|k| k != a.kennung);
                    if let Some(neu) = stand.weg_neu_fuer(&a) {
                        weg = neu;
                    }
                    let aufbau = duplication_aufbauen(&a).and_then(move |d| {
                        let weg_hier = weg_fuer_aufnahme(weg, d.drehung);
                        Aufnahme::neu(d, encoder::texturweg(Z.codec_id.load(Ordering::Relaxed) as usize, weg_hier))
                    });
                    match aufbau {
                        Ok(neu) => {
                            kein_bildschirm_gemeldet = false;
                            dup_fehler_gemeldet.clear();
                            let d = &neu.dup;
                            // HDR-Desktop an oder aus (auch nach einem
                            // Neuaufbau wegen "HDR verwenden"): gleich
                            // eintragen, damit eine Strominfo unten schon
                            // danach entscheidet; neu entschieden wird unten.
                            // Dazu die Metadaten fuer Strominfo und Encoder.
                            let quelle_hdr = d.quelle_hdr();
                            let quelle_anders = Z.quelle_hdr.swap(quelle_hdr, Ordering::Relaxed) != quelle_hdr;
                            let (weiss, mastering) = hdr_metadaten(&d.farbe);
                            Z.hdr_weiss_nit.store(weiss as u32, Ordering::Relaxed);
                            Z.hdr_master_max_nit.store(mastering.max_nit as u32, Ordering::Relaxed);
                            Z.hdr_master_min.store(mastering.min_zehntausendstel as u32, Ordering::Relaxed);
                            let (gw, gh) = d.desktop_groesse();
                            let ((nw, nh), geaendert) = strom_anpassen(&a, gw as i32, gh as i32);
                            let groesse_neu = geaendert || nw != w || nh != h;
                            if groesse_neu {
                                w = nw;
                                h = nh;
                            }
                            // Der stehende Encoder faellt bei anderer Groesse
                            // und wenn er nicht zur neuen Aufnahme passt
                            // (encoder_bleibt: sein Pool haengt am alten
                            // Geraet, oder der fuer den neuen Ausgang
                            // entschiedene Weg nimmt Texturen) - vor dem
                            // Switch, damit seine letzten Pakete noch zum
                            // alten Strom gehoeren. Ein Pool am alten Geraet
                            // wird nicht mehr geleert.
                            if let Some(e) = enc.take() {
                                if !groesse_neu && encoder_bleibt(e.texturen(), neu.kopie.is_some()) {
                                    enc = Some(e);
                                } else if e.texturen() && !groesse_neu {
                                    drop(e);
                                } else {
                                    e.schliessen();
                                }
                            }
                            if wechsel {
                                // Erst der Switch, dann die Info - auch bei
                                // gleicher Groesse; das Vollbild (unten) traegt
                                // die Parametersaetze (kein GLOBAL_HEADER).
                                encoder::switch_senden(Z.codec_id.load(Ordering::Relaxed) as usize, Z.farbe.load(Ordering::Relaxed));
                            }
                            if wechsel || groesse_neu {
                                netz::strominfo_senden();
                            }
                            gestreamt = Some(a.kennung.clone());
                            let weg_hier = weg_fuer_aufnahme(weg, d.drehung);
                            if weg_hier != weg {
                                log("Null-Kopien-Weg: gedrehter Ausgang wird noch nicht auf der Karte gedreht - Prozessorweg (bgra)");
                                weg = weg_hier;
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
                                    a.karte, a.karte_name, d.format_text(),
                                    if d.im_systemspeicher { "ja" } else { "nein" }, w, h,
                                    if (w, h) != (gw as i32, gh as i32) { " (nativ, ungerader Rand abgeschnitten)" } else { " (nativ)" },
                                    if neu.kopie.is_some() { "Textur" } else { "Prozessorweg" }
                                ));
                                log(format!("Aufnahme {}: Farbe {}", a.name, d.farbe.zeile()));
                                if d.farbe.hdr && !quelle_hdr {
                                    log(format!("Aufnahme {}: HDR-Desktop, aber die Duplication liefert kein FP16 - kein HDR10 (Windows bildet selbst nach SDR ab)", a.name));
                                }
                            }
                            if quelle_anders {
                                Z.hdr_gescheitert.store(-1, Ordering::Relaxed);
                                netz::hdr_neu_entscheiden(if quelle_hdr { "Desktop jetzt HDR" } else { "Desktop jetzt SDR" });
                                farbe_faellig = true;
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
                    }
                }
            }
        }

        // 1c. Nachricht 12 nach einem Wechsel: jetzt, nach dem Aufbau des
        //     neuen Stroms (1.4 Schritt 6) - auch nach einem gescheiterten.
        stand.meldung_nachholen();

        // 2. Encoder oeffnen, sobald die Aufnahme steht - gleich in der Farbe,
        //    die der Host entscheidet (hdr_grund); weicht sie von der zuletzt
        //    angekuendigten ab, gehen Switch 7 und Strominfo hinaus.
        if auf.is_some() && enc.is_none() && Instant::now() >= naechster_enc_versuch {
            let a = auf.as_ref().unwrap();
            let idx = Z.codec_id.load(Ordering::Relaxed) as usize;
            match oeffnen_in_farbe(idx, super::hdr_ziel_pq(idx), w, h, weg, Some((&a.dup.device, &a.dup.ctx))) {
                Ok(b) => {
                    let t = b.farbe().transfer();
                    enc = Some(b);
                    enc_fehler_gemeldet.clear();
                    testbilder = None;
                    // Das Ziel galt beim Anstoss. Was waehrend des Oeffnens
                    // kam (IN_ANZEIGE, "HDR aus"), hat der Netzfaden noch
                    // gegen die alte Farbe in Z.farbe verglichen und darum
                    // keinen Wechsel angefordert - Schritt 3b vergleicht neu.
                    farbe_faellig = true;
                    if Z.farbe.swap(t, Ordering::Relaxed) != t {
                        encoder::switch_senden(idx, t);
                        netz::strominfo_senden();
                    }
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

        // 3. Codecwunsch (Nachricht 66): zwischen zwei Bildern, nie mittendrin -
        //    der neue Kandidat gleich in der Farbe, die fuer ihn gilt.
        if let Some(idx) = encoder::codec_wunsch_abholen() {
            strom_wechseln(idx, super::hdr_ziel_pq(idx), &mut enc, auf.as_ref(), w, h, weg);
            testbilder = None;
            // Wie nach Schritt 2: was waehrend des Oeffnens kam, vergleicht 3b.
            farbe_faellig = true;
        }

        // 3b. HDR: die Farbe neu entscheiden, wenn es angestossen wurde
        //     (IN_ANZEIGE, Farblage, neuer Zuschauer). Weicht sie vom
        //     laufenden Strom ab: ein Farbwechsel wie ein Codecwechsel,
        //     hoechstens einer je 2 s - sonst bleibt es faellig.
        if Z.hdr_neu.swap(false, Ordering::Relaxed) {
            farbe_faellig = true;
        }
        if farbe_faellig {
            match enc.as_ref().map(|e| (e.idx, e.farbe().ist_pq())) {
                // Ohne Encoder entscheidet das naechste Oeffnen (Schritt 2).
                None => farbe_faellig = false,
                Some((idx, laeuft_pq)) => {
                    let ziel_pq = super::hdr_ziel_pq(idx);
                    if ziel_pq == laeuft_pq {
                        farbe_faellig = false;
                    } else if auf.is_some() && Instant::now() >= naechster_farbwechsel {
                        naechster_farbwechsel = Instant::now() + Duration::from_secs(2);
                        strom_wechseln(idx, ziel_pq, &mut enc, auf.as_ref(), w, h, weg);
                        testbilder = None;
                        // Steht der Wechsel, bleibt es faellig: was waehrend
                        // des Oeffnens kam, hat der Netzfaden noch gegen die
                        // alte Farbe verglichen - die naechste Runde
                        // vergleicht neu (ein Zurueck wartet die 2 s ab). Ging
                        // er nicht (Rueckfall in SDR mit Grund 6, oder die
                        // alte Sitzung kam zurueck), laeuft schon die einzige
                        // andere Farbe - kein neuer Versuch alle 2 s; ohne
                        // Sitzung entscheidet das Oeffnen in Schritt 2.
                        farbe_faellig = enc.as_ref().is_some_and(|e| e.farbe().ist_pq() == ziel_pq);
                    }
                }
            }
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
            if a.kopie.is_some() != e.texturen() || a.pq_art() != e.pq_art() {
                match a.quelle_anlegen(e.texturen(), e.pq_art(), w, h) {
                    Ok(mit) => {
                        log(format!(
                            "Aufnahme: Quelle auf {} umgestellt (Kandidat {} {} nimmt {}){}",
                            match e.pq_art() {
                                Some(_) => "PQ-Ebenen des Wandlers",
                                None if e.texturen() => "Textur",
                                None => "Prozessorweg",
                            },
                            e.idx, encoder::kandidat(e.idx).name,
                            match e.pq_art() {
                                Some(true) => "YUV444P16LE, HDR10",
                                Some(false) => "P010, HDR10",
                                None if e.texturen() => "Texturen",
                                None => "Systemspeicher",
                            },
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
                match encoder::testbilder(e.pix_fmt, w, h, e.farbe()) {
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
                    letzte_ankunft_us = super::now_us();
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
                                    if e.codieren(a.quelle(), t_cap, pts, false).is_ok() {
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
            // Ein Verlust ist oft ein Topologiewechsel: gleich neu bewerten
            // (Liste, Ziel), der Aufbau kommt in 2 s.
            stand.pruefung_faellig();
        }

        // 7. Der Takt: Testbild als echte Bilder in Zielrate; feste Bildrate
        //    legt das letzte Bild nach, wenn seit 0,9/fps nichts kam. Auch
        //    ohne feste Bildrate geht das letzte Bild EINMAL hinein, wenn
        //    der Encoder frisch ist (Start, Codecwechsel, Neustart), das
        //    Testbild endet, ein neuer Zuschauer ohne Pause abgeloest hat
        //    oder das neueste Bild nicht in den Encoder kam (Raster "zu
        //    schnell", Encoder voll, Stau) und seit seiner Ankunft eine
        //    Bildzeit lang kein neueres (nachlegen_grund,
        //    ausgelassenes_faellig) - sonst sieht der Zuschauer bei stillem
        //    Desktop bis zur naechsten Aenderung nichts, die Balken oder
        //    einen Zwischenstand der Bewegung. Haelt der
        //    Encoder Bilder zurueck (Vorlauf, h264_mf), schiebt der Takt
        //    ohne feste Bildrate danach das letzte Bild in Zielrate nach,
        //    bis das letzte echte Bild heraus ist - dann ist Ruhe.
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
                    ausgelassenes_faellig(letztes, letzte_ankunft_us, super::now_us(), fps) && takt.nachlegen_faellig(fps),
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
                            let q = if codiert { Quelle::Wiederholung } else { a.quelle() };
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
                } else if let (Some(_), Some((t_cap, true))) = (auf.as_ref(), letztes) {
                    // Vorlauf leeren: das Endbild einer Bewegung, das erste
                    // Bild fuer einen neuen Zuschauer, das Bild nach
                    // Codecwechsel, Neustart oder Testbild stecken sonst im
                    // Encoder, bis sich der Desktop wieder aendert. Die
                    // Wiederholung traegt die echte Aufnahmezeit und ist als
                    // wiederholt gestempelt (ausserhalb der Latenzmessung);
                    // das Raster bleibt unberuehrt. Ein ausgelassenes Bild
                    // (codiert = false) legt oben der Grund "" zuerst nach.
                    let nachschieben = super::takt::vorlauf_nachschieben(
                        e.vorlauf(),
                        e.ausstehend(),
                        super::now_us().saturating_sub(letzte_ankunft_us),
                        fps,
                        Z.fest.load(Ordering::Relaxed),
                    ) && takt.nachlegen_faellig(fps);
                    if !nachschieben || e.inflight() >= INFLIGHT_TAKT {
                        // Nichts auszugeben, oder der Encoder ist noch voll.
                    } else if netz::stau_vor_dem_encoder() {
                        takt.ausgelassen();
                    } else {
                        let pts = takt.pts_vorwaerts(super::now_us() as i64, fps);
                        if e.codieren(Quelle::Nachschub, t_cap, pts, true).is_ok() {
                            Z.nachgeschoben.fetch_add(1, Ordering::Relaxed);
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
    // Der naechste Zuschauer beginnt in SDR (seine Begruessung sagt es so).
    Z.farbe.store(crate::hdr::TRANSFER_SDR, Ordering::Relaxed);
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

/// "PQ/BT.2020" bzw. "SDR" fuers Protokoll.
fn farbe_text(pq: bool) -> &'static str {
    if pq { "HDR10 PQ/BT.2020" } else { "SDR" }
}

/// Eine Encoder-Sitzung fuer den Kandidaten in der gewuenschten Farbe. Geht
/// sie in PQ nicht auf, merkt sich der Host das (Grund 6, Z.hdr_gescheitert
/// fuer genau diesen Kandidaten) und oeffnet denselben Kandidaten in SDR.
fn oeffnen_in_farbe(idx: usize, pq: bool, w: i32, h: i32, weg: Weg, geraet: Option<(&ID3D11Device, &ID3D11DeviceContext)>) -> Result<Betrieb, String> {
    if !pq {
        return Betrieb::oeffnen(idx, w, h, weg, geraet, encoder::StromFarbe::Sdr709);
    }
    match Betrieb::oeffnen(idx, w, h, weg, geraet, encoder::stromfarbe(true)) {
        Ok(b) => Ok(b),
        Err(e) => {
            log(format!("HDR10-Sitzung fuer {} scheitert ({e}) - derselbe Kandidat in SDR (Grund 6)", encoder::kandidat(idx).name));
            Z.hdr_gescheitert.store(idx as i32, Ordering::Relaxed);
            Betrieb::oeffnen(idx, w, h, weg, geraet, encoder::StromFarbe::Sdr709)
        }
    }
}

/// Codec- und Farbwechsel wie main.m: alte Sitzung leeren (ihre letzten
/// Pakete gehen noch raus), neue oeffnen (Kandidat `idx`, PQ oder SDR), ERST
/// DANN Switch 7 mit dem Transfer, Vollbild erzwingen, Strominfo 1. Geht
/// sie in PQ nicht auf, derselbe Kandidat in SDR (oeffnen_in_farbe); scheitert
/// sie ganz, kommt die alte zurueck - ohne Switch. Die Aufnahme stellt sich
/// danach selbst um (Schritt 4b: Quelle, Modus PQ).
fn strom_wechseln(idx: usize, pq: bool, enc: &mut Option<Betrieb>, auf: Option<&Aufnahme>, w: i32, h: i32, weg: Weg) {
    let alt = Z.codec_id.load(Ordering::Relaxed) as usize;
    let alt_pq = match enc.as_ref() {
        Some(e) => e.farbe().ist_pq(),
        None => Z.farbe.load(Ordering::Relaxed) == crate::hdr::TRANSFER_PQ,
    };
    let Some(a) = auf else {
        log(format!("Wechsel auf {} ({}) abgelehnt: keine laufende Aufnahme", encoder::kandidat(idx).name, farbe_text(pq)));
        return;
    };
    if idx == alt && pq == alt_pq && enc.is_some() {
        log(format!("Codecwunsch {} ({}): laeuft bereits", encoder::kandidat(idx).name, farbe_text(pq)));
        return;
    }
    log(format!(
        "{}: {} {} -> {} {}",
        if idx == alt { "Farbwechsel" } else { "Codecwechsel" },
        encoder::kandidat(alt).name,
        farbe_text(alt_pq),
        encoder::kandidat(idx).name,
        farbe_text(pq)
    ));
    if let Some(e) = enc.take() {
        e.schliessen();
    }
    // Der Zuschauer wartet wieder auf ein Vollbild; das SWITCH kommt erst,
    // wenn die neue Sitzung steht.
    Z.wait_key.store(true, Ordering::Relaxed);
    Z.force_key.store(true, Ordering::Relaxed);
    let geraet = Some((&a.dup.device, &a.dup.ctx));
    match oeffnen_in_farbe(idx, pq, w, h, weg, geraet) {
        Ok(b) => {
            let t = b.farbe().transfer();
            Z.codec_id.store(idx as u32, Ordering::Relaxed);
            Z.farbe.store(t, Ordering::Relaxed);
            *enc = Some(b);
            encoder::switch_senden(idx, t);
            Z.force_key.store(true, Ordering::Relaxed);
            netz::strominfo_senden();
            log(format!("Strom gewechselt: {} {}", encoder::kandidat(idx).name, farbe_text(t == crate::hdr::TRANSFER_PQ)));
        }
        Err(e) => {
            log(format!("Wechsel auf {} fehlgeschlagen ({e}) - baue {} wieder auf", encoder::kandidat(idx).name, encoder::kandidat(alt).name));
            match oeffnen_in_farbe(alt, alt_pq, w, h, weg, geraet) {
                Ok(b) => {
                    let t = b.farbe().transfer();
                    let anders = Z.farbe.swap(t, Ordering::Relaxed) != t;
                    *enc = Some(b);
                    if anders {
                        encoder::switch_senden(alt, t);
                    }
                    Z.force_key.store(true, Ordering::Relaxed);
                    netz::strominfo_senden();
                    log(format!("Alter Codec laeuft wieder: {} {}", encoder::kandidat(alt).name, farbe_text(t == crate::hdr::TRANSFER_PQ)));
                }
                Err(e2) => log(format!("Auch der alte Codec {} laesst sich nicht mehr oeffnen ({e2}) - es kommt kein Bild mehr", encoder::kandidat(alt).name)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// Ein Ausgang fuer die Wahl-Tests: Kennung, Name, Lage, Hauptbildschirm.
    fn ausgang(index: usize, kennung: &str, name: &str, links: i32, hz: u16, haupt: bool) -> Ausgang {
        Ausgang {
            index,
            name: format!("\\\\.\\DISPLAY{}", index + 1),
            kennung: kennung.into(),
            anzeigename: name.into(),
            hz,
            links,
            oben: 0,
            breite: 1920,
            hoehe: 1080,
            karte: 0,
            karte_name: "Karte".into(),
            nvidia: false,
            haupt,
        }
    }

    /// Die zwei Bildschirme aus den Pruefvektoren (2.4): "X27 X1" Haupt,
    /// "Virtuell 16:9" daneben.
    fn zwei() -> Vec<Ausgang> {
        vec![ausgang(0, "v1138-m1234-s0", "X27 X1", 0, 120, true), ausgang(1, "v0-m0-s0", "Virtuell 16:9", 1920, 240, false)]
    }

    /// Die Liste aus Ausgaengen ergibt byte-genau den Pruefvektor der
    /// Spezifikation (2.4) - Automatik, der Hauptbildschirm gestreamt.
    #[test]
    fn pruefvektor_aus_ausgaengen() {
        let liste = hex(
            "01 02 00 00 \
             0e 06 80 07 38 04 78 00 03 00 76 31 31 33 38 2d 6d 31 32 33 34 2d 73 30 58 32 37 20 58 31 \
             08 0d 80 07 38 04 f0 00 00 00 76 30 2d 6d 30 2d 73 30 56 69 72 74 75 65 6c 6c 20 31 36 3a 39",
        );
        let b = als_bildschirme(&zwei(), None, Some("v1138-m1234-s0"));
        assert_eq!(bildschirm::bildschirme_kodieren(&b), liste);
        // Mit Wunsch auf den zweiten, der zweite gestreamt: Wunsch und Flags
        // wandern mit; die Kennung kommt aus dem Wunsch-Pruefvektor.
        let b = als_bildschirme(&zwei(), Some("v0-m0-s0"), Some("v0-m0-s0"));
        let p = bildschirm::bildschirme_kodieren(&b);
        assert_eq!(&p[..4], &[1, 2, 8, 0]);
        assert_eq!(&p[4..12], &hex("76 30 2d 6d 30 2d 73 30")[..]);
        let g = bildschirm::bildschirme_lesen(&p).unwrap();
        assert_eq!(g.gestreamt().map(|e| e.name.as_str()), Some("Virtuell 16:9"));
        assert_eq!(g.gewuenschter().map(|e| e.hz), Some(240));
        assert!(g.eintraege[0].haupt && !g.eintraege[0].gestreamt);
        // Ohne Ziel traegt keiner das Bit.
        let b = als_bildschirme(&zwei(), None, None);
        assert!(b.eintraege.iter().all(|e| !e.gestreamt));
        // Groessen ueber u16 werden gekappt, nicht umgebrochen.
        let mut riesig = zwei();
        riesig[0].breite = 70_000;
        assert_eq!(als_bildschirme(&riesig, None, None).eintraege[0].breite, u16::MAX);
    }

    /// ziel_waehlen (1.2), alle Faelle: Automatik folgt dem Hauptbildschirm,
    /// ohne Hauptbildschirm der erste; ein Wunsch gilt, wenn er da ist;
    /// fehlt er, der Hauptbildschirm als Ausweichplatz; leere Liste: keiner.
    #[test]
    fn ziel_waehlen_alle_faelle() {
        let l = zwei();
        assert_eq!(ziel_waehlen(&l, None), Some((l[0].clone(), Wahl::Automatik)));
        assert_eq!(ziel_waehlen(&l, Some("v0-m0-s0")), Some((l[1].clone(), Wahl::Wunsch)));
        assert_eq!(ziel_waehlen(&l, Some("v1138-m1234-s0")), Some((l[0].clone(), Wahl::Wunsch)));
        assert_eq!(ziel_waehlen(&l, Some("DEL4321")), Some((l[0].clone(), Wahl::Ausweich)));
        // Hauptbildschirm gewechselt: Automatik folgt.
        let mut m = zwei();
        m[0].haupt = false;
        m[1].haupt = true;
        assert_eq!(ziel_waehlen(&m, None), Some((m[1].clone(), Wahl::Automatik)));
        assert_eq!(ziel_waehlen(&m, Some("DEL4321")).map(|(a, w)| (a.kennung, w)), Some(("v0-m0-s0".into(), Wahl::Ausweich)));
        // Kein Hauptbildschirm gemeldet: der erste.
        m[1].haupt = false;
        assert_eq!(ziel_waehlen(&m, None).map(|(a, _)| a.index), Some(0));
        assert_eq!(ziel_waehlen(&m, Some("x")).map(|(a, w)| (a.index, w)), Some((0, Wahl::Ausweich)));
        // Nur der gewuenschte ist weg -> der Rest entscheidet; ganz leer -> keiner.
        assert_eq!(ziel_waehlen(&l[..1], Some("v0-m0-s0")), Some((l[0].clone(), Wahl::Ausweich)));
        assert_eq!(ziel_waehlen(&[], None), None);
        assert_eq!(ziel_waehlen(&[], Some("v0-m0-s0")), None);
    }

    /// Die Gruende fuer die Protokollzeile (1.4 Schritt 7).
    #[test]
    fn wechsel_grund_alle_faelle() {
        assert_eq!(wechsel_grund(Wahl::Wunsch, true), "Wunsch des Zuschauers");
        assert_eq!(wechsel_grund(Wahl::Automatik, true), "Wunsch des Zuschauers: Automatik");
        assert_eq!(wechsel_grund(Wahl::Ausweich, true), "Ausweichplatz");
        assert_eq!(wechsel_grund(Wahl::Ausweich, false), "Ausweichplatz");
        assert_eq!(wechsel_grund(Wahl::Wunsch, false), "zurueck zum gewuenschten Bildschirm");
        assert_eq!(wechsel_grund(Wahl::Automatik, false), "Hauptbildschirm gewechselt");
    }

    /// Kennung und Name aus den Strings der Geraete (1.3), rein.
    #[test]
    fn kennung_und_name_aus_geraete_id() {
        let name = "\\\\.\\DISPLAY1";
        assert_eq!(kennung_aus_geraete_id("MONITOR\\ACR0501\\{4d36e96e-e325-11ce-bfc1-08002be10318}\\0001", name), "ACR0501");
        assert_eq!(kennung_aus_geraete_id("MONITOR\\DEL4321\\{4d36e96e-e325-11ce-bfc1-08002be10318}\\0002", name), "DEL4321");
        // Ohne Geraete-ID, ohne zweiten Teil oder nur Steuerzeichen: der Geraetename.
        assert_eq!(kennung_aus_geraete_id("", name), name);
        assert_eq!(kennung_aus_geraete_id("MONITOR", name), name);
        assert_eq!(kennung_aus_geraete_id("MONITOR\\\u{1}\\x", name), name);
        // Steuerzeichen fallen weg, hoechstens 64 Byte.
        assert_eq!(kennung_aus_geraete_id("MONITOR\\AB\tC\\x", name), "ABC");
        let lang = format!("MONITOR\\{}\\x", "k".repeat(100));
        assert_eq!(kennung_aus_geraete_id(&lang, name).len(), bildschirm::KENNUNG_MAX);

        // Zwei gleiche Monitore: beide mit Listenplatz, ein dritter anderer nicht.
        let mut l = vec![ausgang(0, "ACR0501", "a", 0, 60, true), ausgang(1, "DEL4321", "b", 1920, 60, false), ausgang(2, "ACR0501", "c", 3840, 60, false)];
        kennungen_eindeutig(&mut l);
        assert_eq!(l.iter().map(|a| a.kennung.as_str()).collect::<Vec<_>>(), ["ACR0501-0", "DEL4321", "ACR0501-2"]);
        let mut eins = vec![ausgang(0, "ACR0501", "a", 0, 60, true)];
        kennungen_eindeutig(&mut eins);
        assert_eq!(eins[0].kennung, "ACR0501");
        // Eine Kennung an der 64-Byte-Grenze doppelt: der Anhang passt
        // trotzdem hinein (die Basis wird gekuerzt), beide bleiben
        // verschieden und in der Grenze - auch mit mehrbytigen Zeichen.
        for lang in ["k".repeat(bildschirm::KENNUNG_MAX), "\u{e4}".repeat(bildschirm::KENNUNG_MAX / 2)] {
            let mut l = vec![ausgang(0, &lang, "a", 0, 60, true), ausgang(1, &lang, "b", 1920, 60, false)];
            kennungen_eindeutig(&mut l);
            assert_ne!(l[0].kennung, l[1].kennung, "{lang}");
            assert!(l[0].kennung.ends_with("-0") && l[1].kennung.ends_with("-1"), "{:?}", l[1].kennung);
            assert!(l.iter().all(|a| a.kennung.len() <= bildschirm::KENNUNG_MAX && a.kennung.len() > bildschirm::KENNUNG_MAX - 4), "{:?}", l[0].kennung);
        }

        // Name: Monitorname, sonst DeviceString, sonst Geraetename; leer und
        // Leerraum zaehlen nicht, gekuerzt auf 48 Byte.
        assert_eq!(name_waehlen(Some("X27 X1"), Some("PnP-Monitor (Standard)"), name), "X27 X1");
        assert_eq!(name_waehlen(Some("  "), Some("PnP-Monitor (Standard)"), name), "PnP-Monitor (Standard)");
        assert_eq!(name_waehlen(None, None, name), name);
        assert_eq!(name_waehlen(Some(""), Some("\u{7}"), name), name);
        assert_eq!(name_waehlen(Some(&"ä".repeat(40)), None, name), "ä".repeat(24));
    }

    /// Der Stand des Aufnahmefadens (bewerten_mit): Automatik folgt dem
    /// Hauptbildschirm; ein Wunsch wechselt; fehlt der gewuenschte, gilt
    /// der Hauptbildschirm als Ausweichplatz, mit dem Wunsch in der Liste;
    /// kommt er zurueck, geht es von selbst zurueck; eine geaenderte Liste
    /// ist zu melden, eine gleiche nicht.
    #[test]
    fn bewertung_folgt_haupt_wunsch_ausweich_rueckkehr() {
        let mut st = Bildschirmstand::neu(None, zwei(), Weg::Auto, Weg::Bgra);
        assert_eq!(st.ziel.as_ref().map(|a| a.kennung.as_str()), Some("v1138-m1234-s0"));
        assert_eq!(st.wahl, Some(Wahl::Automatik));
        // Dieselbe Liste: nichts zu tun, nichts zu melden.
        assert_eq!(st.bewerten_mit(zwei(), false), Bewertung::default());
        // Hauptbildschirm wandert: Automatik folgt.
        let mut m = zwei();
        m[0].haupt = false;
        m[1].haupt = true;
        let b = st.bewerten_mit(m.clone(), false);
        assert_eq!(b, Bewertung { gewechselt: true, melden: true, grund: Some("Hauptbildschirm gewechselt") });
        assert_eq!(st.ziel.as_ref().map(|a| a.index), Some(1));
        // Nur die Bildrate aendert sich: melden, kein Wechsel.
        m[0].hz = 144;
        assert_eq!(st.bewerten_mit(m.clone(), false), Bewertung { gewechselt: false, melden: true, grund: None });
        // Wunsch auf den ersten (Zuschauer): Wechsel mit Grund.
        st.wunsch = Some("v1138-m1234-s0".into());
        let b = st.bewerten_mit(m.clone(), true);
        assert_eq!(b, Bewertung { gewechselt: true, melden: true, grund: Some("Wunsch des Zuschauers") });
        assert_eq!(st.wahl, Some(Wahl::Wunsch));
        // Derselbe Wunsch noch einmal: kein Wechsel, aber Antwort (melden).
        assert_eq!(st.bewerten_mit(m.clone(), true), Bewertung { gewechselt: false, melden: true, grund: None });
        // Der gewuenschte faellt weg: Ausweichplatz Hauptbildschirm, der
        // Wunsch bleibt und steht in der Liste.
        let nur_zweiter = vec![m[1].clone()];
        let b = st.bewerten_mit(nur_zweiter.clone(), false);
        assert_eq!(b, Bewertung { gewechselt: true, melden: true, grund: Some("Ausweichplatz") });
        assert_eq!(st.wahl, Some(Wahl::Ausweich));
        assert_eq!(st.wunsch.as_deref(), Some("v1138-m1234-s0"));
        let liste = als_bildschirme(&st.liste, st.wunsch.as_deref(), st.ziel.as_ref().map(|a| a.kennung.as_str()));
        assert_eq!(liste.wunsch.as_deref(), Some("v1138-m1234-s0"));
        assert_eq!(liste.gestreamt().map(|e| e.kennung.as_str()), Some("v0-m0-s0"));
        assert!(liste.gewuenschter().is_none());
        // Er kommt zurueck: von selbst zurueck.
        let b = st.bewerten_mit(m.clone(), false);
        assert_eq!(b, Bewertung { gewechselt: true, melden: true, grund: Some("zurueck zum gewuenschten Bildschirm") });
        assert_eq!(st.ziel.as_ref().map(|a| a.index), Some(0));
        // Wunsch auf eine fremde Kennung: gespeichert, Ausweichplatz, kein
        // Wechsel, wenn schon der Hauptbildschirm laeuft ... hier laeuft der
        // erste (nicht Haupt), also Wechsel auf den Hauptbildschirm.
        st.wunsch = Some("DEL4321".into());
        let b = st.bewerten_mit(m.clone(), true);
        assert_eq!(b, Bewertung { gewechselt: true, melden: true, grund: Some("Ausweichplatz") });
        assert_eq!(st.ziel.as_ref().map(|a| a.index), Some(1));
        // Zurueck auf Automatik: der Hauptbildschirm laeuft schon - kein Wechsel.
        st.wunsch = None;
        assert_eq!(st.bewerten_mit(m.clone(), true), Bewertung { gewechselt: false, melden: true, grund: None });
        // Alles weg und wieder da: kein Grund (kein alter bzw. neuer
        // Bildschirm), aber gewechselt.
        assert_eq!(st.bewerten_mit(Vec::new(), false), Bewertung { gewechselt: true, melden: true, grund: None });
        assert!(st.ziel.is_none() && st.wahl.is_none());
        assert_eq!(st.bewerten_mit(m.clone(), false), Bewertung { gewechselt: true, melden: true, grund: None });
        // Der Encoderweg haengt am Ausgang: fuer denselben bleibt die
        // Entscheidung vom Start, fuer einen anderen faellt eine neue
        // (Befehlszeile bgra -> bgra, ohne messung.txt).
        let mut st = Bildschirmstand::neu(None, zwei(), Weg::Yuv444, Weg::Bgra);
        assert_eq!(st.weg_neu_fuer(&zwei()[0]), None, "Startausgang: Entscheidung vom Start bleibt");
        assert_eq!(st.weg_neu_fuer(&zwei()[1]), Some(Weg::Yuv444), "anderer Ausgang: neu entschieden");
        assert_eq!(st.weg_neu_fuer(&zwei()[1]), None);
        assert_eq!(st.weg_neu_fuer(&zwei()[0]), Some(Weg::Yuv444));
    }

    /// Nach einem Aufbau bei gleicher Stromgroesse bleibt nur ein Encoder
    /// auf dem Prozessorweg, und nur, wenn auch die neue Aufnahme den
    /// Prozessorweg nimmt: ein Pool haengt am alten Geraet, und nimmt die
    /// neue Aufnahme Texturen (Null-Kopien fuer den neuen Ausgang
    /// entschieden, 3.3), laeuft der Encoder gleich so - Texturen an einen
    /// Encoder ohne Pool ("Textur ohne Pool") gibt es nie.
    #[test]
    fn encoder_bleibt_nur_prozessorweg_auf_prozessorweg() {
        assert!(encoder_bleibt(false, false), "Prozessorweg -> Prozessorweg: bleibt");
        assert!(!encoder_bleibt(false, true), "Prozessorweg-Encoder, neue Aufnahme mit Texturen: neu");
        assert!(!encoder_bleibt(true, false), "Pool am alten Geraet: neu");
        assert!(!encoder_bleibt(true, true), "Pool am alten Geraet: neu, auch fuer Texturen");
    }

    /// Scheitert das Aufzaehlen (DXGI-Factory), gilt die letzte Liste
    /// weiter: kein Wechsel, nichts zu melden, kein gesunder Strom faellt;
    /// der Fehler wird gemerkt (einmal ins Protokoll) und mit der naechsten
    /// guten Liste vergessen.
    #[test]
    fn aufzaehlfehler_behaelt_die_letzte_liste() {
        let mut st = Bildschirmstand::neu(None, zwei(), Weg::Auto, Weg::Bgra);
        let l = st.liste_aufgezaehlt(Err("CreateDXGIFactory1: kaputt".into()));
        assert_eq!(l, zwei());
        assert_eq!(st.aufzaehl_fehler, "CreateDXGIFactory1: kaputt");
        assert_eq!(st.bewerten_mit(l, false), Bewertung::default());
        assert_eq!(st.ziel.as_ref().map(|a| a.index), Some(0));
        // Derselbe Fehler noch einmal: dieselbe Liste, gemerkt bleibt er.
        assert_eq!(st.liste_aufgezaehlt(Err("CreateDXGIFactory1: kaputt".into())), zwei());
        assert_eq!(st.aufzaehl_fehler, "CreateDXGIFactory1: kaputt");
        // Eine gute Liste: sie gilt, der Fehler ist vergessen.
        let l = st.liste_aufgezaehlt(Ok(vec![zwei()[1].clone()]));
        assert_eq!(l.len(), 1);
        assert!(st.aufzaehl_fehler.is_empty());
        assert_eq!(st.bewerten_mit(l, false).gewechselt, true);
    }

    /// Nachricht 12 nach einem Wechsel bei laufendem Strom erst nach dem
    /// Aufbau des neuen (1.4 Schritt 6); ohne laufenden Strom, bei einer
    /// blossen Listenaenderung und als Antwort auf einen Wunsch sofort. Eine
    /// offene Meldung wird genau einmal nachgeholt.
    #[test]
    fn nachricht_12_nach_wechsel_erst_nach_dem_aufbau() {
        let mut st = Bildschirmstand::neu(None, zwei(), Weg::Auto, Weg::Bgra);
        let wechsel = Bewertung { gewechselt: true, melden: true, grund: Some("Wunsch des Zuschauers") };
        let nur_liste = Bewertung { gewechselt: false, melden: true, grund: None };
        // Ohne laufenden Strom: sofort.
        assert!(st.meldung_faellig(wechsel, false));
        assert!(!st.meldung_offen);
        // Mit laufendem Strom: zurueckgestellt, bis der Aufbau sie nachholt.
        assert!(!st.meldung_faellig(wechsel, true));
        assert!(st.meldung_offen);
        assert!(std::mem::take(&mut st.meldung_offen));
        // Nichts zu melden: nichts. Nur die Liste: sofort, auch im Betrieb.
        assert!(!st.meldung_faellig(Bewertung::default(), true));
        assert!(st.meldung_faellig(nur_liste, true));
        // Eine offene Meldung geht mit der naechsten faelligen Bewertung
        // mit hinaus, auch ohne eigene Aenderung - und danach nicht mehr.
        assert!(!st.meldung_faellig(wechsel, true));
        assert!(st.meldung_faellig(Bewertung::default(), false));
        assert!(!st.meldung_offen);
        assert!(!st.meldung_faellig(Bewertung::default(), false));
    }

    /// bildschirm.txt (1.5): geschrieben und gelesen, "auto" fuer Automatik,
    /// fehlt -> Fehlt, kaputt -> Unlesbar und unveraendert liegen gelassen.
    #[test]
    fn wunsch_datei_hin_und_zurueck() {
        let ordner = std::env::temp_dir().join(format!("qc-bildschirm-{}", std::process::id()));
        std::fs::create_dir_all(&ordner).unwrap();
        let pfad = ordner.join("bildschirm.txt");
        assert_eq!(wunsch_laden_aus(&pfad), Gespeichert::Fehlt);
        wunsch_speichern_nach(&pfad, Some("ACR0501")).unwrap();
        assert_eq!(std::fs::read_to_string(&pfad).unwrap(), "ACR0501\n");
        assert_eq!(wunsch_laden_aus(&pfad), Gespeichert::Kennung("ACR0501".into()));
        wunsch_speichern_nach(&pfad, None).unwrap();
        assert_eq!(std::fs::read_to_string(&pfad).unwrap(), "auto\n");
        assert_eq!(wunsch_laden_aus(&pfad), Gespeichert::Automatik);
        // Keine temporaere Datei bleibt liegen.
        assert_eq!(std::fs::read_dir(&ordner).unwrap().count(), 1);
        // Ein bildschirm.<pid>.neu eines abgestuerzten Laufs raeumt der
        // Start weg (wunsch_laden); anderes und die Datei selbst bleiben.
        std::fs::write(ordner.join("bildschirm.4711.neu"), "ACR0501\n").unwrap();
        std::fs::write(ordner.join("anderes.neu"), "x").unwrap();
        assert_eq!(wunsch_reste_aufraeumen(&pfad), 1);
        assert!(!ordner.join("bildschirm.4711.neu").exists());
        assert!(ordner.join("anderes.neu").exists() && pfad.exists());
        assert_eq!(wunsch_reste_aufraeumen(&pfad), 0);
        std::fs::remove_file(ordner.join("anderes.neu")).unwrap();
        assert_eq!(wunsch_reste_aufraeumen(&ordner.join("gibt-es-nicht").join("bildschirm.txt")), 0);
        // Zeilenende in Windows-Art und Leerzeilen am Rand sind in Ordnung.
        std::fs::write(&pfad, "\r\nv0-m0-s0\r\n\r\n").unwrap();
        assert_eq!(wunsch_laden_aus(&pfad), Gespeichert::Kennung("v0-m0-s0".into()));
        // Kaputt: leer, zwei Zeilen, kein UTF-8, Steuerzeichen, zu lang.
        for kaputt in [b"".to_vec(), b"\n \n".to_vec(), b"a\nb\n".to_vec(), vec![0xff, 0xfe, b'a'], b"a\tb\n".to_vec(), vec![b'k'; 65]] {
            std::fs::write(&pfad, &kaputt).unwrap();
            assert_eq!(wunsch_laden_aus(&pfad), Gespeichert::Unlesbar, "{kaputt:?}");
            assert_eq!(std::fs::read(&pfad).unwrap(), kaputt, "Datei still ersetzt");
        }
        // Ueber die Ablage: im Test der Temp-Ordner (secure::config_dir).
        wunsch_speichern(Some("DEL4321"));
        assert_eq!(wunsch_laden(), Some("DEL4321".into()));
        wunsch_speichern(None);
        assert_eq!(wunsch_laden(), None);
        std::fs::remove_dir_all(&ordner).ok();
    }

    /// Die Win32-Wege der Liste (DXGI, GDI, EnumDisplayDevices,
    /// QueryDisplayConfig, EnumDisplaySettings) laufen ohne Absturz, und
    /// jeder Ausgang traegt eine Kennung und einen Namen in den Grenzen der
    /// Leitung, hoechstens einer ist Hauptbildschirm, die Kennungen sind
    /// eindeutig. Auf der Bau-VM (ssh, Sitzung 0) ist die DXGI-Liste leer -
    /// dann prueft das nur die GDI-Seite.
    #[test]
    fn geraetewege_liefern_kennung_und_name() {
        let liste = ausgaenge().unwrap_or_default();
        for a in &liste {
            println!("Ausgang {}: {} Kennung {} ({}) {} Hz{}", a.index, a.name, a.kennung, a.anzeigename, a.hz, if a.haupt { " Haupt" } else { "" });
            assert!(!a.kennung.is_empty() && a.kennung.len() <= bildschirm::KENNUNG_MAX, "{a:?}");
            assert!(a.anzeigename.len() <= bildschirm::NAME_MAX, "{a:?}");
            assert_eq!(bildschirm::kennung_bereinigen(&a.kennung), a.kennung);
        }
        assert!(liste.iter().filter(|a| a.haupt).count() <= 1);
        let mut kennungen: Vec<&str> = liste.iter().map(|a| a.kennung.as_str()).collect();
        kennungen.sort();
        kennungen.dedup();
        assert_eq!(kennungen.len(), liste.len(), "Kennungen nicht eindeutig");
        for (name, r, haupt) in gdi_monitore() {
            let (id, s) = monitor_geraet(&name);
            println!("GDI {name}: {}x{} Haupt {haupt}, Geraete-ID {id:?}, DeviceString {s:?}, {} Hz", r.right - r.left, r.bottom - r.top, bildrate(&name));
            assert!(!kennung_aus_geraete_id(id.as_deref().unwrap_or(""), &name).is_empty());
        }
        let _ = anzeigenamen();
    }

    #[test]
    fn stromplan_ist_nativ() {
        // Nativ (29.09.2026): genau die Pixel des Anzeigemodus, auch ab 3840
        // Breite - nichts wird halbiert; ein ungerader Rand faellt weg.
        assert_eq!(stromplan(1920, 1080), (1920, 1080));
        assert_eq!(stromplan(2560, 1440), (2560, 1440));
        assert_eq!(stromplan(3840, 2160), (3840, 2160));
        assert_eq!(stromplan(5120, 1440), (5120, 1440));
        assert_eq!(stromplan(5120, 2880), (5120, 2880));
        assert_eq!(stromplan(6016, 3384), (6016, 3384));
        assert_eq!(stromplan(1367, 769), (1366, 768));
        assert_eq!(stromplan(3841, 2161), (3840, 2160));
        assert_eq!(stromplan(3839, 2160), (3838, 2160));
        // Gedreht: der Desktop, wie er steht - Oberflaeche 3840x2160 bei 90
        // bzw. 270 Grad ergibt den Strom 2160x3840, bei 180 Grad 3840x2160.
        assert_eq!(stromplan(1080, 1920), (1080, 1920));
        for d in [Drehung::Grad90, Drehung::Grad270] {
            let (gw, gh) = d.groesse(3840, 2160);
            assert_eq!(stromplan(gw, gh), (2160, 3840));
            let (gw, gh) = d.groesse(1921, 1081);
            assert_eq!(stromplan(gw, gh), (1080, 1920));
        }
        let (gw, gh) = Drehung::Grad180.groesse(3840, 2160);
        assert_eq!(stromplan(gw, gh), (3840, 2160));
        // Der vorlaeufige Wert beim Start folgt derselben Regel.
        let mut a = ausgang(0, "ACR0501", "a", 0, 60, true);
        for (b, h) in [(1920, 1080), (2560, 1440), (3840, 2160), (1367, 769)] {
            (a.breite, a.hoehe) = (b, h);
            assert_eq!(stromgroesse(&a), stromplan(b, h));
        }
    }

    #[test]
    fn null_kopien_auch_ab_4k_nur_nicht_gedreht() {
        // Die Groesse entscheidet nichts mehr (weg_fuer_aufnahme kennt sie
        // gar nicht): ein 4K-Ausgang behaelt d3d11. Nur ein gedrehter geht
        // auf den Prozessorweg - dort wird gedreht.
        for w in [Weg::Bgra, Weg::Yuv444, Weg::D3d11, Weg::Auto] {
            assert_eq!(weg_fuer_aufnahme(w, Drehung::Keine), w);
        }
        for d in [Drehung::Grad90, Drehung::Grad180, Drehung::Grad270] {
            assert_eq!(weg_fuer_aufnahme(Weg::D3d11, d), Weg::Bgra);
            assert_eq!(weg_fuer_aufnahme(Weg::Bgra, d), Weg::Bgra);
            assert_eq!(weg_fuer_aufnahme(Weg::Yuv444, d), Weg::Yuv444);
        }
    }

    #[test]
    fn plan_und_quelle_ergeben_genau_die_stromgroesse() {
        // Ungedreht liefert das Auslesen (die linke obere Ecke in
        // Stromgroesse) genau w*h*4 Byte - mit einem Zeilenabstand wie bei
        // einer gemappten STAGING-Textur, auch bei 4K und groesser.
        for (dw, dh) in [(1920usize, 1080usize), (1367, 769), (2560, 1440), (5120, 1440), (3842, 2160), (3840, 2160), (1080, 1920)] {
            let (w, h) = stromplan(dw as i32, dh as i32);
            let (w, h) = (w as usize, h as usize);
            let abstand = (dw * 4 + 255) & !255;
            let gemappt = vec![9u8; abstand * dh];
            let mut ram = Vec::new();
            zeilen_holen(&gemappt, abstand, w, h, &mut ram).unwrap();
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
        assert!(!ausgelassenes_faellig(None, t, t + bildzeit, fps));
        assert!(!ausgelassenes_faellig(Some((t, true)), t, t + 10 * bildzeit, fps), "codiert: nichts nachzulegen");
        assert!(!ausgelassenes_faellig(Some((t, false)), t, t + bildzeit - 1, fps), "Bewegung laeuft vielleicht noch");
        assert!(ausgelassenes_faellig(Some((t, false)), t, t + bildzeit, fps));
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
                    assert!(!ausgelassenes_faellig(Some(l), l.0, t_us - 1, fps), "n {n}, Bild {k}");
                }
                letztes = Some((t_us, takt.schlitz_frei(t_s, fps)));
            }
            let (t_end, codiert) = letztes.unwrap();
            if !codiert {
                ohne_endbild_vorher += 1;
            }
            // Nach der Bewegung: genau dann faellig, wenn das Endbild fehlt.
            assert_eq!(ausgelassenes_faellig(letztes, t_end, t_end + bildzeit, fps), !codiert, "n {n}");
        }
        assert_eq!(ohne_endbild_vorher, 23, "Probe des Pruefers nicht getroffen");
    }

    /// Wie hosttest.m "waehrend einer Bewegung nichts dazu" (Mac-Host,
    /// b3e9055): ob seit einer Bildzeit nichts Neues kam, zaehlt ab der
    /// Ankunft beim Aufnahmefaden, nicht ab LastPresentTime. Mit der alten
    /// Rechnung (jetzt - Aufnahmezeit) scheitern beide Teile.
    #[test]
    fn ausgelassenes_zaehlt_ab_der_ankunft() {
        // 20 ms zwischen Praesentation und Ankunft - mehr als eine Bildzeit
        // bei 60 fps: gleich nach der Ankunft ist nichts faellig, erst eine
        // ganze Bildzeit spaeter.
        let fps = 60;
        let bildzeit = 1_000_000 / fps as u64;
        let t_cap = 5_000_000;
        let ankunft = t_cap + 20_000;
        assert!(!ausgelassenes_faellig(Some((t_cap, false)), ankunft, ankunft + 2_000, fps), "20 ms Versatz: mitten in der Bewegung nachgelegt");
        assert!(!ausgelassenes_faellig(Some((t_cap, false)), ankunft, ankunft + bildzeit - 1, fps));
        assert!(ausgelassenes_faellig(Some((t_cap, false)), ankunft, ankunft + bildzeit, fps));

        // Der Fall des Windows-Hosts: Zielrate 120 (Voreinstellung), Quelle
        // 144 Hz. Der Faden war beschaeftigt und holt jedes Bild 6 ms nach
        // seiner Praesentation ab (weniger als ein Bildabstand von 6,9 ms -
        // sonst laege schon ein juengeres an), der Takt fragt nach 3 ms
        // Einlesen. Waehrend der Bewegung wird nie nachgelegt, nach ihrem
        // Ende genau das fehlende Endbild, eine Bildzeit nach seiner Ankunft.
        let fps = 120;
        let bildzeit = 1_000_000 / fps as u64;
        let (versatz, einlesen) = (6_000u64, 3_000u64);
        let mut ausgelassen = 0;
        let mut enden = Vec::new();
        for n in 2..=39u32 {
            let mut takt = Schrittmacher::neu();
            let beginn = 100.0 + n as f64;
            let mut letztes: Option<(u64, bool)> = None;
            let mut ankunft = 0;
            for k in 0..n {
                let t_s = beginn + k as f64 / 144.0;
                let t_us = (t_s * 1e6) as u64;
                ankunft = t_us + versatz;
                let codiert = takt.schlitz_frei(t_s, fps);
                ausgelassen += !codiert as u32;
                letztes = Some((t_us, codiert));
                if k + 1 < n {
                    assert!(!ausgelassenes_faellig(letztes, ankunft, ankunft + einlesen, fps), "n {n}, Bild {k}: mitten in der Bewegung nachgelegt");
                }
            }
            enden.push((n, letztes, ankunft));
        }
        assert!(ausgelassen > 30, "Probe ohne ausgelassene Bilder ({ausgelassen})");
        for (n, letztes, ankunft) in enden {
            let codiert = letztes.unwrap().1;
            assert!(!ausgelassenes_faellig(letztes, ankunft, ankunft + bildzeit - 1, fps), "n {n}: Endbild vor Ablauf einer Bildzeit nachgelegt");
            assert_eq!(ausgelassenes_faellig(letztes, ankunft, ankunft + bildzeit, fps), !codiert, "n {n}");
        }
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
        // gemappte Oberflaeche -> drehen -> Stromgroesse, genau so, wie
        // Aufnahme::einlesen es tut (gedreht_in_stromgroesse). Gerade Masse
        // drehen direkt in den Strom (kein Zwischenpuffer), ungerade ueber
        // den Zwischenpuffer mit abgeschnittenem Rand - Punkt fuer Punkt
        // dasselbe wie Drehen und dann Abschneiden.
        for (sw, sh, d) in [
            (1920usize, 1080usize, Drehung::Grad90),
            (1920, 1080, Drehung::Grad270),
            (1367, 769, Drehung::Grad90),
            (1367, 769, Drehung::Grad180),
            (3840, 2160, Drehung::Grad180),
            (3840, 2160, Drehung::Grad90),
            (1200, 1920, Drehung::Grad90),
            (2160, 3840, Drehung::Grad270),
        ] {
            let (gw, gh) = d.groesse(sw, sh);
            let (w, h) = stromplan(gw as i32, gh as i32);
            let (w, h) = (w as usize, h as usize);
            let abstand = (sw * 4 + 255) & !255;
            let mut gemappt = vec![0u8; abstand * sh];
            let bild = lagebild(sw, sh);
            for y in 0..sh {
                gemappt[y * abstand..y * abstand + sw * 4].copy_from_slice(&bild[y * sw * 4..(y + 1) * sw * 4]);
            }
            let (mut voll, mut zwischen, mut ram) = (Vec::new(), Vec::new(), Vec::new());
            zeilen_holen(&gemappt, abstand, sw, sh, &mut voll).unwrap();
            gedreht_in_stromgroesse(&voll, sw, sh, d, w, h, &mut zwischen, &mut ram).unwrap();
            assert_eq!(ram.len(), w * h * 4, "{sw}x{sh} {d:?}");
            assert_eq!(zwischen.is_empty(), (gw, gh) == (w, h), "{sw}x{sh} {d:?}: Zwischenpuffer nur bei ungeradem Rand");
            let (mut gedreht, mut erwartet) = (Vec::new(), Vec::new());
            bgra_drehen(&voll, sw, sh, d, &mut gedreht).unwrap();
            zeilen_holen(&gedreht, gw * 4, w, h, &mut erwartet).unwrap();
            assert!(ram == erwartet, "{sw}x{sh} {d:?}: anderer Inhalt");
        }
    }

    /// Die HDR10-Metadaten eines Schirms: SDR-Weiss aus SDRWhiteLevel (ohne
    /// oder mit unbrauchbarem Wert 80 nit), Spitze und Minimum aus GetDesc1,
    /// ohne Angabe 1000 nit / 0,005 nit; geklemmt statt uebergelaufen.
    #[test]
    fn hdr_metadaten_aus_der_farblage() {
        let f = Farblage {
            hdr: true,
            desc1: true,
            farbraum: DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
            bits: 10,
            spitze_nit: 1015.4,
            vollbild_nit: 600.0,
            min_nit: 0.0123,
            sdr_weiss: Some(3000),
        };
        let (weiss, m) = hdr_metadaten(&f);
        assert_eq!((weiss, m.max_nit, m.min_zehntausendstel), (240, 1015, 123));
        let ohne = Farblage { spitze_nit: 0.0, min_nit: 0.0, sdr_weiss: None, desc1: false, ..f };
        let (weiss, m) = hdr_metadaten(&ohne);
        assert_eq!((weiss, m), (80, encoder::Mastering::VORGABE));
        let faul = Farblage { spitze_nit: f32::NAN, min_nit: 1e-9, sdr_weiss: Some(99_999), ..f };
        let (weiss, m) = hdr_metadaten(&faul);
        assert_eq!((weiss, m), (80, encoder::Mastering::VORGABE));
        let riesig = Farblage { spitze_nit: 1e9, min_nit: 100.0, ..f };
        assert_eq!(hdr_metadaten(&riesig).1, encoder::Mastering { max_nit: u16::MAX, min_zehntausendstel: u16::MAX });
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
