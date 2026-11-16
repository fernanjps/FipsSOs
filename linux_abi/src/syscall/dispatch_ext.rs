#![forbid(unsafe_code)]

use ostd::arch::syscall::SyscallAction;

use super::{
    sys_accept, sys_arch_prctl, sys_bind, sys_clock_gettime, sys_clock_nanosleep, sys_connect,
    sys_fips_sandbox, sys_fstatfs, sys_getpeername, sys_getrandom, sys_getsockname, sys_getsockopt,
    sys_listen, sys_nanosleep, sys_poll, sys_prlimit64, sys_reboot, sys_recvfrom, sys_rt_sigaction,
    sys_rt_sigprocmask, sys_sched_getaffinity, sys_select, sys_sendfile, sys_sendto,
    sys_set_robust_list, sys_set_tid_address, sys_setsockopt, sys_socket, sys_statfs, sys_sysinfo,
    sys_uname, sys_writev, SYS_ACCEPT, SYS_ARCH_PRCTL, SYS_BIND, SYS_CLOCK_GETTIME,
    SYS_CLOCK_NANOSLEEP, SYS_CONNECT, SYS_FIPS_SANDBOX, SYS_FSTATFS, SYS_GETEGID, SYS_GETEUID,
    SYS_GETGID, SYS_GETPEERNAME, SYS_GETPGRP, SYS_GETPPID, SYS_GETRANDOM, SYS_GETSOCKNAME,
    SYS_GETSOCKOPT, SYS_GETUID, SYS_LISTEN, SYS_NANOSLEEP, SYS_POLL, SYS_PPOLL, SYS_PRLIMIT64,
    SYS_PSELECT6, SYS_REBOOT, SYS_RECVFROM, SYS_RECVMSG, SYS_RSEQ, SYS_RT_SIGACTION,
    SYS_RT_SIGPROCMASK, SYS_SCHED_GETAFFINITY, SYS_SELECT, SYS_SENDFILE, SYS_SENDMSG, SYS_SENDTO,
    SYS_SETITIMER, SYS_SETSOCKOPT, SYS_SET_ROBUST_LIST, SYS_SET_TID_ADDRESS, SYS_SHUTDOWN,
    SYS_SOCKET, SYS_SOCKETPAIR, SYS_STATFS, SYS_SYSINFO, SYS_UNAME,
};

