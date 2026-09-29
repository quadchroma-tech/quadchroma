// Bildratenregler des Clients: haelt die Verzoegerung klein, wenn dieser
// Rechner nicht so schnell decodiert, wie der Host Bilder schickt.
//
// Der Empfangsfaden (run_session) decodiert jedes Bild selbst, eines nach
// dem anderen. Schickt der Host mehr Bilder, als dieser Rechner schafft,
// faellt keines aus - sie stauen sich in den Puffern zwischen Host und
// Client (Empfangspuffer hier, Sendepuffer und Warteschlange dort), und die
// Verzoegerung waechst ohne Ende. Gemessen am 29.09.2026: Mac mini M1 an
// einem 4K-Bildschirm mit 240 Hz, HEVC 4:4:4 braucht rund 18 ms je Bild
// (55 Bilder/s, 4:2:0 rund 12 ms), der Host schickte 60 oder 120 - Kette
// 0,3 bis 2,7 s, nur manchmal fing sie sich wieder. Die Stauregel des Hosts
// sieht nur seine eigenen Puffer und griff bei niedriger Bitrate erst nach
// Sekunden (2 MB bei 10 Mbit/s sind 1,6 s).
//
// Der Regler misst drei Dinge:
//   - Rueckstand: wie viel spaeter als die schnellsten Bilder der letzten
//     GRUND_SEKUNDEN dieses Bild ankam - Ankunft beim Client minus "Encoder
//     fertig" beim Host (Stempel, Nachricht 5). Der Versatz der beiden Uhren
//     faellt dabei heraus, ein Zeitabgleich ist nicht noetig.
//   - Arbeit: wie lange der Empfangsfaden an einem Bild rechnet (Decodieren,
//     auf dem Weg ueber den Prozessor auch das Umrechnen). Daraus die
//     Decodierrate dieses Rechners und die Auslastung des Fadens.
//   - Lieferung: wie viele Bilder je Sekunde der Host schickt (Stempel,
//     Hostuhr).
// Waechst der Rueckstand (ueber STAU_AB in STAU_FOLGE Bildern in Folge) und
// ist der Client der Engpass - sein Faden ausgelastet oder die Lieferung
// ueber seiner Decodierrate; staut dagegen die Leitung, hilft ein Vollbild
// nicht, das regelt der Host -, dann
//   1. holt er auf: kein Bild bis zum naechsten Vollbild geht in den
//      Decoder, und der Host wird um eines gebeten. Es erscheint nie ein
//      Bild ohne seinen Bezug. Weiter geht es erst mit einem Vollbild, das
//      hoechstens AUFHOLEN_ENDE Rueckstand hat (ein altes aus dem Stau wird
//      uebergangen); bleibt das angeforderte aus, wird nach
//      ANFORDERN_NOCHMAL erneut gebeten, nach AUFHOLEN_HOECHSTENS gilt jedes.
//   2. wuenscht er sich eine Bildrate, die dieser Rechner schafft: 90 % der
//      Decodierrate, auf STUFE_FPS gerundet (tragbare_fps), mindestens eine
//      Stufe unter der bisherigen, nie unter MINDEST_FPS und nie ueber dem
//      Wunsch des Nutzers. Liegt die Lieferung gar nicht ueber der
//      Decodierrate (ein kurzer Stoss, eine Stockung), wird nur aufgeholt;
//      erst wiederholtes Aufholen (WIEDERHOLT_ANZAHL in WIEDERHOLT_FENSTER)
//      kostet eine Stufe.
// Beides geht ueber Nachricht 64 (Einstellungen: Mbit, fps, Spielmodus) -
// beide Hosts erzwingen auf jede Einstellung hin ein Vollbild, auch die
// aelteren; neu im Protokoll ist nichts. Nach einer ruhigen Zeit (STABIL,
// ohne Stau und ohne Aenderung) geht es wieder aufwaerts, wenn die Messung
// Luft zeigt: der naechste Schritt darf hoechstens ANTEIL der Decodierrate
// verlangen. Haelt ein Schritt keine PROBE lang, geht es
// zurueck, und bis zum naechsten Versuch vergeht doppelt so viel Zeit
// (hoechstens STABIL_MAX). Ein neuer Decoder (anderer Codec, andere Groesse)
// decodiert anders schnell: gemessen wird neu, und die Grenze faellt,
// sobald er sein erstes Bild geliefert hat.
//
// Alles hier ist reine Entscheidung ohne Uhr und ohne Leitung - der
// Empfangsfaden reicht Zeiten und Ereignisse herein und fuehrt die
// Auftraege aus (main.rs, run_session). Zeiten in Mikrosekunden: `jetzt`
// auf der Uhr des Clients, `t_enc` auf der des Hosts.

use std::collections::VecDeque;

/// Darunter wuenscht sich der Regler nie etwas.
pub const MINDEST_FPS: u16 = 20;
/// Raster der gewuenschten Bildraten.
pub const STUFE_FPS: u16 = 5;
/// Anteil der Decodierrate, den eine neue Grenze verlangt.
const ANTEIL: f32 = 0.9;
/// Mehr darf eine Grenze nie verlangen (gerundet, tragbare_fps); ab hier
/// liefert der Host mehr, als dieser Rechner schafft.
const ANTEIL_MAX: f32 = 0.95;
/// Rueckstand, ab dem ein Bild als verspaetet zaehlt ...
const STAU_AB_US: i64 = 30_000;
/// ... und so viele verspaetete Bilder in Folge sind ein Stau.
const STAU_FOLGE: u32 = 3;
/// Ein Vollbild mit hoechstens so viel Rueckstand beendet das Aufholen.
const AUFHOLEN_ENDE_US: i64 = 30_000;
/// Danach beendet jedes Vollbild das Aufholen.
const AUFHOLEN_HOECHSTENS_US: u64 = 1_500_000;
/// Bleibt das angeforderte Vollbild so lange aus, wird noch einmal gebeten.
const ANFORDERN_NOCHMAL_US: u64 = 500_000;
/// Ab dieser Auslastung des Empfangsfadens ist der Client der Engpass.
const ENGPASS_AUSLASTUNG: f32 = 0.85;
/// Fenster fuer die Auslastung.
const AUSLASTUNG_FENSTER_US: u64 = 500_000;
/// Ein angebrochenes Fenster zaehlt ab dieser Laenge mit.
const AUSLASTUNG_TEIL_US: u64 = 200_000;
/// Fenster fuer die Lieferung des Hosts (Hostuhr).
const LIEFER_FENSTER_US: u64 = 1_000_000;
/// Die Lieferung gilt erst mit so vielen Bildern ...
const LIEFER_BILDER: usize = 6;
/// ... ueber mindestens diese Spanne.
const LIEFER_SPANNE_US: u64 = 50_000;
/// Grundlinie des Rueckstands: das schnellste Bild der letzten so vielen
/// Sekunden (je Sekunde ein Minimum).
const GRUND_SEKUNDEN: usize = 15;
/// Erst mit so vielen decodierten Bildern (ohne Vollbilder) gilt die Arbeit.
const MIN_BILDER: usize = 5;
/// Die Arbeit je Bild ist das Mittel der letzten so vielen Bilder ...
const ARBEIT_BILDER: usize = 40;
/// ... ohne die langsamsten ARBEIT_OHNE davon: ein einzelnes Bild, an dem
/// der Faden haengt (das System war beschaeftigt), sagt nichts ueber die
/// Decodierrate.
const ARBEIT_OHNE: usize = 4;
/// Nur aufholen (ohne neue Grenze) hoechstens so oft ...
const AUFHOLEN_ABSTAND_US: u64 = 500_000;
/// ... und ohne Messung gar nicht - ausser der Rueckstand ist so gross.
const NOTFALL_US: i64 = 500_000;
/// Nach einer Aenderung der Grenze wird so lange nur aufgeholt, keine neue
/// Grenze gesetzt und kein Schritt aufwaerts verworfen (ausser im Notfall):
/// der Host stellt erst um - der Windows-Host oeffnet fuer eine neue
/// Bildrate seinen Encoder neu (gemessen bis 0,6 s ohne Bild), danach kommt
/// ein Vollbild, das laenger decodiert. Ein Stau in dieser Zeit sagt nichts
/// ueber die neue Rate.
const RUHE_US: u64 = 1_200_000;
/// So oft Aufholen ohne Lieferung ueber der Decodierrate ...
const WIEDERHOLT_ANZAHL: usize = 3;
/// ... in dieser Zeit kostet eine Stufe.
const WIEDERHOLT_FENSTER_US: u64 = 10_000_000;
/// So lange ohne Stau und ohne Aenderung, dann darf es aufwaerts gehen.
const STABIL_US: u64 = 10_000_000;
/// Hoechste Wartezeit nach wiederholt gescheiterten Schritten aufwaerts.
const STABIL_MAX_US: u64 = 160_000_000;
/// So lange muss ein Schritt aufwaerts ohne Stau halten.
const PROBE_US: u64 = 10_000_000;
/// Hoechstens so oft entscheidet `takt`.
const TAKT_US: u64 = 100_000;
/// Ein neuer Decoder: die Grenze faellt fruehestens so lange nach dem Bau
/// (und erst mit seinem ersten Bild).
const NEUER_DECODER_US: u64 = 500_000;

