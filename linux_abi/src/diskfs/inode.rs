#![forbid(unsafe_code)]

//! DiskFS Inode Definitions, Sector Constants, and Bitmap Primitives (`linux_abi::diskfs::inode`).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const DISKFS_MAGIC: &[u8; 16] = b"FIPSOS_DISKFS_V2";
pub const DISKFS_TOTAL_SECTORS: u32 = 32768; // 16 MiB disk = 32,768 sectors of 512 bytes
pub const DISKFS_INODE_START_LBA: u32 = 1;
pub const DISKFS_INODE_SECTORS: usize = 32; // 32 sectors * 512 = 16,384 bytes
pub const DISKFS_MAX_INODES: usize = 128; // 128 inodes * 128 bytes = 16,384 bytes
pub const DISKFS_INODE_SIZE: usize = 128; // 128 bytes per inode entry

pub const DISKFS_BITMAP_START_LBA: u32 = 33;
pub const DISKFS_BITMAP_SECTORS: usize = 8; // 8 sectors * 512 = 4,096 bytes = 32,768 bits
pub const DISKFS_BITMAP_BYTES: usize = 4096;

pub const DISKFS_DATA_START_LBA: u32 = 64; // Data sectors begin at LBA 64

// Aliases for compatibility
pub const DISKFS_MAX_FILES: usize = DISKFS_MAX_INODES;
pub const DISKFS_ENTRY_SIZE: usize = DISKFS_INODE_SIZE;

pub const PATH_BOOT_LOG: &str = "/disk/boot_log.txt";
pub const PATH_PERSISTENT_NOTE: &str = "/disk/persistent_note.txt";

/// Inode types stored on the SATA AHCI disk.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Unused = 0,
    RegularFile = 1,
    Directory = 2,
}

/// Represents an on-disk inode (128 bytes) in LBA 1..32.
#[derive(Debug, Clone)]
pub struct DiskFsInode {
    pub used: bool,
    pub file_type: FileType,
    pub parent_inode: u32,
    pub size_bytes: u32,
    pub start_lba: u32,
    pub sector_count: u32,
    pub created_time: u32,
    pub path: String,
}

pub type DiskFsDirEntry = DiskFsInode;

impl DiskFsInode {
    pub fn from_bytes(slice: &[u8]) -> Option<Self> {
        if slice.len() < DISKFS_INODE_SIZE {
            return None;
        }
        let used = slice[0] == 1;
        let file_type = match slice[1] {
            1 => FileType::RegularFile,
            2 => FileType::Directory,
            _ => FileType::Unused,
        };
        let parent_inode = u32::from_le_bytes([slice[4], slice[5], slice[6], slice[7]]);
        let size_bytes = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
        let start_lba = u32::from_le_bytes([slice[12], slice[13], slice[14], slice[15]]);
        let sector_count = u32::from_le_bytes([slice[16], slice[17], slice[18], slice[19]]);
        let created_time = u32::from_le_bytes([slice[20], slice[21], slice[22], slice[23]]);

        let path_bytes = &slice[24..128];
        let path_len = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
        let path = core::str::from_utf8(&path_bytes[..path_len])
            .unwrap_or("")
            .into();

        Some(Self {
            used,
            file_type,
            parent_inode,
            size_bytes,
            start_lba,
            sector_count,
            created_time,
            path,
        })
    }

    pub fn write_to_bytes(&self, slice: &mut [u8]) {
        slice[..DISKFS_INODE_SIZE].fill(0);
        slice[0] = if self.used { 1 } else { 0 };
        slice[1] = match self.file_type {
            FileType::Unused => 0,
            FileType::RegularFile => 1,
            FileType::Directory => 2,
        };
        slice[4..8].copy_from_slice(&self.parent_inode.to_le_bytes());
        slice[8..12].copy_from_slice(&self.size_bytes.to_le_bytes());
        slice[12..16].copy_from_slice(&self.start_lba.to_le_bytes());
        slice[16..20].copy_from_slice(&self.sector_count.to_le_bytes());
        slice[20..24].copy_from_slice(&self.created_time.to_le_bytes());

        let path_bytes = self.path.as_bytes();
        let max_copy = path_bytes.len().min(103);
        slice[24..24 + max_copy].copy_from_slice(&path_bytes[..max_copy]);
        slice[24 + max_copy] = 0;
    }
}

