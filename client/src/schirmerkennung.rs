// Schirmerkennung des Windows-Clients: auf welchem Bildschirm das Fenster
// steht und was Windows ueber GENAU diesen Bildschirm weiss - "HDR
// verwenden", SDR-Weiss, Spitze, Vollbild-Spitze. Daraus entstehen die
// Ausgabe der Swapchain (anzeige.rs, lage_pruefen) und IN_ANZEIGE (main.rs,
// anzeige_takt).
//
// Reines Rust ohne Windows-Aufrufe, damit die Zuordnung auf jeder Plattform
// pruefbar ist. Die drei Listen fuellt anzeige.rs (schirm_lesen):
// - GDI: der Monitor des Fensters (MonitorFromWindow - der mit dem groessten
//   Teil des Fensters) und sein Geraetename (GetMonitorInfoW, \\.\DISPLAYn).
//   Er ist der Schluessel fuer alles andere.
// - DXGI: jeder Ausgang JEDES Adapters der Factory mit HMONITOR,
//   Geraetename, Karte (AdapterLuid) und IDXGIOutput6::GetDesc1 (Farbraum,
//   MaxLuminance, MaxFullFrameLuminance). Auf Laptops mit zwei Karten haengt
//   der Bildschirm oft nicht an der Karte der Swapchain, derselbe Monitor
//   kann an zwei Adaptern auftauchen (ein Ausgang der einen Karte, durch die
//   andere gereicht), und eine Factory haelt HMONITOR und Werte von ihrem
//   Anlegen fest - nach einer Umstellung zeigt ein alter HMONITOR womoeglich
//   auf einen anderen Bildschirm.
// - DisplayConfig: die aktiven Pfade, je Pfad der Geraetename der Quelle, die
//   Karte, die sie betreibt, und am Ziel (dem Monitor) Name, SDR-Weiss und
//   Advanced Color - live, ohne Factory.
//
// Regeln (schirm_zuordnen): Ein DXGI-Ausgang gehoert zum Fenster, wenn
// HMONITOR UND Geraetename stimmen. Zeigen ihn mehrere Adapter, gilt der
// Ausgang der Karte, die laut DisplayConfig die Quelle betreibt, sonst der
// erste. Das SDR-Weiss kommt vom Pfad genau dieser Quelle (am liebsten vom
// Ziel an derselben Karte). Was fuer genau diesen Bildschirm fehlt, bleibt bei
// der Vorgabe von hdr::Schirm (0 = unbekannt, HDR aus) - nie ein Wert eines
// anderen Bildschirms. Sagen DXGI und DisplayConfig Verschiedenes ueber "HDR
// verwenden", ist die Factory vermutlich veraltet (`widerspruch`); der
// Aufrufer liest dann mit einer neuen. Wie Windows die Spitzen nennt, bleibt
// unangetastet: hier wird nur zugeordnet, nicht umgedeutet.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::hdr;

/// Der Monitor des Fensters laut GDI.
#[derive(Clone, Debug, PartialEq)]
pub struct FensterMonitor {
    /// HMONITOR aus MonitorFromWindow.
    pub monitor: isize,
    /// Geraetename aus GetMonitorInfoW (\\.\DISPLAYn).
    pub name: String,
}

/// Die Farblage eines DXGI-Ausgangs (IDXGIOutput6::GetDesc1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AusgangFarbe {
    /// ColorSpace G2084/P2020: "HDR verwenden" an.
    pub hdr: bool,
    /// MaxLuminance und MaxFullFrameLuminance, so wie Windows sie nennt.
    pub spitze_nit: f32,
    pub vollbild_spitze_nit: f32,
}

/// Ein Ausgang eines Adapters der Factory.
#[derive(Clone, Debug, PartialEq)]
pub struct DxgiAusgang {
    /// Listenplatz des Adapters (EnumAdapters1), seine Kennung (AdapterLuid,
    /// dieselbe Rechnung wie AdapterInfo::luid) und sein Name.
    pub adapter: u32,
    pub luid: i64,
    pub adapter_name: String,
    /// HMONITOR und Geraetename aus der Beschreibung des Ausgangs.
    pub monitor: isize,
    pub name: String,
    /// None: kein IDXGIOutput6 (Windows vor 10 1703) oder GetDesc1 gescheitert.
    pub farbe: Option<AusgangFarbe>,
}