/// Einstellungen wie in Nachricht 3 und 64: Mbit/s, Bilder je Sekunde,
/// Spielmodus, feste Bildrate, Ton.
pub type Einstellung = (u32, u16, bool, bool, bool);

/// Die Bildrate, die ein Rechner mit dieser Decodierrate auf Dauer schafft:
/// ANTEIL davon, auf STUFE_FPS gerundet - aber nie ueber ANTEIL_MAX (dann
/// eine Stufe tiefer). 54,6 Bilder/s (18,3 ms je Bild) ergeben 50.
pub fn tragbare_fps(kapazitaet: f32) -> u16 {
    if !kapazitaet.is_finite() || kapazitaet <= 0.0 {
        return 0;
    }
    let stufe = STUFE_FPS as f32;
    let mut f = ((kapazitaet * ANTEIL / stufe).round() * stufe) as i32;
    while f > 0 && f as f32 > kapazitaet * ANTEIL_MAX {
        f -= STUFE_FPS as i32;
    }
    f.clamp(0, u16::MAX as i32) as u16
}

/// Was an Bildrate beim Host gilt: der Wunsch, hoechstens die Grenze.
pub fn wirksame_fps(wunsch: u16, grenze: Option<u16>) -> u16 {
    grenze.map_or(wunsch, |g| wunsch.min(g))
}

/// Was der Host meldet (Nachricht 3), gedeutet fuer Oberflaeche, Benchmark
/// und gespeicherte Werte: steht er auf genau dem, was der Regler statt des
/// Wunsches erbeten hat, gilt der Wunsch des Nutzers als seine Einstellung,
/// und die angepasste Bildrate kommt daneben (Some). Sonst die Meldung, wie
/// sie ist. Der Ton zaehlt dabei nicht mit - ein Host vor dem neunten Byte
/// meldet immer "an".
pub fn gemeldet_deuten(gemeldet: Einstellung, wunsch: Option<Einstellung>, grenze: Option<u16>) -> (Einstellung, Option<u16>) {
    if let Some(w) = wunsch {
        let wirksam = wirksame_fps(w.1, grenze);
        if wirksam < w.1 && (gemeldet.0, gemeldet.1, gemeldet.2, gemeldet.3) == (w.0, wirksam, w.2, w.3) {
            return ((w.0, w.1, w.2, w.3, gemeldet.4), Some(wirksam));
        }
    }
    (gemeldet, None)
}

/// Warum ein Auftrag kommt - fuer das Protokoll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anlass {
    /// Der Rueckstand wuchs, und die Lieferung lag ueber der Decodierrate
    /// (oder es wurde wiederholt aufgeholt): neue, niedrigere Grenze.
    Ueberlast,
    /// Nur Aufholen, die Grenze bleibt: Vollbild angefordert.
    Aufholen,
    /// Beim Aufholen blieb das Vollbild aus: noch einmal angefordert.
    Nachfordern,
    /// Nach einer ruhigen Zeit eine Stufe hoeher (oder ganz frei).
    Aufwaerts,
    /// Ein Schritt aufwaerts hielt nicht: zurueck auf die Grenze davor.
    Rueckfall,
    /// Ein neuer Decoder hat sein erstes Bild geliefert: die Grenze faellt.
    NeuerDecoder,
}

/// Was der Empfangsfaden tun soll: Nachricht 64 mit dieser Grenze neu an
/// den Host - das erzwingt dort auch ein Vollbild. Dazu die Messwerte fuer
/// das Protokoll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Auftrag {
    /// Grenze ab jetzt (None: keine, es gilt der Wunsch des Nutzers).
    pub grenze: Option<u16>,
    /// Die Grenze bis eben.
    pub vorher: Option<u16>,
    pub anlass: Anlass,
    /// Rueckstand beim Entschluss, in ms.
    pub stau_ms: f32,
    /// Decodierrate (Bilder/s), Arbeit je Bild (ms), Lieferung des Hosts
    /// (Bilder/s) - soweit schon gemessen.
    pub kapazitaet: Option<f32>,
    pub arbeit_ms: Option<f32>,
    pub lieferung: Option<f32>,
    /// Nach einem Rueckfall: so lange bis zum naechsten Schritt aufwaerts (s).
    pub naechster_versuch_s: f32,
}

impl Auftrag {
    /// Aendert der Auftrag die Grenze? Dann gehoert er einmal ins Protokoll.
    pub fn aendert(&self) -> bool {
        self.grenze != self.vorher
    }

