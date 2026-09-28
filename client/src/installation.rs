// Installation fuer "Mit Windows starten" (nur Windows).
//
// Die geplante Aufgabe startet quadchroma.exe bei der Anmeldung mit hoechsten
// Rechten und OHNE UAC-Abfrage (verknuepfung.rs). Zeigte sie auf den Ordner,
// in den der Nutzer das ZIP entpackt hat (Downloads, Desktop, ein
// Netzlaufwerk), koennte jedes Programm mit normalen Rechten die exe oder eine
// der FFmpeg-DLLs dort austauschen und liefe bei der naechsten Anmeldung still
// als Administrator. Deshalb installiert sich die (schon erhoehte) App beim
// Einschalten nach %ProgramFiles%\QuadChroma - dort duerfen nur
// Administratoren schreiben (geerbt von Program Files) -, und die Aufgabe
// startet die exe von dort. Ordner und Dateien werden vorher geprueft: lokales
// festes Laufwerk, kein Verweis (Junction), Besitzer Administratoren, SYSTEM
// oder TrustedInstaller, und kein Eintrag der DACL gibt einem anderen Konto
// Schreib-, Loesch- oder Rechteaenderungsrechte. Sonst verweigert.
//
// Die Quellen sind beim Start gesperrt (quellen_sperren, erster Schritt in
// main): die laufende exe und die geladenen avcodec-/avutil-DLLs werden nur
// zum Lesen geoeffnet und fuer andere nur lesbar (FILE_SHARE_READ, kein
// FILE_SHARE_WRITE, kein FILE_SHARE_DELETE); die Griffe bleiben offen, bis
// der Prozess endet. Solange laesst sich keine dieser Dateien umbenennen,
// verschieben, ueberschreiben oder loeschen - auch nicht der Ordner, in dem
// sie liegen. Beim Sperren wird geprueft, dass der geoeffnete Griff die
// geladene Datei ist: sein Pfad (GetFinalPathNameByHandleW) ist der des
// geladenen Moduls (GetModuleFileNameW), und die Datei hinter dem geladenen
// Bild (GetMappedFileNameW - dieser Name folgt einem Umbenennen, der des
// Moduls nicht) liegt unter demselben Pfad wie der Griff. Wer die Datei
// zwischen Prozessstart und Sperre beiseite benannt und eine eigene an ihren
// Platz gelegt hat, faellt hier auf. Gelesen wird nur ueber diese Griffe, nie
// erneut ueber den Pfad, und jede gelesene exe bzw. DLL wird vor dem
// Schreiben mit dem geladenen Bild im Speicher verglichen (bild_vergleichen:
// Koepfe und alle nicht beschreibbaren Abschnitte - Code, Konstanten,
// Ressourcen - nach Anwenden der Relokationen; ausgenommen sind nur die
// Stellen, die der Lader selbst schreibt: Importtabellen, Zeiger der
// Ladekonfiguration). Beschreibbare Abschnitte (.data) veraendert das
// Programm beim Laufen selbst - die lassen sich nicht vergleichen. Laesst
// sich eine Datei nicht sperren oder stimmt sie nicht, wird nicht installiert
// und nicht erneuert; die Aufgabe bleibt, wie sie ist.
//
// Laeuft die App aus dem Installationsordner selbst, wird nichts gesperrt:
// dort kann nur ein Administrator schreiben, und eine neuere Fassung muss die
// laufende beiseite benennen koennen (siehe unten).
//
// Keine Rueckstufung: ersetzt wird die installierte Kopie nur, wenn die
// laufende exe nicht aelter ist - verglichen wird die Dateiversion aus der
// Versionsressource (VS_FIXEDFILEINFO, aus res/quadchroma.rc ueber build.rs)
// der laufenden und der installierten exe. Gleiche Version mit anderem
// Inhalt (SHA-256 von exe oder DLLs) ersetzt (ein neuer Bau derselben
// Fassung); eine aeltere ersetzt nie eine neuere (erneuern_entscheiden).
//
// Kopieren: jede Datei ganz in den Speicher lesen (exe und DLLs ueber die
// gesperrten Griffe, die Texte waehrenddessen fuer andere nur lesbar
// geoeffnet), SHA-256 bilden, unter einem Zwischennamen in den Zielordner
// schreiben, zuruecklesen und vergleichen, die Rechte der neuen Datei
// pruefen. Erst wenn alle Dateien so bereitliegen, werden sie umbenannt, die
// exe zuletzt: eine vorhandene Datei erst beiseite (<name>.alt-<pid>), dann
// die neue an ihren Platz. Die beiseite gelegten werden danach geloescht;
// laeuft die alte exe noch (etwa in einer anderen Sitzung), geht das erst
// nach dem Neustart (MOVEFILE_DELAY_UNTIL_REBOOT). Scheitert ein Schritt,
// wird zurueckgerollt.
//
// Kopiert wird, was der Nutzer gerade erhoeht gestartet hat. Wer den
// Quellordner beschreiben kann, haette die exe schon vor diesem Start
// austauschen koennen - genau deshalb darf die Aufgabe nicht dorthin zeigen.
//
// Dazu die Pruefung des Kontos: ein Windows-Standardkonto startet die App mit
// den Zugangsdaten eines Administrators (UAC). Dann gehoert der Prozess einem
// anderen Konto als die Sitzung, und eine Aufgabe "bei der Anmeldung, hoechste
// Rechte" kann fuer ein Standardkonto keine exe mit requireAdministrator still
// starten - der Autostart ist dann gesperrt (konto()).
//
// Reine Logik (Pfade, SID und ACL aus Bytes, Befehl, Konto und Zustand der
// Aufgabe aus ihrem XML, Kopf, Version und Bildvergleich einer PE-Datei, die
// Entscheidungen zu Konto und Erneuern) laeuft auf jeder Plattform und in den
// Tests; Dateien, Sperren, Rechte und Konten hinter cfg(windows).

// Auf dem Mac gibt es keine Installation: dort bleibt ein Teil ungenutzt
// (Tests laufen trotzdem auf beiden Plattformen).
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

/// Name des Installationsordners unter %ProgramFiles%.
pub const ORDNER: &str = "QuadChroma";
/// Name der installierten exe (die laufende kann anders heissen, etwa im Test).
pub const EXE: &str = "quadchroma.exe";
/// Die FFmpeg-DLLs, ohne die die exe nicht startet (Pflicht).
pub const DLLS: [&str; 2] = ["avcodec-63.dll", "avutil-61.dll"];
/// Die Begleittexte des Pakets - mitkopiert, wenn sie neben der exe liegen.
pub const TEXTE: [&str; 5] = ["LICENSE.txt", "THIRD_PARTY_NOTICES.txt", "README.txt", "MANUAL.txt", "FFMPEG-BUILDINFO.txt"];

/// Vertraute Konten: Administratoren, SYSTEM, TrustedInstaller. Nur sie
/// duerfen Besitzer sein oder schreiben.
const VERTRAUT: [&str; 3] = ["S-1-5-32-544", "S-1-5-18", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"];

/// Rechte, mit denen ein Konto die exe oder DLL austauschen koennte: Daten
/// schreiben bzw. Datei anlegen (0x2), anhaengen bzw. Unterordner anlegen
/// (0x4), Kinder loeschen (0x40), DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_ALL,
/// GENERIC_WRITE.
const SCHREIBRECHTE: u32 = 0x2 | 0x4 | 0x40 | 0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x1000_0000 | 0x4000_0000;

/// INHERIT_ONLY_ACE: der Eintrag gilt nur fuer Kinder, nicht fuer das Objekt.
const NUR_VERERBT: u8 = 0x08;

/// Ein Eintrag einer DACL, wie gelesen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    pub typ: u8,
    pub flags: u8,
    pub maske: u32,
    /// Die SID als Text ("S-1-5-32-544"); leer, wenn der Typ keine an
    /// fester Stelle hat (Objekt-Eintraege).
    pub sid: String,
}

/// Eine SID aus ihren Bytes als Text: Revision, Zahl der Unterautoritaeten,
/// 6 Byte Autoritaet (big endian), dann je 4 Byte (little endian).
pub fn sid_text(b: &[u8]) -> Option<String> {
    let (&rev, &n) = (b.first()?, b.get(1)?);
    let n = n as usize;
    if b.len() < 8 + 4 * n {
        return None;
    }
    let autoritaet = b[2..8].iter().fold(0u64, |a, &x| (a << 8) | x as u64);
    let mut t = format!("S-{rev}-{autoritaet}");
    for i in 0..n {
        let s = u32::from_le_bytes([b[8 + 4 * i], b[9 + 4 * i], b[10 + 4 * i], b[11 + 4 * i]]);
        t.push_str(&format!("-{s}"));
    }
    Some(t)
}

/// Ein ACE aus seinen Bytes: Kopf (Typ, Flags, Groesse), dann bei den
/// einfachen Typen (erlaubt 0, verweigert 1, Callback 9 und 10) die Maske und
/// die SID ab Byte 8. Andere Typen ohne SID.
pub fn ace_lesen(b: &[u8]) -> Option<Ace> {
    if b.len() < 8 {
        return None;
    }
    let (typ, flags) = (b[0], b[1]);
    let groesse = u16::from_le_bytes([b[2], b[3]]) as usize;
    if groesse < 8 {
        return None;
    }
    let b = b.get(..groesse)?;
    let maske = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let sid = if matches!(typ, 0 | 1 | 9 | 10) { sid_text(b.get(8..)?)? } else { String::new() };
    Some(Ace { typ, flags, maske, sid })
}

/// Duerfen nur vertraute Konten diesen Ordner bzw. diese Datei aendern?
/// `dacl` None heisst NULL-DACL (jeder darf alles). Verweigernde Eintraege
/// und solche, die nur vererbt werden, zaehlen nicht; ein erlaubender Eintrag
/// unbekannter Form wird vorsichtshalber abgelehnt.
pub fn rechte_pruefen(besitzer: &str, dacl: Option<&[Ace]>) -> Result<(), String> {
    if !VERTRAUT.contains(&besitzer) {
        return Err(format!("Besitzer {besitzer} ist weder Administratoren noch SYSTEM noch TrustedInstaller"));
    }
    let Some(dacl) = dacl else {
        return Err("keine DACL - jeder darf schreiben".into());
    };
    for a in dacl {
        if a.flags & NUR_VERERBT != 0 {
            continue;
        }
        match a.typ {
            // erlaubt bzw. erlaubt mit Bedingung
            0 | 9 => {
                if a.maske & SCHREIBRECHTE != 0 && !VERTRAUT.contains(&a.sid.as_str()) {
                    return Err(format!("{} darf hier schreiben (Rechte 0x{:08x})", a.sid, a.maske));
                }
            }
            // verweigernd
            1 | 6 | 10 | 12 => {}
            t => return Err(format!("unbekannter erlaubender Eintrag (Typ {t})")),
        }
    }
    Ok(())
}

/// Ein Pfad auf einem lokalen Laufwerk ("C:\..." bzw. "\\?\C:\...")? Kein
/// UNC-Pfad ("\\server\freigabe", "\\?\UNC\..."), kein Geraetepfad ("\\.\"),
/// kein relativer Pfad. Ob der Laufwerksbuchstabe ein Netzlaufwerk ist, sagt
/// erst das System (laufwerk_pruefen).
pub fn lokaler_laufwerkspfad(p: &str) -> bool {
    let p = p.replace('/', "\\");
    let rest = p.strip_prefix("\\\\?\\").unwrap_or(&p);
    let b = rest.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// Zwei Pfade gleich (ohne Anfuehrungszeichen, "\\?\" und "\" am Ende,
/// Gross- und Kleinschreibung egal)? Reiner Textvergleich.
pub fn gleicher_pfad(a: &str, b: &str) -> bool {
    let norm = |p: &str| -> String {
        let p = p.trim().trim_matches('"').replace('/', "\\");
        let p = p.strip_prefix("\\\\?\\").unwrap_or(&p).trim_end_matches('\\').to_string();
        p.to_lowercase()
    };
    norm(a) == norm(b)
}

/// XML-Entitaeten aufloesen (&amp; &lt; &gt; &quot; &apos;).
fn xml_text(t: &str) -> String {
    t.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Das Gegenstueck zu xml_text: & < > " ' als Entitaeten, fuer jeden Wert,
/// der in das XML einer Aufgabe eingesetzt wird.
pub fn xml_maskieren(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&apos;"),
            c => s.push(c),
        }
    }
    s
}

/// Der Inhalt des ersten Elements `name` in `xml`, mit oder ohne Attribute
/// (`<Principal id="Author">`); ein leeres `<name/>` ergibt "". None ohne
/// ein solches Element. Ein laengerer Name mit demselben Anfang zaehlt nicht
/// (`<Settings>` ist nicht `<SettingsX>`, und `<IdleSettings>` beginnt gar
/// nicht mit `<Settings`).
pub fn xml_element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let auf = format!("<{name}");
    let mut von = 0;
    while let Some(i) = xml[von..].find(&auf) {
        let kopf_ab = von + i + auf.len();
        let rest = &xml[kopf_ab..];
        if rest.starts_with(['>', '/']) || rest.starts_with(char::is_whitespace) {
            let kopf_ende = rest.find('>')?;
            if rest[..kopf_ende].ends_with('/') {
                return Some("");
            }
            let inhalt = &rest[kopf_ende + 1..];
            return inhalt.find(&format!("</{name}>")).map(|j| &inhalt[..j]);
        }
        von = kopf_ab;
    }
    None
}

