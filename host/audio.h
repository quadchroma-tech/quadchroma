// QuadChroma Host, Tonspur.
//
// Nimmt den Systemton ueber ScreenCaptureKit auf demselben SCStream mit auf,
// der schon das Bild liefert, und reicht ihn als verschachtelte float32-Rahmen
// nach oben. Kein eigener Socket, kein eigener Thread: das Versenden macht der
// Rueckruf, den qc_audio_attach bekommt.
//
// Reihenfolge der Aufrufe:
//   1. qc_audio_configure(cfg)   VOR  [[SCStream alloc] initWithFilter:...]
//                                     (capturesAudio wirkt nur aus der
//                                      Konfiguration, mit der der Strom entsteht)
//   2. qc_audio_attach(stream, cb)  VOR  startCaptureWithCompletionHandler:
//   3. qc_audio_rate() / qc_audio_channels() fuer die Strominfo an den Client
//
// Zum Draht passend gedacht, verschickt wird aber ausserhalb dieses Moduls:
//   Typ 32 Toninfo  : u32 Abtastrate, u8 Kanaele, u8 Format (1 = float32
//                     verschachtelt), u16 frei
//   Typ 33 Tondaten : rohe verschachtelte float32-Werte, Kanal 0, Kanal 1, ...
//
// Freigabe: der Systemton laeuft ueber dieselbe TCC-Freigabe wie das Bild
// (Bildschirmaufnahme). Eine Mikrofon-Freigabe wird NICHT gebraucht, wir hoeren
// nicht mit, sondern greifen die Ausgabe ab. Dieses Modul fragt deshalb nichts
// ab und fragt nichts nach; main.m prueft die Bildschirmfreigabe bereits beim
// Start.
#ifndef QC_AUDIO_H
#define QC_AUDIO_H

#import <ScreenCaptureKit/ScreenCaptureKit.h>
#include <stddef.h>
#include <stdint.h>

// Traegt Ton in eine bestehende Stromkonfiguration ein: 48000 Hz, stereo,
// eigener Prozess ausgenommen. Aendert sonst nichts an cfg.
void qc_audio_configure(SCStreamConfiguration *cfg);

// Haengt den Tonabgriff als zweite Ausgabe an den laufenden Strom. send_cb wird
// auf einer eigenen seriellen Warteschlange aufgerufen, nie auf der des Bildes,
// und bekommt verschachtelte float32-Werte (frames * channels Werte).
// Der Zeiger ist nur fuer die Dauer des Aufrufs gueltig.
void qc_audio_attach(SCStream *stream, void (*send_cb)(const float *pcm, size_t frames, uint32_t rate, uint8_t channels));

// Was tatsaechlich verschickt wird. Immer 2; alle Ausgabegeraete an diesem Mac
// sind stereo, und Quellen mit anderer Kanalzahl werden auf stereo gebracht.
uint8_t qc_audio_channels(void);

// 48000, solange nichts anderes ankommt. Liefert das System wider Erwarten eine
// andere Rate, steht ab dem ersten Tonpuffer die tatsaechliche hier.
uint32_t qc_audio_rate(void);

#endif // QC_AUDIO_H
