// Nachrichtenkennungen der Leitung - EINE Tabelle fuer beide Rollen.
//
// Der Client (Empfang) und der Windows-Host (Versand) lesen dieselben
// Zahlen von hier; auf dem Mac stehen sie als QC_MSG_*/QC_IN_* in
// host/main.m. Wer hier etwas aendert, aendert das Protokoll - und muss es
// auf dem Mac ebenso tun.
//
// Kopf jeder Nachricht: u8 Typ, u8 Flags, u16 frei (bei Bild: Bildnummer),
// u32 Laenge (little endian). Davor beim Verbinden einmal MAGIC.

#![allow(dead_code)]

/// Kennung nach dem Handschlag auf dem Bildkanal: das Geraet ist bekannt,
/// die Sitzung beginnt.
pub const MAGIC: &[u8; 4] = b"QCH1";

// ------------------------------------------ Zugang (Bildkanal, vor MAGIC)
//
// Spezifikation Pairing v1, Abschnitt 3.2. Einem unbekannten Geraet sendet
// der Host nach dem Handschlag statt MAGIC erst MAGIC_ZUGANG und dann
// Zugangsnachrichten mit dem ueblichen 8-Byte-Kopf (Flags und frei = 0).
// Wird es angenommen, folgt MAGIC und alles wie bisher. Kodieren, Lesen und
// alle Laengenpruefungen stehen in zugang.rs. Die Nummern 20-23 gelten nur
// in dieser Phase auf dem Bildkanal; IN_MOVE..IN_KEY (16-19) liegen auf dem
// Eingabekanal und kommen sich damit nicht in die Quere.

/// Kennung statt MAGIC: dieses Geraet ist dem Host unbekannt, es folgt die
/// Zugangsphase.
pub const MAGIC_ZUGANG: &[u8; 4] = b"QCA1";
/// Host -> Client, Laenge 8 + n: u8 Fassung (ZUGANG_FASSUNG), u8 Wege
/// (WEG_*), u16 0, u32 warten_ms (Drossel, 0 = sofort), dann n <= 40 Byte
/// Hostname (UTF-8).
pub const ZUGANG_NOETIG: u8 = 20;
/// Client -> Host, Laenge 32: client_proof (zugang::client_beweis).
pub const ZUGANG_BEWEIS: u8 = 21;
/// Host -> Client, Laenge 8 oder 40: u8 Ergebnis (ERGEBNIS_*), u8 0, u16 0,
/// u32 warten_ms, bei ERGEBNIS_PASSWORT dazu 32 Byte host_proof.
pub const ZUGANG_ERGEBNIS: u8 = 22;
/// Client -> Host, Laenge 0: der Nutzer hat abgebrochen.
pub const ZUGANG_ABBRUCH: u8 = 23;
/// Fassung der Nachricht ZUGANG_NOETIG.
pub const ZUGANG_FASSUNG: u8 = 1;
/// Wege in ZUGANG_NOETIG: Bit 0 Passwort (immer gesetzt), Bit 1 "Zulassen"
/// am Host moeglich (eine Oberflaeche laeuft).
pub const WEG_PASSWORT: u8 = 1;
pub const WEG_ZULASSEN: u8 = 2;
/// Ergebnisse in ZUGANG_ERGEBNIS.
pub const ERGEBNIS_PASSWORT: u8 = 0;
pub const ERGEBNIS_ZULASSEN: u8 = 1;
pub const ERGEBNIS_FALSCH: u8 = 2;
pub const ERGEBNIS_ABGELEHNT: u8 = 3;
pub const ERGEBNIS_SCHLUSS: u8 = 4;

/// Nutzlast von Handschlag-Nachricht 3 (Client -> Host): "QCN1" | u8 n |
/// n <= 40 Byte UTF-8-Name des Clients. Aeltere Clients senden b"client".
pub const NAME_KENNUNG: &[u8; 4] = b"QCN1";

// ------------------------------------------------ Host -> Client (Bildkanal)

