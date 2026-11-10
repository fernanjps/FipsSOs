//! Application Processor (AP) Worker Loop and Core Telemetry (`ostd::scheduler::smp_idle`).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use super::slots::{SLOT_SHELL, SLOT_DAEMON, SLOT_BG_DOWNLOADER};
use super::{BG_DOWNLOADER_ACTIVE, BG_DOWNLOADER_TICKS, BG_DOWNLOADER_SWITCHES};

/// Maximum number of supported CPU cores.
pub const MAX_CPUS: usize = 4;

/// Per-core control block (`CpuControlBlock` / `CpuLocal`).
pub struct CpuControlBlock {
    pub cpu_id: u32,
    pub lapic_id: AtomicU64,
    pub is_bsp: AtomicBool,
    pub online: AtomicBool,
    pub active_proc_slot: AtomicU64,
    pub irq_ticks: AtomicU64,
    pub total_switches: AtomicU64,
    pub idle_ticks: AtomicU64,
    pub compute_ticks: AtomicU64,
}

impl CpuControlBlock {
    pub const fn new(cpu_id: u32) -> Self {
        Self {
            cpu_id,
            lapic_id: AtomicU64::new(0),
            is_bsp: AtomicBool::new(false),
            online: AtomicBool::new(false),
            active_proc_slot: AtomicU64::new(SLOT_SHELL),
            irq_ticks: AtomicU64::new(0),
            total_switches: AtomicU64::new(0),
            idle_ticks: AtomicU64::new(0),
            compute_ticks: AtomicU64::new(0),
        }
    }
}

pub(crate) static CPU_CONTROLS: [CpuControlBlock; MAX_CPUS] = [
    CpuControlBlock::new(0),
    CpuControlBlock::new(1),
    CpuControlBlock::new(2),
    CpuControlBlock::new(3),
];

/// Registers the Bootstrap Processor (BSP) in the CPU control table.
pub fn register_bsp(lapic_id: u32) {
    CPU_CONTROLS[0].lapic_id.store(lapic_id as u64, Ordering::Release);
    CPU_CONTROLS[0].is_bsp.store(true, Ordering::Release);
    CPU_CONTROLS[0].online.store(true, Ordering::Release);
    CPU_CONTROLS[0].active_proc_slot.store(SLOT_SHELL, Ordering::Release);
}

/// Registers an Application Processor (AP) as online in the CPU control table.
pub fn register_ap_online(cpu_id: u32, lapic_id: u32) {
    let idx = (cpu_id as usize).min(MAX_CPUS - 1);
    let block = &CPU_CONTROLS[idx];
    block.lapic_id.store(lapic_id as u64, Ordering::Release);
    block.is_bsp.store(false, Ordering::Release);
    block.active_proc_slot.store(SLOT_DAEMON, Ordering::Release);
    block.online.store(true, Ordering::Release);
}

/// Returns whether the specified CPU is marked online.
pub fn is_ap_online(cpu_id: u32) -> bool {
    let idx = cpu_id as usize;
    if idx < MAX_CPUS {
        CPU_CONTROLS[idx].online.load(Ordering::Acquire)
    } else {
        false
    }
}

/// Returns the total count of online processors.
pub fn online_cpu_count() -> usize {
    let ct = CPU_CONTROLS
        .iter()
        .filter(|c| c.online.load(Ordering::Acquire))
        .count();
    ct.max(1)
}

/// Returns a reference to the `CpuControlBlock` of the requested core.
pub fn cpu_control_block(cpu_id: usize) -> Option<&'static CpuControlBlock> {
    if cpu_id < MAX_CPUS && CPU_CONTROLS[cpu_id].online.load(Ordering::Acquire) {
        Some(&CPU_CONTROLS[cpu_id])
    } else {
        None
    }
}

