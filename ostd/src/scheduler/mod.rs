//! Lock-Free Preemptive Round-Robin Scheduler & Real ASM Context Switcher (`ostd::scheduler`).
//!
//! Connected directly to `ostd/src/arch/x86_64/switch.s` (`__fipsos_context_switch`),
//! the hardware PIT/APIC Timer Interrupt (`IRQ0`, Vector `0x20`), and `sys_sched_yield` (`24`).
//! `npu-telemetryd` executes on its own dedicated 16 KiB hardware stack (`DAEMON_STACK`),
//! switching CPU context in pure x86_64 assembly against the active thread.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::idt::{timer_ticks, InterruptStackFrame};

core::arch::global_asm!(include_str!("../arch/x86_64/switch.s"));

extern "C" {
    pub(crate) fn __fipsos_context_switch(prev_rsp: *mut u64, next_rsp: u64);
}

pub mod daemon;
pub mod signals;
pub mod slots;
pub mod smp_idle;

pub use daemon::*;
pub use signals::*;
pub use slots::*;
pub use smp_idle::*;

/// Round-Robin scheduling quantum in `IRQ0` ticks (`10` ticks @ 100 Hz = `100 ms`).
pub const QUANTUM_TICKS: u64 = 10;

pub(crate) static ACTIVE_SLOT: AtomicU64 = AtomicU64::new(SLOT_SHELL);
static PARENT_SLOT_STACK: [AtomicU64; 4] = [
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
];
static PARENT_SLOT_DEPTH: AtomicU64 = AtomicU64::new(0);

pub(crate) static SHELL_PID: AtomicU64 = AtomicU64::new(1033);
pub(crate) static SHELL_TID: AtomicU64 = AtomicU64::new(1034);
pub(crate) static SHELL_TICKS: AtomicU64 = AtomicU64::new(1);
pub(crate) static SHELL_SWITCHES: AtomicU64 = AtomicU64::new(1);
pub(crate) static SHELL_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
pub(crate) static SHELL_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF0);
pub(crate) static SHELL_CR3: AtomicU64 = AtomicU64::new(0);

pub(crate) static DAEMON_PID: AtomicU64 = AtomicU64::new(1035);
pub(crate) static DAEMON_TID: AtomicU64 = AtomicU64::new(1036);
pub(crate) static DAEMON_TICKS: AtomicU64 = AtomicU64::new(1);
pub(crate) static DAEMON_SWITCHES: AtomicU64 = AtomicU64::new(1);
pub(crate) static DAEMON_RIP: AtomicU64 = AtomicU64::new(0);
pub(crate) static DAEMON_RSP: AtomicU64 = AtomicU64::new(0);
pub(crate) static DAEMON_CR3: AtomicU64 = AtomicU64::new(0);

pub(crate) static CHILD_RUST_PID: AtomicU64 = AtomicU64::new(1037);
pub(crate) static CHILD_RUST_TID: AtomicU64 = AtomicU64::new(1038);
pub(crate) static CHILD_RUST_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHILD_RUST_SWITCHES: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHILD_RUST_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
pub(crate) static CHILD_RUST_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
pub(crate) static CHILD_RUST_CR3: AtomicU64 = AtomicU64::new(0);

pub(crate) static CHILD_CPP_PID: AtomicU64 = AtomicU64::new(1039);
pub(crate) static CHILD_CPP_TID: AtomicU64 = AtomicU64::new(1040);
pub(crate) static CHILD_CPP_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHILD_CPP_SWITCHES: AtomicU64 = AtomicU64::new(0);
pub(crate) static CHILD_CPP_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
pub(crate) static CHILD_CPP_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
pub(crate) static CHILD_CPP_CR3: AtomicU64 = AtomicU64::new(0);

pub(crate) static DESKTOP_UI_PID: AtomicU64 = AtomicU64::new(1041);
pub(crate) static DESKTOP_UI_TID: AtomicU64 = AtomicU64::new(1042);
pub(crate) static DESKTOP_UI_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static DESKTOP_UI_SWITCHES: AtomicU64 = AtomicU64::new(0);
pub(crate) static DESKTOP_UI_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
pub(crate) static DESKTOP_UI_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
pub(crate) static DESKTOP_UI_CR3: AtomicU64 = AtomicU64::new(0);

