// QuadChroma Host: Abgleich der Zwischenablage, Mac-Seite.
//
// Zwei Richtungen, Nachrichtentyp 48, Nutzlast reiner UTF-8-Text ohne
// Abschluss-Null:
//   Mac -> Client : qc_clip_start() meldet ueber den Rueckruf, was der Benutzer
//                   am Mac kopiert hat; main.m schickt es ueber den Bildkanal
//                   (Port 9001).
//   Client -> Mac : kommt ueber den Eingabekanal (Port 9002); qc_clip_set()
//                   legt den Text des Clients in die Ablage.
//
// Dazu Dateiverweise (public.file-url): qc_clip_dateien() meldet eine
// Dateiliste, die der Nutzer am Mac kopiert hat, qc_clip_set_dateien() legt
// eine empfangene in die Ablage. Uebertragen werden die Dateien selbst von
// dateien.m. Bilder werden bewusst ausgelassen, Text ueber 4 MB ebenfalls,
// mit einer Zeile im Protokoll.
#ifndef QUADCHROMA_CLIPBOARD_H
#define QUADCHROMA_CLIPBOARD_H

#include <stddef.h>
#ifdef __OBJC__
#import <Foundation/Foundation.h>
#endif

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

#ifdef __OBJC__
// Rueckruf fuer Dateiverweise; vor qc_clip_start() setzen. Enthaelt die Ablage
// Dateiverweise (public.file-url, gesucht ueber alle Eintraege,
// Datei-Referenz-URLs aufgeloest), kommt statt des Textes die Liste der Pfade
// - der Text geht dann nicht hinaus, auch nicht der Dateiname, den Finder
// zusaetzlich als Text ablegt. Traegt auch nur ein Eintrag die Kennung
// "verdeckt", wird gar nichts gelesen. Gelesen wird wie beim Text nur mit
// Zuschauer (qc_clip_bedingung). Der Rueckruf laeuft auf derselben seriellen
// Warteschlange wie der fuer Text und darf blockieren.
void qc_clip_dateien(void (*on_dateien)(NSArray<NSString *> *pfade));

// Legt eine Dateiliste in die Ablage: je Pfad ein Eintrag mit public.file-url,
// gekennzeichnet als voruebergehend und automatisch erzeugt, nur fuer diesen
// Mac. Darf aus jedem Faden aufgerufen werden und kehrt sofort zurueck;
// geschrieben wird versetzt auf dem Hauptfaden. Loest keinen Rueckruf aus.
void qc_clip_set_dateien(NSArray<NSString *> *pfade);
#endif

#ifdef __cplusplus
}
#endif

#endif // QUADCHROMA_CLIPBOARD_H
