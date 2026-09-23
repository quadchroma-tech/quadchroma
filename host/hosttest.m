// Pruefprogramm fuer die Teile von main.m, die ohne Bildschirmaufnahme laufen:
// Protokoll (Drossel je Art und Adresse, Obergrenze, auch wenn das Umbenennen
// scheitert), Abloesen eines Zuschauers (Typ 10), Zuschauerwechsel im
// laufenden Strom, Testbild-Rest beim neuen Zuschauer, Abbau zwischen
// Hochfahren und Eintragen, Nachreichen bei stillem Bildschirm (als
// wiederholt gestempelt, im Stau ohne Taktversuche), Abschluss eines
// Codecwechsels ohne Zuschauer, Stauregel samt Ton im Stau, Ansage des
// Tonformats, Koennensliste (AV1).
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
// /tmp/quadchroma-m1.log: g_log bleibt leer, alles geht nach stdout - nur die
// Protokollpruefung lenkt g_log fuer sich auf eine Datei im eigenen HOME. Die
// Zuschauer sind echte TCP-Verbindungen ueber 127.0.0.1 ab Port 19100 mit
// tune_socket aus main.m; der Kanal hat einen festen Schluessel statt eines
// Handschlags. Wo es auf den Weg durch die Annahme ankommt, laufen echte
// Handschlaege gegen bild_verbindung und eingabe_verbindung (Ports ab 19400);
// HOME ist dann ein frischer Ordner unter $TMPDIR, die Freigabeliste des
// Nutzers und ein laufender Host bleiben unberuehrt. Die Bilder sind kuenstliche Zugriffseinheiten gegebener
// Groesse; sie gehen wie in encode_buffer erst durch stau_vor_dem_encoder
// und dann durch emit_access_unit - Stauregel und Versand sind also die des
// Hosts, nur der Encoder ist nachgebildet (liefert sofort, nichts im Flug).
// Nachreichen und Codecwechsel nehmen einen echten, kleinen Encoder (HEVC
// 4:2:0, 640x360, wenige Bilder); die Aufnahme ist dort eine Attrappe.
// Dauer rund 80 s. Rueckgabe: Zahl der Fehler.

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

// Zuschauer eintragen, wie bild_verbindung es nach der Begruessung tut -
// in derselben Reihenfolge: erst die Flaggen, dann gilt er als bereit.
static void zuschauer_setzen(int host_fd, qc_chan *c) {
    pthread_mutex_lock(&g_send_mtx);
    g_vid = c;
    memset(g_vid_peer, 0x77, sizeof g_vid_peer);
    g_stau_seit = 0;
    g_ton_stau_seit = 0;
    atomic_store(&g_force_key, 1);
    atomic_store(&g_wait_key, 1);
    atomic_store(&g_audio_info_sent, 0);
    atomic_store(&g_client_fd, host_fd);
    atomic_store(&g_vid_ready, 1);
    pthread_mutex_unlock(&g_send_mtx);
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

// ------------------------------------------------- Annahme und Handschlag

// Lauschender Socket auf 127.0.0.1 ab Port `ab`, mit der Annahme aus
// qc_annahme.c und dem Rueckruf des Hosts. Rueckgabe: der Port, sonst -1.
static int annahme_lauschen(int ab, qc_verbindung_fn fn, const char *name) {
    for (int port = ab; port < ab + 50; port++) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        int one = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
        struct sockaddr_in a = {0};
        a.sin_family = AF_INET;
        a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        a.sin_port = htons((uint16_t)port);
        if (bind(fd, (struct sockaddr *)&a, sizeof a) == 0 && listen(fd, QC_LISTEN_WARTESCHLANGE) == 0) {
            qc_annahme_cfg cfg = { QC_HANDSCHLAEGE, QC_HANDSCHLAEGE_JE_IP, fn, annahme_andrang, (void *)name };
            if (qc_annahme_starten(fd, &cfg) == 0) return port;
        }
        close(fd);
    }
    return -1;
}

