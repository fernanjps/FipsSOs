#![forbid(unsafe_code)]

//! Driver Nativo FAT32 de Lectura/Escritura para FipSOs (`/mnt/fat0`).
//!
//! Monta automáticamente particiones FAT32 en GPT, MBR o Superfloppy manteniendo
//! estricto `#![forbid(unsafe_code)]` y respeto total al escudo anti-borrado.

use alloc::vec::Vec;
use ostd::sync::SpinLock;

pub mod dir_ops;
pub mod fat_table;
pub mod format;
pub mod lfn;
pub mod partition;

pub use dir_ops::Fat32Entry;
pub use partition::{Fat32Partition, PartitionScheme};

pub const FAT32_MOUNT_POINT: &str = "/mnt/fat0";

/// Parámetros BPB calculados para un volumen FAT32 montado.
#[derive(Debug, Clone, Copy)]
pub struct Fat32Volume {
    pub partition: Fat32Partition,
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub fat_size_sectors: u32,
    pub root_cluster: u32,
    pub fsinfo_sector: u16,
    pub fat_begin_lba: u64,
    pub cluster_begin_lba: u64,
    pub total_clusters: u32,
}

static FAT32_VOLUME: SpinLock<Option<Fat32Volume>> = SpinLock::new(None);

fn parse_bpb(partition: Fat32Partition) -> Result<Fat32Volume, &'static str> {
    let mut bpb = [0u8; 512];
    partition::read_part_sector(&partition, 0, &mut bpb)?;
    if !partition::is_fat32_boot_sector(&bpb) {
        return Err("BPB FAT32 invalido");
    }

    let bytes_per_sector = u16::from_le_bytes([bpb[11], bpb[12]]);
    let sectors_per_cluster = bpb[13];
    let reserved_sectors = u16::from_le_bytes([bpb[14], bpb[15]]);
    let num_fats = bpb[16];
    let fat_size_sectors = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]);
    let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]) & 0x0FFF_FFFF;
    let fsinfo_sector = u16::from_le_bytes([bpb[48], bpb[49]]);

    let fat_begin_lba = reserved_sectors as u64;
    let cluster_begin_lba =
        fat_begin_lba + (num_fats as u64) * (fat_size_sectors as u64);
    if cluster_begin_lba >= partition.total_sectors {
        return Err("Geometria FAT32 inconsistente con tamaño de particion");
    }
    let data_sectors = partition.total_sectors - cluster_begin_lba;
    let total_clusters_u64 = data_sectors / (sectors_per_cluster as u64);
    if total_clusters_u64 < 1 || total_clusters_u64 > 0x0FFF_FFF5 {
        return Err("Cantidad de clusters FAT32 fuera de limites");
    }
    let min_fat_sectors = ((total_clusters_u64 + 2) * 4).div_ceil(512);
    if (fat_size_sectors as u64) < min_fat_sectors {
        return Err("Tabla FAT32 mas pequeña que los clusters de datos");
    }
    let total_clusters = total_clusters_u64 as u32;
    if root_cluster < 2 || (root_cluster - 2) >= total_clusters {
        return Err("Cluster raiz FAT32 invalido");
    }

    Ok(Fat32Volume {
        partition,
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        fat_size_sectors,
        root_cluster,
        fsinfo_sector,
        fat_begin_lba,
        cluster_begin_lba,
        total_clusters,
    })
}

fn strip_mount_prefix(vfs_path: &str) -> Option<&str> {
    if vfs_path == FAT32_MOUNT_POINT {
        Some("")
    } else {
        vfs_path.strip_prefix("/mnt/fat0/")
    }
}

/// Detecta y monta automáticamente una partición FAT32 válida en `/mnt/fat0`.
pub fn mount_fat32() -> Option<Fat32Volume> {
    partition::invalidate_sector_cache();
    if !ostd::storage::is_storage_ready() {
        ostd::early_println!("[FAT32] Sin disco SATA/NVMe grabable presente, omitiendo montaje /mnt/fat0.");
        return None;
    }
    let Some(part) = partition::detect_fat32_partition() else {
        ostd::early_println!("[FAT32] Sin particion FAT32 valida en disco, omitiendo /mnt/fat0.");
        return None;
    };
    match parse_bpb(part) {
        Ok(vol) => {
            let mib = (vol.partition.total_sectors.saturating_mul(512)) / (1024 * 1024);
            let cluster_bytes = (vol.sectors_per_cluster as u32) * (vol.bytes_per_sector as u32);
            ostd::early_println!(
                "[FAT32] Particion {} #{} (LBA={}, {} MiB, cluster={} B, root_clus={}) montada en {}",
                vol.partition.scheme.as_str(),
                vol.partition.index,
                vol.partition.start_lba,
                mib,
                cluster_bytes,
                vol.root_cluster,
                FAT32_MOUNT_POINT
            );
            ostd::fb_println!(
                "[FAT32] Particion {} montada en {} ({} MiB, cluster={} B)",
                vol.partition.scheme.as_str(),
                FAT32_MOUNT_POINT,
                mib,
                cluster_bytes
            );
            *FAT32_VOLUME.lock() = Some(vol);
            Some(vol)
        }
        Err(e) => {
            ostd::early_println!("[FAT32] Particion omitida: {}", e);
            None
        }
    }
}

