//! Bare-Metal PCIe NVMe Storage Driver (`ostd::storage::nvme`).
//!
//! Discovers PCIe Mass Storage NVMe controllers (Class `0x01`, Subclass `0x08`, ProgIF `0x02`),
//! maps `BAR0` MMIO registers, initializes the Admin Queue Pair (`ASQ`/`ACQ`), creates
//! I/O Completion/Submission Queue Pair 1 (`IOCQ`/`IOSQ`), and performs real DMA sector I/O.

use crate::bus::pcie::{probe_nvme_controller, PciDevice};
use crate::mm::dma::VmFrame;
use crate::mm::{map_kernel_mmio, VirtAddr};
use crate::sync::SpinLock;
use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};

/// MMIO aperture size for NVMe BAR0 (16 KiB).
pub const NVME_MMIO_SIZE: usize = 0x4000;

/// Admin and I/O queue entry counts (64 entries fit inside one 4 KiB frame each).
pub const ADMIN_QUEUE_DEPTH: u16 = 64;
pub const IO_QUEUE_DEPTH: u16 = 64;
pub const NVME_SECTOR_SIZE: usize = 512;
pub const NVME_MAX_CHUNK_SECTORS: usize = 4096 / NVME_SECTOR_SIZE;

// NVMe MMIO Register Offsets
const REG_CAP: usize = 0x00;  // Controller Capabilities (64-bit)
const REG_VS: usize = 0x08;   // Version (32-bit)
const REG_CC: usize = 0x14;   // Controller Configuration (32-bit)
const REG_CSTS: usize = 0x1C; // Controller Status (32-bit)
const REG_AQA: usize = 0x24;  // Admin Queue Attributes (32-bit)
const REG_ASQ: usize = 0x28;  // Admin Submission Queue Base Address (64-bit)
const REG_ACQ: usize = 0x30;  // Admin Completion Queue Base Address (64-bit)

// NVMe Admin & NVM Opcodes
const ADMIN_OPC_CREATE_IOSQ: u8 = 0x01;
const ADMIN_OPC_CREATE_IOCQ: u8 = 0x05;
const ADMIN_OPC_IDENTIFY: u8 = 0x06;
const NVM_OPC_WRITE: u8 = 0x01;
const NVM_OPC_READ: u8 = 0x02;

const NVME_CMD_TIMEOUT_MS: u64 = 1_500;

/// Architectural telemetry snapshot for the NVMe controller.
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
    pub io_queues_ready: bool,
    pub total_sectors: u64,
    pub sectors_read: u64,
    pub sectors_written: u64,
}

struct NvmeQueue {
    qid: u16,
    depth: u16,
    sq_frame: VmFrame,
    cq_frame: VmFrame,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: u8,
}

impl NvmeQueue {
    fn new(qid: u16, depth: u16) -> Option<Self> {
        let sq_frame = VmFrame::allocate().ok()?;
        let cq_frame = VmFrame::allocate().ok()?;
        // SAFETY: Frames are valid 4 KiB DMA regions owned exclusively by NvmeQueue.
        unsafe {
            core::ptr::write_bytes(sq_frame.vaddr() as *mut u8, 0, 4096);
            core::ptr::write_bytes(cq_frame.vaddr() as *mut u8, 0, 4096);
        }
        Some(Self {
            qid,
            depth,
            sq_frame,
            cq_frame,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: 1,
        })
    }
}

struct NvmeState {
    _pci_dev: PciDevice,
    mmio_base: VirtAddr,
    dstrd: u8,
    _admin_q: NvmeQueue,
    io_q: NvmeQueue,
    bounce_frame: VmFrame,
    next_cid: u16,
    telemetry: NvmeTelemetry,
}

static NVME_READY: AtomicBool = AtomicBool::new(false);
static TOTAL_SECTORS: AtomicU64 = AtomicU64::new(0);
static SECTORS_READ: AtomicU64 = AtomicU64::new(0);
static SECTORS_WRITTEN: AtomicU64 = AtomicU64::new(0);

static NVME_STATE: SpinLock<Option<NvmeState>> = SpinLock::new(None);

const EMPTY_TELEMETRY: NvmeTelemetry = NvmeTelemetry {
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
    io_queues_ready: false,
    total_sectors: 0,
    sectors_read: 0,
    sectors_written: 0,
};

#[inline]
unsafe fn mmio_read32(base: VirtAddr, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset as u64) as *const u32)
}

