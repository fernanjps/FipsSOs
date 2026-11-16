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
    pub dirty: bool,
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
            dirty: false,
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
            dirty: false,
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
            dirty: false,
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
            dirty: false,
            pipe: None,
            is_pipe_write: fd != FD_STDIN,
            socket_id: None,
        }
    }

    pub fn new_pipe(fd: u64, pipe: Arc<SpinLock<KernelPipe>>, is_pipe_write: bool) -> Self {
        Self {
            fd,
            path: alloc::format!("pipe:[{}]", fd),
            handle: HandleId::null(),
            file_size: 0,
            is_device: false,
            offset: 0,
            flags: 0,
            is_dir: false,
            dirty: false,
            pipe: Some(pipe),
            is_pipe_write,
            socket_id: None,
        }
    }
}

/// Process-level file descriptor table tracking open file descriptions and redirections per process.
#[derive(Clone)]
pub struct ProcessFdTable {
    pub descriptors: BTreeMap<u64, VfsDeviceNode>,
    pub next_fd: u64,
    pub cwd: String,
    pub program_name: String,
    pub root_dir: String,
    pub hostname: String,
    pub container_id: u32,
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
            cwd: String::from("/"),
            program_name: String::new(),
            root_dir: String::from("/"),
            hostname: String::from("fipsos-pc"),
            container_id: 0,
        }
    }
}

/// Returns the Mount namespace root directory for process `slot` (defaults to `"/"`).
pub fn get_process_root(slot: u64) -> String {
    PROCESS_FD_MAP
        .lock()
        .get(&slot)
        .map(|t| if t.root_dir.is_empty() { String::from("/") } else { t.root_dir.clone() })
        .unwrap_or_else(|| String::from("/"))
}

/// Updates the Mount namespace root directory for process `slot`.
pub fn set_process_root(slot: u64, new_root: String) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    table.root_dir = new_root;
}

/// Returns the UTS namespace hostname for process `slot` (defaults to `"fipsos-pc"`).
pub fn get_process_hostname(slot: u64) -> String {
    PROCESS_FD_MAP
        .lock()
        .get(&slot)
        .map(|t| if t.hostname.is_empty() { String::from("fipsos-pc") } else { t.hostname.clone() })
        .unwrap_or_else(|| String::from("fipsos-pc"))
}

/// Updates the UTS namespace hostname for process `slot`.
pub fn set_process_hostname(slot: u64, new_host: String) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    table.hostname = new_host;
}

/// Returns the container cgroup ID bound to process `slot` (`0` = host).
pub fn get_process_container_id(slot: u64) -> u32 {
    PROCESS_FD_MAP.lock().get(&slot).map(|t| t.container_id).unwrap_or(0)
}

/// Updates the container cgroup ID bound to process `slot`.
pub fn set_process_container_id(slot: u64, cid: u32) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    table.container_id = cid;
}

/// Returns all active process slots belonging to container `cid`.
pub fn collect_container_slots(cid: u32) -> alloc::vec::Vec<u64> {
    if cid == 0 {
        return alloc::vec::Vec::new();
    }
    PROCESS_FD_MAP
        .lock()
        .iter()
        .filter(|(_, t)| t.container_id == cid)
        .map(|(&slot, _)| slot)
        .collect()
}

/// Returns the isolated working directory for process `slot` (defaults to `"/"`).
pub fn get_process_cwd(slot: u64) -> String {
    PROCESS_FD_MAP
        .lock()
        .get(&slot)
        .map(|t| if t.cwd.is_empty() { String::from("/") } else { t.cwd.clone() })
        .unwrap_or_else(|| String::from("/"))
}

/// Updates the isolated working directory for process `slot`.
pub fn set_process_cwd(slot: u64, new_cwd: String) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    table.cwd = new_cwd;
}

/// Returns the isolated active program name for process `slot`.
pub fn get_process_program_name(slot: u64) -> String {
    PROCESS_FD_MAP
        .lock()
        .get(&slot)
        .map(|t| t.program_name.clone())
        .unwrap_or_default()
}

