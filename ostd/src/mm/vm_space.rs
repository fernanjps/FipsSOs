use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    frame::allocate_frame,
    phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::sync::SpinLock;

/// Canonical Lower-Half User Space base for ELF `PT_LOAD` segments (`0x0040_0000`).
pub const USER_SPACE_LOW_BASE: VirtAddr = 0x0000_0000_0040_0000;

/// Canonical Lower-Half User Space default stack base (`0x0080_0000`).
pub const USER_STACK_DEFAULT_BASE: VirtAddr = 0x0000_0000_0080_0000;

/// Canonical Lower-Half User Space dynamic Heap base for `sys_brk` (`0x0200_0000`).
pub const USER_HEAP_BASE_VADDR: VirtAddr = 0x0000_0000_0200_0000;

/// Maximum Lower-Half User Space dynamic Heap ceiling (`0x0800_0000`, 96 MiB).
pub const USER_HEAP_MAX_VADDR: VirtAddr = 0x0000_0000_0800_0000;

/// Canonical Lower-Half User Space base for `sys_mmap` Zero-Copy VMO mappings (`0x1000_0000`).
pub const USER_MMAP_BASE_VADDR: VirtAddr = 0x0000_0000_1000_0000;

/// Upper bound of canonical Lower-Half User Space (`0x0000_7FFF_FFFF_FFFF`).
pub const USER_SPACE_MAX_VADDR: VirtAddr = 0x0000_7FFF_FFFF_FFFF;

static NEXT_USER_MMAP_VADDR: AtomicU64 = AtomicU64::new(USER_MMAP_BASE_VADDR);
static CURRENT_USER_BRK: AtomicU64 = AtomicU64::new(USER_HEAP_BASE_VADDR);
static MAPPED_USER_BRK_END: AtomicU64 = AtomicU64::new(USER_HEAP_BASE_VADDR);

/// Tracked virtual memory region mapped in Lower-Half User Space for the active process.
#[derive(Debug, Clone, Copy)]
pub struct UserVmRegion {
    pub start_vaddr: VirtAddr,
    pub size_bytes: usize,
    pub writable: bool,
    pub executable: bool,
}

static USER_VM_REGIONS: SpinLock<Vec<UserVmRegion>> = SpinLock::new(Vec::new());

/// Registers or updates a Lower-Half User Space virtual memory range for bounds validation.
pub fn register_user_region(
    start_vaddr: VirtAddr,
    size_bytes: usize,
    writable: bool,
    executable: bool,
) {
    let mut regions = USER_VM_REGIONS.lock();
    for reg in regions.iter_mut() {
        if reg.start_vaddr == start_vaddr && reg.size_bytes == size_bytes {
            reg.writable = writable;
            reg.executable = executable;
            return;
        }
    }
    regions.push(UserVmRegion {
        start_vaddr,
        size_bytes,
        writable,
        executable,
    });
}

/// Validates whether `[user_vaddr, user_vaddr + len)` lies entirely within a mapped Lower-Half
/// User Space region (ELF segments, User Stack, or `mmap` VMO regions).
pub fn validate_user_buffer(
    user_vaddr: VirtAddr,
    len: usize,
    require_write: bool,
) -> Result<(), &'static str> {
    if len == 0 {
        return Ok(());
    }
    let end = user_vaddr
        .checked_add(len as u64)
        .ok_or("Overflow en rango de direccion de usuario")?;

    if user_vaddr < USER_SPACE_LOW_BASE || end > USER_SPACE_MAX_VADDR {
        return Err("Direccion fuera del espacio Lower-Half de usuario");
    }

    let regions = USER_VM_REGIONS.lock();
    for reg in regions.iter() {
        let reg_end = reg.start_vaddr + (reg.size_bytes as u64);
        if user_vaddr >= reg.start_vaddr && end <= reg_end {
            if require_write && !reg.writable {
                return Err("La region de usuario no tiene permisos de escritura");
            }
            return Ok(());
        }
    }

    Err("El buffer de usuario no pertenece a ninguna region mapeada (ELF/Stack/mmap)")
}

/// Safely reads bytes from any mapped Lower-Half User Space buffer into `dst`.
pub fn read_user_bytes(user_vaddr: VirtAddr, dst: &mut [u8]) -> Result<(), &'static str> {
    validate_user_buffer(user_vaddr, dst.len(), false)?;

    let src_ptr = user_vaddr as *const u8;
    for (i, slot) in dst.iter_mut().enumerate() {
        // SAFETY: Validated within a mapped Lower-Half User Space region.
        *slot = unsafe { core::ptr::read_volatile(src_ptr.add(i)) };
    }
    Ok(())
}