/// Strominfo: u16 Breite, u16 Hoehe, u16 fps, u8 Codec (1 HEVC, 2 H.264),
/// u8 Profil (1 = 4:4:4 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit,
/// 4 = 4:2:0 10 Bit), u8 Bereich, u8 frei.
pub const MSG_INFO: u8 = 1;
/// Bild: eine Zugriffseinheit in Annex B; Flag Bit 0 = Vollbild.
pub const MSG_VIDEO: u8 = 2;
/// Was beim Host gilt: u32 Mbit, u16 fps, u8 Gaming, u8 fest, u8 Ton.
pub const MSG_SETTINGS: u8 = 3;
/// Antwort auf den Zeitabgleich: u64 t_client, u64 t_host (us).
pub const MSG_TIME: u8 = 4;
/// Zeitstempel zum naechsten Bild: u32 seq, u8 nachgelegt, 3 frei,
/// u64 t_cap, u64 t_enc (us, Hostuhr).
pub const MSG_STAMP: u8 = 5;
/// Auslastung des Hosts (28 Byte, Fassung 1).
pub const MSG_LAST: u8 = 6;
/// Ab hier neuer Codec: Decoder wegwerfen, das naechste Bild ist ein
/// Schluesselbild mit Parametersaetzen.
pub const MSG_SWITCH: u8 = 7;
/// Koennensliste des Hosts: welche Codecs er anbietet, mit Flaggen.
pub const MSG_CODECS: u8 = 8;
/// Lage des Hosts: u8 (0 = in Ordnung, 1 = kein Bildschirm).
pub const MSG_HOSTSTATUS: u8 = 9;
/// Abgeloest: ein neuer, gekoppelter Zuschauer hat diesen ersetzt. Ohne
/// Nutzlast; die letzte Nachricht, danach macht der Host Bild- und
/// Eingabekanal zu. Der Client verbindet sich darauf NICHT von selbst neu -
/// sonst loesten zwei Clients einander endlos ab. Ein aelterer Client
/// uebergeht sie wie jeden unbekannten Typ.
pub const MSG_ABGELOEST: u8 = 10;
/// Faehigkeiten des Hosts: u32 Bits (FAEHIG_*), mindestens 4 Byte, weitere
/// Bytes werden uebergangen. Geht an jeden neuen Zuschauer, zusammen mit
/// Einstellungen und Codecliste. Ein aelterer Client uebergeht Typ 11.
pub const MSG_FAEHIGKEITEN: u8 = 11;
/// Bildschirme des Hosts (Spezifikation Bildschirmwahl 2.2): u8 Fassung 1,
/// u8 Anzahl, u8 Wunschlaenge (0 = Automatik), u8 frei, Kennung des
/// Wunsches, dann je Eintrag Kennung, Name, Pixel, Hz und Flags (Bit 0
/// Hauptbildschirm, Bit 1 wird gestreamt). Bytes in bildschirm.rs. Geht an
/// jeden neuen Zuschauer nach MSG_FAEHIGKEITEN, nach jedem Wechsel, jeder
/// Aenderung der Liste und als Antwort auf jeden Wunsch. Ein aelterer
/// Client uebergeht Typ 12.
pub const MSG_BILDSCHIRME: u8 = 12;
/// Tonformat: u32 Rate, u8 Kanaele, u8 1 = float32 verschachtelt, 2 frei.
pub const MSG_AUDIO_INFO: u8 = 32;
/// Ton: float32 LE verschachtelt.
pub const MSG_AUDIO: u8 = 33;
/// Zwischenablage (UTF-8), in beide Richtungen unter derselben Nummer.
pub const MSG_CLIP: u8 = 48;
/// Zeigerform: 12 Byte Kopf (u16 Breite, u16 Hoehe, u16 Hotspot x,
/// u16 Hotspot y, u8 sichtbar, u8 Massstab, u16 frei), dann RGBA mit gerader
/// Deckkraft. Kommt nur, wenn sich die Form aendert.
pub const MSG_CURSOR: u8 = 49;

// ------------------------------------ Dateien (beide Richtungen, dateien.rs)
//
// Wie MSG_CLIP gelten sie in beide Richtungen unter derselben Nummer:
// Host -> Client auf dem Bildkanal, Client -> Host auf dem Eingabekanal; die
// Quittung laeuft jeweils in der Gegenrichtung. Formate, Grenzen und Ablauf
// stehen im Kopf von dateien.rs. Gesendet wird nur an eine Gegenseite, die in
// DIESER Sitzung FAEHIG_DATEIEN gemeldet hat - ein aelterer Host trennte
// sonst den Eingabekanal (alles ueber 256 Byte ausser IN_CLIP).

/// Angebot: u32 Kennung, u32 Anzahl, u64 Gesamt, dann die Eintraege.
pub const DATEI_ANGEBOT: u8 = 50;
/// Stueck: u32 Kennung, u32 Eintrag, u64 Versatz, 1 ..= 49152 Datenbytes.
pub const DATEI_STUECK: u8 = 51;
/// Ende: u32 Kennung, u8 Grund (mindestens 5 Byte).
pub const DATEI_ENDE: u8 = 52;
/// Quittung (genau 16 Byte): u32 Kennung, u8 Zustand, u8 frei, u16 frei,
/// u64 empfangen - vom Empfaenger zum Sender.
pub const DATEI_QUITTUNG: u8 = 53;

