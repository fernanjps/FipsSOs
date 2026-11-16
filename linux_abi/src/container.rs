#![forbid(unsafe_code)]

//! Kernel Container Subsystem (`linux_abi::container`).
//! Mount (`sys_chroot`), UTS (`sys_sethostname`), PID tracking & Memory Cgroup enforcement.

use alloc::format;
use alloc::string::String;
use ostd::arch::syscall::read_user_bytes;
use ostd::sync::SpinLock;

use crate::syscall::GLOBAL_VFS;
use crate::vfs::{
    collect_container_slots, get_process_container_id, get_process_root, normalize_path,
    set_process_cwd, set_process_hostname, set_process_root,
};

pub use crate::container_ctl::{dispatch_container_ioctl, seed_container_images};

pub const SYS_CHROOT: u64 = 161;
pub const SYS_SETHOSTNAME: u64 = 170;
pub const MAX_CONTAINERS: usize = 8;

pub const CTR_STATUS_CREATED: u32 = 0;
pub const CTR_STATUS_RUNNING: u32 = 1;
pub const CTR_STATUS_EXITED: u32 = 2;

#[derive(Clone)]
pub(crate) struct ContainerSlot {
    pub active: bool,
    pub id: u32,
    pub status: u32,
    pub pid_count: u32,
    pub last_exit_code: i32,
    pub init_pid: u64,
    pub mem_limit_pages: u64,
    pub used_pages: u64,
    pub peak_pages: u64,
    pub name: String,
    pub image: String,
    pub root_path: String,
    pub hostname: String,
}

impl ContainerSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            active: false,
            id: 0,
            status: CTR_STATUS_CREATED,
            pid_count: 0,
            last_exit_code: 0,
            init_pid: 0,
            mem_limit_pages: 0,
            used_pages: 0,
            peak_pages: 0,
            name: String::new(),
            image: String::new(),
            root_path: String::new(),
            hostname: String::new(),
        }
    }
}

pub(crate) struct ContainerRegistry {
    pub slots: [ContainerSlot; MAX_CONTAINERS],
    pub next_id: u32,
}

impl ContainerRegistry {
    const fn new() -> Self {
        Self {
            slots: [const { ContainerSlot::empty() }; MAX_CONTAINERS],
            next_id: 1,
        }
    }
}

pub(crate) static CONTAINERS: SpinLock<ContainerRegistry> = SpinLock::new(ContainerRegistry::new());

/// Converts a physical kernel VFS `cwd` into a virtual path relative to `root_dir`.
pub fn virtual_cwd(root_dir: &str, cwd_phys: &str) -> String {
    if root_dir.is_empty() || root_dir == "/" {
        return normalize_path(cwd_phys);
    }
    if let Some(rest) = cwd_phys.strip_prefix(root_dir) {
        if rest.is_empty() { String::from("/") } else { normalize_path(rest) }
    } else {
        String::from("/")
    }
}

fn join_confined(root_dir: &str, virt: &str) -> String {
    if root_dir.is_empty() || root_dir == "/" {
        String::from(virt)
    } else if virt == "/" {
        String::from(root_dir)
    } else {
        format!("{}{}", root_dir, virt)
    }
}

/// Resolves and strictly confines `user_path` inside `root_dir` (`..` cannot escape).
pub fn resolve_for_process(root_dir: &str, cwd_phys: &str, user_path: &str) -> String {
    let p = user_path.trim();
    let virt = if p.starts_with('/') {
        normalize_path(p)
    } else {
        let vcwd = virtual_cwd(root_dir, cwd_phys);
        let full = if vcwd == "/" { format!("/{}", p) } else { format!("{}/{}", vcwd, p) };
        normalize_path(&full)
    };
    join_confined(root_dir, &virt)
}

/// Resolves a `dirfd`-relative path while enforcing the active process's `root_dir` jail.
pub fn resolve_dirfd_confined(base_phys: &str, raw_path: &str) -> String {
    let root_dir = get_process_root(ostd::scheduler::active_slot());
    let p = raw_path.trim();
    let vbase = virtual_cwd(&root_dir, base_phys);
    let full = if vbase == "/" { format!("/{}", p) } else { format!("{}/{}", vbase, p) };
    join_confined(&root_dir, &normalize_path(&full))
}

pub(crate) fn live_container_usage(cid: u32) -> (u64, u32) {
    let slots = collect_container_slots(cid);
    let cr3s = ostd::scheduler::snapshot_slot_cr3s();
    let (mut pages, mut pids) = (0u64, 0u32);
    for s in slots {
        let idx = s as usize;
        if idx < cr3s.len() && cr3s[idx] != 0 {
            pages += ostd::mm::count_user_pages(cr3s[idx]) as u64;
            pids += 1;
        }
    }
    (pages, pids)
}

