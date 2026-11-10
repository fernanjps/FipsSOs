#![forbid(unsafe_code)]

use alloc::string::String;
use kernel_objects::Vmo;

pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;
pub const FD_STDERR: u64 = 2;
pub const FD_DRM_CARD0: u64 = 3;
pub const FD_ACCEL_NPU0: u64 = 4;
pub const FD_FB0: u64 = 5;
pub const FD_MOUSE0: u64 = 6;
pub const FD_NET0: u64 = 7;

/// Canonical virtual device and `Initramfs` file paths exposed by the FipSOs capability VFS.
pub const PATH_DRM_CARD0: &str = "/dev/dri/card0";
pub const PATH_ACCEL_NPU0: &str = "/dev/accel/accel0";
pub const PATH_FB0: &str = "/dev/fb0";
pub const PATH_MOUSE0: &str = "/dev/input/mouse0";
pub const PATH_NET0: &str = "/dev/net/eth0";
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
/// `/dev/net/eth0` (PCIe e1000e Network Stack), `/dev/input/mouse0` (PS/2 Mouse),
/// DiskFS SATA AHCI persistence, Task Control, and VFS/System Telemetry.
pub const DISK_IOCTL_SAVE_NOTE: u64 = 0xC010_4401;
pub const DISK_IOCTL_DELETE_FILE: u64 = 0xC010_4402;
pub const DRM_IOCTL_PRIME_ZERO_COPY: u64 = 0xC010_6400;
pub const ACCEL_IOCTL_BIND_TENSOR_VMO: u64 = 0xC010_4100;
pub const ACCEL_IOCTL_RUN_INFERENCE: u64 = 0xC010_4102;
pub const FBIOGET_VSCREENINFO: u64 = 0x4600;
pub const FB_IOCTL_GET_VSCREENINFO: u64 = 0xC010_4600;
pub const FB_IOCTL_PRESENT_RECT: u64 = 0xC010_4601;
pub const FB_IOCTL_GET_INFO: u64 = 0x4600;
pub const FB_IOCTL_POLL_KEY: u64 = 0xC010_4602;
pub const FB_IOCTL_SET_DESKTOP_MODE: u64 = 0xC010_4603;
pub const FB_IOCTL_FLIP_BUFFER: u64 = 0xC010_4604;
pub const MOUSE_IOCTL_POLL_STATE: u64 = 0xC010_4D01;
pub const MOUSE_IOCTL_STEP_SYNTHETIC: u64 = 0xC010_4D02;
pub const NET_IOCTL_PING_GATEWAY: u64 = 0xC010_4E01;
pub const NET_IOCTL_HTTP_FETCH: u64 = 0xC010_4E02;
pub const NET_IOCTL_BROWSER_FETCH_SAVE: u64 = 0xC010_4E03;
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

pub fn build_hwmon_text() -> String {
    let sensors = ostd::arch::x86_64::sensors::snapshot();
    let rtc = ostd::arch::x86_64::rtc_power::read_rtc();
    let topo = ostd::arch::x86_64::topology::detect();
    let faults = ostd::arch::syscall::user_faults_recovered();
    let mem = ostd::mm::memory_stats();
    let c0_usage = ostd::scheduler::cpu_usage_percent(0);
    let c1_usage = ostd::scheduler::cpu_usage_percent(1);
    let c0_idle = ostd::scheduler::cpu_control_block(0)
        .map(|b| b.idle_ticks.load(core::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0);
    let c1_idle = ostd::scheduler::cpu_control_block(1)
        .map(|b| b.idle_ticks.load(core::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0);
    let temp_str = if sensors.temp_celsius == 0 {
        alloc::format!("N/A (VM Hypervisor)")
    } else {
        alloc::format!("{} C (IA32_THERM_STATUS)", sensors.temp_celsius)
    };
    let usb_str = ostd::bus::xhci::summary_text();
    let nvme_str = ostd::storage::nvme::summary_text();
    let local_ip = ostd::net::current_ipv4();
    let gw_ip = ostd::net::current_gateway_ipv4();

    alloc::format!(
        "[HWMON] FipSOs v3.1 Hardware Monitor & Sensores:\n\
         CPU: {} (Family {}, Model {}, Stepping {}) @ {} MHz\n\
         Topology: {} nucleos fisicos / {} hilos logicos (SMT)\n\
         Gobernador de Energia ACPI/HLT: ACTIVO | Core 0: {}% uso (idle: {} ticks) | Core 1: {}% uso (idle: {} ticks)\n\
         Temperatura: {}\n\
         Memoria Fisica: {} KiB Total | {} KiB Usados ({} frames) | {} KiB Libres ({} frames)\n\
         Reloj Real CMOS RTC: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC\n\
         Drivers Hardware:\n\
           - [NIC]  PCIe 00:02.0 Intel e1000e   : {} (IP={}.{}.{}.{}, GW={}.{}.{}.{})\n\
           - [SATA] PCIe 00:1f.2 ICH9 AHCI DMA  : {}\n\
           - [NVMe] PCIe Storage Controller     : {}\n\
           - [GPU]  Limine Framebuffer          : {}\n\
           - [USB]  Host Controllers (PCIe)    : {}\n\
           - [SIMD] Motor SSE2/AVX Engine C11   : {}\n\
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
        temp_str,
        mem.total_bytes() / 1024,
        mem.used_bytes() / 1024,
        mem.used_frames,
        mem.free_bytes() / 1024,
        mem.free_frames,
        rtc.year,
        rtc.month,
        rtc.day,
        rtc.hour,
        rtc.minute,
        rtc.second,
        if sensors.e1000e_ok { "LINK UP (1Gbps)" } else { "NO LINK" },
        local_ip[0],
        local_ip[1],
        local_ip[2],
        local_ip[3],
        gw_ip[0],
        gw_ip[1],
        gw_ip[2],
        gw_ip[3],
        if sensors.ahci_ok { "LINK UP (Puerto 0 DMA EXT)" } else { "NO LINK" },
        nvme_str,
        if sensors.fb0_ok { "ACTIVO (Doble Buffer DMA)" } else { "INACTIVO" },
        usb_str,
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

pub fn sync_mouse_vmo(vmo: &Vmo, snap: &ostd::arch::mouse::MouseSnapshot) -> u64 {
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

pub fn sync_net_vmo(
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

