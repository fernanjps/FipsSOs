#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use ostd::sync::SpinLock;

use crate::handle::{HandleError, HandleId, HandleTable, Rights};
use crate::object::{KernelObject, Koid, ObjectType};
use crate::thread::{Thread, ThreadState};

/// Execution lifecycle state of a FipSOs user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process object created and ELF segments loaded, not yet entered Ring 3.
    Created,
    /// Actively executing instructions in Ring 3 (`CPL=3`).
    RunningRing3,
    /// Blocked in Ring 0 waiting on a synchronous I/O or IPC syscall.
    BlockedOnSyscall,
    /// Terminated cleanly via `sys_exit(code)`.
    Exited(i32),
}

impl fmt::Display for ProcessState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => f.write_str("Created"),
            Self::RunningRing3 => f.write_str("RunningRing3"),
            Self::BlockedOnSyscall => f.write_str("BlockedOnSyscall"),
            Self::Exited(code) => write!(f, "Exited({})", code),
        }
    }
}

/// Zircon-style capability-isolated Process container owning a private `HandleTable`.
pub struct Process {
    koid: Koid,
    name: String,
    entry_point: u64,
    state: SpinLock<ProcessState>,
    handle_table: SpinLock<HandleTable>,
}

impl Process {
    /// Creates a new `Process` kernel object wrapped in an `Arc`.
    pub fn new(name: &str, entry_point: u64) -> Arc<Self> {
        Arc::new(Self {
            koid: Koid::allocate(),
            name: String::from(name),
            entry_point,
            state: SpinLock::new(ProcessState::Created),
            handle_table: SpinLock::new(HandleTable::new()),
        })
    }

    /// Returns the human-readable name of this process.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the Ring 3 virtual entry point (`RIP`) of this process.
    #[inline]
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Returns the current `ProcessState`.
    #[inline]
    pub fn state(&self) -> ProcessState {
        *self.state.lock()
    }

    /// Updates the current `ProcessState`.
    #[inline]
    pub fn set_state(&self, new_state: ProcessState) {
        *self.state.lock() = new_state;
    }

    /// Inserts a capability `KernelObject` with the given `Rights` into this process's `HandleTable`.
    pub fn insert_handle(&self, object: Arc<dyn KernelObject>, rights: Rights) -> HandleId {
        self.handle_table.lock().insert(object, rights)
    }

    /// Verifies that `handle` exists in this process's `HandleTable` with `required_rights`
    /// and returns the referenced `Koid`.
    pub fn verify_handle(&self, handle: HandleId, required_rights: Rights) -> Result<Koid, HandleError> {
        let guard = self.handle_table.lock();
        let entry = guard.get(handle, required_rights)?;
        Ok(entry.koid())
    }

    /// Returns the number of capability handles owned by this process.
    pub fn handle_count(&self) -> usize {
        self.handle_table.lock().len()
    }
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Process")
            .field("koid", &self.koid)
            .field("name", &self.name)
            .field("entry_point", &self.entry_point)
            .field("state", &self.state())
            .field("handles", &self.handle_count())
            .finish()
    }
}

impl KernelObject for Process {
    fn koid(&self) -> Koid {
        self.koid
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Process
    }

    fn as_process(&self) -> Option<&Process> {
        Some(self)
    }
}

/// Paired `Process` and `Thread` entry tracked by the global `SchedulerRegistry`.
#[derive(Clone)]
pub struct ProcessThreadRecord {
    pub slot: u64,
    pub process: Arc<Process>,
    pub thread: Arc<Thread>,
}

struct RegistryState {
    entries: Vec<ProcessThreadRecord>,
}

static SCHEDULER_REGISTRY: SpinLock<RegistryState> = SpinLock::new(RegistryState {
    entries: Vec::new(),
});

/// Global safe capability registry of `Process` and `Thread` objects scheduled on FipSOs.
pub struct SchedulerRegistry;

