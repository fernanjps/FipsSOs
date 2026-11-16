#![forbid(unsafe_code)]

use alloc::sync::Arc;
use kernel_objects::{KernelObject, Koid, Rights, Vmo, VmoUsage};

use super::dir::normalize_path;
use super::fd_table::VfsDeviceNode;
use super::CapabilityVfs;

impl CapabilityVfs {
    fn release_handle_if_unused(&mut self, handle: kernel_objects::HandleId) {
        if handle == kernel_objects::HandleId::null() {
            return;
        }
        let still_used = self.devices.values().any(|n| n.handle == handle)
            || self.registered_files.values().any(|m| m.handle == handle);
        if !still_used {
            let _ = self.handle_table.remove(handle);
        }
    }

    /// Creates or atomically replaces a regular VMO-backed file node and releases stale handles.
    fn upsert_vmo_file(
        &mut self,
        path: &str,
        payload: &[u8],
        rights: Rights,
    ) -> Result<(u64, Koid), i64> {
        let norm = normalize_path(path);
        let alloc_len = payload.len().max(4096);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !payload.is_empty() {
            file_vmo.write(0, payload).map_err(|_| -22i64)?;
        }
        let koid = file_vmo.koid();
        let file_handle = self
            .handle_table
            .insert(Arc::clone(&file_vmo) as Arc<dyn KernelObject>, rights);

        let old_handle = self.registered_files.get(&norm).map(|m| m.handle);
        let existing_fd = self
            .devices
            .values()
            .find(|n| n.path == norm)
            .map(|n| n.fd);
        let fd = match existing_fd {
            Some(reuse_fd) => reuse_fd,
            None => self.alloc_fd(),
        };

        self.record_parent_dirs(&norm);
        self.register_file(norm.clone(), file_handle, payload.len(), false);
        self.devices.insert(
            fd,
            VfsDeviceNode::new_vmo(fd, norm.clone(), file_handle, payload.len(), false),
        );

        if let Some(stale) = old_handle {
            self.release_handle_if_unused(stale);
        }

        Ok((fd, koid))
    }

    /// Dynamically creates a brand-new `Vmo` containing `payload` and mounts or updates `path`
    /// inside `CapabilityVfs` so downloaded files (e.g. `/downloads/fipsos_pkg.txt`) are readable.
    pub fn create_or_update_downloaded_file(
        &mut self,
        path: &'static str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        self.upsert_vmo_file(path, payload, Rights::READ | Rights::MAP | Rights::TRANSFER)
    }

    /// Dynamically creates or updates a persistent `Vmo` backed file on the SATA disk
    /// (e.g. `/disk/persistent_note.txt`).
    pub fn create_or_update_disk_file(
        &mut self,
        path: &str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        self.upsert_vmo_file(
            path,
            payload,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        )
    }

    /// Removes a file node by path from the VFS device directory and releases its capability handle.
    pub fn remove_node(&mut self, path: &str) -> bool {
        let norm = normalize_path(path);
        let removed_meta = self.registered_files.remove(&norm);
        let mut dev_handles = alloc::vec::Vec::new();
        self.devices.retain(|_, n| {
            if n.path == norm {
                dev_handles.push(n.handle);
                false
            } else {
                true
            }
        });
        let removed = removed_meta.is_some() || !dev_handles.is_empty();
        if let Some(meta) = removed_meta {
            self.release_handle_if_unused(meta.handle);
        }
        for handle in dev_handles {
            self.release_handle_if_unused(handle);
        }
        removed
    }

    pub(super) fn flush_backing_node(&self, node: &VfsDeviceNode) -> Result<(), i64> {
        if node.is_device || (!node.path.starts_with("/disk/") && !node.path.starts_with("/mnt/fat0/")) {
            return Ok(());
        }
        let entry = self.handle_table.get(node.handle, Rights::READ).map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        let mut buf = alloc::vec![0u8; node.file_size];
        if node.file_size > 0 {
            vmo.read(0, &mut buf).map_err(|_| -22i64)?;
        }
        if node.path.starts_with("/disk/") {
            crate::diskfs::save_file(&node.path, &buf).map_err(|_| -5i64)?;
        } else if node.path.starts_with("/mnt/fat0/") {
            crate::fat32::write_file(&node.path, &buf).map_err(|_| -5i64)?;
        }
        Ok(())
    }

