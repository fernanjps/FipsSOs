pub mod fpu;
pub mod gdt;
pub mod idt;
pub mod keyboard;
pub mod mouse;
pub mod rtc_power;
pub mod sensors;
pub mod serial;
pub mod smp;
pub mod syscall;
pub mod topology;

/// Initializes x86_64 architectural tables (FPU/SSE2, GDT, TSS, IDT, PIC/PIT, Syscall MSRs, and SMP).
pub fn init() {
    fpu::init();
    gdt::init();
    idt::init();
    syscall::init();
    smp::init();
}

/// Disables interrupts and halts the x86_64 processor in an infinite loop.
pub fn halt_loop() -> ! {
    loop {
        // SAFETY: `cli` and `hlt` are privileged CPU instructions valid in Ring 0
        // when shutting down or idling the core after panic/completion.
        unsafe {
            core::arch::asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

/// Disables hardware interrupts on the current CPU core while executing closure `f`,
/// restoring the previous interrupt state (IF flag in RFLAGS) upon return.
#[inline]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let saved_rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            "cli",
            out(reg) saved_rflags,
            options(nomem, nostack, preserves_flags)
        );
    }
    let res = f();
    if (saved_rflags & (1 << 9)) != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    res
}

/// Returns the initial APIC/CPU ID of the executing CPU core using CPUID leaf 1.
#[inline]
pub fn current_cpu_id() -> usize {
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            out("eax") _,
            out("ecx") _,
            out("edx") _,
            options(nomem, preserves_flags),
        );
    }
    ((ebx >> 24) & 0xFF) as usize
}
