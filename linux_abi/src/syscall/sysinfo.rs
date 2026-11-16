#![forbid(unsafe_code)]

use ostd::arch::syscall::write_user_bytes;
use ostd::mm::validate_user_buffer;

/// Linux `sys_uname` (syscall 63): returns system identification in `struct utsname` (390 bytes).
pub fn sys_uname(buf_addr: u64) -> i64 {
    if validate_user_buffer(buf_addr, 390, true).is_err() {
        return -14; // -EFAULT
    }
    let mut uts = [0u8; 390];
    let sysname = b"FipSOs\0";
    let host_str = crate::vfs::get_process_hostname(ostd::scheduler::active_slot());
    let release = b"6.3.0-FipSOs\0";
    let version = b"#1 SMP Framekernel\0";
    let machine = b"x86_64\0";
    let domain = b"(none)\0";

    uts[0..sysname.len()].copy_from_slice(sysname);
    let host_bytes = host_str.as_bytes();
    let hlen = host_bytes.len().min(64);
    uts[65..65 + hlen].copy_from_slice(&host_bytes[..hlen]);
    uts[65 + hlen] = 0;
    uts[130..130 + release.len()].copy_from_slice(release);
    uts[195..195 + version.len()].copy_from_slice(version);
    uts[260..260 + machine.len()].copy_from_slice(machine);
    uts[325..325 + domain.len()].copy_from_slice(domain);

    if write_user_bytes(buf_addr, &uts).is_err() {
        -14 // -EFAULT
    } else {
        0
    }
}

/// Linux `sys_clock_gettime` (228) implementation writing a 16-byte `timespec` (`tv_sec`, `tv_nsec`).
pub fn sys_clock_gettime(clk_id: u64, tp_user_addr: u64) -> i64 {
    if tp_user_addr == 0 || validate_user_buffer(tp_user_addr, 16, true).is_err() {
        return -14i64; // -EFAULT
    }

    let (sec, nsec) = match clk_id {
        0 => {
            // CLOCK_REALTIME: read wall-clock date and time from CMOS RTC
            let dt = ostd::arch::x86_64::rtc_power::read_rtc();
            let unix_sec = ostd::arch::x86_64::rtc_power::rtc_to_unix_timestamp(&dt);
            let nsec = ((ostd::arch::idt::timer_ticks() % 100) * 10_000_000) as i64;
            (unix_sec, nsec)
        }
        _ => {
            // CLOCK_MONOTONIC (1) and others: monotonic uptime based on IRQ0 PIT ticks
            let ticks = ostd::arch::idt::timer_ticks();
            let sec = (ticks / 100) as i64;
            let nsec = ((ticks % 100) * 10_000_000) as i64;
            (sec, nsec)
        }
    };

    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&sec.to_le_bytes());
    buf[8..16].copy_from_slice(&nsec.to_le_bytes());

    if write_user_bytes(tp_user_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}

/// Linux `sys_sysinfo` (syscall 99): fills a 112-byte standard Linux `struct sysinfo`.
pub fn sys_sysinfo(info_addr: u64) -> i64 {
    if info_addr == 0 || validate_user_buffer(info_addr, 112, true).is_err() {
        return -14i64; // -EFAULT
    }
    let ticks = ostd::arch::idt::timer_ticks();
    let uptime_sec = (ticks / 100) as i64;
    let stats = ostd::mm::memory_stats();
    let totalram = stats.total_bytes() as u64;
    let freeram = stats.free_bytes() as u64;
    let bufferram = ostd::mm::heap_bytes() as u64;

    let mut buf = [0u8; 112];
    buf[0..8].copy_from_slice(&uptime_sec.to_le_bytes());
    buf[32..40].copy_from_slice(&totalram.to_le_bytes());
    buf[40..48].copy_from_slice(&freeram.to_le_bytes());
    buf[56..64].copy_from_slice(&bufferram.to_le_bytes());
    let procs: u16 = 3;
    buf[80..82].copy_from_slice(&procs.to_le_bytes());
    let mem_unit: u32 = 1;
    buf[104..108].copy_from_slice(&mem_unit.to_le_bytes());

    if write_user_bytes(info_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}
