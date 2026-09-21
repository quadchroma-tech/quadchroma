// QuadChroma Client, Tonausgabe unter Windows.
//
// Der Ton kommt als verschraenktes float32 ueber die Videoverbindung (Nachricht
// 32 Tonformat, Nachricht 33 Tondaten) und geht hier ueber WASAPI auf das
// Standardgeraet: geteilter Modus, ereignisgesteuert, niemals exklusiv.
//
// Zwischen Netzfaden und Geraetefaden liegt ein Ringpuffer fester Groesse:
// Ziel 30 ms Vorlauf, harte Obergrenze 120 ms. Laeuft er ueber, fliegen die
// aeltesten Frames raus statt dass die Verzoegerung waechst -- lieber ein kurzer
// Aussetzer als ein Ton, der dem Bild hinterherhinkt. Aussetzer werden gezaehlt
// und ueber stats() fuer die Anzeige herausgereicht.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

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

/// Vorlauf, den wir anstreben, bevor der erste Ton herausgeht.
const ZIEL_MS: u32 = 30;
/// Harte Obergrenze. Darueber wird verworfen, nicht gepuffert.
const MAX_MS: u32 = 120;
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

// ------------------------------------------------------------------ Ringpuffer

/// Frames liegen verschraenkt im Geraeteformat. Schreiber ist der Netzfaden,
/// Leser der Geraetefaden; beide fassen ihn nur kurz an, es wird darin nur
/// kopiert, nie gewartet und nie belegt.
struct Ring {
    buf: Vec<f32>,
    kap: usize,   // Kapazitaet in Frames
    lese: usize,  // Frame-Index des aeltesten Frames
    fuell: usize, // belegte Frames
    ziel: usize,  // Vorlauf in Frames
    bereit: bool, // Vorlauf erreicht, es darf gespielt werden

    // Umrechnung Quelle -> Geraet
    q_kan: usize,
    g_rate: u32,
    g_kan: usize,
    phase: u64,   // Festkomma 32.32, Position in Quell-Frames
    schritt: u64, // Quell-Frames je Ziel-Frame, Festkomma 32.32
}

impl Ring {
    fn neu(q_rate: u32, q_kan: usize, g_rate: u32, g_kan: usize, ziel: usize, kap: usize) -> Self {
        Self {
            buf: vec![0.0; kap * g_kan],
            kap,
            lese: 0,
            fuell: 0,
            ziel,
            bereit: false,
            q_kan,
            g_rate,
            g_kan,
            phase: 0,
            schritt: ((q_rate as u64) << 32) / g_rate.max(1) as u64,
        }
    }

    /// Ein Quell-Frame ab `basis` auf die Geraetekanaele legen. Gibt true
    /// zurueck, wenn dafuer der aelteste Frame weichen musste.
    fn ablegen(&mut self, quelle: &[f32], basis: usize) -> bool {
        let mut verworfen = false;
        if self.fuell == self.kap {
            self.lese = (self.lese + 1) % self.kap;
            self.fuell -= 1;
            verworfen = true;
        }
        let frame = (self.lese + self.fuell) % self.kap;
        let o = frame * self.g_kan;
        for c in 0..self.g_kan {
            // Mono geht auf die ersten beiden Kanaele, sonst Kanal fuer Kanal;
            // was das Geraet mehr hat, bleibt still. Der Mac liefert Stereo,
            // also ist das hier in aller Regel eine reine Kopie.
            self.buf[o + c] = if self.q_kan == 1 {
                if c < 2 {
                    quelle[basis]
                } else {
                    0.0
                }
            } else if c < self.q_kan {
                quelle[basis + c]
            } else {
                0.0
            };
        }
        self.fuell += 1;
        verworfen
    }

    /// `out` vollstaendig fuellen. Gibt true zurueck, wenn der Puffer leerlief
    /// und mit Stille aufgefuellt werden musste.
    fn hole(&mut self, out: &mut [f32]) -> bool {
        let frames = out.len() / self.g_kan;
        if !self.bereit {
            // Vorlauf sammeln. Stille hier ist kein Aussetzer, sondern Absicht.
            if self.fuell >= self.ziel && self.ziel > 0 {
                self.bereit = true;
            } else {
                out.fill(0.0);
                return false;
            }
        }
        let n = frames.min(self.fuell);
        for i in 0..n {
            let idx = (self.lese + i) % self.kap;
            let s = idx * self.g_kan;
            let d = i * self.g_kan;
            out[d..d + self.g_kan].copy_from_slice(&self.buf[s..s + self.g_kan]);
        }
        self.lese = (self.lese + n) % self.kap;
        self.fuell -= n;

        if n < frames {
            out[n * self.g_kan..].fill(0.0);
            self.bereit = false; // Vorlauf neu aufbauen, sonst stottert es weiter
            true
        } else {
            false
        }
    }
}

