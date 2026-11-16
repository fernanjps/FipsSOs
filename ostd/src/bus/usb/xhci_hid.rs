//! Enumeración USB xHCI de Dispositivos HID Boot Protocol (`Teclado Usage 0x06` y `Ratón Usage 0x02`).
//!
//! Implementa el ciclo completo de control xHCI sobre puertos activos (`CCS == 1`):
//! Port Reset (`PR`), `Enable Slot` (TRB 9), `Address Device` (TRB 11), lectura de descriptores
//! vía EP0, `Configure Endpoint` (TRB 12) y anillo DMA Interrupt IN para reportes HID en vivo.

use core::sync::atomic::{fence, Ordering};
use crate::mm::dma::VmFrame;
use crate::mm::VirtAddr;

pub const TRB_TYPE_NORMAL: u32 = 1;
pub const TRB_TYPE_SETUP: u32 = 2;
pub const TRB_TYPE_DATA: u32 = 3;
pub const TRB_TYPE_STATUS: u32 = 4;
pub const TRB_TYPE_LINK: u32 = 6;
pub const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
pub const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
pub const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
pub const TRB_TYPE_CMD_COMPLETION: u32 = 33;

pub const COMP_SUCCESS: u8 = 1;
pub const COMP_USB_TX_ERR: u8 = 4;
pub const COMP_STALL: u8 = 6;
pub const COMP_SHORT_PACKET: u8 = 13;
pub const XHCI_TX_TIMEOUT_MS: u64 = 150;

pub const EVENT_RING_ENTRIES: usize = 64;
pub const CMD_RING_USABLE: usize = 63;
const EP_RING_USABLE: usize = 31;

const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_RW1C_MASK: u32 = 0x00FE_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidDeviceKind {
    Keyboard, // HID Usage Page 0x01, Usage 0x06
    Mouse,    // HID Usage Page 0x01, Usage 0x02
}

impl HidDeviceKind {
    pub const fn usage_id(self) -> u8 {
        match self {
            Self::Keyboard => 0x06,
            Self::Mouse => 0x02,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "Teclado USB HID",
            Self::Mouse => "Raton USB HID",
        }
    }
}

/// Estado DMA por ranura (`Slot ID`) para un dispositivo xHCI HID enumerado.
pub struct XhciHidSlot {
    pub slot_id: u8,
    pub port_num: u8,
    pub speed: u8,
    pub kind: HidDeviceKind,
    pub ctx_frame: VmFrame,
    pub ring_frame: VmFrame,
    pub ep0_enqueue: usize,
    pub ep0_cycle: u32,
    pub int_dci: u8,
    pub int_max_pkt: u16,
    pub int_enqueue: usize,
    pub int_cycle: u32,
    pub prev_kbd_keys: [u8; 6],
}

impl XhciHidSlot {
    #[inline]
    fn ep0_vaddr(&self) -> VirtAddr {
        self.ring_frame.vaddr()
    }
    #[inline]
    fn ep0_paddr(&self) -> u64 {
        self.ring_frame.paddr()
    }
    #[inline]
    fn int_vaddr(&self) -> VirtAddr {
        self.ring_frame.vaddr() + 0x400
    }
    #[inline]
    fn int_paddr(&self) -> u64 {
        self.ring_frame.paddr() + 0x400
    }
    #[inline]
    pub fn data_vaddr(&self) -> VirtAddr {
        self.ring_frame.vaddr() + 0x800
    }
    #[inline]
    pub fn data_paddr(&self) -> u64 {
        self.ring_frame.paddr() + 0x800
    }
}

pub struct XhciRingCtx {
    pub opbase: VirtAddr,
    pub rtbase: VirtAddr,
    pub dbbase: VirtAddr,
    pub ctx_size: usize,
    pub dcbaa_vaddr: VirtAddr,
    pub cmd_vaddr: VirtAddr,
    pub cmd_paddr: u64,
    pub cmd_enqueue: usize,
    pub cmd_cycle: u32,
    pub ev_vaddr: VirtAddr,
    pub ev_paddr: u64,
    pub ev_dequeue: usize,
    pub ev_cycle: u32,
}

#[inline]
pub(crate) unsafe fn write_mmio_u64(addr: VirtAddr, val: u64) {
    let ptr = addr as *mut u32;
    core::ptr::write_volatile(ptr, val as u32);
    core::ptr::write_volatile(ptr.add(1), (val >> 32) as u32);
}

