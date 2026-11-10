//! User Space Process Cloning and Frame Recycling (`ostd::mm::vm_space::cloning`).

use alloc::vec::Vec;
use super::super::{
    frame::{allocate_frame, deallocate_frame},
    phys_to_virt, PhysAddr, PAGE_SIZE,
};
use super::{
    active_cr3, master_kernel_cr3, switch_cr3,
    ProcessVmSpace, PROCESS_VM_SPACES,
    PTE_ADDR_MASK, USER_HEAP_BASE_VADDR, USER_MMAP_BASE_VADDR,
};

/// Deallocates all Lower-Half User Space page tables (PT, PD, PDPT) and user memory frames
/// allocated for the given process `pml4_phys`, cleans up its `ProcessVmSpace` entry, and frees
/// the root PML4 frame.
///
/// SAFETY & INTEGRITY:
/// - Refuses to destroy `MASTER_KERNEL_PML4` or null physical address.
/// - If `pml4_phys` is the currently active `CR3`, automatically switches hardware `CR3` back to
///   `MASTER_KERNEL_PML4` before modifying or freeing page tables.
/// - Does NOT deallocate physical frames that belong to mapped hardware VMOs (such as `/dev/fb0` or MMIO).
pub fn destroy_process_vm_space(pml4_phys: PhysAddr) -> Result<(), &'static str> {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_HUGE: u64 = 1 << 7;

    let master = master_kernel_cr3();
    if pml4_phys == 0 || pml4_phys == master {
        return Err("No se puede destruir el PML4 maestro del kernel o direccion nula");
    }

    // 1. Ensure CPU is not running with `pml4_phys` active
    if active_cr3() == pml4_phys {
        switch_cr3(master);
    }

    // 2. Remove from PROCESS_VM_SPACES and extract its mapped VMO cache
    let vmo_cache = {
        let mut spaces = PROCESS_VM_SPACES.lock();
        if let Some(pos) = spaces.iter().position(|s| s.pml4_phys == pml4_phys) {
            let removed = spaces.remove(pos);
            removed.vmo_cache
        } else {
            Vec::new()
        }
    };

    let is_vmo_frame = |frame: PhysAddr| -> bool {
        for vmo in &vmo_cache {
            let start = vmo.phys_addr;
            let end = start + (vmo.aligned_size as u64);
            if frame >= start && frame < end {
                return true;
            }
        }
        false
    };

    // 3. Walk Lower-Half entries (0..256) and reclaim all leaf frames and intermediate page tables
    let pml4_ptr = phys_to_virt(pml4_phys) as *mut u64;

    for pml4_idx in 0..256 {
        // SAFETY: Reading PML4 entry in HHDM.
        let pml4_entry = unsafe { core::ptr::read_volatile(pml4_ptr.add(pml4_idx)) };
        if (pml4_entry & PTE_PRESENT) == 0 {
            continue;
        }

        let pdpt_paddr = pml4_entry & PTE_ADDR_MASK;
        let pdpt_ptr = phys_to_virt(pdpt_paddr) as *mut u64;

        for pdpt_idx in 0..512 {
            let pdpt_entry = unsafe { core::ptr::read_volatile(pdpt_ptr.add(pdpt_idx)) };
            if (pdpt_entry & PTE_PRESENT) == 0 {
                continue;
            }
            if (pdpt_entry & PTE_HUGE) != 0 {
                let frame = pdpt_entry & PTE_ADDR_MASK;
                if !is_vmo_frame(frame) {
                    deallocate_frame(frame);
                }
                continue;
            }

            let pd_paddr = pdpt_entry & PTE_ADDR_MASK;
            let pd_ptr = phys_to_virt(pd_paddr) as *mut u64;

            for pd_idx in 0..512 {
                let pd_entry = unsafe { core::ptr::read_volatile(pd_ptr.add(pd_idx)) };
                if (pd_entry & PTE_PRESENT) == 0 {
                    continue;
                }
                if (pd_entry & PTE_HUGE) != 0 {
                    let frame = pd_entry & PTE_ADDR_MASK;
                    if !is_vmo_frame(frame) {
                        deallocate_frame(frame);
                    }
                    continue;
                }

                let pt_paddr = pd_entry & PTE_ADDR_MASK;
                let pt_ptr = phys_to_virt(pt_paddr) as *mut u64;

                for pt_idx in 0..512 {
                    let pt_entry = unsafe { core::ptr::read_volatile(pt_ptr.add(pt_idx)) };
                    if (pt_entry & PTE_PRESENT) == 0 {
                        continue;
                    }
                    let leaf_frame = pt_entry & PTE_ADDR_MASK;
                    if leaf_frame != 0 && !is_vmo_frame(leaf_frame) {
                        deallocate_frame(leaf_frame);
                    }
                }

                // Deallocate the PT table frame
                deallocate_frame(pt_paddr);
            }

            // Deallocate the PD table frame
            deallocate_frame(pd_paddr);
        }

        // Deallocate the PDPT table frame
        deallocate_frame(pdpt_paddr);
    }

    // 4. Deallocate the root PML4 frame itself
    deallocate_frame(pml4_phys);

    crate::early_println!(
        "[ostd::mm::vm_space] destroy_process_vm_space({:#x}) -> PML4 y tablas Lower-Half liberadas exitosamente",
        pml4_phys
    );

    Ok(())
}

