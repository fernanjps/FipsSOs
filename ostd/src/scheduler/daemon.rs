//! Dedicated Stack and Execution Loop for npu-telemetryd (`ostd::scheduler::daemon`).

use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::fpu::FpuState;
use crate::arch::idt::timer_ticks;
use super::__fipsos_context_switch;
use super::slots::*;
use super::*;

/// Dedicated 16 KiB stack size for the `npu-telemetryd` kernel worker thread.
pub const DAEMON_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct DaemonStack([u8; DAEMON_STACK_SIZE]);

static mut DAEMON_STACK: DaemonStack = DaemonStack([0; DAEMON_STACK_SIZE]);
pub(crate) static mut CALLER_SAVED_RSP: u64 = 0;
pub(crate) static mut DAEMON_SAVED_RSP: u64 = 0;
pub(crate) static mut ACTIVE_FPU_STATE: FpuState = FpuState::new();
pub(crate) static mut DAEMON_FPU_STATE: FpuState = FpuState::new();
pub(crate) static DAEMON_CONTEXT_READY: AtomicBool = AtomicBool::new(false);
pub(crate) static DAEMON_IN_SWITCH: AtomicBool = AtomicBool::new(false);


/// Real entry point of the `npu-telemetryd` worker thread executing on its dedicated 16 KiB `DAEMON_STACK`.
///
/// Reached via `__fipsos_context_switch` (`switch.s`). On each quantum/yield pass it samples its
/// real hardware `RIP` and `RSP`, updates the telemetry counters and `/dev/fb0` visual indicator,
/// and switches hardware `RSP` back to `CALLER_SAVED_RSP` via `__fipsos_context_switch`.
extern "C" fn npu_telemetry_daemon_entry() -> ! {
    loop {
        let current_rip: u64;
        let current_rsp: u64;
        // SAFETY: Sampling real hardware RIP and RSP on `DAEMON_STACK` in Ring 0.
        unsafe {
            core::arch::asm!(
                "lea {}, [rip]",
                "mov {}, rsp",
                out(reg) current_rip,
                out(reg) current_rsp,
                options(nomem, nostack, preserves_flags)
            );
        }

        DAEMON_RIP.store(current_rip, Ordering::Relaxed);
        DAEMON_RSP.store(current_rsp, Ordering::Relaxed);
        let ticks = DAEMON_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        let sw = DAEMON_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;

        crate::fb_console::draw_scheduler_telemetry_pulse(timer_ticks(), sw + ticks);

        // SAFETY: Preserving the daemon's SIMD/FPU context (`FXSAVE64`) and switching hardware `RSP`
        // from `DAEMON_STACK` back to the caller's stack using `__fipsos_context_switch` in `switch.s`.
        unsafe {
            let daemon_fpu_ptr = &raw mut DAEMON_FPU_STATE;
            (*daemon_fpu_ptr).save();

            let daemon_rsp_slot = &raw mut DAEMON_SAVED_RSP;
            let caller_rsp = core::ptr::read_volatile(&raw const CALLER_SAVED_RSP);
            __fipsos_context_switch(daemon_rsp_slot, caller_rsp);
        }
    }
}

/// Returns the initial entry point address and stack top of `npu-telemetryd`.
pub fn daemon_entry_and_stack_top() -> (u64, u64) {
    let entry = npu_telemetry_daemon_entry as *const () as u64;
    // SAFETY: Computing the top virtual address of the 16 KiB static `DAEMON_STACK`.
    let stack_top = unsafe { (&raw const DAEMON_STACK).cast::<u8>().add(DAEMON_STACK_SIZE) as u64 };
    (entry, stack_top)
}