/// Der Text des Elements am Ende von `pfad` - je Schritt das erste Element
/// dieses Namens innerhalb des vorigen -, Entitaeten aufgeloest, ohne
/// Leerraum am Rand. None, wenn eines davon fehlt.
pub fn xml_wert(xml: &str, pfad: &[&str]) -> Option<String> {
    let mut t = xml;
    for name in pfad {
        t = xml_element(t, name)?;
    }
    Some(xml_text(t.trim()))
}

/// Befehl und Argumente der ersten Aktion aus dem XML einer geplanten
/// Aufgabe (IRegisteredTask::Xml bzw. schtasks /Query /XML):
/// <Exec><Command>..</Command>
/// <Arguments>..</Arguments></Exec>. Ohne <Exec> None; ohne Argumente leer.
pub fn aufgabe_aus_xml(xml: &str) -> Option<(String, String)> {
    let exec = &xml[xml.find("<Exec>")?..];
    let exec = &exec[..exec.find("</Exec>")?];
    let wert = |tag: &str| -> Option<String> {
        let auf = format!("<{tag}>");
        let i = exec.find(&auf)? + auf.len();
        let j = exec[i..].find(&format!("</{tag}>"))?;
        Some(xml_text(exec[i..i + j].trim()))
    };
    Some((wert("Command")?, wert("Arguments").unwrap_or_default()))
}

/// Startet die Aufgabe mit diesem Befehl genau `exe --hintergrund`?
pub fn aufgabe_zeigt_auf(befehl: &str, argumente: &str, exe: &Path) -> bool {
    gleicher_pfad(befehl, &exe.to_string_lossy()) && argumente.trim() == crate::verknuepfung::AUTOSTART_ARGUMENT
}

/// Das Konto, als das die Aufgabe laeuft (<Principals><Principal><UserId>),
/// wie ihr XML es zeigt: "RECHNER\name", "DOMAENE\name" oder
/// eine SID ("S-1-5-21-..."). None ohne UserId (etwa eine Gruppe als
/// Prinzipal) - dann gehoert die Aufgabe keinem einzelnen Konto.
pub fn aufgabe_konto(xml: &str) -> Option<String> {
    let p = &xml[xml.find("<Principals>")?..];
    let p = &p[..p.find("</Principals>")?];
    let i = p.find("<UserId>")? + "<UserId>".len();
    let j = p[i..].find("</UserId>")?;
    let t = xml_text(p[i..i + j].trim());
    (!t.is_empty()).then_some(t)
}

/// Ist die Aufgabe eingeschaltet? schtasks /Change /DISABLE setzt
/// <Settings><Enabled>false</Enabled>; fehlt der Eintrag, ist sie an. Das
/// Enabled eines Ausloesers (<LogonTrigger>) zaehlt hier nicht.
pub fn aufgabe_aktiv(xml: &str) -> bool {
    let Some(i) = xml.find("<Settings>") else { return true };
    let s = &xml[i..];
    let s = &s[..s.find("</Settings>").unwrap_or(s.len())];
    match s.find("<Enabled>") {
        Some(j) => !s[j + "<Enabled>".len()..].trim_start().to_ascii_lowercase().starts_with("false"),
        None => true,
    }
}

/// SHA-256 als Hex (fuer Protokoll und Vergleich).
pub fn sha256_hex(daten: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(daten).iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------ PE-Dateien
//
// exe und DLLs sind PE-Dateien. Gelesen wird nur, was die Installation
// braucht: Koepfe und Abschnittstabelle, die Versionsressource und - fuer den
// Vergleich mit dem geladenen Bild - Relokationen, Importtabellen und
// Ladekonfiguration. Alles aus Bytes, ohne Windows-Aufrufe (version.dll
// bliebe sonst eine weitere DLL, die das System im Ordner der exe suchte).

fn u16_bei(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i.checked_add(2)?)?.try_into().ok()?))
}

fn u32_bei(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i.checked_add(4)?)?.try_into().ok()?))
}

fn u64_bei(b: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(i..i.checked_add(8)?)?.try_into().ok()?))
}

/// Ein Abschnitt aus der Abschnittstabelle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Abschnitt {
    pub name: String,
    /// Adresse im geladenen Bild (RVA) und Groesse dort.
    pub va: u32,
    pub vgroesse: u32,
    /// Stelle und Groesse in der Datei.
    pub roh: u32,
    pub rohgroesse: u32,
    pub merkmale: u32,
}

impl Abschnitt {
    /// Wie viele Bytes aus der Datei im Bild stehen.
    fn laenge(&self) -> u32 {
        if self.vgroesse == 0 {
            self.rohgroesse
        } else {
            self.rohgroesse.min(self.vgroesse)
        }
    }
}

/// IMAGE_SCN_MEM_WRITE: ein beschreibbarer Abschnitt.
const BESCHREIBBAR: u32 = 0x8000_0000;

/// Datenverzeichnisse: Importe, Ressourcen, Relokationen,
/// Ladekonfiguration, Importadressen (IAT), verzoegerte Importe.
const VZ_IMPORT: usize = 1;
const VZ_RESSOURCEN: usize = 2;
const VZ_RELOKATIONEN: usize = 5;
const VZ_LADEKONFIGURATION: usize = 10;
const VZ_IAT: usize = 12;
const VZ_VERZOEGERT: usize = 13;

/// Was aus den Koepfen einer PE-Datei gebraucht wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeKopf {
    /// Wo der optionale Kopf beginnt.
    opt: usize,
    /// PE32+ (64 Bit); sonst PE32.
    pub pe32plus: bool,
    pub image_base: u64,
    /// SizeOfHeaders und SizeOfImage.
    pub groesse_kopf: usize,
    pub groesse_bild: usize,
    pub abschnitte: Vec<Abschnitt>,
    /// Die Datenverzeichnisse: (RVA, Groesse).
    verzeichnisse: Vec<(u32, u32)>,
}

const ABGESCHNITTEN: &str = "PE-Kopf abgeschnitten";

