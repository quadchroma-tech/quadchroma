# QuadChroma

Fernsteuerung mit voller Farbauflösung. Der Mac nimmt seinen Bildschirm auf, codiert
ihn in Hardware als HEVC 4:4:4 mit 10 Bit und schickt ihn an einen Windows-Rechner.
Der Mauszeiger wird bewusst **nicht** ins Bild gerendert: Sichtbar ist der Zeiger von
Windows, der Mac zieht unsichtbar mit. Deshalb fühlt sich die Maus lokal an — und
damit er trotzdem aussieht wie auf dem Mac, schickt der Host nur die *Form* des
Zeigers (Pfeil, Hand, Ziehpfeile, Textcursor, Wartekugel), sobald sie sich ändert.

Stand: 22.09.2026. Läuft, ist aber noch ein Gerüst, keine fertige Anwendung.

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

Auf Windows einfach starten:

    quadchroma.exe

Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, erscheint also von selbst
im Startbildschirm. Drei Kanäle: 9001 Bild und Ton, 9002 Eingaben und Zwischenablage,
9003 die Bekanntgabe. Was der Client entscheidet (Decoder, Anzeige, Zeigerform) und
was FFmpeg dazu sagt, steht in `%APPDATA%\QuadChroma\protokoll.txt`; die Bedienung
im Einzelnen beschreibt `BENUTZUNG.txt`.

## Bedienung im Client

| Taste | Wirkung |
|---|---|
| F9 | Statistik ein und aus |
| F10 oder ESC zwei Sekunden halten | Menü: Bild, Anzeige, Verschlüsselung |
| F11 | Vollbild an und aus |
| F12 | pixelgenaue Darstellung statt gestreckt |
| Strg+Esc | zurück zum Startbildschirm |

Jede Tastenfunktion ist im Menü auch ein Schalter. Alle übrigen Tasten gehen an den
Mac. Übertragen wird die **Position** der Taste, nicht das Zeichen, deshalb
funktionieren Umlaute und AltGr ohne Zutun. Die Windows-Taste ist die Befehlstaste des
Macs. Datenrate, Bildrate, Spielmodus, feste Bildrate, Ton und Codec stellt man im
laufenden Betrieb um; der Mac meldet zurück, was gilt, und der Client merkt sich die
Werte je Host.

## Gemessen auf einem Mac mini M1

| | |
|---|---|
| Bild | 1920×1080, 4:4:4, 10 Bit, 120 Bilder/s Ziel, der Host deckelt darauf |
| Datenrate Bild | 3 bis 11 Mbit/s bei ruhigem Bild, bis zur eingestellten Grenze bei Bewegung |
| Encoder | Hardware, rund 8 ms je Bild, wenige Prozent eines Kerns |
| Codecs | HEVC 4:4:4 und 4:2:0 in 8 und 10 Bit, H.264 — alle in Hardware, im Betrieb umschaltbar |
| Decodieren auf Windows | NVDEC in Hardware; Software 8,6 ms je Bild (scheibenparallel, damit kein Bild zurückgehalten wird) |
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
Bildkanal legitim aufgebaut zu haben.

Freigabe: beim allerersten Mal nimmt der Host die erste Gegenstelle auf, danach
braucht jedes neue Gerät einen Host, der mit `--pair` gestartet wurde. Der
Client merkt sich den Fingerabdruck des Hosts und verweigert die Verbindung,
wenn er sich ändert.

Auf der Mac-Seite ist das Muster eigens umgesetzt (rund 400 Zeilen C auf
Monocypher), auf der Windows-Seite kommt die bekannte Rust-Umsetzung zum
Einsatz. Beide wurden gegeneinander geprüft: gleicher Vergleichscode, gleiche
Fingerabdrücke, verschlüsselter Austausch in beide Richtungen.

## Was fehlt

- Kopplungsdialog in der Oberfläche statt Kommandozeilenschalter
- Rollenwahl (Windows als Host)
- Tonkomprimierung als Wahlmöglichkeit; unkomprimiert braucht mehr Bandbreite als das Bild
- Virtuelles Mikrofon auf dem Host, damit Programme dort den Client hören
- Mehrere Zuschauer gleichzeitig
- Auflösungswechsel im laufenden Betrieb (der Client meldet seine Fenstergröße, der Host stellt den Strom um)
- Bildschirmwahl im Menü statt nur beim Start
- Anzeige: Wahl Sofort/Bildsynchron im Menü, frisches Fenster nach Geräteverlust; ab 4K der Null-Kopien-Weg (NVDEC-Bild bleibt auf der Karte)
- Ein Benchmark, der Codecs, Datenraten und Bildraten der Reihe nach durchmisst

## Aufbau

    host/        Mac: Aufnahme, Encoder, Netz, Eingaben, Ton, Zwischenablage, Zeigerform
    client/      Windows: Empfang, Decodieren, Anzeige (anzeige.rs: Direct3D 11), Eingaben, Ton, Zwischenablage
    Makefile     baut und signiert das App-Bündel

Der Host ist in C und Objective-C geschrieben, der Client in Rust.
