// Anzeige ueber die Grafikkarte: Direct3D 11 und DXGI, direkt ueber das
// windows-Crate, das ohnehin im Baum steckt (Ton, Zwischenablage, Konsole).
// Kein wgpu, kein Shader-Uebersetzer zur Laufzeit, keine neue Kiste: fuenf
// Feature-Zeilen in Cargo.toml, sonst nichts. d3d11.dll, dxgi.dll und
// d3dcompiler_47.dll sind Bestandteile von Windows seit 8.1.
//
// Aufbau: Die Decoder-Ebenen gehen roh als UINT-Texturen auf die Karte.
// Stufe 1 rechnet sie mit derselben 16.16-Ganzzahlarithmetik wie `zeile_rgb`
// in eine B8G8R8A8-Zwischentextur in Bildgroesse - bitidentisch zu `to_rgb`;
// der Goldbildtest (--anzeigetest) verlangt dort Toleranz 0. Stufe 2 legt
// das Bild mit denselben Quellpunkten wie `blit` (Rechteck aus
// `ziel_rechteck`, Schritt in 16.16 mit derselben abgeschnittenen Division)
// in das Ziel, weich abgetastet ueber den Sampler der Karte, und darueber
// die von der CPU gezeichnete Oberflaeche aus einer zweiten Textur, deren
// oberstes Byte die Durchsicht ist (siehe ui.rs).
//
// Praesentation: eine Flip-Swapchain am Fenster (FLIP_DISCARD, zwei Puffer,
// hoechstens ein Bild unterwegs), Present immer mit SyncInterval 0 - mit
// ALLOW_TEARING, wo DXGI es erlaubt. Vor jedem Present wird das
// Frame-Latency-Warteobjekt mit Timeout 0 abgefragt; ist es nicht frei, wird
// das Bild uebersprungen, nie gewartet: Maus und Tastatur laufen auf
// demselben Faden. `ohne_fenster` (--anzeigetest) laesst die Swapchain weg
// und zeichnet in eine eigene Zieltextur.

use std::ffi::c_void;

