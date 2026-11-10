use core::sync::atomic::Ordering;
use crate::bus::pcie::{pci_read_u32, pci_write_u32, DiscoveredUsbDevice, PciDevice};
use crate::mm::dma::VmFrame;
use crate::mm::{map_kernel_mmio, PhysAddr, VirtAddr};
use crate::sync::SpinLock;
use super::{UsbControllerKind, UsbControllerTelemetry, USB_CONTROLLERS, USB_INITIALIZED};

pub const XHCI_MMIO_SIZE: usize = 0x10000;
const HID_BUFFER_CAPACITY: usize = 256;
const EVENT_RING_ENTRIES: usize = 64;

// Capability Register Offsets
const REG_CAPLENGTH: u64 = 0x00;
const REG_HCIVERSION: u64 = 0x02;
const REG_HCSPARAMS1: u64 = 0x04;
const REG_HCCPARAMS1: u64 = 0x10;
const REG_RTSOFF: u64 = 0x18;

// Operational Register Offsets
const REG_USBCMD: u64 = 0x00;
const REG_USBSTS: u64 = 0x04;
const REG_CRCR: u64 = 0x18;
const REG_DCBAAP: u64 = 0x30;
const REG_CONFIG: u64 = 0x38;
const REG_PORT_BASE: u64 = 0x400;
const PORT_REG_STRIDE: u64 = 0x10;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PP: u32 = 1 << 9;

pub struct XhciTelemetry {
    pub present: bool,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0_phys: PhysAddr,
    pub version: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    pub active_ports: u8,
    pub halted: bool,
}

struct XhciState {
    pci_dev: Option<PciDevice>,
    bar0_phys: PhysAddr,
    capbase: VirtAddr,
    opbase: VirtAddr,
    rtbase: VirtAddr,
    version: u16,
    max_slots: u8,
    max_ports: u8,
    active_ports: u8,
    halted: bool,
    dcbaa_frame: Option<VmFrame>,
    cmd_ring_frame: Option<VmFrame>,
    event_ring_frame: Option<VmFrame>,
    erst_frame: Option<VmFrame>,
    event_dequeue_idx: usize,
    event_cycle_state: u32,
    hid_buf: [u8; HID_BUFFER_CAPACITY],
    hid_head: usize,
    hid_tail: usize,
    hid_len: usize,
}

impl XhciState {
    const fn new() -> Self {
        Self {
            pci_dev: None,
            bar0_phys: 0,
            capbase: 0,
            opbase: 0,
            rtbase: 0,
            version: 0,
            max_slots: 0,
            max_ports: 0,
            active_ports: 0,
            halted: true,
            dcbaa_frame: None,
            cmd_ring_frame: None,
            event_ring_frame: None,
            erst_frame: None,
            event_dequeue_idx: 0,
            event_cycle_state: 1,
            hid_buf: [0; HID_BUFFER_CAPACITY],
            hid_head: 0,
            hid_tail: 0,
            hid_len: 0,
        }
    }

    fn push_byte(&mut self, b: u8) {
        if self.hid_len < HID_BUFFER_CAPACITY {
            self.hid_buf[self.hid_tail] = b;
            self.hid_tail = (self.hid_tail + 1) % HID_BUFFER_CAPACITY;
            self.hid_len += 1;
        }
    }

    fn pop_byte(&mut self) -> Option<u8> {
        if self.hid_len == 0 {
            return None;
        }
        let b = self.hid_buf[self.hid_head];
        self.hid_head = (self.hid_head + 1) % HID_BUFFER_CAPACITY;
        self.hid_len -= 1;
        Some(b)
    }
}

static XHCI_DRIVER: SpinLock<XhciState> = SpinLock::new(XhciState::new());

