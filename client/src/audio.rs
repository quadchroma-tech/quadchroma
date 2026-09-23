// QuadChroma Client, Tonausgabe unter Windows.
//
// Der Ton kommt als verschraenktes float32 ueber die Videoverbindung (Nachricht
// 32 Tonformat, Nachricht 33 Tondaten) und geht hier ueber WASAPI auf das
// Standardgeraet: geteilter Modus, ereignisgesteuert, niemals exklusiv.
//
// Ringpuffer, Vorlauf, Aussetzer- und Ueberlaufzaehler liegen in audio_ring.rs
// und sind mit der Mac-Ausgabe (audio_mac.rs) geteilt; hier steht nur das
// Geraet: oeffnen, Mischformat lesen, bei jedem Ereignis nachfuellen - und
// wenn es verloren geht (abgezogen, Format umgestellt, anderes Standardgeraet),
// das dann aktuelle Standardgeraet neu oeffnen, siehe `betreiben`.
//
// Gebraucht wird nur die windows-Kiste mit den Merkmalen aus client/Cargo.toml
// (Win32_Media_Audio, Win32_System_Com, Win32_System_Threading und
// Win32_Foundation). Das Modul ist reines Windows; auf dem Mac laesst es sich
// nicht uebersetzen.
//
// Gebaut und getestet wird es auf der Bau-VM. Die hat kein Tongeraet: der
// echte Weg kommt dort nur bis "kein Standard-Ausgabegeraet" (Test
// echtes_geraet_oder_fehler). Der Wiederaufbau nach einem Geraeteverlust
// (`betreiben`) ist mit einem vorgetaeuschten Geraet getestet (Tests unten).
// UNGEPRUEFT am echten Geraet: Abziehen, Umstellen des Standardformats,
// Wechsel des Standardgeraets.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio_ring::{format_pruefen, sperre, Ausgang, Geteilt, Ring, MAX_MS, ZIEL_MS};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// So lange warten wir auf das Geraeteereignis, bevor wir es fuer tot erklaeren.
const WARTE_MS: u32 = 2000;
/// Nach einem Verlust: Abstand zwischen zwei Versuchen, wieder ein Geraet zu
/// oeffnen.
const NEU_MS: u64 = 500;
/// So oft schauen wir nach, ob Windows inzwischen ein anderes Standardgeraet
/// hat.
const STANDARD_PRUEFEN: Duration = Duration::from_secs(1);
/// Laeuft ein Geraet mindestens so lange, gilt der naechste Verlust wieder
/// als neu und kommt ins Protokoll. Kuerzer: derselbe Grund wie zuletzt wird
/// nur gezaehlt - ein Geraet, das sich oeffnen laesst, aber nie ein Ereignis
/// liefert, schriebe sonst alle 2,5 s zwei Zeilen, ohne Ende.
const STABIL: Duration = Duration::from_secs(30);

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
    // COM fuer diesen Faden. Ein bereits eingerichtetes Apartment (S_FALSE)
    // ist kein Fehler; abgebaut wird, was hier erfolgreich aufgebaut wurde -
    // erst, wenn lauf_com zurueck ist und damit alle COM-Zeiger weg sind.
    let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
    let ergebnis = lauf_com(rate, kanaele, geteilt, tx);
    if com {
        CoUninitialize();
    }
    ergebnis
}

unsafe fn lauf_com(
    rate: u32,
    kanaele: usize,
    geteilt: &Geteilt,
    tx: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let aufzaehler: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("Geraeteliste: {e}"))?;
    let mut wasapi = Wasapi { aufzaehler, rate, kanaele };
    betreiben(&mut wasapi, geteilt, tx, Duration::from_millis(NEU_MS), STABIL, &mut |z| crate::protokoll::zeile(z))
}

// ------------------------------------------------------------------ Wiederaufbau

