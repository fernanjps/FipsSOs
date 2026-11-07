use super::{
    frame::{allocate_contiguous_frames, allocate_frame, deallocate_contiguous_frames},
    phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE,
};

/// Errors that can occur when allocating or accessing a `DmaBuffer` or `VmFrame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    /// Requested size was zero.
    ZeroSize,
    /// Not enough contiguous physical frames available in the `FrameAllocator`.
    OutOfContiguousMemory,
    /// Read/write offset and length exceed the bounds of the buffer.
    OutOfBounds {
        offset: usize,
        len: usize,
        capacity: usize,
    },
}

/// Safe RAII handle to a single 4 KiB physical memory frame (`PAGE_SIZE`).
#[derive(Debug)]
pub struct VmFrame {
    paddr: PhysAddr,
}

impl VmFrame {
    /// Allocates a single zero-initialized 4 KiB physical frame.
    pub fn allocate() -> Result<Self, DmaError> {
        let paddr = allocate_frame().ok_or(DmaError::OutOfContiguousMemory)?;
        Ok(Self { paddr })
    }

    /// Physical address of this 4 KiB frame.
    #[inline]
    pub const fn paddr(&self) -> PhysAddr {
        self.paddr
    }

    /// Higher-Half Direct Map (HHDM) virtual address of this 4 KiB frame.
    #[inline]
    pub fn vaddr(&self) -> VirtAddr {
        phys_to_virt(self.paddr)
    }

    /// Size of the frame in bytes (`4096`).
    #[inline]
    pub const fn size_bytes(&self) -> usize {
        PAGE_SIZE
    }

    /// Reads bytes from the frame at `offset` into `dst`.
    pub fn read_bytes(&self, offset: usize, dst: &mut [u8]) -> Result<(), DmaError> {
        let end = offset.checked_add(dst.len()).ok_or(DmaError::OutOfBounds {
            offset,
            len: dst.len(),
            capacity: PAGE_SIZE,
        })?;
        if end > PAGE_SIZE {
            return Err(DmaError::OutOfBounds {
                offset,
                len: dst.len(),
                capacity: PAGE_SIZE,
            });
        }

        let src_ptr = (self.vaddr() as *const u8).wrapping_add(offset);
        for (i, byte) in dst.iter_mut().enumerate() {
            // SAFETY: Bounds checked against `PAGE_SIZE` and mapped via HHDM.
            *byte = unsafe { core::ptr::read_volatile(src_ptr.add(i)) };
        }
        Ok(())
    }

    /// Writes bytes from `src` into the frame at `offset`.
    pub fn write_bytes(&self, offset: usize, src: &[u8]) -> Result<(), DmaError> {
        let end = offset.checked_add(src.len()).ok_or(DmaError::OutOfBounds {
            offset,
            len: src.len(),
            capacity: PAGE_SIZE,
        })?;
        if end > PAGE_SIZE {
            return Err(DmaError::OutOfBounds {
                offset,
                len: src.len(),
                capacity: PAGE_SIZE,
            });
        }

        let dst_ptr = (self.vaddr() as *mut u8).wrapping_add(offset);
        for (i, &byte) in src.iter().enumerate() {
            // SAFETY: Bounds checked against `PAGE_SIZE` and mapped via HHDM.
            unsafe {
                core::ptr::write_volatile(dst_ptr.add(i), byte);
            }
        }
        Ok(())
    }
}

impl Drop for VmFrame {
    fn drop(&mut self) {
        deallocate_contiguous_frames(self.paddr, 1);
    }
}

/// Safe RAII contiguous physical memory buffer for Zero-Copy sharing across CPU, GPU (DRM),
/// and NPU (`/dev/accel`) DMA engines.
#[derive(Debug)]
pub struct DmaBuffer {
    paddr: PhysAddr,
    frame_count: usize,
    size_bytes: usize,
}

impl DmaBuffer {
    /// Allocates a zero-initialized, physically contiguous buffer of at least `requested_bytes`
    /// (rounded up to a multiple of `PAGE_SIZE` = 4 KiB).
    pub fn allocate(requested_bytes: usize) -> Result<Self, DmaError> {
        if requested_bytes == 0 {
            return Err(DmaError::ZeroSize);
        }

        let frame_count = requested_bytes.div_ceil(PAGE_SIZE);
        let size_bytes = frame_count * PAGE_SIZE;
        let paddr =
            allocate_contiguous_frames(frame_count).ok_or(DmaError::OutOfContiguousMemory)?;

        Ok(Self {
            paddr,
            frame_count,
            size_bytes,
        })
    }

    /// Base physical address of the contiguous DMA region (suitable for PCIe/IOMMU/NPU descriptors).
    #[inline]
    pub const fn paddr(&self) -> PhysAddr {
        self.paddr
    }

    /// Base virtual address of the contiguous DMA region in the Higher-Half Direct Map (HHDM).
    #[inline]
    pub fn vaddr(&self) -> VirtAddr {
        phys_to_virt(self.paddr)
    }

    /// Number of contiguous 4 KiB physical frames backing this buffer.
    #[inline]
    pub const fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Total page-aligned capacity of this buffer in bytes.
    #[inline]
    pub const fn size_bytes(&self) -> usize {
        self.size_bytes
    }

    /// Safely copies `dst.len()` bytes starting from `offset` inside the DMA buffer into `dst`.
    pub fn read_bytes(&self, offset: usize, dst: &mut [u8]) -> Result<(), DmaError> {
        let end = offset.checked_add(dst.len()).ok_or(DmaError::OutOfBounds {
            offset,
            len: dst.len(),
            capacity: self.size_bytes,
        })?;
        if end > self.size_bytes {
            return Err(DmaError::OutOfBounds {
                offset,
                len: dst.len(),
                capacity: self.size_bytes,
            });
        }

        let src_ptr = (self.vaddr() as *const u8).wrapping_add(offset);
        for (i, slot) in dst.iter_mut().enumerate() {
            // SAFETY: `offset + i < self.size_bytes` and memory is mapped in HHDM.
            *slot = unsafe { core::ptr::read_volatile(src_ptr.add(i)) };
        }
        Ok(())
    }

    /// Safely writes `src.len()` bytes from `src` into the DMA buffer starting at `offset`.
    pub fn write_bytes(&self, offset: usize, src: &[u8]) -> Result<(), DmaError> {
        let end = offset.checked_add(src.len()).ok_or(DmaError::OutOfBounds {
            offset,
            len: src.len(),
            capacity: self.size_bytes,
        })?;
        if end > self.size_bytes {
            return Err(DmaError::OutOfBounds {
                offset,
                len: src.len(),
                capacity: self.size_bytes,
            });
        }

        let dst_ptr = (self.vaddr() as *mut u8).wrapping_add(offset);
        for (i, &byte) in src.iter().enumerate() {
            // SAFETY: `offset + i < self.size_bytes` and memory is mapped in HHDM.
            unsafe {
                core::ptr::write_volatile(dst_ptr.add(i), byte);
            }
        }
        Ok(())
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        deallocate_contiguous_frames(self.paddr, self.frame_count);
    }
}