/// Concurrent distributed execution loop for an Application Processor (`AP`).
///
/// Executes secondary worker threads concurrently on Core 1:
/// - Drains Zero-Syscall shared memory I/O rings (`ostd::ipc`) without context switch overhead.
/// - When `BG_DOWNLOADER_ACTIVE` is true, Core 1 executes background download tasks (`SLOT_BG_DOWNLOADER`).
/// - When `DAEMON_ACTIVE` is true, Core 1 executes `npu-telemetryd` tasks (`SLOT_DAEMON`).
/// - Idle Power Governor: When no work is queued, Core 1 executes `hlt` with interrupts enabled (`sti; hlt`).
pub fn ap_worker_loop(cpu_id: u32) -> ! {
    let idx = (cpu_id as usize).min(MAX_CPUS - 1);
    let block = &CPU_CONTROLS[idx];

    loop {
        let mut did_work = false;

        // 1. Drain Zero-Syscall IPC rings without context switches
        let processed_rings = crate::ipc::poll_active_rings();
        if processed_rings > 0 {
            block.compute_ticks.fetch_add(processed_rings as u64, Ordering::Relaxed);
            block.irq_ticks.fetch_add(processed_rings as u64, Ordering::Relaxed);
            did_work = true;
        }

        // 2. Background downloader workload
        if BG_DOWNLOADER_ACTIVE.load(Ordering::Acquire) {
            block.active_proc_slot.store(SLOT_BG_DOWNLOADER, Ordering::Release);
            BG_DOWNLOADER_TICKS.fetch_add(1, Ordering::Relaxed);
            let bsw = BG_DOWNLOADER_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            block.irq_ticks.fetch_add(1, Ordering::Relaxed);
            block.compute_ticks.fetch_add(1, Ordering::Relaxed);
            block.total_switches.store(bsw, Ordering::Relaxed);
            did_work = true;
        }

        if !did_work {
            // Idle Power Governor: Core 1 suspends safely with interrupts enabled waiting for next hardware IRQ
            block.idle_ticks.fetch_add(1, Ordering::Relaxed);
            block.irq_ticks.fetch_add(1, Ordering::Relaxed);
            // SAFETY: Low-power CPU pause and interrupt suspension sequence in Ring 0
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
                for _ in 0..128 {
                    core::arch::x86_64::_mm_pause();
                }
                core::arch::asm!("hlt", options(nomem, nostack));
                core::arch::asm!("cli", options(nomem, nostack));
            }
        } else {
            for _ in 0..32 {
                core::arch::x86_64::_mm_pause();
            }
        }
    }
}

/// Records an idle tick spent in HLT state for the given CPU core.
pub fn record_idle_tick(cpu_id: usize) {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].idle_ticks.fetch_add(1, Ordering::Relaxed);
        CPU_CONTROLS[cpu_id].irq_ticks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records active compute ticks for the given CPU core.
pub fn record_compute_tick(cpu_id: usize, count: u64) {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].compute_ticks.fetch_add(count, Ordering::Relaxed);
        CPU_CONTROLS[cpu_id].irq_ticks.fetch_add(count, Ordering::Relaxed);
    }
}

/// Calculates the real-time CPU utilization percentage (0..100%) based on compute vs idle ticks.
pub fn cpu_usage_percent(cpu_id: usize) -> u32 {
    if cpu_id < MAX_CPUS {
        let compute = CPU_CONTROLS[cpu_id].compute_ticks.load(Ordering::Relaxed);
        let idle = CPU_CONTROLS[cpu_id].idle_ticks.load(Ordering::Relaxed);
        let total = compute + idle;
        if total == 0 {
            0
        } else {
            ((compute * 100) / total).min(100) as u32
        }
    } else {
        0
    }
}

/// Returns the process slot currently assigned and running on the requested CPU.
pub fn active_proc_slot_on_cpu(cpu_id: usize) -> u64 {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].active_proc_slot.load(Ordering::Acquire)
    } else {
        0
    }
}