/// Was der Geraetefaden mit einem Geraet tut. Abgetrennt, damit sich der
/// Wiederaufbau nach einem Verlust ohne Tongeraet pruefen laesst (Tests
/// unten); das echte Geraet ist `Wasapi`.
trait Treiber {
    type Geraet;
    /// Das aktuelle Standardgeraet oeffnen, starten und den Ring passend zu
    /// seinem Format anlegen.
    fn oeffnen(&mut self, geteilt: &Geteilt) -> Result<Self::Geraet, String>;
    /// Nachfuellen, bis `ende` gesetzt ist (None) oder das Geraet nicht mehr
    /// taugt (Some(grund)).
    fn pumpen(&mut self, geraet: &Self::Geraet, geteilt: &Geteilt) -> Option<String>;
}

/// Der Ablauf im Geraetefaden: oeffnen, fuellen, und wenn das Geraet
/// verloren geht, das dann aktuelle Standardgeraet neu oeffnen - so lange,
/// bis es klappt oder der Ausgang endet. Ohne das bliebe der Ton bis zur
/// naechsten Verbindung weg: beide Hosts sagen das Tonformat nur einmal je
/// Zuschauer an, ein neues AudioOut entsteht also nicht von selbst.
///
/// Nur der ERSTE Fehler beim Oeffnen geht an new() und damit in die Anzeige.
/// Danach wird still weiter versucht und nur ins Protokoll geschrieben,
/// derselbe Fehler nur einmal in Folge. Solange kein Geraet offen ist, gibt es
/// keinen Ring: push() verwirft dann, es staut sich nichts, und die Anzeige
/// zeigt 0 ms Vorlauf.
///
/// Das Protokoll hat keine Groessengrenze, also bleibt auch ein Geraet, das
/// immer wieder gleich verloren geht, bei wenigen Zeilen: kommt derselbe Grund
/// wie zuletzt, bevor das Geraet `stabil` lang lief, wird er nur gezaehlt
/// (auch "wieder offen" entfaellt dann), und die Zahl steht einmal da, wenn die
/// Folge endet - anderer Grund, stabiler Lauf oder Ende des Ausgangs.
fn betreiben<T: Treiber>(
    t: &mut T,
    geteilt: &Geteilt,
    tx: &mpsc::Sender<Result<(), String>>,
    pause: Duration,
    stabil: Duration,
    melden: &mut dyn FnMut(String),
) -> Result<(), String> {
    let mut geraet = t.oeffnen(geteilt)?;
    let _ = tx.send(Ok(()));
    // Zuletzt protokollierter Verlustgrund und wie oft er seitdem still
    // wiederkam; zuletzt protokollierter Oeffnungsfehler.
    let mut folge: Option<(String, u32)> = None;
    let mut zuletzt: Option<String> = None;
    loop {
        let seit = Instant::now();
        let Some(grund) = t.pumpen(&geraet, geteilt) else {
            folge_abschliessen(&mut folge, melden);
            return Ok(());
        };
        let lief_stabil = seit.elapsed() >= stabil;
        *sperre(&geteilt.ring) = None;
        drop(geraet);
        if lief_stabil {
            folge_abschliessen(&mut folge, melden);
        }
        let mut still = match folge.as_mut() {
            Some((alt, n)) if *alt == grund => {
                *n += 1;
                true
            }
            _ => false,
        };
        if !still {
            folge_abschliessen(&mut folge, melden);
            melden(format!("Ton: {grund} - Ausgabegeraet wird neu geoeffnet"));
            folge = Some((grund, 0));
            zuletzt = None;
        }
        geraet = loop {
            if warten(geteilt, pause) {
                folge_abschliessen(&mut folge, melden);
                return Ok(());
            }
            match t.oeffnen(geteilt) {
                Ok(g) => break g,
                Err(e) => {
                    if zuletzt.as_deref() != Some(e.as_str()) {
                        melden(format!("Ton: Ausgabegeraet oeffnen: {e}"));
                        zuletzt = Some(e);
                        // Wer den Fehler liest, soll auch lesen, dass es
                        // danach wieder ging.
                        still = false;
                    }
                }
            }
        };
        if !still {
            melden("Ton: Ausgabegeraet wieder offen".into());
        }
    }
}

/// Eine Folge gleicher Verluste abschliessen: stehen stille Wiederholungen
/// aus, ihre Zahl einmal ins Protokoll.
fn folge_abschliessen(folge: &mut Option<(String, u32)>, melden: &mut dyn FnMut(String)) {
    if let Some((grund, n)) = folge.take() {
        if n > 0 {
            melden(format!("Ton: derselbe Verlust ({grund}) noch {n}-mal, nicht einzeln protokolliert"));
        }
    }
}

