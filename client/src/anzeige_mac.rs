// Anzeige ueber Metal auf dem Mac, ohne neue Kiste: Metal, QuartzCore und
// CoreVideo direkt ueber objc_msgSend und ihre C-Funktionen, wie in
// tray_mac.rs und clipboard_mac.rs. Aufbau und Schnittstelle folgen der
// Direct3D-Anzeige unter Windows (anzeige.rs); main.rs ruft beide gleich.
//
// Stufe 1: Die Ebenen des Decoders gehen ohne Kopie auf die Karte. Die
// CVPixelBuffer von VideoToolbox liegen in IOSurfaces; CVMetalTextureCache
// macht aus jeder Ebene eine Textur (Y als R8/R16, U/V als RG8/RG16, alle
// ganzzahlig gelesen). Ein Shader rechnet sie mit derselben 16.16-
// Ganzzahlarithmetik wie `zeile_rgb` in eine Zwischentextur in Bildgroesse:
// 4:4:4 und 4:2:0, 8 und 16 Bit (10 Bit oben buendig), voller und
// begrenzter Wertebereich - bitgleich zu `to_rgb`. Die Zwischentextur ist
// RGBA8 ganzzahlig, mit den Bytes B, G, R, A - so liegt 0x00RRGGBB im
// Speicher, und der RGB-Rueckfall laedt ein fertiges Bild unveraendert.
//
// Stufe 2 legt das Bild ins Ziel: Rechteck aus `ziel_rechteck`, Schritt in
// 16.16 und dieselbe Mischung der vier Nachbarn wie `blit`/`mische`,
// ganzzahlig - die 64-Bit-Summe von `mische` in zwei 32-Bit-Haelften
// (mulhi). Anders als unter Windows (Filter der Karte, Toleranz 2) ist damit
// auch das skalierte Bild bitgleich zum CPU-Weg. Darueber die Oberflaeche
// aus einer zweiten Textur (0xTTRRGGBB, T = Durchsicht, siehe ui.rs):
// Bild * T / 255 + Farbe.
//
// Praesentation: ein CAMetalLayer als Unterschicht der Ansicht von winit
// (wie bei softbuffer bleibt die Schicht der Ansicht selbst unberuehrt;
// Rahmen und Massstab folgen ihr), mit einem Puffer in Bildpunkten (HiDPI),
// mit displaySyncEnabled (Bildwechsel abwarten, kein Reissen) und
// presentDrawable. nextDrawable wartet, wenn alle Puffer der Schicht belegt
// sind - der Fensterfaden traegt aber auch die Eingabe. Deshalb wird vorher
// gezaehlt: sind schon zwei Bilder praesentiert und noch nicht auf dem
// Schirm (presentedTime 0), ist die Anzeige nicht bereit, und das Bild
// wartet auf den naechsten Takt - wie unter Windows mit dem Warteobjekt. Ein
// verdecktes Fenster zeichnet nicht; beim Aufdecken wird das letzte Bild neu
// praesentiert. Ein Waechter zaehlt, ob praesentierte Bilder je auf dem
// Schirm ankommen - tun sie es am sichtbaren Fenster nie, zeichnet main.rs
// mit softbuffer weiter. `fenster_selbsttest` (--anzeige-selbsttest) prueft
// das an einem echten Fenster von winit.
//
// Lebensdauer: Die Karte liest die IOSurface erst, nachdem bild_roh zurueck
// ist. Bis der Befehlspuffer fertig ist, haelt die Anzeige deshalb einen
// eigenen Griff auf den CVPixelBuffer und seine CVMetalTextures - sonst gaebe
// der Decoder den Puffer an seinen Vorrat zurueck und schriebe das naechste
// Bild hinein, waehrend die Karte noch liest. Der Stand der Befehlspuffer
// wird abgefragt (status), nie abgewartet; warten tun nur Drop und die
// Tests.
//
// Alle eigenen Texturen liegen privat auf der Karte. Die CPU schreibt ueber
// einen Zwischenpuffer und einen Blit im Befehlsstrom (Oberflaeche,
// RGB-Rueckfall); so ueberschreibt ein Upload nie, was ein noch laufendes
// Bild liest. Die Zwischenpuffer kommen aus einem kleinen Vorrat - nur
// solche, die kein laufender Befehlspuffer mehr liest. Ohne Metal-Geraet
// (etwa in einer virtuellen Maschine ohne Grafik) scheitert schon `neu`, und
// main.rs zeichnet mit softbuffer.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::time::{Duration, Instant};

use crate::vt_decoder::{self, ffi as cf};
use crate::{protokoll, ui, Frame};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

// NSString kommt aus Foundation.
#[link(name = "Foundation", kind = "framework")]
extern "C" {}

#[link(name = "Metal", kind = "framework")]
extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

// CAMetalLayer und CATransaction; die Uhr, in der auch die Zeitstempel der
// Befehlspuffer stehen.
#[link(name = "QuartzCore", kind = "framework")]
extern "C" {
    fn CACurrentMediaTime() -> f64;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVMetalTextureCacheCreate(
        alloc: *const c_void,
        cache_attribute: *const c_void,
        geraet: Id,
        textur_attribute: *const c_void,
        aus: *mut *mut c_void,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CVMetalTextureCacheCreateTextureFromImage(
        alloc: *const c_void,
        cache: *mut c_void,
        bild: cf::CVPixelBufferRef,
        attribute: *const c_void,
        format: usize,
        breite: usize,
        hoehe: usize,
        ebene: usize,
        aus: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(t: *mut c_void) -> Id;
    fn CVMetalTextureCacheFlush(cache: *mut c_void, flags: u64);
}

/// MTLPixelFormat.
const R8_UINT: usize = 13;
const R16_UINT: usize = 23;
const RG8_UINT: usize = 33;
const RG16_UINT: usize = 63;
const RGBA8_UINT: usize = 73;
const BGRA8_UNORM: usize = 80;
/// MTLTextureUsageShaderRead, MTLTextureUsageRenderTarget.
const NUTZUNG_LESEN: usize = 1;
const NUTZUNG_ZIEL: usize = 4;
/// MTLStorageModePrivate: nur die Karte.
const SPEICHER_PRIVAT: usize = 2;
/// MTLResourceStorageModeShared: Zwischenpuffer, die die CPU fuellt oder liest.
const PUFFER_GETEILT: usize = 0;
/// MTLLoadActionDontCare (jeder Bildpunkt wird geschrieben), MTLStoreActionStore.
const LADEN_EGAL: usize = 0;
const SPEICHERN: usize = 1;
/// MTLPrimitiveTypeTriangle.
const DREIECKE: usize = 3;
/// NSWindowOcclusionStateVisible: ein Teil des Fensters ist zu sehen.
const FENSTER_SICHTBAR: usize = 1 << 1;
/// MTLCommandBufferStatusCompleted und ...Error.
const STATUS_FERTIG: usize = 4;
const STATUS_FEHLER: usize = 5;
/// MTLCommandBufferErrorDeviceRemoved (externe Karte abgezogen; auf Apple
/// silicon gibt es das nicht).
const FEHLER_GERAET_WEG: isize = 11;
/// So viele Bilder duerfen praesentiert und noch nicht auf dem Schirm sein;
/// mit dem gezeigten ist die Schicht mit ihren drei Puffern dann voll.
const HOECHSTENS_UNTERWEGS: usize = 2;
/// Ein praesentiertes Bild, das nach dieser Zeit noch nicht auf dem Schirm
/// ist, hat CoreAnimation uebersprungen (presentedTime bleibt dann 0) - es
/// belegt keinen Puffer mehr.
const PRAESENT_FRIST: Duration = Duration::from_millis(50);
/// Waechter: kommt am sichtbaren Fenster nach so vielen Versuchen und so
/// langer Zeit kein einziges Bild auf dem Schirm an, zeichnet main.rs mit
/// softbuffer weiter. Der Startbildschirm allein praesentiert 30-mal je
/// Sekunde; ein Fenster, das Metal einmal gezeigt hat, bleibt bei Metal.
const WAECHTER_VERSUCHE: u32 = 60;
const WAECHTER_FRIST: Duration = Duration::from_secs(3);
/// Mehr offene Befehlspuffer als das heisst: die Karte kommt nicht nach.
/// Dann wird auf den aeltesten gewartet, statt Speicher anzuhaeufen.
const HOECHSTENS_BEFEHLSPUFFER: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlOrigin {
    x: usize,
    y: usize,
    z: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlSize {
    w: usize,
    h: usize,
    d: usize,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
struct CgSize {
    w: f64,
    h: f64,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
struct CgRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Ein objc_msgSend mit fester Signatur (wie in tray_mac.rs): jede Form
/// bekommt ihren eigenen Funktionszeigertyp, damit Zahlen, Kommazahlen und
/// Strukturen nach der Aufrufkonvention der Plattform uebergeben werden.
/// Rechtecke als Rueckgabe (bounds, frame) gehen ueber `msg_rect` - auf
/// Intel braucht das objc_msgSend_stret.
macro_rules! senden {
    ($obj:expr, $sel:expr $(, $arg:expr => $typ:ty)* ; -> $ret:ty) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $typ)*) -> $ret = std::mem::transmute(objc_msgSend as *const c_void);
        f($obj, $sel $(, $arg)*)
    }};
}

fn klasse(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

unsafe fn msg_id(obj: Id, s: &CStr) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    senden!(obj, sel(s); -> Id)
}

unsafe fn msg_id_1(obj: Id, s: &CStr, a: Id) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    senden!(obj, sel(s), a => Id; -> Id)
}

unsafe fn msg_void(obj: Id, s: &CStr) {
    if !obj.is_null() {
        senden!(obj, sel(s); -> ())
    }
}

unsafe fn msg_void_1(obj: Id, s: &CStr, a: Id) {
    if !obj.is_null() {
        senden!(obj, sel(s), a => Id; -> ())
    }
}

unsafe fn msg_void_zahl(obj: Id, s: &CStr, n: usize) {
    if !obj.is_null() {
        senden!(obj, sel(s), n => usize; -> ())
    }
}

unsafe fn msg_void_bool(obj: Id, s: &CStr, b: bool) {
    if !obj.is_null() {
        senden!(obj, sel(s), b as u8 => u8; -> ())
    }
}

unsafe fn msg_zahl(obj: Id, s: &CStr) -> usize {
    if obj.is_null() {
        return 0;
    }
    senden!(obj, sel(s); -> usize)
}

unsafe fn msg_bool(obj: Id, s: &CStr) -> bool {
    if obj.is_null() {
        return false;
    }
    senden!(obj, sel(s); -> u8) != 0
}

unsafe fn msg_f64(obj: Id, s: &CStr) -> f64 {
    if obj.is_null() {
        return 0.0;
    }
    senden!(obj, sel(s); -> f64)
}

#[cfg(target_arch = "x86_64")]
#[link(name = "objc")]
extern "C" {
    fn objc_msgSend_stret();
}

/// Ein CGRect als Rueckgabe (bounds, frame). Auf Apple silicon kommt es in
/// vier Gleitkommaregistern zurueck, auf Intel ueber einen versteckten
/// Zeiger - dafuer gibt es objc_msgSend_stret.
unsafe fn msg_rect(obj: Id, s: &CStr) -> CgRect {
    if obj.is_null() {
        return CgRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
    }
    #[cfg(target_arch = "x86_64")]
    {
        let f: unsafe extern "C" fn(Id, Sel) -> CgRect = std::mem::transmute(objc_msgSend_stret as *const c_void);
        f(obj, sel(s))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        senden!(obj, sel(s); -> CgRect)
    }
}

/// NSString aus Rust-Text (autoreleased: braucht einen Pool).
unsafe fn ns_text(t: &str) -> Id {
    let c = CString::new(t.replace('\0', " ")).unwrap_or_default();
    msg_id_1(klasse(c"NSString"), c"stringWithUTF8String:", c.as_ptr() as Id)
}