#[inline]
unsafe fn mmio_write32(base: VirtAddr, offset: usize, val: u32) {
    core::ptr::write_volatile((base + offset as u64) as *mut u32, val);
}

#[inline]
unsafe fn mmio_write64(base: VirtAddr, offset: usize, val: u64) {
    core::ptr::write_volatile((base + offset as u64) as *mut u32, val as u32);
    core::ptr::write_volatile((base + offset as u64 + 4) as *mut u32, (val >> 32) as u32);
}

#[inline]
unsafe fn write_doorbell(base: VirtAddr, dstrd: u8, qid: u16, is_cq: bool, val: u16) {
    let idx = (2usize * (qid as usize)) + if is_cq { 1 } else { 0 };
    let offset = 0x1000usize + idx * (4usize << dstrd);
    mmio_write32(base, offset, val as u32);
}

/// Submits a 64-byte NVMe Submission Queue Entry onto `queue` and polls for its CQE.
unsafe fn submit_cmd(
    mmio_base: VirtAddr,
    dstrd: u8,
    queue: &mut NvmeQueue,
    cid: u16,
    opcode: u8,
    nsid: u32,
    prp1: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
) -> Result<u32, &'static str> {
    let mut sqe = [0u32; 16];
    sqe[0] = (opcode as u32) | ((cid as u32) << 16);
    sqe[1] = nsid;
    sqe[6] = prp1 as u32;
    sqe[7] = (prp1 >> 32) as u32;
    sqe[10] = cdw10;
    sqe[11] = cdw11;
    sqe[12] = cdw12;

    let sq_entry_ptr = (queue.sq_frame.vaddr() + (queue.sq_tail as u64) * 64) as *mut u32;
    for (idx, word) in sqe.iter().enumerate() {
        core::ptr::write_volatile(sq_entry_ptr.add(idx), *word);
    }

    fence(Ordering::SeqCst);
    queue.sq_tail = (queue.sq_tail + 1) % queue.depth;
    write_doorbell(mmio_base, dstrd, queue.qid, false, queue.sq_tail);

    let cq_entry_ptr = (queue.cq_frame.vaddr() + (queue.cq_head as u64) * 16) as *const u32;
    let mut timed_out = true;
    let mut dw0 = 0u32;
    let mut dw3 = 0u32;

    let deadline = crate::time::tsc_deadline_ms(NVME_CMD_TIMEOUT_MS);
    while !crate::time::tsc_expired(deadline) {
        fence(Ordering::Acquire);
        dw3 = core::ptr::read_volatile(cq_entry_ptr.add(3));
        let phase = ((dw3 >> 16) & 1) as u8;
        if phase == queue.cq_phase {
            dw0 = core::ptr::read_volatile(cq_entry_ptr);
            timed_out = false;
            break;
        }
        core::hint::spin_loop();
    }

    if timed_out {
        return Err("Timeout esperando CQE de NVMe");
    }

    queue.cq_head += 1;
    if queue.cq_head >= queue.depth {
        queue.cq_head = 0;
        queue.cq_phase ^= 1;
    }
    write_doorbell(mmio_base, dstrd, queue.qid, true, queue.cq_head);

    let status_code = (dw3 >> 17) & 0x7FFF;
    if status_code != 0 {
        return Err("Error de estado en CQE de NVMe");
    }

    Ok(dw0)
}

