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
// Kopieren: jede Datei ganz in den Speicher lesen (waehrenddessen fuer andere
// nur lesbar geoeffnet), SHA-256 bilden, unter einem Zwischennamen in den
// Zielordner schreiben, zuruecklesen und vergleichen, die Rechte der neuen
// Datei pruefen. Erst wenn alle Dateien so bereitliegen, werden sie
// umbenannt, die exe zuletzt: eine vorhandene Datei erst beiseite
// (<name>.alt-<pid>), dann die neue an ihren Platz. Die beiseite gelegten
// werden danach geloescht; laeuft die alte exe noch (etwa in einer anderen
// Sitzung), geht das erst nach dem Neustart (MOVEFILE_DELAY_UNTIL_REBOOT).
// Scheitert ein Schritt, wird zurueckgerollt.
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
// Reine Logik (Dateiliste, Pfade, SID und ACL aus Bytes, der Befehl der
// Aufgabe aus ihrem XML, die Entscheidung zum Konto) laeuft auf jeder
// Plattform und in den Tests; Dateien, Rechte und Konten hinter cfg(windows).

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

/// Befehl und Argumente der ersten Aktion aus dem XML einer geplanten
/// Aufgabe (schtasks /Query /XML): <Exec><Command>..</Command>
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

/// SHA-256 als Hex (fuer Protokoll und Vergleich).
pub fn sha256_hex(daten: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(daten).iter().map(|b| format!("{b:02x}")).collect()
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

/// Wo eine FFmpeg-DLL herkommt: neben der exe, sonst die geladene (ein
/// Entwicklungsbau findet sie ueber PATH). Ohne beides Err.
#[cfg(windows)]
fn dll_quelle(name: &str, exe_ordner: &Path) -> Result<PathBuf, String> {
    use windows::core::HSTRING;
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    let daneben = exe_ordner.join(name);
    if daneben.is_file() {
        return Ok(daneben);
    }
    unsafe {
        if let Ok(h) = GetModuleHandleW(&HSTRING::from(name)) {
            let mut puffer = vec![0u16; 4096];
            let n = GetModuleFileNameW(Some(h), &mut puffer) as usize;
            if n > 0 && n < puffer.len() {
                return Ok(PathBuf::from(String::from_utf16_lossy(&puffer[..n])));
            }
        }
    }
    Err(format!("{name} liegt weder neben {} noch ist sie geladen", exe_ordner.display()))
}

/// Die zu installierenden Dateien: (Name im Zielordner, Quelle, Pflicht) -
/// Texte zuerst, dann die DLLs, die exe zuletzt.
#[cfg(windows)]
fn dateiliste(exe: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let ordner = exe.parent().ok_or_else(|| format!("{}: ohne Ordner", exe.display()))?;
    let mut liste = Vec::new();
    for t in TEXTE {
        let p = ordner.join(t);
        if p.is_file() {
            liste.push((t.to_string(), p));
        }
    }
    for d in DLLS {
        liste.push((d.to_string(), dll_quelle(d, ordner)?));
    }
    liste.push((EXE.to_string(), exe.to_path_buf()));
    Ok(liste)
}

/// Stimmen exe und DLLs im Zielordner mit denen dieser exe ueberein
/// (SHA-256)? Fehlt eine im Ziel, nein.
#[cfg(windows)]
pub fn stimmt_ueberein(exe: &Path, ordner: &Path) -> Result<bool, String> {
    for (name, quelle) in dateiliste(exe)? {
        if TEXTE.contains(&name.as_str()) {
            continue;
        }
        let ziel = ordner.join(&name);
        if !ziel.is_file() {
            return Ok(false);
        }
        let a = sha256_hex(&lesen_gesperrt(&quelle)?);
        let b = sha256_hex(&std::fs::read(&ziel).map_err(|e| format!("{}: {e}", ziel.display()))?);
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
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
/// dort erneuern - siehe oben. Der Ordner wird angelegt, wenn er fehlt;
/// seine Rechte werden vorher, die jeder neuen Datei vor dem Umbenennen
/// geprueft.
#[cfg(windows)]
pub fn installieren(exe: &Path, ordner: &Path) -> Result<Bericht, String> {
    use std::io::Write;
    laufwerk_pruefen(ordner)?;
    if !ordner.exists() {
        std::fs::create_dir(ordner).map_err(|e| format!("{}: {e}", ordner.display()))?;
    }
    ordner_pruefen(ordner)?;
    reste_loeschen(ordner);
    let liste = dateiliste(exe)?;
    let pid = std::process::id();
    // 1. Alles unter Zwischennamen bereitlegen und pruefen.
    let mut bereit: Vec<(String, PathBuf, String)> = Vec::new();
    let aufraeumen = |bereit: &[(String, PathBuf, String)]| {
        for (_, tmp, _) in bereit {
            let _ = std::fs::remove_file(tmp);
        }
    };
    for (name, quelle) in &liste {
        let schritt = || -> Result<(PathBuf, String), String> {
            let daten = lesen_gesperrt(quelle)?;
            let summe = sha256_hex(&daten);
            let tmp = ordner.join(format!("{name}.neu-{pid}"));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            let geschrieben = f.write_all(&daten).and_then(|_| f.sync_all());
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
}