use ffmpeg_next as ffmpeg;
use windows::core::{Interface, BOOL, PCSTR};
use windows::Win32::Foundation::{CloseHandle, DXGI_STATUS_OCCLUDED, HANDLE, HMODULE, HWND, S_OK, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::anzeigeprobe::{fall_pruefen, oberflaeche_probe, probewert};
use crate::{ebenen_format, protokoll, ui, EbenenFormat, Frame, Karte, Rolle};

/// Ein Adapter, wie er im Protokoll und in der Statistik steht.
pub struct AdapterInfo {
    pub name: String,
    pub vendor: u32,
    /// Software-Rasterizer: WARP ("Microsoft Basic Render Driver").
    pub software: bool,
    /// Haengt ein Bildschirm daran? Auf Optimus-Laptops ohne MUX hat nur
    /// die iGPU einen Ausgang.
    pub hat_ausgang: bool,
    /// Kennung des Adapters (AdapterLuid), mit der sich das Geraet der
    /// Anzeige einer erkannten Karte zuordnen laesst.
    pub luid: i64,
    /// Eigener Videospeicher (DedicatedVideoMemory) in MB.
    pub speicher_mb: u64,
}

/// Textur mit Lesesicht: Eingangsebenen und die Oberflaeche.
struct Textur {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    w: u32,
    h: u32,
}

/// Zwischentextur: Ziel von Stufe 1, Quelle von Stufe 2.
struct Zwischen {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    rtv: ID3D11RenderTargetView,
    w: u32,
    h: u32,
}

/// Die Eingangstexturen des laufenden Decoderformats. Der Schluessel sagt,
/// wann sie neu angelegt werden muessen: Codecwechsel, NVDEC <-> Software,
/// andere Aufloesung.
struct Ebenen {
    schluessel: (ffmpeg::format::Pixel, u32, u32),
    fmt: EbenenFormat,
    y: Textur,
    u: Option<Textur>,
    v: Option<Textur>,
    /// NV12/P010/P012/P016: U und V als Paare in EINER Textur.
    uv: Option<Textur>,
}

/// Konstanten von Stufe 1 (cbuffer Format, 16 Byte).
#[repr(C)]
struct KonstFormat {
    sub: u32,
    paar: u32,
    schieb: u32,
    _f: u32,
}

/// Konstanten von Stufe 2 (cbuffer Anzeige, 64 Byte, HLSL-Packung: jede
/// Vektorgroesse bleibt in ihrem 16-Byte-Register).
#[repr(C)]
struct KonstAnzeige {
    /// x, y, Breite, Hoehe des Bildes im Ziel, aus `ziel_rechteck`.
    rect: [i32; 4],
    /// Bildbreite, Bildhoehe.
    bild: [u32; 2],
    /// Quellschritt je Zielpunkt in 16.16, wie in `blit`.
    schritt: [u32; 2],
    /// Bit 0: Bild vorhanden, Bit 1: Oberflaeche sichtbar.
    modus: u32,
    _a: [u32; 3],
    /// Grundfarbe ausserhalb des Bildes (ui::BG).
    grund: [f32; 4],
}

/// Ergebnis eines Praesentierversuchs.
pub enum Praesentiert {
    Ok,
    /// DXGI_STATUS_OCCLUDED: das Fenster ist verdeckt. Kein Fehler.
    Verdeckt,
    /// Etwas anderes ging schief, das Geraet lebt aber (Text fuer das
    /// Protokoll).
    Fehler(String),
    /// DXGI_ERROR_DEVICE_REMOVED oder _RESET, mit dem Grund laut
    /// GetDeviceRemovedReason. Die Karte ist damit unbrauchbar.
    GeraetWeg(String),
}

/// Die Karte: Geraet, Shader, Zustaende, die Texturen des laufenden Bildes
/// und - am Fenster - Swapchain, Backbuffer und Warteobjekt.
pub struct Gpu {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    /// None ohne Fenster (--anzeigetest).
    swapchain: Option<IDXGISwapChain2>,
    /// Sicht auf den Backbuffer. Vor ResizeBuffers IMMER fallen lassen.
    rtv: Option<ID3D11RenderTargetView>,
    /// Frame-Latency-Warteobjekt; null ohne Swapchain.
    warte: HANDLE,
    /// Ein Zaehler des Warteobjekts ist abgefragt und noch nicht durch ein
    /// gelungenes Present zurueckgegeben (siehe `bereit`).
    warte_gehalten: bool,
    /// Flags, mit denen die Swapchain angelegt wurde - ResizeBuffers will
    /// dieselben.
    flags: u32,
    /// Groesse der Swapchain.
    pub breite: u32,
    pub hoehe: u32,
    /// Darf Present ohne Warten auf den Bildwechsel (ALLOW_TEARING)?
    pub tearing: bool,
    vs: ID3D11VertexShader,
    ps_umrechnen: ID3D11PixelShader,
    ps_anzeigen: ID3D11PixelShader,
    /// MIN_MAG_MIP_LINEAR, CLAMP.
    sampler: ID3D11SamplerState,
    /// CULL_NONE.
    raster: ID3D11RasterizerState,
    /// 16 Byte, DYNAMIC.
    konst_format: ID3D11Buffer,
    /// 64 Byte, DYNAMIC.
    konst_anzeige: ID3D11Buffer,
    /// Eingangstexturen des laufenden Formats.
    ebenen: Option<Ebenen>,
    /// B8G8R8A8_UNORM, Bildgroesse, RT + SRV.
    zwischen: Option<Zwischen>,
    /// B8G8R8A8_UNORM, Zielgroesse, DEFAULT, nur SRV.
    oberflaeche: Option<Textur>,
    pub feature_level: D3D_FEATURE_LEVEL,
    pub adapter: AdapterInfo,
}

/// Die Shader, einmal beim Start uebersetzt. Stufe 1 rechnet ganzzahlig mit
/// den Konstanten aus `zeile_rgb`; Stufe 2 nimmt dieselben Quellpunkte wie
/// `blit`. Register: Stufe 1 auf b0 und t0..t3, Stufe 2 auf b1 und t4..t5 -
/// getrennt, damit sich die Deklarationen in EINER Quelle nicht in die Quere
/// kommen.
const SHADER: &str = r#"
// ---------- gemeinsam: ein Dreieck ueber das ganze Ziel, ohne Vertexpuffer ----------
// (0,0) (2,0) (0,2) im Bildraum, nach Clip-Koordinaten gespiegelt (y nach unten).
float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

// ---------- Stufe 1: Ebenen -> RGB, bitidentisch zu zeile_rgb ----------
cbuffer Format : register(b0) { uint sub; uint paar; uint schieb; uint _f; };
Texture2D<uint>  ebene_y  : register(t0);
Texture2D<uint>  ebene_u  : register(t1);
Texture2D<uint>  ebene_v  : register(t2);
Texture2D<uint2> ebene_uv : register(t3);   // NV12/P010/P012/P016: (U,V) in einer Textur
float4 ps_umrechnen(float4 pos : SV_Position) : SV_Target {
    int2 xy = int2(pos.xy);
    int2 c  = (sub != 0) ? (xy >> 1) : xy;                 // naechster Farbwert, wie heute
    int y = int(ebene_y.Load(int3(xy, 0)) >> schieb);     // 8 Bit: >>0, 10LE: >>2, MSB/16: >>8
    int cb, cr;
    if (paar != 0) {
        uint2 uv = ebene_uv.Load(int3(c, 0));
        cb = int(uv.x >> schieb);
        cr = int(uv.y >> schieb);
    } else {
        cb = int(ebene_u.Load(int3(c, 0)) >> schieb);
        cr = int(ebene_v.Load(int3(c, 0)) >> schieb);
    }
    cb -= 128; cr -= 128;                                  // Nullpunkt 128, nicht 0.5
    int r = y + ((103206 * cr) >> 16);                    // 1.5748 - dieselben Konstanten wie zeile_rgb
    int g = y - ((12276 * cb + 30681 * cr) >> 16);        // 0.1873, 0.4681 - EINE Verschiebung der Summe
    int b = y + ((121609 * cb) >> 16);                    // 1.8556
    int3 rgb = clamp(int3(r, g, b), 0, 255);
    return float4(float3(rgb) * (1.0 / 255.0), 1.0);      // UNORM-Rundung liefert exakt rgb
}

// ---------- Stufe 2: Letterbox/1:1 + Oberflaeche ----------
cbuffer Anzeige : register(b1) {
    int4   rect;        // x, y, Breite, Hoehe des Bildes im Ziel (aus ziel_rechteck)
    uint2  bild;        // Bildbreite, Bildhoehe
    uint2  schritt;     // Quellschritt je Zielpunkt in 16.16: (fw << 16) / zw, wie blit
    uint   modus;       // Bit0: Bild vorhanden, Bit1: Oberflaeche sichtbar
    uint3  _a;
    float4 grund;       // ui::BG
};
Texture2D<float4> video       : register(t4);   // Zwischentextur (Stufe 1)
Texture2D<float4> oberflaeche : register(t5);   // B8G8R8A8: rgb vormultipliziert, a = Durchsicht
SamplerState weich : register(s0);              // LINEAR, CLAMP
float4 ps_anzeigen(float4 pos : SV_Position) : SV_Target {
    int2 p = int2(pos.xy) - rect.xy;            // pos.xy ist die Bildpunktmitte (x+0.5)
    float3 farbe = grund.rgb;
    if ((modus & 1) != 0 && all(p >= 0) && all(p < rect.zw)) {
        // blit()-Konvention: Quellpunkt = Zielpunkt * Schritt in 16.16, ohne
        // Mittenversatz; Ganzteil = linker Texel, Rest = Gewicht des rechten.
        uint2 f = uint2(p) * schritt;
        if (all(schritt == 65536)) {
            // 1:1 (pixelgenau, oder Fenster in Bildgroesse): reine Kopie,
            // ohne den Umweg ueber den Filter.
            farbe = video.Load(int3(int2(f >> 16), 0)).rgb;
        } else {
            float2 t = float2(f >> 16) + float2(f & 0xffff) * (1.0 / 65536.0);
            farbe = video.SampleLevel(weich, (t + 0.5) / float2(bild), 0).rgb;
        }
    }
    if ((modus & 2) != 0) {
        float4 o = oberflaeche.Load(int3(pos.xy, 0));
        farbe = farbe * o.a + o.rgb;             // "over" mit vormultiplizierter Farbe und Durchsicht
    }
    return float4(farbe, 1);
}
"#;

/// Windows-Fehler als Text: Meldung und HRESULT, damit im Protokoll steht,
/// was die Karte gesagt hat.
fn fehler(was: &str, e: windows::core::Error) -> String {
    format!("{was}: {} (0x{:08x})", e.message().trim(), e.code().0 as u32)
}

/// Name, Hersteller, Software-Kennzeichen und Ausgang eines Adapters.
fn adapter_info(a: &IDXGIAdapter1) -> Result<AdapterInfo, windows::core::Error> {
    let d = unsafe { a.GetDesc1()? };
    let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
    let name = String::from_utf16_lossy(&d.Description[..n]);
    // Software: das Kennzeichen von DXGI, oder Microsoft als Hersteller
    // (aeltere WARP-Faelle tragen das Kennzeichen nicht).
    let software = (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 || d.VendorId == 0x1414;
    let hat_ausgang = unsafe { a.EnumOutputs(0) }.is_ok();
    let luid = ((d.AdapterLuid.HighPart as i64) << 32) | d.AdapterLuid.LowPart as i64;
    Ok(AdapterInfo {
        name,
        vendor: d.VendorId,
        software,
        hat_ausgang,
        luid,
        speicher_mb: (d.DedicatedVideoMemory as u64) >> 20,
    })
}

/// Hat dieser Adapter gemeinsamen Speicher mit dem Prozessor (Unified
/// Memory Architecture)? Das ist das Kennzeichen einer integrierten Grafik.
/// Gefragt wird ein kurz erzeugtes Geraet ohne Fenster nach
/// D3D11_FEATURE_D3D11_OPTIONS2; das Geraet ist danach wieder weg. None,
/// wenn sich kein Geraet bauen laesst oder die Abfrage fehlt (Windows vor
/// 10) - dann greift die Ersatzregel des Aufrufers.
fn unified_memory(a: &IDXGIAdapter1) -> Option<bool> {
    let (device, _ctx, _fl) = geraet_bauen(Some(a), false, false).ok()?;
    let mut opt = D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
    unsafe {
        device.CheckFeatureSupport(
            D3D11_FEATURE_D3D11_OPTIONS2,
            &mut opt as *mut D3D11_FEATURE_DATA_D3D11_OPTIONS2 as *mut c_void,
            std::mem::size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS2>() as u32,
        )
    }
    .ok()?;
    Some(opt.UnifiedMemoryArchitecture.as_bool())
}

/// Die Karten des Rechners mit ihrer Rolle im Menue: "Integriert" ist der
/// Adapter mit gemeinsamem Speicher (Ersatzregel, wenn die Abfrage nicht
/// geht: Intel als Hersteller oder weniger als 512 MB eigener Speicher),
/// "Grafikkarte" jeder andere Hardware-Adapter, in der Reihenfolge, in der
/// DXGI sie zaehlt. Software-Adapter (WARP) zaehlen nicht mit - auf einer
/// Maschine ohne Karte ist die Liste leer. Laeuft einmal beim Start und
/// schreibt je Adapter eine Zeile ins Protokoll.
pub fn karten_erkennen() -> Vec<Karte> {
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(e) => {
            protokoll::zeile(fehler("Erkennung der Karten: CreateDXGIFactory1", e));
            return Vec::new();
        }
    };
    let alle = match adapter_objekte(&factory) {
        Ok(a) => a,
        Err(e) => {
            protokoll::zeile(format!("Erkennung der Karten: {e}"));
            return Vec::new();
        }
    };
    let mut karten = Vec::new();
    let mut dediziert = 0u8;
    let mut integriert_da = false;
    for (i, (adapter, info)) in alle.iter().enumerate() {
        if info.software {
            protokoll::zeile(format!("Karte {i}: {} - Software, keine Rolle", info.name));
            continue;
        }
        let uma = unified_memory(adapter);
        let ist_integriert = match uma {
            Some(u) => u,
            None => info.vendor == 0x8086 || info.speicher_mb < 512,
        };
        // Eine zweite integrierte gibt es nicht; kaeme doch eine, waere sie
        // eine Grafikkarte ohne eigenen Speicher - besser als gar kein Knopf.
        let rolle = if ist_integriert && !integriert_da {
            integriert_da = true;
            Rolle::Integriert
        } else {
            dediziert += 1;
            Rolle::Grafikkarte(dediziert)
        };
        protokoll::zeile(format!(
            "Karte {i}: {} - {}, {} MB, Ausgang {}, UMA {}",
            info.name,
            rolle.name(),
            info.speicher_mb,
            if info.hat_ausgang { "ja" } else { "nein" },
            match uma { Some(true) => "ja", Some(false) => "nein", None => "unbekannt (Ersatzregel)" }
        ));
        karten.push(Karte {
            index: i as u32,
            name: info.name.clone(),
            vendor: info.vendor,
            speicher_mb: info.speicher_mb,
            hat_ausgang: info.hat_ausgang,
            luid: info.luid,
            rolle,
        });
    }
    if karten.is_empty() {
        protokoll::zeile("Karten: kein Hardware-Adapter gefunden".into());
    }
    karten
}

/// Alle Adapter einer Factory, samt Objekt (fuer D3D11CreateDevice).
fn adapter_objekte(factory: &IDXGIFactory1) -> Result<Vec<(IDXGIAdapter1, AdapterInfo)>, String> {
    let mut liste = Vec::new();
    let mut i = 0;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        let info = adapter_info(&a).map_err(|e| fehler("GetDesc1", e))?;
        liste.push((a, info));
        i += 1;
    }
    Ok(liste)
}

