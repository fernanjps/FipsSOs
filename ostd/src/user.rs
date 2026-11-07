//! Safe Ring 3 User-Space Syscall Primitives and `_user_start` Runtime for `userland`.
//!
//! All `syscall` inline assembly (`0F 05`) and user-mapped pointer operations are
//! encapsulated here inside `ostd` so that `userland` (`libfipsos` and `/bin/npu_infer.elf`)
//! strictly enforces `#![forbid(unsafe_code)]`.

/// Canonical Lower-Half base address where `/bin/npu_infer.elf` is linked (`0x0050_0000`).
pub const USER_EXEC_BASE_VADDR: u64 = 0x0000_0000_0050_0000;

/// Linux `x86_64` canonical syscall numbers exposed to Ring 3 user programs.
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_MMAP: u64 = 9;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;

extern "Rust" {
    static __fipsos_user_main_entry: fn() -> i32;
}

/// Low-level Ring 3 ELF64 entry point placed at `0x0050_0000` by `linker_user_x86_64.ld`.
#[no_mangle]
#[link_section = ".text._user_start"]
pub extern "C" fn _user_start() -> ! {
    // SAFETY: `__fipsos_user_main_entry` is populated at link time by `ostd::user_main!`
    // and verified to be 8 bytes by `linker_user_x86_64.ld`.
    let main_fn: fn() -> i32 =
        unsafe { core::ptr::read_volatile(&raw const __fipsos_user_main_entry) };
    let exit_code = main_fn();
    sys_exit(exit_code)
}

/// Returns `true` if the CPU is currently executing in Ring 3 (`CPL == 3`).
#[inline]
pub fn is_ring3_cpl() -> bool {
    let cs: u16;
    // SAFETY: Reading the CS segment selector is valid in both Ring 0 and Ring 3.
    unsafe {
        core::arch::asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
    }
    (cs & 0b11) == 3
}

/// Invokes a 3-argument `x86_64` Linux `syscall` (`rax=num`, `rdi=a0`, `rsi=a1`, `rdx=a2`).
#[inline(always)]
fn raw_syscall3(num: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    // SAFETY: Executes the architectural `syscall` instruction in Ring 3 following the
    // System V AMD64 / Linux x86_64 ABI (clobbers `rcx` and `r11`).
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

/// Invokes a 6-argument `x86_64` Linux `syscall` (`rax=num`, `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9`).
#[inline(always)]
fn raw_syscall6(num: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ret: i64;
    // SAFETY: Executes the architectural `syscall` instruction in Ring 3.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            in("r8") a4,
            in("r9") a5,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

/// Ring 3 `sys_write` (syscall 1): writes `buf` to file descriptor `fd`.
pub fn sys_write(fd: u64, buf: &[u8]) -> isize {
    raw_syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) as isize
}

/// Ring 3 `sys_read` (syscall 0): reads up to `buf.len()` bytes from file descriptor `fd`.
pub fn sys_read(fd: u64, buf: &mut [u8]) -> isize {
    raw_syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as isize
}

/// Ring 3 `sys_open` (syscall 2): opens a null-terminated path `path_cstr` with `flags`.
pub fn sys_open(path_cstr: &[u8], flags: u64) -> i64 {
    raw_syscall3(SYS_OPEN, path_cstr.as_ptr() as u64, flags, 0)
}

/// Ring 3 `sys_ioctl` (syscall 16): sends a control command `request` with `arg` to `fd`.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> i64 {
    raw_syscall3(SYS_IOCTL, fd, request, arg)
}

/// Ring 3 `sys_sched_yield` (syscall 24): voluntarily yields the current thread's CPU quantum
/// to the preemptive Round-Robin scheduler.
pub fn sys_sched_yield() -> i64 {
    raw_syscall3(SYS_SCHED_YIELD, 0, 0, 0)
}

/// Ring 3 `sys_getpid` (syscall 39): returns the `Koid` / `PID` of the calling `Process`.
pub fn sys_getpid() -> i64 {
    raw_syscall3(SYS_GETPID, 0, 0, 0)
}

/// Ring 3 `sys_mmap` (syscall 9): maps `len` bytes of capability device/file `fd` into Lower-Half User Space.
pub fn sys_mmap(addr: u64, len: usize, prot: u64, flags: u64, fd: u64, offset: usize) -> i64 {
    raw_syscall6(
        SYS_MMAP,
        addr,
        len as u64,
        prot,
        flags,
        fd,
        offset as u64,
    )
}

/// Ring 3 `sys_execve` (syscall 59): executes a child ELF64 binary at `path_cstr` and returns its exit code.
pub fn sys_execve(path_cstr: &[u8]) -> i64 {
    raw_syscall3(SYS_EXECVE, path_cstr.as_ptr() as u64, 0, 0)
}

/// Ring 3 `sys_exit` (syscall 60): terminates the active Ring 3 process with `status`.
pub fn sys_exit(status: i32) -> ! {
    let _ = raw_syscall3(SYS_EXIT, status as u64, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Safely renders an RGB32 Zero-Copy neural/graphics widget pattern into a user `mmap` buffer
/// mapped in the Lower-Half `0x1000_0000..0x4000_0000` range.
pub fn render_zero_copy_rgb32_widget(
    user_mmap_vaddr: u64,
    width: usize,
    height: usize,
    color_seed: u32,
    grid_stride: u32,
) -> Result<usize, &'static str> {
    let total_pixels = width.saturating_mul(height);
    let total_bytes = total_pixels.saturating_mul(4);
    if total_pixels == 0
        || user_mmap_vaddr < 0x1000_0000
        || user_mmap_vaddr.saturating_add(total_bytes as u64) > 0x4000_0000
        || (user_mmap_vaddr & 0x3) != 0
    {
        return Err("Rango mmap de Anillo 3 invalido para render_zero_copy_rgb32_widget");
    }

    let base_ptr = user_mmap_vaddr as *mut u32;
    for i in 0..total_pixels {
        let rev_idx = (total_pixels - i) as u32;
        let pixel = (rev_idx.wrapping_mul(grid_stride) ^ color_seed) & 0x00FF_FFFF;
        // SAFETY: `user_mmap_vaddr` is a validated 4-byte-aligned Lower-Half `sys_mmap` VMO region.
        unsafe {
            core::ptr::write_volatile(base_ptr.add(i), pixel);
        }
    }
    Ok(total_pixels)
}
