use alloc::vec::Vec;
use core::fmt;

/// PCI Configuration Space I/O Ports (Mechanism #1).
const PCI_CONFIG_ADDRESS: u16 = 0x0CF8;
const PCI_CONFIG_DATA: u16 = 0x0CFC;

/// Canonical PCI Base Class Codes of primary interest to FipSOs.
pub const PCI_CLASS_MASS_STORAGE: u8 = 0x01;
pub const PCI_CLASS_NETWORK_CONTROLLER: u8 = 0x02;
pub const PCI_CLASS_DISPLAY_CONTROLLER: u8 = 0x03;
pub const PCI_CLASS_SERIAL_BUS: u8 = 0x0C;
pub const PCI_CLASS_PROCESSING_ACCELERATOR: u8 = 0x12;

/// Storage Subclasses and Programming Interfaces.
pub const PCI_SUBCLASS_SATA: u8 = 0x06;
pub const PCI_SUBCLASS_NVME: u8 = 0x08;
pub const PCI_PROGIF_NVME: u8 = 0x02;

/// USB Subclasses and Programming Interfaces.
pub const PCI_SUBCLASS_USB: u8 = 0x03;
pub const PCI_PROGIF_UHCI: u8 = 0x00;
pub const PCI_PROGIF_OHCI: u8 = 0x10;
pub const PCI_PROGIF_EHCI: u8 = 0x20;
pub const PCI_PROGIF_XHCI: u8 = 0x30;

/// High-level hardware classification for discovered PCI/PCIe devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciDeviceClass {
    /// Class `0x01`: Mass Storage Controller / SATA AHCI (`00:1f.2` target).
    MassStorageController,
    /// Class `0x01:0x08:0x02`: NVM Express (NVMe) PCIe Storage Controller.
    NvmeController,
    /// Class `0x02`: Network Controller / Ethernet NIC (`/dev/net/eth0` target).
    NetworkController,
    /// Class `0x03`: Display Controller / GPU (VGA, 3D Controller, DRM target).
    GpuDisplayController,
    /// Class `0x0C`: USB Host Controller (UHCI/OHCI/EHCI/xHCI).
    UsbController { prog_if: u8 },
    /// Class `0x12`: Processing Accelerator / NPU (`/dev/accel` target).
    NpuProcessingAccelerator,
    /// Other standard PCI/PCIe peripheral or bridge.
    Other(u8),
}

impl fmt::Display for PciDeviceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MassStorageController => f.write_str("Mass Storage / SATA AHCI Controller (Class 0x01)"),
            Self::NvmeController => f.write_str("PCIe NVMe Storage Controller (Class 0x01:0x08:0x02)"),
            Self::NetworkController => f.write_str("NIC / Network Controller (Class 0x02)"),
            Self::GpuDisplayController => f.write_str("GPU / Display Controller (Class 0x03)"),
            Self::UsbController { prog_if } => match *prog_if {
                PCI_PROGIF_XHCI => f.write_str("USB 3.0 xHCI Controller (Class 0x0C:0x03:0x30)"),
                PCI_PROGIF_EHCI => f.write_str("USB 2.0 EHCI Controller (Class 0x0C:0x03:0x20)"),
                PCI_PROGIF_OHCI => f.write_str("USB 1.1 OHCI Controller (Class 0x0C:0x03:0x10)"),
                PCI_PROGIF_UHCI => f.write_str("USB 1.1 UHCI Controller (Class 0x0C:0x03:0x00)"),
                _ => f.write_str("USB Host Controller (Class 0x0C:0x03)"),
            },
            Self::NpuProcessingAccelerator => f.write_str("NPU / Processing Accelerator (Class 0x12)"),
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
            PCI_CLASS_MASS_STORAGE => {
                if self.subclass == PCI_SUBCLASS_NVME {
                    PciDeviceClass::NvmeController
                } else {
                    PciDeviceClass::MassStorageController
                }
            }
            PCI_CLASS_NETWORK_CONTROLLER => PciDeviceClass::NetworkController,
            PCI_CLASS_DISPLAY_CONTROLLER => PciDeviceClass::GpuDisplayController,
            PCI_CLASS_SERIAL_BUS if self.subclass == PCI_SUBCLASS_USB => {
                PciDeviceClass::UsbController { prog_if: self.prog_if }
            }
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

/// Internal iterator across active PCI devices on all buses and functions.
fn for_each_pci_func<F: FnMut(PciDevice) -> bool>(mut callback: F) {
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
                if callback(dev) {
                    return;
                }
            }
        }
    }
}

