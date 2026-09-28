// Wandler des Windows-Hosts: rechnet das Bild der Duplication auf der Karte
// um, bevor es in den gewohnten Bildweg geht.
//
// Modus SDR (HDR-Plan Abschnitt 4, "SDR aus HDR-Desktop"): mit "HDR
// verwenden" liefert die Duplication FP16 in scRGB - linear,
// BT.709-Primaerfarben, 1.0 = 80 nit, SDR-Inhalt steht dort mit dem
// SDR-Weiss des Schiebers in den Windows-Einstellungen (SDRWhiteLevel, 1000 =
// 80 nit). Der Wandler rechnet relativ zu diesem Weiss (rel = scRGB * 1000 /
// SDRWhiteLevel), schneidet farbtontreu auf 1 ab und kodiert stueckweise
// sRGB in 8 Bit. SDR-Inhalt des Desktops kommt so genau mit den Werten
// heraus, die er ohne HDR haette; Helleres (HDR-Video, Spiele) wird auf
// SDR-Weiss begrenzt, ohne den Farbton zu verschieben. Das Ergebnis ist
// BGRA8 in Groesse der Oberflaeche (ungedreht) und ersetzt die Textur der
// Duplication - Drehen, Halbieren und Encoder bleiben, wie sie sind.
//
// Modus PQ (HDR-Plan 5.2, HDR10 auf der Leitung): der Desktop geht, wie er
// ist, als BT.2020/PQ hinaus - absolute Pegel (scRGB * 80 nit), die
// Helligkeit gleicht erst der Client an (SDR-Weiss des Hosts steht in der
// Strominfo). Je Strompunkt: Drehen (wie Drehung::quelle) und, ab 3840
// Breite, das Mittel aus 2x2 Desktoppunkten in linearem Licht, dann BT.709 ->
// BT.2020, Negatives (ausserhalb BT.2020) und NaN auf 0, PQ (SMPTE ST 2084),
// Y'CbCr BT.2020-NCL im vollen Bereich, 10-Bit-Codes oben buendig (<< 6) in
// R16_UINT-Zielen in Stromgroesse. 4:4:4: drei Ziele (Y, Cb, Cr) in einem
// Durchgang (MRT) -> YUV444P16LE. 4:2:0: Y in voller Groesse, dann Cb/Cr als
// Mittel der vier Punkte (R16G16_UINT, halbe Groesse) -> P010. Die Ziele
// gehen gleich in STAGING-Kopien; pq_auslesen holt sie dicht gepackt in den
// Hauptspeicher. Ein BGRA-Bild (Vollbildprogramm mit 8 Bit auf dem
// HDR-Desktop) ist sRGB mit dem SDR-Weiss: linear * SDRWhiteLevel / 1000.
//
// Je Bild: CopyResource der Duplication-Textur in den eigenen Eingang mit
// Lesesicht (FP16, im Modus PQ auch BGRA; gleich nach AcquireNextFrame, vor
// ReleaseFrame), dann ein Dreieck ueber das Ziel. Der Eingang bleibt stehen:
// nach einem Farbwechsel (SDR <-> PQ) rechnet der Wandler das letzte Bild im
// neuen Modus nach, ohne auf die naechste Aenderung am Desktop zu warten.
// Die SDR-Shader stehen in hdr_hlsl.rs, die PQ-Shader hier (PQ_HLSL, hinter
// dem Text von hdr_hlsl::WANDLER); uebersetzt wird einmal je Prozess
// (anzeige::uebersetzen_aus).

use std::sync::OnceLock;

use windows::core::BOOL;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::aufnahme::Drehung;
use crate::hdr_hlsl::{self, KonstWandler};

/// SDRWhiteLevel, wenn Windows keinen nennt: 1000 = 80 nit = scRGB 1.0.
pub const SDR_WEISS_VORGABE: u32 = 1000;

/// Ist der SDRWhiteLevel aus DisplayConfig brauchbar? Windows nennt 1000
/// (80 nit, Schieber links) bis etwa 6000 (480 nit); ausserhalb von 20 bis
/// 1600 nit ist etwas faul - dann die Vorgabe.
pub fn sdr_weiss_pruefen(level: Option<u32>) -> u32 {
    match level {
        Some(l) if (250..=20_000).contains(&l) => l,
        _ => SDR_WEISS_VORGABE,
    }
}

/// SDRWhiteLevel in nit (1000 = 80 nit).
pub fn sdr_weiss_nit(level: u32) -> f32 {
    level as f32 * 80.0 / 1000.0
}

/// Der Faktor des Shaders: scRGB -> relativ zum SDR-Weiss.
pub fn weiss_kehrwert(level: u32) -> f32 {
    1000.0 / level.max(1) as f32
}

/// Was mit einem Bild der Duplication geschieht, nach seinem Format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eingang {
    /// BGRA 8 Bit (auch _SRGB): unveraendert in den Bildweg.
    Bgra,
    /// FP16 in scRGB (HDR-Desktop): durch den Wandler.
    Fp16,
    /// Alles andere: nicht verwendbar.
    Fremd,
}

pub fn eingang_art(f: DXGI_FORMAT) -> Eingang {
    if f == DXGI_FORMAT_B8G8R8A8_UNORM || f == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB {
        Eingang::Bgra
    } else if f == DXGI_FORMAT_R16G16B16A16_FLOAT {
        Eingang::Fp16
    } else {
        Eingang::Fremd
    }
}

/// Ein Format fuers Protokoll: Nummer und Name, soweit hier bekannt.
pub fn format_name(f: DXGI_FORMAT) -> String {
    let name = match f {
        DXGI_FORMAT_R16G16B16A16_FLOAT => "R16G16B16A16_FLOAT",
        DXGI_FORMAT_R10G10B10A2_UNORM => "R10G10B10A2_UNORM",
        DXGI_FORMAT_R8G8B8A8_UNORM => "R8G8B8A8_UNORM",
        DXGI_FORMAT_B8G8R8A8_UNORM => "B8G8R8A8_UNORM",
        DXGI_FORMAT_B8G8R8X8_UNORM => "B8G8R8X8_UNORM",
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => "B8G8R8A8_UNORM_SRGB",
        _ => return format!("{}", f.0),
    };
    format!("{} ({name})", f.0)
}

// ------------------------------------------------------------- Modus PQ

/// Die PQ-Shader: hinter hdr_hlsl::WANDLER (vs_voll, Eingang t0) und
/// hdr_hlsl::PQ (pq_oetf, srgb_eotf, M_709_NACH_2020 - dieselbe Quelle wie
/// die Anzeige des Clients) gesetzt, eigene Konstanten in b1. Die Zahlen
/// hier sind die Luma-Gewichte BT.2020 aus hdr.rs; der Test
/// pq_zahlen_wie_in_hdr_rs haelt sie fest. Einstiege: ps_pq444 (drei Ziele),
/// ps_pq420_y, ps_pq420_uv.
pub(crate) const PQ_HLSL: &str = r#"
// ---------- Wandler, Modus PQ: Desktop -> HDR10-Ebenen (BT.2020, PQ, voll, 10 Bit oben buendig) ----------
cbuffer WandlerPq : register(b1) {
    int2  quelle_groesse;   // Oberflaeche der Duplication (ungedreht) in Punkten
    int2  strom_groesse;    // Strom (gedreht, halbiert) in Punkten
    int   drehung;          // 0 keine, 1 = 90, 2 = 180, 3 = 270 Grad
    int   halb;             // 1: je Strompunkt das Mittel aus 2x2 Desktoppunkten (linear)
    int   eingang_srgb;     // 1: der Eingang ist BGRA 8 Bit (sRGB), nicht scRGB
    float weiss_scrgb;      // SDRWhiteLevel / 1000: sRGB-Weiss im scRGB (nur fuer BGRA)
};

