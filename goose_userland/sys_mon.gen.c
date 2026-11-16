/*
 * GENERATED DETERMINISTICALLY FROM goose_userland/sys_mon.goose
 * FipSOs v4.0 - Capa 3 Goose System Monitor (Static Data-Stack, Zero Heap/GC)
 */

#include "fipsos_rt.h"

typedef struct {
    int64_t  pid;
    int64_t  uptime_s;
    uint64_t total_kib;
    uint64_t free_kib;
    uint64_t used_kib;
    int64_t  mono_sec;
    int64_t  mono_ms;
    uint16_t procs;
} goose_SysSnapshot;

static goose_SysSnapshot goose_collect_snapshot(void) {
    fipsos_sysinfo_t si = {0};
    fipsos_timespec_t ts = {0};

    (void)goose_sysinfo(&si);
    (void)goose_clock_monotonic(&ts);

    uint64_t total_kib = si.totalram / 1024ULL;
    uint64_t free_kib  = si.freeram / 1024ULL;
    uint64_t used_kib  = (total_kib > free_kib) ? (total_kib - free_kib) : 0ULL;

    goose_SysSnapshot snap;
    snap.pid       = goose_getpid();
    snap.uptime_s  = si.uptime;
    snap.total_kib = total_kib;
    snap.free_kib  = free_kib;
    snap.used_kib  = used_kib;
    snap.mono_sec  = ts.tv_sec;
    snap.mono_ms   = ts.tv_nsec / 1000000LL;
    snap.procs     = si.procs;
    return snap;
}

int main(void) {
    fipsos_utsname_t uts = {0};
    (void)goose_uname(&uts);

    goose_SysSnapshot snap = goose_collect_snapshot();

    static char hwmon_buf[2048];
    long hwmon_len = goose_read_file("/sys/fipsos/hwmon", hwmon_buf, sizeof(hwmon_buf));

    goose_write_str("\n=== GOOSE CAPA 3 MONITOR (FipSOs v4.0 | Zero-GC Data-Stack) ===\n");
    goose_write_str("[Goose] PID=");
    goose_write_u64((uint64_t)(snap.pid > 0 ? snap.pid : 0));
    goose_write_str(" | OS=");
    goose_write_str(uts.sysname[0] ? uts.sysname : "FipSOs");
    goose_write_str(" ");
    goose_write_str(uts.machine[0] ? uts.machine : "x86_64");
    goose_write_str(" | Uptime=");
    goose_write_u64((uint64_t)(snap.uptime_s >= 0 ? snap.uptime_s : 0));
    goose_write_str("s (");
    goose_write_u64((uint64_t)(snap.mono_sec >= 0 ? snap.mono_sec : 0));
    goose_write_str(".");
    goose_write_u64((uint64_t)(snap.mono_ms >= 0 ? snap.mono_ms : 0));
    goose_write_str("s monotonic)\n");

    goose_write_str("[Goose] RAM Fisica: ");
    goose_write_u64(snap.used_kib);
    goose_write_str(" KiB usados / ");
    goose_write_u64(snap.total_kib);
    goose_write_str(" KiB total | Libres=");
    goose_write_u64(snap.free_kib);
    goose_write_str(" KiB | Procs=");
    goose_write_u64((uint64_t)snap.procs);
    goose_write_str("\n");

    if (hwmon_len > 0) {
        goose_write_str("[Goose] /sys/fipsos/hwmon (");
        goose_write_u64((uint64_t)hwmon_len);
        goose_write_str(" B):\n");
        goose_write_bytes(hwmon_buf, (size_t)hwmon_len);
        if (hwmon_buf[hwmon_len - 1] != '\n') {
            goose_write_str("\n");
        }
    } else {
        goose_write_str("[Goose] /sys/fipsos/hwmon no disponible.\n");
    }

    goose_write_str("=== [Goose] Telemetria Ring 3 Completada OK ===\n");
    return 0;
}