impl SchedulerRegistry {
    /// Registers the initial Round-Robin processes and threads (`fipsos-shell.elf`,
    /// `npu-telemetryd`, `/bin/npu_infer.elf`, and `/bin/avatar_cpp.elf`) and initializes
    /// the real 16 KiB ASM context-switch stack (`switch.s`) in `ostd::scheduler`.
    pub fn register_boot_entries(
        shell_proc: Arc<Process>,
        shell_thread: Arc<Thread>,
        daemon_proc: Arc<Process>,
        daemon_thread: Arc<Thread>,
        infer_rust_proc: Arc<Process>,
        infer_rust_thread: Arc<Thread>,
        avatar_cpp_proc: Arc<Process>,
        avatar_cpp_thread: Arc<Thread>,
    ) {
        shell_proc.set_state(ProcessState::RunningRing3);
        shell_thread.set_state(ThreadState::Running);

        daemon_proc.set_state(ProcessState::RunningRing3);
        daemon_thread.set_state(ThreadState::Ready);

        infer_rust_proc.set_state(ProcessState::Created);
        infer_rust_thread.set_state(ThreadState::Ready);

        avatar_cpp_proc.set_state(ProcessState::Created);
        avatar_cpp_thread.set_state(ThreadState::Ready);

        ostd::scheduler::init_slots(
            shell_proc.koid().raw(),
            shell_thread.koid().raw(),
            daemon_proc.koid().raw(),
            daemon_thread.koid().raw(),
            infer_rust_proc.koid().raw(),
            infer_rust_thread.koid().raw(),
            avatar_cpp_proc.koid().raw(),
            avatar_cpp_thread.koid().raw(),
        );

        let mut guard = SCHEDULER_REGISTRY.lock();
        guard.entries.clear();
        guard.entries.push(ProcessThreadRecord {
            slot: ostd::scheduler::SLOT_SHELL,
            process: shell_proc,
            thread: shell_thread,
        });
        guard.entries.push(ProcessThreadRecord {
            slot: ostd::scheduler::SLOT_DAEMON,
            process: daemon_proc,
            thread: daemon_thread,
        });
        guard.entries.push(ProcessThreadRecord {
            slot: ostd::scheduler::SLOT_CHILD_RUST,
            process: infer_rust_proc,
            thread: infer_rust_thread,
        });
        guard.entries.push(ProcessThreadRecord {
            slot: ostd::scheduler::SLOT_CHILD_CPP,
            process: avatar_cpp_proc,
            thread: avatar_cpp_thread,
        });
    }

    /// Transitions the target child `Process` and `Thread` (`/bin/npu_infer.elf` or `/bin/avatar_cpp.elf`)
    /// to `Running` upon `sys_execve`, marking the parent `fipsos-shell.elf` as `Blocked`.
    pub fn begin_child_exec(path: &str, entry_rip: u64, user_rsp: u64) -> (Koid, Koid) {
        let target_slot = if path.contains("avatar_cpp") || entry_rip >= 0x0060_0000 {
            ostd::scheduler::SLOT_CHILD_CPP
        } else {
            ostd::scheduler::SLOT_CHILD_RUST
        };

        let guard = SCHEDULER_REGISTRY.lock();
        let mut child_pid = Koid::allocate();
        let mut child_tid = Koid::allocate();

        for rec in &guard.entries {
            if rec.slot == ostd::scheduler::SLOT_SHELL {
                rec.process.set_state(ProcessState::BlockedOnSyscall);
                rec.thread.set_state(ThreadState::Blocked);
            } else if rec.slot == target_slot {
                rec.process.set_state(ProcessState::RunningRing3);
                rec.thread.set_state(ThreadState::Running);
                rec.thread.set_last_rip(entry_rip);
                rec.thread.set_user_rsp(user_rsp);
                child_pid = rec.process.koid();
                child_tid = rec.thread.koid();
            }
        }
        drop(guard);

        ostd::scheduler::on_enter_child(
            target_slot,
            child_pid.raw(),
            child_tid.raw(),
            entry_rip,
            user_rsp,
        );
        (child_pid, child_tid)
    }

    /// Transitions the active child process to `Exited(exit_code)` and restores `fipsos-shell.elf`
    /// to `Running` when the child calls `sys_exit`.
    pub fn finish_child_exec(exit_code: i32, exit_rip: u64) {
        let pre_snap = ostd::scheduler::snapshot();
        let exited_slot = if pre_snap.active_slot == ostd::scheduler::SLOT_CHILD_CPP
            || exit_rip >= 0x0060_0000
        {
            ostd::scheduler::SLOT_CHILD_CPP
        } else {
            ostd::scheduler::SLOT_CHILD_RUST
        };

        ostd::scheduler::on_exit_child(exit_rip);
        let snap = ostd::scheduler::snapshot();
        let guard = SCHEDULER_REGISTRY.lock();
        for rec in &guard.entries {
            if rec.slot == ostd::scheduler::SLOT_SHELL {
                rec.process.set_state(ProcessState::RunningRing3);
                rec.thread.set_state(ThreadState::Running);
                rec.thread.set_cpu_ticks(snap.shell_ticks);
                rec.thread.set_context_switches(snap.shell_switches);
            } else if rec.slot == exited_slot {
                rec.process.set_state(ProcessState::Exited(exit_code));
                rec.thread.set_state(ThreadState::Exited(exit_code));
                rec.thread.set_last_rip(exit_rip);
                if exited_slot == ostd::scheduler::SLOT_CHILD_CPP {
                    rec.thread.set_cpu_ticks(snap.child_cpp_ticks);
                    rec.thread.set_context_switches(snap.child_cpp_switches);
                } else {
                    rec.thread.set_cpu_ticks(snap.child_rust_ticks);
                    rec.thread.set_context_switches(snap.child_rust_switches);
                }
            }
        }
    }

