#ifndef FIPSOS_GOOSE_RT_H
#define FIPSOS_GOOSE_RT_H

/*
 * FipSOs v4.0 Capa 3 - Runtime Minimalista para Lenguaje Goose
 * Sin asignador de heap ni Garbage Collector (datos estaticos / stack only).
 */

#include <stdint.h>
#include <stddef.h>

#define FIPSOS_SYS_READ          0L
#define FIPSOS_SYS_WRITE         1L
#define FIPSOS_SYS_OPEN          2L
#define FIPSOS_SYS_CLOSE         3L
#define FIPSOS_SYS_FSTAT         5L
#define FIPSOS_SYS_GETPID        39L
#define FIPSOS_SYS_EXIT          60L
#define FIPSOS_SYS_UNAME         63L
#define FIPSOS_SYS_SYSINFO       99L
#define FIPSOS_SYS_GETDENTS64    217L
#define FIPSOS_SYS_CLOCK_GETTIME 228L
#define FIPSOS_SYS_OPENAT        257L

#define FIPSOS_AT_FDCWD          (-100L)
#define FIPSOS_O_RDONLY          0L
#define FIPSOS_CLOCK_MONOTONIC   1L

typedef struct {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
    char domainname[65];
} fipsos_utsname_t;

typedef struct {
    int64_t  uptime;
    uint64_t loads[3];
    uint64_t totalram;
    uint64_t freeram;
    uint64_t sharedram;
    uint64_t bufferram;
    uint64_t totalswap;
    uint64_t freeswap;
    uint16_t procs;
    uint16_t pad;
    uint32_t pad2;
    uint64_t totalhigh;
    uint64_t freehigh;
    uint32_t mem_unit;
    char     _f[22];
} fipsos_sysinfo_t;

typedef struct {
    int64_t tv_sec;
    int64_t tv_nsec;
} fipsos_timespec_t;

static inline long fipsos_sc0(long n) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n) : "rcx", "r11", "memory");
    return ret;
}

static inline long fipsos_sc1(long n, long a1) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1) : "rcx", "r11", "memory");
    return ret;
}

static inline long fipsos_sc2(long n, long a1, long a2) {
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(n), "D"(a1), "S"(a2) : "rcx", "r11", "memory");
    return ret;
}

static inline long fipsos_sc3(long n, long a1, long a2, long a3) {
    long ret;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"(n), "D"(a1), "S"(a2), "d"(a3)
                     : "rcx", "r11", "memory");
    return ret;
}

static inline size_t goose_strlen(const char *s) {
    size_t n = 0;
    while (s[n] != '\0') {
        n++;
    }
    return n;
}

static inline void goose_write_str(const char *s) {
    size_t len = goose_strlen(s);
    if (len > 0) {
        if (fipsos_sc3(FIPSOS_SYS_WRITE, 1L, (long)(uintptr_t)s, (long)len) < 0) {
            /* best-effort stdout write */
        }
    }
}

static inline void goose_write_bytes(const char *buf, size_t len) {
    if (len > 0) {
        if (fipsos_sc3(FIPSOS_SYS_WRITE, 1L, (long)(uintptr_t)buf, (long)len) < 0) {
            /* best-effort stdout write */
        }
    }
}

static inline void goose_write_u64(uint64_t val) {
    char tmp[24];
    size_t idx = 0;
    if (val == 0) {
        goose_write_bytes("0", 1);
        return;
    }
    while (val > 0 && idx < sizeof(tmp)) {
        tmp[idx++] = (char)('0' + (val % 10ULL));
        val /= 10ULL;
    }
    while (idx > 0) {
        goose_write_bytes(&tmp[--idx], 1);
    }
}

static inline long goose_getpid(void) {
    return fipsos_sc0(FIPSOS_SYS_GETPID);
}

static inline long goose_uname(fipsos_utsname_t *u) {
    return fipsos_sc1(FIPSOS_SYS_UNAME, (long)(uintptr_t)u);
}

static inline long goose_sysinfo(fipsos_sysinfo_t *si) {
    return fipsos_sc1(FIPSOS_SYS_SYSINFO, (long)(uintptr_t)si);
}

static inline long goose_clock_monotonic(fipsos_timespec_t *ts) {
    return fipsos_sc2(FIPSOS_SYS_CLOCK_GETTIME, FIPSOS_CLOCK_MONOTONIC, (long)(uintptr_t)ts);
}

