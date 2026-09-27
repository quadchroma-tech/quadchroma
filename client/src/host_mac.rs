// Die Host-Engine des Mac im Client und damit die eine App auf dem Mac
// (Plan M4): Objective-C und C aus host/, von build.rs als libqchost.a
// hineingebaut - dieselben Quellen wie die frueher eigene QuadChroma.app,
// ohne host/start.m (dort steht deren main, das nur noch die Pruefstaende
// brauchen).
//
// Hier stehen die C-Schnittstelle aus host/dienst.h und was die App damit
// macht:
//
//   Dienst   die Host-Rolle (freigabe.rs): in resumed einrichten
//            (qc_app_einrichten - Oberflaeche, Schluessel, Zugang, ohne zu
//            lauschen), mit Freigabe qc_dienst_starten, dann
//            qc_oberflaeche_fertig (Symbol, Programmmenue); Freigabe an/aus
//            ueber qc_dienst_fortsetzen/qc_dienst_anhalten; Beenden mit
//            Abschied (qc_dienst_beenden).
//   Symbol   das EINE Symbol in der Menueleiste gehoert menue.m; hier nur,
//            was main.rs vom Symbol braucht (steht, Hinweisblase, Stand des
//            Clients: Freigabe-Haken, Ruhezustand, gefundene Hosts,
//            Tooltip, Sitzung).
//   Rueckrufe  was der Nutzer am Symbol waehlt (QC_APP_*), der Doppelklick
//            auf die laufende App und die Pruefung im Fenster
//            "Geraetename" kommen als Benutzerereignis in die
//            Ereignisschleife bzw. als Aufruf von zugang::geraetename_pruefen.
//
// Alles ausser qc_dienst_beenden, qc_dienst_anhalten, qc_dienst_fortsetzen
// und qc_dienst_name_setzen nur auf dem Hauptfaden - winit ruft main.rs dort.

use crate::symbolmenue::Aktion;
use crate::zugang;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::Mutex;

/// Rueckgaben von qc_dienst_starten und qc_app_einrichten (QC_DIENST_* in host/dienst.h).
pub const DIENST_OK: c_int = 0;
/// Ein anderer Host dieses Nutzers laeuft (host-instanz.lock).
pub const DIENST_LAEUFT_SCHON: c_int = 1;
/// In diesem Prozess schon gestartet.
pub const DIENST_DOPPELT: c_int = 2;
/// Nicht auf dem Hauptfaden gerufen.
pub const DIENST_FADEN: c_int = 3;
/// host.key unlesbar oder nicht anlegbar (bzw. --capture-Datei nicht schreibbar).
pub const DIENST_DATEI: c_int = 5;
/// qc_werkzeug: kein Werkzeug-Schalter auf der Befehlszeile.
pub const KEIN_WERKZEUG: c_int = -1;

/// Was der Rueckruf app meldet (QC_APP_* in host/dienst.h).
pub const APP_OEFFNEN: c_int = 1;
pub const APP_VERBINDEN: c_int = 2;
pub const APP_FREIGABE: c_int = 3;
pub const APP_RUHE: c_int = 4;
pub const APP_NAME: c_int = 5;
pub const APP_BEENDEN: c_int = 6;

/// qc_dienst_cfg aus host/dienst.h.
#[repr(C)]
pub struct DienstCfg {
    /// 1: im Client-Prozess - NSApp.delegate gehoert winit und bleibt unberuehrt.
    pub eingebettet: c_int,
    /// Schalter wie auf der Kommandozeile (argv[0] = Programmname); argv NULL = keine.
    pub argc: c_int,
    pub argv: *const *const c_char,
    /// Doppelklick auf die laufende App; None = Menue des Symbols zeigen.
    pub oeffnen: Option<extern "C" fn()>,
    /// Die eine App: Punkte des Clients im Menue (APP_*).
    pub app: Option<extern "C" fn(c_int, *const c_char)>,
    /// Die eine App: Pruefung im Fenster "Geraetename" (0 gut, 1 zu lang, 2 Zeichen).
    pub name_pruefen: Option<extern "C" fn(*const c_char) -> c_int>,
}

/// qc_app_stand aus host/dienst.h.
#[repr(C)]
struct AppStandC {
    freigabe: c_int,
    ruhe: c_int,
    sitzung: c_int,
    tooltip: *const c_char,
    hosts: c_int,
    host_namen: *const *const c_char,
    host_adressen: *const *const c_char,
    ruhe_grund: *const c_char,
}

extern "C" {
    /// Startet den Dienst ohne Run-Loop (Hauptfaden). DIENST_OK oder ein Fehler oben.
    pub fn qc_dienst_starten(cfg: *const DienstCfg) -> c_int;
    /// Abschied an den Zuschauer (Typ 13, Grund 0), Kanaele zu; aus jedem Faden, hoechstens rund 3 s.
    pub fn qc_dienst_beenden();
    /// Sprache, Programmmenue, Symbol in der Menueleiste - nach dem Einrichten, aus resumed.
    pub fn qc_oberflaeche_fertig();
    /// Geraete-ID des eingebauten Hosts (0, solange der Schluessel nicht geladen ist).
    fn qc_zugang_eigene_id() -> u32;
    /// Oberflaeche, Schluessel und Zugang ohne zu lauschen (Hauptfaden).
    fn qc_app_einrichten(cfg: *const DienstCfg) -> c_int;
    /// Freigabe aus: Abschied mit Grund 1, Ports zu; kehrt sofort zurueck.
    fn qc_dienst_anhalten();
    /// Freigabe wieder an nach qc_dienst_anhalten; kehrt sofort zurueck.
    fn qc_dienst_fortsetzen();
    /// 1, sobald qc_dienst_starten gelungen ist.
    fn qc_dienst_gestartet() -> c_int;
    /// Geraetename fuer Bekanntgabe, Nachricht 20 und Menue; NULL = Rechnername.
    fn qc_dienst_name_setzen(name: *const c_char);
    /// Stand des Clients fuer das Menue (Hauptfaden).
    fn qc_app_stand_setzen(s: *const AppStandC);
    /// Hinweisblase unter dem Symbol; 1 = gezeigt.
    fn qc_menueleiste_hinweis(text: *const c_char) -> c_int;
    /// 1 = das Symbol steht sichtbar in der Menueleiste.
    fn qc_menueleiste_steht() -> c_int;
    /// Das NSMenu des Symbols (Selbsttest), NULL ohne Symbol.
    fn qc_menueleiste_menue() -> *mut c_void;
    /// Das Fenster "Geraetename".
    fn qc_geraetename_fenster();
    /// Sprache der Oberflaeche nach dem Code des Clients; -1 = unbekannt.
    fn qc_texte_setzen_code(code: *const c_char) -> c_int;
    /// Werkzeuge der Kommandozeile (--list, --formattest, --capture ohne --serve).
    fn qc_werkzeug(argc: c_int, argv: *const *const c_char) -> c_int;
}

