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

/// Kennung nach dem Handschlag auf dem Bildkanal.
pub const MAGIC: &[u8; 4] = b"QCH1";

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

// Umschalter als Bitmaske, damit der Mac denselben Zustand sieht wie Windows.
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_CMD: u32 = 8;

// -------------------------------------------------------------- Bekanntgabe

/// UDP-Rundruf des Hosts auf Bildport + 2: "QCHB" | u8 Version | u16 Bildport
/// | u8 Namenslaenge | Name (UTF-8), alle zwei Sekunden.
pub const BEACON_MAGIC: &[u8; 4] = b"QCHB";
pub const BEACON_VERSION: u8 = 1;