static int verbinden(int port) {
    int c = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(c, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    if (connect(c, (struct sockaddr *)&a, sizeof a) != 0) { close(c); return -1; }
    return c;
}

static int rahmen_schreiben(int fd, const uint8_t *b, size_t n) {
    uint8_t l[2] = { (uint8_t)(n & 255), (uint8_t)(n >> 8) };
    if (send(fd, l, 2, 0) != 2) return -1;
    return send(fd, b, n, 0) == (ssize_t)n ? 0 : -1;
}

static int rahmen_lesen(int fd, uint8_t *b, size_t cap, size_t *n) {
    uint8_t l[2];
    struct pollfd pf = { .fd = fd, .events = POLLIN, .revents = 0 };
    if (poll(&pf, 1, 3000) != 1 || recv(fd, l, 2, MSG_WAITALL) != 2) return -1;
    *n = (size_t)l[0] | ((size_t)l[1] << 8);
    if (*n > cap) return -1;
    return recv(fd, b, *n, MSG_WAITALL) == (ssize_t)*n ? 0 : -1;
}

// Ein Client mit dem Schluessel priv: Handschlag wie der echte (Anrufer).
// rx bekommt den Empfangsschluessel, hh die Pruefsumme. -1 = gescheitert.
static int client_verbinden(int port, const uint8_t priv[32], qc_cipher *rx, uint8_t hh[QC_HASHLEN]) {
    int c = verbinden(port);
    if (c < 0) return -1;
    qc_handshake hs;
    qc_handshake_init(&hs, 1, priv, (const uint8_t *)QC_PRO_VIDEO, strlen(QC_PRO_VIDEO));
    uint8_t msg[8192], pl[8192];
    size_t ml = 0, pln = 0;
    if (qc_handshake_write(&hs, NULL, 0, msg, &ml) || rahmen_schreiben(c, msg, ml) ||
        rahmen_lesen(c, msg, sizeof msg, &ml) || qc_handshake_read(&hs, msg, ml, pl, &pln) ||
        qc_handshake_write(&hs, NULL, 0, msg, &ml) || rahmen_schreiben(c, msg, ml)) {
        close(c);
        return -1;
    }
    qc_cipher tx, empfang;
    qc_handshake_split(&hs, &tx, &empfang);
    if (rx) *rx = empfang;
    if (hh) memcpy(hh, qc_handshake_hash(&hs), QC_HASHLEN);
    return c;
}

// stdout zeitweise stumm schalten: logf_ schreibt jede Zeile auch dorthin.
static int g_stdout_alt = -1;
static void stdout_stumm(int stumm) {
    fflush(stdout);
    if (stumm) {
        g_stdout_alt = dup(1);
        int n = open("/dev/null", O_WRONLY);
        dup2(n, 1);
        close(n);
    } else if (g_stdout_alt >= 0) {
        dup2(g_stdout_alt, 1);
        close(g_stdout_alt);
        g_stdout_alt = -1;
    }
}

static char g_home[1024];

// Zeilen der Datei, die `was` enthalten.
static int zeilen_mit(const char *pfad, const char *was) {
    FILE *f = fopen(pfad, "r");
    if (!f) return -1;
    char z[2048];
    int n = 0;
    while (fgets(z, sizeof z, f)) if (strstr(z, was)) n++;
    fclose(f);
    return n;
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

// Einen Datensatz lesen und entschluesseln. 1 = gelesen, sonst wie lies_genau.
static int datensatz(leser *l, int frist_ms) {
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
    return 1;
}

// n Byte Klartext am Stueck (die Kennung "QCH1" vor der ersten Nachricht).
static int klartext(leser *l, void *p, size_t n, int frist_ms) {
    while (l->len < n) {
        int r = datensatz(l, frist_ms);
        if (r != 1) return r;
    }
    memcpy(p, l->buf, n);
    memmove(l->buf, l->buf + n, l->len - n);
    l->len -= n;
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
        int r = datensatz(l, frist_ms);
        if (r != 1) return r;
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
    // Ab hier: was im Stau zusaetzlich geschieht.
    size_t ablage;          // so viel Text vom Mac (clip_cb), 0,3 s nach Staubeginn
    double pause_s;         // vor der Ablage so lange kein Bild (stiller Bildschirm)
    int still;              // nach Staubeginn kein Bild mehr, nur noch die Frist im Takt
    int ton;                // Ton im Dauerlauf (3 Mbit/s, wie ScreenCaptureKit liefert)
} strom;

typedef struct {
    long bilder, gesendet, vollbilder, verworfen;
    double max_emit_ms;
    int max_rueckstand;              // vor einem gesendeten Bild
    int weg;
    double weg_nach_stau_s;
    long ton_verworfen;
    long gesendet_spaet;             // Bilder in der zweiten Haelfte des Laufs
    int blicke;                      // stiller Bildschirm: Blicke im Takt bis zum Austragen
} ergebnis;

static _Atomic int g_ton_lauf = 0;

// Ton wie aus ScreenCaptureKit: alle 10 ms 480 Stereo-Abtastwerte float32.
static void *ton_faden(void *arg) {
    (void)arg;
    static float pcm[2 * 480];
    while (atomic_load(&g_ton_lauf)) {
        audio_cb(pcm, 480, 48000, 2);
        usleep(10 * 1000);
    }
    return NULL;
}

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
    long verworfen0 = atomic_load(&g_skipped_backlog), ton0 = atomic_load(&g_audio_verworfen);
    pthread_t tt;
    if (s->ton) {
        atomic_store(&g_cur_ton, 1);
        atomic_store(&g_ton_lauf, 1);
        pthread_create(&tt, NULL, ton_faden, NULL);
    }
    int ablage_raus = 0;
    double t0 = sek(), erster_stau = 0;
    long n = (long)(s->sekunden * s->fps), codiert = 0;
    for (long i = 0; i < n; i++) {
        if (s->still && erster_stau) {
            // Stiller Bildschirm: kein Bild kommt mehr in encode_buffer. Nur
            // der 5-s-Takt des Dienstes prueft die Frist - hier alle 2,5 s.
            // Wie dort darf es einen Blick mehr brauchen: einmal in 14
            // Laeufen unter ASan reichte der erste nicht - vermutlich nahm
            // der Kernel der Gegenstelle nach dem Blick im Stau noch etwas
            // ab (nicht gemessen), dann zaehlt die Frist ab dort.
            for (int blick = 1; blick <= 2 && atomic_load(&g_client_fd) >= 0; blick++) {
                usleep(2500 * 1000);
                stau_frist_pruefen();
                e.blicke = blick;
            }
            if (atomic_load(&g_client_fd) < 0) { e.weg = 1; e.weg_nach_stau_s = sek() - erster_stau; }
            break;
        }

        e.bilder++;
        // Wie encode_buffer: erst die Stauregel, dann in den Encoder. Der
        // Encoder hier liefert sofort; Vollbild im festen Abstand der
        // codierten Bilder (MaxKeyFrameInterval) oder wenn erzwungen.
        int war_stau = 0;
        if (stau_vor_dem_encoder()) {
            atomic_fetch_add(&g_skipped_backlog, 1);
            if (!erster_stau) erster_stau = sek();
            war_stau = 1;
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
                if (sek() - t0 >= s->sekunden / 2) e.gesendet_spaet++;
                if (key) e.vollbilder++;
                if (rueck > e.max_rueckstand) e.max_rueckstand = rueck;
            }
        }
        // Die Ablage kommt direkt nach einem Blick im Stau: der gilt dann als
        // laufend, auch ueber die Pause hinweg (stiller Bildschirm).
        if (s->ablage && war_stau && !ablage_raus && sek() - erster_stau >= 0.3) {
            ablage_raus = 1;
            if (s->pause_s > 0) usleep((useconds_t)(s->pause_s * 1e6));
            static uint8_t text[4 * 1024 * 1024];
            memset(text, 'x', s->ablage);
            clip_cb((const char *)text, s->ablage);
            t0 = sek() - (double)(i + 1) / s->fps;     // der Takt der Bilder geht danach weiter
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
    if (s->ton) {
        atomic_store(&g_ton_lauf, 0);
        pthread_join(tt, NULL);
    }
    e.ton_verworfen = atomic_load(&g_audio_verworfen) - ton0;
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
    strom dst = { "Gegenstelle liest nicht, danach stiller Bildschirm", 120, 150, K, 0, 0, 0, 6, .still = 1 };
    ergebnis edst = strom_fahren(&dst);
    printf("         (ausgetragen beim %d. Blick im Takt, %.1f s nach Staubeginn)\n", edst.blicke, edst.weg_nach_stau_s);
    pruefe(edst.weg, "auch ohne ein weiteres Bild: die Frist im Takt des Dienstes traegt ihn aus");

    printf("\n-- Stauregel: lebender Zuschauer, im Stau geht mehr hinaus als Bilder\n");
    strom ab1 = { "Ablage 2,5 MB im Stau, Leitung 8 Mbit/s", 60, 150, K, 8, 0, 0, 4, .ablage = 2500 * 1024 };
    ergebnis eab1 = strom_fahren(&ab1);
    pruefe(!eab1.weg, "eine grosse Ablage im Stau trennt den Zuschauer nicht, der die ganze Zeit liest");
    // 3,5 MB: der Leser hat in der Pause Guthaben angesammelt und nimmt die
    // ersten rund 600 KB auf einen Schlag - danach liegt noch deutlich mehr
    // als der alte Stau im Puffer.
    strom ab2 = { "Stau, 3 s kein Bild, dann Ablage 3,5 MB, 8 Mbit/s", 60, 150, K, 8, 0, 0, 4, .ablage = 3500 * 1024, .pause_s = 3 };
    ergebnis eab2 = strom_fahren(&ab2);
    pruefe(!eab2.weg, "nach einer Pause ohne Bilder zaehlt der alte Stau nicht gegen ihn");

    printf("\n-- Ton im Stau\n");
    // Leitung unter der Tonrate: der Ton allein fuellte den Puffer, kein Bild
    // kaeme mehr durch. Im Spielmodus (512 KB), damit der Lauf kurz bleibt.
    strom tl = { "Spielmodus, Ton 3 Mbit/s, Bild 0,5 Mbit/s, Leitung 1 Mbit/s", 60, 0.5, 8 * 1024, 1, 1, 0, 8, .ton = 1 };
    ergebnis etl = strom_fahren(&tl);
    printf("         (%ld Tonpakete verworfen, %ld Bilder in der zweiten Haelfte gesendet)\n", etl.ton_verworfen, etl.gesendet_spaet);
    pruefe(etl.ton_verworfen > 0 && etl.gesendet_spaet > 0 && !etl.weg,
           "Leitung langsamer als der Ton: Ton faellt weg, das Bild bleibt nicht stehen, der Zuschauer bleibt");
    // Gewoehnlicher Stau und gesunde Leitung mit grossen Vollbildern: der
    // Rueckstand liegt nur kurz ueber der Grenze, kein Tonpaket faellt weg.
    strom ts = { "150 Mbit/s auf 100 Mbit/s, mit Ton", 120, 150, K, 100, 0, 0, 4, .ton = 1 };
    ergebnis ets = strom_fahren(&ts);
    strom tg = { "150 Mbit/s auf 400 Mbit/s, 1,9-MB-Vollbilder, mit Ton", 120, 150, K, 400, 0, 0, 3, .ton = 1 };
    ergebnis etg = strom_fahren(&tg);
    printf("         (verworfene Tonpakete: %ld im Stau, %ld auf der gesunden Leitung)\n", ets.ton_verworfen, etg.ton_verworfen);
    pruefe(ets.ton_verworfen == 0 && etg.ton_verworfen == 0 && !ets.weg && !etg.weg,
           "im gewoehnlichen Stau und nach grossen Vollbildern bleibt der Ton ganz");
    atomic_store(&g_cur_ton, 1);
}

// ------------------------------------------ Protokoll: Drossel und Grenze

typedef struct { int port; _Atomic int stop; _Atomic long n; } flut_arg;

// Verbinden und sofort schliessen, so schnell es geht. SO_LINGER 0: ein RST
// statt TIME_WAIT, sonst gingen die Ports fuer die spaeteren Pruefungen aus.
static void *fluten(void *arg) {
    flut_arg *a = arg;
    struct linger lg = { 1, 0 };
    while (!atomic_load(&a->stop)) {
        int c = socket(AF_INET, SOCK_STREAM, 0);
        setsockopt(c, SOL_SOCKET, SO_LINGER, &lg, sizeof lg);
        struct sockaddr_in ad = {0};
        ad.sin_family = AF_INET;
        ad.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        ad.sin_port = htons((uint16_t)a->port);
        if (connect(c, (struct sockaddr *)&ad, sizeof ad) == 0) atomic_fetch_add(&a->n, 1);
        close(c);
    }
    return NULL;
}

// Zurueckgehaltene Zeilen einer Art: alle Adressen zusammen.
static long drossel_weitere(qc_drossel *d) {
    pthread_mutex_lock(&d->m);
    long n = d->sonst;
    for (int i = 0; i < QC_DROSSEL_ADRESSEN; i++) n += d->platz[i].weitere;
    pthread_mutex_unlock(&d->m);
    return n;
}

// Frist der Drossel als abgelaufen behandeln, ohne zehn Sekunden zu warten.
static void drossel_altern(qc_drossel *d) {
    pthread_mutex_lock(&d->m);
    for (int i = 0; i < QC_DROSSEL_ADRESSEN; i++) d->platz[i].zuletzt_s -= QC_MELDEN_S;
    d->sonst_s -= QC_MELDEN_S;
    pthread_mutex_unlock(&d->m);
}

static void protokoll_pruefen(int bild_port, int ein_port) {
    printf("\n-- Protokoll: Flut ungepruefter Verbindungen\n");
    char pfad[1100], alt[1100];
    snprintf(pfad, sizeof pfad, "%s/hosttest.log", g_home);
    snprintf(alt, sizeof alt, "%s/hosttest.alt.log", g_home);
    g_log_pfad = pfad;
    g_log_alt_pfad = alt;
    pthread_mutex_lock(&g_log_mtx);
    log_oeffnen("a");
    pthread_mutex_unlock(&g_log_mtx);
    if (!g_log) { pruefe(0, "eigene Protokolldatei"); return; }

    // Wie die Probe des Pruefers: 8 Faeden, die verbinden und sofort
    // schliessen - je 4 auf den Bild- und den Eingabeport (kein Bildkanal offen).
    flut_arg fa[8];
    pthread_t t[8];
    stdout_stumm(1);
    for (int i = 0; i < 8; i++) {
        fa[i].port = i < 4 ? bild_port : ein_port;
        atomic_store(&fa[i].stop, 0);
        atomic_store(&fa[i].n, 0);
        pthread_create(&t[i], NULL, fluten, &fa[i]);
    }
    usleep(2000 * 1000);
    long n_bild = 0, n_ein = 0;
    for (int i = 0; i < 8; i++) atomic_store(&fa[i].stop, 1);
    for (int i = 0; i < 8; i++) {
        pthread_join(t[i], NULL);
        if (i < 4) n_bild += atomic_load(&fa[i].n); else n_ein += atomic_load(&fa[i].n);
    }
    usleep(300 * 1000);                     // die letzten Verbindungsfaeden enden
    stdout_stumm(0);
    int z_bild = zeilen_mit(pfad, "Handschlag mit 127.0.0.1 gescheitert");
    int z_ein = zeilen_mit(pfad, "kein Bildkanal offen");
    int z_alle = zeilen_mit(pfad, "");
    long w_bild = drossel_weitere(&d_bild_handschlag), w_ein = drossel_weitere(&d_ein_ohne_bild);
    printf("         (%ld Verbindungen auf den Bildport, %ld auf den Eingabeport in 2 s; "
           "im Protokoll %d + %d Zeilen, %d insgesamt; zurueckgehalten %ld + %ld)\n",
           n_bild, n_ein, z_bild, z_ein, z_alle, w_bild, w_ein);
    pruefe(n_bild > 200 && n_ein > 200, "die Flut kommt an (je Port mehr als 100 Verbindungen je Sekunde)");
    pruefe(z_bild == 1 && z_ein == 1, "je Art genau eine Zeile, die erste sofort");
    pruefe(z_alle <= 6, "insgesamt nur eine Handvoll Zeilen (dazu hoechstens die Andrang-Meldungen)");
    // Nicht jede Verbindung kommt bis zur Zeile: viele verdraengt die Annahme
    // vorher (gezaehlt in ihrer eigenen, ebenfalls gedrosselten Andrang-Meldung).
    pruefe(w_bild > 100 && w_ein > 100, "die zurueckgehaltenen werden gezaehlt, nicht vergessen");

    // Mitten in der Flut von 127.0.0.1 versucht es ein anderes Geraet: seine
    // erste Zeile kommt sofort. Weitere Adressen bekommen einen der
    // QC_DROSSEL_ADRESSEN Plaetze; sind alle belegt, zaehlen sie gemeinsam.
    // Ueber Loopback gibt es nur eine Absenderadresse, also direkt ueber die
    // Drossel, mit denselben Zeilen wie bild_verbindung.
    stdout_stumm(1);
    const char *andere[] = { "192.0.2.1", "192.0.2.1", "192.0.2.2", "192.0.2.3", "192.0.2.4", "192.0.2.5" };
    for (size_t i = 0; i < sizeof andere / sizeof andere[0]; i++)
        logf_gedrosselt(&d_bild_handschlag, andere[i], @"Handschlag mit %s gescheitert (%d)", andere[i], -1);
    stdout_stumm(0);
    int z1 = zeilen_mit(pfad, "Handschlag mit 192.0.2.1 gescheitert");
    int z2 = zeilen_mit(pfad, "Handschlag mit 192.0.2.2 gescheitert");
    int z3 = zeilen_mit(pfad, "Handschlag mit 192.0.2.3 gescheitert");
    int z45 = zeilen_mit(pfad, "Handschlag mit 192.0.2.4 gescheitert") + zeilen_mit(pfad, "Handschlag mit 192.0.2.5 gescheitert");
    printf("         (andere Adressen waehrend der Flut: je eine Zeile fuer 192.0.2.1-3: %d/%d/%d, "
           "ohne Platz 192.0.2.4-5: %d, zurueckgehalten jetzt %ld)\n", z1, z2, z3, z45, drossel_weitere(&d_bild_handschlag));
    pruefe(z1 == 1 && z2 == 1 && z3 == 1,
           "je Adresse: die erste Zeile eines anderen Geraets kommt mitten in der Flut sofort, eine Wiederholung nicht");
    pruefe(z45 == 0 && drossel_weitere(&d_bild_handschlag) == w_bild + 3,
           "mehr Adressen als Plaetze: nur noch gezaehlt - hoechstens QC_DROSSEL_ADRESSEN Zeilen je Art und Frist");

    // Nach der Frist kommt die Sammelzeile auch ohne neue Verbindung, je
    // Adresse eine, fuer die ohne Platz eine gemeinsame.
    drossel_altern(&d_bild_handschlag);
    drossel_altern(&d_ein_ohne_bild);
    stdout_stumm(1);
    drosseln_nachtragen();
    drosseln_nachtragen();
    stdout_stumm(0);
    char erwartet[200];
    snprintf(erwartet, sizeof erwartet, "Bildkanal: Handschlag gescheitert: %ld weitere von 127.0.0.1 seit der letzten Meldung", w_bild);
    int s_bild = zeilen_mit(pfad, erwartet);
    snprintf(erwartet, sizeof erwartet, "kein Bildkanal offen: %ld weitere von 127.0.0.1 seit der letzten Meldung", w_ein);
    int s_ein = zeilen_mit(pfad, erwartet);
    int s_1 = zeilen_mit(pfad, "Bildkanal: Handschlag gescheitert: 1 weitere von 192.0.2.1 seit der letzten Meldung");
    int s_sonst = zeilen_mit(pfad, "Bildkanal: Handschlag gescheitert: 2 weitere von anderen Adressen seit der letzten Meldung, zuletzt von 192.0.2.5");
    int s_alle = zeilen_mit(pfad, "Handschlag gescheitert: ");
    pruefe(s_bild == 1 && s_ein == 1 && s_1 == 1 && s_sonst == 1 && s_alle == 3 && drossel_weitere(&d_bild_handschlag) == 0,
           "nach der Frist genau eine Sammelzeile je Adresse mit der Zahl, eine fuer die Adressen ohne Platz");

    // Voller Handschlag mit Wegwerfschluessel, wie ein ungekoppelter Client,
    // der alle zwei Sekunden neu versucht - nur schneller.
    uint8_t anderer_priv[32], anderer_pub[32];
    qc_keypair(anderer_priv, anderer_pub);
    // Liste nicht leer: kein Erstkontakt, der Wegwerfschluessel ist unbekannt.
    pruefe(qc_authorize(anderer_pub, "hosttest") == 0, "Freigabeliste im eigenen HOME angelegt");
    int gelungen = 0;
    stdout_stumm(1);
    for (int i = 0; i < 6; i++) {
        uint8_t wegwerf[32], wp[32];
        qc_keypair(wegwerf, wp);
        int c = client_verbinden(bild_port, wegwerf, NULL, NULL);
        if (c >= 0) { gelungen++; usleep(30 * 1000); close(c); }
    }
    usleep(200 * 1000);
    stdout_stumm(0);
    int z_unb = zeilen_mit(pfad, "Abgewiesen: unbekannte Gegenstelle");
    printf("         (%d Handschlaege mit fremden Schluesseln, %d Zeilen 'unbekannte Gegenstelle', zurueckgehalten %ld)\n",
           gelungen, z_unb, drossel_weitere(&d_unbekannt));
    pruefe(gelungen == 6 && z_unb == 1 && drossel_weitere(&d_unbekannt) == 5,
           "unbekannte Gegenstelle: eine Zeile, der Rest gezaehlt");

    printf("\n-- Protokoll: Obergrenze der Datei\n");
    long grenze_vorher = g_log_grenze;
    g_log_grenze = 4096;
    stdout_stumm(1);
    for (int i = 0; i < 300; i++) logf_(@"Pruefzeile %03d - so lang wie eine gewoehnliche Zeile im Hostprotokoll", i);
    stdout_stumm(0);
    struct stat sa, sn;
    int ok_alt = stat(alt, &sa) == 0, ok_neu = stat(pfad, &sn) == 0;
    int kopf = 0;
    FILE *f = fopen(pfad, "r");
    char erste[256] = {0};
    if (f) { if (fgets(erste, sizeof erste, f)) kopf = strstr(erste, "Protokoll war ueber 4 KB") != NULL; fclose(f); }
    printf("         (Datei %lld Byte, .alt %lld Byte, Grenze %ld)\n",
           ok_neu ? (long long)sn.st_size : -1, ok_alt ? (long long)sa.st_size : -1, g_log_grenze);
    pruefe(ok_alt && ok_neu && sn.st_size < g_log_grenze + 256 && sa.st_size < g_log_grenze + 256,
           "ueber der Grenze wandert die Datei nach .alt.log, beide bleiben unter der Grenze plus einer Zeile");
    pruefe(kopf, "die neue Datei sagt in ihrer ersten Zeile, wo die alten Zeilen stehen");
    pruefe(zeilen_mit(pfad, "Pruefzeile 299") == 1, "die juengste Zeile steht in der neuen Datei");

    // Laesst sich die Datei nicht umbenennen (hier: an der Stelle des .alt.log
    // liegt ein Ordner; in /tmp genuegte eine fremde Datei), haelt die Grenze
    // trotzdem: die Datei beginnt leer, und der Hinweis steht einmal darin.
    unlink(alt);
    mkdir(alt, 0700);
    char drin[1200];
    snprintf(drin, sizeof drin, "%s/fremd", alt);
    FILE *fr = fopen(drin, "w");
    if (fr) fclose(fr);
    stdout_stumm(1);
    for (int i = 0; i < 300; i++) logf_(@"Zweite Runde %03d - so lang wie eine gewoehnliche Zeile im Hostprotokoll", i);
    stdout_stumm(0);
    int ok_neu2 = stat(pfad, &sn) == 0;
    int kopf2 = 0;
    f = fopen(pfad, "r");
    memset(erste, 0, sizeof erste);
    if (f) { if (fgets(erste, sizeof erste, f)) kopf2 = strstr(erste, "liess sich nicht nach") != NULL; fclose(f); }
    int hinweise = zeilen_mit(pfad, "Protokoll war ueber");
    printf("         (Umbenennen gesperrt: Datei %lld Byte, Hinweiszeilen %d)\n", ok_neu2 ? (long long)sn.st_size : -1, hinweise);
    pruefe(ok_neu2 && sn.st_size < g_log_grenze + 512 && kopf2 && hinweise == 1 &&
           zeilen_mit(pfad, "Zweite Runde 299") == 1,
           "Umbenennen gescheitert: die Datei wird geleert, die Grenze haelt, der Hinweis steht einmal vorn");
    unlink(drin);
    rmdir(alt);

    pthread_mutex_lock(&g_log_mtx);
    fclose(g_log);
    g_log = NULL;
    pthread_mutex_unlock(&g_log_mtx);
    g_log_grenze = grenze_vorher;
}

// --------------------------------------------- Zuschauerwechsel, Abbau

// Attrappe fuer einen laufenden Strom: ScreenCaptureKit gehoert nicht in den
// Pruefstand. Anhalten und Umstellen melden sofort Erfolg.
@interface FakeStrom : SCStream
@end
@implementation FakeStrom
- (void)stopCaptureWithCompletionHandler:(void (^)(NSError *))h { if (h) h(nil); }
- (void)updateConfiguration:(SCStreamConfiguration *)c completionHandler:(void (^)(NSError *))h { if (h) h(nil); }
@end

// Nie freigeben: eine nie initialisierte Attrappe darf nicht in dealloc laufen.
static NSMutableArray *g_attrappen;

static void strom_attrappe_setzen(void) {
    FakeStrom *fs = [FakeStrom alloc];
    [g_attrappen addObject:fs];
    stream_setzen(fs);
}

static SCStream *strom_jetzt(void) {
    dispatch_sync(g_lifeq, ^{});            // eingereihter Auf- oder Abbau ist durch
    __block SCStream *st = nil;
    dispatch_sync(g_capq, ^{ st = g_stream; });
    return st;
}

typedef struct { _Atomic int stop; CMSampleBufferRef kb, pb; } hammer_arg;

// Encoder und Ton im Dauerlauf: ein Zwischenbild nach dem anderen, ein
// Vollbild nur, wenn es erzwungen ist - wie encode_buffer -, dazu Ton.
static void *hammern(void *arg) {
    hammer_arg *a = arg;
    static float pcm[2 * 480];
    while (!atomic_load(&a->stop)) {
        int key = atomic_exchange(&g_force_key, 0);
        emit_access_unit(key ? a->kb : a->pb, key ? YES : NO, now_us(), 0);
        audio_cb(pcm, 480, 48000, 2);
        usleep(300);
    }
    return NULL;
}

// Ein Wechsel: A schaut und bekommt laufend Bilder und Ton, B loest ab.
// Rueckgabe: 1 = alles in Ordnung; was schiefging, steht in *was.
static int wechsel_einmal(int bild_port, const uint8_t b_priv[32], char *was, size_t wl) {
    strom_attrappe_setzen();
    int h, c;
    if (paar(&h, &c, 0)) { snprintf(was, wl, "Verbindung"); return 0; }
    zuschauer_setzen(h, kanal(h, 0x61));
    abnehmer ab = { .fd = c, .rate = 1e10 };        // A liest alles
    atomic_store(&ab.stop, 0);
    pthread_t ta;
    pthread_create(&ta, NULL, abnehmen, &ab);
    atomic_store(&g_testbild, 1);                    // hat A fuer den Benchmark eingeschaltet
    hammer_arg ha = { .kb = bild(20 * 1024), .pb = bild(2 * 1024) };
    atomic_store(&ha.stop, 0);
    pthread_t th;
    pthread_create(&th, NULL, hammern, &ha);
    usleep(100 * 1000);                              // A bekommt Bilder und Ton, wartet auf nichts mehr

    qc_cipher rx;
    int b = client_verbinden(bild_port, b_priv, &rx, NULL);
    leser l;
    leser_init(&l, b, 0);
    l.rx = rx;
    char magic[5] = {0};
    int ok_magic = b >= 0 && klartext(&l, magic, 4, 2000) == 1 && memcmp(magic, QC_MAGIC, 4) == 0;
    int n = 0, erster_typ = -1, video = 0, erstes_video_key = -1, ansage_an = -1, ton_an = -1;
    double t0 = sek();
    qc_hdr m;
    uint8_t anf[8];
    while (ok_magic && sek() - t0 < 0.3 && nachricht(&l, &m, anf, 1000) == 1) {
        if (n == 0) erster_typ = m.type;
        if (m.type == QC_MSG_VIDEO && video++ == 0) erstes_video_key = (m.flags & QC_FLAG_KEY) ? 1 : 0;
        if (m.type == QC_MSG_AUDIO_INFO && ansage_an < 0) ansage_an = n;
        if (m.type == QC_MSG_AUDIO && ton_an < 0) ton_an = n;
        n++;
    }
    atomic_store(&ha.stop, 1);
    pthread_join(th, NULL);
    dispatch_sync(g_capq, ^{});                      // Aufraeumen des Testbilds ist durch
    int testbild = atomic_load(&g_testbild);
    SCStream *st = strom_jetzt();
    was[0] = 0;
    if (!ok_magic || erster_typ != QC_MSG_INFO) snprintf(was, wl, "Kennung/Strominfo nicht zuerst (Typ %d)", erster_typ);
    else if (erstes_video_key != 1) snprintf(was, wl, "erstes Bild %s", erstes_video_key == 0 ? "ein ZWISCHENBILD" : "fehlt");
    else if (ansage_an < 0 || ton_an <= ansage_an) snprintf(was, wl, "Ton an Stelle %d, Ansage an Stelle %d", ton_an, ansage_an);
    else if (testbild) snprintf(was, wl, "Testbild noch an");
    else if (atomic_load(&g_anmeldend) != 0 || st == nil) snprintf(was, wl, "unterwegs %d, Strom %s", atomic_load(&g_anmeldend), st ? "da" : "weg");
    zuschauer_weg();
    atomic_store(&ab.stop, 1);
    pthread_join(ta, NULL);
    close(b); close(c); free(l.buf);
    CFRelease(ha.kb); CFRelease(ha.pb);
    return was[0] == 0;
}

static void wechsel_pruefen(int bild_port) {
    printf("\n-- Zuschauerwechsel im laufenden Strom\n");
    uint8_t b_priv[32], b_pub[32];
    qc_keypair(b_priv, b_pub);
    qc_authorize(b_pub, "hosttest B");
    atomic_store(&g_cur_ton, 1);
    // Das Fenster, in dem der Neue den Stand des Vorgaengers erbte, ist kurz:
    // mehrere Wechsel, jeder mit Bildern und Ton im Dauerlauf.
    int gut = 0, laeufe = 4;
    char was[128];
    for (int i = 0; i < laeufe; i++) {
        stdout_stumm(1);
        int r = wechsel_einmal(bild_port, b_priv, was, sizeof was);
        stdout_stumm(0);
        if (r) gut++;
        else printf("         (Wechsel %d: %s)\n", i, was);
    }
    printf("         (%d von %d Wechseln in Ordnung)\n", gut, laeufe);
    pruefe(gut == laeufe, "der Neue bekommt Kennung und Strominfo zuerst, als erstes Bild ein Vollbild, "
                          "Ton erst nach der Ansage, das Testbild des Vorgaengers ist aus");
}

static void *ablage_senden(void *arg) {
    static uint8_t text[1 << 20];
    memset(text, 'x', sizeof text);
    clip_cb((const char *)text, sizeof text);
    return NULL;
}

// Die Probe des Pruefers als Pruefung: der Vorgaenger ist eingefroren, ein
// Senden an ihn haengt unter g_send_mtx; der Neue hat das Hochfahren hinter
// sich und wartet auf die Sperre. Laeuft die Sendefrist ab, traegt der
// Fehlerweg den Alten aus und reiht den Abbau ein - der darf den Strom des
// Neuen nicht anhalten.
static void abbau_wettlauf_pruefen(int bild_port) {
    printf("\n-- Abbau zwischen Hochfahren und Eintragen\n");
    uint8_t c_priv[32], c_pub[32];
    qc_keypair(c_priv, c_pub);
    qc_authorize(c_pub, "hosttest C");
    int schlecht = 0, laeufe = 2;
    for (int i = 0; i < laeufe; i++) {
        strom_attrappe_setzen();
        int h, c;
        if (paar(&h, &c, 16 * 1024)) { pruefe(0, "Verbindung"); return; }
        puffer_fuellen(h);
        zuschauer_setzen(h, kanal(h, 0x71));
        pthread_t t;
        pthread_create(&t, NULL, ablage_senden, NULL);   // haengt bis zu 2 s unter der Sperre
        usleep(300 * 1000);
        int neu = client_verbinden(bild_port, c_priv, NULL, NULL);
        pthread_join(t, NULL);                            // Sendefrist abgelaufen, Alter ausgetragen
        usleep(300 * 1000);
        SCStream *st = strom_jetzt();
        int fd = atomic_load(&g_client_fd);
        int gut = neu >= 0 && fd >= 0 && st != nil && atomic_load(&g_anmeldend) == 0;
        printf("         (Lauf %d: neuer Zuschauer eingetragen: %s, Aufnahme laeuft: %s)\n", i,
               fd >= 0 ? "ja" : "nein", st ? "ja" : "NEIN");
        schlecht += !gut;
        zuschauer_weg();
        if (neu >= 0) close(neu);
        close(c);
    }
    pruefe(schlecht == 0, "der Neue ist eingetragen, und seine Aufnahme laeuft weiter");
    // Gegenprobe: ohne Zuschauer und ohne einen, der unterwegs ist, baut der
    // Abbau weiter ab.
    stream_herunterfahren_anstossen();
    pruefe(strom_jetzt() == nil, "ohne Zuschauer haelt der Abbau den Strom an");
}

// ------------------------------------ Nachreichen bei stillem Bildschirm

static CVPixelBufferRef testpuffer(int w, int h) {
    NSDictionary *attr = @{ (id)kCVPixelBufferIOSurfacePropertiesKey: @{} };
    CVPixelBufferRef pb = NULL;
    CVPixelBufferCreate(NULL, (size_t)w, (size_t)h, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                        (__bridge CFDictionaryRef)attr, &pb);
    if (!pb) return NULL;
    CVPixelBufferLockBaseAddress(pb, 0);
    for (size_t i = 0; i < CVPixelBufferGetPlaneCount(pb); i++)
        memset(CVPixelBufferGetBaseAddressOfPlane(pb, i), 0x80,
               CVPixelBufferGetBytesPerRowOfPlane(pb, i) * CVPixelBufferGetHeightOfPlane(pb, i));
    CVPixelBufferUnlockBaseAddress(pb, 0);
    return pb;
}

// Ein Bild, wie es ScreenCaptureKit liefert: vollstaendig, mit Zeitstempel.
static CMSampleBufferRef aufnahme_bild(CVPixelBufferRef pb, CMTime pts) {
    CMVideoFormatDescriptionRef fd = NULL;
    CMVideoFormatDescriptionCreateForImageBuffer(NULL, pb, &fd);
    CMSampleTimingInfo ti = { .duration = kCMTimeInvalid, .presentationTimeStamp = pts, .decodeTimeStamp = kCMTimeInvalid };
    CMSampleBufferRef sb = NULL;
    CMSampleBufferCreateReadyWithImageBuffer(NULL, pb, fd, &ti, &sb);
    if (fd) CFRelease(fd);
    if (!sb) return NULL;
    CFArrayRef att = CMSampleBufferGetSampleAttachmentsArray(sb, true);
    CFMutableDictionaryRef d = (CFMutableDictionaryRef)CFArrayGetValueAtIndex(att, 0);
    int v = SCFrameStatusComplete;
    CFNumberRef n = CFNumberCreate(NULL, kCFNumberIntType, &v);
    CFDictionarySetValue(d, (__bridge CFStringRef)SCStreamFrameInfoStatus, n);
    CFRelease(n);
    return sb;
}

static CMTime uhr(void) { return CMClockGetTime(CMClockGetHostTimeClock()); }

// Nachrichten lesen, bis frist_ms lang nichts kommt; Bilder und Vollbilder
// zaehlen, dazu die Bilder, deren Stempel "wiederholt" traegt (aus der
// Latenzmessung heraus).
static void bilder_lesen(leser *l, int frist_ms, int *bilder, int *vollbilder, int *erstes_voll, int *wiederholt) {
    qc_hdr m;
    uint8_t anf[8];
    int stempel_wiederholt = 0;
    while (nachricht(l, &m, anf, frist_ms) == 1) {
        if (m.type == QC_MSG_STAMP) { stempel_wiederholt = anf[4] & 1; continue; }
        if (m.type != QC_MSG_VIDEO) continue;
        int key = (m.flags & QC_FLAG_KEY) ? 1 : 0;
        if (*bilder == 0) *erstes_voll = key;
        (*bilder)++;
        *vollbilder += key;
        if (wiederholt) *wiederholt += stempel_wiederholt;
        stempel_wiederholt = 0;
    }
}

static void takt(int schlaege) {
    for (int i = 0; i < schlaege; i++) {
        dispatch_sync(g_capq, ^{ fixed_tick(); });
        usleep(20 * 1000);
    }
}

// Ein Testbild-Rahmen, der beim Wechsel noch im Encoder steckte (oder den der
// Takt gerade noch aus der Schleife nahm), kommt erst nach dem Eintragen des
// Neuen heraus - im Testbild alle zwei Sekunden auch als Vollbild. Er darf
// nicht dessen erstes Bild werden; das naechste vom Bildschirm wird erzwungen.
static void testbild_rest_pruefen(void) {
    printf("\n-- Testbild-Rest beim neuen Zuschauer\n");
    int h, c;
    if (paar(&h, &c, 0)) { pruefe(0, "Verbindung"); return; }
    zuschauer_setzen(h, kanal(h, 0x51));             // wartet auf ein Vollbild
    atomic_store(&g_testbild, 0);                    // bild_verbindung hat es abgeschaltet
    atomic_store(&g_force_key, 0);
    CMSampleBufferRef kb = bild(8 * 1024), pb = bild(1024);
    emit_access_unit(kb, YES, now_us(), QC_BILD_TESTBILD);    // Testbild-Vollbild aus dem Encoder
    emit_access_unit(pb, NO, now_us(), QC_BILD_TESTBILD);     // und ein Zwischenbild dahinter
    int wartet = atomic_load(&g_wait_key), erzwungen = atomic_load(&g_force_key);
    atomic_store(&g_force_key, 0);
    emit_access_unit(kb, YES, now_us(), 0);                   // Vollbild vom Bildschirm
    emit_access_unit(pb, NO, now_us(), 0);
    // Gegenprobe: hat der Zuschauer das Testbild selbst eingeschaltet
    // (Benchmark), gehoert es zu ihm - auch wenn er auf ein Vollbild wartet.
    atomic_store(&g_testbild, 1);
    atomic_store(&g_wait_key, 1);
    emit_access_unit(kb, YES, now_us(), QC_BILD_TESTBILD);
    atomic_store(&g_testbild, 0);
    leser l;
    leser_init(&l, c, 0x51);
    int bilder = 0, voll = 0, erstes = -1;
    bilder_lesen(&l, 300, &bilder, &voll, &erstes, NULL);
    printf("         (%d Bilder beim Zuschauer, %d Vollbilder)\n", bilder, voll);
    pruefe(wartet && erzwungen, "ein veralteter Testbild-Rahmen geht nicht hinaus, der Zuschauer wartet weiter, "
                                "und das naechste Bild wird ein Vollbild");
    pruefe(bilder == 3 && voll == 2 && erstes == 1,
           "beim Zuschauer: Vollbild und Zwischenbild vom Bildschirm, dann sein eigenes Testbild");
    zuschauer_weg();
    close(c); free(l.buf);
    CFRelease(kb); CFRelease(pb);
}

static void nachreichen_pruefen(int bild_port) {
    printf("\n-- Nachreichen bei stillem Bildschirm (echter Encoder, HEVC 4:2:0, 640x360)\n");
    atomic_store(&g_codec_id, 3);
    g_info_w = 640; g_info_h = 360; g_info_fps = 60;
    atomic_store(&g_cur_fps, 60);
    atomic_store(&g_cur_mbit, 10);
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_cur_gaming, 0);
    atomic_store(&g_testbild, 0);
    __block BOOL enc = NO;
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{ enc = encoder_start(3, 640, 360, 60, 10); });
    stdout_stumm(0);
    CVPixelBufferRef pb = testpuffer(640, 360);
    if (!enc || !pb) { pruefe(0, "Encoder und Bildpuffer"); return; }
    strom_attrappe_setzen();
    SCStream *fs = strom_jetzt();
    Grabber *grab = [[Grabber alloc] init];
    // Der Bildschirm aenderte sich zuletzt vor einer Sekunde; der Vorgaenger
    // hat dieses Bild laengst bekommen.
    CMTime vor = CMTimeSubtract(uhr(), CMTimeMake(1, 1));
    dispatch_sync(g_capq, ^{
        g_last_pb = CVPixelBufferRetain(pb);
        g_last_cap_us = cmtime_us(vor);
        g_last_ankunft_us = cmtime_us(vor);
        g_last_pts = vor;
        g_schlitz = 0;
    });
    atomic_store(&g_bild_offen, 0);

    uint8_t d_priv[32], d_pub[32];
    qc_keypair(d_priv, d_pub);
    qc_authorize(d_pub, "hosttest D");
    qc_cipher rx;
    stdout_stumm(1);
    int b = client_verbinden(bild_port, d_priv, &rx, NULL);
    leser l;
    leser_init(&l, b, 0);
    l.rx = rx;
    char magic[4];
    int ok = b >= 0 && klartext(&l, magic, 4, 2000) == 1;
    usleep(50 * 1000);                               // eingetragen
    long nach0 = atomic_load(&g_nachgereicht);
    long enc_n0 = atomic_load(&g_enc_n);
    long long enc_us0 = atomic_load(&g_enc_us);
    takt(5);
    int bilder = 0, voll = 0, erstes = -1, wiederholt = 0;
    bilder_lesen(&l, 300, &bilder, &voll, &erstes, &wiederholt);
    stdout_stumm(0);
    printf("         (neuer Zuschauer, stiller Bildschirm: %d Bild(er), das erste %s, %d als wiederholt gestempelt; "
           "Encoderzeit %ld Bild(er), %lld us)\n", bilder,
           erstes == 1 ? "ein Vollbild" : erstes == 0 ? "ein Zwischenbild" : "-", wiederholt,
           atomic_load(&g_enc_n) - enc_n0, atomic_load(&g_enc_us) - enc_us0);
    pruefe(ok && bilder == 1 && erstes == 1 && !atomic_load(&g_wait_key),
           "der Neue bekommt das zuletzt gesehene Bild als Vollbild, genau einmal");
    // Das Bild ist eine Sekunde alt. Ginge es als frisch hinaus, stuende
    // dieses Alter als Encoderzeit (Host) und als Verzoegerung (Client) in
    // der Messung - bei einem seit Minuten stillen Bildschirm Minuten.
    pruefe(wiederholt == 1 && atomic_load(&g_enc_n) == enc_n0 && atomic_load(&g_enc_us) == enc_us0,
           "das nachgereichte Bild ist als wiederholt gestempelt und bleibt aus der Latenzmessung");

    // Eine kurze Bewegung: zwei Bilder im Abstand von 3 ms, das zweite ist
    // das Endbild und faellt als zu schnell weg.
    // Die Zeitstempel sind die der Aufnahme, nicht die der Zustellung: ein
    // erster Aufruf an einen frischen Encoder kann selbst 40 ms dauern.
    long zs0 = atomic_load(&g_zu_schnell);
    CMTime t1 = uhr();
    CMSampleBufferRef s1 = aufnahme_bild(pb, t1);
    CMSampleBufferRef s2 = aufnahme_bild(pb, CMTimeAdd(t1, CMTimeMake(3, 1000)));
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s1 ofType:SCStreamOutputTypeScreen]; });
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s2 ofType:SCStreamOutputTypeScreen]; });
    int weg = atomic_load(&g_zu_schnell) == zs0 + 1 && atomic_load(&g_bild_offen);
    // Der Takt innerhalb derselben Bildzeit legt nichts dazu.
    dispatch_sync(g_capq, ^{ fixed_tick(); });
    long vorher = atomic_load(&g_nachgereicht);
    takt(4);
    bilder = 0; voll = 0; erstes = -1; wiederholt = 0;
    stdout_stumm(1);
    bilder_lesen(&l, 300, &bilder, &voll, &erstes, &wiederholt);
    stdout_stumm(0);
    printf("         (Bewegung aus 2 Bildern, Endbild zu schnell: %d Bild(er) beim Zuschauer, %ld nachgereicht, "
           "%d als wiederholt gestempelt, Encoderzeit %ld Bild(er))\n",
           bilder, atomic_load(&g_nachgereicht) - vorher, wiederholt, atomic_load(&g_enc_n) - enc_n0);
    pruefe(weg, "das Endbild der Bewegung faellt im Raster weg und bleibt als offen stehen");
    pruefe(bilder == 2 && !atomic_load(&g_bild_offen) && atomic_load(&g_nachgereicht) - vorher == 1,
           "der Takt reicht es genau einmal nach - der Zuschauer sieht den Endstand");
    pruefe(wiederholt == 1 && atomic_load(&g_enc_n) == enc_n0 + 1,
           "nur das frisch aufgenommene Bild zaehlt in die Encoderzeit, das nachgereichte ist als wiederholt gestempelt");

    // Mitten in einer Bewegung (juengstes Bild juenger als eine Bildzeit)
    // legt der Takt nichts dazu; die naechste Aufnahme kommt ohnehin.
    // Das juengste Bild ist gerade angekommen, aufgenommen aber schon 20 ms
    // vorher - mehr als eine Bildzeit bei 60 fps (ScreenCaptureKit liefert mit
    // Verzug). Massgeblich ist die Ankunft; mit der Aufnahmezeit reichte der
    // Takt hier das aeltere Bild nach.
    dispatch_sync(g_capq, ^{ g_last_ankunft_us = now_us(); g_last_cap_us = g_last_ankunft_us - 20000; g_last_pts = CMTimeSubtract(uhr(), CMTimeMake(1, 10)); });
    atomic_store(&g_bild_offen, 1);
    long frames0 = atomic_load(&g_sent_frames);
    dispatch_sync(g_capq, ^{ fixed_tick(); });
    usleep(100 * 1000);
    int still = atomic_load(&g_sent_frames) == frames0 && atomic_load(&g_bild_offen);
    usleep(20 * 1000);
    takt(1);
    usleep(100 * 1000);
    pruefe(still && atomic_load(&g_sent_frames) == frames0 + 1 && !atomic_load(&g_bild_offen),
           "waehrend einer Bewegung nichts dazu, erst eine Bildzeit nach dem juengsten Bild");
    printf("         (insgesamt %ld nachgereicht)\n", atomic_load(&g_nachgereicht) - nach0);

    // Im Stau: das Endbild wartet, aber der Takt versucht es nicht bei jedem
    // Schlag - "Stau" im 5-s-Protokoll zaehlt ausgelassene Bilder, nicht
    // Taktschlaege -, und er belegt keinen Schlitz. Ein echtes Bild zaehlt
    // wie bisher einmal. (Die Frist von 2 s laeuft hier nicht ab.)
    int hfd = atomic_load(&g_client_fd);
    long gefuellt = hfd >= 0 ? puffer_fuellen(hfd) : 0;
    dispatch_sync(g_capq, ^{
        g_last_cap_us = now_us() - 1000000ull;
        g_last_ankunft_us = g_last_cap_us;
        g_last_pts = CMTimeSubtract(uhr(), CMTimeMake(1, 1));
    });
    atomic_store(&g_bild_offen, 1);
    long stau0 = atomic_load(&g_skipped_backlog), fr_stau0 = atomic_load(&g_sent_frames);
    __block double schlitz0 = 0, schlitz1 = 0;
    dispatch_sync(g_capq, ^{ schlitz0 = g_schlitz; });
    takt(5);
    dispatch_sync(g_capq, ^{ schlitz1 = g_schlitz; });
    long stau_takt = atomic_load(&g_skipped_backlog) - stau0;
    CMSampleBufferRef s3 = aufnahme_bild(pb, uhr());
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s3 ofType:SCStreamOutputTypeScreen]; });
    long stau_echt = atomic_load(&g_skipped_backlog) - stau0 - stau_takt;
    printf("         (Stau mit %ld Byte: 5 Taktschlaege zaehlen %ld, ein echtes Bild %ld)\n", gefuellt, stau_takt, stau_echt);
    pruefe(hfd >= 0 && atomic_load(&g_client_fd) == hfd && stau_takt == 0 && schlitz1 == schlitz0 &&
           atomic_load(&g_sent_frames) == fr_stau0 && atomic_load(&g_bild_offen) && stau_echt == 1,
           "im Stau: der Takt zaehlt keine Schlaege als Stau und belegt keinen Schlitz, das Endbild bleibt offen");

    zuschauer_weg();
    stdout_stumm(1);
    stream_herunterfahren_anstossen();
    SCStream *st = strom_jetzt();
    stdout_stumm(0);
    pruefe(st == nil && !g_session && !g_last_pb, "ohne Zuschauer: Strom, Encoder und letztes Bild weg");
    // Ein Nachzuegler der anhaltenden Aufnahme bleibt nicht liegen.
    CMSampleBufferRef s4 = aufnahme_bild(pb, uhr());
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s4 ofType:SCStreamOutputTypeScreen]; });
    __block int liegt = 0;
    dispatch_sync(g_capq, ^{ liegt = g_last_pb != NULL; });
    pruefe(!liegt, "ein Bild nach dem Abbau wird nicht mehr festgehalten");
    atomic_store(&g_bild_offen, 0);
    close(b); free(l.buf);
    CFRelease(s1); CFRelease(s2); CFRelease(s3); CFRelease(s4);
    CVPixelBufferRelease(pb);
    atomic_store(&g_codec_id, 0);
}

