#![forbid(unsafe_code)]

use ostd::mm::expand_user_brk;

use super::GLOBAL_VFS;

/// Linux `sys_brk` (syscall 12): queries (`new_addr == 0`) or expands the Ring 3 User Heap
/// starting at `0x0000_0000_0200_0000` with strict `W^X` (`R+W+NX`) 4 KiB page mappings.
pub fn sys_brk(new_addr: u64) -> i64 {
    let updated_brk = expand_user_brk(new_addr);
    if new_addr != 0 {
        ostd::early_println!(
            "[linux_abi::syscall] sys_brk(new_addr={:#x}) -> UserHeap break={:#x} (W^X R+W+NX)",
            new_addr,
            updated_brk
        );
        ostd::fb_println!(
            "[linux_abi::syscall] sys_brk(new_addr={:#x}) -> UserHeap break={:#x} (W^X R+W+NX)",
            new_addr,
            updated_brk
        );
    }
    updated_brk as i64
}


/// Linux `sys_mmap` (syscall 9): maps a capability VFS device node (`/dev/dri/card0` or `/dev/accel/accel0`)
/// backed by a Zero-Copy `Vmo`, or anonymous memory (`MAP_ANONYMOUS`) into Ring 3 Lower-Half User Space (`0x1000_0000+`).
pub fn sys_mmap(
    addr: u64,
    len: usize,
    prot: u64,
    flags: u64,
    fd: u64,
    offset: usize,
) -> i64 {
    match GLOBAL_VFS.lock().mmap(addr, len, prot, flags, fd, offset) {
        Ok(mapped_addr) => mapped_addr as i64,
        Err(errno) => errno,
    }
}


/// Linux `sys_munmap` (syscall 11): unmaps Lower-Half user pages, flushes TLB, and frees physical frames.
pub fn sys_munmap(addr: u64, len: usize) -> i64 {
    match ostd::mm::unmap_user_pages(addr, len) {
        Ok(()) => {
            ostd::early_println!(
                "[linux_abi::syscall] sys_munmap(addr={:#x}, len={}) -> OK (0)",
                addr,
                len
            );
            0
        }
        Err(err) => {
            ostd::early_println!(
                "[linux_abi::syscall] sys_munmap(addr={:#x}, len={}) -> Error: {}",
                addr,
                len,
                err
            );
            -22 // -EINVAL
        }
    }
}

