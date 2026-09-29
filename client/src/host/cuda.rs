// CUDA-Bruecke des Windows-Hosts: die Ebenen, die der Wandler auf der Karte
// rechnet (D3D11-Texturen R16_UINT bzw. R16G16_UINT), ohne Umweg ueber den
// Hauptspeicher in einen CUDA-Rahmen fuer nvenc.
//
// Warum CUDA: nvenc nimmt ueber FFmpeg D3D11-Texturen nur in den Formaten,
// die hwcontext_d3d11va kennt (NV12, P010, BGRA, X2BGR10, ...) - fuer 4:4:4
// mit 10 Bit gibt es kein DXGI-Format. CUDA-Rahmen gibt es in YUV444P16 und
// P010 (hwcontext_cuda); nvenc liest sie direkt aus dem Kartenspeicher. Die
// Bruecke meldet die Zieltexturen des Wandlers einmal bei CUDA an
// (cuGraphicsD3D11RegisterResource) und kopiert je Bild jede Ebene mit
// cuMemcpy2DAsync in die Ebene des Rahmens - Karte zu Karte, ein paar
// Megabyte, Bruchteile einer Millisekunde.
//
// Reihenfolge: cuGraphicsMapResources sichert zu, dass alles, was D3D11 vor
// dem Abbilden auf den Texturen angestossen hat (das Dreieck des Wandlers),
// fertig ist, bevor die Kopie im Strom beginnt. Kopiert wird auf dem Strom
// des FFmpeg-Geraets (AVCUDADeviceContext.stream - denselben gibt nvenc
// seinem Encoder mit, nvEncSetIOCudaStreams); danach wartet die Bruecke auf
// ihn (cuStreamSynchronize), bevor der Rahmen an nvenc geht - der Encoder
// laeuft ohnehin synchron (delay 0), die Wartezeit ist die der Kopie.
//
// nvcuda.dll kommt mit dem NVIDIA-Treiber und wird zur Laufzeit aus dem
// Systemordner geladen (wie cuda_geraete in main.rs), nie freigegeben -
// FFmpeg laedt dieselbe DLL. Fehlt sie oder eine Funktion, ist das ein Err:
// der Encoder nimmt dann den Prozessorweg (encoder.rs, Betrieb::oeffnen).

use std::ffi::c_void;
use std::sync::OnceLock;

use ffmpeg_next as ffmpeg;
use ffmpeg::sys::*;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;

// CUresult ist int (0 = Erfolg), CUdevice int, die anderen Griffe Zeiger.
// CUDAAPI ist __stdcall - auf x64 dasselbe wie C.
type CuInit = unsafe extern "system" fn(u32) -> i32;
type CuD3d11GetDevice = unsafe extern "system" fn(*mut i32, *mut c_void) -> i32;
type CuCtxPush = unsafe extern "system" fn(*mut c_void) -> i32;
type CuCtxPop = unsafe extern "system" fn(*mut *mut c_void) -> i32;
type CuRegister = unsafe extern "system" fn(*mut *mut c_void, *mut c_void, u32) -> i32;
type CuUnregister = unsafe extern "system" fn(*mut c_void) -> i32;
type CuMapResources = unsafe extern "system" fn(u32, *mut *mut c_void, *mut c_void) -> i32;
type CuMappedArray = unsafe extern "system" fn(*mut *mut c_void, *mut c_void, u32, u32) -> i32;
type CuMemcpy2dAsync = unsafe extern "system" fn(*const Memcpy2d, *mut c_void) -> i32;
type CuStreamSynchronize = unsafe extern "system" fn(*mut c_void) -> i32;
type CuGetErrorName = unsafe extern "system" fn(i32, *mut *const i8) -> i32;

