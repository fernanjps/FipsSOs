#![forbid(unsafe_code)]
#![no_std]

//! Decoupled Linux/POSIX ABI personality layer for FipSOs (including DRM, `/dev/accel`, and ELF64 loader).
//! Strictly forbids `unsafe_code`.

extern crate alloc;

pub mod elf;
pub mod syscall;
pub mod tarfs;
pub mod vfs;

pub use elf::{Elf64Loader, ElfError, LoadedElf64};
pub use syscall::{
    dispatch_syscall, init, read_device_vmo, sys_brk, sys_close, sys_execve, sys_getpid, sys_ioctl,
    sys_mmap, sys_open, sys_read, sys_sched_yield, sys_write, SYS_BRK, SYS_CLOSE, SYS_EXECVE,
    SYS_EXIT, SYS_GETPID, SYS_IOCTL, SYS_MMAP, SYS_OPEN, SYS_READ, SYS_SCHED_YIELD, SYS_WRITE,
};
pub use tarfs::{TarFileEntry, TarFs, TAR_BLOCK_SIZE};
pub use vfs::{
    CapabilityVfs, VfsDeviceNode, ACCEL_IOCTL_BIND_TENSOR_VMO, ACCEL_IOCTL_RUN_INFERENCE,
    DRM_IOCTL_PRIME_ZERO_COPY, FB_IOCTL_POLL_KEY, FB_IOCTL_PRESENT_RECT, FB_WIDGET_DEFAULT_H,
    FB_WIDGET_DEFAULT_W, FB_WIDGET_DEFAULT_X, FB_WIDGET_DEFAULT_Y, FD_ACCEL_NPU0, FD_DRM_CARD0,
    FD_FB0, FD_STDERR, FD_STDIN, FD_STDOUT, PATH_ACCEL_NPU0, PATH_BIN_AVATAR_CPP,
    PATH_BIN_NPU_INFER, PATH_DRM_CARD0, PATH_ETC_MOTD, PATH_FB0, PATH_MODEL_AVATAR, PROT_WRITE,
    VFS_IOCTL_LIST_FILES, VFS_IOCTL_LIST_PROCESSES,
};
