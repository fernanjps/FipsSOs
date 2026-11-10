use crate::bus::pcie::DiscoveredUsbDevice;
use crate::mm::map_kernel_mmio;
use super::{UsbControllerKind, UsbControllerTelemetry, LEGACY_USB_MMIO_SIZE};

pub const OHCI_MMIO_SIZE: usize = 0x1000;

pub fn init_ohci(usb: &DiscoveredUsbDevice) -> UsbControllerTelemetry {
                let capbase = map_kernel_mmio(usb.bar0_phys, LEGACY_USB_MMIO_SIZE);

                // OHCI Operational Registers
                // SAFETY: `capbase` points to OHCI MMIO registers.
                let (hcrev, hccontrol, rh_desca) = unsafe {
                    let rev = core::ptr::read_volatile(capbase as *const u32);
                    let ctrl = core::ptr::read_volatile((capbase + 0x04) as *const u32);
                    let rh = core::ptr::read_volatile((capbase + 0x48) as *const u32);
                    (rev as u16, ctrl, rh)
                };

                let max_ports = (rh_desca & 0xFF) as u8;
                let is_halted = (hccontrol & 0xC0) == 0;
                let mut active_ports = 0u8;

                for p in 0..max_ports {
                    let port_addr = capbase + 0x54 + (p as u64) * 4;
                    // SAFETY: OHCI port status at `0x54 + p * 4`.
                    unsafe {
                        let st = core::ptr::read_volatile(port_addr as *const u32);
                        if (st & 0x01) != 0 {
                            active_ports += 1;
                        }
                    }
                }

                let status_str = if is_halted { "HALTED" } else { "RUNNING" };
                crate::early_println!(
                    "[ostd::bus::xhci] OHCI USB 1.1 {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) BAR0={:#x} -> MMIO={:#x} | HCI={:#x} | Ports={} (Activos={}) | Estado={}",
                    usb.dev.bus,
                    usb.dev.device,
                    usb.dev.function,
                    usb.dev.vendor_id,
                    usb.dev.device_id,
                    usb.pci_cmd,
                    usb.bar0_phys,
                    capbase,
                    hcrev,
                    max_ports,
                    active_ports,
                    status_str
                );

                UsbControllerTelemetry {
                    kind: UsbControllerKind::Ohci,
                    bus: usb.dev.bus,
                    device: usb.dev.device,
                    function: usb.dev.function,
                    vendor_id: usb.dev.vendor_id,
                    device_id: usb.dev.device_id,
                    bar0_phys: usb.bar0_phys,
                    version: hcrev,
                    max_ports,
                    active_ports,
                    halted: is_halted,
                }
}
