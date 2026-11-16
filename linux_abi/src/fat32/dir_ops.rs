#![forbid(unsafe_code)]

//! Operaciones de Directorio y Archivos FAT32 (lectura, escritura, `mkdir` y `unlink`).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::fat_table::{allocate_cluster, collect_chain, free_chain};
use super::lfn::{
    assemble_lfn, build_83_record, build_lfn_records, build_short_name_83, decode_short_name,
    push_lfn_chunk, short_name_checksum,
};
use super::partition::{read_part_sector, write_part_sector};
use super::Fat32Volume;

pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LONG_NAME: u8 = 0x0F;

#[derive(Debug, Clone)]
pub struct Fat32Entry {
    pub name: String,
    pub short_name: String,
    pub is_dir: bool,
    pub first_cluster: u32,
    pub size: u32,
    pub start_slot_idx: usize,
    pub slot_count: usize,
}

fn cluster_lba(vol: &Fat32Volume, cluster: u32) -> Result<u64, &'static str> {
    if cluster < 2 || (cluster - 2) >= vol.total_clusters {
        return Err("Cluster fuera de rango en datos FAT32");
    }
    Ok(vol.cluster_begin_lba + ((cluster - 2) as u64) * (vol.sectors_per_cluster as u64))
}

fn read_cluster(vol: &Fat32Volume, cluster: u32) -> Result<Vec<u8>, &'static str> {
    let base = cluster_lba(vol, cluster)?;
    let spc = vol.sectors_per_cluster as usize;
    let mut buf = vec![0u8; spc * 512];
    for s in 0..spc {
        let mut sec = [0u8; 512];
        read_part_sector(&vol.partition, base + s as u64, &mut sec)?;
        buf[s * 512..(s + 1) * 512].copy_from_slice(&sec);
    }
    Ok(buf)
}

fn write_cluster(vol: &Fat32Volume, cluster: u32, data: &[u8]) -> Result<(), &'static str> {
    let base = cluster_lba(vol, cluster)?;
    let spc = vol.sectors_per_cluster as usize;
    for s in 0..spc {
        let mut sec = [0u8; 512];
        let off = s * 512;
        if off < data.len() {
            let copy_len = (data.len() - off).min(512);
            sec[..copy_len].copy_from_slice(&data[off..off + copy_len]);
        }
        write_part_sector(&vol.partition, base + s as u64, &sec)?;
    }
    Ok(())
}

pub fn list_cluster_entries(
    vol: &Fat32Volume,
    dir_cluster: u32,
) -> Result<(Vec<u32>, Vec<Fat32Entry>), &'static str> {
    let target_clus = if dir_cluster < 2 { vol.root_cluster } else { dir_cluster };
    let chain = collect_chain(vol, target_clus)?;
    let mut entries = Vec::new();

    let mut lfn_slots = [(0u8, [0xFFFFu16; 13]); 20];
    let mut lfn_count = 0usize;
    let mut lfn_seen = 0usize;
    let mut lfn_start_slot = 0usize;
    let mut global_slot = 0usize;

    let max_dir_clus = (vol.total_clusters as usize).min(4096);
    'outer: for &clus in chain.iter().take(max_dir_clus) {
        if clus < 2 || clus >= 0x0FFF_FFF8 {
            break;
        }
        let buf = read_cluster(vol, clus)?;
        for off in (0..buf.len()).step_by(32) {
            let raw = &buf[off..off + 32];
            if raw[0] == 0x00 {
                break 'outer;
            }
            if raw[0] == 0xE5 {
                lfn_count = 0;
                lfn_seen = 0;
                global_slot += 1;
                continue;
            }
            if raw[11] == ATTR_LONG_NAME {
                if (raw[0] & 0x40) != 0 {
                    lfn_slots = [(0u8, [0xFFFFu16; 13]); 20];
                    lfn_start_slot = global_slot;
                    lfn_count = ((raw[0] & 0x1F) as usize).min(20);
                    lfn_seen = 0;
                }
                if push_lfn_chunk(&mut lfn_slots, raw) > 0 {
                    lfn_seen += 1;
                } else {
                    lfn_count = 0;
                    lfn_seen = 0;
                }
                global_slot += 1;
                continue;
            }
            if (raw[11] & ATTR_VOLUME_ID) != 0 {
                lfn_count = 0;
                lfn_seen = 0;
                global_slot += 1;
                continue;
            }

            let short_name = decode_short_name(&raw[0..11]);
            let csum = short_name_checksum(&raw[0..11]);
            let valid_lfn = if lfn_count > 0 && lfn_seen == lfn_count {
                assemble_lfn(&lfn_slots, lfn_count, csum).filter(|s| !s.is_empty())
            } else {
                None
            };

            let (name, start_slot_idx, slot_count) = match valid_lfn {
                Some(lfn) if short_name != "." && short_name != ".." => {
                    (lfn, lfn_start_slot, lfn_count + 1)
                }
                _ => (short_name.clone(), global_slot, 1),
            };

            let hi = u16::from_le_bytes([raw[20], raw[21]]) as u32;
            let lo = u16::from_le_bytes([raw[26], raw[27]]) as u32;
            let first_cluster = ((hi << 16) | lo) & 0x0FFF_FFFF;
            let size = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
            let is_dir = (raw[11] & ATTR_DIRECTORY) != 0;

            entries.push(Fat32Entry {
                name,
                short_name,
                is_dir,
                first_cluster,
                size,
                start_slot_idx,
                slot_count,
            });
            lfn_count = 0;
            lfn_seen = 0;
            global_slot += 1;
        }
    }
    Ok((chain, entries))
}

