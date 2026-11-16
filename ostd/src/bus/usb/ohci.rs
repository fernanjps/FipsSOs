use crate::bus::pcie::DiscoveredUsbDevice;
use crate::mm::map_kernel_mmio;
use super::{UsbControllerKind, UsbControllerTelemetry, LEGACY_USB_MMIO_SIZE, USB_CTRL_TIMEOUT_MS};

pub const OHCI_MMIO_SIZE: usize = 0x1000;

pub fn init_ohci(usb: &DiscoveredUsbDevice) -> UsbControllerTelemetry {
                let capbase = map_kernel_mmio(usb.bar0_phys, LEGACY_USB_MMIO_SIZE);

                // OHCI Operational Registers: bounded 100 ms TSC wait if reset/SMM ownership active
                // SAFETY: `capbase` points to OHCI MMIO registers.
                let (hcrev, hccontrol, rh_desca) = unsafe {
                    let rev = core::ptr::read_volatile(capbase as *const u32);
                    let mut ctrl = core::ptr::read_volatile((capbase + 0x04) as *const u32);
                    if (ctrl & (1 << 8)) != 0 {
                        // Request ownership change / wait up to 100 ms TSC timeout
                        let cmd_sts = core::ptr::read_volatile((capbase + 0x08) as *const u32);
                        core::ptr::write_volatile((capbase + 0x08) as *mut u32, cmd_sts | (1 << 3));
                        let deadline = crate::time::tsc_deadline_ms(USB_CTRL_TIMEOUT_MS);
                        while !crate::time::tsc_expired(deadline) {
                            ctrl = core::ptr::read_volatile((capbase + 0x04) as *const u32);
                            if (ctrl & (1 << 8)) == 0 {
                                break;
                            }
                            core::hint::spin_loop();
                        }
                    }
                    let rh = core::ptr::read_volatile((capbase + 0x48) as *const u32);
                    (rev as u16, ctrl, rh)
                };

                let max_ports = ((rh_desca & 0xFF) as u8).min(15);
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
