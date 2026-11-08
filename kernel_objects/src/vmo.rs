#![forbid(unsafe_code)]

use alloc::sync::Arc;
use core::fmt;
use ostd::mm::{DmaBuffer, DmaError, PhysAddr, VirtAddr, PAGE_SIZE};

use crate::object::{KernelObject, Koid, ObjectType};

/// Intended usage profile of a Virtual Memory Object (`Vmo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmoUsage {
    /// Standard process memory (stacks, heaps, anonymous mappings).
    ProcessMemory,
    /// Zero-Copy physically contiguous buffer shared between CPU, GPU (Vulkan/DRM), and NPU (`/dev/accel`).
    ZeroCopyAccel,
    /// File or AI model payload loaded from `initramfs.tar` (`TarFs`).
    InitramfsFile,
    /// Zero-Syscall lock-free circular Submission Queue / Completion Queue Ring.
    SharedRing,
}

impl fmt::Display for VmoUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessMemory => f.write_str("ProcessMemory"),
            Self::ZeroCopyAccel => f.write_str("ZeroCopyAccel (CPU/GPU/NPU)"),
            Self::InitramfsFile => f.write_str("InitramfsFile (TarFs)"),
            Self::SharedRing => f.write_str("SharedRing (Zero-Syscall SQ/CQ)"),
        }
    }
}

/// Magic identifier for Zero-Syscall VMO I/O Rings (`'VMOR'`).
pub const VMO_RING_MAGIC: u32 = 0x564D_4F52;
/// Protocol version for Zero-Syscall VMO I/O Rings.
pub const VMO_RING_VERSION: u32 = 1;
/// Queue capacity (depth) of the ring buffers (32 entries).
pub const VMO_RING_QUEUE_DEPTH: usize = 32;

/// Operation codes for Zero-Syscall Submission Queue Entries (SQEs).
pub const VMO_RING_OP_NOP: u32 = 0;
pub const VMO_RING_OP_ECHO: u32 = 1;
pub const VMO_RING_OP_3D_TRANSFORM: u32 = 2;
pub const VMO_RING_OP_NET_POLL: u32 = 3;
pub const VMO_RING_OP_MEM_CRC: u32 = 4;

/// Submission Queue Entry (SQE) produced by Ring 3 userland, consumed by Ring 0 kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmoSqEntry {
    pub opcode: u32,
    pub flags: u32,
    pub user_data: u64,
    pub buffer_offset: u64,
    pub len: u32,
    pub reserved: u32,
}

/// Completion Queue Entry (CQE) produced by Ring 0 kernel, consumed by Ring 3 userland.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmoCqEntry {
    pub user_data: u64,
    pub result: i32,
    pub flags: u32,
}

/// Fixed header layout residing at byte 0 of the Shared Ring VMO.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmoRingHeader {
    pub magic: u32,
    pub version: u32,
    pub queue_depth: u32,
    pub flags: u32,
    pub sq_head: u32,
    pub sq_tail: u32,
    pub cq_head: u32,
    pub cq_tail: u32,
    pub sq_dropped: u32,
    pub cq_overflow: u32,
    pub reserved: [u32; 6],
}

impl VmoRingHeader {
    /// Creates a default initialized ring header.
    pub const fn new() -> Self {
        Self {
            magic: VMO_RING_MAGIC,
            version: VMO_RING_VERSION,
            queue_depth: VMO_RING_QUEUE_DEPTH as u32,
            flags: 0,
            sq_head: 0,
            sq_tail: 0,
            cq_head: 0,
            cq_tail: 0,
            sq_dropped: 0,
            cq_overflow: 0,
            reserved: [0; 6],
        }
    }

    /// Serializes header into bytes.
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..12].copy_from_slice(&self.queue_depth.to_le_bytes());
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..20].copy_from_slice(&self.sq_head.to_le_bytes());
        buf[20..24].copy_from_slice(&self.sq_tail.to_le_bytes());
        buf[24..28].copy_from_slice(&self.cq_head.to_le_bytes());
        buf[28..32].copy_from_slice(&self.cq_tail.to_le_bytes());
        buf[32..36].copy_from_slice(&self.sq_dropped.to_le_bytes());
        buf[36..40].copy_from_slice(&self.cq_overflow.to_le_bytes());
        buf
    }
}

