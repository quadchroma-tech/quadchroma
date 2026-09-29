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
//
// HDR10 (HDR-Plan, Abschnitt 5.4): Ein PQ-Bild - das sagt das VUI des
// decodierten Bildes, nicht eine Nachricht - geht in Stufe 1 ganzzahlig nach
// PQ-R'G'B' (BT.2020) in eine R16G16B16A16_UNORM-Zwischentextur. Stufe 2 je
// nach Ausgabe: auf einem HDR-Schirm ("HDR verwenden" an) wird die Swapchain
// R10G10B10A2 mit G2084/P2020 und HDR10-Metadaten; die Codes gehen
// unveraendert durch, wenn das SDR-Weiss beider Seiten gleich ist und die
// Quelle in den Kopfraum passt, sonst mit der Abbildung aus hdr.rs. Auf einem
// SDR-Schirm bleibt sie B8G8R8A8, und das PQ-Bild wird farbtontreu
// abgeschnitten wie in `to_rgb_mit`. Umgeschaltet wird hoechstens alle 0,5 s
// und nur, wenn Bild und Schirm es verlangen; ein SDR-Strom nimmt den
// heutigen Weg, bitgleich. PQ, sRGB, die Matrizen und die Abbildung stehen
// in hdr_hlsl.rs - dieselbe HLSL-Quelle wie im Wandler des Windows-Hosts.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use ffmpeg_next as ffmpeg;
use windows::core::{Interface, BOOL, PCSTR};
use windows::Win32::Foundation::{CloseHandle, DXGI_STATUS_OCCLUDED, HANDLE, HMODULE, HWND, S_OK, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::anzeigeprobe::{fall_pruefen, oberflaeche_probe, probewert, probewert_pq};
use crate::schirmerkennung::{
    schirm_gewechselt, schirm_zuordnen, text_ohne_ausgang, AnzeigePfad, AusgangFarbe, DxgiAusgang, FensterMonitor, Zuordnung,
};
use crate::{ebenen_format, hdr, protokoll, ui, EbenenFormat, Frame, Karte, Rolle};

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
    /// PQ-Bild: R16G16B16A16_UNORM mit PQ-R'G'B' (s / 65535, s = Code x 64,
    /// siehe ps_umrechnen_pq); sonst B8G8R8A8 mit sRGB wie seit jeher.
    pq: bool,
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

/// Konstanten von Stufe 1 fuer PQ-Bilder (cbuffer FormatPq, 32 Byte): Y'CbCr
/// BT.2020-NCL als Festkomma mit 12 Bit Nachkomma, Ergebnis in Einheiten von
/// 1/64 10-Bit-Code (siehe `pq_stufe1`, das dasselbe auf der CPU rechnet).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KonstPq {
    /// Y-Faktor: 65472 / 1023 (voll) oder 65472 / 876 (begrenzt), x 4096.
    ky: i32,
    /// Y-Nullpunkt: 0 voll, 64 begrenzt.
    y0: i32,
    /// Ebenenwert -> 10-Bit-Code: erst << links (8 Bit: 2), dann >> rechts
    /// (oben buendig: 6; 10 Bit LE: beides 0).
    links: u32,
    rechts: u32,
    /// Cr -> R', Cb -> G', Cr -> G', Cb -> B', je x 65472 / Nenner x 4096.
    c: [i32; 4],
}

/// Konstanten von Stufe 2 fuer PQ-Bilder und die HDR10-Ausgabe (cbuffer Hdr,
/// 32 Byte). Aus der Strominfo des Hosts und dem Bildschirm (`hdr_zahlen`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct KonstHdr {
    /// SDR-Weiss der Quelle in nit (W_h).
    weiss_quelle: f32,
    /// Spitze der Quelle relativ zu W_h (Hs).
    hs: f32,
    /// Kopfraum des Ziels (Hd; 1 = SDR-Schirm).
    hd: f32,
    /// SDR-Weiss des Client-Schirms in nit (Ausgabe HDR10).
    weiss_ziel: f32,
    /// Die Zwischentextur haelt PQ-R'G'B' (1) oder sRGB (0).
    quelle_pq: u32,
    /// Die PQ-Codes unveraendert auf den HDR-Schirm (1).
    durchreichen: u32,
    _h: [u32; 2],
}

/// Was die Swapchain ausgibt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ausgabe {
    /// B8G8R8A8, G22/P709 - der heutige Weg, und jedes Bild auf einem SDR-Schirm.
    Sdr,
    /// R10G10B10A2, G2084/P2020 (HDR10) mit Metadaten - ein PQ-Bild auf einem
    /// HDR-Schirm.
    Hdr10,
}

impl Ausgabe {
    fn format(self) -> DXGI_FORMAT {
        match self {
            Ausgabe::Sdr => DXGI_FORMAT_B8G8R8A8_UNORM,
            Ausgabe::Hdr10 => DXGI_FORMAT_R10G10B10A2_UNORM,
        }
    }

    fn farbraum(self) -> DXGI_COLOR_SPACE_TYPE {
        match self {
            Ausgabe::Sdr => DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            Ausgabe::Hdr10 => DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
        }
    }
}

/// Hoechstens so oft wechselt die Swapchain zwischen SDR und HDR10: jeder
/// Wechsel kann ein Bild schwarz zeigen (Plan, Risiko 6).
const UMSCHALT_SPERRE: Duration = Duration::from_millis(500);
/// So oft liest die Anzeige den Bildschirm des Fensters neu ("HDR verwenden",
/// SDR-Schieber, Spitze) - wie der Fensterfaden fuer IN_ANZEIGE.
const SCHIRM_TAKT: Duration = Duration::from_secs(2);
/// So oft sieht sie nach, ob das Fenster auf einem anderen Monitor steht -
/// dann gleich, ohne die zwei Sekunden abzuwarten.
const MONITOR_TAKT: Duration = Duration::from_millis(250);
/// SDR-Weiss eines HDR-Schirms, wenn DisplayConfig nichts sagt: die Vorgabe
/// von Windows (SDRWhiteLevel 1000 = 80 nit).
const WEISS_WINDOWS_VORGABE_NIT: f32 = 80.0;
/// Groesster Wert von s in der PQ-Zwischentextur: Code 1023 x 64.
const PQ_S_MAX: i32 = 65472;

/// Welche Ausgabe die Swapchain haben soll: HDR10 nur, wenn ein PQ-Bild zu
/// sehen ist, der Schirm im HDR-Modus laeuft und DXGI HDR10 nicht schon
/// abgelehnt hat. Alles andere - auch der Startbildschirm und ein SDR-Strom -
/// ist SDR.
fn ausgabe_soll(bild_pq: bool, schirm_hdr: bool, abgelehnt: bool) -> Ausgabe {
    if bild_pq && schirm_hdr && !abgelehnt {
        Ausgabe::Hdr10
    } else {
        Ausgabe::Sdr
    }
}

/// Jetzt umschalten? Nur bei anderem Soll, und hoechstens alle
/// UMSCHALT_SPERRE (`seit`: Zeit seit der letzten Umschaltung, None = noch
/// keine). Bis dahin zeigt die laufende Ausgabe, was kommt: ein SDR-Bild auf
/// HDR10 umgerechnet, ein PQ-Bild auf SDR abgebildet.
fn umschalten_jetzt(ist: Ausgabe, soll: Ausgabe, seit: Option<Duration>) -> bool {
    ist != soll && seit.map_or(true, |d| d >= UMSCHALT_SPERRE)
}

/// Die Entscheidung vor jedem Present: None = bleiben, Some(a) = jetzt auf
/// `a` umschalten (ausgabe_soll, dann umschalten_jetzt).
fn ausgabe_wechsel(ist: Ausgabe, bild_pq: bool, schirm_hdr: bool, abgelehnt: bool, seit: Option<Duration>) -> Option<Ausgabe> {
    let soll = ausgabe_soll(bild_pq, schirm_hdr, abgelehnt);
    umschalten_jetzt(ist, soll, seit).then_some(soll)
}

/// Die Zahlen von Stufe 2 aus der Strominfo (None: Vorgaben 203 / 1000 nit)
/// und dem Bildschirm (None: unbekannt). HDR10: das SDR-Weiss des Schirms
/// (unbekannt: 80 nit wie Windows) wird das SDR-Weiss der Quelle (E3), der
/// Kopfraum ist Spitze / Weiss (Spitze unbekannt: 1000 nit); durchgereicht wird,
/// wenn beide Weiss hoechstens 1 nit auseinanderliegen und die Quelle in den
/// Kopfraum passt. SDR: Kopfraum 1, farbtontreu abgeschnitten (E4).
fn hdr_zahlen(quelle: Option<&hdr::InfoV1>, schirm: Option<&hdr::Schirm>, ausgabe: Ausgabe, quelle_pq: bool) -> KonstHdr {
    let ab = hdr::Abbildung::neu(quelle, 1.0);
    let (weiss_ziel, hd) = match ausgabe {
        Ausgabe::Sdr => (ab.weiss_nit, 1.0),
        Ausgabe::Hdr10 => {
            let s = schirm.copied().unwrap_or_default();
            let weiss = if s.sdr_weiss_nit > 0.0 { s.sdr_weiss_nit } else { WEISS_WINDOWS_VORGABE_NIT };
            let spitze = if s.spitze_nit > 0.0 { s.spitze_nit } else { hdr::SPITZE_VORGABE_NIT };
            (weiss, (spitze / weiss).max(1.0))
        }
    };
    let durchreichen = ausgabe == Ausgabe::Hdr10 && quelle_pq && (ab.weiss_nit - weiss_ziel).abs() <= 1.0 && ab.hs <= hd;
    KonstHdr {
        weiss_quelle: ab.weiss_nit,
        hs: ab.hs,
        hd,
        weiss_ziel,
        quelle_pq: quelle_pq as u32,
        durchreichen: durchreichen as u32,
        _h: [0; 2],
    }
}

/// Die HDR10-Metadaten der Swapchain: Primaerfarben BT.2020 und D65 (in
/// 0,00002), als Spitze die abgebildete Spitze auf diesem Schirm
/// (min(Hs, Hd) x SDR-Weiss des Schirms, in nit), das Minimum aus der
/// Strominfo (sonst 0,005 nit), MaxFALL aus der Strominfo, hoechstens die Spitze.
fn hdr10_metadaten(k: &KonstHdr, quelle: Option<&hdr::InfoV1>) -> DXGI_HDR_METADATA_HDR10 {
    let spitze = (k.hs.min(k.hd) * k.weiss_ziel).round().clamp(1.0, 10000.0);
    let min = quelle.map(|q| q.master_min_zehntausendstel).filter(|&m| m != 0).unwrap_or(50);
    let fall = quelle.map(|q| q.max_fall as f32).unwrap_or(0.0).min(spitze);
    DXGI_HDR_METADATA_HDR10 {
        RedPrimary: [35400, 14600],
        GreenPrimary: [8500, 39850],
        BluePrimary: [6550, 2300],
        WhitePoint: [15635, 16450],
        MaxMasteringLuminance: spitze as u32,
        MinMasteringLuminance: min as u32,
        MaxContentLightLevel: spitze as u16,
        MaxFrameAverageLightLevel: fall as u16,
    }
}

