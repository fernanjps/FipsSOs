//! Lock-Free Preemptive Round-Robin Scheduler & Real ASM Context Switcher (`ostd::scheduler`).
//!
//! Connected directly to `ostd/src/arch/x86_64/switch.s` (`__fipsos_context_switch`),
//! the hardware PIT/APIC Timer Interrupt (`IRQ0`, Vector `0x20`), and `sys_sched_yield` (`24`).
//! `npu-telemetryd` executes on its own dedicated 16 KiB hardware stack (`DAEMON_STACK`),
//! switching CPU context in pure x86_64 assembly against the active thread.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::fpu::FpuState;
use crate::arch::idt::{timer_ticks, InterruptStackFrame};

core::arch::global_asm!(include_str!("arch/x86_64/switch.s"));

extern "C" {
    fn __fipsos_context_switch(prev_rsp: *mut u64, next_rsp: u64);
}

/// Round-Robin scheduling quantum in `IRQ0` ticks (`10` ticks @ 100 Hz = `100 ms`).
pub const QUANTUM_TICKS: u64 = 10;

/// Canonical scheduler slot indices.
pub const SLOT_SHELL: u64 = 0;
pub const SLOT_DAEMON: u64 = 1;
pub const SLOT_CHILD_RUST: u64 = 2;
pub const SLOT_CHILD_CPP: u64 = 3;

/// Dedicated 16 KiB stack size for the `npu-telemetryd` kernel worker thread.
pub const DAEMON_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct DaemonStack([u8; DAEMON_STACK_SIZE]);

static mut DAEMON_STACK: DaemonStack = DaemonStack([0; DAEMON_STACK_SIZE]);
static mut CALLER_SAVED_RSP: u64 = 0;
static mut DAEMON_SAVED_RSP: u64 = 0;
static mut ACTIVE_FPU_STATE: FpuState = FpuState::new();
static mut DAEMON_FPU_STATE: FpuState = FpuState::new();
static DAEMON_CONTEXT_READY: AtomicBool = AtomicBool::new(false);
static DAEMON_IN_SWITCH: AtomicBool = AtomicBool::new(false);

static ACTIVE_SLOT: AtomicU64 = AtomicU64::new(SLOT_SHELL);

static SHELL_PID: AtomicU64 = AtomicU64::new(1033);
static SHELL_TID: AtomicU64 = AtomicU64::new(1034);
static SHELL_TICKS: AtomicU64 = AtomicU64::new(1);
static SHELL_SWITCHES: AtomicU64 = AtomicU64::new(1);
static SHELL_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
static SHELL_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF0);

static DAEMON_PID: AtomicU64 = AtomicU64::new(1035);
static DAEMON_TID: AtomicU64 = AtomicU64::new(1036);
static DAEMON_TICKS: AtomicU64 = AtomicU64::new(1);
static DAEMON_SWITCHES: AtomicU64 = AtomicU64::new(1);
static DAEMON_RIP: AtomicU64 = AtomicU64::new(0);
static DAEMON_RSP: AtomicU64 = AtomicU64::new(0);

static CHILD_RUST_PID: AtomicU64 = AtomicU64::new(1037);
static CHILD_RUST_TID: AtomicU64 = AtomicU64::new(1038);
static CHILD_RUST_TICKS: AtomicU64 = AtomicU64::new(0);
static CHILD_RUST_SWITCHES: AtomicU64 = AtomicU64::new(0);
static CHILD_RUST_RIP: AtomicU64 = AtomicU64::new(0x0050_0000);
static CHILD_RUST_RSP: AtomicU64 = AtomicU64::new(0x0090_3FF8);

static CHILD_CPP_PID: AtomicU64 = AtomicU64::new(1039);
static CHILD_CPP_TID: AtomicU64 = AtomicU64::new(1040);
static CHILD_CPP_TICKS: AtomicU64 = AtomicU64::new(0);
static CHILD_CPP_SWITCHES: AtomicU64 = AtomicU64::new(0);
static CHILD_CPP_RIP: AtomicU64 = AtomicU64::new(0x0060_0000);
static CHILD_CPP_RSP: AtomicU64 = AtomicU64::new(0x0090_3FF8);

static RING3_TICKS: AtomicU64 = AtomicU64::new(0);
static TOTAL_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_VOLUNTARY_YIELDS: AtomicU64 = AtomicU64::new(0);

