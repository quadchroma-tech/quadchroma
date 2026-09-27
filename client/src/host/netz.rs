// Zuschauerplatz des Windows-Hosts: Annahme auf dem Bildkanal (port) und
// dem Eingabekanal (port + 1), Bekanntgabe per Rundruf (port + 2), Versand
// mit Stauregel. Wer nach dem Handschlag herein darf, entscheidet der
// Einlass (einlass.rs: Geraeteliste, Passwort, "Zulassen").
//
// Ein Zuschauer zur Zeit; ein neuer ersetzt den alten (wie main.m). Der
// alte bekommt als letzte Nachricht MSG_ABGELOEST - statt dessen, was noch
// in seiner Warteschlange lag -, damit er sich nicht von selbst neu
// verbindet und den neuen verdraengt; danach sind seine Bildleitung und sein
// Eingabekanal zu. Vor dem Schlusswort geht noch der Rest des Pakets hinaus,
// das der Sendefaden gerade schreibt: solange der Alte davon abnimmt, wird
// gewartet (hoechstens ABLOESUNG_HOECHSTENS), erst ohne Fortschritt gekappt.
// Genauso verabschiedet der Host einen Zuschauer (zuschauer_verabschieden,
// abschied_beim_beenden): Schlusswort MSG_HOST_ENDE mit Grund, wenn die
// Freigabe endet, die Host-Rolle beendet wird oder sein Geraet aus der Liste
// entfernt wird - auch dann verbindet sich der Client nicht von selbst neu.
// Der Bildkanal wird nur geschrieben, der Eingabekanal nur gelesen -
// Antworten (Zeit, Einstellungen) gehen ueber den Bildkanal zurueck.
//
// Stauregel: Windows kennt kein SO_NWRITE. Ersatz ist ein eigener
// Sendefaden mit Warteschlange bei einem Sendepuffer von 256 kB im Kernel:
// ein blockierender write im Sendefaden spiegelt die Leitung, die
// Warteschlange davor ist der Stau. Rueckstand = Warteschlange plus der
// Rest des Pakets, das der Sendefaden gerade schreibt. Entschieden wird wie
// auf dem Mac (stau_vor_dem_encoder in main.m) VOR dem Encoder: liegt mehr
// als das Budget (2 MB, Gaming 512 kB) im Rueckstand, geht das naechste
// Bild gar nicht erst in den Encoder (Zaehler "Stau"). Ein codiertes Bild
// geht immer hinaus - nur ueber der harten Grenze (HARTE_GRENZE) faellt es
// weg, und dann wird ein Vollbild erzwungen. Nimmt der Zuschauer im Stau
// STAU_FRIST_US lang nichts ab, gilt er als weg. send_small (Ton,
// Zwischenablage, Zeiger, Info, Einstellungen, Codecs, Switch, Zeit, Last,
// Hoststatus) geht an der Regel vorbei, aber durch dieselbe Warteschlange -
// die Reihenfolge Switch -> Info -> Vollbild bleibt damit erhalten. Grenzen
// hat es trotzdem, je Art gezaehlt: Ton faellt ueber ton_grenze weg (liegt
// die Leitung unter der Tonrate, fuellte er sonst die Warteschlange ohne
// Ende, und kein Bild kaeme mehr durch); wer ueber KLEIN_GRENZE an
// Steuernachrichten nicht abnimmt, ist weg - die werden nie still verworfen.
//
// Sitzung: Der Eingabekanal gehoert zu genau einem Zuschauer. Geht der
// (ersetzt: `Leitung::abloesen`, weg: `Leitung::schliessen`), wird auch
// dessen Eingabe gekappt, und die Eingabeschleife speist nichts mehr ein.
// Handschlaege laufen je
// Verbindung in einem eigenen Faden, mit Frist (secure.rs, FRIST_ANNAHME) und
// Obergrenze (HANDSCHLAEGE_MAX) - stumme Verbindungen sperren damit niemanden
// mehr aus. Was jede Verbindung ins Protokoll bringen kann, ohne erlaubt
// zu sein (gescheiterter Handschlag, Zugangsphase), laeuft ueber eine
// Drossel je Art und Adresse (Drossel, drosseln_nachtragen).
// Windows bricht ein blockierendes recv/send auf ein shutdown hin nicht ab,
// solange die Gegenstelle lebt, aber schweigt (eingefrorener Prozess) - der
// Faden des alten Kanals bliebe samt Leitung stehen, bis sie geht. Deshalb
// bricht `kappen` zusaetzlich mit CancelIoEx ab, was an der Leitung haengt.
// Die Eingabeschleife prueft ohnehin vor jedem Einspeisen, ob sie noch gilt.
//
// Dateien ueber die Zwischenablage (Spezifikation 2 und 3.3, Kern in
// dateien.rs): Jeder neue Zuschauer bekommt nach der Begruessung
// MSG_FAEHIGKEITEN; was sein Eingabekanal mit IN_FAEHIGKEITEN meldet, gilt
// nur fuer genau diesen Kanal (Warteschlange::faehig). Client -> Host: der
// Eingabekanal liest 50-52 ausserhalb aller Sperren und gibt sie unter
// EINSPEISEN ohne Plattenarbeit an den Empfaenger dieses Zuschauers; dessen
// Quittungen gehen als kleine Steuernachricht (Art::Klein) nur an ihn
// (klein_an) - nie hinter den einen Datei-Platz des eigenen Senders. Host
// -> Client: der Sender reiht seine Pakete als Art::Datei ein (datei_an):
// hoechstens eines wartet, und nur bei hoechstens DATEI_RUECKSTAND
// Rueckstand in der Warteschlange - sonst Gesendet::Voll. Art::Datei zaehlt
// nicht gegen KLEIN_GRENZE. Was schon beim Kernel liegt (Sendepuffer bis
// SNDBUF, 256 KiB), sieht diese Regel nicht; die Dateidaten dort, in der
// Warteschlange und auf der Leitung zusammen begrenzt das Fenster, und das
// ist fuer die Host-Rolle fest hoechstens DATEI_FENSTER_HOST (64 KiB) -
// Windows kennt kein SO_NWRITE, mit dem der Mac-Host den Kernelpuffer
// selbst misst. Vor einem Bild liegen also hoechstens 128 KiB Rueckstand
// und ein Datei-Paket (ein Stueck bis 48 KiB; das Angebot einmal am Stueck,
// bis 1 MiB - bewusst so gelassen, es ist ein Paket des Protokolls), dazu
// im Kernel hoechstens 64 KiB Dateidaten. Beide Wege sind an die
// Zuschauernummer gebunden (Muster send_small_an): nach einem Wechsel melden
// sie Gesendet::Weg. Abloesen und Schliessen brechen Sender und Empfaenger
// des Zuschauers ab. Kopiert der Nutzer Dateien, waehrend der aktuelle
// Eingabekanal noch keine Faehigkeiten gemeldet hat (gleich nach dem
// Verbinden), wird die Liste bis VORMERKEN (5 s) vorgemerkt und geht
// hinaus, sobald IN_FAEHIGKEITEN kommt. Die Host-Rolle hat eine eigene
// Ablagebasis (host_ablage_basis), damit Client und Host-Rolle auf einem
// Rechner einander nichts aufraeumen.
// Sperrreihenfolge: EINSPEISEN, AKTUELL, dann entweder Leitung.q oder
// Leitung.dateien - diese beiden nie ineinander. LAUF (start, stoppen)
// kommt vor AKTUELL.
//
// Starten und Stoppen: `start` bindet beide Ports und startet die beiden
// Annahmefaeden und die Bekanntgabe - ein Lauf mit eigener Laufmarke
// (Schluessel, gilt). `stoppen` nimmt der Marke unter AKTUELL die Geltung,
// weckt die Annahmefaeden mit je einer Verbindung an ihren eigenen Port
// (ein blockiertes accept laesst sich sonst nicht unterbrechen) und die
// Bekanntgabe aus ihrer Pause, und wartet, bis alle drei geendet und damit
// die Ports freigegeben haben. Wer seinen Handschlag erst danach beendet,
// wird kein Zuschauer mehr. Ein verbundener Zuschauer bleibt verbunden -
// wer die Freigabe beendet, verabschiedet ihn (abschied_beim_beenden).
// Danach laesst sich `start` wieder rufen.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::{eingabe, encoder, log, now_us, Z};
use crate::dateien::{self, Gesendet};
use crate::protokoll_konst::*;
use crate::{noise, secure, zugang};

/// Rueckstand, ab dem Bilder gar nicht erst in den Encoder gehen
/// (stau_vor_dem_encoder); im Spielmodus ein Viertel. Wie QC_BACKLOG_LIMIT
/// in main.m: ein Vollbild wird selbst fast so gross, und nach einem
/// Vollbild soll auf einer gesunden Leitung kein Bild ausfallen.
const BACKLOG_LIMIT: usize = 2 * 1024 * 1024;
/// Harte Grenze fuer schon codierte Bilder: nur wenn der Rueckstand darueber
/// liegt, faellt eines weg (mit erzwungenem Vollbild). Die Regel vor dem
/// Encoder haelt ihn bei BACKLOG_LIMIT plus dem, was beim Einsetzen des
/// Staus noch im Encoder steckt (hoechstens INFLIGHT_AUFNAHME Bilder); die
/// harte Grenze faengt nur ungebremstes Wachstum ab - etwa die Konserve,
/// die an keinem Encoder vorbeikommt. So gross wie der Sendepuffer des
/// Mac-Hosts, ueber den hinaus dort das Senden blockiert.
const HARTE_GRENZE: usize = 2 * BACKLOG_LIMIT;
/// Stau ohne Fortschritt: so lange, dann gilt der Zuschauer als weg - wie
/// QC_STAU_FRIST_US in main.m. Ohne diese Frist bliebe ein eingefrorener
/// Zuschauer (Ton aus) fuer immer eingetragen: im Stau geht nichts mehr in
/// den Encoder, und der Sendefaden haengt ohne Frist in seinem write.
const STAU_FRIST_US: u64 = 2_000_000;
/// Sendepuffer im Kernel. Was dort liegt, zaehlt nicht zum Rueckstand -
/// hoechstens so viel kommt zur Warteschlange noch dazu.
const SNDBUF: usize = 256 * 1024;
/// Der Sendefaden schreibt ein Paket in Stuecken dieser Groesse: genau ein
/// Noise-Stueck (secure::write_all teilt ebenso), auf der Leitung also
/// dieselben Bytes wie am Stueck. So sieht die Stauregel, wie weit ein
/// grosses Paket (Vollbild) schon ist, und ein langsamer, aber lebender
/// Zuschauer macht auch mitten in einem Vollbild Fortschritt.
const SCHREIBSTUECK: usize = secure::CHUNK_MAX;
/// So lange darf die letzte Nachricht an einen abgeloesten (oder
/// verabschiedeten) Zuschauer brauchen. Seine Warteschlange ist da schon verworfen; vor ihr liegen
/// hoechstens der Rest des Pakets, das der Sendefaden gerade schreibt, und
/// der Kernelpuffer. Wer sie in der Frist nicht abnimmt, ist eingefroren oder
/// weg: seine Leitung wird ohne sie gekappt. Der Rest des Pakets selbst hat
/// keine feste Frist - siehe `abloesung_abschliessen`.
const FRIST_SCHLUSSWORT: Duration = Duration::from_secs(1);
/// Obergrenze fuer das Warten auf einen abgeloesten Zuschauer, der zwar
/// abnimmt, aber langsam: den Rest eines Vollbilds oder einer grossen
/// Zwischenablage ueber eine duenne Leitung. Kappte man ihn vorher, bekaeme
/// er statt MSG_ABGELOEST einen Abbruch, verbaende sich neu und verdraengte
/// den Neuen.
const ABLOESUNG_HOECHSTENS: Duration = Duration::from_secs(15);
/// Steuernachrichten (alles aus send_small ausser Ton), die hoechstens auf
/// den Zuschauer warten duerfen: mindestens eine volle Zwischenablage. Die
/// Ablage ist auf 4 MB UTF-16 begrenzt (clipboard.rs, MAX_BYTES), gesendet
/// wird UTF-8: bei lateinischem Text bis 2 MB (zwei volle Ablagen passen),
/// bei CJK-Text bis rund 6 MB (nur eine). Sie werden nie
/// still verworfen - sonst bricht etwa die Reihenfolge Switch -> Info ->
/// Vollbild. Wer so weit zurueckliegt, ist weg; ohne diese Grenze liesse ein
/// Zuschauer, der Zeitfragen stellt und kaum liest, den Speicher des Hosts
/// ohne Ende wachsen.
const KLEIN_GRENZE: usize = 2 * HARTE_GRENZE;
/// Datei-Pakete (Art::Datei) nur, solange hoechstens so viel im Rueckstand
/// liegt und kein anderes Datei-Paket mehr wartet (Spezifikation 3.3). Ein
/// Bild wartet so in der Warteschlange hinter hoechstens 128 KiB und einem
/// Datei-Paket (Stueck bis 48 KiB, Angebot einmal bis 1 MiB). Was davor
/// schon beim Kernel liegt, sieht diese Regel nicht - dort begrenzt
/// DATEI_FENSTER_HOST die Dateidaten.
const DATEI_RUECKSTAND: usize = 128 * 1024;
/// Fenster des Senders Host -> Client, fest hoechstens 64 KiB (auch ausser
/// dem Spielmodus). ABWEICHUNG von 2.6 (FENSTER 256 KiB): die Voll-Regel
/// oben sieht den Kernelpuffer (SNDBUF, 256 KiB) nicht, und ein Fenster von
/// 256 KiB fuellte ihn vor jedem Bild mit Dateidaten (Gesamtdurchsicht [8]:
/// im Modell 329 ms statt 131 ms beim Mac-Host). Das Fenster zaehlt, was in
/// Warteschlange, Kernel und Leitung unquittiert unterwegs ist; 64 KiB
/// ersetzen so, was SO_NWRITE auf dem Mac leistet. Das kostet Durchsatz je
/// nach Umlaufzeit: hoechstens 64 KiB je Umlauf, im LAN (um 1 ms) also ueber
/// 60 MB/s, bei 20 ms Umlaufzeit aber nur rund 3,3 MB/s. Der Empfaenger
/// prueft nur die Obergrenze; das Protokoll bleibt vertraeglich.
const DATEI_FENSTER_HOST: u64 = dateien::FENSTER_SPIEL;
/// So lange wartet eine kopierte Dateiliste auf die Faehigkeiten des
/// aktuellen Eingabekanals (Integrationstest Befund 3: gleich nach dem
/// Verbinden steht er oft noch nicht, im --headless-Client bis zu 3 s).
const VORMERKEN: Duration = Duration::from_secs(5);

/// Das Fenster fuer den Sender der Host-Rolle: wie im Kern, aber nie ueber
/// DATEI_FENSTER_HOST.
fn fenster_host(spielmodus: bool) -> u64 {
    dateien::fenster(spielmodus).min(DATEI_FENSTER_HOST)
}

/// Sperre nehmen, auch wenn ein anderer Faden unter ihr in Panik geraten
/// ist. Die Sperren hier schuetzen Abfolgen (EINSPEISEN) oder Daten,
/// die jeder Schritt heil hinterlaesst; mit einem blossen unwrap zoege eine
/// einzige Panik beim Einspeisen jede weitere Annahme mit - und der Sendefaden
/// raeumte AKTUELL nie mehr ab ("kein Zuschauer, keine Arbeit").
fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Eine Leitung kappen - auch mitten in einem send oder recv. Ein shutdown
/// allein bricht unter Windows ein blockierendes send oder recv nicht ab,
/// solange die Gegenstelle lebt, aber nichts abnimmt oder schickt (gemessen:
/// Test abgeloester_zuschauer_der_nichts_abnimmt); der Faden hinge samt
/// Leitung, bis der Zuschauer irgendwann geht. CancelIoEx bricht jede
/// ausstehende Ein-/Ausgabe auf dem Socket ab, gleich aus welchem Faden. `s`
/// ist ein eigener Griff (try_clone) auf denselben Socket, den der Aufrufer
/// haelt - er kann also nicht schon geschlossen und neu vergeben sein.
fn kappen(s: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::IO::CancelIoEx;
    let _ = s.shutdown(Shutdown::Both);
    unsafe {
        let _ = CancelIoEx(HANDLE(s.as_raw_socket() as *mut std::ffi::c_void), None);
    }
}

/// Hoechstens eine Protokollzeile je Art und Adresse in dieser Frist: wer
/// einen Port mit Verbindungen bestreicht, soll das Protokoll nicht fuellen.
const MELDEFRIST: Duration = Duration::from_secs(10);
/// So viele Adressen merkt sich eine Drossel je Art (siehe Drossel).
const DROSSEL_ADRESSEN: usize = 64;

/// Eine Adresse in einer Drossel.
struct Gemerkt {
    /// Absender; None bei Zeilen ohne Gegenstelle (Ton, harte Grenze).
    von: Option<IpAddr>,
    /// Wann zuletzt eine Zeile fuer sie ins Protokoll ging.
    zuletzt: Instant,
    /// Seitdem unterdrueckt.
    weitere: u64,
}

/// Buchfuehrung einer Drossel (unter ihrer Sperre).
struct Drosselbuch {
    gemerkt: Vec<Gemerkt>,
    /// Unterdrueckt ohne eigenen Platz (alle Plaetze belegt), die letzte
    /// Adresse davon und wann zuletzt eine Sammelzeile dafuer hinausging.
    sonst: u64,
    sonst_von: Option<IpAddr>,
    sonst_zuletzt: Option<Instant>,
}

/// Drossel fuer eine Art Protokollzeile, die jede Verbindung ausloesen kann -
/// auch eine ohne Schluessel oder von einem unbekannten Geraet. Wie
/// beim Mac-Host (logf_gedrosselt in main.m) je Art UND Adresse: die erste
/// Zeile einer Adresse kommt sofort, danach hoechstens alle MELDEFRIST eine,
/// mit der Zahl der dazwischen unterdrueckten dieser Adresse. So geht die
/// Zeile eines eigenen Geraets nicht in der Flut eines anderen unter, und
/// die Zahl gehoert zu der Adresse, die in der Zeile steht. Was danach von
/// einer Adresse nicht mehr kommt, traegt drosseln_nachtragen im 5-s-Takt
/// als Sammelzeile nach, sobald die Frist um ist.
///
/// Die Tabelle ist begrenzt (DROSSEL_ADRESSEN je Art). Ist sie voll, macht
/// die aelteste Adresse Platz, die nichts mehr offen hat und deren Frist um
/// ist - vorher zaehlt sie noch. Gibt es keine solche, zaehlen weitere
/// Adressen gemeinsam ("von anderen Adressen", mit der letzten davon). Mehr
/// als DROSSEL_ADRESSEN + 1 Zeilen je Art und Frist gibt es so nie, auch
/// nicht bei einer Flut von vielen Adressen. Wer unbekannt immer wieder neu
/// versucht, fuellt weder Protokoll noch Platte. Ereignisse erlaubter
/// Geraete (Eintrag, Zuschauer verbunden) laufen nie hierueber.
pub(super) struct Drossel {
    /// Name der Art fuer die Sammelzeile.
    art: &'static str,
    buch: Mutex<Drosselbuch>,
}

impl Drossel {
    pub(super) const fn neu(art: &'static str) -> Drossel {
        Drossel { art, buch: Mutex::new(Drosselbuch { gemerkt: Vec::new(), sonst: 0, sonst_von: None, sonst_zuletzt: None }) }
    }

    /// Die Zeile ins Protokoll, wenn die Drossel sie fuer diese Adresse
    /// durchlaesst - dann mit der Zahl der seit der letzten unterdrueckten
    /// dieser Adresse -, sonst nur zaehlen. Der Text entsteht nur, wenn er
    /// geschrieben wird. Liefert die geschriebene Zeile.
    pub(super) fn melden(&self, von: Option<IpAddr>, text: impl FnOnce() -> String) -> Option<String> {
        self.melden_zu(von, Instant::now(), text)
    }

    fn melden_zu(&self, von: Option<IpAddr>, jetzt: Instant, text: impl FnOnce() -> String) -> Option<String> {
        let vorher = {
            let mut b = sperre(&self.buch);
            if let Some(g) = b.gemerkt.iter_mut().find(|g| g.von == von) {
                if jetzt.saturating_duration_since(g.zuletzt) < MELDEFRIST {
                    g.weitere += 1;
                    return None;
                }
                g.zuletzt = jetzt;
                std::mem::take(&mut g.weitere)
            } else {
                let neu = Gemerkt { von, zuletzt: jetzt, weitere: 0 };
                let frei = b
                    .gemerkt
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| g.weitere == 0 && jetzt.saturating_duration_since(g.zuletzt) >= MELDEFRIST)
                    .min_by_key(|(_, g)| g.zuletzt)
                    .map(|(i, _)| i);
                if b.gemerkt.len() < DROSSEL_ADRESSEN {
                    b.gemerkt.push(neu);
                } else if let Some(i) = frei {
                    b.gemerkt[i] = neu;
                } else {
                    b.sonst += 1;
                    b.sonst_von = von;
                    return None;
                }
                0
            }
        };
        let mut zeile = text();
        if vorher > 0 {
            let dort = if von.is_some() { " von dieser Adresse" } else { "" };
            zeile.push_str(&format!(" - dazu {vorher} weitere{dort} seit der letzten Meldung"));
        }
        log(&zeile);
        Some(zeile)
    }

    /// Unterdruecktes, nach dem nichts mehr kam, als Sammelzeile ins
    /// Protokoll - je Adresse, sobald ihre Frist um ist, und einmal fuer die
    /// Adressen ohne Platz. Liefert die geschriebenen Zeilen.
    fn nachtragen_zu(&self, jetzt: Instant) -> Vec<String> {
        let mut zeilen = Vec::new();
        {
            let mut b = sperre(&self.buch);
            for g in b.gemerkt.iter_mut() {
                if g.weitere > 0 && jetzt.saturating_duration_since(g.zuletzt) >= MELDEFRIST {
                    zeilen.push(match g.von {
                        Some(ip) => format!("{}: {} weitere von {ip} seit der letzten Meldung", self.art, g.weitere),
                        None => format!("{}: {} weitere seit der letzten Meldung", self.art, g.weitere),
                    });
                    g.weitere = 0;
                    g.zuletzt = jetzt;
                }
            }
            if b.sonst > 0 && b.sonst_zuletzt.map_or(true, |t| jetzt.saturating_duration_since(t) >= MELDEFRIST) {
                let von = b.sonst_von.map(|ip| ip.to_string()).unwrap_or_else(|| "?".into());
                zeilen.push(format!("{}: {} weitere von anderen Adressen seit der letzten Meldung, zuletzt von {von}", self.art, b.sonst));
                b.sonst = 0;
                b.sonst_zuletzt = Some(jetzt);
            }
        }
        for z in &zeilen {
            log(z);
        }
        zeilen
    }
}

/// Gescheiterte Handschlaege auf dem Bildkanal (Muell, Frist, falsches Protokoll).
static DROSSEL_HANDSCHLAG_BILD: Drossel = Drossel::neu("Bildkanal: Handschlag gescheitert");
/// Gescheiterte Handschlaege auf dem Eingabekanal.
static DROSSEL_HANDSCHLAG_EINGABE: Drossel = Drossel::neu("Eingabekanal: Handschlag gescheitert");
/// Eingabekanaele, die nach dem Handschlag abgewiesen werden.
static DROSSEL_EINGABE_ABGEWIESEN: Drossel = Drossel::neu("Eingabekanal abgewiesen");

/// Alle Drosseln, fuer drosseln_nachtragen (dazu die der Zugangsphase).
fn alle_drosseln() -> Vec<&'static Drossel> {
    let mut d: Vec<&'static Drossel> = vec![
        &DROSSEL_HANDSCHLAG_BILD,
        &DROSSEL_HANDSCHLAG_EINGABE,
        &DROSSEL_EINGABE_ABGEWIESEN,
        &DROSSEL_KEIN_BILD,
        &DROSSEL_HART,
        &DROSSEL_TON,
        &PLAETZE_BILD.gemeldet,
        &PLAETZE_EINGABE.gemeldet,
    ];
    d.extend(super::einlass::drosseln());
    d
}

/// Unterdrueckte Zeilen bleiben nicht liegen, auch wenn danach keine
/// derselben Art und Adresse mehr kommt: der 5-s-Takt des Dienstes (mod.rs)
/// ruft das, und nach der Frist geht die Zahl als Sammelzeile hinaus - wie
/// drosseln_nachtragen im Mac-Host.
pub fn drosseln_nachtragen() {
    let jetzt = Instant::now();
    for d in alle_drosseln() {
        d.nachtragen_zu(jetzt);
    }
}

/// Ein laufender Stau: seit wann ohne Fortschritt (Hostuhr in us) und wie
/// viel der Sendefaden bis dahin geschrieben hatte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stau {
    seit_us: u64,
    geschrieben: u64,
}

/// Was die Stauregel mit dem naechsten Bild tut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stauurteil {
    /// Rueckstand im Budget: codieren.
    Frei,
    /// Ueber dem Budget: dieses Bild auslassen.
    Stau,
    /// Ueber dem Budget, und seit STAU_FRIST_US nimmt der Zuschauer nichts
    /// mehr ab: er ist weg.
    Weg,
}

/// Die Stauregel als reine Entscheidung, wie stau_vor_dem_encoder in
/// main.m. Fortschritt heisst: der Sendefaden hat seit dem letzten Blick
/// etwas abgeliefert (`geschrieben` ist gewachsen). Anders als auf dem Mac
/// (Rueckstand kleiner als beim letzten Blick) gilt damit auch ein langsamer
/// Zuschauer als lebendig, waehrend noch Bilder aus dem Encoder nachkommen
/// und der Rueckstand deshalb nicht sinkt.
fn stau_urteil(stau: &mut Option<Stau>, rueckstand: usize, grenze: usize, geschrieben: u64, jetzt_us: u64) -> Stauurteil {
    if rueckstand <= grenze {
        *stau = None;
        return Stauurteil::Frei;
    }
    match *stau {
        Some(s) if s.geschrieben == geschrieben => {
            if jetzt_us.saturating_sub(s.seit_us) >= STAU_FRIST_US {
                *stau = None;
                Stauurteil::Weg
            } else {
                Stauurteil::Stau
            }
        }
        _ => {
            *stau = Some(Stau { seit_us: jetzt_us, geschrieben });
            Stauurteil::Stau
        }
    }
}

/// Budget der Stauregel: 2 MB, im Spielmodus ein Viertel.
fn budget(gaming: bool) -> usize {
    if gaming { BACKLOG_LIMIT / 4 } else { BACKLOG_LIMIT }
}

/// Ton, der hoechstens noch auf den Zuschauer warten darf: ein Viertel des
/// Budgets (rund 1,3 s, im Spielmodus 0,3 s unkomprimierter Ton). Gezaehlt
/// wird nur der wartende Ton selbst - ein grosses Vollbild davor laesst ihn
/// also nicht ausfallen. Liegt die Leitung unter der Tonrate (float32
/// Stereo, rund 3 Mbit/s), bleibt dem Bild so immer drei Viertel des
/// Budgets, und der Ton hinkt nicht immer weiter nach.
fn ton_grenze(gaming: bool) -> usize {
    budget(gaming) / 4
}

/// Warum ein Paket nicht in die Warteschlange kam.
#[derive(Debug, PartialEq, Eq)]
enum Abgewiesen {
    /// Der Zuschauer ist abgeloest oder weg.
    Zu,
    /// Der Rueckstand liegt schon ueber der Grenze.
    Voll,
}

/// Was ein Paket ist - danach richtet sich, woran seine Grenze gemessen
/// wird (siehe `Leitung::einreihen`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Art {
    /// Codiertes Bild: am ganzen Rueckstand.
    Bild,
    /// Tonpaket: am Ton, der selbst noch wartet.
    Ton,
    /// Alles andere - Begruessung und Steuernachrichten: an dem, was davon
    /// noch wartet.
    Klein,
    /// Paket des Datei-Senders (50-52, Host -> Client): nur ohne anderes
    /// wartendes Datei-Paket und bis DATEI_RUECKSTAND (Leitung::datei_einreihen).
    /// Zaehlt nicht gegen KLEIN_GRENZE.
    Datei,
}

/// Warum ein Zuschauer geht, ohne selbst zu gehen - danach richtet sich sein
/// Schlusswort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Schluss {
    /// Ein neuer Zuschauer ersetzt ihn: MSG_ABGELOEST.
    Abgeloest,
    /// Der Host verabschiedet ihn: MSG_HOST_ENDE mit Grund (HOST_ENDE_*).
    Ende(u8),
}

impl Schluss {
    /// Die Nachricht samt Kopf.
    fn nachricht(self) -> Vec<u8> {
        match self {
            Schluss::Abgeloest => kopf(MSG_ABGELOEST, 0, 0, 0).to_vec(),
            Schluss::Ende(grund) => host_ende(grund).to_vec(),
        }
    }

    /// Fuer das Protokoll.
    fn name(self) -> String {
        match self {
            Schluss::Abgeloest => "Abloesung".into(),
            Schluss::Ende(grund) => format!("Abschied ({})", ende_grund(grund)),
        }
    }
}

/// MSG_HOST_ENDE: Kopf und ein Byte Grund - Byte fuer Byte wie der Mac-Host
/// (qc_host_ende_kodieren in zugang.c).
fn host_ende(grund: u8) -> [u8; 9] {
    let mut m = [0u8; 9];
    m[..8].copy_from_slice(&kopf(MSG_HOST_ENDE, 0, 0, 1));
    m[8] = grund;
    m
}

/// Ein Grund aus MSG_HOST_ENDE in Worten, fuer das Protokoll.
fn ende_grund(grund: u8) -> &'static str {
    match grund {
        HOST_ENDE_BEENDET => "Host-Rolle beendet",
        HOST_ENDE_FREIGABE_AUS => "Freigabe beendet",
        HOST_ENDE_ENTFERNT => "Geraet entfernt",
        _ => "unbekannter Grund",
    }
}

struct Warteschlange {
    pakete: VecDeque<(Art, Vec<u8>)>,
    bytes: usize,
    /// Davon Ton bzw. Steuernachrichten.
    ton: usize,
    klein: usize,
    /// Wartende Datei-Pakete (Anzahl, nicht Byte).
    datei: usize,
    /// Was der Eingabekanal mit dieser Nummer mit IN_FAEHIGKEITEN gemeldet
    /// hat. Gilt nur, solange genau er der eingetragene ist (`eingabe`).
    faehig: Option<(u64, u32)>,
    /// Rest des Pakets, das der Sendefaden gerade schreibt (noch nicht beim
    /// Kernel) - zaehlt zum Rueckstand.
    im_schreiben: usize,
    /// Bytes, die der Sendefaden bisher beim Kernel abgeliefert hat; waechst
    /// nur. Ist der Kernelpuffer voll, kommt nur hinein, was der Zuschauer
    /// abnimmt - daran misst die Stauregel Fortschritt.
    geschrieben: u64,
    /// Laufender Stau dieses Zuschauers (siehe stau_urteil).
    stau: Option<Stau>,
    offen: bool,
    /// Abbruchgriff des Eingabekanals dieses Zuschauers, mit seiner Nummer.
    /// Liegt unter derselben Sperre wie `offen`: wer schliesst, sieht einen
    /// eben eingetragenen Griff sicher, und wer eintragen will, sieht sicher,
    /// dass der Zuschauer schon weg ist.
    eingabe: Option<(u64, TcpStream)>,
    /// Letzte Nachricht an einen abgeloesten oder verabschiedeten Zuschauer
    /// (MSG_ABGELOEST, MSG_HOST_ENDE) und welche es ist. Der Sendefaden
    /// schreibt sie statt der verworfenen Warteschlange, mit Frist, und
    /// macht danach die Leitung zu.
    schlusswort: Option<(Vec<u8>, Schluss)>,
    /// Der Sendefaden ist durch (siehe `abloesung_abschliessen`).
    beendet: bool,
}

impl Warteschlange {
    /// Alles verwerfen, was noch wartet.
    fn leeren(&mut self) {
        self.pakete.clear();
        self.bytes = 0;
        self.ton = 0;
        self.klein = 0;
        self.datei = 0;
    }

    /// Das vorderste Paket herausnehmen, samt Buchfuehrung je Art.
    fn nehmen(&mut self) -> Option<Vec<u8>> {
        let (art, p) = self.pakete.pop_front()?;
        self.bytes -= p.len();
        match art {
            Art::Ton => self.ton -= p.len(),
            Art::Klein => self.klein -= p.len(),
            Art::Datei => self.datei -= 1,
            Art::Bild => {}
        }
        Some(p)
    }

    /// Hat der eingetragene Eingabekanal "Dateien" gemeldet?
    fn faehig_aktuell(&self) -> bool {
        match (&self.eingabe, self.faehig) {
            (Some((n, _)), Some((m, bits))) => *n == m && dateien::kann_dateien(bits),
            _ => false,
        }
    }

