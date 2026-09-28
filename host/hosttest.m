// Pruefprogramm fuer die Teile von main.m, die ohne Bildschirmaufnahme laufen:
// Protokoll (Drossel je Art und Adresse, Obergrenze, auch wenn das Umbenennen
// scheitert), Abloesen eines Zuschauers (Typ 10), Zuschauerwechsel im
// laufenden Strom, Testbild-Rest beim neuen Zuschauer, Abbau zwischen
// Hochfahren und Eintragen, Nachreichen bei stillem Bildschirm (als
// wiederholt gestempelt, im Stau ohne Taktversuche), Abschluss eines
// Codecwechsels ohne Zuschauer, Codecwechsel mit anderem Aufnahmeformat bei
// stillem Bildschirm (Umrechnung des letzten Bildes), --fest beim Start,
// Stauregel samt Ton im Stau, Ansage des Tonformats, Koennensliste (AV1),
// Dateien ueber die Zwischenablage (Faehigkeiten, beide Richtungen ueber echte
// Kanaele, Fenster, Umlaute im Protokoll, Zuschauerwechsel mitten in der
// Uebertragung, Sitzungsbindung des Wegs, Zuschauer ohne Eingabekanal,
// aelterer Client ohne IN_FAEHIGKEITEN, Faehigkeit nur fuer den Eingabekanal,
// der sie gemeldet hat), Bildschirmwahl (Pruefvektoren der Nachrichten 12 und
// 70, Kuerzen, reine Wahl, Stromgroesse, bildschirm.txt, --display als Pin,
// Begruessung mit Faehigkeiten 3 und Liste, Automatik folgt dem
// Hauptbildschirm entprellt (INFO als Fassung 1 in SDR, SWITCH mit Transfer
// SDR), Wunsch ueber den echten Eingabekanal, fehlender
// Wunsch mit Ausweichplatz und Rueckkehr, andere Groesse mit neuem Encoder,
// Warten auf einen Codecwechsel (endet, sobald kein Wechsel mehr ansteht;
// 5-s-Frist), leere Liste bei laufendem Strom, Bildschirmverlust und
// Wiederherstellung - Liste und Strom aus Attrappen, Encoder echt),
// HDR-Aushandlung (Faehigkeiten mit Bit 2, IN_ANZEIGE ueber den echten
// Eingabekanal: neue Strominfo in SDR mit dem Grund des Hosts, nichts bei
// gleicher Lage, Unlesbares uebergangen; dann bis HDR10 und zurueck: HDR am
// Bildschirm des Hosts an/aus, Codecwechsel mit Farbe, Sperre 2 s, Grund 6,
// wenn die Aufnahme HDR ablehnt, Abloesung mitten in HDR10), HDR10 mit echtem
// Encoder und Decoder (SEI 137/144 byte-genau hinter PPS jedes Vollbilds,
// VUI und Anhaenge 2020/PQ/2020, HDR-Testbild, P3 -> BT.2020), Dienst-Takt
// ohne Run-Loop (Auslastung im Takt, Drosselzeilen nachgetragen), Abschied
// beim Beenden (Typ 13 mit Grund 0 als letzte Nachricht, ohne Typ 10, dann
// Verbindung zu) und Zugang
// (zugang.h: ein unbekannter Client wie der Rust-Client bekommt "QCA1" und 20,
// Passwort richtig mit host_proof und danach "QCH1", falsch mit Drossel und
// Schluss nach 5 Versuchen, Zulassen und Ablehnen ueber den Test-Haken statt
// der Oberflaeche, Abbruch, Verbindungsende, nur eine Anfrage zugleich,
// Grenzen je Adresse und Schluessel, Frist, ein Wartender stoert den
// laufenden Zuschauer nicht, beschaedigte Liste, Entfernen trennt die
// Sitzung mit dem Abschied (Typ 13, Grund 2), ohne Bildschirmfreigabe oder ohne Bildschirm Hoststatus 1 statt
// einer Abweisung, Name mit vorgetaeuschter ID, Argumente ohne Wert).
//
//   clang -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations \
//         -mmacosx-version-min=14.0 -framework Foundation -framework AppKit \
//         -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia \
//         -framework CoreVideo -framework CoreGraphics -framework CoreFoundation -framework IOKit \
//         -framework SystemConfiguration \
//         host/hosttest.m host/audio.m host/clipboard.m host/zeiger.m host/testbild.m host/last.m \
//         host/dateien.m host/bildschirm.m host/hdr.c host/qc_noise.c host/qc_secure.c host/qc_annahme.c \
//         host/zugang.c host/vendor/monocypher/monocypher.c -o /tmp/hosttest
//   /tmp/hosttest
//
// main.m wird hier eingebunden (main selbst steht in start.m und gehoert
// nicht dazu; qc_dienst_starten und qc_werkzeug laufen hier nie). Die
// Oberflaeche (menue.m) bindet der Pruefstand nicht: main.m bringt
// schwache Standardfassungen fuer ihre Einstiege mit. Es startet also kein
// Dienst, keine Aufnahme, keine Oberflaeche, und nichts schreibt in
// /tmp/quadchroma-m1.log: g_log bleibt leer, alles geht nach stdout - nur die
// Protokollpruefung lenkt g_log fuer sich auf eine Datei im eigenen HOME. Die
// Zuschauer sind echte TCP-Verbindungen ueber 127.0.0.1 ab Port 19100 mit
// tune_socket aus main.m; der Kanal hat einen festen Schluessel statt eines
// Handschlags. Wo es auf den Weg durch die Annahme ankommt, laufen echte
// Handschlaege gegen bild_verbindung und eingabe_verbindung (Ports ab 19400);
// HOME ist dann ein frischer Ordner unter $TMPDIR, Geraeteliste und Passwort
// des Nutzers und ein laufender Host bleiben unberuehrt. Die Bilder sind kuenstliche Zugriffseinheiten gegebener
// Groesse; sie gehen wie in encode_buffer erst durch stau_vor_dem_encoder
// und dann durch emit_access_unit - Stauregel und Versand sind also die des
// Hosts, nur der Encoder ist nachgebildet (liefert sofort, nichts im Flug).
// Nachreichen und Codecwechsel nehmen einen echten, kleinen Encoder (HEVC
// 4:2:0, 640x360, wenige Bilder); die Aufnahme ist dort eine Attrappe.
// Dateien: die Ablagebasis liegt im eigenen HOME, und statt
// qc_clip_set_dateien bekommt ein Rekorder die fertigen Pfade - die
// Zwischenablage des Nutzers bleibt unberuehrt.
// Dauer rund 125 s. Rueckgabe: Zahl der Fehler.

#include "main.m"

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

    // Voller Handschlag mit Wegwerfschluessel, wie ein neuer Client, der
    // alle zwei Sekunden neu versucht und aufgibt - nur schneller: jeder
    // bekommt die Zugangsphase ("QCA1", 20), und das Protokoll bleibt kurz.
    int gelungen = 0, qca1 = 0;
    stdout_stumm(1);
    for (int i = 0; i < 6; i++) {
        uint8_t wegwerf[32], wp[32];
        qc_keypair(wegwerf, wp);
        qc_cipher rx;
        int c = client_verbinden(bild_port, wegwerf, &rx, NULL);
        if (c >= 0) {
            gelungen++;
            leser l;
            leser_init(&l, c, 0);
            l.rx = rx;
            char k[4];
            if (klartext(&l, k, 4, 2000) == 1 && memcmp(k, QC_ZUGANG_KENNUNG, 4) == 0) qca1++;
            free(l.buf);
            close(c);
        }
        // Die Phase endet mit der Verbindung; hoechstens 2 je Adresse.
        for (int k = 0; k < 200 && qc_zugang_phasen_offen(); k++) usleep(5 * 1000);
    }
    usleep(100 * 1000);
    stdout_stumm(0);
    int z_noetig = zeilen_mit(pfad, "Zugang noetig: 127.0.0.1 (ID ");
    int z_ende = zeilen_mit(pfad, "hat die Verbindung beendet");
    printf("         (%d Handschlaege mit fremden Schluesseln, %d mit \"QCA1\", %d Zeilen 'Zugang noetig', %d 'beendet', "
           "zurueckgehalten %ld + %ld)\n", gelungen, qca1, z_noetig, z_ende, drossel_weitere(&d_zugang), drossel_weitere(&d_zugang_abbruch));
    pruefe(gelungen == 6 && qca1 == 6, "unbekannte Gegenstelle: Zugangsphase (\"QCA1\") statt stummen Schliessens");
    pruefe(z_noetig == 1 && drossel_weitere(&d_zugang) == 5 && z_ende == 1 && drossel_weitere(&d_zugang_abbruch) == 5,
           "Zugang noetig und Verbindungsende: je eine Zeile, der Rest gezaehlt");

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
// Pruefstand. Starten, Anhalten und Umstellen melden sofort Erfolg - das
// Umstellen nur, solange g_fake_umstellen_fehler nicht gesetzt ist (HDR:
// ScreenCaptureKit lehnt die HDR-Aufnahme ab).
static _Atomic int g_fake_umstellen_fehler = 0;
@interface FakeStrom : SCStream
@end
@implementation FakeStrom
- (void)startCaptureWithCompletionHandler:(void (^)(NSError *))h { if (h) h(nil); }
- (void)stopCaptureWithCompletionHandler:(void (^)(NSError *))h { if (h) h(nil); }
- (void)updateConfiguration:(SCStreamConfiguration *)c completionHandler:(void (^)(NSError *))h {
    (void)c;
    NSError *e = atomic_load(&g_fake_umstellen_fehler)
        ? [NSError errorWithDomain:@"hosttest" code:1 userInfo:@{ NSLocalizedDescriptionKey: @"Attrappe lehnt das Umstellen ab" }]
        : nil;
    if (h) h(e);
}
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
    qc_zugang_eintragen(b_pub, "hosttest B");
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
    qc_zugang_eintragen(c_pub, "hosttest C");
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
    dispatch_sync(g_capq, ^{ enc = encoder_start(3, 640, 360, 60, 10, 0); });
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
    qc_zugang_eintragen(d_pub, "hosttest D");
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
        encoder_start(3, 640, 360, 60, 10, 0);
        vorher = g_session;
        g_wechsel_aktiv = 1;
        codec_wechsel_abschliessen(4, 0, 3, 0, f420, 0, NO);
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
        codec_wechsel_abschliessen(3, 0, 4, 0, f420, 0, NO);
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
        codec_wechsel_abschliessen(5, 0, 3, 0, f420, 0, YES);
    });
    st = strom_jetzt();
    dispatch_sync(g_capq, ^{ rest = g_session; });
    stdout_stumm(0);
    pruefe(!wechsel_aktiv() && rest == NULL, "fehlgeschlagener Wechsel ohne Strom: nicht fuer immer unterwegs, keine Sitzung bleibt");
    g_cfg = nil;
    atomic_store(&g_codec_id, 0);
}

// ------------------------------- Codecwechsel mit anderem Aufnahmeformat

// Ein Bild im Aufnahmeformat fmt (xf44 oder 420f), jeder Bildpunkt gleich.
// Die Werte in 10 Bit; 420f bekommt sie auf 8 Bit gekuerzt, xf44 traegt sie
// in den oberen 10 Bit von 16. Farbangaben wie bei der Aufnahme (BT.709).
static CVPixelBufferRef formatpuffer(int w, int h, OSType fmt, int y10, int cb10, int cr10) {
    NSDictionary *attr = @{ (id)kCVPixelBufferIOSurfacePropertiesKey: @{} };
    CVPixelBufferRef pb = NULL;
    CVPixelBufferCreate(NULL, (size_t)w, (size_t)h, fmt, (__bridge CFDictionaryRef)attr, &pb);
    if (!pb) return NULL;
    int breit = fmt == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    CVPixelBufferLockBaseAddress(pb, 0);
    for (size_t e = 0; e < 2; e++) {
        uint8_t *basis = CVPixelBufferGetBaseAddressOfPlane(pb, e);
        size_t bpr = CVPixelBufferGetBytesPerRowOfPlane(pb, e);
        size_t pw = CVPixelBufferGetWidthOfPlane(pb, e), ph = CVPixelBufferGetHeightOfPlane(pb, e);
        for (size_t zeile = 0; zeile < ph; zeile++) {
            uint8_t *z = basis + zeile * bpr;
            for (size_t x = 0; x < pw * (e ? 2 : 1); x++) {
                int v = e == 0 ? y10 : (x & 1) ? cr10 : cb10;
                if (breit) ((uint16_t *)z)[x] = (uint16_t)(v << 6);
                else z[x] = (uint8_t)(v >> 2);
            }
        }
    }
    CVPixelBufferUnlockBaseAddress(pb, 0);
    CVBufferSetAttachment(pb, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVAttachmentMode_ShouldPropagate);
    CVBufferSetAttachment(pb, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2, kCVAttachmentMode_ShouldPropagate);
    CVBufferSetAttachment(pb, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2, kCVAttachmentMode_ShouldPropagate);
    return pb;
}

// Wert eines Bildpunkts in der Mitte, auf 10 Bit umgerechnet.
// ebene 0: Y; ebene 1: komp 0 = Cb, komp 1 = Cr.
static int bildpunkt(CVPixelBufferRef pb, size_t ebene, size_t komp) {
    int breit = CVPixelBufferGetPixelFormatType(pb) == kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    CVPixelBufferLockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    uint8_t *basis = CVPixelBufferGetBaseAddressOfPlane(pb, ebene);
    size_t bpr = CVPixelBufferGetBytesPerRowOfPlane(pb, ebene);
    size_t x = CVPixelBufferGetWidthOfPlane(pb, ebene) / 2, y = CVPixelBufferGetHeightOfPlane(pb, ebene) / 2;
    size_t i = ebene ? 2 * x + komp : x;
    int v = breit ? ((uint16_t *)(basis + y * bpr))[i] >> 6 : (basis + y * bpr)[i] << 2;
    CVPixelBufferUnlockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    return v;
}

static int nahe(int a, int b) { return abs(a - b) <= 8; }   // 2 Stufen in 8 Bit

// Wie bilder_lesen, zaehlt dazu die Wechselansagen (SWITCH).
static void wechsel_lesen(leser *l, int frist_ms, int *wechsel, int *bilder, int *vollbilder, int *erstes_voll) {
    qc_hdr m;
    uint8_t anf[8];
    while (nachricht(l, &m, anf, frist_ms) == 1) {
        if (m.type == QC_MSG_SWITCH) (*wechsel)++;
        if (m.type != QC_MSG_VIDEO) continue;
        int key = (m.flags & QC_FLAG_KEY) ? 1 : 0;
        if (*bilder == 0) *erstes_voll = key;
        (*bilder)++;
        *vollbilder += key;
    }
}

static OSType format_letztes(void) {
    __block OSType f = 0;
    dispatch_sync(g_capq, ^{ f = g_last_pb ? CVPixelBufferGetPixelFormatType(g_last_pb) : 0; });
    return f;
}

// Hinweis der Gegenpruefung (C9), im Protokoll des Hosts nach einem
// Codecwechsel 0 -> 3 gesehen: "Bild im Format xf44 verworfen, Encoder
// erwartet 420f". Ein Bild im alten Aufnahmeformat lag danach fest, der Takt
// scheiterte damit bei jedem Schlag, und bei stillem Bildschirm bekam der
// Zuschauer kein Vollbild im neuen Codec. Die Aufnahme ist eine Attrappe,
// die Encoder sind echt.
static void formatwechsel_pruefen(int bild_port) {
    printf("\n-- Codecwechsel mit anderem Aufnahmeformat bei stillem Bildschirm (echter Encoder, 640x360)\n");
    OSType f444 = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange, f420 = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;

    // a) Die Umrechnung selbst: Werte und Farbangaben bleiben, hin und zurueck.
    CVPixelBufferRef q = formatpuffer(64, 32, f444, 800, 300, 700);
    CVPixelBufferRef z = q ? bild_umrechnen(q, f420) : NULL;
    CVPixelBufferRef r = z ? bild_umrechnen(z, f444) : NULL;
    int ok_z = z && CVPixelBufferGetPixelFormatType(z) == f420 && CVPixelBufferGetWidth(z) == 64 && CVPixelBufferGetHeight(z) == 32 &&
               nahe(bildpunkt(z, 0, 0), 800) && nahe(bildpunkt(z, 1, 0), 300) && nahe(bildpunkt(z, 1, 1), 700);
    int ok_r = r && CVPixelBufferGetPixelFormatType(r) == f444 &&
               nahe(bildpunkt(r, 0, 0), 800) && nahe(bildpunkt(r, 1, 0), 300) && nahe(bildpunkt(r, 1, 1), 700);
    CFTypeRef mz = z ? CVBufferCopyAttachment(z, kCVImageBufferYCbCrMatrixKey, NULL) : NULL;
    int ok_m = mz && CFEqual(mz, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
    printf("         (xf44 Y/Cb/Cr 800/300/700 -> 420f %d/%d/%d -> xf44 %d/%d/%d, Matrix %s)\n",
           z ? bildpunkt(z, 0, 0) : -1, z ? bildpunkt(z, 1, 0) : -1, z ? bildpunkt(z, 1, 1) : -1,
           r ? bildpunkt(r, 0, 0) : -1, r ? bildpunkt(r, 1, 0) : -1, r ? bildpunkt(r, 1, 1) : -1,
           mz ? [(__bridge NSString *)mz UTF8String] : "keine");
    pruefe(ok_z && ok_r && ok_m, "Umrechnung xf44 -> 420f -> xf44: Groesse, Werte (auf 2 Stufen in 8 Bit) und Farbmatrix bleiben");
    if (mz) CFRelease(mz);
    if (q) CVPixelBufferRelease(q);
    if (z) CVPixelBufferRelease(z);
    if (r) CVPixelBufferRelease(r);

    // b) Ein Paar Kandidaten mit verschiedenem Aufnahmeformat.
    stdout_stumm(1);
    codecs_pruefen();
    stdout_stumm(0);
    int von = -1, nach = -1;
    for (int i = 0; i < 3 && von < 0; i++) if (g_befund[i].vorhanden) von = i;       // xf44
    for (int i = 3; i < 5 && nach < 0; i++) if (g_befund[i].vorhanden) nach = i;     // 420f
    if (von < 0 || nach < 0) { pruefe(0, "zwei Kandidaten mit verschiedenem Aufnahmeformat"); return; }
    printf("         (Wechsel zwischen Kandidat %d %s und %d %s)\n", von, g_kandidaten[von].name, nach, g_kandidaten[nach].name);

    atomic_store(&g_codec_id, von);
    g_info_w = 640; g_info_h = 360; g_info_fps = 60;
    atomic_store(&g_cur_fps, 60);
    atomic_store(&g_cur_mbit, 10);
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_cur_gaming, 0);
    atomic_store(&g_testbild, 0);
    g_cfg = [[SCStreamConfiguration alloc] init];
    g_cfg.pixelFormat = pixfmt_fuer(von);
    __block BOOL enc = NO;
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{ enc = encoder_start(von, 640, 360, 60, 10, 0); });
    stdout_stumm(0);
    CVPixelBufferRef still = formatpuffer(640, 360, f444, 600, 400, 620);
    CVPixelBufferRef nachz = formatpuffer(640, 360, f420, 600, 400, 620);
    CVPixelBufferRef echt = formatpuffer(640, 360, f444, 610, 400, 620);
    if (!enc || !still || !nachz || !echt) { pruefe(0, "Encoder und Bildpuffer"); return; }
    strom_attrappe_setzen();
    SCStream *fs = strom_jetzt();
    Grabber *grab = [[Grabber alloc] init];
    // Der Bildschirm ist seit einer Sekunde still; sein letztes Bild liegt fest.
    CMTime vor = CMTimeSubtract(uhr(), CMTimeMake(1, 1));
    dispatch_sync(g_capq, ^{
        g_last_pb = CVPixelBufferRetain(still);
        g_last_cap_us = cmtime_us(vor);
        g_last_ankunft_us = cmtime_us(vor);
        g_last_pts = vor;
        g_schlitz = 0;
        g_behelf = 0;
    });
    atomic_store(&g_bild_offen, 0);

    uint8_t e_priv[32], e_pub[32];
    qc_keypair(e_priv, e_pub);
    qc_zugang_eintragen(e_pub, "hosttest E");
    qc_cipher rx;
    stdout_stumm(1);
    int b = client_verbinden(bild_port, e_priv, &rx, NULL);
    leser l;
    leser_init(&l, b, 0);
    l.rx = rx;
    char magic[4];
    int ok = b >= 0 && klartext(&l, magic, 4, 2000) == 1;
    usleep(50 * 1000);
    takt(5);
    int w0 = 0, n0 = 0, v0 = 0, e0 = -1;
    wechsel_lesen(&l, 300, &w0, &n0, &v0, &e0);
    stdout_stumm(0);
    pruefe(ok && n0 == 1 && e0 == 1, "vor dem Wechsel: der Zuschauer hat sein Vollbild");

    // c) Wechsel ohne ein einziges Bild der Aufnahme, weder waehrend noch
    //    danach: bei stillem Bildschirm ist das letzte Bild von vorher alles,
    //    was es gibt.
    long vorher_n = atomic_load(&g_nachgereicht);
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{ codec_wechseln(nach); });
    dispatch_sync(g_capq, ^{});                      // Abschluss nach dem Umstellen der Attrappe
    int steht = atomic_load(&g_codec_id) == nach && !wechsel_aktiv();
    takt(5);
    int w1 = 0, n1 = 0, v1 = 0, e1 = -1;
    wechsel_lesen(&l, 300, &w1, &n1, &v1, &e1);
    stdout_stumm(0);
    OSType f1 = format_letztes();
    uint32_t f1be = CFSwapInt32HostToBig(f1);
    printf("         (%s -> %s: Ansage %d, danach %d Bild(er), das erste %s, %ld nachgereicht, letztes Bild jetzt %.4s)\n",
           g_kandidaten[von].name, g_kandidaten[nach].name, w1, n1,
           e1 == 1 ? "ein Vollbild" : e1 == 0 ? "ein Zwischenbild" : "-",
           atomic_load(&g_nachgereicht) - vorher_n, f1 ? (char *)&f1be : "----");
    pruefe(steht && w1 == 1 && n1 == 1 && e1 == 1 && f1 == f420 && !atomic_load(&g_wait_key),
           "stiller Bildschirm, kein neues Bild: der Zuschauer bekommt sein Vollbild im neuen Codec, genau einmal "
           "(das letzte Bild, umgerechnet)");

    // d) Zurueck, und diesmal liefert die Aufnahme nach dem Umstellen noch ein
    //    Bild im alten Format nach - der Fall aus dem Protokoll.
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{ codec_wechseln(von); });
    dispatch_sync(g_capq, ^{});
    steht = atomic_load(&g_codec_id) == von && !wechsel_aktiv();
    CMSampleBufferRef s_nachz = aufnahme_bild(nachz, uhr());
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s_nachz ofType:SCStreamOutputTypeScreen]; });
    usleep(30 * 1000);                               // eine Bildzeit ohne Neues
    takt(5);
    int w2 = 0, n2 = 0, v2 = 0, e2 = -1;
    wechsel_lesen(&l, 300, &w2, &n2, &v2, &e2);
    stdout_stumm(0);
    OSType f2 = format_letztes();
    __block int behelf = 0;
    dispatch_sync(g_capq, ^{ behelf = g_behelf; });
    printf("         (%s -> %s mit Nachzuegler in 420f: Ansage %d, danach %d Bild(er), das erste %s)\n",
           g_kandidaten[nach].name, g_kandidaten[von].name, w2, n2,
           e2 == 1 ? "ein Vollbild" : e2 == 0 ? "ein Zwischenbild" : "-");
    pruefe(steht && w2 == 1 && n2 == 1 && e2 == 1 && f2 == f444 && behelf,
           "Nachzuegler im alten Format nach dem Wechsel: er liegt nicht fest, der Zuschauer bekommt sein Vollbild");

    // e) Das erste echte Bild danach ersetzt den Behelf als Vollbild, das
    //    naechste ist wieder ein gewoehnliches Zwischenbild.
    CMSampleBufferRef s_echt1 = aufnahme_bild(echt, uhr());
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s_echt1 ofType:SCStreamOutputTypeScreen]; });
    int w3 = 0, n3 = 0, v3 = 0, e3 = -1;
    stdout_stumm(1);
    wechsel_lesen(&l, 300, &w3, &n3, &v3, &e3);
    CMSampleBufferRef s_echt2 = aufnahme_bild(echt, uhr());
    dispatch_sync(g_capq, ^{ [grab stream:fs didOutputSampleBuffer:s_echt2 ofType:SCStreamOutputTypeScreen]; });
    int w4 = 0, n4 = 0, v4 = 0, e4 = -1;
    wechsel_lesen(&l, 300, &w4, &n4, &v4, &e4);
    stdout_stumm(0);
    dispatch_sync(g_capq, ^{ behelf = g_behelf; });
    printf("         (erstes echtes Bild danach: %s, das naechste: %s)\n",
           e3 == 1 ? "Vollbild" : e3 == 0 ? "Zwischenbild" : "-", e4 == 1 ? "Vollbild" : e4 == 0 ? "Zwischenbild" : "-");
    pruefe(n3 == 1 && e3 == 1 && n4 == 1 && e4 == 0 && !behelf,
           "das erste echte Bild nach dem umgerechneten kommt als Vollbild, danach wieder Zwischenbilder");

    zuschauer_weg();
    stdout_stumm(1);
    stream_herunterfahren_anstossen();
    SCStream *st = strom_jetzt();
    stdout_stumm(0);
    pruefe(st == nil && !g_session && format_letztes() == 0, "ohne Zuschauer: Strom, Encoder und letztes Bild weg");
    atomic_store(&g_bild_offen, 0);
    close(b); free(l.buf);
    CFRelease(s_nachz); CFRelease(s_echt1); CFRelease(s_echt2);
    CVPixelBufferRelease(still); CVPixelBufferRelease(nachz); CVPixelBufferRelease(echt);
    g_cfg = nil;
    atomic_store(&g_codec_id, 0);
}

// ------------------------------------------------ --fest beim Start (C10)

static void fest_pruefen(void) {
    printf("\n-- Feste Bildrate beim Start\n");
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_fixed_gewollt, 0);
    fest_einlesen(@[@"quadchroma-host", @"--serve", @"9001", @"--fps", @"120", @"--mbit", @"50"]);
    int ohne_cur = atomic_load(&g_cur_fixed), ohne_gewollt = atomic_load(&g_fixed_gewollt);
    fest_einlesen(@[@"quadchroma-host", @"--serve", @"9001", @"--fest"]);
    int mit = atomic_load(&g_cur_fixed) == 1 && atomic_load(&g_fixed_gewollt) == 1;
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_fixed_gewollt, 0);
    fest_einlesen(@[@"quadchroma-host", @"--fixed"]);
    int mit2 = atomic_load(&g_cur_fixed) == 1 && atomic_load(&g_fixed_gewollt) == 1;
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_fixed_gewollt, 0);
    printf("         (ohne --fest: g_cur_fixed %d, g_fixed_gewollt %d)\n", ohne_cur, ohne_gewollt);
    pruefe(ohne_cur == 0 && ohne_gewollt == 0,
           "ohne --fest ist die feste Bildrate aus - auch der Wunsch, den jeder neue Strom uebernimmt");
    pruefe(mit && mit2, "mit --fest oder --fixed ist sie an");
}


// ------------------------------------------- Dateien ueber die Zwischenablage

// Statt qc_clip_set_dateien: was in die Ablage gelegt wuerde.
static NSMutableArray<NSArray<NSString *> *> *g_rekorder;

static void rekorder(NSArray<NSString *> *pfade) {
    @synchronized (g_rekorder) { [g_rekorder addObject:pfade]; }
}

static NSUInteger rekorder_anzahl(void) {
    @synchronized (g_rekorder) { return g_rekorder.count; }
}

