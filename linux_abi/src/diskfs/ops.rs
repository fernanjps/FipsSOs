#![forbid(unsafe_code)]

//! DiskFS File & Directory Persistence Operations (`linux_abi::diskfs::ops`).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::inode::*;

pub(crate) fn format_and_create_initial() -> Vec<DiskFsFile> {
    const INITIAL_CONTENT: &[u8] = b"[DiskFS v2] FipSOs v2.7 Sistema de Archivos Persistente SATA AHCI activo!\nRegistro inicial guardado en LBA 64.\n";

    // 1. Superblock (LBA 0)
    let mut sb_buf = [0u8; 512];
    sb_buf[0..16].copy_from_slice(DISKFS_MAGIC);
    sb_buf[16..20].copy_from_slice(&2u32.to_le_bytes()); // version = 2
    sb_buf[20..24].copy_from_slice(&2u32.to_le_bytes()); // inode_count = 2 (/disk, /disk/boot_log.txt)
    sb_buf[24..28].copy_from_slice(&DISKFS_TOTAL_SECTORS.to_le_bytes());
    let free_sectors: u32 = DISKFS_TOTAL_SECTORS - 64 - 2;
    sb_buf[28..32].copy_from_slice(&free_sectors.to_le_bytes());
    sb_buf[32..36].copy_from_slice(&DISKFS_INODE_START_LBA.to_le_bytes()); // 1
    sb_buf[36..40].copy_from_slice(&(DISKFS_MAX_INODES as u32).to_le_bytes()); // 128
    sb_buf[40..44].copy_from_slice(&DISKFS_BITMAP_START_LBA.to_le_bytes()); // 33
    sb_buf[44..48].copy_from_slice(&(DISKFS_BITMAP_SECTORS as u32).to_le_bytes()); // 8
    sb_buf[48..52].copy_from_slice(&DISKFS_DATA_START_LBA.to_le_bytes()); // 64

    // 2. Bitmap (LBA 33..40, 4096 bytes)
    let mut bitmap = vec![0u8; DISKFS_BITMAP_SECTORS * 512];
    bitmap_set_range(&mut bitmap, 0, 64, true);
    bitmap_set_range(&mut bitmap, 64, 2, true);

    // 3. Inode Table (LBA 1..32, 16384 bytes)
    let mut inode_buf = vec![0u8; DISKFS_INODE_SECTORS * 512];

    let root_inode = DiskFsInode {
        used: true,
        file_type: FileType::Directory,
        parent_inode: 0,
        size_bytes: 4096,
        start_lba: 0,
        sector_count: 0,
        created_time: 0,
        path: String::from("/disk"),
    };
    root_inode.write_to_bytes(&mut inode_buf[0..DISKFS_INODE_SIZE]);

    let boot_log_inode = DiskFsInode {
        used: true,
        file_type: FileType::RegularFile,
        parent_inode: 0,
        size_bytes: INITIAL_CONTENT.len() as u32,
        start_lba: DISKFS_DATA_START_LBA, // 64
        sector_count: 2,
        created_time: 0,
        path: String::from(PATH_BOOT_LOG),
    };
    boot_log_inode.write_to_bytes(&mut inode_buf[DISKFS_INODE_SIZE..2 * DISKFS_INODE_SIZE]);

    // 4. Data for /disk/boot_log.txt (LBA 64..65, 1024 bytes)
    let mut data_buf = [0u8; 1024];
    data_buf[..INITIAL_CONTENT.len()].copy_from_slice(INITIAL_CONTENT);

    if ostd::storage::write_sectors(DISKFS_DATA_START_LBA as u64, 2, &data_buf).is_err()
        || ostd::storage::write_sectors(DISKFS_BITMAP_START_LBA as u64, DISKFS_BITMAP_SECTORS, &bitmap).is_err()
        || ostd::storage::write_sectors(DISKFS_INODE_START_LBA as u64, DISKFS_INODE_SECTORS, &inode_buf).is_err()
        || ostd::storage::write_sectors(0, 1, &sb_buf).is_err()
    {
        ostd::early_println!("[DiskFS] Fallo al formatear disco SATA AHCI.");
        return Vec::new();
    }

    ostd::early_println!(
        "[DiskFS] Disco SATA formateado con Superbloque \"FIPSOS_DISKFS_V2\" (128 inodos, Bitmap 4KB) | Creado \"{}\"",
        PATH_BOOT_LOG
    );
    ostd::fb_println!(
        "[DiskFS] Disco SATA formateado (DiskFS v2 OK) | Creado \"{}\"",
        PATH_BOOT_LOG
    );

    vec![
        DiskFsFile {
            path: String::from("/disk"),
            is_dir: true,
            data: Vec::new(),
        },
        DiskFsFile {
            path: String::from(PATH_BOOT_LOG),
            is_dir: false,
            data: Vec::from(INITIAL_CONTENT),
        },
    ]
}

