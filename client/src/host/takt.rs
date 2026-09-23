// Schrittmacher des Windows-Hosts, 1:1 nach main.m: ein festes Raster von
// 1/fps, ein Bild bekommt den naechsten freien Schlitz oder faellt als "zu
// schnell" weg; der Encoder darf hoechstens drei Bilder (Aufnahme) bzw. zwei
// (Takt) unterwegs haben; die feste Bildrate legt das letzte Bild nach, wenn
// seit 0,9/fps nichts kam - nachgelegte Bilder ruecken das Raster NICHT
// weiter; der Stempel traegt die ECHTE Aufnahmezeit. Nach Raster und
// Encoder-Grenze fragt jedes Bild noch die Stauregel (netz.rs,
// stau_vor_dem_encoder); ein Bild, das sie auslaesst, geht nicht in den
// Encoder.
//
// Der Takt selbst laeuft auf dem Aufnahmefaden: AcquireNextFrame wartet
// hoechstens bis zum naechsten Schlag (frist_ms), danach entscheidet
// tick_faellig, ob nachgelegt wird. timeBeginPeriod(1) waehrend der Sitzung
// haelt die Fristen auf die Millisekunde genau.
//
// Encoder mit Vorlauf (h264_mf in Software haelt 16 Bilder zurueck): ohne
// feste Bildrate kaeme ein Bild erst heraus, wenn 16 weitere hineingehen -
// das Endbild einer Bewegung, das erste Bild fuer einen neuen Zuschauer
// oder das Bild nach einem Codecwechsel blieben bei stillem Desktop im
// Encoder stecken. Dann schiebt der Takt das letzte Bild in Zielrate nach
// (vorlauf_nachschieben), bis das letzte echte Bild heraus ist, und hoert
// auf. Ohne Vorlauf (nvenc, delay 0) gibt es nichts nachzuschieben.

use std::time::{Duration, Instant};

