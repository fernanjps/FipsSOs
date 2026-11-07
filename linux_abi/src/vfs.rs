#![forbid(unsafe_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use kernel_objects::{HandleId, HandleTable, KernelObject, Rights, SchedulerRegistry, Vmo, VmoUsage};
use ostd::ffi_jail::{c_npu_driver_entry, FfiJail};
use ostd::mm::map_vmo_to_user;

use crate::tarfs::TarFs;

/// Standard POSIX / Linux file descriptors and FipSOs device node descriptors.
pub const FD_STDIN: u64 = 0;
pub const FD_STDOUT: u64 = 1;
pub const FD_STDERR: u64 = 2;
pub const FD_DRM_CARD0: u64 = 3;
pub const FD_ACCEL_NPU0: u64 = 4;
pub const FD_FB0: u64 = 5;

/// Canonical virtual device and `Initramfs` file paths exposed by the FipSOs capability VFS.
pub const PATH_DRM_CARD0: &str = "/dev/dri/card0";
pub const PATH_ACCEL_NPU0: &str = "/dev/accel/accel0";
pub const PATH_FB0: &str = "/dev/fb0";
pub const PATH_ETC_MOTD: &str = "/etc/motd.txt";
pub const PATH_MODEL_AVATAR: &str = "/models/avatar_v1.bin";
pub const PATH_BIN_NPU_INFER: &str = "/bin/npu_infer.elf";
pub const PATH_BIN_AVATAR_CPP: &str = "/bin/avatar_cpp.elf";

/// FipSOs ioctl command codes for `/dev/dri/card0` (DRM/Vulkan), `/dev/accel/accel0` (NPU/IA),
/// `/dev/fb0` (Framebuffer Zero-Copy presentation), and VFS directory listing (`ls`).
pub const DRM_IOCTL_PRIME_ZERO_COPY: u64 = 0xC010_6400;
pub const ACCEL_IOCTL_BIND_TENSOR_VMO: u64 = 0xC010_4100;
pub const ACCEL_IOCTL_RUN_INFERENCE: u64 = 0xC010_4102;
pub const FB_IOCTL_PRESENT_RECT: u64 = 0xC010_4601;
pub const FB_IOCTL_POLL_KEY: u64 = 0xC010_4602;
pub const VFS_IOCTL_LIST_FILES: u64 = 0xC010_5601;
pub const VFS_IOCTL_LIST_PROCESSES: u64 = 0xC010_5001;

/// Default top-right overlay coordinates and dimensions for `/dev/fb0` Zero-Copy presentation.
pub const FB_WIDGET_DEFAULT_X: usize = 1096;
pub const FB_WIDGET_DEFAULT_Y: usize = 68;
pub const FB_WIDGET_DEFAULT_W: usize = 160;
pub const FB_WIDGET_DEFAULT_H: usize = 80;

/// Linux `mmap` protection bit for writable mappings (`PROT_WRITE`).
pub const PROT_WRITE: u64 = 0x2;

/// Metadata for a virtual device node or `Initramfs` (`TarFs`) file backed by a `Vmo` `HandleId`.
#[derive(Debug, Clone)]
pub struct VfsDeviceNode {
    pub fd: u64,
    pub path: String,
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
    next_fd: u64,
}

impl CapabilityVfs {
    /// Creates an unmounted `CapabilityVfs`.
    pub const fn new() -> Self {
        Self {
            handle_table: HandleTable::new(),
            devices: BTreeMap::new(),
            next_fd: 6,
        }
    }

