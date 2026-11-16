//! Process Slot Descriptors and Table Allocation (`ostd::scheduler::slots`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::SpinLock;
use super::telemetry::{is_slot_reclaim_pending, mark_slot_reclaimed_external, reset_slot_ticks};

pub const SLOT_SHELL: u64 = 0;
pub const SLOT_DAEMON: u64 = 1;
pub const SLOT_CHILD_RUST: u64 = 2;
pub const SLOT_CHILD_CPP: u64 = 3;
pub const SLOT_DESKTOP_UI: u64 = 4;
pub const SLOT_BG_DOWNLOADER: u64 = 5;

/// Maximum number of concurrent processes tracked by the dynamic scheduler.
pub const MAX_PROCESSES: usize = 32;

/// Process and task lifecycle state in the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSlotState {
    Free,
    Ready,
    Running,
    Blocked,
    Zombie,
    Exited,
}

/// Concurrent task descriptor used for deadlock-free SMP scheduling.
#[derive(Debug, Clone, Copy)]
pub struct ProcessSlotDescriptor {
    pub slot: u64,
    pub pid: u64,
    pub ppid: u64,
    pub assigned_cpu: u32,
    pub state: TaskSlotState,
    pub is_running: bool,
    pub exit_code: i32,
    pub cr3_phys: u64,
    pub rip: u64,
    pub rsp: u64,
    pub ticks: u64,
    pub switches: u64,
    pub pending_signals: u64,
    pub blocked_signals: u64,
    pub sig_handlers: [u64; 32],
    pub cap_mask: u64,
    pub sandbox_flags: u64,
    pub auto_reap: bool,
    pub name: [u8; 32],
    pub name_len: u8,
}

pub type TaskSlotDescriptor = ProcessSlotDescriptor;

impl ProcessSlotDescriptor {
    /// Returns the clean process/binary name currently bound to this slot.
    pub fn name_str(&self) -> &str {
        let len = (self.name_len as usize).min(self.name.len());
        core::str::from_utf8(&self.name[..len]).unwrap_or("unknown")
    }