// Eingabekanal wie der Client: Handschlag mit dem Prologue des Eingabekanals
// und der Pruefsumme des Bildkanals. tx bekommt den Sendeschluessel.
static int eingabe_verbinden(int port, const uint8_t priv[32], const uint8_t hh[QC_HASHLEN], qc_cipher *tx) {
    int c = verbinden(port);
    if (c < 0) return -1;
    uint8_t pro[256];
    size_t pl = strlen(QC_PRO_INPUT);
    memcpy(pro, QC_PRO_INPUT, pl);
    memcpy(pro + pl, hh, QC_HASHLEN);
    pl += QC_HASHLEN;
    qc_handshake hs;
    qc_handshake_init(&hs, 1, priv, pro, pl);
    uint8_t msg[8192], nutz[8192];
    size_t ml = 0, nl = 0;
    if (qc_handshake_write(&hs, NULL, 0, msg, &ml) || rahmen_schreiben(c, msg, ml) ||
        rahmen_lesen(c, msg, sizeof msg, &ml) || qc_handshake_read(&hs, msg, ml, nutz, &nl) ||
        qc_handshake_write(&hs, NULL, 0, msg, &ml) || rahmen_schreiben(c, msg, ml)) {
        close(c);
        return -1;
    }
    qc_cipher empfang;
    qc_handshake_split(&hs, tx, &empfang);
    return c;
}

// Eine Nachricht auf dem Eingabekanal, verschluesselt in Datensaetzen wie qc_chan_send.
static int ein_senden(int fd, qc_cipher *tx, uint8_t typ, const void *p, size_t n) {
    size_t gesamt = sizeof(qc_hdr) + n;
    uint8_t *klar = malloc(gesamt);
    qc_hdr h = { .type = typ, .flags = 0, .reserved = 0, .len = (uint32_t)n };
    memcpy(klar, &h, sizeof h);
    if (n) memcpy(klar + sizeof h, p, n);
    static uint8_t ct[QC_CHUNK_MAX + QC_TAGLEN];
    int r = 0;
    for (size_t o = 0; o < gesamt && !r;) {
        size_t w = gesamt - o < QC_CHUNK_MAX ? gesamt - o : QC_CHUNK_MAX;
        size_t cl = 0;
        r = qc_encrypt(tx, klar + o, w, ct, &cl) ? -1 : rahmen_schreiben(fd, ct, cl);
        o += w;
    }
    free(klar);
    return r;
}

static int ein_daten(int fd, qc_cipher *tx, uint8_t typ, NSData *d) { return ein_senden(fd, tx, typ, d.bytes, d.length); }

// Wie nachricht, aber mit der ganzen Nutzlast.
static int nachricht_ganz(leser *l, qc_hdr *h, NSData **nutzlast, int frist_ms) {
    for (;;) {
        if (l->len >= sizeof *h) {
            memcpy(h, l->buf, sizeof *h);
            if (l->len >= sizeof *h + h->len) {
                *nutzlast = [NSData dataWithBytes:l->buf + sizeof *h length:h->len];
                size_t weg = sizeof *h + h->len;
                memmove(l->buf, l->buf + weg, l->len - weg);
                l->len -= weg;
                return 1;
            }
        }
        int r = datensatz(l, frist_ms);
        if (r != 1) return r;
    }
}

// Liest bis zur naechsten Quittung (andere Nachrichten werden uebergangen).
// 1 = gelesen, sonst wie nachricht.
static int quittung_lesen(leser *l, uint8_t *zustand, uint64_t *empfangen, int frist_ms) {
    qc_hdr h;
    NSData *d = nil;
    int r;
    while ((r = nachricht_ganz(l, &h, &d, frist_ms)) == 1) {
        uint32_t k;
        if (h.type == QC_DATEI_QUITTUNG && qc_datei_quittung_lesen(d.bytes, d.length, &k, zustand, empfangen) == 0) return 1;
    }
    return r;
}

static int faehig_jetzt(void) {
    pthread_mutex_lock(&g_send_mtx);
    int f = dateien_faehig_gesperrt();
    pthread_mutex_unlock(&g_send_mtx);
    return f;
}

static int warten_bis(int (*bedingung)(void), double sekunden) {
    double t0 = sek();
    while (!bedingung() && sek() - t0 < sekunden) usleep(10 * 1000);
    return bedingung();
}

static int eingabe_steht(void) { return atomic_load(&g_in_fd) >= 0; }
static int eingabe_weg(void) { return atomic_load(&g_in_fd) < 0; }
static int empfang_ruht(void) { return qc_empfang_ordner() == nil; }
static int sender_ruht(void) { return !qc_senden_laeuft(); }

// Ein Zuschauer mit Bild- und Eingabekanal ueber die echte Annahme. liste:
// die Bildschirmliste (12) aus der Begruessung.
typedef struct {
    int bild, ein; leser l; qc_cipher tx; uint8_t hh[QC_HASHLEN]; char folge[64];
    uint8_t liste[QC_BILDSCHIRM_LISTE_MAX]; size_t liste_n;
    uint8_t info[QC_HDR_INFO_LAENGE]; size_t info_n;      // Strominfo der Begruessung
} schein;

// Vor dem Verbinden einen Strom vortaeuschen (dann laeuft stream_hochfahren_sync
// nicht) - der Abschnitt Bildschirm schaltet das ab und laesst den echten
// Aufnahmestart mit seinen Attrappen laufen.
static int g_schein_attrappe = 1;

static int schein_verbinden(schein *s, int bild_port, int ein_port, const uint8_t priv[32]) {
    memset(s, 0, sizeof *s);
    if (g_schein_attrappe) strom_attrappe_setzen();
    qc_cipher rx;
    s->bild = client_verbinden(bild_port, priv, &rx, s->hh);
    s->ein = -1;
    if (s->bild < 0) return -1;
    leser_init(&s->l, s->bild, 0);
    s->l.rx = rx;
    char magic[4];
    if (klartext(&s->l, magic, 4, 2000) != 1) return -1;
    // Was nach der Begruessung kommt, bis einschliesslich der Bildschirmliste;
    // die Faehigkeiten davor muessen genau 7 sein (Dateien, Bildschirmwahl, HDR).
    qc_hdr h;
    NSData *d = nil;
    size_t n = 0;
    while (nachricht_ganz(&s->l, &h, &d, 1000) == 1 && n + 4 < sizeof s->folge) {
        n += (size_t)snprintf(s->folge + n, sizeof s->folge - n, "%s%d", n ? " " : "", h.type);
        if (h.type == QC_MSG_INFO && d.length <= sizeof s->info) {
            memcpy(s->info, d.bytes, d.length);
            s->info_n = d.length;
        }
        if (h.type == QC_MSG_FAEHIGKEITEN) {
            uint32_t bits = 0;
            if (d.length != 4 || qc_datei_faehigkeiten_lesen(d.bytes, d.length, &bits) != 0 ||
                bits != (QC_FAEHIG_DATEIEN | QC_FAEHIG_BILDSCHIRM | QC_FAEHIG_HDR)) return -1;
        }
        if (h.type == QC_MSG_BILDSCHIRME) {
            if (d.length > sizeof s->liste) return -1;
            memcpy(s->liste, d.bytes, d.length);
            s->liste_n = d.length;
            break;
        }
    }
    if (!s->liste_n) return -1;
    s->ein = eingabe_verbinden(ein_port, priv, s->hh, &s->tx);
    if (s->ein < 0 || !warten_bis(eingabe_steht, 2)) return -1;
    return 0;
}

static void schein_schliessen(schein *s) {
    if (s->ein >= 0) close(s->ein);
    if (s->bild >= 0) close(s->bild);
    free(s->l.buf);
    memset(s, 0, sizeof *s);
}

static NSData *zufall(size_t n) {
    NSMutableData *d = [NSMutableData dataWithLength:n];
    arc4random_buf(d.mutableBytes, n);
    return d;
}

// Client -> Host: Angebot wie in dateitest, mit Ordnern, einer leeren Datei
// und einer Datei ueber mehrere Stuecke.
static NSArray *g_inhalte;
static NSData *beispiel_angebot(uint32_t kennung) {
    g_inhalte = @[ [NSNull null], [@"hallo" dataUsingEncoding:NSUTF8StringEncoding], [NSData data], zufall(150000),
                   [NSNull null], [@"abc" dataUsingEncoding:NSUTF8StringEncoding] ];
    return qc_datei_angebot_kodieren(kennung, @[ [QCDateiEintrag art:1 groesse:0 pfad:@"Bilder"],
                                                 [QCDateiEintrag art:0 groesse:5 pfad:@"Bilder/a.txt"],
                                                 [QCDateiEintrag art:0 groesse:0 pfad:@"b.bin"],
                                                 [QCDateiEintrag art:0 groesse:150000 pfad:@"gross.bin"],
                                                 [QCDateiEintrag art:1 groesse:0 pfad:@"Bilder/sub"],
                                                 [QCDateiEintrag art:0 groesse:3 pfad:@"Bilder/sub/c.txt"] ]);
}

// Die Stuecke des Beispiels, hoechstens `bis` Stueck (0 = alle).
static int beispiel_stuecke(schein *s, uint32_t kennung, int bis) {
    int n = 0;
    for (NSUInteger i = 0; i < g_inhalte.count; i++) {
        if (g_inhalte[i] == [NSNull null]) continue;
        NSData *d = g_inhalte[i];
        for (NSUInteger o = 0; o < d.length; o += QC_DATEI_STUECK_MAX) {
            if (bis && n >= bis) return 0;
            NSUInteger m = d.length - o < QC_DATEI_STUECK_MAX ? d.length - o : QC_DATEI_STUECK_MAX;
            NSData *st = qc_datei_stueck_kodieren(kennung, (uint32_t)i, o, (const uint8_t *)d.bytes + o, m);
            if (ein_daten(s->ein, &s->tx, QC_DATEI_STUECK, st)) return -1;
            n++;
        }
    }
    return 0;
}

// Host -> Client, wie ein Client mit Empfaenger: liest Angebot und Stuecke,
// quittiert je 64 KiB, am Ende mit 1. zurueckhalten: die ersten Stuecke erst
// quittieren, wenn 300 ms nichts mehr kommt (so zeigt sich das Fenster).
// *stoss = so viel kam vor der ersten Quittung; *unterwegs = hoechstens
// unquittiert. Rueckgabe: Inhalte je Eintrag (nil bei Fehler), *ende = Grund.
static NSMutableDictionary<NSNumber *, NSMutableData *> *client_empfangen(schein *s, BOOL zurueckhalten, uint64_t *stoss,
                                                                          uint64_t *unterwegs, int *ende, NSArray **pfade) {
    NSMutableDictionary<NSNumber *, NSMutableData *> *inhalt = [NSMutableDictionary dictionary];
    uint32_t kennung = 0;
    uint64_t erhalten = 0, quittiert = 0;
    BOOL erste = zurueckhalten;
    *stoss = 0; *unterwegs = 0; *ende = -1;
    for (;;) {
        qc_hdr h;
        NSData *d = nil;
        int r = nachricht_ganz(&s->l, &h, &d, erste && kennung ? 300 : 3000);
        if (r != 1) {
            if (!(erste && kennung)) return nil;
            // Nichts mehr ohne Quittung: das Fenster ist ausgeschoepft.
            erste = NO;
            *stoss = erhalten;
            quittiert = erhalten;
            ein_daten(s->ein, &s->tx, QC_DATEI_QUITTUNG, qc_datei_quittung_kodieren(kennung, 0, erhalten));
            continue;
        }
        if (h.type == QC_DATEI_ANGEBOT) {
            NSArray<QCDateiEintrag *> *e = nil;
            uint64_t g;
            if (qc_datei_angebot_lesen(d.bytes, d.length, &kennung, &g, &e, NULL) != 0) return nil;
            NSMutableArray *p = [NSMutableArray array];
            for (QCDateiEintrag *x in e) [p addObject:[x.teile componentsJoinedByString:@"/"]];
            if (pfade) *pfade = p;
            if (!erste) ein_daten(s->ein, &s->tx, QC_DATEI_QUITTUNG, qc_datei_quittung_kodieren(kennung, 0, 0));
        } else if (h.type == QC_DATEI_STUECK) {
            uint32_t k, e; uint64_t v; const uint8_t *p; size_t n;
            if (qc_datei_stueck_lesen(d.bytes, d.length, &k, &e, &v, &p, &n) != 0 || k != kennung) return nil;
            NSMutableData *m = inhalt[@(e)] ?: (inhalt[@(e)] = [NSMutableData data]);
            if (m.length != v) return nil;
            [m appendBytes:p length:n];
            erhalten += n;
            if (erhalten - quittiert > *unterwegs) *unterwegs = erhalten - quittiert;
            if (!erste && erhalten - quittiert >= QC_DATEI_QUITTUNG_ALLE) {
                quittiert = erhalten;
                ein_daten(s->ein, &s->tx, QC_DATEI_QUITTUNG, qc_datei_quittung_kodieren(kennung, 0, erhalten));
            }
        } else if (h.type == QC_DATEI_ENDE) {
            uint32_t k; uint8_t g;
            if (qc_datei_ende_lesen(d.bytes, d.length, &k, &g) != 0 || k != kennung) return nil;
            *ende = g;
            if (g == 0) ein_daten(s->ein, &s->tx, QC_DATEI_QUITTUNG, qc_datei_quittung_kodieren(kennung, 1, erhalten));
            return inhalt;
        }
    }
}

// Alle Nachrichten der naechsten frist_ms: wie viele davon Dateien (50-52).
static int datei_nachrichten(schein *s, int frist_ms) {
    int n = 0;
    qc_hdr h;
    NSData *d;
    while (nachricht_ganz(&s->l, &h, &d, frist_ms) == 1)
        if (h.type == QC_DATEI_ANGEBOT || h.type == QC_DATEI_STUECK || h.type == QC_DATEI_ENDE) n++;
    return n;
}

// Alle Nachrichten der naechsten frist_ms: wie viele davon vom Typ typ.
static int nachrichten_vom_typ(schein *s, uint8_t typ, int frist_ms) {
    int n = 0;
    qc_hdr h;
    NSData *d;
    while (nachricht_ganz(&s->l, &h, &d, frist_ms) == 1)
        if (h.type == typ) n++;
    return n;
}

static uint64_t sitzung_jetzt(void) {
    pthread_mutex_lock(&g_send_mtx);
    uint64_t s = g_sitzung;
    pthread_mutex_unlock(&g_send_mtx);
    return s;
}

static void dateien_pruefen(int bild_port, int ein_port) {
    printf("\n-- Dateien: Faehigkeiten\n");
    static char pfad[1100];                 // g_log_pfad zeigt danach noch hierher
    snprintf(pfad, sizeof pfad, "%s/dateien.log", g_home);
    g_log_pfad = pfad;
    pthread_mutex_lock(&g_log_mtx);
    log_oeffnen("w");
    pthread_mutex_unlock(&g_log_mtx);
    g_rekorder = [NSMutableArray array];
    dateien_einrichten();
    NSString *home = @(g_home);
    qc_dateien_basis_setzen([home stringByAppendingPathComponent:@"ablage"]);
    qc_dateien_fertig_setzen(rekorder);
    atomic_store(&g_cur_gaming, 0);

    uint8_t f_priv[32], f_pub[32], g_priv[32], g_pub[32];
    qc_keypair(f_priv, f_pub);
    qc_keypair(g_priv, g_pub);
    qc_zugang_eintragen(f_pub, "hosttest F");
    qc_zugang_eintragen(g_pub, "hosttest G");
    schein F, G;
    stdout_stumm(1);
    int ok = schein_verbinden(&F, bild_port, ein_port, f_priv) == 0;
    int vorher = faehig_jetzt();
    NSData *eins = qc_datei_faehigkeiten_kodieren(1);
    ein_daten(F.ein, &F.tx, QC_IN_FAEHIGKEITEN, eins);
    int nachher = warten_bis(faehig_jetzt, 1);
    stdout_stumm(0);
    printf("         (nach der Begruessung: %s)\n", F.folge);
    pruefe(ok && strstr(F.folge, "3 8 11 12"), "MSG_FAEHIGKEITEN (11) mit Bit 0, 1 und 2 kommt nach Einstellungen und Codecliste, dann die Bildschirmliste (12)");
    pruefe(!vorher && nachher, "IN_FAEHIGKEITEN wird fuer diese Sitzung und diesen Eingabekanal gemerkt");

    printf("\n-- Dateien: Client -> Host ueber die echten Kanaele\n");
    stdout_stumm(1);
    ein_daten(F.ein, &F.tx, QC_DATEI_ANGEBOT, beispiel_angebot(77));
    uint8_t z = 9; uint64_t em = 9;
    int q0 = quittung_lesen(&F.l, &z, &em, 2000) == 1 && z == 0 && em == 0;
    NSMutableString *folge = [NSMutableString string];
    beispiel_stuecke(&F, 77, 0);
    ein_daten(F.ein, &F.tx, QC_DATEI_ENDE, qc_datei_ende_kodieren(77, 0));
    // Wie viele Zwischenquittungen kommen, haengt davon ab, wann der Eingang
    // leer ist (dann quittiert der Empfaenger sofort); fest steht: steigend,
    // spaetestens je 64 KiB (der Empfaenger quittiert ab 16 KiB offen, ein
    // Stueck hat hoechstens 48 KiB), am Ende 1 mit allen Bytes.
    BOOL folge_ok = YES;
    uint64_t vorige = 0;
    int zwischen = 0;
    z = 9;
    while (quittung_lesen(&F.l, &z, &em, 3000) == 1) {
        [folge appendFormat:@"%s%u/%llu", folge.length ? " " : "", z, em];
        // Zwischenquittungen steigen echt; die letzte darf den Stand der
        // vorigen wiederholen (war der Eingang nach dem letzten Stueck leer).
        if (em < vorige || (em == vorige && !z) || em - vorige > QC_DATEI_QUITTUNG_ALLE) folge_ok = NO;
        vorige = em;
        if (z) break;
        zwischen++;
    }
    qc_dateien_abwarten();                  // die Protokollzeile des Empfaengers ist geschrieben
    stdout_stumm(0);
    printf("         (Quittungen auf dem Bildkanal nach 0/0: %s)\n", folge.UTF8String);
    pruefe(q0 && folge_ok && zwischen >= 2 && z == QC_QUITT_FERTIG && em == 150008,
           "Quittungen kommen auf dem Bildkanal: sofort 0/0, dann steigend und spaetestens je 64 KiB, am Ende 1 mit allen Bytes");
    NSArray<NSString *> *liste = nil;
    @synchronized (g_rekorder) { liste = g_rekorder.lastObject; }
    NSString *wurzel = liste.firstObject.stringByDeletingLastPathComponent;
    BOOL bytes = liste.count == 3 && [liste[0].lastPathComponent isEqualToString:@"Bilder"] &&
                 [liste[1].lastPathComponent isEqualToString:@"b.bin"] && [liste[2].lastPathComponent isEqualToString:@"gross.bin"] &&
                 [[NSData dataWithContentsOfFile:[wurzel stringByAppendingPathComponent:@"Bilder/a.txt"]] isEqualToData:g_inhalte[1]] &&
                 [[NSData dataWithContentsOfFile:[wurzel stringByAppendingPathComponent:@"gross.bin"]] isEqualToData:g_inhalte[3]] &&
                 [[NSData dataWithContentsOfFile:[wurzel stringByAppendingPathComponent:@"Bilder/sub/c.txt"]] isEqualToData:g_inhalte[5]];
    pruefe(rekorder_anzahl() == 1 && bytes && [wurzel hasPrefix:[home stringByAppendingPathComponent:@"ablage/"]],
           "der Rekorder (statt der Ablage) bekommt die obersten Pfade, die Bytes stimmen");
    pruefe(zeilen_mit(pfad, "Dateien: empfangen 6 Eintraege, 0,2 MB - in die Ablage gelegt") == 1 &&
           zeilen_mit(pfad, "Dateien: empfange 6 Eintraege, 0,2 MB") == 1,
           "Protokollzeilen im Host-Protokoll (beim Annehmen und am Ende, wie beim Windows-Host)");

    // Ein Stueck ausser der Reihe: Quittung 4, der Kanal bleibt (Zeitabgleich geht).
    stdout_stumm(1);
    ein_daten(F.ein, &F.tx, QC_DATEI_ANGEBOT, beispiel_angebot(78));
    int q78 = quittung_lesen(&F.l, &z, &em, 2000) == 1 && z == 0;
    ein_daten(F.ein, &F.tx, QC_DATEI_STUECK, qc_datei_stueck_kodieren(78, 3, 0, "xyz", 3));
    int q4 = quittung_lesen(&F.l, &z, &em, 2000) == 1 && z == QC_QUITT_UNGUELTIG;
    uint64_t t_client = 4242;
    ein_senden(F.ein, &F.tx, QC_IN_TIME, &t_client, 8);
    qc_hdr h;
    NSData *d = nil;
    int zeit = 0;
    while (!zeit && nachricht_ganz(&F.l, &h, &d, 2000) == 1) zeit = h.type == QC_MSG_TIME;
    stdout_stumm(0);
    pruefe(q78 && q4 && zeit && warten_bis(empfang_ruht, 1), "Stueck ausser der Reihe: Quittung 4, der Eingabekanal bleibt bestehen");

    printf("\n-- Dateien: Host -> Client (Fenster)\n");
    NSString *q = [home stringByAppendingPathComponent:@"quelle"];
    NSData *x_bin = zufall(600000), *y_bin = zufall(200000);
    [[NSFileManager defaultManager] createDirectoryAtPath:[q stringByAppendingPathComponent:@"Q"] withIntermediateDirectories:YES
                                               attributes:nil error:nil];
    [x_bin writeToFile:[q stringByAppendingPathComponent:@"Q/x.bin"] atomically:NO];
    [[NSData data] writeToFile:[q stringByAppendingPathComponent:@"Q/leer.txt"] atomically:NO];
    [[@"12345" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:[q stringByAppendingPathComponent:@"einzeln.txt"] atomically:NO];
    [y_bin writeToFile:[q stringByAppendingPathComponent:@"y.bin"] atomically:NO];
    // Eine Verknuepfung mit Umlauten im Namen: sie wird uebersprungen, und die
    // Zeile dazu muss den Namen unverfaelscht in UTF-8 tragen (nicht als
    // MacRoman gelesen). Name in NFC, als Bytes.
    char verkn[1200];
    snprintf(verkn, sizeof verkn, "%s/Q/Verkn\xc3\xbcpfung \xc3\xa4", q.fileSystemRepresentation);
    int verkn_da = symlink("x.bin", verkn) == 0;
    uint64_t stoss = 0, unterwegs = 0;
    int ende = -1;
    NSArray *pfade = nil;
    stdout_stumm(1);
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"Q"], [q stringByAppendingPathComponent:@"einzeln.txt"] ]);
    NSDictionary<NSNumber *, NSData *> *inhalt = client_empfangen(&F, YES, &stoss, &unterwegs, &ende, &pfade);
    qc_dateien_abwarten();
    stdout_stumm(0);
    printf("         (Angebot: %s; vor der ersten Quittung %llu Byte, hoechstens %llu unquittiert)\n",
           [pfade componentsJoinedByString:@", "].UTF8String, stoss, unterwegs);
    pruefe([pfade isEqualToArray:(@[ @"Q", @"Q/leer.txt", @"Q/x.bin", @"einzeln.txt" ])] && ende == 0 &&
           [inhalt[@2] isEqualToData:x_bin] && [inhalt[@3] isEqualToData:[@"12345" dataUsingEncoding:NSUTF8StringEncoding]] &&
           !inhalt[@1],
           "Dateiliste in den Sender: Angebot, Stuecke und ENDE kommen beim Scheinclient an, die Bytes stimmen");
    pruefe(stoss == QC_DATEI_FENSTER && unterwegs <= QC_DATEI_FENSTER, "das Fenster wird eingehalten (256 KiB ohne Quittung, nie mehr)");
    pruefe(zeilen_mit(pfad, "Dateien: sende 4 Eintraege, 0,6 MB an 127.0.0.1") == 1 &&
           zeilen_mit(pfad, "Dateien: gesendet und quittiert (0,6 MB in ") == 1, "Protokollzeilen wie beim Windows-Host");
    // NFC wie angelegt oder NFD, falls das Dateisystem den Namen so meldet;
    // falsch gelesen stuende dort statt jedes Umlauts eine Zeichenfolge wie
    // E2 88 9A C2 BA.
    int umlaut = zeilen_mit(pfad, "uebersprungen (symbolische Verknuepfung): ") == 1 &&
                 zeilen_mit(pfad, "/Q/Verkn\xc3\xbcpfung \xc3\xa4") + zeilen_mit(pfad, "/Q/Verknu\xcc\x88pfung a\xcc\x88") == 1;
    pruefe(verkn_da && umlaut, "Host-Protokoll: Namen mit Umlauten stehen unverfaelscht (UTF-8) in der Zeile");
    atomic_store(&g_cur_gaming, 1);
    stdout_stumm(1);
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"y.bin"] ]);
    inhalt = client_empfangen(&F, YES, &stoss, &unterwegs, &ende, &pfade);
    qc_dateien_abwarten();
    stdout_stumm(0);
    atomic_store(&g_cur_gaming, 0);
    pruefe(stoss == QC_DATEI_FENSTER_SPIEL && ende == 0 && [inhalt[@0] isEqualToData:y_bin], "im Spielmodus 64 KiB");

    printf("\n-- Dateien: Zuschauerwechsel mitten in der Uebertragung\n");
    NSData *gross = zufall(2000000);
    [gross writeToFile:[q stringByAppendingPathComponent:@"gross.bin"] atomically:NO];
    stdout_stumm(1);
    // Beide Richtungen laufen, der Client quittiert nichts mehr.
    ein_daten(F.ein, &F.tx, QC_DATEI_ANGEBOT, beispiel_angebot(79));
    beispiel_stuecke(&F, 79, 2);
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"gross.bin"] ]);
    usleep(300 * 1000);
    NSString *laufend = qc_empfang_ordner();
    int sendet = qc_senden_laeuft();
    NSUInteger rek0 = rekorder_anzahl();
    uint64_t s_f = sitzung_jetzt();
    ok = schein_verbinden(&G, bild_port, ein_port, g_priv) == 0;     // G loest F ab
    uint64_t s_g = sitzung_jetzt();
    int empfang_weg = warten_bis(empfang_ruht, 2), sender_weg = warten_bis(sender_ruht, 2);
    int bei_g = datei_nachrichten(&G, 300);
    // Die Sitzungsbindung des Wegs fuer sich: mit der Sitzung des Vorgaengers
    // geht nichts hinaus (so auch keine Quittung eines Empfaengers, der den
    // Wechsel noch nicht bemerkt hat), mit der des Neuen genau das eine.
    NSData *qt = qc_datei_quittung_kodieren(4711, 0, 1);
    int alt_raus = send_small_sitzung(s_f, QC_DATEI_QUITTUNG, qt.bytes, qt.length);
    int alt_rueckstand = rueckstand_sitzung(s_f);
    int neu_raus = send_small_sitzung(s_g, QC_DATEI_QUITTUNG, qt.bytes, qt.length);
    int quittungen_bei_g = nachrichten_vom_typ(&G, QC_DATEI_QUITTUNG, 300);
    stdout_stumm(0);
    printf("         (vorher: Empfang in %s, Sender %s)\n", laufend.lastPathComponent.UTF8String ?: "-", sendet ? "laeuft" : "ruht");
    pruefe(ok && laufend && empfang_weg && ![[NSFileManager defaultManager] fileExistsAtPath:laufend] && rekorder_anzahl() == rek0,
           "Client -> Host: der Neue loest ab, die Uebertragung wird verworfen und ihr Verzeichnis geloescht");
    pruefe(sendet && sender_weg && bei_g == 0, "Host -> Client: der Sender bricht ab, beim Neuen kommt nichts davon an");
    pruefe(zeilen_mit(pfad, "Dateien: abgebrochen (Zuschauer gewechselt oder weg)") == 2,
           "beide Abbrueche stehen im Protokoll");
    pruefe(s_f != s_g && !alt_raus && alt_rueckstand == -1 && neu_raus && quittungen_bei_g == 1,
           "send_small_sitzung mit der Sitzung des Vorgaengers: nichts geht hinaus, Rueckstand -1; mit der des Neuen genau eine Nachricht");

    printf("\n-- Dateien: ohne Eingabekanal, dann aelterer Client (ohne IN_FAEHIGKEITEN)\n");
    stdout_stumm(1);
    // Erst ohne Eingabekanal: das ist kein aelterer Client, der Hinweis
    // dafuer bleibt unverbraucht.
    close(G.ein);
    G.ein = -1;
    int ohne = warten_bis(eingabe_weg, 2);
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"einzeln.txt"] ]);
    int ohne_zeilen = zeilen_mit(pfad, "Dateien: nicht gesendet (Zuschauer ohne Eingabekanal)") == 1 &&
                      zeilen_mit(pfad, "aelterer Client") == 0;
    G.ein = eingabe_verbinden(ein_port, g_priv, G.hh, &G.tx);
    int wieder = G.ein >= 0 && warten_bis(eingabe_steht, 2);
    int g_faehig = faehig_jetzt();
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"einzeln.txt"] ]);
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"einzeln.txt"] ]);
    bei_g = datei_nachrichten(&G, 400);
    qc_dateien_abwarten();
    stdout_stumm(0);
    pruefe(ohne && ohne_zeilen && wieder,
           "Zuschauer ohne Eingabekanal: eigene Zeile statt \"aelterer Client\", der Hinweis bleibt unverbraucht");
    pruefe(!g_faehig && bei_g == 0 && !qc_senden_laeuft(), "ohne IN_FAEHIGKEITEN geht nichts hinaus");
    pruefe(zeilen_mit(pfad, "Dateien: Zuschauer kann keine Dateien empfangen (aelterer Client)") == 1,
           "die Protokollzeile kommt einmal je Sitzung");

    printf("\n-- Dateien: Obergrenzen auf dem Eingabekanal\n");
    stdout_stumm(1);
    ein_daten(G.ein, &G.tx, QC_IN_FAEHIGKEITEN, eins);
    int g_jetzt = warten_bis(faehig_jetzt, 1);
    // Ein Stueck, eins groesser als 16 + 49152: der Kanal endet.
    NSMutableData *zu_gross = [NSMutableData dataWithLength:QC_DATEI_STUECK_KOPF + QC_DATEI_STUECK_MAX + 1];
    ein_daten(G.ein, &G.tx, QC_DATEI_STUECK, zu_gross);
    uint8_t b1;
    struct pollfd pf = { .fd = G.ein, .events = POLLIN, .revents = 0 };
    int zu = poll(&pf, 1, 2000) == 1 && recv(G.ein, &b1, 1, 0) == 0;
    int weg = !faehig_jetzt();
    stdout_stumm(0);
    pruefe(g_jetzt && zu, "ein Stueck ueber 16 + 49152 Byte beendet den Eingabekanal");
    pruefe(weg, "mit dem Kanal ist auch seine Faehigkeit weg");

    printf("\n-- Dateien: die Faehigkeit gilt nur fuer den Eingabekanal, der sie gemeldet hat\n");
    // Derselbe Zuschauer (dieselbe Sitzung) verbindet einen neuen
    // Eingabekanal, meldet darauf aber nichts: die Meldung des alten Kanals
    // gilt nicht mehr - er koennte etwa ein aelterer Client sein.
    stdout_stumm(1);
    close(G.ein);
    G.ein = eingabe_verbinden(ein_port, g_priv, G.hh, &G.tx);
    int neuer_kanal = G.ein >= 0 && warten_bis(eingabe_steht, 2);
    int faehig_neu = faehig_jetzt();
    clip_dateien_cb(@[ [q stringByAppendingPathComponent:@"einzeln.txt"] ]);
    int bei_g2 = datei_nachrichten(&G, 400);
    qc_dateien_abwarten();
    int sendet2 = qc_senden_laeuft();
    ein_daten(G.ein, &G.tx, QC_IN_FAEHIGKEITEN, eins);
    int selbst_gemeldet = warten_bis(faehig_jetzt, 1);
    stdout_stumm(0);
    pruefe(neuer_kanal && !faehig_neu && bei_g2 == 0 && !sendet2,
           "neuer Eingabekanal derselben Sitzung ohne IN_FAEHIGKEITEN: die Meldung des alten gilt nicht, nichts geht hinaus");
    pruefe(selbst_gemeldet, "meldet der neue Kanal die Faehigkeit selbst, gilt sie wieder");

    zuschauer_weg();
    schein_schliessen(&F);
    schein_schliessen(&G);
    qc_dateien_abwarten();
    pthread_mutex_lock(&g_log_mtx);
    fclose(g_log);
    g_log = NULL;
    pthread_mutex_unlock(&g_log_mtx);
}

