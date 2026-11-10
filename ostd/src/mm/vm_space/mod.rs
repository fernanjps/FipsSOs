//! Virtual Memory Space & User Space Isolation (`ostd::mm::vm_space`).
//!
//! Manages Lower-Half User Space (`0x0000_0000_0000_0000..0x0000_7FFF_FFFF_FFFF`)
//! for isolated Ring 3 processes, implementing Zero-Copy VMO mappings (`sys_mmap`),
//! dynamic Heap expansion (`sys_brk`), memory unmapping (`sys_munmap`), memory protection (`sys_mprotect`),
//! process duplication (`clone_user_vm_space` for `sys_fork`), and teardown (`destroy_process_vm_space`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    frame::allocate_frame,
    phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::sync::SpinLock;

pub mod cloning;
pub mod mapping;
pub mod mmio;

pub use cloning::*;
pub use mapping::*;
pub use mmio::*;

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

pub(crate) const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Physical address of the master Kernel `PML4` root captured at boot (`ostd::mm::init`).
pub(crate) static MASTER_KERNEL_PML4: AtomicU64 = AtomicU64::new(0);

/// Tracked virtual memory region mapped in Lower-Half User Space for a process `PML4`.
#[derive(Debug, Clone, Copy)]
pub struct UserVmRegion {
    pub start_vaddr: VirtAddr,
    pub size_bytes: usize,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MappedVmoCacheEntry {
    pub(crate) phys_addr: PhysAddr,
    pub(crate) aligned_size: usize,
    pub(crate) writable: bool,
    pub(crate) user_vaddr: VirtAddr,
}

/// Per-process Virtual Memory Space descriptor indexed by its hardware `CR3` (`PML4` physical frame).
///
/// Isolates Lower-Half (`0..256`) `UserVmRegion` bounds, `sys_brk` heap cursors (`0x0200_0000+`),
/// and `sys_mmap` VMO cursors (`0x1000_0000+`) so every process owns an independent address space.
pub struct ProcessVmSpace {
    pub pml4_phys: PhysAddr,
    pub regions: Vec<UserVmRegion>,
    pub(crate) vmo_cache: Vec<MappedVmoCacheEntry>,
    pub current_brk: VirtAddr,
    pub mapped_brk_end: VirtAddr,
    pub next_mmap_vaddr: VirtAddr,
}

impl ProcessVmSpace {
    pub(crate) fn new(pml4_phys: PhysAddr) -> Self {
        Self {
            pml4_phys,
            regions: Vec::new(),
            vmo_cache: Vec::new(),
            current_brk: USER_HEAP_BASE_VADDR,
            mapped_brk_end: USER_HEAP_BASE_VADDR,
            next_mmap_vaddr: USER_MMAP_BASE_VADDR,
        }
    }

    pub(crate) fn upsert_region(
        &mut self,
        start_vaddr: VirtAddr,
        size_bytes: usize,
        writable: bool,
        executable: bool,
    ) {
        for reg in self.regions.iter_mut() {
            if reg.start_vaddr == start_vaddr && reg.size_bytes == size_bytes {
                reg.writable = writable;
                reg.executable = executable;
                return;
            }
        }
        self.regions.push(UserVmRegion {
            start_vaddr,
            size_bytes,
            writable,
            executable,
        });
    }

