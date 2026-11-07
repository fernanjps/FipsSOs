use core::sync::atomic::{AtomicBool, Ordering};
use limine::memory_map::{Entry, EntryType};

use super::{phys_to_virt, PhysAddr, PAGE_SIZE};

/// Snapshot of physical frame allocator statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStats {
    pub total_usable_frames: usize,
    pub free_frames: usize,
    pub used_frames: usize,
}

impl MemoryStats {
    pub const fn total_bytes(&self) -> usize {
        self.total_usable_frames * PAGE_SIZE
    }

    pub const fn free_bytes(&self) -> usize {
        self.free_frames * PAGE_SIZE
    }

    pub const fn used_bytes(&self) -> usize {
        self.used_frames * PAGE_SIZE
    }
}

/// Bitmap-based physical 4 KiB frame allocator.
///
/// A bit value of `0` indicates a FREE frame; `1` indicates an ALLOCATED/RESERVED frame.
pub struct FrameAllocator {
    bitmap: &'static mut [u64],
    max_frames: usize,
    total_usable_frames: usize,
    free_frames: usize,
    search_hint_word: usize,
}

impl FrameAllocator {
    const fn empty() -> Self {
        Self {
            bitmap: &mut [],
            max_frames: 0,
            total_usable_frames: 0,
            free_frames: 0,
            search_hint_word: 0,
        }
    }

    #[inline]
    fn is_frame_allocated(&self, frame_idx: usize) -> bool {
        let word = frame_idx / 64;
        let bit = frame_idx % 64;
        (self.bitmap[word] & (1u64 << bit)) != 0
    }

    #[inline]
    fn set_frame_allocated(&mut self, frame_idx: usize) {
        let word = frame_idx / 64;
        let bit = frame_idx % 64;
        self.bitmap[word] |= 1u64 << bit;
    }

    #[inline]
    fn set_frame_free(&mut self, frame_idx: usize) {
        let word = frame_idx / 64;
        let bit = frame_idx % 64;
        self.bitmap[word] &= !(1u64 << bit);
    }

    /// Initializes the bitmap frame allocator using Limine's physical memory map.
    fn init_from_memory_map(&mut self, memory_map: &[&Entry]) {
        let mut max_phys_addr: u64 = 0;
        for entry in memory_map {
            if entry.entry_type == EntryType::USABLE {
                let end = entry.base + entry.length;
                if end > max_phys_addr {
                    max_phys_addr = end;
                }
            }
        }

        let max_frames = (max_phys_addr as usize).div_ceil(PAGE_SIZE);
        let bitmap_words = max_frames.div_ceil(64);
        let bitmap_bytes = bitmap_words * core::mem::size_of::<u64>();
        let bitmap_frames = bitmap_bytes.div_ceil(PAGE_SIZE);
        let bitmap_size_aligned = (bitmap_frames * PAGE_SIZE) as u64;

        // Locate a USABLE region (avoiding physical frame 0) large enough to host the bitmap
        let mut bitmap_phys_base: Option<u64> = None;
        for entry in memory_map {
            if entry.entry_type == EntryType::USABLE {
                let start = entry.base.max(PAGE_SIZE as u64);
                let end = entry.base + entry.length;
                if end > start && (end - start) >= bitmap_size_aligned {
                    bitmap_phys_base = Some(start);
                    break;
                }
            }
        }

        let bitmap_phys = bitmap_phys_base
            .expect("No se encontro una region USABLE para alojar el bitmap del FrameAllocator.");
        let bitmap_virt = phys_to_virt(bitmap_phys) as *mut u64;

        // SAFETY: `bitmap_virt` points to a valid, page-aligned USABLE physical memory region
        // mapped via HHDM with space for `bitmap_words` u64 elements.
        let bitmap_slice = unsafe { core::slice::from_raw_parts_mut(bitmap_virt, bitmap_words) };

        // Mark all frames as allocated/reserved (all bits = 1) initially
        for word in bitmap_slice.iter_mut() {
            *word = u64::MAX;
        }

        self.bitmap = bitmap_slice;
        self.max_frames = max_frames;
        self.total_usable_frames = 0;
        self.free_frames = 0;
        self.search_hint_word = 0;

        // Free all frames belonging to USABLE memory regions
        for entry in memory_map {
            if entry.entry_type == EntryType::USABLE {
                let start_frame = (entry.base as usize).div_ceil(PAGE_SIZE);
                let end_frame = ((entry.base + entry.length) as usize) / PAGE_SIZE;
                for frame_idx in start_frame..end_frame {
                    if frame_idx > 0 && frame_idx < self.max_frames {
                        if self.is_frame_allocated(frame_idx) {
                            self.set_frame_free(frame_idx);
                            self.total_usable_frames += 1;
                            self.free_frames += 1;
                        }
                    }
                }
            }
        }

        // Reserve the frames occupied by the bitmap itself
        let bitmap_start_frame = (bitmap_phys as usize) / PAGE_SIZE;
        for frame_idx in bitmap_start_frame..(bitmap_start_frame + bitmap_frames) {
            if frame_idx < self.max_frames && !self.is_frame_allocated(frame_idx) {
                self.set_frame_allocated(frame_idx);
                self.free_frames -= 1;
            }
        }
    }

