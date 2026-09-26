// Oberflaeche der Host-Rolle (Spezifikation Pairing v1, 10.1-10.4):
// Einzelinstanz, Start als zweiter Prozess ohne Konsole, das Menue im
// Infobereich und was seine Punkte tun.
//
// Das Menue (tray_win, zweite Art) wird bei jedem Oeffnen frisch gebaut:
//   QuadChroma / Zustand ("Bereit fuer Verbindungen", "Verbunden: <Name>",
//   "Port 9001 ist ... belegt") - nur Anzeige
//   Geraete-ID: 581 729 911          Klick kopiert die 9 Ziffern
//   Passwort: k7m-4wq-9tz            Klick kopiert (verdeckt, clipboard::set)
//   Passwort aendern ... / Neues Zufallspasswort
//   Erlaubte Geraete >  je Geraet "<Name> - ID ... - seit ..." > Entfernen,
//                       "Alle Geraete entfernen ..." (Rueckfrage);
//                       leer: "Noch keine Geraete"
//   (beschaedigte Liste: "Geraeteliste beschaedigt" + "... zuruecksetzen")
//   [x] Mit Windows starten          Verknuepfung im Autostart-Ordner
//   Freigabe beenden
// Gewaehlt wird im Symbolfaden; was ein Punkt tut, laeuft als `Aktion` ueber
// einen Kanal im Hauptfaden der Host-Rolle (mod.rs) - Fenster und
// Rueckfragen haben ihre eigenen Faeden (fenster.rs).
//
// Aufbau und Zuordnung der Nummern sind reine Rechnung (MenueStand ->
// Eintraege), ohne Fenster testbar.

use crate::strings::{Key, Lang};
use crate::tray_win::Eintrag;
use crate::zugang::{self, Geraet};

/// Name des Mutex, der die Host-Rolle einmalig macht (je Sitzung).
pub const MUTEX: &str = "Local\\QuadChroma-Host";
/// Internes Argument des zweiten Prozesses (Knopf "Diesen PC freigeben"):
/// keine Konsole des Aufrufers erben.
pub const HINTERGRUND: &str = "--hintergrund";

// Befehlsnummern im Menue; Geraete ab NR_GERAET + Stelle in der Liste.
const NR_ID: u32 = 1;
const NR_PASSWORT: u32 = 2;
const NR_AENDERN: u32 = 3;
const NR_ZUFALL: u32 = 4;
const NR_ALLE: u32 = 5;
const NR_ZURUECKSETZEN: u32 = 6;
const NR_AUTOSTART: u32 = 7;
const NR_BEENDEN: u32 = 8;
const NR_GERAET: u32 = 1000;

/// Was ein Menuepunkt ausloest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aktion {
    IdKopieren,
    PasswortKopieren,
    PasswortAendern,
    Zufallspasswort,
    Entfernen([u8; 32]),
    AlleEntfernen,
    ListeZuruecksetzen,
    /// "Mit Windows starten" umschalten.
    Autostart,
    Beenden,
    /// Aus dem Passwortfenster: gespeichert (Sprechblase).
    PasswortGespeichert,
}

/// Was das Menue zeigt, als Daten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenueStand {
    /// Name des verbundenen Zuschauers.
    pub zuschauer: Option<String>,
    /// Der Port liess sich nicht binden.
    pub port_belegt: Option<u16>,
    pub id: u32,
    /// Das Passwort; Err: Datei unlesbar.
    pub passwort: Result<String, ()>,
    /// Die erlaubten Geraete; Err: Liste unlesbar oder beschaedigt.
    pub geraete: Result<Vec<Geraet>, ()>,
    pub autostart: bool,
}

/// Der Tooltip: "QuadChroma - Freigabe laeuft (ID ...)".
pub fn tooltip(lang: &Lang, id: u32) -> String {
    lang.get(Key::HostTooltip).replace("{i}", &zugang::id_text(id))
}

