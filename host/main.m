// QuadChroma Host.
//   M1: Bildschirm ohne Mauszeiger aufnehmen, in Hardware als HEVC 4:4:4 codieren.
//   M2: den Strom ueber TCP ausliefern statt in eine Datei zu schreiben.
//
// Aufrufe:
//   quadchroma-host --list
//   quadchroma-host --capture <sekunden> <datei.hevc> [optionen]
//   quadchroma-host --serve [port]                    [optionen]
//
// Optionen: --display N  --out BxH  --fps N  --mbit N  --fest
//
// Der Codec laesst sich im Betrieb wechseln (Nachricht 66 vom Client); der
// Start erfolgt immer mit Kandidat 0, HEVC 4:4:4 10 Bit.
//
// Ueber "open -n QuadChroma.app --args ..." starten, damit die Freigaben am
// Bundle haengen. Ausgaben zusaetzlich in /tmp/quadchroma-m1.log.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreGraphics/CoreGraphics.h>
#include <dlfcn.h>
#include <pthread.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <errno.h>
#include <stdatomic.h>
#include <ifaddrs.h>
#include <net/if.h>
#import "audio.h"
#import "clipboard.h"
#include "qc_secure.h"
#import "last.h"
#import <IOKit/pwr_mgt/IOPMLib.h>

// ------------------------------------------------------------------ Logging

static FILE *g_log = NULL;

static void logf_(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    NSString *s = [[NSString alloc] initWithFormat:fmt arguments:ap];
    va_end(ap);
    const char *c = s.UTF8String;
    fprintf(stdout, "%s\n", c);
    fflush(stdout);
    if (g_log) { fprintf(g_log, "%s\n", c); fflush(g_log); }
}

// ------------------------------------------------- Undokumentierte Profile

static CFStringRef vt_sym(const char *name) {
    CFStringRef *p = (CFStringRef *)dlsym(RTLD_DEFAULT, name);
    return p ? *p : NULL;
}

static BOOL profile_supported(VTCompressionSessionRef s, CFStringRef profile) {
    if (!profile) return NO;
    CFDictionaryRef d = NULL;
    if (VTSessionCopySupportedPropertyDictionary(s, &d) != noErr || !d) return NO;
    BOOL ok = NO;
    CFDictionaryRef info = CFDictionaryGetValue(d, kVTCompressionPropertyKey_ProfileLevel);
    if (info && CFGetTypeID(info) == CFDictionaryGetTypeID()) {
        CFArrayRef list = CFDictionaryGetValue(info, kVTPropertySupportedValueListKey);
        if (list) {
            for (CFIndex i = 0; i < CFArrayGetCount(list); i++) {
                CFStringRef v = CFArrayGetValueAtIndex(list, i);
                if (v && CFStringCompare(v, profile, 0) == kCFCompareEqualTo) { ok = YES; break; }
            }
        }
    }
    CFRelease(d);
    return ok;
}

// --------------------------------------------------------------- Netzwerk

// Ein Zuschauer zur Zeit. Mehrere Sitzungen kommen spaeter, wenn das Protokoll steht.
#define QC_BACKLOG_LIMIT (2 * 1024 * 1024)   // ungesendete Bytes, ab denen wir Bilder verwerfen

static _Atomic int g_client_fd = -1;
static _Atomic int g_force_key = 0;
static _Atomic int g_wait_key = 0;   // frisch verbundener Zuschauer wartet auf ein Vollbild
static _Atomic long g_sent_frames = 0, g_skipped_backlog = 0;
static pthread_mutex_t g_send_mtx = PTHREAD_MUTEX_INITIALIZER;
static _Atomic long long g_sent_bytes = 0;

// --- Sicherheitsschicht -------------------------------------------------
// Jeder Kanal bekommt seinen eigenen Handschlag. Der Eingabekanal bindet sich
// ueber die Pruefsumme an den Bildkanal, damit er allein nichts wert ist.
static uint8_t g_id_priv[32], g_id_pub[32];
static qc_chan *g_vid = NULL;               // durch g_send_mtx geschuetzt
static _Atomic int g_vid_ready = 0;
static uint8_t g_vid_hh[QC_HASHLEN];        // Pruefsumme des Bildkanals
static uint8_t g_vid_peer[32];              // Schluessel des verbundenen Clients
static _Atomic int g_pair_open = 0;         // Kopplungsfenster offen?
static char g_last_sas[8] = {0};

// Was sich im Betrieb verstellen laesst. Der Client schickt Wuensche, der Host
// setzt sie um und meldet zurueck, was wirklich gilt.
extern VTCompressionSessionRef g_session;
static void set_i32(VTCompressionSessionRef s, CFStringRef key, int32_t v);

static SCStream *g_stream = nil;
static SCStreamConfiguration *g_cfg = nil;
static _Atomic int g_cur_mbit = 0, g_cur_fps = 0, g_cur_gaming = 0, g_cur_fixed = 0;

// Feste Bildrate: normalerweise liefert die Aufnahme nur dann ein Bild, wenn
// sich etwas geaendert hat. Mit diesem Schalter legt der Host das zuletzt
// gesehene Bild im Takt noch einmal nach, sodass die Rate steht, auch wenn der
// Bildschirm still ist. Alles hier unten wird ausschliesslich auf der
// Aufnahmewarteschlange angefasst - deshalb braucht es keine Sperre.
static CVPixelBufferRef g_last_pb = NULL;
static CMTime g_last_pts;
static uint64_t g_last_cap_us = 0;   // echte Aufnahmezeit des zuletzt gesehenen Bildes
static _Atomic long g_repeats = 0;
static dispatch_queue_t g_capq = NULL;
static dispatch_source_t g_tick = NULL;

static const char *QC_PRO_VIDEO = "QuadChroma/1 video Noise_XX_25519_ChaChaPoly_SHA256";
static const char *QC_PRO_INPUT = "QuadChroma/1 input Noise_XX_25519_ChaChaPoly_SHA256";

// --- Protokoll ---------------------------------------------------------
// Beim Verbinden: 4 Byte Kennung "QCH1".
// Danach Nachrichten: u8 Typ, u8 Flags, u16 frei, u32 Laenge (little endian).
//   Typ 1 = Strominfo: u16 Breite, u16 Hoehe, u16 FPS, u8 Codec, u8 Profil, u8 Bereich, u8 frei
//   Typ 2 = Bilddaten (Annex-B), Flag Bit 0 = Vollbild
#define QC_MAGIC      "QCH1"
#define QC_MSG_INFO   1
#define QC_MSG_VIDEO  2
#define QC_FLAG_KEY   0x01

typedef struct __attribute__((packed)) {
    uint8_t type, flags;
    uint16_t reserved;
    uint32_t len;
} qc_hdr;

typedef struct {
    const char *name;
    CMVideoCodecType codec;
    const char *profil;
    int zehn_bit;
    int chroma444;
} qc_codec_kandidat;

