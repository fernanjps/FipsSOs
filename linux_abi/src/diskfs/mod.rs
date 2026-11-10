#![forbid(unsafe_code)]

//! FipSOs Persistent SATA AHCI File System (`linux_abi::diskfs`).
//!
//! Provides on-disk superblock ("FIPSOS_DISKFS_V2"), 128-entry inode table, 4 KiB sector bitmap,
//! hierarchical directory creation/removal, and zero-copy file persistence on SATA AHCI port 0.

pub mod inode;
pub mod ops;

pub use inode::*;
pub use ops::*;

use alloc::vec;
use alloc::vec::Vec;

/// Initializes `DiskFS v2`: if the SATA disk contains `"FIPSOS_DISKFS_V2"`, loads all persistent
/// files and subdirectories; if virgin or older version, formats the superblock, bitmap, and creates initial entries.
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
        let inode_buf = match read_inode_table() {
            Ok(b) => b,
            Err(e) => {
                ostd::early_println!("[DiskFS] Error al leer tabla de inodos: {}", e);
                return Vec::new();
            }
        };

        let mut entries = Vec::new();
        for i in 0..DISKFS_MAX_INODES {
            let offset = i * DISKFS_INODE_SIZE;
            if let Some(inode) = DiskFsInode::from_bytes(&inode_buf[offset..offset + DISKFS_INODE_SIZE]) {
                if inode.used && !inode.path.is_empty() {
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
                            let data_buf = if inode.sector_count > 0 {
                                let mut sbuf = vec![0u8; (inode.sector_count as usize) * 512];
                                if let Ok(()) = ostd::storage::read_sectors(
                                    inode.start_lba as u64,
                                    inode.sector_count as usize,
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
                        }
                        _ => {}
                    }
                }
            }
        }
        entries
    } else {
        format_and_create_initial()
    }
}

/// Returns a list of all active files recorded in the SATA AHCI inode table.
pub fn list_files() -> Vec<DiskFsInode> {
    if !ostd::storage::is_ahci_ready() {
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