/// Monta una partición FAT32 existente en `/mnt/fat0` o auto-formatea LBA 2048 en discos compatibles.
pub fn mount_or_init() -> Option<Fat32Volume> {
    if let Some(vol) = mount_fat32() {
        return Some(vol);
    }
    if !ostd::storage::is_storage_ready() || crate::diskfs::is_protected_mode() {
        return None;
    }
    match format::auto_format_fat32_at_lba2048() {
        Ok(()) => mount_fat32(),
        Err(reason) => {
            ostd::early_println!("[FAT32] Auto-inicializacion omitida: {}", reason);
            None
        }
    }
}

pub fn is_mounted() -> bool {
    FAT32_VOLUME.lock().is_some()
}

pub fn is_dir(vfs_path: &str) -> bool {
    let Some(rel) = strip_mount_prefix(vfs_path) else {
        return false;
    };
    let guard = FAT32_VOLUME.lock();
    let Some(vol) = *guard else {
        return false;
    };
    if rel.is_empty() {
        return true;
    }
    matches!(
        dir_ops::resolve_parent_and_entry(&vol, rel),
        Ok((_, _, Some(e))) if e.is_dir
    )
}

pub fn stat_path(vfs_path: &str) -> Option<(bool, usize)> {
    let rel = strip_mount_prefix(vfs_path)?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard)?;
    if rel.is_empty() {
        return Some((true, 4096));
    }
    match dir_ops::resolve_parent_and_entry(&vol, rel) {
        Ok((_, _, Some(e))) => Some((e.is_dir, e.size as usize)),
        _ => None,
    }
}

pub fn list_dir(vfs_path: &str) -> Vec<Fat32Entry> {
    let Some(rel) = strip_mount_prefix(vfs_path) else {
        return Vec::new();
    };
    let guard = FAT32_VOLUME.lock();
    let Some(vol) = *guard else {
        return Vec::new();
    };
    let target_clus = if rel.is_empty() {
        vol.root_cluster
    } else {
        match dir_ops::resolve_parent_and_entry(&vol, rel) {
            Ok((_, _, Some(e))) if e.is_dir => {
                if e.first_cluster < 2 || e.first_cluster >= 0x0FFF_FFF8 {
                    vol.root_cluster
                } else {
                    e.first_cluster
                }
            }
            _ => return Vec::new(),
        }
    };
    dir_ops::list_cluster_entries(&vol, target_clus)
        .map(|(_, entries)| entries)
        .unwrap_or_default()
}

pub fn read_file(vfs_path: &str) -> Result<Vec<u8>, &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::read_file_bytes(&vol, rel)
}

pub fn try_read_file(vfs_path: &str) -> Result<Vec<u8>, &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let Some(guard) = FAT32_VOLUME.try_lock() else {
        return Err("Volumen FAT32 ocupado");
    };
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::read_file_bytes(&vol, rel)
}

pub fn write_file(vfs_path: &str, data: &[u8]) -> Result<(), &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::write_file_bytes(&vol, rel, data)
}

pub fn try_write_file(vfs_path: &str, data: &[u8]) -> Result<(), &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let Some(guard) = FAT32_VOLUME.try_lock() else {
        return Err("Volumen FAT32 ocupado");
    };
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::write_file_bytes(&vol, rel, data)
}

pub fn mkdir(vfs_path: &str) -> Result<(), &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::create_directory(&vol, rel)
}

pub fn unlink(vfs_path: &str) -> Result<(), &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::remove_entry(&vol, rel, false)
}

pub fn rmdir(vfs_path: &str) -> Result<(), &'static str> {
    let rel = strip_mount_prefix(vfs_path).ok_or("Ruta fuera de /mnt/fat0")?;
    let guard = FAT32_VOLUME.lock();
    let vol = (*guard).ok_or("Volumen FAT32 no montado")?;
    dir_ops::remove_entry(&vol, rel, true)
}
