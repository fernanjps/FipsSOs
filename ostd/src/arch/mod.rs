#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::{current_cpu_id, fpu, halt_loop, idt, init, keyboard, mouse, sensors, serial, syscall, topology, without_interrupts};

#[cfg(target_arch = "aarch64")]
pub use aarch64::{halt_loop, init, serial, syscall, topology};
