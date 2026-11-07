use alloc::collections::BTreeMap;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::fpu::FpuState;
use crate::mm::{DmaBuffer, PhysAddr, VirtAddr};
use crate::sync::SpinLock;

/// Unique identifier for a memory region registered inside `FfiJail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FfiRegionId(u64);

static NEXT_REGION_ID: AtomicU64 = AtomicU64::new(1);

impl FfiRegionId {
    fn allocate() -> Self {
        Self(NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FfiRegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ffi_region:#{}", self.0)
    }
}

/// Registered memory range authorized for external C (`extern "C"`) driver/firmware access.
#[derive(Debug, Clone, Copy)]
pub struct FfiMemoryGrant {
    pub id: FfiRegionId,
    pub tag: &'static str,
    pub base_vaddr: VirtAddr,
    pub base_paddr: PhysAddr,
    pub size_bytes: usize,
    pub writable: bool,
    pub active: bool,
}

/// Errors returned by `FfiJail` when validating or executing an external C routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiJailError {
    /// The specified `FfiRegionId` was never registered in the jail.
    UnregisteredRegion(FfiRegionId),
    /// The specified `FfiRegionId` was revoked prior to execution.
    RevokedRegion(FfiRegionId),
    /// The external call requested write access on a read-only grant.
    WritePermissionDenied(FfiRegionId),
    /// The requested offset/length falls outside the registered memory grant bounds.
    OutOfBoundsAccess {
        region: FfiRegionId,
        offset: usize,
        len: usize,
        capacity: usize,
    },
    /// External C function returned a non-zero error status code.
    ExternalCallFailed(i32),
}

/// Safe C-ABI function signature confined by `FfiJail`.
///
/// Receives `(buffer_base_ptr, buffer_len, opaque_arg) -> status_code`.
pub type SandboxedExternCFn = extern "C" fn(*mut u8, usize, u64) -> i32;

/// Isolated FFI Jail enforcing memory-range registration before any `extern "C"` function executes.
pub struct FfiJail {
    grants: SpinLock<BTreeMap<FfiRegionId, FfiMemoryGrant>>,
    invocations: AtomicU64,
}

impl FfiJail {
    /// Creates a new isolated `FfiJail`.
    pub const fn new() -> Self {
        Self {
            grants: SpinLock::new(BTreeMap::new()),
            invocations: AtomicU64::new(0),
        }
    }

    /// Registers a `DmaBuffer` memory range in the jail before an external C function can touch it.
    pub fn register_dma_region(
        &self,
        buffer: &DmaBuffer,
        writable: bool,
        tag: &'static str,
    ) -> FfiRegionId {
        let id = FfiRegionId::allocate();
        let grant = FfiMemoryGrant {
            id,
            tag,
            base_vaddr: buffer.vaddr(),
            base_paddr: buffer.paddr(),
            size_bytes: buffer.size_bytes(),
            writable,
            active: true,
        };
        self.grants.lock().insert(id, grant);
        id
    }

    /// Revokes a previously registered `FfiRegionId`, preventing any future C FFI access.
    pub fn revoke_region(&self, id: FfiRegionId) -> Result<(), FfiJailError> {
        let mut guard = self.grants.lock();
        let grant = guard
            .get_mut(&id)
            .ok_or(FfiJailError::UnregisteredRegion(id))?;
        grant.active = false;
        Ok(())
    }

    /// Returns metadata for a registered grant.
    pub fn query_grant(&self, id: FfiRegionId) -> Result<FfiMemoryGrant, FfiJailError> {
        self.grants
            .lock()
            .get(&id)
            .copied()
            .ok_or(FfiJailError::UnregisteredRegion(id))
    }

    /// Executes a sandboxed `extern "C"` function on a sub-slice `[offset..offset + len)` of a
    /// registered `FfiRegionId` after verifying bounds, liveness, and write permissions.
    pub fn execute_c_function(
        &self,
        region_id: FfiRegionId,
        offset: usize,
        len: usize,
        require_write: bool,
        opaque_arg: u64,
        c_fn: SandboxedExternCFn,
    ) -> Result<i32, FfiJailError> {
        let grant = {
            let guard = self.grants.lock();
            *guard
                .get(&region_id)
                .ok_or(FfiJailError::UnregisteredRegion(region_id))?
        };

        if !grant.active {
            return Err(FfiJailError::RevokedRegion(region_id));
        }

        if require_write && !grant.writable {
            return Err(FfiJailError::WritePermissionDenied(region_id));
        }

        let end = offset
            .checked_add(len)
            .ok_or(FfiJailError::OutOfBoundsAccess {
                region: region_id,
                offset,
                len,
                capacity: grant.size_bytes,
            })?;

        if end > grant.size_bytes {
            return Err(FfiJailError::OutOfBoundsAccess {
                region: region_id,
                offset,
                len,
                capacity: grant.size_bytes,
            });
        }

        let scoped_ptr = (grant.base_vaddr as *mut u8).wrapping_add(offset);
        self.invocations.fetch_add(1, Ordering::Relaxed);

        // Preserve and restore x87 FPU + SSE/SSE2 XMM0..XMM15 state across external C execution
        // so Ring 0 C11 SIMD/FP32 drivers never corrupt Ring 3 C++20 floating-point registers.
        let mut caller_fpu = FpuState::new();
        caller_fpu.save();
        let status = c_fn(scoped_ptr, len, opaque_arg);
        caller_fpu.restore();

        if status < 0 {
            Err(FfiJailError::ExternalCallFailed(status))
        } else {
            Ok(status)
        }
    }

    /// Total number of verified `extern "C"` calls executed inside this jail.
    #[inline]
    pub fn total_invocations(&self) -> u64 {
        self.invocations.load(Ordering::Relaxed)
    }
}

impl Default for FfiJail {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in reference `extern "C"` driver routine (simulating a C NPU/GPU firmware checksum/transform)
/// that operates strictly within the pointer and length granted by `FfiJail`.
pub extern "C" fn sample_c_firmware_routine(ptr: *mut u8, len: usize, seed: u64) -> i32 {
    if ptr.is_null() || len == 0 {
        return -1;
    }

    let mut checksum: u32 = seed as u32;
    for i in 0..len.min(64) {
        // SAFETY: `FfiJail::execute_c_function` verified `[ptr, ptr + len)` is mapped and valid.
        let b = unsafe { core::ptr::read_volatile(ptr.add(i)) };
        checksum = checksum.wrapping_mul(33).wrapping_add(b as u32);
    }
    (checksum & 0x7FFF_FFFF) as i32
}

extern "C" {
    /// Real freestanding C11 NPU Tensor Multiply-Accumulate (MAC) hardware driver routine
    /// compiled by `gcc` from `ostd/c_src/npu_hw_driver.c`.
    pub fn c_npu_tensor_mac_kernel(dma_buf: *mut u8, len: usize, seed: u64) -> i32;
}

/// Safe `SandboxedExternCFn` bridge to the C11 `c_npu_tensor_mac_kernel` driver.
///
/// Must only be invoked via `FfiJail::execute_c_function`, which validates that
/// `[dma_buf, dma_buf + len)` belongs to an active, writable `FfiMemoryGrant`.
pub extern "C" fn c_npu_driver_entry(dma_buf: *mut u8, len: usize, seed: u64) -> i32 {
    // SAFETY: `FfiJail::execute_c_function` verifies bounds, liveness, and write permissions
    // before invoking this callback.
    unsafe { c_npu_tensor_mac_kernel(dma_buf, len, seed) }
}
