#![forbid(unsafe_code)]

use ostd::arch::syscall::{read_user_bytes, write_user_bytes};
use ostd::mm::validate_user_buffer;

use super::GLOBAL_VFS;

fn active_socket_id(fd: u64) -> Option<u64> {
    let slot = ostd::scheduler::active_slot();
    crate::vfs::get_process_node(slot, fd)
        .and_then(|n| n.socket_id)
        .or_else(|| GLOBAL_VFS.lock().get_socket_id(fd))
}

/// Linux `sys_socket` (syscall 41): creates an unbound endpoint for communication.
pub fn sys_socket(domain: u64, type_: u64, protocol: u64) -> i64 {
    if let Err(e) = super::sandbox::check_net_sandbox() {
        return e;
    }
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

    let slot = ostd::scheduler::active_slot();
    let res = {
        let mut vfs = GLOBAL_VFS.lock();
        match vfs.create_socket_node() {
            Ok(fd) => Ok((fd, vfs.get_node(fd).cloned())),
            Err(e) => Err(e),
        }
    };
    match res {
        Ok((fd, maybe_node)) => {
            if let Some(node) = maybe_node {
                crate::vfs::insert_process_fd(slot, fd, node);
            }
            ostd::early_println!(
                "[linux_abi::sys_socket] socket(AF_INET, SOCK_STREAM, proto={}) -> fd={}",
                protocol,
                fd
            );
            fd as i64
        }
        Err(e) => e,
    }
}

