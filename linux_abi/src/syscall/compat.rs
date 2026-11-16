#![forbid(unsafe_code)]

//! Compatibilidad ABI de Linux para binarios ELF64 estáticos (`_start` / `musl` / `glibc` / `rustc`).
//!
//! Aísla las llamadas al sistema requeridas por runtimes estáticos Linux:
//! `readv`, `writev`, `openat`, `fstat`, `newfstatat`, `arch_prctl`,
//! `set_tid_address`, `set_robust_list`, `prlimit64`, `getrandom`, `fcntl`,
//! `nanosleep` y `clock_nanosleep`.

use alloc::string::String;
use ostd::arch::syscall::{read_user_bytes, write_user_bytes};

use crate::vfs::fd_table::{get_process_node, insert_process_fd};
use super::{get_cwd, resolve_path, sys_read, sys_write, GLOBAL_VFS};

pub const AT_FDCWD: i64 = -100;
pub const AT_EMPTY_PATH: u64 = 0x1000;

pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;

const TIMER_ABSTIME: u64 = 1;
const MAX_IOVCNT: u64 = 1024;
const O_CREAT: u64 = 0x40;

/// Reads a NUL-terminated path string (up to 127 bytes) from user memory.
fn read_user_path(addr: u64) -> Result<String, i64> {
    super::fs_dir::read_user_cstr_path(addr)
}

/// Resolves a path relative to `dirfd` (`AT_FDCWD` or an open directory fd).
fn resolve_at_dirfd(dirfd: i64, raw_path: &str) -> Result<String, i64> {
    if raw_path.starts_with('/') || (dirfd as i32) == -100 {
        return Ok(resolve_path(raw_path));
    }

    let slot = ostd::scheduler::active_slot();
    let base_node = get_process_node(slot, dirfd as u64).ok_or(-9i64)?; // -EBADF
    Ok(crate::container::resolve_dirfd_confined(&base_node.path, raw_path))
}

/// Populates and copies a Linux x86_64 `struct stat` (144 bytes) to user space.
fn write_linux_stat(
    statbuf_addr: u64,
    st_mode: u32,
    st_size: i64,
    st_ino: u64,
    st_rdev: u64,
) -> i64 {
    if statbuf_addr == 0 {
        return -14; // -EFAULT
    }
    let mut stat = [0u8; 144];
    stat[0..8].copy_from_slice(&1u64.to_le_bytes()); // st_dev
    stat[8..16].copy_from_slice(&st_ino.max(1).to_le_bytes()); // st_ino
    stat[16..24].copy_from_slice(&1u64.to_le_bytes()); // st_nlink
    stat[24..28].copy_from_slice(&st_mode.to_le_bytes()); // st_mode
    stat[28..32].copy_from_slice(&0u32.to_le_bytes()); // st_uid
    stat[32..36].copy_from_slice(&0u32.to_le_bytes()); // st_gid
    stat[40..48].copy_from_slice(&st_rdev.to_le_bytes()); // st_rdev
    stat[48..56].copy_from_slice(&st_size.to_le_bytes()); // st_size
    stat[56..64].copy_from_slice(&4096i64.to_le_bytes()); // st_blksize
    let blocks = (st_size + 511) / 512;
    stat[64..72].copy_from_slice(&blocks.to_le_bytes()); // st_blocks

    if write_user_bytes(statbuf_addr, &stat).is_err() {
        return -14; // -EFAULT
    }
    0
}

/// Linux `sys_readv` (syscall 19): scatter-gather read across `iovcnt` `struct iovec` entries.
pub fn sys_readv(fd: u64, iov_ptr: u64, iovcnt: u64) -> i64 {
    if iovcnt == 0 {
        return 0;
    }
    if iovcnt > MAX_IOVCNT {
        return -22; // -EINVAL
    }
    if iov_ptr == 0 {
        return -14; // -EFAULT
    }

    let mut total: isize = 0;
    for i in 0..iovcnt {
        let mut iov = [0u8; 16];
        if read_user_bytes(iov_ptr + i * 16, &mut iov).is_err() {
            return if total > 0 { total as i64 } else { -14 };
        }
        let base = u64::from_le_bytes([
            iov[0], iov[1], iov[2], iov[3], iov[4], iov[5], iov[6], iov[7],
        ]);
        let len = u64::from_le_bytes([
            iov[8], iov[9], iov[10], iov[11], iov[12], iov[13], iov[14], iov[15],
        ]);
        if len == 0 {
            continue;
        }
        let res = sys_read(fd, base, len as usize);
        if res < 0 {
            return if total > 0 { total as i64 } else { res as i64 };
        }
        total += res;
        if (res as u64) < len {
            break;
        }
    }
    total as i64
}