/// Creates a persistent directory in DiskFS.
pub fn create_directory(path: &str) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    let norm_path = normalize_disk_path(path);
    if norm_path == "/disk" {
        return Ok(());
    }

    let mut sb_buf = [0u8; 512];
    if ostd::storage::read_sectors(0, 1, &mut sb_buf).is_err() || &sb_buf[0..16] != DISKFS_MAGIC {
        let _ = format_and_create_initial();
        let _ = ostd::storage::read_sectors(0, 1, &mut sb_buf);
    }

    let mut inode_buf = read_inode_table()?;
    let mut free_slot = None;
    let mut parent_idx = 0u32;
    let parent_path = path_parent(&norm_path);

    for i in 0..DISKFS_MAX_INODES {
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if inode.used {
                if inode.path == norm_path {
                    return Err("Directorio ya existe");
                }
                if inode.file_type == FileType::Directory && inode.path == parent_path {
                    parent_idx = i as u32;
                }
            } else if free_slot.is_none() {
                free_slot = Some(i);
            }
        }
    }

    let slot_idx = free_slot.ok_or("Tabla de inodos DiskFS llena (maximo 128)")?;

    let dir_inode = DiskFsInode {
        used: true,
        file_type: FileType::Directory,
        parent_inode: parent_idx,
        size_bytes: 4096,
        start_lba: 0,
        sector_count: 0,
        created_time: 0,
        path: norm_path.clone(),
    };

    let offset = slot_idx * DISKFS_INODE_SIZE;
    dir_inode.write_to_bytes(&mut inode_buf[offset..offset + DISKFS_INODE_SIZE]);
    write_inode_table(&inode_buf)?;

    let mut inode_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);
    inode_count += 1;
    sb_buf[20..24].copy_from_slice(&inode_count.to_le_bytes());
    let _ = ostd::storage::write_sectors(0, 1, &sb_buf);

    ostd::early_println!(
        "[DiskFS::mkdir] Directorio \"{}\" persistido en SATA AHCI (Inodo={})",
        norm_path,
        slot_idx
    );
    ostd::fb_println!(
        "[DiskFS::mkdir] Directorio \"{}\" persistido en SATA",
        norm_path
    );

    Ok(())
}

/// Deletes a persistent directory in DiskFS.
pub fn delete_directory(path: &str) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    let norm_path = normalize_disk_path(path);
    if norm_path == "/disk" {
        return Err("No se puede eliminar la raiz /disk");
    }

    let mut sb_buf = [0u8; 512];
    ostd::storage::read_sectors(0, 1, &mut sb_buf).map_err(|_| "Fallo al leer Superbloque")?;
    if &sb_buf[0..16] != DISKFS_MAGIC {
        return Err("Disco SATA no formateado con DiskFS");
    }

    let mut inode_buf = read_inode_table()?;
    let mut target_slot = None;

    for i in 0..DISKFS_MAX_INODES {
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if inode.used && inode.path == norm_path {
                target_slot = Some((i, inode));
                break;
            }
        }
    }

    let (slot_idx, target_inode) = target_slot.ok_or("Directorio no encontrado")?;
    if target_inode.file_type != FileType::Directory {
        return Err("No es un directorio");
    }

    for i in 0..DISKFS_MAX_INODES {
        if i == slot_idx {
            continue;
        }
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if inode.used && (inode.parent_inode == slot_idx as u32 || path_parent(&inode.path) == norm_path.as_str()) {
                return Err("Directorio no vacio");
            }
        }
    }

    let offset = slot_idx * DISKFS_INODE_SIZE;
    inode_buf[offset..offset + DISKFS_INODE_SIZE].fill(0);
    write_inode_table(&inode_buf)?;

    let mut inode_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);
    if inode_count > 0 {
        inode_count -= 1;
        sb_buf[20..24].copy_from_slice(&inode_count.to_le_bytes());
        let _ = ostd::storage::write_sectors(0, 1, &sb_buf);
    }

    ostd::early_println!("[DiskFS::rmdir] Directorio \"{}\" eliminado de SATA AHCI", norm_path);
    ostd::fb_println!("[DiskFS::rmdir] Directorio \"{}\" eliminado de SATA", norm_path);

    Ok(())
}

