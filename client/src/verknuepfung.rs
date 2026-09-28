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
// AUFGABE (Aufgabenplanung), nicht mehr ueber eine Verknuepfung im
// Autostart-Ordner. Grund: mit dem Manifest requireAdministrator ist die exe
// erhoeht, und eine erhoehte exe startet Windows NICHT still aus dem
// Autostart-Ordner (sie wird blockiert oder bis zur naechsten Anmeldung mit
// Abfrage aufgeschoben). Die geplante Aufgabe laeuft bei der Anmeldung DIESES
// Kontos (LogonTrigger mit seiner SID) mit hoechsten Rechten
// (HighestAvailable) im Anmeldetoken des Nutzers - erhoeht, aber OHNE
// UAC-Abfrage (LogonType InteractiveToken, kein gespeichertes Passwort). Sie
// laeuft in der interaktiven Sitzung NACH der Anmeldung (Desktop Duplication
// braucht die Sitzung) - kein Dienst vor der Anmeldung. So bietet es auch
// AnyDesk bzw. RustDesk an.
//
// Angelegt wird die Aufgabe ueber die COM-Schnittstelle der Aufgabenplanung
// (ITaskService -> ITaskFolder::RegisterTask) mit dem VOLLSTAENDIGEN XML
// (aufgabe_definition), nicht mehr mit schtasks /Create: dessen Standards
// passen fuer eine App, die dauernd laeuft, nicht - ein Laptop im
// Akkubetrieb startete sie nie (DisallowStartIfOnBatteries), das Abstecken
// beendete sie (StopIfGoingOnBatteries), nach 3 Tagen beendete Windows sie
// (ExecutionTimeLimit PT72H), ihre Prioritaet lag unter normal (7 statt 4,
// schlechter fuer den Encoder), der Ausloeser galt fuer JEDE Anmeldung, und
// einen Arbeitsordner gab es nicht. Das XML geht als Text an RegisterTask,
// nie ueber eine Datei: eine Datei an einem fuer den Nutzer beschreibbaren
// Ort koennte ausgetauscht werden, bevor der erhoehte Prozess sie liest.
// Gelesen wird ebenfalls ueber COM (IRegisteredTask::Xml, UTF-16 - die
// Ausgabe von schtasks stuende in der Codepage der Konsole und verloere
// Umlaute in Kontonamen).
//
// Eine Aufgabe dieses Kontos, die noch mit den alten Standards angelegt ist
// (bzw. deren Einstellungen sonst abweichen: Akku, Zeitlimit, Prioritaet,
// Konto des Ausloesers, Arbeitsordner, hoechste Rechte - abweichungen), wird
// beim Start neu registriert (reparieren; eine Zeile im Protokoll). Eine
// Aufgabe, deren Prinzipal ein anderes Konto oder SYSTEM ist, bleibt, wie
// sie ist.
//
// Eine Aufgabe je Konto: sie heisst "QuadChroma (<SID des Kontos>)"
// (task_fuer_konto), so haben zwei Administratorkonten auf einem PC je ihre
// eigene, und Haken und Menuepunkt zeigen nur die des Kontos, unter dem die
// App laeuft. Die fruehere Aufgabe "QuadChroma" fuer den ganzen Rechner wird
// beim Start uebernommen, wenn sie diesem Konto gehoert (Prinzipal aus ihrem
// XML): die Aufgabe dieses Kontos entsteht, die alte wird geloescht. Gehoert
// sie einem anderen Konto, bleibt sie, wie sie ist (ein Vermerk im
// Protokoll).
//
// Aktion ist die INSTALLIERTE exe in %ProgramFiles%\QuadChroma mit
// --hintergrund, nie die exe dort, wo der Nutzer das ZIP entpackt hat: dort
// koennte jedes Programm mit normalen Rechten sie austauschen und liefe dann
// bei der Anmeldung still als Administrator. Beim Einschalten installiert
// sich die (erhoehte) App deshalb dorthin (installation.rs: aus den beim
// Start gesperrten Dateien, mit dem geladenen Bild verglichen, nie eine
// aeltere Fassung ueber eine neuere), und die Aufgabe zeigt nur auf eine exe
// auf einem lokalen festen Laufwerk, deren Ordner und Datei nur
// Administratoren aendern duerfen. Laeuft die App schon von dort, entsteht
// nur die Aufgabe. Ausschalten loescht die Aufgabe dieses Kontos (und eine
// fruehere "QuadChroma" dieses Kontos); die installierte Kopie bleibt liegen.
//
// Beim Start der App (autostart_beim_start): zeigt die Aufgabe noch auf eine
// andere exe (fruehere Fassung), wird installiert und umgestellt; ist die
// laufende exe eine andere als die installierte (SHA-256), wird die
// installierte erneuert - ausser sie ist neuer. Die alten Verknuepfungen im
// Autostart-Ordner (QuadChroma.lnk der einen App frueherer Fassungen,
// "QuadChroma - Freigabe.lnk" der noch aelteren Host-Rolle mit --host)
// weichen ebenso der Aufgabe, wenn sie diese exe starten. Der Ordner gehoert
// dem Nutzer, die App ist erhoeht: jede Verknuepfung wird ueber EINEN Griff
// geprueft und geloescht (alte_lnk_oeffnen, ueber_griff_loeschen), der bis
// zum Loeschen offen bleibt - kein Verweis, und nichts laesst sich
// dazwischen unter ihrem Pfad austauschen.
//
// Loeschen (schtasks /Delete) und Abschalten (schtasks /Change /DISABLE)
// bleiben bei schtasks.exe. Es startet aus dem Systemordner, den das System
// selbst nennt (installation::system_ordner), nie ueber %SystemRoot% oder
// PATH - beide kann jedes Programm des Kontos ohne Adminrechte setzen.
//
// Scheitert das Umstellen, bleibt keine Aufgabe dieses Kontos still aktiv,
// die eine exe ausserhalb eines Administratorordners startet (eine fruehere
// Fassung zeigte auf den entpackten Ordner): sie wird abgeschaltet (schtasks
// /Change /DISABLE), ebenso eine fruehere "QuadChroma" dieses Kontos, deren
// Uebernahme scheiterte; der Startbildschirm zeigt dann den Hinweis
// AutostartFailed. Dasselbe, wenn das Konto gesperrt ist
// (installation::konto): dann wird nichts umgestellt, aber eine solche
// Aufgabe abgeschaltet (unsichere_aufgaben_abschalten).
//
// Ob der Punkt einen Haken traegt, sagt allein, ob die Aufgabe dieses Kontos
// da und eingeschaltet ist (autostart_an -> IRegisteredTask::Xml).
//
// Reine Logik (Aufgabennamen, das XML der Aufgabe und der Vergleich damit,
// die Befehlszeilen von schtasks) laeuft auf jeder Plattform und in den
// Tests; Aufgabenplanung, schtasks und Konten stehen hinter cfg(windows).

/// Vorsatz der Aufgabennamen - und der Name der frueheren Aufgabe fuer den
/// ganzen Rechner, die beim Start uebernommen wird.
pub const AUTOSTART_TASK: &str = "QuadChroma";
/// Dateiname der frueheren Autostart-Verknuepfung der einen App.
pub const AUTOSTART_DATEI: &str = "QuadChroma.lnk";
/// Die Verknuepfung der Host-Rolle vor der einen App.
pub const AUTOSTART_ALT: &str = "QuadChroma - Freigabe.lnk";
/// Argument der Autostart-Aufgabe (und frueher der Verknuepfung).
pub const AUTOSTART_ARGUMENT: &str = "--hintergrund";

/// Name der Aufgabe eines Kontos: "QuadChroma (<SID>)". Zeichen, die in
/// Aufgabennamen nicht stehen duerfen (\ / : * ? " < > | und
/// Steuerzeichen; "\" truege die Aufgabe sonst in einen Unterordner), werden
/// zu "_" - in einer SID kommen sie nicht vor.
pub fn task_fuer_konto(sid: &str) -> String {
    let rein: String = sid
        .trim()
        .chars()
        .map(|c| if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    format!("{AUTOSTART_TASK} ({rein})")
}

/// Wo "Mit Windows starten" wirkt: Name der Aufgabe dieses Kontos, Name
/// der frueheren Aufgabe fuer den ganzen Rechner und Installationsordner.
/// Tests geben das eindeutig mit und fassen so nie die echten Aufgaben oder
/// %ProgramFiles%\QuadChroma an; None heisst jeweils der Standard.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ort<'a> {
    /// Aufgabe dieses Kontos; None: "QuadChroma (<SID>)".
    pub task: Option<&'a str>,
    /// Fruehere Aufgabe fuer den ganzen Rechner; None: beim Standard
    /// "QuadChroma", sonst (Tests mit eigener Aufgabe) keine.
    pub alte: Option<&'a str>,
    pub ordner: Option<&'a Path>,
}

impl Ort<'static> {
    /// Die echte Aufgabe dieses Kontos, die fruehere "QuadChroma" und
    /// %ProgramFiles%\QuadChroma.
    pub const STANDARD: Ort<'static> = Ort { task: None, alte: None, ordner: None };
}

impl Ort<'_> {
    /// Der Name der frueheren Aufgabe fuer den ganzen Rechner, falls sie hier
    /// zaehlt.
    fn alte_task(&self) -> Option<&str> {
        match (self.task, self.alte) {
            (_, Some(a)) => Some(a),
            (None, None) => Some(AUTOSTART_TASK),
            (Some(_), None) => None,
        }
    }
}

#[cfg(windows)]
impl Ort<'_> {
    /// Der Name der Aufgabe dieses Kontos.
    fn task(&self) -> Result<String, String> {
        match self.task {
            Some(t) => Ok(t.to_string()),
            None => crate::installation::eigene_sid().map(task_fuer_konto),
        }
    }

    fn ordner(&self) -> Result<PathBuf, String> {
        match self.ordner {
            Some(o) => Ok(o.to_path_buf()),
            None => crate::installation::installationsordner(),
        }
    }
}

/// Autor der Aufgabe (RegistrationInfo).
pub const AUFGABE_AUTOR: &str = "QuadChroma";
/// Beschreibung der Aufgabe (RegistrationInfo), wie die Aufgabenplanung sie
/// zeigt - wie der Aufgabenname ein fester Text, nicht in der Sprache der
/// Oberflaeche (die Aufgabe entsteht auch ohne Fenster).
pub const AUFGABE_BESCHREIBUNG: &str =
    "Starts QuadChroma in the background when this account signs in (\"Start with Windows\" in QuadChroma).";
/// Prioritaet der Aufgabe: 4 ist normal. Ohne Angabe nimmt die
/// Aufgabenplanung 7 (unter normal) - schlechter fuer den Encoder als ein
/// Start von Hand.
pub const AUFGABE_PRIORITAET: u32 = 4;

/// Das vollstaendige XML der Aufgabe "Mit Windows starten": bei der
/// Anmeldung des Kontos `konto` (SID oder DOMAENE\name; Ausloeser und
/// Prinzipal), mit dessen Anmeldetoken und hoechsten Rechten, auch im
/// Akkubetrieb und ohne beim Abstecken zu enden, ohne Zeitlimit, mit
/// normaler Prioritaet; Aktion `exe` (in Anfuehrungszeichen wie bisher bei
/// schtasks /TR) mit --hintergrund, Arbeitsordner `ordner`. Jeder
/// eingesetzte Wert ist maskiert (xml_maskieren). Die Eintraege, die
/// abweichungen prueft, stehen alle ausdruecklich da, ebenso die uebrigen
/// Einstellungen - nichts haengt an Standards der Aufgabenplanung.
pub fn aufgabe_definition(konto: &str, exe: &Path, ordner: &Path) -> String {
    use crate::installation::xml_maskieren as m;
    let konto = m(konto.trim());
    let befehl = m(&format!("\"{}\"", exe.display()));
    let ordner = m(&ordner.display().to_string());
    let (autor, beschreibung, arg) = (m(AUFGABE_AUTOR), m(AUFGABE_BESCHREIBUNG), m(AUTOSTART_ARGUMENT));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>{autor}</Author>
    <Description>{beschreibung}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{konto}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{konto}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>{AUFGABE_PRIORITAET}</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{befehl}</Command>
      <Arguments>{arg}</Arguments>
      <WorkingDirectory>{ordner}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Ist die Dauer (ISO 8601, wie in ExecutionTimeLimit) null - "kein
/// Zeitlimit"? PT0S, P0D und aehnliche; jede andere Angabe (auch eine
/// unlesbare) ist ein Limit.
fn ohne_zeitlimit(dauer: &str) -> bool {
    let d = dauer.trim();
    d.starts_with('P')
        && d.chars().any(|c| c.is_ascii_digit())
        && d.chars().all(|c| c == '0' || c == '.' || "PYMWDTHS".contains(c))
}

/// Worin das XML einer Aufgabe (wie die Aufgabenplanung es liefert) von der
/// Definition abweicht, die aufgabe_definition fuer den Installationsordner
/// `ordner` schreibt - je Abweichung eine Zeile fuers Protokoll; leer: sie
/// stimmt. Geprueft werden, was fuer eine dauernd laufende App zaehlt: beide
/// Akku-Bedingungen, das Zeitlimit, die Prioritaet, das Konto des
/// Anmelde-Ausloesers (`eigenes_konto`: ist das dieses Konto?), der
/// Arbeitsordner und die hoechsten Rechte. Fehlt ein Eintrag, gilt der
/// Standard der Aufgabenplanung - so wie sie selbst ihn nicht mitschreibt
/// (schtasks /Create: kein ExecutionTimeLimit, keine Priority, also PT72H
/// und 7).
pub fn abweichungen(xml: &str, ordner: &Path, eigenes_konto: &dyn Fn(&str) -> bool) -> Vec<String> {
    use crate::installation::{gleicher_pfad, xml_element, xml_wert};
    let mut v = Vec::new();
    let einstellung = |name: &str| xml_wert(xml, &["Settings", name]);
    let schalter = |name: &str, standard: bool| match einstellung(name).as_deref() {
        Some("true") | Some("1") => true,
        Some("false") | Some("0") => false,
        _ => standard,
    };
    if schalter("DisallowStartIfOnBatteries", true) {
        v.push("startet nicht im Akkubetrieb (DisallowStartIfOnBatteries)".to_string());
    }
    if schalter("StopIfGoingOnBatteries", true) {
        v.push("endet beim Wechsel auf Akku (StopIfGoingOnBatteries)".to_string());
    }
    let limit = einstellung("ExecutionTimeLimit").unwrap_or_else(|| "PT72H".into());
    if !ohne_zeitlimit(&limit) {
        v.push(format!("Zeitlimit {limit} statt keinem (ExecutionTimeLimit)"));
    }
    let prioritaet = einstellung("Priority").unwrap_or_else(|| "7".into());
    if prioritaet.parse::<u32>().ok() != Some(AUFGABE_PRIORITAET) {
        v.push(format!("Prioritaet {prioritaet} statt {AUFGABE_PRIORITAET} (Priority)"));
    }
    match xml_element(xml, "Triggers").and_then(|t| xml_element(t, "LogonTrigger")) {
        None => v.push("kein Anmelde-Ausloeser (LogonTrigger)".to_string()),
        Some(t) => match xml_wert(t, &["UserId"]).filter(|u| !u.is_empty()) {
            None => v.push("der Anmelde-Ausloeser gilt fuer jede Anmeldung (ohne UserId)".to_string()),
            Some(u) if !eigenes_konto(&u) => v.push(format!("der Anmelde-Ausloeser gilt fuer {u}, nicht fuer dieses Konto")),
            Some(_) => {}
        },
    }
    let soll = ordner.display().to_string();
    match xml_wert(xml, &["Actions", "Exec", "WorkingDirectory"]).filter(|w| !w.is_empty()) {
        None => v.push(format!("ohne Arbeitsordner (soll {soll})")),
        Some(w) if !gleicher_pfad(&w, &soll) => v.push(format!("Arbeitsordner {w} statt {soll}")),
        Some(_) => {}
    }
    let stufe = xml_wert(xml, &["Principals", "Principal", "RunLevel"]).unwrap_or_else(|| "LeastPrivilege".into());
    if stufe != "HighestAvailable" {
        v.push(format!("ohne hoechste Rechte (RunLevel {stufe})"));
    }
    v
}

/// Wert fuer schtasks /TR, wie QuadChroma 0.1.0 die Aufgabe anlegte: die exe
/// in Anfuehrungszeichen (der Pfad kann Leerzeichen enthalten), dann
/// --hintergrund. Nur noch fuer Tests, die eine Aufgabe auf die alte Weise
/// (mit den Standards von schtasks) anlegen.
#[cfg(test)]
fn tr_wert(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_ARGUMENT}", exe.display())
}