/// Represents a persistent file or directory loaded from or saved to the SATA disk.
#[derive(Debug, Clone)]
pub struct DiskFsFile {
    pub path: String,
    pub is_dir: bool,
    pub data: Vec<u8>,
}

pub(crate) fn path_parent(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return "/";
    }
    match p.rfind('/') {
        Some(0) => "/",
        Some(idx) => &p[..idx],
        None => "/",
    }
}

pub(crate) fn normalize_disk_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/disk" || trimmed == "/disk/" {
        return String::from("/disk");
    }
    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.starts_with("/disk/") {
        String::from(without_trailing)
    } else if without_trailing.starts_with('/') {
        alloc::format!("/disk{}", without_trailing)
    } else {
        alloc::format!("/disk/{}", without_trailing)
    }
}

pub(crate) fn read_bitmap() -> Result<Vec<u8>, &'static str> {
    let mut buf = vec![0u8; DISKFS_BITMAP_SECTORS * 512];
    ostd::storage::read_sectors(DISKFS_BITMAP_START_LBA as u64, DISKFS_BITMAP_SECTORS, &mut buf)
        .map_err(|_| "Fallo al leer bitmap de sectores")?;
    Ok(buf)
}

pub(crate) fn write_bitmap(buf: &[u8]) -> Result<(), &'static str> {
    ostd::storage::write_sectors(DISKFS_BITMAP_START_LBA as u64, DISKFS_BITMAP_SECTORS, buf)
        .map_err(|_| "Fallo al escribir bitmap de sectores")
}

pub(crate) fn bitmap_is_allocated(bitmap: &[u8], lba: u32) -> bool {
    let byte_idx = (lba / 8) as usize;
    let bit_idx = (lba % 8) as usize;
    if byte_idx < bitmap.len() {
        (bitmap[byte_idx] & (1 << bit_idx)) != 0
    } else {
        true
    }
}

pub(crate) fn bitmap_set_range(bitmap: &mut [u8], start_lba: u32, count: u32, allocated: bool) {
    for lba in start_lba..(start_lba.saturating_add(count)) {
        let byte_idx = (lba / 8) as usize;
        let bit_idx = (lba % 8) as usize;
        if byte_idx < bitmap.len() {
            if allocated {
                bitmap[byte_idx] |= 1 << bit_idx;
            } else {
                bitmap[byte_idx] &= !(1 << bit_idx);
            }
        }
    }
}

pub(crate) fn bitmap_allocate_sectors(bitmap: &mut [u8], count: u32) -> Option<u32> {
    if count == 0 {
        return Some(0);
    }
    let mut current_start = None;
    let mut consecutive = 0;
    for lba in DISKFS_DATA_START_LBA..DISKFS_TOTAL_SECTORS {
        if !bitmap_is_allocated(bitmap, lba) {
            if current_start.is_none() {
                current_start = Some(lba);
            }
            consecutive += 1;
            if consecutive == count {
                let start = current_start.unwrap();
                bitmap_set_range(bitmap, start, count, true);
                return Some(start);
            }
        } else {
            current_start = None;
            consecutive = 0;
        }
    }
    None
}

pub(crate) fn bitmap_free_sectors(bitmap: &mut [u8], start_lba: u32, count: u32) {
    if start_lba >= DISKFS_DATA_START_LBA && count > 0 {
        bitmap_set_range(bitmap, start_lba, count, false);
    }
}

pub(crate) fn read_inode_table() -> Result<Vec<u8>, &'static str> {
    let mut buf = vec![0u8; DISKFS_INODE_SECTORS * 512];
    ostd::storage::read_sectors(DISKFS_INODE_START_LBA as u64, DISKFS_INODE_SECTORS, &mut buf)
        .map_err(|_| "Fallo al leer tabla de inodos")?;
    Ok(buf)
}

pub(crate) fn write_inode_table(buf: &[u8]) -> Result<(), &'static str> {
    ostd::storage::write_sectors(DISKFS_INODE_START_LBA as u64, DISKFS_INODE_SECTORS, buf)
        .map_err(|_| "Fallo al escribir tabla de inodos")
}
