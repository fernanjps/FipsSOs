#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// Size of a standard POSIX USTAR block/header in bytes.
pub const TAR_BLOCK_SIZE: usize = 512;

/// Parsed regular file entry extracted from an in-memory USTAR `.tar` archive (`initramfs.tar`).
#[derive(Debug, Clone)]
pub struct TarFileEntry<'a> {
    pub path: String,
    pub data: &'a [u8],
}

/// Safe USTAR `.tar` parser and in-memory fallback generator for `linux_abi`.
pub struct TarFs;

impl TarFs {
    /// Parses a USTAR `.tar` byte slice and returns all regular files with normalized absolute paths.
    pub fn parse(tar_bytes: &[u8]) -> Vec<TarFileEntry<'_>> {
        let mut entries = Vec::new();
        let mut offset = 0usize;

        while offset + TAR_BLOCK_SIZE <= tar_bytes.len() {
            let header = &tar_bytes[offset..offset + TAR_BLOCK_SIZE];

            // End-of-archive marker: 512-byte block filled with zeros
            if header.iter().all(|&b| b == 0) {
                break;
            }

            let file_size = parse_octal(&header[124..136]);
            let typeflag = header[156];
            let data_start = offset + TAR_BLOCK_SIZE;
            let Some(data_end) = data_start.checked_add(file_size) else {
                break;
            };
            if data_end > tar_bytes.len() {
                break;
            }

            // Regular file in USTAR is '0' (0x30) or NUL (0x00)
            if typeflag == b'0' || typeflag == 0 {
                let raw_name = extract_cstring(&header[0..100]);
                let raw_prefix = extract_cstring(&header[345..500]);
                let norm_path = normalize_tar_path(raw_prefix, raw_name);

                if !norm_path.is_empty() && norm_path != "/" {
                    entries.push(TarFileEntry {
                        path: norm_path,
                        data: &tar_bytes[data_start..data_end],
                    });
                }
            }

            let aligned_data_blocks = (file_size + TAR_BLOCK_SIZE - 1) & !(TAR_BLOCK_SIZE - 1);
            offset = data_start + aligned_data_blocks;
        }

        entries
    }

    /// Generates a standards-compliant USTAR `.tar` archive in memory containing default
    /// FipSOs `initramfs` files if the bootloader did not provide an external module.
    pub fn build_fallback_tar() -> Vec<u8> {
        let mut archive = Vec::new();

        Self::append_ustar_file(
            &mut archive,
            "etc/motd.txt",
            b"[MOTD] Bienvenido a FipSOs v0.7 (Zircon Framekernel + Linux ABI + TarFs Initramfs + NPU Zero-Copy)\n",
        );
        Self::append_ustar_file(
            &mut archive,
            "models/avatar_v1.bin",
            b"FIPSOS_AI_MODEL_AVATAR_V1|ARCH=NPU_INT8_TENSOR|SHARD=ZERO_COPY_DMA|DIM=512x512\n",
        );
        Self::append_ustar_file(
            &mut archive,
            "bin/npu_infer.elf",
            b"\x7FELF_FIPSOS_NPU_INFER_PIPELINE_V1\n",
        );

        // Two 512-byte zero blocks mark the end of a standard TAR archive
        archive.resize(archive.len() + TAR_BLOCK_SIZE * 2, 0u8);
        archive
    }

    fn append_ustar_file(archive: &mut Vec<u8>, name: &str, content: &[u8]) {
        let mut header = [0u8; TAR_BLOCK_SIZE];
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(99);
        header[0..copy_len].copy_from_slice(&name_bytes[..copy_len]);

        // File mode: "0000644\0"
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");

        // File size in 11 octal digits + NUL at [124..136]
        let mut size_val = content.len();
        for i in (0..11).rev() {
            header[124 + i] = b'0' + ((size_val & 0x7) as u8);
            size_val >>= 3;
        }
        header[135] = 0;

        // Typeflag '0' (regular file) and USTAR magic
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");

        // Checksum calculation (treat [148..156] as spaces)
        header[148..156].fill(b' ');
        let checksum: u32 = header.iter().map(|&b| b as u32).sum();
        let mut chk_val = checksum;
        for i in (0..6).rev() {
            header[148 + i] = b'0' + ((chk_val & 0x7) as u8);
            chk_val >>= 3;
        }
        header[154] = 0;
        header[155] = b' ';

        archive.extend_from_slice(&header);
        archive.extend_from_slice(content);

        let rem = content.len() % TAR_BLOCK_SIZE;
        if rem != 0 {
            archive.resize(archive.len() + (TAR_BLOCK_SIZE - rem), 0u8);
        }
    }
}

fn parse_octal(field: &[u8]) -> usize {
    let mut result = 0usize;
    for &b in field {
        if b == 0 || b == b' ' {
            continue;
        }
        if (b'0'..=b'7').contains(&b) {
            result = (result << 3) + ((b - b'0') as usize);
        } else {
            break;
        }
    }
    result
}

fn extract_cstring(field: &[u8]) -> &str {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).unwrap_or("").trim()
}

fn normalize_tar_path(prefix: &str, name: &str) -> String {
    let mut combined = String::new();
    if !prefix.is_empty() {
        combined.push_str(prefix.trim_matches('/'));
        combined.push('/');
    }
    combined.push_str(name);

    let trimmed = combined
        .trim_start_matches("./")
        .trim_start_matches('/');

    let mut normalized = String::with_capacity(trimmed.len() + 1);
    normalized.push('/');
    normalized.push_str(trimmed);
    normalized
}
