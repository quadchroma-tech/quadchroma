// Oberflaeche der Host-Rolle (Spezifikation Pairing v1, 10.1-10.4):
// Einzelinstanz und die Aktionen, die der Dienst ausfuehrt. Das Menue am
// Symbol baut symbolmenue.rs - fuer die eine App (Client und Host-Rolle, ein
// Symbol, main.rs) wie fuer die reine Host-Rolle (--nur-host, mod.rs). Was
// ein Menuepunkt der Host-Rolle tut, laeuft als `Aktion` ueber einen Kanal
// im Faden des Dienstes (mod.rs) - Fenster und Rueckfragen haben ihre
// eigenen Faeden (fenster.rs).

pub use crate::symbolmenue::einsetzen;
use crate::symbolmenue;

/// Name des Mutex, der die Host-Rolle einmalig macht (je Sitzung). Die eine
/// App haelt ihn, solange die Freigabe an ist; --nur-host haelt ihn, solange
/// er laeuft.
pub const MUTEX: &str = "Local\\QuadChroma-Host";

/// Was der Dienst der Host-Rolle auf einen Menuepunkt hin tut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aktion {
    IdKopieren,
    PasswortKopieren,
    PasswortAendern,
    Zufallspasswort,
    Entfernen([u8; 32]),
    AlleEntfernen,
    ListeZuruecksetzen,
    /// Nur die reine Host-Rolle: "Freigabe beenden" beendet den Dienst.
    Beenden,
    /// Aus dem Passwortfenster: gespeichert (Sprechblase).
    PasswortGespeichert,
}

impl Aktion {
    /// Die Aktion des Dienstes zu einem Menuepunkt; None fuer die Punkte, die
    /// die App selbst ausfuehrt (Oeffnen, Verbinden, Name, Schalter,
    /// Beenden der App).
    pub fn aus_menue(a: &symbolmenue::Aktion) -> Option<Aktion> {
        use symbolmenue::Aktion as M;
        Some(match a {
            M::IdKopieren => Aktion::IdKopieren,
            M::PasswortKopieren => Aktion::PasswortKopieren,
            M::PasswortAendern => Aktion::PasswortAendern,
            M::Zufallspasswort => Aktion::Zufallspasswort,
            M::Entfernen(k) => Aktion::Entfernen(*k),
            M::AlleEntfernen => Aktion::AlleEntfernen,
            M::ListeZuruecksetzen => Aktion::ListeZuruecksetzen,
            M::FreigabeBeenden => Aktion::Beenden,
            M::Oeffnen | M::Verbinden(_) | M::NameAendern | M::Freigabe | M::Autostart | M::RuheVerhindern | M::Beenden => return None,
        })
    }
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
/// laeuft schon - auch, wenn das System den Zugriff auf ihren Mutex
/// verweigert (etwa eine Freigabe mit erhoehten Rechten): dann besteht er.
/// Err: kein Mutex zu bekommen (dann ohne Einzelinstanz).
pub fn einzelinstanz(name: &str) -> Result<Option<Instanz>, String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let h = match unsafe { CreateMutexW(None, false, &HSTRING::from(name)) } {
        Ok(h) => h,
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => return Ok(None),
        Err(e) => return Err(format!("Mutex {name}: {}", e.message())),
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(h);
        }
        return Ok(None);
    }
    Ok(Some(Instanz(h.0 as isize)))
}

