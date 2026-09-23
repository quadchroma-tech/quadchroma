// Pruefprogramm fuer die Teile von main.m, die ohne Bildschirmaufnahme laufen:
// Abloesen eines Zuschauers (Typ 10), Stauregel, Ansage des Tonformats,
// Koennensliste (AV1).
//
//   clang -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations \
//         -mmacosx-version-min=14.0 -framework Foundation -framework AppKit \
//         -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia \
//         -framework CoreVideo -framework CoreGraphics -framework CoreFoundation -framework IOKit \
//         host/hosttest.m host/audio.m host/clipboard.m host/zeiger.m host/testbild.m host/last.m \
//         host/qc_noise.c host/qc_secure.c host/qc_annahme.c host/vendor/monocypher/monocypher.c \
//         -o /tmp/hosttest
//   /tmp/hosttest
//
// main.m wird hier eingebunden; sein main heisst dann host_main und laeuft
// nie. Es startet also kein Dienst, keine Aufnahme, und nichts schreibt in
// /tmp/quadchroma-m1.log (g_log bleibt leer, alles geht nach stdout). Die
// Zuschauer sind echte TCP-Verbindungen ueber 127.0.0.1 ab Port 19100 mit
// tune_socket aus main.m; der Kanal hat einen festen Schluessel statt eines
// Handschlags. Die Bilder sind kuenstliche Zugriffseinheiten gegebener
// Groesse; sie gehen wie in encode_buffer erst durch stau_vor_dem_encoder
// und dann durch emit_access_unit - Stauregel und Versand sind also die des
// Hosts, nur der Encoder ist nachgebildet (liefert sofort, nichts im Flug).
// Dauer rund 40 s. Rueckgabe: Zahl der Fehler.

#define main host_main
#include "main.m"
#undef main

#include <fcntl.h>
#include <poll.h>
#include <signal.h>

static int g_fehler = 0;

static void pruefe(int ok, const char *was) {
    printf("%s  %s\n", ok ? "ok     " : "FEHLER ", was);
    if (!ok) g_fehler++;
}

static double sek(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + t.tv_nsec / 1e9;
}

// ------------------------------------------------------------ Verbindungen

static int g_lausch = -1, g_port = 0;

// Ein Zuschauer: host bekommt tune_socket wie im Host, client ist die
// Gegenstelle. rcvbuf > 0 macht den Empfangspuffer klein - dann ist das
// Lesen der Gegenstelle der Engpass, wie eine langsame Leitung.
static int paar(int *host, int *client, int rcvbuf) {
    if (g_lausch < 0) {
        for (int port = 19100; port < 19300 && g_lausch < 0; port++) {
            int fd = socket(AF_INET, SOCK_STREAM, 0);
            int one = 1;
            setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
            struct sockaddr_in a = {0};
            a.sin_family = AF_INET;
            a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            a.sin_port = htons((uint16_t)port);
            if (bind(fd, (struct sockaddr *)&a, sizeof a) == 0 && listen(fd, 8) == 0) { g_lausch = fd; g_port = port; }
            else close(fd);
        }
        if (g_lausch < 0) return -1;
        printf("Loopback-Port %d\n", g_port);
    }
    int c = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(c, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    if (rcvbuf > 0) setsockopt(c, SOL_SOCKET, SO_RCVBUF, &rcvbuf, sizeof rcvbuf);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)g_port);
    if (connect(c, (struct sockaddr *)&a, sizeof a) != 0) { close(c); return -1; }
    int h = accept(g_lausch, NULL, NULL);
    if (h < 0) { close(c); return -1; }
    tune_socket(h);
    *host = h;
    *client = c;
    return 0;
}

// Sendepuffer bis zum letzten Byte fuellen. MSG_DONTWAIT allein wartet auf
// diesem macOS bei vollem Puffer bis SO_SNDTIMEO (2 s je Schritt, rund 40 s
// je Fuellung) - daher kurz O_NONBLOCK, und danach wieder zuruecknehmen:
// zuschauer_abloesen verlaesst sich auf ein blockierendes Senden mit Frist.
static long puffer_fuellen(int h) {
    static uint8_t fuell[64 * 1024];
    long gefuellt = 0;
    int fl = fcntl(h, F_GETFL);
    fcntl(h, F_SETFL, fl | O_NONBLOCK);
    for (size_t stueck = sizeof fuell; stueck; stueck /= 2)
        for (;;) {
            ssize_t w = send(h, fuell, stueck, 0);
            if (w <= 0) break;
            gefuellt += w;
        }
    fcntl(h, F_SETFL, fl);
    return gefuellt;
}