/// NSString zurueck nach Rust.
unsafe fn rust_text(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = senden!(s, sel(c"UTF8String"); -> *const c_char);
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

/// Ein NSError als Text: Beschreibung und Code.
unsafe fn fehlertext(e: Id) -> String {
    if e.is_null() {
        return "ohne Angabe".into();
    }
    let code = senden!(e, sel(c"code"); -> isize);
    format!("{} (Code {code})", rust_text(msg_id(e, c"localizedDescription")).trim())
}

/// Ein Autorelease-Pool fuer die Dauer eines Geltungsbereichs.
struct Pool(*mut c_void);

impl Pool {
    fn neu() -> Pool {
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

// ------------------------------------------------------------------- HDR

/// Kann dieser Weg HDR-Bilder zeigen (Bit 1 in IN_ANZEIGE)? Noch nicht: die
/// EDR-Schicht und die PQ-Shader kommen mit Schritt 5a des HDR-Plans. Bis
/// dahin meldet der Mac-Client ehrlich "nein" - ein Host sendet ihm dann
/// nie PQ (Grund 5).
pub const HDR_DARSTELLUNG: bool = false;

/// Der Bildschirm des Fensters fuer IN_ANZEIGE, aus NSScreen: der
/// EDR-Kopfraum, den er kann (maximumPotentialExtendedDynamicRange-
/// ColorComponentValue; ueber 1 heisst HDR-faehig, etwa ein XDR-Schirm oder
/// ein virtueller Bildschirm mit HDR), und der, der gerade geht
/// (maximumExtendedDynamicRangeColorComponentValue - Helligkeit,
/// Energiesparen). Pegel in nit nennt macOS nicht (0 = unbekannt). None ohne
/// Fenster oder Bildschirm. Nur auf dem Hauptfaden.
pub fn schirm_lage(ansicht: *mut c_void) -> Option<crate::hdr::Schirm> {
    let _pool = Pool::neu();
    unsafe {
        let fenster = msg_id(ansicht as Id, c"window");
        let schirm = msg_id(fenster, c"screen");
        if schirm.is_null() {
            return None;
        }
        let potentiell = msg_f64(schirm, c"maximumPotentialExtendedDynamicRangeColorComponentValue") as f32;
        let aktuell = msg_f64(schirm, c"maximumExtendedDynamicRangeColorComponentValue") as f32;
        Some(crate::hdr::Schirm {
            hdr: potentiell > 1.0,
            kopfraum_potentiell: potentiell.max(1.0),
            kopfraum_aktuell: aktuell.max(1.0),
            ..crate::hdr::Schirm::default()
        })
    }
}

/// Ein eigener Griff auf ein Objective-C-Objekt; Drop gibt ihn ab.
struct Obj(Id);

impl Obj {
    /// Uebernimmt einen Griff, den der Aufruf schon mitbringt (new..., alloc
    /// + init, ...Create...). None fuer nil.
    unsafe fn eigen(id: Id) -> Option<Obj> {
        (!id.is_null()).then_some(Obj(id))
    }

    /// Haelt ein fremdes (autoreleased) Objekt mit einem eigenen Griff fest.
    unsafe fn halten(id: Id) -> Option<Obj> {
        if id.is_null() {
            return None;
        }
        senden!(id, sel(c"retain"); -> Id);
        Some(Obj(id))
    }

    /// Ein zweiter Griff auf dasselbe Objekt.
    fn zweiter(&self) -> Obj {
        unsafe { senden!(self.0, sel(c"retain"); -> Id) };
        Obj(self.0)
    }
}

impl Drop for Obj {
    fn drop(&mut self) {
        unsafe { senden!(self.0, sel(c"release"); -> ()) }
    }
}

/// Ein eigener Griff auf ein CoreFoundation-Objekt (Textur-Cache,
/// CVMetalTexture, CVPixelBuffer); Drop gibt ihn mit CFRelease ab.
struct Cf(*mut c_void);

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { cf::CFRelease(self.0 as cf::CFTypeRef) }
    }
}

/// Aenderungen an Schichten ohne die Viertelsekunde Animation, die
/// CoreAnimation sonst jeder Aenderung gibt.
unsafe fn ohne_animation(f: impl FnOnce()) {
    let ca = klasse(c"CATransaction");
    msg_void(ca, c"begin");
    senden!(ca, sel(c"setDisableActions:"), 1u8 => u8; -> ());
    f();
    msg_void(ca, c"commit");
}

/// Die Shader, einmal beim Start uebersetzt (newLibraryWithSource). Stufe 1
/// rechnet ganzzahlig mit den Konstanten aus `zeile_rgb`, Stufe 2 mit den
/// Quellpunkten und Gewichten aus `blit`/`mische`.
const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

// ---------- gemeinsam: ein Dreieck ueber das ganze Ziel, ohne Vertexpuffer ----------
struct Punkt { float4 pos [[position]]; };

vertex Punkt vs_main(uint id [[vertex_id]]) {
    float2 p = float2(float((id << 1) & 2u), float(id & 2u));
    Punkt o;
    o.pos = float4(p * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

// ---------- Stufe 1: Ebenen -> RGB, bitgleich zu zeile_rgb ----------
struct Format {
    uint sub;        // 4:2:0: je zwei Bildpunkte teilen sich einen Farbwert
    uint schieb;     // 8 Bit: >>0, oben buendig 16 Bit: >>8
    uint begrenzt;   // begrenzter Wertebereich: vor der Matrix dehnen
    uint _f;
};

fragment uint4 ps_umrechnen(Punkt in [[stage_in]],
                            constant Format& f [[buffer(0)]],
                            texture2d<uint, access::read> ebene_y [[texture(0)]],
                            texture2d<uint, access::read> ebene_uv [[texture(1)]]) {
    uint2 xy = uint2(in.pos.xy);                    // pos.xy ist die Bildpunktmitte (x+0.5)
    uint2 c = (f.sub != 0) ? (xy >> 1) : xy;        // naechster Farbwert, wie zeile_rgb
    int y = int(ebene_y.read(xy).r >> f.schieb);
    uint2 uv = ebene_uv.read(c).rg;                 // U und V als Paar in einer Ebene
    int cb = int(uv.x >> f.schieb) - 128;           // Nullpunkt 128
    int cr = int(uv.y >> f.schieb) - 128;
    if (f.begrenzt != 0) {
        y = ((y - 16) * 76309 + 32768) >> 16;       // 255/219, gerundet
        cb = (cb * 74606 + 32768) >> 16;            // 255/224, gerundet
        cr = (cr * 74606 + 32768) >> 16;
    }
    int r = y + ((103206 * cr) >> 16);              // 1.5748 - dieselben Konstanten wie zeile_rgb
    int g = y - ((12276 * cb + 30681 * cr) >> 16);  // 0.1873, 0.4681 - EINE Verschiebung der Summe
    int b = y + ((121609 * cb) >> 16);              // 1.8556
    int3 rgb = clamp(int3(r, g, b), 0, 255);
    return uint4(uint(rgb.b), uint(rgb.g), uint(rgb.r), 255u);   // Bytes B, G, R, A = 0x00RRGGBB
}

// ---------- Stufe 2: Einpassen wie blit + Oberflaeche ----------
struct Anzeige {
    int4  rect;      // x, y, Breite, Hoehe des Bildes im Ziel (aus ziel_rechteck)
    uint2 bild;      // Bildbreite, Bildhoehe
    uint2 schritt;   // Quellschritt je Zielpunkt in 16.16: (fw << 16) / zw, wie blit
    uint  modus;     // Bit 0: Bild vorhanden, Bit 1: Oberflaeche sichtbar
    uint  grund;     // Grundfarbe 0x00RRGGBB (ui::BG)
    uint  _a0;
    uint  _a1;
};

// Ein Kanal wie mische(): (oben * (65536 - wy) + unten * wy) >> 32. oben und
// unten sind kleiner als 2^24, die Produkte brauchen bis zu 40 Bit - also
// je Produkt Unter- und Oberteil (mulhi) und der Uebertrag der Summe.
static uint kanal(uint a, uint b, uint c, uint d, uint wx, uint wy) {
    uint oben = a * (65536u - wx) + b * wx;
    uint unten = c * (65536u - wx) + d * wx;
    uint wy1 = 65536u - wy;
    uint lo1 = oben * wy1;
    uint hi1 = mulhi(oben, wy1);
    uint lo2 = unten * wy;
    uint hi2 = mulhi(unten, wy);
    uint lo = lo1 + lo2;
    uint hi = hi1 + hi2 + (lo < lo1 ? 1u : 0u);
    return hi & 255u;
}

fragment float4 ps_anzeigen(Punkt in [[stage_in]],
                            constant Anzeige& k [[buffer(0)]],
                            texture2d<uint, access::read> video [[texture(0)]],        // Bytes B, G, R, A
                            texture2d<uint, access::read> oberflaeche [[texture(1)]]) { // Bytes B, G, R, T
    int2 p = int2(in.pos.xy) - k.rect.xy;
    uint3 farbe = uint3((k.grund >> 16) & 255u, (k.grund >> 8) & 255u, k.grund & 255u);
    if ((k.modus & 1u) != 0 && all(p >= 0) && all(p < k.rect.zw)) {
        // blit()-Konvention: Quellpunkt = Zielpunkt * Schritt in 16.16, ohne
        // Mittenversatz; Ganzteil = linker/oberer Nachbar, Rest = Gewicht des
        // rechten/unteren, am Rand derselbe Punkt.
        uint2 f = uint2(p) * k.schritt;
        uint2 q0 = f >> 16;
        uint2 q1 = min(q0 + 1u, k.bild - 1u);
        uint2 w = f & 65535u;
        uint4 a = video.read(q0);
        if (w.x == 0 && w.y == 0) {
            farbe = a.zyx;                           // 1:1 oder ein Punkt genau getroffen
        } else {
            uint4 b = video.read(uint2(q1.x, q0.y));
            uint4 c = video.read(uint2(q0.x, q1.y));
            uint4 d = video.read(q1);
            farbe = uint3(kanal(a.z, b.z, c.z, d.z, w.x, w.y),
                          kanal(a.y, b.y, c.y, d.y, w.x, w.y),
                          kanal(a.x, b.x, c.x, d.x, w.x, w.y));
        }
    }
    if ((k.modus & 2u) != 0) {
        uint4 o = oberflaeche.read(uint2(in.pos.xy));
        farbe = min((farbe * o.w + 127u) / 255u + o.zyx, uint3(255u));   // Bild * T/255 (gerundet) + Farbe (vormultipliziert)
    }
    return float4(float3(farbe) / 255.0, 1.0);
}
"#;

/// Konstanten von Stufe 1 (struct Format, 16 Byte).
#[repr(C)]
struct KonstFormat {
    sub: u32,
    schieb: u32,
    begrenzt: u32,
    _f: u32,
}

/// Konstanten von Stufe 2 (struct Anzeige, 48 Byte; int4 ist in MSL auf 16
/// Byte ausgerichtet, der Rest liegt dicht dahinter).
#[repr(C)]
struct KonstAnzeige {
    rect: [i32; 4],
    bild: [u32; 2],
    schritt: [u32; 2],
    modus: u32,
    grund: u32,
    _a: [u32; 2],
}

/// Ergebnis eines Praesentierversuchs - dieselben Faelle wie unter Windows.
pub enum Praesentiert {
    Ok,
    /// Das Fenster ist verdeckt; es wurde nicht gezeichnet. Kein Fehler.
    Verdeckt,
    /// Etwas anderes ging schief, das Geraet lebt aber (Text fuer das
    /// Protokoll).
    Fehler(String),
    /// Das Geraet ist weg (externe Karte abgezogen).
    GeraetWeg(String),
}

/// Das Geraet, wie es im Protokoll und in der Statistik steht.
pub struct AdapterInfo {
    pub name: String,
    /// Gemeinsamer Speicher mit dem Prozessor (Apple silicon).
    pub gemeinsam: bool,
}

/// Eine Textur der Anzeige und ihre Groesse.
struct Textur {
    tex: Obj,
    w: u32,
    h: u32,
}

/// Ein Zwischenpuffer fuer Uploads der CPU und der Befehlspuffer, der ihn
/// zuletzt liest (None: frei).
struct Zwischenpuffer {
    puffer: Obj,
    laenge: usize,
    belegt: Option<Obj>,
}

/// Ein abgeschickter Befehlspuffer und was bis zu seinem Ende leben muss:
/// der CVPixelBuffer und die CVMetalTextures seiner Ebenen (Stufe 1).
struct Unterwegs {
    cb: Obj,
    _halten: Vec<Cf>,
}

/// Die Karte: Geraet, Warteschlange, die beiden Stufen, der Textur-Cache,
/// die Texturen des laufenden Bildes und - am Fenster - die Schicht.
pub struct Gpu {
    device: Obj,
    queue: Obj,
    stufe1: Obj,
    stufe2: Obj,
    cache: Cf,
    /// 1x1, fuer jede Bindung, die gerade keine Textur hat.
    leer: Obj,
    /// Am Fenster: die Ansicht, ihre Schicht und unsere Unterschicht.
    ansicht: Option<Obj>,
    wurzel: Option<Obj>,
    schicht: Option<Obj>,
    /// Bildgroesse, Ziel von Stufe 1, Quelle von Stufe 2.
    zwischen: Option<Textur>,
    /// Zielgroesse, die Oberflaeche.
    oberflaeche: Option<Textur>,
    unterwegs: VecDeque<Unterwegs>,
    /// Zwischenpuffer fuer die Uploads der CPU.
    vorrat: Vec<Zwischenpuffer>,
    /// Praesentierte Bilder, noch nicht auf dem Schirm, mit der Zeit ihrer
    /// Uebergabe.
    praesentiert: VecDeque<(Obj, Instant)>,
    /// Groesse des Ziels in Bildpunkten, der Massstab und der Rahmen der
    /// Wurzelschicht (Punkte), fuer die die Schicht eingestellt ist.
    pub breite: u32,
    pub hoehe: u32,
    skala: f64,
    rahmen: CgRect,
    /// Reissen erlaubt (displaySyncEnabled aus)? Auf dem Mac nie: die
    /// Anzeige wartet den Bildwechsel ab. Der Name ist der von Windows.
    pub tearing: bool,
    /// displaySyncEnabled ist gerade aus.
    sync_aus: bool,
    /// Verdeckt laut winit (Occluded), und ob seit dem letzten Bild ein
    /// Praesentieren ausfiel, weil das Fenster verdeckt war.
    verdeckt: bool,
    ausgelassen: bool,
    /// Waechter: praesentierte Bilder, die auf dem Schirm ankamen
    /// (presentedTime gesetzt), und - solange es keins gibt - die Versuche
    /// am sichtbaren Fenster seit dem ersten.
    gezeigt: u64,
    versuche: u32,
    erster_versuch: Option<Instant>,
    pub adapter: AdapterInfo,
    /// Format und Groesse der zuletzt gezeigten Ebenen - neu ins Protokoll
    /// nur, wenn sich daran etwas aendert.
    ebenen_stand: Option<(u32, u32, u32)>,
    /// CVMetalTextureCache scheiterte schon einmal (steht im Protokoll).
    cache_rueckfall: bool,
    letzter_fehler: Option<String>,
    weg: Option<String>,
}

/// Eine Stufe: Vertex- und Fragmentfunktion, Format des Ziels.
unsafe fn pipeline(device: Id, vs: Id, fs: Id, format: usize, name: &str) -> Result<Obj, String> {
    let desc = Obj::eigen(msg_id(msg_id(klasse(c"MTLRenderPipelineDescriptor"), c"alloc"), c"init"))
        .ok_or("MTLRenderPipelineDescriptor liess sich nicht anlegen")?;
    msg_void_1(desc.0, c"setVertexFunction:", vs);
    msg_void_1(desc.0, c"setFragmentFunction:", fs);
    let farben = msg_id(desc.0, c"colorAttachments");
    let a0 = senden!(farben, sel(c"objectAtIndexedSubscript:"), 0usize => usize; -> Id);
    msg_void_zahl(a0, c"setPixelFormat:", format);
    let mut err: Id = std::ptr::null_mut();
    let ps = senden!(device, sel(c"newRenderPipelineStateWithDescriptor:error:"), desc.0 => Id, &mut err as *mut Id => *mut Id; -> Id);
    Obj::eigen(ps).ok_or_else(|| format!("Stufe {name}: {}", fehlertext(err)))
}

/// Der Waechter als reine Rechnung: kein Bild je auf dem Schirm, und
/// mindestens `mindest` Versuche seit mindestens `frist`.
fn nie_gezeigt(gezeigt: u64, versuche: u32, seit_erstem: Option<Duration>, mindest: u32, frist: Duration) -> bool {
    gezeigt == 0 && versuche >= mindest && seit_erstem.is_some_and(|d| d >= frist)
}

impl Gpu {
    /// Ohne Fenster: Geraet, Shader, Cache - keine Schicht. Fuer
    /// --anzeigetest und die Tests.
    pub fn ohne_fenster() -> Result<Gpu, String> {
        let _pool = Pool::neu();
        let gpu = Gpu::bauen()?;
        protokoll::zeile(format!(
            "Anzeige: Metal {} (gemeinsamer Speicher {}), ohne Fenster",
            gpu.adapter.name,
            if gpu.adapter.gemeinsam { "ja" } else { "nein" }
        ));
        Ok(gpu)
    }

    /// Geraet und Schicht an der Ansicht des Fensters. `ansicht` ist die
    /// NSView aus winit (raw-window-handle, AppKit). Nur auf dem Hauptfaden.
    /// Alles, was scheitern kann, geschieht, bevor die Ansicht angefasst
    /// wird: scheitert es, bleibt sie frei fuer softbuffer.
    pub fn neu(ansicht: *mut c_void, ww: u32, wh: u32) -> Result<Gpu, String> {
        let _pool = Pool::neu();
        if ansicht.is_null() {
            return Err("keine Ansicht".into());
        }
        let wurzel = unsafe {
            let ansicht = ansicht as Id;
            let mut wurzel = msg_id(ansicht, c"layer");
            if wurzel.is_null() {
                msg_void_bool(ansicht, c"setWantsLayer:", true);
                wurzel = msg_id(ansicht, c"layer");
            }
            Obj::halten(wurzel).ok_or("die Ansicht hat keine Schicht")?
        };
        let mut gpu = Gpu::an_schicht(wurzel, ww, wh)?;
        gpu.ansicht = unsafe { Obj::halten(ansicht as Id) };
        Ok(gpu)
    }

    /// Der Teil von `neu` ab der Schicht der Ansicht (die Tests nehmen eine
    /// Schicht ohne Fenster).
    fn an_schicht(wurzel: Obj, ww: u32, wh: u32) -> Result<Gpu, String> {
        let mut gpu = Gpu::bauen()?;
        unsafe {
            let schicht = Obj::halten(msg_id(klasse(c"CAMetalLayer"), c"layer")).ok_or("CAMetalLayer liess sich nicht anlegen")?;
            msg_void_1(schicht.0, c"setDevice:", gpu.device.0);
            msg_void_zahl(schicht.0, c"setPixelFormat:", BGRA8_UNORM);
            msg_void_bool(schicht.0, c"setFramebufferOnly:", true);
            msg_void_bool(schicht.0, c"setOpaque:", true);
            msg_void_bool(schicht.0, c"setDisplaySyncEnabled:", true);
            msg_void_bool(schicht.0, c"setPresentsWithTransaction:", false);
            msg_void_bool(schicht.0, c"setAllowsNextDrawableTimeout:", true);
            msg_void_zahl(schicht.0, c"setMaximumDrawableCount:", 3);
            ohne_animation(|| msg_void_1(wurzel.0, c"addSublayer:", schicht.0));
            gpu.wurzel = Some(wurzel);
            gpu.schicht = Some(schicht);
        }
        gpu.groesse(ww, wh)?;
        protokoll::zeile(format!(
            "Anzeige: Metal {} (gemeinsamer Speicher {}), {}x{} bei Massstab {}, Bildwechsel abwarten, hoechstens {HOECHSTENS_UNTERWEGS} Bilder unterwegs",
            gpu.adapter.name,
            if gpu.adapter.gemeinsam { "ja" } else { "nein" },
            gpu.breite,
            gpu.hoehe,
            gpu.skala
        ));
        Ok(gpu)
    }

    /// Der gemeinsame Teil: Geraet, Warteschlange, Shader, beide Stufen,
    /// Textur-Cache.
    fn bauen() -> Result<Gpu, String> {
        unsafe {
            let device = Obj::eigen(MTLCreateSystemDefaultDevice()).ok_or("kein Metal-Geraet (MTLCreateSystemDefaultDevice lieferte nichts)")?;
            let name = rust_text(msg_id(device.0, c"name"));
            let gemeinsam = msg_bool(device.0, c"hasUnifiedMemory");
            let queue = Obj::eigen(msg_id(device.0, c"newCommandQueue")).ok_or("newCommandQueue lieferte nichts")?;
            let mut err: Id = std::ptr::null_mut();
            let lib = senden!(device.0, sel(c"newLibraryWithSource:options:error:"),
                ns_text(SHADER) => Id, std::ptr::null_mut() => Id, &mut err as *mut Id => *mut Id; -> Id);
            let lib = Obj::eigen(lib).ok_or_else(|| format!("Shader: {}", fehlertext(err)))?;
            let funktion = |n: &str| Obj::eigen(msg_id_1(lib.0, c"newFunctionWithName:", ns_text(n))).ok_or_else(|| format!("Shader: {n} fehlt"));
            let vs = funktion("vs_main")?;
            let ps1 = funktion("ps_umrechnen")?;
            let ps2 = funktion("ps_anzeigen")?;
            let stufe1 = pipeline(device.0, vs.0, ps1.0, RGBA8_UINT, "1 (umrechnen)")?;
            let stufe2 = pipeline(device.0, vs.0, ps2.0, BGRA8_UNORM, "2 (anzeigen)")?;
            let mut cache: *mut c_void = std::ptr::null_mut();
            let st = CVMetalTextureCacheCreate(std::ptr::null(), std::ptr::null(), device.0, std::ptr::null(), &mut cache);
            if st != 0 || cache.is_null() {
                return Err(format!("CVMetalTextureCacheCreate: CVReturn {st}"));
            }
            let cache = Cf(cache);
            let leer = textur_neu(device.0, 1, 1, RGBA8_UINT, NUTZUNG_LESEN)?;
            Ok(Gpu {
                device,
                queue,
                stufe1,
                stufe2,
                cache,
                leer,
                ansicht: None,
                wurzel: None,
                schicht: None,
                zwischen: None,
                oberflaeche: None,
                unterwegs: VecDeque::new(),
                vorrat: Vec::new(),
                praesentiert: VecDeque::new(),
                breite: 0,
                hoehe: 0,
                skala: 1.0,
                rahmen: CgRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
                tearing: false,
                sync_aus: false,
                verdeckt: false,
                ausgelassen: false,
                gezeigt: 0,
                versuche: 0,
                erster_versuch: None,
                adapter: AdapterInfo { name, gemeinsam },
                ebenen_stand: None,
                cache_rueckfall: false,
                letzter_fehler: None,
                weg: None,
            })
        }
    }

    /// Das Fenster hat eine andere Groesse oder steht auf einem Bildschirm
    /// mit anderem Massstab: Rahmen, Massstab und Puffergroesse der Schicht
    /// neu. Wie bei softbuffer (dem bisherigen, am echten Fenster erprobten
    /// Weg) folgt die Schicht der Wurzelschicht der Ansicht: Rahmen = deren
    /// bounds, Massstab = deren contentsScale. Der Puffer hat die Bildpunkte
    /// von winit (`ww` x `wh`) - danach rechnen Oberflaeche und Bildrechteck.
    /// Passt beides einmal nicht genau zusammen, streckt CoreAnimation den
    /// Puffer auf den Rahmen: das Bild deckt die Ansicht immer ganz, ist nie
    /// zu gross und nie abgeschnitten. Die Oberflaechentextur entsteht beim
    /// naechsten Upload in der neuen Groesse. 0x0 (minimiert): nichts tun.
    /// Billig, wenn sich nichts geaendert hat - main.rs ruft es je Zeichnung.
    pub fn groesse(&mut self, ww: u32, wh: u32) -> Result<(), String> {
        if ww == 0 || wh == 0 {
            return Ok(());
        }
        let (Some(wurzel), Some(schicht)) = (&self.wurzel, &self.schicht) else { return Err("keine Schicht".into()) };
        let (skala, rahmen) = unsafe { (msg_f64(wurzel.0, c"contentsScale"), msg_rect(wurzel.0, c"bounds")) };
        let skala = if skala > 0.0 { skala } else { 1.0 };
        // Noch ohne Rahmen (Wurzelschicht nicht ausgelegt): aus den
        // Bildpunkten und dem Massstab.
        let rahmen = if rahmen.w > 0.0 && rahmen.h > 0.0 {
            rahmen
        } else {
            CgRect { x: 0.0, y: 0.0, w: ww as f64 / skala, h: wh as f64 / skala }
        };
        if (ww, wh, skala, rahmen) == (self.breite, self.hoehe, self.skala, self.rahmen) {
            return Ok(());
        }
        let _pool = Pool::neu();
        unsafe {
            ohne_animation(|| {
                senden!(schicht.0, sel(c"setContentsScale:"), skala => f64; -> ());
                senden!(schicht.0, sel(c"setFrame:"), rahmen => CgRect; -> ());
                senden!(schicht.0, sel(c"setDrawableSize:"), CgSize { w: ww as f64, h: wh as f64 } => CgSize; -> ());
            });
        }
        if (ww, wh) != (self.breite, self.hoehe) {
            self.oberflaeche = None;
        }
        self.breite = ww;
        self.hoehe = wh;
        self.skala = skala;
        self.rahmen = rahmen;
        Ok(())
    }

    /// winit meldet das Fenster verdeckt (oder nicht mehr): verdeckt wird
    /// nicht gezeichnet - CoreAnimation zeigt dort nichts, und nextDrawable
    /// koennte bis zu einer Sekunde warten. true: das Fenster ist wieder
    /// sichtbar, und seit dem letzten Bild fiel ein Praesentieren aus - das
    /// letzte Bild muss neu praesentiert werden. Es wurde verdeckt zwar
    /// hochgeladen, aber nie gezeigt; die Schicht zeigt beim Aufdecken
    /// weiter, was vor dem Verdecken dort stand - und bei stillem Bildschirm
    /// schickt der Host kein neues Bild, das es ersetzen wuerde.
    pub fn verdeckt(&mut self, v: bool) -> bool {
        let nachholen = !v && (self.verdeckt || self.ausgelassen);
        self.verdeckt = v;
        nachholen
    }

    /// Sieht AppKit das Fenster (occlusionState mit Visible)? Ein Fenster,
    /// das schon verdeckt aufgeht - gesperrter Bildschirm, Ruhezustand, ein
    /// virtueller Bildschirm ohne Zuschauer -, meldet winit nie als verdeckt:
    /// Occluded kommt nur bei einer Aenderung. Deshalb fragt `zeichnen`
    /// selbst nach. Ohne Ansicht (Tests) immer ja.
    pub fn fenster_sichtbar(&self) -> bool {
        let Some(ansicht) = &self.ansicht else { return true };
        unsafe {
            let fenster = msg_id(ansicht.0, c"window");
            !fenster.is_null() && msg_zahl(fenster, c"occlusionState") & FENSTER_SICHTBAR != 0
        }
    }

    /// Praesentiert die Schicht gerade in EDR? Noch nie: sie ist immer SDR
    /// (BGRA8), siehe HDR_DARSTELLUNG.
    pub fn hdr_praesentiert(&self) -> bool {
        false
    }

    /// Wie viele praesentierte Bilder kamen auf dem Schirm an? Gezaehlt in
    /// `bereit`, an presentedTime.
    pub fn gezeigt(&self) -> u64 {
        self.gezeigt
    }

    /// Waechter: Metal praesentiert am sichtbaren Fenster, aber seit
    /// WAECHTER_FRIST und WAECHTER_VERSUCHE Versuchen kam nie ein Bild auf
    /// dem Schirm an - die Schicht haengt nicht sichtbar an der Ansicht, oder
    /// nextDrawable liefert nichts. Die Oberflaeche liegt in derselben
    /// Schicht, auch der Knopf "Prozessor" waere nicht zu sehen; main.rs
    /// zeichnet dann mit softbuffer weiter.
    pub fn nie_auf_dem_schirm(&self) -> bool {
        nie_gezeigt(self.gezeigt, self.versuche, self.erster_versuch.map(|t| t.elapsed()), WAECHTER_VERSUCHE, WAECHTER_FRIST)
    }

    /// Ist die Schicht bereit fuer ein weiteres Bild, ohne dass nextDrawable
    /// warten muesste? Zaehlt die praesentierten Bilder, die noch nicht auf
    /// dem Schirm sind - nie warten, der Fensterfaden traegt auch die
    /// Eingabe. Ohne Schicht immer ja.
    pub fn bereit(&mut self) -> bool {
        let jetzt = Instant::now();
        let mut neu_gezeigt = 0;
        self.praesentiert.retain(|(d, seit)| {
            let gezeigt = unsafe { msg_f64(d.0, c"presentedTime") };
            if gezeigt > 0.0 {
                neu_gezeigt += 1;
            }
            gezeigt <= 0.0 && jetzt.duration_since(*seit) < PRAESENT_FRIST
        });
        self.gezeigt += neu_gezeigt;
        self.praesentiert.len() < HOECHSTENS_UNTERWEGS
    }

    /// Ist das Geraet weg? Dann der Grund.
    pub fn geraet_weg(&self) -> Option<String> {
        self.weg.clone()
    }

    /// Stufe 2 in den naechsten Puffer der Schicht und praesentieren.
    /// `sofort`: ohne auf den Bildwechsel zu warten (displaySyncEnabled
    /// aus) - main.rs verlangt das nur, wenn `tearing` es erlaubt, und das
    /// ist auf dem Mac nie der Fall.
    pub fn zeichnen(&mut self, rect: Option<(i32, i32, u32, u32)>, ui_an: bool, sofort: bool) -> Praesentiert {
        let _pool = Pool::neu();
        self.aufraeumen();
        if let Some(grund) = self.geraet_weg() {
            return Praesentiert::GeraetWeg(grund);
        }
        let Some(schicht) = self.schicht.as_ref().map(|s| s.0) else { return Praesentiert::Fehler("keine Schicht".into()) };
        if self.verdeckt || !self.fenster_sichtbar() {
            self.ausgelassen = true;
            return Praesentiert::Verdeckt;
        }
        // Waechter: jeder Versuch am sichtbaren Fenster zaehlt, auch ein
        // gescheiterter - bis das erste Bild auf dem Schirm ankommt.
        if self.gezeigt == 0 {
            self.versuche = self.versuche.saturating_add(1);
            self.erster_versuch.get_or_insert_with(Instant::now);
        }
        let sofort = sofort && self.tearing;
        if sofort != self.sync_aus {
            unsafe { msg_void_bool(schicht, c"setDisplaySyncEnabled:", !sofort) };
            self.sync_aus = sofort;
        }
        let Some(drawable) = (unsafe { Obj::halten(msg_id(schicht, c"nextDrawable")) }) else {
            return Praesentiert::Fehler("nextDrawable lieferte nichts".into());
        };
        let ziel = unsafe { msg_id(drawable.0, c"texture") };
        match self.stufe2(ziel, self.breite, self.hoehe, rect, ui_an, Some(drawable.0)) {
            Ok(_) => {
                self.praesentiert.push_back((drawable, Instant::now()));
                self.ausgelassen = false;
                Praesentiert::Ok
            }
            Err(e) => Praesentiert::Fehler(e),
        }
    }

    /// Ein rohes Decoderbild: seine Ebenen als Texturen (ohne Kopie), Stufe 1
    /// in die Zwischentextur. Das Bild darf danach fallen - die Anzeige haelt
    /// den Puffer selbst, bis die Karte ihn gelesen hat.
    pub fn bild_roh(&mut self, bild: &vt_decoder::Bild) -> Result<(), String> {
        self.bild_roh_puffer(bild).map(|_| ())
    }

    /// `bild_roh` mit dem Befehlspuffer von Stufe 1 (fuer die Zeitmessung).
    fn bild_roh_puffer(&mut self, bild: &vt_decoder::Bild) -> Result<Obj, String> {
        let _pool = Pool::neu();
        self.aufraeumen();
        let format = bild.format();
        let Some(fmt) = vt_decoder::ebenen(format) else {
            return Err(format!("Unbekanntes Bildformat vom Decoder: {}", vt_decoder::fourcc_text(format)));
        };
        let (w, h) = (bild.breite(), bild.hoehe());
        if w == 0 || h == 0 {
            return Err("Decoder liefert ein leeres Bild".into());
        }
        if !fmt.paar || bild.ebenenzahl() < 2 {
            return Err("Decoder liefert zu wenige Bildebenen".into());
        }
        let (cw, ch) = if fmt.sub { ((w + 1) / 2, (h + 1) / 2) } else { (w, h) };
        let (fy, fuv) = if fmt.bits == 8 { (R8_UINT, RG8_UINT) } else { (R16_UINT, RG16_UINT) };
        let pb = bild.puffer();
        let texturen = unsafe {
            let passt = cf::CVPixelBufferGetWidthOfPlane(pb, 0) >= w as usize
                && cf::CVPixelBufferGetHeightOfPlane(pb, 0) >= h as usize
                && cf::CVPixelBufferGetWidthOfPlane(pb, 1) >= cw as usize
                && cf::CVPixelBufferGetHeightOfPlane(pb, 1) >= ch as usize;
            if !passt {
                return Err("Bildebenen des Decoders sind zu klein".into());
            }
            self.ebene(pb, fy, w, h, 0).and_then(|y| Ok((y, self.ebene(pb, fuv, cw, ch, 1)?)))
        };
        let (ty, tuv) = match texturen {
            Ok(t) => t,
            Err(e) => {
                // Ohne IOSurface (oder was der Cache sonst ablehnt): dasselbe
                // Bild auf der CPU umrechnen und fertig hochladen. Langsamer,
                // aber das Bild steht.
                if !self.cache_rueckfall {
                    protokoll::zeile(format!("Anzeige: {e} - Umrechnung auf der CPU"));
                    self.cache_rueckfall = true;
                }
                let f = crate::to_rgb(bild)?;
                return self.bild_rgb_puffer(&f);
            }
        };
        let stand = (format, w, h);
        if self.ebenen_stand != Some(stand) {
            protokoll::zeile(format!(
                "Ebenen neu: {} {w}x{h}, Farbebenen {cw}x{ch}, {} Bit, {} Wertebereich, Zeilen {}/{} Byte, ohne Kopie (IOSurface)",
                vt_decoder::fourcc_text(format),
                fmt.bits,
                if fmt.begrenzt { "begrenzter" } else { "voller" },
                bild.zeilenlaenge(0),
                bild.zeilenlaenge(1)
            ));
            self.ebenen_stand = Some(stand);
        }
        self.zwischen_sichern(w, h)?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?.tex.0;
        let k = KonstFormat { sub: fmt.sub as u32, schieb: fmt.schieb(), begrenzt: fmt.begrenzt as u32, _f: 0 };
        unsafe {
            let cb = self.befehlspuffer()?;
            let enc = malen(cb.0, z)?;
            msg_void_1(enc, c"setRenderPipelineState:", self.stufe1.0);
            senden!(enc, sel(c"setFragmentBytes:length:atIndex:"),
                &k as *const KonstFormat as *const c_void => *const c_void, std::mem::size_of::<KonstFormat>() => usize, 0usize => usize; -> ());
            textur_binden(enc, CVMetalTextureGetTexture(ty.0), 0);
            textur_binden(enc, CVMetalTextureGetTexture(tuv.0), 1);
            dreieck(enc);
            // Eigener Griff auf den Puffer: er bleibt beim Decoder
            // ausgeliehen, bis die Karte ihn gelesen hat.
            cf::CFRetain(pb as cf::CFTypeRef);
            Ok(self.abschicken(cb, vec![ty, tuv, Cf(pb)]))
        }
    }

    /// Eine Ebene des Puffers als Textur aus dem Cache.
    unsafe fn ebene(&self, pb: cf::CVPixelBufferRef, format: usize, w: u32, h: u32, ebene: usize) -> Result<Cf, String> {
        let mut t: *mut c_void = std::ptr::null_mut();
        let st = CVMetalTextureCacheCreateTextureFromImage(
            std::ptr::null(), self.cache.0, pb, std::ptr::null(), format, w as usize, h as usize, ebene, &mut t,
        );
        if st != 0 || t.is_null() {
            return Err(format!("CVMetalTextureCacheCreateTextureFromImage Ebene {ebene}: CVReturn {st}"));
        }
        let t = Cf(t);
        if CVMetalTextureGetTexture(t.0).is_null() {
            return Err(format!("CVMetalTextureGetTexture Ebene {ebene} lieferte nichts"));
        }
        Ok(t)
    }

    /// Ein fertiges RGB-Bild (Rueckfall, dunkles Bild): direkt in die
    /// Zwischentextur. 0x00RRGGBB liegt little-endian als B, G, R, 0 - genau
    /// der Aufbau der Zwischentextur; Stufe 2 liest das vierte Byte nicht.
    pub fn bild_rgb(&mut self, f: &Frame) -> Result<(), String> {
        self.bild_rgb_puffer(f).map(|_| ())
    }

    fn bild_rgb_puffer(&mut self, f: &Frame) -> Result<Obj, String> {
        let _pool = Pool::neu();
        self.aufraeumen();
        let (w, h) = (f.width, f.height);
        if w == 0 || h == 0 || f.pixels.len() < (w as usize) * (h as usize) {
            return Err("RGB-Bild ist leer oder zu klein".into());
        }
        self.zwischen_sichern(w, h)?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?.tex.zweiter();
        self.hochladen(z.0, &f.pixels, w as usize, (0, 0, w, h))
    }

    /// Die Oberflaeche (0xTTRRGGBB, Zielgroesse) in ihre Textur laden - nur
    /// den Kasten, den die Canvas beschrieben hat. Ist die Textur neu (erste
    /// Zeichnung, andere Groesse), alles.
    pub fn oberflaeche_hochladen(&mut self, puffer: &[u32], ww: u32, wh: u32, kasten: ui::Rect) -> Result<(), String> {
        let _pool = Pool::neu();
        self.aufraeumen();
        if ww == 0 || wh == 0 || puffer.len() < (ww as usize) * (wh as usize) {
            return Err("Oberflaechenpuffer passt nicht zur Groesse".into());
        }
        let neu = self.oberflaeche.as_ref().map(|t| (t.w, t.h)) != Some((ww, wh));
        if neu {
            self.oberflaeche = None;
            let tex = unsafe { textur_neu(self.device.0, ww, wh, RGBA8_UINT, NUTZUNG_LESEN)? };
            self.oberflaeche = Some(Textur { tex, w: ww, h: wh });
        }
        let t = self.oberflaeche.as_ref().ok_or("Oberflaechentextur fehlt")?.tex.zweiter();
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
        self.hochladen(t.0, puffer, ww as usize, (x0, y0, x1 - x0, y1 - y0)).map(|_| ())
    }

    /// Ein Ausschnitt (x, y, Breite, Hoehe) aus `quelle` (Zeilen zu
    /// `zeile` Bildpunkten) an dieselbe Stelle der Textur: ueber einen
    /// Zwischenpuffer aus dem Vorrat und einen Blit im Befehlsstrom.
    fn hochladen(&mut self, ziel: Id, quelle: &[u32], zeile: usize, (x0, y0, bw, bh): (u32, u32, u32, u32)) -> Result<Obj, String> {
        let (x0, y0, bw, bh) = (x0 as usize, y0 as usize, bw as usize, bh as usize);
        if quelle.len() < (y0 + bh - 1) * zeile + x0 + bw {
            return Err("Quelle kleiner als der Ausschnitt".into());
        }
        let laenge = bw * bh * 4;
        let i = self.zwischenpuffer(laenge)?;
        unsafe {
            let puffer = self.vorrat[i].puffer.zweiter();
            let p = msg_id(puffer.0, c"contents") as *mut u32;
            if p.is_null() {
                return Err("Zwischenpuffer ohne Inhalt".into());
            }
            for z in 0..bh {
                std::ptr::copy_nonoverlapping(quelle.as_ptr().add((y0 + z) * zeile + x0), p.add(z * bw), bw);
            }
            let cb = self.befehlspuffer()?;
            let blit = msg_id(cb.0, c"blitCommandEncoder");
            if blit.is_null() {
                return Err("blitCommandEncoder lieferte nichts".into());
            }
            senden!(blit, sel(c"copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:"),
                puffer.0 => Id, 0usize => usize, bw * 4 => usize, laenge => usize, MtlSize { w: bw, h: bh, d: 1 } => MtlSize,
                ziel => Id, 0usize => usize, 0usize => usize, MtlOrigin { x: x0, y: y0, z: 0 } => MtlOrigin; -> ());
            msg_void(blit, c"endEncoding");
            let cb = self.abschicken(cb, Vec::new());
            self.vorrat[i].belegt = Some(cb.zweiter());
            Ok(cb)
        }
    }

    /// Ein Zwischenpuffer von mindestens `laenge` Byte, den kein laufender
    /// Befehlspuffer mehr liest: aus dem Vorrat, sonst neu. Ein frischer
    /// Puffer kostet bei 1440p Millisekunden (neue Seiten), ein gebrauchter
    /// nur das Kopieren. Hoechstens drei bleiben liegen.
    fn zwischenpuffer(&mut self, laenge: usize) -> Result<usize, String> {
        let frei = |z: &Zwischenpuffer| z.belegt.as_ref().is_none_or(|cb| unsafe { msg_zahl(cb.0, c"status") } >= STATUS_FERTIG);
        for z in self.vorrat.iter_mut() {
            if frei(z) {
                z.belegt = None;
            }
        }
        if let Some(i) = self.vorrat.iter().position(|z| z.belegt.is_none() && z.laenge >= laenge) {
            return Ok(i);
        }
        if self.vorrat.len() >= 3 {
            if let Some(i) = self.vorrat.iter().position(|z| z.belegt.is_none()) {
                self.vorrat.remove(i);
            }
        }
        let puffer = unsafe { senden!(self.device.0, sel(c"newBufferWithLength:options:"), laenge => usize, PUFFER_GETEILT => usize; -> Id) };
        let puffer = unsafe { Obj::eigen(puffer) }.ok_or_else(|| format!("newBufferWithLength {laenge} lieferte nichts"))?;
        self.vorrat.push(Zwischenpuffer { puffer, laenge, belegt: None });
        Ok(self.vorrat.len() - 1)
    }

    /// Stufe 2 in ein Ziel (ww x wh): Grundfarbe, Bild im Rechteck, darueber
    /// die Oberflaeche; mit `drawable` wird es danach praesentiert. Das Ziel
    /// ist ein Puffer der Schicht (`zeichnen`) oder eine Testtextur.
    fn stufe2(&mut self, ziel: Id, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool, drawable: Option<Id>) -> Result<Obj, String> {
        if ziel.is_null() {
            return Err("Ziel ohne Textur".into());
        }
        let bild = self.zwischen.as_ref();
        let (rect, bild_da) = match (rect, bild) {
            (Some(r), Some(_)) if r.2 > 0 && r.3 > 0 => (r, true),
            _ => ((0, 0, 0, 0), false),
        };
        let (fw, fh) = bild.map(|z| (z.w, z.h)).unwrap_or((1, 1));
        // Derselbe Schritt wie in blit: (fw << 16) / zw, abgeschnitten.
        let schritt = if bild_da {
            [(((fw as u64) << 16) / rect.2 as u64) as u32, (((fh as u64) << 16) / rect.3 as u64) as u32]
        } else {
            [0, 0]
        };
        let ui = self.oberflaeche.as_ref().filter(|t| ui_an && (t.w, t.h) == (ww, wh));
        let k = KonstAnzeige {
            rect: [rect.0, rect.1, rect.2 as i32, rect.3 as i32],
            bild: [fw, fh],
            schritt,
            modus: (bild_da as u32) | ((ui.is_some() as u32) << 1),
            grund: ui::BG,
            _a: [0; 2],
        };
        let video = bild.map(|z| z.tex.0).unwrap_or(self.leer.0);
        let oberflaeche = ui.map(|t| t.tex.0).unwrap_or(self.leer.0);
        unsafe {
            let cb = self.befehlspuffer()?;
            let enc = malen(cb.0, ziel)?;
            msg_void_1(enc, c"setRenderPipelineState:", self.stufe2.0);
            senden!(enc, sel(c"setFragmentBytes:length:atIndex:"),
                &k as *const KonstAnzeige as *const c_void => *const c_void, std::mem::size_of::<KonstAnzeige>() => usize, 0usize => usize; -> ());
            textur_binden(enc, video, 0);
            textur_binden(enc, oberflaeche, 1);
            dreieck(enc);
            if let Some(d) = drawable {
                msg_void_1(cb.0, c"presentDrawable:", d);
            }
            Ok(self.abschicken(cb, Vec::new()))
        }
    }

    /// Ein neuer Befehlspuffer der Warteschlange (mit eigenem Griff).
    unsafe fn befehlspuffer(&self) -> Result<Obj, String> {
        Obj::halten(msg_id(self.queue.0, c"commandBuffer")).ok_or_else(|| "commandBuffer lieferte nichts".into())
    }

    /// Abschicken und merken, bis er fertig ist - samt allem, was so lange
    /// leben muss. Gibt einen zweiten Griff zurueck (Zeitmessung, Tests).
    fn abschicken(&mut self, cb: Obj, halten: Vec<Cf>) -> Obj {
        unsafe { msg_void(cb.0, c"commit") };
        let zweiter = cb.zweiter();
        self.unterwegs.push_back(Unterwegs { cb, _halten: halten });
        zweiter
    }

    /// Fertige Befehlspuffer loslassen (und mit ihnen die Puffer des
    /// Decoders), Fehler ins Protokoll. Fragt nur ab, wartet nicht - ausser
    /// die Karte ist so weit hinten, dass sich Befehlspuffer haeufen.
    fn aufraeumen(&mut self) {
        while self.unterwegs.len() > HOECHSTENS_BEFEHLSPUFFER {
            if let Some(u) = self.unterwegs.front() {
                unsafe { msg_void(u.cb.0, c"waitUntilCompleted") };
            }
            self.einer_fertig();
        }
        while let Some(u) = self.unterwegs.front() {
            if unsafe { msg_zahl(u.cb.0, c"status") } < STATUS_FERTIG {
                break;
            }
            self.einer_fertig();
        }
        unsafe { CVMetalTextureCacheFlush(self.cache.0, 0) };
    }

    /// Den aeltesten Befehlspuffer loslassen; war er fehlerhaft, das ins
    /// Protokoll - denselben Fehler nur einmal.
    fn einer_fertig(&mut self) {
        let Some(u) = self.unterwegs.pop_front() else { return };
        unsafe {
            if msg_zahl(u.cb.0, c"status") != STATUS_FEHLER {
                return;
            }
            let err = msg_id(u.cb.0, c"error");
            let text = fehlertext(err);
            if !err.is_null() && senden!(err, sel(c"code"); -> isize) == FEHLER_GERAET_WEG {
                self.weg = Some(text.clone());
            }
            if self.letzter_fehler.as_deref() != Some(text.as_str()) {
                protokoll::zeile(format!("Anzeige: Befehlspuffer gescheitert: {text}"));
                self.letzter_fehler = Some(text);
            }
        }
    }

    /// Zwischentextur in Bildgroesse, neu nur, wenn die Groesse nicht stimmt.
    fn zwischen_sichern(&mut self, w: u32, h: u32) -> Result<(), String> {
        if self.zwischen.as_ref().map(|z| (z.w, z.h)) == Some((w, h)) {
            return Ok(());
        }
        self.zwischen = None;
        let tex = unsafe { textur_neu(self.device.0, w, h, RGBA8_UINT, NUTZUNG_LESEN | NUTZUNG_ZIEL)? };
        self.zwischen = Some(Textur { tex, w, h });
        Ok(())
    }

    /// Eine Textur in den Hauptspeicher lesen (Blit in einen geteilten
    /// Puffer, dann warten), als 0x00RRGGBB - das vierte Byte maskiert. Nur
    /// fuer die Tests.
    fn auslesen(&mut self, tex: Id, w: u32, h: u32) -> Result<Vec<u32>, String> {
        let (w, h) = (w as usize, h as usize);
        let laenge = w * h * 4;
        let mut aus = vec![0u32; w * h];
        unsafe {
            let puffer = senden!(self.device.0, sel(c"newBufferWithLength:options:"), laenge => usize, PUFFER_GETEILT => usize; -> Id);
            let puffer = Obj::eigen(puffer).ok_or("newBufferWithLength lieferte nichts")?;
            let cb = self.befehlspuffer()?;
            let blit = msg_id(cb.0, c"blitCommandEncoder");
            if blit.is_null() {
                return Err("blitCommandEncoder lieferte nichts".into());
            }
            senden!(blit, sel(c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:"),
                tex => Id, 0usize => usize, 0usize => usize, MtlOrigin { x: 0, y: 0, z: 0 } => MtlOrigin, MtlSize { w, h, d: 1 } => MtlSize,
                puffer.0 => Id, 0usize => usize, w * 4 => usize, laenge => usize; -> ());
            msg_void(blit, c"endEncoding");
            let cb = self.abschicken(cb, Vec::new());
            msg_void(cb.0, c"waitUntilCompleted");
            if msg_zahl(cb.0, c"status") == STATUS_FEHLER {
                return Err(format!("Auslesen: {}", fehlertext(msg_id(cb.0, c"error"))));
            }
            let p = msg_id(puffer.0, c"contents") as *const u32;
            if p.is_null() {
                return Err("Lesepuffer ohne Inhalt".into());
            }
            std::ptr::copy_nonoverlapping(p, aus.as_mut_ptr(), w * h);
        }
        self.aufraeumen();
        for p in aus.iter_mut() {
            *p &= 0x00ff_ffff;
        }
        Ok(aus)
    }

    /// Nur Test: Stufe 2 in eine eigene Zieltextur (wie ein Puffer der
    /// Schicht: BGRA8), dann zurueck in den Hauptspeicher.
    pub fn offscreen(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<Vec<u32>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        let _pool = Pool::neu();
        let ziel = unsafe { textur_neu(self.device.0, ww, wh, BGRA8_UNORM, NUTZUNG_ZIEL)? };
        self.stufe2(ziel.0, ww, wh, rect, ui_an, None)?;
        self.auslesen(ziel.0, ww, wh)
    }

    /// Nur Test: das Ergebnis von Stufe 1, 1:1.
    pub fn zwischen_auslesen(&mut self) -> Result<(Vec<u32>, u32, u32), String> {
        let _pool = Pool::neu();
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        let (tex, w, h) = (z.tex.zweiter(), z.w, z.h);
        Ok((self.auslesen(tex.0, w, h)?, w, h))
    }
}

impl Drop for Gpu {
    /// Erst auf alle Befehlspuffer warten (sie lesen noch Puffer des
    /// Decoders und schreiben in Texturen, die gleich fallen), dann die
    /// Schicht aus der Ansicht nehmen - ein Neubau am selben Fenster findet
    /// sie so frei.
    fn drop(&mut self) {
        let _pool = Pool::neu();
        for u in &self.unterwegs {
            unsafe { msg_void(u.cb.0, c"waitUntilCompleted") };
        }
        self.unterwegs.clear();
        self.praesentiert.clear();
        if let Some(s) = self.schicht.take() {
            unsafe { ohne_animation(|| msg_void(s.0, c"removeFromSuperlayer")) };
        }
    }
}

/// Eine private Textur der Karte.
unsafe fn textur_neu(device: Id, w: u32, h: u32, format: usize, nutzung: usize) -> Result<Obj, String> {
    let d = senden!(klasse(c"MTLTextureDescriptor"), sel(c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
        format => usize, w as usize => usize, h as usize => usize, 0u8 => u8; -> Id);
    if d.is_null() {
        return Err(format!("MTLTextureDescriptor {w}x{h} Format {format} lieferte nichts"));
    }
    msg_void_zahl(d, c"setUsage:", nutzung);
    msg_void_zahl(d, c"setStorageMode:", SPEICHER_PRIVAT);
    Obj::eigen(msg_id_1(device, c"newTextureWithDescriptor:", d)).ok_or_else(|| format!("newTextureWithDescriptor {w}x{h} Format {format} lieferte nichts"))
}

/// Ein Zeichengang in `ziel`: alles wird ueberschrieben (Laden egal), das
/// Ergebnis bleibt (Speichern). Gibt den Encoder zurueck (autoreleased).
unsafe fn malen(cb: Id, ziel: Id) -> Result<Id, String> {
    let pass = msg_id(klasse(c"MTLRenderPassDescriptor"), c"renderPassDescriptor");
    let farben = msg_id(pass, c"colorAttachments");
    let a0 = senden!(farben, sel(c"objectAtIndexedSubscript:"), 0usize => usize; -> Id);
    if a0.is_null() {
        return Err("MTLRenderPassDescriptor ohne Farbziel".into());
    }
    msg_void_1(a0, c"setTexture:", ziel);
    msg_void_zahl(a0, c"setLoadAction:", LADEN_EGAL);
    msg_void_zahl(a0, c"setStoreAction:", SPEICHERN);
    let enc = msg_id_1(cb, c"renderCommandEncoderWithDescriptor:", pass);
    if enc.is_null() {
        return Err("renderCommandEncoderWithDescriptor lieferte nichts".into());
    }
    Ok(enc)
}

unsafe fn textur_binden(enc: Id, tex: Id, platz: usize) {
    senden!(enc, sel(c"setFragmentTexture:atIndex:"), tex => Id, platz => usize; -> ());
}

/// Das Dreieck ueber das ganze Ziel, dann den Encoder schliessen.
unsafe fn dreieck(enc: Id) {
    senden!(enc, sel(c"drawPrimitives:vertexStart:vertexCount:"), DREIECKE => usize, 0usize => usize, 3usize => usize; -> ());
    msg_void(enc, c"endEncoding");
}

// ------------------------------------------------------------------ Anzeigetest

/// Die Formate, die VideoToolbox liefern kann (vt_decoder::ausgabeformat).
const FORMATE: [u32; 8] = [
    vt_decoder::XF44,
    vt_decoder::X444,
    vt_decoder::F444,
    vt_decoder::V444,
    vt_decoder::XF20,
    vt_decoder::X420,
    vt_decoder::F420,
    vt_decoder::V420,
];

/// Ein Probebild in einem CVPixelBuffer mit IOSurface, wie VideoToolbox ihn
/// liefert: das Muster aus anzeigeprobe.rs in beiden Ebenen (U und V als
/// Paare). Die 16-Bit-Formate bekommen volle 16 Bit (auch die unteren sechs,
/// die bei 10 Bit null sind) - so prueft der Test das Abschneiden auf die
/// acht Anzeigebits mit.
fn probebild(format: u32, w: u32, h: u32) -> Result<vt_decoder::Bild, String> {
    let fmt = vt_decoder::ebenen(format).ok_or_else(|| format!("Format {} unbekannt", vt_decoder::fourcc_text(format)))?;
    let max = if fmt.bits == 8 { 255 } else { 65535 };
    let (cw, ch) = if fmt.sub { ((w + 1) / 2, (h + 1) / 2) } else { (w, h) };
    let bpp = fmt.bpp() as usize;
    unsafe {
        let attribute = cf::woerterbuch();
        cf::setzen_und_freigeben(attribute, cf::kCVPixelBufferIOSurfacePropertiesKey, cf::woerterbuch() as cf::CFTypeRef);
        cf::CFDictionarySetValue(attribute, cf::kCVPixelBufferMetalCompatibilityKey, cf::kCFBooleanTrue);
        let mut pb: cf::CVPixelBufferRef = std::ptr::null_mut();
        let st = cf::CVPixelBufferCreate(std::ptr::null(), w as usize, h as usize, format, attribute as cf::CFDictionaryRef, &mut pb);
        cf::CFRelease(attribute as cf::CFTypeRef);
        if st != 0 || pb.is_null() {
            return Err(format!("CVPixelBufferCreate {} {w}x{h}: CVReturn {st}", vt_decoder::fourcc_text(format)));
        }
        if cf::CVPixelBufferLockBaseAddress(pb, 0) != 0 {
            cf::CFRelease(pb as cf::CFTypeRef);
            return Err("CVPixelBufferLockBaseAddress".into());
        }
        let schreib = |d: *mut u8, i: usize, v: u32| {
            if bpp == 1 {
                *d.add(i) = v as u8;
            } else {
                std::ptr::copy_nonoverlapping((v as u16).to_le_bytes().as_ptr(), d.add(i), 2);
            }
        };
        let (d0, z0) = (cf::CVPixelBufferGetBaseAddressOfPlane(pb, 0) as *mut u8, cf::CVPixelBufferGetBytesPerRowOfPlane(pb, 0));
        let (d1, z1) = (cf::CVPixelBufferGetBaseAddressOfPlane(pb, 1) as *mut u8, cf::CVPixelBufferGetBytesPerRowOfPlane(pb, 1));
        let (w, h, cw, ch) = (w as usize, h as usize, cw as usize, ch as usize);
        for y in 0..h {
            for x in 0..w {
                schreib(d0, y * z0 + x * bpp, crate::anzeigeprobe::probewert(x, y, w, h, 0, max));
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                schreib(d1, y * z1 + 2 * x * bpp, crate::anzeigeprobe::probewert(x, y, cw, ch, 1, max));
                schreib(d1, y * z1 + (2 * x + 1) * bpp, crate::anzeigeprobe::probewert(x, y, cw, ch, 2, max));
            }
        }
        cf::CVPixelBufferUnlockBaseAddress(pb, 0);
        vt_decoder::Bild::aus_puffer(pb, 0).map_err(|e| e.to_string())
    }
}

/// Ein Format in einer Groesse: (a) `to_rgb` als Referenz, (b) rohes Bild
/// ueber Stufe 1 gegen die Referenz, dazu der RGB-Upload, (c) skaliert in
/// 700x400 und pixelgenau gegen `blit` - alles Toleranz 0: Stufe 2 mischt
/// ganzzahlig wie `mische`, (d) Oberflaeche ueber dem skalierten Bild:
/// CPU-"over" Schicht fuer Schicht gegen Bild * T/255 + Farbe im Shader
/// (Toleranz 2, wie unter Windows - die Rundung der einzelnen Schichten
/// laesst sich in einer Summe nicht nachbilden). `verzeichnis`: wohin die
/// Differenzbilder gehen (None: keine, fuer die Tests).
fn format_pruefen(gpu: &mut Gpu, format: u32, w: u32, h: u32, verzeichnis: Option<&str>, lang: &'static crate::strings::Lang, u: &mut ui::Ui) -> bool {
    use crate::anzeigeprobe::{fall_pruefen, oberflaeche_probe};
    let name = vt_decoder::fourcc_text(format);
    let bild = match probebild(format, w, h) {
        Ok(b) => b,
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} kein Probebild: {e}");
            return false;
        }
    };
    let referenz = match crate::to_rgb(&bild) {
        Ok(f) => f,
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} to_rgb: {e}");
            return false;
        }
    };
    println!("{name:<15} {w:>4}x{h:<4} Zeilen {}/{} Byte", bild.zeilenlaenge(0), bild.zeilenlaenge(1));
    let mut ok = true;

    match gpu.bild_roh(&bild).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, zw, zh)) if (zw, zh) == (w, h) => {
            ok &= fall_pruefen(&name, w, h, "1:1", &referenz.pixels, &aus, w, h, 0, verzeichnis, lang);
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
    // Das Bild darf fallen, bevor die Karte fertig ist: die Anzeige haelt
    // den Puffer selbst.
    drop(bild);

    let (bw, bh) = (700u32, 400u32);
    let n = (bw as usize) * (bh as usize);
    let mut cpu = vec![0u32; n];
    crate::blit(&mut cpu, bw, bh, &referenz, false);
    let rect = crate::ziel_rechteck(bw, bh, w, h, false);
    match gpu.offscreen(bw, bh, Some(rect), false) {
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "skaliert", &cpu, &aus, bw, bh, 0, verzeichnis, lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} skaliert: {e}");
            ok = false;
        }
    }
    let mut genau = vec![0u32; n];
    crate::blit(&mut genau, bw, bh, &referenz, true);
    let rect_genau = crate::ziel_rechteck(bw, bh, w, h, true);
    match gpu.offscreen(bw, bh, Some(rect_genau), false) {
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "pixelgenau", &genau, &aus, bw, bh, 0, verzeichnis, lang),
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
        Ok(aus) => ok &= fall_pruefen(&name, w, h, "Oberflaeche", &cpu_ui, &aus, bw, bh, 2, verzeichnis, lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} Oberflaeche: {e}");
            ok = false;
        }
    }

    // Zuletzt der RGB-Upload (Rueckfall): er ersetzt die Zwischentextur.
    match gpu.bild_rgb(&referenz).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, _, _)) => ok &= fall_pruefen(&name, w, h, "RGB-Upload", &referenz.pixels, &aus, w, h, 0, verzeichnis, lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} RGB-Upload: {e}");
            ok = false;
        }
    }
    ok
}

