#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use kernel_objects::{KernelObject, Rights, Vmo, VmoUsage};
use ostd::sync::SpinLock;

use super::devfs_sysfs::*;
use super::dir::normalize_path;
use super::fd_table::*;
use super::pipe::KernelPipe;
use super::CapabilityVfs;

fn dynamic_sysfs_text(path: &str) -> Result<Option<String>, i64> {
    if path == PATH_SYS_HWMON {
        Ok(Some(build_hwmon_text()))
    } else if path == PATH_PROC_MEMINFO {
        Ok(Some(build_meminfo_text()))
    } else if path == PATH_PROC_MOUNTS || path == PATH_PROC_SELF_MOUNTS || path == PATH_ETC_MTAB {
        Ok(Some(build_mounts_text()))
    } else if path == PATH_PROC_UPTIME {
        Ok(Some(build_uptime_text()))
    } else if path == PATH_PROC_VERSION {
        Ok(Some(build_version_text()))
    } else if path == PATH_PROC_CPUINFO {
        Ok(Some(build_cpuinfo_text()))
    } else if path == PATH_PROC_STAT {
        Ok(Some(build_stat_text()))
    } else if path == PATH_BAT0_CAPACITY {
        match build_bat0_capacity_text() {
            Some(t) => Ok(Some(t)),
            None => Err(-19i64), // -ENODEV: sin bateria fisica EC
        }
    } else if path == PATH_BAT0_STATUS {
        Ok(Some(build_bat0_status_text()))
    } else if path == PATH_BAT0_PROFILE {
        Ok(Some(build_bat0_profile_text()))
    } else if path == PATH_BAT0_BOOST {
        Ok(Some(build_bat0_boost_text()))
    } else if path == PATH_WLAN0_OPERSTATE {
        Ok(Some(build_wlan0_operstate_text()))
    } else {
        Ok(None)
    }
}

impl CapabilityVfs {
    fn open_fat32(&mut self, path: &str, flags: u64) -> Result<u64, i64> {
        const O_CREAT: u64 = 0x40;
        const O_TRUNC: u64 = 0x200;
        const O_APPEND: u64 = 0x400;

        if !crate::fat32::is_mounted() {
            return Err(-2i64); // -ENOENT
        }

        let data = match crate::fat32::read_file(path) {
            Ok(bytes) => {
                if (flags & O_TRUNC) != 0 {
                    crate::fat32::write_file(path, &[]).map_err(|_| -5i64)?;
                    alloc::vec::Vec::new()
                } else {
                    bytes
                }
            }
            Err(_) => {
                if (flags & O_CREAT) != 0 {
                    crate::fat32::write_file(path, &[]).map_err(|_| -2i64)?;
                    alloc::vec::Vec::new()
                } else {
                    return Err(-2i64); // -ENOENT
                }
            }
        };

        let file_size = data.len();
        let alloc_len = file_size.max(512 * 1024);
        let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
        if !data.is_empty() {
            file_vmo.write(0, &data).map_err(|_| -22i64)?;
        }
        let file_handle = self.handle_table.insert(
            Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        let fd = self.alloc_fd();
        let mut node = VfsDeviceNode::new_vmo(fd, String::from(path), file_handle, file_size, false);
        node.flags = flags;
        node.offset = if (flags & O_APPEND) != 0 { file_size } else { 0 };
        self.devices.insert(fd, node);
        Ok(fd)
    }

    pub fn open(&mut self, raw_path: &str, flags: u64) -> Result<u64, i64> {
        let path = normalize_path(raw_path);
        const O_CREAT: u64 = 0x40;
        const O_TRUNC: u64 = 0x200;
        const O_APPEND: u64 = 0x400;

        if self.is_directory(&path) {
            let fd = self.alloc_fd();
            let node = VfsDeviceNode::new_dir(fd, path);
            self.devices.insert(fd, node);
            return Ok(fd);
        }

        if path == PATH_DEV_NULL || (path.starts_with("/containers/") && path.ends_with("/dev/null")) {
            let fd = self.alloc_fd();
            let mut node = VfsDeviceNode::new_vmo(fd, path, kernel_objects::HandleId::null(), 0, true);
            node.flags = flags;
            self.devices.insert(fd, node);
            return Ok(fd);
        }

        if path.starts_with("/mnt/fat0/") {
            return self.open_fat32(&path, flags);
        }

        // Check if file exists in registered_files
        let existing = self.registered_files.get(&path).cloned();

        if let Some(mut meta) = existing {
            let fd = self.alloc_fd();

            if (flags & O_TRUNC) != 0 && !meta.is_device {
                meta.file_size = 0;
                if let Some(entry) = self.registered_files.get_mut(&path) {
                    entry.file_size = 0;
                }
                self.shared_vmo_state.remove(&meta.handle.raw());
                if path.starts_with("/disk/") {
                    let _ = crate::diskfs::save_file(&path, &[]);
                }
            }

            let mut node = VfsDeviceNode::new_vmo(fd, path.clone(), meta.handle, meta.file_size, meta.is_device);
            node.flags = flags;
            node.offset = if (flags & O_APPEND) != 0 { meta.file_size } else { 0 };

            if let Some(text) = dynamic_sysfs_text(&path)? {
                if let Ok(entry) = self.handle_table.get(node.handle, Rights::READ) {
                    if let Ok(vmo) = entry.as_vmo() {
                        let _ = vmo.write(0, text.as_bytes());
                        node.file_size = text.len();
                        self.shared_vmo_state.insert(node.handle.raw(), (text.len(), node.is_device));
                        if let Some(entry) = self.registered_files.get_mut(&path) {
                            entry.file_size = text.len();
                        }
                    }
                }
            }

            self.devices.insert(fd, node);
            return Ok(fd);
        }

        // If file does not exist, check O_CREAT
        if (flags & O_CREAT) != 0 {
            let alloc_len = 512 * 1024;
            let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile).map_err(|_| -12i64)?;
            let file_handle = self.handle_table.insert(
                Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
            );
            self.record_parent_dirs(&path);
            self.register_file(path.clone(), file_handle, 0, false);

            let fd = self.alloc_fd();
            let mut node = VfsDeviceNode::new_vmo(fd, path.clone(), file_handle, 0, false);
            node.flags = flags;

            if path.starts_with("/disk/") {
                let _ = crate::diskfs::save_file(&path, &[]);
            }

            self.devices.insert(fd, node);
            ostd::early_println!("[linux_abi::vfs] Archivo creado: \"{}\" -> fd={}", path, fd);
            return Ok(fd);
        }

        Err(-2i64) // -ENOENT
    }

