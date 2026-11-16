#![forbid(unsafe_code)]

//! Parser de Tablas de Particiones Reales (`GPT`, `MBR` y `Superfloppy FAT32`).
//!
//! Respeta estrictamente el escudo anti-borrado de la v3.7: jamás formatea ni altera
//! particiones externas NTFS/ext4/sistema; valida la firma FAT32 antes de montar y
//! restringe toda E/S de sectores al rango `[start_lba, start_lba + total_sectors)`.

/// Esquema de particionado detectado en el medio físico.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionScheme {
    Gpt,
    Mbr,
    Superfloppy,
}

impl PartitionScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gpt => "GPT",
            Self::Mbr => "MBR",
            Self::Superfloppy => "Superfloppy",
        }
    }
}

/// Descriptor de partición FAT32 acotada en LBAs de 512 bytes.
#[derive(Debug, Clone, Copy)]
pub struct Fat32Partition {
    pub scheme: PartitionScheme,
    pub index: u8,
    pub start_lba: u64,
    pub total_sectors: u64,
}

/// GUID `EBD0A0A2-B9E5-4433-87C0-68B6B72699C7` (Microsoft Basic Data) en orden mixed-endian GPT.
const GUID_MS_BASIC_DATA: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// GUID `C12A7328-F81F-11D2-BA4B-00A0C93EC93B` (EFI System Partition) en orden mixed-endian GPT.
const GUID_EFI_SYSTEM: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

use alloc::vec::Vec;
use ostd::sync::SpinLock;

const CACHE_SLOTS: usize = 512;

#[derive(Clone, Copy)]
struct CachedSector {
    valid: bool,
    abs_lba: u64,
    data: [u8; 512],
}

impl CachedSector {
    const fn empty() -> Self {
        Self {
            valid: false,
            abs_lba: 0,
            data: [0u8; 512],
        }
    }
}

static SECTOR_CACHE: SpinLock<Vec<CachedSector>> = SpinLock::new(Vec::new());

/// Invalidates the in-RAM FAT32 sector cache (called when mounting/remounting volumes).
pub fn invalidate_sector_cache() {
    let mut cache = SECTOR_CACHE.lock();
    for slot in cache.iter_mut() {
        slot.valid = false;
    }
}

/// Lee un sector de 512 bytes relativo al inicio de la partición FAT32, usando caché en RAM.
pub fn read_part_sector(
    part: &Fat32Partition,
    rel_lba: u64,
    dst: &mut [u8; 512],
) -> Result<(), &'static str> {
    if rel_lba >= part.total_sectors {
        return Err("Lectura FAT32 fuera de los limites de la particion");
    }
    let abs_lba = part
        .start_lba
        .checked_add(rel_lba)
        .ok_or("Desbordamiento de LBA en lectura FAT32")?;

    let idx = (abs_lba as usize) % CACHE_SLOTS;
    {
        let cache = SECTOR_CACHE.lock();
        if cache.len() == CACHE_SLOTS {
            let slot = &cache[idx];
            if slot.valid && slot.abs_lba == abs_lba {
                dst.copy_from_slice(&slot.data);
                return Ok(());
            }
        }
    }

    ostd::storage::read_sectors(abs_lba, 1, dst)?;
    let mut cache = SECTOR_CACHE.lock();
    if cache.len() != CACHE_SLOTS {
        cache.resize(CACHE_SLOTS, CachedSector::empty());
    }
    cache[idx] = CachedSector {
        valid: true,
        abs_lba,
        data: *dst,
    };
    Ok(())
}

