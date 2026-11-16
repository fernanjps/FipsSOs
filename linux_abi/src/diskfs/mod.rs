#![forbid(unsafe_code)]

//! FipSOs Persistent SATA AHCI File System (`linux_abi::diskfs`).
//!
//! Provides on-disk superblock ("FIPSOS_DISKFS_V2"), 128-entry inode table, 4 KiB sector bitmap,
//! hierarchical directory creation/removal, and zero-copy file persistence on SATA AHCI port 0.

pub mod inode;
pub mod ops;

pub use inode::*;
pub use ops::*;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

static PROTECTED_MODE: AtomicBool = AtomicBool::new(false);

/// Returns `true` when an external GPT/MBR partition table or non-FipSOs disk header
/// was detected, locking physical disk writes and mounting `/disk` in temporary RAM.
pub fn is_protected_mode() -> bool {
    PROTECTED_MODE.load(Ordering::Acquire)
}

/// Checks LBA 0 (MBR 0x55AA / partition entries) and LBA 1 (`EFI PART` GPT header).
pub(crate) fn is_external_gpt_or_mbr(lba0: &[u8; 512], lba1: &[u8; 512]) -> bool {
    if &lba1[0..8] == b"EFI PART" {
        return true;
    }
    if lba0[510] == 0x55 && lba0[511] == 0xAA {
        return true;
    }
    for i in 0..4usize {
        let entry = &lba0[446 + i * 16..446 + (i + 1) * 16];
        if entry[4] != 0 || entry[8..16].iter().any(|&b| b != 0) {
            return lba0[510] == 0x55 && lba0[511] == 0xAA;
        }
    }
    false
}

fn protected_ram_entries() -> Vec<DiskFsFile> {
    let info = b"[DiskFS] MODO PROTEGIDO ACTIVO (RAM temporal): Tabla GPT/MBR o datos externos detectados en disco fisico. Ningun sector sera sobrescrito.\n";
    vec![
        DiskFsFile {
            path: String::from("/disk"),
            is_dir: true,
            data: Vec::new(),
        },
        DiskFsFile {
            path: String::from(PATH_BOOT_LOG),
            is_dir: false,
            data: Vec::from(&info[..]),
        },
    ]
}

