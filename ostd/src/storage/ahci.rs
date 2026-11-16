//! Intel ICH9 / Standard SATA AHCI (`00:1f.2`, `Class 0x01:0x06`) Hardware Driver
//! with Contiguous Physical DMA Command List, Received FIS, and PRDT Table (`ostd::storage::ahci`).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::bus::pcie::{probe_ahci_controller, PciDevice};
use crate::mm::dma::{DmaBuffer, VmFrame};
use crate::mm::{map_kernel_mmio, VirtAddr};
use crate::sync::SpinLock;

/// Size of the AHCI ABAR MMIO aperture mapped in Ring 0 (16 KiB = supports up to 32 ports).
pub const AHCI_MMIO_SIZE: usize = 16 * 1024;

/// Bounce buffer size for DMA sector transfers (16 KiB = 32 sectors of 512 bytes).
pub const AHCI_DMA_BUF_SIZE: usize = 16 * 1024;
pub const AHCI_MAX_CHUNK_SECTORS: usize = AHCI_DMA_BUF_SIZE / 512;

// AHCI Generic Host Control Register Offsets (relative to ABAR)
#[allow(dead_code)]
const REG_CAP: usize = 0x00;
const REG_GHC: usize = 0x04;
#[allow(dead_code)]
const REG_IS: usize = 0x08;
const REG_PI: usize = 0x0C;
#[allow(dead_code)]
const REG_VS: usize = 0x10;

// GHC bit definitions
#[allow(dead_code)]
const GHC_HR: u32 = 1 << 0;
#[allow(dead_code)]
const GHC_IE: u32 = 1 << 1;
const GHC_AE: u32 = 1 << 31; // AHCI Enable

// Port Register Offsets (relative to Port MMIO = ABAR + 0x100 + port * 0x80)
const PORT_REG_CLB: usize = 0x00;
const PORT_REG_CLBU: usize = 0x04;
const PORT_REG_FB: usize = 0x08;
const PORT_REG_FBU: usize = 0x0C;
const PORT_REG_IS: usize = 0x10;
#[allow(dead_code)]
const PORT_REG_IE: usize = 0x14;
const PORT_REG_CMD: usize = 0x18;
const PORT_REG_TFD: usize = 0x20;
const PORT_REG_SIG: usize = 0x24;
const PORT_REG_SSTS: usize = 0x28;
#[allow(dead_code)]
const PORT_REG_SCTL: usize = 0x2C;
const PORT_REG_SERR: usize = 0x30;
#[allow(dead_code)]
const PORT_REG_SACT: usize = 0x34;
const PORT_REG_CI: usize = 0x38;

// Port CMD Register Bits
const PORT_CMD_ST: u32 = 1 << 0; // Start
const PORT_CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const PORT_CMD_FR: u32 = 1 << 14; // FIS Receive Running
const PORT_CMD_CR: u32 = 1 << 15; // Command List Running

// Port TFD Register Status Bits
const ATA_DEV_BUSY: u32 = 1 << 7;
const ATA_DEV_DF: u32 = 1 << 5;
const ATA_DEV_DRQ: u32 = 1 << 3;
const ATA_DEV_ERR: u32 = 1 << 0;

// ATA Commands & AHCI Signatures
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_IDENTIFY_DEVICE: u8 = 0xEC;
const FIS_TYPE_REG_H2D: u8 = 0x27;
const AHCI_SIG_ATA: u32 = 0x0000_0101;
const AHCI_SIG_ATAPI: u32 = 0xEB14_0101;
const PORT_IS_TFES: u32 = 1 << 30;
const AHCI_CMD_TIMEOUT_MS: u64 = 1_500;

static AHCI_READY: AtomicBool = AtomicBool::new(false);
static SECTORS_READ: AtomicU64 = AtomicU64::new(0);
static SECTORS_WRITTEN: AtomicU64 = AtomicU64::new(0);
static BYTES_TRANSFERRED: AtomicU64 = AtomicU64::new(0);

