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
// Je Bild: CopyResource der Duplication-Textur in die eigene FP16-Kopie mit
// Lesesicht (gleich nach AcquireNextFrame, vor ReleaseFrame), dann ein
// Dreieck ueber das Ziel mit ps_sdr. Die Shader stehen in hdr_hlsl.rs und
// werden einmal je Prozess uebersetzt (anzeige::uebersetzen_aus).

use std::sync::OnceLock;

use windows::core::BOOL;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

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

fn fehler(was: &str, e: windows::core::Error) -> String {
    format!("{was}: {} (0x{:08x})", e.message().trim(), e.code().0 as u32)
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
    tex.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER))
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
    /// Ausgang Modus SDR: B8G8R8A8_UNORM, Groesse der Oberflaeche.
    sdr: Option<Textur<ID3D11RenderTargetView>>,
}

impl Wandler {
    /// Shader, Zustand und Konstanten auf diesem Geraet; die Texturen
    /// entstehen mit dem ersten Bild (in dessen Groesse).
    pub fn neu(device: &ID3D11Device, ctx: &ID3D11DeviceContext, sdr_weiss: u32) -> Result<Wandler, String> {
        let (vs_code, ps_code) = shader_code()?;
        let mut vs = None;
        let mut ps_sdr = None;
        let mut raster = None;
        let mut konst = None;
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
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<KonstWandler>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            device.CreateBuffer(&bd, None, Some(&mut konst)).map_err(|e| fehler("Wandler: CreateBuffer", e))?;
        }
        Ok(Wandler {
            device: device.clone(),
            ctx: ctx.clone(),
            vs: vs.ok_or("Wandler: CreateVertexShader lieferte nichts")?,
            ps_sdr: ps_sdr.ok_or("Wandler: CreatePixelShader lieferte nichts")?,
            raster: raster.ok_or("Wandler: CreateRasterizerState lieferte nichts")?,
            konst: konst.ok_or("Wandler: CreateBuffer lieferte nichts")?,
            sdr_weiss: sdr_weiss_pruefen(Some(sdr_weiss)),
            konst_weiss: None,
            eingang: None,
            sdr: None,
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

    /// Eingang und Ziel in der Groesse dieses Bildes - neu nur, wenn sie
    /// nicht stimmt.
    fn texturen_sichern(&mut self, w: u32, h: u32) -> windows::core::Result<()> {
        if self.eingang.as_ref().map(|t| (t.w, t.h)) == Some((w, h)) && self.sdr.as_ref().map(|t| (t.w, t.h)) == Some((w, h)) {
            return Ok(());
        }
        self.eingang = None;
        self.sdr = None;
        let tex = textur(&self.device, w, h, DXGI_FORMAT_R16G16B16A16_FLOAT, D3D11_BIND_SHADER_RESOURCE)?;
        let mut srv = None;
        unsafe { self.device.CreateShaderResourceView(&tex, None, Some(&mut srv)) }?;
        let srv = srv.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER))?;
        let ziel = textur(&self.device, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_RENDER_TARGET)?;
        let mut rtv = None;
        unsafe { self.device.CreateRenderTargetView(&ziel, None, Some(&mut rtv)) }?;
        let rtv = rtv.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER))?;
        self.eingang = Some(Textur { tex, sicht: srv, w, h });
        self.sdr = Some(Textur { tex: ziel, sicht: rtv, w, h });
        Ok(())
    }

    /// Modus SDR: das FP16-Bild der Duplication (scRGB) als BGRA8 in seiner
    /// Groesse. Kopiert zuerst (danach darf die Duplication das Bild
    /// freigeben), dann ein Dreieck ueber das Ziel. Die gelieferte Textur
    /// gehoert dem Wandler und gilt bis zum naechsten Aufruf.
    pub fn nach_sdr(&mut self, quelle: &ID3D11Texture2D) -> windows::core::Result<ID3D11Texture2D> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { quelle.GetDesc(&mut d) };
        self.texturen_sichern(d.Width, d.Height)?;
        let (Some(e), Some(z)) = (self.eingang.as_ref(), self.sdr.as_ref()) else {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER));
        };
        unsafe {
            self.ctx.CopyResource(&e.tex, quelle);
            if self.konst_weiss != Some(self.sdr_weiss) {
                let k = KonstWandler { weiss_kehrwert: weiss_kehrwert(self.sdr_weiss), _w: [0.0; 3] };
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(&self.konst, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
                std::ptr::copy_nonoverlapping(&k as *const KonstWandler as *const u8, m.pData as *mut u8, std::mem::size_of::<KonstWandler>());
                self.ctx.Unmap(&self.konst, 0);
                self.konst_weiss = Some(self.sdr_weiss);
            }
            // Der ganze Zustand, den das Dreieck braucht - das Geraet teilt
            // sich die Aufnahme mit dem Encoder, verlassen wird sich auf nichts.
            self.ctx.OMSetRenderTargets(Some(&[Some(z.sicht.clone())]), None);
            self.ctx.OMSetBlendState(None, None, 0xffff_ffff);
            self.ctx.OMSetDepthStencilState(None, 0);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: z.w as f32,
                Height: z.h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.IASetInputLayout(None);
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(&self.ps_sdr, None);
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.konst.clone())]));
            self.ctx.PSSetShaderResources(0, Some(&[Some(e.sicht.clone())]));
            self.ctx.Draw(3, 0);
            // Nichts gebunden lassen: das Ziel geht gleich in CopyResource,
            // die Kopie beim naechsten Bild auch.
            self.ctx.PSSetShaderResources(0, Some(&[None]));
            self.ctx.OMSetRenderTargets(None, None);
        }
        Ok(z.tex.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
