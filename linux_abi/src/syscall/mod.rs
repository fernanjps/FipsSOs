#![forbid(unsafe_code)]

use kernel_objects::SchedulerRegistry;
use ostd::arch::syscall::{has_parent_context, peek_parent_cr3, SyscallAction, TrapFrame};
use ostd::mm::{active_cr3, master_kernel_cr3};
use ostd::sync::SpinLock;

use crate::vfs::{cleanup_process_fds, CapabilityVfs};

pub mod fs;
pub mod fs_dir;
pub mod mem;
pub mod net_sys;
pub mod process;

pub use fs::*;
pub use mem::*;
pub use net_sys::*;
pub use process::*;

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
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_VFORK: u64 = 58;
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


pub(crate) static GLOBAL_VFS: SpinLock<CapabilityVfs> = SpinLock::new(CapabilityVfs::new());
pub(crate) static CURRENT_WORKING_DIR: SpinLock<alloc::string::String> = SpinLock::new(alloc::string::String::new());
pub(crate) static ACTIVE_PROGRAM_NAME: SpinLock<alloc::string::String> = SpinLock::new(alloc::string::String::new());

pub(crate) fn get_cwd() -> alloc::string::String {
    let cwd = CURRENT_WORKING_DIR.lock();
    if cwd.is_empty() {
        alloc::string::String::from("/")
    } else {
        cwd.clone()
    }
}


pub(crate) fn set_cwd(new_path: alloc::string::String) {
    let mut cwd = CURRENT_WORKING_DIR.lock();
    *cwd = new_path;
}


pub(crate) fn resolve_path(path: &str) -> alloc::string::String {
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
            let ret = sys_munmap(arg0, arg1 as usize);
            SyscallAction::Continue(ret as u64)
        }
        SYS_BRK => {
            let ret = sys_brk(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RT_SIGACTION => {
            let ret = sys_rt_sigaction(arg0, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
        }
        SYS_RT_SIGPROCMASK => {
            let ret = sys_rt_sigprocmask(arg0 as i32, arg1, arg2, arg3);
            SyscallAction::Continue(ret as u64)
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
            let ret = sys_rmdir(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_GETUID | SYS_GETEUID => SyscallAction::Continue(0),
        SYS_GETGID | SYS_GETEGID => SyscallAction::Continue(0),
        SYS_GETPPID => {
            let ppid = ostd::scheduler::get_active_ppid();
            SyscallAction::Continue(ppid)
        }
        SYS_GETPGRP => SyscallAction::Continue(1),
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
            let (active_pid, _) = ostd::scheduler::active_pid_and_tid();
            let ret = sys_kill(arg0, arg1);
            if arg0 == active_pid && (arg1 == 9 || (arg1 == 15 && ostd::scheduler::get_sigaction(active_pid, 15) == 0)) {
                let slot = ostd::scheduler::active_slot();
                cleanup_process_fds(slot);
                SyscallAction::ExitUser(128 + arg1 as i32)
            } else {
                SyscallAction::Continue(ret as u64)
            }
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
        SYS_CLONE => sys_clone(arg0, arg1, frame),
        SYS_FORK | SYS_VFORK => sys_fork(frame),
        SYS_EXECVE => sys_execve(arg0, arg1, arg2),
        SYS_EXIT | SYS_EXIT_GROUP => {
            let exit_code = arg0 as i32;
            let exiting_cr3 = active_cr3();
            let exiting_slot = ostd::scheduler::active_slot();
            cleanup_process_fds(exiting_slot);
            if ostd::arch::syscall::is_parent_fork() {
                let child_pid = ostd::scheduler::active_pid();
                ostd::scheduler::mark_process_exit(child_pid, exit_code);
                let target_parent_cr3 = peek_parent_cr3().unwrap_or_else(master_kernel_cr3);
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit/exit_group(status={}) desde proceso forkeado PID={} (RIP={:#x}) | Restaurando CR3: {:#x} -> {:#x}",
                    exit_code,
                    child_pid,
                    frame.user_rip,
                    exiting_cr3,
                    target_parent_cr3
                );
                ostd::fb_println!(
                    "[linux_abi::syscall] sys_exit(status={}) fork PID={} (RIP={:#x}) | CR3 {:#x} -> {:#x}",
                    exit_code,
                    child_pid,
                    frame.user_rip,
                    exiting_cr3,
                    target_parent_cr3
                );
            } else if has_parent_context() {
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