static const float SCRGB_NIT = 80.0;
// Luma-Gewichte BT.2020-NCL und die Teiler fuer Cb und Cr
static const float KR = 0.2627;
static const float KB = 0.0593;
static const float CB_TEILER = 1.8814;
static const float CR_TEILER = 1.4746;

// Ein Desktoppunkt (so gedreht, wie der Desktop steht) -> lineares BT.2020 in
// scRGB-Einheiten (1.0 = 80 nit). Negatives (ausserhalb BT.2020) und NaN
// werden 0 (max liefert bei NaN das andere Glied).
float3 lin2020(int2 d) {
    int2 q = d;
    if (drehung == 1) q = int2(d.y, quelle_groesse.y - 1 - d.x);
    else if (drehung == 2) q = int2(quelle_groesse.x - 1 - d.x, quelle_groesse.y - 1 - d.y);
    else if (drehung == 3) q = int2(quelle_groesse.x - 1 - d.y, d.x);
    float3 c = eingang.Load(int3(q, 0)).rgb;
    if (eingang_srgb != 0) c = srgb_eotf(c) * weiss_scrgb;
    return max(mul(M_709_NACH_2020, c), 0.0);
}

// Ein Strompunkt -> PQ-R'G'B' 0..1.
float3 strompunkt(int2 p) {
    float3 l;
    if (halb != 0) {
        int2 d = p * 2;
        l = (lin2020(d) + lin2020(d + int2(1, 0)) + lin2020(d + int2(0, 1)) + lin2020(d + int2(1, 1))) * 0.25;
    } else {
        l = lin2020(p);
    }
    return pq_oetf(l * SCRGB_NIT);
}

// R'G'B' -> Y' 0..1, Cb/Cr -0.5..0.5 (BT.2020-NCL).
float3 ycbcr2020(float3 e) {
    float y = KR * e.r + (1.0 - KR - KB) * e.g + KB * e.b;
    return float3(y, (e.b - y) / CB_TEILER, (e.r - y) / CR_TEILER);
}

// 10-Bit-Codes im vollen Bereich (Y D = 1023 Y', C D = 1023 C + 512), gerundet,
// geklemmt, oben buendig in 16 Bit.
uint code_y(float y) { return ((uint)clamp(floor(y * 1023.0 + 0.5), 0.0, 1023.0)) << 6; }
uint code_c(float c) { return ((uint)clamp(floor(c * 1023.0 + 512.5), 0.0, 1023.0)) << 6; }

struct Ebenen444 {
    uint y  : SV_Target0;
    uint cb : SV_Target1;
    uint cr : SV_Target2;
};

Ebenen444 ps_pq444(float4 pos : SV_Position) {
    float3 c = ycbcr2020(strompunkt(int2(pos.xy)));
    Ebenen444 o;
    o.y = code_y(c.x);
    o.cb = code_c(c.y);
    o.cr = code_c(c.z);
    return o;
}

uint ps_pq420_y(float4 pos : SV_Position) : SV_Target {
    return code_y(ycbcr2020(strompunkt(int2(pos.xy))).x);
}

// Cb/Cr fuer vier Strompunkte: das Mittel der vier (nichtlinearen) Werte.
uint2 ps_pq420_uv(float4 pos : SV_Position) : SV_Target {
    int2 p = int2(pos.xy) * 2;
    float2 s = ycbcr2020(strompunkt(p)).yz + ycbcr2020(strompunkt(p + int2(1, 0))).yz
             + ycbcr2020(strompunkt(p + int2(0, 1))).yz + ycbcr2020(strompunkt(p + int2(1, 1))).yz;
    s *= 0.25;
    return uint2(code_c(s.x), code_c(s.y));
}
"#;

/// Konstanten des PQ-Modus (cbuffer WandlerPq, 32 Byte).
#[repr(C)]
pub(crate) struct KonstPq {
    pub quelle_groesse: [i32; 2],
    pub strom_groesse: [i32; 2],
    pub drehung: i32,
    pub halb: i32,
    pub eingang_srgb: i32,
    pub weiss_scrgb: f32,
}

/// Was der Modus PQ rechnet: Stromgroesse, Drehung und Halbieren, wie die
/// Aufnahme sie vorgibt, und 4:4:4 (YUV444P16LE) oder 4:2:0 (P010).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PqPlan {
    pub w: u32,
    pub h: u32,
    pub halb: bool,
    pub drehung: Drehung,
    pub chroma444: bool,
}

impl PqPlan {
    /// Die Ebenen: (Breite, Hoehe, Byte je Punkt) - 4:4:4 Y, Cb, Cr; 4:2:0
    /// Y und CbCr als Paare in halber Groesse.
    pub fn ebenen(&self) -> Vec<(u32, u32, usize)> {
        if self.chroma444 {
            vec![(self.w, self.h, 2); 3]
        } else {
            vec![(self.w, self.h, 2), (self.w / 2, self.h / 2, 4)]
        }
    }

    /// Bytes aller Ebenen dicht gepackt (so liefert pq_auslesen sie).
    pub fn bytes(&self) -> usize {
        self.ebenen().iter().map(|&(w, h, b)| w as usize * h as usize * b).sum()
    }

    pub fn text(&self) -> String {
        format!(
            "{}x{} {}{}{}",
            self.w,
            self.h,
            if self.chroma444 { "4:4:4" } else { "4:2:0" },
            if self.halb { ", halbiert" } else { "" },
            if self.drehung != Drehung::Keine { format!(", gedreht {} Grad", self.drehung.grad()) } else { String::new() }
        )
    }
}

fn drehung_code(d: Drehung) -> i32 {
    match d {
        Drehung::Keine => 0,
        Drehung::Grad90 => 1,
        Drehung::Grad180 => 2,
        Drehung::Grad270 => 3,
    }
}

/// Die ganze Quelle der PQ-Shader: der Text des Wandlers (vs_voll, Eingang,
/// gemeinsame Funktionen), die gemeinsamen PQ-Funktionen und dahinter PQ_HLSL.
fn pq_quelle() -> &'static str {
    static Q: OnceLock<String> = OnceLock::new();
    Q.get_or_init(|| format!("{}{}{}", hdr_hlsl::WANDLER, hdr_hlsl::PQ, PQ_HLSL))
}

/// Die uebersetzten Shader (Vertex, Pixel SDR) - einmal je Prozess; ein
/// Fehler bleibt stehen, er aendert sich zur Laufzeit nicht.
fn shader_code() -> Result<&'static (Vec<u8>, Vec<u8>), String> {
    static CODE: OnceLock<Result<(Vec<u8>, Vec<u8>), String>> = OnceLock::new();
    CODE.get_or_init(|| {
        let vs = crate::anzeige::uebersetzen_aus(hdr_hlsl::WANDLER, b"wandler.hlsl\0", b"vs_voll\0", b"vs_4_0\0")?;
        let ps = crate::anzeige::uebersetzen_aus(hdr_hlsl::WANDLER, b"wandler.hlsl\0", b"ps_sdr\0", b"ps_4_0\0")?;
        Ok((vs, ps))
    })
    .as_ref()
    .map_err(|e| e.clone())
}

/// Die uebersetzten PQ-Shader (444, 420 Y, 420 CbCr) - einmal je Prozess,
/// erst wenn der Modus PQ gebraucht wird. Ein Fehler trifft nur ihn.
fn pq_shader_code() -> Result<&'static [Vec<u8>; 3], String> {
    static CODE: OnceLock<Result<[Vec<u8>; 3], String>> = OnceLock::new();
    CODE.get_or_init(|| {
        let q = pq_quelle();
        let a = crate::anzeige::uebersetzen_aus(q, b"wandler_pq.hlsl\0", b"ps_pq444\0", b"ps_4_0\0")?;
        let b = crate::anzeige::uebersetzen_aus(q, b"wandler_pq.hlsl\0", b"ps_pq420_y\0", b"ps_4_0\0")?;
        let c = crate::anzeige::uebersetzen_aus(q, b"wandler_pq.hlsl\0", b"ps_pq420_uv\0", b"ps_4_0\0")?;
        Ok([a, b, c])
    })
    .as_ref()
    .map_err(|e| e.clone())
}

