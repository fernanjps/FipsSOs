//! Ethernet II, ARP, IPv4, and ICMP Packet Parsing & Serialization (`ostd::net::arp_ipv4`).

use alloc::vec::Vec;
use super::current_gateway_ipv4;
use super::current_ipv4;
use super::e1000::E1000Controller;

pub const ETH_HEADER_LEN: usize = 14;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const IPPROTO_ICMP: u8 = 1;
pub const IPPROTO_TCP: u8 = 6;
pub const IPPROTO_UDP: u8 = 17;

/// Inspects an incoming Ethernet frame for ARP (`0x0806`):
/// - If it is an ARP Reply (`oper == 2`) from `10.0.2.2`, returns `Some(sender_mac)`.
/// - If it is an ARP Request (`oper == 1`) asking for `10.0.2.15`, transmits an ARP Reply (`oper == 2`)
///   and learns the requester's MAC if from `10.0.2.2`.
pub fn inspect_and_answer_arp(
    nic: &mut E1000Controller,
    frame: &[u8],
    our_mac: [u8; 6],
) -> Option<[u8; 6]> {
    if frame.len() < 42 {
        return None;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_ARP {
        return None;
    }
    let oper = u16::from_be_bytes([frame[20], frame[21]]);
    let mut sha = [0u8; 6];
    sha.copy_from_slice(&frame[22..28]);
    let spa = [frame[28], frame[29], frame[30], frame[31]];
    let tpa = [frame[38], frame[39], frame[40], frame[41]];

    let my_ip = current_ipv4();
    let gw_ip = current_gateway_ipv4();

    if oper == 1 && (tpa == my_ip || tpa == [10, 0, 2, 15]) {
        let arp_reply = build_arp_frame(our_mac, my_ip, sha, sha, spa, 2);
        let _ = nic.transmit_frame(&arp_reply);
        crate::early_println!("[ostd::net] ARP Request WHO-HAS answered with ARP Reply");
        return Some(sha);
    } else if oper == 2 && (spa == gw_ip || spa == [10, 0, 2, 2]) {
        return Some(sha);
    }
    None
}

pub fn is_icmp_echo_reply(frame: &[u8], expected_seq: u16) -> bool {
    if frame.len() < 42 {
        return false;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_IPV4 {
        return false;
    }
    let ihl = ((frame[14] & 0x0F) as usize) * 4;
    if ihl < 20 || frame.len() < 14 + ihl + 8 {
        return false;
    }
    if frame[23] != IPPROTO_ICMP {
        return false;
    }
    let icmp = &frame[14 + ihl..];
    let icmp_type = icmp[0];
    let icmp_id = u16::from_be_bytes([icmp[4], icmp[5]]);
    let icmp_seq = u16::from_be_bytes([icmp[6], icmp[7]]);
    icmp_type == 0 && (icmp_id == 0xF195 || icmp_seq == expected_seq)
}

pub fn extract_http_payload(frame: &[u8]) -> Option<Vec<u8>> {
    if frame.len() < 54 {
        return None;
    }
    if u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_IPV4 {
        return None;
    }
    let ihl = ((frame[14] & 0x0F) as usize) * 4;
    if ihl < 20 || frame.len() <= 14 + ihl {
        return None;
    }
    let proto = frame[23];
    let l4 = &frame[14 + ihl..];
    let app_data = if proto == IPPROTO_TCP {
        if l4.len() < 20 {
            return None;
        }
        let data_off = ((l4[12] >> 4) as usize) * 4;
        if data_off < 20 || l4.len() <= data_off {
            return None;
        }
        &l4[data_off..]
    } else if proto == IPPROTO_UDP {
        if l4.len() <= 8 {
            return None;
        }
        &l4[8..]
    } else {
        return None;
    };

    if app_data.starts_with(b"HTTP/1.") {
        Some(app_data.to_vec())
    } else {
        None
    }
}

pub fn build_arp_frame(
    src_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_mac: [u8; 6],
    tha: [u8; 6],
    tpa: [u8; 4],
    oper: u16,
) -> [u8; 60] {
    let mut pkt = [0u8; 60];
    pkt[0..6].copy_from_slice(&dst_mac);
    pkt[6..12].copy_from_slice(&src_mac);
    pkt[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
    pkt[14..16].copy_from_slice(&1u16.to_be_bytes()); // HTYPE = Ethernet (1)
    pkt[16..18].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes()); // PTYPE = IPv4 (0x0800)
    pkt[18] = 6; // HLEN = 6
    pkt[19] = 4; // PLEN = 4
    pkt[20..22].copy_from_slice(&oper.to_be_bytes());
    pkt[22..28].copy_from_slice(&src_mac);
    pkt[28..32].copy_from_slice(&src_ip);
    pkt[32..38].copy_from_slice(&tha);
    pkt[38..42].copy_from_slice(&tpa);
    pkt
}

pub fn build_icmp_echo_frame(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    seq: u16,
    payload: &[u8],
    out: &mut [u8],
) -> usize {
    let ip_total_len = 20 + 8 + payload.len();
    let total_len = 14 + ip_total_len;

    out[0..6].copy_from_slice(&dst_mac);
    out[6..12].copy_from_slice(&src_mac);
    out[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let ip = &mut out[14..34];
    ip.fill(0);
    ip[0] = 0x45; // IPv4, IHL=5 (20B)
    ip[2..4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&seq.to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't Fragment
    ip[8] = 64; // TTL
    ip[9] = IPPROTO_ICMP;
    ip[12..16].copy_from_slice(&src_ip);
    ip[16..20].copy_from_slice(&dst_ip);
    let ip_csum = internet_checksum(ip);
    ip[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    let icmp = &mut out[34..total_len];
    icmp[0] = 8; // Echo Request
    icmp[1] = 0;
    icmp[2..4].copy_from_slice(&0u16.to_be_bytes());
    icmp[4..6].copy_from_slice(&0xF195u16.to_be_bytes());
    icmp[6..8].copy_from_slice(&seq.to_be_bytes());
    icmp[8..8 + payload.len()].copy_from_slice(payload);
    let icmp_csum = internet_checksum(icmp);
    icmp[2..4].copy_from_slice(&icmp_csum.to_be_bytes());

    total_len
}

#[allow(clippy::too_many_arguments)]
pub fn build_udp_payload_frame(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
    out: &mut [u8],
) -> usize {
    let udp_len = 8 + payload.len();
    let ip_total_len = 20 + udp_len;
    let total_len = 14 + ip_total_len;

    out[0..6].copy_from_slice(&dst_mac);
    out[6..12].copy_from_slice(&src_mac);
    out[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let ip = &mut out[14..34];
    ip.fill(0);
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&0x2026u16.to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = IPPROTO_UDP;
    ip[12..16].copy_from_slice(&src_ip);
    ip[16..20].copy_from_slice(&dst_ip);
    let ip_csum = internet_checksum(ip);
    ip[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    let udp = &mut out[34..total_len];
    udp[0..2].copy_from_slice(&src_port.to_be_bytes());
    udp[2..4].copy_from_slice(&dst_port.to_be_bytes());
    udp[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    udp[6..8].copy_from_slice(&0u16.to_be_bytes());
    udp[8..8 + payload.len()].copy_from_slice(payload);
    let udp_csum = l4_pseudo_checksum(src_ip, dst_ip, IPPROTO_UDP, udp);
    udp[6..8].copy_from_slice(&udp_csum.to_be_bytes());

    total_len
}

/// Standard RFC 1071 16-bit ones' complement Internet Checksum.
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for pair in &mut chunks {
        sum += u16::from_be_bytes([pair[0], pair[1]]) as u32;
    }
    if let &[rem] = chunks.remainder() {
        sum += ((rem as u16) << 8) as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn l4_pseudo_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], proto: u8, l4: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += proto as u32;
    sum += l4.len() as u32;

    let mut chunks = l4.chunks_exact(2);
    for pair in &mut chunks {
        sum += u16::from_be_bytes([pair[0], pair[1]]) as u32;
    }
    if let &[rem] = chunks.remainder() {
        sum += ((rem as u16) << 8) as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let res = !(sum as u16);
    if proto == IPPROTO_UDP && res == 0 {
        0xFFFF
    } else {
        res
    }
}

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
    let ip_csum = internet_checksum(ip);
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