/// Was die Messung je Bild festhaelt.
struct Messung {
    /// Zeit im Fensterfaden: Stufe 1 anstossen, Oberflaeche hochladen,
    /// Stufe 2 anstossen.
    cpu_ms: f64,
    /// Abgabe von Stufe 1 in der Uhr der Befehlspuffer.
    abgabe: f64,
    stufe1: Obj,
    stufe2: Obj,
}

/// Mittel, 99. Perzentil und Groesstes einer Reihe in ms.
fn kennzahlen(mut werte: Vec<f64>) -> (f64, f64, f64) {
    if werte.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    werte.sort_by(|a, b| a.total_cmp(b));
    let mittel = werte.iter().sum::<f64>() / werte.len() as f64;
    let p99 = werte[((werte.len() * 99).div_ceil(100)).saturating_sub(1)];
    (mittel, p99, *werte.last().unwrap_or(&0.0))
}

/// Bildzeit fuer 1440p mit 120 Hz: 240 Bilder im Takt von 8,33 ms durch
/// beide Stufen in ein Ziel von 2560x1440 (wie ein Puffer der Schicht),
/// jedes vierte mit neuer Oberflaeche (die echte wird alle 33 ms neu
/// gerastert; hier der Schleier ueber der halben Hoehe - mehr als die
/// Statistik je hochlaedt). Quelle sind Bilder aus VideoToolbox in Hardware
/// (Probe des Hardware-Encoders, HEVC 4:4:4 10), also dieselben
/// IOSurface-Puffer wie in einer Sitzung; ohne Encoder synthetische
/// xf44-Bilder. Gemessen werden die Zeit im Fensterfaden, die Zeit der
/// Karte (GPUStartTime bis GPUEndTime beider Stufen) und die Zeit von der
/// Abgabe bis zum fertigen Bild. Nicht enthalten: das Decodieren (eigener
/// Faden) und der Weg vom fertigen Bild auf den Schirm (Bildwechsel).
/// true = 99 % der Bilder sind binnen eines Takts fertig.
fn bildzeit_messen(gpu: &mut Gpu, u: &mut ui::Ui) -> bool {
    const B: u32 = 2560;
    const H: u32 = 1440;
    const BILDER: usize = 240;
    let takt = Duration::from_nanos(1_000_000_000 / 120);
    let takt_ms = takt.as_secs_f64() * 1000.0;
    let mut bilder: Vec<vt_decoder::Bild> = Vec::new();
    let mut quelle = String::new();
    match vt_decoder::probe::hevc_444_10(B, H, 4) {
        Ok(p) => {
            let mut d = vt_decoder::Decoder::neu(false, true);
            for (i, au) in p.einheiten.iter().enumerate() {
                if let Err(e) = d.fuettern(au, i as i64, &mut bilder) {
                    quelle = format!("Decodieren Paket {i}: {e}");
                    bilder.clear();
                    break;
                }
            }
        }
        Err(e) => quelle = format!("Probe: {e}"),
    }
    for z in protokoll::abholen() {
        println!("    {z}");
    }
    let quelle = match bilder.first() {
        Some(b) => format!("{} aus VideoToolbox in Hardware", vt_decoder::fourcc_text(b.format())),
        None => {
            println!("Bildzeit: {quelle} - synthetische Bilder");
            for _ in 0..4 {
                match probebild(vt_decoder::XF44, B, H) {
                    Ok(b) => bilder.push(b),
                    Err(e) => {
                        println!("Bildzeit: {e}");
                        return false;
                    }
                }
            }
            "xf44 synthetisch".into()
        }
    };
    let mut puffer = vec![0xff00_0000u32; (B * H) as usize];
    let kasten = {
        let mut c = ui::Canvas::neu(&mut puffer, B as usize, H as usize);
        crate::anzeigeprobe::oberflaeche_probe(&mut c, u, "Bildzeit 2560x1440 bei 120 Hz");
        c.kasten_nehmen().unwrap_or(ui::Rect { x: 0, y: 0, w: B as i32, h: H as i32 })
    };
    let _pool = Pool::neu();
    let ziel = match unsafe { textur_neu(gpu.device.0, B, H, BGRA8_UNORM, NUTZUNG_ZIEL) } {
        Ok(t) => t,
        Err(e) => {
            println!("Bildzeit: {e}");
            return false;
        }
    };
    let rect = crate::ziel_rechteck(B, H, B, H, false);
    let mut messungen = Vec::with_capacity(BILDER);
    let t0 = Instant::now();
    for i in 0..BILDER {
        let frist = t0 + takt * i as u32;
        let jetzt = Instant::now();
        if frist > jetzt {
            std::thread::sleep(frist - jetzt);
        }
        let _pool = Pool::neu();
        let a = Instant::now();
        let abgabe = unsafe { CACurrentMediaTime() };
        let r = gpu.bild_roh_puffer(&bilder[i % bilder.len()]).and_then(|s1| {
            if i % 4 == 0 {
                gpu.oberflaeche_hochladen(&puffer, B, H, kasten)?;
            }
            let s2 = gpu.stufe2(ziel.0, B, H, Some(rect), true, None)?;
            Ok((s1, s2))
        });
        match r {
            Ok((stufe1, stufe2)) => messungen.push(Messung { cpu_ms: a.elapsed().as_secs_f64() * 1000.0, abgabe, stufe1, stufe2 }),
            Err(e) => {
                println!("Bildzeit: Bild {i}: {e}");
                return false;
            }
        }
    }
    let gesamt = t0.elapsed().as_secs_f64();
    let (mut cpu, mut karte, mut fertig) = (Vec::new(), Vec::new(), Vec::new());
    for m in &messungen {
        unsafe {
            msg_void(m.stufe2.0, c"waitUntilCompleted");
            msg_void(m.stufe1.0, c"waitUntilCompleted");
            let zeit = |cb: &Obj| (msg_f64(cb.0, c"GPUStartTime"), msg_f64(cb.0, c"GPUEndTime"));
            let (a1, e1) = zeit(&m.stufe1);
            let (a2, e2) = zeit(&m.stufe2);
            cpu.push(m.cpu_ms);
            karte.push(((e1 - a1) + (e2 - a2)) * 1000.0);
            fertig.push((e2 - m.abgabe) * 1000.0);
        }
    }
    gpu.aufraeumen();
    let ueber = fertig.iter().filter(|&&f| f > takt_ms).count();
    let (c_m, c_p, c_x) = kennzahlen(cpu);
    let (k_m, k_p, k_x) = kennzahlen(karte);
    let (f_m, f_p, f_x) = kennzahlen(fertig);
    // "Abgabe bis fertig" beginnt vor der Arbeit im Fensterfaden und
    // enthaelt sie also schon.
    let passt = f_p <= takt_ms;
    println!("Bildzeit 1440p: {BILDER} Bilder {B}x{H} ({quelle}) im 120-Hz-Takt ({takt_ms:.2} ms), {:.2} s", gesamt);
    println!("  Fensterfaden (anstossen, hochladen):  Mittel {c_m:.2} ms, 99 % {c_p:.2} ms, hoechstens {c_x:.2} ms");
    println!("  Karte (Stufe 1 + 2):                  Mittel {k_m:.2} ms, 99 % {k_p:.2} ms, hoechstens {k_x:.2} ms");
    println!("  Abgabe bis fertig:                    Mittel {f_m:.2} ms, 99 % {f_p:.2} ms, hoechstens {f_x:.2} ms; ueber einem Takt: {ueber} von {BILDER}");
    println!(
        "  {} (ohne Decodieren und ohne den Bildwechsel bis auf den Schirm)",
        if passt { "passt in 120 Hz" } else { "passt NICHT in 120 Hz" }
    );
    passt
}

