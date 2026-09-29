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
// HDR10 (Schritt 5a des HDR-Plans): Ein Bild mit PQ im VUI
// (vt_decoder::Bild::farbe) geht durch eigene Shader, dieselbe Rechnung wie
// hdr.rs: Codes -> Y'CbCr (BT.2020) -> R'G'B' -> PQ -> Licht relativ zum
// SDR-Weiss des Hosts -> Abbildung. Das Ziel entscheidet der Bildschirm des
// Fensters. Kann er EDR (potentieller Kopfraum ueber 1), rechnet Stufe 1 in
// eine RGBA16Float-Zwischentextur (erweitert linear Display P3, 1.0 = SDR-Weiss
// des Clients), abgebildet auf den aktuellen Kopfraum; Stufe 2 mischt dann in
// linearem Licht (dieselbe 16.16-Quellpunktwahl), legt die Oberflaeche linear
// darueber, und die Schicht wird RGBA16Float mit EDR. Sonst bildet Stufe 1 auf
// SDR ab (farbtontreu beim SDR-Weiss abgeschnitten, BT.709, sRGB) in dieselbe
// RGBA8-Zwischentextur wie ein SDR-Bild - Stufe 2 und die Schicht bleiben die
// heutigen. Ein SDR-Strom laeuft unveraendert bitgleich. Das letzte PQ-Bild
// haelt die Anzeige fest: aendern sich Schirm, Kopfraum oder die Metadaten der
// Strominfo, rechnet `zeichnen` Stufe 1 daraus neu, und die Schicht wechselt
// beim naechsten nextDrawable. Den Anlass dazu gibt `nachzeichnen_faellig`,
// das main.rs je Takt fragt - auch wenn der Host bei stillem Bildschirm
// nichts schickt. `hdr_praesentiert` (Gold in der Oberflaeche)
// heisst: das zuletzt gezeigte Bild lief ueber EDR mit Kopfraum ueber 1.
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
use crate::{hdr, protokoll, ui, Frame};

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

// Der Farbraum der EDR-Schicht.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceExtendedLinearDisplayP3: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut c_void;
    fn CGColorSpaceRelease(cs: *mut c_void);
}

/// MTLPixelFormat.
const R8_UINT: usize = 13;
const R16_UINT: usize = 23;
const RG8_UINT: usize = 33;
const RG16_UINT: usize = 63;
const RGBA8_UINT: usize = 73;
const BGRA8_UNORM: usize = 80;
/// Die EDR-Zwischentextur und der Puffer der EDR-Schicht: halbe
/// Gleitkommazahlen, erweitert linear (Werte ueber 1 = heller als SDR-Weiss).
const RGBA16_FLOAT: usize = 115;
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
/// So oft fragt `nachzeichnen_faellig` den Schirm, solange ein PQ-Bild
/// steht - im Takt der Oberflaeche. Nach dem Einschalten von EDR steigt der
/// aktuelle Kopfraum erst, wenn die EDR-Schicht auf dem Schirm ist; diesem
/// Anstieg folgt das stehende Bild damit binnen 33 ms.
const PQ_PRUEFTAKT: Duration = Duration::from_millis(33);

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

/// Kann dieser Weg HDR-Bilder zeigen (Bit 1 in IN_ANZEIGE)? Ja: die Metal-
/// Anzeige rechnet PQ selbst - auf einem EDR-Schirm in HDR, auf einem
/// SDR-Schirm abgebildet (fuer den Wunsch "Immer" und die Zeit bis zur
/// Neuaushandlung, wenn das Fenster den Schirm wechselt). Der softbuffer-Weg
/// meldet nein (main.rs).
pub const HDR_DARSTELLUNG: bool = true;

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
    uint begrenzt;   // begrenzter Wertebereich (Format oder VUI): vor der Matrix dehnen
    uint _f;
    int4 c;          // Matrix aus dem VUI in 16.16: Cr->R, Cb->G, Cr->G, Cb->B (BT.709 oder BT.601)
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
    int r = y + ((f.c.x * cr) >> 16);               // BT.709: 1.5748 - dieselben Konstanten wie zeile_rgb
    int g = y - ((f.c.y * cb + f.c.z * cr) >> 16);  // 0.1873, 0.4681 - EINE Verschiebung der Summe
    int b = y + ((f.c.w * cb) >> 16);               // 1.8556
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

/// Die HDR-Shader (PQ, BT.2020), dieselbe Rechnung wie hdr.rs (Abschnitt 4 des
/// HDR-Plans). Die Zahlen von PQ, das Knie und die Matrix BT.709 -> Display P3
/// stehen nicht hier: `shader_quelle` setzt sie aus hdr.rs davor
/// (`hdr_konstanten`) - so koennen Rust und MSL nicht auseinanderlaufen. pow
/// ist die schnelle Fassung der Karte: gemessen (M1, --anzeigetest) bleibt sie
/// mit PQ und sRGB in den Toleranzen gegen hdr.rs (SDR +-1; auf EDR ist die
/// Abweichung die Rundung auf halbe Gleitkommazahlen); precise::pow kostete
/// bei 1440p rund 9 ms je Bild mehr auf der Karte, die schnelle rund 1 ms.
const SHADER_HDR: &str = r#"
// ---------- HDR10: PQ (BT.2020) -> Anzeige ----------
struct Hdr {
    float4 m0;          // Zeilen der Matrix Primaerfarben der Quelle -> Ziel (lineares Licht)
    float4 m1;
    float4 m2;
    float4 ycc;         // R' = Y + x Cr, G' = Y - y Cb - z Cr, B' = Y + w Cb
    uint   sub;         // 4:2:0: je zwei Bildpunkte teilen sich einen Farbwert
    uint   acht;        // 8-Bit-Ebenen: Code = Wert << 2; sonst oben buendig: Wert >> 6
    uint   begrenzt;    // begrenzter Bereich nach H.273: Y (D - 64) / 876, C (D - 512) / 896
    uint   _f;
    float  kehr_weiss;  // 1 / SDR-Weiss der Quelle in nit (W_h)
    float  hs;          // Spitze der Quelle relativ zu W_h
    float  hd;          // Kopfraum des Ziels (SDR: 1)
    float  _g;
};

// PQ-Signal 0..1 -> nit (EOTF), ausserhalb geklemmt - wie hdr::pq_eotf. Die
// Basis nie unter 1e-30: 0 hoch y ist mit schneller Mathematik nicht sicher 0
// (log2(0) = -inf), 1e-30 hoch y ist es in beiden Faellen.
static float3 pq_nit(float3 e) {
    e = clamp(e, 0.0, 1.0);
    float3 p = pow(max(e, 1e-30), float3(1.0 / PQ_M2));
    float3 z = max(p - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * p);
    return pow(max(z, 1e-30), float3(1.0 / PQ_M1)) * PQ_SPITZE_NIT;
}

// Die Abbildung von m = max(R, G, B) - wie hdr::abbilden: bis zum Knie
// unberuehrt, darueber C1-stetig auf den Kopfraum, Hs genau auf Hd; ohne
// Kopfraum (Hd <= Knie) farbtontreu abgeschnitten.
static float abbilden(float m, float hs, float hd) {
    if (hs <= hd) return min(m, hd);
    float d = hd - KNIE;
    if (d <= 0.0) return min(m, hd);
    if (m <= KNIE) return m;
    float e = m - KNIE;
    float s = hs - KNIE;
    return min(KNIE + e * (1.0 + e * d / (s * s)) / (1.0 + e / d), hd);
}

// Farbtreu wie hdr::rgb_abbilden: RGB *= m' / m.
static float3 rgb_abbilden(float3 rgb, float hs, float hd) {
    float m = max(rgb.r, max(rgb.g, rgb.b));
    if (!(m > 0.0)) return rgb;
    return rgb * (abbilden(m, hs, hd) / m);
}

// sRGB stueckweise (IEC 61966-2-1), wie hdr::srgb_oetf / hdr::srgb_eotf.
static float3 srgb_oetf(float3 l) {
    l = clamp(l, 0.0, 1.0);
    float3 tief = l * 12.92;
    float3 hoch = 1.055 * pow(max(l, 1e-30), float3(1.0 / 2.4)) - 0.055;
    return select(hoch, tief, l <= 0.0031308);
}

static float3 srgb_eotf(float3 v) {
    float3 tief = v / 12.92;
    float3 hoch = pow((max(v, 0.0) + 0.055) / 1.055, float3(2.4));
    return select(hoch, tief, v <= 0.04045);
}

static float3 p3_aus_709(float3 v) {
    return float3(dot(P3_AUS_709_R, v), dot(P3_AUS_709_G, v), dot(P3_AUS_709_B, v));
}

// Ein Bildpunkt als lineares Licht relativ zum SDR-Weiss der Quelle, in den
// Primaerfarben der Quelle: Codes (10 Bit) -> Y'CbCr -> R'G'B' -> PQ -> nit / W_h.
static float3 pq_relativ(uint2 xy, constant Hdr& k,
                         texture2d<uint, access::read> ebene_y, texture2d<uint, access::read> ebene_uv) {
    uint2 c = (k.sub != 0) ? (xy >> 1) : xy;        // naechster Farbwert, wie ps_umrechnen
    uint yw = ebene_y.read(xy).r;
    uint2 uv = ebene_uv.read(c).rg;
    int3 code = (k.acht != 0) ? int3(int(yw) << 2, int(uv.x) << 2, int(uv.y) << 2)
                              : int3(int(yw >> 6), int(uv.x >> 6), int(uv.y >> 6));
    float3 n = (k.begrenzt != 0)
        ? float3(float(code.x - 64) / 876.0, float(code.y - 512) / 896.0, float(code.z - 512) / 896.0)
        : float3(float(code.x) / 1023.0, float(code.y - 512) / 1023.0, float(code.z - 512) / 1023.0);
    float3 e = float3(n.x + k.ycc.x * n.z, n.x - k.ycc.y * n.y - k.ycc.z * n.z, n.x + k.ycc.w * n.y);
    return pq_nit(e) * k.kehr_weiss;
}

static float3 matrix_mal(constant Hdr& k, float3 v) {
    return float3(dot(k.m0.xyz, v), dot(k.m1.xyz, v), dot(k.m2.xyz, v));
}

// Stufe 1 fuer einen SDR-Schirm: abbilden (Hd <= 1: farbtontreu beim SDR-Weiss
// abgeschnitten), BT.709, sRGB 8 Bit gerundet - in dieselbe Zwischentextur
// (Bytes B, G, R, A) wie ps_umrechnen; Stufe 2 bleibt die heutige.
fragment uint4 ps_umrechnen_hdr_sdr(Punkt in [[stage_in]],
                                    constant Hdr& k [[buffer(0)]],
                                    texture2d<uint, access::read> ebene_y [[texture(0)]],
                                    texture2d<uint, access::read> ebene_uv [[texture(1)]]) {
    float3 rel = pq_relativ(uint2(in.pos.xy), k, ebene_y, ebene_uv);
    float3 s = matrix_mal(k, rgb_abbilden(rel, k.hs, min(k.hd, 1.0)));
    uint3 v = uint3(floor(srgb_oetf(s) * 255.0 + 0.5));
    return uint4(v.b, v.g, v.r, 255u);
}

// Stufe 1 fuer einen EDR-Schirm: abbilden auf den Kopfraum, Display P3,
// negative Anteile 0. Erweitert linear: 1.0 = SDR-Weiss des Clients - das
// SDR-Weiss des Hosts wird das des Clients (Entscheidung E3).
fragment half4 ps_umrechnen_hdr_edr(Punkt in [[stage_in]],
                                    constant Hdr& k [[buffer(0)]],
                                    texture2d<uint, access::read> ebene_y [[texture(0)]],
                                    texture2d<uint, access::read> ebene_uv [[texture(1)]]) {
    float3 rel = pq_relativ(uint2(in.pos.xy), k, ebene_y, ebene_uv);
    // half4 selbst runden: schreibt die Karte float4 in ein RGBA16Float-Ziel,
    // schneidet sie ab (bis eine Stufe daneben).
    return half4(half3(max(matrix_mal(k, rgb_abbilden(rel, k.hs, k.hd)), 0.0)), 1.0h);
}