// --------------------------------------------------------------- Bildschirm
//
// Bildschirmwahl und Umschalten (Spezifikation 3.2). Die Liste kommt aus einer
// Attrappe statt von ScreenCaptureKit, der Strom aus einer Fabrik, die eine
// FakeStrom liefert; Encoder, Annahme, Begruessung, Eingabekanal und der Weg
// ueber g_lifeq/g_capq sind die des Hosts. Bilder kommen wie beim
// Codecwechsel ueber den Grabber.

static NSArray<QCBildschirm *> *g_test_liste = nil;
static NSArray<QCBildschirm *> *test_liste(void) { return g_test_liste ?: @[]; }

static _Atomic int g_fabrik_aufrufe = 0;
static _Atomic CGDirectDisplayID g_fabrik_zuletzt = 0;
static _Atomic long g_fabrik_cfg_w = 0, g_fabrik_cfg_h = 0;
static SCStream *test_fabrik(QCBildschirm *b, SCStreamConfiguration *cfg, id grab, dispatch_queue_t q, NSString **fehler) {
    (void)grab; (void)q; (void)fehler;
    atomic_fetch_add(&g_fabrik_aufrufe, 1);
    atomic_store(&g_fabrik_zuletzt, b.displayID);
    atomic_store(&g_fabrik_cfg_w, (long)cfg.width);
    atomic_store(&g_fabrik_cfg_h, (long)cfg.height);
    FakeStrom *fs = [FakeStrom alloc];
    [g_attrappen addObject:fs];
    return fs;
}

static QCBildschirm *bs(NSString *k, NSString *n, CGDirectDisplayID id_, size_t w, size_t h, double hz, BOOL haupt) {
    return [QCBildschirm kennung:k name:n displayID:id_ w:w h:h hz:hz haupt:haupt];
}

// Die Liste der Attrappe tauschen, auf g_lifeq - dort liest sie der Lieferant.
static void liste_setzen(NSArray<QCBildschirm *> *l) {
    dispatch_sync(g_lifeq, ^{ g_test_liste = l; });
}

static NSData *hex(const char *s) {
    NSMutableData *d = [NSMutableData data];
    for (const char *p = s; *p;) {
        while (*p == ' ') p++;
        if (!*p) break;
        unsigned v = 0;
        sscanf(p, "%2x", &v);
        uint8_t b = (uint8_t)v;
        [d appendBytes:&b length:1];
        p += 2;
    }
    return d;
}

static NSString *zeichen_mal(NSString *s, int n) {
    return [@"" stringByPaddingToLength:(NSUInteger)n * s.length withString:s startingAtIndex:0];
}

// ----------------------------------------------- HDR10: Encoder (echt, VT)
//
// Synthetische PQ-Bilder durch den Encoder des Hosts (encoder_start,
// encode_buffer, emit_access_unit mit --capture-Datei als Ausgang) und mit
// VideoToolbox zurueck, wie der Mac-Client: VUI und Anhaenge 2020/PQ/2020,
// die eigene SEI 137/144 byte-genau hinter VPS/SPS/PPS jedes Vollbilds (und
// nur dort), die Codes des HDR-Testbilds, P3 -> BT.2020 durch VideoToolbox.
// Braucht den Hardware-Encoder: ausserhalb der Seatbelt-Sandbox laufen.

// Die SEI fuer Display P3, 1000 nit, 0,005 nit, MaxCLL/MaxFALL 0 - die Zeile
// "sei p3 10000000 50 0 0" aus client/src/hdr_vektoren.txt.
static const char *QC_SEI_P3_1000 = "4e 01 89 18 33 c2 86 c4 1d 4c 0b b8 84 d0 3e 80 3d 13 40 42 00 98 96 80 00 00 03 00 32 90 04 00 00 03 00 00 80";

typedef struct { const uint8_t *p; size_t n; } nal_t;

// Annex B in NAL-Einheiten (ohne Startcodes; Nullen vor einem Startcode
// gehoeren zu ihm).
static size_t nals_zerlegen(const uint8_t *b, size_t n, nal_t *aus, size_t max) {
    size_t k = 0, anfang = SIZE_MAX;
    for (size_t i = 0; i + 3 <= n;) {
        if (b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1) {
            if (anfang != SIZE_MAX && k < max) {
                size_t e = i;
                while (e > anfang && b[e - 1] == 0) e--;
                aus[k++] = (nal_t){ b + anfang, e - anfang };
            }
            i += 3;
            anfang = i;
            continue;
        }
        i++;
    }
    if (anfang != SIZE_MAX && anfang < n && k < max) aus[k++] = (nal_t){ b + anfang, n - anfang };
    return k;
}

static int nal_typ(nal_t x) { return x.n ? (x.p[0] >> 1) & 0x3f : -1; }
static int nal_vcl(nal_t x) { return x.n && nal_typ(x) < 32; }
static int nal_ist(nal_t x, NSData *d) { return x.n == d.length && memcmp(x.p, d.bytes, x.n) == 0; }

// Eine Praefix-SEI, deren erste Nachricht Typ 137 ist (unsere).
static int nal_sei137(nal_t x) { return nal_typ(x) == 39 && x.n > 2 && x.p[2] == 137; }

typedef struct {
    int vollbilder;          // Zugriffseinheiten mit VPS
    int sei_richtig;         // davon: VPS, SPS, PPS, dann genau unsere SEI, dann erst Bilddaten
    int sei_sonst;           // unsere SEI an anderer Stelle (Zwischenbild, doppelt)
    int bilder;              // Zugriffseinheiten
} strom_befund;

static void strom_pruefen(nal_t *nals, size_t k, NSData *sei, strom_befund *b) {
    memset(b, 0, sizeof *b);
    int sei_zaehler = 0;
    for (size_t i = 0; i < k; i++) {
        if (nal_sei137(nals[i])) sei_zaehler++;
        if (nal_vcl(nals[i]) && nals[i].n > 2 && (nals[i].p[2] & 0x80)) b->bilder++;   // first_slice_segment_in_pic_flag
        if (nal_typ(nals[i]) != 32) continue;
        b->vollbilder++;
        if (i + 4 < k && nal_typ(nals[i + 1]) == 33 && nal_typ(nals[i + 2]) == 34 && sei && nal_ist(nals[i + 3], sei) &&
            !nal_sei137(nals[i + 4]))
            b->sei_richtig++;
    }
    b->sei_sonst = sei_zaehler - b->sei_richtig;
}

// Die erste Zugriffseinheit (ab dem ersten VPS bis vor das zweite Bild): VPS,
// SPS, PPS fuer die Formatbeschreibung, der Rest als Probe mit
// Laengenpraefix - wie der Client decodiert.
static CVPixelBufferRef g_dec_bild = NULL;
static void dec_rueckruf(void *r, void *s, OSStatus st, VTDecodeInfoFlags f, CVImageBufferRef pb, CMTime a, CMTime b) {
    (void)r; (void)s; (void)f; (void)a; (void)b;
    if (st != noErr || !pb) return;
    if (g_dec_bild) CVPixelBufferRelease(g_dec_bild);
    g_dec_bild = CVPixelBufferRetain(pb);
}

static CVPixelBufferRef erstes_bild_decodieren(nal_t *nals, size_t k, OSType aus, CMFormatDescriptionRef *fd_aus) {
    const uint8_t *ps[3] = {0};
    size_t pz[3] = {0};
    size_t i = 0;
    while (i < k && nal_typ(nals[i]) != 32) i++;
    if (i + 3 > k) return NULL;
    for (int j = 0; j < 3; j++) { ps[j] = nals[i + j].p; pz[j] = nals[i + j].n; }
    CMFormatDescriptionRef fd = NULL;
    if (CMVideoFormatDescriptionCreateFromHEVCParameterSets(NULL, 3, ps, pz, 4, NULL, &fd) != noErr || !fd) return NULL;
    NSMutableData *probe = [NSMutableData data];
    int bild = 0;
    for (size_t j = i + 3; j < k; j++) {
        if (nal_typ(nals[j]) == 32) break;
        if (nal_vcl(nals[j]) && nals[j].n > 2 && (nals[j].p[2] & 0x80) && bild++) break;
        uint8_t l[4] = { (uint8_t)(nals[j].n >> 24), (uint8_t)(nals[j].n >> 16), (uint8_t)(nals[j].n >> 8), (uint8_t)nals[j].n };
        [probe appendBytes:l length:4];
        [probe appendBytes:nals[j].p length:nals[j].n];
    }
    CMBlockBufferRef bb = NULL;
    CMBlockBufferCreateWithMemoryBlock(NULL, NULL, probe.length, NULL, NULL, 0, probe.length, 0, &bb);
    CMBlockBufferReplaceDataBytes(probe.bytes, bb, 0, probe.length);
    CMSampleBufferRef sb = NULL;
    size_t groesse = probe.length;
    CMSampleBufferCreateReady(NULL, bb, fd, 1, 0, NULL, 1, &groesse, &sb);
    NSDictionary *ziel = @{ (id)kCVPixelBufferPixelFormatTypeKey: @(aus) };
    VTDecompressionOutputCallbackRecord cb = { dec_rueckruf, NULL };
    VTDecompressionSessionRef d = NULL;
    if (g_dec_bild) { CVPixelBufferRelease(g_dec_bild); g_dec_bild = NULL; }
    if (VTDecompressionSessionCreate(NULL, fd, NULL, (__bridge CFDictionaryRef)ziel, &cb, &d) == noErr && d) {
        VTDecompressionSessionDecodeFrame(d, sb, 0, NULL, NULL);
        VTDecompressionSessionWaitForAsynchronousFrames(d);
        VTDecompressionSessionInvalidate(d);
        CFRelease(d);
    }
    if (sb) CFRelease(sb);
    if (bb) CFRelease(bb);
    if (fd_aus) *fd_aus = fd; else CFRelease(fd);
    CVPixelBufferRef pb = g_dec_bild;
    g_dec_bild = NULL;
    return pb;
}

// Ein Bildpunkt (10 Bit) eines decodierten xf44/xf20: ebene 0 Y, 1 Cb, 2 Cr.
static int dec_punkt(CVPixelBufferRef pb, int x, int y, int k) {
    CVPixelBufferLockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    int v;
    if (k == 0) {
        const uint8_t *b = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
        v = ((const uint16_t *)(b + (size_t)y * CVPixelBufferGetBytesPerRowOfPlane(pb, 0)))[x] >> 6;
    } else {
        int sub = CVPixelBufferGetWidthOfPlane(pb, 1) < CVPixelBufferGetWidth(pb);
        int cx = sub ? x / 2 : x, cy = sub ? y / 2 : y;
        const uint8_t *b = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
        v = ((const uint16_t *)(b + (size_t)cy * CVPixelBufferGetBytesPerRowOfPlane(pb, 1)))[2 * cx + (k - 1)] >> 6;
    }
    CVPixelBufferUnlockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    return v;
}

static int anhang_ist(CVBufferRef pb, CFStringRef key, CFStringRef soll) {
    CFTypeRef v = CVBufferCopyAttachment(pb, key, NULL);
    int ok = v && CFEqual(v, soll);
    if (v) CFRelease(v);
    return ok;
}

static int fd_ist(CMFormatDescriptionRef fd, CFStringRef key, CFTypeRef soll) {
    CFTypeRef v = CMFormatDescriptionGetExtension(fd, key);
    return v && CFEqual(v, soll);
}

// Bilder durch den Encoder des Hosts: n Stueck, bei vollbild_bei ein
// erzwungenes Vollbild; der Strom landet in einer Datei (wie --capture) und
// kommt als NSData zurueck. NULL, wenn der Encoder nicht aufgeht.
static NSData *host_codieren(int idx, int w, int h, int pq, CVPixelBufferRef (^bild)(int i), int n, int vollbild_bei) {
    char pfad[1100];
    snprintf(pfad, sizeof pfad, "%s/hdr-strom.hevc", g_home);
    __block BOOL ok = NO;
    dispatch_sync(g_capq, ^{
        atomic_store(&g_codec_id, idx);
        ok = encoder_start(idx, w, h, 60, 20, pq);
    });
    if (!ok) return nil;
    g_stats.out = fopen(pfad, "wb");
    for (int i = 0; i < n; i++) {
        CVPixelBufferRef pb = bild(i);
        if (i == vollbild_bei) atomic_store(&g_force_key, 1);
        dispatch_sync(g_capq, ^{ encode_buffer(pb, CMTimeMake(i, 60), 0, 0); });
        dispatch_sync(g_capq, ^{ VTCompressionSessionCompleteFrames(g_session, kCMTimeInvalid); });
    }
    dispatch_sync(g_capq, ^{
        VTCompressionSessionRef s = g_session;
        g_session = NULL;
        if (s) { VTCompressionSessionCompleteFrames(s, kCMTimeInvalid); VTCompressionSessionInvalidate(s); CFRelease(s); }
    });
    fclose(g_stats.out);
    g_stats.out = NULL;
    NSData *d = [NSData dataWithContentsOfFile:@(pfad)];
    unlink(pfad);
    return d;
}

// 10-Bit-Codes Y'CbCr (voll) aus R'G'B' mit den Gewichten kr, kb.
static void codes_aus_rgb(const double rgb[3], double kr, double kb, int c[3]) {
    double y = kr * rgb[0] + (1 - kr - kb) * rgb[1] + kb * rgb[2];
    double cb = (rgb[2] - y) / (2 * (1 - kb)), cr = (rgb[0] - y) / (2 * (1 - kr));
    c[0] = (int)lround(y * 1023);
    c[1] = (int)lround(cb * 1023 + 512);
    c[2] = (int)lround(cr * 1023 + 512);
}

static void hdr_encoder_pruefen(void) {
    printf("\n-- HDR10: Encoder, SEI und Rueckweg (echter Encoder und Decoder, 640x360)\n");
    stdout_stumm(1);
    codecs_pruefen();
    stdout_stumm(0);
    int system = hdr_system_kann();
    printf("         (macOS 15 und Apple Silicon: %s; HDR10 je Kandidat: %d %d %d %d %d %d)\n", system ? "ja" : "nein",
           g_befund[0].hdr, g_befund[1].hdr, g_befund[2].hdr, g_befund[3].hdr, g_befund[4].hdr, g_befund[5].hdr);
    pruefe(!g_befund[1].hdr && !g_befund[3].hdr && !g_befund[4].hdr && !g_befund[5].hdr,
           "Koennensliste: HDR10 nie fuer 8 Bit, H.264 oder AV1");
    if (!system || !g_befund[0].hdr) {
        printf("         (dieser Mac kann kein HDR10 - der Rest des Abschnitts entfaellt)\n");
        pruefe(!g_befund[0].hdr && !g_befund[2].hdr, "ohne macOS 15 und Apple Silicon auch fuer HEVC 10 Bit kein HDR10");
        return;
    }
    pruefe(g_befund[0].hdr && g_befund[2].hdr, "Koennensliste: HDR10 mit HEVC 4:4:4 10 Bit und 4:2:0 10 Bit (Probesitzung nimmt BT.2020/PQ und Mastering)");

    // a) Das HDR-Testbild: Anhaenge, Codes wie qc_testbild_punkt_pq, Grau
    //    unabhaengig nachgerechnet (SDR-Grau mal 203 nit in PQ), die Rampe.
    const int W = 640, H = 360;
    OSType xf44 = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
    qc_testbild_start(W, H, xf44, 1);
    CVPixelBufferRef t0 = qc_testbild_naechstes();
    int ok_anh = t0 && anhang_ist(t0, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_2020) &&
                 anhang_ist(t0, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) &&
                 anhang_ist(t0, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_2020);
    int py[3], pr[3], pm[3];
    qc_testbild_punkt_pq(40, 60, W, H, 0, &py[0], &py[1], &py[2]);           // Balken Weiss (Y 235)
    qc_testbild_punkt_pq(W - 1, 300, W, H, 0, &pr[0], &pr[1], &pr[2]);       // Rampe rechts: 1000 nit
    qc_testbild_punkt_pq(480, 300, W, H, 0, &pm[0], &pm[1], &pm[2]);         // Quadrat (4 x SDR)
    double s = 235 / 255.0, lin = s <= 0.04045 ? s / 12.92 : pow((s + 0.055) / 1.055, 2.4);
    int weiss = qc_hdr_pq_code10(QC_TESTBILD_PQ_WEISS_NIT * lin);
    int im_puffer = t0 ? dec_punkt(t0, 40, 60, 0) : -1;
    printf("         (Testbild HDR: Balken Weiss Y %d (soll %d, im Puffer %d), Rampe rechts Y %d, Quadrat Y/Cb/Cr %d/%d/%d)\n",
           py[0], weiss, im_puffer, pr[0], pm[0], pm[1], pm[2]);
    pruefe(ok_anh && py[0] == weiss && py[1] == 512 && py[2] == 512 && im_puffer == weiss && pr[0] == 769 && pm[0] > py[0],
           "HDR-Testbild: Anhaenge 2020/PQ/2020, SDR-Weiss bei 203 nit, Rampe bis 1000 nit (Code 769), Quadrat heller als SDR-Weiss");

    // b) HEVC 4:4:4 10 Bit in HDR10: Parametersaetze, unsere SEI hinter PPS
    //    jedes Vollbilds (zwei: das erste und ein erzwungenes), Rueckweg.
    NSData *sei_soll = hex(QC_SEI_P3_1000);
    qc_hdr_sei_werte sw;
    hdr_sei_werte(&sw);
    uint8_t sei_c[64];
    size_t sei_n = qc_hdr_sei_bauen(&sw, sei_c, sizeof sei_c);
    pruefe(sei_n == sei_soll.length && memcmp(sei_c, sei_soll.bytes, sei_n) == 0,
           "SEI des Hosts (P3, 1000 nit, 0,005 nit, 0/0) = Pruefvektor aus hdr_vektoren.txt");
    // Die Schleife von vorn: das erste codierte Bild ist Bild 0.
    qc_testbild_stop();
    qc_testbild_start(W, H, xf44, 1);
    stdout_stumm(1);
    NSData *strom = host_codieren(0, W, H, 1, ^CVPixelBufferRef(int i) { (void)i; return qc_testbild_naechstes(); }, 6, 3);
    stdout_stumm(0);
    nal_t nals[256];
    size_t k = strom ? nals_zerlegen(strom.bytes, strom.length, nals, 256) : 0;
    strom_befund b;
    strom_pruefen(nals, k, sei_soll, &b);
    printf("         (4:4:4 HDR10: %zu Byte, %zu NAL, %d Bilder, %d Vollbilder, SEI richtig %d, anderswo %d)\n",
           strom ? strom.length : 0, k, b.bilder, b.vollbilder, b.sei_richtig, b.sei_sonst);
    pruefe(strom && b.bilder == 6 && b.vollbilder == 2 && b.sei_richtig == 2 && b.sei_sonst == 0,
           "HDR10-Strom: vor jedem Vollbild VPS, SPS, PPS und dann unsere SEI 137/144 (byte-genau), sonst nirgends");
    CMFormatDescriptionRef fd = NULL;
    CVPixelBufferRef dec = strom ? erstes_bild_decodieren(nals, k, xf44, &fd) : NULL;
    int ok_vui = fd && fd_ist(fd, kCMFormatDescriptionExtension_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_2020) &&
                 fd_ist(fd, kCMFormatDescriptionExtension_TransferFunction, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) &&
                 fd_ist(fd, kCMFormatDescriptionExtension_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_2020) &&
                 fd_ist(fd, kCMFormatDescriptionExtension_FullRangeVideo, kCFBooleanTrue);
    pruefe(ok_vui, "VUI aus den Parametersaetzen (wie der Client sie liest): BT.2020, PQ, BT.2020-NCL, voller Bereich");
    int ok_dec_anh = dec && anhang_ist(dec, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_2020) &&
                     anhang_ist(dec, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) &&
                     anhang_ist(dec, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_2020);
    // Flaechen und die Rampe: codiert und zurueck auf wenige Codes genau.
    int punkte[][2] = { { 40, 60 }, { 440, 60 }, { 600, 60 }, { 100, 300 }, { 320, 300 }, { 600, 300 }, { 480, 300 } };
    int abw = 0;
    for (size_t p = 0; dec && p < sizeof punkte / sizeof punkte[0]; p++) {
        int soll[3];
        qc_testbild_punkt_pq(punkte[p][0], punkte[p][1], W, H, 0, &soll[0], &soll[1], &soll[2]);
        for (int c = 0; c < 3; c++) {
            int d = abs(dec_punkt(dec, punkte[p][0], punkte[p][1], c) - soll[c]);
            if (d > abw) abw = d;
        }
    }
    printf("         (Rueckweg 4:4:4: Anhaenge %s, groesste Abweichung %d Codes an 7 Stellen)\n", ok_dec_anh ? "2020/PQ/2020" : "FALSCH", abw);
    pruefe(dec && ok_dec_anh && abw <= 4,
           "decodiert (Hardware, wie der Mac-Client): Anhaenge 2020/PQ/2020, Balken, Rampe und Quadrat auf 4 Codes genau");
    if (dec) CVPixelBufferRelease(dec);
    if (fd) CFRelease(fd);

    // c) Die Aufnahme kommt als Display P3 PQ mit Matrix BT.709 (wie
    //    ScreenCaptureKit, HDRStream...): VideoToolbox rechnet nach BT.2020 um.
    //    Eine Flaeche mit 300/150/40 nit in P3 muss als dieselben nit in
    //    BT.2020 herauskommen.
    static const double p3_2020[3][3] = {
        { 0.75383303, 0.19859737, 0.0475696 },
        { 0.04574385, 0.9417772, 0.012478931 },
        { -0.00121034, 0.017601717, 0.9836086 },
    };
    double nit_p3[3] = { 300, 150, 40 }, rgb_p3[3], rgb_2020[3];
    for (int c = 0; c < 3; c++) rgb_p3[c] = qc_hdr_pq_aus_nit(nit_p3[c]);
    for (int c = 0; c < 3; c++)
        rgb_2020[c] = qc_hdr_pq_aus_nit(p3_2020[c][0] * nit_p3[0] + p3_2020[c][1] * nit_p3[1] + p3_2020[c][2] * nit_p3[2]);
    int ein[3], soll2020[3];
    codes_aus_rgb(rgb_p3, 0.2126, 0.0722, ein);
    codes_aus_rgb(rgb_2020, 0.2627, 0.0593, soll2020);
    CVPixelBufferRef p3 = formatpuffer(W, H, xf44, ein[0], ein[1], ein[2]);
    CVBufferSetAttachment(p3, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_P3_D65, kCVAttachmentMode_ShouldPropagate);
    CVBufferSetAttachment(p3, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVAttachmentMode_ShouldPropagate);
    CVBufferSetAttachment(p3, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVAttachmentMode_ShouldPropagate);
    stdout_stumm(1);
    strom = host_codieren(0, W, H, 1, ^CVPixelBufferRef(int i) { (void)i; return p3; }, 2, -1);
    stdout_stumm(0);
    k = strom ? nals_zerlegen(strom.bytes, strom.length, nals, 256) : 0;
    dec = strom ? erstes_bild_decodieren(nals, k, xf44, NULL) : NULL;
    int got[3] = { -1, -1, -1 };
    for (int c = 0; dec && c < 3; c++) got[c] = dec_punkt(dec, W / 2, H / 2, c);
    printf("         (P3 PQ 300/150/40 nit, Matrix 709: ein %d/%d/%d -> BT.2020 %d/%d/%d, soll %d/%d/%d)\n",
           ein[0], ein[1], ein[2], got[0], got[1], got[2], soll2020[0], soll2020[1], soll2020[2]);
    pruefe(dec && abs(got[0] - soll2020[0]) <= 4 && abs(got[1] - soll2020[1]) <= 4 && abs(got[2] - soll2020[2]) <= 4,
           "Aufnahme in Display P3 PQ (Matrix 709): VideoToolbox rechnet nach BT.2020 um, auf 4 Codes genau");
    if (dec) CVPixelBufferRelease(dec);
    CVPixelBufferRelease(p3);

    // d) HEVC 4:2:0 10 Bit in HDR10 (Eingang xf44, VideoToolbox unterabtastet).
    qc_testbild_stop();
    qc_testbild_start(W, H, xf44, 1);
    stdout_stumm(1);
    strom = host_codieren(2, W, H, 1, ^CVPixelBufferRef(int i) { (void)i; return qc_testbild_naechstes(); }, 3, -1);
    stdout_stumm(0);
    k = strom ? nals_zerlegen(strom.bytes, strom.length, nals, 256) : 0;
    strom_pruefen(nals, k, sei_soll, &b);
    fd = NULL;
    dec = strom ? erstes_bild_decodieren(nals, k, kCVPixelFormatType_420YpCbCr10BiPlanarFullRange, &fd) : NULL;
    int soll_w[3];
    qc_testbild_punkt_pq(40, 60, W, H, 0, &soll_w[0], &soll_w[1], &soll_w[2]);
    int y420 = dec ? dec_punkt(dec, 40, 60, 0) : -1;
    printf("         (4:2:0 HDR10: %d Vollbild(er), SEI richtig %d, Weiss Y %d soll %d)\n", b.vollbilder, b.sei_richtig, y420, soll_w[0]);
    pruefe(b.vollbilder == 1 && b.sei_richtig == 1 && b.sei_sonst == 0 && fd &&
           fd_ist(fd, kCMFormatDescriptionExtension_TransferFunction, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ) &&
           dec && abs(y420 - soll_w[0]) <= 4,
           "HEVC 4:2:0 10 Bit in HDR10: SEI hinter PPS, VUI PQ, Weiss zurueck auf 4 Codes genau");
    if (dec) CVPixelBufferRelease(dec);
    if (fd) CFRelease(fd);

    // e) SDR bleibt SDR: kein SEI 137, VUI BT.709.
    qc_testbild_start(W, H, xf44, 0);
    stdout_stumm(1);
    strom = host_codieren(0, W, H, 0, ^CVPixelBufferRef(int i) { (void)i; return qc_testbild_naechstes(); }, 3, -1);
    stdout_stumm(0);
    k = strom ? nals_zerlegen(strom.bytes, strom.length, nals, 256) : 0;
    strom_pruefen(nals, k, sei_soll, &b);
    fd = NULL;
    dec = strom ? erstes_bild_decodieren(nals, k, xf44, &fd) : NULL;
    pruefe(b.vollbilder == 1 && b.sei_richtig == 0 && b.sei_sonst == 0 && fd &&
           fd_ist(fd, kCMFormatDescriptionExtension_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2) &&
           fd_ist(fd, kCMFormatDescriptionExtension_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2),
           "SDR wie bisher: keine SEI 137/144, VUI BT.709");
    if (dec) CVPixelBufferRelease(dec);
    if (fd) CFRelease(fd);
    qc_testbild_stop();

    // f) HDR10 nur mit HEVC 10 Bit: 8 Bit und H.264 lehnen ab, ohne Sitzung.
    __block BOOL acht = YES, h264 = YES;
    stdout_stumm(1);
    dispatch_sync(g_capq, ^{
        acht = encoder_start(3, W, H, 60, 10, 1);
        h264 = encoder_start(4, W, H, 60, 10, 1);
    });
    stdout_stumm(0);
    pruefe(!acht && !h264 && !g_session && !atomic_load(&g_farbe_pq), "HDR10 mit HEVC 8 Bit oder H.264: abgelehnt, keine Sitzung, Farbe SDR");
    atomic_store(&g_codec_id, 0);
}

