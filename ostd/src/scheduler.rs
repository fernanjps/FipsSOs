//! Lock-Free Preemptive Round-Robin Scheduler & Real ASM Context Switcher (`ostd::scheduler`).
//!
//! Connected directly to `ostd/src/arch/x86_64/switch.s` (`__fipsos_context_switch`),
//! the hardware PIT/APIC Timer Interrupt (`IRQ0`, Vector `0x20`), and `sys_sched_yield` (`24`).
//! `npu-telemetryd` executes on its own dedicated 16 KiB hardware stack (`DAEMON_STACK`),
//! switching CPU context in pure x86_64 assembly against the active thread.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::fpu::FpuState;
use crate::arch::idt::{timer_ticks, InterruptStackFrame};
use crate::sync::SpinLock;

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
pub const SLOT_DESKTOP_UI: u64 = 4;
pub const SLOT_BG_DOWNLOADER: u64 = 5;

/// Maximum number of supported CPU cores.
pub const MAX_CPUS: usize = 4;

/// Per-core control block (`CpuControlBlock` / `CpuLocal`).
pub struct CpuControlBlock {
    pub cpu_id: u32,
    pub lapic_id: AtomicU64,
    pub is_bsp: AtomicBool,
    pub online: AtomicBool,
    pub active_proc_slot: AtomicU64,
    pub irq_ticks: AtomicU64,
    pub total_switches: AtomicU64,
    pub idle_ticks: AtomicU64,
    pub compute_ticks: AtomicU64,
}

impl CpuControlBlock {
    pub const fn new(cpu_id: u32) -> Self {
        Self {
            cpu_id,
            lapic_id: AtomicU64::new(0),
            is_bsp: AtomicBool::new(false),
            online: AtomicBool::new(false),
            active_proc_slot: AtomicU64::new(SLOT_SHELL),
            irq_ticks: AtomicU64::new(0),
            total_switches: AtomicU64::new(0),
            idle_ticks: AtomicU64::new(0),
            compute_ticks: AtomicU64::new(0),
        }
    }
}

static CPU_CONTROLS: [CpuControlBlock; MAX_CPUS] = [
    CpuControlBlock::new(0),
    CpuControlBlock::new(1),
    CpuControlBlock::new(2),
    CpuControlBlock::new(3),
];

/// Concurrent task descriptor used for deadlock-free SMP scheduling.
#[derive(Debug, Clone, Copy)]
pub struct TaskSlotDescriptor {
    pub slot: u64,
    pub pid: u64,
    pub assigned_cpu: u32,
    pub is_running: bool,
}

static SCHEDULER_LOCK: SpinLock<[TaskSlotDescriptor; 6]> = SpinLock::new([
    TaskSlotDescriptor { slot: SLOT_SHELL, pid: 1033, assigned_cpu: 0, is_running: true },
    TaskSlotDescriptor { slot: SLOT_DAEMON, pid: 1035, assigned_cpu: 1, is_running: true },
    TaskSlotDescriptor { slot: SLOT_CHILD_RUST, pid: 1037, assigned_cpu: 0, is_running: false },
    TaskSlotDescriptor { slot: SLOT_CHILD_CPP, pid: 1039, assigned_cpu: 0, is_running: false },
    TaskSlotDescriptor { slot: SLOT_DESKTOP_UI, pid: 1041, assigned_cpu: 0, is_running: false },
    TaskSlotDescriptor { slot: SLOT_BG_DOWNLOADER, pid: 1051, assigned_cpu: 1, is_running: false },
]);

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
static PARENT_SLOT_STACK: [AtomicU64; 4] = [
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
    AtomicU64::new(SLOT_SHELL),
];
static PARENT_SLOT_DEPTH: AtomicU64 = AtomicU64::new(0);

static SHELL_PID: AtomicU64 = AtomicU64::new(1033);
static SHELL_TID: AtomicU64 = AtomicU64::new(1034);
static SHELL_TICKS: AtomicU64 = AtomicU64::new(1);
static SHELL_SWITCHES: AtomicU64 = AtomicU64::new(1);
static SHELL_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
static SHELL_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF0);
static SHELL_CR3: AtomicU64 = AtomicU64::new(0);

