//! Lower-Half User Virtual Memory Mapping and Protection (`ostd::mm::vm_space::mapping`).

use super::super::{
    frame::{allocate_frame, deallocate_frame},
    phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE,
};
use super::{
    register_user_region, with_active_vm_space, MappedVmoCacheEntry,
    USER_SPACE_LOW_BASE, USER_SPACE_MAX_VADDR,
};

/// Maps a contiguous physical range (`phys_addr`, `size_bytes`) from a `Vmo`/`DmaBuffer`
/// directly into the active `CR3` Lower-Half User Space starting at `0x1000_0000+` (Zero-Copy).
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

    let (cached_hit, user_base) = with_active_vm_space(|space| {
        for item in &space.vmo_cache {
            if item.phys_addr == phys_addr
                && item.aligned_size == aligned_size
                && item.writable == writable
            {
                return (Some(item.user_vaddr), 0);
            }
        }
        let base = space.next_mmap_vaddr;
        space.next_mmap_vaddr = space.next_mmap_vaddr.saturating_add(aligned_size as u64);
        (None, base)
    });

    if let Some(existing_vaddr) = cached_hit {
        return Ok(existing_vaddr);
    }

    for i in 0..num_pages {
        let page_vaddr = user_base + ((i * PAGE_SIZE) as u64);
        let page_paddr = phys_addr + ((i * PAGE_SIZE) as u64);
        // SAFETY: Mapping valid physical frames of a VMO into active CR3 Lower-Half User Space.
        unsafe {
            map_user_4k_page(page_vaddr, page_paddr, writable, false);
        }
    }

    with_active_vm_space(|space| {
        space.upsert_region(user_base, aligned_size, writable, false);
        space.vmo_cache.push(MappedVmoCacheEntry {
            phys_addr,
            aligned_size,
            writable,
            user_vaddr: user_base,
        });
    });
    Ok(user_base)
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
        // Reuse any existing anonymous 4 KiB user frame at `page_vaddr` (zeroed cleanly)
        // so repeated `sys_execve` app launches never leak physical RAM frames.
        // SAFETY: Inspecting active CR3 user PTEs and zeroing owned anonymous user frame via HHDM.
        let frame_paddr = match unsafe { lookup_existing_user_4k_frame(page_vaddr) } {
            Some(existing_paddr) => {
                unsafe {
                    core::ptr::write_bytes(phys_to_virt(existing_paddr) as *mut u8, 0, PAGE_SIZE);
                }
                existing_paddr
            }
            None => {
                let p = allocate_frame().ok_or("Sin marcos fisicos para mapear segmento de usuario")?;
                unsafe {
                    core::ptr::write_bytes(phys_to_virt(p) as *mut u8, 0, PAGE_SIZE);
                }
                p
            }
        };
        // SAFETY: Mapping zeroed physical frame into Lower-Half User Space.
        unsafe {
            map_user_4k_page(page_vaddr, frame_paddr, writable, executable);
        }
    }

    register_user_region(page_aligned_start, total_aligned_bytes, writable, executable);
    Ok(page_aligned_start)
}

/// Dynamically maps an anonymous zeroed memory buffer in Lower-Half User Space starting at
/// `0x1000_0000+` (`USER_MMAP_BASE_VADDR`) with strict `W^X` permissions for `sys_mmap(MAP_ANONYMOUS)`.
pub fn map_anonymous_user_memory(len: usize, writable: bool) -> Result<VirtAddr, &'static str> {
    if len == 0 {
        return Err("Tamano anonimo de longitud cero");
    }
    let num_pages = len.div_ceil(PAGE_SIZE);
    let aligned_size = num_pages * PAGE_SIZE;

    let base_vaddr = with_active_vm_space(|space| {
        let base = space.next_mmap_vaddr;
        space.next_mmap_vaddr = space.next_mmap_vaddr.saturating_add(aligned_size as u64);
        base
    });

    map_user_segment(base_vaddr, aligned_size, writable, false)?;
    Ok(base_vaddr)
}