// Ein Eintrag der Liste (12), wie der Client ihn liest.
typedef struct { char kennung[65]; char name[49]; uint16_t w, h, hz; uint8_t flags; } eintrag;

// Liest die Liste: Wunsch nach *wunsch, Eintrag idx nach *e. Rueckgabe: die
// Zahl der Eintraege, -1 = kaputt.
static int liste_lesen(const uint8_t *p, size_t n, char *wunsch, int idx, eintrag *e) {
    if (n < 4 || p[0] != QC_BILDSCHIRM_FASSUNG) return -1;
    size_t anzahl = p[1], wl = p[2], o = 4;
    if (o + wl > n) return -1;
    if (wunsch) { memcpy(wunsch, p + o, wl); wunsch[wl] = 0; }
    o += wl;
    for (size_t i = 0; i < anzahl; i++) {
        if (o + 10 > n) return -1;
        size_t kl = p[o], nl = p[o + 1];
        if (o + 10 + kl + nl > n || kl > 64 || nl > 48) return -1;
        if ((int)i == idx && e) {
            memcpy(&e->w, p + o + 2, 2); memcpy(&e->h, p + o + 4, 2); memcpy(&e->hz, p + o + 6, 2);
            e->flags = p[o + 8];
            memcpy(e->kennung, p + o + 10, kl); e->kennung[kl] = 0;
            memcpy(e->name, p + o + 10 + kl, nl); e->name[nl] = 0;
        }
        o += 10 + kl + nl;
    }
    return o == n ? (int)anzahl : -1;
}

// Alles, was in frist_ms beim Zuschauer ankommt: Typfolge (ohne Stempel),
// die letzte Liste, die letzte Strominfo, Wechselansagen, Bilder, Hoststatus.
typedef struct {
    char folge[256]; uint8_t liste[QC_BILDSCHIRM_LISTE_MAX]; size_t liste_n;
    uint8_t info[QC_HDR_INFO_LAENGE]; size_t info_n; int infos;
    int wechsel; uint8_t wechsel_codec, wechsel_transfer, wechsel_umrechnung; int bilder, voll, erstes_voll; int status, statusse;
} gelesen;

// Das letzte Vollbild, das alles_lesen sah (Annex B mit Startcodes).
static NSData *g_letztes_vollbild = nil;

static void alles_lesen(schein *s, int frist_ms, gelesen *g) {
    memset(g, 0, sizeof *g);
    g->erstes_voll = -1;
    g->status = -1;
    qc_hdr h;
    NSData *d = nil;
    size_t n = 0;
    while (nachricht_ganz(&s->l, &h, &d, frist_ms) == 1) {
        if (h.type != QC_MSG_STAMP && n + 6 < sizeof g->folge)
            n += (size_t)snprintf(g->folge + n, sizeof g->folge - n, "%s%d", n ? " " : "", h.type);
        switch (h.type) {
            case QC_MSG_BILDSCHIRME:
                g->liste_n = d.length < sizeof g->liste ? d.length : sizeof g->liste;
                memcpy(g->liste, d.bytes, g->liste_n);
                break;
            case QC_MSG_INFO:
                g->info_n = d.length;
                if (d.length >= 8) memcpy(g->info, d.bytes, d.length < sizeof g->info ? d.length : sizeof g->info);
                g->infos++;
                break;
            case QC_MSG_SWITCH:
                g->wechsel++;
                if (d.length) g->wechsel_codec = ((const uint8_t *)d.bytes)[0];
                if (d.length >= 8) g->wechsel_transfer = ((const uint8_t *)d.bytes)[6];
                if (d.length >= 8) g->wechsel_umrechnung = ((const uint8_t *)d.bytes)[5];
                break;
            case QC_MSG_VIDEO: {
                int key = (h.flags & QC_FLAG_KEY) ? 1 : 0;
                if (key) g_letztes_vollbild = d;
                if (g->bilder == 0) g->erstes_voll = key;
                g->bilder++;
                g->voll += key;
                break;
            }
            case QC_MSG_HOSTSTATUS: if (d.length) g->status = ((const uint8_t *)d.bytes)[0]; g->statusse++; break;
            default: break;
        }
    }
}

static uint16_t info_w(const gelesen *g) { uint16_t v; memcpy(&v, g->info, 2); return v; }
static uint16_t info_h(const gelesen *g) { uint16_t v; memcpy(&v, g->info + 2, 2); return v; }

// Die letzte INFO ist Fassung 1 (24 Byte) in SDR mit Grund 7 - dieser Host
// liest noch kein IN_ANZEIGE (hdr.h).
static int info_fassung1_sdr(const gelesen *g) {
    qc_hdr_info i;
    uint8_t soll[QC_HDR_INFO_LAENGE - QC_HDR_INFO_ALT];
    qc_hdr_info_sdr(&i, QC_HDR_GRUND_KEIN_IN_ANZEIGE);
    qc_hdr_info_kodieren(&i, soll);
    return g->info_n == QC_HDR_INFO_LAENGE && memcmp(g->info + QC_HDR_INFO_ALT, soll, sizeof soll) == 0;
}

// IN_ANZEIGE (14 Byte, hdr.h) mit diesen Flags, diesem Wunsch und Kopfraum.
static NSData *anzeige_daten(uint8_t flags, uint8_t wunsch, uint16_t kopfraum) {
    uint8_t p[QC_HDR_ANZEIGE_LAENGE] = { QC_HDR_ANZEIGE_FASSUNG, flags, wunsch, 0 };
    uint16_t weiss = 240, spitze = 1000;
    memcpy(p + 4, &weiss, 2); memcpy(p + 6, &spitze, 2);
    memcpy(p + 10, &kopfraum, 2); memcpy(p + 12, &kopfraum, 2);
    return [NSData dataWithBytes:p length:sizeof p];
}

// Wie oft der Typ in der Folge steht (die Folge sind Zahlen mit Leerzeichen).
static int typen_zaehlen(const char *folge, int typ) {
    int n = 0;
    for (const char *p = folge; *p;) {
        char *ende = NULL;
        long t = strtol(p, &ende, 10);
        if (ende == p) break;
        if (t == typ) n++;
        p = ende;
        while (*p == ' ') p++;
    }
    return n;
}

static uint64_t wartet_seit_jetzt(void) {
    __block uint64_t seit = 0;
    dispatch_sync(g_lifeq, ^{ seit = g_bildschirm_wartet_seit; });
    return seit;
}

// Ein Bild der Aufnahme in den Grabber des Hosts, wie ScreenCaptureKit.
static void bild_einspeisen(CVPixelBufferRef pb) {
    CMSampleBufferRef sb = aufnahme_bild(pb, uhr());
    SCStream *fs = strom_jetzt();
    dispatch_sync(g_capq, ^{ [g_grab stream:fs didOutputSampleBuffer:sb ofType:SCStreamOutputTypeScreen]; });
    CFRelease(sb);
}

static CGDirectDisplayID display_jetzt(void) {
    __block CGDirectDisplayID d = 0;
    dispatch_sync(g_lifeq, ^{ d = g_display_id; });
    return d;
}

static NSString *wunsch_jetzt(void) {
    __block NSString *w = nil;
    dispatch_sync(g_lifeq, ^{ w = g_display_wunsch; });
    return w;
}

static VTCompressionSessionRef session_jetzt(void) {
    __block VTCompressionSessionRef s = NULL;
    dispatch_sync(g_capq, ^{ s = g_session; });
    return s;
}

static int letztes_bild_da(void) {
    __block int da = 0;
    dispatch_sync(g_capq, ^{ da = g_last_pb != NULL; });
    return da;
}

// Der Stand fuer die Ausgabe: Fabrikaufrufe, zuletzt gebauter Bildschirm,
// gestreamter Bildschirm, Maus, Stromgroesse.
static void stand_zeile(const char *was) {
    printf("         (%s: Fabrik %d Aufruf(e), zuletzt Kennung %u, gestreamt Kennung %u, Maus auf %u, Strom %dx%d, g_cfg %ldx%ld)\n",
           was, atomic_load(&g_fabrik_aufrufe), atomic_load(&g_fabrik_zuletzt), display_jetzt(), atomic_load(&g_input_display),
           g_info_w, g_info_h, atomic_load(&g_fabrik_cfg_w), atomic_load(&g_fabrik_cfg_h));
}

static NSString *datei_inhalt(const char *pfad) {
    NSData *d = [NSData dataWithContentsOfFile:@(pfad)];
    return d ? [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] ?: @"(kein UTF-8)" : nil;
}

static void datei_schreiben(const char *pfad, const void *p, size_t n) {
    FILE *f = fopen(pfad, "wb");
    if (!f) return;
    fwrite(p, 1, n, f);
    fclose(f);
}

