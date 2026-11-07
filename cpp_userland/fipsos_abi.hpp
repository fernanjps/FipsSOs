#pragma once
#include <stdint.h>
#include <stddef.h>

namespace fipsos {

constexpr uint64_t SYS_READ        = 0;
constexpr uint64_t SYS_WRITE       = 1;
constexpr uint64_t SYS_OPEN        = 2;
constexpr uint64_t SYS_CLOSE       = 3;
constexpr uint64_t SYS_MMAP        = 9;
constexpr uint64_t SYS_BRK         = 12;
constexpr uint64_t SYS_IOCTL       = 16;
constexpr uint64_t SYS_SCHED_YIELD = 24;
constexpr uint64_t SYS_GETPID      = 39;
constexpr uint64_t SYS_EXIT        = 60;

constexpr uint64_t FD_STDOUT       = 1;
constexpr uint64_t O_RDONLY        = 0;
constexpr uint64_t O_RDWR          = 2;
constexpr uint64_t PROT_READ       = 0x1;
constexpr uint64_t PROT_WRITE      = 0x2;
constexpr uint64_t MAP_SHARED      = 0x1;

constexpr uint64_t ACCEL_IOCTL_RUN_INFERENCE = 0xC0104102ULL;
constexpr uint64_t FB_IOCTL_PRESENT_RECT     = 0xC0104601ULL;
constexpr uint64_t FB_IOCTL_POLL_KEY         = 0xC0104602ULL;

inline int64_t syscall3(uint64_t num, uint64_t a0, uint64_t a1, uint64_t a2) noexcept {
    int64_t ret;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(num), "D"(a0), "S"(a1), "d"(a2)
        : "rcx", "r11", "memory"
    );
    return ret;
}

inline int64_t syscall6(
    uint64_t num,
    uint64_t a0,
    uint64_t a1,
    uint64_t a2,
    uint64_t a3,
    uint64_t a4,
    uint64_t a5
) noexcept {
    int64_t ret;
    register uint64_t r10 asm("r10") = a3;
    register uint64_t r8  asm("r8")  = a4;
    register uint64_t r9  asm("r9")  = a5;
    asm volatile(
        "syscall"
        : "=a"(ret)
        : "a"(num), "D"(a0), "S"(a1), "d"(a2), "r"(r10), "r"(r8), "r"(r9)
        : "rcx", "r11", "memory"
    );
    return ret;
}

inline int64_t sys_write(uint64_t fd, const char* buf, size_t len) noexcept {
    return syscall3(SYS_WRITE, fd, reinterpret_cast<uint64_t>(buf), static_cast<uint64_t>(len));
}

inline int64_t sys_read(uint64_t fd, void* buf, size_t len) noexcept {
    return syscall3(SYS_READ, fd, reinterpret_cast<uint64_t>(buf), static_cast<uint64_t>(len));
}

inline int64_t sys_open(const char* path_cstr, uint64_t flags) noexcept {
    return syscall3(SYS_OPEN, reinterpret_cast<uint64_t>(path_cstr), flags, 0);
}

inline int64_t sys_close(uint64_t fd) noexcept {
    return syscall3(SYS_CLOSE, fd, 0, 0);
}

inline uint64_t sys_brk(uint64_t new_addr) noexcept {
    return static_cast<uint64_t>(syscall3(SYS_BRK, new_addr, 0, 0));
}

inline int64_t sys_ioctl(uint64_t fd, uint64_t request, uint64_t arg) noexcept {
    return syscall3(SYS_IOCTL, fd, request, arg);
}

inline int64_t sys_mmap(
    uint64_t addr,
    size_t len,
    uint64_t prot,
    uint64_t flags,
    uint64_t fd,
    size_t offset
) noexcept {
    return syscall6(
        SYS_MMAP,
        addr,
        static_cast<uint64_t>(len),
        prot,
        flags,
        fd,
        static_cast<uint64_t>(offset)
    );
}

inline int64_t sys_sched_yield() noexcept {
    return syscall3(SYS_SCHED_YIELD, 0, 0, 0);
}

inline int64_t sys_getpid() noexcept {
    return syscall3(SYS_GETPID, 0, 0, 0);
}

[[noreturn]] inline void sys_exit(int32_t status) noexcept {
    syscall3(SYS_EXIT, static_cast<uint64_t>(status), 0, 0);
    __builtin_unreachable();
}

template <size_t N>
inline void print_literal(const char (&lit)[N]) noexcept {
    sys_write(FD_STDOUT, lit, N - 1);
}

} // namespace fipsos
