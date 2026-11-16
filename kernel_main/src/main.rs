#![forbid(unsafe_code)]
#![no_std]
#![no_main]

extern crate alloc;

/// Prints to both the hardware serial port (`0x3F8`) and the visual Framebuffer console (`1280x800`).
#[macro_export]
macro_rules! dual_println {
    ($($arg:tt)*) => {{
        ostd::early_println!($($arg)*);
        ostd::fb_println!($($arg)*);
    }};
}

mod boot_checks;
mod process_launch;

ostd::kernel_main!(fipsos_main);

fn fipsos_main() {
    ostd::boot::init();
    linux_abi::init();
    ostd::scheduler::mark_boot_complete();
    dual_println!(
        "[FipSOs] Framekernel FipSOs v6.3 activo (SMP | Containers | FAT32 LBA 2048 | Clean GUI)."
    );

    boot_checks::run_boot_diagnostics();
    process_launch::launch_system_processes();
}
