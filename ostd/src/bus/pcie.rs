use alloc::vec::Vec;
use core::fmt;

/// PCI Configuration Space I/O Ports (Mechanism #1).
const PCI_CONFIG_ADDRESS: u16 = 0x0CF8;
const PCI_CONFIG_DATA: u16 = 0x0CFC;

/// Canonical PCI Base Class Codes of primary interest to FipSOs.
pub const PCI_CLASS_DISPLAY_CONTROLLER: u8 = 0x03;
pub const PCI_CLASS_PROCESSING_ACCELERATOR: u8 = 0x12;

/// High-level hardware classification for discovered PCI/PCIe devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciDeviceClass {
    /// Class `0x03`: Display Controller / GPU (VGA, 3D Controller, DRM target).
    GpuDisplayController,
    /// Class `0x12`: Processing Accelerator / NPU (`/dev/accel` target).
    NpuProcessingAccelerator,
    /// Other standard PCI/PCIe peripheral or bridge.
    Other(u8),
}

impl fmt::Display for PciDeviceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GpuDisplayController => f.write_str("GPU / Display Controller (Class 0x03)"),
            Self::NpuProcessingAccelerator => {
                f.write_str("NPU / Processing Accelerator (Class 0x12)")
            }
            Self::Other(code) => write!(f, "{} (Class {:#04x})", class_code_name(*code), code),
        }
    }
}

/// Metadata for a discovered PCI/PCIe function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision_id: u8,
    pub bar0: u32,
}

impl PciDevice {
    /// Returns the high-level classification of this PCI/PCIe device.
    pub const fn classification(&self) -> PciDeviceClass {
        match self.class_code {
            PCI_CLASS_DISPLAY_CONTROLLER => PciDeviceClass::GpuDisplayController,
            PCI_CLASS_PROCESSING_ACCELERATOR => PciDeviceClass::NpuProcessingAccelerator,
            other => PciDeviceClass::Other(other),
        }
    }

    /// Returns `true` if this device is a GPU (`0x03`) or NPU (`0x12`) accelerator.
    #[inline]
    pub const fn is_accelerator_or_gpu(&self) -> bool {
        self.class_code == PCI_CLASS_DISPLAY_CONTROLLER
            || self.class_code == PCI_CLASS_PROCESSING_ACCELERATOR
    }
}

/// Scans the PCI/PCIe hierarchy, logging discovered devices and automatically highlighting
/// Class `0x03` (GPU / Display Controller) and Class `0x12` (NPU / Processing Accelerators).
pub fn scan_bus() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for bus in 0u8..=255 {
        for device in 0u8..32 {
            let vendor_f0 = pci_read_u16(bus, device, 0, 0x00);
            if vendor_f0 == 0xFFFF {
                continue;
            }

            let header_type = pci_read_u8(bus, device, 0, 0x0E);
            let max_functions = if (header_type & 0x80) != 0 { 8 } else { 1 };

            for function in 0u8..max_functions {
                let vendor_id = pci_read_u16(bus, device, function, 0x00);
                if vendor_id == 0xFFFF {
                    continue;
                }

                let device_id = pci_read_u16(bus, device, function, 0x02);
                let class_rev = pci_read_u32(bus, device, function, 0x08);
                let revision_id = (class_rev & 0xFF) as u8;
                let prog_if = ((class_rev >> 8) & 0xFF) as u8;
                let subclass = ((class_rev >> 16) & 0xFF) as u8;
                let class_code = ((class_rev >> 24) & 0xFF) as u8;
                let bar0 = pci_read_u32(bus, device, function, 0x10);

                let dev = PciDevice {
                    bus,
                    device,
                    function,
                    vendor_id,
                    device_id,
                    class_code,
                    subclass,
                    prog_if,
                    revision_id,
                    bar0,
                };

                match dev.classification() {
                    PciDeviceClass::GpuDisplayController => {
                        crate::early_println!(
                            "[ostd::bus::pcie] >>> [GPU DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (DRM/Display) BAR0={:#010x}",
                            dev.bus,
                            dev.device,
                            dev.function,
                            dev.vendor_id,
                            dev.device_id,
                            dev.class_code,
                            dev.subclass,
                            dev.bar0
                        );
                    }
                    PciDeviceClass::NpuProcessingAccelerator => {
                        crate::early_println!(
                            "[ostd::bus::pcie] >>> [NPU DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (/dev/accel) BAR0={:#010x}",
                            dev.bus,
                            dev.device,
                            dev.function,
                            dev.vendor_id,
                            dev.device_id,
                            dev.class_code,
                            dev.subclass,
                            dev.bar0
                        );
                    }
                    PciDeviceClass::Other(_) => {
                        crate::early_println!(
                            "[ostd::bus::pcie]     [PCI Device]    {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} ({})",
                            dev.bus,
                            dev.device,
                            dev.function,
                            dev.vendor_id,
                            dev.device_id,
                            dev.class_code,
                            dev.subclass,
                            class_code_name(dev.class_code)
                        );
                    }
                }

                devices.push(dev);
            }
        }
    }

    devices
}

const fn class_code_name(class_code: u8) -> &'static str {
    match class_code {
        0x00 => "Unclassified",
        0x01 => "Mass Storage Controller",
        0x02 => "Network Controller",
        0x03 => "Display Controller (GPU)",
        0x04 => "Multimedia Controller",
        0x05 => "Memory Controller",
        0x06 => "Bridge Device",
        0x07 => "Simple Communication Controller",
        0x08 => "Base System Peripheral",
        0x09 => "Input Device Controller",
        0x0C => "Serial Bus Controller",
        0x12 => "Processing Accelerator (NPU)",
        _ => "Generic PCI Device",
    }
}

#[inline]
fn pci_read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    #[cfg(target_arch = "x86_64")]
    // SAFETY: Accessing standard x86_64 PCI Configuration Mechanism #1 ports (0xCF8/0xCFC) in Ring 0.
    unsafe {
        outl(PCI_CONFIG_ADDRESS, address);
        inl(PCI_CONFIG_DATA)
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (PCI_CONFIG_ADDRESS, PCI_CONFIG_DATA, address);
        0xFFFF_FFFF
    }
}

#[inline]
fn pci_read_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_u32(bus, device, function, offset);
    let shift = ((offset & 2) * 8) as u32;
    ((dword >> shift) & 0xFFFF) as u16
}

#[inline]
fn pci_read_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let dword = pci_read_u32(bus, device, function, offset);
    let shift = ((offset & 3) * 8) as u32;
    ((dword >> shift) & 0xFF) as u8
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn outl(port: u16, value: u32) {
    // SAFETY: Caller guarantees valid 32-bit I/O port.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: Caller guarantees valid 32-bit I/O port.
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            out("eax") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}