// Stufe 2 auf der EDR-Schicht (RGBA16Float, erweitert linear Display P3):
// Quellpunkte wie ps_anzeigen (16.16, ohne Mittenversatz), bilinear in
// linearem Licht; Grundfarbe und Oberflaeche (sRGB, vormultipliziert, T =
// Durchsicht) linear darueber - Oberflaechenweiss = SDR-Weiss des Clients.
fragment half4 ps_anzeigen_edr(Punkt in [[stage_in]],
                                constant Anzeige& k [[buffer(0)]],
                                texture2d<float, access::read> video [[texture(0)]],       // linear P3
                                texture2d<uint, access::read> oberflaeche [[texture(1)]]) { // Bytes B, G, R, T
    int2 p = int2(in.pos.xy) - k.rect.xy;
    float3 grund = float3(uint3((k.grund >> 16) & 255u, (k.grund >> 8) & 255u, k.grund & 255u)) / 255.0;
    float3 farbe = p3_aus_709(srgb_eotf(grund));
    if ((k.modus & 1u) != 0 && all(p >= 0) && all(p < k.rect.zw)) {
        uint2 f = uint2(p) * k.schritt;
        uint2 q0 = f >> 16;
        uint2 q1 = min(q0 + 1u, k.bild - 1u);
        float2 w = float2(f & 65535u) / 65536.0;
        float3 a = video.read(q0).rgb;
        float3 b = video.read(uint2(q1.x, q0.y)).rgb;
        float3 c = video.read(uint2(q0.x, q1.y)).rgb;
        float3 d = video.read(q1).rgb;
        farbe = mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
    }
    if ((k.modus & 2u) != 0) {
        uint4 o = oberflaeche.read(uint2(in.pos.xy));
        float t = float(o.w) / 255.0;
        float deckung = 1.0 - t;
        float3 c = float3(o.zyx) / 255.0;
        // Entmultiplizieren, linear, wieder multiplizieren; ganz durchsichtig
        // mit Farbe (kommt so nicht vor) wirkt additiv.
        float3 lin = (deckung > 0.5 / 255.0) ? srgb_eotf(min(c / deckung, 1.0)) * deckung : srgb_eotf(c);
        farbe = farbe * t + p3_aus_709(lin);
    }
    return half4(half3(farbe), 1.0h);
}
"#;

/// Die Zahlen der HDR-Shader aus hdr.rs als MSL-Konstanten. `{:?}` schreibt
/// die kuerzeste Dezimalzahl, die genau diesen f32-Wert ergibt.
fn hdr_konstanten() -> String {
    let m = &hdr::M_709_NACH_P3;
    let zeile = |r: usize| format!("float3({:?}, {:?}, {:?})", m[r][0], m[r][1], m[r][2]);
    format!(
        "\n// ---------- HDR: Zahlen aus hdr.rs ----------\n\
         constant float PQ_M1 = {:?};\n\
         constant float PQ_M2 = {:?};\n\
         constant float PQ_C1 = {:?};\n\
         constant float PQ_C2 = {:?};\n\
         constant float PQ_C3 = {:?};\n\
         constant float PQ_SPITZE_NIT = {:?};\n\
         constant float KNIE = {:?};\n\
         constant float3 P3_AUS_709_R = {};\n\
         constant float3 P3_AUS_709_G = {};\n\
         constant float3 P3_AUS_709_B = {};\n",
        hdr::PQ_M1 as f32,
        hdr::PQ_M2 as f32,
        hdr::PQ_C1 as f32,
        hdr::PQ_C2 as f32,
        hdr::PQ_C3 as f32,
        hdr::PQ_SPITZE_NIT as f32,
        hdr::KNIE,
        zeile(0),
        zeile(1),
        zeile(2),
    )
}

/// Der ganze Text fuer newLibraryWithSource: die heutigen Stufen, die Zahlen
/// aus hdr.rs, die HDR-Stufen.
fn shader_quelle() -> String {
    format!("{SHADER}{}{SHADER_HDR}", hdr_konstanten())
}

/// Konstanten von Stufe 1 (struct Format, 32 Byte; int4 auf 16 Byte
/// ausgerichtet): Aufbau der Ebenen, Bereich und Matrix (wie `zeile_rgb`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KonstFormat {
    sub: u32,
    schieb: u32,
    begrenzt: u32,
    _f: u32,
    c: [i32; 4],
}

impl KonstFormat {
    /// Fuer ein SDR-Bild dieses Formats in dieser Farbe: begrenzt, wenn das
    /// Format (VideoToolbox ohne "f") oder das VUI es sagt; die Matrix aus
    /// dem VUI (BT.709 oder BT.601).
    fn neu(fmt: &crate::EbenenFormat, farbe: &hdr::Farbe) -> KonstFormat {
        KonstFormat { sub: fmt.sub as u32, schieb: fmt.schieb(), begrenzt: (fmt.begrenzt || !farbe.voll) as u32, _f: 0, c: farbe.sdr_koeffizienten() }
    }
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

/// Konstanten der HDR-Stufe 1 (struct Hdr, 96 Byte: drei float4-Zeilen der
/// Matrix, float4 der Y'CbCr-Koeffizienten, vier uint, vier float).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct KonstHdr {
    m: [[f32; 4]; 3],
    ycc: [f32; 4],
    sub: u32,
    acht: u32,
    begrenzt: u32,
    _f: u32,
    kehr_weiss: f32,
    hs: f32,
    hd: f32,
    _g: f32,
}

impl KonstHdr {
    fn neu(farbe: &hdr::Farbe, fmt: &crate::EbenenFormat, ziel: &PqZiel) -> KonstHdr {
        let m = ausgabematrix(farbe.primaer, ziel.edr);
        let zeile = |r: usize| [m[r][0], m[r][1], m[r][2], 0.0];
        KonstHdr {
            m: [zeile(0), zeile(1), zeile(2)],
            ycc: ycc_koeffizienten(farbe.matrix),
            sub: fmt.sub as u32,
            acht: (fmt.bits == 8) as u32,
            begrenzt: fmt.begrenzt as u32,
            _f: 0,
            kehr_weiss: 1.0 / ziel.ab.weiss_nit,
            hs: ziel.ab.hs,
            hd: if ziel.edr { ziel.ab.hd } else { ziel.ab.hd.min(1.0) },
            _g: 0.0,
        }
    }
}

