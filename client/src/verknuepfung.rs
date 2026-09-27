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

use std::path::{Path, PathBuf};

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
// Die eine App startet mit der Anmeldung des Nutzers ueber eine GEPLANTE
// AUFGABE (Aufgabenplanung, schtasks.exe), nicht mehr ueber eine Verknuepfung
// im Autostart-Ordner. Grund: mit dem Manifest requireAdministrator ist die
// exe erhoeht, und eine erhoehte exe startet Windows NICHT still aus dem
// Autostart-Ordner (sie wird blockiert oder bis zur naechsten Anmeldung mit
// Abfrage aufgeschoben). Die geplante Aufgabe laeuft "bei der Anmeldung"
// (/SC ONLOGON) mit hoechsten Rechten (/RL HIGHEST) im Anmeldetoken des
// Nutzers - erhoeht, aber OHNE UAC-Abfrage (LogonType InteractiveToken, kein
// gespeichertes Passwort). Sie laeuft in der interaktiven Sitzung NACH der
// Anmeldung (Desktop Duplication braucht die Sitzung) - kein Dienst vor der
// Anmeldung. So bietet es auch AnyDesk bzw. RustDesk an.
//
// Aktion ist die INSTALLIERTE exe in %ProgramFiles%\QuadChroma mit
// --hintergrund, nie die exe dort, wo der Nutzer das ZIP entpackt hat: dort
// koennte jedes Programm mit normalen Rechten sie austauschen und liefe dann
// bei der Anmeldung still als Administrator. Beim Einschalten installiert
// sich die (erhoehte) App deshalb dorthin (installation.rs), und die Aufgabe
// zeigt nur auf eine exe auf einem lokalen festen Laufwerk, deren Ordner und
// Datei nur Administratoren aendern duerfen. Laeuft die App schon von dort,
// entsteht nur die Aufgabe. Ausschalten loescht die Aufgabe; die
// installierte Kopie bleibt liegen.
//
// Beim Start der App (autostart_beim_start): zeigt die Aufgabe noch auf eine
// andere exe (fruehere Fassung), wird installiert und umgestellt; ist die
// laufende exe eine andere als die installierte (SHA-256), wird die
// installierte erneuert. Die alten Verknuepfungen im Autostart-Ordner
// (QuadChroma.lnk der einen App frueherer Fassungen, "QuadChroma -
// Freigabe.lnk" der noch aelteren Host-Rolle mit --host) weichen ebenso der
// Aufgabe, wenn sie diese exe starten.
//
// Ob der Punkt einen Haken traegt, sagt allein, ob die Aufgabe da ist
// (autostart_an -> schtasks /Query).
//
// Reine Logik (Aufgabenname und die Befehlszeilen von schtasks) laeuft auf
// jeder Plattform und in den Tests; die echten schtasks-Aufrufe und der Name
// des Nutzers stehen hinter cfg(windows).

/// Name der geplanten Aufgabe. Tests geben einen eindeutigen Namen mit (Ort),
/// damit sie nie die echte Aufgabe anfassen; sonst der Standard.
pub const AUTOSTART_TASK: &str = "QuadChroma";
/// Dateiname der frueheren Autostart-Verknuepfung der einen App.
pub const AUTOSTART_DATEI: &str = "QuadChroma.lnk";
/// Die Verknuepfung der Host-Rolle vor der einen App.
pub const AUTOSTART_ALT: &str = "QuadChroma - Freigabe.lnk";
/// Argument der Autostart-Aufgabe (und frueher der Verknuepfung).
pub const AUTOSTART_ARGUMENT: &str = "--hintergrund";

/// Wo "Mit Windows starten" wirkt: Name der Aufgabe und Installationsordner.
/// Tests geben beides eindeutig mit und fassen so nie die echte Aufgabe oder
/// %ProgramFiles%\QuadChroma an; None heisst jeweils der Standard.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ort<'a> {
    pub task: Option<&'a str>,
    pub ordner: Option<&'a Path>,
}

impl Ort<'static> {
    /// Die echte Aufgabe "QuadChroma" und %ProgramFiles%\QuadChroma.
    pub const STANDARD: Ort<'static> = Ort { task: None, ordner: None };
}

#[cfg(windows)]
impl Ort<'_> {
    fn task(&self) -> &str {
        task_name(self.task)
    }

    fn ordner(&self) -> Result<PathBuf, String> {
        match self.ordner {
            Some(o) => Ok(o.to_path_buf()),
            None => crate::installation::installationsordner(),
        }
    }
}

