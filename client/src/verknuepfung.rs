// Desktop-Verknuepfung je Host (nur Windows-Client).
//
// Eine .lnk-Datei, die `quadchroma.exe --verbinden "<adresse>" --id <id>`
// aufruft (Pairing v1, 9.4) - das ist der Weg "Adresse beim Start", der
// sofort im Fenster verbindet. Die ID sucht den Host im Netz, auch wenn er
// inzwischen eine andere Adresse hat; die Adresse ist der Rueckfall. Ohne
// ID (aeltere Hosts) steht wie frueher nur `"<adresse>"` da.
// Laeuft die App schon, reicht die zweite Instanz die Adresse an die erste
// weiter (einzel.rs). Angelegt wird ueber die Shell (IShellLinkW und
// IPersistFile), das Symbol ist die .ico-Datei aus logo.rs im Ablageordner.
//
// Die Bereinigung des Dateinamens und die Auswertung der Befehlszeile sind
// reine Logik und laufen auf jeder Plattform (auch in den Tests auf dem Mac).

// Auf dem Mac gibt es keine Verknuepfung: dort bleibt ein Teil ungenutzt
// (Tests laufen trotzdem auf beiden Plattformen).
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
#[cfg(windows)]
use std::path::Path;

/// Vorsatz jedes Dateinamens: "QuadChroma - <Name>.lnk".
const VORSATZ: &str = "QuadChroma - ";

/// Hoechstlaenge des Namensteils in Zeichen. Mit Vorsatz, Endung und einem
/// langen Desktop-Pfad bleibt der ganze Pfad weit unter MAX_PATH.
const NAME_MAX: usize = 64;

/// Dateiname der Verknuepfung: "QuadChroma - <Name>.lnk". Beim Namen faellt
/// ein angehaengtes ".local" weg; Zeichen, die in Windows-Dateinamen verboten
/// sind (und Steuerzeichen), werden zu "_"; Punkte und Leerzeichen am Ende
/// fallen weg (Windows schnitte sie still ab); die Laenge ist begrenzt. Ohne
/// verwertbaren Namen steht die Adresse da, mit "_" statt ":".
pub fn dateiname(name: &str, adresse: &str) -> String {
    // Punkte und Leerzeichen am Ende zuerst: ein voll qualifizierter Name
    // ("studio.local.") verloere sein ".local" sonst nicht (Integrationstest
    // 574cd3e, Befund 7).
    let mut n = name.trim().trim_end_matches(['.', ' ']);
    // `get` statt Schnitt: die letzten sechs Byte koennen mitten in einem
    // Mehrbyte-Zeichen beginnen.
    if let Some(i) = n.len().checked_sub(6) {
        if n.get(i..).is_some_and(|e| e.eq_ignore_ascii_case(".local")) {
            n = &n[..i];
        }
    }
    let mut teil = bereinigen(n);
    if teil.is_empty() {
        teil = bereinigen(adresse.trim());
    }
    if teil.is_empty() {
        teil = "Host".into();
    }
    format!("{VORSATZ}{teil}.lnk")
}

/// Ein Namensteil fuer Windows: verbotene Zeichen und Steuerzeichen zu "_",
/// hoechstens NAME_MAX Zeichen, ohne Punkte und Leerzeichen am Ende.
fn bereinigen(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if (c as u32) < 0x20 || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .take(NAME_MAX)
        .collect();
    t.trim_end_matches(['.', ' ']).trim_start().to_string()
}

/// Die Argumente der Verknuepfung: die Adresse samt Port in
/// Anfuehrungszeichen, mit ID als `--verbinden "<adresse>" --id <9 Ziffern>`.
pub fn argumente(adresse: &str, id: Option<u32>) -> String {
    // Ein Anfuehrungszeichen in der Adresse gibt es bei gueltigen Adressen
    // nicht; es wuerde die Befehlszeile zerlegen und faellt deshalb weg.
    let a = adresse.replace('"', "");
    match id {
        Some(id) => format!("--verbinden \"{a}\" --id {}", crate::zugang::id_ziffern(id)),
        None => format!("\"{a}\""),
    }
}