/// Koepfe und Abschnittstabelle einer PE-Datei lesen (auch aus dem Anfang
/// eines geladenen Bildes: dort stehen dieselben Koepfe).
pub fn pe_kopf(b: &[u8]) -> Result<PeKopf, String> {
    if b.get(..2) != Some(b"MZ".as_slice()) {
        return Err("keine PE-Datei (kein MZ)".into());
    }
    let e = u32_bei(b, 0x3c).ok_or(ABGESCHNITTEN)? as usize;
    if b.get(e..e.saturating_add(4)) != Some(b"PE\0\0".as_slice()) {
        return Err("keine PE-Datei (keine PE-Signatur)".into());
    }
    let coff = e + 4;
    let zahl = u16_bei(b, coff + 2).ok_or(ABGESCHNITTEN)? as usize;
    let opt_groesse = u16_bei(b, coff + 16).ok_or(ABGESCHNITTEN)? as usize;
    let opt = coff + 20;
    let (pe32plus, image_base, zahl_vz_bei, vz_bei) = match u16_bei(b, opt).ok_or(ABGESCHNITTEN)? {
        0x20b => (true, u64_bei(b, opt + 24).ok_or(ABGESCHNITTEN)?, opt + 108, opt + 112),
        0x10b => (false, u32_bei(b, opt + 28).ok_or(ABGESCHNITTEN)? as u64, opt + 92, opt + 96),
        m => return Err(format!("unbekannter optionaler Kopf 0x{m:x}")),
    };
    let groesse_bild = u32_bei(b, opt + 56).ok_or(ABGESCHNITTEN)? as usize;
    let groesse_kopf = u32_bei(b, opt + 60).ok_or(ABGESCHNITTEN)? as usize;
    let zahl_vz = (u32_bei(b, zahl_vz_bei).ok_or(ABGESCHNITTEN)? as usize).min(16);
    if vz_bei + 8 * zahl_vz > opt + opt_groesse {
        return Err("Datenverzeichnisse ausserhalb des optionalen Kopfes".into());
    }
    let verzeichnisse = (0..zahl_vz)
        .map(|i| Some((u32_bei(b, vz_bei + 8 * i)?, u32_bei(b, vz_bei + 8 * i + 4)?)))
        .collect::<Option<Vec<_>>>()
        .ok_or(ABGESCHNITTEN)?;
    let tabelle = opt + opt_groesse;
    if tabelle + 40 * zahl > groesse_kopf {
        return Err("Abschnittstabelle ausserhalb der Koepfe".into());
    }
    let abschnitte = (0..zahl)
        .map(|i| {
            let s = tabelle + 40 * i;
            Some(Abschnitt {
                name: String::from_utf8_lossy(b.get(s..s + 8)?).trim_end_matches('\0').to_string(),
                vgroesse: u32_bei(b, s + 8)?,
                va: u32_bei(b, s + 12)?,
                rohgroesse: u32_bei(b, s + 16)?,
                roh: u32_bei(b, s + 20)?,
                merkmale: u32_bei(b, s + 36)?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(ABGESCHNITTEN)?;
    Ok(PeKopf { opt, pe32plus, image_base, groesse_kopf, groesse_bild, abschnitte, verzeichnisse })
}

impl PeKopf {
    /// Die Stelle in der Datei zu einer RVA; None, wenn dort nichts aus der
    /// Datei steht.
    pub fn stelle(&self, rva: u32) -> Option<usize> {
        if (rva as usize) < self.groesse_kopf {
            return Some(rva as usize);
        }
        self.abschnitte.iter().find(|a| rva >= a.va && rva - a.va < a.rohgroesse).map(|a| (a.roh + (rva - a.va)) as usize)
    }

    /// Ein Datenverzeichnis, wenn es belegt ist.
    fn verzeichnis(&self, i: usize) -> Option<(u32, u32)> {
        self.verzeichnisse.get(i).copied().filter(|&(rva, n)| rva != 0 && n != 0)
    }

    /// Breite eines Zeigers im Bild.
    fn zeigerbreite(&self) -> u32 {
        if self.pe32plus {
            8
        } else {
            4
        }
    }

    /// Ein Zeiger aus der Datei an der RVA.
    fn zeiger(&self, b: &[u8], rva: u32) -> Option<u64> {
        let s = self.stelle(rva)?;
        if self.pe32plus {
            u64_bei(b, s)
        } else {
            u32_bei(b, s).map(u64::from)
        }
    }
}

/// Eine Dateiversion (FILEVERSION a,b,c,d); der Vergleich geht Stelle fuer
/// Stelle von vorn (0.2.0.0 ist neuer als 0.1.9.9).
pub type Version = [u16; 4];

/// "0.1.0.0"
pub fn version_text(v: &Version) -> String {
    format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3])
}

/// Signatur von VS_FIXEDFILEINFO.
const FESTE_VERSION: u32 = 0xFEEF_04BD;

/// Wo VS_FIXEDFILEINFO der Versionsressource in der Datei steht, und die
/// Dateiversion daraus (dwFileVersionMS/LS). Der Weg: Ressourcenverzeichnis
/// -> Typ 16 (RT_VERSION) -> erster Name -> erste Sprache -> Daten
/// (VS_VERSIONINFO mit dem Schluessel "VS_VERSION_INFO", dahinter auf 4 Byte
/// ausgerichtet die feste Struktur). None ohne Ressource oder wenn etwas
/// nicht stimmt.
pub fn pe_version_stelle(b: &[u8]) -> Option<(usize, Version)> {
    const UNTER: u32 = 0x8000_0000;
    let k = pe_kopf(b).ok()?;
    let (rva, groesse) = k.verzeichnis(VZ_RESSOURCEN)?;
    let wurzel = k.stelle(rva)?;
    let r = b.get(wurzel..wurzel.checked_add(groesse as usize)?.min(b.len()))?;
    // Der Eintrag eines Verzeichnisses mit der Nummer `id` (None: der erste).
    // Erst die benannten Eintraege, dann die mit Nummer.
    let eintrag = |off: usize, id: Option<u32>| -> Option<u32> {
        let benannt = u16_bei(r, off + 12)? as usize;
        let nummern = u16_bei(r, off + 14)? as usize;
        (0..benannt + nummern).find_map(|i| {
            let e = off + 16 + 8 * i;
            let passt = match id {
                Some(id) => i >= benannt && u32_bei(r, e)? == id,
                None => true,
            };
            if passt {
                u32_bei(r, e + 4)
            } else {
                None
            }
        })
    };
    let typ = eintrag(0, Some(16)).filter(|t| t & UNTER != 0)?;
    let name = eintrag((typ & !UNTER) as usize, None).filter(|n| n & UNTER != 0)?;
    let daten = eintrag((name & !UNTER) as usize, None).filter(|d| d & UNTER == 0)? as usize;
    let (d_rva, d_groesse) = (u32_bei(r, daten)?, u32_bei(r, daten + 4)? as usize);
    let s = k.stelle(d_rva)?;
    let v = b.get(s..s.checked_add(d_groesse)?)?;
    let schluessel: Vec<u8> = "VS_VERSION_INFO\0".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    if v.get(6..6 + schluessel.len())? != schluessel.as_slice() {
        return None;
    }
    let fest = (6 + schluessel.len()).next_multiple_of(4);
    if (u16_bei(v, 2)? as usize) < 52 || u32_bei(v, fest)? != FESTE_VERSION {
        return None;
    }
    let (ms, ls) = (u32_bei(v, fest + 8)?, u32_bei(v, fest + 12)?);
    Some((s + fest, [(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16]))
}

/// Die Dateiversion einer exe bzw. DLL (siehe pe_version_stelle).
pub fn pe_version(b: &[u8]) -> Option<Version> {
    pe_version_stelle(b).map(|(_, v)| v)
}

/// Laufende exe und installierte Kopie im Vergleich.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vergleich {
    /// exe und DLLs gleich (SHA-256).
    pub gleich: bool,
    /// Dateiversion der laufenden exe (None: ohne lesbare Versionsressource).
    pub laufend: Option<Version>,
    /// Dateiversion der installierten exe (None: fehlt oder ohne lesbare
    /// Version).
    pub installiert: Option<Version>,
}

/// Was mit der installierten Kopie geschieht.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Erneuern {
    /// Sie stimmt schon mit der laufenden ueberein.
    Gleich,
    /// Ersetzen: die laufende ist neuer oder gleich alt (ein anderer Bau
    /// derselben Fassung), oder die installierte fehlt bzw. hat keine
    /// lesbare Version.
    Ja,
    /// Nicht ersetzen: die installierte ist neuer - oder die laufende hat
    /// keine lesbare Version, die installierte schon (dann laesst sich nicht
    /// zeigen, dass die laufende nicht aelter ist).
    Nein { laufend: Option<Version>, installiert: Version },
}

/// Keine Rueckstufung: eine aeltere laufende exe ersetzt nie eine neuere
/// installierte Kopie.
pub fn erneuern_entscheiden(v: &Vergleich) -> Erneuern {
    if v.gleich {
        return Erneuern::Gleich;
    }
    match (v.laufend, v.installiert) {
        (_, None) => Erneuern::Ja,
        (Some(l), Some(i)) if l >= i => Erneuern::Ja,
        (laufend, Some(installiert)) => Erneuern::Nein { laufend, installiert },
    }
}

/// Die Zeile fuers Protokoll, wenn die installierte Kopie bleibt.
pub fn nicht_ersetzt_text(laufend: Option<Version>, installiert: Version) -> String {
    match laufend {
        Some(l) => format!(
            "die installierte Kopie ({}) ist neuer als diese exe ({}) - sie wird nicht durch die aeltere ersetzt",
            version_text(&installiert),
            version_text(&l)
        ),
        None => format!(
            "diese exe hat keine lesbare Versionsangabe, die installierte Kopie schon ({}) - sie wird nicht ersetzt",
            version_text(&installiert)
        ),
    }
}

/// Relokationstypen: ABSOLUTE (Fuellung), HIGHLOW (32 Bit), DIR64 (64 Bit).
const REL_ABSOLUT: u16 = 0;
const REL_HIGHLOW: u16 = 3;
const REL_DIR64: u16 = 10;

/// Die Relokationen der Datei: (RVA, Typ), ohne Fuellung. Andere Typen als
/// HIGHLOW und DIR64 gibt es bei x64 nicht - sie ergeben einen Fehler.
fn relokationen(b: &[u8], k: &PeKopf) -> Result<Vec<(u32, u16)>, String> {
    let Some((rva, groesse)) = k.verzeichnis(VZ_RELOKATIONEN) else {
        return Ok(Vec::new());
    };
    let s = k.stelle(rva).ok_or("Relokationen ausserhalb der Datei")?;
    let r = b.get(s..s.saturating_add(groesse as usize)).ok_or("Relokationen abgeschnitten")?;
    let mut v = Vec::new();
    let mut pos = 0;
    while pos + 8 <= r.len() {
        let seite = u32_bei(r, pos).ok_or("Relokationen abgeschnitten")?;
        let n = u32_bei(r, pos + 4).ok_or("Relokationen abgeschnitten")? as usize;
        if n < 8 || pos + n > r.len() {
            return Err(format!("Relokationsblock bei 0x{pos:x} kaputt (Groesse {n})"));
        }
        for i in (8..n).step_by(2) {
            let e = u16_bei(r, pos + i).ok_or("Relokationen abgeschnitten")?;
            match e >> 12 {
                REL_ABSOLUT => {}
                t @ (REL_HIGHLOW | REL_DIR64) => v.push((seite.wrapping_add(u32::from(e & 0xfff)), t)),
                t => return Err(format!("Relokation vom Typ {t} wird nicht unterstuetzt")),
            }
        }
        pos += n;
    }
    Ok(v)
}

/// Die Stellen, die der Lader in sonst nur lesbare Abschnitte schreibt und
/// die deshalb nicht verglichen werden: die Importadressen (IAT-Verzeichnis
/// und die Adresstabelle jedes Imports), bei verzoegerten Importen Adressen
/// und Modulgriff, und die Zeiger der Ladekonfiguration, die der Lader setzt
/// (Security Cookie, Control-Flow-Guard- und XFG-Pruefer, memcpy u. a.).
/// Jeweils [von, bis) als RVA.
fn lader_stellen(b: &[u8], k: &PeKopf) -> Vec<(u32, u32)> {
    let breite = k.zeigerbreite();
    // Zahl der Eintraege einer Tabelle, die mit einem Nullzeiger endet.
    let eintraege = |rva: u32| -> u32 {
        (0u32..1 << 20).take_while(|&i| k.zeiger(b, rva.wrapping_add(i * breite)).is_some_and(|z| z != 0)).count() as u32
    };
    let mut v = Vec::new();
    if let Some((rva, n)) = k.verzeichnis(VZ_IAT) {
        v.push((rva, rva.saturating_add(n)));
    }
    if let Some((rva, n)) = k.verzeichnis(VZ_IMPORT) {
        // IMAGE_IMPORT_DESCRIPTOR, 20 Byte; FirstThunk bei 16, Name bei 12.
        for i in 0..n / 20 {
            let Some(s) = k.stelle(rva + 20 * i) else { break };
            let (name, adressen) = (u32_bei(b, s + 12).unwrap_or(0), u32_bei(b, s + 16).unwrap_or(0));
            if name == 0 && adressen == 0 {
                break;
            }
            if adressen != 0 {
                v.push((adressen, adressen.saturating_add((eintraege(adressen) + 1) * breite)));
            }
        }
    }
    if let Some((rva, n)) = k.verzeichnis(VZ_VERZOEGERT) {
        // ImgDelayDescr, 32 Byte: Attribute, Name, Modulgriff, Adressen,
        // Namen, ... - als RVA (Attribut 1) oder, sehr alt, als Adresse.
        for i in 0..n / 32 {
            let Some(s) = k.stelle(rva + 32 * i) else { break };
            let feld = |o: usize| u32_bei(b, s + o).unwrap_or(0);
            if feld(4) == 0 {
                break;
            }
            let rva_von = |w: u32| if feld(0) & 1 != 0 { w } else { (u64::from(w).wrapping_sub(k.image_base)) as u32 };
            let (griff, adressen, namen) = (rva_von(feld(8)), rva_von(feld(12)), rva_von(feld(16)));
            if feld(8) != 0 {
                v.push((griff, griff.saturating_add(breite)));
            }
            if feld(12) != 0 {
                let zahl = if feld(16) != 0 { eintraege(namen) } else { eintraege(adressen) };
                v.push((adressen, adressen.saturating_add((zahl + 1) * breite)));
            }
        }
    }
    if let Some((rva, _)) = k.verzeichnis(VZ_LADEKONFIGURATION) {
        if let Some(s) = k.stelle(rva) {
            let groesse = u32_bei(b, s).unwrap_or(0) as usize;
            // Felder (Adressen), deren Ziel der Lader beschreibt:
            // SecurityCookie, GuardCFCheck-/DispatchFunctionPointer,
            // GuardRFFailureRoutineFunctionPointer,
            // GuardRFVerifyStackPointerFunctionPointer, GuardXFGCheck-,
            // -Dispatch-, -TableDispatchFunctionPointer,
            // CastGuardOsDeterminedFailureMode, GuardMemcpyFunctionPointer.
            let felder: &[usize] = if k.pe32plus {
                &[0x58, 0x70, 0x78, 0xd8, 0xe8, 0x118, 0x120, 0x128, 0x130, 0x138]
            } else {
                &[0x3c, 0x48, 0x4c, 0x84, 0x90, 0xac, 0xb0, 0xb4, 0xb8, 0xbc]
            };
            for &o in felder {
                if o + breite as usize > groesse {
                    continue;
                }
                let adresse = if k.pe32plus { u64_bei(b, s + o) } else { u32_bei(b, s + o).map(u64::from) };
                if let Some(a) = adresse.filter(|&a| a != 0) {
                    let ziel = a.wrapping_sub(k.image_base) as u32;
                    v.push((ziel, ziel.saturating_add(breite)));
                }
            }
        }
    }
    v
}

/// Stimmt die Datei (`datei`, die Bytes, die kopiert wuerden) mit dem
/// geladenen Bild ueberein? `basis` ist die Ladeadresse, `bild(rva, n)`
/// liefert n Bytes des geladenen Moduls ab rva (None: nicht lesbar).
/// Verglichen werden die Koepfe (bis auf ImageBase, die der Lader auf die
/// Ladeadresse setzt) und jeder nicht beschreibbare Abschnitt, nachdem die
/// Relokationen der Datei mit dem Abstand zur Ladeadresse angewandt sind;
/// ausgenommen sind nur die Stellen, die der Lader selbst schreibt
/// (lader_stellen). Beschreibbare Abschnitte veraendert das Programm beim
/// Laufen - sie werden nicht verglichen.
pub fn bild_vergleichen(datei: &[u8], basis: u64, bild: &dyn Fn(usize, usize) -> Option<Vec<u8>>) -> Result<(), String> {
    let k = pe_kopf(datei)?;
    let kopf = datei.get(..k.groesse_kopf).ok_or("Koepfe abgeschnitten")?;
    let geladen = bild(0, k.groesse_kopf).filter(|g| g.len() == k.groesse_kopf).ok_or("Koepfe des geladenen Bildes nicht lesbar")?;
    let image_base = if k.pe32plus { k.opt + 24..k.opt + 32 } else { k.opt + 28..k.opt + 32 };
    if let Some(i) = (0..k.groesse_kopf).find(|&i| !image_base.contains(&i) && kopf[i] != geladen[i]) {
        return Err(format!("Kopf weicht bei Byte 0x{i:x} vom geladenen Bild ab"));
    }
    let abstand = basis.wrapping_sub(k.image_base);
    let relok = relokationen(datei, &k)?;
    let auslassen = lader_stellen(datei, &k);
    for a in k.abschnitte.iter().filter(|a| a.merkmale & BESCHREIBBAR == 0) {
        let n = a.laenge() as usize;
        if n == 0 {
            continue;
        }
        let von = a.roh as usize;
        let mut soll = datei.get(von..von + n).ok_or_else(|| format!("Abschnitt {} reicht ueber das Dateiende", a.name))?.to_vec();
        let mut frei = vec![false; n];
        for &(rva, typ) in &relok {
            let Some(o) = rva.checked_sub(a.va).map(|o| o as usize).filter(|&o| o < n) else { continue };
            let breite = if typ == REL_DIR64 { 8 } else { 4 };
            if o + breite > n {
                frei[o..].fill(true);
                continue;
            }
            if typ == REL_DIR64 {
                let w = u64::from_le_bytes(soll[o..o + 8].try_into().unwrap()).wrapping_add(abstand);
                soll[o..o + 8].copy_from_slice(&w.to_le_bytes());
            } else {
                let w = u32::from_le_bytes(soll[o..o + 4].try_into().unwrap()).wrapping_add(abstand as u32);
                soll[o..o + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        let ende = a.va.saturating_add(n as u32);
        for &(von, bis) in &auslassen {
            let (von, bis) = (von.max(a.va), bis.min(ende));
            if von < bis {
                frei[(von - a.va) as usize..(bis - a.va) as usize].fill(true);
            }
        }
        let ist = bild(a.va as usize, n).filter(|i| i.len() == n).ok_or_else(|| format!("Abschnitt {} des geladenen Bildes nicht lesbar", a.name))?;
        if let Some(i) = (0..n).find(|&i| !frei[i] && soll[i] != ist[i]) {
            return Err(format!("Abschnitt {} weicht bei RVA 0x{:x} vom geladenen Bild ab", a.name, a.va as usize + i));
        }
    }
    Ok(())
}

/// Darf "Mit Windows starten" unter diesem Konto eingeschaltet werden?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Konto {
    Ok,
    /// Der Prozess gehoert einem anderen Konto als die angemeldete Sitzung -
    /// ein Standardkonto hat an der UAC die Zugangsdaten eines Administrators
    /// eingegeben. Die Aufgabe liefe bei der Anmeldung des ANDEREN Kontos.
    AnderesKonto { prozess: String, sitzung: String },
    /// Das Konto ist kein Administrator (nicht erhoeht, kein geteiltes
    /// Token): eine Aufgabe mit hoechsten Rechten bekaeme nur ein
    /// eingeschraenktes Token, und die exe mit requireAdministrator startete
    /// nicht.
    KeinAdmin(String),
}

impl Konto {
    /// Der Grund fuers Protokoll, wenn gesperrt.
    pub fn grund(&self) -> Option<String> {
        match self {
            Konto::Ok => None,
            Konto::AnderesKonto { prozess, sitzung } => Some(format!(
                "die App laeuft als {prozess}, angemeldet ist {sitzung} (Standardkonto mit den Zugangsdaten eines Administrators?) - eine Aufgabe bei der Anmeldung liefe nicht fuer {sitzung}"
            )),
            Konto::KeinAdmin(k) => Some(format!("{k} ist kein Administrator - eine Aufgabe mit hoechsten Rechten startet die exe (requireAdministrator) nicht still")),
        }
    }

    pub fn gesperrt(&self) -> bool {
        *self != Konto::Ok
    }
}

/// Die Entscheidung zum Konto. `prozess`: SAM-Name des Prozesses
/// (DOMAENE\name); `sitzung`: der der angemeldeten Sitzung, None wenn
/// unbekannt (keine interaktive Sitzung, etwa ueber SSH - dann wird nicht
/// gesperrt); `erhoeht`: Token erhoeht; `geteilt`: Token eines
/// Administrators mit UAC (TokenElevationType Full oder Limited).
pub fn konto_entscheiden(prozess: &str, sitzung: Option<&str>, erhoeht: bool, geteilt: bool) -> Konto {
    if let Some(s) = sitzung.map(str::trim).filter(|s| !s.is_empty()) {
        if !s.eq_ignore_ascii_case(prozess.trim()) {
            return Konto::AnderesKonto { prozess: prozess.to_string(), sitzung: s.to_string() };
        }
    }
    if !erhoeht && !geteilt {
        return Konto::KeinAdmin(prozess.to_string());
    }
    Konto::Ok
}

// ------------------------------------------------------------ Windows

/// Das Konto dieses Prozesses und der Sitzung, einmal bestimmt und
/// gemerkt (beides aendert sich nicht, solange der Prozess laeuft).
#[cfg(windows)]
pub fn konto() -> &'static Konto {
    static KONTO: std::sync::OnceLock<Konto> = std::sync::OnceLock::new();
    KONTO.get_or_init(|| {
        let prozess = crate::verknuepfung::aktueller_benutzer();
        let (erhoeht, geteilt) = token_stand();
        konto_entscheiden(&prozess, sitzungsnutzer().as_deref(), erhoeht, geteilt)
    })
}

/// Ist das Token erhoeht, und ist es ein geteiltes (Administrator mit UAC)?
#[cfg(windows)]
fn token_stand() -> (bool, bool) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TokenElevationType, TokenElevationTypeFull, TokenElevationTypeLimited, TOKEN_ELEVATION,
        TOKEN_ELEVATION_TYPE, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE(std::ptr::null_mut());
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return (false, false);
        }
        let mut n = 0u32;
        let mut e = TOKEN_ELEVATION::default();
        let erhoeht = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut n)
            .is_ok()
            && e.TokenIsElevated != 0;
        let mut t = TOKEN_ELEVATION_TYPE(0);
        let geteilt = GetTokenInformation(token, TokenElevationType, Some(&mut t as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32, &mut n)
            .is_ok()
            && (t == TokenElevationTypeFull || t == TokenElevationTypeLimited);
        let _ = CloseHandle(token);
        (erhoeht, geteilt)
    }
}