    /// Synchronizes all `Thread` objects in the registry with the latest lock-free `IRQ0` and
    /// `switch.s` counters from `ostd::scheduler` and prints the `[Scheduler::ps]` table.
    pub fn print_ps_table() -> usize {
        let snap = ostd::scheduler::snapshot();
        let guard = SCHEDULER_REGISTRY.lock();

        for rec in &guard.entries {
            match rec.slot {
                ostd::scheduler::SLOT_SHELL => {
                    rec.thread.set_cpu_ticks(snap.shell_ticks);
                    rec.thread.set_context_switches(snap.shell_switches);
                    rec.thread.set_last_rip(snap.shell_rip);
                    rec.thread.set_user_rsp(snap.shell_rsp);
                }
                ostd::scheduler::SLOT_DAEMON => {
                    rec.thread.set_cpu_ticks(snap.daemon_ticks);
                    rec.thread.set_context_switches(snap.daemon_switches);
                    rec.thread.set_last_rip(snap.daemon_rip);
                    rec.thread.set_user_rsp(snap.daemon_rsp);
                }
                ostd::scheduler::SLOT_CHILD_RUST => {
                    rec.thread.set_cpu_ticks(snap.child_rust_ticks);
                    rec.thread.set_context_switches(snap.child_rust_switches);
                    rec.thread.set_last_rip(snap.child_rust_rip);
                    rec.thread.set_user_rsp(snap.child_rust_rsp);
                }
                ostd::scheduler::SLOT_CHILD_CPP => {
                    rec.thread.set_cpu_ticks(snap.child_cpp_ticks);
                    rec.thread.set_context_switches(snap.child_cpp_switches);
                    rec.thread.set_last_rip(snap.child_cpp_rip);
                    rec.thread.set_user_rsp(snap.child_cpp_rsp);
                }
                _ => {}
            }
        }

        ostd::early_println!(
            "[Scheduler::ps] Round-Robin IRQ0 + switch.s ASM (Quantum={}t | IRQ0={} | Preemp={} | Yields={}):",
            ostd::scheduler::QUANTUM_TICKS,
            snap.total_irq0_ticks,
            snap.total_preemptions,
            snap.total_voluntary_yields
        );
        ostd::fb_println!(
            "[Scheduler::ps] Round-Robin IRQ0 + switch.s (Quantum={}t | IRQ0={} | Preemp={} | Yields={}):",
            ostd::scheduler::QUANTUM_TICKS,
            snap.total_irq0_ticks,
            snap.total_preemptions,
            snap.total_voluntary_yields
        );

        for rec in &guard.entries {
            ostd::early_println!(
                "  PID={:<9} | TID={:<9} | {:<20} ({:<12}) | {:<10} | RIP={:#018x} | RSP={:#x} | Ticks={:>4} | Sw={:>3}",
                rec.process.koid(),
                rec.thread.koid(),
                rec.process.name(),
                rec.thread.name(),
                rec.thread.state(),
                rec.thread.last_rip(),
                rec.thread.user_rsp(),
                rec.thread.cpu_ticks(),
                rec.thread.context_switches()
            );
            ostd::fb_println!(
                "  PID={:<9} | TID={:<9} | {:<19} ({:<11}) | {:<9} | RIP={:#x} | Ticks={} | Sw={}",
                rec.process.koid(),
                rec.thread.koid(),
                rec.process.name(),
                rec.thread.name(),
                rec.thread.state(),
                rec.thread.last_rip(),
                rec.thread.cpu_ticks(),
                rec.thread.context_switches()
            );
        }

        guard.entries.len()
    }
}
