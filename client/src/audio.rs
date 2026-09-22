// QuadChroma Client, Tonausgabe unter Windows.
//
// Der Ton kommt als verschraenktes float32 ueber die Videoverbindung (Nachricht
// 32 Tonformat, Nachricht 33 Tondaten) und geht hier ueber WASAPI auf das
// Standardgeraet: geteilter Modus, ereignisgesteuert, niemals exklusiv.
//
// Ringpuffer, Vorlauf, Aussetzer- und Ueberlaufzaehler liegen in audio_ring.rs
// und sind mit der Mac-Ausgabe (audio_mac.rs) geteilt; hier steht nur das
// Geraet: oeffnen, Mischformat lesen, bei jedem Ereignis nachfuellen.
//
// Neue Abhaengigkeit, genau diese Zeile nach client/Cargo.toml unter
// [dependencies] (MIT ODER Apache-2.0, passt zur Lizenzvorgabe):
//
//   windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Media_Audio", "Win32_System_Com", "Win32_System_Threading"] }
//
// Sonst kommt keine Kiste dazu. Das Modul ist reines Windows; auf dem Mac laesst
// es sich nicht uebersetzen.
//
// UNGEPRUEFT: die Signaturen von CoCreateInstance, IMMDevice::Activate,
// IAudioClient (Initialize, GetMixFormat, SetEventHandle, GetService),
// IAudioRenderClient, CreateEventW, WaitForSingleObject und CoTaskMemFree sind
// gegen microsoft.github.io/windows-docs-rs abgeglichen, aber nichts davon ist
// hier gebaut worden. Ungeprueft bleiben ausserdem PCWSTR::null(), das Feld .0
// des Rueckgabewerts von WaitForSingleObject und die obige Merkmalsliste.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::audio_ring::{format_pruefen, sperre, Ausgang, Geteilt, Ring, MAX_MS, ZIEL_MS};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// So lange warten wir auf das Geraeteereignis, bevor wir es fuer tot erklaeren.
const WARTE_MS: u32 = 2000;

// Als Zahlen hingeschrieben, damit wir kein weiteres Modul der windows-Kiste
// freischalten muessen.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 00000003-0000-0010-8000-00AA00389B71.
const SUBTYPE_IEEE_FLOAT: GUID = GUID {
    data1: 0x0000_0003,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
};

// ------------------------------------------------------------------ Aussenseite

pub struct AudioOut {
    gemeinsam: Ausgang,
}

impl AudioOut {
    /// Oeffnet das Standard-Ausgabegeraet. Gibt einen Text zurueck statt zu
    /// stuerzen: ohne Ton laeuft der Rest des Programms weiter.
    pub fn new(rate: u32, channels: u16) -> Result<Self, String> {
        format_pruefen(rate, channels)?;

        let geteilt = Geteilt::neu();

        // Das Geraet wird im Geraetefaden geoeffnet: COM gehoert dem Faden, der
        // es benutzt. new() wartet hier nur auf die Rueckmeldung.
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let mit = geteilt.clone();
        std::thread::Builder::new()
            .name("quadchroma-audio".into())
            .spawn(move || faden(rate, channels as usize, mit, tx))
            .map_err(|e| format!("Tonfaden startet nicht: {e}"))?;

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(AudioOut { gemeinsam: Ausgang { geteilt } }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                geteilt.ende.store(true, Ordering::Relaxed);
                Err("Tongeraet meldet sich nicht".into())
            }
        }
    }

    /// Verschraenkte float32-Frames annehmen, siehe audio_ring.rs.
    pub fn push(&self, pcm: &[f32]) {
        self.gemeinsam.push(pcm)
    }

    /// Fuellstand in Millisekunden und Anzahl der Aussetzer, fuer die Anzeige.
    pub fn stats(&self) -> (f32, u64) {
        self.gemeinsam.stats()
    }
}

// ------------------------------------------------------------------ Geraetefaden

fn faden(rate: u32, kanaele: usize, geteilt: Arc<Geteilt>, tx: mpsc::Sender<Result<(), String>>) {
    // Kein Sturz darf aus diesem Faden heraus: schlimmstenfalls ist der Ton weg.
    let ergebnis = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        lauf(rate, kanaele, &geteilt, &tx)
    }));
    match ergebnis {
        // Nach erfolgreichem Start hoert hier niemand mehr zu; das Senden darf
        // dann fehlschlagen.
        Ok(Err(e)) => {
            let _ = tx.send(Err(e));
        }
        Err(_) => {
            let _ = tx.send(Err("Tonfaden abgestuerzt".into()));
        }
        Ok(Ok(())) => {}
    }
    geteilt.tot.store(true, Ordering::Relaxed);
    *sperre(&geteilt.ring) = None;
}