/// Der Text, den Windows als Kommentar der Verknuepfung zeigt: Schluessel
/// DesktopShortcutDescription mit Name ({n}) und Adresse ({m}). Windows
/// nimmt hoechstens INFOTIPSIZE (1024) Zeichen.
pub fn beschreibung(lang: &crate::strings::Lang, name: &str, adresse: &str) -> String {
    let n = if name.trim().is_empty() { adresse } else { name.trim() };
    lang.get(crate::strings::Key::DesktopShortcutDescription)
        .replace("{n}", n)
        .replace("{m}", adresse)
        .chars()
        .take(1000)
        .collect()
}

/// Was `--verknuepfung <adresse> [--name <name>] [--ordner <verzeichnis>]
/// [--id <id>]` verlangt.
#[derive(Debug, PartialEq)]
pub struct Aufruf {
    pub adresse: String,
    pub name: String,
    pub ordner: Option<PathBuf>,
    pub id: Option<u32>,
}

/// Die Befehlszeile auswerten (ohne Programmnamen). None: kein
/// --verknuepfung. Err: ein Schalter ohne Wert.
pub fn aufruf(args: &[String]) -> Option<Result<Aufruf, String>> {
    let i = args.iter().position(|a| a == "--verknuepfung")?;
    let wert = |schalter: &str| -> Result<Option<String>, String> {
        match args.iter().position(|a| a == schalter) {
            None => Ok(None),
            Some(j) => match args.get(j + 1) {
                Some(v) if !v.starts_with("--") => Ok(Some(v.clone())),
                _ => Err(format!("{schalter} ohne Wert")),
            },
        }
    };
    let adresse = match args.get(i + 1) {
        Some(v) if !v.starts_with("--") && !v.trim().is_empty() => v.trim().to_string(),
        _ => return Some(Err("--verknuepfung ohne Adresse".into())),
    };
    let name = match wert("--name") {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => return Some(Err(e)),
    };
    let ordner = match wert("--ordner") {
        Ok(v) => v.map(PathBuf::from),
        Err(e) => return Some(Err(e)),
    };
    let id = match wert("--id") {
        Ok(None) => None,
        Ok(Some(v)) => match crate::zugang::id_lesen(&v) {
            Some(id) => Some(id),
            None => return Some(Err(format!("--id {v}: keine Geraete-ID (9 Ziffern)"))),
        },
        Err(e) => return Some(Err(e)),
    };
    Some(Ok(Aufruf { adresse, name, ordner, id }))
}

/// COM fuer diesen Faden im Single-Threaded Apartment, fuer die Dauer von f.
/// Im Fensterfaden hat winit OLE (STA) schon eingerichtet: dann meldet
/// CoInitializeEx S_FALSE, und das CoUninitialize am Ende gleicht nur den
/// Zaehler aus. Ein Faden im MTA (RPC_E_CHANGED_MODE) arbeitet ohne eigenes
/// Einrichten weiter - die Shell-Verknuepfung geht auch dort. Nie MTA hier:
/// im Hauptfaden vor der Ereignisschleife braechte das winit zum Absturz.
#[cfg(windows)]
fn im_sta<R>(f: impl FnOnce() -> R) -> R {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = f();
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    r
}

/// Der Desktop des Nutzers, auch wenn er umgeleitet ist (OneDrive).
#[cfg(all(windows, not(test)))]
fn desktop() -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Desktop, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).map_err(|e| e.message())?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from).map_err(|e| e.to_string())
    }
}

/// Tests legen nie etwas auf den echten Desktop.
#[cfg(all(windows, test))]
fn desktop() -> Result<PathBuf, String> {
    Err("im Test gibt es keinen Desktop - Ordner angeben".into())
}

