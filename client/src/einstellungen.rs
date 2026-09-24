// Gespeicherte Einstellungen.
//
// Zwei Sorten: solche, die zum Programm gehoeren (Vollbild, Sprache), und
// solche, die zu einem bestimmten Host gehoeren (Datenrate, Bildrate, die
// beiden Schalter). Die zweite Sorte haengt am Fingerabdruck des Hosts, nicht
// an seiner Adresse - die kann sich im Netz jederzeit aendern, der Schluessel
// nicht.
//
// Das Format ist absichtlich stumpf: eine Zeile je Wert, Text. Wer will, kann
// es mit einem Editor reparieren.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostWerte {
    pub mbit: u32,
    pub fps: u16,
    pub gaming: bool,
    pub fest: bool,
    /// Ton uebertragen. Aus spart die rund 3 Mbit/s des unverdichteten Tons.
    pub ton: bool,
}

/// Welche Zeilen die Statistik zeigt. Nicht jeder will alles sehen - manchem
/// reicht die Bildrate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatWahl {
    pub fps: bool,
    pub latenz: bool,
    pub teile: bool,
    pub aufloesung: bool,
    pub codec: bool,
    pub verworfen: bool,
    pub code: bool,
}

impl Default for StatWahl {
    fn default() -> Self {
        StatWahl { fps: true, latenz: true, teile: true, aufloesung: true, codec: true, verworfen: true, code: true }
    }
}

/// Welcher Decoder gewuenscht ist. Die Rollen sind dieselben wie bei der
/// Anzeige: Gpu und Gpu2 sind die dedizierten Karten in der Reihenfolge, in
/// der DXGI sie zaehlt, Integriert die mit gemeinsamem Speicher (siehe
/// `anzeige::karten_erkennen`). Automatik nimmt NVDEC (ueber die cuvid-
/// Decoder von FFmpeg), wenn eine NVIDIA-Karte da ist, sonst D3D11VA auf
/// dem Adapter der Anzeige (nur 4:2:0 und H.264), sonst Software. Software
/// erzwingt die CPU; eine Rolle verlangt genau diese Karte und sagt laut
/// Bescheid, wenn sie doch nicht geht.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DecoderWunsch {
    #[default]
    Automatik,
    Software,
    Gpu,
    Gpu2,
    Integriert,
}

impl DecoderWunsch {
    /// Wert in der Datei und auf der Befehlszeile (--decoder).
    pub fn schluessel(self) -> &'static str {
        match self {
            DecoderWunsch::Automatik => "auto",
            DecoderWunsch::Software => "software",
            DecoderWunsch::Gpu => "gpu",
            DecoderWunsch::Gpu2 => "gpu2",
            DecoderWunsch::Integriert => "integriert",
        }
    }

    /// Umkehrung von `schluessel`. Unbekanntes ergibt None, damit der
    /// Aufrufer bei der Voreinstellung bleibt. Die alten Werte nvidia, nvdec
    /// und cuvid bleiben lesbar und meinen die (erste) Grafikkarte.
    pub fn aus(text: &str) -> Option<Self> {
        Some(match rolle_wort(text) {
            Some(RollenWort::Automatik) => DecoderWunsch::Automatik,
            Some(RollenWort::Prozessor) => DecoderWunsch::Software,
            Some(RollenWort::Gpu) => DecoderWunsch::Gpu,
            Some(RollenWort::Gpu2) => DecoderWunsch::Gpu2,
            Some(RollenWort::Integriert) => DecoderWunsch::Integriert,
            Some(RollenWort::Warp) | None => return None,
        })
    }
}

/// Wer ins Fenster zeichnet. Automatik nimmt Direct3D 11 auf dem ersten
/// Hardware-Adapter mit Bildschirmausgang (ohne einen solchen: der erste
/// von NVIDIA, sonst der erste ueberhaupt) und faellt ohne Karte auf die
/// CPU (softbuffer) zurueck. Gpu, Gpu2 und Integriert nehmen genau diese
/// Rolle aus der Erkennung; Cpu erzwingt den alten Weg; Warp ist der
/// Software-Rasterizer von Windows - nur zum Pruefen auf Maschinen ohne
/// Karte, nur auf der Befehlszeile. Gilt ab dem naechsten Start; im Lauf
/// wird nicht umgeschaltet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AnzeigeWunsch {
    #[default]
    Automatik,
    Gpu,
    Gpu2,
    Integriert,
    Cpu,
    Warp,
}

