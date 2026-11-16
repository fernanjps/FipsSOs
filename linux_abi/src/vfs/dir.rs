#![forbid(unsafe_code)]

use alloc::string::String;

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
        if norm == "/mnt/fat0" || norm.starts_with("/mnt/fat0/") {
            return crate::fat32::is_dir(&norm);
        }
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

        if norm_dir == "/mnt/fat0" || norm_dir.starts_with("/mnt/fat0/") {
            let fat_entries = crate::fat32::list_dir(&norm_dir);
            for (name, is_dir, size) in [
                (String::from("."), true, 4096),
                (String::from(".."), true, 4096),
            ] {
                seen.insert(name.clone());
                entries.push(DirEntryInfo {
                    name,
                    is_dir,
                    is_chr: false,
                    size,
                });
            }
            for fe in fat_entries {
                if seen.insert(fe.name.clone()) {
                    entries.push(DirEntryInfo {
                        name: fe.name,
                        is_dir: fe.is_dir,
                        is_chr: false,
                        size: fe.size as usize,
                    });
                }
            }
            return entries;
        }

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