static void bildschirm_pruefen(int bild_port, int ein_port) {
    printf("\n-- Bildschirm: Pruefvektoren (2.4), Kuerzen, Wahl, Stromgroesse\n");
    QCBildschirm *A = bs(@"v1138-m1234-s0", @"X27 X1", 2, 1920, 1080, 120, YES);
    QCBildschirm *B = bs(@"v0-m0-s0", @"Virtuell 16:9", 6, 1920, 1080, 240, NO);
    NSArray *zwei = @[ A, B ];
    NSData *soll = hex("01 02 00 00 "
                       "0e 06 80 07 38 04 78 00 03 00 76 31 31 33 38 2d 6d 31 32 33 34 2d 73 30 58 32 37 20 58 31 "
                       "08 0d 80 07 38 04 f0 00 00 00 76 30 2d 6d 30 2d 73 30 56 69 72 74 75 65 6c 6c 20 31 36 3a 39");
    pruefe([qc_bildschirme_kodieren(zwei, nil, 2) isEqualToData:soll],
           "Liste mit Automatik, X27 X1 (Haupt, gestreamt) und Virtuell 16:9: die Bytes der Spezifikation");
    pruefe([qc_bildschirm_wunsch_kodieren(@"v0-m0-s0") isEqualToData:hex("08 76 30 2d 6d 30 2d 73 30")] &&
           [qc_bildschirm_wunsch_kodieren(nil) isEqualToData:hex("00")],
           "Wunsch v0-m0-s0 und Automatik: die Bytes der Spezifikation");
    NSString *k = @"x";
    uint8_t w1[] = { 8, 'v', '0', '-', 'm', '0', '-', 's', '0' };
    uint8_t w0[] = { 0 };
    int r1 = qc_bildschirm_wunsch_lesen(w1, sizeof w1, &k);
    int ok1 = r1 == 0 && [k isEqualToString:@"v0-m0-s0"];
    k = @"x";
    int ok0 = qc_bildschirm_wunsch_lesen(w0, 1, &k) == 0 && k == nil;
    // Ein Byte mehr als angesagt wird uebergangen (wie im Rust-Kern).
    uint8_t w_mehr[] = { 2, 'a', 'b', 'c' };
    int ok_mehr = qc_bildschirm_wunsch_lesen(w_mehr, sizeof w_mehr, &k) == 0 && [k isEqualToString:@"ab"];
    pruefe(ok1 && ok0 && ok_mehr, "Wunsch lesen: v0-m0-s0, 00 = Automatik, Nachlauf hinter der Laenge wird uebergangen");
    uint8_t w_kurz[] = { 5, 'a' }, w_steuer[] = { 2, 'a', '\n' }, w_utf[] = { 2, 0xff, 0xfe }, w_c1[] = { 3, 'a', 0xc2, 0x85 };
    uint8_t w_lang[66];
    memset(w_lang, 'k', sizeof w_lang);
    w_lang[0] = 65;
    pruefe(qc_bildschirm_wunsch_lesen(w_kurz, sizeof w_kurz, &k) < 0 && qc_bildschirm_wunsch_lesen(w_steuer, sizeof w_steuer, &k) < 0 &&
           qc_bildschirm_wunsch_lesen(w_utf, sizeof w_utf, &k) < 0 && qc_bildschirm_wunsch_lesen(w_c1, sizeof w_c1, &k) < 0 &&
           qc_bildschirm_wunsch_lesen(w_lang, sizeof w_lang, &k) < 0 && qc_bildschirm_wunsch_lesen(NULL, 0, &k) < 0,
           "Wunsch lesen: Laenge ueber dem Rest, Zeilenumbruch, kein UTF-8, U+0085, 65 Byte, leer -> ungueltig");

    // Kuerzen an Zeichengrenzen, Steuerzeichen weg, leere Kennung "?", 16 Eintraege.
    NSMutableArray *viele = [NSMutableArray array];
    [viele addObject:bs(zeichen_mal(@"ä", 40), zeichen_mal(@"ü", 30), 2, 1920, 1080, 119.88, YES)];   // 80 bzw. 60 Byte
    [viele addObject:bs(@"a\tb", @"Büro – Monitor ä", 6, 70000, 1080, 0, NO)];
    for (int i = 0; i < 20; i++) [viele addObject:bs(@"\x01", @"x", (CGDirectDisplayID)(100 + i), 800, 600, 60, NO)];
    NSData *lang = qc_bildschirme_kodieren(viele, zeichen_mal(@"x", 100), 6);
    char wunsch[65] = {0};
    eintrag e0, e1, e2;
    int anzahl = liste_lesen(lang.bytes, lang.length, wunsch, 0, &e0);
    liste_lesen(lang.bytes, lang.length, NULL, 1, &e1);
    liste_lesen(lang.bytes, lang.length, NULL, 2, &e2);
    printf("         (Liste mit 22 Eintraegen: %d kodiert, %zu Byte, Wunsch %zu Byte, Kennung 0 %zu Byte, Name 0 %zu Byte)\n",
           anzahl, lang.length, strlen(wunsch), strlen(e0.kennung), strlen(e0.name));
    pruefe(anzahl == 16 && strlen(wunsch) == 64 && lang.length <= QC_BILDSCHIRM_LISTE_MAX,
           "hoechstens 16 Eintraege, der Wunsch auf 64 Byte gekuerzt, nie ueber der Hoechstlaenge");
    pruefe([@(e0.kennung) isEqualToString:zeichen_mal(@"ä", 32)] && [@(e0.name) isEqualToString:zeichen_mal(@"ü", 24)] &&
           e0.hz == 120 && e0.flags == QC_BILDSCHIRM_FLAG_HAUPT,
           "Kennung auf 64 und Name auf 48 Byte an Zeichengrenzen gekuerzt (32 bzw. 24 Umlaute), 119,88 Hz -> 120");
    pruefe(strcmp(e1.kennung, "ab") == 0 && [@(e1.name) isEqualToString:@"Büro – Monitor ä"] && e1.w == 65535 && e1.hz == 0 &&
           e1.flags == QC_BILDSCHIRM_FLAG_GESTREAMT && strcmp(e2.kennung, "?") == 0,
           "Steuerzeichen fallen weg, Umlaute bleiben, Breite gedeckelt, gestreamt an der displayID, leere Kennung wird \"?\"");
    pruefe(liste_lesen(soll.bytes, soll.length - 1, NULL, -1, NULL) < 0 && liste_lesen(soll.bytes, 3, NULL, -1, NULL) < 0,
           "Gegenprobe des Lesers: eine abgeschnittene Liste ist kaputt");

    int grund = -1;
    NSArray *ohne_haupt = @[ bs(@"a", @"A", 1, 800, 600, 60, NO), bs(@"b", @"B", 2, 800, 600, 60, NO) ];
    pruefe(qc_bildschirm_wahl(zwei, nil, &grund) == A && grund == QC_WAHL_HAUPT, "Wahl: Automatik nimmt den Hauptbildschirm");
    pruefe(qc_bildschirm_wahl(zwei, @"v0-m0-s0", &grund) == B && grund == QC_WAHL_WUNSCH, "Wahl: der Wunsch, wenn er da ist");
    pruefe(qc_bildschirm_wahl(zwei, @"v9-m9-s9", &grund) == A && grund == QC_WAHL_HAUPT, "Wahl: Wunsch nicht da -> Hauptbildschirm als Ausweichplatz");
    pruefe(qc_bildschirm_wahl(ohne_haupt, nil, &grund) == ohne_haupt[0] && grund == QC_WAHL_ERSTER, "Wahl: ohne Hauptbildschirm der erste");
    pruefe(qc_bildschirm_wahl(@[], @"a", &grund) == nil && grund == QC_WAHL_KEINER, "Wahl: leere Liste -> keiner");
    pruefe([qc_bildschirm_kennung(1138, 1234, 0) isEqualToString:@"v1138-m1234-s0"], "Kennung aus Vendor, Model, Serial");

    int sw = 0, sh = 0;
    g_out_fest_w = g_out_fest_h = 0;
    stromgroesse_fuer(bs(@"k", @"4K", 1, 3840, 2160, 60, NO), &sw, &sh);
    int ok_4k = sw == 1920 && sh == 1080;
    stromgroesse_fuer(bs(@"k", @"breit", 1, 3840, 1080, 60, NO), &sw, &sh);
    int ok_breit = sw == 1920 && sh == 1080;
    stromgroesse_fuer(bs(@"k", @"ungerade", 1, 1921, 1081, 60, NO), &sw, &sh);
    int ok_gerade = sw == 1920 && sh == 1080;
    g_out_fest_w = 640; g_out_fest_h = 360;
    stromgroesse_fuer(bs(@"k", @"4K", 1, 3840, 2160, 60, NO), &sw, &sh);
    int ok_out = sw == 640 && sh == 360;
    g_out_fest_w = g_out_fest_h = 0;
    pruefe(ok_4k && ok_breit && ok_gerade && ok_out, "Stromgroesse: ab 3840 bzw. 2160 halbiert, gerade, --out hat Vorrang");

    printf("\n-- Bildschirm: bildschirm.txt\n");
    char pfad[1200];
    qc_config_path("bildschirm.txt", pfad, sizeof pfad);
    unlink(pfad);
    NSString *w = @"x";
    pruefe(qc_bildschirm_wunsch_laden(&w) == 1 && w == nil, "ohne Datei: Automatik (1)");
    w = @"x";
    pruefe(qc_bildschirm_wunsch_speichern(@"v0-m0-s0") == 0 && [datei_inhalt(pfad) isEqualToString:@"v0-m0-s0\n"] &&
           qc_bildschirm_wunsch_laden(&w) == 0 && [w isEqualToString:@"v0-m0-s0"],
           "Wunsch gespeichert (eine Zeile) und wieder gelesen");
    w = @"x";
    pruefe(qc_bildschirm_wunsch_speichern(nil) == 0 && [datei_inhalt(pfad) isEqualToString:@"auto\n"] &&
           qc_bildschirm_wunsch_laden(&w) == 0 && w == nil, "Automatik als \"auto\"");
    datei_schreiben(pfad, "v1-m2-s3\r\n", 10);
    w = nil;
    pruefe(qc_bildschirm_wunsch_laden(&w) == 0 && [w isEqualToString:@"v1-m2-s3"], "Zeilenende mit \\r\\n wird toleriert");
    struct { const char *inhalt; size_t n; const char *was; } kaputt[] = {
        { "", 0, "leer" },
        { "ab\x01" "c\n", 5, "Steuerzeichen" },
        { "a\nb\n", 4, "zwei Zeilen" },
        { "\xff\xfe\n", 3, "kein UTF-8" },
    };
    int alle_kaputt = 1;
    for (size_t i = 0; i < sizeof kaputt / sizeof kaputt[0]; i++) {
        datei_schreiben(pfad, kaputt[i].inhalt, kaputt[i].n);
        w = @"x";
        int r = qc_bildschirm_wunsch_laden(&w);
        NSData *danach = [NSData dataWithContentsOfFile:@(pfad)];
        int unveraendert = danach.length == kaputt[i].n && memcmp(danach.bytes, kaputt[i].inhalt, kaputt[i].n) == 0;
        if (r != -1 || w != nil || !unveraendert) { alle_kaputt = 0; printf("         (%s: Rueckgabe %d)\n", kaputt[i].was, r); }
    }
    char lang100[102];
    memset(lang100, 'k', 100);
    lang100[100] = '\n';
    datei_schreiben(pfad, lang100, 101);
    w = @"x";
    int r_lang = qc_bildschirm_wunsch_laden(&w);
    pruefe(alle_kaputt && r_lang == -1 && w == nil, "kaputte Datei (leer, Steuerzeichen, zwei Zeilen, kein UTF-8, 100 Byte): -1, Automatik, Datei bleibt");
    unlink(pfad);

    printf("\n-- Bildschirm: Aufnahmestart und Begruessung (Attrappen fuer Liste und Strom, echter Encoder 1920x1080)\n");
    static char logpfad[1100];
    snprintf(logpfad, sizeof logpfad, "%s/bildschirm.log", g_home);
    g_log_pfad = logpfad;
    pthread_mutex_lock(&g_log_mtx);
    log_oeffnen("w");
    pthread_mutex_unlock(&g_log_mtx);
    qc_bildschirm_liste_setzen(test_liste);
    qc_strom_fabrik_setzen(test_fabrik);
    // Die Strom-Attrappe eines frueheren Abschnitts liegt womoeglich noch in
    // g_stream - dann saehe die Bewertung einen laufenden Strom und wechselte.
    stdout_stumm(1);
    stream_herunterfahren_anstossen();
    SCStream *rest = strom_jetzt();
    stdout_stumm(0);
    pruefe(rest == nil, "vorher: kein Strom (die Attrappe des vorigen Abschnitts ist abgebaut)");
    liste_setzen(@[ A, B ]);
    atomic_store(&g_codec_id, 3);
    g_info_w = 0; g_info_h = 0; g_info_fps = 60;
    atomic_store(&g_cur_fps, 60);
    atomic_store(&g_cur_mbit, 10);
    atomic_store(&g_cur_fixed, 0);
    atomic_store(&g_fixed_gewollt, 0);
    atomic_store(&g_cur_gaming, 0);
    atomic_store(&g_testbild, 0);
    g_cfg = [[SCStreamConfiguration alloc] init];
    g_grab = [[Grabber alloc] init];
    dispatch_sync(g_lifeq, ^{ g_display_wunsch = nil; g_display_id = 0; g_display_aktuell = nil; g_bildschirme = nil; g_display_pin = -1; });
    atomic_store(&g_fabrik_aufrufe, 0);

    // --display N: nach der ersten Wahl wird der Listenplatz zum Wunsch fuer
    // diesen Lauf; bildschirm.txt bleibt unberuehrt. Ausser Bereich: Zeile, Automatik.
    stdout_stumm(1);
    dispatch_sync(g_lifeq, ^{ g_display_pin = 1; bildschirm_neu_bewerten(QC_ANLASS_START); });
    NSString *pin_wunsch = wunsch_jetzt();
    CGDirectDisplayID pin_ziel = display_jetzt();
    dispatch_sync(g_lifeq, ^{ g_display_wunsch = nil; g_display_pin = 7; bildschirm_neu_bewerten(QC_ANLASS_START); });
    NSString *pin7_wunsch = wunsch_jetzt();
    CGDirectDisplayID pin7_ziel = display_jetzt();
    stdout_stumm(0);
    pruefe([pin_wunsch isEqualToString:@"v0-m0-s0"] && pin_ziel == 6 && access(pfad, F_OK) != 0 &&
           zeilen_mit(logpfad, "Bildschirm --display 1: Kennung 6 (v0-m0-s0) \"Virtuell 16:9\" gilt als Wunsch fuer diesen Lauf") == 1,
           "--display 1 pinnt die Kennung des Listenplatzes 1 als Wunsch, ohne bildschirm.txt zu schreiben");
    pruefe(pin7_wunsch == nil && pin7_ziel == 2 && zeilen_mit(logpfad, "Bildschirm 7 nicht verfuegbar (2 in der Liste) - Automatik") == 1,
           "--display 7 ausser Bereich: Zeile, Automatik, Hauptbildschirm");

    uint8_t h_priv[32], h_pub[32];
    qc_keypair(h_priv, h_pub);
    qc_zugang_eintragen(h_pub, "hosttest H");
    schein H;
    g_schein_attrappe = 0;
    stdout_stumm(1);
    int ok = schein_verbinden(&H, bild_port, ein_port, h_priv) == 0;
    stdout_stumm(0);
    printf("         (nach der Begruessung: %s)\n", H.folge);
    pruefe(ok && strstr(H.folge, "3 8 11 12"), "Begruessung: Faehigkeiten 7 (Dateien, Bildschirmwahl, HDR), danach die Bildschirmliste (12)");
    pruefe(H.liste_n == soll.length && memcmp(H.liste, soll.bytes, soll.length) == 0,
           "die Liste in der Begruessung ist der Pruefvektor: X27 X1 Haupt und gestreamt, Automatik");
    VTCompressionSessionRef s_a = session_jetzt();
    stand_zeile("nach dem Aufnahmestart");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 1 && atomic_load(&g_fabrik_zuletzt) == 2 && display_jetzt() == 2 &&
           atomic_load(&g_input_display) == 2 && g_info_w == 1920 && g_info_h == 1080 &&
           atomic_load(&g_fabrik_cfg_w) == 1920 && atomic_load(&g_fabrik_cfg_h) == 1080 && s_a != NULL,
           "Aufnahmestart: Strom fuer den Hauptbildschirm, Encoder und g_cfg 1920x1080, die Maus folgt");
    pruefe(zeilen_mit(logpfad, "Aufnahme gestartet: Bildschirm 1920x1080, 120 Hz, Kennung 2 (v1138-m1234-s0) \"X27 X1\" -> 1920x1080") == 1,
           "Protokollzeile beim Aufnahmestart");
    CVPixelBufferRef pb_gross = testpuffer(1920, 1080), pb_klein = testpuffer(1280, 720);
    gelesen g;
    stdout_stumm(1);
    bild_einspeisen(pb_gross);
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(g.bilder == 1 && g.erstes_voll == 1 && letztes_bild_da(), "der Zuschauer bekommt sein erstes Vollbild");

    printf("\n-- Bildschirm: Automatik folgt dem Hauptbildschirm (entprellt)\n");
    QCBildschirm *A2 = bs(@"v1138-m1234-s0", @"X27 X1", 2, 1920, 1080, 120, NO);
    QCBildschirm *B2 = bs(@"v0-m0-s0", @"Virtuell 16:9", 6, 1920, 1080, 240, YES);
    liste_setzen(@[ A2, B2 ]);
    // CoreGraphics meldet eine Aenderung mehrmals kurz hintereinander: ein Wechsel.
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();
    usleep(50 * 1000);
    bildschirm_konfiguration_geaendert();
    usleep(50 * 1000);
    bildschirm_konfiguration_geaendert();
    usleep(150 * 1000);
    int noch_nicht = atomic_load(&g_fabrik_aufrufe) == 1;
    usleep(400 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    eintrag ea, eb;
    liste_lesen(g.liste, g.liste_n, wunsch, 0, &ea);
    liste_lesen(g.liste, g.liste_n, wunsch, 1, &eb);
    printf("         (nach dem Wechsel: %s; Liste: Wunsch \"%s\", %s Flags %d, %s Flags %d)\n", g.folge, wunsch, ea.kennung, ea.flags, eb.kennung, eb.flags);
    stand_zeile(noch_nicht ? "150 ms nach der dritten Meldung noch kein Wechsel; danach" : "150 ms nach der dritten Meldung SCHON gewechselt; danach");
    pruefe(noch_nicht && atomic_load(&g_fabrik_aufrufe) == 2 && atomic_load(&g_fabrik_zuletzt) == 6 && display_jetzt() == 6 &&
           atomic_load(&g_input_display) == 6,
           "drei Meldungen binnen 100 ms ergeben nach 300 ms Ruhe genau einen Wechsel auf den neuen Hauptbildschirm, die Maus folgt");
    pruefe(strstr(g.folge, "7 1 12") && g.wechsel == 1 && g.wechsel_codec == 3 && info_w(&g) == 1920 && info_h(&g) == 1080 &&
           !wunsch[0] && eb.flags == (QC_BILDSCHIRM_FLAG_HAUPT | QC_BILDSCHIRM_FLAG_GESTREAMT) && ea.flags == 0 && session_jetzt() == s_a,
           "SWITCH mit dem laufenden Codec, INFO (gleiche Masse, Encoder bleibt), dann die Liste mit dem neuen als Haupt und gestreamt");
    pruefe(info_fassung1_sdr(&g) && g.wechsel_transfer == QC_HDR_TRANSFER_SDR,
           "INFO als Fassung 1 (24 Byte, SDR, Grund 7), SWITCH mit Transfer 1 (SDR)");
    pruefe(!letztes_bild_da() && g.bilder == 0,
           "auch bei gleicher Groesse ist das letzte Bild des alten Bildschirms weg - es kaeme sonst als erstes Vollbild des neuen");
    stdout_stumm(1);
    bild_einspeisen(pb_gross);
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(g.bilder == 1 && g.erstes_voll == 1, "das erste Bild vom neuen Bildschirm ist ein Vollbild");
    pruefe(zeilen_mit(logpfad, "Bildschirmwechsel: Kennung 2 (v1138-m1234-s0) \"X27 X1\" -> Kennung 6 (v0-m0-s0) \"Virtuell 16:9\" (Hauptbildschirm gewechselt)") == 1 &&
           zeilen_mit(logpfad, "Bildschirm gewaehlt: Kennung 6 (v0-m0-s0) \"Virtuell 16:9\", 1920x1080 Pixel, 240 Hz (Hauptbildschirm)") == 1,
           "Protokollzeilen: Bildschirm gewaehlt, Bildschirmwechsel mit Grund");

    printf("\n-- Bildschirm: Wunsch ueber den Eingabekanal, fehlender Wunsch, Ausweichplatz, Rueckkehr\n");
    // Wunsch auf einen Bildschirm, der nicht angeschlossen ist: kein Wechsel
    // (der Hauptbildschirm laeuft schon), die Liste traegt den Wunsch, die
    // Datei auch.
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v9-m9-s9"));
    usleep(300 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n9 = liste_lesen(g.liste, g.liste_n, wunsch, 1, &eb);
    printf("         (nach dem Wunsch v9-m9-s9: %s; Liste: Wunsch \"%s\", %d Eintraege, %s Flags %d)\n", g.folge, wunsch, n9, eb.kennung, eb.flags);
    stand_zeile("danach");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 2 && g.wechsel == 0 && n9 == 2 && strcmp(wunsch, "v9-m9-s9") == 0 &&
           eb.flags == (QC_BILDSCHIRM_FLAG_HAUPT | QC_BILDSCHIRM_FLAG_GESTREAMT) && [wunsch_jetzt() isEqualToString:@"v9-m9-s9"] &&
           [datei_inhalt(pfad) isEqualToString:@"v9-m9-s9\n"],
           "Wunsch v9-m9-s9 (nicht angeschlossen): kein Wechsel, Antwort ist die Liste mit dem Wunsch und unveraendertem gestreamt, bildschirm.txt geschrieben");
    pruefe(zeilen_mit(logpfad, "Bildschirmwunsch: v9-m9-s9 - nicht angeschlossen, Ausweichplatz Kennung 6 (v0-m0-s0) \"Virtuell 16:9\"") == 1,
           "Protokollzeile zum fehlenden Wunsch");
    // Wunsch auf A (nicht Haupt): Wechsel.
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v1138-m1234-s0"));
    usleep(300 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    liste_lesen(g.liste, g.liste_n, wunsch, 0, &ea);
    liste_lesen(g.liste, g.liste_n, NULL, 1, &eb);
    printf("         (nach dem Wunsch: %s; Liste: Wunsch \"%s\", %s Flags %d, %s Flags %d)\n", g.folge, wunsch, ea.kennung, ea.flags, eb.kennung, eb.flags);
    stand_zeile("danach");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 3 && atomic_load(&g_fabrik_zuletzt) == 2 && display_jetzt() == 2 && atomic_load(&g_input_display) == 2 &&
           strstr(g.folge, "7 1 12") && g.wechsel == 1 && strcmp(wunsch, "v1138-m1234-s0") == 0 &&
           ea.flags == QC_BILDSCHIRM_FLAG_GESTREAMT && eb.flags == QC_BILDSCHIRM_FLAG_HAUPT &&
           [datei_inhalt(pfad) isEqualToString:@"v1138-m1234-s0\n"],
           "Wunsch v1138-m1234-s0 ueber den echten Eingabekanal: Wechsel, Liste mit Wunsch (gestreamt, nicht Haupt), bildschirm.txt");
    pruefe(zeilen_mit(logpfad, "-> Kennung 2 (v1138-m1234-s0) \"X27 X1\" (Wunsch des Zuschauers)") == 1, "Protokollzeile mit Grund \"Wunsch des Zuschauers\"");
    // Der gewuenschte faellt weg: Ausweichplatz Hauptbildschirm, der Wunsch bleibt.
    liste_setzen(@[ B2 ]);
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();
    usleep(600 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_weg = liste_lesen(g.liste, g.liste_n, wunsch, 0, &eb);
    stand_zeile("der gewuenschte Bildschirm ist weg");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 4 && atomic_load(&g_fabrik_zuletzt) == 6 && display_jetzt() == 6 && atomic_load(&g_input_display) == 6 &&
           g.wechsel == 1 && n_weg == 1 && strcmp(wunsch, "v1138-m1234-s0") == 0 && eb.flags == (QC_BILDSCHIRM_FLAG_HAUPT | QC_BILDSCHIRM_FLAG_GESTREAMT) &&
           [wunsch_jetzt() isEqualToString:@"v1138-m1234-s0"],
           "der gewuenschte Bildschirm faellt weg: Wechsel auf den Hauptbildschirm, der Wunsch bleibt in der Liste");
    pruefe(zeilen_mit(logpfad, "-> Kennung 6 (v0-m0-s0) \"Virtuell 16:9\" (Ausweichplatz)") == 1 &&
           zeilen_mit(logpfad, "1920x1080 Pixel, 240 Hz (Hauptbildschirm) - Ausweichplatz") == 1, "Protokollzeilen zum Ausweichplatz");
    // Er kommt zurueck: von selbst wieder hin.
    liste_setzen(@[ B2, A2 ]);
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();
    usleep(600 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    liste_lesen(g.liste, g.liste_n, wunsch, 1, &ea);
    stand_zeile("er ist zurueck");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 5 && atomic_load(&g_fabrik_zuletzt) == 2 && display_jetzt() == 2 && atomic_load(&g_input_display) == 2 &&
           g.wechsel == 1 && ea.flags == QC_BILDSCHIRM_FLAG_GESTREAMT && strcmp(wunsch, "v1138-m1234-s0") == 0,
           "der gewuenschte Bildschirm kommt zurueck: der Host wechselt von selbst zurueck");
    pruefe(zeilen_mit(logpfad, "(zurueck zum gewuenschten Bildschirm)") == 1, "Protokollzeile mit Grund \"zurueck zum gewuenschten Bildschirm\"");

    printf("\n-- Bildschirm: andere Groesse (Encoder neu, INFO, Vollbild, letztes Bild weg)\n");
    QCBildschirm *C = bs(@"v7-m7-s7", @"Klein", 9, 1280, 720, 60, NO);
    liste_setzen(@[ B2, A2, C ]);
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();          // nur die Liste aendert sich: keine Umschaltung, aber eine neue Liste
    usleep(600 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_c = liste_lesen(g.liste, g.liste_n, wunsch, 2, &e2);
    pruefe(atomic_load(&g_fabrik_aufrufe) == 5 && g.wechsel == 0 && n_c == 3 && strcmp(e2.kennung, "v7-m7-s7") == 0 && e2.w == 1280,
           "ein Bildschirm kommt dazu, der gewuenschte laeuft weiter: kein Wechsel, nur die neue Liste");
    // Ein letztes Bild des alten Bildschirms liegt an (jeder Wechsel raeumt
    // es weg, deshalb hier frisch einspeisen).
    stdout_stumm(1);
    bild_einspeisen(pb_gross);
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int bild_vorher = letztes_bild_da() && g.bilder == 1;
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v7-m7-s7"));
    usleep(400 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    VTCompressionSessionRef s_c = session_jetzt();
    printf("         (nach dem Wechsel auf 1280x720: %s; INFO %ux%u)\n", g.folge, info_w(&g), info_h(&g));
    stand_zeile("danach");
    pruefe(bild_vorher && atomic_load(&g_fabrik_aufrufe) == 6 && atomic_load(&g_fabrik_zuletzt) == 9 && display_jetzt() == 9 &&
           atomic_load(&g_input_display) == 9 && strstr(g.folge, "7 1 12") && g.wechsel == 1 && info_w(&g) == 1280 && info_h(&g) == 720 &&
           g_info_w == 1280 && g_info_h == 720 && s_c != NULL && atomic_load(&g_fabrik_cfg_w) == 1280 && atomic_load(&g_fabrik_cfg_h) == 720 &&
           zeilen_mit(logpfad, "Encoder: Kandidat 3 HEVC 4:2:0 8 Bit, 1280x720") == 1,
           "andere Groesse: Encoder neu (1280x720), g_cfg und INFO mit 1280x720, SWITCH davor, Liste danach, Maus folgt");
    pruefe(!letztes_bild_da() && g.bilder == 0, "das liegengebliebene Bild des alten Bildschirms ist weg, ohne Bild geht nichts hinaus");
    stdout_stumm(1);
    bild_einspeisen(pb_klein);
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(g.bilder == 1 && g.erstes_voll == 1, "das erste Bild in der neuen Groesse ist ein Vollbild");

    printf("\n-- Bildschirm: Wunsch waehrend eines Codecwechsels wartet\n");
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 1; });
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v1138-m1234-s0"));
    usleep(700 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 200, &g);
    stdout_stumm(0);
    liste_lesen(g.liste, g.liste_n, wunsch, 2, &e2);
    int wartet = atomic_load(&g_fabrik_aufrufe) == 6 && display_jetzt() == 9 && g.wechsel == 0 && g.liste_n > 0 &&
                 strcmp(wunsch, "v1138-m1234-s0") == 0 && e2.flags == QC_BILDSCHIRM_FLAG_GESTREAMT;
    int wartet_zeile = zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel") == 1;
    stand_zeile("waehrend des Codecwechsels");
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 0; });
    stdout_stumm(1);
    usleep(700 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    stand_zeile("nach dem Codecwechsel");
    pruefe(wartet && wartet_zeile, "solange der Codecwechsel laeuft: kein Bildschirmwechsel, die Antwort (Liste mit Wunsch) kommt trotzdem, eine Zeile");
    pruefe(atomic_load(&g_fabrik_aufrufe) == 7 && atomic_load(&g_fabrik_zuletzt) == 2 && display_jetzt() == 2 && g.wechsel == 1 &&
           info_w(&g) == 1920 && zeilen_mit(logpfad, "(Wunsch des Zuschauers)") == 3 && zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel") == 1,
           "danach folgt der Wechsel, mit dem Grund des Wunsches (der dritte Wunsch-Wechsel), ohne weitere Wartezeile");

    printf("\n-- Bildschirm: der Wartezustand endet, sobald kein Wechsel mehr ansteht\n");
    // Ausgang: Strom auf Kennung 2 (Wunsch v1138-m1234-s0), Liste [B2, A2, C],
    // kein Codecwechsel. Ein Wunsch auf C muss auf den Codecwechsel warten;
    // der naechste Wunsch, zurueck auf den gestreamten Bildschirm, macht den
    // Wechsel hinfaellig. Jeder Wunsch bekommt seine Antwort (Spezifikation
    // 2.2), der Wartezustand endet mit dem hinfaelligen Wechsel - sonst
    // bliebe er stehen und unterdrueckte jede spaetere Antwort und Zeile
    // (Befund der Gegenpruefung) und erzwaenge nach 5 s einen spaeteren
    // Wechsel mitten im Codecwechsel.
    int warte_vorher = zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel");
    int fehlt_vorher = zeilen_mit(logpfad, "nicht angeschlossen, Ausweichplatz");
    int gewaehlt_vorher = zeilen_mit(logpfad, "Bildschirm gewaehlt:");
    int wunsch_vorher = zeilen_mit(logpfad, "(Wunsch des Zuschauers)");
    int fabrik_vorher = atomic_load(&g_fabrik_aufrufe);
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 1; });
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v7-m7-s7"));          // muss warten
    usleep(100 * 1000);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v1138-m1234-s0"));   // zurueck auf den gestreamten
    usleep(700 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int listen = typen_zaehlen(g.folge, QC_MSG_BILDSCHIRME);
    liste_lesen(g.liste, g.liste_n, wunsch, 1, &ea);
    uint64_t seit_hinfaellig = wartet_seit_jetzt();
    printf("         (waehrend des Codecwechsels: Folge \"%s\", letzte Liste mit Wunsch \"%s\", %s Flags %d; Wartezeilen %d -> %d; wartet seit %llu)\n",
           g.folge, wunsch, ea.kennung, ea.flags, warte_vorher, zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel"),
           (unsigned long long)seit_hinfaellig);
    pruefe(listen == 2 && strcmp(wunsch, "v1138-m1234-s0") == 0 && ea.flags == QC_BILDSCHIRM_FLAG_GESTREAMT && g.wechsel == 0 &&
           atomic_load(&g_fabrik_aufrufe) == fabrik_vorher && display_jetzt() == 2 &&
           zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel") == warte_vorher + 1,
           "zwei Wuensche waehrend des Codecwechsels (weg und zurueck): je eine Liste als Antwort, die letzte mit dem letzten Wunsch, kein Wechsel, eine Wartezeile");
    pruefe(seit_hinfaellig == 0, "der Wartezustand endet, sobald kein Wechsel mehr ansteht (das Ziel ist wieder der gestreamte Bildschirm)");
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 0; });
    stdout_stumm(1);
    usleep(500 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 200, &g);
    stdout_stumm(0);
    pruefe(atomic_load(&g_fabrik_aufrufe) == fabrik_vorher && g.wechsel == 0 && display_jetzt() == 2 && [wunsch_jetzt() isEqualToString:@"v1138-m1234-s0"],
           "nach dem Ende des Codecwechsels bleibt es beim gestreamten Bildschirm: kein Wechsel");
    // Danach bekommt ein unerfuellbarer Wunsch wieder Antwort und Zeile (2.3).
    // Der Strom laeuft auf A (nicht Haupt): der Ausweichplatz ist der
    // Hauptbildschirm B, also ein Wechsel mit Grund "Ausweichplatz".
    int ausweich_vorher = zeilen_mit(logpfad, "(Ausweichplatz)");
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v9-m9-s9"));
    usleep(400 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_v9 = liste_lesen(g.liste, g.liste_n, wunsch, 0, &eb);
    printf("         (nach dem Wunsch v9-m9-s9: Folge \"%s\", %d Eintraege, Wunsch \"%s\", %s Flags %d; Zeilen 'nicht angeschlossen' %d -> %d)\n",
           g.folge, n_v9, wunsch, eb.kennung, eb.flags, fehlt_vorher, zeilen_mit(logpfad, "nicht angeschlossen, Ausweichplatz"));
    pruefe(n_v9 == 3 && strcmp(wunsch, "v9-m9-s9") == 0 && eb.flags == (QC_BILDSCHIRM_FLAG_HAUPT | QC_BILDSCHIRM_FLAG_GESTREAMT) &&
           strstr(g.folge, "7 1 12") && g.wechsel == 1 && atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 1 && display_jetzt() == 6 &&
           zeilen_mit(logpfad, "nicht angeschlossen, Ausweichplatz Kennung 6 (v0-m0-s0) \"Virtuell 16:9\"") == fehlt_vorher + 1 &&
           zeilen_mit(logpfad, "(Ausweichplatz)") == ausweich_vorher + 1,
           "danach bekommt ein Wunsch auf einen nicht angeschlossenen Bildschirm wieder seine Zeile und die Liste mit dem Wunsch, hier mit Wechsel auf den Ausweichplatz");
    // Ein neues Warten beginnt frisch: die 5-s-Frist zaehlt ab SEINEM Beginn.
    // Zurueck auf A (ein Wechsel, kein Codecwechsel), dann Wunsch auf C bei
    // laufendem Codecwechsel: er wartet, nichts wird erzwungen.
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v1138-m1234-s0"));
    usleep(400 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    int zurueck_auf_a = atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 2 && g.wechsel == 1 && display_jetzt() == 2;
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 1; });
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v7-m7-s7"));
    usleep(100 * 1000);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v7-m7-s7"));   // derselbe Wunsch noch einmal: auch er bekommt seine Antwort
    usleep(700 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    uint64_t seit_neu = wartet_seit_jetzt();
    printf("         (Wunsch v7-m7-s7 bei laufendem Codecwechsel: Folge \"%s\", Fabrik %d, gestreamt %u, Wartezeilen %d, 'trotzdem' %d, wartet seit %llu)\n",
           g.folge, atomic_load(&g_fabrik_aufrufe), display_jetzt(), zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel"),
           zeilen_mit(logpfad, "Codecwechsel nach 5 s nicht fertig - Bildschirmwechsel trotzdem"), (unsigned long long)seit_neu);
    pruefe(zurueck_auf_a && seit_neu != 0 && atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 2 && g.wechsel == 0 && display_jetzt() == 2 &&
           typen_zaehlen(g.folge, QC_MSG_BILDSCHIRME) == 2 && zeilen_mit(logpfad, "wartet auf den laufenden Codecwechsel") == warte_vorher + 2 &&
           zeilen_mit(logpfad, "Codecwechsel nach 5 s nicht fertig - Bildschirmwechsel trotzdem") == 0,
           "ein neues Warten beginnt frisch: der Wechsel wartet mit einer Zeile, jeder Wunsch bekommt seine Antwort, nichts wird erzwungen");
    // Die 5-s-Frist selbst: liegt der Beginn des Wartens laenger zurueck,
    // kommt der Wechsel trotzdem, mit Zeile, und das Warten ist zu Ende.
    stdout_stumm(1);
    dispatch_sync(g_lifeq, ^{ g_bildschirm_wartet_seit -= 6ull * 1000000ull; });
    usleep(500 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    stand_zeile("Codecwechsel nach 5 s nicht fertig");
    pruefe(atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 3 && atomic_load(&g_fabrik_zuletzt) == 9 && display_jetzt() == 9 &&
           strstr(g.folge, "7 1 12") && g.wechsel == 1 && wartet_seit_jetzt() == 0 &&
           zeilen_mit(logpfad, "Codecwechsel nach 5 s nicht fertig - Bildschirmwechsel trotzdem") == 1 &&
           zeilen_mit(logpfad, "(Wunsch des Zuschauers)") == wunsch_vorher + 2,
           "nach 5 s kommt der Bildschirmwechsel trotzdem, mit dem Grund des Wunsches, und das Warten ist zu Ende");
    dispatch_sync(g_capq, ^{ g_wechsel_aktiv = 0; });
    // Zurueck auf A fuer die naechsten Abschnitte. Die Zeile "Bildschirm
    // gewaehlt" kam je Ziel einmal (v7-m7-s7 beim Warten, Ausweichplatz,
    // zurueck auf A, v7-m7-s7 beim zweiten Warten - nicht noch einmal fuer
    // den wiederholten Wunsch -, jetzt), nie fuer die Wiederholungen des
    // Wartens.
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(@"v1138-m1234-s0"));
    usleep(400 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    printf("         (Zeilen \"Bildschirm gewaehlt\": %d -> %d)\n", gewaehlt_vorher, zeilen_mit(logpfad, "Bildschirm gewaehlt:"));
    pruefe(atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 4 && display_jetzt() == 2 && g.wechsel == 1 && info_w(&g) == 1920 &&
           zeilen_mit(logpfad, "Bildschirm gewaehlt:") == gewaehlt_vorher + 5,
           "zurueck auf den grossen Bildschirm: Wechsel, INFO 1920x1080, je Ziel eine Zeile \"Bildschirm gewaehlt\" (nie fuer die Wiederholungen des Wartens)");

    printf("\n-- Bildschirm: leere Liste bei laufendem Strom\n");
    // Meldet ScreenCaptureKit voruebergehend keinen Bildschirm, laeuft der
    // Strom weiter: kein Ziel wechseln, kein Neuaufbau, wenn die Liste
    // zurueckkommt - faellt der Bildschirm wirklich weg, endet der Strom
    // (didStopWithError), und die Wiederherstellung bewertet neu.
    fabrik_vorher = atomic_load(&g_fabrik_aufrufe);
    liste_setzen(@[]);
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();
    usleep(600 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_leer = liste_lesen(g.liste, g.liste_n, wunsch, -1, NULL);
    printf("         (leere Liste: Folge \"%s\", %d Eintraege, gestreamt %u)\n", g.folge, n_leer, display_jetzt());
    pruefe(strom_jetzt() != nil && atomic_load(&g_fabrik_aufrufe) == fabrik_vorher && g.wechsel == 0 && display_jetzt() == 2 &&
           typen_zaehlen(g.folge, QC_MSG_BILDSCHIRME) == 1 && n_leer == 0,
           "leere Liste bei laufendem Strom: der Strom und sein Bildschirm bleiben, die leere Liste geht hinaus");
    liste_setzen(@[ B2, A2, C ]);
    stdout_stumm(1);
    bildschirm_konfiguration_geaendert();
    usleep(600 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_wieder = liste_lesen(g.liste, g.liste_n, wunsch, 1, &ea);
    pruefe(atomic_load(&g_fabrik_aufrufe) == fabrik_vorher && g.wechsel == 0 && display_jetzt() == 2 && n_wieder == 3 &&
           ea.flags == QC_BILDSCHIRM_FLAG_GESTREAMT,
           "kommt die Liste zurueck, bleibt der Strom stehen: kein unnoetiger Neuaufbau, die Liste traegt den gestreamten Bildschirm");

    printf("\n-- Bildschirm: Bildschirmverlust und Wiederherstellung (ohne Hauptwarteschlange)\n");
    fabrik_vorher = atomic_load(&g_fabrik_aufrufe);
    NSError *err = [NSError errorWithDomain:@"hosttest" code:-3801 userInfo:@{ NSLocalizedDescriptionKey: @"Bildschirm weg" }];
    stdout_stumm(1);
    [g_grab stream:strom_jetzt() didStopWithError:err];
    usleep(300 * 1000);
    SCStream *nach_verlust = strom_jetzt();
    alles_lesen(&H, 200, &g);
    int verlust = nach_verlust == nil && g.status == 1 && g.statusse == 1;
    usleep(2300 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(verlust, "Bildschirmverlust: der Strom ist weg, der Zuschauer bekommt Hoststatus 1");
    stand_zeile("nach der Wiederherstellung");
    pruefe(strom_jetzt() != nil && atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 1 && atomic_load(&g_fabrik_zuletzt) == 2 && g.status == 0 && g.liste_n > 0 &&
           zeilen_mit(logpfad, "Aufnahme wiederhergestellt: Bildschirm 1920x1080, 120 Hz, Kennung 2 (v1138-m1234-s0) \"X27 X1\"") == 1,
           "nach 2 s ist die Aufnahme auf dem gewuenschten Bildschirm wieder da: Hoststatus 0, Liste, Zeile");

    printf("\n-- Bildschirm: Wunsch Automatik ueber den Eingabekanal\n");
    // Der Zuschauer stellt auf Automatik, waehrend der gewuenschte Bildschirm
    // laeuft: Wechsel auf den Hauptbildschirm mit dem Grund des Wunsches -
    // nicht "Hauptbildschirm gewechselt", der blieb, wo er war (Befund des
    // Livetests). Liste ohne Wunsch, bildschirm.txt "auto".
    fabrik_vorher = atomic_load(&g_fabrik_aufrufe);
    int haupt_zeilen_vorher = zeilen_mit(logpfad, "(Hauptbildschirm gewechselt)");
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_BILDSCHIRM, qc_bildschirm_wunsch_kodieren(nil));
    usleep(300 * 1000);
    dispatch_sync(g_lifeq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    int n_auto = liste_lesen(g.liste, g.liste_n, wunsch, 0, &eb);
    stand_zeile("nach dem Wunsch Automatik");
    pruefe(atomic_load(&g_fabrik_aufrufe) == fabrik_vorher + 1 && atomic_load(&g_fabrik_zuletzt) == 6 && display_jetzt() == 6 &&
           atomic_load(&g_input_display) == 6 && g.wechsel == 1 && n_auto == 3 && wunsch[0] == 0 &&
           eb.flags == (QC_BILDSCHIRM_FLAG_HAUPT | QC_BILDSCHIRM_FLAG_GESTREAMT) && wunsch_jetzt() == nil &&
           [datei_inhalt(pfad) isEqualToString:@"auto\n"],
           "Wunsch Automatik ueber den Eingabekanal: Wechsel auf den Hauptbildschirm, Liste ohne Wunsch, bildschirm.txt \"auto\"");
    pruefe(zeilen_mit(logpfad, "-> Kennung 6 (v0-m0-s0) \"Virtuell 16:9\" (Wunsch des Zuschauers: Automatik)") == 1 &&
           zeilen_mit(logpfad, "(Hauptbildschirm gewechselt)") == haupt_zeilen_vorher,
           "Protokollzeile mit Grund \"Wunsch des Zuschauers: Automatik\", keine weitere \"Hauptbildschirm gewechselt\"");

    printf("\n-- HDR: IN_ANZEIGE ueber den Eingabekanal (noch ohne HDR-Encoder)\n");
    // Bis hier hat H kein IN_ANZEIGE geschickt: jede Strominfo trug Grund 7.
    // Jetzt meldet er einen HDR-Schirm mit Wunsch Automatisch: neu
    // entschieden, eine neue Strominfo in SDR mit dem Grund des Hosts (Codec
    // 2 bzw. Host kann nicht 4), kein SWITCH, kein neuer Encoder. Dieselbe
    // Lage und nur ein anderer Kopfraum: nichts. Wunsch Aus: Grund 1.
    // Unlesbares wird uebergangen, der Kanal bleibt.
    int idx_jetzt = atomic_load(&g_codec_id);
    int grund_auto = qc_hdr_codec_kann(idx_jetzt) ? QC_HDR_GRUND_HOST_KANN_NICHT : QC_HDR_GRUND_CODEC;
    VTCompressionSessionRef s_hdr = session_jetzt();
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(QC_HDR_ANZEIGE_SCHIRM_HDR, QC_HDR_WUNSCH_AUTOMATISCH, 417));
    usleep(200 * 1000);
    dispatch_sync(g_capq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    printf("         (nach IN_ANZEIGE: %s, Grund %d)\n", g.folge, g.info_n >= 14 ? g.info[13] : -1);
    pruefe(g.infos == 1 && g.wechsel == 0 && g.info_n == QC_HDR_INFO_LAENGE && g.info[9] == QC_HDR_TRANSFER_SDR &&
           g.info[13] == grund_auto && session_jetzt() == s_hdr,
           "IN_ANZEIGE (HDR-Schirm, Automatisch): eine neue Strominfo in SDR mit dem Grund des Hosts, kein SWITCH, Encoder bleibt");
    pruefe(zeilen_mit(logpfad, "IN_ANZEIGE: Schirm HDR, Darstellung nein, Wunsch 0, Weiss 240 nit, Spitze 1000 nit, Kopfraum 4.17/4.17") == 1 &&
           zeilen_mit(logpfad, "HDR-Entscheidung (IN_ANZEIGE): SDR, Grund") == 1,
           "Protokoll: die Lage des Clients und die Entscheidung");
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(QC_HDR_ANZEIGE_SCHIRM_HDR, QC_HDR_WUNSCH_AUTOMATISCH, 417));
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(QC_HDR_ANZEIGE_SCHIRM_HDR, QC_HDR_WUNSCH_AUTOMATISCH, 250));
    usleep(200 * 1000);
    dispatch_sync(g_capq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(g.infos == 0, "dieselbe Lage oder nur ein anderer Kopfraum: keine neue Strominfo");
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(QC_HDR_ANZEIGE_SCHIRM_HDR, QC_HDR_WUNSCH_AUS, 417));
    usleep(200 * 1000);
    dispatch_sync(g_capq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(g.infos == 1 && g.info[13] == QC_HDR_GRUND_CLIENT_SDR, "Wunsch Aus: neue Strominfo mit Grund 1");
    NSData *kurz = [anzeige_daten(0, QC_HDR_WUNSCH_AUTOMATISCH, 100) subdataWithRange:NSMakeRange(0, 13)];
    NSMutableData *fremd = [anzeige_daten(0, QC_HDR_WUNSCH_AUTOMATISCH, 100) mutableCopy];
    ((uint8_t *)fremd.mutableBytes)[0] = 2;
    stdout_stumm(1);
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, kurz);
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, fremd);
    usleep(200 * 1000);
    dispatch_sync(g_capq, ^{});
    alles_lesen(&H, 300, &g);
    int nach_kaputt = g.infos;
    ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(0, QC_HDR_WUNSCH_AUTOMATISCH, 100));
    usleep(200 * 1000);
    dispatch_sync(g_capq, ^{});
    alles_lesen(&H, 300, &g);
    stdout_stumm(0);
    pruefe(nach_kaputt == 0 && zeilen_mit(logpfad, "IN_ANZEIGE ungueltig (13 Byte)") == 1 && zeilen_mit(logpfad, "IN_ANZEIGE ungueltig (14 Byte)") == 1 &&
           g.infos == 1 && g.info[13] == grund_auto,
           "zu kurz und fremde Fassung: uebergangen und protokolliert, der Kanal lebt (SDR-Schirm, Automatisch: wieder der Grund des Hosts)");


    printf("\n-- HDR: Aushandlung bis HDR10 und zurueck (Attrappen fuer Liste und Strom, echter Encoder 1920x1080)\n");
    // Der Weg des Mac-Hosts (HDR-Plan 5.1): HDR am Bildschirm an/aus,
    // IN_ANZEIGE, Codecwechsel mit Farbe, Sperre 2 s, Grund 6 bei einer
    // Aufnahme, die HDR ablehnt, und eine Abloesung mitten in HDR10.
    schein H2;
    memset(&H2, 0, sizeof H2);
    H2.bild = H2.ein = -1;
    if (!hdr_system_kann() || !g_befund[0].hdr) {
        printf("         (dieser Mac kann kein HDR10 - Abschnitt uebersprungen)\n");
    } else {
        OSType xf44 = kCVPixelFormatType_444YpCbCr10BiPlanarFullRange;
        CVPixelBufferRef pb_sdr = formatpuffer(1920, 1080, xf44, 800, 512, 512);
        // Wie ScreenCaptureKit in HDR (HDRStream...): Display P3 PQ, Matrix BT.709.
        CVPixelBufferRef pb_pq = formatpuffer(1920, 1080, xf44, 594, 512, 512);
        CVBufferSetAttachment(pb_pq, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_P3_D65, kCVAttachmentMode_ShouldPropagate);
        CVBufferSetAttachment(pb_pq, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVAttachmentMode_ShouldPropagate);
        CVBufferSetAttachment(pb_pq, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVAttachmentMode_ShouldPropagate);
        NSData *sei_soll = hex(QC_SEI_P3_1000);
        uint8_t info_pq_soll[QC_HDR_INFO_LAENGE - QC_HDR_INFO_ALT];
        { qc_hdr_info i; hdr_info_pq(&i); qc_hdr_info_kodieren(&i, info_pq_soll); }
        nal_t nals[64];
        strom_befund sb;
        uint8_t hdr_an = QC_HDR_ANZEIGE_SCHIRM_HDR | QC_HDR_ANZEIGE_DARSTELLUNG;

        // 1. HDR am Bildschirm des Hosts an (EDR-Kopfraum 4): erkannt; mit
        //    HEVC 4:2:0 8 Bit bleibt es SDR, Grund 2 wie zuvor.
        QCBildschirm *B3 = bs(@"v0-m0-s0", @"Virtuell 16:9", 6, 1920, 1080, 240, YES);
        B3.edr_potentiell = 4.0;
        B3.edr_aktuell = 4.0;
        stdout_stumm(1);
        liste_setzen(@[ B3, A2, C ]);
        bildschirm_konfiguration_geaendert();
        usleep(450 * 1000);
        dispatch_sync(g_lifeq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        pruefe(atomic_load(&g_quelle_hdr) == 1 && g.wechsel == 0 && g.infos == 0 &&
               zeilen_mit(logpfad, "Bildschirm des Hosts: HDR an (EDR-Kopfraum 4.00) - Bildschirmkonfiguration") == 1,
               "HDR am Bildschirm des Hosts an: erkannt und protokolliert; HEVC 4:2:0 8 Bit bleibt SDR (Grund 2), keine neue Strominfo");

        // 2. Codecwunsch HEVC 4:4:4 10 Bit: der Client hat keine
        //    HDR-Darstellung gemeldet - SDR, Grund 5.
        uint8_t c0 = 0, c3 = 3;
        stdout_stumm(1);
        ein_senden(H.ein, &H.tx, QC_IN_CODEC, &c0, 1);
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        bild_einspeisen(pb_sdr);
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        size_t k = g_letztes_vollbild ? nals_zerlegen(g_letztes_vollbild.bytes, g_letztes_vollbild.length, nals, 64) : 0;
        strom_pruefen(nals, k, sei_soll, &sb);
        printf("         (Codec 0 ohne Darstellung: %s, Grund %d, Vollbild mit %d SEI 137)\n", g.folge, g.info[13], sb.sei_richtig + sb.sei_sonst);
        pruefe(g.wechsel == 1 && g.wechsel_codec == 0 && g.wechsel_transfer == QC_HDR_TRANSFER_SDR && g.infos == 1 &&
               g.info[9] == QC_HDR_TRANSFER_SDR && g.info[13] == QC_HDR_GRUND_CLIENT_OHNE_DARSTELLUNG && !atomic_load(&g_farbe_pq) &&
               g_cfg.pixelFormat == xf44 && !aufnahme_ist_pq(g_cfg) && g.voll == 1 && sb.vollbilder == 1 && sb.sei_richtig + sb.sei_sonst == 0,
               "Codecwunsch 4:4:4 10 Bit ohne HDR-Darstellung beim Client: SWITCH mit Transfer 1, Strominfo SDR Grund 5, Vollbild ohne SEI 137");

        // 3. IN_ANZEIGE: HDR-Schirm und Darstellung, Automatisch -> HDR10.
        stdout_stumm(1);
        ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(hdr_an, QC_HDR_WUNSCH_AUTOMATISCH, 400));
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        uint64_t t_hdr = now_us();
        bild_einspeisen(pb_pq);
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        k = g_letztes_vollbild ? nals_zerlegen(g_letztes_vollbild.bytes, g_letztes_vollbild.length, nals, 64) : 0;
        strom_pruefen(nals, k, sei_soll, &sb);
        int dr_ok = 1;
        if (@available(macOS 15.0, *)) dr_ok = g_cfg.captureDynamicRange == SCCaptureDynamicRangeHDRCanonicalDisplay;
        printf("         (HDR10: %s, SWITCH Codec %d Transfer %d Umrechnung %d, Grund %d, Vollbild: SEI richtig %d)\n", g.folge,
               g.wechsel_codec, g.wechsel_transfer, g.wechsel_umrechnung, g.info[13], sb.sei_richtig);
        pruefe(g.wechsel == 1 && g.wechsel_codec == 0 && g.wechsel_transfer == QC_HDR_TRANSFER_PQ && g.wechsel_umrechnung == 1 &&
               g.infos == 1 && g.info_n == QC_HDR_INFO_LAENGE && memcmp(g.info + QC_HDR_INFO_ALT, info_pq_soll, sizeof info_pq_soll) == 0 &&
               atomic_load(&g_farbe_pq) == 1,
               "IN_ANZEIGE (HDR-Schirm, Darstellung, Automatisch): SWITCH Transfer 16 (P3 -> BT.2020 als Umrechnung), Strominfo PQ/2020/2020 voll, Grund 0, Weiss 203, Mastering 1000/0,005");
        pruefe(aufnahme_ist_pq(g_cfg) && CFEqual(g_cfg.colorSpaceName, kCGColorSpaceDisplayP3_PQ) &&
               g_cfg.colorMatrix && CFEqual(g_cfg.colorMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2) && dr_ok && g_cfg.pixelFormat == xf44,
               "Aufnahme in HDR: kanonisch, Display P3 PQ, Matrix BT.709, xf44 (Apples HDRStreamCanonicalDisplay)");
        pruefe(g.voll == 1 && sb.vollbilder == 1 && sb.sei_richtig == 1 && sb.sei_sonst == 0,
               "das Vollbild beim Zuschauer: VPS, SPS, PPS, dann unsere SEI 137/144 (byte-genau)");
        pruefe(zeilen_mit(logpfad, "HDR-Entscheidung (IN_ANZEIGE): HDR10, Grund 0 (HDR aktiv) - Farbwechsel SDR -> HDR10") == 1 &&
               zeilen_mit(logpfad, "Aufnahme: SDR -> HDR10 - kanonisch, Display P3 PQ, Matrix BT.709") == 1 &&
               zeilen_mit(logpfad, "Aufnahme: erstes Bild xf44 1920x1080, P3_D65 / SMPTE_ST_2084_PQ / ITU_R_709_2 (Encoder HDR10)") >= 1,
               "Protokoll: Entscheidung, Umstellen der Aufnahme, Anhaenge des ersten Bildes");

        // 4. Gleich danach Wunsch Aus: keine 2 s seit dem letzten Farbwechsel -
        //    der Wechsel nach SDR kommt, wenn die Sperre ablaeuft.
        stdout_stumm(1);
        ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(hdr_an, QC_HDR_WUNSCH_AUS, 400));
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 200, &g);
        int sofort = g.wechsel, sofort_infos = g.infos;
        uint64_t bis = t_hdr + 2300000ull;
        while (now_us() < bis) usleep(50 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        printf("         (Wunsch Aus: sofort %d SWITCH, %d Strominfo; nach der Sperre %s, Grund %d)\n", sofort, sofort_infos, g.folge, g.info[13]);
        pruefe(sofort == 0 && sofort_infos == 0 && zeilen_mit(logpfad, "der letzte Farbwechsel ist keine 2 s her") == 1,
               "Farbwechsel binnen 2 s: nicht sofort, sondern vorgemerkt (Zeile)");
        pruefe(g.wechsel == 1 && g.wechsel_transfer == QC_HDR_TRANSFER_SDR && g.infos == 1 && g.info[9] == QC_HDR_TRANSFER_SDR &&
               g.info[13] == QC_HDR_GRUND_CLIENT_SDR && !atomic_load(&g_farbe_pq) && !aufnahme_ist_pq(g_cfg) &&
               g_cfg.colorMatrix && CFEqual(g_cfg.colorMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2),
               "nach Ablauf der Sperre: SWITCH Transfer 1, Strominfo SDR Grund 1, Aufnahme sRGB mit Matrix BT.709");

        // 5. Die Aufnahme lehnt HDR ab: SDR bleibt, Grund 6, kein SWITCH;
        //    dieselbe Lage versucht es nicht noch einmal, eine neue schon.
        dispatch_sync(g_capq, ^{ g_farbe_gewechselt_us = 0; });
        atomic_store(&g_fake_umstellen_fehler, 1);
        int versuche_vorher = zeilen_mit(logpfad, "Farbwechsel: SDR -> HDR10");
        stdout_stumm(1);
        ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(hdr_an, QC_HDR_WUNSCH_AUTOMATISCH, 400));
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        __block VTCompressionSessionRef s6 = NULL;
        dispatch_sync(g_capq, ^{ s6 = g_session; });
        pruefe(g.wechsel == 0 && g.infos == 1 && g.info[13] == QC_HDR_GRUND_WECHSEL_GESCHEITERT && !atomic_load(&g_farbe_pq) &&
               !aufnahme_ist_pq(g_cfg) && s6 != NULL && zeilen_mit(logpfad, "Wechsel nach HDR10 gescheitert") == 1,
               "Aufnahme lehnt HDR ab: kein SWITCH, SDR laeuft weiter (neuer Encoder), Strominfo Grund 6, Aufnahme zurueck auf sRGB");
        stdout_stumm(1);
        ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(hdr_an, QC_HDR_WUNSCH_AUTOMATISCH, 300));
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 200, &g);
        stdout_stumm(0);
        pruefe(g.wechsel == 0 && g.infos == 0 && zeilen_mit(logpfad, "Farbwechsel: SDR -> HDR10") == versuche_vorher + 1,
               "dieselbe Lage (nur ein anderer Kopfraum): kein neuer Versuch, keine neue Strominfo");
        atomic_store(&g_fake_umstellen_fehler, 0);
        stdout_stumm(1);
        ein_daten(H.ein, &H.tx, QC_IN_ANZEIGE, anzeige_daten(hdr_an, QC_HDR_WUNSCH_IMMER, 300));
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        pruefe(g.wechsel == 1 && g.wechsel_transfer == QC_HDR_TRANSFER_PQ && g.info[13] == QC_HDR_GRUND_AKTIV && atomic_load(&g_farbe_pq),
               "eine neue Lage (Wunsch Immer): neuer Versuch, jetzt HDR10");

        // 6. HDR am Bildschirm des Hosts aus: SDR, Grund 3.
        dispatch_sync(g_capq, ^{ g_farbe_gewechselt_us = 0; });
        QCBildschirm *B4 = bs(@"v0-m0-s0", @"Virtuell 16:9", 6, 1920, 1080, 240, YES);
        B4.edr_potentiell = 1.0;
        B4.edr_aktuell = 1.0;
        stdout_stumm(1);
        liste_setzen(@[ B4, A2, C ]);
        bildschirm_konfiguration_geaendert();
        usleep(450 * 1000);
        dispatch_sync(g_lifeq, ^{});
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        pruefe(atomic_load(&g_quelle_hdr) == 0 && g.wechsel == 1 && g.wechsel_transfer == QC_HDR_TRANSFER_SDR &&
               g.info[13] == QC_HDR_GRUND_HOST_SCHIRM_SDR && !atomic_load(&g_farbe_pq) &&
               zeilen_mit(logpfad, "Bildschirm des Hosts: HDR aus (EDR-Kopfraum 1.00) - Bildschirmkonfiguration") == 1,
               "HDR am Bildschirm des Hosts aus: SWITCH Transfer 1, Strominfo Grund 3, Zeile");

        // 7. Wieder an, dann Codecwechsel mit Farbe: 4:2:0 8 Bit wird SDR
        //    (Grund 2), zurueck auf 4:4:4 10 Bit wieder HDR10 - ohne Sperre.
        dispatch_sync(g_capq, ^{ g_farbe_gewechselt_us = 0; });
        stdout_stumm(1);
        liste_setzen(@[ B3, A2, C ]);
        bildschirm_konfiguration_geaendert();
        usleep(450 * 1000);
        dispatch_sync(g_lifeq, ^{});
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        int wieder_an = g.wechsel == 1 && g.wechsel_transfer == QC_HDR_TRANSFER_PQ && atomic_load(&g_farbe_pq);
        ein_senden(H.ein, &H.tx, QC_IN_CODEC, &c3, 1);
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        gelesen g3 = g;
        OSType fmt3 = g_cfg.pixelFormat;
        int pq3 = aufnahme_ist_pq(g_cfg);
        ein_senden(H.ein, &H.tx, QC_IN_CODEC, &c0, 1);
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H, 300, &g);
        stdout_stumm(0);
        pruefe(wieder_an, "HDR am Host wieder an: HDR10");
        pruefe(g3.wechsel == 1 && g3.wechsel_codec == 3 && g3.wechsel_transfer == QC_HDR_TRANSFER_SDR && g3.info[13] == QC_HDR_GRUND_CODEC &&
               fmt3 == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange && !pq3 &&
               zeilen_mit(logpfad, "Codecwechsel: HEVC 4:4:4 10 Bit -> HEVC 4:2:0 8 Bit, Farbe HDR10 -> SDR") == 1,
               "Codecwunsch 4:2:0 8 Bit in HDR10: ein Wechsel mit Farbe - SWITCH Codec 3 Transfer 1, Grund 2, Aufnahme 420f sRGB");
        pruefe(g.wechsel == 1 && g.wechsel_codec == 0 && g.wechsel_transfer == QC_HDR_TRANSFER_PQ && g.info[13] == QC_HDR_GRUND_AKTIV &&
               aufnahme_ist_pq(g_cfg) && g_cfg.pixelFormat == xf44 &&
               zeilen_mit(logpfad, "Codecwechsel: HEVC 4:2:0 8 Bit -> HEVC 4:4:4 10 Bit, Farbe SDR -> HDR10") == 1,
               "zurueck auf 4:4:4 10 Bit: gleich wieder HDR10 (Codecwuensche kennen keine Sperre)");

        // 8. Ein anderer Zuschauer loest mitten in HDR10 ab: die Begruessung
        //    sagt, was laeuft (PQ), dann SDR fuer ihn - Grund 7, bis er sein
        //    IN_ANZEIGE schickt.
        dispatch_sync(g_capq, ^{ g_farbe_gewechselt_us = 0; });
        uint8_t h2_priv[32], h2_pub[32];
        qc_keypair(h2_priv, h2_pub);
        qc_zugang_eintragen(h2_pub, "hosttest H2");
        stdout_stumm(1);
        int ok2 = schein_verbinden(&H2, bild_port, ein_port, h2_priv) == 0;
        usleep(300 * 1000);
        dispatch_sync(g_capq, ^{});
        dispatch_sync(g_capq, ^{});
        alles_lesen(&H2, 300, &g);
        stdout_stumm(0);
        printf("         (Abloesung in HDR10: Begruessung Transfer %d, danach %s, Grund %d)\n", H2.info_n >= 10 ? H2.info[9] : -1, g.folge, g.info[13]);
        pruefe(ok2 && H2.info_n == QC_HDR_INFO_LAENGE && H2.info[9] == QC_HDR_TRANSFER_PQ && g.wechsel == 1 &&
               g.wechsel_transfer == QC_HDR_TRANSFER_SDR && g.info[13] == QC_HDR_GRUND_KEIN_IN_ANZEIGE && !atomic_load(&g_farbe_pq) &&
               zeilen_mit(logpfad, "HDR-Entscheidung (neuer Zuschauer): SDR, Grund 7") == 1,
               "Abloesung in HDR10: Begruessung mit PQ, dann SWITCH nach SDR und Strominfo Grund 7 fuer den Neuen");

        CVPixelBufferRelease(pb_sdr);
        CVPixelBufferRelease(pb_pq);
        dispatch_sync(g_capq, ^{ g_farbe_gewechselt_us = 0; });
        atomic_store(&g_hdr_gescheitert, 0);
    }

    zuschauer_weg();
    schein_schliessen(&H);
    schein_schliessen(&H2);
    stdout_stumm(1);
    stream_herunterfahren_anstossen();
    SCStream *st = strom_jetzt();
    stdout_stumm(0);
    pruefe(st == nil && !session_jetzt() && !letztes_bild_da(), "ohne Zuschauer: Strom, Encoder und letztes Bild weg");
    g_schein_attrappe = 1;
    liste_setzen(@[]);
    dispatch_sync(g_lifeq, ^{ g_display_wunsch = nil; g_display_id = 0; g_display_aktuell = nil; g_bildschirme = nil; g_display_pin = -1; });
    unlink(pfad);
    CVPixelBufferRelease(pb_gross);
    CVPixelBufferRelease(pb_klein);
    atomic_store(&g_bild_offen, 0);
    atomic_store(&g_codec_id, 0);
    g_cfg = nil;
    g_grab = nil;
    pthread_mutex_lock(&g_log_mtx);
    fclose(g_log);
    g_log = NULL;
    pthread_mutex_unlock(&g_log_mtx);
}

