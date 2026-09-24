// Dateien ueber die Zwischenablage - der gemeinsame Kern fuer den
// Windows-Client, die Windows-Host-Rolle und den Mac-Client (Spezifikation
// "Dateien ueber die Zwischenablage", Abschnitte 2 und 3.1). Der Mac-Host
// baut dasselbe Protokoll in Objective-C nach (host/dateien.m); die
// Pruefvektoren in den Tests unten gelten fuer beide.
//
// Kopiert der Nutzer Dateien, sendet die Seite mit der Kopie sofort ueber die
// bestehende, verschluesselte Verbindung. Die Gegenseite schreibt sie in ein
// frisches Uebertragungsverzeichnis und legt die obersten Eintraege als
// Dateiliste in ihre eigene Ablage. Dateien haben Nachrang vor Bild, Ton und
// Eingaben: nie mehr als FENSTER Byte unquittiert, und eine volle oertliche
// Warteschlange (Gesendet::Voll) wird abgewartet statt ueberfuellt.
//
// Plattformunabhaengig mit std. Weichen gibt es nur fuer die Pfadbereinigung
// (EIGENE_REGELN) und den freien Platz (freier_platz).
//
// ============================================================ PROTOKOLL
//
// Kopf wie immer: u8 Typ, u8 Flags (0), u16 frei (0), u32 Laenge; alle Zahlen
// little endian. Typen in protokoll_konst.rs:
//
//   11 MSG_FAEHIGKEITEN  Host -> Client, Bildkanal      u32 Bits (>= 4 Byte)
//   69 IN_FAEHIGKEITEN   Client -> Host, Eingabekanal   u32 Bits (>= 4 Byte)
//   50 DATEI_ANGEBOT     Sender -> Empfaenger           Angebot
//   51 DATEI_STUECK      Sender -> Empfaenger           Stueck
//   52 DATEI_ENDE        Sender -> Empfaenger           Ende
//   53 DATEI_QUITTUNG    Empfaenger -> Sender           Quittung (16 Byte)
//
// Host -> Client laeuft auf dem Bildkanal, Client -> Host auf dem
// Eingabekanal, die Quittung jeweils in der Gegenrichtung. FAEHIG_DATEIEN
// (Bit 0) heisst "Dateien Fassung 1". DATEI_* geht nur an eine Gegenseite,
// die in DIESER Sitzung (Host: auf dem AKTUELLEN Eingabekanal) FAEHIG_DATEIEN
// gemeldet hat; beim Zuschauerwechsel bzw. Sitzungsende gilt sie wieder als
// unwissend.
//
//   Angebot   u32 kennung (!= 0, je Prozess aufsteigend), u32 anzahl
//             (1 ..= 10000), u64 gesamt (Summe aller Dateigroessen, exakt),
//             dann anzahl mal: u8 art (0 Datei, 1 Ordner), u8 frei,
//             u16 pfadlaenge (1 ..= 1024), u64 groesse (Ordner 0),
//             Pfad (UTF-8, relativ, Trenner '/'). Hoechstens 1 MiB.
//             Vorfahr vor Nachfahr; oberste Eintraege ohne '/'.
//   Stueck    u32 kennung, u32 eintrag, u64 versatz, 1 ..= 49152 Byte Daten.
//             Streng der Reihe nach, Ordner und leere Dateien uebersprungen.
//   Ende      u32 kennung, u8 grund (GRUND_*).
//   Quittung  u32 kennung, u8 zustand (ZUSTAND_*), u8 0, u16 0,
//             u64 empfangen (bisher geschriebene Datenbytes).
//
// Die Eingangsgrenzen je Typ fuer den Eingabekanal der Hosts liefert
// eingangsgrenze(); diese Zweige stehen VOR der alten 256-Byte-Regel.
//
// ======================================================== SCHNITTSTELLE
//
// Weg (je Richtung eine Umsetzung der Rolle)
//
//   trait Weg: Send + Sync { fn senden(&self, typ: u8, nutzlast: &[u8]) -> Gesendet }
//   enum Gesendet { Ja, Voll, Weg }
//
//   Ja   = eingereiht. Voll = oertliche Warteschlange voll, spaeter noch einmal
//   (der Kern schlaeft dann VOLL_WARTEN = 2 ms - thread::sleep, das auch unter
//   Windows so kurz schlaeft - und ruft erneut mit DENSELBEN Bytes; ein
//   Abbruch wirkt nach hoechstens einem Schlaf). Weg = Sitzung vorbei,
//   abbrechen.
//   senden() darf nie lange blockieren. Es wird nur aus den Faeden dieses
//   Moduls gerufen, nie unter einer Sperre dieses Moduls. Fuer Closures gibt
//   es eine Umsetzung: `Arc::new(|typ: u8, n: &[u8]| ... )`.
//   ABWEICHUNG von der Spezifikation (dort `nutzlast: Vec<u8>`): die Nutzlast
//   kommt als &[u8]. Bei Voll wird mit denselben Bytes wiederholt, ohne Kopie
//   je Versuch; beide vorhandenen Sendewege (send_small_an, InputLink::send)
//   nehmen ohnehin &[u8] und kopieren in ihren eigenen Rahmen.
//
// Sender (Faden "qc-dateien-senden")
//
//   Sender::starten(pfade: Vec<PathBuf>, weg: Arc<dyn Weg>, fenster: u64,
//                   melden: impl Fn(Ereignis) + Send + 'static) -> Griff
//   Sender::starten_mit(..., vorgaben: Vorgaben) -> Griff
//
//   Vorgaben { stillstand, voll_warten, ende_frist, freier_platz, grenzen }
//   mit Vorgaben::default() fuer die Produktion (STILLSTAND, VOLL_WARTEN,
//   ENDE_FRIST, freier_platz(), Grenzen::default()); Tests verkuerzen die
//   Fristen, setzen einen Platz vor oder setzen die Grenzen des Auflistens
//   herab (Grenzen { eintraege, gesamt, angebot }).
//
//   starten() kehrt sofort zurueck; Auflisten, Lesen und Senden laufen im
//   eigenen Faden, nie im Ablagewaechter. fenster = fenster(spielmodus):
//   FENSTER bzw. FENSTER_SPIEL. Ablauf: eigene Ablage? (dann nichts) ->
//   auflisten() -> Angebot -> Stuecke (hoechstens fenster Byte unquittiert,
//   ein Stueck wird dafuer notfalls kuerzer geschnitten) -> Ende 0 -> warten
//   auf Quittung 1, hoechstens STILLSTAND. Ohne neue Quittung ueber
//   STILLSTAND: Ende 3. Datei kuerzer als angekuendigt oder unlesbar: Ende 2.
//   Laenger geworden: nur die angekuendigte Groesse geht hinaus.
//
//   Griff::kennung() -> u32
//   Griff::quittung(&self, nutzlast: &[u8])  Nutzlast von DATEI_QUITTUNG, wie
//       sie ankommt; fremde Kennungen werden uebergangen. Kehrt sofort zurueck,
//       keine Plattenarbeit: darf im Lesefaden eines Kanals laufen.
//   Griff::abbrechen(&self)  kehrt sofort zurueck (wartet NICHT auf den
//       Faden - sonst drohte eine Verklemmung, wenn der Aufrufer die Sperre
//       haelt, die der Weg braucht). Der Faden schickt, falls das Angebot
//       schon hinaus ist, Ende 1 (hoechstens ENDE_FRIST lang bei Voll, bei
//       Weg gar nicht) und endet. Drop des Griffs bricht ebenso ab.
//   Griff::laeuft(&self) -> bool   false, sobald der Faden zu Ende ist.
//   Griff::abwarten(&self, frist) -> bool   wartet auf das Ende des Fadens.
//
// Empfaenger (Faden "qc-dateien-empfang")
//
//   Empfaenger::neu(basis: PathBuf, weg_fuer_quittungen: Arc<dyn Weg>,
//                   fertig: impl Fn(Vec<PathBuf>) -> bool + Send + 'static,
//                   melden: impl Fn(Ereignis) + Send + 'static) -> Empfaenger
//   Empfaenger::neu_mit(..., vorgaben: Vorgaben)
//
//   basis = ablage_basis() in der Produktion. Je Sitzung ein Empfaenger, weil
//   der Quittungsweg an die Sitzung gebunden ist.
//   Empfaenger::nachricht(&self, typ: u8, nutzlast: Vec<u8>)  legt 50/51/52
//       nur in die Warteschlange und kehrt sofort zurueck (darf im Lesefaden
//       eines Kanals und unter EINSPEISEN laufen). Andere Typen: uebergangen.
//   Empfaenger::abbrechen(&self)  Sitzungsende/Zuschauerwechsel: die laufende
//       Uebertragung wird verworfen, ihr Verzeichnis geloescht, der Faden
//       endet; weitere Nachrichten werden uebergangen. Kehrt sofort zurueck.
//       Drop bricht ebenso ab. Fuer die naechste Sitzung einen neuen
//       Empfaenger anlegen.
//   Empfaenger::laeuft / abwarten wie beim Griff.
//
//   fertig(oberste_pfade) laeuft im Schreibfaden, sobald alles geschrieben
//   ist, und legt die Pfade in die eigene Ablage (clipboard::set_dateien).
//   true -> Quittung 1, Aufraeumen (die drei neuesten bleiben), Stand Fertig.
//   false -> Quittung 5, Verzeichnis geloescht, Abbruch::Ablage.
//   ABWEICHUNG von der Spezifikation (dort `Fn(Vec<PathBuf>)`): der Rueckruf
//   meldet, ob die Ablage gesetzt wurde; Quittung 1 heisst "in die Ablage
//   gelegt" und soll das nicht behaupten, wenn es nicht stimmt.
//
//   Geschrieben wird nur im eigenen Schreibfaden, nie im Lesefaden. Quittung
//   0 nach der Annahme (empfangen 0), dann spaetestens je QUITTUNG_ALLE Byte
//   und zusaetzlich immer dann, wenn die Warteschlange leer ist und noch
//   Unquittiertes vorliegt (so kann kein Sender mit kleinem Fenster haengen
//   bleiben). Ueber STILLSTAND ohne Stueck: Quittung 6, Verzeichnis weg.
//
// Stand und Ereignis (fuer Oberflaeche und Protokoll)
//
//   melden(Ereignis) laeuft im Faden des Senders bzw. Empfaengers, nie unter
//   einer Sperre dieses Moduls; Fortschritt hoechstens alle 100 ms.
//   struct Ereignis { stand: Option<Stand>, zeile: Option<String> }
//     stand -> Oberflaeche (Shared.datei_stand), zeile -> Protokoll
//     (host::log bzw. protokoll::zeile). Die Zeilen sind fertig und deutsch:
//       "Dateien: sende 3 Eintraege, 12,4 MB"     (Host haengt " an <ip>" an)
//       "Dateien: gesendet und quittiert (12,4 MB in 1,8 s)"
//       "Dateien: empfange 3 Eintraege, 12,4 MB"
//       "Dateien: empfangen 3 Eintraege, 12,4 MB - in die Ablage gelegt"
//       "Dateien: abgelehnt (...)", "Dateien: abgebrochen (...)",
//       "Dateien: nicht gesendet, zu gross (...)",
//       "Dateien: uebersprungen (...): <pfad>"
//     ZEILE_ZUSCHAUER_ZU_ALT / ZEILE_HOST_ZU_ALT fuer die Rollen.
//   struct Stand { richtung, kennung, bytes, gesamt, eintraege, dateien,
//                  oberste, ergebnis }
//   enum Ergebnis { Laeuft, Fertig, Abgebrochen(Abbruch), ZuGross,
//                   GegenseiteZuAlt }
//     Senden: bytes = quittierte Byte; Empfangen: geschriebene Byte.
//     Fertig heisst beim Senden FilesSent, beim Empfangen FilesReady (dort
//     ist `oberste` die Zahl der abgelegten Eintraege). ZuGross = FilesTooLarge
//     (auch bei Quittung 2). GegenseiteZuAlt entsteht nie hier: die Rolle
//     meldet sie mit Stand::gegenseite_zu_alt(richtung) (FilesPeerOld).
//   enum Abbruch { Hier, Verbindung, Lesefehler(String), Zeitueberschreitung,
//                  Quittung(u8), Ende(u8), Abgelehnt(u8, String),
//                  Ungueltig(String), Schreibfehler(String), Ablage }
//     Abbruch::text() liefert den Protokolltext.
//
// Ablageverzeichnis
//
//   ablage_basis() -> PathBuf   temp_dir()/QuadChroma-Ablage (unter cfg(test)
//       ein eigener Ordner je Lauf). Nie Schreibtisch, Dokumente, Downloads.
//   basis_anlegen(&Path)        Unix 0700; eine Verknuepfung als Basis gilt nicht.
//   aufraeumen(basis, behalten, hoechstalter: Option<Duration>) -> usize
//   aufraeumen_beim_start() -> usize   beim Programmstart: aelter als 24 h weg.
//   aus_eigener_ablage(pfade, basis) -> bool   Widerhallschutz (2.7 Schritt 2);
//       der Sender prueft es selbst noch einmal.
//   Je Uebertragung <basis>/<unix-ms>-<kennung>; waehrend sie laeuft, liegt
//   daneben die leere Marke <unix-ms>-<kennung>.laeuft. aufraeumen() zaehlt
//   und loescht nur Verzeichnisse ohne Marke (ausser nach 24 h): Client und
//   Host-Rolle teilen unter Windows dieselbe Basis (%TEMP%), und das Aufraeumen
//   des einen darf dem anderen keine laufende Uebertragung wegloeschen. Das
//   ergaenzt 2.9 ("die drei neuesten bleiben") um laufende Uebertragungen.
//   Geloescht wird nur innerhalb der Basis; Verknuepfungen werden nie verfolgt.
//
// Kodierer, Leser, Pfade
//
//   faehigkeiten_kodieren / faehigkeiten_lesen / kann_dateien
//   Angebot { kennung, gesamt, eintraege: Vec<Eintrag> }: kodieren(), lesen()
//     (Format, Grenzen, Summe), pruefen(regeln) (Pfadregeln, Reihenfolge,
//     Bereinigung, Doppelte; liefert die bereinigten relativen Pfade)
//   Stueck { kennung, eintrag, versatz, daten }: kodieren(), kodieren_in(), lesen()
//   Ende { kennung, grund }, Quittung { kennung, zustand, empfangen }
//   kennung_lesen(nutzlast), eingangsgrenze(typ), neue_kennung()
//   pfad_pruefen(pfad), pfad_bereinigen(pfad, regeln),
//   bestandteil_bereinigen(teil, regeln), Regeln { Windows, Mac }, EIGENE_REGELN
//   auflisten(pfade) -> Liste   (Senderseite, 2.7 Schritt 3; auflisten_mit
//       mit eigenen Grenzen). Liste { eintraege, quellen, gesamt, hinweise,
//       zu_gross: Option<String> }
//   freier_platz(pfad) -> Option<u64>   Windows GetDiskFreeSpaceExW, sonst None
//   mb_text(bytes) -> "12,4"
//
// Auslegung der Spezifikation (auch im Bericht genannt):
//   - anzahl > 10000, gesamt > 4 GiB und ein Angebot ueber 1 MiB ergeben
//     Quittung 2 ("zu gross oder zu viele Eintraege"), alle anderen Fehler
//     im Angebot Quittung 4.
//   - Ein neues Angebot verwirft alles, was von frueheren Uebertragungen noch
//     in der Warteschlange steht; Stuecke und Enden mit fremder Kennung werden
//     still uebergangen (etwa der Rest eines abgelehnten Angebots).
//   - Der Sender ueberspringt beim Auflisten ausser Verknuepfungen auch
//     Namen, die die Gegenseite ablehnen muesste ('\', nicht UTF-8, zu lang,
//     zu tief) und Namen, die nach der strengsten (Windows-)Bereinigung ohne
//     Ruecksicht auf die Schreibweise doppelt waeren - auf jeder Stufe, nicht
//     nur oben. Jeweils mit Protokollzeile.
//
// Pflichten der Rollen (Integrationspakete):
//   - Eingang: 50/51/52 -> Empfaenger::nachricht, 53 -> Griff::quittung. Keine
//     Plattenarbeit im Lesefaden oder unter EINSPEISEN; beides tut hier nichts.
//   - Neuer Ablageinhalt (Text oder Dateien, aber nicht aus_eigener_ablage),
//     Sitzungsende, Zuschauerwechsel, Verlust des Eingabekanals: Griff
//     abbrechen bzw. fallen lassen. Je Richtung hoechstens ein Griff.
//   - Sitzungsende/Zuschauerwechsel: Empfaenger::abbrechen.
//   - Programmstart: aufraeumen_beim_start().
//   - Der Sender schneidet ein Stueck notfalls kuerzer, damit nie mehr als das
//     Fenster unterwegs ist. Das muss auch ein anderer Sender (Mac-Host) tun:
//     Bei FENSTER_SPIEL = 64 KiB und Stuecken zu 48 KiB wartete er sonst auf
//     eine Quittung, die ein Empfaenger, der erst nach 64 KiB quittiert, nie
//     schickt. Dieser Empfaenger quittiert zusaetzlich bei leerer
//     Warteschlange und haengt deshalb mit keinem Sender.

#![allow(dead_code)] // Das Modul steht; die Rollen binden es in eigenen Paketen an.

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::protokoll_konst::{
    DATEI_ANGEBOT, DATEI_ENDE, DATEI_QUITTUNG, DATEI_STUECK, FAEHIG_DATEIEN, IN_FAEHIGKEITEN,
};

// ================================================================ Grenzen

/// Hoechstens so viele Eintraege (Dateien und Ordner) je Angebot.
pub const EINTRAEGE_MAX: u32 = 10_000;
/// Hoechstens so viele Datenbytes je Uebertragung (4 GiB).
pub const GESAMT_MAX: u64 = 4 * 1024 * 1024 * 1024;
/// Laengster relativer Pfad in Byte.
pub const PFAD_MAX: usize = 1024;
/// Laengster Bestandteil eines Pfads in Byte.
pub const BESTANDTEIL_MAX: usize = 255;
/// Hoechstens so viele Stufen je Pfad.
pub const TIEFE_MAX: usize = 32;
/// Groesstes Angebot in Byte (1 MiB).
pub const ANGEBOT_MAX: usize = 1024 * 1024;
/// Hoechstens so viele Datenbytes je Stueck.
pub const STUECK_MAX: usize = 49_152;
/// Kopf eines Stuecks: u32 kennung, u32 eintrag, u64 versatz.
pub const STUECK_KOPF: usize = 16;
/// Kopf eines Angebots: u32 kennung, u32 anzahl, u64 gesamt.
pub const ANGEBOT_KOPF: usize = 16;
/// Kopf eines Eintrags: u8 art, u8 frei, u16 pfadlaenge, u64 groesse.
pub const EINTRAG_KOPF: usize = 12;
/// Eine Quittung ist genau so lang.
pub const QUITTUNG_LAENGE: usize = 16;
/// Ein Ende ist mindestens so lang.
pub const ENDE_MIN: usize = 5;
/// Grenze fuer 52, 53 und 69 auf dem Eingabekanal der Hosts.
pub const KLEIN_MAX: usize = 256;
/// Unquittiert unterwegs, normal.
pub const FENSTER: u64 = 262_144;
/// Unquittiert unterwegs im Spielmodus.
pub const FENSTER_SPIEL: u64 = 65_536;
/// Der Empfaenger quittiert spaetestens nach so vielen geschriebenen Byte.
pub const QUITTUNG_ALLE: u64 = 65_536;
/// Ohne Fortschritt so lange, dann bricht die jeweilige Seite ab.
pub const STILLSTAND: Duration = Duration::from_secs(30);
/// So viel muss nach der Uebertragung noch frei sein (1 GiB).
pub const PLATZRESERVE: u64 = 1024 * 1024 * 1024;
/// Bei Gesendet::Voll so lange warten, dann erneut.
pub const VOLL_WARTEN: Duration = Duration::from_millis(2);
/// Ende bzw. Quittung nach einem Abbruch: so lange Voll abwarten, dann aufgeben.
pub const ENDE_FRIST: Duration = Duration::from_secs(1);
/// Nach einem vollstaendigen Empfang bleiben so viele Verzeichnisse.
pub const BEHALTEN: usize = 3;
/// Beim Start werden Verzeichnisse geloescht, die aelter sind.
pub const HOECHSTALTER: Duration = Duration::from_secs(24 * 3600);

/// Fortschritt an die Oberflaeche hoechstens so oft.
const FORTSCHRITT_TAKT: Duration = Duration::from_millis(100);
/// So viel darf beim Empfaenger ungeschrieben warten (Stuecke und Enden,
/// je Nachricht 64 Byte Aufschlag). Ein Sender, der sein Fenster einhaelt,
/// kommt nie ueber FENSTER; darueber ist es ein Protokollfehler (Quittung 4).
const WARTESCHLANGE_MAX: usize = 4 * FENSTER as usize;
/// Endung der Marke einer laufenden Uebertragung.
const MARKE_ENDUNG: &str = ".laeuft";

// Grund im Ende (vom Sender).
pub const GRUND_VOLLSTAENDIG: u8 = 0;
pub const GRUND_ABGEBROCHEN: u8 = 1;
pub const GRUND_LESEFEHLER: u8 = 2;
pub const GRUND_ZEIT: u8 = 3;

// Zustand in der Quittung (vom Empfaenger). Alles ausser 0 beendet die
// Uebertragung auf beiden Seiten.
pub const ZUSTAND_LAEUFT: u8 = 0;
pub const ZUSTAND_FERTIG: u8 = 1;
pub const ZUSTAND_ZU_GROSS: u8 = 2;
pub const ZUSTAND_KEIN_PLATZ: u8 = 3;
pub const ZUSTAND_UNGUELTIG: u8 = 4;
pub const ZUSTAND_SCHREIBFEHLER: u8 = 5;
pub const ZUSTAND_ABGEBROCHEN: u8 = 6;