/// Safely writes bytes from `src` into a writable Lower-Half User Space buffer.
pub fn write_user_bytes(user_vaddr: VirtAddr, src: &[u8]) -> Result<(), &'static str> {
    validate_user_buffer(user_vaddr, src.len(), true)?;

    let dst_ptr = user_vaddr as *mut u8;
    for (i, &byte) in src.iter().enumerate() {
        // SAFETY: Validated within a writable Lower-Half User Space region.
        unsafe {
            core::ptr::write_volatile(dst_ptr.add(i), byte);
        }
    }
    Ok(())
}

/// Maps a contiguous physical range (`phys_addr`, `size_bytes`) from a `Vmo`/`DmaBuffer`
/// directly into Ring 3 Lower-Half User Space starting at `0x1000_0000+` (Zero-Copy).
pub fn map_vmo_to_user(
    phys_addr: PhysAddr,
    size_bytes: usize,
    writable: bool,
) -> Result<VirtAddr, &'static str> {
    if size_bytes == 0 {
        return Err("El tamano del mapeo VMO no puede ser cero");
    }
    if (phys_addr as usize) % PAGE_SIZE != 0 {
        return Err("La direccion fisica del VMO no esta alineada a 4 KiB");
    }

    let num_pages = size_bytes.div_ceil(PAGE_SIZE);
    let aligned_size = num_pages * PAGE_SIZE;
    let user_base = NEXT_USER_MMAP_VADDR.fetch_add(aligned_size as u64, Ordering::AcqRel);

    for i in 0..num_pages {
        let page_vaddr = user_base + ((i * PAGE_SIZE) as u64);
        let page_paddr = phys_addr + ((i * PAGE_SIZE) as u64);
        // SAFETY: Mapping valid physical frames of a VMO into Lower-Half User Space.
        unsafe {
            map_user_4k_page(page_vaddr, page_paddr, writable, false);
        }
    }

    register_user_region(user_base, aligned_size, writable, false);
    Ok(user_base)
}

/// Resets the logical `sys_brk` program break back to `USER_HEAP_BASE_VADDR` (`0x0200_0000`)
/// when launching a fresh child process, while retaining any already-mapped 4 KiB heap pages.
pub fn reset_user_brk() {
    CURRENT_USER_BRK.store(USER_HEAP_BASE_VADDR, Ordering::Release);
}

/// Implements the Linux `sys_brk` (`12`) dynamic User Heap manager in Lower-Half User Space
/// starting at `USER_HEAP_BASE_VADDR` (`0x0000_0000_0200_0000`):
/// - When `new_addr == 0`, returns the current program break (`0x0200_0000+`).
/// - When `new_addr > current_brk` (within `USER_HEAP_BASE_VADDR..=USER_HEAP_MAX_VADDR`),
///   dynamically maps any required 4 KiB pages with strict `W^X` (`PRESENT | WRITABLE | USER | NX`),
///   registers the region for user buffer bounds checks, and returns the updated program break.
pub fn expand_user_brk(new_addr: VirtAddr) -> VirtAddr {
    let current = CURRENT_USER_BRK.load(Ordering::Acquire);
    if new_addr == 0 {
        return current;
    }
    if !(USER_HEAP_BASE_VADDR..=USER_HEAP_MAX_VADDR).contains(&new_addr) {
        return current;
    }

    let mapped_end = MAPPED_USER_BRK_END.load(Ordering::Acquire);
    if new_addr > mapped_end {
        let required_end = (new_addr + (PAGE_SIZE as u64) - 1) & !((PAGE_SIZE as u64) - 1);
        let delta_bytes = (required_end - mapped_end) as usize;
        if map_user_segment(mapped_end, delta_bytes, true, false).is_err() {
            return current;
        }
        MAPPED_USER_BRK_END.store(required_end, Ordering::Release);
        let total_mapped = (required_end - USER_HEAP_BASE_VADDR) as usize;
        register_user_region(USER_HEAP_BASE_VADDR, total_mapped, true, false);
    }

    CURRENT_USER_BRK.store(new_addr, Ordering::Release);
    new_addr
}