/// Linux `sys_writev` (syscall 20): vectored write across `iovcnt` `struct iovec` entries.
pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> i64 {
    if iovcnt == 0 {
        return 0;
    }
    if iovcnt > MAX_IOVCNT {
        return -22; // -EINVAL
    }
    if iov_ptr == 0 {
        return -14; // -EFAULT
    }

    let mut total: isize = 0;
    for i in 0..iovcnt {
        let mut iov = [0u8; 16];
        if read_user_bytes(iov_ptr + i * 16, &mut iov).is_err() {
            return if total > 0 { total as i64 } else { -14 };
        }
        let base = u64::from_le_bytes([
            iov[0], iov[1], iov[2], iov[3], iov[4], iov[5], iov[6], iov[7],
        ]);
        let len = u64::from_le_bytes([
            iov[8], iov[9], iov[10], iov[11], iov[12], iov[13], iov[14], iov[15],
        ]);
        if len == 0 {
            continue;
        }
        let res = sys_write(fd, base, len as usize);
        if res < 0 {
            return if total > 0 { total as i64 } else { res as i64 };
        }
        total += res;
    }
    total as i64
}

/// Linux `sys_openat` (syscall 257): opens a file relative to `dirfd` (`AT_FDCWD = -100`).
pub fn sys_openat(dirfd: i64, pathname_addr: u64, flags: u64, _mode: u64) -> i64 {
    let raw_path = match read_user_path(pathname_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if raw_path.is_empty() {
        return -2; // -ENOENT
    }

    if (dirfd as i32) == -100 || raw_path.starts_with('/') {
        let resolved = resolve_path(&raw_path);
        return super::fs::open_resolved_path(&resolved, flags);
    }

    let resolved = match resolve_at_dirfd(dirfd, &raw_path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_open_sandbox(&resolved, flags) {
        return e;
    }

    let slot = ostd::scheduler::active_slot();
    let mut vfs = GLOBAL_VFS.lock();
    let mut opened = vfs.open(&resolved, flags);
    if opened.is_err() && (flags & O_CREAT) != 0 && resolved.starts_with("/disk/") {
        if vfs.create_or_update_disk_file(&resolved, &[]).is_ok() {
            opened = vfs.open(&resolved, flags);
        }
    }
    match opened {
        Ok(fd) => {
            let maybe_node = vfs.get_node(fd).cloned();
            drop(vfs);
            if let Some(node) = maybe_node {
                insert_process_fd(slot, fd, node);
            }
            fd as i64
        }
        Err(errno) => {
            drop(vfs);
            errno
        }
    }
}

/// Linux `sys_fstat` (syscall 5): retrieves `struct stat` metadata for open descriptor `fd`.
pub fn sys_fstat(fd: u64, statbuf_addr: u64) -> i64 {
    if statbuf_addr == 0 {
        return -14; // -EFAULT
    }

    if fd <= 2 {
        return write_linux_stat(statbuf_addr, 0o020620, 0, fd + 1, 0x8801);
    }

    let slot = ostd::scheduler::active_slot();
    if let Some(node) = get_process_node(slot, fd) {
        let mode = if node.is_dir {
            0o040755u32
        } else if node.is_device {
            0o020666u32
        } else {
            0o100755u32
        };
        let size = if node.is_dir { 4096 } else { node.file_size as i64 };
        return write_linux_stat(statbuf_addr, mode, size, fd + 10, 0);
    }

    let vfs = GLOBAL_VFS.lock();
    if let Some(node) = vfs.get_node(fd) {
        let mode = if node.is_dir {
            0o040755u32
        } else if node.is_device {
            0o020666u32
        } else {
            0o100755u32
        };
        let size = if node.is_dir { 4096 } else { node.file_size as i64 };
        return write_linux_stat(statbuf_addr, mode, size, fd + 10, 0);
    }

    -9 // -EBADF
}

/// Linux `sys_newfstatat` (syscall 262): queries file metadata relative to `dirfd` (`AT_FDCWD`).
pub fn sys_newfstatat(dirfd: i64, path_addr: u64, statbuf_addr: u64, flags: u64) -> i64 {
    if statbuf_addr == 0 {
        return -14; // -EFAULT
    }

    let raw_path = match read_user_path(path_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if raw_path.is_empty() && ((flags & AT_EMPTY_PATH) != 0 || dirfd >= 0) {
        return sys_fstat(dirfd as u64, statbuf_addr);
    }

    let resolved = match resolve_at_dirfd(dirfd, &raw_path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_open_sandbox(&resolved, flags) {
        return e;
    }

    let vfs = GLOBAL_VFS.lock();
    if vfs.is_directory(&resolved)
        || matches!(
            resolved.as_str(),
            "/" | "/dev" | "/proc" | "/bin" | "/disk" | "/etc" | "/models"
        )
    {
        return write_linux_stat(statbuf_addr, 0o040755, 4096, 2, 0);
    }

    if let Some(size) = vfs.get_file_size_by_path(&resolved) {
        let mode = if resolved.starts_with("/dev/") {
            0o020666u32
        } else {
            0o100755u32
        };
        return write_linux_stat(statbuf_addr, mode, size as i64, 100, 0);
    }
    drop(vfs);

    if resolved == "/mnt/fat0" || resolved.starts_with("/mnt/fat0/") {
        if let Some((is_dir, size)) = crate::fat32::stat_path(&resolved) {
            let mode = if is_dir { 0o040755u32 } else { 0o100644u32 };
            return write_linux_stat(statbuf_addr, mode, size as i64, 300, 0);
        }
        return -2; // -ENOENT
    }

    if resolved.starts_with("/disk/") {
        for inode in crate::diskfs::list_files() {
            if inode.used && inode.path == resolved {
                let mode = if inode.file_type == crate::diskfs::FileType::Directory {
                    0o040755u32
                } else {
                    0o100644u32
                };
                return write_linux_stat(statbuf_addr, mode, inode.size_bytes as i64, 200, 0);
            }
        }
    }

    // Also allow stat() on current working directory '.' if passed explicitly
    if raw_path == "." && get_cwd() == "/" {
        return write_linux_stat(statbuf_addr, 0o040755, 4096, 2, 0);
    }

    -2 // -ENOENT
}

/// Linux `sys_arch_prctl` (syscall 158): sets or retrieves Thread-Local Storage `IA32_FS_BASE`.
pub fn sys_arch_prctl(code: u64, addr: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            ostd::arch::syscall::set_user_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            let base = ostd::arch::syscall::get_user_fs_base();
            if write_user_bytes(addr, &base.to_le_bytes()).is_err() {
                return -14; // -EFAULT
            }
            0
        }
        _ => -22, // -EINVAL
    }
}

/// Linux `sys_set_tid_address` (syscall 218): registers clear_child_tid pointer and returns caller TID.
pub fn sys_set_tid_address(_tidptr: u64) -> i64 {
    let (_pid, tid) = ostd::scheduler::active_pid_and_tid();
    tid as i64
}

/// Linux `sys_set_robust_list` (syscall 273): registers robust futex list head for glibc/musl `_start`.
pub fn sys_set_robust_list(_head: u64, len: usize) -> i64 {
    if len != 24 && len != 0 {
        return -22; // -EINVAL
    }
    0
}

/// Linux `sys_prlimit64` (syscall 302): queries or updates process resource limits (`struct rlimit64`).
pub fn sys_prlimit64(pid: i64, resource: u64, new_rlim_addr: u64, old_rlim_addr: u64) -> i64 {
    if pid < 0 {
        return -22; // -EINVAL
    }
    if new_rlim_addr != 0 {
        let mut check = [0u8; 16];
        if read_user_bytes(new_rlim_addr, &mut check).is_err() {
            return -14; // -EFAULT
        }
    }
    if old_rlim_addr != 0 {
        let (cur, max) = match resource {
            3 => (8u64 * 1024 * 1024, u64::MAX), // RLIMIT_STACK: 8 MiB
            7 => (1024u64, 4096u64),             // RLIMIT_NOFILE
            _ => (u64::MAX, u64::MAX),           // RLIMIT_AS / DATA / CORE / etc.
        };
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&cur.to_le_bytes());
        buf[8..16].copy_from_slice(&max.to_le_bytes());
        if write_user_bytes(old_rlim_addr, &buf).is_err() {
            return -14; // -EFAULT
        }
    }
    0
}

/// Linux `sys_getrandom` (syscall 318): fills `buflen` bytes in user space with entropy-seeded PRNG.
pub fn sys_getrandom(buf_addr: u64, buflen: usize, _flags: u64) -> i64 {
    if buflen == 0 {
        return 0;
    }
    if buf_addr == 0 {
        return -14; // -EFAULT
    }

    let mut seed = ostd::arch::idt::timer_ticks()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);

    let mut written = 0usize;
    let mut chunk = [0u8; 256];
    while written < buflen {
        let n = (buflen - written).min(chunk.len());
        for byte in chunk[..n].iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = (seed >> 32) as u8;
        }
        if write_user_bytes(buf_addr + written as u64, &chunk[..n]).is_err() {
            return if written > 0 { written as i64 } else { -14 };
        }
        written += n;
    }
    written as i64
}