    /// Eine Protokollzeile (deutsch, mit den Messwerten); None fuer reines
    /// Aufholen - das meldet der Empfangsfaden mit dem Aufholbericht.
    pub fn zeile(&self, wunsch: Option<u16>) -> Option<String> {
        if !self.aendert() {
            return None;
        }
        let mut mess = Vec::new();
        if let (Some(a), Some(k)) = (self.arbeit_ms, self.kapazitaet) {
            mess.push(format!("Decoder {a:.1} ms je Bild = {k:.1} Bilder/s"));
        }
        if let Some(l) = self.lieferung {
            mess.push(format!("Host lieferte {l:.1} Bilder/s"));
        }
        mess.push(format!("Rueckstand {:.0} ms", self.stau_ms.max(0.0)));
        let mess = mess.join(", ");
        let frei = match wunsch {
            Some(w) => format!("wieder wie gewuenscht ({w})"),
            None => "wieder wie gewuenscht".into(),
        };
        Some(match (self.anlass, self.grenze) {
            (Anlass::NeuerDecoder, _) => format!("Bildrate {frei}: neuer Decoder, es wird neu gemessen"),
            (Anlass::Aufwaerts, Some(g)) => format!("Bildrate an diesen Rechner angepasst: {g} (aufwaerts nach ruhiger Zeit; {mess})"),
            (Anlass::Aufwaerts, None) => format!("Bildrate {frei} (aufwaerts nach ruhiger Zeit; {mess})"),
            (Anlass::Rueckfall, Some(g)) => format!(
                "Bildrate an diesen Rechner angepasst: {g} (der Schritt aufwaerts hielt nicht, naechster Versuch fruehestens in {:.0} s; {mess})",
                self.naechster_versuch_s
            ),
            (_, Some(g)) => format!("Bildrate an diesen Rechner angepasst: {g} ({mess})"),
            (_, None) => format!("Bildrate {frei} ({mess})"),
        })
    }
}

/// Was mit einem angekommenen Bild geschieht.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bildwahl {
    Decodieren,
    /// Es wird aufgeholt: nicht decodieren.
    Verwerfen,
    /// Das Vollbild, mit dem das Aufholen endet: decodieren.
    Aufgeholt(Aufholbericht),
}

/// Ein beendetes Aufholen, fuer das Protokoll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aufholbericht {
    /// Bilder, die nicht decodiert wurden.
    pub verworfen: u32,
    /// Vom Entschluss bis zum Vollbild.
    pub dauer_ms: f32,
    /// Rueckstand beim Entschluss und beim Vollbild.
    pub stau_vorher_ms: f32,
    pub stau_jetzt_ms: f32,
    /// Das Vollbild kam erst nach AUFHOLEN_HOECHSTENS (mit Rueckstand).
    pub frist: bool,
}

struct Aufholen {
    seit: u64,
    angefordert: u64,
    verworfen: u32,
    stau_vorher: i64,
}

pub struct Regler {
    /// Arbeit der letzten ARBEIT_BILDER Bilder (us, ohne Vollbilder).
    arbeit: VecDeque<u32>,
    /// Auslastung: laufendes Fenster und das letzte volle.
    fenster_beginn: u64,
    fenster_arbeit: u64,
    auslastung: Option<f32>,
    /// "Encoder fertig" (Hostuhr) der Bilder im Lieferfenster.
    lieferung: VecDeque<u64>,
    /// Je Sekunde (Clientuhr) die kleinste Laufzeit Ankunft - Encoder.
    grund: VecDeque<(u64, i64)>,
    stau_us: i64,
    ueber_folge: u32,
    aufholen: Option<Aufholen>,
    /// Wann aufgeholt wurde, ohne dass die Grenze sank.
    aufholen_zeiten: VecDeque<u64>,
    /// Beginn des letzten Aufholens (0 = noch keins).
    letztes_aufholen: u64,
    grenze: Option<u16>,
    /// Was der Nutzer will (None: noch keine Meldung des Hosts).
    wunsch: Option<u16>,
    /// Letzte Aenderung der Grenze und letzter Stau (Clientuhr).
    geaendert: u64,
    ruhig_seit: u64,
    /// Wartezeit bis zum naechsten Schritt aufwaerts (verdoppelt sich nach
    /// jedem Rueckfall).
    stabil_us: u64,
    /// Ein Schritt aufwaerts wird erprobt: die Grenze davor, und seit wann.
    probe: Option<(Option<u16>, u64)>,
    auftrag: Option<Auftrag>,
    letzter_takt: u64,
    /// Ein neuer Decoder wurde gebaut: wann.
    neuer_decoder: Option<u64>,
}

impl Default for Regler {
    fn default() -> Self {
        Self::neu()
    }
}

impl Regler {
    pub fn neu() -> Regler {
        Regler {
            arbeit: VecDeque::new(),
            fenster_beginn: 0,
            fenster_arbeit: 0,
            auslastung: None,
            lieferung: VecDeque::new(),
            grund: VecDeque::new(),
            stau_us: 0,
            ueber_folge: 0,
            aufholen: None,
            aufholen_zeiten: VecDeque::new(),
            letztes_aufholen: 0,
            grenze: None,
            wunsch: None,
            geaendert: 0,
            ruhig_seit: 0,
            stabil_us: STABIL_US,
            probe: None,
            auftrag: None,
            letzter_takt: 0,
            neuer_decoder: None,
        }
    }

    /// Die Grenze, die der Regler gerade verlangt.
    #[cfg(test)]
    pub fn grenze(&self) -> Option<u16> {
        self.grenze
    }

    /// Der Wunsch des Nutzers (Bilder je Sekunde), sobald bekannt: nie mehr
    /// als das, und nur darunter wirkt eine Grenze.
    pub fn wunsch_setzen(&mut self, fps: Option<u16>) {
        self.wunsch = fps;
    }

    /// Rueckstand des letzten Bildes mit Stempel, in ms.
    pub fn stau_ms(&self) -> f32 {
        self.stau_us as f32 / 1000.0
    }

    /// Wird gerade aufgeholt?
    #[cfg(test)]
    pub fn holt_auf(&self) -> bool {
        self.aufholen.is_some()
    }

    /// Arbeit je Bild in us: das Mittel der letzten ARBEIT_BILDER ohne die
    /// ARBEIT_OHNE langsamsten, sobald MIN_BILDER da sind.
    fn arbeit_us(&self) -> Option<f32> {
        if self.arbeit.len() < MIN_BILDER {
            return None;
        }
        let mut a: Vec<u32> = self.arbeit.iter().copied().collect();
        a.sort_unstable();
        let ohne = if a.len() >= ARBEIT_BILDER { ARBEIT_OHNE } else { a.len() / 10 };
        let a = &a[..a.len() - ohne];
        Some(a.iter().map(|&x| x as f32).sum::<f32>() / a.len() as f32)
    }

    /// Decodierrate dieses Rechners (Bilder/s), sobald genug gemessen ist.
    pub fn kapazitaet(&self) -> Option<f32> {
        self.arbeit_us().filter(|&a| a > 0.0).map(|a| 1e6 / a)
    }

    /// Bilder je Sekunde, die der Host gerade schickt.
    pub fn lieferrate(&self) -> Option<f32> {
        let n = self.lieferung.len();
        let (a, b) = (*self.lieferung.front()?, *self.lieferung.back()?);
        let spanne = b.saturating_sub(a);
        (n >= LIEFER_BILDER && spanne >= LIEFER_SPANNE_US).then(|| (n - 1) as f32 * 1e6 / spanne as f32)
    }

