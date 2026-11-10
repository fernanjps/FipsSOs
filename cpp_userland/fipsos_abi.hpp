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
constexpr uint64_t SYS_EXECVE      = 59;
constexpr uint64_t SYS_EXIT        = 60;
constexpr uint64_t SYS_KILL        = 62;
constexpr uint64_t SYS_UNLINK      = 87;
constexpr uint64_t SYS_REBOOT      = 169;
constexpr uint64_t SYS_CLOCK_GETTIME = 228;
constexpr uint64_t SYS_SOCKET       = 41;
constexpr uint64_t SYS_CONNECT      = 42;
constexpr uint64_t SYS_SENDTO       = 44;
constexpr uint64_t SYS_RECVFROM     = 45;

constexpr int32_t AF_INET           = 2;
constexpr int32_t SOCK_STREAM       = 1;

constexpr int32_t CLOCK_REALTIME  = 0;
constexpr int32_t CLOCK_MONOTONIC = 1;

constexpr uint32_t LINUX_REBOOT_MAGIC1 = 0xFEE1DEAD;
constexpr uint32_t LINUX_REBOOT_MAGIC2 = 672274793;
constexpr uint32_t LINUX_REBOOT_CMD_RESTART = 0x01234567;
constexpr uint32_t LINUX_REBOOT_CMD_POWER_OFF = 0x4321FEDC;

constexpr uint64_t FD_STDOUT       = 1;
constexpr uint64_t O_RDONLY        = 0;
constexpr uint64_t O_WRONLY        = 1;
constexpr uint64_t O_RDWR          = 2;
constexpr uint64_t O_CREAT         = 0x40;
constexpr uint64_t O_TRUNC         = 0x200;
constexpr uint64_t O_APPEND        = 0x400;
constexpr uint64_t PROT_READ       = 0x1;
constexpr uint64_t PROT_WRITE      = 0x2;
constexpr uint64_t MAP_SHARED      = 0x1;

constexpr uint64_t FB_IOCTL_GET_INFO           = 0x4600ULL;
constexpr uint64_t FBIOGET_VSCREENINFO         = 0x4600ULL;
constexpr uint64_t FB_IOCTL_GET_VSCREENINFO    = 0xC0104600ULL;

struct fb_info {
    uint32_t width;
    uint32_t height;
    uint32_t pitch;
    uint32_t bpp;
};
constexpr uint64_t ACCEL_IOCTL_RUN_INFERENCE  = 0xC0104102ULL;
constexpr uint64_t FB_IOCTL_PRESENT_RECT      = 0xC0104601ULL;
constexpr uint64_t FB_IOCTL_POLL_KEY          = 0xC0104602ULL;
constexpr uint64_t FB_IOCTL_SET_DESKTOP_MODE  = 0xC0104603ULL;
constexpr uint64_t FB_IOCTL_FLIP_BUFFER       = 0xC0104604ULL;
constexpr uint64_t MOUSE_IOCTL_POLL_STATE     = 0xC0104D01ULL;
constexpr uint64_t MOUSE_IOCTL_STEP_SYNTHETIC = 0xC0104D02ULL;
constexpr uint64_t NET_IOCTL_PING_GATEWAY     = 0xC0104E01ULL;
constexpr uint64_t NET_IOCTL_HTTP_FETCH       = 0xC0104E02ULL;
constexpr uint64_t NET_IOCTL_BROWSER_FETCH_SAVE = 0xC0104E03ULL;
constexpr uint64_t DISK_IOCTL_SAVE_NOTE       = 0xC0104401ULL;
constexpr uint64_t DISK_IOCTL_DELETE_FILE     = 0xC0104402ULL;
constexpr uint64_t VFS_IOCTL_TASK_CONTROL     = 0xC0105003ULL;
constexpr uint64_t VFS_IOCTL_LIST_FILES       = 0xC0105601ULL;
constexpr uint64_t VFS_IOCTL_LIST_PROCESSES   = 0xC0105001ULL;
constexpr uint64_t VFS_IOCTL_HWMON_REPORT     = 0xC0105002ULL;
constexpr uint64_t VFS_IOCTL_SYS_TELEMETRY    = 0xC0105301ULL;