#[inline]
pub unsafe fn write_trb(base_vaddr: VirtAddr, idx: usize, dw0: u32, dw1: u32, dw2: u32, dw3: u32) {
    let ptr = (base_vaddr + (idx as u64) * 16) as *mut u32;
    core::ptr::write_volatile(ptr, dw0);
    core::ptr::write_volatile(ptr.add(1), dw1);
    core::ptr::write_volatile(ptr.add(2), dw2);
    fence(Ordering::Release);
    core::ptr::write_volatile(ptr.add(3), dw3);
}

#[inline]
pub unsafe fn ring_doorbell(dbbase: VirtAddr, slot_id: u8, target: u32) {
    fence(Ordering::SeqCst);
    let ptr = (dbbase + (slot_id as u64) * 4) as *mut u32;
    core::ptr::write_volatile(ptr, target);
}

unsafe fn init_link_trb(ring_vaddr: VirtAddr, ring_paddr: u64, link_idx: usize, cycle: u32) {
    write_trb(
        ring_vaddr,
        link_idx,
        ring_paddr as u32,
        (ring_paddr >> 32) as u32,
        0,
        (TRB_TYPE_LINK << 10) | (1 << 1) | (cycle & 1), // TC = 1
    );
}

unsafe fn poll_event_for(
    ctx: &mut XhciRingCtx,
    wanted_type: u32,
    wanted_slot: Option<u8>,
) -> Result<(u32, u32, u32, u32), &'static str> {
    let erdp_addr = ctx.rtbase + 0x20 + 0x18;
    let deadline = crate::time::tsc_deadline_ms(XHCI_TX_TIMEOUT_MS);
    while !crate::time::tsc_expired(deadline) {
        let idx = ctx.ev_dequeue % EVENT_RING_ENTRIES;
        let ptr = (ctx.ev_vaddr + (idx as u64) * 16) as *const u32;
        fence(Ordering::Acquire);
        let dw3 = core::ptr::read_volatile(ptr.add(3));
        if (dw3 & 1) != ctx.ev_cycle {
            core::hint::spin_loop();
            continue;
        }
        let dw0 = core::ptr::read_volatile(ptr);
        let dw1 = core::ptr::read_volatile(ptr.add(1));
        let dw2 = core::ptr::read_volatile(ptr.add(2));

        ctx.ev_dequeue += 1;
        if ctx.ev_dequeue % EVENT_RING_ENTRIES == 0 {
            ctx.ev_cycle ^= 1;
        }
        let next_p = ctx.ev_paddr + ((ctx.ev_dequeue % EVENT_RING_ENTRIES) as u64) * 16;
        write_mmio_u64(erdp_addr, next_p | (1 << 3));

        let ev_type = (dw3 >> 10) & 0x3F;
        let ev_slot = ((dw3 >> 24) & 0xFF) as u8;
        if ev_type == wanted_type && wanted_slot.map_or(true, |s| s == ev_slot) {
            let comp_code = ((dw2 >> 24) & 0xFF) as u8;
            return match comp_code {
                COMP_SUCCESS | COMP_SHORT_PACKET => Ok((dw0, dw1, dw2, dw3)),
                COMP_STALL => Err("xHCI: Transaccion detenida (COMP_STALL)"),
                COMP_USB_TX_ERR => Err("xHCI: Error de transaccion USB (COMP_USB_TX_ERR)"),
                _ => Err("xHCI: Codigo de terminacion TRB inesperado"),
            };
        }
    }
    Err("xHCI: Timeout 150ms esperando evento en Event Ring")
}

unsafe fn submit_command(
    ctx: &mut XhciRingCtx,
    dw0: u32,
    dw1: u32,
    dw2: u32,
    dw3_no_cycle: u32,
) -> Result<(u8, u8), &'static str> {
    if ctx.cmd_enqueue >= CMD_RING_USABLE {
        init_link_trb(ctx.cmd_vaddr, ctx.cmd_paddr, CMD_RING_USABLE, ctx.cmd_cycle);
        ctx.cmd_enqueue = 0;
        ctx.cmd_cycle ^= 1;
    }
    let idx = ctx.cmd_enqueue;
    let cyc = ctx.cmd_cycle;
    ctx.cmd_enqueue += 1;

    write_trb(ctx.cmd_vaddr, idx, dw0, dw1, dw2, (dw3_no_cycle & !1) | cyc);
    ring_doorbell(ctx.dbbase, 0, 0);

    let (_d0, _d1, d2, d3) = poll_event_for(ctx, TRB_TYPE_CMD_COMPLETION, None)?;
    let comp_code = ((d2 >> 24) & 0xFF) as u8;
    let slot_id = ((d3 >> 24) & 0xFF) as u8;
    Ok((comp_code, slot_id))
}