/// Die Konstanten von Stufe 1 fuer ein PQ-Bild: Aufbau der Ebenen und Bereich
/// (begrenzt, wenn das Format oder das VUI es sagt). Gerechnet wird immer mit
/// BT.2020-NCL (Plan, Abschnitt 2; `zeile_rgb_pq` ebenso).
fn konst_pq(fmt: EbenenFormat, farbe: hdr::Farbe) -> KonstPq {
    let begrenzt = fmt.begrenzt || !farbe.voll;
    let (y_nenner, c_nenner, y0) = if begrenzt { (876.0, 896.0, 64) } else { (1023.0, 1023.0, 0) };
    let s = PQ_S_MAX as f64;
    let q = |x: f64| (x * 4096.0).round() as i32;
    let (links, rechts) = match fmt.bits {
        8 => (2, 0),
        10 => (0, 0),
        _ => (0, 6),
    };
    KonstPq {
        ky: q(s / y_nenner),
        y0,
        links,
        rechts,
        c: [
            q(s * hdr::CR_R_2020 as f64 / c_nenner),
            q(s * hdr::CB_G_2020 as f64 / c_nenner),
            q(s * hdr::CR_G_2020 as f64 / c_nenner),
            q(s * hdr::CB_B_2020 as f64 / c_nenner),
        ],
    }
}

/// Stufe 1 fuer PQ auf der CPU, Schritt fuer Schritt wie ps_umrechnen_pq:
/// drei 10-Bit-Codes -> s = R'G'B' x 65472, geklemmt auf 0..65472. Referenz
/// des Goldbildtests (Durchreichen: Code = (s + 32) >> 6, Toleranz 0).
fn pq_stufe1(k: &KonstPq, y: i32, cb: i32, cr: i32) -> [i32; 3] {
    let (cb, cr) = (cb - 512, cr - 512);
    let yy = k.ky * (y - k.y0);
    let r = (yy + k.c[0] * cr + 2048) >> 12;
    let g = (yy - k.c[1] * cb - k.c[2] * cr + 2048) >> 12;
    let b = (yy + k.c[3] * cb + 2048) >> 12;
    [r, g, b].map(|v| v.clamp(0, PQ_S_MAX))
}

