pub mod dma;
pub mod frame;
pub mod heap;
pub mod vm_space;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use limine::memory_map::Entry;

pub use dma::{DmaBuffer, DmaError, VmFrame};
pub use frame::{
    dealloc_frame, deallocate_frame, last_ram_probe_stats, live_ram_rw_probe,
    stats as memory_stats, FrameAllocator, MemoryStats, RamProbeStats,
};
pub use heap::stats as heap_stats;
pub use vm_space::{
    active_cr3, brk_additional_pages, clone_user_vm_space, count_user_pages, create_process_pml4,
    destroy_process_vm_space, expand_user_brk, map_anonymous_user_memory, map_kernel_mmio,
    map_user_segment, map_vmo_to_user, master_kernel_cr3, protect_user_segment, read_user_bytes,
    reset_user_brk, switch_cr3, unmap_user_pages, validate_user_buffer, write_user_bytes,
    ProcessVmSpace, USER_HEAP_BASE_VADDR, USER_HEAP_MAX_VADDR, USER_MMAP_BASE_VADDR,
    USER_SPACE_LOW_BASE, USER_STACK_DEFAULT_BASE,
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
static MM_INIT_STARTED: AtomicBool = AtomicBool::new(false);
static MM_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn select_initial_heap_bytes(free_bytes: usize) -> usize {
    let mib = 1024 * 1024;
    if free_bytes >= 4096 * mib {
        64 * mib
    } else if free_bytes >= 1024 * mib {
        32 * mib
    } else if free_bytes >= 384 * mib {
        16 * mib
    } else {
        KERNEL_HEAP_SIZE
    }
}

/// Initializes the physical frame allocator, HHDM translation layer, and global kernel heap.
pub fn init(hhdm_offset: u64, memory_map: &[&Entry]) {
    if MM_INIT_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    HHDM_OFFSET.store(hhdm_offset, Ordering::Release);

    // 1. Initialize physical 4 KiB Bitmap Frame Allocator from Limine's UEFI/E820 memory map
    frame::init(memory_map);
    let initial_stats = frame::stats();

    // 2. Dynamically scale and allocate contiguous frames for the initial Kernel Heap via HHDM
    let desired_heap = select_initial_heap_bytes(initial_stats.free_bytes());
    let candidates = [desired_heap, 16 * 1024 * 1024, KERNEL_HEAP_SIZE, 4 * 1024 * 1024];
    let mut allocated_heap: Option<(PhysAddr, usize)> = None;
    for &bytes in &candidates {
        if bytes <= desired_heap {
            let frames = bytes / PAGE_SIZE;
            if let Some(phys) = frame::allocate_contiguous_frames(frames) {
                allocated_heap = Some((phys, bytes));
                break;
            }
        }
    }
    let (heap_phys, actual_heap_bytes) = allocated_heap
        .expect("No hay suficientes marcos fisicos contiguos para inicializar el Kernel Heap.");
    let heap_virt = phys_to_virt(heap_phys) as usize;

    // SAFETY: `heap_virt` points to `actual_heap_bytes` bytes of freshly allocated,
    // exclusively owned physical frames mapped in the Higher-Half Direct Map.
    unsafe {
        heap::init(heap_virt, actual_heap_bytes);
    }
    MM_INITIALIZED.store(true, Ordering::Release);

    // 3. Record the bootloader's active Ring 0 PML4 as MASTER_KERNEL_PML4 for per-process CR3 cloning
    vm_space::init_kernel_vm_space();

    let stats = frame::stats();
    crate::early_println!(
        "[ostd::mm] HHDM offset={:#x} | MasterCR3={:#x} | RAM Usable={} MiB ({} frames libres, {} usados) | Heap={} MiB @ {:#x}",
        hhdm_offset,
        vm_space::master_kernel_cr3(),
        stats.total_bytes() / (1024 * 1024),
        stats.free_frames,
        stats.used_frames,
        actual_heap_bytes / (1024 * 1024),
        heap_virt
    );
}

#[inline]
pub(crate) fn is_initialized() -> bool {
    MM_INITIALIZED.load(Ordering::Acquire)
}

/// Returns total active kernel heap capacity in bytes (initial + dynamic expansions).
#[inline]
pub fn heap_bytes() -> usize {
    heap::total_bytes()
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
