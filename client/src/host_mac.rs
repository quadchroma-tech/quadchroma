// Die Host-Engine des Mac im Client: Objective-C und C aus host/, von
// build.rs als libqchost.a hineingebaut - dieselben Quellen wie
// QuadChroma.app, ohne host/start.m (dort steht das main der eigenen App).
//
// Hier steht die C-Schnittstelle aus host/dienst.h, dazu die beiden
// Zugang-Funktionen, mit denen der Test prueft, dass C und Rust dieselbe
// Geraete-ID rechnen und gleich schreiben. Noch startet niemand den Dienst:
// das kommt mit der einen App (qc_dienst_starten eingebettet aus resumed,
// danach qc_oberflaeche_fertig; qc_dienst_beenden vor dem Ende, wenn winits
// event_loop.exit() am Beenden von AppKit vorbeigeht).
//
// Alles ausser qc_dienst_beenden nur auf dem Hauptfaden.

// Die Deklarationen nutzt bis zur einen App nur der Test.
#![allow(dead_code)]

use std::ffi::{c_char, c_int};

/// Rueckgaben von qc_dienst_starten (QC_DIENST_* in host/dienst.h).
pub const DIENST_OK: c_int = 0;
/// Ein anderer Host dieses Nutzers laeuft (host-instanz.lock).
pub const DIENST_LAEUFT_SCHON: c_int = 1;
/// In diesem Prozess schon gestartet.
pub const DIENST_DOPPELT: c_int = 2;
/// Nicht auf dem Hauptfaden gerufen.
pub const DIENST_FADEN: c_int = 3;
/// host.key unlesbar oder nicht anlegbar (bzw. --capture-Datei nicht schreibbar).
pub const DIENST_DATEI: c_int = 5;

/// qc_dienst_cfg aus host/dienst.h.
#[repr(C)]
pub struct DienstCfg {
    /// 1: im Client-Prozess - NSApp.delegate gehoert winit und bleibt unberuehrt.
    pub eingebettet: c_int,
    /// Schalter wie auf der Kommandozeile (argv[0] = Programmname); argv NULL = keine.
    pub argc: c_int,
    pub argv: *const *const c_char,
    /// Doppelklick auf die laufende App; None = Menue des Symbols zeigen.
    pub oeffnen: Option<extern "C" fn()>,
}

extern "C" {
    /// Startet den Dienst ohne Run-Loop (Hauptfaden). DIENST_OK oder ein Fehler oben.
    pub fn qc_dienst_starten(cfg: *const DienstCfg) -> c_int;
    /// Abschied an den Zuschauer (Typ 13, Grund 0), Kanaele zu; aus jedem Faden, hoechstens rund 3 s.
    pub fn qc_dienst_beenden();
    /// Sprache, Menue, Symbol in der Menueleiste - nach qc_dienst_starten, aus resumed.
    pub fn qc_oberflaeche_fertig();
    /// Geraete-ID aus einem oeffentlichen Schluessel (32 Byte), host/zugang.h.
    pub fn qc_zugang_id(schluessel: *const u8) -> u32;
    /// Anzeigeform "ddd ddd ddd" mit Nullbyte in `aus` (12 Byte), host/zugang.h.
    pub fn qc_zugang_id_text(id: u32, aus: *mut c_char);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zugang;
    use std::ffi::{c_void, CStr};

    #[link(name = "objc")]
    extern "C" {
        fn objc_msgSend();
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
    extern "C" {
        /// menue.m: die Geraete-ID, wie das Menue sie zeigt (NSString, autoreleased).
        fn qc_id_text(nummer: u32) -> *mut c_void;
    }

    /// qc_id_text aus menue.m (Menue der Menueleiste) als Rust-Text.
    fn id_text_menue(id: u32) -> String {
        // SAFETY: Objective-C-Laufzeit wie in tray_mac.rs; der Pool haelt das
        // autoreleased NSString, bis der Text kopiert ist.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let s = qc_id_text(id);
            assert!(!s.is_null());
            let utf8: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const c_char =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let text = CStr::from_ptr(utf8(s, sel_registerName(c"UTF8String".as_ptr()))).to_str().expect("UTF-8").to_string();
            objc_autoreleasePoolPop(pool);
            text
        }
    }

    fn id_text_c(id: u32) -> String {
        let mut puffer = [0 as c_char; 12];
        // SAFETY: qc_zugang_id_text schreibt hoechstens 12 Byte samt Nullbyte (snprintf).
        unsafe { qc_zugang_id_text(id, puffer.as_mut_ptr()) };
        // SAFETY: snprintf schliesst mit einem Nullbyte ab.
        unsafe { CStr::from_ptr(puffer.as_ptr()) }.to_str().expect("nur Ziffern und Leerzeichen").to_string()
    }

    fn id_c(schluessel: &[u8; 32]) -> u32 {
        // SAFETY: qc_zugang_id liest genau 32 Byte.
        unsafe { qc_zugang_id(schluessel.as_ptr()) }
    }

    /// Die Geraete-ID aus C ist fuer feste Schluessel dieselbe wie aus Rust
    /// (zugang.rs), samt Schreibweise - im Protokoll und in der Bekanntgabe
    /// (qc_zugang_id_text, zugang.c) wie im Menue (qc_id_text, menue.m).
    #[test]
    fn id_text_aus_c_gleich_rust() {
        let feste: [([u8; 32], &str); 3] = [
            (std::array::from_fn(|i| i as u8 + 1), "581 729 911"),
            ([0u8; 32], "138 912 047"),
            ([0xffu8; 32], "309 912 962"),
        ];
        for (schluessel, erwartet) in &feste {
            let id = id_c(schluessel);
            assert_eq!(id, zugang::geraete_id(schluessel));
            assert_eq!(id_text_c(id), *erwartet);
            assert_eq!(id_text_c(id), zugang::id_text(id));
            assert_eq!(id_text_menue(id), *erwartet);
        }
        for k in 0..=255u8 {
            let schluessel: [u8; 32] = std::array::from_fn(|i| k.wrapping_mul(31).wrapping_add(i as u8));
            let id = id_c(&schluessel);
            assert_eq!(id, zugang::geraete_id(&schluessel), "Schluessel {k}");
            assert_eq!(id_text_c(id), zugang::id_text(id), "Schluessel {k}");
            assert_eq!(id_text_menue(id), zugang::id_text(id), "Schluessel {k}");
        }
        // Randwerte der Schreibweise, auch ueber der Grenze von 10^9.
        for id in [0, 5, 999_999_999, 1_000_000_000, 1_000_000_005, u32::MAX] {
            assert_eq!(id_text_c(id), zugang::id_text(id), "ID {id}");
            assert_eq!(id_text_menue(id), zugang::id_text(id), "ID {id}");
        }
    }
}
