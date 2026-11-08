//! FipSOs Ring 0 TCP State Machine & Socket Transport (`ostd::net::tcp`):
//! RFC 793 Three-Way Handshake, RFC 1071 Pseudo-Header Checksum, PSH/ACK Stream
//! Transmission & Orderly FIN/ACK Teardown over Intel `e1000e` PCIe Bus Master DMA.

use alloc::collections::{BTreeMap, VecDeque};
use core::sync::atomic::{AtomicU16, AtomicU64, Ordering};

use crate::sync::SpinLock;
use super::e1000::PKT_BUF_SIZE;
use super::{
    build_arp_frame, inspect_and_answer_arp, l4_pseudo_checksum, read_tsc,
    ETHERTYPE_IPV4, FALLBACK_SLIRP_GW_MAC, GATEWAY_IPV4, IPPROTO_TCP, LOCAL_IPV4,
    NET_STACK,
};

pub const TCP_FLAG_FIN: u8 = 0x01;
pub const TCP_FLAG_SYN: u8 = 0x02;
pub const TCP_FLAG_RST: u8 = 0x04;
pub const TCP_FLAG_PSH: u8 = 0x08;
pub const TCP_FLAG_ACK: u8 = 0x10;
pub const TCP_FLAG_URG: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    SynSent,
    Established,
    CloseWait,
}

pub struct TcpConnection {
    pub socket_id: u64,
    pub state: TcpState,
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub local_mac: [u8; 6],
    pub remote_mac: [u8; 6],
    pub snd_nxt: u32,
    pub rcv_nxt: u32,
    pub rx_buffer: VecDeque<u8>,
    pub is_eof: bool,
}

static TCP_REGISTRY: SpinLock<BTreeMap<u64, TcpConnection>> = SpinLock::new(BTreeMap::new());
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(49152);

#[derive(Debug, Clone, Copy)]
pub struct TcpParsedHeader {
    pub seq_num: u32,
    pub ack_num: u32,
    pub flags: u8,
    pub window: u16,
}

