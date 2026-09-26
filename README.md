# QuadChroma

Fernsteuerung mit voller Farbauflösung. Der Mac nimmt seinen Bildschirm auf, codiert
ihn in Hardware als HEVC 4:4:4 mit 10 Bit und schickt ihn an einen Windows-Rechner.
Der Mauszeiger wird bewusst **nicht** ins Bild gerendert: Sichtbar ist der Zeiger von
Windows, der Mac zieht unsichtbar mit. Deshalb fühlt sich die Maus lokal an — und
damit er trotzdem aussieht wie auf dem Mac, schickt der Host nur die *Form* des
Zeigers (Pfeil, Hand, Ziehpfeile, Textcursor, Wartekugel), sobald sie sich ändert.

Stand: 26.09.2026. Läuft, ist aber noch ein Gerüst, keine fertige Anwendung.

## Warum 4:4:4

Übliche Fernsteuerungen übertragen Farbe nur in halber Breite und Höhe. Bei 1080p
landet die Farbe damit in 960×540, und Rot franst sichtbar aus. QuadChroma überträgt
für jeden Bildpunkt einen eigenen Farbwert. Apples Media Engine kann das in Hardware,
FFmpeg spricht diese Profile aber nicht an, deshalb steuert der Host den Encoder
direkt an.

## Starten

Auf dem Mac:

    cd ~/Documents/quadchroma
    make
    open -n build/QuadChroma.app --args --serve 9001 --fps 120 --mbit 50 --fest

Beim ersten Start zwei Freigaben erteilen, jeweils in den Systemeinstellungen unter
Datenschutz & Sicherheit:

- **Bildschirmaufnahme** für das Bild
- **Bedienungshilfen** für Maus und Tastatur

Ohne Zuschauer tut der Host nichts: keine Aufnahme, kein Encoder (0,6 % Last im
Leerlauf). Aufgenommen wird der Hauptbildschirm; einen anderen wählt der Client im
Menü, und `--display n` pinnt für diesen Lauf den Listenplatz n (siehe „Bildschirm
des Hosts“). `--fest` schickt Bilder im festen Takt von `--fps`, auch bei stillem
Bildschirm; ohne die Angabe kommen neue Bilder nur bei Änderungen (umschaltbar im
Menü des Clients, dort je Host gespeichert).
Sein Protokoll steht in `/tmp/quadchroma-m1.log`; über 8 MB wandert es nach
`/tmp/quadchroma-m1.alt.log` und beginnt neu.

Auf Windows einfach starten:

    quadchroma.exe

Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, erscheint also von selbst
im Startbildschirm. Drei Kanäle: 9001 Bild und Ton (und alles vom Host zum Client),
9002 Eingaben und Zwischenablage samt Dateien (alles vom Client zum Host), 9003 die
Bekanntgabe. Was der Client entscheidet (Decoder, Anzeige, Zeigerform) und was
FFmpeg dazu sagt, steht in `%APPDATA%\QuadChroma\protokoll.txt`; die Bedienung im
Einzelnen beschreibt `BENUTZUNG.txt`.

### Windows-Client bauen

Rust (MSVC), LLVM für bindgen und die FFmpeg-Bibliotheken aus
`scripts/build-ffmpeg-windows.sh`: ein schlanker LGPL-Bau von FFmpeg 9.0.2 ohne jede
Fremdbibliothek, per MinGW-w64 auf dem Mac (Homebrew: `mingw-w64`, `nasm`, `pkgconf`)
oder unter Linux übersetzt. Das Skript lädt die FFmpeg-Quellen und die NVIDIA-Header,
prüft ihre SHA-256 und legt DLLs, Header und Importbibliotheken in den angegebenen
Ordner:

    scripts/build-ffmpeg-windows.sh ffmpeg-windows

Den Ordner auf den Windows-Rechner kopieren, dann:

    set FFMPEG_DIR=C:\pfad\zu\ffmpeg-windows
    set LIBCLANG_PATH=C:\Program Files\LLVM\bin
    cd client
    cargo build --release