impl AnzeigeWunsch {
    /// Wert in der Datei und auf der Befehlszeile (--anzeige).
    pub fn schluessel(self) -> &'static str {
        match self {
            AnzeigeWunsch::Automatik => "auto",
            AnzeigeWunsch::Gpu => "gpu",
            AnzeigeWunsch::Gpu2 => "gpu2",
            AnzeigeWunsch::Integriert => "integriert",
            AnzeigeWunsch::Cpu => "cpu",
            AnzeigeWunsch::Warp => "warp",
        }
    }

    /// Umkehrung von `schluessel`. Unbekanntes ergibt None, damit der
    /// Aufrufer bei der Voreinstellung bleibt.
    pub fn aus(text: &str) -> Option<Self> {
        Some(match rolle_wort(text)? {
            RollenWort::Automatik => AnzeigeWunsch::Automatik,
            RollenWort::Gpu => AnzeigeWunsch::Gpu,
            RollenWort::Gpu2 => AnzeigeWunsch::Gpu2,
            RollenWort::Integriert => AnzeigeWunsch::Integriert,
            RollenWort::Prozessor => AnzeigeWunsch::Cpu,
            RollenWort::Warp => AnzeigeWunsch::Warp,
        })
    }
}

/// Die Woerter, die --decoder und --anzeige (und die Datei) gemeinsam
/// verstehen - deutsch, englisch und die Schreibweisen von frueher.
enum RollenWort {
    Automatik,
    Prozessor,
    Gpu,
    Gpu2,
    Integriert,
    Warp,
}

fn rolle_wort(text: &str) -> Option<RollenWort> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "auto" | "automatik" | "automatic" => RollenWort::Automatik,
        "cpu" | "software" | "prozessor" | "processor" => RollenWort::Prozessor,
        // "gpu" hiess bei der Anzeige schon immer die Karte; beim Decoder
        // stand bis 2026-09 "nvidia" in der Datei.
        "gpu" | "gpu1" | "grafikkarte" | "grafikkarte1" | "karte" | "d3d11" | "nvidia" | "nvdec" | "cuvid" => RollenWort::Gpu,
        "gpu2" | "grafikkarte2" => RollenWort::Gpu2,
        "integriert" | "integrated" | "igpu" | "intel" => RollenWort::Integriert,
        "warp" => RollenWort::Warp,
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub struct Einstellungen {
    pub vollbild: bool,
    pub pixelgenau: bool,
    pub overlay: bool,
    pub sprache: Option<String>,
    /// Erweiterte Statistik. Muss eigens eingeschaltet werden.
    pub nerd: bool,
    pub stats: StatWahl,
    /// Gewuenschter Decoderpfad (Datei: decoder=auto|software|gpu|gpu2|integriert).
    pub decoder: DecoderWunsch,
    /// Gewuenschte Anzeige (Datei: anzeige=auto|gpu|gpu2|integriert|cpu|warp).
    pub anzeige: AnzeigeWunsch,
    /// Schliessen legt die App in den Infobereich bzw. in die Menueleiste
    /// (Datei: tray=an|aus). Mit tray=aus beendet Schliessen das Programm
    /// wie frueher, und es gibt kein Symbol.
    pub tray: bool,
    /// Die einmalige Sprechblase beim ersten Ablegen ist gezeigt worden
    /// (Datei: tray_hinweis=1).
    pub tray_hinweis: bool,
    /// Fingerabdruck des Hosts -> seine Werte.
    pub hosts: HashMap<String, HostWerte>,
}

impl Default for Einstellungen {
    fn default() -> Self {
        Einstellungen {
            vollbild: true, // sein Wunsch: standardmaessig im Vollbild
            pixelgenau: false,
            overlay: false,
            sprache: None,
            nerd: false,
            stats: StatWahl::default(),
            decoder: DecoderWunsch::Automatik,
            anzeige: AnzeigeWunsch::Automatik,
            tray: true,
            tray_hinweis: false,
            hosts: HashMap::new(),
        }
    }
}