/// Sleeps for `ticks` 100Hz timer ticks cooperatively while allowing IRQ0 timer delivery.
fn sleep_ticks(ticks_to_sleep: u64) {
    ostd::arch::idt::sleep_ticks(ticks_to_sleep);
}

/// Linux `sys_nanosleep` (syscall 35): suspends the calling thread for `req` seconds/nanoseconds.
pub fn sys_nanosleep(req_addr: u64, rem_addr: u64) -> i64 {
    let mut ts = [0u8; 16];
    if read_user_bytes(req_addr, &mut ts).is_err() {
        return -14; // -EFAULT
    }
    let sec = i64::from_le_bytes([ts[0], ts[1], ts[2], ts[3], ts[4], ts[5], ts[6], ts[7]]);
    let nsec = i64::from_le_bytes([
        ts[8], ts[9], ts[10], ts[11], ts[12], ts[13], ts[14], ts[15],
    ]);
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return -22; // -EINVAL
    }

    let ticks = if sec == 0 && nsec == 0 {
        0
    } else {
        ((sec as u64).saturating_mul(100))
            .saturating_add((nsec as u64).div_ceil(10_000_000))
            .max(1)
    };
    sleep_ticks(ticks);

    if rem_addr != 0 {
        let zero = [0u8; 16];
        let _ = write_user_bytes(rem_addr, &zero);
    }
    0
}

