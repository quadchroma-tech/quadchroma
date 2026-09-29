// Der Decoder des Mac-Clients: VideoToolbox direkt, ohne FFmpeg.
//
// Der Host schickt jede Zugriffseinheit im Annex-B-Format (Startcodes
// 00 00 01 bzw. 00 00 00 01), ein Schluesselbild mit seinen
// Parametersaetzen davor (HEVC: VPS, SPS, PPS; H.264: SPS, PPS).
// VideoToolbox will es anders: die Parametersaetze in einer
// Formatbeschreibung (CMVideoFormatDescriptionCreateFromHEVCParameterSets
// bzw. ...FromH264ParameterSets) und die uebrigen NALs mit einem Laengen-
// praefix von vier Byte (wie in MP4). Der Umbau steht hier; er ist reines
// Rechnen und wird auf jeder Plattform geprueft, auch unter Windows.
//
// Die Sitzung (VTDecompressionSession) entsteht erst mit den ersten
// Parametersaetzen und neu, sobald andere kommen. Bis dahin nimmt der
// Decoder Pakete an, ohne ein Bild zu liefern - wie cuvids Parser unter
// Windows. Er decodiert synchron (ohne kVTDecodeFrame_EnableAsynchronous-
// Decompression und ohne zeitliche Umsortierung): der Rueckruf kommt, bevor
// VTDecompressionSessionDecodeFrame zurueckkehrt, und jedes Paket liefert
// sein Bild sofort - der Host codiert ohne Bildumsortierung.
//
// Hardware heisst hier: kVTVideoDecoderSpecification_RequireHardware-
// AcceleratedVideoDecoder. Kann die Media-Engine den Strom nicht, scheitert
// schon das Anlegen der Sitzung am ersten Paket, und die Empfangsschleife
// faellt wie bei NVDEC auf den Prozessor zurueck - hier VideoToolbox ohne
// Hardware. Gemessen auf dem M1 (macOS 27): HEVC Main 4:4:4 10 in Hardware,
// in der Probe (vt444test) 1080p 3,8 ms je Bild, 1440p 6,2 ms; im Client
// selbst (Decodierzeit seiner Statistik) rund 5,6 ms je 1080p-Bild.
//
// Faellt die Sitzung mitten im Strom aus (Ruhezustand, Aussetzer der
// Media-Engine, siehe `sitzung_verwerfen`), wird sie verworfen und mit dem
// naechsten Schluesselbild neu angelegt; der Empfang wartet so lange
// (`Decoder::schluesselbild_noetig`). Fehler kommen gedrosselt ins
// Protokoll (`Zeilendrossel`).
//
// Heraus kommen CVPixelBuffer mit IOSurface (die Metal-Anzeige nimmt sie ohne
// Kopie, anzeige_mac.rs) im Format, das zum Strom passt: xf44 fuer 4:4:4 10 Bit,
// 444f fuer 4:4:4 8 Bit, xf20 (wie P010) und 420f (NV12) fuer 4:2:0 - alle
// zweiebenig mit U/V als Paaren, 10 Bit oben buendig in 16. Das liest
// `zeile_rgb` in main.rs ohne eigenen Pfad (xf44: sub=false, 16, paar=true).
// Der Wertebereich der Ausgabe folgt dem, was die Formatbeschreibung sagt:
// so rechnet VideoToolbox nichts um, und die Werte kommen roh an wie aus
// FFmpeg unter Windows. Den begrenzten Bereich (x444, 444v, x420, 420v)
// dehnen `zeile_rgb` und die Metal-Anzeige selbst.
//
// HDR10: Neben dem Bereich liest die Sitzung aus der Formatbeschreibung
// (also aus dem VUI des SPS) Primaerfarben, Transfer und Matrix; jedes Bild
// traegt diese Farbe (`Bild::farbe`, hdr::Farbe). Auch ein PQ-Strom kommt
// als xf44/xf20 mit den rohen Codes heraus - VideoToolbox rechnet nichts um,
// PQ und die Tonwertabbildung rechnet die Anzeige.

// Unter Windows laufen nur die Tests des reinen Teils.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::sps;

/// NAL-Typen, die der Umbau kennt. HEVC: Typ in den Bits 1..6 des ersten
/// Kopfbytes (zwei Kopfbytes), H.264: in den Bits 0..4 (ein Kopfbyte).
pub const HEVC_VPS: u8 = 32;
pub const HEVC_SPS: u8 = 33;
pub const HEVC_PPS: u8 = 34;
pub const HEVC_AUD: u8 = 35;
pub const H264_SPS: u8 = 7;
pub const H264_PPS: u8 = 8;
pub const H264_AUD: u8 = 9;

/// Typ eines NAL (ohne Startcode).
pub fn nal_typ(nal: &[u8], h264: bool) -> u8 {
    match nal.first() {
        Some(&k) if h264 => k & 0x1f,
        Some(&k) => (k >> 1) & 0x3f,
        None => 0xff,
    }
}

/// Die Parametersaetze eines Stroms, je Art in der Reihenfolge des Stroms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parametersaetze {
    pub vps: Vec<Vec<u8>>,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

impl Parametersaetze {
    /// Genug fuer eine Formatbeschreibung? HEVC braucht alle drei Arten,
    /// H.264 SPS und PPS.
    pub fn vollstaendig(&self, h264: bool) -> bool {
        (h264 || !self.vps.is_empty()) && !self.sps.is_empty() && !self.pps.is_empty()
    }

    /// In der Reihenfolge, die CoreMedia erwartet: VPS, SPS, PPS.
    pub fn liste(&self, h264: bool) -> Vec<&[u8]> {
        let vps = if h264 { &[][..] } else { &self.vps[..] };
        vps.iter().chain(&self.sps).chain(&self.pps).map(Vec::as_slice).collect()
    }

    /// Die bisherigen Saetze, ersetzt durch die neuen: jede Art, die in
    /// `neu` vorkommt, gilt ganz neu (ein Schluesselbild traegt alle Saetze
    /// seiner Art), jede andere bleibt.
    pub fn ergaenzt(&self, neu: &Parametersaetze) -> Parametersaetze {
        let waehle = |alt: &Vec<Vec<u8>>, neu: &Vec<Vec<u8>>| if neu.is_empty() { alt.clone() } else { neu.clone() };
        Parametersaetze { vps: waehle(&self.vps, &neu.vps), sps: waehle(&self.sps, &neu.sps), pps: waehle(&self.pps, &neu.pps) }
    }

    pub fn leer(&self) -> bool {
        self.vps.is_empty() && self.sps.is_empty() && self.pps.is_empty()
    }
}

/// Eine Zugriffseinheit, zerlegt fuer VideoToolbox.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Zerlegt {
    /// Die Parametersaetze, die in der Einheit standen (leer: keine).
    pub saetze: Parametersaetze,
    /// Die uebrigen NALs mit vier Byte Laenge (big-endian) davor - ohne
    /// Parametersaetze und ohne Zugriffseinheiten-Begrenzer.
    pub probe: Vec<u8>,
}

/// Annex B zerlegen: Parametersaetze heraus, der Rest mit Laengenpraefix.
/// Nullen am Ende eines NAL gehoeren zum naechsten Startcode (ein NAL endet
/// nie auf 00 - endet seine RBSP so, steht ein 03 dahinter); ohne Startcode
/// am Anfang zaehlt nichts vor dem ersten.
pub fn zerlegen(au: &[u8], h264: bool) -> Zerlegt {
    let mut z = Zerlegt { saetze: Parametersaetze::default(), probe: Vec::with_capacity(au.len() + 16) };
    for (a, e) in sps::nal_bereiche(au) {
        let nal = &au[a..e];
        // Ein NAL kuerzer als sein Kopf ist Muell.
        if nal.len() < if h264 { 1 } else { 2 } {
            continue;
        }
        let typ = nal_typ(nal, h264);
        match (h264, typ) {
            (false, HEVC_VPS) => z.saetze.vps.push(nal.to_vec()),
            (false, HEVC_SPS) | (true, H264_SPS) => z.saetze.sps.push(nal.to_vec()),
            (false, HEVC_PPS) | (true, H264_PPS) => z.saetze.pps.push(nal.to_vec()),
            (false, HEVC_AUD) | (true, H264_AUD) => {}
            _ => {
                z.probe.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                z.probe.extend_from_slice(nal);
            }
        }
    }
    z
}

/// Was aus einem SPS fuer die Wahl des Ausgabeformats zaehlt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bildart {
    /// chroma_format_idc: 0 einfarbig, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma: u32,
    /// Bittiefe der Helligkeit (bit_depth_luma_minus8 + 8).
    pub bits: u32,
    /// general_profile_idc (HEVC) bzw. profile_idc (H.264), fuer das Protokoll.
    pub profil: u32,
}

/// HEVC-SPS (7.3.2.2.1) bis zu den Bittiefen lesen. `nal` mit den zwei
/// Kopfbytes, ohne Startcode, mit Emulationsschutz.
pub fn hevc_sps_lesen(nal: &[u8]) -> Option<Bildart> {
    if nal.len() < 3 || nal_typ(nal, false) != HEVC_SPS {
        return None;
    }
    let rbsp = sps::entschuetzen(&nal[2..]);
    let mut l = sps::BitLeser::neu(&rbsp);
    let _vps_id = l.u(4)?;
    let max_sub_layers_minus1 = l.u(3)?;
    let _temporal_id_nesting = l.u1()?;
    // profile_tier_level(1, max_sub_layers_minus1), 7.3.3: profile_space,
    // tier und profile_idc (8 Bit), 32 Kompatibilitaetsbits, 4 Quellbits,
    // 43 Bit Einschraenkungen und ein weiteres (80 Bit), level_idc (8).
    let _space_tier = l.u(3)?;
    let profil = l.u(5)?;
    for _ in 0..80 / 16 {
        l.u(16)?;
    }
    let _level = l.u(8)?;
    let mut profil_da = [false; 8];
    let mut stufe_da = [false; 8];
    for i in 0..max_sub_layers_minus1 as usize {
        profil_da[i] = l.u1()?;
        stufe_da[i] = l.u1()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            l.u(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if profil_da[i] {
            // 88 Bit: wie oben ohne level_idc.
            for _ in 0..11 {
                l.u(8)?;
            }
        }
        if stufe_da[i] {
            l.u(8)?;
        }
    }
    let _sps_id = l.ue()?;
    let chroma = l.ue()?;
    if chroma == 3 {
        let _separate_colour_plane = l.u1()?;
    }
    let _breite = l.ue()?;
    let _hoehe = l.ue()?;
    if l.u1()? {
        for _ in 0..4 {
            l.ue()?;
        }
    }
    // Geprueft: ein kaputtes SPS kann hier bis 2^32 - 2 tragen.
    let bits = l.ue()?.checked_add(8)?;
    let _bits_chroma = l.ue()?;
    (chroma <= 3 && bits <= 16).then_some(Bildart { chroma, bits, profil })
}

/// H.264-SPS (7.3.2.1.1) bis zu den Bittiefen lesen. Profile ohne die
/// Felder sind 4:2:0 mit 8 Bit.
pub fn h264_sps_lesen(nal: &[u8]) -> Option<Bildart> {
    if nal.len() < 4 || nal_typ(nal, true) != H264_SPS {
        return None;
    }
    let rbsp = sps::entschuetzen(&nal[1..]);
    let mut l = sps::BitLeser::neu(&rbsp);
    let profil = l.u(8)?;
    let _constraint = l.u(8)?;
    let _level = l.u(8)?;
    let _sps_id = l.ue()?;
    const MIT_CHROMA: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];
    if !MIT_CHROMA.contains(&profil) {
        return Some(Bildart { chroma: 1, bits: 8, profil });
    }
    let chroma = l.ue()?;
    if chroma == 3 {
        let _separate_colour_plane = l.u1()?;
    }
    let bits = l.ue()?.checked_add(8)?;
    (chroma <= 3 && bits <= 16).then_some(Bildart { chroma, bits, profil })
}

/// Vier Zeichen als Zahl, wie CoreVideo seine Formate nennt.
pub const fn fourcc(t: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*t)
}

/// Die Formate, die der Decoder bestellt, voller und begrenzter Wertebereich.
pub const XF44: u32 = fourcc(b"xf44");
pub const X444: u32 = fourcc(b"x444");
pub const F444: u32 = fourcc(b"444f");
pub const V444: u32 = fourcc(b"444v");
pub const XF20: u32 = fourcc(b"xf20");
pub const X420: u32 = fourcc(b"x420");
pub const F420: u32 = fourcc(b"420f");
pub const V420: u32 = fourcc(b"420v");