fn fehler(was: &str, e: windows::core::Error) -> String {
    format!("{was}: {} (0x{:08x})", e.message().trim(), e.code().0 as u32)
}

fn kein_zeiger() -> windows::core::Error {
    windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER)
}

/// Eine Textur des Wandlers mit ihrer Sicht (Lesesicht beim Eingang,
/// Zielsicht beim Ausgang).
struct Textur<S> {
    tex: ID3D11Texture2D,
    sicht: S,
    w: u32,
    h: u32,
}

fn textur(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }?;
    tex.ok_or_else(kein_zeiger)
}

fn staging(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }?;
    tex.ok_or_else(kein_zeiger)
}

/// Eine Eingangstextur mit Lesesicht in diesem Format.
fn eingang_textur(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT) -> windows::core::Result<Textur<ID3D11ShaderResourceView>> {
    let tex = textur(device, w, h, format, D3D11_BIND_SHADER_RESOURCE)?;
    let mut srv = None;
    unsafe { device.CreateShaderResourceView(&tex, None, Some(&mut srv)) }?;
    Ok(Textur { tex, sicht: srv.ok_or_else(kein_zeiger)?, w, h })
}

/// Eine Zieltextur mit Zielsicht in diesem Format.
fn ziel_textur(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT) -> windows::core::Result<Textur<ID3D11RenderTargetView>> {
    let tex = textur(device, w, h, format, D3D11_BIND_RENDER_TARGET)?;
    let mut rtv = None;
    unsafe { device.CreateRenderTargetView(&tex, None, Some(&mut rtv)) }?;
    Ok(Textur { tex, sicht: rtv.ok_or_else(kein_zeiger)?, w, h })
}

/// Eine Ebene des Modus PQ: Ziel (R16_UINT bzw. R16G16_UINT) und seine
/// STAGING-Kopie zum Auslesen.
struct Ebene {
    ziel: Textur<ID3D11RenderTargetView>,
    staging: ID3D11Texture2D,
    /// Byte je Punkt (2 fuer Y/Cb/Cr, 4 fuer das CbCr-Paar).
    punkt_bytes: usize,
}

/// Shader und Konstanten des Modus PQ - entstehen mit dem ersten PQ-Bild.
struct PqShader {
    ps444: ID3D11PixelShader,
    ps420_y: ID3D11PixelShader,
    ps420_uv: ID3D11PixelShader,
    /// cbuffer WandlerPq, 32 Byte, DYNAMIC.
    konst: ID3D11Buffer,
}

/// Die Ziele eines Plans; `bereit`: die STAGING-Kopien tragen ein Bild.
struct PqZiele {
    plan: PqPlan,
    ebenen: Vec<Ebene>,
    bereit: bool,
}

/// Der Wandler auf dem Geraet der Duplication.
pub struct Wandler {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps_sdr: ID3D11PixelShader,
    /// CULL_NONE (das eine Dreieck hat keine Vorderseite, auf die Verlass waere).
    raster: ID3D11RasterizerState,
    /// cbuffer Wandler, 16 Byte, DYNAMIC.
    konst: ID3D11Buffer,
    /// SDRWhiteLevel (1000 = 80 nit) und der Wert, der im Puffer steht.
    sdr_weiss: u32,
    konst_weiss: Option<u32>,
    /// FP16-Kopie der Duplication mit Lesesicht, Groesse der Oberflaeche.
    eingang: Option<Textur<ID3D11ShaderResourceView>>,
    /// BGRA-Kopie (Modus PQ: ein 8-Bit-Bild auf dem HDR-Desktop).
    eingang_bgra: Option<Textur<ID3D11ShaderResourceView>>,
    /// Welcher Eingang das zuletzt aufgenommene Bild haelt (None: keiner -
    /// oder ein neueres ging am Wandler vorbei).
    letzter: Option<Eingang>,
    /// Ausgang Modus SDR: B8G8R8A8_UNORM, Groesse der Oberflaeche.
    sdr: Option<Textur<ID3D11RenderTargetView>>,
    /// Modus PQ: Shader (einmal) und Ziele (je Plan).
    pq_shader: Option<PqShader>,
    pq: Option<PqZiele>,
}

impl Wandler {
    /// Shader, Zustand und Konstanten auf diesem Geraet; die Texturen
    /// entstehen mit dem ersten Bild (in dessen Groesse).
    pub fn neu(device: &ID3D11Device, ctx: &ID3D11DeviceContext, sdr_weiss: u32) -> Result<Wandler, String> {
        let (vs_code, ps_code) = shader_code()?;
        let mut vs = None;
        let mut ps_sdr = None;
        let mut raster = None;
        let konst;
        unsafe {
            device.CreateVertexShader(vs_code, None, Some(&mut vs)).map_err(|e| fehler("Wandler: CreateVertexShader", e))?;
            device.CreatePixelShader(ps_code, None, Some(&mut ps_sdr)).map_err(|e| fehler("Wandler: CreatePixelShader", e))?;
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
            device.CreateRasterizerState(&rd, Some(&mut raster)).map_err(|e| fehler("Wandler: CreateRasterizerState", e))?;
            konst = konstantenpuffer(device, std::mem::size_of::<KonstWandler>())?;
        }
        Ok(Wandler {
            device: device.clone(),
            ctx: ctx.clone(),
            vs: vs.ok_or("Wandler: CreateVertexShader lieferte nichts")?,
            ps_sdr: ps_sdr.ok_or("Wandler: CreatePixelShader lieferte nichts")?,
            raster: raster.ok_or("Wandler: CreateRasterizerState lieferte nichts")?,
            konst,
            sdr_weiss: sdr_weiss_pruefen(Some(sdr_weiss)),
            konst_weiss: None,
            eingang: None,
            eingang_bgra: None,
            letzter: None,
            sdr: None,
            pq_shader: None,
            pq: None,
        })
    }

    /// SDRWhiteLevel, mit dem gerechnet wird (1000 = 80 nit).
    pub fn sdr_weiss(&self) -> u32 {
        self.sdr_weiss
    }

    /// Neues SDR-Weiss (Schieber in den Windows-Einstellungen); gilt ab dem
    /// naechsten Bild.
    pub fn sdr_weiss_setzen(&mut self, level: u32) {
        self.sdr_weiss = sdr_weiss_pruefen(Some(level));
    }

    /// Haelt der Wandler ein Bild, aus dem er nachrechnen kann?
    #[cfg(test)]
    pub fn hat_eingang(&self) -> bool {
        self.letzter.is_some()
    }

    /// Ein neueres Bild ging am Wandler vorbei (BGRA im Modus SDR): was er
    /// haelt, ist nicht mehr das neueste - nachgerechnet wird daraus nichts.
    pub fn eingang_vergessen(&mut self) {
        self.letzter = None;
    }