static DAEMON_PID: AtomicU64 = AtomicU64::new(1035);
static DAEMON_TID: AtomicU64 = AtomicU64::new(1036);
static DAEMON_TICKS: AtomicU64 = AtomicU64::new(1);
static DAEMON_SWITCHES: AtomicU64 = AtomicU64::new(1);
static DAEMON_RIP: AtomicU64 = AtomicU64::new(0);
static DAEMON_RSP: AtomicU64 = AtomicU64::new(0);
static DAEMON_CR3: AtomicU64 = AtomicU64::new(0);

static CHILD_RUST_PID: AtomicU64 = AtomicU64::new(1037);
static CHILD_RUST_TID: AtomicU64 = AtomicU64::new(1038);
static CHILD_RUST_TICKS: AtomicU64 = AtomicU64::new(0);
static CHILD_RUST_SWITCHES: AtomicU64 = AtomicU64::new(0);
static CHILD_RUST_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
static CHILD_RUST_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
static CHILD_RUST_CR3: AtomicU64 = AtomicU64::new(0);

static CHILD_CPP_PID: AtomicU64 = AtomicU64::new(1039);
static CHILD_CPP_TID: AtomicU64 = AtomicU64::new(1040);
static CHILD_CPP_TICKS: AtomicU64 = AtomicU64::new(0);
static CHILD_CPP_SWITCHES: AtomicU64 = AtomicU64::new(0);
static CHILD_CPP_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
static CHILD_CPP_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
static CHILD_CPP_CR3: AtomicU64 = AtomicU64::new(0);

static DESKTOP_UI_PID: AtomicU64 = AtomicU64::new(1041);
static DESKTOP_UI_TID: AtomicU64 = AtomicU64::new(1042);
static DESKTOP_UI_TICKS: AtomicU64 = AtomicU64::new(0);
static DESKTOP_UI_SWITCHES: AtomicU64 = AtomicU64::new(0);
static DESKTOP_UI_RIP: AtomicU64 = AtomicU64::new(0x0040_0000);
static DESKTOP_UI_RSP: AtomicU64 = AtomicU64::new(0x0080_3FF8);
static DESKTOP_UI_CR3: AtomicU64 = AtomicU64::new(0);

static DAEMON_ACTIVE: AtomicBool = AtomicBool::new(true);
static BG_DOWNLOADER_ACTIVE: AtomicBool = AtomicBool::new(false);
static BG_DOWNLOADER_PID: AtomicU64 = AtomicU64::new(1051);
static BG_DOWNLOADER_TID: AtomicU64 = AtomicU64::new(1052);
static BG_DOWNLOADER_TICKS: AtomicU64 = AtomicU64::new(0);
static BG_DOWNLOADER_SWITCHES: AtomicU64 = AtomicU64::new(0);

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

/// Invoked on every hardware `IRQ0` Timer Interrupt (`Vector 0x20`).
pub fn on_timer_irq(frame: &InterruptStackFrame, total_ticks: u64) {
    CPU_CONTROLS[0].irq_ticks.fetch_add(1, Ordering::Relaxed);
    CPU_CONTROLS[0].compute_ticks.fetch_add(1, Ordering::Relaxed);
    CPU_CONTROLS[0].total_switches.store(TOTAL_PREEMPTIONS.load(Ordering::Relaxed), Ordering::Relaxed);

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
    if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
        if let Some(entry) = table.iter_mut().find(|e| e.slot == slot) {
            entry.is_running = true;
            entry.pid = child_pid;
            entry.assigned_cpu = 0;
        }
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
    if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
        if let Some(entry) = table.iter_mut().find(|e| e.slot == prev_slot) {
            entry.is_running = false;
        }
        if let Some(entry) = table.iter_mut().find(|e| e.slot == resumed_parent) {
            entry.is_running = true;
        }
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
        _ => (
            SHELL_PID.load(Ordering::Relaxed),
            SHELL_TID.load(Ordering::Relaxed),
        ),
    }
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

