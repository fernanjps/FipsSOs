#![forbid(unsafe_code)]

use kernel_objects::SchedulerRegistry;
use ostd::arch::syscall::{
    has_parent_context, prepare_child_ring3_stack, read_user_bytes, write_user_bytes,
    SyscallAction, TrapFrame,
};
use ostd::mm::{expand_user_brk, reset_user_brk, validate_user_buffer};
use ostd::sync::SpinLock;

use crate::elf::Elf64Loader;
use crate::vfs::{CapabilityVfs, FD_FB0, FD_STDERR, FD_STDIN, FD_STDOUT};

/// Linux `x86_64` canonical syscall numbers.
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_MMAP: u64 = 9;
pub const SYS_BRK: u64 = 12;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;

static GLOBAL_VFS: SpinLock<CapabilityVfs> = SpinLock::new(CapabilityVfs::new());

/// Initializes the capability VFS and registers the Linux `x86_64` syscall dispatcher with `ostd`.
pub fn init() {
    GLOBAL_VFS.lock().mount_default_nodes();
    ostd::arch::syscall::register_dispatcher(dispatch_syscall);
}

/// Reads bytes directly from a mounted VFS device node's underlying `Vmo` (`fd=3` or `fd=4`).
pub fn read_device_vmo(fd: u64, offset: usize, dst: &mut [u8]) -> Result<(), i64> {
    GLOBAL_VFS.lock().read_device_vmo(fd, offset, dst)
}

