// Zeitregel der Hosts: wie viel darf vor einem Bild warten?
//
// Beide Hosts lassen ein Bild vor dem Encoder aus, wenn zu viel ungesendet
// auf den Zuschauer wartet (Stauregel: stau_vor_dem_encoder in host/main.m
// und host/netz.rs). Bis zum 29.09.2026 war "zu viel" nur eine Bytegrenze
// (2 MB, im Spielmodus 512 kB) - bei 10 Mbit/s sind das 1,6 s Bild, und so
// lange wartete ein Bild, bevor die Regel griff (Messung Alienware -> Mac mini
// M1: Kette bis 2,7 s). Der Nutzer spuert Latenz, nicht Bytes. Deshalb gilt
// dazu eine Zeitgrenze: ein Bild faellt auch aus, wenn mehr wartet, als der
// Zuschauer in STAU_ZEIT abnimmt - nie aber unter STAU_ZEIT_MIN, damit ein
// Vollbild auf einer gesunden Leitung kein folgendes Bild kostet, und nie
// ueber der Bytegrenze.
//
// Was der Zuschauer abnimmt, misst `Abfluss` - nur, solange wirklich etwas
// wartet (ueber BESETZT): dann ist es das, was Leitung und Leser schaffen,
// und nicht bloss das, was der Encoder gerade liefert. Je Messabschnitt
// (mindestens TAKT) ein Wert, gemittelt mit dem vorigen - macOS meldet die
// Abnahme eines langsamen Empfaengers in Schueben (host/main.m,
// QC_SCHUB_AB). Ohne Messung gilt die Bytegrenze allein, ebenso wenn die
// letzte ALTER her ist - und wenn der Zuschauer seit STILL gar nichts mehr
// abgenommen hat: ein eingefrorener Zuschauer laeuft dann wie bisher in die
// Bytegrenze, und die Frist fuer "weg" (samt Nachsicht fuer Schuebe auf dem
// Mac) entscheidet ueber ihn wie bisher. Die Zeitregel laesst Bilder nur
// aus; einen Zuschauer traegt sie nie aus.
//
// Der Windows-Host (host/netz.rs) nimmt diese Fassung, der Mac-Host
// rechnet dasselbe in C (host/main.m: abfluss_verfolgen, stau_grenze_aus) -
// dieselben Zahlen, beide geprueft (hier und in host/hosttest.m).

/// So lange darf ein Bild hoechstens hinter dem warten, was schon beim
/// Zuschauer ansteht.
pub const STAU_ZEIT_US: u64 = 100_000;
/// Darunter greift die Zeitregel nie: ein Vollbild bei niedriger Bitrate
/// soll auf einer gesunden Leitung kein folgendes Bild kosten.
pub const STAU_ZEIT_MIN: usize = 128 * 1024;
/// Gemessen wird nur, solange mehr als das wartet.
pub const BESETZT: usize = 64 * 1024;
/// Laenge eines Messabschnitts.
pub const TAKT_US: u64 = 250_000;
/// Hat der Zuschauer so lange nichts abgenommen, gilt die Messung nicht.
pub const STILL_US: u64 = 2_500_000;
/// Eine Messung, die so alt ist, gilt nicht mehr.
pub const ALTER_US: u64 = 10_000_000;

/// Die Grenze der Stauregel in Byte: die Bytegrenze, und sobald bekannt ist,
/// was abfliesst (Byte/s), hoechstens so viel, wie in STAU_ZEIT abfliesst -
/// aber nie unter STAU_ZEIT_MIN (und nie ueber der Bytegrenze).
pub fn zeit_grenze(rate: Option<f64>, bytegrenze: usize) -> usize {
    match rate {
        None => bytegrenze,
        Some(r) => ((r * STAU_ZEIT_US as f64 / 1e6) as usize).max(STAU_ZEIT_MIN).min(bytegrenze),
    }
}

/// Was ein Zuschauer abnimmt (siehe oben).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Abfluss {
    /// Beginn des laufenden Messabschnitts: Zeit und abgenommen.
    beginn: Option<(u64, u64)>,
    /// Byte/s, 0 = keine Messung.
    rate: f64,
    /// Zeit der letzten Messung.
    gemessen: u64,
    /// Wann zuletzt eine Abnahme zu sehen war, und wie viel bis dahin.
    bewegt: u64,
    abgenommen: u64,
}

impl Abfluss {
    /// Ein Blick: `abgenommen` - was der Zuschauer bisher insgesamt
    /// abgenommen hat (waechst nur), `rueckstand` - was noch wartet.
    pub fn beobachten(&mut self, jetzt: u64, abgenommen: u64, rueckstand: usize) {
        if abgenommen > self.abgenommen {
            self.abgenommen = abgenommen;
            self.bewegt = jetzt;
        }
        if rueckstand <= BESETZT {
            self.beginn = None;
            return;
        }
        match self.beginn {
            None => self.beginn = Some((jetzt, abgenommen)),
            Some((t, a)) if jetzt.saturating_sub(t) >= TAKT_US => {
                let r = abgenommen.saturating_sub(a) as f64 * 1e6 / jetzt.saturating_sub(t) as f64;
                self.rate = if self.rate > 0.0 { 0.5 * (self.rate + r) } else { r };
                self.gemessen = jetzt;
                self.beginn = Some((jetzt, abgenommen));
            }
            Some(_) => {}
        }
    }