fn matrix_produkt(a: &hdr::Matrix, b: &hdr::Matrix) -> hdr::Matrix {
    let mut m = [[0f32; 3]; 3];
    for (r, zeile) in m.iter_mut().enumerate() {
        for (c, wert) in zeile.iter_mut().enumerate() {
            *wert = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

/// Die Matrix von den Primaerfarben der Quelle ins Ziel (lineares Licht):
/// EDR nach Display P3, SDR nach BT.709. Auf der Leitung ist es immer BT.2020;
/// BT.709 und P3 werden trotzdem richtig gezeigt, alles Unbekannte gilt als
/// BT.2020.
fn ausgabematrix(primaer: u8, edr: bool) -> hdr::Matrix {
    const EINS: hdr::Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    match (primaer, edr) {
        (hdr::PRIMAER_709, true) => hdr::M_709_NACH_P3,
        (hdr::PRIMAER_709, false) => EINS,
        (vt_decoder::PRIMAER_P3, true) => EINS,
        (vt_decoder::PRIMAER_P3, false) => matrix_produkt(&hdr::M_2020_NACH_709, &hdr::M_P3_NACH_2020),
        (_, true) => hdr::M_2020_NACH_P3,
        (_, false) => hdr::M_2020_NACH_709,
    }
}

/// R' = Y + a Cr, G' = Y - b Cb - c Cr, B' = Y + d Cb fuer die Matrix des VUI:
/// BT.709 aus Kr/Kb, sonst BT.2020-NCL (hdr.rs).
fn ycc_koeffizienten(matrix: u8) -> [f32; 4] {
    if matrix == hdr::MATRIX_709 {
        let (kr, kb) = (0.2126f32, 0.0722f32);
        let kg = 1.0 - kr - kb;
        [2.0 * (1.0 - kr), 2.0 * kb * (1.0 - kb) / kg, 2.0 * kr * (1.0 - kr) / kg, 2.0 * (1.0 - kb)]
    } else {
        [hdr::CR_R_2020, hdr::CB_G_2020, hdr::CR_G_2020, hdr::CB_B_2020]
    }
}

/// Wohin ein PQ-Bild gerechnet wird.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PqZiel {
    /// EDR (RGBA16Float, erweitert linear P3) statt SDR (sRGB 8 Bit).
    edr: bool,
    /// W_h und Hs aus der Strominfo; Hd = aktueller Kopfraum des Schirms
    /// (EDR) bzw. 1 (SDR).
    ab: hdr::Abbildung,
}

impl PqZiel {
    /// Muss Stufe 1 fuer `neu` noch einmal rechnen? Bei einem anderen Weg
    /// oder anderen Metadaten immer; beim Kopfraum erst ab einem Prozent -
    /// er schwankt mit Helligkeit und Energiesparen in feinen Schritten.
    fn anders(&self, neu: &PqZiel) -> bool {
        self.edr != neu.edr
            || self.ab.weiss_nit != neu.ab.weiss_nit
            || self.ab.hs != neu.ab.hs
            || (self.ab.hd - neu.ab.hd).abs() > 0.01 * self.ab.hd.max(1.0)
    }
}

/// Das letzte PQ-Bild: ein eigener Griff auf den Puffer des Decoders, damit
/// Stufe 1 neu rechnen kann, wenn sich das Ziel aendert, ohne auf das
/// naechste Bild des Hosts zu warten (bei stillem Bildschirm kommt lange
/// keins). Ein Puffer mehr ausser Haus - der Vorrat des Decoders legt dafuer
/// einen an. Faellt mit dem naechsten SDR- oder RGB-Bild.
struct PqBild {
    puffer: Cf,
    fmt: crate::EbenenFormat,
    w: u32,
    h: u32,
    farbe: hdr::Farbe,
    /// Womit die Zwischentextur zuletzt daraus gerechnet wurde.
    gerechnet: PqZiel,
}

/// Halbe Gleitkommazahl (IEEE 754 binary16) nach f32 - zum Auslesen der
/// EDR-Texturen.
fn halb_nach_f32(h: u16) -> f32 {
    let vz = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    match e {
        0 => vz * m * 2f32.powi(-24),
        0x1f if m == 0.0 => vz * f32::INFINITY,
        0x1f => f32::NAN,
        _ => vz * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Die Referenz fuer einen PQ-Bildpunkt auf dem EDR-Ziel (CPU, hdr.rs): Y'CbCr
/// BT.2020-NCL -> PQ -> relativ zu W_h -> Abbildung auf Hd -> Display P3,
/// negative Anteile 0. Das Gegenstueck zu hdr::pq_nach_srgb8 fuer SDR.
fn pq_nach_edr(y: f32, cb: f32, cr: f32, ab: &hdr::Abbildung) -> [f32; 3] {
    let e = hdr::ycbcr_nach_rgb_2020(y, cb, cr);
    let rel = e.map(|c| (hdr::pq_eotf(c as f64) / ab.weiss_nit as f64) as f32);
    hdr::mal(&hdr::M_2020_NACH_P3, hdr::rgb_abbilden(rel, ab.hs, ab.hd)).map(|c| c.max(0.0))
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

/// Eine Textur der Anzeige, ihre Groesse und ihr Format (MTLPixelFormat).
struct Textur {
    tex: Obj,
    w: u32,
    h: u32,
    format: usize,
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
    /// HDR: Stufe 1 fuer PQ auf SDR (RGBA8) und auf EDR (RGBA16Float),
    /// Stufe 2 auf die EDR-Schicht.
    stufe1_hdr_sdr: Obj,
    stufe1_hdr_edr: Obj,
    stufe2_edr: Obj,
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
    /// HDR: die Metadaten der Quelle aus der Strominfo (SDR-Weiss W_h,
    /// Spitze), von main.rs je Zeichnung gesetzt; None: Vorgaben 203/1000 nit.
    quelle: Option<hdr::InfoV1>,
    /// Das letzte PQ-Bild, solange der Strom PQ ist.
    pq_bild: Option<PqBild>,
    /// Wann `nachzeichnen_faellig` den Schirm zuletzt gefragt hat.
    pq_geprueft: Option<Instant>,
    /// Die Schicht ist gerade EDR (RGBA16Float, erweitert linear P3).
    schicht_edr: bool,
    /// Das zuletzt praesentierte Bild lief ueber EDR mit Kopfraum ueber 1.
    hdr_gezeigt: bool,
    /// Ohne Ansicht (Tests, --anzeigetest): der Kopfraum (moeglich, aktuell),
    /// den der Schirm haette; ab Werk 1/1, ein SDR-Schirm.
    kopfraum_ohne_fenster: (f32, f32),
    /// Was zuletzt zur HDR-Lage ins Protokoll ging (neu nur bei Aenderung).
    hdr_stand: Option<String>,
    /// Der Farbraum, den die Schicht ab Werk hat (eigener Griff; None: nil) -
    /// zurueck von EDR bekommt sie genau den wieder, SDR bleibt wie immer.
    sdr_farbraum: Option<Cf>,
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
            // Den Farbraum ab Werk festhalten: die EDR-Schicht ersetzt ihn
            // und gibt ihn beim Zurueckstellen wieder.
            let farbraum = senden!(schicht.0, sel(c"colorspace"); -> *mut c_void);
            if !farbraum.is_null() {
                cf::CFRetain(farbraum as cf::CFTypeRef);
                gpu.sdr_farbraum = Some(Cf(farbraum));
            }
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
                ns_text(&shader_quelle()) => Id, std::ptr::null_mut() => Id, &mut err as *mut Id => *mut Id; -> Id);
            let lib = Obj::eigen(lib).ok_or_else(|| format!("Shader: {}", fehlertext(err)))?;
            let funktion = |n: &str| Obj::eigen(msg_id_1(lib.0, c"newFunctionWithName:", ns_text(n))).ok_or_else(|| format!("Shader: {n} fehlt"));
            let vs = funktion("vs_main")?;
            let ps1 = funktion("ps_umrechnen")?;
            let ps2 = funktion("ps_anzeigen")?;
            let stufe1 = pipeline(device.0, vs.0, ps1.0, RGBA8_UINT, "1 (umrechnen)")?;
            let stufe2 = pipeline(device.0, vs.0, ps2.0, BGRA8_UNORM, "2 (anzeigen)")?;
            let stufe1_hdr_sdr = pipeline(device.0, vs.0, funktion("ps_umrechnen_hdr_sdr")?.0, RGBA8_UINT, "1 (PQ nach SDR)")?;
            let stufe1_hdr_edr = pipeline(device.0, vs.0, funktion("ps_umrechnen_hdr_edr")?.0, RGBA16_FLOAT, "1 (PQ nach EDR)")?;
            let stufe2_edr = pipeline(device.0, vs.0, funktion("ps_anzeigen_edr")?.0, RGBA16_FLOAT, "2 (anzeigen, EDR)")?;
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
                stufe1_hdr_sdr,
                stufe1_hdr_edr,
                stufe2_edr,
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
                quelle: None,
                pq_bild: None,
                pq_geprueft: None,
                schicht_edr: false,
                hdr_gezeigt: false,
                kopfraum_ohne_fenster: (1.0, 1.0),
                hdr_stand: None,
                sdr_farbraum: None,
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

    /// Praesentiert die Anzeige gerade in HDR (Gold in der Oberflaeche)? Ja,
    /// wenn das zuletzt gezeigte Bild ein PQ-Bild ueber die EDR-Schicht war
    /// und der Schirm dabei Kopfraum ueber dem SDR-Weiss hatte - EDR ohne
    /// Kopfraum (Hd = 1) sieht aus wie SDR und zaehlt nicht.
    pub fn hdr_praesentiert(&self) -> bool {
        self.hdr_gezeigt
    }

    /// Nur fuer die Tests in main.rs: so tun, als zeigte die Schicht ein
    /// PQ-Bild ueber EDR mit Kopfraum (`an`) bzw. wieder SDR - ohne Fenster
    /// gibt es keine.
    #[cfg(test)]
    pub fn hdr_vortaeuschen(&mut self, an: bool) {
        self.hdr_gezeigt = an;
    }

    /// Soll main.rs neu zeichnen lassen, obwohl kein neues Bild kam? Ja, wenn
    /// die Anzeige ein PQ-Bild haelt und dessen Ziel sich seit der letzten
    /// Rechnung geaendert hat: vor allem der Kopfraum, der auf einem XDR- oder
    /// eingebauten Schirm erst steigt, nachdem die EDR-Schicht zu sehen ist
    /// (das erste Bild rechnet noch mit Hd 1 - abgeschnitten, kein Gold),
    /// dazu Helligkeit und Schirmwechsel. Bei stillem Bildschirm schickt der
    /// Host nichts, und ohne diesen Anlass blieben Lichter abgeschnitten, bis
    /// sich drueben etwas bewegt. Fragt den Schirm hoechstens alle
    /// PQ_PRUEFTAKT; ohne PQ-Bild kostet es nichts. Verdeckt nie - `zeichnen`
    /// liesse das Bild ohnehin aus, und das Aufdecken holt es nach.
    /// Nur auf dem Hauptfaden.
    pub fn nachzeichnen_faellig(&mut self) -> bool {
        if self.pq_bild.is_none() {
            return false;
        }
        let jetzt = Instant::now();
        if self.pq_geprueft.is_some_and(|t| jetzt.duration_since(t) < PQ_PRUEFTAKT) {
            return false;
        }
        self.pq_geprueft = Some(jetzt);
        if self.verdeckt || !self.fenster_sichtbar() {
            return false;
        }
        let ziel = self.pq_ziel();
        self.pq_bild.as_ref().is_some_and(|p| p.gerechnet.anders(&ziel))
    }

    /// Die Metadaten der Quelle aus der Strominfo Fassung 1 (SDR-Weiss des
    /// Hosts, Mastering-Spitze, MaxCLL); None: ein Host vor 0.2.0 oder noch
    /// keine Strominfo - dann gelten 203 und 1000 nit. main.rs setzt sie vor
    /// jedem Bild; aendern sie sich, rechnet `zeichnen` das letzte PQ-Bild neu.
    pub fn quelle_setzen(&mut self, info: Option<&hdr::InfoV1>) {
        self.quelle = info.copied();
    }

    /// Der Kopfraum des Schirms, auf dem das Fenster steht: (moeglich,
    /// aktuell) aus NSScreen, wie `schirm_lage`. Ohne Ansicht der fuer die
    /// Tests eingestellte. Nur auf dem Hauptfaden.
    fn kopfraum(&self) -> (f32, f32) {
        let Some(ansicht) = &self.ansicht else { return self.kopfraum_ohne_fenster };
        unsafe {
            let schirm = msg_id(msg_id(ansicht.0, c"window"), c"screen");
            if schirm.is_null() {
                return (1.0, 1.0);
            }
            (
                msg_f64(schirm, c"maximumPotentialExtendedDynamicRangeColorComponentValue") as f32,
                msg_f64(schirm, c"maximumExtendedDynamicRangeColorComponentValue") as f32,
            )
        }
    }

    /// Wohin ein PQ-Bild jetzt gehoert: EDR, wenn der Schirm es kann
    /// (moeglicher Kopfraum ueber 1 - der aktuelle steigt erst, wenn eine
    /// Schicht EDR verlangt; nach ihm zu gehen hiesse, nie anzufangen), mit Hd
    /// = aktueller Kopfraum; sonst SDR mit Hd = 1.
    fn pq_ziel(&self) -> PqZiel {
        let (moeglich, aktuell) = self.kopfraum();
        let edr = moeglich > 1.0;
        let hd = if edr && aktuell.is_finite() { aktuell.max(1.0) } else { 1.0 };
        PqZiel { edr, ab: hdr::Abbildung::neu(self.quelle.as_ref(), hd) }
    }

    /// Die HDR-Lage ins Protokoll, wenn sie sich aendert (Weg, Metadaten,
    /// Farbe - nicht jeder Schritt des Kopfraums). None: ein SDR-Strom.
    fn hdr_lage_melden(&mut self, pq: Option<(&hdr::Farbe, &PqZiel)>) {
        let Some((farbe, ziel)) = pq else {
            if self.hdr_stand.take().is_some() {
                protokoll::zeile("Anzeige: SDR-Strom - Stufe 1 wie immer, bitgleich zum CPU-Weg".to_string());
            }
            return;
        };
        let spitze = ziel.ab.weiss_nit * ziel.ab.hs;
        let stand = format!("{} {} {} {}", farbe.text(), ziel.edr, ziel.ab.weiss_nit, ziel.ab.hs);
        if self.hdr_stand.as_deref() == Some(stand.as_str()) {
            return;
        }
        self.hdr_stand = Some(stand);
        protokoll::zeile(if ziel.edr {
            format!(
                "Anzeige: HDR-Strom ({}) auf einem EDR-Schirm: erweitert linear Display P3, abgebildet auf den Kopfraum {:.2}; Quelle Weiss {:.0} nit, Spitze {spitze:.0} nit (Hs {:.2})",
                farbe.text(),
                ziel.ab.hd,
                ziel.ab.weiss_nit,
                ziel.ab.hs
            )
        } else {
            format!(
                "Anzeige: HDR-Strom ({}) auf einem SDR-Schirm: abgebildet auf SDR (farbtontreu beim SDR-Weiss abgeschnitten), BT.709, sRGB; Quelle Weiss {:.0} nit, Spitze {spitze:.0} nit",
                farbe.text(),
                ziel.ab.weiss_nit
            )
        });
    }

    /// Die Schicht auf EDR (RGBA16Float, erweitert linear Display P3, EDR an)
    /// oder zurueck auf SDR (BGRA8 mit dem Farbraum ab Werk, wie immer). EDR nur, solange
    /// ein HDR-Strom laeuft - es kostet Strom. macOS 26 und neuer:
    /// preferredDynamicRange High und contentsHeadroom (die Spitze der Quelle
    /// ueber dem SDR-Weiss); dazu in jedem Fall wantsExtendedDynamicRangeContent,
    /// das CAMetalLayer seit macOS 10.11 kennt. EDRMetadata bleibt nil: die
    /// Abbildung rechnet Stufe 1 selbst, das System soll nichts mehr abbilden.
    fn schicht_umstellen(&mut self, edr: bool) {
        let Some(s) = self.schicht.as_ref().map(|s| s.0) else { return };
        let kopfraum = self.pq_bild.as_ref().map_or(1.0, |p| p.gerechnet.ab.hs.max(1.0));
        let bereich = |name: &CStr| -> Id {
            let p = unsafe { cf::dlsym(cf::RTLD_DEFAULT, name.as_ptr()) } as *const Id;
            if p.is_null() { std::ptr::null_mut() } else { unsafe { *p } }
        };
        let neu = unsafe { senden!(s, sel(c"respondsToSelector:"), sel(c"setPreferredDynamicRange:") => Sel; -> u8) != 0 };
        let dynamik = if neu { bereich(if edr { c"CADynamicRangeHigh" } else { c"CADynamicRangeStandard" }) } else { std::ptr::null_mut() };
        let ab_werk = self.sdr_farbraum.as_ref().map_or(std::ptr::null_mut(), |c| c.0);
        unsafe {
            ohne_animation(|| {
                if edr {
                    msg_void_zahl(s, c"setPixelFormat:", RGBA16_FLOAT);
                    let cs = CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearDisplayP3);
                    senden!(s, sel(c"setColorspace:"), cs => *mut c_void; -> ());
                    if !cs.is_null() {
                        CGColorSpaceRelease(cs);
                    }
                } else {
                    msg_void_zahl(s, c"setPixelFormat:", BGRA8_UNORM);
                    senden!(s, sel(c"setColorspace:"), ab_werk => *mut c_void; -> ());
                }
                msg_void_1(s, c"setEDRMetadata:", std::ptr::null_mut());
                msg_void_bool(s, c"setWantsExtendedDynamicRangeContent:", edr);
                if !dynamik.is_null() {
                    msg_void_1(s, c"setPreferredDynamicRange:", dynamik);
                    senden!(s, sel(c"setContentsHeadroom:"), if edr { kopfraum as f64 } else { 0.0 } => f64; -> ());
                }
            });
        }
        self.schicht_edr = edr;
        protokoll::zeile(if edr {
            format!(
                "Anzeige: Schicht auf EDR umgestellt (RGBA16Float, erweitert linear Display P3, wantsExtendedDynamicRangeContent{})",
                if dynamik.is_null() { String::new() } else { format!(", preferredDynamicRange High, contentsHeadroom {kopfraum:.2}") }
            )
        } else {
            "Anzeige: Schicht zurueck auf SDR (BGRA8)".to_string()
        });
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
        // HDR: hat sich das Ziel des letzten PQ-Bildes geaendert (anderer
        // Schirm, Kopfraum, Metadaten), Stufe 1 daraus neu; dann folgt die
        // Schicht der Zwischentextur - vor nextDrawable, damit der Puffer
        // schon im neuen Format kommt.
        if let Err(e) = self.pq_nachfuehren() {
            return Praesentiert::Fehler(e);
        }
        let edr = self.zwischen.as_ref().is_some_and(|z| z.format == RGBA16_FLOAT);
        if edr != self.schicht_edr {
            self.schicht_umstellen(edr);
        }
        let Some(drawable) = (unsafe { Obj::halten(msg_id(schicht, c"nextDrawable")) }) else {
            return Praesentiert::Fehler("nextDrawable lieferte nichts".into());
        };
        let ziel = unsafe { msg_id(drawable.0, c"texture") };
        let soll = if edr { RGBA16_FLOAT } else { BGRA8_UNORM };
        let ist = unsafe { msg_zahl(ziel, c"pixelFormat") };
        if ist != soll {
            return Praesentiert::Fehler(format!("Puffer der Schicht im Format {ist} statt {soll}"));
        }
        match self.stufe2(ziel, self.breite, self.hoehe, rect, ui_an, Some(drawable.0)) {
            Ok(_) => {
                self.praesentiert.push_back((drawable, Instant::now()));
                self.ausgelassen = false;
                self.hdr_gezeigt = edr && self.pq_bild.as_ref().is_some_and(|p| p.gerechnet.edr && p.gerechnet.ab.hd > 1.0);
                Praesentiert::Ok
            }
            Err(e) => Praesentiert::Fehler(e),
        }
    }

    /// Ein PQ-Strom, und das Ziel hat sich seit dem letzten Bild geaendert
    /// (das Fenster steht auf einem anderen Schirm, der Kopfraum folgt der
    /// Helligkeit, die Strominfo nennt andere Metadaten): Stufe 1 aus dem
    /// gehaltenen Puffer neu rechnen - ohne auf das naechste Bild des Hosts zu
    /// warten. Sonst nichts.
    fn pq_nachfuehren(&mut self) -> Result<(), String> {
        let Some(p) = &self.pq_bild else { return Ok(()) };
        let ziel = self.pq_ziel();
        if !p.gerechnet.anders(&ziel) {
            return Ok(());
        }
        let (pb, fmt, w, h, farbe) = (p.puffer.0 as cf::CVPixelBufferRef, p.fmt, p.w, p.h, p.farbe);
        let (ty, tuv) = unsafe { self.ebenen_texturen(pb, &fmt, w, h)? };
        self.stufe1_pq(ty, tuv, pb, &fmt, w, h, farbe, ziel)?;
        if let Some(p) = self.pq_bild.as_mut() {
            p.gerechnet = ziel;
        }
        Ok(())
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
        let pb = bild.puffer();
        let texturen = unsafe {
            let passt = cf::CVPixelBufferGetWidthOfPlane(pb, 0) >= w as usize
                && cf::CVPixelBufferGetHeightOfPlane(pb, 0) >= h as usize
                && cf::CVPixelBufferGetWidthOfPlane(pb, 1) >= cw as usize
                && cf::CVPixelBufferGetHeightOfPlane(pb, 1) >= ch as usize;
            if !passt {
                return Err("Bildebenen des Decoders sind zu klein".into());
            }
            self.ebenen_texturen(pb, &fmt, w, h)
        };
        let (ty, tuv) = match texturen {
            Ok(t) => t,
            Err(e) => {
                // Ohne IOSurface (oder was der Cache sonst ablehnt): dasselbe
                // Bild auf der CPU umrechnen und fertig hochladen. Langsamer,
                // aber das Bild steht - ein PQ-Bild mit dem SDR-Weiss der
                // Quelle auf SDR abgebildet (to_rgb_mit).
                if !self.cache_rueckfall {
                    protokoll::zeile(format!("Anzeige: {e} - Umrechnung auf der CPU"));
                    self.cache_rueckfall = true;
                }
                let f = crate::to_rgb_mit(bild, self.quelle.as_ref())?;
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
        // HDR10: eigene Shader, das Ziel sagt der Schirm; das Bild bleibt
        // gehalten, bis das naechste kommt.
        let farbe = bild.farbe();
        self.pq_bild = None;
        if farbe.ist_pq() {
            let ziel = self.pq_ziel();
            let cb = self.stufe1_pq(ty, tuv, pb, &fmt, w, h, farbe, ziel)?;
            unsafe { cf::CFRetain(pb as cf::CFTypeRef) };
            self.pq_bild = Some(PqBild { puffer: Cf(pb), fmt, w, h, farbe, gerechnet: ziel });
            return Ok(cb);
        }
        self.hdr_lage_melden(None);
        self.zwischen_sichern(w, h, RGBA8_UINT)?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?.tex.0;
        let k = KonstFormat::neu(&fmt, &farbe);
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

    /// Beide Ebenen eines Puffers als Texturen: Y als R8/R16, U/V als
    /// RG8/RG16, die Farbebenen bei 4:2:0 halb so gross (aufgerundet).
    unsafe fn ebenen_texturen(&self, pb: cf::CVPixelBufferRef, fmt: &crate::EbenenFormat, w: u32, h: u32) -> Result<(Cf, Cf), String> {
        let (cw, ch) = if fmt.sub { ((w + 1) / 2, (h + 1) / 2) } else { (w, h) };
        let (fy, fuv) = if fmt.bits == 8 { (R8_UINT, RG8_UINT) } else { (R16_UINT, RG16_UINT) };
        Ok((self.ebene(pb, fy, w, h, 0)?, self.ebene(pb, fuv, cw, ch, 1)?))
    }

    /// Stufe 1 eines PQ-Bildes: nach SDR in die RGBA8-Zwischentextur (sRGB,
    /// wie ein SDR-Bild) oder nach EDR in eine RGBA16Float-Zwischentextur
    /// (erweitert linear P3). Ebenen und Puffer bleiben gehalten, bis die
    /// Karte fertig ist.
    #[allow(clippy::too_many_arguments)]
    fn stufe1_pq(
        &mut self,
        ty: Cf,
        tuv: Cf,
        pb: cf::CVPixelBufferRef,
        fmt: &crate::EbenenFormat,
        w: u32,
        h: u32,
        farbe: hdr::Farbe,
        ziel: PqZiel,
    ) -> Result<Obj, String> {
        self.hdr_lage_melden(Some((&farbe, &ziel)));
        self.zwischen_sichern(w, h, if ziel.edr { RGBA16_FLOAT } else { RGBA8_UINT })?;
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?.tex.0;
        let k = KonstHdr::neu(&farbe, fmt, &ziel);
        let stufe = if ziel.edr { self.stufe1_hdr_edr.0 } else { self.stufe1_hdr_sdr.0 };
        unsafe {
            let cb = self.befehlspuffer()?;
            let enc = malen(cb.0, z)?;
            msg_void_1(enc, c"setRenderPipelineState:", stufe);
            senden!(enc, sel(c"setFragmentBytes:length:atIndex:"),
                &k as *const KonstHdr as *const c_void => *const c_void, std::mem::size_of::<KonstHdr>() => usize, 0usize => usize; -> ());
            textur_binden(enc, CVMetalTextureGetTexture(ty.0), 0);
            textur_binden(enc, CVMetalTextureGetTexture(tuv.0), 1);
            dreieck(enc);
            cf::CFRetain(pb as cf::CFTypeRef);
            Ok(self.abschicken(cb, vec![ty, tuv, Cf(pb)]))
        }
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
        // Ein fertiges Bild ersetzt auch ein gehaltenes PQ-Bild - sonst
        // rechnete `zeichnen` das alte darueber.
        self.pq_bild = None;
        self.zwischen_sichern(w, h, RGBA8_UINT)?;
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
            self.oberflaeche = Some(Textur { tex, w: ww, h: wh, format: RGBA8_UINT });
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
        // Eine EDR-Zwischentextur (PQ auf EDR) mischt linear in ein
        // RGBA16Float-Ziel, sonst der heutige Weg ins BGRA8-Ziel.
        let stufe = if bild.is_some_and(|z| z.format == RGBA16_FLOAT) { self.stufe2_edr.0 } else { self.stufe2.0 };
        unsafe {
            let cb = self.befehlspuffer()?;
            let enc = malen(cb.0, ziel)?;
            msg_void_1(enc, c"setRenderPipelineState:", stufe);
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

    /// Zwischentextur in Bildgroesse und diesem Format (RGBA8_UINT oder, fuer
    /// PQ auf EDR, RGBA16_FLOAT), neu nur, wenn eins davon nicht stimmt.
    fn zwischen_sichern(&mut self, w: u32, h: u32, format: usize) -> Result<(), String> {
        if self.zwischen.as_ref().map(|z| (z.w, z.h, z.format)) == Some((w, h, format)) {
            return Ok(());
        }
        self.zwischen = None;
        let tex = unsafe { textur_neu(self.device.0, w, h, format, NUTZUNG_LESEN | NUTZUNG_ZIEL)? };
        self.zwischen = Some(Textur { tex, w, h, format });
        Ok(())
    }

    /// Eine Textur in den Hauptspeicher lesen (Blit in einen geteilten
    /// Puffer, dann warten), als 0x00RRGGBB - das vierte Byte maskiert. Nur
    /// fuer die Tests.
    fn auslesen(&mut self, tex: Id, w: u32, h: u32) -> Result<Vec<u32>, String> {
        let roh = self.auslesen_roh(tex, w, h, 4)?;
        Ok(roh.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0x00ff_ffff).collect())
    }

    /// Eine RGBA16Float-Textur als RGBA in f32 lesen (Tests, --anzeigetest).
    fn auslesen_edr(&mut self, tex: Id, w: u32, h: u32) -> Result<Vec<[f32; 4]>, String> {
        let roh = self.auslesen_roh(tex, w, h, 8)?;
        let halb = |p: &[u8], i: usize| halb_nach_f32(u16::from_le_bytes([p[2 * i], p[2 * i + 1]]));
        Ok(roh.chunks_exact(8).map(|p| [halb(p, 0), halb(p, 1), halb(p, 2), halb(p, 3)]).collect())
    }

    /// Die Bytes einer Textur mit `bpp` Byte je Bildpunkt, Zeile an Zeile.
    fn auslesen_roh(&mut self, tex: Id, w: u32, h: u32, bpp: usize) -> Result<Vec<u8>, String> {
        let (w, h) = (w as usize, h as usize);
        let laenge = w * h * bpp;
        let mut aus = vec![0u8; laenge];
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
                puffer.0 => Id, 0usize => usize, w * bpp => usize, laenge => usize; -> ());
            msg_void(blit, c"endEncoding");
            let cb = self.abschicken(cb, Vec::new());
            msg_void(cb.0, c"waitUntilCompleted");
            if msg_zahl(cb.0, c"status") == STATUS_FEHLER {
                return Err(format!("Auslesen: {}", fehlertext(msg_id(cb.0, c"error"))));
            }
            let p = msg_id(puffer.0, c"contents") as *const u8;
            if p.is_null() {
                return Err("Lesepuffer ohne Inhalt".into());
            }
            std::ptr::copy_nonoverlapping(p, aus.as_mut_ptr(), laenge);
        }
        self.aufraeumen();
        Ok(aus)
    }

    /// Nur Test: Stufe 2 in eine eigene Zieltextur (wie ein Puffer der
    /// Schicht: BGRA8), dann zurueck in den Hauptspeicher.
    pub fn offscreen(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<Vec<u32>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        if self.zwischen.as_ref().is_some_and(|z| z.format != RGBA8_UINT) {
            return Err("die Zwischentextur ist EDR - offscreen_edr".into());
        }
        let _pool = Pool::neu();
        let ziel = unsafe { textur_neu(self.device.0, ww, wh, BGRA8_UNORM, NUTZUNG_ZIEL)? };
        self.stufe2(ziel.0, ww, wh, rect, ui_an, None)?;
        self.auslesen(ziel.0, ww, wh)
    }

    /// Wie `offscreen` fuer ein PQ-Bild auf EDR: Stufe 2 in eine
    /// RGBA16Float-Zieltextur (wie ein Puffer der EDR-Schicht), zurueck als
    /// f32 (erweitert linear P3).
    fn offscreen_edr(&mut self, ww: u32, wh: u32, rect: Option<(i32, i32, u32, u32)>, ui_an: bool) -> Result<Vec<[f32; 4]>, String> {
        if ww == 0 || wh == 0 {
            return Err("Zielgroesse null".into());
        }
        if !self.zwischen.as_ref().is_some_and(|z| z.format == RGBA16_FLOAT) {
            return Err("keine EDR-Zwischentextur".into());
        }
        let _pool = Pool::neu();
        let ziel = unsafe { textur_neu(self.device.0, ww, wh, RGBA16_FLOAT, NUTZUNG_ZIEL)? };
        self.stufe2(ziel.0, ww, wh, rect, ui_an, None)?;
        self.auslesen_edr(ziel.0, ww, wh)
    }

    /// Das Ergebnis von Stufe 1 fuer ein PQ-Bild auf EDR, 1:1, als f32.
    fn zwischen_auslesen_edr(&mut self) -> Result<(Vec<[f32; 4]>, u32, u32), String> {
        let _pool = Pool::neu();
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        if z.format != RGBA16_FLOAT {
            return Err("die Zwischentextur ist nicht EDR".into());
        }
        let (tex, w, h) = (z.tex.zweiter(), z.w, z.h);
        Ok((self.auslesen_edr(tex.0, w, h)?, w, h))
    }

    /// Nur Test: das Ergebnis von Stufe 1, 1:1.
    pub fn zwischen_auslesen(&mut self) -> Result<(Vec<u32>, u32, u32), String> {
        let _pool = Pool::neu();
        let z = self.zwischen.as_ref().ok_or("Zwischentextur fehlt")?;
        if z.format != RGBA8_UINT {
            return Err("die Zwischentextur ist EDR - zwischen_auslesen_edr".into());
        }
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
    probebild_farbe(format, w, h, false)
}

/// Wie `probebild`; mit `pq` als HDR10 gekennzeichnet (Anhaenge 2020/PQ/2020
/// wie ein PQ-Bild aus VideoToolbox) und mit dem Muster der PQ-Probebilder
/// (anzeigeprobe::probewert_pq, unten der neutrale Streifen) - dasselbe wie
/// in der Windows-Anzeige.
fn probebild_farbe(format: u32, w: u32, h: u32, pq: bool) -> Result<vt_decoder::Bild, String> {
    probebild_mit(format, w, h, pq, false)
}

/// Wie `probebild`, mit der Matrix BT.601 gekennzeichnet - so traegt ein
/// Bild aus VideoToolbox das VUI des bgra-Wegs eines Windows-Hosts (444v
/// und 420v: BT.601 begrenzt, wie nvenc RGB umrechnet).
fn probebild_bt601(format: u32, w: u32, h: u32) -> Result<vt_decoder::Bild, String> {
    probebild_mit(format, w, h, false, true)
}

fn probebild_mit(format: u32, w: u32, h: u32, pq: bool, bt601: bool) -> Result<vt_decoder::Bild, String> {
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
        let wert = if pq { crate::anzeigeprobe::probewert_pq } else { crate::anzeigeprobe::probewert };
        for y in 0..h {
            for x in 0..w {
                schreib(d0, y * z0 + x * bpp, wert(x, y, w, h, 0, max));
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                schreib(d1, y * z1 + 2 * x * bpp, wert(x, y, cw, ch, 1, max));
                schreib(d1, y * z1 + (2 * x + 1) * bpp, wert(x, y, cw, ch, 2, max));
            }
        }
        cf::CVPixelBufferUnlockBaseAddress(pb, 0);
        if pq {
            cf::pq_kennzeichnen(pb);
        }
        if bt601 {
            cf::bt601_kennzeichnen(pb);
        }
        vt_decoder::Bild::aus_puffer(pb, 0).map_err(|e| e.to_string())
    }
}

/// Ein SDR-Format mit der Matrix BT.601 (und dem Bereich des Formats): Stufe 1
/// gegen `to_rgb` (Toleranz 0) - beide lesen Matrix und Bereich aus dem
/// Bild. Und das VUI muss wirken: dieselben Ebenen ohne Kennzeichen (BT.709)
/// ergeben ein anderes Bild.
fn bt601_pruefen(gpu: &mut Gpu, format: u32, w: u32, h: u32, verzeichnis: Option<&str>, lang: &'static crate::strings::Lang) -> bool {
    use crate::anzeigeprobe::fall_pruefen;
    let name = vt_decoder::fourcc_text(format);
    let (bild, ohne) = match (probebild_bt601(format, w, h), probebild(format, w, h)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            println!("{name:<15} {w:>4}x{h:<4} BT.601: kein Probebild: {e}");
            return false;
        }
    };
    if bild.farbe().matrix != hdr::MATRIX_601 {
        println!("{name:<15} {w:>4}x{h:<4} BT.601: das Bild traegt {} statt BT.601", bild.farbe().text());
        return false;
    }
    let (referenz, bt709) = match (crate::to_rgb(&bild), crate::to_rgb(&ohne)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            println!("{name:<15} {w:>4}x{h:<4} BT.601: to_rgb: {e}");
            return false;
        }
    };
    let mut ok = true;
    if referenz.pixels == bt709.pixels {
        println!("{name:<15} {w:>4}x{h:<4} BT.601      FEHLER: die Matrix wirkt nicht (dasselbe Bild wie BT.709)");
        ok = false;
    }
    match gpu.bild_roh(&bild).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, zw, zh)) if (zw, zh) == (w, h) => ok &= fall_pruefen(&name, w, h, "BT.601", &referenz.pixels, &aus, w, h, 0, verzeichnis, lang),
        Ok((_, zw, zh)) => {
            println!("{name:<15} {w:>4}x{h:<4} BT.601: Zwischentextur ist {zw}x{zh}");
            ok = false;
        }
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} BT.601: {e}");
            ok = false;
        }
    }
    ok
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

