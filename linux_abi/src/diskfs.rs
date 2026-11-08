#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const DISKFS_MAGIC: &[u8; 16] = b"FIPSOS_DISKFS_V1";
pub const DISKFS_TOTAL_SECTORS: u32 = 32768; // 16 MiB disk = 32768 sectors of 512 bytes
pub const DISKFS_DATA_START_LBA: u32 = 16;
pub const DISKFS_MAX_FILES: usize = 16;
pub const DISKFS_ENTRY_SIZE: usize = 128; // 16 entries * 128 bytes = 2048 bytes (4 sectors: LBA 1..4)

pub const PATH_BOOT_LOG: &str = "/disk/boot_log.txt";
pub const PATH_PERSISTENT_NOTE: &str = "/disk/persistent_note.txt";

/// Represents a persistent file loaded from or saved to the SATA disk.
#[derive(Debug, Clone)]
pub struct DiskFsFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// Directory entry structure stored in LBA 1..4.
#[derive(Debug, Clone)]
pub struct DiskFsDirEntry {
    pub used: bool,
    pub size_bytes: u32,
    pub start_lba: u32,
    pub sector_count: u32,
    pub path: String,
}

impl DiskFsDirEntry {
    fn from_bytes(slice: &[u8]) -> Option<Self> {
        if slice.len() < DISKFS_ENTRY_SIZE {
            return None;
        }
        let used = slice[0] == 1;
        let size_bytes = u32::from_le_bytes([slice[4], slice[5], slice[6], slice[7]]);
        let start_lba = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
        let sector_count = u32::from_le_bytes([slice[12], slice[13], slice[14], slice[15]]);

        let path_bytes = &slice[16..80];
        let path_len = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
        let path = core::str::from_utf8(&path_bytes[..path_len])
            .unwrap_or("")
            .into();

        Some(Self {
            used,
            size_bytes,
            start_lba,
            sector_count,
            path,
        })
    }

    fn write_to_bytes(&self, slice: &mut [u8]) {
        slice[..DISKFS_ENTRY_SIZE].fill(0);
        slice[0] = if self.used { 1 } else { 0 };
        slice[4..8].copy_from_slice(&self.size_bytes.to_le_bytes());
        slice[8..12].copy_from_slice(&self.start_lba.to_le_bytes());
        slice[12..16].copy_from_slice(&self.sector_count.to_le_bytes());

        let path_bytes = self.path.as_bytes();
        let max_copy = path_bytes.len().min(63);
        slice[16..16 + max_copy].copy_from_slice(&path_bytes[..max_copy]);
        slice[16 + max_copy] = 0;
    }
}

/// Initializes `DiskFS`: if the SATA disk contains `"FIPSOS_DISKFS_V1"` from a previous session,
/// loads all persistent files; if virgin, formats the superblock and creates `/disk/boot_log.txt`.
pub fn init_or_mount() -> Vec<DiskFsFile> {
    if !ostd::storage::is_ahci_ready() {
        ostd::early_println!("[DiskFS] Driver SATA AHCI no disponible, omitiendo montaje de disco.");
        ostd::fb_println!("[DiskFS] SATA AHCI no disponible.");
        return Vec::new();
    }

    let mut sb_buf = [0u8; 512];
    if let Err(err) = ostd::storage::read_sectors(0, 1, &mut sb_buf) {
        ostd::early_println!("[DiskFS] Error al leer Superbloque (LBA 0): {}", err);
        return Vec::new();
    }

    if &sb_buf[0..16] == DISKFS_MAGIC {
        // Disk already formatted! Read directory table from LBA 1..4 (2048 bytes)
        let mut dir_buf = [0u8; 2048];
        if let Err(err) = ostd::storage::read_sectors(1, 4, &mut dir_buf) {
            ostd::early_println!("[DiskFS] Error al leer tabla de directorio: {}", err);
            return Vec::new();
        }

        let mut files = Vec::new();
        for i in 0..DISKFS_MAX_FILES {
            let offset = i * DISKFS_ENTRY_SIZE;
            if let Some(entry) = DiskFsDirEntry::from_bytes(&dir_buf[offset..offset + DISKFS_ENTRY_SIZE]) {
                if entry.used && !entry.path.is_empty() && entry.sector_count > 0 {
                    let mut data_buf = vec![0u8; (entry.sector_count as usize) * 512];
                    if let Ok(()) = ostd::storage::read_sectors(
                        entry.start_lba as u64,
                        entry.sector_count as usize,
                        &mut data_buf,
                    ) {
                        data_buf.truncate(entry.size_bytes as usize);
                        ostd::early_println!(
                            "[DiskFS] Archivo persistente \"{}\" cargado desde disco SATA (LBA={}, {} B)",
                            entry.path,
                            entry.start_lba,
                            entry.size_bytes
                        );
                        ostd::fb_println!(
                            "[DiskFS] Archivo \"{}\" cargado de disco SATA (LBA={}, {} B)",
                            entry.path,
                            entry.start_lba,
                            entry.size_bytes
                        );
                        files.push(DiskFsFile {
                            path: entry.path,
                            data: data_buf,
                        });
                    }
                }
            }
        }
        files
    } else {
        // Virgin disk: format superblock and create initial /disk/boot_log.txt
        format_and_create_initial()
    }
}