static const qc_codec_kandidat g_kandidaten[] = {
    { "HEVC 4:4:4 10 Bit", kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main44410_AutoLevel", 1, 1 },
    { "HEVC 4:4:4 8 Bit",  kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main444_AutoLevel",   0, 1 },
    { "HEVC 4:2:0 10 Bit", kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main10_AutoLevel",    1, 0 },
    { "HEVC 4:2:0 8 Bit",  kCMVideoCodecType_HEVC, "kVTProfileLevel_HEVC_Main_AutoLevel",      0, 0 },
    { "H.264 High",        kCMVideoCodecType_H264, "kVTProfileLevel_H264_High_AutoLevel",      0, 0 },
    { "AV1",               'av01',                 "kVTProfileLevel_AV1_Main_AutoLevel",       0, 0 },
};

#define QC_KANDIDATEN (sizeof g_kandidaten / sizeof g_kandidaten[0])

typedef struct {
    int vorhanden;
    int hardware;
} qc_codec_befund;

static qc_codec_befund g_befund[QC_KANDIDATEN];

// --- Codecwahl ------------------------------------------------------------
// Welcher Kandidat gerade codiert. Alles, was der Client ueber den Strom
// wissen muss, leitet sich hieraus ab - nicht aus dem Zustand beim Start.
static _Atomic int g_codec_id = 0;

// --- Aufnahme wiederherstellen ----------------------------------------
// Faellt der Bildschirm weg (Monitor aus, Displayschlaf, Neuerkennung),
// beendet ScreenCaptureKit die Aufnahme. Ein Fernsteuerungs-Host muss das
// ueberleben: er wartet, bis wieder ein Bildschirm da ist, und baut die
// Aufnahme dann selbst neu auf. Der Client erfaehrt derweil, woran es liegt.
static id g_grab = nil;                       // der Empfaenger der Aufnahme, wird wiederverwendet
static int g_display_idx = 0;
static _Atomic int g_fixed_gewollt = 0;       // was der Benutzer will, unabhaengig vom Ausfall
static int g_kein_bildschirm_gemeldet = 0;
static IOPMAssertionID g_wach = kIOPMNullAssertionID;
static void aufnahme_wiederherstellen(void);
static void hoststatus_senden(uint8_t lage);
// Kein Zuschauer, keine Arbeit: Aufnahme und Encoder leben nur, solange
// jemand verbunden ist. Auf- und Abbau laufen streng nacheinander auf einer
// eigenen Warteschlange, damit ein gehender und ein kommender Zuschauer sich
// nicht ins Gehege kommen.
static dispatch_queue_t g_lifeq = NULL;
static void stream_herunterfahren_anstossen(void);
static BOOL stream_hochfahren_sync(void);

// Aufnahmeformat je Kandidat. ScreenCaptureKit kennt kein 4:4:4 mit 8 Bit,
// also muss VideoToolbox dort umrechnen - und das steht dann dran, denn eine
// stille Umrechnung verfaelscht genau den Vergleich, um den es geht.
static OSType pixfmt_fuer(int idx) {
    switch (idx) {
        case 0: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44, ohne Kopie
        case 1: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44 -> 8 Bit, Umrechnung
        case 2: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange; // xf44 -> 4:2:0, Umrechnung (Vollbereich bleibt)
        case 3: return kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;  // 420f, ohne Kopie
        case 4: return kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;  // 420f, ohne Kopie
        default: return kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    }
}
static int umrechnung_fuer(int idx) { return idx == 1 || idx == 2; }
static int ist_h264(int idx) { return g_kandidaten[idx].codec == kCMVideoCodecType_H264; }

static int g_info_w = 0, g_info_h = 0, g_info_fps = 0;

// Codecwechsel im Betrieb. Laeuft ausschliesslich auf der Aufnahmewarteschlange;
// die Eingabe reicht den Wunsch nur dorthin weiter.
static void codec_wechseln(int idx);

// Alle Zeiten des Hosts kommen von derselben Uhr wie die Bildzeitstempel der
// Aufnahme. Nur so lassen sich Aufnahme, Encoder und Versand vergleichen.
static uint64_t now_us(void) {
    CMTime t = CMClockGetTime(CMClockGetHostTimeClock());
    if (!CMTIME_IS_VALID(t)) return 0;
    CMTime us = CMTimeConvertScale(t, 1000000, kCMTimeRoundingMethod_Default);
    return (uint64_t)us.value;
}

static uint64_t cmtime_us(CMTime t) {
    if (!CMTIME_IS_VALID(t)) return 0;
    CMTime us = CMTimeConvertScale(t, 1000000, kCMTimeRoundingMethod_Default);
    return (uint64_t)us.value;
}

static _Atomic unsigned g_seq = 0;
static _Atomic int g_formattest = 0;
// Mittlere Encoderzeit je Bild in Mikrosekunden. Die Video-Einheit meldet ihre
// Auslastung nirgends; das hier ist die Groesse, die statt dessen etwas sagt.
static _Atomic long long g_enc_us = 0;
static _Atomic long g_enc_n = 0;

static void tune_socket(int fd) {
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    int snd = 1 << 21;                       // 2 MB Sendepuffer
    setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &snd, sizeof snd);
    int lowat = 64 * 1024;                   // nicht mehr als das im Kernel stauen lassen
    setsockopt(fd, IPPROTO_TCP, TCP_NOTSENT_LOWAT, &lowat, sizeof lowat);
}

static int backlog_bytes(int fd) {
    int n = 0; socklen_t len = sizeof n;
    if (getsockopt(fd, SOL_SOCKET, SO_NWRITE, &n, &len) != 0) return 0;
    return n;
}


// Kleine Nachricht ueber die Bildverbindung. Umgeht bewusst die Vollbild-Sperre
// und die Stauregel: Ton und Zwischenablage sind winzig und duerfen nicht warten.
static void send_small(uint8_t type, const void *data, size_t len) {
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (fd >= 0 && atomic_load(&g_vid_ready)) {
        qc_hdr h = { .type = type, .flags = 0, .reserved = 0, .len = (uint32_t)len };
        struct iovec iov[2];
        iov[0].iov_base = &h;   iov[0].iov_len = sizeof h;
        iov[1].iov_base = (void *)data; iov[1].iov_len = len;
        if (qc_chan_send(g_vid, iov, len ? 2 : 1) != 0) {
            atomic_store(&g_vid_ready, 0);
            atomic_store(&g_client_fd, -1);
            close(fd);
            stream_herunterfahren_anstossen();
        }
    }
    pthread_mutex_unlock(&g_send_mtx);
}

#define QC_MSG_SETTINGS   3    // Host -> Client: was gerade gilt
#define QC_MSG_TIME       4    // Host -> Client: Antwort auf den Zeitabgleich
#define QC_MSG_STAMP      5    // Host -> Client: Zeitstempel zum naechsten Bild
#define QC_MSG_LAST       6    // Host -> Client: Auslastung des Hosts
#define QC_MSG_SWITCH     7    // Host -> Client: ab hier neuer Codec, Decoder zuruecksetzen
#define QC_MSG_CODECS     8    // Host -> Client: was dieser Mac codieren kann
#define QC_IN_CODEC       66   // Client -> Host: Codecwunsch (u8 Index)
#define QC_MSG_HOSTSTATUS 9    // Host -> Client: u8 Lage (0 = in Ordnung, 1 = kein Bildschirm)
#define QC_IN_TIME        65   // Client -> Host: Frage zum Zeitabgleich
#define QC_IN_SETTINGS    64   // Client -> Host: was gewuenscht wird
static void hoststatus_senden(uint8_t lage) {
    uint8_t b[2] = { lage, 0 };
    send_small(QC_MSG_HOSTSTATUS, b, sizeof b);
}

#define QC_MSG_AUDIO_INFO 32
#define QC_MSG_AUDIO      33
#define QC_MSG_CLIP       48

static _Atomic int g_audio_info_sent = 0;
static _Atomic long g_audio_packets = 0;
static _Atomic long long g_audio_bytes = 0;

static void audio_cb(const float *pcm, size_t frames, uint32_t rate, uint8_t channels) {
    if (atomic_load(&g_client_fd) < 0) return;
    if (!atomic_exchange(&g_audio_info_sent, 1)) {
        uint8_t info[8] = {0};
        memcpy(info, &rate, 4);
        info[4] = channels;
        info[5] = 1;                 // 1 = float32, verschachtelt
        send_small(QC_MSG_AUDIO_INFO, info, sizeof info);
    }
    size_t bytes = frames * channels * sizeof(float);
    atomic_fetch_add(&g_audio_packets, 1);
    atomic_fetch_add(&g_audio_bytes, (long long)bytes);
    send_small(QC_MSG_AUDIO, pcm, bytes);
}

static void clip_cb(const char *utf8, size_t len) {
    send_small(QC_MSG_CLIP, utf8, len);
}

// Eckdaten des Stroms, immer aus dem AKTUELLEN Codec abgeleitet.
static void strominfo_fuellen(uint8_t p[8]) {
    int idx = atomic_load(&g_codec_id);
    uint16_t w16 = (uint16_t)g_info_w, h16 = (uint16_t)g_info_h, f16 = (uint16_t)g_info_fps;
    memcpy(p + 0, &w16, 2); memcpy(p + 2, &h16, 2); memcpy(p + 4, &f16, 2);
    p[6] = ist_h264(idx) ? 2 : 1;                 // 1 = HEVC, 2 = H.264
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    // 1 = 4:4:4 8 Bit, 2 = 4:4:4 10 Bit, 3 = 4:2:0 8 Bit, 4 = 4:2:0 10 Bit; alles Vollbereich
    p[7] = k->chroma444 ? (k->zehn_bit ? 2 : 1) : (k->zehn_bit ? 4 : 3);
}

// Koennensliste: was dieser Mac wirklich codiert, mit Hardware- und
// Umrechnungsflagge. Der Client zeigt nur an, was hier drinsteht.
static void codecs_senden(void) {
    uint8_t buf[512];
    size_t n = 0;
    buf[n++] = (uint8_t)QC_KANDIDATEN;
    for (size_t i = 0; i < QC_KANDIDATEN && n + 8 + 32 < sizeof buf; i++) {
        const qc_codec_kandidat *k = &g_kandidaten[i];
        size_t nl = strlen(k->name);
        if (nl > 31) nl = 31;
        buf[n++] = (uint8_t)i;
        buf[n++] = (uint8_t)g_befund[i].vorhanden;
        buf[n++] = (uint8_t)g_befund[i].hardware;
        buf[n++] = (uint8_t)umrechnung_fuer((int)i);
        buf[n++] = (uint8_t)k->chroma444;
        buf[n++] = (uint8_t)k->zehn_bit;
        buf[n++] = (uint8_t)nl;
        memcpy(buf + n, k->name, nl);
        n += nl;
    }
    send_small(QC_MSG_CODECS, buf, n);
}

static void *accept_thread(void *arg) {
    int listen_fd = (int)(intptr_t)arg;
    for (;;) {
        struct sockaddr_in peer; socklen_t plen = sizeof peer;
        int fd = accept(listen_fd, (struct sockaddr *)&peer, &plen);
        if (fd < 0) { if (errno == EINTR) continue; break; }
        tune_socket(fd);
        char ip[INET_ADDRSTRLEN] = {0};
        inet_ntop(AF_INET, &peer.sin_addr, ip, sizeof ip);

        // Zuerst der Handschlag. Vor ihm geht kein einziges Byte Nutzlast raus.
        qc_chan *chan = malloc(sizeof *chan);
        if (!chan) { close(fd); continue; }
        int hr = qc_chan_accept(chan, fd, g_id_priv,
                                (const uint8_t *)QC_PRO_VIDEO, strlen(QC_PRO_VIDEO));
        if (hr != 0) {
            logf_(@"Handschlag mit %s gescheitert (%d)", ip, hr);
            free(chan);
            close(fd);
            continue;
        }

        char fp[24], sas[8];
        qc_fingerprint(chan->peer, fp);
        qc_sas(chan->hh, sas);

        // Freigabe: bekannte Gegenstelle, oder das Kopplungsfenster steht offen.
        BOOL known = qc_is_authorized(chan->peer) ? YES : NO;
        if (!known) {
            if (atomic_load(&g_pair_open) || qc_authorized_count() == 0) {
                qc_authorize(chan->peer, ip);
                logf_(@"Neue Gegenstelle gekoppelt: %s (%s), Vergleichscode %s", fp, ip, sas);
                atomic_store(&g_pair_open, 0);
            } else {
                logf_(@"Abgewiesen: unbekannte Gegenstelle %s von %s. Host mit --pair starten, um sie aufzunehmen.", fp, ip);
                free(chan);
                close(fd);
                continue;
            }
        }

        // Kein Zuschauer, keine Arbeit: Aufnahme und Encoder entstehen erst
        // jetzt. Schlaegt das fehl, gibt es auch keine Begruessung.
        if (!stream_hochfahren_sync()) {
            logf_(@"Aufnahme laesst sich nicht starten - Zuschauer %s abgewiesen", ip);
            free(chan);
            close(fd);
            continue;
        }

        // Begruessung: Kennung und Eckdaten des Stroms, damit der Empfaenger
        // Fenstergroesse und Format kennt, bevor das erste Bild kommt.
        uint8_t hello[4 + sizeof(qc_hdr) + 8];
        memcpy(hello, QC_MAGIC, 4);
        qc_hdr h = { .type = QC_MSG_INFO, .flags = 0, .reserved = 0, .len = 8 };
        memcpy(hello + 4, &h, sizeof h);
        strominfo_fuellen(hello + 4 + sizeof h);

        pthread_mutex_lock(&g_send_mtx);
        int old = atomic_exchange(&g_client_fd, -1);
        atomic_store(&g_vid_ready, 0);
        if (old >= 0 && old != fd) close(old);
        free(g_vid);
        g_vid = chan;
        memcpy(g_vid_hh, chan->hh, QC_HASHLEN);
        memcpy(g_vid_peer, chan->peer, 32);
        memcpy(g_last_sas, sas, sizeof g_last_sas);
        struct iovec iov = { .iov_base = hello, .iov_len = sizeof hello };
        int sent = qc_chan_send(g_vid, &iov, 1);
        if (sent == 0) {
            atomic_store(&g_client_fd, fd);
            atomic_store(&g_vid_ready, 1);
        }
        pthread_mutex_unlock(&g_send_mtx);
        if (sent != 0) { close(fd); continue; }

        atomic_store(&g_force_key, 1);
        atomic_store(&g_wait_key, 1);
        atomic_store(&g_audio_info_sent, 0);
        {
            uint8_t cur[8] = {0};
            uint32_t m = (uint32_t)atomic_load(&g_cur_mbit);
            uint16_t f = (uint16_t)atomic_load(&g_cur_fps);
            memcpy(cur, &m, 4); memcpy(cur + 4, &f, 2);
            cur[6] = (uint8_t)atomic_load(&g_cur_gaming);
            cur[7] = (uint8_t)atomic_load(&g_cur_fixed);
            send_small(QC_MSG_SETTINGS, cur, sizeof cur);
        }
        codecs_senden();
        logf_(@"Zuschauer verbunden: %s:%d, verschluesselt, Gegenstelle %s, Vergleichscode %s",
              ip, ntohs(peer.sin_port), fp, sas);
    }
    return NULL;
}

static int start_server(int port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    a.sin_port = htons((uint16_t)port);
    if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0) { logf_(@"bind fehlgeschlagen: %s", strerror(errno)); close(fd); return -1; }
    if (listen(fd, 4) != 0) { logf_(@"listen fehlgeschlagen: %s", strerror(errno)); close(fd); return -1; }
    pthread_t t;
    pthread_create(&t, NULL, accept_thread, (void *)(intptr_t)fd);
    pthread_detach(t);
    return fd;
}

// ------------------------------------------------------------ Bekanntgabe
// Der Host ruft sich alle zwei Sekunden im lokalen Netz aus, damit Clients ihn
// ohne eingetippte Adresse finden. Winziges Paket, kein Dienst, keine Abhaengigkeit.
//
// Aufbau: "QCHB" | u8 Version | u16 Bildport | u8 Namenslaenge | Name (UTF-8)

static void *beacon_thread(void *arg) {
    int port = (int)(intptr_t)arg;
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) return NULL;
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &on, sizeof on);

    char name[64] = {0};
    gethostname(name, sizeof name - 1);
    size_t nlen = strlen(name);
    if (nlen > 40) nlen = 40;

    uint8_t pkt[64];
    memcpy(pkt, "QCHB", 4);
    pkt[4] = 1;
    uint16_t p16 = (uint16_t)port;
    memcpy(pkt + 5, &p16, 2);
    pkt[7] = (uint8_t)nlen;
    memcpy(pkt + 8, name, nlen);
    size_t len = 8 + nlen;

    // An die Rundrufadresse JEDER aktiven Netzwerkkarte senden. Die allgemeine
    // 255.255.255.255 verlaesst den Mac nicht zuverlaessig, die Netzadresse
    // (zum Beispiel 192.168.178.255) dagegen schon.
    for (;;) {
        struct ifaddrs *list = NULL;
        int gesendet = 0;
        if (getifaddrs(&list) == 0) {
            for (struct ifaddrs *ia = list; ia; ia = ia->ifa_next) {
                if (!ia->ifa_addr || ia->ifa_addr->sa_family != AF_INET) continue;
                if (!(ia->ifa_flags & IFF_UP) || !(ia->ifa_flags & IFF_BROADCAST)) continue;
                if (ia->ifa_flags & IFF_LOOPBACK) continue;
                struct sockaddr_in *b = (struct sockaddr_in *)ia->ifa_dstaddr;
                if (!b) continue;
                struct sockaddr_in to = *b;
                to.sin_family = AF_INET;
                to.sin_port = htons((uint16_t)(port + 2));
                if (sendto(fd, pkt, len, 0, (struct sockaddr *)&to, sizeof to) > 0) gesendet++;
            }
            freeifaddrs(list);
        }
        static int erste = 1;
        if (erste) { logf_(@"Bekanntgabe: an %d Netze, Port %d", gesendet, port + 2); erste = 0; }
        usleep(2000 * 1000);
    }
    return NULL;
}

