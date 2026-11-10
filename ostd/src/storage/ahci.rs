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
const ATA_DEV_DRQ: u32 = 1 << 3;
const ATA_DEV_ERR: u32 = 1 << 0;

// ATA Commands
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const FIS_TYPE_REG_H2D: u8 = 0x27;

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

        // 3. Scan ports for a connected SATA drive (DET=3, IPM=1)
        let mut target_port: Option<(u8, VirtAddr, u32, u32)> = None;
        for p in 0..32u8 {
            if (pi & (1u32 << p)) == 0 {
                continue;
            }
            let port_mmio = abar_mmio + 0x100 + (p as u64) * 0x80;
            let ssts = core::ptr::read_volatile((port_mmio + PORT_REG_SSTS as u64) as *const u32);
            let det = ssts & 0x0F;
            let ipm = (ssts >> 8) & 0x0F;
            let sig = core::ptr::read_volatile((port_mmio + PORT_REG_SIG as u64) as *const u32);

            if det == 3 && ipm == 1 {
                target_port = Some((p, port_mmio, ssts, sig));
                break;
            }
        }

        let Some((active_port, port_mmio, ssts, sig)) = target_port else {
            crate::early_println!(
                "[ostd::storage::ahci] AHCI 00:1f.2 activo pero ningun puerto SATA tiene disco conectado (PI={:#x}).",
                pi
            );
            crate::fb_println!(
                "[ostd::storage::ahci] AHCI 00:1f.2 activo (PI={:#x}, sin disco SATA).",
                pi
            );
            return;
        };

        // 4. Stop port command processing before programming DMA structures
        stop_port(port_mmio);

        // 5. Allocate 1 clean 4 KiB frame for Command List, Received FIS, and Command Table
        let dma_structs_frame = VmFrame::allocate()
            .expect("Fallo al reservar marco fisico DMA para estructuras AHCI");
        let frame_paddr = dma_structs_frame.paddr();
        let frame_vaddr = dma_structs_frame.vaddr();

        // Zero out the entire 4 KiB structures frame
        core::ptr::write_bytes(frame_vaddr as *mut u8, 0, 4096);

        // Command List Base Address (CLB at offset 0, 1 KiB aligned)
        core::ptr::write_volatile(
            (port_mmio + PORT_REG_CLB as u64) as *mut u32,
            (frame_paddr & 0xFFFF_FFFF) as u32,
        );
        core::ptr::write_volatile(
            (port_mmio + PORT_REG_CLBU as u64) as *mut u32,
            ((frame_paddr >> 32) & 0xFFFF_FFFF) as u32,
        );

        // Received FIS Base Address (FB at offset 1024, 256 B aligned)
        let fis_paddr = frame_paddr + 1024;
        core::ptr::write_volatile(
            (port_mmio + PORT_REG_FB as u64) as *mut u32,
            (fis_paddr & 0xFFFF_FFFF) as u32,
        );
        core::ptr::write_volatile(
            (port_mmio + PORT_REG_FBU as u64) as *mut u32,
            ((fis_paddr >> 32) & 0xFFFF_FFFF) as u32,
        );

        // Clear pending interrupt and error bits (RW1C)
        core::ptr::write_volatile((port_mmio + PORT_REG_SERR as u64) as *mut u32, 0xFFFF_FFFF);
        core::ptr::write_volatile((port_mmio + PORT_REG_IS as u64) as *mut u32, 0xFFFF_FFFF);

        // 6. Allocate contiguous 16 KiB bounce DMA buffer (32 sectors) for sector transfers
        let bounce_buffer = DmaBuffer::allocate(AHCI_DMA_BUF_SIZE)
            .expect("Fallo al reservar DmaBuffer de 16 KiB para transferencias SATA AHCI");

        // 7. Restart port command processing
        start_port(port_mmio);

        crate::early_println!(
            "[ostd::storage::ahci] SATA AHCI {:02x}:{:02x}.{} (Vendor={:#06x} Device={:#06x}, CMD={:#06x}) ABAR={:#x} -> MMIO={:#x} | Puerto {} activo (SSTS={:#x}, SIG={:#x}) | DMA CmdList={:#x} FIS={:#x} Buf={:#x} (16 KiB)",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            pci_cmd,
            bar5_phys,
            abar_mmio,
            active_port,
            ssts,
            sig,
            frame_paddr,
            fis_paddr,
            bounce_buffer.paddr()
        );
        crate::fb_println!(
            "[ostd::storage::ahci] SATA AHCI 00:1f.2 (Port {}) ABAR={:#x} | Link=UP | DMA Buf={:#x}",
            active_port,
            bar5_phys,
            bounce_buffer.paddr()
        );

        *AHCI_STATE.lock() = Some(AhciDriverState {
            pci_dev: dev,
            pci_cmd,
            bar5_phys,
            abar_mmio,
            active_port,
            port_mmio,
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

    let mut current_lba = lba;
    let mut offset = 0usize;
    let mut remaining = sector_count;

    while remaining > 0 {
        let chunk_sectors = remaining.min(AHCI_MAX_CHUNK_SECTORS);
        let chunk_bytes = chunk_sectors * 512;

        // SAFETY: Executing ATA_CMD_READ_DMA_EXT via configured AHCI DMA rings.
        unsafe {
            execute_dma_command(state, current_lba, chunk_sectors as u16, false)?;
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
            execute_dma_command(state, current_lba, chunk_sectors as u16, true)?;
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

    for _ in 0..100_000 {
        let cur = core::ptr::read_volatile(cmd_ptr);
        if (cur & (PORT_CMD_CR | PORT_CMD_FR)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

unsafe fn start_port(port_mmio: VirtAddr) {
    let cmd_ptr = (port_mmio + PORT_REG_CMD as u64) as *mut u32;
    for _ in 0..100_000 {
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

/// Executes an ATA 48-bit LBA DMA command (`ATA_CMD_READ_DMA_EXT` = 0x25 or `ATA_CMD_WRITE_DMA_EXT` = 0x35)
/// on slot 0 of the active AHCI port.
unsafe fn execute_dma_command(
    state: &mut AhciDriverState,
    lba: u64,
    count: u16,
    is_write: bool,
) -> Result<(), &'static str> {
    let port_mmio = state.port_mmio;
    let tfd_ptr = (port_mmio + PORT_REG_TFD as u64) as *const u32;

    // 1. Wait until device is not busy (BSY=0, DRQ=0)
    for _ in 0..100_000 {
        let tfd = core::ptr::read_volatile(tfd_ptr);
        if (tfd & (ATA_DEV_BUSY | ATA_DEV_DRQ)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let structs_vaddr = state.dma_structs_frame.vaddr();
    let structs_paddr = state.dma_structs_frame.paddr();

    // 2. Setup Command Header 0 (32 bytes at offset 0)
    let cmd_header_ptr = structs_vaddr as *mut u32;
    let write_bit = if is_write { 1u32 << 6 } else { 0u32 };
    let prdtl = 1u32 << 16; // 1 PRD entry
    let cfl = 5u32; // 5 DWORDs in Register H2D FIS
    core::ptr::write_volatile(cmd_header_ptr.add(0), cfl | write_bit | prdtl);
    core::ptr::write_volatile(cmd_header_ptr.add(1), 0); // PRDBC

    let ctba_paddr = structs_paddr + 2048; // Command Table at offset 2048
    core::ptr::write_volatile(
        cmd_header_ptr.add(2),
        (ctba_paddr & 0xFFFF_FFFF) as u32,
    );
    core::ptr::write_volatile(
        cmd_header_ptr.add(3),
        ((ctba_paddr >> 32) & 0xFFFF_FFFF) as u32,
    );
    for j in 4..8 {
        core::ptr::write_volatile(cmd_header_ptr.add(j), 0);
    }

    // 3. Setup Command Table (offset 2048): CFIS (64 B) + ACMD (16 B) + RSV (48 B) + PRDT (16 B)
    let ct_vaddr = structs_vaddr + 2048;
    let cfis_ptr = ct_vaddr as *mut u8;
    core::ptr::write_bytes(cfis_ptr, 0, 64);

    let cmd_code = if is_write {
        ATA_CMD_WRITE_DMA_EXT
    } else {
        ATA_CMD_READ_DMA_EXT
    };

    core::ptr::write_volatile(cfis_ptr.add(0), FIS_TYPE_REG_H2D);
    core::ptr::write_volatile(cfis_ptr.add(1), 0x80); // C=1 (Command)
    core::ptr::write_volatile(cfis_ptr.add(2), cmd_code);
    core::ptr::write_volatile(cfis_ptr.add(4), (lba & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(5), ((lba >> 8) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(6), ((lba >> 16) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(7), 1 << 6); // LBA mode bit
    core::ptr::write_volatile(cfis_ptr.add(8), ((lba >> 24) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(9), ((lba >> 32) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(10), ((lba >> 40) & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(12), (count & 0xFF) as u8);
    core::ptr::write_volatile(cfis_ptr.add(13), ((count >> 8) & 0xFF) as u8);

    // 4. Setup PRDT[0] at ct_vaddr + 0x80 (128 bytes)
    let prdt_ptr = (ct_vaddr + 0x80) as *mut u32;
    let dma_buf_paddr = state.bounce_buffer.paddr();
    let transfer_bytes = (count as u32) * 512;

    core::ptr::write_volatile(
        prdt_ptr.add(0),
        (dma_buf_paddr & 0xFFFF_FFFF) as u32,
    );
    core::ptr::write_volatile(
        prdt_ptr.add(1),
        ((dma_buf_paddr >> 32) & 0xFFFF_FFFF) as u32,
    );
    core::ptr::write_volatile(prdt_ptr.add(2), 0);
    // Data byte count is 0-based: transfer_bytes - 1
    core::ptr::write_volatile(
        prdt_ptr.add(3),
        (transfer_bytes - 1) | (1u32 << 31),
    );

    // 5. Clear interrupt status (RW1C)
    let is_ptr = (port_mmio + PORT_REG_IS as u64) as *mut u32;
    core::ptr::write_volatile(is_ptr, 0xFFFF_FFFF);

    // 6. Issue Command Slot 0 via PxCI
    let ci_ptr = (port_mmio + PORT_REG_CI as u64) as *mut u32;
    core::ptr::write_volatile(ci_ptr, 1u32 << 0);

    // 7. Wait for completion with timeout
    let mut timeout = 2_000_000usize;
    while (core::ptr::read_volatile(ci_ptr) & 1) != 0 {
        let is = core::ptr::read_volatile(is_ptr);
        if (is & (1 << 30)) != 0 {
            return Err("SATA AHCI: Error en descriptor de tarea (Task File Error)");
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("SATA AHCI: Timeout en operacion DMA");
        }
        core::hint::spin_loop();
    }

    let tfd = core::ptr::read_volatile(tfd_ptr);
    if (tfd & ATA_DEV_ERR) != 0 {
        return Err("SATA AHCI: Error reportado por el disco en Task File Data (TFD.ERR)");
    }

    Ok(())
}