/// Errors returned by `Vmo` operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmoError {
    /// Underlying physical frame / DMA allocation error.
    AllocationFailed(DmaError),
    /// Out-of-bounds read or write on the VMO.
    OutOfBounds {
        offset: usize,
        len: usize,
        size: usize,
    },
}

impl From<DmaError> for VmoError {
    fn from(err: DmaError) -> Self {
        match err {
            DmaError::OutOfBounds {
                offset,
                len,
                capacity,
            } => Self::OutOfBounds {
                offset,
                len,
                size: capacity,
            },
            other => Self::AllocationFailed(other),
        }
    }
}

/// Virtual Memory Object (`Vmo`) wrapping physical frames from `ostd::mm` via `Arc`.
///
/// Supports Zero-Copy sharing across user processes, GPU DRM buffers (Vulkan textures),
/// and NPU accelerator tensors (`/dev/accel`).
#[derive(Debug)]
pub struct Vmo {
    koid: Koid,
    requested_size: usize,
    usage: VmoUsage,
    buffer: Arc<DmaBuffer>,
}

impl Vmo {
    /// Allocates a new `Vmo` backed by physically contiguous 4 KiB frames in `ostd::mm`.
    pub fn new(requested_size: usize, usage: VmoUsage) -> Result<Arc<Self>, VmoError> {
        let dma_buffer = DmaBuffer::allocate(requested_size)?;
        Ok(Arc::new(Self {
            koid: Koid::allocate(),
            requested_size,
            usage,
            buffer: Arc::new(dma_buffer),
        }))
    }

    /// Creates a child/shared `Vmo` that shares the exact same underlying `Arc<DmaBuffer>`
    /// physical frames (Zero-Copy) with a new `Koid`.
    pub fn clone_shared(&self) -> Arc<Self> {
        Arc::new(Self {
            koid: Koid::allocate(),
            requested_size: self.requested_size,
            usage: self.usage,
            buffer: Arc::clone(&self.buffer),
        })
    }

    /// Requested size in bytes when this `Vmo` was created.
    #[inline]
    pub fn requested_size(&self) -> usize {
        self.requested_size
    }

    /// Actual page-aligned size in bytes (multiple of `4096`).
    #[inline]
    pub fn size_bytes(&self) -> usize {
        self.buffer.size_bytes()
    }

    /// Number of 4 KiB physical frames backing this `Vmo`.
    #[inline]
    pub fn frame_count(&self) -> usize {
        self.buffer.frame_count()
    }

    /// Base physical address of the contiguous backing frames (for DMA / GPU / NPU engines).
    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        self.buffer.paddr()
    }

    /// Base virtual address in the Higher-Half Direct Map (HHDM).
    #[inline]
    pub fn virt_addr(&self) -> VirtAddr {
        self.buffer.vaddr()
    }

    /// Usage profile of this `Vmo`.
    #[inline]
    pub fn usage(&self) -> VmoUsage {
        self.usage
    }

    /// Number of active `Arc` references to the underlying `DmaBuffer`.
    #[inline]
    pub fn shared_ref_count(&self) -> usize {
        Arc::strong_count(&self.buffer)
    }

    /// Reference to the underlying contiguous `DmaBuffer`.
    #[inline]
    pub fn dma_buffer(&self) -> &DmaBuffer {
        &self.buffer
    }

    /// Page size constant (`4096` bytes).
    #[inline]
    pub const fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    /// Safely reads bytes from the `Vmo` at `offset` into `dst`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) -> Result<(), VmoError> {
        self.buffer.read_bytes(offset, dst).map_err(VmoError::from)
    }

    /// Safely writes bytes from `src` into the `Vmo` at `offset`.
    pub fn write(&self, offset: usize, src: &[u8]) -> Result<(), VmoError> {
        self.buffer.write_bytes(offset, src).map_err(VmoError::from)
    }
}

impl KernelObject for Vmo {
    fn koid(&self) -> Koid {
        self.koid
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Vmo
    }

    fn as_vmo(&self) -> Option<&Vmo> {
        Some(self)
    }
}