unsafe fn reset_root_port(opbase: VirtAddr, port_idx: u8) -> Result<u8, &'static str> {
    let port_ptr = (opbase + 0x400 + (port_idx as u64) * 0x10) as *mut u32;
    let portsc = core::ptr::read_volatile(port_ptr);
    let clean = (portsc & !(PORTSC_PED | PORTSC_RW1C_MASK)) | PORTSC_PP;
    core::ptr::write_volatile(port_ptr, clean | PORTSC_PR);

    let deadline = crate::time::tsc_deadline_ms(XHCI_TX_TIMEOUT_MS);
    while !crate::time::tsc_expired(deadline) {
        let cur = core::ptr::read_volatile(port_ptr);
        if (cur & 1) == 0 {
            return Err("xHCI: Puerto desconectado o sin medio");
        }
        if (cur & PORTSC_PR) == 0 && (cur & PORTSC_PED) != 0 {
            let speed = ((cur >> 10) & 0x0F) as u8;
            // Clear Port Reset Change (PRC) without clearing PED
            core::ptr::write_volatile(port_ptr, (cur & !(PORTSC_PED | PORTSC_RW1C_MASK)) | PORTSC_PP | (1 << 21));
            return Ok(speed);
        }
        core::hint::spin_loop();
    }
    Err("xHCI: Timeout 150ms en Port Reset")
}

unsafe fn enqueue_ep0(slot: &mut XhciHidSlot, dw0: u32, dw1: u32, dw2: u32, dw3_no_cycle: u32) {
    if slot.ep0_enqueue >= EP_RING_USABLE {
        init_link_trb(slot.ep0_vaddr(), slot.ep0_paddr(), EP_RING_USABLE, slot.ep0_cycle);
        slot.ep0_enqueue = 0;
        slot.ep0_cycle ^= 1;
    }
    let idx = slot.ep0_enqueue;
    let cyc = slot.ep0_cycle;
    slot.ep0_enqueue += 1;
    write_trb(slot.ep0_vaddr(), idx, dw0, dw1, dw2, (dw3_no_cycle & !1) | cyc);
}

unsafe fn ep0_control_in(
    ctx: &mut XhciRingCtx,
    slot: &mut XhciHidSlot,
    setup_lo: u32,
    setup_hi: u32,
    len: u16,
) -> Result<(), &'static str> {
    let data_paddr = slot.data_paddr();
    enqueue_ep0(slot, setup_lo, setup_hi, 8, (TRB_TYPE_SETUP << 10) | (3 << 16) | (1 << 6));
    if len > 0 {
        enqueue_ep0(
            slot,
            data_paddr as u32,
            (data_paddr >> 32) as u32,
            len as u32,
            (TRB_TYPE_DATA << 10) | (1 << 16),
        );
    }
    let status_dir = if len > 0 { 0u32 } else { 1u32 << 16 };
    enqueue_ep0(slot, 0, 0, 0, (TRB_TYPE_STATUS << 10) | status_dir | (1 << 5));
    ring_doorbell(ctx.dbbase, slot.slot_id, 1);

    let _ = poll_event_for(ctx, TRB_TYPE_TRANSFER_EVENT, Some(slot.slot_id))?;
    Ok(())
}

unsafe fn ep0_control_no_data(ctx: &mut XhciRingCtx, slot: &mut XhciHidSlot, setup_lo: u32, setup_hi: u32) -> Result<(), &'static str> {
    enqueue_ep0(slot, setup_lo, setup_hi, 8, (TRB_TYPE_SETUP << 10) | (1 << 6)); // TRT = 0
    enqueue_ep0(slot, 0, 0, 0, (TRB_TYPE_STATUS << 10) | (1 << 16) | (1 << 5)); // DIR=IN, IOC=1
    ring_doorbell(ctx.dbbase, slot.slot_id, 1);
    let _ = poll_event_for(ctx, TRB_TYPE_TRANSFER_EVENT, Some(slot.slot_id))?;
    Ok(())
}

