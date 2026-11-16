#![forbid(unsafe_code)]

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use ostd::mm::{virt_to_phys, DmaBuffer, DmaError, PhysAddr, VirtAddr, PAGE_SIZE};
use ostd::sync::SpinLock;

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
    /// Attempted write to a read-only Zero-Copy static VMO.
    ReadOnly,
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
enum VmoBacking {
    Contiguous(Arc<DmaBuffer>),
    StaticSlice(&'static [u8]),
    Paged(Arc<SpinLock<Vec<u8>>>),
}

/// Virtual Memory Object (`Vmo`) wrapping physical frames or Zero-Copy slices via `Arc`.
///
/// Supports Zero-Copy sharing across user processes, GPU DRM buffers (Vulkan textures),
/// NPU accelerator tensors (`/dev/accel`), and Zero-Copy `initramfs` payloads.
pub struct Vmo {
    koid: Koid,
    requested_size: usize,
    usage: VmoUsage,
    backing: VmoBacking,
}

impl fmt::Debug for Vmo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vmo")
            .field("koid", &self.koid)
            .field("requested_size", &self.requested_size)
            .field("usage", &self.usage)
            .finish()
    }
}

impl Vmo {
    /// Allocates a new `Vmo`. `ZeroCopyAccel`, `SharedRing`, and `ProcessMemory` use physically
    /// contiguous `DmaBuffer` frames; `InitramfsFile` uses paged heap memory to avoid exhausting
    /// contiguous physical frames.
    pub fn new(requested_size: usize, usage: VmoUsage) -> Result<Arc<Self>, VmoError> {
        let backing = match usage {
            VmoUsage::InitramfsFile => {
                let len = requested_size.max(1);
                VmoBacking::Paged(Arc::new(SpinLock::new(vec![0u8; len])))
            }
            _ => VmoBacking::Contiguous(Arc::new(DmaBuffer::allocate(requested_size)?)),
        };
        Ok(Arc::new(Self {
            koid: Koid::allocate(),
            requested_size,
            usage,
            backing,
        }))
    }

    /// Creates a Zero-Copy read-only `Vmo` backed directly by a `'static` slice (e.g. Limine
    /// `initramfs.tar` in HHDM) without allocating or copying any extra physical frames.
    pub fn from_static_slice(slice: &'static [u8], usage: VmoUsage) -> Arc<Self> {
        Arc::new(Self {
            koid: Koid::allocate(),
            requested_size: slice.len(),
            usage,
            backing: VmoBacking::StaticSlice(slice),
        })
    }

    /// Creates a child/shared `Vmo` that shares the exact same underlying backing
    /// (Zero-Copy) with a new `Koid`.
    pub fn clone_shared(&self) -> Arc<Self> {
        let backing = match &self.backing {
            VmoBacking::Contiguous(buf) => VmoBacking::Contiguous(Arc::clone(buf)),
            VmoBacking::StaticSlice(s) => VmoBacking::StaticSlice(*s),
            VmoBacking::Paged(v) => VmoBacking::Paged(Arc::clone(v)),
        };
        Arc::new(Self {
            koid: Koid::allocate(),
            requested_size: self.requested_size,
            usage: self.usage,
            backing,
        })
    }

    /// Requested size in bytes when this `Vmo` was created.
    #[inline]
    pub fn requested_size(&self) -> usize {
        self.requested_size
    }

