#![forbid(unsafe_code)]

//! Gestión de la Tabla de Asignación de Archivos FAT32 (entradas de 32 bits,
//! recorrido de cadenas de clústeres, reserva libre y sincronización FAT1/FAT2).

use alloc::vec::Vec;

use super::partition::{read_part_sector, write_part_sector};
use super::Fat32Volume;

pub const FAT32_MASK: u32 = 0x0FFF_FFFF;
pub const FAT32_FREE: u32 = 0x0000_0000;
pub const FAT32_BAD: u32 = 0x0FFF_FFF7;
pub const FAT32_EOC_MIN: u32 = 0x0FFF_FFF8;
pub const FAT32_EOC_WRITE: u32 = 0x0FFF_FFFF;

const MAX_CHAIN_LEN: usize = 65536;

/// Lee la entrada de 32 bits de la tabla FAT1 para `cluster` (enmascarada con `0x0FFF_FFFF`).
pub fn read_fat_entry(vol: &Fat32Volume, cluster: u32) -> Result<u32, &'static str> {
    if cluster < 2 || (cluster - 2) >= vol.total_clusters {
        return Err("Numero de cluster fuera de rango en lectura FAT");
    }
    let byte_off = (cluster as u64) * 4;
    let rel_lba = vol.fat_begin_lba + (byte_off / 512);
    let in_sec = (byte_off % 512) as usize;

    let mut sec = [0u8; 512];
    read_part_sector(&vol.partition, rel_lba, &mut sec)?;
    let raw = u32::from_le_bytes([
        sec[in_sec],
        sec[in_sec + 1],
        sec[in_sec + 2],
        sec[in_sec + 3],
    ]);
    Ok(raw & FAT32_MASK)
}

/// Escribe `value` (28 bits bajos) preservando los 4 bits altos reservados y sincronizando FAT1/FAT2.
pub fn write_fat_entry(vol: &Fat32Volume, cluster: u32, value: u32) -> Result<(), &'static str> {
    if cluster < 2 || (cluster - 2) >= vol.total_clusters {
        return Err("Numero de cluster fuera de rango en escritura FAT");
    }
    let byte_off = (cluster as u64) * 4;
    let fat_sec_offset = byte_off / 512;
    let in_sec = (byte_off % 512) as usize;

    for fat_idx in 0..(vol.num_fats as u64) {
        let rel_lba = vol.fat_begin_lba + fat_idx * (vol.fat_size_sectors as u64) + fat_sec_offset;
        let mut sec = [0u8; 512];
        read_part_sector(&vol.partition, rel_lba, &mut sec)?;
        let existing = u32::from_le_bytes([
            sec[in_sec],
            sec[in_sec + 1],
            sec[in_sec + 2],
            sec[in_sec + 3],
        ]);
        let updated = (existing & !FAT32_MASK) | (value & FAT32_MASK);
        sec[in_sec..in_sec + 4].copy_from_slice(&updated.to_le_bytes());
        write_part_sector(&vol.partition, rel_lba, &sec)?;
    }
    Ok(())
}

/// Recorre la cadena completa de clústeres comenzando en `start_cluster`.
pub fn collect_chain(vol: &Fat32Volume, start_cluster: u32) -> Result<Vec<u32>, &'static str> {
    let mut chain = Vec::new();
    let mut cur = start_cluster & FAT32_MASK;
    let max_iters = (vol.total_clusters as usize).min(MAX_CHAIN_LEN);

    for _ in 0..max_iters {
        if cur < 2 || cur >= FAT32_EOC_MIN {
            break;
        }
        if cur == FAT32_BAD {
            return Err("Cluster defectuoso (BAD) en cadena FAT32");
        }
        if (cur - 2) >= vol.total_clusters || chain.contains(&cur) {
            return Err("Cadena FAT32 corrupta, ciclica o fuera de limites");
        }
        chain.push(cur);
        cur = read_fat_entry(vol, cur)?;
    }
    if cur >= 2 && cur < FAT32_EOC_MIN && chain.len() >= max_iters {
        return Err("Cadena FAT32 excede limite maximo de clusters");
    }
    Ok(chain)
}

