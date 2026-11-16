//! Real-Time SMP Core, Slot Reclamation & Process Telemetry Snapshot (`ostd::scheduler::telemetry`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use super::slots::{
    collect_cr3_for_free, free_reaped_cr3s, reap_orphan_and_bg_zombies, reset_slot_to_free,
    ProcessSlotDescriptor, TaskSlotState, MAX_PROCESSES, SCHEDULER_LOCK, SLOT_BG_DOWNLOADER,
    SLOT_CHILD_CPP, SLOT_CHILD_RUST, SLOT_DAEMON, SLOT_DESKTOP_UI, SLOT_SHELL,
};
use super::smp_idle::{CPU_CONTROLS, MAX_CPUS};
use super::{
    bg_httpd_status, BG_DOWNLOADER_TICKS, CHILD_CPP_CR3, CHILD_CPP_TICKS, CHILD_CPP_TID,
    CHILD_RUST_CR3, CHILD_RUST_TICKS, CHILD_RUST_TID, DAEMON_ACTIVE, DAEMON_CR3, DAEMON_TICKS,
    DAEMON_TID, DESKTOP_UI_CR3, DESKTOP_UI_TICKS, DESKTOP_UI_TID, SHELL_CR3, SHELL_TICKS,
    SHELL_TID,
};

static RECLAIMED_SLOTS_MASK: AtomicU32 = AtomicU32::new(0);
static SLOT_TICKS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];

#[inline]
pub(crate) fn mark_slot_reclaimed_external(slot: u64) {
    if slot >= 2 && slot < 32 {
        RECLAIMED_SLOTS_MASK.fetch_or(1u32 << slot, Ordering::AcqRel);
    }
}

#[inline]
pub(crate) fn is_slot_reclaim_pending(slot: u64) -> bool {
    slot < 32 && (RECLAIMED_SLOTS_MASK.load(Ordering::Acquire) & (1u32 << slot)) != 0
}

/// Returns and clears the bitmask of scheduler slots freed since the last check (for FD cleanup).
#[inline]
pub fn take_reclaimed_slots_mask() -> u32 {
    RECLAIMED_SLOTS_MASK.swap(0, Ordering::AcqRel)
}

/// Synchronously reaps orphan and auto-reap zombies before draining the reclaimed slot mask.
pub fn reap_orphan_zombies() {
    let mut freed_cr3s = Vec::new();
    {
        let mut table = SCHEDULER_LOCK.lock();
        reap_orphan_and_bg_zombies(&mut table, &mut freed_cr3s);
    }
    free_reaped_cr3s(freed_cr3s);
}