    /// Flushes dirty persistent backing state for an open descriptor (`fsync` / `fdatasync`).
    pub fn fsync_fd(&mut self, fd: u64) -> Result<(), i64> {
        let node = self.devices.get(&fd).ok_or(-9i64)?.clone();
        if node.pipe.is_some() || node.socket_id.is_some() || node.is_device {
            return Err(-22i64); // -EINVAL
        }
        if node.dirty {
            self.flush_backing_node(&node)?;
            if let Some(n) = self.devices.get_mut(&fd) {
                n.dirty = false;
            }
        }
        Ok(())
    }

    /// Truncates or zero-extends an open file descriptor (`ftruncate`).
    pub fn truncate_fd(&mut self, fd: u64, new_len: usize) -> Result<(), i64> {
        let (handle_raw, old_len, node_path) = {
            let node = self.devices.get(&fd).ok_or(-9i64)?;
            if node.is_dir || node.is_device || node.pipe.is_some() || node.socket_id.is_some() {
                return Err(-22i64);
            }
            (node.handle, node.file_size, node.path.clone())
        };
        let entry = self.handle_table.get(handle_raw, Rights::WRITE).map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        if new_len > vmo.size_bytes() {
            return Err(-27i64); // -EFBIG
        }
        if new_len > old_len {
            let zeros = alloc::vec![0u8; new_len - old_len];
            vmo.write(old_len, &zeros).map_err(|_| -22i64)?;
        } else if new_len < old_len {
            let zeros = alloc::vec![0u8; old_len - new_len];
            vmo.write(new_len, &zeros).map_err(|_| -22i64)?;
        }
        self.shared_vmo_state.insert(handle_raw.raw(), (new_len, true));
        for other in self.devices.values_mut() {
            if other.handle.raw() == handle_raw.raw() {
                other.file_size = new_len;
                other.dirty = true;
            }
        }
        if let Some(meta) = self.registered_files.get_mut(&node_path) {
            meta.file_size = new_len;
        }
        Ok(())
    }

    /// Checks whether `path` refers to an existing node on FAT32 or VFS (for `chmod`/`truncate`).
    pub fn path_exists(&self, raw_path: &str) -> bool {
        let norm = normalize_path(raw_path);
        if norm == "/" || (norm.starts_with("/mnt/fat0") && crate::fat32::stat_path(&norm).is_some()) {
            return true;
        }
        self.registered_files.contains_key(&norm) || self.is_directory(&norm)
    }

    /// Truncates an arbitrary path (`truncate`).
    pub fn truncate_path(&mut self, raw_path: &str, new_len: usize) -> Result<(), i64> {
        let norm = normalize_path(raw_path);
        if norm.starts_with("/mnt/fat0/") {
            let mut data = crate::fat32::read_file(&norm).map_err(|_| -2i64)?;
            data.resize(new_len, 0);
            crate::fat32::write_file(&norm, &data).map_err(|_| -5i64)?;
            for node in self.devices.values_mut() {
                if node.path == norm {
                    if let Ok(entry) = self.handle_table.get(node.handle, Rights::WRITE) {
                        if let Ok(vmo) = entry.as_vmo() {
                            if new_len <= vmo.size_bytes() {
                                let _ = vmo.write(0, &data);
                            }
                        }
                    }
                    node.file_size = new_len;
                    node.dirty = false;
                }
            }
            return Ok(());
        }
        let fd_match = self.devices.values().find(|n| n.path == norm).map(|n| n.fd);
        if let Some(fd) = fd_match {
            self.truncate_fd(fd, new_len)?;
            return self.fsync_fd(fd);
        }
        if let Some(meta) = self.registered_files.get_mut(&norm) {
            let entry = self.handle_table.get(meta.handle, Rights::WRITE).map_err(|_| -13i64)?;
            let vmo = entry.as_vmo().map_err(|_| -19i64)?;
            if new_len > vmo.size_bytes() {
                return Err(-27i64);
            }
            if new_len > meta.file_size {
                let zeros = alloc::vec![0u8; new_len - meta.file_size];
                vmo.write(meta.file_size, &zeros).map_err(|_| -22i64)?;
            } else if new_len < meta.file_size {
                let zeros = alloc::vec![0u8; meta.file_size - new_len];
                vmo.write(new_len, &zeros).map_err(|_| -22i64)?;
            }
            meta.file_size = new_len;
            if norm.starts_with("/disk/") {
                let mut buf = alloc::vec![0u8; new_len];
                if new_len > 0 {
                    vmo.read(0, &mut buf).map_err(|_| -22i64)?;
                }
                crate::diskfs::save_file(&norm, &buf).map_err(|_| -5i64)?;
            }
            return Ok(());
        }
        Err(-2i64)
    }

