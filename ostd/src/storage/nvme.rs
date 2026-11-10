//! Bare-Metal PCI Express NVM Express (NVMe) Driver (`ostd::storage::nvme`).
//!
//! Implements direct Ring 0 hardware communication with NVMe SSD controllers:
//! - BAR0 MMIO Configuration Registers (CAP, VS, CC, CSTS, AQA, ASQ, ACQ).
//! - Dedicated contiguous physical DMA frames for Admin Submission (ASQ) and Completion (ACQ) Queues.
//! - Controller enable handshake (CC.EN = 1 -> wait CSTS.RDY == 1).
//! - Hardware telemetry snapshot for kernel sensors and /sys/fipsos/hwmon.

use crate::bus::pcie::{probe_nvme_controller, PciDevice};
use crate::mm::dma::VmFrame;
use crate::mm::{map_kernel_mmio, VirtAddr};
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

/// MMIO aperture size for NVMe BAR0 (16 KiB).
pub const NVME_MMIO_SIZE: usize = 0x4000;

/// Number of entries in Admin Queues (64 entries * 64 B ASQ = 4 KiB frame; 64 entries * 16 B ACQ = 1 KiB).
pub const ADMIN_QUEUE_DEPTH: u16 = 64;

// NVMe MMIO Register Offsets
const REG_CAP: usize = 0x00;   // Controller Capabilities (64-bit)
const REG_VS: usize = 0x08;    // Version (32-bit)
const _REG_INTMS: usize = 0x0C; // Interrupt Mask Set (32-bit)
const _REG_INTMC: usize = 0x10; // Interrupt Mask Clear (32-bit)
const REG_CC: usize = 0x14;    // Controller Configuration (32-bit)
const REG_CSTS: usize = 0x1C;  // Controller Status (32-bit)
const REG_AQA: usize = 0x24;   // Admin Queue Attributes (32-bit)
const REG_ASQ: usize = 0x28;   // Admin Submission Queue Base (64-bit)
const REG_ACQ: usize = 0x30;   // Admin Completion Queue Base (64-bit)

// CC Register Flags
const CC_EN: u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4;
const CC_MPS_4K: u32 = 0 << 7;
const CC_AMS_RR: u32 = 0 << 11;
const CC_IOSQES_64: u32 = 6 << 16;
const CC_IOCQES_16: u32 = 4 << 20;

// CSTS Register Flags
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;

/// NVMe Hardware Telemetry snapshot.
#[derive(Debug, Clone, Copy)]
pub struct NvmeTelemetry {
    pub present: bool,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0_phys: u64,
    pub version_major: u16,
    pub version_minor: u8,
    pub version_tertiary: u8,
    pub mqes: u16,
    pub admin_q_depth: u16,
    pub rdy: bool,
    pub cfs: bool,
}

struct NvmeState {
    pci_dev: Option<PciDevice>,
    bar0_phys: u64,
    mmio_base: VirtAddr,
    asq_frame: Option<VmFrame>,
    acq_frame: Option<VmFrame>,
    telemetry: NvmeTelemetry,
}

impl NvmeState {
    const fn empty() -> Self {
        Self {
            pci_dev: None,
            bar0_phys: 0,
            mmio_base: 0,
            asq_frame: None,
            acq_frame: None,
            telemetry: NvmeTelemetry {
                present: false,
                vendor_id: 0,
                device_id: 0,
                bus: 0,
                device: 0,
                function: 0,
                bar0_phys: 0,
                version_major: 0,
                version_minor: 0,
                version_tertiary: 0,
                mqes: 0,
                admin_q_depth: 0,
                rdy: false,
                cfs: false,
            },
        }
    }
}