/// Die normierten Y'CbCr eines Bildpunkts, wie Stufe 1 sie liest: die Codes
/// aus den Ebenen des Bildes (16 Bit oben buendig >> 6, 8 Bit << 2),
/// normiert nach H.273 (hdr::normieren10).
fn pq_punkt(bild: &vt_decoder::Bild, fmt: &crate::EbenenFormat, x: usize, y: usize) -> (f32, f32, f32) {
    let (cx, cy) = if fmt.sub { (x >> 1, y >> 1) } else { (x, y) };
    let bpp = fmt.bpp() as usize;
    let lese = |ebene: usize, i: usize| -> i32 {
        let d = bild.daten(ebene);
        if bpp == 1 { (d[i] as i32) << 2 } else { (u16::from_le_bytes([d[i], d[i + 1]]) >> 6) as i32 }
    };
    let c0 = cy * bild.zeilenlaenge(1) + 2 * cx * bpp;
    hdr::normieren10(lese(0, y * bild.zeilenlaenge(0) + x * bpp), lese(1, c0), lese(1, c0 + bpp), fmt.begrenzt)
}

/// Stufe 2 auf EDR als Rechnung (Referenz): dieselbe Quellpunktwahl wie
/// ps_anzeigen_edr (16.16), bilinear in f32, Grundfarbe und Oberflaeche
/// (0xTTRRGGBB, vormultipliziert) linear in Display P3.
fn edr_stufe2_referenz(zw: &[[f32; 4]], (fw, fh): (u32, u32), (ww, wh): (u32, u32), rect: (i32, i32, u32, u32), ui: Option<&[u32]>) -> Vec<[f32; 3]> {
    let lin = |c: [f32; 3]| c.map(|v| hdr::srgb_eotf(v as f64) as f32);
    let grund = hdr::mal(&hdr::M_709_NACH_P3, lin([(ui::BG >> 16) & 255, (ui::BG >> 8) & 255, ui::BG & 255].map(|v| v as f32 / 255.0)));
    let schritt = [((fw as u64) << 16) / rect.2 as u64, ((fh as u64) << 16) / rect.3 as u64].map(|s| s as u32);
    let quelle = |x: u32, y: u32| -> [f32; 3] {
        let p = zw[(y * fw + x) as usize];
        [p[0], p[1], p[2]]
    };
    let mische = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    let mut aus = Vec::with_capacity((ww * wh) as usize);
    for y in 0..wh as i32 {
        for x in 0..ww as i32 {
            let (px, py) = (x - rect.0, y - rect.1);
            let mut farbe = grund;
            if px >= 0 && py >= 0 && px < rect.2 as i32 && py < rect.3 as i32 {
                let f = [px as u32 * schritt[0], py as u32 * schritt[1]];
                let q0 = f.map(|v| v >> 16);
                let q1 = [(q0[0] + 1).min(fw - 1), (q0[1] + 1).min(fh - 1)];
                let w = f.map(|v| (v & 65535) as f32 / 65536.0);
                let oben = mische(quelle(q0[0], q0[1]), quelle(q1[0], q0[1]), w[0]);
                let unten = mische(quelle(q0[0], q1[1]), quelle(q1[0], q1[1]), w[0]);
                farbe = mische(oben, unten, w[1]);
            }
            if let Some(ui) = ui {
                let o = ui[(y * ww as i32 + x) as usize];
                let t = (o >> 24) as f32 / 255.0;
                let deckung = 1.0 - t;
                let c = [(o >> 16) & 255, (o >> 8) & 255, o & 255].map(|v| v as f32 / 255.0);
                let l = if deckung > 0.5 / 255.0 { lin(c.map(|v| (v / deckung).min(1.0))).map(|v| v * deckung) } else { lin(c) };
                let p3 = hdr::mal(&hdr::M_709_NACH_P3, l);
                farbe = [0, 1, 2].map(|i| farbe[i] * t + p3[i]);
            }
            aus.push(farbe);
        }
    }
    aus
}