/// Argumente fuer schtasks /Create, wie QuadChroma 0.1.0 die Aufgabe
/// anlegte: bei der Anmeldung (ONLOGON), hoechste Rechte (HIGHEST), als
/// `benutzer`, Aktion `tr`, vorhandene ueberschreiben (/F) - alles andere
/// nach den Standards von schtasks. Nur noch fuer Tests (siehe tr_wert).
#[cfg(test)]
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

/// Argumente fuer schtasks /Query der Aufgabe als XML. Gelesen wird ueber
/// COM (registriertes_xml); nur noch fuer Tests (schtasks aus dem
/// Systemordner, dasselbe XML wie ueber COM).
#[cfg(test)]
fn xml_args(task: &str) -> Vec<String> {
    vec!["/Query".into(), "/TN".into(), task.into(), "/XML".into()]
}

/// Argumente fuer schtasks /Delete der Aufgabe (ohne Rueckfrage, /F).
fn loeschen_task_args(task: &str) -> Vec<String> {
    vec!["/Delete".into(), "/TN".into(), task.into(), "/F".into()]
}

/// Argumente fuer schtasks /Change, das die Aufgabe abschaltet (sie bleibt
/// sichtbar, startet aber nicht mehr).
fn abschalten_args(task: &str) -> Vec<String> {
    vec!["/Change".into(), "/TN".into(), task.into(), "/DISABLE".into()]
}

/// schtasks.exe ausfuehren, ohne Konsolenfenster und mit leerer Eingabe (nie
/// eine Rueckfrage abwarten). Liefert (Erfolg, Ausgabe).
///
/// Voller Pfad im Systemordner, den das System selbst nennt
/// (installation::system_ordner, GetSystemDirectoryW) - weder ueber PATH
/// noch ueber %SystemRoot%: beide kann ein Programm des Kontos ohne
/// Adminrechte setzen (HKCU\Environment), und die App laeuft erhoeht. Aus
/// demselben Grund bekommt schtasks %SystemRoot% und %windir% mit dem echten
/// Windows-Ordner, nicht mit dem geerbten Wert.
#[cfg(windows)]
fn schtasks(args: &[String]) -> Result<(bool, String), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // CREATE_NO_WINDOW: eine erhoehte GUI-App soll fuer schtasks kein
    // Konsolenfenster aufblitzen lassen.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = crate::installation::system_ordner()?.join("schtasks.exe");
    let windows = crate::installation::windows_ordner()?;
    let mut befehl = Command::new(&exe);
    for name in crate::installation::WINDOWS_VARIABLEN {
        befehl.env(name, &windows);
    }
    let ausgabe = befehl
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("schtasks ({}): {e}", exe.display()))?;
    let mut text = String::from_utf8_lossy(&ausgabe.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&ausgabe.stderr));
    Ok((ausgabe.status.success(), text.trim().to_string()))
}

/// SAM-Name des aktuellen Nutzers (RECHNER\user bzw. DOMAENE\user) fuer die
/// Pruefung des Kontos (installation::konto; die Aufgabe selbst nennt das
/// Konto ueber seine SID). %USERDOMAIN% taugt nicht: bei einem lokalen Konto
/// steht dort die Arbeitsgruppe (etwa WORKGROUP), nicht der Rechnername.
/// GetUserNameEx liefert den richtigen Namen; klappt es nicht, der blosse
/// %USERNAME%.
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

/// Wo die alten Autostart-Verknuepfungen liegen: `ordner`, sonst der
/// Autostart-Ordner.
#[cfg(windows)]
fn autostart_lnk_ordner(ordner: Option<&Path>) -> Result<PathBuf, String> {
    match ordner {
        Some(o) => Ok(o.to_path_buf()),
        None => autostart_ordner(),
    }
}

/// Eine alte Autostart-Verknuepfung, wie alte_lnk_oeffnen sie vorfand.
#[cfg(windows)]
#[derive(Debug)]
enum AlteLnk {
    /// Es gibt sie nicht (bzw. dort liegt ein Ordner).
    Fehlt,
    /// Nicht angefasst - der Grund fuers Protokoll (mit Pfad).
    Bleibt(String),
    /// Geprueft: der offene Griff (fuer andere nur lesbar, mit Loeschrecht)
    /// und das Ziel der Verknuepfung.
    Gelesen(std::fs::File, String),
}

/// Hoechstgroesse einer alten Verknuepfung, die gelesen wird.
#[cfg(windows)]
const LNK_MAX: u64 = 1 << 20;

/// Eine alte Autostart-Verknuepfung `datei` in `ordner` oeffnen und ihr Ziel
/// lesen - alles ueber EINEN Griff, der offen bleibt, bis sie ueber ihn
/// geloescht ist (ueber_griff_loeschen). Der Autostart-Ordner gehoert dem
/// Nutzer (und laesst sich fuer das Konto umleiten), die App aber ist
/// erhoeht; zwischen Pruefen und Loeschen darf deshalb nichts unter dem Pfad
/// ausgetauscht werden koennen:
/// - geoeffnet, ohne einem Verweis am Ende zu folgen
///   (FILE_FLAG_OPEN_REPARSE_POINT), mit Lese- und Loeschrecht und fuer
///   andere nur lesbar - solange der Griff offen ist, laesst sich die Datei
///   weder aendern noch umbenennen noch loeschen;
/// - nur eine gewoehnliche Datei (kein Verweis, kein Ordner), die laut Griff
///   (GetFinalPathNameByHandleW) wirklich `datei` im aufgeloesten `ordner`
///   ist, und hoechstens LNK_MAX gross;
/// - gelesen wird ueber den Griff (IPersistStream aus dem Speicher), nie
///   erneut ueber den Pfad.
///
/// So wird genau die Datei geloescht, deren Ziel geprueft wurde. Alles
/// andere bleibt unangetastet (AlteLnk::Bleibt, ein Vermerk).
#[cfg(windows)]
fn alte_lnk_oeffnen(ordner: &Path, datei: &str) -> AlteLnk {
    use std::io::Read;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const GENERIC_READ: u32 = 0x8000_0000;
    const DELETE: u32 = 0x0001_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let pfad = ordner.join(datei);
    let bleibt = |grund: String| AlteLnk::Bleibt(format!("{}: {grund}", pfad.display()));
    // Nur um Fehlendes und Ordner still zu uebergehen - entschieden wird
    // allein ueber den Griff.
    match std::fs::symlink_metadata(&pfad) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AlteLnk::Fehlt,
        Ok(m) if m.is_dir() => return AlteLnk::Fehlt,
        _ => {}
    }
    let mut griff = match std::fs::OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&pfad)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AlteLnk::Fehlt,
        Err(e) => return bleibt(format!("nicht zu oeffnen ({e})")),
    };
    let m = match griff.metadata() {
        Ok(m) => m,
        Err(e) => return bleibt(format!("nicht lesbar ({e})")),
    };
    if m.file_attributes() & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return bleibt("keine gewoehnliche Datei (Verweis oder Ordner)".into());
    }
    if m.len() > LNK_MAX {
        return bleibt(format!("zu gross ({} Byte)", m.len()));
    }
    let Some(end) = crate::installation::endpfad(&griff, 0) else {
        return bleibt("Pfad hinter dem Griff nicht lesbar".into());
    };
    let erwartet = match std::fs::canonicalize(ordner) {
        Ok(o) => o.join(datei),
        Err(e) => return bleibt(format!("Ordner nicht aufloesbar ({e})")),
    };
    if !crate::installation::gleicher_pfad(&end, &erwartet.to_string_lossy()) {
        return bleibt(format!("die geoeffnete Datei liegt unter {end}, nicht unter {}", erwartet.display()));
    }
    let mut daten = Vec::new();
    if let Err(e) = (&mut griff).take(LNK_MAX + 1).read_to_end(&mut daten) {
        return bleibt(format!("nicht lesbar ({e})"));
    }
    match lnk_ziel_aus(&daten) {
        Ok(ziel) => AlteLnk::Gelesen(griff, ziel),
        Err(e) => bleibt(format!("keine lesbare Verknuepfung ({e})")),
    }
}

/// Die ueber `griff` geoeffnete Datei loeschen (FileDispositionInfo) - genau
/// diese, nicht was gerade unter `pfad` liegt (der steht nur in der
/// Meldung). Sie verschwindet, sobald der Griff zu ist.
#[cfg(windows)]
fn ueber_griff_loeschen(griff: std::fs::File, pfad: &Path) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO};
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let r = unsafe {
        SetFileInformationByHandle(
            HANDLE(griff.as_raw_handle()),
            FileDispositionInfo,
            &info as *const FILE_DISPOSITION_INFO as *const core::ffi::c_void,
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    drop(griff);
    r.map_err(|e| format!("{}: nicht geloescht ({})", pfad.display(), e.message()))
}

/// Eine geplante Aufgabe, wie die Aufgabenplanung sie als XML liefert.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aufgabe {
    /// Befehl und Argumente ihrer Aktion (leer, wenn keine lesbar ist).
    pub befehl: String,
    pub argumente: String,
    /// Das Konto, als das sie laeuft (Name oder SID); None bei einer Gruppe.
    pub konto: Option<String>,
    /// Eingeschaltet (nicht mit /DISABLE abgeschaltet).
    pub aktiv: bool,
    /// Ihr ganzes XML (fuer den Vergleich mit der Definition, abweichungen).
    pub xml: String,
}

#[cfg(windows)]
impl Aufgabe {
    /// "<Befehl> <Argumente>" fuers Protokoll.
    fn befehlszeile(&self) -> String {
        format!("{} {}", self.befehl, self.argumente).trim().to_string()
    }
}

