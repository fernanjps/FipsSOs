use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use crate::bus::pcie::{DiscoveredUsbDevice, PciDevice};
use crate::mm::dma::VmFrame;
use crate::mm::{map_kernel_mmio, PhysAddr, VirtAddr};
use crate::sync::SpinLock;
use super::xhci_hid::{
    arm_interrupt_in, enumerate_port, write_mmio_u64, HidDeviceKind, XhciHidSlot, XhciRingCtx,
    COMP_SHORT_PACKET, COMP_STALL, COMP_SUCCESS, COMP_USB_TX_ERR, EVENT_RING_ENTRIES,
    TRB_TYPE_TRANSFER_EVENT,
};
pub use super::xhci_hid::{decode_hid_boot_keycode, process_hid_boot_report};
use super::{UsbControllerKind, UsbControllerTelemetry, USB_CONTROLLERS, USB_CTRL_TIMEOUT_MS, USB_INITIALIZED};

pub const XHCI_MMIO_SIZE: usize = 0x10000;
const HID_BUFFER_CAPACITY: usize = 256;

// Capability Register Offsets
const REG_CAP_VER: u64 = 0x00;
const REG_HCSPARAMS1: u64 = 0x04;
const REG_HCCPARAMS1: u64 = 0x10;
const REG_DBOFF: u64 = 0x14;
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
    pub hid_keyboards: u8,
    pub hid_mice: u8,
    pub halted: bool,
}

struct XhciState {
    pci_dev: Option<PciDevice>,
    bar0_phys: PhysAddr,
    capbase: VirtAddr,
    opbase: VirtAddr,
    rtbase: VirtAddr,
    dbbase: VirtAddr,
    version: u16,
    max_slots: u8,
    max_ports: u8,
    active_ports: u8,
    halted: bool,
    _dcbaa_frame: Option<VmFrame>,
    _cmd_ring_frame: Option<VmFrame>,
    event_ring_frame: Option<VmFrame>,
    _erst_frame: Option<VmFrame>,
    event_dequeue_idx: usize,
    event_cycle_state: u32,
    hid_slots: Vec<XhciHidSlot>,
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
            dbbase: 0,
            version: 0,
            max_slots: 0,
            max_ports: 0,
            active_ports: 0,
            halted: true,
            _dcbaa_frame: None,
            _cmd_ring_frame: None,
            event_ring_frame: None,
            _erst_frame: None,
            event_dequeue_idx: 0,
            event_cycle_state: 1,
            hid_slots: Vec::new(),
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

    // 1. Read Capability Registers via 32-bit aligned MMIO reads (per xHCI spec & QEMU xhci_cap_ops)
    let (caplength, hciversion, hcsparams1, hccparams1, dboff, rtsoff) = unsafe {
        let cap_ptr = capbase as *const u8;
        let cap_ver = core::ptr::read_volatile(cap_ptr.add(REG_CAP_VER as usize) as *const u32);
        let raw_caplen = (cap_ver & 0xFF) as u8;
        let caplen = if raw_caplen >= 0x20 { raw_caplen } else { 0x40 };
        let ver = ((cap_ver >> 16) & 0xFFFF) as u16;
        let hcs1 = core::ptr::read_volatile(cap_ptr.add(REG_HCSPARAMS1 as usize) as *const u32);
        let hcc1 = core::ptr::read_volatile(cap_ptr.add(REG_HCCPARAMS1 as usize) as *const u32);
        let db = core::ptr::read_volatile(cap_ptr.add(REG_DBOFF as usize) as *const u32);
        let rts = core::ptr::read_volatile(cap_ptr.add(REG_RTSOFF as usize) as *const u32);
        (caplen as u64, ver, hcs1, hcc1, db, rts)
    };

    let opbase = capbase + caplength;
    let dbbase = capbase + ((dboff & 0xFFFF_FFFC) as u64);
    let rtbase = capbase + ((rtsoff & 0xFFFF_FFE0) as u64);
    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = (((hcsparams1 >> 24) & 0xFF) as u8).min(64);
    let ctx_size = if (hccparams1 & (1 << 2)) != 0 { 64 } else { 32 };

    // 2. Perform BIOS Handoff via Extended Capabilities (MMIO xECP in BAR0)
    let xecp = ((hccparams1 >> 16) & 0xFFFF) as u16;
    perform_bios_handoff(capbase, xecp);