fn find_free_cluster(vol: &Fat32Volume, start_hint: u32) -> Result<u32, &'static str> {
    let max_cluster = 2u32.saturating_add(vol.total_clusters);
    let hint = start_hint.clamp(2, max_cluster.saturating_sub(1));

    let ranges = [hint..max_cluster, 2..hint];
    let mut cached_lba = u64::MAX;
    let mut sec = [0u8; 512];

    for range in ranges {
        for c in range {
            let byte_off = (c as u64) * 4;
            let rel_lba = vol.fat_begin_lba + (byte_off / 512);
            let in_sec = (byte_off % 512) as usize;
            if rel_lba != cached_lba {
                read_part_sector(&vol.partition, rel_lba, &mut sec)?;
                cached_lba = rel_lba;
            }
            let val = u32::from_le_bytes([
                sec[in_sec],
                sec[in_sec + 1],
                sec[in_sec + 2],
                sec[in_sec + 3],
            ]) & FAT32_MASK;
            if val == FAT32_FREE {
                return Ok(c);
            }
        }
    }
    Err("Sin espacio libre en tabla FAT32")
}

fn update_fsinfo_delta(vol: &Fat32Volume, delta_free: i64, last_alloc: Option<u32>) {
    if vol.fsinfo_sector == 0 || (vol.fsinfo_sector as u64) >= vol.fat_begin_lba {
        return;
    }
    let mut sec = [0u8; 512];
    if read_part_sector(&vol.partition, vol.fsinfo_sector as u64, &mut sec).is_err() {
        return;
    }
    let lead = u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]);
    let struc = u32::from_le_bytes([sec[484], sec[485], sec[486], sec[487]]);
    let trail = u32::from_le_bytes([sec[508], sec[509], sec[510], sec[511]]);
    if lead != 0x4161_5252 || struc != 0x6141_7272 || trail != 0xAA55_0000 {
        return;
    }
    let cur_free = u32::from_le_bytes([sec[488], sec[489], sec[490], sec[491]]);
    if cur_free != 0xFFFF_FFFF {
        let next_free = (cur_free as i64)
            .saturating_add(delta_free)
            .clamp(0, vol.total_clusters as i64) as u32;
        sec[488..492].copy_from_slice(&next_free.to_le_bytes());
    }
    if let Some(alloc_c) = last_alloc {
        sec[492..496].copy_from_slice(&alloc_c.to_le_bytes());
    }
    let _ = write_part_sector(&vol.partition, vol.fsinfo_sector as u64, &sec);
}

/// Reserva un nuevo clúster libre, lo marca `EOC` y lo enlaza al final de `prev_cluster` si existe.
pub fn allocate_cluster(vol: &Fat32Volume, prev_cluster: Option<u32>) -> Result<u32, &'static str> {
    let hint = prev_cluster.map_or(2, |c| c.saturating_add(1));
    let new_cluster = find_free_cluster(vol, hint)?;
    write_fat_entry(vol, new_cluster, FAT32_EOC_WRITE)?;
    if let Some(prev) = prev_cluster {
        write_fat_entry(vol, prev, new_cluster)?;
    }
    update_fsinfo_delta(vol, -1, Some(new_cluster));
    Ok(new_cluster)
}

/// Libera toda la cadena de clústeres a partir de `start_cluster` en FAT1 y FAT2.
pub fn free_chain(vol: &Fat32Volume, start_cluster: u32) -> Result<(), &'static str> {
    let chain = collect_chain(vol, start_cluster)?;
    for &c in &chain {
        write_fat_entry(vol, c, FAT32_FREE)?;
    }
    if !chain.is_empty() {
        update_fsinfo_delta(vol, chain.len() as i64, None);
    }
    Ok(())
}