/// Die Aufgabenplanung dieses Rechners (ITaskService, verbunden) und darin
/// der Stammordner "\", in dem die Aufgaben liegen. Nur innerhalb von im_sta
/// aufrufen; die Objekte muessen vor dessen Ende freigegeben sein.
#[cfg(windows)]
fn stammordner() -> Result<windows::Win32::System::TaskScheduler::ITaskFolder, String> {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::TaskScheduler::{ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;
    let fehler = |was: &str, e: windows::core::Error| format!("Aufgabenplanung ({was}): {} (0x{:08x})", e.message(), e.code().0 as u32);
    unsafe {
        let dienst: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(|e| fehler("ITaskService", e))?;
        // Leere Werte: dieser Rechner, das Konto dieses Prozesses.
        let leer = VARIANT::default();
        dienst.Connect(&leer, &leer, &leer, &leer).map_err(|e| fehler("Connect", e))?;
        dienst.GetFolder(&BSTR::from("\\")).map_err(|e| fehler("GetFolder", e))
    }
}

/// Die Aufgabe `task` mit dem vollstaendigen XML `xml` registrieren
/// (TASK_CREATE_OR_UPDATE: eine vorhandene gleichen Namens wird ersetzt,
/// auch eine abgeschaltete - sie ist danach so, wie das XML sagt), im
/// Anmeldetoken des Prinzipals aus dem XML (TASK_LOGON_INTERACTIVE_TOKEN,
/// kein Passwort). Das XML geht als Text hinein, nie ueber eine Datei.
#[cfg(windows)]
fn aufgabe_registrieren(task: &str, xml: &str) -> Result<(), String> {
    use windows::core::BSTR;
    use windows::Win32::System::TaskScheduler::{TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN};
    use windows::Win32::System::Variant::VARIANT;
    im_sta(|| {
        let ordner = stammordner()?;
        let leer = VARIANT::default();
        unsafe { ordner.RegisterTask(&BSTR::from(task), &BSTR::from(xml), TASK_CREATE_OR_UPDATE.0, &leer, &leer, TASK_LOGON_INTERACTIVE_TOKEN, &leer) }
            .map(|_| ())
            .map_err(|e| format!("Aufgabenplanung (RegisterTask \"{task}\"): {} (0x{:08x})", e.message(), e.code().0 as u32))
    })
}

/// Das XML der registrierten Aufgabe `task` (IRegisteredTask::Xml, als
/// UTF-16 - auch Umlaute in Kontonamen kommen richtig an); None, wenn es
/// sie nicht gibt oder sie nicht lesbar ist.
#[cfg(windows)]
fn registriertes_xml(task: &str) -> Option<String> {
    use windows::core::BSTR;
    im_sta(|| {
        let ordner = stammordner().ok()?;
        let t = unsafe { ordner.GetTask(&BSTR::from(task)) }.ok()?;
        unsafe { t.Xml() }.ok().map(|x| x.to_string())
    })
}

/// Die Aufgabe `task` lesen (ihr XML ueber die Aufgabenplanung); None, wenn
/// es sie nicht gibt.
#[cfg(windows)]
pub fn aufgabe(task: &str) -> Option<Aufgabe> {
    use crate::installation as inst;
    let xml = registriertes_xml(task)?;
    let (befehl, argumente) = inst::aufgabe_aus_xml(&xml).unwrap_or_default();
    Some(Aufgabe { befehl, argumente, konto: inst::aufgabe_konto(&xml), aktiv: inst::aufgabe_aktiv(&xml), xml })
}

/// Startet QuadChroma mit Windows - liegt die Aufgabe dieses Kontos vor und
/// ist sie eingeschaltet? Die eines anderen Kontos zaehlt nicht.
#[cfg(windows)]
pub fn autostart_an(ort: Ort) -> bool {
    ort.task().ok().and_then(|t| aufgabe(&t)).is_some_and(|a| a.aktiv)
}

/// Die fruehere Aufgabe fuer den ganzen Rechner ("QuadChroma"), wenn es sie
/// gibt: ihr Name, sie selbst und ob sie diesem Konto gehoert.
#[cfg(windows)]
fn alte_aufgabe(ort: Ort) -> Option<(String, Aufgabe, bool)> {
    let name = ort.alte_task()?;
    let a = aufgabe(name)?;
    let eigen = a.konto.as_deref().is_some_and(crate::installation::ist_eigenes_konto);
    Some((name.to_string(), a, eigen))
}

/// Die fruehere Aufgabe dieses Kontos loeschen, wenn es sie gibt (die
/// Aufgabe dieses Kontos tritt an ihre Stelle). Eine fremde bleibt.
#[cfg(windows)]
fn alte_entfernen(ort: Ort) -> Result<(), String> {
    let Some((name, _, true)) = alte_aufgabe(ort) else { return Ok(()) };
    let (ok, ausgabe) = schtasks(&loeschen_task_args(&name))?;
    if !ok && aufgabe(&name).is_some() {
        return Err(format!("die fruehere Aufgabe \"{name}\" nicht geloescht: schtasks /Delete: {ausgabe}"));
    }
    Ok(())
}

/// Eine Aufgabe, die abgeschaltet wurde (bzw. werden sollte), weil sie nicht
/// still starten darf.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Abgeschaltet {
    pub task: String,
    /// Was sie startete.
    pub befehl: String,
    /// Warum sie abgeschaltet ist.
    pub grund: String,
    /// Some: schtasks /Change /DISABLE scheiterte - sie ist noch an.
    pub fehler: Option<String>,
}

#[cfg(windows)]
impl Abgeschaltet {
    /// Die Zeile fuers Protokoll.
    pub fn zeile(&self) -> String {
        match &self.fehler {
            None => format!(
                "Mit Windows starten: die Aufgabe \"{}\" ({}) ist abgeschaltet (schtasks /Change /DISABLE) - {}",
                self.task, self.befehl, self.grund
            ),
            Some(f) => format!(
                "Mit Windows starten: die Aufgabe \"{}\" ({}) sollte abgeschaltet werden ({}), schtasks /Change /DISABLE scheiterte: {f}",
                self.task, self.befehl, self.grund
            ),
        }
    }
}

/// Die Aufgabe `task` abschalten (schtasks /Change /DISABLE).
#[cfg(windows)]
fn abschalten(task: &str, a: &Aufgabe, grund: String) -> Abgeschaltet {
    let fehler = match schtasks(&abschalten_args(task)) {
        Ok((true, _)) => None,
        Ok((false, ausgabe)) => Some(ausgabe),
        Err(e) => Some(e),
    };
    Abgeschaltet { task: task.to_string(), befehl: a.befehlszeile(), grund, fehler }
}

/// Warum die Aufgabe nicht still starten darf: ihre exe liegt nicht auf
/// einem lokalen festen Laufwerk in einem Ordner, den nur Administratoren
/// aendern koennen (oder fehlt). None: sie darf.
#[cfg(windows)]
fn unsicher(a: &Aufgabe) -> Option<String> {
    let exe = a.befehl.trim().trim_matches('"');
    if exe.is_empty() {
        return Some("sie hat keinen lesbaren Befehl".into());
    }
    crate::installation::ziel_pruefen(Path::new(exe))
        .err()
        .map(|e| format!("sie startet eine exe, die nicht nur Administratoren aendern koennen ({e})"))
}

/// Die Aufgaben dieses Kontos - die eigene und eine fruehere "QuadChroma",
/// die ihm gehoert -, die eingeschaltet sind und eine exe ausserhalb eines
/// Administratorordners starten (eine fruehere Fassung zeigte auf den
/// entpackten Ordner), abschalten: nach einem gescheiterten Umstellen und
/// wenn das Konto gesperrt ist. Liefert, was abgeschaltet wurde.
#[cfg(windows)]
pub fn unsichere_aufgaben_abschalten(ort: Ort) -> Vec<Abgeschaltet> {
    let mut v = Vec::new();
    if let Ok(t) = ort.task() {
        if let Some(a) = aufgabe(&t).filter(|a| a.aktiv) {
            if let Some(g) = unsicher(&a) {
                v.push(abschalten(&t, &a, g));
            }
        }
    }
    if let Some((name, a, true)) = alte_aufgabe(ort) {
        if let Some(g) = a.aktiv.then(|| unsicher(&a)).flatten() {
            v.push(abschalten(&name, &a, g));
        }
    }
    v
}

/// Ein Fehler samt dem, was danach abgeschaltet wurde, als eine Zeile.
#[cfg(windows)]
fn mit_abgeschalteten(fehler: String, ab: &[Abgeschaltet]) -> String {
    ab.iter().fold(fehler, |t, a| format!("{t}; {}", a.zeile()))
}

/// Die geplante Aufgabe anlegen (bzw. ersetzen) mit dem XML aus
/// aufgabe_definition: `exe` mit --hintergrund und ihrem Ordner als
/// Arbeitsordner, bei der Anmeldung dieses Kontos (seine SID), hoechste
/// Rechte, auch im Akkubetrieb, ohne Zeitlimit, normale Prioritaet. `exe`
/// muss auf einem lokalen festen Laufwerk liegen, und nur Administratoren
/// duerfen sie und ihren Ordner aendern - sonst verweigert (nie ein
/// Netzwerkpfad, nie ein Ordner des Nutzers).
#[cfg(windows)]
fn task_anlegen(task: &str, exe: &Path) -> Result<(), String> {
    crate::installation::ziel_pruefen(exe)?;
    let konto = crate::installation::eigene_sid()?;
    let ordner = exe.parent().ok_or_else(|| format!("{}: kein Ordner", exe.display()))?;
    aufgabe_registrieren(task, &aufgabe_definition(konto, exe, ordner))
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
    /// An: die Kopie in `ordner` stimmte schon mit dieser exe ueberein - nur
    /// die Aufgabe angelegt bzw. erneuert.
    Vorhanden(PathBuf),
    /// An: die Kopie in `ordner` ist neuer als diese exe (bzw. diese hat
    /// keine lesbare Version) - sie bleibt, die Aufgabe startet sie.
    NeuereBleibt { ordner: PathBuf, laufend: Option<crate::installation::Version>, installiert: crate::installation::Version },
    /// Aus: die Aufgabe ist geloescht. Eine installierte Kopie bleibt liegen
    /// (Some: ihr Ordner).
    Aus(Option<PathBuf>),
}

/// Einschalten: diese exe installieren (bzw. die installierte erneuern -
/// nie mit einer aelteren) und die Aufgabe dieses Kontos auf die
/// installierte exe richten; eine fruehere "QuadChroma" dieses Kontos
/// weicht ihr. Laeuft `exe` schon aus dem Installationsordner, nur die
/// Aufgabe.
#[cfg(windows)]
fn einschalten(ort: Ort, exe: &Path) -> Result<Umstellung, String> {
    use crate::installation as inst;
    let task = ort.task()?;
    let ordner = ort.ordner()?;
    let installiert = ordner.join(inst::EXE);
    let u = if inst::dieselbe_datei(exe, &installiert) {
        task_anlegen(&task, &installiert)?;
        Umstellung::NurAufgabe(ordner)
    } else {
        match inst::erneuern_entscheiden(&inst::vergleichen(&ordner)?) {
            inst::Erneuern::Gleich => {
                task_anlegen(&task, &installiert)?;
                Umstellung::Vorhanden(ordner)
            }
            inst::Erneuern::Ja => {
                let bericht = inst::installieren(&ordner)?;
                task_anlegen(&task, &installiert)?;
                Umstellung::Installiert { ordner, bericht }
            }
            inst::Erneuern::Nein { laufend, installiert: v } => {
                task_anlegen(&task, &installiert)?;
                Umstellung::NeuereBleibt { ordner, laufend, installiert: v }
            }
        }
    };
    alte_entfernen(ort)?;
    Ok(u)
}

/// "Mit Windows starten" an (installieren und die Aufgabe dieses Kontos
/// anlegen bzw. erneuern) oder aus (sie loeschen, dazu eine fruehere
/// "QuadChroma" dieses Kontos; fehlt sie schon, ist das kein Fehler -
/// schtasks /Delete meldet dann einen Fehler, den wir uebergehen, wenn die
/// Aufgabe danach wirklich weg ist). Die installierte Kopie bleibt beim
/// Ausschalten liegen. Scheitert das Einschalten, werden unsichere
/// Aufgaben dieses Kontos abgeschaltet (steht dann im Fehler).
#[cfg(windows)]
pub fn autostart_setzen(ort: Ort, an: bool) -> Result<Umstellung, String> {
    if an {
        let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
        return einschalten(ort, &exe).map_err(|e| mit_abgeschalteten(e, &unsichere_aufgaben_abschalten(ort)));
    }
    let task = ort.task()?;
    let (ok, ausgabe) = schtasks(&loeschen_task_args(&task))?;
    if !ok && aufgabe(&task).is_some() {
        return Err(format!("schtasks /Delete: {ausgabe}"));
    }
    alte_entfernen(ort)?;
    let ordner = ort.ordner().ok().filter(|o| o.join(crate::installation::EXE).is_file());
    Ok(Umstellung::Aus(ordner))
}

/// Was `autostart_beim_start` fuer die Aufgabe dieses Kontos tat.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Keine (eingeschaltete) Aufgabe und keine alte Verknuepfung - nichts
    /// zu tun.
    Aus,
    /// Die Aufgabe startet die installierte exe, und die ist dieselbe wie
    /// diese (oder diese laeuft von dort).
    Aktuell,
    /// Die installierte Kopie war eine andere, nicht neuere Fassung als diese
    /// exe - erneuert.
    Erneuert { ordner: PathBuf, bericht: crate::installation::Bericht },
    /// Die installierte Kopie ist neuer als diese exe (bzw. diese hat keine
    /// lesbare Version) - sie bleibt.
    NeuereBleibt { ordner: PathBuf, laufend: Option<crate::installation::Version>, installiert: crate::installation::Version },
    /// Die Aufgabe startete `vorher` (fruehere Fassung) - jetzt installiert
    /// und auf die installierte exe umgestellt.
    Umgestellt { vorher: String, neu: Umstellung },
    /// Die fruehere Aufgabe "QuadChroma" dieses Kontos (sie startete
    /// `vorher`) ist durch die Aufgabe dieses Kontos ersetzt.
    AlteUebernommen { vorher: String, neu: Umstellung },
    /// Eine alte Verknuepfung im Autostart-Ordner ist durch Installation und
    /// Aufgabe ersetzt.
    VerknuepfungErsetzt(Umstellung),
    /// Die einzige alte Verknuepfung startet ein anderes Programm (Ziel wie
    /// gelesen) - sie bleibt, und der Autostart zeigt nicht auf diese exe
    /// (etwa eine Test- oder portable Kopie).
    AndereExe(String),
}

/// Die Aufgabe dieses Kontos wich von ihrer Definition ab und ist neu
/// registriert (reparieren).
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reparatur {
    pub task: String,
    /// Was abwich (abweichungen), je eine Angabe.
    pub maengel: Vec<String>,
}