/// Protokollzeile der Host-Rollen, wenn der Zuschauer nichts davon kann.
pub const ZEILE_ZUSCHAUER_ZU_ALT: &str =
    "Dateien: Zuschauer kann keine Dateien empfangen (aelterer Client)";
/// Protokollzeile des Clients, wenn der Host nichts davon kann.
pub const ZEILE_HOST_ZU_ALT: &str = "Dateien: Host kann keine Dateien empfangen (aelterer Host)";

/// Das Fenster fuer die Rolle: im Spielmodus kleiner.
pub fn fenster(spielmodus: bool) -> u64 {
    if spielmodus {
        FENSTER_SPIEL
    } else {
        FENSTER
    }
}

/// Obergrenze je Nachricht auf dem Eingabekanal der Hosts (2.6); None fuer
/// alle anderen Typen. Groessere Nachrichten beenden den Kanal wie bisher.
pub fn eingangsgrenze(typ: u8) -> Option<usize> {
    match typ {
        DATEI_ANGEBOT => Some(ANGEBOT_MAX),
        DATEI_STUECK => Some(STUECK_KOPF + STUECK_MAX),
        DATEI_ENDE | DATEI_QUITTUNG | IN_FAEHIGKEITEN => Some(KLEIN_MAX),
        _ => None,
    }
}

pub fn grund_text(grund: u8) -> &'static str {
    match grund {
        GRUND_VOLLSTAENDIG => "vollstaendig",
        GRUND_ABGEBROCHEN => "abgebrochen",
        GRUND_LESEFEHLER => "Lesefehler",
        GRUND_ZEIT => "Zeitueberschreitung",
        _ => "unbekannter Grund",
    }
}

pub fn zustand_text(zustand: u8) -> &'static str {
    match zustand {
        ZUSTAND_LAEUFT => "laeuft",
        ZUSTAND_FERTIG => "fertig",
        ZUSTAND_ZU_GROSS => "zu gross oder zu viele Eintraege",
        ZUSTAND_KEIN_PLATZ => "zu wenig Platz",
        ZUSTAND_UNGUELTIG => "ungueltig",
        ZUSTAND_SCHREIBFEHLER => "Schreibfehler",
        ZUSTAND_ABGEBROCHEN => "abgebrochen",
        _ => "unbekannter Zustand",
    }
}

/// Byte als MB (10^6) mit einer Nachkommastelle und Komma: "12,4".
pub fn mb_text(bytes: u64) -> String {
    let zehntel = (bytes as f64 / 100_000.0).round() as u64;
    format!("{},{}", zehntel / 10, zehntel % 10)
}

fn sekunden_text(d: Duration) -> String {
    let zehntel = (d.as_secs_f64() * 10.0).round() as u64;
    format!("{},{}", zehntel / 10, zehntel % 10)
}

// ============================================================ Hilfen

/// Sperre nehmen, auch wenn ein anderer Faden unter ihr in Panik geriet: die
/// Zustaende hier bleiben nach jedem Schritt heil.
fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn warten<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>, d: Duration) -> MutexGuard<'a, T> {
    match cv.wait_timeout(g, d) {
        Ok((g, _)) => g,
        Err(e) => e.into_inner().0,
    }
}

fn warten_ohne_frist<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g).unwrap_or_else(|e| e.into_inner())
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Liest little endian aus einer Nutzlast; jeder Zugriff prueft die Grenzen.
struct Leser<'a> {
    n: &'a [u8],
    i: usize,
}

impl<'a> Leser<'a> {
    fn neu(n: &'a [u8]) -> Leser<'a> {
        Leser { n, i: 0 }
    }
    fn bytes(&mut self, k: usize) -> Option<&'a [u8]> {
        let ende = self.i.checked_add(k)?;
        let s = self.n.get(self.i..ende)?;
        self.i = ende;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.bytes(8).map(|b| {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a)
        })
    }
    fn rest(&self) -> &'a [u8] {
        &self.n[self.i..]
    }
}

// ============================================================ Weg

/// Ergebnis eines Sendeversuchs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesendet {
    /// Eingereiht.
    Ja,
    /// Oertliche Warteschlange voll: spaeter noch einmal.
    Voll,
    /// Sitzung vorbei: abbrechen.
    Weg,
}

/// Ein Sendeweg der Rolle, je Richtung einer. Siehe Modulkopf.
pub trait Weg: Send + Sync {
    fn senden(&self, typ: u8, nutzlast: &[u8]) -> Gesendet;
}

impl<F> Weg for F
where
    F: Fn(u8, &[u8]) -> Gesendet + Send + Sync,
{
    fn senden(&self, typ: u8, nutzlast: &[u8]) -> Gesendet {
        self(typ, nutzlast)
    }
}

/// Grenzen fuer das Auflisten beim Sender. Die Produktion nimmt
/// Grenzen::default() (2.6); Tests setzen sie herab, statt 10000 Dateien
/// anzulegen. Der Empfaenger prueft immer gegen die festen Konstanten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grenzen {
    pub eintraege: u32,
    pub gesamt: u64,
    pub angebot: usize,
}

impl Default for Grenzen {
    fn default() -> Grenzen {
        Grenzen { eintraege: EINTRAEGE_MAX, gesamt: GESAMT_MAX, angebot: ANGEBOT_MAX }
    }
}

/// Zeitvorgaben, freier Platz und Grenzen; Tests verkuerzen die Fristen.
/// Die Produktion nimmt Vorgaben::default().
#[derive(Clone, Copy, Debug)]
pub struct Vorgaben {
    /// Ohne Fortschritt so lange, dann Abbruch (Sender: Ende 3, Empfaenger:
    /// Quittung 6).
    pub stillstand: Duration,
    /// Bei Gesendet::Voll so lange warten.
    pub voll_warten: Duration,
    /// Ende nach einem Abbruch bzw. eine Quittung bei Voll hoechstens so lange
    /// versuchen.
    pub ende_frist: Duration,
    /// Freier Platz am Ort der Basis; None = nicht vorab pruefen.
    pub freier_platz: fn(&Path) -> Option<u64>,
    /// Grenzen beim Auflisten (Sender).
    pub grenzen: Grenzen,
}

impl Default for Vorgaben {
    fn default() -> Vorgaben {
        Vorgaben {
            stillstand: STILLSTAND,
            voll_warten: VOLL_WARTEN,
            ende_frist: ENDE_FRIST,
            freier_platz,
            grenzen: Grenzen::default(),
        }
    }
}

// ============================================================ Faehigkeiten

pub fn faehigkeiten_kodieren(bits: u32) -> Vec<u8> {
    bits.to_le_bytes().to_vec()
}

/// Mindestens 4 Byte; weitere Bytes werden uebergangen.
pub fn faehigkeiten_lesen(nutzlast: &[u8]) -> Option<u32> {
    Leser::neu(nutzlast).u32()
}

pub fn kann_dateien(bits: u32) -> bool {
    bits & FAEHIG_DATEIEN != 0
}

// ============================================================ Nachrichten

/// Die Kennung am Anfang jeder Datei-Nachricht, soweit lesbar.
pub fn kennung_lesen(nutzlast: &[u8]) -> Option<u32> {
    Leser::neu(nutzlast).u32()
}

static NAECHSTE_KENNUNG: AtomicU32 = AtomicU32::new(1);

