#![forbid(unsafe_code)]
#![no_std]

//! Decoupled Linux/POSIX ABI personality layer for FipSOs (including DRM, `/dev/accel`, and ELF64 loader).
//! Strictly forbids `unsafe_code`.

extern crate alloc;

pub mod diskfs;
pub mod elf;
pub mod syscall;
pub mod tarfs;
pub mod vfs;

pub use diskfs::{
    create_directory, delete_directory, delete_file, list_files, save_file, DiskFsDirEntry,
    DiskFsFile, DiskFsInode, FileType,
};
pub use elf::{Elf64Loader, ElfError, LoadedElf64};
pub use syscall::{
    dispatch_syscall, init, read_all_by_path, read_device_vmo, sys_arch_prctl, sys_brk, sys_close, sys_execve,
    sys_fstat, sys_getpid, sys_getrandom, sys_ioctl, sys_kill, sys_lseek, sys_mmap, sys_newfstatat,
    sys_open, sys_read, sys_readlink, sys_sched_yield, sys_set_tid_address, sys_uname, sys_unlink,
    sys_write, sys_writev, ARCH_GET_FS, ARCH_SET_FS, SYS_ARCH_PRCTL, SYS_BRK, SYS_CLOCK_GETTIME,
    SYS_CLOSE, SYS_EXECVE, SYS_EXIT, SYS_EXIT_GROUP, SYS_FSTAT, SYS_GETPID, SYS_GETRANDOM,
    SYS_IOCTL, SYS_KILL, SYS_LSEEK, SYS_MMAP, SYS_MPROTECT, SYS_NEWFSTATAT, SYS_OPEN, SYS_OPENAT,
    SYS_PRLIMIT64, SYS_READ, SYS_READLINK, SYS_READLINKAT, SYS_REBOOT, SYS_RSEQ, SYS_SCHED_YIELD, SYS_SET_ROBUST_LIST,
    SYS_SET_TID_ADDRESS, SYS_UNAME, SYS_UNLINK, SYS_WRITE, SYS_WRITEV, TCGETS, TIOCGWINSZ,
};
pub use tarfs::{TarFileEntry, TarFs, TAR_BLOCK_SIZE};
pub use vfs::{
    CapabilityVfs, KernelPipe, ProcessFdTable, VfsDeviceNode, ACCEL_IOCTL_BIND_TENSOR_VMO, ACCEL_IOCTL_RUN_INFERENCE,
    DISK_IOCTL_DELETE_FILE, DISK_IOCTL_SAVE_NOTE, DRM_IOCTL_PRIME_ZERO_COPY,
    FB_IOCTL_FLIP_BUFFER, FB_IOCTL_GET_INFO, FB_IOCTL_POLL_KEY, FB_IOCTL_PRESENT_RECT,
    FB_IOCTL_SET_DESKTOP_MODE, FB_WIDGET_DEFAULT_H, FB_WIDGET_DEFAULT_W, FB_WIDGET_DEFAULT_X,
    FB_WIDGET_DEFAULT_Y, FD_ACCEL_NPU0, FD_DRM_CARD0, FD_FB0, FD_MOUSE0, FD_NET0, FD_STDERR,
    FD_STDIN, FD_STDOUT, MAP_ANONYMOUS, MOUSE_IOCTL_POLL_STATE, MOUSE_IOCTL_STEP_SYNTHETIC,
    NET_IOCTL_BROWSER_FETCH_SAVE, NET_IOCTL_HTTP_FETCH, NET_IOCTL_PING_GATEWAY, PATH_ACCEL_NPU0,
    PATH_AI_MODEL_V2, PATH_BIN_AVATAR_CPP, PATH_BIN_C_STD_APP, PATH_BIN_DESKTOP_UI,
    PATH_BIN_NPU_INFER, PATH_BOOT_LOG, PATH_DOWNLOAD_PKG, PATH_DRM_CARD0, PATH_ETC_MOTD,
    PATH_FAULT_TEST, PATH_FB0, PATH_INDEX_HTML, PATH_MODEL_AVATAR, PATH_MOUSE0, PATH_NET0,
    PATH_PERSISTENT_NOTE, PATH_SYS_HWMON, PATH_SYS_UPDATE_PKG, PROT_WRITE,
    VFS_IOCTL_HWMON_REPORT, VFS_IOCTL_LIST_FILES, VFS_IOCTL_LIST_PROCESSES,
    VFS_IOCTL_SYS_TELEMETRY, VFS_IOCTL_TASK_CONTROL,
};