// ------------------------------------------------------------ Befehlszeile

/// Die Schalter fuer die Engine aus der Befehlszeile der App, mit dem
/// Programmnamen vorn: --serve [port], --fps N, --mbit N, --out BxH,
/// --display N, --fest (--fixed) und mit --serve auch --capture <s> <datei>.
/// Alles andere (Adresse, --hintergrund, Schalter des Clients) bleibt beim
/// Client - die Engine protokollierte es sonst als unbekannt.
pub fn host_schalter(args: &[String]) -> Vec<String> {
    let mut aus = vec!["quadchroma".to_string()];
    let mit_serve = args.iter().any(|a| a == "--serve");
    let wert = |i: usize| args.get(i).filter(|v| !v.starts_with("--")).cloned();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--fest" | "--fixed" => aus.push(a.into()),
            "--serve" => {
                aus.push(a.into());
                if let Some(p) = wert(i + 1).filter(|v| v.parse::<u16>().is_ok()) {
                    aus.push(p);
                    i += 1;
                }
            }
            "--fps" | "--mbit" | "--out" | "--display" => {
                if let Some(v) = wert(i + 1) {
                    aus.push(a.into());
                    aus.push(v);
                    i += 1;
                }
            }
            "--capture" if mit_serve => {
                if let (Some(s), Some(d)) = (wert(i + 1), wert(i + 2)) {
                    aus.extend([a.to_string(), s, d]);
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    aus
}

/// Ein Werkzeug der Engine (--list, --formattest, --capture ohne --serve):
/// laeuft zu Ende und liefert den Exit-Code; None ohne Werkzeug-Schalter.
/// Braucht eine NSApplication (wie start.m); die Werkzeuge fragen nach der
/// Bildschirmaufnahme - so sind die Ziele make list/capture/permissions
/// des Makefiles gemeint.
pub fn werkzeug(args: &[String]) -> Option<i32> {
    if !args.iter().any(|a| a == "--list" || a == "--formattest" || (a == "--capture" && !args.iter().any(|b| b == "--serve"))) {
        return None;
    }
    objc::anwendung(false);
    let c: Vec<CString> = std::iter::once("quadchroma".to_string())
        .chain(args.iter().cloned())
        .map(|a| CString::new(a.replace('\0', " ")).unwrap_or_default())
        .collect();
    let zeiger: Vec<*const c_char> = c.iter().map(|a| a.as_ptr()).collect();
    // SAFETY: argv lebt waehrend des Aufrufs; qc_werkzeug kopiert, was es braucht.
    let r = unsafe { qc_werkzeug(zeiger.len() as c_int, zeiger.as_ptr()) };
    (r != KEIN_WERKZEUG).then_some(r)
}

// ------------------------------------------------------------ Rueckrufe

/// Was eine Wahl am Symbol fuer den Client bedeutet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wahl {
    /// Ein Punkt wie am Symbol unter Windows.
    Aktion(Aktion),
    /// Aus dem Fenster "Geraetename" (geprueft; leer = Rechnername).
    Name(String),
}

/// Rueckruf app (QC_APP_*) als Wahl; Unbekanntes und "Verbinden" ohne
/// Adresse ergeben nichts.
pub fn wahl_aus(was: c_int, wert: Option<&str>) -> Option<Wahl> {
    Some(match was {
        APP_OEFFNEN => Wahl::Aktion(Aktion::Oeffnen),
        APP_VERBINDEN => Wahl::Aktion(Aktion::Verbinden(wert.map(str::trim).filter(|w| !w.is_empty())?.to_string())),
        APP_FREIGABE => Wahl::Aktion(Aktion::Freigabe),
        APP_RUHE => Wahl::Aktion(Aktion::RuheVerhindern),
        APP_NAME => Wahl::Name(wert.unwrap_or("").to_string()),
        APP_BEENDEN => Wahl::Aktion(Aktion::Beenden),
        _ => return None,
    })
}

/// Pruefung im Fenster "Geraetename" wie im Client: 0 gut, 1 zu lang, 2
/// unzulaessige Zeichen.
pub fn name_pruefen(eingabe: &str) -> c_int {
    match zugang::geraetename_pruefen(eingabe) {
        Ok(_) => 0,
        Err(zugang::NameFehler::ZuLang) => 1,
        Err(zugang::NameFehler::Zeichen) => 2,
    }
}

/// Wohin die Wahlen gehen: die Ereignisschleife (main.rs setzt den Proxy),
/// ohne sie (Selbsttest) in die Liste AUFGEZEICHNET.
static ZIEL: Mutex<Option<winit::event_loop::EventLoopProxy<crate::Benutzer>>> = Mutex::new(None);
static AUFGEZEICHNET: Mutex<Vec<Wahl>> = Mutex::new(Vec::new());

/// Die Wahlen am Symbol gehen ab jetzt als Benutzerereignis an diese Schleife.
pub fn ziel_setzen(p: winit::event_loop::EventLoopProxy<crate::Benutzer>) {
    if let Ok(mut z) = ZIEL.lock() {
        *z = Some(p);
    }
}

fn weitergeben(w: Wahl) {
    let ziel = ZIEL.lock().ok().and_then(|z| z.clone());
    match ziel {
        Some(p) => {
            let b = match w {
                Wahl::Aktion(a) => crate::Benutzer::Menue(a),
                Wahl::Name(n) => crate::Benutzer::Name(Some(n)),
            };
            let _ = p.send_event(b);
        }
        None => {
            if let Ok(mut l) = AUFGEZEICHNET.lock() {
                l.push(w);
            }
        }
    }
}

extern "C" fn app_rueckruf(was: c_int, wert: *const c_char) {
    // SAFETY: menue.m uebergibt NULL oder eine mit 0 abgeschlossene
    // Zeichenkette, die waehrend des Aufrufs gilt.
    let wert = (!wert.is_null()).then(|| unsafe { CStr::from_ptr(wert) }.to_string_lossy().into_owned());
    match wahl_aus(was, wert.as_deref()) {
        Some(w) => weitergeben(w),
        None => crate::protokoll::zeile(format!("Menueleiste: unbekannte Wahl {was} - uebergangen")),
    }
}

extern "C" fn oeffnen_rueckruf() {
    weitergeben(Wahl::Aktion(Aktion::Oeffnen));
}

extern "C" fn name_pruefen_rueckruf(eingabe: *const c_char) -> c_int {
    if eingabe.is_null() {
        return 0;
    }
    // SAFETY: wie app_rueckruf.
    match unsafe { CStr::from_ptr(eingabe) }.to_str() {
        Ok(t) => name_pruefen(t),
        Err(_) => 2,
    }
}

// ------------------------------------------------------------ Dienst

/// Die Host-Rolle der einen App auf dem Mac (hinter freigabe::Freigabe).
pub struct Dienst {
    /// Schalter fuer die Engine (host_schalter); qc_app_einrichten kopiert sie.
    args: Vec<CString>,
    /// Freigabe gewuenscht (einstellungen.txt).
    an: bool,
    /// anlaufen ist gelaufen - und ob die Oberflaeche (das Symbol) steht.
    eingerichtet: bool,
    oberflaeche: bool,
}

impl Dienst {
    pub fn neu(args: &[String], an: bool) -> Dienst {
        let args = host_schalter(args).into_iter().map(|a| CString::new(a).unwrap_or_default()).collect();
        Dienst { args, an, eingerichtet: false, oberflaeche: false }
    }

    fn mit_cfg<R>(&self, f: impl FnOnce(&DienstCfg) -> R) -> R {
        let zeiger: Vec<*const c_char> = self.args.iter().map(|a| a.as_ptr()).collect();
        let cfg = DienstCfg {
            eingebettet: 1,
            argc: zeiger.len() as c_int,
            argv: zeiger.as_ptr(),
            oeffnen: Some(oeffnen_rueckruf),
            app: Some(app_rueckruf),
            name_pruefen: Some(name_pruefen_rueckruf),
        };
        f(&cfg)
    }

    /// Aus resumed, einmal: Sprache, Oberflaeche, Schluessel und Zugang
    /// einrichten, Geraetename, mit Freigabe den Dienst starten, dann Symbol
    /// und Programmmenue (qc_oberflaeche_fertig). Hauptfaden. false: die
    /// Engine liess sich nicht einrichten (etwa host.key beschaedigt) - kein
    /// Symbol, kein Host; die App ist nur Client und braucht ihr Fenster.
    pub fn anlaufen(&mut self, sprache: &str, name: Option<&str>) -> bool {
        if self.eingerichtet {
            return self.oberflaeche;
        }
        self.eingerichtet = true;
        sprache_setzen(sprache);
        // SAFETY: Hauptfaden (resumed); cfg und argv leben waehrend des Aufrufs.
        let r = self.mit_cfg(|cfg| unsafe { qc_app_einrichten(cfg) });
        match r {
            DIENST_OK => crate::protokoll::zeile("Freigabe: Host-Engine eingerichtet (Protokoll in host-protokoll.txt)".into()),
            DIENST_DATEI => crate::protokoll::zeile("Freigabe: host.key unlesbar oder nicht anlegbar - ohne Host und ohne Symbol".into()),
            DIENST_DOPPELT => crate::protokoll::zeile("Freigabe: Host-Engine war schon eingerichtet".into()),
            DIENST_FADEN => crate::protokoll::zeile("Freigabe: nicht auf dem Hauptfaden - Host-Engine nicht eingerichtet".into()),
            r => crate::protokoll::zeile(format!("Freigabe: Host-Engine nicht eingerichtet ({r})")),
        }
        self.oberflaeche = r == DIENST_OK || r == DIENST_DOPPELT;
        if !self.oberflaeche {
            return false;
        }
        Dienst::name_setzen(name);
        if self.an {
            self.starten();
        }
        // SAFETY: Hauptfaden, nach dem Einrichten.
        unsafe { qc_oberflaeche_fertig() };
        true
    }

    fn starten(&mut self) {
        // SAFETY: Hauptfaden; cfg lebt waehrend des Aufrufs.
        let r = self.mit_cfg(|cfg| unsafe { qc_dienst_starten(cfg) });
        match r {
            DIENST_OK => crate::protokoll::zeile("Freigabe: an - der Host lauscht".into()),
            DIENST_LAEUFT_SCHON => {
                crate::protokoll::zeile("Freigabe: ein anderer QuadChroma-Host dieses Nutzers laeuft - diese bleibt aus".into())
            }
            r => crate::protokoll::zeile(format!("Freigabe: Host nicht gestartet ({r})")),
        }
    }

    /// Freigabe an oder aus - sofort. Das erste Einschalten startet den
    /// Dienst (erst dann fragt das System nach Bildschirmaufnahme und
    /// Bedienungshilfen), jedes weitere lauscht nur wieder. Hauptfaden.
    pub fn setzen(&mut self, an: bool) {
        self.an = an;
        if !self.oberflaeche {
            return;
        }
        // SAFETY: ohne Argumente; qc_dienst_starten auf dem Hauptfaden.
        unsafe {
            if !an {
                qc_dienst_anhalten();
            } else if qc_dienst_gestartet() == 0 {
                self.starten();
            } else {
                qc_dienst_fortsetzen();
            }
        }
    }

    /// Der Geraetename fuer die Engine (None: der Rechnername).
    pub fn name_setzen(name: Option<&str>) {
        let c = name.map(|n| CString::new(n).unwrap_or_default());
        // SAFETY: NULL oder eine gueltige Zeichenkette; die Engine kopiert sie.
        unsafe { qc_dienst_name_setzen(c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) };
    }

    /// Geraete-ID des eingebauten Hosts, sobald sein Schluessel geladen ist.
    pub fn id(&self) -> Option<u32> {
        // SAFETY: liest nur einen atomaren Stand.
        let id = unsafe { qc_zugang_eigene_id() };
        (self.eingerichtet && id != 0).then_some(id)
    }

    /// Abschied an einen Zuschauer (Grund 0), hoechstens rund 3 s.
    pub fn beenden(&mut self) {
        // SAFETY: aus jedem Faden erlaubt; ohne gestarteten Dienst ohne Wirkung.
        unsafe { qc_dienst_beenden() };
    }
}

/// Die Texte der Menueleiste folgen der Sprache des Clients.
pub fn sprache_setzen(code: &str) {
    let c = CString::new(code).unwrap_or_default();
    // SAFETY: gueltige Zeichenkette waehrend des Aufrufs.
    if unsafe { qc_texte_setzen_code(c.as_ptr()) } < 0 {
        crate::protokoll::zeile(format!("Menueleiste: Sprache {code} unbekannt - bleibt, wie sie war"));
    }
}

/// Das Fenster "Geraetename" (Menue oder "Umbenennen" im Startbildschirm).
pub fn geraetename_fenster() {
    // SAFETY: Hauptfaden; ohne Oberflaeche ohne Wirkung.
    unsafe { qc_geraetename_fenster() };
}

// ------------------------------------------------------------ Symbol

/// Was der Client zum Menue der Menueleiste beitraegt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stand {
    /// Haken "Diesen Mac freigeben" (der Wunsch).
    pub freigabe: bool,
    /// Haken "Ruhezustand verhindern": was gilt (nicht der Wunsch).
    pub ruhe: bool,
    /// Gewuenscht, aber vom System abgelehnt: der Grund (schon uebersetzt),
    /// als gesperrte Zeile unter dem Haken; leer: keiner.
    pub ruhe_grund: String,
    /// Eine Sitzung laeuft: Cmd+Q und Cmd+H gehen an den Mac drueben.
    pub sitzung: bool,
    pub tooltip: String,
    /// Gefundene Hosts (Name - schon entschaerft -, Adresse), hoechstens vier.
    pub hosts: Vec<(String, String)>,
}

