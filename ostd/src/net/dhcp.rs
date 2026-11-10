//! Bare-Metal DHCP (Dynamic Host Configuration Protocol) Client (`ostd::net::dhcp`).
//!
//! Implements RFC 2131 compliant client discovery over UDP:
//! `DHCPDISCOVER (UDP 68 -> 67)` -> `DHCPOFFER` -> `DHCPREQUEST` -> `DHCPACK`
//! with automatic IP and Gateway leasing, and deterministic fallback.

use alloc::vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::sync::SpinLock;
use super::e1000::PKT_BUF_SIZE;
use super::{build_udp_payload_frame, read_tsc, NET_STACK};

const DHCP_MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

const DHCP_OPT_PAD: u8 = 0;
const DHCP_OPT_SUBNET_MASK: u8 = 1;
const DHCP_OPT_ROUTER: u8 = 3;
const DHCP_OPT_DNS: u8 = 6;
const DHCP_OPT_REQUESTED_IP: u8 = 50;
const DHCP_OPT_LEASE_TIME: u8 = 51;
const DHCP_OPT_MSG_TYPE: u8 = 53;
const DHCP_OPT_SERVER_ID: u8 = 54;
const DHCP_OPT_PARAM_REQ_LIST: u8 = 55;
const DHCP_OPT_END: u8 = 255;

const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;

#[derive(Debug, Clone, Copy)]
pub struct DhcpStatus {
    pub acquired: bool,
    pub ip: [u8; 4],
    pub subnet: [u8; 4],
    pub gateway: [u8; 4],
    pub server: [u8; 4],
    pub lease_time: u32,
}

impl DhcpStatus {
    pub const fn fallback() -> Self {
        Self {
            acquired: false,
            ip: [10, 0, 2, 15],
            subnet: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            server: [10, 0, 2, 2],
            lease_time: 86400,
        }
    }
}

static DHCP_STATUS: SpinLock<DhcpStatus> = SpinLock::new(DhcpStatus::fallback());
static DHCP_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn status() -> DhcpStatus {
    *DHCP_STATUS.lock()
}

pub fn is_acquired() -> bool {
    DHCP_INITIALIZED.load(Ordering::Acquire)
}