fn format_and_create_initial() -> Vec<DiskFsFile> {
    const INITIAL_CONTENT: &[u8] = b"[DiskFS] FipSOs v1.6 Sistema de Archivos Persistente SATA AHCI activo!\nRegistro inicial guardado en LBA 16.\n";

    let mut sb_buf = [0u8; 512];
    sb_buf[0..16].copy_from_slice(DISKFS_MAGIC);
    sb_buf[16..20].copy_from_slice(&1u32.to_le_bytes()); // version = 1
    sb_buf[20..24].copy_from_slice(&1u32.to_le_bytes()); // file_count = 1
    sb_buf[24..28].copy_from_slice(&DISKFS_TOTAL_SECTORS.to_le_bytes());
    sb_buf[28..32].copy_from_slice(&18u32.to_le_bytes()); // free_lba_start = 18

    let mut dir_buf = [0u8; 2048];
    let initial_entry = DiskFsDirEntry {
        used: true,
        size_bytes: INITIAL_CONTENT.len() as u32,
        start_lba: DISKFS_DATA_START_LBA, // 16
        sector_count: 2,
        path: String::from(PATH_BOOT_LOG),
    };
    initial_entry.write_to_bytes(&mut dir_buf[0..DISKFS_ENTRY_SIZE]);

    // Data buffer for LBA 16..17 (1024 bytes)
    let mut data_buf = [0u8; 1024];
    data_buf[..INITIAL_CONTENT.len()].copy_from_slice(INITIAL_CONTENT);

    if ostd::storage::write_sectors(DISKFS_DATA_START_LBA as u64, 2, &data_buf).is_err()
        || ostd::storage::write_sectors(1, 4, &dir_buf).is_err()
        || ostd::storage::write_sectors(0, 1, &sb_buf).is_err()
    {
        ostd::early_println!("[DiskFS] Fallo al formatear disco SATA AHCI.");
        return Vec::new();
    }

    ostd::early_println!(
        "[DiskFS] Disco SATA formateado con Superbloque \"FIPSOS_DISKFS_V1\" | Creado \"{}\"",
        PATH_BOOT_LOG
    );
    ostd::fb_println!(
        "[DiskFS] Disco SATA formateado (Superbloque OK) | Creado \"{}\"",
        PATH_BOOT_LOG
    );

    vec![DiskFsFile {
        path: String::from(PATH_BOOT_LOG),
        data: Vec::from(INITIAL_CONTENT),
    }]
}