unsafe fn lauf(
    rate: u32,
    kanaele: usize,
    geteilt: &Geteilt,
    tx: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    // COM fuer diesen Faden. Ein bereits eingerichtetes Apartment ist kein
    // Fehler, deshalb schauen wir uns das Ergebnis nicht an. CoUninitialize
    // sparen wir uns: die COM-Zeiger leben bis zum Ende dieser Funktion.
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

    let aufzaehler: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("Geraeteliste: {e}"))?;
    let geraet = aufzaehler
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("kein Standard-Ausgabegeraet: {e}"))?;
    let client: IAudioClient = geraet
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("Ausgabegeraet oeffnen: {e}"))?;

    // Im geteilten Modus gibt das Mischformat den Ton an. Wir nehmen es, wie es
    // ist, und rechnen selbst darauf um.
    let mix = client
        .GetMixFormat()
        .map_err(|e| format!("Geraeteformat: {e}"))?;
    if mix.is_null() {
        return Err("Geraet nennt kein Format".into());
    }
    let (g_rate, g_kan, ist_float) = format_lesen(mix);

    if !ist_float || g_rate == 0 || g_kan == 0 {
        CoTaskMemFree(Some(mix as *const c_void));
        return Err(format!(
            "Geraet arbeitet nicht mit float32 ({g_rate} Hz, {g_kan} Kanaele); \
             lieber kein Ton als Rauschen"
        ));
    }

    let init = client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        0, // Puffergroesse: 0 heisst Standardperiode des Geraets
        0, // im geteilten Modus muss die Periode 0 sein
        mix,
        None,
    );
    CoTaskMemFree(Some(mix as *const c_void));
    init.map_err(|e| format!("Tonstrom einrichten: {e}"))?;

    let puffer_frames = client
        .GetBufferSize()
        .map_err(|e| format!("Puffergroesse: {e}"))?;

    let ereignis = CreateEventW(None, false, false, PCWSTR::null())
        .map_err(|e| format!("Ereignis anlegen: {e}"))?;

    if let Err(e) = client.SetEventHandle(ereignis) {
        let _ = CloseHandle(ereignis);
        return Err(format!("Ereignis anmelden: {e}"));
    }
    let render: IAudioRenderClient = match client.GetService() {
        Ok(r) => r,
        Err(e) => {
            let _ = CloseHandle(ereignis);
            return Err(format!("Ausgabepfad: {e}"));
        }
    };

    let ziel = ((g_rate as u64 * ZIEL_MS as u64) / 1000).max(1) as usize;
    let kap = (((g_rate as u64 * MAX_MS as u64) / 1000) as usize).max(ziel + puffer_frames as usize);
    *sperre(&geteilt.ring) = Some(Ring::neu(rate, kanaele, g_rate, g_kan, ziel, kap));

    if let Err(e) = client.Start() {
        *sperre(&geteilt.ring) = None;
        let _ = CloseHandle(ereignis);
        return Err(format!("Tonstrom starten: {e}"));
    }

    let _ = tx.send(Ok(()));

    // Ab hier: bei jedem Ereignis den freien Teil des Geraetepuffers fuellen.
    // Immer vollstaendig, notfalls mit Stille, sonst stolpert die Mischstufe.
    while !geteilt.ende.load(Ordering::Relaxed) {
        // 0 ist WAIT_OBJECT_0. Alles andere heisst Zeitueberschreitung oder
        // Fehler, und beides bedeutet hier: das Geraet ist weg.
        if WaitForSingleObject(ereignis, WARTE_MS).0 != 0 {
            break;
        }
        let Ok(belegt) = client.GetCurrentPadding() else {
            break;
        };
        let frei = puffer_frames.saturating_sub(belegt);
        if frei == 0 {
            continue;
        }
        let Ok(zeiger) = render.GetBuffer(frei) else {
            break;
        };
        if zeiger.is_null() {
            break;
        }
        let mangel = {
            let ziel_feld =
                std::slice::from_raw_parts_mut(zeiger as *mut f32, frei as usize * g_kan);
            let mut g = sperre(&geteilt.ring);
            match g.as_mut() {
                Some(r) => r.hole(ziel_feld),
                None => {
                    ziel_feld.fill(0.0);
                    false
                }
            }
        };
        if render.ReleaseBuffer(frei, 0).is_err() {
            break;
        }
        if mangel {
            geteilt.aussetzer.fetch_add(1, Ordering::Relaxed);
        }
    }

    let _ = client.Stop();
    let _ = CloseHandle(ereignis);
    Ok(())
}

/// Rate, Kanalzahl und "ist float32" aus dem Geraeteformat lesen.
/// WAVEFORMATEX ist gepackt: nur Kopien lesen, keine Referenzen auf Felder.
unsafe fn format_lesen(f: *const WAVEFORMATEX) -> (u32, usize, bool) {
    let tag = (*f).wFormatTag;
    let rate = (*f).nSamplesPerSec;
    let kan = (*f).nChannels as usize;
    let bits = (*f).wBitsPerSample;
    let cb = (*f).cbSize as usize;

    let float = match tag {
        WAVE_FORMAT_IEEE_FLOAT => bits == 32,
        WAVE_FORMAT_EXTENSIBLE => {
            let zusatz =
                std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>();
            if cb >= zusatz {
                let ext = f as *const WAVEFORMATEXTENSIBLE;
                bits == 32 && { let sub = (*ext).SubFormat; sub == SUBTYPE_IEEE_FLOAT }
            } else {
                false
            }
        }
        _ => false,
    };
    (rate, kan, float)
}