/// Primaerfarben Display P3 (D65) nach H.273 (SMPTE EG 432-1) - hdr.rs kennt
/// nur 709 und 2020; P3 kommt vor, wenn ein Encoder die P3-PQ-Aufnahme
/// unveraendert kennzeichnet.
pub const PRIMAER_P3: u8 = 12;

/// Eine Formatkennung lesbar ("xf44"); nicht druckbare Bytes als Zahl.
pub fn fourcc_text(t: u32) -> String {
    let b = t.to_be_bytes();
    if b.iter().all(|c| (0x20..0x7f).contains(c)) {
        b.iter().map(|&c| c as char).collect()
    } else {
        format!("0x{t:08x}")
    }
}

/// Welches Format der Decoder fuer diesen Strom bestellt. 4:4:4 bleibt
/// 4:4:4, mehr als 8 Bit bleiben 16 Bit; alles andere (4:2:0, 4:2:2,
/// einfarbig) wird 4:2:0. Ohne lesbares SPS das Format, das alles fasst
/// (xf44) - VideoToolbox rechnet dann um, aber es geht nichts verloren.
/// `voll`: der Wertebereich laut Formatbeschreibung.
pub fn ausgabeformat(art: Option<Bildart>, voll: bool) -> u32 {
    let (vier, zehn) = match art {
        Some(a) => (a.chroma == 3, a.bits > 8),
        None => (true, true),
    };
    match (vier, zehn, voll) {
        (true, true, true) => XF44,
        (true, true, false) => X444,
        (true, false, true) => F444,
        (true, false, false) => V444,
        (false, true, true) => XF20,
        (false, true, false) => X420,
        (false, false, true) => F420,
        (false, false, false) => V420,
    }
}

/// Der Code eines Farbwerts ohne eigenen Namen bei CoreMedia ("YCbCrMatrix#5",
/// "ColorPrimaries#22"): die Zahl hinter dem letzten "#", wenn sie ein Code
/// nach H.273 ist.
pub fn h273_aus_text(text: &str) -> Option<u8> {
    let (_, zahl) = text.rsplit_once('#')?;
    zahl.parse::<u8>().ok()
}

/// Aufbau der Ebenen eines Ausgabeformats - dieselbe Tabelle wie
/// `ebenen_format` fuer FFmpegs Formate: alle zweiebenig (Y, dann U/V als
/// Paare), 10 Bit oben buendig in 16 Bit wie P010. Die Formate ohne "f"
/// sind im begrenzten Wertebereich; `zeile_rgb` und die Metal-Anzeige
/// dehnen sie auf den vollen.
pub fn ebenen(format: u32) -> Option<crate::EbenenFormat> {
    let (sub, bits, begrenzt) = match format {
        XF44 => (false, 16, false),
        X444 => (false, 16, true),
        F444 => (false, 8, false),
        V444 => (false, 8, true),
        XF20 => (true, 16, false),
        X420 => (true, 16, true),
        F420 => (true, 8, false),
        V420 => (true, 8, true),
        _ => return None,
    };
    Some(crate::EbenenFormat { sub, bits, paar: true, begrenzt })
}

/// Ein Fehler von CoreMedia oder VideoToolbox: was scheiterte, und der
/// OSStatus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fehler {
    pub was: &'static str,
    pub status: i32,
}

/// kVTInvalidSessionErr: die Sitzung ist weg (etwa nach dem Ruhezustand).
pub const VT_SITZUNG_UNGUELTIG: i32 = -12903;

/// Fehler, nach denen die Sitzung nichts mehr taugt, der Strom aber schon:
/// die Sitzung ist weg (kVTInvalidSessionErr) oder die Media-Engine hatte
/// einen Aussetzer - sie hakte (Malfunction, auch der Sitzung), war gerade
/// nicht zu haben (NotAvailableNow; -12915 meldet VideoToolbox dafuer
/// mitunter mit dem Code des Encoders), wurde entfernt oder antwortete
/// nicht (CallbackMessaging, Unknown). Dann wird die Sitzung verworfen und
/// mit dem naechsten Schluesselbild neu angelegt; bis dahin taugt kein
/// Paket (die Referenzbilder sind mit der alten Sitzung weg). Ein kaputtes
/// Paket (BadData) oder ein Format, das die Engine nicht kann, gehoert
/// nicht dazu - dafuer hilft keine neue Sitzung.
pub fn sitzung_verwerfen(status: i32) -> bool {
    matches!(status, VT_SITZUNG_UNGUELTIG | -12911 | -12913 | -12915 | -17690 | -17691 | -17695 | -17696)
}

/// Drossel fuer Protokollzeilen, die je Bild anfallen koennen (Fehler beim
/// Decodieren, verworfene Bilder): die erste sofort, danach hoechstens eine
/// je `abstand`. Was dazwischen anfiel, zaehlt sie und nennt es mit der
/// naechsten Zeile - so steht ein streikender Decoder im Protokoll, ohne
/// es mit 120 Zeilen je Sekunde zu fuellen.
#[derive(Debug)]
pub struct Zeilendrossel {
    abstand: std::time::Duration,
    letzte: Option<std::time::Instant>,
    still: u32,
}

impl Zeilendrossel {
    pub const fn neu(abstand: std::time::Duration) -> Zeilendrossel {
        Zeilendrossel { abstand, letzte: None, still: 0 }
    }

    /// Ein Ereignis. Some(n): jetzt eine Zeile schreiben - n Ereignisse seit
    /// der letzten Zeile blieben ohne eigene. None: still zaehlen.
    pub fn zulassen(&mut self, jetzt: std::time::Instant) -> Option<u32> {
        if self.letzte.is_some_and(|l| jetzt.saturating_duration_since(l) < self.abstand) {
            self.still = self.still.saturating_add(1);
            return None;
        }
        self.letzte = Some(jetzt);
        Some(std::mem::take(&mut self.still))
    }

    /// Wie viele Ereignisse seit der letzten Zeile still blieben - danach
    /// wieder 0 (fuer eine Schlusszeile, wenn keine weiteren mehr kommen).
    pub fn still_abholen(&mut self) -> u32 {
        std::mem::take(&mut self.still)
    }
}

impl Fehler {
    /// Der Name des Fehlercodes, soweit er einer von VideoToolbox ist.
    pub fn name(&self) -> Option<&'static str> {
        Some(match self.status {
            -12902 => "kVTParameterErr",
            -12903 => "kVTInvalidSessionErr",
            -12904 => "kVTAllocationFailedErr",
            -12905 => "kVTPixelTransferNotSupportedErr",
            -12906 => "kVTCouldNotFindVideoDecoderErr",
            -12907 => "kVTCouldNotCreateInstanceErr",
            -12909 => "kVTVideoDecoderBadDataErr",
            -12910 => "kVTVideoDecoderUnsupportedDataFormatErr",
            -12911 => "kVTVideoDecoderMalfunctionErr",
            -12913 => "kVTVideoDecoderNotAvailableNowErr",
            -12915 => "kVTVideoEncoderNotAvailableNowErr",
            -12916 => "kVTFormatDescriptionChangeNotSupportedErr",
            -17690 => "kVTVideoDecoderRemovedErr",
            -17691 => "kVTSessionMalfunctionErr",
            -17694 => "kVTVideoDecoderReferenceMissingErr",
            -17695 => "kVTVideoDecoderCallbackMessagingErr",
            -17696 => "kVTVideoDecoderUnknownErr",
            _ => return None,
        })
    }
}

impl std::fmt::Display for Fehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name() {
            Some(n) => write!(f, "{}: OSStatus {} ({n})", self.was, self.status),
            None => write!(f, "{}: OSStatus {}", self.was, self.status),
        }
    }
}

/// Ein Paket fuer den Decoder: die Zugriffseinheit, wie der Host sie
/// schickt, und die Bildnummer, die mit dem Bild wieder herauskommt.
pub struct Paket<'a> {
    pub daten: &'a [u8],
    pub pts: i64,
}

#[cfg(target_os = "macos")]
pub use mac::{Bild, Decoder};
#[cfg(all(target_os = "macos", test))]
pub use mac::farbe_der_saetze;

/// Die Schnittstellen von CoreFoundation, CoreMedia, CoreVideo und
/// VideoToolbox, soweit Decoder und Probe sie brauchen - und die Metal-
/// Anzeige (anzeige_mac.rs), die die CVPixelBuffer uebernimmt.
#[cfg(target_os = "macos")]
#[allow(non_upper_case_globals, non_snake_case)]
pub(crate) mod ffi {
    use std::ffi::{c_char, c_void};

    pub type OSStatus = i32;
    pub type CFTypeRef = *const c_void;
    pub type CFStringRef = *const c_void;
    pub type CFDictionaryRef = *const c_void;
    pub type CFMutableDictionaryRef = *mut c_void;
    pub type CFBooleanRef = *const c_void;
    pub type CMFormatDescriptionRef = *const c_void;
    pub type CMBlockBufferRef = *mut c_void;
    pub type CMSampleBufferRef = *mut c_void;
    pub type CVPixelBufferRef = *mut c_void;
    pub type VTSessionRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CMTime {
        pub value: i64,
        pub timescale: i32,
        pub flags: u32,
        pub epoch: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CMVideoDimensions {
        pub width: i32,
        pub height: i32,
    }

    /// kCFTypeDictionaryKeyCallBacks und ...ValueCallBacks: nur ihre
    /// Adresse wird gebraucht (Aufbau wie CFDictionaryKeyCallBacks).
    #[repr(C)]
    pub struct CFRueckrufe {
        _felder: [usize; 6],
    }

    pub type VTDecompressionOutputCallback =
        unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CVPixelBufferRef, CMTime, CMTime);

    #[repr(C)]
    pub struct VTDecompressionOutputCallbackRecord {
        pub callback: VTDecompressionOutputCallback,
        pub refcon: *mut c_void,
    }