/// Goldbildtest der Anzeige ohne Fenster: Geraet, Shader, dann jedes Format
/// von VideoToolbox in zwei Groessen (257x131: ungerade Breite und Hoehe,
/// Farbebenen (w+1)/2; 1920x1080: der Regelfall) gegen den CPU-Weg, zuletzt
/// die Bildzeit bei 1440p und 120 Hz. Startet weder Host noch Aufnahme und
/// oeffnet kein Fenster. true = alle Vergleiche innerhalb der Toleranz (die
/// Bildzeit ist eine Messung und entscheidet nicht mit).
pub fn anzeigetest(verzeichnis: &str) -> bool {
    let lang = crate::strings::pick("de");
    let _ = std::fs::create_dir_all(verzeichnis);
    println!("Anzeigetest: Metal ohne Fenster, Bilder nach {verzeichnis}");
    let mut gpu = match Gpu::ohne_fenster() {
        Ok(g) => g,
        Err(e) => {
            println!("Geraet: {e}");
            for z in protokoll::abholen() {
                println!("    {z}");
            }
            return false;
        }
    };
    println!("Geraet: {} (gemeinsamer Speicher {})", gpu.adapter.name, if gpu.adapter.gemeinsam { "ja" } else { "nein" });
    for z in protokoll::abholen() {
        println!("    {z}");
    }
    let mut u = ui::Ui::new();
    u.tick = 40;
    u.mouse = (-1, -1);
    if !u.text.ok() {
        println!("Keine Schrift gefunden - die Oberflaechenprobe bleibt ohne Text");
    }
    let mut ok = true;
    for format in FORMATE {
        for (w, h) in [(257u32, 131u32), (1920, 1080)] {
            ok &= format_pruefen(&mut gpu, format, w, h, Some(verzeichnis), lang, &mut u);
            for z in protokoll::abholen() {
                println!("    {z}");
            }
        }
    }
    bildzeit_messen(&mut gpu, &mut u);
    println!("{}", if ok { "Anzeigetest bestanden" } else { "Anzeigetest NICHT bestanden" });
    ok
}