static inline long goose_read_file(const char *path, char *out_buf, size_t cap) {
    long fd = fipsos_sc3(FIPSOS_SYS_OPEN, (long)(uintptr_t)path, FIPSOS_O_RDONLY, 0L);
    if (fd < 0 || cap == 0) {
        return fd;
    }
    size_t total = 0;
    while (total + 1 < cap) {
        long r = fipsos_sc3(
            FIPSOS_SYS_READ,
            fd,
            (long)(uintptr_t)(out_buf + total),
            (long)(cap - 1 - total)
        );
        if (r <= 0) {
            break;
        }
        total += (size_t)r;
    }
    out_buf[total] = '\0';
    (void)fipsos_sc1(FIPSOS_SYS_CLOSE, fd);
    return (long)total;
}

static inline long goose_audit_dir(
    const char *path,
    uint64_t *out_files,
    uint64_t *out_dirs,
    uint64_t *out_bytes
) {
    uint64_t files = 0;
    uint64_t dirs = 0;
    uint64_t bytes = 0;
    long dfd = fipsos_sc3(FIPSOS_SYS_OPENAT, FIPSOS_AT_FDCWD, (long)(uintptr_t)path, FIPSOS_O_RDONLY);
    if (dfd < 0) {
        if (out_files) *out_files = 0;
        if (out_dirs) *out_dirs = 0;
        if (out_bytes) *out_bytes = 0;
        return dfd;
    }

    uint8_t dirents[1024];
    for (;;) {
        long nread = fipsos_sc3(FIPSOS_SYS_GETDENTS64, dfd, (long)(uintptr_t)dirents, (long)sizeof(dirents));
        if (nread <= 0) {
            break;
        }
        long pos = 0;
        while (pos + 19 < nread) {
            uint16_t reclen = (uint16_t)dirents[pos + 16] | ((uint16_t)dirents[pos + 17] << 8);
            if (reclen < 20 || pos + (long)reclen > nread) {
                break;
            }
            uint8_t dtype = dirents[pos + 18];
            const char *name = (const char *)&dirents[pos + 19];
            int is_dot = (name[0] == '.' && name[1] == '\0');
            int is_dotdot = (name[0] == '.' && name[1] == '.' && name[2] == '\0');
            if (!is_dot && !is_dotdot && name[0] != '\0') {
                if (dtype == 4U) {
                    dirs++;
                } else {
                    files++;
                    char child[256];
                    size_t ci = 0;
                    size_t pi = 0;
                    while (path[pi] != '\0' && ci + 2 < sizeof(child)) {
                        child[ci++] = path[pi++];
                    }
                    if (ci > 0 && child[ci - 1] != '/' && ci + 1 < sizeof(child)) {
                        child[ci++] = '/';
                    }
                    size_t ni = 0;
                    while (name[ni] != '\0' && ci + 1 < sizeof(child)) {
                        child[ci++] = name[ni++];
                    }
                    child[ci] = '\0';
                    long cfd = fipsos_sc3(
                        FIPSOS_SYS_OPENAT,
                        FIPSOS_AT_FDCWD,
                        (long)(uintptr_t)child,
                        FIPSOS_O_RDONLY
                    );
                    if (cfd >= 0) {
                        uint8_t st[144];
                        for (size_t k = 0; k < sizeof(st); ++k) {
                            st[k] = 0;
                        }
                        if (fipsos_sc2(FIPSOS_SYS_FSTAT, cfd, (long)(uintptr_t)st) == 0) {
                            uint64_t fsz = 0;
                            for (size_t b = 0; b < 8; ++b) {
                                fsz |= ((uint64_t)st[48 + b]) << (b * 8U);
                            }
                            bytes += fsz;
                        }
                        (void)fipsos_sc1(FIPSOS_SYS_CLOSE, cfd);
                    }
                }
            }
            pos += (long)reclen;
        }
    }
    (void)fipsos_sc1(FIPSOS_SYS_CLOSE, dfd);
    if (out_files) *out_files = files;
    if (out_dirs) *out_dirs = dirs;
    if (out_bytes) *out_bytes = bytes;
    return 0;
}

#endif /* FIPSOS_GOOSE_RT_H */