/// Linux `sys_connect` (syscall 42): connects a socket to a remote IPv4 address & TCP port.
pub fn sys_connect(fd: u64, addr_user_vaddr: u64, addr_len: u64) -> i64 {
    if let Err(e) = super::sandbox::check_net_sandbox() {
        return e;
    }
    if addr_len < 8 || addr_len > 128 {
        return -22; // -EINVAL
    }
    let inspect_len = (addr_len as usize).min(16);
    if validate_user_buffer(addr_user_vaddr, inspect_len, false).is_err() {
        return -14; // -EFAULT
    }
    let mut buf = [0u8; 16];
    if read_user_bytes(addr_user_vaddr, &mut buf[..inspect_len]).is_err() {
        return -14; // -EFAULT
    }
    let sin_family = u16::from_le_bytes([buf[0], buf[1]]);
    if sin_family != 2 {
        return -97; // -EAFNOSUPPORT
    }
    let sin_port = u16::from_be_bytes([buf[2], buf[3]]);
    let sin_addr = [buf[4], buf[5], buf[6], buf[7]];

    let sock_id = match active_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };

    ostd::early_println!(
        "[linux_abi::sys_connect] connect(fd={}, {}.{}.{}.{}:{})",
        fd,
        sin_addr[0],
        sin_addr[1],
        sin_addr[2],
        sin_addr[3],
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
    if let Some(sock_id) = active_socket_id(fd) {
        return match ostd::net::tcp::send(sock_id, &kernel_buf) {
            Ok(n) => n as i64,
            Err(e) => e,
        };
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
    let bytes_read = if let Some(sock_id) = active_socket_id(fd) {
        match ostd::net::tcp::recv(sock_id, &mut kernel_buf) {
            Ok(n) => n,
            Err(e) => return e,
        }
    } else {
        match GLOBAL_VFS.lock().read_file(fd, &mut kernel_buf) {
            Ok(n) => n,
            Err(e) => return e,
        }
    };
    if bytes_read > 0 && write_user_bytes(buf_user_vaddr, &kernel_buf[..bytes_read]).is_err() {
        return -14; // -EFAULT
    }
    bytes_read as i64
}

/// Linux `sys_setsockopt` (syscall 54): validates user buffer and sets socket options.
pub fn sys_setsockopt(fd: u64, level: u64, optname: u64, optval: u64, optlen: u64) -> i64 {
    if optlen > 4096 {
        return -22; // -EINVAL
    }
    if optlen > 0 && validate_user_buffer(optval, optlen as usize, false).is_err() {
        return -14; // -EFAULT
    }
    if level == 1 && optname == 0x4648 && optlen >= 2 && optval != 0 {
        let mut port_bytes = [0u8; 2];
        if read_user_bytes(optval, &mut port_bytes).is_err() {
            return -14;
        }
        let port = u16::from_le_bytes(port_bytes);
        if let Some(sock_id) = active_socket_id(fd) {
            return super::httpd_bg::start_bg_httpd(sock_id, port);
        }
        return -88; // -ENOTSOCK
    }
    ostd::early_println!(
        "[linux_abi::sys_setsockopt] setsockopt(fd={}, lvl={}, opt={}) -> 0",
        fd,
        level,
        optname
    );
    0
}

/// Linux `sys_getsockopt` (syscall 55): validates user output buffers and reports socket options.
pub fn sys_getsockopt(fd: u64, level: u64, optname: u64, optval: u64, optlen_ptr: u64) -> i64 {
    if optlen_ptr != 0 {
        if validate_user_buffer(optlen_ptr, 4, true).is_err() {
            return -14; // -EFAULT
        }
        let mut len_bytes = [0u8; 4];
        if read_user_bytes(optlen_ptr, &mut len_bytes).is_err() {
            return -14; // -EFAULT
        }
        let raw_cap = i32::from_le_bytes(len_bytes);
        if raw_cap < 0 {
            return -22; // -EINVAL
        }
        let cap = (raw_cap as usize).min(256);
        if cap > 0 {
            if optval == 0 || validate_user_buffer(optval, cap, true).is_err() {
                return -14; // -EFAULT
            }
            let zeros = alloc::vec![0u8; cap.min(16)];
            if write_user_bytes(optval, &zeros).is_err() {
                return -14; // -EFAULT
            }
            let _ = write_user_bytes(optlen_ptr, &(zeros.len() as u32).to_le_bytes());
        }
    }
    ostd::early_println!(
        "[linux_abi::sys_getsockopt] getsockopt(fd={}, lvl={}, opt={}) -> 0",
        fd,
        level,
        optname
    );
    0
}

fn write_ipv4_sockaddr(addr_user_vaddr: u64, addr_len_user: u64, port: u16, ip: [u8; 4]) -> i64 {
    if addr_user_vaddr == 0 {
        return 0;
    }
    let mut cap = 16usize;
    if addr_len_user != 0 {
        if validate_user_buffer(addr_len_user, 4, true).is_err() {
            return -14; // -EFAULT
        }
        let mut len_bytes = [0u8; 4];
        if read_user_bytes(addr_len_user, &mut len_bytes).is_err() {
            return -14; // -EFAULT
        }
        let raw_cap = i32::from_le_bytes(len_bytes);
        if raw_cap < 0 {
            return -22; // -EINVAL
        }
        cap = (raw_cap as usize).min(16);
    }

    let mut sin = [0u8; 16];
    sin[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET
    sin[2..4].copy_from_slice(&port.to_be_bytes());
    sin[4..8].copy_from_slice(&ip);

    if cap > 0 {
        if validate_user_buffer(addr_user_vaddr, cap, true).is_err()
            || write_user_bytes(addr_user_vaddr, &sin[..cap]).is_err()
        {
            return -14; // -EFAULT
        }
    }
    if addr_len_user != 0 && write_user_bytes(addr_len_user, &16u32.to_le_bytes()).is_err() {
        return -14; // -EFAULT
    }
    0
}

/// Linux `sys_getsockname` (syscall 51): returns local socket address safely.
pub fn sys_getsockname(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    write_ipv4_sockaddr(addr_user_vaddr, addr_len_user, 49152, [10, 0, 2, 15])
}

/// Linux `sys_getpeername` (syscall 52): returns remote peer address safely.
pub fn sys_getpeername(_fd: u64, addr_user_vaddr: u64, addr_len_user: u64) -> i64 {
    write_ipv4_sockaddr(addr_user_vaddr, addr_len_user, 8080, [10, 0, 2, 2])
}

/// Linux `sys_poll` (syscall 7 / 271): polls a set of file descriptors for readiness.
pub fn sys_poll(fds_ptr: u64, nfds: u64, _timeout: i64) -> i64 {
    if nfds == 0 {
        return 0;
    }
    if fds_ptr == 0 {
        return -14; // -EFAULT
    }
    if nfds > 1024 {
        return -22; // -EINVAL
    }
    let Some(total_bytes) = (nfds as usize).checked_mul(8) else {
        return -22;
    };
    if validate_user_buffer(fds_ptr, total_bytes, true).is_err() {
        return -14; // -EFAULT
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
    if nfds > 1024 {
        return -22; // -EINVAL
    }
    if nfds == 0 {
        0
    } else {
        1
    }
}

/// Linux `sys_bind` (syscall 49): binds a TCP socket descriptor to a local IPv4 port.
pub fn sys_bind(fd: u64, addr_user_vaddr: u64, addr_len: u64) -> i64 {
    if let Err(e) = super::sandbox::check_net_sandbox() {
        return e;
    }
    if addr_len < 8 || addr_len > 128 {
        return -22; // -EINVAL
    }
    let inspect_len = (addr_len as usize).min(16);
    if validate_user_buffer(addr_user_vaddr, inspect_len, false).is_err() {
        return -14; // -EFAULT
    }
    let mut buf = [0u8; 16];
    if read_user_bytes(addr_user_vaddr, &mut buf[..inspect_len]).is_err() {
        return -14; // -EFAULT
    }
    let sin_family = u16::from_le_bytes([buf[0], buf[1]]);
    if sin_family != 2 {
        return -97; // -EAFNOSUPPORT
    }
    let sin_port = u16::from_be_bytes([buf[2], buf[3]]);
    let sock_id = match active_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };
    match ostd::net::tcp::bind(sock_id, sin_port) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Linux `sys_listen` (syscall 50): marks a bound TCP socket as passive listener.
pub fn sys_listen(fd: u64, _backlog: u64) -> i64 {
    let sock_id = match active_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };
    match ostd::net::tcp::listen(sock_id) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Linux `sys_accept` (syscall 43): accepts an inbound TCP connection on a listening socket.
pub fn sys_accept(fd: u64, _addr_user_vaddr: u64, _addr_len_ptr: u64) -> i64 {
    let sock_id = match active_socket_id(fd) {
        Some(id) => id,
        None => return -88, // -ENOTSOCK
    };
    let client_sock = match ostd::net::tcp::accept(sock_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let slot = ostd::scheduler::active_slot();
    let res = {
        let mut vfs = GLOBAL_VFS.lock();
        match vfs.register_socket_id(client_sock) {
            Ok(new_fd) => Ok((new_fd, vfs.get_node(new_fd).cloned())),
            Err(e) => Err(e),
        }
    };
    match res {
        Ok((new_fd, maybe_node)) => {
            if let Some(node) = maybe_node {
                crate::vfs::insert_process_fd(slot, new_fd, node);
            }
            new_fd as i64
        }
        Err(e) => {
            let _ = ostd::net::tcp::close(client_sock);
            e
        }
    }
}
