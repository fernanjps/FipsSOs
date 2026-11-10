#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};
use ostd::mm::validate_user_buffer;

use super::GLOBAL_VFS;

/// Linux `sys_uname` (syscall 63): returns system identification in `struct utsname` (390 bytes).
pub fn sys_uname(buf_addr: u64) -> i64 {
    let mut uts = [0u8; 390];
    let sysname = b"FipSOs\0";
    let nodename = b"fipsos-pc\0";
    let release = b"2.6.0\0";
    let version = b"#1 SMP\0";
    let machine = b"x86_64\0";
    let domain = b"(none)\0";

    uts[0..sysname.len()].copy_from_slice(sysname);
    uts[65..65 + nodename.len()].copy_from_slice(nodename);
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


/// Linux `sys_getrandom` (syscall 318): fills user buffer with cryptographically pseudo-random bytes.
pub fn sys_getrandom(buf_addr: u64, buflen: usize, _flags: u64) -> i64 {
    let mut rnd = [0u8; 256];
    let to_fill = buflen.min(256);
    let mut seed = ostd::arch::idt::timer_ticks().wrapping_mul(6364136223846793005).wrapping_add(1);
    for b in rnd[..to_fill].iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (seed >> 32) as u8;
    }
    if write_user_bytes(buf_addr, &rnd[..to_fill]).is_err() {
        -14 // -EFAULT
    } else {
        to_fill as i64
    }
}


