//! TCP Remote Console Server on Port 2222 (`ostd::net::remote_shell`).
//!
//! Provides a bidirectional remote terminal bridge on port 2222 with infinite multi-session
//! reconnection support (handling new `SYN`, clean `FIN`, and abrupt `RST`).

use crate::sync::SpinLock;
use super::arp_ipv4::{
    build_tcp_frame, inspect_and_answer_arp, ETHERTYPE_IPV4, IPPROTO_TCP,
};
use super::e1000::PKT_BUF_SIZE;
use super::tcp_client::{
    feed_client_socket, TCP_FLAG_ACK, TCP_FLAG_FIN, TCP_FLAG_PSH, TCP_FLAG_RST, TCP_FLAG_SYN,
};
use super::{current_ipv4, read_tsc, NET_STACK};

/// Well-known TCP port for the FipSOs Bare-Metal Remote Console Bridge.
pub const REMOTE_CONSOLE_PORT: u16 = 2222;
const REMOTE_BUF_SIZE: usize = 1024;

struct RemoteConsoleState {
    active: bool,
    established: bool,
    welcome_sent: bool,
    remote_ip: [u8; 4],
    remote_port: u16,
    local_mac: [u8; 6],
    remote_mac: [u8; 6],
    snd_nxt: u32,
    rcv_nxt: u32,
    rx_buf: [u8; REMOTE_BUF_SIZE],
    rx_head: usize,
    rx_tail: usize,
    rx_len: usize,
}

impl RemoteConsoleState {
    const fn new() -> Self {
        Self {
            active: false,
            established: false,
            welcome_sent: false,
            remote_ip: [0; 4],
            remote_port: 0,
            local_mac: [0; 6],
            remote_mac: [0; 6],
            snd_nxt: 0,
            rcv_nxt: 0,
            rx_buf: [0; REMOTE_BUF_SIZE],
            rx_head: 0,
            rx_tail: 0,
            rx_len: 0,
        }
    }

    fn reset_session(&mut self) {
        self.active = false;
        self.established = false;
        self.welcome_sent = false;
        self.remote_ip = [0; 4];
        self.remote_port = 0;
        self.snd_nxt = 0;
        self.rcv_nxt = 0;
        self.rx_head = 0;
        self.rx_tail = 0;
        self.rx_len = 0;
    }

    fn push_byte(&mut self, b: u8) {
        if self.rx_len < REMOTE_BUF_SIZE {
            self.rx_buf[self.rx_tail] = b;
            self.rx_tail = (self.rx_tail + 1) % REMOTE_BUF_SIZE;
            self.rx_len += 1;
        }
    }

    fn pop_byte(&mut self) -> Option<u8> {
        if self.rx_len == 0 {
            return None;
        }
        let b = self.rx_buf[self.rx_head];
        self.rx_head = (self.rx_head + 1) % REMOTE_BUF_SIZE;
        self.rx_len -= 1;
        Some(b)
    }
}

static REMOTE_CONSOLE: SpinLock<RemoteConsoleState> = SpinLock::new(RemoteConsoleState::new());

/// Polls `e1000e` NIC for incoming TCP frames targeting the remote console (port 2222),
/// completing the 3-way handshake or buffering input data.
pub fn poll_remote_console_tcp() {
    for _ in 0..16 {
        if !poll_one_remote_console_packet() {
            break;
        }
    }
}

