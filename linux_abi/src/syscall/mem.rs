#![forbid(unsafe_code)]

use ostd::mm::{expand_user_brk, protect_user_segment};

use super::GLOBAL_VFS;

const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;

/// Linux `sys_mprotect` (syscall 10): applies real `W^X` page table protections (`R`/`W`/`X`)
/// over mapped Lower-Half User Space pages via `ostd::mm::protect_user_segment`.
pub fn sys_mprotect(addr: u64, len: usize, prot: u64) -> i64 {
    if (addr & 0xFFF) != 0 || addr >= 0x0000_7FFF_FFFF_F000 {
        return -22; // -EINVAL
    }
    if len == 0 {
        return 0;
    }
    let writable = (prot & PROT_WRITE) != 0;
    let executable = (prot & PROT_EXEC) != 0;
    if writable && executable {
        return -13; // -EACCES (strict W^X enforcement)
    }
    match protect_user_segment(addr, len, writable, executable) {
        Ok(()) => 0,
        Err(_) => -12, // -ENOMEM
    }
}

/// Linux `sys_brk` (syscall 12): queries (`new_addr == 0`) or expands the Ring 3 User Heap
/// starting at `0x0000_0000_0200_0000` with strict `W^X` (`R+W+NX`) 4 KiB page mappings.
pub fn sys_brk(new_addr: u64) -> i64 {
    if new_addr != 0 {
        let add_pages = ostd::mm::brk_additional_pages(new_addr);
        if crate::container::check_and_reserve_cgroup_pages(add_pages).is_err() {
            return expand_user_brk(0) as i64;
        }
    }
    let updated_brk = expand_user_brk(new_addr);
    if new_addr != 0 {
        crate::container::refresh_current_container_pages();
        ostd::early_println!(
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
    if (flags & crate::vfs::MAP_ANONYMOUS) != 0 || fd == u64::MAX || (fd as i64) == -1 {
        let add_pages = len.div_ceil(4096);
        if let Err(e) = crate::container::check_and_reserve_cgroup_pages(add_pages) {
            return e;
        }
    }
    match GLOBAL_VFS.lock().mmap(addr, len, prot, flags, fd, offset) {
        Ok(mapped_addr) => {
            crate::container::refresh_current_container_pages();
            mapped_addr as i64
        }
        Err(errno) => errno,
    }
}


/// Linux `sys_munmap` (syscall 11): unmaps Lower-Half user pages, flushes TLB, and frees physical frames.
pub fn sys_munmap(addr: u64, len: usize) -> i64 {
    match ostd::mm::unmap_user_pages(addr, len) {
        Ok(()) => {
            crate::container::refresh_current_container_pages();
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