#[inline]
pub(crate) fn bump_slot_tick(slot: u64) {
    if (slot as usize) < MAX_PROCESSES {
        SLOT_TICKS[slot as usize].fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub(crate) fn reset_slot_ticks(slot: u64) {
    if (slot as usize) < MAX_PROCESSES {
        SLOT_TICKS[slot as usize].store(0, Ordering::Release);
    }
}

#[inline]
pub fn slot_live_ticks(slot: u64) -> u64 {
    match slot {
        SLOT_SHELL => SHELL_TICKS.load(Ordering::Relaxed),
        SLOT_DAEMON => DAEMON_TICKS.load(Ordering::Relaxed),
        SLOT_CHILD_RUST => CHILD_RUST_TICKS.load(Ordering::Relaxed),
        SLOT_CHILD_CPP => CHILD_CPP_TICKS.load(Ordering::Relaxed),
        SLOT_DESKTOP_UI => DESKTOP_UI_TICKS.load(Ordering::Relaxed),
        SLOT_BG_DOWNLOADER => BG_DOWNLOADER_TICKS.load(Ordering::Relaxed),
        _ if (slot as usize) < MAX_PROCESSES => SLOT_TICKS[slot as usize].load(Ordering::Relaxed),
        _ => 0,
    }
}

/// Acquires `preferred_slot` for `sys_execve` when idle, or an unused dynamic slot (`6..32`)
/// if `preferred_slot` is currently occupied, recording `child_cr3` cleanly.
pub fn acquire_exec_slot(preferred_slot: u64, child_cr3: u64) -> Option<u64> {
    let mut freed_cr3s = Vec::new();
    let chosen_slot = {
        let mut table = SCHEDULER_LOCK.lock();
        reap_orphan_and_bg_zombies(&mut table, &mut freed_cr3s);
        let idx = table
            .iter()
            .position(|e| {
                e.slot == preferred_slot
                    && (e.state == TaskSlotState::Free || e.state == TaskSlotState::Exited)
                    && !is_slot_reclaim_pending(e.slot)
            })
            .or_else(|| {
                table.iter().position(|e| {
                    e.slot >= 6
                        && e.state == TaskSlotState::Free
                        && !is_slot_reclaim_pending(e.slot)
                })
            })
            .or_else(|| {
                table.iter().position(|e| {
                    e.slot == preferred_slot
                        && (e.state == TaskSlotState::Free || e.state == TaskSlotState::Exited)
                })
            })
            .or_else(|| table.iter().position(|e| e.slot >= 6 && e.state == TaskSlotState::Free));
        let Some(idx) = idx else {
            drop(table);
            free_reaped_cr3s(freed_cr3s);
            return None;
        };
        let slot = table[idx].slot;
        collect_cr3_for_free(table[idx].cr3_phys, &mut freed_cr3s);
        table[idx].cr3_phys = child_cr3;
        table[idx].state = TaskSlotState::Ready;
        table[idx].is_running = false;
        Some(slot)
    };
    free_reaped_cr3s(freed_cr3s);
    chosen_slot
}

/// Releases an exec slot reserved by `acquire_exec_slot` when ELF load fails.
pub fn abort_exec_slot(freed_cr3: u64) {
    if freed_cr3 == 0 {
        return;
    }
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| e.cr3_phys == freed_cr3) {
        if entry.slot >= 2 && entry.state == TaskSlotState::Ready {
            reset_slot_to_free(entry);
        } else {
            entry.cr3_phys = 0;
        }
    }
}

/// Looks up the scheduler slot bound to physical page-table root `cr3_phys`.
pub fn slot_for_cr3(cr3_phys: u64) -> Option<u64> {
    if cr3_phys == 0 {
        return None;
    }
    let table = SCHEDULER_LOCK.lock();
    table
        .iter()
        .find(|e| e.cr3_phys == cr3_phys && e.state != TaskSlotState::Free)
        .map(|e| e.slot)
}

#[derive(Debug, Clone, Copy)]
pub struct LiveCoreInfo {
    pub active_ticks: u64,
    pub idle_ticks: u64,
    pub irq_count: u64,
    pub apic_id: u32,
    pub online: u32,
}

impl LiveCoreInfo {
    pub const fn empty() -> Self {
        Self {
            active_ticks: 0,
            idle_ticks: 0,
            irq_count: 0,
            apic_id: 0,
            online: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LiveProcInfo {
    pub pid: u64,
    pub tid: u64,
    pub cr3_phys: u64,
    pub slot_id: u32,
    pub state: u32, // 0=Free, 1=Ready, 2=Running, 3=Blocked, 4=Zombie, 5=Exited
    pub user_pages: u32,
    pub ticks: u32,
    pub name: [u8; 16],
}

impl LiveProcInfo {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            tid: 0,
            cr3_phys: 0,
            slot_id: 0,
            state: 0,
            user_pages: 0,
            ticks: 0,
            name: [0; 16],
        }
    }
}

fn state_code(state: TaskSlotState) -> u32 {
    match state {
        TaskSlotState::Free => 0,
        TaskSlotState::Ready => 1,
        TaskSlotState::Running => 2,
        TaskSlotState::Blocked => 3,
        TaskSlotState::Zombie => 4,
        TaskSlotState::Exited => 5,
    }
}

fn copy_name_16(dst: &mut [u8; 16], src: &[u8]) {
    *dst = [0u8; 16];
    let n = src.len().min(16);
    dst[..n].copy_from_slice(&src[..n]);
}

/// Captures live hardware SMP core telemetry (`cores[0..8]`) and active process telemetry
/// (`procs[0..8]`) directly from `CPU_CONTROLS`, `SCHEDULER_LOCK`, and HHDM page tables.
pub fn collect_live_telemetry(
    cores_out: &mut [LiveCoreInfo; 8],
    procs_out: &mut [LiveProcInfo; 8],
) -> (u32, u32) {
    *cores_out = [LiveCoreInfo::empty(); 8];
    *procs_out = [LiveProcInfo::empty(); 8];

    let mut online_cores: u32 = 0;
    let max_cores = MAX_CPUS.min(8);
    for (idx, dst) in cores_out.iter_mut().enumerate().take(max_cores) {
        let ctl = &CPU_CONTROLS[idx];
        let online = ctl.online.load(Ordering::Acquire);
        if online {
            online_cores += 1;
        }
        *dst = LiveCoreInfo {
            active_ticks: ctl.compute_ticks.load(Ordering::Relaxed),
            idle_ticks: ctl.idle_ticks.load(Ordering::Relaxed),
            irq_count: ctl.irq_ticks.load(Ordering::Relaxed),
            apic_id: ctl.lapic_id.load(Ordering::Acquire) as u32,
            online: if online { 1 } else { 0 },
        };
    }

    let mut sampled: [Option<ProcessSlotDescriptor>; 8] = [None; 8];
    let mut count = 0usize;
    {
        let table = SCHEDULER_LOCK.lock();
        for entry in table.iter().take(MAX_PROCESSES) {
            if entry.state != TaskSlotState::Free && entry.pid != 0 {
                sampled[count] = Some(*entry);
                count += 1;
                if count >= 8 {
                    break;
                }
            }
        }
    }

    let master_cr3 = crate::mm::master_kernel_cr3();
    let cur_active_slot = super::active_slot();
    let cur_active_cr3 = crate::mm::active_cr3();

    for i in 0..count {
        if let Some(ref entry) = sampled[i] {
            let fallback_cr3 = match entry.slot {
                SLOT_SHELL => SHELL_CR3.load(Ordering::Acquire),
                SLOT_DAEMON => DAEMON_CR3.load(Ordering::Acquire),
                SLOT_CHILD_RUST => CHILD_RUST_CR3.load(Ordering::Acquire),
                SLOT_CHILD_CPP => CHILD_CPP_CR3.load(Ordering::Acquire),
                SLOT_DESKTOP_UI => DESKTOP_UI_CR3.load(Ordering::Acquire),
                _ => 0,
            };
            let mut cr3 = if entry.cr3_phys != 0 {
                entry.cr3_phys
            } else {
                fallback_cr3
            };
            if cr3 == 0 && entry.slot == cur_active_slot && cur_active_cr3 != master_cr3 {
                cr3 = cur_active_cr3;
            }
            if cr3 == 0 && entry.slot == SLOT_DAEMON {
                cr3 = master_cr3;
            }

            let tid = match entry.slot {
                SLOT_SHELL => SHELL_TID.load(Ordering::Relaxed),
                SLOT_DAEMON => DAEMON_TID.load(Ordering::Relaxed),
                SLOT_CHILD_RUST => CHILD_RUST_TID.load(Ordering::Relaxed),
                SLOT_CHILD_CPP => CHILD_CPP_TID.load(Ordering::Relaxed),
                SLOT_DESKTOP_UI => DESKTOP_UI_TID.load(Ordering::Relaxed),
                SLOT_BG_DOWNLOADER => super::BG_DOWNLOADER_TID.load(Ordering::Relaxed),
                _ => entry.pid.saturating_add(1),
            };

            let st = if entry.slot == SLOT_DAEMON {
                if DAEMON_ACTIVE.load(Ordering::Acquire) {
                    2
                } else {
                    5
                }
            } else if entry.slot == cur_active_slot && entry.state == TaskSlotState::Running {
                2
            } else {
                state_code(entry.state)
            };

            let user_pages = crate::mm::count_user_pages(cr3) as u32;
            let live_ticks = slot_live_ticks(entry.slot).max(entry.ticks) as u32;
            let mut name_buf = [0u8; 16];
            copy_name_16(&mut name_buf, entry.name_str().as_bytes());

            procs_out[i] = LiveProcInfo {
                pid: entry.pid,
                tid,
                cr3_phys: cr3,
                slot_id: entry.slot as u32,
                state: st,
                user_pages,
                ticks: live_ticks,
                name: name_buf,
            };
        }
    }

    let (httpd_active, httpd_pid, _) = bg_httpd_status();
    if httpd_active && httpd_pid != 0 && count < 8 {
        let mut name_buf = [0u8; 16];
        copy_name_16(&mut name_buf, b"bg_httpd");
        procs_out[count] = LiveProcInfo {
            pid: httpd_pid,
            tid: httpd_pid.saturating_add(1),
            cr3_phys: master_cr3,
            slot_id: 31,
            state: 2,
            user_pages: 0,
            ticks: CPU_CONTROLS[1].irq_ticks.load(Ordering::Relaxed) as u32,
            name: name_buf,
        };
        count += 1;
    }

    (online_cores.max(1), count as u32)
}

/// Returns a snapshot of live `cr3_phys` per scheduler slot (`0` when Free or Exited).
pub fn snapshot_slot_cr3s() -> [u64; MAX_PROCESSES] {
    let mut out = [0u64; MAX_PROCESSES];
    let table = SCHEDULER_LOCK.lock();
    let active_slot = super::active_slot();
    let active_cr3 = crate::mm::active_cr3();
    let master_cr3 = crate::mm::master_kernel_cr3();
    for (idx, entry) in table.iter().take(MAX_PROCESSES).enumerate() {
        if entry.state != TaskSlotState::Free
            && entry.state != TaskSlotState::Exited
            && entry.pid != 0
        {
            let mut cr3 = entry.cr3_phys;
            if cr3 == 0 && entry.slot == active_slot && active_cr3 != master_cr3 {
                cr3 = active_cr3;
            }
            out[idx] = cr3;
        }
    }
    out
}
