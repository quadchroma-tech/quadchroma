// Sprachen. Ein Schluessel je Text, eine Tabelle je Sprache.
//
// Die Auswahl richtet sich beim Start nach der Systemsprache und laesst sich in
// den Einstellungen ueberschreiben. Fehlt eine Uebersetzung, wird Englisch
// genommen, damit nie ein leerer Text im Fenster steht.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    // Startfenster
    AppSubtitle,
    Connect,
    Disconnect,
    HostAddress,
    SearchingHosts,
    NoHostsFound,
    FoundHosts,
    Settings,
    Language,
    Quit,
    // Sitzung
    Connecting,
    Connected,
    ConnectionLost,
    Reconnecting,
    Fullscreen,
    PixelExact,
    Stretch,
    ShowOverlay,
    // Anzeigewerte
    Fps,
    DecodeTime,
    Bitrate,
    Resolution,
    Codec,
    Latency,
    EncodeTime,
    NetworkTime,
    AudioBuffer,
    Dropped,
    // Einstellungen
    MaxBitrate,
    MaxFps,
    ExpertMode,
    GamingMode,
    FixedRate,
    FixedRateHint,
    HoldEscHint,
    NerdMode,
    More,
    Back,
    SavedForHost,
    Queue,
    OneFrame,
    Bottleneck,
    TabPicture,
    TabDisplay,
    HostCpu,
    HostGpu,
    HostRam,
    Encryption,
    EncryptionOn,
    EncryptionOff,
    AudioEnabled,
    ClipboardEnabled,
    // Kopplung
    PairingTitle,
    PairingHint,
    PairingCode,
    PairingWrong,
    PairingOk,
    SecuredWith,
    HostFingerprint,
    NotPaired,
    FirstContact,
    // Fehler
    ErrorConnectRefused,
    ErrorTimeout,
    ErrorProtocol,
    ErrorNoDecoder,
}

pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
    pub(crate) table: &'static [(Key, &'static str)],
}

impl Lang {
    pub fn get(&self, k: Key) -> &'static str {
        for (key, text) in self.table {
            if *key == k {
                return text;
            }
        }
        for (key, text) in EN.table {
            if *key == k {
                return text;
            }
        }
        ""
    }
}

use Key::*;

pub static EN: Lang = Lang {
    code: "en",
    name: "English",
    table: &[
        (AppSubtitle, "Remote desktop with full colour detail"),
        (Connect, "Connect"),
        (Disconnect, "Disconnect"),
        (HostAddress, "Host address"),
        (SearchingHosts, "Searching for hosts"),
        (NoHostsFound, "No hosts found"),
        (FoundHosts, "Hosts found"),
        (Settings, "Settings"),
        (Language, "Language"),
        (Quit, "Quit"),
        (Connecting, "Connecting"),
        (Connected, "Connected"),
        (ConnectionLost, "Connection lost"),
        (Reconnecting, "Reconnecting"),
        (Fullscreen, "Fullscreen"),
        (PixelExact, "Pixel exact"),
        (Stretch, "Fit to window"),
        (ShowOverlay, "Show statistics"),
        (Fps, "Frames per second"),
        (DecodeTime, "Decode time"),
        (Bitrate, "Bitrate"),
        (Resolution, "Resolution"),
        (Codec, "Codec"),
        (Latency, "Latency"),
        (EncodeTime, "Encoding"),
        (NetworkTime, "Network"),
        (AudioBuffer, "Audio buffer"),
        (Dropped, "Dropped"),
        (MaxBitrate, "Maximum bitrate"),
        (MaxFps, "Maximum frame rate"),
        (ExpertMode, "Expert mode"),
        (GamingMode, "Gaming mode"),
        (FixedRate, "Fixed frame rate"),
        (FixedRateHint, "Holds the rate even when nothing moves. Constant load on the host."),
        (HoldEscHint, "Keep holding ESC for the menu"),
        (NerdMode, "Nerd mode"),
        (More, "More"),
        (Back, "Back"),
        (SavedForHost, "Saved for this host"),
        (Queue, "Capture & queue"),
        (OneFrame, "One frame"),
        (Bottleneck, "Bottleneck"),
        (TabPicture, "Picture"),
        (TabDisplay, "Display"),
        (HostCpu, "Host CPU"),
        (HostGpu, "Host GPU"),
        (HostRam, "Host memory"),
        (Encryption, "Encryption"),
        (EncryptionOn, "on"),
        (EncryptionOff, "off"),
        (AudioEnabled, "Audio"),
        (ClipboardEnabled, "Clipboard"),
        (PairingTitle, "Pair with host"),
        (PairingHint, "Enter the code shown on the host"),
        (PairingCode, "Pairing code"),
        (PairingWrong, "Wrong code"),
        (PairingOk, "Paired"),
        (SecuredWith, "Secured · comparison code"),
        (HostFingerprint, "Host fingerprint"),
        (NotPaired, "This host does not know your device yet. Start the host with --pair and connect again."),
        (FirstContact, "First contact with this host. Compare the code with the one on the host."),
        (ErrorConnectRefused, "Host refused the connection"),
        (ErrorTimeout, "Host did not answer"),
        (ErrorProtocol, "The other side speaks a different protocol"),
        (ErrorNoDecoder, "No decoder for this format"),
    ],
};