Gebaut wird aus `client\`: nur dort greift `client\.cargo\config.toml`, das die
C-Laufzeit statisch in die exe bindet — sonst bräuchte sie `VCRUNTIME140.dll`
aus dem Visual C++ Redistributable. Eine gesetzte Umgebungsvariable `RUSTFLAGS`
ersetzt diese Einstellung. Fehlt so die statische Laufzeit, bricht der Bau mit
einem Hinweis ab.
Neben die exe gehören `avcodec-63.dll` und `avutil-61.dll` (aus `ffmpeg-windows\bin`);
Voraussetzung ist Windows 10 oder 11 (64 Bit), eine Visual-C++-Laufzeit braucht es
nicht. Fertige FFmpeg-Builds aus dem Netz taugen nicht für die Weitergabe: die meisten
binden Fremdbibliotheken ein, manche davon unter der GPL.

### Mac als Client (im Aufbau)

Derselbe Client baut auch auf dem Mac (arm64), mit Ton über AudioToolbox und
Zwischenablage über NSPasteboard (Text und Dateien); die Anzeige läuft dort noch
über die CPU (softbuffer), Metal kommt später. Schließen legt ihn in die
Menüleiste. Bauen mit Rust und dem FFmpeg aus Homebrew:

    cd client
    FFMPEG_DIR=/opt/homebrew/opt/ffmpeg cargo build --release
    ./target/release/quadchroma 192.168.178.194:9001

Ablage unter `~/Library/Application Support/QuadChroma` (client.key, known_hosts.txt,
einstellungen.txt, protokoll.txt, benchmark.txt, einzel.sock, einzel.lock — neben
host.key und authorized.txt des Hosts, getrennte Dateien). Lizenzhinweis: das
Homebrew-FFmpeg ist ein GPL-Build (libx264, libx265); für den Eigengebrauch in
Ordnung, zur Weitergabe des Mac-Clients braucht es einen LGPL-Build ohne GPL-Teile.

## Erster Start einer heruntergeladenen Version

QuadChroma ist nicht mit einem bezahlten Zertifikat signiert (Apple Developer Program,
Windows-Codesignatur). Windows und macOS fragen deshalb vor dem ersten Start einmal
nach. Nur von der Release-Seite laden und die SHA-256 mit `SHA256SUMS.txt` vergleichen.

- **Windows:** vor dem Entpacken Rechtsklick auf das ZIP → Eigenschaften → „Zulassen“
  anhaken → OK; dann warnt Windows gar nicht. Sonst meldet SmartScreen „Der Computer
  wurde durch Windows geschützt“: „Weitere Informationen“ → „Trotzdem ausführen“.
  Mit eingeschalteter intelligenter App-Steuerung (Windows 11) kann ein unsigniertes
  Programm ganz gesperrt sein; dann hilft nur der Bau aus dem Quelltext.
- **macOS:** `QuadChroma.app` aus der DMG in „Programme“ ziehen. Den ersten Start lehnt
  macOS ab (nicht von Apple notarisiert): Systemeinstellungen → Datenschutz & Sicherheit
  → ganz unten „Dennoch öffnen“, bestätigen. Im Terminal geht dasselbe mit
  `xattr -dr com.apple.quarantine /Applications/QuadChroma.app`. Danach
  Bildschirmaufnahme und Bedienungshilfen erlauben. Versionen mit `-selfsigned` im
  Namen sind mit dem eigenen, kostenlosen Zertifikat „QuadChroma Release“ signiert –
  die Freigaben bleiben über Updates erhalten. Bei `-unsigned` (nur ad hoc signiert)
  sind sie nach jedem Update neu zu erteilen.

## Bedienung im Client

| Taste | Wirkung |
|---|---|
| F9 | Statistik ein und aus |
| F10 oder ESC zwei Sekunden halten | Menü: Bild, Anzeige, Verschlüsselung, Tastenkombinationen, Benchmark |
| F11 | Vollbild an und aus |
| F12 | pixelgenaue Darstellung statt gestreckt |
| Strg+Esc | zurück zum Startbildschirm |

Jede Tastenfunktion ist im Menü auch ein Schalter. Alle übrigen Tasten gehen an den
Mac. Übertragen wird die **Position** der Taste, nicht das Zeichen, deshalb
funktionieren Umlaute und AltGr ohne Zutun. Die Windows-Taste ist die Befehlstaste des
Macs. Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton, Codec und den Bildschirm
des Hosts stellt man im laufenden Betrieb um; der Mac meldet zurück, was gilt, und
der Client merkt sich die Werte je Host (den Bildschirm merkt sich der Host selbst).

Der Reiter **Benchmark** misst Codecs, Bildraten und Datenraten der Reihe nach, je
Schritt einige Sekunden, ohne das Bild anzuhalten: angekommene Bildrate, die ganze
Kette in Millisekunden, verworfene Bilder, Encoderzeit des Hosts gegen sein Budget,
Prozessorlast beider Seiten. Auf Wunsch sendet der Host dabei ein festes, bewegtes
Testbild, damit jeder Schritt denselben Inhalt sieht. Am Ende steht eine Empfehlung,
die sich mit einem Klick übernehmen lässt; die Tabelle liegt in
`%APPDATA%\QuadChroma\benchmark.txt`.

Rechner mit zwei Grafikkarten (Laptop mit Intel-Grafik und NVIDIA-Karte) bekommen im
Reiter **Anzeige** je eine Zeile für die Anzeige und für den Decoder mit den Rollen
Automatik, Grafikkarte, Integriert und Prozessor. Der Client erkennt die Karten beim
Start (gemeinsamer Speicher heißt integriert), zeigt nur Knöpfe, zu denen es eine Karte
gibt, und nennt die erkannte Karte im Tooltip. Der Decoder wechselt sofort, die
Anzeige ab dem nächsten Start.

## Bildschirm des Hosts

Der Host streamt genau einen Bildschirm. Welchen, stellt der Client im Menü ein,
Reiter **Bild**, Zeile „Bildschirm“ über den Codecknöpfen: ein Knopf **Automatisch**
und je Bildschirm des Hosts einer mit Name, Größe und Bildrate („X27 X1 · 1920×1080 ·
120 Hz“, ohne Hz-Teil, wenn der Host die Rate nicht kennt). Der gestreamte Bildschirm
steht in Cyan; bei Automatik dazu „Automatisch“, bei festem Wunsch der gewünschte
Eintrag, sofern er angeschlossen ist. Ein Klick auf den Eintrag, der schon der Wunsch
ist, tut nichts — der gestreamte Eintrag ist bei Automatik nicht der Wunsch, ein Klick
darauf wählt ihn fest. Bis der Host geantwortet hat, steht rechts in Amber „Bildschirm
wird gewechselt …“, bei geschlossenem Menü derselbe Hinweis über dem Bild wie beim
Codecwechsel (höchstens 5 s; läuft gerade beides, zeigt der Kasten den Codecwechsel).
Die Zeile gibt es nur bei einem Host, der die Wahl kennt (Bit 1 seiner Fähigkeiten);
ein älterer Host bekommt nie einen Wunsch, ein älterer Client übergeht die Liste.

- **Automatisch** (Vorgabe): Der Host streamt seinen Hauptbildschirm — auf dem Mac
  den mit der Menüleiste, auf Windows den Monitor, den Windows als primär führt —
  und folgt ihm, wenn er wechselt, ohne Neustart und ohne neuen Zuschauer. Der
  Anlass: Am 26.09. war am Mac mini ein Monitor angeschlossen worden und
  Hauptbildschirm geworden; der Host streamte weiter den beim Start gewählten
  virtuellen Bildschirm, auf dem kein Fenster lag — das Bild war leer, Maus und
  Tastatur schienen tot.
- **Fest**: Der Host streamt den gewählten Bildschirm, erkannt an einer stabilen
  Kennung (Mac `v<Vendor>-m<Model>-s<Serial>`, bei zwei gleichen mit `-u<Unit>`;
  Windows der zweite Teil der Geräte-ID des Monitors wie `ACR0501`, bei zwei
  gleichen mit `-<Listenplatz>`), nie an einem Listenplatz. Fällt er weg, streamt
  der Host den Hauptbildschirm als Ausweichplatz — im Menü steht dann in Amber
  „<Kennung> nicht angeschlossen – Ausweichplatz: <Name>“ —, kommt er zurück,
  wechselt der Host von selbst zurück. Der Wunsch gilt hostweit (ein Strom, der
  letzte Wunsch gewinnt) und bleibt, bis jemand etwas anderes wählt: er steht in
  `bildschirm.txt` im Ablageordner des Hosts, eine Zeile, `auto` oder die Kennung.
  Fehlt sie oder ist sie kaputt, gilt Automatik; eine kaputte Datei wird nie still
  ersetzt.
- `--display N` (Mac) bzw. `--output n` (Windows-Host-Rolle) pinnt für diesen Lauf
  den Bildschirm am Listenplatz N (`--list` zeigt ihn samt Kennung und Name), ohne
  die Datei zu ändern; ein Wunsch aus dem Menü überschreibt den Pin und wird
  gespeichert.

Einen Monitorwechsel meldet macOS dem Mac-Host sofort; er bewertet 300 ms nach der
letzten Meldung neu (entprellt: drei Meldungen kurz nacheinander sind ein Wechsel).
Die Windows-Host-Rolle zählt ihre Ausgänge alle 2 s neu auf, nach einem Verlust der
Duplication sofort. Der Wechsel selbst läuft wie ein Codecwechsel: alter Strom
anhalten, neuer Strom auf dem Zielbildschirm, an den Zuschauer erst Nachricht 7
(SWITCH mit dem laufenden Codec, damit der Client den Decoder neu baut), dann 1
(INFO mit den Maßen), dann die neue Liste (Nachricht 12), dann das erste Bild als
Vollbild; die Maus folgt dem Bild, das Bild steht kurz still. Hat der neue
Bildschirm eine andere Größe, entsteht der Encoder neu; bei gleicher Größe bleibt
er auf dem Mac stehen, auf Windows nur ein Encoder auf dem Prozessorweg (einer auf
dem Texturweg hängt am Gerät der alten Duplication). Der Client baut auch bei einer
INFO mit neuen Maßen seinen Decoder neu und wartet auf das Vollbild. Läuft gerade
ein Codecwechsel, wartet der Bildschirmwechsel, bis er fertig ist (der Mac-Host
nach 5 s trotzdem).

Wird der gestreamte Bildschirm abgezogen, ist das kein Wechsel, sondern ein Verlust:
der Strom endet, der Host meldet „kein Bildschirm“ (Nachricht 9) und baut ihn neu
auf — der Mac nach 2 s, die Windows-Host-Rolle alle 2 s — auf dem dann gewählten
Bildschirm, bei festem Wunsch also dem Ausweichplatz. SWITCH und INFO bekommt der
Zuschauer dabei vom Mac nur bei anderer Stromgröße, von der Windows-Host-Rolle,
sobald es ein anderer Bildschirm ist; der Mac schreibt dazu keine Zeile
`Bildschirmwechsel`, sondern `Aufnahme wiederhergestellt`.

Bei einem Wechsel steht im Protokoll beider Hosts `Bildschirmwechsel: <alt> -> <neu>
(<Grund>)` mit `Wunsch des Zuschauers`, `Wunsch des Zuschauers: Automatik`,
`Hauptbildschirm gewechselt`, `Ausweichplatz` oder `zurueck zum gewuenschten
Bildschirm`. Alle Zeilen, die Nachrichten 12 und 70 im
Einzelnen und der Prüfmodus (`--bildschirm <Kennung|auto>`, Feld „Strom“ in der
Statuszeile) stehen in `BENUTZUNG.txt`.

## Dateien über die Zwischenablage

Dateien und Ordner kopiert man im Explorer bzw. Finder und fügt sie auf der anderen
Seite ein – in beide Richtungen, zwischen dem Client (Windows oder Mac) und jedem
Host (Mac oder Windows-Host-Rolle). Anders als bei RDP beginnt die Übertragung
sofort beim Kopieren, nicht erst beim Einfügen: Die Gegenseite schreibt die Dateien
in ein eigenes Verzeichnis im Temp-Ordner (`QuadChroma-Ablage`, bei den Hosts
`QuadChroma-Host-Ablage`) und legt sie als Dateiliste in ihre Zwischenablage, sobald
alles da ist. Dort bleiben die drei neuesten Übertragungen; beim Start geht, was
älter als 24 h ist oder ein beendeter Lauf halb empfangen liegen ließ. Im Client
zeigt eine schmale Zeile unten im Bild den Fortschritt.

- Grenzen: 4 GB und 10 000 Einträge je Kopie, die Liste der Einträge bis 1 MiB.
- Nachrang vor Bild, Ton und Eingaben: höchstens 256 KiB unquittiert unterwegs (im
  Spielmodus und in der Windows-Host-Rolle 64 KiB), am Eingabekanal wartet höchstens
  ein Datei-Paket hinter den Tasten, die Fäden laufen mit niedrigerer Priorität.
- Neuer Inhalt in der Ablage, Sitzungsende und 30 s ohne Fortschritt brechen ab,
  halb Empfangenes wird gelöscht.
- Der Empfänger prüft Pfade, Längen und Reihenfolge, als wären sie feindlich, und
  bereinigt Namen, die sein System nicht kennt. Verknüpfungen werden weder
  gesendet noch angelegt.
- Ältere Gegenstellen bekommen nichts: Ob die andere Seite Dateien kann, meldet sie
  beim Verbinden (Fähigkeiten, Nachrichten 11 und 69).
- Gemessen im Integrationstest des Endstands (e134fc6):
  - Auf der VM (Software-Encoder, ohne Grafikkarte): Client → Host rund 30 MB/s,
    dabei etwa 2 Bilder/s weniger und rund 60 ms mehr Verzögerung im Encoder.
    Die Ursache ist nicht geklärt, die Prozessorlast ist es nicht.
  - Host → Client auf der VM 69–93 MB/s, ohne messbare Wirkung auf Bild und
    Verzögerung.
  - Im LAN vom Windows-Client zum Mac-Host: 28 MB/s bei unveränderten rund
    114 Bildern/s und 30–37 ms Verzögerung.

Auf dem Mac kann macOS beim ersten Kopieren aus Schreibtisch, Dokumente oder
Downloads nach dem Zugriff fragen, ab macOS 15.4 auch beim Lesen der
Zwischenablage. Anzeige, Protokollzeilen und die Nachrichten 50–53 im Einzelnen
stehen in `BENUTZUNG.txt`.

## Schließen, Einzelinstanz, Desktop-Verknüpfung

Schließen trennt eine laufende Sitzung und legt den Client ab: unter Windows als
Symbol im Infobereich, auf dem Mac in der Menüleiste. Das Symbol holt das Fenster
zurück, verbindet über sein Menü mit gefundenen Hosts oder beendet das Programm;
`tray=aus` in `einstellungen.txt` stellt das alte Verhalten wieder her. Es läuft nur
eine App mit Fenster: Ein zweiter Start reicht seine Adresse an die laufende weiter
und endet.

Darauf baut die Desktop-Verknüpfung (nur Windows): Der Knopf „Verknüpfung“ in jeder
Hostzeile des Startbildschirms oder im Reiter Verschlüsselung legt
`QuadChroma - <Name>.lnk` mit dem Programmsymbol auf den Desktop. Ein Doppelklick
verbindet sofort, auch wenn die App schon im Infobereich liegt. Ohne Fenster:

    quadchroma.exe --verknuepfung <adresse> [--name <name>] [--ordner <verzeichnis>]

Die Verknüpfung trägt den vollen Pfad der exe und die Adresse des Hosts, nicht
seinen Namen: Nach einem Umzug der exe oder einer neuen Adresse legt man sie neu an.

Das Programmsymbol, vier farbige Quadrate, zeichnet der Client selbst: unter Windows
für Titelleiste, Taskleiste, Infobereich und Verknüpfung (die exe trägt keines), auf
dem Mac einfarbig für die Menüleiste.

## Gemessen auf einem Mac mini M1

| | |
|---|---|
| Bild | 1920×1080, 4:4:4, 10 Bit, 120 Bilder/s Ziel, der Host deckelt darauf |
| Datenrate Bild | 3 bis 11 Mbit/s bei ruhigem Bild, bis zur eingestellten Grenze bei Bewegung |
| Encoder | Hardware, rund 8 ms je Bild, wenige Prozent eines Kerns |
| Codecs | HEVC 4:4:4 und 4:2:0 in 8 und 10 Bit, H.264 — alle in Hardware, im Betrieb umschaltbar |
| Decodieren auf Windows | NVDEC in Hardware; auf AMD und Intel D3D11VA (4:2:0 und H.264); Software 8,6 ms je Bild (scheibenparallel, damit kein Bild zurückgehalten wird) |
| Anzeige auf Windows | Direct3D 11: rohe Decoder-Ebenen auf die Karte, Umrechnung und Skalierung im Shader, bitidentisch zum Prozessorweg |
| Verzögerung | 15 bis 20 ms von der Aufnahme bis zur Übergabe an die Anzeige, gemessen mit Zeitstempeln je Bild |
| Ton | unkomprimiert, Stereo, 48 kHz, rund 3 Mbit/s; abschaltbar |

## Oberfläche

Dunkler Grund, feines Raster, Neonlinien in Cyan und Magenta, Ecken als Klammern.
Alles selbst gezeichnet, ohne Fremdbaukasten. Auf dem Prozessorweg liegen Bild und
Oberfläche im selben Puffer; auf der Grafikkarte wird die Oberfläche als eigene Ebene
mit Durchsicht darübergelegt — nur der geänderte Ausschnitt wird hochgeladen, und nur,
wenn etwas zu sehen ist. Die Statistik lässt sich zeilenweise zusammenstellen; der
Nerd-Modus zeigt die Verzögerungskette als Balken auf fester Millisekundenskala mit
Aufnahme, Encoder, Leitung, Decoder und Anzeige.

## Sicherheit

Jede Verbindung ist verschlüsselt und beidseitig ausgewiesen — es gibt keinen
Schalter, der das abstellt. Verwendet wird das Noise-Muster XX mit X25519,
ChaCha20-Poly1305 und SHA-256: beide Seiten weisen sich mit einem dauerhaften
Schlüssel aus, für jede Verbindung entstehen frische Sitzungsschlüssel, und aus
dem Handschlag fällt ein sechsstelliger Vergleichscode, den beide Seiten
anzeigen. Stimmt er überein, sitzt niemand dazwischen.

Der Bildkanal wird zuerst aufgebaut. Der Eingabekanal nimmt die Prüfsumme des
Bildkanals in seinen Handschlag auf — wer sie nicht hat, kommt dort nicht
hinein. Damit kann niemand die Tastatur des Macs übernehmen, ohne vorher den
Bildkanal legitim aufgebaut zu haben. Die Prüfsumme weist nur die Sitzung aus,
nicht den Host; deshalb vergleicht der Client am Eingabekanal zusätzlich den
Schlüssel der Gegenstelle mit dem des Bildkanals, noch im Handschlag und bevor
er sich selbst ausweist, und Tasten, Zwischenablage und Dateien gehen nur an
denselben Host. Der Host bindet den Eingabekanal an genau einen Zuschauer: Löst
ein neuer ihn ab oder reißt sein Bild ab, kappt er ihn und lässt dessen gedrückte
Tasten und Maustasten los.

Freigabe: Solange der Host noch keine Freigabeliste hat, nimmt er die erste
Gegenstelle von selbst auf (der Mac-Host auch bei einer leeren Datei mit 0 Byte),
danach braucht jedes neue Gerät einen Host, der mit `--pair` gestartet wurde. Der
Client merkt sich den Fingerabdruck des Hosts und prüft ihn ebenfalls im
Handschlag, bevor er sich ausweist. Hat er sich geändert, bricht der Client dort
ab: Der Host nimmt ihn gar nicht erst an, ein laufender Zuschauer bleibt
ungestört, und der Client versucht es nicht alle 2 s erneut, sondern wartet, bis
man selbst neu verbindet. Eine Liste oder ein Schlüssel, die vorhanden, aber
unlesbar oder beschädigt sind, gelten nie als erster Start: Dann wird abgewiesen
statt neu gekoppelt, und kein Schlüssel wird still ersetzt (Einzelheiten in
`BENUTZUNG.txt`).

Jeder Handschlag hat eine Gesamtfrist (Host 5 s, Client 3 s) und läuft beim Host
in einem eigenen Faden, höchstens 32 gleichzeitig je Port und nur wenige je
Absender. Wer schweigt oder tröpfelt, hält nur seinen eigenen Platz. Was jeder im
Netz ohne Kopplung auslösen kann — gescheiterte Handschläge, unbekannte
Gegenstellen, Eingabekanäle ohne Bild —, steht gedrosselt im Hostprotokoll:
höchstens alle 10 s eine Zeile je Art und Adresse (Mac-Host 4, Windows-Host 64
Adressen je Art, darüber gemeinsam), mit der Zahl der unterdrückten. Beide Hosts
begrenzen ihr Protokoll auf 8 MB und schieben den älteren Teil in eine `.alt`-Datei.

Ein Zuschauer zur Zeit: Verbindet sich ein anderes gekoppeltes Gerät, übernimmt
es die Sitzung. Das bisherige bekommt vorher Nachricht 10, zeigt "Ein anderes
Gerät hat die Sitzung übernommen." und verbindet sich nicht von selbst neu. Der
neue Zuschauer erbt nichts vom alten: Ein Testbild endet, und er bekommt auch bei
stillem Bildschirm gleich ein erstes Bild.

Auf der Mac-Seite ist das Muster eigens umgesetzt (rund 400 Zeilen C auf
Monocypher), auf der Windows-Seite kommt die bekannte Rust-Umsetzung zum
Einsatz. Beide wurden gegeneinander geprüft: gleicher Vergleichscode, gleiche
Fingerabdrücke, verschlüsselter Austausch in beide Richtungen.

## Was fehlt

- Prüfung auf echter Hardware. Auf der VM und in den Prüfständen belegt, am Gerät
  noch offen: der Windows-Host mit NVIDIA-Karte (NVENC über bgra, yuv444 und d3d11,
  Testbild und Codecwechsel auf d3d11, eine AV1-fähige Karte), bei 125/150 %
  Skalierung, an gedrehten Ausgängen und Handhelds mit hochkantem Panel und mit
  echtem Tongerät; die Stauregel beider Hosts auf einer echten, zu langsamen Leitung
  (auch unter der Tonrate) und im Spielmodus bei hoher Datenrate; am laufenden
  Mac-Host (dort geprüft: Bild, Eingabekanal, Ton und die Umrechnung nach einem
  Codecwechsel mit Formatwechsel, alles mit fester Bildrate) der Codecwechsel bei
  wirklich stillem Bildschirm, der Zuschauerwechsel mit Nachricht 10, das Nachreichen
  bei stillem Bildschirm, Tonformat mit 44,1 kHz, Ablageverwalter, beschädigte
  Freigabeliste; die Bildschirmwahl am Gerät: bei der Windows-Host-Rolle der
  Wechsel selbst (Duplication neu, Switch, Info, Vollbild, Liste danach; bei
  gleicher Größe bleibt der Encoder auf dem Prozessorweg stehen) — er braucht zwei
  echte Ausgänge, die VM hat einen, belegt sind die reinen Teile in Unit-Tests —,
  dort auch Kennung, Name und Bildrate aus echten Monitoren; beim Mac-Host der
  Hauptbildschirmwechsel live samt Namen aus AppKit, den `hosttest` mit Attrappen
  für Liste und Strom belegt (am Gerät belegt am 26.09.2026: Wunsch auf den zweiten
  Bildschirm des Mac-Hosts mit Wechsel, Maus per `--eingabeprobe` auf dem
  gewünschten Bildschirm, zurück mit `auto`);
  der Windows-Client mit zwei NVIDIA-Karten, bei Tonverlust, mit einem Tongerät,
  das erst nach dem Verbinden dazukommt, und auf einem frischen Windows ohne
  Visual-C++-Laufzeit; der Mac-Client mit Handoff und Ablageverwaltern.
  Die Dateien, die Verknüpfung und der Infobereich sind in zwei Integrationstests
  belegt: auf der VM mit Windows-Host-Rolle und Client, und Client → Mac-Host
  im LAN, zuletzt am Stand e134fc6. Dazu kommen `dateitest`, `ablagetest` und
  die Dateiabschnitte von `hosttest`. Nur in Prüfständen und Unit-Tests belegt
  sind die Einzelinstanz, wenn die erste App gerade endet, und das Beenden,
  wenn macOS das Symbol der Menüleiste nicht zeigt. Noch offen ist Kopieren mit
  Strg+C und Einfügen im Explorer von Hand am Laptop (auf der VM kopierte ein Skript
  über .NET, eingefügt wurde per Shell-Befehl), Einfügen im Finder am Mac, Dateien
  vom Mac-Host zum Client im laufenden Betrieb, der Mac-Client mit Dateien,
  Infobereich und Menüleiste in echter Bedienung (Klicks, Menü, Sprechblase bzw.
  Blase, das Fenster wirklich im Vordergrund nach einem Doppelklick auf die
  Verknüpfung), die Verknüpfung über den Knopf auf dem echten, auch auf OneDrive
  umgeleiteten Desktop, und Durchsatz und Verzögerung nach den letzten
  Nachbesserungen (Fenster der Windows-Host-Rolle 64 KiB, Fäden mit niedrigerer
  Priorität; der Mac-Client sendet unter der Dienstklasse „utility“, die Fristen
  und Schlaf deutlich dehnt – im Modell des Codes 20 MB in 4,1 statt 1,1 s)
- Veröffentlichung: Lizenz, Hinweisdatei, Beitragsregeln und die Release-Abläufe liegen
  bereit (Abschnitt „Lizenz und Veröffentlichung“). Es fehlen die Zertifikate
  (Developer ID von Apple für Signatur und Notarisierung des Mac-Hosts, heute ein
  lokales Zertifikat; eine Codesignatur für die exe), die GitHub-Organisation und der
  erste Release; was dafür einmalig zu tun ist, steht in `RELEASING.md`
- Die Kopplung hängt an der Adresse: `known_hosts.txt` merkt sich den Schlüssel je
  Adresse, und die Bekanntgabe im Netz ist nicht ausgewiesen. Meldet sich unter
  bekanntem Namen ein anderes Gerät von einer neuen Adresse, gilt es als Erstkontakt;
  dann schützt nur der Vergleichscode.
- Die Zwischenablage geht während einer Sitzung auch dann hinüber, wenn das Fenster
  des Clients keinen Fokus hat – kopierte Dateien sofort und bis 4 GB, auch wenn
  drüben nie eingefügt wird. Gelesen wird sie auf allen Seiten nur mit Gegenüber;
  der Mac-Host zählt ohne Zuschauer nur mit, dass sich etwas geändert hat.
- Dateien erst beim Einfügen übertragen (wie RDP) statt sofort beim Kopieren; die
  Liste der Einträge in Teilen statt am Stück (bis 1 MiB, dahinter warten Tasten
  bzw. Bilder kurz)
- Desktop-Verknüpfung auf dem Mac-Client; ein Symbol im Infobereich bzw. in der
  Menüleiste für die Hosts (die haben kein Fenster)
- Kopplungsdialog in der Oberfläche statt Kommandozeilenschalter
- Rollenwahl (Windows als Host): `--host` ist da (Zuschauerplatz, Kopplung, Bekanntgabe,
  Eingaben, Zwischenablage samt Dateien, Desktop Duplication, Encoder über NVENC bzw.
  ohne NVIDIA H.264 in Software, Schrittmacher, Codecwechsel, Bildschirmwahl mit
  Umschalten, Testbild, Ton, Zeigerform, Last, Wachhalten), dazu `--list`, `--messen`
  und die Konserve als Prüfweg. Es fehlen AMF/QSV, HDR-Ausgänge, Skalieren und
  Drehen auf der Karte (beides läuft heute über den Prozessor) und ein Menüeintrag
  "Diesen Rechner freigeben". Beschreibung in `BENUTZUNG.txt`.
- AV1: Kein Host bietet ihn an, bis Protokoll und Client ihn kennen
- Tonkomprimierung als Wahlmöglichkeit; unkomprimiert braucht mehr Bandbreite als das Bild
- Virtuelles Mikrofon auf dem Host, damit Programme dort den Client hören
- Mehrere Zuschauer gleichzeitig
- Stromgröße nach Wunsch des Clients (er meldet seine Fenstergröße, der Host stellt den Strom um);
  heute wechselt die Größe nur mit dem Bildschirm des Hosts, und der Client folgt ihr
- Anzeige: Wahl Sofort/Bildsynchron im Menü, frisches Fenster nach Geräteverlust; ab 4K der Null-Kopien-Weg (NVDEC-Bild bleibt auf der Karte)
- D3D11VA ohne Kopie: das decodierte Bild bleibt auf der Karte, die zeichnet (heute eine Kopie über den Prozessor)
- Später als Zusatz denkbar: beide Bildschirme des Hosts auf Wunsch streamen; ein Relay, an das sich weitere Zuschauer heften (Bild und Ton, ohne Eingabe)

## Prüfstände

Für den Mac-Host gibt es kleine Prüfprogramme, die ohne Bildschirmaufnahme laufen
und einen laufenden Host samt Ports, Freigaben und Zwischenablage in Ruhe lassen.
Gebaut wird aus dem Wurzelordner, die Bauzeile steht jeweils im Dateikopf;
Rückgabe ist die Zahl der Fehler.

- `host/annahmetest.c`: Handschlagfrist, Plätze und Verdrängen, Freigabeliste,
  `host.key`. Loopback ab Port 19000, eigenes `HOME`, rund 25 s.
- `host/hosttest.m`: bindet `main.m` ein und prüft Drossel (je Art und Adresse) und
  Obergrenze des Protokolls, Zuschauerwechsel (Nachricht 10) samt Testbild-Rest,
  Abbau zwischen Hochfahren und Eintragen, Nachreichen bei stillem Bildschirm,
  Codecwechsel ohne Zuschauer und mit Formatwechsel (echte VT-Encoder), `--fest`,
  Stauregel samt Ton im Stau, Ansage des Tonformats und AV1 in der Könnensliste,
  dazu Dateien über die Zwischenablage über die echten Kanäle: Fähigkeiten
  (Nachrichten 11 und 69), Client → Host mit Quittungen auf dem Bildkanal und den
  Zeilen „empfange …“ und „empfangen …“ im Host-Protokoll, Host → Client mit
  Fenster, Namen mit Umlauten im Protokoll, Zuschauerwechsel mitten in der
  Übertragung samt Sitzungsbindung des Sendewegs, Zuschauer ohne Eingabekanal,
  älterer Client ohne Fähigkeiten, Fähigkeit nur für den Eingabekanal, der sie
  gemeldet hat, Obergrenzen auf dem Eingabekanal. Statt in die Ablage gehen
  empfangene Dateien an einen Rekorder, die Ablagebasis liegt im eigenen `HOME`.
  Dazu die Bildschirmwahl (`host/bildschirm.m`): Prüfvektoren der Nachrichten 12
  und 70, Kürzen an Zeichengrenzen, die reine Wahl, Stromgröße, `bildschirm.txt`
  (auch kaputt), `--display` als Pin für den Lauf, Begrüßung mit Fähigkeiten 3
  und Liste, Automatik folgt dem Hauptbildschirm (entprellt), Wunsch über den
  echten Eingabekanal, fehlender Wunsch mit Ausweichplatz und Rückkehr, andere
  Größe mit neuem Encoder und Vollbild, Warten auf einen laufenden Codecwechsel
  (jeder Wunsch bekommt seine Antwort; das Warten endet, sobald kein Wechsel
  mehr ansteht; nach 5 s kommt der Wechsel trotzdem), leere Liste bei laufendem
  Strom, Bildschirmverlust und Wiederherstellung — Liste und Strom kommen aus
  Attrappen, die Encoder sind echt.
  Loopback ab Port 19100 und 19400/19450, eigenes `HOME` unter `$TMPDIR` (nach
  einem bestandenen Lauf wieder entfernt), rund 100 s. Nutzt kurz echte
  HEVC-Encoder (640×360, 1920×1080, 1280×720) — nicht während eines Streams starten.
- `host/dateitest.m`: das Dateiprotokoll aus `host/dateien.m` ohne Netz und ohne
  Ablage — Prüfvektoren, Pfadregeln samt Bereinigung für macOS und (für den
  Schlüssel des Senders gegen Doppelte) für Windows, Empfänger und
  Sender gegeneinander im Speicher, Nachzügler aus einer anderen Sitzung, auch
  ein verspätetes Angebot mitten in einer laufenden Übertragung, wann
  der Empfänger quittiert (ab 16 KiB offen und sobald nichts mehr wartet), seine
  Warteschlange nach Datenbytes (Fenster plus ein Stück, höchstens 8 Enden, ein
  neues Angebot leert sie), die Zeile „Dateien: empfange …“ beim Annehmen (nicht,
  wenn schon die Quittung 0 nicht mehr hinausgeht), Namen in NFC auf der Platte
  (die rohen Bytes, auch wenn das Angebot NFD trägt), ein `/` vor einem
  kombinierenden Zeichen, Fenster, Drossel über den Sendepuffer samt Takt (alle
  2–3 ms, nicht 10 ms), Stillstand (eine wiederholte Quittung ohne Fortschritt
  hält den Sender nicht am Leben), Abbrüche, eine Quelldatei, die nach dem
  Auflisten gegen eine FIFO oder eine Verknüpfung getauscht wird, ebenso ein
  Ordner darüber (gegen eine Verknüpfung und gegen einen anderen echten Ordner,
  dort hält der Vergleich von Gerät und Inode die fremde Datei zurück), ein
  Ordner, der während des Auflistens fortlaufend gegen eine Verknüpfung
  getauscht wird (1,5 s Dauerlauf), Namen, die erst nach der Windows-Bereinigung
  doppelt sind, ein `\` vor einem kombinierenden Zeichen, Senden aus einer
  Quelle, deren Pfad absolut über `PATH_MAX` liegt, Aufräumen — auch mit einer
  Basis, die eine Verknüpfung ist, nach einem Rücksprung der Uhr, von
  Übertragungen, deren Pfade absolut über `PATH_MAX` liegen, mit der Marke
  `.laeuft` laufender Übertragungen und von Waisen (unfertig beim Start) —,
  sowie die Dienstklasse der beiden Warteschlangen (`dateien-senden` mit
  `QOS_CLASS_DEFAULT` und relativer Priorität −15, `dateien-empfang` mit
  `QOS_CLASS_UTILITY`). Alle Dateien in einem frischen Ordner unter `$TMPDIR`,
  rund 8 s.
- `host/ablagetest.m`: Kennzeichnung des empfangenen Texts, die Regel „nur mit
  Zuschauer lesen“ und Dateiverweise (`public.file-url`) lesen und schreiben,
  auf einer eigenen benannten Ablage statt der allgemeinen; die Dateien dafür
  liegen unter `$TMPDIR`.

```
clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/annahmetest.c host/qc_annahme.c \
      host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/annahmetest