/// Alle Adapter, wie DXGI sie zaehlt (EnumAdapters1 + GetDesc1 + EnumOutputs).
pub fn adapter_liste() -> Result<Vec<AdapterInfo>, String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| fehler("CreateDXGIFactory1", e))?;
    Ok(adapter_objekte(&factory)?.into_iter().map(|(_, i)| i).collect())
}

/// Ein Adapter als Protokollzeile, dieselbe im Test und beim Start.
fn adapter_zeile(i: usize, a: &AdapterInfo) -> String {
    format!(
        "Adapter {i}: {} (VendorId 0x{:04x}, {}, Ausgang {})",
        a.name, a.vendor,
        if a.software { "Software" } else { "Hardware" },
        if a.hat_ausgang { "ja" } else { "nein" }
    )
}

/// Erlaubt DXGI ein Present ohne Warten auf den Bildwechsel? Windows 10
/// 1607+, nicht ueber RDP, nicht auf WARP. Ohne Swapchain abfragbar; jeder
/// Fehler heisst nein.
pub fn tearing_moeglich() -> bool {
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
        let Ok(f5) = f.cast::<IDXGIFactory5>() else { return false };
        let mut erlaubt = BOOL(0);
        f5.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            &mut erlaubt as *mut BOOL as *mut c_void,
            std::mem::size_of::<BOOL>() as u32,
        )
        .is_ok()
            && erlaubt.as_bool()
    }
}

// ------------------------------------------------------------------- HDR

/// Kann dieser Weg HDR-Bilder zeigen (Bit 1 in IN_ANZEIGE)? Noch nicht: die
/// Swapchain in R10G10B10A2/G2084 und die PQ-Shader kommen mit Schritt 5b
/// des HDR-Plans. Bis dahin meldet der Windows-Client ehrlich "nein" - ein
/// Host sendet ihm dann nie PQ (Grund 5).
pub const HDR_DARSTELLUNG: bool = false;

/// Der Bildschirm des Fensters fuer IN_ANZEIGE: der Ausgang, auf dem der
/// groesste Teil des Fensters liegt (MonitorFromWindow - nicht
/// GetContainingOutput der Swapchain, die der softbuffer-Weg nicht hat),
/// seine Farblage aus IDXGIOutput6::GetDesc1 ("HDR verwenden" = G2084/P2020,
/// Spitzen) und das SDR-Weiss aus DisplayConfig (dieselbe Hilfe wie der
/// Host). Eine frische Factory je Aufruf: eine alte kennt neu angesteckte
/// Bildschirme nicht, und alle zwei Sekunden kostet das nichts. None, wenn
/// kein Ausgang passt.
pub fn schirm_lage(hwnd: isize) -> Option<crate::hdr::Schirm> {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    let monitor = unsafe { MonitorFromWindow(HWND(hwnd as *mut c_void), MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut i = 0;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        i += 1;
        let mut j = 0;
        while let Ok(o) = unsafe { a.EnumOutputs(j) } {
            j += 1;
            let Ok(d) = (unsafe { o.GetDesc() }) else { continue };
            if d.Monitor != monitor {
                continue;
            }
            let ende = d.DeviceName.iter().position(|&c| c == 0).unwrap_or(d.DeviceName.len());
            let name = String::from_utf16_lossy(&d.DeviceName[..ende]);
            let d1 = o.cast::<IDXGIOutput6>().ok().and_then(|o6| unsafe { o6.GetDesc1() }.ok());
            let hdr = d1.is_some_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
            let weiss = crate::host::aufnahme::sdr_weiss_lesen(&name).map(crate::host::wandler::sdr_weiss_nit).unwrap_or(0.0);
            let spitze = d1.map(|d| d.MaxLuminance).unwrap_or(0.0);
            let kopfraum = if hdr && weiss > 0.0 && spitze > weiss { spitze / weiss } else { 1.0 };
            return Some(crate::hdr::Schirm {
                hdr,
                sdr_weiss_nit: weiss,
                spitze_nit: spitze,
                vollbild_spitze_nit: d1.map(|d| d.MaxFullFrameLuminance).unwrap_or(0.0),
                kopfraum_potentiell: kopfraum,
                kopfraum_aktuell: kopfraum,
            });
        }
    }
    None
}

/// Geraet und Kontext. Mit Adapter: genau der (--adapter n); ohne: der
/// Hardware-Adapter, den D3D waehlt, oder WARP fuer den Test.
pub(crate) fn geraet_bauen(
    adapter: Option<&IDXGIAdapter1>,
    warp: bool,
    debug: bool,
) -> Result<(ID3D11Device, ID3D11DeviceContext, D3D_FEATURE_LEVEL), String> {
    let typ = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else if warp {
        D3D_DRIVER_TYPE_WARP
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    let mut flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    if debug {
        flags = flags | D3D11_CREATE_DEVICE_DEBUG;
    }
    let stufen = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
    let basis: Option<IDXGIAdapter> = adapter.and_then(|a| a.cast().ok());
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    let mut fl = D3D_FEATURE_LEVEL(0);
    unsafe {
        D3D11CreateDevice(
            basis.as_ref(),
            typ,
            HMODULE::default(),
            flags,
            Some(&stufen),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut fl),
            Some(&mut ctx),
        )
    }
    .map_err(|e| fehler("D3D11CreateDevice", e))?;
    match (device, ctx) {
        (Some(d), Some(c)) => Ok((d, c, fl)),
        _ => Err("D3D11CreateDevice lieferte kein Geraet".into()),
    }
}

/// Text eines Blobs (Fehlermeldungen des Shader-Uebersetzers).
fn blob_text(b: &ID3DBlob) -> String {
    unsafe {
        let s = std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize());
        String::from_utf8_lossy(s).trim_end_matches('\0').trim().to_string()
    }
}

/// Einen Einstiegspunkt aus SHADER uebersetzen.
fn uebersetzen(einstieg: &[u8], ziel: &[u8]) -> Result<Vec<u8>, String> {
    uebersetzen_aus(SHADER, b"anzeige.hlsl\0", einstieg, ziel)
}

/// Einen Einstiegspunkt aus einer HLSL-Quelle uebersetzen - der Anzeige
/// oder einer anderen (Wandler des Hosts, hdr_hlsl.rs); `datei` ist der
/// Name in den Meldungen, wie `einstieg` und `ziel` mit Nullbyte. Der
/// Fehlertext des Uebersetzers geht wortwoertlich ins Protokoll und in den
/// Err - er nennt Zeile und Spalte, und ohne ihn waere ein Tippfehler im
/// HLSL unauffindbar.
pub(crate) fn uebersetzen_aus(quelle: &str, datei: &[u8], einstieg: &[u8], ziel: &[u8]) -> Result<Vec<u8>, String> {
    let name = String::from_utf8_lossy(&einstieg[..einstieg.len() - 1]).into_owned();
    let mut code: Option<ID3DBlob> = None;
    let mut meldung: Option<ID3DBlob> = None;
    let r = unsafe {
        D3DCompile(
            quelle.as_ptr() as *const c_void,
            quelle.len(),
            PCSTR(datei.as_ptr()),
            None,
            None,
            PCSTR(einstieg.as_ptr()),
            PCSTR(ziel.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut meldung),
        )
    };
    let text = meldung.as_ref().map(blob_text).unwrap_or_default();
    match (r, code) {
        (Ok(()), Some(b)) => {
            if !text.is_empty() {
                protokoll::zeile(format!("Shader {name}: {text}"));
            }
            Ok(unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }.to_vec())
        }
        (r, _) => {
            let grund = match r {
                Err(e) => fehler("D3DCompile", e),
                Ok(()) => "D3DCompile lieferte keinen Code".into(),
            };
            let m = format!("Shader {name} laesst sich nicht uebersetzen: {grund}{}", if text.is_empty() { String::new() } else { format!("\n{text}") });
            protokoll::zeile(m.clone());
            Err(m)
        }
    }
}

fn viewport(w: u32, h: u32) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }
}

