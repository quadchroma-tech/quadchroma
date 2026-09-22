// QuadChroma Client, Tonausgabe: der Teil, der auf allen Plattformen gleich ist.
//
// Der Ton kommt als verschraenktes float32 ueber die Videoverbindung (Nachricht
// 32 Tonformat, Nachricht 33 Tondaten). Zwischen Netzfaden und Geraetefaden
// liegt ein Ringpuffer fester Groesse: Ziel 30 ms Vorlauf, harte Obergrenze
// 120 ms. Laeuft er ueber, fliegen die aeltesten Frames raus statt dass die
// Verzoegerung waechst -- lieber ein kurzer Aussetzer als ein Ton, der dem Bild
// hinterherhinkt. Aussetzer werden gezaehlt und ueber stats() fuer die Anzeige
// herausgereicht.
//
// Die Ausgabeschicht ist je Plattform eine eigene Datei: audio.rs (WASAPI,
// Windows) und audio_mac.rs (AudioQueue, macOS). Beide nehmen ihre Frames aus
// demselben Ring und zaehlen in demselben `Geteilt`; nur das Geraet ist anders.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Vorlauf, den wir anstreben, bevor der erste Ton herausgeht.
pub const ZIEL_MS: u32 = 30;
/// Harte Obergrenze. Darueber wird verworfen, nicht gepuffert.
pub const MAX_MS: u32 = 120;

// ------------------------------------------------------------------ Ringpuffer

/// Frames liegen verschraenkt im Geraeteformat. Schreiber ist der Netzfaden,
/// Leser der Geraetefaden; beide fassen ihn nur kurz an, es wird darin nur
/// kopiert, nie gewartet und nie belegt.
pub struct Ring {
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
    pub fn neu(q_rate: u32, q_kan: usize, g_rate: u32, g_kan: usize, ziel: usize, kap: usize) -> Self {
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
    pub fn hole(&mut self, out: &mut [f32]) -> bool {
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

pub struct Geteilt {
    pub ring: Mutex<Option<Ring>>,
    pub tot: AtomicBool,       // Geraet weg, push() macht nichts mehr
    pub ende: AtomicBool,      // Abbruch von aussen, siehe Drop
    pub aussetzer: AtomicU64,  // Puffer lief leer
    pub ueberlauf: AtomicU64,  // Frames verworfen, weil die Obergrenze erreicht war
}

impl Geteilt {
    pub fn neu() -> Arc<Geteilt> {
        Arc::new(Geteilt {
            ring: Mutex::new(None),
            tot: AtomicBool::new(false),
            ende: AtomicBool::new(false),
            aussetzer: AtomicU64::new(0),
            ueberlauf: AtomicU64::new(0),
        })
    }
}

/// Ein vergifteter Mutex darf die Tonausgabe nicht mitreissen.
pub fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Rate, Kanalzahl auf Plausibilitaet pruefen - dieselben Grenzen auf
/// beiden Plattformen, damit ein unsinniges Tonformat nie ein Geraet oeffnet.
pub fn format_pruefen(rate: u32, channels: u16) -> Result<(), String> {
    if !(8000..=384_000).contains(&rate) {
        return Err(format!("unplausible Abtastrate: {rate}"));
    }
    if channels == 0 || channels > 8 {
        return Err(format!("unplausible Kanalzahl: {channels}"));
    }
    Ok(())
}

// ------------------------------------------------------------------ Aussenseite

/// Die Seite zum Netzfaden hin: Frames annehmen, Fuellstand melden. Die
/// Plattformdatei haelt eine davon und haengt ihr Geraet an `geteilt`.
pub struct Ausgang {
    pub geteilt: Arc<Geteilt>,
}

impl Ausgang {
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

impl Drop for Ausgang {
    fn drop(&mut self) {
        self.geteilt.ende.store(true, Ordering::Relaxed);
    }
}