fn parse_hid_config_descriptor(buf: &[u8]) -> Option<(HidDeviceKind, u8, u8, u16, u8)> {
    let mut i = 0usize;
    let mut cur_iface_num = 0u8;
    let mut cur_kind: Option<HidDeviceKind> = None;

    while i + 2 <= buf.len() {
        let len = buf[i] as usize;
        let dtype = buf[i + 1];
        if len < 2 || i + len > buf.len() {
            break;
        }
        if dtype == 4 && len >= 9 {
            // Interface Descriptor: bInterfaceNumber=2, bClass=5, bSubClass=6, bProto=7
            let if_num = buf[i + 2];
            let class = buf[i + 5];
            let proto = buf[i + 7];
            if class == 0x03 {
                cur_iface_num = if_num;
                cur_kind = match proto {
                    1 => Some(HidDeviceKind::Keyboard),
                    2 => Some(HidDeviceKind::Mouse),
                    _ => None,
                };
            } else {
                cur_kind = None;
            }
        } else if dtype == 5 && len >= 7 {
            if let Some(kind) = cur_kind {
                let ep_addr = buf[i + 2];
                let attrs = buf[i + 3];
                if (ep_addr & 0x80) != 0 && (attrs & 0x03) == 3 {
                    let ep_num = ep_addr & 0x0F;
                    let dci = ep_num * 2 + 1;
                    let max_pkt = u16::from_le_bytes([buf[i + 4], buf[i + 5]]).clamp(4, 64);
                    let interval = buf[i + 6];
                    return Some((kind, cur_iface_num, dci, max_pkt, interval));
                }
            }
        }
        i += len;
    }
    None
}

/// Encola un Normal TRB en el Interrupt IN Transfer Ring del dispositivo HID y toca su timbre.
pub unsafe fn arm_interrupt_in(dbbase: VirtAddr, slot: &mut XhciHidSlot) {
    if slot.int_enqueue >= EP_RING_USABLE {
        init_link_trb(slot.int_vaddr(), slot.int_paddr(), EP_RING_USABLE, slot.int_cycle);
        slot.int_enqueue = 0;
        slot.int_cycle ^= 1;
    }
    let idx = slot.int_enqueue;
    let cyc = slot.int_cycle;
    slot.int_enqueue += 1;

    let buf_paddr = slot.data_paddr();
    write_trb(
        slot.int_vaddr(),
        idx,
        buf_paddr as u32,
        (buf_paddr >> 32) as u32,
        slot.int_max_pkt as u32,
        (TRB_TYPE_NORMAL << 10) | (1 << 5) | (1 << 2) | cyc, // IOC=1, ISP=1
    );
    ring_doorbell(dbbase, slot.slot_id, slot.int_dci as u32);
}

