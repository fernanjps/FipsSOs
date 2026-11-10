#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use kernel_objects::{KernelObject, Koid, Rights, Vmo, VmoUsage};

use super::fd_table::VfsDeviceNode;
use super::CapabilityVfs;

pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub is_chr: bool,
    pub size: usize,
}

/// Normalizes an absolute path string resolving `.` and `..` segments.
pub fn normalize_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() || p == "/" {
        return String::from("/");
    }
    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => {
                parts.push(other);
            }
        }
    }
    if parts.is_empty() {
        return String::from("/");
    }
    let mut out = String::new();
    for seg in parts {
        out.push('/');
        out.push_str(seg);
    }
    out
}

fn path_parent(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return "/";
    }
    match p.rfind('/') {
        Some(0) => "/",
        Some(idx) => &p[..idx],
        None => "/",
    }
}

fn path_basename(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(idx) => &p[idx + 1..],
        None => p,
    }
}


impl CapabilityVfs {
    pub fn is_directory(&self, path: &str) -> bool {
        let norm = normalize_path(path);
        self.directories.contains(&norm)
    }

    /// Adds a directory path to the VFS.
    pub fn add_directory(&mut self, path: &str) {
        let norm = normalize_path(path);
        self.record_parent_dirs(&norm);
        self.directories.insert(norm);
    }

    /// Lists directory contents for `sys_getdents64`.
    pub fn list_directory(&self, dir_path: &str) -> alloc::vec::Vec<DirEntryInfo> {
        let norm_dir = normalize_path(dir_path);
        let mut entries = alloc::vec::Vec::new();
        let mut seen = alloc::collections::BTreeSet::new();

        entries.push(DirEntryInfo {
            name: String::from("."),
            is_dir: true,
            is_chr: false,
            size: 4096,
        });
        entries.push(DirEntryInfo {
            name: String::from(".."),
            is_dir: true,
            is_chr: false,
            size: 4096,
        });
        seen.insert(String::from("."));
        seen.insert(String::from(".."));

        // Direct child directories
        for d in &self.directories {
            if d == &norm_dir || d == "/" {
                continue;
            }
            if path_parent(d) == norm_dir.as_str() {
                let name = String::from(path_basename(d));
                if !seen.contains(&name) {
                    seen.insert(name.clone());
                    entries.push(DirEntryInfo {
                        name,
                        is_dir: true,
                        is_chr: false,
                        size: 4096,
                    });
                }
            }
        }

        // Direct child files & devices
        for (fpath, meta) in &self.registered_files {
            let parent = path_parent(fpath);
            if parent == norm_dir.as_str() {
                let name = String::from(path_basename(fpath));
                if !seen.contains(&name) {
                    seen.insert(name.clone());
                    entries.push(DirEntryInfo {
                        name,
                        is_dir: false,
                        is_chr: meta.is_device,
                        size: meta.file_size,
                    });
                }
            }
        }

        entries
    }

    /// Dynamically creates a brand-new `Vmo` containing `payload` and mounts or updates `path`
    /// inside `CapabilityVfs` so downloaded files (e.g. `/downloads/fipsos_pkg.txt`) are immediately
    /// visible via `ls` and readable via `sys_open`/`sys_read` (`cat`).
    pub fn create_or_update_downloaded_file(
        &mut self,
        path: &'static str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        let alloc_len = payload.len().max(4096);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !payload.is_empty() {
            file_vmo.write(0, payload).map_err(|_| -22i64)?;
        }
        let koid = file_vmo.koid();
        let file_handle = self.handle_table.insert(
            Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );

        let existing_fd = self
            .devices
            .values()
            .find(|n| n.path == path)
            .map(|n| n.fd);
        let fd = match existing_fd {
            Some(reuse_fd) => reuse_fd,
            None => {
                let fresh_fd = self.next_fd;
                self.next_fd += 1;
                fresh_fd
            }
        };

        self.record_parent_dirs(path);
        self.register_file(String::from(path), file_handle, payload.len(), false);
        self.devices.insert(
            fd,
            VfsDeviceNode::new_vmo(
                fd,
                String::from(path),
                file_handle,
                payload.len(),
                false,
            ),
        );

        Ok((fd, koid))
    }

    /// Dynamically creates or updates a persistent `Vmo` backed file on the SATA disk
    /// (e.g. `/disk/persistent_note.txt`).
    pub fn create_or_update_disk_file(
        &mut self,
        path: &str,
        payload: &[u8],
    ) -> Result<(u64, Koid), i64> {
        let alloc_len = payload.len().max(4096);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !payload.is_empty() {
            file_vmo.write(0, payload).map_err(|_| -22i64)?;
        }
        let koid = file_vmo.koid();
        let file_handle = self.handle_table.insert(
            Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );

        let existing_fd = self
            .devices
            .values()
            .find(|n| n.path == path)
            .map(|n| n.fd);
        let fd = match existing_fd {
            Some(reuse_fd) => reuse_fd,
            None => {
                let fresh_fd = self.next_fd;
                self.next_fd += 1;
                fresh_fd
            }
        };

        self.record_parent_dirs(path);
        self.register_file(String::from(path), file_handle, payload.len(), false);
        self.devices.insert(
            fd,
            VfsDeviceNode::new_vmo(
                fd,
                String::from(path),
                file_handle,
                payload.len(),
                false,
            ),
        );

        Ok((fd, koid))
    }

    /// Removes a file node by path from the VFS device directory and releases its capability handle.
    pub fn remove_node(&mut self, path: &str) -> bool {
        let norm = normalize_path(path);
        let removed_meta = self.registered_files.remove(&norm);
        self.devices.retain(|_, n| n.path != norm);
        if let Some(meta) = removed_meta {
            let _ = self.handle_table.remove(meta.handle);
            return true;
        }
        false
    }

    /// Removes a directory by path from the VFS directory set.
    pub fn remove_directory(&mut self, path: &str) -> bool {
        let norm = normalize_path(path);
        if norm == "/" || norm == "/disk" {
            return false;
        }
        let has_child_file = self
            .registered_files
            .keys()
            .any(|f| path_parent(f) == norm.as_str());
        let has_child_dir = self
            .directories
            .iter()
            .any(|d| d != &norm && path_parent(d) == norm.as_str());
        if has_child_file || has_child_dir {
            return false;
        }
        self.devices.retain(|_, n| n.path != norm);
        self.directories.remove(&norm)
    }

}