static qc_chan *kanal(int fd, uint8_t schluessel) {
    qc_chan *c = calloc(1, sizeof *c);
    c->fd = fd;
    c->ok = 1;
    memset(c->tx.k, schluessel, sizeof c->tx.k);
    c->tx.has_key = 1;
    return c;
}

// Zuschauer eintragen, wie bild_verbindung es nach der Begruessung tut.
static void zuschauer_setzen(int host_fd, qc_chan *c) {
    pthread_mutex_lock(&g_send_mtx);
    g_vid = c;
    memset(g_vid_peer, 0x77, sizeof g_vid_peer);
    g_stau_seit = 0;
    atomic_store(&g_client_fd, host_fd);
    atomic_store(&g_vid_ready, 1);
    pthread_mutex_unlock(&g_send_mtx);
    atomic_store(&g_force_key, 1);
    atomic_store(&g_wait_key, 1);
    atomic_store(&g_audio_info_sent, 0);
}

// Zuschauer austragen, falls der Host es nicht schon getan hat.
static void zuschauer_weg(void) {
    pthread_mutex_lock(&g_send_mtx);
    int fd = atomic_exchange(&g_client_fd, -1);
    atomic_store(&g_vid_ready, 0);
    if (fd >= 0) close(fd);
    qc_chan_free(g_vid);
    g_vid = NULL;
    pthread_mutex_unlock(&g_send_mtx);
}

// ---------------------------------------------------------------- Lesen

// Die Clientseite: Datensaetze entschluesseln und Nachrichten zerlegen.
typedef struct {
    int fd;
    qc_cipher rx;
    uint8_t *buf;
    size_t len, cap;
    uint8_t ct[QC_CHUNK_MAX + QC_TAGLEN];
} leser;

static void leser_init(leser *l, int fd, uint8_t schluessel) {
    memset(l, 0, sizeof *l);
    l->fd = fd;
    memset(l->rx.k, schluessel, sizeof l->rx.k);
    l->rx.has_key = 1;
}

static int lies_genau(int fd, void *p, size_t n, int frist_ms) {
    uint8_t *q = p;
    while (n) {
        struct pollfd pf = { .fd = fd, .events = POLLIN, .revents = 0 };
        if (poll(&pf, 1, frist_ms) <= 0) return -1;
        ssize_t r = recv(fd, q, n, 0);
        if (r <= 0) return r == 0 && q == p ? 0 : -1;
        q += r; n -= (size_t)r;
    }
    return 1;
}

// 1 = Nachricht, 0 = Verbindung ordentlich zu, -1 = Fehler oder Frist.
// Von der Nutzlast landen hoechstens 8 Byte in anfang.
static int nachricht(leser *l, qc_hdr *h, uint8_t anfang[8], int frist_ms) {
    for (;;) {
        if (l->len >= sizeof *h) {
            memcpy(h, l->buf, sizeof *h);
            if (l->len >= sizeof *h + h->len) {
                size_t weg = sizeof *h + h->len;
                memset(anfang, 0, 8);
                memcpy(anfang, l->buf + sizeof *h, h->len < 8 ? h->len : 8);
                memmove(l->buf, l->buf + weg, l->len - weg);
                l->len -= weg;
                return 1;
            }
        }
        uint8_t lb[2];
        int r = lies_genau(l->fd, lb, 2, frist_ms);
        if (r <= 0) return r;
        size_t n = (size_t)lb[0] | ((size_t)lb[1] << 8);
        if (lies_genau(l->fd, l->ct, n, frist_ms) != 1) return -1;
        if (l->len + n > l->cap) {
            l->cap = (l->len + n) * 2;
            l->buf = realloc(l->buf, l->cap);
        }
        size_t pt = 0;
        if (qc_decrypt(&l->rx, l->ct, n, l->buf + l->len, &pt)) return -1;
        l->len += pt;
    }
}

// ------------------------------------------------ N1: Abloesen (Typ 10)