/// Allocates and maps anonymous 4 KiB frames in Lower-Half User Space for an ELF `PT_LOAD`
/// segment or User Stack at `[vaddr, vaddr + size_bytes)`, enforcing strict `W^X` (`Write XOR Execute`).
pub fn map_user_segment(
    vaddr: VirtAddr,
    size_bytes: usize,
    writable: bool,
    executable: bool,
) -> Result<VirtAddr, &'static str> {
    if size_bytes == 0 {
        return Err("Segmento de usuario de tamano cero");
    }
    if writable && executable {
        return Err("Violacion W^X: un segmento no puede ser WRITABLE y EXECUTABLE simultaneamente");
    }
    if vaddr < USER_SPACE_LOW_BASE || vaddr >= USER_SPACE_MAX_VADDR {
        return Err("Direccion virtual de segmento fuera de Lower-Half User Space");
    }

    let page_aligned_start = vaddr & !((PAGE_SIZE as u64) - 1);
    let end = vaddr
        .checked_add(size_bytes as u64)
        .ok_or("Overflow en tamano de segmento ELF")?;
    let page_aligned_end = (end + (PAGE_SIZE as u64) - 1) & !((PAGE_SIZE as u64) - 1);
    let total_aligned_bytes = (page_aligned_end - page_aligned_start) as usize;
    let num_pages = total_aligned_bytes / PAGE_SIZE;

    for i in 0..num_pages {
        let page_vaddr = page_aligned_start + ((i * PAGE_SIZE) as u64);
        let frame_paddr =
            allocate_frame().ok_or("Sin marcos fisicos para mapear segmento de usuario")?;
        // SAFETY: Mapping freshly allocated zeroed physical frame into Lower-Half User Space.
        unsafe {
            map_user_4k_page(page_vaddr, frame_paddr, writable, executable);
        }
    }

    register_user_region(page_aligned_start, total_aligned_bytes, writable, executable);
    Ok(page_aligned_start)
}

/// Applies final strict `W^X` (`Write XOR Execute`) hardware page protections to a mapped
/// Lower-Half User Space segment after `Elf64Loader` finishes copying segment data:
/// - If `writable == false`, clears `PTE_WRITABLE` (`1 << 1`) on all leaf PTEs.
/// - If `executable == false`, sets `PTE_NX_BIT` (`1 << 63`) on all leaf PTEs.
/// - Rejects `writable && executable`.
pub fn protect_user_segment(
    vaddr: VirtAddr,
    size_bytes: usize,
    writable: bool,
    executable: bool,
) -> Result<(), &'static str> {
    if size_bytes == 0 {
        return Err("Segmento de usuario de tamano cero");
    }
    if writable && executable {
        return Err("Violacion W^X: un segmento no puede ser WRITABLE y EXECUTABLE simultaneamente");
    }
    if vaddr < USER_SPACE_LOW_BASE || vaddr >= USER_SPACE_MAX_VADDR {
        return Err("Direccion virtual de segmento fuera de Lower-Half User Space");
    }

    let page_aligned_start = vaddr & !((PAGE_SIZE as u64) - 1);
    let end = vaddr
        .checked_add(size_bytes as u64)
        .ok_or("Overflow en tamano de segmento ELF")?;
    let page_aligned_end = (end + (PAGE_SIZE as u64) - 1) & !((PAGE_SIZE as u64) - 1);
    let total_aligned_bytes = (page_aligned_end - page_aligned_start) as usize;
    let num_pages = total_aligned_bytes / PAGE_SIZE;

    for i in 0..num_pages {
        let page_vaddr = page_aligned_start + ((i * PAGE_SIZE) as u64);
        // SAFETY: Updating PTE flags and flushing TLB for a mapped Lower-Half user page.
        unsafe {
            protect_user_4k_page(page_vaddr, writable, executable)?;
        }
    }

    register_user_region(page_aligned_start, total_aligned_bytes, writable, executable);
    Ok(())
}

