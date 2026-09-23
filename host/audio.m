// QuadChroma Host, Tonspur. Siehe audio.h fuer die Aufrufreihenfolge.
//
// Bauen: es kommt KEIN Framework dazu. Alles, was hier benutzt wird, steckt in
// ScreenCaptureKit und CoreMedia, die der Makefile schon bindet;
// AudioStreamBasicDescription und AudioBufferList sind reine Strukturen aus
// CoreAudioTypes und brauchen nichts zum Binden. Im Makefile muss nur die
// Quelldatei dazu:
//     SRC := host/main.m host/audio.m
//
// Warum kein AVAudioConverter, kein AudioToolbox: ScreenCaptureKit liefert
// bereits float32 in der angeforderten Rate. Verschachteln ist eine Schleife,
// dafuer lohnt keine Abhaengigkeit.
#import "audio.h"

#import <Foundation/Foundation.h>
#import <CoreMedia/CoreMedia.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

// ------------------------------------------------------------------ Eckdaten

#define QC_AUDIO_RATE      48000
#define QC_AUDIO_CHANNELS  2

// Laenge eines Stille-Stuecks. Eine Luecke wird in solchen Haeppchen gefuellt,
// damit die einzelne Nachricht so gross bleibt wie ein normaler Tonpuffer.
#define QC_SILENCE_FRAMES  2048

// Laenger als das ist keine Luecke mehr, sondern eine Pause (Strom gestoppt,
// Rechner geschlafen). Dann wird nur neu eingerastet, nicht aufgefuellt.
#define QC_GAP_MAX_SEC     5.0

typedef void (*qc_send_fn)(const float *pcm, size_t frames, uint32_t rate, uint8_t channels);

static _Atomic qc_send_fn g_send = (qc_send_fn)0;   // NULL waere hier ein void*
static _Atomic uint32_t   g_rate = QC_AUDIO_RATE;

// ------------------------------------------------------------------- Logging
//
// logf_ in main.m ist static und main.m wird nicht angefasst, also eine eigene
// kleine Ausgabe in dieselbe Datei. Alle Meldungen hier kommen hoechstens
// einmal pro Sitzung, das Oeffnen je Meldung faellt nicht ins Gewicht, und
// "a" haengt atomar an, es kann sich also nichts mit main.m verschraenken.
static void qc_alog(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    const char *c = s.UTF8String;
    fprintf(stdout, "%s\n", c);
    fflush(stdout);
    FILE *f = fopen("/tmp/quadchroma-m1.log", "a");
    if (f) { fprintf(f, "%s\n", c); fclose(f); }
}

// --------------------------------------------------------- Wiederverwendbares
//
// Alles hier unten wird ausschliesslich auf der seriellen Ton-Warteschlange
// angefasst, deshalb ohne Sperren. Gewachsen wird nur, wenn ein Puffer groesser
// ausfaellt als alle vorherigen; im Dauerbetrieb gibt es keine Zuweisung mehr.

static float  *g_pcm = NULL;       // verschachteltes Ziel
static size_t  g_pcm_cap = 0;      // Fassungsvermoegen in Einzelwerten

static AudioBufferList *g_abl = NULL;
static size_t           g_abl_size = 0;

// Stille kommt aus einem festen Nur-Lese-Block: nichts zu fuellen, nichts zu
// verwechseln mit den echten Daten im wachsenden Puffer.
static const float g_silence[QC_SILENCE_FRAMES * QC_AUDIO_CHANNELS] = {0};

static float *pcm_buffer(size_t values) {
    if (values <= g_pcm_cap) return g_pcm;
    size_t want = g_pcm_cap ? g_pcm_cap : 4096;
    while (want < values) want *= 2;
    float *p = realloc(g_pcm, want * sizeof(float));
    if (!p) return NULL;                 // altes g_pcm bleibt gueltig, wir lassen den Puffer aus
    g_pcm = p;
    g_pcm_cap = want;
    return g_pcm;
}

static AudioBufferList *abl_buffer(size_t bytes) {
    if (bytes <= g_abl_size) return g_abl;
    void *p = realloc(g_abl, bytes);
    if (!p) return NULL;
    g_abl = (AudioBufferList *)p;
    g_abl_size = bytes;
    return g_abl;
}

