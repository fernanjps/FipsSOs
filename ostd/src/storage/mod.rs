pub mod ahci;
pub mod nvme;

pub use ahci::{ahci_snapshot, is_ahci_ready, AhciHardwareSnapshot};
pub use nvme::{is_nvme_ready, nvme_snapshot, NvmeTelemetry};

/// Initializes mass storage controllers (SATA AHCI and PCIe NVMe).
pub fn init() {
    ahci::init();
    nvme::init();
}

/// Returns `true` if at least one block storage backend (SATA AHCI or PCIe NVMe) is ready.
pub fn is_storage_ready() -> bool {
    ahci::is_ahci_ready() || nvme::is_nvme_ready()
}

/// Returns total addressable 512-byte sectors reported by the active disk controller (`0` if unavailable).
pub fn total_sectors() -> u64 {
    if ahci::is_ahci_ready() {
        ahci::total_sectors()
    } else if nvme::is_nvme_ready() {
        nvme::total_sectors()
    } else {
        0
    }
}

/// Returns the active block storage backend identifier used by `DiskFS`.
pub fn active_backend_name() -> &'static str {
    if ahci::is_ahci_ready() {
        "SATA AHCI"
    } else if nvme::is_nvme_ready() {
        "PCIe NVMe"
    } else {
        "NONE"
    }
}

/// Reads `sector_count` 512-byte sectors starting at `lba` from the active storage controller.
pub fn read_sectors(lba: u64, sector_count: usize, dst: &mut [u8]) -> Result<(), &'static str> {
    if ahci::is_ahci_ready() {
        ahci::read_sectors(lba, sector_count, dst)
    } else if nvme::is_nvme_ready() {
        nvme::read_sectors(lba, sector_count, dst)
    } else {
        Err("Controlador de bloques SATA AHCI/NVMe no disponible")
    }
}

/// Writes `sector_count` 512-byte sectors starting at `lba` to the active storage controller.
pub fn write_sectors(lba: u64, sector_count: usize, src: &[u8]) -> Result<(), &'static str> {
    if ahci::is_ahci_ready() {
        ahci::write_sectors(lba, sector_count, src)
    } else if nvme::is_nvme_ready() {
        nvme::write_sectors(lba, sector_count, src)
    } else {
        Err("Controlador de bloques SATA AHCI/NVMe no disponible")
    }
}
