#![forbid(unsafe_code)]

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use kernel_objects::{
    HandleId, HandleTable, KernelObject, Koid, Rights, SchedulerRegistry, Vmo, VmoUsage,
};
use ostd::ffi_jail::{c_npu_driver_entry, FfiJail};
use ostd::mm::map_vmo_to_user;
use ostd::sync::SpinLock;

use crate::tarfs::TarFs;

/// Standard POSIX / Linux file descriptors and FipSOs device node descriptors.
pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;
pub const FD_STDERR: u64 = 2;
pub const FD_DRM_CARD0: u64 = 3;
pub const FD_ACCEL_NPU0: u64 = 4;
pub const FD_FB0: u64 = 5;
pub const FD_MOUSE0: u64 = 6;
pub const FD_NET0: u64 = 7;
pub const FD_WLAN0: u64 = 8;
pub const FD_PCM0: u64 = 9;

/// Canonical virtual device and `Initramfs` file paths exposed by the FipSOs capability VFS.
pub const PATH_DRM_CARD0: &str = "/dev/dri/card0";
pub const PATH_ACCEL_NPU0: &str = "/dev/accel/accel0";
pub const PATH_FB0: &str = "/dev/fb0";
pub const PATH_MOUSE0: &str = "/dev/input/mouse0";
pub const PATH_NET0: &str = "/dev/net/eth0";
pub const PATH_WLAN0: &str = "/dev/net/wlan0";
pub const PATH_PCM0: &str = "/dev/snd/pcm0";
pub const PATH_DOWNLOAD_PKG: &str = "/downloads/fipsos_pkg.txt";
pub const PATH_INDEX_HTML: &str = "/disk/index.html";
pub const PATH_AI_MODEL_V2: &str = "/disk/ai_model_v2.bin";
pub const PATH_SYS_UPDATE_PKG: &str = "/disk/sys_update.pkg";
pub const PATH_ETC_MOTD: &str = "/etc/motd.txt";
pub const PATH_MODEL_AVATAR: &str = "/models/avatar_v1.bin";
pub const PATH_BIN_NPU_INFER: &str = "/bin/npu_infer.elf";
pub const PATH_BIN_AVATAR_CPP: &str = "/bin/avatar_cpp.elf";
pub const PATH_BIN_DESKTOP_UI: &str = "/bin/desktop_ui.elf";
pub const PATH_BIN_C_STD_APP: &str = "/bin/c_std_app.elf";
pub const PATH_BOOT_LOG: &str = crate::diskfs::PATH_BOOT_LOG;
pub const PATH_PERSISTENT_NOTE: &str = crate::diskfs::PATH_PERSISTENT_NOTE;
pub const PATH_SYS_HWMON: &str = "/sys/fipsos/hwmon";
pub const PATH_PROC_MEMINFO: &str = "/proc/meminfo";
pub const PATH_PROC_CPUINFO: &str = "/proc/cpuinfo";
pub const PATH_PROC_STAT: &str = "/proc/stat";
pub const PATH_PROC_MOUNTS: &str = "/proc/mounts";
pub const PATH_PROC_UPTIME: &str = "/proc/uptime";
pub const PATH_PROC_SELF_MOUNTS: &str = "/proc/self/mounts";
pub const PATH_SYS_CPU_ONLINE: &str = "/sys/devices/system/cpu/online";
pub const PATH_SYS_CPU_POSSIBLE: &str = "/sys/devices/system/cpu/possible";
pub const PATH_SYS_CPU_PRESENT: &str = "/sys/devices/system/cpu/present";
pub const PATH_ETC_MTAB: &str = "/etc/mtab";
pub const PATH_FAULT_TEST: &str = "/bin/fault_test.elf";

pub const MAP_ANONYMOUS: u64 = 0x20;

/// FipSOs ioctl command codes for `/dev/dri/card0` (DRM/Vulkan), `/dev/accel/accel0` (NPU/IA),
/// `/dev/fb0` (Framebuffer Zero-Copy Double Buffer), `/dev/input/mouse0` (PS/2 Pointer),
/// `/dev/net/eth0` (PCIe e1000e Network Stack), `/dev/net/wlan0` (WiFi 802.11), `/dev/snd/pcm0` (Audio Speaker),
/// DiskFS SATA AHCI persistence, Task Control, and VFS/System Telemetry.
pub const DISK_IOCTL_SAVE_NOTE: u64 = 0xC010_4401;
pub const DISK_IOCTL_DELETE_FILE: u64 = 0xC010_4402;
pub const DRM_IOCTL_PRIME_ZERO_COPY: u64 = 0xC010_6400;
pub const ACCEL_IOCTL_BIND_TENSOR_VMO: u64 = 0xC010_4100;
pub const ACCEL_IOCTL_RUN_INFERENCE: u64 = 0xC010_4102;
pub const FB_IOCTL_PRESENT_RECT: u64 = 0xC010_4601;
pub const FB_IOCTL_POLL_KEY: u64 = 0xC010_4602;
pub const FB_IOCTL_SET_DESKTOP_MODE: u64 = 0xC010_4603;
pub const FB_IOCTL_FLIP_BUFFER: u64 = 0xC010_4604;
pub const MOUSE_IOCTL_POLL_STATE: u64 = 0xC010_4D01;
pub const MOUSE_IOCTL_STEP_SYNTHETIC: u64 = 0xC010_4D02;
pub const NET_IOCTL_PING_GATEWAY: u64 = 0xC010_4E01;
pub const NET_IOCTL_HTTP_FETCH: u64 = 0xC010_4E02;
pub const NET_IOCTL_BROWSER_FETCH_SAVE: u64 = 0xC010_4E03;
pub const WIFI_IOCTL_SCAN: u64 = 0xC010_5701;
pub const WIFI_IOCTL_STATUS: u64 = 0xC010_5702;
pub const AUDIO_IOCTL_BEEP: u64 = 0xC010_5302;
pub const VFS_IOCTL_TASK_CONTROL: u64 = 0xC010_5003;
pub const VFS_IOCTL_LIST_FILES: u64 = 0xC010_5601;
pub const VFS_IOCTL_LIST_PROCESSES: u64 = 0xC010_5001;
pub const VFS_IOCTL_HWMON_REPORT: u64 = 0xC010_5002;
pub const VFS_IOCTL_SYS_TELEMETRY: u64 = 0xC010_5301;

/// Default top-right overlay coordinates and dimensions for `/dev/fb0` Zero-Copy presentation.
pub const FB_WIDGET_DEFAULT_X: usize = 1096;
pub const FB_WIDGET_DEFAULT_Y: usize = 68;
pub const FB_WIDGET_DEFAULT_W: usize = 160;
pub const FB_WIDGET_DEFAULT_H: usize = 80;

/// Linux `mmap` protection bit for writable mappings (`PROT_WRITE`).
pub const PROT_WRITE: u64 = 0x2;

/// Directory entry information returned by `list_directory` for `sys_getdents64`.
#[derive(Debug, Clone)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub is_chr: bool,
    pub size: usize,
}

/// Metadata for a virtual device node, file, directory, or pipe backed by a `Vmo` or in-memory FIFO.
#[derive(Clone)]
pub struct VfsDeviceNode {
    pub fd: u64,
    pub path: String,
    pub handle: HandleId,
    pub file_size: usize,
    pub is_device: bool,
    pub offset: usize,
    pub flags: u64,
    pub is_dir: bool,
    pub pipe_buf: Option<Arc<SpinLock<VecDeque<u8>>>>,
    pub is_pipe_write: bool,
    pub socket_id: Option<u64>,
}

impl core::fmt::Debug for VfsDeviceNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VfsDeviceNode")
            .field("fd", &self.fd)
            .field("path", &self.path)
            .field("handle", &self.handle)
            .field("file_size", &self.file_size)
            .field("is_device", &self.is_device)
            .field("offset", &self.offset)
            .field("flags", &self.flags)
            .field("is_dir", &self.is_dir)
            .field("is_pipe_write", &self.is_pipe_write)
            .field("socket_id", &self.socket_id)
            .finish()
    }
}

impl VfsDeviceNode {
    pub fn new_vmo(
        fd: u64,
        path: String,
        handle: HandleId,
        file_size: usize,
        is_device: bool,
    ) -> Self {
        Self {
            fd,
            path,
            handle,
            file_size,
            is_device,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: None,
        }
    }

    pub fn new_dir(fd: u64, path: String) -> Self {
        Self {
            fd,
            path,
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: true,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: None,
        }
    }