/// Escribe un sector de 512 bytes relativo al inicio de la partición FAT32 con write-through en caché RAM.
pub fn write_part_sector(
    part: &Fat32Partition,
    rel_lba: u64,
    src: &[u8; 512],
) -> Result<(), &'static str> {
    if rel_lba >= part.total_sectors {
        return Err("Escritura FAT32 fuera de los limites de la particion");
    }
    let abs_lba = part
        .start_lba
        .checked_add(rel_lba)
        .ok_or("Desbordamiento de LBA en escritura FAT32")?;

    ostd::storage::write_sectors(abs_lba, 1, src)?;
    let idx = (abs_lba as usize) % CACHE_SLOTS;
    let mut cache = SECTOR_CACHE.lock();
    if cache.len() != CACHE_SLOTS {
        cache.resize(CACHE_SLOTS, CachedSector::empty());
    }
    cache[idx] = CachedSector {
        valid: true,
        abs_lba,
        data: *src,
    };
    Ok(())
}

/// Comprueba si un sector de 512 bytes corresponde a un Volume Boot Record (BPB) FAT32 válido.
pub fn is_fat32_boot_sector(sec: &[u8; 512]) -> bool {
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return false;
    }
    if &sec[0x52..0x5A] != b"FAT32   " {
        return false;
    }
    let bytes_per_sec = u16::from_le_bytes([sec[11], sec[12]]);
    let sec_per_clus = sec[13];
    let reserved = u16::from_le_bytes([sec[14], sec[15]]);
    let num_fats = sec[16];
    let fat_sz32 = u32::from_le_bytes([sec[36], sec[37], sec[38], sec[39]]);

    bytes_per_sec == 512
        && sec_per_clus > 0
        && sec_per_clus.is_power_of_two()
        && reserved >= 1
        && (1..=2).contains(&num_fats)
        && fat_sz32 > 0
}

fn validate_fat32_candidate(
    scheme: PartitionScheme,
    index: u8,
    start_lba: u64,
    total_sectors: u64,
) -> Option<Fat32Partition> {
    if start_lba == 0 && scheme != PartitionScheme::Superfloppy {
        return None;
    }
    let disk_sectors = ostd::storage::total_sectors();
    let bounded_sectors = if disk_sectors > 0 {
        if start_lba >= disk_sectors {
            return None;
        }
        total_sectors.min(disk_sectors - start_lba)
    } else {
        total_sectors
    };
    if bounded_sectors < 64 {
        return None;
    }
    let mut vbr = [0u8; 512];
    if ostd::storage::read_sectors(start_lba, 1, &mut vbr).is_err() {
        return None;
    }
    if !is_fat32_boot_sector(&vbr) {
        return None;
    }
    Some(Fat32Partition {
        scheme,
        index,
        start_lba,
        total_sectors: bounded_sectors,
    })
}

fn probe_gpt() -> Option<Fat32Partition> {
    let mut hdr = [0u8; 512];
    if ostd::storage::read_sectors(1, 1, &mut hdr).is_err() {
        return None;
    }
    if &hdr[0..8] != b"EFI PART" {
        return None;
    }

    let entry_lba = u64::from_le_bytes([
        hdr[72], hdr[73], hdr[74], hdr[75], hdr[76], hdr[77], hdr[78], hdr[79],
    ]);
    let num_entries = u32::from_le_bytes([hdr[80], hdr[81], hdr[82], hdr[83]]).min(128);
    let entry_size = u32::from_le_bytes([hdr[84], hdr[85], hdr[86], hdr[87]]) as usize;
    if entry_lba < 2 || entry_size < 128 || !entry_size.is_power_of_two() || entry_size > 512 {
        return None;
    }

    let entries_per_sector = 512 / entry_size;
    let total_sectors_to_scan = (num_entries as usize).div_ceil(entries_per_sector).min(32);

    let mut sec = [0u8; 512];
    for s_idx in 0..total_sectors_to_scan {
        let Some(scan_lba) = entry_lba.checked_add(s_idx as u64) else {
            break;
        };
        if ostd::storage::read_sectors(scan_lba, 1, &mut sec).is_err() {
            break;
        }
        for slot in 0..entries_per_sector {
            let entry_idx = s_idx * entries_per_sector + slot;
            if entry_idx >= num_entries as usize {
                break;
            }
            let off = slot * entry_size;
            let type_guid = &sec[off..off + 16];
            if type_guid != GUID_MS_BASIC_DATA && type_guid != GUID_EFI_SYSTEM {
                continue;
            }
            let first_lba = u64::from_le_bytes([
                sec[off + 32],
                sec[off + 33],
                sec[off + 34],
                sec[off + 35],
                sec[off + 36],
                sec[off + 37],
                sec[off + 38],
                sec[off + 39],
            ]);
            let last_lba = u64::from_le_bytes([
                sec[off + 40],
                sec[off + 41],
                sec[off + 42],
                sec[off + 43],
                sec[off + 44],
                sec[off + 45],
                sec[off + 46],
                sec[off + 47],
            ]);
            if last_lba <= first_lba {
                continue;
            }
            let Some(count) = (last_lba - first_lba).checked_add(1) else {
                continue;
            };
            if let Some(part) = validate_fat32_candidate(
                PartitionScheme::Gpt,
                (entry_idx + 1) as u8,
                first_lba,
                count,
            ) {
                return Some(part);
            }
        }
    }
    None
}