    /// Was der eingetragene Eingabekanal zu Dateien sagt (siehe Lage).
    fn lage(&self) -> Lage {
        if !self.offen {
            return Lage::Zu;
        }
        match (&self.eingabe, self.faehig) {
            (Some((n, _)), Some((m, bits))) if *n == m => {
                if dateien::kann_dateien(bits) {
                    Lage::Kann
                } else {
                    Lage::KannNicht
                }
            }
            (kanal, _) => Lage::Offen { kanal: kanal.is_some() },
        }
    }
}

/// Was der aktuelle Eingabekanal eines Zuschauers zu Dateien gemeldet hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lage {
    /// Der Zuschauer ist abgeloest oder weg.
    Zu,
    /// IN_FAEHIGKEITEN mit FAEHIG_DATEIEN.
    Kann,
    /// IN_FAEHIGKEITEN ohne FAEHIG_DATEIEN.
    KannNicht,
    /// Noch nichts: kein Eingabekanal (`kanal` false), oder der eingetragene
    /// hat noch keine Faehigkeiten gemeldet. Ein neuer Client meldet sie als
    /// erste Nachricht; ein aelterer nie.
    Offen { kanal: bool },
}

/// Dateiuebertragung eines Zuschauers. Eigene Sperre (Leitung::dateien),
/// nie zusammen mit Leitung::q genommen; darunter laufen nur Aufrufe des
/// Kerns, die sofort zurueckkehren (nachricht, quittung, abbrechen, starten).
#[derive(Default)]
struct DateiStand {
    /// Client -> Host; angelegt beim ersten Angebot (Leitung::empfaenger_bereit).
    empfaenger: Option<dateien::Empfaenger>,
    /// Host -> Client; hoechstens einer.
    sender: Option<dateien::Griff>,
    /// Die zuletzt abgebrochene Sendung (neuer Text, vorgemerkte Liste),
    /// solange noch keine neue lief: die naechste wartet in ihrem Faden auf
    /// deren Ende 1, bevor ihr Angebot hinausgeht (Sender::starten_nach).
    abgebrochen: Option<dateien::Griff>,
    /// Eine kopierte Liste, die auf die Faehigkeiten des aktuellen
    /// Eingabekanals wartet (hoechstens VORMERKEN).
    vorgemerkt: Option<Vorgemerkt>,
    /// Zaehlt die Vormerkungen: ein spaeter Zeitgeber trifft keine neuere.
    vormerk_nr: u64,
    /// Die Zeile "Zuschauer kann keine Dateien" bzw. "hat keine
    /// Faehigkeiten gemeldet" steht schon im Protokoll (einmal je Zuschauer).
    kann_nicht_gemeldet: bool,
    /// Zuschauer abgeloest oder weg: nichts Neues mehr anlegen.
    zu: bool,
}

/// Eine vorgemerkte Dateiliste (siehe DateiStand::vorgemerkt).
struct Vorgemerkt {
    pfade: Vec<PathBuf>,
    nr: u64,
}

/// Ein verbundener Zuschauer: Warteschlange zum Sendefaden und die Kennung
/// seines Handschlags, an die sich der Eingabekanal bindet.
pub struct Leitung {
    q: Mutex<Warteschlange>,
    cv: Condvar,
    /// Meldet das Ende des Sendefadens. Eigene Bedingung, damit ein Wecken
    /// fuer den Sendefaden nie beim Wartenden in `abloesung_abschliessen`
    /// landet.
    ende: Condvar,
    /// Zweite Hand an der Bildleitung: `schliessen` kappt sie auch dann,
    /// wenn der Sendefaden in einem write haengt (Zuschauer eingefroren).
    bild_griff: Option<TcpStream>,
    pub peer: Vec<u8>,
    pub hh: Vec<u8>,
    pub ip: String,
    /// Name des Geraets (Nachricht 3, sonst die IP) - fuer die Oberflaeche
    /// ("Verbunden: <Name>").
    pub name: String,
    /// Nummer dieses Zuschauers (zuschauer_nr). bild_annehmen setzt sie unter
    /// AKTUELL, bevor ihn jemand sieht; die Datei-Wege binden sich daran.
    nr: AtomicU64,
    /// Dateiuebertragung dieses Zuschauers (siehe DateiStand).
    dateien: Mutex<DateiStand>,
}

impl Leitung {
    fn neu(bild_griff: Option<TcpStream>, peer: Vec<u8>, hh: Vec<u8>, ip: String) -> Leitung {
        Leitung {
            q: Mutex::new(Warteschlange {
                pakete: VecDeque::new(),
                bytes: 0,
                ton: 0,
                klein: 0,
                datei: 0,
                faehig: None,
                im_schreiben: 0,
                geschrieben: 0,
                stau: None,
                offen: true,
                eingabe: None,
                schlusswort: None,
                beendet: false,
            }),
            cv: Condvar::new(),
            ende: Condvar::new(),
            bild_griff,
            peer,
            hh,
            name: ip.clone(),
            ip,
            nr: AtomicU64::new(0),
            dateien: Mutex::new(DateiStand::default()),
        }
    }

    fn nr(&self) -> u64 {
        self.nr.load(Ordering::Relaxed)
    }

    /// Zuschauer beenden: Warteschlange zu, Bildleitung und Eingabekanal
    /// gekappt. Kein Bild, keine Eingabe - ein abgeloester Zuschauer behaelt
    /// nichts, auch wenn er seine Leitungen selbst offen hielte. Seine
    /// Dateiuebertragungen brechen ab.
    fn schliessen(&self) {
        let eingabe = {
            let mut q = sperre(&self.q);
            q.offen = false;
            self.cv.notify_all();
            q.eingabe.take()
        };
        if let Some((_, s)) = eingabe {
            kappen(&s);
            log("Eingabekanal gekappt: sein Zuschauer ist abgeloest oder weg");
        }
        self.bild_kappen();
        self.dateien_abbrechen();
    }

    /// Sitzungsende oder Zuschauerwechsel: Sender und Empfaenger dieses
    /// Zuschauers brechen ab (2.7 Schritt 5, 2.8 Schritt 5), und es entsteht
    /// kein neuer mehr. Kehrt sofort zurueck: der Kern wartet dabei auf
    /// keinen Faden (ein Ende 1 schickt der Sender ueber seinen Weg, der nach
    /// dem Wechsel Weg meldet).
    fn dateien_abbrechen(&self) {
        let (empfaenger, sender) = {
            let mut d = sperre(&self.dateien);
            d.zu = true;
            d.vorgemerkt = None;
            d.abgebrochen = None;
            (d.empfaenger.take(), d.sender.take())
        };
        if let Some(e) = empfaenger {
            e.abbrechen();
        }
        if let Some(g) = sender {
            g.abbrechen();
        }
    }

    fn bild_kappen(&self) {
        if let Some(s) = &self.bild_griff {
            kappen(s);
        }
    }

    /// Ein neuer Zuschauer ersetzt diesen: `schliessen_mit` mit
    /// MSG_ABGELOEST.
    fn abloesen(&self) {
        self.schliessen_mit(Schluss::Abgeloest);
    }

    /// Dieser Zuschauer geht mit einem Schlusswort (Abloesung oder Abschied
    /// des Hosts). Was noch in der Warteschlange liegt, wird verworfen;
    /// hinaus geht nur noch das Schlusswort - danach schliesst der
    /// Sendefaden die Bildleitung selbst. Nur die Begruessung
    /// (MAGIC + Strominfo) bleibt, wenn der Sendefaden noch gar nichts
    /// genommen hat: sie geht mit dem Schlusswort hinaus. Ohne sie laese der
    /// Client das Schlusswort als MAGIC, hielte es fuer ein fremdes
    /// Protokoll und verbaende sich neu - und verdraengte den Neuen. Der
    /// Eingabekanal ist sofort gekappt. Haengt der Sendefaden an einem
    /// Zuschauer, der nichts mehr abnimmt, kappt `abloesung_abschliessen` die
    /// Bildleitung. Seine Dateiuebertragungen brechen ab. Unter EINSPEISEN
    /// und AKTUELL aufrufen; wartet nie.
    fn schliessen_mit(&self, schluss: Schluss) {
        let eingabe = {
            let mut q = sperre(&self.q);
            if q.offen {
                // Die Begruessung reiht bild_annehmen als erstes Paket ein,
                // bevor andere Faeden die Leitung sehen.
                let mut wort = Vec::new();
                if q.geschrieben == 0 && q.im_schreiben == 0 {
                    if let Some(gruss) = q.nehmen() {
                        wort = gruss;
                    }
                }
                wort.extend_from_slice(&schluss.nachricht());
                q.leeren();
                q.schlusswort = Some((wort, schluss));
                q.offen = false;
                self.cv.notify_all();
            }
            q.eingabe.take()
        };
        if let Some((_, s)) = eingabe {
            kappen(&s);
            log("Eingabekanal gekappt: sein Zuschauer ist abgeloest oder weg");
        }
        self.dateien_abbrechen();
    }

    /// Nach `abloesen`, ausserhalb aller Sperren: auf das Ende des
    /// Sendefadens warten, solange er binnen `frist` Fortschritt macht
    /// (`geschrieben` waechst - dasselbe Mass wie in der Stauregel), aber
    /// hoechstens ABLOESUNG_HOECHSTENS; sonst die Bildleitung kappen, auch
    /// wenn er noch in einem write steckt. `frist` muss laenger sein als
    /// FRIST_SCHLUSSWORT: waehrend des Schlussworts waechst `geschrieben`
    /// nicht. true: der Sendefaden ist durch.
    fn abloesung_abschliessen(&self, frist: Duration) -> bool {
        self.schluss_abwarten(frist, ABLOESUNG_HOECHSTENS, "Abgeloester")
    }

    /// Wie `abloesung_abschliessen`, mit eigener Obergrenze `hoechstens`
    /// (beim Beenden wartet niemand 15 s); `wer` steht im Protokoll vor
    /// "Zuschauer".
    fn schluss_abwarten(&self, frist: Duration, hoechstens: Duration, wer: &str) -> bool {
        let beginn = Instant::now();
        let mut q = sperre(&self.q);
        loop {
            let stand = q.geschrieben;
            let rest = hoechstens.saturating_sub(beginn.elapsed());
            q = self.ende.wait_timeout_while(q, frist.min(rest), |q| !q.beendet).unwrap_or_else(|e| e.into_inner()).0;
            if q.beendet || q.geschrieben == stand || beginn.elapsed() >= hoechstens {
                break;
            }
        }
        let beendet = q.beendet;
        drop(q);
        if !beendet {
            log(format!(
                "{wer} Zuschauer {} nimmt nichts mehr ab (nach {:.1} s) - Bildleitung gekappt",
                self.ip,
                beginn.elapsed().as_secs_f32()
            ));
            self.bild_kappen();
        }
        beendet
    }

    #[cfg(test)]
    fn offen(&self) -> bool {
        sperre(&self.q).offen
    }

    /// Gilt Eingabekanal `nr` noch? Nur, solange sein Zuschauer da ist und
    /// kein neuerer Eingabekanal desselben Zuschauers ihn abgeloest hat.
    fn gilt(&self, nr: u64) -> bool {
        let q = sperre(&self.q);
        q.offen && q.eingabe.as_ref().map(|(n, _)| *n == nr).unwrap_or(false)
    }

    /// Eingabekanal an diesen Zuschauer binden. false, wenn der Zuschauer
    /// inzwischen abgeloest ist (Wechsel waehrend des Handschlags). Ein
    /// frueherer Eingabekanal desselben Zuschauers wird gekappt und seine
    /// Tasten losgelassen. Unter EINSPEISEN aufrufen.
    fn eingabe_binden(&self, nr: u64, griff: TcpStream) -> bool {
        let alt = {
            let mut q = sperre(&self.q);
            if !q.offen {
                return false;
            }
            q.eingabe.replace((nr, griff))
        };
        if let Some((_, s)) = alt {
            kappen(&s);
            log("Eingabekanal gekappt: derselbe Zuschauer hat einen neuen aufgebaut");
            eingabe::alle_tasten_loslassen();
        }
        true
    }

    /// Eigenen Griff austragen, wenn er noch der eingetragene ist - sonst
    /// hielte die Kopie die Leitung offen, nachdem die Schleife sie verlassen
    /// hat. true: der Kanal galt bis zuletzt.
    fn eingabe_loesen(&self, nr: u64) -> bool {
        let mut q = sperre(&self.q);
        if q.eingabe.as_ref().map(|(n, _)| *n == nr).unwrap_or(false) {
            q.eingabe = None;
            return true;
        }
        false
    }

    /// IN_FAEHIGKEITEN des Eingabekanals `nr` merken - nur, wenn er noch
    /// der eingetragene ist. Ein neuer Kanal desselben Zuschauers gilt erst
    /// nach seiner eigenen Meldung als faehig (2.2).
    fn faehigkeiten_setzen(&self, nr: u64, bits: u32) {
        let mut q = sperre(&self.q);
        if q.eingabe.as_ref().map(|(n, _)| *n == nr).unwrap_or(false) {
            q.faehig = Some((nr, bits));
        }
    }

    /// Darf DATEI_* an diesen Zuschauer? Nur, solange er da ist und sein
    /// aktueller Eingabekanal FAEHIG_DATEIEN gemeldet hat. Der Dienst fragt
    /// lage(), die Tests diese Kurzform.
    #[cfg(test)]
    fn kann_dateien(&self) -> bool {
        self.lage() == Lage::Kann
    }

    fn lage(&self) -> Lage {
        sperre(&self.q).lage()
    }

    /// Ein Paket des Datei-Senders einreihen (Art::Datei, 3.3): Weg, wenn der
    /// Zuschauer zu ist; Voll, solange noch ein Datei-Paket wartet, der
    /// Rueckstand (Warteschlange und der Rest im Schreiben) ueber
    /// DATEI_RUECKSTAND liegt oder der aktuelle Eingabekanal keine Dateien
    /// gemeldet hat (etwa gleich nach einem neuen Eingabekanal, bis dessen
    /// IN_FAEHIGKEITEN da ist). Das Paket entsteht erst, wenn es auch
    /// hineinkommt - Voll kostet keine Kopie.
    fn datei_einreihen(&self, typ: u8, data: &[u8]) -> Gesendet {
        let mut q = sperre(&self.q);
        if !q.offen {
            return Gesendet::Weg;
        }
        if !q.faehig_aktuell() || q.datei > 0 || q.bytes + q.im_schreiben > DATEI_RUECKSTAND {
            return Gesendet::Voll;
        }
        let mut p = Vec::with_capacity(8 + data.len());
        p.extend_from_slice(&kopf(typ, 0, 0, data.len()));
        p.extend_from_slice(data);
        q.bytes += p.len();
        q.datei += 1;
        q.pakete.push_back((Art::Datei, p));
        self.cv.notify_one();
        Gesendet::Ja
    }

    /// Vor dem Einspeisen eines Angebots: den Empfaenger dieses Zuschauers
    /// anlegen, falls es noch keinen gibt (sein Faden startet also nicht
    /// unter EINSPEISEN). Nach dem Abloesen bzw. Schliessen nicht mehr.
    fn empfaenger_bereit(&self, u: &DateiUmgebung) {
        let mut d = sperre(&self.dateien);
        if d.zu || d.empfaenger.is_some() {
            return;
        }
        let (platz, zaehler, nr) = (u.platz, u.zaehler, self.nr());
        // Quittungen als kleine Steuernachricht, nur an diesen Zuschauer.
        let weg: Arc<dyn dateien::Weg> = Arc::new(move |typ: u8, n: &[u8]| klein_an(platz, zaehler, nr, typ, n));
        let ablegen = u.ablegen.clone();
        d.empfaenger = Some(dateien::Empfaenger::neu_mit(
            u.basis.clone(),
            weg,
            move |pfade: Vec<PathBuf>| ablegen(&pfade),
            u.melden(None),
            u.vorgaben,
        ));
    }

    /// 50, 51, 52 vom Eingabekanal an den Empfaenger: legt nur in seine
    /// Warteschlange (keine Plattenarbeit, darf unter EINSPEISEN laufen).
    /// Ohne Empfaenger (kein Angebot vorher, oder abgebrochen) uebergangen.
    fn datei_eingang(&self, typ: u8, nutzlast: Vec<u8>) {
        if let Some(e) = &sperre(&self.dateien).empfaenger {
            e.nachricht(typ, nutzlast);
        }
    }

    /// 53 vom Eingabekanal an den laufenden Sender; kehrt sofort zurueck.
    fn datei_quittung(&self, nutzlast: &[u8]) {
        if let Some(g) = &sperre(&self.dateien).sender {
            g.quittung(nutzlast);
        }
    }

    /// Neuer Text in der Ablage: die laufende Sendung bricht ab (ihr Faden
    /// schickt Ende 1), eine vorgemerkte Liste faellt weg. Der Griff bleibt
    /// als Vorgaenger der naechsten Sendung liegen. Kehrt sofort zurueck.
    fn sender_abbrechen(&self) {
        let mut d = sperre(&self.dateien);
        d.vorgemerkt = None;
        Leitung::sendung_beenden(&mut d);
    }

    /// Die laufende Sendung bricht ab (neuer Inhalt); ihr Griff bleibt in
    /// `abgebrochen`, bis eine neue Sendung auf ihr Ende wartet.
    fn sendung_beenden(d: &mut DateiStand) {
        if let Some(g) = d.sender.take() {
            g.abbrechen();
            d.abgebrochen = Some(g);
        }
    }

    /// Startet den Sender fuer diesen Zuschauer (unter seiner Dateisperre):
    /// der Weg ist an seine Nummer gebunden, das Fenster ist fenster_host
    /// (je Stueck gefragt, also mit dem Spielmodus von jetzt), und die
    /// vorige Sendung ist der Vorgaenger. Liefert die Kennung.
    fn sender_starten(&self, d: &mut DateiStand, u: &DateiUmgebung, pfade: Vec<PathBuf>) -> u32 {
        let (platz, zaehler, nr) = (u.platz, u.zaehler, self.nr());
        let weg: Arc<dyn dateien::Weg> = Arc::new(move |typ: u8, n: &[u8]| datei_an(platz, zaehler, nr, typ, n));
        let vorgaenger = d.sender.take().or_else(|| d.abgebrochen.take());
        let griff = dateien::Sender::starten_nach(
            vorgaenger,
            pfade,
            weg,
            || fenster_host(Z.gaming.load(Ordering::Relaxed)),
            u.melden(Some(self.ip.clone())),
            u.vorgaben,
        );
        let kennung = griff.kennung();
        d.sender = Some(griff);
        kennung
    }

    /// Eine vorgemerkte Liste pruefen (nach IN_FAEHIGKEITEN bzw. vom
    /// Zeitgeber ihrer Vormerkung, `verfallen` = deren Nummer): kann der
    /// aktuelle Eingabekanal Dateien, geht sie jetzt hinaus; kann er es
    /// nicht, faellt sie weg (ZEILE_ZUSCHAUER_ZU_ALT); hat er noch nichts
    /// gemeldet, wartet sie weiter - ausser ihre Frist ist um, dann faellt
    /// sie mit einer zutreffenden Zeile weg (zeile_ohne_meldung). Die
    /// Zeilen kommen einmal je Zuschauer.
    fn vorgemerkte_pruefen(&self, u: &DateiUmgebung, verfallen: Option<u64>) {
        let lage = self.lage();
        let mut d = sperre(&self.dateien);
        let Some(nr) = d.vorgemerkt.as_ref().map(|v| v.nr) else { return };
        if verfallen.is_some_and(|n| n != nr) {
            return;
        }
        let zeile = match lage {
            Lage::Kann => {
                let Some(v) = d.vorgemerkt.take() else { return };
                self.sender_starten(&mut d, u, v.pfade);
                drop(d);
                (u.zeile)("Dateien: Zuschauer meldet Dateien - die vorgemerkte Liste geht hinaus".into());
                return;
            }
            Lage::Offen { .. } if verfallen.is_none() => return,
            Lage::Offen { kanal } => zeile_ohne_meldung(kanal, u.vormerken),
            Lage::KannNicht => dateien::ZEILE_ZUSCHAUER_ZU_ALT.to_string(),
            Lage::Zu => {
                d.vorgemerkt = None;
                return;
            }
        };
        d.vorgemerkt = None;
        let melden = !d.kann_nicht_gemeldet;
        d.kann_nicht_gemeldet = true;
        drop(d);
        if melden {
            (u.zeile)(zeile);
        }
    }

    /// Ein Paket einreihen. Mit Grenze: nur, wenn der Stand nicht schon
    /// darueber liegt - ein Paket darf sie also um sich selbst
    /// ueberschreiten, auch ein Vollbild, das allein groesser ist. Der Stand
    /// ist fuer ein Bild (und ein Datei-Paket) der ganze Rueckstand, fuer Ton
    /// der wartende Ton, fuer alles andere die wartenden Steuernachrichten.
    fn einreihen(&self, paket: Vec<u8>, art: Art, grenze: Option<usize>) -> Result<(), Abgewiesen> {
        let mut q = sperre(&self.q);
        if !q.offen {
            return Err(Abgewiesen::Zu);
        }
        if let Some(g) = grenze {
            let stand = match art {
                Art::Bild | Art::Datei => q.bytes + q.im_schreiben,
                Art::Ton => q.ton,
                Art::Klein => q.klein,
            };
            if stand > g {
                return Err(Abgewiesen::Voll);
            }
        }
        q.bytes += paket.len();
        match art {
            Art::Ton => q.ton += paket.len(),
            Art::Klein => q.klein += paket.len(),
            Art::Datei => q.datei += 1,
            Art::Bild => {}
        }
        q.pakete.push_back((art, paket));
        self.cv.notify_one();
        Ok(())
    }

    /// Eine Nachricht aus send_small einreihen, mit den Grenzen ihrer Art.
    /// Ton ueber ton_grenze faellt weg (Zaehler "Ton verworfen"), eine
    /// Steuernachricht ueber KLEIN_GRENZE heisst: der Zuschauer ist weg. Die
    /// Frist der Stauregel gilt auch hier: steht das Bild still (kein Bild,
    /// das nach dem Stau fragt) und laeuft nur der Ton in einen
    /// eingefrorenen Zuschauer, fiele er sonst nie auf.
    fn klein_senden(&self, typ: u8, paket: Vec<u8>, gaming: bool, jetzt_us: u64) -> Result<(), Abgewiesen> {
        let (art, grenze) = if typ == MSG_AUDIO { (Art::Ton, ton_grenze(gaming)) } else { (Art::Klein, KLEIN_GRENZE) };
        let r = self.einreihen(paket, art, Some(grenze));
        match r {
            Ok(()) => {
                self.stau(budget(gaming), jetzt_us);
            }
            Err(Abgewiesen::Zu) => {}
            Err(Abgewiesen::Voll) if art == Art::Ton => {
                Z.ton_verworfen.fetch_add(1, Ordering::Relaxed);
                DROSSEL_TON.melden(None, || format!("Leitung langsamer als der Ton: Tonpaket verworfen (ueber {} kB Ton wartet)", grenze / 1024));
                // Der wartende Ton allein kommt nicht mehr ueber das Budget -
                // die Frist laeuft deshalb ab seiner eigenen Grenze.
                self.stau(grenze, jetzt_us);
            }
            Err(Abgewiesen::Voll) => {
                log(format!("Zuschauer weg: nimmt ueber {} kB Steuernachrichten nicht ab", KLEIN_GRENZE / 1024));
                self.schliessen();
            }
        }
        r
    }

    /// Stauregel fuer diesen Zuschauer (stau_urteil) mit Grenze `grenze`.
    /// Ist er weg: ins Protokoll (wie main.m) und die Leitung schliessen -
    /// der Sendefaden endet dann und traegt ihn aus. Ein geschlossener
    /// Zuschauer staut nichts mehr.
    fn stau(&self, grenze: usize, jetzt_us: u64) -> Stauurteil {
        let (urteil, rueckstand) = {
            let mut q = sperre(&self.q);
            if !q.offen {
                return Stauurteil::Frei;
            }
            let rueckstand = q.bytes + q.im_schreiben;
            let geschrieben = q.geschrieben;
            (stau_urteil(&mut q.stau, rueckstand, grenze, geschrieben, jetzt_us), rueckstand)
        };
        if urteil == Stauurteil::Weg {
            log(format!("Zuschauer weg: nimmt seit {} s nichts mehr ab ({rueckstand} Byte im Stau)", STAU_FRIST_US / 1_000_000));
            self.schliessen();
        }
        urteil
    }
}

static AKTUELL: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
static SEQ: AtomicU32 = AtomicU32::new(0);
/// Laufende Nummer der Zuschauer (zuschauer_nr). Waechst unter AKTUELL,
/// zugleich mit dem Eintrag des Neuen.
static NR: AtomicU64 = AtomicU64::new(0);
/// Laufende Nummer der Eingabekanaele (siehe Leitung::eingabe_loesen).
static EINGABE_NR: AtomicU64 = AtomicU64::new(0);
/// Einspeisen und Abloesen schliessen sich aus: Die Eingabeschleife prueft
/// unter dieser Sperre, ob ihr Kanal noch gilt, und fuehrt die Nachricht
/// aus; wer einen Zuschauer oder Eingabekanal abloest und danach alle Tasten
/// loslaesst, tut das ebenfalls darunter. Sonst koennte eine Taste des Alten
/// genau zwischen Pruefung und Loslassen gedrueckt werden und haengen
/// bleiben. Reihenfolge: erst EINSPEISEN, dann AKTUELL.
static EINSPEISEN: Mutex<()> = Mutex::new(());

/// Test-Haken: laeuft einmal im Faden der naechsten Verbindung, nach dem
/// Einlass und vor dem Eintragen als Zuschauer (Wettlauf mit "Entfernen").
#[cfg(test)]
static NACH_EINLASS: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// Handschlaege, die gerade laufen - je Port hoechstens 32, je Absender
/// hoechstens zwei. Mit nur einem Annahmefaden genuegte eine einzige stumme
/// Verbindung, um jeden weiteren Zuschauer auszusperren; ohne Grenze banden
/// Verbindungen ohne Schluessel beliebig viele Faeden. Die Gesamtgrenze ist
/// bewusst weit: wer alle Plaetze stumm halten will, um einen Zuschauer
/// auszusperren, braucht 16 Absenderadressen, nicht zwei. Ein Faden, der im
/// recv wartet, kostet kaum etwas, und jeder endet nach der Frist.
const HANDSCHLAEGE_MAX: usize = 32;
const HANDSCHLAEGE_JE_ABSENDER: usize = 2;

struct Plaetze {
    /// Absender der laufenden Handschlaege.
    laufend: Mutex<Vec<IpAddr>>,
    /// Abweisungen, weil alle Plaetze belegt sind.
    gemeldet: Drossel,
}

static PLAETZE_BILD: Plaetze = Plaetze::neu("Bildkanal: Verbindung abgewiesen, zu viele Handschlaege offen");
static PLAETZE_EINGABE: Plaetze = Plaetze::neu("Eingabekanal: Verbindung abgewiesen, zu viele Handschlaege offen");
/// "Kein Bildkanal offen" auf dem Eingabeport.
static DROSSEL_KEIN_BILD: Drossel = Drossel::neu("Eingabekanal abgewiesen: kein Bildkanal offen");
/// Codierte Bilder, die an der harten Grenze wegfallen.
static DROSSEL_HART: Drossel = Drossel::neu("Stau ueber der harten Grenze: codiertes Bild verworfen");
/// Tonpakete, die an ton_grenze wegfallen.
static DROSSEL_TON: Drossel = Drossel::neu("Leitung langsamer als der Ton: Tonpaket verworfen");

/// Ein belegter Platz; gibt sich beim Wegfallen selbst frei.
struct Platz {
    p: &'static Plaetze,
    ip: IpAddr,
}

impl Plaetze {
    /// `art`: Name der Abweisung fuer die Sammelzeile der Drossel.
    const fn neu(art: &'static str) -> Plaetze {
        Plaetze { laufend: Mutex::new(Vec::new()), gemeldet: Drossel::neu(art) }
    }

    fn belegen(&'static self, ip: IpAddr, kanal: &str) -> Option<Platz> {
        let mut l = sperre(&self.laufend);
        let je_absender = l.iter().filter(|x| **x == ip).count();
        if l.len() >= HANDSCHLAEGE_MAX || je_absender >= HANDSCHLAEGE_JE_ABSENDER {
            self.gemeldet.melden(Some(ip), || {
                format!("{kanal}: Verbindung von {ip} abgewiesen - schon {} Handschlaege offen ({je_absender} von dort)", l.len())
            });
            return None;
        }
        l.push(ip);
        Some(Platz { p: self, ip })
    }
}

impl Drop for Platz {
    fn drop(&mut self) {
        let mut l = sperre(&self.p.laufend);
        if let Some(i) = l.iter().position(|x| *x == self.ip) {
            l.swap_remove(i);
        }
    }
}