/// Laeuft die Host-Rolle in dieser Sitzung (ihr Mutex besteht)? Verweigert
/// das System den Zugriff, besteht er ebenfalls.
pub fn laeuft(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) } {
        Ok(h) => {
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::Eintrag;

    /// Die Punkte der Host-Rolle fuehren zu ihrer Aktion im Dienst; was die
    /// App selbst tut, nicht.
    #[test]
    fn aktionen_aus_dem_menue() {
        use symbolmenue::Aktion as M;
        assert_eq!(Aktion::aus_menue(&M::IdKopieren), Some(Aktion::IdKopieren));
        assert_eq!(Aktion::aus_menue(&M::PasswortKopieren), Some(Aktion::PasswortKopieren));
        assert_eq!(Aktion::aus_menue(&M::PasswortAendern), Some(Aktion::PasswortAendern));
        assert_eq!(Aktion::aus_menue(&M::Zufallspasswort), Some(Aktion::Zufallspasswort));
        assert_eq!(Aktion::aus_menue(&M::Entfernen([3; 32])), Some(Aktion::Entfernen([3; 32])));
        assert_eq!(Aktion::aus_menue(&M::AlleEntfernen), Some(Aktion::AlleEntfernen));
        assert_eq!(Aktion::aus_menue(&M::ListeZuruecksetzen), Some(Aktion::ListeZuruecksetzen));
        assert_eq!(Aktion::aus_menue(&M::FreigabeBeenden), Some(Aktion::Beenden));
        for a in [M::Oeffnen, M::Verbinden("h:1".into()), M::NameAendern, M::Freigabe, M::Autostart, M::RuheVerhindern, M::Beenden] {
            assert_eq!(Aktion::aus_menue(&a), None, "{a:?}");
        }
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

    /// Haelt jemand den Mutex, auf den diese Sitzung nicht zugreifen darf
    /// (wie bei einer Freigabe mit erhoehten Rechten; hier: leere
    /// Zugriffsliste), laeuft die Freigabe schon - kein zweiter Start "ohne
    /// Einzelinstanz", und `laeuft` sagt ja.
    #[test]
    fn einzelinstanz_ohne_zugriff_heisst_laeuft() {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Security::{
            InitializeAcl, InitializeSecurityDescriptor, SetSecurityDescriptorDacl, ACL, ACL_REVISION, PSECURITY_DESCRIPTOR,
            SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        };
        use windows::Win32::System::Threading::CreateMutexW;
        let name = format!("Local\\QuadChroma-Host-Test-Zugriff-{}", std::process::id());
        let mut acl = ACL::default();
        let mut sd = SECURITY_DESCRIPTOR::default();
        let psd = PSECURITY_DESCRIPTOR(&mut sd as *mut _ as *mut core::ffi::c_void);
        let h = unsafe {
            InitializeAcl(&mut acl, std::mem::size_of::<ACL>() as u32, ACL_REVISION).unwrap();
            // 1 = SECURITY_DESCRIPTOR_REVISION (Win32_System_SystemServices ist nicht eingebunden)
            InitializeSecurityDescriptor(psd, 1).unwrap();
            SetSecurityDescriptorDacl(psd, true, Some(&acl as *const ACL), false).unwrap();
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd.0,
                bInheritHandle: false.into(),
            };
            CreateMutexW(Some(&sa as *const SECURITY_ATTRIBUTES), false, &HSTRING::from(name.as_str())).unwrap()
        };
        assert!(einzelinstanz(&name).unwrap().is_none(), "Mutex ohne Zugriff galt nicht als laufende Freigabe");
        assert!(laeuft(&name));
        unsafe {
            let _ = CloseHandle(h);
        }
        assert!(!laeuft(&name));
    }

    /// Das Menue der einen App als echtes Win32-Menue (ohne Shell - geht auch
    /// in Sitzung 0): Untermenue, Haken, deaktivierte Eintraege, die Vorgabe
    /// "QuadChroma oeffnen" (auch der Doppelklick) - siehe tray_win.
    #[test]
    fn echtes_menue_der_einen_app() {
        use symbolmenue::{Art, Freigabe, MenueStand};
        use windows::Win32::UI::WindowsAndMessaging::{
            DestroyMenu, GetMenuDefaultItem, GetMenuItemCount, GetMenuState, GetSubMenu, GET_MENU_DEFAULT_ITEM_FLAGS, MF_BYPOSITION,
            MF_CHECKED, MF_GRAYED,
        };
        let s = MenueStand {
            art: Art::App,
            name: "Büro-PC".into(),
            freigabe: Freigabe::Bereit,
            id: Some(581_729_911),
            passwort: Ok("k7m-4wq-9tz".into()),
            geraete: Ok(vec![crate::zugang::Geraet { schluessel: [1; 32], datum: "2026-09-26".into(), name: "Laptop".into() }]),
            hosts: vec![("Mac".into(), "10.0.0.1:9001".into())],
            autostart: false,
            ruhe_verhindern: true,
        };
        let (m, z) = symbolmenue::menue(crate::strings::pick("de"), &s);
        let h = crate::tray_win::menue_fuer_test(&m).expect("Menue");
        let stelle = |t: &str| m.iter().position(|e| matches!(e, Eintrag::Punkt { text, .. } | Eintrag::Unter { text, .. } if text == t)).unwrap() as u32;
        unsafe {
            assert_eq!(GetMenuItemCount(Some(h)), m.len() as i32);
            assert_ne!(GetMenuState(h, 0, MF_BYPOSITION) & MF_GRAYED.0, 0, "Kopfzeile waehlbar");
            assert_eq!(GetMenuState(h, stelle("Geräte-ID: 581 729 911"), MF_BYPOSITION) & MF_GRAYED.0, 0, "ID nicht waehlbar");
            assert_ne!(GetMenuState(h, stelle("Diesen PC freigeben"), MF_BYPOSITION) & MF_CHECKED.0, 0, "kein Haken bei der Freigabe");
            assert_eq!(GetMenuState(h, stelle("Mit Windows starten"), MF_BYPOSITION) & MF_CHECKED.0, 0, "Haken bei Autostart");
            let ruhe = stelle("Ruhezustand verhindern, solange QuadChroma läuft");
            assert_ne!(GetMenuState(h, ruhe, MF_BYPOSITION) & MF_CHECKED.0, 0, "kein Haken beim Ruhezustand");
            let vorgabe = GetMenuDefaultItem(h, 0, GET_MENU_DEFAULT_ITEM_FLAGS(0));
            assert_eq!(vorgabe, symbolmenue::LINKSKLICK);
            assert_eq!(symbolmenue::aktion_zu(vorgabe, &z), Some(symbolmenue::Aktion::Oeffnen));
            let geraete = GetSubMenu(h, stelle("Erlaubte Geräte") as i32);
            assert!(!geraete.is_invalid());
            assert_eq!(GetMenuItemCount(Some(geraete)), 3);
            assert_eq!(GetMenuItemCount(Some(GetSubMenu(geraete, 0))), 1);
            let _ = DestroyMenu(h);
        }
    }
}