fn probe_mbr(lba0: &[u8; 512]) -> Option<Fat32Partition> {
    for idx in 0..4usize {
        let off = 446 + idx * 16;
        let ptype = lba0[off + 4];
        if !matches!(ptype, 0x0B | 0x0C | 0x0E) {
            continue;
        }
        let start_lba =
            u32::from_le_bytes([lba0[off + 8], lba0[off + 9], lba0[off + 10], lba0[off + 11]]) as u64;
        let sectors =
            u32::from_le_bytes([lba0[off + 12], lba0[off + 13], lba0[off + 14], lba0[off + 15]]) as u64;
        if let Some(part) = validate_fat32_candidate(
            PartitionScheme::Mbr,
            (idx + 1) as u8,
            start_lba,
            sectors,
        ) {
            return Some(part);
        }
    }
    None
}

/// Detecta la primera partición FAT32 válida en esquema GPT, MBR o volumen Superfloppy.
pub fn detect_fat32_partition() -> Option<Fat32Partition> {
    if !ostd::storage::is_storage_ready() {
        return None;
    }

    let mut lba0 = [0u8; 512];
    if ostd::storage::read_sectors(0, 1, &mut lba0).is_err() {
        return None;
    }

    if lba0[510] == 0x55 && lba0[511] == 0xAA {
        // 1. Volumen Superfloppy FAT32 directo sin tabla de particiones
        if is_fat32_boot_sector(&lba0) {
            let tot16 = u16::from_le_bytes([lba0[19], lba0[20]]) as u64;
            let tot32 = u32::from_le_bytes([lba0[32], lba0[33], lba0[34], lba0[35]]) as u64;
            let total = if tot32 != 0 { tot32 } else { tot16 };
            if let Some(part) = validate_fat32_candidate(PartitionScheme::Superfloppy, 0, 0, total)
            {
                return Some(part);
            }
        }

        // 2. Esquema GPT (Protective MBR 0xEE o cabecera válida "EFI PART" en LBA 1)
        if let Some(part) = probe_gpt() {
            return Some(part);
        }

        // 3. Esquema MBR tradicional (tipos FAT32 0x0B, 0x0C, 0x0E)
        if let Some(part) = probe_mbr(&lba0) {
            return Some(part);
        }
    }

    // 4. Híbrido FipSOs: LBA 0..2047 reservado para DiskFS y FAT32 en LBA 2048
    let total_disk = ostd::storage::total_sectors();
    if total_disk > 2048 + 64 {
        let mut vbr = [0u8; 512];
        if ostd::storage::read_sectors(2048, 1, &mut vbr).is_ok() && is_fat32_boot_sector(&vbr) {
            let tot32 = u32::from_le_bytes([vbr[32], vbr[33], vbr[34], vbr[35]]) as u64;
            let part_len = if tot32 != 0 { tot32 } else { total_disk - 2048 };
            if let Some(part) = validate_fat32_candidate(PartitionScheme::Mbr, 1, 2048, part_len) {
                return Some(part);
            }
        }
    }

    None
}