    /// Das Bild der Duplication in den eigenen Eingang kopieren (FP16 oder
    /// BGRA, in seiner Groesse) - danach darf die Duplication es freigeben.
    /// Liefert, welcher Eingang es jetzt haelt.
    pub fn aufnehmen(&mut self, quelle: &ID3D11Texture2D) -> windows::core::Result<Eingang> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { quelle.GetDesc(&mut d) };
        let art = eingang_art(d.Format);
        let (fach, format) = match art {
            Eingang::Fp16 => (&mut self.eingang, DXGI_FORMAT_R16G16B16A16_FLOAT),
            Eingang::Bgra => (&mut self.eingang_bgra, DXGI_FORMAT_B8G8R8A8_UNORM),
            Eingang::Fremd => return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG)),
        };
        if fach.as_ref().map(|t| (t.w, t.h)) != Some((d.Width, d.Height)) {
            *fach = None;
            *fach = Some(eingang_textur(&self.device, d.Width, d.Height, format)?);
        }
        let e = fach.as_ref().ok_or_else(kein_zeiger)?;
        // BGRA und BGRA_SRGB gehoeren zur selben Formatgruppe: CopyResource
        // kopiert die Bytes, die Lesesicht (UNORM) liest sie roh.
        unsafe { self.ctx.CopyResource(&e.tex, quelle) };
        self.letzter = Some(art);
        Ok(art)
    }

    /// Den Zustand setzen, den ein Dreieck braucht - das Geraet teilt sich
    /// die Aufnahme mit dem Encoder, verlassen wird sich auf nichts.
    unsafe fn zustand(&self, ziele: &[Option<ID3D11RenderTargetView>], w: u32, h: u32, ps: &ID3D11PixelShader) {
        self.ctx.OMSetRenderTargets(Some(ziele), None);
        self.ctx.OMSetBlendState(None, None, 0xffff_ffff);
        self.ctx.OMSetDepthStencilState(None, 0);
        self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        self.ctx.RSSetState(&self.raster);
        self.ctx.IASetInputLayout(None);
        self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.ctx.VSSetShader(&self.vs, None);
        self.ctx.PSSetShader(ps, None);
    }

    /// Nichts gebunden lassen: die Ziele gehen gleich in CopyResource, der
    /// Eingang beim naechsten Bild auch.
    unsafe fn loesen(&self) {
        self.ctx.PSSetShaderResources(0, Some(&[None]));
        self.ctx.OMSetRenderTargets(None, None);
    }

    /// Modus SDR aus dem gehaltenen Bild: FP16 durch ps_sdr (BGRA8 in
    /// Oberflaechengroesse), ein gehaltenes BGRA-Bild ist schon SDR. None,
    /// wenn der Wandler kein Bild haelt. Die Textur gehoert dem Wandler und
    /// gilt bis zum naechsten Aufruf.
    pub fn sdr_rechnen(&mut self) -> windows::core::Result<Option<ID3D11Texture2D>> {
        match self.letzter {
            None | Some(Eingang::Fremd) => Ok(None),
            Some(Eingang::Bgra) => Ok(self.eingang_bgra.as_ref().map(|t| t.tex.clone())),
            Some(Eingang::Fp16) => {
                let Some((w, h)) = self.eingang.as_ref().map(|t| (t.w, t.h)) else { return Ok(None) };
                if self.sdr.as_ref().map(|t| (t.w, t.h)) != Some((w, h)) {
                    self.sdr = None;
                    self.sdr = Some(ziel_textur(&self.device, w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?);
                }
                let (Some(e), Some(z)) = (self.eingang.as_ref(), self.sdr.as_ref()) else { return Err(kein_zeiger()) };
                unsafe {
                    if self.konst_weiss != Some(self.sdr_weiss) {
                        let k = KonstWandler { weiss_kehrwert: weiss_kehrwert(self.sdr_weiss), _w: [0.0; 3] };
                        konstanten_schreiben(&self.ctx, &self.konst, &k)?;
                        self.konst_weiss = Some(self.sdr_weiss);
                    }
                    self.zustand(&[Some(z.sicht.clone())], z.w, z.h, &self.ps_sdr);
                    self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.konst.clone())]));
                    self.ctx.PSSetShaderResources(0, Some(&[Some(e.sicht.clone())]));
                    self.ctx.Draw(3, 0);
                    self.loesen();
                }
                Ok(Some(z.tex.clone()))
            }
        }
    }

    /// Modus SDR: das FP16-Bild der Duplication (scRGB) als BGRA8 in seiner
    /// Groesse. Kopiert zuerst (danach darf die Duplication das Bild
    /// freigeben), dann ein Dreieck ueber das Ziel. Die gelieferte Textur
    /// gehoert dem Wandler und gilt bis zum naechsten Aufruf.
    pub fn nach_sdr(&mut self, quelle: &ID3D11Texture2D) -> windows::core::Result<ID3D11Texture2D> {
        if self.aufnehmen(quelle)? != Eingang::Fp16 {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG));
        }
        self.sdr_rechnen()?.ok_or_else(kein_zeiger)
    }

    /// Shader und Konstantenpuffer des Modus PQ - beim ersten Mal.
    fn pq_shader_sichern(&mut self) -> Result<(), String> {
        if self.pq_shader.is_some() {
            return Ok(());
        }
        let [a, b, c] = pq_shader_code()?;
        let erzeugen = |code: &Vec<u8>| -> Result<ID3D11PixelShader, String> {
            let mut ps = None;
            unsafe { self.device.CreatePixelShader(code, None, Some(&mut ps)) }.map_err(|e| fehler("Wandler PQ: CreatePixelShader", e))?;
            ps.ok_or_else(|| "Wandler PQ: CreatePixelShader lieferte nichts".to_string())
        };
        let s = PqShader {
            ps444: erzeugen(a)?,
            ps420_y: erzeugen(b)?,
            ps420_uv: erzeugen(c)?,
            konst: unsafe { konstantenpuffer(&self.device, std::mem::size_of::<KonstPq>())? },
        };
        self.pq_shader = Some(s);
        Ok(())
    }

    /// Die Ziele fuer diesen Plan - neu nur, wenn er sich geaendert hat.
    fn pq_ziele_sichern(&mut self, plan: &PqPlan) -> Result<(), String> {
        if self.pq.as_ref().is_some_and(|z| z.plan == *plan) {
            return Ok(());
        }
        self.pq = None;
        if plan.w == 0 || plan.h == 0 || (!plan.chroma444 && (plan.w % 2 != 0 || plan.h % 2 != 0)) {
            return Err(format!("Wandler PQ: Stromgroesse {}x{} passt nicht", plan.w, plan.h));
        }
        let mut ebenen = Vec::new();
        for (w, h, b) in plan.ebenen() {
            let format = if b == 2 { DXGI_FORMAT_R16_UINT } else { DXGI_FORMAT_R16G16_UINT };
            let ziel = ziel_textur(&self.device, w, h, format).map_err(|e| fehler("Wandler PQ: Ziel", e))?;
            let st = staging(&self.device, w, h, format).map_err(|e| fehler("Wandler PQ: STAGING", e))?;
            ebenen.push(Ebene { ziel, staging: st, punkt_bytes: b });
        }
        self.pq = Some(PqZiele { plan: *plan, ebenen, bereit: false });
        Ok(())
    }

    /// Modus PQ aus dem gehaltenen Bild: die Ebenen nach dem Plan rechnen
    /// und gleich in die STAGING-Kopien geben (pq_auslesen holt sie ab).
    /// Ok(false), wenn der Wandler kein Bild haelt.
    pub fn pq_rechnen(&mut self, plan: &PqPlan) -> Result<bool, String> {
        let (eingang, srgb) = match self.letzter {
            Some(Eingang::Fp16) => (self.eingang.as_ref(), false),
            Some(Eingang::Bgra) => (self.eingang_bgra.as_ref(), true),
            _ => return Ok(false),
        };
        let Some((srv, sw, sh)) = eingang.map(|e| (e.sicht.clone(), e.w, e.h)) else { return Ok(false) };
        self.pq_shader_sichern()?;
        self.pq_ziele_sichern(plan)?;
        let (Some(s), Some(z)) = (self.pq_shader.as_ref(), self.pq.as_mut()) else { return Err("Wandler PQ: nicht eingerichtet".into()) };
        let k = KonstPq {
            quelle_groesse: [sw as i32, sh as i32],
            strom_groesse: [plan.w as i32, plan.h as i32],
            drehung: drehung_code(plan.drehung),
            halb: plan.halb as i32,
            eingang_srgb: srgb as i32,
            weiss_scrgb: self.sdr_weiss as f32 / 1000.0,
        };
        unsafe {
            konstanten_schreiben(&self.ctx, &s.konst, &k).map_err(|e| fehler("Wandler PQ: Konstanten", e))?;
            let ziele: Vec<Option<ID3D11RenderTargetView>> = z.ebenen.iter().map(|e| Some(e.ziel.sicht.clone())).collect();
            let durchgaenge: Vec<(&[Option<ID3D11RenderTargetView>], u32, u32, &ID3D11PixelShader)> = if plan.chroma444 {
                vec![(&ziele[..], plan.w, plan.h, &s.ps444)]
            } else {
                vec![(&ziele[..1], plan.w, plan.h, &s.ps420_y), (&ziele[1..], plan.w / 2, plan.h / 2, &s.ps420_uv)]
            };
            for (rtv, w, h, ps) in durchgaenge {
                // Wie zustand(), aber ohne &self - z haelt einen Teil davon.
                self.ctx.OMSetRenderTargets(Some(rtv), None);
                self.ctx.OMSetBlendState(None, None, 0xffff_ffff);
                self.ctx.OMSetDepthStencilState(None, 0);
                self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
                self.ctx.RSSetState(&self.raster);
                self.ctx.IASetInputLayout(None);
                self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                self.ctx.VSSetShader(&self.vs, None);
                self.ctx.PSSetShader(ps, None);
                self.ctx.PSSetConstantBuffers(1, Some(&[Some(s.konst.clone())]));
                self.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                self.ctx.Draw(3, 0);
            }
            self.ctx.PSSetShaderResources(0, Some(&[None]));
            self.ctx.OMSetRenderTargets(None, None);
            for e in &z.ebenen {
                self.ctx.CopyResource(&e.staging, &e.ziel.tex);
            }
        }
        z.bereit = true;
        Ok(true)
    }

    /// Die Ebenen des letzten pq_rechnen aus STAGING, dicht gepackt (u16
    /// LE, 10 Bit oben buendig): 4:4:4 Y, Cb, Cr je w x h; 4:2:0 Y (w x h),
    /// dann die CbCr-Paare (w/2 x h/2) - genau die Ebenen von YUV444P16LE
    /// bzw. P010 ohne Zeilenrest.
    pub fn pq_auslesen(&self, ziel: &mut Vec<u8>) -> Result<(), String> {
        let Some(z) = self.pq.as_ref().filter(|z| z.bereit) else { return Err("Wandler PQ: noch kein Bild gerechnet".into()) };
        ziel.resize(z.plan.bytes(), 0);
        let mut o = 0usize;
        for e in &z.ebenen {
            let zeile = e.ziel.w as usize * e.punkt_bytes;
            let hoehe = e.ziel.h as usize;
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe { self.ctx.Map(&e.staging, 0, D3D11_MAP_READ, 0, Some(&mut m)) }.map_err(|x| fehler("Wandler PQ: Map", x))?;
            let r = if m.pData.is_null() || (m.RowPitch as usize) < zeile {
                Err(format!("Wandler PQ: Map lieferte {} Byte je Zeile fuer {zeile}", m.RowPitch))
            } else {
                // Sicher gemappt: Hoehe-1 Zeilen zu RowPitch, dann die letzte Zeile.
                let laenge = m.RowPitch as usize * (hoehe - 1) + zeile;
                let quelle = unsafe { std::slice::from_raw_parts(m.pData as *const u8, laenge) };
                for (y, z) in ziel[o..o + zeile * hoehe].chunks_exact_mut(zeile).enumerate() {
                    z.copy_from_slice(&quelle[y * m.RowPitch as usize..y * m.RowPitch as usize + zeile]);
                }
                Ok(())
            };
            unsafe { self.ctx.Unmap(&e.staging, 0) };
            r?;
            o += zeile * hoehe;
        }
        Ok(())
    }
}

