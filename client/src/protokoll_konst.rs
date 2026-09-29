// Nachrichtenkennungen der Leitung - EINE Tabelle fuer beide Rollen.
//
// Der Client (Empfang) und der Windows-Host (Versand) lesen dieselben
// Zahlen von hier; auf dem Mac stehen sie als QC_MSG_*/QC_IN_* in
// host/main.m (Zugang und Abschied in host/zugang.h). Wer hier etwas
// aendert, aendert das Protokoll - und muss es auf dem Mac ebenso tun.
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
/// n <= 40 Byte UTF-8-Name des Clients | u8 Flags (NAME_FLAG_*). Aeltere
/// Clients senden b"client" oder "QCN1" ohne das Flag-Byte (dann Flags 0);
/// Bytes dahinter bleiben spaeteren Fassungen.
pub const NAME_KENNUNG: &[u8; 4] = b"QCN1";
/// Flag in Nachricht 3, Bit 0: der Client kennt den Schluessel dieses Hosts
/// nicht (nicht in hosts.txt) - "weise dich aus". Der Host fuehrt dann die
/// Zugangsphase auch fuer ein Geraet aus seiner Liste: Passwort mit
/// host_proof (22/0) oder "Zulassen" (22/1). Antwortet er trotzdem gleich
/// mit MAGIC, bricht der Client ab und merkt sich nichts. Bit 1-7: 0.
pub const NAME_FLAG_HOST_UNBEKANNT: u8 = 1;

// ------------------------------------------------ Host -> Client (Bildkanal)

/// Strominfo: u16 Breite, u16 Hoehe, u16 fps, u8 Codec (1 HEVC, 2 H.264),
/// u8 Format (1 = 4:4:4 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit,
/// 4 = 4:2:0 10 Bit; immer voller Bereich) - acht Byte bei Hosts vor 0.2.0.
/// Fassung 1 (24 Byte) haengt Farbe und HDR-Metadaten an: u8 Fassung, u8
/// Transfer, u8 Primaerfarben, u8 Matrix, u8 voll, u8 HDR-Grund, u16
/// SDR-Weiss, u16 Mastering max, u16 Mastering min, u16 MaxCLL, u16 MaxFALL
/// (Bytes und Lesen in hdr.rs, InfoV1). Eine andere Fassung liest der
/// Client wie acht Byte.
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
/// Schluesselbild mit Parametersaetzen. Acht Byte: u8 idx, u8 h264, u8
/// chroma444, u8 zehn_bit, u8 voll (immer 1), u8 Umrechnung, u8 Transfer
/// nach H.273 (1 SDR, 16 PQ; Hosts vor 0.2.0: 0 = SDR; nur fuers
/// Protokoll), u8 frei.
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
/// Abschied des Hosts: u8 Grund (HOST_ENDE_*), mindestens 1 Byte, weitere
/// Bytes werden uebergangen. Die letzte Nachricht, danach macht der Host
/// Bild- und Eingabekanal zu - bevor die App endet, die Freigabe ausgeht oder
/// wenn das Geraet dieses Zuschauers entfernt wird. Der Client beendet die
/// Sitzung sofort, zeigt den Grund und verbindet sich NICHT von selbst neu
/// (wie bei MSG_ABGELOEST). Ein aelterer Client uebergeht Typ 13 und sieht
/// danach nur das Ende der Leitung.
pub const MSG_HOST_ENDE: u8 = 13;
/// Gruende in MSG_HOST_ENDE. Ein unbekannter Grund gilt wie HOST_ENDE_BEENDET.
pub const HOST_ENDE_BEENDET: u8 = 0;
pub const HOST_ENDE_FREIGABE_AUS: u8 = 1;
pub const HOST_ENDE_ENTFERNT: u8 = 2;
/// Tonformat: u32 Rate, u8 Kanaele, u8 1 = float32 verschachtelt, 2 frei.
pub const MSG_AUDIO_INFO: u8 = 32;
/// Ton: float32 LE verschachtelt.
pub const MSG_AUDIO: u8 = 33;
/// Zwischenablage (UTF-8), in beide Richtungen unter derselben Nummer.
pub const MSG_CLIP: u8 = 48;
/// Zeigerform: 12 Byte Kopf (u16 Breite, u16 Hoehe, u16 Hotspot x,
/// u16 Hotspot y, u8 sichtbar, u8 Massstab (immer 1: ein Formpunkt je Punkt
/// des Stroms), u8 Merkmale (ZEIGER_*), u8 frei), dann RGBA mit gerader
/// Deckkraft. Kommt nur, wenn sich Form, Sichtbarkeit oder Merkmale aendern,
/// und an jeden neuen Zuschauer. Kopf lesen und schreiben: maus.rs.
pub const MSG_CURSOR: u8 = 49;
/// Merkmal Bit 0 in MSG_CURSOR (Byte 10): die Anwendung des Hosts hat die
/// Maus eingefangen (Spiel: Zeiger versteckt und eingesperrt, zurueckgesetzt
/// oder trotz Bewegung versteckt). Ein Client mit FAEHIG_MAUS faengt dann
/// seinen Zeiger ein und schickt IN_MOVE_REL. Hosts bis 0.2.0 schicken 0,
/// Clients bis 0.2.0 uebergehen das Byte und bleiben beim absoluten Weg.
pub const ZEIGER_GEFANGEN: u8 = 1;

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
/// Bit 2 in MSG_FAEHIGKEITEN: der Host (ab 0.2.0) versteht IN_ANZEIGE und
/// sendet die Strominfo Fassung 1 - auch wenn er gerade kein HDR kann; warum
/// nicht, steht in deren HDR-Grund. Der Client schickt IN_ANZEIGE nur an
/// einen Host, der es in DIESER Sitzung gemeldet hat.
pub const FAEHIG_HDR: u32 = 4;
/// Bit 3 in MSG_FAEHIGKEITEN: der Host (nach 0.2.0) meldet in MSG_CURSOR die
/// Sichtbarkeit verlaesslich (versteckt nur, wenn seine Anwendung den Zeiger
/// wirklich versteckt - nicht das Verstecken beim Tippen) und das Merkmal
/// ZEIGER_GEFANGEN, und er versteht IN_MOVE_REL und MAUS_OHNE_POSITION. Erst
/// damit versteckt der Client seinen Zeiger ueber dem Bild und faengt ein;
/// einem aelteren Host schickt er nie Typ 72.
pub const FAEHIG_MAUS: u32 = 8;