pub fn init_xhci(usb: &DiscoveredUsbDevice) -> UsbControllerTelemetry {
    let capbase = map_kernel_mmio(usb.bar0_phys, XHCI_MMIO_SIZE);

    // 1. Read Capability Registers
    let (caplength, hciversion, hcsparams1, hccparams1, rtsoff) = unsafe {
        let cap_ptr = capbase as *const u8;
        let caplen = core::ptr::read_volatile(cap_ptr.add(REG_CAPLENGTH as usize));
        let ver = core::ptr::read_volatile(cap_ptr.add(REG_HCIVERSION as usize) as *const u16);
        let hcs1 = core::ptr::read_volatile(cap_ptr.add(REG_HCSPARAMS1 as usize) as *const u32);
        let hcc1 = core::ptr::read_volatile(cap_ptr.add(REG_HCCPARAMS1 as usize) as *const u32);
        let rts = core::ptr::read_volatile(cap_ptr.add(REG_RTSOFF as usize) as *const u32);
        (caplen as u64, ver, hcs1, hcc1, rts)
    };

    let opbase = capbase + caplength;
    let rtbase = capbase + ((rtsoff & 0xFFFF_FFE0) as u64);
    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

    // 2. Perform BIOS Handoff via Extended Capabilities
    let xecp = ((hccparams1 >> 16) & 0xFFFF) as u8;
    perform_bios_handoff(usb.dev.bus, usb.dev.device, usb.dev.function, xecp);

    // 3. Halt controller temporarily to safely configure DMA rings
    unsafe {
        let usbcmd = core::ptr::read_volatile((opbase + REG_USBCMD) as *const u32);
        core::ptr::write_volatile((opbase + REG_USBCMD) as *mut u32, usbcmd & !0x01);
        for _ in 0..10_000 {
            let usbsts = core::ptr::read_volatile((opbase + REG_USBSTS) as *const u32);
            if (usbsts & 0x01) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    // 4. Allocate contiguous physical DMA frames for DCBAA, Command Ring, and Event Ring
    let dcbaa_frame = VmFrame::allocate().ok();
    let cmd_ring_frame = VmFrame::allocate().ok();
    let event_ring_frame = VmFrame::allocate().ok();
    let erst_frame = VmFrame::allocate().ok();

    if let (Some(ref dcbaa), Some(ref cmd_ring), Some(ref ev_ring), Some(ref erst)) =
        (&dcbaa_frame, &cmd_ring_frame, &event_ring_frame, &erst_frame)
    {
        unsafe {
            core::ptr::write_bytes(dcbaa.vaddr() as *mut u8, 0, 4096);
            core::ptr::write_bytes(cmd_ring.vaddr() as *mut u8, 0, 4096);
            core::ptr::write_bytes(ev_ring.vaddr() as *mut u8, 0, 4096);
            core::ptr::write_bytes(erst.vaddr() as *mut u8, 0, 4096);

            // Program Max Slots Enabled
            core::ptr::write_volatile((opbase + REG_CONFIG) as *mut u32, max_slots as u32);

            // Program DCBAAP (Device Context Base Address Array Pointer)
            core::ptr::write_volatile((opbase + REG_DCBAAP) as *mut u64, dcbaa.paddr());

            // Program CRCR (Command Ring Control Register with Ring Cycle State RCS = 1)
            core::ptr::write_volatile((opbase + REG_CRCR) as *mut u64, cmd_ring.paddr() | 1);

            // Configure Event Ring Segment Table (ERST): [Base PhysAddr (8B), Size=64 (4B), Res=0 (4B)]
            let erst_ptr = erst.vaddr() as *mut u64;
            core::ptr::write_volatile(erst_ptr, ev_ring.paddr());
            core::ptr::write_volatile(erst.vaddr().wrapping_add(8) as *mut u32, EVENT_RING_ENTRIES as u32);

            // Program Interrupter 0 Registers (rtbase + 0x20)
            let ir0 = rtbase + 0x20;
            core::ptr::write_volatile((ir0 + 0x08) as *mut u32, 1); // ERSTSZ = 1
            core::ptr::write_volatile((ir0 + 0x10) as *mut u64, erst.paddr()); // ERSTBA
            core::ptr::write_volatile((ir0 + 0x18) as *mut u64, ev_ring.paddr() | (1 << 3)); // ERDP with EHB=1
            core::ptr::write_volatile((ir0 + 0x04) as *mut u32, 4000); // IMOD (1ms)
            core::ptr::write_volatile((ir0 + 0x00) as *mut u32, 0x02); // IMAN (IE = 1)
        }
        crate::early_println!(
            "[ostd::bus::xhci] Anillos DMA xHCI configurados (DCBAAP={:#x}, CRCR={:#x}, ERSTBA={:#x}, ERDP={:#x})",
            dcbaa.paddr(), cmd_ring.paddr(), erst.paddr(), ev_ring.paddr()
        );
    }

    // 5. Start Host Controller (USBCMD: RS = 1, INTE = 1)
    let is_running = unsafe {
        let cmd = core::ptr::read_volatile((opbase + REG_USBCMD) as *const u32);
        core::ptr::write_volatile((opbase + REG_USBCMD) as *mut u32, cmd | 0x05);
        let sts = core::ptr::read_volatile((opbase + REG_USBSTS) as *const u32);
        (sts & 0x01) == 0
    };

    // 6. Power on and scan root hub ports
    let mut active_ports = 0u8;
    for p in 0..max_ports {
        let port_offset = opbase + REG_PORT_BASE + (p as u64) * PORT_REG_STRIDE;
        unsafe {
            let port_ptr = port_offset as *mut u32;
            let portsc = core::ptr::read_volatile(port_ptr);
            if (portsc & PORTSC_PP) == 0 {
                core::ptr::write_volatile(port_ptr, portsc | PORTSC_PP);
            }
            let updated = core::ptr::read_volatile(port_ptr);
            if (updated & PORTSC_CCS) != 0 {
                active_ports += 1;
            }
        }
    }

    let status_str = if is_running { "RUNNING" } else { "HALTED" };
    crate::early_println!(
        "[ostd::bus::xhci] xHCI USB 3.0 {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) BAR0={:#x} | HCI={:#x} ({}.{}) | Slots={} Ports={} (Activos={}) | Estado={}",
        usb.dev.bus, usb.dev.device, usb.dev.function, usb.dev.vendor_id, usb.dev.device_id, usb.pci_cmd,
        usb.bar0_phys, hciversion, (hciversion >> 8) & 0xFF, (hciversion & 0xFF) as u8, max_slots, max_ports, active_ports, status_str
    );

    {
        let mut guard = XHCI_DRIVER.lock();
        guard.pci_dev = Some(usb.dev);
        guard.bar0_phys = usb.bar0_phys;
        guard.capbase = capbase;
        guard.opbase = opbase;
        guard.rtbase = rtbase;
        guard.version = hciversion;
        guard.max_slots = max_slots;
        guard.max_ports = max_ports;
        guard.active_ports = active_ports;
        guard.halted = !is_running;
        guard.dcbaa_frame = dcbaa_frame;
        guard.cmd_ring_frame = cmd_ring_frame;
        guard.event_ring_frame = event_ring_frame;
        guard.erst_frame = erst_frame;
        guard.event_dequeue_idx = 0;
        guard.event_cycle_state = 1;
    }

    UsbControllerTelemetry {
        kind: UsbControllerKind::Xhci,
        bus: usb.dev.bus,
        device: usb.dev.device,
        function: usb.dev.function,
        vendor_id: usb.dev.vendor_id,
        device_id: usb.dev.device_id,
        bar0_phys: usb.bar0_phys,
        version: hciversion,
        max_ports,
        active_ports,
        halted: !is_running,
    }
}

pub(crate) fn perform_bios_handoff(bus: u8, dev: u8, func: u8, xecp_offset: u8) {
    if xecp_offset == 0 {
        return;
    }
    let mut current_offset = xecp_offset;
    for _ in 0..16 {
        let cap_dword = pci_read_u32(bus, dev, func, current_offset);
        let cap_id = (cap_dword & 0xFF) as u8;
        let next_offset = ((cap_dword >> 8) & 0xFF) as u8;
        if cap_id == 1 {
            pci_write_u32(bus, dev, func, current_offset, cap_dword | (1 << 24));
            for _ in 0..20_000 {
                if (pci_read_u32(bus, dev, func, current_offset) & (1 << 16)) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            let ctlsts = pci_read_u32(bus, dev, func, current_offset + 4);
            pci_write_u32(bus, dev, func, current_offset + 4, ctlsts & 0xFFFF_0000);
            return;
        }
        if next_offset == 0 {
            break;
        }
        current_offset += next_offset * 4;
    }
}

pub fn poll_xhci_events() {
    let mut guard = XHCI_DRIVER.lock();
    if guard.event_ring_frame.is_none() || guard.rtbase == 0 {
        return;
    }

    let (vaddr, paddr) = {
        let ev = guard.event_ring_frame.as_ref().unwrap();
        (ev.vaddr(), ev.paddr())
    };

    let idx = guard.event_dequeue_idx % EVENT_RING_ENTRIES;
    let trb_ptr = (vaddr + (idx as u64) * 16) as *const u32;

    let (dw0, _dw1, dw2, dw3) = unsafe {
        let d0 = core::ptr::read_volatile(trb_ptr);
        let d1 = core::ptr::read_volatile(trb_ptr.add(1));
        let d2 = core::ptr::read_volatile(trb_ptr.add(2));
        let d3 = core::ptr::read_volatile(trb_ptr.add(3));
        (d0, d1, d2, d3)
    };

    let cycle_bit = dw3 & 1;
    if cycle_bit != guard.event_cycle_state {
        return;
    }

    let trb_type = (dw3 >> 10) & 0x3F;
    if trb_type == 32 {
        let comp_code = (dw2 >> 24) & 0xFF;
        if comp_code == 1 {
            let keycode = (dw0 & 0xFF) as u8;
            let modifier = ((dw0 >> 8) & 0xFF) as u8;
            if keycode != 0 {
                decode_hid_boot_keycode(modifier, keycode);
            }
        }
    }

    guard.event_dequeue_idx += 1;
    if guard.event_dequeue_idx % EVENT_RING_ENTRIES == 0 {
        guard.event_cycle_state ^= 1;
    }

    let next_trb_paddr = paddr + ((guard.event_dequeue_idx % EVENT_RING_ENTRIES) as u64) * 16;
    let erdp_reg = (guard.rtbase + 0x20 + 0x18) as *mut u64;
    unsafe {
        core::ptr::write_volatile(erdp_reg, next_trb_paddr | (1 << 3));
    }
}

pub fn poll_ports() -> (u8, u8) {
    if !USB_INITIALIZED.load(Ordering::Acquire) {
        return (0, 0);
    }
    let mut guard = XHCI_DRIVER.lock();
    if guard.opbase == 0 || guard.max_ports == 0 {
        return (0, 0);
    }
    let mut active = 0u8;
    for p in 0..guard.max_ports {
        let port_offset = guard.opbase + REG_PORT_BASE + (p as u64) * PORT_REG_STRIDE;
        unsafe {
            let portsc = core::ptr::read_volatile(port_offset as *const u32);
            if (portsc & PORTSC_CCS) != 0 {
                active += 1;
            }
        }
    }
    guard.active_ports = active;
    (guard.max_ports, active)
}

pub fn push_hid_byte(b: u8) {
    XHCI_DRIVER.lock().push_byte(b);
}

pub fn decode_hid_boot_keycode(modifier: u8, keycode: u8) {
    if keycode == 0 {
        return;
    }
    let shift = (modifier & 0x22) != 0;
    match keycode {
        0x04..=0x1D => push_hid_byte(if shift { b'A' + (keycode - 0x04) } else { b'a' + (keycode - 0x04) }),
        0x1E..=0x26 => push_hid_byte(if shift { b"!@#$%^&*("[(keycode - 0x1E) as usize] } else { b'1' + (keycode - 0x1E) }),
        0x27 => push_hid_byte(if shift { b')' } else { b'0' }),
        0x28 => push_hid_byte(b'\n'),
        0x29 => push_hid_byte(0x1B),
        0x2A => push_hid_byte(0x08),
        0x2B => push_hid_byte(b'\t'),
        0x2C => push_hid_byte(b' '),
        0x2D => push_hid_byte(if shift { b'_' } else { b'-' }),
        0x2E => push_hid_byte(if shift { b'+' } else { b'=' }),
        0x4F => { push_hid_byte(0x1B); push_hid_byte(b'['); push_hid_byte(b'C'); }
        0x50 => { push_hid_byte(0x1B); push_hid_byte(b'['); push_hid_byte(b'D'); }
        0x51 => { push_hid_byte(0x1B); push_hid_byte(b'['); push_hid_byte(b'B'); }
        0x52 => { push_hid_byte(0x1B); push_hid_byte(b'['); push_hid_byte(b'A'); }
        _ => {}
    }
}

pub fn process_hid_boot_report(report: &[u8]) {
    if report.len() < 3 {
        return;
    }
    let modifier = report[0];
    for &key in &report[2..report.len().min(8)] {
        if key != 0 {
            decode_hid_boot_keycode(modifier, key);
        }
    }
}

pub fn pop_hid_byte() -> Option<u8> {
    if !USB_INITIALIZED.load(Ordering::Relaxed) {
        return None;
    }
    poll_xhci_events();
    XHCI_DRIVER.lock().pop_byte()
}

pub fn snapshot() -> XhciTelemetry {
    let guard = XHCI_DRIVER.lock();
    if let Some(dev) = guard.pci_dev {
        return XhciTelemetry {
            present: true,
            vendor_id: dev.vendor_id,
            device_id: dev.device_id,
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar0_phys: guard.bar0_phys,
            version: guard.version,
            max_slots: guard.max_slots,
            max_ports: guard.max_ports,
            active_ports: guard.active_ports,
            halted: guard.halted,
        };
    }
    drop(guard);

    let list = USB_CONTROLLERS.lock();
    if let Some(ctrl) = list.first() {
        XhciTelemetry {
            present: true,
            vendor_id: ctrl.vendor_id,
            device_id: ctrl.device_id,
            bus: ctrl.bus,
            device: ctrl.device,
            function: ctrl.function,
            bar0_phys: ctrl.bar0_phys,
            version: ctrl.version,
            max_slots: 0,
            max_ports: ctrl.max_ports,
            active_ports: ctrl.active_ports,
            halted: ctrl.halted,
        }
    } else {
        XhciTelemetry {
            present: false,
            vendor_id: 0,
            device_id: 0,
            bus: 0,
            device: 0,
            function: 0,
            bar0_phys: 0,
            version: 0,
            max_slots: 0,
            max_ports: 0,
            active_ports: 0,
            halted: true,
        }
    }
}