// ---------------------------------------------------------- Luecken erkennen

// Erwarteter Zeitstempel des naechsten Puffers: Zeitstempel des letzten plus
// dessen Dauer. Weicht der naechste davon nach oben ab, fehlt Ton.
static CMTime g_expect;
static BOOL   g_have_expect = NO;

static void emit_silence(size_t frames, uint32_t rate) {
    qc_send_fn send = atomic_load(&g_send);
    if (!send) return;
    while (frames > 0) {
        size_t n = frames > QC_SILENCE_FRAMES ? (size_t)QC_SILENCE_FRAMES : frames;
        send(g_silence, n, rate, QC_AUDIO_CHANNELS);
        frames -= n;
    }
}

// Der Client spielt den Ton fortlaufend ab. Faellt beim Aufnehmen eine Strecke
// aus und wir verschicken sie einfach nicht, laeuft seine Wiedergabe dauerhaft
// um diese Strecke hinter dem Bild her: aufgeholt wird nie, weil die Kurve nur
// nach hinten wandert. Deshalb wird die Luecke mit Stille derselben Laenge
// gefuellt - einmal knacken statt fuer immer versetzt.
static void bridge_gap(CMTime pts, size_t frames, uint32_t rate) {
    if (!CMTIME_IS_VALID(pts)) { g_have_expect = NO; return; }
    if (!g_have_expect || !CMTIME_IS_VALID(g_expect)) return;

    double gap = CMTimeGetSeconds(CMTimeSubtract(pts, g_expect));
    double one = rate ? (double)frames / (double)rate : 0.0;
    if (gap > one && gap <= QC_GAP_MAX_SEC) {
        size_t n = (size_t)(gap * (double)rate + 0.5);
        qc_alog(@"Tonluecke %.0f ms, mit Stille aufgefuellt", gap * 1000.0);
        emit_silence(n, rate);
    }
    // gap <= one: alles in Ordnung. gap < 0: Puffer ueberlappen sich, da ist
    // nichts zu fuellen. gap > QC_GAP_MAX_SEC: echte Pause, nur neu einrasten.
}

// ------------------------------------------------------------- Kanalzugriffe

// Ein Kanal der Quelle: Startzeiger, Schrittweite in Einzelwerten und wie viele
// Rahmen wirklich dahinter liegen. Damit ist verschachtelt und planar dieselbe
// Schleife.
typedef struct {
    const float *p;
    uint32_t     stride;
    size_t       frames;
} ton_kanal;

// ------------------------------------------------------------------- Abgriff

@interface QCAudioTap : NSObject <SCStreamOutput>
@end