    pub type VTCompressionOutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef);

    /// kCFNumberSInt32Type und kCFNumberFloat32Type.
    pub const CF_SINT32: isize = 3;
    pub const CF_FLOAT32: isize = 5;
    /// kCMBlockBufferAssureMemoryNowFlag.
    pub const BLOCK_SOFORT: u32 = 1;
    /// kCVPixelBufferLock_ReadOnly.
    pub const NUR_LESEN: u64 = 1;
    /// kCVAttachmentMode_ShouldPropagate.
    pub const ANHANG_WEITERGEBEN: u32 = 1;
    /// kVTDecodeInfo_FrameDropped.
    pub const BILD_VERWORFEN: u32 = 1 << 1;
    /// kCMVideoCodecType_HEVC ('hvc1').
    pub const CODEC_HEVC: u32 = u32::from_be_bytes(*b"hvc1");

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFBooleanTrue: CFBooleanRef;
        pub static kCFBooleanFalse: CFBooleanRef;
        pub static kCFTypeDictionaryKeyCallBacks: CFRueckrufe;
        pub static kCFTypeDictionaryValueCallBacks: CFRueckrufe;
        pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFDictionaryCreateMutable(
            alloc: *const c_void,
            kapazitaet: isize,
            schluessel: *const CFRueckrufe,
            werte: *const CFRueckrufe,
        ) -> CFMutableDictionaryRef;
        pub fn CFDictionarySetValue(d: CFMutableDictionaryRef, schluessel: *const c_void, wert: *const c_void);
        pub fn CFNumberCreate(alloc: *const c_void, typ: isize, wert: *const c_void) -> CFTypeRef;
        pub fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub fn CFStringGetCString(s: CFTypeRef, puffer: *mut std::ffi::c_char, groesse: isize, kodierung: u32) -> u8;
    }

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        pub static kCMFormatDescriptionExtension_FullRangeVideo: CFStringRef;
        // Die Farbangaben der Formatbeschreibung (aus dem VUI des SPS) und
        // ihre Werte, soweit der Client sie unterscheidet.
        pub static kCMFormatDescriptionExtension_ColorPrimaries: CFStringRef;
        pub static kCMFormatDescriptionExtension_TransferFunction: CFStringRef;
        pub static kCMFormatDescriptionExtension_YCbCrMatrix: CFStringRef;
        pub static kCMFormatDescriptionColorPrimaries_ITU_R_709_2: CFStringRef;
        pub static kCMFormatDescriptionColorPrimaries_ITU_R_2020: CFStringRef;
        pub static kCMFormatDescriptionColorPrimaries_P3_D65: CFStringRef;
        pub static kCMFormatDescriptionTransferFunction_ITU_R_709_2: CFStringRef;
        pub static kCMFormatDescriptionTransferFunction_SMPTE_ST_2084_PQ: CFStringRef;
        pub static kCMFormatDescriptionTransferFunction_ITU_R_2100_HLG: CFStringRef;
        pub static kCMFormatDescriptionYCbCrMatrix_ITU_R_709_2: CFStringRef;
        pub static kCMFormatDescriptionYCbCrMatrix_ITU_R_601_4: CFStringRef;
        pub static kCMFormatDescriptionYCbCrMatrix_ITU_R_2020: CFStringRef;
        pub static kCMTimeInvalid: CMTime;
        pub fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
            alloc: *const c_void,
            anzahl: usize,
            zeiger: *const *const u8,
            groessen: *const usize,
            nal_laenge: i32,
            erweiterungen: CFDictionaryRef,
            aus: *mut CMFormatDescriptionRef,
        ) -> OSStatus;
        pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
            alloc: *const c_void,
            anzahl: usize,
            zeiger: *const *const u8,
            groessen: *const usize,
            nal_laenge: i32,
            aus: *mut CMFormatDescriptionRef,
        ) -> OSStatus;
        pub fn CMVideoFormatDescriptionGetDimensions(fd: CMFormatDescriptionRef) -> CMVideoDimensions;
        pub fn CMFormatDescriptionGetExtension(fd: CMFormatDescriptionRef, schluessel: CFStringRef) -> CFTypeRef;
        pub fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            fd: CMFormatDescriptionRef,
            index: usize,
            zeiger: *mut *const u8,
            groesse: *mut usize,
            anzahl: *mut usize,
            nal_laenge: *mut i32,
        ) -> OSStatus;
        pub fn CMBlockBufferCreateWithMemoryBlock(
            alloc: *const c_void,
            speicher: *mut c_void,
            block_laenge: usize,
            block_alloc: *const c_void,
            quelle: *const c_void,
            versatz: usize,
            laenge: usize,
            flags: u32,
            aus: *mut CMBlockBufferRef,
        ) -> OSStatus;
        pub fn CMBlockBufferReplaceDataBytes(quelle: *const c_void, ziel: CMBlockBufferRef, versatz: usize, laenge: usize) -> OSStatus;
        pub fn CMBlockBufferGetDataLength(bb: CMBlockBufferRef) -> usize;
        pub fn CMBlockBufferCopyDataBytes(bb: CMBlockBufferRef, versatz: usize, laenge: usize, ziel: *mut c_void) -> OSStatus;
        pub fn CMSampleBufferCreateReady(
            alloc: *const c_void,
            daten: CMBlockBufferRef,
            fd: CMFormatDescriptionRef,
            proben: isize,
            zeiten_anzahl: isize,
            zeiten: *const c_void,
            groessen_anzahl: isize,
            groessen: *const usize,
            aus: *mut CMSampleBufferRef,
        ) -> OSStatus;
        pub fn CMSampleBufferGetDataBuffer(sb: CMSampleBufferRef) -> CMBlockBufferRef;
        pub fn CMSampleBufferGetFormatDescription(sb: CMSampleBufferRef) -> CMFormatDescriptionRef;
        pub fn CMTimeMake(wert: i64, zeitbasis: i32) -> CMTime;
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        pub static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
        pub static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
        pub static kCVPixelBufferMetalCompatibilityKey: CFStringRef;
        // Die Farbangaben als Anhaenge eines Puffers (dieselben Namen wie in
        // der Formatbeschreibung); VideoToolbox gibt sie jedem Bild mit.
        pub static kCVImageBufferColorPrimariesKey: CFStringRef;
        pub static kCVImageBufferTransferFunctionKey: CFStringRef;
        pub static kCVImageBufferYCbCrMatrixKey: CFStringRef;
        pub static kCVImageBufferColorPrimaries_ITU_R_2020: CFStringRef;
        pub static kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ: CFStringRef;
        pub static kCVImageBufferYCbCrMatrix_ITU_R_2020: CFStringRef;
        pub static kCVImageBufferYCbCrMatrix_ITU_R_601_4: CFStringRef;
        /// Liefert einen eigenen Griff (oder NULL).
        pub fn CVBufferCopyAttachment(puffer: CVPixelBufferRef, schluessel: CFStringRef, modus: *mut u32) -> CFTypeRef;
        pub fn CVBufferSetAttachment(puffer: CVPixelBufferRef, schluessel: CFStringRef, wert: CFTypeRef, modus: u32);
        pub fn CVPixelBufferGetPixelFormatType(pb: CVPixelBufferRef) -> u32;
        pub fn CVPixelBufferGetWidth(pb: CVPixelBufferRef) -> usize;
        pub fn CVPixelBufferGetHeight(pb: CVPixelBufferRef) -> usize;
        pub fn CVPixelBufferGetPlaneCount(pb: CVPixelBufferRef) -> usize;
        pub fn CVPixelBufferGetBaseAddressOfPlane(pb: CVPixelBufferRef, ebene: usize) -> *mut c_void;
        pub fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVPixelBufferRef, ebene: usize) -> usize;
        pub fn CVPixelBufferGetHeightOfPlane(pb: CVPixelBufferRef, ebene: usize) -> usize;
        pub fn CVPixelBufferGetWidthOfPlane(pb: CVPixelBufferRef, ebene: usize) -> usize;
        pub fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
        pub fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
        pub fn CVPixelBufferCreate(
            alloc: *const c_void,
            breite: usize,
            hoehe: usize,
            format: u32,
            attribute: CFDictionaryRef,
            aus: *mut CVPixelBufferRef,
        ) -> i32;
    }

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        pub static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFStringRef;
        pub static kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder: CFStringRef;
        pub static kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder: CFStringRef;
        pub static kVTDecompressionPropertyKey_RealTime: CFStringRef;
        pub static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;
        pub static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
        pub static kVTCompressionPropertyKey_RealTime: CFStringRef;
        pub static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
        pub static kVTCompressionPropertyKey_Quality: CFStringRef;
        pub static kVTCompressionPropertyKey_ColorPrimaries: CFStringRef;
        pub static kVTCompressionPropertyKey_TransferFunction: CFStringRef;
        pub static kVTCompressionPropertyKey_YCbCrMatrix: CFStringRef;
        pub fn VTDecompressionSessionCreate(
            alloc: *const c_void,
            fd: CMFormatDescriptionRef,
            decoder: CFDictionaryRef,
            bildpuffer: CFDictionaryRef,
            rueckruf: *const VTDecompressionOutputCallbackRecord,
            aus: *mut VTSessionRef,
        ) -> OSStatus;
        pub fn VTDecompressionSessionDecodeFrame(
            s: VTSessionRef,
            sb: CMSampleBufferRef,
            flags: u32,
            bild_refcon: *mut c_void,
            info: *mut u32,
        ) -> OSStatus;
        pub fn VTDecompressionSessionInvalidate(s: VTSessionRef);
        pub fn VTSessionCopyProperty(s: VTSessionRef, schluessel: CFStringRef, alloc: *const c_void, aus: *mut c_void) -> OSStatus;
        pub fn VTSessionSetProperty(s: VTSessionRef, schluessel: CFStringRef, wert: CFTypeRef) -> OSStatus;
        pub fn VTCompressionSessionCreate(
            alloc: *const c_void,
            breite: i32,
            hoehe: i32,
            codec: u32,
            encoder: CFDictionaryRef,
            quelle: CFDictionaryRef,
            daten_alloc: *const c_void,
            rueckruf: VTCompressionOutputCallback,
            refcon: *mut c_void,
            aus: *mut VTSessionRef,
        ) -> OSStatus;
        pub fn VTCompressionSessionPrepareToEncodeFrames(s: VTSessionRef) -> OSStatus;
        pub fn VTCompressionSessionEncodeFrame(
            s: VTSessionRef,
            pb: CVPixelBufferRef,
            pts: CMTime,
            dauer: CMTime,
            eigenschaften: CFDictionaryRef,
            refcon: *mut c_void,
            info: *mut u32,
        ) -> OSStatus;
        pub fn VTCompressionSessionCompleteFrames(s: VTSessionRef, bis: CMTime) -> OSStatus;
        pub fn VTCompressionSessionInvalidate(s: VTSessionRef);
    }

    extern "C" {
        /// Aus libSystem: ein Symbol, das die SDK-Koepfe nicht nennen.
        pub fn dlsym(griff: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    /// RTLD_DEFAULT auf macOS.
    pub const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

    /// Ein veraenderbares CFDictionary mit CF-Schluesseln und -Werten.
    pub unsafe fn woerterbuch() -> CFMutableDictionaryRef {
        CFDictionaryCreateMutable(std::ptr::null(), 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks)
    }

    /// Eine CFNumber (32 Bit ganz), vom Aufrufer freizugeben.
    pub unsafe fn zahl(w: i32) -> CFTypeRef {
        CFNumberCreate(std::ptr::null(), CF_SINT32, &w as *const i32 as *const c_void)
    }

    /// Eine CFNumber (32 Bit Gleitkomma), vom Aufrufer freizugeben.
    pub unsafe fn kommazahl(w: f32) -> CFTypeRef {
        CFNumberCreate(std::ptr::null(), CF_FLOAT32, &w as *const f32 as *const c_void)
    }

    /// Eintrag setzen und den eigenen Griff am Wert wieder abgeben (das
    /// Woerterbuch haelt seinen eigenen).
    pub unsafe fn setzen_und_freigeben(d: CFMutableDictionaryRef, schluessel: CFStringRef, wert: CFTypeRef) {
        CFDictionarySetValue(d, schluessel, wert);
        CFRelease(wert);
    }

    /// Einen Puffer als HDR10 kennzeichnen (BT.2020, PQ, Matrix BT.2020),
    /// wie VideoToolbox ein decodiertes PQ-Bild kennzeichnet - fuer die
    /// Probebilder und den Eingang des Probe-Encoders.
    pub unsafe fn pq_kennzeichnen(pb: CVPixelBufferRef) {
        CVBufferSetAttachment(pb, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_2020, ANHANG_WEITERGEBEN);
        CVBufferSetAttachment(pb, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, ANHANG_WEITERGEBEN);
        CVBufferSetAttachment(pb, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_2020, ANHANG_WEITERGEBEN);
    }

    /// Einen Puffer mit der Matrix BT.601 kennzeichnen, wie VideoToolbox ein
    /// Bild aus einem Strom mit VUI BT.470BG/SMPTE 170M kennzeichnet (der
    /// bgra-Weg eines Windows-Hosts) - fuer die Probebilder.
    pub unsafe fn bt601_kennzeichnen(pb: CVPixelBufferRef) {
        CVBufferSetAttachment(pb, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_601_4, ANHANG_WEITERGEBEN);
    }

    pub fn wahr(b: bool) -> CFBooleanRef {
        unsafe {
            if b {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::ffi::*;
    use super::*;
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Hoechstens eine Protokollzeile je so viel Zeit fuer Decodierfehler und
    /// fuer verworfene Bilder (je eine Drossel).
    const MELDEABSTAND: Duration = Duration::from_secs(5);

    /// Was der Rueckruf der Sitzung abliefert.
    enum Ausgang {
        /// Ein Bild (schon mit eigenem Griff) und seine Bildnummer.
        Bild(CVPixelBufferRef, i64),
        Fehler(OSStatus),
        /// Kein Bild, ohne Fehler (kVTDecodeInfo_FrameDropped).
        Verworfen,
    }

    // Nur rohe Griffe an CF-Objekte, die sich zwischen Faeden reichen lassen.
    unsafe impl Send for Ausgang {}

    type Ablage = Mutex<Vec<Ausgang>>;

    /// Der Rueckruf der Sitzung. `refcon` zeigt auf die Ablage des Decoders,
    /// `bild_refcon` traegt die Bildnummer des Pakets.
    unsafe extern "C" fn ausgabe(
        refcon: *mut c_void,
        bild_refcon: *mut c_void,
        status: OSStatus,
        info: u32,
        bild: CVPixelBufferRef,
        _pts: CMTime,
        _dauer: CMTime,
    ) {
        let ablage = &*(refcon as *const Ablage);
        let a = if status != 0 {
            Ausgang::Fehler(status)
        } else if bild.is_null() || info & BILD_VERWORFEN != 0 {
            Ausgang::Verworfen
        } else {
            CFRetain(bild as CFTypeRef);
            Ausgang::Bild(bild, bild_refcon as isize as i64)
        };
        ablage.lock().unwrap_or_else(|e| e.into_inner()).push(a);
    }

    /// Ein Farbwert von CoreMedia/CoreVideo (CFString) als Code nach H.273,
    /// ueber die Tabelle (Konstante, Code); ein Code ohne eigenen Namen steht
    /// als "YCbCrMatrix#5" da (so nennt CoreMedia BT.470BG, das VUI des
    /// bgra-Wegs eines Windows-Hosts - ITU_R_601_4 ist SMPTE 170M, 6). Was
    /// beides nicht trifft, ist 2 ("nicht angegeben"), ein fehlender Wert ebenso.
    unsafe fn h273(wert: CFTypeRef, tabelle: &[(CFStringRef, u8)]) -> u8 {
        if wert.is_null() {
            return 2;
        }
        if let Some(&(_, c)) = tabelle.iter().find(|(k, _)| CFEqual(wert, *k) != 0) {
            return c;
        }
        if CFGetTypeID(wert) != CFStringGetTypeID() {
            return 2;
        }
        let mut puffer = [0 as std::ffi::c_char; 64];
        // kCFStringEncodingUTF8.
        if CFStringGetCString(wert, puffer.as_mut_ptr(), puffer.len() as isize, 0x0800_0100) == 0 {
            return 2;
        }
        let text = std::ffi::CStr::from_ptr(puffer.as_ptr()).to_string_lossy();
        super::h273_aus_text(&text).unwrap_or(2)
    }

    /// Die Farbe aus Primaerfarben, Transfer und Matrix, wie CoreMedia sie
    /// nennt (Formatbeschreibung) oder CoreVideo sie anhaengt (Puffer) - die
    /// Namen sind dieselben. PQ zaehlt mit Matrix und Bereich, SDR mit
    /// Matrix (BT.709 oder BT.601) und Bereich (hdr::Farbe::aus_vui).
    /// CoreMedia nennt SMPTE 170M ITU_R_601_4 und BT.470BG "YCbCrMatrix#5" (h273).
    unsafe fn farbe_aus(prim: CFTypeRef, tf: CFTypeRef, mat: CFTypeRef, voll: bool) -> crate::hdr::Farbe {
        use crate::hdr;
        let p = h273(prim, &[
            (kCMFormatDescriptionColorPrimaries_ITU_R_709_2, hdr::PRIMAER_709),
            (kCMFormatDescriptionColorPrimaries_ITU_R_2020, hdr::PRIMAER_2020),
            (kCMFormatDescriptionColorPrimaries_P3_D65, PRIMAER_P3),
        ]);
        let t = h273(tf, &[
            (kCMFormatDescriptionTransferFunction_ITU_R_709_2, hdr::TRANSFER_SDR),
            (kCMFormatDescriptionTransferFunction_SMPTE_ST_2084_PQ, hdr::TRANSFER_PQ),
            (kCMFormatDescriptionTransferFunction_ITU_R_2100_HLG, hdr::TRANSFER_HLG),
        ]);
        let m = h273(mat, &[
            (kCMFormatDescriptionYCbCrMatrix_ITU_R_709_2, hdr::MATRIX_709),
            (kCMFormatDescriptionYCbCrMatrix_ITU_R_601_4, hdr::MATRIX_601),
            (kCMFormatDescriptionYCbCrMatrix_ITU_R_2020, hdr::MATRIX_2020_NCL),
        ]);
        hdr::Farbe::aus_vui(t, p, m, voll)
    }

    /// Die Farbe laut Formatbeschreibung (aus dem VUI des SPS).
    unsafe fn farbe_der_beschreibung(fd: CMFormatDescriptionRef, voll: bool) -> crate::hdr::Farbe {
        farbe_aus(
            CMFormatDescriptionGetExtension(fd, kCMFormatDescriptionExtension_ColorPrimaries),
            CMFormatDescriptionGetExtension(fd, kCMFormatDescriptionExtension_TransferFunction),
            CMFormatDescriptionGetExtension(fd, kCMFormatDescriptionExtension_YCbCrMatrix),
            voll,
        )
    }

    /// Die Farbe aus den Anhaengen eines Puffers; den Bereich sagt das
    /// Format (xf44 voll, x444 begrenzt).
    unsafe fn farbe_der_anhaenge(pb: CVPixelBufferRef, format: u32) -> crate::hdr::Farbe {
        let holen = |k: CFStringRef| CVBufferCopyAttachment(pb, k, std::ptr::null_mut());
        let (p, t, m) = (holen(kCVImageBufferColorPrimariesKey), holen(kCVImageBufferTransferFunctionKey), holen(kCVImageBufferYCbCrMatrixKey));
        let voll = ebenen(format).is_some_and(|e| !e.begrenzt);
        let f = farbe_aus(p, t, m, voll);
        for w in [p, t, m] {
            if !w.is_null() {
                CFRelease(w);
            }
        }
        f
    }

    /// Eine laufende Sitzung mit ihrer Formatbeschreibung und der Farbe,
    /// die diese nennt - sie gilt fuer jedes Bild der Sitzung (neue
    /// Parametersaetze ergeben eine neue Sitzung).
    struct Sitzung {
        vt: VTSessionRef,
        fd: CMFormatDescriptionRef,
        farbe: crate::hdr::Farbe,
    }

    impl Drop for Sitzung {
        fn drop(&mut self) {
            unsafe {
                VTDecompressionSessionInvalidate(self.vt);
                CFRelease(self.vt as CFTypeRef);
                CFRelease(self.fd);
            }
        }
    }

    /// Der Decoder einer Sitzung des Clients: HEVC oder H.264, mit oder ohne
    /// Hardware. Lebt im Empfangsfaden.
    pub struct Decoder {
        h264: bool,
        hardware: bool,
        /// Die geltenden Parametersaetze; die Sitzung gehoert zu ihnen.
        saetze: Parametersaetze,
        /// Vor der Ablage aufgefuehrt: sie faellt zuerst, die Ablage lebt,
        /// solange eine Sitzung in sie schreiben kann.
        sitzung: Option<Sitzung>,
        ablage: Box<Ablage>,
        /// Die zuletzt protokollierte Beschreibung der Sitzung: die erste
        /// Sitzung sagt im Protokoll, was VideoToolbox gewaehlt hat, jede
        /// weitere nur, wenn sich daran etwas aendert.
        gemeldet: Option<String>,
        /// Die Sitzung wurde nach einem Fehler verworfen (oder liess sich
        /// nicht anlegen): bis zum naechsten Schluesselbild taugt kein Paket.
        /// Der Empfang fragt es nach jedem Paket ab (`schluesselbild_noetig`).
        schluesselbild_noetig: bool,
        /// Protokollzeilen fuer Decodierfehler und verworfene Bilder.
        fehlerdrossel: Zeilendrossel,
        verworfen_drossel: Zeilendrossel,
    }

    impl Decoder {
        /// Ein Decoder ohne Sitzung - die entsteht mit den ersten
        /// Parametersaetzen. `hardware`: nur die Media-Engine, sonst gar
        /// nicht; ohne: VideoToolbox auf dem Prozessor.
        pub fn neu(h264: bool, hardware: bool) -> Decoder {
            Decoder {
                h264,
                hardware,
                saetze: Parametersaetze::default(),
                sitzung: None,
                ablage: Box::new(Mutex::new(Vec::new())),
                gemeldet: None,
                schluesselbild_noetig: false,
                fehlerdrossel: Zeilendrossel::neu(MELDEABSTAND),
                verworfen_drossel: Zeilendrossel::neu(MELDEABSTAND),
            }
        }

        /// Eine Zugriffseinheit (Annex B) decodieren; fertige Bilder haengen
        /// an `bilder`. Ohne Parametersaetze bisher: kein Bild, kein Fehler.
        /// Jeder Fehler kommt gedrosselt ins Protokoll. Nach einem Aussetzer
        /// der Media-Engine oder einer ungueltigen Sitzung (`sitzung_verwerfen`)
        /// ist die Sitzung verworfen; die naechste entsteht mit dem naechsten
        /// Paket, und das muss ein Schluesselbild sein - siehe
        /// `schluesselbild_noetig`.
        pub fn fuettern(&mut self, au: &[u8], pts: i64, bilder: &mut Vec<Bild>) -> Result<(), Fehler> {
            let r = self.fuettern_ungemeldet(au, pts, bilder);
            match &r {
                Err(f) => {
                    let verworfen = sitzung_verwerfen(f.status) && self.sitzung.take().is_some();
                    // Ohne Sitzung (verworfen, oder sie liess sich gar nicht
                    // anlegen) ist dieses Paket verloren, und mit ihm die
                    // Referenzen der folgenden.
                    if self.sitzung.is_none() {
                        self.schluesselbild_noetig = true;
                    }
                    if let Some(still) = self.fehlerdrossel.zulassen(Instant::now()) {
                        let mut z = format!("Decodierfehler bei Bild {pts}: {f}");
                        if still > 0 {
                            z.push_str(&format!(" (seit der letzten Meldung {still} weitere)"));
                        }
                        if verworfen {
                            z.push_str(" - Sitzung verworfen, neu mit dem naechsten Schluesselbild");
                        }
                        crate::protokoll::bibliothek_sagt(crate::protokoll::WARNUNG, &z);
                    }
                }
                Ok(()) => {
                    // Wieder ein Bild nach Fehlern, die keine eigene Zeile
                    // bekamen: einmal sagen, wie viele es waren.
                    let still = self.fehlerdrossel.still_abholen();
                    if still > 0 && !bilder.is_empty() {
                        crate::protokoll::bibliothek_sagt(
                            crate::protokoll::WARNUNG,
                            &format!("Decodieren geht wieder (seit der letzten Meldung {still} weitere Fehler)"),
                        );
                    }
                }
            }
            r
        }

        /// Hat der Decoder seit der letzten Frage seine Sitzung nach einem
        /// Fehler verworfen (oder keine anlegen koennen)? Dann darf bis zum
        /// naechsten Schluesselbild nichts mehr hinein - der Empfang wartet
        /// darauf, wie nach einem Codecwechsel.
        pub fn schluesselbild_noetig(&mut self) -> bool {
            std::mem::take(&mut self.schluesselbild_noetig)
        }

        fn fuettern_ungemeldet(&mut self, au: &[u8], pts: i64, bilder: &mut Vec<Bild>) -> Result<(), Fehler> {
            let z = zerlegen(au, self.h264);
            if !z.saetze.leer() {
                let neu = self.saetze.ergaenzt(&z.saetze);
                if neu != self.saetze {
                    self.saetze = neu;
                    self.sitzung = None;
                }
            }
            if z.probe.is_empty() {
                return Ok(());
            }
            if self.sitzung.is_none() {
                if !self.saetze.vollstaendig(self.h264) {
                    return Ok(());
                }
                self.sitzung = Some(self.sitzung_bauen()?);
            }
            self.dekodieren(&z.probe, pts, bilder)
        }

        fn sitzung_bauen(&mut self) -> Result<Sitzung, Fehler> {
            let liste = self.saetze.liste(self.h264);
            let zeiger: Vec<*const u8> = liste.iter().map(|s| s.as_ptr()).collect();
            let groessen: Vec<usize> = liste.iter().map(|s| s.len()).collect();
            let mut fd: CMFormatDescriptionRef = std::ptr::null();
            let st = unsafe {
                if self.h264 {
                    CMVideoFormatDescriptionCreateFromH264ParameterSets(
                        std::ptr::null(), liste.len(), zeiger.as_ptr(), groessen.as_ptr(), 4, &mut fd,
                    )
                } else {
                    CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        std::ptr::null(), liste.len(), zeiger.as_ptr(), groessen.as_ptr(), 4, std::ptr::null(), &mut fd,
                    )
                }
            };
            if st != 0 || fd.is_null() {
                return Err(Fehler { was: "Formatbeschreibung aus den Parametersaetzen", status: st });
            }
            let voll = unsafe { CMFormatDescriptionGetExtension(fd, kCMFormatDescriptionExtension_FullRangeVideo) == kCFBooleanTrue };
            let farbe = unsafe { farbe_der_beschreibung(fd, voll) };
            let art = self.saetze.sps.first().and_then(|s| if self.h264 { h264_sps_lesen(s) } else { hevc_sps_lesen(s) });
            // Auch PQ bleibt xf44/xf20: die Codes kommen roh an, die Anzeige
            // rechnet PQ selbst (anzeige_mac.rs).
            let format = ausgabeformat(art, voll);
            let mut vt: VTSessionRef = std::ptr::null_mut();
            let st = unsafe {
                let decoder = woerterbuch();
                let schluessel = if self.hardware {
                    kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder
                } else {
                    kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder
                };
                CFDictionarySetValue(decoder, schluessel, wahr(self.hardware));
                let bildpuffer = woerterbuch();
                setzen_und_freigeben(bildpuffer, kCVPixelBufferPixelFormatTypeKey, zahl(format as i32));
                setzen_und_freigeben(bildpuffer, kCVPixelBufferIOSurfacePropertiesKey, woerterbuch() as CFTypeRef);
                // Die Metal-Anzeige nimmt die Ebenen ueber CVMetalTextureCache
                // ohne Kopie; dafuer muessen sie so ausgerichtet sein, wie
                // Metal es verlangt.
                CFDictionarySetValue(bildpuffer, kCVPixelBufferMetalCompatibilityKey, kCFBooleanTrue);
                let rueckruf = VTDecompressionOutputCallbackRecord {
                    callback: ausgabe,
                    refcon: &*self.ablage as *const Ablage as *mut c_void,
                };
                let st = VTDecompressionSessionCreate(std::ptr::null(), fd, decoder, bildpuffer, &rueckruf, &mut vt);
                CFRelease(decoder as CFTypeRef);
                CFRelease(bildpuffer as CFTypeRef);
                st
            };
            if st != 0 || vt.is_null() {
                unsafe { CFRelease(fd) };
                return Err(Fehler { was: "VTDecompressionSessionCreate", status: st });
            }
            let sitzung = Sitzung { vt, fd, farbe };
            let hardware = unsafe {
                // Ein Hinweis an den Decoder; kann er ihn nicht, auch recht.
                VTSessionSetProperty(vt, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
                let mut wert: CFTypeRef = std::ptr::null();
                let st = VTSessionCopyProperty(
                    vt,
                    kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                    std::ptr::null(),
                    &mut wert as *mut CFTypeRef as *mut c_void,
                );
                let hw = (st == 0 && !wert.is_null()).then(|| wert == kCFBooleanTrue);
                if !wert.is_null() {
                    CFRelease(wert);
                }
                // Der Software-Decoder beantwortet die Frage nicht immer;
                // ohne Hardware angelegt ist er ohnehin keine.
                hw.or((!self.hardware).then_some(false))
            };
            let masse = unsafe { CMVideoFormatDescriptionGetDimensions(fd) };
            let art_text = match art {
                Some(a) => format!(
                    "{} {} Bit",
                    match a.chroma {
                        3 => "4:4:4",
                        2 => "4:2:2",
                        1 => "4:2:0",
                        _ => "einfarbig",
                    },
                    a.bits
                ),
                None => "Format unbekannt (SPS nicht lesbar)".into(),
            };
            let zeile = format!(
                "VideoToolbox: {}x{} {} {art_text}, {}, Hardware {}, Ausgabe {} ({})",
                masse.width,
                masse.height,
                if self.h264 { "H.264" } else { "HEVC" },
                farbe.text(),
                match hardware {
                    Some(true) => "ja",
                    Some(false) => "nein",
                    None => "unbekannt",
                },
                fourcc_text(format),
                if voll { "voller Wertebereich" } else { "begrenzter Wertebereich" }
            );
            if self.gemeldet.as_deref() != Some(zeile.as_str()) {
                crate::protokoll::zeile(zeile.clone());
                self.gemeldet = Some(zeile);
            }
            Ok(sitzung)
        }

        fn dekodieren(&mut self, probe: &[u8], pts: i64, bilder: &mut Vec<Bild>) -> Result<(), Fehler> {
            let Some(s) = self.sitzung.as_ref() else { return Ok(()) };
            let mut bb: CMBlockBufferRef = std::ptr::null_mut();
            let st = unsafe {
                CMBlockBufferCreateWithMemoryBlock(
                    std::ptr::null(), std::ptr::null_mut(), probe.len(), std::ptr::null(), std::ptr::null(), 0, probe.len(), BLOCK_SOFORT, &mut bb,
                )
            };
            if st != 0 || bb.is_null() {
                return Err(Fehler { was: "CMBlockBufferCreateWithMemoryBlock", status: st });
            }
            let mut sb: CMSampleBufferRef = std::ptr::null_mut();
            let groesse = probe.len();
            let st = unsafe {
                let st = CMBlockBufferReplaceDataBytes(probe.as_ptr() as *const c_void, bb, 0, probe.len());
                let st = if st == 0 {
                    CMSampleBufferCreateReady(std::ptr::null(), bb, s.fd, 1, 0, std::ptr::null(), 1, &groesse, &mut sb)
                } else {
                    st
                };
                CFRelease(bb as CFTypeRef);
                st
            };
            if st != 0 || sb.is_null() {
                return Err(Fehler { was: "CMSampleBufferCreateReady", status: st });
            }
            let mut info = 0u32;
            // Ohne Flaggen: synchron, der Rueckruf ist durch, wenn das hier
            // zurueckkehrt.
            let st = unsafe { VTDecompressionSessionDecodeFrame(s.vt, sb, 0, pts as isize as *mut c_void, &mut info) };
            unsafe { CFRelease(sb as CFTypeRef) };
            let ausgaenge = std::mem::take(&mut *self.ablage.lock().unwrap_or_else(|e| e.into_inner()));
            let mut fehler = (st != 0).then_some(Fehler { was: "VTDecompressionSessionDecodeFrame", status: st });
            let farbe = s.farbe;
            for a in ausgaenge {
                match a {
                    Ausgang::Bild(pb, pts) => match unsafe { Bild::neu(pb, pts, farbe) } {
                        Ok(b) => bilder.push(b),
                        Err(f) => {
                            fehler.get_or_insert(f);
                        }
                    },
                    Ausgang::Fehler(st) => {
                        fehler.get_or_insert(Fehler { was: "Decodieren (Rueckruf)", status: st });
                    }
                    Ausgang::Verworfen => {
                        if let Some(still) = self.verworfen_drossel.zulassen(Instant::now()) {
                            let mehr = if still > 0 { format!(", seit der letzten Meldung {still} weitere") } else { String::new() };
                            crate::protokoll::bibliothek_sagt(
                                crate::protokoll::WARNUNG,
                                &format!("Bild {pts} verworfen (kVTDecodeInfo_FrameDropped{mehr})"),
                            );
                        }
                    }
                }
            }
            match fehler {
                Some(f) => Err(f),
                None => Ok(()),
            }
        }
    }

    /// Ein decodiertes Bild: ein CVPixelBuffer, zum Lesen gesperrt, solange
    /// es lebt. Laesst sich zwischen Faeden reichen: zeichnet Metal, gibt der
    /// Fensterfaden es frei, nachdem die Anzeige einen eigenen Griff auf den
    /// Puffer genommen hat (anzeige_mac.rs).
    pub struct Bild {
        puffer: CVPixelBufferRef,
        pts: i64,
        format: u32,
        farbe: crate::hdr::Farbe,
    }

    // Ein CVPixelBuffer darf von jedem Faden gelesen und freigegeben werden.
    unsafe impl Send for Bild {}

    impl Bild {
        /// Uebernimmt den Griff `puffer` und sperrt die Ebenen zum Lesen.
        unsafe fn neu(puffer: CVPixelBufferRef, pts: i64, farbe: crate::hdr::Farbe) -> Result<Bild, Fehler> {
            let st = CVPixelBufferLockBaseAddress(puffer, NUR_LESEN);
            if st != 0 {
                CFRelease(puffer as CFTypeRef);
                return Err(Fehler { was: "CVPixelBufferLockBaseAddress", status: st });
            }
            Ok(Bild { puffer, pts, format: CVPixelBufferGetPixelFormatType(puffer), farbe })
        }

        /// Wie `neu`, fuer Puffer, die nicht aus dem Decoder kommen (die
        /// Probebilder des Anzeigetests). Uebernimmt den Griff `puffer`; die
        /// Farbe kommt aus seinen Anhaengen (ohne Anhaenge: SDR).
        pub unsafe fn aus_puffer(puffer: CVPixelBufferRef, pts: i64) -> Result<Bild, Fehler> {
            let farbe = farbe_der_anhaenge(puffer, CVPixelBufferGetPixelFormatType(puffer));
            Bild::neu(puffer, pts, farbe)
        }

        /// Die Farbe des Bildes nach H.273 aus dem VUI (ueber die
        /// Formatbeschreibung der Sitzung): PQ mit Primaerfarben, Matrix und
        /// Bereich, SDR mit Matrix (BT.709 oder BT.601) und Bereich. Die
        /// Anzeige entscheidet je Bild danach.
        pub fn farbe(&self) -> crate::hdr::Farbe {
            self.farbe
        }

        /// Die Farbe laut den Anhaengen des Puffers - was VideoToolbox dem
        /// Bild selbst mitgab (nur fuer den Test, dass beides uebereinstimmt).
        #[cfg(test)]
        pub fn farbe_der_anhaenge(&self) -> crate::hdr::Farbe {
            unsafe { farbe_der_anhaenge(self.puffer, self.format) }
        }

        /// Der CVPixelBuffer selbst (ohne eigenen Griff) - fuer die
        /// Metal-Anzeige, die ihn mit CFRetain haelt, solange die Karte liest.
        pub fn puffer(&self) -> CVPixelBufferRef {
            self.puffer
        }

        pub fn breite(&self) -> u32 {
            unsafe { CVPixelBufferGetWidth(self.puffer) as u32 }
        }

        pub fn hoehe(&self) -> u32 {
            unsafe { CVPixelBufferGetHeight(self.puffer) as u32 }
        }

        pub fn pts(&self) -> i64 {
            self.pts
        }

        /// Das Format als CoreVideo-Kennung (etwa XF44).
        pub fn format(&self) -> u32 {
            self.format
        }

        pub fn ebenenzahl(&self) -> usize {
            unsafe { CVPixelBufferGetPlaneCount(self.puffer) }
        }

        /// Die Bytes einer Ebene, Zeile fuer Zeile mit `zeilenlaenge`
        /// Abstand; leer, wenn es die Ebene nicht gibt.
        pub fn daten(&self, ebene: usize) -> &[u8] {
            if ebene >= self.ebenenzahl() {
                return &[];
            }
            unsafe {
                let p = CVPixelBufferGetBaseAddressOfPlane(self.puffer, ebene) as *const u8;
                if p.is_null() {
                    return &[];
                }
                let n = CVPixelBufferGetBytesPerRowOfPlane(self.puffer, ebene) * CVPixelBufferGetHeightOfPlane(self.puffer, ebene);
                std::slice::from_raw_parts(p, n)
            }
        }

        pub fn zeilenlaenge(&self, ebene: usize) -> usize {
            if ebene >= self.ebenenzahl() {
                return 0;
            }
            unsafe { CVPixelBufferGetBytesPerRowOfPlane(self.puffer, ebene) }
        }

        /// Breite einer Ebene in Werten (bei U/V-Paaren: in Paaren).
        #[cfg(test)]
        pub fn ebenenbreite(&self, ebene: usize) -> usize {
            if ebene >= self.ebenenzahl() {
                return 0;
            }
            unsafe { CVPixelBufferGetWidthOfPlane(self.puffer, ebene) }
        }
    }

    impl Drop for Bild {
        fn drop(&mut self) {
            unsafe {
                CVPixelBufferUnlockBaseAddress(self.puffer, NUR_LESEN);
                CFRelease(self.puffer as CFTypeRef);
            }
        }
    }

    /// Die Farbe, die eine Sitzung aus diesen HEVC- oder H.264-Parametersaetzen
    /// ablesen wuerde (Formatbeschreibung wie in `sitzung_bauen`) - nur Test.
    #[cfg(test)]
    pub fn farbe_der_saetze(saetze: &Parametersaetze, h264: bool) -> Result<crate::hdr::Farbe, Fehler> {
        let liste = saetze.liste(h264);
        let zeiger: Vec<*const u8> = liste.iter().map(|s| s.as_ptr()).collect();
        let groessen: Vec<usize> = liste.iter().map(|s| s.len()).collect();
        let mut fd: CMFormatDescriptionRef = std::ptr::null();
        unsafe {
            let st = if h264 {
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null(), liste.len(), zeiger.as_ptr(), groessen.as_ptr(), 4, &mut fd,
                )
            } else {
                CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    std::ptr::null(), liste.len(), zeiger.as_ptr(), groessen.as_ptr(), 4, std::ptr::null(), &mut fd,
                )
            };
            if st != 0 || fd.is_null() {
                return Err(Fehler { was: "Formatbeschreibung aus den Parametersaetzen", status: st });
            }
            let voll = CMFormatDescriptionGetExtension(fd, kCMFormatDescriptionExtension_FullRangeVideo) == kCFBooleanTrue;
            let f = farbe_der_beschreibung(fd, voll);
            CFRelease(fd);
            Ok(f)
        }
    }
}

/// Eine Probe fuer Tests und --decodertest: ein kurzer HEVC-Strom 4:4:4
/// 10 Bit aus dem Hardware-Encoder, als Annex B wie vom Host (Schluessel-
/// bilder mit VPS, SPS und PPS davor). Das Muster ist bekannt (`muster`),
/// also laesst sich jedes decodierte Bild dagegen pruefen.
#[cfg(target_os = "macos")]
pub mod probe {
    use super::ffi::*;
    use std::ffi::c_void;
    use std::sync::Mutex;

    /// Farben der Bloecke (Y, Cb, Cr, 10 Bit, voller Bereich).
    const FARBEN: [(u16, u16, u16); 8] = [
        (200, 512, 512),
        (820, 512, 512),
        (500, 220, 800),
        (500, 800, 220),
        (300, 320, 330),
        (700, 690, 700),
        (600, 260, 610),
        (420, 740, 360),
    ];
    /// Seitenlaenge der Bloecke.
    pub const BLOCK: u32 = 32;

    /// Der Wert bei (x, y), 10 Bit: Bloecke von 32 x 32 in acht Farben, die
    /// unterste Blockreihe dagegen mit Farbwerten, die von Spalte zu Spalte
    /// wechseln - die gibt es nur in 4:4:4 (in 4:2:0 teilen sich je zwei
    /// Spalten einen Farbwert).
    pub fn muster(x: u32, y: u32, hoehe: u32) -> (u16, u16, u16) {
        if y >= hoehe - BLOCK {
            return if x % 2 == 0 { (512, 300, 724) } else { (512, 724, 300) };
        }
        FARBEN[((x / BLOCK + (y / BLOCK) * 3) % 8) as usize]
    }

    /// Ergebnis des Encoders: die Zugriffseinheiten in Annex B.
    pub struct Probe {
        pub einheiten: Vec<Vec<u8>>,
    }

    struct Sammlung {
        einheiten: Vec<Vec<u8>>,
        fehler: Option<OSStatus>,
    }

    /// Laengenpraefix (4 Byte) nach Annex B, bei einem Schluesselbild mit
    /// den Parametersaetzen der Formatbeschreibung davor.
    unsafe fn annex_b(sb: CMSampleBufferRef) -> Option<Vec<u8>> {
        let bb = CMSampleBufferGetDataBuffer(sb);
        if bb.is_null() {
            return None;
        }
        let n = CMBlockBufferGetDataLength(bb);
        let mut roh = vec![0u8; n];
        if CMBlockBufferCopyDataBytes(bb, 0, n, roh.as_mut_ptr() as *mut c_void) != 0 {
            return None;
        }
        let mut nals: Vec<&[u8]> = Vec::new();
        let mut i = 0usize;
        while i + 4 <= roh.len() {
            let l = u32::from_be_bytes([roh[i], roh[i + 1], roh[i + 2], roh[i + 3]]) as usize;
            i += 4;
            if i + l > roh.len() {
                return None;
            }
            nals.push(&roh[i..i + l]);
            i += l;
        }
        let mut aus = Vec::with_capacity(n + 256);
        // IRAP-Bilder (Typ 16..21) bekommen die Parametersaetze davor.
        if nals.iter().any(|nal| (16..=21).contains(&super::nal_typ(nal, false))) {
            let fd = CMSampleBufferGetFormatDescription(sb);
            let mut anzahl = 0usize;
            if CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                fd, 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut anzahl, std::ptr::null_mut(),
            ) != 0
            {
                return None;
            }
            for k in 0..anzahl {
                let (mut p, mut l) = (std::ptr::null::<u8>(), 0usize);
                if CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, k, &mut p, &mut l, std::ptr::null_mut(), std::ptr::null_mut()) != 0 {
                    return None;
                }
                aus.extend_from_slice(&[0, 0, 0, 1]);
                aus.extend_from_slice(std::slice::from_raw_parts(p, l));
            }
        }
        for nal in nals {
            aus.extend_from_slice(&[0, 0, 0, 1]);
            aus.extend_from_slice(nal);
        }
        Some(aus)
    }

    unsafe extern "C" fn kodiert(refcon: *mut c_void, _bild: *mut c_void, status: OSStatus, _info: u32, sb: CMSampleBufferRef) {
        let s = &*(refcon as *const Mutex<Sammlung>);
        let mut s = s.lock().unwrap_or_else(|e| e.into_inner());
        if status != 0 {
            s.fehler.get_or_insert(status);
            return;
        }
        if sb.is_null() {
            return;
        }
        match annex_b(sb) {
            Some(au) => s.einheiten.push(au),
            None => {
                s.fehler.get_or_insert(-1);
            }
        }
    }

    /// `bilder` Bilder des Musters in `breite` x `hoehe`, HEVC Main 4:4:4 10
    /// aus dem Hardware-Encoder. Err, wenn es den nicht gibt (etwa in einer
    /// virtuellen Maschine oder hinter einer Sandbox ohne Zugang zu den
    /// Diensten von VideoToolbox) - dann gibt es nichts zu pruefen.
    pub fn hevc_444_10(breite: u32, hoehe: u32, bilder: u32) -> Result<Probe, String> {
        hevc_444_10_farbe(breite, hoehe, bilder, false)
    }

    /// Wie `hevc_444_10`; mit `pq` als HDR10 gekennzeichnet (Primaerfarben
    /// BT.2020, Transfer PQ, Matrix BT.2020 im VUI, wie der Mac-Host bei HDR)
    /// - dieselben Codes, nur als PQ gemeint.
    pub fn hevc_444_10_farbe(breite: u32, hoehe: u32, bilder: u32, pq: bool) -> Result<Probe, String> {
        let sammlung = Box::new(Mutex::new(Sammlung { einheiten: Vec::new(), fehler: None }));
        unsafe {
            let profil = dlsym(RTLD_DEFAULT, c"kVTProfileLevel_HEVC_Main44410_AutoLevel".as_ptr()) as *const CFStringRef;
            if profil.is_null() || (*profil).is_null() {
                return Err("VideoToolbox kennt das Profil HEVC Main 4:4:4 10 nicht".into());
            }
            let encoder = woerterbuch();
            CFDictionarySetValue(encoder, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);
            let mut s: VTSessionRef = std::ptr::null_mut();
            let st = VTCompressionSessionCreate(
                std::ptr::null(),
                breite as i32,
                hoehe as i32,
                CODEC_HEVC,
                encoder,
                std::ptr::null(),
                std::ptr::null(),
                kodiert,
                &*sammlung as *const Mutex<Sammlung> as *mut c_void,
                &mut s,
            );
            CFRelease(encoder as CFTypeRef);
            if st != 0 || s.is_null() {
                return Err(format!("kein Hardware-Encoder fuer HEVC (VTCompressionSessionCreate: OSStatus {st})"));
            }
            let mut ok = VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, *profil) == 0;
            VTSessionSetProperty(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
            VTSessionSetProperty(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
            let q = kommazahl(1.0);
            VTSessionSetProperty(s, kVTCompressionPropertyKey_Quality, q);
            CFRelease(q);
            if pq {
                ok &= VTSessionSetProperty(s, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_2020) == 0;
                ok &= VTSessionSetProperty(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) == 0;
                ok &= VTSessionSetProperty(s, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_2020) == 0;
            }
            ok &= VTCompressionSessionPrepareToEncodeFrames(s) == 0;
            let attribute = woerterbuch();
            setzen_und_freigeben(attribute, kCVPixelBufferIOSurfacePropertiesKey, woerterbuch() as CFTypeRef);
            for i in 0..bilder {
                if !ok {
                    break;
                }
                let mut pb: CVPixelBufferRef = std::ptr::null_mut();
                if CVPixelBufferCreate(std::ptr::null(), breite as usize, hoehe as usize, super::XF44, attribute, &mut pb) != 0 || pb.is_null() {
                    ok = false;
                    break;
                }
                CVPixelBufferLockBaseAddress(pb, 0);
                for ebene in 0..2usize {
                    let basis = CVPixelBufferGetBaseAddressOfPlane(pb, ebene) as *mut u8;
                    let zeile = CVPixelBufferGetBytesPerRowOfPlane(pb, ebene);
                    for y in 0..hoehe {
                        let z = std::slice::from_raw_parts_mut(basis.add(y as usize * zeile) as *mut u16, zeile / 2);
                        for x in 0..breite {
                            let (yy, cb, cr) = muster(x, y, hoehe);
                            if ebene == 0 {
                                z[x as usize] = yy << 6;
                            } else {
                                z[2 * x as usize] = cb << 6;
                                z[2 * x as usize + 1] = cr << 6;
                            }
                        }
                    }
                }
                CVPixelBufferUnlockBaseAddress(pb, 0);
                // Eingang wie Ausgang gekennzeichnet: so rechnet VideoToolbox
                // nichts um, die Codes bleiben die des Musters.
                if pq {
                    pq_kennzeichnen(pb);
                }
                let st = VTCompressionSessionEncodeFrame(
                    s, pb, CMTimeMake(i as i64, 60), CMTimeMake(1, 60), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(),
                );
                CFRelease(pb as CFTypeRef);
                ok &= st == 0;
            }
            CFRelease(attribute as CFTypeRef);
            VTCompressionSessionCompleteFrames(s, kCMTimeInvalid);
            VTCompressionSessionInvalidate(s);
            CFRelease(s as CFTypeRef);
            let sammlung = sammlung.into_inner().unwrap_or_else(|e| e.into_inner());
            if let Some(st) = sammlung.fehler {
                return Err(format!("Encoder meldet OSStatus {st}"));
            }
            if !ok || sammlung.einheiten.len() != bilder as usize {
                return Err(format!("Encoder lieferte {} von {bilder} Bildern", sammlung.einheiten.len()));
            }
            Ok(Probe { einheiten: sammlung.einheiten })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hexbytes, wie sie in Spezifikationen stehen.
    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// Die Parametersaetze des Hardware-Encoders von VideoToolbox, mit dem
    /// auch der Mac-Host codiert: HEVC Main 4:4:4 10, 1920x1080, voller
    /// Bereich (abgeschrieben am M1, macOS 27).
    const VPS_444_10: &str = "40 01 0c 01 ff ff 04 08 00 00 03 00 bc 08 00 00 03 00 00 78 17 02 40";
    const SPS_444_10: &str = "42 01 01 04 08 00 00 03 00 bc 08 00 00 03 00 00 78 90 00 78 10 02 20 f8 96 c4 0b dc 8b 22 97 fe 5c fe 27 f5 37 02 02 02 00 80";
    const PPS_444_10: &str = "44 01 c0 72 f0 53 24";
    /// Dieselben als HDR10 (2020/PQ/2020 im VUI; derselbe VPS), aus dem
    /// Hardware-Encoder mit probe::hevc_444_10_farbe (M1, macOS 27).
    const SPS_444_10_PQ: &str = "42 01 01 04 08 00 00 03 00 bc 08 00 00 03 00 00 78 90 00 78 10 02 20 f8 96 c4 0b dc 8b 02 97 fe 5c fe 27 f5 37 09 10 09 00 80";
    const PPS_444_10_PQ: &str = "44 01 c0 60 4d 18 aa 48";
    /// HEVC Main 4:2:0 8 Bit, 256x128, mit dem VUI, das nvenc aus BGRA
    /// schreibt (der bgra-Weg eines Windows-Hosts): Primaerfarben und
    /// Transfer BT.709, Matrix BT.470BG (5), begrenzter Bereich (libx265
    /// mit colormatrix=bt470bg:range=limited).
    const VPS_420_8_BT601: &str = "40 01 0c 01 ff ff 01 60 00 00 03 00 90 00 00 03 00 00 03 00 1e 95 98 09";
    const SPS_420_8_BT601: &str = "42 01 01 01 60 00 00 03 00 90 00 00 03 00 00 03 00 1e a0 08 08 08 16 59 59 a4 93 2b c0 5a 80 80 82 82 00 00 03 00 02 00 00 03 00 02 10";
    const PPS_420_8_BT601: &str = "44 01 c1 72 b4 62 40";

    fn mit_startcode(nal: &[u8], lang: bool) -> Vec<u8> {
        let mut v = if lang { vec![0, 0, 0, 1] } else { vec![0, 0, 1] };
        v.extend_from_slice(nal);
        v
    }

    /// Zerlegen: Parametersaetze heraus, AUD weg, der Rest mit vier Byte
    /// Laenge - mit langem und kurzem Startcode und Nullen am Ende eines NAL
    /// (sie gehoeren zum naechsten Startcode).
    #[test]
    fn annex_b_zerlegen_hevc() {
        let (vps, sps_, pps) = (hex(VPS_444_10), hex(SPS_444_10), hex(PPS_444_10));
        let aud = hex("46 01 50");
        let idr = hex("26 01 af 1d 80 a5 ff");
        let sei = hex("4e 01 05 01 aa 80");
        let mut au = mit_startcode(&aud, true);
        au.extend(mit_startcode(&vps, true));
        au.extend(mit_startcode(&sps_, false));
        au.extend(mit_startcode(&pps, true));
        au.push(0); // eine Null zu viel zwischen zwei NALs
        au.extend(mit_startcode(&sei, false));
        au.extend(mit_startcode(&idr, true));
        let z = zerlegen(&au, false);
        assert_eq!(z.saetze.vps, vec![vps.clone()]);
        assert_eq!(z.saetze.sps, vec![sps_.clone()]);
        assert_eq!(z.saetze.pps, vec![pps.clone()]);
        assert!(z.saetze.vollstaendig(false));
        let mut soll = (sei.len() as u32).to_be_bytes().to_vec();
        soll.extend_from_slice(&sei);
        soll.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        soll.extend_from_slice(&idr);
        assert_eq!(z.probe, soll);
        assert_eq!(z.saetze.liste(false), vec![&vps[..], &sps_[..], &pps[..]]);
        // Ein Bild ohne Saetze: nur die Probe.
        let p = hex("02 01 d0 09 7e");
        let z = zerlegen(&mit_startcode(&p, true), false);
        assert!(z.saetze.leer());
        assert_eq!(z.probe, [&[0, 0, 0, 5][..], &p].concat());
        // Nichts ausser Muell: nichts.
        assert_eq!(zerlegen(&[0, 0, 1, 0x40], false), Zerlegt::default());
        assert_eq!(zerlegen(&[1, 2, 3], false), Zerlegt::default());
    }

    /// H.264: SPS (7) und PPS (8) heraus, AUD (9) weg, der Rest mit Laenge;
    /// ohne VPS vollstaendig. Die Einheit ist das 64x64-Bild der Sitzungstests.
    #[test]
    fn annex_b_zerlegen_h264() {
        let sps_ = hex("67 42 c0 0a dc 42 6c 04 40 00 00 03 00 40 00 00 0f 23 c4 89 e0");
        let idr = hex("65 88 84 3a 11 8a 00 02 18 f1 c0 00 40 f6 38 00 08 79 49 c9 c9 d7 5d 75 d7 5d 75 d7 5d 75 e0");
        let mut au = hex("00 00 00 01 09 f0");
        au.extend(mit_startcode(&sps_, true));
        au.extend(mit_startcode(&hex("68 ce 0f c8"), true));
        au.extend(mit_startcode(&idr, true));
        let z = zerlegen(&au, true);
        assert_eq!(z.saetze.sps, vec![sps_]);
        assert_eq!(z.saetze.pps, vec![hex("68 ce 0f c8")]);
        assert!(z.saetze.vps.is_empty());
        assert!(z.saetze.vollstaendig(true));
        assert!(!z.saetze.vollstaendig(false));
        assert_eq!(z.probe, [&(idr.len() as u32).to_be_bytes()[..], &idr].concat());
        assert_eq!(h264_sps_lesen(&z.saetze.sps[0]), Some(Bildart { chroma: 1, bits: 8, profil: 66 }));
        assert_eq!(ausgabeformat(h264_sps_lesen(&z.saetze.sps[0]), true), F420);
    }

    /// Neue Saetze ersetzen nur ihre eigene Art; dieselben Saetze noch
    /// einmal aendern nichts (keine neue Sitzung je Schluesselbild).
    #[test]
    fn saetze_ergaenzen() {
        let alt = Parametersaetze { vps: vec![vec![1]], sps: vec![vec![2]], pps: vec![vec![3], vec![4]] };
        let nur_pps = Parametersaetze { pps: vec![vec![5]], ..Default::default() };
        let neu = alt.ergaenzt(&nur_pps);
        assert_eq!(neu, Parametersaetze { vps: vec![vec![1]], sps: vec![vec![2]], pps: vec![vec![5]] });
        assert_eq!(alt.ergaenzt(&alt), alt);
        assert_eq!(Parametersaetze::default().ergaenzt(&alt), alt);
    }

    /// Das SPS des Mac-Hosts: Main 4:4:4 10 (Profil 4, RExt), chroma 3,
    /// 10 Bit - daraus wird xf44 (bzw. x444 im begrenzten Bereich).
    #[test]
    fn hevc_sps_444_10() {
        let art = hevc_sps_lesen(&hex(SPS_444_10)).expect("SPS nicht lesbar");
        assert_eq!(art, Bildart { chroma: 3, bits: 10, profil: 4 });
        assert_eq!(ausgabeformat(Some(art), true), XF44);
        assert_eq!(ausgabeformat(Some(art), false), X444);
        // Kein SPS, oder ein abgeschnittenes: nichts.
        assert_eq!(hevc_sps_lesen(&hex(PPS_444_10)), None);
        let sps_ = hex(SPS_444_10);
        assert_eq!(hevc_sps_lesen(&sps_[..12]), None);
        // Ohne lesbares SPS das Format, das alles fasst.
        assert_eq!(ausgabeformat(None, true), XF44);
    }

    /// Die Tabelle der Ausgabeformate: jedes bestellte Format hat einen
    /// Ebenenaufbau, den `zeile_rgb` kennt - xf44 ist sub=false, 16 Bit,
    /// Paare; 420f ist NV12, xf20 wie P010.
    #[test]
    fn formate_und_ebenen() {
        use crate::EbenenFormat as E;
        for (chroma, bits, voll, soll, ebenen_soll) in [
            (3, 10, true, "xf44", E { sub: false, bits: 16, paar: true, begrenzt: false }),
            (3, 10, false, "x444", E { sub: false, bits: 16, paar: true, begrenzt: true }),
            (3, 8, true, "444f", E { sub: false, bits: 8, paar: true, begrenzt: false }),
            (3, 8, false, "444v", E { sub: false, bits: 8, paar: true, begrenzt: true }),
            (1, 10, true, "xf20", E { sub: true, bits: 16, paar: true, begrenzt: false }),
            (1, 10, false, "x420", E { sub: true, bits: 16, paar: true, begrenzt: true }),
            (1, 8, true, "420f", E { sub: true, bits: 8, paar: true, begrenzt: false }),
            (1, 8, false, "420v", E { sub: true, bits: 8, paar: true, begrenzt: true }),
            (2, 8, true, "420f", E { sub: true, bits: 8, paar: true, begrenzt: false }),
            (0, 12, true, "xf20", E { sub: true, bits: 16, paar: true, begrenzt: false }),
        ] {
            let f = ausgabeformat(Some(Bildart { chroma, bits, profil: 1 }), voll);
            assert_eq!(fourcc_text(f), soll, "chroma {chroma}, {bits} Bit");
            assert_eq!(ebenen(f), Some(ebenen_soll), "{soll}");
        }
        assert_eq!(ebenen(fourcc(b"pf44")), None);
        assert_eq!(fourcc_text(0x0102_0304), "0x01020304");
    }

    /// Nach welchen Fehlern die Sitzung verworfen wird: ungueltig und die
    /// Aussetzer der Media-Engine ja, ein kaputtes Paket oder ein Format,
    /// das die Engine nicht kann, nein. Alle haben einen Namen.
    #[test]
    fn sitzung_verwerfen_nach_aussetzern() {
        for st in [VT_SITZUNG_UNGUELTIG, -12911, -12913, -12915, -17690, -17691, -17695, -17696] {
            assert!(sitzung_verwerfen(st), "{st}");
            assert!(Fehler { was: "x", status: st }.name().is_some(), "{st}");
        }
        for st in [0, -1, -12902, -12904, -12906, -12909, -12910, -12916, -17694] {
            assert!(!sitzung_verwerfen(st), "{st}");
        }
        assert_eq!(Fehler { was: "x", status: -12911 }.to_string(), "x: OSStatus -12911 (kVTVideoDecoderMalfunctionErr)");
    }

    /// Die Drossel: die erste Zeile sofort, dann hoechstens eine je Abstand,
    /// mit der Zahl der stillen dazwischen; `still_abholen` leert die Zahl.
    #[test]
    fn zeilendrossel() {
        use std::time::{Duration, Instant};
        let t = Instant::now();
        let s = |ms: u64| t + Duration::from_millis(ms);
        let mut d = Zeilendrossel::neu(Duration::from_secs(5));
        assert_eq!(d.zulassen(s(0)), Some(0));
        assert_eq!(d.zulassen(s(10)), None);
        assert_eq!(d.zulassen(s(4999)), None);
        assert_eq!(d.zulassen(s(5000)), Some(2));
        assert_eq!(d.zulassen(s(5001)), None);
        assert_eq!(d.still_abholen(), 1);
        assert_eq!(d.still_abholen(), 0);
        assert_eq!(d.zulassen(s(9000)), None);
        assert_eq!(d.zulassen(s(20000)), Some(1));
        // 120 Fehler je Sekunde ueber 10 s: drei Zeilen, keine geht verloren.
        let mut d = Zeilendrossel::neu(Duration::from_secs(5));
        let mut zeilen = 0;
        let mut gezaehlt = 0;
        for i in 0..1200u64 {
            if let Some(n) = d.zulassen(s(i * 1000 / 120)) {
                zeilen += 1;
                gezaehlt += 1 + n;
            }
        }
        gezaehlt += d.still_abholen();
        assert_eq!((zeilen, gezaehlt), (2, 1200));
    }

    /// Ein kaputtes SPS mit einer riesigen Bittiefe (ue bis 2^32 - 2): kein
    /// Ueberlauf (in einem Debug-Bau waere das ein Panic, im Release eine
    /// falsche Bittiefe), sondern "nicht lesbar".
    #[test]
    fn sps_mit_riesiger_bittiefe() {
        // Bits als Text ("0"/"1") zu Bytes, der Rest mit Nullen aufgefuellt.
        let bytes = |bits: &str| -> Vec<u8> {
            let b: Vec<u8> = bits.bytes().filter(|c| *c == b'0' || *c == b'1').map(|c| c - b'0').collect();
            b.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, x)| a | x << (7 - i))).collect()
        };
        let riesig = format!("{}1{}", "0".repeat(31), "1".repeat(31));
        let zehn = "011"; // ue(2): 2 + 8 = 10 Bit
        // H.264, High 4:4:4 (244): sps_id 0, chroma 3, separate 0, Bittiefe.
        let h264 = |tiefe: &str| [vec![0x67, 244, 0, 40], bytes(&format!("1 00100 0 {tiefe} 1"))].concat();
        assert_eq!(h264_sps_lesen(&h264(zehn)), Some(Bildart { chroma: 3, bits: 10, profil: 244 }));
        assert_eq!(h264_sps_lesen(&h264(&riesig)), None);
        // HEVC: VPS 0, eine Schicht, Profil 4 (RExt), 80 Bit Flaggen, Level,
        // sps_id 0, chroma 3, separate 0, Breite und Hoehe 0, kein Fenster.
        let hevc = |tiefe: &str| {
            let kopf = format!("0000 000 1 000 00100 {} {} 1 00100 0 1 1 0 {tiefe} 1", "0".repeat(80), "0".repeat(8));
            [vec![0x42, 0x01], bytes(&kopf)].concat()
        };
        assert_eq!(hevc_sps_lesen(&hevc(zehn)), Some(Bildart { chroma: 3, bits: 10, profil: 4 }));
        assert_eq!(hevc_sps_lesen(&hevc(&riesig)), None);
    }

    /// Farbwerte ohne eigenen Namen bei CoreMedia: die Zahl hinter "#".
    #[test]
    fn h273_aus_dem_namen() {
        assert_eq!(h273_aus_text("YCbCrMatrix#5"), Some(5));
        assert_eq!(h273_aus_text("ColorPrimaries#22"), Some(22));
        assert_eq!(h273_aus_text("ITU_R_709_2"), None);
        assert_eq!(h273_aus_text("YCbCrMatrix#"), None);
        assert_eq!(h273_aus_text("YCbCrMatrix#999"), None);
    }

    #[test]
    fn fehlertext() {
        let f = Fehler { was: "VTDecompressionSessionCreate", status: -12906 };
        assert_eq!(f.to_string(), "VTDecompressionSessionCreate: OSStatus -12906 (kVTCouldNotFindVideoDecoderErr)");
        let f = Fehler { was: "x", status: -1 };
        assert_eq!(f.to_string(), "x: OSStatus -1");
    }

    /// Hardware-Decode der Probe (HEVC 4:4:4 10 aus dem Hardware-Encoder):
    /// jedes Paket ein Bild mit seiner Bildnummer, Format xf44 (bzw. x444),
    /// die Werte wie im Muster - in den Bloecken und in der Reihe, in der
    /// die Farbe von Spalte zu Spalte wechselt (das kann nur 4:4:4). Danach
    /// dasselbe durch `to_rgb` gegen die Rechnung von `zeile_rgb`. Ohne
    /// Hardware-Encoder (virtuelle Maschine, Sandbox) uebersprungen.
    #[cfg(target_os = "macos")]
    #[test]
    fn hardware_decode_444_10() {
        pruefen(true);
    }

    /// Dieselbe Probe durch VideoToolbox auf dem Prozessor - der Weg, auf
    /// den der Client zurueckfaellt.
    #[cfg(target_os = "macos")]
    #[test]
    fn software_decode_444_10() {
        pruefen(false);
    }

    /// Laesst sich keine Sitzung anlegen (hier: Parametersaetze, aus denen
    /// keine Formatbeschreibung wird), ist das Paket verloren: der Decoder
    /// sagt einmal, dass bis zum naechsten Schluesselbild gewartet werden
    /// muss. Mit dem naechsten brauchbaren Schluesselbild geht es weiter,
    /// ohne dass er es noch einmal sagt.
    #[cfg(target_os = "macos")]
    #[test]
    fn ohne_sitzung_bis_zum_schluesselbild() {
        let p = match probe::hevc_444_10(256, 128, 2) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("uebersprungen: {e}");
                return;
            }
        };
        let mut d = Decoder::neu(false, true);
        let mut bilder = Vec::new();
        assert!(!d.schluesselbild_noetig());
        let kaputt: Vec<u8> = [&[0, 0, 0, 1, 0x40, 0x01, 0xff][..], &[0, 0, 0, 1, 0x42, 0x01, 0xff], &[0, 0, 0, 1, 0x44, 0x01, 0xff], &[0, 0, 0, 1, 0x26, 0x01, 0xaf, 0x00]].concat();
        assert!(d.fuettern(&kaputt, 7, &mut bilder).is_err());
        assert!(d.schluesselbild_noetig());
        assert!(!d.schluesselbild_noetig(), "nur einmal");
        d.fuettern(&p.einheiten[0], 8, &mut bilder).expect("Schluesselbild mit guten Saetzen");
        d.fuettern(&p.einheiten[1], 9, &mut bilder).expect("Folgebild");
        assert_eq!(bilder.len(), 2);
        assert!(!d.schluesselbild_noetig());
    }

    /// Die Farbe kommt aus dem VUI des SPS, ueber die Formatbeschreibung:
    /// die Saetze des Hardware-Encoders mit 2020/PQ/2020 (HDR10, wie der
    /// Mac-Host bei HDR) ergeben PQ, die heutigen SDR-Saetze SDR, die des
    /// bgra-Wegs eines Windows-Hosts BT.601 begrenzt. Braucht nur
    /// CoreMedia, keinen Encoder.
    #[cfg(target_os = "macos")]
    #[test]
    fn farbe_aus_den_parametersaetzen() {
        let saetze = |v: &str, s: &str, p: &str| Parametersaetze { vps: vec![hex(v)], sps: vec![hex(s)], pps: vec![hex(p)] };
        assert_eq!(farbe_der_saetze(&saetze(VPS_444_10, SPS_444_10, PPS_444_10), false), Ok(crate::hdr::Farbe::SDR));
        assert_eq!(farbe_der_saetze(&saetze(VPS_444_10, SPS_444_10_PQ, PPS_444_10_PQ), false), Ok(crate::hdr::Farbe::PQ));
        // Das VUI des bgra-Wegs: CoreMedia nennt BT.470BG ITU_R_601_4, der
        // Bereich ist begrenzt - so rechnet die Anzeige BT.601 begrenzt.
        assert_eq!(
            farbe_der_saetze(&saetze(VPS_420_8_BT601, SPS_420_8_BT601, PPS_420_8_BT601), false),
            Ok(crate::hdr::Farbe { matrix: crate::hdr::MATRIX_601, voll: false, ..crate::hdr::Farbe::SDR })
        );
        // Das PQ-SPS liest sich wie das SDR-SPS: 4:4:4 10 Bit, also xf44.
        assert_eq!(hevc_sps_lesen(&hex(SPS_444_10_PQ)), Some(Bildart { chroma: 3, bits: 10, profil: 4 }));
    }

    /// H.264 vom Mac-Host (VideoToolbox schreibt kein VUI) mit dem VUI, das
    /// der Windows-Client fuer NVDEC einsetzt (sps::h264_sps_mit_vui):
    /// CoreMedia liest daraus BT.709 im vollen Bereich - ein zweiter Parser
    /// neben dem Rundlauf in sps.rs. SPS 1080p vom M1 (wie sps.rs VT_1080P),
    /// dazu ein PPS fuer High mit CABAC.
    #[cfg(target_os = "macos")]
    #[test]
    fn h264_vui_des_clients_ist_bt709_voll() {
        let alt = hex("27 64 00 33 ac 56 80 78 02 27 e5 40");
        let neu = crate::sps::h264_sps_mit_vui(&alt).expect("wird umgeschrieben");
        let saetze = |sps: &[u8]| Parametersaetze { vps: vec![], sps: vec![sps.to_vec()], pps: vec![hex("28 ee 3c b0")] };
        assert_eq!(farbe_der_saetze(&saetze(&neu), true), Ok(crate::hdr::Farbe::SDR));
    }

    /// Hardware-Rundreise eines HDR10-Stroms (vthdrtest als Test): der
    /// Hardware-Encoder kennzeichnet 2020/PQ/2020, der Decoder liefert xf44
    /// mit den Codes des Musters, und jedes Bild traegt PQ - laut
    /// Formatbeschreibung wie laut den Anhaengen des Puffers. Ohne
    /// Hardware-Encoder (Sandbox) uebersprungen.
    #[cfg(target_os = "macos")]
    #[test]
    fn hardware_decode_444_10_pq() {
        pruefen_farbe(true, true);
    }

    #[cfg(target_os = "macos")]
    fn pruefen(hardware: bool) {
        pruefen_farbe(hardware, false);
    }

    #[cfg(target_os = "macos")]
    fn pruefen_farbe(hardware: bool, pq: bool) {
        use crate::Ebenenbild;
        let (b, h) = (256u32, 128u32);
        let p = match probe::hevc_444_10_farbe(b, h, 4, pq) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("uebersprungen: {e}");
                return;
            }
        };
        assert!(zerlegen(&p.einheiten[0], false).saetze.vollstaendig(false), "erstes Bild ohne Parametersaetze");
        let mut d = Decoder::neu(false, hardware);
        let mut bilder = Vec::new();
        for (i, au) in p.einheiten.iter().enumerate() {
            let vorher = bilder.len();
            d.fuettern(au, 100 + i as i64, &mut bilder).unwrap_or_else(|e| panic!("Paket {i}: {e}"));
            assert_eq!(bilder.len(), vorher + 1, "Paket {i} lieferte kein Bild");
        }
        let pts: Vec<i64> = bilder.iter().map(|b| b.pts()).collect();
        assert_eq!(pts, vec![100, 101, 102, 103]);
        let bild = bilder.last().unwrap();
        assert_eq!((bild.breite(), bild.hoehe()), (b, h));
        assert!(matches!(bild.format(), XF44 | X444), "Format {}", fourcc_text(bild.format()));
        assert_eq!(
            ebenen(bild.format()),
            Some(crate::EbenenFormat { sub: false, bits: 16, paar: true, begrenzt: bild.format() == X444 })
        );
        assert_eq!(bild.ebenenzahl(), 2);
        assert_eq!(bild.ebenenbreite(1), b as usize);
        let farbe = if pq { crate::hdr::Farbe::PQ } else { crate::hdr::Farbe::SDR };
        for (i, bild) in bilder.iter().enumerate() {
            assert_eq!(bild.farbe(), farbe, "Bild {i}: Farbe laut Formatbeschreibung");
            assert_eq!(Ebenenbild::farbe(bild), farbe, "Bild {i}: Farbe ueber Ebenenbild");
            if pq {
                assert_eq!(bild.farbe_der_anhaenge(), farbe, "Bild {i}: Farbe laut Anhaengen");
            }
        }
        let wert = |ebene: usize, x: u32, y: u32, k: usize| -> u16 {
            let d = bild.daten(ebene);
            let i = y as usize * bild.zeilenlaenge(ebene) + (x as usize * if ebene == 0 { 1 } else { 2 } + k) * 2;
            u16::from_le_bytes([d[i], d[i + 1]]) >> 6
        };
        // Die Bloecke: in ihrem Inneren (4 Punkte Abstand zum Rand) auf
        // wenige Stufen genau.
        let mut groesste = 0i32;
        for y in (0..h - probe::BLOCK).filter(|y| (4..probe::BLOCK - 4).contains(&(y % probe::BLOCK))) {
            for x in (0..b).filter(|x| (4..probe::BLOCK - 4).contains(&(x % probe::BLOCK))) {
                let (sy, scb, scr) = probe::muster(x, y, h);
                for (ist, soll) in [(wert(0, x, y, 0), sy), (wert(1, x, y, 0), scb), (wert(1, x, y, 1), scr)] {
                    groesste = groesste.max((ist as i32 - soll as i32).abs());
                }
            }
        }
        assert!(groesste <= 16, "Abweichung in den Bloecken bis {groesste} (10 Bit)");
        // Die Reihe mit wechselnder Farbe: benachbarte Spalten bleiben weit
        // auseinander (Soll 424 Stufen; 4:2:0 gaebe 0).
        let y = h - probe::BLOCK / 2;
        for x in 4..b - 5 {
            let (a, n) = (wert(1, x, y, 0) as i32, wert(1, x + 1, y, 0) as i32);
            assert!((a - n).abs() >= 300, "Spalte {x}: Cb {a} neben {n} - kein 4:4:4");
        }
        // Und durch to_rgb: dieselbe Rechnung wie zeile_rgb mit den Werten
        // des Musters, auf wenige Stufen genau (nur SDR - PQ rechnet die
        // Anzeige, siehe anzeige_mac.rs).
        assert_eq!(bild.ebenen(), ebenen(bild.format()));
        if pq {
            return;
        }
        let rgb = crate::to_rgb(bild).expect("to_rgb");
        assert_eq!((rgb.width, rgb.height), (b, h));
        let mut soll = [0u32; 1];
        let mut groesste = 0i32;
        for y in (0..h).filter(|y| (4..probe::BLOCK - 4).contains(&(y % probe::BLOCK))) {
            for x in (0..b).filter(|x| (4..probe::BLOCK - 4).contains(&(x % probe::BLOCK))) {
                let (sy, scb, scr) = probe::muster(x, y, h);
                let (yr, ur, vr) = ((sy << 6).to_le_bytes(), (scb << 6).to_le_bytes(), (scr << 6).to_le_bytes());
                let k = crate::hdr::SDR_KOEFF_709;
                if bild.format() == X444 {
                    crate::zeile_rgb::<false, 16, false, true>(&mut soll, &yr, &ur, &vr, &k);
                } else {
                    crate::zeile_rgb::<false, 16, false, false>(&mut soll, &yr, &ur, &vr, &k);
                }
                let ist = rgb.pixels[(y * b + x) as usize];
                for s in [16, 8, 0] {
                    let d = ((ist >> s) & 0xff) as i32 - ((soll[0] >> s) & 0xff) as i32;
                    groesste = groesste.max(d.abs());
                }
            }
        }
        assert!(groesste <= 6, "RGB weicht bis {groesste} ab");
    }
}
