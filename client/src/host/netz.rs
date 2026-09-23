// Zuschauerplatz des Windows-Hosts: Annahme auf dem Bildkanal (port) und
// dem Eingabekanal (port + 1), Bekanntgabe per Rundruf (port + 2), Versand
// mit Stauregel, Kopplung.
//
// Ein Zuschauer zur Zeit; ein neuer ersetzt den alten (wie main.m). Der
// alte bekommt als letzte Nachricht MSG_ABGELOEST - statt dessen, was noch
// in seiner Warteschlange lag -, damit er sich nicht von selbst neu
// verbindet und den neuen verdraengt; danach sind seine Bildleitung und sein
// Eingabekanal zu. Vor dem Schlusswort geht noch der Rest des Pakets hinaus,
// das der Sendefaden gerade schreibt: solange der Alte davon abnimmt, wird
// gewartet (hoechstens ABLOESUNG_HOECHSTENS), erst ohne Fortschritt gekappt.
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
// mehr aus. Was jede Verbindung ins Protokoll bringen kann, ohne gekoppelt
// zu sein (gescheiterter Handschlag, Abweisung), laeuft ueber eine Drossel.
// Windows bricht ein blockierendes recv/send auf ein shutdown hin nicht ab,
// solange die Gegenstelle lebt, aber schweigt (eingefrorener Prozess) - der
// Faden des alten Kanals bliebe samt Leitung stehen, bis sie geht. Deshalb
// bricht `kappen` zusaetzlich mit CancelIoEx ab, was an der Leitung haengt.
// Die Eingabeschleife prueft ohnehin vor jedem Einspeisen, ob sie noch gilt.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::{eingabe, encoder, log, now_us, Z};
use crate::protokoll_konst::*;
use crate::{noise, secure};

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
/// So lange darf die letzte Nachricht an einen abgeloesten Zuschauer
/// brauchen. Seine Warteschlange ist da schon verworfen; vor ihr liegen
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
/// den Zuschauer warten duerfen: zwei volle Zwischenablagen. Sie werden nie
/// still verworfen - sonst bricht etwa die Reihenfolge Switch -> Info ->
/// Vollbild. Wer so weit zurueckliegt, ist weg; ohne diese Grenze liesse ein
/// Zuschauer, der Zeitfragen stellt und kaum liest, den Speicher des Hosts
/// ohne Ende wachsen.
const KLEIN_GRENZE: usize = 2 * HARTE_GRENZE;

/// Sperre nehmen, auch wenn ein anderer Faden unter ihr in Panik geraten
/// ist. Die Sperren hier schuetzen Abfolgen (EINSPEISEN, FREIGABE) oder Daten,
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

/// Hoechstens alle zehn Sekunden eine Protokollzeile derselben Art: wer
/// einen Port mit Verbindungen bestreicht, soll das Protokoll nicht fuellen.
fn melden_erlaubt(letzte: &Mutex<Option<Instant>>) -> bool {
    let mut t = sperre(letzte);
    if t.map(|t| t.elapsed() < Duration::from_secs(10)).unwrap_or(false) {
        return false;
    }
    *t = Some(Instant::now());
    true
}

/// Drossel fuer eine Art Protokollzeile, die jede Verbindung ausloesen kann -
/// auch eine ohne Schluessel oder von einer ungekoppelten Gegenstelle:
/// hoechstens eine Zeile alle zehn Sekunden (melden_erlaubt); die
/// unterdrueckten dazwischen werden gezaehlt und stehen an der naechsten.
/// Wer einen Port mit Verbindungen bestreicht oder ungekoppelt alle 2 s neu
/// versucht, fuellt so weder Protokoll noch Platte. Ereignisse gekoppelter
/// Gegenstellen (Kopplung, Zuschauer verbunden) laufen nie hierueber.
struct Drossel {
    letzte: Mutex<Option<Instant>>,
    unterdrueckt: AtomicU64,
}