/// Ein aktiver Pfad aus QueryDisplayConfig.
#[derive(Clone, Debug, PartialEq)]
pub struct AnzeigePfad {
    /// Geraetename der Quelle (\\.\DISPLAYn) und die Karte, die sie betreibt.
    pub quelle: String,
    pub quelle_luid: i64,
    /// Das Ziel, also der Monitor: Karte, Nummer, Anzeigename (leer: keiner,
    /// etwa bei manchem eingebauten Panel).
    pub ziel_luid: i64,
    pub ziel_id: u32,
    pub anzeigename: String,
    /// SDR-Weiss in nit (SDRWhiteLevel * 80 / 1000); None, wenn Windows es
    /// nicht nennt.
    pub sdr_weiss_nit: Option<f32>,
    /// "HDR verwenden" laut Advanced Color (an und nicht bloss erzwungenes
    /// WCG); None, wenn Windows es nicht nennt.
    pub hdr: Option<bool>,
}

/// Der Bildschirm des Fensters und alles, was von genau ihm stammt.
#[derive(Clone, Debug, PartialEq)]
pub struct Zuordnung {
    pub schirm: hdr::Schirm,
    /// Wer er ist: Geraetename, Anzeigename, Adapter des DXGI-Ausgangs.
    pub name: String,
    pub anzeigename: String,
    pub adapter: u32,
    pub adapter_name: String,
    /// Betreibt die Karte des DXGI-Ausgangs laut DisplayConfig die Quelle?
    /// None ohne Pfad.
    pub eigene_karte: Option<bool>,
    /// Kennung des Bildschirms: das Ziel des Pfads (Karte, Nummer), ohne
    /// Pfad der Ausgang (Karte, Geraetename). Aendert sie sich, steht das
    /// Fenster auf einem anderen Bildschirm - ein neuer HMONITOR allein
    /// heisst das nicht.
    pub kennung: String,
    /// DXGI und DisplayConfig sagen Verschiedenes ueber "HDR verwenden".
    pub widerspruch: bool,
}

/// Geraetenamen vergleicht Windows ohne Gross- und Kleinschreibung.
fn gleicher_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Der Bildschirm des Fensters aus den drei Listen (Regeln im Kopf). None,
/// wenn kein DXGI-Ausgang HMONITOR UND Geraetename des Fensters traegt - dann
/// ist die Factory veraltet oder der Bildschirm hat keinen DXGI-Ausgang
/// (etwa eine Fernsitzung); der Aufrufer liest einmal mit einer neuen
/// Factory, sonst gilt der Bildschirm als unbekannt (SDR).
pub fn schirm_zuordnen(fenster: &FensterMonitor, ausgaenge: &[DxgiAusgang], pfade: &[AnzeigePfad]) -> Option<Zuordnung> {
    let eigene: Vec<&AnzeigePfad> = pfade.iter().filter(|p| gleicher_name(&p.quelle, &fenster.name)).collect();
    let karte = eigene.first().map(|p| p.quelle_luid);
    let passend: Vec<&DxgiAusgang> =
        ausgaenge.iter().filter(|a| a.monitor == fenster.monitor && gleicher_name(&a.name, &fenster.name)).collect();
    let a = passend.iter().find(|a| Some(a.luid) == karte).or(passend.first()).copied()?;
    let pfad = eigene.iter().find(|p| p.ziel_luid == a.luid).or(eigene.first()).copied();
    let hdr = a.farbe.is_some_and(|f| f.hdr);
    let weiss = pfad.and_then(|p| p.sdr_weiss_nit).filter(|w| w.is_finite() && *w > 0.0).unwrap_or(0.0);
    let spitze = a.farbe.map_or(0.0, |f| f.spitze_nit);
    let kopfraum = if hdr && weiss > 0.0 && spitze > weiss { spitze / weiss } else { 1.0 };
    Some(Zuordnung {
        schirm: hdr::Schirm {
            hdr,
            sdr_weiss_nit: weiss,
            spitze_nit: spitze,
            vollbild_spitze_nit: a.farbe.map_or(0.0, |f| f.vollbild_spitze_nit),
            kopfraum_potentiell: kopfraum,
            kopfraum_aktuell: kopfraum,
        },
        name: fenster.name.clone(),
        anzeigename: pfad.map(|p| p.anzeigename.clone()).unwrap_or_default(),
        adapter: a.adapter,
        adapter_name: a.adapter_name.clone(),
        eigene_karte: karte.map(|k| k == a.luid),
        kennung: match pfad {
            Some(p) => format!("Ziel {} an {:x}", p.ziel_id, p.ziel_luid),
            None => format!("{} an {:x}", a.name, a.luid),
        },
        widerspruch: a.farbe.is_some() && pfad.and_then(|p| p.hdr).is_some_and(|h| h != hdr),
    })
}

