//! Hardware x87 FPU and SIMD SSE/SSE2 initialization plus 512-byte `FXSAVE64`/`FXRSTOR64`
//! context preservation (`ostd::arch::x86_64::fpu`).

use core::sync::atomic::{AtomicBool, Ordering};

/// CR0 bit flags for x87 FPU / SSE hardware enablement.
const CR0_MP: u64 = 1 << 1; // Monitor Coprocessor (1)
const CR0_EM: u64 = 1 << 2; // Emulation (must be 0 for hardware FPU/SSE)
const CR0_TS: u64 = 1 << 3; // Task Switched (clear to 0)
const CR0_NE: u64 = 1 << 5; // Numeric Error native mode (1)

/// CR4 bit flags for FXSAVE64/FXRSTOR64 and SSE/SSE2 XMM0..XMM15 registers.
const CR4_OSFXSR: u64 = 1 << 9; // OS support for FXSAVE/FXRSTOR and SSE XMM registers
const CR4_OSXMMEXCPT: u64 = 1 << 10; // OS support for unmasked SIMD floating-point exceptions

/// Default architectural `MXCSR` control/status value (`0x1F80`):
/// All 6 SIMD floating-point exceptions masked (IM, DM, ZM, OM, UM, PM = bits 7..12),
/// round-to-nearest-even, flush-to-zero disabled.
pub const DEFAULT_MXCSR: u32 = 0x1F80;

/// Default x87 FPU Control Word (`0x037F`): double-extended precision, all exceptions masked.
pub const DEFAULT_FCW: u16 = 0x037F;

static FPU_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Architectural 512-byte `FXSAVE64` / `FXRSTOR64` state container aligned to 16 bytes.
///
/// Preserves x87 FPU registers, `FCW`/`FSW`, `MXCSR`, and all 16 128-bit `XMM0..XMM15`
/// registers across preemptive/cooperative context switches and Ring 0 C11 `FfiJail` calls.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FpuState([u8; 512]);

impl FpuState {
    /// Creates a valid zeroed 512-byte `FXSAVE64` image pre-populated with architectural
    /// `FCW = 0x037F` (bytes `0..2`) and `MXCSR = 0x1F80` (bytes `24..28`) so `restore()`
    /// is always safe even prior to a hardware `save()`.
    pub const fn new() -> Self {
        let mut buf = [0u8; 512];
        // Bytes 0..2: x87 FPU Control Word (FCW = 0x037F)
        buf[0] = (DEFAULT_FCW & 0xFF) as u8;
        buf[1] = (DEFAULT_FCW >> 8) as u8;
        // Bytes 24..28: MXCSR = 0x0000_1F80 (avoids #GP on FXRSTOR64)
        buf[24] = (DEFAULT_MXCSR & 0xFF) as u8;
        buf[25] = ((DEFAULT_MXCSR >> 8) & 0xFF) as u8;
        buf[26] = ((DEFAULT_MXCSR >> 16) & 0xFF) as u8;
        buf[27] = ((DEFAULT_MXCSR >> 24) & 0xFF) as u8;
        Self(buf)
    }

    /// Saves the current CPU's x87 FPU, `MXCSR`, and `XMM0..XMM15` state into `self`
    /// using the 64-bit `fxsave64` instruction.
    #[inline]
    pub fn save(&mut self) {
        if !FPU_INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        let ptr = self.0.as_mut_ptr();
        // SAFETY: `self` is 16-byte aligned (`#[repr(C, align(16))]`), 512 bytes wide,
        // and `CR4.OSFXSR` was enabled by `fpu::init()`.
        unsafe {
            core::arch::asm!(
                "fxsave64 [{}]",
                in(reg) ptr,
                options(nostack, preserves_flags)
            );
        }
    }

    /// Restores the CPU's x87 FPU, `MXCSR`, and `XMM0..XMM15` state from `self`
    /// using the 64-bit `fxrstor64` instruction.
    #[inline]
    pub fn restore(&self) {
        if !FPU_INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        let ptr = self.0.as_ptr();
        // SAFETY: `self` is 16-byte aligned (`#[repr(C, align(16))]`), 512 bytes wide,
        // contains valid non-reserved `MXCSR` bits, and `CR4.OSFXSR` is active.
        unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) ptr,
                options(nostack, preserves_flags)
            );
        }
    }

    /// Reads the saved 32-bit `MXCSR` field from bytes `24..28` of the `FXSAVE64` area.
    #[inline]
    pub fn mxcsr(&self) -> u32 {
        u32::from_le_bytes([self.0[24], self.0[25], self.0[26], self.0[27]])
    }
}

impl Default for FpuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Enables hardware x87 FPU and SIMD SSE/SSE2 instructions (`XMM0..XMM15`, `FXSAVE64`/`FXRSTOR64`)
/// for both Ring 0 and Ring 3:
/// 1. `CR0`: clears `EM` (bit 2) and `TS` (bit 3), sets `MP` (bit 1) and `NE` (bit 5).
/// 2. `CR4`: sets `OSFXSR` (bit 9) and `OSXMMEXCPT` (bit 10).
/// 3. Executes `fninit` and loads `MXCSR = 0x1F80` via `ldmxcsr`.
pub fn init() {
    if FPU_INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }

    let cr0_after: u64;
    let cr4_after: u64;
    let mxcsr_default: u32 = DEFAULT_MXCSR;

    // SAFETY: Configuring architectural CR0/CR4 FPU/SSE control bits, initializing x87 via
    // `fninit`, and masking default SIMD exceptions via `ldmxcsr` in Ring 0.
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 &= !(CR0_EM | CR0_TS);
        cr0 |= CR0_MP | CR0_NE;
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0_after = cr0;

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4_after = cr4;

        let mxcsr_ptr = &raw const mxcsr_default;
        core::arch::asm!(
            "fninit",
            "ldmxcsr [{}]",
            in(reg) mxcsr_ptr,
            options(nostack, preserves_flags)
        );
    }

    crate::early_println!(
        "[ostd::arch::fpu] FPU x87 + SSE/SSE2 activos (CR0={:#x}, CR4={:#x}, MXCSR={:#x}, FXSAVE64=512B)",
        cr0_after,
        cr4_after,
        mxcsr_default
    );
    crate::fb_println!(
        "[ostd::arch::fpu] FPU x87 + SSE/SSE2 activos (CR0={:#x}, CR4={:#x}, MXCSR={:#x}, FXSAVE64=512B)",
        cr0_after,
        cr4_after,
        mxcsr_default
    );
}
