use core::sync::atomic::{compiler_fence, AtomicU64, Ordering};

const GP_VECTOR: u64 = 13;
const MAX_CPUS: usize = 32;

static MSR_GP_RECOVERED_TOTAL: AtomicU64 = AtomicU64::new(0);
static MSR_GP_LAST_FAULT_MSR: AtomicU64 = AtomicU64::new(0);
static MSR_GP_PER_CPU: [AtomicU64; MAX_CPUS] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MAX_CPUS]
};

#[derive(Clone, Copy, Debug)]
pub struct MsrSelfTestReport {
    pub hwcr: Option<u64>,
    pub therm_status: Option<u64>,
    pub x2apic_probe: Option<u64>,
    pub wrmsr_gp_ok: bool,
    pub gp_before: u64,
    pub gp_after: u64,
}

/// General-purpose registers saved by `__isr_common_exception` in `idt.rs`
/// (from `[rsp + 0]` = `rax` through `[rsp + 112]` = `r15`).
#[repr(C)]
pub struct SavedGprs {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

#[inline]
pub fn msr_gp_recovered_count() -> u64 {
    MSR_GP_RECOVERED_TOTAL.load(Ordering::Relaxed)
}

#[inline]
pub fn last_fault_msr() -> u64 {
    MSR_GP_LAST_FAULT_MSR.load(Ordering::Relaxed)
}

#[inline]
fn current_cpu_slot() -> usize {
    super::current_cpu_id() % MAX_CPUS
}

/// Attempts Ring 0 recovery from `#GP` caused by `rdmsr` (`0F 32`) or `wrmsr` (`0F 30`).
/// Advances `rip` past the 2-byte instruction, zeroes `edx:eax` on `rdmsr`, and records
/// the recovered fault per-CPU and globally so `rdmsr_safe`/`wrmsr_safe` never confuse `0`
/// with a fault.
pub(crate) fn try_recover_msr_gp(
    vector: u64,
    cs: u64,
    rip: &mut u64,
    gprs: &mut SavedGprs,
) -> bool {
    if vector != GP_VECTOR || (cs & 0b11) != 0 {
        return false;
    }

    let addr = *rip;
    // Higher-half canonical kernel instruction check before dereferencing opcodes.
    if addr < 0xFFFF_8000_0000_0000 || addr > 0xFFFF_FFFF_FFFF_FFFD {
        return false;
    }

    // SAFETY: `addr` was executing in Ring 0 higher-half code and triggered synchronous `#GP`,
    // meaning instruction fetch mapped at least the opcode bytes at `addr..addr+2`.
    let (op0, op1) = unsafe {
        (
            core::ptr::read_volatile(addr as *const u8),
            core::ptr::read_volatile((addr + 1) as *const u8),
        )
    };

    if op0 == 0x0F && op1 == 0x32 {
        gprs.rax = 0;
        gprs.rdx = 0;
        *rip = addr + 2;
        MSR_GP_LAST_FAULT_MSR.store(gprs.rcx & 0xFFFF_FFFF, Ordering::Release);
        MSR_GP_PER_CPU[current_cpu_slot()].fetch_add(1, Ordering::Release);
        MSR_GP_RECOVERED_TOTAL.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    if op0 == 0x0F && op1 == 0x30 {
        *rip = addr + 2;
        MSR_GP_LAST_FAULT_MSR.store(gprs.rcx & 0xFFFF_FFFF, Ordering::Release);
        MSR_GP_PER_CPU[current_cpu_slot()].fetch_add(1, Ordering::Release);
        MSR_GP_RECOVERED_TOTAL.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    false
}

/// Reads a 64-bit Model-Specific Register safely in Ring 0.
/// Returns `None` if the CPU or hypervisor raises `#GP` instead of halting the kernel.
pub fn rdmsr_safe(msr: u32) -> Option<u64> {
    super::without_interrupts(|| {
        let slot = current_cpu_slot();
        let before = MSR_GP_PER_CPU[slot].load(Ordering::Acquire);
        compiler_fence(Ordering::SeqCst);

        let low: u32;
        let high: u32;
        // SAFETY: Privileged Ring 0 `rdmsr`; `#GP` is intercepted by `try_recover_msr_gp`,
        // which increments `MSR_GP_PER_CPU[slot]` and skips the 2-byte instruction.
        unsafe {
            core::arch::asm!(
                "rdmsr",
                in("ecx") msr,
                out("eax") low,
                out("edx") high,
                options(nostack, preserves_flags)
            );
        }

        compiler_fence(Ordering::SeqCst);
        let after = MSR_GP_PER_CPU[slot].load(Ordering::Acquire);
        if after != before {
            None
        } else {
            Some(((high as u64) << 32) | (low as u64))
        }
    })
}

/// Writes a 64-bit Model-Specific Register safely in Ring 0.
/// Returns `false` if the CPU or hypervisor raises `#GP`.
pub fn wrmsr_safe(msr: u32, val: u64) -> bool {
    super::without_interrupts(|| {
        let slot = current_cpu_slot();
        let before = MSR_GP_PER_CPU[slot].load(Ordering::Acquire);
        compiler_fence(Ordering::SeqCst);

        let low = val as u32;
        let high = (val >> 32) as u32;
        // SAFETY: Privileged Ring 0 `wrmsr`; `#GP` is intercepted by `try_recover_msr_gp`.
        unsafe {
            core::arch::asm!(
                "wrmsr",
                in("ecx") msr,
                in("eax") low,
                in("edx") high,
                options(nostack, preserves_flags)
            );
        }

        compiler_fence(Ordering::SeqCst);
        let after = MSR_GP_PER_CPU[slot].load(Ordering::Acquire);
        after == before
    })
}

/// Probes hypervisor-restricted MSR reads (`0xC001_0015`, `0x19C`, `0x802`) and a reserved-bit
/// `wrmsr` on `IA32_EFER` in Ring 0 to verify live `#GP` trap recovery for both `0F 32` and `0F 30`.
pub fn probe_msr_gp_recovery_selftest() -> MsrSelfTestReport {
    let gp_before = msr_gp_recovered_count();
    let hwcr = rdmsr_safe(0xC001_0015);
    let therm_status = rdmsr_safe(0x0000_019C);
    let x2apic_probe = rdmsr_safe(0x0000_0802);

    let mut wrmsr_gp_ok = false;
    if let Some(efer) = rdmsr_safe(0xC000_0080) {
        if (efer & (1u64 << 63)) == 0 {
            let accepted = wrmsr_safe(0xC000_0080, efer | (1u64 << 63));
            if accepted {
                let _ = wrmsr_safe(0xC000_0080, efer);
            } else {
                wrmsr_gp_ok = true;
            }
        }
    }

    let gp_after = msr_gp_recovered_count();
    MsrSelfTestReport {
        hwcr,
        therm_status,
        x2apic_probe,
        wrmsr_gp_ok,
        gp_before,
        gp_after,
    }
}