/// Stand der Schicht fuer den Selbsttest am Fenster.
struct Geometrie {
    /// Rahmen der Schicht und bounds der Wurzelschicht, in Punkten.
    rahmen: CgRect,
    wurzel: CgRect,
    /// contentsScale der Schicht und der Wurzelschicht.
    skala: f64,
    wurzel_skala: f64,
    /// drawableSize der Schicht in Bildpunkten.
    puffer: CgSize,
}

impl Gpu {
    /// Wie die Schicht gerade an der Ansicht haengt (nur Selbsttest). Ein
    /// CGSize (zwei Gleitkommazahlen) kommt auch auf Intel in Registern
    /// zurueck; nur Rechtecke brauchen `msg_rect`.
    fn geometrie(&self) -> Option<Geometrie> {
        let (wurzel, schicht) = (self.wurzel.as_ref()?, self.schicht.as_ref()?);
        unsafe {
            Some(Geometrie {
                rahmen: msg_rect(schicht.0, c"frame"),
                wurzel: msg_rect(wurzel.0, c"bounds"),
                skala: msg_f64(schicht.0, c"contentsScale"),
                wurzel_skala: msg_f64(wurzel.0, c"contentsScale"),
                puffer: senden!(schicht.0, sel(c"drawableSize"); -> CgSize),
            })
        }
    }
}