    /// Mounts `/dev/dri/card0`, `/dev/accel/accel0`, `/dev/fb0` and populates the VFS with
    /// `Vmo`-backed files parsed from the Limine `initramfs.tar` module (or in-memory USTAR fallback).
    pub fn mount_default_nodes(&mut self) {
        if !self.devices.is_empty() {
            return;
        }

        const DEVICE_VMO_SIZE: usize = 1024 * 1024; // 1 MiB Zero-Copy buffer per accelerator node

        let drm_vmo = Vmo::new(DEVICE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/dri/card0");
        drm_vmo
            .write(0, b"FIPSOS_DRM_VULKAN_FRAME_V1")
            .expect("Fallo al inicializar cabecera DRM VMO");
        let drm_handle = self.handle_table.insert(
            Arc::clone(&drm_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.devices.insert(
            FD_DRM_CARD0,
            VfsDeviceNode {
                fd: FD_DRM_CARD0,
                path: String::from(PATH_DRM_CARD0),
                handle: drm_handle,
                file_size: DEVICE_VMO_SIZE,
                is_device: true,
            },
        );

        let accel_vmo = Vmo::new(DEVICE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/accel/accel0");
        accel_vmo
            .write(0, b"FIPSOS_NPU_ACCEL_TENSOR_V1")
            .expect("Fallo al inicializar cabecera Accel VMO");
        let accel_handle = self.handle_table.insert(
            Arc::clone(&accel_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.devices.insert(
            FD_ACCEL_NPU0,
            VfsDeviceNode {
                fd: FD_ACCEL_NPU0,
                path: String::from(PATH_ACCEL_NPU0),
                handle: accel_handle,
                file_size: DEVICE_VMO_SIZE,
                is_device: true,
            },
        );

        let fb0_vmo = Vmo::new(DEVICE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/fb0");
        fb0_vmo
            .write(0, b"FIPSOS_FB0_CONSOLE_SURFACE")
            .expect("Fallo al inicializar cabecera FB0 VMO");
        let fb0_handle = self.handle_table.insert(
            Arc::clone(&fb0_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.devices.insert(
            FD_FB0,
            VfsDeviceNode {
                fd: FD_FB0,
                path: String::from(PATH_FB0),
                handle: fb0_handle,
                file_size: DEVICE_VMO_SIZE,
                is_device: true,
            },
        );

        // Parse USTAR Initramfs from Limine ModuleRequest or fallback in-memory USTAR image
        let fallback_tar;
        let (tar_bytes, source_tag) = if let Some(module_slice) = ostd::boot::initramfs_slice() {
            (module_slice, "Limine Module initramfs.tar")
        } else {
            fallback_tar = TarFs::build_fallback_tar();
            (fallback_tar.as_slice(), "TarFs In-Memory Fallback")
        };

        let tar_entries = TarFs::parse(tar_bytes);
        let mut mounted_files = 0usize;

        for entry in tar_entries {
            let alloc_len = entry.data.len().max(4096);
            let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile)
                .expect("Fallo al reservar VMO para archivo de Initramfs");
            if !entry.data.is_empty() {
                file_vmo
                    .write(0, entry.data)
                    .expect("Fallo al copiar contenido USTAR al VMO");
            }
            let file_handle = self.handle_table.insert(
                Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                Rights::READ | Rights::MAP | Rights::TRANSFER,
            );
            let fd = self.next_fd;
            self.next_fd += 1;

            self.devices.insert(
                fd,
                VfsDeviceNode {
                    fd,
                    path: entry.path,
                    handle: file_handle,
                    file_size: entry.data.len(),
                    is_device: false,
                },
            );
            mounted_files += 1;
        }

        ostd::early_println!(
            "[linux_abi::vfs] VFS montado: 3 dispositivos (/dev/*) + {} archivos TarFs ({})",
            mounted_files,
            source_tag
        );
        ostd::fb_println!(
            "[linux_abi::vfs] VFS montado: 3 nodos /dev/* + {} archivos TarFs ({})",
            mounted_files,
            source_tag
        );
    }

    /// Opens a mounted capability VFS node by path (`/dev/...`, `/etc/motd.txt`, `/models/avatar_v1.bin`, etc.)
    /// and returns its file descriptor.
    pub fn open(&self, path: &str, flags: u64) -> Result<u64, i64> {
        for node in self.devices.values() {
            if node.path == path {
                ostd::early_println!(
                    "[linux_abi::vfs] sys_open(path=\"{}\", flags={:#x}) -> fd={} ({}, size={} B)",
                    node.path,
                    flags,
                    node.fd,
                    node.handle,
                    node.file_size
                );
                ostd::fb_println!(
                    "[linux_abi::vfs] sys_open(path=\"{}\", flags={:#x}) -> fd={} ({}, {} B)",
                    node.path,
                    flags,
                    node.fd,
                    node.handle,
                    node.file_size
                );
                return Ok(node.fd);
            }
        }
        Err(-2i64) // -ENOENT
    }

    /// Closes an open capability file descriptor (`sys_close`, syscall 3) after verifying its
    /// `HandleId` and `Koid` in the capability table.
    pub fn close(&self, fd: u64) -> Result<u64, i64> {
        if fd <= FD_STDERR {
            return Ok(0);
        }
        let node = self.devices.get(&fd).ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -9i64)?; // -EBADF

        ostd::early_println!(
            "[linux_abi::vfs] sys_close(fd={} \"{}\") -> {} ({}) liberado OK",
            node.fd,
            node.path,
            node.handle,
            entry.koid()
        );
        ostd::fb_println!(
            "[linux_abi::vfs] sys_close(fd={} \"{}\") -> {} liberado OK",
            node.fd,
            node.path,
            node.handle
        );
        Ok(0)
    }

    /// Reads up to `dst.len()` bytes from the `Vmo` backing `fd` (used by `sys_read` on regular files).
    pub fn read_file(&self, fd: u64, dst: &mut [u8]) -> Result<usize, i64> {
        let node = self.devices.get(&fd).ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        let to_read = dst.len().min(node.file_size);
        if to_read > 0 {
            vmo.read(0, &mut dst[..to_read]).map_err(|_| -22i64)?;
        }
        Ok(to_read)
    }

    /// Reads the entire contents of a mounted `TarFs` file by path (used by `sys_execve` to load ELF64 binaries).
    pub fn read_all_by_path(&self, path: &str) -> Result<alloc::vec::Vec<u8>, i64> {
        for node in self.devices.values() {
            if node.path == path {
                let entry = self
                    .handle_table
                    .get(node.handle, Rights::READ)
                    .map_err(|_| -13i64)?; // -EACCES
                let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV
                let mut bytes = vec![0u8; node.file_size];
                if node.file_size > 0 {
                    vmo.read(0, &mut bytes).map_err(|_| -22i64)?;
                }
                return Ok(bytes);
            }
        }
        Err(-2i64) // -ENOENT
    }

    /// Prints all mounted devices and `TarFs` `Initramfs` files with their `fd`, `Koid`, and size.
    pub fn list_nodes(&self) {
        ostd::early_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        ostd::fb_println!("[VFS::ls] Listado de capacidades y archivos montados en /:");
        for node in self.devices.values() {
            if let Ok(entry) = self.handle_table.get(node.handle, Rights::READ) {
                let kind = if node.is_device { "DEV" } else { "TAR" };
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

    /// Redirects an `ioctl` syscall on `/dev/dri/card0`, `/dev/accel/accel0`, or `/dev/fb0` to its
    /// underlying `Vmo` Zero-Copy object after checking `Rights::READ | Rights::WRITE`.
    pub fn ioctl(&mut self, fd: u64, request: u64, arg: u64) -> Result<u64, i64> {
        if request == VFS_IOCTL_LIST_FILES {
            self.list_nodes();
            return Ok(self.devices.len() as u64);
        }
        if request == VFS_IOCTL_LIST_PROCESSES {
            let count = SchedulerRegistry::print_ps_table();
            return Ok(count as u64);
        }
        if request == FB_IOCTL_POLL_KEY {
            let key_byte = ostd::arch::keyboard::try_pop_key_nonblocking().unwrap_or(0);
            return Ok(key_byte as u64);
        }

        let node = self.devices.get(&fd).cloned().ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ | Rights::WRITE)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        if request == FB_IOCTL_PRESENT_RECT {
            let (width, height) = if arg != 0 {
                let w = ((arg >> 16) & 0xFFFF) as usize;
                let h = (arg & 0xFFFF) as usize;
                (w.clamp(16, 256), h.clamp(16, 256))
            } else {
                (FB_WIDGET_DEFAULT_W, FB_WIDGET_DEFAULT_H)
            };
            let byte_len = width * height * 4;
            let mut pixel_bytes = vec![0u8; byte_len];
            vmo.read(0, &mut pixel_bytes).map_err(|_| -22i64)?;

            ostd::fb_console::present_vmo_rect(
                &pixel_bytes,
                FB_WIDGET_DEFAULT_X,
                FB_WIDGET_DEFAULT_Y,
                width,
                height,
            );
            ostd::early_println!(
                "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", FB_IOCTL_PRESENT_RECT {}x{}) -> Blit Zero-Copy desde VMO {} (Phys={:#x})",
                fd,
                node.path,
                width,
                height,
                vmo.koid(),
                vmo.phys_addr()
            );
            return Ok((width * height) as u64);
        }

        if request == ACCEL_IOCTL_RUN_INFERENCE {
            let model_fd = arg;
            let model_node = self.devices.get(&model_fd).cloned().ok_or(-9i64)?;
            let model_entry = self
                .handle_table
                .get(model_node.handle, Rights::READ)
                .map_err(|_| -13i64)?;
            let model_vmo = model_entry.as_vmo().map_err(|_| -19i64)?;

            let sample_len = model_node.file_size.min(64);
            let mut model_header = vec![0u8; sample_len];
            model_vmo.read(0, &mut model_header).map_err(|_| -22i64)?;

            // 1. Zero-Copy bind input tensor shard into /dev/accel/accel0 VMO at offset 64
            let _ = vmo.write(64, &model_header);

            // 2. Execute real C11 hardware NPU driver (`c_npu_tensor_mac_kernel` in `ostd/c_src/npu_hw_driver.c`)
            //    strictly confined inside `ostd::ffi_jail::FfiJail::execute_c_function`!
            let ffi_jail = FfiJail::new();
            let dma_region_id = ffi_jail.register_dma_region(
                vmo.dma_buffer(),
                true,
                "NPU_C11_MAC_DMA_TENSOR_RW",
            );
            let ticks = ostd::arch::idt::timer_ticks();
            let mac_score = ffi_jail
                .execute_c_function(
                    dma_region_id,
                    0,
                    256,
                    true,
                    0x2026_F195u64 ^ ticks,
                    c_npu_driver_entry,
                )
                .map_err(|_| -5i64)? as u32;

            let mut fp32_bits_buf = [0u8; 4];
            let _ = vmo.read(128, &mut fp32_bits_buf);
            let fp32_t0_bits = u32::from_le_bytes(fp32_bits_buf);

            let mut milli_buf = [0u8; 4];
            let _ = vmo.read(216, &mut milli_buf);
            let milli_t0 = i32::from_le_bytes(milli_buf).max(0) as u32;

            let mut sig_bytes = [0u8; 20];
            let _ = vmo.read(192, &mut sig_bytes);
            let c_sig = core::str::from_utf8(&sig_bytes).unwrap_or("C11_NPU_FP32_SSE2_OK");

            ostd::early_println!(
                "[linux_abi::npu] Driver C11 ({}) en {}: \"{}\" -> \"{}\" | Sig=\"{}\" | FP32[0]={}.{:03} ({:#010x}) | Energy={}.{:03}",
                dma_region_id,
                vmo.koid(),
                model_node.path,
                node.path,
                c_sig,
                milli_t0 / 1000,
                milli_t0 % 1000,
                fp32_t0_bits,
                mac_score / 1000,
                mac_score % 1000
            );
            ostd::fb_println!(
                "[linux_abi::npu] Driver C11 ({}) Sig=\"{}\" | FP32[0]={}.{:03} ({:#010x}) | Energy={}.{:03}",
                dma_region_id,
                c_sig,
                milli_t0 / 1000,
                milli_t0 % 1000,
                fp32_t0_bits,
                mac_score / 1000,
                mac_score % 1000
            );
            return Ok(mac_score as u64);
        }

        let mut tag_buf = [0u8; 26];
        let _ = vmo.read(0, &mut tag_buf);
        let tag_str = core::str::from_utf8(&tag_buf).unwrap_or("<bin>");
        let ticks = ostd::arch::idt::timer_ticks();

        ostd::early_println!(
            "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", cmd={:#x}, arg={:#x}) -> VMO {} (Phys={:#x}, Tag=\"{}\", TimerIRQ0={} ticks)",
            fd,
            node.path,
            request,
            arg,
            vmo.koid(),
            vmo.phys_addr(),
            tag_str,
            ticks
        );
        ostd::fb_println!(
            "[linux_abi::vfs] sys_ioctl(fd={} \"{}\", cmd={:#x}) -> VMO {} (Phys={:#x}, TimerIRQ0={} ticks)",
            fd,
            node.path,
            request,
            vmo.koid(),
            vmo.phys_addr(),
            ticks
        );

        Ok(vmo.koid().raw())
    }

    /// Redirects an `mmap` syscall on `/dev/dri/card0`, `/dev/accel/accel0`, `/dev/fb0`, or a `TarFs` file
    /// to its underlying `Vmo` Zero-Copy object, mapping physical frames into Lower-Half User Space (`0x1000_0000+`).
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