/// Scans the PCI/PCIe hierarchy, logging discovered devices.
pub fn scan_bus() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    for_each_pci_func(|dev| {
        match dev.classification() {
            PciDeviceClass::NvmeController => {
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [NVMe DETECTADO] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x}:{:#02x} (NVMe Storage) BAR0={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.prog_if, dev.bar0
                );
            }
            PciDeviceClass::MassStorageController => {
                let abar = pci_read_u32(dev.bus, dev.device, dev.function, 0x24);
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [SATA/AHCI DETECTADO] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (SATA AHCI) BAR5={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, abar
                );
            }
            PciDeviceClass::NetworkController => {
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [NIC DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (/dev/net/eth0) BAR0={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.bar0
                );
            }
            PciDeviceClass::GpuDisplayController => {
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [GPU DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (DRM/Display) BAR0={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.bar0
                );
            }
            PciDeviceClass::NpuProcessingAccelerator => {
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [NPU DETECTADA] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} (/dev/accel) BAR0={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.bar0
                );
            }
            PciDeviceClass::UsbController { prog_if } => {
                let name = match prog_if {
                    PCI_PROGIF_XHCI => "xHCI USB 3.0",
                    PCI_PROGIF_EHCI => "EHCI USB 2.0",
                    PCI_PROGIF_OHCI => "OHCI USB 1.1",
                    PCI_PROGIF_UHCI => "UHCI USB 1.1",
                    _ => "USB Controller",
                };
                crate::early_println!(
                    "[ostd::bus::pcie] >>> [USB DETECTADO] {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x}:{:02x} ({}) BAR0={:#010x}",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.prog_if, name, dev.bar0
                );
            }
            PciDeviceClass::Other(_) => {
                crate::early_println!(
                    "[ostd::bus::pcie]     [PCI Device]    {:02x}:{:02x}.{} | Vendor={:#06x} Device={:#06x} | Class={:#04x}:{:#04x} ({})",
                    dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, class_code_name(dev.class_code)
                );
            }
        }
        devices.push(dev);
        false
    });
    devices
}

/// Reads the full 32-bit or 64-bit physical MMIO `BAR0` address (`offset 0x10` / `0x14`).
pub fn read_mmio_bar0(bus: u8, device: u8, function: u8) -> Option<u64> {
    let bar0_low = pci_read_u32(bus, device, function, 0x10);
    if bar0_low == 0xFFFF_FFFF || (bar0_low & 0x1) != 0 {
        return None;
    }

    let bar_type = (bar0_low >> 1) & 0x3;
    let phys_addr = if bar_type == 0x2 {
        let bar0_high = pci_read_u32(bus, device, function, 0x14);
        ((bar0_high as u64) << 32) | ((bar0_low & 0xFFFF_FFF0) as u64)
    } else {
        (bar0_low & 0xFFFF_FFF0) as u64
    };

    if phys_addr == 0 { None } else { Some(phys_addr) }
}

/// Reads the full 32-bit or 64-bit physical MMIO `BAR5` (`ABAR`) address (`offset 0x24` / `0x28`).
pub fn read_mmio_bar5(bus: u8, device: u8, function: u8) -> Option<u64> {
    let bar5_low = pci_read_u32(bus, device, function, 0x24);
    if bar5_low == 0xFFFF_FFFF || (bar5_low & 0x1) != 0 {
        return None;
    }

    let bar_type = (bar5_low >> 1) & 0x3;
    let phys_addr = if bar_type == 0x2 {
        let bar5_high = pci_read_u32(bus, device, function, 0x28);
        ((bar5_high as u64) << 32) | ((bar5_low & 0xFFFF_FFF0) as u64)
    } else {
        (bar5_low & 0xFFFF_FFF0) as u64
    };

    if phys_addr == 0 { None } else { Some(phys_addr) }
}

/// Enables `Memory Space` (bit 1) and `Bus Master DMA` (bit 2) in the PCI Command register (`0x04`).
pub fn enable_bus_master_and_mmio(bus: u8, device: u8, function: u8) -> u16 {
    let cmd_status = pci_read_u32(bus, device, function, 0x04);
    let updated_cmd = (cmd_status & 0x0000_FFFF) | (1 << 1) | (1 << 2);
    pci_write_u32(bus, device, function, 0x04, updated_cmd);
    pci_read_u16(bus, device, function, 0x04)
}

/// Probes the PCI/PCIe bus for an Intel `e1000` / `e1000e` Gigabit Ethernet controller.
pub fn probe_e1000_nic() -> Option<(PciDevice, u64, u16)> {
    let mut found = None;
    for_each_pci_func(|dev| {
        let is_intel_eth = dev.vendor_id == 0x8086
            && dev.class_code == PCI_CLASS_NETWORK_CONTROLLER
            && dev.subclass == 0x00;
        if is_intel_eth {
            let pci_cmd = enable_bus_master_and_mmio(dev.bus, dev.device, dev.function);
            let bar0_phys = match read_mmio_bar0(dev.bus, dev.device, dev.function) {
                Some(addr) => addr,
                None => {
                    let fallback_mmio: u32 = 0xFEBC_0000;
                    pci_write_u32(dev.bus, dev.device, dev.function, 0x10, fallback_mmio);
                    fallback_mmio as u64
                }
            };
            found = Some((dev, bar0_phys, pci_cmd));
            return true;
        }
        false
    });
    found
}