fn poll_one_remote_console_packet() -> bool {
    let mut rx_buf = [0u8; PKT_BUF_SIZE];
    let frame_info = {
        let mut guard = match NET_STACK.try_lock() {
            Some(g) => g,
            None => return false,
        };
        if let Some(len) = guard.nic.receive_frame(&mut rx_buf) {
            let src_mac = guard.nic.mac_address();
            let _ = inspect_and_answer_arp(&mut guard.nic, &rx_buf[..len], src_mac);
            Some((len, src_mac))
        } else {
            None
        }
    };

    let Some((pkt_len, my_mac)) = frame_info else {
        return false;
    };

    let frame = &rx_buf[..pkt_len];
    if frame.len() < 54 {
        return true;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_IPV4 {
        return true;
    }
    let ip_hdr = &frame[14..];
    let ihl = ((ip_hdr[0] & 0x0F) as usize) * 4;
    if ip_hdr.len() < ihl + 20 || ip_hdr[9] != IPPROTO_TCP {
        return true;
    }
    let ip_total_len = (u16::from_be_bytes([ip_hdr[2], ip_hdr[3]]) as usize).min(frame.len() - 14);
    let src_ip = [ip_hdr[12], ip_hdr[13], ip_hdr[14], ip_hdr[15]];
    let dst_ip = [ip_hdr[16], ip_hdr[17], ip_hdr[18], ip_hdr[19]];
    let my_ip = current_ipv4();
    if dst_ip != my_ip && dst_ip != [255, 255, 255, 255] && dst_ip != [10, 0, 2, 15] {
        return true;
    }

    let tcp_hdr = &ip_hdr[ihl..];
    let src_port = u16::from_be_bytes([tcp_hdr[0], tcp_hdr[1]]);
    let dst_port = u16::from_be_bytes([tcp_hdr[2], tcp_hdr[3]]);

    if dst_port != REMOTE_CONSOLE_PORT {
        feed_client_socket(frame);
        return true;
    }

    let seq_num = u32::from_be_bytes([tcp_hdr[4], tcp_hdr[5], tcp_hdr[6], tcp_hdr[7]]);
    let _ack_num = u32::from_be_bytes([tcp_hdr[8], tcp_hdr[9], tcp_hdr[10], tcp_hdr[11]]);
    let data_offset = ((tcp_hdr[12] >> 4) as usize) * 4;
    let flags = tcp_hdr[13];
    let payload_len = ip_total_len.saturating_sub(ihl + data_offset);
    let sender_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];

    // 1. Connection initiation: SYN packet (supports infinite reconnection from any port/client)
    if (flags & TCP_FLAG_SYN) != 0 && (flags & TCP_FLAG_ACK) == 0 {
        let isn = ((read_tsc() as u32) ^ 0x3344_5566) & 0x7FFF_FFFF;
        let rcv_nxt = seq_num.wrapping_add(1);
        let snd_nxt = isn.wrapping_add(1);

        {
            let mut state = REMOTE_CONSOLE.lock();
            state.reset_session();
            state.active = true;
            state.established = true;
            state.welcome_sent = false;
            state.remote_ip = src_ip;
            state.remote_port = src_port;
            state.local_mac = my_mac;
            state.remote_mac = sender_mac;
            state.snd_nxt = snd_nxt;
            state.rcv_nxt = rcv_nxt;
        }

        // Send SYN+ACK response
        let mut syn_ack = [0u8; 64];
        let frame_len = build_tcp_frame(
            my_mac,
            sender_mac,
            my_ip,
            src_ip,
            REMOTE_CONSOLE_PORT,
            src_port,
            isn,
            rcv_nxt,
            TCP_FLAG_SYN | TCP_FLAG_ACK,
            16384,
            &[],
            &mut syn_ack,
        );
        if let Some(mut guard) = NET_STACK.try_lock() {
            let _ = guard.nic.transmit_frame(&syn_ack[..frame_len]);
        }

        crate::early_println!(
            "[ostd::net::remote_shell] Remote Console TCP :2222: Nueva sesion aceptada desde {}.{}.{}.{}:{} (SYN+ACK)",
            src_ip[0], src_ip[1], src_ip[2], src_ip[3], src_port
        );
        return true;
    }

    // 2. Incoming data, ACK handshake completion, or FIN / RST on active connection
    let mut state = REMOTE_CONSOLE.lock();
    if state.established && state.remote_port == src_port && state.remote_ip == src_ip {
        // Handle RST: immediate disconnect and reset to LISTEN
        if (flags & TCP_FLAG_RST) != 0 {
            state.reset_session();
            crate::early_println!(
                "[ostd::net::remote_shell] Remote Console TCP :2222: Sesion reseteada por cliente (RST) -> Estado LISTEN"
            );
            return true;
        }

        // Handle FIN: orderly termination with ACK and return to LISTEN
        if (flags & TCP_FLAG_FIN) != 0 {
            let next_ack = seq_num.wrapping_add(1);
            let mut fin_ack = [0u8; 64];
            let flen = build_tcp_frame(
                my_mac,
                sender_mac,
                my_ip,
                src_ip,
                REMOTE_CONSOLE_PORT,
                src_port,
                state.snd_nxt,
                next_ack,
                TCP_FLAG_ACK | TCP_FLAG_FIN,
                16384,
                &[],
                &mut fin_ack,
            );
            state.reset_session();
            drop(state);
            if let Some(mut guard) = NET_STACK.try_lock() {
                let _ = guard.nic.transmit_frame(&fin_ack[..flen]);
            }
            crate::early_println!(
                "[ostd::net::remote_shell] Remote Console TCP :2222: Sesion finalizada por cliente (FIN+ACK) -> Estado LISTEN"
            );
            return true;
        }

        // Check if welcome banner has not been transmitted yet (sent on 3-way ACK or first payload)
        if !state.welcome_sent && ((flags & TCP_FLAG_ACK) != 0 || payload_len > 0) {
            state.welcome_sent = true;
            let welcome = b"\r\n=== Conectado a FipSOs v3.4 Remote Shell (TCP :2222) ===\r\nfipsos:/# ";
            let mut banner_frame = alloc::vec![0u8; 14 + 20 + 20 + welcome.len()];
            let flen = build_tcp_frame(
                my_mac,
                sender_mac,
                my_ip,
                src_ip,
                REMOTE_CONSOLE_PORT,
                src_port,
                state.snd_nxt,
                state.rcv_nxt,
                TCP_FLAG_PSH | TCP_FLAG_ACK,
                16384,
                welcome,
                &mut banner_frame,
            );
            state.snd_nxt = state.snd_nxt.wrapping_add(welcome.len() as u32);
            if let Some(mut guard) = NET_STACK.try_lock() {
                let _ = guard.nic.transmit_frame(&banner_frame[..flen]);
            }
        }

        // Buffer incoming data payload and acknowledge
        if payload_len > 0 && ihl + data_offset + payload_len <= ip_hdr.len() {
            let payload = &ip_hdr[ihl + data_offset..ihl + data_offset + payload_len];
            for &b in payload {
                state.push_byte(b);
            }
            state.rcv_nxt = seq_num.wrapping_add(payload_len as u32);

            let mut ack_pkt = [0u8; 64];
            let flen = build_tcp_frame(
                my_mac,
                sender_mac,
                my_ip,
                src_ip,
                REMOTE_CONSOLE_PORT,
                src_port,
                state.snd_nxt,
                state.rcv_nxt,
                TCP_FLAG_ACK,
                16384,
                &[],
                &mut ack_pkt,
            );
            drop(state);
            if let Some(mut guard) = NET_STACK.try_lock() {
                let _ = guard.nic.transmit_frame(&ack_pkt[..flen]);
            }
        }
        return true;
    }

    false
}

