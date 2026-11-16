#![forbid(unsafe_code)]

//! Container Runtime IOCTL Control & Zero-Copy Base Image Seeding (`linux_abi::container_ctl`).

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use kernel_objects::{KernelObject, Rights, Vmo, VmoUsage};
use ostd::arch::syscall::{read_user_bytes, write_user_bytes};

use crate::container::{
    live_container_usage, ContainerSlot, CONTAINERS, CTR_STATUS_CREATED, CTR_STATUS_EXITED,
    CTR_STATUS_RUNNING,
};
use crate::syscall::GLOBAL_VFS;
use crate::vfs::{normalize_path, set_process_container_id, CapabilityVfs, FIPSOS_IOCTL_CONTAINER_CTL};

pub const CTR_OP_CREATE: u32 = 1;
pub const CTR_OP_ATTACH_SELF: u32 = 2;
pub const CTR_OP_FINISH: u32 = 3;
pub const CTR_OP_LIST: u32 = 4;
pub const CTR_OP_INSPECT: u32 = 5;
pub const CTR_OP_REMOVE: u32 = 6;

const CTL_REQ_BYTES: usize = 184;
const CTR_INFO_BYTES: usize = 208;

fn read_cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from(core::str::from_utf8(&buf[..end]).unwrap_or("").trim())
}

fn write_fixed(dst: &mut [u8], s: &str) {
    dst.fill(0);
    let b = s.as_bytes();
    let n = b.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&b[..n]);
}

fn encode_info(c: &ContainerSlot) -> [u8; CTR_INFO_BYTES] {
    let mut out = [0u8; CTR_INFO_BYTES];
    out[0..4].copy_from_slice(&c.id.to_le_bytes());
    out[4..8].copy_from_slice(&c.status.to_le_bytes());
    out[8..12].copy_from_slice(&c.pid_count.to_le_bytes());
    out[12..16].copy_from_slice(&c.last_exit_code.to_le_bytes());
    out[16..24].copy_from_slice(&c.init_pid.to_le_bytes());
    out[24..32].copy_from_slice(&c.mem_limit_pages.to_le_bytes());
    out[32..40].copy_from_slice(&c.used_pages.to_le_bytes());
    out[40..48].copy_from_slice(&c.peak_pages.to_le_bytes());
    write_fixed(&mut out[48..80], &c.name);
    write_fixed(&mut out[80..112], &c.image);
    write_fixed(&mut out[112..176], &c.root_path);
    write_fixed(&mut out[176..208], &c.hostname);
    out
}

/// Dispatches `FIPSOS_IOCTL_CONTAINER_CTL` (`0xC010_5307`) for `/bin/docker`.
pub fn dispatch_container_ioctl(arg: u64) -> i64 {
    let mut req = [0u8; CTL_REQ_BYTES];
    if arg == 0 || read_user_bytes(arg, &mut req).is_err() {
        return -14; // -EFAULT
    }
    let op = u32::from_le_bytes(req[0..4].try_into().unwrap());
    let id = u32::from_le_bytes(req[4..8].try_into().unwrap());
    let mem_mb = u32::from_le_bytes(req[8..12].try_into().unwrap());
    let exit_code = i32::from_le_bytes(req[12..16].try_into().unwrap());
    let data_ptr = u64::from_le_bytes(req[16..24].try_into().unwrap());
    let name = read_cstr(&req[24..56]);
    let image = read_cstr(&req[56..88]);
    let root_path = normalize_path(&read_cstr(&req[88..152]));
    let hostname = read_cstr(&req[152..184]);

    match op {
        CTR_OP_CREATE => {
            if !GLOBAL_VFS.lock().is_directory(&root_path) {
                return -2;
            }
            let mut reg = CONTAINERS.lock();
            let Some(pos) = reg.slots.iter().position(|c| !c.active) else {
                return -12;
            };
            let cid = reg.next_id;
            reg.next_id = cid.saturating_add(1);
            reg.slots[pos] = ContainerSlot {
                active: true,
                id: cid,
                status: CTR_STATUS_CREATED,
                pid_count: 0,
                last_exit_code: 0,
                init_pid: 0,
                mem_limit_pages: (mem_mb as u64) * 256,
                used_pages: 0,
                peak_pages: 0,
                name: if name.is_empty() { format!("ctr-{}", cid) } else { name },
                image: if image.is_empty() { String::from("fipsos/base:6.3") } else { image },
                root_path,
                hostname: if hostname.is_empty() { format!("ctr-{}", cid) } else { hostname },
            };
            cid as i64
        }
        CTR_OP_ATTACH_SELF => {
            let slot = ostd::scheduler::active_slot();
            let pid = ostd::scheduler::active_pid();
            let mut reg = CONTAINERS.lock();
            let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == id) else {
                return -2;
            };
            c.status = CTR_STATUS_RUNNING;
            c.init_pid = pid;
            c.pid_count = 1;
            drop(reg);
            set_process_container_id(slot, id);
            0
        }
        CTR_OP_FINISH => {
            let (used, _) = live_container_usage(id);
            let mut reg = CONTAINERS.lock();
            if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == id) {
                c.status = CTR_STATUS_EXITED;
                c.pid_count = 0;
                c.last_exit_code = exit_code;
                c.peak_pages = c.peak_pages.max(used);
                c.used_pages = 0;
                0
            } else {
                -2
            }
        }
        CTR_OP_LIST => {
            if data_ptr == 0 {
                return -14;
            }
            let items: alloc::vec::Vec<[u8; CTR_INFO_BYTES]> = {
                let reg = CONTAINERS.lock();
                reg.slots.iter().filter(|c| c.active).map(encode_info).collect()
            };
            for (i, raw) in items.iter().enumerate() {
                if write_user_bytes(data_ptr + (i * CTR_INFO_BYTES) as u64, raw).is_err() {
                    return -14;
                }
            }
            items.len() as i64
        }
        CTR_OP_INSPECT => {
            if data_ptr == 0 {
                return -14;
            }
            let raw = {
                let reg = CONTAINERS.lock();
                reg.slots
                    .iter()
                    .find(|c| c.active && (c.id == id || (!name.is_empty() && c.name == name)))
                    .map(encode_info)
            };
            let Some(info) = raw else { return -2 };
            write_user_bytes(data_ptr, &info).map(|_| 0).unwrap_or(-14)
        }
        CTR_OP_REMOVE => {
            let mut reg = CONTAINERS.lock();
            let Some(c) = reg
                .slots
                .iter_mut()
                .find(|c| c.active && (c.id == id || (!name.is_empty() && c.name == name)))
            else {
                return -2;
            };
            if c.status == CTR_STATUS_RUNNING && c.pid_count > 0 {
                return -16; // -EBUSY
            }
            *c = ContainerSlot::empty();
            0
        }
        _ => -22,
    }
}