#[cfg(windows)]
impl Reparatur {
    /// Die Zeile fuers Protokoll.
    pub fn zeile(&self) -> String {
        format!(
            "Mit Windows starten: die Aufgabe \"{}\" ist neu registriert (auch im Akkubetrieb, ohne Zeitlimit, Prioritaet {AUFGABE_PRIORITAET}, nur bei der Anmeldung dieses Kontos) - vorher: {}",
            self.task,
            self.maengel.join("; ")
        )
    }
}

/// Ergebnis von `autostart_beim_start`.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeimStart {
    /// Was mit der Aufgabe dieses Kontos geschah (Err: gescheitert).
    pub start: Result<Start, String>,
    /// Some: die Aufgabe dieses Kontos startete schon die installierte exe,
    /// wich aber von ihrer Definition ab (etwa mit den Standards von
    /// schtasks aus 0.1.0 angelegt) und ist neu registriert.
    pub repariert: Option<Reparatur>,
    /// Weitere Zeilen fuers Protokoll (eine fruehere Aufgabe eines anderen
    /// Kontos, eine Aufgabe dieses Namens mit fremdem Prinzipal).
    pub vermerke: Vec<String>,
    /// Aufgaben, die nach einem Fehlschlag abgeschaltet wurden - dann gehoert
    /// der Hinweis AutostartFailed in den Startbildschirm.
    pub abgeschaltet: Vec<Abgeschaltet>,
}

/// Beim Start der App (erste Instanz, nur wenn das Konto es erlaubt):
/// - liegt noch eine alte Autostart-Verknuepfung da (QuadChroma.lnk oder
///   "QuadChroma - Freigabe.lnk") und startet sie DIESE exe, tritt die
///   Aufgabe an ihre Stelle (erst installieren und die Aufgabe anlegen, dann
///   die Verknuepfung loeschen). Zeigt die einzige vorhandene auf ein anderes
///   Programm, bleibt alles, wie es ist: eine zweite Kopie (Test, portabel)
///   soll den Autostart der installierten nicht auf sich umbiegen;
/// - gibt es die Aufgabe dieses Kontos nicht, aber die fruehere
///   "QuadChroma" und gehoert sie diesem Konto, wird sie uebernommen
///   (installieren, Aufgabe dieses Kontos anlegen, die alte loeschen);
///   gehoert sie einem anderen, bleibt sie (Vermerk);
/// - zeigt die Aufgabe dieses Kontos nicht auf die installierte exe mit
///   --hintergrund (fruehere Fassung: der entpackte Ordner), wird
///   installiert und umgestellt;
/// - zeigt sie dorthin, ist diese exe aber eine andere (SHA-256 von exe und
///   DLLs), wird die installierte Kopie erneuert - ausser sie ist neuer;
/// - zeigt sie dorthin, weicht aber von ihrer Definition ab (abweichungen:
///   noch mit den Standards von schtasks angelegt), wird sie neu
///   registriert (`repariert`) - nur wenn ihr Prinzipal dieses Konto ist.
///
/// Scheitert etwas, werden die Aufgaben dieses Kontos abgeschaltet, die
/// eine exe ausserhalb eines Administratorordners starten, und eine
/// fruehere "QuadChroma" dieses Kontos, die noch aktiv ist (ihre Uebernahme
/// ist gescheitert, und Haken und Menue zeigen sie nicht).
///
/// `lnk_ordner`: wo die alten Verknuepfungen liegen (Tests), sonst der
/// Autostart-Ordner.
#[cfg(windows)]
pub fn autostart_beim_start(lnk_ordner: Option<&Path>, ort: Ort) -> BeimStart {
    let alte = alte_aufgabe(ort);
    let mut vermerke = Vec::new();
    if let Some((name, a, false)) = &alte {
        vermerke.push(format!(
            "Mit Windows starten: die fruehere Aufgabe \"{name}\" gehoert {}, nicht diesem Konto - sie bleibt, wie sie ist (startet {})",
            a.konto.as_deref().unwrap_or("keinem einzelnen Konto"),
            a.befehlszeile()
        ));
    }
    let eigene_alte = alte.filter(|(_, _, eigen)| *eigen).map(|(_, a, _)| a);
    let mut repariert = None;
    let start = beim_start_pruefen(lnk_ordner, ort, eigene_alte.as_ref(), &mut repariert, &mut vermerke);
    let mut abgeschaltet = Vec::new();
    if start.is_err() {
        abgeschaltet = unsichere_aufgaben_abschalten(ort);
        if let Some((name, a, true)) = alte_aufgabe(ort) {
            if a.aktiv && !abgeschaltet.iter().any(|x| x.task == name) {
                abgeschaltet.push(abschalten(&name, &a, "ihre Uebernahme in die Aufgabe dieses Kontos ist gescheitert".into()));
            }
        }
    }
    BeimStart { start, repariert, vermerke, abgeschaltet }
}

/// Die Aufgabe `task` dieses Kontos, die schon die installierte exe
/// `installiert` startet, mit ihrer Definition vergleichen (abweichungen)
/// und bei einer Abweichung mit dem richtigen XML neu registrieren. Nur,
/// wenn ihr Prinzipal dieses Konto ist: eine Aufgabe eines anderen Kontos
/// (oder SYSTEM) bleibt, wie sie ist - ein Vermerk.
#[cfg(windows)]
fn reparieren(task: &str, a: &Aufgabe, installiert: &Path, vermerke: &mut Vec<String>) -> Result<Option<Reparatur>, String> {
    use crate::installation as inst;
    if !a.konto.as_deref().is_some_and(inst::ist_eigenes_konto) {
        vermerke.push(format!(
            "Mit Windows starten: die Aufgabe \"{task}\" laeuft als {}, nicht als dieses Konto - ihre Einstellungen bleiben, wie sie sind",
            a.konto.as_deref().unwrap_or("keinem einzelnen Konto")
        ));
        return Ok(None);
    }
    let ordner = installiert.parent().unwrap_or(installiert);
    let maengel = abweichungen(&a.xml, ordner, &inst::ist_eigenes_konto);
    if maengel.is_empty() {
        return Ok(None);
    }
    task_anlegen(task, installiert)
        .map_err(|e| format!("die Aufgabe \"{task}\" weicht ab ({}), neu registrieren scheiterte: {e}", maengel.join("; ")))?;
    Ok(Some(Reparatur { task: task.to_string(), maengel }))
}

/// Der Kern von `autostart_beim_start` (ohne Abschalten nach Fehlern).
/// `eigene_alte`: die fruehere Aufgabe "QuadChroma", wenn sie diesem Konto
/// gehoert. Eine alte Verknuepfung, die nicht sicher ueber einen Griff
/// gelesen werden kann (alte_lnk_oeffnen), bleibt - ein Eintrag in
/// `vermerke`. Eine reparierte Aufgabe (reparieren) steht in `repariert`.
#[cfg(windows)]
fn beim_start_pruefen(
    lnk_ordner: Option<&Path>,
    ort: Ort,
    eigene_alte: Option<&Aufgabe>,
    repariert: &mut Option<Reparatur>,
    vermerke: &mut Vec<String>,
) -> Result<Start, String> {
    use crate::installation as inst;
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    let ordner = autostart_lnk_ordner(lnk_ordner)?;
    let mut zu_migrieren = Vec::new();
    let mut fremd = None;
    for datei in [AUTOSTART_DATEI, AUTOSTART_ALT] {
        match alte_lnk_oeffnen(&ordner, datei) {
            AlteLnk::Fehlt => {}
            AlteLnk::Bleibt(grund) => {
                vermerke.push(format!("Mit Windows starten: die alte Verknuepfung {grund} - sie bleibt, wie sie ist"));
            }
            AlteLnk::Gelesen(griff, ziel) => {
                if inst::dieselbe_datei(Path::new(&ziel), &exe) {
                    zu_migrieren.push((ordner.join(datei), griff));
                } else if fremd.is_none() {
                    fremd = Some(ziel);
                }
            }
        }
    }
    if !zu_migrieren.is_empty() {
        // Erst installieren und die Aufgabe anlegen, dann die alten
        // Verknuepfungen loeschen - ueber die Griffe, die seit der Pruefung
        // offen sind (scheitert etwas, schliessen sie, und nichts ist
        // geloescht).
        let neu = einschalten(ort, &exe)?;
        for (pfad, griff) in zu_migrieren {
            ueber_griff_loeschen(griff, &pfad)?;
        }
        return Ok(Start::VerknuepfungErsetzt(neu));
    }
    let task = ort.task()?;
    let Some(a) = aufgabe(&task) else {
        // Keine Aufgabe dieses Kontos: die fruehere dieses Kontos uebernehmen -
        // nur eine eingeschaltete (eine abgeschaltete heisst aus).
        if let Some(alt) = eigene_alte.filter(|a| a.aktiv) {
            let neu = einschalten(ort, &exe)?;
            return Ok(Start::AlteUebernommen { vorher: alt.befehlszeile(), neu });
        }
        return Ok(match fremd {
            Some(ziel) => Start::AndereExe(ziel),
            None => Start::Aus,
        });
    };
    // Die Aufgabe dieses Kontos gilt; eine fruehere daneben ist ueberzaehlig.
    if eigene_alte.is_some() {
        alte_entfernen(ort)?;
    }
    if !a.aktiv {
        // Abgeschaltet (von Hand oder nach einem Fehlschlag): aus.
        return Ok(Start::Aus);
    }
    let ordner = ort.ordner()?;
    let installiert = ordner.join(inst::EXE);
    if inst::aufgabe_zeigt_auf(&a.befehl, &a.argumente, &installiert) {
        // Erst die Aufgabe selbst (eine aus 0.1.0 startete im Akkubetrieb
        // nie), dann die installierte Kopie.
        *repariert = reparieren(&task, &a, &installiert, vermerke)?;
        if inst::dieselbe_datei(&exe, &installiert) {
            return Ok(Start::Aktuell);
        }
        return Ok(match inst::erneuern_entscheiden(&inst::vergleichen(&ordner)?) {
            inst::Erneuern::Gleich => Start::Aktuell,
            inst::Erneuern::Ja => Start::Erneuert { bericht: inst::installieren(&ordner)?, ordner },
            inst::Erneuern::Nein { laufend, installiert: v } => Start::NeuereBleibt { ordner, laufend, installiert: v },
        });
    }
    let neu = einschalten(ort, &exe)?;
    Ok(Start::Umgestellt { vorher: a.befehlszeile(), neu })
}