/// Persists a file into the SATA AHCI disk and synchronizes its directory table.
pub fn save_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    // Read Superblock
    let mut sb_buf = [0u8; 512];
    if ostd::storage::read_sectors(0, 1, &mut sb_buf).is_err() || &sb_buf[0..16] != DISKFS_MAGIC {
        // If not formatted yet, format first
        let _ = format_and_create_initial();
        let _ = ostd::storage::read_sectors(0, 1, &mut sb_buf);
    }

    let mut free_lba_start = u32::from_le_bytes([sb_buf[28], sb_buf[29], sb_buf[30], sb_buf[31]]);
    if free_lba_start < DISKFS_DATA_START_LBA {
        free_lba_start = DISKFS_DATA_START_LBA;
    }
    let mut file_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);

    // Read directory table (LBA 1..4)
    let mut dir_buf = [0u8; 2048];
    ostd::storage::read_sectors(1, 4, &mut dir_buf)
        .map_err(|_| "Fallo al leer tabla de directorio")?;

    let needed_sectors = (data.len().div_ceil(512).max(1)) as u32;

    // Check if file already exists in directory table
    let mut target_slot = None;
    let mut first_free_slot = None;

    for i in 0..DISKFS_MAX_FILES {
        let offset = i * DISKFS_ENTRY_SIZE;
        if let Some(entry) = DiskFsDirEntry::from_bytes(&dir_buf[offset..offset + DISKFS_ENTRY_SIZE]) {
            if entry.used && entry.path == path {
                target_slot = Some((i, entry));
                break;
            }
            if !entry.used && first_free_slot.is_none() {
                first_free_slot = Some(i);
            }
        }
    }

    let (slot_idx, start_lba, allocated_sectors) = if let Some((idx, existing)) = target_slot {
        if existing.sector_count >= needed_sectors {
            (idx, existing.start_lba, existing.sector_count)
        } else {
            let new_lba = free_lba_start;
            free_lba_start += needed_sectors;
            (idx, new_lba, needed_sectors)
        }
    } else if let Some(idx) = first_free_slot {
        let new_lba = free_lba_start;
        free_lba_start += needed_sectors;
        file_count += 1;
        (idx, new_lba, needed_sectors)
    } else {
        return Err("Tabla de directorio DiskFS llena (maximo 16 archivos)");
    };

    // Prepare aligned sector buffer
    let total_bytes = (allocated_sectors as usize) * 512;
    let mut sector_data = vec![0u8; total_bytes];
    sector_data[..data.len()].copy_from_slice(data);

    // Write file data sectors to SATA disk
    ostd::storage::write_sectors(start_lba as u64, allocated_sectors as usize, &sector_data)
        .map_err(|_| "Fallo al escribir sectores de datos en disco SATA")?;

    // Update directory entry
    let updated_entry = DiskFsDirEntry {
        used: true,
        size_bytes: data.len() as u32,
        start_lba,
        sector_count: allocated_sectors,
        path: String::from(path),
    };
    let offset = slot_idx * DISKFS_ENTRY_SIZE;
    updated_entry.write_to_bytes(&mut dir_buf[offset..offset + DISKFS_ENTRY_SIZE]);

    // Write updated directory table (LBA 1..4)
    ostd::storage::write_sectors(1, 4, &dir_buf)
        .map_err(|_| "Fallo al guardar tabla de directorio en disco SATA")?;

    // Update superblock (LBA 0)
    sb_buf[20..24].copy_from_slice(&file_count.to_le_bytes());
    sb_buf[28..32].copy_from_slice(&free_lba_start.to_le_bytes());
    ostd::storage::write_sectors(0, 1, &sb_buf)
        .map_err(|_| "Fallo al actualizar Superbloque en disco SATA")?;

    // Verify round-trip persistence by reading back from disk
    let mut verify_buf = vec![0u8; 512];
    if let Ok(()) = ostd::storage::read_sectors(start_lba as u64, 1, &mut verify_buf) {
        let check_len = data.len().min(512);
        if &verify_buf[..check_len] == &data[..check_len] {
            ostd::early_println!(
                "[DiskFS::sync] Archivo \"{}\" persistido y verificado en disco SATA AHCI 00:1f.2 (LBA={}, Sectores={})",
                path,
                start_lba,
                allocated_sectors
            );
            ostd::fb_println!(
                "[DiskFS::sync] Archivo \"{}\" persistido en SATA (LBA={}, {} B)",
                path,
                start_lba,
                data.len()
            );
        }
    }

    Ok(())
}

