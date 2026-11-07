#![forbid(unsafe_code)]

use core::any::Any;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::channel::Channel;
use crate::process::Process;
use crate::thread::Thread;
use crate::vmo::Vmo;

/// Unique 64-bit Kernel Object Identifier (inspired by Zircon `zx_koid_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Koid(u64);

static NEXT_KOID: AtomicU64 = AtomicU64::new(1024);

impl Koid {
    /// Allocates a new globally unique `Koid`.
    pub fn allocate() -> Self {
        Self(NEXT_KOID.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the raw 64-bit identifier value.
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Koid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "koid:{}", self.0)
    }
}

/// Canonical kernel object types managed by FipSOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    /// Virtual Memory Object (process memory, shared DMA buffers, GPU textures, NPU tensors).
    Vmo,
    /// Process container.
    Process,
    /// Execution thread.
    Thread,
    /// IPC message channel.
    Channel,
    /// Direct Rendering Manager (DRM) GPU device object.
    DrmDevice,
    /// Neural Processing Unit (`/dev/accel`) device object.
    AccelDevice,
}

impl fmt::Display for ObjectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Vmo => "VMO",
            Self::Process => "Process",
            Self::Thread => "Thread",
            Self::Channel => "Channel",
            Self::DrmDevice => "DrmDevice",
            Self::AccelDevice => "AccelDevice",
        };
        f.write_str(name)
    }
}

/// Base trait implemented by all capability-protected kernel objects in FipSOs.
pub trait KernelObject: Any + Send + Sync + fmt::Debug {
    /// Returns the globally unique Kernel Object ID (`Koid`) of this object.
    fn koid(&self) -> Koid;

    /// Returns the canonical `ObjectType` of this kernel object.
    fn object_type(&self) -> ObjectType;

    /// Safe downcast helper to `Vmo` if this object is a Virtual Memory Object.
    fn as_vmo(&self) -> Option<&Vmo> {
        None
    }

    /// Safe downcast helper to `Channel` if this object is an IPC Channel endpoint.
    fn as_channel(&self) -> Option<&Channel> {
        None
    }

    /// Safe downcast helper to `Process` if this object is a Process container.
    fn as_process(&self) -> Option<&Process> {
        None
    }

    /// Safe downcast helper to `Thread` if this object is an execution Thread.
    fn as_thread(&self) -> Option<&Thread> {
        None
    }
}