/// Duplicates the user virtual address space (Lower-Half 0..256) of a parent process into a fresh
/// PML4 root for POSIX `sys_fork` / `sys_clone`.
///
/// - Allocates a new physical PML4 frame and copies kernel Higher-Half entries (256..512) directly
///   from `MASTER_KERNEL_PML4`.
/// - For every mapped user page in Lower-Half (0..256), allocates a new physical 4 KiB frame and copies
///   the exact memory contents from parent to child, enforcing identical `W^X` permissions
///   (`PTE_PRESENT | PTE_USER | PTE_WRITABLE / PTE_NX_BIT`).
/// - Shared VMO frames (e.g. Framebuffer `/dev/fb0` or MMIO) are mapped to the same physical frame.
/// - Registers the cloned `ProcessVmSpace` metadata (regions, heap break, mmap cursor) for the child PML4.
pub fn clone_user_vm_space(parent_cr3: PhysAddr) -> Result<PhysAddr, &'static str> {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_WRITABLE: u64 = 1 << 1;
    const PTE_USER: u64 = 1 << 2;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let master = master_kernel_cr3();
    if parent_cr3 == 0 {
        return Err("parent_cr3 no puede ser nulo en clone_user_vm_space");
    }

    // 1. Allocate root PML4 frame for the child process
    let child_pml4 = allocate_frame().ok_or("Sin marcos libres para PML4 hijo en clone_user_vm_space")?;
    let child_pml4_ptr = phys_to_virt(child_pml4) as *mut u64;

    // Zero out child Lower-Half entries (0..256)
    for i in 0..256 {
        unsafe { core::ptr::write_volatile(child_pml4_ptr.add(i), 0); }
    }

    // Copy Higher-Half kernel mappings (256..512) from MASTER_KERNEL_PML4
    let master_pml4_ptr = phys_to_virt(master) as *const u64;
    for i in 256..512 {
        let entry = unsafe { core::ptr::read_volatile(master_pml4_ptr.add(i)) };
        unsafe { core::ptr::write_volatile(child_pml4_ptr.add(i), entry); }
    }

    // 2. Clone metadata (regions, current_brk, mapped_brk_end, next_mmap_vaddr, vmo_cache)
    let (regions, vmo_cache, current_brk, mapped_brk_end, next_mmap_vaddr) = {
        let spaces = PROCESS_VM_SPACES.lock();
        if let Some(parent_space) = spaces.iter().find(|s| s.pml4_phys == parent_cr3) {
            (
                parent_space.regions.clone(),
                parent_space.vmo_cache.clone(),
                parent_space.current_brk,
                parent_space.mapped_brk_end,
                parent_space.next_mmap_vaddr,
            )
        } else {
            (
                Vec::new(),
                Vec::new(),
                USER_HEAP_BASE_VADDR,
                USER_HEAP_BASE_VADDR,
                USER_MMAP_BASE_VADDR,
            )
        }
    };

    let is_vmo_frame = |frame: PhysAddr| -> bool {
        for vmo in &vmo_cache {
            let start = vmo.phys_addr;
            let end = start + (vmo.aligned_size as u64);
            if frame >= start && frame < end {
                return true;
            }
        }
        false
    };

    // 3. Walk parent Lower-Half page tables and duplicate physical frames
    let parent_pml4_ptr = phys_to_virt(parent_cr3) as *const u64;

    for pml4_idx in 0..256 {
        let parent_pml4_e = unsafe { core::ptr::read_volatile(parent_pml4_ptr.add(pml4_idx)) };
        if (parent_pml4_e & PTE_PRESENT) == 0 {
            continue;
        }

        let child_pdpt = match allocate_frame() {
            Some(f) => f,
            None => {
                let _ = destroy_process_vm_space(child_pml4);
                return Err("Sin marcos libres para PDPT hijo");
            }
        };
        let child_pdpt_ptr = phys_to_virt(child_pdpt) as *mut u64;
        for j in 0..512 {
            unsafe { core::ptr::write_volatile(child_pdpt_ptr.add(j), 0); }
        }
        let intermediate_flags = parent_pml4_e & (PTE_PRESENT | PTE_WRITABLE | PTE_USER);
        unsafe {
            core::ptr::write_volatile(child_pml4_ptr.add(pml4_idx), child_pdpt | intermediate_flags);
        }

        let parent_pdpt_paddr = parent_pml4_e & PTE_ADDR_MASK;
        let parent_pdpt_ptr = phys_to_virt(parent_pdpt_paddr) as *const u64;

        for pdpt_idx in 0..512 {
            let parent_pdpt_e = unsafe { core::ptr::read_volatile(parent_pdpt_ptr.add(pdpt_idx)) };
            if (parent_pdpt_e & PTE_PRESENT) == 0 {
                continue;
            }
            if (parent_pdpt_e & PTE_HUGE) != 0 {
                // 1 GiB page: map directly (VMO/MMIO)
                unsafe { core::ptr::write_volatile(child_pdpt_ptr.add(pdpt_idx), parent_pdpt_e); }
                continue;
            }

            let child_pd = match allocate_frame() {
                Some(f) => f,
                None => {
                    let _ = destroy_process_vm_space(child_pml4);
                    return Err("Sin marcos libres para PD hijo");
                }
            };
            let child_pd_ptr = phys_to_virt(child_pd) as *mut u64;
            for j in 0..512 {
                unsafe { core::ptr::write_volatile(child_pd_ptr.add(j), 0); }
            }
            let pd_flags = parent_pdpt_e & (PTE_PRESENT | PTE_WRITABLE | PTE_USER);
            unsafe {
                core::ptr::write_volatile(child_pdpt_ptr.add(pdpt_idx), child_pd | pd_flags);
            }

            let parent_pd_paddr = parent_pdpt_e & PTE_ADDR_MASK;
            let parent_pd_ptr = phys_to_virt(parent_pd_paddr) as *const u64;

            for pd_idx in 0..512 {
                let parent_pd_e = unsafe { core::ptr::read_volatile(parent_pd_ptr.add(pd_idx)) };
                if (parent_pd_e & PTE_PRESENT) == 0 {
                    continue;
                }
                if (parent_pd_e & PTE_HUGE) != 0 {
                    // 2 MiB page: map directly
                    unsafe { core::ptr::write_volatile(child_pd_ptr.add(pd_idx), parent_pd_e); }
                    continue;
                }

                let child_pt = match allocate_frame() {
                    Some(f) => f,
                    None => {
                        let _ = destroy_process_vm_space(child_pml4);
                        return Err("Sin marcos libres para PT hijo");
                    }
                };
                let child_pt_ptr = phys_to_virt(child_pt) as *mut u64;
                for j in 0..512 {
                    unsafe { core::ptr::write_volatile(child_pt_ptr.add(j), 0); }
                }
                let pt_flags = parent_pd_e & (PTE_PRESENT | PTE_WRITABLE | PTE_USER);
                unsafe {
                    core::ptr::write_volatile(child_pd_ptr.add(pd_idx), child_pt | pt_flags);
                }

                let parent_pt_paddr = parent_pd_e & PTE_ADDR_MASK;
                let parent_pt_ptr = phys_to_virt(parent_pt_paddr) as *const u64;

                for pt_idx in 0..512 {
                    let parent_pt_e = unsafe { core::ptr::read_volatile(parent_pt_ptr.add(pt_idx)) };
                    if (parent_pt_e & PTE_PRESENT) == 0 {
                        continue;
                    }
                    let parent_leaf_frame = parent_pt_e & PTE_ADDR_MASK;
                    let leaf_flags = parent_pt_e & !PTE_ADDR_MASK;

                    if parent_leaf_frame != 0 && is_vmo_frame(parent_leaf_frame) {
                        // Shared VMO frame: map identical physical address
                        unsafe {
                            core::ptr::write_volatile(
                                child_pt_ptr.add(pt_idx),
                                parent_leaf_frame | leaf_flags,
                            );
                        }
                    } else if parent_leaf_frame != 0 {
                        // Regular user memory frame: allocate dedicated frame and copy content
                        let child_frame = match allocate_frame() {
                            Some(f) => f,
                            None => {
                                let _ = destroy_process_vm_space(child_pml4);
                                return Err("Sin marcos para duplicar pagina de usuario");
                            }
                        };
                        let src_ptr = phys_to_virt(parent_leaf_frame) as *const u8;
                        let dst_ptr = phys_to_virt(child_frame) as *mut u8;
                        unsafe {
                            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, PAGE_SIZE);
                            core::ptr::write_volatile(
                                child_pt_ptr.add(pt_idx),
                                child_frame | leaf_flags,
                            );
                        }
                    }
                }
            }
        }
    }

    // 4. Register child space in PROCESS_VM_SPACES
    {
        let mut spaces = PROCESS_VM_SPACES.lock();
        spaces.push(ProcessVmSpace {
            pml4_phys: child_pml4,
            regions,
            vmo_cache,
            current_brk,
            mapped_brk_end,
            next_mmap_vaddr,
        });
    }

    crate::early_println!(
        "[ostd::mm::vm_space] clone_user_vm_space({:#x}) -> Nuevo PML4 hijo {:#x} creado con aislamiento W^X",
        parent_cr3,
        child_pml4
    );

    Ok(child_pml4)
}