/// Der Aufgabenname: der uebergebene (Tests) oder der Standard.
fn task_name(task: Option<&str>) -> &str {
    task.unwrap_or(AUTOSTART_TASK)
}

/// Wert fuer schtasks /TR: die exe in Anfuehrungszeichen (der Pfad kann
/// Leerzeichen enthalten), dann --hintergrund. schtasks legt das als Command
/// und Arguments getrennt ab.
fn tr_wert(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_ARGUMENT}", exe.display())
}

/// Argumente fuer schtasks /Create: bei der Anmeldung (ONLOGON), hoechste
/// Rechte (HIGHEST), als der aktuelle Nutzer (`benutzer`), Aktion `tr`,
/// vorhandene ueberschreiben (/F).
fn erstellen_args(task: &str, benutzer: &str, tr: &str) -> Vec<String> {
    vec![
        "/Create".into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/RL".into(),
        "HIGHEST".into(),
        "/RU".into(),
        benutzer.into(),
        "/TR".into(),
        tr.into(),
        "/TN".into(),
        task.into(),
        "/F".into(),
    ]
}

/// Argumente fuer schtasks /Query der Aufgabe.
fn query_args(task: &str) -> Vec<String> {
    vec!["/Query".into(), "/TN".into(), task.into()]
}

/// Argumente fuer schtasks /Query der Aufgabe als XML (Befehl und Argumente
/// ihrer Aktion, Rechte, Ausloeser).
fn xml_args(task: &str) -> Vec<String> {
    vec!["/Query".into(), "/TN".into(), task.into(), "/XML".into()]
}

/// Argumente fuer schtasks /Delete der Aufgabe (ohne Rueckfrage, /F).
fn loeschen_task_args(task: &str) -> Vec<String> {
    vec!["/Delete".into(), "/TN".into(), task.into(), "/F".into()]
}

/// schtasks.exe ausfuehren (voller Pfad %SystemRoot%\System32\schtasks.exe,
/// damit kein fremdes schtasks im PATH zaehlt), ohne Konsolenfenster und mit
/// leerer Eingabe (nie eine Rueckfrage abwarten). Liefert (Erfolg, Ausgabe).
#[cfg(windows)]
fn schtasks(args: &[String]) -> Result<(bool, String), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // CREATE_NO_WINDOW: eine erhoehte GUI-App soll fuer schtasks kein
    // Konsolenfenster aufblitzen lassen.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let exe = format!("{root}\\System32\\schtasks.exe");
    let ausgabe = Command::new(&exe)
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("schtasks: {e}"))?;
    let mut text = String::from_utf8_lossy(&ausgabe.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&ausgabe.stderr));
    Ok((ausgabe.status.success(), text.trim().to_string()))
}

/// SAM-Name des aktuellen Nutzers (RECHNER\user bzw. DOMAENE\user) fuer /RU
/// und die Pruefung des Kontos (installation::konto). %USERDOMAIN% taugt
/// nicht: bei einem lokalen Konto steht dort die Arbeitsgruppe (etwa
/// WORKGROUP), nicht der Rechnername. GetUserNameEx liefert den richtigen
/// Namen; klappt es nicht, der blosse %USERNAME% (schtasks loest ihn als
/// lokales Konto auf).
#[cfg(windows)]
pub fn aktueller_benutzer() -> String {
    use windows::core::PWSTR;
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
    unsafe {
        // Erster Aufruf ohne Puffer: er scheitert und legt die noetige Laenge
        // (in Zeichen, mit Abschluss) in `laenge`.
        let mut laenge = 0u32;
        let _ = GetUserNameExW(NameSamCompatible, None, &mut laenge);
        if laenge > 0 {
            let mut puffer = vec![0u16; laenge as usize];
            if GetUserNameExW(NameSamCompatible, Some(PWSTR(puffer.as_mut_ptr())), &mut laenge) {
                return String::from_utf16_lossy(&puffer[..laenge as usize]);
            }
        }
    }
    std::env::var("USERNAME").unwrap_or_default()
}

/// Der Autostart-Ordner des Nutzers (fuer die Migration der alten
/// Verknuepfungen).
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

/// Pfad einer alten Autostart-Verknuepfung (in `ordner`, sonst im
/// Autostart-Ordner).
#[cfg(windows)]
fn autostart_lnk_pfad(ordner: Option<&Path>, datei: &str) -> Result<PathBuf, String> {
    Ok(match ordner {
        Some(o) => o.to_path_buf(),
        None => autostart_ordner()?,
    }
    .join(datei))
}

