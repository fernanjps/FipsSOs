#![forbid(unsafe_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use kernel_objects::HandleId;
use ostd::sync::SpinLock;

use super::devfs_sysfs::{FD_STDERR, FD_STDIN, FD_STDOUT};
use super::pipe::KernelPipe;

/// Metadata for a virtual device node, file, directory, or pipe backed by a Vmo or kernel circular pipe.
#[derive(Clone)]
pub struct VfsDeviceNode {
    pub fd: u64,
    pub path: String,
    pub handle: HandleId,
    pub file_size: usize,
    pub is_device: bool,
    pub offset: usize,
    pub flags: u64,
    pub is_dir: bool,
    pub pipe: Option<Arc<SpinLock<KernelPipe>>>,
    pub is_pipe_write: bool,
    pub socket_id: Option<u64>,
}

impl core::fmt::Debug for VfsDeviceNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VfsDeviceNode")
            .field("fd", &self.fd)
            .field("path", &self.path)
            .field("handle", &self.handle)
            .field("file_size", &self.file_size)
            .field("is_device", &self.is_device)
            .field("offset", &self.offset)
            .field("flags", &self.flags)
            .field("is_dir", &self.is_dir)
            .field("is_pipe_write", &self.is_pipe_write)
            .field("socket_id", &self.socket_id)
            .finish()
    }
}

impl VfsDeviceNode {
    pub fn new_vmo(
        fd: u64,
        path: String,
        handle: HandleId,
        file_size: usize,
        is_device: bool,
    ) -> Self {
        Self {
            fd,
            path,
            handle,
            file_size,
            is_device,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe: None,
            is_pipe_write: false,
            socket_id: None,
        }
    }

    pub fn new_dir(fd: u64, path: String) -> Self {
        Self {
            fd,
            path,
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: true,
            pipe: None,
            is_pipe_write: false,
            socket_id: None,
        }
    }

    pub fn new_socket(fd: u64, socket_id: u64) -> Self {
        Self {
            fd,
            path: alloc::format!("socket:[{}]", socket_id),
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe: None,
            is_pipe_write: false,
            socket_id: Some(socket_id),
        }
    }

    pub fn new_console(fd: u64, name: &str) -> Self {
        Self {
            fd,
            path: String::from(name),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            pipe: None,
            is_pipe_write: fd != FD_STDIN,
            socket_id: None,
        }
    }
}

/// Process-level file descriptor table tracking open file descriptions and redirections per process.
#[derive(Clone)]
pub struct ProcessFdTable {
    pub descriptors: BTreeMap<u64, VfsDeviceNode>,
    pub next_fd: u64,
}

impl ProcessFdTable {
    pub fn new() -> Self {
        let mut descriptors = BTreeMap::new();
        descriptors.insert(FD_STDIN, VfsDeviceNode::new_console(FD_STDIN, "stdin"));
        descriptors.insert(FD_STDOUT, VfsDeviceNode::new_console(FD_STDOUT, "stdout"));
        descriptors.insert(FD_STDERR, VfsDeviceNode::new_console(FD_STDERR, "stderr"));
        Self {
            descriptors,
            next_fd: 10,
        }
    }
}

pub static PROCESS_FD_MAP: SpinLock<BTreeMap<u64, ProcessFdTable>> =
    SpinLock::new(BTreeMap::new());

/// Retrieves a cloned `VfsDeviceNode` for descriptor `fd` belonging to process `slot`.
pub fn get_process_node(slot: u64, fd: u64) -> Option<VfsDeviceNode> {
    if let Some(table) = PROCESS_FD_MAP.lock().get(&slot) {
        if let Some(node) = table.descriptors.get(&fd) {
            return Some(node.clone());
        }
    }
    if fd == FD_STDIN {
        Some(VfsDeviceNode::new_console(FD_STDIN, "stdin"))
    } else if fd == FD_STDOUT {
        Some(VfsDeviceNode::new_console(FD_STDOUT, "stdout"))
    } else if fd == FD_STDERR {
        Some(VfsDeviceNode::new_console(FD_STDERR, "stderr"))
    } else {
        None
    }
}

/// Inserts or replaces a file descriptor in process `slot`'s table.
pub fn insert_process_fd(slot: u64, fd: u64, node: VfsDeviceNode) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    if let Some(prev) = table.descriptors.insert(fd, node) {
        if let Some(ref p) = prev.pipe {
            let mut pipe = p.lock();
            if prev.is_pipe_write {
                pipe.writers_count = pipe.writers_count.saturating_sub(1);
            } else {
                pipe.readers_count = pipe.readers_count.saturating_sub(1);
            }
        }
    }
    if fd >= table.next_fd {
        table.next_fd = fd + 1;
    }
}

/// Removes and returns a file descriptor from process `slot`'s table.
pub fn remove_process_fd(slot: u64, fd: u64) -> Option<VfsDeviceNode> {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.get_mut(&slot)?;
    let node = table.descriptors.remove(&fd)?;
    if let Some(ref p) = node.pipe {
        let mut pipe = p.lock();
        if node.is_pipe_write {
            pipe.writers_count = pipe.writers_count.saturating_sub(1);
        } else {
            pipe.readers_count = pipe.readers_count.saturating_sub(1);
        }
    }
    Some(node)
}

/// Clones parent process's file descriptor table to child process on `sys_fork`.
pub fn clone_process_fds(parent_slot: u64, child_slot: u64) {
    let mut map = PROCESS_FD_MAP.lock();
    let parent_table = map.get(&parent_slot).cloned().unwrap_or_else(ProcessFdTable::new);
    for node in parent_table.descriptors.values() {
        if let Some(ref p) = node.pipe {
            let mut pipe = p.lock();
            if node.is_pipe_write {
                pipe.writers_count += 1;
            } else {
                pipe.readers_count += 1;
            }
        }
    }
    map.insert(child_slot, parent_table);
}

/// Cleans up all open file descriptors for process `slot` upon termination.
pub fn cleanup_process_fds(slot: u64) {
    let mut map = PROCESS_FD_MAP.lock();
    if let Some(mut table) = map.remove(&slot) {
        for (_, node) in core::mem::take(&mut table.descriptors) {
            if let Some(ref p) = node.pipe {
                let mut pipe = p.lock();
                if node.is_pipe_write {
                    pipe.writers_count = pipe.writers_count.saturating_sub(1);
                } else {
                    pipe.readers_count = pipe.readers_count.saturating_sub(1);
                }
            }
        }
    }
}