    pub(crate) fn unmap_regions(&mut self, start_vaddr: VirtAddr, size_bytes: usize) {
        let end_vaddr = start_vaddr.saturating_add(size_bytes as u64);
        self.regions.retain(|r| {
            let r_end = r.start_vaddr.saturating_add(r.size_bytes as u64);
            r_end <= start_vaddr || r.start_vaddr >= end_vaddr
        });
        self.vmo_cache.retain(|v| {
            let v_end = v.user_vaddr.saturating_add(v.aligned_size as u64);
            v_end <= start_vaddr || v.user_vaddr >= end_vaddr
        });
    }
}

pub(crate) static PROCESS_VM_SPACES: SpinLock<Vec<ProcessVmSpace>> = SpinLock::new(Vec::new());

/// Captures the bootloader's active Ring 0 `CR3` as the canonical `MASTER_KERNEL_PML4`.
pub(crate) fn init_kernel_vm_space() {
    let boot_cr3 = active_cr3();
    MASTER_KERNEL_PML4.store(boot_cr3, Ordering::Release);
    let mut spaces = PROCESS_VM_SPACES.lock();
    if !spaces.iter().any(|s| s.pml4_phys == boot_cr3) {
        spaces.push(ProcessVmSpace::new(boot_cr3));
    }
}

/// Returns the physical address of the master Ring 0 `PML4` page table.
#[inline]
pub fn master_kernel_cr3() -> PhysAddr {
    let master = MASTER_KERNEL_PML4.load(Ordering::Acquire);
    if master != 0 {
        master
    } else {
        active_cr3()
    }
}

/// Reads the physical `PML4` root address (`4 KiB` aligned) currently loaded in hardware `CR3`.
#[inline]
pub fn active_cr3() -> PhysAddr {
    let cr3: u64;
    // SAFETY: Reading control register `CR3` in Ring 0 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3 & PTE_ADDR_MASK
}

/// Switches the CPU's active 4-level page-table root (`CR3`) to `new_pml4_phys`,
/// automatically flushing all non-global Lower-Half TLB entries on the current core.
#[inline]
pub fn switch_cr3(new_pml4_phys: PhysAddr) {
    let target = new_pml4_phys & PTE_ADDR_MASK;
    assert!(target != 0, "switch_cr3: direccion fisica PML4 nula");
    // SAFETY: `target` is a valid 4 KiB-aligned `PML4` physical frame containing the shared
    // Higher-Half kernel mappings (`256..512`), keeping Ring 0 code, stack, HHDM, and MMIO mapped.
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) target, options(nostack, preserves_flags));
    }
}

/// Allocates and initializes a dedicated 4 KiB `PML4` root frame for an isolated user `Process`:
/// - Zeroes Lower-Half entries `0..256` (`0x0000_0000_0000_0000..0x0000_7FFF_FFFF_FFFF`).
/// - Copies Higher-Half kernel entries `256..512` (`0xFFFF_8000_0000_0000..0xFFFF_FFFF_FFFF_FFFF`)
///   from `MASTER_KERNEL_PML4` so HHDM, kernel text/stacks, Framebuffer, and `e1000e` MMIO stay mapped.
/// - Registers a dedicated `ProcessVmSpace` entry in `PROCESS_VM_SPACES`.
pub fn create_process_pml4() -> Result<PhysAddr, &'static str> {
    let new_pml4_phys =
        allocate_frame().ok_or("Sin marcos fisicos para crear directorio raiz PML4 de proceso")?;
    let master_pml4_phys = master_kernel_cr3();

    let new_pml4_ptr = phys_to_virt(new_pml4_phys) as *mut u64;
    let master_pml4_ptr = phys_to_virt(master_pml4_phys) as *const u64;

    // SAFETY: `new_pml4_ptr` points to a freshly allocated 4 KiB frame (`512 * u64`) in HHDM,
    // and `master_pml4_ptr` points to the active 4 KiB master kernel PML4 in HHDM.
    unsafe {
        // 1. Explicitly clear Lower-Half user entries (0..256) for strict Ring 3 isolation
        core::ptr::write_bytes(new_pml4_ptr, 0, 256);
        // 2. Share Higher-Half kernel entries (256..512) from the master kernel PML4
        core::ptr::copy_nonoverlapping(master_pml4_ptr.add(256), new_pml4_ptr.add(256), 256);
    }

    let mut spaces = PROCESS_VM_SPACES.lock();
    if let Some(existing) = spaces.iter_mut().find(|s| s.pml4_phys == new_pml4_phys) {
        *existing = ProcessVmSpace::new(new_pml4_phys);
    } else {
        spaces.push(ProcessVmSpace::new(new_pml4_phys));
    }

    Ok(new_pml4_phys)
}