/// Unterschichten einer Schicht: wie viele, und wie viele davon
/// CAMetalLayer sind.
unsafe fn unterschichten(wurzel: Id) -> (usize, usize) {
    let unter = msg_id(wurzel, c"sublayers");
    let n = msg_zahl(unter, c"count");
    let metal = klasse(c"CAMetalLayer");
    let zahl_metal = (0..n)
        .filter(|&i| {
            let l = senden!(unter, sel(c"objectAtIndex:"), i => usize; -> Id);
            senden!(l, sel(c"isKindOfClass:"), metal => Id; -> u8) != 0
        })
        .count();
    (n, zahl_metal)
}

/// Die Schritte des Selbsttests am Fenster.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Schritt {
    Zeigen,
    Diagnose,
    Groesse,
    Verbergen,
    Aufdecken,
    Rueckfall,
    Ende,
}

/// Zustand des Selbsttests am Fenster.
struct Fensterprobe {
    fenster: Option<std::sync::Arc<winit::window::Window>>,
    /// Die NSView des Fensters (fuer den Fensterstand).
    ansicht: Id,
    gpu: Option<Gpu>,
    /// Kam ein Bild auf dem Schirm an (presentedTime)? Ohne das warten die
    /// spaeteren Schritte nur auf Geometrie und Ereignisse.
    schirm: bool,
    /// Diagnose: ein praesentiertes Bild, eigens festgehalten, und seit wann.
    festgehalten: Option<(Obj, Instant)>,
    schritt: Schritt,
    /// Beginn des Tests und des laufenden Schritts.
    start: Instant,
    seit: Instant,
    /// Gezeigte Bilder, als die Bedingung des Schritts erfuellt war - der
    /// Schritt endet mit dem naechsten Bild, das danach ankommt.
    marke: Option<u64>,
    /// Letztes Occluded von winit, und ob `verdeckt` dabei das Nachholen
    /// verlangte.
    verdeckt: Option<bool>,
    nachholen: bool,
    /// Groesse, fuer die die Oberflaeche hochgeladen ist.
    ui_masse: (u32, u32),
    fehler: Vec<String>,
    nicht_pruefbar: Option<String>,
}

/// Groesse des Probebildes; das Fenster ist groesser, das Bild wird
/// skaliert (Stufe 2 mit Mischung).
const PROBE_B: u32 = 320;
const PROBE_H: u32 = 180;

impl Fensterprobe {
    fn fehler(&mut self, text: String) {
        println!("Anzeige-Selbsttest: FEHLER {text}");
        self.fehler.push(text);
    }

    fn weiter(&mut self, schritt: Schritt) {
        for z in protokoll::abholen() {
            println!("    {z}");
        }
        self.schritt = schritt;
        self.seit = Instant::now();
        self.marke = None;
    }