/// Discovers, initializes, and brings up Admin + I/O Queue Pairs on the PCIe NVMe controller.
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

    let mmio_base = map_kernel_mmio(bar0_phys, NVME_MMIO_SIZE);

    // SAFETY: Accessing valid mapped NVMe BAR0 MMIO aperture.
    let (cap, vs) = unsafe {
        let cap_val = core::ptr::read_volatile((mmio_base + REG_CAP as u64) as *const u64);
        let vs_val = mmio_read32(mmio_base, REG_VS);
        (cap_val, vs_val)
    };

    let mqes = (cap & 0xFFFF) as u16 + 1;
    let dstrd = ((cap >> 32) & 0x0F) as u8;
    let ver_major = ((vs >> 16) & 0xFFFF) as u16;
    let ver_minor = ((vs >> 8) & 0xFF) as u8;
    let ver_tert = (vs & 0xFF) as u8;

    // 1. Disable controller and wait for CSTS.RDY == 0
    // SAFETY: Resetting CC.EN before programming ASQ/ACQ registers.
    unsafe {
        let cc = mmio_read32(mmio_base, REG_CC);
        if (cc & 1) != 0 {
            mmio_write32(mmio_base, REG_CC, cc & !1);
            let dis_dl = crate::time::tsc_deadline_ms(NVME_CMD_TIMEOUT_MS);
            while !crate::time::tsc_expired(dis_dl) {
                if (mmio_read32(mmio_base, REG_CSTS) & 1) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }

    // 2. Allocate Admin, I/O (QID=1), and 4 KiB data bounce DMA frames
    let mut admin_q = match NvmeQueue::new(0, ADMIN_QUEUE_DEPTH) {
        Some(q) => q,
        None => {
            crate::early_println!("[ostd::storage::nvme] Error: sin memoria fisica DMA para ASQ/ACQ.");
            return;
        }
    };
    let io_q = match NvmeQueue::new(1, IO_QUEUE_DEPTH) {
        Some(q) => q,
        None => {
            crate::early_println!("[ostd::storage::nvme] Error: sin memoria fisica DMA para IOSQ/IOCQ.");
            return;
        }
    };
    let bounce_frame = match VmFrame::allocate() {
        Ok(f) => f,
        Err(_) => {
            crate::early_println!("[ostd::storage::nvme] Error: sin memoria fisica DMA para buffer NVMe.");
            return;
        }
    };

    // 3. Configure AQA, ASQ, ACQ and enable controller (IOSQES=6 -> 64B, IOCQES=4 -> 16B)
    let aqa = (((ADMIN_QUEUE_DEPTH - 1) as u32) << 16) | ((ADMIN_QUEUE_DEPTH - 1) as u32);
    let asq_paddr = admin_q.sq_frame.paddr();
    let acq_paddr = admin_q.cq_frame.paddr();

    // SAFETY: Programming valid DMA physical addresses into ASQ/ACQ and setting CC.EN=1.
    let (rdy, cfs) = unsafe {
        mmio_write32(mmio_base, REG_AQA, aqa);
        mmio_write64(mmio_base, REG_ASQ, asq_paddr);
        mmio_write64(mmio_base, REG_ACQ, acq_paddr);

        let cc = 1u32 | (6u32 << 16) | (4u32 << 20);
        mmio_write32(mmio_base, REG_CC, cc);

        let mut is_rdy = false;
        let mut is_cfs = false;
        let en_dl = crate::time::tsc_deadline_ms(NVME_CMD_TIMEOUT_MS);
        while !crate::time::tsc_expired(en_dl) {
            let csts = mmio_read32(mmio_base, REG_CSTS);
            if (csts & 0x02) != 0 {
                is_cfs = true;
                break;
            }
            if (csts & 0x01) != 0 {
                is_rdy = true;
                break;
            }
            core::hint::spin_loop();
        }
        (is_rdy, is_cfs)
    };

    // 4. Create I/O Completion Queue (opcode 0x05), I/O Submission Queue (opcode 0x01) & Identify NSID 1
    let mut cid = 1u16;
    let mut io_queues_ready = false;
    let mut total_sectors = 0u64;
    if rdy && !cfs {
        let cq_cdw10 = (((IO_QUEUE_DEPTH - 1) as u32) << 16) | 1u32;
        let cq_cdw11 = 0x0000_0001u32; // Physically Contiguous (PC=1)
        let sq_cdw10 = (((IO_QUEUE_DEPTH - 1) as u32) << 16) | 1u32;
        let sq_cdw11 = (1u32 << 16) | 0x0000_0001u32; // CQID=1, PC=1

        // SAFETY: Controller is RDY=1 and I/O DMA frames are zeroed and mapped.
        let iocq_res = unsafe {
            submit_cmd(mmio_base, dstrd, &mut admin_q, cid, ADMIN_OPC_CREATE_IOCQ, 0, io_q.cq_frame.paddr(), cq_cdw10, cq_cdw11, 0)
        };
        cid = cid.wrapping_add(1);

        if iocq_res.is_ok() {
            // SAFETY: IOCQ QID=1 created successfully; now creating IOSQ QID=1.
            let iosq_res = unsafe {
                submit_cmd(mmio_base, dstrd, &mut admin_q, cid, ADMIN_OPC_CREATE_IOSQ, 0, io_q.sq_frame.paddr(), sq_cdw10, sq_cdw11, 0)
            };
            cid = cid.wrapping_add(1);
            if iosq_res.is_ok() {
                io_queues_ready = true;
                let _ = bounce_frame.write_bytes(0, &[0u8; 256]);
                // SAFETY: Admin Queue is active and bounce_frame is a valid 4 KiB DMA buffer.
                let id_res = unsafe {
                    submit_cmd(mmio_base, dstrd, &mut admin_q, cid, ADMIN_OPC_IDENTIFY, 1, bounce_frame.paddr(), 0, 0, 0)
                };
                if id_res.is_ok() {
                    let mut id_buf = [0u8; 256];
                    if bounce_frame.read_bytes(0, &mut id_buf).is_ok() {
                        let nsze = u64::from_le_bytes([
                            id_buf[0], id_buf[1], id_buf[2], id_buf[3],
                            id_buf[4], id_buf[5], id_buf[6], id_buf[7],
                        ]);
                        let flbas = (id_buf[26] & 0x0F) as usize;
                        let lbads = id_buf[128 + flbas * 16 + 2];
                        let mult = if (9..=16).contains(&lbads) { 1u64 << (lbads - 9) } else { 1 };
                        total_sectors = nsze.saturating_mul(mult);
                    }
                }
                cid = cid.wrapping_add(1);
            }
        }
    }

    TOTAL_SECTORS.store(total_sectors, Ordering::Release);
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
        version_tertiary: ver_tert,
        mqes,
        admin_q_depth: ADMIN_QUEUE_DEPTH,
        rdy,
        cfs,
        io_queues_ready,
        total_sectors,
        sectors_read: 0,
        sectors_written: 0,
    };

    *NVME_STATE.lock() = Some(NvmeState {
        _pci_dev: pci_dev,
        mmio_base,
        dstrd,
        _admin_q: admin_q,
        io_q,
        bounce_frame,
        next_cid: cid.max(1),
        telemetry: telem,
    });

    NVME_READY.store(io_queues_ready, Ordering::Release);

    let cap_mib = (total_sectors * 512) / (1024 * 1024);
    crate::early_println!(
        "[ostd::storage::nvme] NVMe v{}.{}.{} | CSTS.RDY={} IO_QUEUES={} Cap={} MiB ({} sect) (MQES={}, ASQ={:#x}, ACQ={:#x})",
        ver_major, ver_minor, ver_tert, rdy as u8, io_queues_ready as u8, cap_mib, total_sectors, mqes, asq_paddr, acq_paddr
    );
    crate::fb_println!(
        "[NVMe] Controladora v{}.{}.{} RDY={} | {} MiB | DMA I/O Queues={}",
        ver_major, ver_minor, ver_tert, rdy as u8, cap_mib, if io_queues_ready { "OK(QID=1)" } else { "OFF" }
    );
}

