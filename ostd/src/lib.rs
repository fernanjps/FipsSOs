#![no_std]

extern crate alloc;

pub mod arch;
pub mod boot;
pub mod bus;
pub mod fb_console;
pub mod ffi_jail;
pub mod ipc;
pub mod mm;
pub mod net;
pub mod scheduler;
pub mod storage;
pub mod sync;
pub mod user;

use core::panic::PanicInfo;

/// Prints formatted text to the early architectural serial console without a newline.
#[macro_export]
macro_rules! early_print {
    ($($arg:tt)*) => {
        $crate::arch::serial::_print(::core::format_args!($($arg)*))
    };
}

/// Prints formatted text to the early architectural serial console with a newline.
#[macro_export]
macro_rules! early_println {
    () => {
        $crate::early_print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::early_print!("{}\n", ::core::format_args!($($arg)*))
    };
}

/// Prints formatted text to the visual linear Framebuffer console without a newline.
#[macro_export]
macro_rules! fb_print {
    ($($arg:tt)*) => {
        $crate::fb_console::_print(::core::format_args!($($arg)*))
    };
}

/// Prints formatted text to the visual linear Framebuffer console with a newline.
#[macro_export]
macro_rules! fb_println {
    () => {
        $crate::fb_print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::fb_print!("{}\n", ::core::format_args!($($arg)*))
    };
}

/// Registers the safe kernel entry point function from `kernel_main`.
///
/// The hardware `_start` symbol lives inside `ostd::boot` so that downstream
/// crates (`kernel_main`) can enforce `#![forbid(unsafe_code)]` without needing
/// `#[no_mangle]` or raw FFI attributes.
#[macro_export]
macro_rules! kernel_main {
    ($main_fn:path) => {
        #[used]
        pub static __FIPSOS_KERNEL_MAIN_ENTRY: fn() = $main_fn;
    };
}

/// Registers the safe Ring 3 user-space entry point function from `userland`.
#[macro_export]
macro_rules! user_main {
    ($main_fn:path) => {
        #[used]
        pub static __FIPSOS_USER_MAIN_ENTRY: fn() -> i32 = $main_fn;
    };
}

/// Halts the current CPU core indefinitely with interrupts disabled.
pub fn halt_loop() -> ! {
    arch::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if user::is_ring3_cpl() {
        let _ = user::sys_write(2, b"[userland PANIC] Abortando proceso de Anillo 3.\n");
        user::sys_exit(101);
    }
    early_println!("\n[FipSOs PANIC] {}", info);
    halt_loop()
}
