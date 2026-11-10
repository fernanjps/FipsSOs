pub mod ahci;
pub mod nvme;

pub use ahci::{
    ahci_snapshot, is_ahci_ready, read_sectors, write_sectors, AhciHardwareSnapshot,
};
pub use nvme::{nvme_snapshot, NvmeTelemetry};

/// Initializes mass storage controllers (SATA AHCI and PCIe NVMe).
pub fn init() {
    ahci::init();
    nvme::init();
}