/// Probes the PCI/PCIe bus for a SATA AHCI controller (`Class 0x01:0x06` or `0x8086:0x2922`).
pub fn probe_ahci_controller() -> Option<(PciDevice, u64, u16)> {
    let mut found = None;
    for_each_pci_func(|dev| {
        let is_ahci = (dev.class_code == PCI_CLASS_MASS_STORAGE && dev.subclass == PCI_SUBCLASS_SATA)
            || (dev.vendor_id == 0x8086 && dev.device_id == 0x2922);
        if is_ahci {
            let pci_cmd = enable_bus_master_and_mmio(dev.bus, dev.device, dev.function);
            let bar5_phys = match read_mmio_bar5(dev.bus, dev.device, dev.function) {
                Some(addr) => addr,
                None => {
                    let fallback_mmio: u32 = 0xFEB9_0000;
                    pci_write_u32(dev.bus, dev.device, dev.function, 0x24, fallback_mmio);
                    fallback_mmio as u64
                }
            };
            found = Some((dev, bar5_phys, pci_cmd));
            return true;
        }
        false
    });
    found
}

/// Probes the PCI/PCIe bus for a PCIe NVMe Controller (`Class 0x01:0x08:0x02` or Subclass `0x08`).
pub fn probe_nvme_controller() -> Option<(PciDevice, u64, u16)> {
    let mut found = None;
    for_each_pci_func(|dev| {
        let is_nvme = dev.class_code == PCI_CLASS_MASS_STORAGE && dev.subclass == PCI_SUBCLASS_NVME;
        if is_nvme {
            let pci_cmd = enable_bus_master_and_mmio(dev.bus, dev.device, dev.function);
            let bar0_phys = match read_mmio_bar0(dev.bus, dev.device, dev.function) {
                Some(addr) => addr,
                None => {
                    let fallback_mmio: u32 = 0xFEBA_0000;
                    pci_write_u32(dev.bus, dev.device, dev.function, 0x10, fallback_mmio);
                    fallback_mmio as u64
                }
            };
            found = Some((dev, bar0_phys, pci_cmd));
            return true;
        }
        false
    });
    found
}

#[derive(Debug, Clone, Copy)]
pub struct DiscoveredUsbDevice {
    pub dev: PciDevice,
    pub bar0_phys: u64,
    pub pci_cmd: u16,
    pub prog_if: u8,
}

/// Discovers all standard USB host controllers (xHCI, EHCI, OHCI, UHCI) on the PCI/PCIe bus.
pub fn probe_all_usb_controllers() -> alloc::vec::Vec<DiscoveredUsbDevice> {
    let mut list = alloc::vec::Vec::new();
    for_each_pci_func(|dev| {
        let is_usb = dev.class_code == PCI_CLASS_SERIAL_BUS && dev.subclass == PCI_SUBCLASS_USB;
        if is_usb {
            let pci_cmd = enable_bus_master_and_mmio(dev.bus, dev.device, dev.function);
            let bar0_phys = match read_mmio_bar0(dev.bus, dev.device, dev.function) {
                Some(addr) => addr,
                None => {
                    let fallback_mmio = 0xFEB8_0000u32 + ((dev.device as u32) << 12);
                    pci_write_u32(dev.bus, dev.device, dev.function, 0x10, fallback_mmio);
                    fallback_mmio as u64
                }
            };
            list.push(DiscoveredUsbDevice {
                dev,
                bar0_phys,
                pci_cmd,
                prog_if: dev.prog_if,
            });
        }
        false
    });
    list
}

/// Probes the PCI/PCIe bus for a USB xHCI Controller (`Class 0x0C:0x03:0x30`).
pub fn probe_xhci_controller() -> Option<(PciDevice, u64, u16)> {
    for d in probe_all_usb_controllers() {
        if d.prog_if == PCI_PROGIF_XHCI {
            return Some((d.dev, d.bar0_phys, d.pci_cmd));
        }
    }
    None
}

const fn class_code_name(class_code: u8) -> &'static str {
    match class_code {
        0x00 => "Unclassified", 0x01 => "Mass Storage Controller",
        0x02 => "Network Controller", 0x03 => "Display Controller (GPU)",
        0x04 => "Multimedia Controller", 0x05 => "Memory Controller",
        0x06 => "Bridge Device", 0x07 => "Simple Communication Controller",
        0x08 => "Base System Peripheral", 0x09 => "Input Device Controller",
        0x0C => "Serial Bus Controller", 0x12 => "Processing Accelerator (NPU)",
        _ => "Generic PCI Device",
    }
}

#[inline]
pub(crate) fn pci_read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
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
pub(crate) fn pci_write_u32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    #[cfg(target_arch = "x86_64")]
    // SAFETY: Writing standard x86_64 PCI Configuration Mechanism #1 ports (0xCF8/0xCFC) in Ring 0.
    unsafe {
        outl(PCI_CONFIG_ADDRESS, address);
        outl(PCI_CONFIG_DATA, value);
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (PCI_CONFIG_ADDRESS, PCI_CONFIG_DATA, address, value);
    }
}

#[inline]
pub(crate) fn pci_read_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_u32(bus, device, function, offset);
    let shift = ((offset & 2) * 8) as u32;
    ((dword >> shift) & 0xFFFF) as u16
}

#[inline]
pub(crate) fn pci_read_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
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