/// Das Symbol der einen App aus Sicht von main.rs. Es selbst baut menue.m
/// (qc_oberflaeche_fertig); hier gibt es nur Stand, Hinweis und die Frage,
/// ob es zu sehen ist.
pub struct Symbol {
    zuletzt: Option<Stand>,
}

impl Symbol {
    pub fn neu() -> Symbol {
        Symbol { zuletzt: None }
    }

    /// Steht das Symbol sichtbar in der Menueleiste? Eine volle Menueleiste
    /// oder "In der Menueleiste erlauben: aus" verdraengt es.
    pub fn steht(&self) -> bool {
        // SAFETY: liest nur, Hauptfaden.
        unsafe { qc_menueleiste_steht() == 1 }
    }

    pub fn grund(&self) -> Option<String> {
        (!self.steht()).then(|| "vom System nicht angezeigt (Menueleiste voll oder ausgeblendet) oder noch nicht angelegt".into())
    }

    /// Den Stand des Clients weitergeben - nur, wenn er sich geaendert hat.
    pub fn stand_setzen(&mut self, s: &Stand) {
        if self.zuletzt.as_ref() == Some(s) {
            return;
        }
        self.zuletzt = Some(s.clone());
        let c = |t: &str| CString::new(t.replace('\0', " ")).unwrap_or_default();
        let tooltip = c(&s.tooltip);
        let ruhe_grund = c(&s.ruhe_grund);
        let namen: Vec<CString> = s.hosts.iter().map(|(n, _)| c(n)).collect();
        let adressen: Vec<CString> = s.hosts.iter().map(|(_, a)| c(a)).collect();
        let nz: Vec<*const c_char> = namen.iter().map(|n| n.as_ptr()).collect();
        let az: Vec<*const c_char> = adressen.iter().map(|a| a.as_ptr()).collect();
        let st = AppStandC {
            freigabe: s.freigabe as c_int,
            ruhe: s.ruhe as c_int,
            sitzung: s.sitzung as c_int,
            tooltip: tooltip.as_ptr(),
            hosts: nz.len() as c_int,
            host_namen: nz.as_ptr(),
            host_adressen: az.as_ptr(),
            ruhe_grund: ruhe_grund.as_ptr(),
        };
        // SAFETY: alle Zeiger leben waehrend des Aufrufs; menue.m kopiert.
        unsafe { qc_app_stand_setzen(&st) };
    }

