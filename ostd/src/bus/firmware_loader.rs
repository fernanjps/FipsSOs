//! Real DMA Firmware Blob Loader (`ostd::bus::firmware_loader`).
//!
//! Zero synthetic firmware blobs: callers must supply real binary bytes read from VFS
//! (e.g. `/mnt/fat0/firmware/<name>.ucode`). If no target PCIe wireless device is present,
//! `load_wifi_firmware()` returns `Err(-19)` (`-ENODEV`).

use crate::mm::dma::DmaBuffer;
use crate::sync::SpinLock;

const MAX_FW_SLOTS: usize = 4;
const MAX_FW_NAME: usize = 48;
const MAX_FW_BYTES: usize = 512 * 1024;

struct FirmwareSlot {
    in_use: bool,
    name: [u8; MAX_FW_NAME],
    name_len: usize,
    size_bytes: usize,
    phys_addr: u64,
    dma: Option<DmaBuffer>,
}

impl FirmwareSlot {
    const fn empty() -> Self {
        Self {
            in_use: false,
            name: [0; MAX_FW_NAME],
            name_len: 0,
            size_bytes: 0,
            phys_addr: 0,
            dma: None,
        }
    }
}

static FW_SLOTS: SpinLock<[FirmwareSlot; MAX_FW_SLOTS]> = SpinLock::new([
    FirmwareSlot::empty(),
    FirmwareSlot::empty(),
    FirmwareSlot::empty(),
    FirmwareSlot::empty(),
]);

/// Validates a firmware filename and stages real binary bytes into a contiguous DMA buffer.
/// Returns the physical DMA address on success, or a negative Linux errno (`-EINVAL`, `-ENODEV`, `-ENOMEM`).
pub fn load_wifi_firmware(name: &str, bytes: &[u8]) -> Result<u64, i64> {
    if name.is_empty() || name.len() > MAX_FW_NAME || name.contains("..") || name.contains('/') {
        return Err(-22); // -EINVAL
    }
    if bytes.is_empty() || bytes.len() > MAX_FW_BYTES {
        return Err(-22); // -EINVAL
    }
    if crate::bus::pcie::probe_wifi_controller().is_none() {
        return Err(-19); // -ENODEV: no PCIe wireless hardware detected
    }

    let aligned = bytes.len().next_power_of_two().max(4096);
    let dma = DmaBuffer::allocate(aligned).map_err(|_| -12i64)?; // -ENOMEM
    dma.write_bytes(0, bytes).map_err(|_| -5i64)?;
    let phys = dma.paddr();

    let mut slots = FW_SLOTS.lock();
    let idx = slots
        .iter()
        .position(|s| s.in_use && s.name_len == name.len() && &s.name[..s.name_len] == name.as_bytes())
        .or_else(|| slots.iter().position(|s| !s.in_use))
        .unwrap_or(0);

    let slot = &mut slots[idx];
    slot.in_use = true;
    slot.name = [0; MAX_FW_NAME];
    slot.name[..name.len()].copy_from_slice(name.as_bytes());
    slot.name_len = name.len();
    slot.size_bytes = bytes.len();
    slot.phys_addr = phys;
    slot.dma = Some(dma);

    crate::early_println!(
        "[ostd::fw] Firmware DMA '{}' cargado ({} B -> phys={:#x})",
        name,
        bytes.len(),
        phys
    );
    Ok(phys)
}

/// Returns `(wifi_hw_present, loaded_count, last_phys_addr)`.
pub fn status() -> (bool, usize, u64) {
    let hw = crate::bus::pcie::probe_wifi_controller().is_some();
    let slots = FW_SLOTS.lock();
    let mut count = 0usize;
    let mut last_phys = 0u64;
    for s in slots.iter() {
        if s.in_use {
            count += 1;
            last_phys = s.phys_addr;
        }
    }
    (hw, count, last_phys)
}