/// Flag Bit 0 im Bildkopf: Vollbild.
pub const FLAG_KEY: u8 = 0x01;

// ---------------------------------------------- Client -> Host (Eingabekanal)

/// Mausbewegung: f32 x, f32 y in 0..1 der Bildflaeche.
pub const IN_MOVE: u8 = 16;
/// Maustaste: u8 taste (0 links, 1 rechts, 2 mitte), u8 gedrueckt, u8
/// Merkmale (MAUS_*), u8 frei, f32 x, f32 y.
pub const IN_BUTTON: u8 = 17;
/// Merkmal Bit 0 in IN_BUTTON (Byte 2): ohne Position - der Host klickt, wo
/// sein Zeiger steht, und setzt ihn nicht erst auf x/y. So schickt der Client
/// Tasten, solange er eingefangen hat (relativer Weg); x/y sind dann die
/// letzte absolute Lage und nur fuer Hosts bis 0.2.0 da, die das Byte
/// uebergehen (sie bekommen den relativen Weg nie).
pub const MAUS_OHNE_POSITION: u8 = 1;
/// Rad: f32 dx, f32 dy in Pixeln.
pub const IN_SCROLL: u8 = 18;
/// Taste: u16 macOS-Keycode, u8 gedrueckt, u8 Merkmale (TASTE_*), u32
/// Umschalter (MOD_*).
pub const IN_KEY: u8 = 19;
/// Merkmal Bit 0 in IN_KEY: Wiederholung einer gehaltenen Taste durch das
/// System des Clients. Der Mac-Host speist sie als keyDown mit
/// kCGKeyboardEventAutorepeat ein, der Windows-Host als weiteren Druck (so
/// wiederholt auch eine echte Tastatur). Hosts bis 0.2.0 uebergehen das Byte
/// und sehen einen weiteren Druck; Clients bis 0.2.0 schicken 0 und keine
/// Wiederholungen.
pub const TASTE_WIEDERHOLUNG: u8 = 1;
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
/// Lage der Anzeige: 14 Byte, u8 Fassung 1, u8 Flags (Bit 0 Bildschirm
/// HDR-faehig, Bit 1 Client kann HDR darstellen), u8 Wunsch (0 Automatisch,
/// 1 Aus, 2 Immer), u8 frei, dann u16 SDR-Weiss, Spitze, Vollbild-Spitze
/// (nit), Kopfraum potentiell und aktuell (x100) - Bytes in hdr.rs
/// (Anzeige). Nur an einen Host mit FAEHIG_HDR; Zustandsnachricht.
pub const IN_ANZEIGE: u8 = 71;
/// Relative Mausbewegung: f32 dx, f32 dy (little endian) in Zaehlern der
/// Maus des Clients (Windows: Raw Input, Mac: Deltas der Mausereignisse),
/// y nach unten. Nur, solange der Client eingefangen hat, nur an einen Host
/// mit FAEHIG_MAUS (ein aelterer uebergaebe sie - unter 256 Byte -, und die
/// Maus waere tot). Der Windows-Host speist sie mit SendInput ohne
/// MOUSEEVENTF_ABSOLUTE ein (Raw Input sieht sie), der Mac-Host als
/// Mausereignis mit kCGMouseEventDeltaX/Y. Lesen: maus::rel_lesen.
pub const IN_MOVE_REL: u8 = 72;

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