/// Hardware snapshot of the PCIe SATA AHCI controller and its DMA counters.
#[derive(Debug, Clone, Copy)]
pub struct AhciHardwareSnapshot {
    pub present: bool,
    pub pci_bus: u8,
    pub pci_dev: u8,
    pub pci_fn: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub pci_cmd: u16,
    pub port_idx: u8,
    pub sectors_read: u64,
    pub sectors_written: u64,
    pub bytes_transferred: u64,
}

#[allow(dead_code)]
struct AhciDriverState {
    pci_dev: PciDevice,
    pci_cmd: u16,
    bar5_phys: u64,
    abar_mmio: VirtAddr,
    active_port: u8,
    port_mmio: VirtAddr,
    total_sectors: u64,
    dma_structs_frame: VmFrame,
    bounce_buffer: DmaBuffer,
}

static AHCI_STATE: SpinLock<Option<AhciDriverState>> = SpinLock::new(None);

/// Probes and initializes the Intel ICH9 / standard PCIe SATA AHCI controller (`00:1f.2`).
pub fn init() {
    let Some((dev, bar5_phys, pci_cmd)) = probe_ahci_controller() else {
        crate::early_println!("[ostd::storage::ahci] No se detecto controladora SATA AHCI en el bus PCIe.");
        crate::fb_println!("[ostd::storage::ahci] No se detecto controladora SATA AHCI en el bus PCIe.");
        return;
    };

    let abar_mmio = map_kernel_mmio(bar5_phys, AHCI_MMIO_SIZE);

    // SAFETY: Accessing mapped ABAR MMIO registers in Ring 0.
    unsafe {
        // 1. Enable AHCI mode in GHC (Global Host Control)
        let ghc_ptr = (abar_mmio + REG_GHC as u64) as *mut u32;
        let ghc = core::ptr::read_volatile(ghc_ptr);
        core::ptr::write_volatile(ghc_ptr, ghc | GHC_AE);

        // 2. Read PI (Ports Implemented)
        let pi_ptr = (abar_mmio + REG_PI as u64) as *const u32;
        let pi = core::ptr::read_volatile(pi_ptr);

        let dma_structs_frame = VmFrame::allocate()
            .expect("Fallo al reservar marco fisico DMA para estructuras AHCI");
        let bounce_buffer = DmaBuffer::allocate(AHCI_DMA_BUF_SIZE)
            .expect("Fallo al reservar DmaBuffer de 16 KiB para transferencias SATA AHCI");
        let frame_paddr = dma_structs_frame.paddr();
        let frame_vaddr = dma_structs_frame.vaddr();
        let fis_paddr = frame_paddr + 1024;

        // 3. Scan ports for a connected ATA disk (DET=3, IPM=1) and verify via IDENTIFY DEVICE (0xEC)
        let mut active_info: Option<(u8, VirtAddr, u32, u32, u64)> = None;
        for p in 0..32u8 {
            if (pi & (1u32 << p)) == 0 {
                continue;
            }
            let port_mmio = abar_mmio + 0x100 + (p as u64) * 0x80;
            let ssts = core::ptr::read_volatile((port_mmio + PORT_REG_SSTS as u64) as *const u32);
            let det = ssts & 0x0F;
            let ipm = (ssts >> 8) & 0x0F;
            let sig = core::ptr::read_volatile((port_mmio + PORT_REG_SIG as u64) as *const u32);

            if det == 3 && ipm == 1 && sig == AHCI_SIG_ATAPI {
                crate::early_println!(
                    "[ostd::storage::ahci] Puerto {} es unidad optica SATAPI (SIG={:#x}), omitido.",
                    p, sig
                );
                continue;
            }
            if det != 3 || ipm != 1 || sig != AHCI_SIG_ATA {
                continue;
            }

            stop_port(port_mmio);
            core::ptr::write_bytes(frame_vaddr as *mut u8, 0, 4096);
            core::ptr::write_volatile((port_mmio + PORT_REG_CLB as u64) as *mut u32, frame_paddr as u32);
            core::ptr::write_volatile((port_mmio + PORT_REG_CLBU as u64) as *mut u32, (frame_paddr >> 32) as u32);
            core::ptr::write_volatile((port_mmio + PORT_REG_FB as u64) as *mut u32, fis_paddr as u32);
            core::ptr::write_volatile((port_mmio + PORT_REG_FBU as u64) as *mut u32, (fis_paddr >> 32) as u32);
            core::ptr::write_volatile((port_mmio + PORT_REG_SERR as u64) as *mut u32, 0xFFFF_FFFF);
            core::ptr::write_volatile((port_mmio + PORT_REG_IS as u64) as *mut u32, 0xFFFF_FFFF);
            start_port(port_mmio);

            match identify_ata_device(port_mmio, &dma_structs_frame, &bounce_buffer) {
                Ok(sectors) => {
                    active_info = Some((p, port_mmio, ssts, sig, sectors));
                    break;
                }
                Err(err) => {
                    crate::early_println!("[ostd::storage::ahci] Puerto {} fallo IDENTIFY ({}), omitido.", p, err);
                    stop_port(port_mmio);
                }
            }
        }

        let Some((active_port, port_mmio, ssts, sig, total_sectors)) = active_info else {
            crate::early_println!(
                "[ostd::storage::ahci] AHCI 00:1f.2 activo pero ningun puerto SATA tiene disco valido (PI={:#x}).",
                pi
            );
            crate::fb_println!("[ostd::storage::ahci] AHCI 00:1f.2 activo (PI={:#x}, sin disco SATA).", pi);
            return;
        };

        let mib = (total_sectors.saturating_mul(512)) / (1024 * 1024);
        crate::early_println!(
            "[ostd::storage::ahci] SATA AHCI {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) ABAR={:#x} -> MMIO={:#x} | Puerto {} (SSTS={:#x}, SIG={:#x}, {} sectores / {} MiB) | DMA CmdList={:#x} Buf={:#x}",
            dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id, pci_cmd,
            bar5_phys, abar_mmio, active_port, ssts, sig, total_sectors, mib, frame_paddr, bounce_buffer.paddr()
        );
        crate::fb_println!(
            "[ostd::storage::ahci] SATA AHCI 00:1f.2 (Port {}) {} MiB ({} sect) | Link=UP | Buf={:#x}",
            active_port, mib, total_sectors, bounce_buffer.paddr()
        );

        *AHCI_STATE.lock() = Some(AhciDriverState {
            pci_dev: dev,
            pci_cmd,
            bar5_phys,
            abar_mmio,
            active_port,
            port_mmio,
            total_sectors,
            dma_structs_frame,
            bounce_buffer,
        });

        AHCI_READY.store(true, Ordering::Release);
    }
}