pub(crate) static DAEMON_ACTIVE: AtomicBool = AtomicBool::new(true);
pub(crate) static BG_DOWNLOADER_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) static BG_DOWNLOADER_PID: AtomicU64 = AtomicU64::new(1051);
pub(crate) static BG_DOWNLOADER_TID: AtomicU64 = AtomicU64::new(1052);
pub(crate) static BG_DOWNLOADER_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static BG_DOWNLOADER_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub(crate) static RING3_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TOTAL_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TOTAL_VOLUNTARY_YIELDS: AtomicU64 = AtomicU64::new(0);


#[derive(Debug, Clone, Copy)]
pub struct SchedulerSnapshot {
    pub total_irq0_ticks: u64,
    pub ring3_ticks: u64,
    pub total_preemptions: u64,
    pub total_voluntary_yields: u64,
    pub active_slot: u64,
    pub active_pid: u64,
    pub active_tid: u64,
    pub active_cr3: u64,
    pub shell_ticks: u64,
    pub shell_switches: u64,
    pub shell_rip: u64,
    pub shell_rsp: u64,
    pub shell_cr3: u64,
    pub daemon_ticks: u64,
    pub daemon_switches: u64,
    pub daemon_rip: u64,
    pub daemon_rsp: u64,
    pub daemon_cr3: u64,
    pub child_rust_ticks: u64,
    pub child_rust_switches: u64,
    pub child_rust_rip: u64,
    pub child_rust_rsp: u64,
    pub child_rust_cr3: u64,
    pub child_cpp_ticks: u64,
    pub child_cpp_switches: u64,
    pub child_cpp_rip: u64,
    pub child_cpp_rsp: u64,
    pub child_cpp_cr3: u64,
    pub desktop_ui_ticks: u64,
    pub desktop_ui_switches: u64,
    pub desktop_ui_rip: u64,
    pub desktop_ui_rsp: u64,
    pub desktop_ui_cr3: u64,
    pub daemon_active: bool,
    pub bg_downloader_active: bool,
    pub bg_downloader_ticks: u64,
    pub bg_downloader_switches: u64,
    pub bg_downloader_pid: u64,
}

/// Records the dedicated physical `PML4` (`CR3`) address owned by the given scheduler slot.
pub fn set_slot_cr3(slot: u64, cr3_phys: u64) {
    match slot {
        SLOT_SHELL => SHELL_CR3.store(cr3_phys, Ordering::Release),
        SLOT_DAEMON => DAEMON_CR3.store(cr3_phys, Ordering::Release),
        SLOT_CHILD_RUST => CHILD_RUST_CR3.store(cr3_phys, Ordering::Release),
        SLOT_CHILD_CPP => CHILD_CPP_CR3.store(cr3_phys, Ordering::Release),
        SLOT_DESKTOP_UI => DESKTOP_UI_CR3.store(cr3_phys, Ordering::Release),
        _ => {}
    }
}