fn find_entry<'a>(entries: &'a [Fat32Entry], name: &str) -> Option<&'a Fat32Entry> {
    entries
        .iter()
        .find(|e| e.name == name || e.short_name == name)
        .or_else(|| {
            entries
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name) || e.short_name.eq_ignore_ascii_case(name))
        })
}

/// Resuelve `(directorio_padre_cluster, nombre_final, entrada_existente)` desde ruta relativa.
pub fn resolve_parent_and_entry(
    vol: &Fat32Volume,
    rel_path: &str,
) -> Result<(u32, String, Option<Fat32Entry>), &'static str> {
    let trimmed = rel_path.trim_matches('/');
    if trimmed.is_empty() {
        return Ok((
            vol.root_cluster,
            String::new(),
            Some(Fat32Entry {
                name: String::from("/"),
                short_name: String::from("/"),
                is_dir: true,
                first_cluster: vol.root_cluster,
                size: 0,
                start_slot_idx: 0,
                slot_count: 0,
            }),
        ));
    }

    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() > 32 {
        return Err("Ruta FAT32 demasiado profunda");
    }
    let mut cur_dir = vol.root_cluster;

    for (idx, &seg) in parts.iter().enumerate() {
        if cur_dir < 2 || cur_dir >= 0x0FFF_FFF8 {
            break;
        }
        let (_chain, entries) = list_cluster_entries(vol, cur_dir)?;
        let found = find_entry(&entries, seg).cloned();
        if idx + 1 == parts.len() {
            return Ok((cur_dir, String::from(seg), found));
        }
        match found {
            Some(dir) if dir.is_dir => {
                cur_dir = if dir.first_cluster < 2 || dir.first_cluster >= 0x0FFF_FFF8 {
                    vol.root_cluster
                } else {
                    dir.first_cluster
                };
            }
            _ => return Err("Directorio padre no encontrado en FAT32"),
        }
    }
    Err("Ruta vacia")
}

pub fn read_file_bytes(vol: &Fat32Volume, rel_path: &str) -> Result<Vec<u8>, &'static str> {
    let (_parent, _name, maybe_entry) = resolve_parent_and_entry(vol, rel_path)?;
    let entry = maybe_entry.ok_or("Archivo no encontrado en FAT32")?;
    if entry.is_dir {
        return Err("Es un directorio");
    }
    if entry.size == 0 || entry.first_cluster < 2 || entry.first_cluster >= 0x0FFF_FFF8 {
        return Ok(Vec::new());
    }
    let chain = collect_chain(vol, entry.first_cluster)?;
    let bpc = vol.sectors_per_cluster as usize * 512;
    let bounded_size = (entry.size as usize).min(chain.len().saturating_mul(bpc));
    let mut out = Vec::with_capacity(bounded_size);
    let mut remaining = bounded_size;
    for clus in chain.into_iter().take(vol.total_clusters as usize) {
        if remaining == 0 || clus < 2 || clus >= 0x0FFF_FFF8 {
            break;
        }
        let cbuf = read_cluster(vol, clus)?;
        let take = remaining.min(cbuf.len());
        out.extend_from_slice(&cbuf[..take]);
        remaining -= take;
    }
    Ok(out)
}