static NVME_STATE: SpinLock<NvmeState> = SpinLock::new(NvmeState::empty());
static NVME_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Probes the PCI bus for an NVMe controller, configures Admin DMA queues and enables the device.
pub fn init() {
    let (pci_dev, bar0_phys, pci_cmd) = match probe_nvme_controller() {
        Some(res) => res,
        None => {
            crate::early_println!("[ostd::storage::nvme] No se detecto controladora PCIe NVMe en el bus.");
            crate::fb_println!("[ostd::storage::nvme] No se detecto controladora PCIe NVMe en el bus.");
            return;
        }
    };

    crate::early_println!(
        "[ostd::storage::nvme] Dispositivo PCIe NVMe detectado en {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) BAR0={:#x}",
        pci_dev.bus, pci_dev.device, pci_dev.function, pci_dev.vendor_id, pci_dev.device_id, pci_cmd, bar0_phys
    );

    // 1. Map BAR0 MMIO aperture into kernel virtual address space
    let mmio_base = map_kernel_mmio(bar0_phys, NVME_MMIO_SIZE);

    // 2. Read Controller Capabilities (CAP) and Version (VS)
    // SAFETY: Accessing valid mapped MMIO BAR0 space of the NVMe controller.
    let (cap, vs) = unsafe {
        let cap_val = core::ptr::read_volatile((mmio_base + REG_CAP as u64) as *const u64);
        let vs_val = core::ptr::read_volatile((mmio_base + REG_VS as u64) as *const u32);
        (cap_val, vs_val)
    };

    let mqes = (cap & 0xFFFF) as u16 + 1;
    let dstrd = ((cap >> 32) & 0x0F) as u8;
    let ver_major = (vs >> 16) as u16;
    let ver_minor = ((vs >> 8) & 0xFF) as u8;
    let ver_tertiary = (vs & 0xFF) as u8;

    crate::early_println!(
        "[ostd::storage::nvme] CAP={:#018x} (MQES={}, DSTRD={}) | VS={:#010x} (NVMe {}.{}.{})",
        cap, mqes, dstrd, vs, ver_major, ver_minor, ver_tertiary
    );

    // 3. Reset controller if already enabled (CC.EN == 1)
    // SAFETY: Reading/writing CC and CSTS registers in mapped MMIO.
    unsafe {
        let cc_val = core::ptr::read_volatile((mmio_base + REG_CC as u64) as *const u32);
        if (cc_val & CC_EN) != 0 {
            core::ptr::write_volatile((mmio_base + REG_CC as u64) as *mut u32, 0);
            for _ in 0..50_000 {
                let csts = core::ptr::read_volatile((mmio_base + REG_CSTS as u64) as *const u32);
                if (csts & CSTS_RDY) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }

    // 4. Allocate contiguous physical DMA frames for Admin Queues
    let asq_frame = match VmFrame::allocate() {
        Ok(f) => f,
        Err(_) => {
            crate::early_println!("[ostd::storage::nvme] Error: No se pudo asignar frame DMA para ASQ.");
            return;
        }
    };
    let acq_frame = match VmFrame::allocate() {
        Ok(f) => f,
        Err(_) => {
            crate::early_println!("[ostd::storage::nvme] Error: No se pudo asignar frame DMA para ACQ.");
            return;
        }
    };

    let asq_paddr = asq_frame.paddr();
    let asq_vaddr = asq_frame.vaddr();
    let acq_paddr = acq_frame.paddr();
    let acq_vaddr = acq_frame.vaddr();

    // Zero out both DMA queue frames
    // SAFETY: Both frames were freshly allocated exclusively for NVMe Admin Queues.
    unsafe {
        core::ptr::write_bytes(asq_vaddr as *mut u8, 0, 4096);
        core::ptr::write_bytes(acq_vaddr as *mut u8, 0, 4096);
    }

    // 5. Configure Admin Queue Attributes (AQA), ASQ, and ACQ
    let aqa_val: u32 = (((ADMIN_QUEUE_DEPTH - 1) as u32) << 16) | ((ADMIN_QUEUE_DEPTH - 1) as u32);
    // SAFETY: Programming standard NVMe Admin Queue registers in MMIO.
    unsafe {
        core::ptr::write_volatile((mmio_base + REG_AQA as u64) as *mut u32, aqa_val);
        core::ptr::write_volatile((mmio_base + REG_ASQ as u64) as *mut u64, asq_paddr);
        core::ptr::write_volatile((mmio_base + REG_ACQ as u64) as *mut u64, acq_paddr);
    }

    // 6. Enable Controller (CC.EN = 1) with standard parameters
    let cc_target = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_AMS_RR | CC_IOSQES_64 | CC_IOCQES_16;
    // SAFETY: Enabling NVMe controller via CC register.
    unsafe {
        core::ptr::write_volatile((mmio_base + REG_CC as u64) as *mut u32, cc_target);
    }

    // 7. Poll CSTS.RDY == 1
    let mut ready = false;
    let mut fatal = false;
    for _ in 0..100_000 {
        // SAFETY: Reading CSTS register.
        let csts = unsafe { core::ptr::read_volatile((mmio_base + REG_CSTS as u64) as *const u32) };
        if (csts & CSTS_CFS) != 0 {
            fatal = true;
            break;
        }
        if (csts & CSTS_RDY) != 0 {
            ready = true;
            break;
        }
        core::hint::spin_loop();
    }

    let status_str = if ready { "READY (CSTS.RDY=1)" } else if fatal { "FATAL (CSTS.CFS=1)" } else { "TIMEOUT" };
    crate::early_println!(
        "[ostd::storage::nvme] NVMe Controller {:02x}:{:02x}.{} | NVMe v{}.{}.{} | Admin Queues={} (ASQ={:#x}, ACQ={:#x}) | Estado: {}",
        pci_dev.bus, pci_dev.device, pci_dev.function, ver_major, ver_minor, ver_tertiary, ADMIN_QUEUE_DEPTH, asq_paddr, acq_paddr, status_str
    );
    crate::fb_println!(
        "[ostd::storage::nvme] NVMe {:02x}:{:02x}.{} v{}.{}.{} Admin Q={} | {}",
        pci_dev.bus, pci_dev.device, pci_dev.function, ver_major, ver_minor, ver_tertiary, ADMIN_QUEUE_DEPTH, status_str
    );

    let telem = NvmeTelemetry {
        present: true,
        vendor_id: pci_dev.vendor_id,
        device_id: pci_dev.device_id,
        bus: pci_dev.bus,
        device: pci_dev.device,
        function: pci_dev.function,
        bar0_phys,
        version_major: ver_major,
        version_minor: ver_minor,
        version_tertiary: ver_tertiary,
        mqes,
        admin_q_depth: ADMIN_QUEUE_DEPTH,
        rdy: ready,
        cfs: fatal,
    };

    {
        let mut state = NVME_STATE.lock();
        state.pci_dev = Some(pci_dev);
        state.bar0_phys = bar0_phys;
        state.mmio_base = mmio_base;
        state.asq_frame = Some(asq_frame);
        state.acq_frame = Some(acq_frame);
        state.telemetry = telem;
    }

    NVME_INITIALIZED.store(true, Ordering::Release);
}

/// Takes a snapshot of NVMe hardware telemetry.
pub fn nvme_snapshot() -> NvmeTelemetry {
    NVME_STATE.lock().telemetry
}

/// Returns a human-readable summary string for sensors and HWMON reporting.
pub fn summary_text() -> alloc::string::String {
    let telem = nvme_snapshot();
    if !telem.present {
        return alloc::string::String::from("NO LINK (No detectado)");
    }
    alloc::format!(
        "LINK UP (PCIe {:02x}:{:02x}.{}, NVMe v{}.{}.{}, Queues={}, {})",
        telem.bus, telem.device, telem.function,
        telem.version_major, telem.version_minor, telem.version_tertiary,
        telem.admin_q_depth,
        if telem.rdy { "RDY=1" } else { "NOT READY" }
    )
}