/// Returns `true` if the SATA AHCI controller is discovered, initialized, and ready for I/O.
pub fn is_ahci_ready() -> bool {
    AHCI_READY.load(Ordering::Acquire)
}

/// Returns total addressable 512-byte sectors reported by ATA `IDENTIFY DEVICE`.
pub fn total_sectors() -> u64 {
    AHCI_STATE.lock().as_ref().map_or(0, |s| s.total_sectors)
}

/// Returns an architectural snapshot of the SATA AHCI controller and its DMA transfer counters.
pub fn ahci_snapshot() -> AhciHardwareSnapshot {
    let guard = AHCI_STATE.lock();
    if let Some(ref st) = *guard {
        AhciHardwareSnapshot {
            present: true,
            pci_bus: st.pci_dev.bus,
            pci_dev: st.pci_dev.device,
            pci_fn: st.pci_dev.function,
            vendor_id: st.pci_dev.vendor_id,
            device_id: st.pci_dev.device_id,
            pci_cmd: st.pci_cmd,
            port_idx: st.active_port,
            sectors_read: SECTORS_READ.load(Ordering::Relaxed),
            sectors_written: SECTORS_WRITTEN.load(Ordering::Relaxed),
            bytes_transferred: BYTES_TRANSFERRED.load(Ordering::Relaxed),
        }
    } else {
        AhciHardwareSnapshot {
            present: false,
            pci_bus: 0,
            pci_dev: 0,
            pci_fn: 0,
            vendor_id: 0,
            device_id: 0,
            pci_cmd: 0,
            port_idx: 0,
            sectors_read: 0,
            sectors_written: 0,
            bytes_transferred: 0,
        }
    }
}