/// Wer in der Sitzung dieses Prozesses angemeldet ist (DOMAENE\name); None
/// ohne interaktiven Nutzer (Sitzung 0, SSH).
#[cfg(windows)]
fn sitzungsnutzer() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::RemoteDesktop::{WTSDomainName, WTSFreeMemory, WTSQuerySessionInformationW, WTSUserName, WTS_CURRENT_SESSION, WTS_INFO_CLASS};
    let lesen = |klasse: WTS_INFO_CLASS| -> Option<String> {
        unsafe {
            let mut p = PWSTR::null();
            let mut n = 0u32;
            WTSQuerySessionInformationW(None, WTS_CURRENT_SESSION, klasse, &mut p, &mut n).ok()?;
            let t = p.to_string().ok();
            WTSFreeMemory(p.0 as _);
            t
        }
    };
    let name = lesen(WTSUserName).filter(|n| !n.trim().is_empty())?;
    Some(match lesen(WTSDomainName).filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{d}\\{name}"),
        None => name,
    })
}

/// Die SID des Kontos, als das dieser Prozess laeuft ("S-1-5-21-..."), aus
/// seinem Token (TokenUser); einmal bestimmt und gemerkt. Sie benennt die
/// Aufgabe "Mit Windows starten" dieses Kontos.
#[cfg(windows)]
pub fn eigene_sid() -> Result<&'static str, String> {
    static SID: std::sync::OnceLock<Result<String, String>> = std::sync::OnceLock::new();
    SID.get_or_init(|| {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut token = HANDLE(std::ptr::null_mut());
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| format!("Token: {}", e.message()))?;
            let mut n = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut n);
            // u64: TOKEN_USER braucht Ausrichtung.
            let mut puffer = vec![0u64; (n as usize).div_ceil(8).max(1)];
            let r = GetTokenInformation(token, TokenUser, Some(puffer.as_mut_ptr() as *mut _), n, &mut n);
            let _ = CloseHandle(token);
            r.map_err(|e| format!("Konto des Tokens: {}", e.message()))?;
            let sid = (*(puffer.as_ptr() as *const TOKEN_USER)).User.Sid;
            let bytes = std::slice::from_raw_parts(sid.0 as *const u8, GetLengthSid(sid) as usize);
            sid_text(bytes).ok_or_else(|| "SID des Kontos unlesbar".to_string())
        }
    })
    .as_deref()
    .map_err(|e| e.clone())
}

/// Die SID zu einem Kontonamen, wie er im Prinzipal einer Aufgabe steht:
/// eine SID bleibt, wie sie ist; ein Name ("RECHNER\name", "name",
/// "name@domaene") wird aufgeloest (LookupAccountNameW). None, wenn das
/// Konto unbekannt ist.
#[cfg(windows)]
pub fn konto_sid(name: &str) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE};
    let name = name.trim();
    if name.len() > 4 && name[..4].eq_ignore_ascii_case("S-1-") {
        return Some(name.to_ascii_uppercase());
    }
    let n = HSTRING::from(name);
    unsafe {
        let (mut groesse, mut domaene, mut art) = (0u32, 0u32, SID_NAME_USE(0));
        let _ = LookupAccountNameW(PCWSTR::null(), &n, None, &mut groesse, None, &mut domaene, &mut art);
        if groesse == 0 {
            return None;
        }
        let mut sid = vec![0u64; (groesse as usize).div_ceil(8)];
        let mut dom = vec![0u16; domaene.max(1) as usize];
        LookupAccountNameW(PCWSTR::null(), &n, Some(PSID(sid.as_mut_ptr() as *mut _)), &mut groesse, Some(PWSTR(dom.as_mut_ptr())), &mut domaene, &mut art).ok()?;
        sid_text(std::slice::from_raw_parts(sid.as_ptr() as *const u8, groesse as usize))
    }
}

/// Ist das (als Name oder SID angegebene) Konto das dieses Prozesses?
#[cfg(windows)]
pub fn ist_eigenes_konto(name: &str) -> bool {
    match (konto_sid(name), eigene_sid()) {
        (Some(a), Ok(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// Ist der Prozess erhoeht? (Tests, die nach Program Files schreiben.)
#[cfg(all(windows, test))]
pub fn erhoeht() -> bool {
    token_stand().0
}

/// %ProgramFiles% ueber den bekannten Ordner (aus der Registrierung der
/// Maschine, nicht aus einer Umgebungsvariablen, die der Nutzer setzen kann).
#[cfg(windows)]
pub fn program_files() -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_ProgramFiles, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None).map_err(|e| format!("Program Files: {}", e.message()))?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from).map_err(|e| e.to_string())
    }
}

/// Der Installationsordner: %ProgramFiles%\QuadChroma.
#[cfg(windows)]
pub fn installationsordner() -> Result<PathBuf, String> {
    Ok(program_files()?.join(ORDNER))
}

/// Einen Ordner des Systems ueber `f` (GetSystemDirectoryW bzw.
/// GetSystemWindowsDirectoryW) lesen; ist der Puffer zu klein, liefert `f`
/// die noetige Laenge samt Abschluss. Nur ein lokaler Laufwerkspfad taugt.
#[cfg(windows)]
fn systempfad(was: &str, f: impl Fn(&mut [u16]) -> u32) -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    let mut puffer = vec![0u16; 261];
    for _ in 0..3 {
        let n = f(&mut puffer) as usize;
        if n == 0 {
            return Err(format!("{was}: {}", std::io::Error::last_os_error()));
        }
        if n < puffer.len() {
            let p = PathBuf::from(std::ffi::OsString::from_wide(&puffer[..n]));
            if !lokaler_laufwerkspfad(&p.to_string_lossy()) {
                return Err(format!("{was}: {} ist kein lokaler Pfad", p.display()));
            }
            return Ok(p);
        }
        puffer.resize(n, 0);
    }
    Err(format!("{was}: Pfad nicht lesbar"))
}

/// Der Systemordner (etwa C:\WINDOWS\system32) vom System selbst
/// (GetSystemDirectoryW) - nie aus %SystemRoot% oder %windir%: die kann
/// jedes Programm des Kontos ohne Adminrechte setzen (HKCU\Environment),
/// und die erhoehte App erbte sie, von der Aufgabe bei der Anmeldung wie
/// beim Start mit UAC-Abfrage aus dem Explorer. schtasks.exe wird von hier
/// gestartet (verknuepfung.rs).
#[cfg(windows)]
pub fn system_ordner() -> Result<PathBuf, String> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    systempfad("Systemordner", |p| unsafe { GetSystemDirectoryW(Some(p)) })
}

/// Der Windows-Ordner (etwa C:\WINDOWS) vom System selbst
/// (GetSystemWindowsDirectoryW) - der richtige Wert fuer %SystemRoot% und
/// %windir% (umgebung_absichern).
#[cfg(windows)]
pub fn windows_ordner() -> Result<PathBuf, String> {
    use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    systempfad("Windows-Ordner", |p| unsafe { GetSystemWindowsDirectoryW(Some(p)) })
}

/// Die Umgebungsvariablen, unter denen der Windows-Ordner steht.
#[cfg(windows)]
pub const WINDOWS_VARIABLEN: [&str; 2] = ["SystemRoot", "windir"];

/// Was umgebung_absichern korrigiert hat (fuer das Protokoll der ersten
/// Instanz; beim Start selbst gibt es noch keine Protokolldatei).
#[cfg(windows)]
static UMGEBUNG_KORRIGIERT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Erster Schritt in main: %SystemRoot% und %windir% dieses Prozesses auf
/// den echten Windows-Ordner setzen, falls sie davon abweichen. Beide kann
/// ein Programm des Kontos ohne Adminrechte setzen (HKCU\Environment), und
/// die erhoehte App erbte sie. Windows selbst setzt sie in Pfade ein - etwa
/// beim Laden von COM-Servern, deren Pfad als %SystemRoot%\... in der
/// Registrierung steht (die Verknuepfungen nutzen IShellLinkW) -, und
/// schtasks.exe erbte sie wiederum. Laeuft vor jedem weiteren Faden.
#[cfg(windows)]
pub fn umgebung_absichern() {
    let Ok(echt) = windows_ordner() else { return };
    let mut korrigiert = Vec::new();
    for name in WINDOWS_VARIABLEN {
        let wert = std::env::var_os(name).map(|w| w.to_string_lossy().into_owned());
        // Nur die Schreibweise anders (C:\Windows statt C:\WINDOWS): derselbe Ordner.
        if wert.as_deref().is_some_and(|w| gleicher_pfad(w, &echt.to_string_lossy())) {
            continue;
        }
        korrigiert.push(format!("{name}={}", wert.unwrap_or_else(|| "(fehlte)".into())));
        std::env::set_var(name, &echt);
    }
    if !korrigiert.is_empty() {
        let _ = UMGEBUNG_KORRIGIERT.set(format!(
            "Umgebung: {} wich vom Windows-Ordner ab - auf {} gesetzt",
            korrigiert.join(", "),
            echt.display()
        ));
    }
}

/// Die Zeile fuers Protokoll, wenn umgebung_absichern etwas korrigiert hat.
#[cfg(windows)]
pub fn umgebung_korrigiert() -> Option<&'static str> {
    UMGEBUNG_KORRIGIERT.get().map(String::as_str)
}

