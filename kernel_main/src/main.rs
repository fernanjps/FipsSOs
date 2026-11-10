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
    dual_println!(
        "[FipSOs] Framekernel FipSOs v3.4 activo (SMP Multicore | Multi-USB xHCI/EHCI/OHCI | DHCP | TCP Remote :2222 | SATA AHCI)."
    );

    let (vmo, endpoint_b) = boot_checks::run_boot_diagnostics();
    process_launch::launch_system_processes(vmo, endpoint_b);
}