    /// Was abfliesst, in Byte/s - None ohne (gueltige) Messung oder wenn der
    /// Zuschauer seit STILL nichts abgenommen hat.
    pub fn rate(&self, jetzt: u64) -> Option<f64> {
        (self.rate > 0.0
            && jetzt.saturating_sub(self.gemessen) <= ALTER_US
            && jetzt.saturating_sub(self.bewegt) <= STILL_US)
            .then_some(self.rate)
    }

    /// Die Grenze fuer den Rueckstand vor dem naechsten Bild (zeit_grenze).
    pub fn grenze(&self, jetzt: u64, bytegrenze: usize) -> usize {
        zeit_grenze(self.rate(jetzt), bytegrenze)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1024 * 1024;

    #[test]
    fn grenze_nach_abfluss() {
        // Ohne Messung: die Bytegrenze.
        assert_eq!(zeit_grenze(None, 2 * MB), 2 * MB);
        // 10 Mbit/s (1,25 MB/s): 100 ms waeren 125 kB - die Untergrenze gilt.
        assert_eq!(zeit_grenze(Some(1_250_000.0), 2 * MB), STAU_ZEIT_MIN);
        // 100 Mbit/s: 1,25 MB statt 2 MB.
        assert_eq!(zeit_grenze(Some(12_500_000.0), 2 * MB), 1_250_000);
        // Gigabit: die Bytegrenze bleibt die Obergrenze, auch im Spielmodus.
        assert_eq!(zeit_grenze(Some(117_000_000.0), 2 * MB), 2 * MB);
        assert_eq!(zeit_grenze(Some(117_000_000.0), MB / 2), MB / 2);
        // Nie ueber einer kleineren Bytegrenze, auch nicht mit der Untergrenze.
        assert_eq!(zeit_grenze(Some(1.0), 64 * 1024), 64 * 1024);
    }

    #[test]
    fn misst_nur_wenn_etwas_wartet() {
        let mut a = Abfluss::default();
        let mut t = 1_000_000u64;
        let mut ab = 0u64;
        // Kaum etwas wartet: keine Messung, obwohl viel abfliesst.
        for _ in 0..100 {
            t += 10_000;
            ab += 100_000;
            a.beobachten(t, ab, 10_000);
        }
        assert_eq!(a.rate(t), None);
        assert_eq!(a.grenze(t, 2 * MB), 2 * MB);
        // Es staut sich, der Zuschauer nimmt 1 MB/s ab.
        for _ in 0..100 {
            t += 10_000;
            ab += 10_000;
            a.beobachten(t, ab, 500_000);
        }
        let r = a.rate(t).expect("Messung");
        assert!((r - 1_000_000.0).abs() < 1.0, "{r}");
        assert_eq!(a.grenze(t, 2 * MB), STAU_ZEIT_MIN);
        // Schuebe (macOS): 250 kB auf einmal, dann 250 ms nichts - im Mittel
        // bleibt es bei rund 1 MB/s, die Grenze bei der Untergrenze.
        for i in 0..40 {
            t += TAKT_US;
            if i % 2 == 0 {
                ab += 500_000;
            }
            a.beobachten(t, ab, 500_000);
        }
        let r = a.rate(t).expect("Messung");
        assert!(r > 500_000.0 && r < 1_600_000.0, "{r}");
        // Ein schneller Leser: die Grenze folgt ihm binnen weniger Abschnitte.
        for _ in 0..8 {
            t += TAKT_US;
            ab += 10_000_000;
            a.beobachten(t, ab, 3 * MB);
        }
        let r = a.rate(t).expect("Messung");
        assert!(r > 35_000_000.0, "{r}");
        assert_eq!(a.grenze(t, 2 * MB), 2 * MB);
    }

    /// Ein Zuschauer, der nichts mehr abnimmt: nach STILL gilt wieder die
    /// Bytegrenze - dort entscheidet die Frist, ob er weg ist. Eine alte
    /// Messung verfaellt nach ALTER.
    #[test]
    fn eingefroren_oder_alt_gilt_die_bytegrenze() {
        let mut a = Abfluss::default();
        let mut t = 5_000_000u64;
        let mut ab = 0u64;
        for _ in 0..20 {
            t += 100_000;
            ab += 100_000;
            a.beobachten(t, ab, 300_000);
        }
        assert!(a.rate(t).is_some());
        let eingefroren = t;
        while t < eingefroren + STILL_US {
            t += 100_000;
            a.beobachten(t, ab, 400_000);
            assert!(a.grenze(t, 2 * MB) < 2 * MB, "{t}");
        }
        t += 100_000;
        a.beobachten(t, ab, 400_000);
        assert_eq!(a.rate(t), None);
        assert_eq!(a.grenze(t, 2 * MB), 2 * MB);
        // Eine Messung, dann nichts mehr wartend: nach ALTER verfallen.
        let mut b = Abfluss::default();
        b.beobachten(1_000_000, 0, 200_000);
        b.beobachten(1_300_000, 300_000, 200_000);
        assert!(b.rate(1_300_000).is_some());
        b.beobachten(1_400_000, 400_000, 0);
        assert!(b.rate(1_300_000 + STILL_US).is_some());
        assert_eq!(b.rate(1_300_000 + ALTER_US + 1), None);
    }
}