/// Ohne feste Bildrate: soll der Takt das letzte Bild noch einmal in den
/// Encoder schieben, damit dessen Vorlauf ein echtes Bild herausgibt? Ja,
/// wenn der Encoder Bilder zurueckhaelt (`vorlauf` > 0), noch echte Bilder
/// in ihm stecken, deren Paket nicht heraus ist (`ausstehend`: neuer Inhalt
/// oder erzwungenes Vollbild), und seit einer ganzen Bildzeit kein neues
/// Bild ankam - waehrend einer Bewegung schieben die neuen Bilder selbst.
/// Mit fester Bildrate legt der Takt ohnehin jeden Schlag nach.
pub fn vorlauf_nachschieben(vorlauf: usize, ausstehend: usize, seit_ankunft_us: u64, fps: u32, fest: bool) -> bool {
    !fest && vorlauf > 0 && ausstehend > 0 && seit_ankunft_us >= 1_000_000 / fps.max(1) as u64
}

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

    /// Ein Bild, das die Stauregel vor dem Encoder ausgelassen hat, zaehlt
    /// fuer "seit 0,9/fps nichts" wie eines, das hineinging (g_last_pts in
    /// main.m wird auch dann gesetzt); die pts bleiben, wie sie sind.
    pub fn ausgelassen(&mut self) {
        self.letztes_bild = Instant::now();
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

    #[test]
    fn vorlauf_wird_nur_bei_bedarf_geleert() {
        let bildzeit = 1_000_000 / 30;
        // h264_mf (Vorlauf 16), ohne feste Bildrate, ein echtes Bild steckt
        // noch im Encoder, seit einer Bildzeit nichts Neues: nachschieben.
        assert!(vorlauf_nachschieben(16, 1, bildzeit, 30, false));
        assert!(vorlauf_nachschieben(16, 3, 10 * bildzeit, 30, false));
        // Waehrend einer Bewegung schieben die neuen Bilder selbst.
        assert!(!vorlauf_nachschieben(16, 1, bildzeit - 1, 30, false));
        assert!(!vorlauf_nachschieben(16, 1, 0, 30, false));
        // Das letzte echte Bild ist heraus: Ruhe, auch bei stillem Desktop.
        assert!(!vorlauf_nachschieben(16, 0, 60_000_000, 30, false));
        // Ohne Vorlauf (nvenc, delay 0): nichts, wie bisher.
        assert!(!vorlauf_nachschieben(0, 1, 60_000_000, 30, false));
        // Feste Bildrate: der Takt legt ohnehin nach, hier nichts zusaetzlich.
        assert!(!vorlauf_nachschieben(16, 1, 60_000_000, 30, true));
        // Die Bildzeit folgt der Zielrate; fps 0 teilt nicht durch null.
        assert!(!vorlauf_nachschieben(16, 1, 1_000_000 / 120 - 1, 120, false));
        assert!(vorlauf_nachschieben(16, 1, 1_000_000 / 120, 120, false));
        assert!(vorlauf_nachschieben(16, 1, 1_000_000, 0, false));
    }

    /// Encoder mit Vorlauf wie h264_mf (mf_probe): ein Bild kommt heraus,
    /// sobald `vorlauf` weitere hinter ihm hineingingen.
    struct Vorlaufmodell {
        vorlauf: usize,
        drin: std::collections::VecDeque<(u32, bool)>,
        heraus: Vec<u32>,
    }

    impl Vorlaufmodell {
        fn geben(&mut self, nr: u32, echt: bool) {
            self.drin.push_back((nr, echt));
            while self.drin.len() > self.vorlauf {
                self.heraus.push(self.drin.pop_front().unwrap().0);
            }
        }
        fn ausstehend(&self) -> usize {
            self.drin.iter().filter(|x| x.1).count()
        }
    }

    /// Takt und Aufnahme im Zeitraffer (1 ms Schritte): Bilder kommen mit
    /// `hz` von `von` bis `bis` (us), Zielrate 30, keine feste Bildrate.
    /// Ein Bild ohne Schlitz legt der Takt nach einer Bildzeit nach (wie
    /// ausgelassenes_faellig). Liefert das Modell, die Nummer des letzten
    /// Bildes und die Zeitpunkte der Nachschuebe.
    fn zeitraffer(vorlauf: usize, hz: f64, von: u64, bis: u64) -> (Vorlaufmodell, u32, Vec<u64>) {
        let fps = 30;
        let bildzeit = 1_000_000 / fps as u64;
        let mut enc = Vorlaufmodell { vorlauf, drin: Default::default(), heraus: Vec::new() };
        let mut s = Schrittmacher::neu();
        let (mut nr, mut ankunft, mut offen) = (0u32, 0u64, None);
        let mut schub = Vec::new();
        let (mut naechstes, mut tick) = (von as f64, 0u64);
        for t in (0..20_000_000u64).step_by(1000) {
            if (t as f64) >= naechstes && t <= bis {
                naechstes += 1e6 / hz;
                nr += 1;
                ankunft = t;
                if s.schlitz_frei(t as f64 / 1e6, fps) {
                    enc.geben(nr, true);
                    offen = None;
                } else {
                    offen = Some(nr);
                }
            }
            if t >= tick {
                tick += bildzeit;
                if let (Some(n), true) = (offen, t - ankunft >= bildzeit) {
                    enc.geben(n, true);
                    offen = None;
                } else if vorlauf_nachschieben(enc.vorlauf, enc.ausstehend(), t - ankunft, fps, false) {
                    enc.geben(0, false);
                    schub.push(t);
                }
            }
        }
        (enc, nr, schub)
    }

    #[test]
    fn vorlauf_endbild_kommt_heraus_danach_ruhe() {
        let bildzeit = 1_000_000 / 30;
        // Bewegung 2 s (60 Hz, 144 Hz, 25 Hz), dann 17 s still: mit Vorlauf
        // 16 genau 16 Nachschuebe nach dem Ende der Bewegung, der letzte
        // hoechstens 18 Bildzeiten nach ihr; das Endbild ist heraus, danach
        // nichts mehr.
        for hz in [60.0, 144.0, 25.0] {
            let (enc, ende, schub) = zeitraffer(16, hz, 1_000_000, 3_000_000);
            assert!(enc.heraus.contains(&ende), "{hz} Hz: Endbild steckt im Encoder");
            assert_eq!(enc.ausstehend(), 0, "{hz} Hz");
            // Bei 25 Hz (langsamer als die Zielrate) schiebt der Takt auch
            // zwischen zwei Bildern - dort kommt jedes nach hoechstens 16
            // Bildzeiten, statt erst mit dem sechzehnten Nachfolger.
            let nach_ende: Vec<_> = schub.iter().filter(|&&t| t > 3_000_000).collect();
            assert_eq!(nach_ende.len(), 16, "{hz} Hz: {schub:?}");
            assert!(*nach_ende[15] <= 3_000_000 + 18 * bildzeit, "{hz} Hz: {schub:?}");
            if hz > 30.0 {
                assert_eq!(schub.len(), 16, "{hz} Hz: waehrend der Bewegung geschoben: {schub:?}");
            }
        }
        // Neuer Zuschauer bei stillem Desktop: ein einziges Bild, dann
        // Ruhe - es ist nach 16 Nachschueben (gut eine halbe Sekunde) heraus.
        let (enc, ende, schub) = zeitraffer(16, 60.0, 1_000_000, 1_000_000);
        assert_eq!(ende, 1);
        assert_eq!(enc.heraus, vec![1]);
        assert_eq!(schub.len(), 16);
        assert!(*schub.last().unwrap() <= 1_000_000 + 18 * bildzeit, "{schub:?}");
        // Ohne Vorlauf (nvenc): nie ein Nachschub, alles sofort heraus.
        let (enc, ende, schub) = zeitraffer(0, 60.0, 1_000_000, 3_000_000);
        assert!(schub.is_empty());
        assert!(enc.heraus.contains(&ende));
    }
}