/// Dispatches auxiliary POSIX, networking, signal, time, and sandbox syscalls.
pub fn dispatch_ext_syscall(
    syscall_num: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> Option<SyscallAction> {
    let action = match syscall_num {
        SYS_RT_SIGACTION => {
            SyscallAction::Continue(sys_rt_sigaction(arg0, arg1, arg2, arg3) as u64)
        }
        SYS_RT_SIGPROCMASK => {
            SyscallAction::Continue(sys_rt_sigprocmask(arg0 as i32, arg1, arg2, arg3) as u64)
        }
        SYS_NANOSLEEP => SyscallAction::Continue(sys_nanosleep(arg0, arg1) as u64),
        SYS_SETITIMER => SyscallAction::Continue(0),
        SYS_POLL | SYS_PPOLL => SyscallAction::Continue(sys_poll(arg0, arg1, arg2 as i64) as u64),
        SYS_SELECT | SYS_PSELECT6 => {
            SyscallAction::Continue(sys_select(arg0, arg1, arg2, arg3) as u64)
        }
        SYS_SENDFILE => {
            SyscallAction::Continue(sys_sendfile(arg0, arg1, arg2, arg3 as usize) as u64)
        }
        SYS_SOCKET => SyscallAction::Continue(sys_socket(arg0, arg1, arg2) as u64),
        SYS_CONNECT => SyscallAction::Continue(sys_connect(arg0, arg1, arg2) as u64),
        SYS_ACCEPT => SyscallAction::Continue(sys_accept(arg0, arg1, arg2) as u64),
        SYS_SENDTO => {
            SyscallAction::Continue(sys_sendto(arg0, arg1, arg2 as usize, arg3, arg4, arg5) as u64)
        }
        SYS_RECVFROM => {
            SyscallAction::Continue(sys_recvfrom(arg0, arg1, arg2 as usize, arg3, arg4, arg5) as u64)
        }
        SYS_SENDMSG => SyscallAction::Continue(sys_writev(arg0, arg1, 1) as u64),
        SYS_RECVMSG | SYS_SHUTDOWN | SYS_SOCKETPAIR => SyscallAction::Continue(0),
        SYS_BIND => SyscallAction::Continue(sys_bind(arg0, arg1, arg2) as u64),
        SYS_LISTEN => SyscallAction::Continue(sys_listen(arg0, arg1) as u64),
        SYS_GETSOCKNAME => SyscallAction::Continue(sys_getsockname(arg0, arg1, arg2) as u64),
        SYS_GETPEERNAME => SyscallAction::Continue(sys_getpeername(arg0, arg1, arg2) as u64),
        SYS_SETSOCKOPT => {
            SyscallAction::Continue(sys_setsockopt(arg0, arg1, arg2, arg3, arg4) as u64)
        }
        SYS_GETSOCKOPT => {
            SyscallAction::Continue(sys_getsockopt(arg0, arg1, arg2, arg3, arg4) as u64)
        }
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => SyscallAction::Continue(0),
        SYS_GETPPID => SyscallAction::Continue(ostd::scheduler::get_active_ppid()),
        SYS_GETPGRP => SyscallAction::Continue(1),
        SYS_SCHED_GETAFFINITY => {
            SyscallAction::Continue(sys_sched_getaffinity(arg0 as i64, arg1 as usize, arg2) as u64)
        }
        SYS_UNAME => SyscallAction::Continue(sys_uname(arg0) as u64),
        SYS_SYSINFO => SyscallAction::Continue(sys_sysinfo(arg0) as u64),
        SYS_STATFS => SyscallAction::Continue(sys_statfs(arg0, arg1) as u64),
        SYS_FSTATFS => SyscallAction::Continue(sys_fstatfs(arg0, arg1) as u64),
        SYS_ARCH_PRCTL => SyscallAction::Continue(sys_arch_prctl(arg0, arg1) as u64),
        crate::container::SYS_CHROOT => {
            SyscallAction::Continue(crate::container::sys_chroot(arg0) as u64)
        }
        SYS_REBOOT => SyscallAction::Continue(sys_reboot(arg0, arg1, arg2, arg3) as u64),
        crate::container::SYS_SETHOSTNAME => {
            SyscallAction::Continue(crate::container::sys_sethostname(arg0, arg1 as usize) as u64)
        }
        SYS_SET_TID_ADDRESS => SyscallAction::Continue(sys_set_tid_address(arg0) as u64),
        SYS_CLOCK_GETTIME => SyscallAction::Continue(sys_clock_gettime(arg0, arg1) as u64),
        SYS_CLOCK_NANOSLEEP => {
            SyscallAction::Continue(sys_clock_nanosleep(arg0, arg1, arg2, arg3) as u64)
        }
        SYS_SET_ROBUST_LIST => {
            SyscallAction::Continue(sys_set_robust_list(arg0, arg1 as usize) as u64)
        }
        SYS_PRLIMIT64 => {
            SyscallAction::Continue(sys_prlimit64(arg0 as i64, arg1, arg2, arg3) as u64)
        }
        SYS_GETRANDOM => SyscallAction::Continue(sys_getrandom(arg0, arg1 as usize, arg2) as u64),
        SYS_RSEQ => SyscallAction::Continue((-38i64) as u64),
        SYS_FIPS_SANDBOX => SyscallAction::Continue(sys_fips_sandbox(arg0, arg1) as u64),
        _ => return None,
    };
    Some(action)
}
