#![forbid(unsafe_code)]

//! Codificación y decodificación de nombres cortos 8.3 y Long File Name (LFN UTF-16LE) FAT32.

use alloc::string::String;
use alloc::vec::Vec;

use super::dir_ops::{ATTR_LONG_NAME, Fat32Entry};

/// Calcula el checksum de 8 bits FAT32 sobre una entrada 8.3 de 11 bytes.
pub fn short_name_checksum(raw83: &[u8]) -> u8 {
    let mut sum: u8 = 0;
    for &b in &raw83[..11] {
        sum = sum.rotate_right(1).wrapping_add(b);
    }
    sum
}

/// Decodifica un nombre corto 8.3 de 11 bytes (maneja `0x05 -> 0xE5` y bytes OEM).
pub fn decode_short_name(raw83: &[u8]) -> String {
    let mut first_fixed = [0u8; 8];
    first_fixed.copy_from_slice(&raw83[0..8]);
    if first_fixed[0] == 0x05 {
        first_fixed[0] = 0xE5;
    }

    let base_end = first_fixed.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    let ext_end = raw83[8..11].iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);

    let mut out = String::new();
    for &b in &first_fixed[..base_end] {
        out.push((b as char).to_ascii_lowercase());
    }
    if ext_end > 0 {
        out.push('.');
        for &b in &raw83[8..8 + ext_end] {
            out.push((b as char).to_ascii_lowercase());
        }
    }
    out
}

/// Extrae una entrada LFN de 32 bytes dentro de la tabla ordenada de fragmentos (1..=20).
pub fn push_lfn_chunk(lfn_slots: &mut [(u8, [u16; 13]); 20], entry: &[u8]) -> u8 {
    let order = entry[0] & 0x1F;
    if order == 0 || order as usize > lfn_slots.len() {
        return 0;
    }
    let mut chars = [0xFFFFu16; 13];
    let offsets = [
        (1usize, 5usize, 0usize),
        (14usize, 6usize, 5usize),
        (28usize, 2usize, 11usize),
    ];
    for (start, count, dst) in offsets {
        for i in 0..count {
            chars[dst + i] = u16::from_le_bytes([entry[start + i * 2], entry[start + i * 2 + 1]]);
        }
    }
    lfn_slots[(order - 1) as usize] = (entry[13], chars);
    order
}

/// Reensambla el nombre UTF-16 completo verificando integridad de todos los fragmentos y checksum.
pub fn assemble_lfn(lfn_slots: &[(u8, [u16; 13]); 20], count: usize, checksum: u8) -> Option<String> {
    if count == 0 || count > lfn_slots.len() {
        return None;
    }
    let mut utf16 = Vec::new();
    for slot in lfn_slots.iter().take(count) {
        if slot.0 != checksum {
            return None;
        }
        for &u in &slot.1 {
            if u == 0x0000 || u == 0xFFFF {
                return String::from_utf16(&utf16).ok();
            }
            utf16.push(u);
        }
    }
    String::from_utf16(&utf16).ok()
}

fn sanitize_83_char(b: u8) -> u8 {
    let u = b.to_ascii_uppercase();
    if u.is_ascii_alphanumeric() || b"!#$%&'()-@^_{}~".contains(&u) {
        u
    } else {
        b'_'
    }
}

/// Genera un alias 8.3 único verificando colisiones tanto contra LFN como contra alias cortos existentes.
pub fn build_short_name_83(name: &str, existing: &[Fat32Entry]) -> [u8; 11] {
    let (stem, ext) = match name.rfind('.') {
        Some(pos) if pos > 0 => (&name[..pos], &name[pos + 1..]),
        _ => (name, ""),
    };
    let mut clean_stem = Vec::new();
    for b in stem.bytes().filter(|&b| b != b' ' && b != b'.') {
        clean_stem.push(sanitize_83_char(b));
    }
    if clean_stem.is_empty() {
        clean_stem.push(b'F');
    }

    let mut clean_ext = [b' '; 3];
    for (i, b) in ext.bytes().filter(|&b| b != b' ' && b != b'.').take(3).enumerate() {
        clean_ext[i] = sanitize_83_char(b);
    }

    for suffix_num in 1..=99usize {
        let mut raw = [b' '; 11];
        raw[8..11].copy_from_slice(&clean_ext);
        let tag = alloc::format!("~{}", suffix_num);
        let max_prefix = 8usize.saturating_sub(tag.len());
        let p_len = clean_stem.len().min(max_prefix);
        raw[..p_len].copy_from_slice(&clean_stem[..p_len]);
        raw[p_len..p_len + tag.len()].copy_from_slice(tag.as_bytes());
        let candidate = decode_short_name(&raw);
        let collides = existing.iter().any(|e| {
            e.name.eq_ignore_ascii_case(&candidate) || e.short_name.eq_ignore_ascii_case(&candidate)
        });
        if !collides {
            return raw;
        }
    }
    [b'F', b'I', b'P', b'S', b'~', b'9', b'9', b' ', clean_ext[0], clean_ext[1], clean_ext[2]]
}

/// Construye registros LFN en orden inverso (máximo 255 unidades UTF-16 = 20 entradas LFN).
pub fn build_lfn_records(name: &str, checksum: u8) -> Vec<[u8; 32]> {
    let mut utf16: Vec<u16> = name.encode_utf16().take(255).collect();
    if utf16.is_empty() {
        utf16.push(b'_' as u16);
    }
    let count = utf16.len().div_ceil(13).min(20);
    let mut records = Vec::with_capacity(count);

    for rev in (0..count).rev() {
        let mut rec = [0u8; 32];
        let ord = (rev + 1) as u8;
        rec[0] = if rev + 1 == count { ord | 0x40 } else { ord };
        rec[11] = ATTR_LONG_NAME;
        rec[13] = checksum;

        let base_idx = rev * 13;
        let offsets = [
            (1usize, 5usize, 0usize),
            (14usize, 6usize, 5usize),
            (28usize, 2usize, 11usize),
        ];
        for (start, seg_len, local_off) in offsets {
            for j in 0..seg_len {
                let u_idx = base_idx + local_off + j;
                let code = if u_idx < utf16.len() {
                    utf16[u_idx]
                } else if u_idx == utf16.len() {
                    0x0000
                } else {
                    0xFFFF
                };
                rec[start + j * 2..start + j * 2 + 2].copy_from_slice(&code.to_le_bytes());
            }
        }
        records.push(rec);
    }
    records
}

/// Construye una entrada clásica de directorio 8.3 (32 bytes).
pub fn build_83_record(raw83: &[u8; 11], attr: u8, first_cluster: u32, size: u32) -> [u8; 32] {
    let mut rec = [0u8; 32];
    rec[0..11].copy_from_slice(raw83);
    rec[11] = attr;
    let hi = (((first_cluster & 0x0FFF_FFFF) >> 16) as u16).to_le_bytes();
    let lo = ((first_cluster & 0xFFFF) as u16).to_le_bytes();
    rec[20..22].copy_from_slice(&hi);
    rec[26..28].copy_from_slice(&lo);
    rec[28..32].copy_from_slice(&size.to_le_bytes());
    rec
}