    /// Overwrites the slot name cleanly (basename without `.elf`), zeroing residual bytes.
    pub fn set_name(&mut self, raw_path: &str) {
        self.name = [0u8; 32];
        let trimmed = raw_path.trim();
        let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
        let clean = base.strip_suffix(".elf").unwrap_or(base);
        let bytes = clean.as_bytes();
        let copy_len = bytes.len().min(self.name.len());
        self.name[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.name_len = copy_len as u8;
    }
}

const fn encode_const_name(s: &str) -> ([u8; 32], u8) {
    let mut buf = [0u8; 32];
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && i < 32 {
        buf[i] = bytes[i];
        i += 1;
    }
    (buf, i as u8)
}

const fn make_default_slot(slot: u64) -> ProcessSlotDescriptor {
    ProcessSlotDescriptor {
        slot,
        pid: 0,
        ppid: 0,
        assigned_cpu: 0,
        state: TaskSlotState::Free,
        is_running: false,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0,
        rsp: 0,
        ticks: 0,
        switches: 0,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: [0; 32],
        name_len: 0,
    }
}

const fn init_scheduler_table() -> [ProcessSlotDescriptor; MAX_PROCESSES] {
    let mut table = [
        make_default_slot(0), make_default_slot(1), make_default_slot(2), make_default_slot(3),
        make_default_slot(4), make_default_slot(5), make_default_slot(6), make_default_slot(7),
        make_default_slot(8), make_default_slot(9), make_default_slot(10), make_default_slot(11),
        make_default_slot(12), make_default_slot(13), make_default_slot(14), make_default_slot(15),
        make_default_slot(16), make_default_slot(17), make_default_slot(18), make_default_slot(19),
        make_default_slot(20), make_default_slot(21), make_default_slot(22), make_default_slot(23),
        make_default_slot(24), make_default_slot(25), make_default_slot(26), make_default_slot(27),
        make_default_slot(28), make_default_slot(29), make_default_slot(30), make_default_slot(31),
    ];
    let (shell_name, shell_name_len) = encode_const_name("fipsos_sh");
    let (daemon_name, daemon_name_len) = encode_const_name("npu-telemetryd");
    table[0] = ProcessSlotDescriptor {
        slot: SLOT_SHELL,
        pid: 1033,
        ppid: 1,
        assigned_cpu: 0,
        state: TaskSlotState::Running,
        is_running: true,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0x0040_0000,
        rsp: 0x0080_3FF0,
        ticks: 1,
        switches: 1,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: shell_name,
        name_len: shell_name_len,
    };
    table[1] = ProcessSlotDescriptor {
        slot: SLOT_DAEMON,
        pid: 1035,
        ppid: 1,
        assigned_cpu: 1,
        state: TaskSlotState::Running,
        is_running: true,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0,
        rsp: 0,
        ticks: 1,
        switches: 1,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: daemon_name,
        name_len: daemon_name_len,
    };
    table[2] = ProcessSlotDescriptor {
        slot: SLOT_CHILD_RUST,
        pid: 1037,
        ppid: 1033,
        assigned_cpu: 0,
        state: TaskSlotState::Free,
        is_running: false,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0x0040_0000,
        rsp: 0x0080_3FF8,
        ticks: 0,
        switches: 0,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: [0; 32],
        name_len: 0,
    };
    table[3] = ProcessSlotDescriptor {
        slot: SLOT_CHILD_CPP,
        pid: 1039,
        ppid: 1033,
        assigned_cpu: 0,
        state: TaskSlotState::Free,
        is_running: false,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0x0040_0000,
        rsp: 0x0080_3FF8,
        ticks: 0,
        switches: 0,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: [0; 32],
        name_len: 0,
    };
    table[4] = ProcessSlotDescriptor {
        slot: SLOT_DESKTOP_UI,
        pid: 1041,
        ppid: 1033,
        assigned_cpu: 0,
        state: TaskSlotState::Free,
        is_running: false,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0x0040_0000,
        rsp: 0x0080_3FF8,
        ticks: 0,
        switches: 0,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: [0; 32],
        name_len: 0,
    };
    table[5] = ProcessSlotDescriptor {
        slot: SLOT_BG_DOWNLOADER,
        pid: 1051,
        ppid: 1035,
        assigned_cpu: 1,
        state: TaskSlotState::Free,
        is_running: false,
        exit_code: 0,
        cr3_phys: 0,
        rip: 0,
        rsp: 0,
        ticks: 0,
        switches: 0,
        pending_signals: 0,
        blocked_signals: 0,
        sig_handlers: [0; 32],
        cap_mask: 0x1F,
        sandbox_flags: 0,
        auto_reap: false,
        name: [0; 32],
        name_len: 0,
    };
    table
}

pub(crate) static SCHEDULER_LOCK: SpinLock<[ProcessSlotDescriptor; MAX_PROCESSES]> =
    SpinLock::new(init_scheduler_table());

pub(crate) static NEXT_DYNAMIC_PID: AtomicU64 = AtomicU64::new(2000);

pub(crate) fn collect_cr3_for_free(cr3_phys: u64, out: &mut Vec<u64>) {
    if cr3_phys != 0
        && cr3_phys != crate::mm::master_kernel_cr3()
        && cr3_phys != crate::mm::active_cr3()
        && !out.contains(&cr3_phys)
    {
        out.push(cr3_phys);
    }
}

pub(crate) fn free_reaped_cr3s(cr3s: Vec<u64>) {
    for cr3 in cr3s {
        let _ = crate::mm::destroy_process_vm_space(cr3);
    }
}

pub(crate) fn reset_slot_to_free(entry: &mut ProcessSlotDescriptor) {
    mark_slot_reclaimed_external(entry.slot);
    reset_slot_ticks(entry.slot);
    entry.state = TaskSlotState::Free;
    entry.is_running = false;
    entry.pid = 0;
    entry.ppid = 0;
    entry.cr3_phys = 0;
    entry.ticks = 0;
    entry.switches = 0;
    entry.cap_mask = 0x1F;
    entry.sandbox_flags = 0;
    entry.auto_reap = false;
    entry.name = [0; 32];
    entry.name_len = 0;
}

pub(crate) fn reap_orphan_and_bg_zombies(
    table: &mut [ProcessSlotDescriptor; MAX_PROCESSES],
    freed_cr3s: &mut Vec<u64>,
) {
    for i in 2..MAX_PROCESSES {
        if table[i].state == TaskSlotState::Zombie {
            let ppid = table[i].ppid;
            let parent_alive = ppid > 1
                && table.iter().any(|p| {
                    p.pid == ppid
                        && p.state != TaskSlotState::Free
                        && p.state != TaskSlotState::Zombie
                        && p.state != TaskSlotState::Exited
                });
            if table[i].auto_reap || !parent_alive {
                collect_cr3_for_free(table[i].cr3_phys, freed_cr3s);
                reset_slot_to_free(&mut table[i]);
            }
        }
    }
}

/// Returns `Some((slot_index, child_pid))` or `None` if the process table is full.
pub fn allocate_child_slot(parent_pid: u64, child_cr3: u64) -> Option<(u64, u64)> {
    let mut freed_cr3s = Vec::new();
    let res = {
        let mut table = SCHEDULER_LOCK.lock();
        reap_orphan_and_bg_zombies(&mut table, &mut freed_cr3s);
        // Prefer clean dynamic slots (6..MAX_PROCESSES) not pending FD reclaim, fallback (2..MAX_PROCESSES)
        let free_idx = table
            .iter()
            .position(|e| {
                e.slot >= 6
                    && e.state == TaskSlotState::Free
                    && !is_slot_reclaim_pending(e.slot)
            })
            .or_else(|| {
                table.iter().position(|e| {
                    e.slot >= 2
                        && e.state == TaskSlotState::Free
                        && !is_slot_reclaim_pending(e.slot)
                })
            })
            .or_else(|| table.iter().position(|e| e.slot >= 6 && e.state == TaskSlotState::Free))
            .or_else(|| table.iter().position(|e| e.slot >= 2 && e.state == TaskSlotState::Free))?;

        let child_pid = NEXT_DYNAMIC_PID.fetch_add(1, Ordering::Relaxed);
        let slot = table[free_idx].slot;

        let (parent_handlers, parent_blocked, parent_caps, parent_sb_flags, parent_name, parent_name_len) =
            if let Some(parent) = table.iter().find(|e| e.pid == parent_pid) {
                (
                    parent.sig_handlers,
                    parent.blocked_signals,
                    parent.cap_mask,
                    parent.sandbox_flags,
                    parent.name,
                    parent.name_len,
                )
            } else {
                ([0; 32], 0, 0x1F, 0, [0; 32], 0)
            };

        table[free_idx].pid = child_pid;
        table[free_idx].ppid = parent_pid;
        table[free_idx].assigned_cpu = 0;
        table[free_idx].state = TaskSlotState::Running;
        table[free_idx].is_running = true;
        table[free_idx].exit_code = 0;
        table[free_idx].cr3_phys = child_cr3;
        table[free_idx].rip = 0;
        table[free_idx].rsp = 0;
        table[free_idx].ticks = 0;
        table[free_idx].switches = 0;
        table[free_idx].pending_signals = 0;
        table[free_idx].blocked_signals = parent_blocked;
        table[free_idx].sig_handlers = parent_handlers;
        table[free_idx].cap_mask = parent_caps;
        table[free_idx].sandbox_flags = parent_sb_flags;
        table[free_idx].auto_reap = false;
        table[free_idx].name = parent_name;
        table[free_idx].name_len = parent_name_len;

        Some((slot, child_pid))
    };
    free_reaped_cr3s(freed_cr3s);
    res
}

/// Marks a terminating process as `Zombie` (or auto-reaps if background/orphaned).
pub fn mark_process_exit(pid: u64, raw_status: i32) -> Option<(u64, u64)> {
    let mut freed_cr3s = Vec::new();
    let res = {
        let mut table = SCHEDULER_LOCK.lock();
        let idx = table.iter().position(|e| e.pid == pid)?;
        let ppid = table[idx].ppid;
        let slot = table[idx].slot;
        if slot < 2 {
            return None;
        }
        let auto_reap = table[idx].auto_reap || ppid <= 1;
        // Caller (`ExitUser` or `recover_from_user_fault`) destroys the active exiting CR3.
        table[idx].cr3_phys = 0;

        if auto_reap && slot >= 2 {
            reset_slot_to_free(&mut table[idx]);
        } else {
            table[idx].state = TaskSlotState::Zombie;
            table[idx].is_running = false;
            table[idx].exit_code = (raw_status & 0xff) << 8;
            if ppid > 0 {
                if let Some(parent) = table.iter_mut().find(|e| e.pid == ppid) {
                    parent.pending_signals |= 1 << 17; // SIGCHLD
                }
            }
        }
        reap_orphan_and_bg_zombies(&mut table, &mut freed_cr3s);
        Some((ppid, slot))
    };
    free_reaped_cr3s(freed_cr3s);
    res
}

/// Collects an exited child process in `Zombie` state and returns `Ok((pid, status, slot, residual_cr3))`.
/// If the child is still running, returns `Err(0)`.
/// If no matching child process exists, returns `Err(-10)` (`-ECHILD`).
pub fn collect_zombie_child(parent_pid: u64, target_pid: i64) -> Result<(u64, i32, u64, u64), i32> {
    let mut freed_cr3s = Vec::new();
    let result = {
        let mut table = SCHEDULER_LOCK.lock();
        if target_pid <= 0 {
            if let Some(entry) = table
                .iter_mut()
                .find(|e| e.ppid == parent_pid && e.state == TaskSlotState::Zombie)
            {
                let pid = entry.pid;
                let code = entry.exit_code;
                let slot = entry.slot;
                let cr3 = entry.cr3_phys;
                reset_slot_to_free(entry);
                Ok((pid, code, slot, cr3))
            } else {
                let has_running = table.iter().any(|e| {
                    e.ppid == parent_pid
                        && (e.state == TaskSlotState::Running
                            || e.state == TaskSlotState::Ready
                            || e.state == TaskSlotState::Blocked)
                });
                reap_orphan_and_bg_zombies(&mut table, &mut freed_cr3s);
                if has_running {
                    Err(0)
                } else {
                    Err(-10) // -ECHILD
                }
            }
        } else {
            let t_pid = target_pid as u64;
            if let Some(entry) = table
                .iter_mut()
                .find(|e| e.pid == t_pid && (e.ppid == parent_pid || e.ppid == 0 || parent_pid == 0))
            {
                if entry.state == TaskSlotState::Zombie {
                    let pid = entry.pid;
                    let code = entry.exit_code;
                    let slot = entry.slot;
                    let cr3 = entry.cr3_phys;
                    reset_slot_to_free(entry);
                    Ok((pid, code, slot, cr3))
                } else {
                    Err(0) // Still running
                }
            } else {
                Err(-10) // -ECHILD
            }
        }
    };
    free_reaped_cr3s(freed_cr3s);
    if let Ok((_, _, _, residual_cr3)) = result {
        if residual_cr3 != 0
            && residual_cr3 != crate::mm::active_cr3()
            && residual_cr3 != crate::mm::master_kernel_cr3()
        {
            let _ = crate::mm::destroy_process_vm_space(residual_cr3);
        }
    }
    result
}

/// Updates the clean executable basename bound to scheduler `slot`.
pub fn set_slot_name(slot: u64, raw_path: &str) {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.set_name(raw_path);
    }
}

