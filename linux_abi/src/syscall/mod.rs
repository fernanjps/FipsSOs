#![forbid(unsafe_code)]

use kernel_objects::SchedulerRegistry;
use ostd::arch::syscall::{has_parent_context, peek_parent_cr3, SyscallAction, TrapFrame};
use ostd::mm::{active_cr3, master_kernel_cr3};
use ostd::sync::SpinLock;

use crate::vfs::{cleanup_process_fds, CapabilityVfs};

pub mod compat;
pub mod dispatch_ext;
pub mod fd_ops;
pub mod fs;
pub mod fs_dir;
pub mod httpd_bg;
pub mod mem;
pub mod net_sys;
pub mod process;
pub mod sandbox;
pub mod sysinfo;

pub use compat::*;
pub use fd_ops::*;
pub use fs::*;
pub use mem::*;
pub use net_sys::*;
pub use process::*;
pub use sandbox::*;
pub use sysinfo::*;

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
pub const SYS_READV: u64 = 19;
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
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
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
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FCHMODAT: u64 = 268;
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


pub(crate) static GLOBAL_VFS: SpinLock<CapabilityVfs> = SpinLock::new(CapabilityVfs::new());

pub(crate) fn get_cwd() -> alloc::string::String {
    crate::vfs::get_process_cwd(ostd::scheduler::active_slot())
}

pub(crate) fn set_cwd(new_path: alloc::string::String) {
    crate::vfs::set_process_cwd(ostd::scheduler::active_slot(), new_path);
}

pub(crate) fn get_active_program_name() -> alloc::string::String {
    crate::vfs::get_process_program_name(ostd::scheduler::active_slot())
}

pub(crate) fn set_active_program_name(name: alloc::string::String) {
    crate::vfs::set_process_program_name(ostd::scheduler::active_slot(), name);
}


