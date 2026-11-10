#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};

use crate::vfs::{FD_STDERR, FD_STDIN, FD_STDOUT};
use super::{get_cwd, resolve_path, set_cwd, ACTIVE_PROGRAM_NAME, GLOBAL_VFS};

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

    ostd::early_println!("[linux_abi::syscall] sys_rmdir(\"{}\")", resolved);
    ostd::fb_println!("[linux_abi::syscall] sys_rmdir(\"{}\")", resolved);

    if resolved == "/disk" || resolved == "/" {
        return -16i64; // -EBUSY
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