/// Invoked on every hardware `IRQ0` Timer Interrupt (`Vector 0x20`).
pub fn on_timer_irq(frame: &InterruptStackFrame, total_ticks: u64) {
    CPU_CONTROLS[0].irq_ticks.fetch_add(1, Ordering::Relaxed);
    CPU_CONTROLS[0].compute_ticks.fetch_add(1, Ordering::Relaxed);
    CPU_CONTROLS[0].total_switches.store(TOTAL_PREEMPTIONS.load(Ordering::Relaxed), Ordering::Relaxed);

    // Non-blocking poll of network frames and TCP remote console on timer tick
    crate::net::poll_network_and_remote_console();

    let is_ring3 = (frame.code_segment & 0b11) == 3;
    let current_slot = ACTIVE_SLOT.load(Ordering::Relaxed);

    if is_ring3 {
        RING3_TICKS.fetch_add(1, Ordering::Relaxed);
        match current_slot {
            SLOT_DESKTOP_UI => {
                DESKTOP_UI_TICKS.fetch_add(1, Ordering::Relaxed);
                DESKTOP_UI_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
                DESKTOP_UI_RSP.store(frame.stack_pointer, Ordering::Relaxed);
            }
            SLOT_CHILD_CPP => {
                CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
                CHILD_CPP_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
                CHILD_CPP_RSP.store(frame.stack_pointer, Ordering::Relaxed);
            }
            SLOT_CHILD_RUST => {
                CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
                CHILD_RUST_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
                CHILD_RUST_RSP.store(frame.stack_pointer, Ordering::Relaxed);
            }
            _ => {
                SHELL_TICKS.fetch_add(1, Ordering::Relaxed);
                SHELL_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
                SHELL_RSP.store(frame.stack_pointer, Ordering::Relaxed);
            }
        }
    } else if (total_ticks & 1) == 0 {
        match current_slot {
            SLOT_DESKTOP_UI => {
                DESKTOP_UI_TICKS.fetch_add(1, Ordering::Relaxed);
            }
            SLOT_CHILD_CPP => {
                CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
            }
            SLOT_CHILD_RUST => {
                CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                SHELL_TICKS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Preemptive Round-Robin quantum rotation every QUANTUM_TICKS (10 ticks = 100 ms):
    if BG_DOWNLOADER_ACTIVE.load(Ordering::Relaxed) {
        BG_DOWNLOADER_TICKS.fetch_add(1, Ordering::Relaxed);
    }

    // Executes a real assembly context switch (`__fipsos_context_switch`) into `npu-telemetryd`!
    if total_ticks > 0 && (total_ticks % QUANTUM_TICKS) == 0 {
        TOTAL_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        match current_slot {
            SLOT_DESKTOP_UI => {
                DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed);
            }
            SLOT_CHILD_CPP => {
                CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed);
            }
            SLOT_CHILD_RUST => {
                CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed);
            }
        }
        if DAEMON_ACTIVE.load(Ordering::Relaxed) {
            switch_to_daemon_once();
        }
    }
}

