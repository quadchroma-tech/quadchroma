#include "qc_annahme.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#define QC_ANNAHME_PLAETZE 32       // obere Grenze fuer max_gesamt

struct qc_annahme {
    int listen_fd;
    // Wecker fuers Anhalten: halt setzt qc_annahme_stoppen, dann ein Byte in
    // weck[1] (das schliesst er selbst); der Annahmefaden wartet in poll auf
    // den Port UND auf weck[0] und schliesst danach listen_fd und weck[0].
    int weck[2];
    _Atomic int halt;
    qc_annahme_cfg cfg;
    pthread_mutex_t mtx;            // schuetzt platz[], nr und idx/verdraengt der Plaetze
    qc_platz *platz[QC_ANNAHME_PLAETZE];    // NULL = frei
    uint64_t nr;                    // fortlaufend: kleiner heisst aelter
    _Atomic int faeden;             // laufende Verbindungsfaeden, auch verdraengte
    // Nur im Annahmefaden:
    long verdraengt, abgewiesen;    // seit der letzten Meldung
    struct sockaddr_in verdraengt_von, abgewiesen_von;
    int64_t gemeldet_s;
};

struct qc_platz {
    qc_annahme *a;
    int idx;                        // unter a->mtx: Platz in der Tabelle, -1 = keiner mehr
    int verdraengt;                 // unter a->mtx
    uint64_t nr;
    int fd;
    struct sockaddr_in peer;
};

static int64_t jetzt_s(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec;
}

int qc_platz_frei(qc_platz *p) {
    if (!p) return 0;
    pthread_mutex_lock(&p->a->mtx);
    if (p->idx >= 0) {
        p->a->platz[p->idx] = NULL;
        p->idx = -1;
    }
    int v = p->verdraengt;
    pthread_mutex_unlock(&p->a->mtx);
    return v;
}

static void *verbindung_faden(void *arg) {
    qc_platz *p = arg;
    qc_annahme *a = p->a;
    a->cfg.verbindung(p, p->fd, &p->peer, a->cfg.ctx);
    qc_platz_frei(p);
    free(p);
    atomic_fetch_sub(&a->faeden, 1);
    return NULL;
}

// Platz fuer eine neue Verbindung. Hat ihre Adresse schon max_je_ip Plaetze,
// weicht deren aeltester Handschlag; ist sonst keiner frei, der aelteste
// ueberhaupt. Der Verdraengte bekommt shutdown - unter der Sperre, und nur
// solange er seinen Platz noch haelt: danach darf sein Faden den fd schliessen.
// 1 = es wurde jemand verdraengt.
static int platz_nehmen(qc_annahme *a, qc_platz *neu) {
    in_addr_t ip = neu->peer.sin_addr.s_addr;
    int frei = -1, gleich = 0, alt = -1, alt_ip = -1;
    pthread_mutex_lock(&a->mtx);
    for (int i = 0; i < a->cfg.max_gesamt; i++) {
        qc_platz *q = a->platz[i];
        if (!q) { if (frei < 0) frei = i; continue; }
        if (alt < 0 || q->nr < a->platz[alt]->nr) alt = i;
        if (q->peer.sin_addr.s_addr == ip) {
            gleich++;
            if (alt_ip < 0 || q->nr < a->platz[alt_ip]->nr) alt_ip = i;
        }
    }
    int opfer = gleich >= a->cfg.max_je_ip ? alt_ip : frei < 0 ? alt : -1;
    if (opfer >= 0) {
        qc_platz *q = a->platz[opfer];
        shutdown(q->fd, SHUT_RDWR);
        q->idx = -1;
        q->verdraengt = 1;
        a->verdraengt_von = q->peer;
        frei = opfer;
    }
    neu->idx = frei;
    neu->nr = ++a->nr;
    a->platz[frei] = neu;
    pthread_mutex_unlock(&a->mtx);
    return opfer >= 0;
}

static void melden(qc_annahme *a, int64_t t) {
    if (a->cfg.andrang)
        a->cfg.andrang(a->verdraengt, &a->verdraengt_von, a->abgewiesen, &a->abgewiesen_von, a->cfg.ctx);
    a->verdraengt = a->abgewiesen = 0;
    a->gemeldet_s = t;
}