    /// Ein neuer Decoder (anderer Codec, andere Groesse): alle Messungen neu.
    /// Die Grenze faellt, sobald er sein erstes Bild geliefert hat und
    /// NEUER_DECODER vergangen ist (`takt`) - er kann ganz anders schnell sein.
    pub fn neuer_decoder(&mut self, jetzt: u64) {
        self.arbeit.clear();
        self.fenster_beginn = 0;
        self.fenster_arbeit = 0;
        self.auslastung = None;
        self.lieferung.clear();
        self.grund.clear();
        self.stau_us = 0;
        self.ueber_folge = 0;
        // Der neue Decoder wartet ohnehin auf sein erstes Vollbild.
        self.aufholen = None;
        self.aufholen_zeiten.clear();
        self.probe = None;
        self.stabil_us = STABIL_US;
        self.auftrag = None;
        self.neuer_decoder = Some(jetzt);
    }

    fn wirksam(&self) -> Option<u16> {
        match (self.wunsch, self.grenze) {
            (Some(w), g) => Some(wirksame_fps(w, g)),
            (None, g) => g,
        }
    }

    fn auftrag(&self, anlass: Anlass, vorher: Option<u16>) -> Auftrag {
        Auftrag {
            grenze: self.grenze,
            vorher,
            anlass,
            stau_ms: self.stau_ms(),
            kapazitaet: self.kapazitaet(),
            arbeit_ms: self.arbeit_us().map(|a| a / 1000.0),
            lieferung: self.lieferrate(),
            naechster_versuch_s: self.stabil_us as f32 / 1e6,
        }
    }

    /// Ist der Client der Engpass? Sein Faden ist ausgelastet (letztes
    /// volles Fenster oder das laufende, wenn es lang genug ist), oder der
    /// Host liefert mehr, als er decodiert.
    fn engpass(&self, jetzt: u64) -> bool {
        let voll = self.auslastung.is_some_and(|a| a >= ENGPASS_AUSLASTUNG);
        let dauer = jetzt.saturating_sub(self.fenster_beginn);
        let teil = self.fenster_beginn > 0
            && dauer >= AUSLASTUNG_TEIL_US
            && self.fenster_arbeit as f32 >= ENGPASS_AUSLASTUNG * dauer as f32;
        let mehr = matches!((self.lieferrate(), self.kapazitaet()), (Some(l), Some(k)) if l >= ANTEIL_MAX * k);
        voll || teil || mehr
    }

    /// Ein Bild ist angekommen (vor dem Decodieren): `jetzt` Ankunft auf der
    /// Clientuhr, `t_enc` "Encoder fertig" aus seinem Stempel (Hostuhr; None
    /// ohne Stempel), `vollbild` Flagge Bit 0. Sagt, ob es in den Decoder
    /// geht.
    pub fn bild_da(&mut self, jetzt: u64, t_enc: Option<u64>, vollbild: bool) -> Bildwahl {
        if let Some(t) = t_enc {
            self.lieferung.push_back(t);
            while self.lieferung.front().is_some_and(|&f| f + LIEFER_FENSTER_US < t) {
                self.lieferung.pop_front();
            }
            let laufzeit = jetzt as i64 - t as i64;
            let sekunde = jetzt / 1_000_000;
            match self.grund.back_mut() {
                Some((s, m)) if *s == sekunde => *m = (*m).min(laufzeit),
                _ => {
                    self.grund.push_back((sekunde, laufzeit));
                    while self.grund.len() > GRUND_SEKUNDEN {
                        self.grund.pop_front();
                    }
                }
            }
            let basis = self.grund.iter().map(|(_, m)| *m).min().unwrap_or(laufzeit);
            self.stau_us = laufzeit - basis;
            if self.stau_us > STAU_AB_US {
                self.ueber_folge += 1;
            } else {
                self.ueber_folge = 0;
            }
        }
        if let Some(a) = self.aufholen.as_mut() {
            let frist = jetzt.saturating_sub(a.seit) >= AUFHOLEN_HOECHSTENS_US;
            if vollbild && (self.stau_us <= AUFHOLEN_ENDE_US || frist) {
                let bericht = Aufholbericht {
                    verworfen: a.verworfen,
                    dauer_ms: jetzt.saturating_sub(a.seit) as f32 / 1000.0,
                    stau_vorher_ms: a.stau_vorher as f32 / 1000.0,
                    stau_jetzt_ms: self.stau_ms(),
                    frist: self.stau_us > AUFHOLEN_ENDE_US,
                };
                self.aufholen = None;
                self.ueber_folge = 0;
                return Bildwahl::Aufgeholt(bericht);
            }
            a.verworfen += 1;
            if jetzt.saturating_sub(a.angefordert) >= ANFORDERN_NOCHMAL_US {
                a.angefordert = jetzt;
                self.auftrag = Some(self.auftrag(Anlass::Nachfordern, self.grenze));
            }
            return Bildwahl::Verwerfen;
        }
        if self.ueber_folge >= STAU_FOLGE && self.engpass(jetzt) && self.ueberlast(jetzt) {
            // Dieses Bild ist selbst schon zu spaet.
            if let Some(a) = self.aufholen.as_mut() {
                a.verworfen += 1;
            }
            return Bildwahl::Verwerfen;
        }
        Bildwahl::Decodieren
    }

    /// Der Client ist im Stau: aufholen, und wenn die Messung es sagt (oder
    /// es schon wiederholt noetig war), die Grenze senken. Ohne Messung
    /// (Decodierrate, Lieferung) und oefter als alle AUFHOLEN_ABSTAND nur im
    /// Notfall - sonst holte ein Rechner, der auch an der Untergrenze nicht
    /// mitkommt, in einem fort auf. false: diesmal nichts.
    fn ueberlast(&mut self, jetzt: u64) -> bool {
        // Kurz nach einer Aenderung (RUHE) wird nur aufgeholt: ein Stau aus
        // dem Umstellen des Hosts sagt nichts ueber die neue Rate.
        let ruhe_vorbei = jetzt.saturating_sub(self.geaendert) >= RUHE_US || self.stau_us >= NOTFALL_US;
        let (kap, lief) = (self.kapazitaet(), self.lieferrate());
        let vorher = self.grenze;
        let mut anlass = Anlass::Aufholen;
        if let Some((davor, seit)) = self.probe.filter(|_| ruhe_vorbei) {
            self.probe = None;
            if jetzt.saturating_sub(seit) < PROBE_US {
                // Der Schritt aufwaerts hielt nicht: zurueck, und laenger
                // warten bis zum naechsten.
                self.stabil_us = (self.stabil_us * 2).min(STABIL_MAX_US);
                self.grenze = davor;
                anlass = Anlass::Rueckfall;
            }
        }
        let wirksam = self.wirksam().or_else(|| lief.map(|l| (l.round() as u16).max(MINDEST_FPS)));
        let eine_tiefer = wirksam.filter(|&w| w > MINDEST_FPS).map(|w| w.saturating_sub(STUFE_FPS).max(MINDEST_FPS));
        if let (Anlass::Aufholen, true, Some(k), Some(l), Some(tiefer)) = (anlass, ruhe_vorbei, kap, lief, eine_tiefer) {
            // Die Lieferung liegt ueber dem, was dieser Rechner schafft.
            if l >= ANTEIL_MAX * k {
                self.grenze = Some(tragbare_fps(k).max(MINDEST_FPS).min(tiefer));
                anlass = Anlass::Ueberlast;
            }
        }
        if anlass == Anlass::Aufholen {
            let messung = kap.is_some() && lief.is_some();
            let zu_frueh = self.letztes_aufholen > 0 && jetzt.saturating_sub(self.letztes_aufholen) < AUFHOLEN_ABSTAND_US;
            if self.stau_us < NOTFALL_US && (!messung || zu_frueh) {
                return false;
            }
            if messung && ruhe_vorbei {
                while self.aufholen_zeiten.front().is_some_and(|&t| t + WIEDERHOLT_FENSTER_US < jetzt) {
                    self.aufholen_zeiten.pop_front();
                }
                self.aufholen_zeiten.push_back(jetzt);
                if let Some(tiefer) = eine_tiefer.filter(|_| self.aufholen_zeiten.len() >= WIEDERHOLT_ANZAHL) {
                    self.grenze = Some(tiefer);
                    anlass = Anlass::Ueberlast;
                }
            }
        }
        if self.grenze != vorher {
            self.geaendert = jetzt;
            self.aufholen_zeiten.clear();
        }
        self.ruhig_seit = jetzt;
        self.letztes_aufholen = jetzt;
        self.aufholen = Some(Aufholen { seit: jetzt, angefordert: jetzt, verworfen: 0, stau_vorher: self.stau_us });
        self.ueber_folge = 0;
        self.auftrag = Some(self.auftrag(anlass, vorher));
        true
    }

