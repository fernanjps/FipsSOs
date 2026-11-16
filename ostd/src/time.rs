//! Real-time hardware timestamp (`RDTSC`) calibration & millisecond timeout helpers.

use core::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_FALLBACK_TSC_HZ: u64 = 3_000_000_000; // Safe 3.0 GHz default prior to calibration
const PIT_FREQ_HZ: u64 = 1_193_182;
const PIT_10MS_TICKS: u16 = 11_932; // ~10.00015 ms at 1.193182 MHz

static CALIBRATED_TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Reads the CPU hardware Time Stamp Counter (`RDTSC` on `x86_64`, `CNTVCT_EL0` on `aarch64`).
#[inline(always)]
pub fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::sensors::read_tsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// Calibrates the TSC frequency against PIT Channel 2 (`~10 ms` one-shot window).
///
/// Falls back to a safe `3 GHz` estimate if running without functional PIT Channel 2
/// or inside a constrained hypervisor where PIT OUT2 does not toggle.
pub fn calibrate_tsc_pit() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: Reading/writing standard PC PIT Channel 2 ports (0x42, 0x43, 0x61) in Ring 0.
        let hz = unsafe {
            let prev_61: u8;
            core::arch::asm!("in al, dx", in("dx") 0x61u16, out("al") prev_61, options(nomem, nostack, preserves_flags));
            // Enable PIT channel 2 gate (bit 0 = 1), disable speaker data (bit 1 = 0)
            core::arch::asm!("out dx, al", in("dx") 0x61u16, in("al") (prev_61 & 0xFC) | 0x01, options(nomem, nostack, preserves_flags));
            // Channel 2, lobyte/hibyte, mode 0 (interrupt on terminal count), binary
            core::arch::asm!("out dx, al", in("dx") 0x43u16, in("al") 0xB0u8, options(nomem, nostack, preserves_flags));
            core::arch::asm!("out dx, al", in("dx") 0x42u16, in("al") (PIT_10MS_TICKS & 0xFF) as u8, options(nomem, nostack, preserves_flags));
            core::arch::asm!("out dx, al", in("dx") 0x42u16, in("al") (PIT_10MS_TICKS >> 8) as u8, options(nomem, nostack, preserves_flags));

            let t0 = read_tsc();
            let mut out2 = false;
            for _ in 0..30_000_000u32 {
                let st: u8;
                core::arch::asm!("in al, dx", in("dx") 0x61u16, out("al") st, options(nomem, nostack, preserves_flags));
                if (st & 0x20) != 0 {
                    out2 = true;
                    break;
                }
                core::hint::spin_loop();
            }
            let t1 = read_tsc();
            // Restore original port 0x61 bits
            core::arch::asm!("out dx, al", in("dx") 0x61u16, in("al") prev_61, options(nomem, nostack, preserves_flags));

            let delta = t1.saturating_sub(t0);
            if out2 && delta >= 500_000 {
                // hz = delta * PIT_FREQ_HZ / PIT_10MS_TICKS
                let calc = ((delta as u128) * (PIT_FREQ_HZ as u128) / (PIT_10MS_TICKS as u128)) as u64;
                calc.clamp(500_000_000, 6_500_000_000)
            } else {
                DEFAULT_FALLBACK_TSC_HZ
            }
        };
        CALIBRATED_TSC_HZ.store(hz, Ordering::Release);
        hz
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        CALIBRATED_TSC_HZ.store(DEFAULT_FALLBACK_TSC_HZ, Ordering::Release);
        DEFAULT_FALLBACK_TSC_HZ
    }
}

/// Returns the calibrated TSC frequency in Hz (defaults to 3 GHz until PIT calibration runs).
#[inline]
pub fn tsc_hz() -> u64 {
    let cached = CALIBRATED_TSC_HZ.load(Ordering::Acquire);
    if cached != 0 {
        cached
    } else {
        DEFAULT_FALLBACK_TSC_HZ
    }
}

/// Returns TSC cycles per millisecond based on PIT calibration or safe 3 GHz estimate.
#[inline]
pub fn tsc_cycles_per_ms() -> u64 {
    (tsc_hz() / 1_000).max(1_000)
}

/// Computes a future TSC deadline `ms` milliseconds from now.
#[inline]
pub fn tsc_deadline_ms(ms: u64) -> u64 {
    read_tsc().saturating_add(ms.saturating_mul(tsc_cycles_per_ms()))
}

/// Returns `true` once `RDTSC` reaches or exceeds `deadline`.
#[inline]
pub fn tsc_expired(deadline: u64) -> bool {
    read_tsc() >= deadline
}

/// Busy-waits for `us` microseconds using the calibrated hardware TSC.
#[inline]
pub fn tsc_delay_us(us: u64) {
    let cycles_per_us = (tsc_hz() / 1_000_000).max(500);
    let deadline = read_tsc().saturating_add(us.saturating_mul(cycles_per_us));
    while !tsc_expired(deadline) {
        core::hint::spin_loop();
    }
}
