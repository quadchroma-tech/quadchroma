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
    /// Ton uebertragen: Schalter im Reiter Bild.
    Sound,
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
    CodecConverted,
    CodecSwitching,
    HostCpu,
    HostGpu,
    HostRam,
    NoDisplay,
    Encryption,
    EncryptionOn,
    EncryptionOff,
    AudioEnabled,
    ClipboardEnabled,
    // Rollen-Knoepfe fuer Anzeige und Decoder (Reiter "Anzeige"): der
    // Name der erkannten Karte steht nur im Tooltip, nie auf dem Knopf.
    DecoderLabel,
    /// Knopf "Automatik".
    RoleAuto,
    /// Knopf "Prozessor" (Anzeige ueber softbuffer, Decoder in Software).
    RoleCpu,
    /// Knopf "Grafikkarte" - die einzige dedizierte Karte.
    RoleGpu,
    /// Knopf "Grafikkarte {n}" - bei zwei dedizierten Karten; {n} bleibt stehen.
    RoleGpuN,
    /// Knopf "Integriert" - die Karte mit gemeinsamem Speicher.
    RoleIntegrated,
    /// Kleine Zeile unter der Anzeigewahl: gespeichert, aber erst ab dem
    /// naechsten Start wirksam.
    NextStartHint,
    /// Im Tooltip hinter Name und Speicher der Karte: haengt ein Bildschirm daran?
    AdapterWithOutput,
    AdapterNoOutput,
    /// Zusatz im Tooltip der Decoder-Knoepfe ohne NVIDIA: D3D11VA kann kein 4:4:4.
    TipOnly420,
    // Anzeige des Clients (Statistik im Nerd-Modus und Latenzkette)
    DisplayLabel,
    DisplayStage,
    ClientCpu,
    Monitor,
    Skipped,
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
    // Tooltips im Menue: erscheinen sofort, sobald die Maus ueber einem
    // Schalter steht - damit jeder weiss, was er tut, ohne die Doku zu lesen.
    TipBitrate,
    TipFps,
    TipGaming,
    TipSound,
    TipCodec,
    TipFullscreen,
    TipPixelExact,
    TipShowOverlay,
    TipNerdMode,
    TipStatRows,
    TipDecoderAuto,
    TipDecoderCpu,
    TipDecoderGpu,
    /// Decoder "Integriert": D3D11VA, nur 4:2:0 und H.264.
    TipDecoderIntegrated,
    /// Anzeige: die Regel der Automatik, Grafikkarte/Integriert, Prozessor.
    TipDisplayAuto,
    TipDisplayGpu,
    TipDisplayIntegrated,
    TipDisplayCpu,
    TipDisconnect,
    // Reiter "Tastenkombinationen"
    TabShortcuts,
    ShortcutMenu,
    ShortcutBack,
    ShortcutWinKey,
    ShortcutOthers,
    // Benchmark
    /// Reiter "Benchmark".
    TabBenchmark,
    /// Knopf: Lauf starten.
    BenchStart,
    /// Knopf: laufenden Lauf abbrechen.
    BenchAbort,
    /// Knopf: Empfehlung uebernehmen.
    BenchApply,
    /// Steller: Messzeit je Schritt.
    BenchDuration,
    /// Schalter: Testbild auf dem Host waehrend des Laufs.
    BenchTestPattern,
    /// Fortschritt "Schritt {n} von {m}" - beide Platzhalter bleiben stehen.
    BenchStep,
    /// Phase: Einstellungen sind unterwegs zum Host.
    BenchPhaseSettings,
    /// Phase: eine Sekunde Ruhe nach dem Einstellen.
    BenchPhaseSettle,
    /// Phase: es wird gemessen.
    BenchPhaseMeasure,
    /// Schritt kam nicht zustande (Codecwechsel oder Einstellung blieb aus).
    BenchFailed,
    /// Lauf zu Ende.
    BenchDone,
    /// Lauf abgebrochen.
    BenchAborted,
    /// Spalte: gemessene Bildrate.
    BenchMeasured,
    /// Kette = alle Glieder von der Aufnahme bis zur Anzeige.
    BenchChain,
    /// Kurze Einheit fuer Spalten und Empfehlung ("Bilder/s").
    BenchColFps,
    /// Spalte: verworfene Bilder.
    BenchColDropped,
    /// Spalte: Prozessorlast des Hosts.
    BenchColHostCpu,
    /// Ueberschrift der Empfehlung.
    BenchRecommendation,
    /// Kein Schritt hat die Regel erfuellt.
    BenchNoRecommendation,
    /// Hinweis ohne Testbild: gemessen wird, was der Mac gerade zeigt.
    BenchHint,
    /// Hinweis mit Testbild: jeder Schritt sieht denselben Inhalt.
    BenchHintPattern,
    TipBenchStart,
    TipBenchApply,
    TipBenchDuration,
    /// Tooltip auf dem Hinweis: die Regel, nach der ein Schritt besteht.
    TipBenchHint,
    TipBenchTestPattern,
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
        (Sound, "Sound"),
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
        (CodecConverted, "converted"),
        (CodecSwitching, "switching codec"),
        (HostCpu, "Host CPU"),
        (HostGpu, "Host GPU"),
        (HostRam, "Host memory"),
        (NoDisplay, "The host has no display right now"),
        (Encryption, "Encryption"),
        (EncryptionOn, "on"),
        (EncryptionOff, "off"),
        (AudioEnabled, "Audio"),
        (ClipboardEnabled, "Clipboard"),
        (DecoderLabel, "Decoder"),
        (RoleAuto, "Automatic"),
        (RoleCpu, "Processor"),
        (RoleGpu, "Graphics card"),
        (RoleGpuN, "Graphics card {n}"),
        (RoleIntegrated, "Integrated"),
        (NextStartHint, "applies from the next start"),
        (AdapterWithOutput, "with display output"),
        (AdapterNoOutput, "no display output"),
        (TipOnly420, "Only 4:2:0 and H.264 (D3D11VA); 4:4:4 falls back to the processor – switch the codec in the Picture tab."),
        (DisplayLabel, "Display"),
        (DisplayStage, "Display"),
        (ClientCpu, "Client CPU"),
        (Monitor, "Monitor"),
        (Skipped, "skipped"),
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
        (TipBitrate, "Upper limit of the bitrate. The encoder uses all of it when things move; 25 to 50 on networks that are not your own."),
        (TipFps, "Target frame rate. It cannot exceed what the Mac's display delivers."),
        (TipGaming, "More keyframes and less backlog allowed: steadier when things move, costs some bitrate."),
        (TipSound, "Send audio. Off saves about 3 Mbit/s; the host then sends no audio at all."),
        (TipCodec, "The host's codec, switched live; the picture pauses for a moment. \"converted\" costs one extra pass."),
        (TipFullscreen, "Borderless full screen. In a window the picture is fitted."),
        (TipPixelExact, "Picture 1:1 without scaling. If the window is smaller, you see a section."),
        (TipShowOverlay, "Show the statistics at the top left. Which rows, see on the right."),
        (TipNerdMode, "Extended statistics: the latency chain as a bar, load on host and client."),
        (TipStatRows, "Which rows the statistics show."),
        (TipDecoderAuto, "NVDEC on an NVIDIA card; otherwise D3D11VA on the card that draws (4:2:0 and H.264 only); otherwise the processor."),
        (TipDecoderCpu, "Decode on the processor. Works everywhere, costs CPU time."),
        (TipDecoderGpu, "Decode on the graphics card. If it cannot, the client falls back to the processor and says why."),
        (TipDecoderIntegrated, "Decode on the integrated graphics (D3D11VA). If it cannot, the client falls back to the processor and says why."),
        (TipDisplayAuto, "Direct3D 11 on the first card with a display output, otherwise the first NVIDIA card, otherwise the first there is. Without a card: the processor. Applies from the next start."),
        (TipDisplayGpu, "Draw with Direct3D 11 on this card. Applies from the next start."),
        (TipDisplayIntegrated, "Draw with Direct3D 11 on the integrated graphics – the one the screen usually hangs on. Applies from the next start."),
        (TipDisplayCpu, "Draw on the processor, without Direct3D. Works everywhere, costs CPU time. Applies from the next start."),
        (TipDisconnect, "End the connection, back to the start screen."),
        (TabShortcuts, "Shortcuts"),
        (ShortcutMenu, "Open the menu (this ESC never reaches the Mac)"),
        (ShortcutBack, "Back to the start screen"),
        (ShortcutWinKey, "Windows key = the Mac's Command key"),
        (ShortcutOthers, "All other keys go to the Mac. What travels is the key's position, not the character – accents and AltGr just work."),
        // Benchmark
        (TabBenchmark, "Benchmark"),
        (BenchStart, "Start"),
        (BenchAbort, "Cancel"),
        (BenchApply, "Apply"),
        (BenchDuration, "Duration per step"),
        (BenchTestPattern, "Test pattern"),
        (BenchStep, "Step {n} of {m}"),
        (BenchPhaseSettings, "applying settings"),
        (BenchPhaseSettle, "settling"),
        (BenchPhaseMeasure, "measuring"),
        (BenchFailed, "failed"),
        (BenchDone, "Finished"),
        (BenchAborted, "Cancelled"),
        (BenchMeasured, "measured"),
        (BenchChain, "Chain"),
        (BenchColFps, "fps"),
        (BenchColDropped, "dropped"),
        (BenchColHostCpu, "Host CPU"),
        (BenchRecommendation, "Recommendation"),
        (BenchNoRecommendation, "No step passed: chain over 30 ms or too few frames. Try lower frame rates or bitrates."),
        (BenchHint, "Measured is what happens on the Mac right now. Steps are only comparable when something moves – a running video is enough."),
        (BenchHintPattern, "With the test pattern on, every step sees the same moving content – nothing needs to run on the Mac."),
        (TipBenchStart, "Runs every combination of the chosen codecs, frame rates and bitrates in turn, each for the set duration. The picture keeps running and the menu may stay open."),
        (TipBenchApply, "Set the recommended codec and settings and save them for this host."),
        (TipBenchDuration, "Measuring time per step. Longer is steadier; 5 s is usually enough."),
        (TipBenchHint, "A step passes when at least 95 % of the target frame rate arrives, under 1 % of the frames are dropped and the whole chain stays within 30 ms – regardless of the frame rate. The host's encoder time is shown but not judged. Recommended: the best colour quality that passes at the highest frame rate, at the highest bitrate."),
        (TipBenchTestPattern, "For the duration of the run the host shows a fixed moving pattern instead of the screen – every step measures the same content."),
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
        (Sound, "Ton"),
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
        (CodecConverted, "mit Umrechnung"),
        (CodecSwitching, "Codec wird gewechselt"),
        (HostCpu, "Host-Prozessor"),
        (HostGpu, "Host-Grafik"),
        (HostRam, "Host-Speicher"),
        (NoDisplay, "Der Host hat gerade keinen Bildschirm"),
        (Encryption, "Verschlüsselung"),
        (EncryptionOn, "an"),
        (EncryptionOff, "aus"),
        (AudioEnabled, "Ton"),
        (ClipboardEnabled, "Zwischenablage"),
        (DecoderLabel, "Decoder"),
        (RoleAuto, "Automatik"),
        (RoleCpu, "Prozessor"),
        (RoleGpu, "Grafikkarte"),
        (RoleGpuN, "Grafikkarte {n}"),
        (RoleIntegrated, "Integriert"),
        (NextStartHint, "gilt ab dem nächsten Start"),
        (AdapterWithOutput, "mit Bildschirmausgang"),
        (AdapterNoOutput, "ohne Bildschirmausgang"),
        (TipOnly420, "Nur 4:2:0 und H.264 (D3D11VA); bei 4:4:4 fällt der Client auf den Prozessor zurück – Codec im Reiter Bild umstellen."),
        (DisplayLabel, "Anzeige"),
        (DisplayStage, "Anzeige"),
        (ClientCpu, "Client-CPU"),
        (Monitor, "Monitor"),
        (Skipped, "ausgelassen"),
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
        (TipBitrate, "Obergrenze der Datenrate. Bei Bewegung nutzt der Encoder sie aus; in fremden Netzen lieber 25 bis 50."),
        (TipFps, "Zielbildrate. Mehr, als der Bildschirm des Macs liefert, geht nicht."),
        (TipGaming, "Häufigere Vollbilder und weniger erlaubter Rückstau: gleichmäßiger bei Bewegung, kostet etwas Datenrate."),
        (TipSound, "Ton übertragen. Aus spart rund 3 Mbit/s; der Host schickt dann keinen Ton."),
        (TipCodec, "Codec des Hosts, Wechsel im laufenden Betrieb; das Bild steht kurz still. „mit Umrechnung“ kostet einen Durchlauf mehr."),
        (TipFullscreen, "Randloses Vollbild. Im Fenster wird das Bild eingepasst."),
        (TipPixelExact, "Bild 1:1 ohne Skalierung. Ist das Fenster kleiner, wird ein Ausschnitt gezeigt."),
        (TipShowOverlay, "Statistik links oben einblenden. Welche Zeilen, steht rechts."),
        (TipNerdMode, "Erweiterte Statistik: Verzögerungskette als Balken, Auslastung von Host und Client."),
        (TipStatRows, "Welche Zeilen die Statistik zeigt."),
        (TipDecoderAuto, "NVDEC auf einer NVIDIA-Karte; sonst D3D11VA auf der Karte, die zeichnet (nur 4:2:0 und H.264); sonst der Prozessor."),
        (TipDecoderCpu, "Decodieren auf dem Prozessor. Läuft überall, kostet Rechenzeit."),
        (TipDecoderGpu, "Decodieren auf der Grafikkarte. Geht sie nicht, fällt der Client auf den Prozessor zurück und sagt warum."),
        (TipDecoderIntegrated, "Decodieren auf der integrierten Grafik (D3D11VA). Geht sie nicht, fällt der Client auf den Prozessor zurück und sagt warum."),
        (TipDisplayAuto, "Direct3D 11 auf der ersten Karte mit Bildschirmausgang, sonst der ersten von NVIDIA, sonst der ersten überhaupt. Ohne Karte: der Prozessor. Gilt ab dem nächsten Start."),
        (TipDisplayGpu, "Zeichnen mit Direct3D 11 auf dieser Karte. Gilt ab dem nächsten Start."),
        (TipDisplayIntegrated, "Zeichnen mit Direct3D 11 auf der integrierten Grafik – an der meist der Bildschirm hängt. Gilt ab dem nächsten Start."),
        (TipDisplayCpu, "Zeichnen auf dem Prozessor, ohne Direct3D. Läuft überall, kostet Rechenzeit. Gilt ab dem nächsten Start."),
        (TipDisconnect, "Verbindung beenden, zurück zum Startbildschirm."),
        (TabShortcuts, "Tastenkombinationen"),
        (ShortcutMenu, "Menü öffnen (dieses ESC kommt nicht beim Mac an)"),
        (ShortcutBack, "Zurück zum Startbildschirm"),
        (ShortcutWinKey, "Windows-Taste = Befehlstaste des Macs"),
        (ShortcutOthers, "Alle anderen Tasten gehen an den Mac. Übertragen wird die Position der Taste, nicht das Zeichen – Umlaute und AltGr stimmen von selbst."),
        // Benchmark
        (TabBenchmark, "Benchmark"),
        (BenchStart, "Start"),
        (BenchAbort, "Abbrechen"),
        (BenchApply, "Übernehmen"),
        (BenchDuration, "Dauer je Schritt"),
        (BenchTestPattern, "Testbild"),
        (BenchStep, "Schritt {n} von {m}"),
        (BenchPhaseSettings, "Einstellungen werden gesetzt"),
        (BenchPhaseSettle, "einschwingen"),
        (BenchPhaseMeasure, "messen"),
        (BenchFailed, "gescheitert"),
        (BenchDone, "Fertig"),
        (BenchAborted, "Abgebrochen"),
        (BenchMeasured, "gemessen"),
        (BenchChain, "Kette"),
        (BenchColFps, "Bilder/s"),
        (BenchColDropped, "verworfen"),
        (BenchColHostCpu, "Host-CPU"),
        (BenchRecommendation, "Empfehlung"),
        (BenchNoRecommendation, "Kein Schritt hat bestanden: Kette über 30 ms oder zu wenige Bilder. Niedrigere Bildraten oder Datenraten versuchen."),
        (BenchHint, "Gemessen wird, was gerade auf dem Mac passiert. Vergleichbar sind die Schritte nur, wenn sich etwas bewegt – ein laufendes Video reicht."),
        (BenchHintPattern, "Mit Testbild sieht jeder Schritt denselben bewegten Inhalt – auf dem Mac muss nichts laufen."),
        (TipBenchStart, "Misst alle Kombinationen der gewählten Codecs, Bildraten und Datenraten der Reihe nach, jede für die eingestellte Dauer. Das Bild läuft weiter, das Menü darf offen bleiben."),
        (TipBenchApply, "Empfohlenen Codec und Einstellungen setzen und für diesen Host speichern."),
        (TipBenchDuration, "Messzeit je Schritt. Länger ist ruhiger; 5 s reichen meist."),
        (TipBenchHint, "Ein Schritt besteht, wenn mindestens 95 % der Zielbildrate ankommen, unter 1 % der Bilder verworfen werden und die ganze Kette höchstens 30 ms lang ist – unabhängig von der Bildrate. Die Encoderzeit des Hosts wird nur angezeigt, nicht bewertet. Empfohlen: die beste Farbqualität, die bei der höchsten Bildrate besteht, mit der höchsten Datenrate."),
        (TipBenchTestPattern, "Der Host zeigt für die Dauer des Laufs ein festes, bewegtes Muster statt des Bildschirms – jeder Schritt misst denselben Inhalt."),
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
