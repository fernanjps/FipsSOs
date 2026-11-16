#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use ostd::sync::SpinLock;

use crate::capability::{CapabilityFlags, CAP_ALL};
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

/// Zircon-style capability-isolated Process container owning a private `HandleTable`
/// and a dedicated hardware 4-level page-table root (`cr3_phys` / `PML4`).
pub struct Process {
    koid: Koid,
    name: String,
    entry_point: u64,
    cr3_phys: AtomicU64,
    capabilities: AtomicU64,
    state: SpinLock<ProcessState>,
    handle_table: SpinLock<HandleTable>,
}

impl Process {
    /// Creates a new isolated Ring 3 `Process` kernel object owning a dedicated physical `PML4` (`CR3`).
    pub fn new(name: &str, entry_point: u64) -> Arc<Self> {
        let pml4_phys = ostd::mm::create_process_pml4()
            .expect("Fallo al crear directorio raiz PML4 aislado para Process");
        Arc::new(Self {
            koid: Koid::allocate(),
            name: String::from(name),
            entry_point,
            cr3_phys: AtomicU64::new(pml4_phys),
            capabilities: AtomicU64::new(CAP_ALL),
            state: SpinLock::new(ProcessState::Created),
            handle_table: SpinLock::new(HandleTable::new()),
        })
    }