// ------------------------------------- Dienst-Takt und Abschied beim Beenden

// Der Takt ersetzt die fruehere 5-s-Schleife in main (die [NSApp run]
// weichen musste). Er laeuft auf eigener Warteschlange - hier ohne jede
// Run-Loop, mit 0,2 s statt 5 s.
static void takt_pruefen(void) {
    printf("\n-- Dienst-Takt ohne Run-Loop, Abschied beim Beenden\n");
    int h, c;
    if (paar(&h, &c, 0)) { pruefe(0, "Verbindung"); return; }
    zuschauer_setzen(h, kanal(h, 0x55));
    // Eine zurueckgehaltene Drosselzeile, deren Frist schon um ist: der Takt
    // muss sie nachtragen, wie frueher die Schleife.
    logf_gedrosselt(&d_ein_fremd, "10.9.8.7", @"Takt-Probe von %s", "10.9.8.7");
    logf_gedrosselt(&d_ein_fremd, "10.9.8.7", @"Takt-Probe von %s", "10.9.8.7");
    pruefe(drossel_weitere(&d_ein_fremd) >= 1, "vorher: eine Zeile zurueckgehalten");
    drossel_altern(&d_ein_fremd);

    double t0 = sek();
    dienst_takt_starten(0.2);
    leser l;
    leser_init(&l, c, 0x55);
    qc_hdr m;
    uint8_t anf[8];
    int last = 0, fassung_gut = 1;
    double erste = 0;
    while (last < 3 && sek() - t0 < 4.0) {
        if (nachricht(&l, &m, anf, 1500) != 1) break;
        if (m.type != QC_MSG_LAST) continue;
        if (!last) erste = sek() - t0;
        if (m.len != 28 || anf[0] != 1) fassung_gut = 0;
        last++;
    }
    double dauer = sek() - t0;
    printf("         (%d Auslastungsmeldungen in %.2f s, die erste nach %.2f s)\n", last, dauer, erste);
    pruefe(last == 3, "Auslastung (Typ 6) kommt im Takt, ohne dass jemand die Run-Loop dreht");
    pruefe(fassung_gut, "Auslastung: 28 Byte, Fassung 1");
    pruefe(erste >= 0.15 && erste < 1.0, "erste Runde nach einem vollen Takt");
    pruefe(drossel_weitere(&d_ein_fremd) == 0, "zurueckgehaltene Drosselzeile nachgetragen");
    dienst_takt_anhalten();

    // Beenden: der Host verabschiedet sich (Typ 13, Grund 0) und schliesst den
    // Bildkanal - ohne Typ 10, den der Client als "ein anderes Geraet hat
    // uebernommen" anzeigen wuerde.
    host_abschied();
    int e, typ10 = 0, letzte = -1, grund = -1;
    uint32_t laenge = 0;
    double t_ab = sek();
    while ((e = nachricht(&l, &m, anf, 2000)) == 1) {
        if (m.type == QC_MSG_ABGELOEST) typ10 = 1;
        letzte = m.type;
        laenge = m.len;
        grund = anf[0];
    }
    printf("         (letzte Nachricht Typ %d, Laenge %u, Grund %d)\n", letzte, laenge, grund);
    pruefe(!typ10, "Abschied: kein Typ 10 (der hiesse beim Client \"anderes Geraet hat uebernommen\")");
    pruefe(letzte == QC_MSG_HOST_ENDE && laenge == 1 && grund == QC_HOST_ENDE_BEENDET,
           "Abschied: Typ 13 mit Grund 0 (App beendet) als letzte Nachricht");
    pruefe(e == 0 && sek() - t_ab < 1.0, "der Host schliesst den Bildkanal sofort");
    pruefe(atomic_load(&g_client_fd) == -1 && g_vid == NULL && !atomic_load(&g_vid_ready),
           "Zuschauer ausgetragen");
    close(c);
    free(l.buf);
}

// ------------------------------------------------------------ Zugang
//
// Die Oberflaeche (P3) ersetzt hier ein Test-Haken: qc_ui_* schreiben mit,
// entschieden wird mit qc_zugang_entscheiden wie spaeter per Klick.

static _Atomic int g_test_ui = 0;
static _Atomic int g_test_zustand = 0;
static pthread_mutex_t g_test_ui_mtx = PTHREAD_MUTEX_INITIALIZER;
static char g_test_ui_folge[4096];                  // "a<nr> " gezeigt, "z<nr> " zurueck
static uint64_t g_test_anfrage = 0;                 // zuletzt gezeigte
static char g_test_anfrage_name[QC_ZUGANG_NAME_MAX + 1];
static uint32_t g_test_anfrage_id = 0, g_test_anfrage_code = 0;

void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code) {
    pthread_mutex_lock(&g_test_ui_mtx);
    size_t n = strlen(g_test_ui_folge);
    snprintf(g_test_ui_folge + n, sizeof g_test_ui_folge - n, "a%llu ", (unsigned long long)anfrage);
    g_test_anfrage = anfrage;
    snprintf(g_test_anfrage_name, sizeof g_test_anfrage_name, "%s", name);
    g_test_anfrage_id = id;
    g_test_anfrage_code = code;
    pthread_mutex_unlock(&g_test_ui_mtx);
}
void qc_ui_anfrage_zurueck(uint64_t anfrage) {
    pthread_mutex_lock(&g_test_ui_mtx);
    size_t n = strlen(g_test_ui_folge);
    snprintf(g_test_ui_folge + n, sizeof g_test_ui_folge - n, "z%llu ", (unsigned long long)anfrage);
    pthread_mutex_unlock(&g_test_ui_mtx);
}
void qc_ui_zustand_geaendert(void) { atomic_fetch_add(&g_test_zustand, 1); }
int qc_ui_vorhanden(void) { return atomic_load(&g_test_ui); }

// Wartet, bis eine andere Anfrage als alt gezeigt wird. 0 = keine.
static uint64_t anfrage_neu(uint64_t alt, double sekunden) {
    double t0 = sek();
    for (;;) {
        pthread_mutex_lock(&g_test_ui_mtx);
        uint64_t a = g_test_anfrage;
        pthread_mutex_unlock(&g_test_ui_mtx);
        if (a && a != alt) return a;
        if (sek() - t0 > sekunden) return 0;
        usleep(5 * 1000);
    }
}

static int ui_folge_hat(const char *was) {
    pthread_mutex_lock(&g_test_ui_mtx);
    int r = strstr(g_test_ui_folge, was) != NULL;
    pthread_mutex_unlock(&g_test_ui_mtx);
    return r;
}

static int ui_zurueck_abwarten(uint64_t a, double sekunden) {
    char z[32];
    snprintf(z, sizeof z, "z%llu ", (unsigned long long)a);
    double t0 = sek();
    while (!ui_folge_hat(z) && sek() - t0 < sekunden) usleep(5 * 1000);
    return ui_folge_hat(z);
}

static int zugang_ruht(void) { return qc_zugang_phasen_offen() == 0; }
static int jemand_anmeldend(void) { return atomic_load(&g_anmeldend) > 0; }
static int niemand_anmeldend(void) { return atomic_load(&g_anmeldend) == 0; }
static int zuschauer_da(void) { return atomic_load(&g_client_fd) >= 0; }
static int zuschauer_fort(void) { return atomic_load(&g_client_fd) < 0; }

// Ein Client wie der Rust-Client: Nachricht 3 traegt "QCN1", den Namen und
// die Flags (Bit 0: er kennt den Schluessel des Hosts nicht), danach liest er
// die Kennung und spricht in der Zugangsphase 21 und 23.
typedef struct { int fd; leser l; qc_cipher tx; uint8_t hh[QC_HASHLEN]; } zclient;