/// Groesste Abweichung einer EDR-Ausgabe (f32 aus halben Gleitkommazahlen)
/// von der Referenz, gemessen an der Toleranz 1e-3 der Helligkeit des
/// Bildpunkts (groesster Anteil) + 1e-4: Ergebnis <= 1 heisst innerhalb. Die
/// Rundung auf binary16 allein macht bis 2,4e-4; relativ zum Punkt und nicht
/// zum einzelnen Anteil, weil ein Anteil ausserhalb des Farbraums als kleine
/// Differenz grosser Glieder (Matrix nach P3) entsteht. Dazu der Ort der
/// schlimmsten Stelle.
fn edr_abweichung(ist: &[[f32; 4]], soll: &[[f32; 3]]) -> (f32, usize) {
    let mut schlimmste = (0f32, 0usize);
    for (i, (a, b)) in ist.iter().zip(soll).enumerate() {
        let hell = b[0].abs().max(b[1].abs()).max(b[2].abs());
        for k in 0..3 {
            let d = (a[k] - b[k]).abs() / (1e-3 * hell + 1e-4);
            let d = if d.is_nan() { f32::INFINITY } else { d };
            if d > schlimmste.0 {
                schlimmste = (d, i);
            }
        }
    }
    schlimmste
}

/// Eine Zeile fuer einen EDR-Vergleich (wie fall_pruefen): die groesste
/// Abweichung in Teilen der Toleranz, wo, und dort Karte und Referenz.
#[allow(clippy::too_many_arguments)]
fn edr_pruefen(name: &str, w: u32, h: u32, fall: &str, ist: &[[f32; 4]], soll: &[[f32; 3]], breite: u32) -> bool {
    if ist.len() != soll.len() {
        println!("{name:<15} {w:>4}x{h:<4} {fall:<11} FEHLER: {} statt {} Bildpunkte", ist.len(), soll.len());
        return false;
    }
    let (d, i) = edr_abweichung(ist, soll);
    let gut = d <= 1.0;
    let (a, b) = (ist.get(i).copied().unwrap_or_default(), soll.get(i).copied().unwrap_or_default());
    println!(
        "{name:<15} {w:>4}x{h:<4} {fall:<11} max {d:.2} der Toleranz bei ({}, {}): Karte {:.5} {:.5} {:.5}, Rechnung {:.5} {:.5} {:.5}  {}",
        i % breite as usize,
        i / breite as usize,
        a[0],
        a[1],
        a[2],
        b[0],
        b[1],
        b[2],
        if gut { "ok" } else { "FEHLER" }
    );
    gut
}