/// Besitzer und DACL eines Ordners bzw. einer Datei lesen (GetFileSecurityW).
#[cfg(windows)]
fn sicherheit_lesen(pfad: &Path) -> Result<(String, Option<Vec<Ace>>), String> {
    use windows::core::{BOOL, HSTRING};
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetFileSecurityW, GetLengthSid, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
        ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    let name = HSTRING::from(pfad.as_os_str());
    let info = (OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION).0;
    let fehler = |was: &str| format!("{}: {was} ({})", pfad.display(), unsafe { GetLastError() }.to_hresult().message());
    unsafe {
        let mut noetig = 0u32;
        let _ = GetFileSecurityW(&name, info, None, 0, &mut noetig);
        if noetig == 0 {
            return Err(fehler("Rechte nicht lesbar"));
        }
        // u64: der Deskriptor braucht Ausrichtung.
        let mut puffer = vec![0u64; (noetig as usize).div_ceil(8)];
        let sd = PSECURITY_DESCRIPTOR(puffer.as_mut_ptr() as *mut _);
        if !GetFileSecurityW(&name, info, Some(sd), noetig, &mut noetig).as_bool() {
            return Err(fehler("Rechte nicht lesbar"));
        }
        let mut besitzer = PSID::default();
        let mut vorgabe = BOOL(0);
        GetSecurityDescriptorOwner(sd, &mut besitzer, &mut vorgabe).map_err(|e| format!("{}: Besitzer: {}", pfad.display(), e.message()))?;
        if besitzer.0.is_null() {
            return Err(format!("{}: kein Besitzer", pfad.display()));
        }
        let sid = std::slice::from_raw_parts(besitzer.0 as *const u8, GetLengthSid(besitzer) as usize);
        let besitzer = sid_text(sid).ok_or_else(|| format!("{}: Besitzer unlesbar", pfad.display()))?;
        let (mut da, mut vorgabe) = (BOOL(0), BOOL(0));
        let mut dacl: *mut ACL = std::ptr::null_mut();
        GetSecurityDescriptorDacl(sd, &mut da, &mut dacl, &mut vorgabe).map_err(|e| format!("{}: DACL: {}", pfad.display(), e.message()))?;
        if !da.as_bool() || dacl.is_null() {
            return Ok((besitzer, None));
        }
        let mut groesse = ACL_SIZE_INFORMATION::default();
        GetAclInformation(dacl, &mut groesse as *mut _ as *mut _, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation)
            .map_err(|e| format!("{}: DACL: {}", pfad.display(), e.message()))?;
        let mut aces = Vec::new();
        for i in 0..groesse.AceCount {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            GetAce(dacl, i, &mut ace).map_err(|e| format!("{}: ACE {i}: {}", pfad.display(), e.message()))?;
            let kopf = &*(ace as *const ACE_HEADER);
            let bytes = std::slice::from_raw_parts(ace as *const u8, kopf.AceSize as usize);
            aces.push(ace_lesen(bytes).ok_or_else(|| format!("{}: ACE {i} unlesbar", pfad.display()))?);
        }
        Ok((besitzer, Some(aces)))
    }
}

/// Darf nur ein Administrator (bzw. das System) diesen Ordner bzw. diese
/// Datei aendern? Sonst Err mit dem Grund.
#[cfg(windows)]
pub fn rechte_von(pfad: &Path) -> Result<(), String> {
    let (besitzer, dacl) = sicherheit_lesen(pfad)?;
    rechte_pruefen(&besitzer, dacl.as_deref()).map_err(|e| format!("{}: {e}", pfad.display()))
}

/// Ein lokaler Pfad auf einem festen Laufwerk (kein UNC-Pfad, kein
/// Netzlaufwerk, kein Wechseldatentraeger)?
#[cfg(windows)]
fn laufwerk_pruefen(pfad: &Path) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    // GetDriveTypeW: 3 = DRIVE_FIXED (4 waere DRIVE_REMOTE, ein Netzlaufwerk).
    const DRIVE_FIXED: u32 = 3;
    let text = pfad.to_string_lossy();
    if !lokaler_laufwerkspfad(&text) {
        return Err(format!("{text}: kein lokaler Pfad (Netzwerk- oder UNC-Pfad)"));
    }
    let rest = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    let wurzel = format!("{}\\", &rest[..2]);
    let typ = unsafe { GetDriveTypeW(&HSTRING::from(wurzel.as_str())) };
    if typ != DRIVE_FIXED {
        return Err(format!("{text}: Laufwerk {wurzel} ist kein festes lokales Laufwerk (Typ {typ})"));
    }
    Ok(())
}

/// Ist der Pfad ein Verweis (Junction, symbolische Verknuepfung)?
#[cfg(windows)]
fn ist_verweis(pfad: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT
    std::fs::symlink_metadata(pfad).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// Der Installationsordner taugt: lokales festes Laufwerk, kein Verweis,
/// nur Administratoren duerfen ihn (und seinen Elternordner) aendern.
#[cfg(windows)]
pub fn ordner_pruefen(ordner: &Path) -> Result<(), String> {
    laufwerk_pruefen(ordner)?;
    if !ordner.is_dir() {
        return Err(format!("{}: kein Ordner", ordner.display()));
    }
    if ist_verweis(ordner) {
        return Err(format!("{}: ist ein Verweis (Junction) - verweigert", ordner.display()));
    }
    if let Some(eltern) = ordner.parent() {
        rechte_von(eltern)?;
    }
    rechte_von(ordner)
}

/// Die exe, auf die die Aufgabe zeigen soll: lokal, kein Verweis, Ordner
/// und Datei nur fuer Administratoren schreibbar.
#[cfg(windows)]
pub fn ziel_pruefen(exe: &Path) -> Result<(), String> {
    let ordner = exe.parent().ok_or_else(|| format!("{}: ohne Ordner", exe.display()))?;
    ordner_pruefen(ordner)?;
    if ist_verweis(exe) {
        return Err(format!("{}: ist ein Verweis - verweigert", exe.display()));
    }
    rechte_von(exe)
}

/// Eine Datei ganz lesen; waehrenddessen duerfen andere sie nur lesen (nicht
/// schreiben, nicht umbenennen oder loeschen).
#[cfg(windows)]
fn lesen_gesperrt(pfad: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let mut f = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(pfad).map_err(|e| format!("{}: {e}", pfad.display()))?;
    let mut daten = Vec::new();
    f.read_to_end(&mut daten).map_err(|e| format!("{}: {e}", pfad.display()))?;
    Ok(daten)
}

/// Dieselbe Datei? Ueber den kanonischen Pfad (Gross- und Kleinschreibung,
/// 8.3-Namen, Verweise); laesst er sich nicht bestimmen (Ziel fehlt), nie.
#[cfg(windows)]
pub fn dieselbe_datei(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase(),
        _ => false,
    }
}

/// Eine Quelle der Installation: die laufende exe oder eine geladene
/// FFmpeg-DLL, beim Start gesperrt (quellen_sperren).
#[cfg(windows)]
pub struct Quelle {
    /// Name im Installationsordner.
    pub name: &'static str,
    /// Pfad, unter dem das Modul geladen ist.
    pub pfad: PathBuf,
    /// Beim Start zum Lesen geoeffnet, fuer andere nur lesbar
    /// (FILE_SHARE_READ); bleibt offen, bis der Prozess endet.
    datei: std::fs::File,
    /// Ladeadresse des Moduls.
    basis: usize,
}

/// Die gesperrten Quellen: die DLLs, dann die exe.
#[cfg(windows)]
pub struct Quellen {
    pub dateien: Vec<Quelle>,
}

#[cfg(windows)]
impl Quelle {
    /// Den ganzen Inhalt ueber den beim Start geoeffneten Griff lesen - nie
    /// erneut ueber den Pfad.
    fn inhalt(&self) -> Result<Vec<u8>, String> {
        use std::os::windows::fs::FileExt;
        let fehler = |e: std::io::Error| format!("{}: {e}", self.pfad.display());
        let n = self.datei.metadata().map_err(fehler)?.len() as usize;
        let mut daten = vec![0u8; n];
        let mut pos = 0;
        while pos < n {
            match self.datei.seek_read(&mut daten[pos..], pos as u64).map_err(fehler)? {
                0 => return Err(format!("{}: nach {pos} von {n} Byte zu Ende", self.pfad.display())),
                k => pos += k,
            }
        }
        Ok(daten)
    }

    /// Den Inhalt lesen und mit dem geladenen Bild vergleichen
    /// (bild_vergleichen) - so wird nur kopiert, was hier wirklich laeuft.
    fn inhalt_geprueft(&self) -> Result<Vec<u8>, String> {
        let daten = self.inhalt()?;
        self.wie_geladen(&daten)?;
        Ok(daten)
    }

    /// Entsprechen `daten` dem geladenen Bild dieses Moduls?
    fn wie_geladen(&self, daten: &[u8]) -> Result<(), String> {
        // Der erste Speicherblock eines Moduls sind seine Koepfe; daraus die
        // Groesse des Bildes, hinter die nie gelesen wird.
        let kopf = unsafe { std::slice::from_raw_parts(self.basis as *const u8, 4096) };
        let groesse = pe_kopf(kopf).map_err(|e| format!("{}: geladenes Bild: {e}", self.pfad.display()))?.groesse_bild;
        let basis = self.basis;
        let bild = move |rva: usize, n: usize| -> Option<Vec<u8>> {
            (rva.checked_add(n)? <= groesse).then(|| unsafe { std::slice::from_raw_parts((basis + rva) as *const u8, n) }.to_vec())
        };
        bild_vergleichen(daten, basis as u64, &bild).map_err(|e| format!("{}: {e} - nicht die geladene Datei, nichts kopiert", self.pfad.display()))
    }
}

/// Einen Endpfad des geoeffneten Griffs (GetFinalPathNameByHandleW mit
/// `flags`: normalisiert oder wie geoeffnet, als DOS- oder NT-Pfad).
#[cfg(windows)]
pub fn endpfad(datei: &std::fs::File, flags: u32) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, GETFINALPATHNAMEBYHANDLE_FLAGS};
    let mut puffer = vec![0u16; 32768];
    let n = unsafe { GetFinalPathNameByHandleW(HANDLE(datei.as_raw_handle()), &mut puffer, GETFINALPATHNAMEBYHANDLE_FLAGS(flags)) } as usize;
    (n > 0 && n < puffer.len()).then(|| String::from_utf16_lossy(&puffer[..n]))
}

/// Ein geladenes Modul sperren: die Datei unter dem Pfad des Moduls nur
/// zum Lesen oeffnen, fuer andere nur lesbar, und pruefen, dass sie die
/// geladene ist - ihr Pfad ist der des Moduls, und die Datei hinter dem
/// geladenen Bild (GetMappedFileNameW; der Name folgt einem Umbenennen) liegt
/// unter demselben Pfad.
#[cfg(windows)]
fn modul_sperren(name: &'static str, basis: usize) -> Result<Quelle, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows::Win32::System::Threading::GetCurrentProcess;
    const FILE_SHARE_READ: u32 = 1;
    // GetFinalPathNameByHandleW: normalisiert 0, wie geoeffnet 8; DOS 0, NT 2.
    const WIE_GEOEFFNET: u32 = 8;
    const NT: u32 = 2;
    let text = |p: &[u16], n: u32| -> Option<String> { (n > 0 && (n as usize) < p.len()).then(|| String::from_utf16_lossy(&p[..n as usize])) };
    let mut puffer = vec![0u16; 32768];
    let n = unsafe { GetModuleFileNameW(Some(HMODULE(basis as *mut _)), &mut puffer) };
    let pfad = PathBuf::from(text(&puffer, n).ok_or_else(|| format!("{name}: Pfad des geladenen Moduls unbekannt"))?);
    let datei = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&pfad)
        .map_err(|e| format!("{}: nicht gesperrt ({e})", pfad.display()))?;
    let dos = [endpfad(&datei, 0), endpfad(&datei, WIE_GEOEFFNET)];
    if !dos.iter().flatten().any(|d| gleicher_pfad(d, &pfad.to_string_lossy())) {
        return Err(format!("{}: der geoeffnete Griff zeigt auf {:?}, nicht auf das geladene Modul", pfad.display(), dos[0]));
    }
    let n = unsafe { K32GetMappedFileNameW(GetCurrentProcess(), basis as *const _, &mut puffer) };
    let bild = text(&puffer, n).ok_or_else(|| format!("{}: Datei des geladenen Bildes unbekannt", pfad.display()))?;
    let nt = [endpfad(&datei, NT), endpfad(&datei, NT | WIE_GEOEFFNET)];
    if !nt.iter().flatten().any(|t| t.to_lowercase() == bild.to_lowercase()) {
        return Err(format!(
            "{}: das geladene Bild stammt aus {bild}, die Datei unter diesem Pfad ist {:?} - umbenannt und ersetzt?",
            pfad.display(),
            nt[0]
        ));
    }
    Ok(Quelle { name, pfad, datei, basis })
}

/// exe und FFmpeg-DLLs sperren. Laeuft die App aus dem Installationsordner,
/// nichts (Err mit dem Grund): kopiert wird von dort nie, und eine neuere
/// Fassung muss die laufende beiseite benennen koennen.
#[cfg(windows)]
fn sperren() -> Result<Quellen, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    if let Ok(o) = installationsordner() {
        if dieselbe_datei(&exe, &o.join(EXE)) {
            return Err(format!("die App laeuft aus {} - dort wird nichts gesperrt und nichts kopiert", o.display()));
        }
    }
    let mut dateien = Vec::new();
    for name in DLLS {
        let modul = unsafe { GetModuleHandleW(&HSTRING::from(name)) }.map_err(|_| format!("{name} ist nicht geladen"))?;
        dateien.push(modul_sperren(name, modul.0 as usize)?);
    }
    let modul = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|e| format!("exe: {}", e.message()))?;
    dateien.push(modul_sperren(EXE, modul.0 as usize)?);
    Ok(Quellen { dateien })
}

#[cfg(windows)]
static QUELLEN: std::sync::OnceLock<Result<Quellen, String>> = std::sync::OnceLock::new();

/// Beim Start, vor allem anderen (main): die laufende exe und die geladenen
/// FFmpeg-DLLs sperren - siehe oben. Das Ergebnis gilt fuer die Lebensdauer
/// des Prozesses; scheiterte es, wird nicht installiert und nicht erneuert
/// (der Grund steht dann in der Meldung dazu).
#[cfg(windows)]
pub fn quellen_sperren() {
    let _ = quellen();
}