    /// Closes an open capability file descriptor (`sys_close`, syscall 3) after verifying its
    /// `HandleId` and `Koid` in the capability table.
    pub fn close(&mut self, fd: u64) -> Result<u64, i64> {
        if fd <= FD_STDERR {
            if let Some(node) = self.devices.get(&fd) {
                if node.pipe.is_some() || !node.is_device {
                    if let Some(n) = self.devices.remove(&fd) {
                        if let Some(ref p) = n.pipe {
                            let mut pipe = p.lock();
                            if n.is_pipe_write {
                                pipe.writers_count = pipe.writers_count.saturating_sub(1);
                            } else {
                                pipe.readers_count = pipe.readers_count.saturating_sub(1);
                            }
                        }
                    }
                    return Ok(0);
                }
            }
            return Ok(0);
        }
        if let Some(node) = self.devices.remove(&fd) {
            if let Some(sock_id) = node.socket_id {
                let _ = ostd::net::tcp::close(sock_id);
                return Ok(0);
            }
            if let Some(ref p) = node.pipe {
                let mut pipe = p.lock();
                if node.is_pipe_write {
                    pipe.writers_count = pipe.writers_count.saturating_sub(1);
                } else {
                    pipe.readers_count = pipe.readers_count.saturating_sub(1);
                }
                return Ok(0);
            }
            if node.is_dir {
                return Ok(0);
            }
            return self.finish_closed_fd_node(&node).map(|_| 0);
        }
        Err(-9i64) // -EBADF
    }

    /// Reads up to `dst.len()` bytes from the `Vmo`, socket, or pipe backing `fd`.
    pub fn read_file(&mut self, fd: u64, dst: &mut [u8]) -> Result<usize, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if let Some(sock_id) = node.socket_id {
            return ostd::net::tcp::recv(sock_id, dst);
        }
        if let Some(ref pipe) = node.pipe {
            if node.is_pipe_write {
                return Err(-9i64);
            }
            let mut p = pipe.lock();
            let count = p.read(dst);
            return Ok(count);
        }

        if node.is_dir {
            return Err(-21i64); // -EISDIR
        }
        if node.path == PATH_DEV_NULL || (node.path.starts_with("/containers/") && node.path.ends_with("/dev/null")) {
            return Ok(0);
        }
        if node.path == PATH_AUDIO {
            return ostd::arch::x86_64::audio::read_audio(dst);
        }

        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;