#[allow(clippy::too_many_arguments)]
pub fn build_tcp_frame(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
    out: &mut [u8],
) -> usize {
    let tcp_hdr_len = 20usize;
    let ip_total_len = 20 + tcp_hdr_len + payload.len();
    let total_len = 14 + ip_total_len;

    out[0..6].copy_from_slice(&dst_mac);
    out[6..12].copy_from_slice(&src_mac);
    out[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let ip = &mut out[14..34];
    ip.fill(0);
    ip[0] = 0x45; // Version 4, IHL 5 (20 bytes)
    ip[2..4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&((seq & 0xFFFF) as u16).to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't Fragment
    ip[8] = 64; // TTL
    ip[9] = IPPROTO_TCP;
    ip[12..16].copy_from_slice(&src_ip);
    ip[16..20].copy_from_slice(&dst_ip);
    let ip_csum = super::internet_checksum(ip);
    ip[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    let tcp = &mut out[34..total_len];
    tcp.fill(0);
    tcp[0..2].copy_from_slice(&src_port.to_be_bytes());
    tcp[2..4].copy_from_slice(&dst_port.to_be_bytes());
    tcp[4..8].copy_from_slice(&seq.to_be_bytes());
    tcp[8..12].copy_from_slice(&ack.to_be_bytes());
    tcp[12] = 5 << 4; // Data Offset = 5 (20 bytes)
    tcp[13] = flags;
    tcp[14..16].copy_from_slice(&window.to_be_bytes());
    if !payload.is_empty() {
        tcp[20..20 + payload.len()].copy_from_slice(payload);
    }
    let tcp_csum = l4_pseudo_checksum(src_ip, dst_ip, IPPROTO_TCP, tcp);
    tcp[16..18].copy_from_slice(&tcp_csum.to_be_bytes());

    total_len
}

pub fn parse_tcp_frame<'a>(
    frame: &'a [u8],
    expected_local_ip: [u8; 4],
    expected_remote_ip: [u8; 4],
    expected_local_port: u16,
    expected_remote_port: u16,
) -> Option<(TcpParsedHeader, &'a [u8])> {
    if frame.len() < 54 {
        return None;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_IPV4 {
        return None;
    }
    let ip_hdr = &frame[14..];
    let version = ip_hdr[0] >> 4;
    let ihl = ((ip_hdr[0] & 0x0F) as usize) * 4;
    if version != 4 || ihl < 20 || ip_hdr.len() < ihl + 20 {
        return None;
    }
    let ip_total_len = u16::from_be_bytes([ip_hdr[2], ip_hdr[3]]) as usize;
    if ip_hdr.len() < ip_total_len || ip_total_len < ihl + 20 {
        return None;
    }
    let proto = ip_hdr[9];
    if proto != IPPROTO_TCP {
        return None;
    }
    let src_ip = [ip_hdr[12], ip_hdr[13], ip_hdr[14], ip_hdr[15]];
    let dst_ip = [ip_hdr[16], ip_hdr[17], ip_hdr[18], ip_hdr[19]];
    if src_ip != expected_remote_ip || dst_ip != expected_local_ip {
        return None;
    }

    let tcp_hdr = &ip_hdr[ihl..];
    let src_port = u16::from_be_bytes([tcp_hdr[0], tcp_hdr[1]]);
    let dst_port = u16::from_be_bytes([tcp_hdr[2], tcp_hdr[3]]);
    if src_port != expected_remote_port || dst_port != expected_local_port {
        return None;
    }

    let seq_num = u32::from_be_bytes([tcp_hdr[4], tcp_hdr[5], tcp_hdr[6], tcp_hdr[7]]);
    let ack_num = u32::from_be_bytes([tcp_hdr[8], tcp_hdr[9], tcp_hdr[10], tcp_hdr[11]]);
    let data_offset = ((tcp_hdr[12] >> 4) as usize) * 4;
    if data_offset < 20 || ihl + data_offset > ip_total_len {
        return None;
    }
    let flags = tcp_hdr[13];
    let window = u16::from_be_bytes([tcp_hdr[14], tcp_hdr[15]]);

    let payload_len = ip_total_len - (ihl + data_offset);
    let payload = &ip_hdr[ihl + data_offset..ihl + data_offset + payload_len];

    Some((
        TcpParsedHeader {
            seq_num,
            ack_num,
            flags,
            window,
        },
        payload,
    ))
}

/// Allocates a new TCP socket handle with an ephemeral port (49152..65000).
pub fn create_socket() -> Result<u64, i64> {
    let sock_id = NEXT_SOCKET_ID.fetch_add(1, Ordering::SeqCst);
    let mut local_port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::SeqCst);
    if local_port >= 65000 {
        NEXT_EPHEMERAL_PORT.store(49152, Ordering::SeqCst);
        local_port = 49152;
    }

    let conn = TcpConnection {
        socket_id: sock_id,
        state: TcpState::Closed,
        local_ip: LOCAL_IPV4,
        local_port,
        remote_ip: [0; 4],
        remote_port: 0,
        local_mac: [0; 6],
        remote_mac: [0; 6],
        snd_nxt: 0,
        rcv_nxt: 0,
        rx_buffer: VecDeque::with_capacity(8192),
        is_eof: false,
    };

    TCP_REGISTRY.lock().insert(sock_id, conn);
    crate::early_println!("[ostd::net::tcp] Socket creado: ID={} (Puerto local efimero={})", sock_id, local_port);
    Ok(sock_id)
}

/// Executes the RFC 793 Three-Way Handshake (SYN -> SYN-ACK -> ACK) over `e1000e` DMA.
pub fn connect(socket_id: u64, remote_ip: [u8; 4], remote_port: u16) -> Result<(), i64> {
    let (src_mac, dst_mac) = {
        let mut guard = NET_STACK.lock();
        let src = guard.nic.mac_address();
        let dst = match guard.gateway_mac {
            Some(mac) => mac,
            None => {
                let arp_req = build_arp_frame(
                    src,
                    LOCAL_IPV4,
                    [0xFF; 6],
                    [0x00; 6],
                    GATEWAY_IPV4,
                    1,
                );
                let _ = guard.nic.transmit_frame(&arp_req);
                let mut rx_buf = [0u8; PKT_BUF_SIZE];
                let mut resolved = None;
                for _ in 0..10_000 {
                    if let Some(len) = guard.nic.receive_frame(&mut rx_buf) {
                        if let Some(gw_mac) = inspect_and_answer_arp(&mut guard.nic, &rx_buf[..len], src) {
                            guard.gateway_mac = Some(gw_mac);
                            resolved = Some(gw_mac);
                            break;
                        }
                    }
                    core::hint::spin_loop();
                }
                resolved.unwrap_or(FALLBACK_SLIRP_GW_MAC)
            }
        };
        (src, dst)
    };

    let (local_port, isn) = {
        let mut registry = TCP_REGISTRY.lock();
        let conn = registry.get_mut(&socket_id).ok_or(-9i64)?; // -EBADF
        conn.remote_ip = remote_ip;
        conn.remote_port = remote_port;
        conn.local_mac = src_mac;
        conn.remote_mac = dst_mac;

        let isn: u32 = ((read_tsc() as u32) ^ 0x5F19_5001) & 0x7FFF_FFFF;
        conn.snd_nxt = isn;
        (conn.local_port, isn)
    };

    crate::early_println!(
        "[ostd::net::tcp] Iniciando Three-Way Handshake TCP: 10.0.2.15:{} -> {}.{}.{}.{}:{} (ISN={:#x})",
        local_port,
        remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3],
        remote_port,
        isn
    );

    // 1. Transmit TCP SYN
    let mut syn_frame = [0u8; 128];
    let syn_len = build_tcp_frame(
        src_mac,
        dst_mac,
        LOCAL_IPV4,
        remote_ip,
        local_port,
        remote_port,
        isn,
        0,
        TCP_FLAG_SYN,
        16384,
        &[],
        &mut syn_frame,
    );

    {
        let mut guard = NET_STACK.lock();
        let _ = guard.nic.transmit_frame(&syn_frame[..syn_len]);
    }

    {
        let mut registry = TCP_REGISTRY.lock();
        if let Some(conn) = registry.get_mut(&socket_id) {
            conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
            conn.state = TcpState::SynSent;
        }
    }

    // 2. Poll RxDesc ring for TCP SYN-ACK
    let mut synack_received = false;
    let mut server_seq: u32 = 0;
    let mut rx_buf = [0u8; PKT_BUF_SIZE];

    for _ in 0..400_000 {
        let maybe_frame = {
            let mut guard = NET_STACK.lock();
            guard.nic.receive_frame(&mut rx_buf)
        };

        if let Some(pkt_len) = maybe_frame {
            let frame = &rx_buf[..pkt_len];
            {
                let mut guard = NET_STACK.lock();
                let _ = inspect_and_answer_arp(&mut guard.nic, frame, src_mac);
            }

            if let Some((tcp_hdr, _payload)) = parse_tcp_frame(
                frame,
                LOCAL_IPV4,
                remote_ip,
                local_port,
                remote_port,
            ) {
                if (tcp_hdr.flags & TCP_FLAG_RST) != 0 {
                    let mut registry = TCP_REGISTRY.lock();
                    if let Some(conn) = registry.get_mut(&socket_id) {
                        conn.state = TcpState::Closed;
                    }
                    crate::early_println!("[ostd::net::tcp] Conexion rechazada con RST por el servidor!");
                    return Err(-111); // -ECONNREFUSED
                }

                if (tcp_hdr.flags & (TCP_FLAG_SYN | TCP_FLAG_ACK)) == (TCP_FLAG_SYN | TCP_FLAG_ACK) {
                    server_seq = tcp_hdr.seq_num;
                    synack_received = true;
                    break;
                }
            }
        }
        core::hint::spin_loop();
    }

    if !synack_received {
        let mut registry = TCP_REGISTRY.lock();
        if let Some(conn) = registry.get_mut(&socket_id) {
            conn.state = TcpState::Closed;
        }
        crate::early_println!("[ostd::net::tcp] Timeout esperando SYN-ACK del servidor!");
        return Err(-110); // -ETIMEDOUT
    }

    // 3. Send TCP ACK to complete 3-way handshake and enter ESTABLISHED state
    let (snd_nxt, rcv_nxt) = {
        let mut registry = TCP_REGISTRY.lock();
        let conn = registry.get_mut(&socket_id).ok_or(-9i64)?;
        conn.rcv_nxt = server_seq.wrapping_add(1);
        conn.state = TcpState::Established;
        (conn.snd_nxt, conn.rcv_nxt)
    };

    let mut ack_frame = [0u8; 64];
    let ack_len = build_tcp_frame(
        src_mac,
        dst_mac,
        LOCAL_IPV4,
        remote_ip,
        local_port,
        remote_port,
        snd_nxt,
        rcv_nxt,
        TCP_FLAG_ACK,
        16384,
        &[],
        &mut ack_frame,
    );

    {
        let mut guard = NET_STACK.lock();
        let _ = guard.nic.transmit_frame(&ack_frame[..ack_len]);
    }

    crate::early_println!(
        "[ostd::net::tcp] Three-Way Handshake completado exitosamente: Estado ESTABLISHED (Seq={:#x}, Ack={:#x})",
        snd_nxt,
        rcv_nxt
    );

    Ok(())
}

/// Transmits payload data over an established TCP connection with `PSH+ACK` flags.
pub fn send(socket_id: u64, data: &[u8]) -> Result<usize, i64> {
    if data.is_empty() {
        return Ok(0);
    }
    let mut registry = TCP_REGISTRY.lock();
    let conn = registry.get_mut(&socket_id).ok_or(-9i64)?;
    if conn.state != TcpState::Established {
        return Err(-107); // -ENOTCONN
    }

    let src_mac = conn.local_mac;
    let dst_mac = conn.remote_mac;
    let local_port = conn.local_port;
    let remote_ip = conn.remote_ip;
    let remote_port = conn.remote_port;

    let max_mss = 1400usize;
    let mut total_sent = 0usize;

    for chunk in data.chunks(max_mss) {
        let mut frame = alloc::vec![0u8; 14 + 20 + 20 + chunk.len()];
        let wire_len = build_tcp_frame(
            src_mac,
            dst_mac,
            LOCAL_IPV4,
            remote_ip,
            local_port,
            remote_port,
            conn.snd_nxt,
            conn.rcv_nxt,
            TCP_FLAG_PSH | TCP_FLAG_ACK,
            16384,
            chunk,
            &mut frame,
        );

        let sent = {
            let mut guard = NET_STACK.lock();
            guard.nic.transmit_frame(&frame[..wire_len])
        };

        if !sent {
            if total_sent > 0 {
                return Ok(total_sent);
            } else {
                return Err(-5); // -EIO
            }
        }

        conn.snd_nxt = conn.snd_nxt.wrapping_add(chunk.len() as u32);
        total_sent += chunk.len();
    }

    crate::early_println!("[ostd::net::tcp] Enviados {} bytes TCP PSH+ACK -> Seq={:#x}", total_sent, conn.snd_nxt);
    Ok(total_sent)
}

/// Reads received stream data from the TCP buffer, polling `e1000e` DMA RX ring when necessary.
pub fn recv(socket_id: u64, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }

    let mut registry = TCP_REGISTRY.lock();
    let conn = registry.get_mut(&socket_id).ok_or(-9i64)?;

    // If data is already buffered, drain it immediately
    if !conn.rx_buffer.is_empty() {
        let to_copy = core::cmp::min(out.len(), conn.rx_buffer.len());
        for slot in out.iter_mut().take(to_copy) {
            *slot = conn.rx_buffer.pop_front().unwrap();
        }
        return Ok(to_copy);
    }

    if conn.is_eof {
        return Ok(0);
    }

    let local_mac = conn.local_mac;
    let remote_mac = conn.remote_mac;
    let remote_ip = conn.remote_ip;
    let local_port = conn.local_port;
    let remote_port = conn.remote_port;

    let mut rx_buf = [0u8; PKT_BUF_SIZE];

    for _ in 0..500_000 {
        let maybe_frame = {
            let mut guard = NET_STACK.lock();
            guard.nic.receive_frame(&mut rx_buf)
        };

        if let Some(pkt_len) = maybe_frame {
            let frame = &rx_buf[..pkt_len];
            {
                let mut guard = NET_STACK.lock();
                let _ = inspect_and_answer_arp(&mut guard.nic, frame, local_mac);
            }

            if let Some((tcp_hdr, payload)) = parse_tcp_frame(
                frame,
                LOCAL_IPV4,
                remote_ip,
                local_port,
                remote_port,
            ) {
                let mut need_ack = false;

                if !payload.is_empty() {
                    if tcp_hdr.seq_num == conn.rcv_nxt {
                        conn.rcv_nxt = conn.rcv_nxt.wrapping_add(payload.len() as u32);
                        conn.rx_buffer.extend(payload.iter().copied());
                        need_ack = true;
                    } else if tcp_hdr.seq_num < conn.rcv_nxt {
                        need_ack = true;
                    }
                }

                if (tcp_hdr.flags & TCP_FLAG_FIN) != 0 {
                    if !conn.is_eof {
                        conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                        conn.is_eof = true;
                        conn.state = TcpState::CloseWait;
                        need_ack = true;
                    }
                }

                if need_ack {
                    let mut ack_frame = [0u8; 64];
                    let ack_len = build_tcp_frame(
                        local_mac,
                        remote_mac,
                        LOCAL_IPV4,
                        remote_ip,
                        local_port,
                        remote_port,
                        conn.snd_nxt,
                        conn.rcv_nxt,
                        TCP_FLAG_ACK,
                        16384,
                        &[],
                        &mut ack_frame,
                    );
                    let mut guard = NET_STACK.lock();
                    let _ = guard.nic.transmit_frame(&ack_frame[..ack_len]);
                }

                if !conn.rx_buffer.is_empty() || conn.is_eof {
                    break;
                }
            }
        }
        core::hint::spin_loop();
    }

    if !conn.rx_buffer.is_empty() {
        let to_copy = core::cmp::min(out.len(), conn.rx_buffer.len());
        for slot in out.iter_mut().take(to_copy) {
            *slot = conn.rx_buffer.pop_front().unwrap();
        }
        crate::early_println!("[ostd::net::tcp] Recibidos {} bytes del flujo TCP", to_copy);
        Ok(to_copy)
    } else if conn.is_eof {
        crate::early_println!("[ostd::net::tcp] Fin de flujo TCP (EOF detectado)");
        Ok(0)
    } else {
        Ok(0)
    }
}

/// Closes the TCP socket connection with an orderly `FIN+ACK` exchange.
pub fn close(socket_id: u64) -> Result<(), i64> {
    let mut registry = TCP_REGISTRY.lock();
    if let Some(mut conn) = registry.remove(&socket_id) {
        if conn.state == TcpState::Established || conn.state == TcpState::CloseWait {
            let mut fin_frame = [0u8; 64];
            let fin_len = build_tcp_frame(
                conn.local_mac,
                conn.remote_mac,
                LOCAL_IPV4,
                conn.remote_ip,
                conn.local_port,
                conn.remote_port,
                conn.snd_nxt,
                conn.rcv_nxt,
                TCP_FLAG_FIN | TCP_FLAG_ACK,
                16384,
                &[],
                &mut fin_frame,
            );
            let mut guard = NET_STACK.lock();
            let _ = guard.nic.transmit_frame(&fin_frame[..fin_len]);
            conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
            conn.state = TcpState::Closed;
            crate::early_println!("[ostd::net::tcp] Socket ID={} cerrado con FIN+ACK", socket_id);
        }
    }
    Ok(())
}
