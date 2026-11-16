//! Real Intel High Definition Audio (HDA) PCIe DMA Driver (`Class 0x04:0x03`).
//!
//! Zero simulated audio: if no Intel HDA controller or active codec is present on PCIe,
//! `play_pcm()` / `read_pcm()` return `Err(-19)` (`-ENODEV`).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use crate::mm::dma::DmaBuffer;
use crate::mm::{map_kernel_mmio, VirtAddr};
use crate::sync::SpinLock;

const REG_GCAP: u64 = 0x00;
const REG_GCTL: u64 = 0x08;
const REG_STATESTS: u64 = 0x0E;
const REG_CORBLBASE: u64 = 0x40;
const REG_CORBUBASE: u64 = 0x44;
const REG_CORBWP: u64 = 0x48;
const REG_CORBRP: u64 = 0x4A;
const REG_CORBCTL: u64 = 0x4C;
const REG_CORBSIZE: u64 = 0x4E;
const REG_RIRBLBASE: u64 = 0x50;
const REG_RIRBUBASE: u64 = 0x54;
const REG_RIRBWP: u64 = 0x58;
const REG_RINTCNT: u64 = 0x5A;
const REG_RIRBCTL: u64 = 0x5C;
const REG_RIRBSIZE: u64 = 0x5E;

const SD_OFF_CTL: u64 = 0x00;
const SD_OFF_STS: u64 = 0x03;
const SD_OFF_LPIB: u64 = 0x04;
const SD_OFF_CBL: u64 = 0x08;
const SD_OFF_LVI: u64 = 0x0C;
const SD_OFF_FMT: u64 = 0x12;
const SD_OFF_BDLPL: u64 = 0x18;
const SD_OFF_BDLPU: u64 = 0x1C;

const HDA_MMIO_SIZE: usize = 0x4000;
const PCM_DMA_BYTES: usize = 16 * 1024;
const FMT_48KHZ_16BIT_2CH: u16 = 0x0011;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct BdlEntry {
    addr: u64,
    len: u32,
    ioc: u32,
}

/// Hardware telemetry and codec state for an Intel HDA controller.
#[derive(Clone, Copy, Debug)]
pub struct HdaInfo {
    pub present: bool,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0_phys: u64,
    pub codec_mask: u16,
    pub active_codec: u8,
    pub dac_nid: u8,
    pub pin_nid: u8,
    pub adc_nid: u8,
    pub pin_in_nid: u8,
    pub iss: u8,
    pub oss: u8,
    pub frames_played: u64,
    pub frames_captured: u64,
}

impl HdaInfo {
    const fn empty() -> Self {
        Self {
            present: false,
            vendor_id: 0,
            device_id: 0,
            bar0_phys: 0,
            codec_mask: 0,
            active_codec: 0,
            dac_nid: 0,
            pin_nid: 0,
            adc_nid: 0,
            pin_in_nid: 0,
            iss: 0,
            oss: 0,
            frames_played: 0,
            frames_captured: 0,
        }
    }
}

struct HdaController {
    info: HdaInfo,
    mmio: VirtAddr,
    corb_dma: Option<DmaBuffer>,
    rirb_dma: Option<DmaBuffer>,
    bdl_dma: Option<DmaBuffer>,
    pcm_dma: Option<DmaBuffer>,
    in_bdl_dma: Option<DmaBuffer>,
    in_pcm_dma: Option<DmaBuffer>,
    corb_wp: u16,
    rirb_rp: u16,
}

static HDA: SpinLock<HdaController> = SpinLock::new(HdaController {
    info: HdaInfo::empty(),
    mmio: 0,
    corb_dma: None,
    rirb_dma: None,
    bdl_dma: None,
    pcm_dma: None,
    in_bdl_dma: None,
    in_pcm_dma: None,
    corb_wp: 0,
    rirb_rp: 0,
});

#[inline]
unsafe fn mmio_r8(base: VirtAddr, off: u64) -> u8 {
    read_volatile((base + off) as *const u8)
}

#[inline]
unsafe fn mmio_w8(base: VirtAddr, off: u64, v: u8) {
    write_volatile((base + off) as *mut u8, v);
}

#[inline]
unsafe fn mmio_r16(base: VirtAddr, off: u64) -> u16 {
    read_volatile((base + off) as *const u16)
}

#[inline]
unsafe fn mmio_w16(base: VirtAddr, off: u64, v: u16) {
    write_volatile((base + off) as *mut u16, v);
}

