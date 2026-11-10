use crate::bus::pcie::DiscoveredUsbDevice;
use crate::mm::map_kernel_mmio;
use super::{UsbControllerKind, UsbControllerTelemetry, LEGACY_USB_MMIO_SIZE};

pub const EHCI_MMIO_SIZE: usize = 0x1000;

pub fn init_ehci(usb: &DiscoveredUsbDevice) -> UsbControllerTelemetry {
                let capbase = map_kernel_mmio(usb.bar0_phys, LEGACY_USB_MMIO_SIZE);

                // EHCI Capability Registers
                // SAFETY: `capbase` is mapped to physical BAR0 MMIO.
                let (caplen, hciversion, hcsparams, hccparams) = unsafe {
                    let cap_ptr = capbase as *const u8;
                    let clen = core::ptr::read_volatile(cap_ptr);
                    let ver = core::ptr::read_volatile(cap_ptr.add(0x02) as *const u16);
                    let hcs = core::ptr::read_volatile(cap_ptr.add(0x04) as *const u32);
                    let hcc = core::ptr::read_volatile(cap_ptr.add(0x08) as *const u32);
                    (clen as u64, ver, hcs, hcc)
                };

                let max_ports = (hcsparams & 0x0F) as u8;
                let eecp = ((hccparams >> 8) & 0xFF) as u8;
                if eecp >= 0x40 {
                    super::xhci::perform_bios_handoff(usb.dev.bus, usb.dev.device, usb.dev.function, eecp);
                }

                let opbase = capbase + caplen;
                // SAFETY: Operational registers at `opbase`.
                let (usbsts, usbcmd) = unsafe {
                    let sts = core::ptr::read_volatile((opbase + 0x04) as *const u32);
                    let cmd = core::ptr::read_volatile(opbase as *const u32);
                    (sts, cmd)
                };

                let mut is_halted = (usbsts & (1 << 12)) != 0;
                if is_halted {
                    // Set Run bit (bit 0) in USBCMD
                    unsafe {
                        core::ptr::write_volatile(opbase as *mut u32, usbcmd | 0x01);
                        let updated = core::ptr::read_volatile((opbase + 0x04) as *const u32);
                        is_halted = (updated & (1 << 12)) != 0;
                    }
                }

                let mut active_ports = 0u8;
                for p in 0..max_ports {
                    let port_addr = opbase + 0x44 + (p as u64) * 4;
                    // SAFETY: Port status at `opbase + 0x44 + p * 4`.
                    unsafe {
                        let portsc = core::ptr::read_volatile(port_addr as *const u32);
                        if (portsc & (1 << 12)) == 0 {
                            core::ptr::write_volatile(port_addr as *mut u32, portsc | (1 << 12));
                        }
                        let updated = core::ptr::read_volatile(port_addr as *const u32);
                        if (updated & 0x01) != 0 {
                            active_ports += 1;
                        }
                    }
                }

                let major = (hciversion >> 8) & 0xFF;
                let minor = (hciversion & 0xFF) as u8;
                let status_str = if is_halted { "HALTED" } else { "RUNNING" };

                crate::early_println!(
                    "[ostd::bus::xhci] EHCI USB 2.0 {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) BAR0={:#x} -> MMIO={:#x} | HCI={:#x} ({}.{}) | Ports={} (Activos={}) | Estado={}",
                    usb.dev.bus,
                    usb.dev.device,
                    usb.dev.function,
                    usb.dev.vendor_id,
                    usb.dev.device_id,
                    usb.pci_cmd,
                    usb.bar0_phys,
                    capbase,
                    hciversion,
                    major,
                    minor,
                    max_ports,
                    active_ports,
                    status_str
                );

                UsbControllerTelemetry {
                    kind: UsbControllerKind::Ehci,
                    bus: usb.dev.bus,
                    device: usb.dev.device,
                    function: usb.dev.function,
                    vendor_id: usb.dev.vendor_id,
                    device_id: usb.dev.device_id,
                    bar0_phys: usb.bar0_phys,
                    version: hciversion,
                    max_ports,
                    active_ports,
                    halted: is_halted,
                }
}
