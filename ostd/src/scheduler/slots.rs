//! Process Slot Descriptors and Table Allocation (`ostd::scheduler::slots`).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::SpinLock;

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
}

pub type TaskSlotDescriptor = ProcessSlotDescriptor;

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
    };
    table
}

pub(crate) static SCHEDULER_LOCK: SpinLock<[ProcessSlotDescriptor; MAX_PROCESSES]> =
    SpinLock::new(init_scheduler_table());

pub(crate) static NEXT_DYNAMIC_PID: AtomicU64 = AtomicU64::new(2000);

/// Returns `Some((slot_index, child_pid))` or `None` if the process table is full.
pub fn allocate_child_slot(parent_pid: u64, child_cr3: u64) -> Option<(u64, u64)> {
    let mut table = SCHEDULER_LOCK.lock();
    // Prefer dynamic slots (6..MAX_PROCESSES), otherwise fallback to (2..MAX_PROCESSES)
    let free_idx = table
        .iter()
        .position(|e| e.slot >= 6 && e.state == TaskSlotState::Free)
        .or_else(|| table.iter().position(|e| e.slot >= 2 && e.state == TaskSlotState::Free))?;

    let child_pid = NEXT_DYNAMIC_PID.fetch_add(1, Ordering::Relaxed);
    let slot = table[free_idx].slot;

    let (parent_handlers, parent_blocked) = if let Some(parent) = table.iter().find(|e| e.pid == parent_pid) {
        (parent.sig_handlers, parent.blocked_signals)
    } else {
        ([0; 32], 0)
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

    Some((slot, child_pid))
}

/// Marks a terminating process as `Zombie` and records its raw exit status code.
pub fn mark_process_exit(pid: u64, raw_status: i32) -> Option<(u64, u64)> {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.pid == pid) {
        entry.state = TaskSlotState::Zombie;
        entry.is_running = false;
        entry.exit_code = (raw_status & 0xff) << 8;
        let ppid = entry.ppid;
        let slot = entry.slot;
        if ppid > 0 {
            if let Some(parent) = table.iter_mut().find(|e| e.pid == ppid) {
                parent.pending_signals |= 1 << 17; // SIGCHLD
            }
        }
        Some((ppid, slot))
    } else {
        None
    }
}


/// If the child is still running, returns `Err(0)`.
/// If no matching child process exists, returns `Err(-10)` (`-ECHILD`).
pub fn collect_zombie_child(parent_pid: u64, target_pid: i64) -> Result<(u64, i32), i32> {
    let mut table = SCHEDULER_LOCK.lock();
    if target_pid <= 0 {
        // Match any child of the calling parent
        if let Some(entry) = table
            .iter_mut()
            .find(|e| e.ppid == parent_pid && e.state == TaskSlotState::Zombie)
        {
            let pid = entry.pid;
            let code = entry.exit_code;
            entry.state = TaskSlotState::Free;
            entry.is_running = false;
            entry.pid = 0;
            entry.ppid = 0;
            entry.cr3_phys = 0;
            return Ok((pid, code));
        }
        let has_running = table.iter().any(|e| {
            e.ppid == parent_pid
                && (e.state == TaskSlotState::Running
                    || e.state == TaskSlotState::Ready
                    || e.state == TaskSlotState::Blocked)
        });
        if has_running {
            Err(0)
        } else {
            Err(-10) // -ECHILD
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
                entry.state = TaskSlotState::Free;
                entry.is_running = false;
                entry.pid = 0;
                entry.ppid = 0;
                entry.cr3_phys = 0;
                return Ok((pid, code));
            } else {
                return Err(0); // Still running
            }
        }
        Err(-10) // -ECHILD
    }
}