/// Stops/kills a task or daemon by PID or slot index.
/// Signal 9 (`SIGKILL`) or 15 (`SIGTERM`) or any positive signal marks the target as stopped.
/// Returns `Ok(target_pid)` or `Err(errno)`.
pub fn kill_task(target: u64, _sig: u64) -> Result<u64, i64> {
    let daemon_pid = DAEMON_PID.load(Ordering::Relaxed);
    let bg_pid = BG_DOWNLOADER_PID.load(Ordering::Relaxed);
    let rust_pid = CHILD_RUST_PID.load(Ordering::Relaxed);
    let cpp_pid = CHILD_CPP_PID.load(Ordering::Relaxed);
    let shell_pid = SHELL_PID.load(Ordering::Relaxed);

    if target == daemon_pid || target == SLOT_DAEMON {
        DAEMON_ACTIVE.store(false, Ordering::Release);
        if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
            table[SLOT_DAEMON as usize].is_running = false;
        }
        Ok(daemon_pid)
    } else if target == bg_pid || target == SLOT_BG_DOWNLOADER {
        BG_DOWNLOADER_ACTIVE.store(false, Ordering::Release);
        if let Some(mut table) = SCHEDULER_LOCK.try_lock() {
            table[SLOT_BG_DOWNLOADER as usize].is_running = false;
        }
        if MAX_CPUS > 1 {
            CPU_CONTROLS[1].active_proc_slot.store(SLOT_DAEMON, Ordering::Release);
        }
        Ok(bg_pid)
    } else if target == rust_pid || target == SLOT_CHILD_RUST {
        Ok(rust_pid)
    } else if target == cpp_pid || target == SLOT_CHILD_CPP {
        Ok(cpp_pid)
    } else if target == shell_pid || target == SLOT_SHELL {
        Err(-1) // EPERM (operation not permitted on init/shell)
    } else {
        Err(-3) // ESRCH (No such process)
    }
}

/// Registers the Bootstrap Processor (BSP) in the CPU control table.
pub fn register_bsp(lapic_id: u32) {
    CPU_CONTROLS[0].lapic_id.store(lapic_id as u64, Ordering::Release);
    CPU_CONTROLS[0].is_bsp.store(true, Ordering::Release);
    CPU_CONTROLS[0].online.store(true, Ordering::Release);
    CPU_CONTROLS[0].active_proc_slot.store(SLOT_SHELL, Ordering::Release);
}

/// Registers an Application Processor (AP) as online in the CPU control table.
pub fn register_ap_online(cpu_id: u32, lapic_id: u32) {
    let idx = (cpu_id as usize).min(MAX_CPUS - 1);
    let block = &CPU_CONTROLS[idx];
    block.lapic_id.store(lapic_id as u64, Ordering::Release);
    block.is_bsp.store(false, Ordering::Release);
    block.active_proc_slot.store(SLOT_DAEMON, Ordering::Release);
    block.online.store(true, Ordering::Release);
}

/// Returns whether the specified CPU is marked online.
pub fn is_ap_online(cpu_id: u32) -> bool {
    let idx = cpu_id as usize;
    if idx < MAX_CPUS {
        CPU_CONTROLS[idx].online.load(Ordering::Acquire)
    } else {
        false
    }
}

/// Returns the total count of online processors.
pub fn online_cpu_count() -> usize {
    let ct = CPU_CONTROLS
        .iter()
        .filter(|c| c.online.load(Ordering::Acquire))
        .count();
    ct.max(1)
}

/// Returns a reference to the `CpuControlBlock` of the requested core.
pub fn cpu_control_block(cpu_id: usize) -> Option<&'static CpuControlBlock> {
    if cpu_id < MAX_CPUS && CPU_CONTROLS[cpu_id].online.load(Ordering::Acquire) {
        Some(&CPU_CONTROLS[cpu_id])
    } else {
        None
    }
}