pub(crate) fn resolve_path(path: &str) -> alloc::string::String {
    let slot = ostd::scheduler::active_slot();
    let root = crate::vfs::get_process_root(slot);
    let cwd = get_cwd();
    crate::container::resolve_for_process(&root, &cwd, path)
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
        SYS_READ => SyscallAction::Continue(sys_read(arg0, arg1, arg2 as usize) as u64),
        SYS_WRITE => SyscallAction::Continue(sys_write(arg0, arg1, arg2 as usize) as u64),
        SYS_OPEN => SyscallAction::Continue(sys_open(arg0, arg1, arg2) as u64),
        SYS_CLOSE => SyscallAction::Continue(sys_close(arg0) as u64),
        SYS_STAT | SYS_LSTAT => {
            SyscallAction::Continue(sys_newfstatat(-100 /* AT_FDCWD */, arg0, arg1, 0) as u64)
        }
        SYS_FSTAT => SyscallAction::Continue(sys_fstat(arg0, arg1) as u64),
        SYS_LSEEK => SyscallAction::Continue(sys_lseek(arg0, arg1 as i64, arg2) as u64),
        SYS_MMAP => {
            SyscallAction::Continue(sys_mmap(arg0, arg1 as usize, arg2, arg3, arg4, arg5 as usize) as u64)
        }
        SYS_MPROTECT => {
            SyscallAction::Continue(sys_mprotect(arg0, arg1 as usize, arg2) as u64)
        }
        SYS_MUNMAP => SyscallAction::Continue(sys_munmap(arg0, arg1 as usize) as u64),
        SYS_BRK => SyscallAction::Continue(sys_brk(arg0) as u64),
        SYS_IOCTL => SyscallAction::Continue(sys_ioctl(arg0, arg1, arg2) as u64),
        SYS_READV => SyscallAction::Continue(sys_readv(arg0, arg1, arg2) as u64),
        SYS_WRITEV => SyscallAction::Continue(sys_writev(arg0, arg1, arg2) as u64),
        SYS_ACCESS => SyscallAction::Continue(sys_access(arg0, arg1) as u64),
        SYS_FACCESSAT => SyscallAction::Continue(sys_access(arg1, arg2) as u64),
        SYS_PIPE | SYS_PIPE2 => SyscallAction::Continue(sys_pipe2(arg0, arg1) as u64),
        SYS_DUP => SyscallAction::Continue(sys_dup(arg0) as u64),
        SYS_DUP2 | SYS_DUP3 => SyscallAction::Continue(sys_dup2(arg0, arg1) as u64),
        SYS_FSYNC | SYS_FDATASYNC => {
            let ret = fs_dir::sys_fsync(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_TRUNCATE => {
            let ret = fs_dir::sys_truncate(arg0, arg1 as i64);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FTRUNCATE => {
            let ret = fs_dir::sys_ftruncate(arg0, arg1 as i64);
            SyscallAction::Continue(ret as u64)
        }
        SYS_CHMOD => {
            let ret = fs_dir::sys_chmod(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FCHMOD => {
            let ret = fs_dir::sys_fchmod(arg0);
            SyscallAction::Continue(ret as u64)
        }
        SYS_FCHMODAT => {
            let ret = fs_dir::sys_fchmodat(arg0 as i64, arg1);
            SyscallAction::Continue(ret as u64)
        }
        SYS_UNLINKAT => {
            let ret = fs_dir::sys_unlinkat(arg0 as i64, arg1, arg2);
            SyscallAction::Continue(ret as u64)
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
        SYS_GETDENTS64 => {
            let ret = sys_getdents64(arg0, arg1, arg2 as usize);
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
        SYS_KILL => {
            let (active_pid, _) = ostd::scheduler::active_pid_and_tid();
            let ret = sys_kill(arg0, arg1);
            if arg0 == active_pid
                && (arg1 == 9
                    || (arg1 == 15 && ostd::scheduler::get_sigaction(active_pid, 15) == 0))
            {
                let slot = ostd::scheduler::active_slot();
                let exit_code = 128 + arg1 as i32;
                let exiting_cid = crate::vfs::get_process_container_id(slot);
                fs::finish_orphan_nodes(cleanup_process_fds(slot));
                crate::container::on_process_exit(exiting_cid, exit_code);
                if has_parent_context() {
                    ostd::scheduler::reset_slot_capabilities(slot);
                    if !ostd::arch::syscall::is_parent_fork() {
                        SchedulerRegistry::finish_child_exec(exit_code, frame.user_rip);
                    }
                }
                SyscallAction::ExitUser(exit_code)
            } else {
                SyscallAction::Continue(ret as u64)
            }
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
        SYS_OPENAT => {
            // Linux openat(dirfd, pathname, flags, mode)
            let ret = sys_openat(arg0 as i64, arg1, arg2, arg3);
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
        SYS_CLONE => sys_clone(arg0, arg1, frame),
        SYS_FORK | SYS_VFORK => sys_fork(frame),
        SYS_EXECVE => sys_execve(arg0, arg1, arg2),
        SYS_EXIT | SYS_EXIT_GROUP => {
            let exit_code = arg0 as i32;
            let exiting_cr3 = active_cr3();
            let exiting_slot = ostd::scheduler::active_slot();
            let exiting_cid = crate::vfs::get_process_container_id(exiting_slot);
            fs::finish_orphan_nodes(cleanup_process_fds(exiting_slot));
            crate::container::on_process_exit(exiting_cid, exit_code);
            if ostd::arch::syscall::is_parent_fork() {
                let child_pid = ostd::scheduler::active_pid();
                let target_parent_cr3 = peek_parent_cr3().unwrap_or_else(master_kernel_cr3);
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit/exit_group(status={}) desde proceso forkeado PID={} (RIP={:#x}) | Restaurando CR3: {:#x} -> {:#x}",
                    exit_code,
                    child_pid,
                    frame.user_rip,
                    exiting_cr3,
                    target_parent_cr3
                );
            } else if has_parent_context() {
                let target_parent_cr3 = peek_parent_cr3().unwrap_or_else(master_kernel_cr3);
                ostd::scheduler::reset_slot_capabilities(exiting_slot);
                SchedulerRegistry::finish_child_exec(exit_code, frame.user_rip);
                ostd::early_println!(
                    "[linux_abi::syscall] sys_exit/exit_group(status={}) desde proceso hijo en Anillo 3 (RIP={:#x}) | Restaurando CR3: {:#x} -> {:#x}",
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
            }
            SyscallAction::ExitUser(exit_code)
        }
        unknown => {
            if let Some(action) =
                dispatch_ext::dispatch_ext_syscall(unknown, arg0, arg1, arg2, arg3, arg4, arg5)
            {
                return action;
            }
            ostd::early_println!(
                "[linux_abi::syscall] Syscall no soportada: num={} @ RIP={:#x}",
                unknown,
                frame.user_rip
            );
            SyscallAction::Continue((-38i64) as u64) // -ENOSYS
        }
    }
}

