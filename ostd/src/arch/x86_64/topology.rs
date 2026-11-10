use core::arch::x86_64::{__cpuid, __cpuid_count};
use core::fmt;

/// CPU Topology and SIMD capability report detected via hardware `cpuid`.
#[derive(Clone, Copy)]
pub struct CpuTopology {
    vendor_bytes: [u8; 12],
    brand_bytes: [u8; 48],
    brand_len: usize,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub logical_cores: u32,
    pub sse4_2: bool,
    pub avx: bool,
    pub avx2: bool,
    pub avx512f: bool,
}

impl CpuTopology {
    /// Returns the 12-character CPU vendor string (e.g., `"GenuineIntel"` or `"AuthenticAMD"`).
    pub fn vendor(&self) -> &str {
        core::str::from_utf8(&self.vendor_bytes).unwrap_or("Unknown")
    }

    /// Returns the processor brand/model string (e.g., `"QEMU Virtual CPU version 2.5+"`).
    pub fn brand(&self) -> &str {
        if self.brand_len == 0 {
            return "Generic x86_64 Processor";
        }
        core::str::from_utf8(&self.brand_bytes[..self.brand_len])
            .unwrap_or("Generic x86_64 Processor")
            .trim()
    }
}

impl fmt::Debug for CpuTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpuTopology")
            .field("vendor", &self.vendor())
            .field("brand", &self.brand())
            .field("family", &self.family)
            .field("model", &self.model)
            .field("stepping", &self.stepping)
            .field("logical_cores", &self.logical_cores)
            .field("sse4_2", &self.sse4_2)
            .field("avx", &self.avx)
            .field("avx2", &self.avx2)
            .field("avx512f", &self.avx512f)
            .finish()
    }
}

/// Detects CPU vendor, model/brand string, logical core count, and SIMD extensions (`AVX2`/`AVX-512`)
/// using the x86_64 `cpuid` instruction.
pub fn detect() -> CpuTopology {
    // Leaf 0x00: Maximum standard leaf and 12-byte Vendor ID (EBX, EDX, ECX)
    let leaf0 = __cpuid(0);
    let max_basic_leaf = leaf0.eax;
    let mut vendor_bytes = [0u8; 12];
    vendor_bytes[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor_bytes[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor_bytes[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

    let mut family = 0u8;
    let mut model = 0u8;
    let mut stepping = 0u8;
    let mut logical_cores = 1u32;
    let mut sse4_2 = false;
    let mut avx = false;

    if max_basic_leaf >= 1 {
        let leaf1 = __cpuid(1);
        stepping = (leaf1.eax & 0xF) as u8;
        let base_model = ((leaf1.eax >> 4) & 0xF) as u8;
        let base_family = ((leaf1.eax >> 8) & 0xF) as u8;
        let ext_model = ((leaf1.eax >> 16) & 0xF) as u8;
        let ext_family = ((leaf1.eax >> 20) & 0xFF) as u8;

        family = if base_family == 0xF {
            base_family.saturating_add(ext_family)
        } else {
            base_family
        };
        model = if base_family == 0x6 || base_family == 0xF {
            (ext_model << 4) | base_model
        } else {
            base_model
        };

        let reported_logical = (leaf1.ebx >> 16) & 0xFF;
        if reported_logical > 0 {
            logical_cores = reported_logical;
        }

        sse4_2 = (leaf1.ecx & (1 << 20)) != 0;
        avx = (leaf1.ecx & (1 << 28)) != 0;
    }

    // Leaf 0x0B (Extended Topology Enumeration) if available and valid
    if max_basic_leaf >= 0x0B {
        let leaf_b = __cpuid_count(0x0B, 1);
        let core_count = leaf_b.ebx & 0xFFFF;
        if core_count > 0 {
            logical_cores = core_count;
        }
    }

    let mut avx2 = false;
    let mut avx512f = false;
    if max_basic_leaf >= 7 {
        let leaf7 = __cpuid_count(7, 0);
        avx2 = (leaf7.ebx & (1 << 5)) != 0;
        avx512f = (leaf7.ebx & (1 << 16)) != 0;
    }

    // Extended leaves 0x80000002..=0x80000004: 48-byte Processor Brand String
    let ext0 = __cpuid(0x8000_0000);
    let mut brand_bytes = [0u8; 48];
    let mut brand_len = 0usize;

    if ext0.eax >= 0x8000_0004 {
        let leaves = [
            __cpuid(0x8000_0002),
            __cpuid(0x8000_0003),
            __cpuid(0x8000_0004),
        ];
        for (i, l) in leaves.iter().enumerate() {
            let base = i * 16;
            brand_bytes[base..base + 4].copy_from_slice(&l.eax.to_le_bytes());
            brand_bytes[base + 4..base + 8].copy_from_slice(&l.ebx.to_le_bytes());
            brand_bytes[base + 8..base + 12].copy_from_slice(&l.ecx.to_le_bytes());
            brand_bytes[base + 12..base + 16].copy_from_slice(&l.edx.to_le_bytes());
        }
        brand_len = brand_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(brand_bytes.len());
    }

    CpuTopology {
        vendor_bytes,
        brand_bytes,
        brand_len,
        family,
        model,
        stepping,
        logical_cores,
        sse4_2,
        avx,
        avx2,
        avx512f,
    }
}