/// Zaehlt der Schritt von `alt` nach `neu` fuer die Anzeige - ein anderer
/// Bildschirm (Kennung) oder "HDR verwenden" um? Dann steht er im Protokoll,
/// und HDR10 darf wieder versucht werden. Ein neuer HMONITOR fuer denselben
/// Bildschirm, ein anderer Adapter fuer denselben Ausgang oder andere Pegel
/// zaehlen nicht.
pub fn schirm_gewechselt(alt: Option<&Zuordnung>, neu: Option<&Zuordnung>) -> bool {
    alt.map(|z| (&z.kennung, z.schirm.hdr)) != neu.map(|z| (&z.kennung, z.schirm.hdr))
}

/// Ein Pegel fuers Protokoll: ganze nit, 0 = unbekannt.
fn nit(v: f32) -> String {
    if v.is_finite() && v > 0.0 {
        format!("{v:.0} nit")
    } else {
        "unbekannt".into()
    }
}

impl Zuordnung {
    /// Fuers Protokoll, eine Zeile mit allem von genau diesem Bildschirm:
    /// "Fenster auf \\.\DISPLAY2 "X27 X1" (Ausgang von Adapter 1, NVIDIA ...):
    /// HDR, SDR-Weiss 480 nit, Spitze 417 nit, Vollbild 270 nit".
    pub fn text(&self) -> String {
        let s = &self.schirm;
        format!(
            "Fenster auf {}{} (Ausgang von Adapter {}, {}{}): {}, SDR-Weiss {}, Spitze {}, Vollbild {}{}",
            self.name,
            if self.anzeigename.is_empty() { String::new() } else { format!(" \"{}\"", self.anzeigename) },
            self.adapter,
            self.adapter_name,
            if self.eigene_karte == Some(false) { "; die Quelle betreibt eine andere Karte" } else { "" },
            if s.hdr { "HDR" } else { "SDR" },
            nit(s.sdr_weiss_nit),
            nit(s.spitze_nit),
            nit(s.vollbild_spitze_nit),
            if self.widerspruch {
                format!(" - DisplayConfig meldet {}, es gilt DXGI", if s.hdr { "SDR" } else { "HDR" })
            } else {
                String::new()
            }
        )
    }
}

