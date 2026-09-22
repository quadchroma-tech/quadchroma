// QuadChroma Client, Tonausgabe unter macOS.
//
// Der Ton kommt als verschraenktes float32 ueber die Videoverbindung (Nachricht
// 32 Tonformat, Nachricht 33 Tondaten) und geht hier ueber eine AudioQueue
// (AudioToolbox) auf das Standard-Ausgabegeraet. Die Queue bekommt das Format,
// das der Host schickt (48000 Hz, Stereo, float32), und rechnet selbst auf das
// Geraet um - deshalb ist der Ring hier immer eine reine Kopie ohne
// Ratenumsetzung. Drei Puffer zu je 10 ms; der Rueckruf der Queue kommt aus
// ihrem eigenen Faden und fuellt jeden Puffer aus dem Ring, notfalls mit
// Stille (das zaehlt als Aussetzer wie auf Windows).
//
// Ringpuffer, Vorlauf und Zaehler liegen in audio_ring.rs und sind mit der
// Windows-Ausgabe (audio.rs) geteilt. Die Schnittstelle nach aussen ist
// dieselbe: AudioOut::new(rate, kanaele), push(pcm), stats().
//
// Keine Kiste dazu: die wenigen Aufrufe aus AudioToolbox und CoreAudio sind
// von Hand deklariert und ueber die Frameworks gelinkt. Die Strukturen sind
// gegen die Kopfdateien des SDK (AudioQueue.h, CoreAudioTypes.h,
// AudioHardware.h) abgeglichen.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::audio_ring::{format_pruefen, sperre, Ausgang, Geteilt, Ring, MAX_MS, ZIEL_MS};

/// Laenge eines Geraetepuffers. Drei davon sind unterwegs.
const PUFFER_MS: u32 = 10;
const PUFFER_ANZAHL: usize = 3;

// ------------------------------------------------------------------- FFI

type OSStatus = i32;
type AudioQueueRef = *mut c_void;
type AudioObjectID = u32;

/// 'lpcm'
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// kLinearPCMFormatFlagIsFloat | kLinearPCMFormatFlagIsPacked
const K_FLAGS_FLOAT_PACKED: u32 = 1 | 8;

/// kAudioObjectSystemObject
const K_SYSTEM_OBJECT: AudioObjectID = 1;
/// kAudioHardwarePropertyDefaultOutputDevice 'dOut'
const K_DEFAULT_OUTPUT_DEVICE: u32 = 0x644F_7574;
/// kAudioDevicePropertyNominalSampleRate 'nsrt'
const K_NOMINAL_SAMPLE_RATE: u32 = 0x6E73_7274;
/// kAudioObjectPropertyScopeGlobal 'glob'
const K_SCOPE_GLOBAL: u32 = 0x676C_6F62;
/// kAudioObjectPropertyElementMain
const K_ELEMENT_MAIN: u32 = 0;

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioQueueBuffer {
    audio_data_bytes_capacity: u32,
    audio_data: *mut c_void,
    audio_data_byte_size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *const c_void,
    packet_description_count: u32,
}

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

type AudioQueueOutputCallback =
    unsafe extern "C" fn(user: *mut c_void, queue: AudioQueueRef, puffer: *mut AudioQueueBuffer);

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewOutput(
        format: *const AudioStreamBasicDescription,
        rueckruf: AudioQueueOutputCallback,
        user: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        heraus: *mut AudioQueueRef,
    ) -> OSStatus;
    fn AudioQueueAllocateBuffer(
        queue: AudioQueueRef,
        bytes: u32,
        heraus: *mut *mut AudioQueueBuffer,
    ) -> OSStatus;
    fn AudioQueueEnqueueBuffer(
        queue: AudioQueueRef,
        puffer: *mut AudioQueueBuffer,
        paketzahl: u32,
        pakete: *const c_void,
    ) -> OSStatus;
    fn AudioQueueStart(queue: AudioQueueRef, startzeit: *const c_void) -> OSStatus;
    fn AudioQueueStop(queue: AudioQueueRef, sofort: u8) -> OSStatus;
    fn AudioQueueDispose(queue: AudioQueueRef, sofort: u8) -> OSStatus;
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        objekt: AudioObjectID,
        adresse: *const AudioObjectPropertyAddress,
        qualifier_groesse: u32,
        qualifier: *const c_void,
        groesse: *mut u32,
        daten: *mut c_void,
    ) -> OSStatus;
}

/// OSStatus lesbar machen: viele Codes sind vier Buchstaben ('fmt?', 'nope'),
/// die uebrigen bleiben Zahlen.
fn status_text(s: OSStatus) -> String {
    let b = (s as u32).to_be_bytes();
    if b.iter().all(|c| (0x20..0x7F).contains(c)) {
        format!("'{}' ({s})", String::from_utf8_lossy(&b))
    } else {
        s.to_string()
    }
}