    /// Den Stand beim naechsten stand_setzen weitergeben, auch wenn er gleich
    /// ist - etwa nach einem Sprachwechsel: menue.m baut Menue und
    /// Programmmenue dann in der neuen Sprache.
    pub fn neu_zeigen(&mut self) {
        self.zuletzt = None;
    }

    /// Die einmalige Hinweisblase unter dem Symbol; true nur, wenn sie
    /// wirklich zu sehen war.
    pub fn hinweis(&mut self, _titel: &str, text: &str) -> bool {
        let t = CString::new(text.replace('\0', " ")).unwrap_or_default();
        // SAFETY: Hauptfaden, gueltige Zeichenkette.
        unsafe { qc_menueleiste_hinweis(t.as_ptr()) == 1 }
    }

    pub fn takt(&mut self) {}
}

/// Aktivierungsart des Programms: sichtbar Regular (Dock-Symbol,
/// Programmmenue), verborgen Accessory (nur das Symbol in der Menueleiste).
pub fn aktivierung(sichtbar: bool) {
    objc::anwendung(sichtbar);
}

// ------------------------------------------------------ Vollbild

/// NSApplicationPresentationOptions (AppKit, NSApplication.h).
const AUTO_DOCK: usize = 1 << 0;
const OHNE_DOCK: usize = 1 << 1;
const AUTO_MENUELEISTE: usize = 1 << 2;
const OHNE_MENUELEISTE: usize = 1 << 3;
const VOLLBILD: usize = 1 << 10;

/// Die Praesentationsoptionen ausserhalb des Vollbilds: Menueleiste und
/// Dock wieder da. Das Vollbild der Sitzung (Fullscreen::Borderless mit
/// with_borderless_game) blendet beide ganz aus - winit 0.30.13 setzt dafuer
/// HideDock|HideMenuBar vor toggleFullScreen und nimmt es beim Verlassen
/// nicht zurueck (window_did_exit_fullscreen stellt nur Stil und Groesse
/// wieder her). Stellt AppKit beim Verlassen die Optionen von vorher her,
/// sind es eben diese - Menueleiste und Dock blieben auch ohne Vollbild weg
/// (am Geraet nicht geprueft; die Pruefung hier kostet nur ein Lesen).
/// Some(neu), wenn etwas zurueckzunehmen ist: kein Vollbild (im Uebergang
/// traegt AppKit noch FullScreen) und eine der vier Arten, Dock oder
/// Menueleiste zu verbergen, gesetzt. Dann gilt die Vorgabe 0 - die App
/// setzt sonst keine Optionen, und 0 ist immer eine erlaubte Kombination
/// (eine unerlaubte loeste eine Ausnahme aus).
pub fn leisten_optionen(jetzt: usize) -> Option<usize> {
    let verborgen = jetzt & (AUTO_DOCK | OHNE_DOCK | AUTO_MENUELEISTE | OHNE_MENUELEISTE) != 0;
    (jetzt & VOLLBILD == 0 && verborgen).then_some(0)
}