impl Drossel {
    const fn neu() -> Drossel {
        Drossel { letzte: Mutex::new(None), unterdrueckt: AtomicU64::new(0) }
    }

    /// Die Zeile ins Protokoll, wenn die Drossel sie durchlaesst - dann mit
    /// der Zahl der seit der letzten unterdrueckten -, sonst nur zaehlen. Der
    /// Text entsteht nur, wenn er geschrieben wird. Liefert die geschriebene
    /// Zeile.
    fn melden(&self, text: impl FnOnce() -> String) -> Option<String> {
        if !melden_erlaubt(&self.letzte) {
            self.unterdrueckt.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let mut zeile = text();
        let n = self.unterdrueckt.swap(0, Ordering::Relaxed);
        if n > 0 {
            zeile.push_str(&format!(" (und {n} weitere seit der letzten Meldung)"));
        }
        log(&zeile);
        Some(zeile)
    }
}

/// Gescheiterte Handschlaege auf dem Bildkanal (Muell, Frist, falsches Protokoll).
static DROSSEL_HANDSCHLAG_BILD: Drossel = Drossel::neu();
/// Unbekannte Gegenstellen ohne Kopplungsfenster (etwa ein ungekoppelter
/// Client, der alle 2 s neu versucht).
static DROSSEL_UNBEKANNT: Drossel = Drossel::neu();
/// Abweisungen, weil die Freigabeliste nicht zu lesen oder nicht zu
/// schreiben ist - auch die kann jede Gegenstelle ausloesen.
static DROSSEL_FREIGABE: Drossel = Drossel::neu();
/// Gescheiterte Handschlaege auf dem Eingabekanal.
static DROSSEL_HANDSCHLAG_EINGABE: Drossel = Drossel::neu();
/// Eingabekanaele, die nach dem Handschlag abgewiesen werden.
static DROSSEL_EINGABE_ABGEWIESEN: Drossel = Drossel::neu();

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
}

struct Warteschlange {
    pakete: VecDeque<(Art, Vec<u8>)>,
    bytes: usize,
    /// Davon Ton bzw. Steuernachrichten.
    ton: usize,
    klein: usize,
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
    /// Letzte Nachricht an einen abgeloesten Zuschauer (MSG_ABGELOEST). Der
    /// Sendefaden schreibt sie statt der verworfenen Warteschlange, mit
    /// Frist, und macht danach die Leitung zu.
    schlusswort: Option<Vec<u8>>,
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
    }

    /// Das vorderste Paket herausnehmen, samt Buchfuehrung je Art.
    fn nehmen(&mut self) -> Option<Vec<u8>> {
        let (art, p) = self.pakete.pop_front()?;
        self.bytes -= p.len();
        match art {
            Art::Ton => self.ton -= p.len(),
            Art::Klein => self.klein -= p.len(),
            Art::Bild => {}
        }
        Some(p)
    }
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
}