/// Returns total addressable 512-byte sectors reported by NVMe Namespace 1 (`0` if unavailable).
pub fn total_sectors() -> u64 {
    TOTAL_SECTORS.load(Ordering::Acquire)
}

/// Returns `true` if the NVMe controller has active Admin + I/O queues ready for block DMA.
pub fn is_nvme_ready() -> bool {
    NVME_READY.load(Ordering::Acquire)
}

/// Executes an NVM Read (`0x02`) or Write (`0x01`) for up to `NVME_MAX_CHUNK_SECTORS` sectors.
fn transfer_sectors_internal(
    lba: u64,
    sector_count: usize,
    mut read_dst: Option<&mut [u8]>,
    write_src: Option<&[u8]>,
) -> Result<(), &'static str> {
    if sector_count == 0 {
        return Ok(());
    }
    let required_bytes = sector_count
        .checked_mul(NVME_SECTOR_SIZE)
        .ok_or("Overflow de sectores NVMe")?;
    let is_write = write_src.is_some();

    if let Some(src) = write_src {
        if src.len() < required_bytes {
            return Err("Buffer origen insuficiente para escritura NVMe");
        }
    }
    if let Some(dst) = read_dst.as_ref() {
        if dst.len() < required_bytes {
            return Err("Buffer destino insuficiente para lectura NVMe");
        }
    }

    let mut guard = NVME_STATE.lock();
    let state = guard.as_mut().ok_or("Controladora NVMe no inicializada")?;
    if !state.telemetry.io_queues_ready {
        return Err("Colas NVMe I/O (IOSQ/IOCQ) no activas");
    }
    if state.telemetry.total_sectors > 0
        && lba.saturating_add(sector_count as u64) > state.telemetry.total_sectors
    {
        return Err("LBA NVMe fuera de rango");
    }

    let mut cur_lba = lba;
    let mut offset = 0usize;
    let mut remaining = sector_count;

    while remaining > 0 {
        let chunk_sectors = remaining.min(NVME_MAX_CHUNK_SECTORS);
        let chunk_bytes = chunk_sectors * NVME_SECTOR_SIZE;

        if let Some(src) = write_src {
            state
                .bounce_frame
                .write_bytes(0, &src[offset..offset + chunk_bytes])
                .map_err(|_| "Fallo al copiar datos hacia buffer DMA NVMe")?;
        }

        let cid = state.next_cid;
        state.next_cid = state.next_cid.wrapping_add(1).max(1);
        let opc = if is_write { NVM_OPC_WRITE } else { NVM_OPC_READ };
        let cdw10 = cur_lba as u32;
        let cdw11 = (cur_lba >> 32) as u32;
        let cdw12 = (chunk_sectors as u32) - 1;

        // SAFETY: io_q and bounce_frame are valid 4 KiB DMA frames mapped in controller BAR0.
        unsafe {
            submit_cmd(
                state.mmio_base,
                state.dstrd,
                &mut state.io_q,
                cid,
                opc,
                1, // NSID 1
                state.bounce_frame.paddr(),
                cdw10,
                cdw11,
                cdw12,
            )?;
        }

        if let Some(dst) = read_dst.as_deref_mut() {
            state
                .bounce_frame
                .read_bytes(0, &mut dst[offset..offset + chunk_bytes])
                .map_err(|_| "Fallo al copiar datos desde buffer DMA NVMe")?;
        }

        cur_lba += chunk_sectors as u64;
        offset += chunk_bytes;
        remaining -= chunk_sectors;
    }

    if is_write {
        SECTORS_WRITTEN.fetch_add(sector_count as u64, Ordering::Relaxed);
    } else {
        SECTORS_READ.fetch_add(sector_count as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// Reads `sector_count` 512-byte sectors from NVMe Namespace 1 starting at `lba`.
pub fn read_sectors(lba: u64, sector_count: usize, dst: &mut [u8]) -> Result<(), &'static str> {
    transfer_sectors_internal(lba, sector_count, Some(dst), None)
}

/// Writes `sector_count` 512-byte sectors to NVMe Namespace 1 starting at `lba`.
pub fn write_sectors(lba: u64, sector_count: usize, src: &[u8]) -> Result<(), &'static str> {
    transfer_sectors_internal(lba, sector_count, None, Some(src))
}