/// Pops a received byte from the TCP Remote Console stream buffer.
pub fn pop_remote_console_byte() -> Option<u8> {
    REMOTE_CONSOLE.lock().pop_byte()
}

/// Broadcasts console output to the connected TCP Remote Console client on port 2222.
pub fn remote_console_broadcast(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let snap = {
        let mut state = REMOTE_CONSOLE.lock();
        if !state.established {
            return;
        }
        let res = (
            state.local_mac,
            state.remote_mac,
            state.remote_ip,
            state.remote_port,
            state.snd_nxt,
            state.rcv_nxt,
        );
        state.snd_nxt = state.snd_nxt.wrapping_add(data.len() as u32);
        res
    };

    let (src_mac, dst_mac, remote_ip, remote_port, snd_nxt, rcv_nxt) = snap;
    let my_ip = current_ipv4();
    let mut frame = alloc::vec![0u8; 14 + 20 + 20 + data.len()];
    let wire_len = build_tcp_frame(
        src_mac,
        dst_mac,
        my_ip,
        remote_ip,
        REMOTE_CONSOLE_PORT,
        remote_port,
        snd_nxt,
        rcv_nxt,
        TCP_FLAG_PSH | TCP_FLAG_ACK,
        16384,
        data,
        &mut frame,
    );

    if let Some(mut guard) = NET_STACK.try_lock() {
        let _ = guard.nic.transmit_frame(&frame[..wire_len]);
    }
}