/// Unmaps a range of Lower-Half user pages `[vaddr, vaddr + len)` from the active `CR3` page table,
/// invalidates the TLB (`invlpg`), and deallocates any anonymous physical frames backing them.
///
/// Hardware VMO frames (e.g. `/dev/fb0` or MMIO) are not freed.
pub fn unmap_user_pages(vaddr: VirtAddr, len: usize) -> Result<(), &'static str> {
    if len == 0 {
        return Ok(());
    }
    if vaddr < USER_SPACE_LOW_BASE || vaddr >= USER_SPACE_MAX_VADDR {
        return Err("Direccion virtual de desmapeo fuera de Lower-Half User Space");
    }

    let page_aligned_start = vaddr & !((PAGE_SIZE as u64) - 1);
    let end = vaddr
        .checked_add(len as u64)
        .ok_or("Overflow en tamano de munmap")?;
    let page_aligned_end = (end + (PAGE_SIZE as u64) - 1) & !((PAGE_SIZE as u64) - 1);
    let total_aligned_bytes = (page_aligned_end - page_aligned_start) as usize;
    let num_pages = total_aligned_bytes / PAGE_SIZE;

    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let is_vmo = |frame: PhysAddr| -> bool {
        with_active_vm_space(|space| {
            for vmo in &space.vmo_cache {
                let start = vmo.phys_addr;
                let end = start + (vmo.aligned_size as u64);
                if frame >= start && frame < end {
                    return true;
                }
            }
            false
        })
    };

    let cr3: u64;
    // SAFETY: Reading CR3 in Ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    let pml4_ptr = phys_to_virt(cr3 & PTE_ADDR_MASK) as *mut u64;

    for i in 0..num_pages {
        let page_vaddr = page_aligned_start + ((i * PAGE_SIZE) as u64);
        let pml4_idx = ((page_vaddr >> 39) & 0x1FF) as usize;

        // Lower-Half only (0..256)
        if pml4_idx >= 256 {
            continue;
        }

        // SAFETY: Reading PML4 entry in HHDM.
        let pml4_entry = unsafe { core::ptr::read_volatile(pml4_ptr.add(pml4_idx)) };
        if (pml4_entry & PTE_PRESENT) == 0 {
            continue;
        }

        let pdpt_ptr = phys_to_virt(pml4_entry & PTE_ADDR_MASK) as *mut u64;
        let pdpt_idx = ((page_vaddr >> 30) & 0x1FF) as usize;

        let pdpt_entry = unsafe { core::ptr::read_volatile(pdpt_ptr.add(pdpt_idx)) };
        if (pdpt_entry & PTE_PRESENT) == 0 || (pdpt_entry & PTE_HUGE) != 0 {
            continue;
        }

        let pd_ptr = phys_to_virt(pdpt_entry & PTE_ADDR_MASK) as *mut u64;
        let pd_idx = ((page_vaddr >> 21) & 0x1FF) as usize;

        let pd_entry = unsafe { core::ptr::read_volatile(pd_ptr.add(pd_idx)) };
        if (pd_entry & PTE_PRESENT) == 0 || (pd_entry & PTE_HUGE) != 0 {
            continue;
        }

        let pt_ptr = phys_to_virt(pd_entry & PTE_ADDR_MASK) as *mut u64;
        let pt_idx = ((page_vaddr >> 12) & 0x1FF) as usize;

        let pt_entry = unsafe { core::ptr::read_volatile(pt_ptr.add(pt_idx)) };
        if (pt_entry & PTE_PRESENT) != 0 {
            let leaf_frame = pt_entry & PTE_ADDR_MASK;
            if leaf_frame != 0 && !is_vmo(leaf_frame) {
                deallocate_frame(leaf_frame);
            }
            // SAFETY: Clearing leaf PTE and flushing TLB for this virtual address.
            unsafe {
                core::ptr::write_volatile(pt_ptr.add(pt_idx), 0u64);
                core::arch::asm!("invlpg [{}]", in(reg) page_vaddr, options(nostack, preserves_flags));
            }
        }
    }

    with_active_vm_space(|space| {
        space.unmap_regions(page_aligned_start, total_aligned_bytes);
    });

    Ok(())
}

/// Returns the physical frame backing `user_vaddr` if an anonymous 4 KiB `PTE_USER` leaf entry
/// is already present in `CR3`.
unsafe fn lookup_existing_user_4k_frame(user_vaddr: VirtAddr) -> Option<PhysAddr> {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_USER: u64 = 1 << 2;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let cr3: u64;
    // SAFETY: Reading CR3 in Ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }

    let mut table_ptr = phys_to_virt(cr3 & PTE_ADDR_MASK) as *const u64;
    let indices = [
        ((user_vaddr >> 39) & 0x1FF) as usize,
        ((user_vaddr >> 30) & 0x1FF) as usize,
        ((user_vaddr >> 21) & 0x1FF) as usize,
    ];

    for &idx in &indices {
        // SAFETY: Reading valid page-table entry via HHDM.
        let entry = unsafe { core::ptr::read_volatile(table_ptr.add(idx)) };
        if (entry & PTE_PRESENT) == 0 || (entry & PTE_HUGE) != 0 {
            return None;
        }
        table_ptr = phys_to_virt(entry & PTE_ADDR_MASK) as *const u64;
    }

    let pt_idx = ((user_vaddr >> 12) & 0x1FF) as usize;
    // SAFETY: Reading leaf PTE in PT.
    let leaf = unsafe { core::ptr::read_volatile(table_ptr.add(pt_idx)) };
    if (leaf & (PTE_PRESENT | PTE_USER)) == (PTE_PRESENT | PTE_USER) {
        Some(leaf & PTE_ADDR_MASK)
    } else {
        None
    }
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