/// Die gesperrten Quellen; Err mit dem Grund, warum nicht gesperrt ist.
#[cfg(windows)]
pub fn quellen() -> Result<&'static Quellen, String> {
    QUELLEN.get_or_init(sperren).as_ref().map_err(|e| format!("exe und DLLs nicht gesperrt: {e}"))
}

/// Die laufende exe und ihre DLLs mit der installierten Kopie in `ordner`
/// vergleichen: gleich (SHA-256 von exe und DLLs) und die Dateiversionen
/// beider exe. Die Quellen werden ueber die gesperrten Griffe gelesen.
#[cfg(windows)]
pub fn vergleichen(ordner: &Path) -> Result<Vergleich, String> {
    let q = quellen()?;
    let mut v = Vergleich { gleich: true, laufend: None, installiert: None };
    for quelle in &q.dateien {
        let daten = quelle.inhalt()?;
        let ziel = ordner.join(quelle.name);
        let installiert = if ziel.is_file() { Some(std::fs::read(&ziel).map_err(|e| format!("{}: {e}", ziel.display()))?) } else { None };
        if quelle.name == EXE {
            v.laufend = pe_version(&daten);
            v.installiert = installiert.as_deref().and_then(pe_version);
        }
        v.gleich &= installiert.is_some_and(|i| sha256_hex(&i) == sha256_hex(&daten));
    }
    Ok(v)
}

/// Was `installieren` tat: je Datei Name und SHA-256; die beiseite gelegten
/// alten Dateien, die erst beim Neustart verschwinden (die alte exe lief).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg(windows)]
pub struct Bericht {
    pub dateien: Vec<(String, String)>,
    pub beim_neustart: Vec<PathBuf>,
}