static void *annahme_faden(void *arg) {
    qc_annahme *a = arg;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    for (;;) {
        // Eine offene Zaehlung kommt spaetestens nach der Sperre heraus,
        // auch wenn danach niemand mehr anklopft.
        int warte_ms = -1;
        if (a->verdraengt || a->abgewiesen) {
            int64_t rest = a->gemeldet_s + QC_MELDEN_S - jetzt_s();
            if (rest <= 0) { melden(a, jetzt_s()); continue; }
            warte_ms = (int)rest * 1000;
        }
        // Gewartet wird immer in poll - auf den Port und auf den Wecker des
        // Anhaltens. accept selbst blockiert nie (der Socket ist nicht
        // blockierend): gibt eine Gegenstelle zwischen poll und accept auf,
        // kommt EAGAIN, und es geht zurueck ins Warten, wo auch das Anhalten
        // ankommt.
        struct pollfd pf[2] = {
            { .fd = a->listen_fd, .events = POLLIN, .revents = 0 },
            { .fd = a->weck[0], .events = POLLIN, .revents = 0 },
        };
        int n = poll(pf, a->weck[0] >= 0 ? 2 : 1, warte_ms);
        if (atomic_load(&a->halt)) break;
        if (n <= 0) {
            if (n < 0 && errno != EINTR) usleep(100 * 1000);
            continue;
        }
        if (pf[0].revents & POLLNVAL) break;
        if (!(pf[0].revents & (POLLIN | POLLERR | POLLHUP))) continue;

        struct sockaddr_in peer; socklen_t plen = sizeof peer;
        memset(&peer, 0, sizeof peer);
        int fd = accept(a->listen_fd, (struct sockaddr *)&peer, &plen);
        if (fd < 0) {
            if (errno == EBADF || errno == EINVAL || errno == ENOTSOCK) break;
            // Alles andere geht vorueber: eine Gegenstelle, die vor dem
            // accept aufgibt, oder kurz keine Dateinummern. Der Port bleibt
            // offen - frueher endete der Faden hier fuer immer.
            if (errno != EINTR && errno != ECONNABORTED && errno != EAGAIN && errno != EWOULDBLOCK) usleep(100 * 1000);
            continue;
        }
        // Unter macOS erbt die neue Verbindung O_NONBLOCK vom lauschenden
        // Socket; Handschlag und Sitzung arbeiten blockierend.
        int fl = fcntl(fd, F_GETFL);
        if (fl >= 0 && (fl & O_NONBLOCK)) fcntl(fd, F_SETFL, fl & ~O_NONBLOCK);

        qc_platz *p = NULL;
        if (atomic_load(&a->faeden) >= 2 * a->cfg.max_gesamt) {
            // Verdraengte Faeden kommen nicht hinterher: dann lieber den
            // Neuen gleich schliessen als Faeden ohne Ende anlegen.
            close(fd);
            a->abgewiesen++;
            a->abgewiesen_von = peer;
        } else if (!(p = calloc(1, sizeof *p))) {
            close(fd);
        } else {
            p->a = a;
            p->fd = fd;
            p->peer = peer;
            if (platz_nehmen(a, p)) a->verdraengt++;
            atomic_fetch_add(&a->faeden, 1);
            pthread_t t;
            if (pthread_create(&t, &attr, verbindung_faden, p) != 0) {
                qc_platz_frei(p);
                close(fd);
                free(p);
                atomic_fetch_sub(&a->faeden, 1);
            }
        }

        int64_t t = jetzt_s();
        if ((a->verdraengt || a->abgewiesen) && t - a->gemeldet_s >= QC_MELDEN_S) melden(a, t);
    }
    pthread_attr_destroy(&attr);
    // Angehalten: der Port wird frei. Eine offene Zaehlung faellt weg - wer
    // anhaelt, will vom Port nichts mehr hoeren.
    if (atomic_load(&a->halt)) {
        close(a->listen_fd);
        if (a->weck[0] >= 0) close(a->weck[0]);
    }
    return NULL;
}

qc_annahme *qc_annahme_neu(int listen_fd, const qc_annahme_cfg *cfg) {
    if (!cfg || !cfg->verbindung || cfg->max_gesamt < 1 || cfg->max_je_ip < 1) return NULL;
    qc_annahme *a = calloc(1, sizeof *a);
    if (!a) return NULL;
    a->listen_fd = listen_fd;
    a->cfg = *cfg;
    if (a->cfg.max_gesamt > QC_ANNAHME_PLAETZE) a->cfg.max_gesamt = QC_ANNAHME_PLAETZE;
    a->gemeldet_s = jetzt_s() - QC_MELDEN_S;    // die erste Meldung kommt sofort
    if (pipe(a->weck) != 0) {
        free(a);
        return NULL;
    }
    fcntl(a->weck[0], F_SETFD, FD_CLOEXEC);
    fcntl(a->weck[1], F_SETFD, FD_CLOEXEC);
    int fl = fcntl(listen_fd, F_GETFL);
    if (fl >= 0) fcntl(listen_fd, F_SETFL, fl | O_NONBLOCK);
    pthread_mutex_init(&a->mtx, NULL);
    pthread_t t;
    if (pthread_create(&t, NULL, annahme_faden, a) != 0) {
        if (fl >= 0) fcntl(listen_fd, F_SETFL, fl);
        close(a->weck[0]);
        close(a->weck[1]);
        pthread_mutex_destroy(&a->mtx);
        free(a);
        return NULL;
    }
    pthread_detach(t);
    return a;
}

int qc_annahme_starten(int listen_fd, const qc_annahme_cfg *cfg) {
    return qc_annahme_neu(listen_fd, cfg) ? 0 : -1;
}

void qc_annahme_stoppen(qc_annahme *a) {
    if (!a || atomic_exchange(&a->halt, 1)) return;
    // Erst halt, dann das Byte: der Faden sieht halt spaetestens, wenn poll
    // wegen des Bytes zurueckkehrt. weck[1] gehoert nur dem Anhalten.
    char x = 1;
    ssize_t r;
    do r = write(a->weck[1], &x, 1); while (r < 0 && errno == EINTR);
    close(a->weck[1]);
    a->weck[1] = -1;
}
