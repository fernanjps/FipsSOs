//! Intel 82540EM / 82574L (`e1000` / `e1000e`, `0x8086:0x100e` / `0x10d3`) PCIe Gigabit Ethernet
//! Hardware Driver with Contiguous Physical `RxDesc` / `TxDesc` DMA Rings (`ostd::net::e1000`).

use crate::bus::pcie::{probe_e1000_nic, PciDevice};
use crate::mm::frame::allocate_frame;
use crate::mm::{map_kernel_mmio, phys_to_virt, PhysAddr, VirtAddr};

/// Size of the Intel `e1000`/`e1000e` `BAR0` MMIO register aperture (128 KiB).
pub const E1000_MMIO_SIZE: usize = 128 * 1024;

/// Number of descriptors per hardware ring (`16 * 16 B = 256 B`, multiple of 128 B per Intel SDM).
pub const RX_RING_LEN: usize = 16;
pub const TX_RING_LEN: usize = 16;

/// Size of each DMA packet buffer (`2048 B`, matching `RCTL.BSIZE = 00b`).
pub const PKT_BUF_SIZE: usize = 2048;

// Intel e1000 / e1000e MMIO Register Offsets
const REG_CTRL: usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_EERD: usize = 0x0014;
const REG_ICR: usize = 0x00C0;
const REG_IMC: usize = 0x00D8;
const REG_RCTL: usize = 0x0100;
const REG_TCTL: usize = 0x0400;
const REG_TIPG: usize = 0x0410;

const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH: usize = 0x2810;
const REG_RDT: usize = 0x2818;

const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH: usize = 0x3810;
const REG_TDT: usize = 0x3818;

const REG_MTA_BASE: usize = 0x5200;
const REG_RAL0: usize = 0x5400;
const REG_RAH0: usize = 0x5404;

// Control / Status / Ring bitmasks
const CTRL_SLU: u32 = 1 << 6; // Set Link Up
const STATUS_LU: u32 = 1 << 1; // Link Up

const RCTL_EN: u32 = 1 << 1; // Receiver Enable
const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enable
const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enable
const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC

const TCTL_EN: u32 = 1 << 1; // Transmit Enable
const TCTL_PSP: u32 = 1 << 3; // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

const TX_CMD_EOP: u8 = 1 << 0; // End Of Packet
const TX_CMD_IFCS: u8 = 1 << 1; // Insert FCS / CRC
const TX_CMD_RS: u8 = 1 << 3; // Report Status (sets DD bit when DMA completes)
const DESC_STATUS_DD: u8 = 1 << 0; // Descriptor Done

const RAH_AV: u32 = 1 << 31; // Receive Address Valid

/// 16-byte Hardware Legacy Receive Descriptor (`RxDesc`).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct E1000RxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

/// 16-byte Hardware Legacy Transmit Descriptor (`TxDesc`).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct E1000TxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

