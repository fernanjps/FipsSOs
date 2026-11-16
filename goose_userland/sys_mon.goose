// FipSOs v4.0 - Capa 3 Userland Monitor en Lenguaje Goose
// Compilado sin asignador dinamico ni Garbage Collector (data-stack only).

struct SysSnapshot {
    pid: i64,
    uptime_s: i64,
    total_kib: u64,
    free_kib: u64,
    used_kib: u64,
    mono_sec: i64,
    mono_ms: i64,
    procs: u16,
}

extern fn goose_getpid() -> i64
extern fn goose_uname(u: *UtsName) -> i64
extern fn goose_sysinfo(si: *SysInfo) -> i64
extern fn goose_clock_monotonic(ts: *TimeSpec) -> i64
extern fn goose_read_file(path: str, buf: []u8, cap: usize) -> i64
extern fn goose_write_str(s: str)
extern fn goose_write_u64(v: u64)

fn collect_snapshot() -> SysSnapshot {
    let mut si: SysInfo
    let mut ts: TimeSpec
    let _ = goose_sysinfo(&si)
    let _ = goose_clock_monotonic(&ts)

    let total_kib: u64 = si.totalram / 1024
    let free_kib: u64 = si.freeram / 1024
    let used_kib: u64 = if total_kib > free_kib { total_kib - free_kib } else { 0 }

    return SysSnapshot {
        pid: goose_getpid(),
        uptime_s: si.uptime,
        total_kib: total_kib,
        free_kib: free_kib,
        used_kib: used_kib,
        mono_sec: ts.tv_sec,
        mono_ms: ts.tv_nsec / 1000000,
        procs: si.procs,
    }
}

fn main() -> i32 {
    let mut uts: UtsName
    let _ = goose_uname(&uts)
    let snap = collect_snapshot()

    let mut hwmon_buf: [2048]u8
    let hwmon_len = goose_read_file("/sys/fipsos/hwmon", &hwmon_buf, 2048)

    goose_write_str("\n=== GOOSE CAPA 3 MONITOR (FipSOs v4.0 | Zero-GC Data-Stack) ===\n")
    goose_write_str("[Goose] PID=")
    goose_write_u64(snap.pid as u64)
    goose_write_str(" | OS=")
    goose_write_str(uts.sysname)
    goose_write_str(" ")
    goose_write_str(uts.machine)
    goose_write_str(" | Uptime=")
    goose_write_u64(snap.uptime_s as u64)
    goose_write_str("s (")
    goose_write_u64(snap.mono_sec as u64)
    goose_write_str(".")
    goose_write_u64(snap.mono_ms as u64)
    goose_write_str("s monotonic)\n")

    goose_write_str("[Goose] RAM Fisica: ")
    goose_write_u64(snap.used_kib)
    goose_write_str(" KiB usados / ")
    goose_write_u64(snap.total_kib)
    goose_write_str(" KiB total | Libres=")
    goose_write_u64(snap.free_kib)
    goose_write_str(" KiB | Procs=")
    goose_write_u64(snap.procs as u64)
    goose_write_str("\n")

    if hwmon_len > 0 {
        goose_write_str("[Goose] /sys/fipsos/hwmon (")
        goose_write_u64(hwmon_len as u64)
        goose_write_str(" B):\n")
        goose_write_str(hwmon_buf[..hwmon_len])
    } else {
        goose_write_str("[Goose] /sys/fipsos/hwmon no disponible.\n")
    }

    goose_write_str("=== [Goose] Telemetria Ring 3 Completada OK ===\n")
    return 0
}
