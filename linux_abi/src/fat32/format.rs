#![forbid(unsafe_code)]

//! Auto-inicializacion segura de particion FAT32 en `LBA 2048` (`linux_abi::fat32::format`).
//! Respeta estrictamente `LBA 0..2047` (DiskFS) y nunca formatea discos GPT/MBR externos.

use alloc::vec;

use super::partition::is_fat32_boot_sector;

pub const FAT32_DEFAULT_START_LBA: u64 = 2048;
const MIN_FORMAT_TOTAL_SECTORS: u64 = 16384; // >= 8 MiB
const MAX_AUTO_FAT32_SECTORS: u64 = 262_144; // 128 MiB
const CHUNK_SECTORS: usize = 64;

fn write_zero_span(abs_lba: u64, total_sectors: u64) -> Result<(), &'static str> {
    let buf = vec![0u8; CHUNK_SECTORS * 512];
    let mut done = 0u64;
    while done < total_sectors {
        let count = ((total_sectors - done) as usize).min(CHUNK_SECTORS);
        ostd::storage::write_sectors(abs_lba + done, count, &buf[..count * 512])?;
        done += count as u64;
    }
    Ok(())
}

/// Formats a clean FAT32 volume starting at `LBA 2048` if the disk is writable,
/// not protected (not external GPT/MBR), and does not already hold a valid FAT32 VBR at LBA 2048.
pub fn auto_format_fat32_at_lba2048() -> Result<(), &'static str> {
    if !ostd::storage::is_storage_ready() {
        return Err("Almacenamiento SATA/NVMe no listo");
    }
    if crate::diskfs::is_protected_mode() {
        return Err("Modo protegido activo: disco externo no se formatea");
    }

    let mut lba0 = [0u8; 512];
    let mut lba1 = [0u8; 512];
    ostd::storage::read_sectors(0, 1, &mut lba0)?;
    let _ = ostd::storage::read_sectors(1, 1, &mut lba1);

    if crate::diskfs::is_external_gpt_or_mbr(&lba0, &lba1) {
        return Err("Tabla GPT/MBR externa detectada: formateo FAT32 omitido");
    }
    let is_diskfs = lba0.starts_with(b"FIPSOS_DISKFS_V2");
    let is_blank = lba0.iter().all(|&b| b == 0);
    if !is_diskfs && !is_blank {
        return Err("Firma desconocida en LBA 0: formateo FAT32 omitido");
    }

    let total_sectors = ostd::storage::total_sectors();
    if total_sectors < MIN_FORMAT_TOTAL_SECTORS || total_sectors <= FAT32_DEFAULT_START_LBA + 128 {
        return Err("Disco demasiado pequeno para particion FAT32 auto-inicializada");
    }

    let mut existing_vbr = [0u8; 512];
    if ostd::storage::read_sectors(FAT32_DEFAULT_START_LBA, 1, &mut existing_vbr).is_ok()
        && is_fat32_boot_sector(&existing_vbr)
    {
        return Ok(());
    }

    let part_sectors = (total_sectors - FAT32_DEFAULT_START_LBA).min(MAX_AUTO_FAT32_SECTORS) as u32;
    let spc: u8 = 8; // 4 KiB clusters
    let reserved: u16 = 32;
    let num_fats: u8 = 2;
    let max_clus = (part_sectors - reserved as u32) / (spc as u32);
    let fat_sz32 = ((max_clus + 2) * 4).div_ceil(512).max(1);
    let fat_begin_rel = reserved as u64;
    let cluster_begin_rel = fat_begin_rel + (num_fats as u64) * (fat_sz32 as u64);
    if cluster_begin_rel + (spc as u64) >= part_sectors as u64 {
        return Err("Geometria insuficiente para raiz FAT32");
    }
    let data_clusters = ((part_sectors as u64 - cluster_begin_rel) / (spc as u64)) as u32;
    if data_clusters < 64 {
        return Err("Clusters insuficientes para volumen FAT32");
    }

    // 1. Volume Boot Record (BPB FAT32)
    let mut vbr = [0u8; 512];
    vbr[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    vbr[3..11].copy_from_slice(b"FIPSOS63");
    vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
    vbr[13] = spc;
    vbr[14..16].copy_from_slice(&reserved.to_le_bytes());
    vbr[16] = num_fats;
    vbr[21] = 0xF8;
    vbr[24..26].copy_from_slice(&63u16.to_le_bytes());
    vbr[26..28].copy_from_slice(&255u16.to_le_bytes());
    vbr[28..32].copy_from_slice(&(FAT32_DEFAULT_START_LBA as u32).to_le_bytes());
    vbr[32..36].copy_from_slice(&part_sectors.to_le_bytes());
    vbr[36..40].copy_from_slice(&fat_sz32.to_le_bytes());
    vbr[44..48].copy_from_slice(&2u32.to_le_bytes()); // root_cluster = 2
    vbr[48..50].copy_from_slice(&1u16.to_le_bytes()); // fsinfo = 1
    vbr[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup_boot = 6
    vbr[64] = 0x80;
    vbr[66] = 0x29;
    vbr[67..71].copy_from_slice(&0x2026_0603u32.to_le_bytes());
    vbr[71..82].copy_from_slice(b"FIPSOS_FAT0");
    vbr[82..90].copy_from_slice(b"FAT32   ");
    vbr[510] = 0x55;
    vbr[511] = 0xAA;

    // 2. FSInfo sector (rel 1)
    let mut fsinfo = [0u8; 512];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes()); // RRaA
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes()); // rrAa
    fsinfo[488..492].copy_from_slice(&data_clusters.saturating_sub(1).to_le_bytes());
    fsinfo[492..496].copy_from_slice(&3u32.to_le_bytes()); // nxt_free = 3
    fsinfo[510] = 0x55;
    fsinfo[511] = 0xAA;

    // Write reserved area (32 sectors) + VBR/FSInfo + backup @ 6/7
    write_zero_span(FAT32_DEFAULT_START_LBA, reserved as u64)?;
    ostd::storage::write_sectors(FAT32_DEFAULT_START_LBA, 1, &vbr)?;
    ostd::storage::write_sectors(FAT32_DEFAULT_START_LBA + 1, 1, &fsinfo)?;
    ostd::storage::write_sectors(FAT32_DEFAULT_START_LBA + 6, 1, &vbr)?;
    ostd::storage::write_sectors(FAT32_DEFAULT_START_LBA + 7, 1, &fsinfo)?;

    // 3. Zero FAT1 + FAT2 and write initial FAT entries (0, 1, root=2 EOC)
    let mut fat_sec0 = [0u8; 512];
    fat_sec0[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    fat_sec0[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    fat_sec0[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes()); // cluster 2 EOF

    let fat1_lba = FAT32_DEFAULT_START_LBA + fat_begin_rel;
    let fat2_lba = fat1_lba + fat_sz32 as u64;
    write_zero_span(fat1_lba, fat_sz32 as u64)?;
    ostd::storage::write_sectors(fat1_lba, 1, &fat_sec0)?;
    write_zero_span(fat2_lba, fat_sz32 as u64)?;
    ostd::storage::write_sectors(fat2_lba, 1, &fat_sec0)?;

    // 4. Root directory cluster 2 (Volume ID entry + zeros)
    let root_lba = FAT32_DEFAULT_START_LBA + cluster_begin_rel;
    let mut root_buf = vec![0u8; (spc as usize) * 512];
    root_buf[0..11].copy_from_slice(b"FIPSOS_FAT0");
    root_buf[11] = 0x08; // ATTR_VOLUME_ID
    ostd::storage::write_sectors(root_lba, spc as usize, &root_buf)?;

    ostd::early_println!(
        "[FAT32] Auto-inicializado limpio en LBA {} ({} MiB, {} clusters) preservando DiskFS LBA 0..2047",
        FAT32_DEFAULT_START_LBA,
        (part_sectors as u64 * 512) / (1024 * 1024),
        data_clusters
    );
    ostd::fb_println!(
        "[FAT32] Auto-formateado en LBA {} ({} MiB, /mnt/fat0)",
        FAT32_DEFAULT_START_LBA,
        (part_sectors as u64 * 512) / (1024 * 1024)
    );
    Ok(())
}

/// Seeds `/mnt/fat0/{fipsos.txt, hello.lua, seq_model.fnn}` when absent on a mounted FAT32 volume.
pub fn seed_default_files() {
    if super::stat_path("/mnt/fat0/fipsos.txt").is_none() {
        let _ = super::write_file("/mnt/fat0/fipsos.txt", b"FipSOs v6.3 FAT32 OK (LBA 2048)\n");
    }
    if super::stat_path("/mnt/fat0/hello.lua").is_none() {
        let _ = super::write_file("/mnt/fat0/hello.lua", b"print(\"FipSOs v6.3 Lua on FAT32 OK\")\n");
    }
    if super::stat_path("/mnt/fat0/seq_model.fnn").is_none() {
        let mut model = alloc::vec::Vec::with_capacity(20 + 108 * 4);
        for word in [0x324E_4E46u32, 8, 8, 4, 0] {
            model.extend_from_slice(&word.to_le_bytes());
        }
        for i in 0..108i32 {
            let w = ((i * 7 - 35) as f32) * 0.02;
            model.extend_from_slice(&w.to_le_bytes());
        }
        let _ = super::write_file("/mnt/fat0/seq_model.fnn", &model);
    }
}