    // 3. Halt & Reset Host Controller (HCRST) with 100 ms TSC timeout
    unsafe {
        let usbcmd = core::ptr::read_volatile((opbase + REG_USBCMD) as *const u32);
        core::ptr::write_volatile((opbase + REG_USBCMD) as *mut u32, usbcmd & !0x01);
        let halt_deadline = crate::time::tsc_deadline_ms(USB_CTRL_TIMEOUT_MS);
        while !crate::time::tsc_expired(halt_deadline) {
            let usbsts = core::ptr::read_volatile((opbase + REG_USBSTS) as *const u32);
            if (usbsts & 0x01) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        core::ptr::write_volatile((opbase + REG_USBCMD) as *mut u32, 1 << 1); // HCRST
        let reset_deadline = crate::time::tsc_deadline_ms(USB_CTRL_TIMEOUT_MS);
        while !crate::time::tsc_expired(reset_deadline) {
            let cmd = core::ptr::read_volatile((opbase + REG_USBCMD) as *const u32);
            let sts = core::ptr::read_volatile((opbase + REG_USBSTS) as *const u32);
            if (cmd & (1 << 1)) == 0 && (sts & (1 << 11)) == 0 {
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

            core::ptr::write_volatile((opbase + REG_CONFIG) as *mut u32, max_slots as u32);
            write_mmio_u64(opbase + REG_DCBAAP, dcbaa.paddr());
            write_mmio_u64(opbase + REG_CRCR, cmd_ring.paddr() | 1);

            let erst_ptr = erst.vaddr() as *mut u64;
            core::ptr::write_volatile(erst_ptr, ev_ring.paddr());
            core::ptr::write_volatile(erst.vaddr().wrapping_add(8) as *mut u32, EVENT_RING_ENTRIES as u32);

            let ir0 = rtbase + 0x20;
            core::ptr::write_volatile((ir0 + 0x08) as *mut u32, 1); // ERSTSZ
            write_mmio_u64(ir0 + 0x10, erst.paddr()); // ERSTBA
            write_mmio_u64(ir0 + 0x18, ev_ring.paddr() | (1 << 3)); // ERDP
            core::ptr::write_volatile((ir0 + 0x04) as *mut u32, 4000); // IMOD
            core::ptr::write_volatile((ir0 + 0x00) as *mut u32, 0x02); // IMAN IE=1
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

    // 6. Power on root hub ports, detect CCS == 1, and enumerate USB HID slots (Keyboard / Mouse)
    let mut active_ports = 0u8;
    let mut hid_slots = Vec::new();
    let mut ev_dequeue = 0usize;
    let mut ev_cycle = 1u32;

    if is_running {
        if let (Some(ref dcbaa), Some(ref cmd_ring), Some(ref ev_ring)) =
            (&dcbaa_frame, &cmd_ring_frame, &event_ring_frame)
        {
            let mut ring_ctx = XhciRingCtx {
                opbase,
                rtbase,
                dbbase,
                ctx_size,
                dcbaa_vaddr: dcbaa.vaddr(),
                cmd_vaddr: cmd_ring.vaddr(),
                cmd_paddr: cmd_ring.paddr(),
                cmd_enqueue: 0,
                cmd_cycle: 1,
                ev_vaddr: ev_ring.vaddr(),
                ev_paddr: ev_ring.paddr(),
                ev_dequeue: 0,
                ev_cycle: 1,
            };

            for p in 0..max_ports {
                let port_ptr = (opbase + REG_PORT_BASE + (p as u64) * PORT_REG_STRIDE) as *mut u32;
                unsafe {
                    let portsc = core::ptr::read_volatile(port_ptr);
                    if (portsc & PORTSC_PP) == 0 {
                        core::ptr::write_volatile(port_ptr, (portsc & !0x00FE_0002) | PORTSC_PP);
                    }
                    let updated = core::ptr::read_volatile(port_ptr);
                    if (updated & PORTSC_CCS) != 0 {
                        active_ports += 1;
                        match enumerate_port(&mut ring_ctx, p) {
                            Ok(slot) => hid_slots.push(slot),
                            Err(reason) => {
                                crate::early_println!("[ostd::bus::xhci] Puerto {} {}: abortado limpio.", p + 1, reason);
                            }
                        }
                    }
                }
            }
            ev_dequeue = ring_ctx.ev_dequeue;
            ev_cycle = ring_ctx.ev_cycle;
        }
    }

    let status_str = if is_running { "RUNNING" } else { "HALTED" };
    crate::early_println!(
        "[ostd::bus::xhci] xHCI USB 3.0 {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) BAR0={:#x} | HCI={:#x} ({}.{}) | Slots={} Ports={} (Activos={}, HID={}) | Estado={}",
        usb.dev.bus, usb.dev.device, usb.dev.function, usb.dev.vendor_id, usb.dev.device_id, usb.pci_cmd,
        usb.bar0_phys, hciversion, (hciversion >> 8) & 0xFF, (hciversion & 0xFF) as u8, max_slots, max_ports, active_ports, hid_slots.len(), status_str
    );

    {
        let mut guard = XHCI_DRIVER.lock();
        guard.pci_dev = Some(usb.dev);
        guard.bar0_phys = usb.bar0_phys;
        guard.capbase = capbase;
        guard.opbase = opbase;
        guard.rtbase = rtbase;
        guard.dbbase = dbbase;
        guard.version = hciversion;
        guard.max_slots = max_slots;
        guard.max_ports = max_ports;
        guard.active_ports = active_ports;
        guard.halted = !is_running;
        guard._dcbaa_frame = dcbaa_frame;
        guard._cmd_ring_frame = cmd_ring_frame;
        guard.event_ring_frame = event_ring_frame;
        guard._erst_frame = erst_frame;
        guard.event_dequeue_idx = ev_dequeue;
        guard.event_cycle_state = ev_cycle;
        guard.hid_slots = hid_slots;
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

pub(crate) fn perform_bios_handoff(capbase: u64, xecp_dwords: u16) {
    if xecp_dwords == 0 {
        return;
    }
    let mut rel_offset = (xecp_dwords as usize) * 4;
    for _ in 0..16 {
        if rel_offset + 8 > XHCI_MMIO_SIZE {
            break;
        }
        let cap_addr = capbase + (rel_offset as u64);
        let cap_dword = unsafe { core::ptr::read_volatile(cap_addr as *const u32) };
        let cap_id = (cap_dword & 0xFF) as u8;
        let next_dwords = ((cap_dword >> 8) & 0xFF) as usize;
        if cap_id == 1 {
            unsafe {
                core::ptr::write_volatile(cap_addr as *mut u32, cap_dword | (1 << 24));
                let ho_deadline = crate::time::tsc_deadline_ms(USB_CTRL_TIMEOUT_MS);
                while !crate::time::tsc_expired(ho_deadline) {
                    if (core::ptr::read_volatile(cap_addr as *const u32) & (1 << 16)) == 0 {
                        break;
                    }
                    core::hint::spin_loop();
                }
                let ctlsts = core::ptr::read_volatile((cap_addr + 4) as *const u32);
                core::ptr::write_volatile((cap_addr + 4) as *mut u32, ctlsts & 0xFFFF_0000);
            }
            return;
        }
        if next_dwords == 0 {
            break;
        }
        rel_offset = rel_offset.saturating_add(next_dwords * 4);
    }
}

pub fn poll_xhci_events() {
    let mut mouse_events = [(0u8, 0i8, 0i8, 0i8); 16];
    let mut mouse_count = 0usize;
    let mut kbd_keys = [(0u8, 0u8); 16];
    let mut kbd_count = 0usize;

    {
        let mut guard = match XHCI_DRIVER.try_lock() {
            Some(g) => g,
            None => return,
        };
        if guard.event_ring_frame.is_none() || guard.rtbase == 0 {
            return;
        }

        let (vaddr, paddr) = {
            let ev = guard.event_ring_frame.as_ref().unwrap();
            (ev.vaddr(), ev.paddr())
        };
        let dbbase = guard.dbbase;
        let erdp_addr = guard.rtbase + 0x20 + 0x18;

        for _ in 0..16 {
            let idx = guard.event_dequeue_idx % EVENT_RING_ENTRIES;
            let trb_ptr = (vaddr + (idx as u64) * 16) as *const u32;

            let (dw0, _dw1, dw2, dw3) = unsafe {
                let d0 = core::ptr::read_volatile(trb_ptr);
                let d1 = core::ptr::read_volatile(trb_ptr.add(1));
                let d2 = core::ptr::read_volatile(trb_ptr.add(2));
                let d3 = core::ptr::read_volatile(trb_ptr.add(3));
                (d0, d1, d2, d3)
            };

            if (dw3 & 1) != guard.event_cycle_state {
                break;
            }

            let trb_type = (dw3 >> 10) & 0x3F;
            let slot_id = ((dw3 >> 24) & 0xFF) as u8;
            let comp_code = ((dw2 >> 24) & 0xFF) as u8;

            if trb_type == TRB_TYPE_TRANSFER_EVENT && (comp_code == COMP_STALL || comp_code == COMP_USB_TX_ERR) {
                // Skip stalled/failed endpoint event cleanly without re-arming or spinning.
            } else if trb_type == TRB_TYPE_TRANSFER_EVENT && (comp_code == COMP_SUCCESS || comp_code == COMP_SHORT_PACKET) {
                if let Some(slot) = guard.hid_slots.iter_mut().find(|s| s.slot_id == slot_id) {
                    let mut rep = [0u8; 8];
                    unsafe {
                        core::ptr::copy_nonoverlapping(slot.data_vaddr() as *const u8, rep.as_mut_ptr(), 8);
                        core::ptr::write_bytes(slot.data_vaddr() as *mut u8, 0, 8);
                        arm_interrupt_in(dbbase, slot);
                    }
                    match slot.kind {
                        HidDeviceKind::Keyboard => {
                            let modifier = rep[0];
                            for &k in &rep[2..8] {
                                if k != 0 && !slot.prev_kbd_keys.contains(&k) && kbd_count < kbd_keys.len() {
                                    kbd_keys[kbd_count] = (modifier, k);
                                    kbd_count += 1;
                                }
                            }
                            slot.prev_kbd_keys.copy_from_slice(&rep[2..8]);
                        }
                        HidDeviceKind::Mouse => {
                            if mouse_count < mouse_events.len() {
                                mouse_events[mouse_count] =
                                    (rep[0] & 0x07, rep[1] as i8, rep[2] as i8, rep[3] as i8);
                                mouse_count += 1;
                            }
                        }
                    }
                } else {
                    let keycode = (dw0 & 0xFF) as u8;
                    let modifier = ((dw0 >> 8) & 0xFF) as u8;
                    if keycode != 0 && kbd_count < kbd_keys.len() {
                        kbd_keys[kbd_count] = (modifier, keycode);
                        kbd_count += 1;
                    }
                }
            }

            guard.event_dequeue_idx += 1;
            if guard.event_dequeue_idx % EVENT_RING_ENTRIES == 0 {
                guard.event_cycle_state ^= 1;
            }
            let next_trb_paddr = paddr + ((guard.event_dequeue_idx % EVENT_RING_ENTRIES) as u64) * 16;
            unsafe {
                write_mmio_u64(erdp_addr, next_trb_paddr | (1 << 3));
            }
        }
    }

    for &(modifier, keycode) in &kbd_keys[..kbd_count] {
        decode_hid_boot_keycode(modifier, keycode);
    }
    for &(buttons, dx, dy, wheel) in &mouse_events[..mouse_count] {
        crate::arch::mouse::inject_usb_hid_report(buttons, dx, dy, wheel);
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

pub fn push_hid_bytes(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut guard = XHCI_DRIVER.lock();
    if guard.hid_len + bytes.len() <= HID_BUFFER_CAPACITY {
        for &b in bytes {
            guard.push_byte(b);
        }
    }
}

pub fn push_hid_byte(b: u8) {
    push_hid_bytes(&[b]);
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
        let mut kbds = 0u8;
        let mut mice = 0u8;
        for s in &guard.hid_slots {
            match s.kind {
                HidDeviceKind::Keyboard => kbds += 1,
                HidDeviceKind::Mouse => mice += 1,
            }
        }
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
            hid_keyboards: kbds,
            hid_mice: mice,
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
            hid_keyboards: 0,
            hid_mice: 0,
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
            hid_keyboards: 0,
            hid_mice: 0,
            halted: true,
        }
    }
}