/// Initializes the 16 KiB `DAEMON_STACK` with a synthetic `__fipsos_context_switch` frame
/// (`r15`, `r14`, `r13`, `r12`, `rbx`, `rbp`, `rflags=0x0002`, `rip=npu_telemetry_daemon_entry`)
/// and registers the initial `Koid`s of all 5 processes/threads.
#[allow(clippy::too_many_arguments)]
pub fn init_slots(
    shell_pid: u64,
    shell_tid: u64,
    daemon_pid: u64,
    daemon_tid: u64,
    child_rust_pid: u64,
    child_rust_tid: u64,
    child_cpp_pid: u64,
    child_cpp_tid: u64,
    desktop_ui_pid: u64,
    desktop_ui_tid: u64,
) {
    SHELL_PID.store(shell_pid, Ordering::Release);
    SHELL_TID.store(shell_tid, Ordering::Release);
    DAEMON_PID.store(daemon_pid, Ordering::Release);
    DAEMON_TID.store(daemon_tid, Ordering::Release);
    CHILD_RUST_PID.store(child_rust_pid, Ordering::Release);
    CHILD_RUST_TID.store(child_rust_tid, Ordering::Release);
    CHILD_CPP_PID.store(child_cpp_pid, Ordering::Release);
    CHILD_CPP_TID.store(child_cpp_tid, Ordering::Release);
    DESKTOP_UI_PID.store(desktop_ui_pid, Ordering::Release);
    DESKTOP_UI_TID.store(desktop_ui_tid, Ordering::Release);
    PARENT_SLOT_DEPTH.store(0, Ordering::Release);
    ACTIVE_SLOT.store(SLOT_SHELL, Ordering::Release);

    // SAFETY: Preparing initial context-switch frame at the top of `DAEMON_STACK` and seeding
    // the 512-byte `FXSAVE64` state buffers (`ACTIVE_FPU_STATE` and `DAEMON_FPU_STATE`).
    unsafe {
        let active_fpu_ptr = &raw mut ACTIVE_FPU_STATE;
        let daemon_fpu_ptr = &raw mut DAEMON_FPU_STATE;
        (*active_fpu_ptr).save();
        (*daemon_fpu_ptr).save();

        let stack_top = (&raw mut DAEMON_STACK)
            .cast::<u8>()
            .add(DAEMON_STACK_SIZE)
            .cast::<u64>();

        let entry_addr = npu_telemetry_daemon_entry as *const () as u64;
        // Frame layout expected by `__fipsos_context_switch` before `ret`:
        // [rsp + 56] = return RIP (npu_telemetry_daemon_entry)
        // [rsp + 48] = RFLAGS (0x0002: reserved bit 1 set, IF=0 during brief stack switch)
        // [rsp + 40] = RBP (0)
        // [rsp + 32] = RBX (0)
        // [rsp + 24] = R12 (0)
        // [rsp + 16] = R13 (0)
        // [rsp + 8]  = R14 (0)
        // [rsp + 0]  = R15 (0)
        core::ptr::write_volatile(stack_top.sub(1), entry_addr);
        core::ptr::write_volatile(stack_top.sub(2), 0x0002u64);
        for offset in 3..=8 {
            core::ptr::write_volatile(stack_top.sub(offset), 0u64);
        }

        let initial_rsp = stack_top.sub(8) as u64;
        core::ptr::write_volatile(&raw mut DAEMON_SAVED_RSP, initial_rsp);
        DAEMON_RIP.store(entry_addr, Ordering::Release);
        DAEMON_RSP.store(initial_rsp, Ordering::Release);
    }

    DAEMON_CONTEXT_READY.store(true, Ordering::Release);

    // Execute one initial hardware ASM context switch into `npu-telemetryd` to verify `switch.s` + `FXSAVE64` at boot!
    switch_to_daemon_once();
}

/// Performs a real assembly context switch (`__fipsos_context_switch` in `switch.s`) from the
/// current stack into `npu-telemetryd`'s 16 KiB `DAEMON_STACK` and back, preserving and restoring
/// 512-byte `FXSAVE64` / `FXRSTOR64` x87 + `XMM0..XMM15` states on both sides.
#[inline]
pub fn switch_to_daemon_once() {
    if !DAEMON_CONTEXT_READY.load(Ordering::Acquire) || !DAEMON_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    // In multi-core SMP mode, Core 1 executes npu-telemetryd concurrently!
    if online_cpu_count() > 1 {
        return;
    }
    if DAEMON_IN_SWITCH
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    // SAFETY: Saving the active thread's `FpuState` (`FXSAVE64`), restoring `DAEMON_FPU_STATE`,
    // switching stacks via `switch.s`, and restoring the active thread's `FpuState` (`FXRSTOR64`)
    // upon return so Ring 3 C++20 `XMM0..XMM15` registers are never clobbered.
    unsafe {
        let active_fpu_ptr = &raw mut ACTIVE_FPU_STATE;
        let daemon_fpu_ptr = &raw const DAEMON_FPU_STATE;
        (*active_fpu_ptr).save();
        (*daemon_fpu_ptr).restore();

        let caller_rsp_slot = &raw mut CALLER_SAVED_RSP;
        let daemon_rsp = core::ptr::read_volatile(&raw const DAEMON_SAVED_RSP);
        __fipsos_context_switch(caller_rsp_slot, daemon_rsp);

        let active_fpu_restore = &raw const ACTIVE_FPU_STATE;
        (*active_fpu_restore).restore();
    }

    DAEMON_IN_SWITCH.store(false, Ordering::Release);
}

/// Returns whether `npu-telemetryd` is active.
pub fn is_daemon_active() -> bool {
    DAEMON_ACTIVE.load(Ordering::Acquire)
}

/// Enables or disables `npu-telemetryd`.
pub fn set_daemon_active(active: bool) {
    DAEMON_ACTIVE.store(active, Ordering::Release);
}

/// Returns whether the background downloader task is active.
pub fn is_bg_downloader_active() -> bool {
    BG_DOWNLOADER_ACTIVE.load(Ordering::Acquire)
}

/// Spawns or reactivates the background downloader task (`SLOT_BG_DOWNLOADER`).
/// Returns `(pid, tid)`.
pub fn spawn_background_downloader() -> (u64, u64) {
    let pid = BG_DOWNLOADER_PID.load(Ordering::Acquire);
    let tid = BG_DOWNLOADER_TID.load(Ordering::Acquire);
    BG_DOWNLOADER_ACTIVE.store(true, Ordering::Release);
    BG_DOWNLOADER_SWITCHES.fetch_add(1, Ordering::Relaxed);
    if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
        table[SLOT_BG_DOWNLOADER as usize].is_running = true;
        table[SLOT_BG_DOWNLOADER as usize].assigned_cpu = 1;
    }
    if MAX_CPUS > 1 {
        CPU_CONTROLS[1].active_proc_slot.store(SLOT_BG_DOWNLOADER, Ordering::Release);
    }
    (pid, tid)
}