/// Hardware snapshot of the PCIe Intel `e1000`/`e1000e` controller and its DMA counters.
#[derive(Debug, Clone, Copy)]
pub struct NicHardwareSnapshot {
    pub present: bool,
    pub pci_bus: u8,
    pub pci_dev: u8,
    pub pci_fn: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub pci_cmd: u16,
    pub mmio_phys: PhysAddr,
    pub rx_ring_phys: PhysAddr,
    pub tx_ring_phys: PhysAddr,
    pub mac: [u8; 6],
    pub link_up: bool,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// Stateful Ring 0 Intel `e1000`/`e1000e` PCIe NIC controller.
pub struct E1000Controller {
    present: bool,
    pci_dev: Option<PciDevice>,
    pci_cmd: u16,
    mmio_phys: PhysAddr,
    mmio_virt: VirtAddr,
    rx_ring_phys: PhysAddr,
    rx_ring_virt: VirtAddr,
    tx_ring_phys: PhysAddr,
    tx_ring_virt: VirtAddr,
    rx_buf_phys: [PhysAddr; RX_RING_LEN],
    rx_buf_virt: [VirtAddr; RX_RING_LEN],
    tx_buf_phys: [PhysAddr; TX_RING_LEN],
    tx_buf_virt: [VirtAddr; TX_RING_LEN],
    rx_tail: usize,
    tx_tail: usize,
    mac: [u8; 6],
    link_up: bool,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

impl E1000Controller {
    pub const fn uninitialized() -> Self {
        Self {
            present: false,
            pci_dev: None,
            pci_cmd: 0,
            mmio_phys: 0,
            mmio_virt: 0,
            rx_ring_phys: 0,
            rx_ring_virt: 0,
            tx_ring_phys: 0,
            tx_ring_virt: 0,
            rx_buf_phys: [0; RX_RING_LEN],
            rx_buf_virt: [0; RX_RING_LEN],
            tx_buf_phys: [0; TX_RING_LEN],
            tx_buf_virt: [0; TX_RING_LEN],
            rx_tail: RX_RING_LEN - 1,
            tx_tail: 0,
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            link_up: false,
            tx_packets: 0,
            rx_packets: 0,
            tx_bytes: 0,
            rx_bytes: 0,
        }
    }

    /// Probes PCIe `00:02.0` (`0x8086:0x10d3` / `0x100e`), maps `BAR0` MMIO (`PRESENT | WRITABLE | NO_CACHE`),
    /// reads MAC from `RAL0`/`RAH0` or EEPROM, and configures 16-entry `RxDesc` & `TxDesc` DMA rings.
    pub fn init_hardware(&mut self) {
        let Some((dev, bar0_phys, pci_cmd)) = probe_e1000_nic() else {
            crate::early_println!(
                "[ostd::net::e1000] Sin controlador PCIe Intel e1000/e1000e; activando modo Loopback determinista."
            );
            return;
        };

        let mmio_virt = map_kernel_mmio(bar0_phys, E1000_MMIO_SIZE);
        self.pci_dev = Some(dev);
        self.pci_cmd = pci_cmd;
        self.mmio_phys = bar0_phys;
        self.mmio_virt = mmio_virt;

        // 1. Read MAC address from RAL0/RAH0 (`0x5400`/`0x5404`) or EEPROM (`0x0014`), fallback `52:54:00:12:34:56`
        self.mac = self.detect_mac_address(dev.device_id);

        // 2. Mask NIC interrupts (`IMC`), clear pending `ICR`, and force `CTRL.SLU` (Set Link Up)
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
        let _ = self.read_reg(REG_ICR);
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | CTRL_SLU);

        // 3. Clear Multicast Table Array (`0x5200..0x5400`) and program `RAL0`/`RAH0` (`AV = 1`)
        for i in 0..128usize {
            self.write_reg(REG_MTA_BASE + i * 4, 0);
        }
        let ral0 = (self.mac[0] as u32)
            | ((self.mac[1] as u32) << 8)
            | ((self.mac[2] as u32) << 16)
            | ((self.mac[3] as u32) << 24);
        let rah0 = (self.mac[4] as u32) | ((self.mac[5] as u32) << 8) | RAH_AV;
        self.write_reg(REG_RAL0, ral0);
        self.write_reg(REG_RAH0, rah0);

        // 4. Allocate physical 4 KiB frames for `RxDesc` and `TxDesc` rings + 2 KiB packet buffers
        let rx_ring_paddr = allocate_frame().expect("Sin frames fisicos para anillo DMA RxDesc e1000");
        let tx_ring_paddr = allocate_frame().expect("Sin frames fisicos para anillo DMA TxDesc e1000");
        self.rx_ring_phys = rx_ring_paddr;
        self.rx_ring_virt = phys_to_virt(rx_ring_paddr);
        self.tx_ring_phys = tx_ring_paddr;
        self.tx_ring_virt = phys_to_virt(tx_ring_paddr);