impl Leitung {
    fn neu(bild_griff: Option<TcpStream>, peer: Vec<u8>, hh: Vec<u8>, ip: String) -> Leitung {
        Leitung {
            q: Mutex::new(Warteschlange {
                pakete: VecDeque::new(),
                bytes: 0,
                ton: 0,
                klein: 0,
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
            ip,
        }
    }

    /// Zuschauer beenden: Warteschlange zu, Bildleitung und Eingabekanal
    /// gekappt. Kein Bild, keine Eingabe - ein abgeloester Zuschauer behaelt
    /// nichts, auch wenn er seine Leitungen selbst offen hielte.
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
    }

    fn bild_kappen(&self) {
        if let Some(s) = &self.bild_griff {
            kappen(s);
        }
    }

    /// Ein neuer Zuschauer ersetzt diesen. Was noch in der Warteschlange
    /// liegt, wird verworfen; hinaus geht nur noch MSG_ABGELOEST - danach
    /// schliesst der Sendefaden die Bildleitung selbst. Nur die Begruessung
    /// (MAGIC + Strominfo) bleibt, wenn der Sendefaden noch gar nichts
    /// genommen hat: sie geht mit dem Schlusswort hinaus. Ohne sie laese der
    /// Client das Schlusswort als MAGIC, hielte es fuer ein fremdes
    /// Protokoll und verbaende sich neu - und verdraengte den Neuen. Der
    /// Eingabekanal ist sofort gekappt. Haengt der Sendefaden an einem
    /// Zuschauer, der nichts mehr abnimmt, kappt `abloesung_abschliessen` die
    /// Bildleitung. Unter EINSPEISEN und AKTUELL aufrufen; wartet nie.
    fn abloesen(&self) {
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
                wort.extend_from_slice(&kopf(MSG_ABGELOEST, 0, 0, 0));
                q.leeren();
                q.schlusswort = Some(wort);
                q.offen = false;
                self.cv.notify_all();
            }
            q.eingabe.take()
        };
        if let Some((_, s)) = eingabe {
            kappen(&s);
            log("Eingabekanal gekappt: sein Zuschauer ist abgeloest oder weg");
        }
    }

    /// Nach `abloesen`, ausserhalb aller Sperren: auf das Ende des
    /// Sendefadens warten, solange er binnen `frist` Fortschritt macht
    /// (`geschrieben` waechst - dasselbe Mass wie in der Stauregel), aber
    /// hoechstens ABLOESUNG_HOECHSTENS; sonst die Bildleitung kappen, auch
    /// wenn er noch in einem write steckt. `frist` muss laenger sein als
    /// FRIST_SCHLUSSWORT: waehrend des Schlussworts waechst `geschrieben`
    /// nicht. true: der Sendefaden ist durch.
    fn abloesung_abschliessen(&self, frist: Duration) -> bool {
        let beginn = Instant::now();
        let mut q = sperre(&self.q);
        loop {
            let stand = q.geschrieben;
            let rest = ABLOESUNG_HOECHSTENS.saturating_sub(beginn.elapsed());
            q = self.ende.wait_timeout_while(q, frist.min(rest), |q| !q.beendet).unwrap_or_else(|e| e.into_inner()).0;
            if q.beendet || q.geschrieben == stand || beginn.elapsed() >= ABLOESUNG_HOECHSTENS {
                break;
            }
        }
        let beendet = q.beendet;
        drop(q);
        if !beendet {
            log(format!(
                "Abgeloester Zuschauer {} nimmt nichts mehr ab (nach {:.1} s) - Bildleitung gekappt",
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

    /// Ein Paket einreihen. Mit Grenze: nur, wenn der Stand nicht schon
    /// darueber liegt - ein Paket darf sie also um sich selbst
    /// ueberschreiten, auch ein Vollbild, das allein groesser ist. Der Stand
    /// ist fuer ein Bild der ganze Rueckstand, fuer Ton der wartende Ton, fuer
    /// alles andere die wartenden Steuernachrichten.
    fn einreihen(&self, paket: Vec<u8>, art: Art, grenze: Option<usize>) -> Result<(), Abgewiesen> {
        let mut q = sperre(&self.q);
        if !q.offen {
            return Err(Abgewiesen::Zu);
        }
        if let Some(g) = grenze {
            let stand = match art {
                Art::Bild => q.bytes + q.im_schreiben,
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
                DROSSEL_TON.melden(|| format!("Leitung langsamer als der Ton: Tonpaket verworfen (ueber {} kB Ton wartet)", grenze / 1024));
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
static PRIV: OnceLock<Vec<u8>> = OnceLock::new();
static SEQ: AtomicU32 = AtomicU32::new(0);
/// Laufende Nummer der Zuschauer, fuer das Protokoll.
static NR: AtomicU64 = AtomicU64::new(0);
/// Laufende Nummer der Eingabekanaele (siehe Leitung::eingabe_loesen).
static EINGABE_NR: AtomicU64 = AtomicU64::new(0);
/// Entscheidung ueber die Freigabe samt Eintrag: seit die Handschlaege
/// nebeneinander laufen, kaemen sonst zwei Unbekannte zugleich durch das
/// eine Kopplungsfenster (oder den Erstkontakt).
static FREIGABE: Mutex<()> = Mutex::new(());
/// Einspeisen und Abloesen schliessen sich aus: Die Eingabeschleife prueft
/// unter dieser Sperre, ob ihr Kanal noch gilt, und fuehrt die Nachricht
/// aus; wer einen Zuschauer oder Eingabekanal abloest und danach alle Tasten
/// loslaesst, tut das ebenfalls darunter. Sonst koennte eine Taste des Alten
/// genau zwischen Pruefung und Loslassen gedrueckt werden und haengen
/// bleiben. Reihenfolge: erst EINSPEISEN, dann AKTUELL.
static EINSPEISEN: Mutex<()> = Mutex::new(());

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

static PLAETZE_BILD: Plaetze = Plaetze::neu();
static PLAETZE_EINGABE: Plaetze = Plaetze::neu();
/// "Kein Bildkanal offen" auf dem Eingabeport.
static DROSSEL_KEIN_BILD: Drossel = Drossel::neu();
/// Codierte Bilder, die an der harten Grenze wegfallen.
static DROSSEL_HART: Drossel = Drossel::neu();
/// Tonpakete, die an ton_grenze wegfallen.
static DROSSEL_TON: Drossel = Drossel::neu();

/// Ein belegter Platz; gibt sich beim Wegfallen selbst frei.
struct Platz {
    p: &'static Plaetze,
    ip: IpAddr,
}

impl Plaetze {
    const fn neu() -> Plaetze {
        Plaetze { laufend: Mutex::new(Vec::new()), gemeldet: Drossel::neu() }
    }

    fn belegen(&'static self, ip: IpAddr, kanal: &str) -> Option<Platz> {
        let mut l = sperre(&self.laufend);
        let je_absender = l.iter().filter(|x| **x == ip).count();
        if l.len() >= HANDSCHLAEGE_MAX || je_absender >= HANDSCHLAEGE_JE_ABSENDER {
            self.gemeldet.melden(|| {
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
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&kopf(typ, 0, 0, data.len()));
    p.extend_from_slice(data);
    let _ = l.klein_senden(typ, p, Z.gaming.load(Ordering::Relaxed), now_us());
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

pub fn hoststatus_senden(lage: u8) {
    send_small(MSG_HOSTSTATUS, &[lage, 0]);
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
            DROSSEL_HART.melden(|| {
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
                if let Some(s) = schluss {
                    schlusswort_senden(&mut sock, &s, &l.ip);
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

/// Die letzte Nachricht an einen abgeloesten Zuschauer, mit Frist: wer sie
/// nicht abnimmt, haelt den Faden nicht fest. Danach ist die Leitung zu -
/// das Schlusswort liegt dann schon im Kernel und geht vor dem FIN hinaus.
fn schlusswort_senden(sock: &mut secure::Secure, s: &[u8], ip: &str) {
    sock.socket().set_write_timeout(Some(FRIST_SCHLUSSWORT)).ok();
    match sock.write_all(s) {
        Ok(()) => log(format!("Abloesung an {ip} gemeldet")),
        Err(e) => log(format!("Abloesung an {ip} nicht zugestellt: {e}")),
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

fn annahme_bild(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // Der Handschlag laeuft in einem eigenen Faden: wer ihn nicht zu
        // Ende bringt, haelt nur seinen Platz, nicht die Annahme.
        let Some(platz) = PLAETZE_BILD.belegen(absender(&stream), "Bildkanal") else { continue };
        std::thread::spawn(move || bild_annehmen(stream, platz));
    }
}

fn bild_annehmen(stream: TcpStream, platz: Platz) {
    let ip = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let port = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
    sendepuffer_setzen(&stream);

    // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
    let sock = match secure::Secure::accept(stream, &noise::prologue_video(), PRIV.get().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            DROSSEL_HANDSCHLAG_BILD.melden(|| format!("Handschlag mit {ip} gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    let fp = sock.peer_fingerprint();
    let sas = sock.sas.clone();

    // Freigabe: bekannte Gegenstelle, oder das Kopplungsfenster steht
    // offen, oder es gibt authorized.txt noch gar nicht (Erstkontakt). Eine
    // vorhandene, aber unlesbare Liste ist KEIN Erstkontakt, ebenso wenig
    // eine vorhandene ohne gueltigen Eintrag (secure::Freigaben::erstkontakt)
    // - dann wird abgewiesen, ebenso wenn sich die neue Freigabe nicht
    // speichern laesst. Die Liste wird dafuer genau einmal gelesen; Pruefen,
    // Erstkontakt und Eintragen entscheiden auf demselben Stand.
    {
        let _freigabe = sperre(&FREIGABE);
        let liste = match secure::Freigaben::lesen() {
            Ok(l) => l,
            Err(e) => {
                DROSSEL_FREIGABE.melden(|| format!("Abgewiesen: Gegenstelle {fp} von {ip} - {e}"));
                return;
            }
        };
        if !liste.enthaelt(&sock.peer) {
            let aufnehmen = if Z.pair_open.load(Ordering::Relaxed) { Ok(true) } else { liste.erstkontakt() };
            match aufnehmen {
                Ok(true) => {
                    if let Err(e) = liste.aufnehmen(&sock.peer, &ip) {
                        DROSSEL_FREIGABE.melden(|| format!("Abgewiesen: Freigabe fuer {fp} ({ip}) konnte nicht gespeichert werden: {e}"));
                        return;
                    }
                    log(format!("Neue Gegenstelle gekoppelt: {fp} ({ip}), Vergleichscode {sas}"));
                    Z.pair_open.store(false, Ordering::Relaxed);
                }
                // Die Leitung faellt mit `sock` zu - der Client deutet das
                // Ende nach dem Handschlag als "nicht gekoppelt".
                Ok(false) => {
                    DROSSEL_UNBEKANNT.melden(|| {
                        format!("Abgewiesen: unbekannte Gegenstelle {fp} von {ip}. Host mit --pair starten, um sie aufzunehmen.")
                    });
                    return;
                }
                Err(e) => {
                    DROSSEL_UNBEKANNT.melden(|| format!("Abgewiesen: unbekannte Gegenstelle {fp} von {ip} - {e}"));
                    return;
                }
            }
        }
    }

    // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
    // Fenstergroesse und Format kennt, bevor das erste Bild kommt.
    let mut hello = Vec::with_capacity(4 + 8 + 8);
    hello.extend_from_slice(MAGIC);
    hello.extend_from_slice(&kopf(MSG_INFO, 0, 0, 8));
    hello.extend_from_slice(&super::strominfo());

    let leitung = Arc::new(Leitung::neu(sock.abbruchgriff(), sock.peer.clone(), sock.handshake_hash.clone(), ip.clone()));
    // Ein neuer Zuschauer ersetzt den alten: dessen Eingabekanal wird
    // gekappt, seine Warteschlange verworfen; er bekommt nur noch
    // MSG_ABGELOEST, dann endet sein Sendefaden (unten wird darauf gewartet).
    let alt = {
        let _einspeisen = sperre(&EINSPEISEN);
        let mut a = sperre(&AKTUELL);
        let alt = a.take();
        if let Some(alt) = &alt {
            log(format!("Zuschauer abgeloest: {}", alt.ip));
            alt.abloesen();
            eingabe::alle_tasten_loslassen();
        }
        let _ = leitung.einreihen(hello, Art::Klein, None);
        *a = Some(leitung.clone());
        alt
    };
    NR.fetch_add(1, Ordering::Relaxed);
    let l2 = leitung.clone();
    std::thread::spawn(move || sendefaden(l2, sock));

    Z.force_key.store(true, Ordering::Relaxed);
    Z.wait_key.store(true, Ordering::Relaxed);
    // Zeigerform und Tonformat gehen dem neuen Zuschauer erneut zu.
    super::zeiger::neu_senden();
    super::ton::info_zuruecksetzen();
    settings_senden();
    send_small(MSG_CODECS, &encoder::codecs_payload());
    log(format!("Zuschauer verbunden: {ip}:{port}, verschluesselt, Gegenstelle {fp}, Vergleichscode {sas}"));
    // Der Neue laeuft schon; jetzt erst auf den Abgeloesten warten: solange
    // er abnimmt (Rest des laufenden Pakets, dann das Schlusswort), sonst
    // wird seine Bildleitung gekappt.
    if let Some(alt) = alt {
        alt.abloesung_abschliessen(FRIST_SCHLUSSWORT * 2);
    }
}

// ------------------------------------------------------------ Eingabe-Teil
// Zweite Verbindung, nur fuer Maus, Tastatur und Zwischenablage. Getrennt
// vom Bild, damit eine Mausbewegung nie hinter einem Vollbild haengt.

fn annahme_eingabe(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        stream.set_nodelay(true).ok();

        // Der Eingabekanal darf erst aufmachen, wenn der Bildkanal steht: sein
        // Prologue enthaelt dessen Pruefsumme. Wer die nicht kennt, kommt hier
        // nicht durch - damit kann niemand nur die Tastatur uebernehmen.
        let Some(bild) = aktuell() else {
            DROSSEL_KEIN_BILD.melden(|| "Eingabekanal abgewiesen: kein Bildkanal offen".into());
            continue;
        };
        let Some(platz) = PLAETZE_EINGABE.belegen(absender(&stream), "Eingabekanal") else { continue };
        std::thread::spawn(move || eingabe_annehmen(stream, bild, platz));
    }
}

fn eingabe_annehmen(stream: TcpStream, bild: Arc<Leitung>, platz: Platz) {
    let mut sock = match secure::Secure::accept(stream, &noise::prologue_input(&bild.hh), PRIV.get().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            DROSSEL_HANDSCHLAG_EINGABE.melden(|| format!("Eingabekanal: Handschlag gescheitert ({e})"));
            return;
        }
    };
    drop(platz);
    if sock.peer != bild.peer {
        DROSSEL_EINGABE_ABGEWIESEN.melden(|| "Eingabekanal abgewiesen: andere Gegenstelle als beim Bild".into());
        return;
    }
    // Erst jetzt an den Zuschauer binden - und nur, wenn er noch der
    // aktuelle ist: ein Wechsel waehrend des Handschlags macht diese
    // Leitung wertlos. Ohne Abbruchgriff liesse sie sich nicht kappen.
    let nr = EINGABE_NR.fetch_add(1, Ordering::Relaxed);
    let einspeisen = sperre(&EINSPEISEN);
    let Some(griff) = sock.abbruchgriff() else {
        DROSSEL_EINGABE_ABGEWIESEN.melden(|| "Eingabekanal abgewiesen: Leitung nicht zu fassen".into());
        return;
    };
    if !bild.eingabe_binden(nr, griff) {
        DROSSEL_EINGABE_ABGEWIESEN.melden(|| "Eingabekanal abgewiesen: Zuschauer inzwischen gewechselt".into());
        return;
    }
    drop(einspeisen);
    log("Eingabekanal verbunden, verschluesselt");

    let mut hdr = [0u8; 8];
    let mut payload: Vec<u8> = Vec::new();
    loop {
        if sock.read_exact(&mut hdr).is_err() {
            break;
        }
        let typ = hdr[0];
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        // Text kann gross sein (bis 4 MB); alles andere ist winzig.
        let grenze = if typ == IN_CLIP { 4 * 1024 * 1024 } else { 256 };
        if len > grenze {
            break;
        }
        payload.resize(len, 0);
        if len > 0 && sock.read_exact(&mut payload).is_err() {
            break;
        }
        // Abgeloest? Dann nichts mehr einspeisen - auch nicht, was schon
        // entschluesselt im Puffer lag. Pruefen und Ausfuehren unter EINSPEISEN.
        let _einspeisen = sperre(&EINSPEISEN);
        if !bild.gilt(nr) {
            break;
        }
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
            _ => {}
        }
    }
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

fn bekanntgabe(port: u16) {
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) else { return };
    sock.set_broadcast(true).ok();
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into());
    let mut nlen = name.len().min(40);
    while !name.is_char_boundary(nlen) {
        nlen -= 1;
    }
    let mut pkt = Vec::with_capacity(8 + nlen);
    pkt.extend_from_slice(BEACON_MAGIC);
    pkt.push(BEACON_VERSION);
    pkt.extend_from_slice(&port.to_le_bytes());
    pkt.push(nlen as u8);
    pkt.extend_from_slice(&name.as_bytes()[..nlen]);
    let mut erste = true;
    loop {
        let mut ziele = rundruf_adressen();
        if ziele.is_empty() {
            ziele.push(std::net::Ipv4Addr::BROADCAST);
        }
        let mut gesendet = 0;
        for z in &ziele {
            if sock.send_to(&pkt, (*z, port + 2)).is_ok() {
                gesendet += 1;
            }
        }
        if erste {
            log(format!("Bekanntgabe: an {gesendet} Netze, Port {}", port + 2));
            erste = false;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Annahmefaeden und Bekanntgabe starten.
pub fn start(port: u16, priv_key: Vec<u8>) -> Result<(), String> {
    PRIV.set(priv_key).ok();
    // Die Startzeile zaehlt eine unlesbare Liste als 0 - hier steht, was das heisst.
    match secure::Freigaben::lesen() {
        Err(e) => log(format!("Freigabeliste: {e} - jede Gegenstelle wird abgewiesen, bis die Datei repariert ist")),
        Ok(l) => {
            if let Err(e) = l.erstkontakt() {
                log(format!("Freigabeliste: {e}"));
            }
        }
    }
    let bild = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("Bild-Port {port} nicht verfuegbar: {e}"))?;
    let eingabe = TcpListener::bind(("0.0.0.0", port + 1))
        .map_err(|e| format!("Eingabe-Port {} nicht verfuegbar: {e}", port + 1))?;
    std::thread::spawn(move || annahme_bild(bild));
    std::thread::spawn(move || annahme_eingabe(eingabe));
    std::thread::spawn(move || bekanntgabe(port));
    Ok(())
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
    /// laufen hoechstens zwei Handschlaege.
    #[test]
    fn zuschauerwechsel_und_parallele_annahme() {
        let (host_priv, _) = noise::keypair().unwrap();
        PRIV.set(host_priv).unwrap();
        secure::test_identitaet();
        // Ist irgendwann ein Faden unter einer der Sperren in Panik geraten,
        // muss der Zuschauerplatz trotzdem weiterlaufen. Hier mit Absicht:
        // alle drei vergiftet, bevor es losgeht.
        for m in [&EINSPEISEN, &FREIGABE] {
            let _ = std::thread::spawn(move || {
                let _g = m.lock();
                panic!("mit Absicht: Sperre vergiften (Test)");
            })
            .join();
        }
        let _ = std::thread::spawn(|| {
            let _g = AKTUELL.lock();
            panic!("mit Absicht: Sperre vergiften (Test)");
        })
        .join();
        assert!(EINSPEISEN.is_poisoned() && FREIGABE.is_poisoned() && AKTUELL.is_poisoned());
        let bild_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ein_l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bild_addr = bild_l.local_addr().unwrap().to_string();
        let ein_addr = ein_l.local_addr().unwrap().to_string();
        std::thread::spawn(move || annahme_bild(bild_l));
        std::thread::spawn(move || annahme_eingabe(ein_l));

        // Zuschauer A, waehrend eine stumme Verbindung am Bildport haengt.
        let stumm = TcpStream::connect(&bild_addr).unwrap();
        let mut a = bild_verbinden(&bild_addr);
        drop(stumm);
        // Eingabekanal von A, ebenso an einer stummen Verbindung vorbei.
        let stumm = TcpStream::connect(&ein_addr).unwrap();
        let t0 = Instant::now();
        let mut a_ein = secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        drop(stumm);
        zeitfrage(&mut a_ein, 0xA1).unwrap();
        assert_eq!(zeitantwort(&mut a), Ok(0xA1));

        // Zuschauer B loest A ab: A verliert Bild UND Eingabe - und bekommt
        // als letzte Nachricht MSG_ABGELOEST, damit er nicht zurueckkommt.
        let mut b = bild_verbinden(&bild_addr);
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
        zeitfrage(&mut b_ein, 0xB1).unwrap();
        assert_eq!(zeitantwort(&mut b), Ok(0xB1));
        // Die alte Pruefsumme passt nicht mehr: A kommt nicht wieder herein.
        assert!(secure::Secure::connect(&ein_addr, &noise::prologue_input(&a.handshake_hash)).is_err());

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
        let _c = bild_verbinden(&bild_addr);
    }

    /// Die Grenzen der laufenden Handschlaege: je Absender zwei, insgesamt
    /// so viele, dass zwei oder drei Adressen niemanden aussperren. Erst
    /// HANDSCHLAEGE_MAX / 2 Absender fuellen die Tabelle.
    #[test]
    fn handschlagplaetze_brauchen_viele_absender() {
        static P: Plaetze = Plaetze::neu();
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
    /// ergaben 300 Zeilen): je Art hoechstens eine Zeile alle zehn Sekunden,
    /// die naechste nennt die Zahl der unterdrueckten. Der Text einer
    /// unterdrueckten Zeile wird gar nicht erst gebaut.
    #[test]
    fn drossel_laesst_eine_zeile_je_art_durch_und_zaehlt_den_rest() {
        static D: Drossel = Drossel::neu();
        static ANDERE: Drossel = Drossel::neu();
        assert_eq!(D.melden(|| "Handschlag mit 10.0.0.1 gescheitert".into()).as_deref(), Some("Handschlag mit 10.0.0.1 gescheitert"));
        let mut gebaut = false;
        assert_eq!(
            D.melden(|| {
                gebaut = true;
                String::new()
            }),
            None
        );
        assert!(!gebaut, "Text einer unterdrueckten Zeile gebaut");
        for _ in 0..298 {
            assert_eq!(D.melden(|| "x".into()), None);
        }
        // Eine andere Art hat ihre eigene Drossel.
        assert!(ANDERE.melden(|| "Abgewiesen: unbekannte Gegenstelle".into()).is_some());
        // Zehn Sekunden spaeter: eine Zeile, samt Zahl der unterdrueckten.
        *sperre(&D.letzte) = Instant::now().checked_sub(Duration::from_secs(11));
        assert_eq!(
            D.melden(|| "Handschlag mit 10.0.0.2 gescheitert".into()).as_deref(),
            Some("Handschlag mit 10.0.0.2 gescheitert (und 299 weitere seit der letzten Meldung)")
        );
        assert_eq!(D.melden(|| "x".into()), None);
        *sperre(&D.letzte) = Instant::now().checked_sub(Duration::from_secs(11));
        assert_eq!(D.melden(|| "wieder".into()).as_deref(), Some("wieder (und 1 weitere seit der letzten Meldung)"));
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
        assert!(weg <= Duration::from_micros(STAU_FRIST_US) + Duration::from_millis(500), "{weg:?}");
        assert_eq!(lauf.verworfen, 0, "{lauf:?}");
    }
}
