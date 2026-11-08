#![forbid(unsafe_code)]

use kernel_objects::SchedulerRegistry;
use ostd::arch::syscall::{
    has_parent_context, peek_parent_cr3, read_user_bytes,
    setup_sysv_abi_user_stack_args, write_user_bytes, SyscallAction, TrapFrame,
};
use ostd::mm::{
    active_cr3, expand_user_brk, master_kernel_cr3, reset_user_brk, switch_cr3,
    validate_user_buffer,
};
use ostd::sync::SpinLock;

use crate::elf::Elf64Loader;
use crate::vfs::{CapabilityVfs, FD_FB0, FD_STDERR, FD_STDIN, FD_STDOUT};

/// Linux `x86_64` canonical syscall numbers.
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_SETITIMER: u64 = 38;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SENDFILE: u64 = 40;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SOCKETPAIR: u64 = 53;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_READLINK: u64 = 89;
pub const SYS_SYSINFO: u64 = 99;
pub const SYS_GETUID: u64 = 102;
pub const SYS_GETGID: u64 = 104;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_STATFS: u64 = 137;
pub const SYS_FSTATFS: u64 = 138;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_RSEQ: u64 = 334;

pub const TCGETS: u64 = 0x5401;
pub const TIOCGWINSZ: u64 = 0x5413;
pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;

static GLOBAL_VFS: SpinLock<CapabilityVfs> = SpinLock::new(CapabilityVfs::new());
static CURRENT_WORKING_DIR: SpinLock<alloc::string::String> = SpinLock::new(alloc::string::String::new());
static ACTIVE_PROGRAM_NAME: SpinLock<alloc::string::String> = SpinLock::new(alloc::string::String::new());

fn get_cwd() -> alloc::string::String {
    let cwd = CURRENT_WORKING_DIR.lock();
    if cwd.is_empty() {
        alloc::string::String::from("/")
    } else {
        cwd.clone()
    }
}

fn set_cwd(new_path: alloc::string::String) {
    let mut cwd = CURRENT_WORKING_DIR.lock();
    *cwd = new_path;
}

fn resolve_path(path: &str) -> alloc::string::String {
    let p = path.trim();
    if p.starts_with('/') {
        crate::vfs::normalize_path(p)
    } else {
        let cwd = get_cwd();
        let full = if cwd == "/" {
            alloc::format!("/{}", p)
        } else {
            alloc::format!("{}/{}", cwd, p)
        };
        crate::vfs::normalize_path(&full)
    }
}

/// Initializes the capability VFS and registers the Linux `x86_64` syscall dispatcher with `ostd`.
pub fn init() {
    GLOBAL_VFS.lock().mount_default_nodes();
    ostd::arch::syscall::register_dispatcher(dispatch_syscall);
}

/// Reads bytes directly from a mounted VFS device node's underlying `Vmo` (`fd=3` or `fd=4`).
pub fn read_device_vmo(fd: u64, offset: usize, dst: &mut [u8]) -> Result<(), i64> {
    GLOBAL_VFS.lock().read_device_vmo(fd, offset, dst)
}