static void start_beacon(int port) {
    pthread_t t;
    pthread_create(&t, NULL, beacon_thread, (void *)(intptr_t)port);
    pthread_detach(t);
}

// ------------------------------------------------------------ Eingabe-Teil
// Zweite Verbindung, nur fuer Maus und Tastatur. Getrennt vom Bild, damit eine
// Mausbewegung nie hinter einem Vollbild in der Warteschlange haengt.
//
// Nachrichten vom Client (gleicher 8-Byte-Kopf):
//   16 MAUS_BEWEGUNG : f32 x, f32 y            (0..1, Anteil der Bildbreite/-hoehe)
//   17 MAUS_TASTE    : u8 taste, u8 gedrueckt, u16 frei, f32 x, f32 y
//   18 RAD           : f32 dx, f32 dy          (Pixel)
#define QC_IN_MOVE    16
#define QC_IN_BUTTON  17
#define QC_IN_SCROLL  18
#define QC_IN_KEY     19

static CGEventSourceRef g_evsrc = NULL;
static CGEventFlags g_mods = 0;
static CGPoint g_pos = {0, 0};
static int g_buttons = 0;                 // Bitmaske der gedrueckten Tasten
static CGDirectDisplayID g_input_display = 0;
static _Atomic long g_input_events = 0;

static CGPoint to_display_point(float nx, float ny) {
    CGRect b = CGDisplayBounds(g_input_display);
    if (nx < 0) nx = 0; if (nx > 1) nx = 1;
    if (ny < 0) ny = 0; if (ny > 1) ny = 1;
    return CGPointMake(b.origin.x + nx * b.size.width, b.origin.y + ny * b.size.height);
}

static void post(CGEventType type, CGMouseButton btn) {
    CGEventRef e = CGEventCreateMouseEvent(g_evsrc, type, g_pos, btn);
    if (!e) return;
    if (g_mods) CGEventSetFlags(e, g_mods);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

static void inject_move(float nx, float ny) {
    g_pos = to_display_point(nx, ny);
    CGEventType t = kCGEventMouseMoved;
    CGMouseButton b = kCGMouseButtonLeft;
    if (g_buttons & 1) { t = kCGEventLeftMouseDragged; b = kCGMouseButtonLeft; }
    else if (g_buttons & 2) { t = kCGEventRightMouseDragged; b = kCGMouseButtonRight; }
    else if (g_buttons & 4) { t = kCGEventOtherMouseDragged; b = kCGMouseButtonCenter; }
    post(t, b);
}

static void inject_button(int button, int down, float nx, float ny) {
    g_pos = to_display_point(nx, ny);
    CGEventType t;
    CGMouseButton b;
    switch (button) {
        case 1: b = kCGMouseButtonRight;  t = down ? kCGEventRightMouseDown : kCGEventRightMouseUp; break;
        case 2: b = kCGMouseButtonCenter; t = down ? kCGEventOtherMouseDown : kCGEventOtherMouseUp; break;
        default: b = kCGMouseButtonLeft;  t = down ? kCGEventLeftMouseDown  : kCGEventLeftMouseUp;  break;
    }
    if (down) g_buttons |= (1 << (button == 1 ? 1 : button == 2 ? 2 : 0));
    else      g_buttons &= ~(1 << (button == 1 ? 1 : button == 2 ? 2 : 0));

    CGEventRef e = CGEventCreateMouseEvent(g_evsrc, t, g_pos, b);
    if (!e) return;
    // Doppelklicks: macOS erwartet den Zaehler im Ereignis selbst.
    static double last_t = 0; static int clicks = 0; static CGPoint last_p = {0, 0};
    if (down) {
        double now = CFAbsoluteTimeGetCurrent();
        double dist = fabs(now - last_t);
        if (dist < 0.4 && fabs(last_p.x - g_pos.x) < 5 && fabs(last_p.y - g_pos.y) < 5) clicks++;
        else clicks = 1;
        last_t = now; last_p = g_pos;
    }
    CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks < 1 ? 1 : clicks);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

static void inject_scroll(float dx, float dy) {
    CGEventRef e = CGEventCreateScrollWheelEvent2(g_evsrc, kCGScrollEventUnitPixel, 2,
                                                  (int32_t)dy, (int32_t)dx, 0);
    if (!e) return;
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);
    atomic_fetch_add(&g_input_events, 1);
}

// Welche Tasten wir selbst gedrueckt haben. Reisst die Verbindung ab, geben
// wir sie frei - sonst haelt der Mac sie fuer immer gedrueckt, und ein
// haengendes Strg macht aus jedem weiteren Klick einen Rechtsklick.
static uint8_t g_key_down[256] = {0};
static pthread_mutex_t g_key_mtx = PTHREAD_MUTEX_INITIALIZER;

static void inject_key(uint16_t keycode, int down, uint32_t mods) {
    // Umschalter aus der Client-Sicht uebernehmen: so sieht der Mac genau den
    // Zustand, den der Benutzer an seiner Tastatur haelt.
    CGEventFlags f = 0;
    if (mods & 1) f |= kCGEventFlagMaskShift;
    if (mods & 2) f |= kCGEventFlagMaskControl;
    if (mods & 4) f |= kCGEventFlagMaskAlternate;
    if (mods & 8) f |= kCGEventFlagMaskCommand;
    g_mods = f;

    CGEventRef e = CGEventCreateKeyboardEvent(g_evsrc, (CGKeyCode)keycode, down ? true : false);
    if (!e) return;
    CGEventSetFlags(e, f);
    CGEventPost(kCGHIDEventTap, e);
    CFRelease(e);

    // Mitschreiben, was gerade gedrueckt ist - das ist die Grundlage fuer das
    // Freigeben, wenn die Verbindung wegbricht.
    if (keycode < 256) {
        pthread_mutex_lock(&g_key_mtx);
        g_key_down[keycode] = down ? 1 : 0;
        pthread_mutex_unlock(&g_key_mtx);
    }
    atomic_fetch_add(&g_input_events, 1);
}


// Einstellungen im laufenden Betrieb. Bitrate und Bildrate gehen ohne
// Unterbrechung; der Gaming-Schalter zieht die Zuegel straffer: haeufigere
// Vollbilder und ein kleinerer erlaubter Stau, damit nichts auflaeuft.
static void apply_settings(int mbit, int fps, int gaming, int fixed) {
    if (mbit < 2) mbit = 2;
    if (mbit > 500) mbit = 500;
    if (fps < 10) fps = 10;
    if (fps > 240) fps = 240;

    // Encoder-Sitzung und Aufnahmekonfiguration gehoeren der Aufnahmewarteschlange:
    // dort tauscht der Codecwechsel die Sitzung aus, also darf sie nur dort
    // angefasst werden - sonst setzen wir hier Eigenschaften auf einer Sitzung,
    // die gerade freigegeben wird.
    dispatch_async(g_capq ?: dispatch_get_main_queue(), ^{
        if (g_session) {
            set_i32(g_session, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
            set_i32(g_session, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
            set_i32(g_session, kVTCompressionPropertyKey_MaxKeyFrameInterval, gaming ? fps : fps * 2);
        }
        if (g_stream && g_cfg) {
            g_cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
            [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
                if (e) logf_(@"Bildrate konnte nicht geaendert werden: %@", e.localizedDescription);
            }];
        }
    });
    atomic_store(&g_cur_mbit, mbit);
    atomic_store(&g_cur_fps, fps);
    atomic_store(&g_cur_gaming, gaming);
    atomic_store(&g_cur_fixed, fixed);
    atomic_store(&g_fixed_gewollt, fixed);
    atomic_store(&g_force_key, 1);

    // Der Takt fuer die feste Bildrate haengt an der eingestellten Rate.
    if (g_tick) {
        uint64_t iv = (uint64_t)(NSEC_PER_SEC / (uint64_t)fps);
        dispatch_source_set_timer(g_tick, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv),
                                  iv, iv / 10);
    }

    uint8_t out[8] = {0};
    uint32_t m = (uint32_t)mbit; uint16_t f = (uint16_t)fps;
    memcpy(out, &m, 4); memcpy(out + 4, &f, 2);
    out[6] = (uint8_t)gaming;
    out[7] = (uint8_t)fixed;
    send_small(QC_MSG_SETTINGS, out, sizeof out);
    logf_(@"Einstellungen: %d Mbit/s, %d fps, Gaming %@, feste Bildrate %@",
          mbit, fps, gaming ? @"an" : @"aus", fixed ? @"an" : @"aus");
}

// Alles loslassen, was noch als gedrueckt vermerkt ist.
static void alle_tasten_loslassen(void) {
    pthread_mutex_lock(&g_key_mtx);
    int offen = 0;
    for (int k = 0; k < 256; k++) {
        if (g_key_down[k]) {
            g_key_down[k] = 0;
            offen++;
            CGEventRef e = CGEventCreateKeyboardEvent(g_evsrc, (CGKeyCode)k, false);
            if (e) {
                CGEventSetFlags(e, 0);
                CGEventPost(kCGHIDEventTap, e);
                CFRelease(e);
            }
        }
    }
    g_mods = 0;
    pthread_mutex_unlock(&g_key_mtx);
    if (offen) logf_(@"Verbindung weg: %d haengende Taste(n) freigegeben", offen);
}