impl Gpu {
    /// Ohne Fenster: Geraet und Shader, keine Swapchain. Fuer --anzeigetest.
    /// `warp`: Software-Rasterizer statt Karte (VM ohne GPU). Zuerst mit der
    /// Debug-Schicht, die Verstoesse gegen die API meldet; fehlt sie (keine
    /// "Grafiktools" installiert), ohne.
    pub fn ohne_fenster(warp: bool) -> Result<Gpu, String> {
        let (device, ctx, fl) = match geraet_bauen(None, warp, true) {
            Ok(g) => g,
            Err(_) => geraet_bauen(None, warp, false)?,
        };
        let gpu = Gpu::aus_geraet(device, ctx, fl)?;
        protokoll::zeile(format!(
            "Anzeige: D3D11 {} ({}), FL {}, Ausgang {}",
            gpu.adapter.name,
            if gpu.adapter.software { "Software" } else { "Hardware" },
            gpu.feature_level_name(),
            if gpu.adapter.hat_ausgang { "ja" } else { "nein" }
        ));
        Ok(gpu)
    }

    /// Geraet und Flip-Swapchain am Fenster. `hwnd` ist der rohe Win32-Griff
    /// aus winit. `warp`: Software-Rasterizer (nur zum Pruefen); sonst zaehlt
    /// ein Software-Adapter als "keine Karte" und ist ein Fehler - der
    /// Aufrufer faellt dann auf softbuffer zurueck. `adapter_idx`: --adapter n
    /// nimmt genau diesen Adapter, was immer er ist. Automatik: der erste
    /// Hardware-Adapter mit Ausgang; ohne einen solchen (Optimus ohne MUX)
    /// der erste von NVIDIA, sonst der erste ueberhaupt.
    pub fn neu(hwnd: isize, ww: u32, wh: u32, warp: bool, adapter_idx: Option<u32>) -> Result<Gpu, String> {
        let hwnd = HWND(hwnd as *mut c_void);
        if hwnd.0.is_null() {
            return Err("kein Fenstergriff".into());
        }
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }.map_err(|e| fehler("CreateDXGIFactory1", e))?;
        let factory1: IDXGIFactory1 = factory.cast().map_err(|e| fehler("IDXGIFactory1", e))?;
        let alle = adapter_objekte(&factory1)?;
        for (i, (_, a)) in alle.iter().enumerate() {
            protokoll::zeile(adapter_zeile(i, a));
        }
        let (device, ctx, fl) = if warp {
            geraet_bauen(None, true, false)?
        } else {
            let wahl = match adapter_idx {
                Some(n) => alle.get(n as usize).ok_or_else(|| format!("Adapter {n} gibt es nicht ({} gefunden)", alle.len()))?,
                None => {
                    let hardware: Vec<&(IDXGIAdapter1, AdapterInfo)> = alle.iter().filter(|(_, a)| !a.software).collect();
                    hardware
                        .iter()
                        .find(|(_, a)| a.hat_ausgang)
                        .or_else(|| hardware.iter().find(|(_, a)| a.vendor == 0x10de))
                        .or_else(|| hardware.first())
                        .copied()
                        .ok_or_else(|| {
                            let namen: Vec<&str> = alle.iter().map(|(_, a)| a.name.as_str()).collect();
                            format!("kein Hardware-Adapter (gefunden: {})", if namen.is_empty() { "keiner".to_string() } else { namen.join(", ") })
                        })?
                }
            };
            geraet_bauen(Some(&wahl.0), false, false)?
        };
        let mut gpu = Gpu::aus_geraet(device, ctx, fl)?;
        gpu.tearing = tearing_moeglich();
        gpu.flags = (DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 | if gpu.tearing { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 } else { 0 }) as u32;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: ww,
            Height: wh,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: gpu.flags,
        };
        let swapchain: IDXGISwapChain2 = unsafe { factory.CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None) }
            .map_err(|e| fehler("CreateSwapChainForHwnd", e))?
            .cast()
            .map_err(|e| fehler("IDXGISwapChain2", e))?;
        // Sonst schaltet Alt+Enter DXGI in den exklusiven Vollbildmodus,
        // statt als Tastendruck zum Mac zu gehen.
        if let Err(e) = unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES) } {
            protokoll::zeile(fehler("MakeWindowAssociation", e));
        }
        // Nie mehr als ein Bild ueber das gezeigte hinaus unterwegs.
        unsafe { swapchain.SetMaximumFrameLatency(1) }.map_err(|e| fehler("SetMaximumFrameLatency", e))?;
        let warte = unsafe { swapchain.GetFrameLatencyWaitableObject() };
        if warte.0.is_null() {
            return Err("GetFrameLatencyWaitableObject lieferte nichts".into());
        }
        gpu.swapchain = Some(swapchain);
        gpu.warte = warte;
        gpu.breite = ww;
        gpu.hoehe = wh;
        gpu.backbuffer_sicht()?;
        protokoll::zeile(format!(
            "Anzeige: D3D11 {}{}, FL {}, Tearing {}, Ausgang {}, Latenz 1",
            gpu.adapter.name,
            if gpu.adapter.software { " (Software)" } else { "" },
            gpu.feature_level_name(),
            if gpu.tearing { "ja" } else { "nein" },
            if gpu.adapter.hat_ausgang { "ja" } else { "nein" }
        ));
        Ok(gpu)
    }

    /// Sicht auf den Backbuffer der Swapchain anlegen.
    fn backbuffer_sicht(&mut self) -> Result<(), String> {
        let sc = self.swapchain.as_ref().ok_or("keine Swapchain")?;
        let tex: ID3D11Texture2D = unsafe { sc.GetBuffer(0) }.map_err(|e| fehler("GetBuffer", e))?;
        self.rtv = Some(self.rtv_anlegen(&tex)?);
        Ok(())
    }

    /// Das Fenster hat eine andere Groesse: Sicht auf den Backbuffer fallen
    /// lassen, Puffer der Swapchain neu, Sicht neu. Die Oberflaechentextur
    /// wird beim naechsten Upload in der neuen Groesse angelegt. 0x0
    /// (minimiert): nichts tun - der Aufrufer zeichnet dann auch nichts.
    pub fn groesse(&mut self, ww: u32, wh: u32) -> Result<(), String> {
        if ww == 0 || wh == 0 || (ww, wh) == (self.breite, self.hoehe) {
            return Ok(());
        }
        let Some(sc) = self.swapchain.clone() else { return Err("keine Swapchain".into()) };
        self.sichten_loesen();
        self.rtv = None;
        self.oberflaeche = None;
        // ResizeBuffers verlangt, dass niemand mehr einen Backbuffer haelt -
        // die Freigabe der Sicht soll verarbeitet sein, bevor es losgeht.
        // Schlaegt es fehl, gilt keine Groesse als eingestellt - der naechste
        // Takt versucht es wieder, statt fuer immer ohne Sicht zu zeichnen.
        self.breite = 0;
        self.hoehe = 0;
        unsafe {
            self.ctx.Flush();
            sc.ResizeBuffers(0, ww, wh, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(self.flags as i32))
        }
        .map_err(|e| fehler(&format!("ResizeBuffers {ww}x{wh}"), e))?;
        self.backbuffer_sicht()?;
        self.breite = ww;
        self.hoehe = wh;
        Ok(())
    }

    /// Praesentiert die Swapchain gerade in HDR (G2084)? Noch nie: sie ist
    /// immer B8G8R8A8/SDR, siehe HDR_DARSTELLUNG.
    pub fn hdr_praesentiert(&self) -> bool {
        false
    }

    /// Ist DXGI bereit fuer ein weiteres Bild? Fragt das Warteobjekt mit
    /// Timeout 0 - nie warten, der Fensterfaden traegt auch die Eingabe.
    /// Das Objekt ist ein Semaphor: wer es einmal erfolgreich abgefragt hat,
    /// haelt einen Zaehler, und den gibt DXGI erst nach einem gelungenen
    /// Present zurueck. Deshalb wird ein gehaltener Zaehler gemerkt - scheitert
    /// das Zeichnen dazwischen, fragt der naechste Takt nicht noch einmal
    /// (er bekaeme nie mehr ein Ja) sondern praesentiert. Ohne Swapchain immer ja.
    pub fn bereit(&mut self) -> bool {
        if self.warte.0.is_null() || self.warte_gehalten {
            return true;
        }
        let frei = unsafe { WaitForSingleObject(self.warte, 0) == WAIT_OBJECT_0 };
        if frei {
            self.warte_gehalten = true;
        }
        frei
    }

    /// Ist das Geraet weg? Dann der Grund als Text (GetDeviceRemovedReason).
    pub fn geraet_weg(&self) -> Option<String> {
        unsafe { self.device.GetDeviceRemovedReason() }.err().map(|e| format!("{} (0x{:08x})", e.message().trim(), e.code().0 as u32))
    }

    /// Ein Fehler von Map/Draw: war es das Geraet, oder nur dieser Aufruf?
    fn einordnen(&self, e: String) -> Praesentiert {
        match self.geraet_weg() {
            Some(grund) => Praesentiert::GeraetWeg(grund),
            None => Praesentiert::Fehler(e),
        }
    }

    /// Stufe 2 auf den Backbuffer und Present mit SyncInterval 0. `sofort`:
    /// mit ALLOW_TEARING, wenn die Swapchain es kann - sonst zum naechsten
    /// Bildwechsel, aber ohne Warteschlange. Das HRESULT wird von Hand
    /// gelesen: OCCLUDED ist kein Fehler, DEVICE_REMOVED/RESET ist das Ende
    /// dieses Geraets.
    pub fn zeichnen(&mut self, rect: Option<(i32, i32, u32, u32)>, ui_an: bool, sofort: bool) -> Praesentiert {
        let Some(sc) = self.swapchain.clone() else { return Praesentiert::Fehler("keine Swapchain".into()) };
        let Some(rtv) = self.rtv.clone() else { return Praesentiert::Fehler("keine Sicht auf den Backbuffer".into()) };
        if let Err(e) = self.stufe2(&rtv, self.breite, self.hoehe, rect, ui_an) {
            return self.einordnen(e);
        }
        let reissen = sofort && self.tearing;
        let flags = if reissen { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
        let hr = unsafe { sc.Present(0, flags) };
        // Ein gelungenes Present gibt den Zaehler des Warteobjekts frei.
        if hr == S_OK || hr == DXGI_STATUS_OCCLUDED {
            self.warte_gehalten = false;
        }
        if hr == S_OK {
            Praesentiert::Ok
        } else if hr == DXGI_STATUS_OCCLUDED {
            Praesentiert::Verdeckt
        } else if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
            Praesentiert::GeraetWeg(self.geraet_weg().unwrap_or_else(|| format!("Present: 0x{:08x}", hr.0 as u32)))
        } else if hr == DXGI_ERROR_INVALID_CALL && reissen {
            // DXGI hatte Tearing zugesagt und nimmt es doch nicht: ab jetzt
            // ohne, statt jedes Bild zu verlieren.
            self.tearing = false;
            Praesentiert::Fehler("Present mit ALLOW_TEARING abgelehnt - ab jetzt bildsynchron".into())
        } else {
            Praesentiert::Fehler(format!("Present: 0x{:08x}", hr.0 as u32))
        }
    }

    /// Der gemeinsame Rest: Shader, Zustaende, Konstantenpuffer, Adapter.
    /// Ohne Swapchain - `neu` haengt sie danach an.
    fn aus_geraet(device: ID3D11Device, ctx: ID3D11DeviceContext, fl: D3D_FEATURE_LEVEL) -> Result<Gpu, String> {
        let vs_code = uebersetzen(b"vs_main\0", b"vs_4_0\0")?;
        let ps1_code = uebersetzen(b"ps_umrechnen\0", b"ps_4_0\0")?;
        let ps2_code = uebersetzen(b"ps_anzeigen\0", b"ps_4_0\0")?;
        let mut vs = None;
        let mut ps_umrechnen = None;
        let mut ps_anzeigen = None;
        let mut sampler = None;
        let mut raster = None;
        unsafe {
            device.CreateVertexShader(&vs_code, None, Some(&mut vs)).map_err(|e| fehler("CreateVertexShader", e))?;
            device.CreatePixelShader(&ps1_code, None, Some(&mut ps_umrechnen)).map_err(|e| fehler("CreatePixelShader umrechnen", e))?;
            device.CreatePixelShader(&ps2_code, None, Some(&mut ps_anzeigen)).map_err(|e| fehler("CreatePixelShader anzeigen", e))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                BorderColor: [0.0; 4],
                MinLOD: 0.0,
                MaxLOD: D3D11_FLOAT32_MAX,
            };
            device.CreateSamplerState(&sd, Some(&mut sampler)).map_err(|e| fehler("CreateSamplerState", e))?;
            let rd = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                FrontCounterClockwise: BOOL(0),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: BOOL(1),
                ScissorEnable: BOOL(0),
                MultisampleEnable: BOOL(0),
                AntialiasedLineEnable: BOOL(0),
            };
            device.CreateRasterizerState(&rd, Some(&mut raster)).map_err(|e| fehler("CreateRasterizerState", e))?;
        }
        let konst_format = konstpuffer(&device, std::mem::size_of::<KonstFormat>() as u32)?;
        let konst_anzeige = konstpuffer(&device, std::mem::size_of::<KonstAnzeige>() as u32)?;
        let adapter = adapter_des_geraets(&device);
        let gpu = Gpu {
            device,
            ctx,
            swapchain: None,
            rtv: None,
            warte: HANDLE::default(),
            warte_gehalten: false,
            flags: 0,
            breite: 0,
            hoehe: 0,
            tearing: false,
            vs: vs.ok_or("CreateVertexShader lieferte nichts")?,
            ps_umrechnen: ps_umrechnen.ok_or("CreatePixelShader lieferte nichts")?,
            ps_anzeigen: ps_anzeigen.ok_or("CreatePixelShader lieferte nichts")?,
            sampler: sampler.ok_or("CreateSamplerState lieferte nichts")?,
            raster: raster.ok_or("CreateRasterizerState lieferte nichts")?,
            konst_format,
            konst_anzeige,
            ebenen: None,
            zwischen: None,
            oberflaeche: None,
            feature_level: fl,
            adapter,
        };
        Ok(gpu)
    }

    /// Feature-Level als Text, wie er im Protokoll steht.
    pub fn feature_level_name(&self) -> String {
        let fl = self.feature_level;
        if fl == D3D_FEATURE_LEVEL_11_1 {
            "11.1".into()
        } else if fl == D3D_FEATURE_LEVEL_11_0 {
            "11.0".into()
        } else if fl == D3D_FEATURE_LEVEL_10_1 {
            "10.1".into()
        } else if fl == D3D_FEATURE_LEVEL_10_0 {
            "10.0".into()
        } else {
            format!("0x{:x}", fl.0)
        }
    }

    fn textur_anlegen(&self, w: u32, h: u32, format: DXGI_FORMAT, usage: D3D11_USAGE, bind: u32, cpu: u32) -> Result<ID3D11Texture2D, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }
            .map_err(|e| fehler(&format!("CreateTexture2D {w}x{h} Format {}", format.0), e))?;
        tex.ok_or_else(|| "CreateTexture2D lieferte nichts".into())
    }

    fn srv_anlegen(&self, tex: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView, String> {
        let mut srv = None;
        unsafe { self.device.CreateShaderResourceView(tex, None, Some(&mut srv)) }.map_err(|e| fehler("CreateShaderResourceView", e))?;
        srv.ok_or_else(|| "CreateShaderResourceView lieferte nichts".into())
    }

    fn rtv_anlegen(&self, tex: &ID3D11Texture2D) -> Result<ID3D11RenderTargetView, String> {
        let mut rtv = None;
        unsafe { self.device.CreateRenderTargetView(tex, None, Some(&mut rtv)) }.map_err(|e| fehler("CreateRenderTargetView", e))?;
        rtv.ok_or_else(|| "CreateRenderTargetView lieferte nichts".into())
    }

    /// Eine Eingangsebene: DYNAMIC, von der CPU per Map beschreibbar.
    fn ebene_textur(&self, w: u32, h: u32, format: DXGI_FORMAT) -> Result<Textur, String> {
        let tex = self.textur_anlegen(
            w, h, format, D3D11_USAGE_DYNAMIC,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
            D3D11_CPU_ACCESS_WRITE.0 as u32,
        )?;
        let srv = self.srv_anlegen(&tex)?;
        Ok(Textur { tex, srv, w, h })
    }

    /// Nichts mehr an den Pixel-Shader binden - vor jedem Neuanlegen von
    /// Texturen, damit keine Sicht auf eine Textur bleibt, die gleich weg ist,
    /// und vor jeder Stufe, deren Ziel in der vorigen Quelle war.
    fn sichten_loesen(&self) {
        unsafe {
            self.ctx.PSSetShaderResources(0, Some(&[None, None, None, None, None, None]));
            self.ctx.OMSetRenderTargets(None, None);
        }
    }

    /// Zwischentextur in Bildgroesse, neu nur wenn die Groesse nicht stimmt.
    fn zwischen_sichern(&mut self, w: u32, h: u32) -> Result<(), String> {
        if self.zwischen.as_ref().map(|z| (z.w, z.h)) == Some((w, h)) {
            return Ok(());
        }
        self.sichten_loesen();
        self.zwischen = None;
        let tex = self.textur_anlegen(
            w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT,
            (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            0,
        )?;
        let srv = self.srv_anlegen(&tex)?;
        let rtv = self.rtv_anlegen(&tex)?;
        self.zwischen = Some(Zwischen { tex, srv, rtv, w, h });
        Ok(())
    }

    /// Eingangstexturen fuer ein Decoderformat anlegen (Tabelle in
    /// `ebenen_format`): Y immer w x h; U/V bei 4:2:0 (w+1)/2 x (h+1)/2;
    /// bei Paaren eine zweikanalige Textur statt zweier.
    fn ebenen_anlegen(&mut self, schluessel: (ffmpeg::format::Pixel, u32, u32), fmt: EbenenFormat, cw: u32, ch: u32) -> Result<(), String> {
        let (_, w, h) = schluessel;
        self.sichten_loesen();
        self.ebenen = None;
        let (einzeln, paar) = if fmt.bits == 8 {
            (DXGI_FORMAT_R8_UINT, DXGI_FORMAT_R8G8_UINT)
        } else {
            (DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R16G16_UINT)
        };
        let y = self.ebene_textur(w, h, einzeln)?;
        let (u, v, uv) = if fmt.paar {
            (None, None, Some(self.ebene_textur(cw, ch, paar)?))
        } else {
            (Some(self.ebene_textur(cw, ch, einzeln)?), Some(self.ebene_textur(cw, ch, einzeln)?), None)
        };
        self.ebenen = Some(Ebenen { schluessel, fmt, y, u, v, uv });
        self.zwischen_sichern(w, h)?;
        protokoll::zeile(format!(
            "Ebenen neu: {:?} {w}x{h}, Farbebenen {cw}x{ch}, {} Bit{}",
            schluessel.0, fmt.bits,
            if fmt.paar { ", U/V verschraenkt" } else { "" }
        ));
        Ok(())
    }

    /// Eine Ebene in ihre Textur kopieren. Die Zeilenschrittweite der Textur
    /// (RowPitch) ist nie als dicht anzunehmen und der Stride des Decoders
    /// nie als Breite: sind beide gleich, reicht EIN memcpy, sonst geht es
    /// zeilenweise. Die Laengenpruefung davor ist dieselbe wie in `to_rgb` -
    /// ohne sie laese der memcpy hinter das Ende der Ebene.
    fn ebene_fuellen(&self, tex: &ID3D11Texture2D, quelle: &[u8], stride: usize, zeilenbytes: usize, zeilen: usize) -> Result<usize, String> {
        if zeilen == 0 || quelle.len() < (zeilen - 1) * stride + zeilenbytes {
            return Err("Bildebenen des Decoders sind zu klein".into());
        }
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx.Map(tex, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).map_err(|e| fehler("Map Ebene", e))?;
            let ziel = m.pData as *mut u8;
            let rp = m.RowPitch as usize;
            if rp == stride {
                std::ptr::copy_nonoverlapping(quelle.as_ptr(), ziel, (zeilen - 1) * stride + zeilenbytes);
            } else {
                for z in 0..zeilen {
                    std::ptr::copy_nonoverlapping(quelle.as_ptr().add(z * stride), ziel.add(z * rp), zeilenbytes);
                }
            }
            self.ctx.Unmap(tex, 0);
        }
        Ok(m.RowPitch as usize)
    }

    /// Ein rohes Decoderbild: Ebenen (bei Bedarf neu) anlegen, hochladen,
    /// Stufe 1 in die Zwischentextur. Das Bild darf danach fallen.
    pub fn bild_roh(&mut self, f: &ffmpeg::frame::Video) -> Result<(), String> {
        let Some(fmt) = ebenen_format(f.format()) else {
            return Err(format!("Unbekanntes Bildformat vom Decoder: {:?}", f.format()));
        };
        let (w, h) = (f.width(), f.height());
        if w == 0 || h == 0 {
            return Err("Decoder liefert ein leeres Bild".into());
        }
        if f.planes() < if fmt.paar { 2 } else { 3 } {
            return Err("Decoder liefert zu wenige Bildebenen".into());
        }
        let cw = if fmt.sub { (w + 1) / 2 } else { w };
        let ch = if fmt.sub { (h + 1) / 2 } else { h };
        let bpp = fmt.bpp() as usize;
        let schluessel = (f.format(), w, h);
        let neu = self.ebenen.as_ref().map(|e| e.schluessel) != Some(schluessel);
        if neu {
            self.ebenen_anlegen(schluessel, fmt, cw, ch)?;
        }
        let e = self.ebenen.as_ref().ok_or("Ebenen fehlen")?;
        let ry = self.ebene_fuellen(&e.y.tex, f.data(0), f.stride(0), w as usize * bpp, h as usize)?;
        let (r1, r2) = if fmt.paar {
            let uv = e.uv.as_ref().ok_or("UV-Ebene fehlt")?;
            (self.ebene_fuellen(&uv.tex, f.data(1), f.stride(1), cw as usize * 2 * bpp, ch as usize)?, 0)
        } else {
            let u = e.u.as_ref().ok_or("U-Ebene fehlt")?;
            let v = e.v.as_ref().ok_or("V-Ebene fehlt")?;
            (
                self.ebene_fuellen(&u.tex, f.data(1), f.stride(1), cw as usize * bpp, ch as usize)?,
                self.ebene_fuellen(&v.tex, f.data(2), f.stride(2), cw as usize * bpp, ch as usize)?,
            )
        };
        if neu {
            // Einmal je Formatwechsel: Zeilenschrittweite des Decoders gegen
            // die der Textur. Gleich = ein memcpy je Ebene, sonst zeilenweise.
            // Was der Laptop mit cuvid liefert, steht damit im Protokoll.
            protokoll::zeile(format!(
                "Ebenen: Zeilen Decoder {}/{}/{} Byte, Textur {}/{}/{} Byte",
                f.stride(0), f.stride(1), if fmt.paar { 0 } else { f.stride(2) }, ry, r1, r2
            ));
        }
        self.stufe1()
    }

    /// Ein fertiges RGB-Bild (Rueckfall, dunkles Bild): direkt in die
    /// Zwischentextur. 0x00RRGGBB liegt little-endian als B,G,R,0 - genau
    /// B8G8R8A8, der Shader liest nur .rgb.
    pub fn bild_rgb(&mut self, f: &Frame) -> Result<(), String> {
        let (w, h) = (f.width, f.height);
        if w == 0 || h == 0 || f.pixels.len() < (w as usize) * (h as usize) {
            return Err("RGB-Bild ist leer oder zu klein".into());
        }
        self.zwischen_sichern(w, h)?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        unsafe {
            self.ctx.UpdateSubresource(&z.tex, 0, None, f.pixels.as_ptr() as *const c_void, w * 4, 0);
        }
        Ok(())
    }

    /// Die Oberflaeche (0xTTRRGGBB, Zielgroesse) in ihre Textur laden - nur
    /// den Kasten, den die Canvas beschrieben hat. Ist die Textur neu (erste
    /// Zeichnung, andere Groesse), alles.
    pub fn oberflaeche_hochladen(&mut self, puffer: &[u32], ww: u32, wh: u32, kasten: ui::Rect) -> Result<(), String> {
        if ww == 0 || wh == 0 || puffer.len() < (ww as usize) * (wh as usize) {
            return Err("Oberflaechenpuffer passt nicht zur Groesse".into());
        }
        let neu = self.oberflaeche.as_ref().map(|t| (t.w, t.h)) != Some((ww, wh));
        if neu {
            self.sichten_loesen();
            self.oberflaeche = None;
            let tex = self.textur_anlegen(ww, wh, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, D3D11_BIND_SHADER_RESOURCE.0 as u32, 0)?;
            let srv = self.srv_anlegen(&tex)?;
            self.oberflaeche = Some(Textur { tex, srv, w: ww, h: wh });
        }
        let t = self.oberflaeche.as_ref().ok_or("Oberflaechentextur fehlt")?;
        let (x0, y0, x1, y1) = if neu {
            (0, 0, ww, wh)
        } else {
            (
                kasten.x.max(0) as u32,
                kasten.y.max(0) as u32,
                kasten.x.saturating_add(kasten.w).clamp(0, ww as i32) as u32,
                kasten.y.saturating_add(kasten.h).clamp(0, wh as i32) as u32,
            )
        };
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let kasten = D3D11_BOX { left: x0, top: y0, front: 0, right: x1, bottom: y1, back: 1 };
        unsafe {
            self.ctx.UpdateSubresource(
                &t.tex, 0, Some(&kasten),
                puffer.as_ptr().add((y0 * ww + x0) as usize) as *const c_void,
                ww * 4, 0,
            );
        }
        Ok(())
    }

    fn konstanten_schreiben<T>(&self, puffer: &ID3D11Buffer, wert: &T) -> Result<(), String> {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx.Map(puffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).map_err(|e| fehler("Map Konstanten", e))?;
            std::ptr::copy_nonoverlapping(wert as *const T as *const u8, m.pData as *mut u8, std::mem::size_of::<T>());
            self.ctx.Unmap(puffer, 0);
        }
        Ok(())
    }

    /// Stufe 1: Ebenen -> Zwischentextur.
    fn stufe1(&self) -> Result<(), String> {
        let e = self.ebenen.as_ref().ok_or("Ebenen fehlen")?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        self.sichten_loesen();
        self.konstanten_schreiben(
            &self.konst_format,
            &KonstFormat { sub: e.fmt.sub as u32, paar: e.fmt.paar as u32, schieb: e.fmt.schieb(), _f: 0 },
        )?;
        unsafe {
            self.ctx.OMSetRenderTargets(Some(&[Some(z.rtv.clone())]), None);
            self.ctx.RSSetViewports(Some(&[viewport(z.w, z.h)]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(&self.ps_umrechnen, None);
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.konst_format.clone())]));
            self.ctx.PSSetShaderResources(
                0,
                Some(&[
                    Some(e.y.srv.clone()),
                    e.u.as_ref().map(|t| t.srv.clone()),
                    e.v.as_ref().map(|t| t.srv.clone()),
                    e.uv.as_ref().map(|t| t.srv.clone()),
                ]),
            );
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.ctx.Draw(3, 0);
            self.ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// Stufe 2 in ein Ziel (ww x wh): Grundfarbe, Bild im Rechteck, darueber
    /// die Oberflaeche. Das Ziel ist der Backbuffer der Swapchain (`zeichnen`)
    /// oder die Testtextur (`offscreen`).
    fn stufe2(&self, ziel: &ID3D11RenderTargetView, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<(), String> {
        let bild = self.zwischen.as_ref();
        let (rect, bild_da) = match (rect, bild) {
            (Some(r), Some(_)) if r.2 > 0 && r.3 > 0 => (r, true),
            _ => ((0, 0, 0, 0), false),
        };
        let (fw, fh) = bild.map(|z| (z.w, z.h)).unwrap_or((1, 1));
        // Derselbe Schritt wie in blit: (fw << 16) / zw, abgeschnitten. Bei
        // 4K sind das 251 Millionen - passt in 32 Bit, auch multipliziert
        // mit dem Zielpunkt (der bleibt unter zw).
        let schritt = if bild_da {
            [(((fw as u64) << 16) / rect.2 as u64) as u32, (((fh as u64) << 16) / rect.3 as u64) as u32]
        } else {
            [0, 0]
        };
        let ui_da = ui_an && self.oberflaeche.is_some();
        let bg = ui::BG;
        let k = KonstAnzeige {
            rect: [rect.0, rect.1, rect.2 as i32, rect.3 as i32],
            bild: [fw, fh],
            schritt,
            modus: (bild_da as u32) | ((ui_da as u32) << 1),
            _a: [0; 3],
            grund: [((bg >> 16) & 255) as f32 / 255.0, ((bg >> 8) & 255) as f32 / 255.0, (bg & 255) as f32 / 255.0, 1.0],
        };
        self.sichten_loesen();
        self.konstanten_schreiben(&self.konst_anzeige, &k)?;
        unsafe {
            self.ctx.OMSetRenderTargets(Some(&[Some(ziel.clone())]), None);
            self.ctx.RSSetViewports(Some(&[viewport(ww, wh)]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(&self.ps_anzeigen, None);
            self.ctx.PSSetConstantBuffers(1, Some(&[Some(self.konst_anzeige.clone())]));
            self.ctx.PSSetShaderResources(
                4,
                Some(&[bild.map(|z| z.srv.clone()), self.oberflaeche.as_ref().map(|t| t.srv.clone())]),
            );
            self.ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.ctx.Draw(3, 0);
            self.ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }

    /// Eine Textur ueber eine STAGING-Kopie in den Hauptspeicher lesen, als
    /// 0x00RRGGBB (Alphabyte maskiert). Nur fuer den Test - das wartet auf
    /// die Karte.
    fn auslesen(&self, tex: &ID3D11Texture2D, w: u32, h: u32) -> Result<Vec<u32>, String> {
        let staging = self.textur_anlegen(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
        let mut aus = vec![0u32; (w as usize) * (h as usize)];
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx.CopyResource(&staging, tex);
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).map_err(|e| fehler("Map Staging", e))?;
            let quelle = m.pData as *const u8;
            let rp = m.RowPitch as usize;
            let zb = w as usize * 4;
            for z in 0..h as usize {
                std::ptr::copy_nonoverlapping(quelle.add(z * rp), (aus.as_mut_ptr() as *mut u8).add(z * zb), zb);
            }
            self.ctx.Unmap(&staging, 0);
        }
        for p in aus.iter_mut() {
            *p &= 0x00ff_ffff;
        }
        Ok(aus)
    }

    /// Nur Test: Stufe 2 in eine eigene Zieltextur, dann zurueck in den
    /// Hauptspeicher.
    pub fn offscreen(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<Vec<u32>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        let ziel = self.textur_anlegen(ww, wh, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let rtv = self.rtv_anlegen(&ziel)?;
        self.stufe2(&rtv, ww, wh, rect, ui_an)?;
        self.auslesen(&ziel, ww, wh)
    }

    /// Nur Test: das Ergebnis von Stufe 1, 1:1.
    pub fn zwischen_auslesen(&mut self) -> Result<(Vec<u32>, u32, u32), String> {
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        Ok((self.auslesen(&z.tex, z.w, z.h)?, z.w, z.h))
    }
}

impl Drop for Gpu {
    /// Erst alles vom Kontext loesen, dann die Sichten, dann die Swapchain
    /// (Feldreihenfolge) - so haelt nichts mehr einen Puffer fest, und ein
    /// Neubau am selben Fenster findet das HWND frei. Das Warteobjekt ist
    /// ein eigener Griff und wird geschlossen.
    fn drop(&mut self) {
        unsafe {
            self.ctx.ClearState();
            self.rtv = None;
            self.ctx.Flush();
            if !self.warte.0.is_null() {
                let _ = CloseHandle(self.warte);
            }
        }
    }
}

fn konstpuffer(device: &ID3D11Device, bytes: u32) -> Result<ID3D11Buffer, String> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: bytes,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut b = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut b)) }.map_err(|e| fehler("CreateBuffer", e))?;
    b.ok_or_else(|| "CreateBuffer lieferte nichts".into())
}