static void abloesen_pruefen(void) {
    printf("\n-- Abloesen: lebender alter Zuschauer\n");
    int h, c;
    if (paar(&h, &c, 0)) { pruefe(0, "Verbindung"); return; }
    int ein[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, ein);
    zuschauer_setzen(h, kanal(h, 0x11));
    atomic_store(&g_in_fd, ein[0]);
    // Vorher liegt schon einiges im Puffer: Typ 10 muss danach kommen.
    static uint8_t mb[1 << 20];
    send_small(QC_MSG_CLIP, mb, sizeof mb);
    send_small(QC_MSG_CLIP, mb, sizeof mb);

    char fp[24] = {0};
    double t0 = sek();
    pthread_mutex_lock(&g_send_mtx);
    int r = zuschauer_abloesen(-1, fp);
    pthread_mutex_unlock(&g_send_mtx);
    double dt = sek() - t0;
    printf("         (zuschauer_abloesen %.1f ms, Rueckgabe %d, Fingerabdruck %s)\n", dt * 1000, r, fp);
    pruefe(r == 1, "Abloese-Nachricht abgeschickt");
    pruefe(atomic_load(&g_client_fd) == -1 && g_vid == NULL && !atomic_load(&g_vid_ready) && atomic_load(&g_in_fd) == -1,
           "alter Zuschauer ausgetragen (Bild, Kanal, Eingabe)");

    leser l;
    leser_init(&l, c, 0x11);
    qc_hdr m;
    uint8_t anf[8];
    int n = 0, letzte = -1, laenge = -1, e;
    while ((e = nachricht(&l, &m, anf, 3000)) == 1) { n++; letzte = m.type; laenge = (int)m.len; }
    printf("         (%d Nachrichten, letzte Typ %d Laenge %d, danach %s)\n", n, letzte, laenge,
           e == 0 ? "Verbindungsende" : "Fehler/Frist");
    pruefe(n == 3 && letzte == QC_MSG_ABGELOEST && laenge == 0,
           "Typ 10 mit Laenge 0 kommt als letzte Nachricht, nach allem Gepufferten");
    pruefe(e == 0, "danach schliesst der Host den Bildkanal");
    uint8_t b;
    struct pollfd pf = { .fd = ein[1], .events = POLLIN, .revents = 0 };
    pruefe(poll(&pf, 1, 1000) == 1 && recv(ein[1], &b, 1, 0) == 0, "Eingabekanal des alten Zuschauers ist zu");
    close(c); close(ein[0]); close(ein[1]); free(l.buf);

    printf("\n-- Abloesen: eingefrorener alter Zuschauer\n");
    if (paar(&h, &c, 16 * 1024)) { pruefe(0, "Verbindung"); return; }
    // Gegenstelle liest nie: Sendepuffer bis zum letzten Byte fuellen.
    long gefuellt = puffer_fuellen(h);
    printf("         (Sendepuffer mit %ld Byte gefuellt)\n", gefuellt);
    zuschauer_setzen(h, kanal(h, 0x22));
    t0 = sek();
    pthread_mutex_lock(&g_send_mtx);
    r = zuschauer_abloesen(-1, fp);
    pthread_mutex_unlock(&g_send_mtx);
    dt = sek() - t0;
    printf("         (zuschauer_abloesen %.1f ms, Rueckgabe %d)\n", dt * 1000, r);
    pruefe(r == 0, "Nachricht kommt nicht durch und wird so gemeldet");
    pruefe(dt >= 0.08 && dt < 0.5, "hoechstens kurzer Versuch (rund 100 ms statt 2 s SO_SNDTIMEO)");
    pruefe(atomic_load(&g_client_fd) == -1 && g_vid == NULL, "trotzdem geschlossen und ausgetragen");
    close(c);
}

// ------------------------------------------------ N4: Tonformat ansagen

static void ton_pruefen(void) {
    printf("\n-- Tonformat\n");
    int h, c;
    if (paar(&h, &c, 0)) { pruefe(0, "Verbindung"); return; }
    zuschauer_setzen(h, kanal(h, 0x33));
    atomic_store(&g_cur_ton, 1);
    static float pcm[2 * 480];
    uint32_t raten[] = { 48000, 48000, 44100, 44100, 48000 };
    for (int i = 0; i < 5; i++) audio_cb(pcm, 441, raten[i], 2);
    audio_cb(pcm, 441, 48000, 1);
    zuschauer_weg();

    leser l;
    leser_init(&l, c, 0x33);
    qc_hdr m;
    uint8_t anf[8];
    char folge[256] = {0};
    size_t fl = 0;
    int e;
    while ((e = nachricht(&l, &m, anf, 2000)) == 1) {
        if (m.type == QC_MSG_AUDIO_INFO) {
            uint32_t rate;
            memcpy(&rate, anf, 4);
            fl += (size_t)snprintf(folge + fl, sizeof folge - fl, "[%u/%u]", rate, anf[4]);
        } else if (m.type == QC_MSG_AUDIO) {
            fl += (size_t)snprintf(folge + fl, sizeof folge - fl, "a");
        }
    }
    printf("         (Folge: %s; [Rate/Kanaele] = Ansage, a = Tonpaket)\n", folge);
    pruefe(strcmp(folge, "[48000/2]aa[44100/2]aa[48000/2]a[48000/1]a") == 0,
           "Ansage beim ersten Paket und bei jedem Wechsel von Rate oder Kanalzahl, sonst nicht");
    close(c); free(l.buf);
}