/// Reads `sector_count` 512-byte sectors starting at `lba` from the SATA disk into `dst`.
pub fn read_sectors(lba: u64, sector_count: usize, dst: &mut [u8]) -> Result<(), &'static str> {
    if sector_count == 0 {
        return Ok(());
    }
    let required_bytes = sector_count
        .checked_mul(512)
        .ok_or("Overflow en tamano de lectura de sectores")?;
    if dst.len() < required_bytes {
        return Err("Buffer destino insuficiente para sectores solicitados");
    }

    let mut guard = AHCI_STATE.lock();
    let state = guard.as_mut().ok_or("Controlador SATA AHCI no disponible")?;
    if state.total_sectors == 0
        || lba >= state.total_sectors
        || lba.checked_add(sector_count as u64).map_or(true, |end| end > state.total_sectors)
    {
        return Err("SATA AHCI: LBA fuera de rango (>= total_sectors)");
    }

    let mut current_lba = lba;
    let mut offset = 0usize;
    let mut remaining = sector_count;

    while remaining > 0 {
        let chunk_sectors = remaining.min(AHCI_MAX_CHUNK_SECTORS);
        let chunk_bytes = chunk_sectors * 512;

        // SAFETY: Executing ATA_CMD_READ_DMA_EXT via configured AHCI DMA rings.
        unsafe {
            execute_slot0_command(
                state.port_mmio,
                &state.dma_structs_frame,
                &state.bounce_buffer,
                ATA_CMD_READ_DMA_EXT,
                current_lba,
                chunk_sectors as u16,
                chunk_bytes as u32,
                false,
                1 << 6,
            )?;
        }

        state
            .bounce_buffer
            .read_bytes(0, &mut dst[offset..offset + chunk_bytes])
            .map_err(|_| "Fallo al leer del buffer DMA AHCI")?;

        current_lba += chunk_sectors as u64;
        offset += chunk_bytes;
        remaining -= chunk_sectors;
    }

    SECTORS_READ.fetch_add(sector_count as u64, Ordering::Relaxed);
    BYTES_TRANSFERRED.fetch_add(required_bytes as u64, Ordering::Relaxed);
    Ok(())
}

/// Writes `sector_count` 512-byte sectors from `src` to the SATA disk starting at `lba`.
pub fn write_sectors(lba: u64, sector_count: usize, src: &[u8]) -> Result<(), &'static str> {
    if sector_count == 0 {
        return Ok(());
    }
    let required_bytes = sector_count
        .checked_mul(512)
        .ok_or("Overflow en tamano de escritura de sectores")?;
    if src.len() < required_bytes {
        return Err("Buffer origen insuficiente para sectores a escribir");
    }

    let mut guard = AHCI_STATE.lock();
    let state = guard.as_mut().ok_or("Controlador SATA AHCI no disponible")?;
    if state.total_sectors == 0
        || lba >= state.total_sectors
        || lba.checked_add(sector_count as u64).map_or(true, |end| end > state.total_sectors)
    {
        return Err("SATA AHCI: LBA de escritura fuera de rango (>= total_sectors)");
    }

    let mut current_lba = lba;
    let mut offset = 0usize;
    let mut remaining = sector_count;

    while remaining > 0 {
        let chunk_sectors = remaining.min(AHCI_MAX_CHUNK_SECTORS);
        let chunk_bytes = chunk_sectors * 512;

        state
            .bounce_buffer
            .write_bytes(0, &src[offset..offset + chunk_bytes])
            .map_err(|_| "Fallo al escribir en buffer DMA AHCI")?;

        // SAFETY: Executing ATA_CMD_WRITE_DMA_EXT via configured AHCI DMA rings.
        unsafe {
            execute_slot0_command(
                state.port_mmio,
                &state.dma_structs_frame,
                &state.bounce_buffer,
                ATA_CMD_WRITE_DMA_EXT,
                current_lba,
                chunk_sectors as u16,
                chunk_bytes as u32,
                true,
                1 << 6,
            )?;
        }

        current_lba += chunk_sectors as u64;
        offset += chunk_bytes;
        remaining -= chunk_sectors;
    }

    SECTORS_WRITTEN.fetch_add(sector_count as u64, Ordering::Relaxed);
    BYTES_TRANSFERRED.fetch_add(required_bytes as u64, Ordering::Relaxed);
    Ok(())
}