/// Eine Datei im Ablageordner des Clients (Windows %APPDATA%\QuadChroma,
/// macOS ~/Library/Application Support/QuadChroma - ein Ort fuer alles, siehe
/// secure::config_dir). Das Verzeichnis wird angelegt, falls es fehlt. Fuer
/// die Einstellungen, und fuer alles andere, was der Client dort ablegt
/// (etwa benchmark.txt).
pub fn datei_pfad(name: &str) -> Option<PathBuf> {
    crate::secure::config_dir().ok().map(|d| d.join(name))
}

fn pfad() -> Option<PathBuf> {
    datei_pfad("einstellungen.txt")
}

impl Einstellungen {
    pub fn laden() -> Einstellungen {
        let Some(p) = pfad() else { return Einstellungen::default() };
        let Ok(text) = std::fs::read_to_string(&p) else { return Einstellungen::default() };
        Einstellungen::aus_text(&text)
    }

    /// Der Inhalt von einstellungen.txt als Einstellungen (ohne Datei, damit
    /// es sich pruefen laesst).
    pub fn aus_text(text: &str) -> Einstellungen {
        let mut e = Einstellungen::default();
        // Ein fehlerhafter Eintrag darf nie den ganzen Start verhindern:
        // alles, was nicht gelesen werden kann, bleibt auf der Voreinstellung.
        let mut aktueller_host: Option<String> = None;
        for zeile in text.lines() {
            let z = zeile.trim();
            if z.is_empty() || z.starts_with('#') {
                continue;
            }
            if let Some(fp) = z.strip_prefix("host ") {
                aktueller_host = Some(fp.trim().to_string());
                e.hosts.entry(fp.trim().to_string()).or_insert(HostWerte {
                    mbit: 50,
                    fps: 120,
                    gaming: false,
                    fest: false,
                    ton: true,
                });
                continue;
            }
            let Some((k, v)) = z.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match (&aktueller_host, k) {
                (None, "vollbild") => e.vollbild = v == "1",
                (None, "pixelgenau") => e.pixelgenau = v == "1",
                (None, "overlay") => e.overlay = v == "1",
                (None, "sprache") => e.sprache = Some(v.to_string()),
                (None, "nerd") => e.nerd = v == "1",
                (None, "stat_fps") => e.stats.fps = v == "1",
                (None, "stat_latenz") => e.stats.latenz = v == "1",
                (None, "stat_teile") => e.stats.teile = v == "1",
                (None, "stat_aufloesung") => e.stats.aufloesung = v == "1",
                (None, "stat_codec") => e.stats.codec = v == "1",
                (None, "stat_verworfen") => e.stats.verworfen = v == "1",
                (None, "stat_code") => e.stats.code = v == "1",
                (None, "decoder") => e.decoder = DecoderWunsch::aus(v).unwrap_or(e.decoder),
                (None, "anzeige") => e.anzeige = AnzeigeWunsch::aus(v).unwrap_or(e.anzeige),
                (None, "tray") => e.tray = schalter(v).unwrap_or(e.tray),
                (None, "tray_hinweis") => e.tray_hinweis = v == "1",
                (Some(fp), _) => {
                    if let Some(h) = e.hosts.get_mut(fp) {
                        match k {
                            "mbit" => h.mbit = v.parse().unwrap_or(h.mbit),
                            "fps" => h.fps = v.parse().unwrap_or(h.fps),
                            "gaming" => h.gaming = v == "1",
                            "fest" => h.fest = v == "1",
                            "ton" => h.ton = v == "1",
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        e
    }

    pub fn sichern(&self) {
        let Some(p) = pfad() else { return };
        std::fs::write(p, self.als_text()).ok();
    }

    /// Die Einstellungen als Inhalt von einstellungen.txt.
    pub fn als_text(&self) -> String {
        let mut t = String::from("# QuadChroma, gespeicherte Einstellungen\n");
        t.push_str(&format!("vollbild={}\n", self.vollbild as u8));
        t.push_str(&format!("pixelgenau={}\n", self.pixelgenau as u8));
        t.push_str(&format!("overlay={}\n", self.overlay as u8));
        if let Some(s) = &self.sprache {
            t.push_str(&format!("sprache={s}\n"));
        }
        t.push_str(&format!("nerd={}\n", self.nerd as u8));
        let w = &self.stats;
        t.push_str(&format!(
            "stat_fps={}\nstat_latenz={}\nstat_teile={}\nstat_aufloesung={}\nstat_codec={}\nstat_verworfen={}\nstat_code={}\n",
            w.fps as u8, w.latenz as u8, w.teile as u8, w.aufloesung as u8,
            w.codec as u8, w.verworfen as u8, w.code as u8
        ));
        t.push_str(&format!("decoder={}\n", self.decoder.schluessel()));
        t.push_str(&format!("anzeige={}\n", self.anzeige.schluessel()));
        t.push_str(&format!("tray={}\n", if self.tray { "an" } else { "aus" }));
        t.push_str(&format!("tray_hinweis={}\n", self.tray_hinweis as u8));
        // Sortiert schreiben, damit die Datei zwischen zwei Laeufen gleich
        // aussieht und man Aenderungen erkennt.
        let mut fps: Vec<&String> = self.hosts.keys().collect();
        fps.sort();
        for fp in fps {
            let h = &self.hosts[fp];
            t.push_str(&format!(
                "\nhost {fp}\nmbit={}\nfps={}\ngaming={}\nfest={}\nton={}\n",
                h.mbit, h.fps, h.gaming as u8, h.fest as u8, h.ton as u8
            ));
        }
        t
    }

    pub fn fuer_host(&self, fingerabdruck: &str) -> Option<HostWerte> {
        self.hosts.get(fingerabdruck).copied()
    }

    pub fn host_merken(&mut self, fingerabdruck: &str, w: HostWerte) {
        let alt = self.hosts.insert(fingerabdruck.to_string(), w);
        if alt != Some(w) {
            self.sichern();
        }
    }
}

/// Ein Schalter in der Datei: an/aus, dazu 1/0, ja/nein und on/off. Anderes
/// ergibt None, dann bleibt die Voreinstellung.
fn schalter(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "an" | "1" | "ja" | "on" | "true" => Some(true),
        "aus" | "0" | "nein" | "off" | "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tray und tray_hinweis: Voreinstellung, lesen, schreiben, und was
    /// nicht gelesen werden kann, laesst die Voreinstellung stehen.
    #[test]
    fn tray_lesen_und_schreiben() {
        let e = Einstellungen::default();
        assert!(e.tray);
        assert!(!e.tray_hinweis);
        // Eine alte Datei ohne die Zeilen: Ablegen an, Hinweis noch offen.
        let alt = Einstellungen::aus_text("vollbild=1\nsprache=de\n");
        assert!(alt.tray && !alt.tray_hinweis);
        assert_eq!(alt.sprache.as_deref(), Some("de"));

        let aus = Einstellungen::aus_text("tray=aus\ntray_hinweis=1\n");
        assert!(!aus.tray);
        assert!(aus.tray_hinweis);
        for (v, soll) in [("an", true), ("AUS", false), ("0", false), ("1", true), (" nein ", false), ("off", false), ("quatsch", true)] {
            assert_eq!(Einstellungen::aus_text(&format!("tray={v}\n")).tray, soll, "tray={v}");
        }
        assert!(!Einstellungen::aus_text("tray_hinweis=0\n").tray_hinweis);
        // Unter einem Host-Block gehoeren die Zeilen dem Host, nicht dem
        // Programm.
        let h = Einstellungen::aus_text("host AAAA-BBBB-CCCC-DDDD\ntray=aus\ntray_hinweis=1\n");
        assert!(h.tray && !h.tray_hinweis);

        // Schreiben und wieder lesen.
        let mut e = Einstellungen::default();
        let t = e.als_text();
        assert!(t.contains("\ntray=an\n"), "{t}");
        assert!(t.contains("\ntray_hinweis=0\n"), "{t}");
        e.tray = false;
        e.tray_hinweis = true;
        e.hosts.insert("AAAA-BBBB-CCCC-DDDD".into(), HostWerte { mbit: 80, fps: 60, gaming: true, fest: false, ton: true });
        let t = e.als_text();
        assert!(t.contains("\ntray=aus\n"), "{t}");
        assert!(t.contains("\ntray_hinweis=1\n"), "{t}");
        // Die Programmwerte stehen vor dem ersten Host-Block.
        assert!(t.find("tray=").unwrap() < t.find("host ").unwrap());
        let zurueck = Einstellungen::aus_text(&t);
        assert!(!zurueck.tray && zurueck.tray_hinweis);
        assert_eq!(zurueck.fuer_host("AAAA-BBBB-CCCC-DDDD"), e.fuer_host("AAAA-BBBB-CCCC-DDDD"));
    }
}