@implementation QCAudioTap

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    (void)stream;
    if (type != SCStreamOutputTypeAudio || !sb) return;
    qc_send_fn send = atomic_load(&g_send);
    if (!send) return;
    if (!CMSampleBufferDataIsReady(sb)) return;

    CMFormatDescriptionRef fd = CMSampleBufferGetFormatDescription(sb);
    if (!fd || CMFormatDescriptionGetMediaType(fd) != kCMMediaType_Audio) return;
    const AudioStreamBasicDescription *asbd =
        CMAudioFormatDescriptionGetStreamBasicDescription((CMAudioFormatDescriptionRef)fd);
    if (!asbd) return;

    // Erwartet wird float32. Alles andere waere Rauschen auf der Leitung,
    // deshalb lieber gar nichts senden und einmal Bescheid sagen.
    if (asbd->mFormatID != kAudioFormatLinearPCM ||
        !(asbd->mFormatFlags & kAudioFormatFlagIsFloat) ||
        asbd->mBitsPerChannel != 32) {
        static BOOL warned = NO;
        if (!warned) { warned = YES; qc_alog(@"Ton kommt nicht als float32 - Tonspur bleibt stumm"); }
        return;
    }

    uint32_t rate = (uint32_t)(asbd->mSampleRate + 0.5);
    if (rate == 0) return;
    if (rate != atomic_load(&g_rate)) {
        atomic_store(&g_rate, rate);
        g_have_expect = NO;
        // Die Formatansage an den Zuschauer wiederholt main.m (audio_cb),
        // sobald die Rate im Rueckruf eine andere ist.
        qc_alog(@"Ton laeuft mit %u Hz statt %d Hz - das Tonformat wird neu angesagt",
                rate, QC_AUDIO_RATE);
    }

    size_t frames = (size_t)CMSampleBufferGetNumSamples(sb);
    if (frames == 0) return;

    // Erst die Groesse der Pufferliste erfragen, dann mit eigenem Speicher
    // abholen. Der zurueckgegebene Blockpuffer haelt die Daten am Leben und
    // muss von Hand freigegeben werden - unter ARC fasst niemand CF-Typen an.
    size_t need = 0;
    if (CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sb, &need, NULL, 0, NULL, NULL, 0, NULL) != noErr ||
        need < sizeof(AudioBufferList)) return;
    AudioBufferList *abl = abl_buffer(need);
    if (!abl) return;

    CMBlockBufferRef bb = NULL;
    OSStatus st = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sb, NULL, abl, need, NULL, NULL,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, &bb);
    if (st != noErr || abl->mNumberBuffers == 0) {
        if (bb) CFRelease(bb);
        return;
    }

    BOOL planar = (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
    ton_kanal ch[QC_AUDIO_CHANNELS];
    BOOL ok = YES;

    if (planar) {
        // Ein Puffer je Kanal. Gibt es nur einen, liegt Mono an und beide
        // Ausgabekanaele kommen aus demselben Puffer.
        for (int c = 0; c < QC_AUDIO_CHANNELS; c++) {
            uint32_t bi = (abl->mNumberBuffers > (uint32_t)c) ? (uint32_t)c : 0;
            const AudioBuffer *b = &abl->mBuffers[bi];
            uint32_t stride = b->mNumberChannels ? b->mNumberChannels : 1;
            if (!b->mData) { ok = NO; break; }
            ch[c].p = (const float *)b->mData;
            ch[c].stride = stride;
            ch[c].frames = b->mDataByteSize / (sizeof(float) * stride);
        }
    } else {
        // Ein Puffer, Werte abwechselnd. Mehr als zwei Kanaele kaeme an diesem
        // Mac nicht vor; falls doch, nehmen wir die ersten beiden.
        const AudioBuffer *b = &abl->mBuffers[0];
        uint32_t stride = b->mNumberChannels ? b->mNumberChannels : 1;
        if (!b->mData) ok = NO;
        else {
            const float *base = (const float *)b->mData;
            size_t n = b->mDataByteSize / (sizeof(float) * stride);
            for (int c = 0; c < QC_AUDIO_CHANNELS; c++) {
                uint32_t off = (stride > (uint32_t)c) ? (uint32_t)c : 0;
                ch[c].p = base + off;
                ch[c].stride = stride;
                ch[c].frames = n;
            }
        }
    }

    if (ok) {
        for (int c = 0; c < QC_AUDIO_CHANNELS; c++)
            if (ch[c].frames < frames) frames = ch[c].frames;
    }

    if (ok && frames > 0) {
        float *dst = pcm_buffer(frames * QC_AUDIO_CHANNELS);
        if (!dst) {
            static BOOL warned = NO;
            if (!warned) { warned = YES; qc_alog(@"Kein Speicher fuer den Tonpuffer - Puffer wird ausgelassen"); }
            ok = NO;
        } else {
            CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
            bridge_gap(pts, frames, rate);
            for (size_t i = 0; i < frames; i++) {
                dst[i * QC_AUDIO_CHANNELS + 0] = ch[0].p[i * ch[0].stride];
                dst[i * QC_AUDIO_CHANNELS + 1] = ch[1].p[i * ch[1].stride];
            }
            send(dst, frames, rate, QC_AUDIO_CHANNELS);

            CMTime dur = CMSampleBufferGetDuration(sb);
            if (!CMTIME_IS_VALID(dur) || CMTimeGetSeconds(dur) <= 0)
                dur = CMTimeMake((int64_t)frames, (int32_t)rate);
            if (CMTIME_IS_VALID(pts)) {
                g_expect = CMTimeAdd(pts, dur);
                g_have_expect = YES;
            } else {
                g_have_expect = NO;
            }
        }
    }

    if (bb) CFRelease(bb);
}