fn absender(s: &TcpStream) -> IpAddr {
    s.peer_addr().map(|a| a.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

pub fn zuschauer_da() -> bool {
    sperre(&AKTUELL).is_some()
}

/// Das Gegenueber der Host-Rolle fuer den Ablagewaechter
/// (clipboard::watch): Some(Nummer des Zuschauers), solange einer da ist -
/// ein neuer Zuschauer ist eine neue Sitzung -, sonst None.
pub fn zuschauer_sitzung() -> Option<u64> {
    let a = sperre(&AKTUELL);
    a.is_some().then(|| NR.load(Ordering::Relaxed))
}

/// Name des verbundenen Zuschauers (Oberflaeche: "Verbunden: <Name>").
pub fn zuschauer_name() -> Option<String> {
    sperre(&AKTUELL).as_ref().map(|l| l.name.clone())
}

/// Den Zuschauer verabschieden (MSG_HOST_ENDE mit `grund`) - nur, wenn er
/// dieses Geraet ist (`peer`; None: jeden), etwa wenn sein Geraet aus der
/// Liste entfernt wird. Wie eine Abloesung: was noch wartet, wird verworfen,
/// hinaus gehen der Rest des laufenden Pakets und der Abschied, dann ist die
/// Leitung zu; der Eingabekanal ist sofort gekappt und seine Tasten
/// losgelassen. Der Client verbindet sich darauf nicht von selbst neu; tut
/// es der Nutzer, braucht das Geraet, was jedes unbekannte braucht. Kehrt
/// sofort zurueck - ein eigener Faden wartet auf den Abschied (oder kappt
/// einen Zuschauer, der nichts mehr abnimmt). Liefert, ob einer
/// verabschiedet wurde.
pub fn zuschauer_verabschieden(peer: Option<&[u8]>, grund: u8) -> bool {
    let Some(l) = verabschieden(peer, grund) else { return false };
    std::thread::spawn(move || l.schluss_abwarten(FRIST_SCHLUSSWORT * 2, ABLOESUNG_HOECHSTENS, "Verabschiedeter"));
    true
}

/// Vor dem Ende des Prozesses: den Zuschauer verabschieden und warten, bis
/// der Abschied beim Kernel liegt und die Leitung zu ist - hoechstens
/// `hoechstens` (wie der Mac-Host: 3 s), sonst wird gekappt. Wer danach den
/// Prozess beendet, schneidet den Abschied nicht mehr ab.
pub fn abschied_beim_beenden(grund: u8, hoechstens: Duration) {
    if let Some(l) = verabschieden(None, grund) {
        l.schluss_abwarten(FRIST_SCHLUSSWORT * 2, hoechstens, "Verabschiedeter");
    }
}

/// Gemeinsamer Teil: unter EINSPEISEN und AKTUELL das Schlusswort setzen
/// und die Tasten loslassen. Ausgetragen wird er wie immer vom Sendefaden.
fn verabschieden(peer: Option<&[u8]>, grund: u8) -> Option<Arc<Leitung>> {
    let _einspeisen = sperre(&EINSPEISEN);
    let a = sperre(&AKTUELL);
    let l = a.as_ref()?.clone();
    if peer.is_some_and(|p| p != l.peer.as_slice()) {
        return None;
    }
    log(format!("Zuschauer {} ({}) wird verabschiedet: {}", l.name, l.ip, ende_grund(grund)));
    l.schliessen_mit(Schluss::Ende(grund));
    eingabe::alle_tasten_loslassen();
    Some(l)
}

/// Nummer des aktuellen Zuschauers. Sie aendert sich bei jeder Annahme -
/// auch wenn ein Neuer den Alten ohne Luecke abloest und zuschauer_da()
/// dabei nie false wird. Wer sie sieht, sieht auch schon den Neuen in
/// AKTUELL.
pub fn zuschauer_nr() -> u64 {
    let _a = sperre(&AKTUELL);
    NR.load(Ordering::Relaxed)
}

fn aktuell() -> Option<Arc<Leitung>> {
    sperre(&AKTUELL).clone()
}

/// Kopf einer Nachricht.
fn kopf(typ: u8, flags: u8, reserviert: u16, len: usize) -> [u8; 8] {
    let mut h = [0u8; 8];
    h[0] = typ;
    h[1] = flags;
    h[2..4].copy_from_slice(&reserviert.to_le_bytes());
    h[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    h
}

/// Kleine Nachricht ueber die Bildverbindung. Umgeht bewusst die Vollbild-
/// Sperre und die Stauregel: Ton und Zwischenablage sind winzig und duerfen
/// nicht warten. Grenzen und Frist: siehe Leitung::klein_senden.
pub fn send_small(typ: u8, data: &[u8]) {
    let Some(l) = aktuell() else { return };
    let _ = klein_auf(&l, typ, data);
}

/// Eine kleine Nachricht auf diese Leitung (siehe send_small).
fn klein_auf(l: &Leitung, typ: u8, data: &[u8]) -> Result<(), Abgewiesen> {
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&kopf(typ, 0, 0, data.len()));
    p.extend_from_slice(data);
    l.klein_senden(typ, p, Z.gaming.load(Ordering::Relaxed), now_us())
}

/// Die Leitung des Zuschauers `nr` aus `platz` - nur, solange er noch der
/// aktuelle ist (`zaehler` steht auf `nr`). Nummer und Eintrag werden unter
/// derselben Sperre gelesen (NR waechst unter AKTUELL). Loest danach einer
/// ab, landet eine Nachricht an `nr` schlimmstenfalls in der Warteschlange
/// des Abgeloesten, die die Abloesung zumacht - nie beim Neuen.
fn leitung_von(platz: &Mutex<Option<Arc<Leitung>>>, zaehler: &AtomicU64, nr: u64) -> Option<Arc<Leitung>> {
    let a = sperre(platz);
    if zaehler.load(Ordering::Relaxed) != nr {
        return None;
    }
    a.clone()
}

/// Kleine Nachricht nur an den Zuschauer mit dieser Nummer (zuschauer_nr);
/// ist inzwischen ein anderer da oder keiner, geht sie nirgendwohin. Liefert,
/// ob sie an ihn ging.
fn send_small_an(platz: &Mutex<Option<Arc<Leitung>>>, zaehler: &AtomicU64, nr: u64, typ: u8, data: &[u8]) -> bool {
    let Some(l) = leitung_von(platz, zaehler, nr) else { return false };
    let _ = klein_auf(&l, typ, data);
    true
}

/// Weg der Quittungen des Empfaengers (Client -> Host): eine kleine
/// Steuernachricht (Art::Klein) nur an den Zuschauer `nr`, wie
/// send_small_an. Sie wartet also nie hinter dem einen Datei-Platz des
/// eigenen Senders (beide Richtungen laufen zugleich). Ja, wenn sie
/// eingereiht ist; sonst ist der Zuschauer gewechselt oder weg (auch
/// KLEIN_GRENZE schliesst ihn): Weg.
fn klein_an(platz: &Mutex<Option<Arc<Leitung>>>, zaehler: &AtomicU64, nr: u64, typ: u8, data: &[u8]) -> Gesendet {
    let Some(l) = leitung_von(platz, zaehler, nr) else { return Gesendet::Weg };
    match klein_auf(&l, typ, data) {
        Ok(()) => Gesendet::Ja,
        Err(_) => Gesendet::Weg,
    }
}

/// Weg des Senders (Host -> Client): ein Datei-Paket (Art::Datei) nur an den
/// Zuschauer `nr`; nach einem Wechsel oder ohne Zuschauer Weg, sonst siehe
/// Leitung::datei_einreihen.
fn datei_an(platz: &Mutex<Option<Arc<Leitung>>>, zaehler: &AtomicU64, nr: u64, typ: u8, data: &[u8]) -> Gesendet {
    match leitung_von(platz, zaehler, nr) {
        Some(l) => l.datei_einreihen(typ, data),
        None => Gesendet::Weg,
    }
}

// ------------------------------------------------------------ Dateien
// Anbindung des Kerns (dateien.rs) an die Host-Rolle; siehe Kopf dieser Datei.

/// Woran die Dateiuebertragung der Host-Rolle haengt: Platz und Zaehler der
/// Zuschauer (fuer die nummerngebundenen Wege), die Ablagebasis, was mit
/// fertig empfangenen Dateien geschieht, wohin die Protokollzeilen gehen, wie
/// lange eine Liste vorgemerkt bleibt, und die Vorgaben des Kerns. Der
/// Dienst nimmt AKTUELL, NR, host_ablage_basis(), clipboard::set_dateien und
/// host::log (DateiUmgebung::dienst); Tests eigene, mit Rekordern.
#[derive(Clone)]
struct DateiUmgebung {
    platz: &'static Mutex<Option<Arc<Leitung>>>,
    zaehler: &'static AtomicU64,
    basis: PathBuf,
    ablegen: Arc<dyn Fn(&[PathBuf]) -> bool + Send + Sync>,
    /// Stand jeder Uebertragung. Die Host-Rolle hat keine Oberflaeche und
    /// uebergeht ihn (ihre Zeilen gehen ins Protokoll); Tests sehen daran
    /// das Ergebnis.
    stand: Arc<dyn Fn(&dateien::Stand) + Send + Sync>,
    /// Protokollzeilen der Dateiuebertragung (Dienst: host::log).
    zeile: Arc<dyn Fn(String) + Send + Sync>,
    /// So lange wartet eine kopierte Liste auf die Faehigkeiten (VORMERKEN).
    vormerken: Duration,
    /// Vorgaben des Kerns; eigene_basis ist host_ablage_basis.
    vorgaben: dateien::Vorgaben,
}

impl DateiUmgebung {
    fn dienst() -> DateiUmgebung {
        DateiUmgebung {
            platz: &AKTUELL,
            zaehler: &NR,
            basis: host_ablage_basis(),
            ablegen: Arc::new(ablage_setzen),
            stand: Arc::new(|_: &dateien::Stand| {}),
            zeile: Arc::new(|z: String| log(z)),
            vormerken: VORMERKEN,
            vorgaben: dateien::Vorgaben { eigene_basis: host_ablage_basis, ..dateien::Vorgaben::default() },
        }
    }

    /// Was Sender bzw. Empfaenger melden: der Stand an `stand`, die Zeile
    /// ins Host-Protokoll - beim Sender mit dem Ziel `ip` (zeile_mit_ziel).
    fn melden(&self, ip: Option<String>) -> impl Fn(dateien::Ereignis) + Send + 'static {
        let stand = self.stand.clone();
        let zeile = self.zeile.clone();
        move |e: dateien::Ereignis| {
            if let Some(s) = &e.stand {
                stand(s);
            }
            if let Some(z) = e.zeile {
                zeile(match &ip {
                    Some(ip) => zeile_mit_ziel(z, ip),
                    None => z,
                });
            }
        }
    }
}

/// Fertig empfangene Dateien: als Dateiliste in die Ablage der Sitzung, in
/// der die Host-Rolle laeuft.
#[cfg(not(test))]
fn ablage_setzen(pfade: &[PathBuf]) -> bool {
    crate::clipboard::set_dateien(crate::protokoll::Herkunft::Host, pfade)
}

/// Unter cfg(test) fasst der Dienstweg die echte Ablage nie an; Tests
/// bringen ihre eigene DateiUmgebung mit Rekorder mit.
#[cfg(test)]
fn ablage_setzen(_pfade: &[PathBuf]) -> bool {
    false
}

static UMGEBUNG: OnceLock<DateiUmgebung> = OnceLock::new();

fn umgebung() -> &'static DateiUmgebung {
    UMGEBUNG.get_or_init(DateiUmgebung::dienst)
}

/// Ablagebasis der Host-Rolle: %TEMP%\QuadChroma-Host-Ablage. ABWEICHUNG von
/// 2.9 (dort teilen Client und Host die Basis QuadChroma-Ablage): eine eigene
/// Basis, damit Client und Host-Rolle auf demselben Rechner einander nichts
/// aufraeumen und die Widerhall-Pruefung (aus_eigener_ablage) je Rolle
/// eindeutig ist. Unter cfg(test) ein eigener Ordner je Lauf.
pub fn host_ablage_basis() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("qc-test-{}-host-ablage", std::process::id()))
    } else {
        std::env::temp_dir().join("QuadChroma-Host-Ablage")
    }
}

/// Beim Start der Host-Rolle: Uebertragungen aelter als 24 h loeschen (2.9)
/// und verwaiste (ihr Prozess lebt nicht mehr, siehe dateien.rs). Liefert die
/// Zahl der geloeschten Verzeichnisse.
pub fn host_ablage_aufraeumen() -> usize {
    dateien::aufraeumen(&host_ablage_basis(), usize::MAX, Some(dateien::HOECHSTALTER))
}

/// Was aus einer kopierten Dateiliste wurde (fuer die Tests).
#[derive(Debug, PartialEq, Eq)]
enum Start {
    KeinZuschauer,
    /// Die Liste stammt aus der eigenen Ablage (2.7 Schritt 2): nichts
    /// gesendet, eine laufende Sendung bleibt.
    EigeneAblage,
    /// Der Zuschauer ist eben gegangen.
    Zu,
    /// Der aktuelle Eingabekanal hat Faehigkeiten ohne FAEHIG_DATEIEN
    /// gemeldet; `gemeldet`: die Zeile ZEILE_ZUSCHAUER_ZU_ALT ging eben
    /// hinaus (einmal je Zuschauer).
    ZuAlt { gemeldet: bool },
    /// Der aktuelle Eingabekanal hat noch nichts gemeldet (oder es gibt
    /// keinen): die Liste wartet hoechstens VORMERKEN auf IN_FAEHIGKEITEN.
    Vorgemerkt,
    /// Der Sender laeuft, mit dieser Kennung.
    Gestartet(u32),
}

/// Die Zeile, wenn eine vorgemerkte Liste verfaellt: was wirklich war, statt
/// "aelterer Client" (Integrationstest Befund 3).
fn zeile_ohne_meldung(kanal: bool, frist: Duration) -> String {
    let s = format!("{:.1}", frist.as_secs_f64()).replace('.', ",");
    if kanal {
        format!("Dateien: nicht gesendet, der Eingabekanal des Zuschauers hat in {s} s keine Faehigkeiten gemeldet")
    } else {
        format!("Dateien: nicht gesendet, der Eingabekanal des Zuschauers stand nach {s} s noch nicht")
    }
}

/// Die Sender-Zeile "Dateien: sende ..." nennt das Ziel (3.3).
fn zeile_mit_ziel(z: String, ip: &str) -> String {
    if z.starts_with("Dateien: sende ") {
        format!("{z} an {ip}")
    } else {
        z
    }
}

/// Eine kopierte Dateiliste an den aktuellen Zuschauer: eine laufende
/// Sendung bricht ab (neuer Inhalt, sie wird der Vorgaenger der neuen),
/// dann startet der Sender - wenn der aktuelle Eingabekanal FAEHIG_DATEIEN
/// gemeldet hat. Hat er Faehigkeiten ohne das Bit gemeldet, einmal je
/// Zuschauer ZEILE_ZUSCHAUER_ZU_ALT. Hat er noch nichts gemeldet (etwa gleich
/// nach dem Verbinden) oder steht noch keiner, wird die Liste vorgemerkt:
/// sie geht hinaus, sobald IN_FAEHIGKEITEN kommt (eingabe_lesen), sonst
/// faellt sie nach u.vormerken mit einer zutreffenden Zeile weg. Der Sender
/// liest und sendet in seinem eigenen Faden; sein Weg ist an die Nummer
/// dieses Zuschauers gebunden.
fn dateien_senden_mit(u: &DateiUmgebung, pfade: Vec<PathBuf>) -> Start {
    let Some(l) = sperre(u.platz).clone() else { return Start::KeinZuschauer };
    // Vor dem Abbrechen (Pflicht der Rollen im Kern): eine Liste aus dem
    // eigenen Empfang ist kein neuer Inhalt des Nutzers.
    if dateien::aus_eigener_ablage(&pfade, &u.basis) {
        (u.zeile)("Dateien: nicht gesendet, sie stammen aus einem Empfang".into());
        return Start::EigeneAblage;
    }
    // Vor der Dateisperre: q und dateien liegen nie ineinander.
    let lage = l.lage();
    let mut d = sperre(&l.dateien);
    if d.zu || lage == Lage::Zu {
        return Start::Zu;
    }
    // Neuer Inhalt ersetzt auch eine vorgemerkte Liste.
    d.vorgemerkt = None;
    match lage {
        Lage::Kann => Start::Gestartet(l.sender_starten(&mut d, u, pfade)),
        Lage::KannNicht | Lage::Zu => {
            Leitung::sendung_beenden(&mut d);
            let gemeldet = !d.kann_nicht_gemeldet;
            d.kann_nicht_gemeldet = true;
            drop(d);
            if gemeldet {
                (u.zeile)(dateien::ZEILE_ZUSCHAUER_ZU_ALT.into());
            }
            Start::ZuAlt { gemeldet }
        }
        Lage::Offen { .. } => {
            Leitung::sendung_beenden(&mut d);
            d.vormerk_nr += 1;
            let nr = d.vormerk_nr;
            d.vorgemerkt = Some(Vorgemerkt { pfade, nr });
            drop(d);
            // Kam IN_FAEHIGKEITEN zwischen lage() oben und dem Vormerken,
            // fand eingabe_lesen noch nichts vor; ohne diese Nachschau ginge
            // die Liste erst mit dem Zeitgeber hinaus, bis zu VORMERKEN
            // spaeter (Hinweis der Gegenpruefung).
            l.vorgemerkte_pruefen(u, None);
            // Der Zeitgeber haelt die Leitung nicht fest.
            let (leitung, u2) = (Arc::downgrade(&l), u.clone());
            let zeitgeber = std::thread::Builder::new().name("qc-dateien-vormerken".into()).spawn(move || {
                std::thread::sleep(u2.vormerken);
                if let Some(l) = leitung.upgrade() {
                    l.vorgemerkte_pruefen(&u2, Some(nr));
                }
            });
            if zeitgeber.is_err() {
                // Ohne Zeitgeber nicht warten: gleich entscheiden.
                l.vorgemerkte_pruefen(u, Some(nr));
            }
            Start::Vorgemerkt
        }
    }
}

/// Neuer Text in der Ablage: eine laufende Sendung bricht ab, eine
/// vorgemerkte Liste faellt weg.
fn sendung_abbrechen_mit(u: &DateiUmgebung) {
    let l = sperre(u.platz).clone();
    if let Some(l) = l {
        l.sender_abbrechen();
    }
}

/// Auftraege des Ablagewaechters, der Reihe nach in einem eigenen Faden:
/// Der Waechter (wndproc) darf nicht warten, und die Pruefung auf die
/// eigene Ablage fragt das Dateisystem (canonicalize, auch Netzpfade). Die
/// Reihenfolge bleibt dabei die des Kopierens - Text nach Dateien bricht die
/// Sendung ab, nicht umgekehrt.
enum Auftrag {
    Senden(Vec<PathBuf>),
    Abbrechen,
}

static AUFTRAEGE: Mutex<Option<std::sync::mpsc::Sender<Auftrag>>> = Mutex::new(None);

fn auftrag(a: Auftrag) {
    let mut s = sperre(&AUFTRAEGE);
    if s.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<Auftrag>();
        let faden = std::thread::Builder::new().name("qc-dateien-auftraege".into()).spawn(move || {
            for a in rx {
                match a {
                    Auftrag::Senden(pfade) => {
                        let _ = dateien_senden_mit(umgebung(), pfade);
                    }
                    Auftrag::Abbrechen => sendung_abbrechen_mit(umgebung()),
                }
            }
        });
        if let Err(e) = faden {
            log(format!("Dateien: Auftragsfaden nicht gestartet: {e}"));
            return;
        }
        *s = Some(tx);
    }
    if let Some(tx) = s.as_ref() {
        let _ = tx.send(a);
    }
}

/// Aus dem Ablagewaechter: der Nutzer hat hier Dateien kopiert. Kehrt sofort
/// zurueck (siehe Auftrag, dateien_senden_mit).
pub fn dateien_senden(pfade: Vec<PathBuf>) {
    auftrag(Auftrag::Senden(pfade));
}

/// Aus dem Ablagewaechter: neuer Text - eine laufende Datei-Sendung bricht
/// ab. Kehrt sofort zurueck.
pub fn datei_sendung_abbrechen() {
    auftrag(Auftrag::Abbrechen);
}

pub fn settings_senden() {
    let mut cur = [0u8; 9];
    cur[0..4].copy_from_slice(&Z.mbit.load(Ordering::Relaxed).to_le_bytes());
    cur[4..6].copy_from_slice(&(Z.fps.load(Ordering::Relaxed) as u16).to_le_bytes());
    cur[6] = Z.gaming.load(Ordering::Relaxed) as u8;
    cur[7] = Z.fest.load(Ordering::Relaxed) as u8;
    cur[8] = Z.ton.load(Ordering::Relaxed) as u8;
    send_small(MSG_SETTINGS, &cur);
}

pub fn strominfo_senden() {
    send_small(MSG_INFO, &super::strominfo());
}

/// Nachricht 12: die Bildschirme des Hosts, wie sie in Z stehen.
fn bildschirme_payload() -> Vec<u8> {
    crate::bildschirm::bildschirme_kodieren(&sperre(&Z.bildschirme))
}

/// Die Bildschirmliste (12) an den aktuellen Zuschauer - nach einem
/// Wechsel, einer geaenderten Liste oder als Antwort auf einen Wunsch.
pub fn bildschirme_senden() {
    send_small(MSG_BILDSCHIRME, &bildschirme_payload());
}

pub fn hoststatus_senden(lage: u8) {
    send_small(MSG_HOSTSTATUS, &[lage, 0]);
}

/// Hoststatus nur an den Zuschauer `nr` (siehe send_small_an) - fuer den
/// Aufnahmefaden nach einer Panik: ein Zuschauer, der inzwischen abgeloest
/// hat, bekommt eine eigene Sitzung und darf die Meldung des alten nicht
/// erben. Liefert, ob sie an ihn ging.
pub fn hoststatus_senden_an(nr: u64, lage: u8) -> bool {
    send_small_an(&AKTUELL, &NR, nr, MSG_HOSTSTATUS, &[lage, 0])
}

/// Stauregel vor dem Encoder, wie stau_vor_dem_encoder in main.m: true =
/// dieses Bild auslassen (zaehlt als "Stau"). Der Aufnahmefaden fragt nach
/// Raster und Encoder-Grenze und vor dem Abholen eines erzwungenen
/// Vollbilds, fuer jedes Bild, das in den Encoder soll - aufgenommen,
/// nachgelegt oder Testbild. Der Encoder sieht dann nur weniger Bilder,
/// jedes codierte hat sein Bezugsbild, und es braucht kein Vollbild.
pub fn stau_vor_dem_encoder() -> bool {
    let Some(l) = aktuell() else { return false };
    if l.stau(budget(Z.gaming.load(Ordering::Relaxed)), now_us()) == Stauurteil::Frei {
        return false;
    }
    Z.stau.fetch_add(1, Ordering::Relaxed);
    true
}

/// Ergebnis eines Bildversands.
#[derive(PartialEq, Eq, Debug)]
pub enum Versand {
    Gesendet,
    /// Kein Zuschauer, oder der wartet noch auf ein Vollbild.
    Verworfen,
    /// Rueckstand ueber der harten Grenze: verworfen, Vollbild erzwungen.
    Stau,
}