    /// Ein Bild ist decodiert: so lange hat der Empfangsfaden daran
    /// gearbeitet (Decodieren und Umrechnen), fertig um `jetzt`. Vollbilder
    /// zaehlen fuer die Auslastung, nicht fuer die Arbeit je Bild - sie sind
    /// selten und teurer.
    pub fn bild_fertig(&mut self, jetzt: u64, arbeit_us: u64, vollbild: bool) {
        if self.fenster_beginn == 0 {
            self.fenster_beginn = jetzt.saturating_sub(arbeit_us).max(1);
        }
        self.fenster_arbeit += arbeit_us;
        let dauer = jetzt.saturating_sub(self.fenster_beginn);
        if dauer >= AUSLASTUNG_FENSTER_US {
            self.auslastung = Some((self.fenster_arbeit as f32 / dauer as f32).min(1.0));
            self.fenster_beginn = jetzt;
            self.fenster_arbeit = 0;
        }
        if vollbild {
            return;
        }
        self.arbeit.push_back(arbeit_us.min(u32::MAX as u64) as u32);
        while self.arbeit.len() > ARBEIT_BILDER {
            self.arbeit.pop_front();
        }
    }

    /// Regelmaessig aufrufen (je Bild; entschieden wird hoechstens alle
    /// TAKT_US): liefert, was jetzt an den Host muss - einen Auftrag aus
    /// `bild_da` (Aufholen, Ueberlast) sofort, sonst nach einer ruhigen Zeit
    /// einen Schritt aufwaerts oder nach einem neuen Decoder das Ende der
    /// Grenze.
    pub fn takt(&mut self, jetzt: u64) -> Option<Auftrag> {
        if let Some(a) = self.auftrag.take() {
            return Some(a);
        }
        if jetzt.saturating_sub(self.letzter_takt) < TAKT_US {
            return None;
        }
        self.letzter_takt = jetzt;
        if self.aufholen.is_some() {
            return None;
        }
        if let Some(t) = self.neuer_decoder {
            if self.fenster_beginn == 0 || jetzt.saturating_sub(t) < NEUER_DECODER_US {
                return None;
            }
            self.neuer_decoder = None;
            let vorher = self.grenze.take()?;
            self.geaendert = jetzt;
            self.ruhig_seit = jetzt;
            return Some(self.auftrag(Anlass::NeuerDecoder, Some(vorher)));
        }
        if self.probe.is_some_and(|(_, seit)| jetzt.saturating_sub(seit) >= PROBE_US) {
            self.probe = None;
            self.stabil_us = STABIL_US;
        }
        let g = self.grenze?;
        let wunsch = self.wunsch.unwrap_or(u16::MAX);
        // Die Grenze liegt ueber dem Wunsch: sie wirkt nicht, ein Schritt
        // aufwaerts aenderte beim Host nichts.
        if g >= wunsch
            || jetzt.saturating_sub(self.ruhig_seit) < self.stabil_us
            || jetzt.saturating_sub(self.geaendert) < self.stabil_us
        {
            return None;
        }
        // Aufwaerts nur mit Luft: der naechste Schritt darf hoechstens ANTEIL
        // der Decodierrate verlangen - knapp darunter schwankt die Messung
        // (Alienware -> M1: 18,9 ms bei 45 geliefert, bei 50 wieder Stau).
        let k = self.kapazitaet()?;
        let naechste = g.saturating_add(STUFE_FPS);
        if naechste as f32 > ANTEIL * k {
            return None;
        }
        let ziel = tragbare_fps(k).max(naechste);
        self.probe = Some((Some(g), jetzt));
        self.grenze = (ziel < wunsch).then_some(ziel);
        self.geaendert = jetzt;
        Some(self.auftrag(Anlass::Aufwaerts, Some(g)))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000;
    const S: u64 = 1_000_000;
    /// Beginn jedes Laufs auf der Clientuhr.
    const T0: u64 = 10 * S;

    /// Ein Lauf im Nachbau des Empfangsfadens.
    #[derive(Debug)]
    struct Lauf {
        /// Hoechste Verspaetung (Ankunft beim Decoder - Encoder fertig, ms)
        /// eines decodierten Bildes je Sekunde.
        verspaetung_je_s: Vec<f32>,
        /// Auftraege mit der Zeit (us ab T0).
        auftraege: Vec<(u64, Auftrag)>,
        decodiert: u64,
        verworfen: u64,
        aufgeholt: Vec<Aufholbericht>,
        endgrenze: Option<u16>,
    }

    /// Nachbau des Empfangsfadens (run_session): der Host liefert mit der
    /// Rate, die gerade gilt (`wunsch`, hoechstens die Grenze), ein Vollbild
    /// zu Beginn, alle 240 Bilder und auf jeden Auftrag hin (Nachricht 64
    /// erzwingt eines); die Leitung braucht 0,2 ms. Der Client liest das
    /// naechste Bild erst, wenn er mit dem vorigen fertig ist, und braucht
    /// `arbeit_ms(t)` je Bild (t ab T0) - alles dazwischen wartet in den
    /// Puffern. Die Hostuhr geht um `versatz` us anders. Ohne Regler
    /// (`mit_regler` false) wird jedes Bild decodiert.
    fn fahren(wunsch: u16, arbeit_ms: impl Fn(u64) -> f32, sekunden: u64, versatz: i64, mit_regler: bool) -> Lauf {
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(wunsch));
        let mut lauf = Lauf {
            verspaetung_je_s: vec![0.0; sekunden as usize],
            auftraege: Vec::new(),
            decodiert: 0,
            verworfen: 0,
            aufgeholt: Vec::new(),
            endgrenze: None,
        };
        let mut fps = wunsch;
        let mut host_naechstes = T0;
        let mut schlange: VecDeque<(u64, bool)> = VecDeque::new();
        let mut client_frei = T0;
        let mut nr = 0u64;
        let mut vollbild_faellig = true;
        let ende = T0 + sekunden * S;
        loop {
            while host_naechstes <= client_frei {
                schlange.push_back((host_naechstes, vollbild_faellig || nr % 240 == 0));
                vollbild_faellig = false;
                nr += 1;
                host_naechstes += S / fps as u64;
            }
            let Some((t_enc, key)) = schlange.pop_front() else {
                client_frei = host_naechstes;
                continue;
            };
            let jetzt = client_frei.max(t_enc) + 200;
            if jetzt >= ende {
                break;
            }
            let wahl = if mit_regler {
                r.bild_da(jetzt, Some((t_enc as i64 + versatz) as u64), key)
            } else {
                Bildwahl::Decodieren
            };
            let mut arbeit = 0;
            match wahl {
                Bildwahl::Verwerfen => lauf.verworfen += 1,
                w => {
                    if let Bildwahl::Aufgeholt(b) = w {
                        lauf.aufgeholt.push(b);
                    }
                    let s = ((jetzt - T0) / S) as usize;
                    lauf.verspaetung_je_s[s] = lauf.verspaetung_je_s[s].max((jetzt - t_enc) as f32 / 1000.0);
                    arbeit = (arbeit_ms(jetzt - T0) * 1000.0) as u64;
                    if mit_regler {
                        r.bild_fertig(jetzt + arbeit, arbeit, key);
                    }
                    lauf.decodiert += 1;
                }
            }
            client_frei = jetzt + arbeit;
            if !mit_regler {
                continue;
            }
            if let Some(a) = r.takt(client_frei) {
                lauf.auftraege.push((client_frei - T0, a));
                fps = wirksame_fps(wunsch, a.grenze);
                vollbild_faellig = true;
                host_naechstes = host_naechstes.max(client_frei + MS);
            }
        }
        lauf.endgrenze = r.grenze();
        lauf
    }