pub static DE: Lang = Lang {
    code: "de",
    name: "Deutsch",
    table: &[
        (AppSubtitle, "Fernsteuerung mit voller Farbauflösung"),
        (Connect, "Verbinden"),
        (Disconnect, "Trennen"),
        (HostAddress, "Adresse des Hosts"),
        (SearchingHosts, "Suche nach Hosts"),
        (NoHostsFound, "Kein Host gefunden"),
        (FoundHosts, "Gefundene Hosts"),
        (Settings, "Einstellungen"),
        (Language, "Sprache"),
        (Quit, "Beenden"),
        (Connecting, "Verbinde"),
        (Connected, "Verbunden"),
        (ConnectionLost, "Verbindung verloren"),
        (Reconnecting, "Verbinde neu"),
        (Fullscreen, "Vollbild"),
        (PixelExact, "Pixelgenau"),
        (Stretch, "An Fenster anpassen"),
        (ShowOverlay, "Zahlen einblenden"),
        (Fps, "Bilder pro Sekunde"),
        (DecodeTime, "Decodierzeit"),
        (Bitrate, "Datenrate"),
        (Resolution, "Auflösung"),
        (Codec, "Codec"),
        (Latency, "Verzögerung"),
        (EncodeTime, "Encoder"),
        (NetworkTime, "Leitung"),
        (AudioBuffer, "Tonpuffer"),
        (Dropped, "Ausgelassene Bilder"),
        (MaxBitrate, "Höchste Datenrate"),
        (MaxFps, "Höchste Bildrate"),
        (ExpertMode, "Expertenmodus"),
        (GamingMode, "Spielmodus"),
        (FixedRate, "Feste Bildrate"),
        (FixedRateHint, "Hält die Rate, auch wenn sich nichts bewegt. Dauerlast auf dem Host."),
        (HoldEscHint, "ESC weiter halten für das Menü"),
        (NerdMode, "Nerd-Modus"),
        (More, "Mehr"),
        (Back, "Zurück"),
        (SavedForHost, "Für diesen Host gespeichert"),
        (Queue, "Aufnahme & Warten"),
        (OneFrame, "Ein Bild"),
        (Bottleneck, "Engpass"),
        (TabPicture, "Bild"),
        (TabDisplay, "Anzeige"),
        (HostCpu, "Host-Prozessor"),
        (HostGpu, "Host-Grafik"),
        (HostRam, "Host-Speicher"),
        (Encryption, "Verschlüsselung"),
        (EncryptionOn, "an"),
        (EncryptionOff, "aus"),
        (AudioEnabled, "Ton"),
        (ClipboardEnabled, "Zwischenablage"),
        (PairingTitle, "Mit Host koppeln"),
        (PairingHint, "Code eingeben, der auf dem Host steht"),
        (PairingCode, "Kopplungscode"),
        (PairingWrong, "Falscher Code"),
        (PairingOk, "Gekoppelt"),
        (SecuredWith, "Gesichert · Vergleichscode"),
        (HostFingerprint, "Fingerabdruck des Hosts"),
        (NotPaired, "Dieser Host kennt dein Gerät noch nicht. Host mit --pair starten und erneut verbinden."),
        (FirstContact, "Erstkontakt mit diesem Host. Code mit dem auf dem Host vergleichen."),
        (ErrorConnectRefused, "Host hat die Verbindung abgelehnt"),
        (ErrorTimeout, "Host antwortet nicht"),
        (ErrorProtocol, "Die Gegenstelle spricht ein anderes Protokoll"),
        (ErrorNoDecoder, "Kein Decoder für dieses Format"),
    ],
};

/// Alle verfuegbaren Sprachen. Weitere Tabellen kommen aus strings_more.rs.
pub fn all() -> &'static [&'static Lang] {
    crate::strings_more::ALL
}

/// Sprache anhand des Systems waehlen, sonst Englisch.
pub fn pick(system: &str) -> &'static Lang {
    let low = system.to_ascii_lowercase();
    // Erst genau, dann nur der Sprachteil vor dem Bindestrich.
    for l in all() {
        if low == l.code {
            return l;
        }
    }
    let short = low.split(['-', '_']).next().unwrap_or("");
    for l in all() {
        if short == l.code {
            return l;
        }
    }
    &EN
}