// --------------------------------------------------------------- Gemeinsames

struct Geteilt {
    ring: Mutex<Option<Ring>>,
    tot: AtomicBool,       // Geraet weg, push() macht nichts mehr
    ende: AtomicBool,      // Abbruch von aussen, siehe Drop
    aussetzer: AtomicU64,  // Puffer lief leer
    ueberlauf: AtomicU64,  // Frames verworfen, weil die Obergrenze erreicht war
}

/// Ein vergifteter Mutex darf die Tonausgabe nicht mitreissen.
fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ Aussenseite

pub struct AudioOut {
    geteilt: Arc<Geteilt>,
}

impl AudioOut {
    /// Oeffnet das Standard-Ausgabegeraet. Gibt einen Text zurueck statt zu
    /// stuerzen: ohne Ton laeuft der Rest des Programms weiter.
    pub fn new(rate: u32, channels: u16) -> Result<Self, String> {
        if !(8000..=384_000).contains(&rate) {
            return Err(format!("unplausible Abtastrate: {rate}"));
        }
        if channels == 0 || channels > 8 {
            return Err(format!("unplausible Kanalzahl: {channels}"));
        }

        let geteilt = Arc::new(Geteilt {
            ring: Mutex::new(None),
            tot: AtomicBool::new(false),
            ende: AtomicBool::new(false),
            aussetzer: AtomicU64::new(0),
            ueberlauf: AtomicU64::new(0),
        });

        // Das Geraet wird im Geraetefaden geoeffnet: COM gehoert dem Faden, der
        // es benutzt. new() wartet hier nur auf die Rueckmeldung.
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let mit = geteilt.clone();
        std::thread::Builder::new()
            .name("quadchroma-audio".into())
            .spawn(move || faden(rate, channels as usize, mit, tx))
            .map_err(|e| format!("Tonfaden startet nicht: {e}"))?;

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(AudioOut { geteilt }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                geteilt.ende.store(true, Ordering::Relaxed);
                Err("Tongeraet meldet sich nicht".into())
            }
        }
    }

    /// Verschraenkte float32-Frames annehmen. Wartet auf nichts: der Mutex wird
    /// nur fuer das Kopieren gehalten, nie ueber Netz oder Geraeteaufrufe.
    pub fn push(&self, pcm: &[f32]) {
        if pcm.is_empty() || self.geteilt.tot.load(Ordering::Relaxed) {
            return;
        }
        let mut verworfen = 0u64;
        {
            let mut g = sperre(&self.geteilt.ring);
            let Some(r) = g.as_mut() else { return };
            let q_kan = r.q_kan;
            let q_frames = pcm.len() / q_kan;
            if q_frames == 0 {
                return;
            }
            // UNGEPRUEFT: Ratenumsetzung nach naechstem Nachbarn, klanglich nicht
            // nachgemessen. Die Phase laeuft ueber Chunk-Grenzen weiter, damit
            // nichts driftet. Bei gleicher Rate ist schritt genau 1<<32, dann ist
            // das hier eine reine Kopie und die Naeherung greift gar nicht.
            let schritt = r.schritt;
            let mut p = r.phase;
            let ende = (q_frames as u64) << 32;
            while p < ende {
                let i = (p >> 32) as usize;
                if r.ablegen(pcm, i * q_kan) {
                    verworfen += 1;
                }
                p = p.saturating_add(schritt);
            }
            r.phase = p.saturating_sub(ende);
        }
        if verworfen > 0 {
            self.geteilt.ueberlauf.fetch_add(verworfen, Ordering::Relaxed);
        }
    }

    /// Fuellstand in Millisekunden und Anzahl der Aussetzer, fuer die Anzeige.
    pub fn stats(&self) -> (f32, u64) {
        let aussetzer = self.geteilt.aussetzer.load(Ordering::Relaxed);
        if self.geteilt.tot.load(Ordering::Relaxed) {
            return (0.0, aussetzer);
        }
        let g = sperre(&self.geteilt.ring);
        let ms = match g.as_ref() {
            Some(r) if r.g_rate > 0 => r.fuell as f32 * 1000.0 / r.g_rate as f32,
            _ => 0.0,
        };
        (ms, aussetzer)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.geteilt.ende.store(true, Ordering::Relaxed);
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