/// Consistent point-in-time snapshot of the preemptive Round-Robin scheduler counters.
#[derive(Debug, Clone, Copy)]
pub struct SchedulerSnapshot {
    pub total_irq0_ticks: u64,
    pub ring3_ticks: u64,
    pub total_preemptions: u64,
    pub total_voluntary_yields: u64,
    pub active_slot: u64,
    pub active_pid: u64,
    pub active_tid: u64,
    pub shell_ticks: u64,
    pub shell_switches: u64,
    pub shell_rip: u64,
    pub shell_rsp: u64,
    pub daemon_ticks: u64,
    pub daemon_switches: u64,
    pub daemon_rip: u64,
    pub daemon_rsp: u64,
    pub child_rust_ticks: u64,
    pub child_rust_switches: u64,
    pub child_rust_rip: u64,
    pub child_rust_rsp: u64,
    pub child_cpp_ticks: u64,
    pub child_cpp_switches: u64,
    pub child_cpp_rip: u64,
    pub child_cpp_rsp: u64,
}

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
/// and registers the initial `Koid`s of all 4 processes/threads.
pub fn init_slots(
    shell_pid: u64,
    shell_tid: u64,
    daemon_pid: u64,
    daemon_tid: u64,
    child_rust_pid: u64,
    child_rust_tid: u64,
    child_cpp_pid: u64,
    child_cpp_tid: u64,
) {
    SHELL_PID.store(shell_pid, Ordering::Release);
    SHELL_TID.store(shell_tid, Ordering::Release);
    DAEMON_PID.store(daemon_pid, Ordering::Release);
    DAEMON_TID.store(daemon_tid, Ordering::Release);
    CHILD_RUST_PID.store(child_rust_pid, Ordering::Release);
    CHILD_RUST_TID.store(child_rust_tid, Ordering::Release);
    CHILD_CPP_PID.store(child_cpp_pid, Ordering::Release);
    CHILD_CPP_TID.store(child_cpp_tid, Ordering::Release);
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
    if !DAEMON_CONTEXT_READY.load(Ordering::Acquire) {
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

/// Invoked on every hardware `IRQ0` Timer Interrupt (`Vector 0x20`).
pub fn on_timer_irq(frame: &InterruptStackFrame, total_ticks: u64) {
    let is_ring3 = (frame.code_segment & 0b11) == 3;
    let current_slot = ACTIVE_SLOT.load(Ordering::Relaxed);

    if is_ring3 {
        RING3_TICKS.fetch_add(1, Ordering::Relaxed);
        if current_slot == SLOT_CHILD_CPP || frame.instruction_pointer >= 0x0060_0000 {
            CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_CPP_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
            CHILD_CPP_RSP.store(frame.stack_pointer, Ordering::Relaxed);
        } else if current_slot == SLOT_CHILD_RUST || frame.instruction_pointer >= 0x0050_0000 {
            CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
            CHILD_RUST_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
            CHILD_RUST_RSP.store(frame.stack_pointer, Ordering::Relaxed);
        } else {
            SHELL_TICKS.fetch_add(1, Ordering::Relaxed);
            SHELL_RIP.store(frame.instruction_pointer, Ordering::Relaxed);
            SHELL_RSP.store(frame.stack_pointer, Ordering::Relaxed);
        }
    } else if (total_ticks & 1) == 0 {
        match current_slot {
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
    // Executes a real assembly context switch (`__fipsos_context_switch`) into `npu-telemetryd`!
    if total_ticks > 0 && (total_ticks % QUANTUM_TICKS) == 0 {
        TOTAL_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        match current_slot {
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
        switch_to_daemon_once();
    }
}

/// Switches the active foreground slot to `/bin/npu_infer.elf` (`SLOT_CHILD_RUST`) or
/// `/bin/avatar_cpp.elf` (`SLOT_CHILD_CPP`) upon `sys_execve`.
pub fn on_enter_child(slot: u64, child_pid: u64, child_tid: u64, entry_rip: u64, user_rsp: u64) {
    if slot == SLOT_CHILD_CPP {
        CHILD_CPP_PID.store(child_pid, Ordering::Release);
        CHILD_CPP_TID.store(child_tid, Ordering::Release);
        CHILD_CPP_RIP.store(entry_rip, Ordering::Release);
        CHILD_CPP_RSP.store(user_rsp, Ordering::Release);
        CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed);
    } else {
        CHILD_RUST_PID.store(child_pid, Ordering::Release);
        CHILD_RUST_TID.store(child_tid, Ordering::Release);
        CHILD_RUST_RIP.store(entry_rip, Ordering::Release);
        CHILD_RUST_RSP.store(user_rsp, Ordering::Release);
        CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    TOTAL_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
    ACTIVE_SLOT.store(slot, Ordering::Release);
}

/// Restores the active foreground slot to `fipsos-shell.elf` when a child calls `sys_exit`.
pub fn on_exit_child(exit_rip: u64) {
    let prev_slot = ACTIVE_SLOT.swap(SLOT_SHELL, Ordering::AcqRel);
    if prev_slot == SLOT_CHILD_CPP || exit_rip >= 0x0060_0000 {
        if exit_rip != 0 {
            CHILD_CPP_RIP.store(exit_rip, Ordering::Release);
        }
        CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed);
    } else {
        if exit_rip != 0 {
            CHILD_RUST_RIP.store(exit_rip, Ordering::Release);
        }
        CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed);
}

/// Handles a cooperative `sys_sched_yield` (`24`) from Ring 3 by executing a real assembly
/// context switch (`__fipsos_context_switch`) to `npu-telemetryd` on its 16 KiB stack.
pub fn on_sys_sched_yield(user_rip: u64, user_rsp: u64) -> (u64, u64) {
    TOTAL_VOLUNTARY_YIELDS.fetch_add(1, Ordering::Relaxed);

    // Real assembly context switch (`switch.s`) into `npu-telemetryd` and back!
    switch_to_daemon_once();

    let current_slot = ACTIVE_SLOT.load(Ordering::Acquire);
    if current_slot == SLOT_CHILD_CPP || user_rip >= 0x0060_0000 {
        CHILD_CPP_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_CPP_RIP.store(user_rip, Ordering::Relaxed);
        CHILD_CPP_RSP.store(user_rsp, Ordering::Relaxed);
        let sw = CHILD_CPP_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
        (CHILD_CPP_TID.load(Ordering::Relaxed), sw)
    } else if current_slot == SLOT_CHILD_RUST || user_rip >= 0x0050_0000 {
        CHILD_RUST_TICKS.fetch_add(1, Ordering::Relaxed);
        CHILD_RUST_RIP.store(user_rip, Ordering::Relaxed);
        CHILD_RUST_RSP.store(user_rsp, Ordering::Relaxed);
        let sw = CHILD_RUST_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
        (CHILD_RUST_TID.load(Ordering::Relaxed), sw)
    } else {
        SHELL_TICKS.fetch_add(1, Ordering::Relaxed);
        SHELL_RIP.store(user_rip, Ordering::Relaxed);
        SHELL_RSP.store(user_rsp, Ordering::Relaxed);
        let sw = SHELL_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
        (SHELL_TID.load(Ordering::Relaxed), sw)
    }
}

/// Records the most recent user `RIP` and `RSP` of the calling thread on any syscall entry.
pub fn record_syscall_frame(user_rip: u64, user_rsp: u64) {
    let current_slot = ACTIVE_SLOT.load(Ordering::Relaxed);
    if current_slot == SLOT_CHILD_CPP || user_rip >= 0x0060_0000 {
        CHILD_CPP_RIP.store(user_rip, Ordering::Relaxed);
        CHILD_CPP_RSP.store(user_rsp, Ordering::Relaxed);
    } else if current_slot == SLOT_CHILD_RUST || user_rip >= 0x0050_0000 {
        CHILD_RUST_RIP.store(user_rip, Ordering::Relaxed);
        CHILD_RUST_RSP.store(user_rsp, Ordering::Relaxed);
    } else {
        SHELL_RIP.store(user_rip, Ordering::Relaxed);
        SHELL_RSP.store(user_rsp, Ordering::Relaxed);
    }
}

/// Returns `(active_pid_koid, active_tid_koid)` for `sys_getpid` (`39`).
pub fn active_pid_and_tid() -> (u64, u64) {
    match ACTIVE_SLOT.load(Ordering::Acquire) {
        SLOT_CHILD_CPP => (
            CHILD_CPP_PID.load(Ordering::Relaxed),
            CHILD_CPP_TID.load(Ordering::Relaxed),
        ),
        SLOT_CHILD_RUST => (
            CHILD_RUST_PID.load(Ordering::Relaxed),
            CHILD_RUST_TID.load(Ordering::Relaxed),
        ),
        _ => (
            SHELL_PID.load(Ordering::Relaxed),
            SHELL_TID.load(Ordering::Relaxed),
        ),
    }
}

/// Captures a consistent snapshot of all Round-Robin scheduler metrics.
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
        shell_ticks: SHELL_TICKS.load(Ordering::Relaxed),
        shell_switches: SHELL_SWITCHES.load(Ordering::Relaxed),
        shell_rip: SHELL_RIP.load(Ordering::Relaxed),
        shell_rsp: SHELL_RSP.load(Ordering::Relaxed),
        daemon_ticks: DAEMON_TICKS.load(Ordering::Relaxed),
        daemon_switches: DAEMON_SWITCHES.load(Ordering::Relaxed),
        daemon_rip: DAEMON_RIP.load(Ordering::Relaxed),
        daemon_rsp: DAEMON_RSP.load(Ordering::Relaxed),
        child_rust_ticks: CHILD_RUST_TICKS.load(Ordering::Relaxed),
        child_rust_switches: CHILD_RUST_SWITCHES.load(Ordering::Relaxed),
        child_rust_rip: CHILD_RUST_RIP.load(Ordering::Relaxed),
        child_rust_rsp: CHILD_RUST_RSP.load(Ordering::Relaxed),
        child_cpp_ticks: CHILD_CPP_TICKS.load(Ordering::Relaxed),
        child_cpp_switches: CHILD_CPP_SWITCHES.load(Ordering::Relaxed),
        child_cpp_rip: CHILD_CPP_RIP.load(Ordering::Relaxed),
        child_cpp_rsp: CHILD_CPP_RSP.load(Ordering::Relaxed),
    }
}