// ---------------------------------------------------- N2: Stauregel

// Liest mit fester Rate (Byte/s); 0 = liest gar nicht (eingefroren).
typedef struct {
    int fd;
    double rate;
    _Atomic int stop;
} abnehmer;

static void *abnehmen(void *arg) {
    abnehmer *a = arg;
    static const size_t PUFFER = 256 * 1024;
    uint8_t *puffer = malloc(PUFFER);
    double t_alt = sek(), guthaben = 0;
    while (!atomic_load(&a->stop)) {
        usleep(1000);
        if (a->rate <= 0) continue;
        double t = sek();
        guthaben += (t - t_alt) * a->rate;
        t_alt = t;
        if (guthaben > 4 * 1024 * 1024) guthaben = 4 * 1024 * 1024;
        while (guthaben >= 1) {
            size_t n = guthaben > PUFFER ? PUFFER : (size_t)guthaben;
            ssize_t r = recv(a->fd, puffer, n, MSG_DONTWAIT);
            if (r <= 0) break;
            guthaben -= (double)r;
        }
    }
    free(puffer);
    return NULL;
}

// Kuenstliche Zugriffseinheit: eine NAL-Einheit mit 4-Byte-Laenge.
static CMSampleBufferRef bild(size_t groesse) {
    uint8_t *mem = malloc(groesse);
    memset(mem, 0x42, groesse);
    uint32_t l = htonl((uint32_t)(groesse - 4));
    memcpy(mem, &l, 4);
    CMBlockBufferRef bb = NULL;
    CMBlockBufferCreateWithMemoryBlock(NULL, mem, groesse, kCFAllocatorMalloc, NULL, 0, groesse, 0, &bb);
    CMSampleBufferRef sb = NULL;
    size_t sz = groesse;
    CMSampleBufferCreate(NULL, bb, true, NULL, NULL, NULL, 1, 0, NULL, 1, &sz, &sb);
    CFRelease(bb);
    return sb;
}

typedef struct {
    const char *name;
    int fps;
    double mbit;            // Bitrate des Stroms
    size_t vollbild;        // Groesse eines Vollbilds
    double leitung_mbit;    // Leserate der Gegenstelle, 0 = eingefroren
    int spiel;              // Spielmodus
    int sndbuf_alt;         // Sendepuffer wie vor der Aenderung (2 MB)
    double sekunden;
} strom;

typedef struct {
    long bilder, gesendet, vollbilder, verworfen;
    double max_emit_ms;
    int max_rueckstand;              // vor einem gesendeten Bild
    int weg;
    double weg_nach_stau_s;
} ergebnis;

