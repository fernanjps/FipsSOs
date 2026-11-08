pub mod audio;
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
