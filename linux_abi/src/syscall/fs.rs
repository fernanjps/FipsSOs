#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};
use ostd::mm::validate_user_buffer;
use ostd::sync::SpinLock;

use crate::vfs::{
    get_process_node, insert_process_fd, remove_process_fd, KernelPipe, ProcessFdTable,
    VfsDeviceNode, FD_FB0, FD_STDERR, FD_STDIN, FD_STDOUT,
};
use super::{resolve_path, GLOBAL_VFS, TCGETS, TIOCGWINSZ};

pub use super::fs_dir::*;

/// Linux `sys_read` (syscall 0):
/// - When `fd == FD_STDIN (0)`: reads an interactive line from `IRQ1` PS/2 keyboard / console with echo,
///   unless redirected to a file or pipe.
/// - When `fd` is a pipe, file or device: reads up to `len` bytes from the backing descriptor.
pub fn sys_read(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if len == 0 {
        return 0;
    }

    let slot = ostd::scheduler::active_slot();
    let proc_node = get_process_node(slot, fd);

    // 1. If reading from a KernelPipe:
    if let Some(ref node) = proc_node {
        if let Some(ref pipe) = node.pipe {
            if node.is_pipe_write {
                return -9; // -EBADF
            }
            let max_len = len.min(4096);
            if validate_user_buffer(user_buf_vaddr, max_len, true).is_err() {
                return -14; // -EFAULT
            }
            let mut kernel_buf = [0u8; 4096];
            loop {
                let mut p = pipe.lock();
                if p.count > 0 {
                    let n = p.read(&mut kernel_buf[..max_len]);
                    drop(p);
                    if write_user_bytes(user_buf_vaddr, &kernel_buf[..n]).is_err() {
                        return -14;
                    }
                    return n as isize;
                }
                if p.writers_count == 0 {
                    // EOF: All writers have closed and buffer is empty
                    return 0;
                }
                drop(p);
                ostd::scheduler::on_sys_sched_yield(0, 0);
            }
        }
    }

    let is_pipe_or_vmo = if let Some(ref node) = proc_node {
        !node.is_device || node.socket_id.is_some()
    } else {
        let vfs = GLOBAL_VFS.lock();
        if let Some(node) = vfs.get_node(fd) {
            node.pipe.is_some() || node.socket_id.is_some() || !node.is_device
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

    if validate_user_buffer(user_buf_vaddr, len, true).is_err() {
        return -14; // -EFAULT
    }

    // Interactive character-at-a-time read (used by interactive shell / ANSI escape navigation)
    if len == 1 {
        let ch = ostd::arch::keyboard::read_char_blocking();
        if write_user_bytes(user_buf_vaddr, &[ch]).is_err() {
            return -14;
        }
        return 1;
    }

    let max_len = len.min(128);
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
            0x1B => {
                if pos + 1 < max_len {
                    line_buf[pos] = 0x1B;
                    pos += 1;
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
    let slot = ostd::scheduler::active_slot();
    let mut vfs = GLOBAL_VFS.lock();
    let res = vfs.open(&resolved, flags);
    match res {
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


/// Linux `sys_close` (syscall 3): validates and closes an open capability file descriptor.
pub fn sys_close(fd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let _ = remove_process_fd(slot, fd);
    if fd <= FD_STDERR {
        return 0;
    }
    match GLOBAL_VFS.lock().close(fd) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}


/// Linux `sys_write` (syscall 1): writes bytes from a verified Ring 3 user buffer
/// to the serial port / Framebuffer console, or to regular files / pipes.
pub fn sys_write(fd: u64, user_buf_vaddr: u64, len: usize) -> isize {
    if len == 0 {
        return 0;
    }

    let slot = ostd::scheduler::active_slot();
    let proc_node = get_process_node(slot, fd);

    // 1. If writing to a KernelPipe:
    if let Some(ref node) = proc_node {
        if let Some(ref pipe) = node.pipe {
            if !node.is_pipe_write {
                return -9; // -EBADF
            }
            let mut written = 0usize;
            let mut kernel_buf = [0u8; 512];
            while written < len {
                let chunk = core::cmp::min(len - written, kernel_buf.len());
                if read_user_bytes(user_buf_vaddr + (written as u64), &mut kernel_buf[..chunk]).is_err() {
                    return if written > 0 { written as isize } else { -14 }; // -EFAULT
                }
                let res = pipe.lock().write(&kernel_buf[..chunk]);
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
    }

    let is_pipe_or_file = if let Some(ref node) = proc_node {
        !node.is_device || node.socket_id.is_some()
    } else {
        let vfs = GLOBAL_VFS.lock();
        if let Some(node) = vfs.get_node(fd) {
            node.pipe.is_some() || node.socket_id.is_some() || !node.is_device
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


/// Linux `sys_ioctl` (syscall 16): dispatches device control operations, including POSIX terminal
/// inquiries (`TCGETS` 0x5401 and `TIOCGWINSZ` 0x5413) and capability devices.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> i64 {
    if request == TCGETS || request == TIOCGWINSZ {
        // Return 0 only for actual interactive console/terminal streams, not pipes or regular files
        let slot = ostd::scheduler::active_slot();
        let is_tty = {
            if let Some(node) = get_process_node(slot, fd) {
                node.is_device && node.pipe.is_none() && (fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR)
            } else {
                let vfs = GLOBAL_VFS.lock();
                if let Some(node) = vfs.get_node(fd) {
                    node.is_device && node.pipe.is_none() && (fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR)
                } else {
                    false
                }
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


/// Linux `sys_pipe2` (syscall 293 / 22): creates an anonymous pipe pair `[read_fd, write_fd]`.
pub fn sys_pipe2(pipefd_addr: u64, _flags: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let pipe = alloc::sync::Arc::new(SpinLock::new(KernelPipe::new()));

    let mut map = crate::vfs::PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    let r_fd = table.next_fd;
    table.next_fd += 1;
    let w_fd = table.next_fd;
    table.next_fd += 1;

    let r_node = VfsDeviceNode {
        fd: r_fd,
        path: alloc::format!("pipe:[{}]", r_fd),
        handle: kernel_objects::HandleId::null(),
        file_size: 0,
        is_device: false,
        offset: 0,
        flags: 0,
        is_dir: false,
        pipe: Some(alloc::sync::Arc::clone(&pipe)),
        is_pipe_write: false,
        socket_id: None,
    };
    let w_node = VfsDeviceNode {
        fd: w_fd,
        path: alloc::format!("pipe:[{}]", w_fd),
        handle: kernel_objects::HandleId::null(),
        file_size: 0,
        is_device: false,
        offset: 0,
        flags: 0,
        is_dir: false,
        pipe: Some(pipe),
        is_pipe_write: true,
        socket_id: None,
    };
    table.descriptors.insert(r_fd, r_node);
    table.descriptors.insert(w_fd, w_node);
    drop(map);

    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&(r_fd as i32).to_le_bytes());
    buf[4..8].copy_from_slice(&(w_fd as i32).to_le_bytes());
    if write_user_bytes(pipefd_addr, &buf).is_err() {
        remove_process_fd(slot, r_fd);
        remove_process_fd(slot, w_fd);
        return -14;
    }
    0
}


/// Linux `sys_dup` (syscall 32): duplicates an open file descriptor.
pub fn sys_dup(oldfd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let mut map = crate::vfs::PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    let newfd = table.next_fd;
    table.next_fd += 1;
    drop(map);
    sys_dup2(oldfd, newfd)
}


/// Linux `sys_dup2` (syscall 33 / 292): duplicates an open file descriptor to `newfd`.
pub fn sys_dup2(oldfd: u64, newfd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    if oldfd == newfd {
        if get_process_node(slot, oldfd).is_some() || GLOBAL_VFS.lock().get_node(oldfd).is_some() {
            return newfd as i64;
        } else {
            return -9; // -EBADF
        }
    }
    let node = match get_process_node(slot, oldfd) {
        Some(n) => n,
        None => {
            if let Some(n) = GLOBAL_VFS.lock().get_node(oldfd).cloned() {
                n
            } else {
                return -9;
            }
        }
    };
    let mut cloned = node;
    cloned.fd = newfd;
    if let Some(ref p) = cloned.pipe {
        let mut pipe = p.lock();
        if cloned.is_pipe_write {
            pipe.writers_count += 1;
        } else {
            pipe.readers_count += 1;
        }
    }
    insert_process_fd(slot, newfd, cloned);
    newfd as i64
}