/// Die Funktionen aus nvcuda.dll, die die Bruecke braucht.
struct Funktionen {
    d3d11_geraet: CuD3d11GetDevice,
    ctx_push: CuCtxPush,
    ctx_pop: CuCtxPop,
    anmelden: CuRegister,
    abmelden: CuUnregister,
    abbilden: CuMapResources,
    freigeben: CuMapResources,
    feld: CuMappedArray,
    kopieren: CuMemcpy2dAsync,
    warten: CuStreamSynchronize,
    fehlername: Option<CuGetErrorName>,
}

/// CUDA_MEMCPY2D (cuda.h, Fassung 2 - die von cuMemcpy2DAsync_v2): 128 Byte
/// auf x64, CUmemorytype ist ein enum (4 Byte, danach aufgefuellt).
#[repr(C)]
#[derive(Clone, Copy)]
struct Memcpy2d {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: u32,
    src_host: *const c_void,
    src_device: u64,
    src_array: *mut c_void,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: u32,
    dst_host: *mut c_void,
    dst_device: u64,
    dst_array: *mut c_void,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

/// CU_MEMORYTYPE_DEVICE und CU_MEMORYTYPE_ARRAY.
const SPEICHER_KARTE: u32 = 2;
const SPEICHER_FELD: u32 = 3;
/// CU_GRAPHICS_REGISTER_FLAGS_NONE.
const ANMELDUNG_OHNE: u32 = 0;

/// AVCUDADeviceContext (libavutil/hwcontext_cuda.h): Kontext, Strom, intern.
#[repr(C)]
struct AvCudaGeraet {
    cuda_ctx: *mut c_void,
    stream: *mut c_void,
    internal: *mut c_void,
}

fn funktionen() -> Result<&'static Funktionen, String> {
    static F: OnceLock<Result<Funktionen, String>> = OnceLock::new();
    F.get_or_init(|| unsafe { laden() }).as_ref().map_err(|e| e.clone())
}