/// Das Standard-Ausgabegeraet und seine Nennrate. Ohne Geraet (kein
/// Lautsprecher, kein HDMI-Ton) gibt es hier den Grund - so wie Windows
/// "kein Standard-Ausgabegeraet" meldet.
fn standardgeraet() -> Result<(AudioObjectID, f64), String> {
    let mut geraet: AudioObjectID = 0;
    let mut groesse = std::mem::size_of::<AudioObjectID>() as u32;
    let adresse = AudioObjectPropertyAddress {
        selector: K_DEFAULT_OUTPUT_DEVICE,
        scope: K_SCOPE_GLOBAL,
        element: K_ELEMENT_MAIN,
    };
    let s = unsafe {
        AudioObjectGetPropertyData(
            K_SYSTEM_OBJECT,
            &adresse,
            0,
            std::ptr::null(),
            &mut groesse,
            &mut geraet as *mut AudioObjectID as *mut c_void,
        )
    };
    if s != 0 {
        return Err(format!("kein Standard-Ausgabegeraet: {}", status_text(s)));
    }
    if geraet == 0 {
        return Err("kein Standard-Ausgabegeraet".into());
    }
    // Die Nennrate ist nur fuer das Protokoll; die Queue rechnet selbst um.
    let mut rate: f64 = 0.0;
    let mut groesse = std::mem::size_of::<f64>() as u32;
    let adresse = AudioObjectPropertyAddress {
        selector: K_NOMINAL_SAMPLE_RATE,
        scope: K_SCOPE_GLOBAL,
        element: K_ELEMENT_MAIN,
    };
    let _ = unsafe {
        AudioObjectGetPropertyData(
            geraet,
            &adresse,
            0,
            std::ptr::null(),
            &mut groesse,
            &mut rate as *mut f64 as *mut c_void,
        )
    };
    Ok((geraet, rate))
}

// ------------------------------------------------------------------ Aussenseite

pub struct AudioOut {
    gemeinsam: Ausgang,
    queue: AudioQueueRef,
    /// Der Rueckruf haelt eine eigene Zaehlung an `Geteilt` (Arc::into_raw);
    /// sie wird erst nach dem Entsorgen der Queue zurueckgegeben.
    rueckruf_griff: *const Geteilt,
}

// Die Queue ist von jedem Faden aus benutzbar; der Rohzeiger ist nur ein
// Griff darauf.
unsafe impl Send for AudioOut {}

impl AudioOut {
    /// Oeffnet das Standard-Ausgabegeraet ueber eine AudioQueue. Gibt einen
    /// Text zurueck statt zu stuerzen: ohne Ton laeuft der Rest weiter.
    pub fn new(rate: u32, channels: u16) -> Result<Self, String> {
        format_pruefen(rate, channels)?;
        let (_geraet, geraet_rate) = standardgeraet()?;

        let kanaele = channels as usize;
        let geteilt = Geteilt::neu();

        // Der Ring liegt im Quellformat; die Queue setzt auf das Geraet um.
        let puffer_frames = ((rate as u64 * PUFFER_MS as u64) / 1000).max(1) as usize;
        let ziel = ((rate as u64 * ZIEL_MS as u64) / 1000).max(1) as usize;
        let kap = (((rate as u64 * MAX_MS as u64) / 1000) as usize)
            .max(ziel + puffer_frames * PUFFER_ANZAHL);
        *sperre(&geteilt.ring) = Some(Ring::neu(rate, kanaele, rate, kanaele, ziel, kap));

        let format = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: K_FLAGS_FLOAT_PACKED,
            bytes_per_packet: 4 * channels as u32,
            frames_per_packet: 1,
            bytes_per_frame: 4 * channels as u32,
            channels_per_frame: channels as u32,
            bits_per_channel: 32,
            reserved: 0,
        };

        let griff = Arc::into_raw(geteilt.clone());
        let mut queue: AudioQueueRef = std::ptr::null_mut();
        let s = unsafe {
            AudioQueueNewOutput(
                &format,
                rueckruf,
                griff as *mut c_void,
                std::ptr::null(), // kein RunLoop: die Queue ruft aus ihrem eigenen Faden
                std::ptr::null(),
                0,
                &mut queue,
            )
        };
        if s != 0 || queue.is_null() {
            unsafe { drop(Arc::from_raw(griff)) };
            *sperre(&geteilt.ring) = None;
            return Err(format!("AudioQueue anlegen: {}", status_text(s)));
        }

        let mut aus = AudioOut { gemeinsam: Ausgang { geteilt }, queue, rueckruf_griff: griff };

        // Drei Puffer anlegen und gleich fuellen (aus dem noch leeren Ring:
        // Stille), damit die Queue beim Start etwas zu spielen hat.
        let bytes = (puffer_frames * kanaele * 4) as u32;
        for _ in 0..PUFFER_ANZAHL {
            let mut p: *mut AudioQueueBuffer = std::ptr::null_mut();
            let s = unsafe { AudioQueueAllocateBuffer(queue, bytes, &mut p) };
            if s != 0 || p.is_null() {
                aus.entsorgen();
                return Err(format!("AudioQueue-Puffer: {}", status_text(s)));
            }
            unsafe { rueckruf(griff as *mut c_void, queue, p) };
        }

        let s = unsafe { AudioQueueStart(queue, std::ptr::null()) };
        if s != 0 {
            aus.entsorgen();
            return Err(format!("AudioQueue starten: {}", status_text(s)));
        }

