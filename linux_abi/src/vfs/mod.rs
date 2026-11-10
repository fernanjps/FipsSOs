#![forbid(unsafe_code)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use kernel_objects::{HandleId, HandleTable, KernelObject, Rights};
use ostd::mm::map_vmo_to_user;

pub mod devfs_sysfs;
pub mod dir;
pub mod fd_table;
pub mod ioctl;
pub mod mount;
pub mod ops;
pub mod pipe;

pub use devfs_sysfs::*;
pub use dir::*;
pub use fd_table::*;
pub use pipe::*;

#[derive(Clone, Debug)]
pub struct VfsFileMeta {
    pub handle: HandleId,
    pub file_size: usize,
    pub is_device: bool,
}

/// Capability-oriented Virtual File System unifying hardware device nodes (`/dev/dri/card0`,
/// `/dev/accel/accel0`, `/dev/fb0`) and USTAR `Initramfs` files (`/etc/motd.txt`, `/models/avatar_v1.bin`, etc.)
/// where every node is backed by a `Vmo` in `kernel_objects`.
pub struct CapabilityVfs {
    handle_table: HandleTable,
    devices: BTreeMap<u64, VfsDeviceNode>,
    pub registered_files: BTreeMap<String, VfsFileMeta>,
    directories: BTreeSet<String>,
    pub next_fd: u64,
}


impl CapabilityVfs {
    /// Creates an unmounted `CapabilityVfs`.
    pub const fn new() -> Self {
        Self {
            handle_table: HandleTable::new(),
            devices: BTreeMap::new(),
            registered_files: BTreeMap::new(),
            directories: BTreeSet::new(),
            next_fd: 10,
        }
    }

    pub fn register_file(&mut self, path: String, handle: HandleId, file_size: usize, is_device: bool) {
        let norm = normalize_path(&path);
        self.record_parent_dirs(&norm);
        self.registered_files.insert(
            norm,
            VfsFileMeta {
                handle,
                file_size,
                is_device,
            },
        );
    }

    pub fn record_parent_dirs(&mut self, path: &str) {
        let norm = normalize_path(path);
        let mut curr = norm.as_str();
        while let Some(idx) = curr.rfind('/') {
            if idx == 0 {
                self.directories.insert(String::from("/"));
                break;
            }
            let parent = &curr[..idx];
            self.directories.insert(String::from(parent));
            curr = parent;
        }
    }

    pub fn mmap(
        &self,
        _addr: u64,
        len: usize,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: usize,
    ) -> Result<u64, i64> {
        if len == 0 {
            return Err(-22i64); // -EINVAL
        }

        let writable = (prot & PROT_WRITE) != 0;

        // Support standard Linux MAP_ANONYMOUS (flags & 0x20 != 0 or fd == !0) for malloc/mmap
        if (flags & MAP_ANONYMOUS) != 0 || fd == u64::MAX || (fd as i64) == -1 {
            let mapped_addr = ostd::mm::map_anonymous_user_memory(len, writable).map_err(|_| -12i64)?;
            ostd::early_println!(
                "[linux_abi::vfs] sys_mmap(MAP_ANONYMOUS, len={}, prot={:#x}) -> UserVirt={:#x}",
                len,
                prot,
                mapped_addr
            );
            return Ok(mapped_addr);
        }

        let required_rights = if writable {
            Rights::READ | Rights::WRITE | Rights::MAP
        } else {
            Rights::READ | Rights::MAP
        };

        let node = self.devices.get(&fd).ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, required_rights)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        if offset.saturating_add(len) > vmo.size_bytes() {
            return Err(-22i64); // -EINVAL
        }

        let target_phys = vmo.phys_addr() + (offset as u64);
        let user_vaddr = map_vmo_to_user(target_phys, len, writable).map_err(|_| -12i64)?; // -ENOMEM

        ostd::early_println!(
            "[linux_abi::vfs] sys_mmap(fd={} \"{}\", len={} B, prot={:#x}, flags={:#x}) -> Zero-Copy VMO {} mapeado en Anillo 3 UserVirt={:#x} (Phys={:#x})",
            fd,
            node.path,
            len,
            prot,
            flags,
            vmo.koid(),
            user_vaddr,
            target_phys
        );
        ostd::fb_println!(
            "[linux_abi::vfs] sys_mmap(fd={} \"{}\", len={} B) -> Zero-Copy VMO {} en UserVirt={:#x} (Phys={:#x})",
            fd,
            node.path,
            len,
            vmo.koid(),
            user_vaddr,
            target_phys
        );

        Ok(user_vaddr)
    }

    /// Reads bytes directly from the underlying `Vmo` of a mounted device node (`fd`).
    pub fn read_device_vmo(&self, fd: u64, offset: usize, dst: &mut [u8]) -> Result<(), i64> {
        let node = self.devices.get(&fd).ok_or(-9i64)?;
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        vmo.read(offset, dst).map_err(|_| -22i64)
    }
}

impl Default for CapabilityVfs {
    fn default() -> Self {
        Self::new()
    }
}
