#![forbid(unsafe_code)]

//! Descriptor duplication, pipes, `fcntl` and `sendfile` syscalls.

use ostd::arch::syscall::write_user_bytes;
use ostd::sync::SpinLock;

use crate::vfs::fd_table::{
    alloc_unified_fd, get_process_node, insert_process_fd, remove_process_fd,
    reserve_unified_fd, PROCESS_FD_MAP,
};
use crate::vfs::{KernelPipe, ProcessFdTable, VfsDeviceNode, FD_STDERR, FD_STDOUT};

use super::fs::finish_orphan_nodes;
use super::GLOBAL_VFS;

pub const F_DUPFD: u64 = 0;
pub const F_GETFD: u64 = 1;
pub const F_SETFD: u64 = 2;
pub const F_GETFL: u64 = 3;
pub const F_SETFL: u64 = 4;
pub const F_DUPFD_CLOEXEC: u64 = 1030;
pub const FD_CLOEXEC: u64 = 1;

/// Linux `sys_sendfile` (syscall 40): copies data between descriptors in kernel space.
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

/// Linux `sys_pipe2` (syscall 293 / 22): creates an anonymous pipe pair `[read_fd, write_fd]`.
pub fn sys_pipe2(pipefd_addr: u64, _flags: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let pipe = alloc::sync::Arc::new(SpinLock::new(KernelPipe::new()));
    let r_fd = alloc_unified_fd();
    let w_fd = alloc_unified_fd();

    let r_node = VfsDeviceNode::new_pipe(r_fd, alloc::sync::Arc::clone(&pipe), false);
    let w_node = VfsDeviceNode::new_pipe(w_fd, pipe, true);

    {
        let mut map = PROCESS_FD_MAP.lock();
        let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
        table.descriptors.insert(r_fd, r_node);
        table.descriptors.insert(w_fd, w_node);
        if w_fd >= table.next_fd {
            table.next_fd = w_fd + 1;
        }
    }

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
    let newfd = alloc_unified_fd();
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
    insert_process_fd(slot, newfd, cloned)
        .into_iter()
        .for_each(|prev| finish_orphan_nodes(alloc::vec![prev]));
    newfd as i64
}

/// Linux `sys_fcntl` (syscall 72): file descriptor duplication and flag control.
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let source = match get_process_node(slot, fd) {
                Some(n) => n,
                None => return -9, // -EBADF
            };
            let mut candidate = arg.max(3);
            {
                let map = PROCESS_FD_MAP.lock();
                if let Some(table) = map.get(&slot) {
                    while table.descriptors.contains_key(&candidate) {
                        candidate += 1;
                    }
                }
            }
            if candidate >= 10 {
                reserve_unified_fd(candidate);
            }
            let mut cloned = source;
            cloned.fd = candidate;
            if let Some(ref p) = cloned.pipe {
                let mut pipe = p.lock();
                if cloned.is_pipe_write {
                    pipe.writers_count += 1;
                } else {
                    pipe.readers_count += 1;
                }
            }
            insert_process_fd(slot, candidate, cloned);
            candidate as i64
        }
        F_GETFD => {
            let node = match get_process_node(slot, fd) {
                Some(n) => n,
                None => return -9, // -EBADF
            };
            if (node.flags & 0x80000) != 0 {
                FD_CLOEXEC as i64
            } else {
                0
            }
        }
        F_SETFD => {
            let mut map = PROCESS_FD_MAP.lock();
            if let Some(table) = map.get_mut(&slot) {
                if let Some(node) = table.descriptors.get_mut(&fd) {
                    if (arg & FD_CLOEXEC) != 0 {
                        node.flags |= 0x80000;
                    } else {
                        node.flags &= !0x80000;
                    }
                    return 0;
                }
            }
            if fd <= 2 { 0 } else { -9 }
        }
        F_GETFL => match get_process_node(slot, fd) {
            Some(node) => (node.flags & !0x80000) as i64,
            None => -9,
        },
        F_SETFL => {
            let mut map = PROCESS_FD_MAP.lock();
            if let Some(table) = map.get_mut(&slot) {
                if let Some(node) = table.descriptors.get_mut(&fd) {
                    let cloexec = node.flags & 0x80000;
                    node.flags = (arg & !0x80000) | cloexec;
                    return 0;
                }
            }
            if fd <= 2 { 0 } else { -9 }
        }
        _ => 0,
    }
}
