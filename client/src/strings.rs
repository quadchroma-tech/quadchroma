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
    /// Der Host meldet: ein anderes Geraet hat die Sitzung uebernommen
    /// (Nachricht 10). Keine automatische Neuverbindung.
    SessionTakenOver,
    /// Fingerabdruck einer bekannten Adresse geaendert: {n} Adresse, {m} neuer
    /// Fingerabdruck, {p} Pfad von known_hosts.txt - alle bleiben stehen.
    HostKeyChanged,
    /// client.key hat die falsche Laenge; {p} ist der Pfad.
    KeyFileDamaged,
    /// Datei vorhanden, aber nicht lesbar; {p} ist der Pfad. Dahinter in
    /// Klammern der Wortlaut des Systems.
    FileUnreadable,
    /// Vertrauensliste nicht in UTF-8; {p} ist der Pfad.
    FileNotUtf8,
    /// Neuer Schluessel oder Eintrag laesst sich nicht schreiben; {p} ist der
    /// Pfad, dahinter der Wortlaut des Systems.
    FileNotWritable,
    /// Kein Ablageordner fuer Schluessel und Einstellungen.
    StorageUnavailable,
    /// Adresse ergibt kein Ziel; {n} ist die Adresse.
    ErrorAddress,
    /// Verbindung scheitert aus anderem Grund als Ablehnung oder Frist;
    /// dahinter der Wortlaut des Systems.
    ErrorNoConnection,
    /// Der Noise-Handschlag scheitert (nicht an der Frist).
    ErrorHandshake,
    /// FFmpeg laesst sich nicht starten; dahinter FFmpegs Wortlaut.
    ErrorFfmpegStart,
    /// Tonausgabe laesst sich nicht oeffnen; dahinter der Grund.
    ErrorSound,
    /// Anzeige auf der Karte gewuenscht, laeuft aber ueber Software.
    ErrorGpuDisplay,
    /// Grafikkarte waehrend der Sitzung verloren, Neubau gescheitert.
    ErrorGpuLost,
    /// Der Decoder liefert ein Bildformat, das der Client nicht wandeln kann.
    ErrorPixelFormat,
    /// Kartenwunsch fuer den Decoder nicht erfuellt, Software laeuft.
    DecoderFallback,
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
    // Desktop-Verknuepfung (nur Windows-Client)
    /// Knopf je Hostzeile auf dem Startbildschirm und im Reiter
    /// Verschluesselung. Nicht zu verwechseln mit TabShortcuts/Shortcut*
    /// (Tastenkombinationen).
    DesktopShortcut,
    /// Tooltip zum Knopf.
    TipDesktopShortcut,
    /// Ergebnis: {n} ist der Dateiname der Verknuepfung.
    DesktopShortcutCreated,
    /// Fehlschlag: {n} ist der Grund.
    DesktopShortcutFailed,
    /// Kommentar der Verknuepfung im Explorer: {n} Name, {m} Adresse.
    DesktopShortcutDescription,
    // Dateien ueber die Zwischenablage (Zeile unten mittig ueber dem Bild)
    /// Senden laeuft: {n} quittierte MB, {m} Gesamtgroesse samt "MB".
    FilesSending,
    /// Empfangen laeuft: {n} geschriebene MB, {m} Gesamtgroesse samt "MB".
    FilesReceiving,
    /// Empfangen und in die Ablage gelegt: {n} Zahl der obersten Eintraege.
    FilesReady,
    /// Gesendet und quittiert: {n} Zahl der obersten Eintraege.
    FilesSent,
    /// Abgebrochen: {n} der Grund, einer der Texte FilesAbort* (Einzelheiten
    /// wie Pfade und Systemtexte stehen nur im Protokoll).
    FilesAborted,
    /// Ueber den Grenzen (2.6), nichts uebertragen.
    FilesTooLarge,
    /// Die Gegenseite hat keine Faehigkeit "Dateien" gemeldet.
    FilesPeerOld,
    // Symbol im Infobereich (Windows) bzw. in der Menueleiste (macOS)
    /// Menuepunkt: Fenster zeigen.
    TrayOpen,
    /// Menuepunkt je gefundenem Host: {n} ist sein Name bzw. seine Adresse.
    TrayConnect,
    /// Menuepunkt: Programm beenden.
    TrayQuit,
    /// Einmalige Sprechblase beim ersten Ablegen (Windows).
    TrayStillRunning,
    /// Dasselbe fuer die Menueleiste (macOS).
    TrayStillRunningMac,
    // Abbruchgruende fuer FilesAborted ({n}), zugeordnet in main.rs
    // (abbruch_schluessel) aus dateien::Abbruch
    /// Auf diesem Geraet abgebrochen: neuer Inhalt, Trennen, Kanal weg.
    FilesAbortLocal,
    /// Die Gegenseite hat abgebrochen (Ende 1, Quittung 6).
    FilesAbortPeer,
    /// Die Verbindung ist weg.
    FilesAbortConnection,
    /// 30 s ohne Fortschritt (Ende 3, Quittung 6 beim Empfaenger).
    FilesAbortTimeout,
    /// Zu wenig Platz (Quittung 3).
    FilesAbortNoSpace,
    /// Ungueltige Daten (Quittung 4, Protokollfehler).
    FilesAbortInvalid,
    /// Lesefehler beim Sender (Ende 2).
    FilesAbortRead,
    /// Schreibfehler beim Empfaenger (Quittung 5).
    FilesAbortWrite,
    /// Die Ablage liess sich nicht setzen.
    FilesAbortClipboard,
    // Bildschirmwahl im Reiter "Bild" (Spezifikation Bildschirmwahl 3.1)
    /// Zeile "Bildschirm" ueber den Codecknoepfen.
    ScreenLabel,
    /// Knopf "Automatisch": der Host folgt seinem Hauptbildschirm.
    ScreenAuto,
    /// Tooltip der Bildschirmzeile.
    TipScreen,
    /// Ein Bildschirmwunsch ist unterwegs.
    ScreenSwitching,
    /// {n} = gewuenschte Kennung, {m} = der Bildschirm, der stattdessen laeuft.
    ScreenFallback,
    // Zugang (Spezifikation Pairing v1, Abschnitt 14). EN und DE wie dort;
    // die uebrigen 27 Tabellen uebersetzt (Paket P6) - jeder Text, den auch
    // der Mac-Host fuehrt, steht in host/texte.m wortgleich (Test
    // zugang_texte_wie_mac_host).
    // Platzhalter: {n} Name, {i} ID "ddd ddd ddd", {c} Code "628 306",
    // {s} Sekunden, {p} Passwort bzw. Port, {d} Datum.
    // Client: Zugangsdialog und Meldungen
    /// Zugangsdialog, Titel: {n} Name des Hosts.
    AccessTitle,
    /// Zugangsdialog, Erklaerung: {n} Name des Hosts (zweimal).
    AccessText,
    /// Zugangsdialog, nur wenn der Host "Zulassen" anbietet (Bit 1): {n} Name des Hosts.
    AccessOrAllow,
    /// Zugangsdialog: Vergleichscode, {c} = "628 306".
    AccessCode,
    /// Zugangsdialog: Beschriftung des Passwortfelds.
    AccessPassword,
    /// Zugangsdialog: Knopf, der das Passwort lesbar zeigt.
    AccessShow,
    /// Zugangsdialog: Knopf Abbrechen (sendet ZUGANG_ABBRUCH).
    AccessCancel,
    /// Zugangsdialog: Beweis ist unterwegs, Knopf deaktiviert.
    AccessChecking,
    /// Zugangsdialog: Ergebnis 2, Passwort falsch.
    AccessWrong,
    /// Zugangsdialog: Drossel, {s} Sekunden bis zum naechsten Versuch.
    AccessWait,
    /// Zugangsdialog (8.3): anderer Schluessel unter bekannter Adresse, {i} neue ID.
    AccessNewIdentity,
    /// Meldung: Ergebnis 3, am Host abgelehnt; {n} Name des Hosts.
    MsgRefused,
    /// Meldung: keine Antwort (Frist); {n} Name des Hosts.
    MsgNoAnswer,
    /// Meldung: Ergebnis 4, zu viele Versuche.
    MsgTooManyAttempts,
    /// Meldung: Host antwortet weder QCH1 noch QCA1; {n} Name des Hosts.
    MsgHostOutdated,
    /// Meldung: host_proof falsch (moeglicher Angriff); {n} Name des Hosts.
    MsgHostProofBad,
    /// Meldung (8.2): der Schluessel passt nicht zur gewaehlten ID.
    MsgOtherDevice,
    /// Meldung (9.2): {i} die gesuchte ID.
    MsgIdNotFound,
    // Client: Startbildschirm
    /// Hostzeile rechts: {i} ID "ddd ddd ddd".
    StartId,
    /// Knopf (Windows): Host-Rolle starten.
    StartShare,
    /// Derselbe Knopf, wenn die Host-Rolle schon laeuft (deaktiviert).
    StartSharing,
    // Host: Windows-Host-Rolle (Mac-Host: dieselben Texte in host/texte.m;
    // die mit (Mac) markierten braucht nur er - sie stehen hier, damit beide
    // Dateien dieselben Schluessel und Uebersetzungen fuehren)
    /// Host-Menue, Kopfzeile: niemand verbunden.
    HostReady,
    /// Host-Menue, Kopfzeile: {n} Name des Zuschauers.
    HostConnected,
    /// Host-Menue: eigene ID, {i} "ddd ddd ddd".
    HostDeviceId,
    /// Host-Menue: {p} das Zugangspasswort.
    HostPassword,
    /// Host: Hinweis nach dem Kopieren von ID oder Passwort.
    HostCopied,
    /// Host-Menue: Fenster zum Aendern des Passworts oeffnen.
    HostChangePassword,
    /// Host-Menue: neues Zufallspasswort erzeugen.
    HostRandomPassword,
    /// Passwortfenster: erstes Feld.
    HostNewPassword,
    /// Passwortfenster: zweites Feld.
    HostRepeatPassword,
    /// Passwortfenster: die Felder sind verschieden.
    HostPasswordsDiffer,
    /// Passwortfenster: norm(pw) kuerzer als 8 Byte.
    HostPasswordShort,
    /// Passwortfenster: gespeichert.
    HostPasswordSaved,
    /// Host-Menue: host-password.txt unlesbar.
    HostPasswordUnreadable,
    /// Host-Menue: Untermenue der erlaubten Geraete.
    HostDevices,
    /// Host-Menue: die Liste ist leer.
    HostNoDevices,
    /// Host-Menue, je Geraet: {n} Name, {i} ID, {d} Datum.
    HostDeviceLine,
    /// Host-Menue, je Geraet: entfernen.
    HostRemove,
    /// Host-Menue: alle Geraete entfernen (mit Rueckfrage).
    HostRemoveAll,
    /// Rueckfrage zu "Alle entfernen".
    HostRemoveAllAsk,
    /// Host-Menue: host-devices.txt beschaedigt.
    HostListDamaged,
    /// Host-Menue: beschaedigte Liste neu anlegen (alte bleibt als .defekt-<zeit>).
    HostListReset,
    /// Zulassen-Fenster: {n} Name des Clients, {i} seine ID.
    HostRequest,
    /// Zulassen-Fenster: Knopf Zulassen (Standard).
    HostAllow,
    /// Zulassen-Fenster: Knopf Ablehnen.
    HostDeny,
    /// Host-Menue (Mac): beim Anmelden starten.
    HostStartLogin,
    /// Host-Menue (Windows): mit Windows starten.
    HostStartWindows,
    /// Host-Menue (Mac): App liegt nicht in /Applications.
    HostMoveToApps,
    /// Host-Menue (Mac): Bildschirmaufnahme fehlt.
    HostScreenMissing,
    /// Host-Menue (Mac): Bedienungshilfen fehlen.
    HostAccessMissing,
    /// Host-Menue: {p} der belegte Port.
    HostPortBusy,
    /// Host-Menue (Mac): beenden.
    HostQuit,
    /// Host-Menue (Windows): Host-Rolle beenden.
    HostStopSharing,
    /// Infobereich (Windows): Tooltip, {i} die eigene ID.
    HostTooltip,
    /// Host-Fenster: Knopf OK.
    HostOk,
    // Nachgetragen bei der Zusammenfuehrung (gemeldet von P3/P4/P5)
    /// Zugangsdialog: Gegenstueck zu AccessShow, solange das Passwort
    /// lesbar angezeigt wird.
    AccessHide,
    /// Meldung (9.5): "Diesen PC freigeben" konnte die Host-Rolle nicht starten.
    MsgShareFailed,
    /// Meldung: {n} Name des Hosts - er hat dieses Geraet entfernt und
    /// antwortet beim Wiederverbinden mit der Zugangsphase.
    MsgDeviceRemoved,
    /// Passwortfenster: Schreibfehler beim Speichern (das alte gilt weiter).
    HostPasswordNotSaved,
    /// Passwortfenster: unzulaessige Zeichen (Zeilenumbruch, Steuerzeichen,
    /// zu lang).
    HostPasswordInvalid,
    /// Passwortfenster: Titel.
    HostPasswordTitle,
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
        (SessionTakenOver, "Another device has taken over the session."),
        (HostKeyChanged, "The fingerprint of {n} has changed (now {m}). Connection refused. If the host was set up again, delete its line in {p}."),
        (KeyFileDamaged, "The key file {p} is damaged and was left as it is. Check or delete it – a new key is then created, and the host has to pair this device again."),
        (FileUnreadable, "{p} cannot be read – not connecting."),
        (FileNotUtf8, "{p} is not saved as UTF-8 – not connecting."),
        (FileNotWritable, "{p} cannot be written – not connecting."),
        (StorageUnavailable, "The folder for keys and settings is not available – not connecting."),
        (ErrorAddress, "The address {n} cannot be resolved"),
        (ErrorNoConnection, "No connection to the host"),
        (ErrorHandshake, "The secure connection could not be established"),
        (ErrorFfmpegStart, "FFmpeg could not be started"),
        (ErrorSound, "Sound is not available"),
        (ErrorGpuDisplay, "Graphics card not usable, display runs in software"),
        (ErrorGpuLost, "Graphics card lost – please restart with --anzeige cpu"),
        (ErrorPixelFormat, "The decoder delivers pictures this client cannot display"),
        (DecoderFallback, "The selected graphics card does not decode – software decoder in use"),
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
        (DesktopShortcut, "Desktop shortcut"),
        (TipDesktopShortcut, "Creates a shortcut to this host on the desktop. Double-clicking it connects right away."),
        (DesktopShortcutCreated, "Shortcut created on the desktop: {n}"),
        (DesktopShortcutFailed, "Shortcut not created: {n}"),
        (DesktopShortcutDescription, "QuadChroma: connect to {n} ({m})"),
        (FilesSending, "Sending files: {n} of {m}"),
        (FilesReceiving, "Receiving files: {n} of {m}"),
        (FilesReady, "Files ready to paste: {n}"),
        (FilesSent, "Files transferred: {n}"),
        (FilesAborted, "File transfer aborted: {n}"),
        (FilesTooLarge, "Files not transferred: more than 4 GB or 10,000 items"),
        (FilesPeerOld, "The other side cannot receive files yet."),
        (TrayOpen, "Open"),
        (TrayConnect, "Connect: {n}"),
        (TrayQuit, "Quit"),
        (TrayStillRunning, "QuadChroma is still running in the notification area."),
        (TrayStillRunningMac, "QuadChroma is still running in the menu bar."),
        (FilesAbortLocal, "by this device"),
        (FilesAbortPeer, "by the other side"),
        (FilesAbortConnection, "connection interrupted"),
        (FilesAbortTimeout, "timeout, no progress"),
        (FilesAbortNoSpace, "not enough disk space"),
        (FilesAbortInvalid, "invalid data"),
        (FilesAbortRead, "read error"),
        (FilesAbortWrite, "write error"),
        (FilesAbortClipboard, "clipboard not available"),
        (ScreenLabel, "Screen"),
        (ScreenAuto, "Automatic"),
        (TipScreen, "Which of the host's screens you see. Automatic follows the main screen; a fixed screen stays until you choose something else."),
        (ScreenSwitching, "Switching screen …"),
        (ScreenFallback, "{n} not connected – fallback: {m}"),
        // Zugang (Spezifikation Pairing v1, Abschnitt 14)
        (AccessTitle, "Password for {n}"),
        (AccessText, "{n} does not know this device yet. Enter the password shown in the QuadChroma menu on {n}."),
        (AccessOrAllow, "Or ask someone at {n} to click \"Allow\"."),
        (AccessCode, "Code: {c}"),
        (AccessPassword, "Password"),
        (AccessShow, "Show"),
        (AccessCancel, "Cancel"),
        (AccessChecking, "Checking ..."),
        (AccessWrong, "Wrong password."),
        (AccessWait, "Too many attempts. Try again in {s} s."),
        (AccessNewIdentity, "The device at this address has a new identity (ID {i})."),
        (MsgRefused, "{n} refused the connection."),
        (MsgNoAnswer, "No answer from {n}. Please try again."),
        (MsgTooManyAttempts, "Too many attempts. Please wait a moment and try again."),
        (MsgHostOutdated, "{n} uses an older QuadChroma version. Please update it there."),
        (MsgHostProofBad, "{n} could not confirm the password. The connection was stopped for safety."),
        (MsgOtherDevice, "A different device answers at this address."),
        (MsgIdNotFound, "No device with ID {i} found in the network."),
        (StartId, "ID {i}"),
        (StartShare, "Share this PC"),
        (StartSharing, "Sharing is on"),
        (HostReady, "Ready for connections"),
        (HostConnected, "Connected: {n}"),
        (HostDeviceId, "Device ID: {i}"),
        (HostPassword, "Password: {p}"),
        (HostCopied, "Copied"),
        (HostChangePassword, "Change password ..."),
        (HostRandomPassword, "New random password"),
        (HostNewPassword, "New password (at least 8 characters)"),
        (HostRepeatPassword, "Repeat password"),
        (HostPasswordsDiffer, "The passwords do not match."),
        (HostPasswordShort, "At least 8 characters, please."),
        (HostPasswordSaved, "Password saved."),
        (HostPasswordUnreadable, "Password file unreadable - new devices only via \"Allow\"."),
        (HostDevices, "Allowed devices"),
        (HostNoDevices, "No devices yet"),
        (HostDeviceLine, "{n} - ID {i} - since {d}"),
        (HostRemove, "Remove"),
        (HostRemoveAll, "Remove all devices ..."),
        (HostRemoveAllAsk, "Remove all allowed devices? They will need the password again."),
        (HostListDamaged, "Device list damaged"),
        (HostListReset, "Reset device list"),
        (HostRequest, "{n} (ID {i}) wants to control this computer."),
        (HostAllow, "Allow"),
        (HostDeny, "Deny"),
        (HostStartLogin, "Start at login"),
        (HostStartWindows, "Start with Windows"),
        (HostMoveToApps, "Move QuadChroma to Applications first"),
        (HostScreenMissing, "Screen Recording not allowed - open System Settings ..."),
        (HostAccessMissing, "Accessibility not allowed - open System Settings ..."),
        (HostPortBusy, "Port {p} is used by another program"),
        (HostQuit, "Quit QuadChroma"),
        (HostStopSharing, "Stop sharing"),
        (HostTooltip, "QuadChroma - sharing this PC (ID {i})"),
        (HostOk, "OK"),
        (AccessHide, "Hide"),
        (MsgShareFailed, "Could not start sharing this PC."),
        (MsgDeviceRemoved, "{n} no longer knows this device. Connect again and enter the password."),
        (HostPasswordNotSaved, "The password could not be saved."),
        (HostPasswordInvalid, "The password contains characters that are not allowed."),
        (HostPasswordTitle, "Change password"),
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
        (SessionTakenOver, "Ein anderes Gerät hat die Sitzung übernommen."),
        (HostKeyChanged, "Der Fingerabdruck von {n} hat sich geändert (jetzt {m}). Verbindung abgelehnt. Wurde der Host neu aufgesetzt, seine Zeile in {p} löschen."),
        (KeyFileDamaged, "Die Schlüsseldatei {p} ist beschädigt und bleibt, wie sie ist. Datei prüfen oder löschen – dann entsteht ein neuer Schlüssel, und der Host muss dieses Gerät neu koppeln."),
        (FileUnreadable, "{p} lässt sich nicht lesen – keine Verbindung."),
        (FileNotUtf8, "{p} ist nicht als UTF-8 gespeichert – keine Verbindung."),
        (FileNotWritable, "{p} lässt sich nicht schreiben – keine Verbindung."),
        (StorageUnavailable, "Der Ordner für Schlüssel und Einstellungen ist nicht verfügbar – keine Verbindung."),
        (ErrorAddress, "Die Adresse {n} lässt sich nicht auflösen"),
        (ErrorNoConnection, "Keine Verbindung zum Host"),
        (ErrorHandshake, "Die gesicherte Verbindung kam nicht zustande"),
        (ErrorFfmpegStart, "FFmpeg ließ sich nicht starten"),
        (ErrorSound, "Ton nicht verfügbar"),
        (ErrorGpuDisplay, "Grafikkarte nicht nutzbar, Anzeige über Software"),
        (ErrorGpuLost, "Grafikkarte verloren – bitte mit --anzeige cpu neu starten"),
        (ErrorPixelFormat, "Der Decoder liefert Bilder, die dieser Client nicht darstellen kann"),
        (DecoderFallback, "Die gewählte Grafikkarte decodiert nicht – Software-Decoder läuft"),
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
        (DesktopShortcut, "Verknüpfung"),
        (TipDesktopShortcut, "Legt auf dem Desktop eine Verknüpfung zu diesem Host an. Ein Doppelklick darauf verbindet sofort."),
        (DesktopShortcutCreated, "Verknüpfung auf dem Desktop angelegt: {n}"),
        (DesktopShortcutFailed, "Verknüpfung nicht angelegt: {n}"),
        (DesktopShortcutDescription, "QuadChroma: mit {n} verbinden ({m})"),
        (FilesSending, "Dateien werden gesendet: {n} von {m}"),
        (FilesReceiving, "Dateien werden empfangen: {n} von {m}"),
        (FilesReady, "Dateien bereit zum Einfügen: {n}"),
        (FilesSent, "Dateien übertragen: {n}"),
        (FilesAborted, "Dateiübertragung abgebrochen: {n}"),
        (FilesTooLarge, "Dateien nicht übertragen: mehr als 4 GB oder 10 000 Einträge"),
        (FilesPeerOld, "Die Gegenseite kann noch keine Dateien empfangen."),
        (TrayOpen, "Öffnen"),
        (TrayConnect, "Verbinden: {n}"),
        (TrayQuit, "Beenden"),
        (TrayStillRunning, "QuadChroma läuft im Infobereich weiter."),
        (TrayStillRunningMac, "QuadChroma läuft in der Menüleiste weiter."),
        (FilesAbortLocal, "von diesem Gerät"),
        (FilesAbortPeer, "von der Gegenseite"),
        (FilesAbortConnection, "Verbindung unterbrochen"),
        (FilesAbortTimeout, "Zeitüberschreitung, kein Fortschritt"),
        (FilesAbortNoSpace, "zu wenig Speicherplatz"),
        (FilesAbortInvalid, "ungültige Daten"),
        (FilesAbortRead, "Lesefehler"),
        (FilesAbortWrite, "Schreibfehler"),
        (FilesAbortClipboard, "Zwischenablage nicht verfügbar"),
        (ScreenLabel, "Bildschirm"),
        (ScreenAuto, "Automatisch"),
        (TipScreen, "Welchen Bildschirm des Hosts du siehst. Automatisch folgt dem Hauptbildschirm; ein fester Bildschirm gilt, bis du etwas anderes wählst."),
        (ScreenSwitching, "Bildschirm wird gewechselt …"),
        (ScreenFallback, "{n} nicht angeschlossen – Ausweichplatz: {m}"),
        // Zugang (Spezifikation Pairing v1, Abschnitt 14)
        (AccessTitle, "Passwort für {n}"),
        (AccessText, "{n} kennt dieses Gerät noch nicht. Gib das Passwort ein, das im QuadChroma-Menü auf {n} steht."),
        (AccessOrAllow, "Oder bitte jemanden an {n}, auf „Zulassen“ zu klicken."),
        (AccessCode, "Code: {c}"),
        (AccessPassword, "Passwort"),
        (AccessShow, "Anzeigen"),
        (AccessCancel, "Abbrechen"),
        (AccessChecking, "Wird geprüft …"),
        (AccessWrong, "Falsches Passwort."),
        (AccessWait, "Zu viele Versuche. Erneut in {s} s."),
        (AccessNewIdentity, "Das Gerät unter dieser Adresse hat eine neue Identität (ID {i})."),
        (MsgRefused, "{n} hat die Verbindung abgelehnt."),
        (MsgNoAnswer, "Keine Antwort von {n}. Bitte erneut versuchen."),
        (MsgTooManyAttempts, "Zu viele Versuche. Bitte kurz warten und erneut versuchen."),
        (MsgHostOutdated, "{n} verwendet eine ältere QuadChroma-Version. Bitte dort aktualisieren."),
        (MsgHostProofBad, "{n} konnte das Passwort nicht bestätigen. Die Verbindung wurde sicherheitshalber beendet."),
        (MsgOtherDevice, "An dieser Adresse antwortet ein anderes Gerät."),
        (MsgIdNotFound, "Kein Gerät mit der ID {i} im Netz gefunden."),
        (StartId, "ID {i}"),
        (StartShare, "Diesen PC freigeben"),
        (StartSharing, "Freigabe läuft"),
        (HostReady, "Bereit für Verbindungen"),
        (HostConnected, "Verbunden: {n}"),
        (HostDeviceId, "Geräte-ID: {i}"),
        (HostPassword, "Passwort: {p}"),
        (HostCopied, "Kopiert"),
        (HostChangePassword, "Passwort ändern …"),
        (HostRandomPassword, "Neues Zufallspasswort"),
        (HostNewPassword, "Neues Passwort (mindestens 8 Zeichen)"),
        (HostRepeatPassword, "Passwort wiederholen"),
        (HostPasswordsDiffer, "Die Passwörter stimmen nicht überein."),
        (HostPasswordShort, "Bitte mindestens 8 Zeichen."),
        (HostPasswordSaved, "Passwort gespeichert."),
        (HostPasswordUnreadable, "Passwortdatei unlesbar – neue Geräte nur über „Zulassen“."),
        (HostDevices, "Erlaubte Geräte"),
        (HostNoDevices, "Noch keine Geräte"),
        (HostDeviceLine, "{n} – ID {i} – seit {d}"),
        (HostRemove, "Entfernen"),
        (HostRemoveAll, "Alle Geräte entfernen …"),
        (HostRemoveAllAsk, "Alle erlaubten Geräte entfernen? Sie brauchen dann wieder das Passwort."),
        (HostListDamaged, "Geräteliste beschädigt"),
        (HostListReset, "Geräteliste zurücksetzen"),
        (HostRequest, "{n} (ID {i}) möchte diesen Computer steuern."),
        (HostAllow, "Zulassen"),
        (HostDeny, "Ablehnen"),
        (HostStartLogin, "Beim Anmelden starten"),
        (HostStartWindows, "Mit Windows starten"),
        (HostMoveToApps, "QuadChroma zuerst in den Ordner Programme bewegen"),
        (HostScreenMissing, "Bildschirmaufnahme nicht erlaubt – Systemeinstellungen öffnen …"),
        (HostAccessMissing, "Bedienungshilfen nicht erlaubt – Systemeinstellungen öffnen …"),
        (HostPortBusy, "Port {p} ist von einem anderen Programm belegt"),
        (HostQuit, "QuadChroma beenden"),
        (HostStopSharing, "Freigabe beenden"),
        (HostTooltip, "QuadChroma – Freigabe läuft (ID {i})"),
        (HostOk, "OK"),
        (AccessHide, "Verbergen"),
        (MsgShareFailed, "Die Freigabe dieses PCs konnte nicht gestartet werden."),
        (MsgDeviceRemoved, "{n} kennt dieses Gerät nicht mehr. Verbinde erneut und gib das Passwort ein."),
        (HostPasswordNotSaved, "Das Passwort konnte nicht gespeichert werden."),
        (HostPasswordInvalid, "Das Passwort enthält unzulässige Zeichen."),
        (HostPasswordTitle, "Passwort ändern"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Jede der 29 Tabellen fuehrt jeden Schluessel genau einmal, in der
    /// Reihenfolge des Enums (der i-te Eintrag ist der i-te Schluessel), und
    /// alle gleich viele - bisher nur Konvention, jetzt geprueft.
    #[test]
    fn alle_tabellen_in_enum_reihenfolge() {
        let n = EN.table.len();
        assert_eq!(all().len(), 29);
        for l in all() {
            assert_eq!(l.table.len(), n, "{}: Zahl der Eintraege", l.code);
            for (i, (k, t)) in l.table.iter().enumerate() {
                assert_eq!(*k as usize, i, "{}: Eintrag {i} ist {k:?}", l.code);
                assert!(!t.is_empty(), "{}: {k:?} leer", l.code);
            }
        }
        // Der letzte Schluessel des Enums steht auch in der Tabelle.
        assert!(n > HostPasswordTitle as usize);
        assert_eq!(n, HostPasswordTitle as usize + 1, "Tabellen laenger als das Enum");
    }

    /// Die Abbruchgruende der Datei-Zeile: in jeder Sprache vorhanden, ohne
    /// Platzhalter, je Sprache verschieden voneinander, und eigene Worte je
    /// Sprache (EN/DE verschieden).
    #[test]
    fn abbruchgruende() {
        let gruende = [
            FilesAbortLocal,
            FilesAbortPeer,
            FilesAbortConnection,
            FilesAbortTimeout,
            FilesAbortNoSpace,
            FilesAbortInvalid,
            FilesAbortRead,
            FilesAbortWrite,
            FilesAbortClipboard,
        ];
        for l in all() {
            let mut texte = std::collections::HashSet::new();
            for k in gruende {
                let t = l.get(k);
                assert!(!t.is_empty() && !t.contains('{'), "{} {k:?}: {t}", l.code);
                assert!(texte.insert(t), "{} {k:?}: doppelt ({t})", l.code);
            }
        }
        for k in gruende {
            assert_ne!(EN.get(k), DE.get(k), "{k:?}");
        }
        assert_eq!(DE.get(FilesAbortNoSpace), "zu wenig Speicherplatz");
        assert_eq!(EN.get(FilesAbortTimeout), "timeout, no progress");
    }

    /// Die Zeile zu Dateiuebertragungen: Platzhalter in jeder Sprache
    /// vorhanden, wo sie gebraucht werden, sonst keiner; eigene Worte je
    /// Sprache (EN/DE verschieden).
    #[test]
    fn datei_texte() {
        for l in all() {
            for k in [FilesSending, FilesReceiving] {
                let t = l.get(k);
                assert!(t.contains("{n}") && t.contains("{m}"), "{} {k:?}: {t}", l.code);
            }
            for k in [FilesReady, FilesSent, FilesAborted] {
                let t = l.get(k);
                assert!(t.contains("{n}") && !t.contains("{m}"), "{} {k:?}: {t}", l.code);
            }
            for k in [FilesTooLarge, FilesPeerOld] {
                assert!(!l.get(k).contains('{'), "{} {k:?}", l.code);
            }
            assert!(l.get(FilesTooLarge).contains('4'), "{}", l.code);
        }
        for k in [FilesSending, FilesReceiving, FilesReady, FilesSent, FilesAborted, FilesTooLarge, FilesPeerOld] {
            assert_ne!(EN.get(k), DE.get(k), "{k:?}");
        }
    }

    /// Die Texte am Symbol im Infobereich bzw. in der Menueleiste: in jeder
    /// Sprache vorhanden, der Hostname hat seinen Platzhalter, Windows und
    /// Mac nennen verschiedene Orte.
    #[test]
    fn tray_texte() {
        for l in all() {
            assert!(l.get(TrayConnect).contains("{n}"), "{}", l.code);
            for k in [TrayOpen, TrayQuit, TrayStillRunning, TrayStillRunningMac] {
                assert!(!l.get(k).contains('{'), "{}: {k:?}", l.code);
            }
            assert_ne!(l.get(TrayStillRunning), l.get(TrayStillRunningMac), "{}", l.code);
            // Beenden heisst am Symbol wie auf dem Startbildschirm.
            assert_eq!(l.get(TrayQuit), l.get(Quit), "{}", l.code);
        }
        for k in [TrayOpen, TrayConnect, TrayQuit, TrayStillRunning, TrayStillRunningMac] {
            assert_ne!(EN.get(k), DE.get(k), "{k:?}");
        }
        assert_eq!(DE.get(TrayStillRunning), "QuadChroma läuft im Infobereich weiter.");
        assert_eq!(DE.get(TrayStillRunningMac), "QuadChroma läuft in der Menüleiste weiter.");
        assert_eq!(DE.get(TrayConnect), "Verbinden: {n}");
    }

    /// Die Texte der Bildschirmwahl (Spezifikation Bildschirmwahl 3.1): in
    /// jeder Sprache vorhanden, der Ausweichplatz mit beiden Platzhaltern,
    /// die anderen ohne; die Zeile heisst nie wie der Reiter "Anzeige", auf
    /// dem sie steht; EN/DE verschieden; die deutschen Texte wie vorgegeben.
    #[test]
    fn bildschirm_texte() {
        for l in all() {
            let f = l.get(ScreenFallback);
            assert!(f.contains("{n}") && f.contains("{m}"), "{}: {f}", l.code);
            for k in [ScreenLabel, ScreenAuto, TipScreen, ScreenSwitching] {
                assert!(!l.get(k).contains('{'), "{}: {k:?}", l.code);
            }
            assert_ne!(l.get(ScreenLabel).to_lowercase(), l.get(TabDisplay).to_lowercase(), "{}", l.code);
            // Der Hinweis "wird gewechselt" endet wie beim Codec mit "…".
            assert!(l.get(ScreenSwitching).ends_with('…'), "{}: {}", l.code, l.get(ScreenSwitching));
        }
        for k in [ScreenLabel, ScreenAuto, TipScreen, ScreenSwitching, ScreenFallback] {
            assert_ne!(EN.get(k), DE.get(k), "{k:?}");
        }
        assert_eq!(DE.get(ScreenLabel), "Bildschirm");
        assert_eq!(DE.get(ScreenAuto), "Automatisch");
        assert_eq!(
            DE.get(TipScreen),
            "Welchen Bildschirm des Hosts du siehst. Automatisch folgt dem Hauptbildschirm; ein fester Bildschirm gilt, bis du etwas anderes wählst."
        );
        assert_eq!(DE.get(ScreenSwitching), "Bildschirm wird gewechselt …");
        assert_eq!(DE.get(ScreenFallback), "{n} nicht angeschlossen – Ausweichplatz: {m}");
        // Die fuenf Schluessel stehen am Ende des Enums, hinter dem letzten
        // Abbruchgrund - so bleiben alle 29 Tabellen davor unveraendert.
        assert_eq!(ScreenLabel as usize, FilesAbortClipboard as usize + 1);
        assert_eq!(ScreenFallback as usize, ScreenLabel as usize + 4);
    }

    /// Die Texte der Desktop-Verknuepfung: in jeder Sprache eigene Worte
    /// (ausser dort, wo das Wort gleich bleibt), Platzhalter vorhanden.
    #[test]
    fn verknuepfung_texte() {
        for l in all() {
            assert!(l.get(DesktopShortcutCreated).contains("{n}"), "{}", l.code);
            assert!(l.get(DesktopShortcutFailed).contains("{n}"), "{}", l.code);
            let d = l.get(DesktopShortcutDescription);
            assert!(d.contains("{n}") && d.contains("{m}"), "{}", l.code);
            assert!(!l.get(TipDesktopShortcut).contains('{'), "{}", l.code);
        }
        for k in [DesktopShortcut, TipDesktopShortcut, DesktopShortcutCreated, DesktopShortcutFailed, DesktopShortcutDescription] {
            assert_ne!(EN.get(k), DE.get(k), "{k:?}");
        }
        // Nicht mit den Tastenkombinationen verwechseln: im Reiter
        // Verschluesselung stehen beide Woerter auf einem Bild. Keines darf
        // das andere sein oder mit ihm beginnen ("Shortcut"/"Shortcuts").
        for l in all() {
            let (k, t) = (l.get(DesktopShortcut).to_lowercase(), l.get(TabShortcuts).to_lowercase());
            assert!(!k.starts_with(&t) && !t.starts_with(&k), "{}: {k} / {t}", l.code);
        }
    }

    /// Die Zugangstexte (Spezifikation Pairing v1, Abschnitt 14, und die bei
    /// der Zusammenfuehrung nachgetragenen): am Ende des Enums hinter der
    /// Bildschirmwahl, in jeder Tabelle, mit genau den Platzhaltern, die der
    /// Aufrufer ersetzt, und keinem weiteren. EN und DE wie vorgegeben, DE
    /// mit Umlauten und typografischen Zeichen. Die 27 weiteren Tabellen
    /// uebersetzt, in der Schreibweise ihrer Tabelle.
    #[test]
    fn zugang_texte() {
        let n: &[&str] = &["{n}"];
        let i: &[&str] = &["{i}"];
        let keiner: &[&str] = &[];
        let alle: [(Key, &[&str]); 61] = [
            (AccessTitle, n),
            (AccessText, n),
            (AccessOrAllow, n),
            (AccessCode, &["{c}"]),
            (AccessPassword, keiner),
            (AccessShow, keiner),
            (AccessCancel, keiner),
            (AccessChecking, keiner),
            (AccessWrong, keiner),
            (AccessWait, &["{s}"]),
            (AccessNewIdentity, i),
            (MsgRefused, n),
            (MsgNoAnswer, n),
            (MsgTooManyAttempts, keiner),
            (MsgHostOutdated, n),
            (MsgHostProofBad, n),
            (MsgOtherDevice, keiner),
            (MsgIdNotFound, i),
            (StartId, i),
            (StartShare, keiner),
            (StartSharing, keiner),
            (HostReady, keiner),
            (HostConnected, n),
            (HostDeviceId, i),
            (HostPassword, &["{p}"]),
            (HostCopied, keiner),
            (HostChangePassword, keiner),
            (HostRandomPassword, keiner),
            (HostNewPassword, keiner),
            (HostRepeatPassword, keiner),
            (HostPasswordsDiffer, keiner),
            (HostPasswordShort, keiner),
            (HostPasswordSaved, keiner),
            (HostPasswordUnreadable, keiner),
            (HostDevices, keiner),
            (HostNoDevices, keiner),
            (HostDeviceLine, &["{n}", "{i}", "{d}"]),
            (HostRemove, keiner),
            (HostRemoveAll, keiner),
            (HostRemoveAllAsk, keiner),
            (HostListDamaged, keiner),
            (HostListReset, keiner),
            (HostRequest, &["{n}", "{i}"]),
            (HostAllow, keiner),
            (HostDeny, keiner),
            (HostStartLogin, keiner),
            (HostStartWindows, keiner),
            (HostMoveToApps, keiner),
            (HostScreenMissing, keiner),
            (HostAccessMissing, keiner),
            (HostPortBusy, &["{p}"]),
            (HostQuit, keiner),
            (HostStopSharing, keiner),
            (HostTooltip, i),
            (HostOk, keiner),
            (AccessHide, keiner),
            (MsgShareFailed, keiner),
            (MsgDeviceRemoved, n),
            (HostPasswordNotSaved, keiner),
            (HostPasswordInvalid, keiner),
            (HostPasswordTitle, keiner),
        ];
        // Lueckenlos am Ende: so bleiben alle 29 Tabellen davor unveraendert.
        assert_eq!(AccessTitle as usize, ScreenFallback as usize + 1);
        for (nr, (k, _)) in alle.iter().enumerate() {
            assert_eq!(*k as usize, AccessTitle as usize + nr, "{k:?}");
        }
        assert_eq!(HostPasswordTitle as usize + 1, EN.table.len());
        let bekannt = ["{n}", "{m}", "{i}", "{c}", "{s}", "{p}", "{d}"];
        for l in all() {
            for (k, soll) in &alle {
                let t = l.get(*k);
                assert!(!t.is_empty(), "{} {k:?}", l.code);
                for p in bekannt {
                    assert_eq!(t.contains(p), soll.contains(&p), "{} {k:?}: {p} in {t}", l.code);
                }
                // Keine Klammer ausser den erwarteten Platzhaltern.
                let erwartet: usize = soll.iter().map(|p| t.matches(p).count()).sum();
                assert_eq!(t.matches('{').count(), erwartet, "{} {k:?}: {t}", l.code);
                assert_eq!(t.matches('}').count(), erwartet, "{} {k:?}: {t}", l.code);
            }
        }
        // Eigene Worte je Sprache - ausser Code, ID und OK, die im Deutschen
        // genauso heissen.
        let gleich = [AccessCode, StartId, HostOk];
        for (k, _) in &alle {
            if gleich.contains(k) {
                assert_eq!(EN.get(*k), DE.get(*k), "{k:?}");
            } else {
                assert_ne!(EN.get(*k), DE.get(*k), "{k:?}");
            }
        }
        // Deutsch typografisch wie der Bestand: keine geraden Anfuehrungs-
        // zeichen, kein " - " statt Gedankenstrich, kein "..." statt "…",
        // keine Umschreibung von Umlauten.
        for (k, _) in &alle {
            let t = DE.get(*k);
            assert!(!t.contains('"') && !t.contains(" - ") && !t.contains("..."), "{k:?}: {t}");
            let umschrieben = [
                "Geraet",
                "fuer",
                "Menue",
                "geprueft",
                "aender",
                "oeffnen",
                "ueber",
                "laeuft",
                "moechte",
                "schaedigt",
                "zurueck",
                "aeltere",
                "staetigen",
                "Passwoerter",
                "Identitaet",
            ];
            for w in umschrieben {
                assert!(!t.contains(w), "{k:?}: {t}");
            }
        }
        assert_eq!(EN.get(AccessOrAllow), "Or ask someone at {n} to click \"Allow\".");
        assert_eq!(DE.get(AccessOrAllow), "Oder bitte jemanden an {n}, auf „Zulassen“ zu klicken.");
        assert_eq!(
            DE.get(AccessText),
            "{n} kennt dieses Gerät noch nicht. Gib das Passwort ein, das im QuadChroma-Menü auf {n} steht."
        );
        assert_eq!(DE.get(HostDeviceLine), "{n} – ID {i} – seit {d}");
        assert_eq!(DE.get(HostPasswordUnreadable), "Passwortdatei unlesbar – neue Geräte nur über „Zulassen“.");
        assert_eq!(DE.get(StartShare), "Diesen PC freigeben");
        assert_eq!(EN.get(HostTooltip), "QuadChroma - sharing this PC (ID {i})");
        assert_eq!(EN.get(AccessHide), "Hide");
        assert_eq!(DE.get(AccessHide), "Verbergen");
        assert_eq!(EN.get(MsgShareFailed), "Could not start sharing this PC.");
        assert_eq!(DE.get(MsgShareFailed), "Die Freigabe dieses PCs konnte nicht gestartet werden.");
        assert_eq!(EN.get(MsgDeviceRemoved), "{n} no longer knows this device. Connect again and enter the password.");
        assert_eq!(DE.get(MsgDeviceRemoved), "{n} kennt dieses Gerät nicht mehr. Verbinde erneut und gib das Passwort ein.");
        assert_eq!(EN.get(HostPasswordNotSaved), "The password could not be saved.");
        assert_eq!(DE.get(HostPasswordNotSaved), "Das Passwort konnte nicht gespeichert werden.");
        assert_eq!(EN.get(HostPasswordInvalid), "The password contains characters that are not allowed.");
        assert_eq!(DE.get(HostPasswordInvalid), "Das Passwort enthält unzulässige Zeichen.");
        assert_eq!(EN.get(HostPasswordTitle), "Change password");
        assert_eq!(DE.get(HostPasswordTitle), "Passwort ändern");
        // Die uebrigen 27 Sprachen sind uebersetzt (Paket P6): gleich wie
        // Englisch nur Code, ID und OK - und die Woerter, die eine Sprache
        // aus dem Englischen uebernimmt (italienisch "Password",
        // niederlaendisch "Code").
        let lehnwort = [("it", AccessPassword), ("it", HostPassword), ("nl", AccessCode)];
        for l in all().iter().skip(2) {
            for (k, _) in &alle {
                if l.get(*k) == EN.get(*k) {
                    assert!(gleich.contains(k) || lehnwort.contains(&(l.code, *k)), "{} {k:?}: noch englisch", l.code);
                }
            }
        }
        // Jede Sprache wie ihre Tabelle: "…" statt "...", Gedankenstrich
        // statt " - " (nur Englisch steht wie vorgegeben); genau die
        // Menuepunkte, die ein Fenster oder eine Folge oeffnen, und "wird
        // geprueft" enden auf " …". Das Wort fuer "Zulassen" steht im Hinweis
        // genauso wie auf dem Knopf, Abbrechen heisst wie im Benchmark,
        // Beenden wie auf dem Startbildschirm. Kein Text doppelt je Sprache.
        let punkte = [AccessChecking, HostChangePassword, HostRemoveAll, HostScreenMissing, HostAccessMissing];
        for l in all() {
            let mut texte = std::collections::HashSet::new();
            for (k, _) in &alle {
                let t = l.get(*k);
                assert!(texte.insert(t), "{} {k:?}: doppelt ({t})", l.code);
                if l.code != "en" {
                    assert!(!t.contains("...") && !t.contains(" - "), "{} {k:?}: {t}", l.code);
                    assert_eq!(t.ends_with(" …"), punkte.contains(k), "{} {k:?}: {t}", l.code);
                }
            }
            for k in [AccessOrAllow, HostPasswordUnreadable] {
                assert!(l.get(k).contains(l.get(HostAllow)), "{} {k:?}: {}", l.code, l.get(k));
            }
            assert_eq!(l.get(AccessCancel), l.get(BenchAbort), "{}", l.code);
            let beenden = l.get(Quit).to_lowercase();
            assert!(l.get(HostQuit).to_lowercase().contains(&beenden), "{}: {}", l.code, l.get(HostQuit));
        }
    }

    /// Dieselben Worte auf beiden Hosts: jeder Zugangstext, den auch der
    /// Mac-Host fuehrt (host/texte.m, Schluessel QCText<Name>), steht dort in
    /// allen 29 Sprachen wortgleich wie hier. Fehlt die Datei (Bau nur aus
    /// client/, etwa auf der Windows-VM), gibt es nichts zu vergleichen.
    #[test]
    fn zugang_texte_wie_mac_host() {
        let pfad = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../host/texte.m");
        let Ok(quelle) = std::fs::read_to_string(&pfad) else {
            eprintln!("{} fehlt - Vergleich mit dem Mac-Host uebersprungen", pfad.display());
            return;
        };
        let zugang = &EN.table[AccessTitle as usize..];
        let mut je_sprache = Vec::new();
        for block in quelle.split("static const qc_sprache QC_").skip(1) {
            // Kopf: XX = { "xx", "Name", {
            let code = block.split('"').nth(1).expect("Sprachcode");
            let l = all().iter().find(|l| l.code == code).unwrap_or_else(|| panic!("{code}: keine Tabelle im Client"));
            let ende = block.find("}};").expect("Tabellenende");
            let mut verglichen = 0;
            for zeile in block[..ende].lines() {
                let Some(rest) = zeile.trim().strip_prefix("[QCText") else { continue };
                let (name, wert) = rest.split_once(']').expect("Schluessel");
                // Schluessel, die es nur in einer der beiden Dateien gibt,
                // haben hier kein Gegenstueck.
                let Some((k, _)) = zugang.iter().find(|(k, _)| format!("{k:?}") == name) else { continue };
                let wert = wert.trim().strip_prefix('=').and_then(|w| w.trim().strip_suffix(','));
                let wert = wert.and_then(|w| w.strip_prefix('"')?.strip_suffix('"')).expect("Zeichenkette");
                assert_eq!(wert.replace("\\\"", "\""), l.get(*k), "{code} {k:?}");
                verglichen += 1;
            }
            je_sprache.push(verglichen);
        }
        assert_eq!(je_sprache.len(), 29);
        // Alle 34 Host-Texte, Code und Abbrechen und die drei Texte des
        // Passwortfensters (nicht gespeichert, unzulaessig, Titel), in jeder
        // Sprache.
        assert!(je_sprache.iter().all(|&n| n == 39), "{je_sprache:?}");
    }
}