/// Linux `sys_clock_gettime` (228) implementation writing a 16-byte `timespec` (`tv_sec`, `tv_nsec`).
pub fn sys_clock_gettime(clk_id: u64, tp_user_addr: u64) -> i64 {
    if tp_user_addr == 0 {
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

    if ostd::arch::syscall::write_user_bytes(tp_user_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}


/// Linux `sys_sysinfo` (syscall 99): fills a 112-byte standard Linux `struct sysinfo`.
pub fn sys_sysinfo(info_addr: u64) -> i64 {
    let ticks = ostd::arch::idt::timer_ticks();
    let uptime_sec = (ticks / 100) as i64;
    let stats = ostd::mm::memory_stats();
    let totalram = stats.total_bytes() as u64;
    let freeram = stats.free_bytes() as u64;

    let mut buf = [0u8; 112];
    // uptime (offset 0..8, i64)
    buf[0..8].copy_from_slice(&uptime_sec.to_le_bytes());
    // loads[3] (offset 8..32, u64*3) -> leave 0
    // totalram (offset 32..40, u64)
    buf[32..40].copy_from_slice(&totalram.to_le_bytes());
    // freeram (offset 40..48, u64)
    buf[40..48].copy_from_slice(&freeram.to_le_bytes());
    // sharedram (offset 48..56, u64) -> 0
    // bufferram (offset 56..64, u64) -> 0
    // totalswap (offset 64..72, u64) -> 0
    // freeswap (offset 72..80, u64) -> 0
    // procs (offset 80..82, u16)
    let procs: u16 = 3;
    buf[80..82].copy_from_slice(&procs.to_le_bytes());
    // pad (offset 82..84, u16) -> 0
    // padding 84..88 -> 0
    // totalhigh (offset 88..96, u64) -> 0
    // freehigh (offset 96..104, u64) -> 0
    // mem_unit (offset 104..108, u32) -> 1 byte
    let mem_unit: u32 = 1;
    buf[104..108].copy_from_slice(&mem_unit.to_le_bytes());

    if ostd::arch::syscall::write_user_bytes(info_addr, &buf).is_err() {
        return -14i64; // -EFAULT
    }
    0
}


/// Linux `sys_socket` (syscall 41): creates an unbound endpoint for communication.
pub fn sys_socket(domain: u64, type_: u64, protocol: u64) -> i64 {
    const AF_UNSPEC: u64 = 0;
    const AF_INET: u64 = 2;
    const SOCK_STREAM: u64 = 1;

    let base_type = type_ & 0x0F;
    if domain != AF_INET && domain != AF_UNSPEC {
        ostd::early_println!("[linux_abi::sys_socket] Dominio no soportado: {}", domain);
        return -97; // -EAFNOSUPPORT
    }
    if base_type != SOCK_STREAM {
        ostd::early_println!("[linux_abi::sys_socket] Tipo de socket no soportado: {}", base_type);
        return -93; // -EPROTONOSUPPORT
    }

    match GLOBAL_VFS.lock().create_socket_node() {
        Ok(fd) => {
            ostd::early_println!("[linux_abi::sys_socket] socket(AF_INET, SOCK_STREAM, proto={}) -> fd={}", protocol, fd);
            fd as i64
        }
        Err(e) => e,
    }
}


/// Linux `sys_connect` (syscall 42): connects a socket to a remote IPv4 address & TCP port.
pub fn sys_connect(fd: u64, addr_user_vaddr: u64, addr_len: u64) -> i64 {
    if addr_len < 8 {
        return -22; // -EINVAL
    }
    let mut buf = [0u8; 16];
    if read_user_bytes(addr_user_vaddr, &mut buf).is_err() {
        return -14; // -EFAULT
    }
    let sin_family = u16::from_le_bytes([buf[0], buf[1]]);
    if sin_family != 2 {
        return -97; // -EAFNOSUPPORT
    }
    let sin_port = u16::from_be_bytes([buf[2], buf[3]]);
    let sin_addr = [buf[4], buf[5], buf[6], buf[7]];

    let sock_id = match GLOBAL_VFS.lock().get_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };

    ostd::early_println!(
        "[linux_abi::sys_connect] connect(fd={}, {}.{}.{}.{}:{})",
        fd,
        sin_addr[0], sin_addr[1], sin_addr[2], sin_addr[3],
        sin_port
    );

    match ostd::net::tcp::connect(sock_id, sin_addr, sin_port) {
        Ok(_) => 0,
        Err(e) => e,
    }
}


/// Linux `sys_sendto` (syscall 44): sends data on a socket.
pub fn sys_sendto(
    fd: u64,
    buf_user_vaddr: u64,
    len: usize,
    _flags: u64,
    _dest_addr: u64,
    _addr_len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let max_len = len.min(16384);
    if validate_user_buffer(buf_user_vaddr, max_len, false).is_err() {
        return -14; // -EFAULT
    }
    let mut kernel_buf = alloc::vec![0u8; max_len];
    if read_user_bytes(buf_user_vaddr, &mut kernel_buf).is_err() {
        return -14; // -EFAULT
    }
    match GLOBAL_VFS.lock().write_file(fd, &kernel_buf) {
        Ok(n) => n as i64,
        Err(e) => e,
    }
}


/// Linux `sys_recvfrom` (syscall 45): receives data from a socket.
pub fn sys_recvfrom(
    fd: u64,
    buf_user_vaddr: u64,
    len: usize,
    _flags: u64,
    _src_addr: u64,
    _addr_len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let max_len = len.min(16384);
    if validate_user_buffer(buf_user_vaddr, max_len, true).is_err() {
        return -14; // -EFAULT
    }
    let mut kernel_buf = alloc::vec![0u8; max_len];
    let bytes_read = match GLOBAL_VFS.lock().read_file(fd, &mut kernel_buf) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if bytes_read > 0 && write_user_bytes(buf_user_vaddr, &kernel_buf[..bytes_read]).is_err() {
        return -14; // -EFAULT
    }
    bytes_read as i64
}


/// Linux `sys_setsockopt` (syscall 54): sets socket options (SO_RCVTIMEO, SO_KEEPALIVE, etc.).
pub fn sys_setsockopt(fd: u64, level: u64, optname: u64, _optval: u64, _optlen: u64) -> i64 {
    ostd::early_println!("[linux_abi::sys_setsockopt] setsockopt(fd={}, lvl={}, opt={}) -> 0", fd, level, optname);
    0
}


/// Linux `sys_getsockopt` (syscall 55): gets socket options.
pub fn sys_getsockopt(fd: u64, level: u64, optname: u64, _optval: u64, _optlen: u64) -> i64 {
    ostd::early_println!("[linux_abi::sys_getsockopt] getsockopt(fd={}, lvl={}, opt={}) -> 0", fd, level, optname);
    0
}


/// Linux `sys_getsockname` (syscall 51): returns local socket address.
pub fn sys_getsockname(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    if addr_user_vaddr == 0 {
        return 0;
    }
    let mut sin = [0u8; 16];
    sin[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
    sin[2..4].copy_from_slice(&49152u16.to_be_bytes());
    sin[4..8].copy_from_slice(&[10, 0, 2, 15]);
    let _ = write_user_bytes(addr_user_vaddr, &sin);
    if addr_len_user != 0 {
        let _ = write_user_bytes(addr_len_user, &16u32.to_le_bytes());
    }
    0
}


/// Linux `sys_getpeername` (syscall 52): returns remote peer address.
pub fn sys_getpeername(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    if addr_user_vaddr == 0 {
        return 0;
    }
    let mut sin = [0u8; 16];
    sin[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
    sin[2..4].copy_from_slice(&8080u16.to_be_bytes());
    sin[4..8].copy_from_slice(&[10, 0, 2, 2]);
    let _ = write_user_bytes(addr_user_vaddr, &sin);
    if addr_len_user != 0 {
        let _ = write_user_bytes(addr_len_user, &16u32.to_le_bytes());
    }
    0
}


/// Linux `sys_poll` (syscall 7 / 271): polls a set of file descriptors for readiness.
pub fn sys_poll(fds_ptr: u64, nfds: u64, _timeout: i64) -> i64 {
    if nfds == 0 || fds_ptr == 0 {
        return 0;
    }
    const POLLIN: i16 = 0x0001;
    const POLLOUT: i16 = 0x0004;
    let mut ready_count = 0i64;
    for i in 0..nfds {
        let entry_addr = fds_ptr + i * 8;
        let mut buf = [0u8; 8];
        if read_user_bytes(entry_addr, &mut buf).is_err() {
            return -14; // -EFAULT
        }
        let events = i16::from_le_bytes([buf[4], buf[5]]);
        let revents = events & (POLLIN | POLLOUT);
        let revents_bytes = revents.to_le_bytes();
        buf[6] = revents_bytes[0];
        buf[7] = revents_bytes[1];
        if write_user_bytes(entry_addr, &buf).is_err() {
            return -14;
        }
        if revents != 0 {
            ready_count += 1;
        }
    }
    ready_count
}


/// Linux `sys_select` (syscall 23 / 270): synchronous I/O multiplexing.
pub fn sys_select(nfds: u64, _readfds: u64, _writefds: u64, _exceptfds: u64) -> i64 {
    if nfds == 0 {
        0
    } else {
        1
    }
}