/// Die Eintraege des Menues und die Schluessel der Geraete in Menue-Reihenfolge
/// (fuer `aktion_zu`).
pub fn menue(lang: &Lang, s: &MenueStand) -> (Vec<Eintrag>, Vec<[u8; 32]>) {
    let mut m = Vec::new();
    let mut schluessel = Vec::new();
    let zustand = if let Some(p) = s.port_belegt {
        lang.get(Key::HostPortBusy).replace("{p}", &p.to_string())
    } else if let Some(n) = &s.zuschauer {
        lang.get(Key::HostConnected).replace("{n}", n)
    } else {
        lang.get(Key::HostReady).to_string()
    };
    m.push(Eintrag::anzeige("QuadChroma"));
    m.push(Eintrag::anzeige(zustand));
    m.push(Eintrag::Trenner);
    m.push(Eintrag::punkt(lang.get(Key::HostDeviceId).replace("{i}", &zugang::id_text(s.id)), NR_ID));
    match &s.passwort {
        Ok(pw) => m.push(Eintrag::punkt(lang.get(Key::HostPassword).replace("{p}", pw), NR_PASSWORT)),
        Err(()) => m.push(Eintrag::anzeige(lang.get(Key::HostPasswordUnreadable))),
    }
    m.push(Eintrag::punkt(lang.get(Key::HostChangePassword), NR_AENDERN));
    m.push(Eintrag::punkt(lang.get(Key::HostRandomPassword), NR_ZUFALL));
    m.push(Eintrag::Trenner);
    match &s.geraete {
        Ok(liste) => {
            let mut unter = Vec::new();
            if liste.is_empty() {
                unter.push(Eintrag::anzeige(lang.get(Key::HostNoDevices)));
            }
            for g in liste {
                let zeile = lang
                    .get(Key::HostDeviceLine)
                    .replace("{n}", &g.name)
                    .replace("{i}", &zugang::id_text(g.id()))
                    .replace("{d}", &g.datum);
                unter.push(Eintrag::Unter {
                    text: zeile,
                    eintraege: vec![Eintrag::punkt(lang.get(Key::HostRemove), NR_GERAET + schluessel.len() as u32)],
                    aktiv: true,
                });
                schluessel.push(g.schluessel);
            }
            if !liste.is_empty() {
                unter.push(Eintrag::Trenner);
                unter.push(Eintrag::punkt(lang.get(Key::HostRemoveAll), NR_ALLE));
            }
            m.push(Eintrag::Unter { text: lang.get(Key::HostDevices).to_string(), eintraege: unter, aktiv: true });
        }
        Err(()) => {
            m.push(Eintrag::anzeige(lang.get(Key::HostListDamaged)));
            m.push(Eintrag::punkt(lang.get(Key::HostListReset), NR_ZURUECKSETZEN));
        }
    }
    m.push(Eintrag::Trenner);
    m.push(Eintrag::Punkt {
        text: lang.get(Key::HostStartWindows).to_string(),
        nummer: NR_AUTOSTART,
        haken: s.autostart,
        aktiv: true,
        fett: false,
    });
    m.push(Eintrag::Trenner);
    m.push(Eintrag::punkt(lang.get(Key::HostStopSharing), NR_BEENDEN));
    (m, schluessel)
}

/// Die Aktion zur gewaehlten Nummer, aufgeloest gegen die Schluessel des
/// Menues, aus dem gewaehlt wurde.
pub fn aktion_zu(nr: u32, schluessel: &[[u8; 32]]) -> Option<Aktion> {
    Some(match nr {
        NR_ID => Aktion::IdKopieren,
        NR_PASSWORT => Aktion::PasswortKopieren,
        NR_AENDERN => Aktion::PasswortAendern,
        NR_ZUFALL => Aktion::Zufallspasswort,
        NR_ALLE => Aktion::AlleEntfernen,
        NR_ZURUECKSETZEN => Aktion::ListeZuruecksetzen,
        NR_AUTOSTART => Aktion::Autostart,
        NR_BEENDEN => Aktion::Beenden,
        n if n >= NR_GERAET => Aktion::Entfernen(*schluessel.get((n - NR_GERAET) as usize)?),
        _ => return None,
    })
}

// ------------------------------------------------------ Einzelinstanz

/// Der Mutex der Host-Rolle, solange sie laeuft.
pub struct Instanz(isize);

impl Drop for Instanz {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(self.0 as *mut _));
        }
    }
}

/// Die Host-Rolle fuer diese Sitzung beanspruchen. Ok(None): eine andere
/// laeuft schon. Err: kein Mutex zu bekommen (dann ohne Einzelinstanz).
pub fn einzelinstanz(name: &str) -> Result<Option<Instanz>, String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let h = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.map_err(|e| format!("Mutex {name}: {}", e.message()))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(h);
        }
        return Ok(None);
    }
    Ok(Some(Instanz(h.0 as isize)))
}

/// Laeuft die Host-Rolle in dieser Sitzung (ihr Mutex besteht)?
pub fn laeuft(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) } {
        Ok(h) => {
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        Err(_) => false,
    }
}

