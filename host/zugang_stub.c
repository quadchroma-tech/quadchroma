// Platzhalter fuer den Kern des Zugangs (host/zugang.h), nur fuer die Bauten
// von Paket P3 (Oberflaeche), solange Paket P2 host/zugang.c schreibt.
//
// wird bei der Zusammenfuehrung durch P2 ersetzt: dann diese Datei loeschen
// und im Makefile die Variable UI_STUB samt ihrer Zeilen entfernen - SRC nennt
// schon host/zugang.c.
//
// Alles liegt nur im Speicher (nichts auf der Platte, kein Protokoll): eine
// feste ID, ein Passwort, zwei Beispielgeraete. Fadensicher ueber eine Sperre.
#include "zugang.h"

#include <pthread.h>
#include <stdlib.h>
#include <string.h>

static pthread_mutex_t g_m = PTHREAD_MUTEX_INITIALIZER;
static char g_pw[64] = "k7m-4wq-9tz";
static qc_geraet g_liste[8] = {
    { .pub = { 1 }, .id = 581729911, .name = "Windows-PC", .datum = "2026-09-26" },
    { .pub = { 2 }, .id = 5,         .name = "Laptop",     .datum = "2026-09-20" },
};
static int g_anzahl = 2;

uint32_t qc_zugang_eigene_id(void) { return 581729911u; }

int qc_zugang_passwort(char *puffer, size_t groesse) {
    if (!puffer || !groesse) return -1;
    pthread_mutex_lock(&g_m);
    strlcpy(puffer, g_pw, groesse);
    pthread_mutex_unlock(&g_m);
    return 0;
}

// norm aus Abschnitt 3.4 nur fuer die Laengenpruefung: ohne Leerraum und '-'.
static size_t norm_laenge(const char *pw) {
    size_t n = 0;
    for (const char *p = pw; *p; p++)
        if (*p != ' ' && *p != '\t' && *p != '\n' && *p != '\r' && *p != '-') n++;
    return n;
}

int qc_zugang_passwort_setzen(const char *pw) {
    if (!pw || norm_laenge(pw) < 8) return -1;
    if (strlen(pw) >= sizeof g_pw) return -2;
    pthread_mutex_lock(&g_m);
    strlcpy(g_pw, pw, sizeof g_pw);
    pthread_mutex_unlock(&g_m);
    return 0;
}

int qc_zugang_passwort_zufall(void) {
    static const char zeichen[] = "abcdefghjkmnpqrstuvwxyz23456789";
    char neu[12];
    for (int i = 0; i < 11; i++)
        neu[i] = (i == 3 || i == 7) ? '-' : zeichen[arc4random_uniform((uint32_t)(sizeof zeichen - 1))];
    neu[11] = 0;
    pthread_mutex_lock(&g_m);
    strlcpy(g_pw, neu, sizeof g_pw);
    pthread_mutex_unlock(&g_m);
    return 0;
}

int qc_zugang_geraete(qc_geraet *liste, int max) {
    pthread_mutex_lock(&g_m);
    int n = g_anzahl < max ? g_anzahl : max;
    if (n > 0 && liste) memcpy(liste, g_liste, (size_t)n * sizeof *liste);
    pthread_mutex_unlock(&g_m);
    return n < 0 ? 0 : n;
}

int qc_zugang_geraet_entfernen(const uint8_t pub[32]) {
    pthread_mutex_lock(&g_m);
    for (int i = 0; i < g_anzahl; i++)
        if (memcmp(g_liste[i].pub, pub, 32) == 0) {
            memmove(&g_liste[i], &g_liste[i + 1], (size_t)(g_anzahl - i - 1) * sizeof g_liste[0]);
            g_anzahl--;
            break;
        }
    pthread_mutex_unlock(&g_m);
    return 0;
}

int qc_zugang_alle_entfernen(void) {
    pthread_mutex_lock(&g_m);
    g_anzahl = 0;
    pthread_mutex_unlock(&g_m);
    return 0;
}

int qc_zugang_liste_zuruecksetzen(void) { return qc_zugang_alle_entfernen(); }

void qc_zugang_entscheiden(uint64_t anfrage, int zulassen) { (void)anfrage; (void)zulassen; }

int qc_zustand_zuschauer(char *name, size_t groesse) {
    if (name && groesse) name[0] = 0;
    return 0;
}

int qc_zustand_bildschirmfreigabe(void) { return 1; }
int qc_zustand_bedienungshilfen(void) { return 1; }
int qc_zustand_port_belegt(void) { return 0; }

// Schwache Standardfassungen der Oberflaechen-Rueckrufe wie bei P2: ohne
// menue.m gilt "keine Oberflaeche".
__attribute__((weak)) void qc_ui_anfrage(uint64_t anfrage, const char *name, uint32_t id, uint32_t code) {
    (void)anfrage; (void)name; (void)id; (void)code;
}
__attribute__((weak)) void qc_ui_anfrage_zurueck(uint64_t anfrage) { (void)anfrage; }
__attribute__((weak)) void qc_ui_zustand_geaendert(void) {}
__attribute__((weak)) int qc_ui_vorhanden(void) { return 0; }