static ergebnis strom_fahren(const strom *s) {
    ergebnis e = {0};
    int h, c;
    if (paar(&h, &c, 64 * 1024)) { pruefe(0, "Verbindung"); return e; }
    if (s->sndbuf_alt) { int snd = 1 << 21; setsockopt(h, SOL_SOCKET, SO_SNDBUF, &snd, sizeof snd); }
    zuschauer_setzen(h, kanal(h, 0x44));
    atomic_store(&g_cur_gaming, s->spiel);
    abnehmer ab = { .fd = c, .rate = s->leitung_mbit * 1e6 / 8 };
    atomic_store(&ab.stop, 0);
    pthread_t t;
    pthread_create(&t, NULL, abnehmen, &ab);

    int gop = s->fps * 2;                 // MaxKeyFrameInterval = 2 * fps
    size_t p = (size_t)((s->mbit * 1e6 / 8 * 2 - (double)s->vollbild) / (gop - 1));
    CMSampleBufferRef kb = bild(s->vollbild), pb = bild(p);
    long verworfen0 = atomic_load(&g_skipped_backlog);
    double t0 = sek(), erster_stau = 0;
    long n = (long)(s->sekunden * s->fps), codiert = 0;
    for (long i = 0; i < n; i++) {
        e.bilder++;
        // Wie encode_buffer: erst die Stauregel, dann in den Encoder. Der
        // Encoder hier liefert sofort; Vollbild im festen Abstand der
        // codierten Bilder (MaxKeyFrameInterval) oder wenn erzwungen.
        if (stau_vor_dem_encoder()) {
            atomic_fetch_add(&g_skipped_backlog, 1);
            if (!erster_stau) erster_stau = sek();
        } else {
            int key = (codiert++ % gop) == 0;
            if (atomic_exchange(&g_force_key, 0)) key = 1;
            long gesendet0 = atomic_load(&g_sent_frames);
            int rueck = backlog_bytes(h);
            double a = sek();
            emit_access_unit(key ? kb : pb, key ? YES : NO, now_us(), 0);
            double d = (sek() - a) * 1000;
            if (d > e.max_emit_ms) e.max_emit_ms = d;
            if (atomic_load(&g_sent_frames) > gesendet0) {
                e.gesendet++;
                if (key) e.vollbilder++;
                if (rueck > e.max_rueckstand) e.max_rueckstand = rueck;
            }
        }
        if (atomic_load(&g_client_fd) < 0) {
            e.weg = 1;
            e.weg_nach_stau_s = erster_stau ? sek() - erster_stau : -1;
            break;
        }
        double soll = t0 + (double)(i + 1) / s->fps, jetzt = sek();
        if (soll > jetzt) usleep((useconds_t)((soll - jetzt) * 1e6));
    }
    e.verworfen = atomic_load(&g_skipped_backlog) - verworfen0;
    atomic_store(&ab.stop, 1);
    pthread_join(t, NULL);
    zuschauer_weg();
    atomic_store(&g_cur_gaming, 0);
    close(c);
    CFRelease(kb); CFRelease(pb);
    printf("         %-44s %4ld Bilder, %4ld gesendet (%3ld Vollbilder), %4ld verworfen, "
           "laengstes Senden %6.1f ms, Rueckstand vor Senden hoechstens %4d KB (%.0f ms Leitung)%s\n",
           s->name, e.bilder, e.gesendet, e.vollbilder, e.verworfen, e.max_emit_ms,
           e.max_rueckstand / 1024, s->leitung_mbit > 0 ? e.max_rueckstand / (s->leitung_mbit * 1e6 / 8) * 1000 : 0.0,
           e.weg ? " - Zuschauer weg" : "");
    return e;
}