@end

// -------------------------------------------------------------- Schnittstelle

// SCStream haelt die Ausgabe nicht fest, also halten wir sie hier. Dasselbe
// gilt fuer die Warteschlange.
static QCAudioTap    *g_tap = nil;
static dispatch_queue_t g_queue = nil;

void qc_audio_configure(SCStreamConfiguration *cfg) {
    if (!cfg) return;
    cfg.capturesAudio = YES;
    cfg.sampleRate = QC_AUDIO_RATE;
    cfg.channelCount = QC_AUDIO_CHANNELS;
    // Ohne das hoerte sich der Host selbst zu, sobald er je etwas ausgibt.
    cfg.excludesCurrentProcessAudio = YES;
    // Freigabe: nur Bildschirmaufnahme, die main.m schon prueft. Kein Mikrofon,
    // kein zusaetzlicher Dialog - deshalb fragt dieses Modul nichts nach.
    // UNGEPRUEFT: die Eigenschaften stimmen gegen die Dokumentation ab macOS
    // 13; ob macOS 27 daran etwas geaendert hat, konnte ich hier nicht pruefen.
}

void qc_audio_attach(SCStream *stream, void (*send_cb)(const float *pcm, size_t frames, uint32_t rate, uint8_t channels)) {
    if (!stream || !send_cb) return;
    atomic_store(&g_send, (qc_send_fn)send_cb);
    if (g_tap) {
        // g_expect gehoert der Ton-Warteschlange. Ein spaeter Rueckruf des
        // alten Stroms kann dort noch laufen, deshalb wird auch das Vergessen
        // dort eingereiht - vor jedem Puffer des neuen Stroms, der erst nach
        // startCapture kommt.
        dispatch_async(g_queue, ^{
            g_expect = kCMTimeInvalid;
            g_have_expect = NO;
        });
        // Ein neuer Strom nach dem Leerlauf (kein Zuschauer - kein Strom):
        // der Abgriff bleibt derselbe, aber der neue Strom kennt ihn nicht.
        // Ohne diese Anmeldung gab es Ton nur in der ersten Sitzung nach
        // dem Start des Hosts - so geschehen am 21.09.
        NSError *err = nil;
        if (![stream addStreamOutput:g_tap type:SCStreamOutputTypeAudio sampleHandlerQueue:g_queue error:&err]) {
            qc_alog(@"Ton konnte am neuen Strom nicht angemeldet werden: %@", err.localizedDescription);
            atomic_store(&g_send, (qc_send_fn)0);
            return;
        }
        qc_alog(@"Ton am neuen Strom angemeldet");
        return;
    }

    // Eigene serielle Warteschlange: der Ton wartet nie hinter einem Vollbild.
    // Hoch eingestufte Guete, damit er unter Last nicht liegen bleibt.
    // Noch kein Abgriff, also auch noch kein Rueckruf: direkt setzen.
    g_expect = kCMTimeInvalid;
    g_have_expect = NO;
    dispatch_queue_attr_t attr = dispatch_queue_attr_make_with_qos_class(
        DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0);
    g_queue = dispatch_queue_create("tech.quadchroma.audio", attr);
    g_tap = [[QCAudioTap alloc] init];

    NSError *err = nil;
    if (![stream addStreamOutput:g_tap type:SCStreamOutputTypeAudio sampleHandlerQueue:g_queue error:&err]) {
        qc_alog(@"Ton konnte nicht angemeldet werden: %@", err.localizedDescription);
        atomic_store(&g_send, (qc_send_fn)0);
        g_tap = nil;
        g_queue = nil;
        return;
    }
    qc_alog(@"Ton angemeldet: %u Hz, %d Kanaele, float32 verschachtelt",
            atomic_load(&g_rate), QC_AUDIO_CHANNELS);
}

uint8_t qc_audio_channels(void) {
    return QC_AUDIO_CHANNELS;
}

uint32_t qc_audio_rate(void) {
    return atomic_load(&g_rate);
}