/// Initializes `DiskFS v2`: mounts existing `"FIPSOS_DISKFS_V2"` disks, protects any
/// external GPT/MBR or non-empty disks in RAM-only mode, and only formats completely zeroed disks.
pub fn init_or_mount() -> Vec<DiskFsFile> {
    if !ostd::storage::is_storage_ready() {
        ostd::early_println!("[DiskFS] Controlador SATA AHCI/NVMe no disponible, omitiendo montaje de disco.");
        ostd::fb_println!("[DiskFS] Controlador AHCI/NVMe no disponible.");
        return Vec::new();
    }

    let mut sb_buf = [0u8; 512];
    if let Err(err) = ostd::storage::read_sectors(0, 1, &mut sb_buf) {
        ostd::early_println!("[DiskFS] Error al leer Superbloque (LBA 0): {}", err);
        return Vec::new();
    }

    if &sb_buf[0..16] == DISKFS_MAGIC {
        PROTECTED_MODE.store(false, Ordering::Release);
        let inode_buf = match read_inode_table() {
            Ok(b) => b,
            Err(e) => {
                ostd::early_println!("[DiskFS] Error al leer tabla de inodos: {}", e);
                return Vec::new();
            }
        };

        let mut entries = Vec::new();
        let mut file_count = 0usize;
        for i in 0..DISKFS_MAX_INODES {
            let offset = i * DISKFS_INODE_SIZE;
            if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
                if !inode.used {
                    continue;
                }
                if inode.path.is_empty() || !inode.path.is_ascii() || !inode.path.starts_with('/') {
                    ostd::early_println!("[DiskFS] Inodo {} con nombre invalido, omitido.", i);
                    continue;
                }
                match inode.file_type {
                    FileType::Directory => {
                        ostd::early_println!(
                            "[DiskFS] Directorio \"{}\" montado de disco SATA (Inodo={})",
                            inode.path,
                            i
                        );
                        entries.push(DiskFsFile {
                            path: inode.path,
                            is_dir: true,
                            data: Vec::new(),
                        });
                    }
                    FileType::RegularFile => {
                        const MAX_BOOT_DISKFS_FILE_BYTES: u32 = 64 * 1024;
                        if inode.size_bytes > MAX_BOOT_DISKFS_FILE_BYTES
                            || inode.start_lba == 0
                            || inode.sector_count > 128
                        {
                            ostd::early_println!(
                                "[DiskFS] Inodo {} (\"{}\") fuera de rango (LBA={}, size={}, sect={}), omitido.",
                                i,
                                inode.path,
                                inode.start_lba,
                                inode.size_bytes,
                                inode.sector_count
                            );
                            continue;
                        }
                        let data_buf = if inode.sector_count > 0 && inode.size_bytes > 0 {
                            let sectors_to_read = (((inode.size_bytes as usize) + 511) / 512)
                                .min(inode.sector_count as usize);
                            let mut sbuf = vec![0u8; sectors_to_read * 512];
                            if let Ok(()) = ostd::storage::read_sectors(
                                inode.start_lba as u64,
                                sectors_to_read,
                                &mut sbuf,
                            ) {
                                sbuf.truncate(inode.size_bytes as usize);
                                sbuf
                            } else {
                                Vec::new()
                            }
                        } else {
                            Vec::new()
                        };

                        ostd::early_println!(
                            "[DiskFS] Archivo \"{}\" cargado de disco SATA (Inodo={}, LBA={}, {} B)",
                            inode.path,
                            i,
                            inode.start_lba,
                            inode.size_bytes
                        );
                        ostd::fb_println!(
                            "[DiskFS] Archivo \"{}\" cargado de disco SATA (LBA={}, {} B)",
                            inode.path,
                            inode.start_lba,
                            inode.size_bytes
                        );
                        entries.push(DiskFsFile {
                            path: inode.path,
                            is_dir: false,
                            data: data_buf,
                        });
                        file_count += 1;
                    }
                    _ => {}
                }
            }
        }
        ostd::early_println!("[DiskFS] Montaje completado ({} archivos)", file_count);
        ostd::fb_println!("[DiskFS] Montaje completado ({} archivos)", file_count);
        ostd::fb_console::flush_console();
        entries
    } else {
        let mut lba1_buf = [0u8; 512];
        let _ = ostd::storage::read_sectors(1, 1, &mut lba1_buf);

        if is_external_gpt_or_mbr(&sb_buf, &lba1_buf) {
            PROTECTED_MODE.store(true, Ordering::Release);
            ostd::early_println!(
                "[DiskFS] Tabla GPT/MBR externa detectada - Modo Protegido (Sin formateo)"
            );
            ostd::fb_println!(
                "[DiskFS] Tabla GPT/MBR externa detectada - Modo Protegido (Sin formateo)"
            );
            return protected_ram_entries();
        }

        if sb_buf.iter().any(|&b| b != 0) {
            PROTECTED_MODE.store(true, Ordering::Release);
            ostd::early_println!(
                "[DiskFS] Datos externos en LBA 0 (no ceros) - Modo Protegido (Sin formateo)"
            );
            ostd::fb_println!(
                "[DiskFS] Datos externos detectados - Modo Protegido (Sin formateo)"
            );
            return protected_ram_entries();
        }

        PROTECTED_MODE.store(false, Ordering::Release);
        format_and_create_initial()
    }
}

/// Returns a list of all active files recorded in the disk inode table (empty in protected mode).
pub fn list_files() -> Vec<DiskFsInode> {
    if !ostd::storage::is_storage_ready() || is_protected_mode() {
        return Vec::new();
    }
    let inode_buf = match read_inode_table() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let mut entries = Vec::new();
    for i in 0..DISKFS_MAX_INODES {
        let offset = i * DISKFS_INODE_SIZE;
        if let Some(entry) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
            if entry.used && !entry.path.is_empty() {
                entries.push(entry);
            }
        }
    }
    entries
}