static void *input_thread(void *arg) {
    int listen_fd = (int)(intptr_t)arg;
    uint8_t payload[256];
    for (;;) {
        int fd = accept(listen_fd, NULL, NULL);
        if (fd < 0) { if (errno == EINTR) continue; break; }
        int one = 1;
        setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);

        // Der Eingabekanal darf erst aufmachen, wenn der Bildkanal steht: sein
        // Prologue enthaelt dessen Pruefsumme. Wer die nicht kennt, kommt hier
        // nicht durch - damit kann niemand nur die Tastatur uebernehmen.
        if (!atomic_load(&g_vid_ready)) {
            logf_(@"Eingabekanal abgewiesen: kein Bildkanal offen");
            close(fd);
            continue;
        }
        uint8_t pro[256];
        size_t prolen = strlen(QC_PRO_INPUT);
        memcpy(pro, QC_PRO_INPUT, prolen);
        pthread_mutex_lock(&g_send_mtx);
        memcpy(pro + prolen, g_vid_hh, QC_HASHLEN);
        uint8_t expect[32];
        memcpy(expect, g_vid_peer, 32);
        pthread_mutex_unlock(&g_send_mtx);
        prolen += QC_HASHLEN;

        qc_chan *in = malloc(sizeof *in);
        if (!in) { close(fd); continue; }
        int hr = qc_chan_accept(in, fd, g_id_priv, pro, prolen);
        if (hr != 0) {
            logf_(@"Eingabekanal: Handschlag gescheitert (%d)", hr);
            free(in);
            close(fd);
            continue;
        }
        if (memcmp(in->peer, expect, 32) != 0) {
            logf_(@"Eingabekanal abgewiesen: andere Gegenstelle als beim Bild");
            free(in);
            close(fd);
            continue;
        }
        logf_(@"Eingabekanal verbunden, verschluesselt");

        for (;;) {
            qc_hdr h;
            if (qc_chan_read(in, &h, sizeof h) != 0) break;
            if (h.type == QC_MSG_CLIP) {
                // Text kann gross sein, deshalb hier ein eigener Puffer.
                if (h.len > 4 * 1024 * 1024) break;
                char *big = h.len ? malloc(h.len + 1) : NULL;
                if (h.len && (!big || qc_chan_read(in, big, h.len) != 0)) { free(big); break; }
                if (big) { big[h.len] = 0; qc_clip_set(big, h.len); free(big); }
                continue;
            }
            if (h.len > sizeof payload) break;
            if (h.len && qc_chan_read(in, payload, h.len) != 0) break;
            float f[2];
            switch (h.type) {
                case QC_IN_MOVE:
                    if (h.len >= 8) { memcpy(f, payload, 8); inject_move(f[0], f[1]); }
                    break;
                case QC_IN_BUTTON:
                    if (h.len >= 12) { memcpy(f, payload + 4, 8); inject_button(payload[0], payload[1], f[0], f[1]); }
                    break;
                case QC_IN_SCROLL:
                    if (h.len >= 8) { memcpy(f, payload, 8); inject_scroll(f[0], f[1]); }
                    break;
                case QC_IN_KEY:
                    if (h.len >= 8) {
                        uint16_t kc; uint32_t mods;
                        memcpy(&kc, payload, 2);
                        memcpy(&mods, payload + 4, 4);
                        inject_key(kc, payload[2], mods);
                    }
                    break;
                case QC_IN_TIME:
                    // Zeitabgleich: die Frage traegt die Uhrzeit des Clients,
                    // die Antwort gibt sie zurueck und haengt die des Hosts an.
                    // Der Client rechnet daraus Versatz und Umlaufzeit aus.
                    if (h.len >= 8) {
                        uint64_t t_client;
                        memcpy(&t_client, payload, 8);
                        uint8_t out[16];
                        uint64_t t_host = now_us();
                        memcpy(out, &t_client, 8);
                        memcpy(out + 8, &t_host, 8);
                        send_small(QC_MSG_TIME, out, sizeof out);
                    }
                    break;
                case QC_IN_SETTINGS:
                    if (h.len >= 8) {
                        uint32_t m; uint16_t f;
                        memcpy(&m, payload, 4);
                        memcpy(&f, payload + 4, 2);
                        int g = payload[6] ? 1 : 0;
                        int fx = payload[7] ? 1 : 0;
                        dispatch_async(dispatch_get_main_queue(), ^{
                            apply_settings((int)m, (int)f, g, fx);
                        });
                    }
                    break;
                case QC_IN_CODEC:
                    // Codecwunsch. Nur weiterreichen - der Wechsel selbst
                    // gehoert auf die Aufnahmewarteschlange, nie hierher.
                    if (h.len >= 1) {
                        int idx = payload[0];
                        if (g_capq) dispatch_async(g_capq, ^{ codec_wechseln(idx); });
                        else logf_(@"Codecwunsch %d verworfen: Aufnahme laeuft noch nicht", idx);
                    }
                    break;
                default: break;
            }
        }
        logf_(@"Eingabekanal getrennt");
        alle_tasten_loslassen();
        free(in);
        close(fd);
    }
    return NULL;
}

static int start_input_server(int port, CGDirectDisplayID display) {
    g_input_display = display;
    g_evsrc = CGEventSourceCreate(kCGEventSourceStateCombinedSessionState);
    if (g_evsrc) CGEventSourceSetLocalEventsSuppressionInterval(g_evsrc, 0.0);

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    a.sin_port = htons((uint16_t)port);
    if (bind(fd, (struct sockaddr *)&a, sizeof a) != 0 || listen(fd, 4) != 0) {
        logf_(@"Eingabe-Port %d nicht verfuegbar: %s", port, strerror(errno));
        close(fd);
        return -1;
    }
    pthread_t t;
    pthread_create(&t, NULL, input_thread, (void *)(intptr_t)fd);
    pthread_detach(t);
    return fd;
}

// ------------------------------------------------------------- Encoder-Teil

typedef struct {
    FILE *out;
    int nal_len;
    BOOL wrote_ps;
    long encoded;
    long dropped;
    long long bytes;
    OSStatus first_err;
} EncStats;

static EncStats g_stats = {0};
VTCompressionSessionRef g_session = NULL;
// Encoder-Fehler je Sitzung. Wird in encoder_start auf 0 gesetzt; gemeldet wird
// nur der ERSTE Fehler einer Sitzung, sonst kaeme je Bild eine Zeile. Atomar,
// weil encode_buffer (g_capq) und enc_cb (VideoToolbox-Thread) beide zaehlen.
static _Atomic int g_enc_fehler = 0;

static const uint8_t kStartCode[4] = {0, 0, 0, 1};

// Sammelt Parametersaetze und Bilddaten in eine Zugriffseinheit und schickt sie
// in EINEM Aufruf weg: weniger Systemaufrufe, keine halben Bilder auf der Leitung.
#define QC_MAX_IOV 256

static void emit_access_unit(CMSampleBufferRef sb, BOOL keyframe, uint64_t t_cap_us, int wiederholt) {
    struct iovec iov[QC_MAX_IOV];

    // Vor jedem Bild geht ein Zeitstempel raus: wann es aufgenommen wurde und
    // wann der Encoder fertig war. Beides auf der Uhr des Hosts. Der Client
    // rechnet das mit seinem Zeitabgleich in echte Millisekunden um. Stempel
    // und Bild gehen in einem Rutsch raus, damit sie nicht auseinanderfallen.
    uint8_t stamp[24];
    uint32_t seq = atomic_fetch_add(&g_seq, 1);
    // Die Aufnahmezeit kommt vom Begleitzettel des Bildes, nicht aus dem
    // Zeitstempel des Encoders: der ist bei nachgelegten Bildern die
    // Nachlegezeit und wuerde die Messung schoenen.
    uint64_t t_cap = t_cap_us ? t_cap_us : cmtime_us(CMSampleBufferGetPresentationTimeStamp(sb));
    uint64_t t_enc = now_us();
    if (!wiederholt && t_cap && t_enc > t_cap) {
        atomic_fetch_add(&g_enc_us, (long long)(t_enc - t_cap));
        atomic_fetch_add(&g_enc_n, 1);
    }
    memcpy(stamp + 0, &seq, 4);
    memset(stamp + 4, 0, 4);
    stamp[4] = (uint8_t)(wiederholt ? 1 : 0);
    memcpy(stamp + 8, &t_cap, 8);
    memcpy(stamp + 16, &t_enc, 8);
    qc_hdr shdr = { .type = QC_MSG_STAMP, .flags = 0, .reserved = 0, .len = sizeof stamp };

    qc_hdr hdr = { .type = QC_MSG_VIDEO, .flags = (uint8_t)(keyframe ? QC_FLAG_KEY : 0),
                   .reserved = (uint16_t)(seq & 0xffff), .len = 0 };
    iov[0].iov_base = &shdr; iov[0].iov_len = sizeof shdr;
    iov[1].iov_base = stamp; iov[1].iov_len = sizeof stamp;
    int cnt = 3;                 // iov[2] bleibt fuer den Bildkopf reserviert
    iov[2].iov_base = &hdr;
    iov[2].iov_len = sizeof hdr;
    size_t total = 0;

    if (keyframe) {
        // Parametersaetze (VPS/SPS/PPS bei HEVC, SPS/PPS bei H.264) vor jedes
        // Vollbild. Welche Abfrage gilt, entscheidet der laufende Codec.
        CMFormatDescriptionRef fd = CMSampleBufferGetFormatDescription(sb);
        size_t count = 0; int nal = 4;
        BOOL h264 = ist_h264(atomic_load(&g_codec_id)) ? YES : NO;
        OSStatus ps_st = !fd ? -1
                       : h264 ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, 0, NULL, NULL, &count, &nal)
                              : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, 0, NULL, NULL, &count, &nal);
        if (ps_st == noErr) {
            g_stats.nal_len = nal;
            for (size_t i = 0; i < count && cnt + 2 < QC_MAX_IOV; i++) {
                const uint8_t *ps = NULL; size_t psz = 0;
                OSStatus r = h264 ? CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, i, &ps, &psz, NULL, NULL)
                                  : CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fd, i, &ps, &psz, NULL, NULL);
                if (r == noErr && ps && psz) {
                    iov[cnt].iov_base = (void *)kStartCode; iov[cnt].iov_len = 4; cnt++;
                    iov[cnt].iov_base = (void *)ps;         iov[cnt].iov_len = psz; cnt++;
                    total += 4 + psz;
                }
            }
        }
    }

    CMBlockBufferRef bb = CMSampleBufferGetDataBuffer(sb);
    size_t len = 0; char *data = NULL;
    if (!bb || CMBlockBufferGetDataPointer(bb, 0, NULL, &len, &data) != noErr) return;
    int nl = g_stats.nal_len > 0 ? g_stats.nal_len : 4;
    size_t off = 0;
    while (off + (size_t)nl <= len && cnt + 2 < QC_MAX_IOV) {
        uint32_t n = 0;
        for (int i = 0; i < nl; i++) n = (n << 8) | (uint8_t)data[off + i];
        if (off + nl + n > len) break;
        iov[cnt].iov_base = (void *)kStartCode;        iov[cnt].iov_len = 4; cnt++;
        iov[cnt].iov_base = (void *)(data + off + nl); iov[cnt].iov_len = n; cnt++;
        total += 4 + n;
        off += (size_t)nl + n;
    }
    if (cnt <= 3) return;
    hdr.len = (uint32_t)total;

    // In die Datei geht der reine Bitstrom ohne unsere Koepfe, damit man sie
    // direkt abspielen kann.
    if (g_stats.out) {
        for (int i = 3; i < cnt; i++) fwrite(iov[i].iov_base, 1, iov[i].iov_len, g_stats.out);
        g_stats.bytes += total;
    }

    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_load(&g_client_fd);
    if (fd >= 0) {
        // Ein frisch verbundener Zuschauer bekommt erst ab dem naechsten Vollbild
        // Daten, sonst faengt er mitten in einem Bild ohne Kopfdaten an.
        if (!atomic_load(&g_vid_ready)) { pthread_mutex_unlock(&g_send_mtx); return; }
        if (atomic_load(&g_wait_key) && !keyframe) { pthread_mutex_unlock(&g_send_mtx); return; }
        // Staut es sich, werfen wir lieber ein Bild weg als Verzoegerung aufzubauen.
        // g_wait_key bleibt dabei stehen: faellt hier das erste Vollbild einer
        // neuen Sitzung weg, muss der Zuschauer weiter auf ein Vollbild warten -
        // sonst bekaeme sein Decoder ein Zwischenbild ohne Bezugsbild.
        if (backlog_bytes(fd) > (atomic_load(&g_cur_gaming) ? QC_BACKLOG_LIMIT / 4 : QC_BACKLOG_LIMIT)) {
            atomic_fetch_add(&g_skipped_backlog, 1);
            atomic_store(&g_force_key, 1);
            pthread_mutex_unlock(&g_send_mtx);
            return;
        }
        // Erst wenn das Bild wirklich rausgeht, ist das Warten vorbei.
        atomic_store(&g_wait_key, 0);
        if (qc_chan_send(g_vid, iov, cnt) != 0) {
            logf_(@"Zuschauer weg: %s", strerror(errno));
            atomic_store(&g_vid_ready, 0);
            atomic_store(&g_client_fd, -1);
            stream_herunterfahren_anstossen();
            close(fd);
            pthread_mutex_unlock(&g_send_mtx);
            return;
        }
        atomic_fetch_add(&g_sent_frames, 1);
        atomic_fetch_add(&g_sent_bytes, (long long)total);
    }
    pthread_mutex_unlock(&g_send_mtx);
}

static void enc_cb(void *ref, void *src, OSStatus status, VTEncodeInfoFlags flags, CMSampleBufferRef sb) {
    (void)ref;
    uint64_t zettel = (uint64_t)(uintptr_t)src;
    uint64_t t_cap_us = zettel >> 1;
    int wiederholt = (int)(zettel & 1u);
    if (flags & kVTEncodeInfo_FrameDropped) g_stats.dropped++;
    if (status != noErr) {
        if (!g_stats.first_err) g_stats.first_err = status;
        // Im Dienstbetrieb sieht sonst niemand, dass der Encoder nach einem
        // Wechsel nichts mehr liefert. Einmal je Sitzung, nicht je Bild.
        if (atomic_fetch_add(&g_enc_fehler, 1) == 0)
            logf_(@"Encoder %s meldet Fehler beim Codieren (%d)", g_kandidaten[atomic_load(&g_codec_id)].name, (int)status);
        return;
    }
    if (!sb) return;

    CFArrayRef att = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    BOOL keyframe = YES;
    if (att && CFArrayGetCount(att) > 0) {
        CFDictionaryRef d = CFArrayGetValueAtIndex(att, 0);
        keyframe = !CFDictionaryContainsKey(d, kCMSampleAttachmentKey_NotSync);
    }
    if (!g_stats.wrote_ps) { keyframe = YES; g_stats.wrote_ps = YES; }

    emit_access_unit(sb, keyframe, t_cap_us, wiederholt);
    g_stats.encoded++;
}