unsafe fn laden() -> Result<Funktionen, String> {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    let dll = LoadLibraryExW(w!("nvcuda.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).map_err(|e| format!("nvcuda.dll fehlt ({e})"))?;
    macro_rules! holen {
        ($name:literal, $typ:ty) => {
            match GetProcAddress(dll, s!($name)) {
                Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $typ>(f),
                None => return Err(format!("nvcuda.dll ohne {} (Treiber zu alt)", $name)),
            }
        };
    }
    let init = holen!("cuInit", CuInit);
    let f = Funktionen {
        d3d11_geraet: holen!("cuD3D11GetDevice", CuD3d11GetDevice),
        ctx_push: holen!("cuCtxPushCurrent_v2", CuCtxPush),
        ctx_pop: holen!("cuCtxPopCurrent_v2", CuCtxPop),
        anmelden: holen!("cuGraphicsD3D11RegisterResource", CuRegister),
        abmelden: holen!("cuGraphicsUnregisterResource", CuUnregister),
        abbilden: holen!("cuGraphicsMapResources", CuMapResources),
        freigeben: holen!("cuGraphicsUnmapResources", CuMapResources),
        feld: holen!("cuGraphicsSubResourceGetMappedArray", CuMappedArray),
        kopieren: holen!("cuMemcpy2DAsync_v2", CuMemcpy2dAsync),
        warten: holen!("cuStreamSynchronize", CuStreamSynchronize),
        fehlername: GetProcAddress(dll, s!("cuGetErrorName")).map(|f| std::mem::transmute::<unsafe extern "system" fn() -> isize, CuGetErrorName>(f)),
    };
    let r = init(0);
    if r != 0 {
        return Err(format!("cuInit: {}", fehlertext(Some(&f), r)));
    }
    Ok(f)
}

/// Ein CUDA-Fehler als Text: Name und Nummer (CUDA_ERROR_INVALID_VALUE (1)).
fn fehlertext(f: Option<&Funktionen>, r: i32) -> String {
    let name = f.and_then(|f| f.fehlername).and_then(|g| unsafe {
        let mut p: *const i8 = std::ptr::null();
        (g(r, &mut p) == 0 && !p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
    });
    match name {
        Some(n) => format!("{n} ({r})"),
        None => format!("CUDA-Fehler {r}"),
    }
}

fn pruefen(f: &Funktionen, was: &str, r: i32) -> Result<(), String> {
    if r == 0 { Ok(()) } else { Err(format!("{was}: {}", fehlertext(Some(f), r))) }
}

/// Die CUDA-Ordnungszahl der Karte, auf der dieses D3D11-Geraet liegt
/// (cuD3D11GetDevice mit ihrem DXGI-Adapter) - nicht der Index bei DXGI.
pub fn ordnungszahl(device: &ID3D11Device) -> Result<i32, String> {
    let f = funktionen()?;
    let dxgi: IDXGIDevice = device.cast().map_err(|e| format!("IDXGIDevice: {e}"))?;
    let adapter = unsafe { dxgi.GetAdapter() }.map_err(|e| format!("GetAdapter: {e}"))?;
    let mut n = -1i32;
    pruefen(f, "cuD3D11GetDevice", unsafe { (f.d3d11_geraet)(&mut n, adapter.as_raw()) })?;
    Ok(n)
}

/// Ein FFmpeg-Geraet (CUDA) auf der Karte dieses D3D11-Geraets - mit
/// eigenem Kontext (av_hwdevice_ctx_create, FFmpeg laedt nvcuda.dll selbst).
pub fn geraet(device: &ID3D11Device) -> Result<*mut AVBufferRef, String> {
    let n = ordnungszahl(device)?;
    let name = std::ffi::CString::new(n.to_string()).unwrap_or_default();
    let mut r: *mut AVBufferRef = std::ptr::null_mut();
    let e = unsafe { av_hwdevice_ctx_create(&mut r, AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, name.as_ptr(), std::ptr::null_mut(), 0) };
    if e < 0 || r.is_null() {
        return Err(crate::ffmpeg_grund(&format!("av_hwdevice_ctx_create (CUDA-Geraet {n})"), e));
    }
    Ok(r)
}

/// Rahmenpool (CUDA) in diesem Format (YUV444P16LE oder P010LE) und dieser
/// Groesse - Kartenspeicher, den nvenc direkt liest.
pub fn pool(geraet: *mut AVBufferRef, sw_format: AVPixelFormat, w: i32, h: i32) -> Result<*mut AVBufferRef, String> {
    unsafe {
        let r = av_hwframe_ctx_alloc(geraet);
        if r.is_null() {
            return Err("av_hwframe_ctx_alloc (CUDA)".into());
        }
        let fc = (*r).data as *mut AVHWFramesContext;
        (*fc).format = AVPixelFormat::AV_PIX_FMT_CUDA;
        (*fc).sw_format = sw_format;
        (*fc).width = w;
        (*fc).height = h;
        let e = av_hwframe_ctx_init(r);
        if e < 0 {
            let mut r = r;
            av_buffer_unref(&mut r);
            return Err(crate::ffmpeg_grund("av_hwframe_ctx_init (CUDA)", e));
        }
        Ok(r)
    }
}

/// Eine Ebene fuer die Bruecke: die Textur des Wandlers und wie viele Byte
/// je Zeile und Zeilen davon in die Ebene des Rahmens gehen.
pub struct Ebene {
    pub textur: ID3D11Texture2D,
    pub bytes_je_zeile: usize,
    pub zeilen: usize,
}

/// Die angemeldeten Texturen des Wandlers auf dem Kontext des
/// FFmpeg-Geraets. Haelt eine eigene Referenz auf das Geraet (der Kontext
/// lebt, bis alles abgemeldet ist) und die Texturen selbst.
pub struct Bruecke {
    geraet: *mut AVBufferRef,
    ctx: *mut c_void,
    strom: *mut c_void,
    res: Vec<*mut c_void>,
    ebenen: Vec<Ebene>,
}

unsafe impl Send for Bruecke {}

/// Den Kontext fuer die Dauer eines Aufrufs anheften (cuCtxPush/Pop).
struct Angeheftet<'a>(&'a Funktionen);

impl<'a> Angeheftet<'a> {
    fn neu(f: &'a Funktionen, ctx: *mut c_void) -> Result<Angeheftet<'a>, String> {
        pruefen(f, "cuCtxPushCurrent", unsafe { (f.ctx_push)(ctx) })?;
        Ok(Angeheftet(f))
    }
}

impl Drop for Angeheftet<'_> {
    fn drop(&mut self) {
        let mut alt = std::ptr::null_mut();
        unsafe { (self.0.ctx_pop)(&mut alt) };
    }
}

impl Bruecke {
    /// Die Texturen auf dem CUDA-Geraet `geraet` anmelden, in dieser
    /// Reihenfolge - Ebene i geht in data[i] des Rahmens.
    pub fn neu(geraet: *mut AVBufferRef, ebenen: Vec<Ebene>) -> Result<Bruecke, String> {
        let f = funktionen()?;
        if geraet.is_null() {
            return Err("CUDA-Bruecke ohne Geraet".into());
        }
        let (ctx, strom) = unsafe {
            let dc = (*geraet).data as *const AVHWDeviceContext;
            if (*dc).type_ != AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA {
                return Err("CUDA-Bruecke: kein CUDA-Geraet".into());
            }
            let g = (*dc).hwctx as *const AvCudaGeraet;
            ((*g).cuda_ctx, (*g).stream)
        };
        let eigen = unsafe { av_buffer_ref(geraet) };
        if eigen.is_null() {
            return Err("CUDA-Bruecke: av_buffer_ref".into());
        }
        let mut b = Bruecke { geraet: eigen, ctx, strom, res: Vec::new(), ebenen: Vec::new() };
        {
            let _k = Angeheftet::neu(f, ctx)?;
            for e in &ebenen {
                let mut r = std::ptr::null_mut();
                pruefen(f, "cuGraphicsD3D11RegisterResource", unsafe { (f.anmelden)(&mut r, e.textur.as_raw(), ANMELDUNG_OHNE) })?;
                b.res.push(r);
            }
        }
        b.ebenen = ebenen;
        Ok(b)
    }

    /// Die Ebenen in den Rahmen `frame` (aus einem CUDA-Pool: data[i] ist
    /// ein CUdeviceptr, linesize[i] der Zeilenabstand) kopieren und warten,
    /// bis die Kopie fertig ist.
    pub fn kopieren(&self, frame: *const AVFrame) -> Result<(), String> {
        let f = funktionen()?;
        if frame.is_null() {
            return Err("CUDA-Bruecke: kein Rahmen".into());
        }
        let ziele: Vec<(u64, usize)> = unsafe { (0..self.ebenen.len()).map(|i| ((*frame).data[i] as u64, (*frame).linesize[i].max(0) as usize)).collect() };
        if let Some(i) = ziele.iter().zip(&self.ebenen).position(|((p, ls), e)| *p == 0 || *ls < e.bytes_je_zeile) {
            return Err(format!("CUDA-Bruecke: Ebene {i} des Rahmens fehlt oder ist zu schmal"));
        }
        let _k = Angeheftet::neu(f, self.ctx)?;
        let mut res = self.res.clone();
        pruefen(f, "cuGraphicsMapResources", unsafe { (f.abbilden)(res.len() as u32, res.as_mut_ptr(), self.strom) })?;
        let mut ergebnis = Ok(());
        for (i, (e, (ziel, abstand))) in self.ebenen.iter().zip(&ziele).enumerate() {
            let mut feld = std::ptr::null_mut();
            let r = unsafe { (f.feld)(&mut feld, res[i], 0, 0) };
            if r != 0 {
                ergebnis = Err(format!("cuGraphicsSubResourceGetMappedArray (Ebene {i}): {}", fehlertext(Some(f), r)));
                break;
            }
            let k = Memcpy2d {
                src_x_in_bytes: 0,
                src_y: 0,
                src_memory_type: SPEICHER_FELD,
                src_host: std::ptr::null(),
                src_device: 0,
                src_array: feld,
                src_pitch: 0,
                dst_x_in_bytes: 0,
                dst_y: 0,
                dst_memory_type: SPEICHER_KARTE,
                dst_host: std::ptr::null_mut(),
                dst_device: *ziel,
                dst_array: std::ptr::null_mut(),
                dst_pitch: *abstand,
                width_in_bytes: e.bytes_je_zeile,
                height: e.zeilen,
            };
            let r = unsafe { (f.kopieren)(&k, self.strom) };
            if r != 0 {
                ergebnis = Err(format!("cuMemcpy2DAsync (Ebene {i}): {}", fehlertext(Some(f), r)));
                break;
            }
        }
        let r = unsafe { (f.freigeben)(res.len() as u32, res.as_mut_ptr(), self.strom) };
        ergebnis?;
        pruefen(f, "cuGraphicsUnmapResources", r)?;
        pruefen(f, "cuStreamSynchronize", unsafe { (f.warten)(self.strom) })
    }
}

impl Drop for Bruecke {
    fn drop(&mut self) {
        if let Ok(f) = funktionen() {
            if let Ok(_k) = Angeheftet::neu(f, self.ctx) {
                for r in self.res.drain(..) {
                    unsafe { (f.abmelden)(r) };
                }
            }
        }
        unsafe { av_buffer_unref(&mut self.geraet) };
    }
}

/// Einen Rahmen aus dem CUDA-Pool in `frame` holen (der alte Inhalt wird
/// vorher freigegeben).
pub fn rahmen_holen(pool: *mut AVBufferRef, frame: *mut AVFrame) -> Result<(), String> {
    if frame.is_null() || pool.is_null() {
        return Err("CUDA-Rahmen: kein Rahmen oder kein Pool".into());
    }
    unsafe {
        av_frame_unref(frame);
        let e = av_hwframe_get_buffer(pool, frame, 0);
        if e < 0 {
            return Err(crate::ffmpeg_grund("av_hwframe_get_buffer (CUDA)", e));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CUDA_MEMCPY2D liegt wie in cuda.h (x64): 128 Byte, die Speicherarten
    /// als 4-Byte-enum mit Auffuellung davor. AVCUDADeviceContext: drei Zeiger.
    #[test]
    fn memcpy2d_wie_in_cuda_h() {
        assert_eq!(std::mem::size_of::<Memcpy2d>(), 128);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_memory_type), 16);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_host), 24);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_device), 32);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_array), 40);
        assert_eq!(std::mem::offset_of!(Memcpy2d, src_pitch), 48);
        assert_eq!(std::mem::offset_of!(Memcpy2d, dst_memory_type), 72);
        assert_eq!(std::mem::offset_of!(Memcpy2d, dst_device), 88);
        assert_eq!(std::mem::offset_of!(Memcpy2d, dst_pitch), 104);
        assert_eq!(std::mem::offset_of!(Memcpy2d, width_in_bytes), 112);
        assert_eq!(std::mem::offset_of!(Memcpy2d, height), 120);
        assert_eq!(std::mem::size_of::<AvCudaGeraet>(), 24);
    }

    /// Ohne NVIDIA-Treiber (VM) ist CUDA ein Err mit Grund - kein Absturz;
    /// mit Treiber laedt es.
    #[test]
    fn ohne_treiber_ein_grund() {
        match funktionen() {
            Ok(_) => {}
            Err(e) => assert!(e.contains("nvcuda.dll") || e.contains("cuInit"), "{e}"),
        }
    }
}