static int zc_verbinden_flags(zclient *z, int port, const uint8_t priv[32], const char *name, uint8_t flags) {
    memset(z, 0, sizeof *z);
    z->fd = -1;
    int c = verbinden(port);
    if (c < 0) return -1;
    qc_handshake hs;
    qc_handshake_init(&hs, 1, priv, (const uint8_t *)QC_PRO_VIDEO, strlen(QC_PRO_VIDEO));
    uint8_t msg[8192], pl[8192], n3[64];
    size_t ml = 0, pln = 0, n3l = name ? qc_zugang_name_kodieren(name, flags, n3, sizeof n3) : 0;
    if (qc_handshake_write(&hs, NULL, 0, msg, &ml) || rahmen_schreiben(c, msg, ml) ||
        rahmen_lesen(c, msg, sizeof msg, &ml) || qc_handshake_read(&hs, msg, ml, pl, &pln) ||
        qc_handshake_write(&hs, n3l ? n3 : NULL, n3l, msg, &ml) || rahmen_schreiben(c, msg, ml)) {
        close(c);
        return -1;
    }
    qc_cipher rx;
    qc_handshake_split(&hs, &z->tx, &rx);
    memcpy(z->hh, qc_handshake_hash(&hs), QC_HASHLEN);
    z->fd = c;
    leser_init(&z->l, c, 0);
    z->l.rx = rx;
    return 0;
}

static int zc_verbinden(zclient *z, int port, const uint8_t priv[32], const char *name) {
    return zc_verbinden_flags(z, port, priv, name, 0);
}

static void zc_zu(zclient *z) {
    if (z->fd >= 0) close(z->fd);
    z->fd = -1;
    free(z->l.buf);
    z->l.buf = NULL;
}

static int zc_kennung(zclient *z, const char *soll) {
    char k[4];
    return z->fd >= 0 && klartext(&z->l, k, 4, 2000) == 1 && memcmp(k, soll, 4) == 0;
}

static int zc_noetig(zclient *z, uint8_t *wege, uint32_t *warten, char name[QC_ZUGANG_NAME_MAX + 1]) {
    qc_hdr h;
    NSData *d = nil;
    if (nachricht_ganz(&z->l, &h, &d, 2000) != 1 || h.type != QC_ZUGANG_NOETIG) return -1;
    return qc_zugang_noetig_lesen(d.bytes, d.length, wege, warten, name);
}

static int zc_ergebnis(zclient *z, uint8_t *erg, uint32_t *warten, uint8_t proof[32], int frist_ms) {
    qc_hdr h;
    NSData *d = nil;
    if (nachricht_ganz(&z->l, &h, &d, frist_ms) != 1 || h.type != QC_ZUGANG_ERGEBNIS) return -1;
    return qc_zugang_ergebnis_lesen(d.bytes, d.length, erg, warten, proof);
}

static int zc_beweis(zclient *z, const char *pw) {
    uint8_t k[32], p[32];
    if (qc_zugang_schluessel(pw, g_id_pub, k)) return -1;       // host_pub aus dem Handschlag
    qc_zugang_beweis(k, z->hh, 0, p);
    return ein_senden(z->fd, &z->tx, QC_ZUGANG_BEWEIS, p, sizeof p);
}

static int zc_abbruch(zclient *z) { return ein_senden(z->fd, &z->tx, QC_ZUGANG_ABBRUCH, NULL, 0); }

// 1 = der Host schliesst (nach allem, was noch kam), 0 = nicht in der Frist.
// letzte/grund (duerfen NULL sein): Typ und erstes Nutzlastbyte der letzten
// Nachricht davor, -1 wenn keine kam.
static int zc_schliesst_mit(zclient *z, int frist_ms, int *letzte, int *grund) {
    qc_hdr h;
    NSData *d = nil;
    int r;
    if (letzte) *letzte = -1;
    if (grund) *grund = -1;
    while ((r = nachricht_ganz(&z->l, &h, &d, frist_ms)) == 1) {
        if (letzte) *letzte = h.type;
        if (grund) *grund = d.length ? ((const uint8_t *)d.bytes)[0] : -1;
    }
    return r == 0;
}

static int zc_schliesst(zclient *z, int frist_ms) { return zc_schliesst_mit(z, frist_ms, NULL, NULL); }

// Bis zur Strominfo: 1 = "QCH1" und INFO kamen.
static int zc_sitzung(zclient *z) {
    qc_hdr h;
    uint8_t anf[8];
    return zc_kennung(z, QC_MAGIC) && nachricht(&z->l, &h, anf, 2000) == 1 && h.type == QC_MSG_INFO;
}

static BOOL test_tcc_ja(void) { return YES; }
static BOOL test_tcc_nein(void) { return NO; }

// Die eine App (Plan M4): der eingestellte Geraetename gilt sofort in
// Nachricht 20; Freigabe aus verabschiedet den Zuschauer mit Grund 1, und
// wer danach noch hereinkommt, bekommt nach "QCH1" denselben Abschied.
static void name_und_freigabe_pruefen(int bild_port) {
    printf("\n-- Geraetename (qc_dienst_name_setzen)\n");
    char gn[QC_ZUGANG_NAME_MAX + 1] = {0}, rn[QC_ZUGANG_NAME_MAX + 1] = {0}, hn[QC_ZUGANG_NAME_MAX + 1] = {0};
    rechnername(rn);
    qc_dienst_name_setzen("  Büro-\x01Mac\t ");
    qc_zustand_geraetename(gn, sizeof gn);
    pruefe(!strcmp(gn, "Büro-Mac"), "eingestellter Name, gesaeubert wie jeder fremde (Rand, Steuerzeichen)");
    uint8_t n_priv[32], n_pub[32];
    qc_keypair(n_priv, n_pub);
    qc_zugang_drossel_leeren();
    zclient z;
    uint8_t wege = 0;
    uint32_t warten = 0;
    stdout_stumm(1);
    int ok = zc_verbinden(&z, bild_port, n_priv, "Namensprobe") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
             zc_noetig(&z, &wege, &warten, hn) == 0;
    zc_abbruch(&z);
    zc_schliesst(&z, 2000);
    stdout_stumm(0);
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);
    pruefe(ok && !strcmp(hn, "Büro-Mac"), "Nachricht 20 traegt ihn sofort (ohne Neustart)");
    qc_dienst_name_setzen(NULL);
    qc_zustand_geraetename(gn, sizeof gn);
    char rn2[QC_ZUGANG_NAME_MAX + 1] = {0};
    qc_zustand_rechnername(rn2, sizeof rn2);
    pruefe(!strcmp(gn, rn) && !strcmp(rn2, rn), "NULL: wieder der Rechnername");

    printf("\n-- Freigabe aus (qc_dienst_anhalten)\n");
    uint8_t f_priv[32], f_pub[32];
    qc_keypair(f_priv, f_pub);
    qc_zugang_eintragen(f_pub, "Freigabeprobe");
    strom_attrappe_setzen();
    stdout_stumm(1);
    int da = zc_verbinden(&z, bild_port, f_priv, "Freigabeprobe") == 0 && zc_sitzung(&z) && warten_bis(zuschauer_da, 1);
    dienst_lauschen_beenden();
    int letzte = -1, grund = -1;
    int zu = warten_bis(zuschauer_fort, 1) && zc_schliesst_mit(&z, 2000, &letzte, &grund);
    stdout_stumm(0);
    zc_zu(&z);
    pruefe(da && zu && letzte == QC_MSG_HOST_ENDE && grund == QC_HOST_ENDE_FREIGABE_AUS,
           "der Zuschauer bekommt als letzte Nachricht den Abschied (Typ 13, Grund 1)");
    pruefe(atomic_load(&g_freigabe_aus) && !atomic_load(&g_dienst_laeuft) && !g_annahme_bild,
           "Merker gesetzt, Dienst gilt als angehalten");
    strom_attrappe_setzen();
    stdout_stumm(1);
    int nach = zc_verbinden(&z, bild_port, f_priv, "Freigabeprobe") == 0 && zc_kennung(&z, QC_MAGIC);
    letzte = grund = -1;
    int zu2 = nach && zc_schliesst_mit(&z, 2000, &letzte, &grund);
    stdout_stumm(0);
    zc_zu(&z);
    pruefe(zu2 && letzte == QC_MSG_HOST_ENDE && grund == QC_HOST_ENDE_FREIGABE_AUS && zuschauer_fort() &&
               warten_bis(niemand_anmeldend, 1),
           "wer danach noch hereinkommt: \"QCH1\", dann derselbe Abschied - kein Zuschauer");
    atomic_store(&g_freigabe_aus, 0);
    qc_zugang_geraet_entfernen(f_pub);
    warten_bis(zugang_ruht, 2);
}

