//! Kernel MMIO Direct Physical Page Table Mapper (`ostd::mm::vm_space::mmio`).

use super::super::{
    frame::allocate_frame,
    phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE,
};

/// Maps a PCIe device MMIO physical aperture `[phys_addr, phys_addr + size_bytes)` into the
/// Higher-Half Direct Map (`phys_to_virt(phys_addr)`) of the active `CR3` page table with
/// `PTE_PRESENT | PTE_WRITABLE | PTE_PWT | PTE_PCD (NO_CACHE) | PTE_NX_BIT`.
pub fn map_kernel_mmio(phys_addr: PhysAddr, size_bytes: usize) -> VirtAddr {
    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_WRITABLE: u64 = 1 << 1;
    const PTE_PWT: u64 = 1 << 3;
    const PTE_PCD: u64 = 1 << 4;
    const PTE_HUGE: u64 = 1 << 7;
    const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    const PTE_NX_BIT: u64 = 1 << 63;

    let phys_page_start = phys_addr & !((PAGE_SIZE as u64) - 1);
    let page_offset = (phys_addr & ((PAGE_SIZE as u64) - 1)) as usize;
    let num_pages = page_offset.saturating_add(size_bytes).div_ceil(PAGE_SIZE).max(1);

    let cr3: u64;
    // SAFETY: Reading CR3 in Ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    let pml4_ptr = phys_to_virt(cr3 & PTE_ADDR_MASK) as *mut u64;

    for i in 0..num_pages {
        let paddr = phys_page_start + ((i * PAGE_SIZE) as u64);
        let vaddr = phys_to_virt(paddr);

        let pml4_idx = ((vaddr >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((vaddr >> 30) & 0x1FF) as usize;
        let pd_idx = ((vaddr >> 21) & 0x1FF) as usize;
        let pt_idx = ((vaddr >> 12) & 0x1FF) as usize;

        // SAFETY: Walking and populating Ring 0 Higher-Half page tables in `CR3`.
        unsafe {
            // 1. PML4 -> PDPT
            let pml4_e = core::ptr::read_volatile(pml4_ptr.add(pml4_idx));
            let pdpt_paddr = if (pml4_e & PTE_PRESENT) == 0 {
                let new_pdpt = allocate_frame()
                    .expect("Fallo al reservar marco fisico PDPT para Kernel MMIO");
                core::ptr::write_volatile(
                    pml4_ptr.add(pml4_idx),
                    new_pdpt | PTE_PRESENT | PTE_WRITABLE,
                );
                new_pdpt
            } else {
                core::ptr::write_volatile(
                    pml4_ptr.add(pml4_idx),
                    (pml4_e | PTE_PRESENT | PTE_WRITABLE) & !PTE_NX_BIT,
                );
                pml4_e & PTE_ADDR_MASK
            };
            let pdpt_ptr = phys_to_virt(pdpt_paddr) as *mut u64;

            // 2. PDPT -> PD (transparently split 1 GiB huge page if present)
            let pdpt_e = core::ptr::read_volatile(pdpt_ptr.add(pdpt_idx));
            let pd_paddr = if (pdpt_e & PTE_PRESENT) == 0 {
                let new_pd =
                    allocate_frame().expect("Fallo al reservar marco fisico PD para Kernel MMIO");
                core::ptr::write_volatile(
                    pdpt_ptr.add(pdpt_idx),
                    new_pd | PTE_PRESENT | PTE_WRITABLE,
                );
                new_pd
            } else if (pdpt_e & PTE_HUGE) != 0 {
                let gb_phys = pdpt_e & 0x000F_FFFF_C000_0000;
                let sub_flags =
                    (pdpt_e & (PTE_WRITABLE | PTE_PWT | PTE_PCD | PTE_NX_BIT)) | PTE_PRESENT | PTE_HUGE;
                let new_pd = allocate_frame()
                    .expect("Fallo al dividir pagina de 1 GiB para Kernel MMIO");
                let new_pd_ptr = phys_to_virt(new_pd) as *mut u64;
                for j in 0..512usize {
                    core::ptr::write_volatile(
                        new_pd_ptr.add(j),
                        (gb_phys + ((j as u64) * 0x20_0000)) | sub_flags,
                    );
                }
                core::ptr::write_volatile(
                    pdpt_ptr.add(pdpt_idx),
                    new_pd | PTE_PRESENT | PTE_WRITABLE,
                );
                new_pd
            } else {
                core::ptr::write_volatile(
                    pdpt_ptr.add(pdpt_idx),
                    (pdpt_e | PTE_PRESENT | PTE_WRITABLE) & !PTE_NX_BIT,
                );
                pdpt_e & PTE_ADDR_MASK
            };
            let pd_ptr = phys_to_virt(pd_paddr) as *mut u64;

            // 3. PD -> PT (transparently split 2 MiB huge page if present)
            let pd_e = core::ptr::read_volatile(pd_ptr.add(pd_idx));
            let pt_paddr = if (pd_e & PTE_PRESENT) == 0 {
                let new_pt =
                    allocate_frame().expect("Fallo al reservar marco fisico PT para Kernel MMIO");
                core::ptr::write_volatile(
                    pd_ptr.add(pd_idx),
                    new_pt | PTE_PRESENT | PTE_WRITABLE,
                );
                new_pt
            } else if (pd_e & PTE_HUGE) != 0 {
                let mb_phys = pd_e & 0x000F_FFFF_FFE0_0000;
                let sub_flags =
                    (pd_e & (PTE_WRITABLE | PTE_PWT | PTE_PCD | PTE_NX_BIT)) | PTE_PRESENT;
                let new_pt = allocate_frame()
                    .expect("Fallo al dividir pagina de 2 MiB para Kernel MMIO");
                let new_pt_ptr = phys_to_virt(new_pt) as *mut u64;
                for j in 0..512usize {
                    core::ptr::write_volatile(
                        new_pt_ptr.add(j),
                        (mb_phys + ((j as u64) * 0x1000)) | sub_flags,
                    );
                }
                core::ptr::write_volatile(
                    pd_ptr.add(pd_idx),
                    new_pt | PTE_PRESENT | PTE_WRITABLE,
                );
                new_pt
            } else {
                core::ptr::write_volatile(
                    pd_ptr.add(pd_idx),
                    (pd_e | PTE_PRESENT | PTE_WRITABLE) & !PTE_NX_BIT,
                );
                pd_e & PTE_ADDR_MASK
            };
            let pt_ptr = phys_to_virt(pt_paddr) as *mut u64;

            // 4. Leaf 4 KiB PTE with Uncached MMIO attributes (PRESENT | WRITABLE | PWT | PCD | NX)
            let mmio_leaf = (paddr & PTE_ADDR_MASK)
                | PTE_PRESENT
                | PTE_WRITABLE
                | PTE_PWT
                | PTE_PCD
                | PTE_NX_BIT;
            core::ptr::write_volatile(pt_ptr.add(pt_idx), mmio_leaf);
            core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack, preserves_flags));
        }
    }

    phys_to_virt(phys_addr)
}


