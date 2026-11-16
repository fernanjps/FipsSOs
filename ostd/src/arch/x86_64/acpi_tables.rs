//! Real ACPI Table Parser (RSDP, RSDT/XSDT, MADT/APIC, FADT/FACP) via Limine `RsdpRequest`.
//!
//! Zero hardcoded fallbacks: if ACPI tables are absent or fail checksum validation,
//! `AcpiInfo::valid` remains `false` and callers must handle absence explicitly.

use core::ptr::read_unaligned;
use crate::mm::{hhdm_offset, map_kernel_mmio};
use crate::sync::SpinLock;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV1 {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV2 {
    v1: RsdpV1,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct GenericAddress {
    pub address_space: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// Discovered I/O APIC entry from ACPI MADT.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoApicInfo {
    pub id: u8,
    pub mmio_addr: u32,
    pub gsi_base: u32,
}

/// Parsed hardware facts from ACPI RSDP -> XSDT/RSDT -> MADT & FADT.
#[derive(Clone, Copy, Debug)]
pub struct AcpiInfo {
    pub valid: bool,
    pub revision: u8,
    pub rsdp_phys: u64,
    pub madt_phys: u64,
    pub fadt_phys: u64,
    pub local_apic_addr: u64,
    pub cpu_count: u8,
    pub local_apic_ids: [u8; 16],
    pub ioapic_count: u8,
    pub ioapics: [IoApicInfo; 4],
    pub iso_count: u8,
    pub pm1a_cnt_blk: u32,
    pub pm1b_cnt_blk: u32,
    pub reset_reg_space: u8,
    pub reset_reg_addr: u64,
    pub reset_value: u8,
}

impl AcpiInfo {
    const fn empty() -> Self {
        Self {
            valid: false,
            revision: 0,
            rsdp_phys: 0,
            madt_phys: 0,
            fadt_phys: 0,
            local_apic_addr: 0,
            cpu_count: 0,
            local_apic_ids: [0; 16],
            ioapic_count: 0,
            ioapics: [IoApicInfo { id: 0, mmio_addr: 0, gsi_base: 0 }; 4],
            iso_count: 0,
            pm1a_cnt_blk: 0,
            pm1b_cnt_blk: 0,
            reset_reg_space: 0,
            reset_reg_addr: 0,
            reset_value: 0,
        }
    }
}

static ACPI_INFO: SpinLock<AcpiInfo> = SpinLock::new(AcpiInfo::empty());

#[inline]
fn map_acpi(phys: u64, size: usize) -> *const u8 {
    let p = to_phys(phys);
    map_kernel_mmio(p, size) as *const u8
}

#[inline]
fn to_phys(addr: u64) -> u64 {
    let hhdm = hhdm_offset();
    if addr >= hhdm { addr - hhdm } else { addr }
}

unsafe fn checksum_ok(ptr: *const u8, len: usize) -> bool {
    if ptr.is_null() || len == 0 {
        return false;
    }
    let mut sum = 0u8;
    for i in 0..len {
        sum = sum.wrapping_add(*ptr.add(i));
    }
    sum == 0
}

unsafe fn parse_madt(phys: u64, info: &mut AcpiInfo) {
    let hdr_ptr = map_acpi(phys, core::mem::size_of::<SdtHeader>());
    let hdr = read_unaligned(hdr_ptr as *const SdtHeader);
    let len = hdr.length as usize;
    if len < 44 || len > 65536 {
        return;
    }
    let base = map_acpi(phys, len);
    if !checksum_ok(base, len) {
        return;
    }
    info.madt_phys = phys;
    info.local_apic_addr = read_unaligned(base.add(36) as *const u32) as u64;

    let mut off = 44usize;
    while off + 2 <= len {
        let rec_type = *base.add(off);
        let rec_len = *base.add(off + 1) as usize;
        if rec_len < 2 || off + rec_len > len {
            break;
        }
        match rec_type {
            0 if rec_len >= 8 => {
                let apic_id = *base.add(off + 3);
                let flags = read_unaligned(base.add(off + 4) as *const u32);
                if (flags & 0x03) != 0 && (info.cpu_count as usize) < info.local_apic_ids.len() {
                    let idx = info.cpu_count as usize;
                    info.local_apic_ids[idx] = apic_id;
                    info.cpu_count += 1;
                }
            }
            1 if rec_len >= 12 => {
                let id = *base.add(off + 2);
                let mmio_addr = read_unaligned(base.add(off + 4) as *const u32);
                let gsi_base = read_unaligned(base.add(off + 8) as *const u32);
                if (info.ioapic_count as usize) < info.ioapics.len() {
                    let idx = info.ioapic_count as usize;
                    info.ioapics[idx] = IoApicInfo { id, mmio_addr, gsi_base };
                    info.ioapic_count += 1;
                }
            }
            2 if rec_len >= 10 => {
                info.iso_count = info.iso_count.saturating_add(1);
            }
            5 if rec_len >= 12 => {
                info.local_apic_addr = read_unaligned(base.add(off + 4) as *const u64);
            }
            _ => {}
        }
        off += rec_len;
    }
}

unsafe fn parse_fadt(phys: u64, info: &mut AcpiInfo) {
    let hdr_ptr = map_acpi(phys, core::mem::size_of::<SdtHeader>());
    let hdr = read_unaligned(hdr_ptr as *const SdtHeader);
    let len = hdr.length as usize;
    if len < 76 || len > 65536 {
        return;
    }
    let base = map_acpi(phys, len);
    if !checksum_ok(base, len) {
        return;
    }
    info.fadt_phys = phys;
    info.pm1a_cnt_blk = read_unaligned(base.add(64) as *const u32);
    info.pm1b_cnt_blk = read_unaligned(base.add(68) as *const u32);
    if len >= 129 {
        let reset_ga = read_unaligned(base.add(116) as *const GenericAddress);
        info.reset_reg_space = reset_ga.address_space;
        info.reset_reg_addr = reset_ga.address;
        info.reset_value = *base.add(128);
    }
    if len >= 220 {
        let x_pm1a = read_unaligned(base.add(208) as *const GenericAddress);
        if info.pm1a_cnt_blk == 0 && x_pm1a.address_space == 1 && x_pm1a.address != 0 {
            info.pm1a_cnt_blk = x_pm1a.address as u32;
        }
    }
}

/// Discovers and parses ACPI RSDP, XSDT/RSDT, MADT, and FADT tables from Limine.
pub fn init() {
    let Some(raw_addr) = crate::boot::rsdp_addr() else {
        crate::early_println!("[ostd::acpi] Limine RsdpRequest sin respuesta (ACPI no disponible)");
        return;
    };
    let rsdp_phys = to_phys(raw_addr);
    let rsdp_ptr = map_acpi(rsdp_phys, 64);

    let mut info = AcpiInfo::empty();
    info.rsdp_phys = rsdp_phys;

    // SAFETY: `rsdp_ptr` points to Limine-provided RSDP mapped in HHDM.
    unsafe {
        let v1 = read_unaligned(rsdp_ptr as *const RsdpV1);
        if &v1.signature != b"RSD PTR " || !checksum_ok(rsdp_ptr, core::mem::size_of::<RsdpV1>()) {
            crate::early_println!("[ostd::acpi] RSDP invalido o checksum v1 fallido en {:#x}", rsdp_phys);
            return;
        }
        info.revision = v1.revision;
        let (sdt_phys, use_xsdt) = if v1.revision >= 2 {
            let v2 = read_unaligned(rsdp_ptr as *const RsdpV2);
            let v2_len = (v2.length as usize).clamp(36, 64);
            if checksum_ok(rsdp_ptr, v2_len) && v2.xsdt_address != 0 {
                (v2.xsdt_address, true)
            } else {
                (v1.rsdt_address as u64, false)
            }
        } else {
            (v1.rsdt_address as u64, false)
        };

        let sdt_hdr_ptr = map_acpi(sdt_phys, core::mem::size_of::<SdtHeader>());
        let sdt_hdr = read_unaligned(sdt_hdr_ptr as *const SdtHeader);
        let sdt_len = sdt_hdr.length as usize;
        if sdt_len < 36 || sdt_len > 65536 {
            crate::early_println!("[ostd::acpi] XSDT/RSDT longitud invalida ({}) en {:#x}", sdt_len, sdt_phys);
            return;
        }
        let sdt_ptr = map_acpi(sdt_phys, sdt_len);
        if !checksum_ok(sdt_ptr, sdt_len) {
            crate::early_println!("[ostd::acpi] XSDT/RSDT checksum fallido en {:#x}", sdt_phys);
            return;
        }
        let entry_sz = if use_xsdt { 8usize } else { 4usize };
        let count = (sdt_len - 36) / entry_sz;
        for i in 0..count {
            let tbl_phys = if use_xsdt {
                read_unaligned(sdt_ptr.add(36 + i * 8) as *const u64)
            } else {
                read_unaligned(sdt_ptr.add(36 + i * 4) as *const u32) as u64
            };
            if tbl_phys == 0 {
                continue;
            }
            let sig = *(map_acpi(tbl_phys, 4) as *const [u8; 4]);
            if &sig == b"APIC" {
                parse_madt(tbl_phys, &mut info);
            } else if &sig == b"FACP" {
                parse_fadt(tbl_phys, &mut info);
            }
        }
        info.valid = true;
    }

    *ACPI_INFO.lock() = info;
    crate::early_println!(
        "[ostd::acpi] RSDP@{:#x} (rev={}) | MADT@{:#x} LAPIC={:#x} CPUs={} IOAPICs={} ISO={}",
        info.rsdp_phys, info.revision, info.madt_phys, info.local_apic_addr,
        info.cpu_count, info.ioapic_count, info.iso_count
    );
    crate::early_println!(
        "[ostd::acpi] FADT@{:#x} PM1a_CNT={:#x} PM1b_CNT={:#x} RESET_REG=(space={}, addr={:#x}, val={:#x})",
        info.fadt_phys, info.pm1a_cnt_blk, info.pm1b_cnt_blk,
        info.reset_reg_space, info.reset_reg_addr, info.reset_value
    );
}

/// Returns the parsed ACPI hardware table snapshot.
pub fn info() -> AcpiInfo {
    *ACPI_INFO.lock()
}

/// Returns the FADT `PM1a_CNT_BLK` I/O port if discovered.
pub fn pm1a_cnt_port() -> Option<u16> {
    let i = *ACPI_INFO.lock();
    if i.valid && i.pm1a_cnt_blk != 0 && i.pm1a_cnt_blk <= u16::MAX as u32 {
        Some(i.pm1a_cnt_blk as u16)
    } else {
        None
    }
}

/// Returns the FADT `RESET_REG` `(address_space, address, reset_value)` if discovered.
pub fn reset_reg() -> Option<(u8, u64, u8)> {
    let i = *ACPI_INFO.lock();
    if i.valid && i.reset_reg_addr != 0 {
        Some((i.reset_reg_space, i.reset_reg_addr, i.reset_value))
    } else {
        None
    }
}