/// Der Adapter, auf dem das Geraet wirklich laeuft - bei WARP der
/// "Microsoft Basic Render Driver".
fn adapter_des_geraets(device: &ID3D11Device) -> AdapterInfo {
    let info = (|| -> Result<AdapterInfo, windows::core::Error> {
        let d: IDXGIDevice = device.cast()?;
        let a: IDXGIAdapter1 = unsafe { d.GetAdapter()? }.cast()?;
        adapter_info(&a)
    })();
    info.unwrap_or_else(|_| AdapterInfo { name: "unbekannt".into(), vendor: 0, software: false, hat_ausgang: false, luid: 0, speicher_mb: 0 })
}

// ------------------------------------------------------------ --anzeigetest

/// Groesster Wert je Ebene, wie ihn der jeweilige Decoder liefert: 8 Bit
/// 255, 10 Bit LE 1023, oben buendig 65472 (10 Bit << 6), 65520 (12 Bit
/// << 4) oder 65535 (16 Bit).
fn hoechstwert(p: ffmpeg::format::Pixel, fmt: EbenenFormat) -> u32 {
    use ffmpeg::format::Pixel;
    match fmt.bits {
        8 => 255,
        10 => 1023,
        _ => match p {
            Pixel::YUV444P10MSBLE | Pixel::P010LE => 65472,
            Pixel::YUV444P12MSBLE | Pixel::P012LE => 65520,
            _ => 65535,
        },
    }
}

