//! POSIX Signal Dispatch and Mask Management (`ostd::scheduler::signals`).

use core::sync::atomic::Ordering;
use super::slots::{
    TaskSlotState, SCHEDULER_LOCK, SLOT_SHELL, SLOT_DAEMON, SLOT_BG_DOWNLOADER,
};
use super::smp_idle::{CPU_CONTROLS, MAX_CPUS};
use super::{
    DAEMON_ACTIVE, DAEMON_PID, BG_DOWNLOADER_ACTIVE, BG_DOWNLOADER_PID, SHELL_PID,
};

/// Returns `Ok(old_handler)` or `Err(errno)`.
pub fn set_sigaction(pid: u64, signum: usize, handler: u64) -> Result<u64, i32> {
    if signum >= 32 {
        return Err(-22); // -EINVAL
    }
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free) {
        let old = entry.sig_handlers[signum];
        entry.sig_handlers[signum] = handler;
        Ok(old)
    } else {
        Err(-3) // -ESRCH
    }
}

/// Returns the current signal handler function address for `signum` in process `pid`.
pub fn get_sigaction(pid: u64, signum: usize) -> u64 {
    if signum >= 32 {
        return 0;
    }
    let table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter().find(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free) {
        entry.sig_handlers[signum]
    } else {
        0
    }
}

/// Updates the blocked signal mask for process `pid`.
/// how: 0 = SIG_BLOCK, 1 = SIG_UNBLOCK, 2 = SIG_SETMASK.
/// Returns `Ok(old_mask)` or `Err(errno)`.
pub fn set_sigprocmask(pid: u64, how: i32, set: u64) -> Result<u64, i32> {
    let mut table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter_mut().find(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free) {
        let old = entry.blocked_signals;
        let mut new_mask = match how {
            0 => old | set,        // SIG_BLOCK
            1 => old & !set,       // SIG_UNBLOCK
            2 => set,              // SIG_SETMASK
            _ => return Err(-22),  // -EINVAL
        };
        // SIGKILL (9) and SIGSTOP (19) cannot be blocked in POSIX
        new_mask &= !((1 << 9) | (1 << 19));
        entry.blocked_signals = new_mask;
        Ok(old)
    } else {
        Err(-3) // -ESRCH
    }
}

/// Returns the blocked signal mask for process `pid`.
pub fn get_sigprocmask(pid: u64) -> u64 {
    let table = SCHEDULER_LOCK.lock();
    if let Some(entry) = table.iter().find(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free) {
        entry.blocked_signals
    } else {
        0
    }
}

/// Sends signal `sig` to target process `pid`.
/// Returns `Ok(())` or `Err(errno)`.
pub fn send_signal(pid: u64, sig: usize) -> Result<(), i64> {
    if sig >= 32 {
        return Err(-22); // -EINVAL
    }
    let mut table = SCHEDULER_LOCK.lock();
    let idx = table.iter().position(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free && e.state != TaskSlotState::Exited);
    let Some(i) = idx else {
        let daemon_pid = DAEMON_PID.load(Ordering::Relaxed);
        let bg_pid = BG_DOWNLOADER_PID.load(Ordering::Relaxed);
        if pid == daemon_pid || pid == bg_pid {
            return Ok(());
        }
        return Err(-3); // -ESRCH
    };

    if sig == 0 {
        return Ok(()); // Existence check
    }

    let handler = table[i].sig_handlers[sig];
    let ppid = table[i].ppid;

    if sig == 9 /* SIGKILL */ || (sig == 15 /* SIGTERM */ && handler == 0) {
        table[i].state = TaskSlotState::Zombie;
        table[i].is_running = false;
        table[i].exit_code = (128 + sig as i32) << 8;
        if ppid > 0 {
            if let Some(parent) = table.iter_mut().find(|e| e.pid == ppid) {
                parent.pending_signals |= 1 << 17; // SIGCHLD
            }
        }
        return Ok(());
    }

    if handler != 0 {
        table[i].pending_signals |= 1 << sig;
        return Ok(());
    }

    // Default action without user handler
    if sig == 2 /* SIGINT */ || sig == 3 /* SIGQUIT */ || sig == 15 /* SIGTERM */ {
        table[i].state = TaskSlotState::Zombie;
        table[i].is_running = false;
        table[i].exit_code = (128 + sig as i32) << 8;
        if ppid > 0 {
            if let Some(parent) = table.iter_mut().find(|e| e.pid == ppid) {
                parent.pending_signals |= 1 << 17;
            }
        }
        return Ok(());
    }

    if sig == 17 /* SIGCHLD */ {
        // Default: ignore
        return Ok(());
    }

    table[i].pending_signals |= 1 << sig;
    Ok(())
}

/// Checks and pops the lowest pending unblocked signal for process `pid`.
/// Returns `Some((signum, handler))` or `None`.
pub fn pop_pending_signal(pid: u64) -> Option<(usize, u64)> {
    let mut table = SCHEDULER_LOCK.lock();
    let entry = table.iter_mut().find(|e| (e.pid == pid || (pid < 32 && e.slot == pid)) && e.state != TaskSlotState::Free)?;
    let deliverable = entry.pending_signals & !entry.blocked_signals;
    if deliverable == 0 {
        return None;
    }
    // Find lowest set bit
    let sig = deliverable.trailing_zeros() as usize;
    if sig >= 32 {
        return None;
    }
    entry.pending_signals &= !(1 << sig);
    let handler = entry.sig_handlers[sig];
    Some((sig, handler))
}

/// Collects an exited child process in `Zombie` state, recycles its slot to `Free`,

/// Signal 9 (`SIGKILL`) or 15 (`SIGTERM`) or any positive signal marks the target as stopped.
/// Returns `Ok(target_pid)` or `Err(errno)`.
pub fn kill_task(target: u64, sig: u64) -> Result<u64, i64> {
    let daemon_pid = DAEMON_PID.load(Ordering::Relaxed);
    let bg_pid = BG_DOWNLOADER_PID.load(Ordering::Relaxed);
    let shell_pid = SHELL_PID.load(Ordering::Relaxed);

    if target == shell_pid || target == SLOT_SHELL {
        return Err(-1); // EPERM (operation not permitted on init/shell)
    }

    if target == daemon_pid || target == SLOT_DAEMON {
        DAEMON_ACTIVE.store(false, Ordering::Release);
        if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
            table[SLOT_DAEMON as usize].is_running = false;
        }
        return Ok(daemon_pid);
    } else if target == bg_pid || target == SLOT_BG_DOWNLOADER {
        BG_DOWNLOADER_ACTIVE.store(false, Ordering::Release);
        if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
            table[SLOT_BG_DOWNLOADER as usize].is_running = false;
        }
        if MAX_CPUS > 1 {
            CPU_CONTROLS[1].active_proc_slot.store(SLOT_DAEMON, Ordering::Release);
        }
        return Ok(bg_pid);
    }

    let signal_num = if sig == 0 { 9 } else { sig as usize };
    send_signal(target, signal_num)?;
    Ok(target)
}

