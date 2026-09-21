#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#include <mach/mach.h>
#include <mach/mach_host.h>
#include <mach/mach_time.h>
#include <libproc.h>
#include <sys/sysctl.h>
#include <string.h>

#include "last.h"

// ------------------------------------------------------------------- Kerne
//
// host_processor_info liefert je logischem Kern vier Zaehler in Zehntel-
// millisekunden. Auslastung ist die Differenz zweier Proben - ein einzelner
// Abruf sagt nur, was seit dem Start war, und das ist uninteressant.

static uint64_t g_letzt_belegt = 0, g_letzt_gesamt = 0;

static void kerne_lesen(uint16_t *promille) {
    natural_t anzahl = 0;
    processor_info_array_t info = NULL;
    mach_msg_type_number_t cnt = 0;
    if (host_processor_info(mach_host_self(), PROCESSOR_CPU_LOAD_INFO,
                            &anzahl, &info, &cnt) != KERN_SUCCESS) {
        *promille = 0;
        return;
    }
    processor_cpu_load_info_t last = (processor_cpu_load_info_t)info;
    uint64_t belegt = 0, gesamt = 0;
    for (natural_t i = 0; i < anzahl; i++) {
        uint64_t u = last[i].cpu_ticks[CPU_STATE_USER];
        uint64_t s = last[i].cpu_ticks[CPU_STATE_SYSTEM];
        uint64_t n = last[i].cpu_ticks[CPU_STATE_NICE];
        uint64_t l = last[i].cpu_ticks[CPU_STATE_IDLE];
        belegt += u + s + n;
        gesamt += u + s + n + l;
    }
    // Der Speicher gehoert uns, nicht dem Kern - ohne das laeuft er voll.
    vm_deallocate(mach_task_self(), (vm_address_t)info, cnt * sizeof(integer_t));

    uint64_t db = belegt - g_letzt_belegt;
    uint64_t dg = gesamt - g_letzt_gesamt;
    *promille = (g_letzt_gesamt && dg) ? (uint16_t)((db * 1000) / dg) : 0;
    g_letzt_belegt = belegt;
    g_letzt_gesamt = gesamt;
}

// ---------------------------------------------------------- Eigener Anteil

static uint64_t g_eigen_ns = 0;
static uint64_t g_eigen_zeit_ns = 0;

static uint64_t jetzt_ns(void) {
    static mach_timebase_info_data_t tb;
    if (tb.denom == 0) mach_timebase_info(&tb);
    return mach_absolute_time() * tb.numer / tb.denom;
}

static void eigen_lesen(uint16_t *promille, uint32_t *mb) {
    struct rusage_info_v4 ri;
    if (proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t *)&ri) != 0) {
        *promille = 0;
        *mb = 0;
        return;
    }
    // Diese Felder kommen in Mach-Zeiteinheiten, nicht in Nanosekunden. Ohne
    // Umrechnung ueber die Zeitbasis meldet ein Apple-Silicon-Mac rund den
    // vierzigsten Teil der echten Auslastung - gemessen: 1 % statt 25 %.
    static mach_timebase_info_data_t tb;
    if (tb.denom == 0) mach_timebase_info(&tb);
    uint64_t cpu = (ri.ri_user_time + ri.ri_system_time) * tb.numer / tb.denom;
    uint64_t nun = jetzt_ns();
    if (g_eigen_zeit_ns && nun > g_eigen_zeit_ns) {
        uint64_t dc = cpu - g_eigen_ns;
        uint64_t dt = nun - g_eigen_zeit_ns;
        // Auf dieselbe Bezugsgroesse wie die Gesamtauslastung gebracht: Anteil
        // an ALLEN Kernen. Sonst steht "14 % gesamt, davon 16 % eigen" da -
        // richtig gerechnet, aber unlesbar, weil das eine je Maschine und das
        // andere je Kern gemeint war.
        static uint64_t kerne = 0;
        if (kerne == 0) {
            int n = 0; size_t len = sizeof n;
            kerne = (sysctlbyname("hw.logicalcpu", &n, &len, NULL, 0) == 0 && n > 0) ? (uint64_t)n : 1;
        }
        uint64_t p = (dc * 1000) / (dt * kerne);
        *promille = (uint16_t)(p > 1000 ? 1000 : p);
    } else {
        *promille = 0;
    }
    g_eigen_ns = cpu;
    g_eigen_zeit_ns = nun;
    *mb = (uint32_t)(ri.ri_phys_footprint / (1024 * 1024));
}