        crate::protokoll::zeile(format!(
            "Ton: AudioQueue {rate} Hz, {channels} Kanaele, float32; Geraet {:.0} Hz",
            geraet_rate
        ));
        Ok(aus)
    }

    /// Verschraenkte float32-Frames annehmen, siehe audio_ring.rs.
    pub fn push(&self, pcm: &[f32]) {
        self.gemeinsam.push(pcm)
    }

    /// Fuellstand in Millisekunden und Anzahl der Aussetzer, fuer die Anzeige.
    pub fn stats(&self) -> (f32, u64) {
        self.gemeinsam.stats()
    }

    /// Queue anhalten und entsorgen, danach den Griff des Rueckrufs
    /// zurueckgeben. `sofort` = 1: AudioQueueDispose kehrt erst zurueck, wenn
    /// kein Rueckruf mehr laeuft - deshalb ist der Griff danach frei.
    fn entsorgen(&mut self) {
        if self.queue.is_null() {
            return;
        }
        self.gemeinsam.geteilt.ende.store(true, Ordering::Relaxed);
        unsafe {
            let _ = AudioQueueStop(self.queue, 1);
            let _ = AudioQueueDispose(self.queue, 1);
        }
        self.queue = std::ptr::null_mut();
        self.gemeinsam.geteilt.tot.store(true, Ordering::Relaxed);
        *sperre(&self.gemeinsam.geteilt.ring) = None;
        if !self.rueckruf_griff.is_null() {
            unsafe { drop(Arc::from_raw(self.rueckruf_griff)) };
            self.rueckruf_griff = std::ptr::null();
        }
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.entsorgen();
    }
}

// ------------------------------------------------------------------ Rueckruf

/// Wird von der Queue gerufen, sobald ein Puffer abgespielt ist: aus dem Ring
/// nachfuellen, immer vollstaendig (notfalls Stille), wieder einreihen. Kein
/// Sturz darf hier heraus - die Queue laeuft in ihrem eigenen Faden.
unsafe extern "C" fn rueckruf(user: *mut c_void, queue: AudioQueueRef, puffer: *mut AudioQueueBuffer) {
    if user.is_null() || puffer.is_null() {
        return;
    }
    let geteilt = &*(user as *const Geteilt);
    let kap = (*puffer).audio_data_bytes_capacity as usize;
    let n = kap / 4;
    let feld = std::slice::from_raw_parts_mut((*puffer).audio_data as *mut f32, n);
    let ende = geteilt.ende.load(Ordering::Relaxed);
    let mangel = if ende {
        feld.fill(0.0);
        false
    } else {
        let mut g = sperre(&geteilt.ring);
        match g.as_mut() {
            Some(r) => r.hole(feld),
            None => {
                feld.fill(0.0);
                false
            }
        }
    };
    (*puffer).audio_data_byte_size = (n * 4) as u32;
    if mangel {
        geteilt.aussetzer.fetch_add(1, Ordering::Relaxed);
    }
    if !ende {
        // Nach AudioQueueStop schlaegt das fehl; dann ist ohnehin Schluss.
        let _ = AudioQueueEnqueueBuffer(queue, puffer, 0, std::ptr::null());
    }
}

// --------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Unsinnige Formate oeffnen kein Geraet.
    #[test]
    fn unplausibles_format() {
        assert!(AudioOut::new(0, 2).is_err());
        assert!(AudioOut::new(48_000, 0).is_err());
    }

    /// 200 ms Sinus (440 Hz) ueber die AudioQueue - hoerbar, deshalb nur
    /// auf Wunsch: cargo test -- --ignored ton_hoerbar
    #[test]
    #[ignore]
    fn ton_hoerbar() {
        let aus = AudioOut::new(48_000, 2).expect("Tongeraet");
        let stueck = 480; // 10 ms
        let mut pcm = vec![0.0f32; stueck * 2];
        let mut t = 0usize;
        // Die ersten 100 ms auf einmal (unter der Obergrenze von 120 ms),
        // danach im Takt: so kann der Schlaf des Testfadens den Ring nicht
        // leerlaufen lassen - wie der Netzfaden, der immer vorneweg ist.
        for n in 0..20 {
            for i in 0..stueck {
                let w = ((t + i) as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.2;
                pcm[i * 2] = w;
                pcm[i * 2 + 1] = w;
            }
            t += stueck;
            aus.push(&pcm);
            if n >= 10 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        // Rest ausspielen lassen, dann die Zaehler ansehen. Genau ein
        // Aussetzer ist der Normalfall: der Puffer laeuft am Ende leer.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let (ms, aussetzer) = aus.stats();
        let ueberlauf = aus.gemeinsam.geteilt.ueberlauf.load(Ordering::Relaxed);
        println!("Fuellstand {ms:.1} ms, Aussetzer {aussetzer}, Ueberlauf {ueberlauf} Frames");
        assert_eq!(ueberlauf, 0, "Frames verworfen: {ueberlauf}");
        assert!(aussetzer <= 1, "zu viele Aussetzer: {aussetzer}");
    }
}