/// Switches the active foreground slot to `/bin/desktop_ui.elf` (`SLOT_DESKTOP_UI`),
/// `/bin/npu_infer.elf` (`SLOT_CHILD_RUST`), or `/bin/avatar_cpp.elf` (`SLOT_CHILD_CPP`) upon `sys_execve`,
/// pushing the caller's slot onto `PARENT_SLOT_STACK`.
pub fn on_enter_child(slot: u64, child_pid: u64, child_tid: u64, entry_rip: u64, user_rsp: u64) {
    let parent_slot = ACTIVE_SLOT.load(Ordering::Acquire);
    let depth = PARENT_SLOT_DEPTH.fetch_add(1, Ordering::AcqRel) as usize;
    if depth < PARENT_SLOT_STACK.len() {
        PARENT_SLOT_STACK[depth].store(parent_slot, Ordering::Release);
    }

    match slot {
        SLOT_DESKTOP_UI => {
            DESKTOP_UI_PID.store(child_pid, Ordering::Release);
            DESKTOP_UI_TID.store(child_tid, Ordering::Release);
            DESKTOP_UI_RIP.store(entry_rip, Ordering::Release);
            DESKTOP_UI_RSP.store(user_rsp, Ordering::Release);
            DESKTOP_UI_TICKS.fetch_add(1, Ordering::Relaxed);
            DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
        SLOT_CHILD_CPP => {
            CHILD_CPP_PID.store(child_pid, Ordering::Release);
            CHILD_CPP_TID.store(child_tid, Ordering::Release);
            CHILD_CPP_RIP.store(entry_rip, Ordering::Release);
            CHILD_CPP_RSP.store(user_rsp, Ordering::Release);
            CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            CHILD_RUST_PID.store(child_pid, Ordering::Release);
            CHILD_RUST_TID.store(child_tid, Ordering::Release);
            CHILD_RUST_RIP.store(entry_rip, Ordering::Release);
            CHILD_RUST_RSP.store(user_rsp, Ordering::Release);
            CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
    }

    if parent_slot == SLOT_DESKTOP_UI {
        DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed);
    } else {
        SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    TOTAL_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
    ACTIVE_SLOT.store(slot, Ordering::Release);
    CPU_CONTROLS[0].active_proc_slot.store(slot, Ordering::Release);
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
        entry.is_running = true;
        entry.state = TaskSlotState::Running;
        entry.pid = child_pid;
        entry.assigned_cpu = 0;
        entry.pending_signals = 0;
        entry.sig_handlers = [0; 32];
    }
}

/// Restores the active foreground slot to the stacked parent (`SLOT_DESKTOP_UI` or `SLOT_SHELL`)
/// when a child calls `sys_exit`, returning `(exited_slot, resumed_parent_slot)`.
pub fn on_exit_child(exit_rip: u64) -> (u64, u64) {
    let cur_depth = PARENT_SLOT_DEPTH.load(Ordering::Acquire);
    let resumed_parent = if cur_depth > 0 {
        let new_depth = cur_depth - 1;
        PARENT_SLOT_DEPTH.store(new_depth, Ordering::Release);
        let idx = (new_depth as usize).min(PARENT_SLOT_STACK.len() - 1);
        PARENT_SLOT_STACK[idx].load(Ordering::Acquire)
    } else {
        SLOT_SHELL
    };

    let prev_slot = ACTIVE_SLOT.swap(resumed_parent, Ordering::AcqRel);
    match prev_slot {
        SLOT_DESKTOP_UI => {
            if exit_rip != 0 {
                DESKTOP_UI_RIP.store(exit_rip, Ordering::Release);
            }
            DESKTOP_UI_TICKS.fetch_add(1, Ordering::Relaxed);
            DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
        SLOT_CHILD_CPP => {
            if exit_rip != 0 {
                CHILD_CPP_RIP.store(exit_rip, Ordering::Release);
            }
            CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            if exit_rip != 0 {
                CHILD_RUST_RIP.store(exit_rip, Ordering::Release);
            }
            CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed);
        }
    }

    if resumed_parent == SLOT_DESKTOP_UI {
        DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed);
    } else {
        SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    CPU_CONTROLS[0].active_proc_slot.store(resumed_parent, Ordering::Release);
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.slot == prev_slot) {
        entry.is_running = false;
        entry.state = TaskSlotState::Free;
    }
    if let Some(entry) = table.iter_mut().find(|e| e.slot == resumed_parent) {
        entry.is_running = true;
        entry.state = TaskSlotState::Running;
    }
    (prev_slot, resumed_parent)
}

/// Handles a cooperative `sys_sched_yield` (`24`) from Ring 3 by executing a real assembly
/// context switch (`__fipsos_context_switch`) to `npu-telemetryd` on its 16 KiB stack.
pub fn on_sys_sched_yield(user_rip: u64, user_rsp: u64) -> (u64, u64) {
    TOTAL_VOLUNTARY_YIELDS.fetch_add(1, Ordering::Relaxed);

    // Real assembly context switch (`switch.s`) into `npu-telemetryd` and back!
    switch_to_daemon_once();

    let current_slot = ACTIVE_SLOT.load(Ordering::Acquire);
    match current_slot {
        SLOT_DESKTOP_UI => {
            DESKTOP_UI_TICKS.fetch_add(1, Ordering::Relaxed);
            DESKTOP_UI_RIP.store(user_rip, Ordering::Relaxed);
            DESKTOP_UI_RSP.store(user_rsp, Ordering::Relaxed);
            let sw = DESKTOP_UI_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            (DESKTOP_UI_TID.load(Ordering::Relaxed), sw)
        }
        SLOT_CHILD_CPP => {
            CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_CPP_RIP.store(user_rip, Ordering::Relaxed);
            CHILD_CPP_RSP.store(user_rsp, Ordering::Relaxed);
            let sw = CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            (CHILD_CPP_TID.load(Ordering::Relaxed), sw)
        }
        SLOT_CHILD_RUST => {
            CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_RUST_RIP.store(user_rip, Ordering::Relaxed);
            CHILD_RUST_RSP.store(user_rsp, Ordering::Relaxed);
            let sw = CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            (CHILD_RUST_TID.load(Ordering::Relaxed), sw)
        }
        _ => {
            SHELL_TICKS.fetch_add(1, Ordering::Relaxed);
            SHELL_RIP.store(user_rip, Ordering::Relaxed);
            SHELL_RSP.store(user_rsp, Ordering::Relaxed);
            let sw = SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            (SHELL_TID.load(Ordering::Relaxed), sw)
        }
    }
}