/// Synthetisches Bild: Verlaeufe in allen drei Ebenen, ein Streifen harter
/// Kanten (Schachbrett in ungeraden Feldgroessen, damit die Felder nicht auf
/// den 2x2-Farbraster fallen) und in den Ecken die Extremwerte 0 und
/// Maximum, je Ebene anders verteilt, damit das Abschneiden auf 0..255 in
/// jeder Richtung vorkommt. Der Allokator von FFmpeg legt die Zeilen breiter
/// an als das Bild - damit prueft der Test die zeilenweise Kopie.
fn probebild(p: ffmpeg::format::Pixel, w: u32, h: u32) -> Option<ffmpeg::frame::Video> {
    let fmt = ebenen_format(p)?;
    let max = hoechstwert(p, fmt);
    let mut f = ffmpeg::frame::Video::new(p, w, h);
    let wert = |x: usize, y: usize, bw: usize, bh: usize, ebene: usize| -> u32 { probewert(x, y, bw, bh, ebene, max) };
    let (w, h) = (w as usize, h as usize);
    let bpp = fmt.bpp() as usize;
    let cw = if fmt.sub { (w + 1) / 2 } else { w };
    let ch = if fmt.sub { (h + 1) / 2 } else { h };
    let schreib = |d: &mut [u8], i: usize, v: u32| {
        if bpp == 1 {
            d[i] = v as u8;
        } else {
            d[i..i + 2].copy_from_slice(&(v as u16).to_le_bytes());
        }
    };
    {
        let s = f.stride(0);
        let d = f.data_mut(0);
        for y in 0..h {
            for x in 0..w {
                schreib(d, y * s + x * bpp, wert(x, y, w, h, 0));
            }
        }
    }
    if fmt.paar {
        let s = f.stride(1);
        let d = f.data_mut(1);
        for y in 0..ch {
            for x in 0..cw {
                schreib(d, y * s + 2 * x * bpp, wert(x, y, cw, ch, 1));
                schreib(d, y * s + (2 * x + 1) * bpp, wert(x, y, cw, ch, 2));
            }
        }
    } else {
        for e in 1..3 {
            let s = f.stride(e);
            let d = f.data_mut(e);
            for y in 0..ch {
                for x in 0..cw {
                    schreib(d, y * s + x * bpp, wert(x, y, cw, ch, e));
                }
            }
        }
    }
    Some(f)
}

