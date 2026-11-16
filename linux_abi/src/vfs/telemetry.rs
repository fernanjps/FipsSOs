#![forbid(unsafe_code)]

use super::CapabilityVfs;
use super::devfs_sysfs::{
    FIPSOS_IOCTL_LIVE_KERNEL_TELEMETRY, FIPSOS_IOCTL_MSR_GP_TEST, FIPSOS_IOCTL_RAM_RW_PROBE,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LiveCoreTelemetry {
    pub active_ticks: u64,
    pub idle_ticks: u64,
    pub irq_count: u64,
    pub apic_id: u32,
    pub online: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LiveProcTelemetry {
    pub pid: u64,
    pub tid: u64,
    pub cr3_phys: u64,
    pub slot_id: u32,
    pub state: u32,
    pub user_pages: u32,
    pub ticks: u32,
    pub name: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FipsosLiveTelemetry {
    pub total_frames: u64,
    pub used_frames: u64,
    pub free_frames: u64,
    pub heap_used_bytes: u64,
    pub heap_total_bytes: u64,
    pub irq0_ticks: u64,
    pub last_ram_rw_mbps: u32,
    pub last_ram_rw_pages_ok: u32,
    pub online_cores: u32,
    pub active_procs: u32,
    pub cores: [LiveCoreTelemetry; 8],
    pub procs: [LiveProcTelemetry; 8],
}

const _: () = assert!(core::mem::size_of::<LiveCoreTelemetry>() == 32);
const _: () = assert!(core::mem::size_of::<LiveProcTelemetry>() == 56);
const _: () = assert!(core::mem::size_of::<FipsosLiveTelemetry>() == 768);

#[inline]
fn put_u64(buf: &mut [u8], off: usize, val: u64) {
    buf[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

#[inline]
fn put_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn serialize_live_telemetry(
    mem: &ostd::mm::MemoryStats,
    heap_used: usize,
    heap_total: usize,
    irq0_ticks: u64,
    last_rw_mbps: u32,
    last_rw_pages: u32,
    online_cores: u32,
    active_procs: u32,
    cores: &[ostd::scheduler::LiveCoreInfo; 8],
    procs: &[ostd::scheduler::LiveProcInfo; 8],
) -> [u8; 768] {
    let mut pkt = [0u8; 768];
    put_u64(&mut pkt, 0, mem.total_usable_frames as u64);
    put_u64(&mut pkt, 8, mem.used_frames as u64);
    put_u64(&mut pkt, 16, mem.free_frames as u64);
    put_u64(&mut pkt, 24, heap_used as u64);
    put_u64(&mut pkt, 32, heap_total as u64);
    put_u64(&mut pkt, 40, irq0_ticks);
    put_u32(&mut pkt, 48, last_rw_mbps);
    put_u32(&mut pkt, 52, last_rw_pages);
    put_u32(&mut pkt, 56, online_cores);
    put_u32(&mut pkt, 60, active_procs);

    for (i, c) in cores.iter().enumerate() {
        let base = 64 + i * 32;
        put_u64(&mut pkt, base, c.active_ticks);
        put_u64(&mut pkt, base + 8, c.idle_ticks);
        put_u64(&mut pkt, base + 16, c.irq_count);
        put_u32(&mut pkt, base + 24, c.apic_id);
        put_u32(&mut pkt, base + 28, c.online);
    }

    for (i, p) in procs.iter().enumerate() {
        let base = 320 + i * 56;
        put_u64(&mut pkt, base, p.pid);
        put_u64(&mut pkt, base + 8, p.tid);
        put_u64(&mut pkt, base + 16, p.cr3_phys);
        put_u32(&mut pkt, base + 24, p.slot_id);
        put_u32(&mut pkt, base + 28, p.state);
        put_u32(&mut pkt, base + 32, p.user_pages);
        put_u32(&mut pkt, base + 36, p.ticks);
        pkt[base + 40..base + 56].copy_from_slice(&p.name);
    }

    pkt
}

impl CapabilityVfs {
    fn is_valid_telemetry_fd(&self, fd: u64) -> bool {
        fd <= super::FD_STDERR
            || fd == super::FD_FB0
            || self.devices.contains_key(&fd)
            || super::fd_table::get_process_node(ostd::scheduler::active_slot(), fd).is_some()
    }

    /// Dispatches live telemetry (`0xC010_5304`), RAM R/W probe (`0xC010_5305`), and MSR `#GP` self-test (`0xC010_5306`).
    pub fn dispatch_live_telemetry_ioctl(
        &self,
        fd: u64,
        request: u64,
        arg: u64,
    ) -> Option<Result<u64, i64>> {
        if request != FIPSOS_IOCTL_LIVE_KERNEL_TELEMETRY
            && request != FIPSOS_IOCTL_RAM_RW_PROBE
            && request != FIPSOS_IOCTL_MSR_GP_TEST
        {
            return None;
        }
        if !self.is_valid_telemetry_fd(fd) {
            return Some(Err(-9i64)); // -EBADF
        }

        if request == FIPSOS_IOCTL_MSR_GP_TEST {
            #[cfg(target_arch = "x86_64")]
            {
                let rep = ostd::arch::msr::probe_msr_gp_recovery_selftest();
                super::devfs_sysfs::invalidate_hwmon_cache();
                if arg >= 0x10000 {
                    let mut out = [0u8; 32];
                    put_u64(&mut out, 0, rep.gp_before);
                    put_u64(&mut out, 8, rep.gp_after);
                    put_u64(&mut out, 16, rep.hwcr.is_some() as u64);
                    put_u64(&mut out, 24, rep.therm_status.is_some() as u64);
                    if ostd::mm::write_user_bytes(arg, &out).is_err() {
                        return Some(Err(-14i64));
                    }
                }
                return Some(Ok(rep.gp_after));
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = arg;
                return Some(Ok(0));
            }
        }

        if request == FIPSOS_IOCTL_RAM_RW_PROBE {
            let mut user_req_ptr = 0u64;
            let mut num_pages: usize = 64;
            if arg >= 0x10000 {
                user_req_ptr = arg;
                if ostd::mm::validate_user_buffer(user_req_ptr, 32, true).is_err() {
                    return Some(Err(-14i64)); // -EFAULT
                }
                let mut hdr = [0u8; 8];
                if ostd::arch::syscall::read_user_bytes(arg, &mut hdr).is_err() {
                    return Some(Err(-14i64));
                }
                let requested = u64::from_le_bytes(hdr);
                if requested > 2048 {
                    return Some(Err(-22i64)); // -EINVAL
                }
                if requested >= 1 {
                    num_pages = requested as usize;
                }
            } else if arg > 2048 {
                return Some(Err(-22i64)); // -EINVAL
            } else if arg >= 1 {
                num_pages = arg as usize;
            }

            let stats = match ostd::mm::live_ram_rw_probe(num_pages) {
                Ok(st) => st,
                Err(_) => return Some(Err(-12i64)),
            };

            if user_req_ptr != 0 {
                let mut out = [0u8; 32];
                put_u64(&mut out, 0, num_pages as u64);
                put_u64(&mut out, 8, stats.bytes_tested);
                put_u64(&mut out, 16, stats.tsc_cycles);
                put_u32(&mut out, 24, stats.mbps);
                put_u32(&mut out, 28, stats.pages_tested);
                if ostd::mm::write_user_bytes(user_req_ptr, &out).is_err() {
                    return Some(Err(-14i64));
                }
            }

            return Some(Ok(((stats.mbps as u64) << 16) | (stats.pages_tested as u64)));
        }

        if arg == 0 {
            return Some(Err(-14i64));
        }

        let mem = ostd::mm::memory_stats();
        let (heap_used, heap_total) = ostd::mm::heap_stats();
        let irq0_ticks = ostd::arch::idt::timer_ticks();
        let (last_rw_mbps, last_rw_pages) = ostd::mm::last_ram_probe_stats();
        let mut cores = [ostd::scheduler::LiveCoreInfo::empty(); 8];
        let mut procs = [ostd::scheduler::LiveProcInfo::empty(); 8];
        let (online_cores, active_procs) =
            ostd::scheduler::collect_live_telemetry(&mut cores, &mut procs);

        let pkt = serialize_live_telemetry(
            &mem,
            heap_used,
            heap_total,
            irq0_ticks,
            last_rw_mbps,
            last_rw_pages,
            online_cores,
            active_procs,
            &cores,
            &procs,
        );
        if ostd::mm::write_user_bytes(arg, &pkt).is_err() {
            return Some(Err(-14i64));
        }
        Some(Ok(active_procs as u64))
    }
}