/// Legt die Verknuepfung an: Ziel die laufende exe mit der Adresse als
/// Argument, Arbeitsordner der Ordner der exe, Symbol die .ico im
/// Ablageordner (geschrieben bzw. erneuert; ohne sie das Symbol der exe).
/// Zielordner ist der Desktop oder `ordner`. Eine vorhandene Datei gleichen
/// Namens wird ueberschrieben - es ist derselbe Host. Gibt den Pfad zurueck.
///
/// Abweichung von der Spezifikation (3.7): zusaetzlich die Sprache, weil der
/// Kommentar der Verknuepfung (DesktopShortcutDescription) in der Sprache der
/// Oberflaeche stehen soll, und die kann im Fenster umgeschaltet sein.
#[cfg(windows)]
pub fn verknuepfung_anlegen(
    ordner: Option<&Path>,
    adresse: &str,
    id: Option<u32>,
    name: &str,
    lang: &crate::strings::Lang,
) -> Result<PathBuf, String> {
    let adresse = adresse.trim();
    if adresse.is_empty() {
        return Err("keine Adresse".into());
    }
    let ziel_ordner = match ordner {
        Some(o) => o.to_path_buf(),
        None => desktop()?,
    };
    if !ziel_ordner.is_dir() {
        return Err(format!("{}: kein Ordner", ziel_ordner.display()));
    }
    let pfad = ziel_ordner.join(dateiname(name, adresse));
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    let arbeitsordner = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    // Laesst sich die Symboldatei nicht erneuern, nimmt ico_schreiben die
    // vorhandene; gibt es keine, zeigt die Verknuepfung eben das
    // Standardsymbol der exe - kein Grund, sie nicht anzulegen.
    // Bewusst ohne Protokollzeile: der Befehlszeilenweg liefe sonst in
    // protokoll.txt und leerte das Protokoll einer laufenden App.
    let symbol = match crate::logo::ico_schreiben() {
        Ok(p) => (p, 0),
        Err(_) => (exe.clone(), 0),
    };
    let text = beschreibung(lang, name, adresse);
    im_sta(|| lnk_speichern(&pfad, &exe, &argumente(adresse, id), &arbeitsordner, &text, &symbol.0, symbol.1))?;
    Ok(pfad)
}

#[cfg(windows)]
fn lnk_speichern(
    pfad: &Path,
    exe: &Path,
    argumente: &str,
    arbeitsordner: &Path,
    beschreibung: &str,
    symbol: &Path,
    symbol_index: i32,
) -> Result<(), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let fehler = |was: &str, e: windows::core::Error| format!("{was}: {}", e.message());
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| fehler("Shell", e))?;
        link.SetPath(&HSTRING::from(exe.as_os_str())).map_err(|e| fehler("Ziel", e))?;
        link.SetArguments(&HSTRING::from(argumente)).map_err(|e| fehler("Argumente", e))?;
        link.SetWorkingDirectory(&HSTRING::from(arbeitsordner.as_os_str())).map_err(|e| fehler("Arbeitsordner", e))?;
        link.SetDescription(&HSTRING::from(beschreibung)).map_err(|e| fehler("Kommentar", e))?;
        link.SetIconLocation(&HSTRING::from(symbol.as_os_str()), symbol_index).map_err(|e| fehler("Symbol", e))?;
        let datei: IPersistFile = link.cast().map_err(|e| fehler("Shell", e))?;
        datei
            .Save(&HSTRING::from(pfad.as_os_str()), true)
            .map_err(|e| format!("{}: {}", pfad.display(), e.message()))
    }
}

// ------------------------------------------------ Mit Windows starten
//
// Die eine App startet mit der Anmeldung ueber EINE Verknuepfung im
// Autostart-Ordner des Nutzers (FOLDERID_Startup) auf dieselbe exe mit
// --hintergrund: still ins Symbol, die Freigabe wie eingestellt
// (Spezifikation Pairing v1, 10.2) - keine Registry, kein Dienst: Desktop
// Duplication braucht die Sitzung des Nutzers. Ob der Punkt einen Haken
// traegt, sagt allein, ob eine der beiden Dateien da ist. Die alte
// Verknuepfung der Host-Rolle ("QuadChroma - Freigabe.lnk", --host) laeuft
// weiter (--host startet die App im Hintergrund mit Freigabe an) und wird
// beim Start der App durch die neue ersetzt (autostart_migrieren).

/// Dateiname der Autostart-Verknuepfung.
pub const AUTOSTART_DATEI: &str = "QuadChroma.lnk";
/// Die Verknuepfung der Host-Rolle vor der einen App.
pub const AUTOSTART_ALT: &str = "QuadChroma - Freigabe.lnk";
/// Argument der Autostart-Verknuepfung.
pub const AUTOSTART_ARGUMENT: &str = "--hintergrund";

/// Der Autostart-Ordner des Nutzers.
#[cfg(all(windows, not(test)))]
fn autostart_ordner() -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Startup, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None).map_err(|e| e.message())?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from).map_err(|e| e.to_string())
    }
}

/// Tests legen nie etwas in den echten Autostart-Ordner.
#[cfg(all(windows, test))]
fn autostart_ordner() -> Result<PathBuf, String> {
    Err("im Test gibt es keinen Autostart-Ordner - Ordner angeben".into())
}