pub(crate) fn with_active_vm_space<R>(f: impl FnOnce(&mut ProcessVmSpace) -> R) -> R {
    let cr3 = active_cr3();
    let mut spaces = PROCESS_VM_SPACES.lock();
    if let Some(idx) = spaces.iter().position(|s| s.pml4_phys == cr3) {
        f(&mut spaces[idx])
    } else {
        spaces.push(ProcessVmSpace::new(cr3));
        let last = spaces.len() - 1;
        f(&mut spaces[last])
    }
}

/// Registers or updates a Lower-Half User Space virtual memory range in the active `CR3` `ProcessVmSpace`.
pub fn register_user_region(
    start_vaddr: VirtAddr,
    size_bytes: usize,
    writable: bool,
    executable: bool,
) {
    with_active_vm_space(|space| {
        space.upsert_region(start_vaddr, size_bytes, writable, executable);
    });
}

/// Validates whether `[user_vaddr, user_vaddr + len)` lies entirely within a mapped Lower-Half
/// User Space region (ELF segments, User Stack, Heap, or `mmap` VMO regions) of the active `CR3`.
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

    with_active_vm_space(|space| {
        for reg in &space.regions {
            let reg_end = reg.start_vaddr + (reg.size_bytes as u64);
            if user_vaddr >= reg.start_vaddr && end <= reg_end {
                if require_write && !reg.writable {
                    return Err("La region de usuario no tiene permisos de escritura");
                }
                return Ok(());
            }
        }
        Err("El buffer de usuario no pertenece a ninguna region mapeada en el CR3 activo")
    })
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



/// Resets the logical `sys_brk` program break of the active `CR3` `ProcessVmSpace` back to
/// `USER_HEAP_BASE_VADDR` (`0x0200_0000`) when launching a fresh child process, while retaining
/// any already-mapped 4 KiB heap pages inside that `PML4` for zero-leak frame reuse.
pub fn reset_user_brk() {
    with_active_vm_space(|space| {
        space.current_brk = USER_HEAP_BASE_VADDR;
    });
}

/// Implements the Linux `sys_brk` (`12`) dynamic User Heap manager in the active `CR3`
/// `ProcessVmSpace` starting at `USER_HEAP_BASE_VADDR` (`0x0000_0000_0200_0000`):
/// - When `new_addr == 0`, returns the current program break (`0x0200_0000+`).
/// - When `new_addr > current_brk` (within `USER_HEAP_BASE_VADDR..=USER_HEAP_MAX_VADDR`),
///   dynamically maps any required 4 KiB pages with strict `W^X` (`PRESENT | WRITABLE | USER | NX`),
///   registers the region in this `ProcessVmSpace`, and returns the updated program break.
pub fn expand_user_brk(new_addr: VirtAddr) -> VirtAddr {
    let (current, mapped_end) =
        with_active_vm_space(|space| (space.current_brk, space.mapped_brk_end));
    if new_addr == 0 {
        return current;
    }
    if !(USER_HEAP_BASE_VADDR..=USER_HEAP_MAX_VADDR).contains(&new_addr) {
        return current;
    }

    if new_addr > mapped_end {
        let required_end = (new_addr + (PAGE_SIZE as u64) - 1) & !((PAGE_SIZE as u64) - 1);
        let delta_bytes = (required_end - mapped_end) as usize;
        if map_user_segment(mapped_end, delta_bytes, true, false).is_err() {
            return current;
        }
        let total_mapped = (required_end - USER_HEAP_BASE_VADDR) as usize;
        with_active_vm_space(|space| {
            space.mapped_brk_end = required_end;
            space.upsert_region(USER_HEAP_BASE_VADDR, total_mapped, true, false);
            space.current_brk = new_addr;
        });
    } else {
        with_active_vm_space(|space| {
            space.current_brk = new_addr;
        });
    }

    new_addr
}


