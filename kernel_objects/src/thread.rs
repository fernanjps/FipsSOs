#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use ostd::sync::SpinLock;

use crate::object::{KernelObject, Koid, ObjectType};

/// Execution state of a FipSOs `Thread` object managed by the preemptive Round-Robin scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Ready in the Round-Robin runqueue waiting for a CPU quantum.
    Ready,
    /// Currently executing on a logical CPU core.
    Running,
    /// Blocked waiting on synchronous I/O, IPC channel, or child process completion.
    Blocked,
    /// Sleeping until a timer deadline or hardware event triggers.
    Sleeping,
    /// Terminated with the given exit status code.
    Exited(i32),
}

impl fmt::Display for ThreadState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => f.write_str("Ready"),
            Self::Running => f.write_str("Running"),
            Self::Blocked => f.write_str("Blocked"),
            Self::Sleeping => f.write_str("Sleeping"),
            Self::Exited(code) => write!(f, "Exited({})", code),
        }
    }
}

/// Zircon-style `Thread` kernel object representing a schedulable execution context
/// belonging to a parent `Process`.
pub struct Thread {
    koid: Koid,
    name: String,
    process_koid: Koid,
    entry_rip: u64,
    last_rip: AtomicU64,
    user_rsp: AtomicU64,
    state: SpinLock<ThreadState>,
    cpu_ticks: AtomicU64,
    context_switches: AtomicU64,
}

impl Thread {
    /// Creates a new `Thread` kernel object associated with `process_koid`.
    pub fn new(name: &str, process_koid: Koid, entry_rip: u64, user_rsp: u64) -> Arc<Self> {
        Arc::new(Self {
            koid: Koid::allocate(),
            name: String::from(name),
            process_koid,
            entry_rip,
            last_rip: AtomicU64::new(entry_rip),
            user_rsp: AtomicU64::new(user_rsp),
            state: SpinLock::new(ThreadState::Ready),
            cpu_ticks: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
        })
    }

    /// Returns the human-readable thread name.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the `Koid` of the owning `Process`.
    #[inline]
    pub fn process_koid(&self) -> Koid {
        self.process_koid
    }

    /// Returns the initial instruction pointer (`RIP`) of this thread.
    #[inline]
    pub fn entry_rip(&self) -> u64 {
        self.entry_rip
    }

    /// Returns the most recently sampled instruction pointer (`RIP`) of this thread.
    #[inline]
    pub fn last_rip(&self) -> u64 {
        self.last_rip.load(Ordering::Relaxed)
    }

    /// Updates the most recently sampled instruction pointer (`RIP`) of this thread.
    #[inline]
    pub fn set_last_rip(&self, rip: u64) {
        if rip != 0 {
            self.last_rip.store(rip, Ordering::Relaxed);
        }
    }

    /// Returns the current user stack pointer (`RSP`) of this thread.
    #[inline]
    pub fn user_rsp(&self) -> u64 {
        self.user_rsp.load(Ordering::Relaxed)
    }

    /// Updates the current user stack pointer (`RSP`) of this thread.
    #[inline]
    pub fn set_user_rsp(&self, rsp: u64) {
        if rsp != 0 {
            self.user_rsp.store(rsp, Ordering::Relaxed);
        }
    }

    /// Returns the current `ThreadState`.
    #[inline]
    pub fn state(&self) -> ThreadState {
        *self.state.lock()
    }

    /// Updates the current `ThreadState`.
    #[inline]
    pub fn set_state(&self, new_state: ThreadState) {
        *self.state.lock() = new_state;
    }

    /// Returns the total hardware timer ticks (`IRQ0`) and execution quanta accumulated by this thread.
    #[inline]
    pub fn cpu_ticks(&self) -> u64 {
        self.cpu_ticks.load(Ordering::Relaxed)
    }

    /// Atomically sets the synchronized `cpu_ticks` value from the `ostd` hardware scheduler.
    #[inline]
    pub fn set_cpu_ticks(&self, ticks: u64) {
        self.cpu_ticks.store(ticks, Ordering::Relaxed);
    }

    /// Atomically increments `cpu_ticks` by `delta` and returns the new total.
    #[inline]
    pub fn add_cpu_ticks(&self, delta: u64) -> u64 {
        self.cpu_ticks.fetch_add(delta, Ordering::Relaxed) + delta
    }

    /// Returns the total number of preemptive and cooperative context switches for this thread.
    #[inline]
    pub fn context_switches(&self) -> u64 {
        self.context_switches.load(Ordering::Relaxed)
    }

    /// Atomically sets the synchronized `context_switches` count from the `ostd` hardware scheduler.
    #[inline]
    pub fn set_context_switches(&self, switches: u64) {
        self.context_switches.store(switches, Ordering::Relaxed);
    }

    /// Atomically increments `context_switches` by 1 and returns the updated value.
    #[inline]
    pub fn record_context_switch(&self) -> u64 {
        self.context_switches.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl fmt::Debug for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thread")
            .field("koid", &self.koid)
            .field("name", &self.name)
            .field("process_koid", &self.process_koid)
            .field("state", &self.state())
            .field("last_rip", &self.last_rip())
            .field("user_rsp", &self.user_rsp())
            .field("cpu_ticks", &self.cpu_ticks())
            .field("context_switches", &self.context_switches())
            .finish()
    }
}

impl KernelObject for Thread {
    fn koid(&self) -> Koid {
        self.koid
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Thread
    }

    fn as_thread(&self) -> Option<&Thread> {
        Some(self)
    }
}
