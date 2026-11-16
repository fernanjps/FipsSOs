use core::sync::atomic::{AtomicBool, Ordering};
use limine::memory_map::{Entry, EntryType};

use super::{phys_to_virt, PhysAddr, PAGE_SIZE};

/// Maximum physical address tracked by the bitmap allocator (32 GiB).
const MAX_TRACKED_PHYS_ADDR: u64 = 32 * 1024 * 1024 * 1024;
/// Protect low physical memory (< 1 MiB) including IVT/BIOS areas and the 0x8000 SMP AP trampoline.
const MIN_ALLOCATABLE_FRAME: usize = (1024 * 1024) / PAGE_SIZE;

fn e820_type_label(entry_type: EntryType) -> &'static str {
    if entry_type == EntryType::USABLE {
        "USABLE"
    } else if entry_type == EntryType::RESERVED {
        "RESERVED"
    } else if entry_type == EntryType::ACPI_RECLAIMABLE {
        "ACPI_RECLAIMABLE"
    } else if entry_type == EntryType::ACPI_NVS {
        "ACPI_NVS"
    } else if entry_type == EntryType::BOOTLOADER_RECLAIMABLE {
        "BOOTLOADER_RECLAIMABLE"
    } else if entry_type == EntryType::FRAMEBUFFER {
        "FRAMEBUFFER"
    } else {
        "FIRMWARE/KERNEL"
    }
}

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
        let mut total_e820_usable: u64 = 0;
        for (idx, entry) in memory_map.iter().enumerate() {
            let end = entry.base.saturating_add(entry.length);
            crate::early_println!(
                "[ostd::mm::e820] #{:02} base={:#012x} end={:#012x} ({} KiB) type={}",
                idx,
                entry.base,
                end,
                entry.length / 1024,
                e820_type_label(entry.entry_type)
            );
            if entry.entry_type == EntryType::USABLE {
                total_e820_usable = total_e820_usable.saturating_add(entry.length);
                if end > max_phys_addr {
                    max_phys_addr = end;
                }
            }
        }
        if max_phys_addr > MAX_TRACKED_PHYS_ADDR {
            crate::early_println!(
                "[ostd::mm::e820] Limite bitmap 32 GiB (fisico maximo firmware={:#x}, usable={} MiB)",
                max_phys_addr,
                total_e820_usable / (1024 * 1024)
            );
            max_phys_addr = MAX_TRACKED_PHYS_ADDR;
        }

        let max_frames = (max_phys_addr as usize).div_ceil(PAGE_SIZE);
        let bitmap_words = max_frames.div_ceil(64);
        let bitmap_bytes = bitmap_words * core::mem::size_of::<u64>();
        let bitmap_frames = bitmap_bytes.div_ceil(PAGE_SIZE);
        let bitmap_size_aligned = (bitmap_frames * PAGE_SIZE) as u64;

        // Locate a USABLE region (>= 1 MiB preferred to protect BIOS/trampoline) large enough for the bitmap
        let mut bitmap_phys_base: Option<u64> = None;
        for min_base in [(1024 * 1024) as u64, PAGE_SIZE as u64] {
            for entry in memory_map {
                if entry.entry_type == EntryType::USABLE && entry.base < max_phys_addr {
                    let start = entry.base.max(min_base);
                    let end = entry.base.saturating_add(entry.length).min(max_phys_addr);
                    if end > start && (end - start) >= bitmap_size_aligned {
                        bitmap_phys_base = Some(start);
                        break;
                    }
                }
            }
            if bitmap_phys_base.is_some() {
                break;
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
        self.search_hint_word = MIN_ALLOCATABLE_FRAME / 64;

        // Free frames belonging to USABLE memory regions (>= 1 MiB to preserve trampoline/IVT)
        for entry in memory_map {
            if entry.entry_type == EntryType::USABLE && entry.base < max_phys_addr {
                let start_frame = ((entry.base as usize).div_ceil(PAGE_SIZE)).max(MIN_ALLOCATABLE_FRAME);
                let capped_end = entry.base.saturating_add(entry.length).min(max_phys_addr);
                let end_frame = ((capped_end as usize) / PAGE_SIZE).min(self.max_frames);
                for frame_idx in start_frame..end_frame {
                    if self.is_frame_allocated(frame_idx) {
                        self.set_frame_free(frame_idx);
                        self.total_usable_frames += 1;
                        self.free_frames += 1;
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
        if hint_frame > MIN_ALLOCATABLE_FRAME {
            return self.scan_range(
                MIN_ALLOCATABLE_FRAME,
                (hint_frame + count).min(self.max_frames),
                count,
            );
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
            if idx >= MIN_ALLOCATABLE_FRAME && idx < self.max_frames && self.is_frame_allocated(idx) {
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
    crate::arch::without_interrupts(|| {
        while FRAME_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        // SAFETY: Protected by `FRAME_LOCK` atomic spinlock with local IRQs masked.
        let result = unsafe { f(&mut *(&raw mut GLOBAL_FRAME_ALLOCATOR)) };

        FRAME_LOCK.store(false, Ordering::Release);
        result
    })
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

/// Deallocates a single 4 KiB physical frame starting at `paddr`.
#[inline]
pub fn deallocate_frame(paddr: PhysAddr) {
    deallocate_contiguous_frames(paddr, 1);
}

/// Alias explícito para liberar un marco físico de 4 KiB hacia el bitmap global.
#[inline]
pub fn dealloc_frame(paddr: PhysAddr) {
    deallocate_frame(paddr);
}

/// Returns current physical memory allocation statistics.
pub fn stats() -> MemoryStats {
    with_allocator(|alloc| alloc.stats())
}

/// Resultado de una sonda en vivo de lectura/escritura directa en RAM física (HHDM).
#[derive(Debug, Clone, Copy)]
pub struct RamProbeStats {
    pub pages_tested: u32,
    pub bytes_tested: u64,
    pub tsc_cycles: u64,
    pub mbps: u32,
}

static LAST_RAM_RW_MBPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static LAST_RAM_RW_PAGES_OK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Devuelve `(ultimo_mbps, ultimas_paginas_verificadas)` de la sonda RAM R/W más reciente.
#[inline]
pub fn last_ram_probe_stats() -> (u32, u32) {
    (
        LAST_RAM_RW_MBPS.load(Ordering::Acquire),
        LAST_RAM_RW_PAGES_OK.load(Ordering::Acquire),
    )
}

#[inline]
fn ram_probe_word_pattern(paddr: PhysAddr, page_idx: usize, word_idx: usize) -> u64 {
    let salt = paddr
        .wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (page_idx as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    salt ^ ((word_idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ 0xA55A_5AA5_F00D_CAFE
}

/// Asigna `num_pages` marcos físicos reales de 4 KiB, escribe y verifica patrones de 64 bits
/// directamente vía HHDM midiendo ciclos TSC reales, y libera cada marco sin fugas.
pub fn live_ram_rw_probe(num_pages: usize) -> Result<RamProbeStats, &'static str> {
    if num_pages == 0 || num_pages > 2048 {
        return Err("live_ram_rw_probe: num_pages fuera de rango (1..=2048)");
    }

    let mut frames: alloc::vec::Vec<PhysAddr> = alloc::vec::Vec::with_capacity(num_pages);
    for _ in 0..num_pages {
        if let Some(pa) = allocate_frame() {
            frames.push(pa);
        } else {
            for &pa in frames.iter().rev() {
                dealloc_frame(pa);
            }
            return Err("live_ram_rw_probe: sin marcos fisicos libres suficientes");
        }
    }

    let t0 = crate::time::read_tsc();
    for (page_idx, &pa) in frames.iter().enumerate() {
        let virt = super::phys_to_virt(pa) as *mut u64;
        // SAFETY: `pa` es un marco físico de 4 KiB recién asignado y de propiedad exclusiva,
        // mapeado linealmente en el HHDM; escribimos exactamente 512 u64 (4096 bytes).
        unsafe {
            for w in 0..(PAGE_SIZE / 8) {
                virt.add(w).write_volatile(ram_probe_word_pattern(pa, page_idx, w));
            }
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);

    let mut mismatch = false;
    for (page_idx, &pa) in frames.iter().enumerate() {
        let virt = super::phys_to_virt(pa) as *const u64;
        // SAFETY: `pa` sigue siendo de propiedad exclusiva hasta liberar al final de esta función;
        // leemos exactamente 512 u64 dentro del marco de 4096 bytes.
        unsafe {
            for w in 0..(PAGE_SIZE / 8) {
                let got = virt.add(w).read_volatile();
                if got != ram_probe_word_pattern(pa, page_idx, w) {
                    mismatch = true;
                    break;
                }
            }
        }
        if mismatch {
            break;
        }
    }
    let t1 = crate::time::read_tsc();

    for &pa in frames.iter().rev() {
        dealloc_frame(pa);
    }

    if mismatch {
        return Err("live_ram_rw_probe: discrepancia de datos al verificar RAM fisica");
    }

    let cycles = t1.saturating_sub(t0).max(1);
    let bytes_tested = (num_pages as u64) * (PAGE_SIZE as u64);
    let rw_bytes = bytes_tested.saturating_mul(2);
    let hz = crate::time::tsc_hz().max(1_000_000);
    let bytes_per_sec = ((rw_bytes as u128).saturating_mul(hz as u128)) / (cycles as u128);
    let mbps = ((bytes_per_sec / (1024 * 1024)).clamp(1, u32::MAX as u128)) as u32;

    LAST_RAM_RW_MBPS.store(mbps, Ordering::Release);
    LAST_RAM_RW_PAGES_OK.store(num_pages as u32, Ordering::Release);

    Ok(RamProbeStats {
        pages_tested: num_pages as u32,
        bytes_tested,
        tsc_cycles: cycles,
        mbps,
    })
}
