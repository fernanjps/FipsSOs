//! FipSOs Ring 0 Passive TCP Listener (`bind` / `listen` / `accept`) for Ring 3 servers.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::sync::SpinLock;
use super::arp_ipv4::{
    build_tcp_frame, inspect_and_answer_arp, parse_tcp_frame, TcpParsedHeader,
};
use super::e1000::PKT_BUF_SIZE;
use super::tcp_client::{
    insert_accepted_connection, TCP_FLAG_ACK, TCP_FLAG_SYN,
};
use super::{current_ipv4, read_tsc, NET_STACK};

#[derive(Debug, Clone, Copy)]
struct ListenerState {
    port: u16,
    listening: bool,
}

static LISTENERS: SpinLock<BTreeMap<u64, ListenerState>> = SpinLock::new(BTreeMap::new());

/// Binds a socket ID to a local TCP `port`.
pub fn bind(socket_id: u64, port: u16) -> Result<(), i64> {
    if port == 0 {
        return Err(-22); // -EINVAL
    }
    let mut listeners = LISTENERS.lock();
    if listeners.values().any(|l| l.port == port) {
        return Err(-98); // -EADDRINUSE
    }
    listeners.insert(socket_id, ListenerState { port, listening: false });
    crate::early_println!("[ostd::net::tcp_server] bind(sock={}, port={})", socket_id, port);
    Ok(())
}

/// Removes a bound/listening socket from `LISTENERS` when closed.
pub fn unbind(socket_id: u64) {
    LISTENERS.lock().remove(&socket_id);
}

/// Marks a bound socket ID as passive listener.
pub fn listen(socket_id: u64) -> Result<(), i64> {
    let mut listeners = LISTENERS.lock();
    let st = listeners.get_mut(&socket_id).ok_or(-22i64)?;
    st.listening = true;
    crate::early_println!("[ostd::net::tcp_server] listen(sock={}, port={})", socket_id, st.port);
    Ok(())
}

/// Extracts `(src_mac, src_ip, src_port, dst_port, parsed_hdr, payload)` for any TCP packet
/// addressed to `LOCAL_IPV4` and `expected_local_port`.
fn match_inbound_tcp<'a>(
    frame: &'a [u8],
    expected_local_port: u16,
) -> Option<([u8; 6], [u8; 4], u16, TcpParsedHeader, &'a [u8])> {
    if frame.len() < 54 {
        return None;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != 0x0800 {
        return None;
    }
    let ip_hdr = &frame[14..];
    let ihl = ((ip_hdr[0] & 0x0F) as usize) * 4;
    if ihl < 20 || ip_hdr.len() < ihl + 20 || ip_hdr[9] != 6 {
        return None;
    }
    let src_ip = [ip_hdr[12], ip_hdr[13], ip_hdr[14], ip_hdr[15]];
    let dst_ip = [ip_hdr[16], ip_hdr[17], ip_hdr[18], ip_hdr[19]];
    let local_ip = current_ipv4();
    if dst_ip != local_ip && dst_ip != [10, 0, 2, 15] {
        return None;
    }
    let tcp_hdr = &ip_hdr[ihl..];
    let src_port = u16::from_be_bytes([tcp_hdr[0], tcp_hdr[1]]);
    let dst_port = u16::from_be_bytes([tcp_hdr[2], tcp_hdr[3]]);
    if dst_port != expected_local_port {
        return None;
    }
    let src_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
    let (parsed, payload) =
        parse_tcp_frame(frame, dst_ip, src_ip, expected_local_port, src_port)?;
    Some((src_mac, src_ip, src_port, parsed, payload))
}

/// Blocks until an inbound TCP client completes the 3-way handshake on `listener_sock_id`,
/// returning the newly accepted `client_sock_id`.
pub fn accept(listener_sock_id: u64) -> Result<u64, i64> {
    let port = {
        let listeners = LISTENERS.lock();
        let st = listeners.get(&listener_sock_id).ok_or(-22i64)?;
        if !st.listening {
            return Err(-22);
        }
        st.port
    };

    let local_mac = NET_STACK.lock().nic.mac_address();
    let mut rx_buf = [0u8; PKT_BUF_SIZE];

    // Phase 1: Wait for SYN on bound port
    let (client_mac, client_ip, client_port, client_isn) = {
        let mut found = None;
        for _iter in 0..20_000_000u64 {
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
                if let Some((src_mac, src_ip, src_port, hdr, _)) = match_inbound_tcp(frame, port) {
                    if (hdr.flags & TCP_FLAG_SYN) != 0 && (hdr.flags & TCP_FLAG_ACK) == 0 {
                        found = Some((src_mac, src_ip, src_port, hdr.seq_num));
                        break;
                    }
                }
            } else {
                core::hint::spin_loop();
            }
        }
        found.ok_or(-11i64)? // -EAGAIN
    };

    let server_isn: u32 = ((read_tsc() as u32) ^ 0x5A3C_9E17).wrapping_add(1);
    let rcv_nxt = client_isn.wrapping_add(1);

    // Phase 2: Transmit SYN+ACK
    let mut synack = [0u8; 64];
    let synack_len = build_tcp_frame(
        local_mac,
        client_mac,
        current_ipv4(),
        client_ip,
        port,
        client_port,
        server_isn,
        rcv_nxt,
        TCP_FLAG_SYN | TCP_FLAG_ACK,
        16384,
        &[],
        &mut synack,
    );
    {
        let mut guard = NET_STACK.lock();
        if !guard.nic.transmit_frame(&synack[..synack_len]) {
            return Err(-5);
        }
    }

    let snd_nxt = server_isn.wrapping_add(1);

    // Phase 3: Wait for final ACK
    for _iter in 0..2_000_000u64 {
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
            if let Some((hdr, payload)) =
                parse_tcp_frame(frame, current_ipv4(), client_ip, port, client_port)
            {
                if (hdr.flags & TCP_FLAG_ACK) != 0
                    && hdr.ack_num == snd_nxt
                    && hdr.seq_num == rcv_nxt
                {
                    let final_rcv = rcv_nxt.wrapping_add(payload.len() as u32);
                    let client_sock = insert_accepted_connection(
                        port,
                        client_ip,
                        client_port,
                        local_mac,
                        client_mac,
                        snd_nxt,
                        final_rcv,
                        payload,
                    );
                    return Ok(client_sock);
                }
            }
        } else {
            core::hint::spin_loop();
        }
    }

    Err(-110) // -ETIMEDOUT
}

