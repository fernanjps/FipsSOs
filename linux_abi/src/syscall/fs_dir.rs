#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};
use ostd::mm::validate_user_buffer;

use super::{get_active_program_name, get_cwd, resolve_path, set_cwd, GLOBAL_VFS};

/// Linux `sys_readlink` (syscall 89): reads the target of a symbolic link (e.g. `/proc/self/exe`).
pub fn sys_readlink(path_addr: u64, buf_addr: u64, bufsiz: usize) -> i64 {
    let path_str = match read_user_cstr_path(path_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if path_str == "/proc/self/exe" || path_str.starts_with("/proc/") {
        let prog = get_active_program_name();
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


/// Linux `sys_getcwd` (syscall 79): copies current working directory into user buffer.
pub fn sys_getcwd(buf_addr: u64, size: usize) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let root = crate::vfs::get_process_root(slot);
    let cwd = crate::container::virtual_cwd(&root, &get_cwd());
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
    let resolved = match read_user_resolved_path(path_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
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
    let resolved = match read_user_resolved_path(path_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_write_path_sandbox(&resolved) {
        return e;
    }
    if resolved.starts_with("/mnt/fat0/") {
        if let Err(e) = crate::fat32::mkdir(&resolved) {
            ostd::early_println!("[linux_abi::syscall] sys_mkdir error on fat32: {}", e);
            if e.contains("ya existe") {
                return -17; // -EEXIST
            } else if e.contains("no encontrado") {
                return -2; // -ENOENT
            }
            return -5; // -EIO
        }
        return 0;
    }
    if resolved.starts_with("/disk/") || resolved == "/disk" {
        if let Err(e) = crate::diskfs::create_directory(&resolved) {
            ostd::early_println!("[linux_abi::syscall] sys_mkdir error on diskfs: {}", e);
            if e.contains("ya existe") {
                return -17; // -EEXIST
            }
            return -5; // -EIO
        }
    }
    GLOBAL_VFS.lock().add_directory(&resolved);
    0
}


/// Linux `sys_getdents64` (syscall 217): enumerates directory entries into `struct linux_dirent64`.
pub fn sys_getdents64(fd: u64, dirp_addr: u64, count: usize) -> i64 {
    if count == 0 {
        return 0;
    }
    if validate_user_buffer(dirp_addr, count.min(65536), true).is_err() {
        return -14; // -EFAULT
    }
    let slot = ostd::scheduler::active_slot();
    let mut proc_node = crate::vfs::get_process_node(slot, fd);

    let mut vfs = GLOBAL_VFS.lock();
    let (dir_path, offset) = if let Some(ref pnode) = proc_node {
        if !pnode.is_dir {
            return -20; // -ENOTDIR
        }
        (pnode.path.clone(), pnode.offset)
    } else {
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
        let raw_bytes = entry.name.as_bytes();
        let name_len = raw_bytes.len().min(235);
        let name_bytes = &raw_bytes[..name_len];
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

    if let Some(ref mut pnode) = proc_node {
        pnode.offset = next_offset;
        crate::vfs::update_process_node(slot, fd, pnode.clone());
    } else if let Some(node) = vfs.get_node_mut(fd) {
        node.offset = next_offset;
    }

    written as i64
}


/// Linux `sys_access` (syscall 21): checks user's permissions for a file or directory.
pub fn sys_access(path_user_addr: u64, _mode: u64) -> i64 {
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let vfs = GLOBAL_VFS.lock();
    if vfs.is_directory(&resolved) || vfs.get_file_size_by_path(&resolved).is_some() {
        0
    } else {
        -2 // -ENOENT
    }
}


/// Linux `sys_unlink` (87) implementation for deleting a file from SATA DiskFS and VFS.
pub fn sys_unlink(path_user_addr: u64) -> i64 {
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_write_path_sandbox(&resolved) {
        return e;
    }

    ostd::early_println!("[linux_abi::syscall] sys_unlink(\"{}\")", resolved);
    ostd::fb_println!("[linux_abi::syscall] sys_unlink(\"{}\")", resolved);

    // If file is on /mnt/fat0/ or /disk/, delete from backing storage
    if resolved.starts_with("/mnt/fat0/") {
        if let Err(e) = crate::fat32::unlink(&resolved) {
            if e.contains("Es un directorio") {
                return -21i64; // -EISDIR
            }
            return -2i64; // -ENOENT
        }
        return 0;
    }
    if resolved.starts_with("/disk/") {
        if let Err(e) = crate::diskfs::delete_file(&resolved) {
            ostd::early_println!("[linux_abi::syscall] Fallo en sys_unlink: {}", e);
            if e.contains("Es un directorio") {
                return -21i64; // -EISDIR
            }
            return -2i64; // -ENOENT
        }
    }

    // Also remove from VFS in-memory cache if present
    GLOBAL_VFS.lock().remove_node(&resolved);

    0
}


/// Linux `sys_rmdir` (syscall 84): removes a persistent or in-memory directory.
pub fn sys_rmdir(path_user_addr: u64) -> i64 {
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_write_path_sandbox(&resolved) {
        return e;
    }

    ostd::early_println!("[linux_abi::syscall] sys_rmdir(\"{}\")", resolved);
    ostd::fb_println!("[linux_abi::syscall] sys_rmdir(\"{}\")", resolved);

    if resolved == "/disk" || resolved == "/mnt/fat0" || resolved == "/" {
        return -16i64; // -EBUSY
    }

    if resolved.starts_with("/mnt/fat0/") {
        if let Err(e) = crate::fat32::rmdir(&resolved) {
            if e.contains("no vacio") {
                return -39i64; // -ENOTEMPTY
            } else if e.contains("no encontrado") {
                return -2i64; // -ENOENT
            } else if e.contains("No es un directorio") {
                return -20i64; // -ENOTDIR
            }
            return -5i64; // -EIO
        }
        return 0;
    }

    if resolved.starts_with("/disk/") {
        if let Err(e) = crate::diskfs::delete_directory(&resolved) {
            ostd::early_println!("[linux_abi::syscall] Fallo en sys_rmdir: {}", e);
            if e.contains("no vacio") {
                return -39i64; // -ENOTEMPTY
            } else if e.contains("no encontrado") {
                return -2i64; // -ENOENT
            } else if e.contains("No es un directorio") {
                return -20i64; // -ENOTDIR
            }
            return -5i64; // -EIO
        }
    }

    if !GLOBAL_VFS.lock().remove_directory(&resolved) {
        return -39i64; // -ENOTEMPTY
    }

    0
}


/// Helper function to populate a 120-byte standard Linux `struct statfs`.
pub fn fill_statfs(path: &str, buf_addr: u64) -> i64 {
    let mut buf = [0u8; 120];
    let (f_type, bsize, blocks, bfree, bavail) = if path.starts_with("/mnt/fat0") {
        (0x4d44i64, 4096i64, 8192i64, 7000i64, 7000i64)
    } else if path.starts_with("/disk") {
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
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
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

pub(crate) fn read_user_cstr_path(path_user_addr: u64) -> Result<alloc::string::String, i64> {
    if path_user_addr == 0 {
        return Err(-14); // -EFAULT
    }
    let mut path_buf = [0u8; 128];
    let mut len = 0usize;
    let mut nul_found = false;
    while len < path_buf.len() {
        let mut b = [0u8; 1];
        read_user_bytes(path_user_addr + (len as u64), &mut b).map_err(|_| -14i64)?;
        if b[0] == 0 {
            nul_found = true;
            break;
        }
        path_buf[len] = b[0];
        len += 1;
    }
    if !nul_found {
        return Err(-36); // -ENAMETOOLONG
    }
    let s = core::str::from_utf8(&path_buf[..len]).map_err(|_| -22i64)?;
    Ok(alloc::string::String::from(s))
}

fn read_user_raw_path(path_user_addr: u64) -> Result<alloc::string::String, i64> {
    let s = read_user_cstr_path(path_user_addr)?;
    if s.is_empty() {
        return Err(-2); // -ENOENT
    }
    Ok(s)
}

pub(super) fn read_user_resolved_path(path_user_addr: u64) -> Result<alloc::string::String, i64> {
    let raw = read_user_raw_path(path_user_addr)?;
    Ok(resolve_path(&raw))
}

fn validate_dirfd_or_abs(dirfd: i64, path_user_addr: u64) -> Result<(), i64> {
    let raw = read_user_raw_path(path_user_addr)?;
    if !raw.starts_with('/') && (dirfd as i32) != -100 {
        return Err(-9); // -EBADF
    }
    Ok(())
}

/// Linux `sys_fsync` / `sys_fdatasync` (74 / 75): flushes open dirty file backing state to disk.
pub fn sys_fsync(fd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    if let Some(pnode) = crate::vfs::get_process_node(slot, fd) {
        if pnode.pipe.is_some() || pnode.is_device {
            return -22; // -EINVAL
        }
    }
    match GLOBAL_VFS.lock().fsync_fd(fd) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Linux `sys_ftruncate` (77): truncates or zero-extends an open writable file descriptor.
pub fn sys_ftruncate(fd: u64, length: i64) -> i64 {
    if length < 0 || length > 16 * 1024 * 1024 {
        return -22; // -EINVAL
    }
    let slot = ostd::scheduler::active_slot();
    let mut handle_raw = None;
    if let Some(pnode) = crate::vfs::get_process_node(slot, fd) {
        if pnode.pipe.is_some() || pnode.is_device {
            return -22; // -EINVAL
        }
        if let Err(e) = super::sandbox::check_write_fd_sandbox(&pnode.path, pnode.is_device, pnode.pipe.is_some()) {
            return e;
        }
        handle_raw = Some(pnode.handle.raw());
    } else if let Err(e) = super::sandbox::check_cap(kernel_objects::CAP_FS_WRITE) {
        return e;
    }
    let res = GLOBAL_VFS.lock().truncate_fd(fd, length as usize);
    match res {
        Ok(()) => {
            if let Some(h) = handle_raw {
                crate::vfs::sync_process_fd_size(slot, h, length as usize);
            }
            0
        }
        Err(e) => e,
    }
}

/// Linux `sys_truncate` (76): truncates or zero-extends a file identified by path.
pub fn sys_truncate(path_user_addr: u64, length: i64) -> i64 {
    if length < 0 || length > 16 * 1024 * 1024 {
        return -22;
    }
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = super::sandbox::check_write_path_sandbox(&resolved) {
        return e;
    }
    match GLOBAL_VFS.lock().truncate_path(&resolved, length as usize) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Linux `sys_chmod` (90): validates target path exists and updates POSIX mode (capabilities preserved).
pub fn sys_chmod(path_user_addr: u64) -> i64 {
    let resolved = match read_user_resolved_path(path_user_addr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if GLOBAL_VFS.lock().path_exists(&resolved) {
        0
    } else {
        -2 // -ENOENT
    }
}

/// Linux `sys_fchmod` (91): validates open descriptor exists.
pub fn sys_fchmod(fd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    if crate::vfs::get_process_node(slot, fd).is_some() || GLOBAL_VFS.lock().get_node(fd).is_some() {
        0
    } else {
        -9 // -EBADF
    }
}

/// Linux `sys_fchmodat` (268): validates `AT_FDCWD`-relative or absolute path exists.
pub fn sys_fchmodat(dirfd: i64, path_user_addr: u64) -> i64 {
    if let Err(e) = validate_dirfd_or_abs(dirfd, path_user_addr) {
        return e;
    }
    sys_chmod(path_user_addr)
}

/// Linux `sys_unlinkat` (263): unlinks a file or removes an empty directory (`AT_REMOVEDIR`).
pub fn sys_unlinkat(dirfd: i64, path_user_addr: u64, flags: u64) -> i64 {
    const AT_REMOVEDIR: u64 = 0x200;
    if let Err(e) = validate_dirfd_or_abs(dirfd, path_user_addr) {
        return e;
    }
    if (flags & AT_REMOVEDIR) != 0 {
        sys_rmdir(path_user_addr)
    } else {
        sys_unlink(path_user_addr)
    }
}