fn share_vfs_file(vfs: &mut CapabilityVfs, src: &str, dst: &str) {
    if let Some(meta) = vfs.registered_files.get(src).cloned() {
        vfs.register_file(String::from(dst), meta.handle, meta.file_size, meta.is_device);
    }
}

/// Seeds `/containers/base-rootfs` (`fipsos/base:6.3`) and `/containers/ai-rootfs` (`fipsos/ai-runtime:6.3`)
/// using Zero-Copy VMO capability sharing for existing system binaries.
pub fn seed_container_images(vfs: &mut CapabilityVfs) {
    let _ = FIPSOS_IOCTL_CONTAINER_CTL;
    vfs.add_directory("/containers");
    for &(root, is_ai) in &[("/containers/base-rootfs", false), ("/containers/ai-rootfs", true)] {
        for sub in ["", "/bin", "/etc", "/dev"] {
            vfs.add_directory(&format!("{}{}", root, sub));
        }
        let release_text = if is_ai {
            "NAME=\"FipSOs AI Container Runtime\"\nVERSION=\"6.3\"\nIMAGE=\"fipsos/ai-runtime:6.3\"\n"
        } else {
            "NAME=\"FipSOs Base Container\"\nVERSION=\"6.3\"\nIMAGE=\"fipsos/base:6.3\"\n"
        };
        if let Ok(vmo) = Vmo::new(4096, VmoUsage::InitramfsFile) {
            let _ = vmo.write(0, release_text.as_bytes());
            let h = vfs.register_raw_vmo_handle(
                Arc::clone(&vmo) as Arc<dyn KernelObject>,
                Rights::READ | Rights::MAP | Rights::TRANSFER,
            );
            vfs.register_file(format!("{}/etc/os-release", root), h, release_text.len(), false);
        }
        vfs.register_file(format!("{}/dev/null", root), kernel_objects::HandleId::null(), 0, true);
        for applet in ["busybox", "uname", "ls", "cat", "echo", "pwd", "sh"] {
            share_vfs_file(vfs, "/bin/busybox", &format!("{}/bin/{}", root, applet));
        }
        for bin in ["/bin/fipsos_sh.elf", "/bin/lua", "/bin/fips_ai", "/bin/c_std_app.elf", "/bin/sample_tool.elf"] {
            if let Some(fname) = bin.rsplit('/').next() {
                share_vfs_file(vfs, bin, &format!("{}/bin/{}", root, fname));
            }
        }
        if is_ai {
            vfs.add_directory(&format!("{}/models", root));
            share_vfs_file(vfs, "/bin/npu_infer.elf", &format!("{}/bin/npu_infer.elf", root));
            share_vfs_file(vfs, "/bin/benchmark_simd.elf", &format!("{}/bin/benchmark_simd.elf", root));
            share_vfs_file(vfs, "/models/avatar_v1.bin", &format!("{}/models/avatar_v1.bin", root));
        }
    }
}