/// Ein Format in einer Groesse: (a) `to_rgb` als Referenz, (b) rohes Bild
/// ueber Stufe 1 gegen die Referenz (Toleranz 0), dazu der RGB-Upload
/// (Toleranz 0), (c) skaliert in 700x400 gegen `blit` (Toleranz 2: der
/// Filter der Karte rechnet mit 8 Bit Gewicht, blit mit 16 und schneidet
/// ab), pixelgenau gegen `blit` (Toleranz 0, reine Kopie), (d) Oberflaeche
/// ueber dem skalierten Bild: CPU-"over" gegen den Shader (Toleranz 2).
fn format_pruefen(gpu: &mut Gpu, p: ffmpeg::format::Pixel, w: u32, h: u32, verzeichnis: &str, lang: &'static crate::strings::Lang, u: &mut ui::Ui) -> bool {
    let name = format!("{p:?}");
    let Some(bild) = probebild(p, w, h) else {
        println!("{name:<15} {w:>4}x{h:<4} kein Probebild: Format unbekannt");
        return false;
    };
    let referenz = match crate::to_rgb(&bild) {
        Ok(f) => f,
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} to_rgb: {e}");
            return false;
        }
    };
    let strides: Vec<String> = (0..bild.planes()).map(|i| bild.stride(i).to_string()).collect();
    println!("{name:<15} {w:>4}x{h:<4} Zeilen {} Byte", strides.join("/"));
    let mut ok = true;

    match gpu.bild_roh(&bild).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, zw, zh)) if (zw, zh) == (w, h) => {
            ok &= fall_pruefen(&name, w, h, "1:1", &referenz.pixels, &aus, w, h, 0, Some(verzeichnis), lang);
        }
        Ok((_, zw, zh)) => {
            println!("{name:<15} {w:>4}x{h:<4} 1:1: Zwischentextur ist {zw}x{zh}");
            ok = false;
        }
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} 1:1: {e}");
            ok = false;
        }
    }
    match gpu.bild_rgb(&referenz).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, _, _)) => ok &= fall_pruefen(&name, w, h, "RGB-Upload", &referenz.pixels, &aus, w, h, 0, Some(verzeichnis), lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} RGB-Upload: {e}");
            ok = false;
        }
    }

    let (bw, bh) = (700u32, 400u32);
    let n = (bw as usize) * (bh as usize);
    let mut cpu = vec![0u32; n];
    crate::blit(&mut cpu, bw, bh, &referenz, false);
    let rect = crate::ziel_rechteck(bw, bh, w, h, false);
    match gpu.offscreen(bw, bh, Some(rect), false) {
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "skaliert", &cpu, &aus, bw, bh, 2, Some(verzeichnis), lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} skaliert: {e}");
            ok = false;
        }
    }
    let mut genau = vec![0u32; n];
    crate::blit(&mut genau, bw, bh, &referenz, true);
    let rect_genau = crate::ziel_rechteck(bw, bh, w, h, true);
    match gpu.offscreen(bw, bh, Some(rect_genau), false) {
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "pixelgenau", &genau, &aus, bw, bh, 0, Some(verzeichnis), lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} pixelgenau: {e}");
            ok = false;
        }
    }

    // Oberflaeche: einmal per Canvas ueber das geblittete Bild, einmal ueber
    // einem leeren Puffer (T = 255) als Ebene fuer den Shader.
    let zeile = format!("{name} {w}x{h} ueber dem Bild");
    let mut cpu_ui = cpu.clone();
    {
        let mut c = ui::Canvas::neu(&mut cpu_ui, bw as usize, bh as usize);
        oberflaeche_probe(&mut c, u, &zeile);
    }
    let mut leer = vec![0xff00_0000u32; n];
    let kasten = {
        let mut c = ui::Canvas::neu(&mut leer, bw as usize, bh as usize);
        oberflaeche_probe(&mut c, u, &zeile);
        c.kasten_nehmen().unwrap_or(ui::Rect { x: 0, y: 0, w: 0, h: 0 })
    };
    match gpu.oberflaeche_hochladen(&leer, bw, bh, kasten).and_then(|_| gpu.offscreen(bw, bh, Some(rect), true)) {
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "Oberflaeche", &cpu_ui, &aus, bw, bh, 2, Some(verzeichnis), lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} Oberflaeche: {e}");
            ok = false;
        }
    }
    ok
}

