// Schrittmacher des Windows-Hosts, 1:1 nach main.m: ein festes Raster von
// 1/fps, ein Bild bekommt den naechsten freien Schlitz oder faellt als "zu
// schnell" weg; der Encoder darf hoechstens drei Bilder (Aufnahme) bzw. zwei
// (Takt) unterwegs haben; die feste Bildrate legt das letzte Bild nach, wenn
// seit 0,9/fps nichts kam - nachgelegte Bilder ruecken das Raster NICHT
// weiter; der Stempel traegt die ECHTE Aufnahmezeit.
//
// Der Takt selbst laeuft auf dem Aufnahmefaden: AcquireNextFrame wartet
// hoechstens bis zum naechsten Schlag (frist_ms), danach entscheidet
// tick_faellig, ob nachgelegt wird. timeBeginPeriod(1) waehrend der Sitzung
// haelt die Fristen auf die Millisekunde genau.

use std::time::{Duration, Instant};

/// Encoder-Inflight-Grenze fuer frisch aufgenommene Bilder.
pub const INFLIGHT_AUFNAHME: usize = 3;
/// Grenze fuer nachgelegte Bilder - ein Auslasser ist billiger als eine
/// wachsende Warteschlange.
pub const INFLIGHT_TAKT: usize = 2;

pub struct Schrittmacher {
    /// Naechster freier Schlitz, Sekunden der Hostuhr.
    schlitz: f64,
    /// Naechster Schlag des Takts.
    naechster: Instant,
    /// Zeit (us) des letzten Bildes, das in den Encoder ging.
    pub last_pts_us: i64,
    /// Wann das letzte Bild in den Encoder ging (fuer "seit 0,9/fps nichts").
    pub letztes_bild: Instant,
}

impl Schrittmacher {
    pub fn neu() -> Schrittmacher {
        Schrittmacher { schlitz: 0.0, naechster: Instant::now(), last_pts_us: 0, letztes_bild: Instant::now() - Duration::from_secs(1) }
    }

    /// Ist fuer ein Bild zur Zeit t (Hostuhr, Sekunden) ein Schlitz frei?
    /// Wenn ja, ist er damit belegt. Das Raster ist fest: der naechste Schlitz
    /// liegt eine Bildzeit hinter dem vorigen, nicht hinter diesem Bild -
    /// sonst wuerde ein Strom mit 148 Bildern je Sekunde auf jedes zweite
    /// Bild halbiert statt auf 120 gedeckelt. Nach einer Pause faengt das
    /// Raster bei diesem Bild neu an. Ein Zehntel Bildzeit zu frueh ist noch
    /// recht (Zitter der Aufnahme).
    pub fn schlitz_frei(&mut self, t: f64, fps: u32) -> bool {
        if fps == 0 {
            return true;
        }
        let bildzeit = 1.0 / fps as f64;
        if t + 0.1 * bildzeit < self.schlitz {
            return false;
        }
        self.schlitz = if t > self.schlitz + bildzeit { t + bildzeit } else { self.schlitz + bildzeit };
        true
    }

    /// Frist bis zum naechsten Schlag, in ganzen Millisekunden (mindestens 1).
    pub fn frist_ms(&self) -> u32 {
        let jetzt = Instant::now();
        if self.naechster <= jetzt {
            return 1;
        }
        ((self.naechster - jetzt).as_secs_f64() * 1000.0).ceil().max(1.0) as u32
    }

    /// Ist der Takt faellig? Dann rueckt der Schlag um 1/fps weiter (nach
    /// einer langen Pause vom Jetzt aus, damit kein Schlagstau abgearbeitet
    /// wird).
    pub fn tick_faellig(&mut self, fps: u32) -> bool {
        let jetzt = Instant::now();
        if jetzt < self.naechster {
            return false;
        }
        let takt = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
        self.naechster = if jetzt - self.naechster > takt * 4 { jetzt + takt } else { self.naechster + takt };
        true
    }

    /// Kam seit 0,9/fps kein Bild? Dann darf nachgelegt werden.
    pub fn nachlegen_faellig(&self, fps: u32) -> bool {
        self.letztes_bild.elapsed().as_secs_f64() >= 0.9 / fps.max(1) as f64
    }

    /// Zeitstempel fuer den Encoder: muss immer vorwaerts gehen, sonst weist
    /// er das Bild ab. Merkt sich das Bild als das letzte.
    pub fn pts_vorwaerts(&mut self, gewuenscht_us: i64, fps: u32) -> i64 {
        let pts = if gewuenscht_us <= self.last_pts_us { self.last_pts_us + (1_000_000 / fps.max(1) as i64).max(1) } else { gewuenscht_us };
        self.last_pts_us = pts;
        self.letztes_bild = Instant::now();
        pts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_deckelt_und_setzt_neu_an() {
        let mut s = Schrittmacher::neu();
        // 148 Bilder je Sekunde auf 120 gedeckelt: von 148 Bildern kommen
        // etwa 120 durch, nicht 74.
        let mut durch = 0;
        for i in 0..148 {
            if s.schlitz_frei(i as f64 / 148.0, 120) {
                durch += 1;
            }
        }
        assert!((118..=121).contains(&durch), "{durch}");
        // Nach einer Pause faengt das Raster beim Bild neu an.
        assert!(s.schlitz_frei(10.0, 120));
        assert!(!s.schlitz_frei(10.001, 120));
        // Ein Zehntel zu frueh ist noch recht.
        assert!(s.schlitz_frei(10.0 + 1.0 / 120.0 - 0.0005, 120));
        // pts geht immer vorwaerts.
        let a = s.pts_vorwaerts(5000, 60);
        let b = s.pts_vorwaerts(4000, 60);
        assert!(b > a);
    }
}
