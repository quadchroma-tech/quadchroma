# QuadChroma

Fernsteuerung mit voller Farbauflösung. Der Mac nimmt seinen Bildschirm auf, codiert
ihn in Hardware als HEVC 4:4:4 mit 10 Bit und schickt ihn an einen Windows-Rechner.
Der Mauszeiger wird bewusst **nicht** ins Bild gerendert: Sichtbar ist der Zeiger von
Windows, der Mac zieht unsichtbar mit. Deshalb fühlt sich die Maus lokal an — und
damit er trotzdem aussieht wie auf dem Mac, schickt der Host nur die *Form* des
Zeigers (Pfeil, Hand, Ziehpfeile, Textcursor, Wartekugel), sobald sie sich ändert.

Stand: 23.09.2026. Läuft, ist aber noch ein Gerüst, keine fertige Anwendung.

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
Leerlauf). Aufgenommen wird der Hauptbildschirm, `--display n` wählt einen anderen.
Sein Protokoll steht in `/tmp/quadchroma-m1.log`; über 8 MB wandert es nach
`/tmp/quadchroma-m1.alt.log` und beginnt neu.

Auf Windows einfach starten:

    quadchroma.exe

Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, erscheint also von selbst
im Startbildschirm. Drei Kanäle: 9001 Bild und Ton (und alles vom Host zum Client),
9002 Eingaben und Zwischenablage vom Client, 9003 die Bekanntgabe. Was der Client
entscheidet (Decoder, Anzeige, Zeigerform) und was FFmpeg dazu sagt, steht in
`%APPDATA%\QuadChroma\protokoll.txt`; die Bedienung im Einzelnen beschreibt
`BENUTZUNG.txt`.

### Windows-Client bauen

Rust (MSVC), LLVM für bindgen und ein FFmpeg-9-Build unter LGPL als DLLs:

    set FFMPEG_DIR=C:\pfad\zu\ffmpeg-n9.0-latest-win64-lgpl-shared-9.0
    set LIBCLANG_PATH=C:\Program Files\LLVM\bin
    cd client
    cargo build --release

Gebaut wird aus `client\`: nur dort greift `client\.cargo\config.toml`, das die
C-Laufzeit statisch in die exe bindet — sonst bräuchte sie `VCRUNTIME140.dll`
aus dem Visual C++ Redistributable. Eine gesetzte Umgebungsvariable `RUSTFLAGS`
ersetzt diese Einstellung. Fehlt so die statische Laufzeit, bricht der Bau mit
einem Hinweis ab.
Neben die exe gehören `avcodec-63.dll`, `avformat-63.dll`, `avutil-61.dll` und
`swresample-7.dll`; Voraussetzung ist Windows 10 oder 11 (64 Bit), eine
Visual-C++-Laufzeit braucht es nicht.

### Mac als Client (im Aufbau)

Derselbe Client baut auch auf dem Mac (arm64), mit Ton über AudioToolbox und
Zwischenablage über NSPasteboard; die Anzeige läuft dort noch über die CPU
(softbuffer), Metal kommt später. Bauen mit Rust und dem FFmpeg aus Homebrew:

    cd client
    FFMPEG_DIR=/opt/homebrew/opt/ffmpeg cargo build --release
    ./target/release/quadchroma 192.168.178.194:9001

Ablage unter `~/Library/Application Support/QuadChroma` (client.key, known_hosts.txt,
einstellungen.txt, protokoll.txt, benchmark.txt — neben host.key und authorized.txt
des Hosts, getrennte Dateien). Lizenzhinweis: das Homebrew-FFmpeg ist ein GPL-Build
(libx264, libx265); für den Eigengebrauch in Ordnung, zur Weitergabe des Mac-Clients
braucht es einen LGPL-Build ohne GPL-Teile.

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
Macs. Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton und Codec stellt man im
laufenden Betrieb um; der Mac meldet zurück, was gilt, und der Client merkt sich die
Werte je Host.

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
er sich selbst ausweist, und Tasten und Zwischenablage gehen nur an denselben
Host. Der Host bindet den Eingabekanal an genau einen Zuschauer: Löst ein neuer
ihn ab oder reißt sein Bild ab, kappt er ihn und lässt dessen gedrückte Tasten
und Maustasten los.

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
höchstens alle 10 s eine Zeile je Art (beim Mac-Host je Art und Adresse), mit
der Zahl der unterdrückten. Beide Hosts begrenzen ihr Protokoll auf 8 MB und
schieben den älteren Teil in eine `.alt`-Datei.

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
  (auch unter der Tonrate) und im Spielmodus bei hoher Datenrate; der Mac-Host nach
  einem Neubau (Zuschauerwechsel mit Nachricht 10, Nachreichen bei stillem
  Bildschirm, Tonformat mit 44,1 kHz, Ablageverwalter, beschädigte Freigabeliste);
  der Windows-Client mit zwei NVIDIA-Karten, bei Tonverlust, mit einem Tongerät,
  das erst nach dem Verbinden dazukommt, und auf einem frischen Windows ohne
  Visual-C++-Laufzeit; der Mac-Client mit Handoff und Ablageverwaltern
- Veröffentlichung: ein Lizenzpaket für die Binärdateien (GPL- und LGPL-Text, Hinweise
  der Rust-Abhängigkeiten, der zu den DLLs passende FFmpeg-Quellstand samt
  Bauangaben) und eine Lizenz für das Projekt selbst; Developer-ID-Signatur und
  Notarisierung für den Mac-Host (heute ein lokales Zertifikat), eine Signatur der exe
- Die Kopplung hängt an der Adresse: `known_hosts.txt` merkt sich den Schlüssel je
  Adresse, und die Bekanntgabe im Netz ist nicht ausgewiesen. Meldet sich unter
  bekanntem Namen ein anderes Gerät von einer neuen Adresse, gilt es als Erstkontakt;
  dann schützt nur der Vergleichscode.
- Die Zwischenablage geht während einer Sitzung auch dann hinüber, wenn das Fenster
  des Clients keinen Fokus hat. Der Mac-Host fragt sie auch ohne Zuschauer ab
  (gesendet wird dann nichts); Client und Windows-Host lesen sie nur mit Gegenüber.
- Kopplungsdialog in der Oberfläche statt Kommandozeilenschalter
- Rollenwahl (Windows als Host): `--host` ist da (Zuschauerplatz, Kopplung, Bekanntgabe,
  Eingaben, Zwischenablage, Desktop Duplication, Encoder über NVENC bzw. ohne NVIDIA
  H.264 in Software, Schrittmacher, Codecwechsel, Testbild, Ton, Zeigerform, Last,
  Wachhalten), dazu `--list`, `--messen` und die Konserve als Prüfweg. Es fehlen
  AMF/QSV, HDR-Ausgänge, Skalieren und Drehen auf der Karte (beides läuft heute über
  den Prozessor), die Bildschirmwahl im Menü und ein Menüeintrag "Diesen Rechner
  freigeben". Beschreibung in `BENUTZUNG.txt`.
- AV1: Kein Host bietet ihn an, bis Protokoll und Client ihn kennen
- Tonkomprimierung als Wahlmöglichkeit; unkomprimiert braucht mehr Bandbreite als das Bild
- Virtuelles Mikrofon auf dem Host, damit Programme dort den Client hören
- Mehrere Zuschauer gleichzeitig
- Auflösungswechsel im laufenden Betrieb (der Client meldet seine Fenstergröße, der Host stellt den Strom um)
- Bildschirmwahl im Menü statt nur beim Start
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
  Codecwechsel ohne Zuschauer, Stauregel samt Ton im Stau, Ansage des Tonformats
  und AV1 in der Könnensliste. Loopback ab Port 19100 und 19400/19450, eigenes
  `HOME` unter `$TMPDIR`, rund 80 s. Nutzt kurz einen echten kleinen
  HEVC-Encoder (640×360) — nicht während eines Streams starten.
- `host/ablagetest.m`: Kennzeichnung des empfangenen Texts, auf einer eigenen
  benannten Ablage statt der allgemeinen.

```
clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/annahmetest.c host/qc_annahme.c \
      host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/annahmetest