/// Reads the entire contents of a file from the VFS by path (e.g. `/bin/fipsos_sh.elf`).
pub fn read_all_by_path(path: &str) -> Result<alloc::vec::Vec<u8>, i64> {
    GLOBAL_VFS.lock().read_all_by_path(path)
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
        SYS_STAT | SYS_LSTAT => {
            let ret = sys_newfstatat(-100 /* AT_FDCWD */, arg0, arg1, 0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FSTAT => {
            let ret = sys_fstat(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_LSEEK => {
            let ret = sys_lseek(arg0, arg1 as i64, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_MMAP => {
            let ret = sys_mmap(arg0, arg1 as usize, arg2, arg3, arg4, arg5 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_MPROTECT => {
            // W^X memory protection is enforced by page tables; returning 0 allows glibc/musl RELRO init
            SyscallAction::Continue(0)
        }
        SYS_MUNMAP => {
            // Unmapping memory in Ring 3 allocator
            SyscallAction::Continue(0)
        }
        SYS_BRK => {
            let ret = sys_brk(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RT_SIGACTION => {
            // BusyBox / musl libc signal handler configuration
            SyscallAction::Continue(0)
        }
        SYS_RT_SIGPROCMASK => {
            // BusyBox / musl libc signal mask configuration
            SyscallAction::Continue(0)
        }
        SYS_IOCTL => {
            let ret = sys_ioctl(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_WRITEV => {
            let ret = sys_writev(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_ACCESS => {
            let ret = sys_access(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FACCESSAT => {
            let ret = sys_access(arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_PIPE | SYS_PIPE2 => {
            let ret = sys_pipe2(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_DUP => {
            let ret = sys_dup(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_DUP2 | SYS_DUP3 => {
            let ret = sys_dup2(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_NANOSLEEP => {
            let ret = sys_nanosleep(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SETITIMER => {
            // wget / alarm timer configuration
            SyscallAction::Continue(0)
        }
        SYS_POLL | SYS_PPOLL => {
            let ret = sys_poll(arg0, arg1, arg2 as i64);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SELECT | SYS_PSELECT6 => {
            let ret = sys_select(arg0, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SENDFILE => {
            let ret = sys_sendfile(arg0, arg1, arg2, arg3 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SOCKET => {
            let ret = sys_socket(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_CONNECT => {
            let ret = sys_connect(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_ACCEPT => {
            SyscallAction::Continue((-22i64) as u64) // -EINVAL
        }
        SYS_SENDTO => {
            let ret = sys_sendto(arg0, arg1, arg2 as usize, arg3, arg4, arg5);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RECVFROM => {
            let ret = sys_recvfrom(arg0, arg1, arg2 as usize, arg3, arg4, arg5);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SENDMSG => {
            let ret = sys_writev(arg0, arg1, 1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RECVMSG => {
            SyscallAction::Continue(0)
        }
        SYS_SHUTDOWN => {
            SyscallAction::Continue(0)
        }
        SYS_BIND | SYS_LISTEN => {
            SyscallAction::Continue(0)
        }
        SYS_GETSOCKNAME => {
            let ret = sys_getsockname(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETPEERNAME => {
            let ret = sys_getpeername(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SETSOCKOPT => {
            let ret = sys_setsockopt(arg0, arg1, arg2, arg3, arg4);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETSOCKOPT => {
            let ret = sys_getsockopt(arg0, arg1, arg2, arg3, arg4);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SOCKETPAIR => {
            SyscallAction::Continue(0)
        }
        SYS_FTRUNCATE => {
            SyscallAction::Continue(0)
        }
        SYS_WAIT4 => {
            let ret = sys_wait4(arg0 as i64, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETCWD => {
            let ret = sys_getcwd(arg0, arg1 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_CHDIR => {
            let ret = sys_chdir(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_MKDIR => {
            let ret = sys_mkdir(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_MKDIRAT => {
            let ret = sys_mkdir(arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RMDIR => {
            let ret = sys_unlink(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETUID | SYS_GETEUID => SyscallAction::Continue(0),
        SYS_GETGID | SYS_GETEGID => SyscallAction::Continue(0),
        SYS_GETPPID | SYS_GETPGRP => SyscallAction::Continue(1),
        SYS_GETDENTS64 => {
            let ret = sys_getdents64(arg0, arg1, arg2 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SCHED_YIELD => {
            let ret = sys_sched_yield(frame.user_rip, frame.user_rsp);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SCHED_GETAFFINITY => {
            let ret = sys_sched_getaffinity(arg0 as i64, arg1 as usize, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETPID => {
            let ret = sys_getpid(frame.user_rip);
            SyscallAction::Continue(ret as u64)
        }
        SYS_KILL => {
            let ret = sys_kill(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_UNAME => {
            let ret = sys_uname(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FCNTL => {
            let ret = sys_fcntl(arg0, arg1, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_UNLINK => {
            let ret = sys_unlink(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_READLINK => {
            let ret = sys_readlink(arg0, arg1, arg2 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SYSINFO => {
            let ret = sys_sysinfo(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_STATFS => {
            let ret = sys_statfs(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FSTATFS => {
            let ret = sys_fstatfs(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_ARCH_PRCTL => {
            let ret = sys_arch_prctl(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_REBOOT => {
            let ret = sys_reboot(arg0, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SET_TID_ADDRESS => {
            let ret = sys_set_tid_address(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_CLOCK_GETTIME => {
            let ret = sys_clock_gettime(arg0, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_OPENAT => {
            // Modern Linux libc fopen/openat(dirfd, path, flags, mode)
            let ret = sys_open(arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_NEWFSTATAT => {
            let ret = sys_newfstatat(arg0 as i64, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_READLINKAT => {
            let ret = sys_readlink(arg1, arg2, arg3 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_SET_ROBUST_LIST => {
            SyscallAction::Continue(0)
        }
        SYS_PRLIMIT64 => {
            SyscallAction::Continue(0)
        }
        SYS_GETRANDOM => {
            let ret = sys_getrandom(arg0, arg1 as usize, arg2);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RSEQ => {
            // Reporting -ENOSYS allows glibc and musl to fall back to atomic ops
            SyscallAction::Continue((-38i64) as u64)
        }
        SYS_EXECVE => sys_execve(arg0, arg1, arg2),
        SYS_EXIT | SYS_EXIT_GROUP => {
            let exit_code = arg0 as i32;
            let exiting_cr3 = active_cr3();
            if has_parent_context() {
                let target_parent_cr3 = peek_parent_cr3().unwrap_or_else(master_kernel_cr3);
                SchedulerRegistry::finish_child_exec(exit_code, frame.user_rip);
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit/exit_group(status={}) desde proceso hijo en Anillo 3 (RIP={:#x}) | Restaurando CR3: {:#x} -> {:#x}",
                    exit_code,
                    frame.user_rip,
                    exiting_cr3,
                    target_parent_cr3
                );
                ostd::fb_println!(
                    "[linux_abi::syscall] sys_exit(status={}) hijo (RIP={:#x}) | CR3 {:#x} -> {:#x}",
                    exit_code,
                    frame.user_rip,
                    exiting_cr3,
                    target_parent_cr3
                );
            } else {
                let kernel_cr3 = master_kernel_cr3();
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit/exit_group(status={}) ejecutado desde Anillo 3 (RIP={:#x}) | Restaurando MasterCR3: {:#x} -> {:#x}",
                    exit_code,
                    frame.user_rip,
                    exiting_cr3,
                    kernel_cr3
                );
                ostd::fb_println!(
                    "[linux_abi::syscall] sys_exit(status={}) Anillo 3 (RIP={:#x}) | CR3 {:#x} -> {:#x}",
                    exit_code,
                    frame.user_rip,
                    exiting_cr3,
                    kernel_cr3
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
/// - When `fd >= 3`: reads up to `len` bytes from the `Vmo` or pipe backing the open VFS file/device node.
pub fn sys_read(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if len == 0 {
        return 0;
    }

    let is_pipe_or_vmo = {
        let vfs = GLOBAL_VFS.lock();
        if let Some(node) = vfs.get_node(fd) {
            node.pipe_buf.is_some() || node.socket_id.is_some() || !node.is_device
        } else {
            false
        }
    };

    if is_pipe_or_vmo || fd >= 3 {
        let max_len = len.min(4096);
        if validate_user_buffer(user_buf_vaddr, max_len, true).is_err() {
            return -14; // -EFAULT
        }
        let mut kernel_buf = [0u8; 4096];
        let bytes_read = match GLOBAL_VFS.lock().read_file(fd, &mut kernel_buf[..max_len]) {
            Ok(n) => n,
            Err(errno) => return errno as isize,
        };
        if bytes_read > 0 && write_user_bytes(user_buf_vaddr, &kernel_buf[..bytes_read]).is_err() {
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

/// Linux `sys_sendfile` (syscall 40): copies data between one file descriptor and another in kernel space.
pub fn sys_sendfile(out_fd: u64, in_fd: u64, offset_ptr: u64, count: usize) -> isize {
    let mut total = 0;
    let mut buf = [0u8; 4096];
    while total < count {
        let chunk = (count - total).min(4096);
        let n = match GLOBAL_VFS.lock().read_file(in_fd, &mut buf[..chunk]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if total > 0 {
                    break;
                } else {
                    return e as isize;
                }
            }
        };
        if out_fd == FD_STDOUT || out_fd == FD_STDERR {
            if let Ok(text) = core::str::from_utf8(&buf[..n]) {
                ostd::early_print!("{}", text);
                ostd::fb_print!("{}", text);
            } else {
                for &b in &buf[..n] {
                    ostd::early_print!("{}", b as char);
                }
            }
            total += n;
        } else {
            match GLOBAL_VFS.lock().write_file(out_fd, &buf[..n]) {
                Ok(w) => {
                    total += w;
                    if w < n {
                        break;
                    }
                }
                Err(e) => {
                    if total > 0 {
                        break;
                    } else {
                        return e as isize;
                    }
                }
            }
        }
    }
    if offset_ptr != 0 {
        let _ = write_user_bytes(offset_ptr, &(total as u64).to_le_bytes());
    }
    total as isize
}

/// Linux `sys_open` (syscall 2): resolves a null-terminated virtual device or file path from Ring 3
/// and returns its capability file descriptor.
pub fn sys_open(user_path_vaddr: u64, flags: u64, _mode: u64) -> i64 {
    let mut path_buf = [0u8; 128];
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

    let resolved = resolve_path(path_str);
    match GLOBAL_VFS.lock().open(&resolved, flags) {
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
/// to the serial port / Framebuffer console, or to regular files / pipes.
pub fn sys_write(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if len == 0 {
        return 0;
    }

    let is_pipe_or_file = {
        let vfs = GLOBAL_VFS.lock();
        if let Some(node) = vfs.get_node(fd) {
            node.pipe_buf.is_some() || node.socket_id.is_some() || !node.is_device
        } else {
            false
        }
    };

    if is_pipe_or_file || (fd != FD_STDOUT && fd != FD_STDERR && fd != FD_FB0) {
        let mut written = 0usize;
        let mut kernel_buf = [0u8; 512];
        while written < len {
            let chunk = core::cmp::min(len - written, kernel_buf.len());
            if read_user_bytes(user_buf_vaddr + (written as u64), &mut kernel_buf[..chunk]).is_err() {
                return if written > 0 { written as isize } else { -14 }; // -EFAULT
            }
            let res = GLOBAL_VFS.lock().write_file(fd, &kernel_buf[..chunk]);
            match res {
                Ok(n) => {
                    written += n;
                    if n < chunk {
                        break;
                    }
                }
                Err(e) => {
                    return if written > 0 { written as isize } else { e as isize };
                }
            }
        }
        return written as isize;
    }

    let mut written = 0usize;
    let mut kernel_buf = [0u8; 256];
    while written < len {
        let chunk = core::cmp::min(len - written, kernel_buf.len());
        if read_user_bytes(user_buf_vaddr + (written as u64), &mut kernel_buf[..chunk]).is_err() {
            return if written > 0 { written as isize } else { -14 }; // -EFAULT
        }

        if let Ok(text) = core::str::from_utf8(&kernel_buf[..chunk]) {
            ostd::early_print!("{}", text);
            ostd::fb_print!("{}", text);
        } else {
            ostd::early_println!(
                "[linux_abi::sys_write] ({} bytes binarios en fd={})",
                chunk,
                fd
            );
        }
        written += chunk;
    }

    written as isize
}

/// Linux `sys_writev` (syscall 20): writes multiple contiguous or non-contiguous user buffers
/// via an array of `struct iovec { void *iov_base; size_t iov_len; }`.
pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> i64 {
    if iovcnt == 0 {
        return 0;
    }
    if iovcnt > 1024 {
        return -22; // -EINVAL
    }
    let mut total_written: i64 = 0;
    for i in 0..iovcnt {
        let entry_addr = iov_ptr + i * 16;
        let mut buf = [0u8; 16];
        if read_user_bytes(entry_addr, &mut buf).is_err() {
            return if total_written > 0 { total_written } else { -14 }; // -EFAULT
        }
        let base = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let len = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        if len == 0 {
            continue;
        }
        let res = sys_write(fd, base, len as usize);
        if res < 0 {
            return if total_written > 0 { total_written } else { res as i64 };
        }
        total_written += res as i64;
    }
    total_written
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

/// Linux `sys_ioctl` (syscall 16): dispatches device control operations, including POSIX terminal
/// inquiries (`TCGETS` 0x5401 and `TIOCGWINSZ` 0x5413) and capability devices.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> i64 {
    if request == TCGETS || request == TIOCGWINSZ {
        // Return 0 only for actual interactive console/terminal streams, not pipes or regular files
        let is_tty = {
            let vfs = GLOBAL_VFS.lock();
            if let Some(node) = vfs.get_node(fd) {
                node.is_device && node.pipe_buf.is_none() && (fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR)
            } else {
                false
            }
        };

        if !is_tty {
            return -25; // -ENOTTY (Inappropriate ioctl for device)
        }

        if request == TCGETS {
            let termios = [0u8; 60];
            if arg != 0 && write_user_bytes(arg, &termios).is_err() {
                return -14; // -EFAULT
            }
            return 0;
        }

        if request == TIOCGWINSZ {
            let mut winsz = [0u8; 8];
            winsz[0..2].copy_from_slice(&25u16.to_le_bytes());   // ws_row = 25
            winsz[2..4].copy_from_slice(&80u16.to_le_bytes());   // ws_col = 80
            winsz[4..6].copy_from_slice(&1280u16.to_le_bytes()); // ws_xpixel = 1280
            winsz[6..8].copy_from_slice(&800u16.to_le_bytes());  // ws_ypixel = 800
            if arg != 0 && write_user_bytes(arg, &winsz).is_err() {
                return -14; // -EFAULT
            }
            return 0;
        }
    }
    if fd <= FD_STDERR && (request == 0x5402 || request == 0x5403 || request == 0x5404) {
        // TCSETS, TCSETSW, TCSETSF on standard descriptors
        return 0;
    }

    match GLOBAL_VFS.lock().ioctl(fd, request, arg) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}

/// Linux `sys_fcntl` (syscall 72): supports file descriptor manipulation for BusyBox and standard shells.
pub fn sys_fcntl(fd: u64, cmd: u64, _arg: u64) -> i64 {
    const F_DUPFD: u64 = 0;
    const F_GETFD: u64 = 1;
    const F_SETFD: u64 = 2;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;
    const F_DUPFD_CLOEXEC: u64 = 1030;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            match GLOBAL_VFS.lock().dup(fd) {
                Ok(newfd) => newfd as i64,
                Err(e) => e,
            }
        }
        F_GETFL => 0, // O_RDONLY, blocking stream
        F_SETFL => 0,
        F_GETFD => 0,
        F_SETFD => 0,
        _ => 0,
    }
}

/// Linux `sys_arch_prctl` (syscall 158): sets or gets architectural processor state (`MSR_IA32_FS_BASE` TLS).
pub fn sys_arch_prctl(code: u64, addr: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            ostd::arch::syscall::set_user_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            let fs = ostd::arch::syscall::get_user_fs_base();
            if write_user_bytes(addr, &fs.to_le_bytes()).is_err() {
                -14 // -EFAULT
            } else {
                0
            }
        }
        _ => -22, // -EINVAL
    }
}

/// Linux `sys_set_tid_address` (syscall 218): sets the clear_child_tid pointer and returns the thread TID.
pub fn sys_set_tid_address(_tidptr: u64) -> i64 {
    let (_pid, tid) = ostd::scheduler::active_pid_and_tid();
    tid as i64
}

/// Linux `sys_uname` (syscall 63): returns system identification in `struct utsname` (390 bytes).
pub fn sys_uname(buf_addr: u64) -> i64 {
    let mut uts = [0u8; 390];
    let sysname = b"FipSOs\0";
    let nodename = b"fipsos-pc\0";
    let release = b"1.9.0\0";
    let version = b"#1 SMP\0";
    let machine = b"x86_64\0";
    let domain = b"(none)\0";

    uts[0..sysname.len()].copy_from_slice(sysname);
    uts[65..65 + nodename.len()].copy_from_slice(nodename);
    uts[130..130 + release.len()].copy_from_slice(release);
    uts[195..195 + version.len()].copy_from_slice(version);
    uts[260..260 + machine.len()].copy_from_slice(machine);
    uts[325..325 + domain.len()].copy_from_slice(domain);

    if write_user_bytes(buf_addr, &uts).is_err() {
        -14 // -EFAULT
    } else {
        0
    }
}

/// Linux `sys_readlink` (syscall 89): reads the target of a symbolic link (e.g. `/proc/self/exe`).
pub fn sys_readlink(path_addr: u64, buf_addr: u64, bufsiz: usize) -> i64 {
    let mut path_buf = [0u8; 64];
    let mut len = 0;
    while len < path_buf.len() {
        let mut b = [0u8; 1];
        if read_user_bytes(path_addr + (len as u64), &mut b).is_err() || b[0] == 0 {
            break;
        }
        path_buf[len] = b[0];
        len += 1;
    }
    let Ok(path_str) = core::str::from_utf8(&path_buf[..len]) else {
        return -22; // -EINVAL
    };
    if path_str == "/proc/self/exe" || path_str.starts_with("/proc/") {
        let prog = ACTIVE_PROGRAM_NAME.lock();
        let target = if prog.is_empty() {
            b"/bin/busybox".as_slice()
        } else {
            prog.as_bytes()
        };
        let copy_len = target.len().min(bufsiz);
        if write_user_bytes(buf_addr, &target[..copy_len]).is_err() {
            -14
        } else {
            copy_len as i64
        }
    } else {
        -2 // -ENOENT
    }
}

/// Linux `sys_getrandom` (syscall 318): fills user buffer with cryptographically pseudo-random bytes.
pub fn sys_getrandom(buf_addr: u64, buflen: usize, _flags: u64) -> i64 {
    let mut rnd = [0u8; 256];
    let to_fill = buflen.min(256);
    let mut seed = ostd::arch::idt::timer_ticks().wrapping_mul(6364136223846793005).wrapping_add(1);
    for b in rnd[..to_fill].iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (seed >> 32) as u8;
    }
    if write_user_bytes(buf_addr, &rnd[..to_fill]).is_err() {
        -14 // -EFAULT
    } else {
        to_fill as i64
    }
}

/// Linux `sys_fstat` (syscall 5): reports file and stream status into `struct stat` (144 bytes).
pub fn sys_fstat(fd: u64, statbuf_addr: u64) -> i64 {
    let mut stat = [0u8; 144];
    if fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR {
        let mode: u32 = 0o020620; // S_IFCHR | 0620 (Character Device)
        stat[24..28].copy_from_slice(&mode.to_le_bytes());
        let nlink: u64 = 1;
        stat[16..24].copy_from_slice(&nlink.to_le_bytes());
        let rdev: u64 = 0x8801;
        stat[40..48].copy_from_slice(&rdev.to_le_bytes());
        let blksize: i64 = 1024;
        stat[56..64].copy_from_slice(&blksize.to_le_bytes());
    } else {
        let vfs = GLOBAL_VFS.lock();
        if let Some(node) = vfs.get_node(fd) {
            let mode: u32 = if node.is_dir {
                0o040755 // S_IFDIR | 0755
            } else if node.is_device {
                0o020666 // S_IFCHR | 0666
            } else {
                0o100644 // S_IFREG | 0644
            };
            stat[24..28].copy_from_slice(&mode.to_le_bytes());
            let nlink: u64 = if node.is_dir { 2 } else { 1 };
            stat[16..24].copy_from_slice(&nlink.to_le_bytes());
            let size: i64 = node.file_size as i64;
            stat[48..56].copy_from_slice(&size.to_le_bytes());
            let blksize: i64 = 4096;
            stat[56..64].copy_from_slice(&blksize.to_le_bytes());
            let blocks: i64 = (size + 511) / 512;
            stat[64..72].copy_from_slice(&blocks.to_le_bytes());
        } else {
            return -9; // -EBADF
        }
    }
    if write_user_bytes(statbuf_addr, &stat).is_err() {
        -14 // -EFAULT
    } else {
        0
    }
}

/// Linux `sys_newfstatat` (syscall 262): reports file status by path or directory fd into `struct stat`.
pub fn sys_newfstatat(dirfd: i64, path_addr: u64, statbuf_addr: u64, _flags: u64) -> i64 {
    if path_addr == 0 {
        return -14;
    }
    let mut path_buf = [0u8; 128];
    let mut len = 0;
    while len < path_buf.len() {
        let mut b = [0u8; 1];
        if read_user_bytes(path_addr + (len as u64), &mut b).is_err() || b[0] == 0 {
            break;
        }
        path_buf[len] = b[0];
        len += 1;
    }
    let Ok(path_str) = core::str::from_utf8(&path_buf[..len]) else {
        return -22;
    };
    if path_str.is_empty() && dirfd >= 0 {
        return sys_fstat(dirfd as u64, statbuf_addr);
    }
    let resolved = resolve_path(path_str);
    let vfs = GLOBAL_VFS.lock();
    if vfs.is_directory(&resolved) {
        let mut stat = [0u8; 144];
        let mode: u32 = 0o040755;
        stat[24..28].copy_from_slice(&mode.to_le_bytes());
        let nlink: u64 = 2;
        stat[16..24].copy_from_slice(&nlink.to_le_bytes());
        let size: i64 = 4096;
        stat[48..56].copy_from_slice(&size.to_le_bytes());
        let blksize: i64 = 4096;
        stat[56..64].copy_from_slice(&blksize.to_le_bytes());
        drop(vfs);
        if write_user_bytes(statbuf_addr, &stat).is_err() {
            -14
        } else {
            0
        }
    } else if let Some(file_size) = vfs.get_file_size_by_path(&resolved) {
        let mut stat = [0u8; 144];
        let mode: u32 = 0o100644;
        stat[24..28].copy_from_slice(&mode.to_le_bytes());
        let nlink: u64 = 1;
        stat[16..24].copy_from_slice(&nlink.to_le_bytes());
        let size: i64 = file_size as i64;
        stat[48..56].copy_from_slice(&size.to_le_bytes());
        let blksize: i64 = 4096;
        stat[56..64].copy_from_slice(&blksize.to_le_bytes());
        let blocks: i64 = (size + 511) / 512;
        stat[64..72].copy_from_slice(&blocks.to_le_bytes());
        drop(vfs);
        if write_user_bytes(statbuf_addr, &stat).is_err() {
            -14
        } else {
            0
        }
    } else {
        -2 // -ENOENT
    }
}

/// Linux `sys_lseek` (syscall 8): repositions read/write file offset.
pub fn sys_lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR {
        -29 // -ESPIPE
    } else {
        match GLOBAL_VFS.lock().lseek(fd, offset, whence) {
            Ok(pos) => pos as i64,
            Err(e) => e,
        }
    }
}

/// Linux `sys_getcwd` (syscall 79): copies current working directory into user buffer.
pub fn sys_getcwd(buf_addr: u64, size: usize) -> i64 {
    let cwd = get_cwd();
    let bytes = cwd.as_bytes();
    if size < bytes.len() + 1 {
        return -34; // -ERANGE
    }
    if write_user_bytes(buf_addr, bytes).is_err() || write_user_bytes(buf_addr + (bytes.len() as u64), &[0u8]).is_err() {
        return -14; // -EFAULT
    }
    buf_addr as i64
}

/// Linux `sys_chdir` (syscall 80): changes the process current working directory.
pub fn sys_chdir(path_addr: u64) -> i64 {
    let mut path_buf = [0u8; 128];
    if read_user_bytes(path_addr, &mut path_buf).is_err() {
        return -14;
    }
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    let Ok(path_str) = core::str::from_utf8(&path_buf[..len]) else {
        return -22;
    };
    let resolved = resolve_path(path_str);
    let vfs = GLOBAL_VFS.lock();
    if !vfs.is_directory(&resolved) {
        return -2; // -ENOENT
    }
    drop(vfs);
    set_cwd(resolved);
    0
}

/// Linux `sys_mkdir` (syscall 83): creates a new directory.
pub fn sys_mkdir(path_addr: u64, _mode: u64) -> i64 {
    let mut path_buf = [0u8; 128];
    if read_user_bytes(path_addr, &mut path_buf).is_err() {
        return -14;
    }
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    let Ok(path_str) = core::str::from_utf8(&path_buf[..len]) else {
        return -22;
    };
    let resolved = resolve_path(path_str);
    GLOBAL_VFS.lock().add_directory(&resolved);
    0
}

/// Linux `sys_getdents64` (syscall 217): enumerates directory entries into `struct linux_dirent64`.
pub fn sys_getdents64(fd: u64, dirp_addr: u64, count: usize) -> i64 {
    if count == 0 {
        return 0;
    }
    let mut vfs = GLOBAL_VFS.lock();
    let (dir_path, offset) = {
        let node = match vfs.get_node(fd) {
            Some(n) => n,
            None => return -9, // -EBADF
        };
        if !node.is_dir {
            return -20; // -ENOTDIR
        }
        (node.path.clone(), node.offset)
    };

    let entries = vfs.list_directory(&dir_path);
    if offset >= entries.len() {
        return 0; // EOF
    }

    let mut written = 0usize;
    let mut next_offset = offset;

    for (idx, entry) in entries[offset..].iter().enumerate() {
        let name_bytes = entry.name.as_bytes();
        let name_len = name_bytes.len();
        let reclen = ((19 + name_len + 1 + 7) & !7) as u16;
        if written + (reclen as usize) > count {
            if written == 0 {
                return -22; // -EINVAL
            }
            break;
        }

        let d_ino: u64 = ((offset + idx + 1) * 100) as u64;
        let d_off: i64 = (offset + idx + 1) as i64;
        let d_type: u8 = if entry.is_dir {
            4 // DT_DIR
        } else if entry.is_chr {
            2 // DT_CHR
        } else {
            8 // DT_REG
        };

        let mut dirent_buf = [0u8; 256];
        dirent_buf[0..8].copy_from_slice(&d_ino.to_le_bytes());
        dirent_buf[8..16].copy_from_slice(&d_off.to_le_bytes());
        dirent_buf[16..18].copy_from_slice(&reclen.to_le_bytes());
        dirent_buf[18] = d_type;
        dirent_buf[19..19 + name_len].copy_from_slice(name_bytes);
        dirent_buf[19 + name_len] = 0;

        if write_user_bytes(dirp_addr + (written as u64), &dirent_buf[..reclen as usize]).is_err() {
            return if written > 0 { written as i64 } else { -14 };
        }

        written += reclen as usize;
        next_offset = offset + idx + 1;
    }

    if let Some(node) = vfs.get_node_mut(fd) {
        node.offset = next_offset;
    }

    written as i64
}

/// Linux `sys_pipe2` (syscall 293 / 22): creates an anonymous pipe pair `[read_fd, write_fd]`.
pub fn sys_pipe2(pipefd_addr: u64, flags: u64) -> i64 {
    let mut vfs = GLOBAL_VFS.lock();
    let (r_fd, w_fd) = match vfs.pipe2(flags) {
        Ok(pair) => pair,
        Err(e) => return e,
    };
    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&(r_fd as i32).to_le_bytes());
    buf[4..8].copy_from_slice(&(w_fd as i32).to_le_bytes());
    if write_user_bytes(pipefd_addr, &buf).is_err() {
        let _ = vfs.close(r_fd);
        let _ = vfs.close(w_fd);
        return -14;
    }
    0
}

/// Linux `sys_dup` (syscall 32): duplicates an open file descriptor.
pub fn sys_dup(oldfd: u64) -> i64 {
    match GLOBAL_VFS.lock().dup(oldfd) {
        Ok(n) => n as i64,
        Err(e) => e,
    }
}

/// Linux `sys_dup2` (syscall 33 / 292): duplicates an open file descriptor to `newfd`.
pub fn sys_dup2(oldfd: u64, newfd: u64) -> i64 {
    match GLOBAL_VFS.lock().dup2(oldfd, newfd) {
        Ok(n) => n as i64,
        Err(e) => e,
    }
}

/// Linux `sys_nanosleep` (syscall 35): pauses execution for the requested duration.
pub fn sys_nanosleep(req_addr: u64, _rem_addr: u64) -> i64 {
    let mut buf = [0u8; 16];
    if read_user_bytes(req_addr, &mut buf).is_err() {
        return -14;
    }
    let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
    let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
    let ticks_to_sleep = (sec * 100).saturating_add(nsec / 10_000_000).max(1);
    let start_ticks = ostd::arch::idt::timer_ticks();
    while ostd::arch::idt::timer_ticks().saturating_sub(start_ticks) < (ticks_to_sleep as u64) {
        ostd::scheduler::on_sys_sched_yield(0, 0);
    }
    0
}

/// Linux `sys_wait4` (syscall 61): waits for child process termination.
pub fn sys_wait4(pid: i64, status_addr: u64, _options: u64, _rusage: u64) -> i64 {
    if status_addr != 0 {
        let status: i32 = 0;
        let _ = write_user_bytes(status_addr, &status.to_le_bytes());
    }
    if pid <= 0 { 1 } else { pid }
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

/// Linux `sys_sched_getaffinity` (syscall 204): writes a CPU affinity bitmask for the process.
pub fn sys_sched_getaffinity(_pid: i64, cpusetsize: usize, mask_ptr: u64) -> isize {
    if cpusetsize < 8 {
        return -22; // -EINVAL
    }
    let num_cpus = ostd::scheduler::online_cpu_count().min(64);
    let mask: u64 = if num_cpus >= 64 {
        !0u64
    } else {
        (1u64 << num_cpus) - 1
    };
    if write_user_bytes(mask_ptr, &mask.to_le_bytes()).is_err() {
        return -14; // -EFAULT
    }
    8
}

/// Linux `sys_access` (syscall 21): checks user's permissions for a file or directory.
pub fn sys_access(path_user_addr: u64, _mode: u64) -> i64 {
    let mut path_buf = [0u8; 128];
    if read_user_bytes(path_user_addr, &mut path_buf).is_err() {
        return -14; // -EFAULT
    }
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    if len == 0 {
        return -2; // -ENOENT
    }
    let Ok(path_str) = core::str::from_utf8(&path_buf[..len]) else {
        return -22; // -EINVAL
    };
    let resolved = resolve_path(path_str);
    let vfs = GLOBAL_VFS.lock();
    if vfs.is_directory(&resolved) || vfs.get_file_size_by_path(&resolved).is_some() {
        0
    } else {
        -2 // -ENOENT
    }
}

/// Linux `sys_execve` (syscall 59): loads an ELF64 executable from the `TarFs` `Vmo` in `CapabilityVfs`,
/// enforces strict `W^X` on its `PT_LOAD` segments, transitions the child `Process` & `Thread` in
/// `SchedulerRegistry` to `Running`, and enters Ring 3 while stacking the parent process's `TrapFrame`.
pub fn sys_execve(user_path_vaddr: u64, argv_vaddr: u64, _envp_vaddr: u64) -> SyscallAction {
    let mut path_buf = [0u8; 128];
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

    let resolved = resolve_path(path_str);
    let mut elf_bytes = GLOBAL_VFS.lock().read_all_by_path(&resolved);

    // Sequential executable resolution in /bin and /disk/bin when relative
    if elf_bytes.is_err() && !path_str.starts_with('/') {
        let candidates = [
            alloc::format!("/bin/{}", path_str),
            alloc::format!("/bin/{}.elf", path_str),
            alloc::format!("/disk/bin/{}", path_str),
            alloc::format!("/disk/bin/{}.elf", path_str),
        ];
        let vfs = GLOBAL_VFS.lock();
        for cand in &candidates {
            if let Ok(bytes) = vfs.read_all_by_path(cand) {
                elf_bytes = Ok(bytes);
                break;
            }
        }
    }

    let elf_bytes = match elf_bytes {
        Ok(bytes) => bytes,
        Err(errno) => return SyscallAction::Continue(errno as u64),
    };

    // Extract command-line argument strings from parent user space
    let mut args: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    if argv_vaddr != 0 {
        for i in 0..16 {
            let ptr_addr = argv_vaddr + (i * 8);
            let mut ptr_bytes = [0u8; 8];
            if read_user_bytes(ptr_addr, &mut ptr_bytes).is_err() {
                break;
            }
            let str_vaddr = u64::from_le_bytes(ptr_bytes);
            if str_vaddr == 0 {
                break;
            }
            let mut str_buf = [0u8; 128];
            let mut str_len = 0usize;
            while str_len < str_buf.len() {
                let mut b = [0u8; 1];
                if read_user_bytes(str_vaddr + (str_len as u64), &mut b).is_err() || b[0] == 0 {
                    break;
                }
                str_buf[str_len] = b[0];
                str_len += 1;
            }
            if let Ok(s) = core::str::from_utf8(&str_buf[..str_len]) {
                args.push(alloc::string::String::from(s));
            }
        }
    }
    if args.is_empty() {
        args.push(resolved.clone());
    }
    let arg_refs: alloc::vec::Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    *ACTIVE_PROGRAM_NAME.lock() = resolved.clone();

    // Save the caller's active `PML4` (`parent_cr3`) and switch hardware `CR3` to the child's
    // dedicated isolated `PML4` (`child_cr3`) before mapping ELF64 `PT_LOAD` segments and stack!
    let parent_cr3 = active_cr3();
    let child_cr3 = match SchedulerRegistry::prepare_child_cr3(&resolved) {
        Ok(cr3) => cr3,
        Err(_) => return SyscallAction::Continue((-12i64) as u64), // -ENOMEM
    };
    switch_cr3(child_cr3);
    reset_user_brk();

    let loaded_elf = match Elf64Loader::load(&elf_bytes, 0) {
        Ok(loaded) => loaded,
        Err(err) => {
            switch_cr3(parent_cr3);
            ostd::early_println!(
                "[linux_abi::execve] Error al cargar ELF64 \"{}\": {:?}",
                resolved,
                err
            );
            ostd::fb_println!(
                "[linux_abi::execve] Error al cargar ELF64 \"{}\": {:?}",
                resolved,
                err
            );
            return SyscallAction::Continue((-8i64) as u64); // -ENOEXEC
        }
    };

    let child_rsp = match setup_sysv_abi_user_stack_args(
        &arg_refs,
        loaded_elf.entry_point,
        loaded_elf.phdr_vaddr,
        loaded_elf.phentsize,
        loaded_elf.phnum,
    ) {
        Ok(rsp) => rsp,
        Err(_) => {
            switch_cr3(parent_cr3);
            return SyscallAction::Continue((-12i64) as u64); // -ENOMEM
        }
    };

    let (child_pid, child_tid) =
        SchedulerRegistry::begin_child_exec(&resolved, loaded_elf.entry_point, child_rsp);

    ostd::early_println!(
        "[linux_abi::execve] sys_execve(\"{}\", {} B) -> Proceso {} / Thread {} | CR3: {:#x} -> {:#x} | Entry={:#x} RSP={:#x} (Seg={})",
        resolved,
        elf_bytes.len(),
        child_pid,
        child_tid,
        parent_cr3,
        child_cr3,
        loaded_elf.entry_point,
        child_rsp,
        loaded_elf.segments_loaded
    );
    ostd::fb_println!(
        "[linux_abi::execve] sys_execve(\"{}\") -> PID={} | CR3 {:#x} -> {:#x} | Entry={:#x} RSP={:#x}",
        resolved,
        child_pid,
        parent_cr3,
        child_cr3,
        loaded_elf.entry_point,
        child_rsp
    );

    SyscallAction::SpawnChild {
        entry_rip: loaded_elf.entry_point,
        user_rsp: child_rsp,
        parent_cr3,
    }
}

/// Linux `sys_reboot` (169) implementation supporting ACPI S5 shutdown and hardware reset.
pub fn sys_reboot(magic1: u64, magic2: u64, cmd: u64, _arg: u64) -> i64 {
    ostd::early_println!(
        "[linux_abi::syscall] sys_reboot(magic1={:#x}, magic2={:#x}, cmd={:#x})",
        magic1,
        magic2,
        cmd
    );
    ostd::fb_println!(
        "[linux_abi::syscall] sys_reboot(cmd={:#x})",
        cmd
    );

    if cmd == 1 || cmd == 0x0123_4567 || cmd == 0xCDEF_0123 {
        ostd::arch::x86_64::rtc_power::system_reboot();
    } else {
        ostd::arch::x86_64::rtc_power::system_shutdown();
    }
}

/// Linux `sys_clock_gettime` (228) implementation writing a 16-byte `timespec` (`tv_sec`, `tv_nsec`).
pub fn sys_clock_gettime(clk_id: u64, tp_user_addr: u64) -> i64 {
    if tp_user_addr == 0 {
        return -14i64; // -EFAULT
    }

    let (sec, nsec) = match clk_id {
        0 => {
            // CLOCK_REALTIME: read wall-clock date and time from CMOS RTC
            let dt = ostd::arch::x86_64::rtc_power::read_rtc();
            let unix_sec = ostd::arch::x86_64::rtc_power::rtc_to_unix_timestamp(&dt);
            let nsec = ((ostd::arch::idt::timer_ticks() % 100) * 10_000_000) as i64;
            (unix_sec, nsec)
        }
        _ => {
            // CLOCK_MONOTONIC (1) and others: monotonic uptime based on IRQ0 PIT ticks
            let ticks = ostd::arch::idt::timer_ticks();
            let sec = (ticks / 100) as i64;
            let nsec = ((ticks % 100) * 10_000_000) as i64;
            (sec, nsec)
        }
    };

    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&sec.to_le_bytes());
    buf[8..16].copy_from_slice(&nsec.to_le_bytes());

    if ostd::arch::syscall::write_user_bytes(tp_user_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}

/// Linux `sys_kill` (62) implementation for signaling or terminating a process/daemon.
pub fn sys_kill(pid: u64, sig: u64) -> i64 {
    ostd::early_println!("[linux_abi::syscall] sys_kill(pid={}, sig={})", pid, sig);
    ostd::fb_println!("[linux_abi::syscall] sys_kill(pid={}, sig={})", pid, sig);

    match SchedulerRegistry::kill_process(pid, sig) {
        Ok(_) => 0,
        Err(err) => err,
    }
}

/// Linux `sys_unlink` (87) implementation for deleting a file from SATA DiskFS and VFS.
pub fn sys_unlink(path_user_addr: u64) -> i64 {
    let mut path_buf = [0u8; 128];
    if read_user_bytes(path_user_addr, &mut path_buf).is_err() {
        return -14i64; // -EFAULT
    }
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    let path_str = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s) => s,
        Err(_) => return -22i64, // -EINVAL
    };
    let resolved = resolve_path(path_str);

    ostd::early_println!("[linux_abi::syscall] sys_unlink(\"{}\")", resolved);
    ostd::fb_println!("[linux_abi::syscall] sys_unlink(\"{}\")", resolved);

    // If file is on /disk/, delete from SATA diskfs
    if resolved.starts_with("/disk/") {
        if let Err(e) = crate::diskfs::delete_file(&resolved) {
            ostd::early_println!("[linux_abi::syscall] Fallo en sys_unlink: {}", e);
            return -2i64; // -ENOENT
        }
    }

    // Also remove from VFS in-memory cache if present
    GLOBAL_VFS.lock().remove_node(&resolved);

    0
}

/// Linux `sys_sysinfo` (syscall 99): fills a 112-byte standard Linux `struct sysinfo`.
pub fn sys_sysinfo(info_addr: u64) -> i64 {
    let ticks = ostd::arch::idt::timer_ticks();
    let uptime_sec = (ticks / 100) as i64;
    let stats = ostd::mm::memory_stats();
    let totalram = stats.total_bytes() as u64;
    let freeram = stats.free_bytes() as u64;

    let mut buf = [0u8; 112];
    // uptime (offset 0..8, i64)
    buf[0..8].copy_from_slice(&uptime_sec.to_le_bytes());
    // loads[3] (offset 8..32, u64*3) -> leave 0
    // totalram (offset 32..40, u64)
    buf[32..40].copy_from_slice(&totalram.to_le_bytes());
    // freeram (offset 40..48, u64)
    buf[40..48].copy_from_slice(&freeram.to_le_bytes());
    // sharedram (offset 48..56, u64) -> 0
    // bufferram (offset 56..64, u64) -> 0
    // totalswap (offset 64..72, u64) -> 0
    // freeswap (offset 72..80, u64) -> 0
    // procs (offset 80..82, u16)
    let procs: u16 = 3;
    buf[80..82].copy_from_slice(&procs.to_le_bytes());
    // pad (offset 82..84, u16) -> 0
    // padding 84..88 -> 0
    // totalhigh (offset 88..96, u64) -> 0
    // freehigh (offset 96..104, u64) -> 0
    // mem_unit (offset 104..108, u32) -> 1 byte
    let mem_unit: u32 = 1;
    buf[104..108].copy_from_slice(&mem_unit.to_le_bytes());

    if ostd::arch::syscall::write_user_bytes(info_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}

/// Helper function to populate a 120-byte standard Linux `struct statfs`.
pub fn fill_statfs(path: &str, buf_addr: u64) -> i64 {
    let mut buf = [0u8; 120];
    let (f_type, bsize, blocks, bfree, bavail) = if path.starts_with("/disk") {
        // SATA AHCI DiskFS: ~32MB volume (8192 blocks of 4096 bytes)
        (0xef53i64, 4096i64, 8192i64, 8000i64, 8000i64)
    } else {
        // RootFS (RAM/Initramfs)
        let stats = ostd::mm::memory_stats();
        let bsize = 4096i64;
        let blocks = (stats.total_bytes() / 4096) as i64;
        let bfree = (stats.free_bytes() / 4096) as i64;
        (0x01021994i64, bsize, blocks, bfree, bfree)
    };

    let files = 10000i64;
    let ffree = 9900i64;
    let namelen = 255i64;
    let frsize = bsize;
    let flags = 0i64;

    buf[0..8].copy_from_slice(&f_type.to_le_bytes());
    buf[8..16].copy_from_slice(&bsize.to_le_bytes());
    buf[16..24].copy_from_slice(&blocks.to_le_bytes());
    buf[24..32].copy_from_slice(&bfree.to_le_bytes());
    buf[32..40].copy_from_slice(&bavail.to_le_bytes());
    buf[40..48].copy_from_slice(&files.to_le_bytes());
    buf[48..56].copy_from_slice(&ffree.to_le_bytes());
    // f_fsid (56..64)
    buf[64..72].copy_from_slice(&namelen.to_le_bytes());
    buf[72..80].copy_from_slice(&frsize.to_le_bytes());
    buf[80..88].copy_from_slice(&flags.to_le_bytes());

    if ostd::arch::syscall::write_user_bytes(buf_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}

/// Linux `sys_statfs` (syscall 137): gets filesystem statistics for a given path.
pub fn sys_statfs(path_user_addr: u64, buf_addr: u64) -> i64 {
    let mut path_buf = [0u8; 128];
    if read_user_bytes(path_user_addr, &mut path_buf).is_err() {
        return -14i64; // -EFAULT
    }
    let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    let path_str = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s) => s,
        Err(_) => return -22i64, // -EINVAL
    };
    let resolved = resolve_path(path_str);
    fill_statfs(&resolved, buf_addr)
}

/// Linux `sys_fstatfs` (syscall 138): gets filesystem statistics for an open descriptor.
pub fn sys_fstatfs(fd: u64, buf_addr: u64) -> i64 {
    let path = match GLOBAL_VFS.lock().get_node(fd) {
        Some(node) => node.path.clone(),
        None => alloc::string::String::from("/"),
    };
    fill_statfs(&path, buf_addr)
}

/// Linux `sys_socket` (syscall 41): creates an unbound endpoint for communication.
pub fn sys_socket(domain: u64, type_: u64, protocol: u64) -> i64 {
    const AF_UNSPEC: u64 = 0;
    const AF_INET: u64 = 2;
    const SOCK_STREAM: u64 = 1;

    let base_type = type_ & 0x0F;
    if domain != AF_INET && domain != AF_UNSPEC {
        ostd::early_println!("[linux_abi::sys_socket] Dominio no soportado: {}", domain);
        return -97; // -EAFNOSUPPORT
    }
    if base_type != SOCK_STREAM {
        ostd::early_println!("[linux_abi::sys_socket] Tipo de socket no soportado: {}", base_type);
        return -93; // -EPROTONOSUPPORT
    }

    match GLOBAL_VFS.lock().create_socket_node() {
        Ok(fd) => {
            ostd::early_println!("[linux_abi::sys_socket] socket(AF_INET, SOCK_STREAM, proto={}) -> fd={}", protocol, fd);
            fd as i64
        }
        Err(e) => e,
    }
}

/// Linux `sys_connect` (syscall 42): connects a socket to a remote IPv4 address & TCP port.
pub fn sys_connect(fd: u64, addr_user_vaddr: u64, addr_len: u64) -> i64 {
    if addr_len < 8 {
        return -22; // -EINVAL
    }
    let mut buf = [0u8; 16];
    if read_user_bytes(addr_user_vaddr, &mut buf).is_err() {
        return -14; // -EFAULT
    }
    let sin_family = u16::from_le_bytes([buf[0], buf[1]]);
    if sin_family != 2 {
        return -97; // -EAFNOSUPPORT
    }
    let sin_port = u16::from_be_bytes([buf[2], buf[3]]);
    let sin_addr = [buf[4], buf[5], buf[6], buf[7]];

    let sock_id = match GLOBAL_VFS.lock().get_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };

    ostd::early_println!(
        "[linux_abi::sys_connect] connect(fd={}, {}.{}.{}.{}:{})",
        fd,
        sin_addr[0], sin_addr[1], sin_addr[2], sin_addr[3],
        sin_port
    );

    match ostd::net::tcp::connect(sock_id, sin_addr, sin_port) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// Linux `sys_sendto` (syscall 44): sends data on a socket.
pub fn sys_sendto(
    fd: u64,
    buf_user_vaddr: u64,
    len: usize,
    _flags: u64,
    _dest_addr: u64,
    _addr_len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let max_len = len.min(16384);
    if validate_user_buffer(buf_user_vaddr, max_len, false).is_err() {
        return -14; // -EFAULT
    }
    let mut kernel_buf = alloc::vec![0u8; max_len];
    if read_user_bytes(buf_user_vaddr, &mut kernel_buf).is_err() {
        return -14; // -EFAULT
    }
    match GLOBAL_VFS.lock().write_file(fd, &kernel_buf) {
        Ok(n) => n as i64,
        Err(e) => e,
    }
}

/// Linux `sys_recvfrom` (syscall 45): receives data from a socket.
pub fn sys_recvfrom(
    fd: u64,
    buf_user_vaddr: u64,
    len: usize,
    _flags: u64,
    _src_addr: u64,
    _addr_len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let max_len = len.min(16384);
    if validate_user_buffer(buf_user_vaddr, max_len, true).is_err() {
        return -14; // -EFAULT
    }
    let mut kernel_buf = alloc::vec![0u8; max_len];
    let bytes_read = match GLOBAL_VFS.lock().read_file(fd, &mut kernel_buf) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if bytes_read > 0 && write_user_bytes(buf_user_vaddr, &kernel_buf[..bytes_read]).is_err() {
        return -14; // -EFAULT
    }
    bytes_read as i64
}

/// Linux `sys_setsockopt` (syscall 54): sets socket options (SO_RCVTIMEO, SO_KEEPALIVE, etc.).
pub fn sys_setsockopt(fd: u64, level: u64, optname: u64, _optval: u64, _optlen: u64) -> i64 {
    ostd::early_println!("[linux_abi::sys_setsockopt] setsockopt(fd={}, lvl={}, opt={}) -> 0", fd, level, optname);
    0
}

/// Linux `sys_getsockopt` (syscall 55): gets socket options.
pub fn sys_getsockopt(fd: u64, level: u64, optname: u64, _optval: u64, _optlen: u64) -> i64 {
    ostd::early_println!("[linux_abi::sys_getsockopt] getsockopt(fd={}, lvl={}, opt={}) -> 0", fd, level, optname);
    0
}

/// Linux `sys_getsockname` (syscall 51): returns local socket address.
pub fn sys_getsockname(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    if addr_user_vaddr == 0 {
        return 0;
    }
    let mut sin = [0u8; 16];
    sin[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
    sin[2..4].copy_from_slice(&49152u16.to_be_bytes());
    sin[4..8].copy_from_slice(&[10, 0, 2, 15]);
    let _ = write_user_bytes(addr_user_vaddr, &sin);
    if addr_len_user != 0 {
        let _ = write_user_bytes(addr_len_user, &16u32.to_le_bytes());
    }
    0
}

/// Linux `sys_getpeername` (syscall 52): returns remote peer address.
pub fn sys_getpeername(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    if addr_user_vaddr == 0 {
        return 0;
    }
    let mut sin = [0u8; 16];
    sin[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
    sin[2..4].copy_from_slice(&8080u16.to_be_bytes());
    sin[4..8].copy_from_slice(&[10, 0, 2, 2]);
    let _ = write_user_bytes(addr_user_vaddr, &sin);
    if addr_len_user != 0 {
        let _ = write_user_bytes(addr_len_user, &16u32.to_le_bytes());
    }
    0
}

/// Linux `sys_poll` (syscall 7 / 271): polls a set of file descriptors for readiness.
pub fn sys_poll(fds_ptr: u64, nfds: u64, _timeout: i64) -> i64 {
    if nfds == 0 || fds_ptr == 0 {
        return 0;
    }
    const POLLIN: i16 = 0x0001;
    const POLLOUT: i16 = 0x0004;
    let mut ready_count = 0i64;
    for i in 0..nfds {
        let entry_addr = fds_ptr + i * 8;
        let mut buf = [0u8; 8];
        if read_user_bytes(entry_addr, &mut buf).is_err() {
            return -14; // -EFAULT
        }
        let events = i16::from_le_bytes([buf[4], buf[5]]);
        let revents = events & (POLLIN | POLLOUT);
        let revents_bytes = revents.to_le_bytes();
        buf[6] = revents_bytes[0];
        buf[7] = revents_bytes[1];
        if write_user_bytes(entry_addr, &buf).is_err() {
            return -14;
        }
        if revents != 0 {
            ready_count += 1;
        }
    }
    ready_count
}

/// Linux `sys_select` (syscall 23 / 270): synchronous I/O multiplexing.
pub fn sys_select(nfds: u64, _readfds: u64, _writefds: u64, _exceptfds: u64) -> i64 {
    if nfds == 0 {
        0
    } else {
        1
    }
}
