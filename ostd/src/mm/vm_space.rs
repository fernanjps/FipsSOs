use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    frame::{allocate_frame, deallocate_frame},
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

const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Physical address of the master Kernel `PML4` root captured at boot (`ostd::mm::init`).
static MASTER_KERNEL_PML4: AtomicU64 = AtomicU64::new(0);

/// Tracked virtual memory region mapped in Lower-Half User Space for a process `PML4`.
#[derive(Debug, Clone, Copy)]
pub struct UserVmRegion {
    pub start_vaddr: VirtAddr,
    pub size_bytes: usize,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, Copy)]
struct MappedVmoCacheEntry {
    phys_addr: PhysAddr,
    aligned_size: usize,
    writable: bool,
    user_vaddr: VirtAddr,
}

/// Per-process Virtual Memory Space descriptor indexed by its hardware `CR3` (`PML4` physical frame).
///
/// Isolates Lower-Half (`0..256`) `UserVmRegion` bounds, `sys_brk` heap cursors (`0x0200_0000+`),
/// and `sys_mmap` VMO cursors (`0x1000_0000+`) so every process owns an independent address space.
pub struct ProcessVmSpace {
    pub pml4_phys: PhysAddr,
    pub regions: Vec<UserVmRegion>,
    vmo_cache: Vec<MappedVmoCacheEntry>,
    pub current_brk: VirtAddr,
    pub mapped_brk_end: VirtAddr,
    pub next_mmap_vaddr: VirtAddr,
}

impl ProcessVmSpace {
    fn new(pml4_phys: PhysAddr) -> Self {
        Self {
            pml4_phys,
            regions: Vec::new(),
            vmo_cache: Vec::new(),
            current_brk: USER_HEAP_BASE_VADDR,
            mapped_brk_end: USER_HEAP_BASE_VADDR,
            next_mmap_vaddr: USER_MMAP_BASE_VADDR,
        }
    }

    fn upsert_region(
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
}

static PROCESS_VM_SPACES: SpinLock<Vec<ProcessVmSpace>> = SpinLock::new(Vec::new());

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

fn with_active_vm_space<R>(f: impl FnOnce(&mut ProcessVmSpace) -> R) -> R {
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
