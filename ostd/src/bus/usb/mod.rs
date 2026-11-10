pub const LEGACY_USB_MMIO_SIZE: usize = 0x1000;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::bus::pcie::probe_all_usb_controllers;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::mm::PhysAddr;
use crate::sync::SpinLock;

pub mod ehci;
pub mod ohci;
pub mod xhci;

pub use ehci::*;
pub use ohci::*;
pub use xhci::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbControllerKind {
    Xhci,
    Ehci,
    Ohci,
    Uhci,
    Unknown(u8),
}

impl UsbControllerKind {
    pub const fn from_prog_if(prog_if: u8) -> Self {
        match prog_if {
            0x30 => Self::Xhci,
            0x20 => Self::Ehci,
            0x10 => Self::Ohci,
            0x00 => Self::Uhci,
            other => Self::Unknown(other),
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Xhci => "xHCI USB 3.0",
            Self::Ehci => "EHCI USB 2.0",
            Self::Ohci => "OHCI USB 1.1",
            Self::Uhci => "UHCI USB 1.1",
            Self::Unknown(_) => "USB Desconocido",
        }
    }
}

/// Telemetry details for each discovered USB host controller.
#[derive(Debug, Clone, Copy)]
pub struct UsbControllerTelemetry {
    pub kind: UsbControllerKind,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0_phys: PhysAddr,
    pub version: u16,
    pub max_ports: u8,
    pub active_ports: u8,
    pub halted: bool,
}

/// Backward-compatible telemetry state snapshot for `/sys/fipsos/hwmon` and `sensors`.
pub(crate) static USB_INITIALIZED: AtomicBool = AtomicBool::new(false);
pub(crate) static USB_CONTROLLERS: SpinLock<Vec<UsbControllerTelemetry>> = SpinLock::new(Vec::new());

/// Probes and initializes all standard USB controllers on the PCIe bus (xHCI, EHCI, OHCI).
pub fn init() {
    let controllers = probe_all_usb_controllers();
    if controllers.is_empty() {
        crate::early_println!("[ostd::bus::usb] No se detectaron controladoras USB en bus PCIe.");
        return;
    }

    let mut discovered = Vec::new();
    for usb in &controllers {
        let kind = UsbControllerKind::from_prog_if(usb.prog_if);
        match kind {
            UsbControllerKind::Xhci => {
                discovered.push(xhci::init_xhci(usb));
            }
            UsbControllerKind::Ehci => {
                discovered.push(ehci::init_ehci(usb));
            }
            UsbControllerKind::Ohci => {
                discovered.push(ohci::init_ohci(usb));
            }
            UsbControllerKind::Uhci | UsbControllerKind::Unknown(_) => {
                crate::early_println!(
                    "[ostd::bus::usb] USB Host {:02x}:{:02x}.{} (ProgIF={:#04x}) detectada.",
                    usb.dev.bus, usb.dev.device, usb.dev.function, usb.prog_if
                );
            }
        }
    }
    *USB_CONTROLLERS.lock() = discovered;
    USB_INITIALIZED.store(true, Ordering::Release);
}

pub fn snapshot_all() -> Vec<UsbControllerTelemetry> {
    USB_CONTROLLERS.lock().clone()
}

pub fn summary_text() -> String {
    let controllers = USB_CONTROLLERS.lock();
    if controllers.is_empty() {
        return String::from("Ninguna");
    }
    let mut parts = Vec::new();
    for c in controllers.iter() {
        parts.push(format!("{}: {} puertos ({} activos)", c.kind.as_str(), c.max_ports, c.active_ports));
    }
    parts.join(", ")
}
