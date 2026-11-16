//! Real Hardware Sensor Telemetry (RDTSC, SMT Cores/Threads, MSR Thermal Status, Driver Health).

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static CALIBRATED_MHZ: AtomicU32 = AtomicU32::new(0);
static LAST_SAMPLE_TSC: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE_TICK: AtomicU64 = AtomicU64::new(0);

/// Snapshot of hardware telemetry sensors and driver health.
#[derive(Debug, Clone, Copy)]
pub struct HardwareSensorsSnapshot {
    pub cpu_mhz: u32,
    pub temp_celsius: u32,
    pub physical_cores: u16,
    pub logical_threads: u16,
    pub tsc: u64,
    pub e1000e_ok: bool,
    pub ahci_ok: bool,
    pub fb0_ok: bool,
    pub npu_ok: bool,
}

/// Reads the x86_64 Time Stamp Counter (`RDTSC`).
#[inline(always)]
pub fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: Standard architectural RDTSC instruction.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Estimates CPU frequency in MHz using PIT-calibrated TSC immediately, refined by IRQ0 ticks.
pub fn cpu_mhz() -> u32 {
    let cur_tsc = read_tsc();
    let cur_tick = crate::arch::idt::timer_ticks();

    let mut cached = CALIBRATED_MHZ.load(Ordering::Relaxed);
    if cached == 0 {
        let hz = crate::time::tsc_hz();
        if hz >= 100_000_000 {
            cached = ((hz / 1_000_000) as u32).clamp(400, 6000);
            CALIBRATED_MHZ.store(cached, Ordering::Relaxed);
        }
    }

    let last_tsc = LAST_SAMPLE_TSC.load(Ordering::Relaxed);
    let last_tick = LAST_SAMPLE_TICK.load(Ordering::Relaxed);

    if last_tick == 0 {
        LAST_SAMPLE_TSC.store(cur_tsc, Ordering::Relaxed);
        LAST_SAMPLE_TICK.store(cur_tick, Ordering::Relaxed);
        return cached;
    }

    let delta_ticks = cur_tick.saturating_sub(last_tick);
    if delta_ticks >= 20 {
        let delta_tsc = cur_tsc.saturating_sub(last_tsc);
        let mhz = (delta_tsc / (delta_ticks * 10_000)).clamp(400, 6000) as u32;
        CALIBRATED_MHZ.store(mhz, Ordering::Relaxed);
        LAST_SAMPLE_TSC.store(cur_tsc, Ordering::Relaxed);
        LAST_SAMPLE_TICK.store(cur_tick, Ordering::Relaxed);
        mhz
    } else {
        cached
    }
}

/// Detects package temperature in Celsius via MSR `IA32_THERM_STATUS` (`0x19C`) with `#GP` trap guard.
///
/// Returns 0 if running on AMD CPUs (`AuthenticAMD`), inside a hypervisor,
/// or if MSR `0x19C` is invalid or raises `#GP`.
pub fn cpu_temperature_celsius() -> u32 {
    let topo = super::topology::detect();

    // Check CPUID leaf 1 ECX bit 31 (Hypervisor present)
    let is_hypervisor = {
        let leaf1 = core::arch::x86_64::__cpuid(1);
        (leaf1.ecx & (1 << 31)) != 0
    };

    if is_hypervisor || topo.vendor() == "AuthenticAMD" {
        return 0;
    }

    // Check CPUID leaf 6 (Thermal and Power Management) bit 0 (DTS)
    let dts_supported = {
        let leaf6 = core::arch::x86_64::__cpuid(6);
        (leaf6.eax & 0x01) != 0
    };

    if dts_supported && topo.vendor() == "GenuineIntel" {
        if let Some(val) = super::msr::rdmsr_safe(0x19C) {
            let valid = (val & (1 << 31)) != 0;
            if valid {
                let delta = ((val >> 16) & 0x7F) as u32;
                // Standard TjMax = 100 C
                return 100u32.saturating_sub(delta).clamp(25, 95);
            }
        }
    }

    0
}

/// Takes a consolidated snapshot of hardware sensors and all 4 PCIe/platform drivers.
pub fn snapshot() -> HardwareSensorsSnapshot {
    let topo = super::topology::detect();
    let net = crate::net::snapshot();
    let ahci = crate::storage::ahci_snapshot();
    let fb_active = crate::fb_console::is_active();

    let online_cpus = crate::scheduler::online_cpu_count() as u16;
    let threads = if online_cpus > 1 { online_cpus } else { topo.logical_cores as u16 }.max(1);
    let cores = threads;

    let npu_present = crate::bus::pcie::is_npu_present();

    HardwareSensorsSnapshot {
        cpu_mhz: cpu_mhz(),
        temp_celsius: cpu_temperature_celsius(),
        physical_cores: cores,
        logical_threads: threads,
        tsc: read_tsc(),
        e1000e_ok: net.link_up,
        ahci_ok: ahci.present,
        fb0_ok: fb_active,
        npu_ok: npu_present,
    }
}