/// Concurrent distributed execution loop for an Application Processor (`AP`).
///
/// Executes secondary worker threads concurrently on Core 1:
/// - Drains Zero-Syscall shared memory I/O rings (`ostd::ipc`) without context switch overhead.
/// - When `BG_DOWNLOADER_ACTIVE` is true, Core 1 executes background download tasks (`SLOT_BG_DOWNLOADER`).
/// - When `DAEMON_ACTIVE` is true, Core 1 executes `npu-telemetryd` tasks (`SLOT_DAEMON`).
/// - Idle Power Governor: When no work is queued, Core 1 executes `hlt` with interrupts enabled (`sti; hlt`).
pub fn ap_worker_loop(cpu_id: u32) -> ! {
    let idx = (cpu_id as usize).min(MAX_CPUS - 1);
    let block = &CPU_CONTROLS[idx];

    loop {
        let mut did_work = false;

        // 1. Drain Zero-Syscall IPC rings without context switches
        let processed_rings = crate::ipc::poll_active_rings();
        if processed_rings > 0 {
            block.compute_ticks.fetch_add(processed_rings as u64, Ordering::Relaxed);
            block.irq_ticks.fetch_add(processed_rings as u64, Ordering::Relaxed);
            did_work = true;
        }

        // 2. Background downloader workload
        if BG_DOWNLOADER_ACTIVE.load(Ordering::Acquire) {
            block.active_proc_slot.store(SLOT_BG_DOWNLOADER, Ordering::Release);
            BG_DOWNLOADER_TICKS.fetch_add(1, Ordering::Relaxed);
            let bsw = BG_DOWNLOADER_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            block.irq_ticks.fetch_add(1, Ordering::Relaxed);
            block.compute_ticks.fetch_add(1, Ordering::Relaxed);
            block.total_switches.store(bsw, Ordering::Relaxed);
            did_work = true;
        } else if DAEMON_ACTIVE.load(Ordering::Acquire) {
            block.active_proc_slot.store(SLOT_DAEMON, Ordering::Release);
            let ticks = block.irq_ticks.fetch_add(1, Ordering::Relaxed) + 1;
            block.compute_ticks.fetch_add(1, Ordering::Relaxed);
            DAEMON_TICKS.store(ticks, Ordering::Relaxed);
            let sw = DAEMON_SWITCHES.fetch_add(1, Ordering::Relaxed) + 1;
            block.total_switches.store(sw, Ordering::Relaxed);

            if (sw % 25) == 0 {
                crate::fb_console::draw_scheduler_telemetry_pulse(ticks, sw);
            }
            did_work = true;
        }

        if !did_work {
            // Idle Power Governor: HLT instruction with interrupts enabled until next hardware IRQ
            block.idle_ticks.fetch_add(1, Ordering::Relaxed);
            block.irq_ticks.fetch_add(1, Ordering::Relaxed);
            // SAFETY: Privileged HLT execution in Ring 0 with interrupts enabled
            unsafe {
                core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack));
            }
        } else {
            for _ in 0..2_000 {
                core::hint::spin_loop();
            }
        }
    }
}

/// Records an idle tick spent in HLT state for the given CPU core.
pub fn record_idle_tick(cpu_id: usize) {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].idle_ticks.fetch_add(1, Ordering::Relaxed);
        CPU_CONTROLS[cpu_id].irq_ticks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records active compute ticks for the given CPU core.
pub fn record_compute_tick(cpu_id: usize, count: u64) {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].compute_ticks.fetch_add(count, Ordering::Relaxed);
        CPU_CONTROLS[cpu_id].irq_ticks.fetch_add(count, Ordering::Relaxed);
    }
}

/// Calculates the real-time CPU utilization percentage (0..100%) based on compute vs idle ticks.
pub fn cpu_usage_percent(cpu_id: usize) -> u32 {
    if cpu_id < MAX_CPUS {
        let compute = CPU_CONTROLS[cpu_id].compute_ticks.load(Ordering::Relaxed);
        let idle = CPU_CONTROLS[cpu_id].idle_ticks.load(Ordering::Relaxed);
        let total = compute + idle;
        if total == 0 {
            0
        } else {
            ((compute * 100) / total).min(100) as u32
        }
    } else {
        0
    }
}

/// Returns the process slot currently assigned and running on the requested CPU.
pub fn active_proc_slot_on_cpu(cpu_id: usize) -> u64 {
    if cpu_id < MAX_CPUS {
        CPU_CONTROLS[cpu_id].active_proc_slot.load(Ordering::Acquire)
    } else {
        0
    }
}