        // 2 buffers of 2048 B per 4096 B physical frame
        for pair in 0..(RX_RING_LEN / 2) {
            let frame_rx = allocate_frame().expect("Sin frames fisicos para buffers RX DMA e1000");
            let frame_tx = allocate_frame().expect("Sin frames fisicos para buffers TX DMA e1000");
            for sub in 0..2usize {
                let idx = pair * 2 + sub;
                let off = (sub * PKT_BUF_SIZE) as u64;
                self.rx_buf_phys[idx] = frame_rx + off;
                self.rx_buf_virt[idx] = phys_to_virt(frame_rx + off);
                self.tx_buf_phys[idx] = frame_tx + off;
                self.tx_buf_virt[idx] = phys_to_virt(frame_tx + off);
            }
        }

        // 5. Populate `RxDesc` ring (`0x2800`) and enable hardware Receiver (`RCTL`)
        let rx_descs = self.rx_ring_virt as *mut E1000RxDesc;
        for i in 0..RX_RING_LEN {
            let desc = E1000RxDesc {
                buffer_addr: self.rx_buf_phys[i],
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
            // SAFETY: `rx_descs` points to a dedicated 4 KiB physical frame mapped in HHDM.
            unsafe {
                core::ptr::write_volatile(rx_descs.add(i), desc);
            }
        }
        self.rx_tail = RX_RING_LEN - 1;
        self.write_reg(REG_RDBAL, (self.rx_ring_phys & 0xFFFF_FFFF) as u32);
        self.write_reg(REG_RDBAH, (self.rx_ring_phys >> 32) as u32);
        self.write_reg(REG_RDLEN, (RX_RING_LEN * core::mem::size_of::<E1000RxDesc>()) as u32);
        self.write_reg(REG_RDH, 0);
        self.write_reg(REG_RDT, self.rx_tail as u32);
        self.write_reg(
            REG_RCTL,
            RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC,
        );

        // 6. Populate `TxDesc` ring (`0x3800`) and enable hardware Transmitter (`TCTL`)
        let tx_descs = self.tx_ring_virt as *mut E1000TxDesc;
        for i in 0..TX_RING_LEN {
            let desc = E1000TxDesc {
                buffer_addr: self.tx_buf_phys[i],
                length: 0,
                cso: 0,
                cmd: 0,
                status: DESC_STATUS_DD,
                css: 0,
                special: 0,
            };
            // SAFETY: `tx_descs` points to a dedicated 4 KiB physical frame mapped in HHDM.
            unsafe {
                core::ptr::write_volatile(tx_descs.add(i), desc);
            }
        }
        self.tx_tail = 0;
        self.write_reg(REG_TDBAL, (self.tx_ring_phys & 0xFFFF_FFFF) as u32);
        self.write_reg(REG_TDBAH, (self.tx_ring_phys >> 32) as u32);
        self.write_reg(REG_TDLEN, (TX_RING_LEN * core::mem::size_of::<E1000TxDesc>()) as u32);
        self.write_reg(REG_TDH, 0);
        self.write_reg(REG_TDT, 0);
        self.write_reg(REG_TIPG, 0x0060_200A);
        self.write_reg(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (0x10 << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT),
        );

        let status = self.read_reg(REG_STATUS);
        self.link_up = (status & STATUS_LU) != 0;
        self.present = true;

        crate::early_println!(
            "[ostd::net::e1000] NIC PCIe {:02x}:{:02x}.{} ({:#06x}:{:#06x}, CMD={:#06x}) BAR0={:#x} -> MMIO={:#x} | MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} | Link={} | DMA RX={:#x} TX={:#x}",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            pci_cmd,
            bar0_phys,
            mmio_virt,
            self.mac[0],
            self.mac[1],
            self.mac[2],
            self.mac[3],
            self.mac[4],
            self.mac[5],
            if self.link_up { "UP (1Gbps)" } else { "DOWN" },
            self.rx_ring_phys,
            self.tx_ring_phys
        );
        crate::fb_println!(
            "[ostd::net::e1000] NIC {:02x}:{:02x}.{} ({:#06x}:{:#06x}) MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} Link={}",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            self.mac[0],
            self.mac[1],
            self.mac[2],
            self.mac[3],
            self.mac[4],
            self.mac[5],
            if self.link_up { "UP" } else { "DOWN" }
        );
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    pub fn is_present(&self) -> bool {
        self.present
    }

    pub fn snapshot(&self) -> NicHardwareSnapshot {
        let (bus, device, function, vendor_id, device_id) = match self.pci_dev {
            Some(d) => (d.bus, d.device, d.function, d.vendor_id, d.device_id),
            None => (0, 2, 0, 0x8086, 0x10d3),
        };
        NicHardwareSnapshot {
            present: self.present,
            pci_bus: bus,
            pci_dev: device,
            pci_fn: function,
            vendor_id,
            device_id,
            pci_cmd: self.pci_cmd,
            mmio_phys: self.mmio_phys,
            rx_ring_phys: self.rx_ring_phys,
            tx_ring_phys: self.tx_ring_phys,
            mac: self.mac,
            link_up: self.link_up,
            tx_packets: self.tx_packets,
            rx_packets: self.rx_packets,
            tx_bytes: self.tx_bytes,
            rx_bytes: self.rx_bytes,
        }
    }

    /// Copies an Ethernet II frame into `TxDesc[tx_tail]`, rings the hardware `TDT` doorbell (`0x3818`),
    /// and waits for Bus Master DMA completion (`DESC_STATUS_DD`).
    pub fn transmit_frame(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > PKT_BUF_SIZE {
            return false;
        }
        let wire_len = frame.len().max(60);

        if !self.present {
            self.tx_packets = self.tx_packets.wrapping_add(1);
            self.tx_bytes = self.tx_bytes.wrapping_add(wire_len as u64);
            return false;
        }

        let idx = self.tx_tail;
        let dst_ptr = self.tx_buf_virt[idx] as *mut u8;
        let tx_descs = self.tx_ring_virt as *mut E1000TxDesc;

        // SAFETY: Writing packet bytes to owned 2 KiB DMA buffer and updating `TxDesc[idx]`.
        unsafe {
            for i in 0..wire_len {
                let b = if i < frame.len() { frame[i] } else { 0 };
                core::ptr::write_volatile(dst_ptr.add(i), b);
            }

            let desc = E1000TxDesc {
                buffer_addr: self.tx_buf_phys[idx],
                length: wire_len as u16,
                cso: 0,
                cmd: TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS,
                status: 0,
                css: 0,
                special: 0,
            };
            core::ptr::write_volatile(tx_descs.add(idx), desc);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }

        self.tx_tail = (idx + 1) % TX_RING_LEN;
        self.write_reg(REG_TDT, self.tx_tail as u32);

        let mut completed = false;
        for _ in 0..100_000 {
            // SAFETY: Polling volatile `status` byte updated by PCIe Bus Master DMA.
            let desc_now = unsafe { core::ptr::read_volatile(tx_descs.add(idx)) };
            if (desc_now.status & DESC_STATUS_DD) != 0 {
                completed = true;
                break;
            }
            core::hint::spin_loop();
        }

        if completed {
            self.tx_packets = self.tx_packets.wrapping_add(1);
            self.tx_bytes = self.tx_bytes.wrapping_add(wire_len as u64);
            self.link_up = (self.read_reg(REG_STATUS) & STATUS_LU) != 0;
        }
        completed
    }

    /// Polls `RxDesc[(rx_tail + 1) % RX_RING_LEN]` for a received Ethernet II frame (`DD != 0`),
    /// copies it into `out`, rearms the descriptor, and advances `REG_RDT` (`0x2818`).
    pub fn receive_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        if !self.present {
            return None;
        }

        let next_idx = (self.rx_tail + 1) % RX_RING_LEN;
        let rx_descs = self.rx_ring_virt as *mut E1000RxDesc;

        // SAFETY: Reading volatile `RxDesc` entry written by PCIe Bus Master DMA.
        let desc = unsafe { core::ptr::read_volatile(rx_descs.add(next_idx)) };
        if (desc.status & DESC_STATUS_DD) == 0 {
            return None;
        }

        let pkt_len = (desc.length as usize).min(PKT_BUF_SIZE);
        let copy_len = pkt_len.min(out.len());
        let src_ptr = self.rx_buf_virt[next_idx] as *const u8;

        // SAFETY: Reading `copy_len` bytes from DMA RX buffer and re-arming `RxDesc`.
        unsafe {
            for (i, slot) in out.iter_mut().take(copy_len).enumerate() {
                *slot = core::ptr::read_volatile(src_ptr.add(i));
            }
            let rearmed = E1000RxDesc {
                buffer_addr: self.rx_buf_phys[next_idx],
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
            core::ptr::write_volatile(rx_descs.add(next_idx), rearmed);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }

        self.rx_tail = next_idx;
        self.write_reg(REG_RDT, self.rx_tail as u32);
        self.rx_packets = self.rx_packets.wrapping_add(1);
        self.rx_bytes = self.rx_bytes.wrapping_add(pkt_len as u64);
        Some(copy_len)
    }

    fn detect_mac_address(&self, device_id: u16) -> [u8; 6] {
        let ral0 = self.read_reg(REG_RAL0);
        let rah0 = self.read_reg(REG_RAH0);
        let from_ral = [
            (ral0 & 0xFF) as u8,
            ((ral0 >> 8) & 0xFF) as u8,
            ((ral0 >> 16) & 0xFF) as u8,
            ((ral0 >> 24) & 0xFF) as u8,
            (rah0 & 0xFF) as u8,
            ((rah0 >> 8) & 0xFF) as u8,
        ];
        if is_valid_unicast_mac(&from_ral) {
            return from_ral;
        }

        if let (Some(w0), Some(w1), Some(w2)) = (
            self.read_eeprom_word(0, device_id),
            self.read_eeprom_word(1, device_id),
            self.read_eeprom_word(2, device_id),
        ) {
            let from_eep = [
                (w0 & 0xFF) as u8,
                (w0 >> 8) as u8,
                (w1 & 0xFF) as u8,
                (w1 >> 8) as u8,
                (w2 & 0xFF) as u8,
                (w2 >> 8) as u8,
            ];
            if is_valid_unicast_mac(&from_eep) {
                return from_eep;
            }
        }

        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
    }

    fn read_eeprom_word(&self, word_addr: u8, device_id: u16) -> Option<u16> {
        let is_e1000e = device_id == 0x10d3 || device_id == 0x10ea || device_id == 0x153a;
        let (cmd, done_mask) = if is_e1000e {
            (((word_addr as u32) << 2) | 1, 1u32 << 1)
        } else {
            (((word_addr as u32) << 8) | 1, 1u32 << 4)
        };
        self.write_reg(REG_EERD, cmd);
        for _ in 0..20_000 {
            let val = self.read_reg(REG_EERD);
            if (val & done_mask) != 0 {
                return Some(((val >> 16) & 0xFFFF) as u16);
            }
            core::hint::spin_loop();
        }
        None
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: `mmio_virt` maps the 128 KiB `BAR0` MMIO aperture with `NO_CACHE`.
        unsafe { core::ptr::read_volatile((self.mmio_virt + (offset as u64)) as *const u32) }
    }

    #[inline]
    fn write_reg(&self, offset: usize, val: u32) {
        // SAFETY: `mmio_virt` maps the 128 KiB `BAR0` MMIO aperture with `NO_CACHE`.
        unsafe {
            core::ptr::write_volatile((self.mmio_virt + (offset as u64)) as *mut u32, val);
        }
    }
}

#[inline]
fn is_valid_unicast_mac(mac: &[u8; 6]) -> bool {
    let all_zero = mac.iter().all(|&b| b == 0x00);
    let all_ff = mac.iter().all(|&b| b == 0xFF);
    !all_zero && !all_ff && (mac[0] & 0x01) == 0
}