/// Der 10-Bit-Code, den ein HDR-Schirm beim Durchreichen bekommt: s / 64,
/// gerundet (wie ps_anzeigen_hdr).
fn pq_durch(s: i32) -> u16 {
    ((s + 32) >> 6).min(1023) as u16
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
    /// Das Fenster (MonitorFromWindow, schirm_lesen); null ohne Fenster.
    hwnd: HWND,
    /// Was die Swapchain gerade ausgibt; ohne Fenster immer SDR.
    ausgabe: Ausgabe,
    /// Zeitpunkt der letzten Umschaltung (Sperre UMSCHALT_SPERRE).
    umgeschaltet: Option<Instant>,
    /// DXGI hat HDR10 an diesem Schirm abgelehnt: kein neuer Versuch, bis
    /// sich der Schirm aendert; bis dahin meldet die Anzeige dem Host keine
    /// HDR-Darstellung (hdr_darstellung), er sendet dann SDR.
    hdr10_abgelehnt: bool,
    /// Der Bildschirm des Fensters mit allem, was von genau ihm stammt
    /// (schirm_lesen), und wann er gelesen wurde; der HMONITOR, auf dem das
    /// Fenster zuletzt stand, und wann das geprueft wurde.
    schirm: Option<Zuordnung>,
    schirm_zeit: Option<Instant>,
    monitor: isize,
    monitor_zeit: Option<Instant>,
    /// Strominfo Fassung 1 des laufenden Stroms (SDR-Weiss und Spitze des
    /// Hosts); kommt mit jedem rohen Bild (`quelle_setzen`).
    quelle: Option<hdr::InfoV1>,
    /// Die zuletzt gesetzten HDR10-Metadaten - gesetzt wird nur, was sich aendert.
    metadaten: Option<DXGI_HDR_METADATA_HDR10>,
    /// Ein PQ-Bild mit einer anderen Matrix als BT.2020-NCL steht schon im Protokoll.
    matrix_gemeldet: bool,
    vs: ID3D11VertexShader,
    ps_umrechnen: ID3D11PixelShader,
    ps_anzeigen: ID3D11PixelShader,
    /// Stufe 1 fuer PQ-Bilder, Stufe 2 fuer PQ auf SDR und fuer HDR10.
    ps_umrechnen_pq: ID3D11PixelShader,
    ps_anzeigen_pq_sdr: ID3D11PixelShader,
    ps_anzeigen_hdr: ID3D11PixelShader,
    /// MIN_MAG_MIP_LINEAR, CLAMP.
    sampler: ID3D11SamplerState,
    /// CULL_NONE.
    raster: ID3D11RasterizerState,
    /// 16 Byte, DYNAMIC.
    konst_format: ID3D11Buffer,
    /// 64 Byte, DYNAMIC.
    konst_anzeige: ID3D11Buffer,
    /// 32 Byte je, DYNAMIC: Stufe 1 PQ (b2) und Stufe 2 HDR (b3).
    konst_pq: ID3D11Buffer,
    konst_hdr: ID3D11Buffer,
    /// Eingangstexturen des laufenden Formats.
    ebenen: Option<Ebenen>,
    /// Bildgroesse, RT + SRV: B8G8R8A8_UNORM, bei PQ-Bildern R16G16B16A16_UNORM.
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
/// kommen. Die HDR-Einstiege dahinter nehmen dazu b2 (Stufe 1 PQ) und b3
/// (Stufe 2 HDR). Davor steht hdr_hlsl::GEMEINSAM_PQ (vs_voll, sRGB, PQ,
/// Matrizen, Abbildung - dieselbe Quelle wie der Wandler des Hosts, ihre
/// Zahlen prueft hdr_hlsl.rs gegen hdr.rs); `quelle` setzt beides zusammen.
const SHADER: &str = r#"
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

// ================= HDR10 (PQ, BT.2020): dieselbe Rechnung wie hdr.rs =================

// ---------- Stufe 1 fuer PQ-Bilder: Ebenen -> PQ-R'G'B', ganzzahlig wie pq_stufe1 ----------
// Gespeichert wird s = R'G'B' x 65472 (10-Bit-Code x 64, sechs Bit Nachkomma)
// als s / 65535 in R16G16B16A16_UNORM: UNORM16 trifft s genau, und
// (s + 32) >> 6 ist der Code, den ein HDR-Schirm beim Durchreichen bekommt.
cbuffer FormatPq : register(b2) {
    int  pq_ky;      // Y-Faktor, Festkomma 12 Bit: 65472 / 1023 (voll) oder / 876 (begrenzt)
    int  pq_y0;      // Y-Nullpunkt: 0 voll, 64 begrenzt
    uint pq_links;   // Ebenenwert -> 10-Bit-Code: << links (8 Bit: 2) ...
    uint pq_rechts;  // ... >> rechts (oben buendig: 6)
    int4 pq_c;       // Cr->R', Cb->G', Cr->G', Cb->B' (BT.2020-NCL), Festkomma 12 Bit
};
uint pq_code(uint v) { return (v << pq_links) >> pq_rechts; }
float4 ps_umrechnen_pq(float4 pos : SV_Position) : SV_Target {
    int2 xy = int2(pos.xy);
    int2 c  = (sub != 0) ? (xy >> 1) : xy;
    int y = int(pq_code(ebene_y.Load(int3(xy, 0))));
    int cb, cr;
    if (paar != 0) {
        uint2 uv = ebene_uv.Load(int3(c, 0));
        cb = int(pq_code(uv.x));
        cr = int(pq_code(uv.y));
    } else {
        cb = int(pq_code(ebene_u.Load(int3(c, 0))));
        cr = int(pq_code(ebene_v.Load(int3(c, 0))));
    }
    cb -= 512; cr -= 512;
    int yy = pq_ky * (y - pq_y0);
    int r = (yy + pq_c.x * cr + 2048) >> 12;
    int g = (yy - pq_c.y * cb - pq_c.z * cr + 2048) >> 12;
    int b = (yy + pq_c.w * cb + 2048) >> 12;
    int3 s = clamp(int3(r, g, b), 0, 65472);
    return float4(float3(s) * (1.0 / 65535.0), 1.0);
}

// ---------- Stufe 2 fuer PQ-Bilder und die HDR10-Ausgabe ----------
cbuffer Hdr : register(b3) {
    float h_weiss_quelle;   // SDR-Weiss des Hosts (W_h) in nit
    float h_hs;             // Spitze der Quelle / W_h
    float h_hd;             // Kopfraum des Ziels (1 = SDR-Schirm)
    float h_weiss_ziel;     // SDR-Weiss des Client-Schirms in nit (HDR10)
    uint  h_quelle_pq;      // Zwischentextur: 1 = PQ-R'G'B' (Stufe 1 PQ), 0 = sRGB
    uint  h_durchreichen;   // 1 = PQ-Codes unveraendert auf den HDR-Schirm
    uint2 _h;
};

// Der Wert der Zwischentextur an diesem Zielpunkt, mit denselben Quellpunkten
// wie ps_anzeigen (blit); false ausserhalb des Bildes.
bool bild_lesen(float4 pos, out float3 v) {
    v = float3(0, 0, 0);
    int2 p = int2(pos.xy) - rect.xy;
    if ((modus & 1) == 0 || any(p < 0) || any(p >= rect.zw)) return false;
    uint2 f = uint2(p) * schritt;
    if (all(schritt == 65536)) {
        v = video.Load(int3(int2(f >> 16), 0)).rgb;
    } else {
        float2 t = float2(f >> 16) + float2(f & 0xffff) * (1.0 / 65536.0);
        v = video.SampleLevel(weich, (t + 0.5) / float2(bild), 0).rgb;
    }
    return true;
}

// Zwischentextur PQ (s / 65535) -> PQ-Signal 0..1 (s / 65472).
float3 pq_signal(float3 v) { return saturate(v * (65535.0 / 65472.0)); }

// PQ-Bild -> lineares Licht BT.2020 relativ zum SDR-Weiss des Hosts.
float3 pq_relativ(float3 v) { return max(pq_eotf(pq_signal(v)) / h_weiss_quelle, 0.0); }

// SDR-Inhalt (sRGB) auf dem HDR10-Ziel: SDR-Weiss = SDR-Weiss des Schirms.
float3 sdr_nach_nit(float3 v) { return mul(M_709_NACH_2020, srgb_eotf(v)) * h_weiss_ziel; }

// PQ-Bild auf einem SDR-Ziel (B8G8R8A8): farbtontreu beim SDR-Weiss
// abgeschnitten (E4), BT.2020 -> BT.709, sRGB - wie pq_nach_srgb8. Die
// Oberflaeche liegt darueber wie in ps_anzeigen.
float4 ps_anzeigen_pq_sdr(float4 pos : SV_Position) : SV_Target {
    float3 farbe = grund.rgb;
    float3 v;
    if (bild_lesen(pos, v)) {
        float3 rel = rgb_abbilden(pq_relativ(v), h_hs, 1.0);
        farbe = acht_bit(srgb_oetf(mul(M_2020_NACH_709, rel)));
    }
    if ((modus & 2) != 0) {
        float4 o = oberflaeche.Load(int3(pos.xy, 0));
        farbe = farbe * o.a + o.rgb;
    }
    return float4(farbe, 1);
}

// Ausgabe HDR10 (R10G10B10A2, G2084/P2020). Ein PQ-Bild geht beim
// Durchreichen mit seinen Codes durch, sonst: relativ zum SDR-Weiss des
// Hosts, abgebildet auf den Kopfraum des Schirms, mal dessen SDR-Weiss (E3).
// Ein SDR-Bild (nur waehrend der Umschaltsperre) und die Oberflaeche: sRGB ->
// linear, SDR-Weiss des Schirms, BT.709 -> BT.2020; die Oberflaeche "over" in
// linearem Licht. Dann PQ, zehn Bit.
float4 ps_anzeigen_hdr(float4 pos : SV_Position) : SV_Target {
    float3 nit = sdr_nach_nit(grund.rgb);
    float3 v;
    bool roh = false;
    float3 code = float3(0, 0, 0);
    if (bild_lesen(pos, v)) {
        if (h_quelle_pq == 0) {
            nit = sdr_nach_nit(v);
        } else if (h_durchreichen != 0) {
            float3 s = floor(v * 65535.0 + 0.5);            // s genau zurueck (UNORM16)
            code = min(floor((s + 32.0) * (1.0 / 64.0)), 1023.0);
            roh = true;
        } else {
            nit = rgb_abbilden(pq_relativ(v), h_hs, h_hd) * h_weiss_ziel;
        }
    }
    if ((modus & 2) != 0) {
        float4 o = oberflaeche.Load(int3(pos.xy, 0));
        float deckung = 1.0 - o.a;
        if (deckung > 0.0 || any(o.rgb > 0.0)) {
            if (roh) {
                nit = pq_eotf(code * (1.0 / 1023.0));      // beim Durchreichen: nit der Quelle = nit am Schirm
                roh = false;
            }
            float3 farbe = (deckung > 0.0) ? o.rgb / deckung : float3(0, 0, 0);
            nit = nit * o.a + sdr_nach_nit(farbe) * deckung;
        }
    }
    if (roh) return float4(code * (1.0 / 1023.0), 1);
    return float4(zehn_bit(pq_oetf(nit)), 1);
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
    let luid = luid_zahl(d.AdapterLuid);
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

/// Kann dieser Weg HDR-Bilder zeigen (Bit 1 in IN_ANZEIGE)? Ja: PQ-Bilder
/// gehen ueber Stufe 1 PQ - auf einem HDR-Schirm in eine HDR10-Swapchain
/// (R10G10B10A2, G2084/P2020), auf einem SDR-Schirm abgebildet. Nein, sobald
/// DXGI HDR10 an diesem Schirm abgelehnt hat (`abgelehnt`): dann soll der Host
/// SDR senden (Unsicherheit ergibt SDR), statt die ganze Sitzung PQ zu
/// liefern, das hier nur abgebildet wird - bis sich der Schirm aendert
/// (lage_pruefen). Ob der Schirm HDR kann, sagt Bit 0 (schirm_lage); der
/// softbuffer-Weg meldet nein.
fn hdr_darstellung_moeglich(abgelehnt: bool) -> bool {
    !abgelehnt
}

/// So lange gilt eine eben angelegte Factory der Schirmerkennung als frisch:
/// ein zweiter Versuch in dieser Zeit (etwa die Anzeige gleich nach dem
/// Fensterfaden) legt keine weitere an.
const FABRIK_FRISCH: Duration = Duration::from_secs(1);

thread_local! {
    /// Die Factory der Schirmerkennung und wann sie angelegt wurde. Sie
    /// bleibt, solange sie gilt (IsCurrent - eine alte kennt neu angesteckte
    /// Bildschirme, neu vergebene HMONITOR und einen umgeschalteten HDR-Modus
    /// nicht).
    static FABRIK: RefCell<Option<(IDXGIFactory1, Instant)>> = const { RefCell::new(None) };
    /// Die zuletzt ins Protokoll geschriebene Zeile der Schirmerkennung.
    static SCHIRM_GEMELDET: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Text aus einem nullterminierten UTF-16-Feld (Geraete- und Adapternamen).
fn utf16_text(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

/// Eine LUID als Zahl - dieselbe Rechnung fuer AdapterInfo::luid, die
/// Adapter der Schirmerkennung und die Karten aus DisplayConfig, damit sie
/// sich vergleichen lassen.
fn luid_zahl(l: windows::Win32::Foundation::LUID) -> i64 {
    ((l.HighPart as i64) << 32) | l.LowPart as i64
}

/// Die Factory der Schirmerkennung: die gemerkte, solange sie gilt; `neu`
/// verlangt eine neue, ausser die gemerkte ist juenger als FABRIK_FRISCH.
fn fabrik(neu: bool) -> Option<IDXGIFactory1> {
    FABRIK.with(|f| {
        let mut f = f.borrow_mut();
        let behalten = f
            .as_ref()
            .is_some_and(|(x, seit)| unsafe { x.IsCurrent() }.as_bool() && (!neu || seit.elapsed() < FABRIK_FRISCH));
        if !behalten {
            *f = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok().map(|x| (x, Instant::now()));
        }
        f.as_ref().map(|(x, _)| x.clone())
    })
}

/// Gilt die gemerkte Factory nicht mehr (IsCurrent nein)? Dann hat Windows
/// Bildschirme oder "HDR verwenden" umgestellt, und `lage_pruefen` liest
/// sofort. Ohne gemerkte Factory nein.
fn fabrik_veraltet() -> bool {
    FABRIK.with(|f| f.borrow().as_ref().is_some_and(|(x, _)| !unsafe { x.IsCurrent() }.as_bool()))
}

/// Der Monitor des Fensters laut GDI: HMONITOR (MonitorFromWindow, der mit
/// dem groessten Teil des Fensters) und sein Geraetename (GetMonitorInfoW).
fn fenster_monitor(hwnd: HWND) -> Option<FensterMonitor> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
    let m = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if m.is_invalid() {
        return None;
    }
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
        return None;
    }
    Some(FensterMonitor { monitor: m.0 as isize, name: utf16_text(&info.szDevice) })
}

/// Alle Ausgaenge ALLER Adapter der Factory - nicht nur die der Karte der
/// Swapchain: auf Laptops mit zwei Karten haengt der Bildschirm oft an der
/// anderen. Je Ausgang HMONITOR, Geraetename und Karte, dazu die Farblage aus
/// IDXGIOutput6::GetDesc1 - Beschreibung und Werte aus EINEM Aufruf.
fn dxgi_ausgaenge(factory: &IDXGIFactory1) -> Vec<DxgiAusgang> {
    let mut liste = Vec::new();
    let mut i = 0;
    while let Ok(a) = unsafe { factory.EnumAdapters1(i) } {
        let adapter = i;
        i += 1;
        let Ok(ad) = (unsafe { a.GetDesc1() }) else { continue };
        let mut j = 0;
        while let Ok(o) = unsafe { a.EnumOutputs(j) } {
            j += 1;
            let (monitor, name, farbe) = match o.cast::<IDXGIOutput6>().ok().and_then(|o6| unsafe { o6.GetDesc1() }.ok()) {
                Some(d) => (
                    d.Monitor.0 as isize,
                    utf16_text(&d.DeviceName),
                    Some(AusgangFarbe {
                        hdr: d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
                        spitze_nit: d.MaxLuminance,
                        vollbild_spitze_nit: d.MaxFullFrameLuminance,
                    }),
                ),
                None => match unsafe { o.GetDesc() } {
                    Ok(d) => (d.Monitor.0 as isize, utf16_text(&d.DeviceName), None),
                    Err(_) => continue,
                },
            };
            liste.push(DxgiAusgang {
                adapter,
                luid: luid_zahl(ad.AdapterLuid),
                adapter_name: utf16_text(&ad.Description),
                monitor,
                name,
                farbe,
            });
        }
    }
    liste
}

/// Die aktiven Pfade aus DisplayConfig (live, ohne Factory), je mit der
/// Karte der Quelle, dem Ziel, seinem Anzeigenamen, SDR-Weiss und "HDR
/// verwenden" (Advanced Color) - dieselben Hilfen wie der Host.
fn anzeige_pfade() -> Vec<AnzeigePfad> {
    use crate::host::aufnahme as au;
    au::anzeigepfade()
        .into_iter()
        .map(|(quelle, p)| AnzeigePfad {
            quelle,
            quelle_luid: luid_zahl(p.sourceInfo.adapterId),
            ziel_luid: luid_zahl(p.targetInfo.adapterId),
            ziel_id: p.targetInfo.id,
            anzeigename: au::ziel_anzeigename(&p).unwrap_or_default(),
            sdr_weiss_nit: au::ziel_sdr_weiss(&p).map(crate::host::wandler::sdr_weiss_nit),
            hdr: au::ziel_hdr(&p),
        })
        .collect()
}

/// Der Bildschirm des Fensters mit allem, was von genau ihm stammt
/// (schirmerkennung.rs): der Monitor laut GDI, sein DXGI-Ausgang ueber alle
/// Adapter, sein Pfad in DisplayConfig. Passt kein Ausgang oder sagen DXGI
/// und DisplayConfig Verschiedenes ueber "HDR verwenden", wird einmal mit
/// einer neuen Factory gelesen. Jede neue Lage (anderer Bildschirm, andere
/// Werte) steht einmal im Protokoll ("Fenster auf ..."): der Fensterfaden
/// (IN_ANZEIGE) und die Anzeige (lage_pruefen) fragen auf demselben Faden.
/// None, wenn das Fenster keinen Monitor hat oder kein Ausgang passt.
fn schirm_lesen(hwnd: HWND) -> Option<Zuordnung> {
    let fenster = fenster_monitor(hwnd)?;
    let pfade = anzeige_pfade();
    let mut z = None;
    for neu in [false, true] {
        let Some(factory) = fabrik(neu) else { break };
        z = schirm_zuordnen(&fenster, &dxgi_ausgaenge(&factory), &pfade);
        if z.as_ref().is_some_and(|z| !z.widerspruch) {
            break;
        }
    }
    let zeile = z.as_ref().map_or_else(|| text_ohne_ausgang(&fenster), Zuordnung::text);
    SCHIRM_GEMELDET.with(|g| {
        let mut g = g.borrow_mut();
        if g.as_deref() != Some(zeile.as_str()) {
            protokoll::zeile(zeile.clone());
            *g = Some(zeile);
        }
    });
    z
}

/// Der Bildschirm des Fensters fuer IN_ANZEIGE (main.rs, alle zwei Sekunden
/// und nach einem Verschieben): HDR, SDR-Weiss und Spitzen von genau dem
/// Bildschirm, auf dem der groesste Teil des Fensters liegt (schirm_lesen).
/// None, wenn keiner passt - dann gilt SDR.
pub fn schirm_lage(hwnd: isize) -> Option<crate::hdr::Schirm> {
    schirm_lesen(HWND(hwnd as *mut c_void)).map(|z| z.schirm)
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

/// Die ganze Quelle der Anzeige: die gemeinsamen Funktionen aus hdr_hlsl.rs
/// (auch PQ) und dahinter SHADER.
fn hlsl_quelle() -> &'static str {
    static Q: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    Q.get_or_init(|| format!("{}{}", crate::hdr_hlsl::GEMEINSAM_PQ, SHADER))
}

/// Einen Einstiegspunkt aus der Quelle der Anzeige uebersetzen.
fn uebersetzen(einstieg: &[u8], ziel: &[u8]) -> Result<Vec<u8>, String> {
    uebersetzen_aus(hlsl_quelle(), b"anzeige.hlsl\0", einstieg, ziel)
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
        gpu.hwnd = hwnd;
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
        // Das Format bleibt (DXGI_FORMAT_UNKNOWN); den Farbraum der
        // HDR10-Ausgabe sicherheitshalber neu setzen, wie Microsofts Beispiel
        // es nach jedem ResizeBuffers tut.
        if self.ausgabe == Ausgabe::Hdr10 {
            if let Ok(sc3) = sc.cast::<IDXGISwapChain3>() {
                if let Err(e) = unsafe { sc3.SetColorSpace1(Ausgabe::Hdr10.farbraum()) } {
                    protokoll::zeile(fehler("Anzeige: SetColorSpace1 G2084 nach ResizeBuffers", e));
                }
            }
        }
        self.backbuffer_sicht()?;
        self.breite = ww;
        self.hoehe = wh;
        Ok(())
    }

    /// Praesentiert die Swapchain gerade in HDR (HDR10: R10G10B10A2 mit
    /// G2084/P2020)? Das ist die Seite des Clients fuer das goldene "HDR"
    /// (hdr::lage) - eine HDR-Quelle auf einem SDR-Schirm ist nur abgebildet.
    pub fn hdr_praesentiert(&self) -> bool {
        self.ausgabe == Ausgabe::Hdr10
    }

    /// Bit 1 in IN_ANZEIGE: kann diese Anzeige HDR-Bilder zeigen? Nein, solange
    /// DXGI HDR10 an diesem Schirm abgelehnt hat (hdr_darstellung_moeglich).
    pub fn hdr_darstellung(&self) -> bool {
        hdr_darstellung_moeglich(self.hdr10_abgelehnt)
    }

    /// Die Strominfo Fassung 1 des Stroms, zu dem das naechste rohe Bild
    /// gehoert (None: Host vor 0.2.0 - dann gelten die Vorgaben 203 / 1000
    /// nit). Der Empfangsfaden gibt sie jedem Bild mit.
    pub fn quelle_setzen(&mut self, quelle: Option<&hdr::InfoV1>) {
        self.quelle = quelle.copied();
    }

    /// Den Bildschirm des Fensters neu lesen, wenn es Zeit ist: alle
    /// SCHIRM_TAKT, und sofort, wenn das Fenster auf einem anderen Monitor
    /// steht oder Windows Bildschirme oder "HDR verwenden" umgestellt hat (die
    /// Factory der Schirmerkennung gilt nicht mehr) - beides alle
    /// MONITOR_TAKT nachgesehen, beides billig. Ein anderer Bildschirm oder
    /// ein umgeschaltetes "HDR verwenden" (schirm_gewechselt) kommt ins
    /// Protokoll, und HDR10 darf wieder versucht werden; ein neuer HMONITOR
    /// fuer denselben Bildschirm oder andere Pegel aendern daran nichts.
    fn lage_pruefen(&mut self) {
        use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
        if self.hwnd.0.is_null() {
            return;
        }
        let jetzt = Instant::now();
        let mut lesen = self.schirm_zeit.map_or(true, |t| jetzt.duration_since(t) >= SCHIRM_TAKT);
        if lesen || self.monitor_zeit.map_or(true, |t| jetzt.duration_since(t) >= MONITOR_TAKT) {
            self.monitor_zeit = Some(jetzt);
            let m = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize;
            if m != self.monitor || fabrik_veraltet() {
                self.monitor = m;
                lesen = true;
            }
        }
        if !lesen {
            return;
        }
        self.schirm_zeit = Some(jetzt);
        let neu = schirm_lesen(self.hwnd);
        if schirm_gewechselt(self.schirm.as_ref(), neu.as_ref()) {
            protokoll::zeile(format!(
                "Anzeige: Bildschirm {}",
                match neu.as_ref().map(|z| &z.schirm) {
                    Some(s) if s.hdr => format!("HDR (SDR-Weiss {:.0} nit, Spitze {:.0} nit)", s.sdr_weiss_nit, s.spitze_nit),
                    Some(_) => "SDR".into(),
                    None => "unbekannt (gilt als SDR)".into(),
                }
            ));
            self.hdr10_abgelehnt = false;
        }
        self.schirm = neu;
    }

    /// Die Swapchain auf eine andere Ausgabe umstellen: Sicht fallen lassen,
    /// ResizeBuffers mit dem Format (gleiche Groesse, gleiche Flags),
    /// CheckColorSpaceSupport und SetColorSpace1, bei HDR10 die Metadaten,
    /// bei SDR keine. Lehnt DXGI HDR10 ab, geht es zurueck auf SDR, und bis
    /// sich der Schirm aendert, wird es nicht wieder versucht.
    fn ausgabe_setzen(&mut self, neu: Ausgabe) -> Result<(), String> {
        // Auch ein gescheiterter Versuch zaehlt fuer die Sperre.
        self.umgeschaltet = Some(Instant::now());
        let Some(sc) = self.swapchain.clone() else { return Err("keine Swapchain".into()) };
        let (ww, wh, flags) = (self.breite, self.hoehe, DXGI_SWAP_CHAIN_FLAG(self.flags as i32));
        if ww == 0 || wh == 0 {
            return Err("Swapchain ohne Groesse".into());
        }
        let sc3: IDXGISwapChain3 = sc.cast().map_err(|e| fehler("IDXGISwapChain3", e))?;
        self.sichten_loesen();
        self.rtv = None;
        unsafe { self.ctx.Flush() };
        let stellen = |a: Ausgabe| -> Result<(), String> {
            unsafe { sc.ResizeBuffers(0, ww, wh, a.format(), flags) }.map_err(|e| fehler("ResizeBuffers", e))?;
            let geht = unsafe { sc3.CheckColorSpaceSupport(a.farbraum()) }.map_err(|e| fehler("CheckColorSpaceSupport", e))?;
            if geht & DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT.0 as u32 == 0 {
                return Err(format!("CheckColorSpaceSupport: Farbraum {} nicht praesentierbar (0x{geht:x})", a.farbraum().0));
            }
            unsafe { sc3.SetColorSpace1(a.farbraum()) }.map_err(|e| fehler("SetColorSpace1", e))
        };
        let ergebnis = stellen(neu);
        match &ergebnis {
            Ok(()) => {
                self.ausgabe = neu;
                self.metadaten = None;
                if neu == Ausgabe::Sdr {
                    if let Ok(sc4) = sc.cast::<IDXGISwapChain4>() {
                        let _ = unsafe { sc4.SetHDRMetaData(DXGI_HDR_METADATA_TYPE_NONE, None) };
                    }
                }
            }
            Err(_) => {
                if let Err(e) = stellen(Ausgabe::Sdr) {
                    protokoll::zeile(format!("Anzeige: zurueck auf SDR gescheitert: {e}"));
                }
                self.ausgabe = Ausgabe::Sdr;
                self.metadaten = None;
            }
        }
        if let Err(e) = self.backbuffer_sicht() {
            // Ohne Sicht kein Bild: keine Groesse gilt als eingestellt, dann
            // baut `groesse` beim naechsten Takt Puffer und Sicht neu (wie
            // nach einem gescheiterten ResizeBuffers dort).
            self.breite = 0;
            self.hoehe = 0;
            return Err(e);
        }
        ergebnis
    }

    /// Vor jedem Present: Schirm pruefen, Ausgabe waehlen (umschalten, wenn
    /// noetig und erlaubt), bei HDR10 die Metadaten nachfuehren. Gibt die
    /// Zahlen fuer Stufe 2 zurueck.
    fn ausgabe_fuehren(&mut self, bild_da: bool) -> KonstHdr {
        self.lage_pruefen();
        let pq = bild_da && self.zwischen.as_ref().is_some_and(|z| z.pq);
        let schirm_hdr = self.schirm.as_ref().is_some_and(|z| z.schirm.hdr);
        if let Some(soll) = ausgabe_wechsel(self.ausgabe, pq, schirm_hdr, self.hdr10_abgelehnt, self.umgeschaltet.map(|t| t.elapsed())) {
            match self.ausgabe_setzen(soll) {
                Ok(()) => {
                    let k = hdr_zahlen(self.quelle.as_ref(), self.schirm.as_ref().map(|z| &z.schirm), soll, pq);
                    protokoll::zeile(match soll {
                        Ausgabe::Hdr10 => format!(
                            "Anzeige: Swapchain HDR10 (R10G10B10A2, G2084/P2020), Weiss Quelle {:.0} nit, Schirm {:.0} nit, Kopfraum Quelle {:.2}, Schirm {:.2}, Durchreichen {}",
                            k.weiss_quelle, k.weiss_ziel, k.hs, k.hd, if k.durchreichen != 0 { "ja" } else { "nein" }
                        ),
                        Ausgabe::Sdr => "Anzeige: Swapchain SDR (B8G8R8A8, G22/P709)".into(),
                    });
                }
                Err(e) => {
                    // HDR10 geht hier nicht (DXGI lehnt ab, Windows vor 10):
                    // nicht wieder versuchen, bis sich der Schirm aendert, und
                    // bis dahin dem Host keine HDR-Darstellung melden
                    // (IN_ANZEIGE Bit 1 = 0 beim naechsten anzeige_takt) - er
                    // stellt dann auf SDR um.
                    let hdr10 = soll == Ausgabe::Hdr10;
                    self.hdr10_abgelehnt |= hdr10;
                    protokoll::zeile(format!(
                        "Anzeige: Umschalten auf {} gescheitert: {e}{}",
                        if hdr10 { "HDR10" } else { "SDR" },
                        if hdr10 { " - bis sich der Bildschirm aendert: keine HDR-Darstellung an den Host (er sendet SDR), HDR-Bilder bis dahin auf SDR abgebildet" } else { "" }
                    ));
                }
            }
        }
        let k = hdr_zahlen(self.quelle.as_ref(), self.schirm.as_ref().map(|z| &z.schirm), self.ausgabe, self.zwischen.as_ref().is_some_and(|z| z.pq));
        if self.ausgabe == Ausgabe::Hdr10 {
            let m = hdr10_metadaten(&k, self.quelle.as_ref());
            if self.metadaten != Some(m) {
                if let Some(sc4) = self.swapchain.as_ref().and_then(|s| s.cast::<IDXGISwapChain4>().ok()) {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(&m as *const DXGI_HDR_METADATA_HDR10 as *const u8, std::mem::size_of::<DXGI_HDR_METADATA_HDR10>())
                    };
                    match unsafe { sc4.SetHDRMetaData(DXGI_HDR_METADATA_TYPE_HDR10, Some(bytes)) } {
                        Ok(()) => protokoll::zeile(format!(
                            "Anzeige: HDR10-Metadaten: Spitze {} nit, Minimum {:.4} nit, MaxCLL {}, MaxFALL {}",
                            m.MaxMasteringLuminance,
                            m.MinMasteringLuminance as f32 / 10000.0,
                            m.MaxContentLightLevel,
                            m.MaxFrameAverageLightLevel
                        )),
                        Err(e) => protokoll::zeile(fehler("Anzeige: SetHDRMetaData", e)),
                    }
                }
                self.metadaten = Some(m);
            }
        }
        k
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
        // Vor der Sicht auf den Backbuffer: eine Umschaltung legt sie neu an.
        let k = self.ausgabe_fuehren(rect.is_some_and(|r| r.2 > 0 && r.3 > 0));
        let Some(rtv) = self.rtv.clone() else { return Praesentiert::Fehler("keine Sicht auf den Backbuffer".into()) };
        if let Err(e) = self.stufe2(&rtv, self.breite, self.hoehe, rect, ui_an, self.ausgabe, &k) {
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
        let vs_code = uebersetzen(b"vs_voll\0", b"vs_4_0\0")?;
        let ps1_code = uebersetzen(b"ps_umrechnen\0", b"ps_4_0\0")?;
        let ps2_code = uebersetzen(b"ps_anzeigen\0", b"ps_4_0\0")?;
        let ps1_pq_code = uebersetzen(b"ps_umrechnen_pq\0", b"ps_4_0\0")?;
        let ps2_pq_sdr_code = uebersetzen(b"ps_anzeigen_pq_sdr\0", b"ps_4_0\0")?;
        let ps2_hdr_code = uebersetzen(b"ps_anzeigen_hdr\0", b"ps_4_0\0")?;
        let mut vs = None;
        let mut ps_umrechnen = None;
        let mut ps_anzeigen = None;
        let mut ps_umrechnen_pq = None;
        let mut ps_anzeigen_pq_sdr = None;
        let mut ps_anzeigen_hdr = None;
        let mut sampler = None;
        let mut raster = None;
        unsafe {
            device.CreateVertexShader(&vs_code, None, Some(&mut vs)).map_err(|e| fehler("CreateVertexShader", e))?;
            device.CreatePixelShader(&ps1_code, None, Some(&mut ps_umrechnen)).map_err(|e| fehler("CreatePixelShader umrechnen", e))?;
            device.CreatePixelShader(&ps2_code, None, Some(&mut ps_anzeigen)).map_err(|e| fehler("CreatePixelShader anzeigen", e))?;
            device.CreatePixelShader(&ps1_pq_code, None, Some(&mut ps_umrechnen_pq)).map_err(|e| fehler("CreatePixelShader umrechnen_pq", e))?;
            device
                .CreatePixelShader(&ps2_pq_sdr_code, None, Some(&mut ps_anzeigen_pq_sdr))
                .map_err(|e| fehler("CreatePixelShader anzeigen_pq_sdr", e))?;
            device.CreatePixelShader(&ps2_hdr_code, None, Some(&mut ps_anzeigen_hdr)).map_err(|e| fehler("CreatePixelShader anzeigen_hdr", e))?;
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
        let konst_pq = konstpuffer(&device, std::mem::size_of::<KonstPq>() as u32)?;
        let konst_hdr = konstpuffer(&device, std::mem::size_of::<KonstHdr>() as u32)?;
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
            hwnd: HWND(std::ptr::null_mut()),
            ausgabe: Ausgabe::Sdr,
            umgeschaltet: None,
            hdr10_abgelehnt: false,
            schirm: None,
            schirm_zeit: None,
            monitor: 0,
            monitor_zeit: None,
            quelle: None,
            metadaten: None,
            matrix_gemeldet: false,
            vs: vs.ok_or("CreateVertexShader lieferte nichts")?,
            ps_umrechnen: ps_umrechnen.ok_or("CreatePixelShader lieferte nichts")?,
            ps_anzeigen: ps_anzeigen.ok_or("CreatePixelShader lieferte nichts")?,
            ps_umrechnen_pq: ps_umrechnen_pq.ok_or("CreatePixelShader lieferte nichts")?,
            ps_anzeigen_pq_sdr: ps_anzeigen_pq_sdr.ok_or("CreatePixelShader lieferte nichts")?,
            ps_anzeigen_hdr: ps_anzeigen_hdr.ok_or("CreatePixelShader lieferte nichts")?,
            sampler: sampler.ok_or("CreateSamplerState lieferte nichts")?,
            raster: raster.ok_or("CreateRasterizerState lieferte nichts")?,
            konst_format,
            konst_anzeige,
            konst_pq,
            konst_hdr,
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

    /// Zwischentextur in Bildgroesse, neu nur wenn Groesse oder Art nicht
    /// stimmen: B8G8R8A8 fuer SDR, R16G16B16A16_UNORM fuer PQ-Bilder.
    fn zwischen_sichern(&mut self, w: u32, h: u32, pq: bool) -> Result<(), String> {
        let alt = self.zwischen.as_ref().map(|z| (z.w, z.h, z.pq));
        if alt == Some((w, h, pq)) {
            return Ok(());
        }
        self.sichten_loesen();
        self.zwischen = None;
        let format = if pq { DXGI_FORMAT_R16G16B16A16_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM };
        let tex = self.textur_anlegen(
            w, h, format, D3D11_USAGE_DEFAULT,
            (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            0,
        )?;
        let srv = self.srv_anlegen(&tex)?;
        let rtv = self.rtv_anlegen(&tex)?;
        self.zwischen = Some(Zwischen { tex, srv, rtv, w, h, pq });
        // Nur der Wechsel der Art geht ins Protokoll, nicht jede neue Groesse.
        if alt.map(|a| a.2) != Some(pq) && (alt.is_some() || pq) {
            protokoll::zeile(format!(
                "Anzeige: Bild {} - Zwischentextur {w}x{h} {}",
                if pq { "PQ/BT.2020 (HDR10)" } else { "SDR" },
                if pq { "R16G16B16A16" } else { "B8G8R8A8" }
            ));
        }
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
    /// Stufe 1 in die Zwischentextur - fuer ein PQ-Bild (Transfer aus dem VUI
    /// des Bildes) Stufe 1 PQ. Das Bild darf danach fallen.
    pub fn bild_roh(&mut self, f: &ffmpeg::frame::Video) -> Result<(), String> {
        let farbe = crate::Ebenenbild::farbe(f);
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
        let pq = farbe.ist_pq();
        self.zwischen_sichern(w, h, pq)?;
        if pq {
            if farbe.matrix != hdr::MATRIX_2020_NCL && !self.matrix_gemeldet {
                self.matrix_gemeldet = true;
                protokoll::zeile(format!("Anzeige: PQ-Bild mit Matrix {} - gerechnet wird BT.2020-NCL", farbe.matrix));
            }
            self.stufe1(Some(konst_pq(fmt, farbe)))
        } else {
            self.stufe1(None)
        }
    }

    /// Ein fertiges RGB-Bild (Rueckfall, dunkles Bild): direkt in die
    /// Zwischentextur. 0x00RRGGBB liegt little-endian als B,G,R,0 - genau
    /// B8G8R8A8, der Shader liest nur .rgb.
    pub fn bild_rgb(&mut self, f: &Frame) -> Result<(), String> {
        let (w, h) = (f.width, f.height);
        if w == 0 || h == 0 || f.pixels.len() < (w as usize) * (h as usize) {
            return Err("RGB-Bild ist leer oder zu klein".into());
        }
        self.zwischen_sichern(w, h, false)?;
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

    /// Stufe 1: Ebenen -> Zwischentextur. Mit `pq` (ein PQ-Bild) ueber
    /// ps_umrechnen_pq in die R16G16B16A16-Zwischentextur, ganzzahlig wie
    /// `pq_stufe1`; sonst der heutige Weg (ps_umrechnen, bitidentisch zu to_rgb).
    fn stufe1(&self, pq: Option<KonstPq>) -> Result<(), String> {
        let e = self.ebenen.as_ref().ok_or("Ebenen fehlen")?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        self.sichten_loesen();
        self.konstanten_schreiben(
            &self.konst_format,
            &KonstFormat { sub: e.fmt.sub as u32, paar: e.fmt.paar as u32, schieb: e.fmt.schieb(), _f: 0 },
        )?;
        if let Some(k) = &pq {
            self.konstanten_schreiben(&self.konst_pq, k)?;
        }
        unsafe {
            self.ctx.OMSetRenderTargets(Some(&[Some(z.rtv.clone())]), None);
            self.ctx.RSSetViewports(Some(&[viewport(z.w, z.h)]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(if pq.is_some() { &self.ps_umrechnen_pq } else { &self.ps_umrechnen }, None);
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.konst_format.clone())]));
            if pq.is_some() {
                self.ctx.PSSetConstantBuffers(2, Some(&[Some(self.konst_pq.clone())]));
            }
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
    /// oder die Testtextur (`offscreen`, `offscreen_hdr`). Der Shader folgt
    /// Ausgabe und Bild: SDR-Bild auf SDR der heutige (bitgleich), PQ-Bild auf
    /// SDR abgebildet, alles auf HDR10 ueber ps_anzeigen_hdr mit den Zahlen `k`.
    #[allow(clippy::too_many_arguments)]
    fn stufe2(
        &self,
        ziel: &ID3D11RenderTargetView,
        ww: u32,
        wh: u32,
        rect: Option<(i32, i32, u32, u32)>,
        ui_an: bool,
        ausgabe: Ausgabe,
        k: &KonstHdr,
    ) -> Result<(), String> {
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
        let ka = KonstAnzeige {
            rect: [rect.0, rect.1, rect.2 as i32, rect.3 as i32],
            bild: [fw, fh],
            schritt,
            modus: (bild_da as u32) | ((ui_da as u32) << 1),
            _a: [0; 3],
            grund: [((bg >> 16) & 255) as f32 / 255.0, ((bg >> 8) & 255) as f32 / 255.0, (bg & 255) as f32 / 255.0, 1.0],
        };
        let pq = bild_da && bild.is_some_and(|z| z.pq);
        let ps = match (ausgabe, pq) {
            (Ausgabe::Sdr, false) => &self.ps_anzeigen,
            (Ausgabe::Sdr, true) => &self.ps_anzeigen_pq_sdr,
            (Ausgabe::Hdr10, _) => &self.ps_anzeigen_hdr,
        };
        self.sichten_loesen();
        self.konstanten_schreiben(&self.konst_anzeige, &ka)?;
        if ausgabe == Ausgabe::Hdr10 || pq {
            self.konstanten_schreiben(&self.konst_hdr, k)?;
        }
        unsafe {
            self.ctx.OMSetRenderTargets(Some(&[Some(ziel.clone())]), None);
            self.ctx.RSSetViewports(Some(&[viewport(ww, wh)]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(ps, None);
            self.ctx.PSSetConstantBuffers(1, Some(&[Some(self.konst_anzeige.clone())]));
            self.ctx.PSSetConstantBuffers(3, Some(&[Some(self.konst_hdr.clone())]));
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
        let mut aus = self.auslesen_roh(tex, w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        for p in aus.iter_mut() {
            *p &= 0x00ff_ffff;
        }
        Ok(aus)
    }

    /// Wie `auslesen`, fuer jedes Format mit vier Byte je Punkt, ohne Maske
    /// (R10G10B10A2: R in Bit 0-9, G 10-19, B 20-29).
    fn auslesen_roh(&self, tex: &ID3D11Texture2D, w: u32, h: u32, format: DXGI_FORMAT) -> Result<Vec<u32>, String> {
        let staging = self.textur_anlegen(w, h, format, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
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
        Ok(aus)
    }

    /// Nur Test: Stufe 2 in eine eigene B8G8R8A8-Zieltextur (Ausgabe SDR),
    /// dann zurueck in den Hauptspeicher. Ein PQ-Bild wird dabei abgebildet
    /// wie auf einem SDR-Schirm.
    pub fn offscreen(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<Vec<u32>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        let ziel = self.textur_anlegen(ww, wh, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let rtv = self.rtv_anlegen(&ziel)?;
        let pq = self.zwischen.as_ref().is_some_and(|z| z.pq);
        let k = hdr_zahlen(self.quelle.as_ref(), None, Ausgabe::Sdr, pq);
        self.stufe2(&rtv, ww, wh, rect, ui_an, Ausgabe::Sdr, &k)?;
        self.auslesen(&ziel, ww, wh)
    }

    /// Nur Test: Stufe 2 in eine eigene R10G10B10A2-Zieltextur, wie auf einem
    /// HDR-Schirm mit dieser Lage (Ausgabe HDR10), zurueck als rohe Worte.
    pub fn offscreen_hdr(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool, schirm: &hdr::Schirm) -> Result<Vec<u32>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        let format = Ausgabe::Hdr10.format();
        let ziel = self.textur_anlegen(ww, wh, format, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let rtv = self.rtv_anlegen(&ziel)?;
        let pq = self.zwischen.as_ref().is_some_and(|z| z.pq);
        let k = hdr_zahlen(self.quelle.as_ref(), Some(schirm), Ausgabe::Hdr10, pq);
        self.stufe2(&rtv, ww, wh, rect, ui_an, Ausgabe::Hdr10, &k)?;
        self.auslesen_roh(&ziel, ww, wh, format)
    }

    /// Nur Test: das Ergebnis von Stufe 1, 1:1 (nur SDR - die PQ-Zwischentextur
    /// hat acht Byte je Punkt und wird ueber Stufe 2 geprueft).
    pub fn zwischen_auslesen(&mut self) -> Result<(Vec<u32>, u32, u32), String> {
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        if z.pq {
            return Err("PQ-Zwischentextur laesst sich nicht als BGRA lesen".into());
        }
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
    probebild_muster(p, w, h, probewert)
}

/// Wie `probebild`, mit dem Muster der PQ-Probebilder (unten der neutrale
/// Streifen, anzeigeprobe::probewert_pq) - dasselbe wie in der Mac-Anzeige.
/// Das VUI setzt `als_pq`.
fn probebild_pq(p: ffmpeg::format::Pixel, w: u32, h: u32) -> Option<ffmpeg::frame::Video> {
    probebild_muster(p, w, h, probewert_pq)
}

fn probebild_muster(p: ffmpeg::format::Pixel, w: u32, h: u32, muster: fn(usize, usize, usize, usize, usize, u32) -> u32) -> Option<ffmpeg::frame::Video> {
    let fmt = ebenen_format(p)?;
    let max = hoechstwert(p, fmt);
    let mut f = ffmpeg::frame::Video::new(p, w, h);
    let wert = |x: usize, y: usize, bw: usize, bh: usize, ebene: usize| -> u32 { muster(x, y, bw, bh, ebene, max) };
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

    // HDR10: die Formate, in denen PQ ankommt (Software-Decoder 4:4:4 und
    // 4:2:0, cuvid oben buendig und P010), dazu zweimal begrenzter Bereich.
    println!("HDR10: PQ-Ebenen gegen hdr.rs - Durchreichen exakt, sonst +-1 (CPU-Tabellen +-2)");
    let pq_formate = [Pixel::YUV444P10LE, Pixel::YUV444P10MSBLE, Pixel::YUV444P16LE, Pixel::YUV420P10LE, Pixel::P010LE];
    for p in pq_formate {
        for (w, h) in [(257u32, 131u32), (1920, 1080)] {
            ok &= pq_pruefen(&mut gpu, p, w, h, true, verzeichnis, lang, &mut u);
            for z in protokoll::abholen() {
                println!("    {z}");
            }
        }
    }
    for p in [Pixel::YUV444P10LE, Pixel::P010LE] {
        ok &= pq_pruefen(&mut gpu, p, 257, 131, false, verzeichnis, lang, &mut u);
    }
    ok &= sdr_auf_hdr10_pruefen(&mut gpu, 257, 131);
    for z in protokoll::abholen() {
        println!("    {z}");
    }
    println!("{}", if ok { "Anzeigetest bestanden" } else { "Anzeigetest NICHT bestanden" });
    ok
}

// ------------------------------------------------ --anzeigetest: HDR10

/// Das VUI eines HDR10-Stroms an einem Probebild: PQ, BT.2020, BT.2020-NCL,
/// voller oder begrenzter Bereich.
fn als_pq(mut f: ffmpeg::frame::Video, voll: bool) -> ffmpeg::frame::Video {
    use ffmpeg::color;
    f.set_color_transfer_characteristic(color::TransferCharacteristic::SMPTE2084);
    f.set_color_primaries(color::Primaries::BT2020);
    f.set_color_space(color::Space::BT2020NCL);
    f.set_color_range(if voll { color::Range::JPEG } else { color::Range::MPEG });
    f
}

/// Die drei 10-Bit-Codes eines Bildpunkts, so gelesen wie Stufe 1 PQ sie liest
/// (8 Bit << 2, 10 Bit LE wie sie sind, oben buendig >> 6).
fn codes10(f: &ffmpeg::frame::Video, fmt: EbenenFormat, x: usize, y: usize) -> (i32, i32, i32) {
    let wert = |ebene: usize, i: usize, zeile: usize| -> i32 {
        let (d, s) = (f.data(ebene), f.stride(ebene));
        let v = if fmt.bits == 8 { d[zeile * s + i] as u32 } else { u16::from_le_bytes([d[zeile * s + 2 * i], d[zeile * s + 2 * i + 1]]) as u32 };
        match fmt.bits {
            8 => (v << 2) as i32,
            10 => v as i32,
            _ => (v >> 6) as i32,
        }
    };
    let (cx, cy) = if fmt.sub { (x / 2, y / 2) } else { (x, y) };
    let (cb, cr) = if fmt.paar { (wert(1, 2 * cx, cy), wert(1, 2 * cx + 1, cy)) } else { (wert(1, cx, cy), wert(2, cx, cy)) };
    (wert(0, x, y), cb, cr)
}

/// Soll einer HDR10-Ausgabe mit Abbildung (nicht durchgereicht), in f64 aus
/// hdr.rs: Codes -> Y'CbCr -> R'G'B' -> nit -> relativ zu W_h -> Abbildung Hs
/// auf Hd -> mal SDR-Weiss des Schirms -> PQ-Code.
fn soll_hdr(c: (i32, i32, i32), begrenzt: bool, k: &KonstHdr) -> [u16; 3] {
    let (y, cb, cr) = hdr::normieren10(c.0, c.1, c.2, begrenzt);
    let e = hdr::ycbcr_nach_rgb_2020(y, cb, cr);
    let rel = e.map(|v| (hdr::pq_eotf(v as f64) / k.weiss_quelle as f64) as f32);
    hdr::rgb_abbilden(rel, k.hs, k.hd).map(|v| hdr::pq_code10(v as f64 * k.weiss_ziel as f64))
}

/// Soll der Oberflaeche ueber einem HDR10-Punkt (Code `video`): wie
/// ps_anzeigen_hdr - Farbe aus der vormultiplizierten Oberflaeche
/// zurueckgewonnen, sRGB -> linear, BT.709 -> BT.2020, mal SDR-Weiss des
/// Schirms, "over" in linearem Licht, PQ. Ganz durchsichtig: `video` unberuehrt.
fn soll_ui_hdr(video: [u16; 3], o: u32, weiss_ziel: f64) -> [u16; 3] {
    let a = ((o >> 24) & 255) as f64 / 255.0;
    let rgb = [(o >> 16) & 255, (o >> 8) & 255, o & 255].map(|c| c as f64 / 255.0);
    let deckung = 1.0 - a;
    if deckung <= 0.0 && rgb.iter().all(|&c| c <= 0.0) {
        return video;
    }
    let farbe = if deckung > 0.0 { rgb.map(|c| (c / deckung).min(1.0)) } else { [0.0; 3] };
    let ui = hdr::mal(&hdr::M_709_NACH_2020, farbe.map(|c| hdr::srgb_eotf(c) as f32));
    [0, 1, 2].map(|i| hdr::pq_code10(hdr::pq_eotf(video[i] as f64 / 1023.0) * a + ui[i] as f64 * weiss_ziel * deckung))
}

/// Ein Vergleich in zehn Bit (R10G10B10A2 aus der Karte gegen das Soll):
/// groesste und mittlere Abweichung ueber alle Kanaele, eine Zeile; bei
/// Ueberschreitung der erste Punkt mit der groessten Abweichung.
fn fall_pruefen10(name: &str, w: u32, h: u32, fall: &str, soll: &[[u16; 3]], karte: &[u32], toleranz: u32) -> bool {
    if karte.len() < soll.len() {
        println!("{name:<15} {w:>4}x{h:<4} {fall:<11} FEHLER: Puffer zu klein ({} statt {})", karte.len(), soll.len());
        return false;
    }
    let mut max = 0u32;
    let mut summe = 0u64;
    let mut stelle = None;
    for (i, (s, &k)) in soll.iter().zip(karte).enumerate() {
        let ist = [k & 0x3ff, (k >> 10) & 0x3ff, (k >> 20) & 0x3ff];
        let d = (0..3).map(|c| ist[c].abs_diff(s[c] as u32)).max().unwrap_or(0);
        if d > max {
            max = d;
            stelle = Some((i, ist, *s));
        }
        summe += d as u64;
    }
    let ok = max <= toleranz;
    println!(
        "{name:<15} {w:>4}x{h:<4} {fall:<11} max {max:>3}  mittel {:.4}  {}",
        summe as f64 / soll.len().max(1) as f64,
        if ok { "ok".to_string() } else { format!("FEHLER (Toleranz {toleranz}, zehn Bit)") }
    );
    if let (false, Some((i, ist, s))) = (ok, stelle) {
        println!("    groesste Abweichung bei ({}, {}): Karte {ist:?}, Soll {s:?}", i % w as usize, i / w as usize);
    }
    ok
}

/// Ein PQ-Format in einer Groesse (Strominfo: SDR-Weiss 203 nit, Mastering
/// 1000 nit): (1) HDR-Schirm mit demselben Weiss - durchgereicht, Codes exakt
/// wie `pq_stufe1`; (2) Schirm-Weiss 1,25-fach (253,75 nit, Spitze 1000 nit) -
/// abgebildet, +-1 gegen hdr.rs; (3) SDR-Schirm - B8G8R8A8 gegen
/// pq_nach_srgb8 +-1 und gegen den CPU-Weg (to_rgb_mit, Tabellen) +-2;
/// (4) skaliert auf beide Ausgaben, nur dass es laeuft; (5) ab 700x400 die
/// Oberflaeche ueber dem durchgereichten Bild, linear ueberblendet, +-1.
#[allow(clippy::too_many_arguments)]
fn pq_pruefen(gpu: &mut Gpu, p: ffmpeg::format::Pixel, w: u32, h: u32, voll: bool, verzeichnis: &str, lang: &'static crate::strings::Lang, u: &mut ui::Ui) -> bool {
    let name = format!("{p:?}{}", if voll { "" } else { " begr." });
    let (Some(fmt), Some(roh)) = (ebenen_format(p), probebild_pq(p, w, h)) else {
        println!("{name:<15} {w:>4}x{h:<4} kein Probebild: Format unbekannt");
        return false;
    };
    let bild = als_pq(roh, voll);
    let farbe = crate::Ebenenbild::farbe(&bild);
    if !farbe.ist_pq() || farbe.voll != voll {
        println!("{name:<15} {w:>4}x{h:<4} VUI falsch gelesen: {}", farbe.text());
        return false;
    }
    let quelle = hdr::InfoV1 {
        farbe: hdr::Farbe::PQ,
        grund: hdr::GRUND_AKTIV,
        sdr_weiss_nit: 203,
        master_max_nit: 1000,
        master_min_zehntausendstel: 50,
        max_cll: 0,
        max_fall: 0,
    };
    gpu.quelle_setzen(Some(&quelle));
    if let Err(e) = gpu.bild_roh(&bild) {
        println!("{name:<15} {w:>4}x{h:<4} Stufe 1 PQ: {e}");
        return false;
    }
    let k = konst_pq(fmt, farbe);
    let (wu, hu) = (w as usize, h as usize);
    let codes: Vec<(i32, i32, i32)> = (0..hu).flat_map(|y| (0..wu).map(move |x| (x, y))).map(|(x, y)| codes10(&bild, fmt, x, y)).collect();
    let rect = Some((0, 0, w, h));
    let mut ok = true;

    // (1) Durchreichen.
    let schirm_gleich = hdr::Schirm { hdr: true, sdr_weiss_nit: 203.0, spitze_nit: 1000.0, vollbild_spitze_nit: 600.0, kopfraum_potentiell: 4.93, kopfraum_aktuell: 4.93 };
    let durch: Vec<[u16; 3]> = codes.iter().map(|c| pq_stufe1(&k, c.0, c.1, c.2).map(pq_durch)).collect();
    if hdr_zahlen(Some(&quelle), Some(&schirm_gleich), Ausgabe::Hdr10, true).durchreichen == 0 {
        println!("{name:<15} {w:>4}x{h:<4} FEHLER: gleiches Weiss, und doch nicht durchgereicht");
        ok = false;
    }
    match gpu.offscreen_hdr(w, h, rect, false, &schirm_gleich) {
        Ok(aus) => ok &= fall_pruefen10(&name, w, h, "PQ durch", &durch, &aus, 0),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ durch: {e}");
            ok = false;
        }
    }

    // (2) Weiss-Angleichung 1,25 mit Abbildung (Kopfraum 3,94 < Hs 4,93).
    let schirm_hell = hdr::Schirm { sdr_weiss_nit: 253.75, ..schirm_gleich };
    let kh = hdr_zahlen(Some(&quelle), Some(&schirm_hell), Ausgabe::Hdr10, true);
    let soll: Vec<[u16; 3]> = codes.iter().map(|&c| soll_hdr(c, !voll, &kh)).collect();
    match gpu.offscreen_hdr(w, h, rect, false, &schirm_hell) {
        Ok(aus) => ok &= fall_pruefen10(&name, w, h, "PQ Weiss", &soll, &aus, 1),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ Weiss: {e}");
            ok = false;
        }
    }

    // (3) SDR-Schirm.
    let ab = hdr::Abbildung::sdr(Some(&quelle));
    let referenz: Vec<u32> = codes
        .iter()
        .map(|&(y, cb, cr)| {
            let (yn, cbn, crn) = hdr::normieren10(y, cb, cr, !voll);
            let v = hdr::pq_nach_srgb8(yn, cbn, crn, &ab);
            ((v[0] as u32) << 16) | ((v[1] as u32) << 8) | v[2] as u32
        })
        .collect();
    match gpu.offscreen(w, h, rect, false) {
        Ok(aus) => {
            ok &= fall_pruefen(&name, w, h, "PQ->SDR", &referenz, &aus, w, h, 1, Some(verzeichnis), lang);
            match crate::to_rgb_mit(&bild, Some(&quelle)) {
                Ok(cpu) => ok &= fall_pruefen(&name, w, h, "PQ CPU", &cpu.pixels, &aus, w, h, 2, Some(verzeichnis), lang),
                Err(e) => {
                    println!("{name:<15} {w:>4}x{h:<4} to_rgb_mit: {e}");
                    ok = false;
                }
            }
        }
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ->SDR: {e}");
            ok = false;
        }
    }

    // (4) Skaliert: der Weg ueber den Filter, auf beide Ausgaben.
    let (bw, bh) = (700u32, 400u32);
    let r = crate::ziel_rechteck(bw, bh, w, h, false);
    match gpu.offscreen(bw, bh, Some(r), false).and_then(|_| gpu.offscreen_hdr(bw, bh, Some(r), false, &schirm_hell)) {
        Ok(_) => println!("{name:<15} {w:>4}x{h:<4} {:<11} ok (laeuft, ohne Vergleich)", "PQ skal."),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ skaliert: {e}");
            ok = false;
        }
    }

    // (5) Oberflaeche ueber dem durchgereichten Bild.
    if w >= 700 && h >= 400 {
        let mut leer = vec![0xff00_0000u32; wu * hu];
        let kasten = {
            let mut c = ui::Canvas::neu(&mut leer, wu, hu);
            oberflaeche_probe(&mut c, u, &format!("{name} {w}x{h} HDR10"));
            c.kasten_nehmen().unwrap_or(ui::Rect { x: 0, y: 0, w: 0, h: 0 })
        };
        let soll: Vec<[u16; 3]> = durch.iter().zip(&leer).map(|(&v, &o)| soll_ui_hdr(v, o, 203.0)).collect();
        match gpu.oberflaeche_hochladen(&leer, w, h, kasten).and_then(|_| gpu.offscreen_hdr(w, h, rect, true, &schirm_gleich)) {
            Ok(aus) => ok &= fall_pruefen10(&name, w, h, "PQ UI HDR", &soll, &aus, 1),
            Err(e) => {
                println!("{name:<15} {w:>4}x{h:<4} PQ UI HDR: {e}");
                ok = false;
            }
        }
    }
    gpu.quelle_setzen(None);
    ok
}

/// Ein SDR-Bild auf der HDR10-Ausgabe (so zeigt die Anzeige SDR waehrend der
/// Umschaltsperre): sRGB -> linear, BT.709 -> BT.2020, mal SDR-Weiss des
/// Schirms (240 nit), PQ - gegen to_rgb und hdr.rs, +-1.
fn sdr_auf_hdr10_pruefen(gpu: &mut Gpu, w: u32, h: u32) -> bool {
    let p = ffmpeg::format::Pixel::YUV444P10LE;
    let name = format!("{p:?}");
    let Some(bild) = probebild(p, w, h) else { return false };
    let referenz = match crate::to_rgb(&bild) {
        Ok(f) => f,
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} to_rgb: {e}");
            return false;
        }
    };
    gpu.quelle_setzen(None);
    if let Err(e) = gpu.bild_roh(&bild) {
        println!("{name:<15} {w:>4}x{h:<4} SDR->HDR10: {e}");
        return false;
    }
    let schirm = hdr::Schirm { hdr: true, sdr_weiss_nit: 240.0, spitze_nit: 1000.0, vollbild_spitze_nit: 600.0, kopfraum_potentiell: 4.17, kopfraum_aktuell: 4.17 };
    let soll: Vec<[u16; 3]> = referenz
        .pixels
        .iter()
        .map(|&px| {
            let lin = [(px >> 16) & 255, (px >> 8) & 255, px & 255].map(|c| hdr::srgb_eotf(c as f64 / 255.0) as f32);
            hdr::mal(&hdr::M_709_NACH_2020, lin).map(|c| hdr::pq_code10(c as f64 * 240.0))
        })
        .collect();
    match gpu.offscreen_hdr(w, h, Some((0, 0, w, h)), false, &schirm) {
        Ok(aus) => fall_pruefen10(&name, w, h, "SDR->HDR10", &soll, &aus, 1),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} SDR->HDR10: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quelle(weiss: u16, cll: u16) -> hdr::InfoV1 {
        hdr::InfoV1 {
            farbe: hdr::Farbe::PQ,
            grund: hdr::GRUND_AKTIV,
            sdr_weiss_nit: weiss,
            master_max_nit: 1000,
            master_min_zehntausendstel: 0,
            max_cll: cll,
            max_fall: 0,
        }
    }

    fn schirm(weiss: f32, spitze: f32) -> hdr::Schirm {
        hdr::Schirm { hdr: true, sdr_weiss_nit: weiss, spitze_nit: spitze, ..hdr::Schirm::default() }
    }

    /// Die Konstanten liegen so, wie der Shader sie liest: Groessen, Lage und
    /// Reihenfolge in den cbuffern FormatPq (b2) und Hdr (b3).
    #[test]
    fn konstanten_wie_im_shader() {
        assert_eq!(std::mem::size_of::<KonstPq>(), 32);
        assert_eq!(std::mem::offset_of!(KonstPq, c), 16);
        assert_eq!(std::mem::size_of::<KonstHdr>(), 32);
        assert_eq!(std::mem::offset_of!(KonstHdr, quelle_pq), 16);
        let reihenfolge = |cb: &str, namen: &[&str]| {
            let t = &SHADER[SHADER.find(cb).unwrap_or_else(|| panic!("{cb} fehlt"))..];
            let t = &t[..t.find("};").expect("Ende des cbuffer")];
            let lage: Vec<usize> = namen.iter().map(|n| t.find(n).unwrap_or_else(|| panic!("{n} fehlt in {cb}"))).collect();
            assert!(lage.windows(2).all(|w| w[0] < w[1]), "{cb}: Reihenfolge {namen:?}");
        };
        reihenfolge("cbuffer FormatPq : register(b2)", &["pq_ky", "pq_y0", "pq_links", "pq_rechts", "pq_c"]);
        reihenfolge("cbuffer Hdr : register(b3)", &["h_weiss_quelle", "h_hs", "h_hd", "h_weiss_ziel", "h_quelle_pq", "h_durchreichen", "uint2 _h"]);
        for e in ["float4 ps_umrechnen_pq(", "float4 ps_anzeigen_pq_sdr(", "float4 ps_anzeigen_hdr("] {
            assert_eq!(SHADER.matches(e).count(), 1, "{e}");
        }
    }

    /// PQ, sRGB, beide Matrizen und die Abbildung kommen aus hdr_hlsl.rs
    /// (dort gegen hdr.rs geprueft) und stehen nicht doppelt in SHADER; hier
    /// bleibt die Skala der PQ-Zwischentextur.
    #[test]
    fn pq_zahlen_wie_in_hdr_rs() {
        assert!(hlsl_quelle().starts_with(crate::hdr_hlsl::GEMEINSAM_PQ) && hlsl_quelle().ends_with(SHADER));
        for doppelt in ["float3 pq_eotf(", "float3 srgb_oetf(", "float3 acht_bit(", "float3x3 M_2020_NACH_709", "float abbilden(", "float4 vs_voll("] {
            assert!(!SHADER.contains(doppelt), "{doppelt} gehoert nach hdr_hlsl.rs");
            assert_eq!(hlsl_quelle().matches(doppelt).count(), 1, "{doppelt}");
        }
        assert_eq!(PQ_S_MAX, 1023 * 64);
        assert!(SHADER.contains("clamp(int3(r, g, b), 0, 65472)") && SHADER.contains("(65535.0 / 65472.0)"));
    }

    /// Stufe 1 PQ rechnet ganzzahlig, was hdr.rs in Gleitkomma rechnet:
    /// hoechstens 1/64 Code daneben, voll wie begrenzt. Dazu die Lesart der
    /// Ebenen und das Runden beim Durchreichen.
    #[test]
    fn stufe1_pq_wie_hdr_rs() {
        let fmt = |bits: u8| EbenenFormat { sub: false, bits, paar: false, begrenzt: false };
        for voll in [true, false] {
            let k = konst_pq(fmt(10), hdr::Farbe { voll, ..hdr::Farbe::PQ });
            let mut max = 0f32;
            for y in (0..1024).step_by(31).chain([1023]) {
                for cb in (0..1024).step_by(17).chain([1023]) {
                    for cr in (0..1024).step_by(13).chain([1023]) {
                        let s = pq_stufe1(&k, y, cb, cr);
                        let (yn, cbn, crn) = hdr::normieren10(y, cb, cr, !voll);
                        let e = hdr::ycbcr_nach_rgb_2020(yn, cbn, crn);
                        for c in 0..3 {
                            max = max.max((s[c] as f32 - e[c].clamp(0.0, 1.0) * PQ_S_MAX as f32).abs());
                        }
                    }
                }
            }
            assert!(max <= 1.0, "voll {voll}: {max} x 1/64 Code daneben");
        }
        // Begrenzt auch, wenn nur das Format es sagt (VideoToolbox x444).
        assert_eq!(konst_pq(EbenenFormat { begrenzt: true, ..fmt(10) }, hdr::Farbe::PQ).y0, 64);
        assert_eq!(konst_pq(fmt(10), hdr::Farbe::PQ).y0, 0);
        assert_eq!(konst_pq(fmt(10), hdr::Farbe::PQ).ky, 64 * 4096);
        let lesart = |bits| {
            let k = konst_pq(fmt(bits), hdr::Farbe::PQ);
            (k.links, k.rechts)
        };
        assert_eq!((lesart(8), lesart(10), lesart(16)), ((2, 0), (0, 0), (0, 6)));
        // Schwarz, Weiss, Grau 520 (100 nit) und die Grenzen.
        let k = konst_pq(fmt(10), hdr::Farbe::PQ);
        assert_eq!(pq_stufe1(&k, 0, 512, 512), [0; 3]);
        assert_eq!(pq_stufe1(&k, 1023, 512, 512), [PQ_S_MAX; 3]);
        assert_eq!(pq_stufe1(&k, 520, 512, 512).map(pq_durch), [520; 3]);
        assert_eq!((pq_durch(31), pq_durch(32), pq_durch(PQ_S_MAX)), (0, 1, 1023));
    }

    /// Durchgereicht wird nur bei gleichem SDR-Weiss (+-1 nit) und wenn die
    /// Quelle in den Kopfraum passt; sonst abgebildet. Vorgaben ohne Angaben:
    /// Quelle 203 / 1000 nit, Schirm 80 nit Weiss (Windows), 1000 nit Spitze.
    #[test]
    fn durchreichen_nur_bei_gleichem_weiss_und_genug_kopfraum() {
        let q = quelle(203, 0);
        let k = hdr_zahlen(Some(&q), Some(&schirm(203.0, 1000.0)), Ausgabe::Hdr10, true);
        assert_eq!((k.durchreichen, k.quelle_pq), (1, 1));
        assert!((k.hs - 1000.0 / 203.0).abs() < 1e-6 && (k.hd - 1000.0 / 203.0).abs() < 1e-6);
        // 1 nit daneben reicht noch durch (Spitze mit Luft: Hs <= Hd bleibt), 2 nit nicht mehr.
        assert_eq!(hdr_zahlen(Some(&q), Some(&schirm(204.0, 1100.0)), Ausgabe::Hdr10, true).durchreichen, 1);
        assert_eq!(hdr_zahlen(Some(&q), Some(&schirm(202.0, 1100.0)), Ausgabe::Hdr10, true).durchreichen, 1);
        assert_eq!(hdr_zahlen(Some(&q), Some(&schirm(205.0, 1100.0)), Ausgabe::Hdr10, true).durchreichen, 0);
        // Weiss 1 nit daneben, Spitze genau 1000 nit: Hs 4,93 > Hd 4,90 - abbilden.
        assert_eq!(hdr_zahlen(Some(&q), Some(&schirm(204.0, 1000.0)), Ausgabe::Hdr10, true).durchreichen, 0);
        // Der Schirm ist dunkler als die Quelle: abbilden.
        let k = hdr_zahlen(Some(&q), Some(&schirm(203.0, 600.0)), Ausgabe::Hdr10, true);
        assert_eq!(k.durchreichen, 0);
        assert!((k.hd - 600.0 / 203.0).abs() < 1e-6);
        // MaxCLL geht vor Mastering: 400 nit passen auf 600.
        assert_eq!(hdr_zahlen(Some(&quelle(203, 400)), Some(&schirm(203.0, 600.0)), Ausgabe::Hdr10, true).durchreichen, 1);
        // Schirm ohne Angaben.
        let k = hdr_zahlen(Some(&q), Some(&hdr::Schirm { hdr: true, ..hdr::Schirm::default() }), Ausgabe::Hdr10, true);
        assert_eq!((k.weiss_ziel, k.hd, k.durchreichen), (80.0, 12.5, 0));
        // Ohne Strominfo: 203 / 1000 nit.
        let k = hdr_zahlen(None, Some(&schirm(203.0, 1000.0)), Ausgabe::Hdr10, true);
        assert_eq!((k.weiss_quelle, k.durchreichen), (203.0, 1));
        // SDR-Ausgabe: Kopfraum 1, nie durch; ein SDR-Bild nie durch.
        let k = hdr_zahlen(Some(&q), Some(&schirm(203.0, 1000.0)), Ausgabe::Sdr, true);
        assert_eq!((k.hd, k.durchreichen), (1.0, 0));
        assert_eq!(hdr_zahlen(Some(&q), Some(&schirm(203.0, 1000.0)), Ausgabe::Hdr10, false).durchreichen, 0);
    }

    /// HDR10 nur fuer ein PQ-Bild auf einem HDR-Schirm, den DXGI nicht
    /// abgelehnt hat; umgeschaltet hoechstens alle 0,5 s.
    #[test]
    fn hdr10_nur_fuer_pq_auf_hdr_schirm() {
        for pq in [false, true] {
            for schirm_hdr in [false, true] {
                for abgelehnt in [false, true] {
                    let soll = if pq && schirm_hdr && !abgelehnt { Ausgabe::Hdr10 } else { Ausgabe::Sdr };
                    assert_eq!(ausgabe_soll(pq, schirm_hdr, abgelehnt), soll, "{pq} {schirm_hdr} {abgelehnt}");
                }
            }
        }
        assert!(umschalten_jetzt(Ausgabe::Sdr, Ausgabe::Hdr10, None));
        assert!(!umschalten_jetzt(Ausgabe::Sdr, Ausgabe::Hdr10, Some(Duration::from_millis(499))));
        assert!(umschalten_jetzt(Ausgabe::Hdr10, Ausgabe::Sdr, Some(Duration::from_millis(500))));
        assert!(!umschalten_jetzt(Ausgabe::Hdr10, Ausgabe::Hdr10, None));
        assert!(!umschalten_jetzt(Ausgabe::Sdr, Ausgabe::Sdr, Some(Duration::from_secs(9))));
        assert_eq!((Ausgabe::Hdr10.format(), Ausgabe::Hdr10.farbraum()), (DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020));
        assert_eq!((Ausgabe::Sdr.format(), Ausgabe::Sdr.farbraum()), (DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709));
    }

    /// Ein Ablauf wie in der Sitzung, Present fuer Present: das erste PQ-Bild
    /// auf dem HDR-Schirm schaltet sofort; ein SDR-Bild 0,1 s danach wartet
    /// die Sperre ab (bis dahin rechnet ps_anzeigen_hdr es um); zurueck zu PQ
    /// ebenso; "HDR verwenden" aus schaltet auf SDR; lehnt DXGI ab, bleibt SDR.
    #[test]
    fn umschalten_im_ablauf() {
        // (Zeit in ms, Bild PQ, Schirm HDR, abgelehnt, Ausgabe danach)
        let ablauf = [
            (0, false, true, false, Ausgabe::Sdr),
            (10, true, true, false, Ausgabe::Hdr10),
            (110, false, true, false, Ausgabe::Hdr10),
            (509, false, true, false, Ausgabe::Hdr10),
            (510, false, true, false, Ausgabe::Sdr),
            (700, true, true, false, Ausgabe::Sdr),
            (1010, true, true, false, Ausgabe::Hdr10),
            (5000, true, false, false, Ausgabe::Sdr),
            (9000, true, true, true, Ausgabe::Sdr),
            (9500, true, true, false, Ausgabe::Hdr10),
        ];
        let (mut ist, mut zuletzt): (Ausgabe, Option<u64>) = (Ausgabe::Sdr, None);
        for (t, pq, schirm_hdr, abgelehnt, danach) in ablauf {
            let seit = zuletzt.map(|z| Duration::from_millis(t - z));
            if let Some(a) = ausgabe_wechsel(ist, pq, schirm_hdr, abgelehnt, seit) {
                ist = a;
                zuletzt = Some(t);
            }
            assert_eq!(ist, danach, "bei {t} ms");
        }
    }

    /// Lehnt DXGI HDR10 ab, meldet die Anzeige keine HDR-Darstellung (Bit 1),
    /// und der Host entscheidet SDR (Grund 5) - auch auf einem HDR-Schirm mit
    /// HDR an. Ohne Ablehnung bleibt es HDR.
    #[test]
    fn abgelehntes_hdr10_ergibt_sdr_beim_host() {
        let s = schirm(203.0, 1000.0);
        let a = hdr::Anzeige::fuer_client(Some(&s), hdr_darstellung_moeglich(true), true);
        assert!(!a.darstellung() && a.schirm_hdr());
        assert_eq!(hdr::hdr_entscheiden(true, true, 0, Some(&a)), hdr::GRUND_CLIENT_OHNE_DARSTELLUNG);
        let a = hdr::Anzeige::fuer_client(Some(&s), hdr_darstellung_moeglich(false), true);
        assert!(a.darstellung());
        assert_eq!(hdr::hdr_entscheiden(true, true, 0, Some(&a)), hdr::GRUND_AKTIV);
    }

    /// Die Metadaten: BT.2020 und D65, als Spitze die abgebildete Spitze am
    /// Schirm, Minimum aus der Strominfo oder 0,005 nit.
    #[test]
    fn metadaten_mit_abgebildeter_spitze() {
        let q = quelle(203, 0);
        let k = hdr_zahlen(Some(&q), Some(&schirm(240.0, 600.0)), Ausgabe::Hdr10, true);
        let m = hdr10_metadaten(&k, Some(&q));
        assert_eq!((m.MaxMasteringLuminance, m.MaxContentLightLevel, m.MinMasteringLuminance), (600, 600, 50));
        assert_eq!((m.RedPrimary, m.GreenPrimary, m.BluePrimary, m.WhitePoint), ([35400, 14600], [8500, 39850], [6550, 2300], [15635, 16450]));
        let k = hdr_zahlen(Some(&q), Some(&schirm(203.0, 1000.0)), Ausgabe::Hdr10, true);
        let m = hdr10_metadaten(&k, Some(&hdr::InfoV1 { master_min_zehntausendstel: 10, max_fall: 2000, ..q }));
        assert_eq!((m.MaxMasteringLuminance, m.MinMasteringLuminance, m.MaxFrameAverageLightLevel), (1000, 10, 1000));
    }

    /// Die Schirmerkennung am echten System (die Windows-VM hat einen
    /// virtuellen Bildschirm; ohne Desktop, etwa ueber SSH, gibt DXGI keine
    /// Ausgaenge her - dann steht nur da, was fehlt): der Monitor des
    /// Desktop-Fensters, alle Ausgaenge, alle Pfade und die Zuordnung. Wo es
    /// eine gibt, stammt sie von genau diesem Monitor. Die Faelle mit zwei
    /// Karten prueft schirmerkennung.rs an Listen.
    #[test]
    fn schirmerkennung_am_system() {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        let hwnd = unsafe { GetDesktopWindow() };
        let Some(fenster) = fenster_monitor(hwnd) else {
            println!("Schirmerkennung: kein Monitor am Desktop-Fenster");
            return;
        };
        let ausgaenge = fabrik(true).map(|f| dxgi_ausgaenge(&f)).unwrap_or_default();
        let pfade = anzeige_pfade();
        println!("Schirmerkennung: {fenster:?}\nAusgaenge: {ausgaenge:#?}\nPfade: {pfade:#?}");
        match schirm_lesen(hwnd) {
            Some(z) => {
                println!("{}", z.text());
                assert!(z.name.eq_ignore_ascii_case(&fenster.name));
                assert!(ausgaenge.iter().any(|a| a.monitor == fenster.monitor && a.name.eq_ignore_ascii_case(&fenster.name) && a.adapter == z.adapter));
            }
            None => println!("{}", text_ohne_ausgang(&fenster)),
        }
    }
}