    /// Allocates `count` physically contiguous 4 KiB frames and zeroes their contents.
    pub fn allocate_contiguous(&mut self, count: usize) -> Option<PhysAddr> {
        if count == 0 || count > self.free_frames {
            return None;
        }

        let start_frame = self.find_contiguous_free_frames(count)?;
        for idx in start_frame..(start_frame + count) {
            self.set_frame_allocated(idx);
        }
        self.free_frames -= count;
        self.search_hint_word = (start_frame + count) / 64;

        let paddr = (start_frame * PAGE_SIZE) as PhysAddr;
        let vaddr = phys_to_virt(paddr) as *mut u8;

        // SAFETY: The newly allocated physical frames are mapped in HHDM and exclusively owned.
        unsafe {
            core::ptr::write_bytes(vaddr, 0, count * PAGE_SIZE);
        }

        Some(paddr)
    }

    fn find_contiguous_free_frames(&self, count: usize) -> Option<usize> {
        let total_words = self.bitmap.len();
        let hint_frame = (self.search_hint_word * 64).min(self.max_frames);

        if let Some(found) = self.scan_range(hint_frame, self.max_frames, count) {
            return Some(found);
        }
        if hint_frame > 0 {
            return self.scan_range(1, (hint_frame + count).min(self.max_frames), count);
        }
        let _ = total_words;
        None
    }

    fn scan_range(&self, start_frame: usize, end_frame: usize, count: usize) -> Option<usize> {
        let mut run_start = start_frame;
        let mut run_len = 0usize;
        let mut idx = start_frame;

        while idx < end_frame {
            // Fast-skip fully allocated 64-frame words when not currently in a free run
            if run_len == 0 && (idx % 64 == 0) {
                let word_idx = idx / 64;
                if word_idx < self.bitmap.len() && self.bitmap[word_idx] == u64::MAX {
                    idx += 64;
                    run_start = idx;
                    continue;
                }
            }

            if !self.is_frame_allocated(idx) {
                if run_len == 0 {
                    run_start = idx;
                }
                run_len += 1;
                if run_len == count {
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
            idx += 1;
        }
        None
    }

    /// Frees `count` physically contiguous 4 KiB frames starting at `base_paddr`.
    pub fn deallocate_contiguous(&mut self, base_paddr: PhysAddr, count: usize) {
        let start_frame = (base_paddr as usize) / PAGE_SIZE;
        for idx in start_frame..(start_frame + count) {
            if idx > 0 && idx < self.max_frames && self.is_frame_allocated(idx) {
                self.set_frame_free(idx);
                self.free_frames += 1;
            }
        }
        let word = start_frame / 64;
        if word < self.search_hint_word {
            self.search_hint_word = word;
        }
    }

    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            total_usable_frames: self.total_usable_frames,
            free_frames: self.free_frames,
            used_frames: self.total_usable_frames.saturating_sub(self.free_frames),
        }
    }
}

static FRAME_LOCK: AtomicBool = AtomicBool::new(false);
static mut GLOBAL_FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::empty();

fn with_allocator<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> R {
    while FRAME_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }

    // SAFETY: Protected by `FRAME_LOCK` atomic spinlock.
    let result = unsafe { f(&mut *(&raw mut GLOBAL_FRAME_ALLOCATOR)) };

    FRAME_LOCK.store(false, Ordering::Release);
    result
}

pub(crate) fn init(memory_map: &[&Entry]) {
    with_allocator(|alloc| alloc.init_from_memory_map(memory_map));
}

/// Allocates a single zeroed 4 KiB physical frame.
pub fn allocate_frame() -> Option<PhysAddr> {
    allocate_contiguous_frames(1)
}

/// Allocates `count` physically contiguous zeroed 4 KiB frames.
pub fn allocate_contiguous_frames(count: usize) -> Option<PhysAddr> {
    with_allocator(|alloc| alloc.allocate_contiguous(count))
}

/// Deallocates `count` physically contiguous 4 KiB frames starting at `base_paddr`.
pub fn deallocate_contiguous_frames(base_paddr: PhysAddr, count: usize) {
    with_allocator(|alloc| alloc.deallocate_contiguous(base_paddr, count));
}

/// Returns current physical memory allocation statistics.
pub fn stats() -> MemoryStats {
    with_allocator(|alloc| alloc.stats())
}