fn find_contiguous_slots(free_map: &[bool], needed: usize) -> Option<usize> {
    let mut run = 0usize;
    for (idx, &is_free) in free_map.iter().enumerate() {
        run = if is_free { run + 1 } else { 0 };
        if run == needed {
            return Some(idx + 1 - needed);
        }
    }
    None
}

fn write_slot_records(
    vol: &Fat32Volume,
    dir_cluster: u32,
    records: &[[u8; 32]],
) -> Result<(), &'static str> {
    let mut chain = collect_chain(vol, dir_cluster)?;
    let slots_per_clus = (vol.sectors_per_cluster as usize * 512) / 32;
    let needed = records.len();

    let mut flat_slots = Vec::new();
    for &c in &chain {
        let buf = read_cluster(vol, c)?;
        for off in (0..buf.len()).step_by(32) {
            let b0 = buf[off];
            flat_slots.push(b0 == 0x00 || b0 == 0xE5);
        }
    }

    let mut found_start = find_contiguous_slots(&flat_slots, needed);
    let mut alloc_attempts = 0usize;
    while found_start.is_none() && alloc_attempts < 4 {
        let last = chain.last().copied().ok_or("Directorio padre vacio")?;
        let extra = allocate_cluster(vol, Some(last))?;
        let zeroed = vec![0u8; vol.sectors_per_cluster as usize * 512];
        write_cluster(vol, extra, &zeroed)?;
        chain.push(extra);
        flat_slots.resize(flat_slots.len() + slots_per_clus, true);
        found_start = find_contiguous_slots(&flat_slots, needed);
        alloc_attempts += 1;
    }

    let Some(start_slot) = found_start else {
        return Err("Sin espacio contiguo para entradas de directorio FAT32");
    };

    for (i, rec) in records.iter().enumerate() {
        let g = start_slot + i;
        let clus = *chain.get(g / slots_per_clus).ok_or("Indice de cluster fuera de rango")?;
        let off = (g % slots_per_clus) * 32;
        let mut buf = read_cluster(vol, clus)?;
        buf[off..off + 32].copy_from_slice(rec);
        write_cluster(vol, clus, &buf)?;
    }
    Ok(())
}

fn mark_slots_deleted(vol: &Fat32Volume, dir_cluster: u32, start_slot: usize, count: usize) -> Result<(), &'static str> {
    let chain = collect_chain(vol, dir_cluster)?;
    let slots_per_clus = (vol.sectors_per_cluster as usize * 512) / 32;
    for g in start_slot..(start_slot + count) {
        let c_idx = g / slots_per_clus;
        if c_idx >= chain.len() {
            break;
        }
        let clus = chain[c_idx];
        let off = (g % slots_per_clus) * 32;
        let mut buf = read_cluster(vol, clus)?;
        buf[off] = 0xE5;
        write_cluster(vol, clus, &buf)?;
    }
    Ok(())
}

fn write_data_chain(vol: &Fat32Volume, data: &[u8]) -> Result<u32, &'static str> {
    if data.is_empty() {
        return Ok(0);
    }
    let bpc = vol.sectors_per_cluster as usize * 512;
    let needed = data.len().div_ceil(bpc);
    let mut first = 0u32;
    let mut prev: Option<u32> = None;

    for idx in 0..needed {
        let clus = allocate_cluster(vol, prev)?;
        if first == 0 {
            first = clus;
        }
        let start = idx * bpc;
        let end = (start + bpc).min(data.len());
        write_cluster(vol, clus, &data[start..end])?;
        prev = Some(clus);
    }
    Ok(first)
}