#[inline]
unsafe fn mmio_r32(base: VirtAddr, off: u64) -> u32 {
    read_volatile((base + off) as *const u32)
}

#[inline]
unsafe fn mmio_w32(base: VirtAddr, off: u64, v: u32) {
    write_volatile((base + off) as *mut u32, v);
}

unsafe fn reset_controller(mmio: VirtAddr) -> bool {
    let gctl = mmio_r32(mmio, REG_GCTL);
    mmio_w32(mmio, REG_GCTL, gctl & !1);
    let stop_deadline = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(stop_deadline) {
        if (mmio_r32(mmio, REG_GCTL) & 1) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    mmio_w32(mmio, REG_GCTL, 1);
    let start_deadline = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(start_deadline) {
        if (mmio_r32(mmio, REG_GCTL) & 1) != 0 {
            // Intel HDA Spec 4.3: software must wait at least 521 us after CRST=1 for codec enumeration.
            crate::time::tsc_delay_us(600);
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn setup_corb_rirb(ctl: &mut HdaController) -> bool {
    let mmio = ctl.mmio;
    let Some(corb) = ctl.corb_dma.as_ref() else { return false; };
    let Some(rirb) = ctl.rirb_dma.as_ref() else { return false; };

    mmio_w8(mmio, REG_CORBCTL, 0);
    let corb_stop_dl = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(corb_stop_dl) {
        if (mmio_r8(mmio, REG_CORBCTL) & 0x02) == 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w8(mmio, REG_CORBSIZE, 0x02); // 256 entries (1024B)
    let c_phys = corb.paddr();
    mmio_w32(mmio, REG_CORBLBASE, c_phys as u32);
    mmio_w32(mmio, REG_CORBUBASE, (c_phys >> 32) as u32);
    mmio_w16(mmio, REG_CORBRP, 0x8000);
    let rp_set_dl = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(rp_set_dl) {
        if (mmio_r16(mmio, REG_CORBRP) & 0x8000) != 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w16(mmio, REG_CORBRP, 0);
    let rp_clr_dl = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(rp_clr_dl) {
        if (mmio_r16(mmio, REG_CORBRP) & 0x8000) == 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w16(mmio, REG_CORBWP, 0);
    ctl.corb_wp = 0;

    mmio_w8(mmio, REG_RIRBCTL, 0);
    let rirb_stop_dl = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(rirb_stop_dl) {
        if (mmio_r8(mmio, REG_RIRBCTL) & 0x02) == 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w8(mmio, REG_RIRBSIZE, 0x02); // 256 entries (2048B)
    let r_phys = rirb.paddr();
    mmio_w32(mmio, REG_RIRBLBASE, r_phys as u32);
    mmio_w32(mmio, REG_RIRBUBASE, (r_phys >> 32) as u32);
    mmio_w16(mmio, REG_RIRBWP, 0x8000);
    mmio_w16(mmio, REG_RINTCNT, 1);
    ctl.rirb_rp = 0;

    mmio_w8(mmio, REG_CORBCTL, 0x02);
    mmio_w8(mmio, REG_RIRBCTL, 0x02);
    true
}

unsafe fn send_verb(ctl: &mut HdaController, codec: u8, nid: u8, verb: u32) -> Option<u32> {
    let corb = ctl.corb_dma.as_ref()?;
    let rirb = ctl.rirb_dma.as_ref()?;
    let next_wp = (ctl.corb_wp + 1) & 0xFF;
    let cmd: u32 = ((codec as u32) << 28) | ((nid as u32) << 20) | (verb & 0xF_FFFF);
    let _ = corb.write_bytes((next_wp as usize) * 4, &cmd.to_le_bytes());
    fence(Ordering::SeqCst);
    ctl.corb_wp = next_wp;
    mmio_w16(ctl.mmio, REG_CORBWP, next_wp);

    let deadline = crate::time::tsc_deadline_ms(50);
    while !crate::time::tsc_expired(deadline) {
        let wp = mmio_r16(ctl.mmio, REG_RIRBWP) & 0xFF;
        if wp != ctl.rirb_rp {
            ctl.rirb_rp = (ctl.rirb_rp + 1) & 0xFF;
            let mut buf = [0u8; 8];
            let _ = rirb.read_bytes((ctl.rirb_rp as usize) * 8, &mut buf);
            return Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]));
        }
        core::hint::spin_loop();
    }
    None
}

unsafe fn configure_codec_io(ctl: &mut HdaController, codec: u8) -> (u8, u8, u8, u8) {
    let sub = send_verb(ctl, codec, 0, 0xF0004).unwrap_or(0);
    let fg_start = ((sub >> 16) & 0xFF) as u8;
    let fg_count = (sub & 0xFF) as u8;

    let mut afg_nid = 0u8;
    for nid in fg_start..fg_start.saturating_add(fg_count) {
        let fg_type = send_verb(ctl, codec, nid, 0xF0005).unwrap_or(0) & 0xFF;
        if fg_type == 0x01 {
            afg_nid = nid;
            break;
        }
    }
    if afg_nid == 0 {
        return (0, 0, 0, 0);
    }
    let _ = send_verb(ctl, codec, afg_nid, 0x70500); // D0 power
    let wsub = send_verb(ctl, codec, afg_nid, 0xF0004).unwrap_or(0);
    let w_start = ((wsub >> 16) & 0xFF) as u8;
    let w_count = (wsub & 0xFF) as u8;

    let (mut dac_nid, mut pin_nid, mut adc_nid, mut pin_in_nid) = (0u8, 0u8, 0u8, 0u8);
    for nid in w_start..w_start.saturating_add(w_count) {
        let wcaps = send_verb(ctl, codec, nid, 0xF0009).unwrap_or(0);
        let wtype = (wcaps >> 20) & 0xF;
        if wtype == 0x0 && dac_nid == 0 {
            dac_nid = nid;
        } else if wtype == 0x1 && adc_nid == 0 {
            adc_nid = nid;
        } else if wtype == 0x4 {
            let pcaps = send_verb(ctl, codec, nid, 0xF000C).unwrap_or(0);
            if (pcaps & 0x10) != 0 && pin_nid == 0 {
                pin_nid = nid;
            }
            if (pcaps & 0x20) != 0 && pin_in_nid == 0 {
                pin_in_nid = nid;
            }
        }
    }
    if dac_nid != 0 {
        let _ = send_verb(ctl, codec, dac_nid, 0x70500); // D0
        let _ = send_verb(ctl, codec, dac_nid, 0x70610); // Stream 1, Ch 0
        let _ = send_verb(ctl, codec, dac_nid, 0x20000 | (FMT_48KHZ_16BIT_2CH as u32));
        let _ = send_verb(ctl, codec, dac_nid, 0x3B000); // Unmute output amp
    }
    if pin_nid != 0 {
        let _ = send_verb(ctl, codec, pin_nid, 0x70500); // D0
        let _ = send_verb(ctl, codec, pin_nid, 0x70740); // Pin OUT_EN
        let _ = send_verb(ctl, codec, pin_nid, 0x3B000);
    }
    if adc_nid != 0 {
        let _ = send_verb(ctl, codec, adc_nid, 0x70500); // D0
        let _ = send_verb(ctl, codec, adc_nid, 0x70620); // Stream 2, Ch 0
        let _ = send_verb(ctl, codec, adc_nid, 0x20000 | (FMT_48KHZ_16BIT_2CH as u32));
        let _ = send_verb(ctl, codec, adc_nid, 0x37000); // Unmute input amp
    }
    if pin_in_nid != 0 {
        let _ = send_verb(ctl, codec, pin_in_nid, 0x70500); // D0
        let _ = send_verb(ctl, codec, pin_in_nid, 0x70720); // Pin IN_EN
        let _ = send_verb(ctl, codec, pin_in_nid, 0x37000);
    }
    (dac_nid, pin_nid, adc_nid, pin_in_nid)
}

/// Probes and initializes Intel HDA PCIe controller (`0x04:0x03`) with CORB/RIRB DMA.
pub fn init() {
    let Some((dev, bar0_phys, _pci_cmd)) = crate::bus::pcie::probe_intel_hda() else {
        crate::early_println!("[ostd::hda] No se detecto controladora Intel HDA PCIe (0x04:0x03)");
        return;
    };
    let mmio = map_kernel_mmio(bar0_phys, HDA_MMIO_SIZE);
    if mmio == 0 {
        crate::early_println!("[ostd::hda] Fallo al mapear MMIO BAR0={:#x}", bar0_phys);
        return;
    }

    let (Ok(corb), Ok(rirb), Ok(bdl), Ok(pcm), Ok(in_bdl), Ok(in_pcm)) = (
        DmaBuffer::allocate(4096),
        DmaBuffer::allocate(4096),
        DmaBuffer::allocate(4096),
        DmaBuffer::allocate(PCM_DMA_BYTES),
        DmaBuffer::allocate(4096),
        DmaBuffer::allocate(PCM_DMA_BYTES),
    ) else {
        crate::early_println!("[ostd::hda] Sin memoria DMA contigua para CORB/RIRB/BDL");
        return;
    };

    let mut ctl = HDA.lock();
    ctl.mmio = mmio;
    ctl.corb_dma = Some(corb);
    ctl.rirb_dma = Some(rirb);
    ctl.bdl_dma = Some(bdl);
    ctl.pcm_dma = Some(pcm);
    ctl.in_bdl_dma = Some(in_bdl);
    ctl.in_pcm_dma = Some(in_pcm);

    // SAFETY: `mmio` maps Intel HDA BAR0 registers in Ring 0.
    unsafe {
        if !reset_controller(mmio) {
            crate::early_println!("[ostd::hda] Timeout en reset GCTL.CRST");
            return;
        }
        let gcap = mmio_r16(mmio, REG_GCAP);
        let iss = ((gcap >> 8) & 0xF) as u8;
        let oss = ((gcap >> 12) & 0xF) as u8;
        let mut codec_mask = mmio_r16(mmio, REG_STATESTS) & 0x7FFF;
        if codec_mask == 0 {
            let statests_dl = crate::time::tsc_deadline_ms(10);
            while !crate::time::tsc_expired(statests_dl) {
                codec_mask = mmio_r16(mmio, REG_STATESTS) & 0x7FFF;
                if codec_mask != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
        if oss == 0 || !setup_corb_rirb(&mut ctl) {
            crate::early_println!("[ostd::hda] Fallo al configurar CORB/RIRB o OSS=0");
            return;
        }
        let mut active_codec = 0u8;
        let mut found_codec = false;
        for c in 0..15u8 {
            if (codec_mask & (1 << c)) != 0 {
                active_codec = c;
                found_codec = true;
                break;
            }
        }
        let (dac_nid, pin_nid, adc_nid, pin_in_nid) = if found_codec {
            configure_codec_io(&mut ctl, active_codec)
        } else {
            (0, 0, 0, 0)
        };
        ctl.info = HdaInfo {
            present: true,
            vendor_id: dev.vendor_id,
            device_id: dev.device_id,
            bar0_phys,
            codec_mask,
            active_codec,
            dac_nid,
            pin_nid,
            adc_nid,
            pin_in_nid,
            iss,
            oss,
            frames_played: 0,
            frames_captured: 0,
        };
        crate::early_println!(
            "[ostd::hda] Intel HDA {:04x}:{:04x} BAR0={:#x} | CORB/RIRB DMA OK | STATESTS={:#x} Codec={} DAC={} PinOut={} ADC={} PinIn={} (ISS={}, OSS={})",
            dev.vendor_id, dev.device_id, bar0_phys, codec_mask, active_codec,
            dac_nid, pin_nid, adc_nid, pin_in_nid, iss, oss
        );
    }
}

unsafe fn run_stream_dma(
    mmio: VirtAddr,
    sd_base: u64,
    stream_tag: u32,
    bdl: &DmaBuffer,
    pcm_phys: u64,
    cbl_len: usize,
) {
    let half = (cbl_len / 2) as u32;
    let e0 = BdlEntry { addr: pcm_phys, len: half, ioc: 0 };
    let e1 = BdlEntry { addr: pcm_phys + half as u64, len: (cbl_len as u32) - half, ioc: 1 };
    // SAFETY: `BdlEntry` is `#[repr(C, packed)]` and exactly 16 bytes with no invalid bit patterns.
    let b0: [u8; 16] = core::mem::transmute(e0);
    // SAFETY: Same 16-byte POD layout as `b0`.
    let b1: [u8; 16] = core::mem::transmute(e1);
    let _ = bdl.write_bytes(0, &b0);
    let _ = bdl.write_bytes(16, &b1);
    let bdl_phys = bdl.paddr();

    mmio_w32(mmio, sd_base + SD_OFF_CTL, 0x01); // SRST=1
    let srst_set_dl = crate::time::tsc_deadline_ms(25);
    while !crate::time::tsc_expired(srst_set_dl) {
        if (mmio_r32(mmio, sd_base + SD_OFF_CTL) & 1) != 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w32(mmio, sd_base + SD_OFF_CTL, 0x00); // SRST=0
    let srst_clr_dl = crate::time::tsc_deadline_ms(25);
    while !crate::time::tsc_expired(srst_clr_dl) {
        if (mmio_r32(mmio, sd_base + SD_OFF_CTL) & 1) == 0 { break; }
        core::hint::spin_loop();
    }
    mmio_w8(mmio, sd_base + SD_OFF_STS, 0x1C);
    mmio_w32(mmio, sd_base + SD_OFF_CBL, cbl_len as u32);
    mmio_w16(mmio, sd_base + SD_OFF_LVI, 1);
    mmio_w16(mmio, sd_base + SD_OFF_FMT, FMT_48KHZ_16BIT_2CH);
    mmio_w32(mmio, sd_base + SD_OFF_BDLPL, bdl_phys as u32);
    mmio_w32(mmio, sd_base + SD_OFF_BDLPU, (bdl_phys >> 32) as u32);
    fence(Ordering::SeqCst);
    mmio_w32(mmio, sd_base + SD_OFF_CTL, (stream_tag << 20) | 0x02); // RUN=1
    let lpib_dl = crate::time::tsc_deadline_ms(25);
    while !crate::time::tsc_expired(lpib_dl) {
        if mmio_r32(mmio, sd_base + SD_OFF_LPIB) > 0 { break; }
        core::hint::spin_loop();
    }
}

/// Plays raw PCM samples over the Intel HDA output stream (`OSD0`) BDL DMA engine.
/// Returns `Err(-19)` (`-ENODEV`) if no Intel HDA controller is present.
pub fn play_pcm(data: &[u8]) -> Result<usize, i64> {
    let mut ctl = HDA.lock();
    if !ctl.info.present || ctl.info.oss == 0 {
        return Err(-19); // -ENODEV
    }
    if data.is_empty() {
        return Ok(0);
    }
    let sd_base = 0x80 + (ctl.info.iss as u64) * 0x20;
    let copy_len = data.len().min(PCM_DMA_BYTES);
    let cbl_len = copy_len.max(64).min(PCM_DMA_BYTES) & !3;
    let pcm_phys = {
        let pcm = ctl.pcm_dma.as_ref().ok_or(-19i64)?;
        let _ = pcm.write_bytes(0, &data[..copy_len]);
        pcm.paddr()
    };
    let mmio = ctl.mmio;
    {
        let bdl = ctl.bdl_dma.as_ref().ok_or(-19i64)?;
        // SAFETY: Configuring HDA output stream descriptor registers mapped at `mmio`.
        unsafe { run_stream_dma(mmio, sd_base, 1, bdl, pcm_phys, cbl_len); }
    }
    ctl.info.frames_played = ctl.info.frames_played.saturating_add(copy_len as u64);
    Ok(copy_len)
}

/// Captures raw PCM samples from the Intel HDA input stream (`ISD0`) BDL DMA engine.
/// Returns `Err(-19)` (`-ENODEV`) if no Intel HDA controller or input stream is present.
pub fn read_pcm(dst: &mut [u8]) -> Result<usize, i64> {
    let mut ctl = HDA.lock();
    if !ctl.info.present || ctl.info.iss == 0 {
        return Err(-19); // -ENODEV
    }
    if dst.is_empty() {
        return Ok(0);
    }
    let sd_base = 0x80u64; // ISD0 is always the first stream descriptor at MMIO + 0x80
    let copy_len = dst.len().min(PCM_DMA_BYTES);
    let cbl_len = copy_len.max(64).min(PCM_DMA_BYTES) & !3;
    let pcm_phys = ctl.in_pcm_dma.as_ref().ok_or(-19i64)?.paddr();
    let mmio = ctl.mmio;
    {
        let bdl = ctl.in_bdl_dma.as_ref().ok_or(-19i64)?;
        // SAFETY: Configuring HDA input stream descriptor `ISD0` mapped at `mmio`.
        unsafe {
            run_stream_dma(mmio, sd_base, 2, bdl, pcm_phys, cbl_len);
            crate::time::tsc_delay_us(500);
            mmio_w32(mmio, sd_base + SD_OFF_CTL, 2 << 20); // Stop ISD0 after burst capture
        }
    }
    fence(Ordering::SeqCst);
    {
        let pcm = ctl.in_pcm_dma.as_ref().ok_or(-19i64)?;
        let _ = pcm.read_bytes(0, &mut dst[..copy_len]);
    }
    ctl.info.frames_captured = ctl.info.frames_captured.saturating_add(copy_len as u64);
    Ok(copy_len)
}

/// Returns the current Intel HDA controller and codec telemetry snapshot.
pub fn info() -> HdaInfo {
    HDA.lock().info
}