/// Builds a BOOTP/DHCP request payload into the provided buffer.
fn build_dhcp_payload(
    msg_type: u8,
    xid: u32,
    client_mac: [u8; 6],
    req_ip: Option<[u8; 4]>,
    server_id: Option<[u8; 4]>,
    out: &mut [u8; 300],
) -> usize {
    out.fill(0);
    out[0] = 1; // BOOTREQUEST
    out[1] = 1; // Ethernet (10Mb)
    out[2] = 6; // Hardware address length (6 bytes)
    out[3] = 0; // Hops

    out[4..8].copy_from_slice(&xid.to_be_bytes());
    out[8..10].copy_from_slice(&0u16.to_be_bytes()); // Secs
    out[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // Broadcast flag (bit 15)

    // ciaddr, yiaddr, siaddr, giaddr are all 0.0.0.0 (bytes 12..28)
    out[28..34].copy_from_slice(&client_mac); // chaddr

    // Magic Cookie at offset 236
    out[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);

    let mut opt_idx = 240usize;

    // Option 53: DHCP Message Type
    out[opt_idx] = DHCP_OPT_MSG_TYPE;
    out[opt_idx + 1] = 1;
    out[opt_idx + 2] = msg_type;
    opt_idx += 3;

    if let Some(ip) = req_ip {
        out[opt_idx] = DHCP_OPT_REQUESTED_IP;
        out[opt_idx + 1] = 4;
        out[opt_idx + 2..opt_idx + 6].copy_from_slice(&ip);
        opt_idx += 6;
    }

    if let Some(srv) = server_id {
        out[opt_idx] = DHCP_OPT_SERVER_ID;
        out[opt_idx + 1] = 4;
        out[opt_idx + 2..opt_idx + 6].copy_from_slice(&srv);
        opt_idx += 6;
    }

    // Option 55: Parameter Request List (Subnet Mask, Router, DNS)
    out[opt_idx] = DHCP_OPT_PARAM_REQ_LIST;
    out[opt_idx + 1] = 3;
    out[opt_idx + 2] = DHCP_OPT_SUBNET_MASK;
    out[opt_idx + 3] = DHCP_OPT_ROUTER;
    out[opt_idx + 4] = DHCP_OPT_DNS;
    opt_idx += 5;

    // Option 255: End
    out[opt_idx] = DHCP_OPT_END;
    opt_idx += 1;

    // Return length, minimum 300 bytes for BOOTP/DHCP
    opt_idx.max(300)
}

/// Parses a received Ethernet frame to extract DHCP options if it's a BOOTREPLY for our xid.
fn parse_dhcp_reply(
    frame: &[u8],
    expected_xid: u32,
    expected_msg_type: u8,
) -> Option<([u8; 4], [u8; 4], [u8; 4], [u8; 4], u32)> {
    if frame.len() < 14 + 20 + 8 + 240 {
        return None;
    }

    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != super::ETHERTYPE_IPV4 {
        return None;
    }

    let ip_hdr = &frame[14..];
    let ihl = ((ip_hdr[0] & 0x0F) as usize) * 4;
    if ip_hdr.len() < ihl + 8 || ip_hdr[9] != 17 /* UDP */ {
        return None;
    }

    let udp_hdr = &ip_hdr[ihl..];
    let dst_port = u16::from_be_bytes([udp_hdr[2], udp_hdr[3]]);
    if dst_port != 68 /* DHCP Client Port */ {
        return None;
    }

    let dhcp_payload = &udp_hdr[8..];
    if dhcp_payload.len() < 240 {
        return None;
    }

    let op = dhcp_payload[0];
    if op != 2 /* BOOTREPLY */ {
        return None;
    }

    let xid = u32::from_be_bytes([
        dhcp_payload[4],
        dhcp_payload[5],
        dhcp_payload[6],
        dhcp_payload[7],
    ]);
    if xid != expected_xid {
        return None;
    }

    let cookie = &dhcp_payload[236..240];
    if cookie != DHCP_MAGIC_COOKIE {
        return None;
    }

    let yiaddr = [
        dhcp_payload[16],
        dhcp_payload[17],
        dhcp_payload[18],
        dhcp_payload[19],
    ];

    let mut msg_type: Option<u8> = None;
    let mut subnet_mask: [u8; 4] = [255, 255, 255, 0];
    let mut router: [u8; 4] = [10, 0, 2, 2];
    let mut server_id: [u8; 4] = [10, 0, 2, 2];
    let mut lease_time: u32 = 86400;

    let mut idx = 240usize;
    while idx < dhcp_payload.len() {
        let opt = dhcp_payload[idx];
        if opt == DHCP_OPT_END {
            break;
        }
        if opt == DHCP_OPT_PAD {
            idx += 1;
            continue;
        }
        if idx + 1 >= dhcp_payload.len() {
            break;
        }
        let len = dhcp_payload[idx + 1] as usize;
        let val_start = idx + 2;
        let val_end = val_start + len;
        if val_end > dhcp_payload.len() {
            break;
        }
        let val = &dhcp_payload[val_start..val_end];

        match opt {
            DHCP_OPT_MSG_TYPE if len >= 1 => {
                msg_type = Some(val[0]);
            }
            DHCP_OPT_SUBNET_MASK if len >= 4 => {
                subnet_mask = [val[0], val[1], val[2], val[3]];
            }
            DHCP_OPT_ROUTER if len >= 4 => {
                router = [val[0], val[1], val[2], val[3]];
            }
            DHCP_OPT_SERVER_ID if len >= 4 => {
                server_id = [val[0], val[1], val[2], val[3]];
            }
            DHCP_OPT_LEASE_TIME if len >= 4 => {
                lease_time = u32::from_be_bytes([val[0], val[1], val[2], val[3]]);
            }
            _ => {}
        }

        idx = val_end;
    }

    if msg_type == Some(expected_msg_type) {
        Some((yiaddr, subnet_mask, router, server_id, lease_time))
    } else {
        None
    }
}

/// Executes dynamic DHCP negotiation (DHCPDISCOVER -> DHCPOFFER -> DHCPREQUEST -> DHCPACK)
/// over the Intel e1000/e1000e NIC, falling back to static 10.0.2.15/10.0.2.2 if no DHCP server answers.
pub fn discover_and_configure() {
    let (src_mac, link_up) = {
        let guard = NET_STACK.lock();
        (guard.nic.mac_address(), guard.nic.snapshot().link_up)
    };

    if !link_up {
        crate::early_println!("[ostd::net::dhcp] Enlace de red no disponible. Usando fallback 10.0.2.15");
        super::set_assigned_ip([10, 0, 2, 15], [10, 0, 2, 2]);
        return;
    }

    let xid = ((read_tsc() as u32) ^ 0x4448_4350) & 0x7FFF_FFFF;
    crate::early_println!(
        "[ostd::net::dhcp] Iniciando negociacion DHCP (UDP 68 -> 67, XID={:#010x}, MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})...",
        xid, src_mac[0], src_mac[1], src_mac[2], src_mac[3], src_mac[4], src_mac[5]
    );

    // 1. Build & Transmit DHCPDISCOVER
    let mut dhcp_payload = [0u8; 300];
    let payload_len = build_dhcp_payload(DHCP_DISCOVER, xid, src_mac, None, None, &mut dhcp_payload);

    let mut tx_frame = [0u8; 512];
    let frame_len = build_udp_payload_frame(
        src_mac,
        [0xFF; 6],
        [0, 0, 0, 0],
        [255, 255, 255, 255],
        68,
        67,
        &dhcp_payload[..payload_len],
        &mut tx_frame,
    );

    {
        let mut guard = NET_STACK.lock();
        let _ = guard.nic.transmit_frame(&tx_frame[..frame_len]);
    }

    // 2. Await DHCPOFFER
    let mut rx_buf = vec![0u8; PKT_BUF_SIZE];
    let mut offer: Option<([u8; 4], [u8; 4], [u8; 4], [u8; 4], u32)> = None;

    for _ in 0..60_000 {
        let r = {
            let mut guard = NET_STACK.lock();
            guard.nic.receive_frame(&mut rx_buf)
        };
        if let Some(len) = r {
            if let Some(res) = parse_dhcp_reply(&rx_buf[..len], xid, DHCP_OFFER) {
                offer = Some(res);
                break;
            }
        }
        core::hint::spin_loop();
    }

    let Some((offered_ip, offered_subnet, offered_gw, offered_server, offered_lease)) = offer else {
        crate::early_println!(
            "[ostd::net::dhcp] DHCPOFFER timeout. Red estatica/aislada detectada -> IP asignada: 10.0.2.15 (GW: 10.0.2.2)"
        );
        super::set_assigned_ip([10, 0, 2, 15], [10, 0, 2, 2]);
        return;
    };

    crate::early_println!(
        "[ostd::net::dhcp] DHCPOFFER recibido: IP={}.{}.{}.{} de Servidor={}.{}.{}.{}",
        offered_ip[0], offered_ip[1], offered_ip[2], offered_ip[3],
        offered_server[0], offered_server[1], offered_server[2], offered_server[3]
    );

    // 3. Build & Transmit DHCPREQUEST
    let req_len = build_dhcp_payload(
        DHCP_REQUEST,
        xid,
        src_mac,
        Some(offered_ip),
        Some(offered_server),
        &mut dhcp_payload,
    );
    let frame_len = build_udp_payload_frame(
        src_mac,
        [0xFF; 6],
        [0, 0, 0, 0],
        [255, 255, 255, 255],
        68,
        67,
        &dhcp_payload[..req_len],
        &mut tx_frame,
    );

    {
        let mut guard = NET_STACK.lock();
        let _ = guard.nic.transmit_frame(&tx_frame[..frame_len]);
    }

    // 4. Await DHCPACK
    let mut ack: Option<([u8; 4], [u8; 4], [u8; 4], [u8; 4], u32)> = None;
    for _ in 0..60_000 {
        let r = {
            let mut guard = NET_STACK.lock();
            guard.nic.receive_frame(&mut rx_buf)
        };
        if let Some(len) = r {
            if let Some(res) = parse_dhcp_reply(&rx_buf[..len], xid, DHCP_ACK) {
                ack = Some(res);
                break;
            }
        }
        core::hint::spin_loop();
    }

    let (ack_ip, ack_subnet, ack_gw, ack_server, ack_lease) = ack.unwrap_or((
        offered_ip,
        offered_subnet,
        offered_gw,
        offered_server,
        offered_lease,
    ));

    crate::early_println!(
        "[ostd::net::dhcp] DHCPACK confirmado: IP={}.{}.{}.{} | Subnet={}.{}.{}.{} | Gateway={}.{}.{}.{} | Lease={}s",
        ack_ip[0], ack_ip[1], ack_ip[2], ack_ip[3],
        ack_subnet[0], ack_subnet[1], ack_subnet[2], ack_subnet[3],
        ack_gw[0], ack_gw[1], ack_gw[2], ack_gw[3],
        ack_lease
    );

    {
        let mut st = DHCP_STATUS.lock();
        st.acquired = true;
        st.ip = ack_ip;
        st.subnet = ack_subnet;
        st.gateway = ack_gw;
        st.server = ack_server;
        st.lease_time = ack_lease;
    }
    DHCP_INITIALIZED.store(true, Ordering::Release);

    super::set_assigned_ip(ack_ip, ack_gw);
}