/// `dauer` warten, in kleinen Schritten, damit ein Ende von aussen (neues
/// Tonformat, Trennung) nicht lange haengt. true, wenn das Ende kam.
fn warten(geteilt: &Geteilt, dauer: Duration) -> bool {
    let bis = Instant::now() + dauer;
    loop {
        if geteilt.ende.load(Ordering::Relaxed) {
            return true;
        }
        let jetzt = Instant::now();
        if jetzt >= bis {
            return false;
        }
        std::thread::sleep((bis - jetzt).min(Duration::from_millis(50)));
    }
}

// ------------------------------------------------------------------ WASAPI

/// Das echte Geraet: WASAPI, geteilter Modus, am Standardgeraet.
struct Wasapi {
    aufzaehler: IMMDeviceEnumerator,
    /// Quellformat, wie der Host es angesagt hat. Bleibt ueber jeden
    /// Wiederaufbau gleich; nur das Geraeteformat kann sich aendern.
    rate: u32,
    kanaele: usize,
}

/// Ein offenes, laufendes Ausgabegeraet. Drop haelt den Strom an und gibt
/// das Ereignis frei.
struct Geraet {
    client: IAudioClient,
    render: IAudioRenderClient,
    ereignis: HANDLE,
    puffer_frames: u32,
    g_kan: usize,
    /// Kennung des Endpunkts, um einen Wechsel des Standardgeraets zu
    /// bemerken. None, wenn Windows keine nennt - dann wird nicht verglichen.
    id: Option<String>,
}

impl Drop for Geraet {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.ereignis);
        }
    }
}

/// Kennung eines Endpunkts als Text. Die Zeichenkette von GetId gehoert uns
/// und wird hier freigegeben.
unsafe fn endpunkt_id(geraet: &IMMDevice) -> Option<String> {
    let p = geraet.GetId().ok()?;
    if p.is_null() {
        return None;
    }
    let id = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const c_void));
    id
}

impl Treiber for Wasapi {
    type Geraet = Geraet;

    fn oeffnen(&mut self, geteilt: &Geteilt) -> Result<Geraet, String> {
        unsafe { oeffnen(&self.aufzaehler, self.rate, self.kanaele, geteilt) }
    }

    fn pumpen(&mut self, g: &Geraet, geteilt: &Geteilt) -> Option<String> {
        unsafe { pumpen(&self.aufzaehler, g, geteilt) }
    }
}

/// Das Standardgeraet oeffnen und starten; der Ring bekommt dessen Format.
unsafe fn oeffnen(
    aufzaehler: &IMMDeviceEnumerator,
    rate: u32,
    kanaele: usize,
    geteilt: &Geteilt,
) -> Result<Geraet, String> {
    let geraet = aufzaehler
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("kein Standard-Ausgabegeraet: {e}"))?;
    let id = endpunkt_id(&geraet);
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

    Ok(Geraet { client, render, ereignis, puffer_frames, g_kan, id })
}