    /// Writes `src` into a regular VMO node (used for process-dup'd FDs absent from `devices`).
    pub fn write_node_vmo(&mut self, node: &mut VfsDeviceNode, src: &[u8]) -> Result<usize, i64> {
        if node.path == super::devfs_sysfs::PATH_BAT0_PROFILE {
            let text = core::str::from_utf8(src).map_err(|_| -22i64)?;
            if !text.trim().is_empty() {
                ostd::arch::x86_64::power_battery::set_profile(text).map_err(|_| -22i64)?;
                super::devfs_sysfs::invalidate_hwmon_cache();
            }
            return Ok(src.len());
        }
        let entry = self.handle_table.get(node.handle, Rights::WRITE).map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        let base_size = self
            .shared_vmo_state
            .get(&node.handle.raw())
            .map(|(sz, _)| *sz)
            .unwrap_or(node.file_size)
            .max(node.file_size);
        if (node.flags & 0x400) != 0 {
            node.offset = base_size;
        }
        let write_end = node.offset.saturating_add(src.len());
        let to_write = if write_end > vmo.size_bytes() {
            let avail = vmo.size_bytes().saturating_sub(node.offset);
            if avail == 0 {
                return Err(-28i64);
            }
            src.len().min(avail)
        } else {
            src.len()
        };
        if node.offset > base_size {
            let gap = (node.offset - base_size).min(vmo.size_bytes().saturating_sub(base_size));
            if gap > 0 {
                let zeros = alloc::vec![0u8; gap];
                vmo.write(base_size, &zeros).map_err(|_| -22i64)?;
            }
        }
        vmo.write(node.offset, &src[..to_write]).map_err(|_| -22i64)?;
        node.offset += to_write;
        node.file_size = base_size.max(node.offset);
        node.dirty = true;
        if let Some(meta) = self.registered_files.get_mut(&node.path) {
            meta.file_size = node.file_size;
        }
        self.shared_vmo_state.insert(node.handle.raw(), (node.file_size, true));
        Ok(to_write)
    }

    /// Reads into `dst` from a regular VMO node (used for process-dup'd FDs absent from `devices`).
    pub fn read_node_vmo(&self, node: &mut VfsDeviceNode, dst: &mut [u8]) -> Result<usize, i64> {
        let entry = self.handle_table.get(node.handle, Rights::READ).map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        let total_sz = self
            .shared_vmo_state
            .get(&node.handle.raw())
            .map(|(sz, _)| *sz)
            .unwrap_or(node.file_size)
            .max(node.file_size);
        if node.offset >= total_sz {
            return Ok(0);
        }
        let to_read = dst.len().min(total_sz - node.offset);
        if to_read > 0 {
            vmo.read(node.offset, &mut dst[..to_read]).map_err(|_| -22i64)?;
            node.offset += to_read;
        }
        Ok(to_read)
    }

    /// Flushes and releases a closed file node only when no device or process FD still holds its handle.
    pub fn finish_closed_fd_node(&mut self, node: &VfsDeviceNode) -> Result<(), i64> {
        if node.is_device || node.is_dir || node.pipe.is_some() || node.socket_id.is_some() {
            return Ok(());
        }
        let any_open = self.devices.values().any(|n| n.handle == node.handle)
            || super::fd_table::any_process_holds_handle(node.handle);
        if any_open {
            return Ok(());
        }
        let (shared_sz, shared_dirty) = self
            .shared_vmo_state
            .remove(&node.handle.raw())
            .unwrap_or((node.file_size, node.dirty));
        let mut effective = node.clone();
        effective.file_size = shared_sz.max(node.file_size);
        effective.dirty = node.dirty || shared_dirty;
        let flush_res = if effective.dirty {
            self.flush_backing_node(&effective)
        } else {
            Ok(())
        };
        let still_in_reg = self.registered_files.values().any(|m| m.handle == node.handle);
        if !still_in_reg && node.handle != kernel_objects::HandleId::null() {
            let _ = self.handle_table.remove(node.handle);
        }
        flush_res
    }
}
