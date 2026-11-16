#![forbid(unsafe_code)]

//! Per-process Ring-3 Capability Sandbox flags (Fuchsia / OpenBSD pledge style).

/// Permission to open/read regular files outside standard runtime libraries.
pub const CAP_FS_READ: u64 = 1 << 0;
/// Permission to create, write, truncate, or unlink files.
pub const CAP_FS_WRITE: u64 = 1 << 1;
/// Permission to open network sockets, bind, listen, or connect.
pub const CAP_NET: u64 = 1 << 2;
/// Permission to spawn/fork/execve child processes.
pub const CAP_SPAWN: u64 = 1 << 3;
/// Permission to access raw `/dev/fb0` framebuffer device or ioctls.
pub const CAP_RAW_FB: u64 = 1 << 4;
/// Full capability mask granted to unsandboxed Ring-3 processes.
pub const CAP_ALL: u64 = CAP_FS_READ | CAP_FS_WRITE | CAP_NET | CAP_SPAWN | CAP_RAW_FB;

/// Restricts filesystem writes exclusively to `/mnt/fat0/sandbox/`.
pub const SANDBOX_RESTRICT_FAT0: u64 = 1 << 0;
/// Marks the calling slot for automatic kernel reaping upon exit (background `&`).
pub const SANDBOX_AUTO_REAP: u64 = 1 << 1;

/// Irreversibly narrowable per-process capability bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityFlags(pub u64);

impl CapabilityFlags {
    /// Returns full capabilities (`CAP_ALL`).
    #[inline]
    pub const fn all() -> Self {
        Self(CAP_ALL)
    }

    /// Wraps a raw capability mask, masking unknown bits.
    #[inline]
    pub const fn from_raw(mask: u64) -> Self {
        Self(mask & CAP_ALL)
    }

    /// Returns the raw capability bitmask.
    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Checks whether all bits in `required` are present.
    #[inline]
    pub const fn has(self, required: u64) -> bool {
        (self.0 & required) == required
    }

    /// Irreversibly narrows this mask by intersecting with `new_mask`.
    #[inline]
    pub const fn narrow(self, new_mask: u64) -> Self {
        Self(self.0 & (new_mask & CAP_ALL))
    }
}