/// Bei jedem Ereignis den freien Teil des Geraetepuffers fuellen. Immer
/// vollstaendig, notfalls mit Stille, sonst stolpert die Mischstufe. None bei
/// `ende`, sonst der Grund, warum das Geraet nicht mehr taugt.
unsafe fn pumpen(aufzaehler: &IMMDeviceEnumerator, g: &Geraet, geteilt: &Geteilt) -> Option<String> {
    let mut geprueft = Instant::now();
    while !geteilt.ende.load(Ordering::Relaxed) {
        // Hat Windows inzwischen ein anderes Standardgeraet? Das alte
        // spielt womoeglich weiter, gehoert wird aber am neuen. Hier
        // oben ist der Puffer gerade gefuellt und deckt die Frage.
        if let Some(alt) = g.id.as_ref().filter(|_| geprueft.elapsed() >= STANDARD_PRUEFEN) {
            geprueft = Instant::now();
            if let Ok(d) = aufzaehler.GetDefaultAudioEndpoint(eRender, eConsole) {
                if endpunkt_id(&d).is_some_and(|neu| neu != *alt) {
                    return Some("anderes Standardgeraet".into());
                }
            }
        }
        // 0 ist WAIT_OBJECT_0. Alles andere heisst Zeitueberschreitung oder
        // Fehler, und beides bedeutet hier: das Geraet ist weg.
        if WaitForSingleObject(g.ereignis, WARTE_MS).0 != 0 {
            return Some(format!("kein Geraeteereignis seit {WARTE_MS} ms"));
        }
        let belegt = match g.client.GetCurrentPadding() {
            Ok(b) => b,
            Err(e) => return Some(format!("Ausgabegeraet verloren: {e}")),
        };
        let frei = g.puffer_frames.saturating_sub(belegt);
        if frei == 0 {
            continue;
        }
        let zeiger = match g.render.GetBuffer(frei) {
            Ok(z) => z,
            Err(e) => return Some(format!("Geraetepuffer holen: {e}")),
        };
        if zeiger.is_null() {
            return Some("Geraet liefert keinen Puffer".into());
        }
        let mangel = {
            let ziel_feld =
                std::slice::from_raw_parts_mut(zeiger as *mut f32, frei as usize * g.g_kan);
            let mut r = sperre(&geteilt.ring);
            match r.as_mut() {
                Some(r) => r.hole(ziel_feld),
                None => {
                    ziel_feld.fill(0.0);
                    false
                }
            }
        };
        if let Err(e) = g.render.ReleaseBuffer(frei, 0) {
            return Some(format!("Geraetepuffer abgeben: {e}"));
        }
        if mangel {
            geteilt.aussetzer.fetch_add(1, Ordering::Relaxed);
        }
    }
    None
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

// --------------------------------------------------------------------- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Vorgetaeuschtes Geraet: jeder Versuch zu oeffnen nimmt das naechste
    /// Ergebnis aus `oeffnen` (leer: Fehler "kein Geraet"), jedes Pumpen den
    /// naechsten Verlustgrund aus `verluste` (leer: Ende von aussen).
    struct Attrappe {
        oeffnen: VecDeque<Result<(), String>>,
        verluste: VecDeque<String>,
        versuche: usize,
        /// Lag beim Versuch schon ein Ring? Nach einem Verlust darf keiner
        /// mehr da sein, sonst stauten sich dort Frames.
        ring_beim_versuch: Vec<bool>,
    }

    impl Attrappe {
        fn neu(oeffnen: Vec<Result<(), &str>>, verluste: Vec<&str>) -> Attrappe {
            Attrappe {
                oeffnen: oeffnen.into_iter().map(|r| r.map_err(|e| e.to_string())).collect(),
                verluste: verluste.into_iter().map(|v| v.to_string()).collect(),
                versuche: 0,
                ring_beim_versuch: Vec::new(),
            }
        }
    }

    impl Treiber for Attrappe {
        type Geraet = usize;

        fn oeffnen(&mut self, geteilt: &Geteilt) -> Result<usize, String> {
            self.versuche += 1;
            self.ring_beim_versuch.push(sperre(&geteilt.ring).is_some());
            self.oeffnen.pop_front().unwrap_or_else(|| Err("kein Geraet".into()))?;
            *sperre(&geteilt.ring) = Some(Ring::neu(48_000, 2, 48_000, 2, 1_440, 5_760));
            Ok(self.versuche)
        }

        fn pumpen(&mut self, _geraet: &usize, _geteilt: &Geteilt) -> Option<String> {
            self.verluste.pop_front()
        }
    }

    fn lauf_mit(t: &mut Attrappe, geteilt: &Geteilt, pause: Duration) -> (Result<(), String>, Vec<Result<(), String>>, Vec<String>) {
        // stabil = 0: jeder Lauf gilt als stabil, jeder Verlust als neu.
        lauf_stabil(t, geteilt, pause, Duration::ZERO)
    }

    fn lauf_stabil(
        t: &mut Attrappe,
        geteilt: &Geteilt,
        pause: Duration,
        stabil: Duration,
    ) -> (Result<(), String>, Vec<Result<(), String>>, Vec<String>) {
        let (tx, rx) = mpsc::channel();
        let mut zeilen = Vec::new();
        let ergebnis = betreiben(t, geteilt, &tx, pause, stabil, &mut |z| zeilen.push(z));
        (ergebnis, rx.try_iter().collect(), zeilen)
    }

    /// Scheitert schon das erste Oeffnen, geht der Fehler an new() - wie
    /// bisher -, und es wird nicht weiter versucht.
    #[test]
    fn erster_fehler_geht_an_new() {
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(vec![Err("kein Standard-Ausgabegeraet")], vec![]);
        let (ergebnis, gemeldet, zeilen) = lauf_mit(&mut t, &geteilt, Duration::from_millis(1));
        assert_eq!(ergebnis, Err("kein Standard-Ausgabegeraet".to_string()));
        assert!(gemeldet.is_empty());
        assert!(zeilen.is_empty());
        assert_eq!(t.versuche, 1);
        assert!(sperre(&geteilt.ring).is_none());
    }

    /// Geraet geht verloren: Ring weg, neu oeffnen, bis es klappt; derselbe
    /// Fehler steht nur einmal im Protokoll. Danach laeuft der Ton wieder
    /// (Ring da, push kommt an), und new() hat genau ein Ok gesehen.
    #[test]
    fn verlust_oeffnet_neu() {
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(
            vec![Ok(()), Err("weg"), Err("weg"), Err("anders"), Ok(())],
            vec!["Ausgabegeraet verloren: 0x88890004"],
        );
        let (ergebnis, gemeldet, zeilen) = lauf_mit(&mut t, &geteilt, Duration::from_millis(1));
        assert_eq!(ergebnis, Ok(()));
        assert_eq!(gemeldet, vec![Ok(())]);
        assert_eq!(t.versuche, 5);
        assert_eq!(t.ring_beim_versuch, vec![false; 5]);
        assert_eq!(
            zeilen,
            vec![
                "Ton: Ausgabegeraet verloren: 0x88890004 - Ausgabegeraet wird neu geoeffnet".to_string(),
                "Ton: Ausgabegeraet oeffnen: weg".to_string(),
                "Ton: Ausgabegeraet oeffnen: anders".to_string(),
                "Ton: Ausgabegeraet wieder offen".to_string(),
            ]
        );
        assert!(!geteilt.tot.load(Ordering::Relaxed));
        let ausgang = Ausgang { geteilt: geteilt.clone() };
        ausgang.push(&[0.5; 96]);
        assert!(ausgang.stats().0 > 0.0, "nach dem Wiederaufbau kommt nichts im Ring an");
    }

    /// Mehrere Verluste hintereinander: jedes Mal wieder offen.
    #[test]
    fn mehrfacher_verlust() {
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(vec![Ok(()), Ok(()), Ok(())], vec!["anderes Standardgeraet", "kein Geraeteereignis seit 2000 ms"]);
        let (ergebnis, gemeldet, zeilen) = lauf_mit(&mut t, &geteilt, Duration::from_millis(1));
        assert_eq!(ergebnis, Ok(()));
        assert_eq!(gemeldet.len(), 1);
        assert_eq!(t.versuche, 3);
        assert_eq!(zeilen.iter().filter(|z| z.ends_with("wieder offen")).count(), 2);
    }

    /// Ein Geraet, das sich oeffnen laesst, aber immer gleich wieder verloren
    /// geht (etwa nie ein Ereignis liefert): der Grund steht einmal da, die
    /// Wiederholungen nur als Zahl am Ende der Folge. Ein anderer Grund oder
    /// ein stabiler Lauf beendet die Folge.
    #[test]
    fn gleicher_verlust_nur_gezaehlt() {
        const G: &str = "kein Geraeteereignis seit 2000 ms";
        let neu = format!("Ton: {G} - Ausgabegeraet wird neu geoeffnet");
        let offen = "Ton: Ausgabegeraet wieder offen".to_string();

        // Fuenfmal derselbe Grund, nie stabil: drei Zeilen statt zehn.
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(vec![Ok(()); 6], vec![G; 5]);
        let (ergebnis, gemeldet, zeilen) =
            lauf_stabil(&mut t, &geteilt, Duration::from_millis(1), Duration::from_secs(3600));
        assert_eq!(ergebnis, Ok(()));
        assert_eq!(gemeldet, vec![Ok(())]);
        assert_eq!(t.versuche, 6);
        assert_eq!(t.ring_beim_versuch, vec![false; 6]);
        assert_eq!(
            zeilen,
            vec![
                neu.clone(),
                offen.clone(),
                format!("Ton: derselbe Verlust ({G}) noch 4-mal, nicht einzeln protokolliert"),
            ]
        );

        // Dazwischen ein anderer Grund: die Folge endet mit ihrer Zahl, der
        // neue Grund steht da, und danach ist der alte wieder neu. Ein
        // Oeffnungsfehler mitten in einer stillen Folge kommt ins Protokoll -
        // und dann auch, dass es wieder ging.
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(
            vec![Ok(()), Ok(()), Err("weg"), Ok(()), Ok(()), Ok(())],
            vec![G, G, "anderes Standardgeraet", G],
        );
        let (ergebnis, _, zeilen) =
            lauf_stabil(&mut t, &geteilt, Duration::from_millis(1), Duration::from_secs(3600));
        assert_eq!(ergebnis, Ok(()));
        assert_eq!(
            zeilen,
            vec![
                neu.clone(),
                offen.clone(),
                "Ton: Ausgabegeraet oeffnen: weg".to_string(),
                offen.clone(),
                format!("Ton: derselbe Verlust ({G}) noch 1-mal, nicht einzeln protokolliert"),
                "Ton: anderes Standardgeraet - Ausgabegeraet wird neu geoeffnet".to_string(),
                offen.clone(),
                neu.clone(),
                offen.clone(),
            ]
        );

        // Lief das Geraet jedes Mal stabil, ist jeder Verlust wieder neu.
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(vec![Ok(()); 3], vec![G; 2]);
        let (_, _, zeilen) = lauf_mit(&mut t, &geteilt, Duration::from_millis(1));
        assert_eq!(zeilen, vec![neu.clone(), offen.clone(), neu, offen]);
    }

    /// Kommt kein Geraet wieder, wird weiter versucht - bis der Ausgang endet
    /// (neues Tonformat, Trennung). Das Ende wartet nicht auf die Pause.
    #[test]
    fn ende_bricht_wiederaufbau_ab() {
        let geteilt = Geteilt::neu();
        let mut t = Attrappe::neu(vec![Ok(())], vec!["Ausgabegeraet verloren"]);
        let aussen = geteilt.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            aussen.ende.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let (ergebnis, _, zeilen) = lauf_mit(&mut t, &geteilt, Duration::from_millis(20));
        stopper.join().unwrap();
        assert_eq!(ergebnis, Ok(()));
        assert!(start.elapsed() < Duration::from_secs(2), "Ende haengt: {:?}", start.elapsed());
        assert!(t.versuche > 2, "nur {} Versuche", t.versuche);
        assert_eq!(zeilen.iter().filter(|z| z.contains("oeffnen: kein Geraet")).count(), 1);
        assert!(sperre(&geteilt.ring).is_none());

        // Auch mit der Vorgabe NEU_MS endet das Warten sofort.
        let geteilt = Geteilt::neu();
        geteilt.ende.store(true, Ordering::Relaxed);
        let start = Instant::now();
        assert!(warten(&geteilt, Duration::from_millis(NEU_MS)));
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    /// Am echten WASAPI: ohne Tongeraet (Bau-VM) kommt der Fehler des ersten
    /// Versuchs zurueck, mit Geraet ein laufender Ausgang, der beim
    /// Fallenlassen endet (spielt 20 ms Stille). Haengen darf beides nicht.
    #[test]
    fn echtes_geraet_oder_fehler() {
        let start = Instant::now();
        match AudioOut::new(48_000, 2) {
            Ok(a) => {
                a.push(&[0.0; 1_920]);
                println!("Tongeraet offen");
            }
            Err(e) => {
                println!("Tongeraet: {e}");
                assert!(!e.is_empty());
            }
        }
        assert!(start.elapsed() < Duration::from_secs(5), "haengt: {:?}", start.elapsed());
    }
}