static void set_i32(VTCompressionSessionRef s, CFStringRef key, int32_t v) {
    CFNumberRef n = CFNumberCreate(NULL, kCFNumberSInt32Type, &v);
    VTSessionSetProperty(s, key, n);
    CFRelease(n);
}

// ---------------------------------------------------------- Was kann dieser Mac?
//
// Nicht raten, sondern fragen: fuer jeden Kandidaten wird eine Sitzung
// aufgemacht und der Encoder selbst befragt, ob er das Profil anbietet - einmal
// mit der Forderung nach Hardware, einmal ohne. Was hier nicht auftaucht, kann
// die Maschine nicht, egal was in den Kopfdateien steht.



// Einen Kandidaten pruefen. hw_pflicht = 1 verlangt einen Hardware-Encoder.
static int kandidat_pruefen(const qc_codec_kandidat *k, int hw_pflicht, int *ist_hw) {
    CFMutableDictionaryRef spec = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (hw_pflicht)
        CFDictionarySetValue(spec, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);

    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, 1920, 1080, k->codec, spec, NULL, NULL, NULL, NULL, &s);
    CFRelease(spec);
    if (st != noErr || !s) return 0;

    int ok = 0;
    CFStringRef profil = vt_sym(k->profil);
    if (profil && profile_supported(s, profil)) {
        // Anbieten allein reicht nicht - es muss sich auch setzen lassen.
        ok = (VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, profil) == noErr);
    }
    if (ok && ist_hw) {
        CFBooleanRef hw = NULL;
        if (VTSessionCopyProperty(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, NULL, &hw) == noErr && hw) {
            *ist_hw = CFBooleanGetValue(hw);
            CFRelease(hw);
        }
    }
    VTCompressionSessionInvalidate(s);
    CFRelease(s);
    return ok;
}

static void codecs_pruefen(void) {
    logf_(@"--- Was dieser Mac codieren kann ---");
    for (size_t i = 0; i < QC_KANDIDATEN; i++) {
        const qc_codec_kandidat *k = &g_kandidaten[i];
        int hw = 0;
        int da = kandidat_pruefen(k, 1, &hw);   // erst mit Hardware-Pflicht
        if (da) {
            g_befund[i].vorhanden = 1;
            g_befund[i].hardware = hw;
        } else {
            hw = 0;
            da = kandidat_pruefen(k, 0, &hw);   // dann ohne
            g_befund[i].vorhanden = da;
            g_befund[i].hardware = da ? hw : 0;
        }
        logf_(@"  %-18s %@%@", k->name,
              g_befund[i].vorhanden ? @"ja " : @"nein",
              g_befund[i].vorhanden ? (g_befund[i].hardware ? @"(Hardware)" : @"(Software)") : @"");
    }
}

// Encoder fuer einen Kandidaten aus g_kandidaten oeffnen. Codec, Profil und
// Aufnahmeformat kommen aus der Tabelle; das Profil muss der Encoder selbst
// anbieten, sonst brechen wir ab statt still etwas anderes zu liefern.
// g_session wird erst gesetzt, wenn die Sitzung vollstaendig steht - bei einem
// Fehlschlag bleibt sie so, wie sie war (beim Codecwechsel: NULL).
static BOOL encoder_start(int idx, int w, int h, int fps, int mbit) {
    if (idx < 0 || idx >= (int)QC_KANDIDATEN) return NO;
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    OSType pixfmt = pixfmt_fuer(idx);

    CFMutableDictionaryRef spec = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    // Hardware verlangen, wenn die Pruefung beim Start welche gefunden hat. Sonst
    // ist der Kandidat nur in Software da - und genau so steht er in der Koennensliste.
    if (g_befund[idx].hardware)
        CFDictionarySetValue(spec, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue);

    VTCompressionSessionRef s = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, w, h, k->codec, spec, NULL, NULL, enc_cb, NULL, &s);
    CFRelease(spec);
    if (st != noErr || !s) { logf_(@"Encoder-Session fuer %s fehlgeschlagen (%d)", k->name, (int)st); return NO; }

    CFStringRef want = vt_sym(k->profil);
    if (!profile_supported(s, want)) {
        logf_(@"Profil %s wird von diesem Encoder nicht angeboten - Abbruch statt stiller Ersatzausgabe", k->profil);
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
        return NO;
    }
    st = VTSessionSetProperty(s, kVTCompressionPropertyKey_ProfileLevel, want);
    if (st != noErr) {
        logf_(@"Profil %s abgelehnt (%d)", k->profil, (int)st);
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
        return NO;
    }

    VTSessionSetProperty(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_AllowOpenGOP, kCFBooleanFalse);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_ColorPrimaries,   kCVImageBufferColorPrimaries_ITU_R_709_2);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
    VTSessionSetProperty(s, kVTCompressionPropertyKey_YCbCrMatrix,      kCVImageBufferYCbCrMatrix_ITU_R_709_2);
    set_i32(s, kVTCompressionPropertyKey_MaxFrameDelayCount, 1);
    set_i32(s, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
    set_i32(s, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
    set_i32(s, kVTCompressionPropertyKey_MaxKeyFrameInterval, fps * 2);
    VTCompressionSessionPrepareToEncodeFrames(s);

    CFBooleanRef hw = NULL;
    VTSessionCopyProperty(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, NULL, &hw);
    BOOL is_hw = hw && CFBooleanGetValue(hw);
    if (hw) CFRelease(hw);

    atomic_store(&g_enc_fehler, 0);   // neue Sitzung, neue Meldung erlaubt
    g_session = s;
    logf_(@"Encoder: Kandidat %d %s, %dx%d, Hardware: %@, %d Mbit/s, Eingabe %.4s%@",
          idx, k->name, w, h, is_hw ? @"ja" : @"NEIN", mbit,
          (char *)&(uint32_t){CFSwapInt32HostToBig(pixfmt)},
          umrechnung_fuer(idx) ? @" (Umrechnung durch VideoToolbox)" : @"");
    return YES;
}

// ------------------------------------------------------------ Codecwechsel
//
// Alles hier laeuft AUSSCHLIESSLICH auf der Aufnahmewarteschlange g_capq -
// derselben, auf der Bilder in den Encoder gehen und der feste Takt nachlegt.
// Deshalb kann waehrend des Wechsels kein Bild in eine halb abgebaute Sitzung
// laufen, und deshalb braucht nichts hier eine Sperre.
//
// Reihenfolge auf der Leitung: letzte Zugriffseinheit des alten Codecs,
// dann SWITCH (Typ 7), dann Strominfo (Typ 1), dann das erste Vollbild des
// neuen Codecs mit Parametersaetzen. SWITCH geht ERST raus, wenn die neue
// Sitzung wirklich steht (Erfolgszweig von codec_wechsel_abschliessen) - so
// baut der Client seinen Decoder genau einmal um, und scheitert der Wechsel,
// bekommt er gar kein SWITCH und behaelt den alten Decoder. Dass SWITCH
// trotzdem genau zwischen alt und neu landet, liegt daran, dass CompleteFrames
// erst zurueckkommt, wenn der alte Encoder alles abgeliefert hat, dass die
// neue Sitzung Bilder nur ueber g_capq bekommt - und codec_wechsel_abschliessen
// laeuft selbst auf g_capq, sendet SWITCH also, bevor dort das naechste Bild
// drankommt. send_small nimmt dieselbe Sperre wie der Bildversand.

static int g_wechsel_aktiv = 0;      // nur auf g_capq: ein Wechsel ist unterwegs
static int g_wechsel_wunsch = -1;    // nur auf g_capq: waehrenddessen eingegangener naechster Wunsch

static void strominfo_senden(void) {
    uint8_t p[8];
    strominfo_fuellen(p);
    send_small(QC_MSG_INFO, p, sizeof p);
}

// Typ 7: u8 idx, u8 h264, u8 chroma444, u8 zehn_bit, u8 vollbereich (immer 1),
// u8 umrechnung, u16 frei.
static void switch_senden(int idx) {
    const qc_codec_kandidat *k = &g_kandidaten[idx];
    uint8_t p[8] = {0};
    p[0] = (uint8_t)idx;
    p[1] = (uint8_t)ist_h264(idx);
    p[2] = (uint8_t)k->chroma444;
    p[3] = (uint8_t)k->zehn_bit;
    p[4] = 1;
    p[5] = (uint8_t)umrechnung_fuer(idx);
    send_small(QC_MSG_SWITCH, p, sizeof p);
}

// Was der Zuschauer gerade eingestellt hat, auf die frische Sitzung uebertragen.
// Dieselben Aufrufe wie in apply_settings, aber ohne deren Meldung und ohne
// Aufnahmekonfiguration - die ist von einem Codecwechsel nicht betroffen.
static void encoder_einstellungen_nachziehen(void) {
    int fps = atomic_load(&g_cur_fps), mbit = atomic_load(&g_cur_mbit), gaming = atomic_load(&g_cur_gaming);
    if (!g_session || fps <= 0 || mbit <= 0) return;
    set_i32(g_session, kVTCompressionPropertyKey_AverageBitRate, mbit * 1000000);
    set_i32(g_session, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
    set_i32(g_session, kVTCompressionPropertyKey_MaxKeyFrameInterval, gaming ? fps : fps * 2);
}

// Wechsel beendet - ob gelungen oder nicht. Kam waehrenddessen ein weiterer
// Wunsch herein, wird er jetzt angestossen, statt verloren zu gehen.
static void codec_wechsel_fertig(void) {
    g_wechsel_aktiv = 0;
    int n = g_wechsel_wunsch;
    g_wechsel_wunsch = -1;
    if (n >= 0) dispatch_async(g_capq, ^{ codec_wechseln(n); });
}

// Schritt h: der neue Kandidat liess sich nicht oeffnen, der alte kommt zurueck.
// Ein SWITCH gab es nicht, der Client decodiert weiter mit dem alten Codec -
// nur die Strominfo geht noch einmal raus.
static void codec_alt_aufbauen(int alt) {
    if (encoder_start(alt, g_info_w, g_info_h, atomic_load(&g_cur_fps), atomic_load(&g_cur_mbit))) {
        encoder_einstellungen_nachziehen();
        atomic_store(&g_force_key, 1);
        strominfo_senden();
        logf_(@"Alter Codec laeuft wieder: %s", g_kandidaten[alt].name);
    } else {
        logf_(@"Auch der alte Codec %s laesst sich nicht mehr oeffnen - es kommt kein Bild mehr", g_kandidaten[alt].name);
    }
    codec_wechsel_fertig();
}

// Schritt g: neue Sitzung bauen, SWITCH ansagen, Einstellungen nachziehen,
// Strominfo melden. Die Reihenfolge ist Absicht: erst g_codec_id, damit
// emit_access_unit die Parametersaetze des neuen Codecs abfragt; dann SWITCH -
// noch kann kein Bild in der neuen Sitzung sein, weil Bilder nur ueber g_capq
// hineingehen und wir gerade darauf laufen; dann Vollbild erzwingen und die
// Strominfo hinterher.
static void codec_wechsel_abschliessen(int idx, int alt, OSType alt_fmt, BOOL fmt_geaendert) {
    if (encoder_start(idx, g_info_w, g_info_h, atomic_load(&g_cur_fps), atomic_load(&g_cur_mbit))) {
        atomic_store(&g_codec_id, idx);
        switch_senden(idx);
        encoder_einstellungen_nachziehen();
        atomic_store(&g_force_key, 1);
        strominfo_senden();
        logf_(@"Codec gewechselt: %s", g_kandidaten[idx].name);
        codec_wechsel_fertig();
        return;
    }

    logf_(@"Codecwechsel auf %s fehlgeschlagen - baue %s wieder auf", g_kandidaten[idx].name, g_kandidaten[alt].name);
    // Kein SWITCH: der Client hat noch seinen alten Decoder und behaelt ihn.
    if (fmt_geaendert) {
        g_cfg.pixelFormat = alt_fmt;
        [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
            if (e) logf_(@"Aufnahmeformat liess sich nicht zuruecksetzen: %@", e.localizedDescription);
            dispatch_async(g_capq, ^{ codec_alt_aufbauen(alt); });
        }];
    } else {
        codec_alt_aufbauen(alt);
    }
}

static void codec_wechseln(int idx) {
    // a) Nur, was die Pruefung beim Start als vorhanden gemeldet hat.
    if (idx < 0 || idx >= (int)QC_KANDIDATEN || !g_befund[idx].vorhanden) {
        logf_(@"Codecwunsch %d abgelehnt: %@", idx,
              (idx < 0 || idx >= (int)QC_KANDIDATEN) ? @"kein solcher Kandidat" : @"auf diesem Mac nicht vorhanden");
        return;
    }
    int alt = atomic_load(&g_codec_id);
    if (g_wechsel_aktiv) {
        // Ein Wechsel laeuft schon (die Aufnahme stellt gerade um). Den Wunsch
        // merken und danach ausfuehren - nie zwei Wechsel ineinander.
        g_wechsel_wunsch = idx;
        logf_(@"Codecwunsch %s vorgemerkt, ein Wechsel laeuft noch", g_kandidaten[idx].name);
        return;
    }
    // "Laeuft bereits" gilt nur, wenn wirklich eine Sitzung laeuft. Nach einem
    // doppelten Fehlschlag (neuer Codec kaputt, alter liess sich nicht wieder
    // oeffnen) steht g_codec_id noch auf dem alten - der darf dann neu versucht werden.
    if (idx == alt && g_session) {
        logf_(@"Codecwunsch %s: laeuft bereits", g_kandidaten[idx].name);
        return;
    }
    // Ohne Aufnahme geht nichts. Eine fehlende Encoder-Sitzung ist dagegen kein
    // Hinderungsgrund: genau dann (nach dem doppelten Fehlschlag) muss der
    // Zuschauer noch auf einen dritten Kandidaten ausweichen koennen.
    if (!g_stream || !g_cfg) {
        logf_(@"Codecwunsch %s abgelehnt: keine laufende Aufnahme", g_kandidaten[idx].name);
        return;
    }
    g_wechsel_aktiv = 1;
    logf_(@"Codecwechsel: %s -> %s", g_kandidaten[alt].name, g_kandidaten[idx].name);

    // b) Aendert sich das Aufnahmeformat, darf der feste Takt kein altes Bild
    //    mehr nachlegen - sonst bekaeme der neue Encoder Bilder im alten Format.
    OSType alt_fmt = g_cfg.pixelFormat;
    OSType neu_fmt = pixfmt_fuer(idx);
    BOOL fmt_geaendert = (alt_fmt != neu_fmt);
    if (fmt_geaendert && g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }

    if (g_session) {
        // c) Alles, was der alte Encoder noch hat, abliefern lassen. Danach feuert
        //    kein enc_cb der alten Sitzung mehr.
        VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);

        // d) Sitzung aushaengen, dann abbauen. Ab hier verwirft encode_buffer
        //    ankommende Bilder, bis die neue Sitzung steht - gewollt.
        VTCompressionSessionRef s = g_session;
        g_session = NULL;
        VTCompressionSessionInvalidate(s);
        CFRelease(s);
    }

    // e) Der Zuschauer wartet wieder auf ein Vollbild, das erste neue Bild wird
    //    erzwungen, und die NAL-Laenge kommt frisch aus den neuen Parametersaetzen.
    //    Das SWITCH (Typ 7) kommt NICHT hier, sondern erst, wenn die neue Sitzung
    //    steht - siehe codec_wechsel_abschliessen.
    atomic_store(&g_wait_key, 1);
    atomic_store(&g_force_key, 1);
    g_stats.nal_len = 4;

    // f) Aufnahmeformat umstellen, falls noetig. Das geht im Betrieb (gemessen
    //    mit --formattest); weiter geht es erst, wenn die Aufnahme umgestellt hat.
    if (fmt_geaendert) {
        uint32_t a = CFSwapInt32HostToBig(alt_fmt), n = CFSwapInt32HostToBig(neu_fmt);
        logf_(@"Aufnahmeformat: %.4s -> %.4s", (char *)&a, (char *)&n);
        g_cfg.pixelFormat = neu_fmt;
        [g_stream updateConfiguration:g_cfg completionHandler:^(NSError *e) {
            if (e) {
                // Die Aufnahme liefert weiter das alte Format - dann bleibt es
                // beim alten Codec. Der Client hat kein SWITCH bekommen und
                // muss von nichts erfahren.
                logf_(@"Aufnahmeformat liess sich nicht umstellen: %@ - bleibe bei %s", e.localizedDescription, g_kandidaten[alt].name);
                dispatch_async(g_capq, ^{
                    g_cfg.pixelFormat = alt_fmt;
                    codec_alt_aufbauen(alt);
                });
                return;
            }
            dispatch_async(g_capq, ^{ codec_wechsel_abschliessen(idx, alt, alt_fmt, fmt_geaendert); });
        }];
    } else {
        codec_wechsel_abschliessen(idx, alt, alt_fmt, fmt_geaendert);
    }
}