/// Reads a single 512-byte sector from NVMe Namespace 1 at `lba`.
pub fn read_sector(lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < NVME_SECTOR_SIZE {
        return Err("Buffer < 512 bytes para read_sector NVMe");
    }
    read_sectors(lba, 1, &mut buf[..NVME_SECTOR_SIZE])
}

/// Writes a single 512-byte sector to NVMe Namespace 1 at `lba`.
pub fn write_sector(lba: u64, buf: &[u8]) -> Result<(), &'static str> {
    if buf.len() < NVME_SECTOR_SIZE {
        return Err("Buffer < 512 bytes para write_sector NVMe");
    }
    write_sectors(lba, 1, &buf[..NVME_SECTOR_SIZE])
}

/// Returns a thread-safe snapshot of the NVMe controller telemetry.
pub fn nvme_snapshot() -> NvmeTelemetry {
    let guard = NVME_STATE.lock();
    if let Some(ref st) = *guard {
        let mut telem = st.telemetry;
        telem.sectors_read = SECTORS_READ.load(Ordering::Relaxed);
        telem.sectors_written = SECTORS_WRITTEN.load(Ordering::Relaxed);
        telem
    } else {
        EMPTY_TELEMETRY
    }
}

/// Formats a compact status string for `/proc/hwmon` and diagnostics.
pub fn summary_text() -> alloc::string::String {
    let telem = nvme_snapshot();
    if !telem.present {
        return alloc::string::String::from("NO LINK (No detectado)");
    }
    alloc::format!(
        "LINK UP (PCIe {:02x}:{:02x}.{}, NVMe v{}.{}.{}, {}, I/O={}, R={} W={})",
        telem.bus,
        telem.device,
        telem.function,
        telem.version_major,
        telem.version_minor,
        telem.version_tertiary,
        if telem.rdy { "RDY=1" } else { "NOT READY" },
        if telem.io_queues_ready { "ON" } else { "OFF" },
        telem.sectors_read,
        telem.sectors_written
    )
}