// ------------------------------------------- Codecwechsel ohne Zuschauer

static int wechsel_aktiv(void) {
    __block int a = 0;
    dispatch_sync(g_capq, ^{ a = g_wechsel_aktiv; });
    return a;
}

static void codec_abschluss_pruefen(void) {
    printf("\n-- Codecwechsel, dessen Abschluss nach dem Zuschauer kommt (echter Encoder)\n");
    atomic_store(&g_codec_id, 3);
    g_info_w = 640; g_info_h = 360;
    atomic_store(&g_cur_fps, 60);
    atomic_store(&g_cur_mbit, 10);
    OSType f420 = pixfmt_fuer(3);

    // a) Ein neuer Zuschauer hat den Strom schon neu aufgebaut, mit eigener
    //    Sitzung - dann erst kommt der Abschluss des alten Wechsels (3 -> 4).
    __block VTCompressionSessionRef vorher = NULL, nachher = NULL;
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{
        encoder_start(3, 640, 360, 60, 10);
        vorher = g_session;
        g_wechsel_aktiv = 1;
        codec_wechsel_abschliessen(4, 3, f420, NO);
        nachher = g_session;
    });
    stdout_stumm(0);
    pruefe(vorher && nachher == vorher && atomic_load(&g_codec_id) == 3 && !wechsel_aktiv(),
           "Strom inzwischen neu aufgebaut: der Wechsel wird verworfen, die Sitzung des Neuen bleibt");
    dispatch_sync(g_capq, ^{
        VTCompressionSessionInvalidate(g_session);
        CFRelease(g_session);
        g_session = NULL;
    });

    // b) Der Zuschauer ist weg, sein Abbau lief schon (keine Sitzung, kein
    //    Strom) - dann baut der Abschluss eine Sitzung fuer niemanden.
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{
        g_wechsel_aktiv = 1;
        codec_wechsel_abschliessen(3, 4, f420, NO);
    });
    SCStream *st = strom_jetzt();                    // der angestossene Abbau ist durch
    __block VTCompressionSessionRef rest = NULL;
    dispatch_sync(g_capq, ^{ rest = g_session; });
    stdout_stumm(0);
    pruefe(st == nil && rest == NULL && !wechsel_aktiv(), "Zuschauer weg: die frische Sitzung wird gleich wieder abgebaut");

    // c) Wie b, aber der neue Codec laesst sich nicht oeffnen und das Format
    //    war gewechselt: ohne Strom kaeme der Abschluss des Zuruecksetzens nie.
    //    (AV1 codiert hier niemand; kann ein spaeterer Mac es doch, gilt der
    //    Erfolgszweig - auch dann darf nichts haengen bleiben.)
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{
        g_wechsel_aktiv = 1;
        codec_wechsel_abschliessen(5, 3, f420, YES);
    });
    st = strom_jetzt();
    dispatch_sync(g_capq, ^{ rest = g_session; });
    stdout_stumm(0);
    pruefe(!wechsel_aktiv() && rest == NULL, "fehlgeschlagener Wechsel ohne Strom: nicht fuer immer unterwegs, keine Sitzung bleibt");
    g_cfg = nil;
    atomic_store(&g_codec_id, 0);
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
        // Eigenes HOME: Freigabeliste und Protokollpruefung schreiben dorthin.
        const char *tmp = getenv("TMPDIR");
        snprintf(g_home, sizeof g_home, "%s/qc-hosttest-XXXXXX", tmp && *tmp ? tmp : "/tmp");
        if (!mkdtemp(g_home)) { printf("kein eigenes HOME\n"); return 1; }
        char lib[1100];
        snprintf(lib, sizeof lib, "%s/Library", g_home);
        mkdir(lib, 0700);
        snprintf(lib, sizeof lib, "%s/Library/Application Support", g_home);
        mkdir(lib, 0700);
        setenv("HOME", g_home, 1);
        printf("HOME %s\n", g_home);
        qc_keypair(g_id_priv, g_id_pub);
        g_capq = dispatch_queue_create("hosttest.aufnahme", DISPATCH_QUEUE_SERIAL);
        g_lifeq = dispatch_queue_create("hosttest.lebenslauf", DISPATCH_QUEUE_SERIAL);
        int bild_port = annahme_lauschen(19400, bild_verbindung, "Bildkanal");
        int ein_port = annahme_lauschen(19450, eingabe_verbindung, "Eingabekanal");
        printf("Annahme: Bildport %d, Eingabeport %d\n", bild_port, ein_port);
        if (bild_port < 0 || ein_port < 0) return 1;
        g_stats.nal_len = 4;
        atomic_store(&g_cur_fps, 120);
        g_attrappen = [NSMutableArray array];
        protokoll_pruefen(bild_port, ein_port);
        abloesen_pruefen();
        wechsel_pruefen(bild_port);
        testbild_rest_pruefen();
        abbau_wettlauf_pruefen(bild_port);
        nachreichen_pruefen(bild_port);
        codec_abschluss_pruefen();
        ton_pruefen();
        stau_pruefen();
        codecs_pruefen_pruefen();
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