/// Goldbildtest der Anzeige ohne Fenster: Adapter, Feature-Level, Tearing,
/// d3dcompiler_47.dll, dann jedes Decoderformat in zwei Groessen (257x131:
/// ungerade Breite und Hoehe, Farbebenen (w+1)/2; 1920x1080: der Regelfall)
/// gegen den CPU-Weg. true = alles innerhalb der Toleranz.
pub fn anzeigetest(verzeichnis: &str) -> bool {
    use ffmpeg::format::Pixel;
    let lang = crate::strings::pick("de");
    let _ = std::fs::create_dir_all(verzeichnis);
    println!("Anzeigetest: Direct3D 11 ohne Fenster, Bilder nach {verzeichnis}");

    let adapter = match adapter_liste() {
        Ok(l) => l,
        Err(e) => {
            println!("Adapterliste: {e}");
            Vec::new()
        }
    };
    // Konsole und Protokolldatei - auf der Konsole steht es ohnehin, also
    // nur in die Datei, nicht noch einmal in die Reihe.
    let sagen = |z: String| {
        println!("{z}");
        protokoll::nur_datei(&z);
    };
    for (i, a) in adapter.iter().enumerate() {
        sagen(adapter_zeile(i, a));
    }
    let hardware = adapter.iter().any(|a| !a.software);
    sagen(format!("Tearing: {}", if tearing_moeglich() { "ja" } else { "nein" }));
    {
        let wurzel = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let pfad = format!("{wurzel}\\System32\\d3dcompiler_47.dll");
        let da = std::path::Path::new(&pfad).exists();
        let geladen = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("d3dcompiler_47.dll")) }.is_ok();
        sagen(format!(
            "d3dcompiler_47.dll: {} ({pfad}{})",
            if da { "gefunden" } else { "FEHLT" },
            if geladen { ", im Prozess geladen" } else { "" }
        ));
    }

    let mut gpu = match Gpu::ohne_fenster(!hardware) {
        Ok(g) => g,
        Err(e) => {
            println!("Geraet: {e}");
            for z in protokoll::abholen() {
                println!("    {z}");
            }
            return false;
        }
    };
    println!(
        "Geraet: {} ({}), Feature-Level {}",
        gpu.adapter.name,
        if !hardware { "WARP" } else if gpu.adapter.software { "Software" } else { "Hardware" },
        gpu.feature_level_name()
    );
    for z in protokoll::abholen() {
        println!("    {z}");
    }

    let mut u = ui::Ui::new();
    u.tick = 40;
    u.mouse = (-1, -1);
    if !u.text.ok() {
        println!("Keine Schrift gefunden - die Oberflaechenprobe bleibt ohne Text");
    }
    let formate = [
        Pixel::YUV444P, Pixel::YUV444P10LE, Pixel::YUV444P16LE, Pixel::YUV444P10MSBLE, Pixel::YUV444P12MSBLE,
        Pixel::YUV420P, Pixel::YUVJ420P, Pixel::YUV420P10LE,
        Pixel::NV12, Pixel::P010LE, Pixel::P012LE, Pixel::P016LE,
    ];
    let mut ok = true;
    for p in formate {
        for (w, h) in [(257u32, 131u32), (1920, 1080)] {
            ok &= format_pruefen(&mut gpu, p, w, h, verzeichnis, lang, &mut u);
            for z in protokoll::abholen() {
                println!("    {z}");
            }
        }
    }
    println!("{}", if ok { "Anzeigetest bestanden" } else { "Anzeigetest NICHT bestanden" });
    ok
}