    fn aenderungen(lauf: &Lauf) -> Vec<(Anlass, Option<u16>)> {
        lauf.auftraege.iter().filter(|(_, a)| a.aendert()).map(|(_, a)| (a.anlass, a.grenze)).collect()
    }

    #[test]
    fn tragbare_fps_rundet_auf_stufen() {
        // 18,3 ms je Bild (M1, 4K 4:4:4): 54,6/s -> 50, wie im Auftrag.
        assert_eq!(tragbare_fps(1000.0 / 18.3), 50);
        // 4:2:0 mit 11-12 ms: 83-90/s -> 75 bzw. 80.
        assert_eq!(tragbare_fps(83.0), 75);
        assert_eq!(tragbare_fps(90.0), 80);
        // Gerundet laege es ueber 95 %: eine Stufe tiefer.
        assert_eq!(tragbare_fps(52.0), 45);
        assert_eq!(tragbare_fps(0.0), 0);
        assert_eq!(tragbare_fps(f32::NAN), 0);
        assert_eq!(wirksame_fps(120, Some(50)), 50);
        assert_eq!(wirksame_fps(40, Some(50)), 40);
        assert_eq!(wirksame_fps(60, None), 60);
    }

    #[test]
    fn meldung_des_hosts_wird_zum_wunsch() {
        let wunsch = (50, 120, false, true, true);
        // Der Host steht auf dem, was der Regler erbeten hat.
        assert_eq!(gemeldet_deuten((50, 50, false, true, true), Some(wunsch), Some(50)), (wunsch, Some(50)));
        // Ein Host vor dem neunten Byte meldet immer Ton an.
        assert_eq!(
            gemeldet_deuten((50, 50, false, true, true), Some((50, 120, false, true, false)), Some(50)),
            (wunsch, Some(50))
        );
        // Etwas anderes (spaete Antwort, anderer Stand): wie gemeldet.
        assert_eq!(gemeldet_deuten((50, 120, false, true, true), Some(wunsch), Some(50)), ((50, 120, false, true, true), None));
        assert_eq!(gemeldet_deuten((25, 50, false, true, true), Some(wunsch), Some(50)), ((25, 50, false, true, true), None));
        // Die Grenze liegt ueber dem Wunsch, oder es gibt keine: nichts angepasst.
        let vierzig = (50, 40, false, true, true);
        assert_eq!(gemeldet_deuten(vierzig, Some(vierzig), Some(50)), (vierzig, None));
        assert_eq!(gemeldet_deuten((50, 120, false, true, true), Some(wunsch), None), (wunsch, None));
        assert_eq!(gemeldet_deuten((50, 50, false, true, true), None, Some(50)), ((50, 50, false, true, true), None));
    }

    /// Die Messung vom 29.09.2026 nachgestellt: der Host liefert 120, der
    /// Client decodiert in 18,3 ms. Ohne Regler waechst die Verspaetung
    /// ohne Ende (nach 10 s ueber 5 s); mit ihm holt er auf, wuenscht sich
    /// 50 und bleibt ab der ersten Sekunde unter 100 ms - ohne Zeitabgleich
    /// (die Uhren liegen hier 7000 s auseinander).
    #[test]
    fn m1_an_4k_444_mit_120_bleibt_unter_100_ms() {
        let ohne = fahren(120, |_| 18.3, 10, 7_000_000_000, false);
        println!("ohne Regler: {:?}", ohne.verspaetung_je_s);
        assert!(ohne.verspaetung_je_s[9] > 5000.0, "{:?}", ohne.verspaetung_je_s);

        let lauf = fahren(120, |_| 18.3, 30, 7_000_000_000, true);
        println!("mit Regler: {:?}", lauf.verspaetung_je_s);
        println!("Auftraege: {:?}", lauf.auftraege);
        assert_eq!(aenderungen(&lauf), vec![(Anlass::Ueberlast, Some(50))]);
        assert_eq!(lauf.endgrenze, Some(50));
        let (t, erste) = lauf.auftraege.iter().find(|(_, a)| a.aendert()).unwrap();
        assert!(*t < 500 * MS, "erst nach {t} us");
        let zeile = erste.zeile(Some(120)).unwrap();
        assert!(zeile.starts_with("Bildrate an diesen Rechner angepasst: 50 (Decoder 18.3 ms je Bild = 54.6 Bilder/s, Host lieferte "), "{zeile}");
        assert!(!lauf.aufgeholt.is_empty() && lauf.aufgeholt.iter().all(|b| !b.frist), "{:?}", lauf.aufgeholt);
        for (s, v) in lauf.verspaetung_je_s.iter().enumerate().skip(1) {
            assert!(*v < 100.0, "Sekunde {s}: {v} ms");
        }
        // Danach 50 Bilder je Sekunde, alle decodiert.
        assert!(lauf.decodiert > 50 * 28, "{}", lauf.decodiert);
    }

    /// Der Host liefert 60, der Client schafft 55: ebenfalls 50, und der
    /// Rueckstand ist bald weg.
    #[test]
    fn knapp_zu_langsam_bei_60() {
        let lauf = fahren(60, |_| 18.3, 20, -3_000_000, true);
        println!("{:?}\n{:?}", lauf.verspaetung_je_s, lauf.auftraege);
        assert_eq!(aenderungen(&lauf), vec![(Anlass::Ueberlast, Some(50))]);
        for (s, v) in lauf.verspaetung_je_s.iter().enumerate().skip(2) {
            assert!(*v < 100.0, "Sekunde {s}: {v} ms");
        }
    }