// ------------------------------------------------------------ Aufnahme-Teil

@interface Grabber : NSObject <SCStreamOutput, SCStreamDelegate>
@property (nonatomic) long framesIn;
@property (nonatomic) long framesSkipped;
@end

// Ein Bild in den Encoder geben. Beide Wege - frisch aufgenommen und im Takt
// nachgelegt - laufen hier zusammen, damit sie sich nicht auseinander
// entwickeln koennen.
// Der Begleitzettel reist als Zahl mit, nicht als Zeiger: die echte
// Aufnahmezeit in Mikrosekunden, nach links geschoben, im untersten Bit das
// Kennzeichen "nachgelegt". So haengt am Bild kein Speicher, der beim
// Verwerfen oder bei einem Fehler liegen bleiben koennte.
#define QC_ZETTEL(t_cap_us, wiederholt) ((void *)(uintptr_t)(((uint64_t)(t_cap_us) << 1) | ((wiederholt) ? 1u : 0u)))

static void encode_buffer(CVPixelBufferRef pb, CMTime pts, uint64_t t_cap_us, int wiederholt) {
    if (!pb || !g_session) return;
    // Rund um einen Formatwechsel der Aufnahme kann noch ein Bild im alten
    // Format eintreffen. Das gehoert nicht in den neuen Encoder - verwerfen,
    // und sagen, dass es passiert ist (einmal je Format, nicht je Bild).
    OSType ist = CVPixelBufferGetPixelFormatType(pb);
    OSType soll = pixfmt_fuer(atomic_load(&g_codec_id));
    if (ist != soll) {
        static OSType gemeldet = 0;
        if (gemeldet != ist) {
            gemeldet = ist;
            uint32_t a = CFSwapInt32HostToBig(ist), b = CFSwapInt32HostToBig(soll);
            logf_(@"Bild im Format %.4s verworfen, Encoder erwartet %.4s", (char *)&a, (char *)&b);
        }
        return;
    }
    NSDictionary *opts = nil;
    if (atomic_exchange(&g_force_key, 0))
        opts = @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES};
    OSStatus st = VTCompressionSessionEncodeFrame(g_session, pb, pts, kCMTimeInvalid,
                                                  (__bridge CFDictionaryRef)opts,
                                                  QC_ZETTEL(t_cap_us, wiederholt), NULL);
    if (st != noErr) {
        if (!g_stats.first_err) g_stats.first_err = st;
        // Im Dienstbetrieb wuerde ein Encoder, der nach dem Wechsel kein Bild
        // annimmt, sonst stumm bleiben. Einmal je Sitzung, nicht je Bild.
        if (atomic_fetch_add(&g_enc_fehler, 1) == 0)
            logf_(@"Encoder %s nimmt Bild nicht an (%d)", g_kandidaten[atomic_load(&g_codec_id)].name, (int)st);
    }
}

// Der Takt. Laeuft auf derselben Warteschlange wie die Aufnahme, kommt ihr
// also nie in die Quere. Kam seit dem letzten Schlag ein echtes Bild, passiert
// hier nichts - nachgelegt wird nur, was sonst ausfallen wuerde.
static void fixed_tick(void) {
    if (!atomic_load(&g_cur_fixed)) return;
    // Ohne Zuschauer wird nichts nachgelegt. Sonst laeuft der Encoder mit
    // voller Rate fuer niemanden - gemessen: 24 % Last im Leerlauf.
    if (atomic_load(&g_client_fd) < 0) return;
    if (!g_last_pb || !g_session) return;
    int fps = atomic_load(&g_cur_fps);
    if (fps <= 0) return;

    CMTime now = CMClockGetTime(CMClockGetHostTimeClock());
    double seit = CMTimeGetSeconds(CMTimeSubtract(now, g_last_pts));
    if (seit < 0.9 / (double)fps) return;

    // Zeitstempel muss immer vorwaerts gehen, sonst weist der Encoder das Bild ab.
    if (CMTIME_COMPARE_INLINE(now, <=, g_last_pts))
        now = CMTimeAdd(g_last_pts, CMTimeMake(1, (int32_t)fps));
    // Wichtig: der Zettel traegt die ECHTE Aufnahmezeit des wiederholten
    // Bildes, nicht die Nachlegezeit. Sonst sieht die Messung auf der anderen
    // Seite aus, als waere jedes Bild blitzschnell unterwegs gewesen.
    encode_buffer(g_last_pb, now, g_last_cap_us, 1);
    g_last_pts = now;
    atomic_fetch_add(&g_repeats, 1);
}

@implementation Grabber

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    CFArrayRef arr = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (arr && CFArrayGetCount(arr) > 0) {
        CFDictionaryRef d = CFArrayGetValueAtIndex(arr, 0);
        CFNumberRef st = CFDictionaryGetValue(d, (__bridge CFStringRef)SCStreamFrameInfoStatus);
        int v = -1;
        if (st) CFNumberGetValue(st, kCFNumberIntType, &v);
        if (v != SCFrameStatusComplete) { self.framesSkipped++; return; }
    }
    CVImageBufferRef pb = CMSampleBufferGetImageBuffer(sb);
    if (atomic_exchange(&g_formattest, 0)) {
        OSType t = pb ? CVPixelBufferGetPixelFormatType(pb) : 0;
        uint32_t be = CFSwapInt32HostToBig(t);
        logf_(@"   angekommen: %.4s  (%zux%zu)", (char *)&be,
              pb ? CVPixelBufferGetWidth(pb) : 0, pb ? CVPixelBufferGetHeight(pb) : 0);
    }
    if (!pb || !g_session) return;

    self.framesIn++;
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
    if (!CMTIME_IS_VALID(pts)) pts = CMClockGetTime(CMClockGetHostTimeClock());

    // Die echte Aufnahmezeit wird VOR jeder Korrektur festgehalten. Die
    // Korrektur unten dient nur dem Encoder, der aufsteigende Zeitstempel
    // verlangt - sie darf die Messung nicht beschoenigen.
    uint64_t t_cap = cmtime_us(pts);
    if (g_last_pb && CMTIME_COMPARE_INLINE(pts, <=, g_last_pts))
        pts = CMTimeAdd(g_last_pts, CMTimeMake(1, 1000));

    encode_buffer(pb, pts, t_cap, 0);

    // Fuer den festen Takt das Bild festhalten. Der Puffer gehoert der
    // Aufnahme; ohne eigene Referenz wird er unter uns wiederverwendet.
    if (g_last_pb != pb) {
        CVPixelBufferRef alt = g_last_pb;
        g_last_pb = CVPixelBufferRetain(pb);
        if (alt) CVPixelBufferRelease(alt);
    }
    g_last_pts = pts;
    g_last_cap_us = t_cap;
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    logf_(@"Aufnahme gestoppt: %@ (Code %ld)", error.localizedDescription, (long)error.code);
    // Ohne Aufnahme darf der Takt nicht weiterlaufen, sonst sendet der Host
    // bis in alle Ewigkeit dasselbe eingefrorene Bild.
    atomic_store(&g_cur_fixed, 0);
    dispatch_async(g_capq ?: dispatch_get_main_queue(), ^{
        if (g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }
    });
    // Solange keine Aufnahme laeuft, gibt es auch keinen Codecwechsel.
    g_stream = nil;
    // Ohne Zuschauer gibt es nichts wiederherzustellen - der naechste baut neu auf.
    if (atomic_load(&g_client_fd) < 0) return;
    hoststatus_senden(1);
    // Nicht sofort: direkt nach dem Abriss ist der Bildschirm meist noch weg.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        aufnahme_wiederherstellen();
    });
}