/// Ein PQ-Format in einer Groesse, gegen die Rechnung von hdr.rs: (a) auf
/// einem SDR-Schirm Stufe 1 gegen hdr::pq_nach_srgb8 (Toleranz 1), (b) auf
/// einem EDR-Schirm (Kopfraum 4 moeglich, 2,5 aktuell) Stufe 1 als
/// RGBA16Float gegen pq_nach_edr, (c) Stufe 2 auf EDR skaliert in 700x400
/// mit der Oberflaeche gegen die Rechnung, (d) der Kopfraum steigt auf 3,5:
/// `pq_nachfuehren` rechnet dasselbe Bild neu, (e) zurueck auf dem SDR-Schirm
/// wieder RGBA8. SDR-Weiss der Quelle 240 nit, MaxCLL 1000 nit (Hs 4,17).
fn pq_pruefen(gpu: &mut Gpu, format: u32, w: u32, h: u32, verzeichnis: Option<&str>, lang: &'static crate::strings::Lang, u: &mut ui::Ui) -> bool {
    let name = format!("{} PQ", vt_decoder::fourcc_text(format));
    match probebild_farbe(format, w, h, true) {
        Ok(b) => pq_bild_pruefen(gpu, b, &name, verzeichnis, lang, u),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} kein Probebild: {e}");
            false
        }
    }
}

