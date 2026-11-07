#![forbid(unsafe_code)]

use alloc::vec::Vec;
use ostd::mm::{map_user_segment, protect_user_segment, write_user_bytes, VirtAddr};

/// Standard ELF64 constants (`x86_64` System V ABI).
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 0x3E;
pub const PT_LOAD: u32 = 1;

pub const PF_X: u32 = 1 << 0;
pub const PF_W: u32 = 1 << 1;
pub const PF_R: u32 = 1 << 2;

const ELF64_EHDR_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;

/// Errors returned when parsing or loading an ELF64 executable image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    BufferTooSmall,
    InvalidMagic,
    UnsupportedClassOrEndian,
    UnsupportedType(u16),
    UnsupportedMachine(u16),
    InvalidProgramHeaderTable,
    SegmentOutOfBounds,
    NoLoadableSegments,
    MappingFailed(&'static str),
}

/// Metadata describing an ELF64 binary loaded into Lower-Half User Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedElf64 {
    pub entry_point: VirtAddr,
    pub elf_type: u16,
    pub segments_loaded: usize,
    pub total_mapped_bytes: usize,
}

/// Safe ELF64 (`ET_EXEC` / `ET_DYN` `x86_64`) parser and loader for `linux_abi`.
pub struct Elf64Loader;

impl Elf64Loader {
    /// Parses an ELF64 binary image from `elf_bytes` and maps all `PT_LOAD` segments
    /// into Lower-Half User Space via `ostd::mm`.
    pub fn load(elf_bytes: &[u8], pie_base_offset: VirtAddr) -> Result<LoadedElf64, ElfError> {
        if elf_bytes.len() < ELF64_EHDR_SIZE {
            return Err(ElfError::BufferTooSmall);
        }

        if elf_bytes[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }

        let ei_class = elf_bytes[4];
        let ei_data = elf_bytes[5];
        if ei_class != ELFCLASS64 || ei_data != ELFDATA2LSB {
            return Err(ElfError::UnsupportedClassOrEndian);
        }

        let e_type = read_u16_le(elf_bytes, 16)?;
        if e_type != ET_EXEC && e_type != ET_DYN {
            return Err(ElfError::UnsupportedType(e_type));
        }

        let e_machine = read_u16_le(elf_bytes, 18)?;
        if e_machine != EM_X86_64 {
            return Err(ElfError::UnsupportedMachine(e_machine));
        }

        let raw_entry = read_u64_le(elf_bytes, 24)?;
        let e_phoff = read_u64_le(elf_bytes, 32)? as usize;
        let e_phentsize = read_u16_le(elf_bytes, 54)? as usize;
        let e_phnum = read_u16_le(elf_bytes, 56)? as usize;

        if e_phentsize < ELF64_PHDR_SIZE {
            return Err(ElfError::InvalidProgramHeaderTable);
        }

        let ph_table_end = e_phoff
            .checked_add(e_phentsize.saturating_mul(e_phnum))
            .ok_or(ElfError::InvalidProgramHeaderTable)?;
        if ph_table_end > elf_bytes.len() {
            return Err(ElfError::InvalidProgramHeaderTable);
        }

        let load_bias = if e_type == ET_DYN { pie_base_offset } else { 0 };
        let entry_point = raw_entry + load_bias;

        let mut segments_loaded = 0usize;
        let mut total_mapped_bytes = 0usize;

        for i in 0..e_phnum {
            let ph_base = e_phoff + i * e_phentsize;
            let p_type = read_u32_le(elf_bytes, ph_base)?;
            if p_type != PT_LOAD {
                continue;
            }

            let p_flags = read_u32_le(elf_bytes, ph_base + 4)?;
            let p_offset = read_u64_le(elf_bytes, ph_base + 8)? as usize;
            let p_vaddr = read_u64_le(elf_bytes, ph_base + 16)? + load_bias;
            let p_filesz = read_u64_le(elf_bytes, ph_base + 32)? as usize;
            let p_memsz = read_u64_le(elf_bytes, ph_base + 40)? as usize;

            if p_memsz == 0 {
                continue;
            }
            if p_filesz > p_memsz {
                return Err(ElfError::SegmentOutOfBounds);
            }

            let file_end = p_offset
                .checked_add(p_filesz)
                .ok_or(ElfError::SegmentOutOfBounds)?;
            if file_end > elf_bytes.len() {
                return Err(ElfError::SegmentOutOfBounds);
            }

            let writable = (p_flags & PF_W) != 0;
            let executable = (p_flags & PF_X) != 0;
            if writable && executable {
                return Err(ElfError::MappingFailed(
                    "Violacion W^X en segmento PT_LOAD (PF_W | PF_X prohibido)",
                ));
            }

            // Map temporarily as RW+NX while copying ELF segment bytes into user memory
            map_user_segment(p_vaddr, p_memsz, true, false)
                .map_err(ElfError::MappingFailed)?;

            if p_filesz > 0 {
                write_user_bytes(p_vaddr, &elf_bytes[p_offset..file_end])
                    .map_err(ElfError::MappingFailed)?;
            }

            // Apply final strict W^X permissions (clears PTE_WRITABLE if !writable, sets PTE_NX_BIT if !executable)
            protect_user_segment(p_vaddr, p_memsz, writable, executable)
                .map_err(ElfError::MappingFailed)?;

            segments_loaded += 1;
            total_mapped_bytes += p_memsz;
        }

        if segments_loaded == 0 {
            return Err(ElfError::NoLoadableSegments);
        }

        ostd::early_println!(
            "[linux_abi::elf] Binario ELF64 x86_64 cargado con W^X ({} segmento(s) PT_LOAD, {} bytes) -> EntryPoint={:#x}",
            segments_loaded,
            total_mapped_bytes,
            entry_point
        );
        ostd::fb_println!(
            "[linux_abi::elf] Binario ELF64 x86_64 cargado con W^X ({} segmento(s) PT_LOAD, {} bytes) -> EntryPoint={:#x}",
            segments_loaded,
            total_mapped_bytes,
            entry_point
        );

        Ok(LoadedElf64 {
            entry_point,
            elf_type: e_type,
            segments_loaded,
            total_mapped_bytes,
        })
    }