/// Startet die Host-Rolle als zweiten Prozess derselben exe (Spezifikation
/// 9.5/10.1, Knopf "Diesen PC freigeben" im Client): `--host --hintergrund`,
/// ohne Konsolenfenster (die exe ist ein GUI-Programm; DETACHED_PROCESS
/// dazu, damit sie auch keine Konsole eines aus der Eingabeaufforderung
/// gestarteten Clients erbt - schloesse die jemand, endete die Freigabe
/// mit). Laeuft schon eine, beendet sich die neue still (Einzelinstanz).
pub fn freigabe_starten() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
    Command::new(exe)
        .args(["--host", HINTERGRUND])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Freigabe nicht gestartet: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geraet(n: u8, name: &str) -> Geraet {
        Geraet { schluessel: [n; 32], datum: "2026-09-26".into(), name: name.into() }
    }

    fn stand() -> MenueStand {
        MenueStand {
            zuschauer: None,
            port_belegt: None,
            id: 581_729_911,
            passwort: Ok("k7m-4wq-9tz".into()),
            geraete: Ok(vec![geraet(1, "Laptop"), geraet(2, "Büro-PC")]),
            autostart: true,
        }
    }

    fn texte(e: &[Eintrag]) -> Vec<String> {
        e.iter()
            .map(|x| match x {
                Eintrag::Punkt { text, .. } | Eintrag::Unter { text, .. } => text.clone(),
                Eintrag::Trenner => "-".into(),
            })
            .collect()
    }

    /// Das ganze Menue auf Deutsch: Kopf, ID, Passwort, Geraete mit
    /// Untermenue, Haken bei "Mit Windows starten", Freigabe beenden.
    #[test]
    fn menue_mit_geraeten() {
        let de = crate::strings::pick("de");
        let (m, k) = menue(de, &stand());
        assert_eq!(
            texte(&m),
            [
                "QuadChroma",
                "Bereit für Verbindungen",
                "-",
                "Geräte-ID: 581 729 911",
                "Passwort: k7m-4wq-9tz",
                "Passwort ändern …",
                "Neues Zufallspasswort",
                "-",
                "Erlaubte Geräte",
                "-",
                "Mit Windows starten",
                "-",
                "Freigabe beenden",
            ]
        );
        assert_eq!(k, vec![[1u8; 32], [2u8; 32]]);
        // Kopfzeilen nur zur Anzeige.
        assert!(matches!(&m[0], Eintrag::Punkt { aktiv: false, nummer: 0, .. }));
        assert!(matches!(&m[1], Eintrag::Punkt { aktiv: false, nummer: 0, .. }));
        assert!(matches!(&m[10], Eintrag::Punkt { haken: true, aktiv: true, .. }));
        let Eintrag::Unter { eintraege, .. } = &m[8] else { panic!("{:?}", m[8]) };
        let id = |n: u8| zugang::id_text(zugang::geraete_id(&[n; 32]));
        assert_eq!(
            texte(eintraege),
            [
                format!("Laptop – ID {} – seit 2026-09-26", id(1)),
                format!("Büro-PC – ID {} – seit 2026-09-26", id(2)),
                "-".to_string(),
                "Alle Geräte entfernen …".to_string(),
            ]
        );
        // Jedes Geraet hat ein Untermenue "Entfernen", das auf seinen Schluessel fuehrt.
        for (i, e) in eintraege[..2].iter().enumerate() {
            let Eintrag::Unter { eintraege: u, .. } = e else { panic!() };
            let Eintrag::Punkt { text, nummer, .. } = &u[0] else { panic!() };
            assert_eq!(text, "Entfernen");
            assert_eq!(aktion_zu(*nummer, &k), Some(Aktion::Entfernen([i as u8 + 1; 32])));
        }
    }

    /// Alle waehlbaren Nummern fuehren zu ihrer Aktion; fremde zu nichts.
    #[test]
    fn nummern_und_aktionen() {
        let k = [[9u8; 32]];
        assert_eq!(aktion_zu(NR_ID, &k), Some(Aktion::IdKopieren));
        assert_eq!(aktion_zu(NR_PASSWORT, &k), Some(Aktion::PasswortKopieren));
        assert_eq!(aktion_zu(NR_AENDERN, &k), Some(Aktion::PasswortAendern));
        assert_eq!(aktion_zu(NR_ZUFALL, &k), Some(Aktion::Zufallspasswort));
        assert_eq!(aktion_zu(NR_ALLE, &k), Some(Aktion::AlleEntfernen));
        assert_eq!(aktion_zu(NR_ZURUECKSETZEN, &k), Some(Aktion::ListeZuruecksetzen));
        assert_eq!(aktion_zu(NR_AUTOSTART, &k), Some(Aktion::Autostart));
        assert_eq!(aktion_zu(NR_BEENDEN, &k), Some(Aktion::Beenden));
        assert_eq!(aktion_zu(NR_GERAET, &k), Some(Aktion::Entfernen([9u8; 32])));
        assert_eq!(aktion_zu(NR_GERAET + 1, &k), None);
        assert_eq!(aktion_zu(0, &k), None);
        assert_eq!(aktion_zu(99, &k), None);
    }

    /// Zustaende: verbunden, Port belegt, keine Geraete, beschaedigte Liste,
    /// unlesbares Passwort, kein Autostart; dazu der Tooltip.
    #[test]
    fn menue_zustaende() {
        let de = crate::strings::pick("de");
        let en = crate::strings::pick("en");
        let mut s = stand();
        s.zuschauer = Some("Laptop".into());
        assert_eq!(texte(&menue(de, &s).0)[1], "Verbunden: Laptop");
        s.port_belegt = Some(9001);
        assert_eq!(texte(&menue(de, &s).0)[1], "Port 9001 ist von einem anderen Programm belegt");
        assert_eq!(texte(&menue(en, &s).0)[1], "Port 9001 is used by another program");
        let mut s = stand();
        s.geraete = Ok(Vec::new());
        s.autostart = false;
        let (m, k) = menue(de, &s);
        assert!(k.is_empty());
        let Eintrag::Unter { eintraege, .. } = &m[8] else { panic!() };
        assert_eq!(texte(eintraege), ["Noch keine Geräte"]);
        assert!(matches!(&eintraege[0], Eintrag::Punkt { aktiv: false, .. }));
        assert!(matches!(&m[10], Eintrag::Punkt { haken: false, .. }));
        let mut s = stand();
        s.geraete = Err(());
        s.passwort = Err(());
        let (m, _) = menue(de, &s);
        let t = texte(&m);
        assert_eq!(t[4], "Passwortdatei unlesbar – neue Geräte nur über „Zulassen“.");
        assert!(matches!(&m[4], Eintrag::Punkt { aktiv: false, .. }));
        assert_eq!(t[8], "Geräteliste beschädigt");
        assert!(matches!(&m[9], Eintrag::Punkt { nummer: NR_ZURUECKSETZEN, aktiv: true, .. }));
        assert_eq!(t[9], "Geräteliste zurücksetzen");
        assert_eq!(tooltip(de, 581_729_911), "QuadChroma – Freigabe läuft (ID 581 729 911)");
        assert_eq!(tooltip(en, 5), "QuadChroma - sharing this PC (ID 000 000 005)");
    }

    /// Einzelinstanz: der zweite Anspruch auf denselben Namen scheitert,
    /// `laeuft` sieht den Mutex; nach dem Ende ist er frei. Eigener Name -
    /// nie der einer laufenden Freigabe.
    #[test]
    fn einzelinstanz_je_name() {
        let name = format!("Local\\QuadChroma-Host-Test-{}", std::process::id());
        assert!(!laeuft(&name));
        let erste = einzelinstanz(&name).unwrap().expect("erste Instanz");
        assert!(laeuft(&name));
        assert!(einzelinstanz(&name).unwrap().is_none(), "zweite Instanz bekam den Mutex");
        drop(erste);
        assert!(!laeuft(&name));
        assert!(einzelinstanz(&name).unwrap().is_some());
    }

    /// Das allgemeine Menue als echtes Win32-Menue (ohne Shell): Untermenue,
    /// Haken, deaktivierte Eintraege - siehe tray_win.
    #[test]
    fn echtes_menue_der_host_rolle() {
        let (m, _) = menue(crate::strings::pick("de"), &stand());
        let h = crate::tray_win::menue_fuer_test(&m).expect("Menue");
        use windows::Win32::UI::WindowsAndMessaging::{DestroyMenu, GetMenuItemCount, GetMenuState, GetSubMenu, MF_BYPOSITION, MF_CHECKED, MF_GRAYED};
        unsafe {
            assert_eq!(GetMenuItemCount(Some(h)), m.len() as i32);
            assert_ne!(GetMenuState(h, 0, MF_BYPOSITION) & MF_GRAYED.0, 0, "Kopfzeile waehlbar");
            assert_eq!(GetMenuState(h, 3, MF_BYPOSITION) & MF_GRAYED.0, 0, "ID nicht waehlbar");
            assert_ne!(GetMenuState(h, 10, MF_BYPOSITION) & MF_CHECKED.0, 0, "kein Haken bei Autostart");
            let geraete = GetSubMenu(h, 8);
            assert!(!geraete.is_invalid());
            assert_eq!(GetMenuItemCount(Some(geraete)), 4);
            let erstes = GetSubMenu(geraete, 0);
            assert_eq!(GetMenuItemCount(Some(erstes)), 1);
            let _ = DestroyMenu(h);
        }
    }
}