static void stau_pruefen(void) {
    printf("\n-- Stauregel: Obergrenze von SO_NWRITE\n");
    for (int alt = 1; alt >= 0; alt--) {
        int h, c;
        if (paar(&h, &c, 16 * 1024)) { pruefe(0, "Verbindung"); return; }
        if (alt) { int snd = 1 << 21; setsockopt(h, SOL_SOCKET, SO_SNDBUF, &snd, sizeof snd); }
        int puffer = 0; socklen_t pl = sizeof puffer;
        getsockopt(h, SOL_SOCKET, SO_SNDBUF, &puffer, &pl);
        puffer_fuellen(h);
        int voll = backlog_bytes(h);
        printf("         (SO_SNDBUF %d: SO_NWRITE bei vollem Puffer %d)\n", puffer, voll);
        if (alt) pruefe(voll <= QC_BACKLOG_LIMIT, "mit 2 MB Sendepuffer (vorher) kommt SO_NWRITE nie ueber die 2-MB-Grenze");
        else pruefe(voll > QC_BACKLOG_LIMIT + 1024 * 1024, "mit tune_socket (4 MB) liegt die Grenze deutlich unter dem vollen Puffer");
        close(h); close(c);
    }

    // Vollbilder: 1,9 MB - so gross wie die groessten, die das Hostprotokoll
    // im Stau des Spielmodus bei 500 Mbit/s zeigt. Zwischenbilder fuellen die
    // Bitrate des Stroms auf.
    const size_t K = 1900 * 1024;
    printf("\n-- Stauregel: normaler Verkehr (darf nichts verwerfen)\n");
    strom a = { "150 Mbit/s, 120 fps, Leitung 400 Mbit/s", 120, 150, K, 400, 0, 0, 5 };
    ergebnis ea = strom_fahren(&a);
    pruefe(ea.verworfen == 0 && !ea.weg, "150 Mbit/s mit 1,9-MB-Vollbildern auf 400 Mbit/s: nichts verworfen");
    strom a2 = { "50 Mbit/s, 60 fps, Leitung 100 Mbit/s", 60, 50, K, 100, 0, 0, 5 };
    ergebnis ea2 = strom_fahren(&a2);
    pruefe(ea2.verworfen == 0 && !ea2.weg, "50 Mbit/s mit 1,9-MB-Vollbildern auf 100 Mbit/s: nichts verworfen");
    strom b = { "500 Mbit/s, 240 fps, Leitung 940 Mbit/s", 240, 500, K, 940, 0, 0, 5 };
    ergebnis eb = strom_fahren(&b);
    pruefe(eb.verworfen == 0 && !eb.weg, "500 Mbit/s, 240 fps auf Gigabit: nichts verworfen");
    strom bs = { "dasselbe im Spielmodus (512 KB)", 240, 500, K, 940, 1, 0, 5 };
    ergebnis ebs = strom_fahren(&bs);
    pruefe(ebs.vollbilder <= 3 && ebs.gesendet >= ebs.bilder * 9 / 10,
           "Spielmodus: nach einem Vollbild fallen nur wenige Bilder aus, keine Vollbild-Kaskade");

    printf("\n-- Stauregel: Leitung zu langsam (muss greifen)\n");
    strom cn = { "150 Mbit/s auf 100 Mbit/s", 120, 150, K, 100, 0, 0, 5 };
    ergebnis ec = strom_fahren(&cn);
    pruefe(ec.verworfen > 0, "die Regel greift ohne Spielmodus");
    pruefe(ec.max_emit_ms < 50, "kein blockierendes Senden");
    pruefe(ec.max_rueckstand <= QC_BACKLOG_LIMIT, "kein Bild geht hinter mehr als 2 MB Rueckstand raus");
    pruefe(ec.vollbilder <= 3, "keine Vollbild-Kaskade: nur die regulaeren Vollbilder (alle 2 s)");
    pruefe(ec.gesendet >= 300, "die Leitung traegt weiter Zwischenbilder (rund 80 je Sekunde moeglich)");
    pruefe(!ec.weg, "der langsame, aber lebende Zuschauer bleibt");
    strom ca = { "dasselbe mit 2 MB Sendepuffer (vorher)", 120, 150, K, 100, 0, 1, 5 };
    ergebnis eca = strom_fahren(&ca);
    pruefe(eca.verworfen == 0 && eca.max_emit_ms > ec.max_emit_ms,
           "Vergleich vorher: nie verworfen, statt dessen blockiert das Senden");

    printf("\n-- Stauregel: eingefrorener Zuschauer, Ton aus\n");
    atomic_store(&g_cur_ton, 0);
    strom d = { "150 Mbit/s, Gegenstelle liest nicht", 120, 150, K, 0, 0, 0, 6 };
    ergebnis ed = strom_fahren(&d);
    printf("         (Zuschauer weg %.2f s nach dem ersten verworfenen Bild)\n", ed.weg_nach_stau_s);
    pruefe(ed.weg && ed.weg_nach_stau_s >= 1.9 && ed.weg_nach_stau_s < 2.6,
           "nach 2 s Stau ohne Fortschritt gilt er als weg");
    pruefe(ed.max_emit_ms < 50, "bis dahin kein blockierendes Senden");
    pruefe(ed.gesendet < 20, "im Stau wird nichts mehr codiert und gesendet");
    strom ds = { "dasselbe im Spielmodus", 120, 150, K, 0, 1, 0, 6 };
    ergebnis eds = strom_fahren(&ds);
    pruefe(eds.weg && eds.weg_nach_stau_s < 2.6, "auch im Spielmodus (vorher dort laut Code: nie)");
    atomic_store(&g_cur_ton, 1);
}

// --------------------------------------------------- N5: Koennensliste

static void codecs_pruefen_pruefen(void) {
    printf("\n-- Koennensliste\n");
    codecs_pruefen();
    int av1 = -1;
    for (size_t i = 0; i < QC_KANDIDATEN; i++) if (g_kandidaten[i].codec == 'av01') av1 = (int)i;
    pruefe(av1 >= 0 && !g_befund[av1].vorhanden && !g_befund[av1].hardware, "AV1 wird nie als vorhanden gemeldet");
    pruefe(g_befund[0].vorhanden || g_befund[3].vorhanden, "HEVC wird weiter geprueft und gefunden");
}

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IONBF, 0);
        signal(SIGPIPE, SIG_IGN);
        g_stats.nal_len = 4;
        atomic_store(&g_cur_fps, 120);
        abloesen_pruefen();
        ton_pruefen();
        stau_pruefen();
        codecs_pruefen_pruefen();
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