@end

// ------------------------------------------------------------------- Helfer

static SCDisplay *pick_display(int idx, size_t *pxW, size_t *pxH, double *hz) {
    __block SCDisplay *display = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
        if (!e && c.displays.count > (NSUInteger)idx) display = c.displays[idx];
        else logf_(@"Bildschirm %d nicht verfuegbar: %@", idx, e.localizedDescription);
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC));
    if (!display) return nil;
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(display.displayID);
    if (pxW) *pxW = CGDisplayModeGetPixelWidth(m);
    if (pxH) *pxH = CGDisplayModeGetPixelHeight(m);
    if (hz) *hz = CGDisplayModeGetRefreshRate(m);
    CGDisplayModeRelease(m);
    return display;
}

static void list_displays(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
        if (e) logf_(@"Inhalte nicht abrufbar: %@", e.localizedDescription);
        int i = 0;
        for (SCDisplay *d in c.displays) {
            CGDisplayModeRef m = CGDisplayCopyDisplayMode(d.displayID);
            size_t pw = m ? CGDisplayModeGetPixelWidth(m) : 0, ph = m ? CGDisplayModeGetPixelHeight(m) : 0;
            double hz = m ? CGDisplayModeGetRefreshRate(m) : 0;
            if (m) CGDisplayModeRelease(m);
            logf_(@"Display %d: id=%u, %ld x %ld Punkte, %zu x %zu Pixel, %.0f Hz%@",
                  i++, d.displayID, (long)d.width, (long)d.height, pw, ph, hz,
                  d.displayID == CGMainDisplayID() ? @"  (Hauptbildschirm)" : @"");
        }
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC));
}

static void aufnahme_wiederherstellen(void) {
    // Nur solange jemand zuschaut. Geht der Zuschauer waehrend des Wartens,
    // endet die Kette hier.
    if (atomic_load(&g_client_fd) < 0) return;
    // Auf der Lebenslauf-Warteschlange: pick_display wartet blockierend auf
    // ScreenCaptureKit, und das darf weder die Aufnahme- noch die Hauptschleife
    // anhalten - und kein Abbau darf dazwischenfunken.
    dispatch_async(g_lifeq ?: dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        if (g_stream) return;
        size_t pw = 0, ph = 0; double hz = 0;
        SCDisplay *d = pick_display(g_display_idx, &pw, &ph, &hz);
        if (!d) {
            if (!g_kein_bildschirm_gemeldet) {
                g_kein_bildschirm_gemeldet = 1;
                logf_(@"Kein Bildschirm - warte auf seine Rueckkehr");
            }
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                aufnahme_wiederherstellen();
            });
            return;
        }
        g_kein_bildschirm_gemeldet = 0;

        SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:d excludingWindows:@[]];
        int fps = atomic_load(&g_cur_fps);
        if (fps > 0) g_cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
        SCStream *st = [[SCStream alloc] initWithFilter:f configuration:g_cfg delegate:g_grab];
        NSError *err = nil;
        if (![st addStreamOutput:g_grab type:SCStreamOutputTypeScreen sampleHandlerQueue:g_capq error:&err]) {
            logf_(@"Aufnahme: Ausgabe nicht anmeldbar (%@), neuer Versuch", err.localizedDescription);
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                aufnahme_wiederherstellen();
            });
            return;
        }
        qc_audio_attach(st, audio_cb);
        [st startCaptureWithCompletionHandler:^(NSError *e) {
            if (e) {
                logf_(@"Aufnahme: Start misslungen (%@, Code %ld), neuer Versuch", e.localizedDescription, (long)e.code);
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                    aufnahme_wiederherstellen();
                });
                return;
            }
            g_stream = st;
            atomic_store(&g_cur_fixed, atomic_load(&g_fixed_gewollt));
            atomic_store(&g_force_key, 1);
            atomic_store(&g_wait_key, 1);
            hoststatus_senden(0);
            logf_(@"Aufnahme wiederhergestellt: Bildschirm %zux%zu, %.0f Hz", pw, ph, hz);
        }];
    });
}

// Aufnahme und Encoder fuer einen Zuschauer aufbauen. Wird vom Annahmefaden
// gerufen, bevor er die Begruessung schickt - die Verbindung dauert dadurch
// einen Moment laenger, dafuer arbeitet der Mac ohne Zuschauer gar nicht.
static BOOL stream_hochfahren_sync(void) {
    __block BOOL ok = NO;
    dispatch_sync(g_lifeq, ^{
        if (g_stream) { ok = YES; return; }
        size_t pw = 0, ph = 0; double hz = 0;
        SCDisplay *d = pick_display(g_display_idx, &pw, &ph, &hz);
        if (!d) { logf_(@"Kein Bildschirm - Aufnahme kann nicht starten"); return; }

        // Encoder zuerst, auf der Aufnahmewarteschlange - wie beim Codecwechsel.
        __block BOOL enc = YES;
        dispatch_sync(g_capq, ^{
            if (!g_session)
                enc = encoder_start(atomic_load(&g_codec_id), g_info_w, g_info_h,
                                    atomic_load(&g_cur_fps), atomic_load(&g_cur_mbit));
        });
        if (!enc) { logf_(@"Encoder laesst sich nicht starten"); return; }

        int fps = atomic_load(&g_cur_fps);
        if (fps > 0) g_cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
        g_cfg.pixelFormat = pixfmt_fuer(atomic_load(&g_codec_id));
        SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:d excludingWindows:@[]];
        SCStream *st = [[SCStream alloc] initWithFilter:f configuration:g_cfg delegate:g_grab];
        NSError *err = nil;
        if (![st addStreamOutput:g_grab type:SCStreamOutputTypeScreen sampleHandlerQueue:g_capq error:&err]) {
            logf_(@"Aufnahme: Ausgabe nicht anmeldbar (%@)", err.localizedDescription);
            return;
        }
        qc_audio_attach(st, audio_cb);
        __block BOOL gestartet = NO;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [st startCaptureWithCompletionHandler:^(NSError *e) {
            if (e) logf_(@"Aufnahme: Start misslungen (%@, Code %ld)", e.localizedDescription, (long)e.code);
            else gestartet = YES;
            dispatch_semaphore_signal(sem);
        }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC));
        if (!gestartet) return;

        g_stream = st;
        atomic_store(&g_cur_fixed, atomic_load(&g_fixed_gewollt));
        atomic_store(&g_force_key, 1);
        atomic_store(&g_wait_key, 1);
        g_stats.nal_len = 4;
        // Solange gestreamt wird, darf der Bildschirm nicht einschlafen.
        if (g_wach == kIOPMNullAssertionID)
            IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
                                        CFSTR("QuadChroma streamt diesen Bildschirm"), &g_wach);
        logf_(@"Aufnahme gestartet: Bildschirm %zux%zu, %.0f Hz", pw, ph, hz);
        ok = YES;
    });
    return ok;
}

// Der Zuschauer ist weg: Aufnahme anhalten, Encoder abbauen, Bildschirm
// freigeben. Darf aus jedem Faden angestossen werden, auch unter g_send_mtx -
// hier wird nur eingereiht, gearbeitet wird spaeter und nacheinander.
static void stream_herunterfahren_anstossen(void) {
    if (!g_lifeq) return;
    dispatch_async(g_lifeq, ^{
        if (atomic_load(&g_client_fd) >= 0) return;   // inzwischen ist wieder jemand da
        if (!g_stream && !g_session) return;
        if (g_stream) {
            SCStream *st = g_stream;
            g_stream = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [st stopCaptureWithCompletionHandler:^(NSError *e) { (void)e; dispatch_semaphore_signal(sem); }];
            dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
        }
        dispatch_sync(g_capq, ^{
            if (g_last_pb) { CVPixelBufferRelease(g_last_pb); g_last_pb = NULL; }
            if (g_session) {
                VTCompressionSessionRef alt = g_session;
                g_session = NULL;
                VTCompressionSessionCompleteFrames(alt, kCMTimeInvalid);
                VTCompressionSessionInvalidate(alt);
                CFRelease(alt);
            }
        });
        atomic_store(&g_cur_fixed, 0);
        if (g_wach != kIOPMNullAssertionID) { IOPMAssertionRelease(g_wach); g_wach = kIOPMNullAssertionID; }
        logf_(@"Aufnahme angehalten: kein Zuschauer");
    });
}