unsafe fn stop_port(port_mmio: VirtAddr) {
    let cmd_ptr = (port_mmio + PORT_REG_CMD as u64) as *mut u32;
    let mut cmd = core::ptr::read_volatile(cmd_ptr);
    cmd &= !(PORT_CMD_ST | PORT_CMD_FRE);
    core::ptr::write_volatile(cmd_ptr, cmd);

    let deadline = crate::time::tsc_deadline_ms(AHCI_CMD_TIMEOUT_MS);
    while !crate::time::tsc_expired(deadline) {
        let cur = core::ptr::read_volatile(cmd_ptr);
        if (cur & (PORT_CMD_CR | PORT_CMD_FR)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

unsafe fn start_port(port_mmio: VirtAddr) {
    let cmd_ptr = (port_mmio + PORT_REG_CMD as u64) as *mut u32;
    let deadline = crate::time::tsc_deadline_ms(AHCI_CMD_TIMEOUT_MS);
    while !crate::time::tsc_expired(deadline) {
        let cur = core::ptr::read_volatile(cmd_ptr);
        if (cur & PORT_CMD_CR) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let cur = core::ptr::read_volatile(cmd_ptr);
    core::ptr::write_volatile(cmd_ptr, cur | PORT_CMD_FRE);
    let cur2 = core::ptr::read_volatile(cmd_ptr);
    core::ptr::write_volatile(cmd_ptr, cur2 | PORT_CMD_ST);
}

unsafe fn identify_ata_device(
    port_mmio: VirtAddr,
    dma_structs_frame: &VmFrame,
    bounce_buffer: &DmaBuffer,
) -> Result<u64, &'static str> {
    let mut id_buf = [0u8; 512];
    let _ = bounce_buffer.write_bytes(0, &id_buf);
    execute_slot0_command(
        port_mmio,
        dma_structs_frame,
        bounce_buffer,
        ATA_CMD_IDENTIFY_DEVICE,
        0,
        0,
        512,
        false,
        0,
    )?;
    bounce_buffer
        .read_bytes(0, &mut id_buf)
        .map_err(|_| "SATA AHCI: Fallo al leer buffer IDENTIFY")?;

    let word83 = u16::from_le_bytes([id_buf[166], id_buf[167]]);
    let lba48 = if (word83 & (1 << 10)) != 0 {
        u64::from_le_bytes([
            id_buf[200], id_buf[201], id_buf[202], id_buf[203],
            id_buf[204], id_buf[205], id_buf[206], id_buf[207],
        ])
    } else {
        0
    };
    let lba28 = u32::from_le_bytes([id_buf[120], id_buf[121], id_buf[122], id_buf[123]]) as u64;
    let total = if lba48 > 0 { lba48 } else { lba28 };
    if total == 0 {
        return Err("SATA AHCI: IDENTIFY DEVICE devolvio 0 sectores");
    }
    Ok(total)
}

/// Executes an ATA command (`IDENTIFY DEVICE` `0xEC`, `READ_DMA_EXT` `0x25`, `WRITE_DMA_EXT` `0x35`)
/// on slot 0 of the active AHCI port with real 1500 ms TSC timeouts.
#[allow(clippy::too_many_arguments)]
unsafe fn execute_slot0_command(
    port_mmio: VirtAddr,
    dma_structs_frame: &VmFrame,
    bounce_buffer: &DmaBuffer,
    cmd_code: u8,
    lba: u64,
    count: u16,
    transfer_bytes: u32,
    is_write: bool,
    device_reg: u8,
) -> Result<(), &'static str> {
    let tfd_ptr = (port_mmio + PORT_REG_TFD as u64) as *const u32;

    let busy_deadline = crate::time::tsc_deadline_ms(AHCI_CMD_TIMEOUT_MS);
    loop {
        let tfd = core::ptr::read_volatile(tfd_ptr);
        if (tfd & (ATA_DEV_BUSY | ATA_DEV_DRQ)) == 0 {
            break;
        }
        if crate::time::tsc_expired(busy_deadline) {
            return Err("SATA AHCI: Dispositivo ocupado (BSY/DRQ timeout 1500 ms)");
        }
        core::hint::spin_loop();
    }

    let structs_vaddr = dma_structs_frame.vaddr();
    let structs_paddr = dma_structs_frame.paddr();

    let cmd_header_ptr = structs_vaddr as *mut u32;
    let write_bit = if is_write { 1u32 << 6 } else { 0u32 };
    let prdtl = 1u32 << 16;
    core::ptr::write_volatile(cmd_header_ptr.add(0), 5u32 | write_bit | prdtl);
    core::ptr::write_volatile(cmd_header_ptr.add(1), 0);

    let ctba_paddr = structs_paddr + 2048;
    core::ptr::write_volatile(cmd_header_ptr.add(2), ctba_paddr as u32);
    core::ptr::write_volatile(cmd_header_ptr.add(3), (ctba_paddr >> 32) as u32);
    for j in 4..8 {
        core::ptr::write_volatile(cmd_header_ptr.add(j), 0);
    }

    let ct_vaddr = structs_vaddr + 2048;
    let cfis_ptr = ct_vaddr as *mut u8;
    core::ptr::write_bytes(cfis_ptr, 0, 64);

    core::ptr::write_volatile(cfis_ptr.add(0), FIS_TYPE_REG_H2D);
    core::ptr::write_volatile(cfis_ptr.add(1), 0x80);
    core::ptr::write_volatile(cfis_ptr.add(2), cmd_code);
    core::ptr::write_volatile(cfis_ptr.add(4), (lba & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(5), ((lba >> 8) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(6), ((lba >> 16) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(7), device_reg);
    core::ptr::write_volatile(cfis_ptr.add(8), ((lba >> 24) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(9), ((lba >> 32) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(10), ((lba >> 40) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(12), (count & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(13), ((count >> 8) & 0xFF) as u8);

    let prdt_ptr = (ct_vaddr + 0x80) as *mut u32;
    let dma_buf_paddr = bounce_buffer.paddr();
    core::ptr::write_volatile(prdt_ptr.add(0), dma_buf_paddr as u32);
    core::ptr::write_volatile(prdt_ptr.add(1), (dma_buf_paddr >> 32) as u32);
    core::ptr::write_volatile(prdt_ptr.add(2), 0);
    core::ptr::write_volatile(prdt_ptr.add(3), (transfer_bytes.max(1) - 1) | (1u32 << 31));

    let serr_ptr = (port_mmio + PORT_REG_SERR as u64) as *mut u32;
    core::ptr::write_volatile(serr_ptr, 0xFFFF_FFFF);
    let is_ptr = (port_mmio + PORT_REG_IS as u64) as *mut u32;
    core::ptr::write_volatile(is_ptr, 0xFFFF_FFFF);

    let ci_ptr = (port_mmio + PORT_REG_CI as u64) as *mut u32;
    core::ptr::write_volatile(ci_ptr, 1u32 << 0);

    let cmd_deadline = crate::time::tsc_deadline_ms(AHCI_CMD_TIMEOUT_MS);
    while (core::ptr::read_volatile(ci_ptr) & 1) != 0 {
        let is = core::ptr::read_volatile(is_ptr);
        if (is & PORT_IS_TFES) != 0 {
            return Err("SATA AHCI: Error en descriptor de tarea (PxIS.TFES)");
        }
        if crate::time::tsc_expired(cmd_deadline) {
            return Err("SATA AHCI: Timeout real (1500 ms) en operacion SATA");
        }
        core::hint::spin_loop();
    }

    if (core::ptr::read_volatile(is_ptr) & PORT_IS_TFES) != 0 {
        return Err("SATA AHCI: Error reportado en PxIS.TFES tras comando SATA");
    }
    let tfd = core::ptr::read_volatile(tfd_ptr);
    if (tfd & (ATA_DEV_ERR | ATA_DEV_DF)) != 0 {
        return Err("SATA AHCI: Error de disco en PxTFD (ERR/DF)");
    }

    Ok(())
}
