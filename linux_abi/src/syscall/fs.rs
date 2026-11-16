#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};
use ostd::mm::validate_user_buffer;

use crate::vfs::{
    get_process_node, insert_process_fd, remove_process_fd, update_process_node, VfsDeviceNode,
    FD_FB0, FD_STDERR, FD_STDIN, FD_STDOUT, PATH_DEV_NULL,
};
use super::{GLOBAL_VFS, TCGETS, TIOCGWINSZ};

pub use super::fs_dir::*;

fn is_dev_null(proc_node: &Option<VfsDeviceNode>, fd: u64) -> bool {
    let is_null_p = |p: &str| {
        p == PATH_DEV_NULL || (p.starts_with("/containers/") && p.ends_with("/dev/null"))
    };
    if let Some(ref node) = proc_node {
        return is_null_p(&node.path);
    }
    GLOBAL_VFS
        .lock()
        .get_node(fd)
        .map(|n| is_null_p(&n.path))
        .unwrap_or(false)
}

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

    if is_dev_null(&proc_node, fd) {
        if validate_user_buffer(user_buf_vaddr, len, true).is_err() {
            return -14; // -EFAULT
        }
        return 0;
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
        if let Some(mut pnode) = process_only_regular_node(slot, fd, &proc_node) {
            let res = GLOBAL_VFS.lock().read_node_vmo(&mut pnode, &mut kernel_buf[..max_len]);
            let n = match res {
                Ok(n) => n,
                Err(e) => return e as isize,
            };
            update_process_node(slot, fd, pnode);
            if n > 0 && write_user_bytes(user_buf_vaddr, &kernel_buf[..n]).is_err() {
                return -14;
            }
            return n as isize;
        }
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
        let ch = ostd::arch::keyboard::wait_for_input_event();
        if write_user_bytes(user_buf_vaddr, &[ch]).is_err() {
            return -14;
        }
        return 1;
    }

    let max_len = len.min(128);
    let mut line_buf = [0u8; 128];
    let mut pos = 0usize;

    loop {
        let ch = ostd::arch::keyboard::wait_for_input_event();
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


/// Opens an already-resolved path in `GLOBAL_VFS` and registers it in `PROCESS_FD_MAP`
/// after releasing `GLOBAL_VFS` (strict lock ordering).
pub(super) fn open_resolved_path(resolved: &str, flags: u64) -> i64 {
    if let Err(e) = super::sandbox::check_open_sandbox(resolved, flags) {
        return e;
    }
    let slot = ostd::scheduler::active_slot();
    let mut vfs = GLOBAL_VFS.lock();
    let res = vfs.open(resolved, flags);
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

/// Linux `sys_open` (syscall 2): resolves a null-terminated virtual device or file path from Ring 3
/// and returns its capability file descriptor.
pub fn sys_open(user_path_vaddr: u64, flags: u64, _mode: u64) -> i64 {
    let resolved = match read_user_resolved_path(user_path_vaddr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    open_resolved_path(&resolved, flags)
}


fn close_socket_node(fd: u64, sid: u64) {
    let mut close_hw_sock = false;
    {
        let mut vfs = GLOBAL_VFS.lock();
        if fd >= crate::vfs::FIRST_DYNAMIC_FD
            && vfs.get_node(fd).and_then(|g| g.socket_id) == Some(sid)
            && !crate::vfs::any_process_holds_fd(fd)
        {
            vfs.remove_device_node(fd);
        }
        if !crate::vfs::any_process_holds_socket(sid) {
            close_hw_sock = true;
        }
    }
    if close_hw_sock {
        let _ = ostd::net::tcp::close(sid);
    }
}

fn release_closed_fd_node(node: VfsDeviceNode) -> Result<(), i64> {
    if let Some(sid) = node.socket_id {
        close_socket_node(node.fd, sid);
        return Ok(());
    }
    let mut vfs = GLOBAL_VFS.lock();
    if node.fd >= crate::vfs::FIRST_DYNAMIC_FD && !crate::vfs::any_process_holds_fd(node.fd) {
        let global_same = vfs
            .get_node(node.fd)
            .map(|g| {
                g.handle == node.handle
                    && g.is_device == node.is_device
                    && g.is_dir == node.is_dir
                    && g.pipe.is_some() == node.pipe.is_some()
            })
            .unwrap_or(false);
        if global_same {
            vfs.remove_device_node(node.fd);
        }
    }
    vfs.finish_closed_fd_node(&node)
}

/// Linux `sys_close` (syscall 3): validates and closes an open capability file descriptor.
pub fn sys_close(fd: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    if let Some(node) = remove_process_fd(slot, fd) {
        return match release_closed_fd_node(node) {
            Ok(()) => 0,
            Err(e) => e,
        };
    }
    if fd <= FD_STDERR {
        return 0;
    }
    if crate::vfs::any_process_holds_fd(fd) {
        return -9; // -EBADF
    }
    match GLOBAL_VFS.lock().close(fd) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}

/// Flushes/releases regular-file nodes, dynamic FDs, and TCP sockets dropped from a process FD table.
pub fn finish_orphan_nodes(nodes: alloc::vec::Vec<VfsDeviceNode>) {
    for node in nodes {
        let _ = release_closed_fd_node(node);
    }
}

/// Reclaims residual open file descriptors/VMOs/sockets for any recently freed/killed slots.
pub fn reclaim_dead_process_fds() {
    ostd::scheduler::reap_orphan_zombies();
    let mask = ostd::scheduler::take_reclaimed_slots_mask();
    if mask == 0 {
        return;
    }
    for slot in 2..32u64 {
        if (mask & (1u32 << slot)) != 0 {
            let cid = crate::vfs::get_process_container_id(slot);
            finish_orphan_nodes(crate::vfs::cleanup_process_fds(slot));
            crate::container::on_process_exit(cid, 0);
        }
    }
}

/// Returns a process-local regular-file node when it is NOT mirrored in `GLOBAL_VFS` (dup'd FDs).
fn process_only_regular_node(slot: u64, fd: u64, proc_node: &Option<VfsDeviceNode>) -> Option<VfsDeviceNode> {
    let node = proc_node.as_ref()?;
    if node.is_device || node.is_dir || node.pipe.is_some() || node.socket_id.is_some() {
        return None;
    }
    let mirrored = GLOBAL_VFS.lock().get_node(fd).map(|g| g.handle == node.handle).unwrap_or(false);
    if mirrored {
        return None;
    }
    let _ = slot;
    Some(node.clone())
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

    if is_dev_null(&proc_node, fd) {
        if validate_user_buffer(user_buf_vaddr, len, false).is_err() {
            return -14; // -EFAULT
        }
        return len as isize;
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

    if let Some(mut pnode) = process_only_regular_node(slot, fd, &proc_node) {
        if let Err(e) = super::sandbox::check_write_fd_sandbox(&pnode.path, pnode.is_device, pnode.pipe.is_some()) {
            return e as isize;
        }
        let mut written = 0usize;
        let mut kernel_buf = [0u8; 2048];
        while written < len {
            let chunk = core::cmp::min(len - written, kernel_buf.len());
            if read_user_bytes(user_buf_vaddr + (written as u64), &mut kernel_buf[..chunk]).is_err() {
                break;
            }
            match GLOBAL_VFS.lock().write_node_vmo(&mut pnode, &kernel_buf[..chunk]) {
                Ok(n) => {
                    written += n;
                    if n < chunk {
                        break;
                    }
                }
                Err(e) => {
                    if written == 0 {
                        return e as isize;
                    }
                    break;
                }
            }
        }
        update_process_node(slot, fd, pnode);
        return if written > 0 { written as isize } else { -14 };
    }

    if is_pipe_or_file || (fd != FD_STDOUT && fd != FD_STDERR && fd != FD_FB0) {
        if let Some(ref node) = proc_node {
            if let Err(e) = super::sandbox::check_write_fd_sandbox(&node.path, node.is_device, node.pipe.is_some()) {
                return e as isize;
            }
        }
        let mut written = 0usize;
        let mut kernel_buf = [0u8; 2048];
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


/// Linux `sys_ioctl` (syscall 16): dispatches device control operations, including POSIX terminal
/// inquiries (`TCGETS` 0x5401 and `TIOCGWINSZ` 0x5413) and capability devices.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> i64 {
    if request == TCGETS || request == TIOCGWINSZ {
        // Return 0 only for actual interactive console/terminal streams, not pipes or regular files
        let slot = ostd::scheduler::active_slot();
        let is_tty = {
            if let Some(node) = get_process_node(slot, fd) {
                node.is_device
                    && node.pipe.is_none()
                    && node.path != PATH_DEV_NULL
                    && (fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR)
            } else {
                let vfs = GLOBAL_VFS.lock();
                if let Some(node) = vfs.get_node(fd) {
                    node.is_device
                        && node.pipe.is_none()
                        && node.path != PATH_DEV_NULL
                        && (fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR)
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
    if fd == FD_FB0 {
        if let Err(e) = super::sandbox::check_raw_fb_sandbox() {
            return e;
        }
    }

    if request == crate::vfs::FIPSOS_IOCTL_LIVE_KERNEL_TELEMETRY {
        reclaim_dead_process_fds();
    }
    if request == crate::vfs::FIPSOS_IOCTL_CONTAINER_CTL {
        return crate::container::dispatch_container_ioctl(arg);
    }

    match GLOBAL_VFS.lock().ioctl(fd, request, arg) {
        Ok(val) => val as i64,
        Err(errno) => errno,
    }
}


/// Linux `sys_lseek` (syscall 8): repositions read/write file offset.
pub fn sys_lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR {
        return -29; // -ESPIPE
    }
    let slot = ostd::scheduler::active_slot();
    let proc_node = get_process_node(slot, fd);
    if let Some(mut pnode) = process_only_regular_node(slot, fd, &proc_node) {
        const SEEK_SET: u64 = 0;
        const SEEK_CUR: u64 = 1;
        const SEEK_END: u64 = 2;
        let new_offset = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (pnode.offset as i64).saturating_add(offset),
            SEEK_END => (pnode.file_size as i64).saturating_add(offset),
            _ => return -22,
        };
        if new_offset < 0 {
            return -22;
        }
        pnode.offset = new_offset as usize;
        update_process_node(slot, fd, pnode);
        return new_offset;
    }
    match GLOBAL_VFS.lock().lseek(fd, offset, whence) {
        Ok(pos) => {
            if let Some(mut pnode) = proc_node {
                pnode.offset = pos as usize;
                update_process_node(slot, fd, pnode);
            }
            pos as i64
        }
        Err(e) => e,
    }
}