/// Top-level Linux `x86_64` ABI syscall dispatcher invoked from `ostd::arch::syscall`.
pub fn dispatch_syscall(frame: &mut TrapFrame) -> SyscallAction {
    let syscall_num = frame.rax;
    let arg0 = frame.rdi;
    let arg1 = frame.rsi;
    let arg2 = frame.rdx;
    let arg3 = frame.r10;
    let arg4 = frame.r8;
    let arg5 = frame.r9;

    ostd::scheduler::record_syscall_frame(frame.user_rip, frame.user_rsp);

    match syscall_num {
        SYS_READ => {
            let ret = sys_read(arg0, arg1, arg2 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_WRITE => {
            let ret = sys_write(arg0, arg1, arg2 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_OPEN => {
            let ret = sys_open(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_CLOSE => {
            let ret = sys_close(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_MMAP => {
            let ret = sys_mmap(arg0, arg1 as usize, arg2, arg3, arg4, arg5 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_BRK => {
            let ret = sys_brk(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_IOCTL => {
            let ret = sys_ioctl(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SCHED_YIELD => {
            let ret = sys_sched_yield(frame.user_rip, frame.user_rsp);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETPID => {
            let ret = sys_getpid(frame.user_rip);
            SyscallAction::Continue(ret as u64)
        }
        SYS_EXECVE => sys_execve(arg0, arg1, arg2),
        SYS_EXIT => {
            let exit_code = arg0 as i32;
            if has_parent_context() {
                SchedulerRegistry::finish_child_exec(exit_code, frame.user_rip);
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit(status={}) desde proceso hijo en Anillo 3 (RIP={:#x}). Retornando al Shell padre.",
                    exit_code,
                    frame.user_rip
                );
                ostd::fb_println!(
                    "[linux_abi::syscall] sys_exit(status={}) desde proceso hijo (RIP={:#x}) -> Retornando al Shell.",
                    exit_code,
                    frame.user_rip
                );
            } else {
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit(status={}) ejecutado desde Anillo 3 (RIP={:#x}). Retornando al Kernel.",
                    exit_code,
                    frame.user_rip
                );
                ostd::fb_println!(
                    "[linux_abi::syscall] sys_exit(status={}) ejecutado desde Anillo 3 (RIP={:#x}). Retornando al Kernel.",
                    exit_code,
                    frame.user_rip
                );
            }
            SyscallAction::ExitUser(exit_code)
        }
        unknown => {
            ostd::early_println!(
                "[linux_abi::syscall] Syscall no soportada: num={} @ RIP={:#x}",
                unknown,
                frame.user_rip
            );
            SyscallAction::Continue((-38i64) as u64) // -ENOSYS
        }
    }
}

/// Linux `sys_read` (syscall 0):
/// - When `fd == FD_STDIN (0)`: reads an interactive line from `IRQ1` PS/2 keyboard / console with echo.
/// - When `fd >= 3`: reads up to `len` bytes from the `Vmo` backing the open VFS file/device node.
pub fn sys_read(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if len == 0 {
        return 0;
    }

    if fd >= 3 {
        let max_len = len.min(256);
        if validate_user_buffer(user_buf_vaddr, max_len, true).is_err() {
            return -14; // -EFAULT
        }
        let mut kernel_buf = [0u8; 256];
        let bytes_read = match GLOBAL_VFS.lock().read_file(fd, &mut kernel_buf[..max_len]) {
            Ok(n) => n,
            Err(errno) => return errno as isize,
        };
        if write_user_bytes(user_buf_vaddr, &kernel_buf[..bytes_read]).is_err() {
            return -14; // -EFAULT
        }
        return bytes_read as isize;
    }

    if fd != FD_STDIN {
        return -9; // -EBADF
    }

    let max_len = len.min(128);
    if validate_user_buffer(user_buf_vaddr, max_len, true).is_err() {
        return -14; // -EFAULT
    }

    let mut line_buf = [0u8; 128];
    let mut pos = 0usize;

    loop {
        let ch = ostd::arch::keyboard::read_char_blocking();
        match ch {
            b'\n' | b'\r' => {
                ostd::early_print!("\n");
                ostd::fb_print!("\n");
                if pos < max_len {
                    line_buf[pos] = b'\n';
                    pos += 1;
                }
                break;
            }
            0x08 | 0x7F => {
                if pos > 0 {
                    pos -= 1;
                    line_buf[pos] = 0;
                    ostd::early_print!("\x08 \x08");
                    ostd::fb_print!("\x08");
                }
            }
            byte if (0x20..=0x7E).contains(&byte) => {
                if pos + 1 < max_len {
                    line_buf[pos] = byte;
                    pos += 1;
                    let echo = [byte];
                    if let Ok(s) = core::str::from_utf8(&echo) {
                        ostd::early_print!("{}", s);
                        ostd::fb_print!("{}", s);
                    }
                }
            }
            _ => {}
        }
    }

    if write_user_bytes(user_buf_vaddr, &line_buf[..pos]).is_err() {
        return -14; // -EFAULT
    }

    pos as isize
}

/// Linux `sys_open` (syscall 2): resolves a null-terminated virtual device path from Ring 3
/// (`/dev/dri/card0`, `/dev/accel/accel0`, `/dev/fb0`) and returns its capability file descriptor.
pub fn sys_open(user_path_vaddr: u64, flags: u64, _mode: u64) -> i64 {
    let mut path_buf = [0u8; 64];
    let mut path_len = 0usize;

    for slot in &mut path_buf {
        let mut byte = [0u8; 1];
        if read_user_bytes(user_path_vaddr + (path_len as u64), &mut byte).is_err() {
            return -14; // -EFAULT
        }
        if byte[0] == 0 {
            break;
        }
        *slot = byte[0];
        path_len += 1;
    }

    if path_len == 0 {
        return -2; // -ENOENT
    }

    let Ok(path_str) = core::str::from_utf8(&path_buf[..path_len]) else {
        return -22; // -EINVAL
    };

    match GLOBAL_VFS.lock().open(path_str, flags) {
        Ok(fd) => fd as i64,
        Err(errno) => errno,
    }
}

/// Linux `sys_close` (syscall 3): validates and closes an open capability file descriptor.
pub fn sys_close(fd: u64) -> i64 {
    match GLOBAL_VFS.lock().close(fd) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}

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

/// Linux `sys_write` (syscall 1): writes bytes from a verified Ring 3 user buffer
/// (code/data segment, stack, or Zero-Copy `mmap` VMO region) to both the serial port
/// and the visual Framebuffer console (`FD_STDOUT`, `FD_STDERR`, or `/dev/fb0`).
pub fn sys_write(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if fd != FD_STDOUT && fd != FD_STDERR && fd != FD_FB0 {
        return -9; // -EBADF
    }
    if len == 0 {
        return 0;
    }

    let capped_len = len.min(256);
    let mut kernel_buf = [0u8; 256];
    if read_user_bytes(user_buf_vaddr, &mut kernel_buf[..capped_len]).is_err() {
        return -14; // -EFAULT
    }

    if let Ok(text) = core::str::from_utf8(&kernel_buf[..capped_len]) {
        ostd::early_print!("{}", text);
        ostd::fb_print!("{}", text);
    } else {
        ostd::early_println!(
            "[linux_abi::sys_write] ({} bytes binarios en fd={})",
            capped_len,
            fd
        );
    }

    capped_len as isize
}

/// Linux `sys_mmap` (syscall 9): maps a capability VFS device node (`/dev/dri/card0` or `/dev/accel/accel0`)
/// backed by a Zero-Copy `Vmo` into Ring 3 Lower-Half User Space (`0x1000_0000+`).
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

/// Linux `sys_ioctl` (syscall 16): dispatches device control operations on `/dev/dri/card0`
/// and `/dev/accel/accel0` to their Zero-Copy `Vmo` objects.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> i64 {
    match GLOBAL_VFS.lock().ioctl(fd, request, arg) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}

/// Linux `sys_sched_yield` (syscall 24): voluntarily yields the active thread's CPU quantum
/// to the preemptive Round-Robin scheduler and `npu-telemetryd`.
pub fn sys_sched_yield(user_rip: u64, user_rsp: u64) -> i64 {
    let (tid, switches) = ostd::scheduler::on_sys_sched_yield(user_rip, user_rsp);
    if switches <= 4 {
        ostd::early_println!(
            "[Scheduler::yield] sys_sched_yield() en TID=koid:{} (RIP={:#x}) -> Quantum cedido a npu-telemetryd (Switches={})",
            tid,
            user_rip,
            switches
        );
        ostd::fb_println!(
            "[Scheduler::yield] sys_sched_yield() TID=koid:{} (RIP={:#x}) -> npu-telemetryd (Sw={})",
            tid,
            user_rip,
            switches
        );
    }
    0
}

/// Linux `sys_getpid` (syscall 39): returns the `Koid` / `PID` of the calling Ring 3 `Process`.
pub fn sys_getpid(user_rip: u64) -> i64 {
    let (pid, tid) = ostd::scheduler::active_pid_and_tid();
    ostd::early_println!(
        "[linux_abi::syscall] sys_getpid() -> PID=koid:{} (TID=koid:{}, RIP={:#x})",
        pid,
        tid,
        user_rip
    );
    ostd::fb_println!(
        "[linux_abi::syscall] sys_getpid() -> PID=koid:{} (TID=koid:{}, RIP={:#x})",
        pid,
        tid,
        user_rip
    );
    pid as i64
}

/// Linux `sys_execve` (syscall 59): loads an ELF64 executable from the `TarFs` `Vmo` in `CapabilityVfs`,
/// enforces strict `W^X` on its `PT_LOAD` segments, transitions the child `Process` & `Thread` in
/// `SchedulerRegistry` to `Running`, and enters Ring 3 while stacking the parent process's `TrapFrame`.
pub fn sys_execve(user_path_vaddr: u64, _argv_vaddr: u64, _envp_vaddr: u64) -> SyscallAction {
    let mut path_buf = [0u8; 64];
    let mut path_len = 0usize;

    for slot in &mut path_buf {
        let mut byte = [0u8; 1];
        if read_user_bytes(user_path_vaddr + (path_len as u64), &mut byte).is_err() {
            return SyscallAction::Continue((-14i64) as u64); // -EFAULT
        }
        if byte[0] == 0 {
            break;
        }
        *slot = byte[0];
        path_len += 1;
    }

    if path_len == 0 {
        return SyscallAction::Continue((-2i64) as u64); // -ENOENT
    }

    let Ok(path_str) = core::str::from_utf8(&path_buf[..path_len]) else {
        return SyscallAction::Continue((-22i64) as u64); // -EINVAL
    };

    let elf_bytes = match GLOBAL_VFS.lock().read_all_by_path(path_str) {
        Ok(bytes) => bytes,
        Err(errno) => return SyscallAction::Continue(errno as u64),
    };

    let loaded_elf = match Elf64Loader::load(&elf_bytes, 0) {
        Ok(loaded) => loaded,
        Err(err) => {
            ostd::early_println!(
                "[linux_abi::execve] Error al cargar ELF64 \"{}\": {:?}",
                path_str,
                err
            );
            ostd::fb_println!(
                "[linux_abi::execve] Error al cargar ELF64 \"{}\": {:?}",
                path_str,
                err
            );
            return SyscallAction::Continue((-8i64) as u64); // -ENOEXEC
        }
    };

    let child_rsp = match prepare_child_ring3_stack() {
        Ok(rsp) => rsp,
        Err(_) => return SyscallAction::Continue((-12i64) as u64), // -ENOMEM
    };

    reset_user_brk();

    let (child_pid, child_tid) =
        SchedulerRegistry::begin_child_exec(path_str, loaded_elf.entry_point, child_rsp);

    ostd::early_println!(
        "[linux_abi::execve] sys_execve(\"{}\", {} B) -> Proceso {} / Thread {} (EntryPoint={:#x}, RSP={:#x}, Segmentos={}) activo en Anillo 3",
        path_str,
        elf_bytes.len(),
        child_pid,
        child_tid,
        loaded_elf.entry_point,
        child_rsp,
        loaded_elf.segments_loaded
    );
    ostd::fb_println!(
        "[linux_abi::execve] sys_execve(\"{}\", {} B) -> PID={} TID={} (Entry={:#x}, RSP={:#x})",
        path_str,
        elf_bytes.len(),
        child_pid,
        child_tid,
        loaded_elf.entry_point,
        child_rsp
    );

    SyscallAction::SpawnChild {
        entry_rip: loaded_elf.entry_point,
        user_rsp: child_rsp,
    }
}