/// Eine Zugriffseinheit samt Stempel (Nachricht 5) in EINEM Paket. Ein
/// frisch verbundener Zuschauer bekommt erst ab dem naechsten Vollbild
/// Daten. Gegen Stau wird vor dem Encoder ausgelassen (stau_vor_dem_encoder):
/// ein codiertes Bild geht immer hinaus. Fiele es hier weg, fehlte den
/// folgenden Zwischenbildern ihr Bezug, das naechste Bild muesste ein
/// Vollbild sein - und auf einer zu langsamen Leitung kaemen dann nur noch
/// Vollbilder, die den Stau ihrerseits vergroessern. Nur ueber HARTE_GRENZE
/// faellt es doch weg, mit erzwungenem Vollbild; g_wait_key bleibt dabei
/// stehen, wie auf dem Mac.
pub fn bild_senden(au: &[u8], key: bool, t_cap: u64, wiederholt: bool) -> Versand {
    let Some(l) = aktuell() else { return Versand::Verworfen };
    if Z.wait_key.load(Ordering::Relaxed) && !key {
        return Versand::Verworfen;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let t_enc = now_us();
    if !wiederholt && t_enc > t_cap {
        Z.enc_us.fetch_add(t_enc - t_cap, Ordering::Relaxed);
        Z.enc_n.fetch_add(1, Ordering::Relaxed);
    }
    let mut p = Vec::with_capacity(8 + 24 + 8 + au.len());
    p.extend_from_slice(&kopf(MSG_STAMP, 0, 0, 24));
    p.extend_from_slice(&seq.to_le_bytes());
    p.push(wiederholt as u8);
    p.extend_from_slice(&[0, 0, 0]);
    p.extend_from_slice(&t_cap.to_le_bytes());
    p.extend_from_slice(&t_enc.to_le_bytes());
    p.extend_from_slice(&kopf(MSG_VIDEO, if key { FLAG_KEY } else { 0 }, (seq & 0xffff) as u16, au.len()));
    p.extend_from_slice(au);
    match l.einreihen(p, Art::Bild, Some(HARTE_GRENZE)) {
        Ok(()) => {}
        Err(Abgewiesen::Zu) => return Versand::Verworfen,
        Err(Abgewiesen::Voll) => {
            Z.stau.fetch_add(1, Ordering::Relaxed);
            Z.force_key.store(true, Ordering::Relaxed);
            DROSSEL_HART.melden(None, || {
                format!("Stau ueber der harten Grenze ({} kB): codiertes Bild verworfen, Vollbild erzwungen", HARTE_GRENZE / 1024)
            });
            // Auch hier gilt die Frist - die Konserve fragt nie vor einem
            // Encoder nach dem Stau.
            l.stau(HARTE_GRENZE, now_us());
            return Versand::Stau;
        }
    }
    // Erst wenn das Bild wirklich rausgeht, ist das Warten vorbei.
    Z.wait_key.store(false, Ordering::Relaxed);
    Z.sent_frames.fetch_add(1, Ordering::Relaxed);
    Z.sent_bytes.fetch_add(au.len() as u64, Ordering::Relaxed);
    Versand::Gesendet
}

/// Sendefaden eines Zuschauers: nimmt Pakete aus der Warteschlange und
/// schreibt sie verschluesselt auf die Leitung, in Stuecken (SCHREIBSTUECK)
/// und nach jedem Stueck mit Buchfuehrung fuer die Stauregel. Ein Paket geht
/// immer ganz hinaus, auch nach einer Abloesung - das Schlusswort muss an
/// einer Nachrichtengrenze beginnen. Ein Fehler beim Schreiben heisst: der
/// Zuschauer ist weg.
fn sendefaden(l: Arc<Leitung>, mut sock: secure::Secure) {
    loop {
        let paket = {
            let mut q = sperre(&l.q);
            while q.pakete.is_empty() && q.offen {
                q = l.cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            // Geschlossen (abgeloest oder weg): Was noch wartet, gehoert
            // niemandem mehr - nicht in die gekappte Leitung schreiben.
            // Ein Abgeloester bekommt nur noch sein Schlusswort.
            if !q.offen {
                q.leeren();
                let schluss = q.schlusswort.take();
                drop(q);
                if let Some((s, art)) = schluss {
                    schlusswort_senden(&mut sock, &s, art, &l.ip);
                }
                break;
            }
            match q.nehmen() {
                Some(p) => {
                    q.im_schreiben = p.len();
                    p
                }
                None => break,
            }
        };
        let mut fehler = None;
        for stueck in paket.chunks(SCHREIBSTUECK) {
            if let Err(e) = sock.write_all(stueck) {
                fehler = Some(e);
                break;
            }
            let mut q = sperre(&l.q);
            q.im_schreiben -= stueck.len();
            q.geschrieben += stueck.len() as u64;
        }
        if let Some(e) = fehler {
            // Scheitert das Schreiben, weil `schliessen` die Leitung eben
            // gekappt hat, ist der Zuschauer nicht weg, sondern abgeloest.
            if sperre(&l.q).offen {
                log(format!("Zuschauer weg: {e}"));
            }
            l.schliessen();
            break;
        }
    }
    {
        let _einspeisen = sperre(&EINSPEISEN);
        let mut a = sperre(&AKTUELL);
        if a.as_ref().map(|x| Arc::ptr_eq(x, &l)).unwrap_or(false) {
            *a = None;
            drop(a);
            eingabe::alle_tasten_loslassen();
            log("Zuschauer getrennt - kein Zuschauer");
        }
    }
    let mut q = sperre(&l.q);
    q.beendet = true;
    l.ende.notify_all();
}

/// Die letzte Nachricht an einen abgeloesten oder verabschiedeten
/// Zuschauer, mit Frist: wer sie nicht abnimmt, haelt den Faden nicht fest.
/// Danach ist die Leitung zu - das Schlusswort liegt dann schon im Kernel
/// und geht vor dem FIN hinaus.
fn schlusswort_senden(sock: &mut secure::Secure, s: &[u8], art: Schluss, ip: &str) {
    sock.socket().set_write_timeout(Some(FRIST_SCHLUSSWORT)).ok();
    match sock.write_all(s) {
        Ok(()) => log(format!("{} an {ip} gemeldet", art.name())),
        Err(e) => log(format!("{} an {ip} nicht zugestellt: {e}", art.name())),
    }
    let _ = sock.socket().shutdown(Shutdown::Both);
}

/// Sendepuffer im Kernel klein halten, damit ein blockierender write die
/// Leitung spiegelt (siehe Stauregel oben).
fn sendepuffer_setzen(s: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_SNDBUF};
    let wert = (SNDBUF as i32).to_ne_bytes();
    unsafe {
        setsockopt(SOCKET(s.as_raw_socket() as usize), SOL_SOCKET, SO_SNDBUF, Some(&wert));
    }
}

fn annahme_bild(listener: TcpListener, marke: Arc<Laufmarke>) {
    for stream in listener.incoming() {
        // Gestoppt: die Verbindung, die hier weckte, faellt mit dem
        // Listener zu (siehe stoppen).
        if !marke.gilt() {
            break;
        }
        let Ok(stream) = stream else { continue };
        // Der Handschlag laeuft in einem eigenen Faden: wer ihn nicht zu
        // Ende bringt, haelt nur seinen Platz, nicht die Annahme.
        let Some(platz) = PLAETZE_BILD.belegen(absender(&stream), "Bildkanal") else { continue };
        let m = marke.clone();
        std::thread::spawn(move || bild_annehmen(stream, platz, &m));
    }
}

fn bild_annehmen(stream: TcpStream, platz: Platz, marke: &Laufmarke) {
    // Schluessel der Drossel: dieselbe Adresse wie beim Handschlagplatz.
    let absender_ip = platz.ip;
    let von = Some(absender_ip);
    let ip = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let port = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
    sendepuffer_setzen(&stream);

    // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
    let sock = match secure::Secure::accept(stream, &noise::prologue_video(), &marke.schluessel) {
        Ok(s) => s,
        Err(e) => {
            DROSSEL_HANDSCHLAG_BILD.melden(von, || format!("Handschlag mit {ip} gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    let mut sock = sock;
    let fp = sock.peer_fingerprint();
    let sas = sock.sas.clone();
    // Der Name aus Nachricht 3 (unbeglaubigt, nur Anzeige); aeltere Clients
    // senden keinen - dann steht die Adresse da, ebenso bei einem Namen, der
    // eine ID vortaeuschen koennte (einlass::anzeigename).
    let name = super::einlass::anzeigename(&sock.nachricht3, &ip);

    // Einlass (Spezifikation Pairing v1, 3.1-3.4): ein Geraet aus der Liste
    // kommt wie bisher herein; ein unbekanntes durchlaeuft die Zugangsphase
    // (Passwort oder "Zulassen"), ohne dass dabei eine globale Sperre
    // gehalten wird - ein laufender Zuschauer merkt davon nichts. Wer nicht
    // herein darf, hat sein Ergebnis (22) schon bekommen; die Leitung faellt
    // mit `sock` zu.
    let Some(einlass) = super::einlass::einlass() else {
        log(format!("Abgewiesen: {name} ({ip}) - kein Einlass eingerichtet"));
        return;
    };
    // Stand des Entfernen-Zaehlers vor dem Blick in die Liste (siehe unten).
    let mut entfernt_stand = einlass.entfernt_stand();
    let ausgang = einlass.pruefen(&mut sock, absender_ip, &name);
    if !ausgang.herein() {
        return;
    }
    #[cfg(test)]
    {
        let haken = sperre(&NACH_EINLASS).take();
        if let Some(h) = haken {
            h();
        }
    }

    // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
    // Fenstergroesse und Format kennt, bevor das erste Bild kommt.
    let mut hello = Vec::with_capacity(4 + 8 + 8);
    hello.extend_from_slice(MAGIC);
    hello.extend_from_slice(&kopf(MSG_INFO, 0, 0, 8));
    hello.extend_from_slice(&super::strominfo());

    let mut leitung = Leitung::neu(sock.abbruchgriff(), sock.peer.clone(), sock.handshake_hash.clone(), ip.clone());
    leitung.name = name.clone();
    let leitung = Arc::new(leitung);
    // Ein neuer Zuschauer ersetzt den alten: dessen Eingabekanal wird
    // gekappt, seine Warteschlange verworfen; er bekommt nur noch
    // MSG_ABGELOEST, dann endet sein Sendefaden (unten wird darauf gewartet).
    let (alt, nr) = loop {
        let einspeisen = sperre(&EINSPEISEN);
        let mut a = sperre(&AKTUELL);
        // Die Freigabe endete, waehrend dieser Handschlag lief (stoppen
        // nimmt der Marke unter AKTUELL die Geltung): kein neuer Zuschauer.
        if !marke.gilt() {
            drop(a);
            drop(einspeisen);
            log(format!("Zuschauer {name} ({ip}) abgewiesen: die Freigabe endete waehrend des Handschlags"));
            return;
        }
        // Wurde seit dem Blick in die Liste ein Geraet entfernt, hat
        // zuschauer_verabschieden dieses hier nicht gesehen (es stand noch nicht
        // in AKTUELL) - vielleicht war es genau seins ("Alle entfernen"
        // trifft auch ein eben zugelassenes). Dann ausserhalb der Sperren
        // noch einmal nachsehen (Datei). Unter AKTUELL gilt der Stand erst,
        // wenn seitdem nichts mehr entfernt wurde: jedes spaetere Entfernen
        // zaehlt erst hoch und trennt dann - und sieht den Neuen in AKTUELL.
        let stand = einlass.entfernt_stand();
        if stand != entfernt_stand {
            entfernt_stand = stand;
            drop(a);
            drop(einspeisen);
            if !einlass.noch_bekannt(&leitung.peer, absender_ip) {
                log(format!(
                    "Zuschauer {name} ({ip}), ID {} abgewiesen: sein Geraet wurde eben aus der Liste entfernt",
                    zugang::id_text(zugang::geraete_id(&leitung.peer))
                ));
                return;
            }
            continue;
        }
        let alt = a.take();
        if let Some(alt) = &alt {
            log(format!("Zuschauer abgeloest: {}", alt.ip));
            alt.abloesen();
            eingabe::alle_tasten_loslassen();
        }
        // Ein Testbild ueberlebt den Zuschauer nicht - auch nicht, wenn ihn
        // ein Neuer ohne Luecke abloest und die Aufnahme dabei weiterlaeuft.
        // Hier frei von Wettlaeufen: der Eingabekanal des Alten speist nur
        // unter EINSPEISEN ein, der des Neuen bindet sich erst an AKTUELL.
        if Z.testbild.swap(false, Ordering::Relaxed) {
            log("Testbild aus (Zuschauer gewechselt)");
        }
        // Vollbild fuer den Neuen, bevor ihn irgendwer in AKTUELL sieht:
        // ein Bild, das die Aufnahme ab jetzt codiert, ist eines - und geht
        // an ihn (bild_senden wartet auf AKTUELL).
        Z.force_key.store(true, Ordering::Relaxed);
        Z.wait_key.store(true, Ordering::Relaxed);
        let _ = leitung.einreihen(hello, Art::Klein, None);
        let nr = NR.fetch_add(1, Ordering::Relaxed) + 1;
        leitung.nr.store(nr, Ordering::Relaxed);
        *a = Some(leitung.clone());
        break (alt, nr);
    };
    let l2 = leitung.clone();
    std::thread::spawn(move || sendefaden(l2, sock));

    // Zeigerform und Tonformat gehen dem neuen Zuschauer erneut zu.
    super::zeiger::neu_senden();
    super::ton::info_zuruecksetzen();
    settings_senden();
    send_small(MSG_CODECS, &encoder::codecs_payload());
    // Was dieser Host kann (2.2): Dateien Fassung 1 und die Bildschirmwahl -
    // nur an genau diesen Zuschauer; danach die Bildschirme (12). Ein
    // aelterer Client uebergeht Typ 11 und 12.
    send_small_an(&AKTUELL, &NR, nr, MSG_FAEHIGKEITEN, &dateien::faehigkeiten_kodieren(FAEHIG_DATEIEN | FAEHIG_BILDSCHIRM));
    send_small_an(&AKTUELL, &NR, nr, MSG_BILDSCHIRME, &bildschirme_payload());
    log(format!(
        "Zuschauer verbunden: {name} ({ip}:{port}), ID {}, verschluesselt, Gegenstelle {fp}, Vergleichscode {sas}{}",
        zugang::id_text(zugang::geraete_id(&leitung.peer)),
        match ausgang {
            super::einlass::Ausgang::Passwort => " - eben per Passwort erlaubt",
            super::einlass::Ausgang::Zugelassen => " - eben am Host zugelassen",
            _ => "",
        }
    ));
    // Der Neue laeuft schon; jetzt erst auf den Abgeloesten warten: solange
    // er abnimmt (Rest des laufenden Pakets, dann das Schlusswort), sonst
    // wird seine Bildleitung gekappt.
    if let Some(alt) = alt {
        alt.abloesung_abschliessen(FRIST_SCHLUSSWORT * 2);
    }
}

// ------------------------------------------------------------ Eingabe-Teil
// Zweite Verbindung, nur fuer Maus, Tastatur und Zwischenablage (Text und
// Dateien vom Client, Quittungen fuer Dateien vom Host). Getrennt vom Bild,
// damit eine Mausbewegung nie hinter einem Vollbild haengt.

fn annahme_eingabe(listener: TcpListener, marke: Arc<Laufmarke>) {
    for stream in listener.incoming() {
        if !marke.gilt() {
            break;
        }
        let Ok(stream) = stream else { continue };
        stream.set_nodelay(true).ok();

        // Der Eingabekanal darf erst aufmachen, wenn der Bildkanal steht: sein
        // Prologue enthaelt dessen Pruefsumme. Wer die nicht kennt, kommt hier
        // nicht durch - damit kann niemand nur die Tastatur uebernehmen.
        let von = absender(&stream);
        let Some(bild) = aktuell() else {
            DROSSEL_KEIN_BILD.melden(Some(von), || format!("Eingabekanal abgewiesen: kein Bildkanal offen ({von})"));
            continue;
        };
        let Some(platz) = PLAETZE_EINGABE.belegen(von, "Eingabekanal") else { continue };
        let m = marke.clone();
        std::thread::spawn(move || eingabe_annehmen(stream, bild, platz, &m));
    }
}

fn eingabe_annehmen(stream: TcpStream, bild: Arc<Leitung>, platz: Platz, marke: &Laufmarke) {
    let ip = platz.ip;
    let von = Some(ip);
    let mut sock = match secure::Secure::accept(stream, &noise::prologue_input(&bild.hh), &marke.schluessel) {
        Ok(s) => s,
        Err(e) => {
            DROSSEL_HANDSCHLAG_EINGABE.melden(von, || format!("Eingabekanal: Handschlag mit {ip} gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    if sock.peer != bild.peer {
        DROSSEL_EINGABE_ABGEWIESEN.melden(von, || format!("Eingabekanal abgewiesen: andere Gegenstelle als beim Bild ({ip})"));
        return;
    }
    // Erst jetzt an den Zuschauer binden - und nur, wenn er noch der
    // aktuelle ist: ein Wechsel waehrend des Handschlags macht diese
    // Leitung wertlos. Ohne Abbruchgriff liesse sie sich nicht kappen.
    let nr = EINGABE_NR.fetch_add(1, Ordering::Relaxed);
    let einspeisen = sperre(&EINSPEISEN);
    let Some(griff) = sock.abbruchgriff() else {
        DROSSEL_EINGABE_ABGEWIESEN.melden(von, || format!("Eingabekanal abgewiesen: Leitung nicht zu fassen ({ip})"));
        return;
    };
    if !bild.eingabe_binden(nr, griff) {
        DROSSEL_EINGABE_ABGEWIESEN.melden(von, || format!("Eingabekanal abgewiesen: Zuschauer inzwischen gewechselt ({ip})"));
        return;
    }
    drop(einspeisen);
    log("Eingabekanal verbunden, verschluesselt");

    let _ = eingabe_lesen(&mut sock, &bild, nr, umgebung());
    // Galt der Kanal bis zuletzt, gibt er seine Tasten selbst frei. Sonst hat
    // das schon getan, wer ihn abgeloest hat - und ein spaet endender alter
    // Faden liesse sonst die Tasten des neuen Zuschauers los.
    let _einspeisen = sperre(&EINSPEISEN);
    let galt = bild.eingabe_loesen(nr);
    log("Eingabekanal getrennt");
    if galt {
        eingabe::alle_tasten_loslassen();
    }
}

/// Warum die Leseschleife eines Eingabekanals endete.
#[derive(Debug, PartialEq, Eq)]
enum Kanalende {
    /// Lesefehler, Leitung zu oder gekappt.
    Leitung,
    /// Eine Nachricht ueber der Grenze ihres Typs.
    ZuGross { typ: u8, len: usize },
    /// Der Kanal gilt nicht mehr (Zuschauer abgeloest, neuer Kanal).
    Abgeloest,
}

/// Obergrenze je Nachricht auf dem Eingabekanal. Datei-Nachrichten und
/// Faehigkeiten (50-53, 69) haben eigene Grenzen (2.6, dateien::eingangsgrenze)
/// - VOR der alten Regel. Text kann gross sein (bis 4 MB); alles andere ist
/// winzig: ein unbekannter Typ ueber 256 Byte beendet den Kanal wie bisher.
fn eingangsgrenze(typ: u8) -> usize {
    match dateien::eingangsgrenze(typ) {
        Some(g) => g,
        None if typ == IN_CLIP => 4 * 1024 * 1024,
        None => 256,
    }
}

/// Die Leseschleife des Eingabekanals `nr` von Zuschauer `bild`. Gelesen
/// wird ausserhalb aller Sperren; unter EINSPEISEN wird nur geprueft, ob der
/// Kanal noch gilt, und dann eingespeist bzw. weitergegeben - Dateien ohne
/// Plattenarbeit (Empfaenger bzw. Sender des Zuschauers, siehe DateiStand).
fn eingabe_lesen(sock: &mut secure::Secure, bild: &Leitung, nr: u64, u: &DateiUmgebung) -> Kanalende {
    let mut hdr = [0u8; 8];
    let mut payload: Vec<u8> = Vec::new();
    loop {
        if sock.read_exact(&mut hdr).is_err() {
            return Kanalende::Leitung;
        }
        let typ = hdr[0];
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        let grenze = eingangsgrenze(typ);
        if len > grenze {
            log(format!("Eingabekanal: Nachricht {typ} mit {len} Byte ueber der Grenze von {grenze} Byte - Kanal beendet"));
            return Kanalende::ZuGross { typ, len };
        }
        payload.resize(len, 0);
        if len > 0 && sock.read_exact(&mut payload).is_err() {
            return Kanalende::Leitung;
        }
        // Den Empfaenger (samt Faden) gibt es vor EINSPEISEN.
        if typ == DATEI_ANGEBOT {
            bild.empfaenger_bereit(u);
        }
        // Abgeloest? Dann nichts mehr einspeisen - auch nicht, was schon
        // entschluesselt im Puffer lag. Pruefen und Ausfuehren unter EINSPEISEN.
        let einspeisen = sperre(&EINSPEISEN);
        if !bild.gilt(nr) {
            return Kanalende::Abgeloest;
        }
        let mut faehigkeiten = false;
        match typ {
            IN_MOVE | IN_BUTTON | IN_SCROLL | IN_KEY => eingabe::verarbeiten(typ, &payload),
            IN_CLIP => {
                if let Ok(text) = std::str::from_utf8(&payload) {
                    crate::clipboard::set(text);
                }
            }
            IN_TIME => {
                // Zeitabgleich: die Frage traegt die Uhrzeit des Clients,
                // die Antwort gibt sie zurueck und haengt die des Hosts an.
                if len >= 8 {
                    let mut out = [0u8; 16];
                    out[0..8].copy_from_slice(&payload[0..8]);
                    out[8..16].copy_from_slice(&now_us().to_le_bytes());
                    send_small(MSG_TIME, &out);
                }
            }
            IN_SETTINGS => {
                if len >= 8 {
                    let m = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let f = u16::from_le_bytes([payload[4], payload[5]]) as u32;
                    // Ein Client ohne das neunte Byte will Ton.
                    let ton = if len >= 9 { payload[8] != 0 } else { true };
                    super::apply_settings(m, f, payload[6] != 0, payload[7] != 0, ton);
                }
            }
            IN_TESTBILD => {
                // Der Aufnahmefaden setzt es um und meldet es; mit einer
                // Konserve gibt es keinen Encoder, der es speisen koennte.
                if len >= 1 {
                    let an = payload[0] != 0;
                    if Z.konserve.load(Ordering::Relaxed) {
                        log(format!("Testbild {}: mit Konserve nicht moeglich", if an { "an" } else { "aus" }));
                    } else {
                        Z.testbild.store(an, Ordering::Relaxed);
                    }
                }
            }
            IN_CODEC => {
                if len >= 1 {
                    encoder::codec_wunsch(payload[0] as usize);
                }
            }
            IN_BILDSCHIRM => {
                // Bildschirmwunsch: nur vormerken - der Wechsel selbst
                // gehoert in den Aufnahmefaden. Ungueltiges (Laenge, UTF-8,
                // Steuerzeichen) wird uebergangen, der Kanal bleibt.
                match crate::bildschirm::wunsch_lesen(&payload) {
                    Some(w) => super::aufnahme::bildschirm_wunsch(w),
                    None => log(format!("Bildschirmwunsch: ungueltig ({len} Byte) - uebergangen")),
                }
            }
            // Dateien Client -> Host: nur in die Warteschlange des
            // Empfaenger-Fadens; die Nutzlast wandert ohne Kopie hinueber.
            DATEI_ANGEBOT | DATEI_STUECK | DATEI_ENDE => bild.datei_eingang(typ, std::mem::take(&mut payload)),
            // Quittung fuer die Sendung Host -> Client.
            DATEI_QUITTUNG => bild.datei_quittung(&payload),
            IN_FAEHIGKEITEN => {
                if let Some(bits) = dateien::faehigkeiten_lesen(&payload) {
                    bild.faehigkeiten_setzen(nr, bits);
                    faehigkeiten = true;
                    log(format!(
                        "Eingabekanal: Faehigkeiten {bits:#x}{}",
                        if dateien::kann_dateien(bits) { " (Dateien)" } else { "" }
                    ));
                }
            }
            _ => {}
        }
        drop(einspeisen);
        // Eine vorgemerkte Liste wartete auf genau diese Meldung. Ausserhalb
        // von EINSPEISEN: das Starten des Senders ist ein Fadenstart.
        if faehigkeiten {
            bild.vorgemerkte_pruefen(u, None);
        }
    }
}

// ------------------------------------------------------------ Bekanntgabe
// Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, damit Clients
// ihn ohne eingetippte Adresse finden. An die Rundrufadresse JEDER aktiven
// IPv4-Netzwerkkarte, wie auf dem Mac - die allgemeine 255.255.255.255 ist
// nur der Rueckfall, falls sich die Karten nicht abfragen lassen.

/// Rundrufadressen aller aktiven IPv4-Karten (Unicast + Praefixlaenge ->
/// Broadcast), ohne Loopback.
fn rundruf_adressen() -> Vec<std::net::Ipv4Addr> {
    use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    let mut adressen = Vec::new();
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        let r = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if r == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if r != 0 {
            return adressen;
        }
        break;
    }
    let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !a.is_null() {
            let ad = &*a;
            // OperStatus 1 = IfOperStatusUp; IfType 24 = Loopback.
            if ad.OperStatus.0 == 1 && ad.IfType != 24 {
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    let ua = &*u;
                    let sa = ua.Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let ip = u32::from_be(sin.sin_addr.S_un.S_addr);
                        let praefix = ua.OnLinkPrefixLength as u32;
                        if praefix < 32 {
                            let maske = if praefix == 0 { 0 } else { !0u32 << (32 - praefix) };
                            adressen.push(std::net::Ipv4Addr::from(ip | !maske));
                        }
                    }
                    u = ua.Next;
                }
            }
            a = ad.Next;
        }
    }
    adressen
}

/// Das Paket der Bekanntgabe (Spezifikation Pairing v1, Abschnitt 2):
/// Rechnername, dahinter ID und Flags - Bit 0, wenn am Host jemand
/// "Zulassen" klicken kann.
fn bekanntgabe_paket(port: u16, name: &str, id: u32, zulassen: bool) -> Vec<u8> {
    zugang::bekanntgabe(port, name, id, if zulassen { BEACON_FLAG_ZULASSEN } else { 0 })
}

fn bekanntgabe(port: u16, marke: Arc<Laufmarke>, ziele: fn() -> Vec<Ipv4Addr>) {
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) else { return };
    sock.set_broadcast(true).ok();
    let mut name = zugang::geraetename();
    let mut erste = true;
    while marke.gilt() {
        // Je Runde neu: ob "Zulassen" geht, haengt an der Oberflaeche; der
        // Name am Fenster "Geraetename" - ein neuer gilt ab der naechsten
        // Runde, ohne Neustart.
        let (id, zulassen) = super::einlass::einlass().map(|e| (e.id(), e.zulassen_moeglich())).unwrap_or((0, false));
        let jetzt = zugang::geraetename();
        if jetzt != name {
            log(format!("Bekanntgabe: Name jetzt {jetzt} (vorher {name})"));
            name = jetzt;
        }
        let pkt = bekanntgabe_paket(port, &name, id, zulassen);
        let mut ziele = ziele();
        if ziele.is_empty() {
            ziele.push(Ipv4Addr::BROADCAST);
        }
        let mut gesendet = 0;
        for z in &ziele {
            if sock.send_to(&pkt, (*z, port + 2)).is_ok() {
                gesendet += 1;
            }
        }
        if erste {
            log(format!(
                "Bekanntgabe: an {gesendet} Netze, Port {}, Name {name}, ID {}",
                port + 2,
                zugang::id_text(id)
            ));
            erste = false;
        }
        if marke.pause(Duration::from_secs(2)) {
            break;
        }
    }
}

/// Ein Lauf des Zuschauerplatzes (start bis stoppen): der Schluessel, mit
/// dem seine Handschlaege laufen, und ob er noch gilt. Annahmefaeden,
/// Handschlagfaeden und die Bekanntgabe halten ihn.
struct Laufmarke {
    schluessel: Vec<u8>,
    /// Faellt unter AKTUELL auf false (stoppen): wer danach mit seinem
    /// Handschlag fertig wird, wird kein Zuschauer mehr (bild_annehmen).
    gilt: AtomicBool,
    /// Weckt die Bekanntgabe aus ihrer Pause (stoppen).
    wecker: (Mutex<()>, Condvar),
}

impl Laufmarke {
    fn neu(schluessel: Vec<u8>) -> Arc<Laufmarke> {
        Arc::new(Laufmarke { schluessel, gilt: AtomicBool::new(true), wecker: (Mutex::new(()), Condvar::new()) })
    }

    fn gilt(&self) -> bool {
        self.gilt.load(Ordering::SeqCst)
    }

    /// Unter AKTUELL die Geltung nehmen, dann die Bekanntgabe wecken.
    fn beenden(&self) {
        {
            let _a = sperre(&AKTUELL);
            self.gilt.store(false, Ordering::SeqCst);
        }
        // Unter der Sperre des Weckers: `pause` prueft `gilt` unter ihr,
        // der Weckruf geht also nie zwischen Pruefen und Warten verloren.
        let _w = sperre(&self.wecker.0);
        self.wecker.1.notify_all();
    }

    /// Bis zu `dauer` warten; true, wenn der Lauf inzwischen beendet ist.
    fn pause(&self, dauer: Duration) -> bool {
        let w = sperre(&self.wecker.0);
        let _ = self.wecker.1.wait_timeout_while(w, dauer, |_| self.gilt());
        !self.gilt()
    }
}

/// Der laufende Zuschauerplatz.
struct Lauf {
    marke: Arc<Laufmarke>,
    port: u16,
    /// Wohin die Weckverbindungen gehen: die Adressen der beiden Listener
    /// (an 0.0.0.0 gebunden: 127.0.0.1).
    wecken: Vec<std::net::SocketAddr>,
    faeden: Vec<(&'static str, std::thread::JoinHandle<()>)>,
}

static LAUF: Mutex<Option<Lauf>> = Mutex::new(None);

/// So lange wartet `stoppen` hoechstens darauf, dass die Faeden eines Laufs
/// enden.
const STOPP_FRIST: Duration = Duration::from_secs(3);

/// Annahmefaeden und Bekanntgabe starten. Der Einlass (einlass::einrichten)
/// muss vorher stehen. Scheitert das Binden (Port belegt), laesst sich
/// `start` spaeter noch einmal rufen; ebenso nach `stoppen`.
pub fn start(port: u16, priv_key: Vec<u8>) -> Result<(), String> {
    start_mit(Ipv4Addr::UNSPECIFIED, port, priv_key, rundruf_adressen)
}

/// `start` an der Adresse `adresse`, die Bekanntgabe an `ziele` (Tests:
/// Loopback statt aller Netze).
fn start_mit(adresse: Ipv4Addr, port: u16, priv_key: Vec<u8>, ziele: fn() -> Vec<Ipv4Addr>) -> Result<(), String> {
    let mut lauf = sperre(&LAUF);
    if let Some(l) = lauf.as_ref() {
        return Err(format!("Zuschauerplatz laeuft schon (Port {})", l.port));
    }
    let bild = TcpListener::bind((adresse, port)).map_err(|e| format!("Bild-Port {port} nicht verfuegbar: {e}"))?;
    let eingabe = TcpListener::bind((adresse, port + 1))
        .map_err(|e| format!("Eingabe-Port {} nicht verfuegbar: {e}", port + 1))?;
    let weckziel = |l: &TcpListener| {
        l.local_addr().map(|mut a| {
            if a.ip().is_unspecified() {
                a.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
            }
            a
        })
    };
    let wecken = vec![
        weckziel(&bild).map_err(|e| format!("Bild-Port {port}: {e}"))?,
        weckziel(&eingabe).map_err(|e| format!("Eingabe-Port {}: {e}", port + 1))?,
    ];
    let marke = Laufmarke::neu(priv_key);
    let mut faeden = Vec::new();
    let mut starten = |name: &'static str, faden: &'static str, f: Box<dyn FnOnce() + Send>| -> Result<(), String> {
        let h = std::thread::Builder::new().name(faden.into()).spawn(f).map_err(|e| format!("{name}: kein Faden ({e})"))?;
        faeden.push((name, h));
        Ok(())
    };
    let (m1, m2, m3) = (marke.clone(), marke.clone(), marke.clone());
    let r = starten("Bildkanal", "qc-annahme-bild", Box::new(move || annahme_bild(bild, m1)))
        .and_then(|_| starten("Eingabekanal", "qc-annahme-eingabe", Box::new(move || annahme_eingabe(eingabe, m2))))
        .and_then(|_| starten("Bekanntgabe", "qc-bekanntgabe", Box::new(move || bekanntgabe(port, m3, ziele))));
    let neu = Lauf { marke, port, wecken, faeden };
    if let Err(e) = r {
        // Was schon laeuft, wieder abbauen.
        lauf_beenden(neu);
        return Err(e);
    }
    *lauf = Some(neu);
    Ok(())
}

/// Den Zuschauerplatz schliessen: beide Listener zu, die Bekanntgabe aus
/// (siehe Kopf). Kehrt zurueck, wenn die Ports frei sind (hoechstens
/// STOPP_FRIST). Ein verbundener Zuschauer bleibt verbunden - wer die
/// Freigabe beendet, verabschiedet ihn (abschied_beim_beenden). Liefert, ob
/// ein Lauf lief.
pub fn stoppen() -> bool {
    // LAUF bleibt gesperrt, bis die Ports frei sind: ein `start` gleich
    // danach wartet und bindet dann, statt an den noch offenen Ports zu
    // scheitern. Reihenfolge: LAUF, dann AKTUELL (Laufmarke::beenden).
    let mut lauf = sperre(&LAUF);
    let Some(l) = lauf.take() else { return false };
    let port = l.port;
    if lauf_beenden(l) {
        log(format!("Zuschauerplatz geschlossen: Port {port} und {} zu, Bekanntgabe aus", port + 1));
    }
    true
}

/// Marke beenden, Annahmefaeden wecken und auf alle Faeden warten. true,
/// wenn alle rechtzeitig endeten.
fn lauf_beenden(l: Lauf) -> bool {
    l.marke.beenden();
    for a in &l.wecken {
        // Die Weckverbindung selbst nimmt niemand an: der Annahmefaden sieht
        // die beendete Marke und schliesst den Listener samt ihr.
        let _ = TcpStream::connect_timeout(a, Duration::from_secs(1));
    }
    let bis = Instant::now() + STOPP_FRIST;
    let mut alle = true;
    for (name, f) in l.faeden {
        while !f.is_finished() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        if f.is_finished() {
            let _ = f.join();
        } else {
            alle = false;
            log(format!("Zuschauerplatz: {name} endet nicht binnen {} s - sein Port bleibt womoeglich belegt", STOPP_FRIST.as_secs()));
        }
    }
    alle
}

/// Laeuft der Zuschauerplatz (zwischen start und stoppen)?
#[cfg_attr(not(test), allow(dead_code))]
pub fn laeuft() -> bool {
    sperre(&LAUF).is_some()
}

/// Tests, die den einen Zuschauerplatz des Prozesses anfassen (LAUF, der
/// Zuschauer in AKTUELL, Abschied an ihn), laufen nacheinander.
#[cfg(test)]
pub(super) fn platz_pruefung() -> MutexGuard<'static, ()> {
    static PLATZ: Mutex<()> = Mutex::new(());
    sperre(&PLATZ)
}

/// Fuer Tests: Kandidaten P fuer den Zuschauerplatz, so dass P, P+1 und
/// P+2 in 20000..32000 liegen - zufaellig gestreut und ausserhalb der
/// Bereiche, aus denen Windows, macOS (ab 49152) und Linux (ab 32768)
/// kurzlebige Ports vergeben. Einen Port mit bind(127.0.0.1:0) zu ziehen
/// taugt dafuer nicht: Windows vergibt diese der Reihe nach, steht der
/// Zaehler kurz vor 65535, liegen viele Ziehungen hintereinander so hoch,
/// dass P+2 nicht mehr passt. Belegte Kandidaten scheitern beim Binden und
/// werden uebersprungen.
#[cfg(test)]
fn testport_kandidaten() -> impl Iterator<Item = u16> {
    use std::hash::BuildHasher;
    const ANFANG: u16 = 20000;
    const ENDE: u16 = 32000;
    let zufall = std::collections::hash_map::RandomState::new();
    (0u32..200).map(move |i| ANFANG + (zufall.hash_one(i) % u64::from(ENDE - ANFANG - 2)) as u16)
}

/// Fuer Tests: den Zuschauerplatz auf Loopback starten, an freien Ports P
/// und P+1 (die Bekanntgabe geht an 127.0.0.1:P+2). Liefert P.
#[cfg(test)]
pub(super) fn start_loopback(schluessel: &[u8]) -> u16 {
    for p in testport_kandidaten() {
        if start_mit(Ipv4Addr::LOCALHOST, p, schluessel.to_vec(), || vec![Ipv4Addr::LOCALHOST]).is_ok() {
            return p;
        }
    }
    panic!("keine freien Ports auf Loopback");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// `schliessen` kappt Bildleitung und eingetragenen Eingabekanal: ein
    /// blockierendes Lesen auf der Gegenseite endet sofort, obwohl hier noch
    /// weitere Griffe an denselben Leitungen offen sind. Danach laesst sich
    /// kein Eingabekanal mehr eintragen.
    #[test]
    fn schliessen_kappt_eingabe_und_bild() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let mut ein_c = TcpStream::connect(addr).unwrap();
        let (ein_h, _) = l.accept().unwrap();
        let mut bild_c = TcpStream::connect(addr).unwrap();
        let (bild_h, _) = l.accept().unwrap();
        let leitung = Leitung::neu(bild_h.try_clone().ok(), Vec::new(), Vec::new(), String::new());
        assert!(leitung.eingabe_binden(1, ein_h.try_clone().unwrap()));
        ein_c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        bild_c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let faden = std::thread::spawn(move || {
            let mut b = [0u8; 1];
            let t0 = Instant::now();
            let e = ein_c.read(&mut b);
            let b2 = bild_c.read(&mut b);
            (matches!(e, Ok(0)) || e.is_err(), matches!(b2, Ok(0)) || b2.is_err(), t0.elapsed())
        });
        std::thread::sleep(Duration::from_millis(200));
        leitung.schliessen();
        let (ein_zu, bild_zu, dauer) = faden.join().unwrap();
        assert!(ein_zu && bild_zu);
        // Deutlich unter der Lesefrist von 5 s: gekappt, nicht abgelaufen.
        assert!(dauer < Duration::from_secs(2), "{dauer:?}");
        assert!(!leitung.eingabe_binden(2, ein_h.try_clone().unwrap()));
        assert!(!leitung.offen());
    }

    /// Liest vom Bildkanal, bis eine Zeitantwort kommt; gibt die
    /// zurueckgespiegelte Client-Uhrzeit zurueck.
    fn zeitantwort(s: &mut secure::Secure) -> Result<u64, String> {
        loop {
            let mut h = [0u8; 8];
            s.read_exact(&mut h)?;
            let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
            let mut p = vec![0u8; len];
            s.read_exact(&mut p)?;
            if h[0] == MSG_TIME && len >= 8 {
                return Ok(u64::from_le_bytes(p[0..8].try_into().unwrap()));
            }
        }
    }

    /// Liest vom Bildkanal bis zu MSG_ABGELOEST; danach muss die Leitung zu
    /// sein. Err, wenn sie vorher endet oder danach noch etwas kommt.
    fn bis_abloesung(s: &mut secure::Secure) -> Result<(), String> {
        loop {
            let mut h = [0u8; 8];
            s.read_exact(&mut h)?;
            let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
            let mut p = vec![0u8; len];
            s.read_exact(&mut p)?;
            if h[0] == MSG_ABGELOEST {
                if len != 0 {
                    return Err(format!("Schlusswort mit {len} Byte Nutzlast"));
                }
                return match s.read_exact(&mut [0u8; 1]) {
                    Err(_) => Ok(()),
                    Ok(()) => Err("nach dem Schlusswort kam noch etwas".into()),
                };
            }
        }
    }

    /// Ein Paar gesicherter Leitungen ueber Loopback: (Host, Client).
    fn paar() -> (secure::Secure, secure::Secure) {
        secure::test_identitaet();
        let (host_priv, _) = noise::keypair().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let t = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            secure::Secure::accept(s, &noise::prologue_video(), &host_priv).unwrap()
        });
        let c = secure::Secure::connect(&addr, &noise::prologue_video()).unwrap();
        (t.join().unwrap(), c)
    }

    fn leitung_zu(h: &secure::Secure) -> Arc<Leitung> {
        Arc::new(Leitung::neu(h.abbruchgriff(), h.peer.clone(), h.handshake_hash.clone(), "127.0.0.1".into()))
    }

    /// Drei Zeitantworten 0x11, 0x22, 0x33 fuer die Abloesungs-Tests.
    fn drei_zeitantworten(leitung: &Leitung) {
        for t in [0x11u64, 0x22, 0x33] {
            let mut p = kopf(MSG_TIME, 0, 0, 16).to_vec();
            p.extend_from_slice(&t.to_le_bytes());
            p.extend_from_slice(&[0u8; 8]);
            assert!(leitung.einreihen(p, Art::Klein, None).is_ok());
        }
    }

    /// Liest einen Kopf: (Typ, Laenge).
    fn kopf_lesen(c: &mut secure::Secure) -> (u8, usize) {
        let mut hdr = [0u8; 8];
        c.read_exact(&mut hdr).unwrap();
        (hdr[0], u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize)
    }

    /// Beim Abloesen wird verworfen, was noch wartet: hat der Sendefaden
    /// schon geschrieben, kommt beim alten Zuschauer nur MSG_ABGELOEST an,
    /// danach ist die Leitung zu, und einreihen nimmt nichts mehr an.
    #[test]
    fn abloesung_verwirft_warteschlange() {
        let (h, mut c) = paar();
        let leitung = leitung_zu(&h);
        // Als haette der Sendefaden die Begruessung schon abgeliefert.
        sperre(&leitung.q).geschrieben = 20;
        drei_zeitantworten(&leitung);
        leitung.abloesen();
        assert_eq!(leitung.einreihen(kopf(MSG_TIME, 0, 0, 0).to_vec(), Art::Klein, None), Err(Abgewiesen::Zu));
        let l2 = leitung.clone();
        std::thread::spawn(move || sendefaden(l2, h));
        assert!(leitung.abloesung_abschliessen(Duration::from_secs(2)));
        c.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(kopf_lesen(&mut c), (MSG_ABGELOEST, 0));
        assert!(c.read_exact(&mut [0u8; 1]).is_err());
    }

    /// Wird ein Zuschauer abgeloest, bevor sein Sendefaden das erste Paket
    /// genommen hat (zwei Clients verbinden fast gleichzeitig), geht die
    /// Begruessung noch hinaus - und erst dann MSG_ABGELOEST. Sonst laese der
    /// Client [10,0,0,0] als MAGIC, meldete ein fremdes Protokoll und
    /// verbaende sich neu. Alles danach bleibt verworfen.
    #[test]
    fn abloesung_vor_dem_ersten_write_behaelt_begruessung() {
        let (h, mut c) = paar();
        let leitung = leitung_zu(&h);
        let mut hello = MAGIC.to_vec();
        hello.extend_from_slice(&kopf(MSG_INFO, 0, 0, 8));
        hello.extend_from_slice(&[7u8; 8]);
        assert!(leitung.einreihen(hello, Art::Klein, None).is_ok());
        drei_zeitantworten(&leitung);
        leitung.abloesen();
        {
            let q = sperre(&leitung.q);
            assert_eq!((q.bytes, q.ton, q.klein, q.pakete.len()), (0, 0, 0, 0), "Buchfuehrung nach dem Abloesen");
        }
        let l2 = leitung.clone();
        std::thread::spawn(move || sendefaden(l2, h));
        assert!(leitung.abloesung_abschliessen(Duration::from_secs(2)));
        c.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut magic = [0u8; 4];
        c.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, MAGIC);
        assert_eq!(kopf_lesen(&mut c), (MSG_INFO, 8));
        let mut info = [0u8; 8];
        c.read_exact(&mut info).unwrap();
        assert_eq!(info, [7u8; 8]);
        // Direkt danach das Schlusswort - keine der drei Zeitantworten.
        assert_eq!(kopf_lesen(&mut c), (MSG_ABGELOEST, 0));
        assert!(c.read_exact(&mut [0u8; 1]).is_err());
    }

    /// Ein abgeloester Zuschauer, der langsam, aber stetig abnimmt, bekommt
    /// den Rest des laufenden Pakets und danach MSG_ABGELOEST - auch wenn das
    /// laenger dauert als die Frist ohne Fortschritt. Gekappt wurde er bis
    /// dahin nach 2 s: der Client sah einen Abbruch statt der Abloesung,
    /// verband sich neu und verdraengte den Neuen.
    #[test]
    fn langsamer_abgeloester_bekommt_rest_und_schlusswort() {
        let (h, mut c) = paar();
        sendepuffer_setzen(h.socket());
        let leitung = leitung_zu(&h);
        let l2 = leitung.clone();
        let sender = std::thread::spawn(move || sendefaden(l2, h));
        // Ein Paket von 4 MB (Vollbild oder grosse Zwischenablage).
        let n = 4 << 20;
        let mut p = kopf(MSG_CLIP, 0, 0, n).to_vec();
        p.resize(8 + n, b'x');
        assert!(leitung.einreihen(p, Art::Klein, None).is_ok());
        // Der Client liest 64 kB alle 60 ms (rund 1 MB/s).
        let leser = std::thread::spawn(move || -> Result<(), String> {
            c.socket().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let (typ, len) = kopf_lesen(&mut c);
            if (typ, len) != (MSG_CLIP, n) {
                return Err(format!("erst {typ}/{len} statt der Zwischenablage"));
            }
            let mut puffer = vec![0u8; 64 * 1024];
            let mut rest = len;
            while rest > 0 {
                let k = rest.min(puffer.len());
                c.read_exact(&mut puffer[..k])?;
                rest -= k;
                std::thread::sleep(Duration::from_millis(60));
            }
            bis_abloesung(&mut c)
        });
        std::thread::sleep(Duration::from_millis(400));
        let t0 = Instant::now();
        leitung.abloesen();
        let durch = leitung.abloesung_abschliessen(FRIST_SCHLUSSWORT * 2);
        let dauer = t0.elapsed();
        println!("Abloesung eines langsamen Zuschauers nach {dauer:?} abgeschlossen: {durch}");
        assert!(durch, "Sendefaden nicht durch - Bildleitung gekappt");
        // Sonst haette schon die alte feste Frist gereicht: Probe ohne Wert.
        assert!(dauer > FRIST_SCHLUSSWORT * 2, "{dauer:?}");
        assert!(dauer < ABLOESUNG_HOECHSTENS, "{dauer:?}");
        assert_eq!(endet_binnen(leser, Duration::from_secs(10)), Some(Ok(())));
        assert!(endet_binnen(sender, Duration::from_secs(5)).is_some());
    }

    /// Ein abgeloester Zuschauer, der nichts mehr abnimmt (eingefroren): der
    /// Sendefaden steckt in einem write. Er darf danach nicht haengen
    /// bleiben - die Frist kappt die Leitung, der Faden endet.
    #[test]
    fn abgeloester_zuschauer_der_nichts_abnimmt() {
        let (h, c) = paar();
        sendepuffer_setzen(h.socket());
        let leitung = leitung_zu(&h);
        let l2 = leitung.clone();
        let faden = std::thread::spawn(move || sendefaden(l2, h));
        // Weit mehr, als beide Kernelpuffer fassen: 16 MB, der Client liest nie.
        for _ in 0..16 {
            assert!(leitung.einreihen(vec![0u8; 1 << 20], Art::Bild, None).is_ok());
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(sperre(&leitung.q).bytes > 0, "Sendefaden haengt nicht - Probe ohne Wert");
        let t0 = Instant::now();
        leitung.abloesen();
        assert!(!leitung.abloesung_abschliessen(Duration::from_millis(300)));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = faden.join();
            let _ = tx.send(());
        });
        let fertig = rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let dauer = t0.elapsed();
        drop(c);
        assert!(fertig, "Sendefaden haengt nach dem Kappen weiter");
        assert!(dauer < Duration::from_secs(3), "{dauer:?}");
        println!("Sendefaden {dauer:?} nach dem Abloesen beendet");
    }

    /// `kappen` bricht auch ein recv ab, auf das die Gegenstelle nie
    /// antwortet (Eingabekanal eines eingefrorenen Zuschauers).
    #[test]
    fn kappen_bricht_haengendes_lesen_ab() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let _c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (h, _) = l.accept().unwrap();
        let griff = h.try_clone().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = (&h).read(&mut [0u8; 1]);
            let _ = tx.send(r.is_err() || matches!(r, Ok(0)));
        });
        std::thread::sleep(Duration::from_millis(200));
        let t0 = Instant::now();
        kappen(&griff);
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok(true), "Lesen haengt nach dem Kappen weiter");
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    }

    fn zeitfrage(s: &mut secure::Secure, t: u64) -> Result<(), String> {
        let mut m = kopf(IN_TIME, 0, 0, 8).to_vec();
        m.extend_from_slice(&t.to_le_bytes());
        s.write_all(&m)
    }

    /// IN_FAEHIGKEITEN mit FAEHIG_DATEIEN auf einen Eingabekanal.
    fn faehigkeiten_melden(s: &mut secure::Secure) -> Result<(), String> {
        let mut m = kopf(IN_FAEHIGKEITEN, 0, 0, 4).to_vec();
        m.extend_from_slice(&dateien::faehigkeiten_kodieren(FAEHIG_DATEIEN));
        s.write_all(&m)
    }

    /// Die naechste Nachricht vom Bildkanal: (Typ, Nutzlast).
    fn naechste(s: &mut secure::Secure) -> Result<(u8, Vec<u8>), String> {
        let mut h = [0u8; 8];
        s.read_exact(&mut h)?;
        let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        let mut p = vec![0u8; len];
        s.read_exact(&mut p)?;
        Ok((h[0], p))
    }

    /// Die Bildschirmliste, die der Host in der Begruessung schickt: die
    /// zwei Eintraege der Pruefvektoren (bildschirm.rs), Automatik.
    fn begruessungsliste() -> crate::bildschirm::Bildschirme {
        use crate::bildschirm::{BildschirmEintrag, Bildschirme};
        Bildschirme {
            wunsch: None,
            eintraege: vec![
                BildschirmEintrag { kennung: "v1138-m1234-s0".into(), name: "X27 X1".into(), breite: 1920, hoehe: 1080, hz: 120, haupt: true, gestreamt: true },
                BildschirmEintrag { kennung: "v0-m0-s0".into(), name: "Virtuell 16:9".into(), breite: 1920, hoehe: 1080, hz: 240, haupt: false, gestreamt: false },
            ],
        }
    }

    /// Liest vom Bildkanal nach MAGIC bis MSG_FAEHIGKEITEN und gibt dessen
    /// Nutzlast zurueck. Die erste Nachricht muss die Begruessung (MSG_INFO)
    /// sein.
    fn bis_faehigkeiten(s: &mut secure::Secure) -> Result<Vec<u8>, String> {
        let mut erste = true;
        loop {
            let mut h = [0u8; 8];
            s.read_exact(&mut h)?;
            let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
            let mut p = vec![0u8; len];
            s.read_exact(&mut p)?;
            if erste && h[0] != MSG_INFO {
                return Err(format!("erst Nachricht {} statt der Begruessung", h[0]));
            }
            erste = false;
            if h[0] == MSG_FAEHIGKEITEN {
                return Ok(p);
            }
        }
    }

    fn bild_verbinden(addr: &str) -> secure::Secure {
        let t0 = Instant::now();
        let mut s = secure::Secure::connect(addr, &noise::prologue_video()).unwrap();
        let mut magic = [0u8; 4];
        s.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, MAGIC);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        s
    }

    /// Der ganze Zuschauerplatz auf Loopback: stumme Verbindungen halten die
    /// Annahme nicht auf, ein neuer Zuschauer kappt den Eingabekanal des
    /// alten und bekommt selbst einen funktionierenden, und je Absender
    /// laufen hoechstens zwei Handschlaege. Der Testschluessel steht in der
    /// Geraeteliste des Einlasses (kein Erstkontakt mehr); ein unbekanntes
    /// Geraet in der Zugangsphase stoert den laufenden Zuschauer nicht.
    #[test]
    fn zuschauerwechsel_und_parallele_annahme() {
        let _platz = platz_pruefung();
        // Unten wird der Name aus Nachricht 3 mit geraetename() verglichen.
        let _name = zugang::name_test_sperre();
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let marke = Laufmarke::neu(host_priv);
        let (_, client_pub) = secure::test_identitaet();
        let geraete = zugang::ablage_pfad(zugang::GERAETE_DATEI).unwrap();
        let passwort = zugang::ablage_pfad(zugang::PASSWORT_DATEI).unwrap();
        let k: [u8; 32] = client_pub.clone().try_into().unwrap();
        zugang::geraet_eintragen(&geraete, &k, "Testclient", "2026-09-26").unwrap();
        super::super::einlass::einrichten(Arc::new(super::super::einlass::Einlass::neu(geraete, passwort, &host_pub, "Testhost")))
            .unwrap();
        // Ist irgendwann ein Faden unter einer der Sperren in Panik geraten,
        // muss der Zuschauerplatz trotzdem weiterlaufen. Hier mit Absicht:
        // beide vergiftet, bevor es losgeht.
        let _ = std::thread::spawn(|| {
            let _g = EINSPEISEN.lock();
            panic!("mit Absicht: Sperre vergiften (Test)");
        })
        .join();
        let _ = std::thread::spawn(|| {
            let _g = AKTUELL.lock();
            panic!("mit Absicht: Sperre vergiften (Test)");
        })
        .join();
        assert!(EINSPEISEN.is_poisoned() && AKTUELL.is_poisoned());
        let bild_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ein_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bild_addr = bild_l.local_addr().unwrap().to_string();
        let ein_addr = ein_l.local_addr().unwrap().to_string();
        let m2 = marke.clone();
        std::thread::spawn(move || annahme_bild(bild_l, m2));
        std::thread::spawn(move || annahme_eingabe(ein_l, marke));

        // Die Bildschirme, die der Aufnahmefaden in Z hinterlegt haette.
        *sperre(&Z.bildschirme) = begruessungsliste();

        // Zuschauer A, waehrend eine stumme Verbindung am Bildport haengt.
        let stumm = TcpStream::connect(&bild_addr).unwrap();
        let mut a = bild_verbinden(&bild_addr);
        drop(stumm);
        // Nach der Begruessung kommt MSG_FAEHIGKEITEN: Dateien Fassung 1 und
        // Bildschirmwahl (3), gleich danach die Bildschirme (12).
        a.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(bis_faehigkeiten(&mut a), Ok(vec![3, 0, 0, 0]));
        let (typ, p) = naechste(&mut a).unwrap();
        assert_eq!(typ, MSG_BILDSCHIRME, "nach 11 kommt 12");
        assert_eq!(crate::bildschirm::bildschirme_lesen(&p), Some(begruessungsliste()));
        // Eingabekanal von A, ebenso an einer stummen Verbindung vorbei.
        let stumm = TcpStream::connect(&ein_addr).unwrap();
        let t0 = Instant::now();
        let mut a_ein = secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        drop(stumm);
        // A meldet Dateien; die Zeitantwort danach heisst: gelesen.
        assert!(!aktuell().unwrap().kann_dateien());
        faehigkeiten_melden(&mut a_ein).unwrap();
        zeitfrage(&mut a_ein, 0xA1).unwrap();
        assert_eq!(zeitantwort(&mut a), Ok(0xA1));
        assert!(aktuell().unwrap().kann_dateien(), "IN_FAEHIGKEITEN von A nicht gemerkt");
        // A hat das Testbild eingeschaltet (Nachricht 68).
        zeitfrage(&mut a_ein, 0xA3).unwrap();
        let mut m = kopf(IN_TESTBILD, 0, 0, 1).to_vec();
        m.push(1);
        a_ein.write_all(&m).unwrap();
        zeitfrage(&mut a_ein, 0xA4).unwrap();
        assert_eq!(zeitantwort(&mut a), Ok(0xA3));
        assert_eq!(zeitantwort(&mut a), Ok(0xA4));
        assert!(Z.testbild.load(Ordering::Relaxed), "Testbild von A nicht angekommen");
        let nr_a = zuschauer_nr();

        // Zuschauer B loest A ab: A verliert Bild UND Eingabe - und bekommt
        // als letzte Nachricht MSG_ABGELOEST, damit er nicht zurueckkommt.
        let mut b = bild_verbinden(&bild_addr);
        // Ohne Luecke: ein Zuschauer ist durchgehend da, aber die Nummer ist
        // eine neue - daran erkennen Aufnahme und Ton den Wechsel. Das
        // Testbild von A gilt fuer B nicht, die Faehigkeiten von A auch nicht.
        assert!(zuschauer_da());
        assert_eq!(zuschauer_nr(), nr_a + 1);
        assert!(!Z.testbild.load(Ordering::Relaxed), "Testbild von A ueberlebt die Abloesung");
        assert!(!aktuell().unwrap().kann_dateien(), "Faehigkeiten von A gelten fuer B");
        // Auch B bekommt MSG_FAEHIGKEITEN und die Bildschirme.
        b.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(bis_faehigkeiten(&mut b), Ok(vec![3, 0, 0, 0]));
        let (typ, p) = naechste(&mut b).unwrap();
        assert_eq!(typ, MSG_BILDSCHIRME);
        assert_eq!(crate::bildschirm::bildschirme_lesen(&p), Some(begruessungsliste()));
        a_ein.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        a.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t0 = Instant::now();
        assert!(a_ein.read_exact(&mut [0u8; 1]).is_err());
        assert_eq!(bis_abloesung(&mut a), Ok(()));
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        let _ = zeitfrage(&mut a_ein, 0xA2);
        // B bekommt einen eigenen, funktionierenden Eingabekanal; die
        // Frage von A kommt nie an.
        let mut b_ein = secure::Secure::connect(&ein_addr, &noise::prologue_input(&b.handshake_hash)).unwrap();
        faehigkeiten_melden(&mut b_ein).unwrap();
        zeitfrage(&mut b_ein, 0xB1).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB1));
        assert!(aktuell().unwrap().kann_dateien(), "IN_FAEHIGKEITEN von B nicht gemerkt");
        // Die alte Pruefsumme passt nicht mehr: A kommt nicht wieder herein.
        assert!(secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).is_err());

        // Ein unbekanntes Geraet: QCA1 und Nachricht 20 statt MAGIC; solange
        // es in der Zugangsphase steht und wenn es abbricht, bleibt B der
        // Zuschauer (gleiche Nummer), und B's Leitung laeuft weiter.
        let nr_b = zuschauer_nr();
        let (fremd_priv, _) = noise::keypair().unwrap();
        let mut fremd = super::super::einlass::stub::Stub::verbinden(&bild_addr, &fremd_priv, &zugang::nachricht3("Fremd", 0)).unwrap();
        assert_eq!(&fremd.kennung().unwrap(), MAGIC_ZUGANG);
        assert!(matches!(fremd.nachricht(), Ok(zugang::Nachricht::Noetig(_))));
        assert_eq!(zuschauer_nr(), nr_b, "Zugangsphase hat den Zuschauer abgeloest");
        // Der Name aus Nachricht 3 - oder die Adresse, solange der Client
        // keinen sendet.
        let n = zuschauer_name().unwrap();
        assert!(n == "127.0.0.1" || n == zugang::geraetename(), "{n}");
        zeitfrage(&mut b_ein, 0xB2).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB2));
        fremd.senden(&zugang::Nachricht::Abbruch).unwrap();
        assert!(fremd.zu());
        zeitfrage(&mut b_ein, 0xB3).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB3));
        assert_eq!(zuschauer_nr(), nr_b);
        // Ein fremdes Geraet verabschiedet zuschauer_verabschieden nicht.
        assert!(!zuschauer_verabschieden(Some(&[7u8; 32]), HOST_ENDE_ENTFERNT));
        assert_eq!(zuschauer_nr(), nr_b);

        // Entfernen, waehrend ein bekanntes Geraet hereinkommt: es stand beim
        // Blick in die Liste noch darin, ist aber noch kein Zuschauer - das
        // Menue (Entfernen, dann zuschauer_verabschieden) findet es nicht. Es
        // bekommt trotzdem kein MAGIC, und B bleibt der Zuschauer. Wird
        // stattdessen ein anderes Geraet entfernt, kommt es herein.
        let einlass = super::super::einlass::einlass().unwrap();
        let liste = zugang::ablage_pfad(zugang::GERAETE_DATEI).unwrap();
        let (c_priv, c_pub) = noise::keypair().unwrap();
        let (x_priv, x_pub) = noise::keypair().unwrap();
        let (_, y_pub) = noise::keypair().unwrap();
        for (k, n) in [(&c_pub, "Wettlauf"), (&x_pub, "Zweiter"), (&y_pub, "Unbeteiligt")] {
            let k: [u8; 32] = k.clone().try_into().unwrap();
            zugang::geraet_eintragen(&liste, &k, n, "2026-09-26").unwrap();
        }
        let angehalten = |wer: Vec<u8>, name: &'static str, entfernen: Vec<u8>| {
            let (da_tx, da_rx) = std::sync::mpsc::channel::<()>();
            let (weiter_tx, weiter_rx) = std::sync::mpsc::channel::<()>();
            let weiter_rx = Mutex::new(weiter_rx);
            *sperre(&NACH_EINLASS) = Some(Box::new(move || {
                let _ = da_tx.send(());
                let _ = sperre(&weiter_rx).recv_timeout(Duration::from_secs(5));
            }));
            let addr = bild_addr.clone();
            let t = std::thread::spawn(move || {
                let mut s = super::super::einlass::stub::Stub::verbinden(&addr, &wer, &zugang::nachricht3(name, 0))?;
                s.kennung()
            });
            da_rx.recv_timeout(Duration::from_secs(5)).expect("Einlass nicht erreicht");
            // Wie das Menue: erst aus der Liste, dann den Zuschauer verabschieden.
            assert_eq!(einlass.geraet_entfernen(&entfernen).unwrap(), true);
            assert!(!zuschauer_verabschieden(Some(&entfernen), HOST_ENDE_ENTFERNT), "noch kein Zuschauer");
            weiter_tx.send(()).unwrap();
            t.join().unwrap()
        };
        assert!(angehalten(c_priv, "Wettlauf", c_pub.clone()).is_err(), "trotz Entfernen hereingekommen");
        assert_eq!(zuschauer_nr(), nr_b, "B abgeloest");
        zeitfrage(&mut b_ein, 0xB4).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB4));
        assert_eq!(angehalten(x_priv, "Zweiter", y_pub.clone()), Ok(*MAGIC), "ein anderes Geraet entfernt: herein");
        assert_eq!(zuschauer_nr(), nr_b + 1);
        assert!(!einlass.noch_bekannt(&c_pub, "127.0.0.1".parse().unwrap()));

        // Zwei laufende Handschlaege von einem Absender: der dritte wird
        // sofort abgewiesen, nicht erst nach der Frist.
        let s1 = TcpStream::connect(&bild_addr).unwrap();
        let s2 = TcpStream::connect(&bild_addr).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        assert!(secure::Secure::connect(&bild_addr, &noise::prologue_video()).is_err());
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        // Geben sie auf, ist der Platz wieder frei.
        drop((s1, s2));
        std::thread::sleep(Duration::from_millis(200));
        let mut c = bild_verbinden(&bild_addr);

        // "Freigabe beenden": der Zuschauer bekommt den Abschied mit Grund 1
        // als letzte Nachricht (nach der Begruessung, die schon unterwegs
        // war), danach ist die Leitung zu, und niemand schaut mehr zu.
        assert!(warten_bis_da(|| zuschauer_nr() > nr_b + 1));
        abschied_beim_beenden(HOST_ENDE_FREIGABE_AUS, Duration::from_secs(3));
        c.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut typen = Vec::new();
        let abschied = loop {
            match naechste(&mut c) {
                Ok((MSG_HOST_ENDE, p)) => break p,
                Ok((t, _)) => typen.push(t),
                Err(e) => panic!("kein Abschied, Leitung vorher zu ({e}), vorher {typen:?}"),
            }
        };
        assert_eq!(abschied, vec![HOST_ENDE_FREIGABE_AUS]);
        assert!(c.read_exact(&mut [0u8; 1]).is_err(), "nach dem Abschied kam noch etwas");
        assert!(warten_bis_da(|| !zuschauer_da()), "Zuschauer nach dem Abschied noch eingetragen");
    }

    /// Bis die Bedingung gilt, hoechstens 5 s.
    fn warten_bis_da(f: impl Fn() -> bool) -> bool {
        let t0 = Instant::now();
        while !f() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        f()
    }

    /// Der Abschied des Hosts (MSG_HOST_ENDE) verwirft wie die Abloesung, was
    /// noch wartet, und kommt als letzte Nachricht: Kopf 13 mit einem Byte
    /// Grund - Byte fuer Byte wie der Mac-Host (zugangtest.c) -, danach ist
    /// die Leitung zu und einreihen nimmt nichts mehr an.
    #[test]
    fn abschied_als_letzte_nachricht() {
        assert_eq!(host_ende(HOST_ENDE_BEENDET), [13, 0, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(host_ende(HOST_ENDE_ENTFERNT), [13, 0, 0, 0, 1, 0, 0, 0, 2]);
        let (h, mut c) = paar();
        let leitung = leitung_zu(&h);
        // Als haette der Sendefaden die Begruessung schon abgeliefert.
        sperre(&leitung.q).geschrieben = 20;
        drei_zeitantworten(&leitung);
        leitung.schliessen_mit(Schluss::Ende(HOST_ENDE_ENTFERNT));
        assert_eq!(leitung.einreihen(kopf(MSG_TIME, 0, 0, 0).to_vec(), Art::Klein, None), Err(Abgewiesen::Zu));
        let l2 = leitung.clone();
        std::thread::spawn(move || sendefaden(l2, h));
        assert!(leitung.schluss_abwarten(Duration::from_secs(2), Duration::from_secs(3), "Verabschiedeter"));
        c.socket().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(kopf_lesen(&mut c), (MSG_HOST_ENDE, 1));
        let mut grund = [0u8; 1];
        c.read_exact(&mut grund).unwrap();
        assert_eq!(grund, [HOST_ENDE_ENTFERNT]);
        assert!(c.read_exact(&mut [0u8; 1]).is_err());
    }

    /// Bekanntgabe (Spezifikation 2): Rechnername, dahinter ID und Flags;
    /// Bit 0 nur, wenn "Zulassen" moeglich ist. Ein alter Leser (bis zum
    /// Namen) sieht dasselbe wie frueher.
    #[test]
    fn bekanntgabe_mit_id_und_flags() {
        let p = bekanntgabe_paket(9001, "BUERO-PC", 581_729_911, true);
        assert!(p.len() <= zugang::BEKANNTGABE_MAX);
        assert_eq!(&p[..4], BEACON_MAGIC);
        assert_eq!((p[4], u16::from_le_bytes([p[5], p[6]]), p[7]), (BEACON_VERSION, 9001, 8));
        assert_eq!(&p[8..16], b"BUERO-PC");
        let b = zugang::bekanntgabe_lesen(&p).unwrap();
        assert_eq!((b.id, b.flags, b.zulassen_moeglich()), (Some(581_729_911), BEACON_FLAG_ZULASSEN, true));
        let b = zugang::bekanntgabe_lesen(&bekanntgabe_paket(9101, "PC", 5, false)).unwrap();
        assert_eq!((b.port, b.name.as_str(), b.id, b.flags), (9101, "PC", Some(5), 0));
        // Ein langer Name wird auf 40 Byte gekuerzt, nie mitten im Zeichen.
        let p = bekanntgabe_paket(9001, &"ä".repeat(30), 1, false);
        assert_eq!(p[7], 40);
        assert!(p.len() <= zugang::BEKANNTGABE_MAX);
    }

    /// Die Grenzen der laufenden Handschlaege: je Absender zwei, insgesamt
    /// so viele, dass zwei oder drei Adressen niemanden aussperren. Erst
    /// HANDSCHLAEGE_MAX / 2 Absender fuellen die Tabelle.
    #[test]
    fn handschlagplaetze_brauchen_viele_absender() {
        static P: Plaetze = Plaetze::neu("Test: Verbindung abgewiesen");
        let ip = |n: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, n));
        let mut belegt = Vec::new();
        // Zwei Angreiferadressen mit je zwei stummen Verbindungen.
        for n in 1..=2 {
            for _ in 0..2 {
                belegt.push(P.belegen(ip(n), "Test").unwrap());
            }
            assert!(P.belegen(ip(n), "Test").is_none(), "je Absender hoechstens zwei");
        }
        // Ein Zuschauer von einer dritten Adresse kommt trotzdem an die Reihe.
        assert!(P.belegen(ip(100), "Test").is_some());
        // Voll ist die Tabelle erst mit HANDSCHLAEGE_MAX / 2 Absendern.
        let absender = (HANDSCHLAEGE_MAX / HANDSCHLAEGE_JE_ABSENDER) as u8;
        assert!(absender >= 16, "{absender}");
        for n in 3..=absender {
            for _ in 0..2 {
                belegt.push(P.belegen(ip(n), "Test").unwrap());
            }
        }
        assert_eq!(belegt.len(), HANDSCHLAEGE_MAX);
        assert!(P.belegen(ip(100), "Test").is_none());
        // Ein Handschlag endet (Frist, Fehler oder fertig): Platz wieder frei.
        belegt.pop();
        assert!(P.belegen(ip(100), "Test").is_some());
    }

    /// Protokoll-Flut (Integrationstest: 300 Muell-Verbindungen in 0,1 s
    /// ergaben 300 Zeilen): je Art und Adresse hoechstens eine Zeile alle
    /// zehn Sekunden, die naechste nennt die Zahl der unterdrueckten dieser
    /// Adresse. Der Text einer unterdrueckten Zeile wird gar nicht erst
    /// gebaut. Die erste Zeile eines anderen Geraets kommt mitten in der
    /// Flut sofort (Integrationstest des Endstands: die Sammelzeile nannte
    /// 127.0.0.1 und zaehlte die Versuche von 127.0.0.2 bis .5 mit).
    #[test]
    fn drossel_je_art_und_adresse() {
        let d = Drossel::neu("Bildkanal: Handschlag gescheitert");
        let andere = Drossel::neu("Abgewiesen: unbekannte Gegenstelle");
        let ip = |n: u8| Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, n)));
        let t0 = Instant::now();
        let bei = |ms: u64| t0 + Duration::from_millis(ms);
        let zeile = |n: u8| format!("Handschlag mit 127.0.0.{n} gescheitert (-1)");
        assert_eq!(d.melden_zu(ip(1), bei(0), || zeile(1)), Some(zeile(1)));
        let mut gebaut = false;
        let r = d.melden_zu(ip(1), bei(1), || {
            gebaut = true;
            String::new()
        });
        assert_eq!(r, None);
        assert!(!gebaut, "Text einer unterdrueckten Zeile gebaut");
        for i in 0..298 {
            assert_eq!(d.melden_zu(ip(1), bei(2 + i / 3), || zeile(1)), None);
        }
        // Mitten in der Flut: je eine Zeile fuer 127.0.0.2 bis .5, eine
        // Wiederholung von dort nicht.
        for n in 2..=5 {
            assert_eq!(d.melden_zu(ip(n), bei(50), || zeile(n)), Some(zeile(n)), "erste Zeile von .{n} in der Flut untergegangen");
            assert_eq!(d.melden_zu(ip(n), bei(60), || zeile(n)), None);
        }
        // Eine andere Art hat ihre eigene Drossel.
        assert!(andere.melden_zu(ip(1), bei(70), || "Abgewiesen: unbekannte Gegenstelle".into()).is_some());
        // Vor Ablauf der Frist nichts, danach eine Zeile mit der Zahl DIESER Adresse.
        assert_eq!(d.melden_zu(ip(1), bei(9_999), || zeile(1)), None);
        assert_eq!(
            d.melden_zu(ip(1), bei(10_001), || zeile(1)).as_deref(),
            Some("Handschlag mit 127.0.0.1 gescheitert (-1) - dazu 300 weitere von dieser Adresse seit der letzten Meldung")
        );
        assert_eq!(
            d.melden_zu(ip(2), bei(10_060), || zeile(2)).as_deref(),
            Some("Handschlag mit 127.0.0.2 gescheitert (-1) - dazu 1 weitere von dieser Adresse seit der letzten Meldung")
        );
        // Zeilen ohne Gegenstelle (Ton, harte Grenze) zaehlen fuer sich.
        let ton = Drossel::neu("Leitung langsamer als der Ton: Tonpaket verworfen");
        assert!(ton.melden_zu(None, bei(0), || "Ton".into()).is_some());
        assert!(ton.melden_zu(None, bei(1), || "Ton".into()).is_none());
        assert_eq!(ton.melden_zu(None, bei(10_000), || "Ton".into()).as_deref(), Some("Ton - dazu 1 weitere seit der letzten Meldung"));
    }

    /// Das Ende einer Flut bleibt nicht ungemeldet, bis wieder eine Zeile
    /// derselben Art kommt: der 5-s-Takt traegt die Zahl nach, sobald die
    /// Frist der Adresse um ist - je Adresse mit ihrer Adresse.
    #[test]
    fn drossel_traegt_das_ende_einer_flut_im_takt_nach() {
        let d = Drossel::neu("Bildkanal: Handschlag gescheitert");
        let ip = |n: u8| Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, n)));
        let t0 = Instant::now();
        let bei = |ms: u64| t0 + Duration::from_millis(ms);
        for i in 0..51 {
            d.melden_zu(ip(1), bei(i), || "a".into());
        }
        for i in 0..4 {
            d.melden_zu(ip(2), bei(100 + i), || "b".into());
        }
        // Takt bei 5 s: die Frist laeuft noch.
        assert!(d.nachtragen_zu(bei(5_000)).is_empty());
        // Takt bei 10,1 s: je Adresse eine Sammelzeile.
        assert_eq!(
            d.nachtragen_zu(bei(10_100)),
            vec![
                "Bildkanal: Handschlag gescheitert: 50 weitere von 127.0.0.1 seit der letzten Meldung".to_string(),
                "Bildkanal: Handschlag gescheitert: 3 weitere von 127.0.0.2 seit der letzten Meldung".to_string(),
            ]
        );
        // Nichts Neues: keine Zeile. Die Sammelzeile zaehlt als Meldung -
        // eine Zeile gleich danach wird wieder nur gezaehlt.
        assert!(d.nachtragen_zu(bei(15_100)).is_empty());
        assert_eq!(d.melden_zu(ip(1), bei(12_000), || "a".into()), None);
        assert!(d.nachtragen_zu(bei(20_000)).is_empty(), "Frist nach der Sammelzeile nicht eingehalten");
        assert_eq!(d.nachtragen_zu(bei(20_100)), vec!["Bildkanal: Handschlag gescheitert: 1 weitere von 127.0.0.1 seit der letzten Meldung".to_string()]);
        assert!(d.nachtragen_zu(bei(60_000)).is_empty());
    }

    /// Die Tabelle je Art ist begrenzt. Ist sie voll, zaehlen weitere
    /// Adressen gemeinsam und stehen mit der letzten davon in einer
    /// Sammelzeile; mehr als DROSSEL_ADRESSEN + 1 Zeilen je Art und Frist
    /// gibt es nie. Nach der Frist macht die aelteste Adresse Platz, die
    /// nichts mehr offen hat - eine mit Offenem nicht.
    #[test]
    fn drossel_tabelle_ist_begrenzt() {
        let d = Drossel::neu("Abgewiesen: unbekannte Gegenstelle");
        let ip = |n: u32| Some(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n)));
        let t0 = Instant::now();
        let bei = |ms: u64| t0 + Duration::from_millis(ms);
        let n = DROSSEL_ADRESSEN as u32;
        let mut zeilen = 0;
        // Eine Flut von 3 * n Adressen, jede zweimal, binnen einer Sekunde.
        for runde in 0..2u64 {
            for a in 1..=3 * n {
                if d.melden_zu(ip(a), bei(runde * 500 + a as u64), || "x".into()).is_some() {
                    zeilen += 1;
                }
            }
        }
        assert_eq!(zeilen, DROSSEL_ADRESSEN, "Zeilen in der ersten Frist");
        assert_eq!(sperre(&d.buch).gemerkt.len(), DROSSEL_ADRESSEN);
        let nach = d.nachtragen_zu(bei(10_500));
        assert_eq!(nach.len(), DROSSEL_ADRESSEN + 1);
        assert_eq!(nach[0], "Abgewiesen: unbekannte Gegenstelle: 1 weitere von 10.0.0.1 seit der letzten Meldung");
        assert_eq!(
            nach[DROSSEL_ADRESSEN],
            format!("Abgewiesen: unbekannte Gegenstelle: {} weitere von anderen Adressen seit der letzten Meldung, zuletzt von 10.0.0.{}", 4 * n, 3 * n)
        );
        // Direkt nach der Sammelzeile hat jede gemerkte Adresse ihre Frist
        // noch vor sich: eine neue zaehlt gemeinsam, ohne Zeile.
        assert_eq!(d.melden_zu(ip(1000), bei(11_000), || "neu".into()), None);
        // Zehn Sekunden spaeter macht die aelteste freie Platz (10.0.0.1,
        // gemeldet bei 10,5 s wie alle, aber als erste in der Tabelle), die
        // Tabelle waechst nicht.
        assert_eq!(d.melden_zu(ip(1001), bei(20_600), || "neu".into()).as_deref(), Some("neu"));
        {
            let b = sperre(&d.buch);
            assert_eq!(b.gemerkt.len(), DROSSEL_ADRESSEN);
            assert!(b.gemerkt.iter().all(|g| g.von != ip(1)), "aelteste Adresse nicht entfernt");
        }
        // Eine Adresse mit Offenem macht keinen Platz: 10.0.0.2 bekommt
        // noch etwas, alle anderen alten Plaetze werden frei und gehen
        // an neue Adressen - 10.0.0.2 bleibt.
        assert_eq!(d.melden_zu(ip(2), bei(20_700), || "x".into()).as_deref(), Some("x"));
        assert_eq!(d.melden_zu(ip(2), bei(20_701), || "x".into()), None);
        for a in 0..(n - 2) {
            assert!(d.melden_zu(ip(2000 + a), bei(21_000 + a as u64), || "neu".into()).is_some(), "Adresse {a}");
        }
        assert_eq!(d.melden_zu(ip(3000), bei(22_000), || "neu".into()), None, "Platz mit Offenem vergeben");
        assert!(sperre(&d.buch).gemerkt.iter().any(|g| g.von == ip(2) && g.weitere == 1));
    }

    /// Nach einer Panik im Aufnahmefaden geht Hoststatus 1 nur an den
    /// Zuschauer, fuer den die Sitzung lief (aufnahme::start merkt sich
    /// seine Nummer). Hat inzwischen ein anderer abgeloest, bekommt der
    /// nichts davon - mit dem frueheren hoststatus_senden(1) landete die
    /// Meldung bei ihm, und er saehe dauerhaft "kein Bildschirm".
    #[test]
    fn hoststatus_nach_panik_nur_an_den_gemerkten_zuschauer() {
        let platz: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        let zaehler = AtomicU64::new(0);
        // Ohne Zuschauer geht nichts hinaus.
        assert!(!send_small_an(&platz, &zaehler, 0, MSG_HOSTSTATUS, &[1, 0]));
        // Zuschauer A (Nummer 1), wie in bild_annehmen unter der Sperre.
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "A".into()));
        {
            let mut p = sperre(&platz);
            zaehler.fetch_add(1, Ordering::Relaxed);
            *p = Some(a.clone());
        }
        let nr_a = zaehler.load(Ordering::Relaxed);
        assert!(send_small_an(&platz, &zaehler, nr_a, MSG_HOSTSTATUS, &[1, 0]));
        assert_eq!(sperre(&a.q).pakete.len(), 1);
        // B loest A ab.
        let b = Arc::new(Leitung::neu(None, vec![2], vec![2], "B".into()));
        {
            let mut p = sperre(&platz);
            if let Some(alt) = p.take() {
                alt.abloesen();
            }
            zaehler.fetch_add(1, Ordering::Relaxed);
            *p = Some(b.clone());
        }
        assert!(!send_small_an(&platz, &zaehler, nr_a, MSG_HOSTSTATUS, &[1, 0]), "Meldung fuer A ging an B");
        assert!(sperre(&b.q).pakete.is_empty(), "B hat die Meldung fuer A bekommen");
        // An B selbst geht sie, mit Kopf und Lage.
        assert!(send_small_an(&platz, &zaehler, nr_a + 1, MSG_HOSTSTATUS, &[0, 0]));
        {
            let q = sperre(&b.q);
            assert_eq!(q.pakete.len(), 1);
            assert_eq!(q.pakete[0].1, [&kopf(MSG_HOSTSTATUS, 0, 0, 2)[..], &[0u8, 0][..]].concat());
        }
        // Die Nummer, die nie vergeben wird, trifft auch im Dienst niemanden.
        assert!(!hoststatus_senden_an(u64::MAX, 1));
    }

    /// Die Stauregel als reine Entscheidung: im Budget codieren, darueber
    /// auslassen; ohne Fortschritt ist der Zuschauer nach STAU_FRIST_US weg,
    /// jeder Fortschritt (auch bei wachsendem Rueckstand) startet die Frist
    /// neu, und unter dem Budget ist der Stau vorbei.
    #[test]
    fn stauregel_entscheidet_vor_dem_encoder() {
        assert_eq!(budget(false), 2 * 1024 * 1024);
        assert_eq!(budget(true), 512 * 1024);
        let g = budget(false);
        let mut s = None;
        assert_eq!(stau_urteil(&mut s, g, g, 0, 0), Stauurteil::Frei, "genau das Budget ist noch frei");
        assert_eq!(s, None);
        assert_eq!(stau_urteil(&mut s, g + 1, g, 100, 1_000), Stauurteil::Stau);
        assert_eq!(s, Some(Stau { seit_us: 1_000, geschrieben: 100 }));
        assert_eq!(stau_urteil(&mut s, g + 1, g, 100, 1_000 + STAU_FRIST_US - 1), Stauurteil::Stau);
        // Fortschritt: der Sendefaden hat etwas abgeliefert - die Frist
        // beginnt neu, obwohl der Rueckstand gewachsen ist (Bilder aus dem
        // Encoder kamen nach).
        let t = 1_000 + STAU_FRIST_US;
        assert_eq!(stau_urteil(&mut s, g + 900_000, g, 165_619, t), Stauurteil::Stau);
        assert_eq!(s, Some(Stau { seit_us: t, geschrieben: 165_619 }));
        assert_eq!(stau_urteil(&mut s, g + 900_000, g, 165_619, t + STAU_FRIST_US - 1), Stauurteil::Stau);
        // Die Frist ohne Fortschritt: weg, der Stand ist zurueckgesetzt.
        assert_eq!(stau_urteil(&mut s, g + 900_000, g, 165_619, t + STAU_FRIST_US), Stauurteil::Weg);
        assert_eq!(s, None);
        // Unter dem Budget ist der Stau vorbei; der naechste beginnt von vorn.
        assert_eq!(stau_urteil(&mut s, g + 1, g, 5, 10), Stauurteil::Stau);
        assert_eq!(stau_urteil(&mut s, g, g, 5, 10 + STAU_FRIST_US - 1), Stauurteil::Frei);
        assert_eq!(s, None);
        assert_eq!(stau_urteil(&mut s, g + 1, g, 5, 10 + STAU_FRIST_US + 1), Stauurteil::Stau);
        assert_eq!(stau_urteil(&mut s, g + 1, g, 5, 10 + 2 * STAU_FRIST_US), Stauurteil::Stau);
        assert_eq!(stau_urteil(&mut s, g + 1, g, 5, 10 + 2 * STAU_FRIST_US + 1), Stauurteil::Weg);
    }

    /// Ein codiertes Bild geht hinaus, solange der Rueckstand nicht schon
    /// ueber der harten Grenze liegt - auch eines, das allein groesser ist
    /// (ein Vollbild bei 500 Mbit/s und 10 fps hat bis 6 MB). Was der
    /// Sendefaden gerade schreibt, zaehlt mit. Ist der Zuschauer weg, meldet
    /// `stau` das einmal und schliesst ihn; danach staut er nichts mehr.
    #[test]
    fn codiertes_bild_geht_bis_zur_harten_grenze_hinaus() {
        let l = Leitung::neu(None, Vec::new(), Vec::new(), String::new());
        assert_eq!(l.einreihen(vec![0u8; HARTE_GRENZE + 1], Art::Bild, Some(HARTE_GRENZE)), Ok(()));
        assert_eq!(l.einreihen(vec![0u8; 1], Art::Bild, Some(HARTE_GRENZE)), Err(Abgewiesen::Voll));
        assert_eq!(l.einreihen(vec![0u8; 8], Art::Klein, Some(KLEIN_GRENZE)), Ok(()), "kleine Nachrichten gehen vorbei");
        {
            let mut q = sperre(&l.q);
            q.leeren();
            q.im_schreiben = HARTE_GRENZE;
        }
        assert_eq!(l.einreihen(vec![0u8; 1], Art::Bild, Some(HARTE_GRENZE)), Ok(()));
        assert_eq!(l.einreihen(vec![0u8; 1], Art::Bild, Some(HARTE_GRENZE)), Err(Abgewiesen::Voll));
        // Stau ohne Fortschritt ueber die Frist: weg und geschlossen.
        assert_eq!(l.stau(BACKLOG_LIMIT, 1_000), Stauurteil::Stau);
        assert!(l.offen());
        assert_eq!(l.stau(BACKLOG_LIMIT, 1_000 + STAU_FRIST_US), Stauurteil::Weg);
        assert!(!l.offen());
        assert_eq!(l.stau(0, 2 * STAU_FRIST_US), Stauurteil::Frei, "ein geschlossener Zuschauer staut nichts mehr");
        assert_eq!(l.einreihen(vec![0u8; 1], Art::Bild, Some(HARTE_GRENZE)), Err(Abgewiesen::Zu));
    }

    /// Eine Nachricht aus send_small, wie sie in die Warteschlange geht.
    fn nachricht(typ: u8, n: usize) -> Vec<u8> {
        let mut p = kopf(typ, 0, 0, n).to_vec();
        p.resize(8 + n, 0);
        p
    }

    /// 10 ms Ton: 480 Rahmen float32 Stereo.
    const TONPAKET: usize = 480 * 2 * 4;

    /// Ton hat seine eigene Grenze, gemessen am wartenden Ton: ein grosses
    /// Vollbild davor laesst ihn nicht ausfallen, und mehr als ton_grenze
    /// kommt nie hinein - Bilder gehen dann weiter durch die Stauregel.
    /// Nimmt der Zuschauer nichts mehr ab und laeuft nur noch Ton, ist er
    /// nach der Frist trotzdem weg.
    #[test]
    fn ton_hat_eigene_grenze_unter_dem_budget() {
        for gaming in [false, true] {
            let l = Leitung::neu(None, Vec::new(), Vec::new(), String::new());
            let (mut angenommen, mut verworfen) = (0usize, 0usize);
            for _ in 0..2000 {
                match l.klein_senden(MSG_AUDIO, nachricht(MSG_AUDIO, TONPAKET), gaming, 1_000) {
                    Ok(()) => angenommen += 1,
                    Err(Abgewiesen::Voll) => verworfen += 1,
                    Err(Abgewiesen::Zu) => panic!("Zuschauer ohne Frist ausgetragen"),
                }
            }
            {
                let q = sperre(&l.q);
                assert!(q.ton <= ton_grenze(gaming) + 8 + TONPAKET, "{} Byte Ton", q.ton);
                assert_eq!(q.ton, q.bytes);
            }
            assert!(angenommen * (8 + TONPAKET) > ton_grenze(gaming), "{angenommen}");
            assert!(verworfen > 0);
            // Der Ton allein haelt kein Bild auf.
            assert_eq!(l.stau(budget(gaming), 1_000), Stauurteil::Frei, "gaming {gaming}");
            // Eingefroren, Bild still, nur Ton: der naechste Ton beginnt die
            // Frist, nach ihr ist der Zuschauer weg.
            assert_eq!(l.klein_senden(MSG_AUDIO, nachricht(MSG_AUDIO, TONPAKET), gaming, 2_000), Err(Abgewiesen::Voll));
            assert!(l.offen());
            assert_eq!(l.klein_senden(MSG_AUDIO, nachricht(MSG_AUDIO, TONPAKET), gaming, 2_000 + STAU_FRIST_US), Err(Abgewiesen::Voll));
            assert!(!l.offen(), "gaming {gaming}: eingefrorener Zuschauer bleibt eingetragen");
        }
        // Ein wartendes Vollbild von 3 MB zaehlt fuer den Ton nicht mit.
        let l = Leitung::neu(None, Vec::new(), Vec::new(), String::new());
        assert_eq!(l.einreihen(vec![0u8; 3 << 20], Art::Bild, Some(HARTE_GRENZE)), Ok(()));
        assert_eq!(l.klein_senden(MSG_AUDIO, nachricht(MSG_AUDIO, TONPAKET), true, 1_000), Ok(()));
    }

    /// Steuernachrichten fallen nie weg; wer ueber KLEIN_GRENZE davon nicht
    /// abnimmt, ist weg. Ein grosses Bild im Rueckstand zaehlt dafuer nicht.
    #[test]
    fn steuernachrichten_bis_zur_grenze_dann_weg() {
        let l = Leitung::neu(None, Vec::new(), Vec::new(), String::new());
        assert_eq!(l.einreihen(vec![0u8; 6 << 20], Art::Bild, Some(HARTE_GRENZE)), Ok(()));
        assert_eq!(l.klein_senden(MSG_LAST, nachricht(MSG_LAST, 28), false, 1_000), Ok(()));
        let zwischenablage = KLEIN_GRENZE / 8;
        for i in 0..7 {
            assert_eq!(l.klein_senden(MSG_CLIP, nachricht(MSG_CLIP, zwischenablage), false, 1_000), Ok(()), "{i}");
        }
        assert!(l.offen());
        // Eine darf die Grenze um sich selbst ueberschreiten, dann ist Schluss.
        assert_eq!(l.klein_senden(MSG_CLIP, nachricht(MSG_CLIP, zwischenablage), false, 1_000), Ok(()));
        assert_eq!(l.klein_senden(MSG_TIME, nachricht(MSG_TIME, 16), false, 1_000), Err(Abgewiesen::Voll));
        assert!(!l.offen());
        assert_eq!(l.klein_senden(MSG_TIME, nachricht(MSG_TIME, 16), false, 1_000), Err(Abgewiesen::Zu));
    }

    /// Pruefstand fuer den Ton: eine Leitung mit 1 Mbit/s, weit unter der
    /// Tonrate (3 Mbit/s), Spielmodus. Ton kommt alle 10 ms, dazu Bilder mit
    /// 30 fps durch die Stauregel. Ohne Grenze fuer den Ton lag der
    /// Rueckstand nach rund 2 s dauerhaft ueber dem Budget - kein Bild kam
    /// mehr durch, und die Warteschlange wuchs ohne Ende. Jetzt: der Ton
    /// bleibt unter seiner Grenze, Bilder gehen auch in der zweiten Haelfte
    /// hinein, und der lebende Zuschauer wird nicht ausgetragen.
    #[test]
    fn ton_auf_zu_langsamer_leitung_sperrt_das_bild_nicht() {
        let (h, mut c) = paar();
        sendepuffer_setzen(h.socket());
        let leitung = leitung_zu(&h);
        let l2 = leitung.clone();
        let sender = std::thread::spawn(move || sendefaden(l2, h));
        let leser = std::thread::spawn(move || {
            c.socket().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let t0 = Instant::now();
            let mut gelesen = 0u64;
            let mut puffer = vec![0u8; SCHREIBSTUECK];
            let (mut bilder, mut ton) = (0u32, 0u32);
            loop {
                let mut hdr = [0u8; 8];
                if gedrosselt_lesen(&mut c, 8, 1, t0, &mut gelesen, &mut hdr).is_err() {
                    break;
                }
                let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                if gedrosselt_lesen(&mut c, len, 1, t0, &mut gelesen, &mut puffer).is_err() {
                    break;
                }
                match hdr[0] {
                    MSG_VIDEO => bilder += 1,
                    MSG_AUDIO => ton += 1,
                    _ => {}
                }
            }
            (bilder, ton)
        });
        let dauer = Duration::from_secs(6);
        let (mut codiert_zweite_haelfte, mut ausgelassen, mut ton_verworfen) = (0u32, 0u32, 0u32);
        let (mut hoechster_ton, mut hoechster_rueckstand) = (0usize, 0usize);
        let t0 = Instant::now();
        let mut i = 0u32;
        while t0.elapsed() < dauer {
            if let Some(w) = (t0 + Duration::from_millis(10) * i).checked_duration_since(Instant::now()) {
                std::thread::sleep(w);
            }
            match leitung.klein_senden(MSG_AUDIO, nachricht(MSG_AUDIO, TONPAKET), true, now_us()) {
                Ok(()) => {}
                Err(Abgewiesen::Voll) => ton_verworfen += 1,
                Err(Abgewiesen::Zu) => panic!("lebender Zuschauer ausgetragen"),
            }
            if i % 3 == 0 {
                match leitung.stau(budget(true), now_us()) {
                    Stauurteil::Frei => {
                        assert_eq!(leitung.einreihen(nachricht(MSG_VIDEO, 4000), Art::Bild, Some(HARTE_GRENZE)), Ok(()));
                        if t0.elapsed() > dauer / 2 {
                            codiert_zweite_haelfte += 1;
                        }
                    }
                    Stauurteil::Stau => ausgelassen += 1,
                    Stauurteil::Weg => panic!("lebender Zuschauer ausgetragen"),
                }
            }
            {
                let q = sperre(&leitung.q);
                hoechster_ton = hoechster_ton.max(q.ton);
                hoechster_rueckstand = hoechster_rueckstand.max(q.bytes + q.im_schreiben);
            }
            i += 1;
        }
        leitung.schliessen();
        let (bilder, ton) = endet_binnen(leser, Duration::from_secs(15)).expect("Gegenstelle endet nicht");
        assert!(endet_binnen(sender, Duration::from_secs(5)).is_some(), "Sendefaden endet nicht");
        println!(
            "Ton auf 1 Mbit/s: Bilder in der zweiten Haelfte {codiert_zweite_haelfte}, ausgelassen {ausgelassen}, Ton verworfen {ton_verworfen}, \
             hoechster Ton {hoechster_ton}, hoechster Rueckstand {hoechster_rueckstand}, empfangen {bilder} Bilder und {ton} Tonpakete"
        );
        assert!(ton_verworfen > 0, "Leitung nicht zu langsam - Probe ohne Wert");
        assert!(hoechster_ton <= ton_grenze(true) + 8 + TONPAKET, "{hoechster_ton}");
        // Bilder bis zum Budget, der Ton daneben bis zu seiner Grenze.
        assert!(hoechster_rueckstand <= budget(true) + ton_grenze(true) + 2 * (8 + TONPAKET) + 8 + 4000, "{hoechster_rueckstand}");
        assert!(codiert_zweite_haelfte >= 5, "Bild in der zweiten Haelfte gesperrt: {codiert_zweite_haelfte}");
        assert!(bilder > 0 && ton > 0);
    }

    /// Ergebnis eines Laufs im Pruefstand.
    #[derive(Debug, Default)]
    struct Lauf {
        /// Bilder, die in den "Encoder" gingen.
        codiert: u32,
        /// Von der Stauregel vor dem Encoder ausgelassen.
        ausgelassen: u32,
        /// Schon codiert und doch verworfen (harte Grenze bzw. alte Stelle).
        verworfen: u32,
        /// Bei der Gegenstelle angekommen, davon Vollbilder.
        empfangen: u32,
        vollbilder: u32,
        /// Hoechster Rueckstand, den die Stauregel vor einem Bild sah.
        hoechster_rueckstand: usize,
        /// Vom ersten ausgelassenen Bild bis "Zuschauer weg".
        weg_nach: Option<Duration>,
    }

    /// Liest `n` Byte im Takt von `mbit`, gerechnet ab `t0` mit `gelesen`
    /// Byte bisher.
    fn gedrosselt_lesen(c: &mut secure::Secure, mut n: usize, mbit: u32, t0: Instant, gelesen: &mut u64, puffer: &mut [u8]) -> Result<(), String> {
        while n > 0 {
            let k = n.min(puffer.len());
            c.read_exact(&mut puffer[..k])?;
            n -= k;
            *gelesen += k as u64;
            let soll = Duration::from_secs_f64(*gelesen as f64 * 8.0 / (mbit as f64 * 1e6));
            if let Some(w) = soll.checked_sub(t0.elapsed()) {
                std::thread::sleep(w);
            }
        }
        Ok(())
    }

    /// Endet der Faden binnen `frist`? Sonst false (und er laeuft weiter).
    fn endet_binnen<T: Send + 'static>(f: std::thread::JoinHandle<T>, frist: Duration) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f.join());
        });
        rx.recv_timeout(frist).ok().and_then(|r| r.ok())
    }

    /// Pruefstand wie hosttest.m auf dem Mac: kuenstliche Bilder mit `fps`
    /// durch die Stauregel (Leitung::stau) und den Versand
    /// (Leitung::einreihen mit HARTE_GRENZE, wie bild_senden) einer echten
    /// Leitung samt Sendefaden, auf Loopback. Die Gegenstelle liest mit
    /// `lese_mbit` (0 = gar nicht: eingefroren) und zaehlt Bilder und
    /// Vollbilder. Ein Zwischenbild hat mbit/fps, ein Vollbild `vollbild`
    /// Byte; eines kommt zu Beginn und nach jedem verworfenen Bild.
    /// `alte_stelle`: die Regel wie bis 77c4564 - nach dem Codieren
    /// verwerfen, wenn Warteschlange + SNDBUF ueber dem Budget liegen, und
    /// ein Vollbild erzwingen (Gegenprobe).
    fn pruefstand(mbit: u32, fps: u32, vollbild: usize, lese_mbit: u32, dauer: Duration, alte_stelle: bool) -> Lauf {
        let (h, mut c) = paar();
        sendepuffer_setzen(h.socket());
        let leitung = leitung_zu(&h);
        let l2 = leitung.clone();
        let sender = std::thread::spawn(move || sendefaden(l2, h));
        let (halt_tx, halt_rx) = std::sync::mpsc::channel::<()>();
        let leser = std::thread::spawn(move || {
            let (mut empfangen, mut vollbilder) = (0u32, 0u32);
            if lese_mbit == 0 {
                // Eingefroren: nichts lesen, die Leitung nur offen halten.
                let _ = halt_rx.recv();
                return (empfangen, vollbilder);
            }
            c.socket().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let t0 = Instant::now();
            let mut gelesen = 0u64;
            let mut puffer = vec![0u8; SCHREIBSTUECK];
            loop {
                let mut hdr = [0u8; 8];
                if gedrosselt_lesen(&mut c, 8, lese_mbit, t0, &mut gelesen, &mut hdr).is_err() {
                    break;
                }
                let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                if gedrosselt_lesen(&mut c, len, lese_mbit, t0, &mut gelesen, &mut puffer).is_err() {
                    break;
                }
                match hdr[0] {
                    MSG_VIDEO => {
                        empfangen += 1;
                        if hdr[1] & FLAG_KEY != 0 {
                            vollbilder += 1;
                        }
                    }
                    // Das Ende des Laufs.
                    MSG_TIME => break,
                    _ => {}
                }
            }
            (empfangen, vollbilder)
        });

        let mut lauf = Lauf::default();
        let grenze = budget(false);
        let zwischen = (mbit as usize * 1_000_000 / 8 / fps as usize).max(1);
        let bildzeit = Duration::from_secs_f64(1.0 / fps as f64);
        let mut vollbild_faellig = true;
        let mut erstes_auslassen: Option<Instant> = None;
        let t0 = Instant::now();
        let mut i = 0u32;
        while t0.elapsed() < dauer {
            if let Some(w) = (t0 + bildzeit * i).checked_duration_since(Instant::now()) {
                std::thread::sleep(w);
            }
            i += 1;
            {
                let q = sperre(&leitung.q);
                lauf.hoechster_rueckstand = lauf.hoechster_rueckstand.max(q.bytes + q.im_schreiben);
            }
            if !alte_stelle {
                match leitung.stau(grenze, now_us()) {
                    Stauurteil::Frei => {}
                    Stauurteil::Stau => {
                        lauf.ausgelassen += 1;
                        if erstes_auslassen.is_none() {
                            erstes_auslassen = Some(Instant::now());
                        }
                        continue;
                    }
                    Stauurteil::Weg => {
                        lauf.weg_nach = erstes_auslassen.map(|t| t.elapsed());
                        break;
                    }
                }
            }
            let key = std::mem::take(&mut vollbild_faellig);
            let n = if key { vollbild } else { zwischen };
            let mut p = kopf(MSG_VIDEO, if key { FLAG_KEY } else { 0 }, 0, n).to_vec();
            p.resize(8 + n, 0);
            lauf.codiert += 1;
            let r = if alte_stelle {
                let voll = sperre(&leitung.q).bytes + SNDBUF > grenze;
                if voll { Err(Abgewiesen::Voll) } else { leitung.einreihen(p, Art::Bild, None) }
            } else {
                leitung.einreihen(p, Art::Bild, Some(HARTE_GRENZE))
            };
            match r {
                Ok(()) => {}
                Err(Abgewiesen::Voll) => {
                    lauf.verworfen += 1;
                    vollbild_faellig = true;
                }
                Err(Abgewiesen::Zu) => break,
            }
        }
        if lauf.weg_nach.is_none() {
            // Ende des Laufs: die Gegenstelle liest, was noch kommt, bis zum
            // Schlusszeichen; dann ist die Leitung zu.
            let _ = leitung.einreihen(kopf(MSG_TIME, 0, 0, 0).to_vec(), Art::Klein, None);
        }
        let _ = halt_tx.send(());
        let (empfangen, vollbilder) = endet_binnen(leser, Duration::from_secs(15)).expect("Gegenstelle endet nicht");
        lauf.empfangen = empfangen;
        lauf.vollbilder = vollbilder;
        leitung.schliessen();
        assert!(endet_binnen(sender, Duration::from_secs(5)).is_some(), "Sendefaden endet nicht");
        lauf
    }

    /// Normale Last (hosttest.m: 50 Mbit/s auf 100 Mbit/s): nichts
    /// ausgelassen, nichts verworfen, jedes Bild kommt an, nur das erste ist
    /// ein Vollbild.
    #[test]
    fn stauregel_laesst_bei_normaler_last_nichts_aus() {
        let lauf = pruefstand(30, 60, 1 << 20, 80, Duration::from_secs(3), false);
        println!("normale Last: {lauf:?}");
        assert_eq!((lauf.ausgelassen, lauf.verworfen), (0, 0), "{lauf:?}");
        assert!(lauf.codiert >= 150, "{lauf:?}");
        assert_eq!(lauf.empfangen, lauf.codiert, "{lauf:?}");
        assert_eq!(lauf.vollbilder, 1, "{lauf:?}");
    }

    /// Zu langsame Leitung (60 Mbit/s auf 40): ausgelassen wird vor dem
    /// Encoder, jedes codierte Bild kommt an, und es bleibt beim einen
    /// Vollbild vom Anfang - Zwischenbilder statt einer Vollbild-Kaskade.
    /// Der Rueckstand bleibt beim Budget plus einem Bild. Gegenprobe mit der
    /// alten Stelle: dort folgt nach dem ersten Verwerfen Vollbild auf
    /// Vollbild, und es kommen weniger Bilder an (auf der VM gemessen: 55
    /// Bilder, davon 18 Vollbilder, gegen 175 mit einem).
    #[test]
    fn stauregel_zu_langsame_leitung_ohne_vollbild_kaskade() {
        let (mbit, fps, vollbild, lese, dauer) = (60, 60, 1usize << 20, 40, Duration::from_secs(4));
        let neu = pruefstand(mbit, fps, vollbild, lese, dauer, false);
        let alt = pruefstand(mbit, fps, vollbild, lese, dauer, true);
        println!("zu langsam, vor dem Encoder: {neu:?}");
        println!("zu langsam, alte Stelle:     {alt:?}");
        assert!(neu.ausgelassen > 0, "Leitung nicht zu langsam - Probe ohne Wert: {neu:?}");
        assert_eq!(neu.verworfen, 0, "{neu:?}");
        assert_eq!(neu.empfangen, neu.codiert, "{neu:?}");
        assert_eq!(neu.vollbilder, 1, "{neu:?}");
        assert!(neu.hoechster_rueckstand <= BACKLOG_LIMIT + vollbild, "{neu:?}");
        // So viele Zwischenbilder, wie die Leitung in der Zeit fasst - mit
        // Abschlag fuer das Vollbild und das Zittern der Uhr.
        let fasst = (lese as f64 * 1e6 / 8.0 * dauer.as_secs_f64() / (mbit as f64 * 1e6 / 8.0 / fps as f64)) as u32;
        assert!(neu.empfangen * 10 >= fasst * 7, "{} von {fasst}: {neu:?}", neu.empfangen);
        // Gegenprobe: der Pruefstand erkennt eine Kaskade.
        assert!(alt.verworfen > 0 && alt.vollbilder >= 5, "alte Stelle ohne Kaskade: {alt:?}");
        assert!(neu.empfangen > alt.empfangen, "neu {neu:?} alt {alt:?}");
    }

    /// Eingefrorener Zuschauer bei ausgeschaltetem Ton: im Stau nimmt er
    /// nichts mehr ab und ist STAU_FRIST_US nach dem ersten ausgelassenen
    /// Bild weg - die Leitung ist zu, der Sendefaden endet, obwohl er in
    /// einem write hing.
    #[test]
    fn stauregel_traegt_eingefrorenen_zuschauer_nach_2_s_aus() {
        let lauf = pruefstand(40, 60, 1 << 20, 0, Duration::from_secs(12), false);
        println!("eingefroren: {lauf:?}");
        let weg = lauf.weg_nach.expect("Zuschauer nicht ausgetragen");
        assert!(weg >= Duration::from_micros(STAU_FRIST_US) - Duration::from_millis(50), "{weg:?}");
        // Nach oben 2 s Spielraum: auf geteilten CI-Runnern (4 Kerne, die
        // ganze Testreihe parallel) kam der Austrag nach 3,4 s - die Frist
        // wird dann spaeter geprueft, nicht falsch. Die VM liegt bei gut 2 s.
        assert!(weg <= Duration::from_micros(STAU_FRIST_US) + Duration::from_secs(2), "{weg:?}");
        assert_eq!(lauf.verworfen, 0, "{lauf:?}");
    }

    // -------------------------------------------------------- Dateien
    // Ohne Laufmarke und ohne AKTUELL/NR: jeder Test hat seinen eigenen Platz und
    // Zaehler (statics im Test) und eine eigene DateiUmgebung mit Rekorder
    // statt der Windows-Ablage und eigener Basis unter temp_dir.

    /// Ein eigener, leerer Ordner je Test unter temp_dir.
    fn test_ordner(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("qc-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Rekorder statt der Windows-Ablage: merkt sich jede abgelegte Liste.
    #[derive(Clone, Default)]
    struct Rekorder(Arc<Mutex<Vec<Vec<PathBuf>>>>);

    impl Rekorder {
        fn abgelegt(&self) -> Vec<Vec<PathBuf>> {
            sperre(&self.0).clone()
        }
        fn ablegen(&self) -> Arc<dyn Fn(&[PathBuf]) -> bool + Send + Sync> {
            let r = self.0.clone();
            Arc::new(move |p: &[PathBuf]| {
                sperre(&r).push(p.to_vec());
                true
            })
        }
    }

    /// Staende, die Sender und Empfaenger melden.
    #[derive(Clone, Default)]
    struct Staende(Arc<Mutex<Vec<dateien::Stand>>>);

    impl Staende {
        fn hook(&self) -> Arc<dyn Fn(&dateien::Stand) + Send + Sync> {
            let s = self.0.clone();
            Arc::new(move |st: &dateien::Stand| sperre(&s).push(st.clone()))
        }
        fn fertig(&self, richtung: dateien::Richtung) -> bool {
            sperre(&self.0).iter().any(|s| s.richtung == richtung && s.ergebnis == dateien::Ergebnis::Fertig)
        }
    }

    /// Wie im Dienst die eigene Basis der Host-Rolle, nur mit laengerer
    /// Stillstandsfrist.
    fn test_vorgaben() -> dateien::Vorgaben {
        dateien::Vorgaben {
            stillstand: Duration::from_secs(10),
            eigene_basis: host_ablage_basis,
            ..dateien::Vorgaben::default()
        }
    }

    /// Protokollzeilen der Dateiuebertragung (DateiUmgebung::zeile).
    #[derive(Clone, Default)]
    struct Zeilen(Arc<Mutex<Vec<String>>>);

    impl Zeilen {
        fn hook(&self) -> Arc<dyn Fn(String) + Send + Sync> {
            let z = self.0.clone();
            Arc::new(move |t: String| {
                println!("{t}");
                sperre(&z).push(t)
            })
        }
        fn alle(&self) -> Vec<String> {
            sperre(&self.0).clone()
        }
        fn zahl(&self, f: impl Fn(&str) -> bool) -> usize {
            sperre(&self.0).iter().filter(|z| f(z)).count()
        }
    }

    fn umgebung_test(
        platz: &'static Mutex<Option<Arc<Leitung>>>,
        zaehler: &'static AtomicU64,
        name: &str,
        r: &Rekorder,
        s: &Staende,
    ) -> DateiUmgebung {
        umgebung_mit_zeilen(platz, zaehler, name, r, s, &Zeilen::default())
    }

    fn umgebung_mit_zeilen(
        platz: &'static Mutex<Option<Arc<Leitung>>>,
        zaehler: &'static AtomicU64,
        name: &str,
        r: &Rekorder,
        s: &Staende,
        z: &Zeilen,
    ) -> DateiUmgebung {
        DateiUmgebung {
            platz,
            zaehler,
            basis: test_ordner(name),
            ablegen: r.ablegen(),
            stand: s.hook(),
            zeile: z.hook(),
            vormerken: VORMERKEN,
            vorgaben: test_vorgaben(),
        }
    }

    /// Traegt `l` als neuen Zuschauer in `platz` ein, wie bild_annehmen
    /// (ein Vorgaenger wird abgeloest); liefert seine Nummer.
    fn zuschauer_eintragen(platz: &Mutex<Option<Arc<Leitung>>>, zaehler: &AtomicU64, l: Arc<Leitung>) -> u64 {
        let mut p = sperre(platz);
        if let Some(alt) = p.take() {
            alt.abloesen();
        }
        let nr = zaehler.fetch_add(1, Ordering::Relaxed) + 1;
        l.nr.store(nr, Ordering::Relaxed);
        *p = Some(l);
        nr
    }

    fn tcp_paar() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (h, _) = l.accept().unwrap();
        (h, c)
    }

    /// Bindet Eingabekanal `einr` an `l` und laesst ihn Dateien melden. Die
    /// beiden Enden muessen leben bleiben.
    fn faehig_machen(l: &Leitung, einr: u64) -> (TcpStream, TcpStream) {
        let (h, c) = tcp_paar();
        assert!(l.eingabe_binden(einr, h.try_clone().unwrap()));
        l.faehigkeiten_setzen(einr, FAEHIG_DATEIEN);
        assert!(l.kann_dateien());
        (h, c)
    }

    /// Nimmt alles aus der Warteschlange wie der Sendefaden: (Art, Typ, Nutzlast).
    fn abholen(l: &Leitung) -> Vec<(Art, u8, Vec<u8>)> {
        let mut q = sperre(&l.q);
        let mut v = Vec::new();
        while let Some(art) = q.pakete.front().map(|(a, _)| *a) {
            let p = q.nehmen().unwrap();
            v.push((art, p[0], p[8..].to_vec()));
        }
        v
    }

    /// Holt ab, bis ein Paket `soll` erfuellt (hoechstens `frist`); liefert
    /// alles bis dahin Abgeholte.
    fn abholen_bis(l: &Leitung, frist: Duration, soll: impl Fn(&(Art, u8, Vec<u8>)) -> bool) -> Vec<(Art, u8, Vec<u8>)> {
        let bis = Instant::now() + frist;
        let mut alles = Vec::new();
        loop {
            let neu = abholen(l);
            let gefunden = neu.iter().any(&soll);
            alles.extend(neu);
            if gefunden {
                return alles;
            }
            let kurz: Vec<(Art, u8, usize)> = alles.iter().map(|(a, t, n)| (*a, *t, n.len())).collect();
            assert!(Instant::now() < bis, "nicht gekommen; abgeholt: {kurz:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Ist unter den Paketen ein Ende dieser Kennung mit diesem Grund?
    fn ist_ende(p: &(Art, u8, Vec<u8>), kennung: u32, grund: u8) -> bool {
        p.1 == DATEI_ENDE && dateien::Ende::lesen(&p.2) == Some(dateien::Ende { kennung, grund })
    }

    /// Art::Datei hat ihre eigene Grenze: hoechstens ein wartendes Paket und
    /// nur bis DATEI_RUECKSTAND Rueckstand (Voll), nur an einen Zuschauer,
    /// dessen AKTUELLER Eingabekanal Dateien gemeldet hat - und sie zaehlt
    /// nicht gegen KLEIN_GRENZE.
    #[test]
    fn datei_pakete_zaehlen_nicht_gegen_klein_grenze_und_voll_greift() {
        let l = Leitung::neu(None, Vec::new(), Vec::new(), String::new());
        let stueck = vec![7u8; dateien::STUECK_KOPF + dateien::STUECK_MAX];
        let ende = dateien::Ende { kennung: 1, grund: 0 }.kodieren();
        // Ohne Eingabekanal, ohne Meldung, ohne das Bit, mit der Meldung
        // eines anderen Kanals: Voll, nichts eingereiht.
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Voll);
        let (h, _c) = tcp_paar();
        assert!(l.eingabe_binden(7, h.try_clone().unwrap()));
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Voll);
        l.faehigkeiten_setzen(7, 0);
        assert!(!l.kann_dateien());
        l.faehigkeiten_setzen(6, FAEHIG_DATEIEN);
        assert!(!l.kann_dateien(), "Meldung eines fremden Kanals gilt");
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Voll);
        assert!(sperre(&l.q).pakete.is_empty());
        l.faehigkeiten_setzen(7, FAEHIG_DATEIEN);
        assert!(l.kann_dateien());
        // Ein neuer Eingabekanal desselben Zuschauers gilt erst nach seiner
        // eigenen Meldung.
        let (h2, _c2) = tcp_paar();
        assert!(l.eingabe_binden(8, h2.try_clone().unwrap()));
        assert!(!l.kann_dateien(), "Faehigkeit des alten Eingabekanals gilt weiter");
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Voll);
        l.faehigkeiten_setzen(8, FAEHIG_DATEIEN);
        // Ein Paket geht hinein, mit Kopf; solange es wartet, kein zweites.
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Ja);
        assert_eq!(l.datei_einreihen(DATEI_ENDE, &ende), Gesendet::Voll);
        {
            let mut q = sperre(&l.q);
            assert_eq!((q.datei, q.klein, q.bytes, q.pakete.len()), (1, 0, 8 + stueck.len(), 1));
            assert_eq!(q.pakete[0].0, Art::Datei);
            assert_eq!(q.pakete[0].1[..8], kopf(DATEI_STUECK, 0, 0, stueck.len()));
            assert_eq!(q.pakete[0].1[8..], stueck[..]);
            // Der Sendefaden nimmt es und schreibt noch daran: frei.
            let p = q.nehmen().unwrap();
            q.im_schreiben = p.len();
            assert_eq!((q.datei, q.bytes), (0, 0));
        }
        assert_eq!(l.datei_einreihen(DATEI_ENDE, &ende), Gesendet::Ja);
        // Rueckstand: genau DATEI_RUECKSTAND ist frei, darueber Voll - auch
        // ohne wartendes Datei-Paket, und der Rest im Schreiben zaehlt mit.
        for (warten, schreiben, soll) in [
            (DATEI_RUECKSTAND, 0, Gesendet::Ja),
            (DATEI_RUECKSTAND + 1, 0, Gesendet::Voll),
            (0, DATEI_RUECKSTAND + 1, Gesendet::Voll),
            (DATEI_RUECKSTAND / 2, DATEI_RUECKSTAND / 2, Gesendet::Ja),
        ] {
            {
                let mut q = sperre(&l.q);
                q.leeren();
                q.im_schreiben = schreiben;
            }
            if warten > 0 {
                assert_eq!(l.einreihen(vec![0u8; warten], Art::Bild, None), Ok(()));
            }
            assert_eq!(l.datei_einreihen(DATEI_ENDE, &ende), soll, "wartend {warten}, im Schreiben {schreiben}");
        }
        {
            let mut q = sperre(&l.q);
            q.leeren();
            q.im_schreiben = 0;
        }
        // Art::Datei zaehlt nicht gegen KLEIN_GRENZE: auch ueber 8 MB
        // wartende Datei-Pakete (hier am Voll vorbei eingereiht) lassen eine
        // Steuernachricht durch, und der Zuschauer bleibt.
        let mut n = 0;
        while n <= KLEIN_GRENZE {
            assert_eq!(l.einreihen(vec![0u8; 1 << 20], Art::Datei, None), Ok(()));
            n += 1 << 20;
        }
        assert_eq!(sperre(&l.q).klein, 0);
        assert_eq!(l.klein_senden(MSG_TIME, nachricht(MSG_TIME, 16), false, 1_000), Ok(()));
        assert!(l.offen(), "Datei-Pakete gegen KLEIN_GRENZE gezaehlt");
        {
            let q = sperre(&l.q);
            assert_eq!((q.klein, q.datei), (8 + 16, KLEIN_GRENZE / (1 << 20) + 1));
        }
        // Zu: Weg.
        l.schliessen();
        assert_eq!(l.datei_einreihen(DATEI_STUECK, &stueck), Gesendet::Weg);
    }

    /// Beide Datei-Wege sind an die Zuschauernummer gebunden (Muster
    /// send_small_an): nach einem Wechsel meldet der Weg des Alten Weg, und
    /// beim Neuen kommt nichts davon an. Der Wechsel bricht Sender und
    /// Empfaenger des Alten ab; neue entstehen fuer ihn nicht mehr.
    #[test]
    fn datei_wege_sind_an_die_zuschauernummer_gebunden() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = umgebung_test(&PLATZ, &ZAEHLER, "nummer", &r, &s);
        let quelle = test_ordner("nummer-quelle").join("a.bin");
        std::fs::write(&quelle, vec![1u8; 100_000]).unwrap();
        let ende = dateien::Ende { kennung: 1, grund: 0 }.kodieren();
        let quittung = dateien::Quittung { kennung: 1, zustand: 0, empfangen: 0 }.kodieren();
        // Ohne Zuschauer: Weg.
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, 0, DATEI_ENDE, &ende), Gesendet::Weg);
        assert_eq!(klein_an(&PLATZ, &ZAEHLER, 0, DATEI_QUITTUNG, &quittung), Gesendet::Weg);
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "A".into()));
        let nr_a = zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let _ea = faehig_machen(&a, 1);
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr_a, DATEI_ENDE, &ende), Gesendet::Ja);
        assert_eq!(klein_an(&PLATZ, &ZAEHLER, nr_a, DATEI_QUITTUNG, &quittung), Gesendet::Ja);
        // A hat einen Empfaenger (erstes Angebot) und einen Sender.
        a.empfaenger_bereit(&u);
        assert!(matches!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Gestartet(_)));
        {
            let d = sperre(&a.dateien);
            assert!(d.empfaenger.is_some() && d.sender.is_some());
        }
        // B loest A ab.
        let b = Arc::new(Leitung::neu(None, vec![2], vec![2], "B".into()));
        let nr_b = zuschauer_eintragen(&PLATZ, &ZAEHLER, b.clone());
        assert_eq!(nr_b, nr_a + 1);
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr_a, DATEI_ENDE, &ende), Gesendet::Weg, "Datei-Paket fuer A ging an B");
        assert_eq!(klein_an(&PLATZ, &ZAEHLER, nr_a, DATEI_QUITTUNG, &quittung), Gesendet::Weg, "Quittung fuer A ging an B");
        {
            let d = sperre(&a.dateien);
            assert!(d.zu && d.empfaenger.is_none() && d.sender.is_none(), "Sender oder Empfaenger von A laufen nach dem Wechsel weiter");
        }
        a.empfaenger_bereit(&u);
        assert!(sperre(&a.dateien).empfaenger.is_none(), "nach dem Wechsel neuer Empfaenger fuer A");
        // An B: Voll, bis sein Eingabekanal Dateien meldet, dann Ja.
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr_b, DATEI_ENDE, &ende), Gesendet::Voll);
        let _eb = faehig_machen(&b, 2);
        std::thread::sleep(Duration::from_millis(50));
        assert!(sperre(&b.q).pakete.is_empty(), "B hat bekommen, was an A ging");
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr_b, DATEI_ENDE, &ende), Gesendet::Ja);
        assert_eq!(klein_an(&PLATZ, &ZAEHLER, nr_b, DATEI_QUITTUNG, &quittung), Gesendet::Ja);
        // B geht: Weg.
        b.schliessen();
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr_b, DATEI_ENDE, &ende), Gesendet::Weg);
        assert_eq!(klein_an(&PLATZ, &ZAEHLER, nr_b, DATEI_QUITTUNG, &quittung), Gesendet::Weg);
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Die Quittungen des Empfaengers (Client -> Host) gehen als kleine
    /// Steuernachricht hinaus, auch wenn der eigene Sender den einen
    /// Datei-Platz belegt und der Rueckstand ueber DATEI_RUECKSTAND liegt.
    /// Fertige Dateien landen in der eigenen Basis der Host-Rolle, der
    /// Rekorder bekommt sie.
    #[test]
    fn quittungen_gehen_trotz_belegtem_datei_platz_hinaus() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = umgebung_test(&PLATZ, &ZAEHLER, "quittung", &r, &s);
        let l = Arc::new(Leitung::neu(None, vec![1], vec![1], "C".into()));
        let nr = zuschauer_eintragen(&PLATZ, &ZAEHLER, l.clone());
        let _e = faehig_machen(&l, 1);
        // Der eigene Sender belegt den Datei-Platz, dazu Bild im Rueckstand.
        let stueck = vec![0u8; dateien::STUECK_KOPF + dateien::STUECK_MAX];
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr, DATEI_STUECK, &stueck), Gesendet::Ja);
        assert_eq!(l.einreihen(vec![0u8; DATEI_RUECKSTAND], Art::Bild, None), Ok(()));
        assert_eq!(datei_an(&PLATZ, &ZAEHLER, nr, DATEI_STUECK, &stueck), Gesendet::Voll);
        // Angebot, Stueck und Ende vom Client, wie sie die Leseschleife
        // weitergibt.
        let angebot = dateien::Angebot {
            kennung: 7,
            gesamt: 5,
            eintraege: vec![dateien::Eintrag { art: dateien::EintragArt::Datei, pfad: "a.txt".into(), groesse: 5 }],
        };
        l.empfaenger_bereit(&u);
        l.datei_eingang(DATEI_ANGEBOT, angebot.kodieren());
        l.datei_eingang(DATEI_STUECK, dateien::Stueck { kennung: 7, eintrag: 0, versatz: 0, daten: b"hallo" }.kodieren());
        l.datei_eingang(DATEI_ENDE, dateien::Ende { kennung: 7, grund: 0 }.kodieren());
        let bis = Instant::now() + Duration::from_secs(10);
        let quittungen = loop {
            let qs: Vec<(Art, dateien::Quittung)> = sperre(&l.q)
                .pakete
                .iter()
                .filter(|(_, p)| p[0] == DATEI_QUITTUNG)
                .map(|(art, p)| (*art, dateien::Quittung::lesen(&p[8..]).unwrap()))
                .collect();
            if qs.iter().any(|(_, q)| q.zustand != dateien::ZUSTAND_LAEUFT) || Instant::now() > bis {
                break qs;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        println!("Quittungen bei belegtem Datei-Platz: {quittungen:?}");
        assert!(quittungen.iter().all(|(art, _)| *art == Art::Klein), "{quittungen:?}");
        let erste = quittungen.first().map(|(_, q)| (q.kennung, q.zustand, q.empfangen));
        let letzte = quittungen.last().map(|(_, q)| (q.kennung, q.zustand, q.empfangen));
        assert_eq!(erste, Some((7, dateien::ZUSTAND_LAEUFT, 0)), "{quittungen:?}");
        assert_eq!(letzte, Some((7, dateien::ZUSTAND_FERTIG, 5)), "{quittungen:?}");
        assert_eq!(sperre(&l.q).datei, 1, "der Datei-Platz blieb nicht belegt - Probe ohne Wert");
        // In der eigenen Basis, mit Inhalt, beim Rekorder.
        let abgelegt = r.abgelegt();
        assert_eq!(abgelegt.len(), 1, "{abgelegt:?}");
        assert_eq!(abgelegt[0].len(), 1);
        assert!(abgelegt[0][0].starts_with(&u.basis), "{abgelegt:?}");
        assert_eq!(std::fs::read(&abgelegt[0][0]).unwrap(), b"hallo");
        assert!(s.fertig(dateien::Richtung::Empfangen));
        l.schliessen();
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Die Host-Rolle hat eine eigene Ablagebasis (nicht die des Clients);
    /// beim Start geht dort nur, was aelter als 24 h ist.
    #[test]
    fn host_ablage_eigene_basis_und_aufraeumen_beim_start() {
        let basis = host_ablage_basis();
        assert_ne!(basis, dateien::ablage_basis());
        assert!(basis.starts_with(std::env::temp_dir()));
        let jetzt_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let alt = basis.join(format!("{}-1", jetzt_ms - 25 * 3600 * 1000));
        let frisch = basis.join(format!("{}-2", jetzt_ms - 3600 * 1000));
        for d in [&alt, &frisch] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("x.txt"), b"x").unwrap();
        }
        assert_eq!(host_ablage_aufraeumen(), 1);
        assert!(!alt.exists() && frisch.exists());
        let _ = std::fs::remove_dir_all(&basis);
    }

    /// Der Bildschirmwunsch (70) geht ueber den echten Eingabekanal in die
    /// Vormerkung des Aufnahmefadens (aufnahme::bildschirm_wunsch_abholen):
    /// eine Kennung, Automatik, der letzte gewinnt; Ungueltiges (Laenge,
    /// Steuerzeichen) wird uebergangen, ohne den Kanal zu beenden.
    #[test]
    fn bildschirmwunsch_kommt_ueber_den_eingabekanal() {
        use super::super::aufnahme::bildschirm_wunsch_abholen;
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = Arc::new(umgebung_test(&PLATZ, &ZAEHLER, "bildschirmwunsch", &r, &s));
        let (mut h, mut c) = paar();
        let l = Arc::new(Leitung::neu(None, vec![1], vec![1], "C".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, l.clone());
        assert!(l.eingabe_binden(1, h.abbruchgriff().unwrap()));
        let (l2, u2) = (l.clone(), u.clone());
        let leser = std::thread::spawn(move || eingabe_lesen(&mut h, &l2, 1, &u2));
        let wunsch = |c: &mut secure::Secure, nutzlast: &[u8]| {
            let mut m = kopf(IN_BILDSCHIRM, 0, 0, nutzlast.len()).to_vec();
            m.extend_from_slice(nutzlast);
            c.write_all(&m).unwrap();
        };
        let abholen = || {
            let bis = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(w) = bildschirm_wunsch_abholen() {
                    return w;
                }
                assert!(Instant::now() < bis, "kein Wunsch angekommen");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let _ = bildschirm_wunsch_abholen();
        // Der Pruefvektor (2.4): Wunsch auf v0-m0-s0.
        wunsch(&mut c, &[8, 0x76, 0x30, 0x2d, 0x6d, 0x30, 0x2d, 0x73, 0x30]);
        assert_eq!(abholen(), Some("v0-m0-s0".into()));
        // Automatik.
        wunsch(&mut c, &[0]);
        assert_eq!(abholen(), None);
        // Zwei kurz nacheinander: der letzte gewinnt (kein Puffer).
        wunsch(&mut c, b"\x03ABC");
        wunsch(&mut c, b"\x03DEF");
        faehigkeiten_melden(&mut c).unwrap();
        let bis = Instant::now() + Duration::from_secs(5);
        while !l.kann_dateien() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(l.kann_dateien());
        assert_eq!(bildschirm_wunsch_abholen(), Some(Some("DEF".into())));
        // Ungueltig: Laenge ueber dem Rest, Steuerzeichen, leer - uebergangen,
        // der Kanal lebt (die Faehigkeiten danach kommen an).
        for kaputt in [&b"\x05ab"[..], &b"\x02a\n"[..], &b""[..]] {
            wunsch(&mut c, kaputt);
            let mut m = kopf(IN_FAEHIGKEITEN, 0, 0, 4).to_vec();
            m.extend_from_slice(&dateien::faehigkeiten_kodieren(0));
            c.write_all(&m).unwrap();
            let bis = Instant::now() + Duration::from_secs(5);
            while l.kann_dateien() && Instant::now() < bis {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(!l.kann_dateien(), "Kanal nach {kaputt:?} nicht mehr am Leben");
            assert_eq!(bildschirm_wunsch_abholen(), None, "{kaputt:?} durchgelassen");
            faehigkeiten_melden(&mut c).unwrap();
            let bis = Instant::now() + Duration::from_secs(5);
            while !l.kann_dateien() && Instant::now() < bis {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(l.kann_dateien());
        }
        // Ueber 65 Byte ist der Wunsch nie; die Grenze des Typs bleibt 256.
        assert_eq!(eingangsgrenze(IN_BILDSCHIRM), 256);
        l.schliessen();
        assert!(endet_binnen(leser, Duration::from_secs(5)).is_some());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Nimmt die Grenzen aus 2.6 fuer 50-53 und 69: genau an der Grenze
    /// bleibt der Kanal (auch bei einem Angebot aus Nullen, das der
    /// Empfaenger mit Quittung ablehnt), ein Byte mehr beendet ihn - ohne die
    /// Nutzlast zu lesen. Die alte 256-Byte-Regel gilt fuer andere Typen weiter.
    #[test]
    fn eingabekanal_grenzen_fuer_dateien() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = Arc::new(umgebung_test(&PLATZ, &ZAEHLER, "grenzen", &r, &s));
        let faelle = [
            (DATEI_ANGEBOT, dateien::ANGEBOT_MAX),
            (DATEI_STUECK, dateien::STUECK_KOPF + dateien::STUECK_MAX),
            (DATEI_ENDE, 256),
            (DATEI_QUITTUNG, 256),
            (IN_FAEHIGKEITEN, 256),
            (200, 256),
        ];
        for (typ, grenze) in faelle {
            let (mut h, mut c) = paar();
            let l = Arc::new(Leitung::neu(None, vec![1], vec![1], "C".into()));
            zuschauer_eintragen(&PLATZ, &ZAEHLER, l.clone());
            assert!(l.eingabe_binden(1, h.abbruchgriff().unwrap()));
            let (l2, u2) = (l.clone(), u.clone());
            let leser = std::thread::spawn(move || eingabe_lesen(&mut h, &l2, 1, &u2));
            // Mit Frist: liest die Schleife nicht mehr, haengt das Schreiben
            // sonst (die Leitung haelt den Kanal noch offen).
            c.socket().set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut m = kopf(typ, 0, 0, grenze).to_vec();
            m.resize(8 + grenze, 0);
            let r = c.write_all(&m);
            assert!(r.is_ok(), "Typ {typ}: Nachricht mit {grenze} Byte nicht gelesen: {r:?}");
            // Der Kanal lebt: die Meldung danach kommt an.
            let r = faehigkeiten_melden(&mut c);
            assert!(r.is_ok(), "Typ {typ}: Kanal nach einer Nachricht mit {grenze} Byte beendet: {r:?}");
            let bis = Instant::now() + Duration::from_secs(5);
            while !l.kann_dateien() && Instant::now() < bis {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(l.kann_dateien(), "Typ {typ}: Kanal nach einer Nachricht mit {grenze} Byte beendet");
            c.write_all(&kopf(typ, 0, 0, grenze + 1)).unwrap();
            assert_eq!(
                endet_binnen(leser, Duration::from_secs(5)),
                Some(Kanalende::ZuGross { typ, len: grenze + 1 }),
                "Typ {typ}"
            );
            l.schliessen();
        }
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Alle Eintraege unter `wurzel`, relativ und sortiert, mit Inhalt
    /// (None fuer Ordner).
    fn baum(wurzel: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
        fn gehen(basis: &std::path::Path, p: &std::path::Path, v: &mut Vec<(String, Option<Vec<u8>>)>) {
            for e in std::fs::read_dir(p).unwrap() {
                let e = e.unwrap();
                let pfad = e.path();
                let rel = pfad.strip_prefix(basis).unwrap().to_string_lossy().replace('\\', "/");
                if e.file_type().unwrap().is_dir() {
                    v.push((rel, None));
                    gehen(basis, &pfad, v);
                } else {
                    v.push((rel, Some(std::fs::read(&pfad).unwrap())));
                }
            }
        }
        let mut v = Vec::new();
        gehen(wurzel, wurzel, &mut v);
        v.sort();
        v
    }

    /// Quelle fuer einen Lauf: verschachtelte Ordner (einer leer), eine leere
    /// Datei, eine Datei ueber viele Stuecke (mehr als ein Fenster) und
    /// Nicht-ASCII im Namen.
    fn quelle_anlegen(name: &str, saat: u8) -> PathBuf {
        let wurzel = test_ordner(name).join(format!("Quelle {saat}"));
        let tief = wurzel.join("Bilder").join("Urlaub").join("tief");
        std::fs::create_dir_all(&tief).unwrap();
        std::fs::create_dir_all(wurzel.join("leerer Ordner")).unwrap();
        std::fs::write(wurzel.join("leer.txt"), b"").unwrap();
        std::fs::write(wurzel.join("Bilder").join("a.txt"), b"hallo").unwrap();
        std::fs::write(wurzel.join("Bilder").join("Urlaub").join("\u{c4}rger \u{fc} \u{20ac}.txt"), "Gr\u{fc}\u{df}e".as_bytes()).unwrap();
        let gross: Vec<u8> = (0..700_000u32).map(|i| ((i.wrapping_mul(2_654_435_761) >> 13) as u8) ^ saat).collect();
        std::fs::write(tief.join("gross.bin"), &gross).unwrap();
        wurzel
    }

    /// Ein Lauf in beide Richtungen zugleich ueber Loopback, mit dem
    /// vorhandenen Stapel: Bildkanal mit Sendefaden, Eingabekanal mit der
    /// Leseschleife des Dienstes, dazu ein Scheinclient mit dem Kern
    /// (Sender und Empfaenger). Client -> Host: Angebot, Stuecke und Ende
    /// ueber den Eingabekanal, Quittungen ueber den Bildkanal; Host ->
    /// Client umgekehrt. Am Ende stimmen beide Baeume Byte fuer Byte.
    #[test]
    fn dateien_in_beide_richtungen_ueber_loopback() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r_host, s_host) = (Rekorder::default(), Staende::default());
        let u = Arc::new(umgebung_test(&PLATZ, &ZAEHLER, "lauf-host-ablage", &r_host, &s_host));
        // Der Dienst: Bildkanal mit Sendefaden, Eingabekanal mit Leseschleife.
        let (h_bild, mut c_bild) = paar();
        sendepuffer_setzen(h_bild.socket());
        let l = leitung_zu(&h_bild);
        zuschauer_eintragen(&PLATZ, &ZAEHLER, l.clone());
        let l2 = l.clone();
        let sendefaden_griff = std::thread::spawn(move || sendefaden(l2, h_bild));
        let (mut h_ein, c_ein) = paar();
        assert!(l.eingabe_binden(1, h_ein.abbruchgriff().unwrap()));
        let (l3, u3) = (l.clone(), u.clone());
        let leseschleife = std::thread::spawn(move || eingabe_lesen(&mut h_ein, &l3, 1, &u3));
        // Der Scheinclient schreibt auf den Eingabekanal ...
        c_ein.socket().set_write_timeout(Some(Duration::from_secs(20))).unwrap();
        let c_ein = Arc::new(Mutex::new(c_ein));
        let c = c_ein.clone();
        let weg_c: Arc<dyn dateien::Weg> = Arc::new(move |typ: u8, n: &[u8]| {
            let mut m = kopf(typ, 0, 0, n.len()).to_vec();
            m.extend_from_slice(n);
            match sperre(&c).write_all(&m) {
                Ok(()) => Gesendet::Ja,
                Err(_) => Gesendet::Weg,
            }
        });
        faehigkeiten_melden(&mut sperre(&c_ein)).unwrap();
        let bis = Instant::now() + Duration::from_secs(5);
        while !l.kann_dateien() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(l.kann_dateien());
        // ... und hat Empfaenger und Sender aus dem Kern.
        let (r_client, s_client) = (Rekorder::default(), Staende::default());
        let client_basis = test_ordner("lauf-client-ablage");
        let ablegen_c = r_client.ablegen();
        let stand_c = s_client.hook();
        let c_empf = Arc::new(dateien::Empfaenger::neu_mit(
            client_basis.clone(),
            weg_c.clone(),
            move |p: Vec<PathBuf>| ablegen_c(&p),
            move |e: dateien::Ereignis| {
                if let Some(s) = &e.stand {
                    stand_c(s);
                }
            },
            test_vorgaben(),
        ));
        let c_griff: Arc<Mutex<Option<dateien::Griff>>> = Arc::new(Mutex::new(None));
        // ... und liest den Bildkanal: 50-52 an den Empfaenger, 53 an den Sender.
        let (e2, g2) = (c_empf.clone(), c_griff.clone());
        let bildleser = std::thread::spawn(move || {
            c_bild.socket().set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let mut gezaehlt = [0u32; 4];
            loop {
                let mut hdr = [0u8; 8];
                if c_bild.read_exact(&mut hdr).is_err() {
                    break;
                }
                let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                let mut p = vec![0u8; len];
                if c_bild.read_exact(&mut p).is_err() {
                    break;
                }
                match hdr[0] {
                    DATEI_ANGEBOT | DATEI_STUECK | DATEI_ENDE => {
                        gezaehlt[(hdr[0] - DATEI_ANGEBOT) as usize] += 1;
                        e2.nachricht(hdr[0], p);
                    }
                    DATEI_QUITTUNG => {
                        gezaehlt[3] += 1;
                        if let Some(g) = sperre(&g2).as_ref() {
                            g.quittung(&p);
                        }
                    }
                    _ => {}
                }
            }
            gezaehlt
        });
        let quelle_c = quelle_anlegen("lauf-quelle-client", 1);
        let quelle_h = quelle_anlegen("lauf-quelle-host", 2);
        // Beide Richtungen zugleich.
        let t0 = Instant::now();
        let stand_cs = s_client.hook();
        *sperre(&c_griff) = Some(dateien::Sender::starten_mit(
            vec![quelle_c.clone()],
            weg_c.clone(),
            dateien::FENSTER,
            move |e: dateien::Ereignis| {
                if let Some(s) = &e.stand {
                    stand_cs(s);
                }
            },
            test_vorgaben(),
        ));
        assert!(matches!(dateien_senden_mit(&u, vec![quelle_h.clone()]), Start::Gestartet(_)));
        let bis = Instant::now() + Duration::from_secs(30);
        let fertig = || {
            s_host.fertig(dateien::Richtung::Senden)
                && s_host.fertig(dateien::Richtung::Empfangen)
                && s_client.fertig(dateien::Richtung::Senden)
                && s_client.fertig(dateien::Richtung::Empfangen)
        };
        while !fertig() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(10));
        }
        let dauer = t0.elapsed();
        println!("Host: {:?}", sperre(&s_host.0).last());
        println!("Client: {:?}", sperre(&s_client.0).last());
        assert!(fertig(), "nicht fertig nach {dauer:?}");
        println!("beide Richtungen fertig nach {dauer:?}");
        // Client -> Host: in der Basis der Host-Rolle, Byte fuer Byte.
        let beim_host = r_host.abgelegt();
        assert_eq!(beim_host.len(), 1, "{beim_host:?}");
        assert_eq!(beim_host[0].len(), 1);
        assert!(beim_host[0][0].starts_with(&u.basis), "{beim_host:?}");
        assert_eq!(beim_host[0][0].file_name(), quelle_c.file_name());
        assert_eq!(baum(&beim_host[0][0]), baum(&quelle_c), "Client -> Host");
        // Host -> Client.
        let beim_client = r_client.abgelegt();
        assert_eq!(beim_client.len(), 1, "{beim_client:?}");
        assert!(beim_client[0][0].starts_with(&client_basis));
        assert_eq!(beim_client[0][0].file_name(), quelle_h.file_name());
        assert_eq!(baum(&beim_client[0][0]), baum(&quelle_h), "Host -> Client");
        assert!(baum(&quelle_h).iter().any(|(p, n)| p.ends_with("gross.bin") && n.as_ref().map(|n| n.len()) == Some(700_000)));
        // Aufraeumen: Leitung zu, alle Faeden enden.
        l.schliessen();
        assert!(sperre(&l.dateien).sender.is_none());
        assert_eq!(endet_binnen(leseschleife, Duration::from_secs(5)), Some(Kanalende::Leitung));
        assert!(endet_binnen(sendefaden_griff, Duration::from_secs(5)).is_some(), "Sendefaden endet nicht");
        let gezaehlt = endet_binnen(bildleser, Duration::from_secs(5)).expect("Bildleser endet nicht");
        println!("beim Client angekommen: Angebote, Stuecke, Enden, Quittungen = {gezaehlt:?}");
        assert!(gezaehlt[1] > (700_000 / dateien::STUECK_MAX) as u32, "{gezaehlt:?}");
        for p in [&u.basis, &client_basis, quelle_c.parent().unwrap(), quelle_h.parent().unwrap()] {
            let _ = std::fs::remove_dir_all(p);
        }
    }

    /// Bindet Eingabekanal `einr` an `l` und laesst ihn Faehigkeiten OHNE
    /// Dateien melden (ein Client, der das Bit nicht setzt).
    fn unfaehig_machen(l: &Leitung, einr: u64) -> (TcpStream, TcpStream) {
        let (h, c) = tcp_paar();
        assert!(l.eingabe_binden(einr, h.try_clone().unwrap()));
        l.faehigkeiten_setzen(einr, 0);
        assert_eq!(l.lage(), Lage::KannNicht);
        (h, c)
    }

    /// Der Waechter: eine kopierte Dateiliste geht nur an einen Zuschauer,
    /// der Dateien gemeldet hat - hat er Faehigkeiten ohne Dateien gemeldet,
    /// einmal je Zuschauer die Zeile ZEILE_ZUSCHAUER_ZU_ALT. Neuer Inhalt
    /// (Dateien oder Text) bricht eine laufende Sendung ab, die dann Ende 1
    /// schickt - und zwar VOR dem Angebot der neuen (der neue Sender wartet
    /// in seinem Faden auf seinen Vorgaenger; ohne das stritten beide um den
    /// einen Datei-Platz, und das Ende konnte verhungern). Eine Liste aus der
    /// eigenen Ablage bricht nichts ab. Die Sender-Zeile nennt das Ziel.
    #[test]
    fn kopierte_dateien_nur_an_faehige_zuschauer_und_neuer_inhalt_bricht_ab() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s, z) = (Rekorder::default(), Staende::default(), Zeilen::default());
        let u = umgebung_mit_zeilen(&PLATZ, &ZAEHLER, "waechter", &r, &s, &z);
        let quelle = test_ordner("waechter-quelle").join("a.bin");
        std::fs::write(&quelle, vec![3u8; 200_000]).unwrap();
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::KeinZuschauer);
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.1".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        // Faehigkeiten ohne Dateien: nichts gesendet, die Zeile nur einmal.
        let _e1 = unfaehig_machen(&a, 1);
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::ZuAlt { gemeldet: true });
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::ZuAlt { gemeldet: false });
        assert_eq!(z.zahl(|t| t == dateien::ZEILE_ZUSCHAUER_ZU_ALT), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(sperre(&a.q).pakete.is_empty());
        let _e2 = faehig_machen(&a, 2);
        let Start::Gestartet(k1) = dateien_senden_mit(&u, vec![quelle.clone()]) else { panic!("nicht gestartet") };
        let v = abholen_bis(&a, Duration::from_secs(5), |p| p.1 == DATEI_ANGEBOT);
        assert!(v.iter().all(|p| p.0 == Art::Datei));
        // Neue Dateien: der erste Sender bricht ab, sein Ende 1 geht vor dem
        // Angebot des zweiten hinaus.
        let Start::Gestartet(k2) = dateien_senden_mit(&u, vec![quelle.clone()]) else { panic!("nicht gestartet") };
        assert_ne!(k1, k2);
        let v = abholen_bis(&a, Duration::from_secs(5), |p| p.1 == DATEI_ANGEBOT && dateien::kennung_lesen(&p.2) == Some(k2));
        let ende_k1 = v.iter().position(|p| ist_ende(p, k1, dateien::GRUND_ABGEBROCHEN));
        let angebot_k2 = v.iter().position(|p| p.1 == DATEI_ANGEBOT);
        assert!(
            matches!((ende_k1, angebot_k2), (Some(e), Some(an)) if e < an),
            "Ende(k1) {ende_k1:?} nicht vor Angebot(k2) {angebot_k2:?}"
        );
        // Neuer Text: auch der zweite.
        sendung_abbrechen_mit(&u);
        assert!(sperre(&a.dateien).sender.is_none());
        abholen_bis(&a, Duration::from_secs(5), |p| ist_ende(p, k2, dateien::GRUND_ABGEBROCHEN));
        // Eine Liste aus der eigenen Ablage: nichts gesendet, der laufende bleibt.
        let Start::Gestartet(k3) = dateien_senden_mit(&u, vec![quelle.clone()]) else { panic!("nicht gestartet") };
        let eigene = u.basis.join("1-1").join("x.txt");
        std::fs::create_dir_all(eigene.parent().unwrap()).unwrap();
        std::fs::write(&eigene, b"x").unwrap();
        assert_eq!(dateien_senden_mit(&u, vec![eigene]), Start::EigeneAblage);
        assert_eq!(sperre(&a.dateien).sender.as_ref().map(|g| g.kennung()), Some(k3));
        // Ein neuer Zuschauer: der Sender von A endet, und fuer B kommt die
        // Zeile wieder einmal.
        let b = Arc::new(Leitung::neu(None, vec![2], vec![2], "10.0.0.2".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, b.clone());
        assert!(sperre(&a.dateien).sender.is_none());
        let _e3 = unfaehig_machen(&b, 3);
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::ZuAlt { gemeldet: true });
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::ZuAlt { gemeldet: false });
        b.schliessen();
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Zu);
        // Die Sender-Zeile nennt das Ziel, die anderen bleiben.
        assert_eq!(
            zeile_mit_ziel("Dateien: sende 3 Eintraege, 12,4 MB".into(), "10.0.0.1"),
            "Dateien: sende 3 Eintraege, 12,4 MB an 10.0.0.1"
        );
        assert_eq!(
            zeile_mit_ziel("Dateien: gesendet und quittiert (12,4 MB in 1,8 s)".into(), "10.0.0.1"),
            "Dateien: gesendet und quittiert (12,4 MB in 1,8 s)"
        );
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Hinweis Phase B (M4): die Zeile des Senders im Protokoll der
    /// Host-Rolle nennt das Ziel, " an <ip>" - an der echten Anbindung
    /// (DateiUmgebung::melden aus sender_starten), nicht nur zeile_mit_ziel.
    #[test]
    fn senderzeile_nennt_das_ziel() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s, z) = (Rekorder::default(), Staende::default(), Zeilen::default());
        let u = umgebung_mit_zeilen(&PLATZ, &ZAEHLER, "senderzeile", &r, &s, &z);
        let quelle = test_ordner("senderzeile-quelle").join("a.bin");
        std::fs::write(&quelle, vec![3u8; 1000]).unwrap();
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.9".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let _e = faehig_machen(&a, 1);
        assert!(matches!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Gestartet(_)));
        abholen_bis(&a, Duration::from_secs(5), |p| p.1 == DATEI_ANGEBOT);
        let bis = Instant::now() + Duration::from_secs(5);
        while z.zahl(|t| t.starts_with("Dateien: sende ")) == 0 && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        let zeilen = z.alle();
        assert!(zeilen.iter().any(|t| t == "Dateien: sende 1 Eintraege, 0,0 MB an 10.0.0.9"), "{zeilen:?}");
        a.schliessen();
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Gesamtdurchsicht [8]: das Fenster Host -> Client ist fest hoechstens
    /// 64 KiB, auch ausser dem Spielmodus - ohne Quittung liegen nie mehr
    /// Dateidaten in Warteschlange, Kernel und Leitung.
    #[test]
    fn fenster_der_host_rolle_hoechstens_64_kib() {
        assert!(fenster_host(false) <= 65_536 && fenster_host(true) <= 65_536);
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = umgebung_test(&PLATZ, &ZAEHLER, "fenster-host", &r, &s);
        let quelle = test_ordner("fenster-host-quelle").join("gross.bin");
        std::fs::write(&quelle, vec![5u8; 1_000_000]).unwrap();
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.1".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let _e = faehig_machen(&a, 1);
        assert!(matches!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Gestartet(_)));
        // Wie ein Sendefaden, der alles sofort abnimmt; quittiert wird nicht.
        let bis = Instant::now() + Duration::from_millis(500);
        let mut daten = 0usize;
        while Instant::now() < bis {
            for p in abholen(&a) {
                if p.1 == DATEI_STUECK {
                    daten += p.2.len() - dateien::STUECK_KOPF;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(daten as u64, DATEI_FENSTER_HOST, "unquittiert hinaus: {daten}");
        a.schliessen();
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Gesamtdurchsicht [16]: der Sender der Host-Rolle prueft den Widerhall
    /// gegen IHRE Basis (Vorgaben::eigene_basis), nicht gegen die des
    /// Clients: eine Datei aus der Ablage des Clients auf demselben Rechner
    /// geht hinaus.
    #[test]
    fn host_sender_prueft_die_eigene_basis() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s, z) = (Rekorder::default(), Staende::default(), Zeilen::default());
        let u = umgebung_mit_zeilen(&PLATZ, &ZAEHLER, "eigene-basis", &r, &s, &z);
        assert_eq!((u.vorgaben.eigene_basis)(), host_ablage_basis());
        assert_eq!((DateiUmgebung::dienst().vorgaben.eigene_basis)(), host_ablage_basis());
        // Unter der Basis des Clients (im Test je Prozess), in einem Ordner,
        // den kein Aufraeumen anfasst.
        let vom_client = dateien::ablage_basis().join("n-kern-16").join("bericht.pdf");
        std::fs::create_dir_all(vom_client.parent().unwrap()).unwrap();
        std::fs::write(&vom_client, b"pdf").unwrap();
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.1".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let _e = faehig_machen(&a, 1);
        assert!(matches!(dateien_senden_mit(&u, vec![vom_client.clone()]), Start::Gestartet(_)));
        abholen_bis(&a, Duration::from_secs(5), |p| p.1 == DATEI_ANGEBOT);
        assert_eq!(z.zahl(|t| t.contains("aus einem Empfang")), 0, "{:?}", z.alle());
        // Ein Pfad unter der eigenen Basis (die Rolle prueft hier u.basis,
        // also erst der Kern): nichts gesendet. Der Kern prueft vor jedem
        // Zugriff auf die Platte, die Datei muss es dafuer nicht geben - so
        // stoert der Test das Aufraeumen der Basis in anderen Tests nicht.
        let eigen = host_ablage_basis().join("n-kern-16").join("x.txt");
        assert!(matches!(dateien_senden_mit(&u, vec![eigen]), Start::Gestartet(_)));
        let bis = Instant::now() + Duration::from_secs(5);
        while z.zahl(|t| t.contains("aus einem Empfang")) == 0 && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(z.zahl(|t| t.contains("aus einem Empfang")), 1, "{:?}", z.alle());
        a.schliessen();
        let _ = std::fs::remove_dir_all(vom_client.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Integrationstest Befund 3 (Host-Seite): kopiert der Nutzer Dateien,
    /// waehrend der aktuelle Eingabekanal noch keine Faehigkeiten gemeldet
    /// hat, wird die Liste vorgemerkt und geht hinaus, sobald IN_FAEHIGKEITEN
    /// ueber die echte Leseschleife kommt. Sonst faellt sie nach der Frist
    /// mit einer zutreffenden Zeile weg (nicht "aelterer Client"); neuer
    /// Text verwirft sie.
    #[test]
    fn vorgemerkte_liste_geht_nach_den_faehigkeiten_hinaus() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s, z) = (Rekorder::default(), Staende::default(), Zeilen::default());
        let mut u = umgebung_mit_zeilen(&PLATZ, &ZAEHLER, "vormerken", &r, &s, &z);
        u.vormerken = Duration::from_millis(400);
        let u = Arc::new(u);
        let quelle = test_ordner("vormerken-quelle").join("a.bin");
        std::fs::write(&quelle, vec![3u8; 1000]).unwrap();
        // 1. Eingabekanal steht, hat aber noch nichts gemeldet. Hier mit
        // langer Frist: das Angebot muss auf die Meldung hin kommen, nicht
        // erst mit dem Zeitgeber der Vormerkung.
        let u_lang = Arc::new(DateiUmgebung { vormerken: Duration::from_secs(3), ..(*u).clone() });
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.1".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let (mut h, mut c) = paar();
        assert!(a.eingabe_binden(1, h.abbruchgriff().unwrap()));
        let (l2, u2) = (a.clone(), u_lang.clone());
        let leser = std::thread::spawn(move || eingabe_lesen(&mut h, &l2, 1, &u2));
        assert_eq!(dateien_senden_mit(&u_lang, vec![quelle.clone()]), Start::Vorgemerkt);
        std::thread::sleep(Duration::from_millis(50));
        assert!(sperre(&a.q).pakete.is_empty());
        faehigkeiten_melden(&mut c).unwrap();
        let v = abholen_bis(&a, Duration::from_secs(1), |p| p.1 == DATEI_ANGEBOT);
        assert!(v.iter().all(|p| p.0 == Art::Datei));
        assert!(z.zahl(|t| t.contains("vorgemerkte Liste geht hinaus")) == 1, "{:?}", z.alle());
        assert_eq!(z.zahl(|t| t.contains("aelterer Client")), 0, "{:?}", z.alle());
        a.schliessen();
        assert_eq!(endet_binnen(leser, Duration::from_secs(5)), Some(Kanalende::Leitung));

        // 2. Neuer Text verwirft eine vorgemerkte Liste; ihr Zeitgeber
        // trifft danach nichts mehr.
        let b = Arc::new(Leitung::neu(None, vec![2], vec![2], "10.0.0.2".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, b.clone());
        let (h2, _c2) = tcp_paar();
        assert!(b.eingabe_binden(2, h2.try_clone().unwrap()));
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Vorgemerkt);
        sendung_abbrechen_mit(&u);
        b.faehigkeiten_setzen(2, FAEHIG_DATEIEN);
        b.vorgemerkte_pruefen(&u, None);
        std::thread::sleep(Duration::from_millis(100));
        assert!(sperre(&b.q).pakete.is_empty(), "nach neuem Text ging die Liste doch hinaus");
        assert!(sperre(&b.dateien).sender.is_none());
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(z.zahl(|t| t.starts_with("Dateien: nicht gesendet")), 0, "{:?}", z.alle());

        // 3. Keine Meldung binnen der Frist: die Zeile sagt, was war - hier
        // steht der Eingabekanal, hat aber nichts gemeldet.
        let c3 = Arc::new(Leitung::neu(None, vec![3], vec![3], "10.0.0.3".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, c3.clone());
        let (h3, _c3) = tcp_paar();
        assert!(c3.eingabe_binden(3, h3.try_clone().unwrap()));
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Vorgemerkt);
        let ohne_meldung = "Dateien: nicht gesendet, der Eingabekanal des Zuschauers hat in 0,4 s keine Faehigkeiten gemeldet";
        let bis = Instant::now() + Duration::from_secs(5);
        while z.zahl(|t| t == ohne_meldung) == 0 && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(z.zahl(|t| t == ohne_meldung), 1, "{:?}", z.alle());
        assert!(sperre(&c3.dateien).vorgemerkt.is_none());
        assert!(sperre(&c3.q).pakete.is_empty());

        // 4. Ohne Eingabekanal: "stand nach ... noch nicht".
        let d = Arc::new(Leitung::neu(None, vec![4], vec![4], "10.0.0.4".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, d.clone());
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Vorgemerkt);
        let kein_kanal = "Dateien: nicht gesendet, der Eingabekanal des Zuschauers stand nach 0,4 s noch nicht";
        let bis = Instant::now() + Duration::from_secs(5);
        while z.zahl(|t| t == kein_kanal) == 0 && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(z.zahl(|t| t == kein_kanal), 1, "{:?}", z.alle());

        // 5. Meldet der Kanal Faehigkeiten ohne Dateien, faellt die Liste
        // sofort weg, mit der Zeile fuer aeltere Clients.
        let e = Arc::new(Leitung::neu(None, vec![5], vec![5], "10.0.0.5".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, e.clone());
        let (h5, _c5) = tcp_paar();
        assert!(e.eingabe_binden(5, h5.try_clone().unwrap()));
        assert_eq!(dateien_senden_mit(&u, vec![quelle.clone()]), Start::Vorgemerkt);
        e.faehigkeiten_setzen(5, 0);
        e.vorgemerkte_pruefen(&u, None);
        assert!(sperre(&e.dateien).vorgemerkt.is_none());
        assert_eq!(z.zahl(|t| t == dateien::ZEILE_ZUSCHAUER_ZU_ALT), 1, "{:?}", z.alle());
        e.schliessen();
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Hinweis der Gegenpruefung: kommt IN_FAEHIGKEITEN, nachdem
    /// dateien_senden_mit die Lage (noch ohne Meldung) gelesen, die Liste
    /// aber noch nicht vorgemerkt hat, findet die Leseschleife nichts vor.
    /// Die Nachschau nach dem Vormerken schickt die Liste trotzdem sofort,
    /// nicht erst mit dem Zeitgeber (hier 5 s). Der Test haelt dafuer die
    /// Dateisperre, bis die Meldung eingetragen ist; die Leseschleife
    /// (vorgemerkte_pruefen nach IN_FAEHIGKEITEN) kaeme in diesem Fenster zu
    /// frueh und wird deshalb gar nicht gerufen.
    #[test]
    fn meldung_zwischen_lage_und_vormerken_geht_sofort_hinaus() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s, z) = (Rekorder::default(), Staende::default(), Zeilen::default());
        let mut u = umgebung_mit_zeilen(&PLATZ, &ZAEHLER, "vormerken-nachschau", &r, &s, &z);
        u.vormerken = Duration::from_secs(5);
        let u = Arc::new(u);
        let quelle = test_ordner("vormerken-nachschau-quelle").join("a.bin");
        std::fs::write(&quelle, vec![3u8; 1000]).unwrap();
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "10.0.0.1".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        let (h, _c) = tcp_paar();
        assert!(a.eingabe_binden(1, h.try_clone().unwrap()));
        assert_eq!(a.lage(), Lage::Offen { kanal: true });
        let dateisperre = sperre(&a.dateien);
        let (u2, q2) = (u.clone(), quelle.clone());
        let kopieren = std::thread::spawn(move || dateien_senden_mit(&u2, vec![q2]));
        // dateien_senden_mit liest die Lage (Offen) und wartet dann auf die
        // Dateisperre.
        std::thread::sleep(Duration::from_millis(200));
        a.faehigkeiten_setzen(1, FAEHIG_DATEIEN);
        let t0 = Instant::now();
        drop(dateisperre);
        assert_eq!(
            kopieren.join().unwrap(),
            Start::Vorgemerkt,
            "die Lage wurde erst nach der Meldung gelesen - Probe ohne Wert"
        );
        let v = abholen_bis(&a, Duration::from_secs(10), |p| p.1 == DATEI_ANGEBOT);
        assert!(t0.elapsed() < Duration::from_secs(2), "Angebot erst nach {:?} (Zeitgeber)", t0.elapsed());
        assert!(v.iter().all(|p| p.0 == Art::Datei));
        assert_eq!(z.zahl(|t| t.contains("vorgemerkte Liste geht hinaus")), 1, "{:?}", z.alle());
        a.schliessen();
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Hinweis Phase B (M3): Datei-Nachrichten eines ueberholten
    /// Eingabekanals gehen nicht mehr an Empfaenger oder Sender. Der alte
    /// Leser hat sein Angebot bzw. seine Quittung schon gelesen und wartet
    /// auf EINSPEISEN, als ein neuer Kanal gebunden wird; danach endet er mit
    /// Abgeloest, ohne weiterzugeben.
    #[test]
    fn ueberholter_eingabekanal_gibt_keine_datei_nachrichten_weiter() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = Arc::new(umgebung_test(&PLATZ, &ZAEHLER, "ueberholt", &r, &s));
        let angebot = dateien::Angebot {
            kennung: 7,
            gesamt: 5,
            eintraege: vec![dateien::Eintrag { art: dateien::EintragArt::Datei, pfad: "a.txt".into(), groesse: 5 }],
        }
        .kodieren();
        let l = Arc::new(Leitung::neu(None, vec![1], vec![1], "C".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, l.clone());
        // Ein laufender Sender Host -> Client, dessen Quittung gleich ueber
        // den alten Kanal kommt.
        let _e0 = faehig_machen(&l, 10);
        let quelle = test_ordner("ueberholt-quelle").join("a.bin");
        std::fs::write(&quelle, vec![3u8; 1000]).unwrap();
        let Start::Gestartet(k) = dateien_senden_mit(&u, vec![quelle.clone()]) else { panic!("nicht gestartet") };
        abholen_bis(&l, Duration::from_secs(5), |p| p.1 == DATEI_ENDE);
        let abbruch = dateien::Quittung { kennung: k, zustand: dateien::ZUSTAND_ABGEBROCHEN, empfangen: 0 }.kodieren();
        for (runde, nachricht) in [(1u64, (DATEI_ANGEBOT, angebot.clone())), (3, (DATEI_QUITTUNG, abbruch))] {
            let (mut h, mut c) = paar();
            assert!(l.eingabe_binden(runde, h.abbruchgriff().unwrap()));
            let (l2, u2) = (l.clone(), u.clone());
            let leser = std::thread::spawn(move || eingabe_lesen(&mut h, &l2, runde, &u2));
            let einspeisen = sperre(&EINSPEISEN);
            let mut m = kopf(nachricht.0, 0, 0, nachricht.1.len()).to_vec();
            m.extend_from_slice(&nachricht.1);
            c.write_all(&m).unwrap();
            if nachricht.0 == DATEI_ANGEBOT {
                // Das Angebot ist gelesen, sobald es den Empfaenger gibt (vor
                // EINSPEISEN angelegt).
                let bis = Instant::now() + Duration::from_secs(5);
                while sperre(&l.dateien).empfaenger.is_none() && Instant::now() < bis {
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert!(sperre(&l.dateien).empfaenger.is_some());
            } else {
                std::thread::sleep(Duration::from_millis(300));
            }
            // Ein neuer Kanal desselben Zuschauers ueberholt den alten.
            let (h_neu, _c_neu) = tcp_paar();
            assert!(l.eingabe_binden(runde + 1, h_neu.try_clone().unwrap()));
            drop(einspeisen);
            assert_eq!(endet_binnen(leser, Duration::from_secs(5)), Some(Kanalende::Abgeloest));
            std::thread::sleep(Duration::from_millis(300));
            let quittungen = sperre(&l.q).pakete.iter().filter(|(_, p)| p[0] == DATEI_QUITTUNG).count();
            assert_eq!(quittungen, 0, "das Angebot des ueberholten Kanals wurde angenommen");
            let laeuft = sperre(&l.dateien).sender.as_ref().is_some_and(|g| g.laeuft());
            assert!(laeuft, "die Quittung des ueberholten Kanals beendete den Sender");
        }
        l.schliessen();
        let _ = std::fs::remove_dir_all(quelle.parent().unwrap());
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    /// Hinweis Phase B (M5): die Quittungen eines Empfaengers gehen nur an
    /// SEINEN Zuschauer (nummerngebunden), auch im Wettlauffenster, in dem
    /// schon ein neuer Zuschauer eingetragen, der alte aber noch nicht
    /// abgeloest ist.
    #[test]
    fn quittungen_des_empfaengers_nur_an_seinen_zuschauer() {
        static PLATZ: Mutex<Option<Arc<Leitung>>> = Mutex::new(None);
        static ZAEHLER: AtomicU64 = AtomicU64::new(0);
        let (r, s) = (Rekorder::default(), Staende::default());
        let u = umgebung_test(&PLATZ, &ZAEHLER, "quittung-nummer", &r, &s);
        let a = Arc::new(Leitung::neu(None, vec![1], vec![1], "A".into()));
        zuschauer_eintragen(&PLATZ, &ZAEHLER, a.clone());
        a.empfaenger_bereit(&u);
        // B wird eingetragen, ohne dass A abgeloest ist.
        let b = Arc::new(Leitung::neu(None, vec![2], vec![2], "B".into()));
        {
            let mut p = sperre(&PLATZ);
            let nr = ZAEHLER.fetch_add(1, Ordering::Relaxed) + 1;
            b.nr.store(nr, Ordering::Relaxed);
            *p = Some(b.clone());
        }
        let angebot = dateien::Angebot {
            kennung: 7,
            gesamt: 5,
            eintraege: vec![dateien::Eintrag { art: dateien::EintragArt::Datei, pfad: "a.txt".into(), groesse: 5 }],
        };
        a.datei_eingang(DATEI_ANGEBOT, angebot.kodieren());
        // Der Empfaenger von A nimmt an und quittiert - sein Weg meldet Weg,
        // er verwirft (Abbruch Verbindung). Bei B kommt nichts an.
        let bis = Instant::now() + Duration::from_secs(5);
        let verbindung = || {
            sperre(&s.0).iter().any(|st| st.ergebnis == dateien::Ergebnis::Abgebrochen(dateien::Abbruch::Verbindung))
        };
        while !verbindung() && Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(verbindung(), "{:?}", sperre(&s.0));
        std::thread::sleep(Duration::from_millis(100));
        for (wer, l) in [("A", &a), ("B", &b)] {
            let n = sperre(&l.q).pakete.iter().filter(|(_, p)| p[0] == DATEI_QUITTUNG).count();
            assert_eq!(n, 0, "Quittung bei {wer}");
        }
        a.schliessen();
        b.schliessen();
        let _ = std::fs::remove_dir_all(&u.basis);
    }

    // ------------------------------------------------- Starten und Stoppen

    fn loopback() -> Vec<Ipv4Addr> {
        vec![Ipv4Addr::LOCALHOST]
    }

    /// Den Zuschauerplatz auf Loopback starten, an freien Ports P und P+1;
    /// die Bekanntgabe geht an 127.0.0.1:P+2, das der Test selbst belegt.
    fn start_auf_loopback(schluessel: &[u8]) -> (u16, UdpSocket) {
        for p in testport_kandidaten() {
            let Ok(udp) = UdpSocket::bind(("127.0.0.1", p + 2)) else { continue };
            if start_mit(Ipv4Addr::LOCALHOST, p, schluessel.to_vec(), loopback).is_ok() {
                return (p, udp);
            }
        }
        panic!("keine freien Ports auf Loopback");
    }

    /// Die Testports liegen samt P+1 und P+2 unterhalb aller Bereiche fuer
    /// kurzlebige Ports und sind gestreut, nicht der Reihe nach.
    #[test]
    fn testports_ausserhalb_kurzlebiger_bereiche() {
        let alle: Vec<u16> = testport_kandidaten().collect();
        assert_eq!(alle.len(), 200);
        assert!(alle.iter().all(|&p| (20000..32000).contains(&p) && p + 2 < 32000), "{alle:?}");
        let verschieden: std::collections::HashSet<_> = alle.iter().collect();
        assert!(verschieden.len() > 150, "kaum gestreut: {alle:?}");
        assert!(alle.windows(2).any(|w| w[1] != w[0] + 1), "der Reihe nach: {alle:?}");
    }

    /// Handschlag bis Nachricht 2: antwortet der Host mit diesem Schluessel?
    /// Danach bricht der Test ab (keine Nachricht 3) - der Einlass sieht die
    /// Verbindung nie, ein laufender Zuschauer anderer Tests auch nicht.
    fn host_antwortet(port: u16, host_pub: &[u8]) -> Result<(), String> {
        const ABBRUCH: &str = "Test: nach Nachricht 2 abgebrochen";
        let gesehen = std::cell::RefCell::new(None);
        let r = secure::Secure::connect_pruefend(&format!("127.0.0.1:{port}"), &noise::prologue_video(), |k| {
            *gesehen.borrow_mut() = Some(k.to_vec());
            Err(secure::Fehler::Handschlag { grund: ABBRUCH.into(), frist: false, system: None })
        });
        match (r, gesehen.into_inner()) {
            (Err(secure::Fehler::Handschlag { grund, .. }), Some(k)) if grund == ABBRUCH && k == host_pub => Ok(()),
            (Err(e), k) => Err(format!("{e} (Schluessel gesehen: {:?})", k.map(|k| k == host_pub))),
            (Ok(_), _) => Err("Handschlag trotz Abbruch fertig".into()),
        }
    }

    /// Weist der Port Verbindungen ab? (Windows versucht es bei einem
    /// geschlossenen Port auf Loopback rund 2 s lang, daher die lange Frist.)
    fn abgewiesen(port: u16) -> Result<(), String> {
        let ziel = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match TcpStream::connect_timeout(&ziel, Duration::from_secs(6)) {
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
            Err(e) => Err(format!("Port {port}: {e}")),
            Ok(_) => Err(format!("Port {port} nimmt noch an")),
        }
    }

    /// Kommt binnen `frist` eine Bekanntgabe fuer `port`?
    fn bekanntgabe_kommt(udp: &UdpSocket, port: u16, frist: Duration) -> bool {
        let bis = Instant::now() + frist;
        let mut b = [0u8; 256];
        while Instant::now() < bis {
            udp.set_read_timeout(Some(bis.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)))).unwrap();
            match udp.recv_from(&mut b) {
                Ok((n, _)) if zugang::bekanntgabe_lesen(&b[..n]).is_some_and(|g| g.port == port) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    /// Die naechste Bekanntgabe fuer `port` binnen `frist`, gelesen.
    fn naechste_bekanntgabe(udp: &UdpSocket, port: u16, frist: Duration) -> Option<zugang::Bekanntgabe> {
        let bis = Instant::now() + frist;
        let mut b = [0u8; 256];
        while Instant::now() < bis {
            udp.set_read_timeout(Some(bis.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)))).unwrap();
            match udp.recv_from(&mut b) {
                Ok((n, _)) => match zugang::bekanntgabe_lesen(&b[..n]) {
                    Some(g) if g.port == port => return Some(g),
                    _ => {}
                },
                Err(_) => return None,
            }
        }
        None
    }

    /// Der Name in der Bekanntgabe ist der eingestellte Geraetename, und ein
    /// neuer gilt ab der naechsten Runde - ohne Neustart des Zuschauerplatzes.
    #[test]
    fn bekanntgabe_mit_eingestelltem_namen() {
        let _platz = platz_pruefung();
        let _name = zugang::name_test_sperre();
        let vorher = zugang::geraetename_eingestellt();
        zugang::geraetename_setzen(Some("Arbeitszimmer".into()));
        let (host_priv, _) = noise::keypair().unwrap();
        let (port, udp) = start_auf_loopback(&host_priv);
        let g = naechste_bekanntgabe(&udp, port, Duration::from_secs(3)).expect("keine Bekanntgabe");
        assert_eq!(g.name, "Arbeitszimmer");
        zugang::geraetename_setzen(Some("Küche".into()));
        // Was schon unterwegs war, zaehlt nicht; binnen zwei Runden der neue.
        let mut neu = None;
        let bis = Instant::now() + Duration::from_secs(6);
        while Instant::now() < bis {
            match naechste_bekanntgabe(&udp, port, Duration::from_secs(3)) {
                Some(g) if g.name == "Küche" => {
                    neu = Some(g);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(neu.is_some(), "neuer Name nicht in der Bekanntgabe");
        zugang::geraetename_setzen(None);
        let bis = Instant::now() + Duration::from_secs(6);
        let mut system = false;
        while Instant::now() < bis && !system {
            system = naechste_bekanntgabe(&udp, port, Duration::from_secs(3)).is_some_and(|g| g.name == zugang::rechnername());
        }
        assert!(system, "ohne Einstellung nicht der Rechnername");
        assert!(stoppen());
        zugang::geraetename_setzen(vorher);
    }

    /// W2: starten -> verbinden -> stoppen -> abgewiesen -> starten ->
    /// verbinden, auf Loopback. Gestoppt sind beide Ports zu und die
    /// Bekanntgabe still, sobald `stoppen` zurueckkehrt; ein zweiter Start
    /// waehrend eines Laufs scheitert, ein zweites Stoppen tut nichts.
    #[test]
    fn starten_stoppen_starten() {
        let _platz = platz_pruefung();
        let (host_priv, host_pub) = noise::keypair().unwrap();
        let (port, udp) = start_auf_loopback(&host_priv);
        assert!(laeuft());
        assert!(start_mit(Ipv4Addr::LOCALHOST, port, host_priv.clone(), loopback).is_err(), "zweiter Start waehrend eines Laufs");
        host_antwortet(port, &host_pub).expect("erster Lauf: Bildkanal");
        TcpStream::connect(("127.0.0.1", port + 1)).expect("erster Lauf: Eingabekanal");
        assert!(bekanntgabe_kommt(&udp, port, Duration::from_secs(3)), "erster Lauf: keine Bekanntgabe");

        let t0 = Instant::now();
        assert!(stoppen());
        assert!(t0.elapsed() < Duration::from_secs(2), "Stoppen dauerte {:?}", t0.elapsed());
        assert!(!laeuft());
        assert!(!stoppen(), "zweites Stoppen");
        abgewiesen(port).expect("gestoppt: Bildkanal");
        abgewiesen(port + 1).expect("gestoppt: Eingabekanal");
        // Was schon unterwegs war, abholen; danach bleibt es still.
        udp.set_nonblocking(true).unwrap();
        let mut b = [0u8; 256];
        while udp.recv_from(&mut b).is_ok() {}
        udp.set_nonblocking(false).unwrap();
        assert!(!bekanntgabe_kommt(&udp, port, Duration::from_millis(2500)), "gestoppt: Bekanntgabe laeuft weiter");

        start_mit(Ipv4Addr::LOCALHOST, port, host_priv.clone(), loopback).expect("zweiter Lauf auf demselben Port");
        host_antwortet(port, &host_pub).expect("zweiter Lauf: Bildkanal");
        TcpStream::connect(("127.0.0.1", port + 1)).expect("zweiter Lauf: Eingabekanal");
        assert!(bekanntgabe_kommt(&udp, port, Duration::from_secs(3)), "zweiter Lauf: keine Bekanntgabe");
        assert!(stoppen());
        abgewiesen(port).expect("wieder gestoppt: Bildkanal");
    }

    /// Wer seinen Handschlag erst nach dem Stoppen beendet, wird kein
    /// Zuschauer: die Marke gilt nicht mehr (Pruefung unter AKTUELL in
    /// bild_annehmen). Hier die Marke selbst - sie wird unter AKTUELL
    /// beendet, weckt eine laufende Pause sofort und gilt danach nie wieder.
    #[test]
    fn laufmarke_beenden_weckt_und_gilt_nicht_mehr() {
        let marke = Laufmarke::neu(Vec::new());
        assert!(marke.gilt());
        assert!(!marke.pause(Duration::from_millis(20)), "Pause ohne Ende meldet beendet");
        let m2 = marke.clone();
        let t0 = Instant::now();
        let schlaefer = std::thread::spawn(move || m2.pause(Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(100));
        marke.beenden();
        assert!(schlaefer.join().unwrap(), "Pause nicht als beendet gemeldet");
        assert!(t0.elapsed() < Duration::from_secs(2), "Pause nicht geweckt: {:?}", t0.elapsed());
        assert!(!marke.gilt());
        assert!(marke.pause(Duration::from_secs(10)), "beendete Marke wartet");
    }
}