    /// 4:2:0 mit 11,5 ms bei 120: 80, und nicht 50.
    #[test]
    fn vier_zwei_null_bei_120() {
        let lauf = fahren(120, |_| 11.5, 20, 0, true);
        assert_eq!(aenderungen(&lauf), vec![(Anlass::Ueberlast, Some(80))]);
        for (s, v) in lauf.verspaetung_je_s.iter().enumerate().skip(1) {
            assert!(*v < 100.0, "Sekunde {s}: {v} ms");
        }
    }

    /// Schnell genug: nichts geschieht - keine Grenze, kein Aufholen.
    #[test]
    fn schnell_genug_bleibt_frei() {
        let lauf = fahren(60, |_| 11.0, 20, 0, true);
        assert!(lauf.auftraege.is_empty(), "{:?}", lauf.auftraege);
        assert_eq!(lauf.verworfen, 0);
        assert!(lauf.verspaetung_je_s.iter().all(|v| *v < 20.0), "{:?}", lauf.verspaetung_je_s);
    }

    /// Nach einer ruhigen Zeit wieder aufwaerts, sobald die Messung Luft
    /// zeigt (der Inhalt wird leichter: 12 ms je Bild) - bis zum Wunsch,
    /// dann ist die Grenze weg.
    #[test]
    fn aufwaerts_wenn_luft_da_ist() {
        let lauf = fahren(60, |t| if t < 5 * S { 18.3 } else { 12.0 }, 30, 0, true);
        println!("{:?}", lauf.auftraege);
        assert_eq!(aenderungen(&lauf), vec![(Anlass::Ueberlast, Some(50)), (Anlass::Aufwaerts, None)]);
        assert_eq!(lauf.endgrenze, None);
        let mut t = lauf.auftraege.iter().filter(|(_, a)| a.aendert()).map(|(t, _)| *t);
        let (t_ueber, t_auf) = (t.next().unwrap(), t.next().unwrap());
        assert!(t_auf - t_ueber >= STABIL_US, "{}", t_auf - t_ueber);
        let auf = lauf.auftraege.iter().find(|(_, a)| a.anlass == Anlass::Aufwaerts).unwrap().1;
        assert_eq!(
            auf.zeile(Some(60)).unwrap(),
            format!(
                "Bildrate wieder wie gewuenscht (60) (aufwaerts nach ruhiger Zeit; Decoder {:.1} ms je Bild = {:.1} Bilder/s, Host lieferte {:.1} Bilder/s, Rueckstand {:.0} ms)",
                auf.arbeit_ms.unwrap(),
                auf.kapazitaet.unwrap(),
                auf.lieferung.unwrap(),
                auf.stau_ms.max(0.0)
            )
        );
        for (s, v) in lauf.verspaetung_je_s.iter().enumerate().skip(1) {
            assert!(*v < 100.0, "Sekunde {s}: {v} ms");
        }
    }

    /// Ein Schritt aufwaerts, der nicht haelt (gleich danach wird das
    /// Decodieren wieder teurer), geht zurueck - und der naechste Versuch
    /// wartet doppelt so lange.
    #[test]
    fn rueckfall_verdoppelt_die_wartezeit() {
        let lauf = fahren(60, |t| if (5 * S..12 * S).contains(&t) { 12.0 } else { 18.3 }, 40, 0, true);
        println!("{:?}", lauf.auftraege);
        assert_eq!(
            aenderungen(&lauf),
            vec![(Anlass::Ueberlast, Some(50)), (Anlass::Aufwaerts, None), (Anlass::Rueckfall, Some(50))]
        );
        let rueck = lauf.auftraege.iter().find(|(_, a)| a.anlass == Anlass::Rueckfall).unwrap().1;
        assert_eq!(rueck.naechster_versuch_s, 20.0);
        assert!(rueck.zeile(Some(60)).unwrap().contains("naechster Versuch fruehestens in 20 s"));
        for (s, v) in lauf.verspaetung_je_s.iter().enumerate().skip(1) {
            assert!(*v < 150.0, "Sekunde {s}: {v} ms");
        }
    }