/// Pfad der Autostart-Verknuepfung (in `ordner`, sonst im Autostart-Ordner).
#[cfg(windows)]
pub fn autostart_pfad(ordner: Option<&Path>) -> Result<PathBuf, String> {
    Ok(match ordner {
        Some(o) => o.to_path_buf(),
        None => autostart_ordner()?,
    }
    .join(AUTOSTART_DATEI))
}

/// Pfad der alten Autostart-Verknuepfung der Host-Rolle.
#[cfg(windows)]
fn autostart_alt_pfad(ordner: Option<&Path>) -> Result<PathBuf, String> {
    Ok(autostart_pfad(ordner)?.with_file_name(AUTOSTART_ALT))
}

/// Startet QuadChroma mit Windows (liegt die neue oder noch die alte
/// Verknuepfung da)?
#[cfg(windows)]
pub fn autostart_an(ordner: Option<&Path>) -> bool {
    let da = |p: Result<PathBuf, String>| p.map(|p| p.is_file()).unwrap_or(false);
    da(autostart_pfad(ordner)) || da(autostart_alt_pfad(ordner))
}

/// Eine Datei loeschen; fehlt sie schon, ist das kein Fehler.
#[cfg(windows)]
fn loeschen(pfad: &Path) -> Result<(), String> {
    match std::fs::remove_file(pfad) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", pfad.display())),
    }
}

/// Die neue Verknuepfung schreiben: die laufende exe mit --hintergrund.
#[cfg(windows)]
fn autostart_schreiben(pfad: &Path, beschreibung: &str) -> Result<(), String> {
    let ziel_ordner = pfad.parent().map(Path::to_path_buf).unwrap_or_default();
    if !ziel_ordner.is_dir() {
        std::fs::create_dir_all(&ziel_ordner).map_err(|e| format!("{}: {e}", ziel_ordner.display()))?;
    }
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    let arbeitsordner = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let symbol = crate::logo::ico_schreiben().unwrap_or_else(|_| exe.clone());
    im_sta(|| lnk_speichern(pfad, &exe, AUTOSTART_ARGUMENT, &arbeitsordner, beschreibung, &symbol, 0))
}

/// "Mit Windows starten" an (die eine Verknuepfung anlegen bzw. erneuern,
/// eine alte der Host-Rolle weg) oder aus (beide loeschen; fehlen sie
/// schon, ist das kein Fehler). `beschreibung`: Kommentar der Verknuepfung.
#[cfg(windows)]
pub fn autostart_setzen(ordner: Option<&Path>, an: bool, beschreibung: &str) -> Result<(), String> {
    let pfad = autostart_pfad(ordner)?;
    let alt = autostart_alt_pfad(ordner)?;
    if !an {
        let r = loeschen(&pfad);
        return loeschen(&alt).and(r);
    }
    autostart_schreiben(&pfad, beschreibung)?;
    loeschen(&alt)
}

/// Was `autostart_migrieren` tat.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Migration {
    /// Keine alte Verknuepfung - nichts zu tun.
    Keine,
    /// Die alte ist durch die eine ersetzt.
    Ersetzt,
    /// Die alte startet ein anderes Programm (Ziel wie gelesen) - sie bleibt,
    /// und der Autostart zeigt nicht auf diese exe (etwa eine Test- oder
    /// portable Kopie).
    AndereExe(String),
}

/// Beim Start der App: liegt noch die alte Verknuepfung der Host-Rolle da
/// und startet sie DIESE exe, tritt die eine an ihre Stelle (die alte wird
/// erst geloescht, wenn die neue steht). Zeigt sie auf ein anderes
/// Programm, bleibt alles, wie es ist: eine zweite Kopie (Test, portabel)
/// soll den Autostart der installierten nicht auf sich umbiegen.
#[cfg(windows)]
pub fn autostart_migrieren(ordner: Option<&Path>, beschreibung: &str) -> Result<Migration, String> {
    let alt = autostart_alt_pfad(ordner)?;
    if !alt.is_file() {
        return Ok(Migration::Keine);
    }
    let ziel = lnk_ziel(&alt)?;
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    if !dieselbe_datei(Path::new(&ziel), &exe) {
        return Ok(Migration::AndereExe(ziel));
    }
    autostart_schreiben(&autostart_pfad(ordner)?, beschreibung)?;
    loeschen(&alt)?;
    Ok(Migration::Ersetzt)
}