    /// Actual capacity in bytes.
    #[inline]
    pub fn size_bytes(&self) -> usize {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.size_bytes(),
            VmoBacking::StaticSlice(s) => s.len(),
            VmoBacking::Paged(v) => v.lock().len(),
        }
    }

    /// Number of 4 KiB physical frames backing this `Vmo`.
    #[inline]
    pub fn frame_count(&self) -> usize {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.frame_count(),
            VmoBacking::StaticSlice(s) => s.len().div_ceil(PAGE_SIZE),
            VmoBacking::Paged(v) => v.lock().len().div_ceil(PAGE_SIZE),
        }
    }

    /// Base physical address of the backing memory (HHDM-translated for static/paged VMOs).
    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.paddr(),
            VmoBacking::StaticSlice(s) => virt_to_phys(s.as_ptr() as VirtAddr).unwrap_or(0),
            VmoBacking::Paged(v) => {
                let g = v.lock();
                virt_to_phys(g.as_ptr() as VirtAddr).unwrap_or(0)
            }
        }
    }

    /// Base virtual address in the Higher-Half Direct Map (HHDM).
    #[inline]
    pub fn virt_addr(&self) -> VirtAddr {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.vaddr(),
            VmoBacking::StaticSlice(s) => s.as_ptr() as VirtAddr,
            VmoBacking::Paged(v) => v.lock().as_ptr() as VirtAddr,
        }
    }

    /// Usage profile of this `Vmo`.
    #[inline]
    pub fn usage(&self) -> VmoUsage {
        self.usage
    }

    /// Number of active `Arc` references to the underlying backing.
    #[inline]
    pub fn shared_ref_count(&self) -> usize {
        match &self.backing {
            VmoBacking::Contiguous(buf) => Arc::strong_count(buf),
            VmoBacking::StaticSlice(_) => 1,
            VmoBacking::Paged(v) => Arc::strong_count(v),
        }
    }

    /// Reference to the underlying contiguous `DmaBuffer`, if contiguous.
    #[inline]
    pub fn try_dma_buffer(&self) -> Option<&DmaBuffer> {
        match &self.backing {
            VmoBacking::Contiguous(buf) => Some(buf),
            _ => None,
        }
    }

    /// Reference to the underlying contiguous `DmaBuffer`.
    #[inline]
    pub fn dma_buffer(&self) -> &DmaBuffer {
        self.try_dma_buffer()
            .expect("dma_buffer() invocado sobre un Vmo no contiguo")
    }

    /// Page size constant (`4096` bytes).
    #[inline]
    pub const fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    /// Safely reads bytes from the `Vmo` at `offset` into `dst`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) -> Result<(), VmoError> {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.read_bytes(offset, dst).map_err(VmoError::from),
            VmoBacking::StaticSlice(s) => {
                let end = offset.checked_add(dst.len()).ok_or(VmoError::OutOfBounds {
                    offset,
                    len: dst.len(),
                    size: s.len(),
                })?;
                if end > s.len() {
                    return Err(VmoError::OutOfBounds {
                        offset,
                        len: dst.len(),
                        size: s.len(),
                    });
                }
                dst.copy_from_slice(&s[offset..end]);
                Ok(())
            }
            VmoBacking::Paged(v) => {
                let g = v.lock();
                let end = offset.checked_add(dst.len()).ok_or(VmoError::OutOfBounds {
                    offset,
                    len: dst.len(),
                    size: g.len(),
                })?;
                if end > g.len() {
                    return Err(VmoError::OutOfBounds {
                        offset,
                        len: dst.len(),
                        size: g.len(),
                    });
                }
                dst.copy_from_slice(&g[offset..end]);
                Ok(())
            }
        }
    }

    /// Safely writes bytes from `src` into the `Vmo` at `offset`.
    pub fn write(&self, offset: usize, src: &[u8]) -> Result<(), VmoError> {
        match &self.backing {
            VmoBacking::Contiguous(buf) => buf.write_bytes(offset, src).map_err(VmoError::from),
            VmoBacking::StaticSlice(_) => Err(VmoError::ReadOnly),
            VmoBacking::Paged(v) => {
                let mut g = v.lock();
                let end = offset.checked_add(src.len()).ok_or(VmoError::OutOfBounds {
                    offset,
                    len: src.len(),
                    size: g.len(),
                })?;
                if end > g.len() {
                    return Err(VmoError::OutOfBounds {
                        offset,
                        len: src.len(),
                        size: g.len(),
                    });
                }
                g[offset..end].copy_from_slice(src);
                Ok(())
            }
        }
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