/// Das Ziel einer Verknuepfung (IShellLinkW::GetPath) aus ihrem Inhalt
/// `daten` - geladen ueber IPersistStream aus dem Speicher, nicht ueber
/// einen Pfad (alte_lnk_oeffnen hat die Datei ueber ihren Griff gelesen).
#[cfg(windows)]
fn lnk_ziel_aus(daten: &[u8]) -> Result<String, String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{CoCreateInstance, IPersistStream, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkW, SHCreateMemStream, ShellLink};
    im_sta(|| unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let strom = SHCreateMemStream(Some(daten)).ok_or_else(|| "kein Speicherstrom".to_string())?;
        let persist: IPersistStream = link.cast().map_err(|e| e.message())?;
        persist.Load(&strom).map_err(|e| e.message())?;
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

    /// Reine Logik der geplanten Aufgabe (auf jeder Plattform): der Name je
    /// Konto, welche fruehere Aufgabe zaehlt, der /TR-Wert mit der exe in
    /// Anfuehrungszeichen und die Befehlszeilen von schtasks.
    #[test]
    fn schtasks_befehlszeilen() {
        assert_eq!(AUTOSTART_TASK, "QuadChroma");
        assert_eq!(task_fuer_konto("S-1-5-21-3623811015-3361044348-30300820-1013"), "QuadChroma (S-1-5-21-3623811015-3361044348-30300820-1013)");
        assert_eq!(task_fuer_konto(" PC\\a/b:c*d?e\"f<g>h|i\u{1} "), "QuadChroma (PC_a_b_c_d_e_f_g_h_i_)");
        assert!(Ort::STANDARD.task.is_none() && Ort::STANDARD.alte.is_none() && Ort::STANDARD.ordner.is_none());
        // Beim Standard zaehlt die fruehere "QuadChroma"; ein Test mit eigener
        // Aufgabe fasst sie nie an, ausser er nennt eine eigene.
        assert_eq!(Ort::STANDARD.alte_task(), Some("QuadChroma"));
        assert_eq!(Ort { task: Some("T"), ..Ort::default() }.alte_task(), None);
        assert_eq!(Ort { task: Some("T"), alte: Some("A"), ordner: None }.alte_task(), Some("A"));
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
        assert_eq!(xml_args("QC"), vec!["/Query", "/TN", "QC", "/XML"]);
        assert_eq!(loeschen_task_args("QC"), vec!["/Delete", "/TN", "QC", "/F"]);
        assert_eq!(abschalten_args("QuadChroma (S-1-5-18)"), vec!["/Change", "/TN", "QuadChroma (S-1-5-18)", "/DISABLE"]);
    }

    const SID: &str = "S-1-5-21-1413765741-3029102522-1828496402-1002";
    const PF: &str = "C:\\Program Files\\QuadChroma";
    const PF_EXE: &str = "C:\\Program Files\\QuadChroma\\quadchroma.exe";

    /// Das XML der Aufgabe: jede Einstellung steht ausdruecklich da (nichts
    /// haengt an Standards der Aufgabenplanung), Ausloeser und Prinzipal
    /// nennen das Konto, die Aktion ist die exe in Anfuehrungszeichen mit
    /// --hintergrund und ihrem Ordner als Arbeitsordner - und die Parser,
    /// die die App beim Lesen benutzt, finden alles wieder.
    #[test]
    fn aufgabe_definition_vollstaendig() {
        use crate::installation::{aufgabe_aktiv, aufgabe_aus_xml, aufgabe_konto, aufgabe_zeigt_auf, xml_wert};
        let xml = aufgabe_definition(SID, Path::new(PF_EXE), Path::new(PF));
        let w = |pfad: &[&str]| xml_wert(&xml, pfad).unwrap_or_else(|| panic!("{pfad:?} fehlt: {xml}"));
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">"));
        assert_eq!(w(&["RegistrationInfo", "Author"]), "QuadChroma");
        assert_eq!(w(&["RegistrationInfo", "Description"]), AUFGABE_BESCHREIBUNG);
        assert_eq!(w(&["Triggers", "LogonTrigger", "UserId"]), SID);
        assert_eq!(w(&["Triggers", "LogonTrigger", "Enabled"]), "true");
        assert_eq!(w(&["Principals", "Principal", "UserId"]), SID);
        assert_eq!(w(&["Principals", "Principal", "LogonType"]), "InteractiveToken");
        assert_eq!(w(&["Principals", "Principal", "RunLevel"]), "HighestAvailable");
        for (name, wert) in [
            ("MultipleInstancesPolicy", "IgnoreNew"),
            ("DisallowStartIfOnBatteries", "false"),
            ("StopIfGoingOnBatteries", "false"),
            ("AllowHardTerminate", "true"),
            ("StartWhenAvailable", "false"),
            ("RunOnlyIfNetworkAvailable", "false"),
            ("AllowStartOnDemand", "true"),
            ("Enabled", "true"),
            ("Hidden", "false"),
            ("RunOnlyIfIdle", "false"),
            ("WakeToRun", "false"),
            ("ExecutionTimeLimit", "PT0S"),
            ("Priority", "4"),
        ] {
            assert_eq!(w(&["Settings", name]), wert, "{name}");
        }
        assert_eq!(w(&["Settings", "IdleSettings", "StopOnIdleEnd"]), "false");
        assert_eq!(w(&["Settings", "IdleSettings", "RestartOnIdle"]), "false");
        assert_eq!(w(&["Actions", "Exec", "Command"]), format!("\"{PF_EXE}\""));
        assert_eq!(w(&["Actions", "Exec", "Arguments"]), "--hintergrund");
        assert_eq!(w(&["Actions", "Exec", "WorkingDirectory"]), PF);
        // Genau ein Ausloeser und eine Aktion.
        assert_eq!(xml.matches("<LogonTrigger>").count(), 1);
        assert_eq!(xml.matches("<Exec>").count(), 1);
        // Die Parser der App lesen dasselbe.
        let (befehl, argumente) = aufgabe_aus_xml(&xml).unwrap();
        assert!(aufgabe_zeigt_auf(&befehl, &argumente, Path::new(PF_EXE)), "{befehl} {argumente}");
        assert_eq!(aufgabe_konto(&xml).as_deref(), Some(SID));
        assert!(aufgabe_aktiv(&xml));
        assert_eq!(abweichungen(&xml, Path::new(PF), &|u| u == SID), Vec::<String>::new());
    }

    /// Jeder eingesetzte Wert ist maskiert: ein Pfad mit & < > " ' und ein
    /// Kontoname mit & ergeben wohlgeformtes XML, und gelesen kommt genau
    /// der Wert zurueck.
    #[test]
    fn aufgabe_definition_maskiert() {
        use crate::installation::{aufgabe_aus_xml, aufgabe_konto, xml_wert};
        let ordner = "C:\\A & B <x> \"q\" 'y'";
        let exe = format!("{ordner}\\quadchroma.exe");
        let xml = aufgabe_definition("PC\\a&b", Path::new(&exe), Path::new(ordner));
        assert!(xml.contains("<WorkingDirectory>C:\\A &amp; B &lt;x&gt; &quot;q&quot; &apos;y&apos;</WorkingDirectory>"), "{xml}");
        assert!(xml.contains("<Command>&quot;C:\\A &amp; B &lt;x&gt; &quot;q&quot; &apos;y&apos;\\quadchroma.exe&quot;</Command>"), "{xml}");
        assert!(xml.contains("<UserId>PC\\a&amp;b</UserId>"), "{xml}");
        assert!(!xml.contains("<x>") && !xml.contains("& B") && !xml.contains("'y'"), "{xml}");
        // In jedem Elementtext: kein < > " ' und kein & ausserhalb einer Entitaet.
        let mut rest = xml.as_str();
        while let Some(i) = rest.find('>') {
            rest = &rest[i + 1..];
            let text = &rest[..rest.find('<').unwrap_or(rest.len())];
            assert!(!text.contains(['>', '"', '\'']), "{text:?}");
            for (j, _) in text.match_indices('&') {
                let e = &text[j..];
                assert!(["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"].iter().any(|x| e.starts_with(x)), "{text:?}");
            }
        }
        assert_eq!(aufgabe_aus_xml(&xml), Some((format!("\"{exe}\""), "--hintergrund".into())));
        assert_eq!(xml_wert(&xml, &["Actions", "Exec", "WorkingDirectory"]).as_deref(), Some(ordner));
        assert_eq!(aufgabe_konto(&xml).as_deref(), Some("PC\\a&b"));
        assert_eq!(xml_wert(&xml, &["Triggers", "LogonTrigger", "UserId"]).as_deref(), Some("PC\\a&b"));
        // Die Definition selbst stimmt mit sich ueberein, auch mit diesem Pfad.
        assert!(abweichungen(&xml, Path::new(ordner), &|u| u == "PC\\a&b").is_empty());
    }

    /// Eine Aufgabe, wie schtasks /Create /SC ONLOGON /RL HIGHEST /RU ...
    /// sie in 0.1.0 anlegte, so wie die Aufgabenplanung sie liefert
    /// (IRegisteredTask::Xml bzw. schtasks /Query /XML, Windows 11 24H2):
    /// ohne ExecutionTimeLimit und Priority (also PT72H und 7), Akku-Sperren
    /// an, Ausloeser ohne UserId, kein Arbeitsordner.
    const ALTE_AUFGABE: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Date>2026-09-28T03:03:57</Date>
    <Author>FORK-WIN\rober</Author>
    <URI>\QuadChroma (S-1-5-21-1413765741-3029102522-1828496402-1002)</URI>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>S-1-5-21-1413765741-3029102522-1828496402-1002</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <IdleSettings>
      <Duration>PT10M</Duration>
      <WaitTimeout>PT1H</WaitTimeout>
      <StopOnIdleEnd>true</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
  </Settings>
  <Triggers>
    <LogonTrigger>
      <StartBoundary>2026-09-28T03:03:00</StartBoundary>
    </LogonTrigger>
  </Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>"C:\Program Files\QuadChroma\quadchroma.exe"</Command>
      <Arguments>--hintergrund</Arguments>
    </Exec>
  </Actions>