    /// Builds a valid, standards-compliant 64-bit `x86_64` `ET_EXEC` ELF binary image in memory
    /// containing a single `PT_LOAD` segment at `segment_vaddr` with `code_at_0` and `rodata_at_0x100`.
    pub fn build_single_segment_elf64(
        segment_vaddr: VirtAddr,
        code_bytes: &[u8],
        rodata_offset: usize,
        rodata_bytes: &[u8],
    ) -> Vec<u8> {
        let segment_file_offset: usize = 0x100; // Place PT_LOAD payload at file offset 256
        let payload_size = rodata_offset + rodata_bytes.len();
        let total_file_size = segment_file_offset + payload_size;

        let mut img = Vec::with_capacity(total_file_size);
        img.resize(total_file_size, 0u8);

        // 1. ELF64 Header (64 bytes)
        img[0..4].copy_from_slice(&ELF_MAGIC);
        img[4] = ELFCLASS64; // 64-bit
        img[5] = ELFDATA2LSB; // Little-endian
        img[6] = 1; // EV_CURRENT
        img[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        img[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        img[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
        img[24..32].copy_from_slice(&segment_vaddr.to_le_bytes()); // e_entry
        img[32..40].copy_from_slice(&(ELF64_EHDR_SIZE as u64).to_le_bytes()); // e_phoff = 64
        img[52..54].copy_from_slice(&(ELF64_EHDR_SIZE as u16).to_le_bytes()); // e_ehsize = 64
        img[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes()); // e_phentsize = 56
        img[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum = 1

        // 2. Program Header #0: PT_LOAD (56 bytes at offset 64) with strict W^X (PF_R | PF_X)
        let ph = ELF64_EHDR_SIZE;
        let flags = PF_R | PF_X;
        img[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        img[ph + 4..ph + 8].copy_from_slice(&flags.to_le_bytes());
        img[ph + 8..ph + 16].copy_from_slice(&(segment_file_offset as u64).to_le_bytes()); // p_offset
        img[ph + 16..ph + 24].copy_from_slice(&segment_vaddr.to_le_bytes()); // p_vaddr
        img[ph + 24..ph + 32].copy_from_slice(&segment_vaddr.to_le_bytes()); // p_paddr
        img[ph + 32..ph + 40].copy_from_slice(&(payload_size as u64).to_le_bytes()); // p_filesz
        img[ph + 40..ph + 48].copy_from_slice(&4096u64.to_le_bytes()); // p_memsz (1 page)
        img[ph + 48..ph + 56].copy_from_slice(&4096u64.to_le_bytes()); // p_align

        // 3. Copy code and rodata into the PT_LOAD payload
        img[segment_file_offset..segment_file_offset + code_bytes.len()]
            .copy_from_slice(code_bytes);
        let ro_start = segment_file_offset + rodata_offset;
        img[ro_start..ro_start + rodata_bytes.len()].copy_from_slice(rodata_bytes);

        img
    }
}

#[inline]
fn read_u16_le(buf: &[u8], offset: usize) -> Result<u16, ElfError> {
    let slice = buf.get(offset..offset + 2).ok_or(ElfError::BufferTooSmall)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

#[inline]
fn read_u32_le(buf: &[u8], offset: usize) -> Result<u32, ElfError> {
    let slice = buf.get(offset..offset + 4).ok_or(ElfError::BufferTooSmall)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[inline]
fn read_u64_le(buf: &[u8], offset: usize) -> Result<u64, ElfError> {
    let slice = buf.get(offset..offset + 8).ok_or(ElfError::BufferTooSmall)?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}