/// Uebrig gebliebene Zwischen- und alte Dateien frueherer Laeufe loeschen
/// (was noch in Gebrauch ist, bleibt).
#[cfg(windows)]
fn reste_loeschen(ordner: &Path) {
    let Ok(eintraege) = std::fs::read_dir(ordner) else { return };
    for e in eintraege.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.contains(".neu-") || n.contains(".alt-") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Eine Datei beim naechsten Neustart loeschen (nur als Administrator).
#[cfg(windows)]
fn beim_neustart_loeschen(pfad: &Path) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
    unsafe { MoveFileExW(&HSTRING::from(pfad.as_os_str()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT) }
        .map_err(|e| format!("{}: {}", pfad.display(), e.message()))
}

/// Die Dateien dieser exe (exe, DLLs, Texte) nach `ordner` installieren bzw.
/// dort erneuern - siehe oben. exe und DLLs kommen aus den gesperrten
/// Griffen und muessen dem geladenen Bild entsprechen. Der Ordner wird
/// angelegt, wenn er fehlt; seine Rechte werden vorher, die jeder neuen
/// Datei vor dem Umbenennen geprueft. Ob ersetzt werden darf (Version),
/// entscheidet der Aufrufer (erneuern_entscheiden).
#[cfg(windows)]
pub fn installieren(ordner: &Path) -> Result<Bericht, String> {
    use std::io::Write;
    // 0. Erst alles lesen und pruefen, bevor der Zielordner angefasst wird:
    // die Texte neben der exe, dann DLLs und exe ueber die gesperrten Griffe,
    // jede mit dem geladenen Bild verglichen. Ohne Sperre nichts.
    let q = quellen()?;
    let exe_ordner = q.dateien.last().and_then(|e| e.pfad.parent()).ok_or("exe ohne Ordner")?;
    let mut liste: Vec<(String, Vec<u8>)> = Vec::new();
    for t in TEXTE {
        let p = exe_ordner.join(t);
        if p.is_file() {
            liste.push((t.to_string(), lesen_gesperrt(&p)?));
        }
    }
    for quelle in &q.dateien {
        liste.push((quelle.name.to_string(), quelle.inhalt_geprueft()?));
    }
    laufwerk_pruefen(ordner)?;
    if !ordner.exists() {
        std::fs::create_dir(ordner).map_err(|e| format!("{}: {e}", ordner.display()))?;
    }
    ordner_pruefen(ordner)?;
    reste_loeschen(ordner);
    let pid = std::process::id();
    // 1. Alles unter Zwischennamen bereitlegen und pruefen.
    let mut bereit: Vec<(String, PathBuf, String)> = Vec::new();
    let aufraeumen = |bereit: &[(String, PathBuf, String)]| {
        for (_, tmp, _) in bereit {
            let _ = std::fs::remove_file(tmp);
        }
    };
    for (name, daten) in &liste {
        let schritt = || -> Result<(PathBuf, String), String> {
            let summe = sha256_hex(daten);
            let tmp = ordner.join(format!("{name}.neu-{pid}"));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            let geschrieben = f.write_all(daten).and_then(|_| f.sync_all());
            drop(f);
            let pruefen = || -> Result<(), String> {
                geschrieben.map_err(|e| format!("{}: {e}", tmp.display()))?;
                let zurueck = sha256_hex(&std::fs::read(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?);
                if zurueck != summe {
                    return Err(format!("{}: SHA-256 nach dem Kopieren anders ({zurueck} statt {summe})", tmp.display()));
                }
                rechte_von(&tmp)
            };
            if let Err(e) = pruefen() {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            Ok((tmp, summe))
        };
        match schritt() {
            Ok((tmp, summe)) => bereit.push((name.clone(), tmp, summe)),
            Err(e) => {
                aufraeumen(&bereit);
                return Err(e);
            }
        }
    }
    // 2. Umbenennen: alte Datei beiseite, neue an ihren Platz.
    let mut getauscht: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let zurueckrollen = |getauscht: &[(PathBuf, Option<PathBuf>)]| {
        for (ziel, alt) in getauscht.iter().rev() {
            let _ = std::fs::remove_file(ziel);
            if let Some(alt) = alt {
                let _ = std::fs::rename(alt, ziel);
            }
        }
    };
    for (i, (name, tmp, _)) in bereit.iter().enumerate() {
        let ziel = ordner.join(name);
        let mut alt = None;
        if ziel.exists() {
            let a = ordner.join(format!("{name}.alt-{pid}"));
            if let Err(e) = std::fs::rename(&ziel, &a) {
                zurueckrollen(&getauscht);
                aufraeumen(&bereit[i..]);
                return Err(format!("{} nicht beiseite gelegt: {e}", ziel.display()));
            }
            alt = Some(a);
        }
        if let Err(e) = std::fs::rename(tmp, &ziel) {
            if let Some(a) = &alt {
                let _ = std::fs::rename(a, &ziel);
            }
            zurueckrollen(&getauscht);
            aufraeumen(&bereit[i..]);
            return Err(format!("{} nicht an seinen Platz gebracht: {e}", ziel.display()));
        }
        getauscht.push((ziel, alt));
    }
    // 3. Die alten Dateien loeschen - eine laufende exe erst beim Neustart.
    let mut bericht = Bericht { dateien: bereit.iter().map(|(n, _, s)| (n.clone(), s.clone())).collect(), beim_neustart: Vec::new() };
    for (_, alt) in &getauscht {
        let Some(alt) = alt else { continue };
        if std::fs::remove_file(alt).is_err() {
            match beim_neustart_loeschen(alt) {
                Ok(()) => bericht.beim_neustart.push(alt.clone()),
                Err(e) => crate::protokoll::zeile(format!("Installation: {} bleibt liegen ({e})", alt.display())),
            }
        }
    }
    Ok(bericht)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SID aus Bytes: Administratoren, SYSTEM, TrustedInstaller, ein
    /// Nutzerkonto; zu kurze Bytes ergeben nichts.
    #[test]
    fn sid_aus_bytes() {
        assert_eq!(sid_text(&[1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 0x02, 0, 0]).as_deref(), Some("S-1-5-32-544"));
        assert_eq!(sid_text(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]).as_deref(), Some("S-1-5-18"));
        let ti: Vec<u8> = [vec![1u8, 6, 0, 0, 0, 0, 0, 5], [80u32, 956008885, 3418522649, 1831038044, 1853292631, 2271478464].iter().flat_map(|s| s.to_le_bytes()).collect()]
            .concat();
        assert_eq!(sid_text(&ti).as_deref(), Some(VERTRAUT[2]));
        assert_eq!(sid_text(&[1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]).as_deref(), Some("S-1-1-0"));
        assert_eq!(sid_text(&[1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0]), None);
        assert_eq!(sid_text(&[1]), None);
    }

    /// Ein ACE aus Bytes (ACCESS_ALLOWED_ACE: Kopf, Maske, SID).
    fn ace_bytes(typ: u8, flags: u8, maske: u32, sid: &[u8]) -> Vec<u8> {
        let groesse = (8 + sid.len()) as u16;
        let mut b = vec![typ, flags];
        b.extend_from_slice(&groesse.to_le_bytes());
        b.extend_from_slice(&maske.to_le_bytes());
        b.extend_from_slice(sid);
        b
    }

    const BENUTZER: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x21, 0x02, 0, 0];

    #[test]
    fn ace_aus_bytes() {
        let a = ace_lesen(&ace_bytes(0, 0x03, 0x001f_01ff, &BENUTZER)).unwrap();
        assert_eq!(a, Ace { typ: 0, flags: 3, maske: 0x001f_01ff, sid: "S-1-5-32-545".into() });
        // Die Groesse im Kopf zaehlt, nicht die Laenge des Puffers.
        let mut b = ace_bytes(1, 0, 0x2, &BENUTZER);
        b.extend_from_slice(&[9, 9, 9]);
        assert_eq!(ace_lesen(&b).unwrap().sid, "S-1-5-32-545");
        // Objekt-Eintrag: ohne SID an fester Stelle.
        assert_eq!(ace_lesen(&ace_bytes(5, 0, 0x2, &BENUTZER)).unwrap().sid, "");
        assert_eq!(ace_lesen(&[0, 0, 4]), None);
        assert_eq!(ace_lesen(&ace_bytes(0, 0, 1, &BENUTZER)[..10]), None);
    }

    fn ace(typ: u8, flags: u8, maske: u32, sid: &str) -> Ace {
        Ace { typ, flags, maske, sid: sid.into() }
    }

    /// Die Rechte von Program Files\QuadChroma, wie Windows sie vererbt:
    /// erlaubt. Jedes andere Konto mit Schreib-, Loesch- oder
    /// Rechteaenderungsrechten, ein fremder Besitzer oder keine DACL: nein.
    #[test]
    fn rechte_nur_administratoren() {
        let vererbt = vec![
            ace(0, 0x10, 0x001f_01ff, "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"),
            ace(0, 0x13, 0x001f_01ff, "S-1-5-18"),
            ace(0, 0x13, 0x001f_01ff, "S-1-5-32-544"),
            ace(0, 0x13, 0x0012_00a9, "S-1-5-32-545"),
            ace(0, 0x1b, 0x1000_0000, "S-1-3-0"),
            ace(0, 0x13, 0x0012_00a9, "S-1-15-2-1"),
        ];
        assert_eq!(rechte_pruefen("S-1-5-32-544", Some(&vererbt)), Ok(()));
        assert_eq!(rechte_pruefen("S-1-5-18", Some(&vererbt)), Ok(()));
        // Nutzer duerfen aendern (icacls /grant Users:M): nein.
        for maske in [0x2, 0x4, 0x40, 0x0001_0000, 0x0004_0000, 0x0008_0000, 0x1000_0000, 0x4000_0000, 0x0013_01bf] {
            let mut d = vererbt.clone();
            d.push(ace(0, 0x03, maske, "S-1-5-32-545"));
            let e = rechte_pruefen("S-1-5-32-544", Some(&d)).unwrap_err();
            assert!(e.contains("S-1-5-32-545"), "{maske:x}: {e}");
        }
        // Dasselbe als Callback-Eintrag: nein; nur vererbt (IO): zaehlt nicht.
        let mut d = vererbt.clone();
        d.push(ace(9, 0, 0x2, "S-1-1-0"));
        assert!(rechte_pruefen("S-1-5-32-544", Some(&d)).is_err());
        let mut d = vererbt.clone();
        d.push(ace(0, 0x08 | 0x03, 0x001f_01ff, "S-1-1-0"));
        assert_eq!(rechte_pruefen("S-1-5-32-544", Some(&d)), Ok(()));
        // Verweigernde Eintraege und reines Lesen fuer alle: gleichgueltig.
        let mut d = vererbt.clone();
        d.push(ace(1, 0, 0x001f_01ff, "S-1-1-0"));
        d.push(ace(0, 0, 0x0012_00a9, "S-1-1-0"));
        assert_eq!(rechte_pruefen("S-1-5-32-544", Some(&d)), Ok(()));
        // Unbekannter erlaubender Typ (Objekt-Eintrag): vorsichtshalber nein.
        let mut d = vererbt.clone();
        d.push(ace(5, 0, 0x2, ""));
        assert!(rechte_pruefen("S-1-5-32-544", Some(&d)).is_err());
        // Ein Nutzer als Besitzer (darf die DACL aendern): nein.
        assert!(rechte_pruefen("S-1-5-21-1-2-3-1001", Some(&vererbt)).unwrap_err().contains("Besitzer"));
        // NULL-DACL: nein.
        assert!(rechte_pruefen("S-1-5-32-544", None).is_err());
    }

    #[test]
    fn lokale_pfade() {
        for p in ["C:\\Program Files\\QuadChroma", "c:\\x", "\\\\?\\C:\\Program Files\\QuadChroma\\quadchroma.exe", "D:/a/b"] {
            assert!(lokaler_laufwerkspfad(p), "{p}");
        }
        for p in [
            "\\\\server\\freigabe\\quadchroma.exe",
            "//server/freigabe/q.exe",
            "\\\\?\\UNC\\server\\freigabe\\q.exe",
            "\\\\.\\pipe\\x",
            "quadchroma\\q.exe",
            "C:q.exe",
            "",
        ] {
            assert!(!lokaler_laufwerkspfad(p), "{p}");
        }
    }

    #[test]
    fn pfade_vergleichen() {
        assert!(gleicher_pfad("\"C:\\Program Files\\QuadChroma\\quadchroma.exe\"", "c:\\program files\\quadchroma\\QUADCHROMA.EXE"));
        assert!(gleicher_pfad("\\\\?\\C:\\Program Files\\QuadChroma\\", "C:/Program Files/QuadChroma"));
        assert!(!gleicher_pfad("C:\\Users\\rob\\Downloads\\quadchroma.exe", "C:\\Program Files\\QuadChroma\\quadchroma.exe"));
    }

    /// Befehl und Argumente aus dem XML von schtasks /Query /XML - so, wie
    /// schtasks /Create /TR "\"<exe>\" --hintergrund" sie ablegt.
    #[test]
    fn aufgabe_aus_dem_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals><Principal id="Author"><UserId>FORK-WIN\rober</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Actions Context="Author">
    <Exec>
      <Command>"C:\Program Files\QuadChroma\quadchroma.exe"</Command>
      <Arguments>--hintergrund</Arguments>
    </Exec>
  </Actions>
</Task>"#;
        let (befehl, argumente) = aufgabe_aus_xml(xml).unwrap();
        assert_eq!(befehl, "\"C:\\Program Files\\QuadChroma\\quadchroma.exe\"");
        assert_eq!(argumente, "--hintergrund");
        let exe = Path::new("C:\\Program Files\\QuadChroma\\quadchroma.exe");
        assert!(aufgabe_zeigt_auf(&befehl, &argumente, exe));
        assert!(!aufgabe_zeigt_auf(&befehl, "", exe));
        assert!(!aufgabe_zeigt_auf("C:\\Users\\rob\\Downloads\\quadchroma-0.1.0\\quadchroma.exe", "--hintergrund", exe));
        // Entitaeten, ohne Argumente, ohne Aktion.
        let xml = "<Exec><Command>C:\\A &amp; B\\q.exe</Command></Exec>";
        assert_eq!(aufgabe_aus_xml(xml), Some(("C:\\A & B\\q.exe".into(), String::new())));
        assert_eq!(aufgabe_aus_xml("<Task></Task>"), None);
    }

    /// Elemente im XML einer Aufgabe: mit Attributen, leer (`<X/>`), nicht
    /// ein laengerer Name mit demselben Anfang, verschachtelt ueber einen
    /// Pfad, Entitaeten aufgeloest; maskieren und aufloesen sind
    /// gegenlaeufig.
    #[test]
    fn xml_elemente_und_masken() {
        let xml = "<Task><IdleSettings><Enabled>x</Enabled></IdleSettings><SettingsX>nein</SettingsX>\
                   <Settings><Priority> 4 </Priority><Leer/><Hidden /></Settings>\
                   <Principals><Principal id=\"Author\"><UserId>PC\\a &amp; b</UserId></Principal></Principals></Task>";
        assert_eq!(xml_element(xml, "Settings"), Some("<Priority> 4 </Priority><Leer/><Hidden />"));
        assert_eq!(xml_wert(xml, &["Settings", "Priority"]).as_deref(), Some("4"));
        assert_eq!(xml_wert(xml, &["Settings", "Leer"]).as_deref(), Some(""));
        assert_eq!(xml_wert(xml, &["Settings", "Hidden"]).as_deref(), Some(""));
        assert_eq!(xml_wert(xml, &["Settings", "Enabled"]), None, "Enabled steht nur in IdleSettings");
        assert_eq!(xml_wert(xml, &["Principals", "Principal", "UserId"]).as_deref(), Some("PC\\a & b"));
        assert_eq!(xml_element(xml, "Principal"), Some("<UserId>PC\\a &amp; b</UserId>"));
        assert_eq!(xml_element(xml, "Setting"), None);
        assert_eq!(xml_element("<A>ohne Ende", "A"), None);
        assert_eq!(xml_element("<A", "A"), None);
        let t = "C:\\A & B <x> \"q\" 'y' &amp;";
        assert_eq!(xml_maskieren(t), "C:\\A &amp; B &lt;x&gt; &quot;q&quot; &apos;y&apos; &amp;amp;");
        assert_eq!(xml_text(&xml_maskieren(t)), t);
        assert_eq!(xml_maskieren("QuadChroma (S-1-5-18)"), "QuadChroma (S-1-5-18)");
    }

    /// Das Konto: gleiche Sitzung und erhoeht (bzw. Administrator mit UAC)
    /// ist gut; ein anderes Konto als die Sitzung oder kein Administrator
    /// sperrt; eine unbekannte Sitzung sperrt nicht.
    #[test]
    fn konto_je_fall() {
        assert_eq!(konto_entscheiden("PC\\rob", Some("PC\\rob"), true, true), Konto::Ok);
        assert_eq!(konto_entscheiden("PC\\rob", Some("pc\\ROB"), true, false), Konto::Ok);
        assert_eq!(konto_entscheiden("PC\\rob", None, true, true), Konto::Ok);
        assert_eq!(konto_entscheiden("PC\\rob", Some(""), false, true), Konto::Ok);
        let k = konto_entscheiden("PC\\admin", Some("PC\\kind"), true, true);
        assert_eq!(k, Konto::AnderesKonto { prozess: "PC\\admin".into(), sitzung: "PC\\kind".into() });
        assert!(k.gesperrt() && k.grund().unwrap().contains("PC\\kind"));
        let k = konto_entscheiden("PC\\kind", Some("PC\\kind"), false, false);
        assert_eq!(k, Konto::KeinAdmin("PC\\kind".into()));
        assert!(k.gesperrt() && k.grund().unwrap().contains("kein Administrator"));
        assert!(!Konto::Ok.gesperrt() && Konto::Ok.grund().is_none());
    }

    #[test]
    fn dateinamen() {
        assert_eq!(EXE, "quadchroma.exe");
        assert_eq!(DLLS, ["avcodec-63.dll", "avutil-61.dll"]);
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    /// Konto und Zustand aus dem XML von schtasks /Query /XML: der
    /// Prinzipal (Name oder SID, eine Gruppe ist keines Kontos), und aus nur
    /// ueber <Settings><Enabled>false</Enabled> (schtasks /Change /DISABLE) -
    /// ein abgeschalteter Ausloeser zaehlt hier nicht.
    #[test]
    fn aufgabe_konto_und_zustand() {
        let xml = |principal: &str, settings: &str| {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n<Task version=\"1.2\">\r\n  <Triggers>\r\n    <LogonTrigger>\r\n      <Enabled>true</Enabled>\r\n    </LogonTrigger>\r\n  </Triggers>\r\n  <Principals>\r\n    <Principal id=\"Author\">\r\n      {principal}\r\n      <LogonType>InteractiveToken</LogonType>\r\n      <RunLevel>HighestAvailable</RunLevel>\r\n    </Principal>\r\n  </Principals>\r\n  <Settings>\r\n    <IdleSettings>\r\n      <StopOnIdleEnd>true</StopOnIdleEnd>\r\n    </IdleSettings>\r\n{settings}  </Settings>\r\n</Task>"
            )
        };
        let x = xml("<UserId>FORK-WIN\\rober</UserId>", "    <Enabled>true</Enabled>\r\n");
        assert_eq!(aufgabe_konto(&x).as_deref(), Some("FORK-WIN\\rober"));
        assert!(aufgabe_aktiv(&x));
        let x = xml("<UserId>S-1-5-21-1-2-3-1001</UserId>", "    <Enabled>false</Enabled>\r\n");
        assert_eq!(aufgabe_konto(&x).as_deref(), Some("S-1-5-21-1-2-3-1001"));
        assert!(!aufgabe_aktiv(&x));
        // Ohne Enabled in Settings: an, auch wenn der Ausloeser aus ist.
        let x = xml("<UserId>PC\\a &amp; b</UserId>", "").replace("<Enabled>true</Enabled>", "<Enabled>false</Enabled>");
        assert!(aufgabe_aktiv(&x));
        assert_eq!(aufgabe_konto(&x).as_deref(), Some("PC\\a & b"));
        // Eine Gruppe als Prinzipal, kein Prinzipal: kein Konto.
        assert_eq!(aufgabe_konto(&xml("<GroupId>S-1-5-32-544</GroupId>", "")), None);
        assert_eq!(aufgabe_konto("<Task></Task>"), None);
        assert!(aufgabe_aktiv("<Task></Task>"));
    }

    fn p16(b: &mut [u8], i: usize, v: u16) {
        b[i..i + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn p32(b: &mut [u8], i: usize, v: u32) {
        b[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn p64(b: &mut [u8], i: usize, v: u64) {
        b[i..i + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Bevorzugte Ladeadresse der Test-PE.
    const TEST_BASIS: u64 = 0x1_4000_0000;
    /// Der optionale Kopf der Test-PE.
    const TEST_OPT: usize = 0x98;

    /// Eine kleine PE32+-Datei wie vom Linker: .text (Code, bei RVA 0x1010
    /// eine 64-Bit-Adresse mit Relokation, bei 0x1040 die Importadressen),
    /// .data (beschreibbar), .rsrc (mit Versionsressource, wenn `version`),
    /// .reloc.
    fn test_pe(version: Option<Version>) -> Vec<u8> {
        let mut b = vec![0u8; 0xC00];
        b[..2].copy_from_slice(b"MZ");
        p32(&mut b, 0x3c, 0x80);
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        p16(&mut b, 0x84, 0x8664);
        p16(&mut b, 0x86, 4);
        p32(&mut b, 0x88, 0x6543_2100);
        p16(&mut b, 0x94, 240);
        p16(&mut b, 0x96, 0x22);
        let o = TEST_OPT;
        p16(&mut b, o, 0x20b);
        p64(&mut b, o + 24, TEST_BASIS);
        p32(&mut b, o + 32, 0x1000);
        p32(&mut b, o + 36, 0x200);
        p32(&mut b, o + 56, 0x5000);
        p32(&mut b, o + 60, 0x400);
        p32(&mut b, o + 108, 16);
        let vz = |b: &mut [u8], i: usize, rva: u32, n: u32| {
            p32(b, o + 112 + 8 * i, rva);
            p32(b, o + 116 + 8 * i, n);
        };
        vz(&mut b, VZ_RELOKATIONEN, 0x4000, 12);
        vz(&mut b, VZ_IAT, 0x1040, 16);
        let tabelle = o + 240;
        for (i, (name, va, roh, merkmale)) in
            [(".text", 0x1000, 0x400, 0x6000_0020), (".data", 0x2000, 0x600, 0xC000_0040), (".rsrc", 0x3000, 0x800, 0x4000_0040), (".reloc", 0x4000, 0xA00, 0x4200_0040)]
                .into_iter()
                .enumerate()
        {
            let s = tabelle + 40 * i;
            b[s..s + name.len()].copy_from_slice(name.as_bytes());
            p32(&mut b, s + 8, if name == ".reloc" { 12 } else { 0x100 });
            p32(&mut b, s + 12, va);
            p32(&mut b, s + 16, 0x200);
            p32(&mut b, s + 20, roh);
            p32(&mut b, s + 36, merkmale);
        }
        for i in 0..0x100 {
            b[0x400 + i] = (i as u8).wrapping_mul(7);
        }
        p64(&mut b, 0x410, TEST_BASIS + 0x2000);
        p64(&mut b, 0x440, 0x1080);
        p64(&mut b, 0x448, 0);
        b[0x600..0x610].copy_from_slice(b"Anfangswerte....");
        p32(&mut b, 0xA00, 0x1000);
        p32(&mut b, 0xA04, 12);
        p16(&mut b, 0xA08, (REL_DIR64 << 12) | 0x10);
        if let Some(v) = version {
            // Typ 16 -> Name 1 -> Sprache 0x409 -> Daten bei RVA 0x3058.
            vz(&mut b, VZ_RESSOURCEN, 0x3000, 0x58 + 92);
            let r = 0x800;
            for (ebene, id, weiter) in [(0x00, 16, 0x8000_0018), (0x18, 1, 0x8000_0030), (0x30, 0x409, 0x48)] {
                p16(&mut b, r + ebene + 14, 1);
                p32(&mut b, r + ebene + 16, id);
                p32(&mut b, r + ebene + 20, weiter);
            }
            p32(&mut b, r + 0x48, 0x3058);
            p32(&mut b, r + 0x4c, 92);
            let s = r + 0x58;
            p16(&mut b, s, 92);
            p16(&mut b, s + 2, 52);
            for (i, c) in "VS_VERSION_INFO".encode_utf16().enumerate() {
                p16(&mut b, s + 6 + 2 * i, c);
            }
            p32(&mut b, s + 40, FESTE_VERSION);
            p32(&mut b, s + 44, 0x0001_0000);
            p32(&mut b, s + 48, (u32::from(v[0]) << 16) | u32::from(v[1]));
            p32(&mut b, s + 52, (u32::from(v[2]) << 16) | u32::from(v[3]));
        }
        b
    }

    /// So legt der Lader die Test-PE an `basis` ins Bild: Koepfe (ImageBase
    /// wird die Ladeadresse), Abschnitte an ihre RVA, Relokationen
    /// angewandt, Importadressen geschrieben - und das Programm hat .data
    /// schon veraendert.
    fn laden(datei: &[u8], basis: u64) -> Vec<u8> {
        let k = pe_kopf(datei).unwrap();
        let mut bild = vec![0u8; k.groesse_bild];
        bild[..k.groesse_kopf].copy_from_slice(&datei[..k.groesse_kopf]);
        p64(&mut bild, TEST_OPT + 24, basis);
        for a in &k.abschnitte {
            let (va, roh, n) = (a.va as usize, a.roh as usize, a.laenge() as usize);
            bild[va..va + n].copy_from_slice(&datei[roh..roh + n]);
        }
        for (rva, _) in relokationen(datei, &k).unwrap() {
            let r = rva as usize;
            let w = u64_bei(&bild, r).unwrap().wrapping_add(basis.wrapping_sub(TEST_BASIS));
            p64(&mut bild, r, w);
        }
        p64(&mut bild, 0x1040, 0x7ffa_1234_5678);
        bild[0x2000..0x2004].copy_from_slice(b"neu!");
        bild
    }

    fn leser(bild: &[u8]) -> impl Fn(usize, usize) -> Option<Vec<u8>> + '_ {
        move |rva, n| bild.get(rva..rva.checked_add(n)?).map(<[u8]>::to_vec)
    }

    /// Kopf, Abschnitte und Dateiversion einer PE-Datei aus Bytes; ohne
    /// Versionsressource, mit falscher Signatur oder aus Unsinn keine.
    #[test]
    fn pe_kopf_und_version() {
        let d = test_pe(Some([0, 1, 0, 0]));
        let k = pe_kopf(&d).unwrap();
        assert!(k.pe32plus);
        assert_eq!((k.image_base, k.groesse_kopf, k.groesse_bild), (TEST_BASIS, 0x400, 0x5000));
        let namen: Vec<&str> = k.abschnitte.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(namen, [".text", ".data", ".rsrc", ".reloc"]);
        assert_eq!((k.stelle(0x1010), k.stelle(0x3058), k.stelle(0x80), k.stelle(0x5000)), (Some(0x410), Some(0x858), Some(0x80), None));
        assert_eq!(pe_version(&d), Some([0, 1, 0, 0]));
        let (stelle, _) = pe_version_stelle(&d).unwrap();
        assert_eq!(u32_bei(&d, stelle), Some(FESTE_VERSION));
        assert_eq!(pe_version(&test_pe(Some([1, 2, 65535, 4]))), Some([1, 2, 65535, 4]));
        assert_eq!(pe_version(&test_pe(None)), None);
        let mut falsch = d.clone();
        falsch[stelle] ^= 1;
        assert_eq!(pe_version(&falsch), None);
        for unsinn in [&b""[..], &b"MZ"[..], &b"MZ\0\0\0\0\0\0"[..], &d[..0x100]] {
            assert_eq!(pe_version(unsinn), None);
            assert!(pe_kopf(unsinn).is_err());
        }
        assert_eq!(version_text(&[0, 1, 0, 0]), "0.1.0.0");
    }

    /// Keine Rueckstufung: neuer ersetzt, gleich alt mit anderem Inhalt
    /// ersetzt (ein neuer Bau), aelter nie; eine fehlende bzw. unlesbare
    /// installierte Version ersetzt, eine unlesbare laufende nicht.
    #[test]
    fn keine_rueckstufung() {
        let e = |gleich, laufend, installiert| erneuern_entscheiden(&Vergleich { gleich, laufend, installiert });
        let (alt, neu) = ([0, 1, 0, 0], [0, 2, 0, 0]);
        assert_eq!(e(true, Some(alt), Some(alt)), Erneuern::Gleich);
        assert_eq!(e(false, Some(neu), Some(alt)), Erneuern::Ja);
        assert_eq!(e(false, Some(alt), Some(alt)), Erneuern::Ja);
        assert_eq!(e(false, Some(alt), Some(neu)), Erneuern::Nein { laufend: Some(alt), installiert: neu });
        assert_eq!(e(false, Some([0, 1, 9, 9]), Some([0, 2, 0, 0])), Erneuern::Nein { laufend: Some([0, 1, 9, 9]), installiert: [0, 2, 0, 0] });
        assert_eq!(e(false, Some([1, 0, 0, 0]), Some([0, 99, 0, 0])), Erneuern::Ja);
        assert_eq!(e(false, Some([0, 1, 0, 1]), Some([0, 1, 0, 0])), Erneuern::Ja);
        assert_eq!(e(false, None, Some(alt)), Erneuern::Nein { laufend: None, installiert: alt });
        assert_eq!(e(false, Some(alt), None), Erneuern::Ja);
        assert_eq!(e(false, None, None), Erneuern::Ja);
        let t = nicht_ersetzt_text(Some(alt), neu);
        assert!(t.contains("0.2.0.0") && t.contains("0.1.0.0"), "{t}");
        assert!(nicht_ersetzt_text(None, neu).contains("keine lesbare Versionsangabe"));
    }

    /// Datei und geladenes Bild: gleich an der bevorzugten und an einer
    /// anderen Ladeadresse (Relokationen), obwohl der Lader die
    /// Importadressen geschrieben und das Programm .data veraendert hat. Ein
    /// anderes Byte in Code, Ressourcen, Kopf oder an der relozierten Adresse
    /// faellt auf; eines in .data oder in den Importadressen nicht.
    #[test]
    fn datei_und_geladenes_bild() {
        let d = test_pe(Some([0, 1, 0, 0]));
        for basis in [TEST_BASIS, 0x7ff6_1230_0000] {
            let bild = laden(&d, basis);
            assert_eq!(bild_vergleichen(&d, basis, &leser(&bild)), Ok(()), "{basis:x}");
        }
        let basis = 0x7ff6_1230_0000;
        let bild = laden(&d, basis);
        for (stelle, was) in [(0x420, ".text"), (0x410, ".text"), (0x858 + 48, ".rsrc"), (0x88, "Kopf"), (0xA0A, ".reloc")] {
            let mut f = d.clone();
            f[stelle] ^= 0x40;
            let e = bild_vergleichen(&f, basis, &leser(&bild)).unwrap_err();
            assert!(e.contains(was), "0x{stelle:x}: {e}");
        }
        let mut f = d.clone();
        f[0x600] ^= 1;
        f[0x440] ^= 1;
        assert_eq!(bild_vergleichen(&f, basis, &leser(&bild)), Ok(()));
        // Ohne lesbares Bild und mit einer unbekannten Relokation: nein.
        assert!(bild_vergleichen(&d, basis, &|_, _| None).is_err());
        let mut f = d.clone();
        p16(&mut f, 0xA08, (7 << 12) | 0x10);
        assert!(bild_vergleichen(&f, basis, &leser(&bild)).unwrap_err().contains("Typ 7"));
    }

    /// Die Quellen dieses Testprogramms (Windows): exe und die geladenen
    /// FFmpeg-DLLs sind gesperrt - umbenennen und zum Schreiben oeffnen
    /// scheitern mit einer Freigabeverletzung -, jede entspricht dem
    /// geladenen Bild (auch die mit MinGW gebauten DLLs), und die exe traegt
    /// die Version aus Cargo.toml.
    #[cfg(windows)]
    #[test]
    fn quellen_gesperrt_und_wie_geladen() {
        let q = quellen().unwrap();
        let namen: Vec<&str> = q.dateien.iter().map(|d| d.name).collect();
        assert_eq!(namen, [DLLS[0], DLLS[1], EXE]);
        for d in &q.dateien {
            let daten = d.inhalt_geprueft().unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(sha256_hex(&daten), sha256_hex(&std::fs::read(&d.pfad).unwrap()), "{}", d.pfad.display());
            // Ein anderes Byte mitten im Code, in den Konstanten oder in den
            // Ressourcen faellt auf.
            let k = pe_kopf(&daten).unwrap();
            let mut geprueft = 0;
            for a in k.abschnitte.iter().filter(|a| a.merkmale & BESCHREIBBAR == 0 && a.laenge() > 64) {
                let mut falsch = daten.clone();
                falsch[(a.roh + a.laenge() / 2) as usize] ^= 0x55;
                let e = d.wie_geladen(&falsch).unwrap_err();
                // Eine andere Relokation faellt dort auf, wo sie wirkt.
                assert!(a.name == ".reloc" || e.contains(&a.name), "{}: {e}", d.pfad.display());
                geprueft += 1;
            }
            assert!(geprueft >= 2, "{}: nur {geprueft} Abschnitte verglichen", d.pfad.display());
            let weg = d.pfad.with_extension("weg");
            if std::fs::rename(&d.pfad, &weg).is_ok() {
                std::fs::rename(&weg, &d.pfad).unwrap();
                panic!("{}: liess sich trotz Sperre umbenennen", d.pfad.display());
            }
            let e = std::fs::rename(&d.pfad, &weg).unwrap_err();
            assert_eq!(e.raw_os_error(), Some(32), "{}: {e}", d.pfad.display());
            let e = std::fs::OpenOptions::new().write(true).open(&d.pfad).unwrap_err();
            assert_eq!(e.raw_os_error(), Some(32), "{}: {e}", d.pfad.display());
        }
        let zahl = |v: &str| v.parse::<u16>().unwrap();
        let v = [zahl(env!("CARGO_PKG_VERSION_MAJOR")), zahl(env!("CARGO_PKG_VERSION_MINOR")), zahl(env!("CARGO_PKG_VERSION_PATCH")), 0];
        assert_eq!(pe_version(&q.dateien[2].inhalt().unwrap()), Some(v));
    }

    /// Ein Tausch vor der Sperre faellt auf (Windows): das Testprogramm
    /// laeuft als Kopie in einem eigenen Ordner; bevor es sich sperrt, wird
    /// seine Datei beiseite benannt und eine andere an ihren Platz gelegt -
    /// die Sperre scheitert. Ohne Tausch gelingt sie, und die laufende Kopie
    /// laesst sich danach weder umbenennen noch ersetzen.
    #[cfg(windows)]
    #[test]
    fn tausch_vor_der_sperre_faellt_auf() {
        use std::process::{Command, Stdio};
        let ordner = std::env::temp_dir().join(format!("{}-tausch", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let exe = ordner.join("tausch.exe");
        let warten = |datei: &str| {
            for _ in 0..400 {
                if ordner.join(datei).exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("{datei} kam nicht");
        };
        let lauf = |tauschen: bool| -> String {
            for d in ["bereit", "los", "ergebnis", "ende", "tausch-alt.exe"] {
                let _ = std::fs::remove_file(ordner.join(d));
            }
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
            let mut kind = Command::new(&exe)
                .args(["--ignored", "--exact", "installation::tests::tausch_kind", "--test-threads=1"])
                .env("QC_TAUSCH", &ordner)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            warten("bereit");
            if tauschen {
                std::fs::rename(&exe, ordner.join("tausch-alt.exe")).unwrap();
                std::fs::write(&exe, b"MZ - eine andere Datei").unwrap();
            }
            std::fs::write(ordner.join("los"), b"").unwrap();
            warten("ergebnis");
            let r = std::fs::read_to_string(ordner.join("ergebnis")).unwrap();
            if r == "ok" {
                let e = std::fs::rename(&exe, ordner.join("x.exe")).unwrap_err();
                assert_eq!(e.raw_os_error(), Some(32), "umbenennen: {e}");
                let e = std::fs::write(&exe, b"MZ").unwrap_err();
                assert_eq!(e.raw_os_error(), Some(32), "ersetzen: {e}");
            }
            std::fs::write(ordner.join("ende"), b"").unwrap();
            let _ = kind.wait();
            r
        };
        let r = lauf(true);
        assert!(r.contains("umbenannt und ersetzt"), "{r}");
        assert_eq!(lauf(false), "ok");
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Hilfe fuer den Test oben (nur mit QC_TAUSCH): meldet sich bereit,
    /// wartet auf "los", sperrt die eigene exe, schreibt das Ergebnis und
    /// haelt die Sperre bis "ende".
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn tausch_kind() {
        use windows::core::PCWSTR;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        let Some(ordner) = std::env::var_os("QC_TAUSCH").map(PathBuf::from) else { return };
        let warten = |datei: &str| {
            for _ in 0..600 {
                if ordner.join(datei).exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        std::fs::write(ordner.join("bereit"), b"").unwrap();
        warten("los");
        let modul = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap();
        let sperre = modul_sperren(EXE, modul.0 as usize);
        let text = match &sperre {
            Ok(_) => "ok".to_string(),
            Err(e) => e.clone(),
        };
        std::fs::write(ordner.join("ergebnis.tmp"), text).unwrap();
        std::fs::rename(ordner.join("ergebnis.tmp"), ordner.join("ergebnis")).unwrap();
        warten("ende");
        drop(sperre);
    }
}
