/* Generated from goose_userland/disk_audit.goose (Zero-GC data-stack backend) */
#include "fipsos_rt.h"

static void goose_report_mount(const char *label, const char *path) {
    uint64_t files = 0;
    uint64_t dirs = 0;
    uint64_t bytes = 0;
    long rc = goose_audit_dir(path, &files, &dirs, &bytes);
    goose_write_str("[goose_audit] ");
    goose_write_str(label);
    if (rc < 0) {
        goose_write_str(" -> no montado / inaccesible\n");
        return;
    }
    goose_write_str(" | archivos=");
    goose_write_u64(files);
    goose_write_str(" | dirs=");
    goose_write_u64(dirs);
    goose_write_str(" | bytes=");
    goose_write_u64(bytes);
    goose_write_str("\n");
}

int main(void) {
    fipsos_sysinfo_t si = {0};
    goose_write_str("=== FipSOs Goose Zero-GC Disk Audit (v4.6) ===\n");
    goose_report_mount("/mnt/fat0", "/mnt/fat0");
    goose_report_mount("/disk    ", "/disk");
    if (goose_sysinfo(&si) == 0) {
        uint64_t unit = si.mem_unit ? (uint64_t)si.mem_unit : 1ULL;
        uint64_t total_kb = (si.totalram * unit) / 1024ULL;
        uint64_t free_kb = (si.freeram * unit) / 1024ULL;
        goose_write_str("[goose_audit] RAM libre: ");
        goose_write_u64(free_kb);
        goose_write_str(" KB / ");
        goose_write_u64(total_kb);
        goose_write_str(" KB | procs=");
        goose_write_u64((uint64_t)si.procs);
        goose_write_str("\n");
    }
    return 0;
}