/// Linux `sys_clock_nanosleep` (syscall 230): high-resolution sleep supporting relative and `TIMER_ABSTIME`.
pub fn sys_clock_nanosleep(clock_id: u64, flags: u64, req_addr: u64, rem_addr: u64) -> i64 {
    if (flags & TIMER_ABSTIME) == 0 {
        return sys_nanosleep(req_addr, rem_addr);
    }

    let mut ts = [0u8; 16];
    if read_user_bytes(req_addr, &mut ts).is_err() {
        return -14; // -EFAULT
    }
    let sec = i64::from_le_bytes([ts[0], ts[1], ts[2], ts[3], ts[4], ts[5], ts[6], ts[7]]);
    let nsec = i64::from_le_bytes([
        ts[8], ts[9], ts[10], ts[11], ts[12], ts[13], ts[14], ts[15],
    ]);
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return -22; // -EINVAL
    }

    let target_ticks = if clock_id == 0 {
        let dt = ostd::arch::x86_64::rtc_power::read_rtc();
        let now_sec = ostd::arch::x86_64::rtc_power::rtc_to_unix_timestamp(&dt);
        if sec <= now_sec {
            0
        } else {
            ((sec - now_sec) as u64)
                .saturating_mul(100)
                .saturating_add((nsec as u64).div_ceil(10_000_000))
        }
    } else {
        let abs_ticks = ((sec as u64).saturating_mul(100))
            .saturating_add((nsec as u64).div_ceil(10_000_000));
        abs_ticks.saturating_sub(ostd::arch::idt::timer_ticks())
    };

    sleep_ticks(target_ticks);
    0
}
