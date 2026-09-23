#include "qc_annahme.h"

#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#define QC_ANNAHME_PLAETZE 16       // obere Grenze fuer max_gesamt

typedef struct {
    int listen_fd;
    qc_annahme_cfg cfg;
    pthread_mutex_t mtx;            // schuetzt belegt und ip
    int belegt[QC_ANNAHME_PLAETZE];
    in_addr_t ip[QC_ANNAHME_PLAETZE];
    long abgewiesen;                // seit der letzten Meldung, nur im Annahmefaden
    int64_t gemeldet_s;
} qc_annahme;

struct qc_platz {
    qc_annahme *a;
    int idx;
    _Atomic int frei;
    int fd;
    struct sockaddr_in peer;
};

static int64_t jetzt_s(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec;
}

void qc_platz_frei(qc_platz *p) {
    if (!p || atomic_exchange(&p->frei, 1)) return;
    pthread_mutex_lock(&p->a->mtx);
    p->a->belegt[p->idx] = 0;
    pthread_mutex_unlock(&p->a->mtx);
}

static void *verbindung_faden(void *arg) {
    qc_platz *p = arg;
    p->a->cfg.verbindung(p, p->fd, &p->peer, p->a->cfg.ctx);
    qc_platz_frei(p);
    free(p);
    return NULL;
}

// Freien Platz suchen und belegen. -1 = keiner frei oder diese Adresse hat
// schon genug.
static int platz_nehmen(qc_annahme *a, in_addr_t ip) {
    pthread_mutex_lock(&a->mtx);
    int frei = -1, gleich = 0;
    for (int i = 0; i < a->cfg.max_gesamt; i++) {
        if (!a->belegt[i]) { if (frei < 0) frei = i; continue; }
        if (a->ip[i] == ip) gleich++;
    }
    if (gleich >= a->cfg.max_je_ip) frei = -1;
    if (frei >= 0) { a->belegt[frei] = 1; a->ip[frei] = ip; }
    pthread_mutex_unlock(&a->mtx);
    return frei;
}

static void *annahme_faden(void *arg) {
    qc_annahme *a = arg;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    for (;;) {
        struct sockaddr_in peer; socklen_t plen = sizeof peer;
        memset(&peer, 0, sizeof peer);
        int fd = accept(a->listen_fd, (struct sockaddr *)&peer, &plen);
        if (fd < 0) {
            if (errno == EBADF || errno == EINVAL || errno == ENOTSOCK) break;
            // Alles andere geht vorueber: eine Gegenstelle, die vor dem
            // accept aufgibt, oder kurz keine Dateinummern. Der Port bleibt
            // offen - frueher endete der Faden hier fuer immer.
            if (errno != EINTR && errno != ECONNABORTED) usleep(100 * 1000);
            continue;
        }

        int idx = platz_nehmen(a, peer.sin_addr.s_addr);
        if (idx < 0) {
            close(fd);
            a->abgewiesen++;
            int64_t t = jetzt_s();
            if (a->cfg.abgewiesen && (a->gemeldet_s == 0 || t - a->gemeldet_s >= 10)) {
                a->cfg.abgewiesen(a->abgewiesen, &peer, a->cfg.ctx);
                a->abgewiesen = 0;
                a->gemeldet_s = t ? t : 1;
            }
            continue;
        }

        qc_platz *p = calloc(1, sizeof *p);
        if (!p) {
            close(fd);
            pthread_mutex_lock(&a->mtx);
            a->belegt[idx] = 0;
            pthread_mutex_unlock(&a->mtx);
            continue;
        }
        p->a = a;
        p->idx = idx;
        p->fd = fd;
        p->peer = peer;
        pthread_t t;
        if (pthread_create(&t, &attr, verbindung_faden, p) != 0) {
            close(fd);
            qc_platz_frei(p);
            free(p);
        }
    }
    pthread_attr_destroy(&attr);
    return NULL;
}

int qc_annahme_starten(int listen_fd, const qc_annahme_cfg *cfg) {
    if (!cfg || !cfg->verbindung || cfg->max_gesamt < 1 || cfg->max_je_ip < 1) return -1;
    qc_annahme *a = calloc(1, sizeof *a);
    if (!a) return -1;
    a->listen_fd = listen_fd;
    a->cfg = *cfg;
    if (a->cfg.max_gesamt > QC_ANNAHME_PLAETZE) a->cfg.max_gesamt = QC_ANNAHME_PLAETZE;
    pthread_mutex_init(&a->mtx, NULL);
    pthread_t t;
    if (pthread_create(&t, NULL, annahme_faden, a) != 0) {
        pthread_mutex_destroy(&a->mtx);
        free(a);
        return -1;
    }
    pthread_detach(t);
    return 0;
}
