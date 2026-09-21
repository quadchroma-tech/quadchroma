# QuadChroma

Fernsteuerung mit voller Farbauflösung. Der Mac nimmt seinen Bildschirm auf, codiert
ihn in Hardware als HEVC 4:4:4 mit 10 Bit und schickt ihn an einen Windows-Rechner.
Der Mauszeiger wird bewusst **nicht** ins Bild gerendert: Sichtbar ist der Zeiger von
Windows, der Mac zieht unsichtbar mit. Deshalb fühlt sich die Maus lokal an.

Stand: 20.09.2026. Läuft, ist aber noch ein Gerüst, keine fertige Anwendung.

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
    open -n build/QuadChroma.app --args --serve 9001 --fps 120 --mbit 150

Beim ersten Start zwei Freigaben erteilen, jeweils in den Systemeinstellungen unter
Datenschutz & Sicherheit:

- **Bildschirmaufnahme** für das Bild
- **Bedienungshilfen** für Maus und Tastatur

Auf Windows einfach starten:

    quadchroma.exe

Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, erscheint also von selbst
im Startbildschirm. Drei Kanäle: 9001 Bild und Ton, 9002 Eingaben und Zwischenablage,
9003 die Bekanntgabe.

## Tasten im Client

| Taste | Wirkung |
|---|---|
| F9 | Zahlen einblenden |
| F11 | Vollbild an und aus |
| F12 | pixelgenaue Darstellung statt gestreckt |
| Strg+Esc | zurück zum Startbildschirm |

Alle übrigen Tasten gehen an den Mac. Übertragen wird die **Position** der Taste, nicht
das Zeichen, deshalb funktionieren Umlaute und AltGr ohne Zutun. Die Windows-Taste ist
die Befehlstaste des Macs.

## Gemessen auf einem Mac mini M1

| | |
|---|---|
| Bild | 1920×1080, 4:4:4, 10 Bit, bis 105 Bilder/s |
| Datenrate Bild | 3 bis 11 Mbit/s, je nach Bewegung |
| Encoder | Hardware, rund 5 % eines Kerns |
| Decodieren auf Windows | 2,0 ms je Bild, ohne Grafikkarte |
| Ton | unkomprimiert, Stereo, 48 kHz, rund 3 Mbit/s |

## Oberfläche

Dunkler Grund, feines Raster, Neonlinien in Cyan und Magenta, Ecken als Klammern.
Alles direkt in den Bildpuffer gezeichnet, ohne Fremdbaukasten, damit das Bild des
Macs und die Oberfläche im selben Puffer liegen.

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
- Rollenwahl (Windows als Host), Einstellungen im laufenden Betrieb
- Tonkomprimierung als Wahlmöglichkeit; unkomprimiert braucht mehr Bandbreite als das Bild
- Virtuelles Mikrofon auf dem Host, damit Programme dort den Client hören
- Mehrere Zuschauer gleichzeitig
- Auflösungswechsel im laufenden Betrieb
- Anzeige über die Grafikkarte, nötig erst ab 4K

## Aufbau

    host/        Mac: Aufnahme, Encoder, Netz, Eingaben, Ton, Zwischenablage
    client/      Windows: Empfang, Decodieren, Anzeige, Eingaben, Ton, Zwischenablage
    Makefile     baut und signiert das App-Bündel

Der Host ist in C und Objective-C geschrieben, der Client in Rust.