/// Ein Konstantenpuffer (DYNAMIC, CPU schreibt) dieser Groesse.
unsafe fn konstantenpuffer(device: &ID3D11Device, bytes: usize) -> Result<ID3D11Buffer, String> {
    let bd = D3D11_BUFFER_DESC {
        ByteWidth: bytes as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut b = None;
    device.CreateBuffer(&bd, None, Some(&mut b)).map_err(|e| fehler("Wandler: CreateBuffer", e))?;
    b.ok_or_else(|| "Wandler: CreateBuffer lieferte nichts".to_string())
}

/// Konstanten (repr(C)) in einen DYNAMIC-Puffer schreiben.
unsafe fn konstanten_schreiben<T>(ctx: &ID3D11DeviceContext, puffer: &ID3D11Buffer, k: &T) -> windows::core::Result<()> {
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.Map(puffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
    if m.pData.is_null() {
        ctx.Unmap(puffer, 0);
        return Err(kein_zeiger());
    }
    std::ptr::copy_nonoverlapping(k as *const T as *const u8, m.pData as *mut u8, std::mem::size_of::<T>());
    ctx.Unmap(puffer, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdr;
    use crate::hdr_hlsl::spiegel;

    #[test]
    fn eingang_nach_format() {
        assert_eq!(eingang_art(DXGI_FORMAT_B8G8R8A8_UNORM), Eingang::Bgra);
        assert_eq!(eingang_art(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB), Eingang::Bgra);
        assert_eq!(eingang_art(DXGI_FORMAT_R16G16B16A16_FLOAT), Eingang::Fp16);
        // BGRX kopiert CopyResource nicht in BGRA (andere Formatgruppe).
        assert_eq!(eingang_art(DXGI_FORMAT_B8G8R8X8_UNORM), Eingang::Fremd);
        assert_eq!(eingang_art(DXGI_FORMAT_R10G10B10A2_UNORM), Eingang::Fremd);
        assert_eq!(format_name(DXGI_FORMAT_R16G16B16A16_FLOAT), "10 (R16G16B16A16_FLOAT)");
        assert_eq!(format_name(DXGI_FORMAT(999)), "999");
    }

    #[test]
    fn sdr_weiss_grenzen_und_umrechnung() {
        assert_eq!(sdr_weiss_pruefen(None), 1000);
        assert_eq!(sdr_weiss_pruefen(Some(0)), 1000);
        assert_eq!(sdr_weiss_pruefen(Some(100)), 1000);
        assert_eq!(sdr_weiss_pruefen(Some(99_999)), 1000);
        assert_eq!(sdr_weiss_pruefen(Some(3000)), 3000);
        assert_eq!(sdr_weiss_nit(1000), 80.0);
        assert_eq!(sdr_weiss_nit(3000), 240.0);
        assert_eq!(weiss_kehrwert(1000), 1.0);
        assert_eq!(weiss_kehrwert(2500), 0.4);
        assert_eq!(weiss_kehrwert(0), 1000.0);
    }

    /// Die Konstanten des Modus PQ liegen so, wie der Shader sie liest: 32
    /// Byte, die Glieder in derselben Reihenfolge wie im cbuffer, und die
    /// int2-Paare teilen sich das erste 16-Byte-Register.
    #[test]
    fn pq_konstanten_wie_im_shader() {
        assert_eq!(std::mem::size_of::<KonstPq>(), 32);
        assert_eq!(std::mem::offset_of!(KonstPq, quelle_groesse), 0);
        assert_eq!(std::mem::offset_of!(KonstPq, strom_groesse), 8);
        assert_eq!(std::mem::offset_of!(KonstPq, drehung), 16);
        assert_eq!(std::mem::offset_of!(KonstPq, halb), 20);
        assert_eq!(std::mem::offset_of!(KonstPq, eingang_srgb), 24);
        assert_eq!(std::mem::offset_of!(KonstPq, weiss_scrgb), 28);
        let cb = &PQ_HLSL[PQ_HLSL.find("cbuffer WandlerPq : register(b1)").expect("cbuffer WandlerPq in b1")..];
        let lage: Vec<usize> = ["int2  quelle_groesse;", "int2  strom_groesse;", "int   drehung;", "int   halb;", "int   eingang_srgb;", "float weiss_scrgb;"]
            .iter()
            .map(|n| cb.find(n).unwrap_or_else(|| panic!("{n} fehlt")))
            .collect();
        assert!(lage.windows(2).all(|p| p[0] < p[1]), "Reihenfolge im cbuffer: {lage:?}");
        // Der Wandler-Text davor belegt b0 und t0 - PQ nimmt b1 und denselben Eingang.
        assert!(hdr_hlsl::WANDLER.contains("register(b0)") && !hdr_hlsl::WANDLER.contains("register(b1)"));
        assert!(!PQ_HLSL.contains("register(t"), "PQ liest den Eingang des Wandlers (t0)");
        for e in ["Ebenen444 ps_pq444(", "uint ps_pq420_y(", "uint2 ps_pq420_uv(", "float3 pq_oetf(", "float3 srgb_eotf(", "float3 lin2020("] {
            assert_eq!(pq_quelle().matches(e).count(), 1, "{e}");
        }
    }

    /// Die Zahlen im PQ-HLSL des Wandlers sind die aus hdr.rs: Luma-Gewichte
    /// und Teiler BT.2020 (PQ und BT.709 -> BT.2020 kommen aus hdr_hlsl::PQ,
    /// dort geprueft). Die gemeinsamen Funktionen stehen nicht doppelt da.
    #[test]
    fn pq_zahlen_wie_in_hdr_rs() {
        assert!(pq_quelle().contains(hdr_hlsl::PQ));
        for doppelt in ["PQ_M1 =", "float3 pq_oetf(", "float3 srgb_eotf(", "float3x3 M_709_NACH_2020"] {
            assert!(!PQ_HLSL.contains(doppelt), "{doppelt} gehoert nach hdr_hlsl.rs");
        }
        assert!(PQ_HLSL.contains("mul(M_709_NACH_2020, c)"));
        assert!(PQ_HLSL.contains(&format!("KR = {};", hdr::KR_2020)));
        assert!(PQ_HLSL.contains(&format!("KB = {};", hdr::KB_2020)));
        assert!(PQ_HLSL.contains(&format!("CB_TEILER = {};", hdr::CB_B_2020)));
        assert!(PQ_HLSL.contains(&format!("CR_TEILER = {};", hdr::CR_R_2020)));
        assert!(PQ_HLSL.contains("SCRGB_NIT = 80.0;"));
    }

    #[test]
    fn plan_ebenen_und_bytes() {
        let p = PqPlan { w: 64, h: 40, halb: false, drehung: Drehung::Keine, chroma444: true };
        assert_eq!(p.ebenen(), vec![(64, 40, 2); 3]);
        assert_eq!(p.bytes(), 64 * 40 * 2 * 3);
        let p = PqPlan { chroma444: false, ..p };
        assert_eq!(p.ebenen(), vec![(64, 40, 2), (32, 20, 4)]);
        assert_eq!(p.bytes(), 64 * 40 * 2 * 3 / 2);
        assert_eq!(PqPlan { halb: true, drehung: Drehung::Grad90, ..p }.text(), "64x40 4:2:0, halbiert, gedreht 90 Grad");
    }

    /// Ein Geraet fuer die Kartentests: WARP (die VM hat keine Karte). Ohne
    /// D3D11 werden sie uebersprungen.
    fn warp() -> Option<(ID3D11Device, ID3D11DeviceContext)> {
        match crate::anzeige::geraet_bauen(None, true, false) {
            Ok((d, c, _)) => Some((d, c)),
            Err(e) => {
                eprintln!("uebersprungen: {e}");
                None
            }
        }
    }

    /// Eine FP16-Textur (DEFAULT) mit diesen scRGB-Werten, zeilenweise w x h.
    fn fp16_textur(device: &ID3D11Device, w: u32, h: u32, punkte: &[[f32; 3]]) -> ID3D11Texture2D {
        assert_eq!(punkte.len(), (w * h) as usize);
        let daten: Vec<u16> = punkte.iter().flat_map(|p| [spiegel::halb_bits(p[0]), spiegel::halb_bits(p[1]), spiegel::halb_bits(p[2]), 0x3c00]).collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: daten.as_ptr() as *const _, SysMemPitch: w * 8, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }.expect("FP16-Textur");
        tex.unwrap()
    }

    /// Eine BGRA8-Textur (DEFAULT) mit diesen Werten [R, G, B], zeilenweise.
    fn bgra_textur(device: &ID3D11Device, w: u32, h: u32, punkte: &[[u8; 3]]) -> ID3D11Texture2D {
        assert_eq!(punkte.len(), (w * h) as usize);
        let daten: Vec<u8> = punkte.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: daten.as_ptr() as *const _, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }.expect("BGRA-Textur");
        tex.unwrap()
    }

    /// Eine BGRA8-Textur ueber STAGING lesen: je Punkt [R, G, B, A].
    fn bgra_lesen(device: &ID3D11Device, ctx: &ID3D11DeviceContext, tex: &ID3D11Texture2D) -> (u32, u32, Vec<[u8; 4]>) {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut d) };
        assert_eq!(d.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
        let sd = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..d };
        let mut st = None;
        unsafe { device.CreateTexture2D(&sd, None, Some(&mut st)) }.expect("STAGING");
        let st = st.unwrap();
        let mut aus = Vec::new();
        unsafe {
            ctx.CopyResource(&st, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).expect("Map");
            for y in 0..d.Height as usize {
                let z = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), d.Width as usize * 4);
                aus.extend(z.chunks_exact(4).map(|p| [p[2], p[1], p[0], p[3]]));
            }
            ctx.Unmap(&st, 0);
        }
        (d.Width, d.Height, aus)
    }

    /// Modus SDR auf WARP, bei vier SDR-Weiss-Stufen: SDR-Inhalt (Grau- und
    /// Farbrampe, so in den scRGB-Desktop gelegt wie Windows es tut) kommt
    /// exakt heraus; Flecken ueber SDR-Weiss (12.5 = 1000 nit), negative
    /// Werte und NaN wie der Rust-Spiegel des Shaders (+-1 zaehlt mit, muss
    /// aber 0 bleiben, wo der Spiegel exakt ist). Alpha immer 255. Eine
    /// zweite Groesse legt die Texturen neu an.
    #[test]
    fn sdr_aus_hdr_desktop_auf_warp() {
        let Some((device, ctx)) = warp() else { return };
        let mut wandler = Wandler::neu(&device, &ctx, 1000).expect("Wandler");
        let (w, h) = (256u32, 4u32);
        for level in [1000u32, 2500, 3000, 5000] {
            wandler.sdr_weiss_setzen(level);
            assert_eq!(wandler.sdr_weiss(), level);
            let weiss = level as f32 / 1000.0;
            let lin = |k: u32| spiegel::srgb_eotf(k as f32 / 255.0) * weiss;
            let mut punkte = Vec::new();
            let mut soll_exakt = Vec::new();
            for v in 0..w {
                // Zeile 0: Grau, Zeile 1: Farbe - SDR-Inhalt, muss exakt sein.
                punkte.push([lin(v); 3]);
                soll_exakt.push([v as u8; 3]);
            }
            for v in 0..w {
                let rgb = [v, 255 - v, (v * 7) % 256];
                punkte.push(rgb.map(lin));
                soll_exakt.push(rgb.map(|k| k as u8));
            }
            for v in 0..w {
                // Zeile 2: ueber SDR-Weiss, Grau und farbig (bis 12.5 = 1000 nit).
                let f = v as f32 / 255.0;
                punkte.push(if v % 2 == 0 { [12.5 * f; 3] } else { [12.5 * f, 2.5 * f, 0.5] });
            }
            for v in 0..w {
                // Zeile 3: negative Anteile (ausserhalb BT.709) und NaN.
                let f = v as f32 / 255.0;
                punkte.push([-f, f * weiss, if v % 17 == 0 { f32::NAN } else { 0.25 * weiss }]);
            }
            let tex = fp16_textur(&device, w, h, &punkte);
            let aus = wandler.nach_sdr(&tex).expect("nach_sdr");
            let (aw, ah, bild) = bgra_lesen(&device, &ctx, &aus);
            assert_eq!((aw, ah), (w, h));
            let mut exakt_falsch = Vec::new();
            let mut spiegel_falsch = Vec::new();
            for (i, p) in bild.iter().enumerate() {
                assert_eq!(p[3], 255, "Alpha bei {i}");
                let rgb = [p[0], p[1], p[2]];
                let karte = punkte[i].map(spiegel::halb);
                let sp = spiegel::sdr_aus_scrgb(karte, weiss_kehrwert(level));
                if i < soll_exakt.len() && rgb != soll_exakt[i] {
                    exakt_falsch.push((i, rgb, soll_exakt[i]));
                }
                if (0..3).any(|c| (rgb[c] as i32 - sp[c] as i32).abs() > 1) {
                    spiegel_falsch.push((i, rgb, sp));
                }
            }
            assert!(exakt_falsch.is_empty(), "Weiss {level}: {} SDR-Werte falsch, erste {:?}", exakt_falsch.len(), &exakt_falsch[..exakt_falsch.len().min(5)]);
            assert!(spiegel_falsch.is_empty(), "Weiss {level}: {} Punkte weiter als 1 vom Spiegel, erste {:?}", spiegel_falsch.len(), &spiegel_falsch[..spiegel_falsch.len().min(5)]);
        }
        // Andere Groesse (ungerade): neue Texturen, Ergebnis in dieser Groesse.
        let punkte = vec![[1.0f32, 0.5, 0.0]; 7 * 3];
        let tex = fp16_textur(&device, 7, 3, &punkte);
        wandler.sdr_weiss_setzen(1000);
        let aus = wandler.nach_sdr(&tex).expect("nach_sdr 7x3");
        let (aw, ah, bild) = bgra_lesen(&device, &ctx, &aus);
        assert_eq!((aw, ah), (7, 3));
        let sp = spiegel::sdr_aus_scrgb([1.0, 0.5, 0.0], 1.0);
        assert!(bild.iter().all(|p| [p[0], p[1], p[2]] == sp), "{:?} statt {sp:?}", bild[0]);
        // Aus dem gehaltenen Bild nachgerechnet: dasselbe.
        let nochmal = wandler.sdr_rechnen().expect("sdr_rechnen").expect("gehaltenes Bild");
        let (_, _, bild2) = bgra_lesen(&device, &ctx, &nochmal);
        assert_eq!(bild2, bild);
        wandler.eingang_vergessen();
        assert!(!wandler.hat_eingang());
        assert!(wandler.sdr_rechnen().expect("sdr_rechnen").is_none());
    }

    // ------------------------------------------------ Modus PQ gegen hdr.rs

    /// Die Referenz des Modus PQ in hdr.rs (f64-PQ): je Strompunkt die
    /// Oberflaeche nach Drehung::quelle, BT.709 -> BT.2020 (hdr::mal),
    /// Negatives und NaN auf 0, bei halb das Mittel aus 2x2, * 80 nit,
    /// hdr::pq_oetf, hdr::rgb_nach_ycbcr_2020 - Y', Cb, Cr als f64.
    fn referenz_punkt(ober: &[[f32; 3]], sw: usize, sh: usize, plan: &PqPlan, x: usize, y: usize) -> [f64; 3] {
        let lin = |dx: usize, dy: usize| -> [f32; 3] {
            let (qx, qy) = plan.drehung.quelle(sw, sh, dx, dy);
            hdr::mal(&hdr::M_709_NACH_2020, ober[qy * sw + qx]).map(|v| v.max(0.0))
        };
        let l = if plan.halb {
            let (a, b, c, d) = (lin(2 * x, 2 * y), lin(2 * x + 1, 2 * y), lin(2 * x, 2 * y + 1), lin(2 * x + 1, 2 * y + 1));
            [0, 1, 2].map(|k| (a[k] + b[k] + c[k] + d[k]) * 0.25)
        } else {
            lin(x, y)
        };
        let e = l.map(|v| hdr::pq_oetf(v as f64 * 80.0) as f32);
        hdr::rgb_nach_ycbcr_2020(e).map(|v| v as f64)
    }

    fn code_y(y: f64) -> i32 {
        (y * 1023.0).round().clamp(0.0, 1023.0) as i32
    }

    fn code_c(c: f64) -> i32 {
        (c * 1023.0 + 512.0).round().clamp(0.0, 1023.0) as i32
    }

    /// Die Referenz-Ebenen (10-Bit-Codes, nicht verschoben) fuer einen Plan.
    fn referenz_ebenen(ober: &[[f32; 3]], sw: usize, sh: usize, plan: &PqPlan) -> Vec<Vec<i32>> {
        let (w, h) = (plan.w as usize, plan.h as usize);
        let punkte: Vec<[f64; 3]> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| referenz_punkt(ober, sw, sh, plan, x, y)).collect();
        let y: Vec<i32> = punkte.iter().map(|p| code_y(p[0])).collect();
        if plan.chroma444 {
            vec![y, punkte.iter().map(|p| code_c(p[1])).collect(), punkte.iter().map(|p| code_c(p[2])).collect()]
        } else {
            let mut uv = Vec::new();
            for by in 0..h / 2 {
                for bx in 0..w / 2 {
                    let i = |dx: usize, dy: usize| (2 * by + dy) * w + 2 * bx + dx;
                    let (a, b, c, d) = (punkte[i(0, 0)], punkte[i(1, 0)], punkte[i(0, 1)], punkte[i(1, 1)]);
                    uv.push(code_c((a[1] + b[1] + c[1] + d[1]) * 0.25));
                    uv.push(code_c((a[2] + b[2] + c[2] + d[2]) * 0.25));
                }
            }
            vec![y, uv]
        }
    }

    /// Die dicht gepackten Ebenen aus pq_auslesen als u16.
    fn als_u16(b: &[u8]) -> Vec<u16> {
        b.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect()
    }

    /// Vergleicht die Ebenen der Karte mit der Referenz: jeder Code oben
    /// buendig (untere 6 Bit 0) und hoechstens 1 daneben; liefert die Zahl
    /// der abweichenden Codes.
    fn vergleichen(karte: &[u8], soll: &[Vec<i32>], was: &str) -> usize {
        let k = als_u16(karte);
        let alle: Vec<i32> = soll.iter().flatten().copied().collect();
        assert_eq!(k.len(), alle.len(), "{was}: Laenge");
        let mut abweichend = 0;
        for (i, (&a, &s)) in k.iter().zip(&alle).enumerate() {
            assert_eq!(a & 0x3f, 0, "{was}: Code {i} nicht oben buendig ({a:#06x})");
            let d = (a >> 6) as i32 - s;
            assert!(d.abs() <= 1, "{was}: Code {i} ist {} statt {s}", a >> 6);
            abweichend += (d != 0) as usize;
        }
        abweichend
    }

    /// Die Oberflaeche fuer die PQ-Tests: SDR-Grau und -Farbe (x 2,5 wie
    /// Windows sie in den Desktop legt), Flecken um 12,5 (1000 nit) bis 200
    /// (ueber 10000 nit), negative Anteile, NaN - so wie die Karte sie
    /// haelt (FP16 gerundet).
    fn pq_oberflaeche(sw: usize, sh: usize) -> Vec<[f32; 3]> {
        (0..sw * sh)
            .map(|i| {
                let v = (i * 37 % 256) as f32 / 255.0;
                let lin = spiegel::srgb_eotf(v) * 2.5;
                match i % 7 {
                    0 | 1 => [lin; 3],
                    2 => [lin, spiegel::srgb_eotf(1.0 - v) * 2.5, 0.3],
                    3 => [12.5 * v, 12.5, 2.5 * v],
                    4 => [125.0 * v, 200.0 * v, 12.5],
                    5 => [-0.2 * v, lin, 0.5],
                    _ => [if i % 3 == 0 { f32::NAN } else { lin }, 1.0, 0.0],
                }
                .map(spiegel::halb)
            })
            .collect()
    }

    /// Modus PQ auf WARP gegen hdr.rs: 4:4:4 und 4:2:0, alle vier
    /// Drehungen, mit und ohne Halbieren - jeder Code hoechstens 1 von der
    /// f64-Referenz, fast alle genau. Dazu: ein zweiter Plan rechnet aus dem
    /// gehaltenen Bild nach (Farbwechsel ohne neues Bild der Duplication).
    #[test]
    fn pq_ebenen_auf_warp_wie_hdr_rs() {
        let Some((device, ctx)) = warp() else { return };
        let mut wandler = Wandler::neu(&device, &ctx, 2500).expect("Wandler");
        let (dw, dh) = (36usize, 20usize);
        let mut gesamt = 0usize;
        let mut abweichend = 0usize;
        for d in [Drehung::Keine, Drehung::Grad90, Drehung::Grad180, Drehung::Grad270] {
            let (sw, sh) = d.groesse(dw, dh);
            let ober = pq_oberflaeche(sw, sh);
            let tex = fp16_textur(&device, sw as u32, sh as u32, &ober);
            assert_eq!(wandler.aufnehmen(&tex).expect("aufnehmen"), Eingang::Fp16);
            for halb in [false, true] {
                for chroma444 in [true, false] {
                    let (w, h) = if halb { (dw / 2, dh / 2) } else { (dw, dh) };
                    let plan = PqPlan { w: w as u32, h: h as u32, halb, drehung: d, chroma444 };
                    assert!(wandler.pq_rechnen(&plan).expect("pq_rechnen"), "{}", plan.text());
                    let mut aus = Vec::new();
                    wandler.pq_auslesen(&mut aus).expect("pq_auslesen");
                    assert_eq!(aus.len(), plan.bytes());
                    let soll = referenz_ebenen(&ober, sw, sh, &plan);
                    gesamt += soll.iter().map(Vec::len).sum::<usize>();
                    abweichend += vergleichen(&aus, &soll, &plan.text());
                }
            }
        }
        // Rundungsgrenzen: f32 auf der Karte gegen f64 - selten.
        eprintln!("PQ auf WARP: {abweichend} von {gesamt} Codes um 1 neben hdr.rs");
        assert!(abweichend * 100 <= gesamt, "{abweichend} von {gesamt} Codes um 1 daneben");
    }

    /// SDR-Inhalt im PQ-Strom: Grau v bei SDR-Weiss 2,5 (200 nit) ist Y' =
    /// pq_code10(sRGB(v) * 200 nit), Cb = Cr = 512 - so bildet der Client ihn
    /// exakt auf SDR zurueck (hdr.rs, zeile_rgb_pq_wie_die_referenz). Und ein
    /// BGRA-Bild (8 Bit auf dem HDR-Desktop) ergibt dasselbe wie sein
    /// scRGB-Gegenstueck: sRGB * SDR-Weiss.
    #[test]
    fn pq_sdr_grau_und_bgra_eingang() {
        let Some((device, ctx)) = warp() else { return };
        let mut wandler = Wandler::neu(&device, &ctx, 2500).expect("Wandler");
        let (w, h) = (256u32, 2u32);
        let plan = PqPlan { w, h, halb: false, drehung: Drehung::Keine, chroma444: true };
        let grau: Vec<[f32; 3]> = (0..w * h).map(|i| [spiegel::srgb_eotf((i % 256) as f32 / 255.0) * 2.5; 3]).collect();
        let tex = fp16_textur(&device, w, h, &grau);
        wandler.aufnehmen(&tex).unwrap();
        assert!(wandler.pq_rechnen(&plan).unwrap());
        let mut fp16 = Vec::new();
        wandler.pq_auslesen(&mut fp16).unwrap();
        let k = als_u16(&fp16);
        let n = (w * h) as usize;
        let mut daneben = 0;
        for i in 0..n {
            let soll = hdr::pq_code10(hdr::srgb_eotf((i % 256) as f64 / 255.0) * 200.0) as i32;
            let d = (k[i] >> 6) as i32 - soll;
            assert!(d.abs() <= 1, "Grau {}: Y {} statt {soll}", i % 256, k[i] >> 6);
            daneben += (d != 0) as usize;
            assert_eq!(k[n + i] >> 6, 512, "Cb bei Grau {}", i % 256);
            assert_eq!(k[2 * n + i] >> 6, 512, "Cr bei Grau {}", i % 256);
        }
        assert!(daneben <= 4, "{daneben} Grauwerte um 1 daneben");

        // BGRA: dieselben Werte als 8 Bit, dazu Farben.
        let bgra: Vec<[u8; 3]> = (0..w * h).map(|i| if i < w { [i as u8; 3] } else { [i as u8, 255 - i as u8, (i * 7) as u8] }).collect();
        let tex = bgra_textur(&device, w, h, &bgra);
        assert_eq!(wandler.aufnehmen(&tex).unwrap(), Eingang::Bgra);
        assert!(wandler.pq_rechnen(&plan).unwrap());
        let mut aus = Vec::new();
        wandler.pq_auslesen(&mut aus).unwrap();
        let ober: Vec<[f32; 3]> = bgra.iter().map(|p| p.map(|v| hdr::srgb_eotf(v as f64 / 255.0) as f32 * 2.5)).collect();
        let soll = referenz_ebenen(&ober, w as usize, h as usize, &plan);
        let abweichend = vergleichen(&aus, &soll, "BGRA-Eingang");
        assert!(abweichend * 50 <= soll.iter().map(Vec::len).sum::<usize>(), "{abweichend} Codes um 1 daneben");
        // Die Grauzeile aus BGRA ist die aus FP16 (bis auf die FP16-Rundung).
        let b = als_u16(&aus);
        assert!((0..w as usize).all(|i| ((b[i] >> 6) as i32 - (k[i] >> 6) as i32).abs() <= 1));

        // Ohne gehaltenes Bild: nichts zu rechnen, und Auslesen vor dem
        // ersten Rechnen ist ein Fehler.
        let mut leer = Wandler::neu(&device, &ctx, 1000).unwrap();
        assert!(!leer.pq_rechnen(&plan).unwrap());
        assert!(leer.pq_auslesen(&mut aus).is_err());
        // Eine ungerade Stromgroesse gibt es fuer 4:2:0 nicht.
        leer.aufnehmen(&tex).unwrap();
        assert!(leer.pq_rechnen(&PqPlan { w: 7, h: 2, chroma444: false, ..plan }).is_err());
    }
}