/// Bit 0 in MSG_FAEHIGKEITEN / IN_FAEHIGKEITEN: "Dateien Fassung 1".
pub const FAEHIG_DATEIEN: u32 = 1;
/// Bit 1 in MSG_FAEHIGKEITEN: der Host kennt die Bildschirmwahl
/// (MSG_BILDSCHIRME und IN_BILDSCHIRM). Der Client schickt IN_BILDSCHIRM nur
/// an einen Host, der es in DIESER Sitzung gemeldet hat; in IN_FAEHIGKEITEN
/// meldet der Client weiter nur FAEHIG_DATEIEN.
pub const FAEHIG_BILDSCHIRM: u32 = 2;

/// Flag Bit 0 im Bildkopf: Vollbild.
pub const FLAG_KEY: u8 = 0x01;

// ---------------------------------------------- Client -> Host (Eingabekanal)

/// Mausbewegung: f32 x, f32 y in 0..1 der Bildflaeche.
pub const IN_MOVE: u8 = 16;
/// Maustaste: u8 taste (0 links, 1 rechts, 2 mitte), u8 gedrueckt, u16 frei, f32 x, f32 y.
pub const IN_BUTTON: u8 = 17;
/// Rad: f32 dx, f32 dy in Pixeln.
pub const IN_SCROLL: u8 = 18;
/// Taste: u16 macOS-Keycode, u8 gedrueckt, u8 frei, u32 Umschalter (MOD_*).
pub const IN_KEY: u8 = 19;
/// Zwischenablage vom Client (UTF-8).
pub const IN_CLIP: u8 = 48;
/// Wunsch: wie MSG_SETTINGS.
pub const IN_SETTINGS: u8 = 64;
/// Frage zum Zeitabgleich: u64 t_client.
pub const IN_TIME: u8 = 65;
/// Codecwunsch: ein Byte, der Index aus der Koennensliste.
pub const IN_CODEC: u8 = 66;
/// Testbild: ein Byte, 1 = der Host zeigt sein bewegtes Muster statt des
/// Bildschirms, 0 = wieder der Bildschirm. Fuer den Benchmark, damit jeder
/// Schritt denselben Inhalt misst.
pub const IN_TESTBILD: u8 = 68;
/// Faehigkeiten des Clients: u32 Bits (FAEHIG_*), wie MSG_FAEHIGKEITEN. Die
/// erste Nachricht auf JEDEM neu stehenden Eingabekanal (gehoert zu
/// NACHREICHEN). Ein aelterer Host uebergeht sie, weil sie unter 256 Byte hat.
pub const IN_FAEHIGKEITEN: u8 = 69;
/// Bildschirmwunsch: u8 Kennungslaenge (0 = Automatik, sonst 1 ..= 64),
/// dann die Kennung (UTF-8) - hoechstens 65 Byte (bildschirm.rs). Nur an
/// einen Host mit FAEHIG_BILDSCHIRM; ein aelterer uebergeht sie ohnehin
/// (unter 256 Byte). Zustandsnachricht, gehoert zu NACHREICHEN.
pub const IN_BILDSCHIRM: u8 = 70;

// Umschalter als Bitmaske, damit der Mac denselben Zustand sieht wie Windows.
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_CMD: u32 = 8;

// -------------------------------------------------------------- Bekanntgabe

/// UDP-Rundruf des Hosts auf Bildport + 2: "QCHB" | u8 Version | u16 Bildport
/// | u8 Namenslaenge | Name (UTF-8), alle zwei Sekunden. Dahinter (Pairing
/// v1, Abschnitt 2; aeltere Clients uebergehen es): u8 BEACON_EXT | u32 ID
/// (LE) | u8 Flags (BEACON_FLAG_*). Gesamt hoechstens 128 Byte.
pub const BEACON_MAGIC: &[u8; 4] = b"QCHB";
pub const BEACON_VERSION: u8 = 1;
/// Kennung der Erweiterung hinter dem Namen.
pub const BEACON_EXT: u8 = 1;
/// Flag Bit 0: am Host kann jemand "Zulassen" klicken. Bit 1-7: 0.
pub const BEACON_FLAG_ZULASSEN: u8 = 1;