/// Maps a single 4 KiB physical frame at `user_vaddr` in the active `CR3` 4-level page table
/// with `PRESENT | USER` and strict `W^X` (`WRITABLE` vs `PTE_NX_BIT`) flags.
pub(crate) unsafe fn map_user_4k_page(
    user_vaddr: VirtAddr,
    frame_paddr: PhysAddr,
    writable: bool,
    executable: bool,
) {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_WRITABLE: u64 = 1 << 1;
    const PTE_USER: u64 = 1 << 2;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    const PTE_NX_BIT: u64 = 1 << 63;
    const INTERMEDIATE_FLAGS: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_USER;

    let cr3: u64;
    // SAFETY: Reading CR3 in Ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }

    let pml4_paddr = cr3 & PTE_ADDR_MASK;
    let pml4_ptr = phys_to_virt(pml4_paddr) as *mut u64;

    let indices = [
        ((user_vaddr >> 39) & 0x1FF) as usize,
        ((user_vaddr >> 30) & 0x1FF) as usize,
        ((user_vaddr >> 21) & 0x1FF) as usize,
    ];

    let mut table_ptr = pml4_ptr;
    for &idx in &indices {
        // SAFETY: `table_ptr` points to a valid 4 KiB page table mapped via HHDM.
        let entry = unsafe { core::ptr::read_volatile(table_ptr.add(idx)) };
        let next_table_paddr = if (entry & PTE_PRESENT) == 0 || (entry & PTE_HUGE) != 0 {
            let new_table = allocate_frame()
                .expect("Fallo al reservar marco fisico para tabla de paginas de Anillo 3");
            // SAFETY: Writing intermediate PTE.
            unsafe {
                core::ptr::write_volatile(table_ptr.add(idx), new_table | INTERMEDIATE_FLAGS);
            }
            new_table
        } else {
            let updated = (entry | INTERMEDIATE_FLAGS) & !PTE_NX_BIT;
            // SAFETY: Ensuring intermediate PTE has USER_ACCESSIBLE and WRITABLE set.
            unsafe {
                core::ptr::write_volatile(table_ptr.add(idx), updated);
            }
            entry & PTE_ADDR_MASK
        };
        table_ptr = phys_to_virt(next_table_paddr) as *mut u64;
    }

    let pt_idx = ((user_vaddr >> 12) & 0x1FF) as usize;
    let mut leaf_flags = PTE_PRESENT | PTE_USER;
    if writable {
        leaf_flags |= PTE_WRITABLE;
    }
    if !executable {
        leaf_flags |= PTE_NX_BIT;
    }

    // SAFETY: Writing leaf PTE and invalidating TLB entry.
    unsafe {
        core::ptr::write_volatile(
            table_ptr.add(pt_idx),
            (frame_paddr & PTE_ADDR_MASK) | leaf_flags,
        );
        core::arch::asm!("invlpg [{}]", in(reg) user_vaddr, options(nostack, preserves_flags));
    }
}

/// Updates the `PTE_WRITABLE` and `PTE_NX_BIT` flags on an already-mapped 4 KiB leaf PTE.
pub(crate) unsafe fn protect_user_4k_page(
    user_vaddr: VirtAddr,
    writable: bool,
    executable: bool,
) -> Result<(), &'static str> {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_WRITABLE: u64 = 1 << 1;
    const PTE_USER: u64 = 1 << 2;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    const PTE_NX_BIT: u64 = 1 << 63;

    let cr3: u64;
    // SAFETY: Reading CR3 in Ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }

    let pml4_paddr = cr3 & PTE_ADDR_MASK;
    let mut table_ptr = phys_to_virt(pml4_paddr) as *mut u64;

    let indices = [
        ((user_vaddr >> 39) & 0x1FF) as usize,
        ((user_vaddr >> 30) & 0x1FF) as usize,
        ((user_vaddr >> 21) & 0x1FF) as usize,
    ];

    for &idx in &indices {
        // SAFETY: `table_ptr` points to a valid 4 KiB page table mapped via HHDM.
        let entry = unsafe { core::ptr::read_volatile(table_ptr.add(idx)) };
        if (entry & PTE_PRESENT) == 0 || (entry & PTE_HUGE) != 0 {
            return Err("Tabla de paginas intermedia no mapeada al aplicar protect_user_segment");
        }
        table_ptr = phys_to_virt(entry & PTE_ADDR_MASK) as *mut u64;
    }

    let pt_idx = ((user_vaddr >> 12) & 0x1FF) as usize;
    // SAFETY: Reading leaf PTE in PT.
    let leaf_entry = unsafe { core::ptr::read_volatile(table_ptr.add(pt_idx)) };
    if (leaf_entry & PTE_PRESENT) == 0 {
        return Err("PTE hoja no presente al aplicar protect_user_segment");
    }

    let mut updated_entry = (leaf_entry & !(PTE_WRITABLE | PTE_NX_BIT)) | PTE_PRESENT | PTE_USER;
    if writable {
        updated_entry |= PTE_WRITABLE;
    }
    if !executable {
        updated_entry |= PTE_NX_BIT;
    }

    // SAFETY: Writing updated leaf PTE and flushing TLB entry.
    unsafe {
        core::ptr::write_volatile(table_ptr.add(pt_idx), updated_entry);
        core::arch::asm!("invlpg [{}]", in(reg) user_vaddr, options(nostack, preserves_flags));
    }

    Ok(())
}