/// Fuers Protokoll, wenn kein DXGI-Ausgang zum Monitor des Fensters passt.
pub fn text_ohne_ausgang(fenster: &FensterMonitor) -> String {
    format!("Fenster auf {}: kein DXGI-Ausgang zeigt diesen Monitor - unbekannt (gilt als SDR)", fenster.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Der Laptop aus dem Feldtest (Alienware X17, zwei Karten): das
    // eingebaute Panel an der iGPU, der X27 an der RTX 3080 Ti, "HDR
    // verwenden" nur auf dem X27. Werte wie im Protokoll des Clients.
    const INTEL: i64 = 0x0000_0000_0000_8a01;
    const NVIDIA: i64 = 0x0000_0001_0000_3c02;
    const H_PANEL: isize = 0x0001_0001;
    const H_X27: isize = 0x0002_0003;
    const PANEL: &str = r"\\.\DISPLAY1";
    const X27: &str = r"\\.\DISPLAY2";

    fn farbe(hdr: bool, spitze: f32, vollbild: f32) -> Option<AusgangFarbe> {
        Some(AusgangFarbe { hdr, spitze_nit: spitze, vollbild_spitze_nit: vollbild })
    }

    fn ausgang(adapter: u32, monitor: isize, name: &str, farbe: Option<AusgangFarbe>) -> DxgiAusgang {
        let (luid, adapter_name) = if adapter == 0 {
            (INTEL, "Intel(R) UHD Graphics")
        } else {
            (NVIDIA, "NVIDIA GeForce RTX 3080 Ti Laptop GPU")
        };
        DxgiAusgang { adapter, luid, adapter_name: adapter_name.into(), monitor, name: name.into(), farbe }
    }

    fn fenster(monitor: isize, name: &str) -> FensterMonitor {
        FensterMonitor { monitor, name: name.into() }
    }

    /// DisplayConfig des Laptops: das Panel an der iGPU (SDR, 80 nit), der
    /// X27 an der dGPU (HDR, SDR-Schieber auf 480 nit).
    fn pfade() -> Vec<AnzeigePfad> {
        vec![
            AnzeigePfad {
                quelle: PANEL.into(),
                quelle_luid: INTEL,
                ziel_luid: INTEL,
                ziel_id: 0x1100,
                anzeigename: String::new(),
                sdr_weiss_nit: Some(80.0),
                hdr: Some(false),
            },
            AnzeigePfad {
                quelle: X27.into(),
                quelle_luid: NVIDIA,
                ziel_luid: NVIDIA,
                ziel_id: 0x1200,
                anzeigename: "X27 X1".into(),
                sdr_weiss_nit: Some(480.0),
                hdr: Some(true),
            },
        ]
    }

    /// Eine frische Factory auf dem Laptop: Adapter 0 die iGPU mit dem Panel,
    /// Adapter 1 die dGPU mit dem X27.
    fn frisch() -> Vec<DxgiAusgang> {
        vec![ausgang(0, H_PANEL, PANEL, farbe(false, 240.0, 240.0)), ausgang(1, H_X27, X27, farbe(true, 417.0, 270.0))]
    }

    fn schirm(z: &Option<Zuordnung>) -> (bool, f32, f32, f32) {
        let s = z.as_ref().expect("zugeordnet").schirm;
        (s.hdr, s.sdr_weiss_nit, s.spitze_nit, s.vollbild_spitze_nit)
    }

    /// Der X27 haengt am ZWEITEN Adapter, und der erste zeigt denselben
    /// Monitor noch einmal (derselbe HMONITOR, derselbe Name) mit Werten, die
    /// nicht die des X27 sind. Frueher gewann der erste Treffer: HDR und
    /// Spitze 240 von diesem Ausgang, dazu das SDR-Weiss 480 des X27 -
    /// genau die Zeile "HDR, SDR-Weiss 480 nit, Spitze 240 nit" aus dem
    /// Feldtest. Jetzt gilt der Ausgang der Karte, die die Quelle betreibt.
    #[test]
    fn hybrid_der_schirm_am_zweiten_adapter_gewinnt() {
        let ausgaenge = vec![
            ausgang(0, H_PANEL, PANEL, farbe(false, 240.0, 240.0)),
            ausgang(0, H_X27, X27, farbe(true, 240.0, 240.0)),
            ausgang(1, H_X27, X27, farbe(true, 417.0, 270.0)),
        ];
        let z = schirm_zuordnen(&fenster(H_X27, X27), &ausgaenge, &pfade());
        assert_eq!(schirm(&z), (true, 480.0, 417.0, 270.0));
        let z = z.unwrap();
        assert_eq!((z.adapter, z.eigene_karte, z.widerspruch), (1, Some(true), false));
        assert_eq!(z.schirm.kopfraum_potentiell, 1.0);
    }

    /// Derselbe Fall andersherum: die dGPU zaehlt zuerst und reicht das
    /// Panel der iGPU durch, ohne Farblage. Es gilt der Ausgang der iGPU.
    #[test]
    fn hybrid_durchgereichtes_panel_gilt_nicht() {
        let ausgaenge = vec![
            ausgang(1, H_PANEL, PANEL, None),
            ausgang(1, H_X27, X27, farbe(true, 417.0, 270.0)),
            ausgang(0, H_PANEL, PANEL, farbe(false, 240.0, 240.0)),
        ];
        let z = schirm_zuordnen(&fenster(H_PANEL, PANEL), &ausgaenge, &pfade());
        assert_eq!(schirm(&z), (false, 80.0, 240.0, 240.0));
        assert_eq!(z.unwrap().adapter, 0);
        let z = schirm_zuordnen(&fenster(H_X27, X27), &ausgaenge, &pfade());
        assert_eq!(schirm(&z), (true, 480.0, 417.0, 270.0));
    }

    /// Eine veraltete Factory: Windows hat die HMONITOR neu vergeben, der
    /// alte Ausgang des Panels traegt jetzt den HMONITOR des X27, der des X27
    /// einen, den es nicht mehr gibt. Frueher: die Werte des Panels (SDR,
    /// Spitze 240) mit dem SDR-Weiss des Panels - "SDR, SDR-Weiss 80 nit,
    /// Spitze 240 nit" fuer ein Fenster auf dem X27. Jetzt passt nichts (der
    /// Aufrufer liest mit einer neuen Factory), und die neue ergibt den X27.
    #[test]
    fn veralteter_hmonitor_zeigt_keinen_fremden_schirm() {
        let alt = vec![ausgang(0, H_X27, PANEL, farbe(false, 240.0, 240.0)), ausgang(1, 0x0009_0009, X27, farbe(true, 417.0, 270.0))];
        assert_eq!(schirm_zuordnen(&fenster(H_X27, X27), &alt, &pfade()), None);
        let z = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &pfade());
        assert_eq!(schirm(&z), (true, 480.0, 417.0, 270.0));
    }

    /// Beide Bildschirme einzeln, jeder mit seinen eigenen Werten.
    #[test]
    fn panel_und_x27_je_mit_eigenen_werten() {
        let panel = schirm_zuordnen(&fenster(H_PANEL, PANEL), &frisch(), &pfade());
        assert_eq!(schirm(&panel), (false, 80.0, 240.0, 240.0));
        let x27 = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &pfade());
        assert_eq!(schirm(&x27), (true, 480.0, 417.0, 270.0));
        assert!(schirm_gewechselt(panel.as_ref(), x27.as_ref()));
        assert!(schirm_gewechselt(None, x27.as_ref()));
    }

    /// Fehlt ein Wert fuer genau diesen Bildschirm, gilt die Vorgabe (0 =
    /// unbekannt, HDR aus) - nie der Wert des anderen Bildschirms, auch
    /// nicht der eines zweiten Ausgangs mit demselben HMONITOR.
    #[test]
    fn fehlende_werte_nie_vom_nachbarn() {
        let mut p = pfade();
        p[1].sdr_weiss_nit = None;
        let ausgaenge = vec![ausgang(1, H_X27, X27, None), ausgang(0, H_PANEL, PANEL, farbe(false, 240.0, 240.0)), ausgang(0, H_X27, X27, farbe(true, 240.0, 240.0))];
        let z = schirm_zuordnen(&fenster(H_X27, X27), &ausgaenge, &p).expect("zugeordnet");
        assert_eq!(z.schirm, hdr::Schirm { kopfraum_potentiell: 1.0, kopfraum_aktuell: 1.0, ..hdr::Schirm::default() });
        assert_eq!(z.adapter, 1);
        // Ohne DisplayConfig: der erste passende Ausgang, SDR-Weiss unbekannt.
        let z = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &[]).expect("zugeordnet");
        assert_eq!((z.schirm.hdr, z.schirm.sdr_weiss_nit, z.schirm.spitze_nit), (true, 0.0, 417.0));
        assert_eq!(z.eigene_karte, None);
        // Kein Ausgang mit diesem Monitor: unbekannt.
        assert_eq!(schirm_zuordnen(&fenster(0x0007_0007, r"\\.\DISPLAY7"), &frisch(), &pfade()), None);
    }

    /// DXGI (aus der Factory) und DisplayConfig (live) uneinig ueber "HDR
    /// verwenden": ein Zeichen fuer eine veraltete Factory.
    #[test]
    fn widerspruch_zwischen_dxgi_und_displayconfig() {
        let alt = vec![ausgang(1, H_X27, X27, farbe(false, 417.0, 270.0))];
        let z = schirm_zuordnen(&fenster(H_X27, X27), &alt, &pfade()).expect("zugeordnet");
        assert!(z.widerspruch);
        assert!(z.text().ends_with(" - DisplayConfig meldet HDR, es gilt DXGI"), "{}", z.text());
        let mut p = pfade();
        p[1].hdr = None;
        assert!(!schirm_zuordnen(&fenster(H_X27, X27), &alt, &p).unwrap().widerspruch);
        assert!(!schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &pfade()).unwrap().widerspruch);
    }

    /// Ein neuer HMONITOR und eine neue Factory fuer denselben Bildschirm
    /// (Windows ordnet nach einer Umstellung neu): dieselben Werte, dieselbe
    /// Kennung - kein Wechsel, also weder Umschalten noch IN_ANZEIGE.
    #[test]
    fn neuer_hmonitor_derselbe_schirm_ist_kein_wechsel() {
        let vorher = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &pfade());
        let h_neu = 0x0004_0005;
        let nachher_liste = vec![ausgang(0, H_PANEL, PANEL, farbe(false, 240.0, 240.0)), ausgang(1, h_neu, X27, farbe(true, 417.0, 270.0))];
        let nachher = schirm_zuordnen(&fenster(h_neu, X27), &nachher_liste, &pfade());
        assert_eq!(vorher, nachher);
        assert!(!schirm_gewechselt(vorher.as_ref(), nachher.as_ref()));
        // Andere Pegel desselben Bildschirms (SDR-Schieber) sind auch keiner.
        let mut p = pfade();
        p[1].sdr_weiss_nit = Some(240.0);
        let heller = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &p);
        assert!(!schirm_gewechselt(vorher.as_ref(), heller.as_ref()));
        // "HDR verwenden" aus auf demselben Bildschirm schon.
        let sdr = vec![ausgang(1, H_X27, X27, farbe(false, 417.0, 270.0))];
        let mut p = pfade();
        p[1].hdr = Some(false);
        let aus = schirm_zuordnen(&fenster(H_X27, X27), &sdr, &p);
        assert!(schirm_gewechselt(vorher.as_ref(), aus.as_ref()));
    }

    /// Die Protokollzeile nennt den Bildschirm, den Adapter und alle Werte.
    #[test]
    fn protokollzeile() {
        let z = schirm_zuordnen(&fenster(H_X27, X27), &frisch(), &pfade()).unwrap();
        assert_eq!(
            z.text(),
            r#"Fenster auf \\.\DISPLAY2 "X27 X1" (Ausgang von Adapter 1, NVIDIA GeForce RTX 3080 Ti Laptop GPU): HDR, SDR-Weiss 480 nit, Spitze 417 nit, Vollbild 270 nit"#
        );
        let z = schirm_zuordnen(&fenster(H_PANEL, PANEL), &frisch(), &pfade()).unwrap();
        assert_eq!(
            z.text(),
            r"Fenster auf \\.\DISPLAY1 (Ausgang von Adapter 0, Intel(R) UHD Graphics): SDR, SDR-Weiss 80 nit, Spitze 240 nit, Vollbild 240 nit"
        );
        let geliehen = vec![ausgang(0, H_X27, X27, farbe(true, 417.0, 270.0))];
        assert!(schirm_zuordnen(&fenster(H_X27, X27), &geliehen, &pfade()).unwrap().text().contains("; die Quelle betreibt eine andere Karte)"));
        assert_eq!(
            text_ohne_ausgang(&fenster(H_X27, X27)),
            r"Fenster auf \\.\DISPLAY2: kein DXGI-Ausgang zeigt diesen Monitor - unbekannt (gilt als SDR)"
        );
    }
}