/// Resetea y enumera un puerto activo (`CCS == 1`), configurando su slot, EP0 y Endpoint Interrupt IN HID.
pub unsafe fn enumerate_port(ctx: &mut XhciRingCtx, port_idx: u8) -> Result<XhciHidSlot, &'static str> {
    let speed = reset_root_port(ctx.opbase, port_idx)?;
    let (comp, slot_id) = submit_command(ctx, 0, 0, 0, TRB_TYPE_ENABLE_SLOT << 10)?;
    if comp != COMP_SUCCESS || slot_id == 0 {
        return Err("xHCI: EnableSlot fallo");
    }

    let ctx_frame = VmFrame::allocate().map_err(|_| "xHCI: Sin memoria DMA para ctx")?;
    let ring_frame = VmFrame::allocate().map_err(|_| "xHCI: Sin memoria DMA para ring")?;
    core::ptr::write_bytes(ctx_frame.vaddr() as *mut u8, 0, 4096);
    core::ptr::write_bytes(ring_frame.vaddr() as *mut u8, 0, 4096);

    core::ptr::write_volatile((ctx.dcbaa_vaddr + (slot_id as u64) * 8) as *mut u64, ctx_frame.paddr());

    let in_ctx_vaddr = ctx_frame.vaddr() + 0x800;
    let in_ctx_paddr = ctx_frame.paddr() + 0x800;
    let cs = ctx.ctx_size as u64;

    core::ptr::write_volatile((in_ctx_vaddr + 4) as *mut u32, 0x03);
    let slot_ctx = in_ctx_vaddr + cs;
    core::ptr::write_volatile(slot_ctx as *mut u32, ((speed as u32) << 20) | (1u32 << 27));
    core::ptr::write_volatile((slot_ctx + 4) as *mut u32, ((port_idx as u32) + 1) << 16);

    let max_pkt0: u32 = match speed {
        1 | 2 => 8,
        3 => 64,
        _ => 512,
    };
    let ep0_ctx = in_ctx_vaddr + 2 * cs;
    let ep0_paddr = ring_frame.paddr();
    core::ptr::write_volatile((ep0_ctx + 4) as *mut u32, (3 << 1) | (4 << 3) | (max_pkt0 << 16));
    core::ptr::write_volatile((ep0_ctx + 8) as *mut u32, (ep0_paddr as u32) | 1);
    core::ptr::write_volatile((ep0_ctx + 12) as *mut u32, (ep0_paddr >> 32) as u32);
    core::ptr::write_volatile((ep0_ctx + 16) as *mut u32, 8);

    let (addr_comp, _) = submit_command(
        ctx,
        in_ctx_paddr as u32,
        (in_ctx_paddr >> 32) as u32,
        0,
        (TRB_TYPE_ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
    )?;
    if addr_comp != COMP_SUCCESS {
        core::ptr::write_volatile((ctx.dcbaa_vaddr + (slot_id as u64) * 8) as *mut u64, 0);
        return Err("xHCI: AddressDevice rechazado por dispositivo");
    }

    let mut slot = XhciHidSlot {
        slot_id,
        port_num: port_idx + 1,
        speed,
        kind: HidDeviceKind::Keyboard,
        ctx_frame,
        ring_frame,
        ep0_enqueue: 0,
        ep0_cycle: 1,
        int_dci: 3,
        int_max_pkt: 8,
        int_enqueue: 0,
        int_cycle: 1,
        prev_kbd_keys: [0; 6],
    };

    // GET_DESCRIPTOR (Configuration, index 0, len=64)
    if let Err(err) = ep0_control_in(ctx, &mut slot, 0x0200_0680, (64u32) << 16, 64) {
        core::ptr::write_volatile((ctx.dcbaa_vaddr + (slot_id as u64) * 8) as *mut u64, 0);
        return Err(err);
    }
    let mut cfg_len_buf = [0u8; 64];
    core::ptr::copy_nonoverlapping(slot.data_vaddr() as *const u8, cfg_len_buf.as_mut_ptr(), 64);

    let Some((kind, iface_num, dci, max_pkt, raw_interval)) = parse_hid_config_descriptor(&cfg_len_buf) else {
        core::ptr::write_volatile((ctx.dcbaa_vaddr + (slot_id as u64) * 8) as *mut u64, 0);
        return Err("xHCI: Puerto no HID Boot (hub o lector sin medio), omitido");
    };
    slot.kind = kind;
    slot.int_dci = dci;
    slot.int_max_pkt = max_pkt;

    ep0_control_no_data(ctx, &mut slot, 0x0001_0900, 0)?;
    let _ = ep0_control_no_data(ctx, &mut slot, 0x0000_0B21, iface_num as u32);

    core::ptr::write_bytes(in_ctx_vaddr as *mut u8, 0, 2048);
    core::ptr::write_volatile((in_ctx_vaddr + 4) as *mut u32, (1u32 << 0) | (1u32 << dci));
    core::ptr::copy_nonoverlapping(slot.ctx_frame.vaddr() as *const u8, slot_ctx as *mut u8, ctx.ctx_size);
    let dw0 = core::ptr::read_volatile(slot_ctx as *const u32);
    core::ptr::write_volatile(slot_ctx as *mut u32, (dw0 & !(0x1F << 27)) | ((dci as u32) << 27));

    let ep_ctx = in_ctx_vaddr + ((dci as u64) + 1) * cs;
    let xhci_interval = if speed >= 3 { 6u32 } else { raw_interval.clamp(3, 10) as u32 };
    let int_paddr = slot.int_paddr();
    core::ptr::write_volatile(ep_ctx as *mut u32, xhci_interval << 16);
    core::ptr::write_volatile((ep_ctx + 4) as *mut u32, (3 << 1) | (7 << 3) | ((max_pkt as u32) << 16));
    core::ptr::write_volatile((ep_ctx + 8) as *mut u32, (int_paddr as u32) | 1);
    core::ptr::write_volatile((ep_ctx + 12) as *mut u32, (int_paddr >> 32) as u32);
    core::ptr::write_volatile((ep_ctx + 16) as *mut u32, (max_pkt as u32) | ((max_pkt as u32) << 16));

    let (cfg_comp, _) = submit_command(
        ctx,
        in_ctx_paddr as u32,
        (in_ctx_paddr >> 32) as u32,
        0,
        (TRB_TYPE_CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
    )?;
    if cfg_comp != COMP_SUCCESS {
        core::ptr::write_volatile((ctx.dcbaa_vaddr + (slot_id as u64) * 8) as *mut u64, 0);
        return Err("xHCI: ConfigureEndpoint fallo");
    }

    core::ptr::write_bytes(slot.data_vaddr() as *mut u8, 0, 64);
    arm_interrupt_in(ctx.dbbase, &mut slot);

    crate::early_println!(
        "[ostd::bus::xhci::hid] Puerto {} -> Slot {} (Speed={}) enumerado: {} (Usage={:#04x}, DCI={}, MaxPkt={})",
        slot.port_num, slot.slot_id, slot.speed, slot.kind.label(), slot.kind.usage_id(), slot.int_dci, slot.int_max_pkt
    );

    Ok(slot)
}

#[inline]
pub(super) fn hid_keycode_to_ascii(modifier: u8, keycode: u8) -> Option<u8> {
    if keycode == 0 {
        return None;
    }
    let shift = (modifier & 0x22) != 0;
    let ctrl = (modifier & 0x11) != 0;
    match keycode {
        0x04..=0x1D => {
            if ctrl {
                Some((keycode - 0x04) + 1)
            } else {
                Some(if shift { b'A' + (keycode - 0x04) } else { b'a' + (keycode - 0x04) })
            }
        }
        0x1E..=0x26 => Some(if shift { b"!@#$%^&*("[(keycode - 0x1E) as usize] } else { b'1' + (keycode - 0x1E) }),
        0x27 => Some(if shift { b')' } else { b'0' }),
        0x28 => Some(b'\n'),
        0x29 => Some(0x1B),
        0x2A => Some(0x08),
        0x2B => Some(b'\t'),
        0x2C => Some(b' '),
        0x2D => Some(if shift { b'_' } else { b'-' }),
        0x2E => Some(if shift { b'+' } else { b'=' }),
        0x2F => Some(if shift { b'{' } else { b'[' }),
        0x30 => Some(if shift { b'}' } else { b']' }),
        0x31 => Some(if shift { b'|' } else { b'\\' }),
        0x33 => Some(if shift { b':' } else { b';' }),
        0x34 => Some(if shift { b'"' } else { b'\'' }),
        0x36 => Some(if shift { b'<' } else { b',' }),
        0x37 => Some(if shift { b'>' } else { b'.' }),
        0x38 => Some(if shift { b'?' } else { b'/' }),
        _ => None,
    }
}

pub fn decode_hid_boot_keycode(modifier: u8, keycode: u8) {
    match keycode {
        0x4A => super::push_hid_bytes(b"\x1b[H"),  // Home
        0x4B => super::push_hid_bytes(b"\x1b[5~"), // PageUp
        0x4C => super::push_hid_bytes(b"\x1b[3~"), // Delete
        0x4D => super::push_hid_bytes(b"\x1b[F"),  // End
        0x4E => super::push_hid_bytes(b"\x1b[6~"), // PageDown
        0x4F => super::push_hid_bytes(b"\x1b[C"),  // Right
        0x50 => super::push_hid_bytes(b"\x1b[D"),  // Left
        0x51 => super::push_hid_bytes(b"\x1b[B"),  // Down
        0x52 => super::push_hid_bytes(b"\x1b[A"),  // Up
        _ => {
            if let Some(b) = hid_keycode_to_ascii(modifier, keycode) {
                super::push_hid_bytes(&[b]);
            }
        }
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