/// Startet QuadChroma mit Windows - liegt die geplante Aufgabe vor?
/// (schtasks /Query liefert 0, wenn es sie gibt, sonst 1.)
#[cfg(windows)]
pub fn autostart_an(task: Option<&str>) -> bool {
    schtasks(&query_args(task_name(task))).map(|(ok, _)| ok).unwrap_or(false)
}

/// Befehl und Argumente der Aufgabe (schtasks /Query /XML); None, wenn es
/// sie nicht gibt. Eine Aufgabe ohne lesbare Aktion ergibt leere Werte - sie
/// zeigt dann jedenfalls nicht auf die installierte exe.
#[cfg(windows)]
pub fn aufgabe_befehl(task: Option<&str>) -> Option<(String, String)> {
    let (ok, xml) = schtasks(&xml_args(task_name(task))).ok()?;
    if !ok {
        return None;
    }
    Some(crate::installation::aufgabe_aus_xml(&xml).unwrap_or_default())
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

/// Die geplante Aufgabe anlegen (bzw. mit /F erneuern): `exe` mit
/// --hintergrund, bei der Anmeldung, hoechste Rechte, als der aktuelle
/// Nutzer. `exe` muss auf einem lokalen festen Laufwerk liegen, und nur
/// Administratoren duerfen sie und ihren Ordner aendern - sonst verweigert
/// (nie ein Netzwerkpfad, nie ein Ordner des Nutzers).
#[cfg(windows)]
fn task_anlegen(task: &str, exe: &Path) -> Result<(), String> {
    crate::installation::ziel_pruefen(exe)?;
    let benutzer = aktueller_benutzer();
    if benutzer.is_empty() {
        return Err("kein aktueller Nutzer fuer /RU".into());
    }
    let (ok, ausgabe) = schtasks(&erstellen_args(task, &benutzer, &tr_wert(exe)))?;
    if ok {
        Ok(())
    } else {
        Err(format!("schtasks /Create: {ausgabe}"))
    }
}

/// Was "Mit Windows starten" beim Umschalten tat.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Umstellung {
    /// An: nach `ordner` installiert bzw. erneuert; die Aufgabe startet die
    /// exe dort.
    Installiert { ordner: PathBuf, bericht: crate::installation::Bericht },
    /// An: die App lief schon aus `ordner` - nur die Aufgabe angelegt bzw.
    /// erneuert.
    NurAufgabe(PathBuf),
    /// Aus: die Aufgabe ist geloescht. Eine installierte Kopie bleibt liegen
    /// (Some: ihr Ordner).
    Aus(Option<PathBuf>),
}

/// Einschalten: diese exe installieren (bzw. die installierte erneuern) und
/// die Aufgabe auf die installierte exe richten. Laeuft `exe` schon aus dem
/// Installationsordner, nur die Aufgabe.
#[cfg(windows)]
fn einschalten(ort: Ort, exe: &Path) -> Result<Umstellung, String> {
    let ordner = ort.ordner()?;
    let installiert = ordner.join(crate::installation::EXE);
    if dieselbe_datei(exe, &installiert) {
        task_anlegen(ort.task(), &installiert)?;
        return Ok(Umstellung::NurAufgabe(ordner));
    }
    let bericht = crate::installation::installieren(exe, &ordner)?;
    task_anlegen(ort.task(), &installiert)?;
    Ok(Umstellung::Installiert { ordner, bericht })
}

/// "Mit Windows starten" an (installieren und die geplante Aufgabe anlegen
/// bzw. erneuern) oder aus (die Aufgabe loeschen; fehlt sie schon, ist das
/// kein Fehler - schtasks /Delete meldet dann einen Fehler, den wir
/// uebergehen, wenn die Aufgabe danach wirklich weg ist). Die installierte
/// Kopie bleibt beim Ausschalten liegen.
#[cfg(windows)]
pub fn autostart_setzen(ort: Ort, an: bool) -> Result<Umstellung, String> {
    if an {
        let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
        return einschalten(ort, &exe);
    }
    let (ok, ausgabe) = schtasks(&loeschen_task_args(ort.task()))?;
    if !ok && autostart_an(ort.task) {
        return Err(format!("schtasks /Delete: {ausgabe}"));
    }
    let ordner = ort.ordner().ok().filter(|o| o.join(crate::installation::EXE).is_file());
    Ok(Umstellung::Aus(ordner))
}