/// Kennung fuer ein neues Angebot: ungleich 0, je Prozess aufsteigend.
pub fn neue_kennung() -> u32 {
    loop {
        let k = NAECHSTE_KENNUNG.fetch_add(1, Ordering::Relaxed);
        if k != 0 {
            return k;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EintragArt {
    Datei,
    Ordner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eintrag {
    pub art: EintragArt,
    /// Relativ, UTF-8, Trenner '/'.
    pub pfad: String,
    /// Bei Ordnern 0.
    pub groesse: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Angebot {
    pub kennung: u32,
    pub gesamt: u64,
    pub eintraege: Vec<Eintrag>,
}

/// Warum ein Angebot nicht angenommen wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ablehnung {
    /// Quittung 2.
    ZuGross(String),
    /// Quittung 3.
    KeinPlatz(String),
    /// Quittung 4.
    Ungueltig(String),
}

impl Ablehnung {
    pub fn zustand(&self) -> u8 {
        match self {
            Ablehnung::ZuGross(_) => ZUSTAND_ZU_GROSS,
            Ablehnung::KeinPlatz(_) => ZUSTAND_KEIN_PLATZ,
            Ablehnung::Ungueltig(_) => ZUSTAND_UNGUELTIG,
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Ablehnung::ZuGross(t) | Ablehnung::KeinPlatz(t) | Ablehnung::Ungueltig(t) => t,
        }
    }
}

impl Angebot {
    pub fn kodiert_laenge(&self) -> usize {
        ANGEBOT_KOPF + self.eintraege.iter().map(|e| EINTRAG_KOPF + e.pfad.len()).sum::<usize>()
    }

    /// Ohne Pruefung: der Sender baut Angebote nur aus auflisten(), das die
    /// Grenzen einhaelt.
    pub fn kodieren(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.kodiert_laenge());
        v.extend_from_slice(&self.kennung.to_le_bytes());
        v.extend_from_slice(&(self.eintraege.len() as u32).to_le_bytes());
        v.extend_from_slice(&self.gesamt.to_le_bytes());
        for e in &self.eintraege {
            v.push(match e.art {
                EintragArt::Datei => 0,
                EintragArt::Ordner => 1,
            });
            v.push(0);
            v.extend_from_slice(&(e.pfad.len() as u16).to_le_bytes());
            v.extend_from_slice(&e.groesse.to_le_bytes());
            v.extend_from_slice(e.pfad.as_bytes());
        }
        v
    }

    /// Liest ein Angebot und prueft Format, Grenzen und Summe (2.3, 2.6 und
    /// den Formatteil von 2.5). Die Pfadregeln prueft pruefen().
    pub fn lesen(n: &[u8]) -> Result<Angebot, Ablehnung> {
        let u = |t: String| Ablehnung::Ungueltig(t);
        if n.len() > ANGEBOT_MAX {
            return Err(Ablehnung::ZuGross(format!(
                "Angebot mit {} Byte, erlaubt sind {ANGEBOT_MAX}",
                n.len()
            )));
        }
        let mut l = Leser::neu(n);
        let (Some(kennung), Some(anzahl), Some(gesamt)) = (l.u32(), l.u32(), l.u64()) else {
            return Err(u("Angebot zu kurz".into()));
        };
        if kennung == 0 {
            return Err(u("Kennung 0".into()));
        }
        if anzahl == 0 {
            return Err(u("Angebot ohne Eintraege".into()));
        }
        if anzahl > EINTRAEGE_MAX {
            return Err(Ablehnung::ZuGross(format!(
                "{anzahl} Eintraege, erlaubt sind {EINTRAEGE_MAX}"
            )));
        }
        if gesamt > GESAMT_MAX {
            return Err(Ablehnung::ZuGross(format!("{} MB, erlaubt sind 4 GB", mb_text(gesamt))));
        }
        // Nicht blind nach der Anzahl vorbelegen: jeder Eintrag hat mindestens
        // 13 Byte, mehr als der Rest hergibt, kann es nicht geben.
        let platz = (anzahl as usize).min(l.rest().len() / (EINTRAG_KOPF + 1));
        let mut eintraege = Vec::with_capacity(platz);
        let mut summe: u64 = 0;
        for i in 0..anzahl {
            let (Some(art), Some(_frei), Some(laenge), Some(groesse)) =
                (l.u8(), l.u8(), l.u16(), l.u64())
            else {
                return Err(u(format!("Eintrag {i} unvollstaendig")));
            };
            let art = match art {
                0 => EintragArt::Datei,
                1 => EintragArt::Ordner,
                a => return Err(u(format!("Eintrag {i}: unbekannte Art {a}"))),
            };
            let laenge = laenge as usize;
            if laenge == 0 {
                return Err(u(format!("Eintrag {i}: leerer Pfad")));
            }
            if laenge > PFAD_MAX {
                return Err(u(format!("Eintrag {i}: Pfad laenger als {PFAD_MAX} Byte")));
            }
            let Some(pb) = l.bytes(laenge) else {
                return Err(u(format!("Eintrag {i}: Pfad unvollstaendig")));
            };
            let Ok(pfad) = std::str::from_utf8(pb) else {
                return Err(u(format!("Eintrag {i}: Pfad nicht in UTF-8")));
            };
            if art == EintragArt::Ordner && groesse != 0 {
                return Err(u(format!("Eintrag {i}: Ordner mit Groesse {groesse}")));
            }
            summe = match summe.checked_add(groesse) {
                Some(s) => s,
                None => return Err(u("Summe der Groessen laeuft ueber".into())),
            };
            eintraege.push(Eintrag { art, pfad: pfad.to_string(), groesse });
        }
        if !l.rest().is_empty() {
            return Err(u(format!("{} Byte nach dem letzten Eintrag", l.rest().len())));
        }
        if summe != gesamt {
            return Err(u(format!("Summe {summe} statt angekuendigt {gesamt}")));
        }
        Ok(Angebot { kennung, gesamt, eintraege })
    }

    /// Pfadregeln (2.5), Reihenfolge (Vorfahr vor Nachfahr), Bereinigung nach
    /// `regeln` und Doppelte nach der Bereinigung ohne Ruecksicht auf die
    /// Schreibweise. Liefert je Eintrag den bereinigten relativen Pfad.
    pub fn pruefen(&self, regeln: Regeln) -> Result<Vec<String>, Ablehnung> {
        let mut ordner: HashSet<&str> = HashSet::new();
        let mut gesehen: HashSet<String> = HashSet::new();
        let mut bereinigt = Vec::with_capacity(self.eintraege.len());
        let mut oberste = 0usize;
        for (i, e) in self.eintraege.iter().enumerate() {
            let teile = pfad_pruefen(&e.pfad)
                .map_err(|t| Ablehnung::Ungueltig(format!("Eintrag {i}: {t}")))?;
            match e.pfad.rsplit_once('/') {
                Some((eltern, _)) => {
                    if !ordner.contains(eltern) {
                        return Err(Ablehnung::Ungueltig(format!(
                            "Eintrag {i}: Ordner {eltern:?} fehlt davor"
                        )));
                    }
                }
                None => oberste += 1,
            }
            if e.art == EintragArt::Ordner {
                ordner.insert(&e.pfad);
            }
            let b = teile.iter().map(|t| bestandteil_bereinigen(t, regeln)).collect::<Vec<_>>().join("/");
            if !gesehen.insert(b.to_lowercase()) {
                return Err(Ablehnung::Ungueltig(format!("Eintrag {i}: {b:?} doppelt")));
            }
            bereinigt.push(b);
        }
        if oberste == 0 {
            return Err(Ablehnung::Ungueltig("kein oberster Eintrag".into()));
        }
        Ok(bereinigt)
    }

    pub fn oberste(&self) -> usize {
        self.eintraege.iter().filter(|e| !e.pfad.contains('/')).count()
    }

    pub fn dateien(&self) -> usize {
        self.eintraege.iter().filter(|e| e.art == EintragArt::Datei).count()
    }
}

/// Ein Stueck; `daten` zeigt in die Nutzlast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stueck<'a> {
    pub kennung: u32,
    pub eintrag: u32,
    pub versatz: u64,
    pub daten: &'a [u8],
}

impl<'a> Stueck<'a> {
    pub fn kodieren(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(STUECK_KOPF + self.daten.len());
        self.kodieren_in(&mut v);
        v
    }

    /// In einen vorhandenen Puffer (der Sender nimmt immer denselben).
    pub fn kodieren_in(&self, v: &mut Vec<u8>) {
        v.clear();
        v.extend_from_slice(&self.kennung.to_le_bytes());
        v.extend_from_slice(&self.eintrag.to_le_bytes());
        v.extend_from_slice(&self.versatz.to_le_bytes());
        v.extend_from_slice(self.daten);
    }

    /// None bei weniger als 1 oder mehr als STUECK_MAX Datenbytes.
    pub fn lesen(n: &'a [u8]) -> Option<Stueck<'a>> {
        let mut l = Leser::neu(n);
        let (kennung, eintrag, versatz) = (l.u32()?, l.u32()?, l.u64()?);
        let daten = l.rest();
        if daten.is_empty() || daten.len() > STUECK_MAX {
            return None;
        }
        Some(Stueck { kennung, eintrag, versatz, daten })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ende {
    pub kennung: u32,
    pub grund: u8,
}

impl Ende {
    pub fn kodieren(&self) -> Vec<u8> {
        let mut v = self.kennung.to_le_bytes().to_vec();
        v.push(self.grund);
        v
    }
    /// Mindestens 5 Byte; weitere werden uebergangen.
    pub fn lesen(n: &[u8]) -> Option<Ende> {
        let mut l = Leser::neu(n);
        Some(Ende { kennung: l.u32()?, grund: l.u8()? })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quittung {
    pub kennung: u32,
    pub zustand: u8,
    pub empfangen: u64,
}

impl Quittung {
    pub fn kodieren(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(QUITTUNG_LAENGE);
        v.extend_from_slice(&self.kennung.to_le_bytes());
        v.push(self.zustand);
        v.push(0);
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&self.empfangen.to_le_bytes());
        v
    }
    /// Genau 16 Byte, sonst None.
    pub fn lesen(n: &[u8]) -> Option<Quittung> {
        if n.len() != QUITTUNG_LAENGE {
            return None;
        }
        let mut l = Leser::neu(n);
        let kennung = l.u32()?;
        let zustand = l.u8()?;
        let _ = (l.u8()?, l.u16()?);
        Some(Quittung { kennung, zustand, empfangen: l.u64()? })
    }
}

// ============================================================ Pfade

/// Welche Namen das eigene System nicht kennt (2.5, "Bereinigt").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regeln {
    Windows,
    Mac,
}

/// Die Plattformweiche der Bereinigung.
pub const EIGENE_REGELN: Regeln = if cfg!(windows) { Regeln::Windows } else { Regeln::Mac };

/// Die Ablehnungsregeln aus 2.5 fuer einen einzelnen Pfad; liefert die
/// Bestandteile. Ein Fehler lehnt das ganze Angebot ab (Quittung 4).
pub fn pfad_pruefen(pfad: &str) -> Result<Vec<&str>, String> {
    if pfad.is_empty() {
        return Err("leerer Pfad".into());
    }
    if pfad.len() > PFAD_MAX {
        return Err(format!("Pfad laenger als {PFAD_MAX} Byte"));
    }
    if pfad.starts_with('/') {
        return Err("Pfad beginnt mit '/'".into());
    }
    if pfad.contains('\0') {
        return Err("NUL im Pfad".into());
    }
    if pfad.contains('\\') {
        return Err("'\\' im Pfad".into());
    }
    let teile: Vec<&str> = pfad.split('/').collect();
    if teile.len() > TIEFE_MAX {
        return Err(format!("mehr als {TIEFE_MAX} Stufen"));
    }
    for t in &teile {
        if t.is_empty() {
            return Err("leerer Bestandteil".into());
        }
        if *t == "." || *t == ".." {
            return Err(format!("Bestandteil {t:?}"));
        }
        if t.len() > BESTANDTEIL_MAX {
            return Err(format!("Bestandteil laenger als {BESTANDTEIL_MAX} Byte"));
        }
    }
    Ok(teile)
}

/// Prueft und bereinigt einen ganzen Pfad.
pub fn pfad_bereinigen(pfad: &str, regeln: Regeln) -> Result<String, String> {
    let teile = pfad_pruefen(pfad)?;
    Ok(teile.iter().map(|t| bestandteil_bereinigen(t, regeln)).collect::<Vec<_>>().join("/"))
}

/// Macht aus einem Bestandteil einen gueltigen Namen (2.5):
/// Windows: < > : " | ? * und U+0000-U+001F -> '_'; Punkte und Leerzeichen am
/// Ende -> '_'; Geraetenamen (CON, PRN, AUX, NUL, COM0-9, LPT0-9, dazu
/// COM/LPT mit hochgestellter 1-3 und CONIN$/CONOUT$), auch mit Endung und
/// in jeder Schreibweise, bekommen ein '_' davor.
/// Mac: nur U+0000-U+001F -> '_'; ':' bleibt.
pub fn bestandteil_bereinigen(teil: &str, regeln: Regeln) -> String {
    let steuer = |c: char| (c as u32) < 0x20;
    match regeln {
        Regeln::Mac => teil.chars().map(|c| if steuer(c) { '_' } else { c }).collect(),
        Regeln::Windows => {
            let mut s: String = teil
                .chars()
                .map(|c| if steuer(c) || "<>:\"|?*".contains(c) { '_' } else { c })
                .collect();
            let bleibt = s.trim_end_matches(['.', ' ']).len();
            let ersetzt = s.len() - bleibt;
            s.truncate(bleibt);
            s.push_str(&"_".repeat(ersetzt));
            if geraetename(&s) {
                s.insert(0, '_');
            }
            s
        }
    }
}

fn geraetename(name: &str) -> bool {
    let stamm = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let gross = stamm.to_ascii_uppercase();
    if matches!(gross.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    if gross.starts_with("COM") || gross.starts_with("LPT") {
        // Die ersten drei Byte sind ASCII, der Schnitt liegt also auf einer
        // Zeichengrenze.
        let rest = &stamm[3..];
        let ziffer = rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit();
        return ziffer || matches!(rest, "\u{b9}" | "\u{b2}" | "\u{b3}");
    }
    false
}

/// Haengt einen bereinigten relativen Pfad an; None, wenn ein Bestandteil
/// fuer das eigene System kein schlichter Name ist (Laufwerk, Wurzel, "..").
/// Das ist die letzte Wache vor dem Dateisystem.
fn sicher_anhaengen(basis: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = basis.to_path_buf();
    for t in rel.split('/') {
        let mut c = Path::new(t).components();
        match (c.next(), c.next()) {
            (Some(Component::Normal(x)), None) if x == std::ffi::OsStr::new(t) => p.push(x),
            _ => return None,
        }
    }
    Some(p)
}

// ============================================================ Ablageverzeichnis

/// Die Basis fuer empfangene Dateien. Nie im Schreibtisch, in Dokumente
/// oder Downloads: das waere auf dem Mac eine Freigabe-Rueckfrage.
pub fn ablage_basis() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("qc-test-{}-ablage", std::process::id()))
    } else {
        std::env::temp_dir().join("QuadChroma-Ablage")
    }
}

/// Legt die Basis an (Unix 0700) und prueft, dass sie ein echter Ordner und
/// keine Verknuepfung ist.
pub fn basis_anlegen(basis: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(basis)?;
    let md = fs::symlink_metadata(basis)?;
    if !md.is_dir() {
        return Err(io::Error::other("Ablagebasis ist kein Ordner"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if md.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(basis, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Liegt einer der Pfade in der eigenen Basis? Dann stammt die Liste aus
/// einem Empfang und geht nicht zurueck (2.7 Schritt 2). Verglichen wird wie
/// gegeben und nach canonicalize (Kurznamen, /private/var).
pub fn aus_eigener_ablage(pfade: &[PathBuf], basis: &Path) -> bool {
    let echt = fs::canonicalize(basis).ok();
    pfade.iter().any(|p| {
        p.starts_with(basis)
            || match (&echt, fs::canonicalize(p)) {
                (Some(b), Ok(p)) => p.starts_with(b),
                _ => false,
            }
    })
}

/// "<unix-ms>-<kennung>", sonst None.
fn name_lesen(name: &str) -> Option<(u64, u32)> {
    let (ms, k) = name.split_once('-')?;
    let ziffern = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !ziffern(ms) || !ziffern(k) {
        return None;
    }
    Some((ms.parse().ok()?, k.parse().ok()?))
}

/// Raeumt die Basis auf: Verzeichnisse aelter als `hoechstalter` (nach dem
/// Zeitstempel im Namen) gehen immer, von den uebrigen fertigen bleiben die
/// `behalten` neuesten. Laufende (mit Marke) werden weder gezaehlt noch
/// geloescht. Nur Eintraege der Form <ms>-<kennung> bzw. <ms>-<kennung>.laeuft
/// werden angefasst, Verknuepfungen nie (auch nicht beim Loeschen darin:
/// remove_dir_all folgt ihnen nicht). Liefert die Zahl der geloeschten
/// Verzeichnisse.
pub fn aufraeumen(basis: &Path, behalten: usize, hoechstalter: Option<Duration>) -> usize {
    aufraeumen_zu(basis, behalten, hoechstalter, unix_ms())
}

/// Beim Programmstart: alles in der Basis, was aelter als 24 h ist.
pub fn aufraeumen_beim_start() -> usize {
    aufraeumen(&ablage_basis(), usize::MAX, Some(HOECHSTALTER))
}

fn aufraeumen_zu(basis: &Path, behalten: usize, hoechstalter: Option<Duration>, jetzt_ms: u64) -> usize {
    match fs::symlink_metadata(basis) {
        Ok(md) if md.is_dir() => {}
        _ => return 0,
    }
    let Ok(rd) = fs::read_dir(basis) else { return 0 };
    let mut ordner: Vec<(u64, u32, PathBuf, String)> = Vec::new();
    let mut marken: Vec<(u64, PathBuf, String)> = Vec::new();
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let Ok(art) = e.file_type() else { continue };
        if let Some(stamm) = name.strip_suffix(MARKE_ENDUNG) {
            if art.is_file() {
                if let Some((ms, _)) = name_lesen(stamm) {
                    marken.push((ms, e.path(), stamm.to_string()));
                }
            }
        } else if art.is_dir() && !art.is_symlink() {
            if let Some((ms, k)) = name_lesen(&name) {
                ordner.push((ms, k, e.path(), name));
            }
        }
    }
    let alt = |ms: u64| hoechstalter.is_some_and(|h| jetzt_ms.saturating_sub(ms) > h.as_millis() as u64);
    let laufend: HashSet<&str> =
        marken.iter().filter(|m| !alt(m.0)).map(|m| m.2.as_str()).collect();
    let mut geloescht = 0;
    let mut fertige = Vec::new();
    for o in &ordner {
        if alt(o.0) {
            if fs::remove_dir_all(&o.2).is_ok() {
                geloescht += 1;
            }
        } else if !laufend.contains(o.3.as_str()) {
            fertige.push(o);
        }
    }
    for m in &marken {
        if alt(m.0) {
            let _ = fs::remove_file(&m.1);
        }
    }
    fertige.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for o in fertige.iter().skip(behalten) {
        if fs::remove_dir_all(&o.2).is_ok() {
            geloescht += 1;
        }
    }
    geloescht
}

/// Ein Ordner, frisch (nicht rekursiv); unter Unix 0700.
fn ordner_anlegen(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(p);
    #[cfg(not(unix))]
    return fs::create_dir(p);
}

/// Legt Marke und Uebertragungsverzeichnis an, beide frisch (create_new bzw.
/// create_dir). Liefert (Verzeichnis, Marke).
fn verzeichnis_anlegen(basis: &Path, kennung: u32) -> io::Result<(PathBuf, PathBuf)> {
    let ms = unix_ms();
    for versuch in 0..100u64 {
        let name = format!("{}-{kennung}", ms + versuch);
        let marke = basis.join(format!("{name}{MARKE_ENDUNG}"));
        match OpenOptions::new().write(true).create_new(true).open(&marke) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        let v = basis.join(&name);
        match ordner_anlegen(&v) {
            Ok(()) => return Ok((v, marke)),
            Err(e) => {
                let _ = fs::remove_file(&marke);
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
        }
    }
    Err(io::Error::other("kein freier Name fuer das Uebertragungsverzeichnis"))
}

fn verzeichnis_loeschen(p: &Path) -> io::Result<()> {
    for versuch in 0..2 {
        match fs::remove_dir_all(p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            // Unter Windows haelt etwa ein Virenpruefer eine frische Datei
            // kurz offen.
            Err(_) if versuch == 0 => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Oeffnet eine vom Empfaenger angelegte Datei zum Schreiben, ohne einer
/// Verknuepfung zu folgen (macOS O_NOFOLLOW, Windows
/// FILE_FLAG_OPEN_REPARSE_POINT), und nur, wenn es eine gewoehnliche Datei ist.
fn zum_schreiben_oeffnen(p: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true);
    #[cfg(target_os = "macos")]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, 0x0100);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(&mut o, 0x0020_0000);
    let f = o.open(p)?;
    let md = f.metadata()?;
    if !md.is_file() || md.file_type().is_symlink() {
        return Err(io::Error::other("keine gewoehnliche Datei"));
    }
    Ok(f)
}

/// Freier Platz fuer den Aufrufer am Ort `pfad` (einem vorhandenen Ordner).
#[cfg(windows)]
pub fn freier_platz(pfad: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w: Vec<u16> = pfad.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut frei = 0u64;
    // SAFETY: w ist mit NUL abgeschlossen und lebt bis nach dem Aufruf; frei
    // ist ein gueltiger Zeiger auf ein u64.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut frei), None, None) }.ok()?;
    Some(frei)
}

/// macOS: keine Vorabpruefung (2.6); ein Schreibfehler bricht sauber ab
/// (Quittung 5).
#[cfg(not(windows))]
pub fn freier_platz(_pfad: &Path) -> Option<u64> {
    None
}

// ============================================================ Auflisten

/// Was der Sender aus den kopierten Pfaden macht (2.7 Schritt 3).
#[derive(Clone, Debug, Default)]
pub struct Liste {
    /// Fertig fuer das Angebot: Vorfahr vor Nachfahr, Namen sortiert.
    pub eintraege: Vec<Eintrag>,
    /// Je Eintrag die Quelle auf der Platte.
    pub quellen: Vec<PathBuf>,
    pub gesamt: u64,
    /// Protokollzeilen fuer Uebersprungenes.
    pub hinweise: Vec<String>,
    /// Some: ueber einer Grenze, es wird nichts gesendet (FilesTooLarge).
    pub zu_gross: Option<String>,
}

struct Auflister {
    liste: Liste,
    g: Grenzen,
    /// Schon vergebene Pfade, streng bereinigt und klein geschrieben.
    gesehen: HashSet<String>,
    /// Laenge des Angebots bis hier.
    laenge: usize,
}

/// Listet die kopierten Pfade rekursiv auf. Verknuepfungen (auch Junctions),
/// besondere Dateien und fuer die Gegenseite ungueltige oder doppelte Namen
/// werden uebersprungen, jeweils mit Hinweis. Ueber EINTRAEGE_MAX,
/// GESAMT_MAX oder ANGEBOT_MAX: zu_gross, und die Suche endet.
pub fn auflisten(pfade: &[PathBuf]) -> Liste {
    auflisten_mit(pfade, Grenzen::default())
}

/// Wie auflisten(), mit eigenen Grenzen (Tests).
pub fn auflisten_mit(pfade: &[PathBuf], g: Grenzen) -> Liste {
    let mut a = Auflister { liste: Liste::default(), g, gesehen: HashSet::new(), laenge: ANGEBOT_KOPF };
    for p in pfade {
        if a.liste.zu_gross.is_some() {
            break;
        }
        let Some(name) = p.file_name() else {
            a.hinweis("kein Dateiname", p);
            continue;
        };
        let Some(name) = name.to_str() else {
            a.hinweis("Name nicht in UTF-8", p);
            continue;
        };
        a.eintrag(p, name.to_string(), 1);
    }
    a.liste
}

impl Auflister {
    fn hinweis(&mut self, was: &str, p: &Path) {
        self.liste.hinweise.push(format!("Dateien: uebersprungen ({was}): {}", p.display()));
    }

    fn eintrag(&mut self, quelle: &Path, rel: String, tiefe: usize) {
        let md = match fs::symlink_metadata(quelle) {
            Ok(m) => m,
            Err(e) => return self.hinweis(&format!("nicht lesbar: {e}"), quelle),
        };
        let art = md.file_type();
        if art.is_symlink() {
            return self.hinweis("symbolische Verknuepfung", quelle);
        }
        if !art.is_file() && !art.is_dir() {
            return self.hinweis("keine gewoehnliche Datei", quelle);
        }
        let name = rel.rsplit('/').next().unwrap_or("");
        if name.contains('\\') || name.contains('\0') || name.len() > BESTANDTEIL_MAX {
            return self.hinweis("Name fuer die Gegenseite ungueltig", quelle);
        }
        if rel.len() > PFAD_MAX || tiefe > TIEFE_MAX {
            return self.hinweis("Pfad zu lang oder zu tief", quelle);
        }
        let schluessel = rel
            .split('/')
            .map(|t| bestandteil_bereinigen(t, Regeln::Windows))
            .collect::<Vec<_>>()
            .join("/")
            .to_lowercase();
        if !self.gesehen.insert(schluessel) {
            return self.hinweis("doppelter Name, nur der erste geht hinaus", quelle);
        }
        let (art, groesse) =
            if art.is_dir() { (EintragArt::Ordner, 0) } else { (EintragArt::Datei, md.len()) };
        self.laenge += EINTRAG_KOPF + rel.len();
        self.liste.gesamt = self.liste.gesamt.saturating_add(groesse);
        self.liste.eintraege.push(Eintrag { art, pfad: rel.clone(), groesse });
        self.liste.quellen.push(quelle.to_path_buf());
        if self.liste.eintraege.len() > self.g.eintraege as usize {
            self.liste.zu_gross = Some(format!("mehr als {} Eintraege", self.g.eintraege));
        } else if self.liste.gesamt > self.g.gesamt {
            self.liste.zu_gross =
                Some(format!("{} MB, hoechstens {} MB", mb_text(self.liste.gesamt), mb_text(self.g.gesamt)));
        } else if self.laenge > self.g.angebot {
            self.liste.zu_gross = Some(format!("Dateiliste ueber {} Byte", self.g.angebot));
        }
        if self.liste.zu_gross.is_some() || art != EintragArt::Ordner {
            return;
        }
        let mut kinder: Vec<(String, PathBuf)> = Vec::new();
        match fs::read_dir(quelle) {
            Ok(rd) => {
                for e in rd {
                    match e {
                        Ok(e) => match e.file_name().into_string() {
                            Ok(n) => kinder.push((n, e.path())),
                            Err(_) => self.hinweis("Name nicht in UTF-8", &e.path()),
                        },
                        Err(err) => self.hinweis(&format!("Ordner nicht ganz lesbar: {err}"), quelle),
                    }
                }
            }
            Err(e) => return self.hinweis(&format!("Ordner nicht lesbar: {e}"), quelle),
        }
        kinder.sort_by(|a, b| a.0.cmp(&b.0));
        for (n, p) in kinder {
            if self.liste.zu_gross.is_some() {
                return;
            }
            self.eintrag(&p, format!("{rel}/{n}"), tiefe + 1);
        }
    }
}

// ============================================================ Stand

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Richtung {
    Senden,
    Empfangen,
}

/// Warum eine Uebertragung abbrach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Abbruch {
    /// Hier abgebrochen: neuer Inhalt, Sitzungsende, Zuschauerwechsel.
    Hier,
    /// Der Weg meldete Weg: die Sitzung ist vorbei.
    Verbindung,
    /// Sender: Quelle unlesbar oder kuerzer als angekuendigt (Ende 2).
    Lesefehler(String),
    /// STILLSTAND ohne Fortschritt (Sender Ende 3, Empfaenger Quittung 6).
    Zeitueberschreitung,
    /// Sender: der Empfaenger beendete mit diesem Zustand (3 bis 6, oder 1 zu frueh).
    Quittung(u8),
    /// Empfaenger: der Sender beendete mit diesem Grund (1 bis 3).
    Ende(u8),
    /// Empfaenger: Angebot abgelehnt (Quittung 3 oder 4), mit Grund.
    Abgelehnt(u8, String),
    /// Empfaenger: Protokollfehler nach der Annahme (Quittung 4).
    Ungueltig(String),
    /// Empfaenger: Schreibfehler (Quittung 5).
    Schreibfehler(String),
    /// Empfaenger: der fertig-Rueckruf konnte die Ablage nicht setzen (Quittung 5).
    Ablage,
}

impl Abbruch {
    /// Kurzer deutscher Text fuer das Protokoll.
    pub fn text(&self) -> String {
        match self {
            Abbruch::Hier => "hier abgebrochen".into(),
            Abbruch::Verbindung => "Verbindung weg".into(),
            Abbruch::Lesefehler(t) => format!("Lesefehler: {t}"),
            Abbruch::Zeitueberschreitung => "Zeitueberschreitung, kein Fortschritt".into(),
            Abbruch::Quittung(z) => format!("Gegenseite: {}", zustand_text(*z)),
            Abbruch::Ende(g) => format!("Gegenseite: {}", grund_text(*g)),
            Abbruch::Abgelehnt(_, t) | Abbruch::Ungueltig(t) => t.clone(),
            Abbruch::Schreibfehler(t) => format!("Schreibfehler: {t}"),
            Abbruch::Ablage => "Ablage nicht gesetzt".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ergebnis {
    Laeuft,
    /// Senden: quittiert (FilesSent). Empfangen: in der Ablage (FilesReady).
    Fertig,
    Abgebrochen(Abbruch),
    /// Ueber einer Grenze (FilesTooLarge), auch bei Quittung 2.
    ZuGross,
    /// Nur von der Rolle gesetzt (FilesPeerOld).
    GegenseiteZuAlt,
}

/// Der Stand einer Uebertragung fuer die Oberflaeche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stand {
    pub richtung: Richtung,
    pub kennung: u32,
    /// Senden: quittierte Byte; Empfangen: geschriebene Byte.
    pub bytes: u64,
    pub gesamt: u64,
    /// Alle Eintraege des Angebots (Dateien und Ordner).
    pub eintraege: u32,
    /// Davon Dateien.
    pub dateien: u32,
    /// Oberste Eintraege (so viele liegen am Ende in der Ablage).
    pub oberste: u32,
    pub ergebnis: Ergebnis,
}

impl Stand {
    fn neu(richtung: Richtung, kennung: u32) -> Stand {
        Stand {
            richtung,
            kennung,
            bytes: 0,
            gesamt: 0,
            eintraege: 0,
            dateien: 0,
            oberste: 0,
            ergebnis: Ergebnis::Laeuft,
        }
    }

    fn aus_angebot(richtung: Richtung, a: &Angebot) -> Stand {
        Stand {
            gesamt: a.gesamt,
            eintraege: a.eintraege.len() as u32,
            dateien: a.dateien() as u32,
            oberste: a.oberste() as u32,
            ..Stand::neu(richtung, a.kennung)
        }
    }

    /// Fuer die Rolle: die Gegenseite kann keine Dateien (FilesPeerOld).
    pub fn gegenseite_zu_alt(richtung: Richtung) -> Stand {
        Stand { ergebnis: Ergebnis::GegenseiteZuAlt, ..Stand::neu(richtung, 0) }
    }

    pub fn laeuft(&self) -> bool {
        self.ergebnis == Ergebnis::Laeuft
    }
}

/// Was Sender und Empfaenger melden: einen Stand fuer die Oberflaeche,
/// eine Protokollzeile oder beides.
#[derive(Clone, Debug)]
pub struct Ereignis {
    pub stand: Option<Stand>,
    pub zeile: Option<String>,
}

impl Ereignis {
    fn zeile(z: String) -> Ereignis {
        Ereignis { stand: None, zeile: Some(z) }
    }
    fn stand(s: Stand) -> Ereignis {
        Ereignis { stand: Some(s), zeile: None }
    }
    fn beides(s: Stand, z: String) -> Ereignis {
        Ereignis { stand: Some(s), zeile: Some(z) }
    }
}

// ============================================================ Sender

/// Startet Uebertragungen; siehe Modulkopf.
pub struct Sender;

/// Griff auf eine laufende Uebertragung. Drop bricht ab.
pub struct Griff {
    innen: Arc<SenderInnen>,
}

struct SenderInnen {
    kennung: u32,
    z: Mutex<SenderZustand>,
    cv: Condvar,
}

#[derive(Default)]
struct SenderZustand {
    abbruch: bool,
    angenommen: bool,
    quittiert: u64,
    /// Letzter Fortschritt (Angebot hinaus, erste Quittung, mehr quittiert).
    letzte: Option<Instant>,
    /// Quittung mit Zustand ungleich 0.
    schluss: Option<Quittung>,
    beendet: bool,
}

/// Warum der Lauf endete, wenn nicht mit Quittung 1.
enum Aus {
    Abbruch,
    Weg,
    Stillstand,
    Schluss(Quittung),
    Lesefehler(String),
}

/// Setzt `beendet`, wenn der Faden endet - auch bei einer Panik.
struct SenderEndet<'a>(&'a SenderInnen);

impl Drop for SenderEndet<'_> {
    fn drop(&mut self) {
        sperre(&self.0.z).beendet = true;
        self.0.cv.notify_all();
    }
}

impl Sender {
    pub fn starten(
        pfade: Vec<PathBuf>,
        weg: Arc<dyn Weg>,
        fenster: u64,
        melden: impl Fn(Ereignis) + Send + 'static,
    ) -> Griff {
        Sender::starten_mit(pfade, weg, fenster, melden, Vorgaben::default())
    }

    pub fn starten_mit(
        pfade: Vec<PathBuf>,
        weg: Arc<dyn Weg>,
        fenster: u64,
        melden: impl Fn(Ereignis) + Send + 'static,
        vorgaben: Vorgaben,
    ) -> Griff {
        let innen = Arc::new(SenderInnen {
            kennung: neue_kennung(),
            z: Mutex::new(SenderZustand::default()),
            cv: Condvar::new(),
        });
        let faden = innen.clone();
        let fenster = fenster.max(1);
        let gestartet = std::thread::Builder::new().name("qc-dateien-senden".into()).spawn(move || {
            let _endet = SenderEndet(&faden);
            faden.lauf(&pfade, &*weg, fenster, &melden, &vorgaben);
        });
        if gestartet.is_err() {
            sperre(&innen.z).beendet = true;
        }
        Griff { innen }
    }
}

impl Griff {
    pub fn kennung(&self) -> u32 {
        self.innen.kennung
    }

    /// Nutzlast einer DATEI_QUITTUNG, wie sie ankommt. Kehrt sofort zurueck.
    pub fn quittung(&self, nutzlast: &[u8]) {
        let Some(q) = Quittung::lesen(nutzlast) else { return };
        if q.kennung != self.innen.kennung {
            return;
        }
        let mut z = sperre(&self.innen.z);
        if z.beendet || z.schluss.is_some() {
            return;
        }
        if q.zustand == ZUSTAND_LAEUFT {
            // Nur Fortschritt zaehlt als neue Quittung; eine Wiederholung
            // desselben Standes haelt den Sender nicht am Leben.
            if !z.angenommen || q.empfangen > z.quittiert {
                z.angenommen = true;
                z.quittiert = z.quittiert.max(q.empfangen);
                z.letzte = Some(Instant::now());
            }
        } else {
            z.schluss = Some(q);
        }
        drop(z);
        self.innen.cv.notify_all();
    }

    /// Bricht ab und kehrt sofort zurueck; der Faden schickt Ende 1, falls
    /// noetig, und endet.
    pub fn abbrechen(&self) {
        sperre(&self.innen.z).abbruch = true;
        self.innen.cv.notify_all();
    }

    pub fn laeuft(&self) -> bool {
        !sperre(&self.innen.z).beendet
    }

    /// Wartet hoechstens `frist` auf das Ende des Fadens.
    pub fn abwarten(&self, frist: Duration) -> bool {
        let bis = Instant::now() + frist;
        let mut z = sperre(&self.innen.z);
        while !z.beendet {
            let jetzt = Instant::now();
            if jetzt >= bis {
                return false;
            }
            z = warten(&self.innen.cv, z, bis - jetzt);
        }
        true
    }
}

impl Drop for Griff {
    fn drop(&mut self) {
        self.abbrechen();
    }
}

fn voll_lesen(f: &mut File, puffer: &mut [u8]) -> io::Result<bool> {
    let mut da = 0;
    while da < puffer.len() {
        match f.read(&mut puffer[da..]) {
            Ok(0) => return Ok(false),
            Ok(n) => da += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

impl SenderInnen {
    fn frist_neu(&self) {
        sperre(&self.z).letzte = Some(Instant::now());
    }

    fn quittiert(&self) -> u64 {
        sperre(&self.z).quittiert
    }

    fn frist(z: &SenderZustand, v: &Vorgaben) -> Instant {
        z.letzte.unwrap_or_else(Instant::now) + v.stillstand
    }

    /// Abbruch oder Schluss vom Empfaenger?
    fn pruefen(z: &SenderZustand) -> Result<(), Aus> {
        if z.abbruch {
            return Err(Aus::Abbruch);
        }
        if let Some(q) = z.schluss {
            return Err(Aus::Schluss(q));
        }
        Ok(())
    }

    /// Sendet; bei Voll wird gewartet (voll_warten, geweckt von Abbruch und
    /// Quittung), bis Ja, Weg, Abbruch, Schluss oder Stillstand.
    fn senden(&self, weg: &dyn Weg, typ: u8, n: &[u8], v: &Vorgaben) -> Result<(), Aus> {
        loop {
            {
                let z = sperre(&self.z);
                SenderInnen::pruefen(&z)?;
                if Instant::now() >= SenderInnen::frist(&z, v) {
                    return Err(Aus::Stillstand);
                }
            }
            match weg.senden(typ, n) {
                Gesendet::Ja => return Ok(()),
                Gesendet::Weg => return Err(Aus::Weg),
                // Schlafen statt Condvar mit Frist: unter Windows dauert eine
                // Condvar-Frist von 2 ms bis zur Timeraufloesung (~15 ms),
                // thread::sleep ist dort genau. Ein Abbruch wirkt nach
                // hoechstens einem Schlaf.
                Gesendet::Voll => std::thread::sleep(v.voll_warten),
            }
        }
    }

    /// Wartet, bis unter dem Fenster Platz ist; liefert den freien Platz.
    fn fenster_abwarten(&self, gesendet: u64, fenster: u64, v: &Vorgaben) -> Result<u64, Aus> {
        let mut z = sperre(&self.z);
        loop {
            SenderInnen::pruefen(&z)?;
            let unterwegs = gesendet.saturating_sub(z.quittiert);
            if unterwegs < fenster {
                return Ok(fenster - unterwegs);
            }
            let frist = SenderInnen::frist(&z, v);
            let jetzt = Instant::now();
            if jetzt >= frist {
                return Err(Aus::Stillstand);
            }
            z = warten(&self.cv, z, frist - jetzt);
        }
    }

    /// Nach Ende 0: auf Quittung 1 warten.
    fn schluss_abwarten(&self, v: &Vorgaben) -> Result<(), Aus> {
        let mut z = sperre(&self.z);
        loop {
            if z.abbruch {
                return Err(Aus::Abbruch);
            }
            if let Some(q) = z.schluss {
                return if q.zustand == ZUSTAND_FERTIG { Ok(()) } else { Err(Aus::Schluss(q)) };
            }
            let frist = SenderInnen::frist(&z, v);
            let jetzt = Instant::now();
            if jetzt >= frist {
                return Err(Aus::Stillstand);
            }
            z = warten(&self.cv, z, frist - jetzt);
        }
    }

    /// Ende nach einem Abbruch: bei Voll hoechstens ende_frist lang.
    fn ende_senden(&self, weg: &dyn Weg, grund: u8, v: &Vorgaben) {
        let n = Ende { kennung: self.kennung, grund }.kodieren();
        let bis = Instant::now() + v.ende_frist;
        loop {
            match weg.senden(DATEI_ENDE, &n) {
                Gesendet::Ja | Gesendet::Weg => return,
                Gesendet::Voll => {
                    if Instant::now() >= bis {
                        return;
                    }
                    std::thread::sleep(v.voll_warten);
                }
            }
        }
    }

    fn lauf(&self, pfade: &[PathBuf], weg: &dyn Weg, fenster: u64, melden: &dyn Fn(Ereignis), v: &Vorgaben) {
        if aus_eigener_ablage(pfade, &ablage_basis()) {
            melden(Ereignis::zeile("Dateien: nicht gesendet, sie stammen aus einem Empfang".into()));
            return;
        }
        let liste = auflisten_mit(pfade, v.grenzen);
        for h in &liste.hinweise {
            melden(Ereignis::zeile(h.clone()));
        }
        if let Some(grund) = &liste.zu_gross {
            let st = Stand {
                gesamt: liste.gesamt,
                eintraege: liste.eintraege.len() as u32,
                ergebnis: Ergebnis::ZuGross,
                ..Stand::neu(Richtung::Senden, self.kennung)
            };
            melden(Ereignis::beides(st, format!("Dateien: nicht gesendet, zu gross ({grund})")));
            return;
        }
        if liste.eintraege.is_empty() {
            melden(Ereignis::zeile("Dateien: nichts zu senden".into()));
            return;
        }
        if sperre(&self.z).abbruch {
            return;
        }
        let angebot = Angebot { kennung: self.kennung, gesamt: liste.gesamt, eintraege: liste.eintraege };
        let mut stand = Stand::aus_angebot(Richtung::Senden, &angebot);
        let t0 = Instant::now();
        let mut hinaus = false;
        match self.uebertragen(&angebot, &liste.quellen, weg, fenster, melden, v, &mut stand, &mut hinaus) {
            Ok(()) => {
                stand.bytes = stand.gesamt;
                stand.ergebnis = Ergebnis::Fertig;
                let z = format!(
                    "Dateien: gesendet und quittiert ({} MB in {} s)",
                    mb_text(stand.gesamt),
                    sekunden_text(t0.elapsed())
                );
                melden(Ereignis::beides(stand, z));
            }
            Err(aus) => {
                let (ergebnis, ende) = match aus {
                    Aus::Abbruch => (Ergebnis::Abgebrochen(Abbruch::Hier), Some(GRUND_ABGEBROCHEN)),
                    Aus::Weg => (Ergebnis::Abgebrochen(Abbruch::Verbindung), None),
                    Aus::Stillstand => (Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung), Some(GRUND_ZEIT)),
                    Aus::Lesefehler(t) => (Ergebnis::Abgebrochen(Abbruch::Lesefehler(t)), Some(GRUND_LESEFEHLER)),
                    Aus::Schluss(q) if q.zustand == ZUSTAND_ZU_GROSS => (Ergebnis::ZuGross, None),
                    Aus::Schluss(q) => (Ergebnis::Abgebrochen(Abbruch::Quittung(q.zustand)), None),
                };
                // Ende nur, wenn das Angebot hinaus ist und der Empfaenger
                // nicht selbst schon beendet hat.
                if let (true, Some(g)) = (hinaus, ende) {
                    self.ende_senden(weg, g, v);
                }
                let zeile = match &ergebnis {
                    Ergebnis::ZuGross => "Dateien: abgelehnt (Gegenseite: zu gross oder zu viele Eintraege)".to_string(),
                    Ergebnis::Abgebrochen(Abbruch::Quittung(z))
                        if matches!(*z, ZUSTAND_KEIN_PLATZ | ZUSTAND_UNGUELTIG) =>
                    {
                        format!("Dateien: abgelehnt (Gegenseite: {})", zustand_text(*z))
                    }
                    Ergebnis::Abgebrochen(a) => format!("Dateien: abgebrochen ({})", a.text()),
                    _ => String::new(),
                };
                stand.bytes = self.quittiert().min(stand.gesamt);
                stand.ergebnis = ergebnis;
                melden(Ereignis::beides(stand, zeile));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn uebertragen(
        &self,
        angebot: &Angebot,
        quellen: &[PathBuf],
        weg: &dyn Weg,
        fenster: u64,
        melden: &dyn Fn(Ereignis),
        v: &Vorgaben,
        stand: &mut Stand,
        hinaus: &mut bool,
    ) -> Result<(), Aus> {
        self.frist_neu();
        self.senden(weg, DATEI_ANGEBOT, &angebot.kodieren(), v)?;
        *hinaus = true;
        self.frist_neu();
        melden(Ereignis::beides(
            stand.clone(),
            format!("Dateien: sende {} Eintraege, {} MB", angebot.eintraege.len(), mb_text(angebot.gesamt)),
        ));
        let mut gesendet = 0u64;
        let mut puffer = vec![0u8; STUECK_MAX];
        let mut rahmen = Vec::with_capacity(STUECK_KOPF + STUECK_MAX);
        let mut gemeldet = Instant::now();
        for (i, e) in angebot.eintraege.iter().enumerate() {
            if e.art != EintragArt::Datei || e.groesse == 0 {
                continue;
            }
            let quelle = &quellen[i];
            let mut f = File::open(quelle)
                .map_err(|err| Aus::Lesefehler(format!("{}: {err}", quelle.display())))?;
            let mut versatz = 0u64;
            while versatz < e.groesse {
                let frei = self.fenster_abwarten(gesendet, fenster, v)?;
                // Notfalls kuerzer als STUECK_MAX, damit nie mehr als das
                // Fenster unterwegs ist - und ein Empfaenger, der erst nach
                // 64 KiB quittiert, bei einem Fenster von 64 KiB auch dort
                // ankommt.
                let n = (STUECK_MAX as u64).min(frei).min(e.groesse - versatz) as usize;
                match voll_lesen(&mut f, &mut puffer[..n]) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(Aus::Lesefehler(format!(
                            "{}: kuerzer als angekuendigt",
                            quelle.display()
                        )))
                    }
                    Err(err) => return Err(Aus::Lesefehler(format!("{}: {err}", quelle.display()))),
                }
                Stueck { kennung: self.kennung, eintrag: i as u32, versatz, daten: &puffer[..n] }
                    .kodieren_in(&mut rahmen);
                self.senden(weg, DATEI_STUECK, &rahmen, v)?;
                gesendet += n as u64;
                versatz += n as u64;
                if gemeldet.elapsed() >= FORTSCHRITT_TAKT {
                    gemeldet = Instant::now();
                    stand.bytes = self.quittiert().min(stand.gesamt);
                    melden(Ereignis::stand(stand.clone()));
                }
            }
        }
        self.senden(weg, DATEI_ENDE, &Ende { kennung: self.kennung, grund: GRUND_VOLLSTAENDIG }.kodieren(), v)?;
        self.schluss_abwarten(v)
    }
}

// ============================================================ Empfaenger

/// Nimmt Uebertragungen einer Sitzung an; siehe Modulkopf. Drop bricht ab.
pub struct Empfaenger {
    innen: Arc<EmpfInnen>,
}

struct EmpfInnen {
    q: Mutex<EmpfQ>,
    cv: Condvar,
}

#[derive(Default)]
struct EmpfQ {
    eingang: VecDeque<Eingang>,
    /// Wartende Bytes (Stuecke und Enden, mit Aufschlag).
    bytes: usize,
    ueberlauf: bool,
    abbruch: bool,
    beendet: bool,
}

enum Eingang {
    Angebot(Vec<u8>),
    Stueck(Vec<u8>),
    Ende(Vec<u8>),
    /// Die Warteschlange lief ueber; die Kennung des verworfenen Stuecks.
    Ueberlauf(Option<u32>),
}

/// Aufschlag je wartender Nachricht, damit auch viele kleine begrenzt sind.
const AUFSCHLAG: usize = 64;

struct EmpfEndet<'a>(&'a EmpfInnen);

impl Drop for EmpfEndet<'_> {
    fn drop(&mut self) {
        sperre(&self.0.q).beendet = true;
        self.0.cv.notify_all();
    }
}

impl Empfaenger {
    pub fn neu(
        basis: PathBuf,
        weg: Arc<dyn Weg>,
        fertig: impl Fn(Vec<PathBuf>) -> bool + Send + 'static,
        melden: impl Fn(Ereignis) + Send + 'static,
    ) -> Empfaenger {
        Empfaenger::neu_mit(basis, weg, fertig, melden, Vorgaben::default())
    }

    pub fn neu_mit(
        basis: PathBuf,
        weg: Arc<dyn Weg>,
        fertig: impl Fn(Vec<PathBuf>) -> bool + Send + 'static,
        melden: impl Fn(Ereignis) + Send + 'static,
        vorgaben: Vorgaben,
    ) -> Empfaenger {
        let innen = Arc::new(EmpfInnen { q: Mutex::new(EmpfQ::default()), cv: Condvar::new() });
        let faden = innen.clone();
        let gestartet = std::thread::Builder::new().name("qc-dateien-empfang".into()).spawn(move || {
            let _endet = EmpfEndet(&faden);
            let mut s = Schreiber {
                basis,
                weg,
                fertig: Box::new(fertig),
                melden: Box::new(melden),
                v: vorgaben,
                ue: None,
            };
            s.lauf(&faden);
        });
        if gestartet.is_err() {
            sperre(&innen.q).beendet = true;
        }
        Empfaenger { innen }
    }

    /// Legt eine Nachricht (50, 51, 52) in die Warteschlange und kehrt sofort
    /// zurueck. Ein neues Angebot verwirft alles, was davor noch wartet: der
    /// Sender hat die fruehere Uebertragung damit aufgegeben.
    pub fn nachricht(&self, typ: u8, nutzlast: Vec<u8>) {
        let mut q = sperre(&self.innen.q);
        if q.abbruch || q.beendet {
            return;
        }
        match typ {
            DATEI_ANGEBOT => {
                q.eingang.clear();
                q.bytes = 0;
                q.ueberlauf = false;
                q.eingang.push_back(Eingang::Angebot(nutzlast));
            }
            DATEI_STUECK | DATEI_ENDE => {
                if q.ueberlauf {
                    return;
                }
                let b = nutzlast.len() + AUFSCHLAG;
                if q.bytes + b > WARTESCHLANGE_MAX {
                    q.ueberlauf = true;
                    let k = kennung_lesen(&nutzlast);
                    q.eingang.push_back(Eingang::Ueberlauf(k));
                } else {
                    q.bytes += b;
                    q.eingang.push_back(if typ == DATEI_STUECK {
                        Eingang::Stueck(nutzlast)
                    } else {
                        Eingang::Ende(nutzlast)
                    });
                }
            }
            _ => return,
        }
        drop(q);
        self.innen.cv.notify_all();
    }

    /// Verwirft die laufende Uebertragung samt Verzeichnis und beendet den
    /// Faden. Kehrt sofort zurueck.
    pub fn abbrechen(&self) {
        let mut q = sperre(&self.innen.q);
        q.abbruch = true;
        q.eingang.clear();
        q.bytes = 0;
        drop(q);
        self.innen.cv.notify_all();
    }

    pub fn laeuft(&self) -> bool {
        !sperre(&self.innen.q).beendet
    }

    /// Wartet hoechstens `frist` auf das Ende des Fadens.
    pub fn abwarten(&self, frist: Duration) -> bool {
        let bis = Instant::now() + frist;
        let mut q = sperre(&self.innen.q);
        while !q.beendet {
            let jetzt = Instant::now();
            if jetzt >= bis {
                return false;
            }
            q = warten(&self.innen.cv, q, bis - jetzt);
        }
        true
    }
}

impl Drop for Empfaenger {
    fn drop(&mut self) {
        self.abbrechen();
    }
}

/// Eine angenommene Uebertragung.
struct Uebertragung {
    kennung: u32,
    angebot: Angebot,
    /// Je Eintrag das Ziel auf der Platte.
    ziele: Vec<PathBuf>,
    verzeichnis: PathBuf,
    marke: PathBuf,
    /// Index der naechsten Datei mit Daten; eintraege.len() = alles da.
    naechster: usize,
    /// Davon schon geschrieben.
    im_eintrag: u64,
    empfangen: u64,
    quittiert: u64,
    datei: Option<File>,
    letzte: Instant,
    gemeldet: Instant,
    stand: Stand,
}

/// Naechste Datei mit Daten ab `ab`.
fn naechste_datei(a: &Angebot, ab: usize) -> usize {
    (ab..a.eintraege.len())
        .find(|&i| a.eintraege[i].art == EintragArt::Datei && a.eintraege[i].groesse > 0)
        .unwrap_or(a.eintraege.len())
}

impl Uebertragung {
    /// Schliesst, loescht Verzeichnis und Marke; liefert den Stand und ggf.
    /// eine Zeile, falls das Loeschen scheiterte.
    fn verwerfen(mut self) -> (Stand, Option<String>) {
        self.datei = None;
        let hinweis = verzeichnis_loeschen(&self.verzeichnis).err().map(|e| {
            format!("Dateien: Verzeichnis liess sich nicht loeschen ({}): {e}", self.verzeichnis.display())
        });
        let _ = fs::remove_file(&self.marke);
        self.stand.bytes = self.empfangen;
        (self.stand, hinweis)
    }

    /// Prueft ein Stueck gegen die erwartete Stelle.
    fn pruefen(&self, s: &Stueck) -> Result<(), String> {
        let i = s.eintrag as usize;
        let Some(e) = self.angebot.eintraege.get(i) else {
            return Err(format!("Stueck fuer Eintrag {i}, den es nicht gibt"));
        };
        if e.art != EintragArt::Datei {
            return Err(format!("Stueck fuer den Ordner {i}"));
        }
        if i != self.naechster {
            return Err(format!("Stueck ausser der Reihe (Eintrag {i}, erwartet {})", self.naechster));
        }
        if s.versatz != self.im_eintrag {
            return Err(format!("Stueck an Versatz {}, erwartet {}", s.versatz, self.im_eintrag));
        }
        if s.versatz + s.daten.len() as u64 > e.groesse {
            return Err(format!("Stueck ueber die Groesse von Eintrag {i} hinaus"));
        }
        Ok(())
    }

    fn schreiben(&mut self, s: &Stueck) -> Result<(), String> {
        let i = s.eintrag as usize;
        if self.datei.is_none() {
            let f = zum_schreiben_oeffnen(&self.ziele[i])
                .map_err(|e| format!("{}: {e}", self.ziele[i].display()))?;
            self.datei = Some(f);
        }
        if let Some(f) = self.datei.as_mut() {
            f.write_all(s.daten).map_err(|e| format!("{}: {e}", self.ziele[i].display()))?;
        }
        let n = s.daten.len() as u64;
        self.im_eintrag += n;
        self.empfangen += n;
        self.letzte = Instant::now();
        if self.im_eintrag == self.angebot.eintraege[i].groesse {
            self.datei = None;
            self.naechster = naechste_datei(&self.angebot, i + 1);
            self.im_eintrag = 0;
        }
        Ok(())
    }
}

enum Holen {
    Nachricht(Eingang),
    Quittieren,
    Stillstand,
    Schluss,
}

/// Der Schreibfaden.
struct Schreiber {
    basis: PathBuf,
    weg: Arc<dyn Weg>,
    fertig: Box<dyn Fn(Vec<PathBuf>) -> bool + Send>,
    melden: Box<dyn Fn(Ereignis) + Send>,
    v: Vorgaben,
    ue: Option<Uebertragung>,
}

impl Schreiber {
    fn lauf(&mut self, innen: &EmpfInnen) {
        loop {
            let holen = {
                let mut q = sperre(&innen.q);
                loop {
                    if q.abbruch {
                        break Holen::Schluss;
                    }
                    if let Some(e) = q.eingang.pop_front() {
                        let b = match &e {
                            Eingang::Stueck(n) | Eingang::Ende(n) => n.len() + AUFSCHLAG,
                            _ => 0,
                        };
                        q.bytes = q.bytes.saturating_sub(b);
                        break Holen::Nachricht(e);
                    }
                    match &self.ue {
                        None => q = warten_ohne_frist(&innen.cv, q),
                        Some(u) => {
                            // Nichts wartet mehr: Unquittiertes jetzt quittieren.
                            if u.empfangen > u.quittiert {
                                break Holen::Quittieren;
                            }
                            let frist = u.letzte + self.v.stillstand;
                            let jetzt = Instant::now();
                            if jetzt >= frist {
                                break Holen::Stillstand;
                            }
                            q = warten(&innen.cv, q, frist - jetzt);
                        }
                    }
                }
            };
            match holen {
                Holen::Schluss => break,
                Holen::Quittieren => self.quittieren(innen),
                Holen::Stillstand => {
                    self.fehler(innen, ZUSTAND_ABGEBROCHEN, Abbruch::Zeitueberschreitung)
                }
                Holen::Nachricht(Eingang::Angebot(n)) => self.angebot(&n, innen),
                Holen::Nachricht(Eingang::Stueck(n)) => self.stueck(&n, innen),
                Holen::Nachricht(Eingang::Ende(n)) => self.ende(&n, innen),
                Holen::Nachricht(Eingang::Ueberlauf(k)) => {
                    if self.aktiv(k) {
                        let t = "mehr als das Fenster unquittiert".to_string();
                        self.fehler(innen, ZUSTAND_UNGUELTIG, Abbruch::Ungueltig(t));
                    }
                }
            }
        }
        // Sitzungsende oder Zuschauerwechsel: verwerfen, ohne Quittung.
        if let Some(u) = self.ue.take() {
            let (st, h) = u.verwerfen();
            self.abgebrochen_melden(st, h, Abbruch::Hier);
        }
    }

    fn aktiv(&self, k: Option<u32>) -> bool {
        matches!((&self.ue, k), (Some(u), Some(k)) if u.kennung == k)
    }

    fn melden(&self, e: Ereignis) {
        (self.melden)(e)
    }

    fn abgebrochen_melden(&self, mut st: Stand, hinweis: Option<String>, grund: Abbruch) {
        let zeile = format!("Dateien: abgebrochen ({})", grund.text());
        st.ergebnis = Ergebnis::Abgebrochen(grund);
        self.melden(Ereignis::beides(st, zeile));
        if let Some(h) = hinweis {
            self.melden(Ereignis::zeile(h));
        }
    }

    /// Schickt eine Quittung; bei Voll hoechstens ende_frist lang (danach
    /// traegt die naechste den Stand mit). false: Weg.
    fn quittung_senden(&self, innen: &EmpfInnen, q: Quittung) -> bool {
        let n = q.kodieren();
        let bis = Instant::now() + self.v.ende_frist;
        loop {
            match self.weg.senden(DATEI_QUITTUNG, &n) {
                Gesendet::Ja => return true,
                Gesendet::Weg => return false,
                Gesendet::Voll => {
                    if Instant::now() >= bis || sperre(&innen.q).abbruch {
                        return true;
                    }
                    std::thread::sleep(self.v.voll_warten);
                }
            }
        }
    }

    fn quittieren(&mut self, innen: &EmpfInnen) {
        let Some(u) = self.ue.as_mut() else { return };
        u.quittiert = u.empfangen;
        let q = Quittung { kennung: u.kennung, zustand: ZUSTAND_LAEUFT, empfangen: u.empfangen };
        if !self.quittung_senden(innen, q) {
            if let Some(u) = self.ue.take() {
                let (st, h) = u.verwerfen();
                self.abgebrochen_melden(st, h, Abbruch::Verbindung);
            }
        }
    }

    /// Beendet die laufende Uebertragung mit einer Quittung ungleich 0 und
    /// loescht ihr Verzeichnis.
    fn fehler(&mut self, innen: &EmpfInnen, zustand: u8, grund: Abbruch) {
        let Some(u) = self.ue.take() else { return };
        self.quittung_senden(innen, Quittung { kennung: u.kennung, zustand, empfangen: u.empfangen });
        let (st, h) = u.verwerfen();
        self.abgebrochen_melden(st, h, grund);
    }

    /// Ein Angebot nicht annehmen: Quittung und Meldung, sonst nichts.
    fn ablehnen(&self, innen: &EmpfInnen, kennung: Option<u32>, zustand: u8, text: String) {
        if let Some(k) = kennung {
            self.quittung_senden(innen, Quittung { kennung: k, zustand, empfangen: 0 });
        }
        let mut st = Stand::neu(Richtung::Empfangen, kennung.unwrap_or(0));
        let praefix = if zustand == ZUSTAND_SCHREIBFEHLER { "abgebrochen" } else { "abgelehnt" };
        let zeile = if zustand == ZUSTAND_SCHREIBFEHLER {
            format!("Dateien: {praefix} (Schreibfehler: {text})")
        } else {
            format!("Dateien: {praefix} ({text})")
        };
        st.ergebnis = match zustand {
            ZUSTAND_ZU_GROSS => Ergebnis::ZuGross,
            ZUSTAND_SCHREIBFEHLER => Ergebnis::Abgebrochen(Abbruch::Schreibfehler(text)),
            _ => Ergebnis::Abgebrochen(Abbruch::Abgelehnt(zustand, text)),
        };
        self.melden(Ereignis::beides(st, zeile));
    }

    fn angebot(&mut self, n: &[u8], innen: &EmpfInnen) {
        if let Some(u) = self.ue.take() {
            let (st, h) = u.verwerfen();
            self.abgebrochen_melden(st, h, Abbruch::Ende(GRUND_ABGEBROCHEN));
        }
        let kennung = kennung_lesen(n);
        let geprueft = Angebot::lesen(n).and_then(|a| {
            let rel = a.pruefen(EIGENE_REGELN)?;
            Ok((a, rel))
        });
        let (a, rel) = match geprueft {
            Ok(x) => x,
            Err(abl) => return self.ablehnen(innen, kennung, abl.zustand(), abl.text().to_string()),
        };
        if let Err(e) = basis_anlegen(&self.basis) {
            let t = format!("Ablage {}: {e}", self.basis.display());
            return self.ablehnen(innen, kennung, ZUSTAND_SCHREIBFEHLER, t);
        }
        if let Some(frei) = (self.v.freier_platz)(&self.basis) {
            let noetig = a.gesamt.saturating_add(PLATZRESERVE);
            if frei < noetig {
                let t = format!("zu wenig Platz: {} MB frei, {} MB noetig", mb_text(frei), mb_text(noetig));
                return self.ablehnen(innen, kennung, ZUSTAND_KEIN_PLATZ, t);
            }
        }
        let (verzeichnis, marke) = match verzeichnis_anlegen(&self.basis, a.kennung) {
            Ok(x) => x,
            Err(e) => {
                let t = format!("Uebertragungsverzeichnis: {e}");
                return self.ablehnen(innen, kennung, ZUSTAND_SCHREIBFEHLER, t);
            }
        };
        let mut ziele = Vec::with_capacity(a.eintraege.len());
        let mut fehler = None;
        for (e, r) in a.eintraege.iter().zip(&rel) {
            let Some(ziel) = sicher_anhaengen(&verzeichnis, r) else {
                fehler = Some((ZUSTAND_UNGUELTIG, format!("kein gueltiger Name: {r:?}")));
                break;
            };
            let angelegt = match e.art {
                EintragArt::Ordner => ordner_anlegen(&ziel),
                EintragArt::Datei => OpenOptions::new().write(true).create_new(true).open(&ziel).map(|_| ()),
            };
            if let Err(err) = angelegt {
                // Schon da: zwei Namen, die das Dateisystem gleich sieht.
                let z = if err.kind() == io::ErrorKind::AlreadyExists {
                    ZUSTAND_UNGUELTIG
                } else {
                    ZUSTAND_SCHREIBFEHLER
                };
                fehler = Some((z, format!("{r}: {err}")));
                break;
            }
            ziele.push(ziel);
        }
        if let Some((z, t)) = fehler {
            let _ = verzeichnis_loeschen(&verzeichnis);
            let _ = fs::remove_file(&marke);
            return self.ablehnen(innen, kennung, z, t);
        }
        let jetzt = Instant::now();
        let u = Uebertragung {
            kennung: a.kennung,
            naechster: naechste_datei(&a, 0),
            stand: Stand::aus_angebot(Richtung::Empfangen, &a),
            angebot: a,
            ziele,
            verzeichnis,
            marke,
            im_eintrag: 0,
            empfangen: 0,
            quittiert: 0,
            datei: None,
            letzte: jetzt,
            gemeldet: jetzt,
        };
        let q = Quittung { kennung: u.kennung, zustand: ZUSTAND_LAEUFT, empfangen: 0 };
        if !self.quittung_senden(innen, q) {
            let (st, h) = u.verwerfen();
            return self.abgebrochen_melden(st, h, Abbruch::Verbindung);
        }
        let zeile = format!(
            "Dateien: empfange {} Eintraege, {} MB",
            u.angebot.eintraege.len(),
            mb_text(u.angebot.gesamt)
        );
        self.melden(Ereignis::beides(u.stand.clone(), zeile));
        self.ue = Some(u);
    }

    fn stueck(&mut self, n: &[u8], innen: &EmpfInnen) {
        // Fremde Kennung: Rest einer verworfenen Uebertragung, still uebergehen.
        if !self.aktiv(kennung_lesen(n)) {
            return;
        }
        let Some(s) = Stueck::lesen(n) else {
            let t = "Stueck mit ungueltiger Laenge".to_string();
            return self.fehler(innen, ZUSTAND_UNGUELTIG, Abbruch::Ungueltig(t));
        };
        let Some(u) = self.ue.as_mut() else { return };
        if let Err(t) = u.pruefen(&s) {
            return self.fehler(innen, ZUSTAND_UNGUELTIG, Abbruch::Ungueltig(t));
        }
        if let Err(t) = u.schreiben(&s) {
            return self.fehler(innen, ZUSTAND_SCHREIBFEHLER, Abbruch::Schreibfehler(t));
        }
        let quittieren = u.empfangen - u.quittiert >= QUITTUNG_ALLE;
        let fortschritt = if u.gemeldet.elapsed() >= FORTSCHRITT_TAKT {
            u.gemeldet = Instant::now();
            u.stand.bytes = u.empfangen;
            Some(u.stand.clone())
        } else {
            None
        };
        if quittieren {
            self.quittieren(innen);
        }
        if let Some(st) = fortschritt {
            self.melden(Ereignis::stand(st));
        }
    }

    fn ende(&mut self, n: &[u8], innen: &EmpfInnen) {
        if !self.aktiv(kennung_lesen(n)) {
            return;
        }
        let Some(e) = Ende::lesen(n) else {
            let t = "Ende zu kurz".to_string();
            return self.fehler(innen, ZUSTAND_UNGUELTIG, Abbruch::Ungueltig(t));
        };
        let Some(mut u) = self.ue.take() else { return };
        if e.grund != GRUND_VOLLSTAENDIG {
            let (st, h) = u.verwerfen();
            return self.abgebrochen_melden(st, h, Abbruch::Ende(e.grund));
        }
        if u.naechster < u.angebot.eintraege.len() {
            self.ue = Some(u);
            let t = "Ende vor dem letzten Stueck".to_string();
            return self.fehler(innen, ZUSTAND_UNGUELTIG, Abbruch::Ungueltig(t));
        }
        u.datei = None;
        let _ = fs::remove_file(&u.marke);
        let oberste: Vec<PathBuf> = u
            .angebot
            .eintraege
            .iter()
            .zip(&u.ziele)
            .filter(|(e, _)| !e.pfad.contains('/'))
            .map(|(_, z)| z.clone())
            .collect();
        if !(self.fertig)(oberste) {
            self.quittung_senden(
                innen,
                Quittung { kennung: u.kennung, zustand: ZUSTAND_SCHREIBFEHLER, empfangen: u.empfangen },
            );
            let (st, h) = u.verwerfen();
            return self.abgebrochen_melden(st, h, Abbruch::Ablage);
        }
        self.quittung_senden(innen, Quittung { kennung: u.kennung, zustand: ZUSTAND_FERTIG, empfangen: u.empfangen });
        aufraeumen(&self.basis, BEHALTEN, None);
        let mut st = u.stand.clone();
        st.bytes = u.empfangen;
        st.ergebnis = Ergebnis::Fertig;
        let zeile = format!(
            "Dateien: empfangen {} Eintraege, {} MB - in die Ablage gelegt",
            u.angebot.eintraege.len(),
            mb_text(u.empfangen)
        );
        self.melden(Ereignis::beides(st, zeile));
    }
}

// ============================================================ Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|w| u8::from_str_radix(w, 16).unwrap()).collect()
    }

    /// Eigener leerer Ordner je Test unter temp_dir; beim Verlassen weg.
    struct Ordner(PathBuf);

    impl Ordner {
        fn neu(name: &str) -> Ordner {
            let d = std::env::temp_dir().join(format!("qc-test-{}-dateien-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Ordner(d)
        }
        fn p(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Ordner {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn vorgaben(stillstand_ms: u64) -> Vorgaben {
        Vorgaben {
            stillstand: Duration::from_millis(stillstand_ms),
            voll_warten: VOLL_WARTEN,
            ende_frist: Duration::from_millis(200),
            freier_platz: |_| None,
            grenzen: Grenzen::default(),
        }
    }

    /// Bis die Bedingung gilt, hoechstens `frist`.
    fn bis(frist: Duration, f: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < frist {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    type Ereignisse = Arc<Mutex<Vec<Ereignis>>>;

    fn sammler() -> (Ereignisse, impl Fn(Ereignis) + Send + 'static) {
        let e: Ereignisse = Arc::new(Mutex::new(Vec::new()));
        let e2 = e.clone();
        (e, move |x| sperre(&e2).push(x))
    }

    fn ergebnisse(e: &Ereignisse) -> Vec<Ergebnis> {
        sperre(e).iter().filter_map(|x| x.stand.as_ref().map(|s| s.ergebnis.clone())).collect()
    }

    fn zeilen(e: &Ereignisse) -> Vec<String> {
        sperre(e).iter().filter_map(|x| x.zeile.clone()).collect()
    }

    fn letztes(e: &Ereignisse) -> Option<Ergebnis> {
        ergebnisse(e).last().cloned()
    }

    /// Ein Weg, der alles mitschreibt. `voll` antwortet Voll (ohne
    /// Mitschreiben), `antworten` gibt die naechsten Antworten vor.
    #[derive(Default)]
    struct Rekorder {
        n: Mutex<Vec<(u8, Vec<u8>)>>,
        antworten: Mutex<VecDeque<Gesendet>>,
        voll: std::sync::atomic::AtomicBool,
        versuche: std::sync::atomic::AtomicUsize,
    }

    impl Weg for Rekorder {
        fn senden(&self, typ: u8, nutzlast: &[u8]) -> Gesendet {
            self.versuche.fetch_add(1, Ordering::SeqCst);
            if self.voll.load(Ordering::SeqCst) {
                return Gesendet::Voll;
            }
            let a = sperre(&self.antworten).pop_front().unwrap_or(Gesendet::Ja);
            if a == Gesendet::Ja {
                sperre(&self.n).push((typ, nutzlast.to_vec()));
            }
            a
        }
    }

    impl Rekorder {
        fn neu() -> Arc<Rekorder> {
            Arc::new(Rekorder::default())
        }
        fn alle(&self) -> Vec<(u8, Vec<u8>)> {
            sperre(&self.n).clone()
        }
        fn quittungen(&self) -> Vec<Quittung> {
            self.alle().iter().filter(|(t, _)| *t == DATEI_QUITTUNG).map(|(_, n)| Quittung::lesen(n).unwrap()).collect()
        }
        fn hat_quittung(&self, zustand: u8) -> bool {
            self.quittungen().iter().any(|q| q.zustand == zustand)
        }
        fn enden(&self) -> Vec<Ende> {
            self.alle().iter().filter(|(t, _)| *t == DATEI_ENDE).map(|(_, n)| Ende::lesen(n).unwrap()).collect()
        }
        fn datenbytes(&self) -> u64 {
            self.alle()
                .iter()
                .filter(|(t, _)| *t == DATEI_STUECK)
                .map(|(_, n)| Stueck::lesen(n).unwrap().daten.len() as u64)
                .sum()
        }
    }

    /// Unterverzeichnisse und Marken in der Basis.
    fn inhalt(basis: &Path) -> Vec<String> {
        let mut v: Vec<String> = match fs::read_dir(basis) {
            Ok(rd) => rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
            Err(_) => Vec::new(),
        };
        v.sort();
        v
    }

    fn datei(p: &Path, inhalt: &[u8]) {
        fs::write(p, inhalt).unwrap();
    }

    fn muster(n: usize, saat: u32) -> Vec<u8> {
        let mut x = saat.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    // ------------------------------------------------------ Pruefvektoren 2.4

    #[test]
    fn pruefvektor_faehigkeiten() {
        assert_eq!(faehigkeiten_kodieren(FAEHIG_DATEIEN), hex("01 00 00 00"));
        assert_eq!(faehigkeiten_lesen(&hex("01 00 00 00")), Some(1));
        // Weitere Bytes werden uebergangen, zu kurz ist nichts.
        assert_eq!(faehigkeiten_lesen(&hex("01 00 00 00 ff ee")), Some(1));
        assert_eq!(faehigkeiten_lesen(&hex("01 00 00")), None);
        assert!(kann_dateien(1) && !kann_dateien(2));
    }

    fn vektor_angebot() -> Angebot {
        Angebot {
            kennung: 7,
            gesamt: 5,
            eintraege: vec![
                Eintrag { art: EintragArt::Ordner, pfad: "Bilder".into(), groesse: 0 },
                Eintrag { art: EintragArt::Datei, pfad: "Bilder/a.txt".into(), groesse: 5 },
                Eintrag { art: EintragArt::Datei, pfad: "b.bin".into(), groesse: 0 },
            ],
        }
    }

    const VEKTOR_ANGEBOT: &str = "07 00 00 00  03 00 00 00  05 00 00 00 00 00 00 00
        01 00 06 00  00 00 00 00 00 00 00 00  42 69 6c 64 65 72
        00 00 0c 00  05 00 00 00 00 00 00 00  42 69 6c 64 65 72 2f 61 2e 74 78 74
        00 00 05 00  00 00 00 00 00 00 00 00  62 2e 62 69 6e";

    #[test]
    fn pruefvektor_angebot() {
        let a = vektor_angebot();
        assert_eq!(a.kodieren(), hex(VEKTOR_ANGEBOT));
        assert_eq!(a.kodiert_laenge(), hex(VEKTOR_ANGEBOT).len());
        assert_eq!(Angebot::lesen(&hex(VEKTOR_ANGEBOT)), Ok(a.clone()));
        assert_eq!(a.pruefen(Regeln::Windows).unwrap(), vec!["Bilder", "Bilder/a.txt", "b.bin"]);
        assert_eq!((a.oberste(), a.dateien()), (2, 2));
    }

    #[test]
    fn pruefvektoren_stueck_ende_quittung() {
        let s = Stueck { kennung: 7, eintrag: 1, versatz: 0, daten: b"hallo" };
        let v = hex("07 00 00 00  01 00 00 00  00 00 00 00 00 00 00 00  68 61 6c 6c 6f");
        assert_eq!(s.kodieren(), v);
        assert_eq!(Stueck::lesen(&v), Some(s));

        let e = Ende { kennung: 7, grund: 0 };
        assert_eq!(e.kodieren(), hex("07 00 00 00 00"));
        assert_eq!(Ende::lesen(&hex("07 00 00 00 00")), Some(e));

        let q = Quittung { kennung: 7, zustand: 1, empfangen: 5 };
        let v = hex("07 00 00 00  01 00 00 00  05 00 00 00 00 00 00 00");
        assert_eq!(q.kodieren(), v);
        assert_eq!(Quittung::lesen(&v), Some(q));
    }

    #[test]
    fn hin_und_zurueck() {
        let a = Angebot {
            kennung: u32::MAX,
            gesamt: GESAMT_MAX,
            eintraege: vec![
                Eintrag { art: EintragArt::Ordner, pfad: "ä ö_ü".into(), groesse: 0 },
                Eintrag { art: EintragArt::Datei, pfad: format!("ä ö_ü/{}", "x".repeat(255)), groesse: GESAMT_MAX },
            ],
        };
        assert_eq!(Angebot::lesen(&a.kodieren()), Ok(a));

        let daten = muster(STUECK_MAX, 3);
        let s = Stueck { kennung: 9, eintrag: 9999, versatz: GESAMT_MAX - 1, daten: &daten };
        assert_eq!(Stueck::lesen(&s.kodieren()), Some(s));
        let mut puffer = vec![1, 2, 3];
        s.kodieren_in(&mut puffer);
        assert_eq!(puffer, s.kodieren());

        for g in 0..=255u8 {
            let e = Ende { kennung: 3, grund: g };
            assert_eq!(Ende::lesen(&e.kodieren()), Some(e));
        }
        let q = Quittung { kennung: 1, zustand: 6, empfangen: u64::MAX };
        assert_eq!(Quittung::lesen(&q.kodieren()), Some(q));
    }

    #[test]
    fn leser_grenzen_der_kleinen_nachrichten() {
        // Stueck: 1 ..= STUECK_MAX Datenbytes.
        let mut n = Stueck { kennung: 1, eintrag: 0, versatz: 0, daten: &[0] }.kodieren();
        assert!(Stueck::lesen(&n).is_some());
        n.truncate(STUECK_KOPF);
        assert_eq!(Stueck::lesen(&n), None, "Stueck ohne Daten");
        let gross = vec![0u8; STUECK_MAX + 1];
        let n = Stueck { kennung: 1, eintrag: 0, versatz: 0, daten: &gross }.kodieren();
        assert_eq!(Stueck::lesen(&n), None, "Stueck ueber STUECK_MAX");
        // Ende: mindestens 5 Byte, Rest wird uebergangen.
        assert_eq!(Ende::lesen(&hex("07 00 00 00")), None);
        assert_eq!(Ende::lesen(&hex("07 00 00 00 02 ff")), Some(Ende { kennung: 7, grund: 2 }));
        // Quittung: genau 16 Byte.
        let q = Quittung { kennung: 7, zustand: 0, empfangen: 1 }.kodieren();
        assert_eq!(Quittung::lesen(&q[..15]), None);
        let mut laenger = q.clone();
        laenger.push(0);
        assert_eq!(Quittung::lesen(&laenger), None);
    }

    #[test]
    fn eingangsgrenzen_je_typ() {
        assert_eq!(eingangsgrenze(DATEI_ANGEBOT), Some(1024 * 1024));
        assert_eq!(eingangsgrenze(DATEI_STUECK), Some(16 + 49152));
        assert_eq!(eingangsgrenze(DATEI_ENDE), Some(256));
        assert_eq!(eingangsgrenze(DATEI_QUITTUNG), Some(256));
        assert_eq!(eingangsgrenze(IN_FAEHIGKEITEN), Some(256));
        assert_eq!(eingangsgrenze(crate::protokoll_konst::IN_CLIP), None);
        assert_eq!(eingangsgrenze(crate::protokoll_konst::IN_KEY), None);
    }

    #[test]
    fn kennungen_aufsteigend_und_nie_null() {
        let a = neue_kennung();
        let b = neue_kennung();
        assert!(a != 0 && b > a);
    }

    #[test]
    fn mb_und_sekunden_mit_komma() {
        assert_eq!(mb_text(12_400_000), "12,4");
        assert_eq!(mb_text(3_149_999), "3,1");
        assert_eq!(mb_text(0), "0,0");
        assert_eq!(sekunden_text(Duration::from_millis(1800)), "1,8");
    }

    // ------------------------------------------------------ Pfadregeln 2.5

    fn ungueltig(pfad: &str) -> String {
        pfad_pruefen(pfad).expect_err(pfad)
    }

    #[test]
    fn pfad_leer_absolut_und_doppelter_trenner() {
        assert!(ungueltig("").contains("leer"));
        assert!(ungueltig("/abs").contains("'/'"));
        assert!(ungueltig("/etc/passwd").contains("'/'"));
        assert!(ungueltig("a//b").contains("leerer Bestandteil"));
        assert!(ungueltig("a/").contains("leerer Bestandteil"));
        assert!(pfad_pruefen("a/b").is_ok());
    }

    #[test]
    fn pfad_punkt_und_punktpunkt() {
        assert!(ungueltig("..").contains(".."));
        assert!(ungueltig("../x").contains(".."));
        assert!(ungueltig("a/../../x").contains(".."));
        assert!(ungueltig("a/./b").contains("\".\""));
        assert!(ungueltig(".").contains("\".\""));
        // Nur genau "." und ".." sind verboten.
        assert!(pfad_pruefen("...").is_ok());
        assert!(pfad_pruefen(".versteckt").is_ok());
    }

    #[test]
    fn pfad_nul_und_rueckstrich() {
        assert!(ungueltig("a\0b").contains("NUL"));
        assert!(ungueltig("a\\b").contains("'\\'"));
        assert!(ungueltig("..\\..\\x").contains("'\\'"));
        assert!(ungueltig("C:\\Windows").contains("'\\'"));
    }

    #[test]
    fn pfad_zu_lang_bestandteil_zu_lang_zu_tief() {
        let lang = format!("{}/{}/{}/{}/{}", "a".repeat(200), "b".repeat(200), "c".repeat(200), "d".repeat(200), "e".repeat(221));
        assert_eq!(lang.len(), 1025);
        assert!(ungueltig(&lang).contains("laenger als 1024"));
        assert!(pfad_pruefen(&lang[..1024]).is_ok());
        assert!(ungueltig(&"x".repeat(256)).contains("laenger als 255"));
        assert!(pfad_pruefen(&"x".repeat(255)).is_ok());
        // 255 Byte mit Umlauten: gezaehlt werden Byte, nicht Zeichen.
        assert!(ungueltig(&"ä".repeat(128)).contains("laenger als 255"));
        let tief = vec!["t"; 33].join("/");
        assert!(ungueltig(&tief).contains("32 Stufen"));
        assert!(pfad_pruefen(&vec!["t"; 32].join("/")).is_ok());
    }

    #[test]
    fn laufwerk_und_geraetenamen_werden_bereinigt() {
        let w = |t: &str| bestandteil_bereinigen(t, Regeln::Windows);
        let m = |t: &str| bestandteil_bereinigen(t, Regeln::Mac);
        // Laufwerk: unter Windows waere "C:" ein Praefix, der den ganzen Pfad ersetzt.
        assert_eq!(w("C:"), "C_");
        assert_eq!(w("C:x"), "C_x");
        assert_eq!(w("a.txt:strom"), "a.txt_strom");
        assert_eq!(m("C:"), "C:");
        // Geraetenamen, in jeder Schreibweise, auch mit Endung.
        assert_eq!(w("CON"), "_CON");
        assert_eq!(w("con"), "_con");
        assert_eq!(w("aux.txt"), "_aux.txt");
        assert_eq!(w("Nul.tar.gz"), "_Nul.tar.gz");
        assert_eq!(w("com1"), "_com1");
        assert_eq!(w("LPT9.log"), "_LPT9.log");
        assert_eq!(w("COM\u{b9}"), "_COM\u{b9}");
        assert_eq!(w("CON .txt"), "_CON .txt");
        assert_eq!(w("conin$"), "_conin$");
        assert_eq!(w("COM10"), "COM10");
        assert_eq!(w("console"), "console");
        assert_eq!(w("prn_"), "prn_");
        assert_eq!(m("CON"), "CON");
        assert_eq!(m("aux.txt"), "aux.txt");
        // Verbotene Zeichen und Steuerzeichen.
        assert_eq!(w("a<b>c\"d|e?f*g"), "a_b_c_d_e_f_g");
        assert_eq!(w("zei\u{1}le\u{1f}"), "zei_le_");
        assert_eq!(m("zei\u{1}le\u{1f}:"), "zei_le_:");
        // Punkte und Leerzeichen am Ende.
        assert_eq!(w("a."), "a_");
        assert_eq!(w("a. ."), "a___");
        assert_eq!(w("..."), "___");
        assert_eq!(w("con."), "con_");
        assert_eq!(w(" a"), " a");
        assert_eq!(m("a. "), "a. ");
        // Ganzer Pfad.
        assert_eq!(pfad_bereinigen("C:/aux.txt", Regeln::Windows).unwrap(), "C_/_aux.txt");
        assert!(pfad_bereinigen("C:\\aux.txt", Regeln::Windows).is_err());
        // Die letzte Wache: nur schlichte Namen.
        let b = Path::new("basis");
        assert!(sicher_anhaengen(b, "C_/_aux.txt").is_some());
        #[cfg(windows)]
        assert!(sicher_anhaengen(b, "C:").is_none());
        assert!(sicher_anhaengen(b, "..").is_none());
        assert!(sicher_anhaengen(b, "a/../b").is_none());
    }

    fn eintraege(v: &[(EintragArt, &str, u64)]) -> Angebot {
        Angebot {
            kennung: 1,
            gesamt: v.iter().map(|e| e.2).sum(),
            eintraege: v.iter().map(|&(art, p, g)| Eintrag { art, pfad: p.into(), groesse: g }).collect(),
        }
    }

    use EintragArt::{Datei as D, Ordner as O};

    #[test]
    fn angebot_reihenfolge_vorfahr_vor_nachfahr() {
        let r = Regeln::Windows;
        assert!(eintraege(&[(O, "a", 0), (D, "a/b", 1)]).pruefen(r).is_ok());
        // Kind vor dem Ordner.
        let e = eintraege(&[(D, "a/b", 1), (O, "a", 0)]).pruefen(r).unwrap_err();
        assert!(e.text().contains("fehlt davor"), "{e:?}");
        // Eltern ist eine Datei.
        let e = eintraege(&[(D, "a", 1), (D, "a/b", 1)]).pruefen(r).unwrap_err();
        assert!(e.text().contains("fehlt davor"), "{e:?}");
        // Grosseltern fehlt.
        let e = eintraege(&[(O, "a", 0), (D, "a/b/c", 1)]).pruefen(r).unwrap_err();
        assert!(e.text().contains("fehlt davor"), "{e:?}");
        // Eltern anders geschrieben.
        assert!(eintraege(&[(O, "A", 0), (D, "a/b", 1)]).pruefen(r).is_err());
        assert_eq!(e.zustand(), ZUSTAND_UNGUELTIG);
    }

    #[test]
    fn angebot_doppelt_nach_bereinigung() {
        let doppelt = |v: &[(EintragArt, &str, u64)], r| {
            let e = eintraege(v).pruefen(r).unwrap_err();
            assert!(e.text().contains("doppelt"), "{e:?}");
            assert_eq!(e.zustand(), ZUSTAND_UNGUELTIG);
        };
        doppelt(&[(D, "a", 1), (D, "a", 1)], Regeln::Mac);
        doppelt(&[(D, "Bild.PNG", 1), (D, "bild.png", 1)], Regeln::Mac);
        doppelt(&[(O, "ä", 0), (D, "Ä", 1)], Regeln::Mac);
        doppelt(&[(D, "a:b", 1), (D, "a_b", 1)], Regeln::Windows);
        doppelt(&[(D, "x.", 1), (D, "x_", 1)], Regeln::Windows);
        doppelt(&[(O, "d", 0), (D, "d/a?", 1), (D, "d/A*", 1)], Regeln::Windows);
        // Auf dem Mac sind ':' und '_' verschieden.
        assert!(eintraege(&[(D, "a:b", 1), (D, "a_b", 1)]).pruefen(Regeln::Mac).is_ok());
    }

    fn roh(kennung: u32, anzahl: u32, gesamt: u64, eintraege: &[(u8, &[u8], u64)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&kennung.to_le_bytes());
        v.extend_from_slice(&anzahl.to_le_bytes());
        v.extend_from_slice(&gesamt.to_le_bytes());
        for (art, p, g) in eintraege {
            v.push(*art);
            v.push(0);
            v.extend_from_slice(&(p.len() as u16).to_le_bytes());
            v.extend_from_slice(&g.to_le_bytes());
            v.extend_from_slice(p);
        }
        v
    }

    fn abgelehnt(n: &[u8]) -> Ablehnung {
        Angebot::lesen(n).expect_err("abgelehnt erwartet")
    }

    #[test]
    fn angebot_summe_anzahl_art_und_ordnergroesse() {
        // Summe stimmt nicht (zu klein, zu gross).
        let e = abgelehnt(&roh(1, 1, 4, &[(0, b"a", 5)]));
        assert!(e.text().contains("Summe") && e.zustand() == ZUSTAND_UNGUELTIG, "{e:?}");
        let e = abgelehnt(&roh(1, 1, 6, &[(0, b"a", 5)]));
        assert!(e.text().contains("Summe"), "{e:?}");
        // Ueberlauf der Summe.
        let e = abgelehnt(&roh(1, 2, 0, &[(0, b"a", u64::MAX), (0, b"b", 2)]));
        assert!(e.text().contains("Summe"), "{e:?}");
        // Anzahl 0.
        let e = abgelehnt(&roh(1, 0, 0, &[]));
        assert!(e.text().contains("ohne Eintraege") && e.zustand() == ZUSTAND_UNGUELTIG, "{e:?}");
        // Art 2.
        let e = abgelehnt(&roh(1, 1, 0, &[(2, b"a", 0)]));
        assert!(e.text().contains("Art 2"), "{e:?}");
        // Ordner mit Groesse.
        let e = abgelehnt(&roh(1, 1, 3, &[(1, b"a", 3)]));
        assert!(e.text().contains("Ordner mit Groesse"), "{e:?}");
        // Anzahl groesser als die Eintraege.
        let e = abgelehnt(&roh(1, 2, 0, &[(0, b"a", 0)]));
        assert!(e.text().contains("unvollstaendig"), "{e:?}");
    }

    #[test]
    fn angebot_ueber_den_grenzen_ist_zu_gross() {
        let e = abgelehnt(&roh(1, EINTRAEGE_MAX + 1, 0, &[(0, b"a", 0)]));
        assert_eq!(e.zustand(), ZUSTAND_ZU_GROSS, "{e:?}");
        let e = abgelehnt(&roh(1, 1, GESAMT_MAX + 1, &[(0, b"a", GESAMT_MAX + 1)]));
        assert_eq!(e.zustand(), ZUSTAND_ZU_GROSS, "{e:?}");
        let mut n = roh(1, 1, 0, &[(0, b"a", 0)]);
        n.resize(ANGEBOT_MAX + 1, 0);
        assert_eq!(abgelehnt(&n).zustand(), ZUSTAND_ZU_GROSS);
        // Genau an den Grenzen geht es.
        assert!(Angebot::lesen(&roh(1, 1, GESAMT_MAX, &[(0, b"a", GESAMT_MAX)])).is_ok());
        let viele: Vec<(u8, Vec<u8>, u64)> =
            (0..EINTRAEGE_MAX).map(|i| (0u8, format!("{i}").into_bytes(), 0u64)).collect();
        let viele: Vec<(u8, &[u8], u64)> = viele.iter().map(|(a, p, g)| (*a, p.as_slice(), *g)).collect();
        let a = Angebot::lesen(&roh(1, EINTRAEGE_MAX, 0, &viele)).unwrap();
        assert!(a.pruefen(EIGENE_REGELN).is_ok());
    }

    #[test]
    fn angebot_mit_kaputtem_format() {
        let u = |n: &[u8], was: &str| {
            let e = abgelehnt(n);
            assert!(e.text().contains(was) && e.zustand() == ZUSTAND_UNGUELTIG, "{was}: {e:?}");
        };
        u(&hex("07 00 00 00 01 00 00 00 00 00 00"), "zu kurz");
        u(&roh(0, 1, 0, &[(0, b"a", 0)]), "Kennung 0");
        u(&roh(1, 1, 0, &[(0, b"", 0)]), "leerer Pfad");
        u(&roh(1, 1, 0, &[(0, &[b'x'; 1025], 0)]), "laenger als 1024");
        u(&roh(1, 1, 0, &[(0, &[0xff, 0xfe], 0)]), "UTF-8");
        let mut n = roh(1, 1, 0, &[(0, b"a", 0)]);
        n.push(0);
        u(&n, "nach dem letzten Eintrag");
        let mut n = roh(1, 1, 0, &[(0, b"abc", 0)]);
        n.pop();
        u(&n, "unvollstaendig");
    }

    // ------------------------------------------------------ Empfaenger

    type Fertig = Arc<Mutex<Option<Vec<PathBuf>>>>;

    struct Testempfang {
        e: Empfaenger,
        weg: Arc<Rekorder>,
        ev: Ereignisse,
        fertig: Fertig,
    }

    fn testempfang(basis: &Path, v: Vorgaben) -> Testempfang {
        testempfang_mit(basis, v, true)
    }

    fn testempfang_mit(basis: &Path, v: Vorgaben, ablage_klappt: bool) -> Testempfang {
        let weg = Rekorder::neu();
        let (ev, melden) = sammler();
        let fertig: Fertig = Arc::new(Mutex::new(None));
        let f2 = fertig.clone();
        let e = Empfaenger::neu_mit(
            basis.to_path_buf(),
            weg.clone(),
            move |p| {
                *sperre(&f2) = Some(p);
                ablage_klappt
            },
            melden,
            v,
        );
        Testempfang { e, weg, ev, fertig }
    }

    /// Kennung 7: Ordner d, d/x mit 100 Byte, y mit 50 Byte.
    fn kleines_angebot() -> Angebot {
        Angebot {
            kennung: 7,
            gesamt: 150,
            eintraege: vec![
                Eintrag { art: EintragArt::Ordner, pfad: "d".into(), groesse: 0 },
                Eintrag { art: EintragArt::Datei, pfad: "d/x".into(), groesse: 100 },
                Eintrag { art: EintragArt::Datei, pfad: "y".into(), groesse: 50 },
            ],
        }
    }

    fn stueck(eintrag: u32, versatz: u64, daten: &[u8]) -> Vec<u8> {
        Stueck { kennung: 7, eintrag, versatz, daten }.kodieren()
    }

    fn angenommen(t: &Testempfang) -> bool {
        bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_LAEUFT))
    }

    /// Ein Protokollfehler nach der Annahme: Quittung 4 und das Verzeichnis
    /// ist weg (Marke auch).
    fn stuecke_abgelehnt(name: &str, stuecke: &[Vec<u8>], was: &str) {
        let b = Ordner::neu(name);
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        assert_eq!(inhalt(b.p()).len(), 2, "Verzeichnis und Marke");
        for s in stuecke {
            t.e.nachricht(DATEI_STUECK, s.clone());
        }
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_UNGUELTIG)), "{was}: keine Quittung 4");
        assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()), "{was}: {:?}", inhalt(b.p()));
        let ungueltig = || matches!(letztes(&t.ev), Some(Ergebnis::Abgebrochen(Abbruch::Ungueltig(_))));
        assert!(bis(Duration::from_secs(3), ungueltig), "{was}: {:?}", ergebnisse(&t.ev));
        let z = zeilen(&t.ev).join("\n");
        assert!(z.contains(was), "{was}: {z}");
    }

    #[test]
    fn stueck_ausser_der_reihe() {
        stuecke_abgelehnt("reihe", &[stueck(2, 0, &[1; 50])], "ausser der Reihe");
    }

    #[test]
    fn stueck_mit_luecke() {
        stuecke_abgelehnt("luecke", &[stueck(1, 0, &[1; 10]), stueck(1, 20, &[1; 10])], "Versatz 20, erwartet 10");
    }

    #[test]
    fn stueck_doppelt() {
        stuecke_abgelehnt("doppelt", &[stueck(1, 0, &[1; 10]), stueck(1, 0, &[1; 10])], "Versatz 0, erwartet 10");
    }

    #[test]
    fn stueck_ueber_die_groesse_hinaus() {
        stuecke_abgelehnt("zu-gross", &[stueck(1, 0, &[1; 101])], "ueber die Groesse");
        stuecke_abgelehnt("zu-gross-2", &[stueck(1, 0, &[1; 90]), stueck(1, 90, &[1; 11])], "ueber die Groesse");
    }

    #[test]
    fn stueck_fuer_ordner_unbekannt_oder_leer() {
        stuecke_abgelehnt("ordner", &[stueck(0, 0, &[1])], "fuer den Ordner");
        stuecke_abgelehnt("unbekannt", &[stueck(3, 0, &[1])], "den es nicht gibt");
        let mut leer = stueck(1, 0, &[1]);
        leer.pop();
        stuecke_abgelehnt("leer", &[leer], "ungueltiger Laenge");
    }

    #[test]
    fn stuecke_und_enden_fremder_kennung_werden_uebergangen() {
        let b = Ordner::neu("fremd");
        let t = testempfang(b.p(), vorgaben(5000));
        // Ohne Angebot: nichts.
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 10]));
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        let mut fremd = Stueck { kennung: 8, eintrag: 2, versatz: 0, daten: &[1; 10] }.kodieren();
        t.e.nachricht(DATEI_STUECK, fremd.clone());
        fremd.truncate(3);
        t.e.nachricht(DATEI_STUECK, fremd);
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 8, grund: 1 }.kodieren());
        // Danach geht die eigene Uebertragung normal zu Ende.
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 100]));
        t.e.nachricht(DATEI_STUECK, stueck(2, 0, &[2; 50]));
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_FERTIG)));
        assert!(!t.weg.hat_quittung(ZUSTAND_UNGUELTIG));
        assert_eq!(t.weg.quittungen().iter().filter(|q| q.kennung != 7).count(), 0);
    }

    #[test]
    fn empfang_schreibt_quittiert_und_legt_ab() {
        let b = Ordner::neu("empfang");
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        assert_eq!(t.weg.quittungen()[0], Quittung { kennung: 7, zustand: 0, empfangen: 0 });
        let x = muster(100, 1);
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &x[..60]));
        t.e.nachricht(DATEI_STUECK, stueck(1, 60, &x[60..]));
        t.e.nachricht(DATEI_STUECK, stueck(2, 0, &[9; 50]));
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_FERTIG)));
        let q = *t.weg.quittungen().last().unwrap();
        assert_eq!(q, Quittung { kennung: 7, zustand: ZUSTAND_FERTIG, empfangen: 150 });
        let pfade = sperre(&t.fertig).clone().unwrap();
        assert_eq!(pfade.len(), 2);
        let verz = pfade[0].parent().unwrap().to_path_buf();
        assert_eq!(verz.parent().unwrap(), b.p());
        let name = verz.file_name().unwrap().to_str().unwrap();
        assert_eq!(name_lesen(name).map(|x| x.1), Some(7), "{name}");
        assert_eq!(pfade, vec![verz.join("d"), verz.join("y")]);
        assert_eq!(fs::read(verz.join("d").join("x")).unwrap(), x);
        assert_eq!(fs::read(verz.join("y")).unwrap(), vec![9; 50]);
        // Die Marke ist weg, nur das Verzeichnis bleibt.
        assert_eq!(inhalt(b.p()), vec![name.to_string()]);
        assert!(bis(Duration::from_secs(3), || letztes(&t.ev) == Some(Ergebnis::Fertig)));
        let z = zeilen(&t.ev);
        assert!(z.iter().any(|z| z == "Dateien: empfange 3 Eintraege, 0,0 MB"), "{z:?}");
        assert!(z.iter().any(|z| z == "Dateien: empfangen 3 Eintraege, 0,0 MB - in die Ablage gelegt"), "{z:?}");
        let st = sperre(&t.ev).iter().rev().find_map(|e| e.stand.clone()).unwrap();
        assert_eq!((st.richtung, st.bytes, st.gesamt, st.eintraege, st.dateien, st.oberste), (Richtung::Empfangen, 150, 150, 3, 2, 2));
        assert_eq!(st.ergebnis, Ergebnis::Fertig);
    }

    #[test]
    fn empfaenger_quittiert_je_64_kib() {
        let b = Ordner::neu("quittung-je");
        let mut v = vorgaben(5000);
        v.ende_frist = Duration::from_secs(2);
        let t = testempfang(b.p(), v);
        let groesse = 3 * QUITTUNG_ALLE + 10;
        let a = Angebot {
            kennung: 7,
            gesamt: groesse,
            eintraege: vec![Eintrag { art: EintragArt::Datei, pfad: "g".into(), groesse }],
        };
        // Der Schreibfaden haengt an Quittung 0, bis alles eingereiht ist; so
        // laeuft die Warteschlange nie leer, und es wird nur nach
        // QUITTUNG_ALLE quittiert (und am Ende).
        t.weg.voll.store(true, Ordering::SeqCst);
        t.e.nachricht(DATEI_ANGEBOT, a.kodieren());
        let daten = muster(groesse as usize, 5);
        let mut v = 0usize;
        while v < daten.len() {
            let n = STUECK_MAX.min(daten.len() - v);
            t.e.nachricht(DATEI_STUECK, stueck(0, v as u64, &daten[v..v + n]));
            v += n;
        }
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        t.weg.voll.store(false, Ordering::SeqCst);
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_FERTIG)));
        let q = t.weg.quittungen();
        let laufend: Vec<u64> = q.iter().filter(|q| q.zustand == 0).map(|q| q.empfangen).collect();
        // Stuecke zu 49152: nach 98304 und 196608 sind je 64 KiB voll.
        assert_eq!(laufend, vec![0, 98_304, 196_608]);
        assert_eq!(*q.last().unwrap(), Quittung { kennung: 7, zustand: ZUSTAND_FERTIG, empfangen: groesse });
    }

    #[test]
    fn abgelehntes_angebot_legt_nichts_an() {
        let faelle: Vec<(&str, Vec<u8>, u8)> = vec![
            ("punktpunkt", roh(3, 1, 0, &[(0, b"../x", 0)]), ZUSTAND_UNGUELTIG),
            // Der Angriff in voller Form: erst ".." als Ordner, dann darunter.
            ("punktpunkt-ordner", roh(3, 2, 0, &[(1, b"..", 0), (0, b"../x", 0)]), ZUSTAND_UNGUELTIG),
            ("laufwerk-rueckstrich", roh(3, 1, 0, &[(0, b"C:\\x", 0)]), ZUSTAND_UNGUELTIG),
            ("absolut", roh(3, 1, 0, &[(0, b"/abs", 0)]), ZUSTAND_UNGUELTIG),
            ("trenner", roh(3, 2, 0, &[(1, b"a", 0), (0, b"a//b", 0)]), ZUSTAND_UNGUELTIG),
            ("rueckstrich", roh(3, 1, 0, &[(0, b"a\\..\\..\\b", 0)]), ZUSTAND_UNGUELTIG),
            ("nul", roh(3, 1, 0, &[(0, b"a\0b", 0)]), ZUSTAND_UNGUELTIG),
            ("tief", roh(3, 1, 0, &[(0, vec!["t"; 33].join("/").as_bytes(), 0)]), ZUSTAND_UNGUELTIG),
            ("reihenfolge", roh(3, 2, 0, &[(0, b"a/b", 0), (1, b"a", 0)]), ZUSTAND_UNGUELTIG),
            ("doppelt", roh(3, 2, 0, &[(0, b"X", 0), (0, b"x", 0)]), ZUSTAND_UNGUELTIG),
            ("summe", roh(3, 1, 9, &[(0, b"a", 1)]), ZUSTAND_UNGUELTIG),
            ("gesamt", roh(3, 1, GESAMT_MAX + 1, &[(0, b"a", GESAMT_MAX + 1)]), ZUSTAND_ZU_GROSS),
            ("anzahl", roh(3, EINTRAEGE_MAX + 1, 0, &[]), ZUSTAND_ZU_GROSS),
        ];
        for (name, n, zustand) in faelle {
            let b = Ordner::neu(&format!("abgelehnt-{name}"));
            let t = testempfang(b.p(), vorgaben(5000));
            t.e.nachricht(DATEI_ANGEBOT, n);
            assert!(bis(Duration::from_secs(3), || !t.weg.quittungen().is_empty()), "{name}");
            let q = t.weg.quittungen();
            assert_eq!(q, vec![Quittung { kennung: 3, zustand, empfangen: 0 }], "{name}");
            assert!(inhalt(b.p()).is_empty(), "{name}: {:?}", inhalt(b.p()));
            assert!(bis(Duration::from_secs(3), || !zeilen(&t.ev).is_empty()), "{name}");
            let z = zeilen(&t.ev).join("\n");
            assert!(z.starts_with("Dateien: abgelehnt ("), "{name}: {z}");
            if zustand == ZUSTAND_ZU_GROSS {
                assert_eq!(letztes(&t.ev), Some(Ergebnis::ZuGross), "{name}");
            }
        }
    }

    #[test]
    fn feindliche_namen_landen_bereinigt_im_verzeichnis() {
        let b = Ordner::neu("feindlich");
        let t = testempfang(b.p(), vorgaben(5000));
        let namen = ["C:", "CON", "aux.txt", "a.", "zei\u{1}le", "x:y"];
        let a = Angebot {
            kennung: 7,
            gesamt: namen.len() as u64,
            eintraege: namen.iter().map(|n| Eintrag { art: EintragArt::Datei, pfad: n.to_string(), groesse: 1 }).collect(),
        };
        t.e.nachricht(DATEI_ANGEBOT, a.kodieren());
        assert!(angenommen(&t));
        for i in 0..namen.len() {
            t.e.nachricht(DATEI_STUECK, stueck(i as u32, 0, &[b'0' + i as u8]));
        }
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_FERTIG)), "{:?}", zeilen(&t.ev));
        let pfade = sperre(&t.fertig).clone().unwrap();
        let verz = pfade[0].parent().unwrap().to_path_buf();
        for (i, n) in namen.iter().enumerate() {
            let ziel = verz.join(bestandteil_bereinigen(n, EIGENE_REGELN));
            assert_eq!(pfade[i], ziel);
            assert_eq!(fs::read(&ziel).unwrap(), vec![b'0' + i as u8], "{n}");
        }
        // Nichts ausserhalb des Verzeichnisses.
        assert_eq!(inhalt(b.p()).len(), 1);
        let mut drin: Vec<String> = inhalt(&verz);
        drin.sort();
        assert_eq!(drin.len(), namen.len());
        if cfg!(windows) {
            assert!(drin.contains(&"C_".to_string()) && drin.contains(&"_CON".to_string()));
            assert!(drin.contains(&"_aux.txt".to_string()) && drin.contains(&"a_".to_string()));
        }
    }

    #[test]
    fn ende_mit_grund_loescht_das_verzeichnis() {
        for grund in [GRUND_ABGEBROCHEN, GRUND_LESEFEHLER, GRUND_ZEIT] {
            let b = Ordner::neu(&format!("ende-{grund}"));
            let t = testempfang(b.p(), vorgaben(5000));
            t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
            assert!(angenommen(&t));
            t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 60]));
            t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund }.kodieren());
            assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()), "{grund}: {:?}", inhalt(b.p()));
            assert!(bis(Duration::from_secs(1), || letztes(&t.ev) == Some(Ergebnis::Abgebrochen(Abbruch::Ende(grund)))));
            // Keine Quittung auf ein Ende mit Grund.
            assert!(t.weg.quittungen().iter().all(|q| q.zustand == ZUSTAND_LAEUFT));
            assert!(sperre(&t.fertig).is_none());
        }
    }

    #[test]
    fn ende_vor_dem_letzten_stueck_ist_ungueltig() {
        let b = Ordner::neu("ende-zu-frueh");
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 100]));
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_UNGUELTIG)));
        assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()));
        assert!(sperre(&t.fertig).is_none());
    }

    #[test]
    fn abbruch_beim_empfaenger_loescht_das_verzeichnis() {
        let b = Ordner::neu("abbruch-empfaenger");
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 60]));
        assert!(bis(Duration::from_secs(3), || t.weg.quittungen().iter().any(|q| q.empfangen == 60)));
        t.e.abbrechen();
        assert!(t.e.abwarten(Duration::from_secs(3)), "Faden haengt");
        assert!(!t.e.laeuft());
        assert!(inhalt(b.p()).is_empty(), "{:?}", inhalt(b.p()));
        assert_eq!(letztes(&t.ev), Some(Ergebnis::Abgebrochen(Abbruch::Hier)));
        // Danach wird nichts mehr angenommen.
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        thread::sleep(Duration::from_millis(50));
        assert!(inhalt(b.p()).is_empty());
    }

    #[test]
    fn drop_des_empfaengers_beendet_den_faden() {
        let b = Ordner::neu("drop-empfaenger");
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        let innen = t.e.innen.clone();
        drop(t.e);
        assert!(bis(Duration::from_secs(3), || sperre(&innen.q).beendet));
        assert!(inhalt(b.p()).is_empty());
    }

    #[test]
    fn neues_angebot_verwirft_die_laufende() {
        let b = Ordner::neu("neues-angebot");
        let t = testempfang(b.p(), vorgaben(5000));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 60]));
        assert!(bis(Duration::from_secs(3), || t.weg.quittungen().iter().any(|q| q.empfangen == 60)));
        let alt = inhalt(b.p());
        let mut neu = kleines_angebot();
        neu.kennung = 8;
        t.e.nachricht(DATEI_ANGEBOT, neu.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.quittungen().iter().any(|q| q.kennung == 8)));
        let jetzt = inhalt(b.p());
        assert_eq!(jetzt.len(), 2);
        assert!(alt.iter().all(|a| !jetzt.contains(a)), "{alt:?} {jetzt:?}");
        assert!(ergebnisse(&t.ev).contains(&Ergebnis::Abgebrochen(Abbruch::Ende(GRUND_ABGEBROCHEN))));
    }

    #[test]
    fn stillstand_beim_empfaenger() {
        let b = Ordner::neu("stillstand-empfaenger");
        let t = testempfang(b.p(), vorgaben(300));
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 10]));
        let t0 = Instant::now();
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_ABGEBROCHEN)));
        assert!(t0.elapsed() >= Duration::from_millis(250), "zu frueh: {:?}", t0.elapsed());
        let q = *t.weg.quittungen().last().unwrap();
        assert_eq!((q.zustand, q.empfangen), (ZUSTAND_ABGEBROCHEN, 10));
        assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()));
        let zeit = Some(Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung));
        assert!(bis(Duration::from_secs(3), || letztes(&t.ev) == zeit), "{:?}", ergebnisse(&t.ev));
    }

    #[test]
    fn zu_wenig_platz_gibt_quittung_3() {
        let b = Ordner::neu("platz");
        let mut v = vorgaben(5000);
        v.freier_platz = |_| Some(PLATZRESERVE + 149);
        let t = testempfang(b.p(), v);
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_KEIN_PLATZ)));
        assert!(inhalt(b.p()).is_empty());
        // Genau genug Platz: angenommen.
        let b = Ordner::neu("platz-genug");
        v.freier_platz = |_| Some(PLATZRESERVE + 150);
        let t = testempfang(b.p(), v);
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
    }

    #[test]
    fn freier_platz_je_plattform() {
        let frei = freier_platz(&std::env::temp_dir());
        if cfg!(windows) {
            assert!(frei.is_some_and(|f| f > 0), "{frei:?}");
            assert_eq!(freier_platz(Path::new("Q:\\gibt\\es\\nicht\\hoffentlich")), None);
        } else {
            assert_eq!(frei, None);
        }
    }

    #[test]
    fn ablage_nicht_gesetzt_gibt_quittung_5() {
        let b = Ordner::neu("ablage-scheitert");
        let t = testempfang_mit(b.p(), vorgaben(5000), false);
        t.e.nachricht(DATEI_ANGEBOT, kleines_angebot().kodieren());
        assert!(angenommen(&t));
        t.e.nachricht(DATEI_STUECK, stueck(1, 0, &[1; 100]));
        t.e.nachricht(DATEI_STUECK, stueck(2, 0, &[2; 50]));
        t.e.nachricht(DATEI_ENDE, Ende { kennung: 7, grund: 0 }.kodieren());
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_SCHREIBFEHLER)));
        assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()));
        let ablage = Some(Ergebnis::Abgebrochen(Abbruch::Ablage));
        assert!(bis(Duration::from_secs(3), || letztes(&t.ev) == ablage));
    }

    #[test]
    fn warteschlange_laeuft_nicht_ueber() {
        let b = Ordner::neu("ueberlauf");
        let mut v = vorgaben(5000);
        v.ende_frist = Duration::from_secs(2);
        let t = testempfang(b.p(), v);
        let groesse = 64 * STUECK_MAX as u64;
        let a = Angebot {
            kennung: 7,
            gesamt: groesse,
            eintraege: vec![Eintrag { art: EintragArt::Datei, pfad: "g".into(), groesse }],
        };
        // Der Schreibfaden haengt an der Quittung 0, bis `voll` wieder faellt;
        // so staut sich alles in der Warteschlange.
        t.weg.voll.store(true, Ordering::SeqCst);
        t.e.nachricht(DATEI_ANGEBOT, a.kodieren());
        let daten = vec![7u8; STUECK_MAX];
        for i in 0..64u64 {
            t.e.nachricht(DATEI_STUECK, stueck(0, i * STUECK_MAX as u64, &daten));
        }
        assert!(sperre(&t.e.innen.q).bytes <= WARTESCHLANGE_MAX);
        assert!(sperre(&t.e.innen.q).ueberlauf);
        t.weg.voll.store(false, Ordering::SeqCst);
        assert!(bis(Duration::from_secs(3), || t.weg.hat_quittung(ZUSTAND_UNGUELTIG)));
        assert!(bis(Duration::from_secs(3), || inhalt(b.p()).is_empty()));
    }

    // ------------------------------------------------------ Sender

    fn quelle_datei(o: &Ordner, name: &str, n: usize) -> PathBuf {
        let p = o.p().join(name);
        datei(&p, &muster(n, n as u32));
        p
    }

    #[test]
    fn fenster_wird_eingehalten() {
        for fenster in [FENSTER_SPIEL, FENSTER] {
            let o = Ordner::neu(&format!("fenster-{fenster}"));
            let p = quelle_datei(&o, "gross.bin", 1_000_000);
            let weg = Rekorder::neu();
            let (ev, melden) = sammler();
            let g = Sender::starten_mit(vec![p], weg.clone(), fenster, melden, vorgaben(10_000));
            // Ohne Quittung geht genau das Fenster hinaus, nicht mehr.
            assert!(bis(Duration::from_secs(3), || weg.datenbytes() == fenster), "{}", weg.datenbytes());
            thread::sleep(Duration::from_millis(100));
            assert_eq!(weg.datenbytes(), fenster);
            let mut quittiert = 0;
            for schritt in [40_000u64, 60_000, 7] {
                quittiert += schritt;
                g.quittung(&Quittung { kennung: g.kennung(), zustand: 0, empfangen: quittiert }.kodieren());
                assert!(bis(Duration::from_secs(3), || weg.datenbytes() == quittiert + fenster));
                thread::sleep(Duration::from_millis(50));
                assert_eq!(weg.datenbytes(), quittiert + fenster, "nach Quittung {quittiert}");
            }
            // Jedes Stueck hat hoechstens STUECK_MAX, die Versaetze schliessen an.
            let mut v = 0;
            for (t, n) in weg.alle() {
                if t == DATEI_STUECK {
                    let s = Stueck::lesen(&n).unwrap();
                    assert!(s.daten.len() <= STUECK_MAX);
                    assert_eq!(s.versatz, v);
                    v += s.daten.len() as u64;
                }
            }
            g.abbrechen();
            assert!(g.abwarten(Duration::from_secs(3)));
            assert_eq!(weg.enden(), vec![Ende { kennung: g.kennung(), grund: GRUND_ABGEBROCHEN }]);
            assert_eq!(letztes(&ev), Some(Ergebnis::Abgebrochen(Abbruch::Hier)));
        }
    }

    #[test]
    fn stillstand_beim_sender() {
        let o = Ordner::neu("stillstand-sender");
        let p = quelle_datei(&o, "a.bin", 500_000);
        let weg = Rekorder::neu();
        let (ev, melden) = sammler();
        let t0 = Instant::now();
        let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER, melden, vorgaben(300));
        assert!(g.abwarten(Duration::from_secs(3)));
        assert!(t0.elapsed() >= Duration::from_millis(250), "zu frueh: {:?}", t0.elapsed());
        assert_eq!(weg.enden(), vec![Ende { kennung: g.kennung(), grund: GRUND_ZEIT }]);
        assert_eq!(letztes(&ev), Some(Ergebnis::Abgebrochen(Abbruch::Zeitueberschreitung)));
    }

    #[test]
    fn sender_wartet_nach_dem_ende_auf_quittung_1() {
        let o = Ordner::neu("schluss");
        let p = quelle_datei(&o, "a.bin", 1000);
        let weg = Rekorder::neu();
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER, melden, vorgaben(5000));
        assert!(bis(Duration::from_secs(3), || !weg.enden().is_empty()));
        assert!(g.laeuft());
        g.quittung(&Quittung { kennung: g.kennung() + 1, zustand: 1, empfangen: 1000 }.kodieren());
        thread::sleep(Duration::from_millis(50));
        assert!(g.laeuft(), "fremde Kennung beendet nichts");
        g.quittung(&Quittung { kennung: g.kennung(), zustand: 1, empfangen: 1000 }.kodieren());
        assert!(g.abwarten(Duration::from_secs(3)));
        assert_eq!(letztes(&ev), Some(Ergebnis::Fertig));
        let z = zeilen(&ev);
        assert!(z.iter().any(|z| z == "Dateien: sende 1 Eintraege, 0,0 MB"), "{z:?}");
        assert!(z.iter().any(|z| z.starts_with("Dateien: gesendet und quittiert (0,0 MB in ")), "{z:?}");
    }

    #[test]
    fn quittung_ungleich_null_beendet_ohne_ende() {
        for zustand in [ZUSTAND_ZU_GROSS, ZUSTAND_KEIN_PLATZ, ZUSTAND_UNGUELTIG, ZUSTAND_SCHREIBFEHLER, ZUSTAND_ABGEBROCHEN] {
            let o = Ordner::neu(&format!("quittung-{zustand}"));
            let p = quelle_datei(&o, "a.bin", 500_000);
            let weg = Rekorder::neu();
            let (ev, melden) = sammler();
            let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER_SPIEL, melden, vorgaben(5000));
            assert!(bis(Duration::from_secs(3), || weg.datenbytes() == FENSTER_SPIEL));
            g.quittung(&Quittung { kennung: g.kennung(), zustand, empfangen: 0 }.kodieren());
            assert!(g.abwarten(Duration::from_secs(3)));
            assert!(weg.enden().is_empty(), "{zustand}: kein Ende nach Quittung ungleich 0");
            let erwartet = if zustand == ZUSTAND_ZU_GROSS {
                Ergebnis::ZuGross
            } else {
                Ergebnis::Abgebrochen(Abbruch::Quittung(zustand))
            };
            assert_eq!(letztes(&ev), Some(erwartet));
        }
    }

    #[test]
    fn voll_wird_abgewartet_und_weg_bricht_ab() {
        let o = Ordner::neu("voll");
        let p = quelle_datei(&o, "a.bin", 100_000);
        let weg = Rekorder::neu();
        sperre(&weg.antworten).extend([Gesendet::Voll; 5]);
        let (_ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p.clone()], weg.clone(), FENSTER, melden, vorgaben(5000));
        assert!(bis(Duration::from_secs(3), || !weg.enden().is_empty()));
        assert_eq!(weg.datenbytes(), 100_000);
        assert_eq!(weg.alle()[0].0, DATEI_ANGEBOT);
        drop(g);

        // Weg nach dem Angebot: sofort Schluss, kein Ende, kein weiterer Versuch.
        let weg = Rekorder::neu();
        sperre(&weg.antworten).extend([Gesendet::Ja, Gesendet::Weg]);
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER, melden, vorgaben(5000));
        assert!(g.abwarten(Duration::from_secs(3)));
        assert_eq!(weg.versuche.load(Ordering::SeqCst), 2);
        assert_eq!(weg.alle().len(), 1);
        assert_eq!(letztes(&ev), Some(Ergebnis::Abgebrochen(Abbruch::Verbindung)));
    }

    #[test]
    fn abbruch_waehrend_voll_endet_ohne_zu_haengen() {
        let o = Ordner::neu("abbruch-voll");
        let p = quelle_datei(&o, "a.bin", 100_000);
        let weg = Rekorder::neu();
        weg.voll.store(true, Ordering::SeqCst);
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER, melden, vorgaben(10_000));
        assert!(bis(Duration::from_secs(3), || weg.versuche.load(Ordering::SeqCst) > 3));
        // Kein Busy-Loop: bei Voll je Versuch rund 2 ms Pause.
        let v0 = weg.versuche.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(100));
        let versuche = weg.versuche.load(Ordering::SeqCst) - v0;
        assert!(versuche <= 80, "{versuche} Versuche in 100 ms");
        let t0 = Instant::now();
        g.abbrechen();
        assert!(g.abwarten(Duration::from_secs(2)));
        assert!(t0.elapsed() < Duration::from_millis(500));
        // Das Angebot war nicht hinaus: kein Ende.
        assert!(weg.alle().is_empty());
        assert_eq!(letztes(&ev), Some(Ergebnis::Abgebrochen(Abbruch::Hier)));
    }

    /// Sperrt den Weg, bis die Quelle veraendert ist.
    fn nach_dem_auflisten(p: &Path, weg: &Rekorder, aendern: impl FnOnce(&Path)) {
        assert!(bis(Duration::from_secs(3), || weg.versuche.load(Ordering::SeqCst) > 0));
        aendern(p);
        weg.voll.store(false, Ordering::SeqCst);
    }

    #[test]
    fn datei_kuerzer_als_angekuendigt_gibt_ende_2() {
        let o = Ordner::neu("kuerzer");
        let p = quelle_datei(&o, "a.bin", 100_000);
        let weg = Rekorder::neu();
        weg.voll.store(true, Ordering::SeqCst);
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p.clone()], weg.clone(), FENSTER, melden, vorgaben(5000));
        nach_dem_auflisten(&p, &weg, |p| {
            OpenOptions::new().write(true).open(p).unwrap().set_len(60_000).unwrap()
        });
        assert!(g.abwarten(Duration::from_secs(3)));
        assert_eq!(weg.enden(), vec![Ende { kennung: g.kennung(), grund: GRUND_LESEFEHLER }]);
        assert_eq!(weg.datenbytes(), STUECK_MAX as u64);
        assert!(matches!(letztes(&ev), Some(Ergebnis::Abgebrochen(Abbruch::Lesefehler(t))) if t.contains("kuerzer")));
    }

    #[test]
    fn datei_laenger_geworden_nur_die_angekuendigte_groesse() {
        let o = Ordner::neu("laenger");
        let p = quelle_datei(&o, "a.bin", 1000);
        let weg = Rekorder::neu();
        weg.voll.store(true, Ordering::SeqCst);
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p.clone()], weg.clone(), FENSTER, melden, vorgaben(5000));
        nach_dem_auflisten(&p, &weg, |p| {
            OpenOptions::new().append(true).open(p).unwrap().write_all(&[1; 500]).unwrap()
        });
        assert!(bis(Duration::from_secs(3), || !weg.enden().is_empty()));
        assert_eq!(weg.enden()[0].grund, GRUND_VOLLSTAENDIG);
        assert_eq!(weg.datenbytes(), 1000);
        g.quittung(&Quittung { kennung: g.kennung(), zustand: 1, empfangen: 1000 }.kodieren());
        assert!(g.abwarten(Duration::from_secs(3)));
        assert_eq!(letztes(&ev), Some(Ergebnis::Fertig));
    }

    #[test]
    fn eigene_ablage_wird_nicht_gesendet() {
        let basis = ablage_basis();
        let p = basis.join("12-34").join("x.txt");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        datei(&p, b"x");
        assert!(aus_eigener_ablage(&[p.clone()], &basis));
        assert!(!aus_eigener_ablage(&[std::env::temp_dir()], &basis));
        let weg = Rekorder::neu();
        let (ev, melden) = sammler();
        let g = Sender::starten_mit(vec![p], weg.clone(), FENSTER, melden, vorgaben(5000));
        assert!(g.abwarten(Duration::from_secs(3)));
        assert_eq!(weg.versuche.load(Ordering::SeqCst), 0);
        assert!(zeilen(&ev).iter().any(|z| z.contains("aus einem Empfang")));
        let _ = fs::remove_dir_all(&basis);
    }

    // ------------------------------------------------------ Auflisten

    #[test]
    fn auflisten_sortiert_vorfahr_vor_nachfahr_und_doppelte_oberste() {
        let o = Ordner::neu("auflisten");
        let a = o.p().join("eins");
        let b = o.p().join("zwei");
        fs::create_dir_all(a.join("Top").join("unter")).unwrap();
        fs::create_dir_all(b.join("top")).unwrap();
        datei(&a.join("Top").join("z.txt"), b"zz");
        datei(&a.join("Top").join("b.txt"), b"b");
        datei(&a.join("Top").join("unter").join("c"), b"ccc");
        datei(&b.join("top").join("x"), b"x");
        datei(&a.join("leer"), b"");
        let l = auflisten(&[a.join("Top"), a.join("leer"), b.join("top"), o.p().join("gibt-es-nicht")]);
        let pfade: Vec<(&str, EintragArt, u64)> = l.eintraege.iter().map(|e| (e.pfad.as_str(), e.art, e.groesse)).collect();
        assert_eq!(
            pfade,
            vec![
                ("Top", O, 0),
                ("Top/b.txt", D, 1),
                ("Top/unter", O, 0),
                ("Top/unter/c", D, 3),
                ("Top/z.txt", D, 2),
                ("leer", D, 0),
            ]
        );
        assert_eq!(l.gesamt, 6);
        assert_eq!(l.quellen[3], a.join("Top").join("unter").join("c"));
        assert!(l.zu_gross.is_none());
        let h = l.hinweise.join("\n");
        assert!(h.contains("doppelter Name") && h.contains("zwei"), "{h}");
        assert!(h.contains("nicht lesbar") && h.contains("gibt-es-nicht"), "{h}");
        // Das Angebot daraus besteht die Pruefung des Empfaengers.
        let an = Angebot { kennung: 1, gesamt: l.gesamt, eintraege: l.eintraege };
        let an = Angebot::lesen(&an.kodieren()).unwrap();
        assert!(an.pruefen(Regeln::Windows).is_ok() && an.pruefen(Regeln::Mac).is_ok());
    }

    #[test]
    fn auflisten_ueber_den_grenzen() {
        assert_eq!(Grenzen::default(), Grenzen { eintraege: 10_000, gesamt: 4 << 30, angebot: 1 << 20 });
        let o = Ordner::neu("auflisten-grenzen");
        let d = o.p().join("viele");
        fs::create_dir_all(&d).unwrap();
        for i in 0..5 {
            datei(&d.join(format!("{i}")), &[1; 10]);
        }
        let g = |eintraege, gesamt, angebot| Grenzen { eintraege, gesamt, angebot };
        // Ordner "viele" + 5 Dateien "viele/i" = 6 Eintraege, 50 Byte,
        // Angebot 16 + 6*12 + 5 + 5*7 = 128 Byte.
        let l = auflisten_mit(&[d.clone()], g(6, 50, 128));
        assert!(l.zu_gross.is_none(), "{:?}", l.zu_gross);
        assert_eq!(Angebot { kennung: 1, gesamt: l.gesamt, eintraege: l.eintraege }.kodiert_laenge(), 128);
        let l = auflisten_mit(&[d.clone()], g(5, 50, 128));
        assert!(l.zu_gross.as_deref().is_some_and(|z| z.contains("mehr als 5 Eintraege")), "{:?}", l.zu_gross);
        let l = auflisten_mit(&[d.clone()], g(6, 49, 128));
        assert!(l.zu_gross.as_deref().is_some_and(|z| z.contains("MB")), "{:?}", l.zu_gross);
        let l = auflisten_mit(&[d.clone()], g(6, 50, 127));
        assert!(l.zu_gross.as_deref().is_some_and(|z| z.contains("Dateiliste")), "{:?}", l.zu_gross);
        // Der Sender sendet dann nichts und meldet ZuGross.
        let weg = Rekorder::neu();
        let (ev, melden) = sammler();
        let mut v = vorgaben(5000);
        v.grenzen = g(5, 50, 128);
        let s = Sender::starten_mit(vec![d], weg.clone(), FENSTER, melden, v);
        assert!(s.abwarten(Duration::from_secs(3)));
        assert_eq!(weg.versuche.load(Ordering::SeqCst), 0);
        assert_eq!(letztes(&ev), Some(Ergebnis::ZuGross));
        assert!(zeilen(&ev).iter().any(|z| z.starts_with("Dateien: nicht gesendet, zu gross (mehr als 5")));
    }

    #[test]
    fn auflisten_ueberspringt_zu_tiefe_ordner() {
        let o = Ordner::neu("auflisten-tief");
        let mut d = o.p().join("t0");
        let oben = d.clone();
        for i in 1..=33 {
            d = d.join(format!("t{i}"));
        }
        fs::create_dir_all(&d).unwrap();
        let l = auflisten(&[oben]);
        assert_eq!(l.eintraege.len(), TIEFE_MAX);
        assert!(l.hinweise.iter().any(|h| h.contains("zu tief")));
        let an = Angebot { kennung: 1, gesamt: 0, eintraege: l.eintraege };
        assert!(an.pruefen(EIGENE_REGELN).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symbolische_verknuepfungen_werden_uebersprungen() {
        use std::os::unix::fs::symlink;
        let o = Ordner::neu("verknuepfungen");
        let draussen = o.p().join("draussen");
        fs::create_dir_all(&draussen).unwrap();
        datei(&draussen.join("geheim"), b"geheim");
        let t = o.p().join("t");
        fs::create_dir_all(t.join("s")).unwrap();
        datei(&t.join("a.txt"), b"a");
        datei(&t.join("s").join("b.txt"), b"b");
        symlink(draussen.join("geheim"), t.join("datei-link")).unwrap();
        symlink(&draussen, t.join("ordner-link")).unwrap();
        symlink(&draussen, o.p().join("oben-link")).unwrap();
        let l = auflisten(&[t.clone(), o.p().join("oben-link")]);
        let pfade: Vec<&str> = l.eintraege.iter().map(|e| e.pfad.as_str()).collect();
        assert_eq!(pfade, vec!["t", "t/a.txt", "t/s", "t/s/b.txt"]);
        let n = l.hinweise.iter().filter(|h| h.contains("symbolische Verknuepfung")).count();
        assert_eq!(n, 3, "{:?}", l.hinweise);
        assert!(l.quellen.iter().all(|q| !q.starts_with(&draussen)));
    }

    // ------------------------------------------------------ Aufraeumen

    #[test]
    fn aufraeumen_behaelt_die_drei_neuesten_und_bleibt_in_der_basis() {
        let o = Ordner::neu("aufraeumen");
        let b = o.p().join("basis");
        let draussen = o.p().join("draussen");
        fs::create_dir_all(&draussen).unwrap();
        datei(&draussen.join("wichtig.txt"), b"bleibt");
        for (ms, k) in [(1000, 1), (2000, 2), (3000, 3), (4000, 4), (5000, 5), (5000, 4)] {
            let d = b.join(format!("{ms}-{k}"));
            fs::create_dir_all(d.join("innen")).unwrap();
            datei(&d.join("innen").join("f"), b"f");
        }
        // Fremdes bleibt unangetastet.
        fs::create_dir_all(b.join("fremd")).unwrap();
        fs::create_dir_all(b.join("12-x")).unwrap();
        datei(&b.join("notiz.txt"), b"n");
        datei(&b.join("100-1"), b"datei mit passendem Namen");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&draussen, b.join("500-9")).unwrap();
            symlink(&draussen, b.join("1000-1").join("link")).unwrap();
            symlink(draussen.join("wichtig.txt"), b.join("2000-2").join("dlink")).unwrap();
        }
        let n = aufraeumen(&b, BEHALTEN, None);
        assert_eq!(n, 3);
        let mut erwartet = vec!["100-1", "12-x", "4000-4", "5000-4", "5000-5", "fremd", "notiz.txt"];
        if cfg!(unix) {
            erwartet.push("500-9");
            erwartet.sort();
        }
        assert_eq!(inhalt(&b), erwartet);
        assert_eq!(fs::read(draussen.join("wichtig.txt")).unwrap(), b"bleibt");
        #[cfg(unix)]
        assert!(fs::symlink_metadata(b.join("500-9")).unwrap().file_type().is_symlink());
        // Eine Verknuepfung als Basis wird nicht betreten.
        #[cfg(unix)]
        {
            let link = o.p().join("basis-link");
            std::os::unix::fs::symlink(&b, &link).unwrap();
            assert_eq!(aufraeumen(&link, 0, None), 0);
            assert_eq!(inhalt(&b).len(), erwartet.len());
        }
    }

    #[test]
    fn aufraeumen_laesst_laufende_stehen_und_raeumt_nach_24_stunden() {
        let o = Ordner::neu("aufraeumen-alter");
        let b = o.p();
        let jetzt = unix_ms();
        let stunde = 3_600_000;
        let alt = jetzt - 25 * stunde;
        let jung = jetzt - stunde;
        for name in [format!("{alt}-1"), format!("{jung}-2"), format!("{alt}-3"), format!("{jung}-4"), format!("{jung}-5")] {
            fs::create_dir_all(b.join(&name)).unwrap();
        }
        // 3 (alt) und 4 (jung) laufen noch.
        datei(&b.join(format!("{alt}-3{MARKE_ENDUNG}")), b"");
        datei(&b.join(format!("{jung}-4{MARKE_ENDUNG}")), b"");
        // Nach einem Empfang (ohne Alter), behalten 1: die laufende bleibt
        // und zaehlt nicht mit.
        let n = aufraeumen_zu(b, 1, None, jetzt);
        assert_eq!(n, 2, "{:?}", inhalt(b));
        let mut e = vec![format!("{alt}-3"), format!("{alt}-3{MARKE_ENDUNG}"), format!("{jung}-4"), format!("{jung}-4{MARKE_ENDUNG}"), format!("{jung}-5")];
        e.sort();
        assert_eq!(inhalt(b), e);
        // Beim Start: aelter als 24 h geht, auch mit Marke.
        let n = aufraeumen_zu(b, usize::MAX, Some(HOECHSTALTER), jetzt);
        assert_eq!(n, 1);
        let mut e = vec![format!("{jung}-4"), format!("{jung}-4{MARKE_ENDUNG}"), format!("{jung}-5")];
        e.sort();
        assert_eq!(inhalt(b), e);
    }

    #[test]
    fn basis_ist_ein_eigener_ordner() {
        let o = Ordner::neu("basis");
        let b = o.p().join("a").join("QuadChroma-Ablage");
        basis_anlegen(&b).unwrap();
        assert!(fs::symlink_metadata(&b).unwrap().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&b).unwrap().permissions().mode() & 0o777, 0o700);
            fs::set_permissions(&b, fs::Permissions::from_mode(0o755)).unwrap();
            basis_anlegen(&b).unwrap();
            assert_eq!(fs::metadata(&b).unwrap().permissions().mode() & 0o777, 0o700);
            let link = o.p().join("link");
            std::os::unix::fs::symlink(&b, &link).unwrap();
            assert!(basis_anlegen(&link).is_err());
        }
        // Unter cfg(test) ein eigener Ordner je Lauf, nie die echte Ablage.
        let echt = ablage_basis();
        assert!(echt.to_string_lossy().contains(&format!("qc-test-{}", std::process::id())));
    }

    // ------------------------------------------------------ Sender -> Empfaenger

    struct Paar {
        empf: Arc<Empfaenger>,
        griff: Arc<Griff>,
        fertig: Fertig,
        ev_s: Ereignisse,
        ev_e: Ereignisse,
    }

    /// Sender und Empfaenger ueber Wege im Speicher; die Quittungen laufen
    /// ueber einen eigenen Faden zurueck wie ueber den Bild- bzw. Eingabekanal.
    fn paar(pfade: Vec<PathBuf>, basis: &Path, fenster: u64, v: Vorgaben) -> Paar {
        let (qtx, qrx) = mpsc::channel::<Vec<u8>>();
        let qtx = Mutex::new(qtx);
        let fertig: Fertig = Arc::new(Mutex::new(None));
        let f2 = fertig.clone();
        let (ev_e, me) = sammler();
        let empf = Arc::new(Empfaenger::neu_mit(
            basis.to_path_buf(),
            Arc::new(move |t: u8, n: &[u8]| {
                assert_eq!(t, DATEI_QUITTUNG);
                let _ = sperre(&qtx).send(n.to_vec());
                Gesendet::Ja
            }),
            move |p| {
                *sperre(&f2) = Some(p);
                true
            },
            me,
            v,
        ));
        let e2 = empf.clone();
        let (ev_s, ms) = sammler();
        let griff = Arc::new(Sender::starten_mit(
            pfade,
            Arc::new(move |t: u8, n: &[u8]| {
                e2.nachricht(t, n.to_vec());
                Gesendet::Ja
            }),
            fenster,
            ms,
            v,
        ));
        let g2 = griff.clone();
        thread::spawn(move || {
            while let Ok(q) = qrx.recv() {
                g2.quittung(&q);
            }
        });
        Paar { empf, griff, fertig, ev_s, ev_e }
    }

    /// Vergleicht zwei Baeume Byte fuer Byte.
    fn gleich(a: &Path, b: &Path) {
        let ma = fs::symlink_metadata(a).unwrap();
        let mb = fs::symlink_metadata(b).unwrap();
        assert_eq!(ma.is_dir(), mb.is_dir(), "{} / {}", a.display(), b.display());
        if ma.is_dir() {
            let namen = |p: &Path| {
                let mut v: Vec<String> = fs::read_dir(p).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
                v.sort();
                v
            };
            let (na, nb) = (namen(a), namen(b));
            assert_eq!(na, nb, "{}", a.display());
            for n in na {
                gleich(&a.join(&n), &b.join(&n));
            }
        } else {
            assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap(), "{}", a.display());
        }
    }

    #[test]
    fn lauf_sender_zu_empfaenger_im_speicher() {
        let q = Ordner::neu("lauf-quelle");
        let z = Ordner::neu("lauf-ziel");
        let bilder = q.p().join("Bilder");
        fs::create_dir_all(bilder.join("Unter").join("Tief")).unwrap();
        datei(&bilder.join("a.txt"), b"hallo");
        datei(&bilder.join("leer.bin"), b"");
        datei(&bilder.join("Unter").join("gross.bin"), &muster(200_000, 1));
        datei(&q.p().join("b.bin"), b"");
        datei(&q.p().join("c.dat"), &muster(123_457, 2));
        fs::create_dir_all(q.p().join("leerer Ordner")).unwrap();
        let oben = vec![bilder.clone(), q.p().join("b.bin"), q.p().join("c.dat"), q.p().join("leerer Ordner")];
        let pa = paar(oben.clone(), z.p(), FENSTER, vorgaben(5000));
        assert!(pa.griff.abwarten(Duration::from_secs(10)), "Sender haengt: {:?}", zeilen(&pa.ev_s));
        assert_eq!(letztes(&pa.ev_s), Some(Ergebnis::Fertig), "{:?}", zeilen(&pa.ev_s));
        assert!(bis(Duration::from_secs(3), || letztes(&pa.ev_e) == Some(Ergebnis::Fertig)));
        let pfade = sperre(&pa.fertig).clone().unwrap();
        assert_eq!(pfade.len(), 4);
        let verz = pfade[0].parent().unwrap().to_path_buf();
        for (quelle, ziel) in oben.iter().zip(&pfade) {
            assert_eq!(ziel, &verz.join(quelle.file_name().unwrap()));
            gleich(quelle, ziel);
        }
        let st = sperre(&pa.ev_s).iter().rev().find_map(|e| e.stand.clone()).unwrap();
        assert_eq!((st.bytes, st.gesamt, st.eintraege, st.dateien, st.oberste), (323_462, 323_462, 9, 5, 4));
        // Nur das Uebertragungsverzeichnis, keine Marke.
        assert_eq!(inhalt(z.p()).len(), 1);
        pa.empf.abbrechen();
        assert!(pa.empf.abwarten(Duration::from_secs(3)));
        // Nach dem Abbrechen bleibt eine FERTIGE Uebertragung liegen.
        assert_eq!(inhalt(z.p()).len(), 1);
    }

    #[test]
    fn nach_jedem_empfang_bleiben_die_drei_neuesten() {
        let q = Ordner::neu("drei-quelle");
        let z = Ordner::neu("drei-ziel");
        let p = q.p().join("x.txt");
        datei(&p, b"x");
        for i in 1..=5 {
            let pa = paar(vec![p.clone()], z.p(), FENSTER, vorgaben(5000));
            assert!(pa.griff.abwarten(Duration::from_secs(5)));
            assert_eq!(letztes(&pa.ev_s), Some(Ergebnis::Fertig));
            assert!(bis(Duration::from_secs(3), || letztes(&pa.ev_e) == Some(Ergebnis::Fertig)));
            assert_eq!(inhalt(z.p()).len(), i.min(BEHALTEN), "nach Lauf {i}");
            // Neue Verzeichnisse brauchen einen neuen Zeitstempel im Namen.
            thread::sleep(Duration::from_millis(3));
        }
    }

    #[test]
    fn abbruch_beim_sender_loescht_beim_empfaenger() {
        let q = Ordner::neu("abbruch-quelle");
        let z = Ordner::neu("abbruch-ziel");
        let p = q.p().join("gross.bin");
        datei(&p, &muster(3_000_000, 4));
        // Langsamer Empfaenger: der Weg zum Empfaenger haelt jedes Stueck kurz
        // auf, so ist die Uebertragung sicher mitten im Lauf.
        let (ev_e, me) = sammler();
        let (qtx, qrx) = mpsc::channel::<Vec<u8>>();
        let qtx = Mutex::new(qtx);
        let empf = Arc::new(Empfaenger::neu_mit(
            z.p().to_path_buf(),
            Arc::new(move |_t: u8, n: &[u8]| {
                let _ = sperre(&qtx).send(n.to_vec());
                Gesendet::Ja
            }),
            |_| true,
            me,
            vorgaben(5000),
        ));
        let e2 = empf.clone();
        let (ev_s, ms) = sammler();
        let griff = Arc::new(Sender::starten_mit(
            vec![p],
            Arc::new(move |t: u8, n: &[u8]| {
                if t == DATEI_STUECK {
                    thread::sleep(Duration::from_millis(10));
                }
                e2.nachricht(t, n.to_vec());
                Gesendet::Ja
            }),
            FENSTER,
            ms,
            vorgaben(5000),
        ));
        let g2 = griff.clone();
        thread::spawn(move || {
            while let Ok(q) = qrx.recv() {
                g2.quittung(&q);
            }
        });
        assert!(bis(Duration::from_secs(3), || sperre(&ev_e).iter().any(|e| e.stand.as_ref().is_some_and(|s| s.bytes > 100_000))));
        assert_eq!(inhalt(z.p()).len(), 2, "Verzeichnis und Marke");
        griff.abbrechen();
        assert!(griff.abwarten(Duration::from_secs(3)));
        assert_eq!(letztes(&ev_s), Some(Ergebnis::Abgebrochen(Abbruch::Hier)));
        assert!(bis(Duration::from_secs(3), || inhalt(z.p()).is_empty()), "{:?}", inhalt(z.p()));
        assert!(bis(Duration::from_secs(1), || letztes(&ev_e) == Some(Ergebnis::Abgebrochen(Abbruch::Ende(GRUND_ABGEBROCHEN)))));
    }
}