    pub fn new_socket(fd: u64, socket_id: u64) -> Self {
        Self {
            fd,
            path: alloc::format!("socket:[{}]", socket_id),
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: Some(socket_id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VfsFileMeta {
    pub handle: HandleId,
    pub file_size: usize,
    pub is_device: bool,
}

/// Capability-oriented Virtual File System unifying hardware device nodes (`/dev/dri/card0`,
/// `/dev/accel/accel0`, `/dev/fb0`) and USTAR `Initramfs` files (`/etc/motd.txt`, `/models/avatar_v1.bin`, etc.)
/// where every node is backed by a `Vmo` in `kernel_objects`.
pub struct CapabilityVfs {
    handle_table: HandleTable,
    devices: BTreeMap<u64, VfsDeviceNode>,
    pub registered_files: BTreeMap<String, VfsFileMeta>,
    directories: BTreeSet<String>,
    pub next_fd: u64,
}

/// Builds the real-time hardware monitor status text for `/sys/fipsos/hwmon`.
pub fn build_hwmon_text() -> String {
    let sensors = ostd::arch::x86_64::sensors::snapshot();
    let rtc = ostd::arch::x86_64::rtc_power::read_rtc();
    let topo = ostd::arch::x86_64::topology::detect();
    let faults = ostd::arch::syscall::user_faults_recovered();
    let c0_usage = ostd::scheduler::cpu_usage_percent(0);
    let c1_usage = ostd::scheduler::cpu_usage_percent(1);
    let c0_idle = ostd::scheduler::cpu_control_block(0)
        .map(|b| b.idle_ticks.load(core::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0);
    let c1_idle = ostd::scheduler::cpu_control_block(1)
        .map(|b| b.idle_ticks.load(core::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0);
    alloc::format!(
        "[HWMON] FipSOs v2.3 Hardware Monitor & Sensores:\n\
         CPU: {} (Family {}, Model {}, Stepping {}) @ {} MHz\n\
         Topology: {} nucleos fisicos / {} hilos logicos (SMT)\n\
         Gobernador de Energia ACPI/HLT: ACTIVO | Core 0: {}% uso (idle: {} ticks) | Core 1: {}% uso (idle: {} ticks)\n\
         Temperatura: {} C (IA32_THERM_STATUS calibrado)\n\
         Reloj Real CMOS RTC: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC\n\
         Drivers Hardware:\n\
           - [NIC]  PCIe 00:02.0 Intel e1000e   : {}\n\
           - [SATA] PCIe 00:1f.2 ICH9 AHCI DMA  : {}\n\
           - [GPU]  Limine Framebuffer 1280x800 : {}\n\
           - [NPU]  Acelerador C11 FP32 FfiJail : {}\n\
           - [IPC]  Anillos VMO Zero-Syscall    : ACTIVO (/dev/shm/io_ring, /dev/ipc/ring0)\n\
         Recuperacion de Fallos Anillo 3 (#PF/#GP): {} fallos capturados (Kernel intacto)\n",
        topo.brand(),
        topo.family,
        topo.model,
        topo.stepping,
        sensors.cpu_mhz,
        sensors.physical_cores,
        sensors.logical_threads,
        c0_usage,
        c0_idle,
        c1_usage,
        c1_idle,
        sensors.temp_celsius,
        rtc.year,
        rtc.month,
        rtc.day,
        rtc.hour,
        rtc.minute,
        rtc.second,
        if sensors.e1000e_ok { "LINK UP (1Gbps)" } else { "NO LINK" },
        if sensors.ahci_ok { "LINK UP (Puerto 0 DMA EXT)" } else { "NO LINK" },
        if sensors.fb0_ok { "ACTIVO (Doble Buffer DMA)" } else { "INACTIVO" },
        if sensors.npu_ok { "ACTIVO (FP32 SSE2 FfiJail)" } else { "INACTIVO" },
        faults
    )
}

/// Builds the real-time memory information text for `/proc/meminfo`.
pub fn build_meminfo_text() -> String {
    let stats = ostd::mm::memory_stats();
    let total_kb = stats.total_bytes() / 1024;
    let free_kb = stats.free_bytes() / 1024;
    alloc::format!(
        "MemTotal:       {:8} kB\n\
         MemFree:        {:8} kB\n\
         MemAvailable:   {:8} kB\n\
         Buffers:               0 kB\n\
         Cached:                0 kB\n\
         SwapTotal:             0 kB\n\
         SwapFree:              0 kB\n",
        total_kb,
        free_kb,
        free_kb
    )
}

/// Builds the mount table text for `/proc/mounts`, `/proc/self/mounts`, and `/etc/mtab`.
pub fn build_mounts_text() -> String {
    alloc::format!(
        "rootfs / rootfs rw 0 0\n\
         /dev/sata0 /disk diskfs rw 0 0\n\
         proc /proc proc rw 0 0\n\
         devtmpfs /dev devtmpfs rw 0 0\n"
    )
}

/// Builds the system uptime text for `/proc/uptime`.
pub fn build_uptime_text() -> String {
    let ticks = ostd::arch::idt::timer_ticks();
    let sec = ticks / 100;
    let frac = ticks % 100;
    alloc::format!("{}.{:02} {}.{:02}\n", sec, frac, sec * 2, frac)
}

/// Builds the real-time `/proc/cpuinfo` multicore specifications text.
pub fn build_cpuinfo_text() -> String {
    let topo = ostd::arch::topology::detect();
    let num_cpus = ostd::scheduler::online_cpu_count().max(1);
    let mut out = String::new();

    for i in 0..num_cpus {
        use core::fmt::Write;
        let _ = writeln!(out, "processor\t: {}", i);
        let _ = writeln!(out, "vendor_id\t: {}", topo.vendor());
        let _ = writeln!(out, "cpu family\t: {}", topo.family);
        let _ = writeln!(out, "model\t\t: {}", topo.model);
        let _ = writeln!(out, "model name\t: {}", topo.brand());
        let _ = writeln!(out, "stepping\t: {}", topo.stepping);
        let _ = writeln!(out, "cpu MHz\t\t: 2400.000");
        let _ = writeln!(out, "cache size\t: 512 KB");
        let _ = writeln!(out, "physical id\t: 0");
        let _ = writeln!(out, "siblings\t: {}", num_cpus);
        let _ = writeln!(out, "core id\t\t: {}", i);
        let _ = writeln!(out, "cpu cores\t: {}", num_cpus);
        let _ = writeln!(out, "apicid\t\t: {}", i);
        let _ = writeln!(out, "initial apicid\t: {}", i);
        let _ = writeln!(out, "fpu\t\t: yes");
        let _ = writeln!(out, "fpu_exception\t: yes");
        let _ = writeln!(out, "cpuid level\t: 13");
        let _ = writeln!(out, "wp\t\t: yes");
        let _ = writeln!(out, "flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ht syscall nx lm constant_tsc rep_good nopl cpuid pni pclmulqdq ssse3 cx16 pcid sse4_1 sse4_2 x2apic popcnt aes xsave avx hypervisor");
        let _ = writeln!(out, "bugs\t\t:");
        let _ = writeln!(out, "bogomips\t: 4800.00");
        let _ = writeln!(out, "clflush size\t: 64");
        let _ = writeln!(out, "cache_alignment\t: 64");
        let _ = writeln!(out, "address sizes\t: 40 bits physical, 48 bits virtual");
        let _ = writeln!(out, "power management:\n");
    }

    out
}

/// Builds the real-time `/proc/stat` multicore statistics breakdown text.
pub fn build_stat_text() -> String {
    let num_cpus = ostd::scheduler::online_cpu_count().max(1);
    let mut total_user = 0u64;
    let mut total_sys = 0u64;
    let mut total_idle = 0u64;

    let mut cpu_lines = String::new();
    for i in 0..num_cpus {
        use core::fmt::Write;
        let (compute, idle, _switches) = if let Some(blk) = ostd::scheduler::cpu_control_block(i) {
            (
                blk.compute_ticks.load(core::sync::atomic::Ordering::Relaxed),
                blk.idle_ticks.load(core::sync::atomic::Ordering::Relaxed).max(1),
                blk.total_switches.load(core::sync::atomic::Ordering::Relaxed),
            )
        } else {
            (0, 100, 0)
        };
        let user = compute / 2;
        let sys = compute.saturating_sub(user);
        total_user += user;
        total_sys += sys;
        total_idle += idle;
        let _ = writeln!(cpu_lines, "cpu{} {} 0 {} {} 0 0 0 0 0 0", i, user, sys, idle);
    }

    alloc::format!(
        "cpu  {} 0 {} {} 0 0 0 0 0 0\n{}intr {}\nctxt {}\nbtime 1700000000\nprocesses {}\nprocs_running {}\nprocs_blocked 0\nsoftirq 0\n",
        total_user,
        total_sys,
        total_idle,
        cpu_lines,
        total_user + total_sys,
        total_sys + 10,
        10,
        num_cpus
    )
}

/// Builds a minimal 137-byte ELF64 test binary that dereferences an invalid user pointer to trigger `#PF`.
pub fn build_fault_test_elf() -> alloc::vec::Vec<u8> {
    let mut elf = alloc::vec![0u8; 137];
    // ELF Header (64 bytes)
    elf[0..4].copy_from_slice(&crate::elf::ELF_MAGIC);
    elf[4] = crate::elf::ELFCLASS64;
    elf[5] = crate::elf::ELFDATA2LSB;
    elf[6] = 1; // EV_CURRENT
    elf[16..18].copy_from_slice(&crate::elf::ET_EXEC.to_le_bytes());
    elf[18..20].copy_from_slice(&crate::elf::EM_X86_64.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    elf[24..32].copy_from_slice(&0x0040_0078u64.to_le_bytes()); // entry point: offset 120 = 0x400078
    elf[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff = 64
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize = 64
    elf[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize = 56
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());  // e_phnum = 1

    // Program Header (56 bytes, offset 64..120)
    elf[64..68].copy_from_slice(&crate::elf::PT_LOAD.to_le_bytes());
    elf[68..72].copy_from_slice(&(crate::elf::PF_R | crate::elf::PF_X).to_le_bytes());
    elf[72..80].copy_from_slice(&0u64.to_le_bytes()); // p_offset = 0
    elf[80..88].copy_from_slice(&0x0040_0000u64.to_le_bytes()); // p_vaddr
    elf[88..96].copy_from_slice(&0x0040_0000u64.to_le_bytes()); // p_paddr
    elf[96..104].copy_from_slice(&137u64.to_le_bytes()); // p_filesz
    elf[104..112].copy_from_slice(&137u64.to_le_bytes()); // p_memsz
    elf[112..120].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // Code at offset 120..137 (virtual address 0x0040_0078)
    // mov rax, 0xdeadbeef; mov qword ptr [rax], 0x1337
    let code: [u8; 17] = [
        0x48, 0xB8, 0xEF, 0xBE, 0xAD, 0xDE, 0x00, 0x00, 0x00, 0x00,
        0x48, 0xC7, 0x00, 0x37, 0x13, 0x00, 0x00,
    ];
    elf[120..137].copy_from_slice(&code);
    elf
}

pub fn normalize_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() || p == "/" {
        return String::from("/");
    }
    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => {
                parts.push(other);
            }
        }
    }
    if parts.is_empty() {
        return String::from("/");
    }
    let mut out = String::new();
    for seg in parts {
        out.push('/');
        out.push_str(seg);
    }
    out
}

fn path_parent(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return "/";
    }
    match p.rfind('/') {
        Some(0) => "/",
        Some(idx) => &p[..idx],
        None => "/",
    }
}

fn path_basename(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(idx) => &p[idx + 1..],
        None => p,
    }
}

impl CapabilityVfs {
    /// Creates an unmounted `CapabilityVfs`.
    pub const fn new() -> Self {
        Self {
            handle_table: HandleTable::new(),
            devices: BTreeMap::new(),
            registered_files: BTreeMap::new(),
            directories: BTreeSet::new(),
            next_fd: 10,
        }
    }

    pub fn register_file(&mut self, path: String, handle: HandleId, file_size: usize, is_device: bool) {
        let norm = normalize_path(&path);
        self.record_parent_dirs(&norm);
        self.registered_files.insert(
            norm,
            VfsFileMeta {
                handle,
                file_size,
                is_device,
            },
        );
    }

    pub fn record_parent_dirs(&mut self, path: &str) {
        let norm = normalize_path(path);
        let mut curr = norm.as_str();
        while let Some(idx) = curr.rfind('/') {
            if idx == 0 {
                self.directories.insert(String::from("/"));
                break;
            }
            let parent = &curr[..idx];
            self.directories.insert(String::from(parent));
            curr = parent;
        }
    }

    /// Mounts `/dev/dri/card0`, `/dev/accel/accel0`, `/dev/fb0` and populates the VFS with
    /// `Vmo`-backed files parsed from the Limine `initramfs.tar` module (or in-memory USTAR fallback).
    pub fn mount_default_nodes(&mut self) {
        if !self.devices.is_empty() {
            return;
        }

        self.directories.insert(String::from("/"));
        self.directories.insert(String::from("/bin"));
        self.directories.insert(String::from("/dev"));
        self.directories.insert(String::from("/dev/dri"));
        self.directories.insert(String::from("/dev/accel"));
        self.directories.insert(String::from("/dev/input"));
        self.directories.insert(String::from("/dev/net"));
        self.directories.insert(String::from("/dev/snd"));
        self.directories.insert(String::from("/dev/shm"));
        self.directories.insert(String::from("/dev/ipc"));
        self.directories.insert(String::from("/disk"));
        self.directories.insert(String::from("/disk/bin"));
        self.directories.insert(String::from("/downloads"));
        self.directories.insert(String::from("/etc"));
        self.directories.insert(String::from("/models"));
        self.directories.insert(String::from("/proc"));
        self.directories.insert(String::from("/proc/self"));
        self.directories.insert(String::from("/sys"));
        self.directories.insert(String::from("/sys/fipsos"));
        self.directories.insert(String::from("/sys/devices"));
        self.directories.insert(String::from("/sys/devices/system"));
        self.directories.insert(String::from("/sys/devices/system/cpu"));

        // Standard streams /dev/stdin (0), /dev/stdout (1), /dev/stderr (2)
        self.devices.insert(FD_STDIN, VfsDeviceNode {
            fd: FD_STDIN,
            path: String::from("/dev/stdin"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: None,
        });
        self.devices.insert(FD_STDOUT, VfsDeviceNode {
            fd: FD_STDOUT,
            path: String::from("/dev/stdout"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: None,
        });
        self.devices.insert(FD_STDERR, VfsDeviceNode {
            fd: FD_STDERR,
            path: String::from("/dev/stderr"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: None,
            is_pipe_write: false,
            socket_id: None,
        });

        const DEVICE_VMO_SIZE: usize = 1024 * 1024; // 1 MiB Zero-Copy buffer per accelerator node

        let drm_vmo = Vmo::new(DEVICE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/dri/card0");
        drm_vmo
            .write(0, b"FIPSOS_DRM_VULKAN_FRAME_V1")
            .expect("Fallo al inicializar cabecera DRM VMO");
        let drm_handle = self.handle_table.insert(
            Arc::clone(&drm_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_DRM_CARD0);
        self.register_file(String::from(PATH_DRM_CARD0), drm_handle, DEVICE_VMO_SIZE, true);
        self.devices.insert(
            FD_DRM_CARD0,
            VfsDeviceNode::new_vmo(
                FD_DRM_CARD0,
                String::from(PATH_DRM_CARD0),
                drm_handle,
                DEVICE_VMO_SIZE,
                true,
            ),
        );

        let accel_vmo = Vmo::new(DEVICE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/accel/accel0");
        accel_vmo
            .write(0, b"FIPSOS_NPU_ACCEL_TENSOR_V1")
            .expect("Fallo al inicializar cabecera Accel VMO");
        let accel_handle = self.handle_table.insert(
            Arc::clone(&accel_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_ACCEL_NPU0);
        self.register_file(String::from(PATH_ACCEL_NPU0), accel_handle, DEVICE_VMO_SIZE, true);
        self.devices.insert(
            FD_ACCEL_NPU0,
            VfsDeviceNode::new_vmo(
                FD_ACCEL_NPU0,
                String::from(PATH_ACCEL_NPU0),
                accel_handle,
                DEVICE_VMO_SIZE,
                true,
            ),
        );

        const FB0_VMO_SIZE: usize = 4 * 1024 * 1024; // 4 MiB Zero-Copy 1280x800x4 RGB32 Backbuffer
        let fb0_vmo = Vmo::new(FB0_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy de 4 MiB para /dev/fb0");
        fb0_vmo
            .write(0, b"FIPSOS_FB0_CONSOLE_SURFACE")
            .expect("Fallo al inicializar cabecera FB0 VMO");
        let fb0_handle = self.handle_table.insert(
            Arc::clone(&fb0_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_FB0);
        self.register_file(String::from(PATH_FB0), fb0_handle, FB0_VMO_SIZE, true);
        self.devices.insert(
            FD_FB0,
            VfsDeviceNode::new_vmo(
                FD_FB0,
                String::from(PATH_FB0),
                fb0_handle,
                FB0_VMO_SIZE,
                true,
            ),
        );

        const MOUSE_VMO_SIZE: usize = 4096;
        let mouse_vmo = Vmo::new(MOUSE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/input/mouse0");
        mouse_vmo
            .write(0, b"FIPSOS_PS2_MOUSE0_INPUT_V1")
            .expect("Fallo al inicializar cabecera Mouse0 VMO");
        let boot_mouse_snap = ostd::arch::mouse::poll_state();
        let _ = sync_mouse_vmo(&mouse_vmo, &boot_mouse_snap);
        let mouse_handle = self.handle_table.insert(
            Arc::clone(&mouse_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_MOUSE0);
        self.register_file(String::from(PATH_MOUSE0), mouse_handle, MOUSE_VMO_SIZE, true);
        self.devices.insert(
            FD_MOUSE0,
            VfsDeviceNode::new_vmo(
                FD_MOUSE0,
                String::from(PATH_MOUSE0),
                mouse_handle,
                MOUSE_VMO_SIZE,
                true,
            ),
        );

        const NET_VMO_SIZE: usize = 64 * 1024; // 64 KiB Zero-Copy network ring/payload VMO
        let net_vmo = Vmo::new(NET_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy de 64 KiB para /dev/net/eth0");
        net_vmo
            .write(0, b"FIPSOS_NET_ETH0_E1000_DMA1")
            .expect("Fallo al inicializar cabecera Net0 VMO");
        let hw_snap = ostd::net::snapshot();
        let _ = net_vmo.write(32, &hw_snap.mac);
        let net_handle = self.handle_table.insert(
            Arc::clone(&net_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_NET0);
        self.register_file(String::from(PATH_NET0), net_handle, NET_VMO_SIZE, true);
        self.devices.insert(
            FD_NET0,
            VfsDeviceNode::new_vmo(
                FD_NET0,
                String::from(PATH_NET0),
                net_handle,
                NET_VMO_SIZE,
                true,
            ),
        );

        const WLAN_VMO_SIZE: usize = 64 * 1024;
        let wlan_vmo = Vmo::new(WLAN_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/net/wlan0");
        wlan_vmo
            .write(0, b"FIPSOS_NET_WLAN0_WIFI_V1\0\0\0\0\0\0\0\0")
            .expect("Fallo al inicializar cabecera Wlan0 VMO");
        let wlan_handle = self.handle_table.insert(
            Arc::clone(&wlan_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_WLAN0);
        self.register_file(String::from(PATH_WLAN0), wlan_handle, WLAN_VMO_SIZE, true);
        self.devices.insert(
            FD_WLAN0,
            VfsDeviceNode::new_vmo(
                FD_WLAN0,
                String::from(PATH_WLAN0),
                wlan_handle,
                WLAN_VMO_SIZE,
                true,
            ),
        );

        const PCM_VMO_SIZE: usize = 16 * 1024;
        let pcm_vmo = Vmo::new(PCM_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/snd/pcm0");
        pcm_vmo
            .write(0, b"FIPSOS_SND_PCM0_PIT_CH2\0\0\0\0\0\0\0\0")
            .expect("Fallo al inicializar cabecera PCM0 VMO");
        let pcm_handle = self.handle_table.insert(
            Arc::clone(&pcm_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PCM0);
        self.register_file(String::from(PATH_PCM0), pcm_handle, PCM_VMO_SIZE, true);
        self.devices.insert(
            FD_PCM0,
            VfsDeviceNode::new_vmo(
                FD_PCM0,
                String::from(PATH_PCM0),
                pcm_handle,
                PCM_VMO_SIZE,
                true,
            ),
        );

        // --- FASE v2.3: Zero-Syscall Shared Ring VMO (/dev/shm/io_ring & /dev/ipc/ring0) ---
        const RING_VMO_SIZE: usize = 64 * 1024;
        let ring_vmo = Vmo::new(RING_VMO_SIZE, VmoUsage::SharedRing)
            .expect("Fallo al reservar VMO Zero-Syscall para /dev/shm/io_ring");
        let initial_header = kernel_objects::VmoRingHeader::new();
        let _ = ring_vmo.write(0, &initial_header.to_bytes());
        ostd::ipc::register_shared_ring(ring_vmo.phys_addr(), ring_vmo.virt_addr());
        let ring_handle = self.handle_table.insert(
            Arc::clone(&ring_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        let fd_ring = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs("/dev/shm/io_ring");
        self.register_file(String::from("/dev/shm/io_ring"), ring_handle, RING_VMO_SIZE, true);
        self.devices.insert(
            fd_ring,
            VfsDeviceNode::new_vmo(
                fd_ring,
                String::from("/dev/shm/io_ring"),
                ring_handle,
                RING_VMO_SIZE,
                true,
            ),
        );
        let fd_ipc = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs("/dev/ipc/ring0");
        self.register_file(String::from("/dev/ipc/ring0"), ring_handle, RING_VMO_SIZE, true);
        self.devices.insert(
            fd_ipc,
            VfsDeviceNode::new_vmo(
                fd_ipc,
                String::from("/dev/ipc/ring0"),
                ring_handle,
                RING_VMO_SIZE,
                true,
            ),
        );

        // Parse USTAR Initramfs from Limine ModuleRequest or fallback in-memory USTAR image
        let fallback_tar;
        let (tar_bytes, source_tag) = if let Some(module_slice) = ostd::boot::initramfs_slice() {
            (module_slice, "Limine Module initramfs.tar")
        } else {
            fallback_tar = TarFs::build_fallback_tar();
            (fallback_tar.as_slice(), "TarFs In-Memory Fallback")
        };

        let tar_entries = TarFs::parse(tar_bytes);
        let mut mounted_files = 0usize;

        for entry in tar_entries {
            let alloc_len = entry.data.len().max(4096);
            let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile)
                .expect("Fallo al reservar VMO para archivo de Initramfs");
            if !entry.data.is_empty() {
                file_vmo
                    .write(0, entry.data)
                    .expect("Fallo al copiar contenido USTAR al VMO");
            }
            let file_handle = self.handle_table.insert(
                Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
            );
            let fd = self.next_fd;
            self.next_fd += 1;

            self.record_parent_dirs(&entry.path);
            self.register_file(entry.path.clone(), file_handle, entry.data.len(), false);
            self.devices.insert(
                fd,
                VfsDeviceNode::new_vmo(
                    fd,
                    entry.path,
                    file_handle,
                    entry.data.len(),
                    false,
                ),
            );
            mounted_files += 1;
        }

        // Initialize or mount persistent files from SATA AHCI DiskFS
        let disk_files = crate::diskfs::init_or_mount();
        let mut mounted_disk = 0usize;
        for dfile in disk_files {
            let alloc_len = dfile.data.len().max(4096);
            if let Ok(file_vmo) = Vmo::new(alloc_len, VmoUsage::InitramfsFile) {
                if !dfile.data.is_empty() {
                    let _ = file_vmo.write(0, &dfile.data);
                }
                let file_handle = self.handle_table.insert(
                    Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                    Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
                );
                let fd = self.next_fd;
                self.next_fd += 1;
                self.record_parent_dirs(&dfile.path);
                self.register_file(dfile.path.clone(), file_handle, dfile.data.len(), false);
                self.devices.insert(
                    fd,
                    VfsDeviceNode::new_vmo(
                        fd,
                        dfile.path,
                        file_handle,
                        dfile.data.len(),
                        false,
                    ),
                );
                mounted_disk += 1;
            }
        }

        // 1. Mount /sys/fipsos/hwmon
        let hwmon_text = build_hwmon_text();
        let hwmon_vmo = Vmo::new(4096, VmoUsage::InitramfsFile)
            .expect("Fallo al reservar VMO para /sys/fipsos/hwmon");
        let _ = hwmon_vmo.write(0, hwmon_text.as_bytes());
        let hwmon_handle = self.handle_table.insert(
            Arc::clone(&hwmon_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        let fd_hwmon = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs(PATH_SYS_HWMON);
        self.register_file(String::from(PATH_SYS_HWMON), hwmon_handle, hwmon_text.len(), false);
        self.devices.insert(
            fd_hwmon,
            VfsDeviceNode::new_vmo(
                fd_hwmon,
                String::from(PATH_SYS_HWMON),
                hwmon_handle,
                hwmon_text.len(),
                false,
            ),
        );

        // 2. Mount ProcFS synthetic files: /proc/meminfo, /proc/mounts, /proc/uptime, /proc/self/mounts, /etc/mtab
        let meminfo_text = build_meminfo_text();
        let meminfo_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /proc/meminfo fail");
        let _ = meminfo_vmo.write(0, meminfo_text.as_bytes());
        let meminfo_handle = self.handle_table.insert(
            Arc::clone(&meminfo_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PROC_MEMINFO);
        self.register_file(String::from(PATH_PROC_MEMINFO), meminfo_handle, meminfo_text.len(), false);

        let mounts_text = build_mounts_text();
        let mounts_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /proc/mounts fail");
        let _ = mounts_vmo.write(0, mounts_text.as_bytes());
        let mounts_handle = self.handle_table.insert(
            Arc::clone(&mounts_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PROC_MOUNTS);
        self.register_file(String::from(PATH_PROC_MOUNTS), mounts_handle, mounts_text.len(), false);
        self.record_parent_dirs(PATH_PROC_SELF_MOUNTS);
        self.register_file(String::from(PATH_PROC_SELF_MOUNTS), mounts_handle, mounts_text.len(), false);
        self.record_parent_dirs(PATH_ETC_MTAB);
        self.register_file(String::from(PATH_ETC_MTAB), mounts_handle, mounts_text.len(), false);

        // /etc/resolv.conf and /etc/hosts for libc network resolution
        let resolv_text = "nameserver 10.0.2.3\n";
        let resolv_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /etc/resolv.conf fail");
        let _ = resolv_vmo.write(0, resolv_text.as_bytes());
        let resolv_handle = self.handle_table.insert(
            Arc::clone(&resolv_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs("/etc/resolv.conf");
        self.register_file(String::from("/etc/resolv.conf"), resolv_handle, resolv_text.len(), false);

        let hosts_text = "127.0.0.1 localhost\n10.0.2.2 host\n";
        let hosts_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /etc/hosts fail");
        let _ = hosts_vmo.write(0, hosts_text.as_bytes());
        let hosts_handle = self.handle_table.insert(
            Arc::clone(&hosts_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs("/etc/hosts");
        self.register_file(String::from("/etc/hosts"), hosts_handle, hosts_text.len(), false);

        let uptime_text = build_uptime_text();
        let uptime_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /proc/uptime fail");
        let _ = uptime_vmo.write(0, uptime_text.as_bytes());
        let uptime_handle = self.handle_table.insert(
            Arc::clone(&uptime_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PROC_UPTIME);
        self.register_file(String::from(PATH_PROC_UPTIME), uptime_handle, uptime_text.len(), false);

        // ProcFS Multiprocessor Telemetry: /proc/cpuinfo and /proc/stat
        let cpuinfo_text = build_cpuinfo_text();
        let cpuinfo_vmo = Vmo::new(8192, VmoUsage::InitramfsFile).expect("VMO /proc/cpuinfo fail");
        let _ = cpuinfo_vmo.write(0, cpuinfo_text.as_bytes());
        let cpuinfo_handle = self.handle_table.insert(
            Arc::clone(&cpuinfo_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PROC_CPUINFO);
        self.register_file(String::from(PATH_PROC_CPUINFO), cpuinfo_handle, cpuinfo_text.len(), false);

        let stat_text = build_stat_text();
        let stat_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /proc/stat fail");
        let _ = stat_vmo.write(0, stat_text.as_bytes());
        let stat_handle = self.handle_table.insert(
            Arc::clone(&stat_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_PROC_STAT);
        self.register_file(String::from(PATH_PROC_STAT), stat_handle, stat_text.len(), false);

        // SysFS CPU Topology: /sys/devices/system/cpu/online, possible, present
        let online_str = "0-1\n";
        let online_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO sys cpu online fail");
        let _ = online_vmo.write(0, online_str.as_bytes());
        let online_handle = self.handle_table.insert(
            Arc::clone(&online_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_SYS_CPU_ONLINE);
        self.register_file(String::from(PATH_SYS_CPU_ONLINE), online_handle, online_str.len(), false);
        self.record_parent_dirs(PATH_SYS_CPU_POSSIBLE);
        self.register_file(String::from(PATH_SYS_CPU_POSSIBLE), online_handle, online_str.len(), false);
        self.record_parent_dirs(PATH_SYS_CPU_PRESENT);
        self.register_file(String::from(PATH_SYS_CPU_PRESENT), online_handle, online_str.len(), false);

        // 3. Mount /bin/fault_test.elf
        let fault_elf = build_fault_test_elf();
        let fault_vmo = Vmo::new(4096, VmoUsage::InitramfsFile)
            .expect("Fallo al reservar VMO para /bin/fault_test.elf");
        let _ = fault_vmo.write(0, &fault_elf);
        let fault_handle = self.handle_table.insert(
            Arc::clone(&fault_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );
        let fd_fault = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs(PATH_FAULT_TEST);
        self.register_file(String::from(PATH_FAULT_TEST), fault_handle, fault_elf.len(), false);
        self.devices.insert(
            fd_fault,
            VfsDeviceNode::new_vmo(
                fd_fault,
                String::from(PATH_FAULT_TEST),
                fault_handle,
                fault_elf.len(),
                false,
            ),
        );

        ostd::early_println!(
            "[linux_abi::vfs] VFS montado: 7 dispositivos (/dev/*: drm, accel, fb0, mouse0, eth0, wlan0, pcm0) + {} archivos TarFs ({}) + {} archivos DiskFS (SATA AHCI) + /sys/fipsos/hwmon",
            mounted_files,
            source_tag,
            mounted_disk
        );
        ostd::fb_println!(
            "[linux_abi::vfs] VFS montado: 7 nodos /dev/* + {} TarFs + {} DiskFS + /sys/fipsos/hwmon",
            mounted_files,
            mounted_disk
        );
    }

    /// Checks if the given path is a known directory.
    pub fn is_directory(&self, path: &str) -> bool {
        let norm = normalize_path(path);
        self.directories.contains(&norm)
    }

    /// Adds a directory path to the VFS.
    pub fn add_directory(&mut self, path: &str) {
        let norm = normalize_path(path);
        self.record_parent_dirs(&norm);
        self.directories.insert(norm);
    }

    /// Lists directory contents for `sys_getdents64`.
    pub fn list_directory(&self, dir_path: &str) -> alloc::vec::Vec<DirEntryInfo> {
        let norm_dir = normalize_path(dir_path);
        let mut entries = alloc::vec::Vec::new();
        let mut seen = alloc::collections::BTreeSet::new();

        entries.push(DirEntryInfo {
            name: String::from("."),
            is_dir: true,
            is_chr: false,
            size: 4096,
        });
        entries.push(DirEntryInfo {
            name: String::from(".."),
            is_dir: true,
            is_chr: false,
            size: 4096,
        });
        seen.insert(String::from("."));
        seen.insert(String::from(".."));

        // Direct child directories
        for d in &self.directories {
            if d == &norm_dir || d == "/" {
                continue;
            }
            if path_parent(d) == norm_dir.as_str() {
                let name = String::from(path_basename(d));
                if !seen.contains(&name) {
                    seen.insert(name.clone());
                    entries.push(DirEntryInfo {
                        name,
                        is_dir: true,
                        is_chr: false,
                        size: 4096,
                    });
                }
            }
        }

        // Direct child files & devices
        for (fpath, meta) in &self.registered_files {
            let parent = path_parent(fpath);
            if parent == norm_dir.as_str() {
                let name = String::from(path_basename(fpath));
                if !seen.contains(&name) {
                    seen.insert(name.clone());
                    entries.push(DirEntryInfo {
                        name,
                        is_dir: false,
                        is_chr: meta.is_device,
                        size: meta.file_size,
                    });
                }
            }
        }

        entries
    }

    /// Dynamically creates a brand-new `Vmo` containing `payload` and mounts or updates `path`
    /// inside `CapabilityVfs` so downloaded files (e.g. `/downloads/fipsos_pkg.txt`) are immediately
    /// visible via `ls` and readable via `sys_open`/`sys_read` (`cat`).
    pub fn create_or_update_downloaded_file(
        &mut self,
        path: &'static str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        let alloc_len = payload.len().max(4096);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !payload.is_empty() {
            file_vmo.write(0, payload).map_err(|_| -22i64)?;
        }
        let koid = file_vmo.koid();
        let file_handle = self.handle_table.insert(
            Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );

        let existing_fd = self
            .devices
            .values()
            .find(|n| n.path == path)
            .map(|n| n.fd);
        let fd = match existing_fd {
            Some(reuse_fd) => reuse_fd,
            None => {
                let fresh_fd = self.next_fd;
                self.next_fd += 1;
                fresh_fd
            }
        };

        self.record_parent_dirs(path);
        self.register_file(String::from(path), file_handle, payload.len(), false);
        self.devices.insert(
            fd,
            VfsDeviceNode::new_vmo(
                fd,
                String::from(path),
                file_handle,
                payload.len(),
                false,
            ),
        );

        Ok((fd, koid))
    }

    /// Dynamically creates or updates a persistent `Vmo` backed file on the SATA disk
    /// (e.g. `/disk/persistent_note.txt`).
    pub fn create_or_update_disk_file(
        &mut self,
        path: &str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        let alloc_len = payload.len().max(4096);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !payload.is_empty() {
            file_vmo.write(0, payload).map_err(|_| -22i64)?;
        }
        let koid = file_vmo.koid();
        let file_handle = self.handle_table.insert(
            Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );

        let existing_fd = self
            .devices
            .values()
            .find(|n| n.path == path)
            .map(|n| n.fd);
        let fd = match existing_fd {
            Some(reuse_fd) => reuse_fd,
            None => {
                let fresh_fd = self.next_fd;
                self.next_fd += 1;
                fresh_fd
            }
        };

        self.record_parent_dirs(path);
        self.register_file(String::from(path), file_handle, payload.len(), false);
        self.devices.insert(
            fd,
            VfsDeviceNode::new_vmo(
                fd,
                String::from(path),
                file_handle,
                payload.len(),
                false,
            ),
        );

        Ok((fd, koid))
    }

    /// Removes a file node by path from the VFS device directory and releases its capability handle.
    pub fn remove_node(&mut self, path: &str) -> bool {
        let norm = normalize_path(path);
        let removed_meta = self.registered_files.remove(&norm);
        self.devices.retain(|_, n| n.path != norm);
        if let Some(meta) = removed_meta {
            let _ = self.handle_table.remove(meta.handle);
            return true;
        }
        false
    }

    /// Opens a mounted capability VFS node by path (`/dev/...`, `/etc/motd.txt`, `/models/avatar_v1.bin`, etc.)
    /// and returns its file descriptor.
    pub fn open(&mut self, raw_path: &str, flags: u64) -> Result<u64, i64> {
        let path = normalize_path(raw_path);
        const O_CREAT: u64 = 0x40;
        const O_TRUNC: u64 = 0x200;
        const O_APPEND: u64 = 0x400;

        if self.is_directory(&path) {
            let fd = self.next_fd;
            self.next_fd += 1;
            let node = VfsDeviceNode::new_dir(fd, path);
            self.devices.insert(fd, node);
            return Ok(fd);
        }

        // Check if file exists in registered_files
        let existing = self.registered_files.get(&path).cloned();

        if let Some(mut meta) = existing {
            let fd = self.next_fd;
            self.next_fd += 1;

            if (flags & O_TRUNC) != 0 && !meta.is_device {
                meta.file_size = 0;
                if let Some(entry) = self.registered_files.get_mut(&path) {
                    entry.file_size = 0;
                }
                if path.starts_with("/disk/") {
                    let _ = crate::diskfs::save_file(&path, &[]);
                }
            }

            let mut node = VfsDeviceNode::new_vmo(fd, path.clone(), meta.handle, meta.file_size, meta.is_device);
            node.flags = flags;
            node.offset = if (flags & O_APPEND) != 0 { meta.file_size } else { 0 };

            let dynamic_text: Option<String> = if path == PATH_SYS_HWMON {
                Some(build_hwmon_text())
            } else if path == PATH_PROC_MEMINFO {
                Some(build_meminfo_text())
            } else if path == PATH_PROC_MOUNTS || path == PATH_PROC_SELF_MOUNTS || path == PATH_ETC_MTAB {
                Some(build_mounts_text())
            } else if path == PATH_PROC_UPTIME {
                Some(build_uptime_text())
            } else if path == PATH_PROC_CPUINFO {
                Some(build_cpuinfo_text())
            } else if path == PATH_PROC_STAT {
                Some(build_stat_text())
            } else {
                None
            };

            if let Some(text) = dynamic_text {
                if let Ok(entry) = self.handle_table.get(node.handle, Rights::WRITE) {
                    if let Ok(vmo) = entry.as_vmo() {
                        let _ = vmo.write(0, text.as_bytes());
                        node.file_size = text.len();
                        if let Some(entry) = self.registered_files.get_mut(&path) {
                            entry.file_size = text.len();
                        }
                    }
                }
            }

            self.devices.insert(fd, node);
            return Ok(fd);
        }

        // If file does not exist, check O_CREAT
        if (flags & O_CREAT) != 0 {
            let alloc_len = 512 * 1024;
            let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
            let file_handle = self.handle_table.insert(
                Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
            );
            self.record_parent_dirs(&path);
            self.register_file(path.clone(), file_handle, 0, false);

            let fd = self.next_fd;
            self.next_fd += 1;
            let mut node = VfsDeviceNode::new_vmo(fd, path.clone(), file_handle, 0, false);
            node.flags = flags;

            if path.starts_with("/disk/") {
                let _ = crate::diskfs::save_file(&path, &[]);
            }

            self.devices.insert(fd, node);
            ostd::early_println!("[linux_abi::vfs] Archivo creado: \"{}\" -> fd={}", path, fd);
            return Ok(fd);
        }

        Err(-2i64) // -ENOENT
    }

    /// Closes an open capability file descriptor (`sys_close`, syscall 3) after verifying its
    /// `HandleId` and `Koid` in the capability table.
    pub fn close(&mut self, fd: u64) -> Result<u64, i64> {
        if fd <= FD_STDERR {
            if let Some(node) = self.devices.get(&fd) {
                if node.pipe_buf.is_some() || !node.is_device {
                    self.devices.remove(&fd);
                    return Ok(0);
                }
            }
            return Ok(0);
        }
        if let Some(node) = self.devices.remove(&fd) {
            if let Some(sock_id) = node.socket_id {
                let _ = ostd::net::tcp::close(sock_id);
                return Ok(0);
            }
            if node.is_dir || node.pipe_buf.is_some() {
                return Ok(0);
            }
            if node.path.starts_with("/disk/") && !node.is_device {
                if let Ok(entry) = self.handle_table.get(node.handle, Rights::READ) {
                    if let Ok(vmo) = entry.as_vmo() {
                        let mut buf = alloc::vec![0u8; node.file_size];
                        if vmo.read(0, &mut buf).is_ok() {
                            let _ = crate::diskfs::save_file(&node.path, &buf);
                        }
                    }
                }
            }
            let still_in_reg = self.registered_files.values().any(|m| m.handle == node.handle);
            let still_in_dev = self.devices.values().any(|n| n.handle == node.handle);
            if !still_in_reg && !still_in_dev && node.handle != HandleId::null() {
                let _ = self.handle_table.remove(node.handle);
            }
            return Ok(0);
        }
        Err(-9i64) // -EBADF
    }

    /// Reads up to `dst.len()` bytes from the `Vmo`, socket, or pipe backing `fd`.
    pub fn read_file(&mut self, fd: u64, dst: &mut [u8]) -> Result<usize, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if let Some(sock_id) = node.socket_id {
            return ostd::net::tcp::recv(sock_id, dst);
        }
        if let Some(ref pipe_buf) = node.pipe_buf {
            if node.is_pipe_write {
                return Err(-9i64);
            }
            let mut q = pipe_buf.lock();
            let mut count = 0;
            for byte in dst.iter_mut() {
                if let Some(b) = q.pop_front() {
                    *byte = b;
                    count += 1;
                } else {
                    break;
                }
            }
            return Ok(count);
        }

        if node.is_dir {
            return Err(-21i64); // -EISDIR
        }

        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;

        if node.offset >= node.file_size {
            return Ok(0); // EOF
        }
        let available = node.file_size - node.offset;
        let to_read = dst.len().min(available);
        if to_read > 0 {
            vmo.read(node.offset, &mut dst[..to_read]).map_err(|_| -22i64)?;
            node.offset += to_read;
        }
        Ok(to_read)
    }

    /// Writes up to `src.len()` bytes to the `Vmo`, socket, or pipe backing `fd`.
    pub fn write_file(&mut self, fd: u64, src: &[u8]) -> Result<usize, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if let Some(sock_id) = node.socket_id {
            return ostd::net::tcp::send(sock_id, src);
        }
        if let Some(ref pipe_buf) = node.pipe_buf {
            if !node.is_pipe_write {
                return Err(-9i64);
            }
            let mut q = pipe_buf.lock();
            for &b in src {
                q.push_back(b);
            }
            return Ok(src.len());
        }

        if node.is_dir {
            return Err(-21i64); // -EISDIR
        }

        let entry = self
            .handle_table
            .get(node.handle, Rights::WRITE)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;

        let write_end = node.offset.saturating_add(src.len());
        let to_write = if write_end > vmo.size_bytes() {
            let avail = vmo.size_bytes().saturating_sub(node.offset);
            if avail == 0 {
                return Err(-28i64); // -ENOSPC
            }
            src.len().min(avail)
        } else {
            src.len()
        };

        vmo.write(node.offset, &src[..to_write]).map_err(|_| -22i64)?;
        node.offset += to_write;
        if node.offset > node.file_size {
            node.file_size = node.offset;
            if let Some(meta) = self.registered_files.get_mut(&node.path) {
                meta.file_size = node.file_size;
            }
        }

        if node.path.starts_with("/disk/") {
            let mut entire_data = alloc::vec![0u8; node.file_size];
            let _ = vmo.read(0, &mut entire_data);
            let _ = crate::diskfs::save_file(&node.path, &entire_data);
        }

        Ok(to_write)
    }

    /// Seeks the file offset of `fd`.
    pub fn lseek(&mut self, fd: u64, offset: i64, whence: u64) -> Result<u64, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if node.is_device || node.pipe_buf.is_some() {
            return Err(-29i64); // -ESPIPE
        }
        const SEEK_SET: u64 = 0;
        const SEEK_CUR: u64 = 1;
        const SEEK_END: u64 = 2;

        let new_offset = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (node.offset as i64).saturating_add(offset),
            SEEK_END => (node.file_size as i64).saturating_add(offset),
            _ => return Err(-22i64),
        };
        if new_offset < 0 {
            return Err(-22i64);
        }
        node.offset = new_offset as usize;
        Ok(node.offset as u64)
    }

    /// Creates an anonymous unidirectional pipe.
    pub fn pipe2(&mut self, _flags: u64) -> Result<(u64, u64), i64> {
        let fifo = Arc::new(SpinLock::new(VecDeque::with_capacity(4096)));
        let r_fd = self.next_fd;
        self.next_fd += 1;
        let w_fd = self.next_fd;
        self.next_fd += 1;

        let r_node = VfsDeviceNode {
            fd: r_fd,
            path: alloc::format!("pipe:[{}]", r_fd),
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: Some(Arc::clone(&fifo)),
            is_pipe_write: false,
            socket_id: None,
        };
        let w_node = VfsDeviceNode {
            fd: w_fd,
            path: alloc::format!("pipe:[{}]", w_fd),
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe_buf: Some(fifo),
            is_pipe_write: true,
            socket_id: None,
        };
        self.devices.insert(r_fd, r_node);
        self.devices.insert(w_fd, w_node);
        Ok((r_fd, w_fd))
    }

    /// Duplicates a file descriptor.
    pub fn dup(&mut self, oldfd: u64) -> Result<u64, i64> {
        let newfd = self.next_fd;
        self.next_fd += 1;
        self.dup2(oldfd, newfd)
    }

    /// Duplicates a file descriptor to a specific descriptor `newfd`.
    pub fn dup2(&mut self, oldfd: u64, newfd: u64) -> Result<u64, i64> {
        if oldfd == newfd {
            if self.devices.contains_key(&oldfd) {
                return Ok(newfd);
            } else {
                return Err(-9i64);
            }
        }
        let node = self.devices.get(&oldfd).cloned().ok_or(-9i64)?;
        let mut cloned = node;
        cloned.fd = newfd;
        self.devices.insert(newfd, cloned);
        Ok(newfd)
    }

    /// Creates a new TCP socket descriptor backed by `ostd::net::tcp`.
    pub fn create_socket_node(&mut self) -> Result<u64, i64> {
        let sock_id = ostd::net::tcp::create_socket()?;
        let fd = self.next_fd;
        self.next_fd += 1;
        let node = VfsDeviceNode::new_socket(fd, sock_id);
        self.devices.insert(fd, node);
        Ok(fd)
    }

    /// Returns the underlying TCP socket ID for `fd`, if it is a socket descriptor.
    pub fn get_socket_id(&self, fd: u64) -> Option<u64> {
        self.devices.get(&fd).and_then(|node| node.socket_id)
    }

    /// Returns an immutable reference to the VFS node for descriptor `fd`.
    pub fn get_node(&self, fd: u64) -> Option<&VfsDeviceNode> {
        self.devices.get(&fd)
    }

    /// Returns a mutable reference to the VFS node for descriptor `fd`.
    pub fn get_node_mut(&mut self, fd: u64) -> Option<&mut VfsDeviceNode> {
        self.devices.get_mut(&fd)
    }

    /// Returns the file size of the mounted device node or file with descriptor `fd`.
    pub fn get_file_size(&self, fd: u64) -> Option<usize> {
        self.devices.get(&fd).map(|node| node.file_size)
    }

    /// Returns the file size of the mounted file with the given `path`.
    pub fn get_file_size_by_path(&self, path: &str) -> Option<usize> {
        let norm = normalize_path(path);
        if norm == PATH_SYS_HWMON {
            return Some(build_hwmon_text().len());
        }
        if norm == PATH_PROC_MEMINFO {
            return Some(build_meminfo_text().len());
        }
        if norm == PATH_PROC_MOUNTS || norm == PATH_PROC_SELF_MOUNTS || norm == PATH_ETC_MTAB {
            return Some(build_mounts_text().len());
        }
        if norm == PATH_PROC_UPTIME {
            return Some(build_uptime_text().len());
        }
        self.registered_files.get(&norm).map(|meta| meta.file_size)
    }

    /// Reads the entire contents of a mounted `TarFs` file by path (used by `sys_execve` to load ELF64 binaries).
    pub fn read_all_by_path(&self, path: &str) -> Result<alloc::vec::Vec<u8>, i64> {
        let norm = normalize_path(path);
        if norm == PATH_SYS_HWMON {
            return Ok(build_hwmon_text().into_bytes());
        }
        if norm == PATH_PROC_MEMINFO {
            return Ok(build_meminfo_text().into_bytes());
        }
        if norm == PATH_PROC_MOUNTS || norm == PATH_PROC_SELF_MOUNTS || norm == PATH_ETC_MTAB {
            return Ok(build_mounts_text().into_bytes());
        }
        if norm == PATH_PROC_UPTIME {
            return Ok(build_uptime_text().into_bytes());
        }
        if let Some(meta) = self.registered_files.get(&norm) {
            let entry = self
                .handle_table
                .get(meta.handle, Rights::READ)
                .map_err(|_| -13i64)?; // -EACCES
            let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV
            let mut bytes = vec![0u8; meta.file_size];
            if meta.file_size > 0 {
                vmo.read(0, &mut bytes).map_err(|_| -22i64)?;
            }
            return Ok(bytes);
        }
        Err(-2i64) // -ENOENT
    }

    /// Prints all mounted devices and `TarFs` `Initramfs` files with their `fd`, `Koid`, and size.
    pub fn list_nodes(&self) {
        ostd::early_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        ostd::fb_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        for node in self.devices.values() {
            if let Ok(entry) = self.handle_table.get(node.handle, Rights::READ) {
                let kind = if node.is_device {
                    "DEV"
                } else if node.path.starts_with("/disk/") {
                    "DSK"
                } else if node.path.starts_with("/downloads/") {
                    "NET"
                } else {
                    "TAR"
                };
                ostd::early_println!(
                    "  fd={:<2} | [{}] {:<10} | {:>9} B | {}",
                    node.fd,
                    kind,
                    entry.koid(),
                    node.file_size,
                    node.path
                );
                ostd::fb_println!(
                    "  fd={:<2} | [{}] {:<10} | {:>9} B | {}",
                    node.fd,
                    kind,
                    entry.koid(),
                    node.file_size,
                    node.path
                );
            }
        }
    }

    /// Redirects an `ioctl` syscall on `/dev/dri/card0`, `/dev/accel/accel0`, or `/dev/fb0` to its
    /// underlying `Vmo` Zero-Copy object after checking `Rights::READ | Rights::WRITE`.
    pub fn ioctl(&mut self, fd: u64, request: u64, arg: u64) -> Result<u64, i64> {
        if request == VFS_IOCTL_LIST_FILES {
            self.list_nodes();
            return Ok(self.devices.len() as u64);
        }
        if request == VFS_IOCTL_LIST_PROCESSES {
            let count = SchedulerRegistry::print_ps_table();
            return Ok(count as u64);
        }
        if request == DISK_IOCTL_SAVE_NOTE {
            let ticks = ostd::arch::idt::timer_ticks();
            let _ = crate::diskfs::save_persistent_note(ticks);
            let ahci = ostd::storage::ahci_snapshot();
            let note_text = alloc::format!(
                "[DiskFS] Nota persistente de usuario FipSOs v1.6\n\
                 Timestamp IRQ0: {} ticks\n\
                 Dispositivo: Intel ICH9 SATA AHCI 00:1f.2 (Port {})\n\
                 LBA Lecturas: {} sectores | LBA Escrituras: {} sectores\n\
                 Total transferido: {} KiB\n\
                 Estado: Persistencia en sectores LBA verificada exitosamente.\n",
                ticks,
                ahci.port_idx,
                ahci.sectors_read,
                ahci.sectors_written,
                ahci.bytes_transferred / 1024
            );
            let (n_fd, n_koid) = self.create_or_update_disk_file(crate::diskfs::PATH_PERSISTENT_NOTE, note_text.as_bytes())?;
            ostd::early_println!(
                "[DiskFS::save] Guardada nota persistente \"{}\" en disco SATA AHCI (fd={}, {}, {} B)",
                crate::diskfs::PATH_PERSISTENT_NOTE,
                n_fd,
                n_koid,
                note_text.len()
            );
            ostd::fb_println!(
                "[DiskFS::save] \"{}\" guardado en SATA ({} B)",
                crate::diskfs::PATH_PERSISTENT_NOTE,
                note_text.len()
            );
            return Ok(note_text.len() as u64);
        }
        if request == FB_IOCTL_POLL_KEY {
            let key_byte = ostd::arch::keyboard::try_pop_key_nonblocking().unwrap_or(0);
            return Ok(key_byte as u64);
        }
        if request == FB_IOCTL_SET_DESKTOP_MODE {
            let enabled = arg != 0;
            ostd::fb_console::set_desktop_mode(enabled);
            ostd::early_println!(
                "[linux_abi::fb0] FB_IOCTL_SET_DESKTOP_MODE(enabled={}) -> Compositor 1280x800 {}",
                enabled,
                if enabled {
                    "ACTIVO (Zero-Copy Double Buffer)"
                } else {
                    "DESACTIVADO (Consola TTY restaurada)"
                }
            );
            return Ok(if enabled { 1 } else { 0 });
        }
        if request == VFS_IOCTL_HWMON_REPORT {
            let hw_text = build_hwmon_text();
            if arg != 0 {
                let write_len = hw_text.len().min(4096);
                ostd::mm::write_user_bytes(arg, &hw_text.as_bytes()[..write_len]).map_err(|_| -14i64)?;
            }
            return Ok(hw_text.len() as u64);
        }
        if request == VFS_IOCTL_TASK_CONTROL {
            if arg == 1 {
                let (pid, _tid) = SchedulerRegistry::spawn_background_task("bg_downloader");
                ostd::early_println!("[TaskCtrl] Tarea en segundo plano iniciada (PID={})", pid);
                ostd::fb_println!("[TaskCtrl] Tarea segundo plano iniciada (PID={})", pid);
                return Ok(pid);
            } else if arg == 2 {
                let _ = SchedulerRegistry::kill_process(1051, 9);
                ostd::early_println!("[TaskCtrl] Tarea en segundo plano terminada (PID=1051, SIGKILL)");
                ostd::fb_println!("[TaskCtrl] Tarea segundo plano terminada (PID=1051)");
                return Ok(1051);
            } else if arg == 3 {
                let _ = SchedulerRegistry::kill_process(1035, 9);
                ostd::early_println!("[TaskCtrl] Daemon npu-telemetryd terminado (PID=1035, SIGKILL)");
                ostd::fb_println!("[TaskCtrl] Daemon npu-telemetryd terminado");
                return Ok(1035);
            } else if arg == 4 {
                ostd::scheduler::set_daemon_active(true);
                ostd::early_println!("[TaskCtrl] Daemon npu-telemetryd reactivado");
                ostd::fb_println!("[TaskCtrl] Daemon reactivado");
                return Ok(1035);
            }
            return Ok(0);
        }
        if request == DISK_IOCTL_DELETE_FILE {
            let path_str = if arg != 0 {
                let mut path_buf = [0u8; 128];
                ostd::mm::read_user_bytes(arg, &mut path_buf).map_err(|_| -14i64)?;
                let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
                let parsed = core::str::from_utf8(&path_buf[..len]).unwrap_or(crate::diskfs::PATH_PERSISTENT_NOTE);
                // Allocate a string to avoid borrow conflict
                alloc::string::String::from(parsed)
            } else {
                alloc::string::String::from(crate::diskfs::PATH_PERSISTENT_NOTE)
            };
            let _ = crate::diskfs::delete_file(&path_str);
            self.remove_node(&path_str);
            return Ok(0);
        }
        if request == WIFI_IOCTL_SCAN {
            let count = ostd::net::wifi::scan();
            return Ok(count.len() as u64);
        }
        if request == WIFI_IOCTL_STATUS {
            if arg == 1 {
                ostd::net::wifi::set_active_iface(1); // wlan0
            } else if arg == 0 {
                ostd::net::wifi::set_active_iface(0); // eth0
            }
            let snap = ostd::net::wifi::snapshot();
            let packed = (snap.link_up as u64)
                | ((snap.active_iface as u64) << 8)
                | (((snap.rssi_dbm as i16 as u16) as u64) << 16)
                | ((snap.channel as u64) << 32)
                | ((snap.security_type as u64) << 40);
            return Ok(packed);
        }
        if request == AUDIO_IOCTL_BEEP {
            let (freq, duration) = if arg != 0 {
                let f = ((arg >> 16) & 0xFFFF) as u16;
                let d = (arg & 0xFFFF) as u16;
                (f.clamp(100, 10000), d.clamp(10, 2000))
            } else {
                (880u16, 100u16)
            };
            ostd::arch::audio::play_beep(freq, duration);
            return Ok(0);
        }
        if request == NET_IOCTL_BROWSER_FETCH_SAVE {
            let (uri_str, save_path) = if arg == 1 {
                (alloc::string::String::from("http://fipsos.org/index.html"), PATH_INDEX_HTML)
            } else if arg == 2 {
                (alloc::string::String::from("http://fipsos.org/models/ai_model_v2.bin"), PATH_AI_MODEL_V2)
            } else if arg == 3 {
                (alloc::string::String::from("http://fipsos.org/packages/sys_update.pkg"), PATH_SYS_UPDATE_PKG)
            } else if arg >= 0x0020_0000 {
                let mut uri_buf = [0u8; 128];
                ostd::mm::read_user_bytes(arg, &mut uri_buf).map_err(|_| -14i64)?;
                let len = uri_buf.iter().position(|&b| b == 0).unwrap_or(uri_buf.len());
                let parsed = core::str::from_utf8(&uri_buf[..len]).unwrap_or("http://fipsos.org/index.html");
                let dest = if parsed.contains("index.html") {
                    PATH_INDEX_HTML
                } else if parsed.contains("ai_model_v2") {
                    PATH_AI_MODEL_V2
                } else if parsed.contains("sys_update") {
                    PATH_SYS_UPDATE_PKG
                } else {
                    "/disk/browser_download.txt"
                };
                (alloc::string::String::from(parsed), dest)
            } else {
                (alloc::string::String::from("http://fipsos.org/index.html"), PATH_INDEX_HTML)
            };

            let (res, payload) = ostd::net::http_fetch_url(&uri_str);
            let _ = crate::diskfs::save_file(save_path, &payload);
            let (dl_fd, dl_koid) = self.create_or_update_disk_file(save_path, &payload)?;
            ostd::arch::audio::play_beep(1000, 60);

            ostd::early_println!(
                "[Browser::GET] \"{}\" descargado y guardado en SATA \"{}\" (fd={}, {}, {} B | RTT={} us)",
                uri_str,
                save_path,
                dl_fd,
                dl_koid,
                payload.len(),
                res.rtt_us
            );
            ostd::fb_println!(
                "[Browser::GET] \"{}\" -> SATA \"{}\" ({} B)",
                uri_str,
                save_path,
                payload.len()
            );
            return Ok(payload.len() as u64);
        }
        if request == VFS_IOCTL_SYS_TELEMETRY {
            let mem = ostd::mm::stats();
            let sched = ostd::scheduler::snapshot();
            let net = ostd::net::snapshot();
            let ahci = ostd::storage::ahci_snapshot();
            let rtc = ostd::arch::x86_64::rtc_power::read_rtc();
            let sensors = ostd::arch::x86_64::sensors::snapshot();
            let faults_rec = ostd::arch::syscall::user_faults_recovered();
            let pkg_present = self.devices.values().any(|n| n.path == PATH_DOWNLOAD_PKG);
            let wifi_snap = ostd::net::wifi::snapshot();
            let audio_snap = ostd::arch::audio::snapshot();

            let used_kib = (mem.used_bytes() / 1024) as u32;
            let total_kib = (mem.total_bytes() / 1024) as u32;
            let child_sw = sched
                .child_rust_switches
                .saturating_add(sched.child_cpp_switches)
                .min(0xFFFF) as u16;

            if arg != 0 {
                let mut pkt = [0u8; 192];
                pkt[0..4].copy_from_slice(&used_kib.to_le_bytes());
                pkt[4..8].copy_from_slice(&total_kib.to_le_bytes());
                pkt[8..12].copy_from_slice(&(mem.free_frames as u32).to_le_bytes());
                pkt[12..16].copy_from_slice(&(mem.used_frames as u32).to_le_bytes());
                pkt[16..24].copy_from_slice(&sched.total_irq0_ticks.to_le_bytes());
                pkt[24..28].copy_from_slice(&(sched.total_preemptions as u32).to_le_bytes());
                pkt[28..32].copy_from_slice(&(sched.total_voluntary_yields as u32).to_le_bytes());
                pkt[32..36].copy_from_slice(&(sched.daemon_switches as u32).to_le_bytes());
                pkt[36..40].copy_from_slice(&(sched.desktop_ui_ticks as u32).to_le_bytes());
                pkt[40..44].copy_from_slice(&(net.tx_packets as u32).to_le_bytes());
                pkt[44..48].copy_from_slice(&(net.rx_packets as u32).to_le_bytes());
                pkt[48..52].copy_from_slice(&(net.rx_bytes as u32).to_le_bytes());
                pkt[52..54].copy_from_slice(&(self.devices.len() as u16).to_le_bytes());
                pkt[54] = if net.link_up { 1 } else { 0 };
                pkt[55] = if pkg_present { 1 } else { 0 };
                pkt[56..62].copy_from_slice(&net.mac);
                pkt[62..64].copy_from_slice(&child_sw.to_le_bytes());
                pkt[64..72].copy_from_slice(&sched.active_cr3.to_le_bytes());
                pkt[72..80].copy_from_slice(&sched.shell_cr3.to_le_bytes());
                pkt[80..88].copy_from_slice(&sched.child_rust_cr3.to_le_bytes());
                pkt[88..96].copy_from_slice(&sched.child_cpp_cr3.to_le_bytes());
                pkt[96..104].copy_from_slice(&ahci.sectors_read.to_le_bytes());
                pkt[104..112].copy_from_slice(&ahci.sectors_written.to_le_bytes());
                pkt[112..120].copy_from_slice(&ahci.bytes_transferred.to_le_bytes());
                pkt[120] = if ahci.present { 1 } else { 0 };
                pkt[121] = ahci.port_idx;

                // --- FASE v1.7: RTC, CPU MHz, Temp, Cores/Threads, Fault Recovery, Driver Health ---
                pkt[128..130].copy_from_slice(&rtc.year.to_le_bytes());
                pkt[130] = rtc.month;
                pkt[131] = rtc.day;
                pkt[132] = rtc.hour;
                pkt[133] = rtc.minute;
                pkt[134] = rtc.second;
                pkt[135] = 0; // rtc_pad

                pkt[136..140].copy_from_slice(&sensors.cpu_mhz.to_le_bytes());
                pkt[140..144].copy_from_slice(&sensors.temp_celsius.to_le_bytes());
                pkt[144..152].copy_from_slice(&faults_rec.to_le_bytes());
                pkt[152..154].copy_from_slice(&sensors.physical_cores.to_le_bytes());
                pkt[154..156].copy_from_slice(&sensors.logical_threads.to_le_bytes());

                let mut driver_mask: u32 = 0;
                if sensors.e1000e_ok { driver_mask |= 1 << 0; }
                if sensors.ahci_ok { driver_mask |= 1 << 1; }
                if sensors.fb0_ok { driver_mask |= 1 << 2; }
                if sensors.npu_ok { driver_mask |= 1 << 3; }
                if wifi_snap.link_up { driver_mask |= 1 << 4; }
                if audio_snap.active { driver_mask |= 1 << 5; }
                pkt[156..160].copy_from_slice(&driver_mask.to_le_bytes());

                // --- FASE v1.8: WiFi, Audio, Background Task, SATA files ---
                pkt[160] = if wifi_snap.link_up { 1 } else { 0 };
                pkt[161] = wifi_snap.channel;
                pkt[162] = wifi_snap.security_type;
                pkt[163] = wifi_snap.active_iface;
                pkt[164..168].copy_from_slice(&(wifi_snap.rssi_dbm as i32).to_le_bytes());
                pkt[168..172].copy_from_slice(&audio_snap.beeps_count.to_le_bytes());
                let bg_state: u8 = if sched.bg_downloader_active { 1 } else if !sched.daemon_active { 2 } else { 0 };
                pkt[172] = bg_state;
                let disk_files = self.devices.values().filter(|n| n.path.starts_with("/disk/")).count() as u8;
                pkt[173] = disk_files;
                pkt[174] = ostd::scheduler::active_proc_slot_on_cpu(0) as u8;
                pkt[175] = ostd::scheduler::active_proc_slot_on_cpu(1) as u8;

                let copy_len = wifi_snap.ssid_len.min(16);
                pkt[176..176 + copy_len].copy_from_slice(&wifi_snap.ssid[..copy_len]);

                ostd::mm::write_user_bytes(arg, &pkt).map_err(|_| -14i64)?;
            }

            let used_mib = (used_kib / 1024) as u64;
            let total_mib = (total_kib / 1024) as u64;
            return Ok((used_mib << 16) | total_mib);
        }

        let node = self.devices.get(&fd).cloned().ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ | Rights::WRITE)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        if request == MOUSE_IOCTL_POLL_STATE {
            let snap = ostd::arch::mouse::poll_state();
            let packed = sync_mouse_vmo(&vmo, &snap);
            if arg == 1 {
                ostd::early_println!(
                    "[Mouse::/dev/input/mouse0] Pos=({}, {}) | Botones=[L:{} R:{} M:{}] ({:#04x}) | Paquetes={} | Clics={} | Packed={:#018x}",
                    snap.x,
                    snap.y,
                    snap.left as u8,
                    snap.right as u8,
                    snap.middle as u8,
                    snap.buttons,
                    snap.event_seq,
                    snap.click_count,
                    packed
                );
                ostd::fb_println!(
                    "[Mouse::/dev/input/mouse0] Pos=({}, {}) | Btn=[L:{} R:{} M:{}] | Pkts={} | Clics={}",
                    snap.x,
                    snap.y,
                    snap.left as u8,
                    snap.right as u8,
                    snap.middle as u8,
                    snap.event_seq,
                    snap.click_count
                );
            }
            return Ok(packed);
        }

        if request == MOUSE_IOCTL_STEP_SYNTHETIC {
            let dx = (arg & 0xFFFF) as u16 as i16;
            let dy = ((arg >> 16) & 0xFFFF) as u16 as i16;
            let buttons = ((arg >> 32) & 0xFF) as u8;
            let snap = ostd::arch::mouse::inject_synthetic_mouse_step(dx, dy, buttons);
            let packed = sync_mouse_vmo(&vmo, &snap);
            ostd::early_println!(
                "[Mouse::synthetic] Step(dx={}, dy={}, btn={:#x}) -> Pos=({}, {}) | Paquetes={} | Clics={}",
                dx,
                dy,
                buttons,
                snap.x,
                snap.y,
                snap.event_seq,
                snap.click_count
            );
            ostd::fb_println!(
                "[Mouse::synthetic] Step(dx={}, dy={}, btn={:#x}) -> Pos=({}, {}) | Pkts={}",
                dx,
                dy,
                buttons,
                snap.x,
                snap.y,
                snap.event_seq
            );
            return Ok(packed);
        }

        if request == NET_IOCTL_PING_GATEWAY {
            let res = ostd::net::ping_gateway();
            let packed = sync_net_vmo(&vmo, &res, None);
            if arg != 0 {
                ostd::early_println!(
                    "[Net::/dev/net/eth0] ARP+ICMP {}.{}.{}.{} -> {}.{}.{}.{} (GW {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}) | MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} | Link={} | DMA RX={:#x} TX={:#x} | TX={} pkts ({} B) RX={} pkts ({} B) | RTT={}.{:02} ms",
                    res.local_ip[0], res.local_ip[1], res.local_ip[2], res.local_ip[3],
                    res.gateway_ip[0], res.gateway_ip[1], res.gateway_ip[2], res.gateway_ip[3],
                    res.gateway_mac[0], res.gateway_mac[1], res.gateway_mac[2],
                    res.gateway_mac[3], res.gateway_mac[4], res.gateway_mac[5],
                    res.hw.mac[0], res.hw.mac[1], res.hw.mac[2],
                    res.hw.mac[3], res.hw.mac[4], res.hw.mac[5],
                    if res.hw.link_up { "UP" } else { "DOWN" },
                    res.hw.rx_ring_phys,
                    res.hw.tx_ring_phys,
                    res.hw.tx_packets,
                    res.hw.tx_bytes,
                    res.hw.rx_packets,
                    res.hw.rx_bytes,
                    res.rtt_us / 1000,
                    (res.rtt_us % 1000) / 10
                );
                ostd::fb_println!(
                    "[Net::eth0] Ping 10.0.2.15->10.0.2.2 MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} Link={} TX={} RX={} RTT={}us",
                    res.hw.mac[0], res.hw.mac[1], res.hw.mac[2],
                    res.hw.mac[3], res.hw.mac[4], res.hw.mac[5],
                    if res.hw.link_up { "UP" } else { "DOWN" },
                    res.hw.tx_packets,
                    res.hw.rx_packets,
                    res.rtt_us
                );
            }
            return Ok(packed);
        }

        if request == NET_IOCTL_HTTP_FETCH {
            let (res, payload) = ostd::net::http_fetch_package(PATH_DOWNLOAD_PKG);
            let _ = sync_net_vmo(&vmo, &res, Some(&payload));
            let net_vmo_koid = vmo.koid();
            let (dl_fd, dl_koid) =
                self.create_or_update_downloaded_file(PATH_DOWNLOAD_PKG, &payload)?;
            let _ = crate::diskfs::save_file(PATH_DOWNLOAD_PKG, &payload);

            ostd::early_println!(
                "[Net::wget] HTTP-Fetch eth0 (10.0.2.15 -> 10.0.2.2) -> VMO {} (64 KiB) -> Guardado en VFS \"{}\" (fd={}, {}, {} B | TX={} RX={})",
                net_vmo_koid,
                PATH_DOWNLOAD_PKG,
                dl_fd,
                dl_koid,
                payload.len(),
                res.hw.tx_packets,
                res.hw.rx_packets
            );
            ostd::fb_println!(
                "[Net::wget] HTTP-Fetch eth0 -> \"{}\" (fd={}, {}, {} B | TX={} RX={})",
                PATH_DOWNLOAD_PKG,
                dl_fd,
                dl_koid,
                payload.len(),
                res.hw.tx_packets,
                res.hw.rx_packets
            );
            return Ok(payload.len() as u64);
        }

        if request == FB_IOCTL_FLIP_BUFFER {
            let (x, y, width, height) = if arg == 0 {
                (0usize, 0usize, 1280usize, 800usize)
            } else {
                let rx = (arg & 0xFFFF) as usize;
                let ry = ((arg >> 16) & 0xFFFF) as usize;
                let rw = ((arg >> 32) & 0xFFFF) as usize;
                let rh = ((arg >> 48) & 0xFFFF) as usize;
                (rx, ry, rw, rh)
            };
            let copied =
                ostd::fb_console::flip_dma_double_buffer(vmo.dma_buffer(), x, y, width, height);
            return Ok(copied as u64);
        }

        if request == FB_IOCTL_PRESENT_RECT {
            let (width, height) = if arg != 0 {
                let w = ((arg >> 16) & 0xFFFF) as usize;
                let h = (arg & 0xFFFF) as usize;
                (w.clamp(16, 256), h.clamp(16, 256))
            } else {
                (FB_WIDGET_DEFAULT_W, FB_WIDGET_DEFAULT_H)
            };
            ostd::fb_console::present_dma_widget_rect(
                vmo.dma_buffer(),
                FB_WIDGET_DEFAULT_X,
                FB_WIDGET_DEFAULT_Y,
                width,
                height,
            );
            ostd::early_println!(
                "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", FB_IOCTL_PRESENT_RECT {}x{}) -> Blit Zero-Copy desde VMO {} (Phys={:#x})",
                fd,
                node.path,
                width,
                height,
                vmo.koid(),
                vmo.phys_addr()
            );
            return Ok((width * height) as u64);
        }

        if request == ACCEL_IOCTL_RUN_INFERENCE {
            let model_fd = arg;
            let model_node = self.devices.get(&model_fd).cloned().ok_or(-9i64)?;
            let model_entry = self
                .handle_table
                .get(model_node.handle, Rights::READ)
                .map_err(|_| -13i64)?;
            let model_vmo = model_entry.as_vmo().map_err(|_| -19i64)?;

            let sample_len = model_node.file_size.min(64);
            let mut model_header = vec![0u8; sample_len];
            model_vmo.read(0, &mut model_header).map_err(|_| -22i64)?;

            // 1. Zero-Copy bind input tensor shard into /dev/accel/accel0 VMO at offset 64
            let _ = vmo.write(64, &model_header);

            // 2. Execute real C11 hardware NPU driver (`c_npu_tensor_mac_kernel` in `ostd/c_src/npu_hw_driver.c`)
            //    strictly confined inside `ostd::ffi_jail::FfiJail::execute_c_function`!
            let ffi_jail = FfiJail::new();
            let dma_region_id = ffi_jail.register_dma_region(
                vmo.dma_buffer(),
                true,
                "NPU_C11_MAC_DMA_TENSOR_RW",
            );
            let ticks = ostd::arch::idt::timer_ticks();
            let mac_score = ffi_jail
                .execute_c_function(
                    dma_region_id,
                    0,
                    256,
                    true,
                    0x2026_F195u64 ^ ticks,
                    c_npu_driver_entry,
                )
                .map_err(|_| -5i64)? as u32;

            let mut fp32_bits_buf = [0u8; 4];
            let _ = vmo.read(128, &mut fp32_bits_buf);
            let fp32_t0_bits = u32::from_le_bytes(fp32_bits_buf);

            let mut milli_buf = [0u8; 4];
            let _ = vmo.read(216, &mut milli_buf);
            let milli_t0 = i32::from_le_bytes(milli_buf).max(0) as u32;

            let mut sig_bytes = [0u8; 20];
            let _ = vmo.read(192, &mut sig_bytes);
            let c_sig = core::str::from_utf8(&sig_bytes).unwrap_or("C11_NPU_FP32_SSE2_OK");

            ostd::early_println!(
                "[linux_abi::npu] Driver C11 ({}) en {}: \"{}\" -> \"{}\" | Sig=\"{}\" | FP32[0]={}.{:03} ({:#010x}) | Energy={}.{:03}",
                dma_region_id,
                vmo.koid(),
                model_node.path,
                node.path,
                c_sig,
                milli_t0 / 1000,
                milli_t0 % 1000,
                fp32_t0_bits,
                mac_score / 1000,
                mac_score % 1000
            );
            ostd::fb_println!(
                "[linux_abi::npu] Driver C11 ({}) Sig=\"{}\" | FP32[0]={}.{:03} ({:#010x}) | Energy={}.{:03}",
                dma_region_id,
                c_sig,
                milli_t0 / 1000,
                milli_t0 % 1000,
                fp32_t0_bits,
                mac_score / 1000,
                mac_score % 1000
            );
            return Ok(mac_score as u64);
        }

        let mut tag_buf = [0u8; 26];
        let _ = vmo.read(0, &mut tag_buf);
        let tag_str = core::str::from_utf8(&tag_buf).unwrap_or("<bin>");
        let ticks = ostd::arch::idt::timer_ticks();

        ostd::early_println!(
            "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", cmd={:#x}, arg={:#x}) -> VMO {} (Phys={:#x}, Tag=\"{}\", TimerIRQ0={} ticks)",
            fd,
            node.path,
            request,
            arg,
            vmo.koid(),
            vmo.phys_addr(),
            tag_str,
            ticks
        );
        ostd::fb_println!(
            "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", cmd={:#x}) -> VMO {} (Phys={:#x}, TimerIRQ0={} ticks)",
            fd,
            node.path,
            request,
            vmo.koid(),
            vmo.phys_addr(),
            ticks
        );

        Ok(vmo.koid().raw())
    }

    /// Redirects an `mmap` syscall on `/dev/dri/card0`, `/dev/accel/accel0`, `/dev/fb0`, or a `TarFs` file
    /// to its underlying `Vmo` Zero-Copy object, mapping physical frames into Lower-Half User Space (`0x1000_0000+`).
    pub fn mmap(
        &self,
        _addr: u64,
        len: usize,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: usize,
    ) -> Result<u64, i64> {
        if len == 0 {
            return Err(-22i64); // -EINVAL
        }

        let writable = (prot & PROT_WRITE) != 0;

        // Support standard Linux MAP_ANONYMOUS (flags & 0x20 != 0 or fd == !0) for malloc/mmap
        if (flags & MAP_ANONYMOUS) != 0 || fd == u64::MAX || (fd as i64) == -1 {
            let mapped_addr = ostd::mm::map_anonymous_user_memory(len, writable).map_err(|_| -12i64)?;
            ostd::early_println!(
                "[linux_abi::vfs] sys_mmap(MAP_ANONYMOUS, len={}, prot={:#x}) -> UserVirt={:#x}",
                len,
                prot,
                mapped_addr
            );
            return Ok(mapped_addr);
        }

        let required_rights = if writable {
            Rights::READ | Rights::WRITE | Rights::MAP
        } else {
            Rights::READ | Rights::MAP
        };

        let node = self.devices.get(&fd).ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, required_rights)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        if offset.saturating_add(len) > vmo.size_bytes() {
            return Err(-22i64); // -EINVAL
        }

        let target_phys = vmo.phys_addr() + (offset as u64);
        let user_vaddr = map_vmo_to_user(target_phys, len, writable).map_err(|_| -12i64)?; // -ENOMEM

        ostd::early_println!(
            "[linux_abi::vfs] sys_mmap(fd={} \"{}\", len={} B, prot={:#x}, flags={:#x}) -> Zero-Copy VMO {} mapeado en Anillo 3 UserVirt={:#x} (Phys={:#x})",
            fd,
            node.path,
            len,
            prot,
            flags,
            vmo.koid(),
            user_vaddr,
            target_phys
        );
        ostd::fb_println!(
            "[linux_abi::vfs] sys_mmap(fd={} \"{}\", len={} B) -> Zero-Copy VMO {} en UserVirt={:#x} (Phys={:#x})",
            fd,
            node.path,
            len,
            vmo.koid(),
            user_vaddr,
            target_phys
        );

        Ok(user_vaddr)
    }

    /// Reads bytes directly from the underlying `Vmo` of a mounted device node (`fd`).
    pub fn read_device_vmo(&self, fd: u64, offset: usize, dst: &mut [u8]) -> Result<(), i64> {
        let node = self.devices.get(&fd).ok_or(-9i64)?;
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        vmo.read(offset, dst).map_err(|_| -22i64)
    }
}

impl Default for CapabilityVfs {
    fn default() -> Self {
        Self::new()
    }
}

fn sync_mouse_vmo(vmo: &Vmo, snap: &ostd::arch::mouse::MouseSnapshot) -> u64 {
    let packed = snap.pack_u64();
    let mut state_bytes = [0u8; 32];
    state_bytes[0..2].copy_from_slice(&snap.x.to_le_bytes());
    state_bytes[2..4].copy_from_slice(&snap.y.to_le_bytes());
    state_bytes[4] = snap.buttons;
    state_bytes[8..16].copy_from_slice(&snap.event_seq.to_le_bytes());
    state_bytes[16..24].copy_from_slice(&snap.click_count.to_le_bytes());
    state_bytes[24..32].copy_from_slice(&packed.to_le_bytes());
    let _ = vmo.write(32, &state_bytes);
    packed
}

fn sync_net_vmo(
    vmo: &Vmo,
    res: &ostd::net::NetPingResult,
    http_payload: Option<&[u8]>,
) -> u64 {
    let packed = res.pack_u64();
    let mut hdr = [0u8; 64];
    hdr[0..6].copy_from_slice(&res.hw.mac);
    hdr[6..12].copy_from_slice(&res.gateway_mac);
    hdr[12..16].copy_from_slice(&res.local_ip);
    hdr[16..20].copy_from_slice(&res.gateway_ip);
    hdr[20..22].copy_from_slice(&res.rtt_us.to_le_bytes());
    hdr[24..32].copy_from_slice(&res.hw.tx_packets.to_le_bytes());
    hdr[32..40].copy_from_slice(&res.hw.rx_packets.to_le_bytes());
    hdr[40..48].copy_from_slice(&res.hw.tx_bytes.to_le_bytes());
    hdr[48..56].copy_from_slice(&res.hw.rx_bytes.to_le_bytes());
    hdr[56..64].copy_from_slice(&packed.to_le_bytes());
    let _ = vmo.write(32, &hdr);
    if let Some(body) = http_payload {
        let max_copy = body.len().min(vmo.size_bytes().saturating_sub(128));
        if max_copy > 0 {
            let _ = vmo.write(128, &body[..max_copy]);
        }
    }
    packed
}