        if node.offset >= node.file_size {
            return Ok(0); // EOF
        }
        let available = node.file_size - node.offset;
        let to_read = dst.len().min(available);
        if to_read > 0 {
            vmo.read(node.offset, &mut dst[..to_read]).map_err(|_| -22i64)?;
            node.offset += to_read;
        }
        Ok(to_read)
    }

    /// Writes up to `src.len()` bytes to the `Vmo`, socket, or pipe backing `fd`.
    pub fn write_file(&mut self, fd: u64, src: &[u8]) -> Result<usize, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if let Some(sock_id) = node.socket_id {
            return ostd::net::tcp::send(sock_id, src);
        }
        if let Some(ref pipe) = node.pipe {
            if !node.is_pipe_write {
                return Err(-9i64);
            }
            let mut p = pipe.lock();
            return p.write(src);
        }

        if node.is_dir {
            return Err(-21i64); // -EISDIR
        }
        if node.path == PATH_DEV_NULL || (node.path.starts_with("/containers/") && node.path.ends_with("/dev/null")) {
            return Ok(src.len());
        }
        if node.path == PATH_AUDIO {
            return ostd::arch::x86_64::audio::write_audio(src);
        }
        if node.path == PATH_BAT0_PROFILE {
            let text = core::str::from_utf8(src).map_err(|_| -22i64)?;
            if !text.trim().is_empty() {
                ostd::arch::x86_64::power_battery::set_profile(text).map_err(|_| -22i64)?;
                invalidate_hwmon_cache();
            }
            return Ok(src.len());
        }

        let entry = self
            .handle_table
            .get(node.handle, Rights::WRITE)
            .map_err(|_| -13i64)?;
        let vmo = entry.as_vmo().map_err(|_| -19i64)?;
        let cur_size = self
            .shared_vmo_state
            .get(&node.handle.raw())
            .map(|(sz, _)| *sz)
            .unwrap_or(node.file_size)
            .max(node.file_size);
        if (node.flags & 0x400) != 0 {
            node.offset = cur_size;
        }

        let write_end = node.offset.saturating_add(src.len());
        let to_write = if write_end > vmo.size_bytes() {
            let avail = vmo.size_bytes().saturating_sub(node.offset);
            if avail == 0 {
                return Err(-28i64); // -ENOSPC
            }
            src.len().min(avail)
        } else {
            src.len()
        };

        if node.offset > cur_size {
            let gap = node.offset - cur_size;
            let zeros = alloc::vec![0u8; gap.min(vmo.size_bytes().saturating_sub(cur_size))];
            if !zeros.is_empty() {
                vmo.write(cur_size, &zeros).map_err(|_| -22i64)?;
            }
        }
        vmo.write(node.offset, &src[..to_write]).map_err(|_| -22i64)?;
        node.offset += to_write;
        node.file_size = cur_size.max(node.offset);
        if let Some(meta) = self.registered_files.get_mut(&node.path) {
            meta.file_size = node.file_size;
        }
        node.dirty = true;
        self.shared_vmo_state.insert(node.handle.raw(), (node.file_size, true));

        Ok(to_write)
    }

    /// Seeks the file offset of `fd`.
    pub fn lseek(&mut self, fd: u64, offset: i64, whence: u64) -> Result<u64, i64> {
        let node = self.devices.get_mut(&fd).ok_or(-9i64)?;
        if node.is_device || node.pipe.is_some() {
            return Err(-29i64); // -ESPIPE
        }
        const SEEK_SET: u64 = 0;
        const SEEK_CUR: u64 = 1;
        const SEEK_END: u64 = 2;

        let new_offset = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (node.offset as i64).saturating_add(offset),
            SEEK_END => (node.file_size as i64).saturating_add(offset),
            _ => return Err(-22i64),
        };
        if new_offset < 0 {
            return Err(-22i64);
        }
        node.offset = new_offset as usize;
        Ok(node.offset as u64)
    }

    /// Creates an anonymous unidirectional pipe.
    pub fn pipe2(&mut self, _flags: u64) -> Result<(u64, u64), i64> {
        let pipe = Arc::new(SpinLock::new(KernelPipe::new()));
        let r_fd = self.alloc_fd();
        let w_fd = self.alloc_fd();

        let r_node = VfsDeviceNode::new_pipe(r_fd, Arc::clone(&pipe), false);
        let w_node = VfsDeviceNode::new_pipe(w_fd, pipe, true);
        self.devices.insert(r_fd, r_node);
        self.devices.insert(w_fd, w_node);
        Ok((r_fd, w_fd))
    }

    /// Duplicates a file descriptor.
    pub fn dup(&mut self, oldfd: u64) -> Result<u64, i64> {
        let newfd = self.alloc_fd();
        self.dup2(oldfd, newfd)
    }

    /// Duplicates a file descriptor to a specific descriptor `newfd`.
    pub fn dup2(&mut self, oldfd: u64, newfd: u64) -> Result<u64, i64> {
        if oldfd == newfd {
            if self.devices.contains_key(&oldfd) {
                return Ok(newfd);
            } else {
                return Err(-9i64);
            }
        }
        let node = self.devices.get(&oldfd).cloned().ok_or(-9i64)?;
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
        if let Some(prev) = self.devices.insert(newfd, cloned) {
            if let Some(ref p) = prev.pipe {
                let mut pipe = p.lock();
                if prev.is_pipe_write {
                    pipe.writers_count = pipe.writers_count.saturating_sub(1);
                } else {
                    pipe.readers_count = pipe.readers_count.saturating_sub(1);
                }
            }
        }
        Ok(newfd)
    }

    /// Creates a new TCP socket descriptor backed by `ostd::net::tcp`.
    pub fn create_socket_node(&mut self) -> Result<u64, i64> {
        let sock_id = ostd::net::tcp::create_socket()?;
        self.register_socket_id(sock_id)
    }

    /// Binds an existing `ostd::net::tcp` socket ID to a new VFS file descriptor.
    pub fn register_socket_id(&mut self, sock_id: u64) -> Result<u64, i64> {
        let fd = self.alloc_fd();
        let node = VfsDeviceNode::new_socket(fd, sock_id);
        self.devices.insert(fd, node);
        Ok(fd)
    }

    /// Returns the underlying TCP socket ID for `fd`, if it is a socket descriptor.
    pub fn get_socket_id(&self, fd: u64) -> Option<u64> {
        self.devices.get(&fd).and_then(|node| node.socket_id)
    }

    /// Returns an immutable reference to the VFS node for descriptor `fd`.
    pub fn get_node(&self, fd: u64) -> Option<&VfsDeviceNode> {
        self.devices.get(&fd)
    }

    /// Returns a mutable reference to the VFS node for descriptor `fd`.
    pub fn get_node_mut(&mut self, fd: u64) -> Option<&mut VfsDeviceNode> {
        self.devices.get_mut(&fd)
    }

    /// Returns the file size of the mounted device node or file with descriptor `fd`.
    pub fn get_file_size(&self, fd: u64) -> Option<usize> {
        self.devices.get(&fd).map(|node| node.file_size)
    }

    /// Returns the file size of the mounted file with the given `path`.
    pub fn get_file_size_by_path(&self, path: &str) -> Option<usize> {
        let norm = normalize_path(path);
        if norm == PATH_DEV_NULL || (norm.starts_with("/containers/") && norm.ends_with("/dev/null")) {
            return Some(0);
        }
        if norm == "/mnt/fat0" || norm.starts_with("/mnt/fat0/") {
            return crate::fat32::stat_path(&norm).map(|(_, sz)| sz);
        }
        if let Ok(Some(text)) = dynamic_sysfs_text(&norm) {
            return Some(text.len());
        }
        if norm == PATH_BAT0_CAPACITY {
            return None;
        }
        self.registered_files.get(&norm).map(|meta| meta.file_size)
    }

    /// Reads the entire contents of a mounted `TarFs` file by path (used by `sys_execve` to load ELF64 binaries).
    pub fn read_all_by_path(&self, path: &str) -> Result<alloc::vec::Vec<u8>, i64> {
        let norm = normalize_path(path);
        if norm.starts_with("/mnt/fat0/") {
            return crate::fat32::read_file(&norm).map_err(|_| -2i64);
        }
        if let Some(text) = dynamic_sysfs_text(&norm)? {
            return Ok(text.into_bytes());
        }
        if let Some(meta) = self.registered_files.get(&norm) {
            let entry = self
                .handle_table
                .get(meta.handle, Rights::READ)
                .map_err(|_| -13i64)?; // -EACCES
            let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV
            let mut bytes = vec![0u8; meta.file_size];
            if meta.file_size > 0 {
                vmo.read(0, &mut bytes).map_err(|_| -22i64)?;
            }
            return Ok(bytes);
        }
        Err(-2i64) // -ENOENT
    }

    /// Prints all mounted devices and `TarFs` `Initramfs` files with their `fd`, `Koid`, and size.
    pub fn list_nodes(&self) {
        ostd::early_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        ostd::fb_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        for node in self.devices.values() {
            if let Ok(entry) = self.handle_table.get(node.handle, Rights::READ) {
                let kind = if node.is_device {
                    "DEV"
                } else if node.path.starts_with("/disk/") {
                    "DSK"
                } else if node.path.starts_with("/downloads/") {
                    "NET"
                } else {
                    "TAR"
                };
                ostd::early_println!(
                    "  fd={:<2} | [{}] {:<10} | {:>9} B | {}",
                    node.fd,
                    kind,
                    entry.koid(),
                    node.file_size,
                    node.path
                );
                ostd::fb_println!(
                    "  fd={:<2} | [{}] {:<10} | {:>9} B | {}",
                    node.fd,
                    kind,
                    entry.koid(),
                    node.file_size,
                    node.path
                );
            }
        }
    }

}