constexpr uint8_t MOUSE_BTN_LEFT   = 0x01;
constexpr uint8_t MOUSE_BTN_RIGHT  = 0x02;
constexpr uint8_t MOUSE_BTN_MIDDLE = 0x04;

struct Timespec {
    int64_t tv_sec;
    int64_t tv_nsec;
};

struct fb_var_screeninfo {
    uint32_t xres;
    uint32_t yres;
    uint32_t xres_virtual;
    uint32_t yres_virtual;
    uint32_t xoffset;
    uint32_t yoffset;
    uint32_t bits_per_pixel;
    uint32_t bytes_per_line;
    uint32_t reserved[8];
};

struct MousePacketState {
    uint16_t x;
    uint16_t y;
    uint8_t  buttons;
    uint32_t event_seq;
};

struct NetPingTelemetry {
    bool     link_up;
    bool     icmp_ok;
    uint16_t rtt_us;
    uint16_t tx_packets;
    uint16_t rx_packets;
};

struct SysTelemetryPacket {
    uint32_t ram_used_kib;
    uint32_t ram_total_kib;
    uint32_t ram_free_frames;
    uint32_t ram_used_frames;
    uint64_t irq0_ticks;
    uint32_t preemptions;
    uint32_t yields;
    uint32_t daemon_switches;
    uint32_t desktop_ticks;
    uint32_t net_tx_packets;
    uint32_t net_rx_packets;
    uint32_t net_rx_bytes;
    uint16_t vfs_node_count;
    uint8_t  net_link_up;
    uint8_t  pkg_downloaded;
    uint8_t  mac[6];
    uint16_t child_switches;
    uint64_t active_cr3;
    uint64_t shell_cr3;
    uint64_t infer_cr3;
    uint64_t cpp_cr3;
    // --- FASE v1.6: Telemetría SATA AHCI ---
    uint64_t disk_sectors_read;
    uint64_t disk_sectors_written;
    uint64_t disk_bytes_transferred;
    uint8_t  disk_present;
    uint8_t  disk_port;
    uint8_t  disk_reserved[6];
    // --- FASE v1.7: CMOS RTC, Sensores HW y Recuperación de Fallos ---
    uint16_t rtc_year;
    uint8_t  rtc_month;
    uint8_t  rtc_day;
    uint8_t  rtc_hour;
    uint8_t  rtc_min;
    uint8_t  rtc_sec;
    uint8_t  rtc_pad;
    uint32_t cpu_mhz;
    uint32_t temp_celsius;
    uint64_t user_faults_recovered;
    uint16_t physical_cores;
    uint16_t logical_threads;
    uint32_t driver_health_mask;
    // --- FASE v2.5: Hardware Real e1000e, SATA AHCI, SMP slots ---
    uint8_t  reserved_pad1[12];
    uint8_t  bg_task_state;     // 0=inactivo, 1=ejecutando, 2=terminado
    uint8_t  disk_files_count;
    uint8_t  smp_core0_slot;
    uint8_t  smp_core1_slot;
    uint8_t  reserved_pad2[16];
};
static_assert(sizeof(SysTelemetryPacket) == 192, "SysTelemetryPacket ABI size mismatch");

inline constexpr uint64_t pack_dirty_rect(uint16_t x, uint16_t y, uint16_t w, uint16_t h) noexcept {
    return static_cast<uint64_t>(x)
         | (static_cast<uint64_t>(y) << 16)
         | (static_cast<uint64_t>(w) << 32)
         | (static_cast<uint64_t>(h) << 48);
}

