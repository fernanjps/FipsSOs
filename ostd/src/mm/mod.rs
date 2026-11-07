pub mod dma;
pub mod frame;
pub mod heap;
pub mod vm_space;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use limine::memory_map::Entry;

pub use dma::{DmaBuffer, DmaError, VmFrame};
pub use frame::{FrameAllocator, MemoryStats};
pub use vm_space::{
    expand_user_brk, map_user_segment, map_vmo_to_user, protect_user_segment, read_user_bytes,
    reset_user_brk, validate_user_buffer, write_user_bytes, USER_HEAP_BASE_VADDR,
    USER_HEAP_MAX_VADDR, USER_MMAP_BASE_VADDR, USER_SPACE_LOW_BASE, USER_STACK_DEFAULT_BASE,
};

/// Standard hardware page/frame size (4 KiB).
pub const PAGE_SIZE: usize = 4096;

/// Initial kernel heap size (8 MiB = 2048 contiguous 4 KiB frames).
pub const KERNEL_HEAP_SIZE: usize = 8 * 1024 * 1024;

/// Physical memory address type.
pub type PhysAddr = u64;

/// Virtual memory address type.
pub type VirtAddr = u64;

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);
static MM_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initializes the physical frame allocator, HHDM translation layer, and global kernel heap.
pub fn init(hhdm_offset: u64, memory_map: &[&Entry]) {
    if MM_INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }

    HHDM_OFFSET.store(hhdm_offset, Ordering::Release);

    // 1. Initialize physical 4 KiB Bitmap Frame Allocator from Limine's memory map
    frame::init(memory_map);

    // 2. Allocate physically contiguous frames for the initial Kernel Heap and map via HHDM
    let heap_frames = KERNEL_HEAP_SIZE / PAGE_SIZE;
    let heap_phys = frame::allocate_contiguous_frames(heap_frames)
        .expect("No hay suficientes marcos fisicos contiguos para inicializar el Kernel Heap.");
    let heap_virt = phys_to_virt(heap_phys) as usize;

    // SAFETY: `heap_virt` points to `KERNEL_HEAP_SIZE` bytes of freshly allocated,
    // exclusively owned physical frames mapped in the Higher-Half Direct Map.
    unsafe {
        heap::init(heap_virt, KERNEL_HEAP_SIZE);
    }

    let stats = frame::stats();
    crate::early_println!(
        "[ostd::mm] HHDM offset={:#x} | RAM Usable={} MiB ({} frames libres, {} usados) | Heap={} MiB @ {:#x}",
        hhdm_offset,
        stats.total_bytes() / (1024 * 1024),
        stats.free_frames,
        stats.used_frames,
        KERNEL_HEAP_SIZE / (1024 * 1024),
        heap_virt
    );
}

/// Returns the active Higher-Half Direct Map (HHDM) virtual offset provided by Limine.
#[inline]
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Acquire)
}

/// Converts a physical address to its Higher-Half Direct Map (HHDM) virtual address.
#[inline]
pub fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    paddr + hhdm_offset()
}

/// Converts an HHDM virtual address back to its underlying physical address.
#[inline]
pub fn virt_to_phys(vaddr: VirtAddr) -> Option<PhysAddr> {
    let offset = hhdm_offset();
    if vaddr >= offset {
        Some(vaddr - offset)
    } else {
        None
    }
}

/// Returns a snapshot of current physical frame allocation statistics.
pub fn stats() -> MemoryStats {
    frame::stats()
}