int main(int argc, const char *argv[]) { @autoreleasepool {
    g_log = fopen("/tmp/quadchroma-m1.log", "a");
    setvbuf(stdout, NULL, _IONBF, 0);

    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];

    NSArray<NSString *> *args = [[NSProcessInfo processInfo] arguments];

    // Eigener dauerhafter Schluessel. Er entsteht beim ersten Start und bleibt
    // danach liegen, damit Gegenstellen den Host wiedererkennen.
    if (qc_identity_load(g_id_priv, g_id_pub) != 0) {
        logf_(@"Schluessel konnte nicht angelegt werden - Abbruch.");
        return 5;
    }
    {
        char fp[24];
        qc_fingerprint(g_id_pub, fp);
        logf_(@"Fingerabdruck dieses Hosts: %s   freigegebene Gegenstellen: %d",
              fp, qc_authorized_count());
    }
    if ([args containsObject:@"--pair"]) {
        atomic_store(&g_pair_open, 1);
        logf_(@"Kopplung offen: die naechste unbekannte Gegenstelle wird aufgenommen.");
    }
    if ([args containsObject:@"--forget"]) {
        // Alle Freigaben loeschen. Danach koppelt sich die naechste Gegenstelle neu.
        const char *home = getenv("HOME");
        if (home) {
            NSString *pf = [NSString stringWithFormat:@"%s/Library/Application Support/QuadChroma/authorized.txt", home];
            [[NSFileManager defaultManager] removeItemAtPath:pf error:nil];
        }
        logf_(@"Alle Freigaben geloescht.");
    }

    NSInteger capIdx = [args indexOfObject:@"--capture"];
    NSInteger srvIdx = [args indexOfObject:@"--serve"];
    BOOL do_list = [args containsObject:@"--list"];
    if (!do_list && capIdx == NSNotFound && srvIdx == NSNotFound && ![args containsObject:@"--formattest"]) {
        logf_(@"Aufruf: --list | --capture <sekunden> <datei.hevc> | --serve [port]   "
               "[--display N] [--out BxH] [--fps N] [--mbit N] [--fest] [--pair] [--forget]");
        return 2;
    }

    if (!CGPreflightScreenCaptureAccess()) {
        logf_(@"Bildschirmaufnahme nicht freigegeben, frage nach.");
        CGRequestScreenCaptureAccess();
        return 3;
    }
    if (do_list) { list_displays(); codecs_pruefen(); return 0; }

    // Pruefmodus: nimmt die Aufnahme einen Formatwechsel im Betrieb an?
    // Das ist nirgends dokumentiert und entscheidet, ob die Codecwahl ohne
    // Neustart des Stroms geht. Also messen statt annehmen.
    if ([args containsObject:@"--formattest"]) {
        size_t pw = 0, ph = 0; double hz2 = 0;
        SCDisplay *d = pick_display(0, &pw, &ph, &hz2);
        if (!d) return 4;
        SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
        cfg.width = 1920; cfg.height = 1080;
        cfg.pixelFormat = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
        cfg.showsCursor = NO;
        cfg.minimumFrameInterval = CMTimeMake(1, 60);
        SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:d excludingWindows:@[]];
        Grabber *g = [[Grabber alloc] init];
        SCStream *st = [[SCStream alloc] initWithFilter:f configuration:cfg delegate:g];
        dispatch_queue_t q2 = dispatch_queue_create("tech.quadchroma.formattest", DISPATCH_QUEUE_SERIAL);
        NSError *e2 = nil;
        [st addStreamOutput:g type:SCStreamOutputTypeScreen sampleHandlerQueue:q2 error:&e2];
        dispatch_semaphore_t sem2 = dispatch_semaphore_create(0);
        [st startCaptureWithCompletionHandler:^(NSError *x) {
            logf_(@"Start: %@", x ? x.localizedDescription : @"ok");
            dispatch_semaphore_signal(sem2);
        }];
        dispatch_semaphore_wait(sem2, dispatch_time(DISPATCH_TIME_NOW, 10ull * NSEC_PER_SEC));
        atomic_store(&g_formattest, 1);
        [NSThread sleepForTimeInterval:1.5];

        OSType ziele[] = { kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                           kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange,
                           kCVPixelFormatType_32BGRA };
        for (int zi = 0; zi < 3; zi++) {
            uint32_t be = CFSwapInt32HostToBig(ziele[zi]);
            logf_(@"--- wechsle auf %.4s ---", (char *)&be);
            cfg.pixelFormat = ziele[zi];
            dispatch_semaphore_t s3 = dispatch_semaphore_create(0);
            [st updateConfiguration:cfg completionHandler:^(NSError *x) {
                logf_(@"updateConfiguration: %@", x ? x.localizedDescription : @"ohne Fehler");
                dispatch_semaphore_signal(s3);
            }];
            dispatch_semaphore_wait(s3, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
            atomic_store(&g_formattest, 1);
            [NSThread sleepForTimeInterval:1.5];
        }
        [st stopCaptureWithCompletionHandler:^(NSError *x) { (void)x; }];
        [NSThread sleepForTimeInterval:0.5];
        return 0;
    }
    codecs_pruefen();

    int displayIdx = 0, fps = 120, mbit = 150, outW = 0, outH = 0, port = 9001;
    double seconds = 0;
    NSString *outPath = nil;
    if ([args containsObject:@"--fest"] || [args containsObject:@"--fixed"])
        atomic_store(&g_cur_fixed, 1);
        atomic_store(&g_fixed_gewollt, 1);
    NSInteger i;
    if (capIdx != NSNotFound) {
        seconds = [args[capIdx + 1] doubleValue];
        outPath = args[capIdx + 2];
    }
    if (srvIdx != NSNotFound && srvIdx + 1 < (NSInteger)args.count && ![args[srvIdx + 1] hasPrefix:@"--"])
        port = [args[srvIdx + 1] intValue];
    if ((i = [args indexOfObject:@"--display"]) != NSNotFound) displayIdx = [args[i + 1] intValue];
    if ((i = [args indexOfObject:@"--fps"]) != NSNotFound) fps = [args[i + 1] intValue];
    if ((i = [args indexOfObject:@"--mbit"]) != NSNotFound) mbit = [args[i + 1] intValue];
    if ((i = [args indexOfObject:@"--out"]) != NSNotFound) sscanf(args[i + 1].UTF8String, "%dx%d", &outW, &outH);

    size_t pxW = 0, pxH = 0; double hz = 0;
    SCDisplay *display = pick_display(displayIdx, &pxW, &pxH, &hz);
    if (!display) return 4;
    if (outW <= 0 || outH <= 0) {
        outW = (int)(pxW >= 3840 ? pxW / 2 : pxW);
        outH = (int)(pxH >= 2160 ? pxH / 2 : pxH);
    }
    outW &= ~1; outH &= ~1;

    // Start immer mit Kandidat 0 (HEVC 4:4:4 10 Bit); das Aufnahmeformat
    // gehoert zum Kandidaten, nicht zur Kommandozeile.
    const int start_idx = 0;
    OSType pixfmt = pixfmt_fuer(start_idx);
    g_stats.nal_len = 4;
    if (outPath) {
        g_stats.out = fopen(outPath.UTF8String, "wb");
        if (!g_stats.out) { logf_(@"Ausgabedatei nicht schreibbar: %@", outPath); return 5; }
    }

    g_info_w = outW; g_info_h = outH; g_info_fps = fps;
    atomic_store(&g_codec_id, start_idx);

    if (srvIdx != NSNotFound) {
        if (start_server(port) < 0) return 9;
        start_input_server(port + 1, display.displayID);
        start_beacon(port);
        BOOL ax = AXIsProcessTrusted();
        logf_(@"\n=== Dienst laeuft: Bild %d, Eingabe %d, Bekanntgabe %d ===", port, port + 1, port + 2);
        logf_(@"Display %d (%zux%zu Pixel, %.0f Hz) -> %dx%d, %d fps", displayIdx, pxW, pxH, hz, outW, outH, fps);
        logf_(@"Bedienungshilfen-Freigabe (fuer Maus und Tastatur): %@", ax ? @"erteilt" : @"FEHLT - Eingaben werden ignoriert");
    } else {
        logf_(@"\n=== Aufnahme %.1f s: Display %d (%zux%zu Pixel) -> %dx%d, %d fps ===",
              seconds, displayIdx, pxW, pxH, outW, outH, fps);
    }

    // Im Dienstbetrieb entsteht der Encoder erst mit dem ersten Zuschauer.
    if (srvIdx == NSNotFound) {
        if (!encoder_start(start_idx, outW, outH, fps, mbit)) { if (g_stats.out) fclose(g_stats.out); return 6; }
    }

    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    cfg.width = outW;
    cfg.height = outH;
    cfg.pixelFormat = pixfmt;
    cfg.showsCursor = NO;
    if (@available(macOS 15.0, *)) cfg.showMouseClicks = NO;
    cfg.colorSpaceName = kCGColorSpaceSRGB;
    cfg.queueDepth = 8;
    qc_audio_configure(cfg);   // setzt capturesAudio und das Tonformat
    cfg.preservesAspectRatio = YES;
    cfg.captureResolution = SCCaptureResolutionBest;
    cfg.minimumFrameInterval = CMTimeMake(100, (int32_t)(fps * 100 * 0.9));
    cfg.scalesToFit = YES;

    Grabber *grab = [[Grabber alloc] init];
    g_grab = grab;
    g_display_idx = displayIdx;
    g_cfg = cfg;
    atomic_store(&g_cur_mbit, mbit);
    atomic_store(&g_cur_fps, fps);

    dispatch_queue_t q = dispatch_queue_create("tech.quadchroma.capture", DISPATCH_QUEUE_SERIAL);
    g_capq = q;
    g_lifeq = dispatch_queue_create("tech.quadchroma.lebenslauf", DISPATCH_QUEUE_SERIAL);

    // Taktgeber fuer die feste Bildrate. Er laeuft immer mit, tut aber nichts,
    // solange der Schalter aus ist oder niemand zuschaut - so kostet er nichts
    // und ist sofort da, wenn im Betrieb umgeschaltet wird.
    g_tick = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, q);
    if (g_tick) {
        uint64_t iv = (uint64_t)(NSEC_PER_SEC / (uint64_t)(fps > 0 ? fps : 60));
        dispatch_source_set_timer(g_tick, dispatch_time(DISPATCH_TIME_NOW, (int64_t)iv), iv, iv / 10);
        dispatch_source_set_event_handler(g_tick, ^{ fixed_tick(); });
        dispatch_resume(g_tick);
    }

    if (srvIdx != NSNotFound) {
        // Dienstbetrieb: keine Aufnahme, kein Encoder, bis sich jemand meldet.
        qc_clip_start(clip_cb);
    } else {
        // Aufnahme in eine Datei: sofort loslegen.
        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
        SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:grab];
        g_stream = stream;
        NSError *err = nil;
        if (![stream addStreamOutput:grab type:SCStreamOutputTypeScreen sampleHandlerQueue:q error:&err]) {
            logf_(@"Ausgabe konnte nicht angemeldet werden: %@", err.localizedDescription);
            return 7;
        }
        qc_audio_attach(stream, audio_cb);
        __block BOOL started = NO;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [stream startCaptureWithCompletionHandler:^(NSError *e) {
            if (e) logf_(@"Start fehlgeschlagen: %@ (Code %ld)", e.localizedDescription, (long)e.code);
            else started = YES;
            dispatch_semaphore_signal(sem);
        }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 15ull * NSEC_PER_SEC));
        if (!started) { if (g_stats.out) fclose(g_stats.out); return 8; }
    }

    // Einmal blind messen, damit die erste echte Meldung schon eine Differenz
    // hat und nicht mit Nullen anfaengt.
    { qc_last vorlauf; qc_last_probe(&vorlauf); }

    NSDate *t0 = [NSDate date];
    if (srvIdx != NSNotFound) {
        // Dienstbetrieb: alle fuenf Sekunden eine Zeile mit dem Stand.
        long lastFrames = 0; long long lastBytes = 0;
        long lastFrames2 = 0;   // eigener Bezugspunkt fuer die gemeldete Bildrate
        for (;;) {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:5.0]];
            long f = atomic_load(&g_sent_frames);
            long long b = atomic_load(&g_sent_bytes);
            if (atomic_load(&g_client_fd) >= 0)
                logf_(@"[%.0f s] Bild: %ld (%.1f/s, %.1f Mbit/s) | Ton: %ld Pakete, %.0f kB | Stau: %ld | Encoder verworfen: %ld | nachgelegt: %ld",
                      -[t0 timeIntervalSinceNow], f, (f - lastFrames) / 5.0,
                      (b - lastBytes) * 8.0 / 5.0 / 1e6,
                      atomic_load(&g_audio_packets), atomic_load(&g_audio_bytes) / 1000.0,
                      atomic_load(&g_skipped_backlog), g_stats.dropped,
                      atomic_load(&g_repeats));
            lastFrames = f; lastBytes = b;

            // Auslastung des Hosts. Nur wenn jemand zuschaut - sonst misst
            // der Mac sich selbst ohne Zweck.
            if (atomic_load(&g_client_fd) >= 0) {
                qc_last l;
                qc_last_probe(&l);
                long n = atomic_exchange(&g_enc_n, 0);
                long long summe = atomic_exchange(&g_enc_us, 0);
                uint16_t enc_zehntel = n ? (uint16_t)((summe / n) / 100) : 0;
                uint16_t host_fps_zehntel = (uint16_t)(((f - lastFrames2) * 10) / 5);
                lastFrames2 = f;

                uint8_t buf[28] = {0};
                buf[0] = 1;                       // Fassung
                memcpy(buf + 2,  &l.cpu_promille, 2);
                memcpy(buf + 4,  &l.cpu_eigen_promille, 2);
                memcpy(buf + 6,  &l.gpu_promille, 2);
                memcpy(buf + 8,  &l.druck, 2);
                memcpy(buf + 10, &l.ram_benutzt_mb, 4);
                memcpy(buf + 14, &l.ram_gesamt_mb, 4);
                memcpy(buf + 18, &l.eigen_mb, 4);
                memcpy(buf + 22, &enc_zehntel, 2);
                memcpy(buf + 24, &host_fps_zehntel, 2);
                send_small(QC_MSG_LAST, buf, sizeof buf);
            }
        }
    }

    [[NSRunLoop currentRunLoop] runUntilDate:[t0 dateByAddingTimeInterval:seconds]];
    double dt = -[t0 timeIntervalSinceNow];
    // Nur die Aufnahme in eine Datei kommt hierher; der Dienstbetrieb kehrt
    // aus seiner Schleife nie zurueck.
    dispatch_semaphore_t ende = dispatch_semaphore_create(0);
    [g_stream stopCaptureWithCompletionHandler:^(NSError *e) { (void)e; dispatch_semaphore_signal(ende); }];
    dispatch_semaphore_wait(ende, dispatch_time(DISPATCH_TIME_NOW, 5ull * NSEC_PER_SEC));
    if (g_session) VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid);
    if (g_stats.out) fclose(g_stats.out);

    logf_(@"Bilder aufgenommen: %ld (unveraendert uebersprungen: %ld)", grab.framesIn, grab.framesSkipped);
    logf_(@"Bilder codiert: %ld, verworfen: %ld, erster Fehler: %d", g_stats.encoded, g_stats.dropped, (int)g_stats.first_err);
    logf_(@"Tempo: %.1f Bilder/s ueber %.1f s", g_stats.encoded / dt, dt);
    logf_(@"Datenmenge: %.1f MB, entspricht %.1f Mbit/s", g_stats.bytes / 1e6, g_stats.bytes * 8.0 / dt / 1e6);
    return 0;
}}