</Task>"#;

    /// Die Aufgabe aus aufgabe_definition, wie die Aufgabenplanung sie nach
    /// RegisterTask zurueckgibt (IRegisteredTask::Xml, Windows 11 24H2):
    /// Eintraege mit dem Standardwert fallen weg, die uebrigen sind
    /// umgeordnet, und der Ausloeser nennt das Konto beim Namen statt mit
    /// der SID.
    const REGISTRIERT: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>QuadChroma</Author>
    <Description>Starts QuadChroma in the background when this account signs in ("Start with Windows" in QuadChroma).</Description>
    <URI>\QuadChroma (S-1-5-21-1413765741-3029102522-1828496402-1002)</URI>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>S-1-5-21-1413765741-3029102522-1828496402-1002</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <Priority>4</Priority>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
  </Settings>
  <Triggers>
    <LogonTrigger>
      <UserId>FORK-WIN\rober</UserId>
    </LogonTrigger>
  </Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>"C:\Program Files\QuadChroma\quadchroma.exe"</Command>
      <Arguments>--hintergrund</Arguments>
      <WorkingDirectory>C:\Program Files\QuadChroma</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#;

    /// Braucht die Aufgabe eine Reparatur? Die aus 0.1.0 (Standards von
    /// schtasks) weicht in genau sechs Punkten ab; die neue Definition in
    /// keinem - auch nicht so, wie die Aufgabenplanung sie zurueckgibt
    /// (Eintraege umgeordnet, der Ausloeser mit dem Kontonamen). Jede
    /// einzelne Abweichung faellt auf, ein fehlender Eintrag zaehlt mit dem
    /// Standard der Aufgabenplanung.
    #[test]
    fn abweichungen_alt_und_neu() {
        let eigen = |u: &str| u == SID || u.eq_ignore_ascii_case("FORK-WIN\\rober");
        let pf = Path::new(PF);
        let alt = abweichungen(ALTE_AUFGABE, pf, &eigen);
        assert_eq!(
            alt,
            vec![
                "startet nicht im Akkubetrieb (DisallowStartIfOnBatteries)",
                "endet beim Wechsel auf Akku (StopIfGoingOnBatteries)",
                "Zeitlimit PT72H statt keinem (ExecutionTimeLimit)",
                "Prioritaet 7 statt 4 (Priority)",
                "der Anmelde-Ausloeser gilt fuer jede Anmeldung (ohne UserId)",
                "ohne Arbeitsordner (soll C:\\Program Files\\QuadChroma)",
            ]
        );
        let neu = aufgabe_definition(SID, Path::new(PF_EXE), pf);
        assert!(abweichungen(&neu, pf, &eigen).is_empty());
        assert!(abweichungen(REGISTRIERT, pf, &eigen).is_empty(), "{:?}", abweichungen(REGISTRIERT, pf, &eigen));
        // Je eine Abweichung.
        let eine = |von: &str, zu: &str| -> Vec<String> {
            assert!(neu.contains(von), "{von}");
            abweichungen(&neu.replace(von, zu), pf, &eigen)
        };
        let akku = eine("<DisallowStartIfOnBatteries>false", "<DisallowStartIfOnBatteries>true");
        assert!(akku.len() == 1 && akku[0].contains("Akkubetrieb"), "{akku:?}");
        assert_eq!(eine("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>", "").len(), 1);
        assert_eq!(eine("<ExecutionTimeLimit>PT0S", "<ExecutionTimeLimit>P3D"), vec!["Zeitlimit P3D statt keinem (ExecutionTimeLimit)"]);
        assert!(eine("<ExecutionTimeLimit>PT0S", "<ExecutionTimeLimit>P0D").is_empty());
        assert_eq!(eine("<Priority>4</Priority>", "").len(), 1);
        assert_eq!(eine("<Priority>4", "<Priority>6"), vec!["Prioritaet 6 statt 4 (Priority)"]);
        let fremd = eine(&format!("<UserId>{SID}</UserId>\n    </LogonTrigger>"), "<UserId>PC\\anderer</UserId>\n    </LogonTrigger>");
        assert_eq!(fremd, vec!["der Anmelde-Ausloeser gilt fuer PC\\anderer, nicht fuer dieses Konto"]);
        assert_eq!(
            eine(&format!("<UserId>{SID}</UserId>\n    </LogonTrigger>"), "</LogonTrigger>"),
            vec!["der Anmelde-Ausloeser gilt fuer jede Anmeldung (ohne UserId)"]
        );
        assert_eq!(
            abweichungen(&neu.replace("<LogonTrigger>", "<BootTrigger>").replace("</LogonTrigger>", "</BootTrigger>"), pf, &eigen),
            vec!["kein Anmelde-Ausloeser (LogonTrigger)"]
        );
        assert_eq!(
            eine("<WorkingDirectory>C:\\Program Files\\QuadChroma<", "<WorkingDirectory>C:\\Users\\rob\\Downloads<"),
            vec!["Arbeitsordner C:\\Users\\rob\\Downloads statt C:\\Program Files\\QuadChroma"]
        );
        // Gross/klein und ein "\" am Ende sind derselbe Ordner.
        assert!(eine("<WorkingDirectory>C:\\Program Files\\QuadChroma<", "<WorkingDirectory>c:\\program files\\quadchroma\\<").is_empty());
        assert_eq!(eine("<RunLevel>HighestAvailable", "<RunLevel>LeastPrivilege"), vec!["ohne hoechste Rechte (RunLevel LeastPrivilege)"]);
        assert_eq!(eine("<RunLevel>HighestAvailable</RunLevel>", "").len(), 1);
        // Eine leere Aufgabe weicht in allem ab.
        assert_eq!(abweichungen("<Task></Task>", pf, &eigen).len(), 7);
    }

    #[test]
    fn zeitlimit() {
        for d in ["PT0S", "P0D", "PT0M", "P0DT0H0M0S", " PT0S "] {
            assert!(ohne_zeitlimit(d), "{d}");
        }
        for d in ["PT72H", "P3D", "PT1S", "", "P", "PT", "0", "unbegrenzt", "PT0.5S"] {
            assert!(!ohne_zeitlimit(d), "{d}");
        }
    }

    /// Ein Ort fuer Tests: eigene Aufgabe, eigener Ordner, keine fruehere
    /// Aufgabe.
    #[cfg(windows)]
    fn test_ort<'a>(task: &'a str, ordner: &'a Path) -> Ort<'a> {
        Ort { task: Some(task), alte: None, ordner: Some(ordner) }
    }

    /// Ein leerer Ordner als Autostart-Ordner der Tests (ohne alte
    /// Verknuepfungen; der echte ist im Test gesperrt).
    #[cfg(windows)]
    fn leer() -> PathBuf {
        let o = std::env::temp_dir().join(format!("{}-lnk-leer", crate::secure::test_lauf()));
        std::fs::create_dir_all(&o).unwrap();
        o
    }

    /// Befehl und Argumente der Aufgabe.
    #[cfg(windows)]
    fn befehl(task: &str) -> Option<(String, String)> {
        aufgabe(task).map(|a| (a.befehl, a.argumente))
    }

    /// Eine Aufgabe wie frueher (0.1.0) anlegen - schtasks /Create mit dessen
    /// Standards: sie startet `exe` dort, wo sie liegt, als `konto` (ohne die
    /// Pruefungen von task_anlegen).
    #[cfg(windows)]
    fn alte_anlegen(task: &str, konto: &str, exe: &Path) {
        let (ok, a) = schtasks(&erstellen_args(task, konto, &tr_wert(exe))).unwrap();
        assert!(ok, "{a}");
    }

    /// Die Aufgabe, wie Get-ScheduledTask (PowerShell, aus dem Systemordner)
    /// sie zeigt: Akku-Bedingungen, Zeitlimit, Prioritaet, Konto des ersten
    /// Ausloesers, Arbeitsordner der ersten Aktion, Stufe und Konto des
    /// Prinzipals, eingeschaltet - als Name -> Wert.
    #[cfg(windows)]
    fn geplant(task: &str) -> std::collections::HashMap<String, String> {
        let ps = crate::installation::system_ordner().unwrap().join("WindowsPowerShell\\v1.0\\powershell.exe");
        let skript = format!(
            "$ErrorActionPreference = 'Stop'; $t = Get-ScheduledTask -TaskPath '\\' -TaskName '{}'; $s = $t.Settings; \
             'DisallowStartIfOnBatteries=' + $s.DisallowStartIfOnBatteries; 'StopIfGoingOnBatteries=' + $s.StopIfGoingOnBatteries; \
             'ExecutionTimeLimit=' + $s.ExecutionTimeLimit; 'Priority=' + $s.Priority; 'Enabled=' + $s.Enabled; \
             'Trigger=' + $t.Triggers.Count; 'TriggerUserId=' + $t.Triggers[0].UserId; \
             'WorkingDirectory=' + $t.Actions[0].WorkingDirectory; 'RunLevel=' + $t.Principal.RunLevel; \
             'LogonType=' + $t.Principal.LogonType",
            task.replace('\'', "''")
        );
        let aus = std::process::Command::new(ps).args(["-NoProfile", "-NonInteractive", "-Command", &skript]).output().unwrap();
        let text = String::from_utf8_lossy(&aus.stdout).to_string();
        assert!(aus.status.success(), "Get-ScheduledTask {task}: {text}{}", String::from_utf8_lossy(&aus.stderr));
        text.lines().filter_map(|z| z.split_once('=')).map(|(n, w)| (n.trim().to_string(), w.trim().to_string())).collect()
    }

    /// Get-ScheduledTask zeigt die Aufgabe `task` mit den Einstellungen aus
    /// aufgabe_definition: startet auch im Akkubetrieb und endet dort nicht,
    /// kein Zeitlimit, Prioritaet 4, genau ein Ausloeser, und der gilt fuer
    /// dieses Konto, Arbeitsordner `ordner`, hoechste Rechte im Anmeldetoken.
    /// Auch das XML, das die App liest, weicht nicht ab.
    #[cfg(windows)]
    fn einstellungen_pruefen(task: &str, ordner: &Path) {
        use crate::installation::{gleicher_pfad, ist_eigenes_konto};
        let g = geplant(task);
        let w = |n: &str| g.get(n).cloned().unwrap_or_else(|| panic!("{n} fehlt: {g:?}"));
        assert_eq!(w("DisallowStartIfOnBatteries"), "False", "{g:?}");
        assert_eq!(w("StopIfGoingOnBatteries"), "False", "{g:?}");
        assert_eq!(w("ExecutionTimeLimit"), "PT0S", "{g:?}");
        assert_eq!(w("Priority"), "4", "{g:?}");
        assert_eq!(w("Enabled"), "True", "{g:?}");
        assert_eq!(w("Trigger"), "1", "{g:?}");
        assert!(!w("TriggerUserId").is_empty() && ist_eigenes_konto(&w("TriggerUserId")), "{g:?}");
        assert!(gleicher_pfad(&w("WorkingDirectory"), &ordner.to_string_lossy()), "{g:?}");
        assert_eq!(w("RunLevel"), "Highest", "{g:?}");
        assert_eq!(w("LogonType"), "Interactive", "{g:?}");
        let a = aufgabe(task).unwrap();
        assert_eq!(abweichungen(&a.xml, ordner, &ist_eigenes_konto), Vec::<String>::new(), "{}", a.xml);
    }

    /// Die Dateiversion einer exe auf der Platte aendern (Versionsressource).
    #[cfg(windows)]
    fn version_setzen(pfad: &Path, v: crate::installation::Version) {
        let mut d = std::fs::read(pfad).unwrap();
        let (s, _) = crate::installation::pe_version_stelle(&d).unwrap();
        d[s + 8..s + 12].copy_from_slice(&((u32::from(v[0]) << 16) | u32::from(v[1])).to_le_bytes());
        d[s + 12..s + 16].copy_from_slice(&((u32::from(v[2]) << 16) | u32::from(v[3])).to_le_bytes());
        std::fs::write(pfad, d).unwrap();
        assert_eq!(crate::installation::pe_version(&std::fs::read(pfad).unwrap()), Some(v));
    }

    /// Die Aufgabe je Konto (Windows): "QuadChroma (<SID dieses Prozesses>)";
    /// das eigene Konto wird als Name und als SID erkannt, SYSTEM und ein
    /// unbekanntes Konto nicht.
    #[cfg(windows)]
    #[test]
    fn aufgabe_je_konto() {
        use crate::installation::{eigene_sid, ist_eigenes_konto, konto_sid};
        let sid = eigene_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(Ort::STANDARD.task().unwrap(), format!("QuadChroma ({sid})"));
        assert!(ist_eigenes_konto(&aktueller_benutzer()), "{}", aktueller_benutzer());
        assert!(ist_eigenes_konto(sid) && ist_eigenes_konto(&sid.to_lowercase()));
        assert_eq!(konto_sid("SYSTEM").as_deref(), Some("S-1-5-18"));
        assert!(!ist_eigenes_konto("SYSTEM") && !ist_eigenes_konto("S-1-5-18") && !ist_eigenes_konto("gibt-es-nicht-4711"));
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
    /// Anmeldung, als der aktuelle Nutzer. Zweimal an ist kein Fehler (/F),
    /// die Kopie stimmt dann schon; aus loescht die Aufgabe, die Dateien
    /// bleiben; zweimal aus ist kein Fehler. Eigene Aufgabe und eigener
    /// Ordner, nie die echten.
    #[cfg(windows)]
    #[test]
    fn autostart_an_und_aus() {
        use crate::installation::{DLLS, EXE};
        let Some(ordner) = pf_ordner("anaus") else { return };
        let task = format!("{}-anaus", crate::secure::test_lauf());
        let ort = test_ort(&task, &ordner);
        // Vor und nach dem Test aufraeumen, egal wie er ausging.
        let _ = autostart_setzen(ort, false);
        assert!(!autostart_an(ort), "Aufgabe schon da?");
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
        assert!(autostart_an(ort), "Aufgabe nach 'an' nicht da");
        let exe = std::env::current_exe().unwrap();
        let installiert = ordner.join(EXE);
        assert_eq!(sha(&exe), sha(&installiert));
        for d in DLLS {
            assert!(ordner.join(d).is_file(), "{d}");
        }
        assert!(reste(&ordner).is_empty(), "{:?}", reste(&ordner));
        // Die Aufgabe: die installierte exe mit --hintergrund, hoechste
        // Rechte, Anmelde-Ausloeser, als der aktuelle Nutzer, eingeschaltet -
        // auch so, wie schtasks /Query /XML sie liest.
        let (befehl, argumente) = befehl(&task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert), "{befehl} {argumente}");
        let (ok, xml) = schtasks(&xml_args(&task)).unwrap();
        assert!(ok, "Query /XML: {xml}");
        assert!(xml.contains("HighestAvailable"), "kein HIGHEST: {xml}");
        assert!(xml.contains("<LogonTrigger>"), "kein Anmelde-Ausloeser: {xml}");
        let konto = crate::installation::aufgabe_konto(&xml).unwrap();
        assert!(crate::installation::ist_eigenes_konto(&konto), "nicht als aktueller Nutzer ({konto}): {xml}");
        assert!(crate::installation::aufgabe_aktiv(&xml));
        // Registriert ueber COM mit dem ganzen XML: auch im Akkubetrieb, ohne
        // Zeitlimit, Prioritaet 4, Ausloeser dieses Kontos, Arbeitsordner.
        einstellungen_pruefen(&task, &ordner);
        // Zweimal an: kein Fehler, die Aufgabe bleibt genau eine, die Kopie
        // stimmt schon.
        assert_eq!(autostart_setzen(ort, true), Ok(Umstellung::Vorhanden(ordner.clone())));
        assert!(autostart_an(ort));
        // Abgeschaltet (schtasks /Change /DISABLE) zaehlt als aus.
        assert!(schtasks(&abschalten_args(&task)).unwrap().0);
        assert!(!autostart_an(ort) && aufgabe(&task).is_some());
        assert_eq!(autostart_setzen(ort, true), Ok(Umstellung::Vorhanden(ordner.clone())));
        assert!(autostart_an(ort), "an schaltet sie wieder ein");
        // Aus: Aufgabe weg, die installierte Kopie bleibt.
        assert_eq!(autostart_setzen(ort, false), Ok(Umstellung::Aus(Some(ordner.clone()))));
        assert!(!autostart_an(ort) && aufgabe(&task).is_none(), "Aufgabe nach 'aus' noch da");
        assert!(installiert.is_file());
        // Zweimal aus: die Aufgabe fehlt schon, das ist kein Fehler.
        assert!(autostart_setzen(ort, false).is_ok());
        assert!(!autostart_an(ort));
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Reparatur beim Start: eine Aufgabe dieses Kontos, die schon die
    /// installierte exe startet, aber wie in 0.1.0 mit schtasks und dessen
    /// Standards angelegt ist (startet nicht im Akkubetrieb, Zeitlimit 72 h,
    /// Prioritaet 7, Ausloeser fuer jede Anmeldung, kein Arbeitsordner),
    /// wird beim Start mit dem richtigen XML neu registriert; danach zeigt
    /// Get-ScheduledTask die richtigen Einstellungen, und ein zweiter Start
    /// findet nichts mehr. Eine Aufgabe dieses Namens, die als SYSTEM laeuft,
    /// bleibt, wie sie ist (ein Vermerk). Eigene Aufgabe und eigener Ordner.
    #[cfg(windows)]
    #[test]
    fn autostart_aufgabe_reparieren() {
        use crate::installation::EXE;
        let Some(ordner) = pf_ordner("reparatur") else { return };
        let task = format!("{}-reparatur", crate::secure::test_lauf());
        let ort = test_ort(&task, &ordner);
        let _ = autostart_setzen(ort, false);
        assert!(matches!(autostart_setzen(ort, true), Ok(Umstellung::Installiert { .. })));
        let installiert = ordner.join(EXE);
        // So registriert, wie diese Fassung es tut: nichts zu reparieren.
        let b = autostart_beim_start(Some(&leer()), ort);
        assert_eq!((&b.start, &b.repariert), (&Ok(Start::Aktuell), &None), "{b:?}");
        // Wie 0.1.0: schtasks /Create mit dessen Standards, auf die
        // installierte exe (sie zeigt also schon richtig).
        alte_anlegen(&task, &aktueller_benutzer(), &installiert);
        let g = geplant(&task);
        assert_eq!((g["DisallowStartIfOnBatteries"].as_str(), g["StopIfGoingOnBatteries"].as_str()), ("True", "True"), "{g:?}");
        assert_eq!((g["ExecutionTimeLimit"].as_str(), g["Priority"].as_str()), ("PT72H", "7"), "{g:?}");
        assert_eq!((g["TriggerUserId"].as_str(), g["WorkingDirectory"].as_str()), ("", ""), "{g:?}");
        let b = autostart_beim_start(Some(&leer()), ort);
        assert!(b.abgeschaltet.is_empty() && b.vermerke.is_empty(), "{b:?}");
        assert_eq!(b.start, Ok(Start::Aktuell), "{b:?}");
        let r = b.repariert.expect("nicht repariert");
        assert_eq!(r.task, task);
        assert_eq!(r.maengel.len(), 6, "{:?}", r.maengel);
        for teil in ["DisallowStartIfOnBatteries", "StopIfGoingOnBatteries", "PT72H", "Prioritaet 7", "jede Anmeldung", "ohne Arbeitsordner"] {
            assert!(r.maengel.iter().any(|m| m.contains(teil)), "{teil}: {:?}", r.maengel);
        }
        assert!(r.zeile().contains("neu registriert") && r.zeile().contains("Akkubetrieb"), "{}", r.zeile());
        einstellungen_pruefen(&task, &ordner);
        let (befehl, argumente) = befehl(&task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert), "{befehl} {argumente}");
        // Zweiter Start: nichts mehr zu reparieren.
        let b = autostart_beim_start(Some(&leer()), ort);
        assert_eq!((&b.start, &b.repariert), (&Ok(Start::Aktuell), &None), "{b:?}");
        // Eine abgeschaltete Aufgabe mit alten Standards bleibt aus.
        alte_anlegen(&task, &aktueller_benutzer(), &installiert);
        assert!(schtasks(&abschalten_args(&task)).unwrap().0);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert_eq!((&b.start, &b.repariert), (&Ok(Start::Aus), &None), "{b:?}");
        assert!(aufgabe(&task).is_some_and(|a| !a.aktiv));
        // Laeuft sie als SYSTEM, bleibt sie, wie sie ist - mit Vermerk.
        alte_anlegen(&task, "SYSTEM", &installiert);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert_eq!(b.repariert, None, "{b:?}");
        assert!(b.vermerke.len() == 1 && b.vermerke[0].contains("S-1-5-18") && b.vermerke[0].contains("bleiben"), "{:?}", b.vermerke);
        assert_eq!(geplant(&task)["DisallowStartIfOnBatteries"], "True");
        // Aus: geloescht, auch die als SYSTEM.
        assert!(autostart_setzen(ort, false).is_ok());
        assert!(aufgabe(&task).is_none());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Beim Start: ohne Aufgabe nichts zu tun; eine Aufgabe, die noch auf
    /// eine andere exe zeigt (fruehere Fassung: der entpackte Ordner), wird
    /// nach der Installation umgestellt; weicht die installierte Kopie bei
    /// gleicher Version von dieser exe ab (ein anderer Bau), wird sie
    /// erneuert; stimmt alles, nichts.
    #[cfg(windows)]
    #[test]
    fn autostart_beim_start_umstellen_und_erneuern() {
        use crate::installation::EXE;
        let Some(ordner) = pf_ordner("start") else { return };
        let lnk = std::env::temp_dir().join(format!("{}-start-lnk", crate::secure::test_lauf()));
        std::fs::create_dir_all(&lnk).unwrap();
        let task = format!("{}-start", crate::secure::test_lauf());
        let ort = test_ort(&task, &ordner);
        let _ = autostart_setzen(ort, false);
        assert_eq!(autostart_beim_start(Some(&lnk), ort).start, Ok(Start::Aus));
        let exe = std::env::current_exe().unwrap();
        alte_anlegen(&task, &aktueller_benutzer(), &exe);
        let b = autostart_beim_start(Some(&lnk), ort);
        assert!(b.abgeschaltet.is_empty() && b.vermerke.is_empty(), "{b:?}");
        match b.start.unwrap() {
            Start::Umgestellt { vorher, neu: Umstellung::Installiert { ordner: o, .. } } => {
                let alt = vorher.trim_end_matches(AUTOSTART_ARGUMENT).trim();
                assert!(crate::installation::gleicher_pfad(alt, &exe.to_string_lossy()), "{vorher}");
                assert_eq!(o, ordner);
            }
            s => panic!("{s:?}"),
        }
        let installiert = ordner.join(EXE);
        let (befehl, argumente) = befehl(&task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert), "{befehl} {argumente}");
        // Stimmt alles: nichts zu tun.
        assert_eq!(autostart_beim_start(Some(&lnk), ort).start, Ok(Start::Aktuell));
        // Die installierte Kopie ist ein anderer Bau derselben Version: erneuert.
        let mut f = std::fs::OpenOptions::new().append(true).open(&installiert).unwrap();
        std::io::Write::write_all(&mut f, b"anderer Bau").unwrap();
        drop(f);
        assert_ne!(sha(&installiert), sha(&exe));
        assert!(matches!(autostart_beim_start(Some(&lnk), ort).start, Ok(Start::Erneuert { .. })));
        assert_eq!(sha(&installiert), sha(&exe));
        assert_eq!(autostart_beim_start(Some(&lnk), ort).start, Ok(Start::Aktuell));
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = std::fs::remove_dir_all(&lnk);
    }

    /// Keine Rueckstufung: ist die installierte Kopie neuer als diese exe
    /// (hoehere Dateiversion), bleibt sie - beim Start wie beim Einschalten,
    /// und die Aufgabe startet sie. Ist sie aelter, wird sie erneuert.
    #[cfg(windows)]
    #[test]
    fn autostart_keine_rueckstufung() {
        use crate::installation::{pe_version, EXE};
        let Some(ordner) = pf_ordner("version") else { return };
        let task = format!("{}-version", crate::secure::test_lauf());
        let ort = test_ort(&task, &ordner);
        let _ = autostart_setzen(ort, false);
        assert!(matches!(autostart_setzen(ort, true), Ok(Umstellung::Installiert { .. })));
        let exe = std::env::current_exe().unwrap();
        let laufend = pe_version(&std::fs::read(&exe).unwrap()).expect("Testprogramm ohne Versionsressource");
        let installiert = ordner.join(EXE);
        let neuer = [laufend[0] + 1, 0, 0, 0];
        version_setzen(&installiert, neuer);
        let vorher = sha(&installiert);
        let erwartet = Start::NeuereBleibt { ordner: ordner.clone(), laufend: Some(laufend), installiert: neuer };
        assert_eq!(autostart_beim_start(Some(&leer()), ort).start, Ok(erwartet));
        assert_eq!(sha(&installiert), vorher, "die neuere Kopie wurde ersetzt");
        let erwartet = Umstellung::NeuereBleibt { ordner: ordner.clone(), laufend: Some(laufend), installiert: neuer };
        assert_eq!(autostart_setzen(ort, true), Ok(erwartet));
        assert_eq!(sha(&installiert), vorher, "die neuere Kopie wurde beim Einschalten ersetzt");
        let (befehl, argumente) = befehl(&task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &installiert));
        // Aelter als diese exe: erneuert.
        version_setzen(&installiert, [0, 0, 1, 0]);
        assert!(matches!(autostart_beim_start(Some(&leer()), ort).start, Ok(Start::Erneuert { .. })));
        assert_eq!(sha(&installiert), sha(&exe));
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Die fruehere Aufgabe fuer den ganzen Rechner: gehoert sie diesem
    /// Konto, wird sie beim Start uebernommen (installiert, die Aufgabe
    /// dieses Kontos angelegt, die alte geloescht); gehoert sie einem anderen
    /// Konto (hier SYSTEM), bleibt sie, wie sie ist, mit einem Vermerk, und
    /// "aus" laesst sie ebenso stehen.
    #[cfg(windows)]
    #[test]
    fn autostart_fruehere_aufgabe_uebernehmen() {
        use crate::installation::EXE;
        let Some(ordner) = pf_ordner("uebernahme") else { return };
        let task = format!("{}-uebernahme", crate::secure::test_lauf());
        let alte = format!("{}-uebernahme-alt", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), alte: Some(&alte), ordner: Some(&ordner) };
        let _ = schtasks(&loeschen_task_args(&task));
        let _ = schtasks(&loeschen_task_args(&alte));
        let exe = std::env::current_exe().unwrap();
        // 1. Die fruehere gehoert diesem Konto: uebernommen.
        alte_anlegen(&alte, &aktueller_benutzer(), &exe);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert!(b.abgeschaltet.is_empty() && b.vermerke.is_empty(), "{b:?}");
        match b.start.unwrap() {
            Start::AlteUebernommen { vorher, neu: Umstellung::Installiert { .. } } => {
                assert!(crate::installation::gleicher_pfad(vorher.trim_end_matches(AUTOSTART_ARGUMENT).trim(), &exe.to_string_lossy()), "{vorher}");
            }
            s => panic!("{s:?}"),
        }
        assert!(aufgabe(&alte).is_none(), "die fruehere ist noch da");
        assert!(autostart_an(ort));
        let (befehl, argumente) = befehl(&task).unwrap();
        assert!(crate::installation::aufgabe_zeigt_auf(&befehl, &argumente, &ordner.join(EXE)), "{befehl} {argumente}");
        assert_eq!(autostart_beim_start(Some(&leer()), ort).start, Ok(Start::Aktuell));
        // 2. Die fruehere gehoert SYSTEM: sie bleibt, eingeschaltet, mit Vermerk.
        assert!(autostart_setzen(ort, false).is_ok());
        alte_anlegen(&alte, "SYSTEM", &exe);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert_eq!(b.start, Ok(Start::Aus));
        assert!(b.vermerke.len() == 1 && b.vermerke[0].contains("S-1-5-18") && b.vermerke[0].contains("bleibt"), "{:?}", b.vermerke);
        assert!(b.abgeschaltet.is_empty());
        assert!(aufgabe(&alte).is_some_and(|a| a.aktiv) && !autostart_an(ort));
        // Einschalten und ausschalten lassen sie ebenso stehen.
        assert!(autostart_setzen(ort, true).is_ok() && autostart_an(ort));
        assert!(autostart_setzen(ort, false).is_ok());
        assert!(aufgabe(&alte).is_some_and(|a| a.aktiv), "die fremde Aufgabe wurde angefasst");
        assert!(unsichere_aufgaben_abschalten(ort).is_empty(), "eine fremde Aufgabe wird nicht abgeschaltet");
        let _ = schtasks(&loeschen_task_args(&alte));
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Scheitert das Umstellen, bleibt keine Aufgabe dieses Kontos still
    /// aktiv, die eine exe ausserhalb eines Administratorordners startet:
    /// hier zeigen die Aufgabe dieses Kontos bzw. die fruehere auf dieses
    /// Testprogramm (target, fuer Nutzer beschreibbar), und der
    /// Installationsordner liegt im Temp-Ordner des Nutzers, so dass die
    /// Installation scheitert. Beide werden abgeschaltet, ein zweiter Start
    /// laesst sie aus. Ist das Konto gesperrt, schaltet
    /// unsichere_aufgaben_abschalten dasselbe ab, eine Aufgabe auf die
    /// installierte Kopie aber nicht.
    #[cfg(windows)]
    #[test]
    fn autostart_abschalten_nach_fehlschlag() {
        use crate::installation::EXE;
        let Some(pf) = pf_ordner("abschalten") else { return };
        let temp = std::env::temp_dir().join(format!("{}-abschalten", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&temp);
        let task = format!("{}-abschalten", crate::secure::test_lauf());
        let alte = format!("{}-abschalten-alt", crate::secure::test_lauf());
        let ort = Ort { task: Some(&task), alte: Some(&alte), ordner: Some(&temp) };
        let _ = schtasks(&loeschen_task_args(&task));
        let _ = schtasks(&loeschen_task_args(&alte));
        let exe = std::env::current_exe().unwrap();
        // 1. Die Aufgabe dieses Kontos zeigt auf den beschreibbaren Ordner.
        alte_anlegen(&task, &aktueller_benutzer(), &exe);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert!(b.start.is_err(), "{b:?}");
        assert_eq!(b.abgeschaltet.len(), 1, "{b:?}");
        assert_eq!((b.abgeschaltet[0].task.as_str(), &b.abgeschaltet[0].fehler), (task.as_str(), &None));
        assert!(b.abgeschaltet[0].zeile().contains("abgeschaltet"), "{}", b.abgeschaltet[0].zeile());
        assert!(aufgabe(&task).is_some_and(|a| !a.aktiv) && !autostart_an(ort));
        assert!(!temp.join(EXE).exists());
        // Zweiter Start: abgeschaltet heisst aus - nichts wird wieder an.
        assert_eq!(autostart_beim_start(Some(&leer()), ort).start, Ok(Start::Aus));
        assert!(aufgabe(&task).is_some_and(|a| !a.aktiv));
        // Einschalten scheitert hier ebenso - und laesst nichts an.
        alte_anlegen(&task, &aktueller_benutzer(), &exe);
        let e = autostart_setzen(ort, true).unwrap_err();
        assert!(e.contains("abgeschaltet"), "{e}");
        assert!(aufgabe(&task).is_some_and(|a| !a.aktiv));
        let _ = schtasks(&loeschen_task_args(&task));
        // 2. Nur die fruehere dieses Kontos: ihre Uebernahme scheitert - abgeschaltet.
        alte_anlegen(&alte, &aktueller_benutzer(), &exe);
        let b = autostart_beim_start(Some(&leer()), ort);
        assert!(b.start.is_err(), "{b:?}");
        assert!(b.abgeschaltet.len() == 1 && b.abgeschaltet[0].task == alte, "{b:?}");
        assert!(aufgabe(&alte).is_some_and(|a| !a.aktiv) && aufgabe(&task).is_none());
        // Abgeschaltet wird sie beim naechsten Start nicht uebernommen.
        assert_eq!(autostart_beim_start(Some(&leer()), ort).start, Ok(Start::Aus));
        assert!(aufgabe(&task).is_none());
        let _ = schtasks(&loeschen_task_args(&alte));
        // 3. Gesperrtes Konto: abgeschaltet wird nur, was auf einen
        // beschreibbaren Ordner zeigt, nicht die Aufgabe auf die installierte
        // Kopie in Program Files.
        let sicher = test_ort(&task, &pf);
        assert!(matches!(autostart_setzen(sicher, true), Ok(Umstellung::Installiert { .. })));
        assert!(unsichere_aufgaben_abschalten(sicher).is_empty());
        assert!(autostart_an(sicher));
        alte_anlegen(&task, &aktueller_benutzer(), &exe);
        let ab = unsichere_aufgaben_abschalten(sicher);
        assert!(ab.len() == 1 && ab[0].fehler.is_none(), "{ab:?}");
        assert!(!autostart_an(sicher));
        let _ = schtasks(&loeschen_task_args(&task));
        let _ = std::fs::remove_dir_all(&temp);
        let _ = std::fs::remove_dir_all(&pf);
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
        let _ = schtasks(&loeschen_task_args(&task));
        // 1. Ein Ordner im Temp-Verzeichnis (dem Nutzer gehoerend).
        let temp = std::env::temp_dir().join(format!("{}-offen", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&temp);
        let e = autostart_setzen(test_ort(&task, &temp), true).unwrap_err();
        assert!(e.contains("Besitzer") || e.contains("darf hier schreiben"), "{e}");
        assert!(!temp.join(EXE).exists() && !autostart_an(test_ort(&task, &temp)), "{e}");
        // 2. Unter Program Files, aber "Benutzer" (S-1-5-32-545) duerfen aendern.
        std::fs::create_dir(&pf).unwrap();
        let icacls = std::process::Command::new(crate::installation::system_ordner().unwrap().join("icacls.exe"))
            .arg(&pf)
            .args(["/grant", "*S-1-5-32-545:(OI)(CI)M"])
            .output()
            .unwrap();
        assert!(icacls.status.success(), "{}", String::from_utf8_lossy(&icacls.stdout));
        let e = autostart_setzen(test_ort(&task, &pf), true).unwrap_err();
        assert!(e.contains("S-1-5-32-545"), "{e}");
        assert!(!pf.join(EXE).exists() && aufgabe(&task).is_none(), "{e}");
        // 3. Ein UNC-Pfad (derselbe Rechner ueber die Admin-Freigabe).
        let text = pf.to_string_lossy().to_string();
        let unc = PathBuf::from(format!("\\\\localhost\\{}${}", &text[..1], &text[2..]));
        let e = autostart_setzen(test_ort(&task, &unc), true).unwrap_err();
        assert!(e.contains("kein lokaler Pfad"), "{e}");
        assert!(aufgabe(&task).is_none());
        // 4. Die Aufgabe direkt auf eine exe im Temp-Ordner: verweigert.
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join(EXE), b"MZ").unwrap();
        assert!(task_anlegen(&task, &temp.join(EXE)).is_err());
        assert!(aufgabe(&task).is_none());
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
        installieren(&ordner).unwrap();
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
        let bericht = installieren(&ordner);
        let _ = kind.kill();
        let _ = kind.wait();
        let bericht = bericht.unwrap();
        assert!(
            bericht.beim_neustart.iter().any(|p| p.file_name().unwrap().to_string_lossy().starts_with("quadchroma.exe.alt-")),
            "{bericht:?}"
        );
        assert_eq!(sha(&installiert), sha(&exe));
        installieren(&ordner).unwrap();
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
        use crate::installation::dieselbe_datei;
        let Some(pf) = pf_ordner("fremd") else { return };
        let ordner = std::env::temp_dir().join(format!("{}-autostart-fremd", crate::secure::test_lauf()));
        let task = format!("{}-fremd", crate::secure::test_lauf());
        let ort = test_ort(&task, &pf);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = autostart_setzen(ort, false);
        std::fs::create_dir_all(&ordner).unwrap();
        let alt = ordner.join(AUTOSTART_ALT);
        let andere = ordner.join("installiert").join("quadchroma.exe");
        std::fs::create_dir_all(andere.parent().unwrap()).unwrap();
        std::fs::write(&andere, b"MZ").unwrap();
        im_sta(|| lnk_speichern(&alt, &andere, "--host", &ordner, "QuadChroma: Diesen PC freigeben", &andere, 0)).unwrap();
        match autostart_beim_start(Some(&ordner), ort).start {
            // Dieselbe Datei, egal ob lang oder als 8.3-Name geschrieben
            // (temp_dir kann "RUNNER~1" liefern, die Verknuepfung den langen Namen).
            Ok(Start::AndereExe(z)) => assert!(dieselbe_datei(Path::new(&z), &andere), "{z} / {}", andere.display()),
            r => panic!("{r:?}"),
        }
        assert!(alt.is_file(), "die alte bleibt");
        assert!(!autostart_an(ort) && !pf.exists(), "keine Aufgabe und keine Installation fuer eine fremde exe");
        // Ziel fehlt ganz: ebenso nicht diese exe.
        std::fs::remove_file(&andere).unwrap();
        assert!(matches!(autostart_beim_start(Some(&ordner), ort).start, Ok(Start::AndereExe(_))));
        assert!(alt.is_file() && !autostart_an(ort));
        // Diese exe, in Grossbuchstaben geschrieben: dieselbe Datei.
        let exe = std::env::current_exe().unwrap();
        let gross = std::path::PathBuf::from(exe.display().to_string().to_uppercase());
        assert!(dieselbe_datei(&gross, &exe));
        std::fs::remove_file(&alt).unwrap();
        im_sta(|| lnk_speichern(&alt, &gross, "--host", &ordner, "x", &exe, 0)).unwrap();
        assert!(matches!(autostart_beim_start(Some(&ordner), ort).start, Ok(Start::VerknuepfungErsetzt(Umstellung::Installiert { .. }))));
        assert!(!alt.exists() && autostart_an(ort), "Aufgabe angelegt, alte weg");
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
        let ort = test_ort(&task, &pf);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = autostart_setzen(ort, false);
        std::fs::create_dir_all(&ordner).unwrap();
        let exe = std::env::current_exe().unwrap();
        // Ohne alte Verknuepfung und ohne Aufgabe nichts zu tun.
        assert_eq!(autostart_beim_start(Some(&ordner), ort).start, Ok(Start::Aus), "ohne alte nichts zu tun");
        assert!(!autostart_an(ort));
        for (i, datei) in [AUTOSTART_DATEI, AUTOSTART_ALT].into_iter().enumerate() {
            let lnk = ordner.join(datei);
            im_sta(|| lnk_speichern(&lnk, &exe, "--hintergrund", &ordner, "QuadChroma", &exe, 0)).unwrap();
            // Beim ersten Mal wird installiert, danach stimmt die Kopie schon.
            match autostart_beim_start(Some(&ordner), ort).start {
                Ok(Start::VerknuepfungErsetzt(Umstellung::Installiert { .. })) if i == 0 => {}
                Ok(Start::VerknuepfungErsetzt(Umstellung::Vorhanden(_))) if i == 1 => {}
                s => panic!("{datei}: {s:?}"),
            }
            assert!(!lnk.exists() && autostart_an(ort), "{datei}: Aufgabe da, .lnk weg");
            // Zweiter Start: keine .lnk mehr, die Aufgabe zeigt auf die
            // installierte exe, und die stimmt.
            assert_eq!(autostart_beim_start(Some(&ordner), ort).start, Ok(Start::Aktuell), "{datei}: zweiter Start");
            assert!(autostart_setzen(ort, false).is_ok());
            assert!(!autostart_an(ort));
        }
        let _ = autostart_setzen(ort, false);
        let _ = std::fs::remove_dir_all(&ordner);
        let _ = std::fs::remove_dir_all(&pf);
    }

    /// Eine alte Verknuepfung wird ueber EINEN Griff geprueft und geloescht:
    /// solange er offen ist, laesst sie sich weder loeschen noch umbenennen
    /// noch ueberschreiben, und geloescht wird genau die geoeffnete Datei,
    /// auch wenn inzwischen eine andere unter ihrem Pfad liegt. Ein Verweis
    /// und eine Datei ohne lesbare Verknuepfung bleiben unangetastet, ein
    /// Ordner zaehlt als fehlend; beim Start gibt das einen Vermerk, keinen
    /// Fehler. Ohne Adminrechte (keine Aufgabe, keine Installation).
    #[cfg(windows)]
    #[test]
    fn alte_verknuepfung_ueber_griff() {
        use crate::installation::dieselbe_datei;
        let ordner = std::env::temp_dir().join(format!("{}-lnk-griff", crate::secure::test_lauf()));
        let _ = std::fs::remove_dir_all(&ordner);
        std::fs::create_dir_all(&ordner).unwrap();
        let exe = std::env::current_exe().unwrap();
        let lnk = ordner.join(AUTOSTART_DATEI);
        let anlegen = |p: &Path| im_sta(|| lnk_speichern(p, &exe, AUTOSTART_ARGUMENT, &ordner, "QuadChroma", &exe, 0)).unwrap();
        // 1. Geprueft, und solange der Griff offen ist, bleibt die Datei, wie
        // sie ist; geloescht ueber den Griff.
        anlegen(&lnk);
        let (griff, ziel) = match alte_lnk_oeffnen(&ordner, AUTOSTART_DATEI) {
            AlteLnk::Gelesen(g, z) => (g, z),
            a => panic!("{a:?}"),
        };
        assert!(dieselbe_datei(Path::new(&ziel), &exe), "{ziel}");
        assert!(std::fs::remove_file(&lnk).is_err(), "geloescht trotz offenem Griff");
        assert!(std::fs::rename(&lnk, ordner.join("anders.lnk")).is_err(), "umbenannt trotz offenem Griff");
        assert!(std::fs::write(&lnk, b"x").is_err(), "ueberschrieben trotz offenem Griff");
        ueber_griff_loeschen(griff, &lnk).unwrap();
        assert!(!lnk.exists());
        // 2. Der Ordner wird nach der Pruefung verschoben (sofern Windows das
        // bei offenem Griff zulaesst), und an seiner Stelle liegt eine andere
        // Datei gleichen Namens: geloescht wird die gepruefte, nicht die neue.
        anlegen(&lnk);
        let AlteLnk::Gelesen(griff, _) = alte_lnk_oeffnen(&ordner, AUTOSTART_DATEI) else { panic!("nicht gelesen") };
        let verschoben = ordner.with_extension("verschoben");
        let _ = std::fs::remove_dir_all(&verschoben);
        if std::fs::rename(&ordner, &verschoben).is_ok() {
            std::fs::create_dir_all(&ordner).unwrap();
            std::fs::write(&lnk, b"andere Datei").unwrap();
            ueber_griff_loeschen(griff, &lnk).unwrap();
            assert_eq!(std::fs::read(&lnk).unwrap(), b"andere Datei", "die neue Datei wurde angefasst");
            assert!(!verschoben.join(AUTOSTART_DATEI).exists(), "die gepruefte Datei ist noch da");
            std::fs::remove_file(&lnk).unwrap();
            let _ = std::fs::remove_dir_all(&verschoben);
        } else {
            ueber_griff_loeschen(griff, &lnk).unwrap();
        }
        assert!(!lnk.exists());
        // 3. Ein Verweis (symbolische Verknuepfung auf eine echte .lnk; braucht
        // das Recht dazu, sonst uebersprungen) bleibt samt Ziel.
        let echt = ordner.join("echt.lnk");
        anlegen(&echt);
        if std::os::windows::fs::symlink_file(&echt, &lnk).is_ok() {
            match alte_lnk_oeffnen(&ordner, AUTOSTART_DATEI) {
                AlteLnk::Bleibt(g) => assert!(g.contains("Verweis"), "{g}"),
                a => panic!("{a:?}"),
            }
            assert!(std::fs::symlink_metadata(&lnk).is_ok() && echt.is_file());
            std::fs::remove_file(&lnk).unwrap();
        } else {
            eprintln!("keine symbolische Verknuepfung moeglich - Teil 3 uebersprungen");
        }
        // 4. Keine lesbare Verknuepfung bleibt; ein Ordner und eine fehlende
        // Datei zaehlen als fehlend.
        std::fs::write(&lnk, b"keine Verknuepfung").unwrap();
        assert!(matches!(alte_lnk_oeffnen(&ordner, AUTOSTART_DATEI), AlteLnk::Bleibt(g) if g.contains("keine lesbare")));
        assert!(lnk.is_file());
        std::fs::create_dir(ordner.join(AUTOSTART_ALT)).unwrap();
        assert!(matches!(alte_lnk_oeffnen(&ordner, AUTOSTART_ALT), AlteLnk::Fehlt));
        assert!(matches!(alte_lnk_oeffnen(&ordner, "gibt-es-nicht.lnk"), AlteLnk::Fehlt));
        // Beim Start: ein Vermerk, kein Fehler, nichts angefasst.
        let task = format!("{}-lnk-griff", crate::secure::test_lauf());
        let b = autostart_beim_start(Some(&ordner), test_ort(&task, &ordner));
        assert_eq!(b.start, Ok(Start::Aus), "{b:?}");
        assert!(b.vermerke.len() == 1 && b.vermerke[0].contains("bleibt"), "{:?}", b.vermerke);
        assert!(b.abgeschaltet.is_empty() && lnk.is_file());
        let _ = std::fs::remove_dir_all(&ordner);
    }

    /// Systemordner und Windows-Ordner kommen vom System, nicht aus der
    /// Umgebung: schtasks.exe und icacls.exe liegen im Systemordner, und der
    /// ist System32 im Windows-Ordner. Ein Kindprozess mit falschem
    /// %SystemRoot% und %windir% (wie aus HKCU\Environment geerbt) findet
    /// dieselben Ordner, startet schtasks aus dem echten, und
    /// umgebung_absichern stellt beide Variablen richtig.
    #[cfg(windows)]
    #[test]
    fn systemordner_unabhaengig_von_der_umgebung() {
        use crate::installation::{dieselbe_datei, system_ordner, windows_ordner};
        let sys = system_ordner().unwrap();
        let win = windows_ordner().unwrap();
        assert!(sys.join("schtasks.exe").is_file() && sys.join("icacls.exe").is_file(), "{}", sys.display());
        assert!(dieselbe_datei(&sys, &win.join("System32")), "{} / {}", sys.display(), win.display());
        let falsch = std::env::temp_dir().join(format!("{}-falscher-windows-ordner", crate::secure::test_lauf()));
        std::fs::create_dir_all(falsch.join("System32")).unwrap();
        let aus = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "verknuepfung::tests::umgebung_im_kind"])
            .env("QC_UMGEBUNG_KIND", &falsch)
            .env("SystemRoot", &falsch)
            .env("windir", &falsch)
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&aus.stdout), String::from_utf8_lossy(&aus.stderr));
        assert!(aus.status.success() && text.contains("1 passed"), "{text}");
        let _ = std::fs::remove_dir_all(&falsch);
    }

    /// Hilfe fuer den Test oben: prueft nur mit QC_UMGEBUNG_KIND (dem
    /// falschen Windows-Ordner, der auch in %SystemRoot% und %windir% steht).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn umgebung_im_kind() {
        use crate::installation::{gleicher_pfad, system_ordner, umgebung_absichern, umgebung_korrigiert, windows_ordner, WINDOWS_VARIABLEN};
        let Some(falsch) = std::env::var_os("QC_UMGEBUNG_KIND") else { return };
        let falsch = falsch.to_string_lossy().to_string();
        assert_eq!(std::env::var("SystemRoot").unwrap(), falsch, "nicht geerbt");
        let sys = system_ordner().unwrap();
        assert!(!sys.starts_with(&falsch) && sys.join("schtasks.exe").is_file(), "{}", sys.display());
        // schtasks startet aus dem echten Systemordner (im falschen liegt keins).
        let (ok, _) = schtasks(&xml_args(&format!("{}-gibt-es-nicht", crate::secure::test_lauf()))).unwrap();
        assert!(!ok);
        umgebung_absichern();
        let win = windows_ordner().unwrap();
        for name in WINDOWS_VARIABLEN {
            assert!(gleicher_pfad(&std::env::var(name).unwrap(), &win.to_string_lossy()), "{name}");
        }
        assert!(umgebung_korrigiert().is_some_and(|z| z.contains(&falsch)), "{:?}", umgebung_korrigiert());
    }
}
