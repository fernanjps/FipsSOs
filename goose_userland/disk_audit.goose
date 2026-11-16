// FipSOs Ring 3 Zero-GC Storage Auditor written in Goose (https://aardappel.github.io/goose/)
// Compiles to freestanding C without GC or heap allocation via data-stack value types.

extern fn goose_write_str(s: str)
extern fn goose_write_u64(v: u64)
extern fn goose_sysinfo(si: *SysInfo) -> i64
extern fn goose_audit_dir(path: str, files: *u64, dirs: *u64, bytes: *u64) -> i64

struct SysInfo {
    uptime: i64,
    loads: [u64; 3],
    totalram: u64,
    freeram: u64,
    sharedram: u64,
    bufferram: u64,
    totalswap: u64,
    freeswap: u64,
    procs: u16,
    mem_unit: u32,
}

fn report_mount(label: str, path: str) {
    var files: u64 = 0
    var dirs: u64 = 0
    var bytes: u64 = 0
    let rc = goose_audit_dir(path, &files, &dirs, &bytes)
    goose_write_str("[goose_audit] ")
    goose_write_str(label)
    if rc < 0 {
        goose_write_str(" -> no montado / inaccesible\n")
        return
    }
    goose_write_str(" | archivos=")
    goose_write_u64(files)
    goose_write_str(" | dirs=")
    goose_write_u64(dirs)
    goose_write_str(" | bytes=")
    goose_write_u64(bytes)
    goose_write_str("\n")
}

fn main() -> i64 {
    var si = SysInfo {
        uptime: 0,
        loads: [0, 0, 0],
        totalram: 0,
        freeram: 0,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: 0,
        mem_unit: 0,
    }
    goose_write_str("=== FipSOs Goose Zero-GC Disk Audit (v4.6) ===\n")
    report_mount("/mnt/fat0", "/mnt/fat0")
    report_mount("/disk    ", "/disk")
    if goose_sysinfo(&si) == 0 {
        let unit = if si.mem_unit > 0 { si.mem_unit as u64 } else { 1 }
        let total_kb = (si.totalram * unit) / 1024
        let free_kb = (si.freeram * unit) / 1024
        goose_write_str("[goose_audit] RAM libre: ")
        goose_write_u64(free_kb)
        goose_write_str(" KB / ")
        goose_write_u64(total_kb)
        goose_write_str(" KB | procs=")
        goose_write_u64(si.procs as u64)
        goose_write_str("\n")
    }
    return 0
}