/// Dieselbe Datei? Ueber den kanonischen Pfad (Gross- und Kleinschreibung,
/// 8.3-Namen, Verweise); laesst er sich nicht bestimmen (Ziel fehlt), nie.
#[cfg(windows)]
fn dieselbe_datei(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase(),
        _ => false,
    }
}

/// Das Ziel einer Verknuepfung (IShellLinkW::GetPath).
#[cfg(windows)]
fn lnk_ziel(pfad: &Path) -> Result<String, String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    im_sta(|| unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let datei: IPersistFile = link.cast().map_err(|e| e.message())?;
        datei.Load(&HSTRING::from(pfad.as_os_str()), STGM_READ).map_err(|e| format!("{}: {}", pfad.display(), e.message()))?;
        let mut ziel = vec![0u16; 1024];
        link.GetPath(&mut ziel, std::ptr::null_mut(), 0).map_err(|e| e.message())?;
        let n = ziel.iter().position(|&c| c == 0).unwrap_or(ziel.len());
        Ok(String::from_utf16_lossy(&ziel[..n]))
    })
}

/// Eine Verknuepfung zuruecklesen (Tests): Ziel, Argumente, Arbeitsordner,
/// Kommentar, Symbol und Index.
#[cfg(all(windows, test))]
pub fn lnk_lesen(pfad: &Path) -> Result<(String, String, String, String, String, i32), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    fn text(p: &[u16]) -> String {
        let n = p.iter().position(|&c| c == 0).unwrap_or(p.len());
        String::from_utf16_lossy(&p[..n])
    }
    im_sta(|| unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let datei: IPersistFile = link.cast().map_err(|e| e.message())?;
        datei.Load(&HSTRING::from(pfad.as_os_str()), STGM_READ).map_err(|e| e.message())?;
        let mut ziel = vec![0u16; 1024];
        link.GetPath(&mut ziel, std::ptr::null_mut(), 0).map_err(|e| e.message())?;
        let mut arg = vec![0u16; 1024];
        link.GetArguments(&mut arg).map_err(|e| e.message())?;
        let mut ordner = vec![0u16; 1024];
        link.GetWorkingDirectory(&mut ordner).map_err(|e| e.message())?;
        let mut kommentar = vec![0u16; 1024];
        link.GetDescription(&mut kommentar).map_err(|e| e.message())?;
        let mut symbol = vec![0u16; 1024];
        let mut index = -1i32;
        link.GetIconLocation(&mut symbol, &mut index).map_err(|e| e.message())?;
        Ok((text(&ziel), text(&arg), text(&ordner), text(&kommentar), text(&symbol), index))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dateiname_bereinigt() {
        assert_eq!(dateiname("Mac-mini-von-Robert.local", "192.168.178.194:9001"), "QuadChroma - Mac-mini-von-Robert.lnk");
        assert_eq!(dateiname("studio.LOCAL", "x"), "QuadChroma - studio.lnk");
        // Auch mit Punkt (und Leerzeichen) am Ende, wie ein voll qualifizierter Name.
        assert_eq!(dateiname("x.local.", "y"), "QuadChroma - x.lnk");
        assert_eq!(dateiname("studio.local. . ", "y"), "QuadChroma - studio.lnk");
        assert_eq!(dateiname("x.local..", "y"), "QuadChroma - x.lnk");
        // Nur ein angehaengtes .local faellt weg.
        assert_eq!(dateiname("local.host", "x"), "QuadChroma - local.host.lnk");
        // Verbotene Zeichen und Steuerzeichen werden zu "_".
        assert_eq!(dateiname("a<b>c:d\"e/f\\g|h?i*j", "x"), "QuadChroma - a_b_c_d_e_f_g_h_i_j.lnk");
        assert_eq!(dateiname("Zeile\nzwei\tdrei\u{1}", "x"), "QuadChroma - Zeile_zwei_drei_.lnk");
        // Ersatzzeichen aus from_utf8_lossy bleiben - sie sind erlaubt.
        assert_eq!(dateiname("PC-\u{fffd}", "x"), "QuadChroma - PC-\u{fffd}.lnk");
        // Punkte und Leerzeichen am Ende fallen weg.
        assert_eq!(dateiname("Buero. . ", "x"), "QuadChroma - Buero.lnk");
        // Ohne Namen die Adresse, ":" wird "_".
        assert_eq!(dateiname("", "192.168.178.194:9001"), "QuadChroma - 192.168.178.194_9001.lnk");
        assert_eq!(dateiname("  ", "[fe80::1]:9001"), "QuadChroma - [fe80__1]_9001.lnk");
        assert_eq!(dateiname(".local", "10.0.0.5:9001"), "QuadChroma - 10.0.0.5_9001.lnk");
        assert_eq!(dateiname("", ""), "QuadChroma - Host.lnk");
        // Die Laenge ist begrenzt - auch bei Zeichen ausserhalb von ASCII.
        let lang = "ä".repeat(300);
        let d = dateiname(&lang, "x");
        assert_eq!(d.chars().count(), VORSATZ.chars().count() + NAME_MAX + 4);
        // Mehrbyte-Zeichen vor ".local" zerreissen nichts.
        assert_eq!(dateiname("Bür.local", "x"), "QuadChroma - Bür.lnk");
        assert_eq!(dateiname("ü", "x"), "QuadChroma - ü.lnk");
        assert_eq!(dateiname("ääää", "x"), "QuadChroma - ääää.lnk");
    }

    #[test]
    fn argumente_mit_anfuehrungszeichen() {
        assert_eq!(argumente("192.168.178.194:9001", None), "\"192.168.178.194:9001\"");
        assert_eq!(argumente("a\"b:1", None), "\"ab:1\"");
        assert_eq!(argumente("192.168.178.194:9001", Some(5)), "--verbinden \"192.168.178.194:9001\" --id 000000005");
        assert_eq!(argumente("a\"b:1", Some(581_729_911)), "--verbinden \"ab:1\" --id 581729911");
    }

    #[test]
    fn kommentar_mit_name_und_adresse() {
        let de = crate::strings::pick("de");
        assert_eq!(beschreibung(de, "studio.local", "10.0.0.5:9001"), "QuadChroma: mit studio.local verbinden (10.0.0.5:9001)");
        assert_eq!(beschreibung(de, "", "10.0.0.5:9001"), "QuadChroma: mit 10.0.0.5:9001 verbinden (10.0.0.5:9001)");
        for l in crate::strings::all() {
            let t = beschreibung(l, "N", "A:1");
            assert!(t.contains('N') && t.contains("A:1") && !t.contains('{'), "{}: {t}", l.code);
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn befehlszeile_verknuepfung() {
        assert_eq!(aufruf(&s(&["10.0.0.5:9001"])), None);
        assert_eq!(
            aufruf(&s(&["--verknuepfung", "10.0.0.5:9001", "--name", "Test Host.local", "--ordner", "C:\\qc\\desk"])),
            Some(Ok(Aufruf {
                adresse: "10.0.0.5:9001".into(),
                name: "Test Host.local".into(),
                ordner: Some(PathBuf::from("C:\\qc\\desk")),
                id: None
            }))
        );
        assert_eq!(
            aufruf(&s(&["--ordner", "d", "--verknuepfung", "h", "--id", "581 729 911"])),
            Some(Ok(Aufruf { adresse: "h".into(), name: String::new(), ordner: Some(PathBuf::from("d")), id: Some(581_729_911) }))
        );
        assert!(matches!(aufruf(&s(&["--verknuepfung", "h", "--id", "12"])), Some(Err(_))));
        assert!(matches!(aufruf(&s(&["--verknuepfung", "h", "--id"])), Some(Err(_))));
        assert!(matches!(aufruf(&s(&["--verknuepfung"])), Some(Err(_))));
        assert!(matches!(aufruf(&s(&["--verknuepfung", "--name", "x"])), Some(Err(_))));
        assert!(matches!(aufruf(&s(&["--verknuepfung", "h", "--name"])), Some(Err(_))));
        assert!(matches!(aufruf(&s(&["--verknuepfung", "h", "--ordner", "--name", "x"])), Some(Err(_))));
    }

    /// Unter Windows: anlegen in einem eigenen Temp-Ordner und ueber
    /// IPersistFile::Load zuruecklesen - Ziel, Argumente, Arbeitsordner,
    /// Kommentar, Symbol. Ein zweites Anlegen ueberschreibt dieselbe Datei.
    #[cfg(windows)]
    #[test]
    fn lnk_anlegen_und_zuruecklesen() {
        let ordner = std::env::temp_dir().join(format!("qc-test-{}-lnk", std::process::id()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let de = crate::strings::pick("de");
        let pfad = verknuepfung_anlegen(Some(&ordner), "192.168.178.194:9001", Some(581_729_911), "Mac-mini-von-Robert.local", de).unwrap();
        assert_eq!(pfad, ordner.join("QuadChroma - Mac-mini-von-Robert.lnk"));
        let (ziel, arg, arbeit, kommentar, symbol, index) = lnk_lesen(&pfad).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(ziel.to_lowercase(), exe.display().to_string().to_lowercase());
        assert_eq!(arg, "--verbinden \"192.168.178.194:9001\" --id 581729911");
        assert_eq!(arbeit.to_lowercase(), exe.parent().unwrap().display().to_string().to_lowercase());
        assert_eq!(kommentar, "QuadChroma: mit Mac-mini-von-Robert.local verbinden (192.168.178.194:9001)");
        // Das Symbol ist die geschriebene .ico im Ablageordner (unter
        // cfg(test) ein Temp-Ordner, nie APPDATA).
        let ico = crate::einstellungen::datei_pfad("quadchroma.ico").unwrap();
        assert_eq!(symbol.to_lowercase(), ico.display().to_string().to_lowercase());
        assert_eq!(index, 0);
        assert_eq!(std::fs::read(&ico).unwrap(), crate::logo::ico(&crate::logo::ICO_GROESSEN));
        // Noch einmal, anderer Kommentar: dieselbe Datei, ueberschrieben.
        let en = crate::strings::pick("en");
        let pfad2 = verknuepfung_anlegen(Some(&ordner), "192.168.178.194:9001", None, "Mac-mini-von-Robert.local", en).unwrap();
        assert_eq!(pfad2, pfad);
        assert_eq!(lnk_lesen(&pfad).unwrap().3, "QuadChroma: connect to Mac-mini-von-Robert.local (192.168.178.194:9001)");
        assert_eq!(lnk_lesen(&pfad).unwrap().1, "\"192.168.178.194:9001\"");
        assert_eq!(std::fs::read_dir(&ordner).unwrap().count(), 1);
        // Ohne Ordner kein Desktop im Test, und ein fehlender Ordner ist ein Fehler.
        assert!(verknuepfung_anlegen(None, "h:1", None, "", de).is_err());
        assert!(verknuepfung_anlegen(Some(&ordner.join("fehlt")), "h:1", None, "", de).is_err());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// "Mit Windows starten": an legt die eine Verknuepfung auf die exe mit
    /// --hintergrund an (zweimal an ist dieselbe Datei), aus loescht sie,
    /// zweimal aus ist kein Fehler. Im Test nie im echten Autostart-Ordner.
    #[cfg(windows)]
    #[test]
    fn autostart_an_und_aus() {
        let ordner = std::env::temp_dir().join(format!("{}-autostart", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&ordner);
        assert!(!autostart_an(Some(&ordner)));
        autostart_setzen(Some(&ordner), true, "QuadChroma – Fernsteuerung").unwrap();
        assert!(autostart_an(Some(&ordner)));
        let pfad = ordner.join(AUTOSTART_DATEI);
        assert_eq!(AUTOSTART_DATEI, "QuadChroma.lnk");
        let (ziel, arg, arbeit, kommentar, _, _) = lnk_lesen(&pfad).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(ziel.to_lowercase(), exe.display().to_string().to_lowercase());
        assert_eq!(arg, "--hintergrund");
        assert_eq!(arbeit.to_lowercase(), exe.parent().unwrap().display().to_string().to_lowercase());
        assert_eq!(kommentar, "QuadChroma – Fernsteuerung");
        autostart_setzen(Some(&ordner), true, "x").unwrap();
        assert_eq!(std::fs::read_dir(&ordner).unwrap().count(), 1);
        autostart_setzen(Some(&ordner), false, "").unwrap();
        assert!(!autostart_an(Some(&ordner)));
        autostart_setzen(Some(&ordner), false, "").unwrap();
        // Ohne Ordner im Test kein Autostart-Ordner.
        assert!(autostart_pfad(None).is_err());
        assert!(!autostart_an(None));
        assert!(autostart_migrieren(None, "").is_err());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Die alte Verknuepfung startet ein anderes Programm (eine andere
    /// Kopie): sie bleibt, keine neue entsteht - der Autostart zeigt nie auf
    /// eine Test- oder portable Kopie. Zeigt sie auf diese exe, nur ueber
    /// einen anderen Schreibweg (Grossbuchstaben), wird sie ersetzt.
    #[cfg(windows)]
    #[test]
    fn autostart_alte_verknuepfung_anderer_exe() {
        let ordner = std::env::temp_dir().join(format!("{}-autostart-fremd", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let alt = ordner.join(AUTOSTART_ALT);
        let neu = ordner.join(AUTOSTART_DATEI);
        let andere = ordner.join("installiert").join("quadchroma.exe");
        std::fs::create_dir_all(andere.parent().unwrap()).unwrap();
        std::fs::write(&andere, b"MZ").unwrap();
        im_sta(|| lnk_speichern(&alt, &andere, "--host", &ordner, "QuadChroma: Diesen PC freigeben", &andere, 0)).unwrap();
        match autostart_migrieren(Some(&ordner), "x") {
            Ok(Migration::AndereExe(z)) => assert_eq!(z.to_lowercase(), andere.display().to_string().to_lowercase()),
            r => panic!("{r:?}"),
        }
        assert!(alt.is_file() && !neu.exists(), "die alte bleibt, keine neue");
        assert!(autostart_an(Some(&ordner)), "die alte zaehlt weiter als an");
        // Ziel fehlt ganz: ebenso nicht diese exe.
        std::fs::remove_file(&andere).unwrap();
        assert!(matches!(autostart_migrieren(Some(&ordner), "x"), Ok(Migration::AndereExe(_))));
        assert!(alt.is_file() && !neu.exists());
        // Diese exe, in Grossbuchstaben geschrieben: dieselbe Datei.
        let exe = std::env::current_exe().unwrap();
        let gross = std::path::PathBuf::from(exe.display().to_string().to_uppercase());
        assert!(dieselbe_datei(&gross, &exe));
        std::fs::remove_file(&alt).unwrap();
        im_sta(|| lnk_speichern(&alt, &gross, "--host", &ordner, "x", &exe, 0)).unwrap();
        assert_eq!(autostart_migrieren(Some(&ordner), "x"), Ok(Migration::Ersetzt));
        assert!(!alt.exists() && neu.is_file());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Die alte Verknuepfung der Host-Rolle ("QuadChroma - Freigabe.lnk",
    /// --host): sie zaehlt als "an"; beim Start der App tritt die eine an
    /// ihre Stelle, ein zweiter Start findet nichts mehr zu tun. "aus"
    /// loescht beide, "an" laesst nur die neue stehen.
    #[cfg(windows)]
    #[test]
    fn autostart_alte_verknuepfung() {
        let ordner = std::env::temp_dir().join(format!("{}-autostart-alt", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let alt = ordner.join(AUTOSTART_ALT);
        let neu = ordner.join(AUTOSTART_DATEI);
        let exe = std::env::current_exe().unwrap();
        let alte_anlegen = || im_sta(|| lnk_speichern(&alt, &exe, "--host", &ordner, "QuadChroma: Diesen PC freigeben", &exe, 0)).unwrap();
        assert_eq!(autostart_migrieren(Some(&ordner), "x"), Ok(Migration::Keine), "ohne alte nichts zu tun");
        alte_anlegen();
        assert!(autostart_an(Some(&ordner)), "die alte zaehlt als an");
        assert_eq!(autostart_migrieren(Some(&ordner), "QuadChroma – Fernsteuerung"), Ok(Migration::Ersetzt));
        assert!(!alt.exists() && neu.is_file());
        assert_eq!(lnk_lesen(&neu).unwrap().1, "--hintergrund");
        assert!(autostart_an(Some(&ordner)));
        assert_eq!(autostart_migrieren(Some(&ordner), "x"), Ok(Migration::Keine), "zweiter Start");
        // aus: beide weg.
        alte_anlegen();
        autostart_setzen(Some(&ordner), false, "").unwrap();
        assert!(!alt.exists() && !neu.exists() && !autostart_an(Some(&ordner)));
        // an: nur die neue.
        alte_anlegen();
        autostart_setzen(Some(&ordner), true, "x").unwrap();
        assert!(!alt.exists() && neu.is_file());
        assert_eq!(std::fs::read_dir(&ordner).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&ordner);
    }
}