    /// Gleich nach einem Schritt aufwaerts stellt der Host um (Windows: neuer
    /// Encoder, bis 0,6 s ohne Bild, dann ein Vollbild): ein Stau in dieser
    /// Zeit (RUHE) wird nur aufgeholt, der Schritt bleibt. Erst ein Stau
    /// danach nimmt ihn zurueck.
    #[test]
    fn stau_beim_umstellen_verwirft_keinen_schritt() {
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(60));
        let t = 100 * S;
        r.arbeit.extend([18_000u32; 40]);
        r.auslastung = Some(1.0);
        for k in 0..50u64 {
            r.lieferung.push_back(t - S + k * 20 * MS);
        }
        r.grund.push_back((t / S, 0));
        // Eben aufwaerts von 45 auf 50.
        r.grenze = Some(50);
        r.geaendert = t;
        r.probe = Some((Some(45), t));
        let stau = |r: &mut Regler, t0: u64| -> Bildwahl {
            let mut w = Bildwahl::Decodieren;
            for k in 1..=3u64 {
                w = r.bild_da(t0 + k * 20 * MS, Some(t0 + k * 20 * MS - k * 40 * MS), false);
            }
            w
        };
        // 0,8 s danach: Stau - aufholen, die Grenze bleibt 50.
        assert_eq!(stau(&mut r, t + 800 * MS), Bildwahl::Verwerfen);
        let a = r.takt(t + 900 * MS).expect("Vollbild anfordern");
        assert_eq!((a.anlass, a.grenze), (Anlass::Aufholen, Some(50)));
        assert!(matches!(r.bild_da(t + 950 * MS, Some(t + 949 * MS), true), Bildwahl::Aufgeholt(_)));
        // 3 s danach wieder Stau: der Schritt hielt nicht.
        assert_eq!(stau(&mut r, t + 3 * S), Bildwahl::Verwerfen);
        let a = r.takt(t + 3 * S + 100 * MS).expect("Rueckfall");
        assert_eq!((a.anlass, a.grenze), (Anlass::Rueckfall, Some(45)));
    }

    /// Staut die Leitung (der Faden des Clients wartet meist aufs Netz),
    /// holt der Client nicht auf und senkt nichts - ein Vollbild machte es
    /// dort nur schlimmer. Das regelt der Host.
    #[test]
    fn stau_der_leitung_ist_nicht_seiner() {
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(60));
        let mut t_enc = 50 * S;
        for i in 0..300u64 {
            let verspaetung = if i < 60 { MS } else { MS + (i - 60) * 2 * MS };
            let key = i % 120 == 0;
            assert_eq!(r.bild_da(t_enc + verspaetung, Some(t_enc), key), Bildwahl::Decodieren, "Bild {i}");
            r.bild_fertig(t_enc + verspaetung + 4 * MS, 4 * MS, key);
            assert_eq!(r.takt(t_enc + verspaetung + 4 * MS), None);
            t_enc += S / 60;
        }
        assert!(r.stau_ms() > 400.0, "{}", r.stau_ms());
        assert_eq!(r.grenze(), None);
    }

    /// Beim Aufholen geht kein Zwischenbild in den Decoder; ein altes
    /// Vollbild aus dem Stau wird uebergangen, erst ein frisches beendet
    /// es. Bleibt es aus, wird nachgefordert, und nach der Frist gilt jedes.
    /// Liegt die Lieferung nicht ueber der Decodierrate, bleibt die Grenze.
    #[test]
    fn aufholen_nur_bis_zu_einem_frischen_vollbild() {
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(60));
        // 50 geliefert, 17 ms je Bild, der Faden ist ausgelastet.
        let (mut t, mut t_enc) = (20 * S, 20 * S);
        for i in 0..40u64 {
            assert_eq!(r.bild_da(t, Some(t_enc), i == 0), Bildwahl::Decodieren);
            r.bild_fertig(t + 17 * MS, 17 * MS, i == 0);
            t += 17 * MS;
            t_enc += 20 * MS;
            t = t.max(t_enc);
        }
        // Ein Stoss: drei Bilder mit wachsendem Rueckstand.
        for k in 1..=3u64 {
            t_enc += 20 * MS;
            let w = r.bild_da(t_enc + k * 40 * MS, Some(t_enc), false);
            if k < 3 {
                assert_eq!(w, Bildwahl::Decodieren);
                r.bild_fertig(t_enc + k * 40 * MS + 17 * MS, 17 * MS, false);
            } else {
                assert_eq!(w, Bildwahl::Verwerfen);
            }
        }
        let t = t_enc + 120 * MS;
        assert!(r.holt_auf());
        let a = r.takt(t).expect("Vollbild anfordern");
        assert_eq!((a.anlass, a.grenze), (Anlass::Aufholen, None), "{a:?}");
        assert!(!a.aendert() && a.zeile(Some(60)).is_none());
        // Zwischenbilder und ein altes Vollbild: verworfen.
        assert_eq!(r.bild_da(t + MS, Some(t_enc + 20 * MS), false), Bildwahl::Verwerfen);
        assert_eq!(r.bild_da(t + 2 * MS, Some(t_enc + 40 * MS), true), Bildwahl::Verwerfen);
        // 0,5 s ohne frisches Vollbild: noch einmal anfordern.
        assert_eq!(r.bild_da(t + 501 * MS, Some(t + 400 * MS), false), Bildwahl::Verwerfen);
        assert_eq!(r.takt(t + 501 * MS).map(|a| a.anlass), Some(Anlass::Nachfordern));
        // Ein frisches Vollbild beendet es.
        match r.bild_da(t + 600 * MS, Some(t + 599 * MS), true) {
            Bildwahl::Aufgeholt(b) => {
                assert_eq!(b.verworfen, 4);
                assert!(!b.frist);
                assert!(b.stau_vorher_ms > 100.0 && b.stau_jetzt_ms < 30.0, "{b:?}");
            }
            w => panic!("{w:?}"),
        }
        assert!(!r.holt_auf());
        assert_eq!(r.grenze(), None);

        // Nach der Frist tut es jedes Vollbild, auch eines mit Rueckstand.
        let mut r = Regler::neu();
        let t = 20 * S;
        r.grund.push_back((t / S, 0));
        r.aufholen = Some(Aufholen { seit: t, angefordert: t, verworfen: 0, stau_vorher: 500_000 });
        assert_eq!(r.bild_da(t + 1_400 * MS, Some(t + 1_200 * MS), true), Bildwahl::Verwerfen);
        match r.bild_da(t + 1_500 * MS, Some(t + 1_300 * MS), true) {
            Bildwahl::Aufgeholt(b) => assert!(b.frist),
            w => panic!("{w:?}"),
        }
    }

    /// Kurze Stockungen bei knapper Auslastung (30 ms je Bild bei 30
    /// geliefert, alle 2 s haengt ein Bild 200 ms): die Lieferung liegt
    /// nicht ueber der Decodierrate - nur aufholen; erst das dritte Mal in
    /// 10 s kostet eine Stufe.
    #[test]
    fn wiederholtes_aufholen_kostet_eine_stufe() {
        let lauf = fahren(30, |t| if t % (2 * S) < 20 * MS { 200.0 } else { 29.0 }, 12, 0, true);
        println!("{:?}", lauf.auftraege);
        let anlaesse: Vec<(Anlass, Option<u16>)> = lauf.auftraege.iter().map(|(_, a)| (a.anlass, a.grenze)).collect();
        assert!(anlaesse.len() >= 3, "{anlaesse:?}");
        assert_eq!(&anlaesse[..3], &[(Anlass::Aufholen, None), (Anlass::Aufholen, None), (Anlass::Ueberlast, Some(25))]);
    }

    /// Nie unter MINDEST_FPS: auch ein Rechner, der nur 16 Bilder je
    /// Sekunde schafft, bekommt 20 - und holt dann eben auf.
    #[test]
    fn nie_unter_der_untergrenze() {
        let lauf = fahren(60, |_| 60.0, 20, 0, true);
        println!("{:?}", lauf.auftraege);
        assert_eq!(aenderungen(&lauf), vec![(Anlass::Ueberlast, Some(MINDEST_FPS))]);
        assert!(lauf.auftraege.iter().all(|(_, a)| a.grenze.map_or(true, |g| g >= MINDEST_FPS)));
        assert!(lauf.aufgeholt.len() >= 2);
    }

    /// Nie ueber dem Wunsch: wer 30 will, bekommt nie mehr - und ohne Stau
    /// gibt es nichts zu regeln.
    #[test]
    fn nie_ueber_dem_wunsch() {
        let lauf = fahren(30, |_| 18.3, 20, 0, true);
        assert!(lauf.auftraege.is_empty(), "{:?}", lauf.auftraege);
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(40));
        r.grenze = Some(35);
        r.arbeit.extend([5_000u32; 40]);
        let a = r.takt(100 * S).expect("aufwaerts");
        assert_eq!((a.anlass, a.grenze), (Anlass::Aufwaerts, None));
    }

    /// Ein neuer Decoder (Codecwechsel) misst neu: die Grenze faellt mit
    /// seinem ersten Bild, fruehestens nach NEUER_DECODER.
    #[test]
    fn neuer_decoder_hebt_die_grenze_auf() {
        let mut r = Regler::neu();
        r.wunsch_setzen(Some(120));
        r.grenze = Some(50);
        let t = 40 * S;
        r.neuer_decoder(t);
        assert_eq!(r.takt(t + 600 * MS), None, "noch kein Bild");
        r.bild_da(t + 700 * MS, Some(t + 699 * MS), true);
        r.bild_fertig(t + 712 * MS, 12 * MS, true);
        let a = r.takt(t + 800 * MS).expect("Grenze faellt");
        assert_eq!((a.anlass, a.grenze, a.vorher), (Anlass::NeuerDecoder, None, Some(50)));
        assert_eq!(a.zeile(Some(120)).unwrap(), "Bildrate wieder wie gewuenscht (120): neuer Decoder, es wird neu gemessen");
        assert_eq!(r.takt(t + 2 * S), None);
        // Ohne Grenze kein Auftrag.
        let mut r = Regler::neu();
        r.neuer_decoder(t);
        r.bild_fertig(t + 10 * MS, 10 * MS, true);
        assert_eq!(r.takt(t + S), None);
    }
}