/// Was `autostart_beim_start` tat.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Keine Aufgabe und keine alte Verknuepfung - nichts zu tun.
    Aus,
    /// Die Aufgabe startet die installierte exe, und die ist dieselbe wie
    /// diese (oder diese laeuft von dort).
    Aktuell,
    /// Die installierte Kopie war eine andere als diese exe - erneuert.
    Erneuert { ordner: PathBuf, bericht: crate::installation::Bericht },
    /// Die Aufgabe startete `vorher` (fruehere Fassung) - jetzt installiert
    /// und auf die installierte exe umgestellt.
    Umgestellt { vorher: String, neu: Umstellung },
    /// Eine alte Verknuepfung im Autostart-Ordner ist durch Installation und
    /// Aufgabe ersetzt.
    VerknuepfungErsetzt(Umstellung),
    /// Die einzige alte Verknuepfung startet ein anderes Programm (Ziel wie
    /// gelesen) - sie bleibt, und der Autostart zeigt nicht auf diese exe
    /// (etwa eine Test- oder portable Kopie).
    AndereExe(String),
}

/// Beim Start der App (erste Instanz, nur wenn das Konto es erlaubt):
/// - liegt noch eine alte Autostart-Verknuepfung da (QuadChroma.lnk oder
///   "QuadChroma - Freigabe.lnk") und startet sie DIESE exe, tritt die
///   Aufgabe an ihre Stelle (erst installieren und die Aufgabe anlegen, dann
///   die Verknuepfung loeschen). Zeigt die einzige vorhandene auf ein anderes
///   Programm, bleibt alles, wie es ist: eine zweite Kopie (Test, portabel)
///   soll den Autostart der installierten nicht auf sich umbiegen;
/// - zeigt die Aufgabe nicht auf die installierte exe mit --hintergrund
///   (fruehere Fassung: der entpackte Ordner), wird installiert und
///   umgestellt;
/// - zeigt sie dorthin, ist diese exe aber eine andere (SHA-256 von exe und
///   DLLs), wird die installierte Kopie erneuert.
///
/// `lnk_ordner`: wo die alten Verknuepfungen liegen (Tests), sonst der
/// Autostart-Ordner.
#[cfg(windows)]
pub fn autostart_beim_start(lnk_ordner: Option<&Path>, ort: Ort) -> Result<Start, String> {
    use crate::installation as inst;
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    let mut zu_migrieren = Vec::new();
    let mut fremd = None;
    for datei in [AUTOSTART_DATEI, AUTOSTART_ALT] {
        let pfad = autostart_lnk_pfad(lnk_ordner, datei)?;
        if !pfad.is_file() {
            continue;
        }
        let ziel = lnk_ziel(&pfad)?;
        if dieselbe_datei(Path::new(&ziel), &exe) {
            zu_migrieren.push(pfad);
        } else if fremd.is_none() {
            fremd = Some(ziel);
        }
    }
    if !zu_migrieren.is_empty() {
        // Erst installieren und die Aufgabe anlegen, dann die alten
        // Verknuepfungen loeschen.
        let neu = einschalten(ort, &exe)?;
        for pfad in zu_migrieren {
            loeschen(&pfad)?;
        }
        return Ok(Start::VerknuepfungErsetzt(neu));
    }
    let Some((befehl, argumente)) = aufgabe_befehl(ort.task) else {
        return Ok(match fremd {
            Some(ziel) => Start::AndereExe(ziel),
            None => Start::Aus,
        });
    };
    let ordner = ort.ordner()?;
    let installiert = ordner.join(inst::EXE);
    if inst::aufgabe_zeigt_auf(&befehl, &argumente, &installiert) {
        if dieselbe_datei(&exe, &installiert) || inst::stimmt_ueberein(&exe, &ordner)? {
            return Ok(Start::Aktuell);
        }
        let bericht = inst::installieren(&exe, &ordner)?;
        return Ok(Start::Erneuert { ordner, bericht });
    }
    let neu = einschalten(ort, &exe)?;
    Ok(Start::Umgestellt { vorher: format!("{befehl} {argumente}").trim().to_string(), neu })
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

    /// Reine Logik der geplanten Aufgabe (auf jeder Plattform): der Standard-
    /// bzw. der uebergebene Name, der /TR-Wert mit der exe in
    /// Anfuehrungszeichen und die vier Befehlszeilen von schtasks.
    #[test]
    fn schtasks_befehlszeilen() {
        assert_eq!(task_name(None), "QuadChroma");
        assert_eq!(AUTOSTART_TASK, "QuadChroma");
        assert_eq!(task_name(Some("QC-Test-1")), "QC-Test-1");
        assert!(Ort::STANDARD.task.is_none() && Ort::STANDARD.ordner.is_none());
        assert_eq!(
            tr_wert(Path::new("C:\\Program Files\\QuadChroma\\quadchroma.exe")),
            "\"C:\\Program Files\\QuadChroma\\quadchroma.exe\" --hintergrund"
        );
        assert_eq!(
            erstellen_args("QC", "PC\\rob", "\"c:\\a b\\q.exe\" --hintergrund"),
            vec![
                "/Create", "/SC", "ONLOGON", "/RL", "HIGHEST", "/RU", "PC\\rob", "/TR",
                "\"c:\\a b\\q.exe\" --hintergrund", "/TN", "QC", "/F"
            ]
        );
        assert_eq!(query_args("QC"), vec!["/Query", "/TN", "QC"]);
        assert_eq!(xml_args("QC"), vec!["/Query", "/TN", "QC", "/XML"]);
        assert_eq!(loeschen_task_args("QC"), vec!["/Delete", "/TN", "QC", "/F"]);
    }

    /// Ein eigener Installationsordner unter Program Files - nur dort gelten
    /// die Rechte wie im Ernstfall (nur Administratoren duerfen schreiben).
    /// None, wenn der Test nicht erhoeht laeuft: dann uebersprungen.
    #[cfg(windows)]
    fn pf_ordner(name: &str) -> Option<PathBuf> {
        if !crate::installation::erhoeht() {
            eprintln!("nicht erhoeht - Test mit Program Files uebersprungen");
            return None;
        }
        let o = crate::installation::program_files().unwrap().join(format!("{}-{name}", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&o);
        Some(o)
    }

    #[cfg(windows)]
    fn sha(p: &Path) -> String {
        crate::installation::sha256_hex(&std::fs::read(p).unwrap())
    }

    /// Zwischen- und beiseite gelegte Dateien im Ordner.
    #[cfg(windows)]
    fn reste(ordner: &Path) -> Vec<String> {
        std::fs::read_dir(ordner)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".neu-") || n.contains(".alt-"))
            .collect()
    }

    /// "Mit Windows starten" an: diese exe und die FFmpeg-DLLs liegen danach
    /// im Installationsordner mit gleichem SHA-256, die Aufgabe startet die
    /// installierte exe mit --hintergrund, hoechsten Rechten, bei der
    /// Anmeldung, als der aktuelle Nutzer. Zweimal an ist kein Fehler (/F);
    /// aus loescht die Aufgabe, die Dateien bleiben; zweimal aus ist kein
    /// Fehler. Eigene Aufgabe und eigener Ordner, nie die echten.
    #[cfg(windows)]
    #[test]
    fn autostart_an_und_aus() {
        use crate::installation::{DLLS, EXE};
        let Some(ordner) = pf_ordner("anaus") else { return };
        let task = format!("{}-anaus", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), ordner: Some(&ordner) };
        // Vor und nach dem Test aufraeumen, egal wie er ausging.
        let _ = autostart_setzen(ort, false);
        assert!(!autostart_an(ort.task), "Aufgabe schon da?");
        match autostart_setzen(ort, true).unwrap() {
            Umstellung::Installiert { ordner: o, bericht } => {
                assert_eq!(o, ordner);
                let namen: Vec<&str> = bericht.dateien.iter().map(|(n, _)| n.as_str()).collect();
                assert_eq!(namen.last(), Some(&EXE), "exe zuletzt: {namen:?}");
                for d in DLLS {
                    assert!(namen.contains(&d), "{d} fehlt: {namen:?}");
                }
                assert!(bericht.beim_neustart.is_empty(), "{bericht:?}");
            }
            u => panic!("{u:?}"),
        }
        assert!(autostart_an(ort.task), "Aufgabe nach 'an' nicht da");
        let exe = std::env::current_exe().unwrap();
        let installiert = ordner.join(EXE);
        assert_eq!(sha(&exe), sha(&installiert));
        for d in DLLS {
            assert!(ordner.join(d).is_file(), "{d}");
        }
        assert!(reste(&ordner).is_empty(), "{:?}", reste(&ordner));
        // Die Aufgabe: die installierte exe mit --hintergrund, hoechste
        // Rechte, Anmelde-Ausloeser, als der aktuelle Nutzer.
        let (befehl, argumente) = aufgabe_befehl(ort.task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert), "{befehl} {argumente}");
        let (ok, xml) = schtasks(&xml_args(&task)).unwrap();
        assert!(ok, "Query /XML: {xml}");
        assert!(xml.contains("HighestAvailable"), "kein HIGHEST: {xml}");
        assert!(xml.contains("<LogonTrigger>"), "kein Anmelde-Ausloeser: {xml}");
        let benutzer = aktueller_benutzer();
        assert!(xml.to_lowercase().contains(&benutzer.to_lowercase()), "nicht als aktueller Nutzer ({benutzer}): {xml}");
        // Zweimal an: kein Fehler, die Aufgabe bleibt genau eine.
        assert!(matches!(autostart_setzen(ort, true), Ok(Umstellung::Installiert { .. })));
        assert!(autostart_an(ort.task));
        // Aus: Aufgabe weg, die installierte Kopie bleibt.
        assert_eq!(autostart_setzen(ort, false), Ok(Umstellung::Aus(Some(ordner.clone()))));
        assert!(!autostart_an(ort.task), "Aufgabe nach 'aus' noch da");
        assert!(installiert.is_file());
        // Zweimal aus: die Aufgabe fehlt schon, das ist kein Fehler.
        assert!(autostart_setzen(ort, false).is_ok());
        assert!(!autostart_an(ort.task));
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Beim Start: ohne Aufgabe nichts zu tun; eine Aufgabe, die noch auf
    /// eine andere exe zeigt (fruehere Fassung: der entpackte Ordner), wird
    /// nach der Installation umgestellt; weicht die installierte Kopie von
    /// dieser exe ab, wird sie erneuert; stimmt alles, nichts.
    #[cfg(windows)]
    #[test]
    fn autostart_beim_start_umstellen_und_erneuern() {
        use crate::installation::EXE;
        let Some(ordner) = pf_ordner("start") else { return };
        let lnk = std::env::temp_dir().join(format!("{}-start-lnk", crate::secure::test_lauf()));
        std::fs::create_dir_all(&lnk).unwrap();
        let task = format!("{}-start", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), ordner: Some(&ordner) };
        let _ = autostart_setzen(ort, false);
        assert_eq!(autostart_beim_start(Some(&lnk), ort), Ok(Start::Aus));
        // Eine Aufgabe wie frueher: sie startet die exe dort, wo sie liegt
        // (hier dieses Testprogramm).
        let exe = std::env::current_exe().unwrap();
        let (ok, a) = schtasks(&erstellen_args(&task, &aktueller_benutzer(), &tr_wert(&exe))).unwrap();
        assert!(ok, "{a}");
        match autostart_beim_start(Some(&lnk), ort).unwrap() {
            Start::Umgestellt { vorher, neu: Umstellung::Installiert { ordner: o, .. } } => {
                let alt = vorher.trim_end_matches(AUTOSTART_ARGUMENT).trim();
                assert!(crate::installation::gleicher_pfad(alt, &exe.to_string_lossy()), "{vorher}");
                assert_eq!(o, ordner);
            }
            s => panic!("{s:?}"),
        }
        let installiert = ordner.join(EXE);
        let (befehl, argumente) = aufgabe_befehl(ort.task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert), "{befehl} {argumente}");
        // Stimmt alles: nichts zu tun.
        assert_eq!(autostart_beim_start(Some(&lnk), ort), Ok(Start::Aktuell));
        // Die installierte Kopie ist eine andere (eine andere Fassung): erneuert.
        let mut f = std::fs::OpenOptions::new().append(true).open(&installiert).unwrap();
        std::io::Write::write_all(&mut f, b"andere Fassung").unwrap();
        drop(f);
        assert_ne!(sha(&installiert), sha(&exe));
        assert!(matches!(autostart_beim_start(Some(&lnk), ort), Ok(Start::Erneuert { .. })));
        assert_eq!(sha(&installiert), sha(&exe));
        assert_eq!(autostart_beim_start(Some(&lnk), ort), Ok(Start::Aktuell));
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = std::fs::remove_dir_all(&lnk);
    }

    /// Nie ein Ordner, den andere als Administratoren aendern koennen, nie
    /// ein Netzwerkpfad: ein Ordner im Temp-Verzeichnis des Nutzers, ein
    /// Ordner unter Program Files, in dem "Benutzer" aendern duerfen, und ein
    /// UNC-Pfad werden verweigert - ohne Aufgabe und ohne kopierte exe. Auch
    /// direkt zeigt die Aufgabe nie auf eine exe im Temp-Ordner.
    #[cfg(windows)]
    #[test]
    fn autostart_verweigert_unsichere_ordner() {
        use crate::installation::EXE;
        let Some(pf) = pf_ordner("offen") else { return };
        let task = format!("{}-offen", crate::secure::test_lauf());
        let t = Some(task.as_str());
        let _ = schtasks(&loeschen_task_args(&task));
        // 1. Ein Ordner im Temp-Verzeichnis (dem Nutzer gehoerend).
        let temp = std::env::temp_dir().join(format!("{}-offen", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&temp);
        let e = autostart_setzen(Ort { task: t, ordner: Some(&temp) }, true).unwrap_err();
        assert!(e.contains("Besitzer") || e.contains("darf hier schreiben"), "{e}");
        assert!(!temp.join(EXE).exists() && !autostart_an(t), "{e}");
        // 2. Unter Program Files, aber "Benutzer" (S-1-5-32-545) duerfen aendern.
        std::fs::create_dir(&pf).unwrap();
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let icacls = std::process::Command::new(format!("{root}\\System32\\icacls.exe"))
            .arg(&pf)
            .args(["/grant", "*S-1-5-32-545:(OI)(CI)M"])
            .output()
            .unwrap();
        assert!(icacls.status.success(), "{}", String::from_utf8_lossy(&icacls.stdout));
        let e = autostart_setzen(Ort { task: t, ordner: Some(&pf) }, true).unwrap_err();
        assert!(e.contains("S-1-5-32-545"), "{e}");
        assert!(!pf.join(EXE).exists() && !autostart_an(t), "{e}");
        // 3. Ein UNC-Pfad (derselbe Rechner ueber die Admin-Freigabe).
        let text = pf.to_string_lossy().to_string();
        let unc = PathBuf::from(format!("\\\\localhost\\{}${}", &text[..1], &text[2..]));
        let e = autostart_setzen(Ort { task: t, ordner: Some(&unc) }, true).unwrap_err();
        assert!(e.contains("kein lokaler Pfad"), "{e}");
        assert!(!autostart_an(t));
        // 4. Die Aufgabe direkt auf eine exe im Temp-Ordner: verweigert.
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join(EXE), b"MZ").unwrap();
        assert!(task_anlegen(&task, &temp.join(EXE)).is_err());
        assert!(!autostart_an(t));
        let _ = std::fs::remove_dir_all(&temp);
        let _ = std::fs::remove_dir_all(&pf);
    }

    /// Laeuft die installierte exe gerade (etwa in einer anderen Sitzung),
    /// laesst sie sich nicht ueberschreiben - erneuern geht trotzdem: sie wird
    /// beiseite umbenannt (und beim Neustart geloescht), die neue liegt sofort
    /// an ihrem Platz. Beim naechsten Installieren verschwinden die Reste.
    #[cfg(windows)]
    #[test]
    fn installieren_waehrend_die_alte_laeuft() {
        use crate::installation::{installieren, EXE};
        let Some(ordner) = pf_ordner("laeuft") else { return };
        let exe = std::env::current_exe().unwrap();
        installieren(&exe, &ordner).unwrap();
        let installiert = ordner.join(EXE);
        // Die installierte Kopie (dieses Testprogramm) laufen lassen - nur
        // den wartenden Test unten.
        let mut kind = std::process::Command::new(&installiert)
            .args(["--ignored", "--exact", "verknuepfung::tests::nur_warten"])
            .env("QC_NUR_WARTEN", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        assert!(kind.try_wait().unwrap().is_none(), "die installierte Kopie laeuft nicht");
        assert!(std::fs::write(&installiert, b"MZ").is_err(), "eine laufende exe liess sich ueberschreiben?");
        let bericht = installieren(&exe, &ordner);
        let _ = kind.kill();
        let _ = kind.wait();
        let bericht = bericht.unwrap();
        assert!(
            bericht.beim_neustart.iter().any(|p| p.file_name().unwrap().to_string_lossy().starts_with("quadchroma.exe.alt-")),
            "{bericht:?}"
        );
        assert_eq!(sha(&installiert), sha(&exe));
        installieren(&exe, &ordner).unwrap();
        assert!(reste(&ordner).is_empty(), "{:?}", reste(&ordner));
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Hilfe fuer den Test oben: wartet, wenn QC_NUR_WARTEN gesetzt ist
    /// (hoechstens 60 s), sonst nichts.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn nur_warten() {
        if std::env::var_os("QC_NUR_WARTEN").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    /// Migration einer alten Verknuepfung, die ein ANDERES Programm startet
    /// (eine andere Kopie): sie bleibt, keine Aufgabe entsteht - der Autostart
    /// zeigt nie auf eine Test- oder portable Kopie. Zeigt sie auf diese exe,
    /// nur ueber einen anderen Schreibweg (Grossbuchstaben), wird sie durch
    /// Installation und geplante Aufgabe ersetzt.
    #[cfg(windows)]
    #[test]
    fn autostart_alte_verknuepfung_anderer_exe() {
        let Some(pf) = pf_ordner("fremd") else { return };
        let ordner = std::env::temp_dir().join(format!("{}-autostart-fremd", crate::secure::test_lauf()));
        let task = format!("{}-fremd", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), ordner: Some(&pf) };
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = autostart_setzen(ort, false);
        std::fs::create_dir_all(&ordner).unwrap();
        let alt = ordner.join(AUTOSTART_ALT);
        let andere = ordner.join("installiert").join("quadchroma.exe");
        std::fs::create_dir_all(andere.parent().unwrap()).unwrap();
        std::fs::write(&andere, b"MZ").unwrap();
        im_sta(|| lnk_speichern(&alt, &andere, "--host", &ordner, "QuadChroma: Diesen PC freigeben", &andere, 0)).unwrap();
        match autostart_beim_start(Some(&ordner), ort) {
            // Dieselbe Datei, egal ob lang oder als 8.3-Name geschrieben
            // (temp_dir kann "RUNNER~1" liefern, die Verknuepfung den langen Namen).
            Ok(Start::AndereExe(z)) => assert!(dieselbe_datei(Path::new(&z), &andere), "{z} / {}", andere.display()),
            r => panic!("{r:?}"),
        }
        assert!(alt.is_file(), "die alte bleibt");
        assert!(!autostart_an(ort.task) && !pf.exists(), "keine Aufgabe und keine Installation fuer eine fremde exe");
        // Ziel fehlt ganz: ebenso nicht diese exe.
        std::fs::remove_file(&andere).unwrap();
        assert!(matches!(autostart_beim_start(Some(&ordner), ort), Ok(Start::AndereExe(_))));
        assert!(alt.is_file() && !autostart_an(ort.task));
        // Diese exe, in Grossbuchstaben geschrieben: dieselbe Datei.
        let exe = std::env::current_exe().unwrap();
        let gross = std::path::PathBuf::from(exe.display().to_string().to_uppercase());
        assert!(dieselbe_datei(&gross, &exe));
        std::fs::remove_file(&alt).unwrap();
        im_sta(|| lnk_speichern(&alt, &gross, "--host", &ordner, "x", &exe, 0)).unwrap();
        assert!(matches!(autostart_beim_start(Some(&ordner), ort), Ok(Start::VerknuepfungErsetzt(Umstellung::Installiert { .. }))));
        assert!(!alt.exists() && autostart_an(ort.task), "Aufgabe angelegt, alte weg");
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = std::fs::remove_dir_all(&pf);
    }

    /// Migration beider alter Verknuepfungen (QuadChroma.lnk der einen App,
    /// "QuadChroma - Freigabe.lnk" der Host-Rolle mit --host): zeigen sie auf
    /// diese exe, treten beim Start Installation und geplante Aufgabe an ihre
    /// Stelle und die .lnk verschwindet; ein zweiter Start findet alles
    /// aktuell. "aus" loescht die Aufgabe.
    #[cfg(windows)]
    #[test]
    fn autostart_alte_verknuepfung() {
        let Some(pf) = pf_ordner("alt") else { return };
        let ordner = std::env::temp_dir().join(format!("{}-autostart-alt", crate::secure::test_lauf()));
        let task = format!("{}-alt", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), ordner: Some(&pf) };
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = autostart_setzen(ort, false);
        std::fs::create_dir_all(&ordner).unwrap();
        let exe = std::env::current_exe().unwrap();
        // Ohne alte Verknuepfung und ohne Aufgabe nichts zu tun.
        assert_eq!(autostart_beim_start(Some(&ordner), ort), Ok(Start::Aus), "ohne alte nichts zu tun");
        assert!(!autostart_an(ort.task));
        for datei in [AUTOSTART_DATEI, AUTOSTART_ALT] {
            let lnk = ordner.join(datei);
            im_sta(|| lnk_speichern(&lnk, &exe, "--hintergrund", &ordner, "QuadChroma", &exe, 0)).unwrap();
            assert!(
                matches!(autostart_beim_start(Some(&ordner), ort), Ok(Start::VerknuepfungErsetzt(Umstellung::Installiert { .. }))),
                "{datei}"
            );
            assert!(!lnk.exists() && autostart_an(ort.task), "{datei}: Aufgabe da, .lnk weg");
            // Zweiter Start: keine .lnk mehr, die Aufgabe zeigt auf die
            // installierte exe, und die stimmt.
            assert_eq!(autostart_beim_start(Some(&ordner), ort), Ok(Start::Aktuell), "{datei}: zweiter Start");
            assert!(autostart_setzen(ort, false).is_ok());
            assert!(!autostart_an(ort.task));
        }
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = std::fs::remove_dir_all(&pf);
    }
}