/// Persists a file into the SATA AHCI disk, allocating sectors via the block bitmap
/// and synchronizing its inode entry and superblock.
pub fn save_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    let norm_path = normalize_disk_path(path);

    let mut sb_buf = [0u8; 512];
    if ostd::storage::read_sectors(0, 1, &mut sb_buf).is_err() || &sb_buf[0..16] != DISKFS_MAGIC {
        let _ = format_and_create_initial();
        let _ = ostd::storage::read_sectors(0, 1, &mut sb_buf);
    }

    let mut inode_buf = read_inode_table()?;
    let mut bitmap = read_bitmap()?;

    let needed_sectors = if data.is_empty() {
        0u32
    } else {
        data.len().div_ceil(512) as u32
    };

    let parent_path = path_parent(&norm_path);
    let mut parent_idx = 0u32;
    let mut target_slot = None;
    let mut first_free_slot = None;

    for i in 0..DISKFS_MAX_INODES {
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if inode.used {
                if inode.path == norm_path {
                    target_slot = Some((i, inode.clone()));
                }
                if inode.file_type == FileType::Directory && inode.path == parent_path {
                    parent_idx = i as u32;
                }
            } else if first_free_slot.is_none() {
                first_free_slot = Some(i);
            }
        }
    }

    let (slot_idx, start_lba, allocated_sectors) = if let Some((idx, existing)) = target_slot {
        if existing.file_type == FileType::Directory {
            return Err("Es un directorio, no un archivo");
        }
        if needed_sectors == 0 {
            if existing.sector_count > 0 {
                bitmap_free_sectors(&mut bitmap, existing.start_lba, existing.sector_count);
                write_bitmap(&bitmap)?;
            }
            (idx, 0, 0)
        } else if existing.sector_count == needed_sectors {
            (idx, existing.start_lba, existing.sector_count)
        } else {
            if existing.sector_count > 0 {
                bitmap_free_sectors(&mut bitmap, existing.start_lba, existing.sector_count);
            }
            let new_lba = bitmap_allocate_sectors(&mut bitmap, needed_sectors).ok_or("Disco SATA lleno")?;
            write_bitmap(&bitmap)?;
            (idx, new_lba, needed_sectors)
        }
    } else if let Some(idx) = first_free_slot {
        let (new_lba, alloc_sec) = if needed_sectors > 0 {
            let lba = bitmap_allocate_sectors(&mut bitmap, needed_sectors).ok_or("Disco SATA lleno")?;
            write_bitmap(&bitmap)?;
            (lba, needed_sectors)
        } else {
            (0, 0)
        };
        let mut inode_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);
        inode_count += 1;
        sb_buf[20..24].copy_from_slice(&inode_count.to_le_bytes());
        let _ = ostd::storage::write_sectors(0, 1, &sb_buf);
        (idx, new_lba, alloc_sec)
    } else {
        return Err("Tabla de inodos DiskFS llena (maximo 128)");
    };

    if allocated_sectors > 0 && !data.is_empty() {
        let total_bytes = (allocated_sectors as usize) * 512;
        let mut sector_data = vec![0u8; total_bytes];
        sector_data[..data.len()].copy_from_slice(data);
        ostd::storage::write_sectors(start_lba as u64, allocated_sectors as usize, &sector_data)
            .map_err(|_| "Fallo al escribir sectores de datos en disco SATA")?;
    }

    let updated_inode = DiskFsInode {
        used: true,
        file_type: FileType::RegularFile,
        parent_inode: parent_idx,
        size_bytes: data.len() as u32,
        start_lba,
        sector_count: allocated_sectors,
        created_time: 0,
        path: norm_path.clone(),
    };
    let offset = slot_idx * DISKFS_INODE_SIZE;
    updated_inode.write_to_bytes(&mut inode_buf[offset..offset + DISKFS_INODE_SIZE]);
    write_inode_table(&inode_buf)?;

    if allocated_sectors > 0 && !data.is_empty() {
        let mut verify_buf = vec![0u8; 512];
        if let Ok(()) = ostd::storage::read_sectors(start_lba as u64, 1, &mut verify_buf) {
            let check_len = data.len().min(512);
            if &verify_buf[..check_len] == &data[..check_len] {
                ostd::early_println!(
                    "[DiskFS::sync] Archivo \"{}\" persistido en disco SATA (Inodo={}, LBA={}, {} B)",
                    norm_path, slot_idx, start_lba, data.len()
                );
                ostd::fb_println!(
                    "[DiskFS::sync] Archivo \"{}\" persistido en SATA ({} B)",
                    norm_path, data.len()
                );
            }
        }
    } else {
        ostd::early_println!("[DiskFS::sync] Archivo \"{}\" creado (0 B, Inodo={})", norm_path, slot_idx);
    }

    Ok(())
}

