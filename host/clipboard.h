// QuadChroma Host: Abgleich der Zwischenablage, Mac-Seite.
//
// Zwei Richtungen, beide ueber die Eingabeverbindung (Port 9002), Nachrichtentyp
// 48, Nutzlast reiner UTF-8-Text ohne Abschluss-Null:
//   Mac -> Client : qc_clip_start() meldet ueber den Rueckruf, was der Benutzer
//                   am Mac kopiert hat.
//   Client -> Mac : qc_clip_set() legt den Text des Clients in die Ablage.
//
// Nur Text. Bilder und Dateien werden bewusst ausgelassen; mehr als 4 MB
// ebenfalls, mit einer Zeile im Protokoll.
#ifndef QUADCHROMA_CLIPBOARD_H
#define QUADCHROMA_CLIPBOARD_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Startet die Ueberwachung der allgemeinen Zwischenablage. Einmal aufrufen,
// gerne direkt aus main(); weitere Aufrufe werden ignoriert und protokolliert.
//
// Der Rueckruf laeuft auf einer eigenen seriellen Warteschlange, nicht auf dem
// Hauptfaden: er darf also ohne Schaden fuer Bild und Eingabe blockieren, etwa
// beim Senden ueber den Socket. Aufrufe kommen nacheinander, nie gleichzeitig.
// Der Zeiger utf8 gilt nur waehrend des Aufrufs; wer den Text laenger braucht,
// kopiert ihn. Der Puffer ist zusaetzlich mit einer Null abgeschlossen,
// massgeblich ist aber len (Text darf im Prinzip Nullbytes enthalten).
//
// Der Inhalt, der beim Start schon in der Ablage liegt, wird nicht gemeldet:
// erst die naechste Aenderung loest den Rueckruf aus.
void qc_clip_start(void (*on_change)(const char *utf8, size_t len));

// Liest den Inhalt nur, solange aktiv() nicht 0 liefert (Hauptlauf: nur mit
// Zuschauer). Ohne Gegenueber wird nur der Zaehler weitergefuehrt: Aenderungen
// aus dieser Zeit werden weder gelesen noch spaeter nachgereicht. Ohne Aufruf
// (oder mit NULL) liest der Abgleich wie bisher immer.
void qc_clip_bedingung(int (*aktiv)(void));

// Legt Text vom Client in die Zwischenablage. Darf aus jedem Faden aufgerufen
// werden und kehrt sofort zurueck; geschrieben wird versetzt auf dem Hauptfaden.
// Loest den Rueckruf von qc_clip_start() nicht aus. Ungueltiges UTF-8 und
// Nutzlasten ueber 4 MB werden verworfen und protokolliert.
void qc_clip_set(const char *utf8, size_t len);

#ifdef __cplusplus
}
#endif

#endif // QUADCHROMA_CLIPBOARD_H