    /// Ein Durchgang wie in main.rs: Groesse, Oberflaeche (bei neuer Groesse
    /// ein weisser Kasten oben links), bereit?, Stufe 2 und praesentieren.
    fn zeichnen(&mut self) {
        let (Some(w), Some(g)) = (self.fenster.as_ref(), self.gpu.as_mut()) else { return };
        let s = w.inner_size();
        if s.width == 0 || s.height == 0 {
            return;
        }
        let mut fehler = None;
        if let Err(e) = g.groesse(s.width, s.height) {
            fehler = Some(format!("groesse: {e}"));
        } else {
            if self.ui_masse != (s.width, s.height) {
                let mut puffer = vec![0xff00_0000u32; (s.width * s.height) as usize];
                for y in 16..64.min(s.height as usize) {
                    let z = y * s.width as usize;
                    puffer[z + 16..z + 64.min(s.width as usize)].fill(0x00ff_ffff);
                }
                let kasten = ui::Rect { x: 0, y: 0, w: s.width as i32, h: s.height as i32 };
                match g.oberflaeche_hochladen(&puffer, s.width, s.height, kasten) {
                    Ok(()) => self.ui_masse = (s.width, s.height),
                    Err(e) => fehler = Some(format!("Oberflaeche: {e}")),
                }
            }
            if g.bereit() {
                let rect = crate::ziel_rechteck(s.width, s.height, PROBE_B, PROBE_H, false);
                match g.zeichnen(Some(rect), true, false) {
                    Praesentiert::Ok | Praesentiert::Verdeckt => {}
                    Praesentiert::Fehler(e) | Praesentiert::GeraetWeg(e) => fehler = Some(format!("zeichnen: {e}")),
                }
            }
        }
        if let Some(e) = fehler {
            if !self.fehler.contains(&e) {
                self.fehler(e);
            }
        }
    }

    /// Stand von Fenster und Bildschirm, wie AppKit ihn sieht.
    fn fenster_stand(&self) -> String {
        unsafe {
            let fenster = msg_id(self.ansicht, c"window");
            let occ = msg_zahl(fenster, c"occlusionState");
            let schirm = msg_id(fenster, c"screen");
            let bildschirme = msg_zahl(msg_id(klasse(c"NSScreen"), c"screens"), c"count");
            format!(
                "occlusionState {occ} (sichtbar {}), isVisible {}, aktiver Space {}, Bildschirm \"{}\" bei Massstab {}, {bildschirme} Bildschirm(e)",
                occ & 2 != 0,
                msg_bool(fenster, c"isVisible"),
                msg_bool(fenster, c"isOnActiveSpace"),
                rust_text(msg_id(schirm, c"localizedName")),
                msg_f64(schirm, c"backingScaleFactor"),
            )
        }
    }

    /// Rahmen, Massstab und Puffer der Schicht gegen die Ansicht und winit.
    fn geometrie_pruefen(&mut self, wann: &str) {
        let (Some(w), Some(g)) = (self.fenster.as_ref(), self.gpu.as_ref()) else { return };
        let Some(m) = g.geometrie() else {
            self.fehler(format!("{wann}: keine Schicht"));
            return;
        };
        let (s, skala) = (w.inner_size(), w.scale_factor());
        println!(
            "Anzeige-Selbsttest: {wann}: winit {}x{} bei Massstab {skala}, Ansicht {}x{} Punkte bei {}, Schicht {}x{} bei ({}, {}) mit {}, Puffer {}x{}",
            s.width, s.height, m.wurzel.w, m.wurzel.h, m.wurzel_skala, m.rahmen.w, m.rahmen.h, m.rahmen.x, m.rahmen.y, m.skala, m.puffer.w, m.puffer.h
        );
        let mut fehler = Vec::new();
        if m.rahmen != m.wurzel {
            fehler.push(format!("{wann}: Rahmen der Schicht {:?} ist nicht der der Ansicht {:?}", m.rahmen, m.wurzel));
        }
        if m.skala != m.wurzel_skala || m.wurzel_skala != skala {
            fehler.push(format!("{wann}: Massstab Schicht {}, Ansicht {}, winit {skala}", m.skala, m.wurzel_skala));
        }
        if m.puffer != (CgSize { w: s.width as f64, h: s.height as f64 }) {
            fehler.push(format!("{wann}: Puffer {:?} statt {}x{}", m.puffer, s.width, s.height));
        }
        if ((m.wurzel.w * skala).round(), (m.wurzel.h * skala).round()) != (s.width as f64, s.height as f64) {
            fehler.push(format!("{wann}: Ansicht {}x{} Punkte passt nicht zu {}x{} Bildpunkten", m.wurzel.w, m.wurzel.h, s.width, s.height));
        }
        if fehler.is_empty() {
            println!("Anzeige-Selbsttest: ok   Schicht deckt die Ansicht, Massstab und Puffer stimmen ({wann})");
        }
        for f in fehler {
            self.fehler(f);
        }
    }

    /// Der Rueckfall: Anzeige fallen lassen, keine Metal-Schicht mehr an der
    /// Ansicht, softbuffer zeichnet am selben Fenster.
    fn rueckfall_pruefen(&mut self) {
        let (Some(w), Some(g)) = (self.fenster.clone(), self.gpu.take()) else { return };
        let Some(wurzel) = g.wurzel.as_ref().map(Obj::zweiter) else {
            self.fehler("Rueckfall: keine Wurzelschicht".into());
            return;
        };
        drop(g);
        let (_, metal) = unsafe { unterschichten(wurzel.0) };
        if metal != 0 {
            self.fehler(format!("Rueckfall: nach dem Drop haengen noch {metal} Metal-Schichten an der Ansicht"));
            return;
        }
        let s = w.inner_size();
        let ergebnis = (|| -> Result<(usize, usize), String> {
            let kontext = softbuffer::Context::new(w.clone()).map_err(|e| format!("softbuffer-Kontext: {e}"))?;
            let mut flaeche = softbuffer::Surface::new(&kontext, w.clone()).map_err(|e| format!("softbuffer-Flaeche: {e}"))?;
            let (Some(b), Some(h)) = (std::num::NonZeroU32::new(s.width), std::num::NonZeroU32::new(s.height)) else {
                return Err("Fenster ohne Groesse".into());
            };
            flaeche.resize(b, h).map_err(|e| format!("softbuffer resize: {e}"))?;
            let mut puffer = flaeche.buffer_mut().map_err(|e| format!("softbuffer buffer_mut: {e}"))?;
            puffer.fill(0x0020_4060);
            puffer.present().map_err(|e| format!("softbuffer present: {e}"))?;
            Ok(unsafe { unterschichten(wurzel.0) })
        })();
        match ergebnis {
            Ok((n, 0)) if n >= 1 => println!("Anzeige-Selbsttest: ok   Rueckfall: Metal-Schicht entfernt, softbuffer zeichnet am selben Fenster"),
            Ok((n, metal)) => self.fehler(format!("Rueckfall: {n} Unterschichten, davon {metal} Metal")),
            Err(e) => self.fehler(format!("Rueckfall: {e}")),
        }
    }

    /// Ein Takt: der laufende Schritt.
    fn takt(&mut self) {
        let dauer = self.seit.elapsed();
        if self.start.elapsed() > Duration::from_secs(30) {
            self.fehler(format!("Zeit abgelaufen in Schritt {:?}", self.schritt));
            self.weiter(Schritt::Ende);
            return;
        }
        match self.schritt {
            Schritt::Zeigen => {
                self.zeichnen();
                let Some(g) = self.gpu.as_ref() else { return self.weiter(Schritt::Ende) };
                let (gezeigt, versuche, waechter, sichtbar) = (g.gezeigt(), g.versuche, g.nie_auf_dem_schirm(), g.fenster_sichtbar());
                if gezeigt > 0 {
                    self.schirm = true;
                    println!("Anzeige-Selbsttest: ok   Bild auf dem Schirm nach {} ms ({versuche} Versuche)", dauer.as_millis());
                    if waechter {
                        self.fehler("Waechter schlaegt an, obwohl ein Bild ankam".into());
                    }
                    self.geometrie_pruefen("am Anfang");
                    self.groesse_aendern();
                } else if dauer > Duration::from_secs(5) {
                    println!("Anzeige-Selbsttest: Fenster: {}", self.fenster_stand());
                    if !sichtbar {
                        // AppKit sieht das Fenster nicht, und winit hat das
                        // nie gemeldet: dann darf nicht praesentiert und
                        // nicht gezaehlt werden - sonst schluege der
                        // Waechter hier ohne Grund an.
                        if versuche != 0 || waechter {
                            self.fehler(format!("am unsichtbaren Fenster {versuche} Versuche gezaehlt, Waechter {waechter}"));
                        } else {
                            println!("Anzeige-Selbsttest: ok   unsichtbares Fenster: nicht praesentiert, Waechter zaehlt nicht");
                        }
                        self.nicht_pruefbar = Some(
                            "AppKit meldet das Fenster als nicht sichtbar (gesperrter Bildschirm, Ruhezustand oder ein virtueller Bildschirm ohne Zuschauer) - ob Bilder ankommen und ob Verdecken und Aufdecken gemeldet werden, laesst sich so nicht pruefen".into(),
                        );
                        self.geometrie_pruefen("am Anfang");
                        self.groesse_aendern();
                    } else {
                        self.fehler(format!(
                            "nach 5 s und {versuche} Versuchen kein Bild auf dem Schirm (presentedTime blieb 0); Waechter {}",
                            if waechter { "schlaegt an - die App faellt hier auf softbuffer zurueck" } else { "still" }
                        ));
                        self.geometrie_pruefen("am Anfang");
                        self.weiter(Schritt::Diagnose);
                    }
                }
            }
            Schritt::Diagnose => {
                // Ein Bild praesentieren und festhalten (nicht nach 50 ms
                // vergessen wie in `bereit`): kommt es spaeter doch an?
                if self.festgehalten.is_none() {
                    let Some(g) = self.gpu.as_mut() else { return self.weiter(Schritt::Ende) };
                    let rect = crate::ziel_rechteck(g.breite, g.hoehe, PROBE_B, PROBE_H, false);
                    let p = g.zeichnen(Some(rect), true, false);
                    match (p, g.praesentiert.back()) {
                        (Praesentiert::Ok, Some((d, _))) => self.festgehalten = Some((d.zweiter(), Instant::now())),
                        _ => {
                            println!("Anzeige-Selbsttest: Diagnose: Praesentieren gescheitert");
                            self.groesse_aendern();
                        }
                    }
                    return;
                }
                let Some((d, seit)) = self.festgehalten.as_ref() else { return };
                let t = unsafe { msg_f64(d.0, c"presentedTime") };
                if t > 0.0 || seit.elapsed() > Duration::from_secs(3) {
                    println!(
                        "Anzeige-Selbsttest: Diagnose: festgehaltenes Bild nach {} ms: presentedTime {t:.3} ({})",
                        seit.elapsed().as_millis(),
                        if t > 0.0 { "kam an, nur spaeter als 50 ms" } else { "blieb 0" }
                    );
                    self.festgehalten = None;
                    self.groesse_aendern();
                }
            }
            Schritt::Groesse => {
                self.zeichnen();
                let (Some(w), Some(g)) = (self.fenster.as_ref(), self.gpu.as_ref()) else { return self.weiter(Schritt::Ende) };
                let skala = w.scale_factor();
                let soll = ((800.0 * skala).round() as u32, (450.0 * skala).round() as u32);
                let s = w.inner_size();
                let passt = (s.width, s.height) == soll && (g.breite, g.hoehe) == soll;
                // Mit Schirm: das naechste Bild, das danach ankommt; ohne:
                // der naechste Versuch; am unsichtbaren Fenster nur die
                // Geometrie.
                let stand = if self.schirm { g.gezeigt() } else { g.versuche as u64 };
                if passt {
                    let marke = *self.marke.get_or_insert(stand);
                    if stand > marke || self.nicht_pruefbar.is_some() {
                        if self.schirm {
                            println!("Anzeige-Selbsttest: ok   neue Groesse {}x{} auf dem Schirm", s.width, s.height);
                        }
                        self.geometrie_pruefen("nach der Groessenaenderung");
                        // Ohne sichtbares Fenster aendert Aus- und
                        // Einblenden nichts, was winit melden koennte.
                        if self.nicht_pruefbar.is_some() {
                            self.weiter(Schritt::Rueckfall);
                        } else {
                            self.verbergen();
                        }
                        return;
                    }
                }
                if dauer > Duration::from_secs(3) {
                    self.fehler(if passt {
                        "Groessenaenderung: kein Bild in der neuen Groesse auf dem Schirm".to_string()
                    } else {
                        format!("Groessenaenderung: {}x{} statt {}x{}", s.width, s.height, soll.0, soll.1)
                    });
                    self.verbergen();
                }
            }
            Schritt::Verbergen => {
                if self.verdeckt == Some(true) {
                    let Some(g) = self.gpu.as_mut() else { return self.weiter(Schritt::Ende) };
                    let (versuche, erster) = (g.versuche, g.erster_versuch);
                    match g.zeichnen(None, true, false) {
                        Praesentiert::Verdeckt => println!("Anzeige-Selbsttest: ok   Occluded(true) nach {} ms, verdeckt nicht praesentiert", dauer.as_millis()),
                        _ => self.fehler("verdeckt trotzdem praesentiert".into()),
                    }
                    if let Some(g) = self.gpu.as_ref() {
                        if (g.versuche, g.erster_versuch) != (versuche, erster) {
                            self.fehler("verdeckt zaehlt der Waechter mit".into());
                        }
                    }
                    self.aufdecken();
                } else if dauer > Duration::from_secs(3) {
                    self.fehler("kein Occluded(true) nach dem Ausblenden des Fensters".into());
                    self.aufdecken();
                }
            }
            Schritt::Aufdecken => {
                if self.verdeckt == Some(false) {
                    if !self.nachholen {
                        self.fehler("Occluded(false): verdeckt() verlangte kein Neuzeichnen".into());
                        self.nachholen = true;
                    }
                    let schirm = self.schirm;
                    let stand = |g: Option<&Gpu>| g.map(|g| if schirm { g.gezeigt() } else { g.versuche as u64 }).unwrap_or(0);
                    let marke = *self.marke.get_or_insert(stand(self.gpu.as_ref()));
                    self.zeichnen();
                    if stand(self.gpu.as_ref()) > marke {
                        println!(
                            "Anzeige-Selbsttest: ok   Occluded(false), nach dem Aufdecken neu praesentiert{}",
                            if schirm { " und auf dem Schirm" } else { "" }
                        );
                        self.weiter(Schritt::Rueckfall);
                        return;
                    }
                }
                if dauer > Duration::from_secs(3) {
                    self.fehler(match self.verdeckt {
                        Some(false) => "nach dem Aufdecken kein Bild auf dem Schirm".into(),
                        _ => "kein Occluded(false) nach dem Einblenden des Fensters".to_string(),
                    });
                    self.weiter(Schritt::Rueckfall);
                }
            }
            Schritt::Rueckfall => {
                self.rueckfall_pruefen();
                self.weiter(Schritt::Ende);
            }
            Schritt::Ende => {}
        }
    }