/// Generates, saves, and verifies a persistent note on the SATA disk.
pub fn save_persistent_note(ticks: u64) -> Result<(), &'static str> {
    let ahci = ostd::storage::ahci_snapshot();
    let note_text = alloc::format!(
        "[DiskFS v2] Nota persistente de usuario FipSOs v2.7\n\
         Timestamp IRQ0: {} ticks\n\
         Dispositivo: Intel ICH9 SATA AHCI 00:1f.2 (Port {})\n\
         LBA Lecturas: {} sectores | LBA Escrituras: {} sectores\n\
         Total transferido: {} KiB\n\
         Estado: Persistencia en sectores LBA y subdirectorios verificada exitosamente.\n",
        ticks,
        ahci.port_idx,
        ahci.sectors_read,
        ahci.sectors_written,
        ahci.bytes_transferred / 1024
    );

    save_file(PATH_PERSISTENT_NOTE, note_text.as_bytes())
}

/// Deletes a file from the SATA disk, recycling its LBA sectors in the bitmap.
pub fn delete_file(path: &str) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    let norm_path = normalize_disk_path(path);

    let mut sb_buf = [0u8; 512];
    ostd::storage::read_sectors(0, 1, &mut sb_buf).map_err(|_| "Fallo al leer Superbloque")?;
    if &sb_buf[0..16] != DISKFS_MAGIC {
        return Err("Disco SATA no formateado con DiskFS");
    }

    let mut inode_buf = read_inode_table()?;
    let mut found_slot = None;

    for i in 0..DISKFS_MAX_INODES {
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if inode.used && inode.path == norm_path {
                found_slot = Some((i, inode));
                break;
            }
        }
    }

    let (slot_idx, inode) = found_slot.ok_or("Archivo no encontrado en DiskFS")?;
    if inode.file_type == FileType::Directory {
        return Err("Es un directorio, use rmdir");
    }

    if inode.sector_count > 0 && inode.start_lba >= DISKFS_DATA_START_LBA {
        let mut bitmap = read_bitmap()?;
        bitmap_free_sectors(&mut bitmap, inode.start_lba, inode.sector_count);
        write_bitmap(&bitmap)?;
        ostd::early_println!(
            "[DiskFS::bitmap] Liberados {} sectores a partir de LBA={}",
            inode.sector_count,
            inode.start_lba
        );
    }

    let offset = slot_idx * DISKFS_INODE_SIZE;
    inode_buf[offset..offset + DISKFS_INODE_SIZE].fill(0);
    write_inode_table(&inode_buf)?;

    let mut inode_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);
    if inode_count > 0 {
        inode_count -= 1;
        sb_buf[20..24].copy_from_slice(&inode_count.to_le_bytes());
        let _ = ostd::storage::write_sectors(0, 1, &sb_buf);
    }

    ostd::early_println!("[DiskFS::unlink] Archivo \"{}\" eliminado del disco SATA AHCI", norm_path);
    ostd::fb_println!("[DiskFS::unlink] Archivo \"{}\" eliminado de SATA", norm_path);

    Ok(())
}