/// Records the most recent user `RIP` and `RSP` of the calling thread on any syscall entry.
pub fn record_syscall_frame(user_rip: u64, user_rsp: u64) {
    let current_slot = ACTIVE_SLOT.load(Ordering::Relaxed);
    match current_slot {
        SLOT_DESKTOP_UI => {
            DESKTOP_UI_RIP.store(user_rip, Ordering::Relaxed);
            DESKTOP_UI_RSP.store(user_rsp, Ordering::Relaxed);
        }
        SLOT_CHILD_CPP => {
            CHILD_CPP_RIP.store(user_rip, Ordering::Relaxed);
            CHILD_CPP_RSP.store(user_rsp, Ordering::Relaxed);
        }
        SLOT_CHILD_RUST => {
            CHILD_RUST_RIP.store(user_rip, Ordering::Relaxed);
            CHILD_RUST_RSP.store(user_rsp, Ordering::Relaxed);
        }
        _ => {
            SHELL_RIP.store(user_rip, Ordering::Relaxed);
            SHELL_RSP.store(user_rsp, Ordering::Relaxed);
        }
    }
}

/// Returns `(active_pid_koid, active_tid_koid)` for `sys_getpid` (`39`).
pub fn active_pid_and_tid() -> (u64, u64) {
    match ACTIVE_SLOT.load(Ordering::Acquire) {
        SLOT_DESKTOP_UI => (
            DESKTOP_UI_PID.load(Ordering::Relaxed),
            DESKTOP_UI_TID.load(Ordering::Relaxed),
        ),
        SLOT_CHILD_CPP => (
            CHILD_CPP_PID.load(Ordering::Relaxed),
            CHILD_CPP_TID.load(Ordering::Relaxed),
        ),
        SLOT_CHILD_RUST => (
            CHILD_RUST_PID.load(Ordering::Relaxed),
            CHILD_RUST_TID.load(Ordering::Relaxed),
        ),
        SLOT_DAEMON => (
            DAEMON_PID.load(Ordering::Relaxed),
            DAEMON_TID.load(Ordering::Relaxed),
        ),
        SLOT_BG_DOWNLOADER => (
            BG_DOWNLOADER_PID.load(Ordering::Relaxed),
            BG_DOWNLOADER_TID.load(Ordering::Relaxed),
        ),
        SLOT_SHELL => (
            SHELL_PID.load(Ordering::Relaxed),
            SHELL_TID.load(Ordering::Relaxed),
        ),
        dyn_slot => {
            if let Some(table) = SCHEDULER_LOCK.try_lock() {
                if let Some(entry) = table.iter().find(|e| e.slot == dyn_slot && e.pid != 0) {
                    return (entry.pid, entry.pid + 1);
                }
            }
            (SHELL_PID.load(Ordering::Relaxed), SHELL_TID.load(Ordering::Relaxed))
        }
    }
}

/// Returns the currently active scheduler slot index.
#[inline]
pub fn active_slot() -> u64 {
    ACTIVE_SLOT.load(Ordering::Acquire)
}

/// Sets the currently active scheduler slot index.
#[inline]
pub fn set_active_slot(slot: u64) {
    ACTIVE_SLOT.store(slot, Ordering::Release);
    CPU_CONTROLS[0].active_proc_slot.store(slot, Ordering::Release);
}

/// Returns the PID of the current process running in `ACTIVE_SLOT`.
#[inline]
pub fn active_pid() -> u64 {
    let (pid, _) = active_pid_and_tid();
    pid
}