    fn groesse_aendern(&mut self) {
        if let Some(w) = &self.fenster {
            let _ = w.request_inner_size(winit::dpi::LogicalSize::new(800.0, 450.0));
        }
        self.weiter(Schritt::Groesse);
    }

    fn verbergen(&mut self) {
        if let Some(w) = &self.fenster {
            w.set_visible(false);
        }
        self.weiter(Schritt::Verbergen);
    }

    fn aufdecken(&mut self) {
        self.nachholen = false;
        if let Some(w) = &self.fenster {
            w.set_visible(true);
        }
        self.weiter(Schritt::Aufdecken);
    }
}

impl winit::application::ApplicationHandler for Fensterprobe {
    fn resumed(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if self.fenster.is_some() {
            return;
        }
        // Klein, immer im Vordergrund (sonst koennte ein anderes Fenster es
        // verdecken), ohne den Fokus zu nehmen.
        let attrs = winit::window::Window::default_attributes()
            .with_title("QuadChroma Anzeige-Selbsttest")
            .with_inner_size(winit::dpi::LogicalSize::new(640.0, 360.0))
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_active(false);
        let w = match el.create_window(attrs) {
            Ok(w) => std::sync::Arc::new(w),
            Err(e) => {
                self.fehler(format!("Fenster: {e}"));
                self.weiter(Schritt::Ende);
                el.exit();
                return;
            }
        };
        let ansicht = match w.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::AppKit(h)) => h.ns_view.as_ptr(),
            _ => std::ptr::null_mut(),
        };
        self.ansicht = ansicht;
        let s = w.inner_size();
        let bau = Gpu::neu(ansicht, s.width, s.height).and_then(|mut g| {
            let bild = probebild(vt_decoder::XF44, PROBE_B, PROBE_H)?;
            g.bild_roh(&bild)?;
            Ok(g)
        });
        self.fenster = Some(w);
        match bau {
            Ok(g) => self.gpu = Some(g),
            Err(e) => {
                self.fehler(format!("Metal am Fenster: {e}"));
                self.weiter(Schritt::Ende);
                el.exit();
                return;
            }
        }
        self.weiter(Schritt::Zeigen);
        el.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(8)));
    }

    fn window_event(&mut self, el: &winit::event_loop::ActiveEventLoop, _id: winit::window::WindowId, event: winit::event::WindowEvent) {
        use winit::event::WindowEvent;
        match event {
            WindowEvent::Occluded(v) => {
                println!("Anzeige-Selbsttest:      Occluded({v}) nach {} ms", self.start.elapsed().as_millis());
                self.verdeckt = Some(v);
                if let Some(g) = self.gpu.as_mut() {
                    if g.verdeckt(v) {
                        self.nachholen = true;
                    }
                }
            }
            WindowEvent::CloseRequested => {
                self.fehler("Fenster wurde geschlossen".into());
                self.weiter(Schritt::Ende);
                el.exit();
            }
            // Gezeichnet wird im Takt, nicht auf Anforderung von AppKit.
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        if self.fenster.is_some() {
            self.takt();
        }
        if self.schritt == Schritt::Ende {
            el.exit();
            return;
        }
        el.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(8)));
    }
}

/// --anzeige-selbsttest: die Anzeige an einem echten Fenster von winit,
/// gebaut wie in der App (Ansicht aus raw-window-handle, `Gpu::neu`, ein
/// rohes 4:4:4-Bild ueber Stufe 1). Prueft, was die Tests ohne Fenster nicht
/// koennen: (1) praesentierte Bilder kommen auf dem Schirm an
/// (presentedTime), der Waechter bleibt still; (2) Rahmen, Massstab und
/// Puffer der Schicht passen zur Ansicht und zu winit, auch nach einer
/// Groessenaenderung; (3) winit meldet Verdecken und Aufdecken (Fenster aus-
/// und wieder eingeblendet), verdeckt wird nicht praesentiert und nicht
/// mitgezaehlt, nach dem Aufdecken wird neu praesentiert und kommt an; (4)
/// der Rueckfall: nach dem Drop haengt keine Metal-Schicht mehr an der
/// Ansicht, und softbuffer zeichnet am selben Fenster. Ein kleines Fenster
/// fuer wenige Sekunden, immer im Vordergrund; ohne Dock-Symbol, ohne Host,
/// ohne Aufnahme, ohne Netz. Rueckgabe 0 bestanden, 1 nicht bestanden, 3
/// nicht pruefbar (das Fenster war verdeckt, etwa bei gesperrtem Bildschirm).
pub fn fenster_selbsttest() -> i32 {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    let el = match winit::event_loop::EventLoop::builder().with_activation_policy(ActivationPolicy::Accessory).with_default_menu(false).build() {
        Ok(el) => el,
        Err(e) => {
            println!("Anzeige-Selbsttest: keine Ereignisschleife ({e})");
            return 1;
        }
    };
    let jetzt = Instant::now();
    let mut probe = Fensterprobe {
        fenster: None,
        ansicht: std::ptr::null_mut(),
        gpu: None,
        schirm: false,
        festgehalten: None,
        schritt: Schritt::Zeigen,
        start: jetzt,
        seit: jetzt,
        marke: None,
        verdeckt: None,
        nachholen: false,
        ui_masse: (0, 0),
        fehler: Vec::new(),
        nicht_pruefbar: None,
    };
    if let Err(e) = el.run_app(&mut probe) {
        probe.fehler(format!("Ereignisschleife: {e}"));
    }
    probe.festgehalten = None;
    drop(probe.gpu.take());
    for z in protokoll::abholen() {
        println!("    {z}");
    }
    let dauer = probe.start.elapsed().as_secs_f32();
    match (&probe.nicht_pruefbar, probe.fehler.is_empty()) {
        (Some(grund), true) => {
            println!("Anzeige-Selbsttest nicht pruefbar: {grund} ({dauer:.1} s)");
            3
        }
        (_, true) => {
            println!("Anzeige-Selbsttest bestanden ({dauer:.1} s)");
            0
        }
        (_, false) => {
            println!("Anzeige-Selbsttest NICHT bestanden: {} Fehler ({dauer:.1} s)", probe.fehler.len());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Konstanten liegen so, wie die Shader sie lesen.
    #[test]
    fn konstanten_wie_im_shader() {
        assert_eq!(std::mem::size_of::<KonstFormat>(), 16);
        assert_eq!(std::mem::size_of::<KonstAnzeige>(), 48);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, bild), 16);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, modus), 32);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, grund), 36);
    }

    /// Ein Geraet fuer die Goldbildtests - ohne Metal (virtuelle Maschine,
    /// Sandbox) werden sie uebersprungen.
    fn geraet() -> Option<Gpu> {
        match Gpu::ohne_fenster() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("uebersprungen: {e}");
                None
            }
        }
    }

    fn goldbild(format: u32) {
        let Some(mut g) = geraet() else { return };
        let mut u = ui::Ui::new();
        u.tick = 40;
        u.mouse = (-1, -1);
        let lang = crate::strings::pick("de");
        for (w, h) in [(257u32, 131u32), (64, 36)] {
            assert!(format_pruefen(&mut g, format, w, h, None, lang, &mut u), "{} {w}x{h}", vt_decoder::fourcc_text(format));
        }
    }

    /// 4:4:4 10 Bit (xf44, wie der Host streamt): Stufe 1 und das skalierte
    /// wie das pixelgenaue Bild bitgleich zum CPU-Weg.
    #[test]
    fn goldbild_444_10() {
        goldbild(vt_decoder::XF44);
    }

    /// 4:2:0 8 Bit (420f, NV12): dasselbe.
    #[test]
    fn goldbild_420_8() {
        goldbild(vt_decoder::F420);
    }

    /// Begrenzter Wertebereich (x420): die Dehnung im Shader ist die von
    /// `zeile_rgb`.
    #[test]
    fn goldbild_begrenzt() {
        goldbild(vt_decoder::X420);
    }

    /// Der Waechter als Rechnung: erst nach genug Versuchen UND genug Zeit,
    /// und nie, sobald ein Bild ankam.
    #[test]
    fn waechter_rechnung() {
        let f = WAECHTER_FRIST;
        let n = WAECHTER_VERSUCHE;
        assert!(!nie_gezeigt(0, n - 1, Some(f * 2), n, f), "zu wenige Versuche");
        assert!(!nie_gezeigt(0, n, Some(f - Duration::from_millis(1)), n, f), "zu frueh");
        assert!(nie_gezeigt(0, n, Some(f), n, f));
        assert!(!nie_gezeigt(1, n * 100, Some(f * 10), n, f), "ein Bild kam an");
        assert!(!nie_gezeigt(0, n, None, n, f), "ohne ersten Versuch");
    }

    /// Der Weg ueber die Schicht, ohne Fenster: eine CALayer als Schicht der
    /// Ansicht, darunter der CAMetalLayer. Praesentieren geht, ohne dass
    /// nextDrawable wartet; nach zwei Bildern, die nicht auf dem Schirm
    /// ankommen (hier nie), ist die Anzeige nicht bereit - bis die Frist
    /// abgelaufen ist. Rahmen und Massstab folgen der Wurzelschicht, der
    /// Puffer den Bildpunkten; verdeckt wird nicht gezeichnet und nicht
    /// gezaehlt, das Aufdecken verlangt einmal ein Neuzeichnen; der Waechter
    /// zaehlt die Versuche ohne gezeigtes Bild; Drop nimmt die Schicht wieder
    /// heraus.
    #[test]
    fn schicht_ohne_fenster() {
        if unsafe { Obj::eigen(MTLCreateSystemDefaultDevice()) }.is_none() {
            eprintln!("uebersprungen: kein Metal-Geraet");
            return;
        }
        let _pool = Pool::neu();
        let wurzel = unsafe { Obj::halten(msg_id(klasse(c"CALayer"), c"layer")).expect("CALayer") };
        let rahmen = |w: f64, h: f64| CgRect { x: 0.0, y: 0.0, w, h };
        unsafe {
            senden!(wurzel.0, sel(c"setContentsScale:"), 2.0f64 => f64; -> ());
            senden!(wurzel.0, sel(c"setBounds:"), rahmen(320.0, 180.0) => CgRect; -> ());
        }
        let mut g = Gpu::an_schicht(wurzel.zweiter(), 640, 360).expect("Schicht");
        assert_eq!((g.breite, g.hoehe, g.skala), (640, 360, 2.0));
        let m = g.geometrie().expect("Geometrie");
        assert_eq!((m.rahmen, m.skala, m.puffer), (rahmen(320.0, 180.0), 2.0, CgSize { w: 640.0, h: 360.0 }));
        let unter = |w: &Obj| unsafe { msg_zahl(msg_id(w.0, c"sublayers"), c"count") };
        assert_eq!(unter(&wurzel), 1);
        assert!(g.fenster_sichtbar(), "ohne Ansicht gilt die Schicht als sichtbar");
        assert!(!g.verdeckt(false), "nichts ausgefallen, nichts nachzuholen");
        let bild = probebild(vt_decoder::XF44, 320, 180).expect("Probebild");
        g.bild_roh(&bild).expect("bild_roh");
        drop(bild);
        let rect = crate::ziel_rechteck(640, 360, 320, 180, false);
        for i in 0..HOECHSTENS_UNTERWEGS {
            assert!(g.bereit(), "Bild {i}");
            let t = Instant::now();
            assert!(matches!(g.zeichnen(Some(rect), false, false), Praesentiert::Ok), "Bild {i}");
            assert!(t.elapsed() < Duration::from_millis(200), "nextDrawable wartete {:?}", t.elapsed());
        }
        assert!(!g.bereit(), "zwei Bilder unterwegs, trotzdem bereit");
        std::thread::sleep(PRAESENT_FRIST + Duration::from_millis(20));
        assert!(g.bereit(), "nach der Frist nicht wieder bereit");
        // Ohne Fenster kommt nie ein Bild an: der Waechter zaehlt, schlaegt
        // mit den echten Schwellen aber noch nicht an.
        assert_eq!((g.gezeigt(), g.versuche), (0, HOECHSTENS_UNTERWEGS as u32));
        assert!(!g.nie_auf_dem_schirm());
        assert!(nie_gezeigt(g.gezeigt, g.versuche, g.erster_versuch.map(|t| t.elapsed()), HOECHSTENS_UNTERWEGS as u32, Duration::ZERO));
        // Neue Bildpunkte bei gleicher Ansicht: der Puffer folgt, der Rahmen
        // bleibt der der Ansicht; dann eine groessere Ansicht.
        g.groesse(800, 600).expect("groesse");
        assert_eq!((g.breite, g.hoehe), (800, 600));
        let m = g.geometrie().expect("Geometrie");
        assert_eq!((m.rahmen, m.puffer), (rahmen(320.0, 180.0), CgSize { w: 800.0, h: 600.0 }));
        unsafe { senden!(wurzel.0, sel(c"setBounds:"), rahmen(400.0, 300.0) => CgRect; -> ()) };
        g.groesse(800, 600).expect("groesse");
        assert_eq!(g.geometrie().expect("Geometrie").rahmen, rahmen(400.0, 300.0));
        assert!(matches!(g.zeichnen(None, false, false), Praesentiert::Ok));
        let versuche = g.versuche;
        assert!(!g.verdeckt(true), "Verdecken verlangt kein Neuzeichnen");
        assert!(matches!(g.zeichnen(None, false, false), Praesentiert::Verdeckt));
        assert_eq!(g.versuche, versuche, "verdeckt zaehlt der Waechter nicht");
        assert!(g.verdeckt(false), "Aufdecken nach einem ausgefallenen Bild verlangt ein Neuzeichnen");
        assert!(matches!(g.zeichnen(None, false, false), Praesentiert::Ok));
        assert!(!g.verdeckt(false), "nach dem neuen Bild ist nichts nachzuholen");
        drop(g);
        assert_eq!(unter(&wurzel), 0, "Schicht haengt nach Drop noch an");
    }
}