#[derive(Debug, Clone, Copy)]
struct PendingSyn {
    port: u16,
    local_mac: [u8; 6],
    client_mac: [u8; 6],
    snd_nxt: u32,
    rcv_nxt: u32,
}

static PENDING_SYNS: SpinLock<BTreeMap<([u8; 4], u16), PendingSyn>> =
    SpinLock::new(BTreeMap::new());
static RX_BACKLOG: SpinLock<VecDeque<Vec<u8>>> = SpinLock::new(VecDeque::new());

/// Enqueues an inbound TCP frame intercepted by `remote_shell` when passive listeners exist.
pub fn enqueue_rx_frame(frame: &[u8]) {
    let has_listener = LISTENERS.try_lock().map_or(false, |l| !l.is_empty());
    if !has_listener {
        return;
    }
    if let Some(mut q) = RX_BACKLOG.try_lock() {
        if q.len() < 16 {
            q.push_back(frame.to_vec());
        }
    }
}

/// Non-blocking single-step TCP accept for background kernel-polled daemons (IRQ-safe).
pub fn try_accept_nonblocking(listener_sock_id: u64) -> Option<u64> {
    let port = {
        let listeners = LISTENERS.try_lock()?;
        let st = listeners.get(&listener_sock_id)?;
        if !st.listening {
            return None;
        }
        st.port
    };

    let mut rx_buf = [0u8; PKT_BUF_SIZE];
    for _ in 0..8 {
        let local_mac = NET_STACK.try_lock()?.nic.mac_address();
        let queued = RX_BACKLOG.try_lock().and_then(|mut q| q.pop_front());
        let pkt_len = if let Some(qframe) = queued {
            let n = qframe.len().min(rx_buf.len());
            rx_buf[..n].copy_from_slice(&qframe[..n]);
            n
        } else {
            let mut guard = NET_STACK.try_lock()?;
            guard.nic.receive_frame(&mut rx_buf)?
        };
        let frame = &rx_buf[..pkt_len];
        if let Some(mut guard) = NET_STACK.try_lock() {
            let _ = inspect_and_answer_arp(&mut guard.nic, frame, local_mac);
        }
        let Some((src_mac, src_ip, src_port, hdr, payload)) = match_inbound_tcp(frame, port) else {
            continue;
        };

        if (hdr.flags & TCP_FLAG_SYN) != 0 && (hdr.flags & TCP_FLAG_ACK) == 0 {
            let server_isn: u32 = ((read_tsc() as u32) ^ 0x5A3C_9E17).wrapping_add(1);
            let rcv_nxt = hdr.seq_num.wrapping_add(1);
            let mut synack = [0u8; 64];
            let synack_len = build_tcp_frame(
                local_mac,
                src_mac,
                current_ipv4(),
                src_ip,
                port,
                src_port,
                server_isn,
                rcv_nxt,
                TCP_FLAG_SYN | TCP_FLAG_ACK,
                16384,
                &[],
                &mut synack,
            );
            if let Some(mut guard) = NET_STACK.try_lock() {
                if guard.nic.transmit_frame(&synack[..synack_len]) {
                    if let Some(mut syns) = PENDING_SYNS.try_lock() {
                        syns.insert(
                            (src_ip, src_port),
                            PendingSyn {
                                port,
                                local_mac,
                                client_mac: src_mac,
                                snd_nxt: server_isn.wrapping_add(1),
                                rcv_nxt,
                            },
                        );
                    }
                }
            }
        } else if (hdr.flags & TCP_FLAG_ACK) != 0 {
            let pending = PENDING_SYNS.try_lock().and_then(|mut syns| syns.remove(&(src_ip, src_port)));
            if let Some(p) = pending {
                if p.port == port && hdr.ack_num == p.snd_nxt && hdr.seq_num == p.rcv_nxt {
                    let final_rcv = p.rcv_nxt.wrapping_add(payload.len() as u32);
                    let sock = insert_accepted_connection(
                        port,
                        src_ip,
                        src_port,
                        p.local_mac,
                        p.client_mac,
                        p.snd_nxt,
                        final_rcv,
                        payload,
                    );
                    return Some(sock);
                }
            }
        }
    }
    None
}