// ------------------------------------------------------------- Arbeitsspeicher

static void speicher_lesen(uint32_t *benutzt_mb, uint32_t *gesamt_mb, uint16_t *druck) {
    vm_size_t seite = 0;
    host_page_size(mach_host_self(), &seite);
    vm_statistics64_data_t vm;
    mach_msg_type_number_t cnt = HOST_VM_INFO64_COUNT;
    if (host_statistics64(mach_host_self(), HOST_VM_INFO64,
                          (host_info64_t)&vm, &cnt) == KERN_SUCCESS && seite) {
        // "Benutzt" im Sinne der Aktivitaetsanzeige: alles ausser frei und
        // spekulativ. Inaktive Seiten sind belegt, auch wenn sie sich
        // zurueckholen lassen.
        uint64_t belegt = (uint64_t)(vm.active_count + vm.wire_count +
                                     vm.compressor_page_count) * seite;
        *benutzt_mb = (uint32_t)(belegt / (1024 * 1024));
    } else {
        *benutzt_mb = 0;
    }
    uint64_t gesamt = 0;
    size_t len = sizeof gesamt;
    if (sysctlbyname("hw.memsize", &gesamt, &len, NULL, 0) == 0) {
        *gesamt_mb = (uint32_t)(gesamt / (1024 * 1024));
    } else {
        *gesamt_mb = 0;
    }
    int stufe = 1;
    len = sizeof stufe;
    if (sysctlbyname("kern.memorystatus_vm_pressure_level", &stufe, &len, NULL, 0) != 0) {
        stufe = 1;
    }
    *druck = (uint16_t)stufe;
}

// -------------------------------------------------------------------- Grafik
//
// Die Grafikkerne melden sich ueber die IORegistry. Das ist der einzige Weg
// ohne Root und ohne Hilfsprogramm. Nicht jede Maschine fuehrt den Zaehler -
// deshalb gibt es einen Wert fuer "nicht lesbar" statt einer geratenen Null.

static void grafik_lesen(uint16_t *promille) {
    *promille = 0xffff;
    io_iterator_t it = 0;
    if (IOServiceGetMatchingServices(kIOMainPortDefault,
                                     IOServiceMatching("IOAccelerator"), &it) != KERN_SUCCESS) {
        return;
    }
    io_registry_entry_t e;
    while ((e = IOIteratorNext(it))) {
        CFMutableDictionaryRef props = NULL;
        if (IORegistryEntryCreateCFProperties(e, &props, kCFAllocatorDefault, 0) == KERN_SUCCESS && props) {
            CFDictionaryRef stats = CFDictionaryGetValue(props, CFSTR("PerformanceStatistics"));
            if (stats && CFGetTypeID(stats) == CFDictionaryGetTypeID()) {
                CFNumberRef n = CFDictionaryGetValue(stats, CFSTR("Device Utilization %"));
                if (n && CFGetTypeID(n) == CFNumberGetTypeID()) {
                    int v = 0;
                    CFNumberGetValue(n, kCFNumberIntType, &v);
                    if (v < 0) v = 0;
                    if (v > 100) v = 100;
                    *promille = (uint16_t)(v * 10);
                }
            }
            CFRelease(props);
        }
        IOObjectRelease(e);
        if (*promille != 0xffff) break;
    }
    IOObjectRelease(it);
}

void qc_last_probe(qc_last *out) {
    memset(out, 0, sizeof *out);
    kerne_lesen(&out->cpu_promille);
    eigen_lesen(&out->cpu_eigen_promille, &out->eigen_mb);
    speicher_lesen(&out->ram_benutzt_mb, &out->ram_gesamt_mb, &out->druck);
    grafik_lesen(&out->gpu_promille);
}