static void zugang_pruefen(int bild_port, int ein_port) {
    printf("\n-- Zugang: unbekanntes Geraet mit Passwort\n");
    static char logpfad[1100];
    snprintf(logpfad, sizeof logpfad, "%s/zugang.log", g_home);
    g_log_pfad = logpfad;
    pthread_mutex_lock(&g_log_mtx);
    log_oeffnen("w");
    pthread_mutex_unlock(&g_log_mtx);
    qc_zugang_drossel_leeren();
    const char *PW = "Hosttest-Passwort 1";
    stdout_stumm(1);
    int pw_ok = qc_zugang_passwort_setzen(PW) == 0;
    stdout_stumm(0);
    pruefe(pw_ok, "Zugangspasswort im eigenen HOME gesetzt");
    drossel_altern(&d_zugang);

    uint8_t a_priv[32], a_pub[32];
    qc_keypair(a_priv, a_pub);
    char a_id[12];
    qc_zugang_id_text(qc_zugang_id(a_pub), a_id);
    strom_attrappe_setzen();
    zclient z;
    stdout_stumm(1);
    int ok_k = zc_verbinden(&z, bild_port, a_priv, "Testgeraet A") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG);
    uint8_t wege = 0, erg = 9, hp[32], hp_soll[32], k[32];
    uint32_t warten = 99, w = 99;
    char hn[QC_ZUGANG_NAME_MAX + 1] = {0}, rn[QC_ZUGANG_NAME_MAX + 1];
    rechnername(rn);
    int ok_20 = ok_k && zc_noetig(&z, &wege, &warten, hn) == 0;
    int ok_22 = ok_20 && zc_beweis(&z, PW) == 0 && zc_ergebnis(&z, &erg, &w, hp, 3000) == 0;
    qc_zugang_schluessel(PW, g_id_pub, k);
    qc_zugang_beweis(k, z.hh, 1, hp_soll);
    int ok_s = ok_22 && zc_sitzung(&z);
    warten_bis(zuschauer_da, 1);
    char zn[64] = {0}, gname[QC_ZUGANG_NAME_MAX + 1] = {0};
    int verbunden = qc_zustand_zuschauer(zn, sizeof zn);
    int bek = qc_zugang_bekannt(a_pub, gname);
    stdout_stumm(0);
    char zeile[200];
    snprintf(zeile, sizeof zeile, "Zugang noetig: Testgeraet A (ID %s) von 127.0.0.1 - Passwort", a_id);
    printf("         (20: Wege %u, warten %u ms, Host \"%s\"; 22: Ergebnis %u)\n", wege, warten, hn, erg);
    pruefe(ok_k, "unbekannt: nach dem Handschlag \"QCA1\" statt \"QCH1\"");
    pruefe(ok_20 && wege == QC_ZUGANG_WEG_PASSWORT && warten == 0 && strcmp(hn, rn) == 0 && hn[0],
           "20: nur Passwort (keine Oberflaeche), keine Wartezeit, Rechnername des Hosts");
    pruefe(ok_22 && erg == QC_ERGEBNIS_PASSWORT && w == 0 && memcmp(hp, hp_soll, 32) == 0,
           "richtiges Passwort: 22/0 mit dem host_proof, den der Client nachrechnet");
    pruefe(ok_s, "danach \"QCH1\" und Strominfo wie bei einem bekannten Geraet");
    pruefe(bek == 1 && strcmp(gname, "Testgeraet A") == 0, "eingetragen mit dem Namen aus Nachricht 3 (QCN1)");
    pruefe(verbunden && strcmp(zn, "Testgeraet A") == 0, "qc_zustand_zuschauer: verbunden, mit Namen");
    pruefe(zeilen_mit(logpfad, zeile) == 1 && zeilen_mit(logpfad, "mit Passwort angenommen und eingetragen") == 1,
           "Zeilen: Zugang noetig (Name, ID, Adresse), angenommen und eingetragen");
    zuschauer_weg();
    zc_zu(&z);

    // Bekannt: sofort "QCH1".
    strom_attrappe_setzen();
    stdout_stumm(1);
    int ok_bek = zc_verbinden(&z, bild_port, a_priv, "Testgeraet A") == 0 && zc_sitzung(&z);
    warten_bis(zuschauer_da, 1);
    stdout_stumm(0);
    pruefe(ok_bek, "bekanntes Geraet: sofort \"QCH1\" und Strominfo, keine Zugangsphase");
    zuschauer_weg();
    zc_zu(&z);

    // Bekannt, aber der Client kennt diesen Host nicht (Bit 0 in Nachricht
    // 3, Spezifikation 1.4): trotzdem die Zugangsphase - erst der host_proof
    // weist den Host aus. Ein falsches Passwort nimmt den Eintrag nicht weg,
    // und er bleibt, wie er war (Name aus der ersten Aufnahme).
    printf("\n-- Zugang: bekanntes Geraet mit Bit 0 (der Host weist sich aus)\n");
    drossel_altern(&d_zugang);
    strom_attrappe_setzen();
    stdout_stumm(1);
    int ok_b0 = zc_verbinden_flags(&z, bild_port, a_priv, "Testgeraet A neu", QC_ZUGANG_N3_HOST_UNBEKANNT) == 0 &&
                zc_kennung(&z, QC_ZUGANG_KENNUNG) && zc_noetig(&z, &wege, &warten, hn) == 0;
    uint8_t e_b0f = 9, e_b0r = 9;
    uint32_t w_b0 = 9;
    int ok_b0f = ok_b0 && zc_beweis(&z, "falsch-falsch") == 0 && zc_ergebnis(&z, &e_b0f, &w_b0, NULL, 3000) == 0;
    int bek_b0f = qc_zugang_bekannt(a_pub, NULL);
    int ok_b0r = ok_b0f && zc_beweis(&z, PW) == 0 && zc_ergebnis(&z, &e_b0r, &w, hp, 3000) == 0;
    qc_zugang_beweis(k, z.hh, 1, hp_soll);
    int ok_b0s = ok_b0r && zc_sitzung(&z);
    warten_bis(zuschauer_da, 1);
    char gname_b0[QC_ZUGANG_NAME_MAX + 1] = {0};
    int bek_b0 = qc_zugang_bekannt(a_pub, gname_b0);
    stdout_stumm(0);
    pruefe(ok_b0 && wege == QC_ZUGANG_WEG_PASSWORT, "Bit 0: \"QCA1\" und 20, obwohl das Geraet in der Liste steht");
    pruefe(ok_b0f && e_b0f == QC_ERGEBNIS_FALSCH && w_b0 == 0 && bek_b0f == 1, "falsches Passwort: 22/2, der Eintrag bleibt");
    pruefe(ok_b0r && e_b0r == QC_ERGEBNIS_PASSWORT && memcmp(hp, hp_soll, 32) == 0,
           "richtiges Passwort: 22/0 mit dem host_proof, den der Client nachrechnet");
    pruefe(ok_b0s, "danach \"QCH1\" und Strominfo");
    pruefe(bek_b0 == 1 && strcmp(gname_b0, "Testgeraet A") == 0, "der Eintrag bleibt, wie er war");
    pruefe(zeilen_mit(logpfad, "kennt diesen Host aber nicht (Bit 0 in Nachricht 3)") == 1 &&
           zeilen_mit(logpfad, "mit Passwort angenommen und war schon eingetragen") == 1,
           "Zeilen: Zugang noetig mit Bit 0, angenommen, schon eingetragen");
    zuschauer_weg();
    zc_zu(&z);

    printf("\n-- Zugang: falsches Passwort, Drossel\n");
    drossel_altern(&d_zugang_falsch);
    uint8_t b_priv[32], b_pub[32];
    qc_keypair(b_priv, b_pub);
    stdout_stumm(1);
    int ok_b = zc_verbinden(&z, bild_port, b_priv, "Rater") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    uint8_t e[5] = {9, 9, 9, 9, 9};
    uint32_t ws[5] = {1, 1, 1, 1, 1};
    const char *versuche[5] = { "falsch eins", "falsch zwei", "falsch drei", PW /* zu frueh */, "falsch vier" };
    for (int i = 0; i < 5 && ok_b; i++)
        if (zc_beweis(&z, versuche[i]) != 0 || zc_ergebnis(&z, &e[i], &ws[i], NULL, 3000) != 0) ok_b = 0;
    int zu = ok_b && zc_schliesst(&z, 2000);
    int b_bek = qc_zugang_bekannt(b_pub, NULL);
    stdout_stumm(0);
    printf("         (Ergebnisse %u %u %u %u %u, warten %u %u %u %u %u ms)\n", e[0], e[1], e[2], e[3], e[4],
           ws[0], ws[1], ws[2], ws[3], ws[4]);
    pruefe(ok_b && e[0] == 2 && e[1] == 2 && e[2] == 2 && ws[0] == 0 && ws[1] == 0 && ws[2] == 5000,
           "falsch: 22/2, die ersten zwei ohne Wartezeit, der dritte 5 s");
    pruefe(e[3] == 2 && ws[3] == 10000, "das richtige Passwort vor Ablauf der Wartezeit zaehlt als Fehlversuch (10 s)");
    pruefe(e[4] == 4 && ws[4] == 20000 && zu, "der fuenfte Fehlversuch: 22/4 mit 20 s, dann schliesst der Host");
    pruefe(b_bek == 0, "nichts eingetragen");
    pruefe(zeilen_mit(logpfad, "Zugang: Passwort falsch fuer Rater (ID ") == 1 && drossel_weitere(&d_zugang_falsch) == 4,
           "Zeile \"Passwort falsch\" mit Name und ID, die weiteren gedrosselt gezaehlt");
    zc_zu(&z);
    stdout_stumm(1);
    int ok_b2 = zc_verbinden(&z, bild_port, b_priv, "Rater") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
                zc_noetig(&z, &wege, &warten, hn) == 0;
    zc_abbruch(&z);
    int zu2 = zc_schliesst(&z, 2000);
    stdout_stumm(0);
    printf("         (neue Verbindung desselben Geraets: warten %u ms)\n", warten);
    pruefe(ok_b2 && warten > 15000 && warten <= 20000 && zu2,
           "die Drossel gilt ueber die Verbindung hinaus: 20 meldet die Wartezeit, Abbruch (23) schliesst");
    zc_zu(&z);
    qc_zugang_drossel_leeren();

    printf("\n-- Zugang: Zulassen und Ablehnen (Test-Haken statt Oberflaeche)\n");
    atomic_store(&g_test_ui, 1);
    uint8_t c_priv[32], c_pub[32];
    qc_keypair(c_priv, c_pub);
    strom_attrappe_setzen();
    uint64_t vorher = anfrage_neu(0, 0);
    stdout_stumm(1);
    int ok_c = zc_verbinden(&z, bild_port, c_priv, "Tablet") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    uint64_t a = anfrage_neu(vorher, 2);
    pthread_mutex_lock(&g_test_ui_mtx);
    int anfrage_gut = strcmp(g_test_anfrage_name, "Tablet") == 0 && g_test_anfrage_id == qc_zugang_id(c_pub) &&
                      g_test_anfrage_code == qc_zugang_code(z.hh);
    pthread_mutex_unlock(&g_test_ui_mtx);
    qc_zugang_entscheiden(a, 1);
    uint8_t proof[32];
    int ok_c22 = ok_c && zc_ergebnis(&z, &erg, &w, proof, 3000) == 0 && erg == QC_ERGEBNIS_ZUGELASSEN && w == 0;
    int ok_cs = ok_c22 && zc_sitzung(&z);
    warten_bis(zuschauer_da, 1);
    stdout_stumm(0);
    pruefe(ok_c && wege == (QC_ZUGANG_WEG_PASSWORT | QC_ZUGANG_WEG_ZULASSEN), "20: Passwort oder Zulassen, wenn die Oberflaeche da ist");
    pruefe(a && anfrage_gut, "die Oberflaeche bekommt die Anfrage mit Name, ID und Vergleichscode");
    pruefe(ok_c22 && ok_cs && qc_zugang_bekannt(c_pub, NULL) == 1, "Zulassen: 22/1, \"QCH1\", eingetragen");
    pruefe(ui_zurueck_abwarten(a, 1) && zeilen_mit(logpfad, "am Host zugelassen und eingetragen") == 1,
           "das Fenster schliesst (qc_ui_anfrage_zurueck), Zeile");
    zuschauer_weg();
    zc_zu(&z);

    // Bit 0 von einem bekannten Geraet, und am Host wird "Zulassen" geklickt:
    // die Anfrage kommt wie bei einem neuen Geraet (mit Vergleichscode), dann
    // 22/1 und "QCH1"; der Eintrag bleibt.
    strom_attrappe_setzen();
    stdout_stumm(1);
    int ok_cb = zc_verbinden_flags(&z, bild_port, c_priv, "Tablet neu", QC_ZUGANG_N3_HOST_UNBEKANNT) == 0 &&
                zc_kennung(&z, QC_ZUGANG_KENNUNG) && zc_noetig(&z, &wege, &warten, hn) == 0;
    uint64_t a_cb = anfrage_neu(a, 2);
    pthread_mutex_lock(&g_test_ui_mtx);
    int anfrage_cb = strcmp(g_test_anfrage_name, "Tablet neu") == 0 && g_test_anfrage_code == qc_zugang_code(z.hh);
    pthread_mutex_unlock(&g_test_ui_mtx);
    qc_zugang_entscheiden(a_cb, 1);
    int ok_cb22 = ok_cb && zc_ergebnis(&z, &erg, &w, proof, 3000) == 0 && erg == QC_ERGEBNIS_ZUGELASSEN && zc_sitzung(&z);
    warten_bis(zuschauer_da, 1);
    char gname_cb[QC_ZUGANG_NAME_MAX + 1] = {0};
    int bek_cb = qc_zugang_bekannt(c_pub, gname_cb);
    stdout_stumm(0);
    pruefe(ok_cb && wege == (QC_ZUGANG_WEG_PASSWORT | QC_ZUGANG_WEG_ZULASSEN) && a_cb && anfrage_cb,
           "Bit 0, bekannt: Anfrage an die Oberflaeche mit Name und Vergleichscode");
    pruefe(ok_cb22 && bek_cb == 1 && strcmp(gname_cb, "Tablet") == 0 &&
           zeilen_mit(logpfad, "am Host zugelassen und war schon eingetragen") == 1,
           "Zulassen: 22/1, \"QCH1\", der Eintrag bleibt, Zeile");
    if (a_cb) a = a_cb;
    zuschauer_weg();
    zc_zu(&z);

    uint8_t d_priv[32], d_pub[32];
    qc_keypair(d_priv, d_pub);
    stdout_stumm(1);
    int ok_d = zc_verbinden(&z, bild_port, d_priv, "Fremd") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    a = anfrage_neu(a, 2);
    qc_zugang_entscheiden(a, 0);
    int ok_d22 = ok_d && zc_ergebnis(&z, &erg, &w, NULL, 3000) == 0 && erg == QC_ERGEBNIS_ABGELEHNT && zc_schliesst(&z, 2000);
    stdout_stumm(0);
    pruefe(ok_d22 && qc_zugang_bekannt(d_pub, NULL) == 0 && zeilen_mit(logpfad, "Zugang: Fremd (ID ") == 1 &&
           zeilen_mit(logpfad, "am Host abgelehnt") == 1, "Ablehnen: 22/3, der Host schliesst, nichts eingetragen, Zeile");
    zc_zu(&z);

    // Ein Name aus Nachricht 3, der eine ID vortaeuscht (neun oder mehr
    // Ziffern, auch Vollbreite), erscheint im Fenster als Adresse - wie in
    // der Windows-Host-Rolle. Acht Ziffern bleiben stehen.
    const char *namen[3] = { "Roberts Mac (ID 123 456 789)", "Mac \xef\xbc\x91\xef\xbc\x92\xef\xbc\x93 456 789", "Studio 2024 PC 1234" };
    const char *soll[3] = { "127.0.0.1", "127.0.0.1", "Studio 2024 PC 1234" };
    int namen_gut = 1;
    uint8_t v_priv[32], v_pub[32];
    qc_keypair(v_priv, v_pub);
    stdout_stumm(1);
    for (int i = 0; i < 3; i++) {
        int ok_v = zc_verbinden(&z, bild_port, v_priv, namen[i]) == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
                   zc_noetig(&z, &wege, &warten, hn) == 0;
        a = anfrage_neu(a, 2);
        pthread_mutex_lock(&g_test_ui_mtx);
        int gleich = strcmp(g_test_anfrage_name, soll[i]) == 0;
        pthread_mutex_unlock(&g_test_ui_mtx);
        zc_abbruch(&z);
        zc_schliesst(&z, 2000);
        zc_zu(&z);
        warten_bis(zugang_ruht, 2);
        if (!(ok_v && a && gleich)) namen_gut = 0;
    }
    stdout_stumm(0);
    pruefe(namen_gut, "Name mit vorgetaeuschter ID (9+ Ziffern, auch Vollbreite): im Zulassen-Fenster steht die Adresse; 8 Ziffern bleiben");

    printf("\n-- Zugang: Abbruch, Verbindungsende, Warteschlange\n");
    uint8_t e_priv[32], e_pub[32];
    qc_keypair(e_priv, e_pub);
    stdout_stumm(1);
    int ok_e = zc_verbinden(&z, bild_port, e_priv, "Abbrecher") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    a = anfrage_neu(a, 2);
    zc_abbruch(&z);
    int ok_e_zu = zc_schliesst(&z, 2000);
    stdout_stumm(0);
    pruefe(ok_e && a && ok_e_zu && ui_zurueck_abwarten(a, 1) && warten_bis(zugang_ruht, 1),
           "Abbruch (23): der Host schliesst, das Fenster schliesst, die Phase ist zu Ende");
    zc_zu(&z);

    stdout_stumm(1);
    int ok_f = zc_verbinden(&z, bild_port, e_priv, "Weggeher") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    a = anfrage_neu(a, 2);
    zc_zu(&z);                                          // einfach weg, ohne 23
    stdout_stumm(0);
    pruefe(ok_f && a && ui_zurueck_abwarten(a, 2) && warten_bis(zugang_ruht, 2),
           "Verbindungsende waehrend des Wartens: das Fenster schliesst, die Phase ist zu Ende");

    // Zwei Unbekannte zugleich: die Oberflaeche sieht eine, dann die naechste.
    uint8_t g1_priv[32], g1_pub[32], g2_priv[32], g2_pub[32];
    qc_keypair(g1_priv, g1_pub);
    qc_keypair(g2_priv, g2_pub);
    zclient z2;
    stdout_stumm(1);
    int ok_g = zc_verbinden(&z, bild_port, g1_priv, "Erster") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    uint64_t a1 = anfrage_neu(a, 2);
    ok_g = ok_g && zc_verbinden(&z2, bild_port, g2_priv, "Zweiter") == 0 && zc_kennung(&z2, QC_ZUGANG_KENNUNG) &&
           zc_noetig(&z2, &wege, &warten, hn) == 0;
    usleep(200 * 1000);
    pthread_mutex_lock(&g_test_ui_mtx);
    int nur_eine = g_test_anfrage == a1;
    pthread_mutex_unlock(&g_test_ui_mtx);
    zc_abbruch(&z);
    uint64_t a2 = anfrage_neu(a1, 2);
    pthread_mutex_lock(&g_test_ui_mtx);
    int zweiter_gezeigt = strcmp(g_test_anfrage_name, "Zweiter") == 0;
    pthread_mutex_unlock(&g_test_ui_mtx);
    char folge[64];
    snprintf(folge, sizeof folge, "z%llu a%llu ", (unsigned long long)a1, (unsigned long long)a2);
    int reihenfolge = ui_folge_hat(folge);
    qc_zugang_entscheiden(a2, 0);
    int ok_g2 = zc_ergebnis(&z2, &erg, &w, NULL, 3000) == 0 && erg == QC_ERGEBNIS_ABGELEHNT;
    stdout_stumm(0);
    pruefe(ok_g && nur_eine, "zwei Anfragen zugleich: die Oberflaeche zeigt nur die erste");
    pruefe(a2 && zweiter_gezeigt && reihenfolge && ok_g2, "zieht sich die erste zurueck: erst zurueck, dann die zweite");
    zc_zu(&z);
    zc_zu(&z2);
    warten_bis(zugang_ruht, 2);

    printf("\n-- Zugang: Grenzen, Frist\n");
    atomic_store(&g_test_ui, 0);
    uint8_t h1[32], h1p[32], h2[32], h2p[32], h3[32], h3p[32];
    qc_keypair(h1, h1p);
    qc_keypair(h2, h2p);
    qc_keypair(h3, h3p);
    zclient y1, y2, y3;
    stdout_stumm(1);
    int ok_h = zc_verbinden(&y1, bild_port, h1, "H1") == 0 && zc_kennung(&y1, QC_ZUGANG_KENNUNG) && zc_noetig(&y1, &wege, &warten, hn) == 0 &&
               zc_verbinden(&y2, bild_port, h2, "H2") == 0 && zc_kennung(&y2, QC_ZUGANG_KENNUNG) && zc_noetig(&y2, &wege, &warten, hn) == 0;
    int ok_h3 = zc_verbinden(&y3, bild_port, h3, "H3") == 0 && zc_kennung(&y3, QC_ZUGANG_KENNUNG) &&
                zc_ergebnis(&y3, &erg, &w, NULL, 2000) == 0 && erg == QC_ERGEBNIS_SCHLUSS && w == QC_ZUGANG_VOLL_WARTEN_MS &&
                zc_schliesst(&y3, 2000);
    zc_zu(&y3);
    zc_abbruch(&y2);
    zc_schliesst(&y2, 2000);
    for (int i = 0; i < 200 && qc_zugang_phasen_offen() > 1; i++) usleep(5 * 1000);
    // Derselbe Schluessel ein zweites Mal, waehrend seine Phase laeuft.
    int ok_h1b = zc_verbinden(&y3, bild_port, h1, "H1 nochmal") == 0 && zc_kennung(&y3, QC_ZUGANG_KENNUNG) &&
                 zc_ergebnis(&y3, &erg, &w, NULL, 2000) == 0 && erg == QC_ERGEBNIS_SCHLUSS && zc_schliesst(&y3, 2000);
    stdout_stumm(0);
    pruefe(ok_h && ok_h3, "dritte Zugangsphase derselben Adresse: \"QCA1\", sofort 22/4 mit 5 s, geschlossen");
    pruefe(ok_h1b, "zweite Zugangsphase desselben Schluessels: 22/4, geschlossen");
    zc_zu(&y1); zc_zu(&y2); zc_zu(&y3);
    warten_bis(zugang_ruht, 2);

    g_zugang_frist_ms = 1200;
    drossel_altern(&d_zugang_frist);
    stdout_stumm(1);
    double t0 = sek();
    int ok_i = zc_verbinden(&z, bild_port, h2, "Schlaefer") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0 && zc_ergebnis(&z, &erg, &w, NULL, 4000) == 0 &&
               erg == QC_ERGEBNIS_SCHLUSS && zc_schliesst(&z, 2000);
    double dauer = sek() - t0;
    stdout_stumm(0);
    g_zugang_frist_ms = QC_ZUGANG_FRIST_MS;
    printf("         (Frist 1,2 s: geschlossen nach %.2f s)\n", dauer);
    pruefe(ok_i && dauer > 1.0 && dauer < 3.0 && zeilen_mit(logpfad, "Zugang: Frist fuer Schlaefer (ID ") == 1,
           "Gesamtfrist abgelaufen: 22/4, geschlossen, Zeile");
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);

    // Der Kopf kommt tropfenweise (0,7 s), vom Beweis nur ein Stueck: die
    // Frist gilt fuer Kopf und Beweis zusammen, nie ueber die Gesamtfrist
    // hinaus - und am Ende steht 22/4, kein stilles Schliessen.
    g_zugang_frist_ms = 1200;
    drossel_altern(&d_zugang_frist);
    uint8_t hb[32], hbp[32];
    qc_keypair(hb, hbp);
    stdout_stumm(1);
    int ok_hb = zc_verbinden(&z, bild_port, hb, "Halber") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
                zc_noetig(&z, &wege, &warten, hn) == 0;
    double t_hb = sek();
    if (ok_hb) {
        qc_hdr kopf = { .type = QC_ZUGANG_BEWEIS, .flags = 0, .reserved = 0, .len = 32 };
        uint8_t ct[64], l[2];
        size_t cl = 0;
        ok_hb = qc_encrypt(&z.tx, (const uint8_t *)&kopf, sizeof kopf, ct, &cl) == 0 && cl > 5;
        l[0] = (uint8_t)cl;
        l[1] = 0;
        ok_hb = ok_hb && send(z.fd, l, 2, 0) == 2 && send(z.fd, ct, 5, 0) == 5;
        usleep(700 * 1000);
        ok_hb = ok_hb && send(z.fd, ct + 5, cl - 5, 0) == (ssize_t)(cl - 5);
        uint8_t halb[7] = { 48, 0, 1, 2, 3, 4, 5 };     // Laenge des Beweis-Datensatzes, dann fuenf von 48 Byte
        ok_hb = ok_hb && send(z.fd, halb, sizeof halb, 0) == (ssize_t)sizeof halb;
    }
    int ok_hb22 = ok_hb && zc_ergebnis(&z, &erg, &w, NULL, 4000) == 0 && erg == QC_ERGEBNIS_SCHLUSS && zc_schliesst(&z, 2000);
    double d_hb = sek() - t_hb;
    stdout_stumm(0);
    g_zugang_frist_ms = QC_ZUGANG_FRIST_MS;
    printf("         (Frist 1,2 s, Kopf nach 0,7 s, Beweis halb: Ende nach %.2f s)\n", d_hb);
    pruefe(ok_hb22 && d_hb > 0.9 && d_hb < 1.6 && zeilen_mit(logpfad, "Zugang: Frist fuer Halber (ID ") == 1,
           "halbe Nachricht: eine Frist fuer Kopf und Beweis, nie ueber die Gesamtfrist; dann 22/4 und Zeile");
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);

    printf("\n-- Zugang: zwei Verbindungen derselben Adresse, UTF-8-Namen\n");
    // Zwei Phasen von einer Adresse mit verschiedenen Schluesseln (erlaubt:
    // 2 je Adresse). Die erste raet dreimal falsch - ab da wartet die Adresse
    // 5 s. Die zweite bekam in 20 keine Wartezeit, darf jetzt aber auch nicht
    // raten: ihr Beweis (sogar der richtige) zaehlt als Fehlversuch.
    qc_zugang_drossel_leeren();
    drossel_altern(&d_zugang);
    drossel_altern(&d_zugang_falsch);
    uint8_t p1[32], p1p[32], p2[32], p2p[32];
    qc_keypair(p1, p1p);
    qc_keypair(p2, p2p);
    // "Juergens Mac - Buero" mit echten Umlauten und Gedankenstrich (UTF-8).
    const char *jn = "J\xc3\xbcrgens Mac \xe2\x80\x93 B\xc3\xbcro";
    uint32_t w_p2 = 99;
    stdout_stumm(1);
    int ok_p = zc_verbinden(&y1, bild_port, p1, jn) == 0 && zc_kennung(&y1, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&y1, &wege, &warten, hn) == 0 &&
               zc_verbinden(&y2, bild_port, p2, "Nachbar") == 0 && zc_kennung(&y2, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&y2, &wege, &w_p2, hn) == 0;
    uint8_t pe[3] = {9, 9, 9};
    uint32_t pws[3] = {1, 1, 1};
    for (int i = 0; i < 3 && ok_p; i++)
        if (zc_beweis(&y1, "falsch geraten") != 0 || zc_ergebnis(&y1, &pe[i], &pws[i], NULL, 3000) != 0) ok_p = 0;
    drossel_altern(&d_zugang_falsch);
    uint8_t e2 = 9;
    uint32_t wp2 = 0;
    int ok_p2 = ok_p && zc_beweis(&y2, PW) == 0 && zc_ergebnis(&y2, &e2, &wp2, NULL, 3000) == 0;
    int p2_bek = qc_zugang_bekannt(p2p, NULL);
    zc_abbruch(&y1);
    zc_abbruch(&y2);
    zc_schliesst(&y1, 2000);
    zc_schliesst(&y2, 2000);
    stdout_stumm(0);
    char jz[200];
    snprintf(jz, sizeof jz, "Zugang noetig: %s (ID ", jn);
    printf("         (erste: %u %u %u / %u %u %u ms; zweite: warten in 20 %u ms, dann Ergebnis %u, %u ms)\n",
           pe[0], pe[1], pe[2], pws[0], pws[1], pws[2], w_p2, e2, wp2);
    pruefe(ok_p && w_p2 == 0 && pe[2] == QC_ERGEBNIS_FALSCH && pws[2] == 5000,
           "erste Verbindung: der dritte Fehlversuch bringt der Adresse 5 s");
    pruefe(ok_p2 && e2 == QC_ERGEBNIS_FALSCH && wp2 == 10000 && p2_bek == 0,
           "zweite Verbindung derselben Adresse: ihr Beweis kommt vor Ablauf der Wartezeit der Adresse - "
           "Fehlversuch (10 s), nicht geprueft, nicht eingetragen");
    pruefe(zeilen_mit(logpfad, "Zugang: Beweis vor Ablauf der Wartezeit fuer Nachbar (ID ") == 1, "Zeile dazu");
    pruefe(zeilen_mit(logpfad, jz) == 1, "Namen in UTF-8 stehen unverfaelscht im Protokoll (Umlaute, Gedankenstrich)");
    zc_zu(&y1);
    zc_zu(&y2);
    warten_bis(zugang_ruht, 2);
    qc_zugang_drossel_leeren();

    printf("\n-- Zugang: eigener Schluessel (Selbstschutz)\n");
    // Dieser Rechner selbst: auch als eingetragenes Geraet nie herein -
    // "QCA1" und gleich 22/3, keine Anfrage, kein Zuschauer.
    qc_zugang_eintragen(g_id_pub, "Ich");
    stdout_stumm(1);
    uint8_t se = 9;
    uint32_t sw = 1;
    int ok_selbst = zc_verbinden(&z, bild_port, g_id_priv, "Ich") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
                    zc_ergebnis(&z, &se, &sw, NULL, 2000) == 0 && zc_schliesst(&z, 2000);
    stdout_stumm(0);
    pruefe(ok_selbst && se == QC_ERGEBNIS_ABGELEHNT && atomic_load(&g_client_fd) < 0 &&
           zeilen_mit(logpfad, "meldet sich mit dem eigenen Schluessel dieses Hosts") == 1,
           "eigener Schluessel: QCA1 und gleich 22/3, kein Zuschauer, Zeile im Protokoll");
    qc_zugang_geraet_entfernen(g_id_pub);
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);

    printf("\n-- Zugang: Passwortdatei unlesbar\n");
    // Dann gibt es nur Zulassen (4.3). Ein Beweis ist kein Fehler des
    // Clients: 22/2 ohne Wartezeit, die Drossel bleibt, wie sie ist.
    char pwpfad[1200];
    qc_config_path("host-password.txt", pwpfad, sizeof pwpfad);
    [@"kurz\n" writeToFile:@(pwpfad) atomically:NO encoding:NSUTF8StringEncoding error:nil];
    drossel_altern(&d_zugang);
    drossel_altern(&d_zugang_falsch);
    uint8_t u_priv[32], u_pub[32];
    qc_keypair(u_priv, u_pub);
    stdout_stumm(1);
    int ok_u = zc_verbinden(&z, bild_port, u_priv, "Ohne Passwort") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    uint8_t ue[3] = {9, 9, 9};
    uint32_t uw[3] = {1, 1, 1};
    for (int i = 0; i < 3 && ok_u; i++)
        if (zc_beweis(&z, PW) != 0 || zc_ergebnis(&z, &ue[i], &uw[i], NULL, 3000) != 0) ok_u = 0;
    uint32_t u_drossel = qc_zugang_drossel_warten(inet_addr("127.0.0.1"), u_pub, mono_ms());
    zc_abbruch(&z);
    zc_schliesst(&z, 2000);
    int u_bek = qc_zugang_bekannt(u_pub, NULL);
    int pw_wieder = qc_zugang_passwort_setzen(PW) == 0;
    stdout_stumm(0);
    printf("         (Ergebnisse %u %u %u, warten %u %u %u ms, Drossel danach %u ms)\n", ue[0], ue[1], ue[2],
           uw[0], uw[1], uw[2], u_drossel);
    pruefe(ok_u && ue[0] == 2 && ue[1] == 2 && ue[2] == 2 && !uw[0] && !uw[1] && !uw[2] && u_drossel == 0 && u_bek == 0,
           "unlesbares Passwort: 22/2 ohne Wartezeit, die Drossel waechst nicht (auch nicht beim dritten)");
    pruefe(zeilen_mit(logpfad, "Zugang: Beweis von Ohne Passwort (ID ") == 1, "Zeile: nicht pruefbar, nur Zulassen");
    pruefe(pw_wieder, "Passwort wieder gesetzt");
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);

    printf("\n-- Zugang: ein Wartender stoert den laufenden Zuschauer nicht\n");
    atomic_store(&g_test_ui, 1);
    int hfd, cfd;
    if (paar(&hfd, &cfd, 0)) { pruefe(0, "Verbindung"); return; }
    zuschauer_setzen(hfd, kanal(hfd, 0x61));
    uint8_t j_priv[32], j_pub[32];
    qc_keypair(j_priv, j_pub);
    stdout_stumm(1);
    int ok_j = zc_verbinden(&z, bild_port, j_priv, "Wartender") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG) &&
               zc_noetig(&z, &wege, &warten, hn) == 0;
    a = anfrage_neu(a2, 2);
    qc_cipher etx;
    int ein = eingabe_verbinden(ein_port, j_priv, z.hh, &etx);
    usleep(100 * 1000);
    int unberuehrt = atomic_load(&g_client_fd) == hfd && atomic_load(&g_vid_ready) && atomic_load(&g_in_fd) < 0;
    struct pollfd pf = { .fd = cfd, .events = POLLIN, .revents = 0 };
    int still = poll(&pf, 1, 100) == 0;
    zc_abbruch(&z);
    zc_schliesst(&z, 2000);
    stdout_stumm(0);
    pruefe(ok_j && a && unberuehrt && still, "waehrend der Zugangsphase: der Zuschauer bleibt eingetragen und bekommt nichts");
    pruefe(ein < 0, "der Wartende bekommt keinen Eingabekanal (sein Handschlag passt nicht zum Bildkanal)");
    if (ein >= 0) close(ein);
    zc_zu(&z);
    zuschauer_weg();
    close(cfd);
    atomic_store(&g_test_ui, 0);
    warten_bis(zugang_ruht, 2);

    printf("\n-- Zugang: beschaedigte Liste, Entfernen\n");
    char listenpfad[1200];
    qc_config_path("host-devices.txt", listenpfad, sizeof listenpfad);
    NSData *heil = [NSData dataWithContentsOfFile:@(listenpfad)];
    [@"kaputt\n" writeToFile:@(listenpfad) atomically:NO encoding:NSUTF8StringEncoding error:nil];
    stdout_stumm(1);
    int ok_l = zc_verbinden(&z, bild_port, a_priv, "Testgeraet A") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG);
    zc_abbruch(&z);
    zc_schliesst(&z, 2000);
    stdout_stumm(0);
    NSData *danach = [NSData dataWithContentsOfFile:@(listenpfad)];
    pruefe(ok_l && [danach isEqualToData:[@"kaputt\n" dataUsingEncoding:NSUTF8StringEncoding]],
           "Liste beschaedigt: auch ein bekanntes Geraet muss sich ausweisen, die Datei bleibt unangetastet");
    zc_zu(&z);
    [heil writeToFile:@(listenpfad) atomically:NO];
    warten_bis(zugang_ruht, 2);

    strom_attrappe_setzen();
    stdout_stumm(1);
    int ok_m = zc_verbinden(&z, bild_port, a_priv, "Testgeraet A") == 0 && zc_sitzung(&z) && warten_bis(zuschauer_da, 1);
    int zust = atomic_load(&g_test_zustand);
    int entfernt = qc_zugang_geraet_entfernen(a_pub) == 0;
    int letzte_m = -1, grund_m = -1;
    int getrennt = warten_bis(zuschauer_fort, 1) && zc_schliesst_mit(&z, 2000, &letzte_m, &grund_m);
    stdout_stumm(0);
    pruefe(ok_m && entfernt && getrennt && qc_zugang_bekannt(a_pub, NULL) == 0 && atomic_load(&g_test_zustand) > zust,
           "Geraet entfernen: seine laufende Sitzung endet, die Oberflaeche erfaehrt es");
    pruefe(letzte_m == QC_MSG_HOST_ENDE && grund_m == QC_HOST_ENDE_ENTFERNT,
           "Geraet entfernen: der Zuschauer bekommt als letzte Nachricht den Abschied (Typ 13, Grund 2)");
    zc_zu(&z);
    stdout_stumm(1);
    int ok_m2 = zc_verbinden(&z, bild_port, a_priv, "Testgeraet A") == 0 && zc_kennung(&z, QC_ZUGANG_KENNUNG);
    zc_abbruch(&z);
    zc_schliesst(&z, 2000);
    stdout_stumm(0);
    pruefe(ok_m2, "danach braucht es wieder die Zugangsphase");
    zc_zu(&z);
    warten_bis(zugang_ruht, 2);

    // Entfernen, waehrend ein bekanntes Geraet hereinkommt: es steht schon in
    // der Liste, faehrt aber noch die Aufnahme hoch (hier: die angehaltene
    // Lebenslauf-Warteschlange) und ist noch kein Zuschauer.
    uint8_t r_priv[32], r_pub[32], x_priv[32], x_pub[32];
    qc_keypair(r_priv, r_pub);
    qc_keypair(x_priv, x_pub);
    qc_zugang_eintragen(r_pub, "Wettlauf");
    qc_zugang_eintragen(x_pub, "Unbeteiligt");
    strom_attrappe_setzen();
    dispatch_semaphore_t halt = dispatch_semaphore_create(0);
    dispatch_async(g_lifeq, ^{ dispatch_semaphore_wait(halt, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)); });
    stdout_stumm(1);
    int ok_r1 = zc_verbinden(&z, bild_port, r_priv, "Wettlauf") == 0 && warten_bis(jemand_anmeldend, 2);
    int x_weg = qc_zugang_geraet_entfernen(x_pub) == 0;
    dispatch_semaphore_signal(halt);
    ok_r1 = ok_r1 && zc_sitzung(&z) && warten_bis(zuschauer_da, 1);
    stdout_stumm(0);
    pruefe(ok_r1 && x_weg, "ein anderes Geraet wird entfernt, waehrend eins hereinkommt: das kommt trotzdem herein");
    zuschauer_weg();
    zc_zu(&z);

    strom_attrappe_setzen();
    halt = dispatch_semaphore_create(0);
    dispatch_async(g_lifeq, ^{ dispatch_semaphore_wait(halt, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)); });
    stdout_stumm(1);
    int ok_r2 = zc_verbinden(&z, bild_port, r_priv, "Wettlauf") == 0 && warten_bis(jemand_anmeldend, 2);
    int r_weg = qc_zugang_geraet_entfernen(r_pub) == 0;
    dispatch_semaphore_signal(halt);
    int kein_qch1 = !zc_kennung(&z, QC_MAGIC);
    int nicht_drin = warten_bis(niemand_anmeldend, 2) && atomic_load(&g_client_fd) < 0;
    stdout_stumm(0);
    pruefe(ok_r2 && r_weg && kein_qch1 && nicht_drin &&
           zeilen_mit(logpfad, "Zuschauer Wettlauf (ID ") == 1 &&
           zeilen_mit(logpfad, "abgewiesen: sein Geraet wurde eben aus der Liste entfernt") == 1,
           "genau dieses Geraet wird entfernt, waehrend es hereinkommt: kein \"QCH1\", kein Zuschauer, Zeile");
    zc_zu(&z);
    strom_jetzt();                                      // der Abbau ist durch

    printf("\n-- Argumente ohne Wert (7.7), entfernte Schalter\n");
    stdout_stumm(1);
    NSString *w1 = wert_nach(@[ @"host", @"--fps" ], @"--fps", 1);
    NSString *w2 = wert_nach(@[ @"host", @"--fps", @"--mbit", @"80" ], @"--fps", 1);
    NSString *w3 = wert_nach(@[ @"host", @"--fps", @"--mbit", @"80" ], @"--mbit", 1);
    NSString *w4 = wert_nach(@[ @"host", @"--capture", @"5" ], @"--capture", 2);
    NSString *w5 = wert_nach(@[ @"host", @"--capture", @"5", @"x.hevc" ], @"--capture", 2);
    argumente_pruefen(@[ @"host", @"--pair", @"--forget", @"--fps", @"60", @"--gibtsnicht", @"-psn_0_1" ]);
    stdout_stumm(0);
    pruefe(!w1 && !w2 && [w3 isEqualToString:@"80"] && !w4 && [w5 isEqualToString:@"x.hevc"] &&
           zeilen_mit(logpfad, "--fps: Wert 1 fehlt - der Standard gilt") == 2,
           "--fps am Ende oder vor dem naechsten Schalter: kein Absturz, Standard und Zeile");
    pruefe(zeilen_mit(logpfad, "Unbekanntes Argument --pair - uebergangen (entfallen") == 1 &&
           zeilen_mit(logpfad, "Unbekanntes Argument --forget") == 1 && zeilen_mit(logpfad, "Unbekanntes Argument --gibtsnicht") == 1 &&
           zeilen_mit(logpfad, "Unbekanntes Argument") == 3, "--pair und --forget werden nur noch als unbekannt protokolliert");

    printf("\n-- Zugang: ohne Bildschirmfreigabe\n");
    // Ohne Freigabe (und ohne Bildschirmliste) kommt ein Zuschauer trotzdem
    // herein und bekommt Hoststatus 1; die Wiederherstellung fragt nach.
    uint8_t n_priv[32], n_pub[32];
    qc_keypair(n_priv, n_pub);
    qc_zugang_eintragen(n_pub, "ohne Freigabe");
    stream_setzen(nil);
    g_tcc_bildschirm = test_tcc_nein;
    int tcc = qc_zustand_bildschirmfreigabe();
    stdout_stumm(1);
    double t_ein = sek();
    int ok_n = zc_verbinden(&z, bild_port, n_priv, "ohne Freigabe") == 0 && zc_sitzung(&z);
    int status = -1;
    qc_hdr h;
    NSData *d = nil;
    while (ok_n && status < 0 && nachricht_ganz(&z.l, &h, &d, 2000) == 1)
        if (h.type == QC_MSG_HOSTSTATUS && d.length >= 1) status = ((const uint8_t *)d.bytes)[0];
    int drin = atomic_load(&g_client_fd) >= 0 && atomic_load(&g_ohne_aufnahme);
    zuschauer_weg();
    zc_zu(&z);
    // Die eingereihte Wiederherstellung (3 s) muss ohne Zuschauer ins Leere laufen.
    while (sek() - t_ein < 3.5) usleep(50 * 1000);
    dispatch_sync(g_lifeq, ^{ g_kein_bildschirm_gemeldet = 0; });
    stdout_stumm(0);
    g_tcc_bildschirm = test_tcc_ja;
    atomic_store(&g_ohne_aufnahme, 0);
    pruefe(tcc == 0 && ok_n && status == 1 && drin,
           "keine Freigabe: der Zuschauer kommt herein (\"QCH1\", Strominfo) und bekommt Hoststatus 1 statt einer Abweisung");
    pruefe(zeilen_mit(logpfad, "Bildschirmaufnahme nicht freigegeben - Zuschauer bekommt Hoststatus 1") == 1, "Zeile");

    printf("\n-- Zugang: Freigabe da, Aufnahme startet nicht\n");
    // Freigabe erteilt, aber kein Bildschirm (Monitor aus, KVM umgeschaltet;
    // hier die leere Liste der Attrappe): der Zuschauer kommt ebenso herein
    // und bekommt Hoststatus 1. Still zuzumachen hiesse fuer den Client
    // "aeltere Fassung, bitte aktualisieren" - dauerhaft und falsch.
    stream_setzen(nil);
    g_tcc_bildschirm = test_tcc_ja;
    stdout_stumm(1);
    t_ein = sek();
    int ok_ob = zc_verbinden(&z, bild_port, n_priv, "ohne Freigabe") == 0 && zc_sitzung(&z);
    status = -1;
    while (ok_ob && status < 0 && nachricht_ganz(&z.l, &h, &d, 2000) == 1)
        if (h.type == QC_MSG_HOSTSTATUS && d.length >= 1) status = ((const uint8_t *)d.bytes)[0];
    int drin_b = atomic_load(&g_client_fd) >= 0 && atomic_load(&g_ohne_aufnahme);
    zuschauer_weg();
    zc_zu(&z);
    while (sek() - t_ein < 3.5) usleep(50 * 1000);
    dispatch_sync(g_lifeq, ^{ g_kein_bildschirm_gemeldet = 0; });
    stdout_stumm(0);
    atomic_store(&g_ohne_aufnahme, 0);
    pruefe(ok_ob && status == 1 && drin_b,
           "kein Bildschirm trotz Freigabe: der Zuschauer kommt herein (\"QCH1\", Strominfo) und bekommt Hoststatus 1");
    pruefe(zeilen_mit(logpfad, "Aufnahme laesst sich nicht starten - Zuschauer 127.0.0.1 bekommt Hoststatus 1") == 1 &&
           zeilen_mit(logpfad, "Zuschauer 127.0.0.1 abgewiesen") == 0, "Zeile, keine Abweisung");

    pthread_mutex_lock(&g_log_mtx);
    fclose(g_log);
    g_log = NULL;
    pthread_mutex_unlock(&g_log_mtx);
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
        // Zugang wie im Host: Zeilen ins Protokoll, Entfernen trennt, Passwort
        // im eigenen HOME. Die Freigabe fuer die Bildschirmaufnahme gilt als
        // erteilt - nur der Abschnitt Zugang nimmt sie weg.
        qc_zugang_protokoll_setzen(zugang_zeile);
        qc_zugang_entfernt_setzen(zuschauer_entfernt);
        qc_zugang_start(g_id_pub);
        g_tcc_bildschirm = test_tcc_ja;
        g_capq = dispatch_queue_create("hosttest.aufnahme", DISPATCH_QUEUE_SERIAL);
        g_lifeq = dispatch_queue_create("hosttest.lebenslauf", DISPATCH_QUEUE_SERIAL);
        int bild_port = annahme_lauschen(19400, bild_verbindung, "Bildkanal");
        int ein_port = annahme_lauschen(19450, eingabe_verbindung, "Eingabekanal");
        printf("Annahme: Bildport %d, Eingabeport %d\n", bild_port, ein_port);
        if (bild_port < 0 || ein_port < 0) return 1;
        g_stats.nal_len = 4;
        atomic_store(&g_cur_fps, 120);
        g_attrappen = [NSMutableArray array];
        // Nie ScreenCaptureKit: die Bildschirmliste kommt aus der Attrappe
        // (leer, bis der Abschnitt Bildschirm sie fuellt), der Strom aus der Fabrik.
        qc_bildschirm_liste_setzen(test_liste);
        qc_strom_fabrik_setzen(test_fabrik);
        protokoll_pruefen(bild_port, ein_port);
        zugang_pruefen(bild_port, ein_port);
        name_und_freigabe_pruefen(bild_port);
        abloesen_pruefen();
        wechsel_pruefen(bild_port);
        testbild_rest_pruefen();
        abbau_wettlauf_pruefen(bild_port);
        nachreichen_pruefen(bild_port);
        codec_abschluss_pruefen();
        formatwechsel_pruefen(bild_port);
        fest_pruefen();
        dateien_pruefen(bild_port, ein_port);
        bildschirm_pruefen(bild_port, ein_port);
        ton_pruefen();
        stau_pruefen();
        takt_pruefen();
        codecs_pruefen_pruefen();
        hdr_encoder_pruefen();
        // Das eigene HOME bleibt nur liegen, wenn etwas fehlschlug (zum Nachsehen).
        if (!g_fehler) [[NSFileManager defaultManager] removeItemAtPath:@(g_home) error:nil];
        printf("\n%s: %d Fehler\n", g_fehler ? "NICHT BESTANDEN" : "bestanden", g_fehler);
        return g_fehler;
    }
}
