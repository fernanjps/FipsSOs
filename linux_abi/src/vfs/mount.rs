#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use kernel_objects::{HandleId, KernelObject, Rights, Vmo, VmoUsage};

use crate::tarfs::TarFs;
use super::devfs_sysfs::*;
use super::fd_table::*;
use super::CapabilityVfs;

impl CapabilityVfs {
    pub fn mount_default_nodes(&mut self) {
        if !self.devices.is_empty() {
            return;
        }

        self.directories.insert(String::from("/"));
        self.directories.insert(String::from("/bin"));
        self.directories.insert(String::from("/dev"));
        self.directories.insert(String::from("/dev/dri"));
        self.directories.insert(String::from("/dev/accel"));
        self.directories.insert(String::from("/dev/input"));
        self.directories.insert(String::from("/dev/net"));
        self.directories.insert(String::from("/dev/snd"));
        self.directories.insert(String::from("/dev/shm"));
        self.directories.insert(String::from("/dev/ipc"));
        self.directories.insert(String::from("/disk"));
        self.directories.insert(String::from("/disk/bin"));
        self.directories.insert(String::from("/downloads"));
        self.directories.insert(String::from("/etc"));
        self.directories.insert(String::from("/models"));
        self.directories.insert(String::from("/proc"));
        self.directories.insert(String::from("/proc/self"));
        self.directories.insert(String::from("/sys"));
        self.directories.insert(String::from("/sys/fipsos"));
        self.directories.insert(String::from("/sys/devices"));
        self.directories.insert(String::from("/sys/devices/system"));
        self.directories.insert(String::from("/sys/devices/system/cpu"));
        self.directories.insert(String::from("/tmp"));

        // Standard streams /dev/stdin (0), /dev/stdout (1), /dev/stderr (2)
        self.devices.insert(FD_STDIN, VfsDeviceNode {
            fd: FD_STDIN,
            path: String::from("/dev/stdin"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            dirty: false,
            pipe: None,
            is_pipe_write: false,
            socket_id: None,
        });
        self.devices.insert(FD_STDOUT, VfsDeviceNode {
            fd: FD_STDOUT,
            path: String::from("/dev/stdout"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            dirty: false,
            pipe: None,
            is_pipe_write: false,
            socket_id: None,
        });
        self.devices.insert(FD_STDERR, VfsDeviceNode {
            fd: FD_STDERR,
            path: String::from("/dev/stderr"),
            handle: HandleId::null(),
            file_size: 0,
            is_device: true,
            offset: 0,
            flags: 0,
            is_dir: false,
            dirty: false,
            pipe: None,
            is_pipe_write: false,
            socket_id: None,
        });

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
        self.record_parent_dirs(PATH_DRM_CARD0);
        self.register_file(String::from(PATH_DRM_CARD0), drm_handle, DEVICE_VMO_SIZE, true);
        self.devices.insert(
            FD_DRM_CARD0,
            VfsDeviceNode::new_vmo(
                FD_DRM_CARD0,
                String::from(PATH_DRM_CARD0),
                drm_handle,
                DEVICE_VMO_SIZE,
                true,
            ),
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
        self.record_parent_dirs(PATH_ACCEL_NPU0);
        self.register_file(String::from(PATH_ACCEL_NPU0), accel_handle, DEVICE_VMO_SIZE, true);
        self.devices.insert(
            FD_ACCEL_NPU0,
            VfsDeviceNode::new_vmo(
                FD_ACCEL_NPU0,
                String::from(PATH_ACCEL_NPU0),
                accel_handle,
                DEVICE_VMO_SIZE,
                true,
            ),
        );

        const FB0_VMO_SIZE: usize = 16 * 1024 * 1024; // 16 MiB Zero-Copy Multi-Resolution Backbuffer
        let fb0_vmo = Vmo::new(FB0_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy de 16 MiB para /dev/fb0");
        fb0_vmo
            .write(0, b"FIPSOS_FB0_CONSOLE_SURFACE")
            .expect("Fallo al inicializar cabecera FB0 VMO");
        let fb0_handle = self.handle_table.insert(
            Arc::clone(&fb0_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_FB0);
        self.register_file(String::from(PATH_FB0), fb0_handle, FB0_VMO_SIZE, true);
        self.devices.insert(
            FD_FB0,
            VfsDeviceNode::new_vmo(
                FD_FB0,
                String::from(PATH_FB0),
                fb0_handle,
                FB0_VMO_SIZE,
                true,
            ),
        );

        const MOUSE_VMO_SIZE: usize = 4096;
        let mouse_vmo = Vmo::new(MOUSE_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy para /dev/input/mouse0");
        mouse_vmo
            .write(0, b"FIPSOS_PS2_MOUSE0_INPUT_V1")
            .expect("Fallo al inicializar cabecera Mouse0 VMO");
        let boot_mouse_snap = ostd::arch::mouse::poll_state();
        let _ = sync_mouse_vmo(&mouse_vmo, &boot_mouse_snap);
        let mouse_handle = self.handle_table.insert(
            Arc::clone(&mouse_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_MOUSE0);
        self.register_file(String::from(PATH_MOUSE0), mouse_handle, MOUSE_VMO_SIZE, true);
        self.devices.insert(
            FD_MOUSE0,
            VfsDeviceNode::new_vmo(
                FD_MOUSE0,
                String::from(PATH_MOUSE0),
                mouse_handle,
                MOUSE_VMO_SIZE,
                true,
            ),
        );

        const NET_VMO_SIZE: usize = 64 * 1024; // 64 KiB Zero-Copy network ring/payload VMO
        let net_vmo = Vmo::new(NET_VMO_SIZE, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO Zero-Copy de 64 KiB para /dev/net/eth0");
        net_vmo
            .write(0, b"FIPSOS_NET_ETH0_E1000_DMA1")
            .expect("Fallo al inicializar cabecera Net0 VMO");
        let hw_snap = ostd::net::snapshot();
        let _ = net_vmo.write(32, &hw_snap.mac);
        let net_handle = self.handle_table.insert(
            Arc::clone(&net_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_NET0);
        self.register_file(String::from(PATH_NET0), net_handle, NET_VMO_SIZE, true);
        self.devices.insert(
            FD_NET0,
            VfsDeviceNode::new_vmo(
                FD_NET0,
                String::from(PATH_NET0),
                net_handle,
                NET_VMO_SIZE,
                true,
            ),
        );

        let audio_vmo = Vmo::new(4096, VmoUsage::ZeroCopyAccel)
            .expect("Fallo al reservar VMO para /dev/audio");
        let audio_info = ostd::arch::x86_64::audio::status_summary();
        let _ = audio_vmo.write(0, audio_info.as_bytes());
        let audio_handle = self.handle_table.insert(
            Arc::clone(&audio_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_AUDIO);
        self.register_file(String::from(PATH_AUDIO), audio_handle, audio_info.len(), true);
        self.devices.insert(
            FD_AUDIO,
            VfsDeviceNode::new_vmo(
                FD_AUDIO,
                String::from(PATH_AUDIO),
                audio_handle,
                audio_info.len(),
                true,
            ),
        );

        self.record_parent_dirs(PATH_DEV_NULL);
        self.register_file(String::from(PATH_DEV_NULL), kernel_objects::HandleId::null(), 0, true);

        // --- FASE v2.3: Zero-Syscall Shared Ring VMO (/dev/shm/io_ring & /dev/ipc/ring0) ---
        const RING_VMO_SIZE: usize = 64 * 1024;
        let ring_vmo = Vmo::new(RING_VMO_SIZE, VmoUsage::SharedRing)
            .expect("Fallo al reservar VMO Zero-Syscall para /dev/shm/io_ring");
        let initial_header = kernel_objects::VmoRingHeader::new();
        let _ = ring_vmo.write(0, &initial_header.to_bytes());
        ostd::ipc::register_shared_ring(ring_vmo.phys_addr(), ring_vmo.virt_addr());
        let ring_handle = self.handle_table.insert(
            Arc::clone(&ring_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        let fd_ring = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs("/dev/shm/io_ring");
        self.register_file(String::from("/dev/shm/io_ring"), ring_handle, RING_VMO_SIZE, true);
        self.devices.insert(
            fd_ring,
            VfsDeviceNode::new_vmo(
                fd_ring,
                String::from("/dev/shm/io_ring"),
                ring_handle,
                RING_VMO_SIZE,
                true,
            ),
        );
        let fd_ipc = self.next_fd;
        self.next_fd += 1;
        self.record_parent_dirs("/dev/ipc/ring0");
        self.register_file(String::from("/dev/ipc/ring0"), ring_handle, RING_VMO_SIZE, true);
        self.devices.insert(
            fd_ipc,
            VfsDeviceNode::new_vmo(
                fd_ipc,
                String::from("/dev/ipc/ring0"),
                ring_handle,
                RING_VMO_SIZE,
                true,
            ),
        );

        // Parse USTAR Initramfs from Limine ModuleRequest (Zero-Copy) or fallback in-memory USTAR image
        ostd::early_println!("[VFS] Montando Initramfs...");
        ostd::fb_println!("[VFS] Montando Initramfs...");
        ostd::fb_console::flush_console();
        let mut mounted_files = 0usize;
        if let Some(module_slice) = ostd::boot::initramfs_slice() {
            let tar_entries = TarFs::parse(module_slice);
            for entry in tar_entries {
                let file_vmo = Vmo::from_static_slice(entry.data, VmoUsage::InitramfsFile);
                let file_handle = self.handle_table.insert(
                    Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                    Rights::READ | Rights::MAP | Rights::TRANSFER,
                );
                let fd = self.next_fd;
                self.next_fd += 1;

                self.record_parent_dirs(&entry.path);
                self.register_file(entry.path.clone(), file_handle, entry.data.len(), false);
                self.devices.insert(
                    fd,
                    VfsDeviceNode::new_vmo(
                        fd,
                        entry.path.clone(),
                        file_handle,
                        entry.data.len(),
                        false,
                    ),
                );
                // Register lightweight VFS aliases without duplicating binaries in initramfs.tar
                let aliases: &[&str] = match entry.path.as_str() {
                    "/bin/desktop_ui.elf" => &["/bin/gui", "/bin/desktop"],
                    "/bin/fipsos_sh.elf" => &["/bin/sh"],
                    "/bin/pkg.elf" => &["/bin/pkg"],
                    "/bin/kilo.elf" => &["/bin/edit"],
                    "/bin/fipsos_httpd.elf" => &["/bin/httpd"],
                    _ => &[],
                };
                for &alias in aliases {
                    self.register_file(String::from(alias), file_handle, entry.data.len(), false);
                }
                mounted_files += 1;
            }
        } else {
            let fallback_tar = TarFs::build_fallback_tar();
            let tar_entries = TarFs::parse(fallback_tar.as_slice());
            for entry in tar_entries {
                let alloc_len = entry.data.len().max(1);
                let file_vmo = Vmo::new(alloc_len, VmoUsage::InitramfsFile)
                    .expect("Fallo al reservar VMO para archivo de Initramfs");
                if !entry.data.is_empty() {
                    let _ = file_vmo.write(0, entry.data);
                }
                let file_handle = self.handle_table.insert(
                    Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                    Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
                );
                let fd = self.next_fd;
                self.next_fd += 1;
                self.record_parent_dirs(&entry.path);
                self.register_file(entry.path.clone(), file_handle, entry.data.len(), false);
                self.devices.insert(
                    fd,
                    VfsDeviceNode::new_vmo(fd, entry.path, file_handle, entry.data.len(), false),
                );
                mounted_files += 1;
            }
        }
        let source_tag = if ostd::boot::initramfs_slice().is_some() {
            "Limine Module initramfs.tar (Zero-Copy)"
        } else {
            "TarFs In-Memory Fallback"
        };

        // Initialize or mount persistent files from SATA AHCI DiskFS BEFORE FAT32 LBA 2048 probe/init
        ostd::early_println!("[VFS] Montando DiskFS...");
        ostd::fb_println!("[VFS] Montando DiskFS...");
        ostd::fb_console::flush_console();
        let disk_files = crate::diskfs::init_or_mount();
        let mut mounted_disk = 0usize;
        for dfile in disk_files {
            if dfile.is_dir {
                self.record_parent_dirs(&dfile.path);
                self.directories.insert(dfile.path);
                mounted_disk += 1;
                continue;
            }
            let alloc_len = dfile.data.len().max(4096);
            if let Ok(file_vmo) = Vmo::new(alloc_len, VmoUsage::InitramfsFile) {
                if !dfile.data.is_empty() {
                    let _ = file_vmo.write(0, &dfile.data);
                }
                let file_handle = self.handle_table.insert(
                    Arc::clone(&file_vmo) as Arc<dyn KernelObject>,
                    Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
                );
                let fd = self.next_fd;
                self.next_fd += 1;
                self.record_parent_dirs(&dfile.path);
                self.register_file(dfile.path.clone(), file_handle, dfile.data.len(), false);
                self.devices.insert(
                    fd,
                    VfsDeviceNode::new_vmo(
                        fd,
                        dfile.path,
                        file_handle,
                        dfile.data.len(),
                        false,
                    ),
                );
                mounted_disk += 1;
            }
        }

        // Probe, auto-initialize (at LBA 2048 if safe), and mount FAT32 at /mnt/fat0
        ostd::early_println!("[VFS] Montando FAT32...");
        ostd::fb_println!("[VFS] Montando FAT32...");
        ostd::fb_console::flush_console();
        if crate::fat32::mount_or_init().is_some() {
            self.directories.insert(String::from("/mnt"));
            self.directories.insert(String::from("/mnt/fat0"));
            crate::fat32::format::seed_default_files();
        }

        // 1. Mount /proc and /sys nodes lazily (content generated on open/read via dynamic_sysfs_text)
        ostd::early_println!("[VFS] Montando /proc y /sys...");
        ostd::fb_println!("[VFS] Montando /proc y /sys...");
        ostd::fb_console::flush_console();

        let lazy_nodes: [(&str, bool, bool); 14] = [
            (PATH_SYS_HWMON, true, true),
            (PATH_BAT0_CAPACITY, false, true),
            (PATH_BAT0_STATUS, false, true),
            (PATH_BAT0_PROFILE, true, true),
            (PATH_BAT0_BOOST, false, true),
            (PATH_WLAN0_OPERSTATE, false, true),
            (PATH_PROC_MEMINFO, true, false),
            (PATH_PROC_MOUNTS, true, false),
            (PATH_PROC_SELF_MOUNTS, true, false),
            (PATH_ETC_MTAB, true, false),
            (PATH_PROC_UPTIME, true, false),
            (PATH_PROC_VERSION, false, false),
            (PATH_PROC_CPUINFO, true, false),
            (PATH_PROC_STAT, true, false),
        ];
        for (path, writable, register_dev) in lazy_nodes {
            let vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO lazy sysfs/procfs fail");
            let rights = if writable {
                Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER
            } else {
                Rights::READ | Rights::MAP | Rights::TRANSFER
            };
            let handle = self.handle_table.insert(Arc::clone(&vmo) as Arc<dyn KernelObject>, rights);
            self.record_parent_dirs(path);
            self.register_file(String::from(path), handle, 0, false);
            if register_dev {
                let fd = self.next_fd;
                self.next_fd += 1;
                self.devices.insert(
                    fd,
                    VfsDeviceNode::new_vmo(fd, String::from(path), handle, 0, false),
                );
            }
        }

        // /etc/resolv.conf and /etc/hosts for libc network resolution
        let resolv_text = "nameserver 10.0.2.3\n";
        let resolv_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /etc/resolv.conf fail");
        let _ = resolv_vmo.write(0, resolv_text.as_bytes());
        let resolv_handle = self.handle_table.insert(
            Arc::clone(&resolv_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs("/etc/resolv.conf");
        self.register_file(String::from("/etc/resolv.conf"), resolv_handle, resolv_text.len(), false);

        let hosts_text = "127.0.0.1 localhost\n10.0.2.2 host\n";
        let hosts_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO /etc/hosts fail");
        let _ = hosts_vmo.write(0, hosts_text.as_bytes());
        let hosts_handle = self.handle_table.insert(
            Arc::clone(&hosts_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::WRITE | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs("/etc/hosts");
        self.register_file(String::from("/etc/hosts"), hosts_handle, hosts_text.len(), false);

        // SysFS CPU Topology: /sys/devices/system/cpu/online, possible, present
        let n_cpus = ostd::scheduler::online_cpu_count().max(1);
        let online_str = if n_cpus <= 1 {
            String::from("0\n")
        } else {
            alloc::format!("0-{}\n", n_cpus - 1)
        };
        let online_vmo = Vmo::new(4096, VmoUsage::InitramfsFile).expect("VMO sys cpu online fail");
        let _ = online_vmo.write(0, online_str.as_bytes());
        let online_handle = self.handle_table.insert(
            Arc::clone(&online_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );
        self.record_parent_dirs(PATH_SYS_CPU_ONLINE);
        self.register_file(String::from(PATH_SYS_CPU_ONLINE), online_handle, online_str.len(), false);
        self.record_parent_dirs(PATH_SYS_CPU_POSSIBLE);
        self.register_file(String::from(PATH_SYS_CPU_POSSIBLE), online_handle, online_str.len(), false);
        self.record_parent_dirs(PATH_SYS_CPU_PRESENT);
        self.register_file(String::from(PATH_SYS_CPU_PRESENT), online_handle, online_str.len(), false);

        // 3. Mount /bin/fault_test.elf
        let fault_elf = build_fault_test_elf();
        let fault_vmo = Vmo::new(4096, VmoUsage::InitramfsFile)
            .expect("Fallo al reservar VMO para /bin/fault_test.elf");
        let _ = fault_vmo.write(0, &fault_elf);
        let fault_handle = self.handle_table.insert(
            Arc::clone(&fault_vmo) as Arc<dyn KernelObject>,
            Rights::READ | Rights::MAP | Rights::TRANSFER,
        );
        let fd_fault = self.next_fd;
        self.next_fd += 1;
        reserve_unified_fd(self.next_fd);
        self.record_parent_dirs(PATH_FAULT_TEST);
        self.register_file(String::from(PATH_FAULT_TEST), fault_handle, fault_elf.len(), false);
        self.devices.insert(
            fd_fault,
            VfsDeviceNode::new_vmo(
                fd_fault,
                String::from(PATH_FAULT_TEST),
                fault_handle,
                fault_elf.len(),
                false,
            ),
        );

        // 4. Seed zero-copy container base images (/containers/base-rootfs & ai-rootfs)
        crate::container::seed_container_images(self);

        ostd::early_println!(
            "[linux_abi::vfs] VFS montado: 6 dispositivos (/dev/*: drm, accel, fb0, mouse0, eth0, audio) + {} archivos TarFs ({}) + {} archivos DiskFS (SATA AHCI) + /sys/fipsos/hwmon",
            mounted_files,
            source_tag,
            mounted_disk
        );
        ostd::fb_println!(
            "[linux_abi::vfs] VFS montado: 6 nodos /dev/* + {} TarFs + {} DiskFS + /sys/fipsos/hwmon",
            mounted_files,
            mounted_disk
        );
    }

}