/// Enforces Memory Cgroup quota before `sys_brk` or `sys_mmap(MAP_ANONYMOUS)` maps pages.
pub fn check_and_reserve_cgroup_pages(add_pages: usize) -> Result<(), i64> {
    if add_pages == 0 {
        return Ok(());
    }
    let cid = get_process_container_id(ostd::scheduler::active_slot());
    if cid == 0 {
        return Ok(());
    }
    let limit = {
        let reg = CONTAINERS.lock();
        reg.slots
            .iter()
            .find(|c| c.active && c.id == cid)
            .map(|c| c.mem_limit_pages)
            .unwrap_or(0)
    };
    let (used, pids) = live_container_usage(cid);
    let next_used = used.saturating_add(add_pages as u64);
    let mut reg = CONTAINERS.lock();
    if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == cid) {
        c.used_pages = used;
        c.peak_pages = c.peak_pages.max(used);
        c.pid_count = pids.max(1);
        if limit > 0 && next_used > limit {
            ostd::early_println!(
                "[cgroup::mem] Container #{} \"{}\" OOM bloqueado: used={} + req={} > limit={} paginas (-ENOMEM)",
                cid, c.name, used, add_pages, limit
            );
            return Err(-12); // -ENOMEM
        }
        c.used_pages = next_used;
        c.peak_pages = c.peak_pages.max(next_used);
    }
    Ok(())
}

/// Refreshes live container memory usage and process count after execve/munmap.
pub fn refresh_current_container_pages() {
    let cid = get_process_container_id(ostd::scheduler::active_slot());
    if cid == 0 {
        return;
    }
    let (used, pids) = live_container_usage(cid);
    let mut reg = CONTAINERS.lock();
    if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == cid) {
        c.used_pages = used;
        c.peak_pages = c.peak_pages.max(used);
        if c.status == CTR_STATUS_RUNNING {
            c.pid_count = pids.max(1);
        }
    }
}

pub fn on_process_fork(cid: u32) {
    if cid == 0 {
        return;
    }
    let mut reg = CONTAINERS.lock();
    if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == cid) {
        c.pid_count = c.pid_count.saturating_add(1);
    }
}

pub fn on_process_exit(cid: u32, exit_code: i32) {
    if cid == 0 {
        return;
    }
    let (used, pids) = live_container_usage(cid);
    let mut reg = CONTAINERS.lock();
    if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == cid) {
        c.peak_pages = c.peak_pages.max(used);
        c.pid_count = pids;
        if pids == 0 {
            c.status = CTR_STATUS_EXITED;
            c.last_exit_code = exit_code;
            c.used_pages = 0;
        }
    }
}

/// Linux `sys_chroot` (161): confines active process (and descendants) inside directory `path`.
pub fn sys_chroot(path_addr: u64) -> i64 {
    let raw = match crate::syscall::fs_dir::read_user_cstr_path(path_addr) {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return -2,
        Err(e) => return e,
    };
    let slot = ostd::scheduler::active_slot();
    let root = get_process_root(slot);
    let cwd = crate::vfs::get_process_cwd(slot);
    let target = resolve_for_process(&root, &cwd, &raw);
    if !GLOBAL_VFS.lock().is_directory(&target) {
        return -2; // -ENOENT
    }
    set_process_root(slot, target.clone());
    set_process_cwd(slot, target.clone());
    let pid = ostd::scheduler::active_pid();
    ostd::early_println!("[container] sys_chroot(PID={}, slot={}) -> root=\"{}\"", pid, slot, target);
    0
}

/// Linux `sys_sethostname` (170): sets per-process UTS namespace hostname inherited across fork/exec.
pub fn sys_sethostname(name_addr: u64, len: usize) -> i64 {
    if len == 0 || len > 63 || name_addr == 0 {
        return -22; // -EINVAL
    }
    let mut buf = [0u8; 64];
    if read_user_bytes(name_addr, &mut buf[..len]).is_err() {
        return -14; // -EFAULT
    }
    let s = match core::str::from_utf8(&buf[..len]) {
        Ok(v) => v.trim_end_matches('\0').trim(),
        Err(_) => return -22,
    };
    if s.is_empty() {
        return -22;
    }
    let slot = ostd::scheduler::active_slot();
    set_process_hostname(slot, String::from(s));
    let cid = get_process_container_id(slot);
    if cid != 0 {
        let mut reg = CONTAINERS.lock();
        if let Some(c) = reg.slots.iter_mut().find(|c| c.active && c.id == cid) {
            c.hostname = String::from(s);
        }
    }
    0
}
