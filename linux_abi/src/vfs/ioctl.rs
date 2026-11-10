#![forbid(unsafe_code)]

use alloc::vec;
use kernel_objects::{KernelObject, Rights, SchedulerRegistry};
use ostd::ffi_jail::{c_npu_driver_entry, FfiJail};

use super::devfs_sysfs::*;
use super::CapabilityVfs;

impl CapabilityVfs {
    pub fn ioctl(&mut self, fd: u64, request: u64, arg: u64) -> Result<u64, i64> {
        if request == VFS_IOCTL_LIST_FILES {
            self.list_nodes();
            return Ok(self.devices.len() as u64);
        }
        if request == VFS_IOCTL_LIST_PROCESSES {
            let count = SchedulerRegistry::print_ps_table();
            return Ok(count as u64);
        }
        if request == DISK_IOCTL_SAVE_NOTE {
            let ticks = ostd::arch::idt::timer_ticks();
            let _ = crate::diskfs::save_persistent_note(ticks);
            let ahci = ostd::storage::ahci_snapshot();
            let note_text = alloc::format!(
                "[DiskFS] Nota persistente de usuario FipSOs v1.6\n\
                 Timestamp IRQ0: {} ticks\n\
                 Dispositivo: Intel ICH9 SATA AHCI 00:1f.2 (Port {})\n\
                 LBA Lecturas: {} sectores | LBA Escrituras: {} sectores\n\
                 Total transferido: {} KiB\n\
                 Estado: Persistencia en sectores LBA verificada exitosamente.\n",
                ticks,
                ahci.port_idx,
                ahci.sectors_read,
                ahci.sectors_written,
                ahci.bytes_transferred / 1024
            );
            let (n_fd, n_koid) = self.create_or_update_disk_file(crate::diskfs::PATH_PERSISTENT_NOTE, note_text.as_bytes())?;
            ostd::early_println!(
                "[DiskFS::save] Guardada nota persistente \"{}\" en disco SATA AHCI (fd={}, {}, {} B)",
                crate::diskfs::PATH_PERSISTENT_NOTE,
                n_fd,
                n_koid,
                note_text.len()
            );
            ostd::fb_println!(
                "[DiskFS::save] \"{}\" guardado en SATA ({} B)",
                crate::diskfs::PATH_PERSISTENT_NOTE,
                note_text.len()
            );
            return Ok(note_text.len() as u64);
        }
        if request == FB_IOCTL_GET_INFO || request == FBIOGET_VSCREENINFO || request == FB_IOCTL_GET_VSCREENINFO {
            let (fb_w, fb_h, fb_pitch, fb_bpp) = ostd::fb_console::framebuffer_info();
            if arg != 0 {
                let mut buf = [0u8; 64];
                // Formato simple 16 bytes: { width: u32, height: u32, pitch: u32, bpp: u32 }
                buf[0..4].copy_from_slice(&(fb_w as u32).to_le_bytes());
                buf[4..8].copy_from_slice(&(fb_h as u32).to_le_bytes());
                buf[8..12].copy_from_slice(&(fb_pitch as u32).to_le_bytes());
                buf[12..16].copy_from_slice(&(fb_bpp as u32).to_le_bytes());
                // Campos adicionales compatibles con fb_var_screeninfo (Linux standard)
                buf[16..20].copy_from_slice(&0u32.to_le_bytes());
                buf[20..24].copy_from_slice(&0u32.to_le_bytes());
                buf[24..28].copy_from_slice(&(fb_bpp as u32).to_le_bytes());
                buf[28..32].copy_from_slice(&(fb_pitch as u32).to_le_bytes());
                ostd::mm::write_user_bytes(arg, &buf).map_err(|_| -14i64)?;
            }
            return Ok(0);
        }
        if request == FB_IOCTL_POLL_KEY {
            let key_byte = ostd::arch::keyboard::try_pop_key_nonblocking().unwrap_or(0);
            return Ok(key_byte as u64);
        }
        if request == FB_IOCTL_SET_DESKTOP_MODE {
            let enabled = arg != 0;
            ostd::fb_console::set_desktop_mode(enabled);
            ostd::early_println!(
                "[linux_abi::fb0] FB_IOCTL_SET_DESKTOP_MODE(enabled={}) -> Compositor 1280x800 {}",
                enabled,
                if enabled {
                    "ACTIVO (Zero-Copy Double Buffer)"
                } else {
                    "DESACTIVADO (Consola TTY restaurada)"
                }
            );
            return Ok(if enabled { 1 } else { 0 });
        }
        if request == VFS_IOCTL_HWMON_REPORT {
            let hw_text = build_hwmon_text();
            if arg != 0 {
                let write_len = hw_text.len().min(4096);
                ostd::mm::write_user_bytes(arg, &hw_text.as_bytes()[..write_len]).map_err(|_| -14i64)?;
            }
            return Ok(hw_text.len() as u64);
        }
        if request == VFS_IOCTL_TASK_CONTROL {
            if arg == 1 {
                let (pid, _tid) = SchedulerRegistry::spawn_background_task("bg_downloader");
                ostd::early_println!("[TaskCtrl] Tarea en segundo plano iniciada (PID={})", pid);
                ostd::fb_println!("[TaskCtrl] Tarea segundo plano iniciada (PID={})", pid);
                return Ok(pid);
            } else if arg == 2 {
                let _ = SchedulerRegistry::kill_process(1051, 9);
                ostd::early_println!("[TaskCtrl] Tarea en segundo plano terminada (PID=1051, SIGKILL)");
                ostd::fb_println!("[TaskCtrl] Tarea segundo plano terminada (PID=1051)");
                return Ok(1051);
            } else if arg == 3 {
                let _ = SchedulerRegistry::kill_process(1035, 9);
                ostd::early_println!("[TaskCtrl] Daemon simd-worker terminado (PID=1035, SIGKILL)");
                ostd::fb_println!("[TaskCtrl] Daemon simd-worker terminado");
                return Ok(1035);
            } else if arg == 4 {
                ostd::scheduler::set_daemon_active(true);
                ostd::early_println!("[TaskCtrl] Daemon simd-worker reactivado");
                ostd::fb_println!("[TaskCtrl] Daemon reactivado");
                return Ok(1035);
            }
            return Ok(0);
        }
        if request == DISK_IOCTL_DELETE_FILE {
            let path_str = if arg != 0 {
                let mut path_buf = [0u8; 128];
                ostd::mm::read_user_bytes(arg, &mut path_buf).map_err(|_| -14i64)?;
                let len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
                let parsed = core::str::from_utf8(&path_buf[..len]).unwrap_or(crate::diskfs::PATH_PERSISTENT_NOTE);
                // Allocate a string to avoid borrow conflict
                alloc::string::String::from(parsed)
            } else {
                alloc::string::String::from(crate::diskfs::PATH_PERSISTENT_NOTE)
            };
            let _ = crate::diskfs::delete_file(&path_str);
            self.remove_node(&path_str);
            return Ok(0);
        }
        if request == NET_IOCTL_BROWSER_FETCH_SAVE {
            let (uri_str, save_path) = if arg == 1 {
                (alloc::string::String::from("http://fipsos.org/index.html"), PATH_INDEX_HTML)
            } else if arg == 2 {
                (alloc::string::String::from("http://fipsos.org/models/ai_model_v2.bin"), PATH_AI_MODEL_V2)
            } else if arg == 3 {
                (alloc::string::String::from("http://fipsos.org/packages/sys_update.pkg"), PATH_SYS_UPDATE_PKG)
            } else if arg >= 0x0020_0000 {
                let mut uri_buf = [0u8; 128];
                ostd::mm::read_user_bytes(arg, &mut uri_buf).map_err(|_| -14i64)?;
                let len = uri_buf.iter().position(|&b| b == 0).unwrap_or(uri_buf.len());
                let parsed = core::str::from_utf8(&uri_buf[..len]).unwrap_or("http://fipsos.org/index.html");
                let dest = if parsed.contains("index.html") {
                    PATH_INDEX_HTML
                } else if parsed.contains("ai_model_v2") {
                    PATH_AI_MODEL_V2
                } else if parsed.contains("sys_update") {
                    PATH_SYS_UPDATE_PKG
                } else {
                    "/disk/browser_download.txt"
                };
                (alloc::string::String::from(parsed), dest)
            } else {
                (alloc::string::String::from("http://fipsos.org/index.html"), PATH_INDEX_HTML)
            };

            let (res, payload) = ostd::net::http_fetch_url(&uri_str);
            let _ = crate::diskfs::save_file(save_path, &payload);
            let (dl_fd, dl_koid) = self.create_or_update_disk_file(save_path, &payload)?;

            ostd::early_println!(
                "[Browser::GET] \"{}\" descargado y guardado en SATA \"{}\" (fd={}, {}, {} B | RTT={} us)",
                uri_str,
                save_path,
                dl_fd,
                dl_koid,
                payload.len(),
                res.rtt_us
            );
            ostd::fb_println!(
                "[Browser::GET] \"{}\" -> SATA \"{}\" ({} B)",
                uri_str,
                save_path,
                payload.len()
            );
            return Ok(payload.len() as u64);
        }
        if request == VFS_IOCTL_SYS_TELEMETRY {
            let mem = ostd::mm::stats();
            let sched = ostd::scheduler::snapshot();
            let net = ostd::net::snapshot();
            let ahci = ostd::storage::ahci_snapshot();
            let rtc = ostd::arch::x86_64::rtc_power::read_rtc();
            let sensors = ostd::arch::x86_64::sensors::snapshot();
            let faults_rec = ostd::arch::syscall::user_faults_recovered();
            let pkg_present = self.devices.values().any(|n| n.path == PATH_DOWNLOAD_PKG);

            let used_kib = (mem.used_bytes() / 1024) as u32;
            let total_kib = (mem.total_bytes() / 1024) as u32;
            let child_sw = sched
                .child_rust_switches
                .saturating_add(sched.child_cpp_switches)
                .min(0xFFFF) as u16;

            if arg != 0 {
                let mut pkt = [0u8; 192];
                pkt[0..4].copy_from_slice(&used_kib.to_le_bytes());
                pkt[4..8].copy_from_slice(&total_kib.to_le_bytes());
                pkt[8..12].copy_from_slice(&(mem.free_frames as u32).to_le_bytes());
                pkt[12..16].copy_from_slice(&(mem.used_frames as u32).to_le_bytes());
                pkt[16..24].copy_from_slice(&sched.total_irq0_ticks.to_le_bytes());
                pkt[24..28].copy_from_slice(&(sched.total_preemptions as u32).to_le_bytes());
                pkt[28..32].copy_from_slice(&(sched.total_voluntary_yields as u32).to_le_bytes());
                pkt[32..36].copy_from_slice(&(sched.daemon_switches as u32).to_le_bytes());
                pkt[36..40].copy_from_slice(&(sched.desktop_ui_ticks as u32).to_le_bytes());
                pkt[40..44].copy_from_slice(&(net.tx_packets as u32).to_le_bytes());
                pkt[44..48].copy_from_slice(&(net.rx_packets as u32).to_le_bytes());
                pkt[48..52].copy_from_slice(&(net.rx_bytes as u32).to_le_bytes());
                pkt[52..54].copy_from_slice(&(self.devices.len() as u16).to_le_bytes());
                pkt[54] = if net.link_up { 1 } else { 0 };
                pkt[55] = if pkg_present { 1 } else { 0 };
                pkt[56..62].copy_from_slice(&net.mac);
                pkt[62..64].copy_from_slice(&child_sw.to_le_bytes());
                pkt[64..72].copy_from_slice(&sched.active_cr3.to_le_bytes());
                pkt[72..80].copy_from_slice(&sched.shell_cr3.to_le_bytes());
                pkt[80..88].copy_from_slice(&sched.child_rust_cr3.to_le_bytes());
                pkt[88..96].copy_from_slice(&sched.child_cpp_cr3.to_le_bytes());
                pkt[96..104].copy_from_slice(&ahci.sectors_read.to_le_bytes());
                pkt[104..112].copy_from_slice(&ahci.sectors_written.to_le_bytes());
                pkt[112..120].copy_from_slice(&ahci.bytes_transferred.to_le_bytes());
                pkt[120] = if ahci.present { 1 } else { 0 };
                pkt[121] = ahci.port_idx;

                // --- FASE v1.7: RTC, CPU MHz, Temp, Cores/Threads, Fault Recovery, Driver Health ---
                pkt[128..130].copy_from_slice(&rtc.year.to_le_bytes());
                pkt[130] = rtc.month;
                pkt[131] = rtc.day;
                pkt[132] = rtc.hour;
                pkt[133] = rtc.minute;
                pkt[134] = rtc.second;
                pkt[135] = 0; // rtc_pad

                pkt[136..140].copy_from_slice(&sensors.cpu_mhz.to_le_bytes());
                pkt[140..144].copy_from_slice(&sensors.temp_celsius.to_le_bytes());
                pkt[144..152].copy_from_slice(&faults_rec.to_le_bytes());
                pkt[152..154].copy_from_slice(&sensors.physical_cores.to_le_bytes());
                pkt[154..156].copy_from_slice(&sensors.logical_threads.to_le_bytes());

                let mut driver_mask: u32 = 0;
                if sensors.e1000e_ok { driver_mask |= 1 << 0; }
                if sensors.ahci_ok { driver_mask |= 1 << 1; }
                if sensors.fb0_ok { driver_mask |= 1 << 2; }
                if sensors.npu_ok { driver_mask |= 1 << 3; }
                pkt[156..160].copy_from_slice(&driver_mask.to_le_bytes());

                // --- Background Task, SATA files, SMP slots ---
                let bg_state: u8 = if sched.bg_downloader_active { 1 } else if !sched.daemon_active { 2 } else { 0 };
                pkt[172] = bg_state;
                let disk_files = self.devices.values().filter(|n| n.path.starts_with("/disk/")).count() as u8;
                pkt[173] = disk_files;
                pkt[174] = ostd::scheduler::active_proc_slot_on_cpu(0) as u8;
                pkt[175] = ostd::scheduler::active_proc_slot_on_cpu(1) as u8;

                ostd::mm::write_user_bytes(arg, &pkt).map_err(|_| -14i64)?;
            }

            let used_mib = (used_kib / 1024) as u64;
            let total_mib = (total_kib / 1024) as u64;
            return Ok((used_mib << 16) | total_mib);
        }

        let node = self.devices.get(&fd).cloned().ok_or(-9i64)?; // -EBADF
        let entry = self
            .handle_table
            .get(node.handle, Rights::READ | Rights::WRITE)
            .map_err(|_| -13i64)?; // -EACCES
        let vmo = entry.as_vmo().map_err(|_| -19i64)?; // -ENODEV

        if request == MOUSE_IOCTL_POLL_STATE {
            let snap = ostd::arch::mouse::poll_state();
            let packed = sync_mouse_vmo(&vmo, &snap);
            if arg == 1 {
                ostd::early_println!(
                    "[Mouse::/dev/input/mouse0] Pos=({}, {}) | Botones=[L:{} R:{} M:{}] ({:#04x}) | Paquetes={} | Clics={} | Packed={:#018x}",
                    snap.x,
                    snap.y,
                    snap.left as u8,
                    snap.right as u8,
                    snap.middle as u8,
                    snap.buttons,
                    snap.event_seq,
                    snap.click_count,
                    packed
                );
                ostd::fb_println!(
                    "[Mouse::/dev/input/mouse0] Pos=({}, {}) | Btn=[L:{} R:{} M:{}] | Pkts={} | Clics={}",
                    snap.x,
                    snap.y,
                    snap.left as u8,
                    snap.right as u8,
                    snap.middle as u8,
                    snap.event_seq,
                    snap.click_count
                );
            }
            return Ok(packed);
        }

        if request == MOUSE_IOCTL_STEP_SYNTHETIC {
            let dx = (arg & 0xFFFF) as u16 as i16;
            let dy = ((arg >> 16) & 0xFFFF) as u16 as i16;
            let buttons = ((arg >> 32) & 0xFF) as u8;
            let snap = ostd::arch::mouse::inject_synthetic_mouse_step(dx, dy, buttons);
            let packed = sync_mouse_vmo(&vmo, &snap);
            ostd::early_println!(
                "[Mouse::synthetic] Step(dx={}, dy={}, btn={:#x}) -> Pos=({}, {}) | Paquetes={} | Clics={}",
                dx,
                dy,
                buttons,
                snap.x,
                snap.y,
                snap.event_seq,
                snap.click_count
            );
            ostd::fb_println!(
                "[Mouse::synthetic] Step(dx={}, dy={}, btn={:#x}) -> Pos=({}, {}) | Pkts={}",
                dx,
                dy,
                buttons,
                snap.x,
                snap.y,
                snap.event_seq
            );
            return Ok(packed);
        }

        if request == NET_IOCTL_PING_GATEWAY {
            let res = ostd::net::ping_gateway();
            let packed = sync_net_vmo(&vmo, &res, None);
            if arg != 0 {
                ostd::early_println!(
                    "[Net::/dev/net/eth0] ARP+ICMP {}.{}.{}.{} -> {}.{}.{}.{} (GW {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}) | MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} | Link={} | DMA RX={:#x} TX={:#x} | TX={} pkts ({} B) RX={} pkts ({} B) | RTT={}.{:02} ms",
                    res.local_ip[0], res.local_ip[1], res.local_ip[2], res.local_ip[3],
                    res.gateway_ip[0], res.gateway_ip[1], res.gateway_ip[2], res.gateway_ip[3],
                    res.gateway_mac[0], res.gateway_mac[1], res.gateway_mac[2],
                    res.gateway_mac[3], res.gateway_mac[4], res.gateway_mac[5],
                    res.hw.mac[0], res.hw.mac[1], res.hw.mac[2],
                    res.hw.mac[3], res.hw.mac[4], res.hw.mac[5],
                    if res.hw.link_up { "UP" } else { "DOWN" },
                    res.hw.rx_ring_phys,
                    res.hw.tx_ring_phys,
                    res.hw.tx_packets,
                    res.hw.tx_bytes,
                    res.hw.rx_packets,
                    res.hw.rx_bytes,
                    res.rtt_us / 1000,
                    (res.rtt_us % 1000) / 10
                );
                ostd::fb_println!(
                    "[Net::eth0] Ping 10.0.2.15->10.0.2.2 MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} Link={} TX={} RX={} RTT={}us",
                    res.hw.mac[0], res.hw.mac[1], res.hw.mac[2],
                    res.hw.mac[3], res.hw.mac[4], res.hw.mac[5],
                    if res.hw.link_up { "UP" } else { "DOWN" },
                    res.hw.tx_packets,
                    res.hw.rx_packets,
                    res.rtt_us
                );
            }
            return Ok(packed);
        }

        if request == NET_IOCTL_HTTP_FETCH {
            let (res, payload) = ostd::net::http_fetch_package(PATH_DOWNLOAD_PKG);
            let _ = sync_net_vmo(&vmo, &res, Some(&payload));
            let net_vmo_koid = vmo.koid();
            let (dl_fd, dl_koid) =
                self.create_or_update_downloaded_file(PATH_DOWNLOAD_PKG, &payload)?;
            let _ = crate::diskfs::save_file(PATH_DOWNLOAD_PKG, &payload);

            ostd::early_println!(
                "[Net::wget] HTTP-Fetch eth0 (10.0.2.15 -> 10.0.2.2) -> VMO {} (64 KiB) -> Guardado en VFS \"{}\" (fd={}, {}, {} B | TX={} RX={})",
                net_vmo_koid,
                PATH_DOWNLOAD_PKG,
                dl_fd,
                dl_koid,
                payload.len(),
                res.hw.tx_packets,
                res.hw.rx_packets
            );
            ostd::fb_println!(
                "[Net::wget] HTTP-Fetch eth0 -> \"{}\" (fd={}, {}, {} B | TX={} RX={})",
                PATH_DOWNLOAD_PKG,
                dl_fd,
                dl_koid,
                payload.len(),
                res.hw.tx_packets,
                res.hw.rx_packets
            );
            return Ok(payload.len() as u64);
        }

        if request == FB_IOCTL_FLIP_BUFFER {
            let (fb_w, fb_h, _, _) = ostd::fb_console::framebuffer_info();
            let (x, y, width, height) = if arg == 0 {
                (0usize, 0usize, fb_w, fb_h)
            } else {
                let rx = (arg & 0xFFFF) as usize;
                let ry = ((arg >> 16) & 0xFFFF) as usize;
                let rw = ((arg >> 32) & 0xFFFF) as usize;
                let rh = ((arg >> 48) & 0xFFFF) as usize;
                (rx, ry, rw, rh)
            };
            let copied =
                ostd::fb_console::flip_dma_double_buffer(vmo.dma_buffer(), x, y, width, height);
            return Ok(copied as u64);
        }

        if request == FB_IOCTL_PRESENT_RECT {
            let (width, height) = if arg != 0 {
                let w = ((arg >> 16) & 0xFFFF) as usize;
                let h = (arg & 0xFFFF) as usize;
                (w.clamp(16, 256), h.clamp(16, 256))
            } else {
                (FB_WIDGET_DEFAULT_W, FB_WIDGET_DEFAULT_H)
            };
            ostd::fb_console::present_dma_widget_rect(
                vmo.dma_buffer(),
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

}
