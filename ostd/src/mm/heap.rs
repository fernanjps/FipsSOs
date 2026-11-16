use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static HEAP_TOTAL_BYTES: AtomicUsize = AtomicUsize::new(0);

const MIN_BLOCK_SIZE: usize = core::mem::size_of::<FreeBlock>();

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

/// Intrusive Free-List Heap Allocator with block splitting and address-ordered coalescing.
pub struct FreeListHeap {
    head: *mut FreeBlock,
}

// SAFETY: Access to `FreeListHeap` is serialized by `LockedHeap`'s spinlock.
unsafe impl Send for FreeListHeap {}

impl FreeListHeap {
    pub const fn new() -> Self {
        Self { head: null_mut() }
    }

    /// Initializes the heap with a contiguous virtual memory region `[heap_start, heap_start + heap_size)`.
    ///
    /// # Safety
    /// Caller must guarantee the memory range is valid, writable, exclusively owned, and aligned.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        let aligned_start = align_up(heap_start, core::mem::align_of::<FreeBlock>());
        let aligned_size = heap_size.saturating_sub(aligned_start - heap_start);
        assert!(
            aligned_size >= MIN_BLOCK_SIZE,
            "Region de Heap demasiado pequena."
        );

        let initial_block = aligned_start as *mut FreeBlock;
        // SAFETY: `initial_block` is properly aligned and within the exclusively owned heap region.
        unsafe {
            (*initial_block).size = aligned_size;
            (*initial_block).next = null_mut();
        }
        self.head = initial_block;
        HEAP_TOTAL_BYTES.store(aligned_size, Ordering::Release);
    }

    /// Appends a freshly allocated HHDM memory block to the free list, coalescing adjacent ranges.
    ///
    /// # Safety
    /// Caller must guarantee the region is valid, writable, exclusively owned, and mapped in HHDM.
    pub unsafe fn add_region(&mut self, region_start: usize, region_size: usize) -> usize {
        let aligned_start = align_up(region_start, core::mem::align_of::<FreeBlock>());
        let usable_size = region_size.saturating_sub(aligned_start - region_start);
        let aligned_size = usable_size & !(core::mem::align_of::<FreeBlock>() - 1);
        if aligned_size < MIN_BLOCK_SIZE {
            return 0;
        }
        // SAFETY: `aligned_size` satisfies `MIN_BLOCK_SIZE` and `FreeBlock` alignment.
        let layout = unsafe {
            Layout::from_size_align_unchecked(aligned_size, core::mem::align_of::<FreeBlock>())
        };
        // SAFETY: Inserts and coalesces the new memory region in address order.
        unsafe {
            self.deallocate(aligned_start as *mut u8, layout);
        }
        aligned_size
    }

    pub fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let size = align_up(
            layout.size().max(MIN_BLOCK_SIZE),
            core::mem::align_of::<FreeBlock>(),
        );
        let align = layout.align().max(core::mem::align_of::<FreeBlock>());

        let mut prev: *mut FreeBlock = null_mut();
        let mut current = self.head;

        while !current.is_null() {
            // SAFETY: `current` is a valid node in the intrusive free list.
            let block_addr = current as usize;
            let block_size = unsafe { (*current).size };
            let next_block = unsafe { (*current).next };

            let alloc_start = align_up(block_addr, align);
            let front_padding = alloc_start - block_addr;

            // If front padding exists, ensure it's either 0 or large enough to remain a FreeBlock
            let adjusted_start = if front_padding > 0 && front_padding < MIN_BLOCK_SIZE {
                align_up(block_addr + MIN_BLOCK_SIZE, align)
            } else {
                alloc_start
            };

            let total_front = adjusted_start - block_addr;
            if block_size >= total_front.saturating_add(size) {
                let remaining_back = block_size - total_front - size;
                let back_split = remaining_back >= MIN_BLOCK_SIZE;

                // Determine the replacement link for the current free node
                let mut replacement: *mut FreeBlock = next_block;

                if back_split {
                    let back_addr = adjusted_start + size;
                    let back_block = back_addr as *mut FreeBlock;
                    // SAFETY: `back_addr` is aligned to at least `FreeBlock` and within the current block.
                    unsafe {
                        (*back_block).size = remaining_back;
                        (*back_block).next = next_block;
                    }
                    replacement = back_block;
                }

                if total_front >= MIN_BLOCK_SIZE {
                    // Keep `current` as a shrunk front free block
                    // SAFETY: `current` remains valid at `block_addr`.
                    unsafe {
                        (*current).size = total_front;
                        (*current).next = replacement;
                    }
                } else if prev.is_null() {
                    self.head = replacement;
                } else {
                    // SAFETY: `prev` is a valid preceding block in the free list.
                    unsafe {
                        (*prev).next = replacement;
                    }
                }

                return adjusted_start as *mut u8;
            }

            prev = current;
            current = next_block;
        }

        null_mut()
    }

    pub unsafe fn deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        let size = align_up(
            layout.size().max(MIN_BLOCK_SIZE),
            core::mem::align_of::<FreeBlock>(),
        );
        let block_addr = ptr as usize;
        let new_block = block_addr as *mut FreeBlock;

        // Insert in address order to support immediate coalescing of adjacent blocks
        let mut prev: *mut FreeBlock = null_mut();
        let mut current = self.head;

        while !current.is_null() && (current as usize) < block_addr {
            prev = current;
            // SAFETY: `current` is a valid node in the free list.
            current = unsafe { (*current).next };
        }

        // SAFETY: `new_block` points to the freed allocation of at least `MIN_BLOCK_SIZE` bytes.
        unsafe {
            (*new_block).size = size;
            (*new_block).next = current;
        }

        if prev.is_null() {
            self.head = new_block;
        } else {
            // SAFETY: `prev` is valid.
            unsafe {
                (*prev).next = new_block;
            }
        }

        // Coalesce with next block if adjacent
        if !current.is_null() && (block_addr + size) == (current as usize) {
            // SAFETY: Both `new_block` and `current` are valid adjacent free blocks.
            unsafe {
                (*new_block).size += (*current).size;
                (*new_block).next = (*current).next;
            }
        }

        // Coalesce with previous block if adjacent
        if !prev.is_null() {
            // SAFETY: `prev` and `new_block` are valid free blocks.
            unsafe {
                if (prev as usize) + (*prev).size == block_addr {
                    (*prev).size += (*new_block).size;
                    (*prev).next = (*new_block).next;
                }
            }
        }
    }

    /// Sums the total free bytes currently available across the intrusive free list.
    pub fn free_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut current = self.head;
        let mut guard = 0usize;
        while !current.is_null() && guard < 131_072 {
            // SAFETY: `current` traverses blocks linked inside `self.head` while holding `LockedHeap`.
            unsafe {
                total = total.saturating_add((*current).size);
                current = (*current).next;
            }
            guard += 1;
        }
        total
    }
}