inline constexpr MousePacketState unpack_mouse_state(uint64_t raw) noexcept {
    return MousePacketState{
        static_cast<uint16_t>(raw & 0xFFFFULL),
        static_cast<uint16_t>((raw >> 16) & 0xFFFFULL),
        static_cast<uint8_t>((raw >> 32) & 0xFFULL),
        static_cast<uint32_t>((raw >> 40) & 0xFFFFFFULL)
    };
}

inline constexpr NetPingTelemetry unpack_net_telemetry(uint64_t raw) noexcept {
    return NetPingTelemetry{
        (raw & 0x1ULL) != 0ULL,
        (raw & 0x4ULL) != 0ULL,
        static_cast<uint16_t>((raw >> 8) & 0xFFFFULL),
        static_cast<uint16_t>((raw >> 24) & 0xFFFFULL),
        static_cast<uint16_t>((raw >> 40) & 0xFFFFULL)
    };
}

inline constexpr uint64_t pack_synthetic_mouse_step(int16_t dx, int16_t dy, uint8_t buttons) noexcept {
    return static_cast<uint64_t>(static_cast<uint16_t>(dx))
         | (static_cast<uint64_t>(static_cast<uint16_t>(dy)) << 16)
         | (static_cast<uint64_t>(buttons) << 32);
}

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

inline int64_t sys_kill(uint64_t pid, uint64_t sig) noexcept {
    return syscall3(SYS_KILL, pid, sig, 0);
}

inline int64_t sys_unlink(const char* path_cstr) noexcept {
    return syscall3(SYS_UNLINK, reinterpret_cast<uint64_t>(path_cstr), 0, 0);
}

inline int64_t sys_execve(const char* path_cstr) noexcept {
    return syscall3(SYS_EXECVE, reinterpret_cast<uint64_t>(path_cstr), 0, 0);
}

[[noreturn]] inline void sys_exit(int32_t status) noexcept {
    syscall3(SYS_EXIT, static_cast<uint64_t>(status), 0, 0);
    __builtin_unreachable();
}

inline int64_t sys_reboot(uint32_t magic1, uint32_t magic2, uint32_t cmd, uint64_t arg = 0) noexcept {
    return syscall6(SYS_REBOOT, magic1, magic2, cmd, arg, 0, 0);
}

inline int64_t sys_clock_gettime(int32_t clk_id, Timespec* tp) noexcept {
    return syscall3(SYS_CLOCK_GETTIME, static_cast<uint64_t>(clk_id), reinterpret_cast<uint64_t>(tp), 0);
}

struct InAddr {
    uint32_t s_addr;
};

struct SockAddrIn {
    int16_t  sin_family;
    uint16_t sin_port;
    InAddr   sin_addr;
    uint8_t  sin_zero[8];
};

inline int64_t sys_socket(int32_t domain, int32_t type, int32_t protocol) noexcept {
    return syscall3(SYS_SOCKET, static_cast<uint64_t>(domain), static_cast<uint64_t>(type), static_cast<uint64_t>(protocol));
}

inline int64_t sys_connect(int64_t sockfd, const SockAddrIn* addr, uint64_t addrlen) noexcept {
    return syscall3(SYS_CONNECT, static_cast<uint64_t>(sockfd), reinterpret_cast<uint64_t>(addr), addrlen);
}

inline int64_t sys_sendto(int64_t sockfd, const void* buf, size_t len, int32_t flags) noexcept {
    return syscall6(SYS_SENDTO, static_cast<uint64_t>(sockfd), reinterpret_cast<uint64_t>(buf), static_cast<uint64_t>(len), static_cast<uint64_t>(flags), 0, 0);
}

inline int64_t sys_recvfrom(int64_t sockfd, void* buf, size_t len, int32_t flags) noexcept {
    return syscall6(SYS_RECVFROM, static_cast<uint64_t>(sockfd), reinterpret_cast<uint64_t>(buf), static_cast<uint64_t>(len), static_cast<uint64_t>(flags), 0, 0);
}

template <size_t N>
inline void print_literal(const char (&lit)[N]) noexcept {
    sys_write(FD_STDOUT, lit, N - 1);
}

} // namespace fipsos