/// Synchronizes slot PID and binary name from boot initialization or `sys_execve`.
pub fn sync_slot_identity(slot: u64, pid: u64, raw_path: &str) {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.pid = pid;
        entry.set_name(raw_path);
    }
}

/// Returns `(cap_mask, sandbox_flags)` for the given scheduler slot.
pub fn slot_capabilities(slot: u64) -> (u64, u64) {
    let table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter().find(|e| e.slot == slot) {
        (entry.cap_mask, entry.sandbox_flags)
    } else {
        (0x1F, 0)
    }
}

/// Irreversibly narrows `cap_mask` and ORs `sandbox_flags` on `slot`.
pub fn narrow_slot_capabilities(slot: u64, mask: u64, flags: u64) -> (u64, u64) {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.cap_mask &= mask & 0x1F;
        entry.sandbox_flags |= flags;
        (entry.cap_mask, entry.sandbox_flags)
    } else {
        (0x1F, 0)
    }
}

/// Marks `slot` for automatic kernel reaping upon process exit.
pub fn set_slot_auto_reap(slot: u64, enabled: bool) {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.auto_reap = enabled;
    }
}

/// Copies `cap_mask` and `sandbox_flags` from `from_slot` to `to_slot` across `execve`.
pub fn inherit_slot_capabilities(from_slot: u64, to_slot: u64) {
    if from_slot == to_slot {
        return;
    }
    let mut table = SCHEDULER_LOCK.lock();
    let (caps, flags) = table
        .iter()
        .find(|e| e.slot == from_slot)
        .map(|e| (e.cap_mask, e.sandbox_flags))
        .unwrap_or((0x1F, 0));
    if let Some(dst) = table.iter_mut().find(|e| e.slot == to_slot) {
        dst.cap_mask = caps;
        dst.sandbox_flags = flags;
    }
}

/// Resets `slot` capability mask and sandbox flags back to `CAP_ALL` on child exec exit.
pub fn reset_slot_capabilities(slot: u64) {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.cap_mask = 0x1F;
        entry.sandbox_flags = 0;
    }
}

/// Returns a consistent snapshot of all non-`Free` slots across the 32-process table.
pub fn occupied_slots_snapshot() -> Vec<ProcessSlotDescriptor> {
    let table = SCHEDULER_LOCK.lock();
    table
        .iter()
        .copied()
        .filter(|e| e.state != TaskSlotState::Free && e.pid != 0)
        .collect()
}