/// Ausserhalb des Vollbilds (das Fenster ist keins oder hat es verlassen):
/// Menueleiste und Dock wieder zeigen, falls das Vollbild sie verborgen
/// zuruecklaesst (siehe leisten_optionen). Die Optionen gelten ohnehin nur,
/// solange QuadChroma die aktive App ist - Cmd+Tab zu einer anderen zeigt
/// deren Menueleiste. true, wenn zurueckgesetzt wurde. Hauptfaden; nur
/// rufen, wenn winit kein Vollbild will (Window::fullscreen() None) - beim
/// Betreten setzt winit die Optionen, bevor AppKit FullScreen traegt.
pub fn leisten_zurueck() -> bool {
    objc::praesentation_zuruecksetzen(leisten_optionen)
}

// ------------------------------------------------------------ Objective-C
// Von Hand ueber objc_msgSend wie in clipboard_mac.rs: keine Kiste.

mod objc {
    use std::ffi::{c_char, c_void, CStr};

    pub type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    /// NSApplicationActivationPolicyRegular / Accessory.
    const REGULAR: isize = 0;
    const ACCESSORY: isize = 1;

    pub fn klasse(name: &CStr) -> Id {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn sel(name: &CStr) -> Sel {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// Ein objc_msgSend mit fester Signatur (je Form ein eigener
    /// Funktionszeigertyp, damit die Aufrufkonvention stimmt).
    macro_rules! senden {
        ($obj:expr, $sel:expr $(, $arg:expr => $typ:ty)* ; -> $ret:ty) => {{
            let f: unsafe extern "C" fn(Id, Sel $(, $typ)*) -> $ret = std::mem::transmute(objc_msgSend as *const c_void);
            f($obj, $sel $(, $arg)*)
        }};
    }

    pub unsafe fn id(obj: Id, s: &CStr) -> Id {
        if obj.is_null() {
            return std::ptr::null_mut();
        }
        senden!(obj, sel(s); -> Id)
    }

    pub unsafe fn id_int(obj: Id, s: &CStr, n: isize) -> Id {
        if obj.is_null() {
            return std::ptr::null_mut();
        }
        senden!(obj, sel(s), n => isize; -> Id)
    }

    pub unsafe fn int(obj: Id, s: &CStr) -> isize {
        if obj.is_null() {
            return 0;
        }
        senden!(obj, sel(s); -> isize)
    }

    pub unsafe fn nichts_int(obj: Id, s: &CStr, n: isize) {
        if !obj.is_null() {
            senden!(obj, sel(s), n => isize; -> ())
        }
    }

    /// NSString nach Rust.
    pub unsafe fn text(s: Id) -> String {
        if s.is_null() {
            return String::new();
        }
        let p = senden!(s, sel(c"UTF8String"); -> *const c_char);
        if p.is_null() {
            return String::new();
        }
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }

    /// Die RunLoop kurz laufen lassen (Selbsttest).
    pub unsafe fn laufen(sekunden: f64) {
        let bis = senden!(klasse(c"NSDate"), sel(c"dateWithTimeIntervalSinceNow:"), sekunden => f64; -> Id);
        let schleife = id(klasse(c"NSRunLoop"), c"currentRunLoop");
        if !schleife.is_null() {
            senden!(schleife, sel(c"runUntilDate:"), bis => Id; -> ())
        }
    }

    /// NSApplication anlegen (falls noch nicht) und die Aktivierungsart setzen.
    pub fn anwendung(sichtbar: bool) {
        let _pool = Pool::neu();
        unsafe {
            let app = id(klasse(c"NSApplication"), c"sharedApplication");
            if !app.is_null() {
                senden!(app, sel(c"setActivationPolicy:"), if sichtbar { REGULAR } else { ACCESSORY } => isize; -> u8);
            }
        }
    }

    /// NSApp.presentationOptions lesen; liefert `neu` einen Wert, ihn setzen.
    /// true, wenn gesetzt wurde.
    pub fn praesentation_zuruecksetzen(neu: impl FnOnce(usize) -> Option<usize>) -> bool {
        let _pool = Pool::neu();
        unsafe {
            let app = id(klasse(c"NSApplication"), c"sharedApplication");
            if app.is_null() {
                return false;
            }
            let jetzt = senden!(app, sel(c"presentationOptions"); -> usize);
            match neu(jetzt) {
                Some(n) => {
                    senden!(app, sel(c"setPresentationOptions:"), n => usize; -> ());
                    true
                }
                None => false,
            }
        }
    }

    /// Ein Autorelease-Pool fuer die Dauer eines Geltungsbereichs.
    pub struct Pool(*mut c_void);

    impl Pool {
        pub fn neu() -> Pool {
            Pool(unsafe { objc_autoreleasePoolPush() })
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }
}

// ------------------------------------------------------------ Selbsttest

/// --menueleiste-selbsttest: das eine Symbol so, wie die App es anlegt -
/// Oberflaeche, Schluessel und Zugang eingerichtet (in einem Wegwerf-HOME,
/// nie in der echten Ablage), Stand des Clients, Symbol und Programmmenue -,
/// aber OHNE den Dienst: kein Lauschen, keine Aufnahme, keine Rueckfrage des
/// Systems. Geprueft: Kopf mit Geraetename, "Freigabe ist aus", die Punkte des
/// Clients und ihre Rueckrufe (Oeffnen, Verbinden, Freigabe, Ruhezustand,
/// Beenden - ohne dass der Prozess endet), die ID des Schluessels, das
/// Programmmenue mit Cmd+Q, die Bearbeiten-Tasten nur fuer Textfelder (nicht
/// im Baum des Hauptmenues), der Sprachwechsel des Clients, die Pruefung des
/// Geraetenamens und die Hinweisblase. Rueckgabe 0 = bestanden.
pub fn selbsttest() -> i32 {
    let _pool = objc::Pool::neu();
    let start = std::time::Instant::now();
    let home = std::env::temp_dir().join(format!("qc-menueleiste-selbsttest-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(home.join("Library/Application Support")) {
        eprintln!("Menueleisten-Selbsttest: kein Wegwerf-HOME ({e})");
        return 1;
    }
    // Vor jedem Zugriff der Engine auf ihre Ablage (qc_config_path liest HOME).
    std::env::set_var("HOME", &home);
    objc::anwendung(false);
    let mut fehler = Vec::<String>::new();

    let mut d = Dienst::neu(&[], false);
    d.eingerichtet = true;
    d.oberflaeche = true;
    sprache_setzen("de");
    // SAFETY: Hauptfaden; cfg lebt waehrend des Aufrufs.
    let r = d.mit_cfg(|cfg| unsafe { qc_app_einrichten(cfg) });
    if r != DIENST_OK {
        eprintln!("Menueleisten-Selbsttest: qc_app_einrichten lieferte {r}");
        let _ = std::fs::remove_dir_all(&home);
        return 1;
    }
    Dienst::name_setzen(Some("Selbsttest-Mac"));
    let mut s = Symbol::neu();
    s.stand_setzen(&Stand {
        freigabe: false,
        ruhe: true,
        ruhe_grund: String::new(),
        sitzung: false,
        tooltip: "QuadChroma – Selbsttest".into(),
        hosts: vec![("Selbsttest".into(), "127.0.0.1:9".into())],
    });
    // SAFETY: Hauptfaden, nach dem Einrichten.
    unsafe { qc_oberflaeche_fertig() };

    // Das Symbol entsteht mit dem ersten gelesenen Zustand (eigene
    // Warteschlange, dann Main Queue) - die RunLoop drehen, bis es da ist.
    let menue = loop {
        // SAFETY: Hauptfaden.
        unsafe { objc::laufen(0.05) };
        let m = unsafe { qc_menueleiste_menue() };
        if (!m.is_null() && unsafe { objc::int(m, c"numberOfItems") } > 0) || start.elapsed().as_secs() >= 5 {
            break m;
        }
    };
    if menue.is_null() {
        eprintln!("Menueleisten-Selbsttest: kein Symbol nach 5 s");
        let _ = std::fs::remove_dir_all(&home);
        return 1;
    }
    // SAFETY: Hauptfaden; das Menue lebt, solange das Symbol lebt.
    let punkte: Vec<(String, isize, isize)> = unsafe {
        (0..objc::int(menue, c"numberOfItems"))
            .map(|i| {
                let it = objc::id_int(menue, c"itemAtIndex:", i);
                (objc::text(objc::id(it, c"title")), objc::int(it, c"state"), objc::int(it, c"tag"))
            })
            .collect()
    };
    let titel: Vec<&str> = punkte.iter().map(|p| p.0.as_str()).collect();
    let id = d.id().map(zugang::id_text).unwrap_or_default();
    let soll = [
        "QuadChroma – Selbsttest-Mac".to_string(),
        "Freigabe ist aus".into(),
        "QuadChroma öffnen".into(),
        "Verbinden: Selbsttest".into(),
        format!("Geräte-ID: {id}"),
        "Gerätename ändern …".into(),
        "Diesen Mac freigeben".into(),
        "Ruhezustand verhindern, solange QuadChroma läuft".into(),
        "QuadChroma beenden".into(),
    ];
    for t in &soll {
        if !titel.contains(&t.as_str()) {
            fehler.push(format!("Punkt \"{t}\" fehlt: {titel:?}"));
        }
    }
    if titel.first() != Some(&soll[0].as_str()) || titel.get(1) != Some(&soll[1].as_str()) {
        fehler.push(format!("Kopf und Zustand: {:?}", titel.get(..2)));
    }
    let haken = |t: &str| punkte.iter().find(|p| p.0 == t).map(|p| p.1 == 1);
    if haken("Diesen Mac freigeben") != Some(false) || haken("Ruhezustand verhindern, solange QuadChroma läuft") != Some(true) {
        fehler.push("Haken: Freigabe aus, Ruhezustand an erwartet".into());
    }
    // Jede Wahl des Clients genau als ihr Rueckruf - Beenden eingeschlossen:
    // der Prozess laeuft danach weiter (die App endet ueber winit, nicht
    // ueber terminate:).
    let wahlen = [
        ("QuadChroma öffnen", Wahl::Aktion(Aktion::Oeffnen)),
        ("Verbinden: Selbsttest", Wahl::Aktion(Aktion::Verbinden("127.0.0.1:9".into()))),
        ("Diesen Mac freigeben", Wahl::Aktion(Aktion::Freigabe)),
        ("Ruhezustand verhindern, solange QuadChroma läuft", Wahl::Aktion(Aktion::RuheVerhindern)),
        ("QuadChroma beenden", Wahl::Aktion(Aktion::Beenden)),
    ];
    for (t, soll) in wahlen {
        let Some(i) = titel.iter().position(|x| *x == t) else { continue };
        AUFGEZEICHNET.lock().map(|mut l| l.clear()).ok();
        // SAFETY: Hauptfaden, gueltiger Index.
        unsafe { objc::nichts_int(menue, c"performActionForItemAtIndex:", i as isize) };
        let ist = AUFGEZEICHNET.lock().map(|l| l.clone()).unwrap_or_default();
        if ist != [soll.clone()] {
            fehler.push(format!("\"{t}\": {ist:?} statt {soll:?}"));
        }
    }
    // Programmmenue: "QuadChroma beenden" mit Cmd+Q.
    // SAFETY: Hauptfaden.
    let cmd_q = unsafe {
        let app = objc::id(objc::klasse(c"NSApplication"), c"sharedApplication");
        let prog = objc::id(objc::id_int(objc::id(app, c"mainMenu"), c"itemAtIndex:", 0), c"submenu");
        (0..objc::int(prog, c"numberOfItems")).any(|i| {
            let it = objc::id_int(prog, c"itemAtIndex:", i);
            objc::text(objc::id(it, c"keyEquivalent")) == "q" && objc::text(objc::id(it, c"title")) == "QuadChroma beenden"
        })
    };
    if !cmd_q {
        fehler.push("Programmmenue ohne \"QuadChroma beenden\" (Cmd+Q)".into());
    }
    // Die Bearbeiten-Tasten (Cmd+Z/X/C/V/A) nicht im Baum des Hauptmenues -
    // dort schluckte es sie auch im Fenster des Clients und in einer Sitzung
    // -, sondern im QCHauptmenue, das sie nur Textfeldern gibt.
    // SAFETY: Hauptfaden; "bearbeiten" nur an einem QCHauptmenue.
    let (art, tasten, bearbeiten) = unsafe {
        let app = objc::id(objc::klasse(c"NSApplication"), c"sharedApplication");
        let haupt = objc::id(app, c"mainMenu");
        let mut tasten = Vec::new();
        for i in 0..objc::int(haupt, c"numberOfItems") {
            let unter = objc::id(objc::id_int(haupt, c"itemAtIndex:", i), c"submenu");
            for j in 0..objc::int(unter, c"numberOfItems") {
                let t = objc::text(objc::id(objc::id_int(unter, c"itemAtIndex:", j), c"keyEquivalent"));
                if !t.is_empty() {
                    tasten.push(t);
                }
            }
        }
        let art = objc::text(objc::id(haupt, c"className"));
        let bearbeiten = if art == "QCHauptmenue" { objc::int(objc::id(haupt, c"bearbeiten"), c"numberOfItems") } else { 0 };
        (art, tasten, bearbeiten)
    };
    if art != "QCHauptmenue" || bearbeiten != 6 || tasten.iter().any(|t| ["z", "Z", "x", "c", "v", "a"].contains(&t.as_str())) {
        fehler.push(format!("Hauptmenue {art} mit den Tasten {tasten:?} im Baum, {bearbeiten} Bearbeiten-Tasten fuer Textfelder"));
    }
    // Sprachwechsel im Client: Menue und Programmmenue in der neuen Sprache.
    sprache_setzen("en");
    s.neu_zeigen();
    s.stand_setzen(&Stand {
        freigabe: false,
        ruhe: true,
        ruhe_grund: String::new(),
        sitzung: false,
        tooltip: "QuadChroma – Selbsttest".into(),
        hosts: vec![("Selbsttest".into(), "127.0.0.1:9".into())],
    });
    let englisch = (0..40).any(|_| {
        // SAFETY: Hauptfaden; das Menue lebt, solange das Symbol lebt.
        unsafe {
            objc::laufen(0.05);
            let m = qc_menueleiste_menue();
            (0..objc::int(m, c"numberOfItems")).any(|i| objc::text(objc::id(objc::id_int(m, c"itemAtIndex:", i), c"title")) == "Open QuadChroma")
        }
    });
    // SAFETY: Hauptfaden.
    let prog_englisch = unsafe {
        let app = objc::id(objc::klasse(c"NSApplication"), c"sharedApplication");
        let prog = objc::id(objc::id_int(objc::id(app, c"mainMenu"), c"itemAtIndex:", 0), c"submenu");
        (0..objc::int(prog, c"numberOfItems")).any(|i| objc::text(objc::id(objc::id_int(prog, c"itemAtIndex:", i), c"title")) == "Quit QuadChroma")
    };
    if !englisch || !prog_englisch {
        fehler.push(format!("nach dem Sprachwechsel: Menue englisch {englisch}, Programmmenue englisch {prog_englisch}"));
    }
    // Pruefung des Geraetenamens: dieselbe wie im Client.
    for (eingabe, soll) in [("Büro-Mac", 0), ("", 0), (&*"x".repeat(41), 1), ("a\u{202e}b", 2)] {
        let c = CString::new(eingabe).unwrap_or_default();
        let ist = name_pruefen_rueckruf(c.as_ptr());
        if ist != soll {
            fehler.push(format!("Namenspruefung \"{eingabe}\": {ist} statt {soll}"));
        }
    }
    // Keine Freigabe, kein Dienst.
    // SAFETY: liest nur.
    if unsafe { qc_dienst_gestartet() } != 0 {
        fehler.push("der Dienst ist gestartet".into());
    }
    let steht = s.steht();
    let gezeigt = s.hinweis("QuadChroma", crate::strings::pick("de").get(crate::tray::HINWEIS));
    if gezeigt != steht {
        fehler.push(format!("Hinweisblase: gezeigt {gezeigt}, Symbol sichtbar {steht}"));
    }
    // SAFETY: Hauptfaden.
    unsafe { objc::laufen(0.2) };
    println!(
        "Menueleisten-Selbsttest: {} Punkte, ID {id}, Symbol {}, Hinweisblase {}",
        titel.len(),
        if steht { "sichtbar" } else { "vom System nicht angezeigt (Menueleiste voll oder Bildschirm gesperrt?)" },
        if gezeigt { "gezeigt" } else { "ausgelassen" }
    );
    let _ = std::fs::remove_dir_all(&home);
    if !fehler.is_empty() {
        for f in &fehler {
            eprintln!("Menueleisten-Selbsttest: FEHLER {f}");
        }
        return 1;
    }
    println!("Menueleisten-Selbsttest: bestanden, ohne Dienst und ohne Aufnahme ({} ms)", start.elapsed().as_millis());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "objc")]
    extern "C" {
        fn objc_msgSend();
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
    extern "C" {
        /// menue.m: die Geraete-ID, wie das Menue sie zeigt (NSString, autoreleased).
        fn qc_id_text(nummer: u32) -> *mut c_void;
        /// Geraete-ID aus einem oeffentlichen Schluessel (32 Byte), host/zugang.h.
        fn qc_zugang_id(schluessel: *const u8) -> u32;
        /// Anzeigeform "ddd ddd ddd" mit Nullbyte in `aus` (12 Byte), host/zugang.h.
        fn qc_zugang_id_text(id: u32, aus: *mut c_char);
    }

    /// qc_id_text aus menue.m (Menue der Menueleiste) als Rust-Text.
    fn id_text_menue(id: u32) -> String {
        // SAFETY: Objective-C-Laufzeit; der Pool haelt das autoreleased
        // NSString, bis der Text kopiert ist.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let s = qc_id_text(id);
            assert!(!s.is_null());
            let utf8: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const c_char =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let text = CStr::from_ptr(utf8(s, sel_registerName(c"UTF8String".as_ptr()))).to_str().expect("UTF-8").to_string();
            objc_autoreleasePoolPop(pool);
            text
        }
    }

    fn id_text_c(id: u32) -> String {
        let mut puffer = [0 as c_char; 12];
        // SAFETY: qc_zugang_id_text schreibt hoechstens 12 Byte samt Nullbyte (snprintf).
        unsafe { qc_zugang_id_text(id, puffer.as_mut_ptr()) };
        // SAFETY: snprintf schliesst mit einem Nullbyte ab.
        unsafe { CStr::from_ptr(puffer.as_ptr()) }.to_str().expect("nur Ziffern und Leerzeichen").to_string()
    }

    fn id_c(schluessel: &[u8; 32]) -> u32 {
        // SAFETY: qc_zugang_id liest genau 32 Byte.
        unsafe { qc_zugang_id(schluessel.as_ptr()) }
    }

    /// Die Geraete-ID aus C ist fuer feste Schluessel dieselbe wie aus Rust
    /// (zugang.rs), samt Schreibweise - im Protokoll und in der Bekanntgabe
    /// (qc_zugang_id_text, zugang.c) wie im Menue (qc_id_text, menue.m).
    #[test]
    fn id_text_aus_c_gleich_rust() {
        let feste: [([u8; 32], &str); 3] = [
            (std::array::from_fn(|i| i as u8 + 1), "581 729 911"),
            ([0u8; 32], "138 912 047"),
            ([0xffu8; 32], "309 912 962"),
        ];
        for (schluessel, erwartet) in &feste {
            let id = id_c(schluessel);
            assert_eq!(id, zugang::geraete_id(schluessel));
            assert_eq!(id_text_c(id), *erwartet);
            assert_eq!(id_text_c(id), zugang::id_text(id));
            assert_eq!(id_text_menue(id), *erwartet);
        }
        for k in 0..=255u8 {
            let schluessel: [u8; 32] = std::array::from_fn(|i| k.wrapping_mul(31).wrapping_add(i as u8));
            let id = id_c(&schluessel);
            assert_eq!(id, zugang::geraete_id(&schluessel), "Schluessel {k}");
            assert_eq!(id_text_c(id), zugang::id_text(id), "Schluessel {k}");
            assert_eq!(id_text_menue(id), zugang::id_text(id), "Schluessel {k}");
        }
        // Randwerte der Schreibweise, auch ueber der Grenze von 10^9.
        for id in [0, 5, 999_999_999, 1_000_000_000, 1_000_000_005, u32::MAX] {
            assert_eq!(id_text_c(id), zugang::id_text(id), "ID {id}");
            assert_eq!(id_text_menue(id), zugang::id_text(id), "ID {id}");
        }
    }

    /// Die Engine bekommt nur ihre Schalter: Adresse, Schalter des Clients
    /// und Werte ohne Schalter bleiben draussen; ein Schalter ohne Wert
    /// faellt weg; --capture nur mit --serve.
    #[test]
    fn schalter_fuer_die_engine() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(host_schalter(&[]), a(&["quadchroma"]));
        assert_eq!(
            host_schalter(&a(&["192.168.1.5", "--fps", "120", "--hintergrund", "--serve", "9101", "--fest", "--anzeige", "cpu", "--mbit"])),
            a(&["quadchroma", "--fps", "120", "--serve", "9101", "--fest"])
        );
        assert_eq!(host_schalter(&a(&["--serve", "--out", "1920x1080", "--display", "1"])), a(&["quadchroma", "--serve", "--out", "1920x1080", "--display", "1"]));
        assert_eq!(host_schalter(&a(&["--serve", "abc"])), a(&["quadchroma", "--serve"]));
        assert_eq!(host_schalter(&a(&["--capture", "10", "/tmp/x.hevc"])), a(&["quadchroma"]));
        assert_eq!(
            host_schalter(&a(&["--serve", "--capture", "10", "/tmp/x.hevc", "--fixed"])),
            a(&["quadchroma", "--serve", "--capture", "10", "/tmp/x.hevc", "--fixed"])
        );
        assert_eq!(host_schalter(&a(&["--fps", "--mbit", "50"])), a(&["quadchroma", "--mbit", "50"]));
    }

    /// Jede Wahl am Symbol wird genau der Punkt, den das Menue unter Windows
    /// fuer sie hat; "Verbinden" ohne Adresse und Unbekanntes ergeben nichts.
    #[test]
    fn wahl_am_symbol() {
        assert_eq!(wahl_aus(APP_OEFFNEN, None), Some(Wahl::Aktion(Aktion::Oeffnen)));
        assert_eq!(wahl_aus(APP_VERBINDEN, Some(" 10.0.0.3:9001 ")), Some(Wahl::Aktion(Aktion::Verbinden("10.0.0.3:9001".into()))));
        assert_eq!(wahl_aus(APP_VERBINDEN, Some("  ")), None);
        assert_eq!(wahl_aus(APP_VERBINDEN, None), None);
        assert_eq!(wahl_aus(APP_FREIGABE, None), Some(Wahl::Aktion(Aktion::Freigabe)));
        assert_eq!(wahl_aus(APP_RUHE, None), Some(Wahl::Aktion(Aktion::RuheVerhindern)));
        assert_eq!(wahl_aus(APP_NAME, Some("Büro-Mac")), Some(Wahl::Name("Büro-Mac".into())));
        assert_eq!(wahl_aus(APP_NAME, None), Some(Wahl::Name(String::new())));
        assert_eq!(wahl_aus(APP_BEENDEN, None), Some(Wahl::Aktion(Aktion::Beenden)));
        assert_eq!(wahl_aus(0, None), None);
        assert_eq!(wahl_aus(99, Some("x")), None);
    }

    /// Menueleiste und Dock nach dem Vollbild: zurueck auf 0 nur ohne
    /// FullScreen und nur, wenn eine Art des Verbergens gesetzt ist.
    #[test]
    fn leisten_nach_dem_vollbild() {
        assert_eq!(leisten_optionen(0), None, "nichts verborgen");
        assert_eq!(leisten_optionen(OHNE_DOCK | OHNE_MENUELEISTE), Some(0), "von winit zurueckgelassen");
        assert_eq!(leisten_optionen(AUTO_DOCK | AUTO_MENUELEISTE), Some(0));
        assert_eq!(leisten_optionen(OHNE_DOCK), Some(0));
        assert_eq!(leisten_optionen(VOLLBILD | OHNE_DOCK | OHNE_MENUELEISTE), None, "im Vollbild bleiben sie aus");
        assert_eq!(leisten_optionen(VOLLBILD | AUTO_DOCK | AUTO_MENUELEISTE), None, "auch im Uebergang");
        assert_eq!(leisten_optionen(1 << 9), None, "andere Optionen allein bleiben");
        assert_eq!((OHNE_DOCK, OHNE_MENUELEISTE, VOLLBILD), (2, 8, 1024), "Werte aus NSApplication.h");
    }

    /// Die Pruefung im Fenster "Geraetename" ist die des Clients.
    #[test]
    fn namenspruefung_wie_im_client() {
        for (eingabe, soll) in [("Büro-Mac", 0), ("  ", 0), ("", 0), (&*"ü".repeat(20), 0), (&*"ü".repeat(21), 1), ("a\tb", 2), ("\u{2066}x", 2)] {
            assert_eq!(name_pruefen(eingabe), soll, "{eingabe:?}");
            let c = CString::new(eingabe).unwrap();
            assert_eq!(name_pruefen_rueckruf(c.as_ptr()), soll, "{eingabe:?}");
        }
        assert_eq!(name_pruefen_rueckruf(std::ptr::null()), 0);
        // Kein gueltiges UTF-8 (kommt aus AppKit nicht vor): unzulaessig.
        let kaputt = [0xC3u8, 0x28, 0];
        assert_eq!(name_pruefen_rueckruf(kaputt.as_ptr() as *const c_char), 2);
    }
}