/// Updates the isolated active program name for process `slot`.
pub fn set_process_program_name(slot: u64, name: String) {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    table.program_name = name;
}

pub static PROCESS_FD_MAP: SpinLock<BTreeMap<u64, ProcessFdTable>> =
    SpinLock::new(BTreeMap::new());

/// First dynamic file descriptor (`0..=9` are reserved for stdio and static `/dev/*` nodes).
pub const FIRST_DYNAMIC_FD: u64 = 10;

static NEXT_UNIFIED_FD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(FIRST_DYNAMIC_FD);

/// Allocates a unified descriptor number (`>= 10`) shared across `CapabilityVfs` and `ProcessFdTable`.
pub fn alloc_unified_fd() -> u64 {
    NEXT_UNIFIED_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

/// Ensures subsequent `alloc_unified_fd()` calls stay strictly above `fd`.
pub fn reserve_unified_fd(fd: u64) {
    let target = fd.saturating_add(1);
    let mut cur = NEXT_UNIFIED_FD.load(core::sync::atomic::Ordering::Relaxed);
    while cur < target {
        match NEXT_UNIFIED_FD.compare_exchange_weak(
            cur,
            target,
            core::sync::atomic::Ordering::Relaxed,
            core::sync::atomic::Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// Synchronizes `file_size` and `dirty` on any descriptors in `slot`'s table matching `handle_raw`.
pub fn sync_process_fd_size(slot: u64, handle_raw: u32, new_len: usize) {
    if let Some(table) = PROCESS_FD_MAP.lock().get_mut(&slot) {
        for node in table.descriptors.values_mut() {
            if node.handle.raw() == handle_raw {
                node.file_size = new_len;
                node.dirty = true;
            }
        }
    }
}

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

/// Inserts or replaces a file descriptor in process `slot`'s table, returning any displaced node.
pub fn insert_process_fd(slot: u64, fd: u64, node: VfsDeviceNode) -> Option<VfsDeviceNode> {
    let mut map = PROCESS_FD_MAP.lock();
    let table = map.entry(slot).or_insert_with(ProcessFdTable::new);
    let displaced = table.descriptors.insert(fd, node);
    if let Some(ref prev) = displaced {
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
    if fd >= FIRST_DYNAMIC_FD {
        reserve_unified_fd(fd);
    }
    displaced
}

/// Updates an existing descriptor node in `slot`'s table.
pub fn update_process_node(slot: u64, fd: u64, node: VfsDeviceNode) {
    if let Some(table) = PROCESS_FD_MAP.lock().get_mut(&slot) {
        table.descriptors.insert(fd, node);
    }
}

/// Returns `true` if any active process holds an open FD backed by `handle`.
pub fn any_process_holds_handle(handle: kernel_objects::HandleId) -> bool {
    if handle == kernel_objects::HandleId::null() {
        return false;
    }
    PROCESS_FD_MAP
        .lock()
        .values()
        .any(|t| t.descriptors.values().any(|n| n.handle == handle))
}

/// Returns `true` if any active process holds an open FD backed by TCP `socket_id`.
pub fn any_process_holds_socket(socket_id: u64) -> bool {
    PROCESS_FD_MAP
        .lock()
        .values()
        .any(|t| t.descriptors.values().any(|n| n.socket_id == Some(socket_id)))
}

/// Returns `true` if any active process still references descriptor number `fd`.
pub fn any_process_holds_fd(fd: u64) -> bool {
    PROCESS_FD_MAP
        .lock()
        .values()
        .any(|t| t.descriptors.contains_key(&fd))
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

/// Cleans up all open file descriptors for process `slot` upon termination,
/// returning closed regular file nodes so their VMO handles/dirty state can be flushed.
pub fn cleanup_process_fds(slot: u64) -> alloc::vec::Vec<VfsDeviceNode> {
    let mut orphans = alloc::vec::Vec::new();
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
            } else if node.socket_id.is_some()
                || node.fd >= FIRST_DYNAMIC_FD
                || (!node.is_device && !node.is_dir)
            {
                orphans.push(node);
            }
        }
    }
    orphans
}