pub fn write_file_bytes(vol: &Fat32Volume, rel_path: &str, data: &[u8]) -> Result<(), &'static str> {
    let (parent_dir, name, existing) = resolve_parent_and_entry(vol, rel_path)?;
    if name.is_empty() || name == "." || name == ".." {
        return Err("Nombre de archivo invalido");
    }
    if let Some(ref e) = existing {
        if e.is_dir {
            return Err("Es un directorio");
        }
    }

    let new_first_clus = write_data_chain(vol, data)?;
    if let Some(old) = existing {
        let old_clus = old.first_cluster;
        let entry_slot = old.start_slot_idx + old.slot_count.saturating_sub(1);
        let chain = collect_chain(vol, parent_dir)?;
        let slots_per_clus = (vol.sectors_per_cluster as usize * 512) / 32;
        let clus = *chain.get(entry_slot / slots_per_clus).ok_or("Slot de entrada fuera de rango")?;
        let off = (entry_slot % slots_per_clus) * 32;
        let mut buf = read_cluster(vol, clus)?;
        let hi = ((new_first_clus >> 16) as u16).to_le_bytes();
        let lo = ((new_first_clus & 0xFFFF) as u16).to_le_bytes();
        buf[off + 20..off + 22].copy_from_slice(&hi);
        buf[off + 26..off + 28].copy_from_slice(&lo);
        buf[off + 28..off + 32].copy_from_slice(&(data.len() as u32).to_le_bytes());
        write_cluster(vol, clus, &buf)?;
        if old_clus >= 2 {
            let _ = free_chain(vol, old_clus);
        }
        return Ok(());
    }

    let (_, siblings) = list_cluster_entries(vol, parent_dir)?;
    let raw83 = build_short_name_83(&name, &siblings);
    let csum = short_name_checksum(&raw83);
    let mut records = build_lfn_records(&name, csum);
    records.push(build_83_record(&raw83, ATTR_ARCHIVE, new_first_clus, data.len() as u32));
    write_slot_records(vol, parent_dir, &records)
}

pub fn create_directory(vol: &Fat32Volume, rel_path: &str) -> Result<(), &'static str> {
    let (parent_dir, name, existing) = resolve_parent_and_entry(vol, rel_path)?;
    if name.is_empty() || name == "." || name == ".." || existing.is_some() {
        return Err("Directorio ya existe o nombre invalido");
    }

    let new_clus = allocate_cluster(vol, None)?;
    let mut dir_buf = vec![0u8; vol.sectors_per_cluster as usize * 512];
    let dot_raw = *b".          ";
    let dotdot_raw = *b"..         ";
    let parent_dotdot = if parent_dir == vol.root_cluster { 0 } else { parent_dir };
    dir_buf[0..32].copy_from_slice(&build_83_record(&dot_raw, ATTR_DIRECTORY, new_clus, 0));
    dir_buf[32..64].copy_from_slice(&build_83_record(&dotdot_raw, ATTR_DIRECTORY, parent_dotdot, 0));
    write_cluster(vol, new_clus, &dir_buf)?;

    let (_, siblings) = list_cluster_entries(vol, parent_dir)?;
    let raw83 = build_short_name_83(&name, &siblings);
    let csum = short_name_checksum(&raw83);
    let mut records = build_lfn_records(&name, csum);
    records.push(build_83_record(&raw83, ATTR_DIRECTORY, new_clus, 0));
    write_slot_records(vol, parent_dir, &records)
}

pub fn remove_entry(vol: &Fat32Volume, rel_path: &str, expect_dir: bool) -> Result<(), &'static str> {
    let (parent_dir, name, existing) = resolve_parent_and_entry(vol, rel_path)?;
    if name.is_empty() || name == "." || name == ".." {
        return Err("Operacion no permitida sobre entrada especial FAT32");
    }
    let entry = existing.ok_or("Entrada no encontrada en FAT32")?;
    if expect_dir && !entry.is_dir {
        return Err("No es un directorio");
    }
    if !expect_dir && entry.is_dir {
        return Err("Es un directorio");
    }
    if expect_dir && entry.first_cluster >= 2 {
        let (_, children) = list_cluster_entries(vol, entry.first_cluster)?;
        if children.iter().any(|c| c.name != "." && c.name != "..") {
            return Err("Directorio no vacio");
        }
    }
    mark_slots_deleted(vol, parent_dir, entry.start_slot_idx, entry.slot_count)?;
    if entry.first_cluster >= 2 {
        free_chain(vol, entry.first_cluster)?;
    }
    Ok(())
}