/// Spinlock-protected Global Heap Allocator for `ostd`.
pub struct LockedHeap {
    lock: AtomicBool,
    heap: core::cell::UnsafeCell<FreeListHeap>,
}

// SAFETY: `LockedHeap` synchronizes all access to `UnsafeCell<FreeListHeap>` using `lock`.
unsafe impl Sync for LockedHeap {}

impl LockedHeap {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            heap: core::cell::UnsafeCell::new(FreeListHeap::new()),
        }
    }

    fn with_lock<R>(&self, f: impl FnOnce(&mut FreeListHeap) -> R) -> R {
        crate::arch::without_interrupts(|| {
            while self
                .lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }

            // SAFETY: Exclusive access is guaranteed by `self.lock` and disabled interrupts.
            let result = unsafe { f(&mut *self.heap.get()) };

            self.lock.store(false, Ordering::Release);
            result
        })
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = self.with_lock(|heap| heap.allocate(layout));
        if !ptr.is_null() {
            return ptr;
        }
        if super::is_initialized() {
            let min_need = layout.size().saturating_add(layout.align()).max(super::PAGE_SIZE);
            let grow_bytes = min_need.max(2 * 1024 * 1024);
            let grow_frames = grow_bytes.div_ceil(super::PAGE_SIZE);
            if let Some(phys) = super::frame::allocate_contiguous_frames(grow_frames) {
                let virt = super::phys_to_virt(phys) as usize;
                let actual_bytes = grow_frames * super::PAGE_SIZE;
                return self.with_lock(|heap| {
                    // SAFETY: `virt..virt+actual_bytes` is freshly allocated HHDM memory from `frame`.
                    let added = unsafe { heap.add_region(virt, actual_bytes) };
                    HEAP_TOTAL_BYTES.fetch_add(added, Ordering::AcqRel);
                    heap.allocate(layout)
                });
            }
        }
        null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Caller guarantees `ptr` and `layout` match a previous allocation.
        self.with_lock(|heap| unsafe { heap.deallocate(ptr, layout) });
    }
}

#[global_allocator]
static GLOBAL_HEAP: LockedHeap = LockedHeap::new();

#[inline]
const fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

#[inline]
pub fn total_bytes() -> usize {
    HEAP_TOTAL_BYTES.load(Ordering::Acquire)
}

/// Returns live `(used_bytes, total_bytes)` for the global kernel heap.
pub fn stats() -> (usize, usize) {
    let total = total_bytes();
    let free = GLOBAL_HEAP.with_lock(|heap| heap.free_bytes());
    let used = total.saturating_sub(free);
    (used, total)
}

/// Initializes the global kernel heap allocator.
///
/// # Safety
/// Must be called once during `ostd::mm::init` with a valid, exclusively owned HHDM region.
pub(crate) unsafe fn init(heap_start: usize, heap_size: usize) {
    // SAFETY: Guaranteed by caller (`ostd::mm::init`).
    GLOBAL_HEAP.with_lock(|heap| unsafe { heap.init(heap_start, heap_size) });
}