clang -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations \
      -mmacosx-version-min=14.0 -framework Foundation -framework AppKit \
      -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia \
      -framework CoreVideo -framework CoreGraphics -framework CoreFoundation -framework IOKit \
      host/hosttest.m host/audio.m host/clipboard.m host/zeiger.m host/testbild.m host/last.m \
      host/qc_noise.c host/qc_secure.c host/qc_annahme.c host/vendor/monocypher/monocypher.c \
      -o /tmp/hosttest

clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/noisetest.c host/qc_noise.c \
      host/vendor/monocypher/monocypher.c -o /tmp/noisetest
```

`noisetest` ohne Argument ist der Selbsttest der Kryptoschicht ("OK", sonst die
Stelle des Fehlers), einschließlich eines gescheiterten Handschlags, nach dem kein
DH-Ergebnis auf dem Stapel bleiben darf; mit einem Port wartet er auf den
Gegentest der Rust-Seite (`quadchroma --noisetest adresse:port`).

Die Rust-Seite prüft sich mit `cargo test --release` in `client\` bzw. `client/`;
mehrere Tests brauchen Loopback (TCP über 127.0.0.1). Auf Windows gehört der
`bin`-Ordner von FFmpeg in den `PATH`, und `APPDATA` sollte auf einen eigenen Ordner
zeigen: Ein Test beschreibt die Zwischenablage der Sitzung und schreibt ins
Protokoll. Auf dem Mac mit `-- --skip clipboard`, sonst lesen und beschreiben Tests
die echte Zwischenablage.

## Aufbau

    host/        Mac: Aufnahme, Encoder, Netz, Eingaben, Ton, Zwischenablage, Zeigerform
    client/      Windows: Empfang, Decodieren, Anzeige (anzeige.rs: Direct3D 11), Eingaben, Ton, Zwischenablage;
                 dazu die Host-Rolle (client/src/host/: Aufnahme, Encoder, Netz, Eingaben, Ton, Zeigerform)
    Makefile     baut und signiert das App-Bündel

Der Host ist in C und Objective-C geschrieben, der Client in Rust.