/// Returns the parent PID (`PPID`) of the active process.
pub fn get_active_ppid() -> u64 {
    let slot = ACTIVE_SLOT.load(Ordering::Acquire);
    if slot == SLOT_SHELL {
        return 1;
    }
    if let Some(table) = SCHEDULER_LOCK.try_lock() {
        if let Some(entry) = table.iter().find(|e| e.slot == slot) {
            return entry.ppid.max(1);
        }
    }
    1
}


/// Captures a consistent snapshot of all Round-Robin scheduler metrics and hardware `CR3` roots.
pub fn snapshot() -> SchedulerSnapshot {
    let (active_pid, active_tid) = active_pid_and_tid();
    SchedulerSnapshot {
        total_irq0_ticks: timer_ticks(),
        ring3_ticks: RING3_TICKS.load(Ordering::Relaxed),
        total_preemptions: TOTAL_PREEMPTIONS.load(Ordering::Relaxed),
        total_voluntary_yields: TOTAL_VOLUNTARY_YIELDS.load(Ordering::Relaxed),
        active_slot: ACTIVE_SLOT.load(Ordering::Relaxed),
        active_pid,
        active_tid,
        active_cr3: crate::mm::active_cr3(),
        shell_ticks: SHELL_TICKS.load(Ordering::Relaxed),
        shell_switches: SHELL_SWITCHES.load(Ordering::Relaxed),
        shell_rip: SHELL_RIP.load(Ordering::Relaxed),
        shell_rsp: SHELL_RSP.load(Ordering::Relaxed),
        shell_cr3: SHELL_CR3.load(Ordering::Relaxed),
        daemon_ticks: DAEMON_TICKS.load(Ordering::Relaxed),
        daemon_switches: DAEMON_SWITCHES.load(Ordering::Relaxed),
        daemon_rip: DAEMON_RIP.load(Ordering::Relaxed),
        daemon_rsp: DAEMON_RSP.load(Ordering::Relaxed),
        daemon_cr3: DAEMON_CR3.load(Ordering::Relaxed),
        child_rust_ticks: CHILD_RUST_TICKS.load(Ordering::Relaxed),
        child_rust_switches: CHILD_RUST_SWITCHES.load(Ordering::Relaxed),
        child_rust_rip: CHILD_RUST_RIP.load(Ordering::Relaxed),
        child_rust_rsp: CHILD_RUST_RSP.load(Ordering::Relaxed),
        child_rust_cr3: CHILD_RUST_CR3.load(Ordering::Relaxed),
        child_cpp_ticks: CHILD_CPP_TICKS.load(Ordering::Relaxed),
        child_cpp_switches: CHILD_CPP_SWITCHES.load(Ordering::Relaxed),
        child_cpp_rip: CHILD_CPP_RIP.load(Ordering::Relaxed),
        child_cpp_rsp: CHILD_CPP_RSP.load(Ordering::Relaxed),
        child_cpp_cr3: CHILD_CPP_CR3.load(Ordering::Relaxed),
        desktop_ui_ticks: DESKTOP_UI_TICKS.load(Ordering::Relaxed),
        desktop_ui_switches: DESKTOP_UI_SWITCHES.load(Ordering::Relaxed),
        desktop_ui_rip: DESKTOP_UI_RIP.load(Ordering::Relaxed),
        desktop_ui_rsp: DESKTOP_UI_RSP.load(Ordering::Relaxed),
        desktop_ui_cr3: DESKTOP_UI_CR3.load(Ordering::Relaxed),
        daemon_active: DAEMON_ACTIVE.load(Ordering::Relaxed),
        bg_downloader_active: BG_DOWNLOADER_ACTIVE.load(Ordering::Relaxed),
        bg_downloader_ticks: BG_DOWNLOADER_TICKS.load(Ordering::Relaxed),
        bg_downloader_switches: BG_DOWNLOADER_SWITCHES.load(Ordering::Relaxed),
        bg_downloader_pid: BG_DOWNLOADER_PID.load(Ordering::Relaxed),
    }
}

