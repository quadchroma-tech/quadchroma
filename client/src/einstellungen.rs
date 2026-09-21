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

/// Welcher Decoder gewuenscht ist. Automatik nimmt NVDEC (ueber die
/// cuvid-Decoder von FFmpeg), wenn eine NVIDIA-Karte da ist, und faellt sonst
/// auf Software zurueck. Software erzwingt die CPU, NVIDIA verlangt die
/// Karte und sagt laut Bescheid, wenn sie doch nicht geht.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DecoderWunsch {
    #[default]
    Automatik,
    Software,
    Nvidia,
}

impl DecoderWunsch {
    /// Wert in der Datei und auf der Befehlszeile (--decoder).
    pub fn schluessel(self) -> &'static str {
        match self {
            DecoderWunsch::Automatik => "auto",
            DecoderWunsch::Software => "software",
            DecoderWunsch::Nvidia => "nvidia",
        }
    }

    /// Umkehrung von `schluessel`. Unbekanntes ergibt None, damit der
    /// Aufrufer bei der Voreinstellung bleibt.
    pub fn aus(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatik" | "automatic" => DecoderWunsch::Automatik,
            "software" | "cpu" => DecoderWunsch::Software,
            // Im Menue heisst der Knopf "Grafikkarte" - hinter ihm steht heute
            // NVDEC, spaeter auch D3D11VA fuer AMD und Intel (4:2:0).
            "gpu" | "grafikkarte" | "nvidia" | "nvdec" | "cuvid" => DecoderWunsch::Nvidia,
            _ => return None,
        })
    }
}

/// Wer ins Fenster zeichnet. Automatik nimmt die Grafikkarte (Direct3D 11
/// auf einem Hardware-Adapter), wenn eine da ist, und faellt sonst auf die
/// CPU (softbuffer) zurueck. Cpu erzwingt den alten Weg, Gpu verlangt die
/// Karte und sagt laut Bescheid, wenn sie doch nicht geht; Warp ist der
/// Software-Rasterizer von Windows - nur zum Pruefen auf Maschinen ohne
/// Karte. Gilt ab dem naechsten Start; im Lauf wird nicht umgeschaltet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AnzeigeWunsch {
    #[default]
    Automatik,
    Gpu,
    Cpu,
    Warp,
}

impl AnzeigeWunsch {
    /// Wert in der Datei und auf der Befehlszeile (--anzeige).
    pub fn schluessel(self) -> &'static str {
        match self {
            AnzeigeWunsch::Automatik => "auto",
            AnzeigeWunsch::Gpu => "gpu",
            AnzeigeWunsch::Cpu => "cpu",
            AnzeigeWunsch::Warp => "warp",
        }
    }

    /// Umkehrung von `schluessel`. Unbekanntes ergibt None, damit der
    /// Aufrufer bei der Voreinstellung bleibt.
    pub fn aus(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatik" | "automatic" => AnzeigeWunsch::Automatik,
            "gpu" | "grafikkarte" | "karte" | "d3d11" => AnzeigeWunsch::Gpu,
            "cpu" | "software" | "prozessor" => AnzeigeWunsch::Cpu,
            "warp" => AnzeigeWunsch::Warp,
            _ => return None,
        })
    }
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
    /// Gewuenschter Decoderpfad (Datei: decoder=auto|software|nvidia).
    pub decoder: DecoderWunsch,
    /// Gewuenschte Anzeige (Datei: anzeige=auto|gpu|cpu|warp).
    pub anzeige: AnzeigeWunsch,
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
            hosts: HashMap::new(),
        }
    }
}

fn pfad() -> Option<PathBuf> {
    let base = std::env::var("APPDATA").or_else(|_| std::env::var("HOME")).ok()?;
    let dir = PathBuf::from(base).join("QuadChroma");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("einstellungen.txt"))
}

impl Einstellungen {
    pub fn laden() -> Einstellungen {
        let mut e = Einstellungen::default();
        let Some(p) = pfad() else { return e };
        let Ok(text) = std::fs::read_to_string(&p) else { return e };

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
        std::fs::write(p, t).ok();
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