    /// Creates a Ring 0 kernel daemon `Process` (`npu-telemetryd`) bound to `MASTER_KERNEL_PML4`.
    pub fn new_kernel(name: &str, entry_point: u64) -> Arc<Self> {
        Arc::new(Self {
            koid: Koid::allocate(),
            name: String::from(name),
            entry_point,
            cr3_phys: AtomicU64::new(ostd::mm::master_kernel_cr3()),
            capabilities: AtomicU64::new(CAP_ALL),
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

    /// Returns the physical `PML4` root address (`CR3`) dedicated to this process.
    #[inline]
    pub fn cr3_phys(&self) -> u64 {
        self.cr3_phys.load(Ordering::Acquire)
    }

    /// Updates the physical `PML4` root address (`CR3`) owned by this process.
    #[inline]
    pub fn set_cr3_phys(&self, pml4_phys: u64) {
        self.cr3_phys.store(pml4_phys, Ordering::Release);
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

    /// Returns the current `CapabilityFlags` bitmask for this process.
    #[inline]
    pub fn capabilities(&self) -> CapabilityFlags {
        CapabilityFlags::from_raw(self.capabilities.load(Ordering::Acquire))
    }

    /// Irreversibly narrows the process's `CapabilityFlags` bitmask.
    pub fn narrow_capabilities(&self, mask: u64) -> CapabilityFlags {
        let updated = self.capabilities().narrow(mask);
        self.capabilities.store(updated.bits(), Ordering::Release);
        updated
    }
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Process")
            .field("koid", &self.koid)
            .field("name", &self.name)
            .field("entry_point", &self.entry_point)
            .field("cr3_phys", &self.cr3_phys())
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
    /// Registers the initial primary Ring 3 process (`shell_proc`) and thread (`shell_thread`)
    /// without allocating phantom boot processes.
    pub fn register_primary_process(shell_proc: Arc<Process>, shell_thread: Arc<Thread>) {
        shell_proc.set_state(ProcessState::RunningRing3);
        shell_thread.set_state(ThreadState::Running);

        ostd::scheduler::init_slots(
            shell_proc.koid().raw(),
            shell_thread.koid().raw(),
            0, 0, 0, 0, 0, 0, 0, 0,
        );
        ostd::scheduler::set_slot_cr3(ostd::scheduler::SLOT_SHELL, shell_proc.cr3_phys());
        ostd::scheduler::set_slot_name(ostd::scheduler::SLOT_SHELL, shell_proc.name());

        let mut guard = SCHEDULER_REGISTRY.lock();
        guard.entries.clear();
        guard.entries.push(ProcessThreadRecord {
            slot: ostd::scheduler::SLOT_SHELL,
            process: shell_proc,
            thread: shell_thread,
        });
    }

    fn resolve_child_slot(path: &str) -> u64 {
        if path.contains("desktop_ui")
            || path.ends_with("/gui")
            || path.ends_with("/desktop")
            || path == "gui"
            || path == "desktop"
        {
            ostd::scheduler::SLOT_DESKTOP_UI
        } else if path.contains("avatar_cpp") {
            ostd::scheduler::SLOT_CHILD_CPP
        } else {
            ostd::scheduler::SLOT_CHILD_RUST
        }
    }

    /// Returns (or creates) the dedicated physical `PML4` (`CR3`) assigned to the target child
    /// `Process` before `sys_execve` loads its ELF64 `PT_LOAD` segments and Ring 3 stack.
    pub fn prepare_child_cr3(path: &str) -> Result<u64, &'static str> {
        let preferred_slot = Self::resolve_child_slot(path);
        let cr3 = ostd::mm::create_process_pml4()?;
        let Some(assigned_slot) = ostd::scheduler::acquire_exec_slot(preferred_slot, cr3) else {
            let _ = ostd::mm::destroy_process_vm_space(cr3);
            return Err("No hay slots de ejecucion libres en SchedulerRegistry");
        };

        let guard = SCHEDULER_REGISTRY.lock();
        for rec in &guard.entries {
            if rec.slot == assigned_slot && matches!(rec.process.state(), ProcessState::Created) {
                rec.process.set_cr3_phys(cr3);
            }
        }
        ostd::scheduler::set_slot_cr3(assigned_slot, cr3);
        Ok(cr3)
    }

    /// Clears a cached child `CR3` if `sys_execve` fails after `prepare_child_cr3` frees it.
    pub fn abort_child_cr3(path: &str, freed_cr3: u64) {
        let preferred_slot = Self::resolve_child_slot(path);
        let slot = ostd::scheduler::slot_for_cr3(freed_cr3).unwrap_or(preferred_slot);
        let guard = SCHEDULER_REGISTRY.lock();
        for rec in &guard.entries {
            if rec.slot == slot && rec.process.cr3_phys() == freed_cr3 {
                rec.process.set_cr3_phys(0);
            }
        }
        drop(guard);
        ostd::scheduler::abort_exec_slot(freed_cr3);
        ostd::scheduler::set_slot_cr3(slot, 0);
    }

    /// Transitions the target child `Process` and `Thread` (`/bin/desktop_ui.elf`,
    /// `/bin/npu_infer.elf`, or `/bin/avatar_cpp.elf`) to `Running` upon `sys_execve`,
    /// marking the calling parent process (`Shell` or `Desktop UI`) as `Blocked`.
    pub fn begin_child_exec(path: &str, entry_rip: u64, user_rsp: u64) -> (Koid, Koid) {
        let pre_snap = ostd::scheduler::snapshot();
        let parent_slot = pre_snap.active_slot;
        let preferred_slot = Self::resolve_child_slot(path);
        let target_slot =
            ostd::scheduler::slot_for_cr3(ostd::mm::active_cr3()).unwrap_or(preferred_slot);

        let guard = SCHEDULER_REGISTRY.lock();
        let mut child_pid = Koid::allocate();
        let mut child_tid = Koid::allocate();

        for rec in &guard.entries {
            if rec.slot == parent_slot {
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

        ostd::scheduler::sync_slot_identity(target_slot, child_pid.raw(), path);
        ostd::scheduler::on_enter_child(
            target_slot,
            child_pid.raw(),
            child_tid.raw(),
            entry_rip,
            user_rsp,
        );
        (child_pid, child_tid)
    }

    /// Transitions the active child process to `Exited(exit_code)` and restores its stacked parent
    /// (`Desktop UI` or `fipsos-shell.elf`) to `Running` when the child calls `sys_exit`.
    pub fn finish_child_exec(exit_code: i32, exit_rip: u64) {
        let (exited_slot, resumed_parent_slot) = ostd::scheduler::on_exit_child(exit_rip);
        let snap = ostd::scheduler::snapshot();
        let guard = SCHEDULER_REGISTRY.lock();
        for rec in &guard.entries {
            if rec.slot == resumed_parent_slot {
                rec.process.set_state(ProcessState::RunningRing3);
                rec.thread.set_state(ThreadState::Running);
                if resumed_parent_slot == ostd::scheduler::SLOT_DESKTOP_UI {
                    rec.thread.set_cpu_ticks(snap.desktop_ui_ticks);
                    rec.thread.set_context_switches(snap.desktop_ui_switches);
                } else {
                    rec.thread.set_cpu_ticks(snap.shell_ticks);
                    rec.thread.set_context_switches(snap.shell_switches);
                }
            } else if rec.slot == exited_slot {
                rec.process.set_state(ProcessState::Exited(exit_code));
                rec.thread.set_state(ThreadState::Exited(exit_code));
                rec.thread.set_last_rip(exit_rip);
                rec.process.set_cr3_phys(0);
                ostd::scheduler::set_slot_cr3(exited_slot, 0);
                match exited_slot {
                    ostd::scheduler::SLOT_DESKTOP_UI => {
                        rec.thread.set_cpu_ticks(snap.desktop_ui_ticks);
                        rec.thread.set_context_switches(snap.desktop_ui_switches);
                    }
                    ostd::scheduler::SLOT_CHILD_CPP => {
                        rec.thread.set_cpu_ticks(snap.child_cpp_ticks);
                        rec.thread.set_context_switches(snap.child_cpp_switches);
                    }
                    _ => {
                        rec.thread.set_cpu_ticks(snap.child_rust_ticks);
                        rec.thread.set_context_switches(snap.child_rust_switches);
                    }
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
                ostd::scheduler::SLOT_DESKTOP_UI => {
                    rec.thread.set_cpu_ticks(snap.desktop_ui_ticks);
                    rec.thread.set_context_switches(snap.desktop_ui_switches);
                    rec.thread.set_last_rip(snap.desktop_ui_rip);
                    rec.thread.set_user_rsp(snap.desktop_ui_rsp);
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
                ostd::scheduler::SLOT_BG_DOWNLOADER => {
                    rec.thread.set_cpu_ticks(snap.bg_downloader_ticks);
                    rec.thread.set_context_switches(snap.bg_downloader_switches);
                }
                _ => {}
            }

            if rec.slot == ostd::scheduler::SLOT_DAEMON && !snap.daemon_active {
                rec.process.set_state(ProcessState::Exited(9));
                rec.thread.set_state(ThreadState::Exited(9));
            } else if rec.slot == ostd::scheduler::SLOT_BG_DOWNLOADER && !snap.bg_downloader_active {
                rec.process.set_state(ProcessState::Exited(9));
                rec.thread.set_state(ThreadState::Exited(9));
            }
        }

        ostd::early_println!(
            "[Scheduler::ps] Round-Robin IRQ0 + switch.s ASM (Quantum={}t | ActiveCR3={:#x} | IRQ0={} | Preemp={} | Yields={}):",
            ostd::scheduler::QUANTUM_TICKS,
            snap.active_cr3,
            snap.total_irq0_ticks,
            snap.total_preemptions,
            snap.total_voluntary_yields
        );
        ostd::fb_println!(
            "[Scheduler::ps] Round-Robin IRQ0 + switch.s (ActiveCR3={:#x} | IRQ0={} | Preemp={} | Yields={}):",
            snap.active_cr3,
            snap.total_irq0_ticks,
            snap.total_preemptions,
            snap.total_voluntary_yields
        );

        let occupied = ostd::scheduler::occupied_slots_snapshot();
        for s in &occupied {
            let (ticks, switches, rip, rsp, cr3) = match s.slot {
                ostd::scheduler::SLOT_SHELL => (
                    snap.shell_ticks,
                    snap.shell_switches,
                    snap.shell_rip,
                    snap.shell_rsp,
                    snap.shell_cr3,
                ),
                ostd::scheduler::SLOT_DAEMON => (
                    snap.daemon_ticks,
                    snap.daemon_switches,
                    snap.daemon_rip,
                    snap.daemon_rsp,
                    snap.daemon_cr3,
                ),
                ostd::scheduler::SLOT_DESKTOP_UI => (
                    snap.desktop_ui_ticks,
                    snap.desktop_ui_switches,
                    snap.desktop_ui_rip,
                    snap.desktop_ui_rsp,
                    snap.desktop_ui_cr3,
                ),
                ostd::scheduler::SLOT_CHILD_RUST => (
                    snap.child_rust_ticks,
                    snap.child_rust_switches,
                    snap.child_rust_rip,
                    snap.child_rust_rsp,
                    snap.child_rust_cr3,
                ),
                ostd::scheduler::SLOT_CHILD_CPP => (
                    snap.child_cpp_ticks,
                    snap.child_cpp_switches,
                    snap.child_cpp_rip,
                    snap.child_cpp_rsp,
                    snap.child_cpp_cr3,
                ),
                ostd::scheduler::SLOT_BG_DOWNLOADER => (
                    snap.bg_downloader_ticks,
                    snap.bg_downloader_switches,
                    s.rip,
                    s.rsp,
                    s.cr3_phys,
                ),
                _ => (s.ticks, s.switches, s.rip, s.rsp, s.cr3_phys),
            };
            ostd::early_println!(
                "  SLOT={:<2} | PID={:<6} | PPID={:<6} | {:<18} | {:?} | CR3={:#010x} | RIP={:#018x} | RSP={:#x} | Ticks={:>4} | Sw={:>3}",
                s.slot,
                s.pid,
                s.ppid,
                s.name_str(),
                s.state,
                cr3,
                rip,
                rsp,
                ticks,
                switches
            );
            ostd::fb_println!(
                "  SLOT={:<2} | PID={:<6} | {:<16} | {:?} | CR3={:#x} | Ticks={} | Sw={}",
                s.slot,
                s.pid,
                s.name_str(),
                s.state,
                cr3,
                ticks,
                switches
            );
        }

        occupied.len()
    }

    /// Terminates or signals a process by PID or scheduler slot using `ostd::scheduler::kill_task`.
    pub fn kill_process(pid: u64, sig: u64) -> Result<u64, i64> {
        let killed_pid = ostd::scheduler::kill_task(pid, sig)?;
        let guard = SCHEDULER_REGISTRY.lock();
        for rec in &guard.entries {
            if rec.process.koid().raw() == killed_pid {
                rec.process.set_state(ProcessState::Exited(sig as i32));
                rec.thread.set_state(ThreadState::Exited(sig as i32));
            }
        }
        Ok(killed_pid)
    }

    /// Spawns or reactivates a background downloader task (`SLOT_BG_DOWNLOADER`) in the registry.
    pub fn spawn_background_task(name: &str) -> (u64, u64) {
        let (pid, tid) = ostd::scheduler::spawn_background_downloader();
        let mut guard = SCHEDULER_REGISTRY.lock();
        let mut found = false;
        for rec in &guard.entries {
            if rec.slot == ostd::scheduler::SLOT_BG_DOWNLOADER {
                rec.process.set_state(ProcessState::RunningRing3);
                rec.thread.set_state(ThreadState::Running);
                found = true;
                break;
            }
        }
        if !found {
            let proc = Arc::new(Process {
                koid: Koid::from_raw(pid),
                name: String::from(name),
                entry_point: 0x0040_0000,
                cr3_phys: AtomicU64::new(ostd::mm::master_kernel_cr3()),
                capabilities: AtomicU64::new(CAP_ALL),
                state: SpinLock::new(ProcessState::RunningRing3),
                handle_table: SpinLock::new(HandleTable::new()),
            });
            let thread = Thread::new(name, proc.koid(), 0x0040_0000, 0x0000_7FFF_FFFF_0000);
            thread.set_state(ThreadState::Running);
            guard.entries.push(ProcessThreadRecord {
                slot: ostd::scheduler::SLOT_BG_DOWNLOADER,
                process: proc,
                thread,
            });
        }
        (pid, tid)
    }
}