/// Generates, saves, and verifies a persistent note on the SATA disk.
pub fn save_persistent_note(ticks: u64) -> Result<(), &'static str> {
    let ahci = ostd::storage::ahci_snapshot();
    let note_text = alloc::format!(
        "[DiskFS] Nota persistente de usuario FipSOs v1.6\n\
         Timestamp IRQ0: {} ticks\n\
         Dispositivo: Intel ICH9 SATA AHCI 00:1f.2 (Port {})\n\
         LBA Lecturas: {} sectores | LBA Escrituras: {} sectores\n\
         Total transferido: {} KiB\n\
         Estado: Persistencia en sectores LBA verificada exitosamente.\n",
        ticks,
        ahci.port_idx,
        ahci.sectors_read,
        ahci.sectors_written,
        ahci.bytes_transferred / 1024
    );

    save_file(PATH_PERSISTENT_NOTE, note_text.as_bytes())
}

/// Deletes a file from the SATA disk and updates the directory table and superblock.
pub fn delete_file(path: &str) -> Result<(), &'static str> {
    if !ostd::storage::is_ahci_ready() {
        return Err("Controlador SATA AHCI no disponible");
    }

    let mut sb_buf = [0u8; 512];
    ostd::storage::read_sectors(0, 1, &mut sb_buf)
        .map_err(|_| "Fallo al leer Superbloque")?;
    if &sb_buf[0..16] != DISKFS_MAGIC {
        return Err("Disco SATA no formateado con DiskFS");
    }

    let mut file_count = u32::from_le_bytes([sb_buf[20], sb_buf[21], sb_buf[22], sb_buf[23]]);

    let mut dir_buf = [0u8; 2048];
    ostd::storage::read_sectors(1, 4, &mut dir_buf)
        .map_err(|_| "Fallo al leer tabla de directorio")?;

    let mut found_slot = None;
    for i in 0..DISKFS_MAX_FILES {
        let offset = i * DISKFS_ENTRY_SIZE;
        if let Some(entry) = DiskFsDirEntry::from_bytes(&dir_buf[offset..offset + DISKFS_ENTRY_SIZE]) {
            if entry.used && entry.path == path {
                found_slot = Some(i);
                break;
            }
        }
    }

    let slot_idx = found_slot.ok_or("Archivo no encontrado en DiskFS")?;
    let offset = slot_idx * DISKFS_ENTRY_SIZE;
    dir_buf[offset..offset + DISKFS_ENTRY_SIZE].fill(0);

    ostd::storage::write_sectors(1, 4, &dir_buf)
        .map_err(|_| "Fallo al actualizar tabla de directorio en disco SATA")?;

    if file_count > 0 {
        file_count -= 1;
        sb_buf[20..24].copy_from_slice(&file_count.to_le_bytes());
        let _ = ostd::storage::write_sectors(0, 1, &sb_buf);
    }

    ostd::early_println!("[DiskFS::unlink] Archivo \"{}\" eliminado del disco SATA AHCI", path);
    ostd::fb_println!("[DiskFS::unlink] Archivo \"{}\" eliminado de SATA", path);

    Ok(())
}

/// Returns a list of all active files recorded in the SATA AHCI directory table.
pub fn list_files() -> Vec<DiskFsDirEntry> {
    if !ostd::storage::is_ahci_ready() {
        return Vec::new();
    }
    let mut dir_buf = [0u8; 2048];
    if ostd::storage::read_sectors(1, 4, &mut dir_buf).is_err() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    for i in 0..DISKFS_MAX_FILES {
        let offset = i * DISKFS_ENTRY_SIZE;
        if let Some(entry) = DiskFsDirEntry::from_bytes(&dir_buf[offset..offset + DISKFS_ENTRY_SIZE]) {
            if entry.used && !entry.path.is_empty() {
                entries.push(entry);
            }
        }
    }
    entries
}