/// `pq_pruefen` fuer ein gegebenes PQ-Bild (Probebild oder aus dem
/// Hardware-Decoder); die Referenz rechnet mit den Codes in seinen Ebenen.
fn pq_bild_pruefen(gpu: &mut Gpu, bild: vt_decoder::Bild, name: &str, verzeichnis: Option<&str>, lang: &'static crate::strings::Lang, u: &mut ui::Ui) -> bool {
    use crate::anzeigeprobe::{fall_pruefen, oberflaeche_probe};
    let (w, h) = (bild.breite(), bild.hoehe());
    if !bild.farbe().ist_pq() {
        println!("{name:<15} {w:>4}x{h:<4} FEHLER: das Bild traegt {} statt PQ", bild.farbe().text());
        return false;
    }
    let Some(fmt) = vt_decoder::ebenen(bild.format()) else { return false };
    let info = hdr::InfoV1 {
        farbe: hdr::Farbe::PQ,
        grund: hdr::GRUND_AKTIV,
        sdr_weiss_nit: 240,
        master_max_nit: 4000,
        master_min_zehntausendstel: 50,
        max_cll: 1000,
        max_fall: 400,
    };
    gpu.quelle_setzen(Some(&info));
    let (wu, hu) = (w as usize, h as usize);
    let punkte: Vec<(f32, f32, f32)> = (0..hu).flat_map(|y| (0..wu).map(move |x| (x, y))).map(|(x, y)| pq_punkt(&bild, &fmt, x, y)).collect();
    let mut ok = true;

    // (a) SDR-Schirm.
    gpu.kopfraum_ohne_fenster = (1.0, 1.0);
    let sdr = hdr::Abbildung::sdr(Some(&info));
    let referenz: Vec<u32> = punkte
        .iter()
        .map(|&(y, cb, cr)| {
            let [r, g, b] = hdr::pq_nach_srgb8(y, cb, cr, &sdr);
            ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        })
        .collect();
    match gpu.bild_roh(&bild).and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, _, _)) => ok &= fall_pruefen(&name, w, h, "PQ-SDR", &referenz, &aus, w, h, 1, verzeichnis, lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ-SDR: {e}");
            ok = false;
        }
    }

    // (b) EDR-Schirm, Stufe 1.
    gpu.kopfraum_ohne_fenster = (4.0, 2.5);
    let edr = hdr::Abbildung::neu(Some(&info), 2.5);
    let soll: Vec<[f32; 3]> = punkte.iter().map(|&(y, cb, cr)| pq_nach_edr(y, cb, cr, &edr)).collect();
    let zw = match gpu.bild_roh(&bild).and_then(|_| gpu.zwischen_auslesen_edr()) {
        Ok((aus, _, _)) => {
            ok &= edr_pruefen(&name, w, h, "PQ-EDR", &aus, &soll, w);
            aus
        }
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} PQ-EDR: {e}");
            return false;
        }
    };
    drop(bild);

    // (c) Stufe 2 auf EDR, skaliert, mit Oberflaeche.
    let (bw, bh) = (700u32, 400u32);
    let mut leer = vec![0xff00_0000u32; (bw * bh) as usize];
    let kasten = {
        let mut c = ui::Canvas::neu(&mut leer, bw as usize, bh as usize);
        oberflaeche_probe(&mut c, u, &format!("{name} {w}x{h} EDR"));
        c.kasten_nehmen().unwrap_or(ui::Rect { x: 0, y: 0, w: 0, h: 0 })
    };
    let rect = crate::ziel_rechteck(bw, bh, w, h, false);
    match gpu.oberflaeche_hochladen(&leer, bw, bh, kasten).and_then(|_| gpu.offscreen_edr(bw, bh, Some(rect), true)) {
        Ok(aus) => {
            let soll = edr_stufe2_referenz(&zw, (w, h), (bw, bh), rect, Some(&leer));
            ok &= edr_pruefen(&name, w, h, "EDR-Stufe-2", &aus, &soll, bw);
        }
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} EDR-Stufe-2: {e}");
            ok = false;
        }
    }

    // (d) Mehr Kopfraum: dasselbe gehaltene Bild neu, ohne neues vom Host.
    gpu.kopfraum_ohne_fenster = (4.0, 3.5);
    let mehr = hdr::Abbildung::neu(Some(&info), 3.5);
    let soll: Vec<[f32; 3]> = punkte.iter().map(|&(y, cb, cr)| pq_nach_edr(y, cb, cr, &mehr)).collect();
    match gpu.pq_nachfuehren().and_then(|_| gpu.zwischen_auslesen_edr()) {
        Ok((aus, _, _)) => ok &= edr_pruefen(&name, w, h, "Kopfraum", &aus, &soll, w),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} Kopfraum: {e}");
            ok = false;
        }
    }

    // (e) Zurueck auf einen SDR-Schirm: wieder RGBA8, wie (a).
    gpu.kopfraum_ohne_fenster = (1.0, 1.0);
    match gpu.pq_nachfuehren().and_then(|_| gpu.zwischen_auslesen()) {
        Ok((aus, _, _)) => ok &= fall_pruefen(&name, w, h, "zurueck-SDR", &referenz, &aus, w, h, 1, verzeichnis, lang),
        Err(e) => {
            println!("{name:<15} {w:>4}x{h:<4} zurueck-SDR: {e}");
            ok = false;
        }
    }
    gpu.quelle_setzen(None);
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
/// true = 99 % der Bilder sind binnen eines Takts fertig. `pq`: dasselbe mit
/// einem HDR10-Strom (die PQ-Shader), Some(false) auf einen SDR-Schirm
/// abgebildet, Some(true) auf einen EDR-Schirm (Kopfraum 4) - beide Stufen
/// dann in RGBA16Float.
fn bildzeit_messen(gpu: &mut Gpu, u: &mut ui::Ui, pq: Option<bool>) -> bool {
    const B: u32 = 2560;
    const H: u32 = 1440;
    const BILDER: usize = 240;
    let takt = Duration::from_nanos(1_000_000_000 / 120);
    let takt_ms = takt.as_secs_f64() * 1000.0;
    let edr = pq == Some(true);
    let art = match pq {
        None => "",
        Some(false) => ", PQ nach SDR",
        Some(true) => ", PQ nach EDR",
    };
    gpu.kopfraum_ohne_fenster = if edr { (4.0, 4.0) } else { (1.0, 1.0) };
    let mut bilder: Vec<vt_decoder::Bild> = Vec::new();
    let mut quelle = String::new();
    match vt_decoder::probe::hevc_444_10_farbe(B, H, 4, pq.is_some()) {
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
                match probebild_farbe(vt_decoder::XF44, B, H, pq.is_some()) {
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
    if bilder.iter().any(|b| b.farbe().ist_pq() != pq.is_some()) {
        println!("Bildzeit{art}: die Bilder tragen die falsche Farbe");
        return false;
    }
    let mut puffer = vec![0xff00_0000u32; (B * H) as usize];
    let kasten = {
        let mut c = ui::Canvas::neu(&mut puffer, B as usize, H as usize);
        crate::anzeigeprobe::oberflaeche_probe(&mut c, u, "Bildzeit 2560x1440 bei 120 Hz");
        c.kasten_nehmen().unwrap_or(ui::Rect { x: 0, y: 0, w: B as i32, h: H as i32 })
    };
    let _pool = Pool::neu();
    let ziel = match unsafe { textur_neu(gpu.device.0, B, H, if edr { RGBA16_FLOAT } else { BGRA8_UNORM }, NUTZUNG_ZIEL) } {
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
    gpu.kopfraum_ohne_fenster = (1.0, 1.0);
    let ueber = fertig.iter().filter(|&&f| f > takt_ms).count();
    let (c_m, c_p, c_x) = kennzahlen(cpu);
    let (k_m, k_p, k_x) = kennzahlen(karte);
    let (f_m, f_p, f_x) = kennzahlen(fertig);
    // "Abgabe bis fertig" beginnt vor der Arbeit im Fensterfaden und
    // enthaelt sie also schon.
    let passt = f_p <= takt_ms;
    println!("Bildzeit 1440p: {BILDER} Bilder {B}x{H} ({quelle}{art}) im 120-Hz-Takt ({takt_ms:.2} ms), {:.2} s", gesamt);
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
/// Farbebenen (w+1)/2; 1920x1080: der Regelfall) gegen den CPU-Weg, dann
/// HDR10 (PQ in xf44, xf20, x444) auf SDR- und EDR-Ziel gegen hdr.rs und
/// danach wieder ein SDR-Bild, zuletzt die Bildzeit bei 1440p und 120 Hz -
/// SDR, PQ nach SDR und PQ nach EDR. Startet weder Host noch Aufnahme und
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
    // SDR mit der Matrix BT.601 aus dem VUI: 444v und 420v wie der bgra-Weg
    // eines Windows-Hosts (BT.601 begrenzt), 444f und 420f voll.
    for format in [vt_decoder::V444, vt_decoder::V420, vt_decoder::F444, vt_decoder::F420] {
        for (w, h) in [(257u32, 131u32), (1920, 1080)] {
            ok &= bt601_pruefen(&mut gpu, format, w, h, Some(verzeichnis), lang);
            for z in protokoll::abholen() {
                println!("    {z}");
            }
        }
    }
    // HDR10: die Formate, in denen PQ kommt (4:4:4 und 4:2:0 10 Bit, voll
    // und begrenzt), gegen hdr.rs; danach ein SDR-Bild - es muss wieder
    // bitgleich sein.
    for format in [vt_decoder::XF44, vt_decoder::XF20, vt_decoder::X444] {
        for (w, h) in [(257u32, 131u32), (1920, 1080)] {
            ok &= pq_pruefen(&mut gpu, format, w, h, Some(verzeichnis), lang, &mut u);
            for z in protokoll::abholen() {
                println!("    {z}");
            }
        }
    }
    ok &= format_pruefen(&mut gpu, vt_decoder::XF44, 257, 131, Some(verzeichnis), lang, &mut u);
    for z in protokoll::abholen() {
        println!("    {z}");
    }
    bildzeit_messen(&mut gpu, &mut u, None);
    bildzeit_messen(&mut gpu, &mut u, Some(false));
    bildzeit_messen(&mut gpu, &mut u, Some(true));
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
        assert_eq!(std::mem::size_of::<KonstFormat>(), 32);
        assert_eq!(std::mem::offset_of!(KonstFormat, begrenzt), 8);
        assert_eq!(std::mem::offset_of!(KonstFormat, c), 16);
        {
            let s = &SHADER[SHADER.find("struct Format {").expect("struct Format")..];
            let lage: Vec<usize> = ["uint sub;", "uint schieb;", "uint begrenzt;", "uint _f;", "int4 c;"]
                .iter()
                .map(|f| s.find(f).unwrap_or_else(|| panic!("{f} fehlt")))
                .collect();
            assert!(lage.windows(2).all(|p| p[0] < p[1]), "Reihenfolge {lage:?}");
        }
        assert_eq!(std::mem::size_of::<KonstAnzeige>(), 48);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, bild), 16);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, modus), 32);
        assert_eq!(std::mem::offset_of!(KonstAnzeige, grund), 36);
        // struct Hdr: drei float4 und ein float4, dann vier uint, vier float.
        assert_eq!(std::mem::size_of::<KonstHdr>(), 96);
        assert_eq!(std::mem::offset_of!(KonstHdr, ycc), 48);
        assert_eq!(std::mem::offset_of!(KonstHdr, sub), 64);
        assert_eq!(std::mem::offset_of!(KonstHdr, begrenzt), 72);
        assert_eq!(std::mem::offset_of!(KonstHdr, kehr_weiss), 80);
        assert_eq!(std::mem::offset_of!(KonstHdr, hd), 88);
        // Reihenfolge im MSL wie im struct.
        let s = &SHADER_HDR[SHADER_HDR.find("struct Hdr").expect("struct Hdr")..];
        let lage: Vec<usize> = ["m0;", "m1;", "m2;", "ycc;", "sub;", "acht;", "begrenzt;", "_f;", "kehr_weiss;", "hs;", "hd;", "_g;"]
            .iter()
            .map(|f| s.find(f).unwrap_or_else(|| panic!("{f} fehlt")))
            .collect();
        assert!(lage.windows(2).all(|p| p[0] < p[1]), "Reihenfolge {lage:?}");
    }

    /// Die Zahlen der HDR-Shader sind die von hdr.rs: PQ, Knie und BT.709 ->
    /// P3 werden aus hdr.rs geschrieben und lesen sich als genau dieselben
    /// f32 zurueck; die sRGB-Kurve steht mit den Zahlen von hdr::srgb_*.
    #[test]
    fn hdr_zahlen_wie_in_rust() {
        let text = shader_quelle();
        let wert = |name: &str| -> String {
            let z = text.lines().find(|z| z.starts_with(&format!("constant float {name} = "))).unwrap_or_else(|| panic!("{name} fehlt"));
            z.split(" = ").nth(1).unwrap().trim_end_matches(';').to_string()
        };
        for (name, soll) in [
            ("PQ_M1", hdr::PQ_M1 as f32),
            ("PQ_M2", hdr::PQ_M2 as f32),
            ("PQ_C1", hdr::PQ_C1 as f32),
            ("PQ_C2", hdr::PQ_C2 as f32),
            ("PQ_C3", hdr::PQ_C3 as f32),
            ("PQ_SPITZE_NIT", hdr::PQ_SPITZE_NIT as f32),
            ("KNIE", hdr::KNIE),
        ] {
            assert_eq!(wert(name).parse::<f32>().unwrap(), soll, "{name}");
        }
        assert_eq!(wert("PQ_M1"), "0.15930176");
        for (r, name) in ["P3_AUS_709_R", "P3_AUS_709_G", "P3_AUS_709_B"].into_iter().enumerate() {
            let z = text.lines().find(|z| z.starts_with(&format!("constant float3 {name} = "))).unwrap_or_else(|| panic!("{name} fehlt"));
            let innen = &z[z.find("float3(").unwrap() + 7..z.rfind(')').unwrap()];
            let zahlen: Vec<f32> = innen.split(", ").map(|t| t.parse().unwrap()).collect();
            assert_eq!(zahlen, hdr::M_709_NACH_P3[r].to_vec(), "{name}");
        }
        // sRGB wie hdr::srgb_oetf / srgb_eotf.
        for t in ["l * 12.92", "1.055 * pow(", "- 0.055", "1.0 / 2.4", "l <= 0.0031308", "v / 12.92", "+ 0.055) / 1.055", "float3(2.4)", "v <= 0.04045"] {
            assert!(SHADER_HDR.contains(t), "{t} fehlt im MSL");
        }
        for e in ["ps_umrechnen_hdr_sdr(", "ps_umrechnen_hdr_edr(", "ps_anzeigen_edr("] {
            assert_eq!(SHADER_HDR.matches(e).count(), 1, "{e}");
        }
    }

    /// Die Koeffizienten und Matrizen fuer Stufe 1: BT.709 aus Kr/Kb (die
    /// Zahlen von ps_umrechnen), BT.2020 aus hdr.rs; Weiss bleibt Weiss in
    /// jedem Weg; auf der Leitung (BT.2020) ist es genau die Matrix aus hdr.rs.
    #[test]
    fn matrizen_und_koeffizienten_fuer_stufe_1() {
        let k = ycc_koeffizienten(hdr::MATRIX_709);
        for (ist, soll) in k.iter().zip([1.5748, 0.1873, 0.4681, 1.8556]) {
            assert!((ist - soll).abs() < 1e-4, "{k:?}");
        }
        assert_eq!(ycc_koeffizienten(hdr::MATRIX_2020_NCL), [hdr::CR_R_2020, hdr::CB_G_2020, hdr::CR_G_2020, hdr::CB_B_2020]);
        assert_eq!(ycc_koeffizienten(2), ycc_koeffizienten(hdr::MATRIX_2020_NCL), "unbekannt gilt als BT.2020");
        assert_eq!(ausgabematrix(hdr::PRIMAER_2020, true), hdr::M_2020_NACH_P3);
        assert_eq!(ausgabematrix(hdr::PRIMAER_2020, false), hdr::M_2020_NACH_709);
        assert_eq!(ausgabematrix(2, true), hdr::M_2020_NACH_P3);
        assert_eq!(ausgabematrix(hdr::PRIMAER_709, true), hdr::M_709_NACH_P3);
        for p in [hdr::PRIMAER_709, hdr::PRIMAER_2020, vt_decoder::PRIMAER_P3] {
            for edr in [false, true] {
                for w in hdr::mal(&ausgabematrix(p, edr), [1.0, 1.0, 1.0]) {
                    assert!((w - 1.0).abs() < 1e-5, "Primaerfarben {p}, EDR {edr}: Weiss {w}");
                }
            }
        }
        // Die Konstanten eines Bildes: 4:4:4 16 Bit voll, SDR-Ziel -> Hd 1.
        let fmt = vt_decoder::ebenen(vt_decoder::XF44).unwrap();
        let ab = hdr::Abbildung::neu(None, 3.0);
        let k = KonstHdr::neu(&hdr::Farbe::PQ, &fmt, &PqZiel { edr: false, ab });
        assert_eq!((k.sub, k.acht, k.begrenzt, k.hd), (0, 0, 0, 1.0));
        assert_eq!(k.kehr_weiss, 1.0 / hdr::SDR_WEISS_VORGABE_NIT);
        assert_eq!(KonstHdr::neu(&hdr::Farbe::PQ, &fmt, &PqZiel { edr: true, ab }).hd, 3.0);
        let fmt = vt_decoder::ebenen(vt_decoder::V420).unwrap();
        let k = KonstHdr::neu(&hdr::Farbe::PQ, &fmt, &PqZiel { edr: true, ab });
        assert_eq!((k.sub, k.acht, k.begrenzt), (1, 1, 1));
    }

    /// Neu rechnen nur, wenn es sich lohnt: anderer Weg oder andere
    /// Metadaten immer, der Kopfraum erst ab einem Prozent.
    #[test]
    fn pq_ziel_anders() {
        let info = hdr::InfoV1 { farbe: hdr::Farbe::PQ, grund: 0, sdr_weiss_nit: 203, master_max_nit: 1000, master_min_zehntausendstel: 50, max_cll: 0, max_fall: 0 };
        let z = |edr: bool, hd: f32, i: &hdr::InfoV1| PqZiel { edr, ab: hdr::Abbildung::neu(Some(i), hd) };
        let a = z(true, 2.0, &info);
        assert!(!a.anders(&a));
        assert!(!a.anders(&z(true, 2.015, &info)), "unter einem Prozent");
        assert!(a.anders(&z(true, 2.05, &info)));
        assert!(a.anders(&z(false, 2.0, &info)));
        assert!(a.anders(&z(true, 2.0, &hdr::InfoV1 { sdr_weiss_nit: 240, ..info })));
        assert!(a.anders(&z(true, 2.0, &hdr::InfoV1 { max_cll: 600, ..info })));
    }

    /// Halbe Gleitkommazahlen zurueck nach f32.
    #[test]
    fn halb_nach_f32_wie_ieee() {
        for (h, x) in [(0x0000u16, 0.0f32), (0x3c00, 1.0), (0x4100, 2.5), (0x4a40, 12.5), (0x7bff, 65504.0), (0xc000, -2.0), (0x0001, 5.960_464_5e-8), (0x0400, 6.103_515_6e-5)] {
            assert_eq!(halb_nach_f32(h), x, "{h:04x}");
        }
        assert!(halb_nach_f32(0x7c00).is_infinite() && halb_nach_f32(0x7e00).is_nan());
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

    /// Die Matrix aus dem VUI (BT.601, wie der bgra-Weg eines Windows-Hosts
    /// sie schreibt): 420v und 444v begrenzt, 420f voll - Stufe 1 bitgleich
    /// zu to_rgb, und anders als BT.709.
    #[test]
    fn goldbild_bt601() {
        let Some(mut g) = geraet() else { return };
        let lang = crate::strings::pick("de");
        for format in [vt_decoder::V420, vt_decoder::V444, vt_decoder::F420] {
            for (w, h) in [(257u32, 131u32), (64, 36)] {
                assert!(bt601_pruefen(&mut g, format, w, h, None, lang), "{} BT.601 {w}x{h}", vt_decoder::fourcc_text(format));
            }
        }
        // Danach wieder BT.709: bitgleich wie vorher.
        let mut u = ui::Ui::new();
        u.tick = 40;
        u.mouse = (-1, -1);
        assert!(format_pruefen(&mut g, vt_decoder::F420, 64, 36, None, lang, &mut u), "BT.709 nach BT.601");
    }

    /// Stufe 1 fuer SDR: Bereich aus Format oder VUI, Matrix aus dem VUI.
    #[test]
    fn stufe1_sdr_nach_dem_vui() {
        let fmt = crate::EbenenFormat { sub: true, bits: 8, paar: true, begrenzt: false };
        assert_eq!(KonstFormat::neu(&fmt, &hdr::Farbe::SDR), KonstFormat { sub: 1, schieb: 0, begrenzt: 0, _f: 0, c: hdr::SDR_KOEFF_709 });
        let bt601 = hdr::Farbe { matrix: hdr::MATRIX_601, voll: false, ..hdr::Farbe::SDR };
        assert_eq!(KonstFormat::neu(&fmt, &bt601), KonstFormat { sub: 1, schieb: 0, begrenzt: 1, _f: 0, c: hdr::SDR_KOEFF_601 });
        let v420 = crate::EbenenFormat { begrenzt: true, ..fmt };
        assert_eq!(KonstFormat::neu(&v420, &hdr::Farbe::SDR).begrenzt, 1, "begrenzt laut Format");
        assert_eq!(hdr::SDR_KOEFF_709, [103206, 12276, 30681, 121609], "BT.709 bitgleich zu frueher");
    }

    fn goldbild_pq(format: u32) {
        let Some(mut g) = geraet() else { return };
        let mut u = ui::Ui::new();
        u.tick = 40;
        u.mouse = (-1, -1);
        let lang = crate::strings::pick("de");
        for (w, h) in [(257u32, 131u32), (64, 36)] {
            assert!(pq_pruefen(&mut g, format, w, h, None, lang, &mut u), "{} PQ {w}x{h}", vt_decoder::fourcc_text(format));
        }
        // Danach ein SDR-Bild: wieder bitgleich zum CPU-Weg.
        assert!(format_pruefen(&mut g, vt_decoder::XF44, 64, 36, None, lang, &mut u), "SDR nach PQ");
        assert!(g.pq_bild.is_none(), "ein SDR-Bild laesst das PQ-Bild los");
    }

    /// HDR10 4:4:4 10 Bit (xf44, wie ein PQ-Strom des Mac-Hosts): auf einem
    /// SDR-Schirm gegen hdr::pq_nach_srgb8 (+-1), auf einem EDR-Schirm als
    /// RGBA16Float gegen hdr.rs (1e-3), Stufe 2 in linearem Licht mit der
    /// Oberflaeche, Nachfuehren bei neuem Kopfraum und zurueck auf SDR.
    #[test]
    fn goldbild_pq_444_10() {
        goldbild_pq(vt_decoder::XF44);
    }

    /// Dasselbe mit 4:2:0 10 Bit (xf20, Kandidat 2).
    #[test]
    fn goldbild_pq_420_10() {
        goldbild_pq(vt_decoder::XF20);
    }

    /// Dasselbe im begrenzten Bereich (x444): die Normierung nach H.273.
    #[test]
    fn goldbild_pq_begrenzt() {
        goldbild_pq(vt_decoder::X444);
    }

    /// Ende zu Ende: ein HDR10-Strom aus dem Hardware-Encoder (2020/PQ/2020),
    /// vom Hardware-Decoder zurueck (die Farbe aus der Formatbeschreibung),
    /// durch beide Wege der Anzeige gegen hdr.rs mit den decodierten Codes.
    /// Ohne Hardware-Encoder (Sandbox) uebersprungen.
    #[test]
    fn pq_aus_dem_hardware_decoder() {
        let Some(mut g) = geraet() else { return };
        let p = match vt_decoder::probe::hevc_444_10_farbe(256, 128, 2, true) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("uebersprungen: {e}");
                return;
            }
        };
        let mut d = vt_decoder::Decoder::neu(false, true);
        let mut bilder = Vec::new();
        for (i, au) in p.einheiten.iter().enumerate() {
            d.fuettern(au, i as i64, &mut bilder).expect("decodieren");
        }
        let bild = bilder.pop().expect("kein Bild");
        assert_eq!(bild.farbe(), hdr::Farbe::PQ);
        let mut u = ui::Ui::new();
        u.tick = 40;
        u.mouse = (-1, -1);
        assert!(pq_bild_pruefen(&mut g, bild, "HW PQ", None, crate::strings::pick("de"), &mut u));
    }

    /// Die EDR-Schicht, ohne Fenster: ein PQ-Bild auf einem Schirm mit
    /// Kopfraum stellt die Schicht beim naechsten Zeichnen auf RGBA16Float,
    /// erweitert linear P3 und EDR um, und die Anzeige meldet HDR (Gold).
    /// Ohne aktuellen Kopfraum bleibt die Schicht EDR, meldet aber kein HDR;
    /// ein SDR-Schirm stellt sie ohne neues Bild zurueck (Stufe 1 aus dem
    /// gehaltenen Bild), ein SDR-Bild laesst das PQ-Bild los.
    #[test]
    fn pq_auf_der_schicht() {
        if unsafe { Obj::eigen(MTLCreateSystemDefaultDevice()) }.is_none() {
            eprintln!("uebersprungen: kein Metal-Geraet");
            return;
        }
        let _pool = Pool::neu();
        let wurzel = unsafe { Obj::halten(msg_id(klasse(c"CALayer"), c"layer")).expect("CALayer") };
        unsafe {
            senden!(wurzel.0, sel(c"setContentsScale:"), 1.0f64 => f64; -> ());
            senden!(wurzel.0, sel(c"setBounds:"), CgRect { x: 0.0, y: 0.0, w: 320.0, h: 180.0 } => CgRect; -> ());
        }
        let mut g = Gpu::an_schicht(wurzel.zweiter(), 320, 180).expect("Schicht");
        let schicht = g.schicht.as_ref().map(|s| s.0).expect("Schicht");
        // Format, Farbraum (ab Werk: 0, erweitert linear P3: 1, anderer: 2), EDR.
        let ab_werk = unsafe { senden!(schicht, sel(c"colorspace"); -> Id) };
        let p3 = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearDisplayP3) };
        let stand = |s: Id| unsafe {
            let cs = senden!(s, sel(c"colorspace"); -> Id);
            let welcher = if cs == ab_werk {
                0
            } else if !cs.is_null() && cf::CFEqual(cs as cf::CFTypeRef, p3 as cf::CFTypeRef) != 0 {
                1
            } else {
                2
            };
            (msg_zahl(s, c"pixelFormat"), welcher, msg_bool(s, c"wantsExtendedDynamicRangeContent"))
        };
        let rect = Some((0, 0, 320, 180));
        let zeichnen = |g: &mut Gpu| {
            // Nie mehr als zwei Bilder unterwegs: ohne Fenster kommt keins an,
            // nach der Frist ist die Schicht wieder bereit.
            if !g.bereit() {
                std::thread::sleep(PRAESENT_FRIST + Duration::from_millis(20));
                assert!(g.bereit());
            }
            assert!(matches!(g.zeichnen(rect, false, false), Praesentiert::Ok));
        };
        assert_eq!(stand(schicht), (BGRA8_UNORM, 0, false), "ab Werk SDR");
        g.kopfraum_ohne_fenster = (4.0, 2.5);
        let bild = probebild_farbe(vt_decoder::XF44, 320, 180, true).expect("Probebild");
        g.bild_roh(&bild).expect("bild_roh");
        drop(bild);
        assert!(!g.hdr_praesentiert(), "vor dem Zeichnen nichts gezeigt");
        zeichnen(&mut g);
        assert_eq!(stand(schicht), (RGBA16_FLOAT, 1, true), "EDR");
        assert!(g.hdr_praesentiert());
        unsafe {
            if senden!(schicht, sel(c"respondsToSelector:"), sel(c"preferredDynamicRange") => Sel; -> u8) != 0 {
                let bereich = rust_text(msg_id(schicht, c"preferredDynamicRange"));
                assert!(bereich.to_lowercase().contains("high"), "preferredDynamicRange {bereich}");
                assert!((msg_f64(schicht, c"contentsHeadroom") - (1000.0 / 203.0)).abs() < 1e-3);
            }
        }
        // Kopfraum 1 (etwa volle Helligkeit ohne Reserve): EDR, aber kein Gold.
        g.kopfraum_ohne_fenster = (4.0, 1.0);
        zeichnen(&mut g);
        assert_eq!(stand(schicht).0, RGBA16_FLOAT);
        assert!(!g.hdr_praesentiert(), "ohne Kopfraum kein HDR");
        assert_eq!(g.pq_bild.as_ref().map(|p| p.gerechnet.ab.hd), Some(1.0), "Stufe 1 neu mit Hd 1");
        // Wieder Kopfraum, dann ein SDR-Schirm - ohne neues Bild vom Host.
        g.kopfraum_ohne_fenster = (4.0, 3.0);
        zeichnen(&mut g);
        assert!(g.hdr_praesentiert());
        g.kopfraum_ohne_fenster = (1.0, 1.0);
        zeichnen(&mut g);
        assert_eq!(stand(schicht), (BGRA8_UNORM, 0, false), "zurueck auf SDR, Farbraum ab Werk");
        assert!(!g.hdr_praesentiert());
        assert_eq!(g.zwischen.as_ref().map(|z| z.format), Some(RGBA8_UINT));
        // Ein SDR-Bild auf einem EDR-Schirm: die Schicht bleibt SDR.
        g.kopfraum_ohne_fenster = (4.0, 2.5);
        let bild = probebild(vt_decoder::XF44, 320, 180).expect("Probebild");
        g.bild_roh(&bild).expect("bild_roh");
        assert!(g.pq_bild.is_none());
        zeichnen(&mut g);
        assert_eq!(stand(schicht), (BGRA8_UNORM, 0, false), "SDR-Strom bleibt SDR");
        assert!(!g.hdr_praesentiert());
        unsafe { CGColorSpaceRelease(p3) };
        drop(g);
        assert_eq!(unsafe { msg_zahl(msg_id(wurzel.0, c"sublayers"), c"count") }, 0);
    }

    /// Durchsicht 5a: ein XDR- oder eingebauter Schirm hat vor der
    /// EDR-Anforderung den aktuellen Kopfraum 1 - das erste PQ-Bild rechnet mit
    /// Hd 1, abgeschnitten, kein Gold. Steigt der Kopfraum danach, kommt bei
    /// stillem Bildschirm kein neues Bild; `nachzeichnen_faellig` gibt den
    /// Anlass, und das gehaltene Bild erreicht den Kopfraum (Gold). Ebenso
    /// Helligkeit und Schirmwechsel. Hoechstens alle PQ_PRUEFTAKT gefragt,
    /// ohne PQ-Bild nie.
    #[test]
    fn pq_folgt_dem_kopfraum_ohne_neues_bild() {
        if unsafe { Obj::eigen(MTLCreateSystemDefaultDevice()) }.is_none() {
            eprintln!("uebersprungen: kein Metal-Geraet");
            return;
        }
        let _pool = Pool::neu();
        let wurzel = unsafe { Obj::halten(msg_id(klasse(c"CALayer"), c"layer")).expect("CALayer") };
        unsafe {
            senden!(wurzel.0, sel(c"setContentsScale:"), 1.0f64 => f64; -> ());
            senden!(wurzel.0, sel(c"setBounds:"), CgRect { x: 0.0, y: 0.0, w: 320.0, h: 180.0 } => CgRect; -> ());
        }
        let mut g = Gpu::an_schicht(wurzel.zweiter(), 320, 180).expect("Schicht");
        let zeichnen = |g: &mut Gpu| {
            if !g.bereit() {
                std::thread::sleep(PRAESENT_FRIST + Duration::from_millis(20));
                assert!(g.bereit());
            }
            assert!(matches!(g.zeichnen(Some((0, 0, 320, 180)), false, false), Praesentiert::Ok));
        };
        // Ohne den Takt: jede Frage gilt.
        let faellig = |g: &mut Gpu| {
            g.pq_geprueft = None;
            g.nachzeichnen_faellig()
        };
        assert!(!faellig(&mut g), "ohne PQ-Bild nie");
        // EDR moeglich, aktuell noch 1 (vor der Anforderung).
        g.kopfraum_ohne_fenster = (16.0, 1.0);
        let bild = probebild_farbe(vt_decoder::XF44, 320, 180, true).expect("Probebild");
        g.bild_roh(&bild).expect("bild_roh");
        drop(bild);
        zeichnen(&mut g);
        assert!(g.schicht_edr, "die Schicht verlangt EDR");
        assert!(!g.hdr_praesentiert(), "Hd 1: abgeschnitten, kein Gold");
        assert!(!faellig(&mut g), "nichts geaendert");
        // Die EDR-Schicht ist zu sehen, der Kopfraum steigt - kein neues Bild.
        g.kopfraum_ohne_fenster = (16.0, 2.5);
        assert!(faellig(&mut g), "Kopfraum gestiegen");
        assert!(!g.nachzeichnen_faellig(), "binnen PQ_PRUEFTAKT nicht noch einmal gefragt");
        zeichnen(&mut g);
        assert_eq!(g.pq_bild.as_ref().map(|p| p.gerechnet.ab.hd), Some(2.5));
        assert!(g.hdr_praesentiert(), "voller Kopfraum, Gold");
        assert!(!faellig(&mut g), "nachgefuehrt");
        // Unter einem Prozent: kein Anlass; Helligkeit spuerbar geaendert: ja.
        g.kopfraum_ohne_fenster = (16.0, 2.51);
        assert!(!faellig(&mut g));
        g.kopfraum_ohne_fenster = (16.0, 1.8);
        assert!(faellig(&mut g), "Helligkeit");
        zeichnen(&mut g);
        assert!(g.hdr_praesentiert());
        // Verdeckt: kein Anlass (zeichnen liesse es ohnehin aus).
        g.kopfraum_ohne_fenster = (16.0, 3.0);
        g.verdeckt = true;
        assert!(!faellig(&mut g), "verdeckt");
        g.verdeckt = false;
        assert!(faellig(&mut g), "wieder sichtbar");
        zeichnen(&mut g);
        // Auf einen SDR-Schirm geschoben: zurueck auf SDR, kein Gold.
        g.kopfraum_ohne_fenster = (1.0, 1.0);
        assert!(faellig(&mut g), "Schirmwechsel");
        zeichnen(&mut g);
        assert!(!g.schicht_edr);
        assert!(!g.hdr_praesentiert());
        assert!(!faellig(&mut g));
        // Ein SDR-Bild laesst das PQ-Bild los: kein Anlass mehr.
        g.kopfraum_ohne_fenster = (16.0, 2.5);
        let bild = probebild(vt_decoder::XF44, 320, 180).expect("Probebild");
        g.bild_roh(&bild).expect("bild_roh");
        drop(bild);
        assert!(!faellig(&mut g), "SDR-Strom");
        drop(g);
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
