#![forbid(unsafe_code)]
#![no_std]

//! `libfipsos` — Safe Ring 3 User-Space SDK for FipSOs ELF64 applications.
//!
//! Exposes high-level, zero-`unsafe` wrappers over Linux `x86_64` ABI syscalls
//! (`sys_write`, `sys_open`, `sys_read`, `sys_ioctl`, `sys_mmap`, `sys_execve`, `sys_exit`)
//! and Zero-Copy `/dev/fb0` and `/dev/accel/accel0` helpers.

pub use ostd::user::{
    render_zero_copy_rgb32_widget, sys_execve, sys_exit, sys_getpid, sys_ioctl, sys_mmap,
    sys_open, sys_read, sys_sched_yield, sys_write, SYS_EXECVE, SYS_EXIT, SYS_GETPID, SYS_IOCTL,
    SYS_MMAP, SYS_OPEN, SYS_READ, SYS_SCHED_YIELD, SYS_WRITE, USER_EXEC_BASE_VADDR,
};

/// Standard POSIX file descriptors available in Ring 3.
pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;
pub const FD_STDERR: u64 = 2;

/// Open flags (`O_RDONLY`, `O_RDWR`) and `mmap` protection/sharing constants.
pub const O_RDONLY: u64 = 0;
pub const O_RDWR: u64 = 2;
pub const PROT_READ: u64 = 0x1;
pub const PROT_WRITE: u64 = 0x2;
pub const MAP_SHARED: u64 = 0x1;

/// Canonical FipSOs capability VFS paths.
pub const CSTR_PATH_FB0: &[u8] = b"/dev/fb0\0";
pub const CSTR_PATH_ACCEL0: &[u8] = b"/dev/accel/accel0\0";
pub const CSTR_PATH_MODEL_AVATAR: &[u8] = b"/models/avatar_v1.bin\0";

/// FipSOs `ioctl` command numbers for `/dev/accel/accel0`, `/dev/fb0`, and `SchedulerRegistry`.
pub const ACCEL_IOCTL_BIND_TENSOR_VMO: u64 = 0xC010_4100;
pub const ACCEL_IOCTL_RUN_INFERENCE: u64 = 0xC010_4102;
pub const FB_IOCTL_PRESENT_RECT: u64 = 0xC010_4601;
pub const VFS_IOCTL_LIST_PROCESSES: u64 = 0xC010_5001;

/// Default `/dev/fb0` Zero-Copy widget dimensions (`160x80` RGB32).
pub const FB_WIDGET_W: usize = 160;
pub const FB_WIDGET_H: usize = 80;

/// Writes a UTF-8 message string to `FD_STDOUT` (`fd=1`) from Ring 3.
pub fn print(msg: &str) -> isize {
    sys_write(FD_STDOUT, msg.as_bytes())
}