clang -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations \
      -mmacosx-version-min=14.0 -framework Foundation -framework AppKit \
      -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia \
      -framework CoreVideo -framework CoreGraphics -framework CoreFoundation -framework IOKit \
      host/hosttest.m host/audio.m host/clipboard.m host/zeiger.m host/testbild.m host/last.m \
      host/dateien.m host/bildschirm.m host/qc_noise.c host/qc_secure.c host/qc_annahme.c \
      host/vendor/monocypher/monocypher.c -o /tmp/hosttest

clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
      -framework Foundation host/dateitest.m host/dateien.m -o /tmp/dateitest

clang -fobjc-arc -O2 -Wall -Ihost -mmacosx-version-min=14.0 -framework Foundation \
      -framework AppKit host/ablagetest.m -o /tmp/ablagetest

clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/noisetest.c host/qc_noise.c \
      host/vendor/monocypher/monocypher.c -o /tmp/noisetest
```

`noisetest` ohne Argument ist der Selbsttest der Kryptoschicht ("OK", sonst die
Stelle des Fehlers), einschließlich eines gescheiterten Handschlags, nach dem kein
DH-Ergebnis auf dem Stapel bleiben darf; mit einem Port wartet er auf den
Gegentest der Rust-Seite (`quadchroma --noisetest adresse:port`).

Die Rust-Seite prüft sich mit `cargo test --release` in `client\` bzw. `client/`;
mehrere Tests brauchen Loopback (TCP über 127.0.0.1). Auf Windows gehört der
`bin`-Ordner von FFmpeg in den `PATH`. Die Ablagetests beschreiben dort die echte
Zwischenablage der Sitzung, nacheinander hinter dem benannten Mutex
`Global\QuadChromaAblageTest`, und in einer Sitzung mit Explorer erscheint kurz ein
echtes Symbol im Infobereich (ohne Sprechblase). Auf dem Mac mit
`-- --skip clipboard`, sonst liest und beschreibt `setzen_zaehlen_lesen` die echte
Zwischenablage; die übrigen Ablagetests des Mac-Clients arbeiten auf eigenen Brettern
und laufen einzeln per Namen (`-- --exact clipboard_mac::tests::<name>`). Den
Ablageordner fassen die Tests nie an: Schlüssel, Einstellungen, `protokoll.txt` und
`quadchroma.ico` liegen je Lauf in `qc-test-<pid>-<ms>/QuadChroma` im Temp-Ordner,
`APPDATA` muss also nicht umgelenkt werden. Empfangene Dateien schreiben die Tests
in `qc-test-<pid>-ablage` bzw. `qc-test-<pid>-host-ablage`, ebenfalls im
Temp-Ordner, nie in `QuadChroma-Ablage`.
Zur Bildschirmwahl prüfen die Rust-Tests die Prüfvektoren der Nachrichten 12 und 70
(`bildschirm.rs`), im Client die Sitzung gegen einen Scheinhost (Bit 1, Liste, Wunsch
samt Hinweis und Nachreichen, eine Strominfo mit neuer Größe baut den Decoder neu,
Rücksetzen am Sitzungsende) und die Texte in allen 29 Sprachen, in der
Windows-Host-Rolle die reine Wahl (Automatik folgt dem Hauptbildschirm, Wunsch,
Ausweichplatz, Rückkehr, Gründe), Kennung und Name aus der Geräte-ID,
`bildschirm.txt` samt Resten, Nachricht 12 erst nach dem Aufbau, die Win32-Wege der
Liste, den Wunsch über den echten Eingabekanal und die Begrüßung mit Fähigkeiten 3
und Liste 12.

Das Symbol im Infobereich bzw. in der Menüleiste prüft ein Selbsttest ohne Netz:
`quadchroma.exe --tray-selbsttest` (braucht eine Sitzung mit Explorer) bzw.
`quadchroma --menueleiste-selbsttest` auf dem Mac, Rückgabe 0 oder 1.

## Lizenz und Veröffentlichung

Alle Rechtstexte und die Dateien für GitHub sind englisch:

- `LICENSE.txt` – PolyForm Strict License 1.0.0 im Wortlaut, dazu die Zusatzbedingungen
  des Lizenzgebers: nur persönliche, nichtkommerzielle Nutzung (jede Nutzung in oder für
  Unternehmen, Behörden und andere Organisationen ist kommerziell, auch intern), kein
  Einbetten in andere Produkte, Geräte oder Dienste, keine Weitergabe, keine
  Bearbeitung. Kommerzielle Lizenzen auf Anfrage an hello@quadchroma.tech.
- `THIRD_PARTY_NOTICES.txt` – alle Fremdanteile der verteilten Programme mit ihren
  Lizenztexten: FFmpeg (LGPL 2.1, eigener Bau, Quellen und schriftliches Angebot),
  die NVIDIA-Header, die Compiler-Laufzeit, die Rust-Crates je Ziel, Monocypher.
- `CLA.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`,
  `.github/` (Vorlagen für Issues und Pull Requests) – Beiträge nur mit der
  Beitragsvereinbarung; sie räumt je Beitrag die Rechte ein, die eine spätere
  Doppel-Lizenzierung braucht.
- `.github/README.md` – englische Übersicht für GitHub; dieses README und `BENUTZUNG.txt`
  bleiben die vollständige deutsche Dokumentation.
- `RELEASING.md` – Signieren und Veröffentlichen: was einmalig nötig ist (Apple
  Developer Program und Developer-ID-Zertifikat; für Windows Azure Artifact Signing
  oder ein Codesignatur-Zertifikat; die Secrets für GitHub), und wie ein Release lokal
  (`make sign notarize staple dmg`, `scripts/sign-windows.ps1`) oder über
  `.github/workflows/release.yml` läuft (Tag `vX.Y.Z` → Entwurf mit allen Paketen,
  SHA256SUMS und den FFmpeg-Quellen).

FFmpeg: Der Windows-Client nutzt Bibliotheken des FFmpeg-Projekts unter der LGPLv2.1,
als eigene DLLs neben der exe. Sie entstehen mit `scripts/build-ffmpeg-windows.sh`
aus dem unveränderten FFmpeg 9.0.2 ohne Fremdbibliotheken (vorher: ein fertiger Build,
der über chromaprint GPL-Code enthielt und sich deshalb nicht weitergeben ließ).
Der Startbildschirm nennt FFmpeg in der Fußzeile, wie es die LGPL verlangt, sobald ein
Programm Urheberhinweise zeigt. Die exe trägt Versionsangaben, Symbol und Manifest
(`client/build.rs`, `client/res/`), der Mac-Host die Bundle-Kennung
`tech.quadchroma.host`.

## Aufbau

    host/        Mac: Aufnahme, Encoder, Netz, Eingaben, Ton, Zwischenablage, Dateien (dateien.m),
                 Bildschirmwahl (bildschirm.m), Zeigerform
    client/      Windows: Empfang, Decodieren, Anzeige (anzeige.rs: Direct3D 11), Eingaben, Ton, Zwischenablage;
                 Dateien (dateien.rs) und Bildschirmliste (bildschirm.rs), beide gemeinsam mit der Host-Rolle,
                 Infobereich bzw. Menüleiste (tray*.rs), Einzelinstanz (einzel.rs),
                 Desktop-Verknüpfung (verknuepfung.rs), Programmsymbol (logo.rs);
                 dazu die Host-Rolle (client/src/host/: Aufnahme mit Bildschirmwahl, Encoder, Netz, Eingaben,
                 Ton, Zeigerform)
    Makefile     baut und signiert das App-Bündel; sign, notarize, staple, zip, dmg für die Weitergabe
    scripts/     build-ffmpeg-windows.sh (FFmpeg für Windows), sign-windows.ps1 (Codesignatur der exe)
    .github/     CI und Release (workflows/), Vorlagen für Issues und Pull Requests

Der Host ist in C und Objective-C geschrieben, der Client in Rust.
